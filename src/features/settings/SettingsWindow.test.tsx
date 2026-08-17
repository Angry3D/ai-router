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

import type { SettingsNavigationEvent } from "../../api/ipc";
import { createRouterQueryClient, queryKeys } from "../../api/query";
import type {
  BootstrapSnapshotDto,
  RecoverySnapshotDto,
  ServiceTierPolicy,
  SettingsSnapshotDto,
} from "../../generated";
import {
  previewMenuSnapshot,
  previewRecoveryRequiredBootstrap,
  previewRecoveryWithCandidates,
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
  openProjectRepository: vi.fn(),
  hideSettingsWindow: vi.fn(),
  quitApplication: vi.fn(),
  tauriRuntime: false,
  listeners: {
    navigation: undefined as
      ((event: SettingsNavigationEvent) => void) | undefined,
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
  openProjectRepository: ipc.openProjectRepository,
  hideSettingsWindow: ipc.hideSettingsWindow,
  isTauriRuntime: () => ipc.tauriRuntime,
  listenSettingsCloseRequested: vi.fn(async (listener: () => void) => {
    ipc.listeners.close = listener;
    return vi.fn();
  }),
  listenSettingsNavigation: vi.fn(
    async (listener: (event: SettingsNavigationEvent) => void) => {
      ipc.listeners.navigation = listener;
      return vi.fn();
    },
  ),
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
  ipc.getApplicationUpdateSnapshot.mockReset();
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
  ipc.openProjectRepository.mockReset();
  ipc.hideSettingsWindow.mockReset();
  ipc.quitApplication.mockReset();
  ipc.listeners.navigation = undefined;
  ipc.listeners.close = undefined;
  ipc.tauriRuntime = false;
  ipc.getRunningAppVersion.mockResolvedValue("0.1.1");
  ipc.openProjectRepository.mockResolvedValue(undefined);
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

describe("SettingsWindow interactions", () => {
  it("shows the running app version in the persistent sidebar", async () => {
    await renderSettings();

    expect(await screen.findByText("版本 0.1.1")).toBeInTheDocument();
  });

  it("guards the repository opener while pending and recovers from rejection", async () => {
    let resolveOpen!: () => void;
    ipc.openProjectRepository.mockImplementationOnce(
      () =>
        new Promise<void>((resolve) => {
          resolveOpen = resolve;
        }),
    );
    await renderSettings();

    const repositoryLink = screen.getByRole("button", {
      name: "打开 GitHub 项目",
    });
    fireEvent.click(repositoryLink);
    fireEvent.click(repositoryLink);
    expect(ipc.openProjectRepository).toHaveBeenCalledOnce();
    expect(repositoryLink).toBeDisabled();

    resolveOpen();
    await waitFor(() => expect(repositoryLink).toBeEnabled());

    ipc.openProjectRepository.mockRejectedValueOnce(
      new Error("opener unavailable"),
    );
    fireEvent.click(repositoryLink);
    await waitFor(() => expect(repositoryLink).toBeEnabled());
    expect(ipc.openProjectRepository).toHaveBeenCalledTimes(2);
  });

  it("keeps every shared page heading inside the draggable title band", async () => {
    await renderSettings();

    for (const page of ["用量", "Codex", "系统"]) {
      fireEvent.click(screen.getByRole("button", { name: page }));
      const title = screen.getByRole("heading", { name: page, level: 2 });
      expect(title).toHaveAttribute("data-tauri-drag-region");
      expect(title.parentElement).toHaveAttribute("data-tauri-drag-region");
    }
  });

  it("preserves dirty input when continuing and changes section only after discard", async () => {
    await renderSettings();
    fireEvent.change(screen.getByLabelText("路由名称"), {
      target: { value: "未保存的新名称" },
    });
    expect(
      screen.getByRole("heading", { name: "自定义模型", level: 3 })
        .parentElement,
    ).not.toHaveTextContent("未保存");
    fireEvent.click(screen.getByRole("button", { name: "Codex" }));

    const dialog = screen.getByRole("alertdialog", {
      name: "放弃未保存的修改？",
    });
    fireEvent.click(within(dialog).getByRole("button", { name: "继续编辑" }));
    expect(screen.getByLabelText("路由名称")).toHaveValue("未保存的新名称");
    expect(
      screen.queryByRole("heading", { name: "Codex", level: 2 }),
    ).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Codex" }));
    fireEvent.click(
      within(
        screen.getByRole("alertdialog", { name: "放弃未保存的修改？" }),
      ).getByRole("button", { name: "放弃修改" }),
    );
    expect(
      screen.getByRole("heading", { name: "Codex", level: 2 }),
    ).toBeInTheDocument();
  });

  it("guards native new-route navigation with the controlled discard session", async () => {
    await renderSettings();
    fireEvent.change(screen.getByLabelText("路由名称"), {
      target: { value: "未保存的新名称" },
    });

    act(() => {
      ipc.listeners.navigation?.({ section: "routes", createNewRoute: true });
    });
    let dialog = screen.getByRole("alertdialog", {
      name: "放弃未保存的修改？",
    });
    fireEvent.click(within(dialog).getByRole("button", { name: "继续编辑" }));
    expect(screen.getByLabelText("路由名称")).toHaveValue("未保存的新名称");
    expect(
      screen.queryByRole("heading", { name: "新建路由" }),
    ).not.toBeInTheDocument();

    act(() => {
      ipc.listeners.navigation?.({ section: "routes", createNewRoute: true });
    });
    dialog = screen.getByRole("alertdialog", {
      name: "放弃未保存的修改？",
    });
    fireEvent.click(within(dialog).getByRole("button", { name: "放弃修改" }));

    expect(
      screen.getByRole("heading", { name: "新建路由" }),
    ).toBeInTheDocument();
    expect(screen.getByLabelText("路由名称")).toHaveValue("");
  });

  it("does not render deferred or excluded settings controls", async () => {
    await renderSettings();
    for (const excluded of [
      "Fallback",
      "数据库备份",
      "统计图表",
      "断开 Codex",
      "启动代理",
    ]) {
      expect(screen.queryByText(excluded)).not.toBeInTheDocument();
    }
  });

  it("rehydrates the route-owned model draft after confirming a native close discard", async () => {
    await renderSettings();
    fireEvent.change(screen.getByLabelText("模型 ID 1"), {
      target: { value: "unsaved-model" },
    });

    ipc.listeners.close?.();
    const dialog = await screen.findByRole("alertdialog", {
      name: "放弃未保存的修改？",
    });
    fireEvent.click(within(dialog).getByRole("button", { name: "放弃修改" }));

    expect(await screen.findByLabelText("模型 ID 1")).toHaveValue(
      "relay-custom-model",
    );
    expect(ipc.hideSettingsWindow).toHaveBeenCalledOnce();
  });

  it("does not request database-backed settings while recovery is required", async () => {
    ipc.tauriRuntime = true;
    renderDatabaseBootstrap(
      previewRecoveryRequiredBootstrap,
      previewRecoveryWithCandidates,
    );

    expect(
      await screen.findByRole("heading", { name: "恢复数据库" }),
    ).toBeInTheDocument();
    expect(ipc.getSettingsSnapshot).not.toHaveBeenCalled();
  });
});
