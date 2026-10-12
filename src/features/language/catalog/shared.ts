import type { AppLocale } from "../../../generated";

/** Provider-owned strings: document/window titles applied before paint. */
export interface SharedMessages {
  menuTitle: string;
  settingsTitle: string;
}

export const sharedMessages: Record<AppLocale, SharedMessages> = {
  "zh-CN": {
    menuTitle: "AI Router",
    settingsTitle: "AI Router 设置",
  },
  en: {
    menuTitle: "AI Router",
    settingsTitle: "AI Router Settings",
  },
};
