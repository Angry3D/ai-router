import { createContext, useContext } from "react";

import type { AppearancePreference } from "../../generated";

export type ResolvedAppearance = "light" | "dark";

export interface AppearanceContextValue {
  preference: AppearancePreference;
  resolvedAppearance: ResolvedAppearance;
  pending: boolean;
  error: string | null;
  setPreference: (preference: AppearancePreference) => Promise<void>;
}

export const AppearanceContext = createContext<AppearanceContextValue>({
  preference: "system",
  resolvedAppearance: "light",
  pending: false,
  error: null,
  setPreference: async () => undefined,
});

export function useAppearance() {
  return useContext(AppearanceContext);
}
