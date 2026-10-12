import type { AppLocale } from "../../../generated";

/**
 * System & database-recovery surfaces: appearance/language section,
 * proxy/storage/log settings, recovery/restore/start-over and fatal states.
 */
export interface SystemRecoveryMessages {
  // Appearance & language (established; owned by the appearance/language rows).
  appearanceSectionTitle: string;
  themeLabel: string;
  themeSystem: string;
  themeLight: string;
  themeDark: string;
  languageLabel: string;
  languageSystem: string;
  languageSaveFailed: string;
  languageSyncIncomplete: string;

  // Page + recovery health.
  pageTitle: string;
  recoveryHealthProtected: string;
  recoveryHealthUpdating: string;
  recoveryHealthDegraded: string;
  incidentRepaired: (detectedAt: string) => string;
  incidentRecoveryFlow: (detectedAt: string) => string;
  incidentStartedOver: (detectedAt: string) => string;

  // Outbound proxy.
  outboundProxySectionTitle: string;
  outboundProxyModeLabel: string;
  outboundProxyModeAriaLabel: string;
  outboundProxyModeSystem: string;
  outboundProxyModeCustom: string;
  outboundProxyUrlLabel: string;
  outboundProxyTestButton: string;
  outboundProxyTesting: string;
  outboundProxyTestSuccess: string;
  outboundProxyTestFailure: string;
  outboundProxyHelpProxy: string;
  outboundProxyHelpSystem: string;
  outboundProxyUrlRequired: string;
  outboundProxyUrlTooLong: string;
  outboundProxyUrlControlCharacter: string;
  outboundProxyUrlInvalid: string;
  outboundProxyUrlUnsupportedScheme: string;
  outboundProxyUrlCredentials: string;
  outboundProxyUrlQueryOrFragment: string;

  // Menu bar.
  menuBarSectionTitle: string;
  menuBarStatusTextLabel: string;
  menuBarActivityAnimationLabel: string;

  // Balance query.
  balanceQuerySectionTitle: string;
  balanceMenuDebounceLabel: string;
  balanceAutomaticRefreshLabel: string;
  secondsUnit: string;
  minutesUnit: string;
  applyButton: string;
  savedStatus: string;
  numberRequired: string;
  integerRequired: string;
  integerRange: (minimum: number, maximum: number) => string;

  // Database recovery (settings section).
  databaseRecoverySectionTitle: string;
  databaseRecoveryHelpLabel: string;
  databaseRecoveryHelpBody: string;
  recoveryStatusLabel: string;
  latestRecoveryPointLabel: string;
  noRecoveryPoint: string;
  validRecoveryPointLabel: string;
  recoveryPointCount: (count: number) => string;
  latestIncidentLabel: string;
  createRecoveryPointButton: string;
  creatingStatus: string;

  // Request history.
  requestHistorySectionTitle: string;
  recordCountLabel: string;
  coverageDateLabel: string;
  databaseUsageLabel: string;
  retentionLabel: string;
  retentionDays: (days: number) => string;
  metadataFailureWarning: (failures: number) => string;
  clearHistoryButton: string;
  clearHistoryConfirmTitle: string;
  clearHistoryConfirmBody: string;
  clearHistoryConfirmLabel: string;
  noRecords: string;
  coverageRange: (start: string, end: string) => string;

  // Runtime logs.
  runtimeLogSectionTitle: string;
  openLogDirectoryButton: string;
  clearLogsButton: string;
  clearLogsConfirmTitle: string;
  clearLogsConfirmBody: string;
  clearLogsConfirmLabel: string;

  // Recovery pages.
  recoveryPageTitle: string;
  recoveryRequiredSectionTitle: string;
  proxyStoppedStatus: string;
  recoveryRequiredBody: string;
  availableRecoveryPointsTitle: string;
  selectRecoveryPointLegend: string;
  restoringButton: string;
  restoreSelectedButton: string;
  noAvailableRecoveryPointsTitle: string;
  noAvailableRecoveryPointsBody: string;
  createEmptyDatabaseButton: string;
  rescanButton: string;
  quitButton: string;
  restoreConfirmTitle: string;
  restoreConfirmBody: (dateTime: string) => string;
  restoreConfirmLabel: string;
  startOverConfirmTitle: string;
  startOverConfirmBody: string;
  startOverConfirmLabel: string;
  recoveryLoadFailedTitle: string;
  recoveryLoadFailedBody: string;
  retryButton: string;
  databaseStartupFailedTitle: string;
  retryingDatabaseStartup: string;
  retryDatabaseStartupButton: string;

  // Fatal database startup issue presentations.
  databaseIssuePermissionTitle: string;
  databaseIssuePermissionBody: string;
  databaseIssueDiskFullTitle: string;
  databaseIssueDiskFullBody: string;
  databaseIssueFutureSchemaTitle: string;
  databaseIssueFutureSchemaBody: string;
  databaseIssueUnsafePathTitle: string;
  databaseIssueUnsafePathBody: string;
  databaseIssueUnavailableTitle: string;
  databaseIssueUnavailableBody: string;
  databaseIssueDirectoryInUseTitle: string;
  databaseIssueDirectoryInUseBody: string;
}

export const systemRecoveryMessages: Record<AppLocale, SystemRecoveryMessages> =
  {
    "zh-CN": {
      appearanceSectionTitle: "外观与语言",
      themeLabel: "主题",
      themeSystem: "跟随系统",
      themeLight: "浅色",
      themeDark: "深色",
      languageLabel: "语言",
      languageSystem: "跟随系统",
      languageSaveFailed: "保存失败，请重试。",
      languageSyncIncomplete: "语言已保存，同步尚未完成。",

      pageTitle: "系统",
      recoveryHealthProtected: "已保护",
      recoveryHealthUpdating: "正在更新",
      recoveryHealthDegraded: "保护已降级",
      incidentRepaired: (detectedAt) =>
        `检测到数据库索引损坏，已于 ${detectedAt} 自动修复`,
      incidentRecoveryFlow: (detectedAt) =>
        `检测到数据库损坏，已于 ${detectedAt} 进入恢复流程`,
      incidentStartedOver: (detectedAt) =>
        `数据库已重新开始，时间 ${detectedAt}`,

      outboundProxySectionTitle: "全局出站代理",
      outboundProxyModeLabel: "模式",
      outboundProxyModeAriaLabel: "全局出站代理模式",
      outboundProxyModeSystem: "跟随系统",
      outboundProxyModeCustom: "自定义代理",
      outboundProxyUrlLabel: "代理地址",
      outboundProxyTestButton: "测试连接",
      outboundProxyTesting: "正在测试",
      outboundProxyTestSuccess: "连接正常",
      outboundProxyTestFailure: "连接失败",
      outboundProxyHelpProxy:
        "外部请求（含图片下载和应用更新）使用此代理，本地回环地址始终直连。价格同步和会话获取仍跟随系统。",
      outboundProxyHelpSystem:
        "跟随 macOS 的 HTTP、HTTPS、SOCKS 手动代理和绕过规则，未配置代理时直连。不执行 PAC/WPAD；仅有自动代理可用时需改用自定义代理。本地回环地址始终直连。",
      outboundProxyUrlRequired: "请输入代理地址。",
      outboundProxyUrlTooLong: "代理地址过长。",
      outboundProxyUrlControlCharacter: "代理地址不能包含控制字符。",
      outboundProxyUrlInvalid: "请输入有效的代理地址。",
      outboundProxyUrlUnsupportedScheme:
        "支持 HTTP、HTTPS、SOCKS5 和 SOCKS5H 代理。",
      outboundProxyUrlCredentials: "代理地址不能包含用户名或密码。",
      outboundProxyUrlQueryOrFragment: "代理地址不能包含查询参数或片段。",

      menuBarSectionTitle: "菜单栏",
      menuBarStatusTextLabel: "菜单栏状态文字",
      menuBarActivityAnimationLabel: "菜单栏活动动画",

      balanceQuerySectionTitle: "余额查询",
      balanceMenuDebounceLabel: "菜单查询防抖",
      balanceAutomaticRefreshLabel: "自动查询间隔",
      secondsUnit: "秒",
      minutesUnit: "分钟",
      applyButton: "应用",
      savedStatus: "已保存",
      numberRequired: "请输入数值。",
      integerRequired: "请输入整数。",
      integerRange: (minimum, maximum) =>
        `请输入 ${minimum} 到 ${maximum} 之间的整数。`,

      databaseRecoverySectionTitle: "数据库恢复",
      databaseRecoveryHelpLabel: "数据库恢复说明",
      databaseRecoveryHelpBody:
        "主数据库无法安全启动时，应用会进入独立恢复流程；仅检测到可用恢复点时才允许恢复。",
      recoveryStatusLabel: "恢复状态",
      latestRecoveryPointLabel: "最近恢复点",
      noRecoveryPoint: "尚无恢复点",
      validRecoveryPointLabel: "有效恢复点",
      recoveryPointCount: (count) => `${count} 个`,
      latestIncidentLabel: "最近自动修复",
      createRecoveryPointButton: "创建恢复点",
      creatingStatus: "正在创建",

      requestHistorySectionTitle: "请求记录",
      recordCountLabel: "记录数",
      coverageDateLabel: "覆盖日期",
      databaseUsageLabel: "数据库占用",
      retentionLabel: "保留期限",
      retentionDays: (days) => `${days} 天`,
      metadataFailureWarning: (failures) =>
        `本次运行有 ${failures} 条请求元数据未记录，推理结果不受影响。`,
      clearHistoryButton: "清除全部请求记录",
      clearHistoryConfirmTitle: "清除全部请求记录？",
      clearHistoryConfirmBody:
        "请求记录将被永久删除，路由状态会重置为“未验证”。",
      clearHistoryConfirmLabel: "清除记录",
      noRecords: "暂无记录",
      coverageRange: (start, end) => `${start} 至 ${end}`,

      runtimeLogSectionTitle: "运行日志",
      openLogDirectoryButton: "打开日志目录",
      clearLogsButton: "清除日志",
      clearLogsConfirmTitle: "清除运行日志？",
      clearLogsConfirmBody: "现有日志将被清除，不影响代理运行。",
      clearLogsConfirmLabel: "清除日志",

      recoveryPageTitle: "恢复数据库",
      recoveryRequiredSectionTitle: "需要恢复",
      proxyStoppedStatus: "代理已停止",
      recoveryRequiredBody:
        "主数据库无法使用。请选择一个本地恢复点，恢复完成并验证通过后代理才会重新启动。",
      availableRecoveryPointsTitle: "可用恢复点",
      selectRecoveryPointLegend: "选择恢复点",
      restoringButton: "正在恢复",
      restoreSelectedButton: "恢复所选数据库",
      noAvailableRecoveryPointsTitle: "没有可用恢复点",
      noAvailableRecoveryPointsBody:
        "未找到通过校验的恢复点。创建空数据库会丢失现有路由、Key 和设置。",
      createEmptyDatabaseButton: "创建空数据库",
      rescanButton: "重新扫描恢复点",
      quitButton: "退出 AI Router",
      restoreConfirmTitle: "恢复所选数据库？",
      restoreConfirmBody: (dateTime) =>
        `将使用 ${dateTime} 的恢复点替换当前不可用数据库。`,
      restoreConfirmLabel: "恢复数据库",
      startOverConfirmTitle: "创建空数据库？",
      startOverConfirmBody:
        "现有数据库将被隔离保存，但应用会从空路由和新设置开始。此操作不能从界面撤销。",
      startOverConfirmLabel: "确认创建空数据库",
      recoveryLoadFailedTitle: "恢复点读取失败",
      recoveryLoadFailedBody: "无法读取安全的恢复点列表。可以重试或退出应用。",
      retryButton: "重试",
      databaseStartupFailedTitle: "数据库启动失败",
      retryingDatabaseStartup: "正在重试",
      retryDatabaseStartupButton: "重试数据库启动",

      databaseIssuePermissionTitle: "数据库无法访问",
      databaseIssuePermissionBody:
        "AI Router 无法访问数据库或恢复目录。请检查文件权限后重试。",
      databaseIssueDiskFullTitle: "磁盘空间不足",
      databaseIssueDiskFullBody:
        "释放磁盘空间后再重试。AI Router 不会在空间不足时替换数据库。",
      databaseIssueFutureSchemaTitle: "数据库版本过新",
      databaseIssueFutureSchemaBody:
        "此数据库由更高版本的 AI Router 创建，当前版本不会修改它。",
      databaseIssueUnsafePathTitle: "数据库路径不安全",
      databaseIssueUnsafePathBody:
        "数据库或恢复目录不是受支持的常规文件路径，AI Router 已停止启动。",
      databaseIssueUnavailableTitle: "数据库暂时不可用",
      databaseIssueUnavailableBody:
        "AI Router 无法安全打开数据库。代理仍保持停止状态。",
      databaseIssueDirectoryInUseTitle: "数据目录正在被占用",
      databaseIssueDirectoryInUseBody:
        "另一个 AI Router 进程正在使用该数据目录。请退出那个进程后重试。",
    },
    en: {
      appearanceSectionTitle: "Appearance & language",
      themeLabel: "Theme",
      themeSystem: "Follow system",
      themeLight: "Light",
      themeDark: "Dark",
      languageLabel: "Language",
      languageSystem: "Follow system",
      languageSaveFailed: "Failed to save. Try again.",
      languageSyncIncomplete: "Saved. Sync is not complete yet.",

      pageTitle: "System",
      recoveryHealthProtected: "Protected",
      recoveryHealthUpdating: "Updating",
      recoveryHealthDegraded: "Protection degraded",
      incidentRepaired: (detectedAt) =>
        `Database index corruption detected; automatically repaired on ${detectedAt}`,
      incidentRecoveryFlow: (detectedAt) =>
        `Database corruption detected; recovery flow started on ${detectedAt}`,
      incidentStartedOver: (detectedAt) =>
        `Database started over at ${detectedAt}`,

      outboundProxySectionTitle: "Global outbound proxy",
      outboundProxyModeLabel: "Mode",
      outboundProxyModeAriaLabel: "Global outbound proxy mode",
      outboundProxyModeSystem: "Follow system",
      outboundProxyModeCustom: "Custom proxy",
      outboundProxyUrlLabel: "Proxy address",
      outboundProxyTestButton: "Test connection",
      outboundProxyTesting: "Testing",
      outboundProxyTestSuccess: "Connection OK",
      outboundProxyTestFailure: "Connection failed",
      outboundProxyHelpProxy:
        "External requests, including image downloads and app updates, use this proxy; local loopback addresses always connect directly. Price sync and session fetches still follow the system.",
      outboundProxyHelpSystem:
        "Follows macOS manual HTTP, HTTPS, and SOCKS proxies and bypass rules, and connects directly when no proxy is configured. PAC/WPAD is not applied; when only automatic proxies are available, switch to a custom proxy. Local loopback addresses always connect directly.",
      outboundProxyUrlRequired: "Enter a proxy address.",
      outboundProxyUrlTooLong: "The proxy address is too long.",
      outboundProxyUrlControlCharacter:
        "The proxy address must not contain control characters.",
      outboundProxyUrlInvalid: "Enter a valid proxy address.",
      outboundProxyUrlUnsupportedScheme:
        "HTTP, HTTPS, SOCKS5, and SOCKS5H proxies are supported.",
      outboundProxyUrlCredentials:
        "The proxy address must not contain a username or password.",
      outboundProxyUrlQueryOrFragment:
        "The proxy address must not contain a query string or fragment.",

      menuBarSectionTitle: "Menu bar",
      menuBarStatusTextLabel: "Menu bar status text",
      menuBarActivityAnimationLabel: "Menu bar activity animation",

      balanceQuerySectionTitle: "Balance query",
      balanceMenuDebounceLabel: "Menu query debounce",
      balanceAutomaticRefreshLabel: "Automatic query interval",
      secondsUnit: "s",
      minutesUnit: "min",
      applyButton: "Apply",
      savedStatus: "Saved",
      numberRequired: "Enter a number.",
      integerRequired: "Enter a whole number.",
      integerRange: (minimum, maximum) =>
        `Enter a whole number between ${minimum} and ${maximum}.`,

      databaseRecoverySectionTitle: "Database recovery",
      databaseRecoveryHelpLabel: "About database recovery",
      databaseRecoveryHelpBody:
        "When the primary database cannot start safely, the app enters a separate recovery flow; restoring is allowed only after a usable recovery point is detected.",
      recoveryStatusLabel: "Recovery status",
      latestRecoveryPointLabel: "Latest recovery point",
      noRecoveryPoint: "No recovery point yet",
      validRecoveryPointLabel: "Valid recovery points",
      recoveryPointCount: (count) =>
        count === 1 ? "1 point" : `${count} points`,
      latestIncidentLabel: "Latest automatic repair",
      createRecoveryPointButton: "Create recovery point",
      creatingStatus: "Creating",

      requestHistorySectionTitle: "Request history",
      recordCountLabel: "Records",
      coverageDateLabel: "Date coverage",
      databaseUsageLabel: "Database size",
      retentionLabel: "Retention period",
      retentionDays: (days) => (days === 1 ? "1 day" : `${days} days`),
      metadataFailureWarning: (failures) =>
        `${failures} request metadata ${
          failures === 1 ? "record was" : "records were"
        } not recorded this run; inference results are unaffected.`,
      clearHistoryButton: "Clear all request history",
      clearHistoryConfirmTitle: "Clear all request history?",
      clearHistoryConfirmBody:
        "Request history will be permanently deleted and route state will reset to “unverified”.",
      clearHistoryConfirmLabel: "Clear history",
      noRecords: "No records",
      coverageRange: (start, end) => `${start} – ${end}`,

      runtimeLogSectionTitle: "Runtime logs",
      openLogDirectoryButton: "Open log folder",
      clearLogsButton: "Clear logs",
      clearLogsConfirmTitle: "Clear runtime logs?",
      clearLogsConfirmBody:
        "Existing logs will be cleared; the proxy keeps running.",
      clearLogsConfirmLabel: "Clear logs",

      recoveryPageTitle: "Recover database",
      recoveryRequiredSectionTitle: "Recovery required",
      proxyStoppedStatus: "Proxy stopped",
      recoveryRequiredBody:
        "The primary database is unusable. Select a local recovery point; the proxy restarts only after recovery completes and verification passes.",
      availableRecoveryPointsTitle: "Available recovery points",
      selectRecoveryPointLegend: "Select a recovery point",
      restoringButton: "Restoring",
      restoreSelectedButton: "Restore selected database",
      noAvailableRecoveryPointsTitle: "No available recovery points",
      noAvailableRecoveryPointsBody:
        "No recovery point passed validation. Creating an empty database discards existing routes, keys, and settings.",
      createEmptyDatabaseButton: "Create empty database",
      rescanButton: "Rescan recovery points",
      quitButton: "Quit AI Router",
      restoreConfirmTitle: "Restore the selected database?",
      restoreConfirmBody: (dateTime) =>
        `The current unusable database will be replaced with the recovery point from ${dateTime}.`,
      restoreConfirmLabel: "Restore database",
      startOverConfirmTitle: "Create an empty database?",
      startOverConfirmBody:
        "The existing database will be isolated and kept, but the app starts fresh with no routes and new settings. This cannot be undone from the app.",
      startOverConfirmLabel: "Confirm create empty database",
      recoveryLoadFailedTitle: "Failed to load recovery points",
      recoveryLoadFailedBody:
        "The list of safe recovery points could not be read. You can retry or quit the app.",
      retryButton: "Retry",
      databaseStartupFailedTitle: "Database startup failed",
      retryingDatabaseStartup: "Retrying",
      retryDatabaseStartupButton: "Retry database startup",

      databaseIssuePermissionTitle: "Cannot access the database",
      databaseIssuePermissionBody:
        "AI Router cannot access the database or recovery directory. Check file permissions and try again.",
      databaseIssueDiskFullTitle: "Not enough disk space",
      databaseIssueDiskFullBody:
        "Free up disk space and try again. AI Router will not replace the database when space is low.",
      databaseIssueFutureSchemaTitle: "Database version is too new",
      databaseIssueFutureSchemaBody:
        "This database was created by a newer version of AI Router. The current version will not modify it.",
      databaseIssueUnsafePathTitle: "Unsafe database path",
      databaseIssueUnsafePathBody:
        "The database or recovery directory is not a supported regular file path. AI Router has stopped starting up.",
      databaseIssueUnavailableTitle: "Database temporarily unavailable",
      databaseIssueUnavailableBody:
        "AI Router cannot safely open the database. The proxy stays stopped.",
      databaseIssueDirectoryInUseTitle: "Data directory is in use",
      databaseIssueDirectoryInUseBody:
        "Another AI Router process is using this data directory. Quit that process and try again.",
    },
  };
