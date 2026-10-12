import { createContext, useContext } from "react";

import type { AppLocale, IpcErrorDto, LanguagePreference } from "../../generated";
import { languageMessages, type LanguageMessages } from "./languageMessages";

export interface LanguageContextValue {
  preference: LanguagePreference;
  effectiveLocale: AppLocale;
  messages: LanguageMessages;
  pending: boolean;
  error: IpcErrorDto | null;
  setPreference: (preference: LanguagePreference) => Promise<void>;
}

// The product always mounts LanguageProvider; this default keeps the
// provider-less rendering of existing surfaces on the current product locale.
export const LanguageContext = createContext<LanguageContextValue>({
  preference: "system",
  effectiveLocale: "zh-CN",
  messages: languageMessages["zh-CN"],
  pending: false,
  error: null,
  setPreference: async () => undefined,
});

export function useLanguage() {
  return useContext(LanguageContext);
}

/**
 * Frontend-synthesized error identity for "the preference was committed but
 * the authoritative snapshot has not been read back yet". No Rust command
 * returns this code; the settings control renders a dedicated bootstrap
 * message for it so a committed change is never silently absent.
 */
export const languageSyncIncompleteCode = "language_sync_incomplete";

/**
 * Renders one typed IPC error from the catalog by its stable code. Unknown
 * codes fall back to the localized generic failure — never the raw diagnostic
 * `message`, which stays compatibility data only.
 */
export function ipcErrorMessage(
  error: IpcErrorDto,
  messages: LanguageMessages,
): string {
  const known = (messages.errors.codes as Partial<Record<string, string>>)[
    error.code
  ];
  return known ?? messages.errors.genericFailure;
}
