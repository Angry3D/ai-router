use std::{
    collections::HashMap,
    path::PathBuf,
    process::Command,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use router_core::{
    app_api::{
        BalanceQueryEditDto, BalanceQuerySettingsDto, BalanceTestInputDto, CodexBaselineSummaryDto,
        CodexImagesMcpRepairPreviewDto, CodexModelDto, CodexModelsActivation,
        CodexRecoveryResetPreviewDto, CodexRecoverySummaryDto, CodexRecoveryUpdatePreviewDto,
        CodexRestartNoticeDto, HistorySummaryDto, ImagesGenerationSettingsDto, MenuBarSettingsDto,
        MenuSnapshotDto, MetadataFailureDto, RecoveryCandidateDto, RecoveryHealthDto,
        RecoverySnapshotDto, ReorderRoutesAndFallbackInputDto, ReplaceCodexModelsResult,
        RouteActivationPreviewDto, RouteActivationResultDto, RouteCatalogMode, RouteEditDto,
        RouteSaveInputDto, RouteSaveResultDto, SettingsSnapshotDto,
        UpdateImagesGenerationSettingsInputDto, UsageHistoryPageDto, UsageHistoryQueryDto,
        UsageRequestDetailDto, UsageRouteOptionDto, UsageStatisticsDto, UsageStatisticsQueryDto,
    },
    balance::{
        BalanceCoordinator, BalanceDisplaySnapshot, BalanceExecutor, BalanceQueryConfig,
        BalanceResult, BalanceRouteSource, BalanceTrigger,
    },
    codex_catalog::{
        CodexCatalogError, EffectiveCodexCatalog, LocalCodexCatalog, generate_codex_model_catalog,
    },
    codex_config::{
        CodexConfigError, CodexConfigGuard, CodexConfigService, ConfigOperationResult,
        LocalCodexFilesystem, load_or_create_gateway_token,
    },
    domain::{
        ApiKey, AppearancePreference, BalanceQueryPolicy, BaseUrl, CodexModelValidationError,
        FallbackExcludedModelValidationError, ImagesGenerationTimeout, ReachabilityResult, RouteId,
        ValidationError,
    },
    lifecycle::{
        AppCoordinator, AppLifecycleIssue, AppLifecyclePhase, AppLifecycleServices,
        AppLifecycleSnapshot, LifecycleFailure,
    },
    proxy::{
        ActivatedSkipHealth, AsyncHistoryRecorder, FallbackActivationError, FallbackActivationMode,
        FallbackActivationRequest, FallbackActivator, HealthActivationProof,
        InferenceStatusService, LogicalRequestActivitySink, LogicalRequestActivityTracker,
        ProxyIngressState, ProxyPortError, ProxyPortStore, ProxyServerHandle, ReachabilityProbe,
        RequestTransitionSink, ResponsesForwarder, RouteHealthRegistry, RouteSnapshot,
        RoutingSnapshot, RoutingSnapshotStore, RuntimeDiagnosticEvent, RuntimeDiagnosticSink,
        build_proxy_router, transition_proxy_port_with_listener_replaced,
    },
    qa_acceptance::PRODUCTION_APP_IDENTIFIER,
    recovery::{
        DatabaseStartupClassification, DatabaseStartupIssue, RecoveryCoordinator, RecoveryError,
        RecoveryEventSink, RecoveryFailureCode, RecoveryHealth, RecoveryManager, RecoveryPointId,
        classify_recovery_startup_error, classify_storage_startup_error,
    },
    runtime_log::{
        LOG_FILE_PREFIX, LOG_MAINTENANCE_INTERVAL, MAX_LOG_FILE_BYTES, MAX_LOG_FILES,
        RuntimeLogMaintenance, format_runtime_diagnostic, truncate_log_record,
    },
    state::{
        AppRuntimeState, FallbackStateDto, IpcErrorDto, MutationResultDto, RouteSummaryDto,
        RuntimeProjectionUpdate, StateArea,
    },
    storage::{
        AppSettingsRecord, BalanceQueryInput, CodexModelRecord, CodexRestartNoticeRecord,
        CreateRouteInput, DeleteRouteResult, StorageError, UpdateRouteInput,
        normalize_codex_model_records, normalize_fallback_excluded_models,
    },
    storage::{DatabaseExecutor, SecretStore, SqliteBalanceRouteSource, SqliteSecretStore},
};
use tauri::{AppHandle, Emitter, Manager, Runtime, State, plugin::TauriPlugin};
use tauri_plugin_log::{RotationStrategy, Target, TargetKind};

#[cfg(test)]
struct ReplaceCodexModelsInput {
    models: Vec<CodexModelDto>,
    retry_token: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DesktopRuntimeProfile {
    Production,
    Isolated,
}

const STARTUP_STATE_AREAS: [StateArea; 5] = [
    StateArea::Routes,
    StateArea::Route,
    StateArea::Fallback,
    StateArea::ImagesGeneration,
    StateArea::MenuBar,
];

impl DesktopRuntimeProfile {
    #[must_use]
    pub fn from_identifier(identifier: &str) -> Self {
        if identifier == PRODUCTION_APP_IDENTIFIER {
            Self::Production
        } else {
            Self::Isolated
        }
    }

    #[must_use]
    pub const fn is_isolated(self) -> bool {
        matches!(self, Self::Isolated)
    }

    fn codex_home(self, app_data_dir: &std::path::Path, user_home: &std::path::Path) -> PathBuf {
        match self {
            Self::Production => user_home.join(".codex"),
            Self::Isolated => app_data_dir.join("codex-home"),
        }
    }

    const fn proxy_bind_port(self, configured_port: u16) -> u16 {
        match self {
            Self::Production => configured_port,
            Self::Isolated => 0,
        }
    }
}

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

    fn directory(&self) -> &std::path::Path {
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

struct DesktopRecoveryEventSink {
    runtime_state: Arc<AppRuntimeState>,
}

impl RecoveryEventSink for DesktopRecoveryEventSink {
    fn health_changed(&self, _health: &RecoveryHealth) {
        self.runtime_state
            .publish_background_change(vec![StateArea::Recovery]);
    }

    fn diagnostic(&self, code: RecoveryFailureCode) {
        let code = match code {
            RecoveryFailureCode::PublicationFailed => "recovery_publish_failed",
            RecoveryFailureCode::InventoryUnavailable => "recovery_inventory_unavailable",
        };
        log::error!(target: "ai_router::recovery", "code={code}");
    }
}

pub struct DesktopLifecycleServices {
    app_data_dir: PathBuf,
    codex_home: PathBuf,
    profile: DesktopRuntimeProfile,
    runtime_state: Arc<AppRuntimeState>,
    diagnostics: Arc<dyn RuntimeDiagnosticSink>,
    activity: LogicalRequestActivityTracker,
    database: tokio::sync::Mutex<Option<DatabaseExecutor>>,
    recovery: tokio::sync::Mutex<Option<Arc<RecoveryCoordinator>>>,
    proxy: tokio::sync::Mutex<Option<ProxyServerHandle>>,
    balance: tokio::sync::Mutex<Option<Arc<BalanceCoordinator>>>,
    history: tokio::sync::Mutex<Option<Arc<AsyncHistoryRecorder>>>,
    inference: tokio::sync::Mutex<Option<InferenceStatusService>>,
    ingress: tokio::sync::Mutex<Option<ProxyIngressState>>,
    routing: RoutingSnapshotStore,
    route_health: Arc<RouteHealthRegistry>,
    routing_write_gate: Arc<tokio::sync::Mutex<()>>,
    codex_projection_gate: Arc<tokio::sync::Mutex<()>>,
    balance_settings_write_gate: tokio::sync::Mutex<()>,
    menu_bar_settings_write_gate: tokio::sync::Mutex<()>,
    codex_model_retry: tokio::sync::Mutex<Option<CodexModelRetryPermit>>,
    codex_model_retry_generation: AtomicU64,
    route_activation_permit: tokio::sync::Mutex<Option<RouteActivationPermit>>,
    route_activation_permit_generation: AtomicU64,
    codex_images_mcp_repair_permit: tokio::sync::Mutex<Option<CodexImagesMcpRepairPermit>>,
    codex_images_mcp_repair_permit_generation: AtomicU64,
    codex_recovery_update_permit: tokio::sync::Mutex<Option<CodexConfigGuardPermit>>,
    codex_recovery_update_permit_generation: AtomicU64,
    codex_recovery_reset_permit: tokio::sync::Mutex<Option<CodexConfigGuardPermit>>,
    codex_recovery_reset_permit_generation: AtomicU64,
}

struct CodexConfigGuardPermit {
    token: String,
    guard: CodexConfigGuard,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CodexModelRetryKind {
    CatalogPublicationConnected,
    CatalogPublicationOnly,
    CatalogCleanup,
    ConfigProjection,
}

struct CodexModelRetryPermit {
    token: String,
    models: Vec<CodexModelRecord>,
    kind: CodexModelRetryKind,
    config_guard: Option<CodexConfigGuard>,
}

struct RouteActivationPermit {
    token: String,
    current_route_id: Option<RouteId>,
    target_route_id: RouteId,
    selection_generation: u64,
    source_fingerprint: String,
    target_fingerprint: String,
    connected_config_guard: Option<CodexConfigGuard>,
}

struct CodexImagesMcpRepairPermit {
    token: String,
    config_guard: CodexConfigGuard,
    proxy_port: u16,
    gateway_token: String,
    models: Vec<CodexModelRecord>,
    catalog_path: Option<PathBuf>,
}

impl DesktopLifecycleServices {
    #[cfg(test)]
    pub fn new(
        app_data_dir: PathBuf,
        user_home: &std::path::Path,
        profile: DesktopRuntimeProfile,
        runtime_state: Arc<AppRuntimeState>,
        diagnostics: Arc<dyn RuntimeDiagnosticSink>,
    ) -> Arc<Self> {
        Self::new_with_activity_sink(
            app_data_dir,
            user_home,
            profile,
            runtime_state,
            diagnostics,
            Arc::new(router_core::proxy::NoopLogicalRequestActivitySink),
        )
    }

    pub fn new_with_activity_sink(
        app_data_dir: PathBuf,
        user_home: &std::path::Path,
        profile: DesktopRuntimeProfile,
        runtime_state: Arc<AppRuntimeState>,
        diagnostics: Arc<dyn RuntimeDiagnosticSink>,
        activity_sink: Arc<dyn LogicalRequestActivitySink>,
    ) -> Arc<Self> {
        let codex_home = profile.codex_home(&app_data_dir, user_home);
        let route_health = Arc::new(RouteHealthRegistry::new(
            Arc::new(router_core::proxy::SystemMonotonicClock::default()),
            runtime_state.clone(),
        ));
        Arc::new(Self {
            app_data_dir,
            codex_home,
            profile,
            runtime_state,
            diagnostics,
            activity: LogicalRequestActivityTracker::new(activity_sink),
            database: tokio::sync::Mutex::new(None),
            recovery: tokio::sync::Mutex::new(None),
            proxy: tokio::sync::Mutex::new(None),
            balance: tokio::sync::Mutex::new(None),
            history: tokio::sync::Mutex::new(None),
            inference: tokio::sync::Mutex::new(None),
            ingress: tokio::sync::Mutex::new(None),
            routing: RoutingSnapshotStore::default(),
            route_health,
            routing_write_gate: Arc::new(tokio::sync::Mutex::new(())),
            codex_projection_gate: Arc::new(tokio::sync::Mutex::new(())),
            balance_settings_write_gate: tokio::sync::Mutex::new(()),
            menu_bar_settings_write_gate: tokio::sync::Mutex::new(()),
            codex_model_retry: tokio::sync::Mutex::new(None),
            codex_model_retry_generation: AtomicU64::new(0),
            route_activation_permit: tokio::sync::Mutex::new(None),
            route_activation_permit_generation: AtomicU64::new(0),
            codex_images_mcp_repair_permit: tokio::sync::Mutex::new(None),
            codex_images_mcp_repair_permit_generation: AtomicU64::new(0),
            codex_recovery_update_permit: tokio::sync::Mutex::new(None),
            codex_recovery_update_permit_generation: AtomicU64::new(0),
            codex_recovery_reset_permit: tokio::sync::Mutex::new(None),
            codex_recovery_reset_permit_generation: AtomicU64::new(0),
        })
    }

    fn proxy_ingress(
        &self,
        gateway_token: &str,
        forwarder: ResponsesForwarder,
        history: Arc<AsyncHistoryRecorder>,
    ) -> ProxyIngressState {
        ProxyIngressState::new(gateway_token, Arc::new(forwarder))
            .with_runtime_sinks(history, self.diagnostics.clone())
            .with_activity_tracker(self.activity.clone())
            .with_routing_store(self.routing.clone())
            .with_mcp_image_asset_root(self.app_data_dir.join("mcp-images"))
    }

    async fn database(&self) -> Result<DatabaseExecutor, LifecycleFailure> {
        self.database
            .lock()
            .await
            .clone()
            .ok_or(LifecycleFailure::Database)
    }

    async fn database_for_ipc(&self) -> Result<DatabaseExecutor, IpcErrorDto> {
        self.database
            .lock()
            .await
            .clone()
            .ok_or_else(|| ipc_error("database_unavailable", "数据库尚未就绪。", true))
    }

    pub async fn application_update_database(&self) -> Option<DatabaseExecutor> {
        self.database.lock().await.clone()
    }

    async fn recovery_for_ipc(&self) -> Result<Arc<RecoveryCoordinator>, IpcErrorDto> {
        self.recovery
            .lock()
            .await
            .clone()
            .ok_or_else(|| ipc_error("recovery_unavailable", "恢复服务尚未就绪。", true))
    }

    fn recovery_manager(&self) -> RecoveryManager {
        RecoveryManager::new(self.app_data_dir.join("router.sqlite3"))
    }

    async fn route_summaries(
        &self,
        database: &DatabaseExecutor,
    ) -> Result<Vec<RouteSummaryDto>, IpcErrorDto> {
        let routes = database.list_routes().await.map_err(map_storage_error)?;
        let inference = self.inference.lock().await.clone();
        routes
            .into_iter()
            .map(|route| {
                let base_url = BaseUrl::parse(&route.base_url)
                    .map_err(|error| map_validation_error(&error))?;
                Ok(RouteSummaryDto {
                    inference_status: inference.as_ref().map_or_else(
                        || router_core::domain::InferenceStatus {
                            kind: router_core::domain::InferenceStatusKind::Unverified,
                            last_outcome: None,
                            failure_reason: None,
                            observed_at_ms: None,
                        },
                        |service| service.status(&route.route_id, now_millis()),
                    ),
                    route_id: route.route_id.clone(),
                    name: route.name,
                    base_url_host: base_url.host(),
                    health: self.route_health.snapshot(&route.route_id).map(Into::into),
                })
            })
            .collect()
    }

    async fn refresh_route_projection(
        &self,
        database: &DatabaseExecutor,
    ) -> Result<MutationResultDto, IpcErrorDto> {
        self.refresh_route_projection_with_areas(database, Vec::new())
            .await
    }

    async fn refresh_route_projection_with_areas(
        &self,
        database: &DatabaseExecutor,
        mut extra_areas: Vec<StateArea>,
    ) -> Result<MutationResultDto, IpcErrorDto> {
        let routes = self.route_summaries(database).await?;
        let routing = self.load_routing_snapshot(database).await?;
        let active_route_id = routing.active.as_ref().map(|route| route.route_id.clone());
        let fallback = fallback_state(&routing)?;
        let ingress = self.ingress.lock().await.clone();
        if let Some(ingress) = ingress {
            ingress.set_routing_snapshot(Arc::clone(&routing));
        } else {
            self.routing.store(Arc::clone(&routing));
        }
        let mut areas = vec![
            StateArea::Routes,
            StateArea::Route,
            StateArea::Fallback,
            StateArea::ImagesGeneration,
        ];
        areas.append(&mut extra_areas);
        let ((), mutation) = self.runtime_state.apply_committed::<_, IpcErrorDto>(
            Ok(()),
            areas,
            RuntimeProjectionUpdate {
                routes: Some(routes),
                active_route_id: Some(active_route_id),
                fallback: Some(fallback),
                proxy_status: None,
                appearance_preference: None,
                menu_bar_settings: None,
            },
        )?;
        Ok(mutation)
    }

    #[cfg(test)]
    async fn refresh_fallback_projection(
        &self,
        database: &DatabaseExecutor,
    ) -> Result<MutationResultDto, IpcErrorDto> {
        let routing = self.load_routing_snapshot(database).await?;
        let fallback = fallback_state(&routing)?;
        let ingress = self.ingress.lock().await.clone();
        if let Some(ingress) = ingress {
            ingress.set_routing_snapshot(Arc::clone(&routing));
        } else {
            self.routing.store(routing);
        }
        let ((), mutation) = self.runtime_state.apply_committed::<_, IpcErrorDto>(
            Ok(()),
            vec![StateArea::Fallback],
            RuntimeProjectionUpdate {
                fallback: Some(fallback),
                ..RuntimeProjectionUpdate::default()
            },
        )?;
        Ok(mutation)
    }

    #[allow(clippy::too_many_lines)]
    async fn load_routing_snapshot(
        &self,
        database: &DatabaseExecutor,
    ) -> Result<Arc<RoutingSnapshot>, IpcErrorDto> {
        let routes = database.list_routes().await.map_err(map_storage_error)?;
        let state = database.routing_state().await.map_err(map_storage_error)?;
        let settings = database.app_settings().await.map_err(map_storage_error)?;
        let fallback_exclusions = database
            .all_fallback_excluded_models()
            .await
            .map_err(map_storage_error)?;
        let participant_count = usize::try_from(state.fallback.participant_count)
            .map_err(|_| map_storage_error(StorageError::Initialization))?;
        if participant_count > routes.len() {
            return Err(map_storage_error(StorageError::Initialization));
        }
        let secrets = SqliteSecretStore::new(database.clone());
        let mut participants = Vec::new();
        for route in routes.iter().take(participant_count) {
            let api_key = secrets
                .get(route.secret_id.clone())
                .await
                .map_err(map_storage_error)?;
            participants.push(Arc::new(RouteSnapshot {
                route_id: route.route_id.clone(),
                name: route.name.clone(),
                base_url: BaseUrl::parse(&route.base_url)
                    .map_err(|error| map_validation_error(&error))?,
                api_key: Arc::new(api_key),
                service_tier_policy: route.service_tier_policy,
                fallback_excluded_models: Arc::new(
                    fallback_exclusions
                        .get(&route.route_id)
                        .cloned()
                        .unwrap_or_default()
                        .into_iter()
                        .collect(),
                ),
            }));
        }
        let active = if let Some(active_route_id) = state.active_route_id.as_ref() {
            if let Some(route) = participants
                .iter()
                .find(|route| &route.route_id == active_route_id)
            {
                Some(Arc::clone(route))
            } else if let Some(route) = routes
                .iter()
                .find(|route| &route.route_id == active_route_id)
            {
                let api_key = secrets
                    .get(route.secret_id.clone())
                    .await
                    .map_err(map_storage_error)?;
                Some(Arc::new(RouteSnapshot {
                    route_id: route.route_id.clone(),
                    name: route.name.clone(),
                    base_url: BaseUrl::parse(&route.base_url)
                        .map_err(|error| map_validation_error(&error))?,
                    api_key: Arc::new(api_key),
                    service_tier_policy: route.service_tier_policy,
                    fallback_excluded_models: Arc::new(
                        fallback_exclusions
                            .get(&route.route_id)
                            .cloned()
                            .unwrap_or_default()
                            .into_iter()
                            .collect(),
                    ),
                }))
            } else {
                None
            }
        } else {
            None
        };
        let images_route =
            if let Some(images_route_id) = settings.images_generation_route_id.as_ref() {
                if let Some(route) = participants
                    .iter()
                    .chain(active.iter())
                    .find(|route| &route.route_id == images_route_id)
                {
                    Some(Arc::clone(route))
                } else if let Some(route) = routes
                    .iter()
                    .find(|route| &route.route_id == images_route_id)
                {
                    let api_key = secrets
                        .get(route.secret_id.clone())
                        .await
                        .map_err(map_storage_error)?;
                    Some(Arc::new(RouteSnapshot {
                        route_id: route.route_id.clone(),
                        name: route.name.clone(),
                        base_url: BaseUrl::parse(&route.base_url)
                            .map_err(|error| map_validation_error(&error))?,
                        api_key: Arc::new(api_key),
                        service_tier_policy: route.service_tier_policy,
                        fallback_excluded_models: Arc::new(
                            fallback_exclusions
                                .get(&route.route_id)
                                .cloned()
                                .unwrap_or_default()
                                .into_iter()
                                .collect(),
                        ),
                    }))
                } else {
                    return Err(map_storage_error(StorageError::Initialization));
                }
            } else {
                None
            };
        Ok(Arc::new(RoutingSnapshot {
            active,
            enabled: state.fallback.enabled && participants.len() >= 2,
            participants,
            selection_generation: state.selection_generation,
            health_generation: self.route_health.health_generation(),
            config_revision: state.fallback.config_revision,
            images_generation_enabled: settings.images_generation_enabled,
            images_route,
            images_generation_timeout: settings.images_generation_timeout.duration(),
        }))
    }

    async fn codex_context(&self) -> Result<(DatabaseExecutor, u16, String), IpcErrorDto> {
        let database = self.database_for_ipc().await?;
        let settings = database.app_settings().await.map_err(map_storage_error)?;
        let token = load_or_create_gateway_token(&database)
            .await
            .map_err(|_| ipc_error("gateway_token_unavailable", "本地网关令牌不可用。", false))?;
        Ok((database, settings.proxy_port, token))
    }

    async fn codex_status(
        &self,
    ) -> Result<router_core::codex_config::CodexConfigStatus, IpcErrorDto> {
        let (database, port, token) = self.codex_context().await?;
        let models = database
            .active_codex_models()
            .await
            .map_err(map_storage_error)?;
        Ok(self
            .codex_status_with_guard(&database, port, &token, &models)
            .await
            .0)
    }

    async fn codex_status_with_guard(
        &self,
        database: &DatabaseExecutor,
        port: u16,
        token: &str,
        models: &[CodexModelRecord],
    ) -> (
        router_core::codex_config::CodexConfigStatus,
        Option<CodexConfigGuard>,
    ) {
        let catalog = LocalCodexCatalog::new(self.app_data_dir.clone());
        let catalog_path = (!models.is_empty()).then(|| catalog.path());
        let images_generation_enabled = database
            .app_settings()
            .await
            .is_ok_and(|settings| settings.images_generation_enabled);
        let service = CodexConfigService::new(
            database.clone(),
            LocalCodexFilesystem::new(self.codex_home.clone()),
        )
        .with_images_generation_enabled(images_generation_enabled);
        let (status, guard) = service
            .status_with_catalog_guard(port, token, catalog_path.as_deref())
            .await;
        if status == router_core::codex_config::CodexConfigStatus::Connected
            && !models.is_empty()
            && !catalog.matches(models).unwrap_or(false)
        {
            (router_core::codex_config::CodexConfigStatus::Changed, None)
        } else {
            (status, guard)
        }
    }

    fn publish_codex_catalog(
        &self,
        models: &[CodexModelRecord],
    ) -> Result<Option<PathBuf>, CodexCatalogError> {
        let catalog = LocalCodexCatalog::new(self.app_data_dir.clone());
        if models.is_empty() {
            catalog.remove()?;
            Ok(None)
        } else {
            catalog.publish(models).map(Some)
        }
    }

    async fn issue_codex_model_retry(
        &self,
        models: Vec<CodexModelRecord>,
        kind: CodexModelRetryKind,
        config_guard: Option<CodexConfigGuard>,
    ) -> String {
        let generation = self
            .codex_model_retry_generation
            .fetch_add(1, Ordering::AcqRel)
            .saturating_add(1);
        let token = format!("codex-model-retry-{generation}");
        *self.codex_model_retry.lock().await = Some(CodexModelRetryPermit {
            token: token.clone(),
            models,
            kind,
            config_guard,
        });
        token
    }

    async fn consume_codex_model_retry(
        &self,
        token: Option<&str>,
        models: &[CodexModelRecord],
    ) -> Option<CodexModelRetryPermit> {
        let mut permit = self.codex_model_retry.lock().await;
        let matches = permit
            .as_ref()
            .is_some_and(|permit| Some(permit.token.as_str()) == token && permit.models == models);
        if matches { permit.take() } else { None }
    }

    #[must_use]
    pub const fn is_isolated(&self) -> bool {
        self.profile.is_isolated()
    }

    pub async fn active_balance_snapshot(
        &self,
        route_id: Option<&RouteId>,
    ) -> Option<BalanceDisplaySnapshot> {
        let route_id = route_id?;
        let balance = self.balance.lock().await.clone()?;
        Some(balance.route_snapshot(route_id))
    }

    pub async fn menu_snapshot(&self) -> Result<MenuSnapshotDto, IpcErrorDto> {
        let bootstrap = self.runtime_state.bootstrap_snapshot();
        let database = self.database_for_ipc().await?;
        let balance_enabled_route_ids = SqliteBalanceRouteSource::new(database.clone())
            .eligible_route_ids()
            .await
            .map_err(map_balance_error)?;
        let balance = self.balance.lock().await.clone();
        let balances = balance.as_ref().map_or_else(Vec::new, |balance| {
            bootstrap
                .routes
                .iter()
                .map(|route| balance.route_snapshot(&route.route_id))
                .collect()
        });
        let balance_batch = balance.and_then(|balance| balance.batch_snapshot());
        let codex_restart_notice = if let Some(notice) = database
            .codex_restart_notice()
            .await
            .map_err(map_storage_error)?
        {
            bootstrap
                .routes
                .iter()
                .find(|route| route.route_id == notice.route_id)
                .map(|route| CodexRestartNoticeDto {
                    notice_id: notice.notice_id,
                    route_name: route.name.clone(),
                })
        } else {
            None
        };
        Ok(MenuSnapshotDto {
            bootstrap,
            balances,
            balance_enabled_route_ids,
            balance_batch,
            codex_status: self.codex_status().await?,
            codex_restart_notice,
        })
    }

    pub async fn settings_snapshot(&self) -> Result<SettingsSnapshotDto, IpcErrorDto> {
        let database = self.database_for_ipc().await?;
        let recovery = self.recovery_for_ipc().await?;
        let settings = database.app_settings().await.map_err(map_storage_error)?;
        let menu_bar = menu_bar_settings_dto(&settings);
        let history = database
            .history_summary()
            .await
            .map_err(map_storage_error)?;
        let baseline = database.codex_baseline().await.map_err(map_storage_error)?;
        let recovery_config = database
            .codex_recovery_config()
            .await
            .map_err(map_storage_error)?;
        let metadata_failure =
            self.history
                .lock()
                .await
                .as_ref()
                .map_or_else(empty_metadata_failure, |recorder| {
                    let snapshot = recorder.failure_snapshot();
                    MetadataFailureDto {
                        dropped_records: snapshot.dropped_records,
                        write_failures: snapshot.write_failures,
                        last_error: snapshot.last_error.map(|code| code.as_str().to_owned()),
                    }
                });
        Ok(SettingsSnapshotDto {
            routes: self.route_summaries(&database).await?,
            active_route_id: database
                .active_route_id()
                .await
                .map_err(map_storage_error)?,
            fallback: self.runtime_state.bootstrap_snapshot().fallback,
            proxy_port: settings.proxy_port,
            codex_status: self.codex_status().await?,
            baseline: baseline.as_ref().map_or(
                CodexBaselineSummaryDto {
                    exists: false,
                    original_exists: None,
                    captured_at_ms: None,
                },
                |baseline| CodexBaselineSummaryDto {
                    exists: true,
                    original_exists: Some(baseline.original_exists),
                    captured_at_ms: Some(baseline.captured_at_ms),
                },
            ),
            original_backup: baseline.as_ref().map_or(
                CodexBaselineSummaryDto {
                    exists: false,
                    original_exists: None,
                    captured_at_ms: None,
                },
                |baseline| CodexBaselineSummaryDto {
                    exists: true,
                    original_exists: Some(baseline.original_exists),
                    captured_at_ms: Some(baseline.captured_at_ms),
                },
            ),
            recovery_config: recovery_config.map_or(
                CodexRecoverySummaryDto {
                    exists: false,
                    original_exists: None,
                    updated_at_ms: None,
                },
                |recovery| CodexRecoverySummaryDto {
                    exists: true,
                    original_exists: Some(recovery.original_exists),
                    updated_at_ms: Some(recovery.updated_at_ms),
                },
            ),
            balance_script_risk_confirmed: settings.balance_script_risk_confirmed,
            balance_query: settings.balance_query_policy.into(),
            images_generation: ImagesGenerationSettingsDto {
                enabled: settings.images_generation_enabled,
                route_id: settings.images_generation_route_id,
                timeout_secs: settings.images_generation_timeout.seconds(),
            },
            history: HistorySummaryDto {
                request_count: history.request_count,
                earliest_started_at_ms: history.earliest_started_at_ms,
                latest_started_at_ms: history.latest_started_at_ms,
                database_bytes: history.database_bytes,
                retention_days: history.retention_days,
            },
            metadata_failure,
            recovery: RecoveryHealthDto::from(&recovery.health()),
            menu_bar,
        })
    }

    pub async fn usage_history(
        &self,
        query: UsageHistoryQueryDto,
    ) -> Result<UsageHistoryPageDto, IpcErrorDto> {
        self.database_for_ipc()
            .await?
            .usage_history(query.into())
            .await
            .map(Into::into)
            .map_err(map_storage_error)
    }

    pub async fn usage_statistics(
        &self,
        query: UsageStatisticsQueryDto,
    ) -> Result<UsageStatisticsDto, IpcErrorDto> {
        self.database_for_ipc()
            .await?
            .usage_statistics(query.into())
            .await
            .map(Into::into)
            .map_err(map_storage_error)
    }

    pub async fn usage_route_options(&self) -> Result<Vec<UsageRouteOptionDto>, IpcErrorDto> {
        self.database_for_ipc()
            .await?
            .usage_route_options()
            .await
            .map(|options| options.into_iter().map(Into::into).collect())
            .map_err(map_storage_error)
    }

    pub async fn usage_request_detail(
        &self,
        request_id: String,
    ) -> Result<UsageRequestDetailDto, IpcErrorDto> {
        self.database_for_ipc()
            .await?
            .usage_request_detail(request_id)
            .await
            .map(Into::into)
            .map_err(|error| match error {
                StorageError::NotFound => {
                    ipc_error("usage_request_not_found", "请求记录不存在。", false)
                }
                other => map_storage_error(other),
            })
    }

    pub async fn recovery_snapshot(
        &self,
        lifecycle: &AppLifecycleSnapshot,
    ) -> Result<RecoverySnapshotDto, IpcErrorDto> {
        let required = lifecycle.phase == AppLifecyclePhase::RecoveryRequired;
        let candidates = if required {
            let manager = self.recovery_manager();
            tokio::task::spawn_blocking(move || manager.scan())
                .await
                .map_err(|_| ipc_error("recovery_inventory_unavailable", "无法读取恢复点。", true))?
                .map_err(|error| map_recovery_error(&error, RecoveryOperation::Inventory))?
                .valid_points
                .into_iter()
                .map(|point| RecoveryCandidateDto {
                    point_id: point.point_id.as_str().to_owned(),
                    created_at_ms: point.created_at_ms,
                })
                .collect()
        } else {
            Vec::new()
        };
        let startup_issue = match lifecycle.issue {
            Some(AppLifecycleIssue::Database(issue)) => Some(issue),
            _ => None,
        };
        let health = self
            .recovery
            .lock()
            .await
            .clone()
            .map(|recovery| RecoveryHealthDto::from(&recovery.health()));
        Ok(RecoverySnapshotDto {
            required,
            can_start_over: required && candidates.is_empty(),
            candidates,
            startup_issue,
            health,
        })
    }

    pub async fn create_recovery_point(&self) -> Result<RecoveryHealthDto, IpcErrorDto> {
        let health = self
            .recovery_for_ipc()
            .await?
            .create_point()
            .await
            .map_err(|error| map_recovery_error(&error, RecoveryOperation::Publish))?;
        Ok(RecoveryHealthDto::from(&health))
    }

    async fn require_recovery_candidate(
        &self,
        point_id: &RecoveryPointId,
    ) -> Result<(), IpcErrorDto> {
        let manager = self.recovery_manager();
        let point_id = point_id.clone();
        let found = tokio::task::spawn_blocking(move || {
            manager.scan().map(|inventory| {
                inventory
                    .valid_points
                    .iter()
                    .any(|point| point.point_id == point_id)
            })
        })
        .await
        .map_err(|_| ipc_error("recovery_inventory_unavailable", "无法读取恢复点。", true))?
        .map_err(|error| map_recovery_error(&error, RecoveryOperation::Inventory))?;
        if found {
            Ok(())
        } else {
            Err(ipc_error(
                "recovery_point_stale",
                "所选恢复点已失效，请刷新后重试。",
                false,
            ))
        }
    }

    async fn require_start_over_available(&self) -> Result<(), IpcErrorDto> {
        let manager = self.recovery_manager();
        let has_valid_point = tokio::task::spawn_blocking(move || {
            manager
                .scan()
                .map(|inventory| !inventory.valid_points.is_empty())
        })
        .await
        .map_err(|_| ipc_error("recovery_inventory_unavailable", "无法读取恢复点。", true))?
        .map_err(|error| map_recovery_error(&error, RecoveryOperation::Inventory))?;
        if has_valid_point {
            Err(ipc_error(
                "database_start_over_not_allowed",
                "仍有可用恢复点，不能创建空数据库。",
                false,
            ))
        } else {
            Ok(())
        }
    }

    pub async fn route_edit(&self, route_id: RouteId) -> Result<RouteEditDto, IpcErrorDto> {
        let database = self.database_for_ipc().await?;
        let edit = database
            .route_edit(route_id)
            .await
            .map_err(map_storage_error)?;
        let base_url =
            BaseUrl::parse(&edit.route.base_url).map_err(|error| map_validation_error(&error))?;
        let api_key = String::from_utf8(edit.api_key.expose().to_vec())
            .map_err(|_| ipc_error("route_key_invalid", "路由 Key 无法读取。", false))?;
        Ok(RouteEditDto {
            route_id: edit.route.route_id,
            name: edit.route.name,
            base_url: base_url.as_str().to_owned(),
            inference_url: base_url.inference_url(),
            api_key,
            service_tier_policy: edit.route.service_tier_policy,
            balance_query: edit.balance_query.map(|query| BalanceQueryEditDto {
                mode: query.mode,
                enabled: query.enabled,
                custom_source: query.custom_source,
            }),
            fallback_excluded_models: edit.fallback_excluded_models,
            models: edit.models.into_iter().map(Into::into).collect(),
        })
    }

    #[allow(clippy::too_many_lines)]
    pub async fn save_route(
        &self,
        input: RouteSaveInputDto,
    ) -> Result<RouteSaveResultDto, IpcErrorDto> {
        let database = self.database_for_ipc().await?;
        let api_key =
            ApiKey::parse(&input.api_key).map_err(|error| map_validation_error(&error))?;
        let balance_query = input.balance_query.map(|query| BalanceQueryInput {
            mode: query.mode,
            enabled: query.enabled,
            custom_source: query.custom_source,
        });
        let retry_token = input.retry_token;
        let fallback_excluded_models =
            normalize_fallback_excluded_models(input.fallback_excluded_models)
                .map_err(|error| map_fallback_excluded_model_validation_error(&error))?;
        let creating_route = input.route_id.is_none();
        let candidate =
            normalize_codex_model_records(input.models.into_iter().map(Into::into).collect())
                .map_err(|error| map_codex_model_validation_error(&error))?;
        if !candidate.is_empty() {
            generate_codex_model_catalog(&candidate).map_err(map_codex_catalog_error)?;
        }
        let _routing_write = self.routing_write_gate.lock().await;
        let active_before = database
            .active_route_id()
            .await
            .map_err(map_storage_error)?;
        let previous = if input.route_id.as_ref() == active_before.as_ref() {
            database
                .active_codex_models()
                .await
                .map_err(map_storage_error)?
        } else {
            Vec::new()
        };
        let (route_id, route_changed) = if let Some(route_id) = input.route_id {
            let changed = database
                .update_route_with_models_and_fallback_exclusions(
                    UpdateRouteInput {
                        route_id: route_id.clone(),
                        name: input.name,
                        base_url: input.base_url,
                        api_key,
                        service_tier_policy: input.service_tier_policy,
                        balance_query,
                        accept_script_risk: input.accept_script_risk,
                    },
                    candidate.clone(),
                    fallback_excluded_models.clone(),
                )
                .await
                .map_err(map_storage_error)?;
            (route_id, changed)
        } else {
            let route_id = database
                .create_route_with_models_and_fallback_exclusions(
                    CreateRouteInput {
                        name: input.name,
                        base_url: input.base_url,
                        api_key,
                        service_tier_policy: input.service_tier_policy,
                        balance_query,
                        accept_script_risk: input.accept_script_risk,
                    },
                    candidate.clone(),
                    fallback_excluded_models,
                )
                .await
                .map_err(map_storage_error)?
                .route_id;
            (route_id, true)
        };
        if route_changed && let Some(balance) = self.balance.lock().await.as_ref() {
            balance.invalidate_route(&route_id);
        }
        if creating_route {
            self.route_health.advance_generation_and_clear();
        } else if route_changed {
            let routing = self.load_routing_snapshot(&database).await?;
            let participants = routing
                .participants
                .iter()
                .map(|route| route.route_id.clone())
                .collect::<Vec<_>>();
            self.route_health
                .invalidate_route_and_rebase(&route_id, &participants);
        }
        let mutation = self.refresh_route_projection(&database).await?;
        let active_after = database
            .active_route_id()
            .await
            .map_err(map_storage_error)?;
        let catalog = if active_after.as_ref() == Some(&route_id)
            && (previous != candidate || retry_token.is_some())
        {
            self.reconcile_codex_models(previous, candidate, retry_token)
                .await?
        } else {
            inactive_codex_models_result(candidate)
        };
        Ok(RouteSaveResultDto {
            route_id,
            revision: mutation.revision,
            catalog,
        })
    }

    pub async fn delete_route(
        &self,
        route_id: RouteId,
    ) -> Result<(DeleteRouteResult, MutationResultDto), IpcErrorDto> {
        let database = self.database_for_ipc().await?;
        let _routing_write = self.routing_write_gate.lock().await;
        let deleting_active = database
            .active_route_id()
            .await
            .map_err(map_storage_error)?
            .as_ref()
            == Some(&route_id);
        let previous_models = if deleting_active {
            database
                .list_codex_models(route_id.clone())
                .await
                .map_err(map_storage_error)?
        } else {
            Vec::new()
        };
        let result = database
            .delete_route(route_id.clone())
            .await
            .map_err(map_storage_error)?;
        if let Some(balance) = self.balance.lock().await.as_ref() {
            balance.remove_route(&route_id);
        }
        if let Some(inference) = self.inference.lock().await.as_ref() {
            inference.remove_route(&route_id);
        }
        self.route_health.advance_generation_and_clear();
        let mutation = self.refresh_route_projection(&database).await?;
        if deleting_active {
            let _ = self
                .reconcile_codex_models(previous_models, Vec::new(), None)
                .await?;
        }
        Ok((result, mutation))
    }

    #[cfg(test)]
    pub async fn activate_route(
        &self,
        route_id: RouteId,
    ) -> Result<MutationResultDto, IpcErrorDto> {
        let database = self.database_for_ipc().await?;
        let _routing_write = self.routing_write_gate.lock().await;
        database
            .activate_route(route_id.clone())
            .await
            .map_err(map_storage_error)?;
        self.route_health.advance_generation_and_clear();
        let mutation = self.refresh_route_projection(&database).await?;
        if let Some(balance) = self.balance.lock().await.clone() {
            tauri::async_runtime::spawn(async move {
                let _ = balance.trigger_route_change(route_id).await;
            });
        }
        Ok(mutation)
    }

    pub async fn preview_route_activation(
        &self,
        route_id: RouteId,
    ) -> Result<RouteActivationPreviewDto, IpcErrorDto> {
        let database = self.database_for_ipc().await?;
        let routing = database.routing_state().await.map_err(map_storage_error)?;
        let route = database
            .list_routes()
            .await
            .map_err(map_storage_error)?
            .into_iter()
            .find(|route| route.route_id == route_id)
            .ok_or_else(|| ipc_error("route_not_found", "路由不存在。", false))?;
        let source_models = database
            .active_codex_models()
            .await
            .map_err(map_storage_error)?;
        let target_models = database
            .list_codex_models(route_id.clone())
            .await
            .map_err(map_storage_error)?;
        let source_fingerprint = EffectiveCodexCatalog::from_models(source_models.clone())
            .fingerprint()
            .map_err(map_codex_catalog_error)?;
        let target_catalog = EffectiveCodexCatalog::from_models(target_models);
        let target_fingerprint = target_catalog
            .fingerprint()
            .map_err(map_codex_catalog_error)?;
        let settings = database.app_settings().await.map_err(map_storage_error)?;
        let token = load_or_create_gateway_token(&database)
            .await
            .map_err(|_| ipc_error("gateway_token_unavailable", "本地网关令牌不可用。", false))?;
        let (codex_status, connected_config_guard) = self
            .codex_status_with_guard(&database, settings.proxy_port, &token, &source_models)
            .await;
        let confirmation_required = source_fingerprint != target_fingerprint
            && codex_status == router_core::codex_config::CodexConfigStatus::Connected;
        let generation = self
            .route_activation_permit_generation
            .fetch_add(1, Ordering::AcqRel)
            .saturating_add(1);
        let permit = format!("route-activation-{generation}");
        *self.route_activation_permit.lock().await = Some(RouteActivationPermit {
            token: permit.clone(),
            current_route_id: routing.active_route_id,
            target_route_id: route_id.clone(),
            selection_generation: routing.selection_generation,
            source_fingerprint,
            target_fingerprint,
            connected_config_guard,
        });
        Ok(RouteActivationPreviewDto {
            target_route_id: route_id,
            target_route_name: route.name,
            target_catalog_mode: if matches!(target_catalog, EffectiveCodexCatalog::Original) {
                RouteCatalogMode::Original
            } else {
                RouteCatalogMode::Custom
            },
            confirmation_required,
            permit,
        })
    }

    pub async fn confirm_route_activation(
        &self,
        permit_token: String,
    ) -> Result<RouteActivationResultDto, IpcErrorDto> {
        let permit = self
            .route_activation_permit
            .lock()
            .await
            .take()
            .filter(|permit| permit.token == permit_token)
            .ok_or_else(|| {
                ipc_error(
                    "route_activation_permit_invalid",
                    "切换条件已变化，请重新确认。",
                    true,
                )
            })?;
        let database = self.database_for_ipc().await?;
        let _routing_write = self.routing_write_gate.lock().await;
        let routing = database.routing_state().await.map_err(map_storage_error)?;
        let source_models = database
            .active_codex_models()
            .await
            .map_err(map_storage_error)?;
        let target_models = database
            .list_codex_models(permit.target_route_id.clone())
            .await
            .map_err(map_storage_error)?;
        let source_fingerprint = EffectiveCodexCatalog::from_models(source_models.clone())
            .fingerprint()
            .map_err(map_codex_catalog_error)?;
        let target_fingerprint = EffectiveCodexCatalog::from_models(target_models.clone())
            .fingerprint()
            .map_err(map_codex_catalog_error)?;
        let settings = database.app_settings().await.map_err(map_storage_error)?;
        let token = load_or_create_gateway_token(&database)
            .await
            .map_err(|_| ipc_error("gateway_token_unavailable", "本地网关令牌不可用。", false))?;
        let (codex_status, _) = self
            .codex_status_with_guard(&database, settings.proxy_port, &token, &source_models)
            .await;
        let config = CodexConfigService::new(
            database.clone(),
            LocalCodexFilesystem::new(self.codex_home.clone()),
        );
        let connected_config_matches = match permit.connected_config_guard.as_ref() {
            Some(guard) => {
                codex_status == router_core::codex_config::CodexConfigStatus::Connected
                    && config.guard_is_current(guard).unwrap_or(false)
            }
            None => codex_status != router_core::codex_config::CodexConfigStatus::Connected,
        };
        if routing.active_route_id != permit.current_route_id
            || routing.selection_generation != permit.selection_generation
            || source_fingerprint != permit.source_fingerprint
            || target_fingerprint != permit.target_fingerprint
            || !connected_config_matches
        {
            return Err(ipc_error(
                "route_activation_permit_stale",
                "切换条件已变化，请重新确认。",
                true,
            ));
        }
        database
            .activate_route(permit.target_route_id.clone())
            .await
            .map_err(map_storage_error)?;
        self.route_health.advance_generation_and_clear();
        let mutation = self.refresh_route_projection(&database).await?;
        let catalog = if source_fingerprint == target_fingerprint {
            inactive_codex_models_result(target_models)
        } else {
            self.reconcile_codex_models(source_models, target_models, None)
                .await?
        };
        if let Some(balance) = self.balance.lock().await.clone() {
            let route_id = permit.target_route_id;
            tauri::async_runtime::spawn(async move {
                let _ = balance.trigger_route_change(route_id).await;
            });
        }
        Ok(RouteActivationResultDto {
            revision: mutation.revision,
            catalog,
        })
    }

    pub async fn dismiss_codex_restart_notice(
        &self,
        notice_id: String,
    ) -> Result<MutationResultDto, IpcErrorDto> {
        let database = self.database_for_ipc().await?;
        let _ = database
            .dismiss_codex_restart_notice(notice_id)
            .await
            .map_err(map_storage_error)?;
        self.runtime_state
            .publish_background_change(vec![StateArea::CodexRestartNotice]);
        Ok(MutationResultDto {
            revision: self.runtime_state.bootstrap_snapshot().revision,
        })
    }

    pub async fn set_fallback_enabled(
        &self,
        enabled: bool,
    ) -> Result<MutationResultDto, IpcErrorDto> {
        let database = self.database_for_ipc().await?;
        let _routing_write = self.routing_write_gate.lock().await;
        let previous_revision = self.routing.load().config_revision;
        let fallback = database
            .set_fallback_enabled(enabled)
            .await
            .map_err(map_storage_error)?;
        if fallback.config_revision != previous_revision {
            self.route_health.advance_generation_and_clear();
        }
        self.refresh_route_projection(&database).await
    }

    #[cfg(test)]
    pub async fn set_fallback_participant_count(
        &self,
        participant_count: u32,
    ) -> Result<MutationResultDto, IpcErrorDto> {
        let database = self.database_for_ipc().await?;
        let _routing_write = self.routing_write_gate.lock().await;
        let changed = database
            .set_fallback_participant_count(participant_count)
            .await
            .map_err(map_storage_error)?;
        if changed {
            self.route_health.advance_generation_and_clear();
        }
        if !changed {
            let durable_revision = database
                .routing_state()
                .await
                .map_err(map_storage_error)?
                .fallback
                .config_revision;
            if self.routing.load().config_revision != durable_revision {
                return self.refresh_route_projection(&database).await;
            }
            return Ok(MutationResultDto {
                revision: self.runtime_state.bootstrap_snapshot().revision,
            });
        }
        self.refresh_fallback_projection(&database).await
    }

    pub async fn update_balance_query_settings(
        &self,
        input: BalanceQuerySettingsDto,
    ) -> Result<MutationResultDto, IpcErrorDto> {
        let policy =
            BalanceQueryPolicy::try_from(input).map_err(|error| map_validation_error(&error))?;
        let _write = self.balance_settings_write_gate.lock().await;
        let database = self.database_for_ipc().await?;
        let changed = database
            .set_balance_query_policy(policy)
            .await
            .map_err(map_storage_error)?;
        if !changed {
            return Ok(MutationResultDto {
                revision: self.runtime_state.bootstrap_snapshot().revision,
            });
        }
        if let Some(balance) = self.balance.lock().await.clone() {
            balance.update_policy(policy);
        }
        Ok(self
            .runtime_state
            .publish_background_change(vec![StateArea::BalanceSettings]))
    }

    pub async fn update_appearance_preference(
        &self,
        appearance_preference: AppearancePreference,
    ) -> Result<MutationResultDto, IpcErrorDto> {
        let database = self.database_for_ipc().await?;
        let changed = database
            .set_appearance_preference(appearance_preference)
            .await
            .map_err(map_storage_error)?;
        if !changed {
            return Ok(MutationResultDto {
                revision: self.runtime_state.bootstrap_snapshot().revision,
            });
        }
        let ((), mutation) = self.runtime_state.apply_committed::<_, IpcErrorDto>(
            Ok(()),
            vec![StateArea::Appearance],
            RuntimeProjectionUpdate {
                appearance_preference: Some(appearance_preference),
                ..RuntimeProjectionUpdate::default()
            },
        )?;
        Ok(mutation)
    }

    pub async fn update_menu_bar_settings(
        &self,
        input: MenuBarSettingsDto,
    ) -> Result<MutationResultDto, IpcErrorDto> {
        let _write = self.menu_bar_settings_write_gate.lock().await;
        let database = self.database_for_ipc().await?;
        let changed = database
            .set_menu_bar_settings(input.status_text_enabled, input.activity_animation_enabled)
            .await
            .map_err(map_storage_error)?;
        if !changed {
            return Ok(MutationResultDto {
                revision: self.runtime_state.bootstrap_snapshot().revision,
            });
        }
        let ((), mutation) = self.runtime_state.apply_committed::<_, IpcErrorDto>(
            Ok(()),
            vec![StateArea::MenuBar],
            RuntimeProjectionUpdate {
                menu_bar_settings: Some(input),
                ..RuntimeProjectionUpdate::default()
            },
        )?;
        Ok(mutation)
    }

    pub async fn update_images_generation_settings(
        &self,
        input: UpdateImagesGenerationSettingsInputDto,
    ) -> Result<MutationResultDto, IpcErrorDto> {
        let database = self.database_for_ipc().await?;
        let _routing_write = self.routing_write_gate.lock().await;
        let _projection_write = self.codex_projection_gate.lock().await;
        let before = database.app_settings().await.map_err(map_storage_error)?;
        let timeout = ImagesGenerationTimeout::parse(input.timeout_secs)
            .map_err(|error| map_validation_error(&error))?;
        let changed = database
            .set_images_generation_settings(input.enabled, input.route_id, timeout)
            .await
            .map_err(map_storage_error)?;
        if !changed {
            return Ok(MutationResultDto {
                revision: self.runtime_state.bootstrap_snapshot().revision,
            });
        }
        let extra_areas = (before.images_generation_enabled != input.enabled)
            .then_some(StateArea::CodexConnection)
            .into_iter()
            .collect();
        self.refresh_route_projection_with_areas(&database, extra_areas)
            .await
    }

    pub async fn reorder_routes_and_fallback(
        &self,
        input: ReorderRoutesAndFallbackInputDto,
    ) -> Result<MutationResultDto, IpcErrorDto> {
        let database = self.database_for_ipc().await?;
        let _routing_write = self.routing_write_gate.lock().await;
        let changed = database
            .reorder_routes_and_fallback(
                input.ordered_route_ids,
                input.participant_count,
                input.expected_config_revision,
            )
            .await
            .map_err(map_storage_error)?;
        if !changed {
            let durable_revision = database
                .routing_state()
                .await
                .map_err(map_storage_error)?
                .fallback
                .config_revision;
            if self.routing.load().config_revision != durable_revision {
                return self.refresh_route_projection(&database).await;
            }
            return Ok(MutationResultDto {
                revision: self.runtime_state.bootstrap_snapshot().revision,
            });
        }
        self.route_health.advance_generation_and_clear();
        self.refresh_route_projection(&database).await
    }

    pub fn trigger_menu_open_balance_refresh(self: &Arc<Self>) {
        let services = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            let balance = services.balance.lock().await.clone();
            if let Some(balance) = balance {
                let _ = balance.trigger_menu_open().await;
            }
        });
    }

    pub async fn refresh_balance(
        &self,
        route_id: RouteId,
    ) -> Result<router_core::balance::BalanceDisplaySnapshot, IpcErrorDto> {
        let balance = self
            .balance
            .lock()
            .await
            .clone()
            .ok_or_else(|| ipc_error("balance_unavailable", "余额服务尚未就绪。", true))?;
        let _ = balance
            .query_route(route_id.clone(), BalanceTrigger::Explicit)
            .await;
        Ok(balance.route_snapshot(&route_id))
    }

    pub async fn refresh_all_balances(
        &self,
    ) -> Result<router_core::balance::BalanceRefreshBatchState, IpcErrorDto> {
        let balance = self
            .balance
            .lock()
            .await
            .clone()
            .ok_or_else(|| ipc_error("balance_unavailable", "余额服务尚未就绪。", true))?;
        balance.refresh_all().await.map_err(map_balance_error)
    }

    pub async fn test_balance(
        &self,
        input: BalanceTestInputDto,
    ) -> Result<BalanceResult, IpcErrorDto> {
        let api_key =
            ApiKey::parse(&input.api_key).map_err(|error| map_validation_error(&error))?;
        let base_url =
            BaseUrl::parse(&input.base_url).map_err(|error| map_validation_error(&error))?;
        BalanceExecutor::new()
            .map_err(|_| ipc_error("balance_unavailable", "余额服务尚未就绪。", true))?
            .query(
                &BalanceQueryConfig {
                    mode: input.mode,
                    custom_source: input.custom_source,
                },
                &api_key,
                &base_url,
            )
            .await
            .map_err(map_balance_error)
    }

    pub async fn check_reachability(
        &self,
        base_url: String,
    ) -> Result<ReachabilityResult, IpcErrorDto> {
        let probe = ReachabilityProbe::new()
            .map_err(|_| ipc_error("reachability_unavailable", "地址检查暂不可用。", true))?;
        probe
            .check(&base_url)
            .await
            .map_err(|error| map_validation_error(&error))
    }

    pub async fn apply_proxy_port(&self, port: u16) -> Result<bool, IpcErrorDto> {
        if port == 0 {
            return Err(ipc_field_error(
                "proxy_port_invalid",
                "端口必须在 1 到 65535 之间。",
                "port",
            ));
        }
        let _projection_write = self.codex_projection_gate.lock().await;
        let database = self.database_for_ipc().await?;
        let ingress = self.ingress.lock().await.clone();
        let mut proxy = self.proxy.lock().await;
        if let (Some(proxy), Some(ingress)) = (proxy.as_mut(), ingress) {
            transition_proxy_port_with_listener_replaced(
                proxy,
                port,
                build_proxy_router(ingress),
                &DatabaseProxyPortStore(database),
                || self.activity.begin_new_epoch(),
            )
            .await
            .map_err(|error| map_proxy_port_error(&error))?;
            self.runtime_state
                .publish_background_change(vec![StateArea::Proxy, StateArea::CodexConnection]);
            return Ok(false);
        }
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port))
            .await
            .map_err(|_| ipc_error("proxy_port_unavailable", "该端口已被占用。", true))?;
        database
            .set_proxy_port(port)
            .await
            .map_err(map_storage_error)?;
        drop(listener);
        Ok(true)
    }

    pub async fn connect_codex(
        &self,
        allow_without_route: bool,
    ) -> Result<ConfigOperationResult, IpcErrorDto> {
        let _projection_write = self.codex_projection_gate.lock().await;
        let (database, port, token) = self.codex_context().await?;
        if database
            .active_route_id()
            .await
            .map_err(map_storage_error)?
            .is_none()
            && !allow_without_route
        {
            return Err(ipc_error(
                "no_active_route_confirmation_required",
                "当前没有活动路由。",
                false,
            ));
        }
        let models = database
            .active_codex_models()
            .await
            .map_err(map_storage_error)?;
        let catalog = LocalCodexCatalog::new(self.app_data_dir.clone());
        let catalog_path = if models.is_empty() {
            None
        } else {
            Some(catalog.publish(&models).map_err(map_codex_catalog_error)?)
        };
        let images_generation_enabled = database
            .app_settings()
            .await
            .map_err(map_storage_error)?
            .images_generation_enabled;
        let service =
            CodexConfigService::new(database, LocalCodexFilesystem::new(self.codex_home.clone()))
                .with_images_generation_enabled(images_generation_enabled);
        let result = service
            .connect_with_catalog(port, &token, catalog_path.as_deref())
            .await
            .map_err(|error| map_codex_error(&error))?;
        if models.is_empty() {
            catalog.remove().map_err(map_codex_catalog_error)?;
        }
        self.runtime_state
            .publish_background_change(vec![StateArea::CodexConnection]);
        Ok(result)
    }

    pub async fn reconnect_codex(&self) -> Result<ConfigOperationResult, IpcErrorDto> {
        let _projection_write = self.codex_projection_gate.lock().await;
        let (database, port, token) = self.codex_context().await?;
        let models = database
            .active_codex_models()
            .await
            .map_err(map_storage_error)?;
        let catalog = LocalCodexCatalog::new(self.app_data_dir.clone());
        let catalog_path = if models.is_empty() {
            None
        } else {
            Some(catalog.publish(&models).map_err(map_codex_catalog_error)?)
        };
        let images_generation_enabled = database
            .app_settings()
            .await
            .map_err(map_storage_error)?
            .images_generation_enabled;
        let service =
            CodexConfigService::new(database, LocalCodexFilesystem::new(self.codex_home.clone()))
                .with_images_generation_enabled(images_generation_enabled);
        let result = service
            .reconnect_with_catalog(port, &token, catalog_path.as_deref())
            .await
            .map_err(|error| map_codex_error(&error))?;
        if models.is_empty() {
            catalog.remove().map_err(map_codex_catalog_error)?;
        }
        self.runtime_state
            .publish_background_change(vec![StateArea::CodexConnection]);
        Ok(result)
    }

    pub async fn preview_codex_images_mcp_repair(
        &self,
    ) -> Result<CodexImagesMcpRepairPreviewDto, IpcErrorDto> {
        let _routing_write = self.routing_write_gate.lock().await;
        let _projection_write = self.codex_projection_gate.lock().await;
        let (database, port, token) = self.codex_context().await?;
        let settings = database.app_settings().await.map_err(map_storage_error)?;
        if !settings.images_generation_enabled {
            return Err(ipc_error(
                "codex_images_mcp_repair_not_available",
                "当前图片工具配置不支持修复。",
                false,
            ));
        }
        let models = database
            .active_codex_models()
            .await
            .map_err(map_storage_error)?;
        let catalog = LocalCodexCatalog::new(self.app_data_dir.clone());
        let catalog_path = (!models.is_empty()).then(|| catalog.path());
        let service =
            CodexConfigService::new(database, LocalCodexFilesystem::new(self.codex_home.clone()))
                .with_images_generation_enabled(true);
        let config_guard = service
            .preview_images_mcp_repair_with_catalog(port, &token, catalog_path.as_deref())
            .await
            .map_err(|error| map_codex_error(&error))?;
        let generation = self
            .codex_images_mcp_repair_permit_generation
            .fetch_add(1, Ordering::AcqRel)
            .saturating_add(1);
        let permit = format!("codex-images-mcp-repair-{generation}");
        *self.codex_images_mcp_repair_permit.lock().await = Some(CodexImagesMcpRepairPermit {
            token: permit.clone(),
            config_guard,
            proxy_port: port,
            gateway_token: token,
            models,
            catalog_path,
        });
        Ok(CodexImagesMcpRepairPreviewDto { permit })
    }

    pub async fn confirm_codex_images_mcp_repair(
        &self,
        permit_token: String,
    ) -> Result<ConfigOperationResult, IpcErrorDto> {
        let permit = {
            let mut pending = self.codex_images_mcp_repair_permit.lock().await;
            if pending
                .as_ref()
                .is_none_or(|permit| permit.token != permit_token)
            {
                return Err(ipc_error(
                    "codex_images_mcp_repair_permit_invalid",
                    "修复确认已失效，请重新确认。",
                    true,
                ));
            }
            pending.take().ok_or_else(|| {
                ipc_error(
                    "codex_images_mcp_repair_permit_invalid",
                    "修复确认已失效，请重新确认。",
                    true,
                )
            })?
        };
        let _routing_write = self.routing_write_gate.lock().await;
        let _projection_write = self.codex_projection_gate.lock().await;
        let (database, port, token) = self.codex_context().await?;
        let settings = database.app_settings().await.map_err(map_storage_error)?;
        let models = database
            .active_codex_models()
            .await
            .map_err(map_storage_error)?;
        let catalog = LocalCodexCatalog::new(self.app_data_dir.clone());
        let catalog_path = (!models.is_empty()).then(|| catalog.path());
        let service =
            CodexConfigService::new(database, LocalCodexFilesystem::new(self.codex_home.clone()))
                .with_images_generation_enabled(settings.images_generation_enabled);
        let inputs_match = settings.images_generation_enabled
            && port == permit.proxy_port
            && token == permit.gateway_token
            && models == permit.models
            && catalog_path == permit.catalog_path;
        let config_matches = service
            .guard_is_current(&permit.config_guard)
            .unwrap_or(false);
        if !inputs_match || !config_matches {
            return Err(ipc_error(
                "codex_images_mcp_repair_permit_stale",
                "Codex 配置或修复条件已变化，请重新确认。",
                true,
            ));
        }
        let published_catalog_path = if models.is_empty() {
            None
        } else {
            Some(catalog.publish(&models).map_err(map_codex_catalog_error)?)
        };
        let result = service
            .repair_images_mcp_with_catalog_guarded(
                port,
                &token,
                published_catalog_path.as_deref(),
                &permit.config_guard,
            )
            .await
            .map_err(|error| match error {
                CodexConfigError::ChangedDuringOperation
                | CodexConfigError::ImagesMcpRepairNotAllowed => ipc_error(
                    "codex_images_mcp_repair_permit_stale",
                    "Codex 配置或修复条件已变化，请重新确认。",
                    true,
                ),
                _ => map_codex_error(&error),
            })?;
        if models.is_empty() {
            catalog.remove().map_err(map_codex_catalog_error)?;
        }
        self.runtime_state
            .publish_background_change(vec![StateArea::CodexConnection]);
        Ok(result)
    }

    #[allow(clippy::too_many_lines)]
    #[cfg(test)]
    async fn replace_codex_models(
        &self,
        input: ReplaceCodexModelsInput,
    ) -> Result<ReplaceCodexModelsResult, IpcErrorDto> {
        let retry_token = input.retry_token;
        let candidate =
            normalize_codex_model_records(input.models.into_iter().map(Into::into).collect())
                .map_err(|error| map_codex_model_validation_error(&error))?;
        if !candidate.is_empty() {
            generate_codex_model_catalog(&candidate).map_err(map_codex_catalog_error)?;
        }
        let database = self.database_for_ipc().await?;
        let route_id = database
            .active_route_id()
            .await
            .map_err(map_storage_error)?
            .ok_or_else(|| ipc_error("active_route_missing", "当前没有活动路由。", false))?;
        let previous = database
            .list_codex_models(route_id.clone())
            .await
            .map_err(map_storage_error)?;
        let persisted = database
            .replace_codex_models(route_id, candidate)
            .await
            .map_err(map_storage_error)?;
        self.reconcile_codex_models(previous, persisted, retry_token)
            .await
    }

    #[allow(clippy::too_many_lines)]
    async fn reconcile_codex_models(
        &self,
        previous: Vec<CodexModelRecord>,
        persisted: Vec<CodexModelRecord>,
        retry_token: Option<String>,
    ) -> Result<ReplaceCodexModelsResult, IpcErrorDto> {
        let _projection_write = self.codex_projection_gate.lock().await;
        let (database, port, token) = self.codex_context().await?;
        let changed = previous != persisted;
        let retry_permit = self
            .consume_codex_model_retry(retry_token.as_deref(), &persisted)
            .await;
        if retry_token.is_none() {
            *self.codex_model_retry.lock().await = None;
        }
        let catalog = LocalCodexCatalog::new(self.app_data_dir.clone());
        let previous_path = (!previous.is_empty()).then(|| catalog.path());
        let config = CodexConfigService::new(
            database.clone(),
            LocalCodexFilesystem::new(self.codex_home.clone()),
        )
        .with_images_generation_enabled(
            database
                .app_settings()
                .await
                .map_err(map_storage_error)?
                .images_generation_enabled,
        );
        let (previous_status, previous_guard) = config
            .status_with_catalog_guard(port, &token, previous_path.as_deref())
            .await;
        self.runtime_state
            .publish_background_change(vec![StateArea::CodexCatalog, StateArea::CodexConnection]);
        let models = persisted
            .iter()
            .cloned()
            .map(CodexModelDto::from)
            .collect::<Vec<_>>();
        let catalog_path = if persisted.is_empty() {
            None
        } else if let Ok(path) = self.publish_codex_catalog(&persisted) {
            path
        } else {
            let (retry_kind, config_guard) =
                if previous_status == router_core::codex_config::CodexConfigStatus::Connected {
                    (
                        CodexModelRetryKind::CatalogPublicationConnected,
                        previous_guard,
                    )
                } else {
                    (CodexModelRetryKind::CatalogPublicationOnly, None)
                };
            let retry_token = self
                .issue_codex_model_retry(persisted, retry_kind, config_guard)
                .await;
            return Ok(partial_codex_models_result(
                models,
                changed,
                activation_for_status(&previous_status),
                "codex_catalog_publication_failed",
                Some(retry_token),
            ));
        };
        let retry_kind = retry_permit.as_ref().map(|permit| permit.kind);
        let reconcile_guard = if matches!(
            retry_kind,
            Some(
                CodexModelRetryKind::CatalogPublicationConnected
                    | CodexModelRetryKind::ConfigProjection
            )
        ) {
            retry_permit
                .as_ref()
                .and_then(|permit| permit.config_guard.clone())
        } else if previous_status == router_core::codex_config::CodexConfigStatus::Connected {
            previous_guard
        } else {
            None
        };
        let reconcile = reconcile_guard.is_some();
        if reconcile
            && let Err(error) = config
                .reconnect_with_catalog_guarded(
                    port,
                    &token,
                    catalog_path.as_deref(),
                    reconcile_guard.as_ref().expect("guarded reconciliation"),
                )
                .await
        {
            let mapped = map_codex_error(&error);
            let retry_token = if matches!(
                error,
                CodexConfigError::Filesystem(_) | CodexConfigError::Storage(_)
            ) {
                Some(
                    self.issue_codex_model_retry(
                        persisted,
                        CodexModelRetryKind::ConfigProjection,
                        reconcile_guard,
                    )
                    .await,
                )
            } else {
                None
            };
            return Ok(partial_codex_models_result(
                models,
                changed,
                activation_for_codex_error(&error),
                &mapped.code,
                retry_token,
            ));
        }
        let should_remove_catalog = persisted.is_empty()
            && (reconcile || retry_kind == Some(CodexModelRetryKind::CatalogCleanup));
        let cleanup_guard = if retry_kind == Some(CodexModelRetryKind::CatalogCleanup) {
            retry_permit.and_then(|permit| permit.config_guard)
        } else if should_remove_catalog {
            config.status_with_catalog_guard(port, &token, None).await.1
        } else {
            None
        };
        if should_remove_catalog
            && !cleanup_guard
                .as_ref()
                .is_some_and(|guard| config.guard_is_current(guard).unwrap_or(false))
        {
            let final_status = config.status_with_catalog(port, &token, None).await;
            return Ok(partial_codex_models_result(
                models,
                changed,
                activation_for_status(&final_status),
                "codex_config_changed",
                None,
            ));
        }
        if should_remove_catalog
            && let Err(_error) = LocalCodexCatalog::new(self.app_data_dir.clone()).remove()
        {
            let retry_token = self
                .issue_codex_model_retry(
                    persisted,
                    CodexModelRetryKind::CatalogCleanup,
                    cleanup_guard,
                )
                .await;
            return Ok(partial_codex_models_result(
                models,
                changed,
                CodexModelsActivation::RestartCodex,
                "codex_catalog_cleanup_failed",
                Some(retry_token),
            ));
        }
        let final_status = config
            .status_with_catalog(port, &token, catalog_path.as_deref())
            .await;
        Ok(ReplaceCodexModelsResult {
            models,
            changed,
            projection_applied: true,
            retry_required: false,
            activation: activation_for_status(&final_status),
            error_code: None,
            retry_token: None,
        })
    }

    pub async fn restore_codex(&self) -> Result<ConfigOperationResult, IpcErrorDto> {
        let _projection_write = self.codex_projection_gate.lock().await;
        let database = self.database_for_ipc().await?;
        let service =
            CodexConfigService::new(database, LocalCodexFilesystem::new(self.codex_home.clone()));
        let result = service
            .restore()
            .await
            .map_err(|error| map_codex_error(&error))?;
        self.runtime_state
            .publish_background_change(vec![StateArea::CodexConnection]);
        Ok(result)
    }

    pub async fn preview_update_codex_recovery(
        &self,
    ) -> Result<CodexRecoveryUpdatePreviewDto, IpcErrorDto> {
        let _projection_write = self.codex_projection_gate.lock().await;
        if self.codex_status().await? != router_core::codex_config::CodexConfigStatus::NotConnected
        {
            return Err(ipc_error(
                "codex_recovery_not_disconnected",
                "请先断开 Codex 后再更新断开恢复配置。",
                false,
            ));
        }
        let database = self.database_for_ipc().await?;
        let service = CodexConfigService::new(
            database.clone(),
            LocalCodexFilesystem::new(self.codex_home.clone()),
        );
        let preview = service
            .preview_recovery_update()
            .await
            .map_err(|error| map_codex_error(&error))?;
        let generation = self
            .codex_recovery_update_permit_generation
            .fetch_add(1, Ordering::AcqRel)
            .saturating_add(1);
        let permit = format!("codex-recovery-update-{generation}");
        *self.codex_recovery_update_permit.lock().await = Some(CodexConfigGuardPermit {
            token: permit.clone(),
            guard: preview.guard,
        });
        Ok(CodexRecoveryUpdatePreviewDto {
            permit,
            current_exists: preview.current_exists,
            current_unix_mode: preview.current_unix_mode,
            recovery_target_exists: preview.recovery_target_exists,
            bytes_changed: preview.bytes_changed,
            recovery_updated_at_ms: preview.recovery_updated_at_ms,
        })
    }

    pub async fn confirm_update_codex_recovery(
        &self,
        permit_token: String,
    ) -> Result<ConfigOperationResult, IpcErrorDto> {
        let permit = {
            let mut pending = self.codex_recovery_update_permit.lock().await;
            if pending
                .as_ref()
                .is_none_or(|permit| permit.token != permit_token)
            {
                return Err(ipc_error(
                    "codex_recovery_preview_stale",
                    "恢复配置预览已失效，请重新确认。",
                    true,
                ));
            }
            pending.take().ok_or_else(|| {
                ipc_error(
                    "codex_recovery_preview_stale",
                    "恢复配置预览已失效，请重新确认。",
                    true,
                )
            })?
        };
        let _projection_write = self.codex_projection_gate.lock().await;
        if self.codex_status().await? != router_core::codex_config::CodexConfigStatus::NotConnected
        {
            return Err(ipc_error(
                "codex_recovery_not_disconnected",
                "请先断开 Codex 后再更新断开恢复配置。",
                false,
            ));
        }
        let database = self.database_for_ipc().await?;
        let service =
            CodexConfigService::new(database, LocalCodexFilesystem::new(self.codex_home.clone()));
        let result = service
            .update_recovery_guarded(&permit.guard)
            .await
            .map_err(|error| map_codex_error(&error))?;
        self.runtime_state
            .publish_background_change(vec![StateArea::CodexConnection]);
        Ok(result)
    }

    pub async fn preview_reset_codex_recovery_to_baseline(
        &self,
    ) -> Result<CodexRecoveryResetPreviewDto, IpcErrorDto> {
        let _projection_write = self.codex_projection_gate.lock().await;
        if self.codex_status().await? != router_core::codex_config::CodexConfigStatus::NotConnected
        {
            return Err(ipc_error(
                "codex_recovery_not_disconnected",
                "请先断开 Codex 后再恢复首次连接前状态。",
                false,
            ));
        }
        let database = self.database_for_ipc().await?;
        let baseline = database
            .codex_baseline()
            .await
            .map_err(map_storage_error)?
            .ok_or_else(|| ipc_error("codex_baseline_missing", "尚未创建初始配置。", false))?;
        let service =
            CodexConfigService::new(database, LocalCodexFilesystem::new(self.codex_home.clone()));
        let preview = service
            .preview_reset_recovery_to_baseline()
            .await
            .map_err(|error| map_codex_error(&error))?;
        let generation = self
            .codex_recovery_reset_permit_generation
            .fetch_add(1, Ordering::AcqRel)
            .saturating_add(1);
        let permit = format!("codex-recovery-reset-{generation}");
        *self.codex_recovery_reset_permit.lock().await = Some(CodexConfigGuardPermit {
            token: permit.clone(),
            guard: preview.guard,
        });
        Ok(CodexRecoveryResetPreviewDto {
            permit,
            current_exists: preview.current_exists,
            original_exists: baseline.original_exists,
            recovery_target_exists: preview.recovery_target_exists,
        })
    }

    pub async fn confirm_reset_codex_recovery_to_baseline(
        &self,
        permit_token: String,
    ) -> Result<ConfigOperationResult, IpcErrorDto> {
        let permit = {
            let mut pending = self.codex_recovery_reset_permit.lock().await;
            if pending
                .as_ref()
                .is_none_or(|permit| permit.token != permit_token)
            {
                return Err(ipc_error(
                    "codex_recovery_preview_stale",
                    "恢复配置预览已失效，请重新确认。",
                    true,
                ));
            }
            pending.take().ok_or_else(|| {
                ipc_error(
                    "codex_recovery_preview_stale",
                    "恢复配置预览已失效，请重新确认。",
                    true,
                )
            })?
        };
        let _projection_write = self.codex_projection_gate.lock().await;
        if self.codex_status().await? != router_core::codex_config::CodexConfigStatus::NotConnected
        {
            return Err(ipc_error(
                "codex_recovery_not_disconnected",
                "请先断开 Codex 后再恢复首次连接前状态。",
                false,
            ));
        }
        let database = self.database_for_ipc().await?;
        let service =
            CodexConfigService::new(database, LocalCodexFilesystem::new(self.codex_home.clone()));
        let result = service
            .reset_recovery_to_baseline_guarded(&permit.guard)
            .await
            .map_err(|error| map_codex_error(&error))?;
        self.runtime_state
            .publish_background_change(vec![StateArea::CodexConnection]);
        Ok(result)
    }

    pub async fn clear_history(&self) -> Result<MutationResultDto, IpcErrorDto> {
        let database = self.database_for_ipc().await?;
        if let Some(inference) = self.inference.lock().await.as_ref() {
            inference
                .clear_history_and_reset(&database)
                .await
                .map_err(map_storage_error)?;
        } else {
            database.clear_history().await.map_err(map_storage_error)?;
        }
        Ok(self
            .runtime_state
            .publish_background_change(vec![StateArea::Routes, StateArea::RequestHistorySummary]))
    }

    pub async fn mark_first_run_presented(&self) -> Result<(), IpcErrorDto> {
        self.database_for_ipc()
            .await?
            .mark_first_run_presented()
            .await
            .map_err(map_storage_error)
    }

    pub async fn first_run_pending(&self) -> Result<bool, IpcErrorDto> {
        Ok(!self
            .database_for_ipc()
            .await?
            .app_settings()
            .await
            .map_err(map_storage_error)?
            .first_run_presented)
    }

    async fn start_profiled_proxy(
        &self,
        configured_port: u16,
        ingress: ProxyIngressState,
    ) -> Result<ProxyServerHandle, LifecycleFailure> {
        let server = ProxyServerHandle::start(
            self.profile.proxy_bind_port(configured_port),
            build_proxy_router(ingress),
        )
        .await
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::AddrInUse {
                LifecycleFailure::PortConflict
            } else {
                LifecycleFailure::Proxy
            }
        })?;
        if self.profile.is_isolated() {
            self.database()
                .await?
                .set_proxy_port(server.address().port())
                .await
                .map_err(|_| LifecycleFailure::Database)?;
        }
        Ok(server)
    }

    async fn open_and_install_database(
        &self,
        path: PathBuf,
        manager: RecoveryManager,
        force_current_point: bool,
    ) -> Result<(), LifecycleFailure> {
        let database = tokio::task::spawn_blocking(move || DatabaseExecutor::open(path))
            .await
            .map_err(|_| LifecycleFailure::Database)?
            .map_err(|error| map_database_startup_failure(&error))?;
        let recovery = RecoveryCoordinator::start(
            manager,
            database.clone(),
            Arc::new(DesktopRecoveryEventSink {
                runtime_state: Arc::clone(&self.runtime_state),
            }),
        )
        .await;
        if force_current_point {
            let _ = recovery.create_point().await;
        }
        *self.database.lock().await = Some(database);
        *self.recovery.lock().await = Some(recovery);
        Ok(())
    }
}

#[derive(Clone)]
struct FallbackTransitionCoordinator {
    inner: Arc<FallbackTransitionCoordinatorInner>,
}

struct FallbackTransitionCoordinatorInner {
    database: DatabaseExecutor,
    runtime_state: Arc<AppRuntimeState>,
    pending: tokio::sync::Mutex<HashMap<String, PendingFallbackTransition>>,
    notice_generation: AtomicU64,
}

struct PendingFallbackTransition {
    route_id: RouteId,
    route_name: String,
    selection_generation: u64,
    catalog_fingerprint: Option<String>,
    request_terminal: bool,
    created_at: Instant,
}

const MAX_PENDING_FALLBACK_TRANSITIONS: usize = 256;
const PENDING_FALLBACK_TRANSITION_TTL: Duration = Duration::from_mins(5);

impl FallbackTransitionCoordinator {
    fn new(database: DatabaseExecutor, runtime_state: Arc<AppRuntimeState>) -> Self {
        Self {
            inner: Arc::new(FallbackTransitionCoordinatorInner {
                database,
                runtime_state,
                pending: tokio::sync::Mutex::new(HashMap::new()),
                notice_generation: AtomicU64::new(0),
            }),
        }
    }

    async fn record_activation(
        &self,
        request_id: String,
        route_id: RouteId,
        route_name: String,
        selection_generation: u64,
    ) {
        let mut pending = self.inner.pending.lock().await;
        pending.retain(|_, transition| {
            transition.created_at.elapsed() < PENDING_FALLBACK_TRANSITION_TTL
        });
        if !pending.contains_key(&request_id)
            && pending.len() >= MAX_PENDING_FALLBACK_TRANSITIONS
            && let Some(oldest) = pending
                .iter()
                .min_by_key(|(_, transition)| transition.created_at)
                .map(|(request_id, _)| request_id.clone())
        {
            pending.remove(&oldest);
        }
        pending.insert(
            request_id.clone(),
            PendingFallbackTransition {
                route_id,
                route_name,
                selection_generation,
                catalog_fingerprint: None,
                request_terminal: false,
                created_at: Instant::now(),
            },
        );
        drop(pending);

        let coordinator = self.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(PENDING_FALLBACK_TRANSITION_TTL).await;
            let mut pending = coordinator.inner.pending.lock().await;
            if pending.get(&request_id).is_some_and(|transition| {
                transition.selection_generation == selection_generation
                    && transition.created_at.elapsed() >= PENDING_FALLBACK_TRANSITION_TTL
            }) {
                pending.remove(&request_id);
            }
        });
    }

    async fn projection_finished(
        &self,
        request_id: &str,
        selection_generation: u64,
        fingerprint: Option<String>,
    ) {
        {
            let mut pending = self.inner.pending.lock().await;
            let Some(transition) = pending.get_mut(request_id) else {
                return;
            };
            if transition.selection_generation != selection_generation {
                return;
            }
            let Some(fingerprint) = fingerprint else {
                pending.remove(request_id);
                return;
            };
            transition.catalog_fingerprint = Some(fingerprint);
        }
        self.publish_if_ready(request_id).await;
    }

    async fn mark_terminal(&self, request_id: &str) {
        {
            let mut pending = self.inner.pending.lock().await;
            let Some(transition) = pending.get_mut(request_id) else {
                return;
            };
            transition.request_terminal = true;
        }
        self.publish_if_ready(request_id).await;
    }

    async fn publish_if_ready(&self, request_id: &str) {
        let transition = {
            let mut pending = self.inner.pending.lock().await;
            if !pending.get(request_id).is_some_and(|transition| {
                transition.request_terminal && transition.catalog_fingerprint.is_some()
            }) {
                return;
            }
            pending.remove(request_id)
        };
        let Some(transition) = transition else {
            return;
        };
        let current_fingerprint = match self.inner.database.active_codex_models().await {
            Ok(models) => EffectiveCodexCatalog::from_models(models)
                .fingerprint()
                .ok(),
            Err(_) => None,
        };
        if current_fingerprint.as_deref() != transition.catalog_fingerprint.as_deref() {
            return;
        }
        let generation = self
            .inner
            .notice_generation
            .fetch_add(1, Ordering::AcqRel)
            .saturating_add(1);
        let notice = CodexRestartNoticeRecord {
            notice_id: format!("codex-restart-{generation}"),
            route_id: transition.route_id,
            selection_generation: transition.selection_generation,
            catalog_fingerprint: transition.catalog_fingerprint.unwrap_or_default(),
            created_at_ms: now_millis(),
        };
        if self
            .inner
            .database
            .upsert_codex_restart_notice(notice)
            .await
            .unwrap_or(false)
        {
            log::info!(
                target: "ai_router::catalog",
                "code=codex_restart_notice_published route={}",
                transition.route_name
            );
            self.inner
                .runtime_state
                .publish_background_change(vec![StateArea::CodexRestartNotice]);
        }
    }
}

impl RequestTransitionSink for FallbackTransitionCoordinator {
    fn request_terminal(&self, request_id: &str) {
        let coordinator = self.clone();
        let request_id = request_id.to_owned();
        tauri::async_runtime::spawn(async move {
            coordinator.mark_terminal(&request_id).await;
        });
    }
}

#[derive(Clone)]
struct DesktopFallbackActivator {
    database: DatabaseExecutor,
    routing: RoutingSnapshotStore,
    runtime_state: Arc<AppRuntimeState>,
    routing_write_gate: Arc<tokio::sync::Mutex<()>>,
    codex_projection_gate: Arc<tokio::sync::Mutex<()>>,
    app_data_dir: PathBuf,
    codex_home: PathBuf,
    transitions: FallbackTransitionCoordinator,
    route_health: Arc<RouteHealthRegistry>,
}

impl DesktopFallbackActivator {
    fn skipped_health(request: &FallbackActivationRequest) -> Vec<ActivatedSkipHealth> {
        request
            .skipped_routes
            .iter()
            .map(|skip| ActivatedSkipHealth {
                route_id: skip.route.route_id.clone(),
                kind: skip.kind,
            })
            .collect()
    }

    fn activation_proof_matches_mode(
        request: &FallbackActivationRequest,
        health_proof: &HealthActivationProof,
    ) -> bool {
        matches!(
            (&request.mode, health_proof),
            (
                FallbackActivationMode::Advance,
                HealthActivationProof::Advance { .. }
            ) | (
                FallbackActivationMode::AdvanceRecovered,
                HealthActivationProof::AdvanceRecovered { .. }
            ) | (
                FallbackActivationMode::Recover,
                HealthActivationProof::Recover { .. }
            )
        )
    }

    fn commit_health_activation(
        &self,
        request: &FallbackActivationRequest,
        reservation: &router_core::proxy::HealthActivationReservation,
        health_proof: &HealthActivationProof,
        snapshot: &RoutingSnapshot,
        skipped_health: &[ActivatedSkipHealth],
    ) {
        let participant_ids = snapshot
            .participants
            .iter()
            .map(|route| route.route_id.clone())
            .collect::<Vec<_>>();
        let committed = self.route_health.commit_activation(
            reservation,
            health_proof,
            &request.target_route.route_id,
            snapshot.health_generation,
            &participant_ids,
            skipped_health,
        );
        if !committed {
            self.route_health
                .clear_to_generation(snapshot.health_generation);
        }
    }

    async fn publish_activation(
        &self,
        request: FallbackActivationRequest,
        snapshot: Arc<RoutingSnapshot>,
        projected_fallback: FallbackStateDto,
    ) {
        self.routing.store(Arc::clone(&snapshot));
        let active_route_id = snapshot.active.as_ref().map(|route| route.route_id.clone());
        let _ = self.runtime_state.apply_committed::<_, ()>(
            Ok(()),
            vec![StateArea::Route, StateArea::Fallback],
            RuntimeProjectionUpdate {
                active_route_id: Some(active_route_id),
                fallback: Some(projected_fallback),
                ..RuntimeProjectionUpdate::default()
            },
        );
        self.transitions
            .record_activation(
                request.request_id.clone(),
                request.target_route.route_id.clone(),
                request.target_route.name.clone(),
                snapshot.selection_generation,
            )
            .await;

        let database = self.database.clone();
        let app_data_dir = self.app_data_dir.clone();
        let codex_home = self.codex_home.clone();
        let codex_projection_gate = Arc::clone(&self.codex_projection_gate);
        let runtime_state = Arc::clone(&self.runtime_state);
        let transitions = self.transitions.clone();
        let request_id = request.request_id;
        let source_route_id = request.current_route_id;
        let target_route_id = request.target_route.route_id.clone();
        let selection_generation = snapshot.selection_generation;
        tauri::async_runtime::spawn(async move {
            let fingerprint = project_fallback_catalog(
                &database,
                &app_data_dir,
                &codex_home,
                &codex_projection_gate,
                source_route_id,
                target_route_id,
                selection_generation,
            )
            .await;
            runtime_state.publish_background_change(vec![
                StateArea::CodexCatalog,
                StateArea::CodexConnection,
            ]);
            transitions
                .projection_finished(&request_id, selection_generation, fingerprint)
                .await;
        });
    }

    async fn activate_next_inner(
        &self,
        request: FallbackActivationRequest,
    ) -> Result<Option<Arc<RoutingSnapshot>>, FallbackActivationError> {
        let _routing_write = self.routing_write_gate.lock().await;
        let Some(health_proof) = request.health_proof.as_ref() else {
            return Ok(None);
        };
        let skipped_health = Self::skipped_health(&request);
        if !Self::activation_proof_matches_mode(&request, health_proof) {
            return Ok(None);
        }
        let Some(health_reservation) = self.route_health.begin_activation(
            health_proof,
            &request.current_route_id,
            &request.target_route.route_id,
            &skipped_health,
        ) else {
            return Ok(None);
        };
        let latest_projection = self.routing.load();
        let snapshot = Arc::new(RoutingSnapshot {
            active: Some(Arc::clone(&request.target_route)),
            participants: request.routing.participants.clone(),
            enabled: request.routing.enabled,
            selection_generation: request.routing.selection_generation.saturating_add(1),
            health_generation: request.routing.health_generation.saturating_add(1),
            config_revision: request.routing.config_revision,
            images_generation_enabled: latest_projection.images_generation_enabled,
            images_route: latest_projection.images_route.clone(),
            images_generation_timeout: latest_projection.images_generation_timeout,
        });
        let Ok(projected_fallback) = fallback_state(&snapshot) else {
            self.route_health.cancel_activation(&health_reservation);
            return Err(FallbackActivationError::Persistence);
        };
        let Ok(changed) = self
            .database
            .conditional_activate_forward(
                request.current_route_id.clone(),
                request.routing.selection_generation,
                request.routing.config_revision,
                request.target_route.route_id.clone(),
                request
                    .skipped_routes
                    .iter()
                    .map(|skip| skip.route.route_id.clone())
                    .collect(),
                request
                    .skipped_routes
                    .iter()
                    .filter(|skip| {
                        skip.kind == router_core::proxy::ActivatedSkipKind::ModelFallbackExcluded
                    })
                    .map(|skip| skip.route.route_id.clone())
                    .collect(),
                request.requested_model.clone(),
                request.mode == FallbackActivationMode::Recover,
            )
            .await
        else {
            self.route_health.cancel_activation(&health_reservation);
            return Err(FallbackActivationError::Persistence);
        };
        if !changed {
            self.route_health.cancel_activation(&health_reservation);
            return Ok(None);
        }
        self.commit_health_activation(
            &request,
            &health_reservation,
            health_proof,
            &snapshot,
            &skipped_health,
        );
        self.publish_activation(request, Arc::clone(&snapshot), projected_fallback)
            .await;
        Ok(Some(snapshot))
    }
}

#[async_trait]
impl FallbackActivator for DesktopFallbackActivator {
    async fn activate_next(
        &self,
        request: FallbackActivationRequest,
    ) -> Result<Option<Arc<RoutingSnapshot>>, FallbackActivationError> {
        let activator = self.clone();
        tokio::spawn(async move { activator.activate_next_inner(request).await })
            .await
            .unwrap_or(Err(FallbackActivationError::Persistence))
    }
}

#[allow(clippy::too_many_arguments)]
async fn project_fallback_catalog(
    database: &DatabaseExecutor,
    app_data_dir: &std::path::Path,
    codex_home: &std::path::Path,
    codex_projection_gate: &tokio::sync::Mutex<()>,
    source_route_id: RouteId,
    target_route_id: RouteId,
    selection_generation: u64,
) -> Option<String> {
    let _projection_write = codex_projection_gate.lock().await;
    let current = database.routing_state().await.ok()?;
    if current.active_route_id.as_ref() != Some(&target_route_id)
        || current.selection_generation != selection_generation
    {
        return None;
    }
    let source_models = database.list_codex_models(source_route_id).await.ok()?;
    let target_models = database
        .list_codex_models(target_route_id.clone())
        .await
        .ok()?;
    let source = EffectiveCodexCatalog::from_models(source_models);
    let target = EffectiveCodexCatalog::from_models(target_models.clone());
    let source_fingerprint = source.fingerprint().ok()?;
    let target_fingerprint = target.fingerprint().ok()?;
    if source_fingerprint == target_fingerprint {
        return None;
    }
    let settings = database.app_settings().await.ok()?;
    let token = load_or_create_gateway_token(database).await.ok()?;
    let catalog = LocalCodexCatalog::new(app_data_dir.to_path_buf());
    let source_path = (!source.models().is_empty()).then(|| catalog.path());
    let config = CodexConfigService::new(
        database.clone(),
        LocalCodexFilesystem::new(codex_home.to_path_buf()),
    )
    .with_images_generation_enabled(settings.images_generation_enabled);
    let (status, guard) = config
        .status_with_catalog_guard(settings.proxy_port, &token, source_path.as_deref())
        .await;
    let target_path = if target_models.is_empty() {
        None
    } else {
        Some(catalog.publish(&target_models).ok()?)
    };
    let connected = status == router_core::codex_config::CodexConfigStatus::Connected;
    if connected {
        config
            .reconnect_with_catalog_guarded(
                settings.proxy_port,
                &token,
                target_path.as_deref(),
                guard.as_ref()?,
            )
            .await
            .ok()?;
    }
    if target_models.is_empty() && connected {
        catalog.remove().ok()?;
    }
    let current = database.routing_state().await.ok()?;
    if current.active_route_id.as_ref() != Some(&target_route_id)
        || current.selection_generation != selection_generation
    {
        return None;
    }
    connected.then_some(target_fingerprint)
}

fn fallback_state(snapshot: &RoutingSnapshot) -> Result<FallbackStateDto, IpcErrorDto> {
    let active_index = snapshot.active_participant_index();
    let participant_count = u32::try_from(snapshot.participants.len())
        .map_err(|_| map_storage_error(StorageError::Initialization))?;
    let active_position = active_index
        .map(|index| u32::try_from(index + 1))
        .transpose()
        .map_err(|_| map_storage_error(StorageError::Initialization))?;
    Ok(FallbackStateDto {
        enabled: snapshot.enabled,
        participant_count,
        config_revision: snapshot.config_revision,
        active_position,
        has_next: active_index.is_some() && snapshot.participants.len() > 1,
    })
}

struct DatabaseProxyPortStore(DatabaseExecutor);

#[async_trait]
impl ProxyPortStore for DatabaseProxyPortStore {
    async fn persist_port(&self, port: u16) -> Result<(), ProxyPortError> {
        self.0
            .set_proxy_port(port)
            .await
            .map_err(|_| ProxyPortError::PersistenceFailed)
    }
}

#[async_trait]
impl AppLifecycleServices for DesktopLifecycleServices {
    async fn initialize_database(&self) -> Result<(), LifecycleFailure> {
        let path = self.app_data_dir.join("router.sqlite3");
        let manager = RecoveryManager::new(&path);
        let classification = tokio::task::spawn_blocking({
            let manager = manager.clone();
            move || manager.classify_startup()
        })
        .await
        .map_err(|_| LifecycleFailure::Database)?
        .map_err(|error| {
            LifecycleFailure::DatabaseIssue(
                classify_recovery_startup_error(&error)
                    .unwrap_or(DatabaseStartupIssue::Unavailable),
            )
        })?;
        match classification {
            DatabaseStartupClassification::NewInstall | DatabaseStartupClassification::Ready => {
                self.open_and_install_database(path, manager, false).await
            }
            DatabaseStartupClassification::RecoveryRequired(_) => {
                Err(LifecycleFailure::RecoveryRequired)
            }
            DatabaseStartupClassification::Fatal(issue) => {
                Err(LifecycleFailure::DatabaseIssue(issue))
            }
        }
    }

    async fn start_proxy(&self) -> Result<(), LifecycleFailure> {
        let database = self.database().await?;
        let settings = database
            .app_settings()
            .await
            .map_err(|_| LifecycleFailure::Database)?;
        let gateway_token = load_or_create_gateway_token(&database)
            .await
            .map_err(|_| LifecycleFailure::Database)?;
        let routes = database
            .list_routes()
            .await
            .map_err(|_| LifecycleFailure::Database)?;
        let routing = self
            .load_routing_snapshot(&database)
            .await
            .map_err(|_| LifecycleFailure::Database)?;
        let fallback = fallback_state(&routing).map_err(|_| LifecycleFailure::Database)?;
        let active_models = database
            .active_codex_models()
            .await
            .map_err(|_| LifecycleFailure::Database)?;
        let _ = self
            .reconcile_codex_models(active_models.clone(), active_models, None)
            .await
            .map_err(|_| LifecycleFailure::Database)?;
        let active_route_id = routing.active.as_ref().map(|route| route.route_id.clone());
        let live_route_ids = routes
            .iter()
            .map(|route| route.route_id.clone())
            .collect::<Vec<_>>();
        let inference = InferenceStatusService::new(self.runtime_state.clone());
        inference
            .reconstruct_from_database(&database, &live_route_ids, now_millis())
            .await
            .map_err(|_| LifecycleFailure::Database)?;
        let history = AsyncHistoryRecorder::new(
            database.clone(),
            self.diagnostics.clone(),
            self.runtime_state.clone(),
        );
        self.routing.store(Arc::clone(&routing));
        let transitions =
            FallbackTransitionCoordinator::new(database.clone(), Arc::clone(&self.runtime_state));
        let activator: Arc<dyn FallbackActivator> = Arc::new(DesktopFallbackActivator {
            database: database.clone(),
            routing: self.routing.clone(),
            runtime_state: Arc::clone(&self.runtime_state),
            routing_write_gate: Arc::clone(&self.routing_write_gate),
            codex_projection_gate: Arc::clone(&self.codex_projection_gate),
            app_data_dir: self.app_data_dir.clone(),
            codex_home: self.codex_home.clone(),
            transitions: transitions.clone(),
            route_health: Arc::clone(&self.route_health),
        });
        let transition_sink: Arc<dyn RequestTransitionSink> = Arc::new(transitions);
        let forwarder = ResponsesForwarder::new()
            .map_err(|_| LifecycleFailure::Proxy)?
            .with_runtime_services(history.clone(), self.diagnostics.clone(), inference.clone())
            .with_fallback_services(self.routing.clone(), activator)
            .with_route_health_registry(Arc::clone(&self.route_health))
            .with_request_transition_sink(transition_sink);
        let ingress = self.proxy_ingress(&gateway_token, forwarder, history.clone());
        let summaries = routes
            .iter()
            .map(|route| {
                let base_url =
                    BaseUrl::parse(&route.base_url).map_err(|_| LifecycleFailure::Database)?;
                Ok(RouteSummaryDto {
                    route_id: route.route_id.clone(),
                    name: route.name.clone(),
                    base_url_host: base_url.host(),
                    inference_status: inference.status(&route.route_id, now_millis()),
                    health: self.route_health.snapshot(&route.route_id).map(Into::into),
                })
            })
            .collect::<Result<Vec<_>, LifecycleFailure>>()?;
        let server = self
            .start_profiled_proxy(settings.proxy_port, ingress.clone())
            .await?;
        let _ = self.runtime_state.apply_committed::<_, ()>(
            Ok(()),
            STARTUP_STATE_AREAS.to_vec(),
            RuntimeProjectionUpdate {
                routes: Some(summaries),
                active_route_id: Some(active_route_id),
                fallback: Some(fallback),
                proxy_status: None,
                appearance_preference: Some(settings.appearance_preference),
                menu_bar_settings: Some(menu_bar_settings_dto(&settings)),
            },
        );
        *self.proxy.lock().await = Some(server);
        *self.history.lock().await = Some(history);
        *self.inference.lock().await = Some(inference);
        *self.ingress.lock().await = Some(ingress);
        Ok(())
    }

    async fn start_balance(&self) -> Result<(), LifecycleFailure> {
        let database = self.database().await?;
        let settings = database
            .app_settings()
            .await
            .map_err(|_| LifecycleFailure::Database)?;
        let source = Arc::new(SqliteBalanceRouteSource::new(database));
        let engine = Arc::new(BalanceExecutor::new().map_err(|_| LifecycleFailure::Balance)?);
        let coordinator = BalanceCoordinator::new(
            source,
            engine,
            self.runtime_state.clone(),
            settings.balance_query_policy,
        );
        coordinator.start_scheduler();
        let startup = coordinator.clone();
        tauri::async_runtime::spawn(async move {
            let _ = startup.trigger_startup().await;
        });
        *self.balance.lock().await = Some(coordinator);
        Ok(())
    }

    async fn stop_balance(&self) {
        if let Some(balance) = self.balance.lock().await.take() {
            balance.shutdown().await;
        }
    }

    async fn stop_proxy(&self) {
        if let Some(proxy) = self.proxy.lock().await.take() {
            self.activity.begin_new_epoch();
            proxy.shutdown().await;
        }
    }

    async fn close_database(&self) {
        if let Some(history) = self.history.lock().await.take() {
            history.shutdown().await;
        }
        self.ingress.lock().await.take();
        self.inference.lock().await.take();
        if let Some(recovery) = self.recovery.lock().await.take() {
            let _ = recovery.shutdown(std::time::Duration::from_secs(1)).await;
        }
        self.database.lock().await.take();
    }

    async fn restore_database(&self, point_id: &RecoveryPointId) -> Result<(), LifecycleFailure> {
        let path = self.app_data_dir.join("router.sqlite3");
        let manager = RecoveryManager::new(&path);
        let manager_for_restore = manager.clone();
        let point_id = point_id.clone();
        tokio::task::spawn_blocking(move || manager_for_restore.restore_point(&point_id))
            .await
            .map_err(|_| LifecycleFailure::Database)?
            .map_err(|error| map_recovery_lifecycle_failure(&error))?;
        self.open_and_install_database(path, manager, true).await
    }

    async fn start_over_database(&self) -> Result<(), LifecycleFailure> {
        let path = self.app_data_dir.join("router.sqlite3");
        let manager = RecoveryManager::new(&path);
        let manager_for_start_over = manager.clone();
        tokio::task::spawn_blocking(move || manager_for_start_over.start_over())
            .await
            .map_err(|_| LifecycleFailure::Database)?
            .map_err(|error| map_recovery_lifecycle_failure(&error))?;
        self.open_and_install_database(path, manager, false).await
    }
}

pub fn activate_existing_instance<R: Runtime>(app: &AppHandle<R>) {
    if let Some(settings) = app.get_webview_window("settings")
        && settings.is_visible().unwrap_or(false)
    {
        let _ = settings.unminimize();
        let _ = settings.set_focus();
        return;
    }
    crate::popover::request_menu_show(app);
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
                "[{}][{}] {}",
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

#[tauri::command]
pub async fn get_menu_snapshot(
    services: State<'_, Arc<DesktopLifecycleServices>>,
) -> Result<MenuSnapshotDto, IpcErrorDto> {
    services.menu_snapshot().await
}

#[tauri::command]
pub async fn get_settings_snapshot(
    services: State<'_, Arc<DesktopLifecycleServices>>,
) -> Result<SettingsSnapshotDto, IpcErrorDto> {
    services.settings_snapshot().await
}

#[tauri::command]
pub async fn get_usage_history(
    services: State<'_, Arc<DesktopLifecycleServices>>,
    query: UsageHistoryQueryDto,
) -> Result<UsageHistoryPageDto, IpcErrorDto> {
    services.usage_history(query).await
}

#[tauri::command]
pub async fn get_usage_statistics(
    services: State<'_, Arc<DesktopLifecycleServices>>,
    query: UsageStatisticsQueryDto,
) -> Result<UsageStatisticsDto, IpcErrorDto> {
    services.usage_statistics(query).await
}

#[tauri::command]
pub async fn get_usage_route_options(
    services: State<'_, Arc<DesktopLifecycleServices>>,
) -> Result<Vec<UsageRouteOptionDto>, IpcErrorDto> {
    services.usage_route_options().await
}

#[tauri::command]
pub async fn get_usage_request_detail(
    services: State<'_, Arc<DesktopLifecycleServices>>,
    request_id: String,
) -> Result<UsageRequestDetailDto, IpcErrorDto> {
    services.usage_request_detail(request_id).await
}

#[tauri::command]
pub async fn get_recovery_snapshot(
    services: State<'_, Arc<DesktopLifecycleServices>>,
    coordinator: State<'_, Arc<AppCoordinator>>,
) -> Result<RecoverySnapshotDto, IpcErrorDto> {
    services.recovery_snapshot(&coordinator.snapshot()).await
}

#[tauri::command]
pub async fn create_recovery_point(
    services: State<'_, Arc<DesktopLifecycleServices>>,
) -> Result<RecoveryHealthDto, IpcErrorDto> {
    services.create_recovery_point().await
}

#[tauri::command]
pub async fn restore_recovery_point(
    services: State<'_, Arc<DesktopLifecycleServices>>,
    coordinator: State<'_, Arc<AppCoordinator>>,
    point_id: String,
) -> Result<AppLifecycleSnapshot, IpcErrorDto> {
    require_lifecycle_phase(
        &coordinator.snapshot(),
        AppLifecyclePhase::RecoveryRequired,
        "database_recovery_not_required",
        "数据库当前不需要恢复。",
    )?;
    let point_id = RecoveryPointId::parse(&point_id)
        .map_err(|error| map_recovery_error(&error, RecoveryOperation::Restore))?;
    services.require_recovery_candidate(&point_id).await?;
    map_recovery_lifecycle_result(
        coordinator.restore_database(&point_id).await,
        RecoveryOperation::Restore,
    )
}

#[tauri::command]
pub async fn start_over_database(
    services: State<'_, Arc<DesktopLifecycleServices>>,
    coordinator: State<'_, Arc<AppCoordinator>>,
) -> Result<AppLifecycleSnapshot, IpcErrorDto> {
    require_lifecycle_phase(
        &coordinator.snapshot(),
        AppLifecyclePhase::RecoveryRequired,
        "database_recovery_not_required",
        "数据库当前不需要恢复。",
    )?;
    services.require_start_over_available().await?;
    map_recovery_lifecycle_result(
        coordinator.start_over_database().await,
        RecoveryOperation::StartOver,
    )
}

#[tauri::command]
pub async fn retry_database_startup(
    coordinator: State<'_, Arc<AppCoordinator>>,
) -> Result<AppLifecycleSnapshot, IpcErrorDto> {
    require_lifecycle_phase(
        &coordinator.snapshot(),
        AppLifecyclePhase::DatabaseError,
        "database_retry_not_available",
        "当前数据库状态不支持重试。",
    )?;
    map_recovery_lifecycle_result(coordinator.retry_database().await, RecoveryOperation::Retry)
}

#[tauri::command]
pub async fn get_route_edit(
    services: State<'_, Arc<DesktopLifecycleServices>>,
    route_id: RouteId,
) -> Result<RouteEditDto, IpcErrorDto> {
    services.route_edit(route_id).await
}

#[tauri::command]
pub async fn save_route(
    services: State<'_, Arc<DesktopLifecycleServices>>,
    input: RouteSaveInputDto,
) -> Result<RouteSaveResultDto, IpcErrorDto> {
    services.save_route(input).await
}

#[tauri::command]
pub async fn delete_route(
    services: State<'_, Arc<DesktopLifecycleServices>>,
    route_id: RouteId,
) -> Result<MutationResultDto, IpcErrorDto> {
    services
        .delete_route(route_id)
        .await
        .map(|(_, mutation)| mutation)
}

#[tauri::command]
pub async fn preview_route_activation(
    services: State<'_, Arc<DesktopLifecycleServices>>,
    route_id: RouteId,
) -> Result<RouteActivationPreviewDto, IpcErrorDto> {
    services.preview_route_activation(route_id).await
}

#[tauri::command]
pub async fn confirm_route_activation(
    services: State<'_, Arc<DesktopLifecycleServices>>,
    permit: String,
) -> Result<RouteActivationResultDto, IpcErrorDto> {
    services.confirm_route_activation(permit).await
}

#[tauri::command]
pub async fn dismiss_codex_restart_notice(
    services: State<'_, Arc<DesktopLifecycleServices>>,
    notice_id: String,
) -> Result<MutationResultDto, IpcErrorDto> {
    services.dismiss_codex_restart_notice(notice_id).await
}

#[tauri::command]
pub async fn set_fallback_enabled(
    services: State<'_, Arc<DesktopLifecycleServices>>,
    enabled: bool,
) -> Result<MutationResultDto, IpcErrorDto> {
    services.set_fallback_enabled(enabled).await
}

#[tauri::command]
pub async fn update_balance_query_settings(
    services: State<'_, Arc<DesktopLifecycleServices>>,
    input: BalanceQuerySettingsDto,
) -> Result<MutationResultDto, IpcErrorDto> {
    services.update_balance_query_settings(input).await
}

#[tauri::command]
pub async fn update_appearance_preference(
    services: State<'_, Arc<DesktopLifecycleServices>>,
    appearance_preference: AppearancePreference,
) -> Result<MutationResultDto, IpcErrorDto> {
    services
        .update_appearance_preference(appearance_preference)
        .await
}

#[tauri::command]
pub async fn update_menu_bar_settings(
    services: State<'_, Arc<DesktopLifecycleServices>>,
    input: MenuBarSettingsDto,
) -> Result<MutationResultDto, IpcErrorDto> {
    services.update_menu_bar_settings(input).await
}

#[tauri::command]
pub async fn update_images_generation_settings(
    services: State<'_, Arc<DesktopLifecycleServices>>,
    input: UpdateImagesGenerationSettingsInputDto,
) -> Result<MutationResultDto, IpcErrorDto> {
    services.update_images_generation_settings(input).await
}

#[tauri::command]
pub async fn reorder_routes_and_fallback(
    services: State<'_, Arc<DesktopLifecycleServices>>,
    input: ReorderRoutesAndFallbackInputDto,
) -> Result<MutationResultDto, IpcErrorDto> {
    services.reorder_routes_and_fallback(input).await
}

#[tauri::command]
pub async fn refresh_balance(
    services: State<'_, Arc<DesktopLifecycleServices>>,
    route_id: RouteId,
) -> Result<router_core::balance::BalanceDisplaySnapshot, IpcErrorDto> {
    services.refresh_balance(route_id).await
}

#[tauri::command]
pub async fn refresh_all_balances(
    services: State<'_, Arc<DesktopLifecycleServices>>,
) -> Result<router_core::balance::BalanceRefreshBatchState, IpcErrorDto> {
    services.refresh_all_balances().await
}

#[tauri::command]
pub async fn test_balance_query(
    services: State<'_, Arc<DesktopLifecycleServices>>,
    input: BalanceTestInputDto,
) -> Result<BalanceResult, IpcErrorDto> {
    services.test_balance(input).await
}

#[tauri::command]
pub async fn check_route_reachability(
    services: State<'_, Arc<DesktopLifecycleServices>>,
    base_url: String,
) -> Result<router_core::domain::ReachabilityResult, IpcErrorDto> {
    services.check_reachability(base_url).await
}

#[tauri::command]
pub async fn apply_proxy_port(
    services: State<'_, Arc<DesktopLifecycleServices>>,
    coordinator: State<'_, Arc<router_core::lifecycle::AppCoordinator>>,
    port: u16,
) -> Result<router_core::lifecycle::AppLifecycleSnapshot, IpcErrorDto> {
    let needs_recovery = services.apply_proxy_port(port).await?;
    if needs_recovery {
        Ok(coordinator.recover_proxy().await)
    } else {
        Ok(coordinator.snapshot())
    }
}

#[tauri::command]
pub async fn connect_codex(
    services: State<'_, Arc<DesktopLifecycleServices>>,
    allow_without_route: bool,
) -> Result<ConfigOperationResult, IpcErrorDto> {
    services.connect_codex(allow_without_route).await
}

#[tauri::command]
pub async fn reconnect_codex(
    services: State<'_, Arc<DesktopLifecycleServices>>,
) -> Result<ConfigOperationResult, IpcErrorDto> {
    services.reconnect_codex().await
}

#[tauri::command]
pub async fn preview_codex_images_mcp_repair(
    services: State<'_, Arc<DesktopLifecycleServices>>,
) -> Result<CodexImagesMcpRepairPreviewDto, IpcErrorDto> {
    services.preview_codex_images_mcp_repair().await
}

#[tauri::command]
pub async fn confirm_codex_images_mcp_repair(
    services: State<'_, Arc<DesktopLifecycleServices>>,
    permit: String,
) -> Result<ConfigOperationResult, IpcErrorDto> {
    services.confirm_codex_images_mcp_repair(permit).await
}

#[tauri::command]
pub async fn restore_codex(
    services: State<'_, Arc<DesktopLifecycleServices>>,
) -> Result<ConfigOperationResult, IpcErrorDto> {
    services.restore_codex().await
}

#[tauri::command]
pub async fn preview_update_codex_recovery(
    services: State<'_, Arc<DesktopLifecycleServices>>,
) -> Result<CodexRecoveryUpdatePreviewDto, IpcErrorDto> {
    services.preview_update_codex_recovery().await
}

#[tauri::command]
pub async fn confirm_update_codex_recovery(
    services: State<'_, Arc<DesktopLifecycleServices>>,
    permit: String,
) -> Result<ConfigOperationResult, IpcErrorDto> {
    services.confirm_update_codex_recovery(permit).await
}

#[tauri::command]
pub async fn preview_reset_codex_recovery_to_baseline(
    services: State<'_, Arc<DesktopLifecycleServices>>,
) -> Result<CodexRecoveryResetPreviewDto, IpcErrorDto> {
    services.preview_reset_codex_recovery_to_baseline().await
}

#[tauri::command]
pub async fn confirm_reset_codex_recovery_to_baseline(
    services: State<'_, Arc<DesktopLifecycleServices>>,
    permit: String,
) -> Result<ConfigOperationResult, IpcErrorDto> {
    services
        .confirm_reset_codex_recovery_to_baseline(permit)
        .await
}

#[tauri::command]
pub async fn clear_request_history(
    services: State<'_, Arc<DesktopLifecycleServices>>,
) -> Result<MutationResultDto, IpcErrorDto> {
    services.clear_history().await
}

#[tauri::command]
pub async fn mark_first_run_presented(
    services: State<'_, Arc<DesktopLifecycleServices>>,
) -> Result<(), IpcErrorDto> {
    services.mark_first_run_presented().await
}

#[tauri::command]
#[expect(clippy::needless_pass_by_value, reason = "Tauri state injection")]
pub fn open_codex_config(
    services: State<'_, Arc<DesktopLifecycleServices>>,
) -> Result<(), IpcErrorDto> {
    #[cfg(target_os = "macos")]
    {
        let config = services.codex_home.join("config.toml");
        let target = if config.exists() {
            config
        } else {
            services.codex_home.clone()
        };
        Command::new("open")
            .arg(target)
            .spawn()
            .map_err(|_| ipc_error("codex_config_open_failed", "Codex 配置无法打开。", true))?;
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = services;
        Err(ipc_error(
            "codex_config_open_unsupported",
            "当前平台不支持打开 Codex 配置。",
            false,
        ))
    }
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SettingsNavigationEvent {
    section: String,
    create_new_route: bool,
}

#[tauri::command]
#[expect(clippy::needless_pass_by_value, reason = "Tauri handle injection")]
pub fn show_settings_window(
    app: AppHandle,
    section: String,
    create_new_route: bool,
) -> Result<(), IpcErrorDto> {
    if !matches!(section.as_str(), "routes" | "usage" | "codex" | "system") {
        return Err(ipc_error(
            "settings_section_invalid",
            "设置分区无效。",
            false,
        ));
    }
    crate::popover::hide_menu_window(&app);
    let window = app
        .get_webview_window("settings")
        .ok_or_else(|| ipc_error("settings_window_unavailable", "设置窗口不可用。", true))?;
    crate::popover::position_hidden_settings_window(&app, &window);
    let _ = window.emit(
        "settings-navigate",
        SettingsNavigationEvent {
            section,
            create_new_route,
        },
    );
    window
        .show()
        .and_then(|()| window.set_focus())
        .map_err(|_| ipc_error("settings_window_unavailable", "设置窗口无法显示。", true))
}

#[tauri::command]
#[expect(clippy::needless_pass_by_value, reason = "Tauri handle injection")]
pub fn quit_application(app: AppHandle) {
    app.exit(0);
}

fn empty_metadata_failure() -> MetadataFailureDto {
    MetadataFailureDto {
        dropped_records: 0,
        write_failures: 0,
        last_error: None,
    }
}

fn menu_bar_settings_dto(settings: &AppSettingsRecord) -> MenuBarSettingsDto {
    MenuBarSettingsDto {
        status_text_enabled: settings.menu_bar.status_text_enabled,
        activity_animation_enabled: settings.menu_bar.activity_animation_enabled,
    }
}

fn map_validation_error(error: &ValidationError) -> IpcErrorDto {
    IpcErrorDto {
        code: error.code.to_owned(),
        message: match error.code {
            "base_url_invalid" => "请输入有效的 HTTP(S) 地址。",
            "base_url_too_long" => "地址过长。",
            "base_url_unsupported_endpoint" => "仅支持 Responses API 地址。",
            "base_url_duplicate_responses" => "Responses 地址不能重复包含 /responses。",
            "images_generation_timeout_out_of_range" => "生成等待上限需为 600 至 3600 秒。",
            _ => "输入内容无效。",
        }
        .to_owned(),
        retryable: false,
        field: Some(error.field.to_owned()),
    }
}

fn map_storage_error(error: StorageError) -> IpcErrorDto {
    match error {
        StorageError::Validation(error) => map_validation_error(&error),
        StorageError::CodexModelValidation(error) => map_codex_model_validation_error(&error),
        StorageError::FallbackExcludedModelValidation(error) => {
            map_fallback_excluded_model_validation_error(&error)
        }
        StorageError::InvalidUsageQuery => {
            ipc_error("usage_query_invalid", "用量筛选条件无效。", false)
        }
        StorageError::InvalidFallbackParticipantCount => ipc_field_error(
            "fallback_participant_count_invalid",
            "Fallback 参与数量无效。",
            "participantCount",
        ),
        StorageError::StaleRoutingConfiguration => ipc_error(
            "routing_configuration_stale",
            "路由配置已更新，请重试。",
            true,
        ),
        StorageError::InvalidRoutePermutation => {
            ipc_error("route_order_invalid", "路由顺序无效。", false)
        }
        StorageError::InvalidImagesGenerationRoute => ipc_field_error(
            "images_generation_route_invalid",
            "请选择已存在的图片路由。",
            "routeId",
        ),
        StorageError::NotFound => ipc_error("route_not_found", "路由不存在。", false),
        StorageError::BalanceScriptRiskConfirmationRequired => ipc_error(
            "balance_script_risk_confirmation_required",
            "启用余额脚本前需要确认风险。",
            false,
        ),
        StorageError::ExecutorClosed
        | StorageError::Initialization
        | StorageError::FutureSchema => ipc_error("database_unavailable", "数据库尚未就绪。", true),
        StorageError::UsageStatisticsOverflow
        | StorageError::Database(_)
        | StorageError::Filesystem(_) => {
            ipc_error("database_operation_failed", "数据库操作失败。", true)
        }
    }
}

fn map_codex_model_validation_error(error: &CodexModelValidationError) -> IpcErrorDto {
    let message = match error.code {
        "codex_model_id_required" => "请输入模型 ID。",
        "codex_model_id_control_character" => "模型 ID 不能包含控制字符。",
        "codex_model_id_duplicate" => "模型 ID 不能重复。",
        "codex_model_display_name_control_character" => "显示名称不能包含控制字符。",
        "codex_model_context_window_invalid" => "上下文窗口必须是正整数。",
        _ => "模型配置无效。",
    };
    IpcErrorDto {
        code: error.code.to_owned(),
        message: message.to_owned(),
        retryable: false,
        field: Some(error.field.clone()),
    }
}

fn map_fallback_excluded_model_validation_error(
    error: &FallbackExcludedModelValidationError,
) -> IpcErrorDto {
    let message = match error.code {
        "fallback_excluded_model_required" => "请输入模型 ID。",
        "fallback_excluded_model_control_character" => "模型 ID 不能包含控制字符。",
        "fallback_excluded_model_duplicate" => "模型 ID 不能重复。",
        _ => "Fallback 模型配置无效。",
    };
    IpcErrorDto {
        code: error.code.to_owned(),
        message: message.to_owned(),
        retryable: false,
        field: Some(error.field.clone()),
    }
}

fn map_codex_catalog_error(_error: CodexCatalogError) -> IpcErrorDto {
    ipc_error(
        "codex_catalog_publication_failed",
        "自定义模型目录写入失败。",
        true,
    )
}

fn partial_codex_models_result(
    models: Vec<CodexModelDto>,
    changed: bool,
    activation: CodexModelsActivation,
    error_code: &str,
    retry_token: Option<String>,
) -> ReplaceCodexModelsResult {
    ReplaceCodexModelsResult {
        models,
        changed,
        projection_applied: false,
        retry_required: retry_token.is_some(),
        activation,
        error_code: Some(error_code.to_owned()),
        retry_token,
    }
}

fn inactive_codex_models_result(models: Vec<CodexModelRecord>) -> ReplaceCodexModelsResult {
    ReplaceCodexModelsResult {
        models: models.into_iter().map(Into::into).collect(),
        changed: false,
        projection_applied: true,
        retry_required: false,
        activation: CodexModelsActivation::None,
        error_code: None,
        retry_token: None,
    }
}

fn activation_for_status(
    status: &router_core::codex_config::CodexConfigStatus,
) -> CodexModelsActivation {
    use router_core::codex_config::CodexConfigStatus;
    match status {
        CodexConfigStatus::Connected | CodexConfigStatus::Checking => {
            CodexModelsActivation::RestartCodex
        }
        CodexConfigStatus::NotConnected => CodexModelsActivation::ConnectCodex,
        CodexConfigStatus::Changed => CodexModelsActivation::ReconnectCodex,
        CodexConfigStatus::ImagesMcpNameConflict
        | CodexConfigStatus::ImagesMcpProjectionConflict
        | CodexConfigStatus::Invalid
        | CodexConfigStatus::Unreadable
        | CodexConfigStatus::SymlinkUnsupported => CodexModelsActivation::FixCodexConfig,
    }
}

fn activation_for_codex_error(error: &CodexConfigError) -> CodexModelsActivation {
    match error {
        CodexConfigError::Invalid
        | CodexConfigError::Unreadable
        | CodexConfigError::SymlinkUnsupported
        | CodexConfigError::ImagesMcpNameConflict
        | CodexConfigError::ImagesMcpRepairNotAllowed => CodexModelsActivation::FixCodexConfig,
        CodexConfigError::ChangedDuringOperation
        | CodexConfigError::BaselineMissing
        | CodexConfigError::RecoveryUnavailable
        | CodexConfigError::RecoveryNotDisconnected
        | CodexConfigError::RecoveryPreviewStale
        | CodexConfigError::RecoveryResetPartial
        | CodexConfigError::GatewayTokenInvalid
        | CodexConfigError::Filesystem(_)
        | CodexConfigError::Storage(_) => CodexModelsActivation::ReconnectCodex,
    }
}

fn map_database_startup_failure(error: &StorageError) -> LifecycleFailure {
    LifecycleFailure::DatabaseIssue(classify_storage_startup_error(error))
}

fn map_recovery_lifecycle_failure(error: &RecoveryError) -> LifecycleFailure {
    classify_recovery_startup_error(error).map_or(LifecycleFailure::RecoveryRequired, |issue| {
        LifecycleFailure::DatabaseIssue(issue)
    })
}

#[derive(Clone, Copy)]
enum RecoveryOperation {
    Inventory,
    Publish,
    Restore,
    StartOver,
    Retry,
}

impl RecoveryOperation {
    const fn failure(self) -> (&'static str, &'static str) {
        match self {
            Self::Inventory => ("recovery_inventory_unavailable", "无法读取恢复点。"),
            Self::Publish => ("recovery_publish_failed", "无法创建恢复点。"),
            Self::Restore => ("recovery_restore_failed", "数据库恢复失败。"),
            Self::StartOver => ("database_start_over_failed", "无法创建空数据库。"),
            Self::Retry => ("database_retry_failed", "数据库启动重试失败。"),
        }
    }
}

fn map_recovery_error(error: &RecoveryError, operation: RecoveryOperation) -> IpcErrorDto {
    if let Some(issue) = classify_recovery_startup_error(error) {
        return map_database_startup_issue(issue);
    }
    match error {
        RecoveryError::InvalidPointId => ipc_error(
            "recovery_point_stale",
            "所选恢复点已失效，请刷新后重试。",
            false,
        ),
        RecoveryError::InvalidPoint => match operation {
            RecoveryOperation::StartOver => ipc_error(
                "database_start_over_not_allowed",
                "仍有可用恢复点，不能创建空数据库。",
                false,
            ),
            _ => ipc_error(
                "recovery_point_stale",
                "所选恢复点已失效，请刷新后重试。",
                false,
            ),
        },
        RecoveryError::UnsafeFilesystemObject | RecoveryError::FutureSchema => {
            unreachable!("classified recovery startup error")
        }
        RecoveryError::UnknownTable | RecoveryError::DomainValidation => {
            ipc_error("recovery_point_invalid", "恢复点未通过安全校验。", false)
        }
        RecoveryError::Filesystem(_) | RecoveryError::Database(_) => {
            let (code, message) = operation.failure();
            ipc_error(code, message, true)
        }
        RecoveryError::Storage(_) => {
            unreachable!("classified recovery storage error")
        }
    }
}

fn map_database_startup_issue(issue: DatabaseStartupIssue) -> IpcErrorDto {
    match issue {
        DatabaseStartupIssue::Permission => ipc_error(
            "database_permission_denied",
            "数据库或恢复目录无法访问。",
            true,
        ),
        DatabaseStartupIssue::DiskFull => ipc_error(
            "database_space_unavailable",
            "磁盘空间不足，无法完成数据库操作。",
            true,
        ),
        DatabaseStartupIssue::FutureSchema => ipc_error(
            "database_version_too_new",
            "数据库由更高版本的 AI Router 创建。",
            false,
        ),
        DatabaseStartupIssue::UnsafePath => ipc_error(
            "database_path_unsafe",
            "数据库或恢复目录不是安全的常规路径。",
            false,
        ),
        DatabaseStartupIssue::Unavailable => {
            ipc_error("database_unavailable", "数据库暂时不可用。", true)
        }
    }
}

fn require_lifecycle_phase(
    snapshot: &AppLifecycleSnapshot,
    expected: AppLifecyclePhase,
    code: &str,
    message: &str,
) -> Result<(), IpcErrorDto> {
    if snapshot.phase == expected {
        Ok(())
    } else {
        Err(ipc_error(code, message, false))
    }
}

fn map_recovery_lifecycle_result(
    snapshot: AppLifecycleSnapshot,
    operation: RecoveryOperation,
) -> Result<AppLifecycleSnapshot, IpcErrorDto> {
    match snapshot.phase {
        AppLifecyclePhase::Running => Ok(snapshot),
        AppLifecyclePhase::DatabaseError => {
            if let Some(AppLifecycleIssue::Database(issue)) = snapshot.issue {
                Err(map_database_startup_issue(issue))
            } else {
                let (code, message) = operation.failure();
                Err(ipc_error(code, message, true))
            }
        }
        AppLifecyclePhase::RecoveryRequired => {
            let (code, message) = operation.failure();
            Err(ipc_error(code, message, true))
        }
        _ => Err(ipc_error(
            "database_recovery_unavailable",
            "当前数据库状态不支持此操作。",
            false,
        )),
    }
}

fn map_balance_error(_error: router_core::balance::BalanceError) -> IpcErrorDto {
    ipc_error("balance_query_failed", "余额查询失败。", true)
}

fn map_proxy_port_error(error: &ProxyPortError) -> IpcErrorDto {
    match error {
        ProxyPortError::InvalidPort => {
            ipc_field_error("proxy_port_invalid", "端口必须在 1 到 65535 之间。", "port")
        }
        ProxyPortError::PortUnavailable => {
            ipc_error("proxy_port_unavailable", "该端口已被占用。", true)
        }
        ProxyPortError::PersistenceFailed => {
            ipc_error("proxy_port_save_failed", "端口保存失败。", true)
        }
    }
}

fn map_codex_error(error: &CodexConfigError) -> IpcErrorDto {
    match error {
        CodexConfigError::Invalid => ipc_error("codex_config_invalid", "Codex 配置无效。", false),
        CodexConfigError::Unreadable => {
            ipc_error("codex_config_unreadable", "Codex 配置无法读取。", true)
        }
        CodexConfigError::SymlinkUnsupported => ipc_error(
            "codex_config_symlink_unsupported",
            "不支持符号链接形式的 Codex 配置。",
            false,
        ),
        CodexConfigError::ChangedDuringOperation => ipc_error(
            "codex_config_changed",
            "Codex 配置在操作期间发生变化，请重试。",
            true,
        ),
        CodexConfigError::BaselineMissing => {
            ipc_error("codex_baseline_missing", "尚未创建初始配置。", false)
        }
        CodexConfigError::RecoveryUnavailable => {
            ipc_error("codex_recovery_unavailable", "断开恢复配置暂不可用。", true)
        }
        CodexConfigError::RecoveryNotDisconnected => ipc_error(
            "codex_recovery_not_disconnected",
            "请先断开 Codex 后再执行此操作。",
            false,
        ),
        CodexConfigError::RecoveryPreviewStale => ipc_error(
            "codex_recovery_preview_stale",
            "恢复配置预览已失效，请重新确认。",
            true,
        ),
        CodexConfigError::RecoveryResetPartial => ipc_error(
            "codex_recovery_reset_partial",
            "首次连接前状态仅部分恢复，请刷新后重试。",
            true,
        ),
        CodexConfigError::ImagesMcpNameConflict => ipc_error(
            "codex_images_mcp_name_conflict",
            "Codex 配置中的 ai_router_images 名称已被占用。",
            false,
        ),
        CodexConfigError::ImagesMcpRepairNotAllowed => ipc_error(
            "codex_images_mcp_repair_not_available",
            "当前图片工具配置不支持修复。",
            false,
        ),
        CodexConfigError::GatewayTokenInvalid => {
            ipc_error("gateway_token_unavailable", "本地网关令牌不可用。", false)
        }
        CodexConfigError::Filesystem(_) | CodexConfigError::Storage(_) => ipc_error(
            "codex_config_operation_failed",
            "Codex 配置操作失败。",
            true,
        ),
    }
}

fn ipc_field_error(code: &str, message: &str, field: &str) -> IpcErrorDto {
    IpcErrorDto {
        code: code.to_owned(),
        message: message.to_owned(),
        retryable: false,
        field: Some(field.to_owned()),
    }
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
    use std::{
        fs,
        net::{Ipv4Addr, TcpListener},
        sync::Mutex,
        time::Duration,
    };

    use router_core::state::{StateChangedEventDto, StateEventError, StateEventSink};
    use tempfile::TempDir;

    use super::*;

    struct NoopEventSink;

    impl StateEventSink for NoopEventSink {
        fn publish(&self, _event: &StateChangedEventDto) -> Result<(), StateEventError> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct RecordingEventSink(Mutex<Vec<StateChangedEventDto>>);

    impl StateEventSink for RecordingEventSink {
        fn publish(&self, event: &StateChangedEventDto) -> Result<(), StateEventError> {
            self.0.lock().expect("event sink lock").push(event.clone());
            Ok(())
        }
    }

    struct NoopDiagnosticSink;

    impl RuntimeDiagnosticSink for NoopDiagnosticSink {
        fn emit(&self, _event: RuntimeDiagnosticEvent) {}
    }

    #[test]
    fn usage_statistics_errors_map_to_stable_safe_ipc_categories() {
        let invalid = map_storage_error(StorageError::InvalidUsageQuery);
        assert_eq!(invalid.code, "usage_query_invalid");
        assert!(!invalid.retryable);
        assert_eq!(invalid.field, None);

        let overflow = map_storage_error(StorageError::UsageStatisticsOverflow);
        assert_eq!(overflow.code, "database_operation_failed");
        assert_eq!(overflow.message, "数据库操作失败。");
        assert!(overflow.retryable);
        assert_eq!(overflow.field, None);
    }

    #[test]
    fn route_reorder_errors_map_to_stable_safe_ipc_categories() {
        let stale = map_storage_error(StorageError::StaleRoutingConfiguration);
        assert_eq!(stale.code, "routing_configuration_stale");
        assert_eq!(stale.message, "路由配置已更新，请重试。");
        assert!(stale.retryable);
        assert_eq!(stale.field, None);

        let invalid = map_storage_error(StorageError::InvalidRoutePermutation);
        assert_eq!(invalid.code, "route_order_invalid");
        assert_eq!(invalid.message, "路由顺序无效。");
        assert!(!invalid.retryable);
        assert_eq!(invalid.field, None);
    }

    fn codex_model(model_id: &str) -> CodexModelDto {
        CodexModelDto {
            model_id: model_id.to_owned(),
            display_name: Some(format!("{model_id} display")),
            context_window: Some(128_000),
        }
    }

    fn replace_codex_models_input(
        model_ids: &[&str],
        retry_token: Option<String>,
    ) -> ReplaceCodexModelsInput {
        ReplaceCodexModelsInput {
            models: model_ids
                .iter()
                .map(|model_id| codex_model(model_id))
                .collect(),
            retry_token,
        }
    }

    async fn create_active_test_route(database: &DatabaseExecutor) -> RouteId {
        database
            .create_route(CreateRouteInput {
                name: "Models".to_owned(),
                base_url: "https://models.example/v1".to_owned(),
                api_key: ApiKey::parse("models-key").expect("API key"),
                service_tier_policy: router_core::domain::ServiceTierPolicy::Passthrough,
                balance_query: None,
                accept_script_risk: false,
            })
            .await
            .expect("route")
            .route_id
    }

    fn route_save_health_input(route_id: &RouteId, name: &str) -> RouteSaveInputDto {
        RouteSaveInputDto {
            route_id: Some(route_id.clone()),
            name: name.to_owned(),
            base_url: "https://A.example/v1".to_owned(),
            api_key: "A-key".to_owned(),
            service_tier_policy: router_core::domain::ServiceTierPolicy::Passthrough,
            balance_query: None,
            accept_script_risk: false,
            fallback_excluded_models: Vec::new(),
            models: Vec::new(),
            retry_token: None,
        }
    }

    #[tokio::test]
    async fn route_save_preserves_exact_health_and_invalidates_only_changed_route() {
        let directory = TempDir::new().expect("app data fixture");
        let services = DesktopLifecycleServices::new(
            directory.path().to_path_buf(),
            directory.path(),
            DesktopRuntimeProfile::Isolated,
            Arc::new(AppRuntimeState::new(Arc::new(NoopEventSink))),
            Arc::new(NoopDiagnosticSink),
        );
        services
            .initialize_database()
            .await
            .expect("initialize database");
        let database = services.database().await.expect("database");
        let routes = [
            create_fallback_test_route(&database, "A").await,
            create_fallback_test_route(&database, "B").await,
        ];
        database
            .set_fallback_enabled(true)
            .await
            .expect("enable fallback");
        services
            .refresh_route_projection(&database)
            .await
            .expect("routing projection");
        let generation = services.routing.load().health_generation;
        assert_eq!(
            services.route_health.record_ordinary_failure(
                &routes[0],
                generation,
                router_core::proxy::HealthFailureClass::Service,
                true,
                true,
            ),
            router_core::proxy::StrikeResult::BelowThreshold { failure_count: 1 }
        );
        assert_eq!(
            services.route_health.record_ordinary_failure(
                &routes[1],
                generation,
                router_core::proxy::HealthFailureClass::Service,
                true,
                true,
            ),
            router_core::proxy::StrikeResult::BelowThreshold { failure_count: 1 }
        );

        services
            .save_route(route_save_health_input(&routes[0], "A"))
            .await
            .expect("exact route save");

        assert_eq!(
            services.route_health.snapshot(&routes[0]),
            Some(router_core::proxy::RouteHealthSnapshot::Striking { failure_count: 1 })
        );
        assert_eq!(services.routing.load().health_generation, generation);

        services
            .save_route(route_save_health_input(&routes[0], "A changed"))
            .await
            .expect("changed route save");

        assert_eq!(services.route_health.snapshot(&routes[0]), None);
        assert_eq!(
            services.route_health.snapshot(&routes[1]),
            Some(router_core::proxy::RouteHealthSnapshot::Striking { failure_count: 1 })
        );
        assert_eq!(
            services.route_health.record_ordinary_failure(
                &routes[0],
                generation,
                router_core::proxy::HealthFailureClass::Timeout,
                true,
                true,
            ),
            router_core::proxy::StrikeResult::Stale
        );
        assert_eq!(
            services.routing.load().health_generation,
            generation.saturating_add(1)
        );
        services.close_database().await;
    }

    struct ImageMcpRepairFixture {
        _directory: TempDir,
        services: Arc<DesktopLifecycleServices>,
        database: DatabaseExecutor,
        events: Arc<RecordingEventSink>,
        route_id: RouteId,
        config_path: PathBuf,
        drifted_bytes: Vec<u8>,
        baseline_bytes: Vec<u8>,
    }

    async fn image_mcp_repair_fixture() -> ImageMcpRepairFixture {
        let directory = TempDir::new().expect("app data fixture");
        let events = Arc::new(RecordingEventSink::default());
        let runtime = Arc::new(AppRuntimeState::new(events.clone()));
        let services = DesktopLifecycleServices::new(
            directory.path().to_path_buf(),
            directory.path(),
            DesktopRuntimeProfile::Isolated,
            runtime,
            Arc::new(NoopDiagnosticSink),
        );
        services
            .initialize_database()
            .await
            .expect("initialize database");
        let database = services.database().await.expect("database");
        let route_id = database
            .create_route(CreateRouteInput {
                name: "Image repair".to_owned(),
                base_url: "https://image-repair.example/v1".to_owned(),
                api_key: ApiKey::parse("image-repair-key").expect("API key"),
                service_tier_policy: router_core::domain::ServiceTierPolicy::Passthrough,
                balance_query: None,
                accept_script_risk: false,
            })
            .await
            .expect("image route")
            .route_id;
        services
            .update_images_generation_settings(UpdateImagesGenerationSettingsInputDto {
                enabled: true,
                route_id: Some(route_id.clone()),
                timeout_secs: 600,
            })
            .await
            .expect("enable images");
        services.connect_codex(false).await.expect("connect Codex");
        let baseline = database
            .codex_baseline()
            .await
            .expect("baseline query")
            .expect("baseline");
        assert!(!baseline.original_exists);
        let config_path = services.codex_home.join("config.toml");
        let connected = fs::read_to_string(&config_path).expect("connected config");
        let drifted_bytes = format!(
            "permissions = \"keep\"\n{}",
            connected
                .lines()
                .filter(|line| !line.trim_start().starts_with("http_headers ="))
                .collect::<Vec<_>>()
                .join("\n")
        )
        .into_bytes();
        fs::write(&config_path, &drifted_bytes).expect("write drifted config");
        assert_eq!(
            services.codex_status().await.expect("conflict status"),
            router_core::codex_config::CodexConfigStatus::ImagesMcpProjectionConflict
        );
        ImageMcpRepairFixture {
            _directory: directory,
            services,
            database,
            events,
            route_id,
            config_path,
            drifted_bytes,
            baseline_bytes: baseline.raw_bytes,
        }
    }

    async fn create_fallback_test_route(database: &DatabaseExecutor, name: &str) -> RouteId {
        database
            .create_route(CreateRouteInput {
                name: name.to_owned(),
                base_url: format!("https://{name}.example/v1"),
                api_key: ApiKey::parse(&format!("{name}-key")).expect("API key"),
                service_tier_policy: router_core::domain::ServiceTierPolicy::Passthrough,
                balance_query: None,
                accept_script_risk: false,
            })
            .await
            .expect("route")
            .route_id
    }

    fn fallback_test_activator(
        services: &DesktopLifecycleServices,
        database: &DatabaseExecutor,
        runtime: &Arc<AppRuntimeState>,
    ) -> DesktopFallbackActivator {
        DesktopFallbackActivator {
            database: database.clone(),
            routing: services.routing.clone(),
            runtime_state: Arc::clone(runtime),
            routing_write_gate: Arc::clone(&services.routing_write_gate),
            codex_projection_gate: Arc::clone(&services.codex_projection_gate),
            app_data_dir: services.app_data_dir.clone(),
            codex_home: services.codex_home.clone(),
            transitions: FallbackTransitionCoordinator::new(database.clone(), Arc::clone(runtime)),
            route_health: Arc::clone(&services.route_health),
        }
    }

    fn fallback_test_advance_proof(
        services: &DesktopLifecycleServices,
        route_id: &RouteId,
        health_generation: u64,
    ) -> router_core::proxy::HealthActivationProof {
        let mut trip = None;
        for _ in 0..5 {
            if let router_core::proxy::StrikeResult::TripAcquired(lease) =
                services.route_health.record_ordinary_failure(
                    route_id,
                    health_generation,
                    router_core::proxy::HealthFailureClass::Service,
                    true,
                    true,
                )
            {
                trip = Some(lease);
            }
        }
        let trip = trip.expect("route reaches the test threshold");
        assert!(services.route_health.reserve_trip(&trip));
        router_core::proxy::HealthActivationProof::Advance { source: trip }
    }

    struct FallbackBoundaryFixture {
        _directory: TempDir,
        runtime: Arc<AppRuntimeState>,
        services: Arc<DesktopLifecycleServices>,
        database: DatabaseExecutor,
        route_ids: Vec<RouteId>,
    }

    async fn fallback_boundary_fixture(initial_count: u32) -> FallbackBoundaryFixture {
        let directory = TempDir::new().expect("app data fixture");
        let runtime = Arc::new(AppRuntimeState::new(Arc::new(NoopEventSink)));
        let services = DesktopLifecycleServices::new(
            directory.path().to_path_buf(),
            directory.path(),
            DesktopRuntimeProfile::Isolated,
            Arc::clone(&runtime),
            Arc::new(NoopDiagnosticSink),
        );
        services
            .initialize_database()
            .await
            .expect("initialize database");
        let database = services.database().await.expect("database");
        let mut route_ids = Vec::new();
        for name in ["A", "B", "C"] {
            route_ids.push(create_fallback_test_route(&database, name).await);
        }
        database
            .set_fallback_participant_count(initial_count)
            .await
            .expect("initial boundary");
        database
            .set_fallback_enabled(true)
            .await
            .expect("enable fallback");
        services
            .refresh_route_projection(&database)
            .await
            .expect("routing projection");
        FallbackBoundaryFixture {
            _directory: directory,
            runtime,
            services,
            database,
            route_ids,
        }
    }

    async fn assert_boundary_automatic_activation_order(
        initial_count: u32,
        next_count: u32,
        boundary_first: bool,
    ) {
        let fixture = fallback_boundary_fixture(initial_count).await;
        let captured = fixture.services.routing.load();
        let health_proof = fallback_test_advance_proof(
            &fixture.services,
            &fixture.route_ids[0],
            captured.health_generation,
        );
        let first = captured.active.as_ref().expect("active route");
        let second = captured
            .next_after(&first.route_id)
            .expect("next fallback route");
        let activator =
            fallback_test_activator(&fixture.services, &fixture.database, &fixture.runtime);
        let automatic_request = FallbackActivationRequest {
            request_id: format!("boundary-race-{initial_count}-{next_count}-{boundary_first}"),
            routing: captured,
            current_route_id: fixture.route_ids[0].clone(),
            target_route: second,
            requested_model: "test-model".to_owned(),
            skipped_routes: Vec::new(),
            mode: FallbackActivationMode::Advance,
            health_proof: Some(health_proof),
        };

        let automatic_result = if boundary_first {
            let boundary = fixture.services.set_fallback_participant_count(next_count);
            let automatic = activator.activate_next_inner(automatic_request);
            let (boundary_result, automatic_result) = tokio::join!(biased; boundary, automatic);
            boundary_result.expect("boundary mutation");
            automatic_result
        } else {
            let automatic = activator.activate_next_inner(automatic_request);
            let boundary = fixture.services.set_fallback_participant_count(next_count);
            let (automatic_result, boundary_result) = tokio::join!(biased; automatic, boundary);
            boundary_result.expect("boundary mutation");
            automatic_result
        }
        .expect("automatic activation result");

        assert_eq!(automatic_result.is_some(), !boundary_first);
        let expected_active = if boundary_first {
            &fixture.route_ids[0]
        } else {
            &fixture.route_ids[1]
        };
        let durable = fixture
            .database
            .routing_state()
            .await
            .expect("durable routing");
        assert_eq!(durable.active_route_id.as_ref(), Some(expected_active));
        assert_eq!(durable.fallback.participant_count, next_count);
        let published = fixture.services.routing.load();
        assert_eq!(published.config_revision, durable.fallback.config_revision);
        assert_eq!(published.selection_generation, durable.selection_generation);
        assert_eq!(
            published.participants.len(),
            usize::try_from(next_count).expect("participant count")
        );
        assert_eq!(
            published.active.as_ref().map(|route| &route.route_id),
            Some(expected_active)
        );
        let bootstrap = fixture.runtime.bootstrap_snapshot();
        assert_eq!(bootstrap.active_route_id.as_ref(), Some(expected_active));
        assert_eq!(bootstrap.fallback.participant_count, next_count);
        assert_eq!(
            bootstrap.fallback.active_position,
            Some(if boundary_first { 1 } else { 2 })
        );
        assert!(bootstrap.fallback.has_next);
        fixture.services.close_database().await;
    }

    #[test]
    fn recovery_health_changes_publish_only_bounded_state_metadata() {
        let events = Arc::new(RecordingEventSink::default());
        let runtime = Arc::new(AppRuntimeState::new(events.clone()));
        let sink = DesktopRecoveryEventSink {
            runtime_state: runtime,
        };

        sink.health_changed(&RecoveryHealth {
            kind: router_core::recovery::RecoveryHealthKind::Degraded,
            latest_success_at_ms: Some(1_000),
            valid_point_count: 2,
            live_critical_revision: 8,
            covered_critical_revision: Some(7),
            last_failure: Some(RecoveryFailureCode::PublicationFailed),
        });

        assert_eq!(
            events.0.lock().expect("event sink lock")[0].areas,
            vec![StateArea::Recovery]
        );
    }

    #[test]
    fn recovery_errors_map_to_stable_bounded_ipc_contracts() {
        let cases = [
            (
                RecoveryError::InvalidPointId,
                RecoveryOperation::Restore,
                "recovery_point_stale",
                false,
            ),
            (
                RecoveryError::UnsafeFilesystemObject,
                RecoveryOperation::Restore,
                "database_path_unsafe",
                false,
            ),
            (
                RecoveryError::FutureSchema,
                RecoveryOperation::Restore,
                "database_version_too_new",
                false,
            ),
            (
                RecoveryError::UnknownTable,
                RecoveryOperation::Restore,
                "recovery_point_invalid",
                false,
            ),
            (
                RecoveryError::DomainValidation,
                RecoveryOperation::Restore,
                "recovery_point_invalid",
                false,
            ),
            (
                RecoveryError::Filesystem(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "/private/secret/router.sqlite3",
                )),
                RecoveryOperation::Publish,
                "database_permission_denied",
                true,
            ),
            (
                RecoveryError::Filesystem(std::io::Error::other("/private/secret/router.sqlite3")),
                RecoveryOperation::Publish,
                "recovery_publish_failed",
                true,
            ),
            (
                RecoveryError::Storage(StorageError::Initialization),
                RecoveryOperation::Restore,
                "database_unavailable",
                true,
            ),
        ];

        for (error, operation, expected_code, expected_retryable) in cases {
            let mapped = map_recovery_error(&error, operation);
            assert_eq!(mapped.code, expected_code);
            assert_eq!(mapped.retryable, expected_retryable);
            assert!(mapped.field.is_none());
            assert!(!mapped.message.contains("/private"));
            assert!(!mapped.message.contains("sqlite"));
        }

        for (issue, expected_code, expected_retryable) in [
            (
                DatabaseStartupIssue::Permission,
                "database_permission_denied",
                true,
            ),
            (
                DatabaseStartupIssue::DiskFull,
                "database_space_unavailable",
                true,
            ),
            (
                DatabaseStartupIssue::FutureSchema,
                "database_version_too_new",
                false,
            ),
            (
                DatabaseStartupIssue::UnsafePath,
                "database_path_unsafe",
                false,
            ),
            (
                DatabaseStartupIssue::Unavailable,
                "database_unavailable",
                true,
            ),
        ] {
            let mapped = map_database_startup_issue(issue);
            assert_eq!(mapped.code, expected_code);
            assert_eq!(mapped.retryable, expected_retryable);
        }
    }

    #[test]
    fn recovery_lifecycle_results_report_success_only_after_running() {
        let running = AppLifecycleSnapshot {
            phase: AppLifecyclePhase::Running,
            issue: Some(AppLifecycleIssue::BalanceStartupFailed),
        };
        assert_eq!(
            map_recovery_lifecycle_result(running.clone(), RecoveryOperation::Restore),
            Ok(running)
        );

        let fatal = map_recovery_lifecycle_result(
            AppLifecycleSnapshot {
                phase: AppLifecyclePhase::DatabaseError,
                issue: Some(AppLifecycleIssue::Database(
                    DatabaseStartupIssue::FutureSchema,
                )),
            },
            RecoveryOperation::Retry,
        )
        .expect_err("future schema must not report recovery success");
        assert_eq!(fatal.code, "database_version_too_new");
        assert!(!fatal.retryable);

        let recoverable = map_recovery_lifecycle_result(
            AppLifecycleSnapshot {
                phase: AppLifecyclePhase::RecoveryRequired,
                issue: None,
            },
            RecoveryOperation::Restore,
        )
        .expect_err("failed restore must not report success");
        assert_eq!(recoverable.code, "recovery_restore_failed");
        assert!(recoverable.retryable);
    }

    #[test]
    fn codex_model_activation_covers_every_config_status() {
        use router_core::codex_config::CodexConfigStatus;

        for (status, expected) in [
            (
                CodexConfigStatus::Checking,
                CodexModelsActivation::RestartCodex,
            ),
            (
                CodexConfigStatus::Connected,
                CodexModelsActivation::RestartCodex,
            ),
            (
                CodexConfigStatus::NotConnected,
                CodexModelsActivation::ConnectCodex,
            ),
            (
                CodexConfigStatus::Changed,
                CodexModelsActivation::ReconnectCodex,
            ),
            (
                CodexConfigStatus::ImagesMcpNameConflict,
                CodexModelsActivation::FixCodexConfig,
            ),
            (
                CodexConfigStatus::ImagesMcpProjectionConflict,
                CodexModelsActivation::FixCodexConfig,
            ),
            (
                CodexConfigStatus::Invalid,
                CodexModelsActivation::FixCodexConfig,
            ),
            (
                CodexConfigStatus::Unreadable,
                CodexModelsActivation::FixCodexConfig,
            ),
            (
                CodexConfigStatus::SymlinkUnsupported,
                CodexModelsActivation::FixCodexConfig,
            ),
        ] {
            assert_eq!(activation_for_status(&status), expected);
        }
    }

    #[tokio::test]
    async fn codex_model_retry_permits_are_one_use_and_snapshot_bound() {
        let directory = TempDir::new().expect("app data fixture");
        let services = DesktopLifecycleServices::new(
            directory.path().to_path_buf(),
            directory.path(),
            DesktopRuntimeProfile::Isolated,
            Arc::new(AppRuntimeState::new(Arc::new(NoopEventSink))),
            Arc::new(NoopDiagnosticSink),
        );
        let models = vec![CodexModelRecord {
            model_id: "relay-a".to_owned(),
            display_name: None,
            context_window: None,
        }];
        let token = services
            .issue_codex_model_retry(models.clone(), CodexModelRetryKind::ConfigProjection, None)
            .await;

        assert_eq!(
            services
                .consume_codex_model_retry(Some(&token), &[])
                .await
                .map(|permit| permit.kind),
            None
        );
        assert_eq!(
            services
                .consume_codex_model_retry(Some(&token), &models)
                .await
                .map(|permit| permit.kind),
            Some(CodexModelRetryKind::ConfigProjection)
        );
        assert_eq!(
            services
                .consume_codex_model_retry(Some(&token), &models)
                .await
                .map(|permit| permit.kind),
            None
        );
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn manual_activation_preview_requires_confirmation_and_rejects_stale_catalogs() {
        let directory = TempDir::new().expect("app data fixture");
        let services = DesktopLifecycleServices::new(
            directory.path().to_path_buf(),
            directory.path(),
            DesktopRuntimeProfile::Isolated,
            Arc::new(AppRuntimeState::new(Arc::new(NoopEventSink))),
            Arc::new(NoopDiagnosticSink),
        );
        services
            .initialize_database()
            .await
            .expect("initialize database");
        let database = services.database().await.expect("database");
        let first = database
            .create_route_with_models(
                CreateRouteInput {
                    name: "First".to_owned(),
                    base_url: "https://first.example/v1".to_owned(),
                    api_key: ApiKey::parse("first-key").expect("key"),
                    service_tier_policy: router_core::domain::ServiceTierPolicy::Passthrough,
                    balance_query: None,
                    accept_script_risk: false,
                },
                vec![CodexModelRecord {
                    model_id: "first-model".to_owned(),
                    display_name: None,
                    context_window: None,
                }],
            )
            .await
            .expect("first route");
        let second = database
            .create_route_with_models(
                CreateRouteInput {
                    name: "Second".to_owned(),
                    base_url: "https://second.example/v1".to_owned(),
                    api_key: ApiKey::parse("second-key").expect("key"),
                    service_tier_policy: router_core::domain::ServiceTierPolicy::Passthrough,
                    balance_query: None,
                    accept_script_risk: false,
                },
                vec![CodexModelRecord {
                    model_id: "second-model".to_owned(),
                    display_name: None,
                    context_window: None,
                }],
            )
            .await
            .expect("second route");
        services.connect_codex(true).await.expect("connect Codex");

        let preview = services
            .preview_route_activation(second.route_id.clone())
            .await
            .expect("preview");
        assert!(preview.confirmation_required);
        assert_eq!(preview.target_route_name, "Second");
        assert_eq!(preview.target_catalog_mode, RouteCatalogMode::Custom);

        database
            .replace_codex_models(
                second.route_id.clone(),
                vec![CodexModelRecord {
                    model_id: "second-model-edited".to_owned(),
                    display_name: None,
                    context_window: None,
                }],
            )
            .await
            .expect("edit target models");
        let stale = services
            .confirm_route_activation(preview.permit)
            .await
            .expect_err("stale permit");
        assert_eq!(stale.code, "route_activation_permit_stale");
        assert_eq!(
            database.active_route_id().await.expect("active route"),
            Some(first.route_id.clone())
        );

        let refreshed = services
            .preview_route_activation(second.route_id.clone())
            .await
            .expect("refreshed preview");
        let config_path = services.codex_home.join("config.toml");
        let mut externally_changed = fs::read_to_string(&config_path).expect("connected config");
        externally_changed.push_str("\n# external edit after preview\n");
        fs::write(&config_path, externally_changed).expect("external config edit");
        let stale_config = services
            .confirm_route_activation(refreshed.permit)
            .await
            .expect_err("config-bound permit must be stale");
        assert_eq!(stale_config.code, "route_activation_permit_stale");
        assert_eq!(
            database.active_route_id().await.expect("active route"),
            Some(first.route_id.clone())
        );

        let refreshed = services
            .preview_route_activation(second.route_id.clone())
            .await
            .expect("config-refreshed preview");
        let activated = services
            .confirm_route_activation(refreshed.permit)
            .await
            .expect("activate");
        assert_eq!(
            activated.catalog.activation,
            CodexModelsActivation::RestartCodex
        );
        assert_eq!(
            database.active_route_id().await.expect("active route"),
            Some(second.route_id)
        );
        services.close_database().await;
    }

    #[tokio::test]
    async fn fallback_notice_waits_for_request_terminal_and_projection() {
        let directory = TempDir::new().expect("app data fixture");
        let services = DesktopLifecycleServices::new(
            directory.path().to_path_buf(),
            directory.path(),
            DesktopRuntimeProfile::Isolated,
            Arc::new(AppRuntimeState::new(Arc::new(NoopEventSink))),
            Arc::new(NoopDiagnosticSink),
        );
        services
            .initialize_database()
            .await
            .expect("initialize database");
        let database = services.database().await.expect("database");
        let route_id = create_active_test_route(&database).await;
        let routing = database.routing_state().await.expect("routing");
        let active_fingerprint = EffectiveCodexCatalog::Original
            .fingerprint()
            .expect("original fingerprint");
        let coordinator = FallbackTransitionCoordinator::new(
            database.clone(),
            Arc::clone(&services.runtime_state),
        );

        coordinator
            .record_activation(
                "request-1".to_owned(),
                route_id,
                "Models".to_owned(),
                routing.selection_generation,
            )
            .await;
        coordinator.mark_terminal("request-1").await;
        assert_eq!(
            database
                .codex_restart_notice()
                .await
                .expect("no early notice"),
            None
        );
        coordinator
            .projection_finished(
                "request-1",
                routing.selection_generation,
                Some(active_fingerprint.clone()),
            )
            .await;
        let notice = database
            .codex_restart_notice()
            .await
            .expect("notice load")
            .expect("notice after both events");
        assert_eq!(notice.catalog_fingerprint, active_fingerprint);
        assert_eq!(notice.selection_generation, routing.selection_generation);

        coordinator
            .record_activation(
                "request-race".to_owned(),
                notice.route_id.clone(),
                "Models".to_owned(),
                routing.selection_generation,
            )
            .await;
        coordinator
            .record_activation(
                "request-race".to_owned(),
                notice.route_id,
                "Models".to_owned(),
                routing.selection_generation + 1,
            )
            .await;
        coordinator
            .projection_finished(
                "request-race",
                routing.selection_generation,
                Some("stale-catalog".to_owned()),
            )
            .await;
        assert_eq!(
            coordinator
                .inner
                .pending
                .lock()
                .await
                .get("request-race")
                .and_then(|transition| transition.catalog_fingerprint.as_deref()),
            None
        );
        services.close_database().await;
    }

    #[tokio::test]
    async fn stale_fallback_projection_cannot_replace_the_owned_catalog() {
        let directory = TempDir::new().expect("app data fixture");
        let services = DesktopLifecycleServices::new(
            directory.path().to_path_buf(),
            directory.path(),
            DesktopRuntimeProfile::Isolated,
            Arc::new(AppRuntimeState::new(Arc::new(NoopEventSink))),
            Arc::new(NoopDiagnosticSink),
        );
        services
            .initialize_database()
            .await
            .expect("initialize database");
        let database = services.database().await.expect("database");
        let source_route_id = create_active_test_route(&database).await;
        let source_models = vec![CodexModelRecord {
            model_id: "source-model".to_owned(),
            display_name: None,
            context_window: None,
        }];
        database
            .replace_codex_models(source_route_id.clone(), source_models.clone())
            .await
            .expect("source models");
        let target = database
            .create_route_with_models(
                CreateRouteInput {
                    name: "Fallback target".to_owned(),
                    base_url: "https://fallback.example/v1".to_owned(),
                    api_key: ApiKey::parse("fallback-key").expect("key"),
                    service_tier_policy: router_core::domain::ServiceTierPolicy::Passthrough,
                    balance_query: None,
                    accept_script_risk: false,
                },
                vec![CodexModelRecord {
                    model_id: "target-model".to_owned(),
                    display_name: None,
                    context_window: None,
                }],
            )
            .await
            .expect("target route");
        let catalog = LocalCodexCatalog::new(directory.path().to_path_buf());
        catalog.publish(&source_models).expect("source catalog");
        let routing = database.routing_state().await.expect("routing");

        let projected = project_fallback_catalog(
            &database,
            directory.path(),
            &services.codex_home,
            &services.codex_projection_gate,
            source_route_id,
            target.route_id,
            routing.selection_generation.saturating_add(1),
        )
        .await;

        assert_eq!(projected, None);
        assert!(catalog.matches(&source_models).expect("catalog match"));
        services.close_database().await;
    }

    #[tokio::test]
    async fn connected_catalog_publication_retry_resumes_projection_and_cannot_be_reused() {
        let directory = TempDir::new().expect("app data fixture");
        let services = DesktopLifecycleServices::new(
            directory.path().to_path_buf(),
            directory.path(),
            DesktopRuntimeProfile::Isolated,
            Arc::new(AppRuntimeState::new(Arc::new(NoopEventSink))),
            Arc::new(NoopDiagnosticSink),
        );
        services
            .initialize_database()
            .await
            .expect("initialize database");
        let database = services.database().await.expect("database");
        create_active_test_route(&database).await;
        services.connect_codex(true).await.expect("connect Codex");
        let catalog_path = LocalCodexCatalog::new(directory.path().to_path_buf()).path();
        fs::create_dir(&catalog_path).expect("block catalog publication");

        let partial = services
            .replace_codex_models(replace_codex_models_input(&["relay-a"], None))
            .await
            .expect("partial model replacement");
        let retry_token = partial.retry_token.expect("backend retry token");
        assert!(partial.retry_required);
        assert!(!partial.projection_applied);
        assert_eq!(
            partial.error_code.as_deref(),
            Some("codex_catalog_publication_failed")
        );

        fs::remove_dir(&catalog_path).expect("unblock catalog publication");
        let applied = services
            .replace_codex_models(replace_codex_models_input(
                &["relay-a"],
                Some(retry_token.clone()),
            ))
            .await
            .expect("retry model replacement");
        assert!(!applied.changed);
        assert!(applied.projection_applied);
        assert!(!applied.retry_required);
        assert_eq!(applied.activation, CodexModelsActivation::RestartCodex);
        assert!(catalog_path.is_file());
        assert_eq!(
            services.codex_status().await.expect("Codex status"),
            router_core::codex_config::CodexConfigStatus::Connected
        );

        let config_path = services.codex_home.join("config.toml");
        let connected = fs::read_to_string(&config_path).expect("connected config");
        let externally_changed = connected.replace(
            "stream_idle_timeout_ms = 300000",
            "stream_idle_timeout_ms = 1",
        );
        assert_ne!(externally_changed, connected);
        fs::write(&config_path, &externally_changed).expect("external config change");

        let invalid_retry = services
            .replace_codex_models(replace_codex_models_input(&["relay-b"], Some(retry_token)))
            .await
            .expect("invalid retry save");
        assert_eq!(
            invalid_retry.activation,
            CodexModelsActivation::ReconnectCodex
        );
        assert_eq!(
            fs::read_to_string(&config_path).expect("unchanged config"),
            externally_changed
        );
        let missing_retry = services
            .replace_codex_models(replace_codex_models_input(&["relay-c"], None))
            .await
            .expect("ordinary changed-config save");
        assert_eq!(
            missing_retry.activation,
            CodexModelsActivation::ReconnectCodex
        );
        assert_eq!(
            fs::read_to_string(&config_path).expect("unchanged config"),
            externally_changed
        );

        services.close_database().await;
    }

    #[tokio::test]
    async fn connected_catalog_retry_never_overwrites_config_changed_after_token_issue() {
        let directory = TempDir::new().expect("app data fixture");
        let services = DesktopLifecycleServices::new(
            directory.path().to_path_buf(),
            directory.path(),
            DesktopRuntimeProfile::Isolated,
            Arc::new(AppRuntimeState::new(Arc::new(NoopEventSink))),
            Arc::new(NoopDiagnosticSink),
        );
        services
            .initialize_database()
            .await
            .expect("initialize database");
        let database = services.database().await.expect("database");
        create_active_test_route(&database).await;
        services.connect_codex(true).await.expect("connect Codex");
        let catalog_path = LocalCodexCatalog::new(directory.path().to_path_buf()).path();
        fs::create_dir(&catalog_path).expect("block catalog publication");

        let partial = services
            .replace_codex_models(replace_codex_models_input(&["relay-a"], None))
            .await
            .expect("partial model replacement");
        let retry_token = partial.retry_token.expect("backend retry token");
        let config_path = services.codex_home.join("config.toml");
        let connected = fs::read_to_string(&config_path).expect("connected config");
        let externally_changed = connected.replace(
            "stream_idle_timeout_ms = 300000",
            "stream_idle_timeout_ms = 1",
        );
        assert_ne!(externally_changed, connected);
        fs::write(&config_path, &externally_changed).expect("external config change");
        fs::remove_dir(&catalog_path).expect("unblock catalog publication");

        let refused = services
            .replace_codex_models(replace_codex_models_input(&["relay-a"], Some(retry_token)))
            .await
            .expect("safe retry result");
        assert!(!refused.changed);
        assert!(!refused.projection_applied);
        assert!(!refused.retry_required);
        assert_eq!(refused.retry_token, None);
        assert_eq!(refused.error_code.as_deref(), Some("codex_config_changed"));
        assert_eq!(refused.activation, CodexModelsActivation::ReconnectCodex);
        assert_eq!(
            fs::read_to_string(&config_path).expect("unchanged config"),
            externally_changed
        );

        services.close_database().await;
    }

    #[tokio::test]
    async fn empty_catalog_updates_config_before_retryable_owned_file_cleanup() {
        let directory = TempDir::new().expect("app data fixture");
        let services = DesktopLifecycleServices::new(
            directory.path().to_path_buf(),
            directory.path(),
            DesktopRuntimeProfile::Isolated,
            Arc::new(AppRuntimeState::new(Arc::new(NoopEventSink))),
            Arc::new(NoopDiagnosticSink),
        );
        services
            .initialize_database()
            .await
            .expect("initialize database");
        let database = services.database().await.expect("database");
        let route_id = create_active_test_route(&database).await;
        database
            .replace_codex_models(
                route_id,
                vec![CodexModelRecord {
                    model_id: "relay-a".to_owned(),
                    display_name: None,
                    context_window: None,
                }],
            )
            .await
            .expect("seed models");
        services.connect_codex(true).await.expect("connect Codex");
        let catalog_path = LocalCodexCatalog::new(directory.path().to_path_buf()).path();
        fs::remove_file(&catalog_path).expect("remove owned catalog fixture");
        fs::create_dir(&catalog_path).expect("block catalog cleanup");

        let partial = services
            .replace_codex_models(replace_codex_models_input(&[], None))
            .await
            .expect("partial empty replacement");
        let retry_token = partial.retry_token.expect("cleanup retry token");
        assert_eq!(
            partial.error_code.as_deref(),
            Some("codex_catalog_cleanup_failed")
        );
        assert!(partial.retry_required);
        assert!(!partial.projection_applied);
        let config = fs::read_to_string(services.codex_home.join("config.toml"))
            .expect("config after pointer removal");
        assert!(!config.contains("model_catalog_json"));
        assert!(catalog_path.is_dir());

        fs::remove_dir(&catalog_path).expect("unblock catalog cleanup");
        let applied = services
            .replace_codex_models(replace_codex_models_input(&[], Some(retry_token)))
            .await
            .expect("cleanup retry");
        assert!(!applied.changed);
        assert!(applied.projection_applied);
        assert!(!applied.retry_required);
        assert!(!catalog_path.exists());

        services.close_database().await;
    }

    #[tokio::test]
    async fn cleanup_retry_preserves_owned_catalog_when_config_changed_after_token_issue() {
        let directory = TempDir::new().expect("app data fixture");
        let services = DesktopLifecycleServices::new(
            directory.path().to_path_buf(),
            directory.path(),
            DesktopRuntimeProfile::Isolated,
            Arc::new(AppRuntimeState::new(Arc::new(NoopEventSink))),
            Arc::new(NoopDiagnosticSink),
        );
        services
            .initialize_database()
            .await
            .expect("initialize database");
        let database = services.database().await.expect("database");
        let route_id = create_active_test_route(&database).await;
        database
            .replace_codex_models(
                route_id,
                vec![CodexModelRecord {
                    model_id: "relay-a".to_owned(),
                    display_name: None,
                    context_window: None,
                }],
            )
            .await
            .expect("seed models");
        services.connect_codex(true).await.expect("connect Codex");
        let catalog_path = LocalCodexCatalog::new(directory.path().to_path_buf()).path();
        fs::remove_file(&catalog_path).expect("remove owned catalog fixture");
        fs::create_dir(&catalog_path).expect("block catalog cleanup");

        let partial = services
            .replace_codex_models(replace_codex_models_input(&[], None))
            .await
            .expect("partial empty replacement");
        let retry_token = partial.retry_token.expect("cleanup retry token");
        let config_path = services.codex_home.join("config.toml");
        let projected = fs::read_to_string(&config_path).expect("empty projection");
        let externally_changed = projected.replace(
            "stream_idle_timeout_ms = 300000",
            "stream_idle_timeout_ms = 1",
        );
        assert_ne!(externally_changed, projected);
        fs::write(&config_path, &externally_changed).expect("external config change");

        let refused = services
            .replace_codex_models(replace_codex_models_input(&[], Some(retry_token)))
            .await
            .expect("safe cleanup retry result");
        assert!(!refused.projection_applied);
        assert!(!refused.retry_required);
        assert_eq!(refused.error_code.as_deref(), Some("codex_config_changed"));
        assert!(catalog_path.is_dir());
        assert_eq!(
            fs::read_to_string(config_path).expect("unchanged config"),
            externally_changed
        );

        services.close_database().await;
    }

    #[tokio::test]
    async fn settings_and_bootstrap_recovery_snapshots_expose_only_safe_projection() {
        let directory = TempDir::new().expect("app data fixture");
        let runtime = Arc::new(AppRuntimeState::new(Arc::new(NoopEventSink)));
        let services = DesktopLifecycleServices::new(
            directory.path().to_path_buf(),
            directory.path(),
            DesktopRuntimeProfile::Isolated,
            runtime,
            Arc::new(NoopDiagnosticSink),
        );
        services
            .initialize_database()
            .await
            .expect("initialize database");

        services
            .settings_snapshot()
            .await
            .expect("settings snapshot");
        let database = services.database().await.expect("database");
        let gateway_token_revision = database
            .critical_revision()
            .await
            .expect("gateway-token revision");
        assert_eq!(gateway_token_revision, 1);
        let recovery = services
            .recovery
            .lock()
            .await
            .as_ref()
            .expect("recovery coordinator")
            .clone();
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let health = recovery.health();
                if health.kind == router_core::recovery::RecoveryHealthKind::Protected
                    && health.live_critical_revision == gateway_token_revision
                    && health.covered_critical_revision == Some(gateway_token_revision)
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("gateway-token revision becomes protected");
        let settings = services
            .settings_snapshot()
            .await
            .expect("protected settings snapshot");
        assert_eq!(
            settings.recovery.kind,
            router_core::recovery::RecoveryHealthKind::Protected
        );
        let protected_health = recovery.health();
        assert_eq!(
            protected_health.live_critical_revision,
            gateway_token_revision
        );
        assert_eq!(
            protected_health.covered_critical_revision,
            Some(gateway_token_revision)
        );
        assert_eq!(settings.recovery.valid_point_count, 2);

        let normal = services
            .recovery_snapshot(&AppLifecycleSnapshot {
                phase: AppLifecyclePhase::Running,
                issue: None,
            })
            .await
            .expect("normal recovery snapshot");
        assert!(!normal.required);
        assert!(normal.candidates.is_empty());
        assert!(!normal.can_start_over);
        assert_eq!(normal.health, Some(settings.recovery));

        services.close_database().await;
        std::fs::remove_file(directory.path().join("router.sqlite3"))
            .expect("remove synthetic primary");
        let required = services
            .recovery_snapshot(&AppLifecycleSnapshot {
                phase: AppLifecyclePhase::RecoveryRequired,
                issue: None,
            })
            .await
            .expect("bootstrap recovery snapshot");
        assert!(required.required);
        assert_eq!(required.candidates.len(), 2);
        assert!(!required.can_start_over);
        assert!(required.startup_issue.is_none());
        assert!(required.health.is_none());
        assert!(RecoveryPointId::parse(&required.candidates[0].point_id).is_ok());
    }

    #[tokio::test]
    async fn lifecycle_desktop_services_start_and_stop_real_core_handles() {
        let directory = TempDir::new().expect("app data fixture");
        let runtime = Arc::new(AppRuntimeState::new(Arc::new(NoopEventSink)));
        let services = DesktopLifecycleServices::new(
            directory.path().to_path_buf(),
            directory.path(),
            DesktopRuntimeProfile::Production,
            runtime,
            Arc::new(NoopDiagnosticSink),
        );
        services
            .initialize_database()
            .await
            .expect("initialize database");
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("reserve test port");
        let port = listener.local_addr().expect("test address").port();
        drop(listener);
        services
            .database()
            .await
            .expect("database")
            .set_proxy_port(port)
            .await
            .expect("set test port");

        services.start_proxy().await.expect("start proxy");
        services.start_balance().await.expect("start balance");
        assert_eq!(
            services
                .proxy
                .lock()
                .await
                .as_ref()
                .expect("proxy handle")
                .address()
                .port(),
            port
        );
        assert_eq!(
            services
                .database()
                .await
                .expect("database")
                .app_settings()
                .await
                .expect("settings")
                .proxy_port,
            port
        );
        assert!(services.balance.lock().await.is_some());

        services.stop_balance().await;
        services.stop_proxy().await;
        services.close_database().await;
        assert!(services.database.lock().await.is_none());
    }

    #[tokio::test]
    async fn proxy_listener_epoch_clears_waiting_only_after_success_and_on_stop() {
        use router_core::proxy::{LogicalRequestActivityPhase, RequestActivityDisposition};

        let directory = TempDir::new().expect("app data fixture");
        let runtime = Arc::new(AppRuntimeState::new(Arc::new(NoopEventSink)));
        let services = DesktopLifecycleServices::new(
            directory.path().to_path_buf(),
            directory.path(),
            DesktopRuntimeProfile::Production,
            runtime,
            Arc::new(NoopDiagnosticSink),
        );
        services
            .initialize_database()
            .await
            .expect("initialize database");
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("reserve test port");
        let port = listener.local_addr().expect("test address").port();
        drop(listener);
        services
            .database()
            .await
            .expect("database")
            .set_proxy_port(port)
            .await
            .expect("set test port");
        services.start_proxy().await.expect("start proxy");

        let waiting = services
            .activity
            .acquire_turn(Some("retired-waiting-turn"))
            .expect("waiting activity");
        waiting
            .reporter()
            .mark_terminal(RequestActivityDisposition::ClientToolHandoff);
        drop(waiting);
        let draining = services
            .activity
            .acquire_turn(Some("retired-live-turn"))
            .expect("draining activity");
        assert_eq!(services.activity.phase(), LogicalRequestActivityPhase::Live);
        assert_eq!(services.activity.snapshot().0, 2);

        let occupied = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("occupied port");
        let occupied_port = occupied.local_addr().expect("occupied address").port();
        services
            .apply_proxy_port(occupied_port)
            .await
            .expect_err("occupied port must preserve current listener");
        assert_eq!(services.activity.phase(), LogicalRequestActivityPhase::Live);
        assert_eq!(services.activity.snapshot().0, 2);
        drop(occupied);

        let replacement =
            TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("reserve replacement test port");
        let replacement_port = replacement
            .local_addr()
            .expect("replacement address")
            .port();
        drop(replacement);
        assert!(
            !services
                .apply_proxy_port(replacement_port)
                .await
                .expect("replace proxy listener")
        );
        assert_eq!(services.activity.snapshot().0, 1);
        assert_eq!(services.activity.phase(), LogicalRequestActivityPhase::Live);
        drop(draining);
        assert_eq!(services.activity.phase(), LogicalRequestActivityPhase::Idle);

        let waiting = services
            .activity
            .acquire_turn(Some("stop-waiting-turn"))
            .expect("stop waiting activity");
        waiting
            .reporter()
            .mark_terminal(RequestActivityDisposition::ClientToolHandoff);
        drop(waiting);
        assert_eq!(
            services.activity.phase(),
            LogicalRequestActivityPhase::Waiting
        );
        services.stop_proxy().await;
        assert_eq!(services.activity.phase(), LogicalRequestActivityPhase::Idle);
        services.close_database().await;
    }

    #[tokio::test]
    async fn balance_query_settings_publish_only_after_validation_and_persistence() {
        let directory = TempDir::new().expect("app data fixture");
        let runtime = Arc::new(AppRuntimeState::new(Arc::new(NoopEventSink)));
        let services = DesktopLifecycleServices::new(
            directory.path().to_path_buf(),
            directory.path(),
            DesktopRuntimeProfile::Isolated,
            Arc::clone(&runtime),
            Arc::new(NoopDiagnosticSink),
        );
        services
            .initialize_database()
            .await
            .expect("initialize database");
        services.start_balance().await.expect("start balance");

        let invalid = BalanceQuerySettingsDto {
            menu_debounce_seconds: 9,
            automatic_refresh_minutes: 30,
        };
        let error = services
            .update_balance_query_settings(invalid)
            .await
            .expect_err("invalid settings");
        assert_eq!(error.code, "menu_balance_debounce_out_of_range");
        let coordinator = services
            .balance
            .lock()
            .await
            .clone()
            .expect("balance coordinator");
        assert_eq!(coordinator.policy(), BalanceQueryPolicy::default());

        let input = BalanceQuerySettingsDto {
            menu_debounce_seconds: 45,
            automatic_refresh_minutes: 120,
        };
        let mutation = services
            .update_balance_query_settings(input)
            .await
            .expect("updated settings");
        let policy = BalanceQueryPolicy::parse(45, 120).expect("policy");
        assert_eq!(coordinator.policy(), policy);
        assert_eq!(
            services
                .database()
                .await
                .expect("database")
                .app_settings()
                .await
                .expect("settings")
                .balance_query_policy,
            policy
        );
        assert_eq!(
            services
                .settings_snapshot()
                .await
                .expect("settings snapshot")
                .balance_query,
            input
        );
        let revision_before_repeat = runtime.bootstrap_snapshot().revision;
        let repeated = services
            .update_balance_query_settings(input)
            .await
            .expect("unchanged settings");
        assert!(revision_before_repeat >= mutation.revision);
        assert_eq!(repeated.revision, revision_before_repeat);

        services.stop_balance().await;
        services.close_database().await;
    }

    #[tokio::test]
    async fn appearance_preference_mutation_persists_projects_and_publishes_once() {
        let directory = TempDir::new().expect("app data fixture");
        let events = Arc::new(RecordingEventSink::default());
        let runtime = Arc::new(AppRuntimeState::new(events.clone()));
        let services = DesktopLifecycleServices::new(
            directory.path().to_path_buf(),
            directory.path(),
            DesktopRuntimeProfile::Isolated,
            Arc::clone(&runtime),
            Arc::new(NoopDiagnosticSink),
        );
        services
            .initialize_database()
            .await
            .expect("initialize database");
        events.0.lock().expect("event sink lock").clear();

        let mutation = services
            .update_appearance_preference(AppearancePreference::Dark)
            .await
            .expect("update appearance");
        assert_eq!(
            services
                .database()
                .await
                .expect("database")
                .app_settings()
                .await
                .expect("settings")
                .appearance_preference,
            AppearancePreference::Dark
        );
        let bootstrap = runtime.bootstrap_snapshot();
        assert_eq!(bootstrap.revision, mutation.revision);
        assert_eq!(bootstrap.appearance_preference, AppearancePreference::Dark);
        assert_eq!(
            events.0.lock().expect("event sink lock").as_slice(),
            &[StateChangedEventDto {
                revision: mutation.revision,
                areas: vec![StateArea::Appearance],
            }]
        );

        let repeated = services
            .update_appearance_preference(AppearancePreference::Dark)
            .await
            .expect("unchanged appearance");
        assert_eq!(repeated.revision, mutation.revision);
        assert_eq!(events.0.lock().expect("event sink lock").len(), 1);

        services.close_database().await;
    }

    #[tokio::test]
    async fn menu_bar_settings_mutation_persists_projects_and_noops() {
        let directory = TempDir::new().expect("app data fixture");
        let events = Arc::new(RecordingEventSink::default());
        let runtime = Arc::new(AppRuntimeState::new(events.clone()));
        let services = DesktopLifecycleServices::new(
            directory.path().to_path_buf(),
            directory.path(),
            DesktopRuntimeProfile::Isolated,
            Arc::clone(&runtime),
            Arc::new(NoopDiagnosticSink),
        );
        services
            .initialize_database()
            .await
            .expect("initialize database");
        events.0.lock().expect("event sink lock").clear();
        let input = MenuBarSettingsDto {
            status_text_enabled: false,
            activity_animation_enabled: true,
        };

        let mutation = services
            .update_menu_bar_settings(input)
            .await
            .expect("update menu bar");
        assert_eq!(runtime.menu_bar_settings(), Some(input));
        assert_eq!(
            services
                .settings_snapshot()
                .await
                .expect("settings")
                .menu_bar,
            input
        );
        let menu_bar_events = events
            .0
            .lock()
            .expect("event sink lock")
            .iter()
            .filter(|event| event.areas == [StateArea::MenuBar])
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(
            menu_bar_events,
            vec![StateChangedEventDto {
                revision: mutation.revision,
                areas: vec![StateArea::MenuBar],
            }]
        );
        let repeated = services
            .update_menu_bar_settings(input)
            .await
            .expect("unchanged menu bar");
        assert_eq!(repeated.revision, runtime.bootstrap_snapshot().revision);
        assert_eq!(
            events
                .0
                .lock()
                .expect("event sink lock")
                .iter()
                .filter(|event| event.areas == [StateArea::MenuBar])
                .count(),
            1
        );

        services.close_database().await;
        let failed = services
            .update_menu_bar_settings(MenuBarSettingsDto {
                status_text_enabled: true,
                activity_animation_enabled: false,
            })
            .await;
        assert!(failed.is_err());
        assert_eq!(runtime.menu_bar_settings(), Some(input));
        assert_eq!(
            events
                .0
                .lock()
                .expect("event sink lock")
                .iter()
                .filter(|event| event.areas == [StateArea::MenuBar])
                .count(),
            1
        );
        let persisted = DatabaseExecutor::open(directory.path().join("router.sqlite3"))
            .expect("reopen database")
            .app_settings()
            .await
            .expect("persisted settings after failed write");
        assert_eq!(menu_bar_settings_dto(&persisted), input);
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn image_route_changes_stay_connected_while_global_changes_require_reconnect() {
        let directory = TempDir::new().expect("app data fixture");
        let events = Arc::new(RecordingEventSink::default());
        let runtime = Arc::new(AppRuntimeState::new(events.clone()));
        let services = DesktopLifecycleServices::new(
            directory.path().to_path_buf(),
            directory.path(),
            DesktopRuntimeProfile::Isolated,
            Arc::clone(&runtime),
            Arc::new(NoopDiagnosticSink),
        );
        services
            .initialize_database()
            .await
            .expect("initialize database");
        let database = services.database().await.expect("database");
        let first = database
            .create_route(CreateRouteInput {
                name: "Image A".to_owned(),
                base_url: "https://image-a.example/v1".to_owned(),
                api_key: ApiKey::parse("image-a-key").expect("API key"),
                service_tier_policy: router_core::domain::ServiceTierPolicy::Passthrough,
                balance_query: None,
                accept_script_risk: false,
            })
            .await
            .expect("first image route");
        let second = database
            .create_route(CreateRouteInput {
                name: "Image B".to_owned(),
                base_url: "https://image-b.example/v1".to_owned(),
                api_key: ApiKey::parse("image-b-key").expect("API key"),
                service_tier_policy: router_core::domain::ServiceTierPolicy::Passthrough,
                balance_query: None,
                accept_script_risk: false,
            })
            .await
            .expect("second image route");

        let invalid_timeout = services
            .update_images_generation_settings(UpdateImagesGenerationSettingsInputDto {
                enabled: true,
                route_id: Some(first.route_id.clone()),
                timeout_secs: 599,
            })
            .await
            .expect_err("timeout below the minimum");
        assert_eq!(
            invalid_timeout.code,
            "images_generation_timeout_out_of_range"
        );
        assert_eq!(invalid_timeout.field.as_deref(), Some("timeoutSecs"));

        services
            .update_images_generation_settings(UpdateImagesGenerationSettingsInputDto {
                enabled: true,
                route_id: Some(first.route_id.clone()),
                timeout_secs: 600,
            })
            .await
            .expect("enable image generation");
        services.connect_codex(false).await.expect("connect Codex");
        assert_eq!(
            services.codex_status().await.expect("connected status"),
            router_core::codex_config::CodexConfigStatus::Connected
        );
        events.0.lock().expect("event sink lock").clear();

        services
            .update_images_generation_settings(UpdateImagesGenerationSettingsInputDto {
                enabled: true,
                route_id: Some(second.route_id.clone()),
                timeout_secs: 900,
            })
            .await
            .expect("change image route");
        let route_only = services.routing.load();
        assert!(route_only.images_generation_enabled);
        assert_eq!(
            route_only
                .images_route
                .as_ref()
                .map(|route| &route.route_id),
            Some(&second.route_id)
        );
        assert_eq!(
            route_only.active.as_ref().map(|route| &route.route_id),
            Some(&first.route_id)
        );
        assert_eq!(
            route_only.images_generation_timeout,
            Duration::from_mins(15)
        );
        assert_eq!(
            services.codex_status().await.expect("route-only status"),
            router_core::codex_config::CodexConfigStatus::Connected
        );
        assert_eq!(
            events.0.lock().expect("event sink lock")[0].areas,
            vec![
                StateArea::Routes,
                StateArea::Route,
                StateArea::Fallback,
                StateArea::ImagesGeneration,
            ]
        );

        events.0.lock().expect("event sink lock").clear();
        services
            .update_images_generation_settings(UpdateImagesGenerationSettingsInputDto {
                enabled: false,
                route_id: Some(second.route_id.clone()),
                timeout_secs: 900,
            })
            .await
            .expect("disable image generation");
        assert!(!services.routing.load().images_generation_enabled);
        assert_eq!(
            services.codex_status().await.expect("disabled status"),
            router_core::codex_config::CodexConfigStatus::Changed
        );
        assert_eq!(
            events.0.lock().expect("event sink lock")[0].areas,
            vec![
                StateArea::Routes,
                StateArea::Route,
                StateArea::Fallback,
                StateArea::ImagesGeneration,
                StateArea::CodexConnection,
            ]
        );

        services.close_database().await;
    }

    #[tokio::test]
    async fn image_mcp_repair_preview_rejects_disabled_and_stale_projection_inputs() {
        let fixture = image_mcp_repair_fixture().await;
        fixture
            .services
            .update_images_generation_settings(UpdateImagesGenerationSettingsInputDto {
                enabled: false,
                route_id: Some(fixture.route_id.clone()),
                timeout_secs: 600,
            })
            .await
            .expect("disable images");
        let disabled = fixture
            .services
            .preview_codex_images_mcp_repair()
            .await
            .expect_err("disabled images must reject repair preview");
        assert_eq!(disabled.code, "codex_images_mcp_repair_not_available");
        fixture
            .services
            .update_images_generation_settings(UpdateImagesGenerationSettingsInputDto {
                enabled: true,
                route_id: Some(fixture.route_id.clone()),
                timeout_secs: 600,
            })
            .await
            .expect("re-enable images");

        let port_bound = fixture
            .services
            .preview_codex_images_mcp_repair()
            .await
            .expect("port-bound preview");
        let original_port = fixture
            .database
            .app_settings()
            .await
            .expect("settings")
            .proxy_port;
        fixture
            .database
            .set_proxy_port(original_port.saturating_add(1))
            .await
            .expect("change proxy port");
        let stale_port = fixture
            .services
            .confirm_codex_images_mcp_repair(port_bound.permit.clone())
            .await
            .expect_err("changed port must stale permit");
        assert_eq!(stale_port.code, "codex_images_mcp_repair_permit_stale");
        let reused = fixture
            .services
            .confirm_codex_images_mcp_repair(port_bound.permit)
            .await
            .expect_err("consumed permit must be invalid");
        assert_eq!(reused.code, "codex_images_mcp_repair_permit_invalid");
        fixture
            .database
            .set_proxy_port(original_port)
            .await
            .expect("restore proxy port");

        let models_bound = fixture
            .services
            .preview_codex_images_mcp_repair()
            .await
            .expect("models-bound preview");
        fixture
            .database
            .replace_codex_models(
                fixture.route_id.clone(),
                vec![CodexModelRecord {
                    model_id: "changed-after-preview".to_owned(),
                    display_name: None,
                    context_window: None,
                }],
            )
            .await
            .expect("change models");
        let stale_models = fixture
            .services
            .confirm_codex_images_mcp_repair(models_bound.permit)
            .await
            .expect_err("changed models must stale permit");
        assert_eq!(stale_models.code, "codex_images_mcp_repair_permit_stale");
        assert_eq!(
            fs::read(&fixture.config_path).expect("unchanged config"),
            fixture.drifted_bytes
        );
        fixture.services.close_database().await;
    }

    #[tokio::test]
    async fn image_mcp_repair_permit_is_config_bound_one_use_and_publishes_success() {
        let fixture = image_mcp_repair_fixture().await;
        let config_bound = fixture
            .services
            .preview_codex_images_mcp_repair()
            .await
            .expect("config-bound preview");
        let mut external = fixture.drifted_bytes.clone();
        external.extend_from_slice(b"\n# changed after preview\n");
        fs::write(&fixture.config_path, &external).expect("external config edit");
        let stale_config = fixture
            .services
            .confirm_codex_images_mcp_repair(config_bound.permit)
            .await
            .expect_err("changed config must stale permit");
        assert_eq!(stale_config.code, "codex_images_mcp_repair_permit_stale");
        assert_eq!(
            fs::read(&fixture.config_path).expect("external config"),
            external
        );

        fs::write(&fixture.config_path, &fixture.drifted_bytes).expect("restore drifted config");
        fixture.events.0.lock().expect("event sink lock").clear();
        let valid = fixture
            .services
            .preview_codex_images_mcp_repair()
            .await
            .expect("valid preview");
        let invalid = fixture
            .services
            .confirm_codex_images_mcp_repair("stale-images-repair-permit".to_owned())
            .await
            .expect_err("wrong permit must be rejected without consuming the current one");
        assert_eq!(invalid.code, "codex_images_mcp_repair_permit_invalid");
        let repaired = fixture
            .services
            .confirm_codex_images_mcp_repair(valid.permit.clone())
            .await
            .expect("confirmed repair");
        assert!(repaired.changed);
        assert_eq!(
            fixture
                .services
                .codex_status()
                .await
                .expect("repaired status"),
            router_core::codex_config::CodexConfigStatus::Connected
        );
        let repaired_config = fs::read_to_string(&fixture.config_path).expect("repaired config");
        assert!(repaired_config.contains("permissions = \"keep\""));
        assert!(repaired_config.contains("http_headers ="));
        assert_eq!(
            fixture
                .database
                .codex_baseline()
                .await
                .expect("baseline query")
                .expect("baseline")
                .raw_bytes,
            fixture.baseline_bytes
        );
        assert_eq!(
            fixture
                .events
                .0
                .lock()
                .expect("event sink lock")
                .last()
                .expect("event")
                .areas,
            vec![StateArea::CodexConnection]
        );
        let reused = fixture
            .services
            .confirm_codex_images_mcp_repair(valid.permit)
            .await
            .expect_err("successful permit must be one use");
        assert_eq!(reused.code, "codex_images_mcp_repair_permit_invalid");
        fixture.services.close_database().await;
    }

    #[tokio::test]
    async fn codex_recovery_commands_are_disconnected_guarded_and_one_use() {
        let directory = TempDir::new().expect("app data fixture");
        let events = Arc::new(RecordingEventSink::default());
        let runtime = Arc::new(AppRuntimeState::new(events));
        let services = DesktopLifecycleServices::new(
            directory.path().to_path_buf(),
            directory.path(),
            DesktopRuntimeProfile::Isolated,
            runtime,
            Arc::new(NoopDiagnosticSink),
        );
        services
            .initialize_database()
            .await
            .expect("initialize database");
        services
            .connect_codex(true)
            .await
            .expect("connect from absent config");
        let connected = services
            .preview_update_codex_recovery()
            .await
            .expect_err("connected update must fail");
        assert_eq!(connected.code, "codex_recovery_not_disconnected");
        services.restore_codex().await.expect("disconnect");

        let config_path = services.codex_home.join("config.toml");
        fs::write(&config_path, b"model = \"disconnect-target\"\n").expect("recovery target");
        let update = services
            .preview_update_codex_recovery()
            .await
            .expect("update preview");
        assert!(update.current_exists);
        assert!(!update.recovery_target_exists);
        let wrong = services
            .confirm_update_codex_recovery("wrong-permit".to_owned())
            .await
            .expect_err("wrong permit");
        assert_eq!(wrong.code, "codex_recovery_preview_stale");
        services
            .confirm_update_codex_recovery(update.permit)
            .await
            .expect("update recovery");
        let snapshot = services
            .settings_snapshot()
            .await
            .expect("settings snapshot");
        assert_eq!(snapshot.original_backup.original_exists, Some(false));
        assert_eq!(snapshot.recovery_config.original_exists, Some(true));

        fs::write(&config_path, b"model = \"discarded\"\n").expect("later edit");
        services
            .restore_codex()
            .await
            .expect("restore updated target");
        assert_eq!(
            fs::read(&config_path).expect("restored target"),
            b"model = \"disconnect-target\"\n"
        );
        let reset = services
            .preview_reset_codex_recovery_to_baseline()
            .await
            .expect("reset preview");
        assert!(!reset.original_exists);
        services
            .confirm_reset_codex_recovery_to_baseline(reset.permit.clone())
            .await
            .expect("reset recovery");
        assert!(!config_path.exists());
        let reused = services
            .confirm_reset_codex_recovery_to_baseline(reset.permit)
            .await
            .expect_err("one-use reset permit");
        assert_eq!(reused.code, "codex_recovery_preview_stale");
        let snapshot = services.settings_snapshot().await.expect("reset snapshot");
        assert_eq!(snapshot.recovery_config.original_exists, Some(false));
        services.close_database().await;
    }

    #[tokio::test]
    async fn fallback_activation_preserves_a_newer_image_route_projection() {
        let directory = TempDir::new().expect("app data fixture");
        let runtime = Arc::new(AppRuntimeState::new(Arc::new(NoopEventSink)));
        let services = DesktopLifecycleServices::new(
            directory.path().to_path_buf(),
            directory.path(),
            DesktopRuntimeProfile::Isolated,
            Arc::clone(&runtime),
            Arc::new(NoopDiagnosticSink),
        );
        services
            .initialize_database()
            .await
            .expect("initialize database");
        let database = services.database().await.expect("database");
        let mut routes = Vec::new();
        for name in ["Image A", "Image B"] {
            routes.push(
                database
                    .create_route(CreateRouteInput {
                        name: name.to_owned(),
                        base_url: format!("https://{}.example/v1", name.replace(' ', "-")),
                        api_key: ApiKey::parse(&format!("{}-key", name.replace(' ', "-")))
                            .expect("API key"),
                        service_tier_policy: router_core::domain::ServiceTierPolicy::Passthrough,
                        balance_query: None,
                        accept_script_risk: false,
                    })
                    .await
                    .expect("route"),
            );
        }
        database
            .set_fallback_enabled(true)
            .await
            .expect("enable fallback");
        database
            .set_images_generation_settings(
                true,
                Some(routes[0].route_id.clone()),
                ImagesGenerationTimeout::parse(700).expect("timeout"),
            )
            .await
            .expect("initial image route");
        services
            .refresh_route_projection(&database)
            .await
            .expect("initial routing projection");

        let captured = services.routing.load();
        let health_proof =
            fallback_test_advance_proof(&services, &routes[0].route_id, captured.health_generation);
        let target = captured
            .next_after(&routes[0].route_id)
            .expect("next fallback route");
        services
            .update_images_generation_settings(UpdateImagesGenerationSettingsInputDto {
                enabled: true,
                route_id: Some(routes[1].route_id.clone()),
                timeout_secs: 1_000,
            })
            .await
            .expect("publish newer image route");

        let activated = fallback_test_activator(&services, &database, &runtime)
            .activate_next(FallbackActivationRequest {
                request_id: "image-route-fallback-race".to_owned(),
                routing: captured,
                current_route_id: routes[0].route_id.clone(),
                target_route: target,
                requested_model: "test-model".to_owned(),
                skipped_routes: Vec::new(),
                mode: router_core::proxy::FallbackActivationMode::Advance,
                health_proof: Some(health_proof),
            })
            .await
            .expect("fallback activation")
            .expect("activation succeeds");

        assert_eq!(
            activated.active.as_ref().map(|route| &route.route_id),
            Some(&routes[1].route_id)
        );
        assert!(activated.images_generation_enabled);
        assert_eq!(
            activated.images_route.as_ref().map(|route| &route.route_id),
            Some(&routes[1].route_id)
        );
        assert_eq!(
            activated.images_generation_timeout,
            Duration::from_secs(1_000)
        );
        assert_eq!(
            services
                .routing
                .load()
                .images_route
                .as_ref()
                .map(|route| &route.route_id),
            Some(&routes[1].route_id)
        );

        services.close_database().await;
    }

    #[test]
    fn runtime_profile_fails_closed_for_every_non_production_identifier() {
        assert_eq!(
            DesktopRuntimeProfile::from_identifier(PRODUCTION_APP_IDENTIFIER),
            DesktopRuntimeProfile::Production
        );
        assert_eq!(
            DesktopRuntimeProfile::from_identifier("com.relax.airouter.qa"),
            DesktopRuntimeProfile::Isolated
        );
        assert_eq!(
            DesktopRuntimeProfile::from_identifier("com.relax.airouter-typo"),
            DesktopRuntimeProfile::Isolated
        );
    }

    #[test]
    fn base_url_validation_errors_map_to_safe_field_specific_ipc_messages() {
        for (input, code, message) in [
            (
                "https://example.test/v1/responses/responses",
                "base_url_duplicate_responses",
                "Responses 地址不能重复包含 /responses。",
            ),
            (
                "https://example.test/v1/chat/completions",
                "base_url_unsupported_endpoint",
                "仅支持 Responses API 地址。",
            ),
            (
                "ftp://example.test",
                "base_url_invalid",
                "请输入有效的 HTTP(S) 地址。",
            ),
        ] {
            let validation = BaseUrl::parse(input).expect_err("invalid Base URL");
            let ipc = map_validation_error(&validation);
            assert_eq!(ipc.code, code);
            assert_eq!(ipc.message, message);
            assert_eq!(ipc.field.as_deref(), Some("baseUrl"));
            assert!(!ipc.retryable);
        }
    }

    #[test]
    fn runtime_profile_isolates_codex_home_and_preserves_production_port() {
        let app_data = std::path::Path::new("/tmp/ai-router-app-data");
        let user_home = std::path::Path::new("/tmp/ai-router-user");

        assert_eq!(
            DesktopRuntimeProfile::Production.codex_home(app_data, user_home),
            user_home.join(".codex")
        );
        assert_eq!(
            DesktopRuntimeProfile::Isolated.codex_home(app_data, user_home),
            app_data.join("codex-home")
        );
        assert_eq!(
            DesktopRuntimeProfile::Production.proxy_bind_port(32_189),
            32_189
        );
        assert_eq!(DesktopRuntimeProfile::Isolated.proxy_bind_port(32_189), 0);
    }

    #[test]
    fn fallback_projection_preserves_counts_and_positions_above_u8() {
        let participants = (0..300)
            .map(|index| {
                Arc::new(RouteSnapshot {
                    route_id: RouteId::from_string(format!("route-{index}")),
                    name: format!("Route {index}"),
                    base_url: BaseUrl::parse("https://example.test/v1").expect("base URL"),
                    api_key: Arc::new(ApiKey::parse("test-key").expect("API key")),
                    service_tier_policy: router_core::domain::ServiceTierPolicy::Passthrough,
                    fallback_excluded_models: Arc::new(std::collections::HashSet::new()),
                })
            })
            .collect::<Vec<_>>();
        let snapshot = RoutingSnapshot {
            active: participants.last().cloned(),
            participants,
            enabled: true,
            selection_generation: 1,
            health_generation: 1,
            config_revision: 1,
            images_generation_enabled: false,
            images_route: None,
            images_generation_timeout: Duration::from_mins(10),
        };

        let projected = fallback_state(&snapshot).expect("fallback projection");
        assert_eq!(projected.participant_count, 300);
        assert_eq!(projected.active_position, Some(300));
        assert!(projected.has_next);
    }

    #[tokio::test]
    async fn participant_count_mutation_publishes_fallback_and_keeps_active_outside() {
        let directory = TempDir::new().expect("app data fixture");
        let events = Arc::new(RecordingEventSink::default());
        let runtime = Arc::new(AppRuntimeState::new(events.clone()));
        let services = DesktopLifecycleServices::new(
            directory.path().to_path_buf(),
            directory.path(),
            DesktopRuntimeProfile::Isolated,
            Arc::clone(&runtime),
            Arc::new(NoopDiagnosticSink),
        );
        services
            .initialize_database()
            .await
            .expect("initialize database");
        let database = services.database().await.expect("database");
        let mut route_ids = Vec::new();
        for name in ["A", "B", "C"] {
            route_ids.push(create_fallback_test_route(&database, name).await);
        }
        services
            .refresh_route_projection(&database)
            .await
            .expect("initial projection");
        services
            .set_fallback_enabled(true)
            .await
            .expect("enable fallback");
        services
            .activate_route(route_ids[2].clone())
            .await
            .expect("activate third route");
        events.0.lock().expect("event sink lock").clear();

        let changed = services
            .set_fallback_participant_count(2)
            .await
            .expect("shrink participants");
        let bootstrap = runtime.bootstrap_snapshot();
        assert_eq!(bootstrap.revision, changed.revision);
        assert_eq!(bootstrap.active_route_id, Some(route_ids[2].clone()));
        assert!(bootstrap.fallback.enabled);
        assert_eq!(bootstrap.fallback.participant_count, 2);
        assert_eq!(bootstrap.fallback.active_position, None);
        assert!(!bootstrap.fallback.has_next);
        let routing = services.routing.load();
        assert_eq!(
            routing.active.as_ref().map(|route| &route.route_id),
            Some(&route_ids[2])
        );
        assert_eq!(routing.participants.len(), 2);
        assert_eq!(
            events.0.lock().expect("event sink lock")[0].areas,
            vec![StateArea::Fallback]
        );

        let unchanged = services
            .set_fallback_participant_count(2)
            .await
            .expect("same participant count");
        assert_eq!(unchanged.revision, changed.revision);
        assert_eq!(events.0.lock().expect("event sink lock").len(), 1);

        assert!(
            database
                .set_fallback_participant_count(1)
                .await
                .expect("simulate a committed but unpublished boundary")
        );
        assert_eq!(services.routing.load().participants.len(), 2);
        let repaired = services
            .set_fallback_participant_count(1)
            .await
            .expect("repair unpublished boundary");
        let repaired_bootstrap = runtime.bootstrap_snapshot();
        assert_eq!(repaired_bootstrap.revision, repaired.revision);
        assert_eq!(repaired_bootstrap.fallback.participant_count, 1);
        assert!(!repaired_bootstrap.fallback.enabled);
        assert_eq!(services.routing.load().participants.len(), 1);
        assert_eq!(events.0.lock().expect("event sink lock").len(), 2);
        assert_eq!(
            events.0.lock().expect("event sink lock")[1].areas,
            vec![
                StateArea::Routes,
                StateArea::Route,
                StateArea::Fallback,
                StateArea::ImagesGeneration,
            ]
        );

        services.close_database().await;
    }

    #[tokio::test]
    async fn atomic_route_reorder_publishes_once_and_rejects_stale_candidates() {
        let directory = TempDir::new().expect("app data fixture");
        let events = Arc::new(RecordingEventSink::default());
        let runtime = Arc::new(AppRuntimeState::new(events.clone()));
        let services = DesktopLifecycleServices::new(
            directory.path().to_path_buf(),
            directory.path(),
            DesktopRuntimeProfile::Isolated,
            Arc::clone(&runtime),
            Arc::new(NoopDiagnosticSink),
        );
        services
            .initialize_database()
            .await
            .expect("initialize database");
        let database = services.database().await.expect("database");
        let mut route_ids = Vec::new();
        for name in ["A", "B", "C"] {
            route_ids.push(create_fallback_test_route(&database, name).await);
        }
        services
            .refresh_route_projection(&database)
            .await
            .expect("initial projection");
        events.0.lock().expect("event sink lock").clear();
        let before = runtime.bootstrap_snapshot();
        let candidate = vec![
            route_ids[2].clone(),
            route_ids[0].clone(),
            route_ids[1].clone(),
        ];

        let changed = services
            .reorder_routes_and_fallback(ReorderRoutesAndFallbackInputDto {
                ordered_route_ids: candidate.clone(),
                participant_count: 2,
                expected_config_revision: before.fallback.config_revision,
            })
            .await
            .expect("atomic reorder");
        let after = runtime.bootstrap_snapshot();
        assert_eq!(changed.revision, after.revision);
        assert_eq!(
            after
                .routes
                .iter()
                .map(|route| route.route_id.clone())
                .collect::<Vec<_>>(),
            candidate
        );
        assert_eq!(after.fallback.participant_count, 2);
        assert_eq!(
            after.fallback.config_revision,
            before.fallback.config_revision + 1
        );
        let published = services.routing.load();
        assert_eq!(published.config_revision, after.fallback.config_revision);
        assert_eq!(
            published
                .participants
                .iter()
                .map(|route| route.route_id.clone())
                .collect::<Vec<_>>(),
            candidate[..2]
        );
        assert_eq!(events.0.lock().expect("event sink lock").len(), 1);
        assert_eq!(
            events.0.lock().expect("event sink lock")[0].areas,
            vec![
                StateArea::Routes,
                StateArea::Route,
                StateArea::Fallback,
                StateArea::ImagesGeneration,
            ]
        );

        let no_op = services
            .reorder_routes_and_fallback(ReorderRoutesAndFallbackInputDto {
                ordered_route_ids: candidate.clone(),
                participant_count: 2,
                expected_config_revision: after.fallback.config_revision,
            })
            .await
            .expect("published no-op");
        assert_eq!(no_op.revision, changed.revision);
        assert_eq!(events.0.lock().expect("event sink lock").len(), 1);

        let stale = services
            .reorder_routes_and_fallback(ReorderRoutesAndFallbackInputDto {
                ordered_route_ids: route_ids.clone(),
                participant_count: 3,
                expected_config_revision: before.fallback.config_revision,
            })
            .await
            .expect_err("stale candidate");
        assert_eq!(stale.code, "routing_configuration_stale");
        assert_eq!(runtime.bootstrap_snapshot(), after);
        assert_eq!(events.0.lock().expect("event sink lock").len(), 1);

        services.close_database().await;
    }

    #[tokio::test]
    async fn atomic_route_reorder_noop_repairs_an_unpublished_commit() {
        let directory = TempDir::new().expect("app data fixture");
        let events = Arc::new(RecordingEventSink::default());
        let runtime = Arc::new(AppRuntimeState::new(events.clone()));
        let services = DesktopLifecycleServices::new(
            directory.path().to_path_buf(),
            directory.path(),
            DesktopRuntimeProfile::Isolated,
            Arc::clone(&runtime),
            Arc::new(NoopDiagnosticSink),
        );
        services
            .initialize_database()
            .await
            .expect("initialize database");
        let database = services.database().await.expect("database");
        let mut route_ids = Vec::new();
        for name in ["A", "B", "C"] {
            route_ids.push(create_fallback_test_route(&database, name).await);
        }
        services
            .refresh_route_projection(&database)
            .await
            .expect("initial projection");
        events.0.lock().expect("event sink lock").clear();
        let published_revision = services.routing.load().config_revision;
        let repair_candidate = vec![
            route_ids[1].clone(),
            route_ids[2].clone(),
            route_ids[0].clone(),
        ];

        assert!(
            database
                .reorder_routes_and_fallback(repair_candidate.clone(), 1, published_revision,)
                .await
                .expect("unpublished durable reorder")
        );
        let durable = database.routing_state().await.expect("durable routing");
        assert_ne!(
            services.routing.load().config_revision,
            durable.fallback.config_revision
        );
        let repaired = services
            .reorder_routes_and_fallback(ReorderRoutesAndFallbackInputDto {
                ordered_route_ids: repair_candidate.clone(),
                participant_count: 1,
                expected_config_revision: durable.fallback.config_revision,
            })
            .await
            .expect("repair unpublished projection");

        let repaired_snapshot = runtime.bootstrap_snapshot();
        assert_eq!(repaired.revision, repaired_snapshot.revision);
        assert_eq!(
            repaired_snapshot
                .routes
                .iter()
                .map(|route| route.route_id.clone())
                .collect::<Vec<_>>(),
            repair_candidate
        );
        assert_eq!(repaired_snapshot.fallback.participant_count, 1);
        assert_eq!(
            repaired_snapshot.fallback.config_revision,
            durable.fallback.config_revision
        );
        assert_eq!(events.0.lock().expect("event sink lock").len(), 1);

        services.close_database().await;
    }

    #[tokio::test]
    async fn concurrent_runtime_reorders_publish_only_the_revision_winner() {
        let directory = TempDir::new().expect("app data fixture");
        let events = Arc::new(RecordingEventSink::default());
        let runtime = Arc::new(AppRuntimeState::new(events.clone()));
        let services = DesktopLifecycleServices::new(
            directory.path().to_path_buf(),
            directory.path(),
            DesktopRuntimeProfile::Isolated,
            Arc::clone(&runtime),
            Arc::new(NoopDiagnosticSink),
        );
        services
            .initialize_database()
            .await
            .expect("initialize database");
        let database = services.database().await.expect("database");
        let mut route_ids = Vec::new();
        for name in ["A", "B", "C"] {
            route_ids.push(create_fallback_test_route(&database, name).await);
        }
        services
            .refresh_route_projection(&database)
            .await
            .expect("initial projection");
        events.0.lock().expect("event sink lock").clear();
        let expected_revision = runtime.bootstrap_snapshot().fallback.config_revision;
        let first_candidate = vec![
            route_ids[1].clone(),
            route_ids[0].clone(),
            route_ids[2].clone(),
        ];
        let second_candidate = vec![
            route_ids[2].clone(),
            route_ids[1].clone(),
            route_ids[0].clone(),
        ];

        let (first_result, second_result) = tokio::join!(
            services.reorder_routes_and_fallback(ReorderRoutesAndFallbackInputDto {
                ordered_route_ids: first_candidate.clone(),
                participant_count: 2,
                expected_config_revision: expected_revision,
            }),
            services.reorder_routes_and_fallback(ReorderRoutesAndFallbackInputDto {
                ordered_route_ids: second_candidate.clone(),
                participant_count: 2,
                expected_config_revision: expected_revision,
            })
        );
        let results = [&first_result, &second_result];
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter_map(|result| result.as_ref().err())
                .filter(|error| error.code == "routing_configuration_stale")
                .count(),
            1
        );

        let durable_order = database
            .list_routes()
            .await
            .expect("durable routes")
            .into_iter()
            .map(|route| route.route_id)
            .collect::<Vec<_>>();
        assert!(durable_order == first_candidate || durable_order == second_candidate);
        assert_eq!(
            runtime
                .bootstrap_snapshot()
                .routes
                .into_iter()
                .map(|route| route.route_id)
                .collect::<Vec<_>>(),
            durable_order
        );
        let durable_routing = database.routing_state().await.expect("durable routing");
        assert_eq!(
            services.routing.load().config_revision,
            durable_routing.fallback.config_revision
        );
        assert_eq!(
            durable_routing.fallback.config_revision,
            expected_revision + 1
        );
        assert_eq!(events.0.lock().expect("event sink lock").len(), 1);

        services.close_database().await;
    }

    #[tokio::test]
    async fn boundary_and_automatic_activation_serialize_in_both_commit_orders() {
        for (initial_count, next_count) in [(2, 3), (3, 2)] {
            for boundary_first in [true, false] {
                assert_boundary_automatic_activation_order(
                    initial_count,
                    next_count,
                    boundary_first,
                )
                .await;
            }
        }
    }

    #[tokio::test]
    async fn manual_route_change_queued_before_fallback_keeps_the_newer_selection() {
        let directory = TempDir::new().expect("app data fixture");
        let runtime = Arc::new(AppRuntimeState::new(Arc::new(NoopEventSink)));
        let services = DesktopLifecycleServices::new(
            directory.path().to_path_buf(),
            directory.path(),
            DesktopRuntimeProfile::Isolated,
            Arc::clone(&runtime),
            Arc::new(NoopDiagnosticSink),
        );
        services
            .initialize_database()
            .await
            .expect("initialize database");
        let database = services.database().await.expect("database");
        let mut routes = Vec::new();
        for name in ["A", "B", "C"] {
            routes.push(
                database
                    .create_route(CreateRouteInput {
                        name: name.to_owned(),
                        base_url: format!("https://{name}.example/v1"),
                        api_key: ApiKey::parse(&format!("{name}-key")).expect("API key"),
                        service_tier_policy: router_core::domain::ServiceTierPolicy::Passthrough,
                        balance_query: None,
                        accept_script_risk: false,
                    })
                    .await
                    .expect("route"),
            );
        }
        database
            .set_fallback_enabled(true)
            .await
            .expect("enable fallback");
        services
            .refresh_route_projection(&database)
            .await
            .expect("routing projection");
        let captured = services.routing.load();
        let health_proof =
            fallback_test_advance_proof(&services, &routes[0].route_id, captured.health_generation);
        let first = captured.active.as_ref().expect("active route");
        let second = captured
            .next_after(&first.route_id)
            .expect("next fallback route");
        let activator = DesktopFallbackActivator {
            database: database.clone(),
            routing: services.routing.clone(),
            runtime_state: runtime,
            routing_write_gate: Arc::clone(&services.routing_write_gate),
            codex_projection_gate: Arc::clone(&services.codex_projection_gate),
            app_data_dir: services.app_data_dir.clone(),
            codex_home: services.codex_home.clone(),
            transitions: FallbackTransitionCoordinator::new(
                database.clone(),
                Arc::clone(&services.runtime_state),
            ),
            route_health: Arc::clone(&services.route_health),
        };
        let automatic_request = FallbackActivationRequest {
            request_id: "fallback-race".to_owned(),
            routing: captured,
            current_route_id: routes[0].route_id.clone(),
            target_route: second,
            requested_model: "test-model".to_owned(),
            skipped_routes: Vec::new(),
            mode: router_core::proxy::FallbackActivationMode::Advance,
            health_proof: Some(health_proof),
        };

        let manual = services.activate_route(routes[2].route_id.clone());
        let automatic = activator.activate_next(automatic_request);
        let (manual_result, automatic_result) = tokio::join!(biased; manual, automatic);

        manual_result.expect("manual activation");
        assert!(automatic_result.expect("automatic activation").is_none());
        assert_eq!(
            database.active_route_id().await.expect("active route ID"),
            Some(routes[2].route_id.clone())
        );
        assert_eq!(
            services
                .routing
                .load()
                .active
                .as_ref()
                .map(|route| &route.route_id),
            Some(&routes[2].route_id)
        );
    }

    #[tokio::test]
    async fn isolated_runtime_persists_its_os_assigned_proxy_port() {
        let directory = TempDir::new().expect("app data fixture");
        let runtime = Arc::new(AppRuntimeState::new(Arc::new(NoopEventSink)));
        let services = DesktopLifecycleServices::new(
            directory.path().to_path_buf(),
            &directory.path().join("user-home"),
            DesktopRuntimeProfile::Isolated,
            runtime,
            Arc::new(NoopDiagnosticSink),
        );
        services
            .initialize_database()
            .await
            .expect("initialize database");
        services
            .database()
            .await
            .expect("database")
            .set_proxy_port(1)
            .await
            .expect("set sentinel port");

        services.start_proxy().await.expect("start isolated proxy");

        let selected_port = services
            .proxy
            .lock()
            .await
            .as_ref()
            .expect("proxy handle")
            .address()
            .port();
        let persisted_port = services
            .database()
            .await
            .expect("database")
            .app_settings()
            .await
            .expect("settings")
            .proxy_port;
        assert_ne!(selected_port, 0);
        assert_ne!(selected_port, 1);
        assert_eq!(persisted_port, selected_port);
        assert_eq!(services.codex_home, directory.path().join("codex-home"));

        services.stop_proxy().await;
        services.close_database().await;
    }
}
