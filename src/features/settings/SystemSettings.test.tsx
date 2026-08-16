import { QueryClientProvider } from "@tanstack/react-query";
import {
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { createRouterQueryClient, queryKeys } from "../../api/query";
import type { ServiceTierPolicy, SettingsSnapshotDto } from "../../generated";
import {
  previewMenuSnapshot,
  previewRouteEdits,
  previewSettingsSnapshot,
} from "../../previewFixtures";
import { SettingsWindow } from "./SettingsWindow";

const ipc = vi.hoisted(() => ({
  clearRequestHistory: vi.fn(),
  clearRuntimeLogs: vi.fn(),
  checkRouteReachability: vi.fn(),
  connectCodex: vi.fn(),
  createRecoveryPoint: vi.fn(),
  deleteRoute: vi.fn(),
  getRecoverySnapshot: vi.fn(),
  getApplicationUpdateSnapshot: vi.fn(),
  getRouteEdit: vi.fn(),
  getSettingsSnapshot: vi.fn(),
  getUsageHistory: vi.fn(),
  getUsageRequestDetail: vi.fn(),
  getUsageRouteOptions: vi.fn(),
  moveRoute: vi.fn(),
  restoreCodex: vi.fn(),
  restoreRecoveryPoint: vi.fn(),
  retryDatabaseStartup: vi.fn(),
  saveRoute: vi.fn(),
  setFallbackEnabled: vi.fn(),
  setFallbackParticipantCount: vi.fn(),
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

vi.mock("../../api/ipc", () => ({
  applyProxyPort: vi.fn(),
  checkRouteReachability: ipc.checkRouteReachability,
  clearRequestHistory: ipc.clearRequestHistory,
  clearRuntimeLogs: ipc.clearRuntimeLogs,
  connectCodex: ipc.connectCodex,
  createRecoveryPoint: ipc.createRecoveryPoint,
  deleteRoute: ipc.deleteRoute,
  getBootstrapSnapshot: vi.fn(),
  getMenuSnapshot: vi.fn(),
  getRecoverySnapshot: ipc.getRecoverySnapshot,
  getApplicationUpdateSnapshot: ipc.getApplicationUpdateSnapshot,
  getRouteEdit: ipc.getRouteEdit,
  getSettingsSnapshot: ipc.getSettingsSnapshot,
  getUsageHistory: ipc.getUsageHistory,
  getUsageRequestDetail: ipc.getUsageRequestDetail,
  getUsageRouteOptions: ipc.getUsageRouteOptions,
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
  moveRoute: ipc.moveRoute,
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
  setFallbackParticipantCount: ipc.setFallbackParticipantCount,
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
  ipc.clearRequestHistory.mockReset();
  ipc.clearRuntimeLogs.mockReset();
  ipc.checkRouteReachability.mockReset();
  ipc.connectCodex.mockReset();
  ipc.createRecoveryPoint.mockReset();
  ipc.getRecoverySnapshot.mockReset();
  ipc.getApplicationUpdateSnapshot.mockReset();
  ipc.getRouteEdit.mockReset();
  ipc.getSettingsSnapshot.mockReset();
  ipc.getUsageHistory.mockReset();
  ipc.getUsageRequestDetail.mockReset();
  ipc.getUsageRouteOptions.mockReset();
  ipc.moveRoute.mockReset();
  ipc.saveRoute.mockReset();
  ipc.setFallbackEnabled.mockReset();
  ipc.setFallbackParticipantCount.mockReset();
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
  ipc.moveRoute.mockResolvedValue({ revision: 14 });
  ipc.setFallbackEnabled.mockResolvedValue({ revision: 15 });
  ipc.setFallbackParticipantCount.mockResolvedValue({ revision: 16 });
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

describe("SystemSettings interactions", () => {
  it("validates and atomically applies the two balance query parameters", async () => {
    await renderSettings();
    fireEvent.click(screen.getByRole("button", { name: "系统" }));

    const menuDebounce = screen.getByLabelText("菜单查询防抖");
    const automaticRefresh = screen.getByLabelText("自动查询间隔");
    const apply = screen.getByRole("button", { name: "应用" });
    expect(menuDebounce).toHaveValue(30);
    expect(automaticRefresh).toHaveValue(30);
    expect(screen.getByText("秒")).toBeInTheDocument();
    expect(screen.getByText("分钟")).toBeInTheDocument();
    expect(apply).toBeDisabled();

    fireEvent.change(menuDebounce, { target: { value: "9" } });
    expect(
      screen.getByText("请输入 10 到 600 之间的整数。"),
    ).toBeInTheDocument();
    expect(apply).toBeDisabled();

    fireEvent.change(menuDebounce, { target: { value: "10.5" } });
    expect(screen.getByText("请输入整数。")).toBeInTheDocument();
    fireEvent.change(menuDebounce, { target: { value: "" } });
    expect(screen.getByText("请输入数值。")).toBeInTheDocument();

    fireEvent.change(menuDebounce, { target: { value: "10" } });
    fireEvent.change(automaticRefresh, { target: { value: "1440" } });
    fireEvent.click(apply);

    await waitFor(() =>
      expect(ipc.updateBalanceQuerySettings).toHaveBeenCalledWith({
        menuDebounceSeconds: 10,
        automaticRefreshMinutes: 1440,
      }),
    );
    expect(await screen.findByText("已保存")).toBeInTheDocument();
    expect(apply).toBeDisabled();
  });

  it("renders System as one flat sequence of functional sections", async () => {
    await renderSettings();
    fireEvent.click(screen.getByRole("button", { name: "系统" }));

    expect(
      screen.getByRole("heading", { name: "系统", level: 2 }),
    ).toBeInTheDocument();
    expect(
      screen
        .getAllByRole("heading", { level: 3 })
        .map((heading) => heading.textContent),
    ).toEqual([
      "外观",
      "应用更新",
      "余额查询",
      "数据库恢复",
      "请求记录",
      "运行日志",
    ]);
    expect(screen.getByText("已保护")).toBeInTheDocument();
    const openLogs = screen.getByRole("button", { name: "打开日志目录" });
    const clearLogs = screen.getByRole("button", { name: "清除日志" });
    const logActions = openLogs.closest(".settings-action-group");
    expect(logActions).not.toBeNull();
    expect(clearLogs.closest(".settings-action-group")).toBe(logActions);
    expect(logActions).not.toHaveClass("settings-action-column");
    expect(openLogs.compareDocumentPosition(clearLogs)).toBe(
      Node.DOCUMENT_POSITION_FOLLOWING,
    );
    expect(
      screen.queryByRole("heading", { name: "参数配置" }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("heading", { name: "数据与日志" }),
    ).not.toBeInTheDocument();
  });

  it("shows the database recovery explanation in the help tooltip", async () => {
    await renderSettings();
    fireEvent.click(screen.getByRole("button", { name: "系统" }));

    const trigger = screen.getByRole("button", { name: "数据库恢复说明" });
    expect(screen.queryByRole("tooltip")).not.toBeInTheDocument();

    fireEvent.mouseEnter(trigger);
    expect(screen.getByRole("tooltip")).toHaveTextContent(
      "主数据库无法安全启动时，应用会进入独立恢复流程；仅检测到可用恢复点时才允许恢复。",
    );

    fireEvent.mouseLeave(trigger.parentElement!);
    expect(screen.queryByRole("tooltip")).not.toBeInTheDocument();
  });

  it("keeps a valid parameter draft available after a save failure", async () => {
    ipc.updateBalanceQuerySettings.mockRejectedValue(new Error("injected"));
    await renderSettings();
    fireEvent.click(screen.getByRole("button", { name: "系统" }));
    fireEvent.change(screen.getByLabelText("自动查询间隔"), {
      target: { value: "45" },
    });
    const apply = screen.getByRole("button", { name: "应用" });
    fireEvent.click(apply);

    expect(await screen.findByText("测试失败")).toHaveAttribute(
      "role",
      "alert",
    );
    expect(screen.getByLabelText("自动查询间隔")).toHaveValue(45);
    expect(apply).toBeEnabled();
  });

  it("cancels both destructive data actions without writer work", async () => {
    await renderSettings();
    fireEvent.click(screen.getByRole("button", { name: "系统" }));

    fireEvent.click(screen.getByRole("button", { name: "清除全部请求记录" }));
    fireEvent.click(
      within(
        screen.getByRole("alertdialog", { name: "清除全部请求记录？" }),
      ).getByRole("button", { name: "取消" }),
    );
    fireEvent.click(screen.getByRole("button", { name: "清除日志" }));
    fireEvent.click(
      within(
        screen.getByRole("alertdialog", { name: "清除运行日志？" }),
      ).getByRole("button", {
        name: "取消",
      }),
    );

    expect(ipc.clearRequestHistory).not.toHaveBeenCalled();
    expect(ipc.clearRuntimeLogs).not.toHaveBeenCalled();
  });

  it("renders recovery health and clears manual publication pending and error states", async () => {
    await renderSettings();
    fireEvent.click(screen.getByRole("button", { name: "系统" }));

    expect(screen.getByText("已保护")).toBeInTheDocument();
    expect(screen.getByText("3 个")).toBeInTheDocument();
    ipc.createRecoveryPoint.mockRejectedValueOnce(
      new Error("synthetic failure"),
    );
    fireEvent.click(screen.getByRole("button", { name: "创建恢复点" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("测试失败");
    expect(screen.getByRole("button", { name: "创建恢复点" })).toBeEnabled();

    fireEvent.click(screen.getByRole("button", { name: "创建恢复点" }));
    await waitFor(() =>
      expect(ipc.createRecoveryPoint).toHaveBeenCalledTimes(2),
    );
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });
});
