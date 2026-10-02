use std::{
    path::PathBuf,
    process::Command,
    sync::{Arc, Mutex},
    time::SystemTime,
};

use router_core::{
    proxy::{RuntimeDiagnosticEvent, RuntimeDiagnosticSink},
    runtime_log::{
        LOG_FILE_PREFIX, LOG_MAINTENANCE_INTERVAL, MAX_LOG_FILE_BYTES, MAX_LOG_FILES,
        RuntimeLogMaintenance, format_log_timestamp, format_runtime_diagnostic,
        truncate_log_record,
    },
    state::{AppRuntimeState, IpcErrorDto, MutationResultDto, StateArea},
};
use tauri::{AppHandle, Manager, Runtime, State, plugin::TauriPlugin};
use tauri_plugin_log::{RotationStrategy, Target, TargetKind};

use super::errors::ipc_error;
#[derive(Clone)]
pub struct RuntimeLogController {
    maintenance: RuntimeLogMaintenance,
    write_gate: Arc<Mutex<()>>,
}

impl RuntimeLogController {
    fn new(directory: PathBuf) -> Self {
        Self {
            maintenance: RuntimeLogMaintenance::new(directory),
            write_gate: Arc::new(Mutex::new(())),
        }
    }

    pub fn start_periodic_maintenance(&self) {
        let controller = self.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                tokio::time::sleep(LOG_MAINTENANCE_INTERVAL).await;
                let result = {
                    let _gate = controller
                        .write_gate
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    log::logger().flush();
                    controller.maintenance.maintain(
                        SystemTime::now(),
                        Some(&controller.maintenance.active_log_path()),
                    )
                };
                if result.is_err() {
                    controller.log_fixed(log::Level::Error, "code=runtime_log_maintenance_failed");
                }
            }
        });
    }

    fn clear(&self) -> Result<(), IpcErrorDto> {
        let active = self.maintenance.active_log_path();
        {
            let _gate = self
                .write_gate
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            log::logger().flush();
            self.maintenance
                .clear(&active)
                .map_err(|_| ipc_error("runtime_log_clear_failed", "运行日志清除失败。", true))?;
        }
        self.log_fixed(log::Level::Info, "code=runtime_logs_cleared");
        Ok(())
    }

    pub(crate) fn directory(&self) -> &std::path::Path {
        self.maintenance.directory()
    }

    pub fn log_fixed(&self, level: log::Level, message: &str) {
        let message = truncate_log_record(message);
        let _gate = self
            .write_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        log::log!(target: "ai_router::runtime", level, "{message}");
    }
}

pub struct SafeRuntimeDiagnosticSink {
    write_gate: Arc<Mutex<()>>,
}

impl SafeRuntimeDiagnosticSink {
    pub fn new(logs: &RuntimeLogController) -> Self {
        Self {
            write_gate: Arc::clone(&logs.write_gate),
        }
    }
}

impl RuntimeDiagnosticSink for SafeRuntimeDiagnosticSink {
    fn emit(&self, event: RuntimeDiagnosticEvent) {
        let line = format_runtime_diagnostic(&event);
        let _gate = self
            .write_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        log::info!(target: "ai_router::diagnostic", "{line}");
    }
}
pub fn runtime_log_bootstrap_plugin<R: Runtime>(directory: Option<PathBuf>) -> TauriPlugin<R> {
    tauri::plugin::Builder::new("runtime-log-bootstrap")
        .setup(move |app, _api| {
            let directory = directory
                .clone()
                .map_or_else(|| app.path().app_log_dir(), Result::<_, tauri::Error>::Ok)?;
            let controller = RuntimeLogController::new(directory);
            controller
                .maintenance
                .maintain(SystemTime::now(), None)
                .map_err(|error| tauri::Error::Io(std::io::Error::other(error.to_string())))?;
            app.manage(controller);
            Ok(())
        })
        .build()
}

pub fn runtime_log_plugin<R: Runtime>(directory: Option<PathBuf>) -> TauriPlugin<R> {
    let target = directory.map_or_else(
        || {
            Target::new(TargetKind::LogDir {
                file_name: Some(LOG_FILE_PREFIX.to_owned()),
            })
        },
        |path| {
            Target::new(TargetKind::Folder {
                path,
                file_name: Some(LOG_FILE_PREFIX.to_owned()),
            })
        },
    );
    tauri_plugin_log::Builder::new()
        .targets([target])
        .level(log::LevelFilter::Info)
        .max_file_size(u128::from(MAX_LOG_FILE_BYTES))
        .rotation_strategy(RotationStrategy::KeepSome(MAX_LOG_FILES - 1))
        .format(|out, message, record| {
            let message = truncate_log_record(&message.to_string());
            out.finish(format_args!(
                "[{}][{}][{}] {}",
                format_log_timestamp(SystemTime::now()),
                record.level(),
                record.target(),
                message
            ));
        })
        .build()
}

pub fn finish_runtime_log_setup<R: Runtime>(app: &AppHandle<R>) {
    if let Some(logs) = app.try_state::<RuntimeLogController>() {
        if logs.maintenance.secure_active_file().is_err() {
            logs.log_fixed(log::Level::Error, "code=runtime_log_permissions_failed");
        }
        logs.start_periodic_maintenance();
    }
}

#[tauri::command]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri command state injection requires State<T> by value"
)]
pub fn open_runtime_log_directory(
    logs: State<'_, RuntimeLogController>,
) -> Result<(), IpcErrorDto> {
    #[cfg(target_os = "macos")]
    {
        Command::new("open")
            .arg(logs.directory())
            .spawn()
            .map_err(|_| ipc_error("runtime_log_open_failed", "日志目录打开失败。", true))?;
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = logs;
        Err(ipc_error(
            "runtime_log_open_unsupported",
            "当前平台不支持打开日志目录。",
            false,
        ))
    }
}

#[tauri::command]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri command state injection requires State<T> by value"
)]
pub fn clear_runtime_logs(
    logs: State<'_, RuntimeLogController>,
    runtime: State<'_, Arc<AppRuntimeState>>,
) -> Result<MutationResultDto, IpcErrorDto> {
    logs.clear()?;
    Ok(runtime.publish_background_change(vec![StateArea::RuntimeLogs]))
}
