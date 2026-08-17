mod application_update;
mod popover;
mod runtime;

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

use application_update::{APPLICATION_UPDATE_RESTART_REQUEST_CODE, ApplicationUpdateCoordinator};
use popover::{
    MenuPopoverController, complete_menu_show, handle_tray_event, handle_window_event, hide_menu,
    hide_settings_window, initial_tray_title, menu_frontend_ready, set_menu_usage_preview,
    update_tray_status,
};
use router_core::app_api::{ApplicationUpdateProgressDto, ApplicationUpdateSnapshotDto};
use router_core::lifecycle::{AppCoordinator, AppLifecycleIssue, AppLifecyclePhase};
use router_core::qa_acceptance::QaAcceptanceRoot;
use router_core::state::{
    AppRuntimeState, BootstrapSnapshotDto, StateArea, StateChangedEventDto, StateEventError,
    StateEventSink,
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

struct TauriStateEventSink {
    app_handle: AppHandle,
    tray_refresh: Arc<TrayRefreshCoordinator>,
}

struct TrayRefreshCoordinator {
    latest_revision: AtomicU64,
    refresh_gate: tokio::sync::Mutex<()>,
    apply_gate: Mutex<()>,
}

impl TrayRefreshCoordinator {
    const fn new() -> Self {
        Self {
            latest_revision: AtomicU64::new(0),
            refresh_gate: tokio::sync::Mutex::const_new(()),
            apply_gate: Mutex::new(()),
        }
    }

    fn request(&self, revision: u64) {
        self.latest_revision.fetch_max(revision, Ordering::AcqRel);
    }

    fn is_current(&self, revision: u64) -> bool {
        self.latest_revision.load(Ordering::Acquire) == revision
    }
}

impl StateEventSink for TauriStateEventSink {
    fn publish(&self, event: &StateChangedEventDto) -> Result<(), StateEventError> {
        self.app_handle
            .emit(STATE_CHANGED_EVENT, event)
            .map_err(|_| StateEventError)?;
        if state_event_affects_tray(event) {
            self.refresh_tray_status(event.revision);
        }
        Ok(())
    }
}

impl TauriStateEventSink {
    fn refresh_tray_status(&self, revision: u64) {
        self.tray_refresh.request(revision);
        let app_handle = self.app_handle.clone();
        let refresh = Arc::clone(&self.tray_refresh);
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
            let _apply_guard = refresh
                .apply_gate
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if refresh.is_current(revision) {
                update_tray_status(
                    &app_handle,
                    &snapshot,
                    balance.as_ref(),
                    services.is_isolated(),
                );
            }
        });
    }
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
    let tray_icon = tauri::image::Image::from_bytes(include_bytes!("../icons/tray-route.png"))?;
    let tray_builder = tauri::tray::TrayIconBuilder::with_id("main")
        .icon(tray_icon)
        .icon_as_template(true)
        .tooltip(format!("{app_name} 正在启动"))
        .title(initial_tray_title(profile.is_isolated()))
        .show_menu_on_left_click(false);
    tray_builder.build(app)?;
    let sink = Arc::new(TauriStateEventSink {
        app_handle: app.handle().clone(),
        tray_refresh: Arc::new(TrayRefreshCoordinator::new()),
    });
    let runtime_state = Arc::new(AppRuntimeState::new(sink));
    app.manage(runtime_state.clone());
    app.manage(MenuPopoverController::new());
    let logs = app.state::<RuntimeLogController>();
    let diagnostics = Arc::new(SafeRuntimeDiagnosticSink::new(&logs));
    let user_home = app.path().home_dir()?;
    let services = DesktopLifecycleServices::new(
        app_data_dir,
        &user_home,
        profile,
        runtime_state.clone(),
        diagnostics,
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
        let refresh = TrayRefreshCoordinator::new();
        refresh.request(3);
        refresh.request(2);

        assert!(refresh.is_current(3));
        assert!(!refresh.is_current(2));
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
}
