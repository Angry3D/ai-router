import type { AppLocale, RouteModelsErrorCategory } from "../../../generated";

/** Route list, ordering, drag-and-drop announcements and row content. */
export interface RoutesListMessages {
  title: string;
  ariaLabel: string;
  newRoute: string;
  emptyTitle: string;
  addRoute: string;
  toolsAriaLabel: string;
  moveUp: string;
  moveDown: string;
  fallbackSwitch: string;
  fallbackHelpLabel: string;
  fallbackHelpRule1: string;
  fallbackHelpRule2: string;
  fallbackHelpRule3: string;
  toolsErrorLabel: (detail: string) => string;
  dragHandleLabel: (routeName: string) => string;
  dragHandleTitle: string;
  routeSelectLabel: (
    routeName: string,
    host: string,
    detail: string,
  ) => string;
  announcementStart: (description: string) => string;
  announcementEnd: (description: string) => string;
  announcementCancel: (description: string) => string;
  keyboardInstructions: string;
  fallbackBoundaryItem: (participantCount: number) => string;
  itemFallback: string;
  notParticipating: string;
  participating: (position: number) => string;
  itemPosition: (
    routeName: string,
    routePosition: number,
    fallbackDescription: string,
  ) => string;
}

/** Fallback boundary bar inside the route list. */
export interface RoutesBoundaryMessages {
  label: string;
  dragHandleTitle: string;
  dragHandleLabel: string;
  description: (participantCount: number, routeCount: number) => string;
  savingSuffix: string;
}

/** Route health markers and detail lines for a route row. */
export interface RoutesHealthMessages {
  current: string;
  strikingCurrent: (failureCount: number) => string;
  striking: (failureCount: number) => string;
  strikingDetail: (failureCount: number) => string;
  switchingCurrent: string;
  switching: string;
  switchingDetail: string;
  switchPendingCurrent: string;
  switchPending: string;
  switchPendingDetail: (remaining: number) => string;
  switchPendingWaiting: string;
  recovering: string;
  recoveringDetailRemaining: (remaining: number) => string;
  recoveringDetailWaiting: string;
  bypassedMarker: (remaining: number) => string;
  bypassedDetail: (remaining: number) => string;
  pausedMarker: (remaining: number) => string;
  pausedDetail: (remaining: number) => string;
  pendingMarker: string;
  pendingDetail: string;
  probingMarker: string;
  probingDetail: (attempt: number) => string;
}

/** Route editor form, model catalog and balance-query surfaces. */
export interface RoutesEditorMessages {
  emptyTitle: string;
  loading: string;
  loadFailed: string;
  newRouteTitle: string;
  editRouteLabel: (routeName: string) => string;
  nameLabel: string;
  protocolLabel: string;
  baseUrlLabel: string;
  inferenceUrlLabel: string;
  apiKeyLabel: string;
  menuVisibleLabel: string;
  queryModeLabel: string;
  queryEnabledLabel: string;
  scriptAriaLabel: string;
  invalidUrl: string;
  insecureHttpWarning: string;
  hideApiKey: string;
  showApiKey: string;
  hideKey: string;
  showKey: string;
  menuVisibleHelpLabel: string;
  menuVisibleHelpText: string;
  checkReachability: string;
  fetchModels: string;
  fetchingModels: string;
  modelsFetching: string;
  modelsEmpty: string;
  modelsFetched: (count: number) => string;
  modelsNotFetched: string;
  modelsError: Record<RouteModelsErrorCategory, string>;
  validationModelIdRequired: string;
  validationControlCharacter: string;
  validationModelIdDuplicate: string;
  validationPositiveInteger: string;
  fallbackModelsTitle: string;
  fallbackModelsCount: (count: number) => string;
  fallbackModelsCountUnsaved: (count: number) => string;
  fallbackModelsNotice: string;
  removeFallbackModelLabel: (modelId: string) => string;
  removeFallbackModelTitle: (modelId: string) => string;
  fallbackModelComboboxLabel: string;
  fallbackModelPlaceholder: string;
  addModelLabel: string;
  addModelTitle: string;
  modelsTitle: string;
  modelsCount: (count: number) => string;
  modelsCountUnsaved: (count: number) => string;
  modelsCountRetry: (count: number) => string;
  modelsNotice: string;
  modelsEmptyGrid: string;
  modelsGridLabel: string;
  columnModelId: string;
  columnDisplayName: string;
  columnContextWindow: string;
  modelIdFieldLabel: (index: number) => string;
  displayNameFieldLabel: (index: number) => string;
  contextWindowFieldLabel: (index: number) => string;
  displayNamePlaceholder: string;
  removeModelLabel: (index: number) => string;
  modelSearchLabel: string;
  balanceTitle: string;
  queryModeAriaLabel: string;
  queryModeGeneral: string;
  queryModeCustom: string;
  formatScript: string;
  insertScaffold: string;
  scriptFormatFailed: string;
  balanceResult: (amount: string, unit: string) => string;
  balanceInvalidResult: string;
  testBalance: string;
  activationRestartCodex: string;
  activationConnectCodex: string;
  activationReconnectCodex: string;
  activationFixCodexConfig: string;
  activationSaved: string;
  saveRetryRequired: string;
  reachable: string;
  slow: string;
  pathNotFound: string;
  unreachable: string;
  scriptRiskTitle: string;
  scriptRiskBody: string;
  scriptRiskConfirm: string;
  deleteActiveTitle: (routeName: string) => string;
  deleteTitle: (routeName: string) => string;
  deleteActiveBody: string;
  deleteBody: string;
  deleteConfirm: string;
  deleteRoute: string;
}

export interface RoutesMessages {
  list: RoutesListMessages;
  boundary: RoutesBoundaryMessages;
  health: RoutesHealthMessages;
  editor: RoutesEditorMessages;
}

export const routesMessages: Record<AppLocale, RoutesMessages> = {
  "zh-CN": {
    list: {
      title: "路由",
      ariaLabel: "路由列表",
      newRoute: "新建路由",
      emptyTitle: "还没有路由",
      addRoute: "添加路由",
      toolsAriaLabel: "路由排序与自动 Fallback",
      moveUp: "上移所选路由",
      moveDown: "下移所选路由",
      fallbackSwitch: "自动 Fallback",
      fallbackHelpLabel: "说明自动 Fallback 切换规则",
      fallbackHelpRule1: "5 次可归因失败后，按顺序切换后续路由。",
      fallbackHelpRule2: "兼容请求验证成功 2 次后，恢复更前路由。",
      fallbackHelpRule3: "手动切换后，重新计数。",
      toolsErrorLabel: (detail) => `路由设置失败：${detail}`,
      dragHandleLabel: (routeName) => `拖动调整路由顺序：${routeName}`,
      dragHandleTitle: "拖动调整路由顺序",
      routeSelectLabel: (routeName, host, detail) =>
        `${routeName}，${host}，${detail}`,
      announcementStart: (description) =>
        `已提起${description}。使用上下方向键移动，空格或回车放置，Escape 取消。`,
      announcementEnd: (description) => `已放置${description}。`,
      announcementCancel: (description) => `已取消移动${description}。`,
      keyboardInstructions:
        "按空格提起，使用上下方向键移动，按空格或回车放置，按 Escape 取消。",
      fallbackBoundaryItem: (participantCount) =>
        `Fallback 分界，${participantCount} 条路由参与 Fallback`,
      itemFallback: "路由",
      notParticipating: "不参与 Fallback",
      participating: (position) => `参与 Fallback，序号 ${position}`,
      itemPosition: (routeName, routePosition, fallbackDescription) =>
        `${routeName}，路由第 ${routePosition} 位，${fallbackDescription}`,
    },
    boundary: {
      label: "以下不参与 Fallback",
      dragHandleTitle: "拖动调整参与 Fallback 的路由数量",
      dragHandleLabel: "拖动调整 Fallback 参与分界",
      description: (participantCount, routeCount) =>
        `当前 ${participantCount} 条路由参与 Fallback，共 ${routeCount} 条路由`,
      savingSuffix: "，正在保存",
    },
    health: {
      current: "当前",
      strikingCurrent: (failureCount) => `当前 · ${failureCount}/5`,
      striking: (failureCount) => `失败 ${failureCount}/5`,
      strikingDetail: (failureCount) =>
        `已累计失败 ${failureCount}/5 · 仍使用当前路由，不切换`,
      switchingCurrent: "当前 · 切换中",
      switching: "切换中",
      switchingDetail: "已达到 5/5 · 正在切换到可用的后续路由",
      switchPendingCurrent: "当前 · 待切换",
      switchPending: "待切换",
      switchPendingDetail: (remaining) =>
        `已达到 5/5 · ${remaining} 秒后重试切换，仍使用当前路由`,
      switchPendingWaiting: "已达到 5/5 · 等待可用的后续路由",
      recovering: "恢复 1/2",
      recoveringDetailRemaining: (remaining) =>
        `已通过 1/2 次恢复验证 · ${remaining} 秒后再次验证`,
      recoveringDetailWaiting: "已通过 1/2 次恢复验证 · 等待下一个兼容请求",
      bypassedMarker: (remaining) => `待验证 · ${remaining}s`,
      bypassedDetail: (remaining) =>
        `曾因模型不兼容被跳过 · ${remaining} 秒后等待兼容请求验证`,
      pausedMarker: (remaining) => `暂停 · ${remaining}s`,
      pausedDetail: (remaining) => `暂停使用 · ${remaining} 秒后可恢复验证`,
      pendingMarker: "待验证",
      pendingDetail: "已可恢复验证 · 等待下一个兼容请求",
      probingMarker: "验证中",
      probingDetail: (attempt) => `正在进行第 ${attempt}/2 次恢复验证`,
    },
    editor: {
      emptyTitle: "选择或新建路由",
      loading: "正在读取路由...",
      loadFailed: "路由读取失败。",
      newRouteTitle: "新建路由",
      editRouteLabel: (routeName) => `编辑 ${routeName}`,
      nameLabel: "路由名称",
      protocolLabel: "上游协议",
      baseUrlLabel: "Base URL",
      inferenceUrlLabel: "最终推理地址",
      apiKeyLabel: "API Key",
      menuVisibleLabel: "在顶部菜单中显示",
      queryModeLabel: "查询方式",
      queryEnabledLabel: "启用余额查询",
      scriptAriaLabel: "JavaScript 表达式",
      invalidUrl: "地址无效",
      insecureHttpWarning: "该地址使用非回环 HTTP，API Key 将以明文传输。",
      hideApiKey: "隐藏 API Key",
      showApiKey: "显示 API Key",
      hideKey: "隐藏 Key",
      showKey: "显示 Key",
      menuVisibleHelpLabel: "说明菜单显示与自动 Fallback 资格",
      menuVisibleHelpText:
        "关闭后不会显示在顶部菜单，也不会参与自动 Fallback。",
      checkReachability: "检查推理地址",
      fetchModels: "获取模型列表",
      fetchingModels: "获取中…",
      modelsFetching: "正在获取模型列表…",
      modelsEmpty: "上游未返回模型",
      modelsFetched: (count) => `已获取 ${count} 个模型`,
      modelsNotFetched: "未获取模型列表",
      modelsError: {
        unauthorized: "获取失败：鉴权失败，请检查 API Key",
        not_found: "获取失败：上游未提供 /models 接口",
        network: "获取失败：无法连接上游",
        system_proxy:
          "系统代理不可用，请检查 macOS 手动代理设置或在系统设置中改用自定义代理；不支持 PAC/WPAD。",
        timeout: "获取失败：请求超时",
        http_status: "获取失败：上游响应不可用",
        too_large: "获取失败：上游响应不可用",
        invalid_response: "获取失败：上游响应不可用",
      },
      validationModelIdRequired: "请输入模型 ID。",
      validationControlCharacter: "不能包含控制字符。",
      validationModelIdDuplicate: "模型 ID 不能重复。",
      validationPositiveInteger: "请输入正整数。",
      fallbackModelsTitle: "跳过 Fallback 的模型",
      fallbackModelsCount: (count) => `${count} 个`,
      fallbackModelsCountUnsaved: (count) => `${count} 个 · 未保存`,
      fallbackModelsNotice:
        "当前路由收到这些模型时不会自动切换；作为候选路由时会被跳过。",
      removeFallbackModelLabel: (modelId) =>
        `移除跳过 Fallback 的模型：${modelId}`,
      removeFallbackModelTitle: (modelId) => `移除 ${modelId}`,
      fallbackModelComboboxLabel: "添加跳过 Fallback 的模型",
      fallbackModelPlaceholder: "输入模型 ID",
      addModelLabel: "添加模型",
      addModelTitle: "添加模型",
      modelsTitle: "自定义模型",
      modelsCount: (count) => `${count} 个`,
      modelsCountUnsaved: (count) => `${count} 个 · 未保存`,
      modelsCountRetry: (count) => `${count} 个 · 需要重试`,
      modelsNotice: "保存非空列表后，将替换 Codex 内置模型列表。",
      modelsEmptyGrid: "尚未添加自定义模型",
      modelsGridLabel: "自定义模型列表",
      columnModelId: "模型 ID",
      columnDisplayName: "显示名称",
      columnContextWindow: "上下文窗口（Token）",
      modelIdFieldLabel: (index) => `模型 ID ${index}`,
      displayNameFieldLabel: (index) => `显示名称 ${index}`,
      contextWindowFieldLabel: (index) => `上下文窗口（Token） ${index}`,
      displayNamePlaceholder: "使用模型 ID",
      removeModelLabel: (index) => `删除模型 ${index}`,
      modelSearchLabel: "搜索或输入模型 ID",
      balanceTitle: "余额查询",
      queryModeAriaLabel: "余额查询方式",
      queryModeGeneral: "通用查询",
      queryModeCustom: "自定义脚本",
      formatScript: "格式化",
      insertScaffold: "插入骨架",
      scriptFormatFailed: "脚本格式化失败，请检查 JavaScript 语法。",
      balanceResult: (amount, unit) => `余额 ${amount} ${unit}`,
      balanceInvalidResult: "查询返回无效结果",
      testBalance: "测试余额查询",
      activationRestartCodex: "已保存，重启 Codex 后生效",
      activationConnectCodex: "已保存，连接 Codex 后生效",
      activationReconnectCodex: "已保存，需要重新连接 Codex",
      activationFixCodexConfig: "已保存，请先修复 Codex 配置",
      activationSaved: "已保存",
      saveRetryRequired: "路由已保存，但模型目录尚未完整应用。请重试。",
      reachable: "可达",
      slow: "较慢",
      pathNotFound: "服务器可达，推理路径可能不正确",
      unreachable: "不可达",
      scriptRiskTitle: "允许余额脚本使用 API Key？",
      scriptRiskBody:
        "自定义脚本可以把当前路由的 API Key 发送到任意 HTTP(S) 地址。请只启用你信任的脚本。",
      scriptRiskConfirm: "确认并保存",
      deleteActiveTitle: (routeName) => `删除当前路由“${routeName}”？`,
      deleteTitle: (routeName) => `删除路由“${routeName}”？`,
      deleteActiveBody: "删除后将进入“无中转”，新请求会失败，直到你手动切换路由。",
      deleteBody: "这条路由及其 API Key 和余额脚本将被永久删除。",
      deleteConfirm: "删除路由",
      deleteRoute: "删除路由",
    },
  },
  en: {
    list: {
      title: "Routes",
      ariaLabel: "Route list",
      newRoute: "New route",
      emptyTitle: "No routes yet",
      addRoute: "Add route",
      toolsAriaLabel: "Route ordering and automatic fallback",
      moveUp: "Move selected route up",
      moveDown: "Move selected route down",
      fallbackSwitch: "Automatic fallback",
      fallbackHelpLabel: "Explain the automatic fallback switching rules",
      fallbackHelpRule1:
        "After 5 attributable failures, it switches to the next route in order.",
      fallbackHelpRule2:
        "After 2 successful compatible-request verifications, it restores the earlier route.",
      fallbackHelpRule3: "A manual switch resets the count.",
      toolsErrorLabel: (detail) => `Route settings failed: ${detail}`,
      dragHandleLabel: (routeName) => `Drag to reorder route: ${routeName}`,
      dragHandleTitle: "Drag to reorder routes",
      routeSelectLabel: (routeName, host, detail) =>
        `${routeName}, ${host}, ${detail}`,
      announcementStart: (description) =>
        `Picked up ${description}. Use the up and down arrow keys to move, space or enter to drop, Escape to cancel.`,
      announcementEnd: (description) => `Dropped ${description}.`,
      announcementCancel: (description) => `Cancelled moving ${description}.`,
      keyboardInstructions:
        "Press space to pick up, use the up and down arrow keys to move, press space or enter to drop, press Escape to cancel.",
      fallbackBoundaryItem: (participantCount) =>
        `Fallback boundary, ${participantCount} participating route${
          participantCount === 1 ? "" : "s"
        }`,
      itemFallback: "Route",
      notParticipating: "not participating in fallback",
      participating: (position) => `participating in fallback, position ${position}`,
      itemPosition: (routeName, routePosition, fallbackDescription) =>
        `${routeName}, route position ${routePosition}, ${fallbackDescription}`,
    },
    boundary: {
      label: "The following do not participate in fallback",
      dragHandleTitle: "Drag to adjust how many routes participate in fallback",
      dragHandleLabel: "Drag to adjust the fallback participation boundary",
      description: (participantCount, routeCount) =>
        `${participantCount} of ${routeCount} route${
          routeCount === 1 ? "" : "s"
        } participating in fallback`,
      savingSuffix: ", saving",
    },
    health: {
      current: "Current",
      strikingCurrent: (failureCount) => `Current · ${failureCount}/5`,
      striking: (failureCount) => `Failed ${failureCount}/5`,
      strikingDetail: (failureCount) =>
        `${failureCount}/5 failures accumulated · still using the current route, no switch`,
      switchingCurrent: "Current · switching",
      switching: "Switching",
      switchingDetail: "Reached 5/5 · switching to an available later route",
      switchPendingCurrent: "Current · pending switch",
      switchPending: "Pending switch",
      switchPendingDetail: (remaining) =>
        `Reached 5/5 · retrying the switch in ${remaining}s, still using the current route`,
      switchPendingWaiting: "Reached 5/5 · waiting for an available later route",
      recovering: "Recovering 1/2",
      recoveringDetailRemaining: (remaining) =>
        `Passed 1/2 recovery checks · rechecking in ${remaining}s`,
      recoveringDetailWaiting:
        "Passed 1/2 recovery checks · waiting for the next compatible request",
      bypassedMarker: (remaining) => `Pending check · ${remaining}s`,
      bypassedDetail: (remaining) =>
        `Skipped for model incompatibility · waiting for a compatible request to verify in ${remaining}s`,
      pausedMarker: (remaining) => `Paused · ${remaining}s`,
      pausedDetail: (remaining) =>
        `Paused · recovery check available in ${remaining}s`,
      pendingMarker: "Pending check",
      pendingDetail:
        "Ready for a recovery check · waiting for the next compatible request",
      probingMarker: "Checking",
      probingDetail: (attempt) => `Recovery check ${attempt}/2 in progress`,
    },
    editor: {
      emptyTitle: "Select or create a route",
      loading: "Loading route...",
      loadFailed: "Failed to load the route.",
      newRouteTitle: "New route",
      editRouteLabel: (routeName) => `Edit ${routeName}`,
      nameLabel: "Route name",
      protocolLabel: "Upstream protocol",
      baseUrlLabel: "Base URL",
      inferenceUrlLabel: "Final inference URL",
      apiKeyLabel: "API Key",
      menuVisibleLabel: "Show in the top menu",
      queryModeLabel: "Query method",
      queryEnabledLabel: "Enable balance query",
      scriptAriaLabel: "JavaScript expression",
      invalidUrl: "Invalid URL",
      insecureHttpWarning:
        "This URL uses non-loopback HTTP; the API key is sent in plain text.",
      hideApiKey: "Hide API Key",
      showApiKey: "Show API Key",
      hideKey: "Hide key",
      showKey: "Show key",
      menuVisibleHelpLabel:
        "Explain menu visibility and automatic fallback eligibility",
      menuVisibleHelpText:
        "When off, the route is hidden from the top menu and excluded from automatic fallback.",
      checkReachability: "Check inference URL",
      fetchModels: "Fetch model list",
      fetchingModels: "Fetching…",
      modelsFetching: "Fetching model list…",
      modelsEmpty: "The upstream returned no models",
      modelsFetched: (count) =>
        count === 1 ? `Fetched ${count} model` : `Fetched ${count} models`,
      modelsNotFetched: "Model list not fetched",
      modelsError: {
        unauthorized: "Fetch failed: authentication error, check the API key",
        not_found: "Fetch failed: the upstream has no /models endpoint",
        network: "Fetch failed: cannot reach the upstream",
        system_proxy:
          "The system proxy is unavailable. Check the macOS manual proxy settings or switch to a custom proxy in System Settings; PAC/WPAD is not supported.",
        timeout: "Fetch failed: request timed out",
        http_status: "Fetch failed: unusable upstream response",
        too_large: "Fetch failed: unusable upstream response",
        invalid_response: "Fetch failed: unusable upstream response",
      },
      validationModelIdRequired: "Enter a model ID.",
      validationControlCharacter: "Must not contain control characters.",
      validationModelIdDuplicate: "The model ID must be unique.",
      validationPositiveInteger: "Enter a positive integer.",
      fallbackModelsTitle: "Models skipped by fallback",
      fallbackModelsCount: (count) =>
        count === 1 ? `1 model` : `${count} models`,
      fallbackModelsCountUnsaved: (count) =>
        count === 1 ? `1 model · unsaved` : `${count} models · unsaved`,
      fallbackModelsNotice:
        "This route does not switch automatically when it receives these models, and is skipped when it is a candidate route.",
      removeFallbackModelLabel: (modelId) =>
        `Remove model skipped by fallback: ${modelId}`,
      removeFallbackModelTitle: (modelId) => `Remove ${modelId}`,
      fallbackModelComboboxLabel: "Add a model skipped by fallback",
      fallbackModelPlaceholder: "Enter a model ID",
      addModelLabel: "Add model",
      addModelTitle: "Add model",
      modelsTitle: "Custom models",
      modelsCount: (count) => (count === 1 ? `1 model` : `${count} models`),
      modelsCountUnsaved: (count) =>
        count === 1 ? `1 model · unsaved` : `${count} models · unsaved`,
      modelsCountRetry: (count) =>
        count === 1
          ? `1 model · retry required`
          : `${count} models · retry required`,
      modelsNotice:
        "Saving a non-empty list replaces the built-in Codex model list.",
      modelsEmptyGrid: "No custom models yet",
      modelsGridLabel: "Custom model list",
      columnModelId: "Model ID",
      columnDisplayName: "Display name",
      columnContextWindow: "Context window (tokens)",
      modelIdFieldLabel: (index) => `Model ID ${index}`,
      displayNameFieldLabel: (index) => `Display name ${index}`,
      contextWindowFieldLabel: (index) => `Context window (tokens) ${index}`,
      displayNamePlaceholder: "Use the model ID",
      removeModelLabel: (index) => `Remove model ${index}`,
      modelSearchLabel: "Search or enter a model ID",
      balanceTitle: "Balance query",
      queryModeAriaLabel: "Balance query method",
      queryModeGeneral: "General query",
      queryModeCustom: "Custom script",
      formatScript: "Format",
      insertScaffold: "Insert scaffold",
      scriptFormatFailed:
        "Formatting failed. Check the JavaScript syntax.",
      balanceResult: (amount, unit) =>
        unit ? `Balance ${amount} ${unit}` : `Balance ${amount}`,
      balanceInvalidResult: "The query returned an invalid result",
      testBalance: "Test balance query",
      activationRestartCodex: "Saved; restart Codex to apply",
      activationConnectCodex: "Saved; connect Codex to apply",
      activationReconnectCodex: "Saved; reconnect Codex",
      activationFixCodexConfig: "Saved; fix the Codex configuration first",
      activationSaved: "Saved",
      saveRetryRequired:
        "The route was saved, but the model catalog was not fully applied. Try again.",
      reachable: "Reachable",
      slow: "Slow",
      pathNotFound: "Server reachable, inference path may be incorrect",
      unreachable: "Unreachable",
      scriptRiskTitle: "Allow the balance script to use the API Key?",
      scriptRiskBody:
        "A custom script can send this route's API Key to any HTTP(S) address. Only enable scripts you trust.",
      scriptRiskConfirm: "Confirm and save",
      deleteActiveTitle: (routeName) => `Delete the active route “${routeName}”?`,
      deleteTitle: (routeName) => `Delete route “${routeName}”?`,
      deleteActiveBody:
        "After deletion the app enters “no relay”, and new requests fail until you manually switch routes.",
      deleteBody:
        "This route, its API Key and its balance script are permanently deleted.",
      deleteConfirm: "Delete route",
      deleteRoute: "Delete route",
    },
  },
};
