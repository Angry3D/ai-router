use std::{
    collections::HashSet,
    env,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use router_core::{
    app_api::{
        ApplicationUpdateFailureDto, ApplicationUpdateNotesDto, ApplicationUpdateOperationDto,
        ApplicationUpdateProgressDto, ApplicationUpdateReleaseDto, ApplicationUpdateSnapshotDto,
    },
    state::{AppRuntimeState, IpcErrorDto, StateArea},
};
use semver::Version;
use tauri::{AppHandle, ipc::Channel};
use tauri_plugin_updater::{Update, UpdaterExt};
use url::Url;

use crate::runtime::DesktopLifecycleServices;

const AUTOMATIC_CHECK_SUCCESS_INTERVAL: Duration = Duration::from_hours(24);
const AUTOMATIC_CHECK_FAILURE_INTERVAL: Duration = Duration::from_hours(6);
const AUTOMATIC_CHECK_BUSY_RETRY: Duration = Duration::from_mins(5);
const AUTOMATIC_CHECK_SLEEP_STEP: Duration = Duration::from_mins(15);
const CANONICAL_RELEASE_ORIGIN: &str = "https://github.com";
const CANONICAL_REPOSITORY_PATH: &str = "/Angry3D/ai-router";
const QA_ENDPOINT_ENV: &str = "AI_ROUTER_QA_UPDATER_ENDPOINT";
const QA_PUBLIC_KEY_ENV: &str = "AI_ROUTER_QA_UPDATER_PUBLIC_KEY";
const UPDATER_PUBLIC_KEY_PLACEHOLDER: &str = "__AI_ROUTER_UPDATER_PUBLIC_KEY__";
const MAX_VERSION_CHARS: usize = 128;
const MAX_RELEASE_NOTES_CHARS: usize = 4_000;
const MAX_RELEASE_NOTES_LINES: usize = 80;
const MAX_RELEASE_NOTES_ITEMS: usize = 20;
const MAX_RELEASE_NOTE_ITEM_CHARS: usize = 240;
const MAX_SIGNATURE_CHARS: usize = 16_384;
const MAX_QA_PUBLIC_KEY_CHARS: usize = 8_192;
const MAX_PROGRESS_BYTES: u64 = 9_007_199_254_740_991;
pub(crate) const APPLICATION_UPDATE_RESTART_REQUEST_CODE: i32 = 64;

enum NormalizedRelease {
    Current,
    Available(ApplicationUpdateReleaseDto),
}

/// Internal classification of one automatic check. It never reaches IPC: the
/// user-visible snapshot and the manual failure DTO are unchanged, and this
/// only selects how long the scheduler waits before the next attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AutomaticCheckOutcome {
    /// The update service proved the app is current, or offered a release that
    /// passed full metadata validation.
    Succeeded,
    /// Network, update-service, or metadata validation failure.
    Failed,
}

/// Why one automatic attempt ended, which is the only input that decides the
/// next wait and whether the process-long loop continues.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AutomaticAttemptOutcome {
    /// An installed update owns the next launch; this process stops checking.
    Stopped,
    /// Another update operation held the gate, so no attempt was recorded.
    GateBusy,
    /// The attempt timestamp could not be recorded, so no network work ran.
    NotRecorded,
    /// The update service was reached and the result was classified.
    Checked(AutomaticCheckOutcome),
}

struct CoordinatorState {
    snapshot: ApplicationUpdateSnapshotDto,
    pending: Option<Update>,
}

pub struct ApplicationUpdateCoordinator {
    app: AppHandle,
    runtime_state: Arc<AppRuntimeState>,
    allow_qa_override: bool,
    official_updates_enabled: bool,
    operation_gate: tokio::sync::Mutex<()>,
    generation: AtomicU64,
    scheduler_started: AtomicBool,
    state: Mutex<CoordinatorState>,
}

impl ApplicationUpdateCoordinator {
    #[must_use]
    pub fn new(
        app: AppHandle,
        runtime_state: Arc<AppRuntimeState>,
        allow_qa_override: bool,
    ) -> Arc<Self> {
        let official_updates_enabled = app
            .config()
            .plugins
            .0
            .get("updater")
            .and_then(|config| config.get("pubkey"))
            .and_then(serde_json::Value::as_str)
            .is_some_and(official_public_key_configured);
        Arc::new(Self {
            state: Mutex::new(CoordinatorState {
                snapshot: ApplicationUpdateSnapshotDto {
                    current_version: app.package_info().version.to_string(),
                    operation: ApplicationUpdateOperationDto::Idle,
                    available: None,
                    last_successful_check_at_ms: None,
                    downloaded_bytes: None,
                    total_bytes: None,
                    manual_failure: None,
                },
                pending: None,
            }),
            app,
            runtime_state,
            allow_qa_override,
            official_updates_enabled,
            operation_gate: tokio::sync::Mutex::new(()),
            generation: AtomicU64::new(0),
            scheduler_started: AtomicBool::new(false),
        })
    }

    #[must_use]
    pub fn snapshot(&self) -> ApplicationUpdateSnapshotDto {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .snapshot
            .clone()
    }

    pub fn start_automatic_scheduler(self: &Arc<Self>, services: Arc<DesktopLifecycleServices>) {
        if services.is_isolated() || !self.official_updates_enabled {
            return;
        }
        // The scheduler now owns a process-long loop, so a second task would
        // permanently double this app's request rate against the update service.
        if !claim_scheduler_start(&self.scheduler_started) {
            return;
        }
        let coordinator = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            coordinator.run_automatic_scheduler(&services).await;
        });
    }

    /// Checks immediately on entry and keeps checking for the life of the
    /// process. The persisted attempt timestamp records the last automatic
    /// network attempt for diagnostics; it never suppresses the first check of
    /// a launch, so every restart deliberately resets this cadence.
    async fn run_automatic_scheduler(&self, services: &DesktopLifecycleServices) {
        loop {
            let attempt_started_ms = now_millis();
            let Some(interval) = self.run_automatic_attempt(services).await else {
                return;
            };
            wait_for_next_attempt(attempt_started_ms, interval).await;
        }
    }

    /// Runs one automatic attempt and returns how long to wait, measured from
    /// the attempt start, or `None` when this process should stop checking.
    async fn run_automatic_attempt(&self, services: &DesktopLifecycleServices) -> Option<Duration> {
        automatic_attempt_wait(self.classify_automatic_attempt(services).await)
    }

    async fn classify_automatic_attempt(
        &self,
        services: &DesktopLifecycleServices,
    ) -> AutomaticAttemptOutcome {
        if automatic_scheduler_should_stop(self.snapshot().operation) {
            return AutomaticAttemptOutcome::Stopped;
        }
        let Ok(_operation) = try_operation_gate(&self.operation_gate) else {
            // A manual check or an install owns the single operation gate.
            // Retry soon without recording a network attempt that never ran.
            return AutomaticAttemptOutcome::GateBusy;
        };
        let Some(database) = services.application_update_database().await else {
            return AutomaticAttemptOutcome::NotRecorded;
        };
        if database
            .set_last_automatic_update_check_at_ms(now_millis())
            .await
            .is_err()
        {
            // Never reach the update service without first recording the attempt.
            return AutomaticAttemptOutcome::NotRecorded;
        }
        AutomaticAttemptOutcome::Checked(self.run_check(false).await)
    }

    pub async fn check_manual(&self) -> Result<ApplicationUpdateSnapshotDto, IpcErrorDto> {
        let _operation = try_operation_gate(&self.operation_gate)?;
        self.run_check(true).await;
        Ok(self.snapshot())
    }

    /// Runs one metadata check. The caller must already hold the operation gate.
    async fn run_check(&self, manual: bool) -> AutomaticCheckOutcome {
        let generation = self.next_generation();
        self.update_snapshot(|snapshot| {
            snapshot.operation = ApplicationUpdateOperationDto::Checking;
            if manual {
                snapshot.manual_failure = None;
            }
        });
        self.publish_boundary();

        let result = match self.build_updater() {
            Ok(updater) => updater.check().await,
            Err(error) => Err(error),
        };
        if !self.is_current(generation) {
            // A newer operation owns the snapshot. Discard this result and let
            // the scheduler retry on the bounded failure cadence.
            return AutomaticCheckOutcome::Failed;
        }

        let outcome = match result {
            Ok(Some(update)) => match normalize_release(&update, self.allow_qa_override) {
                Ok(NormalizedRelease::Available(release)) => {
                    let now = now_millis();
                    let mut state = self
                        .state
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    state.pending = Some(update);
                    state.snapshot.operation = ApplicationUpdateOperationDto::Idle;
                    state.snapshot.available = Some(release);
                    state.snapshot.last_successful_check_at_ms = Some(now);
                    state.snapshot.downloaded_bytes = None;
                    state.snapshot.total_bytes = None;
                    state.snapshot.manual_failure = None;
                    AutomaticCheckOutcome::Succeeded
                }
                Ok(NormalizedRelease::Current) => {
                    self.finish_current_check();
                    AutomaticCheckOutcome::Succeeded
                }
                Err(failure) => {
                    self.finish_failed_check(manual, failure);
                    AutomaticCheckOutcome::Failed
                }
            },
            Ok(None) => {
                self.finish_current_check();
                AutomaticCheckOutcome::Succeeded
            }
            Err(error) => {
                self.finish_failed_check(manual, map_check_error(&error));
                AutomaticCheckOutcome::Failed
            }
        };
        self.publish_boundary();
        outcome
    }

    pub async fn download_and_install(
        &self,
        progress: Channel<ApplicationUpdateProgressDto>,
    ) -> Result<ApplicationUpdateSnapshotDto, IpcErrorDto> {
        let _operation = try_operation_gate(&self.operation_gate)?;
        let generation = self.next_generation();
        let update = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let update = state.pending.clone().ok_or_else(|| {
                ipc_error("update_not_available", "当前没有可下载的应用更新。", true)
            })?;
            state.snapshot.operation = ApplicationUpdateOperationDto::Downloading;
            state.snapshot.downloaded_bytes = Some(0);
            state.snapshot.total_bytes = None;
            state.snapshot.manual_failure = None;
            update
        };
        self.publish_boundary();
        let _ = progress.send(self.progress_snapshot());

        let downloaded = Arc::new(AtomicU64::new(0));
        let downloaded_for_chunk = Arc::clone(&downloaded);
        let state_for_chunk = &self.state;
        let progress_for_chunk = progress.clone();
        let generation_for_chunk = &self.generation;
        let state_for_finish = &self.state;
        let progress_for_finish = progress.clone();
        let generation_for_finish = &self.generation;
        let result = update
            .download_and_install(
                move |chunk, total| {
                    if generation_for_chunk.load(Ordering::Acquire) != generation {
                        return;
                    }
                    let chunk = u64::try_from(chunk).unwrap_or(MAX_PROGRESS_BYTES);
                    let current = downloaded_for_chunk
                        .fetch_add(chunk, Ordering::AcqRel)
                        .saturating_add(chunk)
                        .min(MAX_PROGRESS_BYTES);
                    let total = total.map(|value| value.min(MAX_PROGRESS_BYTES));
                    let mut state = state_for_chunk
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    state.snapshot.downloaded_bytes = Some(current);
                    state.snapshot.total_bytes = total;
                    let _ = progress_for_chunk.send(ApplicationUpdateProgressDto {
                        operation: ApplicationUpdateOperationDto::Downloading,
                        downloaded_bytes: Some(current),
                        total_bytes: total,
                    });
                },
                move || {
                    if generation_for_finish.load(Ordering::Acquire) != generation {
                        return;
                    }
                    let mut state = state_for_finish
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    state.snapshot.operation = ApplicationUpdateOperationDto::Installing;
                    let _ = progress_for_finish.send(ApplicationUpdateProgressDto {
                        operation: ApplicationUpdateOperationDto::Installing,
                        downloaded_bytes: state.snapshot.downloaded_bytes,
                        total_bytes: state.snapshot.total_bytes,
                    });
                },
            )
            .await;

        if !self.is_current(generation) {
            return Ok(self.snapshot());
        }
        match result {
            Ok(()) => self.update_snapshot(|snapshot| {
                snapshot.operation = ApplicationUpdateOperationDto::RestartReady;
                snapshot.manual_failure = None;
            }),
            Err(error) => {
                let failure = map_install_error(&error);
                self.update_snapshot(|snapshot| {
                    snapshot.operation = ApplicationUpdateOperationDto::Idle;
                    snapshot.manual_failure = Some(failure);
                });
            }
        }
        self.publish_boundary();
        Ok(self.snapshot())
    }

    pub fn open_release(&self) -> Result<(), IpcErrorDto> {
        let url = self.snapshot().available.map_or_else(
            || format!("{CANONICAL_RELEASE_ORIGIN}{CANONICAL_REPOSITORY_PATH}/releases/latest"),
            |release| release.release_url,
        );
        tauri_plugin_opener::open_url(url, None::<&str>).map_err(|_| {
            ipc_error(
                "update_release_open_failed",
                "GitHub Release 无法打开。",
                true,
            )
        })
    }

    pub fn request_restart(&self) -> Result<(), IpcErrorDto> {
        if !restart_request_is_allowed(self.snapshot().operation) {
            return Err(ipc_error(
                "update_restart_not_ready",
                "应用更新尚未准备好重启。",
                true,
            ));
        }
        // A direct Tauri restart cannot be delayed by ExitRequested::prevent_exit.
        // Emit an ordinary, interceptable exit intent; the event loop restarts
        // only after the existing graceful shutdown path has completed.
        self.app.exit(APPLICATION_UPDATE_RESTART_REQUEST_CODE);
        Ok(())
    }

    fn build_updater(&self) -> tauri_plugin_updater::Result<tauri_plugin_updater::Updater> {
        let mut builder = self
            .app
            .updater_builder()
            .target("darwin-aarch64")
            // The coordinator, not the plugin's availability shortcut, must
            // validate current, forward, and downgrade metadata uniformly.
            .version_comparator(|_, _| true);
        if self.allow_qa_override {
            let endpoint = env::var(QA_ENDPOINT_ENV).ok();
            let public_key = env::var(QA_PUBLIC_KEY_ENV).ok();
            match qa_override_configuration(endpoint.as_deref(), public_key.as_deref())? {
                Some((endpoint, public_key)) => {
                    builder = builder.endpoints(vec![endpoint])?.pubkey(public_key);
                }
                None if self.official_updates_enabled => {}
                None => {
                    return Err(tauri_plugin_updater::Error::ReleaseNotFound);
                }
            }
        } else if !self.official_updates_enabled {
            return Err(tauri_plugin_updater::Error::ReleaseNotFound);
        }
        builder.build()
    }

    fn finish_failed_check(&self, manual: bool, failure: ApplicationUpdateFailureDto) {
        self.update_snapshot(|snapshot| apply_failed_check(snapshot, manual, failure));
    }

    fn finish_current_check(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.pending = None;
        state.snapshot.operation = ApplicationUpdateOperationDto::Idle;
        state.snapshot.available = None;
        state.snapshot.last_successful_check_at_ms = Some(now_millis());
        state.snapshot.downloaded_bytes = None;
        state.snapshot.total_bytes = None;
        state.snapshot.manual_failure = None;
    }

    fn update_snapshot(&self, update: impl FnOnce(&mut ApplicationUpdateSnapshotDto)) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        update(&mut state.snapshot);
    }

    fn progress_snapshot(&self) -> ApplicationUpdateProgressDto {
        let snapshot = self.snapshot();
        ApplicationUpdateProgressDto {
            operation: snapshot.operation,
            downloaded_bytes: snapshot.downloaded_bytes,
            total_bytes: snapshot.total_bytes,
        }
    }

    fn next_generation(&self) -> u64 {
        advance_generation(&self.generation)
    }

    fn is_current(&self, generation: u64) -> bool {
        generation_is_current(&self.generation, generation)
    }

    fn publish_boundary(&self) {
        self.runtime_state
            .publish_background_change(vec![StateArea::ApplicationUpdate]);
    }
}

fn normalize_release(
    update: &Update,
    allow_qa_override: bool,
) -> Result<NormalizedRelease, ApplicationUpdateFailureDto> {
    let raw_version = update
        .raw_json
        .get("version")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(metadata_failure)?;
    if raw_version != update.version {
        return Err(metadata_failure());
    }
    normalize_release_fields(
        raw_version,
        &update.current_version,
        &update.target,
        &update.signature,
        &update.download_url,
        update.body.as_deref().unwrap_or(""),
        allow_qa_override,
    )
}

fn normalize_release_fields(
    version: &str,
    current_version: &str,
    target: &str,
    signature: &str,
    download_url: &Url,
    notes: &str,
    allow_qa_override: bool,
) -> Result<NormalizedRelease, ApplicationUpdateFailureDto> {
    let version = parse_bounded_canonical_version(version)?;
    let current = parse_bounded_canonical_version(current_version)?;
    if !version.pre.is_empty()
        || !version.build.is_empty()
        || version < current
        || target != "darwin-aarch64"
        || signature.is_empty()
        || signature.len() > MAX_SIGNATURE_CHARS
        || !canonical_download_url(download_url, &version, allow_qa_override)
    {
        return Err(metadata_failure());
    }
    if version == current {
        return Ok(NormalizedRelease::Current);
    }
    let (notes, legacy_notes) = normalize_update_notes(notes, &version.to_string())?;
    Ok(NormalizedRelease::Available(ApplicationUpdateReleaseDto {
        version: version.to_string(),
        notes,
        legacy_notes,
        release_url: format!(
            "{CANONICAL_RELEASE_ORIGIN}{CANONICAL_REPOSITORY_PATH}/releases/tag/v{version}"
        ),
    }))
}

fn parse_bounded_canonical_version(value: &str) -> Result<Version, ApplicationUpdateFailureDto> {
    if value.is_empty() || value.len() > MAX_VERSION_CHARS {
        return Err(metadata_failure());
    }
    let version = Version::parse(value).map_err(|_| metadata_failure())?;
    if version.to_string() != value {
        return Err(metadata_failure());
    }
    Ok(version)
}

fn canonical_download_url(url: &Url, version: &Version, allow_qa_override: bool) -> bool {
    if allow_qa_override && is_loopback_url(url) {
        return url.path().ends_with("/AI.Router.app.tar.gz")
            && url.query().is_none()
            && url.fragment().is_none();
    }
    url.scheme() == "https"
        && url.host_str() == Some("github.com")
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
        && url.path()
            == format!(
                "{CANONICAL_REPOSITORY_PATH}/releases/download/v{version}/AI.Router.app.tar.gz"
            )
}

fn is_loopback_url(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https")
        && url
            .host_str()
            .is_some_and(|host| matches!(host, "127.0.0.1" | "localhost" | "::1"))
        && url.username().is_empty()
        && url.password().is_none()
        && url.fragment().is_none()
}

fn qa_override_configuration(
    endpoint: Option<&str>,
    public_key: Option<&str>,
) -> tauri_plugin_updater::Result<Option<(Url, String)>> {
    match (endpoint, public_key) {
        (None, None) => Ok(None),
        (Some(endpoint), Some(public_key)) => {
            let endpoint = Url::parse(endpoint)?;
            if !is_loopback_url(&endpoint)
                || public_key.is_empty()
                || public_key != public_key.trim()
                || public_key.len() > MAX_QA_PUBLIC_KEY_CHARS
            {
                return Err(tauri_plugin_updater::Error::ReleaseNotFound);
            }
            Ok(Some((endpoint, public_key.to_owned())))
        }
        _ => Err(tauri_plugin_updater::Error::ReleaseNotFound),
    }
}

fn normalize_notes(notes: &str) -> String {
    let mut normalized = String::new();
    for (line_index, line) in notes.lines().take(MAX_RELEASE_NOTES_LINES).enumerate() {
        if line_index > 0 && normalized.chars().count() < MAX_RELEASE_NOTES_CHARS {
            normalized.push('\n');
        }
        for character in line.chars().filter(|character| !character.is_control()) {
            if normalized.chars().count() >= MAX_RELEASE_NOTES_CHARS {
                break;
            }
            normalized.push(character);
        }
        if normalized.chars().count() >= MAX_RELEASE_NOTES_CHARS {
            break;
        }
    }
    normalized
}

fn normalize_update_notes(
    raw_notes: &str,
    version: &str,
) -> Result<(Option<ApplicationUpdateNotesDto>, Option<String>), ApplicationUpdateFailureDto> {
    let normalized_newlines = raw_notes.replace("\r\n", "\n");
    let structured = normalized_newlines.starts_with("# AI Router v");
    if structured
        && (normalized_newlines.chars().count() > MAX_RELEASE_NOTES_CHARS
            || normalized_newlines.lines().count() > MAX_RELEASE_NOTES_LINES
            || normalized_newlines
                .chars()
                .any(|character| character.is_control() && character != '\n'))
    {
        return Err(metadata_failure());
    }
    let normalized = normalize_notes(&normalized_newlines).trim().to_owned();
    if !structured {
        return Ok((None, (!normalized.is_empty()).then_some(normalized)));
    }
    parse_structured_notes(&normalized, version).map(|notes| (Some(notes), None))
}

fn parse_structured_notes(
    notes: &str,
    version: &str,
) -> Result<ApplicationUpdateNotesDto, ApplicationUpdateFailureDto> {
    #[derive(Clone, Copy, Eq, PartialEq)]
    enum Section {
        Highlights,
        Fixes,
        Notices,
    }

    let mut lines = notes.lines();
    if lines.next() != Some(format!("# AI Router v{version}").as_str()) {
        return Err(metadata_failure());
    }
    let mut result = ApplicationUpdateNotesDto {
        highlights: Vec::new(),
        fixes: Vec::new(),
        notices: Vec::new(),
    };
    let mut section = None;
    let mut last_section_index = 0_usize;
    let mut seen_sections = HashSet::new();
    let mut seen_items = HashSet::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if let Some(heading) = line.strip_prefix("## ") {
            let (next, index) = match heading {
                "重点更新" => (Section::Highlights, 1),
                "问题修复" => (Section::Fixes, 2),
                "注意事项" => (Section::Notices, 3),
                _ => return Err(metadata_failure()),
            };
            if index <= last_section_index || !seen_sections.insert(index) {
                return Err(metadata_failure());
            }
            last_section_index = index;
            section = Some(next);
            continue;
        }
        let item = line.strip_prefix("- ").ok_or_else(metadata_failure)?;
        let lowercase_item = item.to_ascii_lowercase();
        if item.is_empty()
            || item.trim() != item
            || item.starts_with(' ')
            || item.chars().count() > MAX_RELEASE_NOTE_ITEM_CHARS
            || item
                .chars()
                .any(|character| matches!(character, '`' | '*' | '_' | '~' | '<' | '>' | '[' | ']'))
            || lowercase_item.contains("http://")
            || lowercase_item.contains("https://")
            || lowercase_item.contains("www.")
            || !seen_items.insert(item.to_owned())
        {
            return Err(metadata_failure());
        }
        match section.ok_or_else(metadata_failure)? {
            Section::Highlights => result.highlights.push(item.to_owned()),
            Section::Fixes => result.fixes.push(item.to_owned()),
            Section::Notices => result.notices.push(item.to_owned()),
        }
    }
    let total = result.highlights.len() + result.fixes.len() + result.notices.len();
    if result.highlights.is_empty()
        || result.highlights.len() > 3
        || total > MAX_RELEASE_NOTES_ITEMS
        || (seen_sections.contains(&2) && result.fixes.is_empty())
        || (seen_sections.contains(&3) && result.notices.is_empty())
    {
        return Err(metadata_failure());
    }
    Ok(result)
}

/// Returns how long to wait from the attempt start, or `None` only when this
/// process must stop checking. Every failure keeps the loop alive on a bounded
/// retry so a long-running app never silently stops looking for updates.
fn automatic_attempt_wait(outcome: AutomaticAttemptOutcome) -> Option<Duration> {
    match outcome {
        AutomaticAttemptOutcome::Stopped => None,
        AutomaticAttemptOutcome::GateBusy => Some(AUTOMATIC_CHECK_BUSY_RETRY),
        AutomaticAttemptOutcome::NotRecorded
        | AutomaticAttemptOutcome::Checked(AutomaticCheckOutcome::Failed) => {
            Some(AUTOMATIC_CHECK_FAILURE_INTERVAL)
        }
        AutomaticAttemptOutcome::Checked(AutomaticCheckOutcome::Succeeded) => {
            Some(AUTOMATIC_CHECK_SUCCESS_INTERVAL)
        }
    }
}

fn automatic_scheduler_should_stop(operation: ApplicationUpdateOperationDto) -> bool {
    // An installed update owns the next launch, so this process stops checking
    // and the new process starts a fresh loop.
    operation == ApplicationUpdateOperationDto::RestartReady
}

fn claim_scheduler_start(started: &AtomicBool) -> bool {
    !started.swap(true, Ordering::AcqRel)
}

/// Waits for the next automatic attempt on wall-clock time. macOS suspends the
/// monotonic clock while the machine sleeps, so one long timer would silently
/// stretch a 24-hour cadence across a laptop's sleep cycles.
async fn wait_for_next_attempt(attempt_started_ms: i64, interval: Duration) {
    let due_at_ms = attempt_started_ms.saturating_add(duration_millis(interval));
    while let Some(step) = next_wait_step(due_at_ms, now_millis(), interval) {
        tokio::time::sleep(step).await;
    }
}

/// Returns the next bounded sleep step, or `None` once the attempt is due. A
/// remaining wait longer than the interval means the wall clock moved backward,
/// which is treated as due for the same reason a future attempt timestamp was.
fn next_wait_step(due_at_ms: i64, now_ms: i64, interval: Duration) -> Option<Duration> {
    let remaining_ms = due_at_ms.saturating_sub(now_ms);
    if remaining_ms <= 0 || remaining_ms > duration_millis(interval) {
        return None;
    }
    Some(
        Duration::from_millis(u64::try_from(remaining_ms).unwrap_or(u64::MAX))
            .min(AUTOMATIC_CHECK_SLEEP_STEP),
    )
}

fn duration_millis(duration: Duration) -> i64 {
    i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
}

fn try_operation_gate(
    gate: &tokio::sync::Mutex<()>,
) -> Result<tokio::sync::MutexGuard<'_, ()>, IpcErrorDto> {
    gate.try_lock().map_err(|_| update_busy_error())
}

fn advance_generation(generation: &AtomicU64) -> u64 {
    generation.fetch_add(1, Ordering::AcqRel).saturating_add(1)
}

fn generation_is_current(generation: &AtomicU64, candidate: u64) -> bool {
    generation.load(Ordering::Acquire) == candidate
}

fn apply_failed_check(
    snapshot: &mut ApplicationUpdateSnapshotDto,
    manual: bool,
    failure: ApplicationUpdateFailureDto,
) {
    snapshot.operation = ApplicationUpdateOperationDto::Idle;
    if manual {
        snapshot.manual_failure = Some(failure);
    }
}

fn restart_request_is_allowed(operation: ApplicationUpdateOperationDto) -> bool {
    operation == ApplicationUpdateOperationDto::RestartReady
}

fn official_public_key_configured(value: &str) -> bool {
    !value.trim().is_empty() && value != UPDATER_PUBLIC_KEY_PLACEHOLDER
}

fn map_check_error(error: &tauri_plugin_updater::Error) -> ApplicationUpdateFailureDto {
    let code = match error {
        tauri_plugin_updater::Error::Reqwest(_)
        | tauri_plugin_updater::Error::Network(_)
        | tauri_plugin_updater::Error::ReleaseNotFound => "update_offline",
        _ => "update_metadata_invalid",
    };
    update_failure(
        code,
        if code == "update_offline" {
            "暂时无法连接更新服务，请稍后重试。"
        } else {
            "更新信息未通过安全校验。"
        },
        true,
    )
}

fn map_install_error(error: &tauri_plugin_updater::Error) -> ApplicationUpdateFailureDto {
    let (code, message, retryable) = match error {
        tauri_plugin_updater::Error::Minisign(_)
        | tauri_plugin_updater::Error::Base64(_)
        | tauri_plugin_updater::Error::SignatureUtf8(_) => (
            "update_signature_invalid",
            "更新包签名校验失败，未安装任何内容。",
            false,
        ),
        tauri_plugin_updater::Error::AuthenticationFailed => (
            "update_permission_denied",
            "没有权限安装更新，请使用 GitHub Release 手动更新。",
            true,
        ),
        tauri_plugin_updater::Error::Reqwest(_) | tauri_plugin_updater::Error::Network(_) => {
            ("update_offline", "更新包下载中断，请稍后重试。", true)
        }
        _ => (
            "update_install_failed",
            "更新未能安装，当前应用保持不变。",
            true,
        ),
    };
    update_failure(code, message, retryable)
}

fn metadata_failure() -> ApplicationUpdateFailureDto {
    update_failure("update_metadata_invalid", "更新信息未通过安全校验。", true)
}

fn update_failure(code: &str, message: &str, retryable: bool) -> ApplicationUpdateFailureDto {
    ApplicationUpdateFailureDto {
        code: code.to_owned(),
        message: message.to_owned(),
        retryable,
    }
}

fn update_busy_error() -> IpcErrorDto {
    ipc_error("update_busy", "另一项更新操作正在进行。", true)
}

fn ipc_error(code: &str, message: &str, retryable: bool) -> IpcErrorDto {
    IpcErrorDto {
        code: code.to_owned(),
        message: message.to_owned(),
        retryable,
        field: None,
    }
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot() -> ApplicationUpdateSnapshotDto {
        ApplicationUpdateSnapshotDto {
            current_version: "1.2.3".to_owned(),
            operation: ApplicationUpdateOperationDto::Checking,
            available: Some(ApplicationUpdateReleaseDto {
                version: "1.2.4".to_owned(),
                notes: None,
                legacy_notes: Some("notes".to_owned()),
                release_url: "https://github.com/Angry3D/ai-router/releases/tag/v1.2.4".to_owned(),
            }),
            last_successful_check_at_ms: Some(100),
            downloaded_bytes: None,
            total_bytes: None,
            manual_failure: None,
        }
    }

    fn archive_url(version: &str) -> Url {
        Url::parse(&format!(
            "https://github.com/Angry3D/ai-router/releases/download/v{version}/AI.Router.app.tar.gz"
        ))
        .expect("archive URL")
    }

    #[test]
    fn only_restart_ready_stops_the_loop_and_failures_keep_a_bounded_wait() {
        assert_eq!(
            automatic_attempt_wait(AutomaticAttemptOutcome::Stopped),
            None
        );
        // Every non-stopping outcome keeps the process-long loop alive inside
        // the busy-retry / success-interval band, so no failure ends checking
        // and no failure collapses into dense update-service requests.
        for outcome in [
            AutomaticAttemptOutcome::GateBusy,
            AutomaticAttemptOutcome::NotRecorded,
            AutomaticAttemptOutcome::Checked(AutomaticCheckOutcome::Failed),
            AutomaticAttemptOutcome::Checked(AutomaticCheckOutcome::Succeeded),
        ] {
            let wait = automatic_attempt_wait(outcome).expect("a failure must not end the loop");
            assert!(wait >= AUTOMATIC_CHECK_BUSY_RETRY);
            assert!(wait <= AUTOMATIC_CHECK_SUCCESS_INTERVAL);
        }
    }

    #[test]
    fn automatic_cadence_separates_success_failure_and_busy_intervals() {
        assert_eq!(
            automatic_attempt_wait(AutomaticAttemptOutcome::Checked(
                AutomaticCheckOutcome::Succeeded
            )),
            Some(Duration::from_hours(24))
        );
        assert_eq!(
            automatic_attempt_wait(AutomaticAttemptOutcome::Checked(
                AutomaticCheckOutcome::Failed
            )),
            Some(Duration::from_hours(6))
        );
        // A timestamp that could not be recorded never reaches the network, so
        // it retries on the same bounded failure cadence.
        assert_eq!(
            automatic_attempt_wait(AutomaticAttemptOutcome::NotRecorded),
            Some(AUTOMATIC_CHECK_FAILURE_INTERVAL)
        );
        assert_eq!(
            automatic_attempt_wait(AutomaticAttemptOutcome::GateBusy),
            Some(AUTOMATIC_CHECK_BUSY_RETRY)
        );
        assert!(AUTOMATIC_CHECK_BUSY_RETRY < AUTOMATIC_CHECK_FAILURE_INTERVAL);
        assert!(AUTOMATIC_CHECK_FAILURE_INTERVAL < AUTOMATIC_CHECK_SUCCESS_INTERVAL);
    }

    #[test]
    fn automatic_waiting_uses_bounded_steps_and_ends_when_due() {
        let interval = AUTOMATIC_CHECK_SUCCESS_INTERVAL;
        let started = 1_000_000_i64;
        let due = started + duration_millis(interval);

        // A full interval ahead waits in bounded steps, never one long timer,
        // so a machine that slept through the due point notices on the next step.
        assert_eq!(
            next_wait_step(due, started, interval),
            Some(AUTOMATIC_CHECK_SLEEP_STEP)
        );
        // The final step shrinks to exactly the remaining time.
        assert_eq!(
            next_wait_step(due, due - 1_000, interval),
            Some(Duration::from_secs(1))
        );
        // Due now, and a wall clock that jumped past the due point.
        assert_eq!(next_wait_step(due, due, interval), None);
        assert_eq!(next_wait_step(due, due + 1, interval), None);
        // A wall clock that moved backward beyond the interval is treated as
        // due, matching the existing future-timestamp rule.
        assert_eq!(next_wait_step(due, 0, interval), None);
    }

    #[test]
    fn restart_ready_stops_the_automatic_scheduler() {
        assert!(automatic_scheduler_should_stop(
            ApplicationUpdateOperationDto::RestartReady
        ));
        for operation in [
            ApplicationUpdateOperationDto::Idle,
            ApplicationUpdateOperationDto::Checking,
            ApplicationUpdateOperationDto::Downloading,
            ApplicationUpdateOperationDto::Installing,
        ] {
            assert!(!automatic_scheduler_should_stop(operation));
        }
    }

    #[test]
    fn only_the_first_start_claims_the_process_long_scheduler() {
        let started = AtomicBool::new(false);
        assert!(claim_scheduler_start(&started));
        assert!(!claim_scheduler_start(&started));
        assert!(!claim_scheduler_start(&started));
    }

    #[test]
    fn current_and_forward_versions_pass_the_same_strict_metadata_boundary() {
        assert!(matches!(
            normalize_release_fields(
                "1.2.3",
                "1.2.3",
                "darwin-aarch64",
                "signature",
                &archive_url("1.2.3"),
                "notes",
                false,
            ),
            Ok(NormalizedRelease::Current)
        ));
        let available = normalize_release_fields(
            "1.2.4",
            "1.2.3",
            "darwin-aarch64",
            "signature",
            &archive_url("1.2.4"),
            "notes",
            false,
        )
        .expect("forward release");
        assert!(matches!(available, NormalizedRelease::Available(_)));
    }

    #[test]
    fn version_is_bounded_before_semver_parsing_and_unstable_or_older_is_rejected() {
        let oversized = "1".repeat(MAX_VERSION_CHARS + 1);
        assert!(parse_bounded_canonical_version(&oversized).is_err());
        for candidate in ["1.2.2", "1.2.4-beta.1", "1.2.4+build.1"] {
            assert!(
                normalize_release_fields(
                    candidate,
                    "1.2.3",
                    "darwin-aarch64",
                    "signature",
                    &archive_url(candidate),
                    "notes",
                    false,
                )
                .is_err()
            );
        }
    }

    #[test]
    fn current_metadata_still_rejects_wrong_target_or_archive_url() {
        assert!(
            normalize_release_fields(
                "1.2.3",
                "1.2.3",
                "darwin-x86_64",
                "signature",
                &archive_url("1.2.3"),
                "notes",
                false,
            )
            .is_err()
        );
        assert!(
            normalize_release_fields(
                "1.2.3",
                "1.2.3",
                "darwin-aarch64",
                "signature",
                &Url::parse("https://example.invalid/AI.Router.app.tar.gz").expect("foreign URL"),
                "notes",
                false,
            )
            .is_err()
        );
    }

    #[test]
    fn release_notes_are_plain_bounded_text() {
        let notes = format!("first\u{0000}line\n{}", "x".repeat(5_000));
        let normalized = normalize_notes(&notes);
        assert!(!normalized.contains('\u{0000}'));
        assert!(normalized.chars().count() <= MAX_RELEASE_NOTES_CHARS);
        assert!(normalized.lines().count() <= MAX_RELEASE_NOTES_LINES);
    }

    #[test]
    fn structured_release_notes_are_projected_into_typed_sections() {
        let notes = "# AI Router v1.2.4\n\n## 重点更新\n\n- 第一项改进\n- 第二项改进\n\n## 问题修复\n\n- 修复一个问题\n\n## 注意事项\n\n- 无需迁移配置";
        let (structured, legacy) =
            normalize_update_notes(notes, "1.2.4").expect("structured notes");
        let structured = structured.expect("typed notes");
        assert_eq!(structured.highlights, ["第一项改进", "第二项改进"]);
        assert_eq!(structured.fixes, ["修复一个问题"]);
        assert_eq!(structured.notices, ["无需迁移配置"]);
        assert!(legacy.is_none());
    }

    #[test]
    fn legacy_notes_remain_bounded_and_malformed_structured_notes_fail_closed() {
        let (structured, legacy) =
            normalize_update_notes("旧版本说明", "1.2.4").expect("legacy notes");
        assert!(structured.is_none());
        assert_eq!(legacy.as_deref(), Some("旧版本说明"));
        assert!(
            normalize_update_notes("# AI Router v1.2.4\n\n## 重点更新\n\n普通段落", "1.2.4")
                .is_err()
        );
        assert!(
            normalize_update_notes("# AI Router v9.9.9\n\n## 重点更新\n\n- 版本不匹配", "1.2.4")
                .is_err()
        );
        for notes in [
            "# AI Router v1.2.4\r\n\r\n## 重点更新\r\n\r\n- 合法 CRLF 内容",
            "# AI Router v1.2.4\n\n## 重点更新\n\n- 合法内容",
        ] {
            assert!(normalize_update_notes(notes, "1.2.4").is_ok());
        }
        for notes in [
            "# AI Router v1.2.4\n\n## 重点更新\n\n- 含有\t制表符",
            "# AI Router v1.2.4\n\n## 重点更新\n\n- **Markdown**",
            "# AI Router v1.2.4\n\n## 重点更新\n\n- 合法内容\n\n## 问题修复",
        ] {
            assert!(normalize_update_notes(notes, "1.2.4").is_err());
        }
    }

    #[test]
    fn production_archive_url_is_exact_and_qa_is_loopback_only() {
        let version = Version::parse("1.2.3").expect("version");
        let canonical = Url::parse(
            "https://github.com/Angry3D/ai-router/releases/download/v1.2.3/AI.Router.app.tar.gz",
        )
        .expect("canonical URL");
        assert!(canonical_download_url(&canonical, &version, false));
        assert!(!canonical_download_url(
            &Url::parse("https://example.com/AI.Router.app.tar.gz").expect("foreign URL"),
            &version,
            false
        ));
        assert!(canonical_download_url(
            &Url::parse("http://127.0.0.1:4100/AI.Router.app.tar.gz").expect("QA URL"),
            &version,
            true
        ));
    }

    #[test]
    fn qa_override_requires_a_complete_bounded_loopback_pair() {
        assert!(
            qa_override_configuration(None, None)
                .expect("absent override")
                .is_none()
        );
        assert!(
            qa_override_configuration(Some("http://127.0.0.1:4100/latest.json"), None).is_err()
        );
        assert!(qa_override_configuration(None, Some("synthetic-key")).is_err());
        assert!(
            qa_override_configuration(
                Some("https://example.invalid/latest.json"),
                Some("synthetic-key")
            )
            .is_err()
        );
        assert!(
            qa_override_configuration(Some("http://127.0.0.1:4100/latest.json"), Some(" "))
                .is_err()
        );
        assert!(
            qa_override_configuration(
                Some("http://127.0.0.1:4100/latest.json"),
                Some(&"k".repeat(MAX_QA_PUBLIC_KEY_CHARS + 1))
            )
            .is_err()
        );
        let configured = qa_override_configuration(
            Some("http://localhost:4100/latest.json"),
            Some("synthetic-key"),
        )
        .expect("valid QA override")
        .expect("configured QA override");
        assert_eq!(configured.0.host_str(), Some("localhost"));
        assert_eq!(configured.1, "synthetic-key");
    }

    #[test]
    fn source_build_placeholder_does_not_enable_official_updates() {
        assert!(!official_public_key_configured(""));
        assert!(!official_public_key_configured(
            UPDATER_PUBLIC_KEY_PLACEHOLDER
        ));
        assert!(official_public_key_configured(
            "synthetic-release-public-key"
        ));
    }

    #[tokio::test]
    async fn operation_gate_returns_the_bounded_busy_error() {
        let gate = tokio::sync::Mutex::new(());
        let _held = gate.lock().await;
        let Err(error) = try_operation_gate(&gate) else {
            panic!("gate must stay exclusive");
        };
        assert_eq!(error.code, "update_busy");
        assert!(error.retryable);
    }

    #[test]
    fn stale_generation_cannot_replace_current_work() {
        let generation = AtomicU64::new(0);
        let first = advance_generation(&generation);
        assert!(generation_is_current(&generation, first));
        let second = advance_generation(&generation);
        assert!(!generation_is_current(&generation, first));
        assert!(generation_is_current(&generation, second));
    }

    #[test]
    fn automatic_failure_is_quiet_while_manual_failure_retains_release() {
        let failure = update_failure("update_offline", "offline", true);
        let mut automatic = snapshot();
        apply_failed_check(&mut automatic, false, failure.clone());
        assert_eq!(automatic.operation, ApplicationUpdateOperationDto::Idle);
        assert!(automatic.manual_failure.is_none());
        assert!(automatic.available.is_some());

        let mut manual = snapshot();
        apply_failed_check(&mut manual, true, failure.clone());
        assert_eq!(manual.manual_failure, Some(failure));
        assert!(manual.available.is_some());
    }

    #[test]
    fn install_errors_keep_signature_permission_network_and_generic_categories() {
        let signature = map_install_error(&tauri_plugin_updater::Error::Base64(
            base64::DecodeError::InvalidByte(0, b'?'),
        ));
        assert_eq!(signature.code, "update_signature_invalid");
        assert!(!signature.retryable);

        let permission = map_install_error(&tauri_plugin_updater::Error::AuthenticationFailed);
        assert_eq!(permission.code, "update_permission_denied");
        assert!(permission.retryable);

        let network = map_install_error(&tauri_plugin_updater::Error::Network("offline".into()));
        assert_eq!(network.code, "update_offline");

        let generic = map_install_error(&tauri_plugin_updater::Error::PackageInstallFailed);
        assert_eq!(generic.code, "update_install_failed");
    }

    #[test]
    fn restart_requires_the_restart_ready_state() {
        assert!(restart_request_is_allowed(
            ApplicationUpdateOperationDto::RestartReady
        ));
        for operation in [
            ApplicationUpdateOperationDto::Idle,
            ApplicationUpdateOperationDto::Checking,
            ApplicationUpdateOperationDto::Downloading,
            ApplicationUpdateOperationDto::Installing,
        ] {
            assert!(!restart_request_is_allowed(operation));
        }
    }
}
