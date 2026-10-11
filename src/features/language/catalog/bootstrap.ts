import type { AppLocale } from "../../../generated";

/**
 * App-root surfaces that no feature section owns: failure text for the
 * `AppRoot`/`App` bootstrap wrappers.
 *
 * `appearanceNativeThemeFailure` is frontend-synthesized — applying the Tauri
 * window theme failed, so no Rust command produces this code and it has no
 * `catalog/errors` entry. The error keeps its identity
 * (`appearance_native_theme_failed`) and retryability; only its display text
 * lives here.
 */
export interface BootstrapMessages {
  appearanceNativeThemeFailure: string;
}

export const bootstrapMessages: Record<AppLocale, BootstrapMessages> = {
  "zh-CN": {
    appearanceNativeThemeFailure: "无法应用外观设置，请重试。",
  },
  en: {
    appearanceNativeThemeFailure:
      "The appearance setting could not be applied. Try again.",
  },
};
