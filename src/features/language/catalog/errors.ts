import type { AppLocale } from "../../../generated";

/**
 * Every known stable IPC/application error code. Codes and their
 * field/retryability semantics are owned by Rust; display text is owned by
 * this catalog. Unknown codes render `genericFailure`, never a raw upstream
 * or diagnostic message.
 */
export type IpcErrorCode =
  | "active_route_missing"
  | "active_route_must_be_visible"
  | "api_key_control_character"
  | "api_key_required"
  | "api_key_too_long"
  | "appearance_preference_invalid"
  | "automatic_balance_refresh_out_of_range"
  | "balance_query_failed"
  | "balance_script_required"
  | "balance_script_risk_confirmation_required"
  | "balance_script_too_large"
  | "balance_unavailable"
  | "base_url_duplicate_chat_completions"
  | "base_url_duplicate_responses"
  | "base_url_invalid"
  | "base_url_too_long"
  | "base_url_unsupported_endpoint"
  | "chat_bridge_unsupported_request"
  | "codex_auth_browser_missing"
  | "codex_auth_browser_unsupported"
  | "codex_auth_cookie_store_unreadable"
  | "codex_auth_keychain_denied"
  | "codex_auth_profile_not_logged_in"
  | "codex_auth_restore_failed"
  | "codex_auth_session_fetch_failed"
  | "codex_auth_session_invalid"
  | "codex_auth_store_mode_unsupported"
  | "codex_auth_target_conflict"
  | "codex_auth_unsupported_encryption"
  | "codex_auth_write_failed"
  | "codex_baseline_missing"
  | "codex_catalog_publication_failed"
  | "codex_config_changed"
  | "codex_config_invalid"
  | "codex_config_open_failed"
  | "codex_config_open_unsupported"
  | "codex_config_operation_failed"
  | "codex_config_symlink_unsupported"
  | "codex_config_unreadable"
  | "codex_images_mcp_name_conflict"
  | "codex_images_mcp_repair_permit_invalid"
  | "codex_images_mcp_repair_not_available"
  | "codex_images_mcp_repair_permit_stale"
  | "codex_model_context_window_invalid"
  | "codex_model_display_name_control_character"
  | "codex_model_id_control_character"
  | "codex_model_id_duplicate"
  | "codex_model_id_required"
  | "codex_recovery_not_disconnected"
  | "codex_recovery_preview_stale"
  | "codex_recovery_reset_partial"
  | "codex_recovery_unavailable"
  | "database_directory_in_use"
  | "database_operation_failed"
  | "database_path_unsafe"
  | "database_permission_denied"
  | "database_recovery_not_required"
  | "database_recovery_unavailable"
  | "database_retry_failed"
  | "database_retry_not_available"
  | "database_space_unavailable"
  | "database_start_over_failed"
  | "database_start_over_not_allowed"
  | "database_unavailable"
  | "database_version_too_new"
  | "fallback_excluded_model_control_character"
  | "fallback_excluded_model_duplicate"
  | "fallback_excluded_model_required"
  | "fallback_participant_count_invalid"
  | "gateway_token_unavailable"
  | "images_generation_model_control_character"
  | "images_generation_model_required"
  | "images_generation_model_too_long"
  | "images_generation_route_invalid"
  | "images_generation_timeout_out_of_range"
  | "language_preference_invalid"
  | "mcp_image_assets_busy"
  | "mcp_image_assets_clear_failed"
  | "mcp_image_assets_unavailable"
  | "mcp_image_capacity_warning_out_of_range"
  | "mcp_image_directory_open_failed"
  | "mcp_image_directory_open_unsupported"
  | "menu_balance_debounce_out_of_range"
  | "menu_position_failed"
  | "menu_resize_failed"
  | "menu_show_failed"
  | "menu_window_unavailable"
  | "models_unavailable"
  | "no_active_route_confirmation_required"
  | "outbound_proxy_invalid"
  | "outbound_proxy_unreachable"
  | "outbound_proxy_url_invalid"
  | "outbound_proxy_url_required"
  | "outbound_proxy_url_too_long"
  | "pricing_source_open_failed"
  | "project_repository_open_failed"
  | "proxy_port_invalid"
  | "proxy_port_save_failed"
  | "proxy_port_unavailable"
  | "reachability_unavailable"
  | "recovery_inventory_unavailable"
  | "recovery_point_invalid"
  | "recovery_point_stale"
  | "recovery_publish_failed"
  | "recovery_restore_failed"
  | "recovery_unavailable"
  | "route_activation_permit_invalid"
  | "route_activation_permit_stale"
  | "route_key_invalid"
  | "route_name_control_character"
  | "route_name_required"
  | "route_name_too_long"
  | "route_not_found"
  | "route_order_invalid"
  | "routing_configuration_stale"
  | "runtime_log_clear_failed"
  | "runtime_log_open_failed"
  | "runtime_log_open_unsupported"
  | "settings_section_invalid"
  | "settings_target_invalid"
  | "settings_window_unavailable"
  | "system_proxy_automatic_unsupported"
  | "system_proxy_invalid"
  | "system_proxy_read_failed"
  | "system_proxy_unavailable"
  | "update_busy"
  | "update_install_failed"
  | "update_metadata_invalid"
  | "update_not_available"
  | "update_offline"
  | "update_permission_denied"
  | "update_release_open_failed"
  | "update_restart_not_ready"
  | "update_signature_invalid"
  | "usage_query_invalid"
  | "usage_request_not_found";

export interface ErrorMessages {
  genericFailure: string;
  codes: Record<IpcErrorCode, string>;
}

export const errorMessages: Record<AppLocale, ErrorMessages> = {
  "zh-CN": {
    genericFailure: "操作失败，请重试。",
    codes: {
      active_route_missing: "当前没有活动路由。",
      active_route_must_be_visible: "当前激活路由必须显示在顶部菜单中。",
      api_key_control_character: "API Key 不能包含控制字符。",
      api_key_required: "请输入 API Key。",
      api_key_too_long: "API Key 过长。",
      appearance_preference_invalid: "外观偏好无效。",
      automatic_balance_refresh_out_of_range: "自动刷新间隔需为 5 至 1440 分钟。",
      balance_query_failed: "余额查询失败。",
      balance_script_required: "使用自定义余额来源时必须填写脚本。",
      balance_script_risk_confirmation_required: "启用余额脚本前需要确认风险。",
      balance_script_too_large: "余额脚本过大。",
      balance_unavailable: "余额服务尚未就绪。",
      base_url_duplicate_chat_completions:
        "Chat Completions 地址不能重复包含 /chat/completions。",
      base_url_duplicate_responses: "Responses 地址不能重复包含 /responses。",
      base_url_invalid: "请输入有效的 HTTP(S) 地址。",
      base_url_too_long: "地址过长。",
      base_url_unsupported_endpoint: "地址必须匹配所选的上游协议。",
      chat_bridge_unsupported_request:
        "该请求包含 Chat Completions 上游无法表达的内容，请改用 Responses 上游。",
      codex_auth_browser_missing:
        "未检测到 Chrome 系浏览器。请安装或打开 Google Chrome 后重试。",
      codex_auth_browser_unsupported:
        "检测到浏览器，但当前版本仅支持已验证的 Google Chrome。未做任何修改。",
      codex_auth_cookie_store_unreadable:
        "无法读取浏览器 Cookie 数据库（权限不足）。",
      codex_auth_keychain_denied:
        "钥匙串授权被拒绝。请重试并在系统弹窗中选择「始终允许」。",
      codex_auth_profile_not_logged_in:
        "未检测到已登录的 Chrome 会话。请先在 Chrome 中登录 chatgpt.com，然后重试。",
      codex_auth_restore_failed: "还原失败，原文件保持不变。",
      codex_auth_session_fetch_failed:
        "获取 ChatGPT 会话失败（网络异常或被拦截）。未做任何修改。",
      codex_auth_session_invalid: "会话响应缺少必需字段，无法生成凭证。",
      codex_auth_store_mode_unsupported:
        "当前 Codex 未使用 file 模式存储凭证，无法通过 auth.json 替换。请改为 file 后重试。",
      codex_auth_target_conflict:
        "目标文件状态异常（符号链接或在写入前被修改），已停止且未覆盖。",
      codex_auth_unsupported_encryption:
        "浏览器 Cookie 加密格式暂不支持（可能是浏览器新版本变更）。未做任何修改。",
      codex_auth_write_failed: "写入失败，原文件保持不变。",
      codex_baseline_missing: "尚未创建初始配置。",
      codex_catalog_publication_failed: "自定义模型目录写入失败。",
      codex_config_changed: "Codex 配置在操作期间发生变化，请重试。",
      codex_config_invalid: "Codex 配置无效。",
      codex_config_open_failed: "Codex 配置无法打开。",
      codex_config_open_unsupported: "当前平台不支持打开 Codex 配置。",
      codex_config_operation_failed: "Codex 配置操作失败。",
      codex_config_symlink_unsupported: "不支持符号链接形式的 Codex 配置。",
      codex_config_unreadable: "Codex 配置无法读取。",
      codex_images_mcp_name_conflict:
        "Codex 配置中的 ai_router_images 名称已被占用。",
      codex_images_mcp_repair_permit_invalid: "修复确认已失效，请重新确认。",
      codex_images_mcp_repair_not_available: "当前图片工具配置不支持修复。",
      codex_images_mcp_repair_permit_stale:
        "Codex 配置或修复条件已变化，请重新确认。",
      codex_model_context_window_invalid: "上下文窗口必须是正整数。",
      codex_model_display_name_control_character: "显示名称不能包含控制字符。",
      codex_model_id_control_character: "模型 ID 不能包含控制字符。",
      codex_model_id_duplicate: "模型 ID 不能重复。",
      codex_model_id_required: "请输入模型 ID。",
      codex_recovery_not_disconnected: "请先断开 Codex 后再执行此操作。",
      codex_recovery_preview_stale: "恢复配置预览已失效，请重新确认。",
      codex_recovery_reset_partial: "首次连接前状态仅部分恢复，请刷新后重试。",
      codex_recovery_unavailable: "断开恢复配置暂不可用。",
      database_directory_in_use: "另一个 AI Router 进程正在使用该数据目录。",
      database_operation_failed: "数据库操作失败。",
      database_path_unsafe: "数据库或恢复目录不是安全的常规路径。",
      database_permission_denied: "数据库或恢复目录无法访问。",
      database_recovery_not_required: "数据库当前不需要恢复。",
      database_recovery_unavailable: "当前数据库状态不支持此操作。",
      database_retry_failed: "数据库启动重试失败。",
      database_retry_not_available: "当前数据库状态不支持重试。",
      database_space_unavailable: "磁盘空间不足，无法完成数据库操作。",
      database_start_over_failed: "无法创建空数据库。",
      database_start_over_not_allowed: "仍有可用恢复点，不能创建空数据库。",
      database_unavailable: "数据库尚未就绪。",
      database_version_too_new: "数据库由更高版本的 AI Router 创建。",
      fallback_excluded_model_control_character: "模型 ID 不能包含控制字符。",
      fallback_excluded_model_duplicate: "模型 ID 不能重复。",
      fallback_excluded_model_required: "请输入模型 ID。",
      fallback_participant_count_invalid: "Fallback 参与数量无效。",
      gateway_token_unavailable: "本地网关令牌不可用。",
      images_generation_model_control_character: "生图模型不能包含控制字符。",
      images_generation_model_required: "请输入生图模型。",
      images_generation_model_too_long: "生图模型过长。",
      images_generation_route_invalid: "请选择已存在的图片路由。",
      images_generation_timeout_out_of_range: "生成等待上限需为 600 至 3600 秒。",
      language_preference_invalid: "语言偏好无效。",
      mcp_image_assets_busy: "图片正在生成，请稍后重试。",
      mcp_image_assets_clear_failed: "部分图片无法清除，请刷新后重试。",
      mcp_image_assets_unavailable: "图片目录暂时无法读取。",
      mcp_image_capacity_warning_out_of_range:
        "图片容量告警阈值需为 128 至 102400 MiB。",
      mcp_image_directory_open_failed: "图片目录无法打开。",
      mcp_image_directory_open_unsupported: "当前平台不支持打开图片目录。",
      menu_balance_debounce_out_of_range: "菜单余额刷新防抖需为 10 至 600 秒。",
      menu_position_failed: "菜单定位失败。",
      menu_resize_failed: "菜单尺寸调整失败。",
      menu_show_failed: "菜单无法显示。",
      menu_window_unavailable: "菜单窗口不可用。",
      models_unavailable: "模型列表服务尚未就绪。",
      no_active_route_confirmation_required: "当前没有活动路由。",
      outbound_proxy_invalid: "代理地址无效。",
      outbound_proxy_unreachable: "无法连接到代理服务器。",
      outbound_proxy_url_invalid: "请输入有效的代理地址。",
      outbound_proxy_url_required: "请输入代理地址。",
      outbound_proxy_url_too_long: "代理地址过长。",
      pricing_source_open_failed: "OpenAI 定价页无法打开。",
      project_repository_open_failed: "GitHub 项目无法打开。",
      proxy_port_invalid: "端口必须在 1 到 65535 之间。",
      proxy_port_save_failed: "端口保存失败。",
      proxy_port_unavailable: "该端口已被占用。",
      reachability_unavailable: "地址检查暂不可用。",
      recovery_inventory_unavailable: "无法读取恢复点。",
      recovery_point_invalid: "恢复点未通过安全校验。",
      recovery_point_stale: "所选恢复点已失效，请刷新后重试。",
      recovery_publish_failed: "无法创建恢复点。",
      recovery_restore_failed: "数据库恢复失败。",
      recovery_unavailable: "恢复服务尚未就绪。",
      route_activation_permit_invalid: "切换条件已变化，请重新确认。",
      route_activation_permit_stale: "切换条件已变化，请重新确认。",
      route_key_invalid: "路由 Key 无法读取。",
      route_name_control_character: "名称不能包含控制字符。",
      route_name_required: "请输入名称。",
      route_name_too_long: "名称过长。",
      route_not_found: "路由不存在。",
      route_order_invalid: "路由顺序无效。",
      routing_configuration_stale: "路由配置已更新，请重试。",
      runtime_log_clear_failed: "运行日志清除失败。",
      runtime_log_open_failed: "日志目录打开失败。",
      runtime_log_open_unsupported: "当前平台不支持打开日志目录。",
      settings_section_invalid: "设置分区无效。",
      settings_target_invalid: "设置定位目标无效。",
      settings_window_unavailable: "设置窗口不可用。",
      system_proxy_automatic_unsupported:
        "此目标仅有 PAC/WPAD 自动代理规则，应用不执行这些规则。请配置系统手动代理或切换为自定义代理。",
      system_proxy_invalid:
        "macOS 系统代理设置无效。请检查系统设置或切换为自定义代理。",
      system_proxy_read_failed:
        "无法读取 macOS 系统代理设置。请检查系统设置或切换为自定义代理。",
      system_proxy_unavailable:
        "系统代理无法用于此目标。请检查系统手动代理设置或切换为自定义代理；应用不执行 PAC/WPAD 规则。",
      update_busy: "另一项更新操作正在进行。",
      update_install_failed: "更新未能安装，当前应用保持不变。",
      update_metadata_invalid: "更新信息未通过安全校验。",
      update_not_available: "当前没有可下载的应用更新。",
      update_offline: "更新包下载中断，请稍后重试。",
      update_permission_denied:
        "没有权限安装更新，请使用 GitHub Release 手动更新。",
      update_release_open_failed: "GitHub Release 无法打开。",
      update_restart_not_ready: "应用更新尚未准备好重启。",
      update_signature_invalid: "更新包签名校验失败，未安装任何内容。",
      usage_query_invalid: "用量筛选条件无效。",
      usage_request_not_found: "请求记录不存在。",
    },
  },
  en: {
    genericFailure: "Something went wrong. Try again.",
    codes: {
      active_route_missing: "No active route is available.",
      active_route_must_be_visible:
        "The active route must stay visible in the top menu.",
      api_key_control_character: "The API key must not contain control characters.",
      api_key_required: "Enter an API key.",
      api_key_too_long: "The API key is too long.",
      appearance_preference_invalid: "The appearance preference is invalid.",
      automatic_balance_refresh_out_of_range:
        "The automatic refresh interval must be between 5 and 1440 minutes.",
      balance_query_failed: "The balance query failed.",
      balance_script_required:
        "A script is required when using a custom balance source.",
      balance_script_risk_confirmation_required:
        "Confirm the risk before enabling the balance script.",
      balance_script_too_large: "The balance script is too large.",
      balance_unavailable: "The balance service is not ready yet.",
      base_url_duplicate_chat_completions:
        "The Chat Completions URL must not repeat /chat/completions.",
      base_url_duplicate_responses:
        "The Responses URL must not repeat /responses.",
      base_url_invalid: "Enter a valid HTTP(S) URL.",
      base_url_too_long: "The URL is too long.",
      base_url_unsupported_endpoint:
        "The URL must match the selected upstream protocol.",
      chat_bridge_unsupported_request:
        "This request contains content a Chat Completions upstream cannot express; use a Responses upstream instead.",
      codex_auth_browser_missing:
        "No Chrome-family browser was found. Install or open Google Chrome and try again.",
      codex_auth_browser_unsupported:
        "A browser was found, but this version only supports verified Google Chrome. Nothing was modified.",
      codex_auth_cookie_store_unreadable:
        "The browser cookie database could not be read (insufficient permission).",
      codex_auth_keychain_denied:
        "Keychain access was denied. Try again and choose “Always Allow” in the system prompt.",
      codex_auth_profile_not_logged_in:
        "No signed-in Chrome session was found. Sign in to chatgpt.com in Chrome, then try again.",
      codex_auth_restore_failed:
        "The restore failed; the original file is unchanged.",
      codex_auth_session_fetch_failed:
        "Fetching the ChatGPT session failed (network error or interception). Nothing was modified.",
      codex_auth_session_invalid:
        "The session response is missing required fields; credentials cannot be generated.",
      codex_auth_store_mode_unsupported:
        "Codex is not storing credentials in file mode, so auth.json replacement is unavailable. Switch to file mode and try again.",
      codex_auth_target_conflict:
        "The target file is in an unexpected state (a symlink or modified before writing). The operation stopped without overwriting.",
      codex_auth_unsupported_encryption:
        "The browser cookie encryption format is not supported yet (possibly a browser change). Nothing was modified.",
      codex_auth_write_failed:
        "The write failed; the original file is unchanged.",
      codex_baseline_missing: "The initial configuration has not been created yet.",
      codex_catalog_publication_failed:
        "Writing the custom model catalog failed.",
      codex_config_changed:
        "The Codex configuration changed during the operation. Try again.",
      codex_config_invalid: "The Codex configuration is invalid.",
      codex_config_open_failed: "The Codex configuration could not be opened.",
      codex_config_open_unsupported:
        "This platform cannot open the Codex configuration.",
      codex_config_operation_failed: "The Codex configuration operation failed.",
      codex_config_symlink_unsupported:
        "A symlinked Codex configuration is not supported.",
      codex_config_unreadable: "The Codex configuration could not be read.",
      codex_images_mcp_name_conflict:
        "The ai_router_images name in the Codex configuration is already taken.",
      codex_images_mcp_repair_permit_invalid:
        "The repair confirmation is no longer valid. Confirm again.",
      codex_images_mcp_repair_not_available:
        "The current image-tools configuration does not support repair.",
      codex_images_mcp_repair_permit_stale:
        "The Codex configuration or repair conditions changed. Confirm again.",
      codex_model_context_window_invalid:
        "The context window must be a positive integer.",
      codex_model_display_name_control_character:
        "The display name must not contain control characters.",
      codex_model_id_control_character:
        "The model ID must not contain control characters.",
      codex_model_id_duplicate: "The model ID must be unique.",
      codex_model_id_required: "Enter a model ID.",
      codex_recovery_not_disconnected:
        "Disconnect Codex before performing this operation.",
      codex_recovery_preview_stale:
        "The recovery preview expired. Confirm again.",
      codex_recovery_reset_partial:
        "The pre-first-connection state was only partially restored. Refresh and try again.",
      codex_recovery_unavailable:
        "Disconnect recovery configuration is not available right now.",
      database_directory_in_use:
        "Another AI Router process is using this data directory.",
      database_operation_failed: "The database operation failed.",
      database_path_unsafe:
        "The database or recovery directory is not a safe regular path.",
      database_permission_denied:
        "The database or recovery directory is not accessible.",
      database_recovery_not_required:
        "The database does not need recovery right now.",
      database_recovery_unavailable:
        "The current database state does not support this operation.",
      database_retry_failed: "The database startup retry failed.",
      database_retry_not_available:
        "The current database state does not support retrying.",
      database_space_unavailable:
        "Not enough disk space to complete the database operation.",
      database_start_over_failed: "An empty database could not be created.",
      database_start_over_not_allowed:
        "Valid recovery points still exist; an empty database cannot be created.",
      database_unavailable: "The database is not ready yet.",
      database_version_too_new:
        "The database was created by a newer version of AI Router.",
      fallback_excluded_model_control_character:
        "The model ID must not contain control characters.",
      fallback_excluded_model_duplicate: "The model ID must be unique.",
      fallback_excluded_model_required: "Enter a model ID.",
      fallback_participant_count_invalid:
        "The Fallback participant count is invalid.",
      gateway_token_unavailable: "The local gateway token is unavailable.",
      images_generation_model_control_character:
        "The image model must not contain control characters.",
      images_generation_model_required: "Enter an image model.",
      images_generation_model_too_long: "The image model name is too long.",
      images_generation_route_invalid: "Choose an existing image route.",
      images_generation_timeout_out_of_range:
        "The generation wait limit must be between 600 and 3600 seconds.",
      language_preference_invalid: "The language preference is invalid.",
      mcp_image_assets_busy: "Images are being generated; try again shortly.",
      mcp_image_assets_clear_failed:
        "Some images could not be removed. Refresh and try again.",
      mcp_image_assets_unavailable:
        "The image directory is temporarily unavailable.",
      mcp_image_capacity_warning_out_of_range:
        "The image capacity warning threshold must be between 128 and 102400 MiB.",
      mcp_image_directory_open_failed:
        "The image directory could not be opened.",
      mcp_image_directory_open_unsupported:
        "This platform cannot open the image directory.",
      menu_balance_debounce_out_of_range:
        "The menu balance debounce must be between 10 and 600 seconds.",
      menu_position_failed: "Positioning the menu failed.",
      menu_resize_failed: "Resizing the menu failed.",
      menu_show_failed: "The menu could not be shown.",
      menu_window_unavailable: "The menu window is unavailable.",
      models_unavailable: "The model list service is not ready yet.",
      no_active_route_confirmation_required: "There is no active route.",
      outbound_proxy_invalid: "The proxy address is invalid.",
      outbound_proxy_unreachable: "The proxy server could not be reached.",
      outbound_proxy_url_invalid: "Enter a valid proxy address.",
      outbound_proxy_url_required: "Enter a proxy address.",
      outbound_proxy_url_too_long: "The proxy address is too long.",
      pricing_source_open_failed: "The OpenAI pricing page could not be opened.",
      project_repository_open_failed:
        "The GitHub repository could not be opened.",
      proxy_port_invalid: "The port must be between 1 and 65535.",
      proxy_port_save_failed: "Saving the port failed.",
      proxy_port_unavailable: "That port is already in use.",
      reachability_unavailable: "The address check is temporarily unavailable.",
      recovery_inventory_unavailable: "Recovery points could not be read.",
      recovery_point_invalid:
        "The recovery point did not pass safety validation.",
      recovery_point_stale: "The selected recovery point expired. Refresh and try again.",
      recovery_publish_failed: "The recovery point could not be created.",
      recovery_restore_failed: "Database recovery failed.",
      recovery_unavailable: "The recovery service is not ready yet.",
      route_activation_permit_invalid:
        "The switch conditions changed. Confirm again.",
      route_activation_permit_stale:
        "The switch conditions changed. Confirm again.",
      route_key_invalid: "The route key could not be read.",
      route_name_control_character: "The name must not contain control characters.",
      route_name_required: "Enter a name.",
      route_name_too_long: "The name is too long.",
      route_not_found: "The route does not exist.",
      route_order_invalid: "The route order is invalid.",
      routing_configuration_stale:
        "The routing configuration was updated. Try again.",
      runtime_log_clear_failed: "Clearing the runtime logs failed.",
      runtime_log_open_failed: "Opening the log directory failed.",
      runtime_log_open_unsupported:
        "This platform cannot open the log directory.",
      settings_section_invalid: "The settings section is invalid.",
      settings_target_invalid: "The settings target is invalid.",
      settings_window_unavailable: "The settings window is unavailable.",
      system_proxy_automatic_unsupported:
        "Only PAC/WPAD automatic proxy rules exist for this target, and the app does not run them. Configure a manual system proxy or switch to a custom proxy.",
      system_proxy_invalid:
        "The macOS system proxy settings are invalid. Check System Settings or switch to a custom proxy.",
      system_proxy_read_failed:
        "The macOS system proxy settings could not be read. Check System Settings or switch to a custom proxy.",
      system_proxy_unavailable:
        "The system proxy cannot serve this target. Check the manual system proxy settings or switch to a custom proxy; the app does not run PAC/WPAD rules.",
      update_busy: "Another update operation is already in progress.",
      update_install_failed:
        "The update could not be installed. The current app is unchanged.",
      update_metadata_invalid:
        "The update metadata did not pass safety validation.",
      update_not_available: "No downloadable application update is available.",
      update_offline: "The update download was interrupted. Try again shortly.",
      update_permission_denied:
        "No permission to install the update. Update manually from the GitHub Release.",
      update_release_open_failed: "The GitHub Release could not be opened.",
      update_restart_not_ready: "The update is not ready to restart yet.",
      update_signature_invalid:
        "Update signature verification failed. Nothing was installed.",
      usage_query_invalid: "The usage filter is invalid.",
      usage_request_not_found: "The request record does not exist.",
    },
  },
};
