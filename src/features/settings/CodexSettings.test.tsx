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
  confirmCodexImagesMcpRepair: vi.fn(),
  confirmResetCodexRecoveryToBaseline: vi.fn(),
  confirmUpdateCodexRecovery: vi.fn(),
  connectCodex: vi.fn(),
  createRecoveryPoint: vi.fn(),
  deleteRoute: vi.fn(),
  getRecoverySnapshot: vi.fn(),
  getRouteEdit: vi.fn(),
  getSettingsSnapshot: vi.fn(),
  getUsageHistory: vi.fn(),
  getUsageRequestDetail: vi.fn(),
  getUsageRouteOptions: vi.fn(),
  moveRoute: vi.fn(),
  openCodexConfig: vi.fn(),
  previewCodexImagesMcpRepair: vi.fn(),
  previewResetCodexRecoveryToBaseline: vi.fn(),
  previewUpdateCodexRecovery: vi.fn(),
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
  confirmCodexImagesMcpRepair: ipc.confirmCodexImagesMcpRepair,
  confirmResetCodexRecoveryToBaseline: ipc.confirmResetCodexRecoveryToBaseline,
  confirmUpdateCodexRecovery: ipc.confirmUpdateCodexRecovery,
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
  moveRoute: ipc.moveRoute,
  normalizeIpcError: () => ({
    code: "test",
    message: "测试失败",
    retryable: false,
    field: null,
  }),
  openCodexConfig: ipc.openCodexConfig,
  openRuntimeLogDirectory: vi.fn(),
  previewCodexImagesMcpRepair: ipc.previewCodexImagesMcpRepair,
  previewResetCodexRecoveryToBaseline: ipc.previewResetCodexRecoveryToBaseline,
  previewUpdateCodexRecovery: ipc.previewUpdateCodexRecovery,
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
  ipc.getSettingsSnapshot.mockResolvedValue(settings);
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
  ipc.confirmCodexImagesMcpRepair.mockReset();
  ipc.confirmResetCodexRecoveryToBaseline.mockReset();
  ipc.confirmUpdateCodexRecovery.mockReset();
  ipc.connectCodex.mockReset();
  ipc.createRecoveryPoint.mockReset();
  ipc.getRecoverySnapshot.mockReset();
  ipc.getRouteEdit.mockReset();
  ipc.getSettingsSnapshot.mockReset();
  ipc.getUsageHistory.mockReset();
  ipc.getUsageRequestDetail.mockReset();
  ipc.getUsageRouteOptions.mockReset();
  ipc.moveRoute.mockReset();
  ipc.openCodexConfig.mockReset();
  ipc.previewCodexImagesMcpRepair.mockReset();
  ipc.previewResetCodexRecoveryToBaseline.mockReset();
  ipc.previewUpdateCodexRecovery.mockReset();
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
  ipc.previewCodexImagesMcpRepair.mockResolvedValue({
    permit: "images-repair-permit",
  });
  ipc.confirmCodexImagesMcpRepair.mockResolvedValue({
    changed: true,
    status: "connected",
  });
  ipc.previewUpdateCodexRecovery.mockResolvedValue({
    permit: "recovery-update-permit",
    currentExists: true,
    currentUnixMode: 0o600,
    recoveryTargetExists: true,
    bytesChanged: true,
    recoveryUpdatedAtMs: Date.now() - 86_400_000,
  });
  ipc.confirmUpdateCodexRecovery.mockResolvedValue({
    changed: true,
    status: "not_connected",
  });
  ipc.previewResetCodexRecoveryToBaseline.mockResolvedValue({
    permit: "recovery-reset-permit",
    currentExists: true,
    originalExists: true,
    recoveryTargetExists: true,
  });
  ipc.confirmResetCodexRecoveryToBaseline.mockResolvedValue({
    changed: true,
    status: "not_connected",
  });
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

describe("CodexSettings interactions", () => {
  it("cancels connecting Codex without an active route with zero config writes", async () => {
    await renderSettings({
      settings: { activeRouteId: null, codexStatus: "not_connected" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Codex" }));
    fireEvent.click(screen.getByRole("button", { name: "一键连接 Codex" }));

    const dialog = screen.getByRole("alertdialog", {
      name: "当前没有活动路由",
    });
    expect(dialog).toHaveTextContent(
      "连接后，新请求会失败，直到你添加或选择路由。",
    );
    fireEvent.click(within(dialog).getByRole("button", { name: "取消" }));
    expect(ipc.connectCodex).not.toHaveBeenCalled();
  });

  it("previews the original-backup reset and cancellation is side-effect free", async () => {
    await renderSettings({ settings: { codexStatus: "not_connected" } });
    fireEvent.click(screen.getByRole("button", { name: "Codex" }));
    fireEvent.click(screen.getByRole("button", { name: "恢复首次连接前状态" }));

    const dialog = await screen.findByRole("alertdialog", {
      name: "恢复首次连接前状态？",
    });
    expect(ipc.previewResetCodexRecoveryToBaseline).toHaveBeenCalledTimes(1);
    expect(dialog).toHaveTextContent(
      "当前 config.toml 和断开恢复配置都会被原始备份替换",
    );
    fireEvent.click(within(dialog).getByRole("button", { name: "取消" }));
    expect(ipc.confirmResetCodexRecoveryToBaseline).not.toHaveBeenCalled();
  });

  it("previews and confirms a disconnected recovery update", async () => {
    const { client } = await renderSettings({
      settings: { codexStatus: "not_connected" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Codex" }));
    fireEvent.click(screen.getByRole("button", { name: "更新恢复配置" }));

    const dialog = await screen.findByRole("alertdialog", {
      name: "更新断开恢复配置？",
    });
    expect(dialog).toHaveTextContent("当前文件");
    expect(dialog).toHaveTextContent("内容已更改");
    fireEvent.click(
      within(dialog).getByRole("button", { name: "更新恢复配置" }),
    );
    expect(ipc.confirmUpdateCodexRecovery).toHaveBeenCalledWith(
      "recovery-update-permit",
    );
    await waitFor(() =>
      expect(client.getQueryState(queryKeys.settings)?.isInvalidated).toBe(
        true,
      ),
    );
  });

  it("renders a saved absent-file target distinctly in the update preview", async () => {
    ipc.previewUpdateCodexRecovery.mockResolvedValueOnce({
      permit: "absent-recovery-update",
      currentExists: false,
      currentUnixMode: null,
      recoveryTargetExists: false,
      bytesChanged: false,
      recoveryUpdatedAtMs: Date.now() - 86_400_000,
    });
    await renderSettings({ settings: { codexStatus: "not_connected" } });
    fireEvent.click(screen.getByRole("button", { name: "Codex" }));
    fireEvent.click(screen.getByRole("button", { name: "更新恢复配置" }));

    const dialog = await screen.findByRole("alertdialog", {
      name: "更新断开恢复配置？",
    });
    expect(dialog).toHaveTextContent("现有恢复目标");
    expect(dialog).toHaveTextContent("断开后删除 config.toml");
    expect(dialog).not.toHaveTextContent("尚未创建");
  });

  it("disables recovery mutations while connected", async () => {
    await renderSettings({ settings: { codexStatus: "connected" } });
    fireEvent.click(screen.getByRole("button", { name: "Codex" }));

    expect(screen.getByRole("button", { name: "更新恢复配置" })).toBeDisabled();
    expect(
      screen.getByRole("button", { name: "恢复首次连接前状态" }),
    ).toBeDisabled();
    expect(
      screen.getByText("断开 Codex 后才能从当前 config.toml 更新。"),
    ).toBeInTheDocument();
  });

  it.each(["light", "dark"] as const)(
    "keeps recovery status and danger actions on semantic roles in %s theme",
    async (theme) => {
      document.documentElement.dataset.theme = theme;
      await renderSettings({ settings: { codexStatus: "not_connected" } });
      fireEvent.click(screen.getByRole("button", { name: "Codex" }));

      expect(screen.getByText("可用")).toHaveClass("settings-status-success");
      expect(
        screen.getByRole("button", { name: "恢复首次连接前状态" }),
      ).toHaveClass("settings-button-danger");
      expect(screen.getByRole("button", { name: "更新恢复配置" })).toHaveClass(
        "settings-button-primary",
      );
    },
  );

  it("renders an absent recovery target explicitly", async () => {
    await renderSettings({
      settings: {
        codexStatus: "not_connected",
        recoveryConfig: {
          ...previewSettingsSnapshot.recoveryConfig,
          originalExists: false,
        },
      },
    });
    fireEvent.click(screen.getByRole("button", { name: "Codex" }));

    expect(screen.getByText("断开后删除 config.toml")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "更新恢复配置" })).toBeEnabled();
  });

  it("disables recovery actions when the immutable original backup is missing", async () => {
    await renderSettings({
      settings: {
        codexStatus: "not_connected",
        originalBackup: {
          exists: false,
          originalExists: null,
          capturedAtMs: null,
        },
        recoveryConfig: {
          exists: false,
          originalExists: null,
          updatedAtMs: null,
        },
      },
    });
    fireEvent.click(screen.getByRole("button", { name: "Codex" }));

    expect(screen.getByText("不可用")).toHaveClass("settings-status-warning");
    expect(screen.getByRole("button", { name: "更新恢复配置" })).toBeDisabled();
    expect(
      screen.getByRole("button", { name: "恢复首次连接前状态" }),
    ).toBeDisabled();
    expect(
      screen.getByText("首次连接前没有可用的原始备份。"),
    ).toBeInTheDocument();
  });

  it("confirms reset with its one-use permit", async () => {
    await renderSettings({ settings: { codexStatus: "not_connected" } });
    fireEvent.click(screen.getByRole("button", { name: "Codex" }));
    fireEvent.click(screen.getByRole("button", { name: "恢复首次连接前状态" }));
    const dialog = await screen.findByRole("alertdialog", {
      name: "恢复首次连接前状态？",
    });
    expect(dialog).toHaveTextContent("原始配置文件");
    fireEvent.click(
      within(dialog).getByRole("button", { name: "恢复原始备份" }),
    );

    expect(ipc.confirmResetCodexRecoveryToBaseline).toHaveBeenCalledWith(
      "recovery-reset-permit",
    );
  });

  it("keeps a partial reset failure inline and available for retry", async () => {
    ipc.confirmResetCodexRecoveryToBaseline.mockRejectedValueOnce(
      new Error("partial reset"),
    );
    await renderSettings({ settings: { codexStatus: "not_connected" } });
    fireEvent.click(screen.getByRole("button", { name: "Codex" }));
    fireEvent.click(screen.getByRole("button", { name: "恢复首次连接前状态" }));
    const dialog = await screen.findByRole("alertdialog", {
      name: "恢复首次连接前状态？",
    });
    fireEvent.click(
      within(dialog).getByRole("button", { name: "恢复原始备份" }),
    );

    expect(await screen.findByRole("alert")).toHaveTextContent("测试失败");
    expect(
      screen.getByRole("button", { name: "恢复首次连接前状态" }),
    ).toBeEnabled();
    expect(screen.getByText("可用")).toBeInTheDocument();
  });

  it("locks recovery actions while an update preview is pending", async () => {
    let finishPreview:
      | ((preview: {
          permit: string;
          currentExists: boolean;
          currentUnixMode: number | null;
          recoveryTargetExists: boolean;
          bytesChanged: boolean;
          recoveryUpdatedAtMs: number | null;
        }) => void)
      | undefined;
    ipc.previewUpdateCodexRecovery.mockReturnValueOnce(
      new Promise((resolve) => {
        finishPreview = resolve;
      }),
    );
    await renderSettings({ settings: { codexStatus: "not_connected" } });
    fireEvent.click(screen.getByRole("button", { name: "Codex" }));
    fireEvent.click(screen.getByRole("button", { name: "更新恢复配置" }));

    expect(screen.getByRole("button", { name: "更新恢复配置" })).toBeDisabled();
    expect(
      screen.getByRole("button", { name: "恢复首次连接前状态" }),
    ).toBeDisabled();
    expect(
      screen.getByRole("button", { name: "打开 config.toml" }),
    ).toBeDisabled();

    finishPreview?.({
      permit: "pending-recovery-update",
      currentExists: true,
      currentUnixMode: 0o600,
      recoveryTargetExists: true,
      bytesChanged: false,
      recoveryUpdatedAtMs: null,
    });
    expect(
      await screen.findByRole("alertdialog", {
        name: "更新断开恢复配置？",
      }),
    ).toHaveTextContent("内容未更改");
  });

  it("shows a normalized stale update error and leaves the retry action enabled", async () => {
    ipc.confirmUpdateCodexRecovery.mockRejectedValueOnce(
      new Error("stale permit"),
    );
    await renderSettings({ settings: { codexStatus: "not_connected" } });
    fireEvent.click(screen.getByRole("button", { name: "Codex" }));
    fireEvent.click(screen.getByRole("button", { name: "更新恢复配置" }));
    const dialog = await screen.findByRole("alertdialog", {
      name: "更新断开恢复配置？",
    });
    fireEvent.click(
      within(dialog).getByRole("button", { name: "更新恢复配置" }),
    );

    expect(await screen.findByRole("alert")).toHaveTextContent("测试失败");
    expect(screen.getByRole("button", { name: "更新恢复配置" })).toBeEnabled();
    expect(screen.getByText("可用")).toBeInTheDocument();
  });

  it("shows port conflict and invalid Codex configuration as independent states", async () => {
    await renderSettings({
      proxyStatus: "port_conflict",
      settings: { codexStatus: "invalid" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Codex" }));

    expect(screen.getByText("端口冲突")).toBeInTheDocument();
    expect(screen.getByText("配置无效")).toBeInTheDocument();
  });

  it("keeps a baseline-owned image MCP name conflict manual-only", async () => {
    await renderSettings({
      settings: { codexStatus: "images_mcp_name_conflict" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Codex" }));

    expect(screen.getByText("图片 MCP 名称冲突")).toHaveClass(
      "settings-status-danger",
    );
    expect(
      screen.getByText("首次连接前已存在同名配置，请先重命名或移除。"),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "修复图片配置" }),
    ).not.toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "打开 config.toml" }),
    ).toBeEnabled();
  });

  it("previews image MCP repair and cancellation performs no write", async () => {
    await renderSettings({
      settings: { codexStatus: "images_mcp_projection_conflict" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Codex" }));

    expect(screen.getByText("图片 MCP 配置已修改")).toHaveClass(
      "settings-status-danger",
    );
    expect(
      screen.getByText("图片工具配置已被修改，自动重连无法继续。"),
    ).toBeInTheDocument();
    const actions = screen
      .getByRole("button", { name: "修复图片配置" })
      .closest(".settings-action-group");
    expect(
      within(actions as HTMLElement)
        .getAllByRole("button")
        .map((button) => button.textContent?.trim()),
    ).toEqual(["修复图片配置", "打开 config.toml"]);

    fireEvent.click(screen.getByRole("button", { name: "修复图片配置" }));
    const dialog = await screen.findByRole("alertdialog", {
      name: "替换 ai_router_images 配置？",
    });
    expect(ipc.previewCodexImagesMcpRepair).toHaveBeenCalledTimes(1);
    expect(dialog).toHaveTextContent(
      "只会替换图片工具配置，其他 Codex 配置不会改动。",
    );
    expect(within(dialog).getByRole("button", { name: "取消" })).toHaveFocus();
    fireEvent.click(within(dialog).getByRole("button", { name: "取消" }));
    expect(ipc.confirmCodexImagesMcpRepair).not.toHaveBeenCalled();
  });

  it("shows repair busy state while the preview is pending", async () => {
    let finishPreview: ((preview: { permit: string }) => void) | undefined;
    ipc.previewCodexImagesMcpRepair.mockReturnValueOnce(
      new Promise((resolve) => {
        finishPreview = resolve;
      }),
    );
    await renderSettings({
      settings: { codexStatus: "images_mcp_projection_conflict" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Codex" }));
    fireEvent.click(screen.getByRole("button", { name: "修复图片配置" }));

    const repair = screen.getByRole("button", { name: "修复图片配置" });
    expect(repair).toBeDisabled();
    expect(repair.querySelector(".spin")).not.toBeNull();
    expect(
      screen.getByRole("button", { name: "打开 config.toml" }),
    ).toBeDisabled();

    finishPreview?.({ permit: "pending-preview-permit" });
    expect(
      await screen.findByRole("alertdialog", {
        name: "替换 ai_router_images 配置？",
      }),
    ).toBeInTheDocument();
  });

  it("locks conflict actions while confirmed repair is pending and refreshes success", async () => {
    let finishRepair:
      ((result: { changed: boolean; status: "connected" }) => void) | undefined;
    ipc.confirmCodexImagesMcpRepair.mockReturnValueOnce(
      new Promise((resolve) => {
        finishRepair = resolve;
      }),
    );
    ipc.tauriRuntime = true;
    await renderSettings({
      settings: { codexStatus: "images_mcp_projection_conflict" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Codex" }));
    fireEvent.click(screen.getByRole("button", { name: "修复图片配置" }));
    const dialog = await screen.findByRole("alertdialog", {
      name: "替换 ai_router_images 配置？",
    });
    ipc.getSettingsSnapshot.mockResolvedValue({
      ...previewSettingsSnapshot,
      codexStatus: "connected",
    });
    fireEvent.click(
      within(dialog).getByRole("button", { name: "替换并重新连接" }),
    );

    expect(ipc.confirmCodexImagesMcpRepair).toHaveBeenCalledWith(
      "images-repair-permit",
    );
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "修复图片配置" })).toBeDisabled();
    expect(
      screen.getByRole("button", { name: "打开 config.toml" }),
    ).toBeDisabled();

    finishRepair?.({ changed: true, status: "connected" });
    expect(await screen.findByText("已连接")).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "修复图片配置" }),
    ).not.toBeInTheDocument();
  });

  it("retains the projection conflict and shows normalized repair failure", async () => {
    ipc.confirmCodexImagesMcpRepair.mockRejectedValueOnce(
      new Error("stale permit"),
    );
    await renderSettings({
      settings: { codexStatus: "images_mcp_projection_conflict" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Codex" }));
    fireEvent.click(screen.getByRole("button", { name: "修复图片配置" }));
    const dialog = await screen.findByRole("alertdialog", {
      name: "替换 ai_router_images 配置？",
    });
    fireEvent.click(
      within(dialog).getByRole("button", { name: "替换并重新连接" }),
    );

    expect(await screen.findByRole("alert")).toHaveTextContent("测试失败");
    expect(screen.getByText("图片 MCP 配置已修改")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "修复图片配置" })).toBeEnabled();
  });

  it("defaults image generation off and offers every route when enabled", async () => {
    await renderSettings({
      settings: {
        imagesGeneration: { enabled: false, routeId: null, timeoutSecs: 600 },
      },
    });
    fireEvent.click(screen.getByRole("button", { name: "Codex" }));

    const toggle = screen.getByRole("switch", { name: "启用" });
    const selector = screen.getByRole("combobox", { name: "图片路由" });
    const timeout = screen.getByRole("spinbutton", { name: "生成等待上限" });
    const apply = screen.getByRole("button", { name: "应用" });
    expect(screen.getByText("Codex 图片工具")).toBeInTheDocument();
    expect(screen.queryByText("启用图片生成")).not.toBeInTheDocument();
    expect(toggle).not.toBeChecked();
    expect(selector).toBeDisabled();
    expect(selector).toHaveClass("images-generation-route-select");
    expect(timeout).toBeDisabled();
    expect(timeout.parentElement).toHaveClass(
      "images-generation-timeout-control",
    );
    expect(timeout).toHaveValue(600);
    expect(apply).toBeDisabled();
    expect(screen.getByText("未启用")).toBeInTheDocument();

    fireEvent.click(toggle);
    expect(selector).toBeEnabled();
    expect(timeout).toBeEnabled();
    expect(apply).toBeDisabled();
    expect(
      within(selector)
        .getAllByRole("option")
        .map((option) => option.textContent),
    ).toEqual([
      "选择路由",
      "AI INPUT 工作账号",
      "AI INPUT 个人账号",
      "Ciii 主用",
      "Ciii 测试",
    ]);
  });

  it("shows image model and size constraints in the help tooltip", async () => {
    await renderSettings();
    fireEvent.click(screen.getByRole("button", { name: "Codex" }));

    const trigger = screen.getByRole("button", { name: "图片生成说明" });
    expect(screen.queryByRole("tooltip")).not.toBeInTheDocument();

    fireEvent.mouseEnter(trigger);
    const tooltip = screen.getByRole("tooltip");
    expect(tooltip).toHaveTextContent("gpt-image-2");
    expect(tooltip).toHaveTextContent("最长边小于 3,840px");
    expect(tooltip).toHaveTextContent("2048x1152");
    expect(trigger).toHaveAttribute("aria-describedby");

    fireEvent.mouseLeave(trigger.parentElement!);
    expect(screen.queryByRole("tooltip")).not.toBeInTheDocument();

    fireEvent.focus(trigger);
    expect(screen.getByRole("tooltip")).toBeInTheDocument();
    fireEvent.blur(trigger);
    expect(screen.queryByRole("tooltip")).not.toBeInTheDocument();
  });

  it("applies any existing route and timeout while locking every image control", async () => {
    let resolveUpdate: ((value: { revision: number }) => void) | undefined;
    ipc.updateImagesGenerationSettings.mockReturnValueOnce(
      new Promise((resolve) => {
        resolveUpdate = resolve;
      }),
    );
    await renderSettings({
      settings: {
        imagesGeneration: { enabled: false, routeId: null, timeoutSecs: 600 },
      },
    });
    fireEvent.click(screen.getByRole("button", { name: "Codex" }));

    const toggle = screen.getByRole("switch", { name: "启用" });
    const selector = screen.getByRole("combobox", { name: "图片路由" });
    const timeout = screen.getByRole("spinbutton", { name: "生成等待上限" });
    const apply = screen.getByRole("button", { name: "应用" });
    const routeId = previewSettingsSnapshot.routes[2].routeId;
    fireEvent.click(toggle);
    fireEvent.change(selector, { target: { value: routeId } });
    fireEvent.change(timeout, { target: { value: "900" } });
    fireEvent.click(apply);

    expect(ipc.updateImagesGenerationSettings).toHaveBeenCalledWith({
      enabled: true,
      routeId,
      timeoutSecs: 900,
    });
    expect(toggle).toBeDisabled();
    expect(selector).toBeDisabled();
    expect(timeout).toBeDisabled();
    expect(apply).toBeDisabled();

    resolveUpdate?.({ revision: 18 });
    expect(await screen.findByText("已保存")).toBeInTheDocument();
    await waitFor(() => expect(toggle).toBeEnabled());
  });

  it("keeps a missing-route integration enabled and can disable it safely", async () => {
    await renderSettings({
      settings: {
        imagesGeneration: {
          enabled: true,
          routeId: null,
          timeoutSecs: 600,
        },
      },
    });
    fireEvent.click(screen.getByRole("button", { name: "Codex" }));

    const toggle = screen.getByRole("switch", { name: "启用" });
    expect(toggle).toBeChecked();
    expect(screen.getByText("需要选择路由")).toBeInTheDocument();
    expect(screen.getByRole("combobox", { name: "图片路由" })).toHaveValue("");

    fireEvent.click(toggle);
    fireEvent.click(screen.getByRole("button", { name: "应用" }));
    await waitFor(() =>
      expect(ipc.updateImagesGenerationSettings).toHaveBeenCalledWith({
        enabled: false,
        routeId: null,
        timeoutSecs: 600,
      }),
    );
  });

  it("keeps a valid image draft available after an apply failure", async () => {
    ipc.updateImagesGenerationSettings.mockRejectedValueOnce(
      new Error("injected"),
    );
    await renderSettings({
      settings: {
        imagesGeneration: { enabled: false, routeId: null, timeoutSecs: 600 },
      },
    });
    fireEvent.click(screen.getByRole("button", { name: "Codex" }));

    const toggle = screen.getByRole("switch", { name: "启用" });
    const selector = screen.getByRole("combobox", { name: "图片路由" });
    const timeout = screen.getByRole("spinbutton", { name: "生成等待上限" });
    const routeId = previewSettingsSnapshot.routes[0].routeId;
    fireEvent.click(toggle);
    fireEvent.change(selector, { target: { value: routeId } });
    fireEvent.change(timeout, { target: { value: "1200" } });
    fireEvent.click(screen.getByRole("button", { name: "应用" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("测试失败");
    expect(toggle).toBeChecked();
    expect(selector).toHaveValue(routeId);
    expect(timeout).toHaveValue(1200);
    expect(screen.getByRole("button", { name: "应用" })).toBeEnabled();
  });

  it("rejects an out-of-range timeout locally and keeps the draft editable", async () => {
    await renderSettings();
    fireEvent.click(screen.getByRole("button", { name: "Codex" }));

    const timeout = screen.getByRole("spinbutton", { name: "生成等待上限" });
    fireEvent.change(timeout, { target: { value: "599" } });

    expect(timeout).toHaveAttribute("aria-invalid", "true");
    expect(screen.getByRole("alert")).toHaveTextContent(
      "请输入 600 至 3600 的整数。",
    );
    expect(screen.getByRole("button", { name: "应用" })).toBeDisabled();
    expect(ipc.updateImagesGenerationSettings).not.toHaveBeenCalled();
  });

  it("can disable image generation after the timeout draft becomes invalid", async () => {
    await renderSettings();
    fireEvent.click(screen.getByRole("button", { name: "Codex" }));

    const toggle = screen.getByRole("switch", { name: "启用" });
    const timeout = screen.getByRole("spinbutton", {
      name: "生成等待上限",
    });
    fireEvent.change(timeout, { target: { value: "599" } });
    fireEvent.click(toggle);

    expect(timeout).toBeDisabled();
    expect(timeout).toHaveValue(
      previewSettingsSnapshot.imagesGeneration.timeoutSecs,
    );
    expect(
      screen.queryByText("请输入 600 至 3600 的整数。"),
    ).not.toBeInTheDocument();
    const apply = screen.getByRole("button", { name: "应用" });
    expect(apply).toBeEnabled();
    fireEvent.click(apply);

    await waitFor(() =>
      expect(ipc.updateImagesGenerationSettings).toHaveBeenCalledWith({
        enabled: false,
        routeId: previewSettingsSnapshot.imagesGeneration.routeId,
        timeoutSecs: previewSettingsSnapshot.imagesGeneration.timeoutSecs,
      }),
    );
  });
});
