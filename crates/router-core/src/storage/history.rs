use std::{collections::BTreeMap, fs, sync::Arc};

use chrono::{
    DateTime, Datelike, Duration as ChronoDuration, LocalResult, NaiveDate, NaiveDateTime,
    TimeZone, Timelike, Utc,
};
use chrono_tz::Tz;
use rusqlite::{OptionalExtension, params};

use super::{
    AttemptRole, AttemptRoutingTransition, AttemptRoutingTransitionRecord, ClearHistoryResult,
    DatabaseExecutor, FallbackStopReason, FallbackStopRecord, HistorySummary,
    LatestInferenceAttempt, RequestHistoryRecord, RoutingDecision, RoutingTransitionKind,
    RoutingTransitionSkip, StorageError, UsageAttemptDetail, UsageHistoryCursor, UsageHistoryPage,
    UsageHistoryQuery, UsageHistoryRow, UsageRequestDetail, UsageRouteOption, UsageStatistics,
    UsageStatisticsAttribution, UsageStatisticsAttributionDimension,
    UsageStatisticsAttributionMetric, UsageStatisticsBucket, UsageStatisticsGranularity,
    UsageStatisticsQuery, UsageStatisticsTokens,
};
use crate::domain::{CompletionState, DeliveryState, ModelVerdict, RouteId, model_verdict};
use crate::pricing::{CostStatus, PricedUsage, UsageObservation, fold_request_cost};

impl DatabaseExecutor {
    /// Deletes requests older than the UTC cutoff and reclaims free pages.
    ///
    /// # Errors
    ///
    /// Returns an executor, deletion, or incremental-vacuum error.
    pub async fn cleanup_history(&self, cutoff_ms: i64) -> Result<u64, StorageError> {
        self.call(move |connection| {
            let transaction = connection.transaction()?;
            let deleted = transaction.execute(
                "DELETE FROM proxy_requests WHERE started_at_ms < ?1",
                [cutoff_ms],
            )?;
            transaction.commit()?;
            connection.execute_batch("PRAGMA incremental_vacuum")?;
            Ok(deleted as u64)
        })
        .await
    }

    /// Deletes all request metadata and reclaims free pages.
    ///
    /// # Errors
    ///
    /// Returns an executor, deletion, or incremental-vacuum error.
    pub async fn clear_history(&self) -> Result<ClearHistoryResult, StorageError> {
        self.call(|connection| {
            let transaction = connection.transaction()?;
            let deleted = transaction.execute("DELETE FROM proxy_requests", [])?;
            transaction.commit()?;
            let reclaim_succeeded = connection
                .execute_batch("PRAGMA incremental_vacuum")
                .is_ok();
            Ok(ClearHistoryResult {
                deleted_requests: deleted as u64,
                reclaim_succeeded,
            })
        })
        .await
    }

    /// Returns the bounded aggregate used by the settings screen.
    ///
    /// # Errors
    ///
    /// Returns an executor or `SQLite` aggregate query error.
    pub async fn history_summary(&self) -> Result<HistorySummary, StorageError> {
        let database_bytes = fs::metadata(self.path.as_ref()).map_or(0, |metadata| metadata.len());
        self.call(move |connection| {
            let (count, earliest, latest) = connection.query_row(
                "SELECT COUNT(*), MIN(started_at_ms), MAX(started_at_ms) FROM proxy_requests",
                [],
                |row| Ok((row.get::<_, i64>(0)?, row.get(1)?, row.get(2)?)),
            )?;
            Ok(HistorySummary {
                request_count: u64::try_from(count).unwrap_or_default(),
                earliest_started_at_ms: earliest,
                latest_started_at_ms: latest,
                database_bytes,
                retention_days: 365,
            })
        })
        .await
    }

    /// Returns one validated, newest-first keyset page of retained usage.
    ///
    /// # Errors
    ///
    /// Returns an invalid-query, executor, or database error.
    #[expect(
        clippy::too_many_lines,
        reason = "validation, count, and keyset selection intentionally share one query contract"
    )]
    pub async fn usage_history(
        &self,
        query: UsageHistoryQuery,
    ) -> Result<UsageHistoryPage, StorageError> {
        if query.limit == 0
            || query.limit > 100
            || query.finished_at_or_before_ms < 0
            || query
                .finished_at_or_after_ms
                .is_some_and(|lower| lower < 0 || lower > query.finished_at_or_before_ms)
        {
            return Err(StorageError::InvalidUsageQuery);
        }
        if query
            .model_contains
            .as_ref()
            .is_some_and(|model| model.is_empty() || model.len() > 256)
        {
            return Err(StorageError::InvalidUsageQuery);
        }
        if query.cursor.as_ref().is_some_and(|cursor| {
            cursor.finished_at_ms < 0
                || cursor.request_id.is_empty()
                || cursor.request_id.len() > 128
        }) {
            return Err(StorageError::InvalidUsageQuery);
        }
        self.call(move |connection| {
            let completion = query.completion_state.as_ref().map(completion_state_value);
            let route = query.route_id.as_ref().map(RouteId::as_str);
            let model_pattern = query
                .model_contains
                .as_deref()
                .map(literal_contains_pattern);
            let cursor_time = query.cursor.as_ref().map(|cursor| cursor.finished_at_ms);
            let cursor_id = query.cursor.as_ref().map(|cursor| cursor.request_id.as_str());
            let total_rows: i64 = connection.query_row(
                "SELECT COUNT(*) FROM proxy_requests
                 WHERE finished_at_ms IS NOT NULL
                   AND (?1 IS NULL OR finished_at_ms >= ?1)
                   AND finished_at_ms <= ?2
                   AND (?3 IS NULL OR completion_state = ?3)
                   AND (?4 IS NULL OR final_route_id = ?4)
                   AND (?5 IS NULL OR requested_model LIKE ?5 ESCAPE '\\' COLLATE NOCASE
                        OR actual_model LIKE ?5 ESCAPE '\\' COLLATE NOCASE)",
                params![
                    query.finished_at_or_after_ms,
                    query.finished_at_or_before_ms,
                    completion,
                    route,
                    model_pattern.as_deref(),
                ],
                |row| row.get(0),
            )?;
            let mut statement = connection.prepare(
                "SELECT request_id, started_at_ms, finished_at_ms, final_route_id, final_route_name,
                        requested_model, actual_model, reasoning_effort, streaming,
                        completion_state, http_status,
                        input_tokens, output_tokens, total_tokens, cached_input_tokens,
                        cache_write_input_tokens, total_latency_ms, first_output_latency_ms,
                        pricing_catalog_version, cost_status, upstream_cost_pico_usd,
                        actual_service_tier
                 FROM proxy_requests
                 WHERE finished_at_ms IS NOT NULL
                   AND (?1 IS NULL OR finished_at_ms >= ?1)
                   AND finished_at_ms <= ?2
                   AND (?3 IS NULL OR completion_state = ?3)
                   AND (?4 IS NULL OR final_route_id = ?4)
                   AND (?5 IS NULL OR requested_model LIKE ?5 ESCAPE '\\' COLLATE NOCASE
                        OR actual_model LIKE ?5 ESCAPE '\\' COLLATE NOCASE)
                   AND (?6 IS NULL OR finished_at_ms < ?6 OR (finished_at_ms = ?6 AND request_id < ?7))
                 ORDER BY finished_at_ms DESC, request_id DESC LIMIT ?8",
            )?;
            let fetch_limit = i64::from(query.limit) + 1;
            let rows = statement.query_map(
                params![
                    query.finished_at_or_after_ms,
                    query.finished_at_or_before_ms,
                    completion,
                    route,
                    model_pattern,
                    cursor_time,
                    cursor_id,
                    fetch_limit,
                ],
                |row| usage_history_row(row, row.get(21)?),
            )?;
            let mut rows = rows.collect::<Result<Vec<_>, _>>()?;
            let has_more = rows.len() > usize::from(query.limit);
            rows.truncate(usize::from(query.limit));
            let next_cursor = if has_more {
                rows.last().and_then(|last| {
                    last.finished_at_ms.map(|finished_at_ms| UsageHistoryCursor {
                        finished_at_ms,
                        request_id: last.request_id.clone(),
                    })
                })
            } else {
                None
            };
            Ok(UsageHistoryPage {
                rows,
                next_cursor,
                total_rows: u64::try_from(total_rows).unwrap_or_default(),
            })
        })
        .await
    }

    /// Returns successful-request aggregates for one anchored Usage filter snapshot.
    ///
    /// # Errors
    ///
    /// Returns an invalid-query, checked-overflow, executor, or database error.
    #[expect(
        clippy::too_many_lines,
        reason = "the bounded query and its single-pass aggregate stay in one executor closure"
    )]
    pub async fn usage_statistics(
        &self,
        query: UsageStatisticsQuery,
    ) -> Result<UsageStatistics, StorageError> {
        validate_usage_statistics_query(&query)?;
        let time_zone = query
            .time_zone
            .parse::<Tz>()
            .map_err(|_| StorageError::InvalidUsageQuery)?;
        let granularity = statistics_granularity(&query);
        self.call(move |connection| {
            let route = query.route_id.as_ref().map(RouteId::as_str);
            let model_pattern = query
                .model_contains
                .as_deref()
                .map(literal_contains_pattern);
            let mut statement = connection.prepare(
                "SELECT finished_at_ms, request_id, final_route_id, final_route_name,
                        requested_model, actual_model, input_tokens, cached_input_tokens,
                        cache_write_input_tokens, output_tokens, total_tokens,
                        upstream_cost_pico_usd, MIN(finished_at_ms) OVER ()
                 FROM proxy_requests
                 WHERE finished_at_ms IS NOT NULL
                   AND completion_state = 'completed'
                   AND (?1 IS NULL OR finished_at_ms >= ?1)
                   AND finished_at_ms <= ?2
                   AND (?3 IS NULL OR final_route_id = ?3)
                   AND (?4 IS NULL OR requested_model LIKE ?4 ESCAPE '\\' COLLATE NOCASE
                        OR actual_model LIKE ?4 ESCAPE '\\' COLLATE NOCASE)
                 ORDER BY finished_at_ms DESC, request_id DESC",
            )?;
            let mut rows = statement.query(params![
                query.finished_at_or_after_ms,
                query.finished_at_or_before_ms,
                route,
                model_pattern,
            ])?;
            let mut totals = StatisticsTotals::default();
            let mut bucket_windows = Vec::new();
            let mut bucket_totals = Vec::new();
            let mut attribution = BTreeMap::<String, AttributionAggregate>::new();
            while let Some(row) = rows.next()? {
                let finished_at_ms = row.get::<_, i64>(0)?;
                if bucket_windows.is_empty() {
                    let earliest = row.get::<_, i64>(12)?;
                    let lower = query.finished_at_or_after_ms.unwrap_or(earliest);
                    bucket_windows = statistics_bucket_windows(
                        lower,
                        query.finished_at_or_before_ms,
                        time_zone,
                        granularity,
                    )?;
                    bucket_totals.resize(bucket_windows.len(), StatisticsTotals::default());
                }
                let requested_model = row.get::<_, Option<String>>(4)?;
                let actual_model = row.get::<_, Option<String>>(5)?;
                let redirected = model_verdict(requested_model.as_deref(), actual_model.as_deref())
                    == ModelVerdict::Redirected;
                let observation = StatisticsObservation {
                    input_tokens: row.get(6)?,
                    cached_input_tokens: row.get(7)?,
                    cache_write_input_tokens: row.get(8)?,
                    output_tokens: row.get(9)?,
                    total_tokens: row.get(10)?,
                    cost_pico_usd: row.get(11)?,
                };
                totals.add(&observation, redirected)?;
                if let Some(index) = bucket_windows.iter().position(|window| {
                    finished_at_ms >= window.started_at_ms
                        && (finished_at_ms < window.finished_at_ms
                            || (window.finished_at_ms == query.finished_at_or_before_ms
                                && finished_at_ms == window.finished_at_ms))
                }) {
                    bucket_totals[index].add(&observation, redirected)?;
                }
                let identity = attribution_identity(
                    query.attribution_dimension,
                    row.get(2)?,
                    row.get(3)?,
                    requested_model,
                    actual_model,
                );
                attribution
                    .entry(identity.key)
                    .and_modify(|aggregate| {
                        if aggregate.label.starts_with("未知")
                            && !identity.label.starts_with("未知")
                        {
                            aggregate.label.clone_from(&identity.label);
                        }
                    })
                    .or_insert_with(|| AttributionAggregate {
                        label: identity.label,
                        totals: StatisticsTotals::default(),
                    })
                    .totals
                    .add(&observation, redirected)?;
            }
            let trend = bucket_windows
                .into_iter()
                .zip(bucket_totals)
                .map(|(window, totals)| UsageStatisticsBucket {
                    started_at_ms: window.started_at_ms,
                    finished_at_ms: window.finished_at_ms,
                    label: window.label,
                    request_count: totals.request_count,
                    tokens: totals.tokens,
                    cost_pico_usd: totals.cost_pico_usd,
                })
                .collect();
            let attribution =
                statistics_attribution(attribution, query.attribution_metric, &totals)?;
            Ok(UsageStatistics {
                matched_request_count: totals.request_count,
                tokens: totals.tokens,
                cost_pico_usd: totals.cost_pico_usd,
                redirected_request_count: totals.redirected_request_count,
                redirected_total_tokens: totals.redirected_total_tokens,
                granularity,
                trend,
                attribution,
            })
        })
        .await
    }

    /// Returns current and retained route snapshots for the bounded route filter.
    ///
    /// # Errors
    ///
    /// Returns an executor or database error.
    pub async fn usage_route_options(&self) -> Result<Vec<UsageRouteOption>, StorageError> {
        self.call(|connection| {
            let mut statement = connection.prepare(
                "SELECT route_id, name, retained FROM (
                    SELECT route_id, display_name AS name, 0 AS retained, sort_order AS ordering FROM routes
                    UNION ALL
                    SELECT historical.final_route_id,
                           COALESCE((SELECT latest.final_route_name FROM proxy_requests latest
                            WHERE latest.final_route_id = historical.final_route_id
                              AND latest.final_route_name IS NOT NULL
                            ORDER BY latest.started_at_ms DESC, latest.request_id DESC LIMIT 1), '已删除路由'),
                           1, 1000000
                    FROM proxy_requests historical
                    WHERE historical.final_route_id IS NOT NULL
                      AND historical.final_route_id NOT IN (SELECT route_id FROM routes)
                    GROUP BY historical.final_route_id
                 ) ORDER BY ordering, name",
            )?;
            statement
                .query_map([], |row| {
                    Ok(UsageRouteOption {
                        route_id: RouteId::from_string(row.get(0)?),
                        name: row.get(1)?,
                        retained: row.get::<_, i64>(2)? != 0,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()
                .map_err(StorageError::from)
        })
        .await
    }

    /// Loads one privacy-safe request and its ordered attempts.
    ///
    /// # Errors
    ///
    /// Returns invalid-query, not-found, executor, or database errors.
    #[allow(clippy::too_many_lines)]
    pub async fn usage_request_detail(
        &self,
        request_id: String,
    ) -> Result<UsageRequestDetail, StorageError> {
        if request_id.is_empty() || request_id.len() > 128 {
            return Err(StorageError::InvalidUsageQuery);
        }
        self.call(move |connection| {
            let request = connection
                .query_row(
                    "SELECT request_id, started_at_ms, finished_at_ms, final_route_id, final_route_name,
                            requested_model, actual_model, reasoning_effort, streaming,
                            completion_state, http_status,
                            input_tokens, output_tokens, total_tokens, cached_input_tokens,
                            cache_write_input_tokens, total_latency_ms, first_output_latency_ms,
                            pricing_catalog_version, cost_status, upstream_cost_pico_usd,
                            requested_service_tier, actual_service_tier,
                            fallback_stop_reason, fallback_stop_target_route_id,
                            fallback_stop_target_route_name
                     FROM proxy_requests WHERE request_id = ?1",
                    [&request_id],
                    |row| {
                        let actual_service_tier = row.get::<_, Option<String>>(22)?;
                        Ok((
                            usage_history_row(row, actual_service_tier.clone())?,
                            row.get::<_, Option<String>>(21)?,
                            actual_service_tier,
                            row.get::<_, Option<String>>(23)?,
                            row.get::<_, Option<String>>(24)?.map(RouteId::from_string),
                            row.get::<_, Option<String>>(25)?,
                        ))
                    },
                )
                .optional()?
                .ok_or(StorageError::NotFound)?;
            let mut statement = connection.prepare(
                "SELECT attempt_id, attempt_index, attempt_role, route_id, route_name,
                        started_at_ms, finished_at_ms,
                        http_status, error_category, delivery_state, actual_model,
                        forwarded_service_tier, actual_service_tier, input_tokens, output_tokens, total_tokens,
                        cached_input_tokens, cache_write_input_tokens,
                        pricing_catalog_version, cost_status, cost_pico_usd,
                        routing_transition_kind, routing_transition_target_route_id,
                        routing_transition_target_route_name
                 FROM upstream_attempts WHERE request_id = ?1 ORDER BY attempt_index",
            )?;
            let mut attempts = statement
                .query_map([&request_id], |row| {
                    let attempt_id = row.get::<_, String>(0)?;
                    let transition = match (
                        row.get::<_, Option<String>>(21)?,
                        row.get::<_, Option<String>>(22)?,
                        row.get::<_, Option<String>>(23)?,
                    ) {
                        (Some(kind), Some(target_route_id), Some(target_route_name)) => {
                            let kind = RoutingTransitionKind::parse(&kind).map_err(|error| {
                                rusqlite::Error::FromSqlConversionFailure(
                                    21,
                                    rusqlite::types::Type::Text,
                                    Box::new(error),
                                )
                            })?;
                            let mut skips = connection.prepare(
                                "SELECT route_id, route_name, reason
                                 FROM upstream_attempt_routing_skips
                                 WHERE attempt_id = ?1 ORDER BY skip_order",
                            )?;
                            let skipped_routes = skips
                                .query_map([&attempt_id], |skip| {
                                    let reason = skip.get::<_, String>(2)?;
                                    if reason != "model_fallback_excluded" {
                                        return Err(rusqlite::Error::InvalidQuery);
                                    }
                                    Ok(RoutingTransitionSkip {
                                        route_id: RouteId::from_string(skip.get(0)?),
                                        route_name: skip.get(1)?,
                                    })
                                })?
                                .collect::<Result<Vec<_>, _>>()?;
                            Some(AttemptRoutingTransition {
                                kind,
                                target_route_id: RouteId::from_string(target_route_id),
                                target_route_name,
                                skipped_routes,
                            })
                        }
                        (None, None, None) => None,
                        _ => return Err(rusqlite::Error::InvalidQuery),
                    };
                    Ok(UsageAttemptDetail {
                        attempt_index: row.get(1)?,
                        attempt_role: AttemptRole::parse(&row.get::<_, String>(2)?)
                            .map_err(|error| rusqlite::Error::FromSqlConversionFailure(
                                2,
                                rusqlite::types::Type::Text,
                                Box::new(error),
                            ))?,
                        route_id: RouteId::from_string(row.get(3)?),
                        route_name: row.get(4)?,
                        started_at_ms: row.get(5)?,
                        finished_at_ms: row.get(6)?,
                        http_status: row.get(7)?,
                        error_category: row.get(8)?,
                        delivery_state: parse_delivery_state(&row.get::<_, String>(9)?),
                        actual_model: row.get(10)?,
                        forwarded_service_tier: row.get(11)?,
                        actual_service_tier: row.get(12)?,
                        input_tokens: row.get(13)?,
                        output_tokens: row.get(14)?,
                        total_tokens: row.get(15)?,
                        cached_input_tokens: row.get(16)?,
                        cache_write_input_tokens: row.get(17)?,
                        pricing_catalog_version: row.get(18)?,
                        cost_status: row
                            .get::<_, Option<String>>(19)?
                            .and_then(|value| CostStatus::parse(&value)),
                        cost_pico_usd: row.get(20)?,
                        routing_transition: transition,
                        routing_decision: None,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            let fallback_stop_reason = request
                .3
                .as_deref()
                .map(FallbackStopReason::parse)
                .transpose()?;
            materialize_routing_decisions(
                &mut attempts,
                fallback_stop_reason,
                request.4.as_ref(),
                request.5.as_deref(),
            );
            let cached_input_tokens = request.0.cached_input_tokens;
            let cache_write_input_tokens = request.0.cache_write_input_tokens;
            Ok(UsageRequestDetail {
                request: request.0,
                requested_service_tier: request.1,
                actual_service_tier: request.2,
                cached_input_tokens,
                cache_write_input_tokens,
                attempts,
            })
        })
        .await
    }

    /// Persists the latest request projection and any newly completed attempts.
    ///
    /// # Errors
    ///
    /// Returns an executor or `SQLite` transaction error.
    #[expect(
        clippy::too_many_lines,
        reason = "the request and attempt cost transaction is intentionally one atomic SQL operation"
    )]
    pub async fn record_request_history(
        &self,
        record: RequestHistoryRecord,
    ) -> Result<(), StorageError> {
        let pricing = self.pricing.clone();
        self.call(move |connection| {
            let transaction = connection.transaction()?;
            let request_id = record.request_id;
            transaction.execute(
                "INSERT INTO proxy_requests (
                    request_id, started_at_ms, finished_at_ms, turn_id,
                    requested_model, reasoning_effort, requested_service_tier,
                    actual_model, actual_service_tier,
                    final_route_id, final_route_name,
                    streaming, completion_state, http_status, error_category,
                    input_tokens, output_tokens, total_tokens, cached_input_tokens,
                    cache_write_input_tokens, total_latency_ms,
                    first_output_latency_ms, metadata_complete,
                    fallback_stop_reason, fallback_stop_target_route_id,
                    fallback_stop_target_route_name
                ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
                    ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20,
                    ?21, ?22, ?23, ?24, ?25, ?26
                ) ON CONFLICT(request_id) DO UPDATE SET
                    finished_at_ms = excluded.finished_at_ms,
                    turn_id = excluded.turn_id,
                    requested_model = excluded.requested_model,
                    reasoning_effort = excluded.reasoning_effort,
                    requested_service_tier = excluded.requested_service_tier,
                    actual_model = excluded.actual_model,
                    actual_service_tier = excluded.actual_service_tier,
                    final_route_id = excluded.final_route_id,
                    final_route_name = excluded.final_route_name,
                    streaming = excluded.streaming,
                    completion_state = excluded.completion_state,
                    http_status = excluded.http_status,
                    error_category = excluded.error_category,
                    input_tokens = excluded.input_tokens,
                    output_tokens = excluded.output_tokens,
                    total_tokens = excluded.total_tokens,
                    cached_input_tokens = excluded.cached_input_tokens,
                    cache_write_input_tokens = excluded.cache_write_input_tokens,
                    total_latency_ms = excluded.total_latency_ms,
                    first_output_latency_ms = excluded.first_output_latency_ms,
                    metadata_complete = excluded.metadata_complete,
                    fallback_stop_reason = excluded.fallback_stop_reason,
                    fallback_stop_target_route_id = excluded.fallback_stop_target_route_id,
                    fallback_stop_target_route_name = excluded.fallback_stop_target_route_name",
                params![
                    request_id,
                    record.started_at_ms,
                    record.finished_at_ms,
                    record.turn_id,
                    record.requested_model,
                    record.reasoning_effort,
                    record.requested_service_tier,
                    record.actual_model,
                    record.actual_service_tier,
                    record.final_route_id.as_ref().map(RouteId::as_str),
                    record.final_route_name,
                    record.streaming,
                    completion_state_value(&record.completion_state),
                    record.http_status,
                    record.error_category,
                    record.input_tokens,
                    record.output_tokens,
                    record.total_tokens,
                    record.cached_input_tokens,
                    record.cache_write_input_tokens,
                    record.total_latency_ms,
                    record.first_output_latency_ms,
                    record.metadata_complete,
                    record.fallback_stop_reason.map(FallbackStopReason::as_str),
                    record
                        .fallback_stop_target_route_id
                        .as_ref()
                        .map(RouteId::as_str),
                    record.fallback_stop_target_route_name,
                ],
            )?;
            for attempt in record.attempts {
                let priced = pricing.price(&UsageObservation {
                    requested_model: record.requested_model.as_deref(),
                    actual_model: attempt.actual_model.as_deref(),
                    forwarded_service_tier: attempt.forwarded_service_tier.as_deref(),
                    actual_service_tier: attempt.actual_service_tier.as_deref(),
                    input_tokens: attempt.input_tokens,
                    output_tokens: attempt.output_tokens,
                    total_tokens: attempt.total_tokens,
                    cached_input_tokens: attempt.cached_input_tokens,
                    cache_write_input_tokens: attempt.cache_write_input_tokens,
                    possible_model_work: attempt.delivery_state != DeliveryState::None
                        || attempt.input_tokens.is_some()
                        || attempt.output_tokens.is_some(),
                });
                transaction.execute(
                    "INSERT INTO upstream_attempts (
                        attempt_id, request_id, attempt_index, attempt_role, route_id, route_name,
                        started_at_ms, finished_at_ms, http_status, error_category,
                        delivery_state, actual_model, forwarded_service_tier, actual_service_tier, input_tokens,
                        output_tokens, total_tokens, cached_input_tokens,
                        cache_write_input_tokens, pricing_catalog_version, cost_status,
                        cost_pico_usd
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11,
                              ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22)",
                    params![
                        attempt.attempt_id.as_str(),
                        request_id,
                        attempt.attempt_index,
                        attempt.attempt_role.as_str(),
                        attempt.route_id.as_str(),
                        attempt.route_name,
                        attempt.started_at_ms,
                        attempt.finished_at_ms,
                        attempt.http_status,
                        attempt.error_category,
                        delivery_state_value(&attempt.delivery_state),
                        attempt.actual_model,
                        attempt.forwarded_service_tier,
                        attempt.actual_service_tier,
                        attempt.input_tokens,
                        attempt.output_tokens,
                        attempt.total_tokens,
                        attempt.cached_input_tokens,
                        attempt.cache_write_input_tokens,
                        priced.catalog_version,
                        priced.status.as_str(),
                        priced.amount_pico_usd,
                    ],
                )?;
            }
            let attempt_costs = {
                let mut statement = transaction.prepare(
                    "SELECT cost_status, cost_pico_usd, pricing_catalog_version
                     FROM upstream_attempts WHERE request_id = ?1 ORDER BY attempt_index",
                )?;
                statement
                    .query_map([&request_id], |row| {
                        let status = row.get::<_, String>(0)?;
                        Ok(PricedUsage {
                            catalog_version: persisted_catalog_version(
                                row.get::<_, Option<String>>(2)?,
                            ),
                            status: CostStatus::parse(&status).unwrap_or(CostStatus::Unavailable),
                            amount_pico_usd: row.get(1)?,
                        })
                    })?
                    .collect::<Result<Vec<_>, _>>()?
            };
            let request_cost = fold_request_cost(&attempt_costs);
            transaction.execute(
                "UPDATE proxy_requests SET pricing_catalog_version = ?2,
                    cost_status = ?3, upstream_cost_pico_usd = ?4 WHERE request_id = ?1",
                params![
                    request_id,
                    request_cost.catalog_version,
                    request_cost.status.as_str(),
                    request_cost.amount_pico_usd,
                ],
            )?;
            transaction.commit()?;
            Ok(())
        })
        .await
    }

    /// Persists one terminal automatic-Fallback decision after its owning
    /// attempt has already been queued.
    ///
    /// # Errors
    ///
    /// Returns an executor or database error. A missing request is a quiet
    /// metadata no-op so routing never depends on history availability.
    pub async fn record_fallback_stop(
        &self,
        record: FallbackStopRecord,
    ) -> Result<bool, StorageError> {
        self.call(move |connection| {
            let changed = connection.execute(
                "UPDATE proxy_requests
                 SET fallback_stop_reason = ?1,
                     fallback_stop_target_route_id = ?2,
                     fallback_stop_target_route_name = ?3
                 WHERE request_id = ?4
                   AND (SELECT MAX(attempt_index)
                        FROM upstream_attempts
                        WHERE request_id = ?4) = ?5",
                params![
                    record.reason.as_str(),
                    record.target_route_id.as_ref().map(RouteId::as_str),
                    record.target_route_name,
                    record.request_id,
                    record.attempt_index,
                ],
            )? == 1;
            Ok(changed)
        })
        .await
    }

    /// Persists one explicit routing transition on its owning attempt.
    ///
    /// # Errors
    ///
    /// Returns an executor or database error. A transition whose attempt has
    /// not been persisted is ignored without attaching it to another attempt.
    pub async fn record_attempt_routing_transition(
        &self,
        record: AttemptRoutingTransitionRecord,
    ) -> Result<bool, StorageError> {
        self.call(move |connection| {
            let transaction = connection.transaction()?;
            let attempt_id = transaction
                .query_row(
                    "SELECT attempt_id FROM upstream_attempts
                     WHERE request_id = ?1 AND attempt_index = ?2",
                    params![record.request_id, record.attempt_index],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            let Some(attempt_id) = attempt_id else {
                transaction.commit()?;
                return Ok(false);
            };
            transaction.execute(
                "UPDATE upstream_attempts
                 SET routing_transition_kind = ?1,
                     routing_transition_target_route_id = ?2,
                     routing_transition_target_route_name = ?3
                 WHERE attempt_id = ?4",
                params![
                    record.transition.kind.as_str(),
                    record.transition.target_route_id.as_str(),
                    record.transition.target_route_name,
                    attempt_id,
                ],
            )?;
            transaction.execute(
                "DELETE FROM upstream_attempt_routing_skips WHERE attempt_id = ?1",
                [&attempt_id],
            )?;
            for (skip_order, skipped) in record.transition.skipped_routes.iter().enumerate() {
                let skip_order = i64::try_from(skip_order)
                    .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
                transaction.execute(
                    "INSERT INTO upstream_attempt_routing_skips
                     (attempt_id, skip_order, route_id, route_name, reason)
                     VALUES (?1, ?2, ?3, ?4, 'model_fallback_excluded')",
                    params![
                        attempt_id,
                        skip_order,
                        skipped.route_id.as_str(),
                        skipped.route_name,
                    ],
                )?;
            }
            transaction.commit()?;
            Ok(true)
        })
        .await
    }

    /// Returns the latest non-cancelled retained upstream result per route.
    ///
    /// # Errors
    ///
    /// Returns an executor or `SQLite` query error.
    pub async fn latest_inference_attempts(
        &self,
    ) -> Result<Vec<LatestInferenceAttempt>, StorageError> {
        self.call(|connection| {
            let mut statement = connection.prepare(
                "SELECT a.route_id, a.finished_at_ms, a.http_status, a.delivery_state, a.error_category
                 FROM upstream_attempts a
                 JOIN proxy_requests r ON r.request_id = a.request_id
                 WHERE a.finished_at_ms IS NOT NULL AND r.completion_state != 'cancelled'
                 ORDER BY a.finished_at_ms DESC, a.attempt_index DESC, a.attempt_id DESC",
            )?;
            let rows = statement.query_map([], |row| {
                let status = row.get::<_, Option<u16>>(2)?;
                let delivery = row.get::<_, String>(3)?;
                Ok(LatestInferenceAttempt {
                    route_id: RouteId::from_string(row.get(0)?),
                    finished_at_ms: row.get(1)?,
                    succeeded: status.is_some_and(|status| (200..300).contains(&status))
                        && delivery == "completed",
                    error_category: row.get(4)?,
                })
            })?;
            let mut seen = std::collections::HashSet::new();
            let mut latest = Vec::new();
            for row in rows {
                let row = row?;
                if seen.insert(row.route_id.clone()) {
                    latest.push(row);
                }
            }
            Ok(latest)
        })
        .await
    }
}
fn materialize_routing_decisions(
    attempts: &mut [UsageAttemptDetail],
    stop_reason: Option<FallbackStopReason>,
    stop_target_route_id: Option<&RouteId>,
    stop_target_route_name: Option<&str>,
) {
    const LEGACY_ROUTER_MAX_ATTEMPTS: u32 = 4;

    for index in 0..attempts.len() {
        if let Some(transition) = attempts[index].routing_transition.clone() {
            attempts[index].routing_decision = Some(match transition.kind {
                RoutingTransitionKind::ActivateNext => RoutingDecision::ActivateNext {
                    target_route_id: transition.target_route_id,
                    target_route_name: transition.target_route_name,
                    skipped_routes: transition.skipped_routes,
                },
                RoutingTransitionKind::ResumeCaptured => RoutingDecision::ResumeCaptured {
                    target_route_id: transition.target_route_id,
                    target_route_name: transition.target_route_name,
                },
                RoutingTransitionKind::Recover => RoutingDecision::Recover {
                    target_route_id: transition.target_route_id,
                    target_route_name: transition.target_route_name,
                },
            });
            continue;
        }
        let decision = attempts.get(index + 1).map_or_else(
            || {
                stop_reason.map(|reason| RoutingDecision::Stop {
                    reason,
                    target_route_id: stop_target_route_id.cloned(),
                    target_route_name: stop_target_route_name.map(str::to_owned),
                })
            },
            |next| {
                if next.route_id == attempts[index].route_id {
                    let completed_on_route = attempts[..=index]
                        .iter()
                        .filter(|attempt| attempt.route_id == attempts[index].route_id)
                        .count();
                    Some(RoutingDecision::RetryCurrent {
                        attempt_number: u32::try_from(completed_on_route)
                            .unwrap_or(u32::MAX)
                            .saturating_add(1),
                        max_attempts: LEGACY_ROUTER_MAX_ATTEMPTS,
                    })
                } else {
                    Some(RoutingDecision::ActivateNext {
                        target_route_id: next.route_id.clone(),
                        target_route_name: next.route_name.clone(),
                        skipped_routes: Vec::new(),
                    })
                }
            },
        );
        attempts[index].routing_decision = decision;
    }
}
/// Reads back one persisted catalog version without a closed version whitelist.
///
/// Synchronized catalogs carry their own `openai-*-synced-*` version, so every
/// non-empty version within the accepted length is retained; only empty or
/// oversized values are dropped as unusable provenance.
fn persisted_catalog_version(value: Option<String>) -> Option<Arc<str>> {
    match value {
        Some(version)
            if !version.is_empty()
                && version.len() <= crate::pricing::MAX_CATALOG_VERSION_BYTES =>
        {
            Some(Arc::from(version))
        }
        _ => None,
    }
}
const fn completion_state_value(state: &CompletionState) -> &'static str {
    match state {
        CompletionState::NoUpstream => "no_upstream",
        CompletionState::Completed => "completed",
        CompletionState::Failed => "failed",
        CompletionState::Cancelled => "cancelled",
    }
}

fn parse_completion_state(value: &str) -> CompletionState {
    match value {
        "completed" => CompletionState::Completed,
        "failed" => CompletionState::Failed,
        "cancelled" => CompletionState::Cancelled,
        _ => CompletionState::NoUpstream,
    }
}

const fn delivery_state_value(state: &DeliveryState) -> &'static str {
    match state {
        DeliveryState::None => "none",
        DeliveryState::Started => "started",
        DeliveryState::Completed => "completed",
    }
}

fn parse_delivery_state(value: &str) -> DeliveryState {
    match value {
        "completed" => DeliveryState::Completed,
        "started" => DeliveryState::Started,
        _ => DeliveryState::None,
    }
}

fn literal_contains_pattern(value: &str) -> String {
    let mut pattern = String::with_capacity(value.len().saturating_add(2));
    pattern.push('%');
    for character in value.chars() {
        if matches!(character, '\\' | '%' | '_') {
            pattern.push('\\');
        }
        pattern.push(character);
    }
    pattern.push('%');
    pattern
}

#[derive(Clone, Debug, Default)]
struct StatisticsTotals {
    request_count: u64,
    tokens: UsageStatisticsTokens,
    cost_pico_usd: u64,
    redirected_request_count: u64,
    redirected_total_tokens: u64,
}

impl StatisticsTotals {
    fn add(
        &mut self,
        observation: &StatisticsObservation,
        redirected: bool,
    ) -> Result<(), StorageError> {
        self.request_count = self
            .request_count
            .checked_add(1)
            .ok_or(StorageError::UsageStatisticsOverflow)?;
        checked_statistics_add(&mut self.tokens.total, observation.total_tokens)?;
        checked_statistics_add(
            &mut self.tokens.uncached_input,
            statistics_uncached_input(observation.input_tokens, observation.cached_input_tokens),
        )?;
        checked_statistics_add(
            &mut self.tokens.cached_input,
            observation.cached_input_tokens,
        )?;
        checked_statistics_add(
            &mut self.tokens.cache_write_input,
            observation.cache_write_input_tokens,
        )?;
        checked_statistics_add(&mut self.tokens.output, observation.output_tokens)?;
        checked_statistics_add(&mut self.cost_pico_usd, observation.cost_pico_usd)?;
        if redirected {
            self.redirected_request_count = self
                .redirected_request_count
                .checked_add(1)
                .ok_or(StorageError::UsageStatisticsOverflow)?;
            checked_statistics_add(&mut self.redirected_total_tokens, observation.total_tokens)?;
        }
        Ok(())
    }

    fn selected_value(&self, metric: UsageStatisticsAttributionMetric) -> u64 {
        match metric {
            UsageStatisticsAttributionMetric::Requests => self.request_count,
            UsageStatisticsAttributionMetric::Tokens => self.tokens.total,
            UsageStatisticsAttributionMetric::Cost => self.cost_pico_usd,
        }
    }
}

struct StatisticsObservation {
    input_tokens: Option<i64>,
    cached_input_tokens: Option<i64>,
    cache_write_input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    total_tokens: Option<i64>,
    cost_pico_usd: Option<i64>,
}

struct AttributionIdentity {
    key: String,
    label: String,
}

struct AttributionAggregate {
    label: String,
    totals: StatisticsTotals,
}

struct StatisticsBucketWindow {
    started_at_ms: i64,
    finished_at_ms: i64,
    label: String,
}

fn validate_usage_statistics_query(query: &UsageStatisticsQuery) -> Result<(), StorageError> {
    if query.finished_at_or_before_ms < 0
        || query
            .finished_at_or_after_ms
            .is_some_and(|lower| lower < 0 || lower > query.finished_at_or_before_ms)
        || query.time_zone.is_empty()
        || query.time_zone.len() > 128
        || Utc
            .timestamp_millis_opt(query.finished_at_or_before_ms)
            .single()
            .is_none()
        || query
            .finished_at_or_after_ms
            .is_some_and(|lower| Utc.timestamp_millis_opt(lower).single().is_none())
    {
        return Err(StorageError::InvalidUsageQuery);
    }
    if query
        .model_contains
        .as_ref()
        .is_some_and(|model| model.is_empty() || model.len() > 256)
    {
        return Err(StorageError::InvalidUsageQuery);
    }
    Ok(())
}

fn statistics_granularity(query: &UsageStatisticsQuery) -> UsageStatisticsGranularity {
    let Some(lower) = query.finished_at_or_after_ms else {
        return UsageStatisticsGranularity::Month;
    };
    let duration = query.finished_at_or_before_ms.saturating_sub(lower);
    if duration <= 24 * 60 * 60 * 1_000 {
        UsageStatisticsGranularity::Hour
    } else if duration <= 30 * 24 * 60 * 60 * 1_000 {
        UsageStatisticsGranularity::Day
    } else {
        UsageStatisticsGranularity::Month
    }
}

fn checked_statistics_add(target: &mut u64, value: Option<i64>) -> Result<(), StorageError> {
    let Some(value) = value else {
        return Ok(());
    };
    let value = u64::try_from(value).map_err(|_| StorageError::UsageStatisticsOverflow)?;
    *target = target
        .checked_add(value)
        .ok_or(StorageError::UsageStatisticsOverflow)?;
    Ok(())
}

const fn statistics_uncached_input(input: Option<i64>, cached: Option<i64>) -> Option<i64> {
    match (input, cached) {
        (Some(input), Some(cached)) if input >= 0 && cached >= 0 && cached <= input => {
            Some(input - cached)
        }
        _ => None,
    }
}

fn attribution_identity(
    dimension: UsageStatisticsAttributionDimension,
    route_id: Option<String>,
    route_name: Option<String>,
    requested_model: Option<String>,
    actual_model: Option<String>,
) -> AttributionIdentity {
    match dimension {
        UsageStatisticsAttributionDimension::Route => match route_id {
            Some(route_id) => AttributionIdentity {
                key: format!("route:{route_id}"),
                label: route_name.unwrap_or_else(|| "未知路由".to_owned()),
            },
            None => AttributionIdentity {
                key: "route:unknown".to_owned(),
                label: route_name.unwrap_or_else(|| "未知路由".to_owned()),
            },
        },
        UsageStatisticsAttributionDimension::Model => {
            let model = actual_model.or(requested_model);
            AttributionIdentity {
                key: model.as_ref().map_or_else(
                    || "model:unknown".to_owned(),
                    |model| format!("model:{model}"),
                ),
                label: model.unwrap_or_else(|| "未知模型".to_owned()),
            }
        }
    }
}

fn statistics_attribution(
    aggregates: BTreeMap<String, AttributionAggregate>,
    metric: UsageStatisticsAttributionMetric,
    summary: &StatisticsTotals,
) -> Result<Vec<UsageStatisticsAttribution>, StorageError> {
    let total = summary.selected_value(metric);
    let mut values = aggregates
        .into_iter()
        .map(|(key, aggregate)| {
            (
                key,
                aggregate.label,
                aggregate.totals.selected_value(metric),
                aggregate.totals.redirected_request_count,
                aggregate.totals.redirected_total_tokens,
            )
        })
        .collect::<Vec<_>>();
    values.sort_by(|left, right| {
        right
            .2
            .cmp(&left.2)
            .then_with(|| left.1.cmp(&right.1))
            .then_with(|| left.0.cmp(&right.0))
    });
    let other = if values.len() > 5 {
        let remainder = values.split_off(5);
        Some(remainder.into_iter().try_fold(
            (0_u64, 0_u64, 0_u64),
            |(sum, redirected, redirected_tokens), (_, _, value, item_redirected, item_tokens)| {
                Ok::<_, StorageError>((
                    sum.checked_add(value)
                        .ok_or(StorageError::UsageStatisticsOverflow)?,
                    redirected
                        .checked_add(item_redirected)
                        .ok_or(StorageError::UsageStatisticsOverflow)?,
                    redirected_tokens
                        .checked_add(item_tokens)
                        .ok_or(StorageError::UsageStatisticsOverflow)?,
                ))
            },
        )?)
    } else {
        None
    };
    let mut result = values
        .into_iter()
        .map(
            |(key, label, value, redirected_request_count, redirected_total_tokens)| {
                UsageStatisticsAttribution {
                    key,
                    label,
                    is_other: false,
                    value,
                    share_percent: statistics_share_percent(value, total),
                    redirected_request_count,
                    redirected_total_tokens,
                }
            },
        )
        .collect::<Vec<_>>();
    if let Some((value, redirected_request_count, redirected_total_tokens)) = other {
        result.push(UsageStatisticsAttribution {
            key: "other".to_owned(),
            label: "其他".to_owned(),
            is_other: true,
            value,
            share_percent: statistics_share_percent(value, total),
            redirected_request_count,
            redirected_total_tokens,
        });
    }
    Ok(result)
}

fn statistics_share_percent(value: u64, total: u64) -> String {
    if total == 0 {
        return "0".to_owned();
    }
    let tenths = (u128::from(value) * 1_000 + u128::from(total) / 2) / u128::from(total);
    format!("{}.{:01}", tenths / 10, tenths % 10)
}

fn statistics_bucket_windows(
    lower_ms: i64,
    upper_ms: i64,
    time_zone: Tz,
    granularity: UsageStatisticsGranularity,
) -> Result<Vec<StatisticsBucketWindow>, StorageError> {
    let lower = Utc
        .timestamp_millis_opt(lower_ms)
        .single()
        .ok_or(StorageError::InvalidUsageQuery)?;
    let upper = Utc
        .timestamp_millis_opt(upper_ms)
        .single()
        .ok_or(StorageError::InvalidUsageQuery)?;
    let local_lower = lower.with_timezone(&time_zone);
    let local_upper = upper.with_timezone(&time_zone);
    let mut civil = floor_statistics_civil(local_lower, granularity)?;
    let upper_civil = local_upper.naive_local();
    let mut boundaries = vec![lower_ms, upper_ms];
    let mut iterations = 0_u16;
    while civil <= upper_civil {
        boundaries.extend(resolve_statistics_boundary(time_zone, civil, granularity));
        civil = next_statistics_civil(civil, granularity).ok_or(StorageError::InvalidUsageQuery)?;
        iterations = iterations
            .checked_add(1)
            .ok_or(StorageError::InvalidUsageQuery)?;
        if iterations > 2_000 {
            return Err(StorageError::InvalidUsageQuery);
        }
    }
    boundaries.retain(|boundary| *boundary >= lower_ms && *boundary <= upper_ms);
    boundaries.sort_unstable();
    boundaries.dedup();
    if boundaries.len() == 1 {
        return Ok(vec![StatisticsBucketWindow {
            started_at_ms: lower_ms,
            finished_at_ms: upper_ms,
            label: statistics_bucket_label(lower, time_zone, granularity, false),
        }]);
    }
    let duplicate_hour_labels = if granularity == UsageStatisticsGranularity::Hour {
        let mut labels = BTreeMap::<String, u8>::new();
        for boundary in boundaries.iter().take(boundaries.len() - 1) {
            let instant = Utc
                .timestamp_millis_opt(*boundary)
                .single()
                .ok_or(StorageError::InvalidUsageQuery)?;
            let label = statistics_bucket_label(instant, time_zone, granularity, false);
            *labels.entry(label).or_default() += 1;
        }
        labels
    } else {
        BTreeMap::new()
    };
    boundaries
        .windows(2)
        .map(|window| {
            let instant = Utc
                .timestamp_millis_opt(window[0])
                .single()
                .ok_or(StorageError::InvalidUsageQuery)?;
            let base_label = statistics_bucket_label(instant, time_zone, granularity, false);
            let include_offset = duplicate_hour_labels.get(&base_label).copied().unwrap_or(0) > 1;
            Ok(StatisticsBucketWindow {
                started_at_ms: window[0],
                finished_at_ms: window[1],
                label: statistics_bucket_label(instant, time_zone, granularity, include_offset),
            })
        })
        .collect()
}

fn floor_statistics_civil(
    local: DateTime<Tz>,
    granularity: UsageStatisticsGranularity,
) -> Result<NaiveDateTime, StorageError> {
    let date = NaiveDate::from_ymd_opt(local.year(), local.month(), local.day())
        .ok_or(StorageError::InvalidUsageQuery)?;
    match granularity {
        UsageStatisticsGranularity::Hour => date
            .and_hms_opt(local.hour(), 0, 0)
            .ok_or(StorageError::InvalidUsageQuery),
        UsageStatisticsGranularity::Day => date
            .and_hms_opt(0, 0, 0)
            .ok_or(StorageError::InvalidUsageQuery),
        UsageStatisticsGranularity::Month => {
            NaiveDate::from_ymd_opt(local.year(), local.month(), 1)
                .and_then(|date| date.and_hms_opt(0, 0, 0))
                .ok_or(StorageError::InvalidUsageQuery)
        }
    }
}

fn next_statistics_civil(
    value: NaiveDateTime,
    granularity: UsageStatisticsGranularity,
) -> Option<NaiveDateTime> {
    match granularity {
        UsageStatisticsGranularity::Hour => value.checked_add_signed(ChronoDuration::hours(1)),
        UsageStatisticsGranularity::Day => value.checked_add_signed(ChronoDuration::days(1)),
        UsageStatisticsGranularity::Month => {
            let (year, month) = if value.month() == 12 {
                (value.year().checked_add(1)?, 1)
            } else {
                (value.year(), value.month() + 1)
            };
            NaiveDate::from_ymd_opt(year, month, 1)?.and_hms_opt(0, 0, 0)
        }
    }
}

fn resolve_statistics_boundary(
    time_zone: Tz,
    civil: NaiveDateTime,
    granularity: UsageStatisticsGranularity,
) -> Vec<i64> {
    if granularity == UsageStatisticsGranularity::Hour {
        return resolved_boundary_instants(time_zone, civil, true);
    }
    for minute in 0..24 * 60 {
        let Some(candidate) = civil.checked_add_signed(ChronoDuration::minutes(minute)) else {
            return Vec::new();
        };
        if candidate.date() != civil.date() {
            return Vec::new();
        }
        let resolved = resolved_boundary_instants(time_zone, candidate, false);
        if !resolved.is_empty() {
            return resolved;
        }
    }
    Vec::new()
}

fn resolved_boundary_instants(time_zone: Tz, civil: NaiveDateTime, both: bool) -> Vec<i64> {
    let mut values = match time_zone.from_local_datetime(&civil) {
        LocalResult::None => Vec::new(),
        LocalResult::Single(value) => vec![value.with_timezone(&Utc).timestamp_millis()],
        LocalResult::Ambiguous(first, second) if both => vec![
            first.with_timezone(&Utc).timestamp_millis(),
            second.with_timezone(&Utc).timestamp_millis(),
        ],
        LocalResult::Ambiguous(first, second) => {
            vec![first.min(second).with_timezone(&Utc).timestamp_millis()]
        }
    };
    values.sort_unstable();
    values.dedup();
    values
}

fn statistics_bucket_label(
    instant: DateTime<Utc>,
    time_zone: Tz,
    granularity: UsageStatisticsGranularity,
    include_offset: bool,
) -> String {
    let local = instant.with_timezone(&time_zone);
    let local = match granularity {
        UsageStatisticsGranularity::Hour if include_offset => local.format("%m/%d %H:00 %:z"),
        UsageStatisticsGranularity::Hour => local.format("%m/%d %H:00"),
        UsageStatisticsGranularity::Day => local.format("%m/%d"),
        UsageStatisticsGranularity::Month => local.format("%Y/%m"),
    };
    local.to_string()
}

fn usage_history_row(
    row: &rusqlite::Row<'_>,
    actual_service_tier: Option<String>,
) -> rusqlite::Result<UsageHistoryRow> {
    let requested_model: Option<String> = row.get(5)?;
    let actual_model: Option<String> = row.get(6)?;
    let model_verdict = model_verdict(requested_model.as_deref(), actual_model.as_deref());
    Ok(UsageHistoryRow {
        request_id: row.get(0)?,
        started_at_ms: row.get(1)?,
        finished_at_ms: row.get(2)?,
        final_route_id: row.get::<_, Option<String>>(3)?.map(RouteId::from_string),
        final_route_name: row.get(4)?,
        requested_model,
        actual_model,
        model_verdict,
        actual_service_tier,
        reasoning_effort: row.get(7)?,
        streaming: row.get(8)?,
        completion_state: parse_completion_state(&row.get::<_, String>(9)?),
        http_status: row.get(10)?,
        input_tokens: row.get(11)?,
        output_tokens: row.get(12)?,
        total_tokens: row.get(13)?,
        cached_input_tokens: row.get(14)?,
        cache_write_input_tokens: row.get(15)?,
        total_latency_ms: row.get(16)?,
        first_output_latency_ms: row.get(17)?,
        pricing_catalog_version: row.get(18)?,
        cost_status: row
            .get::<_, Option<String>>(19)?
            .and_then(|value| CostStatus::parse(&value)),
        upstream_cost_pico_usd: row.get(20)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::tests::{SYNCED_PRIORITY_VERSION, SYNCED_STANDARD_VERSION, utc_ms};
    #[test]
    fn routing_decisions_are_derived_from_ordered_attempts_and_terminal_metadata() {
        fn attempt(index: u32, route_id: RouteId, route_name: &str) -> UsageAttemptDetail {
            UsageAttemptDetail {
                attempt_index: index,
                attempt_role: super::AttemptRole::Ordinary,
                route_id,
                route_name: route_name.to_owned(),
                started_at_ms: i64::from(index),
                finished_at_ms: Some(i64::from(index) + 1),
                http_status: Some(500),
                error_category: Some("upstream_http_status".to_owned()),
                delivery_state: crate::domain::DeliveryState::Completed,
                actual_model: None,
                forwarded_service_tier: None,
                actual_service_tier: None,
                input_tokens: None,
                output_tokens: None,
                total_tokens: None,
                cached_input_tokens: None,
                cache_write_input_tokens: None,
                pricing_catalog_version: None,
                cost_status: None,
                cost_pico_usd: None,
                routing_transition: None,
                routing_decision: None,
            }
        }

        let first = RouteId::new();
        let second = RouteId::new();
        let mut attempts = vec![
            attempt(0, first.clone(), "First"),
            attempt(1, first.clone(), "First"),
            attempt(2, second.clone(), "Second"),
        ];
        materialize_routing_decisions(
            &mut attempts,
            Some(FallbackStopReason::AllParticipantsAttempted),
            None,
            None,
        );

        assert_eq!(
            attempts[0].routing_decision,
            Some(RoutingDecision::RetryCurrent {
                attempt_number: 2,
                max_attempts: 4,
            })
        );
        assert_eq!(
            attempts[1].routing_decision,
            Some(RoutingDecision::ActivateNext {
                target_route_id: second.clone(),
                target_route_name: "Second".to_owned(),
                skipped_routes: Vec::new(),
            })
        );
        assert_eq!(
            attempts[2].routing_decision,
            Some(RoutingDecision::Stop {
                reason: FallbackStopReason::AllParticipantsAttempted,
                target_route_id: None,
                target_route_name: None,
            })
        );

        let third = RouteId::new();
        let mut forward_only = vec![
            attempt(0, first, "First"),
            attempt(1, second.clone(), "Second"),
            attempt(2, third.clone(), "Third"),
        ];
        materialize_routing_decisions(
            &mut forward_only,
            Some(FallbackStopReason::AllParticipantsAttempted),
            None,
            None,
        );
        assert_eq!(
            forward_only
                .into_iter()
                .map(|attempt| attempt.routing_decision)
                .collect::<Vec<_>>(),
            vec![
                Some(RoutingDecision::ActivateNext {
                    target_route_id: second,
                    target_route_name: "Second".to_owned(),
                    skipped_routes: Vec::new(),
                }),
                Some(RoutingDecision::ActivateNext {
                    target_route_id: third,
                    target_route_name: "Third".to_owned(),
                    skipped_routes: Vec::new(),
                }),
                Some(RoutingDecision::Stop {
                    reason: FallbackStopReason::AllParticipantsAttempted,
                    target_route_id: None,
                    target_route_name: None,
                }),
            ]
        );
    }
    #[test]
    fn persisted_catalog_versions_are_bounded_but_not_whitelisted() {
        for version in [
            crate::pricing::CATALOG_VERSION,
            crate::pricing::PRIORITY_CATALOG_VERSION,
            SYNCED_STANDARD_VERSION,
            SYNCED_PRIORITY_VERSION,
            "openai-standard-synced-2027-01-01",
        ] {
            assert_eq!(
                super::persisted_catalog_version(Some(version.to_owned())).as_deref(),
                Some(version)
            );
        }

        assert_eq!(super::persisted_catalog_version(None), None);
        assert_eq!(super::persisted_catalog_version(Some(String::new())), None);
        assert_eq!(
            super::persisted_catalog_version(Some(
                "v".repeat(crate::pricing::MAX_CATALOG_VERSION_BYTES + 1)
            )),
            None
        );
    }
    #[test]
    fn usage_statistics_handles_fall_back_dst_hour_labels() {
        let lower = utc_ms(2024, 11, 3, 4, 0);
        let upper = utc_ms(2024, 11, 3, 8, 0);
        let windows = statistics_bucket_windows(
            lower,
            upper,
            "America/New_York".parse().expect("time zone"),
            UsageStatisticsGranularity::Hour,
        )
        .expect("windows");

        assert_eq!(windows.len(), 4);
        assert!(windows[1].label.contains("-04:00"));
        assert!(windows[2].label.contains("-05:00"));
        assert_eq!(windows[0].started_at_ms, lower);
        assert_eq!(windows[3].finished_at_ms, upper);
    }

    #[test]
    fn usage_statistics_handles_spring_forward_and_partial_calendar_edges() {
        let spring = statistics_bucket_windows(
            utc_ms(2024, 3, 10, 5, 0),
            utc_ms(2024, 3, 10, 9, 0),
            "America/New_York".parse().expect("time zone"),
            UsageStatisticsGranularity::Hour,
        )
        .expect("spring windows");
        assert_eq!(
            spring
                .iter()
                .map(|window| window.label.as_str())
                .collect::<Vec<_>>(),
            ["03/10 00:00", "03/10 01:00", "03/10 03:00", "03/10 04:00"]
        );

        let daily_lower = utc_ms(2025, 12, 31, 12, 0);
        let daily_upper = utc_ms(2026, 1, 2, 6, 0);
        let daily = statistics_bucket_windows(
            daily_lower,
            daily_upper,
            Tz::UTC,
            UsageStatisticsGranularity::Day,
        )
        .expect("daily windows");
        assert_eq!(daily.first().expect("first").started_at_ms, daily_lower);
        assert_eq!(daily.last().expect("last").finished_at_ms, daily_upper);
        assert_eq!(
            daily
                .iter()
                .map(|window| window.label.as_str())
                .collect::<Vec<_>>(),
            ["12/31", "01/01", "01/02"]
        );

        let monthly = statistics_bucket_windows(
            utc_ms(2025, 12, 15, 12, 0),
            utc_ms(2026, 2, 10, 6, 0),
            Tz::UTC,
            UsageStatisticsGranularity::Month,
        )
        .expect("monthly windows");
        assert_eq!(
            monthly
                .iter()
                .map(|window| window.label.as_str())
                .collect::<Vec<_>>(),
            ["2025/12", "2026/01", "2026/02"]
        );
    }
    #[test]
    fn usage_statistics_attribution_is_top_five_plus_other_with_stable_ties() {
        let mut aggregates = BTreeMap::new();
        for (key, label, count, redirected, redirected_tokens) in [
            ("route:a", "A", 1, 1, 10),
            ("route:b", "B", 1, 2, 20),
            ("route:c", "C", 1, 3, 30),
            ("route:d", "D", 1, 4, 40),
            ("route:e", "E", 1, 5, 50),
            ("route:f", "F", 1, 6, 60),
        ] {
            let totals = StatisticsTotals {
                request_count: count,
                redirected_request_count: redirected,
                redirected_total_tokens: redirected_tokens,
                ..StatisticsTotals::default()
            };
            aggregates.insert(
                key.to_owned(),
                AttributionAggregate {
                    label: label.to_owned(),
                    totals,
                },
            );
        }
        let summary = StatisticsTotals {
            request_count: 6,
            ..StatisticsTotals::default()
        };

        let result = statistics_attribution(
            aggregates,
            UsageStatisticsAttributionMetric::Requests,
            &summary,
        )
        .expect("attribution");

        assert_eq!(result.len(), 6);
        assert_eq!(result[0].label, "A");
        assert_eq!(result[4].label, "E");
        assert_eq!(result[4].redirected_request_count, 5);
        assert_eq!(result[4].redirected_total_tokens, 50);
        assert!(result[5].is_other);
        assert_eq!(result[5].value, "1".parse::<u64>().unwrap());
        assert_eq!(result[5].redirected_request_count, 6);
        assert_eq!(result[5].redirected_total_tokens, 60);
    }

    #[test]
    fn usage_statistics_overflow_is_rejected() {
        let mut total = u64::MAX;
        assert!(matches!(
            super::checked_statistics_add(&mut total, Some(1)),
            Err(StorageError::UsageStatisticsOverflow)
        ));
    }
}
