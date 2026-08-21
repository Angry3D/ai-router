use std::sync::{
    Arc, RwLock,
    atomic::{AtomicU64, Ordering},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use ts_rs::TS;

use crate::{
    domain::{AppearancePreference, InferenceStatus, ProxyRuntimeStatus, RouteId},
    lifecycle::{AppLifecycleIssue, AppLifecyclePhase, AppLifecycleSnapshot},
    proxy::{RecoveryOrigin, RouteHealthSnapshot},
};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum RouteHealthOriginDto {
    ProviderFailure,
    ModelBypassed,
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
pub enum RouteHealthDto {
    Striking {
        failure_count: u8,
    },
    Switching,
    SwitchPending {
        retry_after_seconds: Option<u16>,
    },
    Open {
        origin: RouteHealthOriginDto,
        recovery_successes: u8,
        retry_after_seconds: u16,
    },
    Probing {
        recovery_successes: u8,
    },
}

impl From<RouteHealthSnapshot> for RouteHealthDto {
    fn from(value: RouteHealthSnapshot) -> Self {
        match value {
            RouteHealthSnapshot::Striking { failure_count } => Self::Striking { failure_count },
            RouteHealthSnapshot::Switching => Self::Switching,
            RouteHealthSnapshot::SwitchPending {
                retry_after_seconds,
            } => Self::SwitchPending {
                retry_after_seconds,
            },
            RouteHealthSnapshot::Open {
                origin,
                recovery_successes,
                retry_after_seconds,
            } => Self::Open {
                origin: match origin {
                    RecoveryOrigin::ProviderFailure => RouteHealthOriginDto::ProviderFailure,
                    RecoveryOrigin::ModelBypassed => RouteHealthOriginDto::ModelBypassed,
                },
                recovery_successes,
                retry_after_seconds,
            },
            RouteHealthSnapshot::Probing { recovery_successes } => {
                Self::Probing { recovery_successes }
            }
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum StateArea {
    Routes,
    Route,
    Fallback,
    Balance,
    BalanceSettings,
    ImagesGeneration,
    Proxy,
    CodexConnection,
    CodexCatalog,
    CodexRestartNotice,
    RequestHistorySummary,
    RuntimeLogs,
    Recovery,
    Appearance,
    MenuBar,
    ApplicationUpdate,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct StateChangedEventDto {
    #[ts(type = "number")]
    pub revision: u64,
    pub areas: Vec<StateArea>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct RouteSummaryDto {
    pub route_id: RouteId,
    pub name: String,
    pub base_url_host: String,
    pub inference_status: InferenceStatus,
    pub health: Option<RouteHealthDto>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct FallbackStateDto {
    pub enabled: bool,
    pub participant_count: u32,
    #[ts(type = "number")]
    pub config_revision: u64,
    pub active_position: Option<u32>,
    pub has_next: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct BootstrapSnapshotDto {
    #[ts(type = "number")]
    pub revision: u64,
    pub routes: Vec<RouteSummaryDto>,
    pub active_route_id: Option<RouteId>,
    pub fallback: FallbackStateDto,
    pub proxy_status: ProxyRuntimeStatus,
    pub lifecycle: AppLifecycleSnapshot,
    pub appearance_preference: AppearancePreference,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct MutationResultDto {
    #[ts(type = "number")]
    pub revision: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct IpcErrorDto {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    pub field: Option<String>,
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
#[error("state event publication failed")]
pub struct StateEventError;

pub trait StateEventSink: Send + Sync {
    /// Publishes one narrow state-change notification.
    ///
    /// # Errors
    ///
    /// Returns a safe publication error when the platform event channel fails.
    fn publish(&self, event: &StateChangedEventDto) -> Result<(), StateEventError>;
}

pub struct StateCoordinator {
    revision: AtomicU64,
    sink: Arc<dyn StateEventSink>,
}

#[derive(Clone)]
struct RuntimeProjection {
    routes: Vec<RouteSummaryDto>,
    active_route_id: Option<RouteId>,
    fallback: FallbackStateDto,
    proxy_status: ProxyRuntimeStatus,
    lifecycle: AppLifecycleSnapshot,
    appearance_preference: AppearancePreference,
    menu_bar_settings: Option<crate::app_api::MenuBarSettingsDto>,
}

impl Default for RuntimeProjection {
    fn default() -> Self {
        Self {
            routes: Vec::new(),
            active_route_id: None,
            fallback: FallbackStateDto::default(),
            proxy_status: ProxyRuntimeStatus::Stopped,
            lifecycle: AppLifecycleSnapshot::default(),
            appearance_preference: AppearancePreference::System,
            menu_bar_settings: None,
        }
    }
}

pub struct AppRuntimeState {
    coordinator: StateCoordinator,
    projection: RwLock<RuntimeProjection>,
}

#[derive(Default)]
pub struct RuntimeProjectionUpdate {
    pub routes: Option<Vec<RouteSummaryDto>>,
    pub active_route_id: Option<Option<RouteId>>,
    pub fallback: Option<FallbackStateDto>,
    pub proxy_status: Option<ProxyRuntimeStatus>,
    pub appearance_preference: Option<AppearancePreference>,
    pub menu_bar_settings: Option<crate::app_api::MenuBarSettingsDto>,
}

impl StateCoordinator {
    #[must_use]
    pub const fn new(sink: Arc<dyn StateEventSink>) -> Self {
        Self {
            revision: AtomicU64::new(0),
            sink,
        }
    }

    #[must_use]
    pub fn revision(&self) -> u64 {
        self.revision.load(Ordering::Acquire)
    }

    /// Converts a successful committed operation into a new revision and event.
    ///
    /// # Errors
    ///
    /// Returns the original operation error without changing revision or
    /// publishing when the supplied result is an error.
    pub fn commit<T, E>(
        &self,
        result: Result<T, E>,
        areas: Vec<StateArea>,
    ) -> Result<(T, MutationResultDto), E> {
        let value = result?;
        Ok((value, self.publish_committed(areas)))
    }

    fn publish_committed(&self, mut areas: Vec<StateArea>) -> MutationResultDto {
        areas.sort_unstable();
        areas.dedup();
        let revision = self.revision.fetch_add(1, Ordering::AcqRel) + 1;
        let event = StateChangedEventDto { revision, areas };
        let _ = self.sink.publish(&event);
        MutationResultDto { revision }
    }
}

impl AppRuntimeState {
    #[must_use]
    pub fn new(sink: Arc<dyn StateEventSink>) -> Self {
        Self {
            coordinator: StateCoordinator::new(sink),
            projection: RwLock::new(RuntimeProjection::default()),
        }
    }

    #[must_use]
    pub fn bootstrap_snapshot(&self) -> BootstrapSnapshotDto {
        let projection = self
            .projection
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        BootstrapSnapshotDto {
            revision: self.coordinator.revision(),
            routes: projection.routes.clone(),
            active_route_id: projection.active_route_id.clone(),
            fallback: projection.fallback.clone(),
            proxy_status: projection.proxy_status.clone(),
            lifecycle: projection.lifecycle.clone(),
            appearance_preference: projection.appearance_preference,
        }
    }

    #[must_use]
    pub fn menu_bar_settings(&self) -> Option<crate::app_api::MenuBarSettingsDto> {
        self.projection
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .menu_bar_settings
    }

    pub fn publish_background_change(&self, areas: Vec<StateArea>) -> MutationResultDto {
        self.coordinator.publish_committed(areas)
    }

    /// Applies an in-memory projection only after its owning operation commits.
    ///
    /// # Errors
    ///
    /// Returns the original operation error without changing projection,
    /// revision, or events.
    pub fn apply_committed<T, E>(
        &self,
        result: Result<T, E>,
        areas: Vec<StateArea>,
        update: RuntimeProjectionUpdate,
    ) -> Result<(T, MutationResultDto), E> {
        let value = result?;
        {
            let mut projection = self
                .projection
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(routes) = update.routes {
                projection.routes = routes;
            }
            if let Some(active_route_id) = update.active_route_id {
                projection.active_route_id = active_route_id;
            }
            if let Some(fallback) = update.fallback {
                projection.fallback = fallback;
            }
            if let Some(proxy_status) = update.proxy_status {
                projection.proxy_status = proxy_status;
            }
            if let Some(appearance_preference) = update.appearance_preference {
                projection.appearance_preference = appearance_preference;
            }
            if let Some(menu_bar_settings) = update.menu_bar_settings {
                projection.menu_bar_settings = Some(menu_bar_settings);
            }
        }
        let mutation = self.coordinator.publish_committed(areas);
        Ok((value, mutation))
    }
}

impl crate::proxy::InferenceStatusChangeSink for AppRuntimeState {
    fn inference_statuses_changed(&self, updates: Vec<(RouteId, InferenceStatus)>) {
        let mut projection = self
            .projection
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut changed = false;
        for (route_id, status) in updates {
            let Some(route) = projection
                .routes
                .iter_mut()
                .find(|route| route.route_id == route_id)
            else {
                continue;
            };
            if route.inference_status != status {
                route.inference_status = status;
                changed = true;
            }
        }
        drop(projection);
        if changed {
            self.publish_background_change(vec![StateArea::Routes]);
        }
    }
}

impl crate::proxy::HealthChangeSink for AppRuntimeState {
    fn route_health_changed(
        &self,
        route_id: RouteId,
        health: Option<crate::proxy::RouteHealthSnapshot>,
    ) {
        let health = health.map(Into::into);
        let mut projection = self
            .projection
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(route) = projection
            .routes
            .iter_mut()
            .find(|route| route.route_id == route_id)
        else {
            return;
        };
        if route.health == health {
            return;
        }
        route.health = health;
        drop(projection);
        self.publish_background_change(vec![StateArea::Routes]);
    }
}

impl crate::proxy::HistorySummaryChangeSink for AppRuntimeState {
    fn history_summary_changed(&self) {
        self.publish_background_change(vec![StateArea::RequestHistorySummary]);
    }
}

impl crate::balance::BalanceStateChangeSink for AppRuntimeState {
    fn balance_changed(&self) {
        self.publish_background_change(vec![StateArea::Balance]);
    }
}

impl crate::lifecycle::LifecycleStateChangeSink for AppRuntimeState {
    fn lifecycle_changed(&self, snapshot: &AppLifecycleSnapshot) {
        let proxy_status = match snapshot.phase {
            AppLifecyclePhase::Booting | AppLifecyclePhase::ShellReady => {
                ProxyRuntimeStatus::Stopped
            }
            AppLifecyclePhase::DatabaseInitializing
            | AppLifecyclePhase::CoreReady
            | AppLifecyclePhase::ProxyStarting => ProxyRuntimeStatus::Starting,
            AppLifecyclePhase::Running => ProxyRuntimeStatus::Running,
            AppLifecyclePhase::RecoveryRequired | AppLifecyclePhase::DatabaseError => {
                ProxyRuntimeStatus::DatabaseError
            }
            AppLifecyclePhase::PortConflict => ProxyRuntimeStatus::PortConflict,
            AppLifecyclePhase::ProxyError => ProxyRuntimeStatus::Error,
            AppLifecyclePhase::ShuttingDown => ProxyRuntimeStatus::ShuttingDown,
            AppLifecyclePhase::Exited => ProxyRuntimeStatus::Stopped,
        };
        {
            let mut projection = self
                .projection
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            projection.proxy_status = proxy_status;
            projection.lifecycle = snapshot.clone();
        }
        let mut areas = vec![StateArea::Proxy, StateArea::Recovery];
        if snapshot.issue == Some(AppLifecycleIssue::BalanceStartupFailed) {
            areas.push(StateArea::Balance);
        }
        self.publish_background_change(areas);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::{
        AppRuntimeState, RouteSummaryDto, RuntimeProjectionUpdate, StateArea, StateChangedEventDto,
        StateCoordinator, StateEventError, StateEventSink,
    };
    use crate::domain::{AppearancePreference, ProxyRuntimeStatus, RouteId};

    #[derive(Default)]
    struct RecordingSink(Mutex<Vec<StateChangedEventDto>>);

    impl StateEventSink for RecordingSink {
        fn publish(&self, event: &StateChangedEventDto) -> Result<(), StateEventError> {
            self.0.lock().expect("sink lock").push(event.clone());
            Ok(())
        }
    }

    #[test]
    fn failed_mutations_do_not_increment_or_publish() {
        let sink = Arc::new(RecordingSink::default());
        let coordinator = StateCoordinator::new(sink.clone());
        let result: Result<((), _), &str> =
            coordinator.commit(Err("rollback"), vec![StateArea::Routes]);
        assert_eq!(result, Err("rollback"));
        assert_eq!(coordinator.revision(), 0);
        assert!(sink.0.lock().expect("sink lock").is_empty());
    }

    #[test]
    fn committed_mutations_increment_and_deduplicate_areas() {
        let sink = Arc::new(RecordingSink::default());
        let coordinator = StateCoordinator::new(sink.clone());
        let ((), result) = coordinator
            .commit::<_, ()>(
                Ok(()),
                vec![StateArea::Routes, StateArea::Proxy, StateArea::Routes],
            )
            .expect("commit");
        assert_eq!(result.revision, 1);
        assert_eq!(
            sink.0.lock().expect("sink lock")[0].areas,
            vec![StateArea::Routes, StateArea::Proxy]
        );
    }

    #[test]
    fn appearance_preference_projection_precedes_publication() {
        let sink = Arc::new(RecordingSink::default());
        let runtime = AppRuntimeState::new(sink.clone());
        runtime
            .apply_committed::<_, ()>(
                Ok(()),
                vec![StateArea::Appearance],
                RuntimeProjectionUpdate {
                    appearance_preference: Some(AppearancePreference::Dark),
                    ..RuntimeProjectionUpdate::default()
                },
            )
            .expect("appearance commit");
        assert_eq!(
            runtime.bootstrap_snapshot().appearance_preference,
            AppearancePreference::Dark
        );
        assert_eq!(
            sink.0.lock().expect("sink lock")[0].areas,
            vec![StateArea::Appearance]
        );
    }

    #[test]
    fn runtime_projection_changes_only_after_commit() {
        let sink = Arc::new(RecordingSink::default());
        let runtime = AppRuntimeState::new(sink.clone());
        let failed: Result<(Vec<RouteSummaryDto>, _), &str> = runtime.apply_committed(
            Err("rollback"),
            vec![StateArea::Routes],
            RuntimeProjectionUpdate::default(),
        );
        assert_eq!(failed, Err("rollback"));
        assert_eq!(runtime.bootstrap_snapshot().revision, 0);

        let route_id = RouteId::new();
        let routes = vec![RouteSummaryDto {
            route_id: route_id.clone(),
            name: "Work".to_owned(),
            base_url_host: "example.com".to_owned(),
            inference_status: crate::domain::InferenceStatus {
                kind: crate::domain::InferenceStatusKind::Unverified,
                last_outcome: None,
                failure_reason: None,
                observed_at_ms: None,
            },
            health: None,
        }];
        let projected_routes = routes.clone();
        runtime
            .apply_committed::<_, ()>(
                Ok(routes),
                vec![StateArea::Routes],
                RuntimeProjectionUpdate {
                    routes: Some(projected_routes),
                    active_route_id: Some(Some(route_id)),
                    fallback: None,
                    proxy_status: Some(ProxyRuntimeStatus::Running),
                    appearance_preference: None,
                    menu_bar_settings: None,
                },
            )
            .expect("commit");
        let snapshot = runtime.bootstrap_snapshot();
        assert_eq!(snapshot.revision, 1);
        assert_eq!(snapshot.routes.len(), 1);
        assert_eq!(
            snapshot.active_route_id,
            Some(snapshot.routes[0].route_id.clone())
        );
    }

    #[test]
    fn background_inference_changes_update_projection_and_publish_the_routes_area() {
        let sink = Arc::new(RecordingSink::default());
        let runtime = AppRuntimeState::new(sink.clone());
        let route_id = RouteId::new();
        let initial = RouteSummaryDto {
            route_id: route_id.clone(),
            name: "Work".to_owned(),
            base_url_host: "example.com".to_owned(),
            inference_status: crate::domain::InferenceStatus {
                kind: crate::domain::InferenceStatusKind::Unverified,
                last_outcome: None,
                failure_reason: None,
                observed_at_ms: None,
            },
            health: None,
        };
        runtime
            .apply_committed::<_, ()>(
                Ok(()),
                vec![StateArea::Routes],
                RuntimeProjectionUpdate {
                    routes: Some(vec![initial]),
                    ..RuntimeProjectionUpdate::default()
                },
            )
            .expect("initial projection");
        sink.0.lock().expect("sink lock").clear();

        crate::proxy::InferenceStatusChangeSink::inference_statuses_changed(
            &runtime,
            vec![(
                route_id,
                crate::domain::InferenceStatus {
                    kind: crate::domain::InferenceStatusKind::RecentFailure,
                    last_outcome: Some(crate::domain::InferenceOutcome::Failure),
                    failure_reason: Some(crate::domain::InferenceFailureReason::Service),
                    observed_at_ms: Some(1_000),
                },
            )],
        );

        let snapshot = runtime.bootstrap_snapshot();
        assert_eq!(snapshot.revision, 2);
        assert_eq!(
            snapshot.routes[0].inference_status.kind,
            crate::domain::InferenceStatusKind::RecentFailure
        );
        assert_eq!(
            sink.0.lock().expect("sink lock")[0].areas,
            vec![StateArea::Routes]
        );
    }

    #[test]
    fn background_health_changes_update_projection_before_publication() {
        let sink = Arc::new(RecordingSink::default());
        let runtime = AppRuntimeState::new(sink.clone());
        let route_id = RouteId::new();
        runtime
            .apply_committed::<_, ()>(
                Ok(()),
                vec![StateArea::Routes],
                RuntimeProjectionUpdate {
                    routes: Some(vec![RouteSummaryDto {
                        route_id: route_id.clone(),
                        name: "Work".to_owned(),
                        base_url_host: "example.com".to_owned(),
                        inference_status: crate::domain::InferenceStatus {
                            kind: crate::domain::InferenceStatusKind::Unverified,
                            last_outcome: None,
                            failure_reason: None,
                            observed_at_ms: None,
                        },
                        health: None,
                    }]),
                    ..RuntimeProjectionUpdate::default()
                },
            )
            .expect("initial projection");
        sink.0.lock().expect("sink lock").clear();

        crate::proxy::HealthChangeSink::route_health_changed(
            &runtime,
            route_id,
            Some(crate::proxy::RouteHealthSnapshot::Striking { failure_count: 3 }),
        );

        assert_eq!(
            runtime.bootstrap_snapshot().routes[0].health,
            Some(super::RouteHealthDto::Striking { failure_count: 3 })
        );
        assert_eq!(
            sink.0.lock().expect("sink lock")[0].areas,
            vec![StateArea::Routes]
        );
    }

    #[test]
    fn background_balance_changes_publish_the_balance_area() {
        let sink = Arc::new(RecordingSink::default());
        let runtime = AppRuntimeState::new(sink.clone());

        crate::balance::BalanceStateChangeSink::balance_changed(&runtime);

        assert_eq!(runtime.bootstrap_snapshot().revision, 1);
        assert_eq!(
            sink.0.lock().expect("sink lock")[0].areas,
            vec![StateArea::Balance]
        );
    }

    #[test]
    fn background_history_changes_publish_the_request_history_summary_area() {
        let sink = Arc::new(RecordingSink::default());
        let runtime = AppRuntimeState::new(sink.clone());

        crate::proxy::HistorySummaryChangeSink::history_summary_changed(&runtime);

        assert_eq!(runtime.bootstrap_snapshot().revision, 1);
        assert_eq!(
            sink.0.lock().expect("sink lock")[0].areas,
            vec![StateArea::RequestHistorySummary]
        );
    }

    #[test]
    fn lifecycle_changes_update_proxy_projection_and_balance_issue_area() {
        let sink = Arc::new(RecordingSink::default());
        let runtime = AppRuntimeState::new(sink.clone());

        crate::lifecycle::LifecycleStateChangeSink::lifecycle_changed(
            &runtime,
            &crate::lifecycle::AppLifecycleSnapshot {
                phase: crate::lifecycle::AppLifecyclePhase::PortConflict,
                issue: None,
            },
        );
        assert_eq!(
            runtime.bootstrap_snapshot().proxy_status,
            ProxyRuntimeStatus::PortConflict
        );
        crate::lifecycle::LifecycleStateChangeSink::lifecycle_changed(
            &runtime,
            &crate::lifecycle::AppLifecycleSnapshot {
                phase: crate::lifecycle::AppLifecyclePhase::Running,
                issue: Some(crate::lifecycle::AppLifecycleIssue::BalanceStartupFailed),
            },
        );

        assert_eq!(
            runtime.bootstrap_snapshot().proxy_status,
            ProxyRuntimeStatus::Running
        );
        assert_eq!(
            sink.0.lock().expect("sink lock")[1].areas,
            vec![StateArea::Balance, StateArea::Proxy, StateArea::Recovery]
        );
    }

    #[test]
    fn recovery_required_lifecycle_changes_publish_the_recovery_area() {
        let sink = Arc::new(RecordingSink::default());
        let runtime = AppRuntimeState::new(sink.clone());

        crate::lifecycle::LifecycleStateChangeSink::lifecycle_changed(
            &runtime,
            &crate::lifecycle::AppLifecycleSnapshot {
                phase: crate::lifecycle::AppLifecyclePhase::RecoveryRequired,
                issue: None,
            },
        );

        assert_eq!(
            sink.0.lock().expect("sink lock")[0].areas,
            vec![StateArea::Proxy, StateArea::Recovery]
        );
    }
}
