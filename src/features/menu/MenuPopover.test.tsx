import { QueryClientProvider } from "@tanstack/react-query";
import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { createRouterQueryClient, queryKeys } from "../../api/query";
import type {
  BalanceDisplayStatus,
  BootstrapSnapshotDto,
  InferenceStatusKind,
  MenuSnapshotDto,
  RouteId,
  UsageHistoryPageDto,
  UsageHistoryRowDto,
} from "../../generated";
import { MenuPopover } from "./MenuPopover";

const ipc = vi.hoisted(() => ({
  confirmRouteActivation: vi.fn(),
  connectCodex: vi.fn(),
  completeMenuShow: vi.fn(),
  dismissCodexRestartNotice: vi.fn(),
  getMenuSnapshot: vi.fn(),
  getApplicationUpdateSnapshot: vi.fn(),
  getUsageHistory: vi.fn(),
  hideMenu: vi.fn(),
  refreshAllBalances: vi.fn(),
  refreshBalance: vi.fn(),
  reconnectCodex: vi.fn(),
  restoreCodex: vi.fn(),
  setFallbackEnabled: vi.fn(),
  setMenuUsagePreview: vi.fn(),
  showSettingsWindow: vi.fn(),
  previewRouteActivation: vi.fn(),
  prepareListener: undefined as
    ((event: { generation: number }) => void) | undefined,
}));

vi.mock("../../api/ipc", () => ({
  confirmRouteActivation: ipc.confirmRouteActivation,
  connectCodex: ipc.connectCodex,
  completeMenuShow: ipc.completeMenuShow,
  dismissCodexRestartNotice: ipc.dismissCodexRestartNotice,
  getBootstrapSnapshot: vi.fn(),
  getMenuSnapshot: ipc.getMenuSnapshot,
  getApplicationUpdateSnapshot: ipc.getApplicationUpdateSnapshot,
  getUsageHistory: ipc.getUsageHistory,
  hideMenu: ipc.hideMenu,
  isTauriRuntime: () => true,
  listenMenuPositioned: vi.fn(async () => vi.fn()),
  listenMenuPrepare: vi.fn(
    async (listener: (event: { generation: number }) => void) => {
      ipc.prepareListener = listener;
      return vi.fn();
    },
  ),
  menuFrontendReady: vi.fn(),
  normalizeIpcError: (reason: unknown) =>
    typeof reason === "object" && reason !== null && "code" in reason
      ? reason
      : { code: "test", message: "测试失败", retryable: false, field: null },
  quitApplication: vi.fn(),
  refreshAllBalances: ipc.refreshAllBalances,
  refreshBalance: ipc.refreshBalance,
  reconnectCodex: ipc.reconnectCodex,
  restoreCodex: ipc.restoreCodex,
  setFallbackEnabled: ipc.setFallbackEnabled,
  setMenuUsagePreview: ipc.setMenuUsagePreview,
  showSettingsWindow: ipc.showSettingsWindow,
  previewRouteActivation: ipc.previewRouteActivation,
}));

const routeId = "menu-route" as RouteId;

function bootstrap(
  proxyStatus: BootstrapSnapshotDto["proxyStatus"] = "running",
) {
  return {
    revision: 1,
    routes: [
      {
        routeId,
        name: "测试路由",
        baseUrlHost: "example.com",
        inferenceStatus: {
          kind: "unverified" as const,
          lastOutcome: null,
          failureReason: null,
          observedAtMs: null,
        },
      },
    ],
    activeRouteId: routeId,
    fallback: {
      enabled: false,
      participantCount: 1,
      configRevision: 1,
      activePosition: null,
      hasNext: false,
    },
    proxyStatus,
    lifecycle: { phase: "running" as const, issue: null },
    appearancePreference: "system" as const,
  };
}

function menuSnapshot(
  options: { balanceEnabled?: boolean; activeRoute?: boolean } = {},
): MenuSnapshotDto {
  return {
    bootstrap: {
      ...bootstrap(),
      activeRouteId: options.activeRoute === false ? null : routeId,
    },
    balances: [],
    balanceEnabledRouteIds: options.balanceEnabled === false ? [] : [routeId],
    balanceBatch: null,
    codexStatus: "connected" as const,
    codexRestartNotice: null,
  };
}

function populateRoutes(snapshot: MenuSnapshotDto, count: number) {
  const template = snapshot.bootstrap.routes[0];
  const routes = Array.from({ length: count }, (_, index) => ({
    ...template,
    routeId: `menu-route-${index + 1}` as RouteId,
    name: `测试路由 ${index + 1}`,
  }));
  snapshot.bootstrap.routes = routes;
  snapshot.bootstrap.activeRouteId = routes[0]?.routeId ?? null;
  snapshot.balanceEnabledRouteIds = routes.map((route) => route.routeId);
  return routes;
}

function usageRow(routeId: RouteId, model: string): UsageHistoryRowDto {
  return {
    requestId: `request-${routeId}`,
    startedAtMs: 1_754_003_000_000,
    finishedAtMs: 1_754_003_012_345,
    routeId,
    routeName: "测试路由",
    requestedModel: model,
    actualModel: model,
    reasoningEffort: "high",
    streaming: true,
    completionState: "completed",
    httpStatus: 200,
    tokens: {
      input: 100,
      uncachedInput: 80,
      output: 20,
      total: 120,
      cachedInput: 20,
      cacheWriteInput: 0,
    },
    totalLatencyMs: 1_000,
    firstOutputLatencyMs: 500,
    cost: {
      state: "exact",
      amountPicoUsd: "1000000",
      currency: "USD",
      catalogVersion: null,
      serviceTier: null,
      fastStatus: null,
    },
  };
}

function renderMenu(
  snapshot?: MenuSnapshotDto,
  fallback?: BootstrapSnapshotDto,
) {
  const client = createRouterQueryClient();
  if (snapshot) client.setQueryData(queryKeys.menu, snapshot);
  if (fallback ?? snapshot?.bootstrap) {
    client.setQueryData(queryKeys.bootstrap, fallback ?? snapshot?.bootstrap);
  }
  return {
    ...render(
      <QueryClientProvider client={client}>
        <MenuPopover />
      </QueryClientProvider>,
    ),
    client,
  };
}

beforeEach(() => {
  ipc.confirmRouteActivation.mockReset();
  ipc.connectCodex.mockReset();
  ipc.completeMenuShow.mockReset();
  ipc.dismissCodexRestartNotice.mockReset();
  ipc.getMenuSnapshot.mockReset();
  ipc.getApplicationUpdateSnapshot.mockReset();
  ipc.getUsageHistory.mockReset();
  ipc.hideMenu.mockReset();
  ipc.refreshAllBalances.mockReset();
  ipc.refreshBalance.mockReset();
  ipc.reconnectCodex.mockReset();
  ipc.restoreCodex.mockReset();
  ipc.setFallbackEnabled.mockReset();
  ipc.setMenuUsagePreview.mockReset();
  ipc.showSettingsWindow.mockReset();
  ipc.previewRouteActivation.mockReset();
  ipc.confirmRouteActivation.mockResolvedValue({
    revision: 2,
    catalog: {
      models: [],
      changed: false,
      projectionApplied: true,
      retryRequired: false,
      activation: "none",
      errorCode: null,
      retryToken: null,
    },
  });
  ipc.dismissCodexRestartNotice.mockResolvedValue({ revision: 2 });
  ipc.setFallbackEnabled.mockResolvedValue({ revision: 2 });
  ipc.previewRouteActivation.mockResolvedValue({
    targetRouteId: routeId,
    targetRouteName: "测试路由",
    targetCatalogMode: "original",
    confirmationRequired: false,
    permit: "route-permit",
  });
  ipc.getMenuSnapshot.mockResolvedValue(menuSnapshot());
  ipc.getApplicationUpdateSnapshot.mockResolvedValue({
    currentVersion: "0.1.0",
    operation: "idle",
    available: null,
    lastSuccessfulCheckAtMs: null,
    downloadedBytes: null,
    totalBytes: null,
    manualFailure: null,
  });
  ipc.getUsageHistory.mockResolvedValue({
    rows: [],
    nextCursor: null,
    totalRows: 0,
  });
  ipc.setMenuUsagePreview.mockResolvedValue(undefined);
  ipc.prepareListener = undefined;
  Object.defineProperty(globalThis, "CSS", {
    configurable: true,
    value: { escape: (value: string) => value },
  });
  Object.defineProperty(HTMLElement.prototype, "scrollIntoView", {
    configurable: true,
    value: vi.fn(),
  });
});

describe("P8 menu interactions", () => {
  it("adds an accessible fixed-slot indicator without changing the footer command", async () => {
    ipc.getApplicationUpdateSnapshot.mockResolvedValue({
      currentVersion: "0.1.0",
      operation: "idle",
      available: {
        version: "0.2.0",
        notes: "Synthetic update",
        releaseUrl: "https://github.com/Angry3D/ai-router/releases/tag/v0.2.0",
      },
      lastSuccessfulCheckAtMs: 1_725_000_000_000,
      downloadedBytes: null,
      totalBytes: null,
      manualFailure: null,
    });
    renderMenu(menuSnapshot());

    const settings = await screen.findByRole("button", {
      name: "打开设置，有可用更新",
    });
    expect(settings).toHaveClass("menu-settings-button");
    expect(
      settings.querySelector(".application-update-indicator"),
    ).toHaveAttribute("aria-hidden", "true");
    expect(screen.getByRole("button", { name: "更新全部余额" })).toBeEnabled();
  });
  it("disables balance refresh when no route has an enabled script", () => {
    renderMenu(menuSnapshot({ balanceEnabled: false }));

    expect(screen.getByText("未配置余额")).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "刷新 测试路由 的余额" }),
    ).toBeDisabled();
    expect(
      screen.getByRole("button", { name: "没有可更新的余额" }),
    ).toBeDisabled();
  });

  it("keeps an enabled script refreshable before its first cache result", () => {
    renderMenu(menuSnapshot());

    expect(screen.getByText("尚无余额")).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "刷新 测试路由 的余额" }),
    ).toBeEnabled();
    expect(screen.getByRole("button", { name: "更新全部余额" })).toBeEnabled();
  });

  it("uses the designed icon radio for selected and unselected routes", () => {
    const activeMenu = renderMenu(menuSnapshot());

    expect(
      screen.getByRole("option").querySelector(".lucide-circle-check"),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("option").querySelector(".lucide-circle"),
    ).not.toBeInTheDocument();

    activeMenu.unmount();
    const snapshot = menuSnapshot({ activeRoute: false });
    renderMenu(snapshot);

    expect(
      screen.getByRole("option").querySelector(".lucide-circle"),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("option").querySelector(".lucide-circle-check"),
    ).not.toBeInTheDocument();
  });

  it("animates the global refresh icon until the batch returns", async () => {
    const snapshot = menuSnapshot();
    let finishRefresh!: (result: unknown) => void;
    ipc.refreshAllBalances.mockReturnValue(
      new Promise((resolve) => {
        finishRefresh = resolve;
      }),
    );
    ipc.getMenuSnapshot.mockResolvedValue(snapshot);
    renderMenu(snapshot);

    const button = screen.getByRole("button", { name: "更新全部余额" });
    fireEvent.click(button);
    expect(button.querySelector("svg")).toHaveClass("spin");

    finishRefresh({
      batchId: "batch-1",
      eligibleCount: 1,
      completedCount: 1,
      successCount: 1,
      failureCount: 0,
      phase: "completed",
    });
    await waitFor(() =>
      expect(button.querySelector("svg")).not.toHaveClass("spin"),
    );
    expect(button).toBeEnabled();
  });

  it("connects Codex from the runtime row and renders the authoritative refresh", async () => {
    const snapshot = menuSnapshot();
    snapshot.codexStatus = "not_connected";
    const connected = menuSnapshot();
    let finishConnect!: (result: unknown) => void;
    ipc.connectCodex.mockReturnValue(
      new Promise((resolve) => {
        finishConnect = resolve;
      }),
    );
    ipc.getMenuSnapshot.mockResolvedValue(connected);
    renderMenu(snapshot);

    fireEvent.click(
      screen.getByRole("button", { name: "Codex 未连接，连接 Codex" }),
    );
    expect(ipc.connectCodex).toHaveBeenCalledWith(false);
    expect(
      screen.getByRole("button", { name: "Codex 未连接，连接中" }),
    ).toBeDisabled();

    finishConnect({ changed: true, status: "connected" });
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "Codex 已连接，断开 Codex" }),
      ).toBeInTheDocument(),
    );
    expect(ipc.hideMenu).not.toHaveBeenCalled();
  });

  it("keeps product identity alone in the title row and both controls in runtime status", () => {
    const snapshot = menuSnapshot();
    snapshot.codexStatus = "not_connected";
    renderMenu(snapshot);

    const titleRow = screen
      .getByRole("heading", { name: "AI Router" })
      .closest(".menu-header-primary");
    const runtime = screen.getByLabelText("运行状态");
    expect(titleRow?.querySelector("button")).toBeNull();
    expect(within(runtime).getAllByRole("button")).toHaveLength(2);
    expect(runtime).toHaveTextContent("代理运行中");
  });

  it.each([
    ["connected" as const, "Codex 已连接", "断开 Codex", false],
    ["checking" as const, "Codex 检查中", "正在检查", true],
    ["changed" as const, "Codex 待重新连接", "重新连接", false],
    ["not_connected" as const, "Codex 未连接", "连接 Codex", false],
    [
      "images_mcp_name_conflict" as const,
      "Codex 图片配置冲突",
      "前往处理",
      false,
    ],
    [
      "images_mcp_projection_conflict" as const,
      "Codex 图片配置冲突",
      "前往处理",
      false,
    ],
    ["invalid" as const, "Codex 配置异常", "修复配置", false],
    ["unreadable" as const, "Codex 配置异常", "修复配置", false],
    ["symlink_unsupported" as const, "Codex 配置异常", "修复配置", false],
  ])(
    "maps Codex %s to stable state and action copy",
    (status, state, action, unavailable) => {
      const snapshot = menuSnapshot();
      snapshot.codexStatus = status;
      renderMenu(snapshot);

      const button = screen.getByRole("button", {
        name: `${state}，${action}`,
      });
      const stateCopy = within(button).getByText(state);
      const actionCopy = within(button).getByText(action);
      expect(stateCopy).toHaveClass("runtime-control-state");
      expect(actionCopy).toHaveClass("runtime-control-action");
      expect(stateCopy.parentElement).toBe(actionCopy.parentElement);
      if (unavailable) expect(button).toHaveAttribute("aria-disabled", "true");
      else expect(button).not.toHaveAttribute("aria-disabled");
    },
  );

  it("reconnects a changed Codex projection", async () => {
    const changed = menuSnapshot();
    changed.codexStatus = "changed";
    ipc.reconnectCodex.mockResolvedValue({
      changed: true,
      status: "connected",
    });
    ipc.getMenuSnapshot.mockResolvedValue(changed);
    renderMenu(changed);

    fireEvent.click(
      screen.getByRole("button", { name: "Codex 待重新连接，重新连接" }),
    );
    await waitFor(() => expect(ipc.reconnectCodex).toHaveBeenCalledTimes(1));
  });

  it.each(["not_connected" as const, "changed" as const])(
    "keeps Codex %s focusable but guarded without an active route",
    (status) => {
      const disconnected = menuSnapshot({ activeRoute: false });
      disconnected.codexStatus = status;
      renderMenu(disconnected);

      const button = screen.getByRole("button", {
        name: `${status === "changed" ? "Codex 待重新连接" : "Codex 未连接"}，请先选择路由`,
      });
      expect(button).toHaveAttribute("aria-disabled", "true");
      expect(button).not.toBeDisabled();
      fireEvent.click(button);
      expect(ipc.connectCodex).not.toHaveBeenCalled();
      expect(ipc.reconnectCodex).not.toHaveBeenCalled();
    },
  );

  it.each([
    "invalid" as const,
    "unreadable" as const,
    "symlink_unsupported" as const,
  ])("routes Codex %s repair to Codex Settings", (status) => {
    const snapshot = menuSnapshot();
    snapshot.codexStatus = status;
    renderMenu(snapshot);

    fireEvent.click(
      screen.getByRole("button", { name: "Codex 配置异常，修复配置" }),
    );
    expect(ipc.showSettingsWindow).toHaveBeenCalledWith("codex");
    expect(ipc.connectCodex).not.toHaveBeenCalled();
  });

  it.each([
    "images_mcp_name_conflict" as const,
    "images_mcp_projection_conflict" as const,
  ])("routes Codex %s conflict to Settings without reconnecting", (status) => {
    const snapshot = menuSnapshot({ activeRoute: false });
    snapshot.codexStatus = status;
    renderMenu(snapshot);

    fireEvent.click(
      screen.getByRole("button", {
        name: "Codex 图片配置冲突，前往处理",
      }),
    );
    expect(ipc.showSettingsWindow).toHaveBeenCalledWith("codex");
    expect(ipc.connectCodex).not.toHaveBeenCalled();
    expect(ipc.reconnectCodex).not.toHaveBeenCalled();
  });

  it("confirms disconnect, restores the recovery config, and keeps the refreshed menu open", async () => {
    const connected = menuSnapshot({ activeRoute: false });
    const disconnected = menuSnapshot({ activeRoute: false });
    disconnected.codexStatus = "not_connected";
    let finishRestore!: (result: unknown) => void;
    ipc.restoreCodex.mockReturnValue(
      new Promise((resolve) => {
        finishRestore = resolve;
      }),
    );
    ipc.getMenuSnapshot.mockResolvedValue(disconnected);
    renderMenu(connected);

    const disconnectButton = screen.getByRole("button", {
      name: "Codex 已连接，断开 Codex",
    });
    expect(disconnectButton).toBeEnabled();
    fireEvent.click(disconnectButton);

    const dialog = screen.getByRole("alertdialog", { name: "断开 Codex？" });
    expect(dialog).toHaveTextContent("更新恢复配置后保留的修改不会丢失");
    expect(ipc.restoreCodex).not.toHaveBeenCalled();

    const cancelButton = screen.getByRole("button", { name: "取消" });
    expect(cancelButton).toHaveFocus();
    fireEvent.click(cancelButton);
    expect(
      screen.queryByRole("alertdialog", { name: "断开 Codex？" }),
    ).not.toBeInTheDocument();
    expect(ipc.restoreCodex).not.toHaveBeenCalled();

    fireEvent.click(
      screen.getByRole("button", { name: "Codex 已连接，断开 Codex" }),
    );
    fireEvent.click(screen.getByRole("button", { name: "断开连接" }));
    expect(ipc.restoreCodex).toHaveBeenCalledTimes(1);
    expect(
      screen.getByRole("button", { name: "Codex 已连接，断开中" }),
    ).toBeDisabled();

    finishRestore({ changed: true, status: "not_connected" });
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "Codex 未连接，请先选择路由" }),
      ).toBeInTheDocument(),
    );
    expect(ipc.hideMenu).not.toHaveBeenCalled();
  });

  it("keeps the connected menu usable when disconnect restore fails", async () => {
    ipc.restoreCodex.mockRejectedValue(new Error("restore failed"));
    renderMenu(menuSnapshot());

    fireEvent.click(
      screen.getByRole("button", { name: "Codex 已连接，断开 Codex" }),
    );
    fireEvent.click(screen.getByRole("button", { name: "断开连接" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("测试失败");
    expect(
      screen.getByRole("button", { name: "Codex 已连接，断开 Codex" }),
    ).toBeEnabled();
    expect(ipc.hideMenu).not.toHaveBeenCalled();
  });

  it("keeps disconnect progress tied to the operation when the snapshot changes", async () => {
    const connected = menuSnapshot();
    const disconnected = menuSnapshot();
    disconnected.codexStatus = "not_connected";
    let finishRestore!: (result: unknown) => void;
    ipc.restoreCodex.mockReturnValue(
      new Promise((resolve) => {
        finishRestore = resolve;
      }),
    );
    ipc.getMenuSnapshot.mockResolvedValue(disconnected);
    const view = renderMenu(connected);

    fireEvent.click(
      screen.getByRole("button", { name: "Codex 已连接，断开 Codex" }),
    );
    fireEvent.click(screen.getByRole("button", { name: "断开连接" }));
    act(() => view.client.setQueryData(queryKeys.menu, disconnected));

    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "Codex 未连接，断开中" }),
      ).toBeDisabled(),
    );
    expect(screen.queryByText("连接中")).not.toBeInTheDocument();

    finishRestore({ changed: true, status: "not_connected" });
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "Codex 未连接，连接 Codex" }),
      ).toBeEnabled(),
    );
  });

  it.each([
    [
      false,
      true,
      "Fallback 已关闭，开启 Fallback",
      "Fallback 已开启，关闭 Fallback",
    ],
    [
      true,
      false,
      "Fallback 已开启，关闭 Fallback",
      "Fallback 已关闭，开启 Fallback",
    ],
  ])(
    "sets Fallback from %s to %s and renders the refetched value",
    async (enabled, nextEnabled, initialName, refreshedName) => {
      const initial = menuSnapshot();
      initial.bootstrap.fallback = {
        enabled,
        participantCount: 3,
        configRevision: 1,
        activePosition: enabled ? 1 : null,
        hasNext: enabled,
      };
      const refreshed = menuSnapshot();
      refreshed.bootstrap.fallback = {
        enabled: nextEnabled,
        participantCount: 3,
        configRevision: 2,
        activePosition: nextEnabled ? 1 : null,
        hasNext: nextEnabled,
      };
      ipc.getMenuSnapshot.mockResolvedValue(refreshed);
      renderMenu(initial);

      fireEvent.click(screen.getByRole("button", { name: initialName }));

      await waitFor(() =>
        expect(ipc.setFallbackEnabled).toHaveBeenCalledWith(nextEnabled),
      );
      await waitFor(() =>
        expect(
          screen.getByRole("button", { name: refreshedName }),
        ).toHaveAttribute("aria-pressed", String(nextEnabled)),
      );
      expect(ipc.getMenuSnapshot).toHaveBeenCalled();
      expect(ipc.hideMenu).not.toHaveBeenCalled();
    },
  );

  it("disables a pending Fallback mutation and prevents duplicate writes", async () => {
    const initial = menuSnapshot();
    initial.bootstrap.fallback = {
      enabled: false,
      participantCount: 3,
      configRevision: 1,
      activePosition: null,
      hasNext: false,
    };
    const refreshed = menuSnapshot();
    refreshed.bootstrap.fallback = {
      enabled: true,
      participantCount: 3,
      configRevision: 2,
      activePosition: 1,
      hasNext: true,
    };
    let finishFallback!: (result: unknown) => void;
    ipc.setFallbackEnabled.mockReturnValue(
      new Promise((resolve) => {
        finishFallback = resolve;
      }),
    );
    ipc.getMenuSnapshot.mockResolvedValue(refreshed);
    renderMenu(initial);

    fireEvent.click(
      screen.getByRole("button", { name: "Fallback 已关闭，开启 Fallback" }),
    );
    const pending = screen.getByRole("button", {
      name: "Fallback 已关闭，开启中",
    });
    expect(pending).toBeDisabled();
    fireEvent.click(pending);
    expect(ipc.setFallbackEnabled).toHaveBeenCalledTimes(1);

    finishFallback({ revision: 2 });
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "Fallback 已开启，关闭 Fallback" }),
      ).toBeEnabled(),
    );
  });

  it("restores confirmed Fallback presentation after a failed mutation", async () => {
    const snapshot = menuSnapshot();
    snapshot.bootstrap.fallback = {
      enabled: false,
      participantCount: 3,
      configRevision: 1,
      activePosition: null,
      hasNext: false,
    };
    ipc.setFallbackEnabled.mockRejectedValue(new Error("fallback failed"));
    renderMenu(snapshot);

    fireEvent.click(
      screen.getByRole("button", { name: "Fallback 已关闭，开启 Fallback" }),
    );

    expect(await screen.findByRole("alert")).toHaveTextContent("测试失败");
    const button = screen.getByRole("button", {
      name: "Fallback 已关闭，开启 Fallback",
    });
    expect(button).toBeEnabled();
    expect(button).toHaveAttribute("aria-pressed", "false");
    expect(ipc.getMenuSnapshot).not.toHaveBeenCalled();
  });

  it("keeps unavailable Fallback focusable and guards enablement below two participants", () => {
    const snapshot = menuSnapshot();
    snapshot.bootstrap.fallback = {
      enabled: false,
      participantCount: 1,
      configRevision: 1,
      activePosition: null,
      hasNext: false,
    };
    renderMenu(snapshot);

    const button = screen.getByRole("button", {
      name: "Fallback 不可用，至少需 2 条路由",
    });
    expect(button).toHaveAttribute("aria-disabled", "true");
    expect(button).not.toBeDisabled();
    fireEvent.click(button);
    expect(ipc.setFallbackEnabled).not.toHaveBeenCalled();
  });

  it("keeps an unavailable Fallback boundary muted for a defensive enabled snapshot", () => {
    const snapshot = menuSnapshot();
    snapshot.bootstrap.fallback = {
      enabled: true,
      participantCount: 1,
      configRevision: 1,
      activePosition: null,
      hasNext: false,
    };
    const { container } = renderMenu(snapshot);

    const boundary = container.querySelector(".menu-fallback-boundary");
    expect(boundary).toHaveClass("is-disabled");
    expect(boundary).toHaveTextContent("Fallback 范围 · 已关闭");
  });

  it("shows the backend-owned Fallback non-participant and no-next warning", () => {
    const snapshot = menuSnapshot();
    snapshot.bootstrap.fallback = {
      enabled: true,
      participantCount: 3,
      configRevision: 1,
      activePosition: null,
      hasNext: false,
    };
    renderMenu(snapshot);

    const button = screen.getByRole("button", {
      name: "Fallback 已开启，关闭 Fallback；当前路由之后没有可用的 Fallback 路由",
    });
    expect(button).toHaveAttribute("aria-pressed", "true");
    expect(button).toHaveAttribute(
      "title",
      "当前路由之后没有可用的 Fallback 路由",
    );
    expect(button).toHaveClass("runtime-status-good");
    expect(button).not.toHaveClass("runtime-status-warning");
    expect(button).not.toHaveAttribute("aria-disabled");
  });

  it.each([
    ["start", 0, 0, "start", false],
    ["middle", 2, 1, "end", true],
    ["end", 4, 3, "end", true],
  ] as const)(
    "promotes exactly one existing route edge at the %s Fallback boundary",
    (_, participantCount, ownerIndex, edge, enabled) => {
      const snapshot = menuSnapshot();
      populateRoutes(snapshot, 4);
      snapshot.bootstrap.fallback = {
        enabled,
        participantCount,
        configRevision: 1,
        activePosition: enabled ? 1 : null,
        hasNext: enabled,
      };
      const { container } = renderMenu(snapshot);

      const options = screen.getAllByRole("option");
      const boundaries = container.querySelectorAll(".menu-fallback-boundary");
      expect(options).toHaveLength(4);
      expect(boundaries).toHaveLength(1);
      expect(boundaries[0]).toHaveClass(`menu-fallback-boundary-${edge}`);
      expect(boundaries[0]).toHaveTextContent(
        enabled ? "以上参与 Fallback" : "Fallback 范围 · 已关闭",
      );
      expect(boundaries[0].closest('[role="option"]')).toBe(
        options[ownerIndex],
      );
    },
  );

  it("keeps the empty-route state without rendering a Fallback boundary", () => {
    const snapshot = menuSnapshot();
    populateRoutes(snapshot, 0);
    snapshot.bootstrap.fallback = {
      enabled: false,
      participantCount: 0,
      configRevision: 1,
      activePosition: null,
      hasNext: false,
    };
    const { container } = renderMenu(snapshot);

    expect(screen.getByRole("heading", { name: "还没有路由" })).toBeVisible();
    expect(container.querySelector(".menu-fallback-boundary")).toBeNull();
  });

  it.each([
    [-3, 0, "start"],
    [99, 3, "end"],
  ] as const)(
    "clamps an out-of-range Fallback count of %s for presentation",
    (participantCount, ownerIndex, edge) => {
      const snapshot = menuSnapshot();
      populateRoutes(snapshot, 4);
      snapshot.bootstrap.fallback = {
        enabled: false,
        participantCount,
        configRevision: 1,
        activePosition: null,
        hasNext: false,
      };
      const { container } = renderMenu(snapshot);

      const boundary = container.querySelector(".menu-fallback-boundary");
      expect(boundary).toHaveClass(`menu-fallback-boundary-${edge}`);
      expect(boundary?.closest('[role="option"]')).toBe(
        screen.getAllByRole("option")[ownerIndex],
      );
    },
  );

  it("keeps the disabled Fallback boundary passive without changing route actions", async () => {
    const snapshot = menuSnapshot();
    const routes = populateRoutes(snapshot, 4);
    snapshot.bootstrap.fallback = {
      enabled: false,
      participantCount: 2,
      configRevision: 1,
      activePosition: null,
      hasNext: false,
    };
    ipc.getMenuSnapshot.mockResolvedValue(snapshot);
    ipc.previewRouteActivation.mockResolvedValueOnce({
      targetRouteId: routes[2].routeId,
      targetRouteName: routes[2].name,
      targetCatalogMode: "original",
      confirmationRequired: false,
      permit: "route-permit-3",
    });
    const { container } = renderMenu(snapshot);

    const boundary = container.querySelector(".menu-fallback-boundary");
    expect(boundary).toHaveTextContent("Fallback 范围 · 已关闭");
    expect(boundary).toHaveAttribute("aria-hidden", "true");
    expect(boundary).not.toHaveAttribute("role");
    expect(boundary).not.toHaveAttribute("tabindex");
    expect(boundary?.querySelector("button, input")).toBeNull();

    fireEvent.click(
      screen.getByRole("button", { name: "刷新 测试路由 1 的余额" }),
    );
    await waitFor(() =>
      expect(ipc.refreshBalance).toHaveBeenCalledWith(routes[0].routeId),
    );

    fireEvent.click(screen.getByRole("button", { name: "切换到 测试路由 3" }));
    await waitFor(() =>
      expect(ipc.previewRouteActivation).toHaveBeenCalledWith(
        routes[2].routeId,
      ),
    );
  });

  it("switches directly when the backend preview does not require confirmation", async () => {
    const snapshot = menuSnapshot();
    ipc.getMenuSnapshot.mockResolvedValue(snapshot);
    renderMenu(snapshot);

    fireEvent.click(screen.getByRole("button", { name: "切换到 测试路由" }));
    await waitFor(() =>
      expect(ipc.previewRouteActivation).toHaveBeenCalledWith(routeId),
    );
    expect(ipc.confirmRouteActivation).toHaveBeenCalledWith("route-permit");
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
  });

  it("keeps the menu open and reports a persisted-but-incomplete catalog switch", async () => {
    ipc.confirmRouteActivation.mockResolvedValueOnce({
      revision: 2,
      catalog: {
        models: [],
        changed: true,
        projectionApplied: false,
        retryRequired: true,
        activation: "reconnect_codex",
        errorCode: "codex_config_changed",
        retryToken: "retry-1",
      },
    });
    renderMenu(menuSnapshot());

    fireEvent.click(screen.getByRole("button", { name: "切换到 测试路由" }));

    expect(
      await screen.findByText(
        "中转已切换，但 Codex 模型列表尚未完整应用。请重新连接 Codex 后重试。",
      ),
    ).toBeInTheDocument();
    expect(ipc.hideMenu).not.toHaveBeenCalled();
  });

  it("confirms a backend-required custom catalog switch without restarting Codex", async () => {
    ipc.previewRouteActivation.mockResolvedValueOnce({
      targetRouteId: routeId,
      targetRouteName: "自定义中转",
      targetCatalogMode: "custom",
      confirmationRequired: true,
      permit: "custom-permit",
    });
    renderMenu(menuSnapshot());

    fireEvent.click(screen.getByRole("button", { name: "切换到 测试路由" }));
    const dialog = await screen.findByRole("alertdialog", {
      name: "切换到“自定义中转”？",
    });
    expect(dialog).toHaveTextContent("该中转使用自定义模型。");
    expect(dialog).toHaveTextContent(
      "切换后需要重启 Codex，模型列表才会更新。",
    );
    expect(ipc.confirmRouteActivation).not.toHaveBeenCalled();

    fireEvent.click(within(dialog).getByRole("button", { name: "取消" }));
    expect(ipc.confirmRouteActivation).not.toHaveBeenCalled();

    ipc.previewRouteActivation.mockResolvedValueOnce({
      targetRouteId: routeId,
      targetRouteName: "自定义中转",
      targetCatalogMode: "custom",
      confirmationRequired: true,
      permit: "custom-permit-2",
    });
    fireEvent.click(screen.getByRole("button", { name: "切换到 测试路由" }));
    const secondDialog = await screen.findByRole("alertdialog", {
      name: "切换到“自定义中转”？",
    });
    fireEvent.click(
      within(secondDialog).getByRole("button", { name: "切换中转" }),
    );
    await waitFor(() =>
      expect(ipc.confirmRouteActivation).toHaveBeenCalledWith(
        "custom-permit-2",
      ),
    );
  });

  it("shows a persisted fallback notice and dismisses only its notice ID", async () => {
    const snapshot = menuSnapshot();
    snapshot.codexRestartNotice = {
      noticeId: "notice-1",
      routeName: "名称很长的备用中转",
    };
    const dismissed = { ...snapshot, codexRestartNotice: null };
    ipc.getMenuSnapshot.mockResolvedValue(dismissed);
    renderMenu(snapshot);

    expect(screen.getByLabelText("Codex 模型列表更新提醒")).toHaveTextContent(
      "已自动切换至 名称很长的备用中转",
    );
    fireEvent.click(
      screen.getByRole("button", { name: "关闭 Codex 模型列表更新提醒" }),
    );
    expect(
      screen.queryByLabelText("Codex 模型列表更新提醒"),
    ).not.toBeInTheDocument();
    await waitFor(() =>
      expect(ipc.dismissCodexRestartNotice).toHaveBeenCalledWith("notice-1"),
    );
  });

  it.each([
    ["database_error" as const, "数据库不可用，代理未启动。"],
    ["port_conflict" as const, "本地代理端口已被占用。"],
  ])(
    "shows the %s bootstrap fallback when the menu snapshot fails",
    async (status, message) => {
      ipc.getMenuSnapshot.mockRejectedValueOnce(
        new Error("snapshot unavailable"),
      );
      renderMenu(undefined, bootstrap(status));

      expect(await screen.findByText(message)).toBeInTheDocument();
    },
  );

  it("refetches the snapshot on menu show without starting balance work", async () => {
    const snapshot = menuSnapshot({ activeRoute: false });
    ipc.getMenuSnapshot.mockResolvedValue(snapshot);
    renderMenu(snapshot, snapshot.bootstrap);

    await waitFor(() => expect(ipc.prepareListener).toBeTypeOf("function"));
    ipc.getMenuSnapshot.mockClear();

    act(() => ipc.prepareListener?.({ generation: 7 }));

    await waitFor(() => expect(ipc.getMenuSnapshot).toHaveBeenCalledTimes(1));
    expect(ipc.refreshBalance).not.toHaveBeenCalled();
    expect(ipc.refreshAllBalances).not.toHaveBeenCalled();
  });

  it("completes the show handshake when a hidden WebView does not schedule animation frames", async () => {
    const animationFrame = vi
      .spyOn(window, "requestAnimationFrame")
      .mockImplementation(() => 1);
    const snapshot = menuSnapshot({ activeRoute: false });
    ipc.getMenuSnapshot.mockResolvedValue(snapshot);
    renderMenu(snapshot, snapshot.bootstrap);

    await waitFor(() => expect(ipc.prepareListener).toBeTypeOf("function"));
    act(() => ipc.prepareListener?.({ generation: 10 }));

    await waitFor(() =>
      expect(ipc.completeMenuShow).toHaveBeenCalledWith(10, expect.any(Number)),
    );
    expect(ipc.completeMenuShow).toHaveBeenCalledTimes(1);
    animationFrame.mockRestore();
  });

  it.each([
    ["above", -24, 20, true],
    ["inside", 40, 88, false],
    ["below", 184, 224, true],
  ])(
    "scrolls an active route %s the viewport only when needed",
    async (_, top, bottom, scrolls) => {
      const snapshot = menuSnapshot();
      ipc.getMenuSnapshot.mockResolvedValue(snapshot);
      renderMenu(snapshot, snapshot.bootstrap);

      const scroller = screen.getByRole("listbox", { name: "路由" });
      const row = screen.getByRole("option");
      vi.spyOn(scroller, "getBoundingClientRect").mockReturnValue({
        top: 0,
        bottom: 200,
      } as DOMRect);
      vi.spyOn(row, "getBoundingClientRect").mockReturnValue({
        top,
        bottom,
      } as DOMRect);
      const scrollIntoView = vi.mocked(row.scrollIntoView);

      await waitFor(() => expect(ipc.prepareListener).toBeTypeOf("function"));
      act(() => ipc.prepareListener?.({ generation: 8 }));
      await waitFor(() =>
        expect(ipc.completeMenuShow).toHaveBeenCalledWith(
          8,
          expect.any(Number),
        ),
      );

      if (scrolls) {
        expect(scrollIntoView).toHaveBeenCalledWith({
          block: "nearest",
          behavior: "instant",
        });
      } else {
        expect(scrollIntoView).not.toHaveBeenCalled();
      }
    },
  );

  it("returns the route list to the top when there is no active route", async () => {
    const snapshot = menuSnapshot({ activeRoute: false });
    ipc.getMenuSnapshot.mockResolvedValue(snapshot);
    renderMenu(snapshot, snapshot.bootstrap);
    const scroller = screen.getByRole("listbox", { name: "路由" });
    scroller.scrollTop = 96;

    await waitFor(() => expect(ipc.prepareListener).toBeTypeOf("function"));
    act(() => ipc.prepareListener?.({ generation: 9 }));
    await waitFor(() =>
      expect(ipc.completeMenuShow).toHaveBeenCalledWith(9, expect.any(Number)),
    );

    expect(scroller.scrollTop).toBe(0);
  });

  it("keeps global refresh usable with mixed scripted routes and a partial result", () => {
    const secondRouteId = "menu-route-without-script" as RouteId;
    const snapshot = menuSnapshot();
    snapshot.bootstrap.routes.push({
      routeId: secondRouteId,
      name: "未配置脚本",
      baseUrlHost: "second.example.com",
      inferenceStatus: {
        kind: "unverified",
        lastOutcome: null,
        failureReason: null,
        observedAtMs: null,
      },
    });
    snapshot.balanceBatch = {
      batchId: "partial-batch",
      eligibleCount: 1,
      completedCount: 1,
      successCount: 0,
      failureCount: 1,
      phase: "completed",
    };
    renderMenu(snapshot);

    expect(
      screen.getByRole("button", { name: "刷新 测试路由 的余额" }),
    ).toBeEnabled();
    expect(
      screen.getByRole("button", { name: "刷新 未配置脚本 的余额" }),
    ).toBeDisabled();
    expect(
      screen.getByRole("button", { name: "已更新 0/1，1 项失败" }),
    ).toBeEnabled();
    expect(
      screen.getByRole("main", { name: "AI Router 菜单" }),
    ).toBeInTheDocument();
  });

  it("keeps route presentation to the approved one-line menu hierarchy", () => {
    const snapshot = menuSnapshot();
    snapshot.balances = [
      {
        routeId,
        value: {
          isValid: true,
          remaining: 12.5,
          used: null,
          total: null,
          unit: "USD",
          planName: null,
          invalidMessage: null,
          extra: null,
        },
        status: "fresh",
        lastSuccessAtMs: Date.now(),
        lastCompletionAtMs: Date.now(),
        nextDueAtMs: Date.now(),
        error: null,
      },
    ];
    renderMenu(snapshot);

    expect(screen.getByText("$12.50")).toBeInTheDocument();
    expect(screen.queryByText("example.com")).not.toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "刷新 测试路由 的余额" }),
    ).toBeInTheDocument();
  });

  it.each([
    ["rate_limit", "限流失败"],
    ["access_denied", "访问拒绝"],
  ] as const)(
    "shows the typed %s short reason for a route whose last inference failed",
    (failureReason, label) => {
      const snapshot = menuSnapshot();
      snapshot.bootstrap.routes[0].inferenceStatus = {
        kind: "recent_failure",
        lastOutcome: "failure",
        failureReason,
        observedAtMs: Date.now(),
      };
      renderMenu(snapshot);

      expect(screen.getByText(label)).toBeInTheDocument();
      expect(screen.queryByText("未验证")).not.toBeInTheDocument();
    },
  );

  it("keeps the cached balance and refresh time visible while its icon spins", () => {
    const snapshot = menuSnapshot();
    snapshot.balances = [
      {
        routeId,
        value: {
          isValid: true,
          remaining: 12.5,
          used: null,
          total: null,
          unit: "USD",
          planName: null,
          invalidMessage: null,
          extra: null,
        },
        status: "refreshing",
        lastSuccessAtMs: Date.now(),
        lastCompletionAtMs: Date.now(),
        nextDueAtMs: Date.now(),
        error: null,
      },
    ];
    renderMenu(snapshot);

    expect(screen.getByText("$12.50")).toBeInTheDocument();
    expect(screen.getByText(/^刷新于 \d{2}:\d{2}$/)).toBeInTheDocument();
    expect(screen.queryByText("正在刷新")).not.toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "刷新 测试路由 的余额" }).firstChild,
    ).toHaveClass("spin");
  });

  it.each([
    ["refreshing" as BalanceDisplayStatus, "尚无余额", null],
    ["fresh" as BalanceDisplayStatus, "$12.50", null],
    ["stale" as BalanceDisplayStatus, "$12.50", "待刷新"],
    ["last_good" as BalanceDisplayStatus, "$12.50", "上次结果"],
    ["failed" as BalanceDisplayStatus, "余额查询失败", null],
    ["unavailable" as BalanceDisplayStatus, "尚无余额", null],
  ])(
    "renders the %s balance state without changing row geometry",
    (status, label, meta) => {
      const snapshot = menuSnapshot();
      snapshot.balances = [
        {
          routeId,
          value:
            status === "failed" ||
            status === "unavailable" ||
            status === "refreshing"
              ? null
              : {
                  isValid: true,
                  remaining: 12.5,
                  used: null,
                  total: null,
                  unit: "$",
                  planName: null,
                  invalidMessage: null,
                  extra: null,
                },
          status,
          lastSuccessAtMs: null,
          lastCompletionAtMs: null,
          nextDueAtMs: null,
          error: null,
        },
      ];
      renderMenu(snapshot);

      expect(screen.getByText(label)).toBeInTheDocument();
      if (meta) expect(screen.getByText(meta)).toBeInTheDocument();
      expect(screen.getByRole("option")).toBeInTheDocument();
    },
  );

  it("puts stale and last-good context on a complete second line", () => {
    const successAt = new Date(2026, 0, 1, 10, 5).getTime();
    const failedAt = new Date(2026, 0, 1, 11, 10).getTime();
    const expectedSuccessTime = new Intl.DateTimeFormat("zh-CN", {
      hour: "2-digit",
      minute: "2-digit",
    }).format(successAt);
    const expectedFailedTime = new Intl.DateTimeFormat("zh-CN", {
      hour: "2-digit",
      minute: "2-digit",
    }).format(failedAt);
    const snapshot = menuSnapshot();
    snapshot.balances = [
      {
        routeId,
        value: {
          isValid: true,
          remaining: 138.42,
          used: null,
          total: null,
          unit: "$",
          planName: null,
          invalidMessage: null,
          extra: null,
        },
        status: "stale",
        lastSuccessAtMs: successAt,
        lastCompletionAtMs: successAt,
        nextDueAtMs: null,
        error: null,
      },
    ];
    const view = renderMenu(snapshot);

    expect(screen.getByText("$138.42")).toBeInTheDocument();
    expect(
      screen.getByText(`${expectedSuccessTime} · 待刷新`),
    ).toBeInTheDocument();
    expect(screen.queryByText(/已过/)).not.toBeInTheDocument();

    view.unmount();
    snapshot.balances[0] = {
      ...snapshot.balances[0],
      status: "last_good",
      lastCompletionAtMs: failedAt,
    };
    renderMenu(snapshot);

    expect(screen.getByText("$138.42")).toBeInTheDocument();
    expect(
      screen.getByText(`${expectedSuccessTime} · 上次结果`),
    ).toBeInTheDocument();
    expect(
      screen.queryByText(`${expectedFailedTime} · 上次结果`),
    ).not.toBeInTheDocument();
  });

  it("renders a successful balance without a numeric amount as available", () => {
    const snapshot = menuSnapshot();
    snapshot.balances = [
      {
        routeId,
        value: {
          isValid: true,
          remaining: null,
          used: null,
          total: null,
          unit: null,
          planName: null,
          invalidMessage: null,
          extra: null,
        },
        status: "fresh",
        lastSuccessAtMs: null,
        lastCompletionAtMs: null,
        nextDueAtMs: null,
        error: null,
      },
    ];
    renderMenu(snapshot);

    expect(screen.getByText("余额可用")).toBeInTheDocument();
  });

  it("renders all four inference states from the typed snapshot", () => {
    const snapshot = menuSnapshot();
    const states: Array<[InferenceStatusKind, string]> = [
      ["unverified", "未验证"],
      ["recent_success", "最近成功"],
      ["recent_failure", "最近失败"],
      ["expired", "状态已过期"],
    ];
    snapshot.bootstrap.routes = states.map(([kind], index) => ({
      routeId: `inference-${index}` as RouteId,
      name: `路由 ${index + 1}`,
      baseUrlHost: "example.com",
      inferenceStatus: {
        kind,
        lastOutcome:
          kind === "recent_success"
            ? "success"
            : kind === "unverified"
              ? null
              : "failure",
        failureReason: null,
        observedAtMs: kind === "unverified" ? null : Date.now(),
      },
    }));
    snapshot.bootstrap.activeRouteId = snapshot.bootstrap.routes[0].routeId;
    snapshot.balanceEnabledRouteIds = [];
    renderMenu(snapshot);

    for (const [, label] of states)
      expect(screen.getByText(label)).toBeInTheDocument();
  });

  it("shows recovery-required status without requesting or exposing menu data", () => {
    const recoveryBootstrap: BootstrapSnapshotDto = {
      ...bootstrap("database_error"),
      routes: [],
      activeRouteId: null,
      lifecycle: { phase: "recovery_required", issue: null },
    };
    renderMenu(menuSnapshot(), recoveryBootstrap);

    expect(
      screen.getByRole("heading", { name: "需要恢复数据库" }),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("listbox", { name: "路由" }),
    ).not.toBeInTheDocument();
    expect(screen.queryByText("测试路由")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "打开恢复设置" }));
    expect(ipc.showSettingsWindow).toHaveBeenCalledWith("system");
  });

  it("renders the typed fatal database label with no destructive action", () => {
    const fatalBootstrap: BootstrapSnapshotDto = {
      ...bootstrap("database_error"),
      routes: [],
      activeRouteId: null,
      lifecycle: {
        phase: "database_error",
        issue: { database: "future_schema" },
      },
    };
    renderMenu(menuSnapshot(), fatalBootstrap);

    expect(
      screen.getByRole("heading", { name: "数据库版本过新" }),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "创建空数据库" }),
    ).not.toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "打开恢复设置" }),
    ).toBeInTheDocument();
  });
});

describe("route usage hover preview", () => {
  function routeNameRegion(option: HTMLElement) {
    const region = option.querySelector<HTMLElement>(".route-identity");
    if (!region) throw new Error("route name region not found");
    return region;
  }

  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it("opens from the full name column after pointer intent", async () => {
    vi.useFakeTimers();
    renderMenu(menuSnapshot());
    const option = screen.getByRole("option", { name: /测试路由/ });
    const name = routeNameRegion(option);

    expect(name.querySelector("strong")).toHaveTextContent("测试路由");

    fireEvent.pointerEnter(name);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(439);
    });
    expect(
      screen.queryByLabelText("测试路由 用量速览"),
    ).not.toBeInTheDocument();

    await act(async () => {
      await vi.advanceTimersByTimeAsync(1);
    });
    expect(screen.getByLabelText("测试路由 用量速览")).toBeInTheDocument();
    expect(ipc.getUsageHistory).toHaveBeenCalledWith(
      expect.objectContaining({
        finishedAtOrAfterMs: null,
        finishedAtOrBeforeMs: Number.MAX_SAFE_INTEGER,
        completionState: null,
        routeId,
        modelContains: null,
        cursor: null,
        limit: 10,
      }),
    );
    expect(ipc.setMenuUsagePreview).toHaveBeenCalledWith(0, 2, true);
  });

  it("refreshes completed history in place without replacing the loaded table", async () => {
    vi.useFakeTimers();
    const previousRow = usageRow(routeId, "previous-model");
    const latestRow = usageRow(routeId, "latest-model");
    let resolveRefresh!: (page: UsageHistoryPageDto) => void;
    ipc.getUsageHistory
      .mockResolvedValueOnce({
        rows: [previousRow],
        nextCursor: null,
        totalRows: 1,
      })
      .mockImplementationOnce(
        () =>
          new Promise<UsageHistoryPageDto>((resolve) => {
            resolveRefresh = resolve;
          }),
      );
    const { client } = renderMenu(menuSnapshot());
    fireEvent.pointerEnter(
      routeNameRegion(screen.getByRole("option", { name: /测试路由/ })),
    );
    await act(async () => {
      await vi.advanceTimersByTimeAsync(440);
      await Promise.resolve();
    });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
      await Promise.resolve();
    });
    expect(screen.getByText("previous-model")).toBeInTheDocument();

    await act(async () => {
      void client.invalidateQueries({ queryKey: queryKeys.usageHistory });
      await vi.advanceTimersByTimeAsync(0);
      await Promise.resolve();
    });
    expect(screen.getByText("previous-model")).toBeInTheDocument();
    expect(screen.queryByLabelText("正在读取请求记录")).not.toBeInTheDocument();

    await act(async () => {
      resolveRefresh({ rows: [latestRow], nextCursor: null, totalRows: 1 });
      await Promise.resolve();
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(screen.getByText("latest-model")).toBeInTheDocument();
    expect(screen.queryByText("previous-model")).not.toBeInTheDocument();
  });

  it("keeps every non-name-column row region outside the trigger", async () => {
    vi.useFakeTimers();
    renderMenu(menuSnapshot());
    const option = screen.getByRole("option", { name: /测试路由/ });
    const excludedTargets = [
      option,
      option.querySelector<HTMLElement>(".route-select"),
      option.querySelector<HTMLElement>(".route-check"),
      option.querySelector<HTMLElement>(".inference"),
      option.querySelector<HTMLElement>(".balance"),
      screen.getByRole("button", { name: "刷新 测试路由 的余额" }),
      option.querySelector<HTMLElement>(".menu-fallback-boundary"),
    ];

    for (const target of excludedTargets) {
      expect(target).not.toBeNull();
      fireEvent.pointerEnter(target!);
      await act(async () => {
        await vi.advanceTimersByTimeAsync(441);
      });
      fireEvent.pointerLeave(target!);
    }

    expect(ipc.setMenuUsagePreview).not.toHaveBeenCalled();
    expect(ipc.getUsageHistory).not.toHaveBeenCalled();
    expect(
      screen.queryByLabelText("测试路由 用量速览"),
    ).not.toBeInTheDocument();
  });

  it("does not open for keyboard focus or a cancelled rapid hover", async () => {
    vi.useFakeTimers();
    renderMenu(menuSnapshot());
    const option = screen.getByRole("option", { name: /测试路由/ });
    const name = routeNameRegion(option);
    screen.getByRole("button", { name: "切换到 测试路由" }).focus();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(300);
    });
    expect(
      screen.queryByLabelText("测试路由 用量速览"),
    ).not.toBeInTheDocument();

    fireEvent.pointerEnter(name);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(100);
    });
    fireEvent.pointerLeave(name);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(300);
    });
    expect(
      screen.queryByLabelText("测试路由 用量速览"),
    ).not.toBeInTheDocument();
    expect(ipc.getUsageHistory).not.toHaveBeenCalled();
  });

  it("keeps the panel open during pointer transfer and shrinks after close grace", async () => {
    vi.useFakeTimers();
    const previewAtNativeClose = vi.fn();
    ipc.setMenuUsagePreview.mockImplementation(
      async (_generation, _revision, open) => {
        if (!open) {
          previewAtNativeClose(screen.queryByLabelText("测试路由 用量速览"));
        }
      },
    );
    renderMenu(menuSnapshot());
    const option = screen.getByRole("option", { name: /测试路由/ });
    const name = routeNameRegion(option);
    fireEvent.pointerEnter(name);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(440);
    });
    const preview = screen.getByLabelText("测试路由 用量速览");

    fireEvent.pointerLeave(name);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(100);
    });
    fireEvent.pointerEnter(preview);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(300);
    });
    expect(screen.getByLabelText("测试路由 用量速览")).toBeInTheDocument();

    fireEvent.pointerLeave(preview);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(180);
    });
    expect(screen.getByLabelText("测试路由 用量速览")).toHaveClass(
      "menu-usage-preview-closing",
    );
    await act(async () => {
      await vi.advanceTimersByTimeAsync(100);
    });
    expect(
      screen.queryByLabelText("测试路由 用量速览"),
    ).not.toBeInTheDocument();
    expect(ipc.setMenuUsagePreview).toHaveBeenLastCalledWith(0, 2, true);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1);
    });
    expect(ipc.setMenuUsagePreview).toHaveBeenLastCalledWith(0, 3, false);
    expect(previewAtNativeClose).toHaveBeenCalledWith(null);
  });

  it("switches the stable shell after the route dwell and never selects stale route data", async () => {
    vi.useFakeTimers();
    const snapshot = menuSnapshot();
    populateRoutes(snapshot, 2);
    renderMenu(snapshot);
    const options = screen.getAllByRole("option");
    fireEvent.pointerEnter(routeNameRegion(options[0]));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(440);
    });
    expect(screen.getByLabelText("测试路由 1 用量速览")).toBeInTheDocument();

    fireEvent.pointerEnter(routeNameRegion(options[1]));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(119);
    });
    expect(screen.getByLabelText("测试路由 1 用量速览")).toBeInTheDocument();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1);
    });
    expect(screen.getByLabelText("测试路由 2 用量速览")).toBeInTheDocument();
    expect(ipc.getUsageHistory).toHaveBeenLastCalledWith(
      expect.objectContaining({
        routeId: "menu-route-2",
        limit: 10,
      }),
    );
  });

  it("does not render a late response from the previously hovered route", async () => {
    vi.useFakeTimers();
    const pending = new Map<
      RouteId | null,
      (page: UsageHistoryPageDto) => void
    >();
    ipc.getUsageHistory.mockImplementation(
      (query: { routeId: RouteId | null }) =>
        new Promise<UsageHistoryPageDto>((resolve) => {
          pending.set(query.routeId, resolve);
        }),
    );
    const snapshot = menuSnapshot();
    const routes = populateRoutes(snapshot, 2);
    renderMenu(snapshot);
    const options = screen.getAllByRole("option");

    fireEvent.pointerEnter(routeNameRegion(options[0]));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(440);
    });
    fireEvent.pointerEnter(routeNameRegion(options[1]));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(120);
    });

    await act(async () => {
      pending.get(routes[0].routeId)?.({
        rows: [usageRow(routes[0].routeId, "stale-route-model")],
        nextCursor: null,
        totalRows: 1,
      });
      await Promise.resolve();
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(screen.queryByText(/stale-route-model/)).not.toBeInTheDocument();
    expect(screen.getByLabelText("测试路由 2 用量速览")).toBeInTheDocument();

    await act(async () => {
      pending.get(routes[1].routeId)?.({
        rows: [usageRow(routes[1].routeId, "current-route-model")],
        nextCursor: null,
        totalRows: 1,
      });
      await Promise.resolve();
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(screen.getByText(/current-route-model/)).toBeInTheDocument();
  });

  it("closes the visual preview when native expansion fails", async () => {
    vi.useFakeTimers();
    ipc.setMenuUsagePreview.mockRejectedValueOnce(new Error("resize failed"));
    renderMenu(menuSnapshot());

    fireEvent.pointerEnter(
      routeNameRegion(screen.getByRole("option", { name: /测试路由/ })),
    );
    await act(async () => {
      await vi.advanceTimersByTimeAsync(440);
      await Promise.resolve();
    });

    expect(
      screen.queryByLabelText("测试路由 用量速览"),
    ).not.toBeInTheDocument();
  });

  it("clears preview state and native geometry when the target route is removed", async () => {
    vi.useFakeTimers();
    const snapshot = menuSnapshot();
    const view = renderMenu(snapshot);

    fireEvent.pointerEnter(
      routeNameRegion(screen.getByRole("option", { name: /测试路由/ })),
    );
    await act(async () => {
      await vi.advanceTimersByTimeAsync(440);
    });
    expect(screen.getByLabelText("测试路由 用量速览")).toBeInTheDocument();

    const withoutRoutes: MenuSnapshotDto = {
      ...snapshot,
      bootstrap: {
        ...snapshot.bootstrap,
        routes: [],
        activeRouteId: null,
      },
      balanceEnabledRouteIds: [],
    };
    await act(async () => {
      view.client.setQueryData(queryKeys.menu, withoutRoutes);
      await vi.advanceTimersByTimeAsync(0);
      await Promise.resolve();
    });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1);
    });

    expect(
      screen.queryByLabelText("测试路由 用量速览"),
    ).not.toBeInTheDocument();
    expect(ipc.setMenuUsagePreview).toHaveBeenLastCalledWith(0, 3, false);

    act(() => view.client.setQueryData(queryKeys.menu, snapshot));
    expect(
      screen.queryByLabelText("测试路由 用量速览"),
    ).not.toBeInTheDocument();
  });

  it("cancels native expansion when the hovered route is removed during intent", async () => {
    vi.useFakeTimers();
    const snapshot = menuSnapshot();
    const view = renderMenu(snapshot);

    fireEvent.pointerEnter(
      routeNameRegion(screen.getByRole("option", { name: /测试路由/ })),
    );
    await act(async () => {
      await vi.advanceTimersByTimeAsync(100);
    });
    act(() =>
      view.client.setQueryData(queryKeys.menu, {
        ...snapshot,
        bootstrap: {
          ...snapshot.bootstrap,
          routes: [],
          activeRouteId: null,
        },
        balanceEnabledRouteIds: [],
      }),
    );
    await act(async () => {
      await vi.advanceTimersByTimeAsync(500);
    });

    expect(ipc.setMenuUsagePreview).not.toHaveBeenCalled();
    expect(
      screen.queryByLabelText("测试路由 用量速览"),
    ).not.toBeInTheDocument();
  });

  it("clears the active preview when a newer menu generation arrives", async () => {
    const snapshot = menuSnapshot();
    renderMenu(snapshot);
    await waitFor(() => expect(ipc.prepareListener).toBeTypeOf("function"));
    vi.useFakeTimers();

    fireEvent.pointerEnter(
      routeNameRegion(screen.getByRole("option", { name: /测试路由/ })),
    );
    await act(async () => {
      await vi.advanceTimersByTimeAsync(440);
    });
    expect(screen.getByLabelText("测试路由 用量速览")).toBeInTheDocument();

    act(() => ipc.prepareListener?.({ generation: 7 }));
    expect(
      screen.queryByLabelText("测试路由 用量速览"),
    ).not.toBeInTheDocument();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(500);
    });
    expect(
      screen.queryByLabelText("测试路由 用量速览"),
    ).not.toBeInTheDocument();
  });

  it.each(["resolve", "reject"] as const)(
    "ignores a late native-open %s after the preview has closed",
    async (settlement) => {
      vi.useFakeTimers();
      let resolveOpen!: () => void;
      let rejectOpen!: (reason: Error) => void;
      ipc.setMenuUsagePreview.mockImplementationOnce(
        () =>
          new Promise<void>((resolve, reject) => {
            resolveOpen = resolve;
            rejectOpen = reject;
          }),
      );
      renderMenu(menuSnapshot());
      const option = screen.getByRole("option", { name: /测试路由/ });

      fireEvent.pointerEnter(routeNameRegion(option));
      await act(async () => {
        await vi.advanceTimersByTimeAsync(440);
      });
      expect(
        screen.queryByLabelText("测试路由 用量速览"),
      ).not.toBeInTheDocument();
      expect(ipc.setMenuUsagePreview).toHaveBeenLastCalledWith(0, 2, true);
      fireEvent.pointerLeave(routeNameRegion(option));
      await act(async () => {
        await vi.advanceTimersByTimeAsync(280);
      });
      await act(async () => {
        await vi.advanceTimersByTimeAsync(1);
      });
      expect(
        screen.queryByLabelText("测试路由 用量速览"),
      ).not.toBeInTheDocument();
      expect(ipc.setMenuUsagePreview).toHaveBeenLastCalledWith(0, 3, false);

      await act(async () => {
        if (settlement === "resolve") resolveOpen();
        else rejectOpen(new Error("late resize failure"));
        await Promise.resolve();
        await vi.advanceTimersByTimeAsync(500);
      });

      expect(
        screen.queryByLabelText("测试路由 用量速览"),
      ).not.toBeInTheDocument();
      expect(ipc.setMenuUsagePreview).toHaveBeenLastCalledWith(0, 3, false);
    },
  );

  it("switches and closes directly after the retained delays under reduced motion", async () => {
    vi.stubGlobal(
      "matchMedia",
      vi.fn(() => ({
        matches: true,
        media: "(prefers-reduced-motion: reduce)",
        onchange: null,
        addEventListener: vi.fn(),
        removeEventListener: vi.fn(),
        addListener: vi.fn(),
        removeListener: vi.fn(),
        dispatchEvent: vi.fn(),
      })),
    );
    vi.useFakeTimers();
    const snapshot = menuSnapshot();
    populateRoutes(snapshot, 2);
    renderMenu(snapshot);
    const options = screen.getAllByRole("option");

    fireEvent.pointerEnter(routeNameRegion(options[0]));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(440);
    });
    expect(screen.getByLabelText("测试路由 1 用量速览")).toBeInTheDocument();

    fireEvent.pointerEnter(routeNameRegion(options[1]));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    const preview = screen.getByLabelText("测试路由 2 用量速览");
    expect(preview).toHaveClass("menu-usage-preview-open");

    fireEvent.pointerLeave(routeNameRegion(options[1]));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(180);
    });
    expect(
      screen.queryByLabelText("测试路由 2 用量速览"),
    ).not.toBeInTheDocument();
  });
});
