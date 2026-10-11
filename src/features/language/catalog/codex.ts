import type { AppLocale } from "../../../generated";

/** Presentation tone for an auth failure, a subset of the shared status tones. */
export type CodexAuthErrorTone = "warning" | "danger";

/** Which action a failure offers, if any. */
export type CodexAuthRetryKind = "primary" | "secondary" | "none";

/** Per-code presentation for the Codex credential section. */
export interface CodexAuthErrorCopy {
  tone: CodexAuthErrorTone;
  label: string;
  message: string;
  retryKind: CodexAuthRetryKind;
}

/**
 * Codex connection, auth and recovery surfaces. Interpolation and plural
 * handling stay inside typed functions; display text never carries raw
 * diagnostics or user input.
 */
export interface CodexMessages {
  statusChecking: string;
  statusConnected: string;
  statusNotConnected: string;
  statusChanged: string;
  statusImagesMcpNameConflict: string;
  statusImagesMcpProjectionConflict: string;
  statusInvalid: string;
  statusUnreadable: string;
  statusSymlinkUnsupported: string;

  proxySectionTitle: string;
  proxyStatusRunning: string;
  proxyStatusPortConflict: string;
  proxyStatusUnavailable: string;
  proxyAddressLabel: string;
  proxyPortLabel: string;
  proxyPortApply: string;

  configSectionTitle: string;
  configFileLabel: string;
  imagesRepairAction: string;
  connectAction: string;
  reconnectAction: string;
  openConfigAction: string;
  imagesProjectionConflictMessage: string;
  imagesNameConflictMessage: string;

  connectNoRouteTitle: string;
  connectNoRouteBody: string;
  connectNoRouteConfirm: string;

  imagesRepairConfirmTitle: string;
  imagesRepairConfirmBody: string;
  imagesRepairConfirmLabel: string;

  recoverySectionTitle: string;
  recoveryStatusUnavailable: string;
  recoveryStatusAvailable: string;
  recoveryNotCreated: string;
  recoveryTargetLabel: string;
  recoveryTargetExists: string;
  recoveryTargetDelete: string;
  recoveryUpdatedAtLabel: string;
  recoveryUpdateAction: string;
  recoveryUpdateConfirmTitle: string;
  recoveryUpdateConfirmBody: string;
  recoveryUpdateConfirmLabel: string;
  recoveryResetAction: string;
  recoveryResetConfirmTitle: string;
  recoveryResetConfirmBody: string;
  recoveryResetConfirmLabel: string;
  recoveryHintNoBackup: string;
  recoveryHintNotDisconnected: string;
  recoveryHintReady: string;
  originalBackupTitle: string;
  originalBackupSummary: (capturedAt: string) => string;
  originalBackupMissing: string;

  detailCurrentFile: string;
  detailOriginalFile: string;
  detailRecoveryTarget: string;
  detailComparedToRecovery: string;
  detailFileMode: string;
  detailExistingRecoveryTarget: string;
  detailRecoveryUpdatedAt: string;
  valueExists: string;
  valueNotExists: string;
  valueChanged: string;
  valueUnchanged: string;

  authSectionTitle: string;
  authHelpLabel: string;
  authHelpStrong: string;
  authHelpBody: string;
  authHelpNote: string;
  authIdleHint: string;
  authExportAction: string;
  authKeychainHint: string;
  authOpenConfigAction: string;
  authHeadingBusy: string;
  authHeadingOperationFailed: string;
  authHeadingDrifted: string;
  authHeadingExpired: string;
  authHeadingSuccess: string;
  authHeadingIdle: string;
  authStoreModeUnsupportedLabel: string;
  authKeychainDeniedLabel: string;
  authAccountLabel: string;
  authPlanLabel: string;
  authExpiryLabel: string;
  authExportedAtLabel: string;
  authExpiredValue: (formatted: string) => string;
  authKeychainDeniedMessage: string;
  authDriftedMessage: string;
  authRestartHint: string;
  authReexportAction: string;
  authRestoreAction: string;
  authRestoreConfirmTitle: string;
  authRestoreConfirmBody: string;
  authRestoreConfirmLabel: string;
  authRetryAction: string;
  authBusyAction: string;

  storeModeKeyring: string;
  storeModeAuto: string;
  storeModeEphemeral: string;
  authStoreModeMessageManaged: (mode: string) => string;
  authStoreModeMessageDefault: (mode: string) => string;

  authErrorFallbackLabel: string;
  authErrors: Record<string, CodexAuthErrorCopy>;
}

export const codexMessages: Record<AppLocale, CodexMessages> = {
  "zh-CN": {
    statusChecking: "检查中",
    statusConnected: "已连接",
    statusNotConnected: "未连接",
    statusChanged: "待重新连接",
    statusImagesMcpNameConflict: "图片 MCP 名称冲突",
    statusImagesMcpProjectionConflict: "图片 MCP 配置已修改",
    statusInvalid: "配置无效",
    statusUnreadable: "配置不可读",
    statusSymlinkUnsupported: "不支持符号链接",

    proxySectionTitle: "本地代理",
    proxyStatusRunning: "运行中",
    proxyStatusPortConflict: "端口冲突",
    proxyStatusUnavailable: "不可用",
    proxyAddressLabel: "地址",
    proxyPortLabel: "端口",
    proxyPortApply: "应用端口",

    configSectionTitle: "Codex 配置",
    configFileLabel: "配置文件",
    imagesRepairAction: "修复图片配置",
    connectAction: "一键连接 Codex",
    reconnectAction: "重新连接 Codex",
    openConfigAction: "打开 config.toml",
    imagesProjectionConflictMessage:
      "图片工具配置已被修改，自动重连无法继续。",
    imagesNameConflictMessage:
      "首次连接前已存在同名配置，请先重命名或移除。",

    connectNoRouteTitle: "当前没有活动路由",
    connectNoRouteBody: "连接后，新请求会失败，直到你添加或选择路由。",
    connectNoRouteConfirm: "继续连接 Codex",

    imagesRepairConfirmTitle: "替换 ai_router_images 配置？",
    imagesRepairConfirmBody:
      "只会替换图片工具配置，其他 Codex 配置不会改动。",
    imagesRepairConfirmLabel: "替换并重新连接",

    recoverySectionTitle: "断开恢复配置",
    recoveryStatusUnavailable: "不可用",
    recoveryStatusAvailable: "可用",
    recoveryNotCreated: "尚未创建",
    recoveryTargetLabel: "恢复目标",
    recoveryTargetExists: "config.toml 存在",
    recoveryTargetDelete: "断开后删除 config.toml",
    recoveryUpdatedAtLabel: "更新时间",
    recoveryUpdateAction: "更新恢复配置",
    recoveryUpdateConfirmTitle: "更新断开恢复配置？",
    recoveryUpdateConfirmBody:
      "当前 config.toml 将成为以后断开连接时的恢复目标。更新后 Codex 仍保持断开。",
    recoveryUpdateConfirmLabel: "更新恢复配置",
    recoveryResetAction: "恢复首次连接前状态",
    recoveryResetConfirmTitle: "恢复首次连接前状态？",
    recoveryResetConfirmBody:
      "当前 config.toml 和断开恢复配置都会被原始备份替换，断开后的手动修改将丢失。",
    recoveryResetConfirmLabel: "恢复原始备份",
    recoveryHintNoBackup: "首次连接前没有可用的原始备份。",
    recoveryHintNotDisconnected: "断开 Codex 后才能从当前 config.toml 更新。",
    recoveryHintReady: "断开连接时，将完整恢复这份配置。更新后仍保持断开。",
    originalBackupTitle: "原始备份",
    originalBackupSummary: (capturedAt) =>
      `首次连接前 · ${capturedAt} · 永久保留`,
    originalBackupMissing: "首次连接前没有可用备份",

    detailCurrentFile: "当前文件",
    detailOriginalFile: "原始配置文件",
    detailRecoveryTarget: "当前恢复目标",
    detailComparedToRecovery: "与现有恢复配置相比",
    detailFileMode: "文件权限",
    detailExistingRecoveryTarget: "现有恢复目标",
    detailRecoveryUpdatedAt: "恢复配置更新时间",
    valueExists: "存在",
    valueNotExists: "不存在",
    valueChanged: "内容已更改",
    valueUnchanged: "内容未更改",

    authSectionTitle: "Codex 凭证",
    authHelpLabel: "Codex 凭证说明",
    authHelpStrong: "仅支持 Chrome 系浏览器。",
    authHelpBody:
      "本功能读取本机 Chrome 的登录 Cookie，在本地生成并替换 Codex 凭证文件，不上传任何数据。",
    authHelpNote: "凭证为一次性：过期后需重新导出。",
    authIdleHint: "从 Chrome 读取已登录的 ChatGPT 会话，并在本地生成 Codex 凭证。",
    authExportAction: "获取并替换 Codex 凭证",
    authKeychainHint: "首次使用会请求钥匙串授权，请在系统弹窗中选择「始终允许」。",
    authOpenConfigAction: "打开 Codex 配置",
    authHeadingBusy: "正在读取 Chrome 会话…",
    authHeadingOperationFailed: "操作失败",
    authHeadingDrifted: "检测到外部修改",
    authHeadingExpired: "凭证已过期",
    authHeadingSuccess: "已写入",
    authHeadingIdle: "未检测",
    authStoreModeUnsupportedLabel: "存储模式不支持",
    authKeychainDeniedLabel: "等待钥匙串授权",
    authAccountLabel: "账号",
    authPlanLabel: "套餐",
    authExpiryLabel: "凭证有效期",
    authExportedAtLabel: "上次导出",
    authExpiredValue: (formatted) => `已过期（${formatted}）`,
    authKeychainDeniedMessage:
      "钥匙串授权被拒绝。请重新点击下方按钮，并在系统弹窗中选择「始终允许」。",
    authDriftedMessage:
      "检测到 ~/.codex/auth.json 已被外部修改。重新导出将覆盖当前文件；原文件会先备份。",
    authRestartHint:
      "请重启 Codex 使新凭证生效。凭证为一次性：过期后需重新导出。",
    authReexportAction: "重新导出",
    authRestoreAction: "还原原凭证",
    authRestoreConfirmTitle: "还原原凭证？",
    authRestoreConfirmBody:
      "将用上次导出的备份覆盖当前 ~/.codex/auth.json。当前文件不会被保留。",
    authRestoreConfirmLabel: "还原",
    authRetryAction: "重试",
    authBusyAction: "正在读取…",

    storeModeKeyring: "keyring（系统钥匙串）",
    storeModeAuto: "auto（自动选择，可能不使用 auth.json）",
    storeModeEphemeral: "ephemeral（临时，退出即失效）",
    authStoreModeMessageManaged: (mode) =>
      `无法导出：凭证存储方式由企业配置锁定，本机不能改为文件存储（当前：${mode}）。如需在 Codex 中登录，请使用官方登录方式，或联系管理员。`,
    authStoreModeMessageDefault: (mode) =>
      `无法导出：Codex 当前的凭证存储方式是 ${mode}，不会读取 auth.json。请点下方「打开 Codex 配置」，把 cli_auth_credentials_store 改为 "file"（或删除这一行，默认即 file），保存后回到本窗口即可重新导出。`,

    authErrorFallbackLabel: "操作失败",
    authErrors: {
      codex_auth_browser_missing: {
        tone: "danger",
        label: "未检测到浏览器",
        message:
          "未检测到 Chrome 系浏览器。请安装或打开 Google Chrome 后重试。",
        retryKind: "secondary",
      },
      codex_auth_browser_unsupported: {
        tone: "warning",
        label: "浏览器不支持",
        message:
          "检测到未验证的浏览器，当前版本仅支持已验证的 Google Chrome。未做任何修改。",
        retryKind: "none",
      },
      codex_auth_unsupported_encryption: {
        tone: "warning",
        label: "加密格式不支持",
        message:
          "浏览器 Cookie 加密格式暂不支持（可能是浏览器新版本变更）。未做任何修改。",
        retryKind: "none",
      },
      codex_auth_keychain_denied: {
        tone: "warning",
        label: "等待钥匙串授权",
        message: "钥匙串授权被拒绝。请重试并在系统弹窗中选择「始终允许」。",
        retryKind: "primary",
      },
      codex_auth_cookie_store_unreadable: {
        tone: "danger",
        label: "无法读取 Cookie",
        message: "无法读取浏览器 Cookie 数据库（权限不足）。",
        retryKind: "secondary",
      },
      codex_auth_profile_not_logged_in: {
        tone: "danger",
        label: "未登录",
        message:
          "未检测到已登录的 Chrome 会话。请先在 Chrome 中登录 chatgpt.com，然后重试。",
        retryKind: "secondary",
      },
      codex_auth_session_fetch_failed: {
        tone: "danger",
        label: "会话获取失败",
        message: "获取 ChatGPT 会话失败（网络异常或被拦截）。未做任何修改。",
        retryKind: "secondary",
      },
      codex_auth_session_invalid: {
        tone: "danger",
        label: "会话无效",
        message: "会话响应缺少必需字段，无法生成凭证。",
        retryKind: "secondary",
      },
      codex_auth_store_mode_unsupported: {
        tone: "warning",
        label: "存储模式不支持",
        message:
          'Codex 当前未使用 file 存储凭证；请把 cli_auth_credentials_store 改为 "file"，保存后重试。',
        retryKind: "none",
      },
      codex_auth_target_conflict: {
        tone: "warning",
        label: "目标已占用",
        message:
          "目标文件状态异常（符号链接或在写入前被修改），已停止且未覆盖。",
        retryKind: "secondary",
      },
      codex_auth_write_failed: {
        tone: "danger",
        label: "写入失败",
        message: "写入失败，原文件保持不变。",
        retryKind: "secondary",
      },
      codex_auth_restore_failed: {
        tone: "danger",
        label: "还原失败",
        message: "还原失败，原文件保持不变。",
        retryKind: "secondary",
      },
    },
  },
  en: {
    statusChecking: "Checking",
    statusConnected: "Connected",
    statusNotConnected: "Not connected",
    statusChanged: "Reconnect required",
    statusImagesMcpNameConflict: "Image MCP name conflict",
    statusImagesMcpProjectionConflict: "Image MCP config modified",
    statusInvalid: "Invalid configuration",
    statusUnreadable: "Configuration unreadable",
    statusSymlinkUnsupported: "Symlinks unsupported",

    proxySectionTitle: "Local proxy",
    proxyStatusRunning: "Running",
    proxyStatusPortConflict: "Port conflict",
    proxyStatusUnavailable: "Unavailable",
    proxyAddressLabel: "Address",
    proxyPortLabel: "Port",
    proxyPortApply: "Apply port",

    configSectionTitle: "Codex configuration",
    configFileLabel: "Config file",
    imagesRepairAction: "Repair image config",
    connectAction: "Connect Codex",
    reconnectAction: "Reconnect Codex",
    openConfigAction: "Open config.toml",
    imagesProjectionConflictMessage:
      "The image tool config was modified; automatic reconnection can't continue.",
    imagesNameConflictMessage:
      "A config with this name already existed before the first connection. Rename or remove it first.",

    connectNoRouteTitle: "No active route",
    connectNoRouteBody:
      "After connecting, new requests will fail until you add or select a route.",
    connectNoRouteConfirm: "Continue connecting Codex",

    imagesRepairConfirmTitle: "Replace the ai_router_images config?",
    imagesRepairConfirmBody:
      "Only the image tool config will be replaced; the rest of your Codex config is unchanged.",
    imagesRepairConfirmLabel: "Replace and reconnect",

    recoverySectionTitle: "Disconnect recovery config",
    recoveryStatusUnavailable: "Unavailable",
    recoveryStatusAvailable: "Available",
    recoveryNotCreated: "Not created yet",
    recoveryTargetLabel: "Recovery target",
    recoveryTargetExists: "config.toml present",
    recoveryTargetDelete: "config.toml deleted on disconnect",
    recoveryUpdatedAtLabel: "Updated",
    recoveryUpdateAction: "Update recovery config",
    recoveryUpdateConfirmTitle: "Update disconnect recovery config?",
    recoveryUpdateConfirmBody:
      "The current config.toml becomes the recovery target for future disconnects. Codex stays disconnected after the update.",
    recoveryUpdateConfirmLabel: "Update recovery config",
    recoveryResetAction: "Restore pre-connection state",
    recoveryResetConfirmTitle: "Restore pre-connection state?",
    recoveryResetConfirmBody:
      "The current config.toml and the disconnect recovery config are both replaced by the original backup; manual edits made after disconnecting are lost.",
    recoveryResetConfirmLabel: "Restore original backup",
    recoveryHintNoBackup:
      "No original backup is available from before the first connection.",
    recoveryHintNotDisconnected:
      "Disconnect Codex before updating from the current config.toml.",
    recoveryHintReady:
      "This config is restored in full when you disconnect. It stays disconnected after the update.",
    originalBackupTitle: "Original backup",
    originalBackupSummary: (capturedAt) =>
      `Before first connection · ${capturedAt} · Kept permanently`,
    originalBackupMissing: "No backup available from before the first connection",

    detailCurrentFile: "Current file",
    detailOriginalFile: "Original config file",
    detailRecoveryTarget: "Current recovery target",
    detailComparedToRecovery: "Compared with the current recovery config",
    detailFileMode: "File mode",
    detailExistingRecoveryTarget: "Existing recovery target",
    detailRecoveryUpdatedAt: "Recovery config updated",
    valueExists: "Present",
    valueNotExists: "Absent",
    valueChanged: "Contents changed",
    valueUnchanged: "Contents unchanged",

    authSectionTitle: "Codex credentials",
    authHelpLabel: "About Codex credentials",
    authHelpStrong: "Chrome-based browsers only.",
    authHelpBody:
      "This reads the signed-in cookies from your local Chrome, generates and replaces the Codex credential file locally, and uploads nothing.",
    authHelpNote: "Credentials are single-use: re-export after they expire.",
    authIdleHint:
      "Read the signed-in ChatGPT session from Chrome and generate Codex credentials locally.",
    authExportAction: "Fetch and replace Codex credentials",
    authKeychainHint:
      "The first use requests keychain access; choose “Always Allow” in the system dialog.",
    authOpenConfigAction: "Open Codex config",
    authHeadingBusy: "Reading Chrome session…",
    authHeadingOperationFailed: "Operation failed",
    authHeadingDrifted: "External change detected",
    authHeadingExpired: "Credential expired",
    authHeadingSuccess: "Written",
    authHeadingIdle: "Not detected",
    authStoreModeUnsupportedLabel: "Store mode unsupported",
    authKeychainDeniedLabel: "Waiting for keychain access",
    authAccountLabel: "Account",
    authPlanLabel: "Plan",
    authExpiryLabel: "Credential expiry",
    authExportedAtLabel: "Last export",
    authExpiredValue: (formatted) => `Expired (${formatted})`,
    authKeychainDeniedMessage:
      "Keychain access was denied. Click the button below again and choose “Always Allow” in the system dialog.",
    authDriftedMessage:
      "~/.codex/auth.json was modified externally. Re-exporting overwrites the current file; the original is backed up first.",
    authRestartHint:
      "Restart Codex for the new credentials to take effect. Credentials are single-use: re-export after they expire.",
    authReexportAction: "Re-export",
    authRestoreAction: "Restore original credentials",
    authRestoreConfirmTitle: "Restore original credentials?",
    authRestoreConfirmBody:
      "Overwrites the current ~/.codex/auth.json with the backup from the last export. The current file is not kept.",
    authRestoreConfirmLabel: "Restore",
    authRetryAction: "Retry",
    authBusyAction: "Reading…",

    storeModeKeyring: "keyring (system keychain)",
    storeModeAuto: "auto (automatic; may not use auth.json)",
    storeModeEphemeral: "ephemeral (temporary; cleared on exit)",
    authStoreModeMessageManaged: (mode) =>
      `Cannot export: the credential store is locked by enterprise policy, so this machine can't switch to file storage (current: ${mode}). To sign in with Codex, use the official sign-in flow or contact your administrator.`,
    authStoreModeMessageDefault: (mode) =>
      `Cannot export: Codex currently stores credentials as ${mode} and won't read auth.json. Click "Open Codex config" below, set cli_auth_credentials_store to "file" (or delete that line — file is the default), save, and return to this window to export again.`,

    authErrorFallbackLabel: "Operation failed",
    authErrors: {
      codex_auth_browser_missing: {
        tone: "danger",
        label: "No browser detected",
        message:
          "No Chrome-based browser detected. Install or open Google Chrome and try again.",
        retryKind: "secondary",
      },
      codex_auth_browser_unsupported: {
        tone: "warning",
        label: "Browser unsupported",
        message:
          "An unverified browser was detected; this version only supports verified Google Chrome. Nothing was modified.",
        retryKind: "none",
      },
      codex_auth_unsupported_encryption: {
        tone: "warning",
        label: "Encryption unsupported",
        message:
          "The browser's cookie encryption format isn't supported yet (possibly changed in a new browser version). Nothing was modified.",
        retryKind: "none",
      },
      codex_auth_keychain_denied: {
        tone: "warning",
        label: "Waiting for keychain access",
        message:
          "Keychain access was denied. Try again and choose “Always Allow” in the system dialog.",
        retryKind: "primary",
      },
      codex_auth_cookie_store_unreadable: {
        tone: "danger",
        label: "Cannot read cookies",
        message:
          "Cannot read the browser cookie database (insufficient permissions).",
        retryKind: "secondary",
      },
      codex_auth_profile_not_logged_in: {
        tone: "danger",
        label: "Not signed in",
        message:
          "No signed-in Chrome session detected. Sign in to chatgpt.com in Chrome first, then try again.",
        retryKind: "secondary",
      },
      codex_auth_session_fetch_failed: {
        tone: "danger",
        label: "Session fetch failed",
        message:
          "Failed to fetch the ChatGPT session (network error or blocked). Nothing was modified.",
        retryKind: "secondary",
      },
      codex_auth_session_invalid: {
        tone: "danger",
        label: "Invalid session",
        message:
          "The session response is missing required fields; credentials can't be generated.",
        retryKind: "secondary",
      },
      codex_auth_store_mode_unsupported: {
        tone: "warning",
        label: "Store mode unsupported",
        message:
          'Codex isn\'t storing credentials as file; set cli_auth_credentials_store to "file", save, and try again.',
        retryKind: "none",
      },
      codex_auth_target_conflict: {
        tone: "warning",
        label: "Target in use",
        message:
          "The target file is in an unexpected state (symlink or modified before writing); stopped without overwriting.",
        retryKind: "secondary",
      },
      codex_auth_write_failed: {
        tone: "danger",
        label: "Write failed",
        message: "Write failed; the original file is unchanged.",
        retryKind: "secondary",
      },
      codex_auth_restore_failed: {
        tone: "danger",
        label: "Restore failed",
        message: "Restore failed; the original file is unchanged.",
        retryKind: "secondary",
      },
    },
  },
};
