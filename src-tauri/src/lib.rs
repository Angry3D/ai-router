mod application_update;
mod popover;
mod runtime;

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
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
use router_core::proxy::{LogicalRequestActivitySink, LogicalRequestActivityTransition};
use router_core::qa_acceptance::QaAcceptanceRoot;
use router_core::state::{
    AppRuntimeState, BootstrapSnapshotDto, IpcErrorDto, StateArea, StateChangedEventDto,
    StateEventError, StateEventSink,
};
use runtime::{
    DesktopLifecycleServices, DesktopRuntimeProfile, RuntimeLogController,
    SafeRuntimeDiagnosticSink, activate_existing_instance, apply_proxy_port,
    check_route_reachability, clear_request_history, clear_runtime_logs,
    confirm_codex_images_mcp_repair, confirm_reset_codex_recovery_to_baseline,
    confirm_route_activation, confirm_update_codex_recovery, connect_codex, create_recovery_point,
    delete_route, dismiss_codex_restart_notice, finish_runtime_log_setup, get_menu_snapshot,
    get_recovery_snapshot, get_route_edit, get_settings_snapshot, get_usage_history,
    get_usage_request_detail, get_usage_route_options, get_usage_statistics,
    mark_first_run_presented, open_codex_config, open_runtime_log_directory,
    preview_codex_images_mcp_repair, preview_reset_codex_recovery_to_baseline,
    preview_route_activation, preview_update_codex_recovery, quit_application, reconnect_codex,
    refresh_all_balances, refresh_balance, reorder_routes_and_fallback, restore_codex,
    restore_recovery_point, retry_database_startup, runtime_log_bootstrap_plugin,
    runtime_log_plugin, save_route, set_fallback_enabled, show_settings_window,
    start_over_database, test_balance_query, update_appearance_preference,
    update_balance_query_settings, update_images_generation_settings,
};
use tauri::{AppHandle, Emitter, Manager, RunEvent, State, ipc::Channel};

const STATE_CHANGED_EVENT: &str = "router-state-changed";
const PROJECT_REPOSITORY_URL: &str = "https://github.com/Angry3D/ai-router";

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
    reduce_motion: AtomicBool,
    assets: TrayIconAssets,
}

impl TrayRefreshCoordinator {
    fn new(assets: TrayIconAssets, reduce_motion: bool) -> Self {
        Self {
            latest_revision: AtomicU64::new(0),
            refresh_gate: tokio::sync::Mutex::const_new(()),
            reduce_motion_gate: tokio::sync::Mutex::const_new(()),
            apply_gate: Mutex::new(()),
            activity: Mutex::new(TrayActivityProjection::default()),
            animation: Mutex::new(TrayAnimationProjection::default()),
            reduce_motion: AtomicBool::new(reduce_motion),
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
        let mut activity = self
            .activity
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if transition.revision <= activity.revision {
            return false;
        }
        activity.revision = transition.revision;
        activity.active = transition.active;
        true
    }

    fn active(&self) -> bool {
        self.activity
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .active
    }

    fn set_reduce_motion(&self, reduce_motion: bool) -> bool {
        self.reduce_motion.swap(reduce_motion, Ordering::AcqRel) != reduce_motion
    }

    fn reduce_motion(&self) -> bool {
        self.reduce_motion.load(Ordering::Acquire)
    }

    fn transition_projection(&self, visual_state: TrayVisualState) -> TrayProjectionDecision {
        let animated = visual_state == TrayVisualState::Active && !self.reduce_motion();
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

    fn invalidate_animation(&self) {
        let _apply_guard = self
            .apply_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut animation = self
            .animation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !animation.shutdown {
            animation.generation = animation.generation.saturating_add(1);
            animation.mode = None;
        }
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
    }
}

#[derive(Default)]
struct TrayActivityProjection {
    active: bool,
    revision: u64,
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
    active_a: tauri::image::Image<'static>,
    active_b: tauri::image::Image<'static>,
}

impl TrayIconAssets {
    fn decode() -> tauri::Result<Self> {
        Ok(Self {
            ready: decode_tray_icon(include_bytes!("../icons/tray-route.png"))?,
            active_static: decode_tray_icon(include_bytes!("../icons/tray-active-static.png"))?,
            active_a: decode_tray_icon(include_bytes!("../icons/tray-active-a.png"))?,
            active_b: decode_tray_icon(include_bytes!("../icons/tray-active-b.png"))?,
        })
    }

    fn static_image(&self, visual_state: TrayVisualState) -> tauri::image::Image<'static> {
        match visual_state {
            TrayVisualState::Ready => self.ready.clone(),
            TrayVisualState::Active => self.active_static.clone(),
        }
    }
}

fn decode_tray_icon(bytes: &'static [u8]) -> tauri::Result<tauri::image::Image<'static>> {
    tauri::image::Image::from_bytes(bytes)
}

impl StateEventSink for TauriStateEventSink {
    fn publish(&self, event: &StateChangedEventDto) -> Result<(), StateEventError> {
        self.app_handle
            .emit(STATE_CHANGED_EVENT, event)
            .map_err(|_| StateEventError)?;
        if state_event_affects_tray(event) {
            let _ = schedule_tray_refresh(self.app_handle.clone(), Arc::clone(&self.tray_refresh));
        }
        Ok(())
    }
}

struct TauriLogicalRequestActivitySink {
    app_handle: AppHandle,
    tray_refresh: Arc<TrayRefreshCoordinator>,
}

impl LogicalRequestActivitySink for TauriLogicalRequestActivitySink {
    fn activity_changed(&self, transition: LogicalRequestActivityTransition) {
        if self.tray_refresh.apply_activity_transition(transition) {
            if transition.active {
                self.tray_refresh.invalidate_animation();
                self.tray_refresh.set_reduce_motion(true);
                if !schedule_tray_refresh(self.app_handle.clone(), Arc::clone(&self.tray_refresh)) {
                    return;
                }
                let app_handle = self.app_handle.clone();
                let refresh = Arc::clone(&self.tray_refresh);
                tauri::async_runtime::spawn(async move {
                    let _ = refresh_system_reduce_motion(&refresh).await;
                    let _ = schedule_tray_refresh(app_handle, refresh);
                });
                return;
            }
            let _ = schedule_tray_refresh(self.app_handle.clone(), Arc::clone(&self.tray_refresh));
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
            refresh.active(),
        );
        let _apply_guard = refresh
            .apply_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if refresh.can_apply(revision) {
            apply_tray_projection(&app_handle, &refresh, presentation);
        }
    });
    true
}

fn apply_tray_projection(
    app_handle: &AppHandle,
    refresh: &Arc<TrayRefreshCoordinator>,
    presentation: TrayPresentation,
) {
    let decision = refresh.transition_projection(presentation.visual_state);
    let Some(tray) = app_handle.tray_by_id("main") else {
        return;
    };
    if decision.changed {
        let image = if decision.animated {
            refresh.assets.active_a.clone()
        } else {
            refresh.assets.static_image(presentation.visual_state)
        };
        let _ = tray.set_icon_with_as_template(Some(image), true);
    }
    let _ = tray.set_title(Some(presentation.title));
    let _ = tray.set_tooltip(Some(presentation.tooltip));

    if decision.changed && decision.animated {
        start_tray_animation(app_handle.clone(), Arc::clone(refresh), decision.generation);
    }
}

fn start_tray_animation(
    app_handle: AppHandle,
    refresh: Arc<TrayRefreshCoordinator>,
    generation: u64,
) {
    tauri::async_runtime::spawn(async move {
        let mut frame_b = true;
        let mut ticks_until_motion_check = 1_u8;
        loop {
            tokio::time::sleep(Duration::from_millis(600)).await;
            if !refresh.animation_is_current(generation) {
                return;
            }
            if ticks_until_motion_check == 1 {
                let (reduce_motion, changed) = refresh_system_reduce_motion(&refresh).await;
                ticks_until_motion_check = 5;
                if changed {
                    let _ = schedule_tray_refresh(app_handle.clone(), Arc::clone(&refresh));
                }
                if reduce_motion || !refresh.animation_is_current(generation) {
                    return;
                }
            } else {
                ticks_until_motion_check -= 1;
            }
            let _apply_guard = refresh
                .apply_gate
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !refresh.animation_is_current(generation) {
                return;
            }
            if let Some(tray) = app_handle.tray_by_id("main") {
                let image = if frame_b {
                    refresh.assets.active_b.clone()
                } else {
                    refresh.assets.active_a.clone()
                };
                let _ = tray.set_icon_with_as_template(Some(image), true);
            }
            frame_b = !frame_b;
        }
    });
}

async fn refresh_system_reduce_motion(refresh: &Arc<TrayRefreshCoordinator>) -> (bool, bool) {
    let _query_guard = refresh.reduce_motion_gate.lock().await;
    let reduce_motion = tauri::async_runtime::spawn_blocking(system_reduce_motion)
        .await
        .ok()
        .flatten()
        .unwrap_or(true);
    let changed = refresh.set_reduce_motion(reduce_motion);
    (reduce_motion, changed)
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
    let error = String::from_utf8_lossy(&output.stderr);
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
            StateArea::Routes | StateArea::Route | StateArea::Balance | StateArea::Proxy
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
            set_fallback_enabled,
            update_balance_query_settings,
            update_appearance_preference,
            update_images_generation_settings,
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
        .title(initial_tray_title(profile.is_isolated()))
        .show_menu_on_left_click(false);
    tray_builder.build(app)?;
    let tray_refresh = Arc::new(TrayRefreshCoordinator::new(
        tray_assets,
        system_reduce_motion().unwrap_or(true),
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

    #[test]
    fn tray_refresh_revision_never_regresses() {
        let refresh =
            TrayRefreshCoordinator::new(TrayIconAssets::decode().expect("tray assets"), false);
        let first = refresh.request().expect("first refresh");
        let second = refresh.request().expect("second refresh");

        assert!(refresh.is_current(second));
        assert!(!refresh.is_current(first));

        refresh.shutdown();
        assert!(refresh.request().is_none());
        assert!(!refresh.is_current(second));
    }

    #[test]
    fn tray_activity_rejects_stale_transitions_and_keeps_latest_state() {
        let refresh =
            TrayRefreshCoordinator::new(TrayIconAssets::decode().expect("tray assets"), false);
        assert!(
            refresh.apply_activity_transition(LogicalRequestActivityTransition {
                active: true,
                count: 1,
                revision: 3,
            })
        );
        assert!(
            !refresh.apply_activity_transition(LogicalRequestActivityTransition {
                active: false,
                count: 0,
                revision: 2,
            })
        );
        assert!(refresh.active());
    }

    #[test]
    fn tray_animation_generation_cancels_stale_ticks_and_reduce_motion_is_static() {
        let refresh =
            TrayRefreshCoordinator::new(TrayIconAssets::decode().expect("tray assets"), false);
        let active = refresh.transition_projection(TrayVisualState::Active);
        assert!(active.changed);
        assert!(active.animated);
        assert!(refresh.animation_is_current(active.generation));

        let idle = refresh.transition_projection(TrayVisualState::Ready);
        assert!(idle.changed);
        assert!(!refresh.animation_is_current(active.generation));

        let restarted = refresh.transition_projection(TrayVisualState::Active);
        assert!(refresh.animation_is_current(restarted.generation));
        refresh.invalidate_animation();
        assert!(!refresh.animation_is_current(restarted.generation));

        assert!(refresh.set_reduce_motion(true));
        let reduced = refresh.transition_projection(TrayVisualState::Active);
        assert!(reduced.changed);
        assert!(!reduced.animated);
        assert!(!refresh.animation_is_current(reduced.generation));

        refresh.set_reduce_motion(false);
        let before_shutdown = refresh.transition_projection(TrayVisualState::Active);
        assert!(refresh.animation_is_current(before_shutdown.generation));
        refresh.shutdown();
        assert!(!refresh.animation_is_current(before_shutdown.generation));
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
