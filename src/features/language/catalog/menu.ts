import type {
  AppLocale,
  DatabaseStartupIssue,
  InferenceFailureReason,
  InferenceStatusKind,
  ProxyRuntimeStatus,
} from "../../../generated";

/**
 * Menu bar popover and usage-preview surfaces: runtime status controls,
 * route rows and balances, restart/capacity notices, confirmations,
 * empty/error states and the footer.
 */
export interface MenuMessages {
  /** Accessible name of the menu shell: `<app name> menu`. */
  shellLabel: (appName: string) => string;
  runtimeStatusLabel: string;

  proxyStatus: Record<ProxyRuntimeStatus, string>;
  inferenceStatus: Record<InferenceStatusKind, string>;
  failureReason: Record<InferenceFailureReason, string>;
  databaseIssue: Record<DatabaseStartupIssue, string>;

  codexControl: {
    /** Accessible state/action pair for the Codex runtime control. */
    controlLabel: (state: string, action: string) => string;
    connectedState: string;
    disconnectAction: string;
    disconnecting: string;
    checkingState: string;
    checkingAction: string;
    changedState: string;
    reconnectAction: string;
    notConnectedState: string;
    connectAction: string;
    connecting: string;
    selectRouteFirst: string;
    imagesConflictState: string;
    imagesConflictAction: string;
    configErrorState: string;
    configErrorAction: string;
  };

  fallbackControl: {
    /**
     * Accessible state/action pair for the Fallback runtime control, with an
     * optional trailing hint when no Fallback route follows the active one.
     */
    controlLabel: (
      state: string,
      action: string,
      noNextRouteHint: string | null,
    ) => string;
    unavailableState: string;
    minimumRoutesAction: string;
    disabledState: string;
    enableAction: string;
    enabledState: string;
    disableAction: string;
    enabling: string;
    disabling: string;
    boundaryIncluded: string;
    boundaryDisabled: string;
    noNextRoute: string;
  };

  balance: {
    notConfigured: string;
    none: string;
    queryFailed: string;
    available: string;
    pendingRefresh: string;
    lastResult: string;
    refreshedAt: (time: string) => string;
  };

  balanceBatch: {
    nothingToRefresh: string;
    refreshAll: string;
    refreshing: (completed: number, eligible: number) => string;
    allSucceeded: string;
    partial: (success: number, eligible: number, failed: number) => string;
  };

  routeList: {
    label: string;
    switchTo: (routeName: string) => string;
    refreshBalanceTitle: string;
    refreshBalanceLabel: (routeName: string) => string;
  };

  restartNotice: {
    label: string;
    switchedTo: (routeName: string) => string;
    body: string;
    dismissLabel: string;
    dismissTitle: string;
  };

  capacityNotice: {
    label: string;
    usage: (size: string) => string;
    threshold: (size: string) => string;
    manage: string;
    dismissLabel: string;
  };

  menuStates: {
    loading: string;
    proxyDatabaseError: string;
    proxyPortConflict: string;
    loadFailed: string;
    noRoutesTitle: string;
    noRoutesBody: string;
    noRoutesAction: string;
    hiddenRoutesTitle: string;
    hiddenRoutesBody: string;
    hiddenRoutesAction: string;
  };

  databaseBlocked: {
    recoveryRequiredTitle: string;
    recoveryRequiredBody: string;
    issueBody: string;
    openSettings: string;
  };

  codexDisconnect: {
    title: string;
    body: string;
    confirm: string;
  };

  routeSwitch: {
    title: (routeName: string) => string;
    customCatalog: string;
    officialCatalog: string;
    restartHint: string;
    confirm: string;
    pending: string;
    activationIncomplete: string;
  };

  footer: {
    proxyStopped: string;
    settingsLabel: string;
    settingsLabelWithUpdate: string;
    settingsTitle: string;
    quitLabel: (appName: string) => string;
    quitTitle: string;
  };

  usagePreview: {
    label: (routeName: string) => string;
    recentCount: string;
    loading: string;
    loadFailed: string;
    empty: string;
  };
}

export const menuMessages: Record<AppLocale, MenuMessages> = {
  "zh-CN": {
    shellLabel: (appName) => `${appName} 菜单`,
    runtimeStatusLabel: "运行状态",
    proxyStatus: {
      running: "代理运行中",
      starting: "代理启动中",
      port_conflict: "端口冲突",
      database_error: "数据库不可用",
      shutting_down: "正在退出",
      stopped: "代理故障",
      error: "代理故障",
    },
    inferenceStatus: {
      unverified: "未验证",
      recent_success: "最近成功",
      recent_failure: "最近失败",
      expired: "状态已过期",
    },
    failureReason: {
      connection: "连接失败",
      timeout: "请求超时",
      service: "服务失败",
      rate_limit: "限流失败",
      invalid_key: "Key无效",
      insufficient_quota: "余额不足",
      billing_limit: "额度受限",
      authentication: "鉴权失败",
      access_denied: "访问拒绝",
    },
    databaseIssue: {
      permission: "数据库权限异常",
      disk_full: "磁盘空间不足",
      future_schema: "数据库版本过新",
      unsafe_path: "数据库路径不安全",
      unavailable: "数据库不可用",
      directory_in_use: "数据目录被占用",
    },
    codexControl: {
      controlLabel: (state, action) => `${state}，${action}`,
      connectedState: "Codex 已连接",
      disconnectAction: "断开 Codex",
      disconnecting: "断开中",
      checkingState: "Codex 检查中",
      checkingAction: "正在检查",
      changedState: "Codex 待重新连接",
      reconnectAction: "重新连接",
      notConnectedState: "Codex 未连接",
      connectAction: "连接 Codex",
      connecting: "连接中",
      selectRouteFirst: "请先选择路由",
      imagesConflictState: "Codex 图片配置冲突",
      imagesConflictAction: "前往处理",
      configErrorState: "Codex 配置异常",
      configErrorAction: "修复配置",
    },
    fallbackControl: {
      controlLabel: (state, action, noNextRouteHint) =>
        `${state}，${action}${noNextRouteHint ? `；${noNextRouteHint}` : ""}`,
      unavailableState: "Fallback 不可用",
      minimumRoutesAction: "至少需 2 条路由",
      disabledState: "Fallback 已关闭",
      enableAction: "开启 Fallback",
      enabledState: "Fallback 已开启",
      disableAction: "关闭 Fallback",
      enabling: "开启中",
      disabling: "关闭中",
      boundaryIncluded: "以上参与 Fallback",
      boundaryDisabled: "Fallback 范围 · 已关闭",
      noNextRoute: "当前路由之后没有可用的 Fallback 路由",
    },
    balance: {
      notConfigured: "未配置余额",
      none: "尚无余额",
      queryFailed: "余额查询失败",
      available: "余额可用",
      pendingRefresh: "待刷新",
      lastResult: "上次结果",
      refreshedAt: (time) => `刷新于 ${time}`,
    },
    balanceBatch: {
      nothingToRefresh: "没有可更新的余额",
      refreshAll: "更新全部余额",
      refreshing: (completed, eligible) => `正在更新 ${completed}/${eligible}`,
      allSucceeded: "全部更新成功",
      partial: (success, eligible, failed) =>
        `已更新 ${success}/${eligible}，${failed} 项失败`,
    },
    routeList: {
      label: "路由",
      switchTo: (routeName) => `切换到 ${routeName}`,
      refreshBalanceTitle: "刷新余额",
      refreshBalanceLabel: (routeName) => `刷新 ${routeName} 的余额`,
    },
    restartNotice: {
      label: "Codex 模型列表更新提醒",
      switchedTo: (routeName) => `已自动切换至 ${routeName}`,
      body: "重启 Codex 后更新模型列表",
      dismissLabel: "关闭 Codex 模型列表更新提醒",
      dismissTitle: "关闭提醒",
    },
    capacityNotice: {
      label: "生成图片容量提醒",
      usage: (size) => `生成图片占用 ${size}`,
      threshold: (size) => `已达到 ${size} 提醒阈值`,
      manage: "管理",
      dismissLabel: "本次超限期间不再提醒",
    },
    menuStates: {
      loading: "正在读取状态...",
      proxyDatabaseError: "数据库不可用，代理未启动。",
      proxyPortConflict: "本地代理端口已被占用。",
      loadFailed: "状态读取失败",
      noRoutesTitle: "还没有路由",
      noRoutesBody: "添加一条自定义 Responses 路由。",
      noRoutesAction: "添加自定义 Responses 路由",
      hiddenRoutesTitle: "没有显示在菜单中的路由",
      hiddenRoutesBody: "可在路由设置中重新显示路由。",
      hiddenRoutesAction: "打开路由设置",
    },
    databaseBlocked: {
      recoveryRequiredTitle: "需要恢复数据库",
      recoveryRequiredBody:
        "代理已停止。请在设置中选择恢复点或创建空数据库。",
      issueBody: "代理已停止。请在设置中查看数据库启动状态。",
      openSettings: "打开恢复设置",
    },
    codexDisconnect: {
      title: "断开 Codex？",
      body: "当前 config.toml 将被断开恢复配置替换；更新恢复配置后保留的修改不会丢失。",
      confirm: "断开连接",
    },
    routeSwitch: {
      title: (routeName) => `切换到“${routeName}”？`,
      customCatalog: "该中转使用自定义模型。",
      officialCatalog: "该中转使用 Codex 官方模型。",
      restartHint: "切换后需要重启 Codex，模型列表才会更新。",
      confirm: "切换中转",
      pending: "切换中",
      activationIncomplete:
        "中转已切换，但 Codex 模型列表尚未完整应用。请重新连接 Codex 后重试。",
    },
    footer: {
      proxyStopped: "代理已停止",
      settingsLabel: "打开设置",
      settingsLabelWithUpdate: "打开设置，有可用更新",
      settingsTitle: "设置",
      quitLabel: (appName) => `退出 ${appName}`,
      quitTitle: "退出",
    },
    usagePreview: {
      label: (routeName) => `${routeName} 用量速览`,
      recentCount: "最近 10 条",
      loading: "正在读取请求记录",
      loadFailed: "请求记录读取失败",
      empty: "暂无请求记录",
    },
  },
  en: {
    shellLabel: (appName) => `${appName} menu`,
    runtimeStatusLabel: "Status",
    proxyStatus: {
      running: "Proxy on",
      starting: "Starting",
      port_conflict: "Port busy",
      database_error: "DB unavailable",
      shutting_down: "Exiting",
      stopped: "Proxy error",
      error: "Proxy error",
    },
    inferenceStatus: {
      unverified: "Unverified",
      recent_success: "Success",
      recent_failure: "Failed",
      expired: "Expired",
    },
    failureReason: {
      connection: "Connection failed",
      timeout: "Timeout",
      service: "Service failed",
      rate_limit: "Rate limited",
      invalid_key: "Invalid key",
      insufficient_quota: "Low balance",
      billing_limit: "Billing limit",
      authentication: "Auth failed",
      access_denied: "Access denied",
    },
    databaseIssue: {
      permission: "DB permission error",
      disk_full: "Low disk space",
      future_schema: "DB version too new",
      unsafe_path: "Unsafe DB path",
      unavailable: "DB unavailable",
      directory_in_use: "Data directory busy",
    },
    codexControl: {
      controlLabel: (state, action) => `${state}, ${action}`,
      connectedState: "Codex on",
      disconnectAction: "Disconnect",
      disconnecting: "Disconnecting",
      checkingState: "Checking Codex",
      checkingAction: "Checking",
      changedState: "Codex changed",
      reconnectAction: "Reconnect",
      notConnectedState: "Codex off",
      connectAction: "Connect",
      connecting: "Connecting",
      selectRouteFirst: "Select route",
      imagesConflictState: "Image config conflict",
      imagesConflictAction: "Resolve",
      configErrorState: "Config error",
      configErrorAction: "Fix config",
    },
    fallbackControl: {
      controlLabel: (state, action, noNextRouteHint) =>
        `${state}, ${action}${noNextRouteHint ? `; ${noNextRouteHint}` : ""}`,
      unavailableState: "Fallback N/A",
      minimumRoutesAction: "Need 2 routes",
      disabledState: "Fallback off",
      enableAction: "Enable",
      enabledState: "Fallback on",
      disableAction: "Disable",
      enabling: "Enabling",
      disabling: "Disabling",
      boundaryIncluded: "Fallback",
      boundaryDisabled: "Fallback off",
      noNextRoute: "No next Fallback route",
    },
    balance: {
      notConfigured: "No script",
      none: "No balance",
      queryFailed: "Query failed",
      available: "Available",
      pendingRefresh: "Pending",
      lastResult: "Last result",
      refreshedAt: (time) => `Updated ${time}`,
    },
    balanceBatch: {
      nothingToRefresh: "No balances",
      refreshAll: "Refresh all",
      refreshing: (completed, eligible) => `Refreshing ${completed}/${eligible}`,
      allSucceeded: "All refreshed",
      partial: (success, eligible, failed) =>
        `${success}/${eligible} updated, ${failed} failed`,
    },
    routeList: {
      label: "Routes",
      switchTo: (routeName) => `Select ${routeName}`,
      refreshBalanceTitle: "Refresh",
      refreshBalanceLabel: (routeName) => `Refresh ${routeName}`,
    },
    restartNotice: {
      label: "Codex model update",
      switchedTo: (routeName) => `Switched to ${routeName}`,
      body: "Restart Codex to update models",
      dismissLabel: "Dismiss Codex model update",
      dismissTitle: "Dismiss",
    },
    capacityNotice: {
      label: "Image capacity warning",
      usage: (size) => `Images use ${size}`,
      threshold: (size) => `Warning threshold: ${size}`,
      manage: "Manage",
      dismissLabel: "Don't warn again",
    },
    menuStates: {
      loading: "Loading...",
      proxyDatabaseError: "DB unavailable; proxy stopped.",
      proxyPortConflict: "Local proxy port is busy.",
      loadFailed: "Status load failed",
      noRoutesTitle: "No routes",
      noRoutesBody: "Add a custom Responses route.",
      noRoutesAction: "Add route",
      hiddenRoutesTitle: "No visible routes",
      hiddenRoutesBody: "Show routes in route settings.",
      hiddenRoutesAction: "Route settings",
    },
    databaseBlocked: {
      recoveryRequiredTitle: "DB recovery needed",
      recoveryRequiredBody:
        "Proxy stopped. Choose a recovery point or create an empty DB in settings.",
      issueBody: "Proxy stopped. Review DB startup status in settings.",
      openSettings: "Recovery settings",
    },
    codexDisconnect: {
      title: "Disconnect Codex?",
      body: "Replace the current config.toml with the restore config? Saved restore-config changes are kept.",
      confirm: "Disconnect",
    },
    routeSwitch: {
      title: (routeName) => `Switch to “${routeName}”?`,
      customCatalog: "This route uses custom models.",
      officialCatalog: "This route uses Codex models.",
      restartHint: "Restart Codex after switching to update models.",
      confirm: "Switch route",
      pending: "Switching",
      activationIncomplete:
        "Route switched, but the Codex model list is incomplete. Reconnect Codex and retry.",
    },
    footer: {
      proxyStopped: "Proxy off",
      settingsLabel: "Settings",
      settingsLabelWithUpdate: "Settings · update",
      settingsTitle: "Settings",
      quitLabel: (appName) => `Quit ${appName}`,
      quitTitle: "Quit",
    },
    usagePreview: {
      label: (routeName) => `${routeName} usage`,
      recentCount: "Latest 10",
      loading: "Loading requests",
      loadFailed: "Request load failed",
      empty: "No requests",
    },
  },
};
