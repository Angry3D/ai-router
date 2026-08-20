use std::{
    collections::{HashMap, HashSet},
    hash::Hash,
    sync::{
        Arc, RwLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use tokio::sync::{Notify, mpsc};

use axum::http::HeaderMap;
use serde::Deserialize;

use crate::{
    domain::{
        InferenceFailureReason, InferenceOutcome, InferenceStatus, InferenceStatusKind, RouteId,
    },
    storage::{
        AttemptRoutingTransitionRecord, DatabaseExecutor, FallbackStopRecord,
        LatestInferenceAttempt, RequestHistoryRecord,
    },
};

const DEFAULT_HISTORY_QUEUE_CAPACITY: usize = 1_024;
const INFERENCE_EXPIRY_MS: i64 = 24 * 60 * 60 * 1_000;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RuntimeDiagnosticComponent {
    ProxyIngress,
    Upstream,
    RequestHistory,
}

impl RuntimeDiagnosticComponent {
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::ProxyIngress => "proxy_ingress",
            Self::Upstream => "upstream",
            Self::RequestHistory => "request_history",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RuntimeDiagnosticCode {
    InvalidLocalGatewayToken,
    InvalidRequest,
    NoUpstreamRoute,
    UpstreamConnectionFailed,
    UpstreamRequestFailed,
    UpstreamHttpStatus,
    UpstreamInvalidEncoding,
    UpstreamTimeout,
    UpstreamResponseTooLarge,
    UpstreamReadFailed,
    UpstreamSemanticFailure,
    UpstreamPreflightProtocolMismatch,
    UpstreamPreflightMeaningfulOutput,
    UpstreamPreflightTerminalSuccess,
    UpstreamPreflightUnknownEvent,
    UpstreamPreflightMalformedEvent,
    UpstreamPreflightEventLimit,
    UpstreamPreflightBufferLimit,
    UpstreamPreflightPolicyChanged,
    UpstreamPreflightSemanticFailure,
    FallbackActivationPersistenceFailed,
    MetadataQueueFull,
    MetadataQueueClosed,
    MetadataWriteFailed,
}

impl RuntimeDiagnosticCode {
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::InvalidLocalGatewayToken => "invalid_local_gateway_token",
            Self::InvalidRequest => "invalid_request",
            Self::NoUpstreamRoute => "no_upstream_route",
            Self::UpstreamConnectionFailed => "upstream_connection_failed",
            Self::UpstreamRequestFailed => "upstream_request_failed",
            Self::UpstreamHttpStatus => "upstream_http_status",
            Self::UpstreamInvalidEncoding => "upstream_invalid_encoding",
            Self::UpstreamTimeout => "upstream_timeout",
            Self::UpstreamResponseTooLarge => "upstream_response_too_large",
            Self::UpstreamReadFailed => "upstream_read_failed",
            Self::UpstreamSemanticFailure => "upstream_semantic_failure",
            Self::UpstreamPreflightProtocolMismatch => "upstream_preflight_protocol_mismatch",
            Self::UpstreamPreflightMeaningfulOutput => "upstream_preflight_meaningful_output",
            Self::UpstreamPreflightTerminalSuccess => "upstream_preflight_terminal_success",
            Self::UpstreamPreflightUnknownEvent => "upstream_preflight_unknown_event",
            Self::UpstreamPreflightMalformedEvent => "upstream_preflight_malformed_event",
            Self::UpstreamPreflightEventLimit => "upstream_preflight_event_limit",
            Self::UpstreamPreflightBufferLimit => "upstream_preflight_buffer_limit",
            Self::UpstreamPreflightPolicyChanged => "upstream_preflight_policy_changed",
            Self::UpstreamPreflightSemanticFailure => "upstream_preflight_semantic_failure",
            Self::FallbackActivationPersistenceFailed => "fallback_activation_persistence_failed",
            Self::MetadataQueueFull => "metadata_queue_full",
            Self::MetadataQueueClosed => "metadata_queue_closed",
            Self::MetadataWriteFailed => "metadata_write_failed",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeDiagnosticEvent {
    pub component: RuntimeDiagnosticComponent,
    pub code: RuntimeDiagnosticCode,
    pub request_id: Option<String>,
    pub route_id: Option<RouteId>,
    pub http_status: Option<u16>,
}

pub trait RuntimeDiagnosticSink: Send + Sync {
    fn emit(&self, event: RuntimeDiagnosticEvent);
}

pub trait HistorySink: Send + Sync {
    fn try_record(&self, record: RequestHistoryRecord) -> bool;

    fn try_record_fallback_stop(&self, _record: FallbackStopRecord) -> bool {
        true
    }

    fn try_record_routing_transition(&self, _record: AttemptRoutingTransitionRecord) -> bool {
        true
    }
}

pub trait HistorySummaryChangeSink: Send + Sync {
    fn history_summary_changed(&self);
}

pub(super) struct NoopRuntimeDiagnosticSink;

impl RuntimeDiagnosticSink for NoopRuntimeDiagnosticSink {
    fn emit(&self, _event: RuntimeDiagnosticEvent) {}
}

pub(super) struct NoopHistorySink;

impl HistorySink for NoopHistorySink {
    fn try_record(&self, _record: RequestHistoryRecord) -> bool {
        true
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MetadataFailureSnapshot {
    pub dropped_records: u64,
    pub write_failures: u64,
    pub last_error: Option<RuntimeDiagnosticCode>,
}

struct MetadataFailureState {
    dropped_records: AtomicU64,
    write_failures: AtomicU64,
    last_error: RwLock<Option<RuntimeDiagnosticCode>>,
}

impl MetadataFailureState {
    fn record_drop(&self, code: RuntimeDiagnosticCode) {
        self.dropped_records.fetch_add(1, Ordering::Relaxed);
        *self
            .last_error
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(code);
    }

    fn record_write_failure(&self) {
        self.write_failures.fetch_add(1, Ordering::Relaxed);
        *self
            .last_error
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some(RuntimeDiagnosticCode::MetadataWriteFailed);
    }

    fn snapshot(&self) -> MetadataFailureSnapshot {
        MetadataFailureSnapshot {
            dropped_records: self.dropped_records.load(Ordering::Relaxed),
            write_failures: self.write_failures.load(Ordering::Relaxed),
            last_error: self
                .last_error
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone(),
        }
    }
}

pub struct AsyncHistoryRecorder {
    sender: RwLock<Option<mpsc::Sender<HistoryCommand>>>,
    worker: std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
    failures: Arc<MetadataFailureState>,
    diagnostics: Arc<dyn RuntimeDiagnosticSink>,
    changes: Arc<dyn HistorySummaryChangeSink>,
}

enum HistoryCommand {
    Request(Box<RequestHistoryRecord>),
    FallbackStop(FallbackStopRecord),
    RoutingTransition(AttemptRoutingTransitionRecord),
}

fn buffer_pending_metadata<K, V>(
    pending: &mut HashMap<K, V>,
    key: K,
    record: V,
    capacity: usize,
    failures: &MetadataFailureState,
    diagnostics: &dyn RuntimeDiagnosticSink,
    request_id: &str,
) where
    K: Eq + Hash,
{
    if pending.len() < capacity || pending.contains_key(&key) {
        pending.insert(key, record);
    } else {
        failures.record_drop(RuntimeDiagnosticCode::MetadataQueueFull);
        diagnostics.emit(RuntimeDiagnosticEvent {
            component: RuntimeDiagnosticComponent::RequestHistory,
            code: RuntimeDiagnosticCode::MetadataQueueFull,
            request_id: Some(request_id.to_owned()),
            route_id: None,
            http_status: None,
        });
    }
}

fn finish_history_command(
    result: &Result<(), crate::storage::StorageError>,
    request_id: String,
    failures: &MetadataFailureState,
    diagnostics: &dyn RuntimeDiagnosticSink,
    changes: &dyn HistorySummaryChangeSink,
) {
    if result.is_err() {
        failures.record_write_failure();
        diagnostics.emit(RuntimeDiagnosticEvent {
            component: RuntimeDiagnosticComponent::RequestHistory,
            code: RuntimeDiagnosticCode::MetadataWriteFailed,
            request_id: Some(request_id),
            route_id: None,
            http_status: None,
        });
    }
    changes.history_summary_changed();
}

fn history_attempt_indexes(record: &RequestHistoryRecord) -> Vec<u32> {
    record
        .attempts
        .iter()
        .map(|attempt| attempt.attempt_index)
        .collect()
}

impl AsyncHistoryRecorder {
    #[must_use]
    pub fn new(
        database: DatabaseExecutor,
        diagnostics: Arc<dyn RuntimeDiagnosticSink>,
        changes: Arc<dyn HistorySummaryChangeSink>,
    ) -> Arc<Self> {
        Self::with_capacity(
            database,
            diagnostics,
            changes,
            DEFAULT_HISTORY_QUEUE_CAPACITY,
        )
    }

    fn with_capacity(
        database: DatabaseExecutor,
        diagnostics: Arc<dyn RuntimeDiagnosticSink>,
        changes: Arc<dyn HistorySummaryChangeSink>,
        capacity: usize,
    ) -> Arc<Self> {
        let (sender, mut receiver) = mpsc::channel::<HistoryCommand>(capacity);
        let failures = Arc::new(MetadataFailureState {
            dropped_records: AtomicU64::new(0),
            write_failures: AtomicU64::new(0),
            last_error: RwLock::new(None),
        });
        let worker_failures = Arc::clone(&failures);
        let worker_diagnostics = Arc::clone(&diagnostics);
        let worker_changes = Arc::clone(&changes);
        let worker = tokio::spawn(async move {
            let mut pending_stops = HashMap::<(String, u32), FallbackStopRecord>::new();
            let mut pending_transitions =
                HashMap::<(String, u32), AttemptRoutingTransitionRecord>::new();
            while let Some(command) = receiver.recv().await {
                let (request_id, result) = match command {
                    HistoryCommand::Request(record) => {
                        let request_id = record.request_id.clone();
                        let attempt_indexes = history_attempt_indexes(&record);
                        let result = async {
                            database.record_request_history(*record).await?;
                            for attempt_index in attempt_indexes {
                                let key = (request_id.clone(), attempt_index);
                                if let Some(transition) = pending_transitions.remove(&key) {
                                    let _ = database
                                        .record_attempt_routing_transition(transition)
                                        .await?;
                                }
                                if let Some(stop) = pending_stops.remove(&key) {
                                    let _ = database.record_fallback_stop(stop).await?;
                                }
                            }
                            Ok(())
                        }
                        .await;
                        (request_id, result)
                    }
                    HistoryCommand::FallbackStop(record) => {
                        let request_id = record.request_id.clone();
                        let key = (request_id.clone(), record.attempt_index);
                        let result = match database.record_fallback_stop(record.clone()).await {
                            Ok(true) => Ok(()),
                            Ok(false) => {
                                buffer_pending_metadata(
                                    &mut pending_stops,
                                    key,
                                    record,
                                    capacity,
                                    &worker_failures,
                                    worker_diagnostics.as_ref(),
                                    &request_id,
                                );
                                Ok(())
                            }
                            Err(error) => Err(error),
                        };
                        (request_id, result)
                    }
                    HistoryCommand::RoutingTransition(record) => {
                        let request_id = record.request_id.clone();
                        let key = (request_id.clone(), record.attempt_index);
                        let result = match database
                            .record_attempt_routing_transition(record.clone())
                            .await
                        {
                            Ok(true) => Ok(()),
                            Ok(false) => {
                                buffer_pending_metadata(
                                    &mut pending_transitions,
                                    key,
                                    record,
                                    capacity,
                                    &worker_failures,
                                    worker_diagnostics.as_ref(),
                                    &request_id,
                                );
                                Ok(())
                            }
                            Err(error) => Err(error),
                        };
                        (request_id, result)
                    }
                };
                finish_history_command(
                    &result,
                    request_id,
                    &worker_failures,
                    worker_diagnostics.as_ref(),
                    worker_changes.as_ref(),
                );
            }
        });
        Arc::new(Self {
            sender: RwLock::new(Some(sender)),
            worker: std::sync::Mutex::new(Some(worker)),
            failures,
            diagnostics,
            changes,
        })
    }

    #[must_use]
    pub fn failure_snapshot(&self) -> MetadataFailureSnapshot {
        self.failures.snapshot()
    }

    pub async fn shutdown(&self) {
        self.sender
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        let worker = self
            .worker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(worker) = worker {
            let _ = worker.await;
        }
    }
}

impl HistorySink for AsyncHistoryRecorder {
    fn try_record(&self, record: RequestHistoryRecord) -> bool {
        let request_id = record.request_id.clone();
        let sender = self
            .sender
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let Some(sender) = sender else {
            self.failures
                .record_drop(RuntimeDiagnosticCode::MetadataQueueClosed);
            self.diagnostics.emit(RuntimeDiagnosticEvent {
                component: RuntimeDiagnosticComponent::RequestHistory,
                code: RuntimeDiagnosticCode::MetadataQueueClosed,
                request_id: Some(request_id),
                route_id: None,
                http_status: None,
            });
            self.changes.history_summary_changed();
            return false;
        };
        match sender.try_send(HistoryCommand::Request(Box::new(record))) {
            Ok(()) => true,
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.failures
                    .record_drop(RuntimeDiagnosticCode::MetadataQueueFull);
                self.diagnostics.emit(RuntimeDiagnosticEvent {
                    component: RuntimeDiagnosticComponent::RequestHistory,
                    code: RuntimeDiagnosticCode::MetadataQueueFull,
                    request_id: Some(request_id),
                    route_id: None,
                    http_status: None,
                });
                self.changes.history_summary_changed();
                false
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                self.failures
                    .record_drop(RuntimeDiagnosticCode::MetadataQueueClosed);
                self.diagnostics.emit(RuntimeDiagnosticEvent {
                    component: RuntimeDiagnosticComponent::RequestHistory,
                    code: RuntimeDiagnosticCode::MetadataQueueClosed,
                    request_id: Some(request_id),
                    route_id: None,
                    http_status: None,
                });
                self.changes.history_summary_changed();
                false
            }
        }
    }

    fn try_record_fallback_stop(&self, record: FallbackStopRecord) -> bool {
        let request_id = record.request_id.clone();
        let sender = self
            .sender
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let Some(sender) = sender else {
            self.failures
                .record_drop(RuntimeDiagnosticCode::MetadataQueueClosed);
            self.diagnostics.emit(RuntimeDiagnosticEvent {
                component: RuntimeDiagnosticComponent::RequestHistory,
                code: RuntimeDiagnosticCode::MetadataQueueClosed,
                request_id: Some(request_id),
                route_id: None,
                http_status: None,
            });
            self.changes.history_summary_changed();
            return false;
        };
        match sender.try_send(HistoryCommand::FallbackStop(record)) {
            Ok(()) => true,
            Err(mpsc::error::TrySendError::Full(_) | mpsc::error::TrySendError::Closed(_)) => {
                let code = if sender.is_closed() {
                    RuntimeDiagnosticCode::MetadataQueueClosed
                } else {
                    RuntimeDiagnosticCode::MetadataQueueFull
                };
                self.failures.record_drop(code.clone());
                self.diagnostics.emit(RuntimeDiagnosticEvent {
                    component: RuntimeDiagnosticComponent::RequestHistory,
                    code,
                    request_id: Some(request_id),
                    route_id: None,
                    http_status: None,
                });
                self.changes.history_summary_changed();
                false
            }
        }
    }

    fn try_record_routing_transition(&self, record: AttemptRoutingTransitionRecord) -> bool {
        let request_id = record.request_id.clone();
        let sender = self
            .sender
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let Some(sender) = sender else {
            self.failures
                .record_drop(RuntimeDiagnosticCode::MetadataQueueClosed);
            return false;
        };
        match sender.try_send(HistoryCommand::RoutingTransition(record)) {
            Ok(()) => true,
            Err(mpsc::error::TrySendError::Full(_) | mpsc::error::TrySendError::Closed(_)) => {
                let code = if sender.is_closed() {
                    RuntimeDiagnosticCode::MetadataQueueClosed
                } else {
                    RuntimeDiagnosticCode::MetadataQueueFull
                };
                self.failures.record_drop(code.clone());
                self.diagnostics.emit(RuntimeDiagnosticEvent {
                    component: RuntimeDiagnosticComponent::RequestHistory,
                    code,
                    request_id: Some(request_id),
                    route_id: None,
                    http_status: None,
                });
                self.changes.history_summary_changed();
                false
            }
        }
    }
}

pub trait InferenceStatusChangeSink: Send + Sync {
    fn inference_statuses_changed(&self, updates: Vec<(RouteId, InferenceStatus)>);
}

#[derive(Clone)]
struct LiveInferenceStatus {
    outcome: InferenceOutcome,
    failure_reason: Option<InferenceFailureReason>,
    observed_at_ms: i64,
    expired: bool,
}

struct InferenceStatusInner {
    statuses: RwLock<HashMap<RouteId, LiveInferenceStatus>>,
    changes: Arc<dyn InferenceStatusChangeSink>,
    expiry_ms: i64,
    notify: Notify,
}

#[derive(Clone)]
pub struct InferenceStatusService {
    inner: Arc<InferenceStatusInner>,
}

impl InferenceStatusService {
    #[must_use]
    pub fn new(changes: Arc<dyn InferenceStatusChangeSink>) -> Self {
        let service = Self::with_expiry(changes, INFERENCE_EXPIRY_MS);
        service.start_expiry_worker();
        service
    }

    fn with_expiry(changes: Arc<dyn InferenceStatusChangeSink>, expiry_ms: i64) -> Self {
        Self {
            inner: Arc::new(InferenceStatusInner {
                statuses: RwLock::new(HashMap::new()),
                changes,
                expiry_ms,
                notify: Notify::new(),
            }),
        }
    }

    pub fn record_result(
        &self,
        route_id: &RouteId,
        outcome: InferenceOutcome,
        observed_at_ms: i64,
    ) {
        self.record_result_with_reason(route_id, outcome, None, observed_at_ms);
    }

    pub fn record_result_with_reason(
        &self,
        route_id: &RouteId,
        outcome: InferenceOutcome,
        failure_reason: Option<InferenceFailureReason>,
        observed_at_ms: i64,
    ) {
        self.inner
            .statuses
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(
                route_id.clone(),
                LiveInferenceStatus {
                    outcome,
                    failure_reason,
                    observed_at_ms,
                    expired: false,
                },
            );
        self.inner.changes.inference_statuses_changed(vec![(
            route_id.clone(),
            self.status(route_id, observed_at_ms),
        )]);
        self.inner.notify.notify_one();
    }

    pub fn reconstruct(
        &self,
        attempts: Vec<LatestInferenceAttempt>,
        live_route_ids: &[RouteId],
        now_ms: i64,
    ) {
        let live = live_route_ids.iter().cloned().collect::<HashSet<_>>();
        let mut statuses = HashMap::new();
        for attempt in attempts {
            if !live.contains(&attempt.route_id) || statuses.contains_key(&attempt.route_id) {
                continue;
            }
            statuses.insert(
                attempt.route_id,
                LiveInferenceStatus {
                    outcome: if attempt.succeeded {
                        InferenceOutcome::Success
                    } else {
                        InferenceOutcome::Failure
                    },
                    failure_reason: if attempt.succeeded {
                        None
                    } else {
                        inference_reason_from_category(attempt.error_category.as_deref())
                    },
                    observed_at_ms: attempt.finished_at_ms,
                    expired: now_ms.saturating_sub(attempt.finished_at_ms) >= self.inner.expiry_ms,
                },
            );
        }
        *self
            .inner
            .statuses
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = statuses;
        self.inner.notify.notify_one();
    }

    /// Rebuilds live route statuses from retained history.
    ///
    /// # Errors
    ///
    /// Returns a database executor or query error.
    pub async fn reconstruct_from_database(
        &self,
        database: &DatabaseExecutor,
        live_route_ids: &[RouteId],
        now_ms: i64,
    ) -> Result<(), crate::storage::StorageError> {
        let attempts = database.latest_inference_attempts().await?;
        self.reconstruct(attempts, live_route_ids, now_ms);
        Ok(())
    }

    /// Commits history deletion before resetting the in-memory route projection.
    ///
    /// # Errors
    ///
    /// Returns a database error without changing inference status when deletion
    /// does not commit.
    pub async fn clear_history_and_reset(
        &self,
        database: &DatabaseExecutor,
    ) -> Result<crate::storage::ClearHistoryResult, crate::storage::StorageError> {
        let result = database.clear_history().await?;
        self.clear();
        Ok(result)
    }

    #[must_use]
    pub fn status(&self, route_id: &RouteId, now_ms: i64) -> InferenceStatus {
        let statuses = self
            .inner
            .statuses
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(status) = statuses.get(route_id) else {
            return unverified_inference_status();
        };
        let expired =
            status.expired || now_ms.saturating_sub(status.observed_at_ms) >= self.inner.expiry_ms;
        InferenceStatus {
            kind: if expired {
                InferenceStatusKind::Expired
            } else if status.outcome == InferenceOutcome::Success {
                InferenceStatusKind::RecentSuccess
            } else {
                InferenceStatusKind::RecentFailure
            },
            last_outcome: Some(status.outcome.clone()),
            failure_reason: status.failure_reason,
            observed_at_ms: Some(status.observed_at_ms),
        }
    }

    pub fn clear(&self) {
        let mut statuses = self
            .inner
            .statuses
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let updates = statuses
            .keys()
            .cloned()
            .map(|route_id| (route_id, unverified_inference_status()))
            .collect::<Vec<_>>();
        statuses.clear();
        drop(statuses);
        if !updates.is_empty() {
            self.inner.changes.inference_statuses_changed(updates);
        }
        self.inner.notify.notify_one();
    }

    pub fn remove_route(&self, route_id: &RouteId) {
        let removed = self
            .inner
            .statuses
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(route_id);
        if removed.is_some() {
            self.inner.changes.inference_statuses_changed(vec![(
                route_id.clone(),
                unverified_inference_status(),
            )]);
        }
        self.inner.notify.notify_one();
    }

    fn expire_due(&self, now_ms: i64) {
        let mut statuses = self
            .inner
            .statuses
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut updates = Vec::new();
        for (route_id, status) in statuses.iter_mut() {
            if !status.expired
                && now_ms.saturating_sub(status.observed_at_ms) >= self.inner.expiry_ms
            {
                status.expired = true;
                updates.push((
                    route_id.clone(),
                    InferenceStatus {
                        kind: InferenceStatusKind::Expired,
                        last_outcome: Some(status.outcome.clone()),
                        failure_reason: status.failure_reason,
                        observed_at_ms: Some(status.observed_at_ms),
                    },
                ));
            }
        }
        drop(statuses);
        if !updates.is_empty() {
            self.inner.changes.inference_statuses_changed(updates);
        }
    }

    fn next_expiry_ms(&self) -> Option<i64> {
        self.inner
            .statuses
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .filter(|status| !status.expired)
            .map(|status| status.observed_at_ms.saturating_add(self.inner.expiry_ms))
            .min()
    }

    fn start_expiry_worker(&self) {
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let service = self.clone();
        tokio::spawn(async move {
            loop {
                let Some(deadline_ms) = service.next_expiry_ms() else {
                    service.inner.notify.notified().await;
                    continue;
                };
                let wait_ms = deadline_ms
                    .saturating_sub(now_millis())
                    .max(0)
                    .cast_unsigned();
                tokio::select! {
                    () = tokio::time::sleep(Duration::from_millis(wait_ms)) => {
                        service.expire_due(now_millis());
                    }
                    () = service.inner.notify.notified() => {}
                }
            }
        });
    }
}

fn unverified_inference_status() -> InferenceStatus {
    InferenceStatus {
        kind: InferenceStatusKind::Unverified,
        last_outcome: None,
        failure_reason: None,
        observed_at_ms: None,
    }
}

fn inference_reason_from_category(category: Option<&str>) -> Option<InferenceFailureReason> {
    match category? {
        "upstream_connection_failed" | "upstream_request_failed" | "upstream_read_failed" => {
            Some(InferenceFailureReason::Connection)
        }
        "upstream_timeout" => Some(InferenceFailureReason::Timeout),
        "upstream_http_status" | "upstream_overloaded" | "upstream_model_unavailable" => {
            Some(InferenceFailureReason::Service)
        }
        "upstream_rate_limited" => Some(InferenceFailureReason::RateLimit),
        "invalid_api_key" => Some(InferenceFailureReason::InvalidKey),
        "insufficient_quota" => Some(InferenceFailureReason::InsufficientQuota),
        "billing_hard_limit_reached" => Some(InferenceFailureReason::BillingLimit),
        "upstream_auth_failed" => Some(InferenceFailureReason::Authentication),
        "upstream_access_denied" => Some(InferenceFailureReason::AccessDenied),
        _ => None,
    }
}

pub(super) fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(i64::MAX)
}

#[derive(Deserialize)]
struct TurnMetadataProjection {
    turn_id: Option<String>,
}

pub(super) fn parse_turn_id(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get("x-codex-turn-metadata")?.to_str().ok()?;
    let projection: TurnMetadataProjection = serde_json::from_str(raw).ok()?;
    projection
        .turn_id
        .map(|turn_id| bounded_string(turn_id, 256))
}

pub(super) fn bounded_string(mut value: String, maximum: usize) -> String {
    if let Some((index, _)) = value.char_indices().nth(maximum) {
        value.truncate(index);
    }
    value
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    use super::*;
    use crate::domain::CompletionState;

    #[derive(Default)]
    struct ChangeCounter(AtomicUsize);

    impl InferenceStatusChangeSink for ChangeCounter {
        fn inference_statuses_changed(&self, _updates: Vec<(RouteId, InferenceStatus)>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    impl HistorySummaryChangeSink for ChangeCounter {
        fn history_summary_changed(&self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[derive(Default)]
    struct DiagnosticCapture(Mutex<Vec<RuntimeDiagnosticEvent>>);

    impl RuntimeDiagnosticSink for DiagnosticCapture {
        fn emit(&self, event: RuntimeDiagnosticEvent) {
            self.0.lock().expect("diagnostic mutex").push(event);
        }
    }

    fn history_record(request_id: &str) -> RequestHistoryRecord {
        RequestHistoryRecord {
            request_id: request_id.to_owned(),
            started_at_ms: 1,
            finished_at_ms: 2,
            turn_id: None,
            requested_model: Some("model".to_owned()),
            reasoning_effort: None,
            requested_service_tier: None,
            actual_model: None,
            actual_service_tier: None,
            final_route_id: None,
            final_route_name: None,
            streaming: false,
            completion_state: CompletionState::Failed,
            http_status: Some(400),
            error_category: Some("invalid_request".to_owned()),
            input_tokens: None,
            output_tokens: None,
            total_tokens: None,
            cached_input_tokens: None,
            cache_write_input_tokens: None,
            total_latency_ms: Some(1),
            first_output_latency_ms: None,
            metadata_complete: false,
            fallback_stop_reason: None,
            fallback_stop_target_route_id: None,
            fallback_stop_target_route_name: None,
            attempts: Vec::new(),
        }
    }

    #[test]
    fn inference_status_is_route_scoped_expires_and_clears() {
        let changes = Arc::new(ChangeCounter::default());
        let service = InferenceStatusService::with_expiry(changes.clone(), 100);
        let success = RouteId::new();
        let failure = RouteId::new();
        let untouched = RouteId::new();
        service.record_result(&success, InferenceOutcome::Success, 1_000);
        service.record_result(&failure, InferenceOutcome::Failure, 1_050);

        assert_eq!(
            service.status(&success, 1_099).kind,
            InferenceStatusKind::RecentSuccess
        );
        assert_eq!(
            service.status(&failure, 1_099).kind,
            InferenceStatusKind::RecentFailure
        );
        assert_eq!(
            service.status(&untouched, 1_099).kind,
            InferenceStatusKind::Unverified
        );
        service.expire_due(1_100);
        let expired = service.status(&success, 1_100);
        assert_eq!(expired.kind, InferenceStatusKind::Expired);
        assert_eq!(expired.last_outcome, Some(InferenceOutcome::Success));
        assert_eq!(expired.observed_at_ms, Some(1_000));

        service.clear();
        assert_eq!(
            service.status(&success, 1_100).kind,
            InferenceStatusKind::Unverified
        );
        assert_eq!(changes.0.load(Ordering::SeqCst), 4);
    }

    #[test]
    fn inference_status_reconstructs_latest_live_attempts_only() {
        let changes = Arc::new(ChangeCounter::default());
        let service = InferenceStatusService::with_expiry(changes, 100);
        let live = RouteId::new();
        let overloaded = RouteId::new();
        let deleted = RouteId::new();
        service.reconstruct(
            vec![
                LatestInferenceAttempt {
                    route_id: live.clone(),
                    finished_at_ms: 900,
                    succeeded: true,
                    error_category: None,
                },
                LatestInferenceAttempt {
                    route_id: overloaded.clone(),
                    finished_at_ms: 975,
                    succeeded: false,
                    error_category: Some("upstream_overloaded".to_owned()),
                },
                LatestInferenceAttempt {
                    route_id: deleted,
                    finished_at_ms: 950,
                    succeeded: false,
                    error_category: Some("upstream_http_status".to_owned()),
                },
            ],
            &[live.clone(), overloaded.clone()],
            1_000,
        );

        assert_eq!(
            service.status(&live, 1_000),
            InferenceStatus {
                kind: InferenceStatusKind::Expired,
                last_outcome: Some(InferenceOutcome::Success),
                failure_reason: None,
                observed_at_ms: Some(900),
            }
        );
        assert_eq!(
            service.status(&overloaded, 1_000),
            InferenceStatus {
                kind: InferenceStatusKind::RecentFailure,
                last_outcome: Some(InferenceOutcome::Failure),
                failure_reason: Some(InferenceFailureReason::Service),
                observed_at_ms: Some(975),
            }
        );
    }

    #[test]
    fn request_history_queue_full_is_non_blocking_and_counted() {
        let (sender, _receiver) = mpsc::channel(1);
        let diagnostics = Arc::new(DiagnosticCapture::default());
        let changes = Arc::new(ChangeCounter::default());
        let recorder = AsyncHistoryRecorder {
            sender: RwLock::new(Some(sender)),
            worker: std::sync::Mutex::new(None),
            failures: Arc::new(MetadataFailureState {
                dropped_records: AtomicU64::new(0),
                write_failures: AtomicU64::new(0),
                last_error: RwLock::new(None),
            }),
            diagnostics: diagnostics.clone(),
            changes: changes.clone(),
        };

        assert!(recorder.try_record(history_record("queued")));
        assert!(!recorder.try_record(history_record("dropped")));
        assert_eq!(
            recorder.failure_snapshot(),
            MetadataFailureSnapshot {
                dropped_records: 1,
                write_failures: 0,
                last_error: Some(RuntimeDiagnosticCode::MetadataQueueFull),
            }
        );
        assert_eq!(
            diagnostics.0.lock().expect("diagnostic mutex")[0].code,
            RuntimeDiagnosticCode::MetadataQueueFull
        );
        assert_eq!(changes.0.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn request_history_success_publishes_summary_change_after_persistence() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let database =
            DatabaseExecutor::open(directory.path().join("router.sqlite3")).expect("database");
        let diagnostics = Arc::new(DiagnosticCapture::default());
        let changes = Arc::new(ChangeCounter::default());
        let recorder = AsyncHistoryRecorder::new(database.clone(), diagnostics, changes.clone());

        assert!(recorder.try_record(history_record("persisted")));
        for _ in 0..100 {
            if changes.0.load(Ordering::SeqCst) == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }

        assert_eq!(changes.0.load(Ordering::SeqCst), 1);
        assert_eq!(
            database
                .history_summary()
                .await
                .expect("history summary")
                .request_count,
            1
        );
    }

    #[tokio::test]
    async fn fallback_stop_is_ordered_after_attempt_history_and_materialized_in_detail() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let database =
            DatabaseExecutor::open(directory.path().join("router.sqlite3")).expect("database");
        let recorder = AsyncHistoryRecorder::new(
            database.clone(),
            Arc::new(DiagnosticCapture::default()),
            Arc::new(ChangeCounter::default()),
        );
        let route_id = RouteId::new();
        let deleted_target_route_id = RouteId::new();
        let mut record = history_record("fallback-stop");
        record.final_route_id = Some(route_id.clone());
        record.final_route_name = Some("Retained route".to_owned());
        record.attempts.push(crate::storage::AttemptHistoryRecord {
            attempt_id: crate::domain::UpstreamAttemptId::new(),
            attempt_index: 0,
            attempt_role: crate::storage::AttemptRole::Ordinary,
            route_id: route_id.clone(),
            route_name: "Retained route".to_owned(),
            started_at_ms: 1,
            finished_at_ms: 2,
            http_status: Some(503),
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
        });

        assert!(recorder.try_record(record));
        assert!(
            recorder.try_record_fallback_stop(crate::storage::FallbackStopRecord {
                request_id: "fallback-stop".to_owned(),
                attempt_index: 0,
                reason: crate::storage::FallbackStopReason::ActivationFailed,
                target_route_id: Some(deleted_target_route_id.clone()),
                target_route_name: Some("Deleted target snapshot".to_owned()),
            })
        );
        recorder.shutdown().await;

        let detail = database
            .usage_request_detail("fallback-stop".to_owned())
            .await
            .expect("usage detail");
        assert_eq!(
            detail.attempts[0].routing_decision,
            Some(crate::storage::RoutingDecision::Stop {
                reason: crate::storage::FallbackStopReason::ActivationFailed,
                target_route_id: Some(deleted_target_route_id),
                target_route_name: Some("Deleted target snapshot".to_owned()),
            })
        );
    }

    #[tokio::test]
    async fn transition_and_stop_buffer_until_their_streaming_attempt_is_persisted() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let database =
            DatabaseExecutor::open(directory.path().join("router.sqlite3")).expect("database");
        let recorder = AsyncHistoryRecorder::new(
            database.clone(),
            Arc::new(DiagnosticCapture::default()),
            Arc::new(ChangeCounter::default()),
        );
        let route_id = RouteId::new();
        let target_route_id = RouteId::new();
        assert!(recorder.try_record_routing_transition(
            crate::storage::AttemptRoutingTransitionRecord {
                request_id: "reverse-metadata".to_owned(),
                attempt_index: 0,
                transition: crate::storage::AttemptRoutingTransition {
                    kind: crate::storage::RoutingTransitionKind::ActivateNext,
                    target_route_id: target_route_id.clone(),
                    target_route_name: "Next route".to_owned(),
                    skipped_routes: Vec::new(),
                },
            }
        ));
        assert!(
            recorder.try_record_fallback_stop(crate::storage::FallbackStopRecord {
                request_id: "reverse-metadata".to_owned(),
                attempt_index: 1,
                reason: crate::storage::FallbackStopReason::ResponseCommitted,
                target_route_id: Some(target_route_id.clone()),
                target_route_name: Some("Next route".to_owned()),
            })
        );
        let mut record = history_record("reverse-metadata");
        record.streaming = true;
        record.attempts.push(crate::storage::AttemptHistoryRecord {
            attempt_id: crate::domain::UpstreamAttemptId::new(),
            attempt_index: 0,
            attempt_role: crate::storage::AttemptRole::Ordinary,
            route_id,
            route_name: "Source route".to_owned(),
            started_at_ms: 1,
            finished_at_ms: 2,
            http_status: Some(200),
            error_category: None,
            delivery_state: crate::domain::DeliveryState::Completed,
            actual_model: None,
            forwarded_service_tier: None,
            actual_service_tier: None,
            input_tokens: None,
            output_tokens: None,
            total_tokens: None,
            cached_input_tokens: None,
            cache_write_input_tokens: None,
        });
        record.attempts.push(crate::storage::AttemptHistoryRecord {
            attempt_id: crate::domain::UpstreamAttemptId::new(),
            attempt_index: 1,
            attempt_role: crate::storage::AttemptRole::Ordinary,
            route_id: target_route_id.clone(),
            route_name: "Next route".to_owned(),
            started_at_ms: 2,
            finished_at_ms: 3,
            http_status: Some(200),
            error_category: None,
            delivery_state: crate::domain::DeliveryState::Completed,
            actual_model: None,
            forwarded_service_tier: None,
            actual_service_tier: None,
            input_tokens: None,
            output_tokens: None,
            total_tokens: None,
            cached_input_tokens: None,
            cache_write_input_tokens: None,
        });
        assert!(recorder.try_record(record));
        recorder.shutdown().await;

        let detail = database
            .usage_request_detail("reverse-metadata".to_owned())
            .await
            .expect("usage detail");
        assert_eq!(
            detail.attempts[0].routing_transition,
            Some(crate::storage::AttemptRoutingTransition {
                kind: crate::storage::RoutingTransitionKind::ActivateNext,
                target_route_id: target_route_id.clone(),
                target_route_name: "Next route".to_owned(),
                skipped_routes: Vec::new(),
            })
        );
        assert_eq!(
            detail.attempts[1].routing_decision,
            Some(crate::storage::RoutingDecision::Stop {
                reason: crate::storage::FallbackStopReason::ResponseCommitted,
                target_route_id: Some(target_route_id),
                target_route_name: Some("Next route".to_owned()),
            })
        );
    }

    #[tokio::test]
    async fn fallback_stop_is_not_attached_when_its_attempt_was_not_persisted() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let database =
            DatabaseExecutor::open(directory.path().join("router.sqlite3")).expect("database");
        let recorder = AsyncHistoryRecorder::new(
            database.clone(),
            Arc::new(DiagnosticCapture::default()),
            Arc::new(ChangeCounter::default()),
        );
        let route_id = RouteId::new();
        let mut record = history_record("missing-final-attempt");
        record.attempts.push(crate::storage::AttemptHistoryRecord {
            attempt_id: crate::domain::UpstreamAttemptId::new(),
            attempt_index: 0,
            attempt_role: crate::storage::AttemptRole::Ordinary,
            route_id,
            route_name: "Earlier route".to_owned(),
            started_at_ms: 1,
            finished_at_ms: 2,
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
        });

        assert!(recorder.try_record(record));
        assert!(
            recorder.try_record_fallback_stop(crate::storage::FallbackStopRecord {
                request_id: "missing-final-attempt".to_owned(),
                attempt_index: 1,
                reason: crate::storage::FallbackStopReason::AllParticipantsAttempted,
                target_route_id: None,
                target_route_name: None,
            })
        );
        recorder.shutdown().await;

        let detail = database
            .usage_request_detail("missing-final-attempt".to_owned())
            .await
            .expect("usage detail");
        assert_eq!(detail.attempts.len(), 1);
        assert_eq!(detail.attempts[0].routing_decision, None);
    }

    #[tokio::test]
    async fn request_history_preserves_attempt_indexes_above_u16() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let database =
            DatabaseExecutor::open(directory.path().join("router.sqlite3")).expect("database");
        let diagnostics = Arc::new(DiagnosticCapture::default());
        let changes = Arc::new(ChangeCounter::default());
        let recorder = AsyncHistoryRecorder::new(database.clone(), diagnostics, changes.clone());
        let wide_index = u32::from(u16::MAX) + 1;
        let route_id = RouteId::new();
        let mut record = history_record("wide-attempt-index");
        record.attempts.push(crate::storage::AttemptHistoryRecord {
            attempt_id: crate::domain::UpstreamAttemptId::new(),
            attempt_index: wide_index,
            attempt_role: crate::storage::AttemptRole::Ordinary,
            route_id,
            route_name: "Route".to_owned(),
            started_at_ms: 1,
            finished_at_ms: 2,
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
        });

        assert!(recorder.try_record(record));
        for _ in 0..100 {
            if changes.0.load(Ordering::SeqCst) == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        recorder.shutdown().await;

        assert_eq!(recorder.failure_snapshot().write_failures, 0);
        let detail = database
            .usage_request_detail("wide-attempt-index".to_owned())
            .await
            .expect("usage detail");
        assert_eq!(detail.attempts[0].attempt_index, wide_index);
        let dto = crate::app_api::UsageRequestDetailDto::from(detail);
        assert_eq!(dto.attempts[0].attempt_index, wide_index);
    }

    #[tokio::test]
    async fn request_history_write_failure_is_reported_after_enqueue() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let database =
            DatabaseExecutor::open(directory.path().join("router.sqlite3")).expect("database");
        let diagnostics = Arc::new(DiagnosticCapture::default());
        let changes = Arc::new(ChangeCounter::default());
        let recorder = AsyncHistoryRecorder::new(database, diagnostics.clone(), changes.clone());

        let attempt_id = crate::domain::UpstreamAttemptId::new();
        let mut first = history_record("duplicate");
        first.attempts.push(crate::storage::AttemptHistoryRecord {
            attempt_id: attempt_id.clone(),
            attempt_index: 0,
            attempt_role: crate::storage::AttemptRole::Ordinary,
            route_id: RouteId::new(),
            route_name: "Route".to_owned(),
            started_at_ms: 1,
            finished_at_ms: 2,
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
        });
        let mut duplicate_attempt = history_record("duplicate");
        duplicate_attempt
            .attempts
            .push(crate::storage::AttemptHistoryRecord {
                attempt_id,
                attempt_index: 1,
                attempt_role: crate::storage::AttemptRole::Ordinary,
                route_id: RouteId::new(),
                route_name: "Route".to_owned(),
                started_at_ms: 2,
                finished_at_ms: 3,
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
            });
        assert!(recorder.try_record(first));
        assert!(recorder.try_record(duplicate_attempt));
        for _ in 0..100 {
            if recorder.failure_snapshot().write_failures == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }

        assert_eq!(recorder.failure_snapshot().write_failures, 1);
        assert!(
            diagnostics
                .0
                .lock()
                .expect("diagnostic mutex")
                .iter()
                .any(|event| event.code == RuntimeDiagnosticCode::MetadataWriteFailed)
        );
        assert_eq!(changes.0.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn inference_status_reconstructs_and_resets_with_committed_history_clear() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let database =
            DatabaseExecutor::open(directory.path().join("router.sqlite3")).expect("database");
        let route_id = RouteId::new();
        let mut record = history_record("reconstruct");
        record.completion_state = CompletionState::Completed;
        record.final_route_id = Some(route_id.clone());
        record.http_status = Some(200);
        record.attempts = vec![crate::storage::AttemptHistoryRecord {
            attempt_id: crate::domain::UpstreamAttemptId::new(),
            attempt_index: 0,
            attempt_role: crate::storage::AttemptRole::Ordinary,
            route_id: route_id.clone(),
            route_name: "Route".to_owned(),
            started_at_ms: 1,
            finished_at_ms: 2,
            http_status: Some(200),
            error_category: None,
            delivery_state: crate::domain::DeliveryState::Completed,
            actual_model: None,
            forwarded_service_tier: None,
            actual_service_tier: None,
            input_tokens: None,
            output_tokens: None,
            total_tokens: None,
            cached_input_tokens: None,
            cache_write_input_tokens: None,
        }];
        database
            .record_request_history(record)
            .await
            .expect("history record");
        let service =
            InferenceStatusService::with_expiry(Arc::new(ChangeCounter::default()), 1_000);

        service
            .reconstruct_from_database(&database, std::slice::from_ref(&route_id), 10)
            .await
            .expect("reconstruct");
        assert_eq!(
            service.status(&route_id, 10).kind,
            InferenceStatusKind::RecentSuccess
        );
        service
            .clear_history_and_reset(&database)
            .await
            .expect("clear history");

        assert_eq!(
            service.status(&route_id, 10).kind,
            InferenceStatusKind::Unverified
        );
        assert_eq!(
            database
                .history_summary()
                .await
                .expect("history summary")
                .request_count,
            0
        );
    }
}
