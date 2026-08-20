import { QueryClientProvider } from "@tanstack/react-query";
import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { useState } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { createRouterQueryClient } from "../../api/query";
import type { RouteId, SettingsSnapshotDto } from "../../generated";
import { previewSettingsSnapshot } from "../../previewFixtures";
import { RoutesSettings } from "./RoutesSettings";

const ipc = vi.hoisted(() => ({
  reorderRoutesAndFallback: vi.fn(),
  setFallbackEnabled: vi.fn(),
}));

vi.mock("../../api/ipc", () => ({
  normalizeIpcError: () => ({
    code: "test",
    message: "测试失败",
    retryable: false,
    field: null,
  }),
  reorderRoutesAndFallback: ipc.reorderRoutesAndFallback,
  setFallbackEnabled: ipc.setFallbackEnabled,
}));

vi.mock("./RouteEditor", () => ({
  RouteEditor: ({
    routeId,
    newRoute,
    externalBusy,
    healthDetail,
  }: {
    routeId: RouteId | null;
    newRoute: boolean;
    externalBusy: boolean;
    healthDetail?: { text: string } | null;
  }) => (
    <section aria-label="路由编辑器" data-route-id={routeId ?? ""}>
      <span>{newRoute ? "新路由" : routeId}</span>
      {healthDetail ? <span>{healthDetail.text}</span> : null}
      <button type="button" disabled={externalBusy}>
        删除路由
      </button>
      <button type="button" disabled={externalBusy}>
        保存
      </button>
    </section>
  ),
}));

function RoutesHarness({ snapshot }: { snapshot: SettingsSnapshotDto }) {
  const [selectedRouteId, setSelectedRouteId] = useState<RouteId | null>(
    snapshot.routes[0]?.routeId ?? null,
  );
  const [newRoute, setNewRoute] = useState(false);
  return (
    <RoutesSettings
      snapshot={snapshot}
      selectedRouteId={selectedRouteId}
      newRoute={newRoute}
      editorRevision={0}
      onBeginNewRoute={() => {
        setSelectedRouteId(null);
        setNewRoute(true);
      }}
      onSelectRoute={(routeId) => {
        setSelectedRouteId(routeId);
        setNewRoute(false);
      }}
      onCancelEditor={() => undefined}
      onSaved={() => undefined}
      onDeleted={() => undefined}
      onDirtyChange={() => undefined}
    />
  );
}

function renderRoutes(snapshot: SettingsSnapshotDto = previewSettingsSnapshot) {
  const queryClient = createRouterQueryClient();
  const rendered = render(
    <QueryClientProvider client={queryClient}>
      <RoutesHarness snapshot={snapshot} />
    </QueryClientProvider>,
  );
  return { queryClient, ...rendered };
}

function rect(top: number, height: number, width = 235): DOMRect {
  return {
    top,
    bottom: top + height,
    left: 0,
    right: width,
    width,
    height,
    x: 0,
    y: top,
    toJSON: () => ({}),
  } as DOMRect;
}

function setSortableGeometry() {
  const viewport = document.querySelector<HTMLElement>(
    ".settings-route-list-viewport",
  );
  if (!viewport) throw new Error("route viewport not found");
  Object.defineProperties(viewport, {
    clientWidth: { configurable: true, value: 235 },
    clientHeight: { configurable: true, value: 240 },
    scrollHeight: { configurable: true, value: 480 },
  });
  vi.spyOn(viewport, "getBoundingClientRect").mockImplementation(() =>
    rect(0, 240),
  );

  let nextTop = 0;
  const items = Array.from(
    viewport.querySelectorAll<HTMLElement>("[data-sortable-item]"),
  );
  items.forEach((item) => {
    const itemTop = nextTop;
    const height = item.classList.contains("fallback-boundary-slot") ? 29 : 58;
    nextTop += height;
    vi.spyOn(item, "getBoundingClientRect").mockImplementation(() =>
      rect(itemTop - viewport.scrollTop, height),
    );
  });
  return { items, viewport };
}

async function keyboardStart(target: HTMLElement) {
  const item = target.closest<HTMLElement>("[data-sortable-item]");
  if (!item) throw new Error("sortable item not found");
  Object.defineProperty(item, "scrollIntoView", {
    configurable: true,
    value: vi.fn(),
  });
  target.focus();
  fireEvent.keyDown(target, { key: " ", code: "Space" });
  await waitFor(() => expect(item).toHaveClass("is-source-placeholder"));
  await act(async () => undefined);
}

function keyboardMove(target: HTMLElement, code: "ArrowUp" | "ArrowDown") {
  fireEvent.keyDown(target, { key: code, code });
}

function keyboardDrop(target: HTMLElement) {
  fireEvent.keyDown(target, { key: "Enter", code: "Enter" });
}

beforeEach(() => {
  ipc.reorderRoutesAndFallback.mockReset();
  ipc.setFallbackEnabled.mockReset();
  ipc.reorderRoutesAndFallback.mockResolvedValue({ revision: 14 });
  ipc.setFallbackEnabled.mockResolvedValue({ revision: 15 });
});

describe("RoutesSettings unified dragging", () => {
  it("keeps the title drag band and route selection separate from handles", () => {
    renderRoutes();
    const routeList = screen.getByRole("region", { name: "路由列表" });
    const select = screen.getByRole("button", {
      name: /AI INPUT 个人账号.*ai\.input\.im/,
    });
    const handle = screen.getByRole("button", {
      name: "拖动调整路由顺序：AI INPUT 个人账号",
    });

    expect(
      routeList.querySelector(".route-list-top-drag-region"),
    ).toHaveAttribute("data-tauri-drag-region");
    expect(select.closest(".settings-route-row")).not.toBeNull();
    expect(handle.closest(".settings-route-row")).toBe(
      select.closest(".settings-route-row"),
    );
    expect(select.contains(handle)).toBe(false);
    expect(handle.compareDocumentPosition(select)).toBe(
      Node.DOCUMENT_POSITION_FOLLOWING,
    );
    expect(screen.getAllByTitle("拖动调整路由顺序")).toHaveLength(
      previewSettingsSnapshot.routes.length,
    );

    fireEvent.click(select);
    expect(select.closest(".settings-route-row")).toHaveClass("selected");
    expect(screen.getByRole("region", { name: "路由编辑器" })).toHaveAttribute(
      "data-route-id",
      previewSettingsSnapshot.routes[1].routeId,
    );
    expect(ipc.reorderRoutesAndFallback).not.toHaveBeenCalled();
  });

  it("renders the persisted prefix and a whole-bar sortable boundary", () => {
    renderRoutes({
      ...previewSettingsSnapshot,
      fallback: {
        ...previewSettingsSnapshot.fallback,
        participantCount: 2,
      },
    });

    expect(screen.getByText("Fallback 1")).toBeInTheDocument();
    expect(screen.getByText("Fallback 2")).toBeInTheDocument();
    expect(screen.queryByText("Fallback 3")).not.toBeInTheDocument();
    const boundary = screen.getByRole("button", {
      name: "拖动调整 Fallback 参与分界",
    });
    expect(boundary).toHaveAccessibleDescription(
      `当前 2 条路由参与 Fallback，共 ${previewSettingsSnapshot.routes.length} 条路由`,
    );
    expect(
      within(boundary).getByText("以下不参与 Fallback"),
    ).toBeInTheDocument();
  });

  it("routes toolbar moves through one complete atomic payload", async () => {
    renderRoutes({
      ...previewSettingsSnapshot,
      fallback: {
        ...previewSettingsSnapshot.fallback,
        participantCount: 2,
      },
    });
    fireEvent.click(
      screen.getByRole("button", { name: /AI INPUT 个人账号.*ai\.input\.im/ }),
    );
    fireEvent.click(screen.getByRole("button", { name: "上移所选路由" }));

    await waitFor(() =>
      expect(ipc.reorderRoutesAndFallback).toHaveBeenCalledWith({
        orderedRouteIds: [
          previewSettingsSnapshot.routes[1].routeId,
          previewSettingsSnapshot.routes[0].routeId,
          ...previewSettingsSnapshot.routes
            .slice(2)
            .map((route) => route.routeId),
        ],
        participantCount: 2,
        expectedConfigRevision: previewSettingsSnapshot.fallback.configRevision,
      }),
    );
    expect(ipc.reorderRoutesAndFallback).toHaveBeenCalledTimes(1);
  });

  it("moves a route across the boundary with keyboard preview and one drop", async () => {
    ipc.reorderRoutesAndFallback.mockImplementationOnce(async () => {
      (document.activeElement as HTMLElement | null)?.blur();
      return { revision: 14 };
    });
    renderRoutes({
      ...previewSettingsSnapshot,
      fallback: {
        ...previewSettingsSnapshot.fallback,
        participantCount: 2,
      },
    });
    setSortableGeometry();
    const handle = screen.getByRole("button", {
      name: "拖动调整路由顺序：Ciii 主用",
    });

    await keyboardStart(handle);
    keyboardMove(handle, "ArrowUp");
    await waitFor(() =>
      expect(screen.getAllByText("Fallback 3")).toHaveLength(2),
    );
    expect(
      document.querySelector("[data-route-drag-overlay]"),
    ).toBeInTheDocument();
    expect(
      document.querySelector("[data-route-drag-announcement]"),
    ).toHaveTextContent("参与 Fallback，序号 3");
    keyboardDrop(handle);

    await waitFor(() =>
      expect(ipc.reorderRoutesAndFallback).toHaveBeenCalledWith({
        orderedRouteIds: previewSettingsSnapshot.routes.map(
          (route) => route.routeId,
        ),
        participantCount: 3,
        expectedConfigRevision: previewSettingsSnapshot.fallback.configRevision,
      }),
    );
    expect(ipc.reorderRoutesAndFallback).toHaveBeenCalledTimes(1);
    await waitFor(() =>
      expect(
        screen.getByRole("button", {
          name: "拖动调整路由顺序：Ciii 主用",
        }),
      ).toHaveFocus(),
    );
  });

  it("moves the boundary with the keyboard without changing route order", async () => {
    renderRoutes();
    setSortableGeometry();
    const boundary = screen.getByRole("button", {
      name: "拖动调整 Fallback 参与分界",
    });

    await keyboardStart(boundary);
    keyboardMove(boundary, "ArrowDown");
    await waitFor(() =>
      expect(screen.getByText("Fallback 4")).toBeInTheDocument(),
    );
    keyboardDrop(boundary);

    await waitFor(() =>
      expect(ipc.reorderRoutesAndFallback).toHaveBeenCalledWith({
        orderedRouteIds: previewSettingsSnapshot.routes.map(
          (route) => route.routeId,
        ),
        participantCount: 4,
        expectedConfigRevision: previewSettingsSnapshot.fallback.configRevision,
      }),
    );
  });

  it("cancels keyboard preview and suppresses no-op persistence", async () => {
    renderRoutes({
      ...previewSettingsSnapshot,
      fallback: {
        ...previewSettingsSnapshot.fallback,
        participantCount: 2,
      },
    });
    setSortableGeometry();
    const handle = screen.getByRole("button", {
      name: "拖动调整路由顺序：Ciii 主用",
    });

    await keyboardStart(handle);
    keyboardMove(handle, "ArrowUp");
    await waitFor(() =>
      expect(screen.getAllByText("Fallback 3")).toHaveLength(2),
    );
    fireEvent.keyDown(handle, { key: "Escape", code: "Escape" });
    await waitFor(() =>
      expect(screen.queryAllByText("Fallback 3")).toHaveLength(0),
    );
    const restoredHandle = screen.getByRole("button", {
      name: "拖动调整路由顺序：Ciii 主用",
    });
    expect(restoredHandle).toHaveFocus();
    expect(
      document.querySelector("[data-route-drag-overlay]"),
    ).not.toBeInTheDocument();
    expect(ipc.reorderRoutesAndFallback).not.toHaveBeenCalled();

    await keyboardStart(restoredHandle);
    keyboardDrop(restoredHandle);
    expect(ipc.reorderRoutesAndFallback).not.toHaveBeenCalled();
  });

  it("uses the 8px pointer activation and cancels without persistence", async () => {
    renderRoutes();
    setSortableGeometry();
    const handle = screen.getByRole("button", {
      name: "拖动调整路由顺序：AI INPUT 工作账号",
    });

    fireEvent.pointerDown(handle, {
      button: 0,
      buttons: 1,
      clientX: 220,
      clientY: 29,
      isPrimary: true,
      pointerId: 8,
    });
    fireEvent.pointerMove(document, {
      buttons: 1,
      clientX: 220,
      clientY: 36,
      isPrimary: true,
      pointerId: 8,
    });
    expect(
      document.querySelector("[data-route-drag-overlay]"),
    ).not.toBeInTheDocument();

    fireEvent.pointerMove(document, {
      buttons: 1,
      clientX: 220,
      clientY: 38,
      isPrimary: true,
      pointerId: 8,
    });
    await waitFor(() =>
      expect(
        document.querySelector("[data-route-drag-overlay]"),
      ).toBeInTheDocument(),
    );
    fireEvent.pointerCancel(document, { pointerId: 8 });
    await waitFor(() =>
      expect(
        document.querySelector("[data-route-drag-overlay]"),
      ).not.toBeInTheDocument(),
    );
    expect(ipc.reorderRoutesAndFallback).not.toHaveBeenCalled();
  });

  it("keeps the candidate pending, then rolls back to a compact safe error", async () => {
    let rejectMutation: ((reason: unknown) => void) | undefined;
    ipc.reorderRoutesAndFallback.mockReturnValueOnce(
      new Promise((_resolve, reject) => {
        rejectMutation = reject;
      }),
    );
    renderRoutes();
    setSortableGeometry();
    const boundary = screen.getByRole("button", {
      name: "拖动调整 Fallback 参与分界",
    });

    await keyboardStart(boundary);
    keyboardMove(boundary, "ArrowDown");
    keyboardDrop(boundary);
    await waitFor(() =>
      expect(ipc.reorderRoutesAndFallback).toHaveBeenCalledTimes(1),
    );
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "新建路由" })).toBeDisabled(),
    );
    expect(
      screen.getByRole("button", {
        name: "拖动调整 Fallback 参与分界",
      }),
    ).toHaveAttribute("aria-busy", "true");
    expect(screen.getByRole("button", { name: "删除路由" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "保存" })).toBeDisabled();
    expect(screen.getByText("Fallback 4")).toBeInTheDocument();

    rejectMutation?.(new Error("private detail"));
    await waitFor(() =>
      expect(
        screen.getByRole("alert", { name: "路由设置失败：测试失败" }),
      ).toBeInTheDocument(),
    );
    expect(screen.queryByText("Fallback 4")).not.toBeInTheDocument();
    expect(screen.queryByText("设置失败")).not.toBeInTheDocument();
    expect(
      screen.getByRole("button", {
        name: "拖动调整 Fallback 参与分界",
      }),
    ).toHaveAttribute("aria-busy", "false");
  });

  it("cancels an active preview when the authoritative revision changes", async () => {
    const { rerender, queryClient } = renderRoutes();
    setSortableGeometry();
    const boundary = screen.getByRole("button", {
      name: "拖动调整 Fallback 参与分界",
    });
    await keyboardStart(boundary);
    keyboardMove(boundary, "ArrowDown");
    await screen.findByText("Fallback 4");

    const nextSnapshot = {
      ...previewSettingsSnapshot,
      fallback: {
        ...previewSettingsSnapshot.fallback,
        configRevision: previewSettingsSnapshot.fallback.configRevision + 1,
      },
    };
    rerender(
      <QueryClientProvider client={queryClient}>
        <RoutesHarness snapshot={nextSnapshot} />
      </QueryClientProvider>,
    );

    await waitFor(() =>
      expect(screen.queryByText("Fallback 4")).not.toBeInTheDocument(),
    );
    expect(
      document.querySelector("[data-fallback-boundary-overlay]"),
    ).not.toBeInTheDocument();
    expect(ipc.reorderRoutesAndFallback).not.toHaveBeenCalled();
  });

  it("keeps Fallback unavailable below two participants and retains help", () => {
    renderRoutes({
      ...previewSettingsSnapshot,
      fallback: {
        ...previewSettingsSnapshot.fallback,
        enabled: false,
        participantCount: 1,
      },
    });

    expect(
      screen.getByRole("switch", { name: "自动 Fallback" }),
    ).toBeDisabled();
    const help = screen.getByRole("button", {
      name: "说明自动 Fallback 切换规则",
    });
    fireEvent.focus(help);
    const tooltip = screen.getByRole("tooltip");
    expect(tooltip.tagName).toBe("DIV");
    expect(tooltip.querySelectorAll("li")).toHaveLength(3);
    expect(tooltip).toHaveTextContent(
      "5 次可归因失败后，按顺序切换后续路由。兼容请求验证成功 2 次后，恢复更前路由。手动切换后，重新计数。",
    );
  });

  it("keeps route health in the existing marker and selected title detail", () => {
    const activeRoute = previewSettingsSnapshot.routes[0];
    renderRoutes({
      ...previewSettingsSnapshot,
      routes: previewSettingsSnapshot.routes.map((route) =>
        route.routeId === activeRoute.routeId
          ? {
              ...route,
              health: { kind: "striking", failureCount: 3 },
            }
          : route,
      ),
    });

    expect(screen.getByText("当前 · 3/5")).toHaveClass("is-warning");
    const healthDetail = screen.getByText(
      "已累计失败 3/5 · 仍使用当前路由，不切换",
    );
    expect(healthDetail).toBeInTheDocument();
    expect(healthDetail).not.toHaveAttribute("role", "status");
    expect(
      screen.getByRole("button", {
        name: new RegExp(
          `${activeRoute.name}.*已累计失败 3/5 · 仍使用当前路由，不切换`,
          "u",
        ),
      }),
    ).toBeInTheDocument();
  });
});
