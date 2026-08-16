use std::{
    env,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use router_core::{
    app_api::{
        ApplicationUpdateFailureDto, ApplicationUpdateOperationDto, ApplicationUpdateProgressDto,
        ApplicationUpdateReleaseDto, ApplicationUpdateSnapshotDto,
    },
    state::{AppRuntimeState, IpcErrorDto, StateArea},
};
use semver::Version;
use tauri::{AppHandle, ipc::Channel};
use tauri_plugin_updater::{Update, UpdaterExt};
use url::Url;

use crate::runtime::DesktopLifecycleServices;

const AUTOMATIC_CHECK_DELAY: Duration = Duration::from_mins(1);
const AUTOMATIC_CHECK_INTERVAL_MS: i64 = 24 * 60 * 60 * 1_000;
const CANONICAL_RELEASE_ORIGIN: &str = "https://github.com";
const CANONICAL_REPOSITORY_PATH: &str = "/Angry3D/ai-router";
const QA_ENDPOINT_ENV: &str = "AI_ROUTER_QA_UPDATER_ENDPOINT";
const QA_PUBLIC_KEY_ENV: &str = "AI_ROUTER_QA_UPDATER_PUBLIC_KEY";
const UPDATER_PUBLIC_KEY_PLACEHOLDER: &str = "__AI_ROUTER_UPDATER_PUBLIC_KEY__";
const MAX_VERSION_CHARS: usize = 128;
const MAX_RELEASE_NOTES_CHARS: usize = 4_000;
const MAX_RELEASE_NOTES_LINES: usize = 80;
const MAX_SIGNATURE_CHARS: usize = 16_384;
const MAX_QA_PUBLIC_KEY_CHARS: usize = 8_192;
const MAX_PROGRESS_BYTES: u64 = 9_007_199_254_740_991;
pub(crate) const APPLICATION_UPDATE_RESTART_REQUEST_CODE: i32 = 64;

enum NormalizedRelease {
    Current,
    Available(ApplicationUpdateReleaseDto),
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
        let coordinator = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(AUTOMATIC_CHECK_DELAY).await;
            let Some(database) = services.application_update_database().await else {
                return;
            };
            let Ok(settings) = database.app_settings().await else {
                return;
            };
            let now = now_millis();
            if !automatic_check_is_due(settings.last_automatic_update_check_at_ms, now) {
                return;
            }
            if database
                .set_last_automatic_update_check_at_ms(now)
                .await
                .is_err()
            {
                return;
            }
            let _ = coordinator.check(false).await;
        });
    }

    pub async fn check_manual(&self) -> Result<ApplicationUpdateSnapshotDto, IpcErrorDto> {
        self.check(true).await
    }

    async fn check(&self, manual: bool) -> Result<ApplicationUpdateSnapshotDto, IpcErrorDto> {
        let _operation = try_operation_gate(&self.operation_gate)?;
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
            return Ok(self.snapshot());
        }

        match result {
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
                }
                Ok(NormalizedRelease::Current) => self.finish_current_check(),
                Err(failure) => self.finish_failed_check(manual, failure),
            },
            Ok(None) => self.finish_current_check(),
            Err(error) => self.finish_failed_check(manual, map_check_error(&error)),
        }
        self.publish_boundary();
        Ok(self.snapshot())
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
    Ok(NormalizedRelease::Available(ApplicationUpdateReleaseDto {
        version: version.to_string(),
        notes: normalize_notes(notes),
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

fn automatic_check_is_due(last_attempt_ms: Option<i64>, now_ms: i64) -> bool {
    last_attempt_ms.is_none_or(|last_attempt| {
        last_attempt > now_ms || now_ms.saturating_sub(last_attempt) >= AUTOMATIC_CHECK_INTERVAL_MS
    })
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
                notes: "notes".to_owned(),
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
    fn automatic_cadence_treats_future_clock_as_due() {
        assert!(automatic_check_is_due(None, 100));
        assert!(automatic_check_is_due(Some(101), 100));
        assert!(!automatic_check_is_due(Some(100), 100));
        assert!(automatic_check_is_due(
            Some(100),
            100 + AUTOMATIC_CHECK_INTERVAL_MS
        ));
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
