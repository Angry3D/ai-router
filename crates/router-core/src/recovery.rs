use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::{sync::watch, task::JoinHandle};
use ts_rs::TS;
use uuid::Uuid;

use crate::{
    balance::BalanceQueryMode,
    domain::{
        BalanceQueryPolicy, BalanceScriptSource, CodexModel, ImagesGenerationModel,
        ImagesGenerationTimeout, McpImageCapacityWarningThreshold, OutboundProxyConfig,
        OutboundProxyUrl,
    },
    incident::{self, IncidentAction, IncidentKind, IncidentRecheck, IncidentRecord},
    storage::{DatabaseExecutor, SCHEMA_VERSION, StorageError},
};

pub const RECOVERY_FORMAT_VERSION: i64 = 1;
pub const MAX_VALID_POINTS: usize = 5;
pub const RECOVERY_RETENTION: Duration = Duration::from_hours(720);
pub const RECOVERY_QUIET_PERIOD: Duration = Duration::from_millis(250);
/// Delay between runtime integrity self-checks.
pub const RECOVERY_SELF_CHECK_INTERVAL: Duration = Duration::from_hours(6);
/// Delay before the first runtime integrity self-check after startup.
pub const RECOVERY_SELF_CHECK_INITIAL_DELAY: Duration = Duration::from_mins(10);
/// Traffic-free window the activity probe must observe before a self-check runs.
pub const RECOVERY_SELF_CHECK_IDLE_MINIMUM: Duration = Duration::from_mins(5);
/// Name of the exclusive data-directory lock file beside the primary database.
pub const DIRECTORY_LOCK_FILE_NAME_SUFFIX: &str = ".lock";

const APPLICATION_TABLES: [&str; 16] = [
    "app_settings",
    "balance_queries",
    "codex_baseline",
    "codex_recovery_config",
    "codex_models",
    "codex_restart_notice",
    "fallback_config",
    "proxy_requests",
    "recovery_point_metadata",
    "recovery_revision",
    "route_fallback_excluded_models",
    "route_state",
    "routes",
    "secrets",
    "upstream_attempts",
    "upstream_attempt_routing_skips",
];

type AppSettingsDomainRow = (
    i64,
    i64,
    i64,
    i64,
    Option<String>,
    i64,
    Option<i64>,
    i64,
    i64,
    i64,
    Option<String>,
    String,
);

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct RecoveryPointId(String);

impl RecoveryPointId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4().to_string())
    }

    /// Parses an opaque point identifier without accepting a path fragment.
    ///
    /// # Errors
    ///
    /// Returns `InvalidPointId` when the value is not a canonical UUID.
    pub fn parse(value: &str) -> Result<Self, RecoveryError> {
        let parsed = Uuid::parse_str(value).map_err(|_| RecoveryError::InvalidPointId)?;
        if parsed.to_string() != value {
            return Err(RecoveryError::InvalidPointId);
        }
        Ok(Self(value.to_owned()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for RecoveryPointId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoveryPoint {
    pub point_id: RecoveryPointId,
    pub created_at_ms: i64,
    pub critical_revision: u64,
    path: PathBuf,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RecoveryInventory {
    pub valid_points: Vec<RecoveryPoint>,
    pub invalid_point_count: usize,
}

/// Classification of one `pragma integrity_check` output.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IntegrityClassification {
    /// The database reported `ok`.
    Ok,
    /// Every reported message describes an index membership inconsistency.
    IndexOnly,
    /// Any other or unclassifiable message; never eligible for automatic repair.
    Unrecoverable,
}

/// Bounded `pragma integrity_check` messages with their classification.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IntegrityReport {
    pub messages: Vec<String>,
    pub classification: IntegrityClassification,
}

/// Classifies collected `pragma integrity_check` messages.
///
/// An empty report and the healthy `ok` row are [`IntegrityClassification::Ok`].
/// A report whose every message is an index membership inconsistency is
/// [`IntegrityClassification::IndexOnly`]; every other shape fails closed as
/// [`IntegrityClassification::Unrecoverable`].
#[must_use]
pub fn classify_integrity_output(messages: &[String]) -> IntegrityClassification {
    let mut index_only = false;
    for message in messages {
        let message = message.trim();
        if message.is_empty() || message.eq_ignore_ascii_case("ok") {
            continue;
        }
        if !is_index_only_message(message) {
            return IntegrityClassification::Unrecoverable;
        }
        index_only = true;
    }
    if index_only {
        IntegrityClassification::IndexOnly
    } else {
        IntegrityClassification::Ok
    }
}

/// Reads every bounded `pragma integrity_check` message of one connection.
///
/// A malformed database can abort the pragma after reporting the pages it
/// already examined. Those messages are kept as evidence and the aborted check
/// fails closed as [`IntegrityClassification::Unrecoverable`].
///
/// # Errors
///
/// Returns the `SQLite` error when the pragma cannot be evaluated at all.
pub fn read_integrity_report(connection: &Connection) -> Result<IntegrityReport, rusqlite::Error> {
    let mut messages = Vec::new();
    let query = connection.pragma_query(None, "integrity_check", |row| {
        messages.push(row.get::<_, String>(0)?);
        Ok(())
    });
    if let Err(error) = query {
        if messages.is_empty() {
            return Err(error);
        }
        return Ok(IntegrityReport {
            messages: incident::bound_integrity_messages(&messages),
            classification: IntegrityClassification::Unrecoverable,
        });
    }
    let messages = incident::bound_integrity_messages(&messages);
    let classification = classify_integrity_output(&messages);
    Ok(IntegrityReport {
        messages,
        classification,
    })
}

fn is_index_only_message(message: &str) -> bool {
    if let Some(rest) = message.strip_prefix("row ") {
        let Some((row, index)) = rest.split_once(" missing from index ") else {
            return false;
        };
        return is_row_id(row) && is_index_name(index);
    }
    if let Some(index) = message.strip_prefix("wrong # of entries in index ") {
        return is_index_name(index);
    }
    // SQLite 3.53 reports a rebuildable index-cell mismatch this way; the
    // message names one index and one row, so `REINDEX` can rebuild the cell.
    if let Some(rest) = message.strip_prefix("index ") {
        let Some((index, row)) =
            rest.split_once(" stores an imprecise floating-point value for row ")
        else {
            return false;
        };
        return is_index_name(index) && is_row_id(row);
    }
    false
}

fn is_row_id(row: &str) -> bool {
    !row.is_empty() && row.bytes().all(|byte| byte.is_ascii_digit())
}

fn is_index_name(index: &str) -> bool {
    !index.is_empty() && !index.chars().any(char::is_whitespace)
}

#[derive(Debug, Error)]
pub enum RecoveryError {
    #[error("invalid recovery point identifier")]
    InvalidPointId,
    #[error("unsafe recovery filesystem object")]
    UnsafeFilesystemObject,
    #[error("recovery point is invalid")]
    InvalidPoint,
    #[error("recovery point uses a future schema")]
    FutureSchema,
    #[error("recovery database table policy is incomplete")]
    UnknownTable,
    #[error("recovery database domain validation failed")]
    DomainValidation,
    #[error("database directory is already in use")]
    DirectoryInUse,
    #[error("recovery filesystem operation failed")]
    Filesystem(#[from] io::Error),
    #[error("recovery sqlite operation failed")]
    Database(#[from] rusqlite::Error),
    #[error("live database operation failed")]
    Storage(#[from] StorageError),
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum RecoveryHealthKind {
    Protected,
    Updating,
    Degraded,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoveryHealth {
    pub kind: RecoveryHealthKind,
    pub latest_success_at_ms: Option<i64>,
    pub valid_point_count: usize,
    pub live_critical_revision: u64,
    pub covered_critical_revision: Option<u64>,
    pub last_failure: Option<RecoveryFailureCode>,
    /// Newest persisted corruption incident, when incident recording is enabled.
    pub last_incident: Option<IncidentRecord>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecoveryFailureCode {
    PublicationFailed,
    InventoryUnavailable,
    IntegrityUnrecoverable,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum DatabaseStartupIssue {
    Permission,
    DiskFull,
    FutureSchema,
    UnsafePath,
    Unavailable,
    DirectoryInUse,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DatabaseStartupClassification {
    NewInstall,
    Ready,
    RecoveryRequired(RecoveryInventory),
    /// The primary exists and its only integrity failures are index
    /// inconsistencies, so a repair attempt may run before recovery.
    Repairable(RecoveryInventory, IntegrityReport),
    Fatal(DatabaseStartupIssue),
}

#[must_use]
pub fn classify_storage_startup_error(error: &StorageError) -> DatabaseStartupIssue {
    match error {
        StorageError::FutureSchema => DatabaseStartupIssue::FutureSchema,
        StorageError::Filesystem(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            DatabaseStartupIssue::Permission
        }
        StorageError::Database(rusqlite::Error::SqliteFailure(error, _)) => {
            use rusqlite::ErrorCode;
            match error.code {
                ErrorCode::PermissionDenied | ErrorCode::ReadOnly => {
                    DatabaseStartupIssue::Permission
                }
                ErrorCode::DiskFull => DatabaseStartupIssue::DiskFull,
                _ => DatabaseStartupIssue::Unavailable,
            }
        }
        _ => DatabaseStartupIssue::Unavailable,
    }
}

#[must_use]
pub fn classify_recovery_startup_error(error: &RecoveryError) -> Option<DatabaseStartupIssue> {
    match error {
        RecoveryError::UnsafeFilesystemObject => Some(DatabaseStartupIssue::UnsafePath),
        RecoveryError::FutureSchema => Some(DatabaseStartupIssue::FutureSchema),
        RecoveryError::DirectoryInUse => Some(DatabaseStartupIssue::DirectoryInUse),
        RecoveryError::Filesystem(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            Some(DatabaseStartupIssue::Permission)
        }
        RecoveryError::Filesystem(error) if error.raw_os_error() == Some(28) => {
            Some(DatabaseStartupIssue::DiskFull)
        }
        RecoveryError::Database(rusqlite::Error::SqliteFailure(error, _)) => {
            use rusqlite::ErrorCode;
            match error.code {
                ErrorCode::PermissionDenied | ErrorCode::ReadOnly => {
                    Some(DatabaseStartupIssue::Permission)
                }
                ErrorCode::DiskFull => Some(DatabaseStartupIssue::DiskFull),
                _ => None,
            }
        }
        RecoveryError::Storage(error) => Some(classify_storage_startup_error(error)),
        RecoveryError::InvalidPointId
        | RecoveryError::InvalidPoint
        | RecoveryError::UnknownTable
        | RecoveryError::DomainValidation
        | RecoveryError::Filesystem(_)
        | RecoveryError::Database(_) => None,
    }
}

pub trait RecoveryEventSink: Send + Sync {
    fn health_changed(&self, health: &RecoveryHealth);
    fn diagnostic(&self, code: RecoveryFailureCode);
}

#[derive(Default)]
pub struct NoopRecoveryEventSink;

impl RecoveryEventSink for NoopRecoveryEventSink {
    fn health_changed(&self, _health: &RecoveryHealth) {}

    fn diagnostic(&self, _code: RecoveryFailureCode) {}
}

/// Reports whether the proxy has been free of in-flight work for a duration.
///
/// The runtime injects this probe so an integrity self-check and its repair
/// only run while the desktop is idle.
pub trait RecoveryActivityProbe: Send + Sync {
    fn idle_for(&self, minimum: Duration) -> bool;
}

/// Default probe used by [`RecoveryCoordinator::start`].
///
/// Without an injected probe the coordinator never claims idleness, so the
/// existing start path keeps its behavior and never starts a self-check.
#[derive(Clone, Copy, Debug, Default)]
pub struct NeverIdleRecoveryActivityProbe;

impl RecoveryActivityProbe for NeverIdleRecoveryActivityProbe {
    fn idle_for(&self, _minimum: Duration) -> bool {
        false
    }
}

/// Cadence of the runtime integrity self-check.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RecoverySelfCheckSchedule {
    /// Delay between coordinator start and the first self-check.
    pub initial_delay: Duration,
    /// Delay between the end of one self-check and the next.
    pub interval: Duration,
    /// Traffic-free window the probe must observe before a self-check runs.
    pub idle_minimum: Duration,
}

impl Default for RecoverySelfCheckSchedule {
    fn default() -> Self {
        Self {
            initial_delay: RECOVERY_SELF_CHECK_INITIAL_DELAY,
            interval: RECOVERY_SELF_CHECK_INTERVAL,
            idle_minimum: RECOVERY_SELF_CHECK_IDLE_MINIMUM,
        }
    }
}

pub struct RecoveryCoordinator {
    context: RecoveryWorkerContext,
    worker: tokio::sync::Mutex<Vec<JoinHandle<()>>>,
    shutdown_sender: watch::Sender<bool>,
}

#[derive(Clone)]
struct RecoveryWorkerContext {
    manager: RecoveryManager,
    database: DatabaseExecutor,
    health: Arc<RwLock<RecoveryHealth>>,
    command_gate: Arc<tokio::sync::Mutex<()>>,
    sink: Arc<dyn RecoveryEventSink>,
    quiet_period: Duration,
}

impl RecoveryCoordinator {
    /// Builds recovery health, performs the required first publication when
    /// unprotected, and starts one coalescing worker.
    pub async fn start(
        manager: RecoveryManager,
        database: DatabaseExecutor,
        sink: Arc<dyn RecoveryEventSink>,
    ) -> Arc<Self> {
        Self::start_with_activity(
            manager,
            database,
            sink,
            Arc::new(NeverIdleRecoveryActivityProbe),
        )
        .await
    }

    /// Starts publication and the low-frequency integrity self-check.
    ///
    /// The self-check runs the full `pragma integrity_check` outside the
    /// database thread, and only while the supplied probe reports idleness for
    /// the schedule's minimum window.
    pub async fn start_with_activity(
        manager: RecoveryManager,
        database: DatabaseExecutor,
        sink: Arc<dyn RecoveryEventSink>,
        activity: Arc<dyn RecoveryActivityProbe>,
    ) -> Arc<Self> {
        Self::start_with_schedule(
            manager,
            database,
            sink,
            activity,
            RecoverySelfCheckSchedule::default(),
            RECOVERY_QUIET_PERIOD,
        )
        .await
    }

    /// Starts both workers with an explicit schedule and quiet period.
    pub async fn start_with_schedule(
        manager: RecoveryManager,
        database: DatabaseExecutor,
        sink: Arc<dyn RecoveryEventSink>,
        activity: Arc<dyn RecoveryActivityProbe>,
        schedule: RecoverySelfCheckSchedule,
        quiet_period: Duration,
    ) -> Arc<Self> {
        let initial = derive_health(&manager, &database, None).await;
        let health = Arc::new(RwLock::new(initial));
        let context = RecoveryWorkerContext {
            manager,
            database,
            health,
            command_gate: Arc::new(tokio::sync::Mutex::new(())),
            sink,
            quiet_period,
        };
        context.publish_health();
        if let Some(code) = context.health().last_failure {
            context.sink.diagnostic(code);
        }
        if context.health().kind == RecoveryHealthKind::Degraded {
            let _ = context.publish().await;
        }
        let (shutdown_sender, shutdown_receiver) = watch::channel(false);
        let coordinator = Arc::new(Self {
            context: context.clone(),
            worker: tokio::sync::Mutex::new(Vec::new()),
            shutdown_sender,
        });
        let worker = tokio::spawn(run_recovery_worker(
            context.clone(),
            shutdown_receiver.clone(),
        ));
        let self_check = tokio::spawn(run_recovery_self_check(
            context,
            shutdown_receiver,
            schedule,
            activity,
        ));
        *coordinator.worker.lock().await = vec![worker, self_check];
        coordinator
    }

    #[must_use]
    pub fn health(&self) -> RecoveryHealth {
        self.context.health()
    }

    /// Immediately creates a point through the serialized publication gate.
    ///
    /// # Errors
    ///
    /// Returns the real recovery publication error without changing committed state.
    pub async fn create_point(&self) -> Result<RecoveryHealth, RecoveryError> {
        self.context.publish().await
    }

    /// Stops notification intake and waits up to the supplied desktop budget.
    ///
    /// Returns `true` when every worker completed within the bound.
    pub async fn shutdown(&self, timeout: Duration) -> bool {
        self.shutdown_sender.send_replace(true);
        let workers = {
            let mut guard = self.worker.lock().await;
            std::mem::take(&mut *guard)
        };
        let mut completed = true;
        for mut worker in workers {
            if tokio::time::timeout(timeout, &mut worker).await.is_err() {
                worker.abort();
                completed = false;
            }
        }
        completed
    }
}

impl RecoveryWorkerContext {
    fn health(&self) -> RecoveryHealth {
        self.health
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn set_health(&self, health: &RecoveryHealth) {
        let changed = {
            let mut current = self
                .health
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if current.eq(health) {
                false
            } else {
                *current = health.clone();
                true
            }
        };
        if changed {
            self.sink.health_changed(health);
        }
    }

    fn publish_health(&self) {
        self.sink.health_changed(&self.health());
    }

    async fn publish(&self) -> Result<RecoveryHealth, RecoveryError> {
        let _gate = self.command_gate.lock().await;
        let before = self.health();
        let updating = RecoveryHealth {
            kind: RecoveryHealthKind::Updating,
            last_failure: None,
            ..before
        };
        self.set_health(&updating);
        match self.manager.create_point(&self.database).await {
            Ok(_) => {
                let health = derive_health(&self.manager, &self.database, None).await;
                self.set_health(&health);
                Ok(health)
            }
            Err(error) => {
                let mut health = derive_health(
                    &self.manager,
                    &self.database,
                    Some(RecoveryFailureCode::PublicationFailed),
                )
                .await;
                if health.covered_critical_revision.is_none()
                    && before.covered_critical_revision.is_some()
                {
                    health.latest_success_at_ms = before.latest_success_at_ms;
                    health.valid_point_count = before.valid_point_count;
                    health.covered_critical_revision = before.covered_critical_revision;
                    health.kind = RecoveryHealthKind::Degraded;
                }
                health.kind = RecoveryHealthKind::Degraded;
                self.set_health(&health);
                self.sink.diagnostic(RecoveryFailureCode::PublicationFailed);
                Err(error)
            }
        }
    }

    /// Runs one full integrity self-check and applies its recovery action.
    async fn run_self_check(&self) {
        let manager = self.manager.clone();
        let report = tokio::task::spawn_blocking(move || manager.inspect_primary_integrity()).await;
        let Ok(Ok(report)) = report else {
            return;
        };
        match report.classification {
            IntegrityClassification::Ok => {}
            IntegrityClassification::IndexOnly => {
                let manager = self.manager.clone();
                let database = self.database.clone();
                let outcome =
                    tokio::task::spawn_blocking(move || manager.repair_primary(&database)).await;
                if matches!(
                    outcome,
                    Ok(Ok(RepairOutcome {
                        recheck: RepairRecheck::Failed,
                        ..
                    }))
                ) {
                    let health = derive_health(
                        &self.manager,
                        &self.database,
                        Some(RecoveryFailureCode::IntegrityUnrecoverable),
                    )
                    .await;
                    self.set_health(&health);
                    self.sink
                        .diagnostic(RecoveryFailureCode::IntegrityUnrecoverable);
                    return;
                }
                let health = derive_health(&self.manager, &self.database, None).await;
                self.set_health(&health);
            }
            IntegrityClassification::Unrecoverable => {
                let manager = self.manager.clone();
                let messages = report.messages.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    let _ = manager.record_incident(
                        IncidentKind::Unrecoverable,
                        &messages,
                        IncidentAction::RecoveryRequired,
                        IncidentRecheck::Failed,
                        0,
                    );
                })
                .await;
                let health = derive_health(
                    &self.manager,
                    &self.database,
                    Some(RecoveryFailureCode::IntegrityUnrecoverable),
                )
                .await;
                self.set_health(&health);
                self.sink
                    .diagnostic(RecoveryFailureCode::IntegrityUnrecoverable);
            }
        }
    }
}

async fn run_recovery_self_check(
    context: RecoveryWorkerContext,
    mut shutdown: watch::Receiver<bool>,
    schedule: RecoverySelfCheckSchedule,
    activity: Arc<dyn RecoveryActivityProbe>,
) {
    if wait_for_shutdown(&mut shutdown, schedule.initial_delay).await {
        return;
    }
    loop {
        if *shutdown.borrow() {
            return;
        }
        if activity.idle_for(schedule.idle_minimum) {
            context.run_self_check().await;
        }
        if wait_for_shutdown(&mut shutdown, schedule.interval).await {
            return;
        }
    }
}

async fn wait_for_shutdown(shutdown: &mut watch::Receiver<bool>, duration: Duration) -> bool {
    tokio::select! {
        () = tokio::time::sleep(duration) => false,
        result = shutdown.changed() => result.is_err() || *shutdown.borrow(),
    }
}

async fn run_recovery_worker(context: RecoveryWorkerContext, mut shutdown: watch::Receiver<bool>) {
    let mut revisions = context.database.subscribe_critical_revisions();
    loop {
        tokio::select! {
            result = revisions.changed() => {
                if result.is_err() {
                    break;
                }
            }
            result = shutdown.changed() => {
                if result.is_err() || *shutdown.borrow() {
                    break;
                }
                continue;
            }
        }
        let health = derive_health(&context.manager, &context.database, None).await;
        context.set_health(&health);
        loop {
            let quiet = tokio::time::sleep(context.quiet_period);
            tokio::pin!(quiet);
            tokio::select! {
                () = &mut quiet => break,
                result = revisions.changed() => {
                    if result.is_err() {
                        return;
                    }
                }
                result = shutdown.changed() => {
                    if result.is_err() || *shutdown.borrow() {
                        return;
                    }
                }
            }
        }
        let _ = context.publish().await;
    }
}

async fn derive_health(
    manager: &RecoveryManager,
    database: &DatabaseExecutor,
    failure: Option<RecoveryFailureCode>,
) -> RecoveryHealth {
    let live_revision = database.critical_revision().await.unwrap_or_default();
    let last_incident = manager.latest_incident();
    match manager.scan() {
        Ok(inventory) => {
            let newest = inventory.valid_points.first();
            let covered = newest.map(|point| point.critical_revision);
            RecoveryHealth {
                kind: if covered.is_some_and(|revision| revision >= live_revision) {
                    RecoveryHealthKind::Protected
                } else {
                    RecoveryHealthKind::Degraded
                },
                latest_success_at_ms: newest.map(|point| point.created_at_ms),
                valid_point_count: inventory.valid_points.len(),
                live_critical_revision: live_revision,
                covered_critical_revision: covered,
                last_failure: failure,
                last_incident,
            }
        }
        Err(_) => RecoveryHealth {
            kind: RecoveryHealthKind::Degraded,
            latest_success_at_ms: None,
            valid_point_count: 0,
            live_critical_revision: live_revision,
            covered_critical_revision: None,
            last_failure: Some(failure.unwrap_or(RecoveryFailureCode::InventoryUnavailable)),
            last_incident,
        },
    }
}

/// Incident recording target for self-heal, quarantine, and recovery actions.
#[derive(Clone, Debug, Eq, PartialEq)]
struct RecoveryIncidentRecorder {
    directory: PathBuf,
    app_version: String,
}

/// Integrity facts captured before the primary is modified.
#[derive(Clone, Debug)]
struct PrimaryCorruptionEvidence {
    detected_at_ms: i64,
    kind: IncidentKind,
    messages: Vec<String>,
    db_bytes: u64,
    db_sha256: String,
}

/// One repair statement attempted on the primary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RepairAttempt {
    Reindex,
    Vacuum,
}

impl RepairAttempt {
    const fn statement(self) -> &'static str {
        match self {
            Self::Reindex => "REINDEX;",
            Self::Vacuum => "VACUUM;",
        }
    }
}

/// Result of the full validation that follows the last repair attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RepairRecheck {
    Ok,
    Failed,
}

/// Outcome of one repair entry point.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepairOutcome {
    /// Attempts actually executed, in order.
    pub attempts: Vec<RepairAttempt>,
    pub recheck: RepairRecheck,
    /// Pre-repair byte-for-byte copy kept as evidence, when one was staged.
    pub quarantine: Option<PathBuf>,
}

/// Exclusive advisory lock over one data directory.
///
/// The lock is released when this value is dropped or the process exits.
#[derive(Debug)]
pub struct RecoveryDirectoryLock {
    file: File,
    path: PathBuf,
}

impl RecoveryDirectoryLock {
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for RecoveryDirectoryLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

#[derive(Clone, Debug)]
pub struct RecoveryManager {
    primary_path: PathBuf,
    recovery_dir: PathBuf,
    incident_recorder: Option<RecoveryIncidentRecorder>,
    #[cfg(test)]
    injected_failures: Arc<std::sync::Mutex<BTreeSet<RecoveryFailurePoint>>>,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum RecoveryFailurePoint {
    BeforePrimaryPublish,
    AfterPrimaryPublish,
    BeforePrimaryRollback,
    RecoveryDirectoryDiskFull,
}

impl RecoveryManager {
    #[must_use]
    pub fn new(primary_path: impl Into<PathBuf>) -> Self {
        let primary_path = primary_path.into();
        let recovery_dir = primary_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("recovery");
        Self {
            primary_path,
            recovery_dir,
            incident_recorder: None,
            #[cfg(test)]
            injected_failures: Arc::new(std::sync::Mutex::new(BTreeSet::new())),
        }
    }

    #[must_use]
    pub fn recovery_dir(&self) -> &Path {
        &self.recovery_dir
    }

    #[must_use]
    pub fn primary_path(&self) -> &Path {
        &self.primary_path
    }

    /// Enables persisted incident recording in the runtime log directory.
    ///
    /// `app_version` is stored verbatim in every record so a later occurrence
    /// can be attributed to the build that wrote it.
    #[must_use]
    pub fn with_incident_recording(
        mut self,
        directory: impl Into<PathBuf>,
        app_version: impl Into<String>,
    ) -> Self {
        self.incident_recorder = Some(RecoveryIncidentRecorder {
            directory: directory.into(),
            app_version: app_version.into(),
        });
        self
    }

    /// Returns the configured incident directory, when recording is enabled.
    #[must_use]
    pub fn incident_directory(&self) -> Option<&Path> {
        self.incident_recorder
            .as_ref()
            .map(|recorder| recorder.directory.as_path())
    }

    /// Returns the newest persisted incident record, when one exists.
    ///
    /// Recording errors and unparsable records are treated as "no record" so
    /// the diagnostic projection never fails.
    #[must_use]
    pub fn latest_incident(&self) -> Option<IncidentRecord> {
        let recorder = self.incident_recorder.as_ref()?;
        incident::read_latest_incident(&recorder.directory)
            .ok()
            .flatten()
    }

    /// Acquires the exclusive data-directory lock held for the process lifetime.
    ///
    /// # Errors
    ///
    /// Returns [`RecoveryError::DirectoryInUse`] when another process already
    /// holds the lock, a filesystem error when the lock file cannot be created,
    /// or [`RecoveryError::UnsafeFilesystemObject`] for an unsafe path.
    pub fn acquire_directory_lock(&self) -> Result<RecoveryDirectoryLock, RecoveryError> {
        let parent = self
            .primary_path
            .parent()
            .ok_or(RecoveryError::UnsafeFilesystemObject)?;
        match fs::symlink_metadata(parent) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(RecoveryError::UnsafeFilesystemObject);
            }
            Ok(_) => {
                set_private_directory_mode(parent)?;
                ensure_private_directory(parent)?;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                fs::create_dir_all(parent)?;
                set_private_directory_mode(parent)?;
                ensure_private_directory(parent)?;
            }
            Err(error) => return Err(error.into()),
        }
        let name = self
            .primary_path
            .file_name()
            .ok_or(RecoveryError::UnsafeFilesystemObject)?
            .to_string_lossy()
            .into_owned();
        let path = parent.join(format!("{name}{DIRECTORY_LOCK_FILE_NAME_SUFFIX}"));
        match fs::symlink_metadata(&path) {
            Ok(metadata)
                if metadata.file_type().is_symlink() || !metadata.file_type().is_file() =>
            {
                return Err(RecoveryError::UnsafeFilesystemObject);
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)?;
        set_private_file_mode(&path)?;
        match file.try_lock() {
            Ok(()) => Ok(RecoveryDirectoryLock { file, path }),
            Err(fs::TryLockError::WouldBlock) => Err(RecoveryError::DirectoryInUse),
            Err(fs::TryLockError::Error(error)) => Err(error.into()),
        }
    }

    /// Reads the primary integrity report without mutating the database.
    ///
    /// # Errors
    ///
    /// Returns a filesystem error for an unsafe or missing primary, or a
    /// database error when the pragma cannot be evaluated.
    pub fn inspect_primary_integrity(&self) -> Result<IntegrityReport, RecoveryError> {
        ensure_private_regular_file(&self.primary_path)?;
        let connection = open_read_only_no_follow(&self.primary_path)?;
        configure_connection(&connection)?;
        read_integrity_report(&connection).map_err(RecoveryError::from)
    }

    /// Classifies startup without creating, migrating, or mutating the primary.
    ///
    /// # Errors
    ///
    /// Returns an error only when recovery discovery itself cannot be safely read.
    pub fn classify_startup(&self) -> Result<DatabaseStartupClassification, RecoveryError> {
        let inventory = self.scan()?;
        let recognized_artifacts = inventory.valid_points.len() + inventory.invalid_point_count;
        match fs::symlink_metadata(&self.primary_path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                if recognized_artifacts == 0 {
                    Ok(DatabaseStartupClassification::NewInstall)
                } else {
                    Ok(DatabaseStartupClassification::RecoveryRequired(inventory))
                }
            }
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => Ok(
                DatabaseStartupClassification::Fatal(DatabaseStartupIssue::Permission),
            ),
            Err(_) => Ok(DatabaseStartupClassification::Fatal(
                DatabaseStartupIssue::Unavailable,
            )),
            Ok(metadata)
                if metadata.file_type().is_symlink() || !metadata.file_type().is_file() =>
            {
                Ok(DatabaseStartupClassification::Fatal(
                    DatabaseStartupIssue::UnsafePath,
                ))
            }
            Ok(metadata) => {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if metadata.permissions().mode() & 0o777 != 0o600 {
                        return Ok(DatabaseStartupClassification::Fatal(
                            DatabaseStartupIssue::Permission,
                        ));
                    }
                }
                let started = Instant::now();
                let inspection = classify_existing_primary(&self.primary_path, inventory);
                if matches!(
                    inspection.classification,
                    DatabaseStartupClassification::RecoveryRequired(_)
                ) {
                    let _ = self.record_incident(
                        inspection.kind,
                        &inspection.messages,
                        IncidentAction::RecoveryRequired,
                        IncidentRecheck::Failed,
                        duration_millis(started.elapsed()),
                    );
                }
                Ok(inspection.classification)
            }
        }
    }

    /// Repairs an index-only corrupted primary that a database executor owns.
    ///
    /// The evidence copy, every attempt, and the post-repair recheck run on the
    /// executor's serialized connection, so no second writer touches the file.
    ///
    /// # Errors
    ///
    /// Returns a typed filesystem, database, or storage error for preconditions
    /// that must fall back to the existing recovery classification, such as an
    /// unsafe path, wrong permissions, a future schema, or a full disk.
    pub fn repair_primary(
        &self,
        database: &DatabaseExecutor,
    ) -> Result<RepairOutcome, RecoveryError> {
        self.repair_primary_with(
            |destination| {
                ensure_private_regular_file(&self.primary_path)?;
                let primary = self.primary_path.clone();
                let destination = destination.to_path_buf();
                database
                    .blocking_maintenance(move |_connection| {
                        Ok(copy_private_file_bytes(&primary, &destination)?)
                    })
                    .map_err(RecoveryError::from)
            },
            |attempt| {
                let statement = attempt.statement();
                database
                    .blocking_maintenance(move |connection| {
                        connection.execute_batch(statement)?;
                        Ok(())
                    })
                    .map_err(RecoveryError::from)
            },
        )
    }

    /// Repairs an index-only corrupted primary that no process has open.
    ///
    /// Used by startup classification, where `DatabaseExecutor::open` cannot
    /// succeed until the index inconsistency is resolved.
    ///
    /// # Errors
    ///
    /// Returns a typed filesystem, database, or storage error for preconditions
    /// that must fall back to the existing recovery classification.
    pub fn repair_primary_closed(&self) -> Result<RepairOutcome, RecoveryError> {
        self.repair_primary_with(
            |destination| copy_private_file(&self.primary_path, destination),
            |attempt| {
                let connection = open_read_write_no_follow(&self.primary_path)?;
                connection.busy_timeout(Duration::from_secs(5))?;
                connection.execute_batch(attempt.statement())?;
                Ok(())
            },
        )
    }

    fn repair_primary_with(
        &self,
        evidence: impl Fn(&Path) -> Result<(), RecoveryError>,
        mutate: impl Fn(RepairAttempt) -> Result<(), RecoveryError>,
    ) -> Result<RepairOutcome, RecoveryError> {
        ensure_private_regular_file(&self.primary_path)?;
        let started = Instant::now();
        let detected_at_ms = now_millis();
        let report = self.inspect_primary_integrity()?;
        let captured =
            self.capture_evidence(detected_at_ms, incident_kind(&report), &report.messages);
        if report.classification != IntegrityClassification::IndexOnly {
            if let Some(evidence) = &captured {
                self.write_incident_record(
                    evidence,
                    IncidentAction::RecoveryRequired,
                    IncidentRecheck::Failed,
                    duration_millis(started.elapsed()),
                );
            }
            return Ok(RepairOutcome {
                attempts: Vec::new(),
                recheck: RepairRecheck::Failed,
                quarantine: None,
            });
        }
        let quarantine = self.stage_repair_quarantine(&evidence)?;
        let mut attempts = Vec::new();
        let mut recheck = RepairRecheck::Failed;
        for attempt in [RepairAttempt::Reindex, RepairAttempt::Vacuum] {
            mutate(attempt)?;
            attempts.push(attempt);
            if self.recheck_primary().is_ok() {
                recheck = RepairRecheck::Ok;
                break;
            }
        }
        if recheck == RepairRecheck::Ok {
            self.apply_quarantine_retention(now_millis(), Some(&quarantine))?;
        }
        if let Some(evidence) = &captured {
            let (action, recheck_result) = match recheck {
                RepairRecheck::Ok => (IncidentAction::Repaired, IncidentRecheck::Ok),
                RepairRecheck::Failed => {
                    (IncidentAction::RecoveryRequired, IncidentRecheck::Failed)
                }
            };
            self.write_incident_record(
                evidence,
                action,
                recheck_result,
                duration_millis(started.elapsed()),
            );
        }
        Ok(RepairOutcome {
            attempts,
            recheck,
            quarantine: Some(quarantine),
        })
    }

    fn stage_repair_quarantine(
        &self,
        evidence: &impl Fn(&Path) -> Result<(), RecoveryError>,
    ) -> Result<PathBuf, RecoveryError> {
        self.ensure_recovery_dir()?;
        let quarantine = self.recovery_dir.join(format!(
            "quarantine-{}-{}.sqlite3",
            now_millis(),
            Uuid::new_v4()
        ));
        if let Err(error) = evidence(&quarantine) {
            let _ = fs::remove_file(&quarantine);
            return Err(error);
        }
        sync_directory(&self.recovery_dir)?;
        Ok(quarantine)
    }

    /// Runs the post-repair validation the open path can accept.
    ///
    /// A current-schema primary must satisfy the complete live-open contract.
    /// An older primary is checked for a non-future schema, integrity, and
    /// foreign keys only: the open path migrates it and re-runs the full
    /// contract, and a migration failure keeps the existing recovery flow.
    fn recheck_primary(&self) -> Result<(), RecoveryError> {
        ensure_private_regular_file(&self.primary_path)?;
        let connection = open_read_only_no_follow(&self.primary_path)?;
        configure_connection(&connection)?;
        let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version > SCHEMA_VERSION {
            return Err(RecoveryError::FutureSchema);
        }
        let integrity: String =
            connection.pragma_query_value(None, "integrity_check", |row| row.get(0))?;
        let foreign_key_violation: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_foreign_key_check)",
            [],
            |row| row.get(0),
        )?;
        if integrity != "ok" || foreign_key_violation {
            return Err(RecoveryError::DomainValidation);
        }
        if version == SCHEMA_VERSION {
            crate::storage::verify_connection(&connection)?;
            ensure_table_inventory(&connection)?;
            verify_domain(&connection)?;
        }
        Ok(())
    }

    fn capture_evidence(
        &self,
        detected_at_ms: i64,
        kind: IncidentKind,
        messages: &[String],
    ) -> Option<PrimaryCorruptionEvidence> {
        self.incident_recorder.as_ref()?;
        let metadata = fs::metadata(&self.primary_path).ok()?;
        let db_sha256 = incident::file_sha256(&self.primary_path).ok()?;
        Some(PrimaryCorruptionEvidence {
            detected_at_ms,
            kind,
            messages: messages.to_vec(),
            db_bytes: metadata.len(),
            db_sha256,
        })
    }

    /// Records one corruption incident for the primary, when recording is on.
    ///
    /// Returns the written path, or `None` when recording is disabled, the
    /// evidence cannot be read, or an identical record already exists.
    #[must_use]
    pub fn record_incident(
        &self,
        kind: IncidentKind,
        messages: &[String],
        action: IncidentAction,
        recheck: IncidentRecheck,
        duration_ms: i64,
    ) -> Option<PathBuf> {
        let evidence = self.capture_evidence(now_millis(), kind, messages)?;
        self.write_incident_record(&evidence, action, recheck, duration_ms)
    }

    fn write_incident_record(
        &self,
        evidence: &PrimaryCorruptionEvidence,
        action: IncidentAction,
        recheck: IncidentRecheck,
        duration_ms: i64,
    ) -> Option<PathBuf> {
        let recorder = self.incident_recorder.as_ref()?;
        let record = IncidentRecord {
            detected_at_ms: evidence.detected_at_ms,
            kind: evidence.kind,
            integrity_messages: evidence.messages.clone(),
            app_version: recorder.app_version.clone(),
            db_bytes: evidence.db_bytes,
            db_sha256: evidence.db_sha256.clone(),
            action,
            recheck,
            duration_ms,
        };
        if self
            .latest_incident()
            .is_some_and(|latest| same_incident(&latest, &record))
        {
            return None;
        }
        incident::write_incident(&recorder.directory, &record).ok()
    }

    /// Publishes a sanitized point from the existing single database executor.
    ///
    /// # Errors
    ///
    /// Returns a typed recovery, storage, database, validation, or filesystem error.
    pub async fn create_point(
        &self,
        database: &DatabaseExecutor,
    ) -> Result<RecoveryPoint, RecoveryError> {
        self.ensure_recovery_dir()?;
        #[cfg(test)]
        self.fail_if_injected(RecoveryFailurePoint::RecoveryDirectoryDiskFull)?;
        let point_id = RecoveryPointId::new();
        let created_at_ms = now_millis();
        let temporary_path = self
            .recovery_dir
            .join(format!(".point-{}.tmp", point_id.as_str()));
        let published_path = self
            .recovery_dir
            .join(point_filename(created_at_ms, &point_id));

        create_private_file(&temporary_path)?;
        if let Err(error) = database.backup_to(temporary_path.clone()).await {
            return Err(error.into());
        }
        sanitize_point(&temporary_path, &point_id, created_at_ms)?;
        sync_file(&temporary_path)?;
        fs::rename(&temporary_path, &published_path)?;
        sync_directory(&self.recovery_dir)?;

        let point = validate_point_file(&published_path)?;
        self.apply_point_retention(now_millis())?;
        Ok(point)
    }

    /// Scans recognized point files without opening or mutating the primary.
    ///
    /// # Errors
    ///
    /// Returns an error when the recovery directory itself is unsafe or unreadable.
    pub fn scan(&self) -> Result<RecoveryInventory, RecoveryError> {
        match fs::symlink_metadata(&self.recovery_dir) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(RecoveryInventory::default());
            }
            Err(error) => return Err(error.into()),
            Ok(_) => ensure_private_directory(&self.recovery_dir)?,
        }
        self.cleanup_recognized_temporaries()?;
        let mut valid_points = Vec::new();
        let mut invalid_point_count = 0;
        for entry in fs::read_dir(&self.recovery_dir)? {
            let entry = entry?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if parse_point_filename(name).is_none() {
                continue;
            }
            match validate_point_file(&entry.path()) {
                Ok(point) => valid_points.push(point),
                Err(_) => invalid_point_count += 1,
            }
        }
        valid_points.sort_by(|left, right| {
            right
                .created_at_ms
                .cmp(&left.created_at_ms)
                .then_with(|| right.point_id.cmp(&left.point_id))
        });
        Ok(RecoveryInventory {
            valid_points,
            invalid_point_count,
        })
    }

    /// Removes only recognized point temporaries left by an interrupted worker.
    ///
    /// # Errors
    ///
    /// Returns an error when a recognized object is unsafe or cannot be removed.
    pub fn cleanup_recognized_temporaries(&self) -> Result<usize, RecoveryError> {
        match fs::symlink_metadata(&self.recovery_dir) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
            Err(error) => return Err(error.into()),
            Ok(_) => ensure_private_directory(&self.recovery_dir)?,
        }
        let mut removed = cleanup_temporaries_in(&self.recovery_dir, is_point_temporary)?;
        let primary_parent = self
            .primary_path
            .parent()
            .ok_or(RecoveryError::UnsafeFilesystemObject)?;
        ensure_private_directory(primary_parent)?;
        let removed_primary = cleanup_temporaries_in(primary_parent, is_primary_temporary)?;
        removed += removed_primary;
        if removed_primary > 0 {
            sync_directory(primary_parent)?;
        }
        if removed > removed_primary {
            sync_directory(&self.recovery_dir)?;
        }
        Ok(removed)
    }

    /// Moves an unusable primary into one private quarantine and expires old ones.
    ///
    /// # Errors
    ///
    /// Returns an error when the primary is unsafe or publication cannot be synced.
    pub fn quarantine_primary(&self) -> Result<Option<PathBuf>, RecoveryError> {
        let started = Instant::now();
        let evidence = self.capture_evidence(now_millis(), IncidentKind::Unrecoverable, &[]);
        let quarantine = self.stage_quarantine_primary()?;
        if let Some(quarantine) = quarantine.as_deref() {
            if let Some(evidence) = &evidence {
                self.write_incident_record(
                    evidence,
                    IncidentAction::Quarantined,
                    IncidentRecheck::Failed,
                    duration_millis(started.elapsed()),
                );
            }
            self.apply_quarantine_retention(now_millis(), Some(quarantine))?;
        }
        Ok(quarantine)
    }

    fn stage_quarantine_primary(&self) -> Result<Option<PathBuf>, RecoveryError> {
        match fs::symlink_metadata(&self.primary_path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
            Ok(_) => {
                ensure_regular_file(&self.primary_path)?;
            }
        }
        self.ensure_recovery_dir()?;
        let quarantine = self.recovery_dir.join(format!(
            "quarantine-{}-{}.sqlite3",
            now_millis(),
            Uuid::new_v4()
        ));
        fs::rename(&self.primary_path, &quarantine)?;
        let staged = (|| {
            set_private_file_mode(&quarantine)?;
            sync_file(&quarantine)?;
            if let Some(parent) = self.primary_path.parent() {
                sync_directory(parent)?;
            }
            sync_directory(&self.recovery_dir)?;
            Ok::<(), RecoveryError>(())
        })();
        if let Err(error) = staged {
            let _ = fs::rename(&quarantine, &self.primary_path);
            return Err(error);
        }
        Ok(Some(quarantine))
    }

    /// Publishes a freshly validated copy of the selected immutable point.
    ///
    /// # Errors
    ///
    /// Returns a stale, validation, storage, or atomic publication error.
    pub fn restore_point(&self, point_id: &RecoveryPointId) -> Result<(), RecoveryError> {
        let inventory = self.scan()?;
        let point = inventory
            .valid_points
            .into_iter()
            .find(|point| &point.point_id == point_id)
            .ok_or(RecoveryError::InvalidPoint)?;
        self.publish_primary(Some(&point.path))
    }

    /// Publishes an empty current-schema database only when no valid point exists.
    ///
    /// # Errors
    ///
    /// Returns an error when a valid point exists or safe publication fails.
    pub fn start_over(&self) -> Result<(), RecoveryError> {
        if !self.scan()?.valid_points.is_empty() {
            return Err(RecoveryError::InvalidPoint);
        }
        let started = Instant::now();
        let evidence = self.capture_evidence(now_millis(), IncidentKind::Unrecoverable, &[]);
        self.publish_primary(None)?;
        if let Some(evidence) = &evidence {
            self.write_incident_record(
                evidence,
                IncidentAction::StartOver,
                IncidentRecheck::Failed,
                duration_millis(started.elapsed()),
            );
        }
        Ok(())
    }

    fn publish_primary(&self, source: Option<&Path>) -> Result<(), RecoveryError> {
        let parent = self
            .primary_path
            .parent()
            .ok_or(RecoveryError::UnsafeFilesystemObject)?;
        ensure_private_directory(parent)?;
        self.ensure_recovery_dir()?;
        let temporary = parent.join(format!(".primary-{}.tmp", Uuid::new_v4()));
        if let Some(source) = source {
            ensure_private_regular_file(source)?;
            copy_private_file(source, &temporary)?;
            DatabaseExecutor::migrate_and_validate_closed(&temporary)?;
        } else {
            DatabaseExecutor::create_validated(&temporary)?;
        }
        validate_live_database(&temporary)?;
        sync_file(&temporary)?;

        let quarantine = self.stage_quarantine_primary()?;
        let publish_result = (|| {
            #[cfg(test)]
            self.fail_if_injected(RecoveryFailurePoint::BeforePrimaryPublish)?;
            fs::rename(&temporary, &self.primary_path)?;
            #[cfg(test)]
            self.fail_if_injected(RecoveryFailurePoint::AfterPrimaryPublish)?;
            set_private_file_mode(&self.primary_path)?;
            sync_file(&self.primary_path)?;
            sync_directory(parent)?;
            Ok::<(), RecoveryError>(())
        })();
        if let Err(error) = publish_result {
            self.rollback_primary_publication(&temporary, quarantine.as_deref());
            return Err(error);
        }
        self.apply_quarantine_retention(now_millis(), quarantine.as_deref())?;
        Ok(())
    }

    fn rollback_primary_publication(&self, temporary: &Path, quarantine: Option<&Path>) {
        let Some(quarantine) = quarantine else {
            let _ = fs::remove_file(temporary);
            return;
        };
        let parent = self.primary_path.parent().unwrap_or_else(|| Path::new("."));
        let displaced_replacement = parent.join(format!(".primary-{}.tmp", Uuid::new_v4()));
        let replacement_was_published = self.primary_path.exists()
            && fs::rename(&self.primary_path, &displaced_replacement).is_ok();
        #[cfg(test)]
        let rollback_injected = self
            .fail_if_injected(RecoveryFailurePoint::BeforePrimaryRollback)
            .is_err();
        #[cfg(not(test))]
        let rollback_injected = false;
        let restored = !rollback_injected && fs::rename(quarantine, &self.primary_path).is_ok();
        if restored {
            let _ = fs::remove_file(&displaced_replacement);
            let _ = fs::remove_file(temporary);
            let _ = sync_directory(parent);
        } else if replacement_was_published {
            let _ = fs::rename(&displaced_replacement, &self.primary_path);
        }
    }

    #[cfg(test)]
    fn inject_failure(&self, point: RecoveryFailurePoint) {
        self.injected_failures
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(point);
    }

    #[cfg(test)]
    fn fail_if_injected(&self, point: RecoveryFailurePoint) -> Result<(), RecoveryError> {
        let injected = self
            .injected_failures
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&point);
        if !injected {
            return Ok(());
        }
        let error = if point == RecoveryFailurePoint::RecoveryDirectoryDiskFull {
            io::Error::from_raw_os_error(28)
        } else {
            io::Error::other("injected recovery failure")
        };
        Err(error.into())
    }

    fn ensure_recovery_dir(&self) -> Result<(), RecoveryError> {
        match fs::symlink_metadata(&self.recovery_dir) {
            Ok(_) => ensure_private_directory(&self.recovery_dir),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                fs::create_dir_all(&self.recovery_dir)?;
                set_private_directory_mode(&self.recovery_dir)?;
                ensure_private_directory(&self.recovery_dir)
            }
            Err(error) => Err(error.into()),
        }
    }

    fn apply_point_retention(&self, now_ms: i64) -> Result<(), RecoveryError> {
        let inventory = self.scan()?;
        if inventory.valid_points.is_empty() {
            return Ok(());
        }
        let cutoff = now_ms.saturating_sub(duration_millis(RECOVERY_RETENTION));
        let mut retained = BTreeSet::new();
        for (index, point) in inventory.valid_points.iter().enumerate() {
            if index == 0 || (retained.len() < MAX_VALID_POINTS && point.created_at_ms >= cutoff) {
                retained.insert(point.path.clone());
            }
        }
        let mut changed = false;
        for entry in fs::read_dir(&self.recovery_dir)? {
            let entry = entry?;
            let path = entry.path();
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if parse_point_filename(name).is_some() && !retained.contains(&path) {
                ensure_private_regular_file(&path)?;
                fs::remove_file(path)?;
                changed = true;
            }
        }
        if changed {
            sync_directory(&self.recovery_dir)?;
        }
        Ok(())
    }

    fn apply_quarantine_retention(
        &self,
        now_ms: i64,
        newest: Option<&Path>,
    ) -> Result<(), RecoveryError> {
        let cutoff = now_ms.saturating_sub(duration_millis(RECOVERY_RETENTION));
        let mut quarantines = Vec::new();
        for entry in fs::read_dir(&self.recovery_dir)? {
            let entry = entry?;
            let Some((created_at_ms, _)) = entry
                .file_name()
                .to_str()
                .and_then(parse_quarantine_filename)
            else {
                continue;
            };
            ensure_private_regular_file(&entry.path())?;
            quarantines.push((created_at_ms, entry.path()));
        }
        quarantines.sort_by_key(|item| std::cmp::Reverse(item.0));
        let keep = newest.map(Path::to_path_buf).or_else(|| {
            quarantines
                .first()
                .filter(|item| item.0 >= cutoff)
                .map(|item| item.1.clone())
        });
        let mut changed = false;
        for (_, path) in quarantines {
            if keep.as_ref() != Some(&path) {
                fs::remove_file(path)?;
                changed = true;
            }
        }
        if changed {
            sync_directory(&self.recovery_dir)?;
        }
        Ok(())
    }
}

/// Classification of one existing primary with its corruption evidence.
struct PrimaryInspection {
    classification: DatabaseStartupClassification,
    kind: IncidentKind,
    messages: Vec<String>,
}

fn classify_existing_primary(path: &Path, inventory: RecoveryInventory) -> PrimaryInspection {
    let connection = match open_read_only_no_follow(path) {
        Ok(connection) => connection,
        Err(error) => {
            return PrimaryInspection {
                classification: classify_primary_error(error, inventory),
                kind: IncidentKind::Unrecoverable,
                messages: Vec::new(),
            };
        }
    };
    let version: i64 = match connection.pragma_query_value(None, "user_version", |row| row.get(0)) {
        Ok(version) => version,
        Err(error) => {
            return PrimaryInspection {
                classification: classify_primary_error(error.into(), inventory),
                kind: IncidentKind::Unrecoverable,
                messages: Vec::new(),
            };
        }
    };
    if version > SCHEMA_VERSION {
        return PrimaryInspection {
            classification: DatabaseStartupClassification::Fatal(
                DatabaseStartupIssue::FutureSchema,
            ),
            kind: IncidentKind::Unrecoverable,
            messages: Vec::new(),
        };
    }
    let report = match read_integrity_report(&connection) {
        Ok(report) => report,
        Err(error) => {
            return PrimaryInspection {
                classification: classify_primary_error(error.into(), inventory),
                kind: IncidentKind::Unrecoverable,
                messages: Vec::new(),
            };
        }
    };
    if report.classification != IntegrityClassification::Ok {
        let classification = if report.classification == IntegrityClassification::IndexOnly {
            DatabaseStartupClassification::Repairable(inventory, report.clone())
        } else {
            DatabaseStartupClassification::RecoveryRequired(inventory)
        };
        return PrimaryInspection {
            classification,
            kind: incident_kind(&report),
            messages: report.messages,
        };
    }
    let outcome = (|| -> Result<(), RecoveryError> {
        if version == SCHEMA_VERSION {
            configure_connection(&connection)?;
            ensure_table_inventory(&connection)?;
            let foreign_key_violation: bool = connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM pragma_foreign_key_check)",
                [],
                |row| row.get(0),
            )?;
            if foreign_key_violation {
                return Err(RecoveryError::DomainValidation);
            }
            verify_domain(&connection)?;
        }
        Ok(())
    })();
    PrimaryInspection {
        classification: match outcome {
            Ok(()) => DatabaseStartupClassification::Ready,
            Err(error) => classify_primary_error(error, inventory),
        },
        kind: IncidentKind::Unrecoverable,
        messages: report.messages,
    }
}

fn classify_primary_error(
    error: RecoveryError,
    inventory: RecoveryInventory,
) -> DatabaseStartupClassification {
    match error {
        RecoveryError::FutureSchema => {
            DatabaseStartupClassification::Fatal(DatabaseStartupIssue::FutureSchema)
        }
        RecoveryError::UnsafeFilesystemObject => {
            DatabaseStartupClassification::Fatal(DatabaseStartupIssue::UnsafePath)
        }
        RecoveryError::Filesystem(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            DatabaseStartupClassification::Fatal(DatabaseStartupIssue::Permission)
        }
        RecoveryError::Database(rusqlite::Error::SqliteFailure(error, _)) => {
            use rusqlite::ErrorCode;
            match error.code {
                ErrorCode::PermissionDenied | ErrorCode::ReadOnly => {
                    DatabaseStartupClassification::Fatal(DatabaseStartupIssue::Permission)
                }
                ErrorCode::DiskFull => {
                    DatabaseStartupClassification::Fatal(DatabaseStartupIssue::DiskFull)
                }
                ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase => {
                    DatabaseStartupClassification::RecoveryRequired(inventory)
                }
                _ => DatabaseStartupClassification::Fatal(DatabaseStartupIssue::Unavailable),
            }
        }
        RecoveryError::InvalidPoint
        | RecoveryError::UnknownTable
        | RecoveryError::DomainValidation
        | RecoveryError::Database(_) => DatabaseStartupClassification::RecoveryRequired(inventory),
        _ => DatabaseStartupClassification::Fatal(DatabaseStartupIssue::Unavailable),
    }
}

fn validate_live_database(path: &Path) -> Result<(), RecoveryError> {
    ensure_private_regular_file(path)?;
    let connection = open_read_only_no_follow(path)?;
    configure_connection(&connection)?;
    ensure_schema_version(&connection)?;
    ensure_table_inventory(&connection)?;
    let report = read_integrity_report(&connection)?;
    let foreign_key_violation: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_foreign_key_check)",
        [],
        |row| row.get(0),
    )?;
    if report.classification != IntegrityClassification::Ok || foreign_key_violation {
        return Err(RecoveryError::DomainValidation);
    }
    verify_domain(&connection)
}

fn copy_private_file(source: &Path, destination: &Path) -> Result<(), RecoveryError> {
    copy_private_file_bytes(source, destination)?;
    Ok(())
}

fn copy_private_file_bytes(source: &Path, destination: &Path) -> io::Result<()> {
    let mut input = OpenOptions::new().read(true).open(source)?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    io::copy(&mut input, &mut output)?;
    set_private_file_mode(destination)?;
    output.sync_all()
}

fn sanitize_point(
    path: &Path,
    point_id: &RecoveryPointId,
    created_at_ms: i64,
) -> Result<(), RecoveryError> {
    ensure_private_regular_file(path)?;
    let mut connection = open_read_write_no_follow(path)?;
    configure_connection(&connection)?;
    ensure_schema_version(&connection)?;
    ensure_table_inventory(&connection)?;
    let transaction = connection.transaction()?;
    transaction.execute("DELETE FROM proxy_requests", [])?;
    transaction.execute(
        "UPDATE app_settings SET last_automatic_update_check_at_ms = NULL, mcp_image_capacity_warning_mib = 1024, mcp_image_capacity_active_episode = NULL, mcp_image_capacity_dismissed_episode = NULL WHERE singleton = 1",
        [],
    )?;
    let critical_revision: i64 = transaction.query_row(
        "SELECT critical_revision FROM recovery_revision WHERE singleton = 1",
        [],
        |row| row.get(0),
    )?;
    transaction.execute("DELETE FROM recovery_point_metadata", [])?;
    transaction.execute(
        "INSERT INTO recovery_point_metadata (singleton, format_version, point_id, created_at_ms, critical_revision) VALUES (1, ?1, ?2, ?3, ?4)",
        params![RECOVERY_FORMAT_VERSION, point_id.as_str(), created_at_ms, critical_revision],
    )?;
    transaction.commit()?;
    connection.execute_batch("VACUUM;")?;
    drop(connection);
    validate_point_contents(path, point_id, created_at_ms).map(|_| ())
}

fn validate_point_file(path: &Path) -> Result<RecoveryPoint, RecoveryError> {
    ensure_private_regular_file(path)?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(RecoveryError::InvalidPoint)?;
    let (filename_created_at, filename_id) =
        parse_point_filename(name).ok_or(RecoveryError::InvalidPoint)?;
    validate_point_contents(path, &filename_id, filename_created_at)
}

fn validate_point_contents(
    path: &Path,
    expected_id: &RecoveryPointId,
    expected_created_at: i64,
) -> Result<RecoveryPoint, RecoveryError> {
    ensure_private_regular_file(path)?;
    let connection = open_read_only_no_follow(path)?;
    configure_connection(&connection)?;
    ensure_schema_version(&connection)?;
    ensure_table_inventory(&connection)?;
    verify_database(&connection)?;
    verify_domain(&connection)?;
    let metadata: (i64, String, i64, i64) = connection
        .query_row(
            "SELECT format_version, point_id, created_at_ms, critical_revision FROM recovery_point_metadata WHERE singleton = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()?
        .ok_or(RecoveryError::InvalidPoint)?;
    let live_revision: i64 = connection.query_row(
        "SELECT critical_revision FROM recovery_revision WHERE singleton = 1",
        [],
        |row| row.get(0),
    )?;
    if metadata.0 != RECOVERY_FORMAT_VERSION
        || metadata.1 != expected_id.as_str()
        || metadata.2 != expected_created_at
        || metadata.3 < 0
        || metadata.3 > live_revision
    {
        return Err(RecoveryError::InvalidPoint);
    }
    Ok(RecoveryPoint {
        point_id: expected_id.clone(),
        created_at_ms: metadata.2,
        critical_revision: u64::try_from(metadata.3).map_err(|_| RecoveryError::InvalidPoint)?,
        path: path.to_path_buf(),
    })
}

fn verify_database(connection: &Connection) -> Result<(), RecoveryError> {
    let report = read_integrity_report(connection)?;
    let foreign_key_violation: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_foreign_key_check)",
        [],
        |row| row.get(0),
    )?;
    let history_count: i64 = connection.query_row(
        "SELECT (SELECT COUNT(*) FROM proxy_requests) + (SELECT COUNT(*) FROM upstream_attempts)",
        [],
        |row| row.get(0),
    )?;
    if report.classification != IntegrityClassification::Ok
        || foreign_key_violation
        || history_count != 0
    {
        return Err(RecoveryError::DomainValidation);
    }
    Ok(())
}

#[allow(
    clippy::too_many_lines,
    reason = "domain recovery validation intentionally audits the complete schema inventory"
)]
fn verify_domain(connection: &Connection) -> Result<(), RecoveryError> {
    let invalid_route_secret: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM routes r LEFT JOIN secrets s ON s.secret_id = r.secret_id AND s.kind = 'route_api_key' WHERE s.secret_id IS NULL)",
        [],
        |row| row.get(0),
    )?;
    let orphan_secret: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM secrets s WHERE (s.kind = 'route_api_key' AND NOT EXISTS(SELECT 1 FROM routes r WHERE r.secret_id = s.secret_id)) OR s.kind NOT IN ('route_api_key', 'gateway_token'))",
        [],
        |row| row.get(0),
    )?;
    let invalid_menu_visibility: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM routes WHERE menu_visible NOT IN (0, 1))",
        [],
        |row| row.get(0),
    )?;
    let gateway_count: i64 = connection.query_row(
        "SELECT COUNT(*) FROM secrets WHERE kind = 'gateway_token'",
        [],
        |row| row.get(0),
    )?;
    let (route_count, fallback_enabled, participant_count, config_revision): (i64, i64, i64, i64) =
        connection.query_row(
            "SELECT (SELECT COUNT(*) FROM routes), enabled, participant_count, config_revision
         FROM fallback_config WHERE singleton = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )?;
    let fallback_valid = matches!(fallback_enabled, 0 | 1)
        && config_revision >= 0
        && u32::try_from(participant_count).is_ok_and(|participant_count| {
            i64::from(participant_count) <= route_count
                && !(fallback_enabled == 1 && participant_count < 2)
        });
    let active_is_valid: bool = connection.query_row(
        "SELECT route_id IS NULL OR EXISTS(SELECT 1 FROM routes WHERE route_id = route_state.route_id) FROM route_state WHERE singleton = 1",
        [],
        |row| row.get(0),
    )?;
    let baseline_is_valid: bool = connection.query_row(
        "SELECT NOT EXISTS(SELECT 1 FROM codex_baseline WHERE
            (original_exists = 1 AND raw_bytes IS NULL)
            OR (original_exists = 0 AND (
                unix_mode IS NOT NULL
                OR (raw_bytes IS NOT NULL AND length(raw_bytes) != 0)
            )))",
        [],
        |row| row.get(0),
    )?;
    let recovery_is_valid: bool = connection.query_row(
        "SELECT NOT EXISTS(SELECT 1 FROM codex_recovery_config WHERE
            singleton != 1
            OR original_exists NOT IN (0, 1)
            OR (original_exists = 1 AND raw_bytes IS NULL)
            OR (original_exists = 0 AND (unix_mode IS NOT NULL OR (raw_bytes IS NOT NULL AND length(raw_bytes) != 0)))
        )",
        [],
        |row| row.get(0),
    )?;
    let codex_snapshots_are_paired: bool = connection.query_row(
        "SELECT (SELECT COUNT(*) FROM codex_baseline) =
                (SELECT COUNT(*) FROM codex_recovery_config)",
        [],
        |row| row.get(0),
    )?;
    let settings: AppSettingsDomainRow = connection.query_row(
        "SELECT proxy_port, menu_balance_debounce_seconds, automatic_balance_refresh_minutes, images_generation_enabled, images_generation_route_id, images_generation_timeout_secs, last_automatic_update_check_at_ms, menu_bar_status_text_enabled, menu_bar_activity_animation_enabled, outbound_proxy_enabled, outbound_proxy_url, images_generation_model FROM app_settings WHERE singleton = 1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?, row.get(6)?, row.get(7)?, row.get(8)?, row.get(9)?, row.get(10)?, row.get(11)?)),
    )?;
    let policy_valid = u16::try_from(settings.1)
        .ok()
        .zip(u16::try_from(settings.2).ok())
        .is_some_and(|(menu, automatic)| BalanceQueryPolicy::parse(menu, automatic).is_ok());
    let images_settings_valid = matches!(settings.3, 0 | 1)
        && settings.4.as_deref().is_none_or(|route_id| {
            connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM routes WHERE route_id = ?1)",
                    [route_id],
                    |row| row.get::<_, bool>(0),
                )
                .unwrap_or(false)
        })
        && u16::try_from(settings.5)
            .is_ok_and(|timeout| ImagesGenerationTimeout::parse(timeout).is_ok())
        && ImagesGenerationModel::parse(&settings.11).is_ok();
    let outbound_proxy_valid = match settings.9 {
        0 => settings
            .10
            .as_deref()
            .map(OutboundProxyUrl::parse)
            .transpose()
            .is_ok(),
        1 => settings
            .10
            .as_deref()
            .and_then(|url| OutboundProxyUrl::parse(url).ok())
            .and_then(|url| OutboundProxyConfig::new(true, Some(url)).ok())
            .is_some(),
        _ => false,
    };
    let capacity_settings: (i64, Option<String>, Option<String>) = connection.query_row(
        "SELECT mcp_image_capacity_warning_mib, mcp_image_capacity_active_episode, mcp_image_capacity_dismissed_episode FROM app_settings WHERE singleton = 1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    let episode_is_valid = |value: &str| {
        Uuid::parse_str(value).is_ok_and(|parsed| parsed.hyphenated().to_string() == value)
    };
    let capacity_settings_valid = u32::try_from(capacity_settings.0)
        .is_ok_and(|threshold| McpImageCapacityWarningThreshold::parse(threshold).is_ok())
        && capacity_settings.1.as_deref().is_none_or(episode_is_valid)
        && capacity_settings.2.as_deref().is_none_or(episode_is_valid)
        && capacity_settings
            .2
            .as_deref()
            .is_none_or(|dismissed| capacity_settings.1.as_deref() == Some(dismissed));
    let balance_queries_valid = {
        let mut statement =
            connection.prepare("SELECT mode, enabled, custom_source FROM balance_queries")?;
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .all(|(mode, enabled, custom_source)| {
                let Some(mode) = BalanceQueryMode::parse_persisted(&mode) else {
                    return false;
                };
                let enabled = match enabled {
                    0 => false,
                    1 => true,
                    _ => return false,
                };
                if mode == BalanceQueryMode::CustomJs && enabled && custom_source.trim().is_empty()
                {
                    return false;
                }
                custom_source.is_empty() || BalanceScriptSource::parse(&custom_source).is_ok()
            })
    };
    let codex_models_valid = {
        let mut statement = connection.prepare(
            "SELECT route_id, model_id, display_name, context_window, sort_order FROM codex_models ORDER BY route_id, sort_order",
        )?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let mut previous_route = None::<&str>;
        let mut route_index = 0_usize;
        rows.iter().all(
            |(route_id, model_id, display_name, context_window, sort_order)| {
                if previous_route != Some(route_id.as_str()) {
                    previous_route = Some(route_id.as_str());
                    route_index = 0;
                }
                let index = route_index;
                route_index = route_index.saturating_add(1);
                let Ok(context_window) = context_window.map(u64::try_from).transpose() else {
                    return false;
                };
                i64::try_from(index).is_ok_and(|index| *sort_order == index)
                    && CodexModel::parse(index, model_id, display_name.as_deref(), context_window)
                        .is_ok()
            },
        )
    };
    let fallback_excluded_models_valid = {
        let mut statement = connection.prepare(
            "SELECT route_id, model_id, sort_order
             FROM route_fallback_excluded_models ORDER BY route_id, sort_order",
        )?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let mut grouped = std::collections::BTreeMap::<String, Vec<(String, i64)>>::new();
        for (route_id, model_id, sort_order) in rows {
            grouped
                .entry(route_id)
                .or_default()
                .push((model_id, sort_order));
        }
        grouped.into_values().all(|rows| {
            let contiguous = rows.iter().enumerate().all(|(index, (_, sort_order))| {
                i64::try_from(index).is_ok_and(|index| *sort_order == index)
            });
            contiguous
                && crate::storage::normalize_fallback_excluded_models(
                    rows.into_iter().map(|(model, _)| model).collect(),
                )
                .is_ok()
        })
    };
    let notice_valid: bool = connection.query_row(
        "SELECT NOT EXISTS(
            SELECT 1 FROM codex_restart_notice n
            LEFT JOIN route_state s ON s.singleton = 1
            WHERE n.singleton != 1 OR n.selection_generation < 0
               OR length(n.notice_id) = 0
               OR length(n.catalog_fingerprint) = 0
               OR n.created_at_ms < 0
               OR s.route_id IS NOT n.route_id
               OR s.selection_generation != n.selection_generation
        )",
        [],
        |row| row.get(0),
    )?;
    if invalid_route_secret
        || orphan_secret
        || invalid_menu_visibility
        || gateway_count > 1
        || !fallback_valid
        || !active_is_valid
        || !baseline_is_valid
        || !recovery_is_valid
        || !codex_snapshots_are_paired
        || !(1..=65_535).contains(&settings.0)
        || !policy_valid
        || !images_settings_valid
        || !outbound_proxy_valid
        || !capacity_settings_valid
        || settings.6.is_some_and(|timestamp| timestamp < 0)
        || !matches!(settings.7, 0 | 1)
        || !matches!(settings.8, 0 | 1)
        || !balance_queries_valid
        || !codex_models_valid
        || !fallback_excluded_models_valid
        || !notice_valid
    {
        return Err(RecoveryError::DomainValidation);
    }
    Ok(())
}

fn ensure_schema_version(connection: &Connection) -> Result<(), RecoveryError> {
    let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version > SCHEMA_VERSION {
        return Err(RecoveryError::FutureSchema);
    }
    if version != SCHEMA_VERSION {
        return Err(RecoveryError::InvalidPoint);
    }
    Ok(())
}

fn ensure_table_inventory(connection: &Connection) -> Result<(), RecoveryError> {
    let mut statement = connection.prepare(
        "SELECT name FROM sqlite_schema WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
    )?;
    let actual = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<BTreeSet<_>, _>>()?;
    let expected = APPLICATION_TABLES
        .into_iter()
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    if actual != expected {
        return Err(RecoveryError::UnknownTable);
    }
    Ok(())
}

fn configure_connection(connection: &Connection) -> Result<(), RecoveryError> {
    connection.busy_timeout(Duration::from_secs(5))?;
    connection.pragma_update(None, "foreign_keys", true)?;
    Ok(())
}

fn open_read_only_no_follow(path: &Path) -> Result<Connection, RecoveryError> {
    let path = resolved_parent_path(path)?;
    Ok(Connection::open_with_flags(
        &path,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )?)
}

fn open_read_write_no_follow(path: &Path) -> Result<Connection, RecoveryError> {
    let path = resolved_parent_path(path)?;
    Ok(Connection::open_with_flags(
        &path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )?)
}

fn resolved_parent_path(path: &Path) -> Result<PathBuf, RecoveryError> {
    let parent = path
        .parent()
        .ok_or(RecoveryError::UnsafeFilesystemObject)?
        .canonicalize()?;
    let name = path
        .file_name()
        .ok_or(RecoveryError::UnsafeFilesystemObject)?;
    Ok(parent.join(name))
}

fn create_private_file(path: &Path) -> Result<(), RecoveryError> {
    let file = OpenOptions::new().write(true).create_new(true).open(path)?;
    set_private_file_mode(path)?;
    file.sync_all()?;
    ensure_private_regular_file(path)
}

fn ensure_private_directory(path: &Path) -> Result<(), RecoveryError> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        return Err(RecoveryError::UnsafeFilesystemObject);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o777 != 0o700 {
            return Err(RecoveryError::UnsafeFilesystemObject);
        }
    }
    Ok(())
}

fn ensure_private_regular_file(path: &Path) -> Result<(), RecoveryError> {
    let metadata = ensure_regular_file(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o777 != 0o600 {
            return Err(RecoveryError::UnsafeFilesystemObject);
        }
    }
    Ok(())
}

fn ensure_regular_file(path: &Path) -> Result<fs::Metadata, RecoveryError> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(RecoveryError::UnsafeFilesystemObject);
    }
    Ok(metadata)
}

#[cfg(unix)]
fn set_private_directory_mode(path: &Path) -> Result<(), io::Error> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn set_private_directory_mode(_path: &Path) -> Result<(), io::Error> {
    Ok(())
}

#[cfg(unix)]
fn set_private_file_mode(path: &Path) -> Result<(), io::Error> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn set_private_file_mode(_path: &Path) -> Result<(), io::Error> {
    Ok(())
}

fn sync_file(path: &Path) -> Result<(), io::Error> {
    OpenOptions::new().read(true).open(path)?.sync_all()
}

fn sync_directory(path: &Path) -> Result<(), io::Error> {
    File::open(path)?.sync_all()
}

fn point_filename(created_at_ms: i64, point_id: &RecoveryPointId) -> String {
    format!("point-{created_at_ms}-{}.sqlite3", point_id.as_str())
}

fn parse_point_filename(name: &str) -> Option<(i64, RecoveryPointId)> {
    let body = name.strip_prefix("point-")?.strip_suffix(".sqlite3")?;
    let (created, id) = body.split_once('-')?;
    let created_at_ms = created.parse().ok()?;
    let point_id = RecoveryPointId::parse(id).ok()?;
    Some((created_at_ms, point_id))
}

fn parse_quarantine_filename(name: &str) -> Option<(i64, Uuid)> {
    let body = name.strip_prefix("quarantine-")?.strip_suffix(".sqlite3")?;
    let (created, id) = body.split_once('-')?;
    Some((created.parse().ok()?, Uuid::parse_str(id).ok()?))
}

fn is_point_temporary(name: &str) -> bool {
    name.strip_prefix(".point-")
        .and_then(|body| body.strip_suffix(".tmp"))
        .is_some_and(|id| Uuid::parse_str(id).is_ok())
}

fn is_primary_temporary(name: &str) -> bool {
    name.strip_prefix(".primary-")
        .and_then(|body| body.strip_suffix(".tmp"))
        .is_some_and(|id| Uuid::parse_str(id).is_ok())
}

fn cleanup_temporaries_in(
    directory: &Path,
    is_recognized: fn(&str) -> bool,
) -> Result<usize, RecoveryError> {
    let mut removed = 0;
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !is_recognized(name) {
            continue;
        }
        ensure_private_regular_file(&entry.path())?;
        fs::remove_file(entry.path())?;
        removed += 1;
    }
    Ok(removed)
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or_default()
}

fn duration_millis(duration: Duration) -> i64 {
    i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
}

fn incident_kind(report: &IntegrityReport) -> IncidentKind {
    match report.classification {
        IntegrityClassification::IndexOnly => IncidentKind::IndexOnly,
        IntegrityClassification::Ok | IntegrityClassification::Unrecoverable => {
            IncidentKind::Unrecoverable
        }
    }
}

fn same_incident(latest: &IncidentRecord, candidate: &IncidentRecord) -> bool {
    latest.kind == candidate.kind
        && latest.action == candidate.action
        && latest.recheck == candidate.recheck
        && latest.db_sha256 == candidate.db_sha256
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        fs::OpenOptions,
        io::{Read, Seek, SeekFrom, Write},
        sync::{Arc, Mutex},
        time::Duration,
    };

    use rusqlite::{Connection, OpenFlags};
    use tempfile::TempDir;

    use super::{
        DatabaseStartupClassification, DatabaseStartupIssue, IntegrityClassification,
        MAX_VALID_POINTS, NeverIdleRecoveryActivityProbe, RECOVERY_QUIET_PERIOD,
        RecoveryActivityProbe, RecoveryCoordinator, RecoveryError, RecoveryEventSink,
        RecoveryFailureCode, RecoveryFailurePoint, RecoveryHealth, RecoveryHealthKind,
        RecoveryManager, RecoveryPointId, RecoverySelfCheckSchedule, RepairAttempt, RepairRecheck,
        classify_recovery_startup_error, create_private_file, sanitize_point,
    };
    use crate::{
        balance::BalanceQueryMode,
        domain::{
            ApiKey, CompletionState, DeliveryState, ImagesGenerationModel, ImagesGenerationTimeout,
            McpImageCapacityWarningThreshold, UpstreamAttemptId,
        },
        incident::{IncidentAction, IncidentKind, IncidentRecheck},
        storage::{
            AttemptHistoryRecord, BalanceQueryInput, CodexModelRecord, CreateRouteInput,
            DatabaseExecutor, RequestHistoryRecord, SCHEMA_VERSION,
        },
    };

    const EXCLUDED_SENTINEL: &str = "V02B_EXCLUDED_HISTORY_SENTINEL_7f3d";

    #[derive(Default)]
    struct RecordingRecoverySink {
        health: Mutex<Vec<RecoveryHealth>>,
        diagnostics: Mutex<Vec<RecoveryFailureCode>>,
    }

    impl RecoveryEventSink for RecordingRecoverySink {
        fn health_changed(&self, health: &RecoveryHealth) {
            self.health
                .lock()
                .expect("health lock")
                .push(health.clone());
        }

        fn diagnostic(&self, code: RecoveryFailureCode) {
            self.diagnostics.lock().expect("diagnostic lock").push(code);
        }
    }

    fn setup() -> (
        TempDir,
        std::path::PathBuf,
        DatabaseExecutor,
        RecoveryManager,
    ) {
        let root = tempfile::tempdir().expect("temporary root");
        let primary = root.path().join("data/router.sqlite3");
        let database = DatabaseExecutor::open(&primary).expect("database");
        let manager = RecoveryManager::new(&primary);
        (root, primary, database, manager)
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one recovery fixture seeds every critical and excluded-history boundary"
    )]
    async fn seed_critical_and_history(database: &DatabaseExecutor) {
        let route = database
            .create_route_with_models_and_fallback_exclusions(
                CreateRouteInput {
                    name: "Synthetic".to_owned(),
                    base_url: "https://example.invalid/v1".to_owned(),
                    api_key: ApiKey::parse("synthetic-route-key").expect("key"),
                    menu_visible: None,
                    balance_query: Some(BalanceQueryInput {
                        mode: BalanceQueryMode::CustomJs,
                        enabled: true,
                        custom_source: "({ request: {}, extractor: () => ({}) })".to_owned(),
                    }),
                    accept_script_risk: true,
                },
                Vec::new(),
                vec!["luna".to_owned(), "sol".to_owned()],
            )
            .await
            .expect("route");
        let timeout = ImagesGenerationTimeout::parse(900).expect("image timeout");
        database
            .set_images_generation_settings(
                true,
                Some(route.route_id.clone()),
                timeout,
                ImagesGenerationModel::parse("gpt-image-2.5-sunburst").expect("image model"),
            )
            .await
            .expect("image settings");
        database
            .get_or_create_singleton_secret(
                "gateway_token".to_owned(),
                ApiKey::parse("synthetic-gateway-token").expect("token"),
            )
            .await
            .expect("gateway token");
        database
            .capture_codex_baseline(true, b"synthetic codex baseline".to_vec(), Some(0o600))
            .await
            .expect("baseline");
        database
            .replace_codex_models(
                route.route_id.clone(),
                vec![
                    CodexModelRecord {
                        model_id: "relay-b".to_owned(),
                        display_name: Some("Relay B".to_owned()),
                        context_window: Some(200_000),
                    },
                    CodexModelRecord {
                        model_id: "relay-a".to_owned(),
                        display_name: None,
                        context_window: None,
                    },
                ],
            )
            .await
            .expect("Codex models");
        database
            .record_request_history(RequestHistoryRecord {
                request_id: "synthetic-request".to_owned(),
                started_at_ms: 1,
                finished_at_ms: 2,
                turn_id: None,
                requested_model: Some(EXCLUDED_SENTINEL.to_owned()),
                reasoning_effort: Some(EXCLUDED_SENTINEL.to_owned()),
                requested_service_tier: None,
                actual_model: None,
                actual_service_tier: None,
                final_route_id: Some(route.route_id.clone()),
                final_route_name: Some(route.name),
                streaming: true,
                completion_state: CompletionState::Failed,
                http_status: Some(502),
                error_category: Some(EXCLUDED_SENTINEL.to_owned()),
                input_tokens: None,
                output_tokens: None,
                total_tokens: None,
                cached_input_tokens: None,
                cache_write_input_tokens: None,
                total_latency_ms: Some(1),
                first_output_latency_ms: None,
                metadata_complete: true,
                fallback_stop_reason: None,
                fallback_stop_target_route_id: None,
                fallback_stop_target_route_name: None,
                attempts: vec![AttemptHistoryRecord {
                    attempt_id: UpstreamAttemptId::new(),
                    attempt_index: 0,
                    attempt_role: crate::storage::AttemptRole::Ordinary,
                    route_id: route.route_id,
                    route_name: EXCLUDED_SENTINEL.to_owned(),
                    started_at_ms: 1,
                    finished_at_ms: 2,
                    http_status: Some(502),
                    error_category: Some(EXCLUDED_SENTINEL.to_owned()),
                    delivery_state: DeliveryState::None,
                    actual_model: None,
                    forwarded_service_tier: None,
                    actual_service_tier: None,
                    input_tokens: None,
                    output_tokens: None,
                    total_tokens: None,
                    cached_input_tokens: None,
                    cache_write_input_tokens: None,
                }],
            })
            .await
            .expect("history");
    }

    async fn wait_until_protected(coordinator: &RecoveryCoordinator, revision: u64) {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let health = coordinator.health();
                if health.kind == RecoveryHealthKind::Protected
                    && health.covered_critical_revision == Some(revision)
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("coordinator becomes protected");
    }

    async fn start_test_coordinator(
        manager: RecoveryManager,
        database: DatabaseExecutor,
        sink: Arc<RecordingRecoverySink>,
        quiet_period: Duration,
    ) -> Arc<RecoveryCoordinator> {
        RecoveryCoordinator::start_with_schedule(
            manager,
            database,
            sink,
            Arc::new(NeverIdleRecoveryActivityProbe),
            RecoverySelfCheckSchedule::default(),
            quiet_period,
        )
        .await
    }

    #[tokio::test]
    async fn coordinator_attempts_initial_point_and_coalesces_latest_revision() {
        let (_root, _primary, database, manager) = setup();
        let sink = Arc::new(RecordingRecoverySink::default());
        let coordinator = start_test_coordinator(
            manager.clone(),
            database.clone(),
            sink.clone(),
            Duration::from_millis(60),
        )
        .await;
        assert_eq!(coordinator.health().kind, RecoveryHealthKind::Protected);
        assert_eq!(manager.scan().expect("initial point").valid_points.len(), 1);

        for index in 0..3 {
            database
                .create_route(CreateRouteInput {
                    name: format!("Route {index}"),
                    base_url: "https://example.invalid/v1".to_owned(),
                    api_key: ApiKey::parse(&format!("synthetic-key-{index}")).expect("key"),
                    menu_visible: None,
                    balance_query: None,
                    accept_script_risk: false,
                })
                .await
                .expect("critical mutation");
        }
        wait_until_protected(&coordinator, 3).await;
        let inventory = manager.scan().expect("coalesced points");
        assert_eq!(inventory.valid_points.len(), 2);
        assert_eq!(inventory.valid_points[0].critical_revision, 3);
        assert!(
            sink.health
                .lock()
                .expect("health lock")
                .iter()
                .any(|health| health.kind == RecoveryHealthKind::Degraded)
        );
        assert!(sink.diagnostics.lock().expect("diagnostics").is_empty());
        assert!(coordinator.shutdown(Duration::from_secs(1)).await);
    }

    #[tokio::test]
    async fn v13_point_remains_ineligible_after_distinct_v14_publication() {
        let (_root, _primary, database, manager) = setup();
        let legacy_point = manager
            .create_point(&database)
            .await
            .expect("initial point");
        let connection = Connection::open(&legacy_point.path).expect("legacy point");
        connection
            .pragma_update(None, "user_version", 13)
            .expect("mark point as v13");
        drop(connection);

        let before = manager.scan().expect("scan v13 point");
        assert!(before.valid_points.is_empty());
        assert_eq!(before.invalid_point_count, 1);

        let coordinator = start_test_coordinator(
            manager.clone(),
            database,
            Arc::new(RecordingRecoverySink::default()),
            Duration::from_millis(20),
        )
        .await;
        assert_eq!(coordinator.health().kind, RecoveryHealthKind::Protected);
        let after = manager.scan().expect("scan after v14 publication");
        assert_eq!(after.valid_points.len(), 1);
        assert_ne!(after.valid_points[0].point_id, legacy_point.point_id);
        let connection =
            Connection::open(&after.valid_points[0].path).expect("published v14 point");
        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("point version");
        assert_eq!(version, SCHEMA_VERSION);
        assert!(coordinator.shutdown(Duration::from_secs(1)).await);
    }

    #[tokio::test]
    async fn recovery_validation_rejects_invalid_menu_visibility() {
        let (_root, primary, database, manager) = setup();
        database
            .create_route(CreateRouteInput {
                name: "Visibility".to_owned(),
                base_url: "https://example.invalid/v1".to_owned(),
                api_key: ApiKey::parse("visibility-key").expect("key"),
                menu_visible: None,
                balance_query: None,
                accept_script_risk: false,
            })
            .await
            .expect("route");
        let point = manager.create_point(&database).await.expect("point");
        drop(database);
        tokio::time::sleep(Duration::from_millis(30)).await;

        for path in [&primary, &point.path] {
            let connection = Connection::open(path).expect("database to corrupt");
            connection
                .pragma_update(None, "ignore_check_constraints", true)
                .expect("bypass CHECK for corruption fixture");
            connection
                .execute("UPDATE routes SET menu_visible = 2", [])
                .expect("inject invalid visibility");
        }

        let inventory = manager.scan().expect("scan corrupt point");
        assert!(inventory.valid_points.is_empty());
        assert_eq!(inventory.invalid_point_count, 1);
        let DatabaseStartupClassification::RecoveryRequired(inventory) = manager
            .classify_startup()
            .expect("classify corrupt primary")
        else {
            panic!("an invalid persisted visibility must require recovery");
        };
        assert_eq!(inventory.invalid_point_count, 1);
    }

    #[tokio::test]
    async fn recovery_validation_rejects_invalid_image_timeout() {
        let (_root, primary, database, manager) = setup();
        let route = database
            .create_route(CreateRouteInput {
                name: "Images".to_owned(),
                base_url: "https://example.invalid/v1".to_owned(),
                api_key: ApiKey::parse("images-key").expect("key"),
                menu_visible: None,
                balance_query: None,
                accept_script_risk: false,
            })
            .await
            .expect("route");
        database
            .set_images_generation_settings(
                true,
                Some(route.route_id),
                ImagesGenerationTimeout::default(),
                ImagesGenerationModel::default(),
            )
            .await
            .expect("image settings");
        let point = manager.create_point(&database).await.expect("point");
        drop(database);
        tokio::time::sleep(Duration::from_millis(30)).await;

        for path in [&primary, &point.path] {
            let connection = Connection::open(path).expect("database to corrupt");
            connection
                .execute(
                    "UPDATE app_settings SET images_generation_timeout_secs = 599",
                    [],
                )
                .expect_err("timeout CHECK prevents direct corruption");
            connection
                .pragma_update(None, "ignore_check_constraints", true)
                .expect("bypass CHECK for corruption fixture");
            connection
                .execute(
                    "UPDATE app_settings SET images_generation_timeout_secs = 599",
                    [],
                )
                .expect("invalidate image timeout");
        }

        let inventory = manager.scan().expect("scan corrupt point");
        assert!(inventory.valid_points.is_empty());
        assert_eq!(inventory.invalid_point_count, 1);
        assert!(matches!(
            manager
                .classify_startup()
                .expect("classify corrupt primary"),
            DatabaseStartupClassification::RecoveryRequired(_)
        ));
    }

    #[tokio::test]
    async fn recovery_validation_rejects_control_character_image_model() {
        let (_root, primary, database, manager) = setup();
        database
            .set_images_generation_settings(
                false,
                None,
                ImagesGenerationTimeout::default(),
                ImagesGenerationModel::parse("gpt-image-2.5-flare").expect("model"),
            )
            .await
            .expect("image settings");
        let point = manager.create_point(&database).await.expect("point");
        drop(database);
        tokio::time::sleep(Duration::from_millis(30)).await;

        for path in [&primary, &point.path] {
            let connection = Connection::open(path).expect("database to corrupt");
            connection
                .execute("UPDATE app_settings SET images_generation_model = ''", [])
                .expect_err("length CHECK prevents an empty model");
            connection
                .execute(
                    "UPDATE app_settings SET images_generation_model = 'gpt-image' || char(10) || '2'",
                    [],
                )
                .expect("length CHECK cannot reject an interior control character");
        }

        let inventory = manager.scan().expect("scan corrupt point");
        assert!(inventory.valid_points.is_empty());
        assert_eq!(inventory.invalid_point_count, 1);
        assert!(matches!(
            manager
                .classify_startup()
                .expect("classify corrupt primary"),
            DatabaseStartupClassification::RecoveryRequired(_)
        ));
    }

    #[tokio::test]
    async fn recovery_validation_rejects_invalid_menu_bar_preferences() {
        for corrupt in [
            "UPDATE app_settings SET menu_bar_status_text_enabled = 2",
            "UPDATE app_settings SET menu_bar_activity_animation_enabled = 2",
        ] {
            let (_root, primary, database, manager) = setup();
            let point = manager.create_point(&database).await.expect("point");
            drop(database);
            tokio::time::sleep(Duration::from_millis(30)).await;

            for path in [&primary, &point.path] {
                let connection = Connection::open(path).expect("database to corrupt");
                connection
                    .pragma_update(None, "ignore_check_constraints", true)
                    .expect("bypass CHECK for corruption fixture");
                connection
                    .execute(corrupt, [])
                    .expect("invalidate menu bar preference");
            }

            let inventory = manager.scan().expect("scan corrupt point");
            assert!(inventory.valid_points.is_empty());
            assert_eq!(inventory.invalid_point_count, 1);
            assert!(matches!(
                manager
                    .classify_startup()
                    .expect("classify corrupt primary"),
                DatabaseStartupClassification::RecoveryRequired(_)
            ));
        }
    }

    #[tokio::test]
    async fn recovery_validation_rejects_invalid_outbound_proxy_settings() {
        for corrupt in [
            "UPDATE app_settings SET outbound_proxy_enabled = 2",
            "UPDATE app_settings SET outbound_proxy_enabled = 1, outbound_proxy_url = NULL",
            "UPDATE app_settings SET outbound_proxy_enabled = 1, outbound_proxy_url = 'http://synthetic-user:synthetic-secret@proxy.invalid:7890'",
        ] {
            let (_root, primary, database, manager) = setup();
            let point = manager.create_point(&database).await.expect("point");
            drop(database);
            tokio::time::sleep(Duration::from_millis(30)).await;

            for path in [&primary, &point.path] {
                let connection = Connection::open(path).expect("database to corrupt");
                connection
                    .pragma_update(None, "ignore_check_constraints", true)
                    .expect("bypass CHECK for corruption fixture");
                connection
                    .execute(corrupt, [])
                    .expect("invalidate outbound proxy setting");
            }

            let inventory = manager.scan().expect("scan corrupt point");
            assert!(inventory.valid_points.is_empty());
            assert_eq!(inventory.invalid_point_count, 1);
            assert!(matches!(
                manager
                    .classify_startup()
                    .expect("classify corrupt primary"),
                DatabaseStartupClassification::RecoveryRequired(_)
            ));
        }
    }

    #[tokio::test]
    async fn recovery_validation_rejects_invalid_capacity_settings() {
        for corrupt in [
            "UPDATE app_settings SET mcp_image_capacity_warning_mib = 127",
            "UPDATE app_settings SET mcp_image_capacity_active_episode = 'not-a-uuid'",
            "UPDATE app_settings SET mcp_image_capacity_dismissed_episode = '00000000-0000-0000-0000-000000000000'",
        ] {
            let (_root, primary, database, manager) = setup();
            let point = manager.create_point(&database).await.expect("point");
            drop(database);
            tokio::time::sleep(Duration::from_millis(30)).await;

            for path in [&primary, &point.path] {
                let connection = Connection::open(path).expect("database to corrupt");
                connection
                    .pragma_update(None, "ignore_check_constraints", true)
                    .expect("bypass CHECK for corruption fixture");
                connection
                    .execute(corrupt, [])
                    .expect("invalidate capacity setting");
            }

            let inventory = manager.scan().expect("scan corrupt point");
            assert!(inventory.valid_points.is_empty());
            assert_eq!(inventory.invalid_point_count, 1);
            assert!(matches!(
                manager
                    .classify_startup()
                    .expect("classify corrupt primary"),
                DatabaseStartupClassification::RecoveryRequired(_)
            ));
        }
    }

    #[tokio::test]
    async fn recovery_validation_rejects_invalid_fallback_participant_boundaries() {
        for corrupt in [
            "UPDATE fallback_config SET participant_count = -1",
            "UPDATE fallback_config SET participant_count = 2",
            "UPDATE fallback_config SET enabled = 1, participant_count = 1",
        ] {
            let (_root, primary, database, manager) = setup();
            database
                .create_route(CreateRouteInput {
                    name: "Only".to_owned(),
                    base_url: "https://example.invalid/v1".to_owned(),
                    api_key: ApiKey::parse("only-key").expect("key"),
                    menu_visible: None,
                    balance_query: None,
                    accept_script_risk: false,
                })
                .await
                .expect("route");
            let point = manager
                .create_point(&database)
                .await
                .expect("recovery point");
            drop(database);
            tokio::time::sleep(Duration::from_millis(30)).await;

            for path in [&primary, &point.path] {
                let connection = Connection::open(path).expect("database to corrupt");
                connection
                    .pragma_update(None, "ignore_check_constraints", true)
                    .expect("bypass CHECK for corruption fixture");
                connection
                    .execute(corrupt, [])
                    .expect("inject invalid fallback boundary");
            }

            let inventory = manager.scan().expect("scan invalid boundary point");
            assert!(inventory.valid_points.is_empty());
            assert_eq!(inventory.invalid_point_count, 1);
            let DatabaseStartupClassification::RecoveryRequired(inventory) =
                manager.classify_startup().expect("classify boundary")
            else {
                panic!("an invalid fallback boundary must require recovery");
            };
            assert_eq!(inventory.invalid_point_count, 1);
        }
    }

    #[tokio::test]
    async fn recovery_validation_rejects_invalid_balance_query_domain() {
        for corrupt in [
            "UPDATE balance_queries SET mode = 'unsupported'",
            "UPDATE balance_queries SET enabled = 2",
            "UPDATE balance_queries SET mode = 'custom_js', enabled = 1, custom_source = ''",
            "UPDATE balance_queries SET mode = 'custom_js', enabled = 1, custom_source = printf('%262145s', 'x')",
        ] {
            let (_root, primary, database, manager) = setup();
            database
                .create_route(CreateRouteInput {
                    name: "Balance".to_owned(),
                    base_url: "https://example.invalid/v1".to_owned(),
                    api_key: ApiKey::parse("balance-key").expect("key"),
                    menu_visible: None,
                    balance_query: Some(BalanceQueryInput {
                        mode: BalanceQueryMode::GeneralV1,
                        enabled: true,
                        custom_source: String::new(),
                    }),
                    accept_script_risk: false,
                })
                .await
                .expect("route");
            drop(database);

            let connection = Connection::open(&primary).expect("database to corrupt");
            connection
                .pragma_update(None, "ignore_check_constraints", true)
                .expect("bypass CHECK for corruption fixture");
            connection
                .execute(corrupt, [])
                .expect("inject invalid query");
            drop(connection);

            assert!(matches!(
                manager.classify_startup().expect("classify invalid query"),
                DatabaseStartupClassification::RecoveryRequired(_)
            ));
        }
    }

    #[tokio::test]
    async fn fallback_commit_does_not_wait_for_recovery_publication() {
        let (_root, _primary, database, manager) = setup();
        let first = database
            .create_route(CreateRouteInput {
                name: "First".to_owned(),
                base_url: "https://example.invalid/v1".to_owned(),
                api_key: ApiKey::parse("first-key").expect("key"),
                menu_visible: None,
                balance_query: None,
                accept_script_risk: false,
            })
            .await
            .expect("first");
        let second = database
            .create_route(CreateRouteInput {
                name: "Second".to_owned(),
                base_url: "https://example.invalid/v1".to_owned(),
                api_key: ApiKey::parse("second-key").expect("key"),
                menu_visible: None,
                balance_query: None,
                accept_script_risk: false,
            })
            .await
            .expect("second");
        database.set_fallback_enabled(true).await.expect("fallback");
        let captured = database.routing_state().await.expect("routing state");
        let coordinator = start_test_coordinator(
            manager.clone(),
            database.clone(),
            Arc::new(RecordingRecoverySink::default()),
            Duration::from_millis(300),
        )
        .await;

        let changed = tokio::time::timeout(
            Duration::from_millis(100),
            database.conditional_activate_next(
                first.route_id,
                captured.selection_generation,
                captured.fallback.config_revision,
                second.route_id,
            ),
        )
        .await
        .expect("fallback commit does not await quiet period")
        .expect("fallback storage");
        assert!(changed);
        assert_eq!(
            manager.scan().expect("pre-worker point").valid_points[0].critical_revision,
            3
        );
        wait_until_protected(&coordinator, 4).await;
        assert!(coordinator.shutdown(Duration::from_secs(1)).await);
    }

    #[test]
    fn startup_classification_is_pre_create_and_distinguishes_recoverable_and_fatal_states() {
        let root = tempfile::tempdir().expect("root");
        let primary = root.path().join("data/router.sqlite3");
        fs::create_dir_all(primary.parent().expect("parent")).expect("parent");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(
                primary.parent().expect("parent"),
                fs::Permissions::from_mode(0o700),
            )
            .expect("private parent");
        }
        let manager = RecoveryManager::new(&primary);
        assert_eq!(
            manager.classify_startup().expect("new install"),
            DatabaseStartupClassification::NewInstall
        );
        assert!(!primary.exists());

        fs::write(&primary, b"not a sqlite database").expect("corrupt primary");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&primary, fs::Permissions::from_mode(0o600))
                .expect("private primary");
        }
        assert!(matches!(
            manager.classify_startup().expect("corrupt"),
            DatabaseStartupClassification::RecoveryRequired(_)
        ));

        fs::remove_file(&primary).expect("remove corrupt");
        let connection = Connection::open(&primary).expect("future database");
        let future_version = SCHEMA_VERSION + 1;
        connection
            .pragma_update(None, "user_version", future_version)
            .expect("future schema");
        drop(connection);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&primary, fs::Permissions::from_mode(0o600))
                .expect("private future primary");
        }
        assert_eq!(
            manager.classify_startup().expect("future"),
            DatabaseStartupClassification::Fatal(DatabaseStartupIssue::FutureSchema)
        );

        fs::remove_file(&primary).expect("remove future primary");
        DatabaseExecutor::create_validated(&primary).expect("current database");
        let connection = Connection::open(&primary).expect("malformed current database");
        connection
            .execute("DELETE FROM app_settings", [])
            .expect("remove required singleton");
        drop(connection);
        assert!(matches!(
            manager.classify_startup().expect("invalid current domain"),
            DatabaseStartupClassification::RecoveryRequired(_)
        ));
    }

    #[tokio::test]
    #[expect(
        clippy::too_many_lines,
        reason = "one restore scenario verifies critical state, exclusions, and history removal"
    )]
    async fn restore_preserves_point_and_codex_file_while_dropping_history() {
        let (root, primary, database, manager) = setup();
        seed_critical_and_history(&database).await;
        for index in 0..2 {
            database
                .create_route(CreateRouteInput {
                    name: format!("Fallback {index}"),
                    base_url: "https://example.invalid/v1".to_owned(),
                    api_key: ApiKey::parse(&format!("fallback-key-{index}")).expect("key"),
                    menu_visible: None,
                    balance_query: None,
                    accept_script_risk: false,
                })
                .await
                .expect("fallback route");
        }
        database
            .set_fallback_participant_count(2)
            .await
            .expect("middle fallback boundary");
        database
            .set_fallback_enabled(true)
            .await
            .expect("enable fallback");
        database
            .set_menu_bar_settings(false, false)
            .await
            .expect("menu bar settings");
        let point = manager.create_point(&database).await.expect("point");
        let point_before = fs::read(&point.path).expect("point bytes");
        let codex = root.path().join("synthetic-codex-config.toml");
        fs::write(&codex, b"model_provider = \"synthetic\"\n").expect("codex config");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&codex, fs::Permissions::from_mode(0o640)).expect("codex mode");
        }
        let codex_before = fs::read(&codex).expect("codex bytes");
        #[cfg(unix)]
        let codex_mode_before = {
            use std::os::unix::fs::PermissionsExt;
            fs::metadata(&codex)
                .expect("codex metadata")
                .permissions()
                .mode()
        };

        drop(database);
        tokio::time::sleep(Duration::from_millis(30)).await;
        fs::write(&primary, b"corrupt primary").expect("corrupt primary");
        manager
            .restore_point(&point.point_id)
            .expect("restore point");

        assert_eq!(fs::read(&point.path).expect("point after"), point_before);
        assert_eq!(fs::read(&codex).expect("codex after"), codex_before);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&codex)
                    .expect("codex metadata")
                    .permissions()
                    .mode(),
                codex_mode_before
            );
        }
        let restored = DatabaseExecutor::open(&primary).expect("restored database");
        assert_eq!(restored.list_routes().await.expect("routes").len(), 3);
        let restored_settings = restored.app_settings().await.expect("app settings");
        assert!(restored_settings.images_generation_enabled);
        assert!(restored_settings.images_generation_route_id.is_some());
        assert_eq!(restored_settings.images_generation_timeout.seconds(), 900);
        assert_eq!(
            restored_settings.images_generation_model.as_str(),
            "gpt-image-2.5-sunburst"
        );
        assert!(!restored_settings.menu_bar.status_text_enabled);
        assert!(!restored_settings.menu_bar.activity_animation_enabled);
        let restored_routing = restored.routing_state().await.expect("routing state");
        assert!(restored_routing.fallback.enabled);
        assert_eq!(restored_routing.fallback.participant_count, 2);
        let restored_route = restored
            .list_routes()
            .await
            .expect("routes")
            .into_iter()
            .next()
            .expect("route");
        assert_eq!(
            restored
                .list_codex_models(restored_route.route_id.clone())
                .await
                .expect("Codex models")
                .iter()
                .map(|model| model.model_id.as_str())
                .collect::<Vec<_>>(),
            ["relay-b", "relay-a"]
        );
        assert_eq!(
            restored
                .list_fallback_excluded_models(restored_route.route_id)
                .await
                .expect("Fallback exclusions"),
            ["luna", "sol"]
        );
        assert_eq!(
            restored
                .history_summary()
                .await
                .expect("history")
                .request_count,
            0
        );
    }

    #[tokio::test]
    async fn startup_accepts_legacy_empty_absent_codex_baseline_only() {
        let (_root, primary, database, manager) = setup();
        database
            .capture_codex_baseline(false, Vec::new(), None)
            .await
            .expect("absent baseline");
        drop(database);

        let connection = Connection::open(&primary).expect("legacy database");
        connection
            .execute(
                "UPDATE codex_baseline SET raw_bytes = X'' WHERE singleton = 1",
                [],
            )
            .expect("legacy empty blob");
        drop(connection);
        assert_eq!(
            manager.classify_startup().expect("compatible baseline"),
            DatabaseStartupClassification::Ready
        );

        let connection = Connection::open(&primary).expect("invalid database");
        connection
            .execute(
                "UPDATE codex_baseline SET unix_mode = 384 WHERE singleton = 1",
                [],
            )
            .expect("invalid absent mode");
        drop(connection);
        assert!(matches!(
            manager.classify_startup().expect("invalid baseline mode"),
            DatabaseStartupClassification::RecoveryRequired(_)
        ));

        let connection = Connection::open(&primary).expect("invalid database");
        connection
            .execute(
                "UPDATE codex_baseline SET raw_bytes = X'01', unix_mode = NULL
                 WHERE singleton = 1",
                [],
            )
            .expect("invalid non-empty blob");
        drop(connection);
        assert!(matches!(
            manager.classify_startup().expect("invalid baseline"),
            DatabaseStartupClassification::RecoveryRequired(_)
        ));
    }

    #[tokio::test]
    async fn startup_rejects_a_baseline_without_its_disconnect_recovery_snapshot() {
        let (_root, primary, database, manager) = setup();
        database
            .capture_codex_baseline(true, b"model = \"original\"\n".to_vec(), Some(0o600))
            .await
            .expect("baseline and recovery");
        drop(database);

        let connection = Connection::open(&primary).expect("database");
        connection
            .execute("DELETE FROM codex_recovery_config", [])
            .expect("remove recovery snapshot");
        drop(connection);

        assert!(matches!(
            manager.classify_startup().expect("invalid snapshot pair"),
            DatabaseStartupClassification::RecoveryRequired(_)
        ));
    }

    #[tokio::test]
    async fn missing_primary_reports_mixed_candidates_and_stale_restore_preserves_primary() {
        let (_root, primary, database, manager) = setup();
        let point = manager.create_point(&database).await.expect("valid point");
        manager.ensure_recovery_dir().expect("recovery directory");
        let invalid = manager.recovery_dir().join(format!(
            "point-1-{}.sqlite3",
            RecoveryPointId::new().as_str()
        ));
        create_private_file(&invalid).expect("invalid point");
        fs::write(&invalid, b"not sqlite").expect("invalid bytes");

        drop(database);
        tokio::time::sleep(Duration::from_millis(30)).await;
        fs::remove_file(&primary).expect("remove primary");
        let DatabaseStartupClassification::RecoveryRequired(inventory) =
            manager.classify_startup().expect("missing classification")
        else {
            panic!("missing primary with recognized candidates must require recovery");
        };
        assert_eq!(inventory.valid_points.len(), 1);
        assert_eq!(inventory.invalid_point_count, 1);

        fs::write(&primary, b"unusable primary remains untouched").expect("unusable primary");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&primary, fs::Permissions::from_mode(0o600))
                .expect("private primary");
        }
        let primary_before = fs::read(&primary).expect("primary before stale restore");
        fs::remove_file(&point.path).expect("selected point disappears");
        assert!(matches!(
            manager.restore_point(&point.point_id),
            Err(RecoveryError::InvalidPoint)
        ));
        assert_eq!(
            fs::read(&primary).expect("primary after stale restore"),
            primary_before
        );
    }

    #[tokio::test]
    async fn disk_full_publication_is_typed_and_publishes_nothing() {
        let (_root, _primary, database, manager) = setup();
        manager.inject_failure(RecoveryFailurePoint::RecoveryDirectoryDiskFull);
        let error = manager
            .create_point(&database)
            .await
            .expect_err("injected disk full");
        assert_eq!(
            classify_recovery_startup_error(&error),
            Some(DatabaseStartupIssue::DiskFull)
        );
        assert!(manager.scan().expect("inventory").valid_points.is_empty());
    }

    #[tokio::test]
    async fn failed_publish_restores_primary_and_defers_old_quarantine_retention() {
        let (_root, primary, database, manager) = setup();
        seed_critical_and_history(&database).await;
        let point = manager.create_point(&database).await.expect("point");
        let point_before = fs::read(&point.path).expect("point bytes");
        drop(database);
        tokio::time::sleep(Duration::from_millis(30)).await;

        let old_quarantine = manager
            .quarantine_primary()
            .expect("old quarantine")
            .expect("old quarantine path");
        fs::write(&primary, b"current unusable primary").expect("current primary");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&primary, fs::Permissions::from_mode(0o600))
                .expect("private primary");
        }
        manager.inject_failure(RecoveryFailurePoint::BeforePrimaryPublish);
        assert!(manager.restore_point(&point.point_id).is_err());
        assert_eq!(
            fs::read(&primary).expect("rolled-back primary"),
            b"current unusable primary"
        );
        assert!(old_quarantine.exists());
        assert_eq!(
            fs::read(&point.path).expect("point after failure"),
            point_before
        );

        manager
            .restore_point(&point.point_id)
            .expect("retry restore succeeds");
        assert!(!old_quarantine.exists());
        let quarantines = fs::read_dir(manager.recovery_dir())
            .expect("recovery directory")
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("quarantine-")
            })
            .count();
        assert_eq!(quarantines, 1);
        assert_eq!(
            fs::read(&point.path).expect("point after retry"),
            point_before
        );
    }

    #[tokio::test]
    async fn failed_post_publish_rollback_keeps_a_valid_primary_and_recovery_point() {
        let (_root, primary, database, manager) = setup();
        seed_critical_and_history(&database).await;
        let point = manager.create_point(&database).await.expect("point");
        let point_before = fs::read(&point.path).expect("point bytes");
        drop(database);
        tokio::time::sleep(Duration::from_millis(30)).await;
        fs::write(&primary, b"unusable primary").expect("unusable primary");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&primary, fs::Permissions::from_mode(0o600))
                .expect("private primary");
        }

        manager.inject_failure(RecoveryFailurePoint::AfterPrimaryPublish);
        manager.inject_failure(RecoveryFailurePoint::BeforePrimaryRollback);
        assert!(manager.restore_point(&point.point_id).is_err());
        assert_eq!(fs::read(&point.path).expect("point retained"), point_before);
        assert_eq!(
            manager.classify_startup().expect("retry classification"),
            DatabaseStartupClassification::Ready
        );
    }

    #[tokio::test]
    async fn start_over_requires_no_valid_point_and_publishes_valid_empty_primary() {
        let root = tempfile::tempdir().expect("root");
        let primary = root.path().join("data/router.sqlite3");
        fs::create_dir_all(primary.parent().expect("parent")).expect("parent");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(
                primary.parent().expect("parent"),
                fs::Permissions::from_mode(0o700),
            )
            .expect("private parent");
        }
        fs::write(&primary, b"corrupt primary").expect("corrupt");
        let manager = RecoveryManager::new(&primary);
        manager.start_over().expect("start over");
        let database = DatabaseExecutor::open(&primary).expect("empty database");
        assert!(database.list_routes().await.expect("routes").is_empty());
    }

    #[tokio::test]
    async fn point_preserves_critical_state_and_removes_history_rows_and_raw_bytes() {
        let (_root, _primary, database, manager) = setup();
        seed_critical_and_history(&database).await;
        database
            .set_last_automatic_update_check_at_ms(1_725_000_000_000)
            .await
            .expect("update cadence");
        let threshold = McpImageCapacityWarningThreshold::parse(512).expect("threshold");
        let capacity = database
            .set_mcp_image_capacity_threshold(threshold, Some(threshold.bytes()))
            .await
            .expect("capacity episode");
        database
            .dismiss_mcp_image_capacity_warning(
                capacity
                    .active_episode_id
                    .as_deref()
                    .expect("active episode"),
            )
            .await
            .expect("dismiss capacity episode");

        let point = manager.create_point(&database).await.expect("point");
        let bytes = fs::read(&point.path).expect("point bytes");
        assert!(
            !bytes
                .windows(EXCLUDED_SENTINEL.len())
                .any(|window| { window == EXCLUDED_SENTINEL.as_bytes() })
        );
        let connection = Connection::open(&point.path).expect("point opens");
        let update_cadence: Option<i64> = connection
            .query_row(
                "SELECT last_automatic_update_check_at_ms FROM app_settings WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .expect("sanitized update cadence");
        assert_eq!(update_cadence, None);
        let capacity: (i64, Option<String>, Option<String>) = connection
            .query_row(
                "SELECT mcp_image_capacity_warning_mib, mcp_image_capacity_active_episode, mcp_image_capacity_dismissed_episode FROM app_settings WHERE singleton = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("sanitized capacity settings");
        assert_eq!(capacity, (1_024, None, None));
        let counts: (i64, i64, i64, i64, i64, i64, i64, i64, i64) = connection
            .query_row(
                "SELECT (SELECT COUNT(*) FROM routes), (SELECT COUNT(*) FROM secrets), (SELECT COUNT(*) FROM codex_baseline), (SELECT COUNT(*) FROM codex_recovery_config), (SELECT COUNT(*) FROM codex_models), (SELECT COUNT(*) FROM route_fallback_excluded_models), (SELECT COUNT(*) FROM proxy_requests), (SELECT COUNT(*) FROM upstream_attempts), (SELECT COUNT(*) FROM upstream_attempt_routing_skips)",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?, row.get(6)?, row.get(7)?, row.get(8)?)),
            )
            .expect("counts");
        assert_eq!(counts, (1, 2, 1, 1, 2, 2, 0, 0, 0));
        let query: (String, bool, String) = connection
            .query_row(
                "SELECT mode, enabled, custom_source FROM balance_queries",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("balance query");
        assert_eq!(
            query,
            (
                "custom_js".to_owned(),
                true,
                "({ request: {}, extractor: () => ({}) })".to_owned(),
            )
        );
        let participant_count: i64 = connection
            .query_row(
                "SELECT participant_count FROM fallback_config WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .expect("fallback participant count");
        assert_eq!(participant_count, 1);
        assert_eq!(point.critical_revision, 5);
        assert_eq!(manager.scan().expect("scan").valid_points.len(), 1);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(manager.recovery_dir())
                    .expect("dir")
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(&point.path)
                    .expect("point")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[tokio::test]
    async fn unknown_application_table_fails_closed_before_publication() {
        let (root, _primary, database, manager) = setup();
        let copy = root.path().join("copy.sqlite3");
        create_private_file(&copy).expect("copy file");
        database.backup_to(copy.clone()).await.expect("backup");
        Connection::open(&copy)
            .expect("copy")
            .execute("CREATE TABLE future_analytics (value TEXT)", [])
            .expect("future table");
        let result = sanitize_point(&copy, &RecoveryPointId::new(), 10);
        assert!(matches!(result, Err(RecoveryError::UnknownTable)));
        assert!(manager.scan().expect("scan").valid_points.is_empty());
    }

    #[tokio::test]
    async fn retention_keeps_five_valid_points_and_removes_recognized_invalid_after_publish() {
        let (_root, _primary, database, manager) = setup();
        let invalid = manager.recovery_dir().join(format!(
            "point-1-{}.sqlite3",
            RecoveryPointId::new().as_str()
        ));
        manager.ensure_recovery_dir().expect("private recovery dir");
        create_private_file(&invalid).expect("invalid file");
        OpenOptions::new()
            .append(true)
            .open(&invalid)
            .expect("invalid open")
            .write_all(b"not sqlite")
            .expect("invalid bytes");
        for _ in 0..=MAX_VALID_POINTS {
            manager.create_point(&database).await.expect("point");
        }
        let inventory = manager.scan().expect("inventory");
        assert_eq!(inventory.valid_points.len(), MAX_VALID_POINTS);
        assert_eq!(inventory.invalid_point_count, 0);
        assert!(!invalid.exists());
    }

    #[test]
    fn recognized_temporary_cleanup_preserves_unknown_files() {
        let (_root, primary, _database, manager) = setup();
        manager.ensure_recovery_dir().expect("private recovery dir");
        let point_temporary = manager
            .recovery_dir()
            .join(format!(".point-{}.tmp", RecoveryPointId::new().as_str()));
        let primary_temporary = primary
            .parent()
            .expect("primary parent")
            .join(format!(".primary-{}.tmp", RecoveryPointId::new().as_str()));
        let unknown = manager.recovery_dir().join("notes.txt");
        create_private_file(&point_temporary).expect("point temporary");
        create_private_file(&primary_temporary).expect("primary temporary");
        fs::write(&unknown, b"leave intact").expect("unknown");
        manager.scan().expect("scan cleans interrupted temporaries");
        assert!(!point_temporary.exists());
        assert!(!primary_temporary.exists());
        assert!(unknown.exists());
    }

    #[test]
    fn quarantine_keeps_only_the_newest_private_primary() {
        let root = tempfile::tempdir().expect("root");
        let primary = root.path().join("data/router.sqlite3");
        fs::create_dir_all(primary.parent().expect("parent")).expect("parent");
        let manager = RecoveryManager::new(&primary);
        fs::write(&primary, b"first unusable primary").expect("first primary");
        let first = manager
            .quarantine_primary()
            .expect("first quarantine")
            .expect("first path");
        fs::write(&primary, b"second unusable primary").expect("second primary");
        let second = manager
            .quarantine_primary()
            .expect("second quarantine")
            .expect("second path");
        assert_ne!(first, second);
        assert!(!first.exists());
        assert!(second.exists());
        let count = fs::read_dir(manager.recovery_dir())
            .expect("recovery dir")
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("quarantine-")
            })
            .count();
        assert_eq!(count, 1);
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_recovery_directory_fails_closed() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().expect("root");
        let primary = root.path().join("data/router.sqlite3");
        fs::create_dir_all(primary.parent().expect("parent")).expect("parent");
        let outside = root.path().join("outside");
        fs::create_dir(&outside).expect("outside");
        let recovery = primary.parent().expect("parent").join("recovery");
        symlink(&outside, &recovery).expect("symlink");
        let manager = RecoveryManager::new(primary);
        assert!(matches!(
            manager.scan(),
            Err(RecoveryError::UnsafeFilesystemObject)
        ));
        fs::remove_file(&recovery).expect("remove symlink");
        symlink(root.path().join("missing"), recovery).expect("broken symlink");
        assert!(matches!(
            manager.scan(),
            Err(RecoveryError::UnsafeFilesystemObject)
        ));
    }

    const KEYSET_INDEX: &str = "proxy_requests_keyset_idx";

    struct FixedActivityProbe(bool);

    impl RecoveryActivityProbe for FixedActivityProbe {
        fn idle_for(&self, _minimum: Duration) -> bool {
            self.0
        }
    }

    async fn record_synthetic_request(database: &DatabaseExecutor, index: i64) {
        database
            .record_request_history(RequestHistoryRecord {
                request_id: format!("synthetic-request-{index}"),
                started_at_ms: 1_700_000_000_000 + index,
                finished_at_ms: 1_700_000_001_000 + index,
                turn_id: None,
                requested_model: Some("synthetic-model".to_owned()),
                reasoning_effort: None,
                requested_service_tier: None,
                actual_model: None,
                actual_service_tier: None,
                final_route_id: None,
                final_route_name: None,
                streaming: false,
                completion_state: CompletionState::Completed,
                http_status: Some(200),
                error_category: None,
                input_tokens: Some(1),
                output_tokens: Some(1),
                total_tokens: Some(2),
                cached_input_tokens: None,
                cache_write_input_tokens: None,
                total_latency_ms: Some(10),
                first_output_latency_ms: Some(5),
                metadata_complete: true,
                fallback_stop_reason: None,
                fallback_stop_target_route_id: None,
                fallback_stop_target_route_name: None,
                attempts: Vec::new(),
            })
            .await
            .expect("history");
    }

    fn request_count(path: &std::path::Path) -> i64 {
        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .expect("read-only connection");
        connection
            .query_row("SELECT COUNT(*) FROM proxy_requests", [], |row| row.get(0))
            .expect("request count")
    }

    fn page_geometry(path: &std::path::Path, object: &str, name: &str) -> (u64, usize) {
        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .expect("read-only connection");
        let page_size: i64 = connection
            .pragma_query_value(None, "page_size", |row| row.get(0))
            .expect("page size");
        let root_page: i64 = connection
            .query_row(
                "SELECT rootpage FROM sqlite_schema WHERE type = ?1 AND name = ?2",
                [object, name],
                |row| row.get(0),
            )
            .expect("root page");
        (
            u64::try_from((root_page - 1) * page_size).expect("page offset"),
            usize::try_from(page_size).expect("page size"),
        )
    }

    /// Splices one b-tree root page from an older snapshot of the same database.
    ///
    /// This reproduces the 2026-09-07 shape: the index page is internally valid
    /// but comes from a previous generation, so cells for newer rows are absent
    /// while the table, file header, and page count stay intact.
    fn splice_root_page(primary: &std::path::Path, snapshot: &std::path::Path, index: &str) {
        let (offset, length) = page_geometry(snapshot, "index", index);
        let mut page = vec![0_u8; length];
        {
            let mut file = fs::File::open(snapshot).expect("snapshot file");
            file.seek(SeekFrom::Start(offset)).expect("snapshot seek");
            file.read_exact(&mut page).expect("snapshot page");
        }
        {
            let mut file = OpenOptions::new()
                .write(true)
                .open(primary)
                .expect("primary file");
            file.seek(SeekFrom::Start(offset)).expect("primary seek");
            file.write_all(&page).expect("primary page");
            file.sync_all().expect("primary sync");
        }
    }

    /// Overwrites the first bytes of one b-tree root page with an invalid type.
    fn damage_root_page_header(primary: &std::path::Path, object: &str, name: &str) {
        let (offset, _) = page_geometry(primary, object, name);
        let mut file = OpenOptions::new()
            .write(true)
            .open(primary)
            .expect("primary file");
        file.seek(SeekFrom::Start(offset)).expect("page seek");
        file.write_all(&[0x7f_u8; 16]).expect("damage page");
        file.sync_all().expect("primary sync");
    }

    /// Builds a closed primary whose keyset index root page comes from one row
    /// earlier, so the newest row is missing from that index.
    async fn build_index_only_corruption(root: &std::path::Path) -> std::path::PathBuf {
        let primary = root.join("data/router.sqlite3");
        let snapshot = root.join("snapshot.sqlite3");
        let database = DatabaseExecutor::open(&primary).expect("database");
        for index in 0..4 {
            record_synthetic_request(&database, index).await;
        }
        drop(database);
        fs::copy(&primary, &snapshot).expect("snapshot copy");
        let database = DatabaseExecutor::open(&primary).expect("database");
        record_synthetic_request(&database, 4).await;
        drop(database);
        splice_root_page(&primary, &snapshot, KEYSET_INDEX);
        primary
    }

    #[test]
    fn integrity_classification_allows_only_index_membership_messages() {
        let incident = vec![
            "index proxy_requests_model_keyset_idx stores an imprecise floating-point value for row 5195"
                .to_owned(),
            "row 5195 missing from index proxy_requests_model_keyset_idx".to_owned(),
            "row 5477 missing from index proxy_requests_model_keyset_idx".to_owned(),
            "row 118113 missing from index proxy_requests_keyset_idx".to_owned(),
            "wrong # of entries in index proxy_requests_route_keyset_idx".to_owned(),
        ];
        assert_eq!(
            super::classify_integrity_output(&incident),
            IntegrityClassification::IndexOnly
        );
        assert_eq!(
            super::classify_integrity_output(&["ok".to_owned()]),
            IntegrityClassification::Ok
        );
        assert_eq!(
            super::classify_integrity_output(&[]),
            IntegrityClassification::Ok
        );
        assert_eq!(
            super::classify_integrity_output(&[
                "ok".to_owned(),
                "row 1 missing from index any_idx".to_owned()
            ]),
            IntegrityClassification::IndexOnly
        );
        for message in [
            "Page 2: btreeInitPage() returns error code 11",
            "Page 5 is never used",
            "free space corruption at offset 100",
            "unable to get page 5 of 9",
            "database disk image is malformed",
            "row 5 missing from index",
            "row five missing from index any_idx",
            "row 5 missing from index any idx",
            "index any_idx stores an imprecise floating-point value for row",
            "index any idx stores an imprecise floating-point value for row 5",
            "integers any_idx stores an imprecise floating-point value for row 5",
            "<4 more messages truncated>",
        ] {
            assert_eq!(
                super::classify_integrity_output(&[message.to_owned()]),
                IntegrityClassification::Unrecoverable,
                "{message}"
            );
        }
    }

    #[tokio::test]
    async fn startup_classification_marks_index_only_corruption_repairable() {
        let root = tempfile::tempdir().expect("root");
        let primary = build_index_only_corruption(root.path()).await;
        let manager = RecoveryManager::new(&primary);
        match manager.classify_startup().expect("classification") {
            DatabaseStartupClassification::Repairable(inventory, report) => {
                assert_eq!(report.classification, IntegrityClassification::IndexOnly);
                assert!(!report.messages.is_empty());
                assert!(inventory.valid_points.is_empty());
            }
            other => panic!("expected a repairable primary, got {other:?}"),
        }
        assert!(
            DatabaseExecutor::open(&primary).is_err(),
            "the open path must still reject an index-inconsistent primary"
        );
    }

    #[tokio::test]
    async fn closed_repair_rebuilds_index_only_corruption_and_keeps_every_row() {
        let root = tempfile::tempdir().expect("root");
        let log_directory = root.path().join("logs");
        let primary = build_index_only_corruption(root.path()).await;

        let report = RecoveryManager::new(&primary)
            .inspect_primary_integrity()
            .expect("integrity report");
        assert_eq!(report.classification, IntegrityClassification::IndexOnly);
        assert!(
            report
                .messages
                .iter()
                .all(|message| message.contains("missing from index")),
            "{:?}",
            report.messages
        );

        let rows_before = request_count(&primary);
        assert_eq!(rows_before, 5);
        let bytes_before = fs::metadata(&primary).expect("metadata").len();
        let sha_before = crate::incident::file_sha256(&primary).expect("sha256");

        let manager =
            RecoveryManager::new(&primary).with_incident_recording(&log_directory, "0.4.2-test");
        let outcome = manager.repair_primary_closed().expect("repair outcome");
        assert_eq!(outcome.attempts, vec![RepairAttempt::Reindex]);
        assert_eq!(outcome.recheck, RepairRecheck::Ok);
        let quarantine = outcome.quarantine.expect("quarantine evidence");
        assert!(quarantine.exists());
        assert_eq!(
            fs::metadata(&quarantine)
                .expect("quarantine metadata")
                .len(),
            bytes_before
        );
        assert_eq!(
            crate::incident::file_sha256(&quarantine).expect("quarantine sha256"),
            sha_before
        );
        assert_eq!(request_count(&primary), rows_before);

        let recheck = manager
            .inspect_primary_integrity()
            .expect("recheck integrity");
        assert_eq!(recheck.classification, IntegrityClassification::Ok);

        let record = crate::incident::read_latest_incident(&log_directory)
            .expect("incident read")
            .expect("incident record");
        assert_eq!(record.kind, IncidentKind::IndexOnly);
        assert_eq!(record.action, IncidentAction::Repaired);
        assert_eq!(record.recheck, IncidentRecheck::Ok);
        assert_eq!(record.db_bytes, bytes_before);
        assert_eq!(record.db_sha256, sha_before);
        assert_eq!(record.app_version, "0.4.2-test");
        assert!(record.detected_at_ms > 0);
        assert!(!record.integrity_messages.is_empty());

        let quarantines = fs::read_dir(manager.recovery_dir())
            .expect("recovery dir")
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("quarantine-")
            })
            .count();
        assert_eq!(quarantines, 1);
    }

    #[tokio::test]
    async fn closed_repair_accepts_an_upgrade_pending_primary() {
        let root = tempfile::tempdir().expect("root");
        let primary = build_index_only_corruption(root.path()).await;
        // A primary that this binary must still migrate: the recheck may not
        // demand the current schema or its domain surface.
        let connection = Connection::open(&primary).expect("database");
        connection
            .pragma_update(None, "user_version", SCHEMA_VERSION - 1)
            .expect("pending upgrade version");
        drop(connection);

        let manager = RecoveryManager::new(&primary);
        assert!(matches!(
            manager.classify_startup().expect("classification"),
            DatabaseStartupClassification::Repairable(_, _)
        ));
        let outcome = manager.repair_primary_closed().expect("repair outcome");
        assert_eq!(outcome.attempts, vec![RepairAttempt::Reindex]);
        assert_eq!(outcome.recheck, RepairRecheck::Ok);

        let connection = Connection::open(&primary).expect("database");
        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("version");
        assert_eq!(version, SCHEMA_VERSION - 1, "repair must not migrate");
    }

    #[tokio::test]
    async fn repair_refuses_page_level_damage_without_modifying_the_primary() {
        let root = tempfile::tempdir().expect("root");
        let log_directory = root.path().join("logs");
        let primary = root.path().join("data/router.sqlite3");
        let database = DatabaseExecutor::open(&primary).expect("database");
        for index in 0..3 {
            record_synthetic_request(&database, index).await;
        }
        drop(database);
        damage_root_page_header(&primary, "table", "proxy_requests");
        let bytes_before = fs::read(&primary).expect("primary bytes");
        let sha_before = crate::incident::file_sha256(&primary).expect("sha256");

        let manager =
            RecoveryManager::new(&primary).with_incident_recording(&log_directory, "0.4.2-test");
        let report = manager
            .inspect_primary_integrity()
            .expect("integrity report");
        assert_eq!(
            report.classification,
            IntegrityClassification::Unrecoverable,
            "{:?}",
            report.messages
        );
        assert!(matches!(
            manager.classify_startup().expect("classification"),
            DatabaseStartupClassification::RecoveryRequired(_)
        ));

        let outcome = manager.repair_primary_closed().expect("repair outcome");
        assert!(outcome.attempts.is_empty());
        assert_eq!(outcome.recheck, RepairRecheck::Failed);
        assert_eq!(outcome.quarantine, None);
        assert_eq!(fs::read(&primary).expect("primary bytes"), bytes_before);
        assert_eq!(
            crate::incident::file_sha256(&primary).expect("sha256"),
            sha_before
        );
        assert!(!manager.recovery_dir().exists());
    }

    #[tokio::test]
    async fn startup_records_unrecoverable_corruption_once() {
        let root = tempfile::tempdir().expect("root");
        let log_directory = root.path().join("logs");
        let primary = root.path().join("data/router.sqlite3");
        let database = DatabaseExecutor::open(&primary).expect("database");
        record_synthetic_request(&database, 0).await;
        drop(database);
        damage_root_page_header(&primary, "table", "proxy_requests");

        let manager =
            RecoveryManager::new(&primary).with_incident_recording(&log_directory, "0.4.2-test");
        assert!(matches!(
            manager.classify_startup().expect("first classification"),
            DatabaseStartupClassification::RecoveryRequired(_)
        ));
        assert!(matches!(
            manager.classify_startup().expect("second classification"),
            DatabaseStartupClassification::RecoveryRequired(_)
        ));
        let records = fs::read_dir(&log_directory)
            .expect("log directory")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().starts_with("incident-"))
            .count();
        assert_eq!(records, 1, "an unchanged corruption records exactly once");
        let record = crate::incident::read_latest_incident(&log_directory)
            .expect("incident read")
            .expect("incident record");
        assert_eq!(record.kind, IncidentKind::Unrecoverable);
        assert_eq!(record.action, IncidentAction::RecoveryRequired);
        assert_eq!(record.recheck, IncidentRecheck::Failed);
    }

    #[tokio::test]
    async fn runtime_self_check_repairs_index_corruption_only_while_idle() {
        let root = tempfile::tempdir().expect("root");
        let log_directory = root.path().join("logs");
        let primary = root.path().join("data/router.sqlite3");
        let snapshot = root.path().join("snapshot.sqlite3");
        let database = DatabaseExecutor::open(&primary).expect("database");
        for index in 0..4 {
            record_synthetic_request(&database, index).await;
        }
        fs::copy(&primary, &snapshot).expect("snapshot copy");
        record_synthetic_request(&database, 4).await;
        splice_root_page(&primary, &snapshot, KEYSET_INDEX);

        let manager =
            RecoveryManager::new(&primary).with_incident_recording(&log_directory, "0.4.2-test");
        let sink = Arc::new(RecordingRecoverySink::default());
        let schedule = RecoverySelfCheckSchedule {
            initial_delay: Duration::from_millis(1),
            interval: Duration::from_millis(20),
            idle_minimum: Duration::from_millis(0),
        };
        let coordinator = RecoveryCoordinator::start_with_schedule(
            manager.clone(),
            database.clone(),
            sink,
            Arc::new(FixedActivityProbe(true)),
            schedule,
            RECOVERY_QUIET_PERIOD,
        )
        .await;
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let projected = coordinator
                    .health()
                    .last_incident
                    .is_some_and(|incident| incident.action == IncidentAction::Repaired);
                if projected {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("self-check repairs runtime corruption");
        assert_eq!(request_count(&primary), 5);
        assert_eq!(
            manager
                .inspect_primary_integrity()
                .expect("integrity report")
                .classification,
            IntegrityClassification::Ok
        );
        let record = crate::incident::read_latest_incident(&log_directory)
            .expect("incident read")
            .expect("incident record");
        assert_eq!(record.action, IncidentAction::Repaired);
        assert_eq!(record.recheck, IncidentRecheck::Ok);
        assert_eq!(
            coordinator
                .health()
                .last_incident
                .expect("health carries the incident")
                .action,
            IncidentAction::Repaired
        );
        assert!(coordinator.shutdown(Duration::from_secs(2)).await);
    }

    #[tokio::test]
    async fn runtime_self_check_skips_while_the_activity_probe_reports_work() {
        let root = tempfile::tempdir().expect("root");
        let log_directory = root.path().join("logs");
        let primary = root.path().join("data/router.sqlite3");
        let snapshot = root.path().join("snapshot.sqlite3");
        let database = DatabaseExecutor::open(&primary).expect("database");
        for index in 0..4 {
            record_synthetic_request(&database, index).await;
        }
        fs::copy(&primary, &snapshot).expect("snapshot copy");
        record_synthetic_request(&database, 4).await;
        splice_root_page(&primary, &snapshot, KEYSET_INDEX);

        let manager =
            RecoveryManager::new(&primary).with_incident_recording(&log_directory, "0.4.2-test");
        let schedule = RecoverySelfCheckSchedule {
            initial_delay: Duration::from_millis(1),
            interval: Duration::from_millis(20),
            idle_minimum: Duration::from_millis(0),
        };
        let coordinator = RecoveryCoordinator::start_with_schedule(
            manager.clone(),
            database.clone(),
            Arc::new(RecordingRecoverySink::default()),
            Arc::new(FixedActivityProbe(false)),
            schedule,
            RECOVERY_QUIET_PERIOD,
        )
        .await;
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(
            manager
                .inspect_primary_integrity()
                .expect("integrity report")
                .classification,
            IntegrityClassification::IndexOnly
        );
        assert_eq!(request_count(&primary), 5);
        assert_eq!(
            crate::incident::read_latest_incident(&log_directory).expect("incident read"),
            None
        );
        let quarantines = fs::read_dir(manager.recovery_dir())
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .filter(|entry| {
                        entry
                            .file_name()
                            .to_string_lossy()
                            .starts_with("quarantine-")
                    })
                    .count()
            })
            .unwrap_or_default();
        assert_eq!(quarantines, 0);
        assert!(coordinator.shutdown(Duration::from_secs(2)).await);
    }

    #[test]
    fn directory_lock_is_exclusive_and_reusable() {
        let root = tempfile::tempdir().expect("root");
        let primary = root.path().join("data/router.sqlite3");
        let manager = RecoveryManager::new(&primary);
        let lock = manager.acquire_directory_lock().expect("first lock");
        let lock_path = lock.path().to_path_buf();
        assert_eq!(
            lock_path.file_name().and_then(|name| name.to_str()),
            Some("router.sqlite3.lock")
        );
        assert!(matches!(
            manager.acquire_directory_lock(),
            Err(RecoveryError::DirectoryInUse)
        ));
        assert_eq!(
            classify_recovery_startup_error(&RecoveryError::DirectoryInUse),
            Some(DatabaseStartupIssue::DirectoryInUse)
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&lock_path)
                    .expect("lock metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        drop(lock);
        let again = manager
            .acquire_directory_lock()
            .expect("lock after release");
        assert_eq!(again.path(), lock_path.as_path());
    }

    #[test]
    fn recovery_scan_preserves_the_directory_lock_and_unknown_files() {
        let root = tempfile::tempdir().expect("root");
        let primary = root.path().join("data/router.sqlite3");
        fs::create_dir_all(primary.parent().expect("parent")).expect("parent");
        let manager = RecoveryManager::new(&primary);
        let lock = manager.acquire_directory_lock().expect("lock");
        manager.ensure_recovery_dir().expect("recovery dir");
        let unknown = manager.recovery_dir().join("notes.txt");
        fs::write(&unknown, b"leave intact").expect("unknown file");
        manager.scan().expect("scan");
        manager
            .cleanup_recognized_temporaries()
            .expect("cleanup temporaries");
        manager
            .apply_point_retention(super::now_millis())
            .expect("point retention");
        assert!(lock.path().exists());
        assert!(unknown.exists());
    }
}
