import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { useQueryClient } from "@tanstack/react-query";

import { isTauriRuntime, normalizeIpcError, updateLanguagePreference } from "../../api/ipc";
import { queryKeys, useBootstrapSnapshot } from "../../api/query";
import type {
  BootstrapSnapshotDto,
  IpcErrorDto,
  LanguageSnapshotDto,
} from "../../generated";
import type { AppView } from "../../App";
import { languageMessages } from "./languageMessages";
import { LanguageContext, languageSyncIncompleteCode } from "./useLanguage";

const fallbackPair: LanguageSnapshotDto = {
  preference: "system",
  effectiveLocale: "en",
};

/**
 * The preference was committed but its authoritative snapshot could not be
 * read back yet. This is deliberately not a persistence failure: storage may
 * already hold the new preference, so the write is never repeated or undone
 * on the strength of this identity.
 */
function languageSyncIncompleteError(): IpcErrorDto {
  return {
    code: languageSyncIncompleteCode,
    message: "language preference committed; authoritative snapshot not read back yet",
    retryable: true,
    field: null,
  };
}

/**
 * Owns the effective locale for app-owned presentation text. Unlike the
 * appearance provider there is no optimistic switch: the committed bootstrap
 * pair drives `document.lang`, the document title, and every consumer until
 * a durable save is reconciled from the authoritative snapshot.
 */
export function LanguageProvider({
  view,
  children,
}: {
  view: AppView;
  children: ReactNode;
}) {
  const bootstrap = useBootstrapSnapshot();
  const queryClient = useQueryClient();
  const [pair, setPair] = useState<LanguageSnapshotDto>(fallbackPair);
  // Browser preview renders immediately; Tauri waits for the first committed
  // pair so the document language and title are applied before any paint.
  const [initialLanguageReady, setInitialLanguageReady] = useState(
    !isTauriRuntime(),
  );
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<IpcErrorDto | null>(null);
  const pendingRef = useRef(false);
  const mutationRef = useRef(0);
  // Lowest bootstrap revision that may be presented as the result of an
  // acknowledged write; null while no commit is waiting for its snapshot.
  const expectedRevisionRef = useRef<number | null>(null);

  const applyPair = useCallback(
    (next: LanguageSnapshotDto) => {
      setPair(next);
      document.documentElement.lang = next.effectiveLocale;
      document.title =
        languageMessages[next.effectiveLocale].shared[
          view === "menu" ? "menuTitle" : "settingsTitle"
        ];
    },
    [view],
  );

  /**
   * Presents the latest authoritative snapshot when it can stand for the
   * acknowledged write (at or after its revision). Returns whether it applied;
   * a snapshot older than the acknowledged revision is never presented as the
   * result of that write.
   */
  const applyAuthoritativeSnapshot = useCallback(() => {
    const snapshot = queryClient.getQueryData<BootstrapSnapshotDto>(
      queryKeys.bootstrap,
    );
    if (!snapshot) return false;
    const expected = expectedRevisionRef.current;
    if (expected !== null && snapshot.revision < expected) return false;
    expectedRevisionRef.current = null;
    applyPair(snapshot.language);
    setInitialLanguageReady(true);
    setError((current) =>
      current?.code === languageSyncIncompleteCode ? null : current,
    );
    return true;
  }, [applyPair, queryClient]);

  useEffect(() => {
    const snapshot = bootstrap.data;
    const next = snapshot?.language;
    if (!next || pendingRef.current) return;
    const expected = expectedRevisionRef.current;
    if (expected !== null && snapshot.revision < expected) return;
    expectedRevisionRef.current = null;
    applyPair(next);
    setInitialLanguageReady(true);
    if (expected !== null) {
      setError((current) =>
        current?.code === languageSyncIncompleteCode ? null : current,
      );
    }
  }, [applyPair, bootstrap.data]);

  const setPreference = useCallback(
    async (next: LanguageSnapshotDto["preference"]) => {
      if (pendingRef.current || next === pair.preference) return;
      const request = ++mutationRef.current;
      pendingRef.current = true;
      setPending(true);
      setError(null);
      try {
        const mutation = await updateLanguagePreference(next);
        if (mutationRef.current !== request) return;
        // The IPC acknowledgement alone is not permission to switch: read the
        // authoritative bootstrap at/after the returned revision, then apply
        // the committed pair before clearing the pending state.
        expectedRevisionRef.current = mutation.revision;
        if (!applyAuthoritativeSnapshot()) {
          await queryClient.refetchQueries({ queryKey: queryKeys.bootstrap });
          if (mutationRef.current !== request) return;
          if (!applyAuthoritativeSnapshot()) {
            // Commit acknowledged, read-back still missing: keep the pending
            // revision armed so a later snapshot settles it, and say so
            // instead of presenting the stale pair as the save result.
            setError(languageSyncIncompleteError());
          }
        }
      } catch (reason) {
        if (mutationRef.current !== request) return;
        const failure = normalizeIpcError(reason);
        // The request failed, but the authority may have moved anyway (lost
        // acknowledgement or a competing writer). Read it back and let it win:
        // nothing is written here, and a genuine failure leaves the
        // authoritative pair unchanged, so the committed pair is retained.
        expectedRevisionRef.current = null;
        await queryClient.refetchQueries({ queryKey: queryKeys.bootstrap });
        if (mutationRef.current !== request) return;
        setError(failure);
        applyAuthoritativeSnapshot();
      } finally {
        if (mutationRef.current === request) {
          pendingRef.current = false;
          setPending(false);
        }
      }
    },
    [applyAuthoritativeSnapshot, pair.preference, queryClient],
  );

  const value = useMemo(
    () => ({
      preference: pair.preference,
      effectiveLocale: pair.effectiveLocale,
      messages: languageMessages[pair.effectiveLocale],
      pending,
      error,
      setPreference,
    }),
    [error, pair, pending, setPreference],
  );
  return (
    <LanguageContext.Provider value={value}>
      {initialLanguageReady ? children : null}
    </LanguageContext.Provider>
  );
}
