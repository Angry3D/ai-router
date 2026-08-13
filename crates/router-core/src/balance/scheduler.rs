use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use futures_util::future::{BoxFuture, FutureExt, Shared};
use serde::{Deserialize, Serialize};
use tokio::sync::{Notify, Semaphore};
use tokio_util::sync::CancellationToken;
use ts_rs::TS;
use uuid::Uuid;

use super::{
    BalanceError, BalanceErrorCategory, BalanceErrorStage, BalanceExecutor, BalanceQueryConfig,
    BalanceResult,
};
use crate::domain::{ApiKey, BalanceQueryPolicy, BaseUrl, RouteId};

const LAST_GOOD_MS: i64 = 10 * 60 * 1_000;
const BALANCE_CONCURRENCY: usize = 4;

pub struct BalanceRouteConfig {
    pub route_id: RouteId,
    pub base_url: BaseUrl,
    pub api_key: ApiKey,
    pub query: BalanceQueryConfig,
    pub query_revision: u64,
}

#[async_trait]
pub trait BalanceRouteSource: Send + Sync {
    async fn load_enabled_route(
        &self,
        route_id: &RouteId,
    ) -> Result<Option<BalanceRouteConfig>, BalanceError>;

    async fn is_current(&self, route_id: &RouteId, query_revision: u64) -> bool;

    async fn eligible_route_ids(&self) -> Result<Vec<RouteId>, BalanceError>;

    async fn active_route_id(&self) -> Option<RouteId>;
}

#[async_trait]
pub trait BalanceQueryEngine: Send + Sync {
    async fn query(
        &self,
        query: &BalanceQueryConfig,
        api_key: &ApiKey,
        base_url: &BaseUrl,
    ) -> Result<BalanceResult, BalanceError>;
}

#[async_trait]
impl BalanceQueryEngine for BalanceExecutor {
    async fn query(
        &self,
        query: &BalanceQueryConfig,
        api_key: &ApiKey,
        base_url: &BaseUrl,
    ) -> Result<BalanceResult, BalanceError> {
        self.query(query, api_key, base_url).await
    }
}

pub trait BalanceStateChangeSink: Send + Sync {
    fn balance_changed(&self);
}

pub trait BalanceClock: Send + Sync {
    fn now_millis(&self) -> i64;
}

pub struct SystemBalanceClock;

impl BalanceClock for SystemBalanceClock {
    fn now_millis(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .try_into()
            .unwrap_or(i64::MAX)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum BalanceTrigger {
    Startup,
    RouteChanged,
    Automatic,
    Explicit,
}

impl BalanceTrigger {
    const fn bypasses_debounce(self) -> bool {
        matches!(self, Self::Explicit)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum BalanceDisplayStatus {
    Unavailable,
    Refreshing,
    Fresh,
    Stale,
    LastGood,
    Failed,
}

#[derive(Clone, Deserialize, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct BalanceDisplaySnapshot {
    pub route_id: RouteId,
    pub value: Option<BalanceResult>,
    pub status: BalanceDisplayStatus,
    #[ts(type = "number | null")]
    pub last_success_at_ms: Option<i64>,
    #[ts(type = "number | null")]
    pub last_completion_at_ms: Option<i64>,
    #[ts(type = "number | null")]
    pub next_due_at_ms: Option<i64>,
    pub error: Option<BalanceError>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum BalanceBatchPhase {
    Running,
    Completed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct BalanceRefreshBatchState {
    pub batch_id: String,
    pub eligible_count: u32,
    pub completed_count: u32,
    pub success_count: u32,
    pub failure_count: u32,
    pub phase: BalanceBatchPhase,
}

pub type BalanceQueryResult = Arc<Result<BalanceResult, BalanceError>>;
type SharedQuery = Shared<BoxFuture<'static, BalanceQueryResult>>;

#[derive(Default)]
struct RouteBalanceState {
    in_flight: Option<(String, SharedQuery)>,
    cached_outcome: Option<BalanceQueryResult>,
    last_success: Option<BalanceResult>,
    last_success_at_ms: Option<i64>,
    last_completion_at_ms: Option<i64>,
    last_good_until_ms: Option<i64>,
    stale_at_ms: Option<i64>,
    next_due_at_ms: Option<i64>,
    last_error: Option<BalanceError>,
}

pub struct BalanceCoordinator {
    source: Arc<dyn BalanceRouteSource>,
    engine: Arc<dyn BalanceQueryEngine>,
    changes: Arc<dyn BalanceStateChangeSink>,
    clock: Arc<dyn BalanceClock>,
    policy: RwLock<BalanceQueryPolicy>,
    semaphore: Arc<Semaphore>,
    routes: Mutex<HashMap<RouteId, RouteBalanceState>>,
    batch: Mutex<Option<BalanceRefreshBatchState>>,
    batch_start: tokio::sync::Mutex<()>,
    scheduler_notify: Notify,
    scheduler_started: AtomicBool,
    cancellation: CancellationToken,
    scheduler_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl BalanceCoordinator {
    #[must_use]
    pub fn new(
        source: Arc<dyn BalanceRouteSource>,
        engine: Arc<dyn BalanceQueryEngine>,
        changes: Arc<dyn BalanceStateChangeSink>,
        policy: BalanceQueryPolicy,
    ) -> Arc<Self> {
        Self::with_clock(
            source,
            engine,
            changes,
            Arc::new(SystemBalanceClock),
            policy,
        )
    }

    #[must_use]
    pub fn with_clock(
        source: Arc<dyn BalanceRouteSource>,
        engine: Arc<dyn BalanceQueryEngine>,
        changes: Arc<dyn BalanceStateChangeSink>,
        clock: Arc<dyn BalanceClock>,
        policy: BalanceQueryPolicy,
    ) -> Arc<Self> {
        Arc::new(Self {
            source,
            engine,
            changes,
            clock,
            policy: RwLock::new(policy),
            semaphore: Arc::new(Semaphore::new(BALANCE_CONCURRENCY)),
            routes: Mutex::new(HashMap::new()),
            batch: Mutex::new(None),
            batch_start: tokio::sync::Mutex::new(()),
            scheduler_notify: Notify::new(),
            scheduler_started: AtomicBool::new(false),
            cancellation: CancellationToken::new(),
            scheduler_task: Mutex::new(None),
        })
    }

    pub async fn query_route(
        self: &Arc<Self>,
        route_id: RouteId,
        trigger: BalanceTrigger,
    ) -> BalanceQueryResult {
        if self.cancellation.is_cancelled() {
            return Arc::new(Err(invalid_route_error()));
        }
        let now_ms = self.clock.now_millis();
        let future = {
            let policy = self
                .policy
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut routes = self
                .routes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let state = routes.entry(route_id.clone()).or_default();
            if let Some((_, future)) = &state.in_flight {
                future.clone()
            } else {
                if !trigger.bypasses_debounce()
                    && state.last_completion_at_ms.is_some_and(|completed| {
                        now_ms.saturating_sub(completed) < policy.menu_debounce_millis()
                    })
                    && let Some(cached) = &state.cached_outcome
                {
                    return Arc::clone(cached);
                }
                let execution_id = Uuid::new_v4().to_string();
                let future_execution_id = execution_id.clone();
                let coordinator = Arc::clone(self);
                let execution_route = route_id.clone();
                let future = async move {
                    coordinator
                        .execute_route(execution_route, future_execution_id)
                        .await
                }
                .boxed()
                .shared();
                state.in_flight = Some((execution_id, future.clone()));
                future
            }
        };
        self.changes.balance_changed();
        future.await
    }

    async fn execute_route(
        self: Arc<Self>,
        route_id: RouteId,
        execution_id: String,
    ) -> BalanceQueryResult {
        let permit = tokio::select! {
            permit = Arc::clone(&self.semaphore).acquire_owned() => {
                let Ok(permit) = permit else {
                    return self.finish_route(
                        &route_id,
                        &execution_id,
                        Err(invalid_route_error()),
                        false,
                    );
                };
                permit
            }
            () = self.cancellation.cancelled() => {
                return self.finish_route(
                    &route_id,
                    &execution_id,
                    Err(invalid_route_error()),
                    false,
                );
            }
        };
        let Ok(Some(config)) = self.source.load_enabled_route(&route_id).await else {
            drop(permit);
            return self.finish_route(&route_id, &execution_id, Err(invalid_route_error()), false);
        };
        let revision = config.query_revision;
        let outcome = tokio::select! {
            outcome = self.engine.query(
                &config.query,
                &config.api_key,
                &config.base_url,
            ) => outcome,
            () = self.cancellation.cancelled() => Err(invalid_route_error()),
        };
        drop(permit);
        let current =
            !self.cancellation.is_cancelled() && self.source.is_current(&route_id, revision).await;
        self.finish_route(&route_id, &execution_id, outcome, current)
    }

    fn finish_route(
        &self,
        route_id: &RouteId,
        execution_id: &str,
        outcome: Result<BalanceResult, BalanceError>,
        current: bool,
    ) -> BalanceQueryResult {
        let now_ms = self.clock.now_millis();
        let policy = self
            .policy
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let automatic_refresh_millis = policy.automatic_refresh_millis();
        let mut routes = self
            .routes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let owns_execution = routes
            .get(route_id)
            .and_then(|state| state.in_flight.as_ref())
            .is_some_and(|(id, _)| id == execution_id);
        if !owns_execution {
            return Arc::new(Err(invalid_route_error()));
        }
        if !current {
            routes.remove(route_id);
            drop(routes);
            self.changes.balance_changed();
            self.scheduler_notify.notify_one();
            return Arc::new(Err(invalid_route_error()));
        }
        let state = routes
            .get_mut(route_id)
            .expect("owned balance execution must retain route state");
        if state
            .in_flight
            .as_ref()
            .is_some_and(|(id, _)| id == execution_id)
        {
            state.in_flight = None;
        }
        state.last_completion_at_ms = Some(now_ms);
        state.next_due_at_ms = Some(now_ms.saturating_add(automatic_refresh_millis));
        let published = Arc::new(outcome);
        state.cached_outcome = Some(Arc::clone(&published));
        match published.as_ref() {
            Ok(result) if result.is_valid => {
                state.last_success = Some(result.clone());
                state.last_success_at_ms = Some(now_ms);
                state.last_good_until_ms = None;
                state.stale_at_ms = Some(now_ms.saturating_add(automatic_refresh_millis));
                state.last_error = None;
            }
            Ok(_) => {
                state.last_success = None;
                state.last_good_until_ms = None;
                state.stale_at_ms = None;
                state.last_error = Some(invalid_result_error());
            }
            Err(error) if error.transient => {
                state.stale_at_ms = None;
                state.last_good_until_ms = state
                    .last_success
                    .as_ref()
                    .map(|_| now_ms.saturating_add(LAST_GOOD_MS));
                state.last_error = Some(error.clone());
            }
            Err(error) => {
                state.last_success = None;
                state.last_good_until_ms = None;
                state.stale_at_ms = None;
                state.last_error = Some(error.clone());
            }
        }
        drop(routes);
        self.changes.balance_changed();
        self.scheduler_notify.notify_one();
        published
    }

    #[must_use]
    pub fn policy(&self) -> BalanceQueryPolicy {
        *self
            .policy
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Publishes a persisted timing policy and rebases retained deadlines.
    ///
    /// Existing in-flight work continues. The scheduler is notified after all
    /// retained state observes the new interval.
    pub fn update_policy(&self, policy: BalanceQueryPolicy) -> bool {
        let mut current = self
            .policy
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if *current == policy {
            return false;
        }
        *current = policy;
        let interval = policy.automatic_refresh_millis();
        let mut routes = self
            .routes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for state in routes.values_mut() {
            let Some(completed_at) = state.last_completion_at_ms else {
                continue;
            };
            let deadline = completed_at.saturating_add(interval);
            state.next_due_at_ms = Some(deadline);
            if state.last_error.is_none() && state.last_success.is_some() {
                state.stale_at_ms = Some(deadline);
            }
        }
        drop(routes);
        drop(current);
        self.changes.balance_changed();
        self.scheduler_notify.notify_one();
        true
    }

    #[must_use]
    pub fn route_snapshot(&self, route_id: &RouteId) -> BalanceDisplaySnapshot {
        let now_ms = self.clock.now_millis();
        let routes = self
            .routes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(state) = routes.get(route_id) else {
            return BalanceDisplaySnapshot {
                route_id: route_id.clone(),
                value: None,
                status: BalanceDisplayStatus::Unavailable,
                last_success_at_ms: None,
                last_completion_at_ms: None,
                next_due_at_ms: None,
                error: None,
            };
        };
        let refreshing = state.in_flight.is_some();
        let last_good = state
            .last_good_until_ms
            .is_some_and(|deadline| now_ms < deadline);
        let is_stale = state.stale_at_ms.is_some_and(|deadline| now_ms >= deadline);
        let value = if state.last_error.is_none() || last_good || is_stale {
            state.last_success.clone()
        } else {
            None
        };
        let status = if refreshing {
            BalanceDisplayStatus::Refreshing
        } else if last_good {
            BalanceDisplayStatus::LastGood
        } else if state.last_error.is_some() {
            BalanceDisplayStatus::Failed
        } else if is_stale {
            BalanceDisplayStatus::Stale
        } else if value.is_some() {
            BalanceDisplayStatus::Fresh
        } else {
            BalanceDisplayStatus::Unavailable
        };
        BalanceDisplaySnapshot {
            route_id: route_id.clone(),
            value,
            status,
            last_success_at_ms: state.last_success_at_ms,
            last_completion_at_ms: state.last_completion_at_ms,
            next_due_at_ms: state.next_due_at_ms,
            error: state.last_error.clone(),
        }
    }

    pub fn invalidate_route(&self, route_id: &RouteId) {
        self.routes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(route_id);
        self.changes.balance_changed();
        self.scheduler_notify.notify_one();
    }

    pub fn remove_route(&self, route_id: &RouteId) {
        self.invalidate_route(route_id);
    }

    /// Starts one fixed-target global refresh or returns the active batch.
    ///
    /// # Errors
    ///
    /// Returns a safe source error when eligible routes cannot be loaded.
    pub async fn refresh_all(self: &Arc<Self>) -> Result<BalanceRefreshBatchState, BalanceError> {
        let _start_guard = self.batch_start.lock().await;
        if let Some(batch) = self
            .batch
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .filter(|batch| batch.phase == BalanceBatchPhase::Running)
            .cloned()
        {
            return Ok(batch);
        }
        let route_ids = self.source.eligible_route_ids().await?;
        let eligible_count = route_ids.len().try_into().unwrap_or(u32::MAX);
        let batch = BalanceRefreshBatchState {
            batch_id: Uuid::new_v4().to_string(),
            eligible_count,
            completed_count: 0,
            success_count: 0,
            failure_count: 0,
            phase: if route_ids.is_empty() {
                BalanceBatchPhase::Completed
            } else {
                BalanceBatchPhase::Running
            },
        };
        *self
            .batch
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(batch.clone());
        self.changes.balance_changed();
        for route_id in route_ids {
            let coordinator = Arc::clone(self);
            let batch_id = batch.batch_id.clone();
            tokio::spawn(async move {
                let outcome = coordinator
                    .query_route(route_id, BalanceTrigger::Explicit)
                    .await;
                coordinator.finish_batch_target(&batch_id, query_succeeded(&outcome));
            });
        }
        Ok(batch)
    }

    fn finish_batch_target(&self, batch_id: &str, succeeded: bool) {
        let mut batches = self
            .batch
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(batch) = batches.as_mut().filter(|batch| batch.batch_id == batch_id) else {
            return;
        };
        batch.completed_count = batch.completed_count.saturating_add(1);
        if succeeded {
            batch.success_count = batch.success_count.saturating_add(1);
        } else {
            batch.failure_count = batch.failure_count.saturating_add(1);
        }
        if batch.completed_count >= batch.eligible_count {
            batch.phase = BalanceBatchPhase::Completed;
        }
        drop(batches);
        self.changes.balance_changed();
    }

    #[must_use]
    pub fn batch_snapshot(&self) -> Option<BalanceRefreshBatchState> {
        self.batch
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub async fn trigger_startup(self: &Arc<Self>) -> Option<BalanceQueryResult> {
        let route_id = self.source.active_route_id().await?;
        Some(self.query_route(route_id, BalanceTrigger::Startup).await)
    }

    pub async fn trigger_menu_open(self: &Arc<Self>) -> Option<BalanceQueryResult> {
        let route_id = self.source.active_route_id().await?;
        Some(self.query_route(route_id, BalanceTrigger::Automatic).await)
    }

    pub async fn trigger_route_change(self: &Arc<Self>, route_id: RouteId) -> BalanceQueryResult {
        self.scheduler_notify.notify_one();
        self.query_route(route_id, BalanceTrigger::RouteChanged)
            .await
    }

    pub async fn run_due_once(self: &Arc<Self>) -> Option<BalanceQueryResult> {
        let route_id = self.source.active_route_id().await?;
        let due = self
            .routes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&route_id)
            .and_then(|state| state.next_due_at_ms)
            .is_some_and(|deadline| self.clock.now_millis() >= deadline);
        if due {
            Some(self.query_route(route_id, BalanceTrigger::Automatic).await)
        } else {
            None
        }
    }

    pub fn start_scheduler(self: &Arc<Self>) {
        if self.scheduler_started.swap(true, Ordering::AcqRel) {
            return;
        }
        let coordinator = Arc::clone(self);
        let task = tokio::spawn(async move {
            loop {
                if coordinator.cancellation.is_cancelled() {
                    break;
                }
                let next_due = coordinator.active_next_due().await;
                let Some(next_due) = next_due else {
                    tokio::select! {
                        () = coordinator.scheduler_notify.notified() => {}
                        () = coordinator.cancellation.cancelled() => break,
                    }
                    continue;
                };
                let wait = next_due
                    .saturating_sub(coordinator.clock.now_millis())
                    .max(0)
                    .cast_unsigned();
                tokio::select! {
                    () = tokio::time::sleep(Duration::from_millis(wait)) => {
                        let _ = coordinator.run_due_once().await;
                    }
                    () = coordinator.scheduler_notify.notified() => {}
                    () = coordinator.cancellation.cancelled() => break,
                }
            }
        });
        *self
            .scheduler_task
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(task);
    }

    pub async fn shutdown(&self) {
        self.cancellation.cancel();
        self.semaphore.close();
        self.scheduler_notify.notify_waiters();
        let task = self
            .scheduler_task
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(task) = task {
            let _ = task.await;
        }
    }

    async fn active_next_due(&self) -> Option<i64> {
        let route_id = self.source.active_route_id().await?;
        self.routes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&route_id)
            .and_then(|state| state.next_due_at_ms)
    }
}

fn query_succeeded(outcome: &BalanceQueryResult) -> bool {
    outcome
        .as_ref()
        .as_ref()
        .is_ok_and(|result| result.is_valid)
}

fn invalid_route_error() -> BalanceError {
    BalanceError {
        stage: BalanceErrorStage::RequestValidation,
        category: BalanceErrorCategory::InvalidRequest,
        transient: false,
    }
}

fn invalid_result_error() -> BalanceError {
    BalanceError {
        stage: BalanceErrorStage::ResultValidation,
        category: BalanceErrorCategory::InvalidResult,
        transient: false,
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::VecDeque,
        sync::atomic::{AtomicI64, AtomicUsize},
    };

    use super::*;

    struct StoredRoute {
        base_url: String,
        api_key: String,
        query: BalanceQueryConfig,
        query_revision: u64,
        enabled: bool,
    }

    #[derive(Default)]
    struct MockSource {
        routes: Mutex<HashMap<RouteId, StoredRoute>>,
        active: Mutex<Option<RouteId>>,
    }

    impl MockSource {
        fn insert(&self, route_id: RouteId, revision: u64) {
            self.routes.lock().expect("source routes mutex").insert(
                route_id,
                StoredRoute {
                    base_url: "https://example.test/v1".to_owned(),
                    api_key: "exact-key".to_owned(),
                    query: BalanceQueryConfig {
                        mode: crate::balance::BalanceQueryMode::CustomJs,
                        custom_source: "script".to_owned(),
                    },
                    query_revision: revision,
                    enabled: true,
                },
            );
        }

        fn set_active(&self, route_id: Option<RouteId>) {
            *self.active.lock().expect("active route mutex") = route_id;
        }

        fn set_enabled(&self, route_id: &RouteId, enabled: bool) {
            if let Some(route) = self
                .routes
                .lock()
                .expect("source routes mutex")
                .get_mut(route_id)
            {
                route.enabled = enabled;
            }
        }

        fn set_revision(&self, route_id: &RouteId, revision: u64) {
            if let Some(route) = self
                .routes
                .lock()
                .expect("source routes mutex")
                .get_mut(route_id)
            {
                route.query_revision = revision;
            }
        }

        fn remove(&self, route_id: &RouteId) {
            self.routes
                .lock()
                .expect("source routes mutex")
                .remove(route_id);
        }
    }

    #[async_trait]
    impl BalanceRouteSource for MockSource {
        async fn load_enabled_route(
            &self,
            route_id: &RouteId,
        ) -> Result<Option<BalanceRouteConfig>, BalanceError> {
            Ok(self
                .routes
                .lock()
                .expect("source routes mutex")
                .get(route_id)
                .filter(|route| route.enabled)
                .map(|route| BalanceRouteConfig {
                    route_id: route_id.clone(),
                    base_url: BaseUrl::parse(&route.base_url).expect("stored base URL"),
                    api_key: ApiKey::parse(&route.api_key).expect("stored API key"),
                    query: route.query.clone(),
                    query_revision: route.query_revision,
                }))
        }

        async fn is_current(&self, route_id: &RouteId, query_revision: u64) -> bool {
            self.routes
                .lock()
                .expect("source routes mutex")
                .get(route_id)
                .is_some_and(|route| route.enabled && route.query_revision == query_revision)
        }

        async fn eligible_route_ids(&self) -> Result<Vec<RouteId>, BalanceError> {
            Ok(self
                .routes
                .lock()
                .expect("source routes mutex")
                .iter()
                .filter(|(_, route)| route.enabled)
                .map(|(route_id, _)| route_id.clone())
                .collect())
        }

        async fn active_route_id(&self) -> Option<RouteId> {
            self.active.lock().expect("active route mutex").clone()
        }
    }

    struct MockEngine {
        calls: AtomicUsize,
        active: AtomicUsize,
        maximum_active: AtomicUsize,
        delay: Duration,
        outcomes: Mutex<VecDeque<Result<BalanceResult, BalanceError>>>,
        keys: Mutex<Vec<Vec<u8>>>,
    }

    struct ReplacementEngine {
        calls: AtomicUsize,
        first_started: Notify,
        release_first: Notify,
    }

    impl ReplacementEngine {
        fn new() -> Self {
            Self {
                calls: AtomicUsize::new(0),
                first_started: Notify::new(),
                release_first: Notify::new(),
            }
        }
    }

    #[async_trait]
    impl BalanceQueryEngine for ReplacementEngine {
        async fn query(
            &self,
            _query: &BalanceQueryConfig,
            _api_key: &ApiKey,
            _base_url: &BaseUrl,
        ) -> Result<BalanceResult, BalanceError> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            if call == 0 {
                self.first_started.notify_one();
                self.release_first.notified().await;
                Ok(success_result(1.0))
            } else {
                Ok(success_result(2.0))
            }
        }
    }

    impl MockEngine {
        fn new(delay: Duration, outcomes: Vec<Result<BalanceResult, BalanceError>>) -> Self {
            Self {
                calls: AtomicUsize::new(0),
                active: AtomicUsize::new(0),
                maximum_active: AtomicUsize::new(0),
                delay,
                outcomes: Mutex::new(outcomes.into()),
                keys: Mutex::new(Vec::new()),
            }
        }

        fn update_maximum(&self, active: usize) {
            let mut observed = self.maximum_active.load(Ordering::SeqCst);
            while active > observed {
                match self.maximum_active.compare_exchange(
                    observed,
                    active,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                ) {
                    Ok(_) => break,
                    Err(current) => observed = current,
                }
            }
        }
    }

    #[async_trait]
    impl BalanceQueryEngine for MockEngine {
        async fn query(
            &self,
            _query: &BalanceQueryConfig,
            api_key: &ApiKey,
            _base_url: &BaseUrl,
        ) -> Result<BalanceResult, BalanceError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.keys
                .lock()
                .expect("engine keys mutex")
                .push(api_key.expose().to_vec());
            let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.update_maximum(active);
            tokio::time::sleep(self.delay).await;
            self.active.fetch_sub(1, Ordering::SeqCst);
            self.outcomes
                .lock()
                .expect("engine outcomes mutex")
                .pop_front()
                .unwrap_or_else(|| Ok(success_result(1.0)))
        }
    }

    #[derive(Default)]
    struct ChangeCounter(AtomicUsize);

    impl BalanceStateChangeSink for ChangeCounter {
        fn balance_changed(&self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[derive(Default)]
    struct ManualClock(AtomicI64);

    impl ManualClock {
        fn set(&self, value: i64) {
            self.0.store(value, Ordering::SeqCst);
        }
    }

    impl BalanceClock for ManualClock {
        fn now_millis(&self) -> i64 {
            self.0.load(Ordering::SeqCst)
        }
    }

    fn success_result(remaining: f64) -> BalanceResult {
        BalanceResult {
            is_valid: true,
            remaining: Some(remaining),
            used: None,
            total: None,
            unit: Some("USD".to_owned()),
            plan_name: None,
            invalid_message: None,
            extra: None,
        }
    }

    fn transient_error() -> BalanceError {
        BalanceError {
            stage: BalanceErrorStage::Http,
            category: BalanceErrorCategory::Network,
            transient: true,
        }
    }

    fn deterministic_error() -> BalanceError {
        BalanceError {
            stage: BalanceErrorStage::ResultValidation,
            category: BalanceErrorCategory::InvalidResult,
            transient: false,
        }
    }

    fn coordinator(
        source: Arc<MockSource>,
        engine: Arc<MockEngine>,
        clock: Arc<ManualClock>,
    ) -> Arc<BalanceCoordinator> {
        BalanceCoordinator::with_clock(
            source,
            engine,
            Arc::new(ChangeCounter::default()),
            clock,
            BalanceQueryPolicy::default(),
        )
    }

    fn automatic_refresh_millis() -> i64 {
        BalanceQueryPolicy::default().automatic_refresh_millis()
    }

    fn menu_debounce_millis() -> i64 {
        BalanceQueryPolicy::default().menu_debounce_millis()
    }

    #[tokio::test]
    async fn balance_scheduler_singleflight_debounce_and_explicit_refresh_share_one_path() {
        let source = Arc::new(MockSource::default());
        let route_id = RouteId::new();
        source.insert(route_id.clone(), 1);
        let engine = Arc::new(MockEngine::new(Duration::from_millis(20), Vec::new()));
        let clock = Arc::new(ManualClock::default());
        clock.set(1_000);
        let coordinator = coordinator(source, engine.clone(), clock.clone());

        let (first, second) = tokio::join!(
            coordinator.query_route(route_id.clone(), BalanceTrigger::Explicit),
            coordinator.query_route(route_id.clone(), BalanceTrigger::Explicit),
        );
        assert!(first.as_ref().is_ok());
        assert!(second.as_ref().is_ok());
        assert_eq!(engine.calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            engine.keys.lock().expect("engine keys mutex").as_slice(),
            &[b"exact-key".to_vec()]
        );

        let cached = coordinator
            .query_route(route_id.clone(), BalanceTrigger::Automatic)
            .await;
        assert!(cached.as_ref().is_ok());
        assert_eq!(engine.calls.load(Ordering::SeqCst), 1);
        let explicit = coordinator
            .query_route(route_id.clone(), BalanceTrigger::Explicit)
            .await;
        assert!(explicit.as_ref().is_ok());
        assert_eq!(engine.calls.load(Ordering::SeqCst), 2);
        let snapshot = coordinator.route_snapshot(&route_id);
        assert_eq!(snapshot.status, BalanceDisplayStatus::Fresh);
        assert_eq!(
            snapshot.next_due_at_ms,
            Some(1_000 + automatic_refresh_millis())
        );

        clock.set(1_000 + automatic_refresh_millis());
        assert_eq!(
            coordinator.route_snapshot(&route_id).status,
            BalanceDisplayStatus::Stale
        );
    }

    #[tokio::test]
    async fn balance_scheduler_last_good_expires_and_deterministic_failure_clears_value() {
        let source = Arc::new(MockSource::default());
        let route_id = RouteId::new();
        source.insert(route_id.clone(), 1);
        let engine = Arc::new(MockEngine::new(
            Duration::ZERO,
            vec![
                Ok(success_result(9.0)),
                Err(transient_error()),
                Err(deterministic_error()),
            ],
        ));
        let clock = Arc::new(ManualClock::default());
        clock.set(1_000);
        let coordinator = coordinator(source, engine, clock.clone());

        let _ = coordinator
            .query_route(route_id.clone(), BalanceTrigger::Explicit)
            .await;
        clock.set(2_000);
        let _ = coordinator
            .query_route(route_id.clone(), BalanceTrigger::Explicit)
            .await;
        let last_good = coordinator.route_snapshot(&route_id);
        assert_eq!(last_good.status, BalanceDisplayStatus::LastGood);
        assert_eq!(
            last_good.value.as_ref().and_then(|value| value.remaining),
            Some(9.0)
        );

        clock.set(2_000 + LAST_GOOD_MS);
        let expired = coordinator.route_snapshot(&route_id);
        assert_eq!(expired.status, BalanceDisplayStatus::Failed);
        assert!(expired.value.is_none());
        let _ = coordinator
            .query_route(route_id.clone(), BalanceTrigger::Explicit)
            .await;
        let deterministic = coordinator.route_snapshot(&route_id);
        assert_eq!(deterministic.status, BalanceDisplayStatus::Failed);
        assert!(deterministic.value.is_none());
    }

    #[tokio::test]
    async fn balance_scheduler_global_batch_freezes_targets_reuses_inflight_and_caps_four_chains() {
        let source = Arc::new(MockSource::default());
        let route_ids = (0..6).map(|_| RouteId::new()).collect::<Vec<_>>();
        for route_id in &route_ids {
            source.insert(route_id.clone(), 1);
        }
        let engine = Arc::new(MockEngine::new(Duration::from_millis(30), Vec::new()));
        let clock = Arc::new(ManualClock::default());
        let coordinator = coordinator(source.clone(), engine.clone(), clock);
        let existing_coordinator = coordinator.clone();
        let existing_route = route_ids[0].clone();
        let existing = tokio::spawn(async move {
            existing_coordinator
                .query_route(existing_route, BalanceTrigger::Explicit)
                .await
        });
        tokio::task::yield_now().await;

        let batch = coordinator.refresh_all().await.expect("global batch");
        assert_eq!(batch.eligible_count, 6);
        assert_eq!(batch.phase, BalanceBatchPhase::Running);
        let repeated = coordinator.refresh_all().await.expect("active batch");
        assert_eq!(repeated.batch_id, batch.batch_id);
        let added_late = RouteId::new();
        source.insert(added_late, 1);

        existing.await.expect("existing query task");
        for _ in 0..100 {
            if coordinator
                .batch_snapshot()
                .is_some_and(|batch| batch.phase == BalanceBatchPhase::Completed)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        let completed = coordinator.batch_snapshot().expect("completed batch");
        assert_eq!(completed.eligible_count, 6);
        assert_eq!(completed.completed_count, 6);
        assert_eq!(completed.success_count, 6);
        assert_eq!(completed.failure_count, 0);
        assert_eq!(engine.calls.load(Ordering::SeqCst), 6);
        assert_eq!(engine.maximum_active.load(Ordering::SeqCst), 4);
    }

    #[tokio::test]
    async fn balance_scheduler_late_script_edits_and_deletes_do_not_publish_stale_results() {
        for delete in [false, true] {
            let source = Arc::new(MockSource::default());
            let route_id = RouteId::new();
            source.insert(route_id.clone(), 1);
            let engine = Arc::new(MockEngine::new(Duration::from_millis(20), Vec::new()));
            let coordinator = coordinator(source.clone(), engine, Arc::new(ManualClock::default()));
            let query_coordinator = coordinator.clone();
            let query_route = route_id.clone();
            let task = tokio::spawn(async move {
                query_coordinator
                    .query_route(query_route, BalanceTrigger::Explicit)
                    .await
            });
            tokio::time::sleep(Duration::from_millis(2)).await;
            if delete {
                source.remove(&route_id);
                coordinator.remove_route(&route_id);
            } else {
                source.set_revision(&route_id, 2);
                coordinator.invalidate_route(&route_id);
            }

            let result = task.await.expect("query task");
            assert!(result.as_ref().is_err());
            let snapshot = coordinator.route_snapshot(&route_id);
            assert_eq!(snapshot.status, BalanceDisplayStatus::Unavailable);
            assert!(snapshot.value.is_none());
        }
    }

    #[tokio::test]
    async fn balance_scheduler_late_old_execution_cannot_clobber_replacement_result() {
        let source = Arc::new(MockSource::default());
        let route_id = RouteId::new();
        source.insert(route_id.clone(), 1);
        let engine = Arc::new(ReplacementEngine::new());
        let coordinator = BalanceCoordinator::with_clock(
            source.clone(),
            engine.clone(),
            Arc::new(ChangeCounter::default()),
            Arc::new(ManualClock::default()),
            BalanceQueryPolicy::default(),
        );

        let old_coordinator = coordinator.clone();
        let old_route = route_id.clone();
        let old_query = tokio::spawn(async move {
            old_coordinator
                .query_route(old_route, BalanceTrigger::Explicit)
                .await
        });
        engine.first_started.notified().await;
        source.set_revision(&route_id, 2);
        coordinator.invalidate_route(&route_id);

        let replacement = coordinator
            .query_route(route_id.clone(), BalanceTrigger::Explicit)
            .await;
        assert!(replacement.as_ref().is_ok());
        engine.release_first.notify_one();
        assert!(old_query.await.expect("old query task").as_ref().is_err());

        let snapshot = coordinator.route_snapshot(&route_id);
        assert_eq!(snapshot.status, BalanceDisplayStatus::Fresh);
        assert_eq!(
            snapshot.value.as_ref().and_then(|value| value.remaining),
            Some(2.0)
        );
    }

    #[tokio::test]
    async fn balance_scheduler_ignores_inactive_route_deadlines() {
        let source = Arc::new(MockSource::default());
        let inactive_route = RouteId::new();
        let active_route = RouteId::new();
        source.insert(inactive_route.clone(), 1);
        source.insert(active_route.clone(), 1);
        let engine = Arc::new(MockEngine::new(Duration::ZERO, Vec::new()));
        let clock = Arc::new(ManualClock::default());
        clock.set(100);
        let coordinator = coordinator(source.clone(), engine, clock.clone());

        let _ = coordinator
            .query_route(inactive_route, BalanceTrigger::Explicit)
            .await;
        clock.set(200);
        let _ = coordinator
            .query_route(active_route.clone(), BalanceTrigger::Explicit)
            .await;
        source.set_active(Some(active_route));

        assert_eq!(
            coordinator.active_next_due().await,
            Some(200 + automatic_refresh_millis())
        );
    }

    #[tokio::test]
    async fn balance_scheduler_startup_due_and_empty_batch_paths_are_deterministic() {
        let source = Arc::new(MockSource::default());
        let route_id = RouteId::new();
        source.insert(route_id.clone(), 1);
        source.set_active(Some(route_id.clone()));
        let engine = Arc::new(MockEngine::new(
            Duration::ZERO,
            vec![Ok(success_result(0.0)), Ok(success_result(300.0))],
        ));
        let clock = Arc::new(ManualClock::default());
        clock.set(100);
        let coordinator = coordinator(source.clone(), engine.clone(), clock.clone());

        assert!(coordinator.trigger_startup().await.is_some());
        assert_eq!(
            coordinator
                .route_snapshot(&route_id)
                .value
                .and_then(|value| value.remaining),
            Some(0.0)
        );
        clock.set(100 + automatic_refresh_millis() - 1);
        assert!(coordinator.run_due_once().await.is_none());
        clock.set(100 + automatic_refresh_millis());
        assert!(coordinator.run_due_once().await.is_some());
        assert_eq!(engine.calls.load(Ordering::SeqCst), 2);
        assert_eq!(
            coordinator
                .route_snapshot(&route_id)
                .value
                .and_then(|value| value.remaining),
            Some(300.0)
        );

        source.remove(&route_id);
        let empty = coordinator.refresh_all().await.expect("empty batch");
        assert_eq!(empty.eligible_count, 0);
        assert_eq!(empty.phase, BalanceBatchPhase::Completed);
    }

    #[tokio::test]
    async fn balance_scheduler_menu_open_reuses_recent_result_then_refreshes_after_debounce() {
        let source = Arc::new(MockSource::default());
        let route_id = RouteId::new();
        source.insert(route_id.clone(), 1);
        source.set_active(Some(route_id));
        let engine = Arc::new(MockEngine::new(Duration::ZERO, Vec::new()));
        let clock = Arc::new(ManualClock::default());
        clock.set(100);
        let coordinator = coordinator(source, engine.clone(), clock.clone());

        assert!(coordinator.trigger_menu_open().await.is_some());
        assert!(coordinator.trigger_menu_open().await.is_some());
        assert_eq!(engine.calls.load(Ordering::SeqCst), 1);

        clock.set(100 + menu_debounce_millis() - 1);
        assert!(coordinator.trigger_menu_open().await.is_some());
        assert_eq!(engine.calls.load(Ordering::SeqCst), 1);

        clock.set(100 + menu_debounce_millis());
        assert!(coordinator.trigger_menu_open().await.is_some());
        assert_eq!(engine.calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn balance_scheduler_uses_the_configured_menu_debounce() {
        let source = Arc::new(MockSource::default());
        let route_id = RouteId::new();
        source.insert(route_id.clone(), 1);
        source.set_active(Some(route_id));
        let engine = Arc::new(MockEngine::new(Duration::ZERO, Vec::new()));
        let clock = Arc::new(ManualClock::default());
        clock.set(100);
        let policy = BalanceQueryPolicy::parse(10, 30).expect("policy");
        let coordinator = BalanceCoordinator::with_clock(
            source,
            engine.clone(),
            Arc::new(ChangeCounter::default()),
            clock.clone(),
            policy,
        );

        assert!(coordinator.trigger_menu_open().await.is_some());
        clock.set(10_099);
        assert!(coordinator.trigger_menu_open().await.is_some());
        assert_eq!(engine.calls.load(Ordering::SeqCst), 1);

        clock.set(10_100);
        assert!(coordinator.trigger_menu_open().await.is_some());
        assert_eq!(engine.calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn balance_scheduler_rebases_policy_changes_and_runs_when_shortened_overdue() {
        let source = Arc::new(MockSource::default());
        let route_id = RouteId::new();
        source.insert(route_id.clone(), 1);
        source.set_active(Some(route_id.clone()));
        let engine = Arc::new(MockEngine::new(Duration::ZERO, Vec::new()));
        let clock = Arc::new(ManualClock::default());
        clock.set(100);
        let coordinator = coordinator(source, engine.clone(), clock.clone());

        let _ = coordinator
            .query_route(route_id.clone(), BalanceTrigger::Explicit)
            .await;
        let long_policy = BalanceQueryPolicy::parse(10, 60).expect("long policy");
        assert!(coordinator.update_policy(long_policy));
        assert_eq!(coordinator.policy(), long_policy);
        assert_eq!(
            coordinator.route_snapshot(&route_id).next_due_at_ms,
            Some(100 + long_policy.automatic_refresh_millis())
        );
        clock.set(100 + automatic_refresh_millis());
        assert_eq!(
            coordinator.route_snapshot(&route_id).status,
            BalanceDisplayStatus::Fresh
        );
        assert!(coordinator.run_due_once().await.is_none());

        coordinator.start_scheduler();
        let short_policy = BalanceQueryPolicy::parse(10, 5).expect("short policy");
        assert!(coordinator.update_policy(short_policy));
        let expected_due = clock
            .now_millis()
            .saturating_add(short_policy.automatic_refresh_millis());
        for _ in 0..100 {
            if coordinator.route_snapshot(&route_id).next_due_at_ms == Some(expected_due) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        assert_eq!(engine.calls.load(Ordering::SeqCst), 2);
        assert_eq!(
            coordinator.route_snapshot(&route_id).next_due_at_ms,
            Some(expected_due)
        );
        assert!(!coordinator.update_policy(short_policy));
        coordinator.shutdown().await;
    }

    #[tokio::test]
    async fn balance_scheduler_menu_open_skips_missing_active_and_disabled_route_work() {
        let source = Arc::new(MockSource::default());
        let route_id = RouteId::new();
        source.insert(route_id.clone(), 1);
        let engine = Arc::new(MockEngine::new(Duration::ZERO, Vec::new()));
        let coordinator = coordinator(
            source.clone(),
            engine.clone(),
            Arc::new(ManualClock::default()),
        );

        assert!(coordinator.trigger_menu_open().await.is_none());
        assert_eq!(engine.calls.load(Ordering::SeqCst), 0);

        source.set_enabled(&route_id, false);
        source.set_active(Some(route_id));
        assert!(coordinator.trigger_menu_open().await.is_some());
        assert_eq!(engine.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn balance_scheduler_shutdown_cancels_inflight_and_rejects_new_queries() {
        let source = Arc::new(MockSource::default());
        let route_id = RouteId::new();
        source.insert(route_id.clone(), 1);
        let engine = Arc::new(MockEngine::new(Duration::from_secs(30), Vec::new()));
        let coordinator = coordinator(source, engine, Arc::new(ManualClock::default()));
        coordinator.start_scheduler();
        let query_coordinator = coordinator.clone();
        let query_route = route_id.clone();
        let query = tokio::spawn(async move {
            query_coordinator
                .query_route(query_route, BalanceTrigger::Explicit)
                .await
        });
        tokio::task::yield_now().await;

        coordinator.shutdown().await;

        assert!(query.await.expect("query task").as_ref().is_err());
        assert!(
            coordinator
                .query_route(route_id, BalanceTrigger::Explicit)
                .await
                .as_ref()
                .is_err()
        );
    }
}
