import { QueryClientProvider } from "@tanstack/react-query";
import { act, render, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { StateChangedEventDto } from "../generated";
import {
  createRouterQueryClient,
  queryKeys,
  useRouterStateSync,
} from "./query";

const ipc = vi.hoisted(() => ({
  listener: undefined as ((event: StateChangedEventDto) => void) | undefined,
  unlisten: vi.fn(),
}));

vi.mock("./ipc", () => ({
  getBootstrapSnapshot: vi.fn(),
  isTauriRuntime: () => true,
  listenStateChanged: vi.fn(
    async (listener: (event: StateChangedEventDto) => void) => {
      ipc.listener = listener;
      return ipc.unlisten;
    },
  ),
}));

function StateSyncProbe({ view = "menu" }: { view?: "menu" | "settings" }) {
  useRouterStateSync(view);
  return null;
}

describe("router state synchronization", () => {
  it("ignores stale events and heals event loss on focus", async () => {
    const client = createRouterQueryClient();
    const invalidate = vi.spyOn(client, "invalidateQueries");
    const result = render(
      <QueryClientProvider client={client}>
        <StateSyncProbe />
      </QueryClientProvider>,
    );
    await waitFor(() => expect(ipc.listener).toBeDefined());

    act(() => {
      ipc.listener?.({ revision: 2, areas: ["routes"] });
      ipc.listener?.({ revision: 2, areas: ["proxy"] });
      ipc.listener?.({ revision: 1, areas: ["balance"] });
    });
    expect(invalidate).toHaveBeenCalledTimes(4);
    expect(invalidate).toHaveBeenCalledWith({ queryKey: queryKeys.routes });
    expect(invalidate).toHaveBeenCalledWith({ queryKey: queryKeys.bootstrap });
    expect(invalidate).toHaveBeenCalledWith({ queryKey: queryKeys.menu });
    expect(invalidate).toHaveBeenCalledWith({ queryKey: queryKeys.settings });

    act(() => {
      ipc.listener?.({ revision: 3, areas: ["balance_settings"] });
    });
    expect(invalidate).toHaveBeenCalledWith({
      queryKey: queryKeys.balanceSettings,
    });
    expect(invalidate).toHaveBeenCalledWith({ queryKey: queryKeys.settings });

    act(() => window.dispatchEvent(new Event("focus")));
    expect(invalidate).toHaveBeenCalledWith({
      queryKey: queryKeys.bootstrap,
      refetchType: "active",
    });
    expect(invalidate).toHaveBeenCalledWith({
      queryKey: queryKeys.applicationUpdate,
      refetchType: "active",
    });

    result.unmount();
    expect(ipc.unlisten).toHaveBeenCalledOnce();
  });

  it("refetches the settings snapshot when the settings window regains focus", () => {
    const client = createRouterQueryClient();
    const invalidate = vi.spyOn(client, "invalidateQueries");
    render(
      <QueryClientProvider client={client}>
        <StateSyncProbe view="settings" />
      </QueryClientProvider>,
    );

    act(() => window.dispatchEvent(new Event("focus")));

    expect(invalidate).toHaveBeenCalledWith({
      queryKey: queryKeys.bootstrap,
      refetchType: "active",
    });
    expect(invalidate).toHaveBeenCalledWith({
      queryKey: queryKeys.settings,
      refetchType: "active",
    });
    expect(invalidate).toHaveBeenCalledWith({
      queryKey: queryKeys.recovery,
      refetchType: "active",
    });
    expect(invalidate).toHaveBeenCalledWith({
      queryKey: queryKeys.applicationUpdate,
      refetchType: "active",
    });
    expect(invalidate).toHaveBeenCalledWith({
      queryKey: queryKeys.usageStatistics,
      refetchType: "active",
    });
  });

  it("invalidates usage statistics from request-history summary changes", async () => {
    const client = createRouterQueryClient();
    const invalidate = vi.spyOn(client, "invalidateQueries");
    render(
      <QueryClientProvider client={client}>
        <StateSyncProbe view="settings" />
      </QueryClientProvider>,
    );
    await waitFor(() => expect(ipc.listener).toBeDefined());

    act(() =>
      ipc.listener?.({ revision: 12, areas: ["request_history_summary"] }),
    );

    expect(invalidate).toHaveBeenCalledWith({
      queryKey: queryKeys.usageStatistics,
    });
  });

  it("invalidates every recovery-owned snapshot from one recovery event", async () => {
    const client = createRouterQueryClient();
    const invalidate = vi.spyOn(client, "invalidateQueries");
    render(
      <QueryClientProvider client={client}>
        <StateSyncProbe view="settings" />
      </QueryClientProvider>,
    );
    await waitFor(() => expect(ipc.listener).toBeDefined());

    act(() => ipc.listener?.({ revision: 9, areas: ["recovery"] }));

    for (const queryKey of [
      queryKeys.recovery,
      queryKeys.bootstrap,
      queryKeys.menu,
      queryKeys.settings,
    ]) {
      expect(invalidate).toHaveBeenCalledWith({ queryKey });
    }
  });

  it("invalidates bootstrap for a shared appearance publication", async () => {
    const client = createRouterQueryClient();
    const invalidate = vi.spyOn(client, "invalidateQueries");
    render(
      <QueryClientProvider client={client}>
        <StateSyncProbe />
      </QueryClientProvider>,
    );
    await waitFor(() => expect(ipc.listener).toBeDefined());
    invalidate.mockClear();

    act(() => ipc.listener?.({ revision: 14, areas: ["appearance"] }));

    expect(invalidate).toHaveBeenCalledOnce();
    expect(invalidate).toHaveBeenCalledWith({ queryKey: queryKeys.bootstrap });
  });

  it("invalidates only the application update snapshot for update boundaries", async () => {
    const client = createRouterQueryClient();
    const invalidate = vi.spyOn(client, "invalidateQueries");
    render(
      <QueryClientProvider client={client}>
        <StateSyncProbe />
      </QueryClientProvider>,
    );
    await waitFor(() => expect(ipc.listener).toBeDefined());
    invalidate.mockClear();

    act(() => ipc.listener?.({ revision: 15, areas: ["application_update"] }));

    expect(invalidate).toHaveBeenCalledOnce();
    expect(invalidate).toHaveBeenCalledWith({
      queryKey: queryKeys.applicationUpdate,
    });
  });
});
