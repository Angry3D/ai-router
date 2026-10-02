import { QueryClientProvider, useQuery, type QueryClient } from "@tanstack/react-query";
import { act, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { StateChangedEventDto } from "../generated";
import { createRouterQueryClient, queryKeys, useRouterStateSync } from "./query";
import { listenStateChanged } from "./ipc";

const ipc = vi.hoisted(() => ({
  listener: undefined as ((event: StateChangedEventDto) => void) | undefined,
  subscriptions: new Set<(event: StateChangedEventDto) => void>(),
}));

vi.mock("./ipc", () => ({
  isTauriRuntime: () => true,
  listenStateChanged: vi.fn(),
}));

const clients: QueryClient[] = [];

beforeEach(() => {
  ipc.listener = undefined;
  ipc.subscriptions.clear();
  vi.mocked(listenStateChanged).mockReset();
  vi.mocked(listenStateChanged).mockImplementation(async (listener) => {
    ipc.listener = listener;
    ipc.subscriptions.add(listener);
    return () => { ipc.subscriptions.delete(listener); };
  });
});

afterEach(() => {
  clients.forEach((client) => client.clear());
  clients.length = 0;
  vi.restoreAllMocks();
});

function observeSnapshots(view: "menu" | "settings", keys: string[]) {
  const values = new Map(keys.map((key) => [key, "before"]));
  const client = createRouterQueryClient();
  clients.push(client);
  function Snapshot({ name }: { name: string }) {
    const { data } = useQuery({
      queryKey: [name],
      queryFn: async () => values.get(name),
      staleTime: Infinity,
    });
    return <output aria-label={name}>{data}</output>;
  }
  function Probe() {
    useRouterStateSync(view);
    return keys.map((name) => <Snapshot key={name} name={name} />);
  }
  const mount = () => render(
    <QueryClientProvider client={client}><Probe /></QueryClientProvider>,
  );
  return { values, client, mount, result: mount() };
}

async function expectValue(key: string, value: string) {
  await waitFor(() => expect(screen.getByLabelText(key)).toHaveTextContent(value));
}

async function publish(revision: number, areas: StateChangedEventDto["areas"]) {
  await act(async () => {
    for (const listener of ipc.subscriptions) listener({ revision, areas });
  });
}

describe("router state synchronization", () => {
  it("keeps displayed snapshots current while rejecting stale and duplicate events", async () => {
    const { values, client } = observeSnapshots("settings", ["settings"]);
    await expectValue("settings", "before");
    values.set("settings", "current");
    await publish(4, ["routes"]);
    await expectValue("settings", "current");
    values.set("settings", "must not refresh");
    await publish(4, ["routes"]);
    await publish(3, ["routes"]);
    expect(client.isFetching()).toBe(0);
    expect(screen.getByLabelText("settings")).toHaveTextContent("current");
    await publish(5, ["routes"]);
    await expectValue("settings", "must not refresh");
  });

  it("refreshes changed pricing without disturbing unrelated displayed snapshots", async () => {
    const { values } = observeSnapshots("settings", ["pricing-table", "settings"]);
    await expectValue("pricing-table", "before");
    await expectValue("settings", "before");
    values.set("pricing-table", "updated prices");
    values.set("settings", "unrelated change");
    await publish(1, ["pricing_table"]);
    await expectValue("pricing-table", "updated prices");
    expect(screen.getByLabelText("settings")).toHaveTextContent("before");
  });

  it("reconciles all displayed recovery snapshots after one recovery transition", async () => {
    const keys = ["bootstrap", "recovery", "settings", "menu"];
    const { values } = observeSnapshots("settings", keys);
    for (const key of keys) await expectValue(key, "before");
    for (const key of keys) values.set(key, "recovered");
    await publish(1, ["recovery"]);
    for (const key of keys) await expectValue(key, "recovered");
  });

  it("heals settings and pricing on focus when event registration fails", async () => {
    vi.mocked(listenStateChanged).mockRejectedValueOnce(new Error("synthetic registration failure"));
    const keys = [queryKeys.settings[0], queryKeys.pricingTable[0]];
    const { values } = observeSnapshots("settings", keys);
    for (const key of keys) await expectValue(key, "before");
    expect(ipc.subscriptions.size).toBe(0);
    for (const key of keys) values.set(key, "after focus");
    await act(async () => window.dispatchEvent(new Event("focus")));
    for (const key of keys) await expectValue(key, "after focus");
  });

  it("heals menu bootstrap after becoming visible despite registration failure", async () => {
    vi.mocked(listenStateChanged).mockRejectedValueOnce(new Error("synthetic registration failure"));
    const { values } = observeSnapshots("menu", ["bootstrap"]);
    await expectValue("bootstrap", "before");
    values.set("bootstrap", "after visibility");
    const visibility = vi.spyOn(document, "visibilityState", "get");
    visibility.mockReturnValue("hidden");
    await act(async () => document.dispatchEvent(new Event("visibilitychange")));
    expect(screen.getByLabelText("bootstrap")).toHaveTextContent("before");
    visibility.mockReturnValue("visible");
    await act(async () => document.dispatchEvent(new Event("visibilitychange")));
    await expectValue("bootstrap", "after visibility");
  });

  it("disposes late registration and ignores callbacks after unmount", async () => {
    let finish: (() => void) | undefined;
    vi.mocked(listenStateChanged).mockImplementationOnce((listener) => {
      ipc.listener = listener;
      return new Promise((resolve) => {
        finish = () => {
          ipc.subscriptions.add(listener);
          resolve(() => { ipc.subscriptions.delete(listener); });
        };
      });
    });
    const { client, result } = observeSnapshots("settings", ["settings"]);
    await expectValue("settings", "before");
    result.unmount();
    await act(async () => finish?.());
    expect(ipc.subscriptions.size).toBe(0);
    await act(async () => ipc.listener?.({ revision: 9, areas: ["routes"] }));
    expect(client.getQueryState(["settings"])?.isInvalidated).toBe(false);
  });

  it("registers again after a failed mount and resumes live updates", async () => {
    vi.mocked(listenStateChanged).mockRejectedValueOnce(new Error("synthetic registration failure"));
    const { values, result, mount } = observeSnapshots("settings", ["settings"]);
    await expectValue("settings", "before");
    result.unmount();
    const remounted = mount();
    await waitFor(() => expect(ipc.subscriptions.size).toBe(1));
    values.set("settings", "live again");
    await publish(1, ["routes"]);
    await expectValue("settings", "live again");
    remounted.unmount();
    expect(ipc.subscriptions.size).toBe(0);
  });
});
