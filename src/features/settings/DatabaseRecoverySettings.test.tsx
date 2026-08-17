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
import type {
  BootstrapSnapshotDto,
  RecoverySnapshotDto,
} from "../../generated";
import {
  previewFatalDatabaseBootstraps,
  previewFatalRecoverySnapshots,
  previewRecoveryRequiredBootstrap,
  previewRecoveryWithCandidates,
  previewRecoveryWithoutCandidates,
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
  getRouteEdit: vi.fn(),
  getSettingsSnapshot: vi.fn(),
  getUsageHistory: vi.fn(),
  getUsageRequestDetail: vi.fn(),
  getUsageRouteOptions: vi.fn(),
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

function renderDatabaseBootstrap(
  bootstrap: BootstrapSnapshotDto,
  recovery: RecoverySnapshotDto,
) {
  const client = createRouterQueryClient();
  client.setQueryData(queryKeys.bootstrap, bootstrap);
  client.setQueryData(queryKeys.recovery, recovery);
  return {
    client,
    ...render(
      <QueryClientProvider client={client}>
        <SettingsWindow />
      </QueryClientProvider>,
    ),
  };
}

beforeEach(() => {
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

describe("DatabaseRecoverySettings interactions", () => {
  it("selects the newest recovery point and uses a cancel-first restore dialog", async () => {
    renderDatabaseBootstrap(
      previewRecoveryRequiredBootstrap,
      previewRecoveryWithCandidates,
    );

    expect(
      await screen.findByRole("heading", { name: "恢复数据库" }),
    ).toBeInTheDocument();
    const candidates = screen.getAllByRole("radio");
    expect(candidates).toHaveLength(2);
    expect(candidates[0]).toBeChecked();
    fireEvent.click(candidates[1]);
    fireEvent.click(screen.getByRole("button", { name: "恢复所选数据库" }));

    let dialog = screen.getByRole("alertdialog", { name: "恢复所选数据库？" });
    const cancel = within(dialog).getByRole("button", { name: "取消" });
    expect(cancel).toHaveFocus();
    fireEvent.click(cancel);
    expect(ipc.restoreRecoveryPoint).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "恢复所选数据库" }));
    dialog = screen.getByRole("alertdialog", { name: "恢复所选数据库？" });
    fireEvent.click(within(dialog).getByRole("button", { name: "恢复数据库" }));
    await waitFor(() =>
      expect(ipc.restoreRecoveryPoint).toHaveBeenCalledWith(
        previewRecoveryWithCandidates.candidates[1].pointId,
      ),
    );
    expect(JSON.stringify(previewRecoveryWithCandidates)).not.toContain("/");
  });

  it("keeps start-over behind a separate destructive confirmation", async () => {
    renderDatabaseBootstrap(
      previewRecoveryRequiredBootstrap,
      previewRecoveryWithoutCandidates,
    );

    expect(await screen.findByText("没有可用恢复点")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "创建空数据库" }));
    let dialog = screen.getByRole("alertdialog", { name: "创建空数据库？" });
    expect(within(dialog).getByRole("button", { name: "取消" })).toHaveFocus();
    fireEvent.click(within(dialog).getByRole("button", { name: "取消" }));
    expect(ipc.startOverDatabase).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "创建空数据库" }));
    dialog = screen.getByRole("alertdialog", { name: "创建空数据库？" });
    fireEvent.click(
      within(dialog).getByRole("button", { name: "确认创建空数据库" }),
    );
    await waitFor(() => expect(ipc.startOverDatabase).toHaveBeenCalledOnce());
  });

  it("renders typed fatal database actions without destructive recovery controls", async () => {
    const permission = renderDatabaseBootstrap(
      previewFatalDatabaseBootstraps.permission,
      previewFatalRecoverySnapshots.permission,
    );
    expect(
      await screen.findByRole("heading", { name: "数据库无法访问" }),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "创建空数据库" }),
    ).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "重试数据库启动" }));
    await waitFor(() =>
      expect(ipc.retryDatabaseStartup).toHaveBeenCalledOnce(),
    );
    permission.unmount();

    renderDatabaseBootstrap(
      previewFatalDatabaseBootstraps.future_schema,
      previewFatalRecoverySnapshots.future_schema,
    );
    expect(
      await screen.findByRole("heading", { name: "数据库版本过新" }),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "重试数据库启动" }),
    ).not.toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "退出 AI Router" }),
    ).toBeInTheDocument();
  });
});
