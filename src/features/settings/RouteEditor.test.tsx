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
import customBalanceScriptScaffold from "./customBalanceScriptScaffold.txt?raw";

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

describe("RouteEditor interactions", () => {
  it("marks the editor title band without marking form controls", async () => {
    await renderSettings();

    const title = screen.getByRole("heading", {
      name: previewRouteEdits[0].name,
      level: 2,
    });
    expect(title).toHaveAttribute("data-tauri-drag-region");
    expect(title.parentElement).toHaveAttribute("data-tauri-drag-region");
    expect(screen.getByLabelText("路由名称")).not.toHaveAttribute(
      "data-tauri-drag-region",
    );
  });

  it("limits new route names to 30 characters without rewriting a loaded legacy name", async () => {
    const legacyName = "旧".repeat(31);
    await renderSettings({ routeName: legacyName });

    expect(screen.getByLabelText("路由名称")).toHaveAttribute(
      "maxlength",
      "30",
    );
    expect(screen.getByLabelText("路由名称")).toHaveValue(legacyName);
  });

  it("hydrates and saves the explicit Service Tier policy through native radios", async () => {
    await renderSettings();
    const group = screen.getByRole("radiogroup", { name: "Service Tier" });
    const passthrough = within(group).getByRole("radio", {
      name: "跟随 Codex",
    });
    const omit = within(group).getByRole("radio", { name: "移除参数" });
    const save = screen.getByRole("button", { name: "保存" });

    expect(passthrough).toBeChecked();
    expect(omit).not.toBeChecked();
    expect(save).toBeDisabled();

    fireEvent.click(omit);
    expect(omit).toBeChecked();
    expect(save).toBeEnabled();
    fireEvent.click(save);

    await waitFor(() =>
      expect(ipc.saveRoute).toHaveBeenCalledWith(
        expect.objectContaining({ serviceTierPolicy: "omit" }),
      ),
    );
  });

  it("defaults new routes to passthrough and hydrates an unchanged omit route", async () => {
    await renderSettings({ serviceTierPolicy: "omit" });
    expect(screen.getByRole("radio", { name: "移除参数" })).toBeChecked();
    expect(screen.getByRole("button", { name: "保存" })).toBeDisabled();

    fireEvent.click(screen.getByRole("button", { name: "新建路由" }));
    expect(screen.getByRole("radio", { name: "跟随 Codex" })).toBeChecked();
    expect(screen.getByRole("button", { name: "保存" })).toBeDisabled();
  });

  it("removes the Images API capability without leaving a route field", async () => {
    await renderSettings();
    expect(screen.queryByText("Images API")).not.toBeInTheDocument();
    expect(
      screen.queryByRole("switch", { name: "支持图片生成" }),
    ).not.toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "自定义模型" })).toBeVisible();
    expect(screen.getByRole("button", { name: "保存" })).toBeDisabled();
  });

  it("keeps a failed Service Tier draft dirty and participates in discard", async () => {
    ipc.saveRoute.mockRejectedValueOnce(new Error("injected"));
    await renderSettings();
    const omit = screen.getByRole("radio", { name: "移除参数" });
    const save = screen.getByRole("button", { name: "保存" });

    fireEvent.click(omit);
    fireEvent.click(save);

    expect(await screen.findByText("测试失败")).toHaveAttribute(
      "role",
      "alert",
    );
    expect(omit).toBeChecked();
    expect(save).toBeEnabled();

    fireEvent.click(screen.getByRole("button", { name: "Codex" }));
    expect(
      screen.getByRole("alertdialog", { name: "放弃未保存的修改？" }),
    ).toBeInTheDocument();
  });

  it("cancels first script-risk confirmation without invoking a save", async () => {
    const { client } = await renderSettings({
      riskConfirmed: false,
      scriptEnabled: false,
    });
    fireEvent.click(screen.getByRole("radio", { name: "自定义脚本" }));
    fireEvent.change(screen.getByLabelText("JavaScript 表达式"), {
      target: {
        value: "({ request: {}, extractor: () => ({ remaining: 1 }) })",
      },
    });
    const toggle = screen.getByRole("switch", { name: "启用余额查询" });
    fireEvent.click(toggle);
    fireEvent.click(screen.getByRole("button", { name: "保存" }));

    const dialog = screen.getByRole("alertdialog", {
      name: "允许余额脚本使用 API Key？",
    });
    fireEvent.click(within(dialog).getByRole("button", { name: "取消" }));
    expect(ipc.saveRoute).not.toHaveBeenCalled();
    expect(toggle).toBeChecked();
    expect(
      client.getQueryData<typeof previewSettingsSnapshot>(queryKeys.settings)
        ?.balanceScriptRiskConfirmed,
    ).toBe(false);
  });

  it("accepts first script risk in the same save input as script enablement", async () => {
    await renderSettings({ riskConfirmed: false, scriptEnabled: false });
    fireEvent.click(screen.getByRole("radio", { name: "自定义脚本" }));
    fireEvent.change(screen.getByLabelText("JavaScript 表达式"), {
      target: {
        value: "({ request: {}, extractor: () => ({ remaining: 1 }) })",
      },
    });
    fireEvent.click(screen.getByRole("switch", { name: "启用余额查询" }));
    fireEvent.click(screen.getByRole("button", { name: "保存" }));
    fireEvent.click(
      within(
        screen.getByRole("alertdialog", { name: "允许余额脚本使用 API Key？" }),
      ).getByRole("button", { name: "确认并保存" }),
    );

    await waitFor(() => expect(ipc.saveRoute).toHaveBeenCalledTimes(1));
    expect(ipc.saveRoute).toHaveBeenCalledWith(
      expect.objectContaining({
        acceptScriptRisk: true,
        balanceQuery: expect.objectContaining({
          mode: "custom_js",
          enabled: true,
        }),
      }),
    );
  });

  it("enables and saves native general queries without consuming script risk", async () => {
    await renderSettings({ riskConfirmed: false, scriptEnabled: false });
    fireEvent.click(screen.getByRole("switch", { name: "启用余额查询" }));
    fireEvent.click(screen.getByRole("button", { name: "保存" }));

    await waitFor(() => expect(ipc.saveRoute).toHaveBeenCalledTimes(1));
    expect(
      screen.queryByRole("alertdialog", { name: "允许余额脚本使用 API Key？" }),
    ).not.toBeInTheDocument();
    expect(ipc.saveRoute).toHaveBeenCalledWith(
      expect.objectContaining({
        acceptScriptRisk: false,
        balanceQuery: { mode: "general_v1", enabled: true, customSource: "" },
      }),
    );
  });

  it("uses the active-route deletion warning and cancellation performs no delete", async () => {
    await renderSettings();
    fireEvent.click(screen.getByRole("button", { name: "删除路由" }));
    const dialog = screen.getByRole("alertdialog", {
      name: "删除当前路由“AI INPUT 工作账号”？",
    });
    expect(dialog).toHaveTextContent(
      "删除后将进入“无中转”，新请求会失败，直到你手动切换路由。",
    );
    fireEvent.click(within(dialog).getByRole("button", { name: "取消" }));
    expect(ipc.deleteRoute).not.toHaveBeenCalled();
  });

  it("renders the direct custom-model editor inside the route and focuses the appended model ID", async () => {
    await renderSettings();

    expect(
      screen.getByRole("heading", { name: "自定义模型", level: 3 }),
    ).toBeInTheDocument();
    expect(screen.getByLabelText("模型 ID 1")).toHaveValue(
      "relay-custom-model",
    );
    expect(screen.getByLabelText("上下文窗口（Token） 2")).toHaveAttribute(
      "placeholder",
      "128000",
    );
    fireEvent.click(screen.getByRole("button", { name: "Codex" }));
    expect(
      screen.queryByRole("heading", { name: "自定义模型" }),
    ).not.toBeInTheDocument();
    expect(
      screen
        .getAllByRole("heading", { level: 3 })
        .map((heading) => heading.textContent),
    ).toEqual(["本地代理", "图片生成", "Codex 配置", "断开恢复配置"]);
    fireEvent.click(screen.getByRole("button", { name: "路由" }));
    await screen.findByLabelText("模型 ID 1");
    fireEvent.click(screen.getByRole("button", { name: "添加模型" }));
    expect(screen.getByLabelText("模型 ID 3")).toHaveFocus();
    expect(screen.getByRole("button", { name: "删除模型 3" })).toHaveAttribute(
      "title",
      "删除模型 3",
    );
    expect(screen.getAllByRole("button", { name: "保存" })).toHaveLength(1);
  });

  it("shows the compact empty state and validates duplicate model IDs before save", async () => {
    await renderSettings();
    fireEvent.click(
      screen.getByRole("button", { name: /Ciii 主用codex\.ciii\.club/ }),
    );
    await screen.findByText("尚未添加自定义模型");
    expect(screen.getByText("尚未添加自定义模型")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "添加模型" }));
    fireEvent.change(screen.getByLabelText("模型 ID 1"), {
      target: { value: "duplicate" },
    });
    fireEvent.click(screen.getByRole("button", { name: "添加模型" }));
    fireEvent.change(screen.getByLabelText("模型 ID 2"), {
      target: { value: "duplicate" },
    });
    fireEvent.click(screen.getByRole("button", { name: "保存" }));
    expect(screen.getByText("模型 ID 不能重复。")).toBeInTheDocument();
    expect(ipc.saveRoute).not.toHaveBeenCalled();
  });

  it("saves route fields and normalized model values in one action and reports restart", async () => {
    await renderSettings();
    fireEvent.change(screen.getByLabelText("模型 ID 1"), {
      target: { value: " relay-updated " },
    });
    ipc.saveRoute.mockResolvedValueOnce({
      routeId: previewRouteEdits[0].routeId,
      revision: 14,
      catalog: {
        models: [
          { ...previewRouteEdits[0].models[0], modelId: "relay-updated" },
          previewRouteEdits[0].models[1],
        ],
        changed: true,
        projectionApplied: true,
        retryRequired: false,
        activation: "restart_codex",
        errorCode: null,
        retryToken: null,
      },
    });

    fireEvent.click(screen.getByRole("button", { name: "保存" }));
    await waitFor(() =>
      expect(ipc.saveRoute).toHaveBeenCalledWith(
        expect.objectContaining({
          retryToken: null,
          models: expect.arrayContaining([
            expect.objectContaining({ modelId: "relay-updated" }),
          ]),
        }),
      ),
    );
    expect(
      await screen.findByText("已保存，重启 Codex 后生效"),
    ).toBeInTheDocument();
  });

  it("keeps a partial application dirty and sends an explicit reconciliation retry", async () => {
    await renderSettings();
    fireEvent.change(screen.getByLabelText("显示名称 1"), {
      target: { value: "Updated Relay" },
    });
    ipc.saveRoute
      .mockResolvedValueOnce({
        routeId: previewRouteEdits[0].routeId,
        revision: 14,
        catalog: {
          models: structuredClone(previewRouteEdits[0].models),
          changed: true,
          projectionApplied: false,
          retryRequired: true,
          activation: "reconnect_codex",
          errorCode: "codex_config_changed",
          retryToken: "permit-1",
        },
      })
      .mockResolvedValueOnce({
        routeId: previewRouteEdits[0].routeId,
        revision: 14,
        catalog: {
          models: structuredClone(previewRouteEdits[0].models),
          changed: false,
          projectionApplied: true,
          retryRequired: false,
          activation: "restart_codex",
          errorCode: null,
          retryToken: null,
        },
      });

    fireEvent.click(screen.getByRole("button", { name: "保存" }));
    expect(await screen.findByText("2 个 · 需要重试")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "系统" }));
    expect(
      screen.getByRole("alertdialog", { name: "放弃未保存的修改？" }),
    ).toHaveTextContent("当前设置的修改尚未保存。");
    fireEvent.click(screen.getByRole("button", { name: "继续编辑" }));
    fireEvent.click(screen.getByRole("button", { name: "保存" }));
    await waitFor(() =>
      expect(ipc.saveRoute).toHaveBeenLastCalledWith(
        expect.objectContaining({ retryToken: "permit-1" }),
      ),
    );
  });

  it("keeps secrets out of ordinary caches and removes the edit secret after unmount", async () => {
    const { client } = await renderSettings();
    expect(
      JSON.stringify(client.getQueryData(queryKeys.settings)),
    ).not.toContain("preview-key-not-real");
    expect(JSON.stringify(client.getQueryData(queryKeys.menu))).not.toContain(
      "preview-key-not-real",
    );
    expect(
      JSON.stringify(
        client
          .getQueryCache()
          .getAll()
          .map((query) => query.state.data),
      ),
    ).not.toContain("preview-key-not-real");
    expect(screen.getByLabelText("API Key")).toHaveValue(
      "preview-key-not-real",
    );

    fireEvent.click(screen.getByRole("button", { name: "Codex" }));

    expect(screen.queryByLabelText("API Key")).not.toBeInTheDocument();
    expect(
      JSON.stringify(
        client
          .getQueryCache()
          .getAll()
          .map((query) => query.state.data),
      ),
    ).not.toContain("preview-key-not-real");
  });

  it.each([
    ["reachable" as const, 186, "可达 · 186 ms"],
    ["slow" as const, 6201, "较慢 · 6201 ms"],
    ["path_not_found" as const, 42, "服务器可达，推理路径可能不正确 · 42 ms"],
    ["unreachable" as const, null, "不可达"],
  ])(
    "renders the %s reachability result as form-only state",
    async (status, ttfbMs, label) => {
      ipc.checkRouteReachability.mockResolvedValue({
        status,
        ttfbMs,
        errorCategory: null,
      });
      await renderSettings();

      fireEvent.click(screen.getByRole("button", { name: "检查推理地址" }));

      expect(await screen.findByText(label)).toBeInTheDocument();
      expect(ipc.checkRouteReachability).toHaveBeenCalledWith(
        "https://ai.input.im/v1",
      );
    },
  );

  it("previews complete endpoints canonically and rejects incompatible paths locally", async () => {
    await renderSettings();
    const input = screen.getByLabelText("Responses Base URL");
    const probe = screen.getByRole("button", { name: "检查推理地址" });

    fireEvent.change(input, {
      target: { value: " https://example.test/openai/v1/responses/ " },
    });
    expect(
      screen.getByText("https://example.test/openai/v1/responses"),
    ).toBeInTheDocument();
    expect(probe).toBeEnabled();

    fireEvent.change(input, {
      target: { value: "https://example.test/v1/chat/completions" },
    });
    expect(screen.getByText("地址无效")).toBeInTheDocument();
    expect(probe).toBeDisabled();
  });

  it("clears a stale reachability result when the Base URL changes", async () => {
    ipc.checkRouteReachability.mockResolvedValue({
      status: "reachable",
      ttfbMs: 18,
      errorCategory: null,
    });
    await renderSettings();

    fireEvent.click(screen.getByRole("button", { name: "检查推理地址" }));
    expect(await screen.findByText("可达 · 18 ms")).toBeInTheDocument();

    fireEvent.change(screen.getByLabelText("Responses Base URL"), {
      target: { value: "https://second.example/v1" },
    });
    expect(screen.queryByText("可达 · 18 ms")).not.toBeInTheDocument();
    expect(
      screen.getByText("https://second.example/v1/responses"),
    ).toBeInTheDocument();
  });

  it("drops an in-flight probe result after the Base URL changes and clears pending state", async () => {
    let resolveProbe:
      | ((result: {
          status: "reachable";
          ttfbMs: number;
          errorCategory: null;
        }) => void)
      | undefined;
    ipc.checkRouteReachability.mockReturnValue(
      new Promise((resolve) => {
        resolveProbe = resolve;
      }),
    );
    await renderSettings();
    const probe = screen.getByRole("button", { name: "检查推理地址" });

    fireEvent.click(probe);
    expect(probe).toBeDisabled();
    fireEvent.change(screen.getByLabelText("Responses Base URL"), {
      target: { value: "https://changed.example/v1" },
    });
    resolveProbe?.({ status: "reachable", ttfbMs: 12, errorCategory: null });

    await waitFor(() => expect(probe).toBeEnabled());
    expect(screen.queryByText("可达 · 12 ms")).not.toBeInTheDocument();
  });

  it("reloads the backend-owned canonical prefix after save", async () => {
    await renderSettings();
    fireEvent.change(screen.getByLabelText("Responses Base URL"), {
      target: { value: "https://example.test/v1/responses" },
    });
    ipc.getRouteEdit.mockResolvedValue({
      ...previewRouteEdits[0],
      baseUrl: "https://example.test/v1",
      inferenceUrl: "https://example.test/v1/responses",
    });

    fireEvent.click(screen.getByRole("button", { name: "保存" }));

    await waitFor(() =>
      expect(ipc.saveRoute).toHaveBeenCalledWith(
        expect.objectContaining({
          baseUrl: "https://example.test/v1/responses",
        }),
      ),
    );
    expect(await screen.findByLabelText("Responses Base URL")).toHaveValue(
      "https://example.test/v1",
    );
  });

  it("keeps general mode script-free and retains a custom draft across mode switches", async () => {
    await renderSettings();

    expect(screen.getByRole("radio", { name: "通用查询" })).toBeChecked();
    expect(
      screen.queryByLabelText("JavaScript 表达式"),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "格式化" }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: /加载.*示例/u }),
    ).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("radio", { name: "自定义脚本" }));
    expect(screen.getByLabelText("JavaScript 表达式")).toHaveValue("");
    expect(screen.getByRole("button", { name: "插入骨架" })).toBeEnabled();
    expect(
      screen.queryByRole("button", { name: "格式化" }),
    ).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "插入骨架" }));
    expect(screen.getByLabelText("JavaScript 表达式")).toHaveValue(
      customBalanceScriptScaffold,
    );
    expect(
      screen.queryByRole("button", { name: "插入骨架" }),
    ).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "格式化" })).toBeEnabled();

    const source = "({ request: {}, extractor: () => ({ remaining: 7 }) })";
    fireEvent.change(screen.getByLabelText("JavaScript 表达式"), {
      target: { value: source },
    });
    fireEvent.click(screen.getByRole("radio", { name: "通用查询" }));
    expect(
      screen.queryByLabelText("JavaScript 表达式"),
    ).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("radio", { name: "自定义脚本" }));
    expect(screen.getByLabelText("JavaScript 表达式")).toHaveValue(source);

    fireEvent.click(screen.getByRole("radio", { name: "通用查询" }));
    fireEvent.click(screen.getByRole("button", { name: "保存" }));
    await waitFor(() => expect(ipc.saveRoute).toHaveBeenCalledTimes(1));
    expect(ipc.saveRoute).toHaveBeenCalledWith(
      expect.objectContaining({
        balanceQuery: {
          mode: "general_v1",
          enabled: true,
          customSource: source,
        },
      }),
    );
  });

  it("keeps the empty balance action cluster trailing after custom-script actions", async () => {
    await renderSettings();

    const testBalance = screen.getByRole("button", { name: "测试余额查询" });
    const cluster = testBalance.closest(".route-balance-actions");
    expect(cluster).not.toBeNull();
    expect(cluster?.querySelector(".balance-test-result")).toBeNull();
    expect(cluster?.parentElement).toHaveClass("route-script-actions");

    fireEvent.click(screen.getByRole("radio", { name: "自定义脚本" }));
    const insertScaffold = screen.getByRole("button", { name: "插入骨架" });
    const actionGroup = cluster?.parentElement;
    expect(actionGroup?.children).toHaveLength(2);
    expect(actionGroup?.children[0]).toContainElement(insertScaffold);
    expect(actionGroup?.children[1]).toBe(cluster);
  });

  it("tests general and custom modes through mode-aware payloads", async () => {
    ipc.testBalanceQuery.mockResolvedValue({
      isValid: true,
      remaining: 12,
      used: null,
      total: null,
      unit: "USD",
      planName: null,
      invalidMessage: null,
      extra: null,
    });
    await renderSettings();

    fireEvent.click(screen.getByRole("button", { name: "测试余额查询" }));
    await waitFor(() =>
      expect(ipc.testBalanceQuery).toHaveBeenLastCalledWith({
        baseUrl: "https://ai.input.im/v1",
        apiKey: "preview-key-not-real",
        mode: "general_v1",
        customSource: "",
      }),
    );

    fireEvent.click(screen.getByRole("radio", { name: "自定义脚本" }));
    const source = "({ request: {}, extractor: () => ({ remaining: 5 }) })";
    fireEvent.change(screen.getByLabelText("JavaScript 表达式"), {
      target: { value: source },
    });
    fireEvent.click(screen.getByRole("button", { name: "测试余额查询" }));
    await waitFor(() =>
      expect(ipc.testBalanceQuery).toHaveBeenLastCalledWith({
        baseUrl: "https://ai.input.im/v1",
        apiKey: "preview-key-not-real",
        mode: "custom_js",
        customSource: source,
      }),
    );
  });

  it("shows balance test results with two decimal places", async () => {
    ipc.testBalanceQuery.mockResolvedValue({
      isValid: true,
      remaining: 300.126,
      used: null,
      total: null,
      unit: "USD",
      planName: null,
      invalidMessage: null,
      extra: null,
    });
    await renderSettings();

    fireEvent.click(screen.getByRole("button", { name: "测试余额查询" }));

    const result = await screen.findByText("余额 300.13 USD");
    const testBalance = screen.getByRole("button", { name: "测试余额查询" });
    expect(result).toHaveClass(
      "settings-status-success",
      "balance-test-result",
    );
    expect(result).toHaveAttribute("role", "status");
    expect(result.parentElement).toBe(testBalance.parentElement);
    expect(result.nextElementSibling).toBe(testBalance);
  });

  it("keeps an invalid balance result danger-styled beside its test action", async () => {
    ipc.testBalanceQuery.mockResolvedValue({
      isValid: false,
      remaining: null,
      used: null,
      total: null,
      unit: null,
      planName: null,
      invalidMessage: "查询返回无效结果",
      extra: null,
    });
    await renderSettings();

    fireEvent.click(screen.getByRole("button", { name: "测试余额查询" }));

    const result = await screen.findByText("查询返回无效结果");
    const testBalance = screen.getByRole("button", { name: "测试余额查询" });
    expect(result).toHaveClass("settings-status-danger", "balance-test-result");
    expect(result).toHaveAttribute("role", "status");
    expect(result.parentElement).toBe(testBalance.parentElement);
    expect(result.nextElementSibling).toBe(testBalance);
  });

  it("keeps a failed balance request beside its test action", async () => {
    ipc.testBalanceQuery.mockRejectedValueOnce(new Error("injected"));
    await renderSettings();

    fireEvent.click(screen.getByRole("button", { name: "测试余额查询" }));

    const result = await screen.findByText("测试失败");
    const testBalance = screen.getByRole("button", { name: "测试余额查询" });
    expect(result).toHaveClass("settings-status-danger", "balance-test-result");
    expect(result).toHaveAttribute("role", "alert");
    expect(result.parentElement).toBe(testBalance.parentElement);
    expect(result.nextElementSibling).toBe(testBalance);
    expect(result).not.toHaveClass("settings-error");
  });
});
