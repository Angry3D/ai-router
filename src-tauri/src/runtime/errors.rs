use router_core::{
    app_api::RouteModelsErrorCategory,
    codex_auth::CodexAuthError,
    codex_catalog::CodexCatalogError,
    codex_config::CodexConfigError,
    domain::{CodexModelValidationError, FallbackExcludedModelValidationError, ValidationError},
    lifecycle::{AppLifecycleIssue, AppLifecyclePhase, AppLifecycleSnapshot, LifecycleFailure},
    proxy::{McpImageAssetMaintenanceError, ProxyPortError, SystemProxyError},
    recovery::{
        DatabaseStartupIssue, RecoveryError, classify_recovery_startup_error,
        classify_storage_startup_error,
    },
    state::IpcErrorDto,
    storage::StorageError,
    upstream_models::UpstreamModelsErrorKind,
};
pub(super) fn map_validation_error(error: &ValidationError) -> IpcErrorDto {
    IpcErrorDto {
        code: error.code.to_owned(),
        message: match error.code {
            "base_url_invalid" => "请输入有效的 HTTP(S) 地址。",
            "base_url_too_long" => "地址过长。",
            "base_url_unsupported_endpoint" => "地址必须匹配所选的上游协议。",
            "base_url_duplicate_responses" => "Responses 地址不能重复包含 /responses。",
            "base_url_duplicate_chat_completions" => {
                "Chat Completions 地址不能重复包含 /chat/completions。"
            }
            "chat_bridge_unsupported_request" => {
                "该请求包含 Chat Completions 上游无法表达的内容，请改用 Responses 上游。"
            }
            "images_generation_timeout_out_of_range" => "生成等待上限需为 600 至 3600 秒。",
            "images_generation_model_required" => "请输入生图模型。",
            "images_generation_model_control_character" => "生图模型不能包含控制字符。",
            "images_generation_model_too_long" => "生图模型过长。",
            _ => "输入内容无效。",
        }
        .to_owned(),
        retryable: false,
        field: Some(error.field.to_owned()),
    }
}

pub(crate) fn map_system_proxy_error(error: SystemProxyError) -> IpcErrorDto {
    let message = match error {
        SystemProxyError::ReadFailed => {
            "无法读取 macOS 系统代理设置。请检查系统设置或切换为自定义代理。"
        }
        SystemProxyError::InvalidSettings => {
            "macOS 系统代理设置无效。请检查系统设置或切换为自定义代理。"
        }
        SystemProxyError::AutomaticProxyUnsupported => {
            "此目标仅有 PAC/WPAD 自动代理规则，应用不执行这些规则。请配置系统手动代理或切换为自定义代理。"
        }
    };
    ipc_error(error.code(), message, true)
}

pub(super) fn map_mcp_image_asset_error(error: McpImageAssetMaintenanceError) -> IpcErrorDto {
    match error {
        McpImageAssetMaintenanceError::Unavailable => ipc_error(
            "mcp_image_assets_unavailable",
            "图片目录暂时无法读取。",
            true,
        ),
        McpImageAssetMaintenanceError::Busy => {
            ipc_error("mcp_image_assets_busy", "图片正在生成，请稍后重试。", true)
        }
        McpImageAssetMaintenanceError::PartialFailure => ipc_error(
            "mcp_image_assets_clear_failed",
            "部分图片无法清除，请刷新后重试。",
            true,
        ),
    }
}

pub(super) fn map_storage_error(error: StorageError) -> IpcErrorDto {
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

pub(super) fn map_codex_model_validation_error(error: &CodexModelValidationError) -> IpcErrorDto {
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

pub(super) fn map_fallback_excluded_model_validation_error(
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

pub(super) fn map_codex_catalog_error(_error: CodexCatalogError) -> IpcErrorDto {
    ipc_error(
        "codex_catalog_publication_failed",
        "自定义模型目录写入失败。",
        true,
    )
}
pub(super) fn map_database_startup_failure(error: &StorageError) -> LifecycleFailure {
    LifecycleFailure::DatabaseIssue(classify_storage_startup_error(error))
}

pub(super) fn map_recovery_lifecycle_failure(error: &RecoveryError) -> LifecycleFailure {
    classify_recovery_startup_error(error).map_or(LifecycleFailure::RecoveryRequired, |issue| {
        LifecycleFailure::DatabaseIssue(issue)
    })
}

#[derive(Clone, Copy)]
pub(super) enum RecoveryOperation {
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

pub(super) fn map_recovery_error(
    error: &RecoveryError,
    operation: RecoveryOperation,
) -> IpcErrorDto {
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
        RecoveryError::UnsafeFilesystemObject
        | RecoveryError::FutureSchema
        | RecoveryError::DirectoryInUse => {
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

pub(super) fn map_database_startup_issue(issue: DatabaseStartupIssue) -> IpcErrorDto {
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
        DatabaseStartupIssue::DirectoryInUse => ipc_error(
            "database_directory_in_use",
            "另一个 AI Router 进程正在使用该数据目录。",
            true,
        ),
    }
}
pub(super) fn map_recovery_lifecycle_result(
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

pub(super) fn map_balance_error(error: &router_core::balance::BalanceError) -> IpcErrorDto {
    if error.category == router_core::balance::BalanceErrorCategory::SystemProxy {
        return ipc_error(
            "system_proxy_unavailable",
            "系统代理无法用于此目标。请检查系统手动代理设置或切换为自定义代理；应用不执行 PAC/WPAD 规则。",
            true,
        );
    }
    ipc_error("balance_query_failed", "余额查询失败。", true)
}

pub(super) const fn route_models_error_category(
    kind: UpstreamModelsErrorKind,
) -> RouteModelsErrorCategory {
    match kind {
        UpstreamModelsErrorKind::Unauthorized => RouteModelsErrorCategory::Unauthorized,
        UpstreamModelsErrorKind::NotFound => RouteModelsErrorCategory::NotFound,
        UpstreamModelsErrorKind::SystemProxy(_) => RouteModelsErrorCategory::SystemProxy,
        UpstreamModelsErrorKind::Network => RouteModelsErrorCategory::Network,
        UpstreamModelsErrorKind::Timeout => RouteModelsErrorCategory::Timeout,
        UpstreamModelsErrorKind::HttpStatus => RouteModelsErrorCategory::HttpStatus,
        UpstreamModelsErrorKind::TooLarge => RouteModelsErrorCategory::TooLarge,
        UpstreamModelsErrorKind::InvalidResponse => RouteModelsErrorCategory::InvalidResponse,
    }
}

pub(super) fn map_proxy_port_error(error: &ProxyPortError) -> IpcErrorDto {
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

pub(super) fn map_codex_error(error: &CodexConfigError) -> IpcErrorDto {
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

pub(super) fn ipc_field_error(code: &str, message: &str, field: &str) -> IpcErrorDto {
    IpcErrorDto {
        code: code.to_owned(),
        message: message.to_owned(),
        retryable: false,
        field: Some(field.to_owned()),
    }
}

pub(super) fn ipc_error(code: &str, message: &str, retryable: bool) -> IpcErrorDto {
    IpcErrorDto {
        code: code.to_owned(),
        message: message.to_owned(),
        retryable,
        field: None,
    }
}
/// Maps one codex-auth domain error to its stable IPC code and Chinese copy.
pub(super) fn map_codex_auth_error(error: &CodexAuthError) -> IpcErrorDto {
    let message = match error {
        CodexAuthError::BrowserMissing => {
            "未检测到 Chrome 系浏览器。请安装或打开 Google Chrome 后重试。"
        }
        CodexAuthError::BrowserUnsupported => {
            "检测到浏览器，但当前版本仅支持已验证的 Google Chrome。未做任何修改。"
        }
        CodexAuthError::UnsupportedEncryption => {
            "浏览器 Cookie 加密格式暂不支持（可能是浏览器新版本变更）。未做任何修改。"
        }
        CodexAuthError::KeychainDenied => {
            "钥匙串授权被拒绝。请重试并在系统弹窗中选择「始终允许」。"
        }
        CodexAuthError::CookieStoreUnreadable => "无法读取浏览器 Cookie 数据库（权限不足）。",
        CodexAuthError::ProfileNotLoggedIn => {
            "未检测到已登录的 Chrome 会话。请先在 Chrome 中登录 chatgpt.com，然后重试。"
        }
        CodexAuthError::SessionFetchFailed => {
            "获取 ChatGPT 会话失败（网络异常或被拦截）。未做任何修改。"
        }
        CodexAuthError::SessionInvalid => "会话响应缺少必需字段，无法生成凭证。",
        CodexAuthError::StoreModeUnsupported => {
            "当前 Codex 未使用 file 模式存储凭证，无法通过 auth.json 替换。请改为 file 后重试。"
        }
        CodexAuthError::TargetConflict => {
            "目标文件状态异常（符号链接或在写入前被修改），已停止且未覆盖。"
        }
        CodexAuthError::WriteFailed => "写入失败，原文件保持不变。",
        CodexAuthError::RestoreFailed => "还原失败，原文件保持不变。",
    };
    ipc_error(error.code(), message, error.retryable())
}

#[cfg(test)]
mod system_proxy_tests {
    use super::*;

    #[test]
    fn proxy_errors_are_bounded_actionable_and_not_field_validation() {
        for (error, code) in [
            (SystemProxyError::ReadFailed, "system_proxy_read_failed"),
            (SystemProxyError::InvalidSettings, "system_proxy_invalid"),
            (
                SystemProxyError::AutomaticProxyUnsupported,
                "system_proxy_automatic_unsupported",
            ),
        ] {
            let mapped = map_system_proxy_error(error);
            assert_eq!(mapped.code, code);
            assert!(mapped.retryable);
            assert_eq!(mapped.field, None);
            assert!(mapped.message.chars().count() < 160);
            assert!(mapped.message.contains("自定义代理"));
            assert!(!mapped.message.contains("://"));
        }
        assert_eq!(
            route_models_error_category(UpstreamModelsErrorKind::SystemProxy(
                SystemProxyError::ReadFailed,
            )),
            RouteModelsErrorCategory::SystemProxy,
        );
        let balance = map_balance_error(&router_core::balance::BalanceError {
            stage: router_core::balance::BalanceErrorStage::Http,
            category: router_core::balance::BalanceErrorCategory::SystemProxy,
            transient: false,
        });
        assert_eq!(balance.code, "system_proxy_unavailable");
        assert!(balance.message.contains("自定义代理"));
        assert!(!balance.message.contains("://"));
    }
}
