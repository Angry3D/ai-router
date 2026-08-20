import { QueryClientProvider } from "@tanstack/react-query";
import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { createRouterQueryClient, queryKeys } from "../../api/query";
import type {
  RouteId,
  ServiceTierPolicy,
  SettingsSnapshotDto,
  UsageStatisticsQueryDto,
} from "../../generated";
import {
  previewMenuSnapshot,
  previewRouteEdits,
  previewSettingsSnapshot,
  previewUsageHistoryPage,
  previewUsageRequestDetails,
  previewUsageStatisticsForQuery,
} from "../../previewFixtures";
import { SettingsWindow } from "./SettingsWindow";
import { formatUsd } from "./usageFormatting";

const ipc = vi.hoisted(() => ({
  clearRequestHistory: vi.fn(),
  clearRuntimeLogs: vi.fn(),
  checkRouteReachability: vi.fn(),
  connectCodex: vi.fn(),
  createRecoveryPoint: vi.fn(),
  deleteRoute: vi.fn(),
  getRecoverySnapshot: vi.fn(),
  getRouteEdit: vi.fn(),
  getSettingsSnapshot: vi.fn(),
  getUsageHistory: vi.fn(),
  getUsageRequestDetail: vi.fn(),
  getUsageRouteOptions: vi.fn(),
  getUsageStatistics: vi.fn(),
  reorderRoutesAndFallback: vi.fn(),
  restoreCodex: vi.fn(),
  restoreRecoveryPoint: vi.fn(),
  retryDatabaseStartup: vi.fn(),
  saveRoute: vi.fn(),
  setFallbackEnabled: vi.fn(),
  startOverDatabase: vi.fn(),
  testBalanceQuery: vi.fn(),
  updateBalanceQuerySettings: vi.fn(),
  updateImagesGenerationSettings: vi.fn(),
  getRunningAppVersion: vi.fn(),
  hideSettingsWindow: vi.fn(),
  quitApplication: vi.fn(),
  tauriRuntime: false,
  listeners: {
    navigation: undefined as (() => void) | undefined,
    close: undefined as (() => void) | undefined,
  },
}));

const charts = vi.hoisted(() => ({
  use: vi.fn(),
  init: vi.fn(),
  setOption: vi.fn(),
  resize: vi.fn(),
  dispose: vi.fn(),
}));

vi.mock("echarts/core", () => ({
  use: charts.use,
  init: charts.init,
}));

vi.mock("../../api/ipc", () => ({
  applyProxyPort: vi.fn(),
  checkRouteReachability: ipc.checkRouteReachability,
  clearRequestHistory: ipc.clearRequestHistory,
  clearRuntimeLogs: ipc.clearRuntimeLogs,
  connectCodex: ipc.connectCodex,
  createRecoveryPoint: ipc.createRecoveryPoint,
  deleteRoute: ipc.deleteRoute,
  getApplicationUpdateSnapshot: vi.fn(async () => ({
    currentVersion: "0.1.0",
    operation: "idle",
    available: null,
    lastSuccessfulCheckAtMs: null,
    downloadedBytes: null,
    totalBytes: null,
    manualFailure: null,
  })),
  getBootstrapSnapshot: vi.fn(),
  getMenuSnapshot: vi.fn(),
  getRecoverySnapshot: ipc.getRecoverySnapshot,
  getRouteEdit: ipc.getRouteEdit,
  getSettingsSnapshot: ipc.getSettingsSnapshot,
  getUsageHistory: ipc.getUsageHistory,
  getUsageRequestDetail: ipc.getUsageRequestDetail,
  getUsageRouteOptions: ipc.getUsageRouteOptions,
  getUsageStatistics: ipc.getUsageStatistics,
  hideSettingsWindow: ipc.hideSettingsWindow,
  isTauriRuntime: () => ipc.tauriRuntime,
  listenSettingsCloseRequested: vi.fn(async (listener: () => void) => {
    ipc.listeners.close = listener;
    return vi.fn();
  }),
  listenSettingsNavigation: vi.fn(async (listener: () => void) => {
    ipc.listeners.navigation = listener;
    return vi.fn();
  }),
  listenStateChanged: vi.fn(async () => vi.fn()),
  reorderRoutesAndFallback: ipc.reorderRoutesAndFallback,
  normalizeIpcError: () => ({
    code: "test",
    message: "测试失败",
    retryable: false,
    field: null,
  }),
  openCodexConfig: vi.fn(),
  openRuntimeLogDirectory: vi.fn(),
  reconnectCodex: vi.fn(),
  restoreCodex: ipc.restoreCodex,
  restoreRecoveryPoint: ipc.restoreRecoveryPoint,
  retryDatabaseStartup: ipc.retryDatabaseStartup,
  saveRoute: ipc.saveRoute,
  setFallbackEnabled: ipc.setFallbackEnabled,
  startOverDatabase: ipc.startOverDatabase,
  testBalanceQuery: ipc.testBalanceQuery,
  updateBalanceQuerySettings: ipc.updateBalanceQuerySettings,
  updateImagesGenerationSettings: ipc.updateImagesGenerationSettings,
  quitApplication: ipc.quitApplication,
}));

vi.mock("../../api/appVersion", () => ({
  getRunningAppVersion: ipc.getRunningAppVersion,
}));

async function renderSettings(
  options: {
    riskConfirmed?: boolean;
    scriptEnabled?: boolean;
    proxyStatus?: "running" | "port_conflict" | "database_error";
    routeName?: string;
    serviceTierPolicy?: ServiceTierPolicy;
    settings?: Partial<SettingsSnapshotDto>;
  } = {},
) {
  const client = createRouterQueryClient();
  ipc.getRouteEdit.mockImplementation(async (routeId) => {
    const edit = previewRouteEdits.find(
      (candidate) => candidate.routeId === routeId,
    );
    if (!edit) throw new Error("route not found");
    return {
      ...structuredClone(edit),
      name: options.routeName ?? edit.name,
      serviceTierPolicy: options.serviceTierPolicy ?? edit.serviceTierPolicy,
      balanceQuery: edit.balanceQuery
        ? {
            ...structuredClone(edit.balanceQuery),
            enabled: options.scriptEnabled ?? edit.balanceQuery.enabled,
          }
        : null,
    };
  });
  const settings = {
    ...previewSettingsSnapshot,
    ...options.settings,
    balanceScriptRiskConfirmed:
      options.riskConfirmed ??
      previewSettingsSnapshot.balanceScriptRiskConfirmed,
  };
  client.setQueryData(queryKeys.settings, settings);
  client.setQueryData(queryKeys.bootstrap, {
    ...previewMenuSnapshot.bootstrap,
    proxyStatus:
      options.proxyStatus ?? previewMenuSnapshot.bootstrap.proxyStatus,
  });
  client.setQueryData(queryKeys.menu, previewMenuSnapshot);
  const rendered = render(
    <QueryClientProvider client={client}>
      <SettingsWindow />
    </QueryClientProvider>,
  );
  await screen.findByLabelText("路由名称");
  return {
    client,
    ...rendered,
  };
}

beforeEach(() => {
  charts.init.mockReset();
  charts.setOption.mockReset();
  charts.resize.mockReset();
  charts.dispose.mockReset();
  charts.init.mockReturnValue({
    setOption: charts.setOption,
    resize: charts.resize,
    dispose: charts.dispose,
  });
  ipc.clearRequestHistory.mockReset();
  ipc.clearRuntimeLogs.mockReset();
  ipc.checkRouteReachability.mockReset();
  ipc.connectCodex.mockReset();
  ipc.createRecoveryPoint.mockReset();
  ipc.getRecoverySnapshot.mockReset();
  ipc.getRouteEdit.mockReset();
  ipc.getSettingsSnapshot.mockReset();
  ipc.getUsageHistory.mockReset();
  ipc.getUsageRequestDetail.mockReset();
  ipc.getUsageRouteOptions.mockReset();
  ipc.getUsageStatistics.mockReset();
  ipc.reorderRoutesAndFallback.mockReset();
  ipc.saveRoute.mockReset();
  ipc.setFallbackEnabled.mockReset();
  ipc.deleteRoute.mockReset();
  ipc.restoreCodex.mockReset();
  ipc.restoreRecoveryPoint.mockReset();
  ipc.retryDatabaseStartup.mockReset();
  ipc.startOverDatabase.mockReset();
  ipc.testBalanceQuery.mockReset();
  ipc.updateBalanceQuerySettings.mockReset();
  ipc.updateImagesGenerationSettings.mockReset();
  ipc.getRunningAppVersion.mockReset();
  ipc.quitApplication.mockReset();
  ipc.tauriRuntime = false;
  ipc.getRunningAppVersion.mockResolvedValue("0.1.1");
  ipc.saveRoute.mockResolvedValue({
    routeId: previewRouteEdits[0].routeId,
    revision: 13,
    catalog: {
      models: structuredClone(previewRouteEdits[0].models),
      changed: false,
      projectionApplied: true,
      retryRequired: false,
      activation: "none",
      errorCode: null,
      retryToken: null,
    },
  });
  ipc.reorderRoutesAndFallback.mockResolvedValue({ revision: 14 });
  ipc.setFallbackEnabled.mockResolvedValue({ revision: 15 });
  ipc.updateBalanceQuerySettings.mockResolvedValue({ revision: 17 });
  ipc.updateImagesGenerationSettings.mockResolvedValue({ revision: 18 });
  ipc.createRecoveryPoint.mockResolvedValue(previewSettingsSnapshot.recovery);
  ipc.restoreRecoveryPoint.mockResolvedValue({ phase: "running", issue: null });
  ipc.retryDatabaseStartup.mockResolvedValue({ phase: "running", issue: null });
  ipc.startOverDatabase.mockResolvedValue({ phase: "running", issue: null });
  ipc.getUsageRouteOptions.mockResolvedValue([
    {
      routeId: previewRouteEdits[0].routeId,
      name: "AI INPUT 工作账号",
      retained: false,
    },
  ]);
  ipc.getUsageHistory.mockResolvedValue({
    rows: [
      {
        requestId: "request-usage-1",
        startedAtMs: 1_700_000_000_000,
        finishedAtMs: 1_700_000_000_120,
        routeId: previewRouteEdits[0].routeId,
        routeName: "AI INPUT 工作账号",
        requestedModel: "gpt-5",
        actualModel: "gpt-5",
        reasoningEffort: "medium",
        streaming: true,
        completionState: "completed",
        httpStatus: 200,
        tokens: {
          input: 10,
          uncachedInput: 6,
          output: 2,
          total: 12,
          cachedInput: 4,
          cacheWriteInput: null,
        },
        totalLatencyMs: 120,
        firstOutputLatencyMs: 40,
        cost: {
          state: "partial",
          amountPicoUsd: "28000000",
          currency: "USD",
          catalogVersion: "openai-standard-2026-07-27",
          serviceTier: "default",
          fastStatus: null,
        },
      },
    ],
    nextCursor: null,
    totalRows: 1,
  });
  ipc.getUsageStatistics.mockImplementation(
    async (query: UsageStatisticsQueryDto) =>
      structuredClone(previewUsageStatisticsForQuery(query)),
  );
  ipc.getUsageRequestDetail.mockResolvedValue({
    request: {
      requestId: "request-usage-1",
      startedAtMs: 1_700_000_000_000,
      finishedAtMs: 1_700_000_000_120,
      routeId: previewRouteEdits[0].routeId,
      routeName: "AI INPUT 工作账号",
      requestedModel: "gpt-5",
      actualModel: "gpt-5",
      reasoningEffort: "medium",
      streaming: true,
      completionState: "completed",
      httpStatus: 200,
      tokens: {
        input: 10,
        uncachedInput: null,
        output: 2,
        total: 12,
        cachedInput: null,
        cacheWriteInput: null,
      },
      totalLatencyMs: 120,
      firstOutputLatencyMs: 40,
      cost: {
        state: "partial",
        amountPicoUsd: "28000000",
        currency: "USD",
        catalogVersion: "openai-standard-2026-07-27",
        serviceTier: "default",
        fastStatus: null,
      },
    },
    requestedServiceTier: null,
    actualServiceTier: "default",
    tokens: {
      input: 10,
      uncachedInput: 6,
      output: 2,
      total: 12,
      cachedInput: 4,
      cacheWriteInput: null,
    },
    attempts: [],
  });
});

describe("UsageSettings interactions", () => {
  it("preserves pico-USD precision for small positive costs", () => {
    expect(formatUsd("50000")).toBe("$0.00000005");
    expect(formatUsd("0")).toBe("$0");
    expect(formatUsd(null)).toBe("不可用");
  });

  it("renders bounded usage filters, partial cost, and on-demand detail", async () => {
    ipc.tauriRuntime = true;
    await renderSettings();

    fireEvent.click(screen.getByRole("button", { name: "用量" }));
    const timeRange = screen.getByLabelText("时间范围");
    expect(timeRange).toHaveValue("7d");
    expect(
      within(timeRange)
        .getAllByRole("option")
        .map((option) => option.textContent),
    ).toEqual([
      "今天",
      "昨天",
      "最近 24 小时",
      "最近 7 天",
      "最近 30 天",
      "全部记录",
    ]);
    expect(screen.getByLabelText("完成状态")).toBeInTheDocument();
    expect(screen.getByRole("combobox", { name: "路由" })).toBeInTheDocument();
    expect(screen.getByLabelText("模型包含")).toBeInTheDocument();
    const partialCost = await screen.findByText("至少 $0.000028");
    expect(
      screen.queryByRole("columnheader", { name: "推理强度" }),
    ).not.toBeInTheDocument();
    expect(
      within(partialCost.closest("tr")!).getByText("推理 medium"),
    ).toBeInTheDocument();
    expect(
      within(partialCost.closest("tr")!).getByText(/^\d{4}\/\d{2}\/\d{2}$/),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "上一页" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "下一页" })).toBeDisabled();

    fireEvent.click(
      within(partialCost.closest("tr")!).getByRole("button", {
        name: /查看请求/,
      }),
    );
    expect(await screen.findByText("请求详情")).toBeInTheDocument();
    expect(
      await screen.findByText("openai-standard-2026-07-27"),
    ).toBeInTheDocument();
    expect(ipc.getUsageRequestDetail).toHaveBeenCalledWith("request-usage-1");
  });

  it("renders forwarded and actual Service Tier evidence per upstream attempt", async () => {
    ipc.getUsageRequestDetail.mockResolvedValue(
      structuredClone(previewUsageRequestDetails[0]),
    );
    await renderSettings();

    fireEvent.click(screen.getByRole("button", { name: "用量" }));
    fireEvent.click(await screen.findByRole("button", { name: /^查看请求/ }));

    const passthroughAttempt = (
      await screen.findByText("尝试 1 · 演示主路由")
    ).closest("li")!;
    expect(
      within(passthroughAttempt).getByText("发送服务层级"),
    ).toBeInTheDocument();
    expect(
      within(passthroughAttempt).getByText("priority"),
    ).toBeInTheDocument();
    expect(
      within(passthroughAttempt).getByText("实际服务层级"),
    ).toBeInTheDocument();
    expect(within(passthroughAttempt).getByText("default")).toBeInTheDocument();
    expect(
      within(passthroughAttempt).getByText("HTTP 500 · upstream_server_error"),
    ).toBeInTheDocument();

    const omitAttempt = screen
      .getByText("尝试 3 · 演示备用路由")
      .closest("li")!;
    expect(within(omitAttempt).getByText("未发送")).toBeInTheDocument();
    expect(within(omitAttempt).getByText("default")).toBeInTheDocument();
    expect(within(omitAttempt).getByText("HTTP 200")).toBeInTheDocument();
  });

  it("renders typed retry, activation, and every terminal Fallback decision", async () => {
    const detail = structuredClone(previewUsageRequestDetails[0]);
    const base = detail.attempts[0];
    const longRouteName = "团队主路由 · 华东生产环境 · 长名称仍然完整换行显示";
    detail.attempts = [
      {
        ...base,
        attemptIndex: 0,
        routingDecision: {
          kind: "retry_current",
          attemptNumber: 2,
          maxAttempts: 4,
        },
      },
      {
        ...base,
        attemptIndex: 1,
        routingDecision: {
          kind: "activate_next",
          targetRouteId: base.routeId,
          targetRouteName: longRouteName,
          skippedRoutes: [
            {
              routeId: base.routeId,
              routeName: "不兼容路由",
              reason: "model_fallback_excluded",
            },
          ],
        },
      },
      {
        ...base,
        attemptIndex: 2,
        attemptRole: "recovery_probe",
        routingDecision: {
          kind: "resume_captured",
          targetRouteId: base.routeId,
          targetRouteName: "当前路由",
        },
      },
      {
        ...base,
        attemptIndex: 3,
        attemptRole: "recovery_probe",
        routingDecision: {
          kind: "recover",
          targetRouteId: base.routeId,
          targetRouteName: "优先路由",
        },
      },
      ...(
        [
          ["fallback_disabled", "未切换 · Fallback 已关闭"],
          ["failure_not_eligible", "未切换 · 当前错误不符合切换条件"],
          ["response_committed", "未切换 · 响应已经开始交付"],
          ["all_participants_attempted", "未切换 · 已到最后一条参与路由"],
          ["stale_policy", "未切换 · 路由配置已经变化"],
          ["activation_failed", "切换至 目标备用路由 失败 · 状态未保存"],
          ["attempt_index_exhausted", "未切换 · 请求尝试次数已达系统上限"],
          [
            "failure_threshold_not_reached",
            "未切换 · 当前路由可归因失败尚未达到 5 次",
          ],
          [
            "failure_threshold_reached_pending",
            "未继续切换 · 已达到失败阈值，等待下一次可执行机会",
          ],
          ["recovery_confirmation_pending", "未切换 · 恢复验证成功 1/2"],
          [
            "model_fallback_excluded",
            "已停止 Fallback · 该模型在此路由不参与 Fallback",
          ],
        ] as const
      ).map(([reason], index) => ({
        ...base,
        attemptIndex: index + 4,
        routingDecision: {
          kind: "stop" as const,
          reason,
          targetRouteId: reason === "activation_failed" ? base.routeId : null,
          targetRouteName:
            reason === "activation_failed" ? "目标备用路由" : null,
        },
      })),
    ];
    ipc.getUsageRequestDetail.mockResolvedValue(detail);
    await renderSettings();

    fireEvent.click(screen.getByRole("button", { name: "用量" }));
    fireEvent.click(await screen.findByRole("button", { name: /^查看请求/ }));

    expect(
      await screen.findByText("重试当前路由（第 2/4 次）"),
    ).toBeInTheDocument();
    expect(
      screen.getByText(`已自动切换至 ${longRouteName}`),
    ).toBeInTheDocument();
    expect(
      screen.getByText("已跳过 不兼容路由 · 该模型在此路由不参与 Fallback"),
    ).toBeInTheDocument();
    expect(
      screen.getByText("恢复验证未通过 · 继续使用 当前路由"),
    ).toBeInTheDocument();
    expect(
      screen.getByText("恢复验证完成 · 已恢复至 优先路由"),
    ).toBeInTheDocument();
    expect(screen.getAllByText(/恢复验证 ·/u)).toHaveLength(2);
    for (const copy of [
      "未切换 · Fallback 已关闭",
      "未切换 · 当前错误不符合切换条件",
      "未切换 · 响应已经开始交付",
      "未切换 · 已到最后一条参与路由",
      "未切换 · 路由配置已经变化",
      "切换至 目标备用路由 失败 · 状态未保存",
      "未切换 · 请求尝试次数已达系统上限",
      "未切换 · 当前路由可归因失败尚未达到 5 次",
      "未继续切换 · 已达到失败阈值，等待下一次可执行机会",
      "未切换 · 恢复验证成功 1/2",
      "已停止 Fallback · 该模型在此路由不参与 Fallback",
    ]) {
      expect(screen.getByText(copy)).toBeInTheDocument();
    }
  });

  it("renders unknown 403 attempt status as access denied without provider text", async () => {
    const detail = structuredClone(previewUsageRequestDetails[0]);
    detail.attempts = [
      {
        ...detail.attempts[0],
        httpStatus: 403,
        errorCategory: "upstream_access_denied",
      },
    ];
    ipc.getUsageRequestDetail.mockResolvedValue(detail);
    await renderSettings();

    fireEvent.click(screen.getByRole("button", { name: "用量" }));
    fireEvent.click(await screen.findByRole("button", { name: /^查看请求/ }));

    expect(await screen.findByText("HTTP 403 · 访问拒绝")).toBeInTheDocument();
    expect(
      screen.queryByText("Current user is in debt."),
    ).not.toBeInTheDocument();
  });

  it("keeps Usage filter edits draft-only until Refresh applies one anchored query", async () => {
    await renderSettings();
    fireEvent.click(screen.getByRole("button", { name: "用量" }));
    await screen.findByText("至少 $0.000028");
    ipc.getUsageHistory.mockClear();

    fireEvent.change(screen.getByLabelText("时间范围"), {
      target: { value: "30d" },
    });
    fireEvent.change(screen.getByLabelText("完成状态"), {
      target: { value: "failed" },
    });
    fireEvent.change(screen.getByRole("combobox", { name: "路由" }), {
      target: { value: previewRouteEdits[0].routeId },
    });
    fireEvent.change(screen.getByLabelText("模型包含"), {
      target: { value: "  GPT-5  " },
    });

    expect(ipc.getUsageHistory).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "刷新" }));
    await waitFor(() => expect(ipc.getUsageHistory).toHaveBeenCalledTimes(1));
    const applied = ipc.getUsageHistory.mock.calls[0][0];
    expect(applied).toMatchObject({
      completionState: "failed",
      routeId: previewRouteEdits[0].routeId,
      modelContains: "GPT-5",
      cursor: null,
      limit: 50,
    });
    expect(applied.finishedAtOrBeforeMs - applied.finishedAtOrAfterMs!).toBe(
      30 * 24 * 60 * 60 * 1_000,
    );
  });

  it("applies today's local calendar bounds to history and statistics", async () => {
    vi.useFakeTimers({ toFake: ["Date"] });
    try {
      vi.setSystemTime(new Date(2026, 7, 19, 9, 0, 0, 0));
      await renderSettings();
      fireEvent.click(screen.getByRole("button", { name: "用量" }));
      await screen.findByText("至少 $0.000028");
      ipc.getUsageHistory.mockClear();

      fireEvent.change(screen.getByLabelText("时间范围"), {
        target: { value: "today" },
      });
      expect(ipc.getUsageHistory).not.toHaveBeenCalled();

      const applyAnchor = new Date(2026, 7, 19, 15, 30, 45, 678).getTime();
      vi.setSystemTime(applyAnchor);
      fireEvent.click(screen.getByRole("button", { name: "刷新" }));
      await waitFor(() => expect(ipc.getUsageHistory).toHaveBeenCalledTimes(1));

      const historyQuery = ipc.getUsageHistory.mock.calls[0][0];
      expect(historyQuery).toMatchObject({
        finishedAtOrAfterMs: new Date(2026, 7, 19).getTime(),
        finishedAtOrBeforeMs: applyAnchor,
      });

      fireEvent.click(screen.getByRole("tab", { name: "用量统计" }));
      await waitFor(() =>
        expect(ipc.getUsageStatistics).toHaveBeenCalledTimes(1),
      );
      expect(ipc.getUsageStatistics.mock.calls[0][0]).toMatchObject({
        finishedAtOrAfterMs: historyQuery.finishedAtOrAfterMs,
        finishedAtOrBeforeMs: historyQuery.finishedAtOrBeforeMs,
      });
    } finally {
      vi.useRealTimers();
    }
  });

  it("uses local calendar operations for yesterday across a DST transition", async () => {
    const originalTimeZone = process.env.TZ;
    process.env.TZ = "America/New_York";
    vi.useFakeTimers({ toFake: ["Date"] });
    try {
      vi.setSystemTime(new Date("2026-03-09T16:00:00.000Z"));
      await renderSettings();
      fireEvent.click(screen.getByRole("button", { name: "用量" }));
      await screen.findByText("至少 $0.000028");
      ipc.getUsageHistory.mockClear();

      fireEvent.change(screen.getByLabelText("时间范围"), {
        target: { value: "yesterday" },
      });
      fireEvent.click(screen.getByRole("button", { name: "刷新" }));
      await waitFor(() => expect(ipc.getUsageHistory).toHaveBeenCalledTimes(1));

      const query = ipc.getUsageHistory.mock.calls[0][0];
      expect(query).toMatchObject({
        finishedAtOrAfterMs: Date.parse("2026-03-08T05:00:00.000Z"),
        finishedAtOrBeforeMs: Date.parse("2026-03-09T03:59:59.999Z"),
      });
      expect(query.finishedAtOrBeforeMs - query.finishedAtOrAfterMs).toBe(
        23 * 60 * 60 * 1_000 - 1,
      );
    } finally {
      vi.useRealTimers();
      if (originalTimeZone === undefined) {
        delete process.env.TZ;
      } else {
        process.env.TZ = originalTimeZone;
      }
    }
  });

  it("shares one applied filter snapshot across records and statistics tabs", async () => {
    await renderSettings();
    fireEvent.click(screen.getByRole("button", { name: "用量" }));
    await screen.findByText("至少 $0.000028");

    const filterForm = screen.getByRole("form", { name: "用量筛选" });
    const tablist = screen.getByRole("tablist", { name: "用量视图" });
    expect(filterForm).toContainElement(tablist);
    expect(tablist.parentElement).toHaveClass("usage-filter-toolbar");
    expect(filterForm).toContainElement(
      screen.getByRole("button", { name: "重置" }),
    );
    expect(filterForm).toContainElement(
      screen.getByRole("button", { name: "刷新" }),
    );
    expect(screen.getByRole("tab", { name: "请求记录" })).toHaveAttribute(
      "aria-selected",
      "true",
    );
    const historyQuery = ipc.getUsageHistory.mock.calls[0][0];
    fireEvent.click(screen.getByRole("tab", { name: "用量统计" }));
    expect(
      await screen.findByRole("img", {
        name: "成功请求数量时间柱状图",
      }),
    ).toBeInTheDocument();
    const allQuery = ipc.getUsageStatistics.mock.calls[0][0];
    expect(allQuery).toMatchObject({
      finishedAtOrAfterMs: historyQuery.finishedAtOrAfterMs,
      finishedAtOrBeforeMs: historyQuery.finishedAtOrBeforeMs,
      routeId: historyQuery.routeId,
      modelContains: historyQuery.modelContains,
      attributionDimension: "model",
      attributionMetric: "requests",
    });

    fireEvent.change(screen.getByLabelText("完成状态"), {
      target: { value: "completed" },
    });
    fireEvent.click(screen.getByRole("button", { name: "刷新" }));
    await waitFor(() =>
      expect(ipc.getUsageStatistics).toHaveBeenCalledTimes(2),
    );
    expect(ipc.getUsageStatistics.mock.calls[1][0]).toMatchObject({
      routeId: allQuery.routeId,
      modelContains: allQuery.modelContains,
    });
  });

  it("keeps request records in place during a background refresh", async () => {
    const { client } = await renderSettings();
    fireEvent.click(screen.getByRole("button", { name: "用量" }));
    const table = await screen.findByRole("table");
    ipc.getUsageHistory.mockReturnValueOnce(new Promise(() => {}));

    act(() => {
      void client.invalidateQueries({ queryKey: queryKeys.usageHistory });
    });

    const refreshButton = screen.getByRole("button", { name: "刷新" });
    await waitFor(() =>
      expect(refreshButton).toHaveAttribute("aria-busy", "true"),
    );
    expect(refreshButton.querySelector("svg")).toHaveClass("spin");
    const status = screen.getByText("正在更新...");
    expect(status).toHaveClass("sr-only");
    expect(status.closest(".usage-filter-actions")).not.toBeNull();
    expect(table).toBeInTheDocument();
    expect(
      document.querySelector(".usage-tab-panel .usage-refreshing"),
    ).toBeNull();
  });

  it("keeps usage statistics in place during a background refresh", async () => {
    const { client } = await renderSettings();
    fireEvent.click(screen.getByRole("button", { name: "用量" }));
    fireEvent.click(screen.getByRole("tab", { name: "用量统计" }));
    const statistics = await screen.findByRole("region", { name: "用量统计" });
    ipc.getUsageStatistics.mockReturnValueOnce(new Promise(() => {}));

    act(() => {
      void client.invalidateQueries({ queryKey: queryKeys.usageStatistics });
    });

    const refreshButton = screen.getByRole("button", { name: "刷新" });
    await waitFor(() =>
      expect(refreshButton).toHaveAttribute("aria-busy", "true"),
    );
    expect(refreshButton.querySelector("svg")).toHaveClass("spin");
    const status = screen.getByText("正在更新...");
    expect(status).toHaveClass("sr-only");
    expect(status.closest(".usage-filter-actions")).not.toBeNull();
    expect(statistics).toBeInTheDocument();
    expect(
      document.querySelector(".usage-tab-panel .usage-refreshing"),
    ).toBeNull();
  });

  it("moves tab focus with arrows and activates only through the native tab button", async () => {
    await renderSettings();
    fireEvent.click(screen.getByRole("button", { name: "用量" }));
    await screen.findByText("至少 $0.000028");

    const recordsTab = screen.getByRole("tab", { name: "请求记录" });
    const statisticsTab = screen.getByRole("tab", { name: "用量统计" });
    recordsTab.focus();
    fireEvent.keyDown(recordsTab, { key: "ArrowRight" });

    expect(statisticsTab).toHaveFocus();
    expect(statisticsTab).toHaveAttribute("aria-selected", "false");
    expect(ipc.getUsageStatistics).not.toHaveBeenCalled();

    fireEvent.click(statisticsTab);
    expect(
      await screen.findByRole("img", {
        name: "成功请求数量时间柱状图",
      }),
    ).toBeInTheDocument();
    expect(statisticsTab).toHaveAttribute("aria-selected", "true");
    expect(
      screen.getByRole("heading", { level: 3, name: "按时间" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("heading", { level: 3, name: "按来源" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("heading", { level: 3, name: "Token 构成" }),
    ).toBeInTheDocument();

    fireEvent.keyDown(statisticsTab, { key: "ArrowLeft" });
    expect(recordsTab).toHaveFocus();
    expect(statisticsTab).toHaveAttribute("aria-selected", "true");
  });

  it("does not issue statistics queries for non-success status filters", async () => {
    await renderSettings();
    fireEvent.click(screen.getByRole("button", { name: "用量" }));
    await screen.findByText("至少 $0.000028");

    for (const status of ["failed", "cancelled", "no_upstream"] as const) {
      fireEvent.change(screen.getByLabelText("完成状态"), {
        target: { value: status },
      });
      fireEvent.click(screen.getByRole("button", { name: "刷新" }));
      if (status === "failed") {
        fireEvent.click(screen.getByRole("tab", { name: "用量统计" }));
      }
      expect(
        await screen.findByText("用量统计只统计已完成请求。"),
      ).toBeInTheDocument();
      expect(ipc.getUsageStatistics).not.toHaveBeenCalled();
      expect(charts.init).not.toHaveBeenCalled();
    }
  });

  it("keeps trend local and refetches model-only source metrics", async () => {
    await renderSettings();
    fireEvent.click(screen.getByRole("button", { name: "用量" }));
    fireEvent.click(screen.getByRole("tab", { name: "用量统计" }));
    await screen.findByRole("img", { name: "成功请求数量时间柱状图" });
    expect(charts.init).toHaveBeenCalledTimes(3);

    const trend = screen.getByRole("group", { name: "趋势指标" });
    fireEvent.click(within(trend).getByLabelText("Token"));
    expect(within(trend).getByLabelText("Token")).toBeChecked();
    expect(ipc.getUsageStatistics).toHaveBeenCalledTimes(1);
    expect(charts.init).toHaveBeenCalledTimes(3);
    expect(
      screen.queryByRole("group", { name: "来源维度" }),
    ).not.toBeInTheDocument();

    const metric = screen.getByRole("group", { name: "来源指标" });
    fireEvent.click(within(metric).getByLabelText("费用"));
    await waitFor(() =>
      expect(ipc.getUsageStatistics).toHaveBeenCalledTimes(2),
    );
    expect(ipc.getUsageStatistics.mock.calls[1][0]).toMatchObject({
      attributionDimension: "model",
      attributionMetric: "cost",
    });
    expect(
      await screen.findByRole("list", { name: "来源图表数据" }),
    ).toHaveTextContent("$0.07");
    expect(screen.getAllByText("0.12M").length).toBeGreaterThan(0);
    expect(screen.getByText("$0.07")).toBeInTheDocument();
    expect(
      screen.getByRole("list", { name: "Token 构成图表数据" }),
    ).toHaveTextContent("未缓存输入: 0.006M");
    expect(charts.init).toHaveBeenCalledTimes(3);
  });

  it("does not initialize charts while statistics are pending", async () => {
    ipc.getUsageStatistics.mockReturnValue(new Promise(() => {}));
    await renderSettings();
    fireEvent.click(screen.getByRole("button", { name: "用量" }));
    fireEvent.click(screen.getByRole("tab", { name: "用量统计" }));

    expect(await screen.findByText("正在读取用量统计...")).toBeInTheDocument();
    expect(charts.init).not.toHaveBeenCalled();
  });

  it("keeps statistics errors safe and does not initialize charts", async () => {
    ipc.getUsageStatistics.mockRejectedValue(
      new Error("sensitive database path"),
    );
    await renderSettings();
    fireEvent.click(screen.getByRole("button", { name: "用量" }));
    fireEvent.click(screen.getByRole("tab", { name: "用量统计" }));

    expect(await screen.findByText("用量统计读取失败。")).toBeInTheDocument();
    expect(
      screen.queryByText(/sensitive database path/),
    ).not.toBeInTheDocument();
    expect(charts.init).not.toHaveBeenCalled();
  });

  it("renders zero statistics without coverage warnings", async () => {
    ipc.getUsageStatistics.mockResolvedValue({
      matchedRequestCount: 0,
      tokens: {
        total: "0",
        uncachedInput: "0",
        cachedInput: "0",
        cacheWriteInput: "0",
        output: "0",
      },
      costPicoUsd: "0",
      granularity: "day",
      trend: [],
      attribution: [],
    });
    await renderSettings();
    fireEvent.click(screen.getByRole("button", { name: "用量" }));
    fireEvent.click(screen.getByRole("tab", { name: "用量统计" }));

    const statistics = await screen.findByRole("region", { name: "用量统计" });
    expect(within(statistics).getByText("$0.00")).toBeInTheDocument();
    expect(within(statistics).getAllByText("0M").length).toBeGreaterThan(0);
    expect(screen.queryByText(/覆盖|估算|下限/)).not.toBeInTheDocument();
    expect(charts.init).not.toHaveBeenCalled();
  });

  it("resets Usage filters and reports count-based page position", async () => {
    await renderSettings();
    fireEvent.click(screen.getByRole("button", { name: "用量" }));
    expect(await screen.findByText("第 1 / 1 页，共 1 条")).toBeInTheDocument();
    ipc.getUsageHistory.mockClear();

    fireEvent.change(screen.getByLabelText("时间范围"), {
      target: { value: "yesterday" },
    });
    fireEvent.change(screen.getByLabelText("完成状态"), {
      target: { value: "failed" },
    });
    fireEvent.change(screen.getByLabelText("模型包含"), {
      target: { value: "gpt" },
    });
    fireEvent.click(screen.getByRole("button", { name: "重置" }));

    await waitFor(() => expect(ipc.getUsageHistory).toHaveBeenCalledTimes(1));
    expect(screen.getByLabelText("时间范围")).toHaveValue("7d");
    expect(screen.getByLabelText("完成状态")).toHaveValue("all");
    expect(screen.getByLabelText("模型包含")).toHaveValue("");
    expect(ipc.getUsageHistory.mock.calls[0][0]).toMatchObject({
      completionState: null,
      routeId: null,
      modelContains: null,
      cursor: null,
    });
  });

  it("renders compact Token and cost cells without list info triggers", async () => {
    await renderSettings();
    fireEvent.click(screen.getByRole("button", { name: "用量" }));
    const partialCost = await screen.findByText("至少 $0.000028");
    const row = partialCost.closest("tr")!;
    expect(within(row).getByText("6")).toBeInTheDocument();
    expect(within(row).getByText("2")).toBeInTheDocument();
    expect(within(row).getByText("4")).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Token 详情" }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "费用详情" }),
    ).not.toBeInTheDocument();
    expect(screen.queryByRole("tooltip")).not.toBeInTheDocument();
    expect(screen.getByText("0.04 s")).toBeInTheDocument();
    expect(screen.getByText("0.12 s")).toBeInTheDocument();
    expect(screen.queryByText("Fast")).not.toBeInTheDocument();
  });

  it("renders typed confirmed and unconfirmed Fast markers without marking mixed cost", async () => {
    const priorityRequest = previewUsageHistoryPage.rows.find(
      ({ requestId }) => requestId === "request-preview-small-cost",
    );
    const unconfirmedRequest = previewUsageHistoryPage.rows.find(
      ({ requestId }) => requestId === "request-preview-unconfirmed-fast",
    );
    const priorityDetail = previewUsageRequestDetails.find(
      ({ request }) => request.requestId === "request-preview-small-cost",
    );
    expect(priorityRequest?.cost).toMatchObject({
      amountPicoUsd: "2000000",
      catalogVersion: "openai-priority-2026-07-28",
      serviceTier: "priority",
      fastStatus: "confirmed",
    });
    expect(priorityDetail).toMatchObject({
      request: priorityRequest,
      requestedServiceTier: "priority",
      actualServiceTier: "priority",
    });
    expect(unconfirmedRequest?.cost).toMatchObject({
      amountPicoUsd: "70316000000",
      catalogVersion: "openai-priority-2026-07-28",
      serviceTier: "priority",
      fastStatus: "unconfirmed",
    });
    ipc.getUsageHistory.mockResolvedValue(
      structuredClone(previewUsageHistoryPage),
    );

    await renderSettings();
    fireEvent.click(screen.getByRole("button", { name: "用量" }));

    const priorityModel = await screen.findByText("gpt-5.6-luna");
    const priorityRow = priorityModel.closest("tr")!;
    expect(within(priorityRow).getByText("$0.000002")).toBeInTheDocument();
    expect(within(priorityRow).getByText("Fast")).toBeInTheDocument();
    expect(screen.getAllByText("Fast")).toHaveLength(1);

    const unconfirmedRow = screen.getByText("gpt-5.6-sol").closest("tr")!;
    expect(within(unconfirmedRow).getByText("$0.070316")).toBeInTheDocument();
    expect(within(unconfirmedRow).getByText("Fast 未确认")).toBeInTheDocument();
    expect(screen.getAllByText("Fast 未确认")).toHaveLength(1);

    const mixedRow = screen.getByText("gpt-5.6-terra").closest("tr")!;
    expect(within(mixedRow).queryByText("Fast")).not.toBeInTheDocument();
    expect(within(mixedRow).queryByText("Fast 未确认")).not.toBeInTheDocument();
  });

  it("compacts large primary Token values inside the fixed column", async () => {
    const page = await ipc.getUsageHistory();
    ipc.getUsageHistory.mockClear();
    ipc.getUsageHistory.mockResolvedValue({
      ...page,
      rows: [
        {
          ...page.rows[0],
          tokens: {
            ...page.rows[0].tokens,
            uncachedInput: 2_469_135,
          },
        },
      ],
    });

    await renderSettings();
    fireEvent.click(screen.getByRole("button", { name: "用量" }));
    expect(await screen.findByText("2.5M")).toBeInTheDocument();
  });

  it("renders relay-aligned completion time, Tokens, Fast pricing, and one first latency", async () => {
    const page = await ipc.getUsageHistory();
    const detail = await ipc.getUsageRequestDetail();
    const fastRequest = {
      ...page.rows[0],
      tokens: {
        input: 60_014,
        uncachedInput: 878,
        output: 40,
        total: 60_054,
        cachedInput: 59_136,
        cacheWriteInput: 0,
      },
      firstOutputLatencyMs: 11_664,
      cost: {
        state: "exact" as const,
        amountPicoUsd: "70316000000",
        currency: "USD",
        catalogVersion: "openai-priority-2026-07-28",
        serviceTier: "priority",
        fastStatus: "unconfirmed" as const,
      },
    };
    ipc.getUsageHistory.mockClear();
    ipc.getUsageRequestDetail.mockClear();
    ipc.getUsageHistory.mockResolvedValue({ ...page, rows: [fastRequest] });
    ipc.getUsageRequestDetail.mockResolvedValue({
      ...detail,
      request: fastRequest,
      requestedServiceTier: "priority",
      actualServiceTier: "default",
      tokens: fastRequest.tokens,
    });

    await renderSettings();
    fireEvent.click(screen.getByRole("button", { name: "用量" }));
    const cost = await screen.findByText("$0.070316");
    const row = cost.closest("tr")!;
    expect(within(row).getByText("878")).toBeInTheDocument();
    expect(within(row).getByText("40")).toBeInTheDocument();
    expect(within(row).getByText("59.1K")).toBeInTheDocument();
    expect(within(row).getByText("Fast 未确认")).toBeInTheDocument();
    expect(within(row).getByText("11.66 s")).toBeInTheDocument();

    fireEvent.click(within(row).getByRole("button", { name: /查看请求/ }));
    expect(await screen.findByText("首字延迟")).toBeInTheDocument();
    expect(screen.queryByText("首输出延迟")).not.toBeInTheDocument();
    expect(screen.getByText("11.66 s")).toBeInTheDocument();
    expect(screen.getByText("开始时间")).toBeInTheDocument();
    expect(screen.getByText("完成时间")).toBeInTheDocument();
    expect(screen.getByText("请求服务层级")).toBeInTheDocument();
    expect(screen.getByText("实际服务层级")).toBeInTheDocument();
  });

  it("renders the single first output metric and preserves null", async () => {
    const page = await ipc.getUsageHistory();
    ipc.getUsageHistory.mockClear();
    ipc.getUsageHistory.mockResolvedValue({
      ...page,
      rows: [
        {
          ...page.rows[0],
          requestId: "request-tool-output",
          firstOutputLatencyMs: 659,
        },
        {
          ...page.rows[0],
          requestId: "request-no-output",
          firstOutputLatencyMs: null,
        },
      ],
      totalRows: 2,
    });

    await renderSettings();
    fireEvent.click(screen.getByRole("button", { name: "用量" }));
    const rows = await screen.findAllByRole("row");

    expect(within(rows[1]).getByText("0.66 s")).toBeInTheDocument();
    expect(within(rows[2]).getByText("-")).toBeInTheDocument();
  });

  it("labels synchronous requests and appends HTTP codes only to non-completed states", async () => {
    const page = await ipc.getUsageHistory();
    ipc.getUsageHistory.mockClear();
    ipc.getUsageHistory.mockResolvedValue({
      ...page,
      rows: [
        {
          ...page.rows[0],
          requestId: "request-failed",
          streaming: false,
          completionState: "failed",
          httpStatus: 502,
        },
        {
          ...page.rows[0],
          requestId: "request-completed",
          streaming: false,
          completionState: "completed",
          httpStatus: 200,
        },
        {
          ...page.rows[0],
          requestId: "request-cancelled",
          completionState: "cancelled",
          httpStatus: null,
        },
      ],
    });

    await renderSettings();
    fireEvent.click(screen.getByRole("button", { name: "用量" }));

    expect(await screen.findByText("失败（502）")).toBeInTheDocument();
    expect(screen.getAllByText("同步")).toHaveLength(2);
    expect(
      within(screen.getByRole("table")).getByText("已完成"),
    ).toBeInTheDocument();
    expect(screen.queryByText("已完成（200）")).not.toBeInTheDocument();
    expect(
      within(screen.getByRole("table")).getByText("已取消"),
    ).toBeInTheDocument();
  });

  it("compacts list cost while preserving full detail precision", async () => {
    const page = await ipc.getUsageHistory();
    const detail = await ipc.getUsageRequestDetail();
    ipc.getUsageHistory.mockClear();
    ipc.getUsageRequestDetail.mockClear();
    ipc.getUsageHistory.mockResolvedValue({
      ...page,
      rows: [
        {
          ...page.rows[0],
          cost: {
            ...page.rows[0].cost,
            amountPicoUsd: "2737900000",
          },
        },
      ],
    });
    ipc.getUsageRequestDetail.mockResolvedValue({
      ...detail,
      request: {
        ...detail.request,
        cost: {
          ...detail.request.cost,
          amountPicoUsd: "2737900000",
        },
      },
    });

    await renderSettings();
    fireEvent.click(screen.getByRole("button", { name: "用量" }));
    const compactCost = await screen.findByText("至少 $0.002737");
    fireEvent.click(
      within(compactCost.closest("tr")!).getByRole("button", {
        name: /查看请求/,
      }),
    );

    expect(await screen.findByText("至少 $0.0027379")).toBeInTheDocument();
  });

  it("compacts long list latency while preserving full detail seconds", async () => {
    const page = await ipc.getUsageHistory();
    const detail = await ipc.getUsageRequestDetail();
    ipc.getUsageHistory.mockClear();
    ipc.getUsageRequestDetail.mockClear();
    ipc.getUsageHistory.mockResolvedValue({
      ...page,
      rows: [
        {
          ...page.rows[0],
          firstOutputLatencyMs: 65_000,
          totalLatencyMs: 3_720_000,
        },
      ],
    });
    ipc.getUsageRequestDetail.mockResolvedValue({
      ...detail,
      request: {
        ...detail.request,
        firstOutputLatencyMs: 65_000,
        totalLatencyMs: 3_720_000,
      },
    });

    await renderSettings();
    fireEvent.click(screen.getByRole("button", { name: "用量" }));
    expect(await screen.findByText("1m 05s")).toBeInTheDocument();
    expect(screen.getByText("1h 02m")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /查看请求/ }));

    expect(await screen.findByText("65.00 s")).toBeInTheDocument();
    expect(screen.getByText("3720.00 s")).toBeInTheDocument();
  });

  it("keeps bounded pagination and detail position, then resets the cursor on apply", async () => {
    ipc.getUsageHistory.mockReset();
    ipc.getUsageHistory
      .mockResolvedValueOnce({
        ...previewUsageHistoryPage,
        rows: previewUsageHistoryPage.rows.slice(0, 1),
        totalRows: 100,
        nextCursor: { finishedAtMs: 99, requestId: "next-page" },
      })
      .mockResolvedValueOnce({
        ...previewUsageHistoryPage,
        rows: previewUsageHistoryPage.rows.slice(1, 2),
        totalRows: 100,
        nextCursor: null,
      })
      .mockResolvedValueOnce({
        ...previewUsageHistoryPage,
        rows: previewUsageHistoryPage.rows.slice(0, 1),
        totalRows: 1,
        nextCursor: null,
      });
    await renderSettings();
    fireEvent.click(screen.getByRole("button", { name: "用量" }));

    expect(
      await screen.findByText("第 1 / 2 页，共 100 条"),
    ).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "下一页" }));
    expect(
      await screen.findByText("第 2 / 2 页，共 100 条"),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "下一页" })).toBeDisabled();
    expect(ipc.getUsageHistory.mock.calls[1][0].cursor).toEqual({
      finishedAtMs: 99,
      requestId: "next-page",
    });

    fireEvent.click(screen.getByRole("button", { name: /查看请求/ }));
    expect(await screen.findByText("请求详情")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "返回请求列表" }));
    expect(screen.getByText("第 2 / 2 页，共 100 条")).toBeInTheDocument();
    expect(ipc.getUsageHistory).toHaveBeenCalledTimes(2);

    fireEvent.change(screen.getByLabelText("模型包含"), {
      target: { value: "terra" },
    });
    fireEvent.click(screen.getByRole("button", { name: "刷新" }));
    await waitFor(() => expect(ipc.getUsageHistory).toHaveBeenCalledTimes(3));
    expect(ipc.getUsageHistory.mock.calls[2][0]).toMatchObject({
      modelContains: "terra",
      cursor: null,
    });
    expect(await screen.findByText("第 1 / 1 页，共 1 条")).toBeInTheDocument();
  });

  it("labels retained route options as deleted beside current routes", async () => {
    ipc.getUsageRouteOptions.mockResolvedValueOnce([
      {
        routeId: previewRouteEdits[0].routeId,
        name: "当前路由",
        retained: false,
      },
      {
        routeId: "retained-route" as RouteId,
        name: "历史路由",
        retained: true,
      },
    ]);
    await renderSettings();
    fireEvent.click(screen.getByRole("button", { name: "用量" }));

    expect(
      await screen.findByRole("option", { name: "当前路由" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("option", { name: "历史路由（已删除）" }),
    ).toBeInTheDocument();
  });

  it("shows a safe route-option failure and retries independently", async () => {
    ipc.getUsageRouteOptions.mockRejectedValueOnce(
      new Error("private database detail"),
    );
    await renderSettings();
    fireEvent.click(screen.getByRole("button", { name: "用量" }));

    expect(await screen.findByText("路由选项读取失败。")).toBeInTheDocument();
    expect(
      screen.queryByText("private database detail"),
    ).not.toBeInTheDocument();
    ipc.getUsageRouteOptions.mockResolvedValueOnce([
      {
        routeId: previewRouteEdits[0].routeId,
        name: "恢复的路由",
        retained: false,
      },
    ]);
    fireEvent.click(screen.getByRole("button", { name: "重试" }));
    expect(
      await screen.findByRole("option", { name: "恢复的路由" }),
    ).toBeInTheDocument();
  });
});
