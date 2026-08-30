mod application_update;
mod popover;
mod runtime;

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering},
};
use std::time::Duration;

use application_update::{APPLICATION_UPDATE_RESTART_REQUEST_CODE, ApplicationUpdateCoordinator};
use popover::{
    MenuPopoverController, TrayPresentation, TrayVisualState, complete_menu_show,
    handle_tray_event, handle_window_event, hide_menu, hide_settings_window, initial_tray_title,
    menu_frontend_ready, set_menu_usage_preview, tray_presentation,
};
use router_core::app_api::{ApplicationUpdateProgressDto, ApplicationUpdateSnapshotDto};
use router_core::lifecycle::{AppCoordinator, AppLifecycleIssue, AppLifecyclePhase};
use router_core::proxy::{
    LogicalRequestActivityPhase, LogicalRequestActivitySink, LogicalRequestActivityTransition,
};
use router_core::qa_acceptance::QaAcceptanceRoot;
use router_core::state::{
    AppRuntimeState, BootstrapSnapshotDto, IpcErrorDto, StateArea, StateChangedEventDto,
    StateEventError, StateEventSink,
};
use runtime::{
    DesktopLifecycleServices, DesktopRuntimeProfile, RuntimeLogController,
    SafeRuntimeDiagnosticSink, activate_existing_instance, apply_proxy_port,
    check_route_reachability, clear_mcp_images, clear_request_history, clear_runtime_logs,
    confirm_codex_images_mcp_repair, confirm_reset_codex_recovery_to_baseline,
    confirm_route_activation, confirm_update_codex_recovery, connect_codex, create_recovery_point,
    delete_route, dismiss_codex_restart_notice, dismiss_mcp_image_capacity_warning,
    finish_runtime_log_setup, get_menu_snapshot, get_recovery_snapshot, get_route_edit,
    get_settings_snapshot, get_usage_history, get_usage_request_detail, get_usage_route_options,
    get_usage_statistics, mark_first_run_presented, open_codex_config, open_mcp_image_directory,
    open_runtime_log_directory, preview_codex_images_mcp_repair,
    preview_reset_codex_recovery_to_baseline, preview_route_activation,
    preview_update_codex_recovery, quit_application, reconnect_codex, refresh_all_balances,
    refresh_balance, reorder_routes_and_fallback, restore_codex, restore_recovery_point,
    retry_database_startup, runtime_log_bootstrap_plugin, runtime_log_plugin, save_route,
    set_fallback_enabled, show_settings_window, start_over_database, test_balance_query,
    update_appearance_preference, update_balance_query_settings, update_images_generation_settings,
    update_mcp_image_capacity_threshold, update_menu_bar_settings,
};
use tauri::{AppHandle, Emitter, Manager, RunEvent, State, ipc::Channel};

const STATE_CHANGED_EVENT: &str = "router-state-changed";
const PROJECT_REPOSITORY_URL: &str = "https://github.com/Angry3D/ai-router";
const ACTIVE_TRAY_FRAME_COUNT: usize = 4;
const TRAY_ANIMATION_FRAME_INTERVAL: Duration = Duration::from_millis(300);
const REDUCE_MOTION_REFRESH_INTERVAL: Duration = Duration::from_secs(3);

struct TauriStateEventSink {
    app_handle: AppHandle,
    tray_refresh: Arc<TrayRefreshCoordinator>,
}

struct TrayRefreshCoordinator {
    latest_revision: AtomicU64,
    refresh_gate: tokio::sync::Mutex<()>,
    reduce_motion_gate: tokio::sync::Mutex<()>,
    apply_gate: Mutex<()>,
    activity: Mutex<TrayActivityProjection>,
    animation: Mutex<TrayAnimationProjection>,
    monitor_generation: AtomicU64,
    reduce_motion: AtomicU8,
    activity_animation_enabled: AtomicBool,
    assets: TrayIconAssets,
}

impl TrayRefreshCoordinator {
    fn new(assets: TrayIconAssets, reduce_motion: Option<bool>) -> Self {
        Self {
            latest_revision: AtomicU64::new(0),
            refresh_gate: tokio::sync::Mutex::const_new(()),
            reduce_motion_gate: tokio::sync::Mutex::const_new(()),
            apply_gate: Mutex::new(()),
            activity: Mutex::new(TrayActivityProjection::default()),
            animation: Mutex::new(TrayAnimationProjection::default()),
            monitor_generation: AtomicU64::new(0),
            reduce_motion: AtomicU8::new(ReduceMotionState::from_observation(reduce_motion) as u8),
            activity_animation_enabled: AtomicBool::new(false),
            assets,
        }
    }

    fn request(&self) -> Option<u64> {
        let _apply_guard = self
            .apply_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self
            .animation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .shutdown
        {
            return None;
        }
        self.latest_revision
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |revision| {
                revision.checked_add(1)
            })
            .ok()
            .map(|revision| revision + 1)
    }

    fn is_current(&self, revision: u64) -> bool {
        self.latest_revision.load(Ordering::Acquire) == revision
    }

    fn can_apply(&self, revision: u64) -> bool {
        self.is_current(revision)
            && !self
                .animation
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .shutdown
    }

    fn apply_activity_transition(&self, transition: LogicalRequestActivityTransition) -> bool {
        let _apply_guard = self
            .apply_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut activity = self
            .activity
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if transition.revision <= activity.revision {
            return false;
        }
        activity.revision = transition.revision;
        activity.phase = transition.phase;
        let mut animation = self
            .animation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !animation.shutdown {
            animation.generation = animation.generation.saturating_add(1);
            animation.mode = None;
        }
        self.monitor_generation.fetch_add(1, Ordering::AcqRel);
        true
    }

    fn phase(&self) -> LogicalRequestActivityPhase {
        self.activity
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .phase
    }

    fn live_activity_revision(&self) -> Option<u64> {
        let activity = self
            .activity
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        (activity.phase == LogicalRequestActivityPhase::Live).then_some(activity.revision)
    }

    fn observe_reduce_motion(&self, reduce_motion: Option<bool>) -> Option<(bool, bool)> {
        let state = ReduceMotionState::from_observation(reduce_motion);
        if state == ReduceMotionState::Unknown {
            return None;
        }
        let previous = self.reduce_motion.swap(state as u8, Ordering::AcqRel);
        Some((state.blocks_animation(), previous != state as u8))
    }

    fn reduce_motion(&self) -> bool {
        ReduceMotionState::from_raw(self.reduce_motion.load(Ordering::Acquire)).blocks_animation()
    }

    fn live_activity_is_current(&self, revision: u64) -> bool {
        let activity = self
            .activity
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        activity.phase == LogicalRequestActivityPhase::Live
            && activity.revision == revision
            && self.activity_animation_enabled.load(Ordering::Acquire)
            && !self
                .animation
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .shutdown
    }

    fn begin_reduce_motion_monitor(&self, activity_revision: u64) -> Option<u64> {
        let _apply_guard = self
            .apply_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !self.live_activity_is_current(activity_revision) {
            return None;
        }
        Some(self.monitor_generation.fetch_add(1, Ordering::AcqRel) + 1)
    }

    fn monitor_is_current(&self, activity_revision: u64, monitor_generation: u64) -> bool {
        self.monitor_generation.load(Ordering::Acquire) == monitor_generation
            && self.live_activity_is_current(activity_revision)
    }

    fn observe_reduce_motion_if_current(
        &self,
        activity_revision: u64,
        monitor_generation: u64,
        observation: Option<bool>,
    ) -> Option<(bool, bool)> {
        let _apply_guard = self
            .apply_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !self.monitor_is_current(activity_revision, monitor_generation) {
            return None;
        }
        self.observe_reduce_motion(observation)
    }

    fn transition_projection(&self, visual_state: TrayVisualState) -> TrayProjectionDecision {
        let animated = visual_state == TrayVisualState::Active
            && self.activity_animation_enabled.load(Ordering::Acquire)
            && !self.reduce_motion();
        let desired = if animated {
            TrayProjectionMode::Animated
        } else {
            TrayProjectionMode::Static(visual_state)
        };
        let mut animation = self
            .animation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if animation.shutdown || animation.mode == Some(desired) {
            return TrayProjectionDecision {
                changed: false,
                animated,
                generation: animation.generation,
            };
        }
        animation.generation = animation.generation.saturating_add(1);
        animation.mode = Some(desired);
        TrayProjectionDecision {
            changed: true,
            animated,
            generation: animation.generation,
        }
    }

    fn animation_is_current(&self, generation: u64) -> bool {
        let animation = self
            .animation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        !animation.shutdown
            && animation.generation == generation
            && animation.mode == Some(TrayProjectionMode::Animated)
    }

    fn set_activity_animation_enabled(&self, enabled: bool) -> bool {
        let _apply_guard = self
            .apply_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let previous = self
            .activity_animation_enabled
            .swap(enabled, Ordering::AcqRel);
        if previous == enabled {
            return false;
        }
        let mut animation = self
            .animation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !animation.shutdown {
            animation.generation = animation.generation.saturating_add(1);
            animation.mode = None;
        }
        self.monitor_generation.fetch_add(1, Ordering::AcqRel);
        true
    }

    fn shutdown(&self) {
        let _apply_guard = self
            .apply_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.latest_revision.fetch_add(1, Ordering::AcqRel);
        let mut animation = self
            .animation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        animation.shutdown = true;
        animation.generation = animation.generation.saturating_add(1);
        animation.mode = None;
        self.monitor_generation.fetch_add(1, Ordering::AcqRel);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
enum ReduceMotionState {
    Unknown = 0,
    Enabled = 1,
    Disabled = 2,
}

impl ReduceMotionState {
    const fn from_observation(value: Option<bool>) -> Self {
        match value {
            Some(true) => Self::Enabled,
            Some(false) => Self::Disabled,
            None => Self::Unknown,
        }
    }

    const fn from_raw(value: u8) -> Self {
        match value {
            1 => Self::Enabled,
            2 => Self::Disabled,
            _ => Self::Unknown,
        }
    }

    const fn blocks_animation(self) -> bool {
        !matches!(self, Self::Disabled)
    }
}

struct TrayActivityProjection {
    phase: LogicalRequestActivityPhase,
    revision: u64,
}

impl Default for TrayActivityProjection {
    fn default() -> Self {
        Self {
            phase: LogicalRequestActivityPhase::Idle,
            revision: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TrayProjectionMode {
    Static(TrayVisualState),
    Animated,
}

#[derive(Default)]
struct TrayAnimationProjection {
    mode: Option<TrayProjectionMode>,
    generation: u64,
    shutdown: bool,
}

struct TrayProjectionDecision {
    changed: bool,
    animated: bool,
    generation: u64,
}

struct TrayIconAssets {
    ready: tauri::image::Image<'static>,
    active_static: tauri::image::Image<'static>,
    active_frames: [tauri::image::Image<'static>; 4],
}

impl TrayIconAssets {
    fn decode() -> tauri::Result<Self> {
        Ok(Self {
            ready: decode_tray_icon(include_bytes!("../icons/tray-route.png"))?,
            active_static: decode_tray_icon(include_bytes!("../icons/tray-active-static.png"))?,
            active_frames: [
                decode_tray_icon(include_bytes!("../icons/tray-active-a.png"))?,
                decode_tray_icon(include_bytes!("../icons/tray-active-b.png"))?,
                decode_tray_icon(include_bytes!("../icons/tray-active-c.png"))?,
                decode_tray_icon(include_bytes!("../icons/tray-active-d.png"))?,
            ],
        })
    }

    fn static_image(&self, visual_state: TrayVisualState) -> tauri::image::Image<'static> {
        match visual_state {
            TrayVisualState::Ready => self.ready.clone(),
            TrayVisualState::Active | TrayVisualState::Waiting => self.active_static.clone(),
        }
    }
}

fn decode_tray_icon(bytes: &'static [u8]) -> tauri::Result<tauri::image::Image<'static>> {
    tauri::image::Image::from_bytes(bytes)
}

impl StateEventSink for TauriStateEventSink {
    fn publish(&self, event: &StateChangedEventDto) -> Result<(), StateEventError> {
        if event.areas.contains(&StateArea::MenuBar)
            && let Some(runtime) = self.app_handle.try_state::<Arc<AppRuntimeState>>()
            && let Some(settings) = runtime.menu_bar_settings()
        {
            let changed = self
                .tray_refresh
                .set_activity_animation_enabled(settings.activity_animation_enabled);
            if changed
                && settings.activity_animation_enabled
                && let Some(revision) = self.tray_refresh.live_activity_revision()
            {
                start_reduce_motion_monitor(
                    self.app_handle.clone(),
                    Arc::clone(&self.tray_refresh),
                    revision,
                );
            }
        }
        if state_event_affects_tray(event) {
            let _ = schedule_tray_refresh(self.app_handle.clone(), Arc::clone(&self.tray_refresh));
        }
        self.app_handle
            .emit(STATE_CHANGED_EVENT, event)
            .map_err(|_| StateEventError)
    }
}

struct TauriLogicalRequestActivitySink {
    app_handle: AppHandle,
    tray_refresh: Arc<TrayRefreshCoordinator>,
}

impl LogicalRequestActivitySink for TauriLogicalRequestActivitySink {
    fn activity_changed(&self, transition: LogicalRequestActivityTransition) {
        if self.tray_refresh.apply_activity_transition(transition) {
            if !schedule_tray_refresh(self.app_handle.clone(), Arc::clone(&self.tray_refresh)) {
                return;
            }
            if transition.phase == LogicalRequestActivityPhase::Live
                && self
                    .tray_refresh
                    .activity_animation_enabled
                    .load(Ordering::Acquire)
            {
                start_reduce_motion_monitor(
                    self.app_handle.clone(),
                    Arc::clone(&self.tray_refresh),
                    transition.revision,
                );
            }
        }
    }
}

fn schedule_tray_refresh(app_handle: AppHandle, refresh: Arc<TrayRefreshCoordinator>) -> bool {
    let Some(revision) = refresh.request() else {
        return false;
    };
    tauri::async_runtime::spawn(async move {
        let _refresh_guard = refresh.refresh_gate.lock().await;
        if !refresh.is_current(revision) {
            return;
        }
        let Some(runtime) = app_handle.try_state::<Arc<AppRuntimeState>>() else {
            return;
        };
        let snapshot = runtime.bootstrap_snapshot();
        let Some(services) = app_handle.try_state::<Arc<DesktopLifecycleServices>>() else {
            return;
        };
        let balance = services
            .active_balance_snapshot(snapshot.active_route_id.as_ref())
            .await;
        let presentation = tray_presentation(
            &app_handle.package_info().name,
            &snapshot,
            balance.as_ref(),
            services.is_isolated(),
            refresh.phase(),
        );
        let menu_bar = runtime.menu_bar_settings();
        let _apply_guard = refresh
            .apply_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if refresh.can_apply(revision) {
            apply_tray_projection(&app_handle, &refresh, presentation, menu_bar);
        }
    });
    true
}

fn apply_tray_projection(
    app_handle: &AppHandle,
    refresh: &Arc<TrayRefreshCoordinator>,
    presentation: TrayPresentation,
    menu_bar: Option<router_core::app_api::MenuBarSettingsDto>,
) {
    let decision = refresh.transition_projection(presentation.visual_state);
    let Some(tray) = app_handle.tray_by_id("main") else {
        return;
    };
    if decision.changed {
        let image = if decision.animated {
            refresh.assets.active_frames[0].clone()
        } else {
            refresh.assets.static_image(presentation.visual_state)
        };
        let _ = tray.set_icon_with_as_template(Some(image), true);
    }
    let title = project_tray_title(
        presentation.title,
        menu_bar,
        app_handle.config().identifier != router_core::qa_acceptance::PRODUCTION_APP_IDENTIFIER,
    );
    let _ = tray.set_title(title);
    let _ = tray.set_tooltip(Some(presentation.tooltip));

    if decision.changed && decision.animated {
        start_tray_animation(app_handle.clone(), Arc::clone(refresh), decision.generation);
    }
}

fn project_tray_title(
    enabled_title: String,
    menu_bar: Option<router_core::app_api::MenuBarSettingsDto>,
    isolated: bool,
) -> Option<String> {
    match menu_bar {
        Some(settings) if settings.status_text_enabled => Some(enabled_title),
        _ if isolated => Some("QA".to_owned()),
        _ => None,
    }
}

fn start_tray_animation(
    app_handle: AppHandle,
    refresh: Arc<TrayRefreshCoordinator>,
    generation: u64,
) {
    tauri::async_runtime::spawn(async move {
        let mut frame_index = 1;
        loop {
            tokio::time::sleep(TRAY_ANIMATION_FRAME_INTERVAL).await;
            if !refresh.animation_is_current(generation) {
                return;
            }
            let _apply_guard = refresh
                .apply_gate
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !refresh.animation_is_current(generation) {
                return;
            }
            if let Some(tray) = app_handle.tray_by_id("main") {
                let image = refresh.assets.active_frames[frame_index].clone();
                let _ = tray.set_icon_with_as_template(Some(image), true);
            }
            frame_index = next_active_frame_index(frame_index);
        }
    });
}

const fn next_active_frame_index(current: usize) -> usize {
    (current + 1) % ACTIVE_TRAY_FRAME_COUNT
}

fn start_reduce_motion_monitor(
    app_handle: AppHandle,
    refresh: Arc<TrayRefreshCoordinator>,
    activity_revision: u64,
) {
    let Some(monitor_generation) = refresh.begin_reduce_motion_monitor(activity_revision) else {
        return;
    };
    tauri::async_runtime::spawn(async move {
        loop {
            if !refresh.monitor_is_current(activity_revision, monitor_generation) {
                return;
            }
            if refresh_system_reduce_motion(&refresh, activity_revision, monitor_generation)
                .await
                .is_some_and(|(_, changed)| changed)
            {
                let _ = schedule_tray_refresh(app_handle.clone(), Arc::clone(&refresh));
            }
            tokio::time::sleep(REDUCE_MOTION_REFRESH_INTERVAL).await;
        }
    });
}

async fn refresh_system_reduce_motion(
    refresh: &Arc<TrayRefreshCoordinator>,
    activity_revision: u64,
    monitor_generation: u64,
) -> Option<(bool, bool)> {
    let _query_guard = refresh.reduce_motion_gate.lock().await;
    if !refresh.monitor_is_current(activity_revision, monitor_generation) {
        return None;
    }
    let observation = tauri::async_runtime::spawn_blocking(system_reduce_motion)
        .await
        .ok()
        .flatten();
    refresh.observe_reduce_motion_if_current(activity_revision, monitor_generation, observation)
}

#[cfg(target_os = "macos")]
fn system_reduce_motion() -> Option<bool> {
    let output = std::process::Command::new("/usr/bin/defaults")
        .args(["read", "com.apple.universalAccess", "reduceMotion"])
        .output()
        .ok()?;
    if output.status.success() {
        let value = String::from_utf8(output.stdout).ok()?;
        return match value.trim() {
            "1" | "true" | "YES" => Some(true),
            "0" | "false" | "NO" => Some(false),
            _ => None,
        };
    }
    let error = String::from_utf8(output.stderr).ok()?;
    error.contains("does not exist").then_some(false)
}

#[cfg(not(target_os = "macos"))]
const fn system_reduce_motion() -> Option<bool> {
    Some(true)
}

fn state_event_affects_tray(event: &StateChangedEventDto) -> bool {
    event.areas.iter().any(|area| {
        matches!(
            area,
            StateArea::Routes
                | StateArea::Route
                | StateArea::Balance
                | StateArea::Proxy
                | StateArea::MenuBar
        )
    })
}

#[tauri::command]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri command state injection requires State<T> by value"
)]
fn get_bootstrap_snapshot(state: State<'_, Arc<AppRuntimeState>>) -> BootstrapSnapshotDto {
    state.bootstrap_snapshot()
}

#[tauri::command]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri command state injection requires State<T> by value"
)]
fn get_application_update_snapshot(
    coordinator: State<'_, Arc<ApplicationUpdateCoordinator>>,
) -> ApplicationUpdateSnapshotDto {
    coordinator.snapshot()
}

#[tauri::command]
async fn check_application_update(
    coordinator: State<'_, Arc<ApplicationUpdateCoordinator>>,
) -> Result<ApplicationUpdateSnapshotDto, router_core::state::IpcErrorDto> {
    coordinator.check_manual().await
}

#[tauri::command]
async fn download_and_install_application_update(
    coordinator: State<'_, Arc<ApplicationUpdateCoordinator>>,
    on_progress: Channel<ApplicationUpdateProgressDto>,
) -> Result<ApplicationUpdateSnapshotDto, router_core::state::IpcErrorDto> {
    coordinator.download_and_install(on_progress).await
}

#[tauri::command]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri command state injection requires State<T> by value"
)]
fn open_application_update_release(
    coordinator: State<'_, Arc<ApplicationUpdateCoordinator>>,
) -> Result<(), router_core::state::IpcErrorDto> {
    coordinator.open_release()
}

#[tauri::command]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri command state injection requires State<T> by value"
)]
fn restart_for_application_update(
    coordinator: State<'_, Arc<ApplicationUpdateCoordinator>>,
) -> Result<(), router_core::state::IpcErrorDto> {
    coordinator.request_restart()
}

fn open_project_repository_with<E>(
    opener: impl FnOnce(&str) -> Result<(), E>,
) -> Result<(), IpcErrorDto> {
    opener(PROJECT_REPOSITORY_URL).map_err(|_| IpcErrorDto {
        code: "project_repository_open_failed".to_owned(),
        message: "GitHub 项目无法打开。".to_owned(),
        retryable: true,
        field: None,
    })
}

#[tauri::command]
fn open_project_repository() -> Result<(), IpcErrorDto> {
    open_project_repository_with(|url| {
        tauri_plugin_opener::open_url(url, None::<&str>).map_err(|_| ())
    })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
/// Starts the desktop application event loop.
///
/// # Panics
///
/// Panics when Tauri cannot initialize or run the application context.
pub fn run() {
    let context = tauri::generate_context!();
    let acceptance_root = QaAcceptanceRoot::from_environment(&context.config().identifier)
        .unwrap_or_else(|error| panic!("invalid QA acceptance root: {error}"));
    if let Some(root) = &acceptance_root {
        root.prepare_runtime_directories()
            .unwrap_or_else(|error| panic!("invalid QA acceptance directories: {error}"));
    }
    let acceptance_log_dir = acceptance_root.as_ref().map(QaAcceptanceRoot::log_dir);
    let builder = tauri::Builder::default();
    #[cfg(target_os = "macos")]
    let builder = builder.plugin(tauri_nspanel::init());
    let app = builder
        .plugin(tauri_plugin_single_instance::init(
            |app, _arguments, _working_directory| activate_existing_instance(app),
        ))
        .plugin(tauri_plugin_positioner::init())
        .plugin(
            tauri_plugin_opener::Builder::new()
                .open_js_links_on_click(false)
                .build(),
        )
        .plugin(
            tauri_plugin_updater::Builder::new()
                .target("darwin-aarch64")
                .build(),
        )
        .plugin(runtime_log_bootstrap_plugin(acceptance_log_dir.clone()))
        .plugin(runtime_log_plugin(acceptance_log_dir))
        .on_tray_icon_event(handle_tray_event)
        .on_window_event(handle_window_event)
        .setup(move |app| setup_application(app, acceptance_root.as_ref()))
        .invoke_handler(tauri::generate_handler![
            get_bootstrap_snapshot,
            get_application_update_snapshot,
            check_application_update,
            download_and_install_application_update,
            open_application_update_release,
            open_project_repository,
            restart_for_application_update,
            get_menu_snapshot,
            get_settings_snapshot,
            get_usage_history,
            get_usage_statistics,
            get_usage_route_options,
            get_usage_request_detail,
            get_recovery_snapshot,
            create_recovery_point,
            restore_recovery_point,
            start_over_database,
            retry_database_startup,
            get_route_edit,
            save_route,
            delete_route,
            preview_route_activation,
            confirm_route_activation,
            dismiss_codex_restart_notice,
            dismiss_mcp_image_capacity_warning,
            set_fallback_enabled,
            update_balance_query_settings,
            update_appearance_preference,
            update_menu_bar_settings,
            update_images_generation_settings,
            update_mcp_image_capacity_threshold,
            reorder_routes_and_fallback,
            refresh_balance,
            refresh_all_balances,
            test_balance_query,
            check_route_reachability,
            apply_proxy_port,
            connect_codex,
            reconnect_codex,
            preview_codex_images_mcp_repair,
            confirm_codex_images_mcp_repair,
            restore_codex,
            preview_update_codex_recovery,
            confirm_update_codex_recovery,
            preview_reset_codex_recovery_to_baseline,
            confirm_reset_codex_recovery_to_baseline,
            clear_request_history,
            mark_first_run_presented,
            open_codex_config,
            open_mcp_image_directory,
            clear_mcp_images,
            open_runtime_log_directory,
            clear_runtime_logs,
            show_settings_window,
            quit_application,
            menu_frontend_ready,
            complete_menu_show,
            set_menu_usage_preview,
            hide_menu,
            hide_settings_window
        ])
        .build(context)
        .expect("failed to build AI Router");
    run_event_loop(app);
}

fn run_event_loop(app: tauri::App) {
    let shutdown_requested = Arc::new(AtomicBool::new(false));
    app.run(move |app_handle, event| match event {
        #[cfg(target_os = "macos")]
        RunEvent::Reopen { .. } => activate_existing_instance(app_handle),
        RunEvent::ExitRequested { api, code, .. }
            if !shutdown_requested.swap(true, Ordering::AcqRel) =>
        {
            api.prevent_exit();
            app_handle.state::<Arc<TrayRefreshCoordinator>>().shutdown();
            let app_handle = app_handle.clone();
            let coordinator = app_handle.state::<Arc<AppCoordinator>>().inner().clone();
            tauri::async_runtime::spawn(async move {
                let report = coordinator.shutdown().await;
                if !report.balance_graceful || !report.proxy_graceful || !report.database_graceful {
                    app_handle
                        .state::<RuntimeLogController>()
                        .log_fixed(log::Level::Error, "code=shutdown_budget_exhausted");
                }
                if code == Some(APPLICATION_UPDATE_RESTART_REQUEST_CODE) {
                    app_handle.request_restart();
                } else {
                    app_handle.exit(code.unwrap_or(0));
                }
            });
        }
        _ => {}
    });
}

fn setup_application(
    app: &mut tauri::App,
    acceptance_root: Option<&QaAcceptanceRoot>,
) -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(target_os = "macos")]
    {
        app.set_activation_policy(tauri::ActivationPolicy::Accessory);
        popover::initialize_menu_panel(app.handle())?;
    }
    let profile = DesktopRuntimeProfile::from_identifier(&app.config().identifier);
    let app_data_dir = acceptance_root
        .as_ref()
        .map_or_else(|| app.path().app_data_dir(), |root| Ok(root.app_data_dir()))?;
    if let Some(root) = &acceptance_root {
        root.write_runtime_marker(
            std::process::id(),
            &app.config().identifier,
            &std::env::current_exe()?,
        )?;
    }
    finish_runtime_log_setup(app.handle());
    let app_name = app.package_info().name.clone();
    let tray_assets = TrayIconAssets::decode()?;
    let tray_icon = tray_assets.ready.clone();
    let tray_builder = tauri::tray::TrayIconBuilder::with_id("main")
        .icon(tray_icon)
        .icon_as_template(true)
        .tooltip(format!("{app_name} 正在启动"))
        .show_menu_on_left_click(false);
    let tray_builder = if profile.is_isolated() {
        tray_builder.title(initial_tray_title(true))
    } else {
        tray_builder
    };
    tray_builder.build(app)?;
    let tray_refresh = Arc::new(TrayRefreshCoordinator::new(
        tray_assets,
        system_reduce_motion(),
    ));
    app.manage(Arc::clone(&tray_refresh));
    let sink = Arc::new(TauriStateEventSink {
        app_handle: app.handle().clone(),
        tray_refresh: Arc::clone(&tray_refresh),
    });
    let runtime_state = Arc::new(AppRuntimeState::new(sink));
    app.manage(runtime_state.clone());
    app.manage(MenuPopoverController::new());
    let logs = app.state::<RuntimeLogController>();
    let diagnostics = Arc::new(SafeRuntimeDiagnosticSink::new(&logs));
    let user_home = app.path().home_dir()?;
    let activity_sink: Arc<dyn LogicalRequestActivitySink> =
        Arc::new(TauriLogicalRequestActivitySink {
            app_handle: app.handle().clone(),
            tray_refresh,
        });
    let services = DesktopLifecycleServices::new_with_activity_sink(
        app_data_dir,
        &user_home,
        profile,
        runtime_state.clone(),
        diagnostics,
        activity_sink,
    );
    app.manage(services.clone());
    let update_coordinator = ApplicationUpdateCoordinator::new(
        app.handle().clone(),
        runtime_state.clone(),
        acceptance_root.is_some() && profile.is_isolated(),
    );
    app.manage(update_coordinator.clone());
    let coordinator = AppCoordinator::new(services.clone(), runtime_state);
    app.manage(coordinator.clone());
    let app_handle = app.handle().clone();
    tauri::async_runtime::spawn(async move {
        let snapshot = coordinator.start().await;
        if matches!(
            snapshot.phase,
            AppLifecyclePhase::DatabaseError
                | AppLifecyclePhase::RecoveryRequired
                | AppLifecyclePhase::PortConflict
                | AppLifecyclePhase::ProxyError
        ) {
            let message = format!("code=startup_failed phase={:?}", snapshot.phase);
            app_handle
                .state::<RuntimeLogController>()
                .log_fixed(log::Level::Error, &message);
            activate_existing_instance(&app_handle);
        } else if snapshot.issue == Some(AppLifecycleIssue::BalanceStartupFailed) {
            app_handle
                .state::<RuntimeLogController>()
                .log_fixed(log::Level::Error, "code=balance_startup_failed");
        }
        if snapshot.phase == AppLifecyclePhase::Running {
            update_coordinator.start_automatic_scheduler(services);
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn activity_transition(
        phase: LogicalRequestActivityPhase,
        revision: u64,
    ) -> LogicalRequestActivityTransition {
        LogicalRequestActivityTransition {
            phase,
            active: phase != LogicalRequestActivityPhase::Idle,
            count: usize::from(phase != LogicalRequestActivityPhase::Idle),
            revision,
        }
    }

    #[test]
    fn tray_refresh_revision_never_regresses() {
        let refresh = TrayRefreshCoordinator::new(
            TrayIconAssets::decode().expect("tray assets"),
            Some(false),
        );
        let first = refresh.request().expect("first refresh");
        let second = refresh.request().expect("second refresh");

        assert!(refresh.is_current(second));
        assert!(!refresh.is_current(first));

        refresh.shutdown();
        assert!(refresh.request().is_none());
        assert!(!refresh.is_current(second));
    }

    #[test]
    fn tray_activity_phase_changes_invalidate_animation_and_monitor_synchronously() {
        let refresh = TrayRefreshCoordinator::new(
            TrayIconAssets::decode().expect("tray assets"),
            Some(false),
        );
        refresh.set_activity_animation_enabled(true);
        assert!(
            refresh.apply_activity_transition(activity_transition(
                LogicalRequestActivityPhase::Live,
                3,
            ))
        );
        let live = refresh.transition_projection(TrayVisualState::Active);
        assert!(live.animated);
        assert!(refresh.animation_is_current(live.generation));
        let live_monitor = refresh
            .begin_reduce_motion_monitor(3)
            .expect("live monitor");
        assert!(refresh.monitor_is_current(3, live_monitor));

        assert!(!refresh.apply_activity_transition(activity_transition(
            LogicalRequestActivityPhase::Waiting,
            2,
        )));
        assert_eq!(refresh.phase(), LogicalRequestActivityPhase::Live);
        assert!(refresh.animation_is_current(live.generation));
        assert!(refresh.monitor_is_current(3, live_monitor));

        assert!(refresh.apply_activity_transition(activity_transition(
            LogicalRequestActivityPhase::Waiting,
            4,
        )));
        assert_eq!(refresh.phase(), LogicalRequestActivityPhase::Waiting);
        assert!(!refresh.animation_is_current(live.generation));
        assert!(!refresh.monitor_is_current(3, live_monitor));
        let waiting = refresh.transition_projection(TrayVisualState::Waiting);
        assert!(waiting.changed);
        assert!(!waiting.animated);
        assert!(!refresh.animation_is_current(waiting.generation));

        assert!(
            refresh.apply_activity_transition(activity_transition(
                LogicalRequestActivityPhase::Idle,
                5,
            ))
        );
        assert_eq!(refresh.phase(), LogicalRequestActivityPhase::Idle);
        assert!(!refresh.live_activity_is_current(5));

        assert!(
            refresh.apply_activity_transition(activity_transition(
                LogicalRequestActivityPhase::Live,
                6,
            ))
        );
        assert!(refresh.live_activity_is_current(6));
        refresh.shutdown();
        assert!(!refresh.live_activity_is_current(6));
    }

    #[test]
    fn tray_animation_generation_cancels_stale_ticks_and_reduce_motion_is_static() {
        let refresh = TrayRefreshCoordinator::new(
            TrayIconAssets::decode().expect("tray assets"),
            Some(false),
        );
        refresh.set_activity_animation_enabled(true);
        let active = refresh.transition_projection(TrayVisualState::Active);
        assert!(active.changed);
        assert!(active.animated);
        assert!(refresh.animation_is_current(active.generation));

        let idle = refresh.transition_projection(TrayVisualState::Ready);
        assert!(idle.changed);
        assert!(!refresh.animation_is_current(active.generation));

        let waiting = refresh.transition_projection(TrayVisualState::Waiting);
        assert!(waiting.changed);
        assert!(!waiting.animated);
        assert!(!refresh.animation_is_current(waiting.generation));

        let restarted = refresh.transition_projection(TrayVisualState::Active);
        assert!(refresh.animation_is_current(restarted.generation));

        assert_eq!(
            refresh.observe_reduce_motion(Some(true)),
            Some((true, true))
        );
        let reduced = refresh.transition_projection(TrayVisualState::Active);
        assert!(reduced.changed);
        assert!(!reduced.animated);
        assert!(!refresh.animation_is_current(reduced.generation));

        refresh.observe_reduce_motion(Some(false));
        let before_shutdown = refresh.transition_projection(TrayVisualState::Active);
        assert!(refresh.animation_is_current(before_shutdown.generation));
        refresh.shutdown();
        assert!(!refresh.animation_is_current(before_shutdown.generation));
    }

    #[test]
    fn tray_activity_animation_setting_is_fail_closed_and_invalidates_active_generation() {
        let refresh = TrayRefreshCoordinator::new(
            TrayIconAssets::decode().expect("tray assets"),
            Some(false),
        );
        let unloaded = refresh.transition_projection(TrayVisualState::Active);
        assert!(!unloaded.animated);

        assert!(refresh.set_activity_animation_enabled(true));
        let enabled = refresh.transition_projection(TrayVisualState::Active);
        assert!(enabled.animated);
        assert!(refresh.animation_is_current(enabled.generation));

        assert!(refresh.set_activity_animation_enabled(false));
        assert!(!refresh.animation_is_current(enabled.generation));
        let disabled = refresh.transition_projection(TrayVisualState::Active);
        assert!(!disabled.animated);
    }

    #[test]
    fn tray_monitor_generation_deduplicates_setting_toggles_and_stale_observations() {
        let refresh = TrayRefreshCoordinator::new(
            TrayIconAssets::decode().expect("tray assets"),
            Some(false),
        );
        assert!(refresh.set_activity_animation_enabled(true));
        assert!(
            refresh.apply_activity_transition(activity_transition(
                LogicalRequestActivityPhase::Live,
                1,
            ))
        );
        let first = refresh
            .begin_reduce_motion_monitor(1)
            .expect("first live monitor");

        assert!(refresh.set_activity_animation_enabled(false));
        assert!(!refresh.monitor_is_current(1, first));
        assert!(refresh.set_activity_animation_enabled(true));
        let second = refresh
            .begin_reduce_motion_monitor(1)
            .expect("replacement live monitor");
        assert_ne!(first, second);
        assert!(refresh.monitor_is_current(1, second));
        assert_eq!(
            refresh.observe_reduce_motion_if_current(1, first, Some(true)),
            None
        );
        assert!(!refresh.reduce_motion());

        assert!(refresh.apply_activity_transition(activity_transition(
            LogicalRequestActivityPhase::Waiting,
            2,
        )));
        assert!(!refresh.monitor_is_current(1, second));
        assert!(refresh.begin_reduce_motion_monitor(2).is_none());
    }

    #[test]
    fn tray_title_setting_preserves_only_qa_identity_when_unloaded_or_disabled() {
        use router_core::app_api::MenuBarSettingsDto;

        let enabled = Some(MenuBarSettingsDto {
            status_text_enabled: true,
            activity_animation_enabled: true,
        });
        let disabled = Some(MenuBarSettingsDto {
            status_text_enabled: false,
            activity_animation_enabled: true,
        });
        assert_eq!(
            project_tray_title("Route($1)".to_owned(), None, false),
            None
        );
        assert_eq!(
            project_tray_title("Route($1)".to_owned(), None, true),
            Some("QA".to_owned())
        );
        assert_eq!(
            project_tray_title("Route($1)".to_owned(), enabled, false),
            Some("Route($1)".to_owned())
        );
        assert_eq!(
            project_tray_title("QA · Route($1)".to_owned(), enabled, true),
            Some("QA · Route($1)".to_owned())
        );
        assert_eq!(
            project_tray_title("Route($1)".to_owned(), disabled, false),
            None
        );
        assert_eq!(
            project_tray_title("QA · Route($1)".to_owned(), disabled, true),
            Some("QA".to_owned())
        );
    }

    #[test]
    fn tray_reduce_motion_unknown_and_failed_reads_preserve_static_safe_state() {
        let refresh =
            TrayRefreshCoordinator::new(TrayIconAssets::decode().expect("tray assets"), None);
        assert!(refresh.reduce_motion());
        assert_eq!(refresh.observe_reduce_motion(None), None);
        assert!(refresh.reduce_motion());
        assert_eq!(
            refresh.observe_reduce_motion(Some(false)),
            Some((false, true))
        );
        assert!(!refresh.reduce_motion());
        assert_eq!(refresh.observe_reduce_motion(None), None);
        assert!(!refresh.reduce_motion());
    }

    #[test]
    fn tray_animation_uses_the_approved_four_frame_sequence() {
        let mut frame = 0;
        let mut sequence = vec![frame];
        for _ in 0..4 {
            frame = next_active_frame_index(frame);
            sequence.push(frame);
        }
        assert_eq!(sequence, [0, 1, 2, 3, 0]);
        assert_eq!(TRAY_ANIMATION_FRAME_INTERVAL, Duration::from_millis(300));
    }

    #[test]
    fn tray_refresh_filters_to_native_presentation_areas() {
        let event = |areas| StateChangedEventDto { revision: 1, areas };

        assert!(state_event_affects_tray(&event(vec![StateArea::Balance])));
        assert!(state_event_affects_tray(&event(vec![StateArea::Proxy])));
        assert!(!state_event_affects_tray(&event(vec![
            StateArea::CodexConnection,
            StateArea::RequestHistorySummary,
        ])));
    }

    #[test]
    fn application_update_restart_uses_an_interceptable_exit_intent() {
        assert_ne!(
            APPLICATION_UPDATE_RESTART_REQUEST_CODE,
            tauri::RESTART_EXIT_CODE
        );
    }

    #[test]
    fn project_repository_command_uses_the_fixed_canonical_target() {
        let mut opened_url = None;

        open_project_repository_with(|url| {
            opened_url = Some(url.to_owned());
            Ok::<(), ()>(())
        })
        .expect("fixed project repository target should open");

        assert_eq!(opened_url.as_deref(), Some(PROJECT_REPOSITORY_URL));
        assert_eq!(
            PROJECT_REPOSITORY_URL,
            "https://github.com/Angry3D/ai-router"
        );
    }

    #[test]
    fn project_repository_command_maps_opener_failures_to_a_safe_error() {
        let error = open_project_repository_with(|_| Err::<(), ()>(()))
            .expect_err("opener failure should be contained");

        assert_eq!(error.code, "project_repository_open_failed");
        assert_eq!(error.message, "GitHub 项目无法打开。");
        assert!(error.retryable);
        assert_eq!(error.field, None);
    }
}
