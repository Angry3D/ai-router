import type {
  ApplicationUpdateSnapshotDto,
  BootstrapSnapshotDto,
  DatabaseStartupIssue,
  MenuSnapshotDto,
  RecoverySnapshotDto,
  RouteEditDto,
  RouteId,
  SettingsSnapshotDto,
  UsageHistoryPageDto,
  UsageHistoryQueryDto,
  UsageRequestDetailDto,
  UsageRouteOptionDto,
  UsageStatisticsDto,
  UsageStatisticsQueryDto,
} from "./generated";

const previewUpdateAvailable = {
  version: "0.2.0",
  notes: {
    highlights: [
      "菜单栏现在会显示代理请求的活动状态。",
      "应用更新页会在安装前展示本版本的重要变化。",
      "设置窗口在较小尺寸下保持操作按钮可见。",
    ],
    fixes: ["修复部分流式请求结束后活动状态未及时恢复的问题。"],
    notices: ["本版本无需迁移配置。"],
  },
  legacyNotes: null,
  releaseUrl: "https://github.com/Angry3D/ai-router/releases/tag/v0.2.0",
};

const previewLegacyUpdateAvailable = {
  version: "0.2.0",
  notes: null,
  legacyNotes:
    "AI Router v0.2.0 已发布。首次安装请下载 DMG；应用内更新会在安装前验证项目 updater 签名。",
  releaseUrl: "https://github.com/Angry3D/ai-router/releases/tag/v0.2.0",
};

export function previewApplicationUpdateSnapshot(): ApplicationUpdateSnapshotDto {
  const state = new URLSearchParams(window.location.search).get("update");
  const base: ApplicationUpdateSnapshotDto = {
    currentVersion: "0.1.0",
    operation: "idle",
    available: null,
    lastSuccessfulCheckAtMs: null,
    downloadedBytes: null,
    totalBytes: null,
    manualFailure: null,
  };
  switch (state) {
    case "checking":
      return { ...base, operation: "checking" };
    case "available":
      return {
        ...base,
        available: previewUpdateAvailable,
        lastSuccessfulCheckAtMs: previewNow,
      };
    case "legacy":
      return {
        ...base,
        available: previewLegacyUpdateAvailable,
        lastSuccessfulCheckAtMs: previewNow,
      };
    case "downloading":
      return {
        ...base,
        operation: "downloading",
        available: previewUpdateAvailable,
        lastSuccessfulCheckAtMs: previewNow,
        downloadedBytes: 31_457_280,
        totalBytes: 52_428_800,
      };
    case "installing":
      return {
        ...base,
        operation: "installing",
        available: previewUpdateAvailable,
        downloadedBytes: 52_428_800,
        totalBytes: 52_428_800,
      };
    case "failure":
      return {
        ...base,
        available: previewUpdateAvailable,
        lastSuccessfulCheckAtMs: previewNow,
        manualFailure: {
          code: "update_offline",
          message: "暂时无法连接更新服务，请稍后重试。",
          retryable: true,
        },
      };
    case "restart-ready":
      return {
        ...base,
        operation: "restart_ready",
        available: previewUpdateAvailable,
        downloadedBytes: 52_428_800,
        totalBytes: 52_428_800,
      };
    case "current":
      return { ...base, lastSuccessfulCheckAtMs: previewNow };
    default:
      return base;
  }
}

const workRouteId = "preview-work" as RouteId;
const personalRouteId = "preview-personal" as RouteId;
const ciiiRouteId = "preview-ciii" as RouteId;
const testRouteId = "preview-test" as RouteId;
const previewNow = new Date(2026, 6, 30, 1, 9, 2, 337).getTime();
const matchedUsageStartedAtMs = previewNow - 4 * 60_000;
const automaticRefreshIntervalMs = 30 * 60 * 1_000;

const routes = [
  {
    routeId: workRouteId,
    name: "AI INPUT 工作账号",
    baseUrlHost: "ai.input.im",
    inferenceStatus: {
      kind: "recent_success" as const,
      lastOutcome: "success" as const,
      failureReason: null,
      observedAtMs: Date.now(),
    },
    health: null,
  },
  {
    routeId: personalRouteId,
    name: "AI INPUT 个人账号",
    baseUrlHost: "ai.input.im",
    inferenceStatus: {
      kind: "recent_success" as const,
      lastOutcome: "success" as const,
      failureReason: null,
      observedAtMs: Date.now(),
    },
    health: null,
  },
  {
    routeId: ciiiRouteId,
    name: "Ciii 主用",
    baseUrlHost: "codex.ciii.club",
    inferenceStatus: {
      kind: "recent_success" as const,
      lastOutcome: "success" as const,
      failureReason: null,
      observedAtMs: Date.now(),
    },
    health: null,
  },
  {
    routeId: testRouteId,
    name: "Ciii 测试",
    baseUrlHost: "codex.ciii.club",
    inferenceStatus: {
      kind: "unverified" as const,
      lastOutcome: null,
      failureReason: null,
      observedAtMs: null,
    },
    health: null,
  },
];

export const previewMenuSnapshot: MenuSnapshotDto = {
  bootstrap: {
    revision: 12,
    routes,
    activeRouteId: workRouteId,
    fallback: {
      enabled: true,
      participantCount: 4,
      configRevision: 7,
      activePosition: 1,
      hasNext: true,
    },
    proxyStatus: "running",
    lifecycle: { phase: "running", issue: null },
    appearancePreference: "system",
  },
  codexStatus: "connected",
  codexRestartNotice: {
    noticeId: "preview-restart-notice",
    routeName: "AI INPUT 工作账号",
  },
  balanceEnabledRouteIds: routes.map((route) => route.routeId),
  balanceBatch: null,
  balances: [
    {
      routeId: workRouteId,
      value: {
        isValid: true,
        remaining: 263.71,
        used: null,
        total: null,
        unit: "$",
        planName: null,
        invalidMessage: null,
        extra: null,
      },
      status: "stale",
      lastSuccessAtMs: previewNow - automaticRefreshIntervalMs,
      lastCompletionAtMs: previewNow - automaticRefreshIntervalMs,
      nextDueAtMs: previewNow - 1,
      error: null,
    },
    {
      routeId: personalRouteId,
      value: {
        isValid: true,
        remaining: 142.58,
        used: null,
        total: null,
        unit: "$",
        planName: null,
        invalidMessage: null,
        extra: null,
      },
      status: "last_good",
      lastSuccessAtMs: previewNow - 60_000,
      lastCompletionAtMs: previewNow,
      nextDueAtMs: previewNow + automaticRefreshIntervalMs,
      error: { stage: "http", category: "network", transient: true },
    },
    {
      routeId: ciiiRouteId,
      value: {
        isValid: true,
        remaining: 89.34,
        used: null,
        total: null,
        unit: "$",
        planName: null,
        invalidMessage: null,
        extra: null,
      },
      status: "fresh",
      lastSuccessAtMs: previewNow,
      lastCompletionAtMs: previewNow,
      nextDueAtMs: previewNow + automaticRefreshIntervalMs,
      error: null,
    },
    {
      routeId: testRouteId,
      value: {
        isValid: true,
        remaining: 15.2,
        used: null,
        total: null,
        unit: "$",
        planName: null,
        invalidMessage: null,
        extra: null,
      },
      status: "fresh",
      lastSuccessAtMs: previewNow,
      lastCompletionAtMs: previewNow,
      nextDueAtMs: previewNow + automaticRefreshIntervalMs,
      error: null,
    },
  ],
};

export const previewSettingsSnapshot: SettingsSnapshotDto = {
  routes,
  activeRouteId: workRouteId,
  fallback: {
    enabled: true,
    participantCount: 3,
    configRevision: 7,
    activePosition: 1,
    hasNext: true,
  },
  proxyPort: 32189,
  menuBar: { statusTextEnabled: true, activityAnimationEnabled: true },
  imagesGeneration: { enabled: true, routeId: workRouteId, timeoutSecs: 600 },
  codexStatus: "changed",
  baseline: {
    exists: true,
    originalExists: true,
    capturedAtMs: Date.now() - 172_800_000,
  },
  originalBackup: {
    exists: true,
    originalExists: true,
    capturedAtMs: Date.now() - 172_800_000,
  },
  recoveryConfig: {
    exists: true,
    originalExists: true,
    updatedAtMs: Date.now() - 86_400_000,
  },
  balanceScriptRiskConfirmed: true,
  balanceQuery: { menuDebounceSeconds: 30, automaticRefreshMinutes: 30 },
  history: {
    requestCount: 18_642,
    earliestStartedAtMs: Date.now() - 31_449_600_000,
    latestStartedAtMs: Date.now(),
    databaseBytes: 13_421_773,
    retentionDays: 365,
  },
  metadataFailure: { droppedRecords: 0, writeFailures: 0, lastError: null },
  recovery: {
    kind: "protected",
    latestSuccessAtMs: previewNow - 15 * 60_000,
    validPointCount: 3,
  },
};

export const previewFallbackUiSettingsSnapshot: SettingsSnapshotDto = {
  ...previewSettingsSnapshot,
  routes: previewSettingsSnapshot.routes.map((route, index) => ({
    ...route,
    health:
      index === 0
        ? { kind: "striking", failureCount: 3 }
        : index === 1
          ? {
              kind: "open",
              origin: "provider_failure",
              recoverySuccesses: 0,
              retryAfterSeconds: 118,
            }
          : index === 2
            ? { kind: "probing", recoverySuccesses: 1 }
            : null,
  })),
};

export const previewMissingImageRouteSettingsSnapshot: SettingsSnapshotDto = {
  ...previewSettingsSnapshot,
  imagesGeneration: { enabled: true, routeId: null, timeoutSecs: 600 },
};

const previewLongRoutes = [
  ...routes,
  ...Array.from({ length: 6 }, (_, index) => ({
    routeId: `preview-relay-${index + 1}` as RouteId,
    name: `备用路由 ${index + 1}`,
    baseUrlHost: `relay-${index + 1}.example`,
    inferenceStatus: {
      kind: "unverified" as const,
      lastOutcome: null,
      failureReason: null,
      observedAtMs: null,
    },
    health: null,
  })),
];

export const previewLongRoutesSettingsSnapshot: SettingsSnapshotDto = {
  ...previewSettingsSnapshot,
  routes: previewLongRoutes,
  fallback: {
    enabled: true,
    participantCount: 5,
    configRevision: 7,
    activePosition: 1,
    hasNext: true,
  },
};

export const previewUpdatingSettingsSnapshot: SettingsSnapshotDto = {
  ...previewSettingsSnapshot,
  recovery: { ...previewSettingsSnapshot.recovery, kind: "updating" },
};

export const previewDegradedSettingsSnapshot: SettingsSnapshotDto = {
  ...previewSettingsSnapshot,
  recovery: {
    kind: "degraded",
    latestSuccessAtMs: previewNow - 2 * 24 * 60 * 60_000,
    validPointCount: 1,
  },
};

export const previewRecoveryRequiredBootstrap: BootstrapSnapshotDto = {
  ...previewMenuSnapshot.bootstrap,
  revision: 20,
  routes: [],
  activeRouteId: null,
  fallback: {
    enabled: false,
    participantCount: 0,
    configRevision: 7,
    activePosition: null,
    hasNext: false,
  },
  proxyStatus: "database_error",
  lifecycle: { phase: "recovery_required", issue: null },
};

export const previewRecoveryWithCandidates: RecoverySnapshotDto = {
  required: true,
  candidates: [
    {
      pointId: "9132641d-6fc5-4437-b1f7-525668e8b2e6",
      createdAtMs: previewNow - 15 * 60_000,
    },
    {
      pointId: "2151990c-e147-4c39-a463-a16f88f3898e",
      createdAtMs: previewNow - 24 * 60 * 60_000,
    },
  ],
  canStartOver: false,
  startupIssue: null,
  health: null,
};

export const previewRecoveryWithoutCandidates: RecoverySnapshotDto = {
  required: true,
  candidates: [],
  canStartOver: true,
  startupIssue: null,
  health: null,
};

function fatalBootstrap(issue: DatabaseStartupIssue): BootstrapSnapshotDto {
  return {
    ...previewRecoveryRequiredBootstrap,
    lifecycle: { phase: "database_error", issue: { database: issue } },
  };
}

function fatalRecovery(issue: DatabaseStartupIssue): RecoverySnapshotDto {
  return {
    required: false,
    candidates: [],
    canStartOver: false,
    startupIssue: issue,
    health: null,
  };
}

export const previewFatalDatabaseBootstraps: Record<
  DatabaseStartupIssue,
  BootstrapSnapshotDto
> = {
  permission: fatalBootstrap("permission"),
  disk_full: fatalBootstrap("disk_full"),
  future_schema: fatalBootstrap("future_schema"),
  unsafe_path: fatalBootstrap("unsafe_path"),
  unavailable: fatalBootstrap("unavailable"),
};

export const previewFatalRecoverySnapshots: Record<
  DatabaseStartupIssue,
  RecoverySnapshotDto
> = {
  permission: fatalRecovery("permission"),
  disk_full: fatalRecovery("disk_full"),
  future_schema: fatalRecovery("future_schema"),
  unsafe_path: fatalRecovery("unsafe_path"),
  unavailable: fatalRecovery("unavailable"),
};

export const previewRouteEdits: RouteEditDto[] = routes.map((route, index) => ({
  routeId: route.routeId,
  name: route.name,
  baseUrl: index < 2 ? "https://ai.input.im/v1" : "https://codex.ciii.club/v1",
  inferenceUrl:
    index < 2
      ? "https://ai.input.im/v1/responses"
      : "https://codex.ciii.club/v1/responses",
  apiKey: "preview-key-not-real",
  serviceTierPolicy: index === 1 ? "omit" : "passthrough",
  balanceQuery: {
    mode: "general_v1",
    enabled: true,
    customSource: "",
  },
  fallbackExcludedModels: [],
  models:
    index === 0
      ? [
          {
            modelId: "relay-custom-model",
            displayName: "Relay Custom",
            contextWindow: 200000,
          },
          {
            modelId: "gpt-5.5-preview",
            displayName: null,
            contextWindow: null,
          },
        ]
      : index === 1
        ? [
            {
              modelId: "personal-relay-model",
              displayName: "Personal Relay",
              contextWindow: 128000,
            },
          ]
        : [],
}));

export const previewFallbackUiRouteEdits: RouteEditDto[] =
  previewRouteEdits.map((route, index) => ({
    ...route,
    fallbackExcludedModels:
      index === 0
        ? ["gpt-5.6-luna", "gpt-5.2-codex", "relay-preview-model"]
        : route.fallbackExcludedModels,
  }));

export const previewUsageRouteOptions: UsageRouteOptionDto[] = [
  { routeId: workRouteId, name: "演示主路由", retained: false },
  { routeId: personalRouteId, name: "演示备用路由", retained: false },
  {
    routeId: "preview-deleted" as RouteId,
    name: "历史演示路由",
    retained: true,
  },
];

export const previewUsageHistoryPage: UsageHistoryPageDto = {
  rows: [
    {
      requestId: "request-preview-fallback",
      startedAtMs: matchedUsageStartedAtMs,
      finishedAtMs: matchedUsageStartedAtMs + 23_934,
      routeId: personalRouteId,
      routeName: "演示备用路由",
      requestedModel: "gpt-5.6-terra",
      actualModel: "gpt-5.6-terra",
      reasoningEffort: "high",
      streaming: true,
      completionState: "completed",
      httpStatus: 200,
      tokens: {
        input: 60_014,
        uncachedInput: 878,
        output: 40,
        total: 60_054,
        cachedInput: 59_136,
        cacheWriteInput: 0,
      },
      totalLatencyMs: 23_934,
      firstOutputLatencyMs: 11_664,
      cost: {
        state: "exact",
        amountPicoUsd: "35758000000",
        currency: "USD",
        catalogVersion: null,
        serviceTier: null,
        fastStatus: null,
      },
    },
    {
      requestId: "request-preview-small-cost",
      startedAtMs: previewNow - 18 * 60_000,
      finishedAtMs: previewNow - 18 * 60_000 + 486,
      routeId: workRouteId,
      routeName: "演示主路由",
      requestedModel: "gpt-5.6-luna",
      actualModel: "gpt-5.6-luna",
      reasoningEffort: "minimal",
      streaming: false,
      completionState: "completed",
      httpStatus: 200,
      tokens: {
        input: 1,
        uncachedInput: 1,
        output: 0,
        total: 1,
        cachedInput: 0,
        cacheWriteInput: null,
      },
      totalLatencyMs: 486,
      firstOutputLatencyMs: null,
      cost: {
        state: "exact",
        amountPicoUsd: "2000000",
        currency: "USD",
        catalogVersion: "openai-priority-2026-07-28",
        serviceTier: "priority",
        fastStatus: "confirmed",
      },
    },
    {
      requestId: "request-preview-unconfirmed-fast",
      startedAtMs: previewNow - 30 * 60_000,
      finishedAtMs: previewNow - 30 * 60_000 + 6_184,
      routeId: workRouteId,
      routeName: "演示主路由",
      requestedModel: "gpt-5.6-sol",
      actualModel: "gpt-5.6-sol",
      reasoningEffort: "high",
      streaming: true,
      completionState: "completed",
      httpStatus: 200,
      tokens: {
        input: 60_014,
        uncachedInput: 878,
        output: 40,
        total: 60_054,
        cachedInput: 59_136,
        cacheWriteInput: 0,
      },
      totalLatencyMs: 6_184,
      firstOutputLatencyMs: 1_720,
      cost: {
        state: "exact",
        amountPicoUsd: "70316000000",
        currency: "USD",
        catalogVersion: "openai-priority-2026-07-28",
        serviceTier: "priority",
        fastStatus: "unconfirmed",
      },
    },
    {
      requestId: "request-preview-legacy",
      startedAtMs: previewNow - 42 * 60_000,
      finishedAtMs: previewNow - 42 * 60_000 + 30_004,
      routeId: "preview-deleted" as RouteId,
      routeName: "历史演示路由",
      requestedModel: "legacy-preview-model-with-a-long-exact-identifier",
      actualModel: null,
      reasoningEffort: null,
      streaming: true,
      completionState: "failed",
      httpStatus: 502,
      tokens: {
        input: null,
        uncachedInput: null,
        output: null,
        total: null,
        cachedInput: null,
        cacheWriteInput: null,
      },
      totalLatencyMs: 30_004,
      firstOutputLatencyMs: null,
      cost: {
        state: "pre_v0_3a",
        amountPicoUsd: null,
        currency: "USD",
        catalogVersion: null,
        serviceTier: null,
        fastStatus: null,
      },
    },
    {
      requestId: "request-preview-unavailable",
      startedAtMs: previewNow - 65 * 60_000,
      finishedAtMs: previewNow - 65 * 60_000 + 1_240,
      routeId: workRouteId,
      routeName: "一个用于验证表格截断行为的超长演示路由名称",
      requestedModel: "preview-model-with-unavailable-official-pricing",
      actualModel: null,
      reasoningEffort: "a-very-long-preview-reasoning-effort-value",
      streaming: false,
      completionState: "cancelled",
      httpStatus: null,
      tokens: {
        input: 12_345_678,
        uncachedInput: 2_469_135,
        output: null,
        total: null,
        cachedInput: 9_876_543,
        cacheWriteInput: 123_456,
      },
      totalLatencyMs: null,
      firstOutputLatencyMs: 1_240,
      cost: {
        state: "unavailable",
        amountPicoUsd: null,
        currency: "USD",
        catalogVersion: null,
        serviceTier: null,
        fastStatus: null,
      },
    },
    {
      requestId: "request-preview-not-applicable",
      startedAtMs: previewNow - 90 * 60_000,
      finishedAtMs: previewNow - 90 * 60_000 + 8,
      routeId: null,
      routeName: null,
      requestedModel: "preview-no-upstream-model",
      actualModel: null,
      reasoningEffort: null,
      streaming: true,
      completionState: "no_upstream",
      httpStatus: 503,
      tokens: {
        input: 0,
        uncachedInput: 0,
        output: 0,
        total: 0,
        cachedInput: 0,
        cacheWriteInput: 0,
      },
      totalLatencyMs: 8,
      firstOutputLatencyMs: null,
      cost: {
        state: "not_applicable",
        amountPicoUsd: null,
        currency: "USD",
        catalogVersion: null,
        serviceTier: null,
        fastStatus: null,
      },
    },
  ],
  nextCursor: {
    finishedAtMs: previewNow - 90 * 60_000 + 8,
    requestId: "request-preview-not-applicable",
  },
  totalRows: 123,
};

export function previewUsageHistoryForQuery(
  query: UsageHistoryQueryDto,
): UsageHistoryPageDto {
  const modelContains = query.modelContains?.toLocaleLowerCase() ?? null;
  const matchingRows = previewUsageHistoryPage.rows
    .filter((row) => query.routeId === null || row.routeId === query.routeId)
    .filter(
      (row) =>
        row.finishedAtMs !== null &&
        (query.finishedAtOrAfterMs === null ||
          row.finishedAtMs >= query.finishedAtOrAfterMs) &&
        row.finishedAtMs <= query.finishedAtOrBeforeMs,
    )
    .filter(
      (row) =>
        query.completionState === null ||
        row.completionState === query.completionState,
    )
    .filter((row) => {
      if (modelContains === null) return true;
      return (row.actualModel ?? row.requestedModel ?? "")
        .toLocaleLowerCase()
        .includes(modelContains);
    });
  const cursorRows = matchingRows.filter((row) => {
    if (query.cursor === null || row.finishedAtMs === null) return true;
    return (
      row.finishedAtMs < query.cursor.finishedAtMs ||
      (row.finishedAtMs === query.cursor.finishedAtMs &&
        row.requestId < query.cursor.requestId)
    );
  });
  const rows = cursorRows.slice(0, query.limit);
  const lastRow = rows.at(-1);
  return {
    rows,
    nextCursor:
      cursorRows.length > rows.length &&
      lastRow !== undefined &&
      lastRow.finishedAtMs !== null
        ? {
            finishedAtMs: lastRow.finishedAtMs,
            requestId: lastRow.requestId,
          }
        : null,
    totalRows: matchingRows.length,
  };
}

export const previewUsageStatistics: UsageStatisticsDto = {
  matchedRequestCount: 3,
  tokens: {
    total: "120108",
    uncachedInput: "5904",
    cachedInput: "114000",
    cacheWriteInput: "204",
    output: "204",
  },
  costPicoUsd: "73916000000",
  granularity: "hour",
  trend: [
    {
      startedAtMs: previewNow - 3 * 60 * 60_000,
      finishedAtMs: previewNow - 2 * 60 * 60_000,
      label: "07/30 22:00",
      requestCount: 1,
      tokens: {
        total: "60054",
        uncachedInput: "878",
        cachedInput: "59136",
        cacheWriteInput: "0",
        output: "40",
      },
      costPicoUsd: "35758000000",
    },
    {
      startedAtMs: previewNow - 2 * 60 * 60_000,
      finishedAtMs: previewNow - 60 * 60_000,
      label: "07/30 23:00",
      requestCount: 2,
      tokens: {
        total: "60054",
        uncachedInput: "5026",
        cachedInput: "54864",
        cacheWriteInput: "204",
        output: "164",
      },
      costPicoUsd: "38158000000",
    },
  ],
  attribution: [
    {
      key: `route:${personalRouteId}`,
      label: "演示备用路由",
      isOther: false,
      value: "2",
      sharePercent: "66.7",
    },
    {
      key: `route:${workRouteId}`,
      label: "演示主路由",
      isOther: false,
      value: "1",
      sharePercent: "33.3",
    },
  ],
};

type PreviewStatisticsAttributionKey =
  `${UsageStatisticsQueryDto["attributionDimension"]}:${UsageStatisticsQueryDto["attributionMetric"]}`;

const previewStatisticsAttribution = {
  "route:requests": previewUsageStatistics.attribution,
  "route:tokens": [
    {
      key: `route:${personalRouteId}`,
      label: "演示备用路由",
      isOther: false,
      value: "60054",
      sharePercent: "50.0",
    },
    {
      key: `route:${workRouteId}`,
      label: "演示主路由",
      isOther: false,
      value: "60054",
      sharePercent: "50.0",
    },
  ],
  "route:cost": [
    {
      key: `route:${personalRouteId}`,
      label: "演示备用路由",
      isOther: false,
      value: "38158000000",
      sharePercent: "51.6",
    },
    {
      key: `route:${workRouteId}`,
      label: "演示主路由",
      isOther: false,
      value: "35758000000",
      sharePercent: "48.4",
    },
  ],
  "model:requests": [
    {
      key: "model:gpt-5.6-terra",
      label: "gpt-5.6-terra",
      isOther: false,
      value: "2",
      sharePercent: "66.7",
    },
    {
      key: "model:gpt-5.6-luna",
      label: "gpt-5.6-luna",
      isOther: false,
      value: "1",
      sharePercent: "33.3",
    },
  ],
  "model:tokens": [
    {
      key: "model:gpt-5.6-terra",
      label: "gpt-5.6-terra",
      isOther: false,
      value: "120000",
      sharePercent: "99.9",
    },
    {
      key: "model:gpt-5.6-luna",
      label: "gpt-5.6-luna",
      isOther: false,
      value: "108",
      sharePercent: "0.1",
    },
  ],
  "model:cost": [
    {
      key: "model:gpt-5.6-terra",
      label: "gpt-5.6-terra",
      isOther: false,
      value: "70316000000",
      sharePercent: "95.1",
    },
    {
      key: "model:gpt-5.6-luna",
      label: "gpt-5.6-luna",
      isOther: false,
      value: "3600000000",
      sharePercent: "4.9",
    },
  ],
} satisfies Record<
  PreviewStatisticsAttributionKey,
  UsageStatisticsDto["attribution"]
>;

export function previewUsageStatisticsForQuery(
  query: UsageStatisticsQueryDto,
): UsageStatisticsDto {
  const key: PreviewStatisticsAttributionKey = `${query.attributionDimension}:${query.attributionMetric}`;
  return {
    ...previewUsageStatistics,
    attribution: previewStatisticsAttribution[key],
  };
}

export const previewUsageRequestDetails: UsageRequestDetailDto[] =
  previewUsageHistoryPage.rows.map((request) => ({
    request,
    requestedServiceTier:
      request.requestId === "request-preview-fallback" ||
      request.requestId === "request-preview-small-cost" ||
      request.requestId === "request-preview-unconfirmed-fast"
        ? "priority"
        : null,
    actualServiceTier:
      request.requestId === "request-preview-small-cost"
        ? "priority"
        : request.requestId === "request-preview-fallback" ||
            request.requestId === "request-preview-unconfirmed-fast" ||
            request.requestId === "request-preview-unavailable" ||
            request.requestId === "request-preview-not-applicable"
          ? "default"
          : null,
    tokens: request.tokens,
    attempts:
      request.requestId === "request-preview-fallback"
        ? [
            {
              attemptIndex: 0,
              attemptRole: "ordinary",
              routeId: workRouteId,
              routeName: "演示主路由",
              startedAtMs: request.startedAtMs,
              finishedAtMs: request.startedAtMs + 812,
              httpStatus: 500,
              errorCategory: "upstream_server_error",
              deliveryState: "none",
              actualModel: "gpt-5.6-terra",
              forwardedServiceTier: "priority",
              actualServiceTier: "default",
              tokens: {
                input: 120,
                uncachedInput: null,
                output: 0,
                total: 120,
                cachedInput: null,
                cacheWriteInput: null,
              },
              cost: {
                state: "exact",
                amountPicoUsd: "600000000",
                currency: "USD",
                catalogVersion: "openai-priority-2026-07-28",
                serviceTier: "priority",
                fastStatus: "unconfirmed",
              },
              routingDecision: {
                kind: "retry_current",
                attemptNumber: 2,
                maxAttempts: 4,
              },
            },
            {
              attemptIndex: 1,
              attemptRole: "ordinary",
              routeId: workRouteId,
              routeName: "演示主路由",
              startedAtMs: request.startedAtMs + 900,
              finishedAtMs: request.startedAtMs + 1_712,
              httpStatus: 500,
              errorCategory: "upstream_http_status",
              deliveryState: "none",
              actualModel: "gpt-5.6-terra",
              forwardedServiceTier: "priority",
              actualServiceTier: "default",
              tokens: {
                input: 120,
                uncachedInput: null,
                output: 0,
                total: 120,
                cachedInput: null,
                cacheWriteInput: null,
              },
              cost: {
                state: "exact",
                amountPicoUsd: "600000000",
                currency: "USD",
                catalogVersion: "openai-priority-2026-07-28",
                serviceTier: "priority",
                fastStatus: "unconfirmed",
              },
              routingDecision: {
                kind: "activate_next",
                targetRouteId: personalRouteId,
                targetRouteName: "演示备用路由",
                skippedRoutes: [],
              },
            },
            {
              attemptIndex: 2,
              attemptRole: "ordinary",
              routeId: personalRouteId,
              routeName: "演示备用路由",
              startedAtMs: request.startedAtMs + 1_800,
              finishedAtMs: request.startedAtMs + 2_184,
              httpStatus: 200,
              errorCategory: null,
              deliveryState: "completed",
              actualModel: "gpt-5.6-terra",
              forwardedServiceTier: null,
              actualServiceTier: "default",
              tokens: request.tokens,
              cost: {
                state: "exact",
                amountPicoUsd: "35158000000",
                currency: "USD",
                catalogVersion: "openai-standard-2026-07-27",
                serviceTier: "default",
                fastStatus: null,
              },
              routingDecision: null,
            },
          ]
        : [],
  }));
