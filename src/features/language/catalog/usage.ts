import type { AppLocale } from "../../../generated";

/**
 * Usage settings, the statistics charts and the shared request-record table.
 *
 * Interpolated copy uses typed functions so both locales share one signature;
 * English handles its own plural forms. Formatting of numbers, dates and
 * currency stays in the formatting modules and is passed in already formatted.
 */
export interface UsageMessages {
  pageTitle: string;
  filtersAriaLabel: string;
  rangeLabel: string;
  rangeAriaLabel: string;
  rangeToday: string;
  rangeYesterday: string;
  range24h: string;
  range7d: string;
  range30d: string;
  rangeAll: string;
  statusLabel: string;
  statusAriaLabel: string;
  statusAll: string;
  completionCompleted: string;
  completionFailed: string;
  completionCancelled: string;
  completionNoUpstream: string;
  routeLabel: string;
  routesLoading: string;
  routesLoadFailedOption: string;
  routesAll: string;
  routeRetainedSuffix: string;
  modelLabel: string;
  modelContainsAriaLabel: string;
  modelPlaceholder: string;
  routeOptionsLoadFailed: string;
  retry: string;
  tabsAriaLabel: string;
  recordsTab: string;
  statisticsTab: string;
  reset: string;
  refresh: string;
  refreshing: string;
  backToList: string;
  requestDetailTitle: string;
  detailLoading: string;
  detailLoadFailed: string;
  recordsLoading: string;
  recordsLoadFailed: string;
  noRecords: string;
  paginationAriaLabel: string;
  previousPage: string;
  nextPage: string;
  /** Page position; the row total arrives already formatted. */
  pagePosition: (
    page: number,
    totalPages: number,
    formattedTotalRows: string,
  ) => string;
  statisticsCompletedOnly: string;
  statisticsLoading: string;
  statisticsLoadFailed: string;
  successfulRequests: string;
  matchedFilterHint: string;
  tokenLabel: string;
  uncachedInput: string;
  cachedInput: string;
  cacheWriteInput: string;
  output: string;
  cost: string;
  recordedUpstreamCost: string;
  noCompletedRequests: string;
  byTime: string;
  trendMetricLabel: string;
  bySource: string;
  sourceMetricLabel: string;
  noSourceData: string;
  tokenComposition: string;
  metricRequests: string;
  metricTokens: string;
  metricCost: string;
  startedAt: string;
  finishedAt: string;
  completedAtColumn: string;
  reasoningEffort: string;
  requestKind: string;
  streaming: string;
  synchronous: string;
  requestedServiceTier: string;
  actualServiceTier: string;
  forwardedServiceTier: string;
  notSpecified: string;
  notReported: string;
  notSent: string;
  finalTokens: string;
  tokensLabel: string;
  firstOutputLatency: string;
  totalLatency: string;
  upstreamTotalCost: string;
  pricingCatalog: string;
  upstreamAttempts: string;
  recoveryProbe: string;
  /** 1-based attempt number inside a request detail. */
  attemptNumbered: (attempt: number) => string;
  delivered: string;
  incomplete: string;
  retryCurrentRoute: (attemptNumber: number, maxAttempts: number) => string;
  switchedToRoute: (routeName: string) => string;
  skippedRouteFallbackExcluded: (routeName: string) => string;
  recoveryProbeFailedContinue: (routeName: string) => string;
  recoveryProbeCompleted: (routeName: string) => string;
  stopFallbackDisabled: string;
  stopFailureNotEligible: string;
  stopResponseCommitted: string;
  stopAllParticipantsAttempted: string;
  stopStalePolicy: string;
  stopActivationFailed: (routeName: string) => string;
  /** Fallback shown when a failed activation carries no target route name. */
  unknownTargetRoute: string;
  stopAttemptIndexExhausted: string;
  stopFailureThresholdNotReached: string;
  stopFailureThresholdReachedPending: string;
  stopRecoveryConfirmationPending: string;
  stopModelFallbackExcluded: string;
  costNotPricedPreV03A: string;
  costUnavailable: string;
  costNotApplicable: string;
  costAtLeast: (formattedAmount: string) => string;
  tokenUnknown: string;
  unknownRouteLabel: string;
  unknownModelLabel: string;
  otherLabel: string;
  deletedRouteLabel: string;
  tokenTotal: (formattedTokens: string) => string;
  tokenInput: (formattedTokens: string) => string;
  tokenOutput: (formattedTokens: string) => string;
  tokenCached: (formattedTokens: string) => string;
  tokenCacheWrite: (formattedTokens: string) => string;
  accessDenied: string;
  timeChartCountAriaLabel: string;
  timeChartMetricAriaLabel: (metricLabel: string) => string;
  sourceChartAriaLabel: (metricLabel: string) => string;
  tokenCompositionAriaLabel: string;
  timeChartDataAriaLabel: string;
  sourceChartDataAriaLabel: string;
  tokenCompositionDataAriaLabel: string;
  sourceSeriesName: (metricLabel: string) => string;
  redirectedCount: (count: number) => string;
  sharePercentInline: (sharePercent: string) => string;
  redirectedCountInline: (count: number) => string;
  /** `requested（actual）` pair for a redirected request-detail model. */
  modelPair: (requested: string, actual: string) => string;
  fastConfirmed: string;
  fastUnconfirmed: string;
  completionWithStatus: (
    completionLabel: string,
    httpStatus: number,
  ) => string;
  modelTitle: (model: string, reasoningEffort: string) => string;
  modelRedirectTitle: (
    requested: string,
    actual: string,
    reasoningEffort: string,
  ) => string;
  reasoningInline: (reasoningEffort: string) => string;
  latencyFirst: string;
  viewRequestAriaLabel: (timestamp: string) => string;
  columnState: string;
  columnTokens: string;
  columnTotalTokens: string;
  columnLatency: string;
  columnFirstOutputLatency: string;
  columnRequest: string;
}

export const usageMessages: Record<AppLocale, UsageMessages> = {
  "zh-CN": {
    pageTitle: "用量",
    filtersAriaLabel: "用量筛选",
    rangeLabel: "时间",
    rangeAriaLabel: "时间范围",
    rangeToday: "今天",
    rangeYesterday: "昨天",
    range24h: "最近 24 小时",
    range7d: "最近 7 天",
    range30d: "最近 30 天",
    rangeAll: "全部记录",
    statusLabel: "状态",
    statusAriaLabel: "完成状态",
    statusAll: "全部状态",
    completionCompleted: "已完成",
    completionFailed: "失败",
    completionCancelled: "已取消",
    completionNoUpstream: "无上游",
    routeLabel: "路由",
    routesLoading: "正在读取路由...",
    routesLoadFailedOption: "路由读取失败",
    routesAll: "全部路由",
    routeRetainedSuffix: "（已删除）",
    modelLabel: "模型",
    modelContainsAriaLabel: "模型包含",
    modelPlaceholder: "模型 ID 片段",
    routeOptionsLoadFailed: "路由选项读取失败。",
    retry: "重试",
    tabsAriaLabel: "用量视图",
    recordsTab: "请求记录",
    statisticsTab: "用量统计",
    reset: "重置",
    refresh: "刷新",
    refreshing: "正在更新...",
    backToList: "返回请求列表",
    requestDetailTitle: "请求详情",
    detailLoading: "正在读取请求详情...",
    detailLoadFailed: "请求详情读取失败。",
    recordsLoading: "正在读取请求记录...",
    recordsLoadFailed: "请求记录读取失败。",
    noRecords: "当前筛选条件下没有请求记录。",
    paginationAriaLabel: "请求记录分页",
    previousPage: "上一页",
    nextPage: "下一页",
    pagePosition: (page, totalPages, formattedTotalRows) =>
      `第 ${page} / ${totalPages} 页，共 ${formattedTotalRows} 条`,
    statisticsCompletedOnly: "用量统计只统计已完成请求。",
    statisticsLoading: "正在读取用量统计...",
    statisticsLoadFailed: "用量统计读取失败。",
    successfulRequests: "成功请求",
    matchedFilterHint: "匹配当前筛选条件",
    tokenLabel: "Token",
    uncachedInput: "未缓存输入",
    cachedInput: "缓存输入",
    cacheWriteInput: "写入缓存",
    output: "输出",
    cost: "费用",
    recordedUpstreamCost: "已记录的上游费用",
    noCompletedRequests: "当前筛选条件下没有已完成请求。",
    byTime: "按时间",
    trendMetricLabel: "趋势指标",
    bySource: "按来源",
    sourceMetricLabel: "来源指标",
    noSourceData: "当前筛选条件下没有来源数据。",
    tokenComposition: "Token 构成",
    metricRequests: "请求",
    metricTokens: "Token",
    metricCost: "费用",
    startedAt: "开始时间",
    finishedAt: "完成时间",
    completedAtColumn: "完成时间",
    reasoningEffort: "推理强度",
    requestKind: "类型",
    streaming: "流式",
    synchronous: "同步",
    requestedServiceTier: "请求服务层级",
    actualServiceTier: "实际服务层级",
    forwardedServiceTier: "发送服务层级",
    notSpecified: "未指定",
    notReported: "未报告",
    notSent: "未发送",
    finalTokens: "最终 Tokens",
    tokensLabel: "Tokens",
    firstOutputLatency: "首字延迟",
    totalLatency: "总耗时",
    upstreamTotalCost: "上游总费用",
    pricingCatalog: "计价目录",
    upstreamAttempts: "上游尝试",
    recoveryProbe: "恢复验证",
    attemptNumbered: (attempt) => `尝试 ${attempt}`,
    delivered: "已交付",
    incomplete: "未完成",
    retryCurrentRoute: (attemptNumber, maxAttempts) =>
      `重试当前路由（第 ${attemptNumber}/${maxAttempts} 次）`,
    switchedToRoute: (routeName) => `已自动切换至 ${routeName}`,
    skippedRouteFallbackExcluded: (routeName) =>
      `已跳过 ${routeName} · 该模型在此路由不参与 Fallback`,
    recoveryProbeFailedContinue: (routeName) =>
      `恢复验证未通过 · 继续使用 ${routeName}`,
    recoveryProbeCompleted: (routeName) =>
      `恢复验证完成 · 已恢复至 ${routeName}`,
    stopFallbackDisabled: "未切换 · Fallback 已关闭",
    stopFailureNotEligible: "未切换 · 当前错误不符合切换条件",
    stopResponseCommitted: "未切换 · 响应已经开始交付",
    stopAllParticipantsAttempted: "未切换 · 已到最后一条参与路由",
    stopStalePolicy: "未切换 · 路由配置已经变化",
    stopActivationFailed: (routeName) =>
      `切换至 ${routeName} 失败 · 状态未保存`,
    unknownTargetRoute: "目标路由",
    stopAttemptIndexExhausted: "未切换 · 请求尝试次数已达系统上限",
    stopFailureThresholdNotReached: "未切换 · 当前路由可归因失败尚未达到 5 次",
    stopFailureThresholdReachedPending:
      "未继续切换 · 已达到失败阈值，等待下一次可执行机会",
    stopRecoveryConfirmationPending: "未切换 · 恢复验证成功 1/2",
    stopModelFallbackExcluded:
      "已停止 Fallback · 该模型在此路由不参与 Fallback",
    costNotPricedPreV03A: "未计价（V0.3A 前）",
    costUnavailable: "不可用",
    costNotApplicable: "不适用",
    costAtLeast: (formattedAmount) => `至少 ${formattedAmount}`,
    tokenUnknown: "未知",
    unknownRouteLabel: "未知路由",
    unknownModelLabel: "未知模型",
    otherLabel: "其他",
    deletedRouteLabel: "已删除路由",
    tokenTotal: (formattedTokens) => `总计 ${formattedTokens}`,
    tokenInput: (formattedTokens) => `输入 ${formattedTokens}`,
    tokenOutput: (formattedTokens) => `输出 ${formattedTokens}`,
    tokenCached: (formattedTokens) => `缓存 ${formattedTokens}`,
    tokenCacheWrite: (formattedTokens) => `写入 ${formattedTokens}`,
    accessDenied: "访问拒绝",
    timeChartCountAriaLabel: "成功请求数量时间柱状图",
    timeChartMetricAriaLabel: (metricLabel) =>
      `成功请求${metricLabel}时间柱状图`,
    sourceChartAriaLabel: (metricLabel) => `模型${metricLabel}占比环形图`,
    tokenCompositionAriaLabel: "成功请求 Token 构成环形图",
    timeChartDataAriaLabel: "时间图表数据",
    sourceChartDataAriaLabel: "来源图表数据",
    tokenCompositionDataAriaLabel: "Token 构成图表数据",
    sourceSeriesName: (metricLabel) => `模型${metricLabel}`,
    redirectedCount: (count) => `转发 ${count} 次`,
    sharePercentInline: (sharePercent) => `，占比 ${sharePercent}%`,
    redirectedCountInline: (count) => `，转发 ${count} 次`,
    modelPair: (requested, actual) => `${requested}（${actual}）`,
    fastConfirmed: "Fast",
    fastUnconfirmed: "Fast 未确认",
    completionWithStatus: (completionLabel, httpStatus) =>
      `${completionLabel}（${httpStatus}）`,
    modelTitle: (model, reasoningEffort) => `${model}\n推理 ${reasoningEffort}`,
    modelRedirectTitle: (requested, actual, reasoningEffort) =>
      `${requested}\n→${actual}\n推理 ${reasoningEffort}`,
    reasoningInline: (reasoningEffort) => `推理 ${reasoningEffort}`,
    latencyFirst: "首字",
    viewRequestAriaLabel: (timestamp) => `查看请求 ${timestamp}`,
    columnState: "类型/状态",
    columnTokens: "TOKEN",
    columnTotalTokens: "Token",
    columnLatency: "延迟",
    columnFirstOutputLatency: "首 Token",
    columnRequest: "请求",
  },
  en: {
    pageTitle: "Usage",
    filtersAriaLabel: "Usage filters",
    rangeLabel: "Time",
    rangeAriaLabel: "Time range",
    rangeToday: "Today",
    rangeYesterday: "Yesterday",
    range24h: "Last 24 hours",
    range7d: "Last 7 days",
    range30d: "Last 30 days",
    rangeAll: "All records",
    statusLabel: "Status",
    statusAriaLabel: "Completion status",
    statusAll: "All statuses",
    completionCompleted: "Completed",
    completionFailed: "Failed",
    completionCancelled: "Cancelled",
    completionNoUpstream: "No upstream",
    routeLabel: "Route",
    routesLoading: "Loading routes...",
    routesLoadFailedOption: "Failed to load routes",
    routesAll: "All routes",
    routeRetainedSuffix: " (deleted)",
    modelLabel: "Model",
    modelContainsAriaLabel: "Model contains",
    modelPlaceholder: "Model ID fragment",
    routeOptionsLoadFailed: "Failed to load route options.",
    retry: "Retry",
    tabsAriaLabel: "Usage views",
    recordsTab: "Request records",
    statisticsTab: "Usage statistics",
    reset: "Reset",
    refresh: "Refresh",
    refreshing: "Updating...",
    backToList: "Back to request list",
    requestDetailTitle: "Request detail",
    detailLoading: "Loading request detail...",
    detailLoadFailed: "Failed to load request detail.",
    recordsLoading: "Loading request records...",
    recordsLoadFailed: "Failed to load request records.",
    noRecords: "No request records match the current filters.",
    paginationAriaLabel: "Request records pagination",
    previousPage: "Previous",
    nextPage: "Next",
    pagePosition: (page, totalPages, formattedTotalRows) =>
      `Page ${page} of ${totalPages} · ${formattedTotalRows} total`,
    statisticsCompletedOnly:
      "Usage statistics cover completed requests only.",
    statisticsLoading: "Loading usage statistics...",
    statisticsLoadFailed: "Failed to load usage statistics.",
    successfulRequests: "Successful requests",
    matchedFilterHint: "Matching the current filters",
    tokenLabel: "Tokens",
    uncachedInput: "Uncached input",
    cachedInput: "Cached input",
    cacheWriteInput: "Cache write",
    output: "Output",
    cost: "Cost",
    recordedUpstreamCost: "Recorded upstream cost",
    noCompletedRequests:
      "No completed requests match the current filters.",
    byTime: "Over time",
    trendMetricLabel: "Trend metric",
    bySource: "By source",
    sourceMetricLabel: "Source metric",
    noSourceData: "No source data matches the current filters.",
    tokenComposition: "Token composition",
    metricRequests: "Requests",
    metricTokens: "Tokens",
    metricCost: "Cost",
    startedAt: "Started",
    finishedAt: "Finished",
    completedAtColumn: "Completed",
    reasoningEffort: "Reasoning effort",
    requestKind: "Type",
    streaming: "Streaming",
    synchronous: "Synchronous",
    requestedServiceTier: "Requested service tier",
    actualServiceTier: "Actual service tier",
    forwardedServiceTier: "Forwarded service tier",
    notSpecified: "Not specified",
    notReported: "Not reported",
    notSent: "Not sent",
    finalTokens: "Final tokens",
    tokensLabel: "Tokens",
    firstOutputLatency: "First token latency",
    totalLatency: "Total time",
    upstreamTotalCost: "Total upstream cost",
    pricingCatalog: "Pricing catalog",
    upstreamAttempts: "Upstream attempts",
    recoveryProbe: "Recovery probe",
    attemptNumbered: (attempt) => `Attempt ${attempt}`,
    delivered: "Delivered",
    incomplete: "Incomplete",
    retryCurrentRoute: (attemptNumber, maxAttempts) =>
      `Retrying the current route (attempt ${attemptNumber}/${maxAttempts})`,
    switchedToRoute: (routeName) => `Automatically switched to ${routeName}`,
    skippedRouteFallbackExcluded: (routeName) =>
      `Skipped ${routeName} · this model is excluded from fallback on this route`,
    recoveryProbeFailedContinue: (routeName) =>
      `Recovery probe failed · staying on ${routeName}`,
    recoveryProbeCompleted: (routeName) =>
      `Recovery probe completed · restored to ${routeName}`,
    stopFallbackDisabled: "Not switched · fallback is disabled",
    stopFailureNotEligible: "Not switched · this error does not qualify",
    stopResponseCommitted:
      "Not switched · the response had already started delivering",
    stopAllParticipantsAttempted:
      "Not switched · the last participating route was reached",
    stopStalePolicy: "Not switched · the routing configuration changed",
    stopActivationFailed: (routeName) =>
      `Failed to switch to ${routeName} · state was not saved`,
    unknownTargetRoute: "the target route",
    stopAttemptIndexExhausted:
      "Not switched · the request attempt limit was reached",
    stopFailureThresholdNotReached:
      "Not switched · attributable failures on this route are below 5",
    stopFailureThresholdReachedPending:
      "Not switching further · the failure threshold was reached, waiting for the next eligible opportunity",
    stopRecoveryConfirmationPending: "Not switched · recovery probe succeeded 1/2",
    stopModelFallbackExcluded:
      "Fallback stopped · this model is excluded from fallback on this route",
    costNotPricedPreV03A: "Not priced (pre-V0.3A)",
    costUnavailable: "Unavailable",
    costNotApplicable: "Not applicable",
    costAtLeast: (formattedAmount) => `At least ${formattedAmount}`,
    tokenUnknown: "Unknown",
    unknownRouteLabel: "Unknown route",
    unknownModelLabel: "Unknown model",
    otherLabel: "Other",
    deletedRouteLabel: "Deleted route",
    tokenTotal: (formattedTokens) => `Total ${formattedTokens}`,
    tokenInput: (formattedTokens) => `Input ${formattedTokens}`,
    tokenOutput: (formattedTokens) => `Output ${formattedTokens}`,
    tokenCached: (formattedTokens) => `Cached ${formattedTokens}`,
    tokenCacheWrite: (formattedTokens) => `Cache write ${formattedTokens}`,
    accessDenied: "Access denied",
    timeChartCountAriaLabel: "Successful request count over time bar chart",
    timeChartMetricAriaLabel: (metricLabel) =>
      `Successful request ${metricLabel} over time bar chart`,
    sourceChartAriaLabel: (metricLabel) =>
      `${metricLabel} by model donut chart`,
    tokenCompositionAriaLabel:
      "Successful request token composition donut chart",
    timeChartDataAriaLabel: "Time chart data",
    sourceChartDataAriaLabel: "Source chart data",
    tokenCompositionDataAriaLabel: "Token composition chart data",
    sourceSeriesName: (metricLabel) => `${metricLabel} by model`,
    redirectedCount: (count) =>
      count === 1 ? "1 redirect" : `${count} redirects`,
    sharePercentInline: (sharePercent) => `, ${sharePercent}% of total`,
    redirectedCountInline: (count) =>
      count === 1 ? ", 1 redirect" : `, ${count} redirects`,
    modelPair: (requested, actual) => `${requested} (${actual})`,
    fastConfirmed: "Fast",
    fastUnconfirmed: "Fast unconfirmed",
    completionWithStatus: (completionLabel, httpStatus) =>
      `${completionLabel} (${httpStatus})`,
    modelTitle: (model, reasoningEffort) =>
      `${model}\nreasoning ${reasoningEffort}`,
    modelRedirectTitle: (requested, actual, reasoningEffort) =>
      `${requested}\n→${actual}\nreasoning ${reasoningEffort}`,
    reasoningInline: (reasoningEffort) => `reasoning ${reasoningEffort}`,
    latencyFirst: "First",
    viewRequestAriaLabel: (timestamp) => `View request ${timestamp}`,
    columnState: "Type/Status",
    columnTokens: "Tokens",
    columnTotalTokens: "Tokens",
    columnLatency: "Latency",
    columnFirstOutputLatency: "First token",
    columnRequest: "Request",
  },
};
