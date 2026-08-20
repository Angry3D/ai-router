import { beforeEach, describe, expect, it, vi } from "vitest";

const tauri = vi.hoisted(() => ({
  invoke: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({
  Channel: class {
    onmessage = undefined;
  },
  invoke: tauri.invoke,
}));

import {
  IPC_COMMANDS,
  getRouteEdit,
  getUsageHistory,
  getUsageRequestDetail,
  getUsageRouteOptions,
  normalizeIpcError,
  openProjectRepository,
} from "./ipc";
import { previewFallbackUiRouteEdits } from "../previewFixtures";

beforeEach(() => {
  tauri.invoke.mockReset();
  window.history.replaceState({}, "", "/");
});

describe("IPC error normalization", () => {
  it("uses a safe fallback for unknown errors", () => {
    const normalized = normalizeIpcError({
      secret: "must-not-leak",
      stack: "raw stack",
    });
    expect(normalized).toEqual({
      code: "ipc_failed",
      message: "Unable to complete the request.",
      retryable: true,
      field: null,
    });
  });

  it("bounds typed messages", () => {
    const normalized = normalizeIpcError({
      code: "safe_code",
      message: "x".repeat(700),
      retryable: false,
      field: "name",
    });
    expect(normalized.message).toHaveLength(512);
  });
});

describe("project repository command", () => {
  it("invokes the fixed no-argument native command", async () => {
    tauri.invoke.mockResolvedValueOnce(undefined);

    await openProjectRepository();

    expect(IPC_COMMANDS.openProjectRepository).toBe("open_project_repository");
    expect(tauri.invoke).toHaveBeenCalledOnce();
    expect(tauri.invoke).toHaveBeenCalledWith("open_project_repository");
  });
});

describe("development Usage preview", () => {
  it("selects the dedicated Fallback UI route fixture from the preview URL", async () => {
    window.history.replaceState({}, "", "/?fallback-ui=preview");

    const edit = await getRouteEdit(previewFallbackUiRouteEdits[0].routeId);

    expect(edit.fallbackExcludedModels).toEqual([
      "gpt-5.6-luna",
      "gpt-5.2-codex",
      "relay-preview-model",
    ]);
    expect(tauri.invoke).not.toHaveBeenCalled();
  });

  it("provides privacy-safe list, route, and on-demand detail fixtures", async () => {
    const page = await getUsageHistory({
      finishedAtOrAfterMs: null,
      finishedAtOrBeforeMs: Date.now(),
      completionState: null,
      routeId: null,
      modelContains: null,
      cursor: null,
      limit: 50,
    });
    const routes = await getUsageRouteOptions();
    const detail = await getUsageRequestDetail(page.rows[0].requestId);
    const details = await Promise.all(
      page.rows.map((row) => getUsageRequestDetail(row.requestId)),
    );

    expect(page.rows).toHaveLength(6);
    expect(page.rows.map((row) => row.cost.fastStatus)).toEqual([
      null,
      "confirmed",
      "unconfirmed",
      null,
      null,
      null,
    ]);
    expect(details.map((item) => item.requestedServiceTier)).toEqual([
      "priority",
      "priority",
      "priority",
      null,
      null,
      null,
    ]);
    expect(details.map((item) => item.actualServiceTier)).toEqual([
      "default",
      "priority",
      "default",
      null,
      "default",
      "default",
    ]);
    expect(routes.some((route) => route.retained)).toBe(true);
    const routePage = await getUsageHistory({
      finishedAtOrAfterMs: null,
      finishedAtOrBeforeMs: Date.now(),
      completionState: null,
      routeId: routes[0].routeId,
      modelContains: null,
      cursor: null,
      limit: 2,
    });
    expect(routePage.rows).toHaveLength(2);
    expect(routePage.totalRows).toBeGreaterThanOrEqual(routePage.rows.length);
    if (routePage.totalRows > routePage.rows.length) {
      expect(routePage.nextCursor).not.toBeNull();
    }
    expect(
      routePage.rows.every((row) => row.routeId === routes[0].routeId),
    ).toBe(true);
    expect(detail.attempts).toHaveLength(3);
    expect(
      detail.attempts.map((attempt) => attempt.forwardedServiceTier),
    ).toEqual(["priority", "priority", null]);
    expect(detail.attempts[2].cost.serviceTier).toBe("default");
    expect(detail.attempts[0].cost.fastStatus).toBe("unconfirmed");
    expect(detail.attempts[1].cost.fastStatus).toBe("unconfirmed");
    expect(detail.attempts[2].cost.fastStatus).toBeNull();
    expect(detail.request.cost.serviceTier).toBeNull();
    expect(JSON.stringify({ page, routes, detail })).not.toMatch(
      /authorization|api.?key|request.?body|response.?body|https?:\/\//i,
    );
  });

  it("keeps the menu preview command name centralized", () => {
    expect(IPC_COMMANDS.setMenuUsagePreview).toBe("set_menu_usage_preview");
  });
});
