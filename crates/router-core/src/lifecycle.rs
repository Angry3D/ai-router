use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::time::timeout;
use ts_rs::TS;

use crate::recovery::{DatabaseStartupIssue, RecoveryPointId};

const DEFAULT_SHUTDOWN_BUDGET: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum AppLifecyclePhase {
    Booting,
    ShellReady,
    DatabaseInitializing,
    CoreReady,
    ProxyStarting,
    Running,
    RecoveryRequired,
    DatabaseError,
    PortConflict,
    ProxyError,
    ShuttingDown,
    Exited,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum AppLifecycleIssue {
    BalanceStartupFailed,
    Database(DatabaseStartupIssue),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct AppLifecycleSnapshot {
    pub phase: AppLifecyclePhase,
    pub issue: Option<AppLifecycleIssue>,
}

impl Default for AppLifecycleSnapshot {
    fn default() -> Self {
        Self {
            phase: AppLifecyclePhase::Booting,
            issue: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LifecycleFailure {
    Database,
    RecoveryRequired,
    DatabaseIssue(DatabaseStartupIssue),
    PortConflict,
    Proxy,
    Balance,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ShutdownReport {
    pub balance_graceful: bool,
    pub proxy_graceful: bool,
    pub database_graceful: bool,
}

#[async_trait]
pub trait AppLifecycleServices: Send + Sync {
    async fn initialize_database(&self) -> Result<(), LifecycleFailure>;
    async fn start_proxy(&self) -> Result<(), LifecycleFailure>;
    async fn start_balance(&self) -> Result<(), LifecycleFailure>;
    async fn stop_balance(&self);
    async fn stop_proxy(&self);
    async fn close_database(&self);
    async fn restore_database(&self, point_id: &RecoveryPointId) -> Result<(), LifecycleFailure>;
    async fn start_over_database(&self) -> Result<(), LifecycleFailure>;
}

pub trait LifecycleStateChangeSink: Send + Sync {
    fn lifecycle_changed(&self, snapshot: &AppLifecycleSnapshot);
}

struct LifecycleState {
    snapshot: AppLifecycleSnapshot,
    database_ready: bool,
    proxy_ready: bool,
    balance_ready: bool,
    shutdown_report: Option<ShutdownReport>,
}

pub struct AppCoordinator {
    services: Arc<dyn AppLifecycleServices>,
    changes: Arc<dyn LifecycleStateChangeSink>,
    state: Mutex<LifecycleState>,
    operation: tokio::sync::Mutex<()>,
    initialization_started: AtomicBool,
    shutdown_budget: Duration,
}

impl AppCoordinator {
    #[must_use]
    pub fn new(
        services: Arc<dyn AppLifecycleServices>,
        changes: Arc<dyn LifecycleStateChangeSink>,
    ) -> Arc<Self> {
        Self::with_shutdown_budget(services, changes, DEFAULT_SHUTDOWN_BUDGET)
    }

    #[must_use]
    pub fn with_shutdown_budget(
        services: Arc<dyn AppLifecycleServices>,
        changes: Arc<dyn LifecycleStateChangeSink>,
        shutdown_budget: Duration,
    ) -> Arc<Self> {
        Arc::new(Self {
            services,
            changes,
            state: Mutex::new(LifecycleState {
                snapshot: AppLifecycleSnapshot::default(),
                database_ready: false,
                proxy_ready: false,
                balance_ready: false,
                shutdown_report: None,
            }),
            operation: tokio::sync::Mutex::new(()),
            initialization_started: AtomicBool::new(false),
            shutdown_budget,
        })
    }

    #[must_use]
    pub fn snapshot(&self) -> AppLifecycleSnapshot {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .snapshot
            .clone()
    }

    pub async fn start(&self) -> AppLifecycleSnapshot {
        if self.initialization_started.swap(true, Ordering::AcqRel) {
            return self.snapshot();
        }
        let _operation = self.operation.lock().await;
        self.transition(AppLifecyclePhase::ShellReady, None);
        self.transition(AppLifecyclePhase::DatabaseInitializing, None);
        if let Err(error) = self.services.initialize_database().await {
            self.transition_database_failure(error);
            return self.snapshot();
        }
        self.set_database_ready();
        self.start_after_database().await
    }

    async fn start_after_database(&self) -> AppLifecycleSnapshot {
        self.transition(AppLifecyclePhase::CoreReady, None);
        self.transition(AppLifecyclePhase::ProxyStarting, None);
        if let Err(error) = self.services.start_proxy().await {
            self.transition(
                if error == LifecycleFailure::PortConflict {
                    AppLifecyclePhase::PortConflict
                } else {
                    AppLifecyclePhase::ProxyError
                },
                None,
            );
            return self.snapshot();
        }
        self.set_proxy_ready();
        self.transition(AppLifecyclePhase::Running, None);
        if self.services.start_balance().await.is_err() {
            self.transition(
                AppLifecyclePhase::Running,
                Some(AppLifecycleIssue::BalanceStartupFailed),
            );
        } else {
            self.set_balance_ready();
        }
        self.snapshot()
    }

    pub async fn retry_database(&self) -> AppLifecycleSnapshot {
        self.recover_database(DatabaseRecoveryAction::Retry).await
    }

    pub async fn restore_database(&self, point_id: &RecoveryPointId) -> AppLifecycleSnapshot {
        self.recover_database(DatabaseRecoveryAction::Restore(point_id.clone()))
            .await
    }

    pub async fn start_over_database(&self) -> AppLifecycleSnapshot {
        self.recover_database(DatabaseRecoveryAction::StartOver)
            .await
    }

    async fn recover_database(&self, action: DatabaseRecoveryAction) -> AppLifecycleSnapshot {
        let _operation = self.operation.lock().await;
        let allowed = match action {
            DatabaseRecoveryAction::Retry => {
                self.snapshot().phase == AppLifecyclePhase::DatabaseError
            }
            DatabaseRecoveryAction::Restore(_) | DatabaseRecoveryAction::StartOver => {
                self.snapshot().phase == AppLifecyclePhase::RecoveryRequired
            }
        };
        if !allowed {
            return self.snapshot();
        }
        self.transition(AppLifecyclePhase::DatabaseInitializing, None);
        let result = match &action {
            DatabaseRecoveryAction::Retry => self.services.initialize_database().await,
            DatabaseRecoveryAction::Restore(point_id) => {
                self.services.restore_database(point_id).await
            }
            DatabaseRecoveryAction::StartOver => self.services.start_over_database().await,
        };
        if let Err(error) = result {
            self.transition_database_failure(error);
            return self.snapshot();
        }
        self.set_database_ready();
        self.start_after_database().await
    }

    pub async fn shutdown(&self) -> ShutdownReport {
        let _operation = self.operation.lock().await;
        if let Some(report) = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .shutdown_report
        {
            return report;
        }
        self.transition(AppLifecyclePhase::ShuttingDown, self.snapshot().issue);
        let (balance_ready, proxy_ready, database_ready) = {
            let state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            (state.balance_ready, state.proxy_ready, state.database_ready)
        };
        let balance_graceful = !balance_ready
            || timeout(self.shutdown_budget, self.services.stop_balance())
                .await
                .is_ok();
        let proxy_graceful = !proxy_ready
            || timeout(self.shutdown_budget, self.services.stop_proxy())
                .await
                .is_ok();
        let database_graceful = !database_ready
            || timeout(self.shutdown_budget, self.services.close_database())
                .await
                .is_ok();
        let report = ShutdownReport {
            balance_graceful,
            proxy_graceful,
            database_graceful,
        };
        {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.balance_ready = false;
            state.proxy_ready = false;
            state.database_ready = false;
            state.shutdown_report = Some(report);
        }
        self.transition(AppLifecyclePhase::Exited, self.snapshot().issue);
        report
    }

    pub async fn recover_proxy(&self) -> AppLifecycleSnapshot {
        let _operation = self.operation.lock().await;
        if !matches!(
            self.snapshot().phase,
            AppLifecyclePhase::PortConflict | AppLifecyclePhase::ProxyError
        ) {
            return self.snapshot();
        }
        self.transition(AppLifecyclePhase::ProxyStarting, self.snapshot().issue);
        if let Err(error) = self.services.start_proxy().await {
            self.transition(
                if error == LifecycleFailure::PortConflict {
                    AppLifecyclePhase::PortConflict
                } else {
                    AppLifecyclePhase::ProxyError
                },
                self.snapshot().issue,
            );
            return self.snapshot();
        }
        self.set_proxy_ready();
        self.transition(AppLifecyclePhase::Running, None);
        if self.services.start_balance().await.is_err() {
            self.transition(
                AppLifecyclePhase::Running,
                Some(AppLifecycleIssue::BalanceStartupFailed),
            );
        } else {
            self.set_balance_ready();
        }
        self.snapshot()
    }

    fn transition(&self, phase: AppLifecyclePhase, issue: Option<AppLifecycleIssue>) {
        let snapshot = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.snapshot = AppLifecycleSnapshot { phase, issue };
            state.snapshot.clone()
        };
        self.changes.lifecycle_changed(&snapshot);
    }

    fn transition_database_failure(&self, failure: LifecycleFailure) {
        match failure {
            LifecycleFailure::RecoveryRequired => {
                self.transition(AppLifecyclePhase::RecoveryRequired, None);
            }
            LifecycleFailure::DatabaseIssue(issue) => self.transition(
                AppLifecyclePhase::DatabaseError,
                Some(AppLifecycleIssue::Database(issue)),
            ),
            _ => self.transition(
                AppLifecyclePhase::DatabaseError,
                Some(AppLifecycleIssue::Database(
                    DatabaseStartupIssue::Unavailable,
                )),
            ),
        }
    }

    fn set_database_ready(&self) {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .database_ready = true;
    }

    fn set_proxy_ready(&self) {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .proxy_ready = true;
    }

    fn set_balance_ready(&self) {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .balance_ready = true;
    }
}

enum DatabaseRecoveryAction {
    Retry,
    Restore(RecoveryPointId),
    StartOver,
}

#[cfg(test)]
mod tests {
    use std::{sync::atomic::AtomicUsize, time::Duration};

    use super::*;

    struct MockServices {
        database_result: LifecycleFailure,
        proxy_result: LifecycleFailure,
        balance_result: LifecycleFailure,
        database_calls: AtomicUsize,
        restore_calls: AtomicUsize,
        proxy_calls: AtomicUsize,
        balance_calls: AtomicUsize,
        recover_proxy_after_failure: bool,
        recovery_result: LifecycleFailure,
        recovery_delay: Duration,
        stop_balance_delay: Duration,
        stop_proxy_delay: Duration,
        close_database_delay: Duration,
    }

    impl Default for MockServices {
        fn default() -> Self {
            Self {
                database_result: LifecycleFailure::Balance,
                proxy_result: LifecycleFailure::Balance,
                balance_result: LifecycleFailure::Database,
                database_calls: AtomicUsize::new(0),
                restore_calls: AtomicUsize::new(0),
                proxy_calls: AtomicUsize::new(0),
                balance_calls: AtomicUsize::new(0),
                recover_proxy_after_failure: false,
                recovery_result: LifecycleFailure::Balance,
                recovery_delay: Duration::ZERO,
                stop_balance_delay: Duration::ZERO,
                stop_proxy_delay: Duration::ZERO,
                close_database_delay: Duration::ZERO,
            }
        }
    }

    #[async_trait]
    impl AppLifecycleServices for MockServices {
        async fn initialize_database(&self) -> Result<(), LifecycleFailure> {
            self.database_calls.fetch_add(1, Ordering::SeqCst);
            match self.database_result {
                LifecycleFailure::Database
                | LifecycleFailure::RecoveryRequired
                | LifecycleFailure::DatabaseIssue(_) => Err(self.database_result),
                _ => Ok(()),
            }
        }

        async fn start_proxy(&self) -> Result<(), LifecycleFailure> {
            let previous_calls = self.proxy_calls.fetch_add(1, Ordering::SeqCst);
            if self.recover_proxy_after_failure && previous_calls == 0 {
                return Err(LifecycleFailure::PortConflict);
            }
            success_unless_any(
                self.proxy_result,
                &[LifecycleFailure::PortConflict, LifecycleFailure::Proxy],
            )
        }

        async fn start_balance(&self) -> Result<(), LifecycleFailure> {
            self.balance_calls.fetch_add(1, Ordering::SeqCst);
            success_unless(self.balance_result, LifecycleFailure::Balance)
        }

        async fn stop_balance(&self) {
            tokio::time::sleep(self.stop_balance_delay).await;
        }

        async fn stop_proxy(&self) {
            tokio::time::sleep(self.stop_proxy_delay).await;
        }

        async fn close_database(&self) {
            tokio::time::sleep(self.close_database_delay).await;
        }

        async fn restore_database(
            &self,
            _point_id: &RecoveryPointId,
        ) -> Result<(), LifecycleFailure> {
            self.restore_calls.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(self.recovery_delay).await;
            match self.recovery_result {
                LifecycleFailure::Database
                | LifecycleFailure::RecoveryRequired
                | LifecycleFailure::DatabaseIssue(_) => Err(self.recovery_result),
                _ => Ok(()),
            }
        }

        async fn start_over_database(&self) -> Result<(), LifecycleFailure> {
            self.restore_database(&RecoveryPointId::new()).await
        }
    }

    #[derive(Default)]
    struct RecordingChanges(Mutex<Vec<AppLifecycleSnapshot>>);

    impl LifecycleStateChangeSink for RecordingChanges {
        fn lifecycle_changed(&self, snapshot: &AppLifecycleSnapshot) {
            self.0
                .lock()
                .expect("lifecycle changes mutex")
                .push(snapshot.clone());
        }
    }

    fn success_unless(
        configured: LifecycleFailure,
        failure: LifecycleFailure,
    ) -> Result<(), LifecycleFailure> {
        if configured == failure {
            Err(failure)
        } else {
            Ok(())
        }
    }

    fn success_unless_any(
        configured: LifecycleFailure,
        failures: &[LifecycleFailure],
    ) -> Result<(), LifecycleFailure> {
        if failures.contains(&configured) {
            Err(configured)
        } else {
            Ok(())
        }
    }

    #[tokio::test]
    async fn lifecycle_database_port_and_balance_failures_remain_distinct() {
        let database_failure = Arc::new(MockServices {
            database_result: LifecycleFailure::Database,
            ..MockServices::default()
        });
        let coordinator = AppCoordinator::new(
            database_failure.clone(),
            Arc::new(RecordingChanges::default()),
        );
        assert_eq!(
            coordinator.start().await.phase,
            AppLifecyclePhase::DatabaseError
        );
        assert_eq!(database_failure.proxy_calls.load(Ordering::SeqCst), 0);

        let port_failure = Arc::new(MockServices {
            proxy_result: LifecycleFailure::PortConflict,
            ..MockServices::default()
        });
        let coordinator =
            AppCoordinator::new(port_failure.clone(), Arc::new(RecordingChanges::default()));
        assert_eq!(
            coordinator.start().await.phase,
            AppLifecyclePhase::PortConflict
        );
        assert_eq!(port_failure.balance_calls.load(Ordering::SeqCst), 0);

        let balance_failure = Arc::new(MockServices {
            balance_result: LifecycleFailure::Balance,
            ..MockServices::default()
        });
        let coordinator =
            AppCoordinator::new(balance_failure, Arc::new(RecordingChanges::default()));
        assert_eq!(
            coordinator.start().await,
            AppLifecycleSnapshot {
                phase: AppLifecyclePhase::Running,
                issue: Some(AppLifecycleIssue::BalanceStartupFailed),
            }
        );
    }

    #[tokio::test]
    async fn lifecycle_shutdown_applies_independent_five_second_style_budgets() {
        let services = Arc::new(MockServices {
            stop_proxy_delay: Duration::from_millis(30),
            ..MockServices::default()
        });
        let changes = Arc::new(RecordingChanges::default());
        let coordinator = AppCoordinator::with_shutdown_budget(
            services,
            changes.clone(),
            Duration::from_millis(5),
        );
        assert_eq!(coordinator.start().await.phase, AppLifecyclePhase::Running);

        let report = coordinator.shutdown().await;

        assert!(report.balance_graceful);
        assert!(!report.proxy_graceful);
        assert!(report.database_graceful);
        assert_eq!(coordinator.snapshot().phase, AppLifecyclePhase::Exited);
        assert!(
            changes
                .0
                .lock()
                .expect("changes mutex")
                .iter()
                .any(|snapshot| snapshot.phase == AppLifecyclePhase::ShuttingDown)
        );
    }

    #[tokio::test]
    async fn single_instance_second_start_does_not_reinitialize_services() {
        let services = Arc::new(MockServices::default());
        let coordinator =
            AppCoordinator::new(services.clone(), Arc::new(RecordingChanges::default()));

        assert_eq!(coordinator.start().await.phase, AppLifecyclePhase::Running);
        assert_eq!(coordinator.start().await.phase, AppLifecyclePhase::Running);

        assert_eq!(services.database_calls.load(Ordering::SeqCst), 1);
        assert_eq!(services.proxy_calls.load(Ordering::SeqCst), 1);
        assert_eq!(services.balance_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn proxy_recovery_does_not_reinitialize_the_database() {
        let services = Arc::new(MockServices {
            recover_proxy_after_failure: true,
            ..MockServices::default()
        });
        let coordinator =
            AppCoordinator::new(services.clone(), Arc::new(RecordingChanges::default()));

        assert_eq!(
            coordinator.start().await.phase,
            AppLifecyclePhase::PortConflict
        );
        assert_eq!(
            coordinator.recover_proxy().await.phase,
            AppLifecyclePhase::Running
        );
        assert_eq!(services.database_calls.load(Ordering::SeqCst), 1);
        assert_eq!(services.proxy_calls.load(Ordering::SeqCst), 2);
        assert_eq!(services.balance_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn concurrent_restore_is_serialized_and_runs_recovery_only_once() {
        let services = Arc::new(MockServices {
            database_result: LifecycleFailure::RecoveryRequired,
            recovery_delay: Duration::from_millis(20),
            ..MockServices::default()
        });
        let coordinator =
            AppCoordinator::new(services.clone(), Arc::new(RecordingChanges::default()));
        assert_eq!(
            coordinator.start().await.phase,
            AppLifecyclePhase::RecoveryRequired
        );
        assert_eq!(services.proxy_calls.load(Ordering::SeqCst), 0);

        let point_id = RecoveryPointId::new();
        let (first, duplicate) = tokio::join!(
            coordinator.restore_database(&point_id),
            coordinator.restore_database(&point_id)
        );
        assert_eq!(first.phase, AppLifecyclePhase::Running);
        assert_eq!(duplicate.phase, AppLifecyclePhase::Running);
        assert_eq!(services.restore_calls.load(Ordering::SeqCst), 1);
        assert_eq!(services.proxy_calls.load(Ordering::SeqCst), 1);
        assert_eq!(services.balance_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn failed_restore_keeps_proxy_and_balance_stopped_with_typed_retry_state() {
        let services = Arc::new(MockServices {
            database_result: LifecycleFailure::RecoveryRequired,
            recovery_result: LifecycleFailure::RecoveryRequired,
            ..MockServices::default()
        });
        let coordinator =
            AppCoordinator::new(services.clone(), Arc::new(RecordingChanges::default()));
        assert_eq!(
            coordinator.start().await.phase,
            AppLifecyclePhase::RecoveryRequired
        );

        let failed = coordinator.restore_database(&RecoveryPointId::new()).await;
        assert_eq!(failed.phase, AppLifecyclePhase::RecoveryRequired);
        assert_eq!(services.restore_calls.load(Ordering::SeqCst), 1);
        assert_eq!(services.proxy_calls.load(Ordering::SeqCst), 0);
        assert_eq!(services.balance_calls.load(Ordering::SeqCst), 0);
    }
}
