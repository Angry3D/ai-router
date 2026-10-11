import { QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { createRouterQueryClient, queryKeys } from "../../api/query";
import type { BootstrapSnapshotDto } from "../../generated";
import { previewMenuSnapshot } from "../../previewFixtures";
import { LanguageProvider } from "./LanguageProvider";
import { useLanguage } from "./useLanguage";

const ipc = vi.hoisted(() => ({
  getBootstrapSnapshot: vi.fn(),
  tauriRuntime: false,
  updateLanguagePreference: vi.fn(),
}));

vi.mock("../../api/ipc", () => ({
  getBootstrapSnapshot: ipc.getBootstrapSnapshot,
  isTauriRuntime: () => ipc.tauriRuntime,
  normalizeIpcError: (error: unknown) => ({
    code: "language_save_failed",
    message: "failed",
    retryable: true,
    field: null,
    ...(typeof error === "object" && error !== null ? error : {}),
  }),
  updateLanguagePreference: ipc.updateLanguagePreference,
}));

function LanguageProbe() {
  const language = useLanguage();
  return (
    <>
      <output aria-label="preference">{language.preference}</output>
      <output aria-label="effective locale">{language.effectiveLocale}</output>
      <output aria-label="section title">
        {language.messages.systemRecovery.appearanceSectionTitle}
      </output>
      <output aria-label="pending">{String(language.pending)}</output>
      {language.error ? (
        <output aria-label="error-code">{language.error.code}</output>
      ) : null}
      {(["system", "zh-CN", "en"] as const).map((preference) => (
        <button
          key={preference}
          type="button"
          onClick={() => void language.setPreference(preference)}
        >
          set-{preference}
        </button>
      ))}
    </>
  );
}

function bootstrapWithLanguage(
  preference: "system" | "zh-CN" | "en",
  effectiveLocale: "zh-CN" | "en",
  revision = 0,
): BootstrapSnapshotDto {
  return {
    ...structuredClone(previewMenuSnapshot.bootstrap),
    revision,
    language: { preference, effectiveLocale },
  };
}

function renderProvider(bootstrap: BootstrapSnapshotDto) {
  const client = createRouterQueryClient();
  client.setQueryData(queryKeys.bootstrap, bootstrap);
  const rendered = render(
    <QueryClientProvider client={client}>
      <LanguageProvider view="settings">
        <LanguageProbe />
      </LanguageProvider>
    </QueryClientProvider>,
  );
  return { client, ...rendered };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

beforeEach(() => {
  ipc.tauriRuntime = false;
  ipc.getBootstrapSnapshot.mockReset();
  ipc.updateLanguagePreference.mockReset();
  ipc.updateLanguagePreference.mockResolvedValue({ revision: 2 });
  ipc.getBootstrapSnapshot.mockResolvedValue(
    structuredClone(previewMenuSnapshot.bootstrap),
  );
  document.documentElement.lang = "";
  document.title = "";
});

describe("LanguageProvider", () => {
  it("applies the committed bootstrap pair before exposing consumers", () => {
    renderProvider(bootstrapWithLanguage("zh-CN", "zh-CN"));

    expect(screen.getByLabelText("preference")).toHaveTextContent("zh-CN");
    expect(screen.getByLabelText("effective locale")).toHaveTextContent("zh-CN");
    expect(screen.getByLabelText("section title")).toHaveTextContent(
      "外观与语言",
    );
    expect(document.documentElement.lang).toBe("zh-CN");
    expect(document.title).toBe("AI Router 设置");
  });

  it("keeps the committed pair and document language while a save is pending", async () => {
    const save = deferred<{ revision: number }>();
    ipc.updateLanguagePreference.mockReturnValue(save.promise);
    const { client } = renderProvider(bootstrapWithLanguage("system", "en", 1));

    fireEvent.click(screen.getByRole("button", { name: "set-zh-CN" }));

    expect(await screen.findByLabelText("pending")).toHaveTextContent("true");
    // No optimistic flip: presentation stays on the committed pair.
    expect(screen.getByLabelText("preference")).toHaveTextContent("system");
    expect(screen.getByLabelText("effective locale")).toHaveTextContent("en");
    expect(document.documentElement.lang).toBe("en");
    expect(ipc.updateLanguagePreference).toHaveBeenCalledWith("zh-CN");

    client.setQueryData(
      queryKeys.bootstrap,
      bootstrapWithLanguage("zh-CN", "zh-CN", 3),
    );
    save.resolve({ revision: 3 });
    await waitFor(() => {
      expect(screen.getByLabelText("pending")).toHaveTextContent("false");
      expect(screen.getByLabelText("preference")).toHaveTextContent("zh-CN");
      expect(screen.getByLabelText("effective locale")).toHaveTextContent(
        "zh-CN",
      );
    });
    expect(document.documentElement.lang).toBe("zh-CN");
  });

  it("retains the committed pair and a typed error identity when the save fails", async () => {
    const authority = bootstrapWithLanguage("system", "en", 1);
    ipc.getBootstrapSnapshot.mockResolvedValue(structuredClone(authority));
    ipc.updateLanguagePreference.mockRejectedValueOnce(
      new Error("storage failed"),
    );
    renderProvider(authority);
    fireEvent.click(screen.getByRole("button", { name: "set-en" }));

    await waitFor(() => {
      expect(screen.getByLabelText("pending")).toHaveTextContent("false");
      expect(screen.getByLabelText("error-code")).toHaveTextContent(
        "language_save_failed",
      );
    });
    expect(screen.getByLabelText("preference")).toHaveTextContent("system");
    expect(screen.getByLabelText("effective locale")).toHaveTextContent("en");
    expect(document.documentElement.lang).toBe("en");
  });

  it("ignores a duplicate selection without invoking IPC", () => {
    renderProvider(bootstrapWithLanguage("en", "en"));

    fireEvent.click(screen.getByRole("button", { name: "set-en" }));

    expect(ipc.updateLanguagePreference).not.toHaveBeenCalled();
  });

  it("heals from a later authoritative bootstrap update when idle", async () => {
    const { client } = renderProvider(bootstrapWithLanguage("system", "en", 1));
    expect(document.documentElement.lang).toBe("en");

    client.setQueryData(
      queryKeys.bootstrap,
      bootstrapWithLanguage("system", "zh-CN", 2),
    );
    await waitFor(() => {
      expect(screen.getByLabelText("effective locale")).toHaveTextContent(
        "zh-CN",
      );
    });
    expect(document.documentElement.lang).toBe("zh-CN");
    expect(screen.getByLabelText("section title")).toHaveTextContent(
      "外观与语言",
    );
  });

  it("settles on the authoritative pair when a newer snapshot arrives before a failed acknowledgement", async () => {
    ipc.tauriRuntime = true;
    const save = deferred<{ revision: number }>();
    ipc.updateLanguagePreference.mockReturnValue(save.promise);
    let authority = bootstrapWithLanguage("system", "en", 1);
    ipc.getBootstrapSnapshot.mockImplementation(() =>
      Promise.resolve(structuredClone(authority)),
    );
    const { client } = renderProvider(authority);
    await screen.findByLabelText("effective locale");

    fireEvent.click(screen.getByRole("button", { name: "set-zh-CN" }));
    expect(await screen.findByLabelText("pending")).toHaveTextContent("true");

    // The authority moves while the request is in flight and the client
    // observes the newer snapshot before the acknowledgement fails.
    authority = bootstrapWithLanguage("zh-CN", "zh-CN", 2);
    client.setQueryData(queryKeys.bootstrap, structuredClone(authority));
    expect(screen.getByLabelText("effective locale")).toHaveTextContent("en");

    save.reject(new Error("lost acknowledgement"));
    await waitFor(() => {
      expect(screen.getByLabelText("pending")).toHaveTextContent("false");
      expect(screen.getByLabelText("effective locale")).toHaveTextContent(
        "zh-CN",
      );
    });
    expect(screen.getByLabelText("preference")).toHaveTextContent("zh-CN");
    expect(document.documentElement.lang).toBe("zh-CN");
    expect(screen.getByLabelText("error-code")).toHaveTextContent(
      "language_save_failed",
    );

    // Reading the same authority again keeps the presentation consistent.
    await client.refetchQueries({ queryKey: queryKeys.bootstrap });
    expect(screen.getByLabelText("effective locale")).toHaveTextContent(
      "zh-CN",
    );
    expect(document.documentElement.lang).toBe("zh-CN");
  });

  it("reports an incomplete sync instead of presenting a stale snapshot as the save result", async () => {
    ipc.tauriRuntime = true;
    const stale = bootstrapWithLanguage("system", "en", 1);
    ipc.updateLanguagePreference.mockResolvedValue({ revision: 2 });
    ipc.getBootstrapSnapshot.mockRejectedValue(new Error("read failed"));
    const { client } = renderProvider(stale);
    await screen.findByLabelText("effective locale");

    fireEvent.click(screen.getByRole("button", { name: "set-zh-CN" }));

    await waitFor(() => {
      expect(screen.getByLabelText("pending")).toHaveTextContent("false");
      expect(screen.getByLabelText("error-code")).toHaveTextContent(
        "language_sync_incomplete",
      );
    });
    // The stale committed pair is not presented as the result of the write.
    expect(screen.getByLabelText("effective locale")).toHaveTextContent("en");
    expect(document.documentElement.lang).toBe("en");

    // A readable snapshot at or after the acknowledged revision settles both
    // the language and the pending-sync notice.
    ipc.getBootstrapSnapshot.mockResolvedValue(
      bootstrapWithLanguage("zh-CN", "zh-CN", 3),
    );
    await client.refetchQueries({ queryKey: queryKeys.bootstrap });
    await waitFor(() => {
      expect(screen.getByLabelText("effective locale")).toHaveTextContent(
        "zh-CN",
      );
    });
    expect(document.documentElement.lang).toBe("zh-CN");
    expect(screen.queryByLabelText("error-code")).toBeNull();
  });

  it("waits for a snapshot at or after the acknowledged revision", async () => {
    ipc.tauriRuntime = true;
    const stale = bootstrapWithLanguage("system", "en", 1);
    ipc.updateLanguagePreference.mockResolvedValue({ revision: 5 });
    ipc.getBootstrapSnapshot.mockResolvedValue(
      bootstrapWithLanguage("zh-CN", "zh-CN", 2),
    );
    const { client } = renderProvider(stale);
    await screen.findByLabelText("effective locale");

    fireEvent.click(screen.getByRole("button", { name: "set-zh-CN" }));

    await waitFor(() => {
      expect(screen.getByLabelText("pending")).toHaveTextContent("false");
      expect(screen.getByLabelText("error-code")).toHaveTextContent(
        "language_sync_incomplete",
      );
    });
    expect(screen.getByLabelText("effective locale")).toHaveTextContent("en");

    client.setQueryData(
      queryKeys.bootstrap,
      bootstrapWithLanguage("zh-CN", "zh-CN", 5),
    );
    await waitFor(() => {
      expect(screen.getByLabelText("effective locale")).toHaveTextContent(
        "zh-CN",
      );
    });
    expect(screen.queryByLabelText("error-code")).toBeNull();
  });
});
