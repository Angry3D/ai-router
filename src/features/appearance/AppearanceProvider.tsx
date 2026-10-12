import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { useQueryClient } from "@tanstack/react-query";
import type { Window as TauriWindow } from "@tauri-apps/api/window";

import { isTauriRuntime, normalizeIpcError, updateAppearancePreference } from "../../api/ipc";
import { queryKeys, useBootstrapSnapshot } from "../../api/query";
import type {
  AppearancePreference,
  BootstrapSnapshotDto,
  IpcErrorDto,
} from "../../generated";
import type { LanguageMessages } from "../language/languageMessages";
import { ipcErrorMessage, useLanguage } from "../language/useLanguage";
import {
  AppearanceContext,
  type ResolvedAppearance,
} from "./useAppearance";

function browserAppearance(): ResolvedAppearance {
  return window.matchMedia?.("(prefers-color-scheme: dark)")?.matches
    ? "dark"
    : "light";
}

function applyDocumentAppearance(appearance: ResolvedAppearance) {
  document.documentElement.dataset.theme = appearance;
  document.documentElement.style.colorScheme = appearance;
}

/**
 * Native window-theme mutations are frontend-synthesized failures: no Rust
 * command produces this code, so its display text is owned by the bootstrap
 * catalog section. Identity and retryability stay typed; `message` remains
 * bounded diagnostic data and is never rendered.
 */
const nativeThemeMutationError: IpcErrorDto = {
  code: "appearance_native_theme_failed",
  message: "native theme mutation failed",
  retryable: true,
  field: null,
};

function appearanceErrorMessage(
  error: IpcErrorDto,
  messages: LanguageMessages,
): string {
  return error.code === nativeThemeMutationError.code
    ? messages.bootstrap.appearanceNativeThemeFailure
    : ipcErrorMessage(error, messages);
}

interface ApplyPreferenceOptions {
  nativeFailureIsError?: boolean;
  isCurrent?: () => boolean;
}

export function AppearanceProvider({ children }: { children: React.ReactNode }) {
  const bootstrap = useBootstrapSnapshot();
  const { messages } = useLanguage();
  const queryClient = useQueryClient();
  const initialPreference = bootstrap.data?.appearancePreference ?? "system";
  const nativeRuntime = isTauriRuntime();
  const [preference, setPreferenceState] =
    useState<AppearancePreference>(initialPreference);
  const [resolvedAppearance, setResolvedAppearance] =
    useState<ResolvedAppearance>(() =>
      initialPreference === "system" ? browserAppearance() : initialPreference,
    );
  const [initialAppearanceReady, setInitialAppearanceReady] =
    useState(!nativeRuntime);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<IpcErrorDto | null>(null);
  const persistedRef = useRef<AppearancePreference>(initialPreference);
  const preferenceRef = useRef<AppearancePreference>(initialPreference);
  const mutationRef = useRef(0);

  const applyPreference = useCallback(async (
    next: AppearancePreference,
    options: ApplyPreferenceOptions = {},
  ) => {
    const isCurrent = options.isCurrent ?? (() => true);
    if (!isCurrent()) return;
    let resolved = next === "system" ? browserAppearance() : next;

    setResolvedAppearance(resolved);
    applyDocumentAppearance(resolved);
    if (nativeRuntime) {
      let currentWindow: TauriWindow;
      try {
        const windowApi = await import("@tauri-apps/api/window");
        if (!isCurrent()) return;
        currentWindow = windowApi.getCurrentWindow();
        await currentWindow.setTheme(next === "system" ? null : next);
      } catch {
        if (options.nativeFailureIsError) throw nativeThemeMutationError;
        return;
      }
      if (next === "system") {
        try {
          resolved = (await currentWindow.theme()) ?? browserAppearance();
        } catch {
          resolved = browserAppearance();
        }
        if (!isCurrent()) return;
        setResolvedAppearance(resolved);
        applyDocumentAppearance(resolved);
      }
    }
  }, [nativeRuntime]);

  useLayoutEffect(() => {
    applyDocumentAppearance(resolvedAppearance);
  }, [resolvedAppearance]);

  useEffect(() => {
    const next = bootstrap.data?.appearancePreference;
    if (!next || pending) return;
    if (
      initialAppearanceReady &&
      next === persistedRef.current &&
      next === preferenceRef.current
    ) {
      return;
    }
    let disposed = false;
    void Promise.resolve().then(async () => {
      if (disposed) return;
      persistedRef.current = next;
      preferenceRef.current = next;
      setPreferenceState(next);
      await applyPreference(next, { isCurrent: () => !disposed });
      if (!disposed) setInitialAppearanceReady(true);
    });
    return () => {
      disposed = true;
    };
  }, [
    applyPreference,
    bootstrap.data?.appearancePreference,
    initialAppearanceReady,
    pending,
  ]);

  useEffect(() => {
    if (preference !== "system") return undefined;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    const media = nativeRuntime
      ? undefined
      : window.matchMedia?.("(prefers-color-scheme: dark)");
    const handleMedia = () => {
      if (!disposed && preferenceRef.current === "system") {
        const next = browserAppearance();
        setResolvedAppearance(next);
        applyDocumentAppearance(next);
      }
    };
    media?.addEventListener("change", handleMedia);
    if (nativeRuntime) {
      void import("@tauri-apps/api/window")
        .then(({ getCurrentWindow }) => getCurrentWindow().onThemeChanged(({ payload }) => {
          if (!disposed && preferenceRef.current === "system") {
            setResolvedAppearance(payload);
            applyDocumentAppearance(payload);
          }
        }))
        .then((dispose) => {
          if (disposed) dispose();
          else unlisten = dispose;
        })
        .catch(() => {});
    }
    return () => {
      disposed = true;
      media?.removeEventListener("change", handleMedia);
      unlisten?.();
    };
  }, [nativeRuntime, preference]);

  const setPreference = useCallback(async (next: AppearancePreference) => {
    if (pending || next === preferenceRef.current) return;
    const request = ++mutationRef.current;
    const previous = persistedRef.current;
    preferenceRef.current = next;
    setPreferenceState(next);
    setPending(true);
    setError(null);
    try {
      await applyPreference(next, {
        nativeFailureIsError: true,
        isCurrent: () => mutationRef.current === request,
      });
      if (mutationRef.current !== request) return;
      await updateAppearancePreference(next);
      if (mutationRef.current !== request) return;
      persistedRef.current = next;
      queryClient.setQueryData<BootstrapSnapshotDto>(queryKeys.bootstrap, (snapshot) =>
        snapshot ? { ...snapshot, appearancePreference: next } : snapshot,
      );
    } catch (reason) {
      if (mutationRef.current !== request) return;
      preferenceRef.current = previous;
      setPreferenceState(previous);
      setError(normalizeIpcError(reason));
      await applyPreference(previous, {
        isCurrent: () => mutationRef.current === request,
      });
    } finally {
      if (mutationRef.current === request) setPending(false);
    }
  }, [applyPreference, pending, queryClient]);

  // The context keeps exposing rendered text so its consumers stay unchanged;
  // the typed error above stays the source of truth and is re-translated on
  // every render, so an alert already on screen follows a language switch.
  const errorText =
    error === null ? null : appearanceErrorMessage(error, messages);
  const value = useMemo(
    () => ({
      preference,
      resolvedAppearance,
      pending,
      error: errorText,
      setPreference,
    }),
    [errorText, pending, preference, resolvedAppearance, setPreference],
  );
  return (
    <AppearanceContext.Provider value={value}>
      {initialAppearanceReady ? children : null}
    </AppearanceContext.Provider>
  );
}
