use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::storage::CodexModelRecord;
use crate::{
    balance::{BalanceDisplaySnapshot, BalanceQueryMode, BalanceRefreshBatchState},
    codex_config::CodexConfigStatus,
    domain::{
        BalanceQueryPolicy, CompletionState, DeliveryState, RouteId, ServiceTierPolicy,
        ValidationError,
    },
    recovery::{DatabaseStartupIssue, RecoveryHealth, RecoveryHealthKind},
    state::{BootstrapSnapshotDto, FallbackStateDto, RouteSummaryDto},
    storage::{
        FallbackStopReason, RoutingDecision, UsageAttemptDetail, UsageHistoryCursor,
        UsageHistoryPage, UsageHistoryQuery, UsageHistoryRow, UsageRequestDetail, UsageRouteOption,
        UsageStatistics, UsageStatisticsAttributionDimension, UsageStatisticsAttributionMetric,
        UsageStatisticsGranularity, UsageStatisticsQuery, UsageStatisticsTokens,
    },
};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum UsageCostStateDto {
    PreV0_3a,
    Exact,
    Partial,
    Unavailable,
    NotApplicable,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum UsageFastStatusDto {
    Confirmed,
    Unconfirmed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct UsageCostDto {
    pub state: UsageCostStateDto,
    pub amount_pico_usd: Option<String>,
    pub currency: String,
    pub catalog_version: Option<String>,
    pub service_tier: Option<String>,
    pub fast_status: Option<UsageFastStatusDto>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct UsageTokensDto {
    #[ts(type = "number | null")]
    pub input: Option<i64>,
    #[ts(type = "number | null")]
    pub uncached_input: Option<i64>,
    #[ts(type = "number | null")]
    pub output: Option<i64>,
    #[ts(type = "number | null")]
    pub total: Option<i64>,
    #[ts(type = "number | null")]
    pub cached_input: Option<i64>,
    #[ts(type = "number | null")]
    pub cache_write_input: Option<i64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct UsageHistoryCursorDto {
    #[ts(type = "number")]
    pub finished_at_ms: i64,
    pub request_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct UsageHistoryQueryDto {
    #[ts(type = "number | null")]
    pub finished_at_or_after_ms: Option<i64>,
    #[ts(type = "number")]
    pub finished_at_or_before_ms: i64,
    pub completion_state: Option<CompletionState>,
    pub route_id: Option<RouteId>,
    pub model_contains: Option<String>,
    pub cursor: Option<UsageHistoryCursorDto>,
    #[serde(default = "default_usage_history_limit")]
    pub limit: u16,
}

const fn default_usage_history_limit() -> u16 {
    50
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct UsageHistoryRowDto {
    pub request_id: String,
    #[ts(type = "number")]
    pub started_at_ms: i64,
    #[ts(type = "number | null")]
    pub finished_at_ms: Option<i64>,
    pub route_id: Option<RouteId>,
    pub route_name: Option<String>,
    pub requested_model: Option<String>,
    pub actual_model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub streaming: bool,
    pub completion_state: CompletionState,
    pub http_status: Option<u16>,
    pub tokens: UsageTokensDto,
    #[ts(type = "number | null")]
    pub total_latency_ms: Option<i64>,
    #[ts(type = "number | null")]
    pub first_output_latency_ms: Option<i64>,
    pub cost: UsageCostDto,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct UsageHistoryPageDto {
    pub rows: Vec<UsageHistoryRowDto>,
    pub next_cursor: Option<UsageHistoryCursorDto>,
    #[ts(type = "number")]
    pub total_rows: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum UsageStatisticsGranularityDto {
    Hour,
    Day,
    Month,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum UsageStatisticsAttributionDimensionDto {
    Route,
    Model,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum UsageStatisticsAttributionMetricDto {
    Requests,
    Tokens,
    Cost,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct UsageStatisticsQueryDto {
    #[ts(type = "number | null")]
    pub finished_at_or_after_ms: Option<i64>,
    #[ts(type = "number")]
    pub finished_at_or_before_ms: i64,
    pub route_id: Option<RouteId>,
    pub model_contains: Option<String>,
    pub time_zone: String,
    pub attribution_dimension: UsageStatisticsAttributionDimensionDto,
    pub attribution_metric: UsageStatisticsAttributionMetricDto,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct UsageStatisticsTokensDto {
    pub total: String,
    pub uncached_input: String,
    pub cached_input: String,
    pub cache_write_input: String,
    pub output: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct UsageStatisticsBucketDto {
    #[ts(type = "number")]
    pub started_at_ms: i64,
    #[ts(type = "number")]
    pub finished_at_ms: i64,
    pub label: String,
    #[ts(type = "number")]
    pub request_count: u64,
    pub tokens: UsageStatisticsTokensDto,
    pub cost_pico_usd: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct UsageStatisticsAttributionDto {
    pub key: String,
    pub label: String,
    pub is_other: bool,
    pub value: String,
    pub share_percent: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct UsageStatisticsDto {
    #[ts(type = "number")]
    pub matched_request_count: u64,
    pub tokens: UsageStatisticsTokensDto,
    pub cost_pico_usd: String,
    pub granularity: UsageStatisticsGranularityDto,
    pub trend: Vec<UsageStatisticsBucketDto>,
    pub attribution: Vec<UsageStatisticsAttributionDto>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct UsageRouteOptionDto {
    pub route_id: RouteId,
    pub name: String,
    pub retained: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct UsageAttemptDto {
    pub attempt_index: u32,
    pub route_id: RouteId,
    pub route_name: String,
    #[ts(type = "number")]
    pub started_at_ms: i64,
    #[ts(type = "number | null")]
    pub finished_at_ms: Option<i64>,
    pub http_status: Option<u16>,
    pub error_category: Option<String>,
    pub delivery_state: DeliveryState,
    pub actual_model: Option<String>,
    pub forwarded_service_tier: Option<String>,
    pub actual_service_tier: Option<String>,
    pub tokens: UsageTokensDto,
    pub cost: UsageCostDto,
    pub routing_decision: Option<RoutingDecisionDto>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum FallbackStopReasonDto {
    FallbackDisabled,
    FailureNotEligible,
    ResponseCommitted,
    AllParticipantsAttempted,
    StalePolicy,
    ActivationFailed,
    AttemptIndexExhausted,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
#[ts(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum RoutingDecisionDto {
    RetryCurrent {
        attempt_number: u32,
        max_attempts: u32,
    },
    ActivateNext {
        target_route_id: RouteId,
        target_route_name: String,
    },
    Stop {
        reason: FallbackStopReasonDto,
        target_route_id: Option<RouteId>,
        target_route_name: Option<String>,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct UsageRequestDetailDto {
    pub request: UsageHistoryRowDto,
    pub requested_service_tier: Option<String>,
    pub actual_service_tier: Option<String>,
    pub tokens: UsageTokensDto,
    pub attempts: Vec<UsageAttemptDto>,
}

impl From<UsageHistoryQueryDto> for UsageHistoryQuery {
    fn from(value: UsageHistoryQueryDto) -> Self {
        Self {
            finished_at_or_after_ms: value.finished_at_or_after_ms,
            finished_at_or_before_ms: value.finished_at_or_before_ms,
            completion_state: value.completion_state,
            route_id: value.route_id,
            model_contains: value.model_contains,
            cursor: value.cursor.map(|cursor| UsageHistoryCursor {
                finished_at_ms: cursor.finished_at_ms,
                request_id: cursor.request_id,
            }),
            limit: value.limit,
        }
    }
}

impl From<UsageStatisticsQueryDto> for UsageStatisticsQuery {
    fn from(value: UsageStatisticsQueryDto) -> Self {
        Self {
            finished_at_or_after_ms: value.finished_at_or_after_ms,
            finished_at_or_before_ms: value.finished_at_or_before_ms,
            route_id: value.route_id,
            model_contains: value.model_contains,
            time_zone: value.time_zone,
            attribution_dimension: match value.attribution_dimension {
                UsageStatisticsAttributionDimensionDto::Route => {
                    UsageStatisticsAttributionDimension::Route
                }
                UsageStatisticsAttributionDimensionDto::Model => {
                    UsageStatisticsAttributionDimension::Model
                }
            },
            attribution_metric: match value.attribution_metric {
                UsageStatisticsAttributionMetricDto::Requests => {
                    UsageStatisticsAttributionMetric::Requests
                }
                UsageStatisticsAttributionMetricDto::Tokens => {
                    UsageStatisticsAttributionMetric::Tokens
                }
                UsageStatisticsAttributionMetricDto::Cost => UsageStatisticsAttributionMetric::Cost,
            },
        }
    }
}

impl From<UsageHistoryCursor> for UsageHistoryCursorDto {
    fn from(value: UsageHistoryCursor) -> Self {
        Self {
            finished_at_ms: value.finished_at_ms,
            request_id: value.request_id,
        }
    }
}

impl From<UsageHistoryPage> for UsageHistoryPageDto {
    fn from(value: UsageHistoryPage) -> Self {
        Self {
            rows: value.rows.into_iter().map(Into::into).collect(),
            next_cursor: value.next_cursor.map(Into::into),
            total_rows: value.total_rows,
        }
    }
}

impl From<UsageStatistics> for UsageStatisticsDto {
    fn from(value: UsageStatistics) -> Self {
        Self {
            matched_request_count: value.matched_request_count,
            tokens: usage_statistics_tokens(&value.tokens),
            cost_pico_usd: value.cost_pico_usd.to_string(),
            granularity: match value.granularity {
                UsageStatisticsGranularity::Hour => UsageStatisticsGranularityDto::Hour,
                UsageStatisticsGranularity::Day => UsageStatisticsGranularityDto::Day,
                UsageStatisticsGranularity::Month => UsageStatisticsGranularityDto::Month,
            },
            trend: value
                .trend
                .into_iter()
                .map(|bucket| UsageStatisticsBucketDto {
                    started_at_ms: bucket.started_at_ms,
                    finished_at_ms: bucket.finished_at_ms,
                    label: bucket.label,
                    request_count: bucket.request_count,
                    tokens: usage_statistics_tokens(&bucket.tokens),
                    cost_pico_usd: bucket.cost_pico_usd.to_string(),
                })
                .collect(),
            attribution: value
                .attribution
                .into_iter()
                .map(|item| UsageStatisticsAttributionDto {
                    key: item.key,
                    label: item.label,
                    is_other: item.is_other,
                    value: item.value.to_string(),
                    share_percent: item.share_percent,
                })
                .collect(),
        }
    }
}

fn usage_statistics_tokens(value: &UsageStatisticsTokens) -> UsageStatisticsTokensDto {
    UsageStatisticsTokensDto {
        total: value.total.to_string(),
        uncached_input: value.uncached_input.to_string(),
        cached_input: value.cached_input.to_string(),
        cache_write_input: value.cache_write_input.to_string(),
        output: value.output.to_string(),
    }
}

impl From<UsageHistoryRow> for UsageHistoryRowDto {
    fn from(value: UsageHistoryRow) -> Self {
        let cost = usage_cost(
            value.cost_status,
            value.upstream_cost_pico_usd,
            value.pricing_catalog_version,
            value.actual_service_tier.as_deref(),
        );
        Self {
            request_id: value.request_id,
            started_at_ms: value.started_at_ms,
            finished_at_ms: value.finished_at_ms,
            route_id: value.final_route_id,
            route_name: value.final_route_name,
            requested_model: value.requested_model,
            actual_model: value.actual_model,
            reasoning_effort: value.reasoning_effort,
            streaming: value.streaming,
            completion_state: value.completion_state,
            http_status: value.http_status,
            tokens: UsageTokensDto {
                input: value.input_tokens,
                uncached_input: uncached_input(value.input_tokens, value.cached_input_tokens),
                output: value.output_tokens,
                total: value.total_tokens,
                cached_input: value.cached_input_tokens,
                cache_write_input: value.cache_write_input_tokens,
            },
            total_latency_ms: value.total_latency_ms,
            first_output_latency_ms: value.first_output_latency_ms,
            cost,
        }
    }
}

impl From<RoutingDecision> for RoutingDecisionDto {
    fn from(value: RoutingDecision) -> Self {
        match value {
            RoutingDecision::RetryCurrent {
                attempt_number,
                max_attempts,
            } => Self::RetryCurrent {
                attempt_number,
                max_attempts,
            },
            RoutingDecision::ActivateNext {
                target_route_id,
                target_route_name,
            } => Self::ActivateNext {
                target_route_id,
                target_route_name,
            },
            RoutingDecision::Stop {
                reason,
                target_route_id,
                target_route_name,
            } => Self::Stop {
                reason: reason.into(),
                target_route_id,
                target_route_name,
            },
        }
    }
}

impl From<FallbackStopReason> for FallbackStopReasonDto {
    fn from(value: FallbackStopReason) -> Self {
        match value {
            FallbackStopReason::FallbackDisabled => Self::FallbackDisabled,
            FallbackStopReason::FailureNotEligible => Self::FailureNotEligible,
            FallbackStopReason::ResponseCommitted => Self::ResponseCommitted,
            FallbackStopReason::AllParticipantsAttempted => Self::AllParticipantsAttempted,
            FallbackStopReason::StalePolicy => Self::StalePolicy,
            FallbackStopReason::ActivationFailed => Self::ActivationFailed,
            FallbackStopReason::AttemptIndexExhausted => Self::AttemptIndexExhausted,
        }
    }
}

impl From<UsageRouteOption> for UsageRouteOptionDto {
    fn from(value: UsageRouteOption) -> Self {
        Self {
            route_id: value.route_id,
            name: value.name,
            retained: value.retained,
        }
    }
}

impl From<UsageAttemptDetail> for UsageAttemptDto {
    fn from(value: UsageAttemptDetail) -> Self {
        let cost = usage_cost(
            value.cost_status,
            value.cost_pico_usd,
            value.pricing_catalog_version,
            value.actual_service_tier.as_deref(),
        );
        Self {
            attempt_index: value.attempt_index,
            route_id: value.route_id,
            route_name: value.route_name,
            started_at_ms: value.started_at_ms,
            finished_at_ms: value.finished_at_ms,
            http_status: value.http_status,
            error_category: value.error_category,
            delivery_state: value.delivery_state,
            actual_model: value.actual_model,
            forwarded_service_tier: value.forwarded_service_tier,
            actual_service_tier: value.actual_service_tier,
            tokens: UsageTokensDto {
                input: value.input_tokens,
                uncached_input: uncached_input(value.input_tokens, value.cached_input_tokens),
                output: value.output_tokens,
                total: value.total_tokens,
                cached_input: value.cached_input_tokens,
                cache_write_input: value.cache_write_input_tokens,
            },
            cost,
            routing_decision: value.routing_decision.map(Into::into),
        }
    }
}

impl From<UsageRequestDetail> for UsageRequestDetailDto {
    fn from(value: UsageRequestDetail) -> Self {
        let tokens = UsageTokensDto {
            input: value.request.input_tokens,
            uncached_input: uncached_input(value.request.input_tokens, value.cached_input_tokens),
            output: value.request.output_tokens,
            total: value.request.total_tokens,
            cached_input: value.cached_input_tokens,
            cache_write_input: value.cache_write_input_tokens,
        };
        Self {
            request: value.request.into(),
            requested_service_tier: value.requested_service_tier,
            actual_service_tier: value.actual_service_tier,
            tokens,
            attempts: value.attempts.into_iter().map(Into::into).collect(),
        }
    }
}

fn usage_cost(
    status: Option<crate::pricing::CostStatus>,
    amount: Option<i64>,
    catalog_version: Option<String>,
    actual_service_tier: Option<&str>,
) -> UsageCostDto {
    let state = match status {
        None => UsageCostStateDto::PreV0_3a,
        Some(crate::pricing::CostStatus::Exact) => UsageCostStateDto::Exact,
        Some(crate::pricing::CostStatus::Partial) => UsageCostStateDto::Partial,
        Some(crate::pricing::CostStatus::Unavailable) => UsageCostStateDto::Unavailable,
        Some(crate::pricing::CostStatus::NotApplicable) => UsageCostStateDto::NotApplicable,
    };
    let service_tier = catalog_version
        .as_deref()
        .and_then(crate::pricing::catalog_service_tier)
        .map(str::to_owned);
    let fast_status = match service_tier.as_deref() {
        Some("priority") if actual_service_tier == Some("priority") => {
            Some(UsageFastStatusDto::Confirmed)
        }
        Some("priority") => Some(UsageFastStatusDto::Unconfirmed),
        _ => None,
    };
    UsageCostDto {
        state,
        amount_pico_usd: amount.map(|value| value.to_string()),
        currency: "USD".to_owned(),
        catalog_version,
        service_tier,
        fast_status,
    }
}

fn uncached_input(input: Option<i64>, cached_input: Option<i64>) -> Option<i64> {
    match (input, cached_input) {
        (Some(input), Some(cached_input))
            if input >= 0 && cached_input >= 0 && cached_input <= input =>
        {
            input.checked_sub(cached_input)
        }
        _ => None,
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct BalanceQueryEditDto {
    pub mode: BalanceQueryMode,
    pub enabled: bool,
    pub custom_source: String,
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct RouteEditDto {
    pub route_id: RouteId,
    pub name: String,
    pub base_url: String,
    pub inference_url: String,
    pub api_key: String,
    pub service_tier_policy: ServiceTierPolicy,
    pub balance_query: Option<BalanceQueryEditDto>,
    pub models: Vec<CodexModelDto>,
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct RouteSaveInputDto {
    pub route_id: Option<RouteId>,
    pub name: String,
    pub base_url: String,
    pub api_key: String,
    pub service_tier_policy: ServiceTierPolicy,
    pub balance_query: Option<BalanceQueryEditDto>,
    pub accept_script_risk: bool,
    pub models: Vec<CodexModelDto>,
    pub retry_token: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct ReorderRoutesAndFallbackInputDto {
    pub ordered_route_ids: Vec<RouteId>,
    pub participant_count: u32,
    #[ts(type = "number")]
    pub expected_config_revision: u64,
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct RouteSaveResultDto {
    pub route_id: RouteId,
    #[ts(type = "number")]
    pub revision: u64,
    pub catalog: ReplaceCodexModelsResult,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct CodexModelDto {
    pub model_id: String,
    pub display_name: Option<String>,
    #[ts(type = "number | null")]
    pub context_window: Option<u64>,
}

impl From<CodexModelRecord> for CodexModelDto {
    fn from(model: CodexModelRecord) -> Self {
        Self {
            model_id: model.model_id,
            display_name: model.display_name,
            context_window: model.context_window,
        }
    }
}

impl From<CodexModelDto> for CodexModelRecord {
    fn from(model: CodexModelDto) -> Self {
        Self {
            model_id: model.model_id,
            display_name: model.display_name,
            context_window: model.context_window,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum CodexModelsActivation {
    None,
    RestartCodex,
    ConnectCodex,
    ReconnectCodex,
    FixCodexConfig,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct ReplaceCodexModelsResult {
    pub models: Vec<CodexModelDto>,
    pub changed: bool,
    pub projection_applied: bool,
    pub retry_required: bool,
    pub activation: CodexModelsActivation,
    pub error_code: Option<String>,
    pub retry_token: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum RouteCatalogMode {
    Original,
    Custom,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct RouteActivationPreviewDto {
    pub target_route_id: RouteId,
    pub target_route_name: String,
    pub target_catalog_mode: RouteCatalogMode,
    pub confirmation_required: bool,
    pub permit: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct RouteActivationResultDto {
    #[ts(type = "number")]
    pub revision: u64,
    pub catalog: ReplaceCodexModelsResult,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct CodexImagesMcpRepairPreviewDto {
    pub permit: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct CodexRestartNoticeDto {
    pub notice_id: String,
    pub route_name: String,
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct BalanceTestInputDto {
    pub base_url: String,
    pub api_key: String,
    pub mode: BalanceQueryMode,
    pub custom_source: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct HistorySummaryDto {
    #[ts(type = "number")]
    pub request_count: u64,
    #[ts(type = "number | null")]
    pub earliest_started_at_ms: Option<i64>,
    #[ts(type = "number | null")]
    pub latest_started_at_ms: Option<i64>,
    #[ts(type = "number")]
    pub database_bytes: u64,
    pub retention_days: u16,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct MetadataFailureDto {
    #[ts(type = "number")]
    pub dropped_records: u64,
    #[ts(type = "number")]
    pub write_failures: u64,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct CodexBaselineSummaryDto {
    pub exists: bool,
    pub original_exists: Option<bool>,
    #[ts(type = "number | null")]
    pub captured_at_ms: Option<i64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct CodexRecoverySummaryDto {
    pub exists: bool,
    pub original_exists: Option<bool>,
    #[ts(type = "number | null")]
    pub updated_at_ms: Option<i64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct CodexRecoveryUpdatePreviewDto {
    pub permit: String,
    pub current_exists: bool,
    #[ts(type = "number | null")]
    pub current_unix_mode: Option<u32>,
    pub recovery_target_exists: bool,
    pub bytes_changed: bool,
    #[ts(type = "number | null")]
    pub recovery_updated_at_ms: Option<i64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct CodexRecoveryResetPreviewDto {
    pub permit: String,
    pub current_exists: bool,
    pub original_exists: bool,
    pub recovery_target_exists: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct BalanceQuerySettingsDto {
    pub menu_debounce_seconds: u16,
    pub automatic_refresh_minutes: u16,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct ImagesGenerationSettingsDto {
    pub enabled: bool,
    pub route_id: Option<RouteId>,
    pub timeout_secs: u16,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct UpdateImagesGenerationSettingsInputDto {
    pub enabled: bool,
    pub route_id: Option<RouteId>,
    pub timeout_secs: u16,
}

impl From<BalanceQueryPolicy> for BalanceQuerySettingsDto {
    fn from(policy: BalanceQueryPolicy) -> Self {
        Self {
            menu_debounce_seconds: policy.menu_debounce_seconds(),
            automatic_refresh_minutes: policy.automatic_refresh_minutes(),
        }
    }
}

impl TryFrom<BalanceQuerySettingsDto> for BalanceQueryPolicy {
    type Error = ValidationError;

    fn try_from(settings: BalanceQuerySettingsDto) -> Result<Self, Self::Error> {
        Self::parse(
            settings.menu_debounce_seconds,
            settings.automatic_refresh_minutes,
        )
    }
}

#[derive(Clone, Deserialize, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct MenuSnapshotDto {
    pub bootstrap: BootstrapSnapshotDto,
    pub balances: Vec<BalanceDisplaySnapshot>,
    pub balance_enabled_route_ids: Vec<RouteId>,
    pub balance_batch: Option<BalanceRefreshBatchState>,
    pub codex_status: CodexConfigStatus,
    pub codex_restart_notice: Option<CodexRestartNoticeDto>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct SettingsSnapshotDto {
    pub routes: Vec<RouteSummaryDto>,
    pub active_route_id: Option<RouteId>,
    pub fallback: FallbackStateDto,
    pub proxy_port: u16,
    pub codex_status: CodexConfigStatus,
    pub baseline: CodexBaselineSummaryDto,
    pub original_backup: CodexBaselineSummaryDto,
    pub recovery_config: CodexRecoverySummaryDto,
    pub balance_script_risk_confirmed: bool,
    pub balance_query: BalanceQuerySettingsDto,
    pub images_generation: ImagesGenerationSettingsDto,
    pub history: HistorySummaryDto,
    pub metadata_failure: MetadataFailureDto,
    pub recovery: RecoveryHealthDto,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum ApplicationUpdateOperationDto {
    Idle,
    Checking,
    Downloading,
    Installing,
    RestartReady,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct ApplicationUpdateNotesDto {
    pub highlights: Vec<String>,
    pub fixes: Vec<String>,
    pub notices: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct ApplicationUpdateReleaseDto {
    pub version: String,
    pub notes: Option<ApplicationUpdateNotesDto>,
    pub legacy_notes: Option<String>,
    pub release_url: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct ApplicationUpdateFailureDto {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct ApplicationUpdateSnapshotDto {
    pub current_version: String,
    pub operation: ApplicationUpdateOperationDto,
    pub available: Option<ApplicationUpdateReleaseDto>,
    #[ts(type = "number | null")]
    pub last_successful_check_at_ms: Option<i64>,
    #[ts(type = "number | null")]
    pub downloaded_bytes: Option<u64>,
    #[ts(type = "number | null")]
    pub total_bytes: Option<u64>,
    pub manual_failure: Option<ApplicationUpdateFailureDto>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct ApplicationUpdateProgressDto {
    pub operation: ApplicationUpdateOperationDto,
    #[ts(type = "number | null")]
    pub downloaded_bytes: Option<u64>,
    #[ts(type = "number | null")]
    pub total_bytes: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct RecoveryHealthDto {
    pub kind: RecoveryHealthKind,
    #[ts(type = "number | null")]
    pub latest_success_at_ms: Option<i64>,
    #[ts(type = "number")]
    pub valid_point_count: usize,
}

impl From<&RecoveryHealth> for RecoveryHealthDto {
    fn from(health: &RecoveryHealth) -> Self {
        Self {
            kind: health.kind,
            latest_success_at_ms: health.latest_success_at_ms,
            valid_point_count: health.valid_point_count,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct RecoveryCandidateDto {
    pub point_id: String,
    #[ts(type = "number")]
    pub created_at_ms: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct RecoverySnapshotDto {
    pub required: bool,
    pub candidates: Vec<RecoveryCandidateDto>,
    pub can_start_over: bool,
    pub startup_issue: Option<DatabaseStartupIssue>,
    pub health: Option<RecoveryHealthDto>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        BalanceQuerySettingsDto, UsageFastStatusDto, UsageHistoryQueryDto, UsageHistoryRowDto,
        UsageStatisticsDto,
    };
    use crate::{
        domain::{BalanceQueryPolicy, CompletionState},
        pricing::{CATALOG_VERSION, CostStatus, PRIORITY_CATALOG_VERSION},
        storage::{
            UsageHistoryRow, UsageStatistics, UsageStatisticsAttribution, UsageStatisticsBucket,
            UsageStatisticsGranularity, UsageStatisticsTokens,
        },
    };

    #[test]
    fn balance_query_settings_dto_rejects_missing_fractional_and_out_of_range_values() {
        assert!(
            serde_json::from_value::<BalanceQuerySettingsDto>(json!({
                "menuDebounceSeconds": 30.5,
                "automaticRefreshMinutes": 30,
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<BalanceQuerySettingsDto>(json!({
                "menuDebounceSeconds": 30,
            }))
            .is_err()
        );
        let zero = serde_json::from_value::<BalanceQuerySettingsDto>(json!({
            "menuDebounceSeconds": 0,
            "automaticRefreshMinutes": 30,
        }))
        .expect("integer DTO");
        assert!(BalanceQueryPolicy::try_from(zero).is_err());
    }

    #[test]
    fn usage_history_query_dto_defaults_omitted_limit_to_fifty() {
        let query = serde_json::from_value::<UsageHistoryQueryDto>(json!({
            "finishedAtOrBeforeMs": 1_700_000_000_000_i64
        }))
        .expect("query with omitted optional fields");
        assert_eq!(query.limit, 50);
    }

    #[test]
    fn usage_statistics_dto_preserves_integer_precision_as_decimal_strings() {
        let tokens = UsageStatisticsTokens {
            total: u64::MAX,
            uncached_input: u64::MAX - 1,
            cached_input: u64::MAX - 2,
            cache_write_input: u64::MAX - 3,
            output: u64::MAX - 4,
        };
        let dto = UsageStatisticsDto::from(UsageStatistics {
            matched_request_count: 2,
            tokens: tokens.clone(),
            cost_pico_usd: u64::MAX,
            granularity: UsageStatisticsGranularity::Hour,
            trend: vec![UsageStatisticsBucket {
                started_at_ms: 1,
                finished_at_ms: 2,
                label: "01/01 00:00".to_owned(),
                request_count: 2,
                tokens,
                cost_pico_usd: u64::MAX - 5,
            }],
            attribution: vec![UsageStatisticsAttribution {
                key: "route:test".to_owned(),
                label: "Test".to_owned(),
                is_other: false,
                value: u64::MAX - 6,
                share_percent: "100.0".to_owned(),
            }],
        });

        assert_eq!(dto.tokens.total, u64::MAX.to_string());
        assert_eq!(dto.tokens.uncached_input, (u64::MAX - 1).to_string());
        assert_eq!(dto.cost_pico_usd, u64::MAX.to_string());
        assert_eq!(dto.trend[0].cost_pico_usd, (u64::MAX - 5).to_string());
        assert_eq!(dto.attribution[0].value, (u64::MAX - 6).to_string());
    }

    #[test]
    fn usage_dto_derives_safe_uncached_input_and_catalog_tier() {
        let row = |input,
                   cached_input,
                   catalog_version: Option<&str>,
                   actual_service_tier: Option<&str>| UsageHistoryRow {
            request_id: "request".to_owned(),
            started_at_ms: 1,
            finished_at_ms: Some(2),
            final_route_id: None,
            final_route_name: None,
            requested_model: Some("gpt-5.6-sol".to_owned()),
            actual_model: None,
            actual_service_tier: actual_service_tier.map(str::to_owned),
            reasoning_effort: None,
            streaming: true,
            completion_state: CompletionState::Completed,
            http_status: Some(200),
            input_tokens: input,
            output_tokens: Some(0),
            total_tokens: input,
            cached_input_tokens: cached_input,
            cache_write_input_tokens: None,
            total_latency_ms: Some(1),
            first_output_latency_ms: Some(1),
            pricing_catalog_version: catalog_version.map(str::to_owned),
            cost_status: Some(CostStatus::Exact),
            upstream_cost_pico_usd: Some(0),
        };

        let priority = UsageHistoryRowDto::from(row(
            Some(60_014),
            Some(59_136),
            Some(PRIORITY_CATALOG_VERSION),
            Some("priority"),
        ));
        assert_eq!(priority.tokens.uncached_input, Some(878));
        assert_eq!(priority.cost.service_tier.as_deref(), Some("priority"));
        assert_eq!(
            priority.cost.fast_status,
            Some(UsageFastStatusDto::Confirmed)
        );

        for actual_service_tier in [None, Some("default"), Some("future-tier")] {
            let unconfirmed = UsageHistoryRowDto::from(row(
                Some(1),
                Some(0),
                Some(PRIORITY_CATALOG_VERSION),
                actual_service_tier,
            ));
            assert_eq!(
                unconfirmed.cost.fast_status,
                Some(UsageFastStatusDto::Unconfirmed)
            );
        }

        let standard = UsageHistoryRowDto::from(row(
            Some(0),
            Some(0),
            Some(CATALOG_VERSION),
            Some("priority"),
        ));
        assert_eq!(standard.tokens.uncached_input, Some(0));
        assert_eq!(standard.cost.service_tier.as_deref(), Some("default"));
        assert_eq!(standard.cost.fast_status, None);

        for invalid in [
            row(None, Some(0), None, Some("priority")),
            row(Some(1), None, None, None),
            row(Some(-1), Some(0), None, None),
            row(Some(1), Some(-1), None, None),
            row(Some(1), Some(2), Some("unknown-catalog"), Some("priority")),
        ] {
            let dto = UsageHistoryRowDto::from(invalid);
            assert_eq!(dto.tokens.uncached_input, None);
            assert_eq!(dto.cost.service_tier, None);
            assert_eq!(dto.cost.fast_status, None);
        }
    }
}
