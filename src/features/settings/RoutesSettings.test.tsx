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
} from "../../generated";
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

function setRouteBoundaryGeometry() {
  const viewport = document.querySelector<HTMLElement>(
    ".settings-route-list-viewport",
  );
  if (!viewport) throw new Error("route viewport not found");
  vi.spyOn(viewport, "getBoundingClientRect").mockReturnValue({
    top: 0,
    bottom: 240,
    left: 0,
    right: 240,
    width: 240,
    height: 240,
    x: 0,
    y: 0,
    toJSON: () => ({}),
  });
  const rows = Array.from(
    viewport.querySelectorAll<HTMLElement>("[data-fallback-route-index]"),
  );
  rows.forEach((row, index) => {
    vi.spyOn(row, "getBoundingClientRect").mockImplementation(() => {
      const top = index * 50 - viewport.scrollTop;
      return {
        top,
        bottom: top + 40,
        left: 0,
        right: 240,
        width: 240,
        height: 40,
        x: 0,
        y: top,
        toJSON: () => ({}),
      };
    });
  });
  const boundary = screen.getByRole("slider", {
    name: "Fallback 参与分界",
  });
  vi.spyOn(boundary, "getBoundingClientRect").mockReturnValue({
    top: 40,
    bottom: 69,
    left: 0,
    right: 240,
    width: 240,
    height: 29,
    x: 0,
    y: 40,
    toJSON: () => ({}),
  });
  Object.assign(boundary, {
    setPointerCapture: vi.fn(),
    releasePointerCapture: vi.fn(),
  });
  return { boundary, rows, viewport };
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

describe("RoutesSettings interactions", () => {
  it("fills the route-list top with drag surface without marking its action", async () => {
    await renderSettings();

    const title = screen.getByRole("heading", { name: "路由", level: 2 });
    const routeList = screen.getByRole("region", { name: "路由列表" });
    expect(title).toHaveAttribute("data-tauri-drag-region");
    expect(title.parentElement).toHaveAttribute("data-tauri-drag-region");
    expect(
      routeList.querySelector(".route-list-top-drag-region"),
    ).toHaveAttribute("data-tauri-drag-region");
    expect(
      screen.getByRole("button", { name: "新建路由" }),
    ).not.toHaveAttribute("data-tauri-drag-region");
  });

  it("keeps edit selection separate from the active route and new mode adds no temporary row", async () => {
    await renderSettings();
    const activeRow = screen.getByRole("button", {
      name: /AI INPUT 工作账号ai\.input\.imFallback 1当前/,
    });
    const personalRow = screen.getByRole("button", {
      name: /AI INPUT 个人账号ai\.input\.imFallback 2/,
    });

    fireEvent.click(personalRow);
    expect(activeRow).not.toHaveClass("selected");
    expect(personalRow).toHaveClass("selected");
    expect(within(activeRow).getByText("当前")).toBeInTheDocument();

    const rowCount = screen.getAllByRole("button", {
      name: /ai\.input\.im|codex\.ciii\.club/,
    }).length;
    fireEvent.click(screen.getByRole("button", { name: "新建路由" }));
    expect(
      screen.getByRole("heading", { name: "新建路由" }),
    ).toBeInTheDocument();
    expect(
      screen.getAllByRole("button", {
        name: /ai\.input\.im|codex\.ciii\.club/,
      }),
    ).toHaveLength(rowCount);
    expect(within(activeRow).getByText("当前")).toBeInTheDocument();
  });

  it("uses the persisted boundary for route markers and Fallback eligibility", async () => {
    await renderSettings({
      settings: {
        fallback: {
          ...previewSettingsSnapshot.fallback,
          participantCount: 2,
        },
      },
    });

    expect(screen.getByText("Fallback 1")).toBeInTheDocument();
    expect(screen.getByText("Fallback 2")).toBeInTheDocument();
    expect(screen.queryByText("Fallback 3")).not.toBeInTheDocument();
    expect(
      screen.getByRole("slider", { name: "Fallback 参与分界" }),
    ).toHaveAttribute("aria-valuenow", "2");
    expect(screen.getByRole("switch", { name: "自动 Fallback" })).toBeChecked();

    fireEvent.click(screen.getByRole("button", { name: /AI INPUT 个人账号/ }));
    fireEvent.click(screen.getByRole("button", { name: "上移所选路由" }));
    await waitFor(() =>
      expect(ipc.moveRoute).toHaveBeenCalledWith(
        previewRouteEdits[1].routeId,
        "up",
      ),
    );

    fireEvent.click(screen.getByRole("switch", { name: "自动 Fallback" }));
    await waitFor(() =>
      expect(ipc.setFallbackEnabled).toHaveBeenCalledWith(false),
    );
  });

  it("explains forward-only Fallback on hover and keyboard focus", async () => {
    await renderSettings();
    const copy =
      "请求失败且符合切换条件时，将按顺序尝试后续路由；到最后一条后停止，不会回到前面的路由。";
    const trigger = screen.getByRole("button", {
      name: "说明自动 Fallback 切换规则",
    });
    const owner = trigger.closest(".settings-help-tooltip");
    if (!owner) throw new Error("Fallback help tooltip owner not found");

    expect(screen.queryByRole("tooltip")).not.toBeInTheDocument();
    fireEvent.mouseEnter(owner);
    expect(screen.getByRole("tooltip")).toHaveTextContent(copy);
    expect(trigger).toHaveAttribute(
      "aria-describedby",
      screen.getByRole("tooltip").id,
    );
    fireEvent.mouseLeave(owner);
    expect(screen.queryByRole("tooltip")).not.toBeInTheDocument();

    fireEvent.focus(trigger);
    expect(screen.getByRole("tooltip")).toHaveTextContent(copy);
    fireEvent.blur(trigger);
    expect(screen.queryByRole("tooltip")).not.toBeInTheDocument();
  });

  it("moves the whole-bar boundary with the keyboard and commits one mutation", async () => {
    const fifthRoute = {
      ...previewSettingsSnapshot.routes[0],
      routeId: "preview-excluded" as RouteId,
      name: "第五条路由",
    };
    await renderSettings({
      settings: {
        routes: [...previewSettingsSnapshot.routes, fifthRoute],
        fallback: {
          ...previewSettingsSnapshot.fallback,
          participantCount: 3,
        },
      },
    });

    const boundary = screen.getByRole("slider", { name: "Fallback 参与分界" });
    expect(boundary).toHaveAttribute("aria-valuetext", "3 条路由参与 Fallback");
    fireEvent.keyDown(boundary, { key: "ArrowDown" });

    expect(boundary).toHaveAttribute("aria-valuenow", "4");
    expect(screen.getByText("Fallback 4")).toBeInTheDocument();
    await waitFor(() =>
      expect(ipc.setFallbackParticipantCount).toHaveBeenCalledWith(4),
    );
    expect(ipc.setFallbackParticipantCount).toHaveBeenCalledTimes(1);
  });

  it("does not preview, lock, or persist the fallback boundary on click", async () => {
    await renderSettings({
      settings: {
        routes: previewSettingsSnapshot.routes.slice(0, 2),
        fallback: {
          ...previewSettingsSnapshot.fallback,
          participantCount: 1,
        },
      },
    });
    const { boundary } = setRouteBoundaryGeometry();
    const boundaryParent = boundary.parentElement;

    fireEvent.pointerDown(boundary, {
      button: 0,
      clientY: 45,
      isPrimary: true,
      pointerId: 26,
    });
    fireEvent.pointerMove(window, {
      buttons: 1,
      clientY: 48,
      isPrimary: true,
      pointerId: 26,
    });

    expect(boundary).not.toHaveClass("is-detached-sensor");
    expect(boundary.parentElement).toBe(boundaryParent);
    expect(boundary).toHaveAttribute("aria-valuenow", "1");
    expect(screen.getByText("Fallback 1")).toBeInTheDocument();
    expect(screen.queryByText("Fallback 2")).not.toBeInTheDocument();
    expect(
      document.querySelector("[data-fallback-boundary-preview]"),
    ).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "新建路由" })).toBeEnabled();

    fireEvent.pointerUp(window, {
      button: 0,
      clientY: 48,
      isPrimary: true,
      pointerId: 26,
    });

    expect(ipc.setFallbackParticipantCount).not.toHaveBeenCalled();
    expect(boundary).not.toHaveClass("is-detached-sensor");
    expect(
      document.querySelector("[data-fallback-boundary-preview]"),
    ).not.toBeInTheDocument();
  });

  it("keeps one stable slider sensor while a passive preview snaps 1→2 and back", async () => {
    await renderSettings({
      settings: {
        routes: previewSettingsSnapshot.routes.slice(0, 2),
        fallback: {
          ...previewSettingsSnapshot.fallback,
          participantCount: 1,
        },
      },
    });
    const { boundary } = setRouteBoundaryGeometry();
    const label = within(boundary).getByText("以下不参与 Fallback");
    const boundaryParent = boundary.parentElement;
    const sensorMutations: MutationRecord[] = [];
    const touchesSensor = (record: MutationRecord) =>
      [...record.addedNodes, ...record.removedNodes].some(
        (node) => node === boundary || node.contains(boundary),
      );
    const observer = new MutationObserver((records) => {
      sensorMutations.push(...records.filter(touchesSensor));
    });
    observer.observe(document.body, { childList: true, subtree: true });

    fireEvent.pointerDown(label, {
      button: 0,
      clientY: 45,
      isPrimary: true,
      pointerId: 24,
    });

    expect(screen.getAllByRole("slider")).toEqual([boundary]);
    expect(boundary).not.toHaveClass("is-detached-sensor");
    expect(
      document.querySelectorAll("[data-fallback-boundary-preview]"),
    ).toHaveLength(0);

    fireEvent.pointerMove(window, {
      buttons: 1,
      clientY: 80,
      isPrimary: true,
      pointerId: 24,
    });

    expect(boundary).toHaveClass("is-detached-sensor");
    expect(
      document.querySelectorAll("[data-fallback-boundary-preview]"),
    ).toHaveLength(1);
    expect(
      document.querySelectorAll(".fallback-boundary:not(.is-detached-sensor)"),
    ).toHaveLength(1);

    expect(screen.getByRole("slider")).toBe(boundary);
    expect(boundary.parentElement).toBe(boundaryParent);
    expect(boundary).toHaveAttribute("aria-valuenow", "2");
    expect(screen.getByText("Fallback 2")).toBeInTheDocument();
    let preview = document.querySelector<HTMLElement>(
      "[data-fallback-boundary-preview]",
    );
    expect(preview).toHaveAttribute("aria-hidden", "true");
    expect(preview?.parentElement).toHaveStyle({ order: "4" });

    fireEvent.pointerMove(window, {
      buttons: 1,
      clientY: 0,
      isPrimary: true,
      pointerId: 24,
    });

    expect(screen.getByRole("slider")).toBe(boundary);
    expect(boundary.parentElement).toBe(boundaryParent);
    expect(boundary).toHaveAttribute("aria-valuenow", "0");
    expect(screen.queryByText("Fallback 1")).not.toBeInTheDocument();
    preview = document.querySelector<HTMLElement>(
      "[data-fallback-boundary-preview]",
    );
    expect(preview?.parentElement).toHaveStyle({ order: "0" });

    fireEvent.pointerMove(window, {
      buttons: 1,
      clientY: 80,
      isPrimary: true,
      pointerId: 24,
    });
    sensorMutations.push(...observer.takeRecords().filter(touchesSensor));
    observer.disconnect();
    expect(sensorMutations).toHaveLength(0);
    fireEvent.pointerUp(window, {
      button: 0,
      clientY: 80,
      isPrimary: true,
      pointerId: 24,
    });

    await waitFor(() =>
      expect(ipc.setFallbackParticipantCount).toHaveBeenCalledWith(2),
    );
    expect(ipc.setFallbackParticipantCount).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("slider")).toBe(boundary);
    expect(
      document.querySelector("[data-fallback-boundary-preview]"),
    ).not.toBeInTheDocument();
  });

  it("rolls drag preview and conflicting locks back when the Routes page unmounts", async () => {
    await renderSettings();
    const boundary = screen.getByRole("slider", { name: "Fallback 参与分界" });
    Object.assign(boundary, { setPointerCapture: vi.fn() });

    fireEvent.pointerDown(boundary, {
      button: 0,
      clientY: 0,
      isPrimary: true,
      pointerId: 21,
    });
    fireEvent.pointerMove(window, {
      buttons: 1,
      clientY: 4,
      isPrimary: true,
      pointerId: 21,
    });
    expect(screen.getByRole("button", { name: "新建路由" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "下移所选路由" })).toBeDisabled();
    expect(
      screen.getByRole("switch", { name: "自动 Fallback" }),
    ).toBeDisabled();
    expect(screen.getByLabelText("路由名称")).toBeDisabled();
    expect(screen.getByRole("button", { name: "删除路由" })).toBeDisabled();

    fireEvent.click(screen.getByRole("button", { name: "用量" }));
    expect(
      screen.getByRole("heading", { name: "用量", level: 2 }),
    ).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "路由" }));

    await screen.findByLabelText("路由名称");
    expect(screen.getByRole("button", { name: "新建路由" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "下移所选路由" })).toBeEnabled();
    expect(screen.getByRole("switch", { name: "自动 Fallback" })).toBeEnabled();
    expect(
      screen.getByRole("slider", { name: "Fallback 参与分界" }),
    ).toHaveAttribute("aria-valuenow", "3");
  });

  it("locks the empty-state route creation entry while the boundary is dragging", async () => {
    const { client } = await renderSettings();
    act(() => {
      client.setQueryData(queryKeys.settings, {
        ...previewSettingsSnapshot,
        routes: [],
        activeRouteId: null,
        fallback: {
          enabled: false,
          participantCount: 0,
          activePosition: null,
          hasNext: false,
        },
      });
    });
    const addRoute = await screen.findByRole("button", { name: "添加路由" });
    const boundary = screen.getByRole("slider", { name: "Fallback 参与分界" });
    Object.assign(boundary, { setPointerCapture: vi.fn() });

    fireEvent.pointerDown(boundary, {
      button: 0,
      clientY: 0,
      isPrimary: true,
      pointerId: 22,
    });
    fireEvent.pointerMove(window, {
      buttons: 1,
      clientY: 4,
      isPrimary: true,
      pointerId: 22,
    });
    expect(screen.getByRole("button", { name: "新建路由" })).toBeDisabled();
    expect(addRoute).toBeDisabled();

    fireEvent.pointerCancel(boundary, { pointerId: 22 });
    expect(addRoute).toBeEnabled();
  });

  it("announces pending boundary persistence and locks every conflicting write", async () => {
    let resolveMutation: ((value: { revision: number }) => void) | undefined;
    ipc.setFallbackParticipantCount.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolveMutation = resolve;
        }),
    );
    await renderSettings();
    fireEvent.change(screen.getByLabelText("路由名称"), {
      target: { value: "待保存路由名称" },
    });
    expect(screen.getByRole("button", { name: "保存" })).toBeEnabled();

    const boundary = screen.getByRole("slider", { name: "Fallback 参与分界" });
    fireEvent.keyDown(boundary, { key: "Home" });

    await waitFor(() => expect(boundary).toHaveAttribute("aria-busy", "true"));
    expect(boundary).toHaveAttribute(
      "aria-valuetext",
      "0 条路由参与 Fallback，正在保存",
    );
    expect(screen.getByRole("button", { name: "新建路由" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "下移所选路由" })).toBeDisabled();
    expect(
      screen.getByRole("switch", { name: "自动 Fallback" }),
    ).toBeDisabled();
    expect(screen.getByLabelText("路由名称")).toBeDisabled();
    expect(screen.getByRole("button", { name: "删除路由" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "保存" })).toBeDisabled();

    resolveMutation?.({ revision: 16 });
    await waitFor(() => expect(boundary).toHaveAttribute("aria-busy", "false"));
    expect(screen.getByRole("button", { name: "保存" })).toBeEnabled();
  });

  it("keeps the switch disabled while fewer than two routes participate", async () => {
    await renderSettings({
      settings: {
        fallback: {
          enabled: false,
          participantCount: 1,
          activePosition: 1,
          hasNext: false,
        },
      },
    });

    expect(
      screen.getByRole("switch", { name: "自动 Fallback" }),
    ).toBeDisabled();
    expect(
      screen.getByRole("slider", { name: "Fallback 参与分界" }),
    ).toHaveAttribute("aria-valuenow", "1");
    const help = screen.getByRole("button", {
      name: "说明自动 Fallback 切换规则",
    });
    expect(help).toBeEnabled();
    fireEvent.focus(help);
    expect(screen.getByRole("tooltip")).toHaveTextContent(
      "请求失败且符合切换条件时，将按顺序尝试后续路由；到最后一条后停止，不会回到前面的路由。",
    );
  });

  it("restores the confirmed boundary and exposes a compact accessible error", async () => {
    ipc.setFallbackParticipantCount.mockRejectedValueOnce(
      new Error("private detail"),
    );
    await renderSettings({
      settings: {
        fallback: {
          ...previewSettingsSnapshot.fallback,
          participantCount: 3,
        },
      },
    });

    const { boundary } = setRouteBoundaryGeometry();
    fireEvent.pointerDown(boundary, {
      button: 0,
      clientY: 145,
      isPrimary: true,
      pointerId: 25,
    });
    fireEvent.pointerMove(window, {
      buttons: 1,
      clientY: 0,
      isPrimary: true,
      pointerId: 25,
    });
    expect(boundary).toHaveAttribute("aria-valuenow", "0");
    expect(
      document.querySelector("[data-fallback-boundary-preview]"),
    ).toBeInTheDocument();
    fireEvent.pointerUp(window, {
      button: 0,
      clientY: 0,
      isPrimary: true,
      pointerId: 25,
    });
    await waitFor(() =>
      expect(
        screen.getByRole("alert", { name: "路由设置失败：测试失败" }),
      ).toBeInTheDocument(),
    );
    expect(boundary).toHaveAttribute("aria-valuenow", "3");
    expect(boundary).not.toHaveClass("is-detached-sensor", "is-pending");
    expect(
      document.querySelector("[data-fallback-boundary-preview]"),
    ).not.toBeInTheDocument();
    expect(screen.queryByText("设置失败")).not.toBeInTheDocument();
    fireEvent.focus(
      screen.getByRole("button", { name: "说明自动 Fallback 切换规则" }),
    );
    expect(screen.getByRole("tooltip")).toBeInTheDocument();
    expect(
      screen.getByRole("alert", { name: "路由设置失败：测试失败" }),
    ).toBeInTheDocument();
  });
});
