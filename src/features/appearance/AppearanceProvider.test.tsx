import { QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { createRouterQueryClient, queryKeys } from "../../api/query";
import type { AppearancePreference } from "../../generated";
import { previewMenuSnapshot } from "../../previewFixtures";
import { AppearanceProvider } from "./AppearanceProvider";
import { useAppearance } from "./useAppearance";

const ipc = vi.hoisted(() => ({
  getBootstrapSnapshot: vi.fn(),
  tauriRuntime: false,
  updateAppearancePreference: vi.fn(),
}));

const native = vi.hoisted(() => ({
  listener: undefined as
    | ((event: { payload: "light" | "dark" }) => void)
    | undefined,
  onThemeChanged: vi.fn(),
  setTheme: vi.fn(),
  theme: vi.fn(),
  unlisten: vi.fn(),
}));

vi.mock("../../api/ipc", () => ({
  getBootstrapSnapshot: ipc.getBootstrapSnapshot,
  isTauriRuntime: () => ipc.tauriRuntime,
  normalizeIpcError: (error: unknown) => {
    if (typeof error === "object" && error !== null && "message" in error) {
      return {
        code: "test",
        message: String(error.message),
        retryable: true,
        field: null,
      };
    }
    return {
      code: "test",
      message: "保存失败",
      retryable: true,
      field: null,
    };
  },
  updateAppearancePreference: ipc.updateAppearancePreference,
}));

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    onThemeChanged: native.onThemeChanged,
    setTheme: native.setTheme,
    theme: native.theme,
  }),
}));

const media = {
  listeners: new Set<() => void>(),
  matches: false,
  addEventListener: vi.fn((_type: string, listener: () => void) => {
    media.listeners.add(listener);
  }),
  removeEventListener: vi.fn((_type: string, listener: () => void) => {
    media.listeners.delete(listener);
  }),
};

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, reject, resolve };
}

function AppearanceProbe() {
  const appearance = useAppearance();
  return (
    <>
      <output aria-label="preference">{appearance.preference}</output>
      <output aria-label="resolved appearance">
        {appearance.resolvedAppearance}
      </output>
      <output aria-label="pending">{String(appearance.pending)}</output>
      {appearance.error ? <p role="alert">{appearance.error}</p> : null}
      {(["system", "light", "dark"] as const).map((preference) => (
        <button
          key={preference}
          type="button"
          onClick={() => void appearance.setPreference(preference)}
        >
          {preference}
        </button>
      ))}
    </>
  );
}

function renderProvider(preference: AppearancePreference) {
  const client = createRouterQueryClient();
  client.setQueryData(queryKeys.bootstrap, {
    ...structuredClone(previewMenuSnapshot.bootstrap),
    appearancePreference: preference,
  });
  return {
    client,
    ...render(
      <QueryClientProvider client={client}>
        <AppearanceProvider>
          <AppearanceProbe />
        </AppearanceProvider>
      </QueryClientProvider>,
    ),
  };
}

async function expectPreference(
  preference: AppearancePreference,
  resolved: "light" | "dark",
) {
  await waitFor(() => {
    expect(screen.getByLabelText("preference")).toHaveTextContent(preference);
    expect(screen.getByLabelText("resolved appearance")).toHaveTextContent(
      resolved,
    );
  });
}

beforeEach(() => {
  ipc.tauriRuntime = false;
  ipc.getBootstrapSnapshot.mockReset();
  ipc.updateAppearancePreference.mockReset();
  ipc.updateAppearancePreference.mockResolvedValue({ revision: 1 });

  native.listener = undefined;
  native.onThemeChanged.mockReset();
  native.onThemeChanged.mockImplementation(async (listener) => {
    native.listener = listener;
    return native.unlisten;
  });
  native.setTheme.mockReset();
  native.setTheme.mockResolvedValue(undefined);
  native.theme.mockReset();
  native.theme.mockResolvedValue("light");
  native.unlisten.mockReset();

  media.matches = false;
  media.listeners.clear();
  media.addEventListener.mockClear();
  media.removeEventListener.mockClear();
  Object.defineProperty(window, "matchMedia", {
    configurable: true,
    value: vi.fn(() => ({
      matches: media.matches,
      media: "(prefers-color-scheme: dark)",
      onchange: null,
      addEventListener: media.addEventListener,
      removeEventListener: media.removeEventListener,
    })),
  });
  delete document.documentElement.dataset.theme;
  document.documentElement.style.removeProperty("color-scheme");
});

describe("AppearanceProvider", () => {
  it("applies a cached persisted preference before children paint", () => {
    renderProvider("dark");

    expect(screen.getByLabelText("preference")).toHaveTextContent("dark");
    expect(document.documentElement.dataset.theme).toBe("dark");
    expect(document.documentElement.style.colorScheme).toBe("dark");
  });

  it("uses a deterministic light fallback when matchMedia is unavailable", async () => {
    Object.defineProperty(window, "matchMedia", {
      configurable: true,
      value: undefined,
    });

    renderProvider("system");

    await expectPreference("system", "light");
    expect(document.documentElement.dataset.theme).toBe("light");
  });

  it("follows browser system media changes while System is selected", async () => {
    const rendered = renderProvider("system");
    await expectPreference("system", "light");

    media.matches = true;
    act(() => media.listeners.forEach((listener) => listener()));

    await expectPreference("system", "dark");
    expect(document.documentElement.dataset.theme).toBe("dark");
    rendered.unmount();
    expect(media.removeEventListener).toHaveBeenCalledWith(
      "change",
      expect.any(Function),
    );
  });

  it("follows native system theme events", async () => {
    ipc.tauriRuntime = true;
    renderProvider("system");
    await screen.findByLabelText("preference");
    await waitFor(() => expect(native.listener).toBeDefined());

    act(() => native.listener?.({ payload: "dark" }));

    await expectPreference("system", "dark");
  });

  it("ignores later system events after an explicit override", async () => {
    ipc.tauriRuntime = true;
    renderProvider("system");
    await screen.findByLabelText("preference");
    await waitFor(() => expect(native.listener).toBeDefined());
    const systemListener = native.listener;

    fireEvent.click(screen.getByRole("button", { name: "dark" }));
    await expectPreference("dark", "dark");
    await waitFor(() => expect(native.unlisten).toHaveBeenCalledOnce());

    act(() => systemListener?.({ payload: "light" }));
    await expectPreference("dark", "dark");
  });

  it("keeps the optimistic selection pending and commits it on success", async () => {
    const mutation = deferred<{ revision: number }>();
    ipc.updateAppearancePreference.mockReturnValueOnce(mutation.promise);
    const { client } = renderProvider("light");

    fireEvent.click(screen.getByRole("button", { name: "dark" }));
    await expectPreference("dark", "dark");
    expect(screen.getByLabelText("pending")).toHaveTextContent("true");
    expect(
      client.getQueryData<{ appearancePreference: AppearancePreference }>(
        queryKeys.bootstrap,
      )?.appearancePreference,
    ).toBe("light");

    mutation.resolve({ revision: 2 });
    await waitFor(() =>
      expect(screen.getByLabelText("pending")).toHaveTextContent("false"),
    );
    expect(
      client.getQueryData<{ appearancePreference: AppearancePreference }>(
        queryKeys.bootstrap,
      )?.appearancePreference,
    ).toBe("dark");
  });

  it("rolls native and document appearance back when persistence fails", async () => {
    ipc.tauriRuntime = true;
    ipc.updateAppearancePreference.mockRejectedValueOnce({
      code: "save_failed",
      message: "保存失败",
    });
    renderProvider("light");
    await screen.findByLabelText("preference");
    native.setTheme.mockClear();

    fireEvent.click(screen.getByRole("button", { name: "dark" }));

    await expectPreference("light", "light");
    expect(await screen.findByRole("alert")).toHaveTextContent("保存失败");
    expect(native.setTheme.mock.calls.map(([theme]) => theme)).toEqual([
      "dark",
      "light",
    ]);
    expect(document.documentElement.dataset.theme).toBe("light");
  });

  it("treats a native user-mutation failure as a rollback error", async () => {
    ipc.tauriRuntime = true;
    renderProvider("light");
    await screen.findByLabelText("preference");
    native.setTheme.mockClear();
    native.setTheme.mockRejectedValueOnce(new Error("native failure"));

    fireEvent.click(screen.getByRole("button", { name: "dark" }));

    await expectPreference("light", "light");
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "无法应用外观设置，请重试。",
    );
    expect(ipc.updateAppearancePreference).not.toHaveBeenCalled();
    expect(native.setTheme.mock.calls.map(([theme]) => theme)).toEqual([
      "dark",
      "light",
    ]);
  });

  it("reconciles a later authoritative bootstrap preference", async () => {
    const { client } = renderProvider("light");
    await expectPreference("light", "light");

    act(() => {
      client.setQueryData(queryKeys.bootstrap, {
        ...structuredClone(previewMenuSnapshot.bootstrap),
        appearancePreference: "dark",
      });
    });

    await expectPreference("dark", "dark");
  });

  it("cleans up a late native listener registration", async () => {
    ipc.tauriRuntime = true;
    const registration = deferred<() => void>();
    const lateUnlisten = vi.fn();
    native.onThemeChanged.mockReturnValueOnce(registration.promise);
    const rendered = renderProvider("system");
    await screen.findByLabelText("preference");
    await waitFor(() => expect(native.onThemeChanged).toHaveBeenCalledOnce());

    rendered.unmount();
    expect(media.addEventListener).not.toHaveBeenCalled();

    registration.resolve(lateUnlisten);
    await waitFor(() => expect(lateUnlisten).toHaveBeenCalledOnce());
  });
});
