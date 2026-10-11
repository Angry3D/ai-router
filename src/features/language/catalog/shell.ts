import type { AppLocale } from "../../../generated";

/**
 * Settings shell and shared primitives: sidebar, loading/fatal path,
 * unsaved-changes confirmation, default dialog buttons, combobox hints,
 * version labels. Common UI strings used across groups live here.
 */
export interface ShellMessages {
  /** Accessible name of the whole settings window shell. */
  shellLabel: (displayName: string) => string;
  /** Settings sidebar section labels. */
  sectionRoutes: string;
  sectionUsage: string;
  sectionCodex: string;
  sectionSystem: string;
  /** Sidebar indicator announcements attached to a section label. */
  indicatorImageCapacity: string;
  indicatorUpdateAvailable: string;
  /** Accessible name of a sidebar item that carries an indicator. */
  navItemLabel: (label: string, indicator: string) => string;
  /** Accessible name of the sidebar navigation landmark. */
  navigationLabel: string;
  /** Sidebar repository opener, used for both aria-label and title. */
  openRepositoryLabel: string;
  /** Sidebar version footer; `badge` is the QA designation when present. */
  versionLabel: (version: string | null, badge: string | null) => string;
  /** Settings snapshot loading and failure states. */
  loadingSettings: string;
  loadingRecoveryPoint: string;
  /** Unsaved-changes confirmation. */
  unsavedConfirmTitle: string;
  unsavedConfirmBody: string;
  unsavedConfirmDiscard: string;
  unsavedConfirmContinue: string;
  /** Default cancel button for confirmations that omit their cancel label. */
  cancel: string;
  /** Shared primary action label for settings forms. */
  save: string;
  /** Model combobox hints. */
  comboboxNoMatches: string;
  comboboxAdded: string;
}

export const shellMessages: Record<AppLocale, ShellMessages> = {
  "zh-CN": {
    shellLabel: (displayName) => `${displayName} 设置`,
    sectionRoutes: "路由",
    sectionUsage: "用量",
    sectionCodex: "Codex",
    sectionSystem: "系统",
    indicatorImageCapacity: "生成图片已达到容量提醒阈值",
    indicatorUpdateAvailable: "有可用更新",
    navItemLabel: (label, indicator) => `${label}，${indicator}`,
    navigationLabel: "设置分区",
    openRepositoryLabel: "打开 GitHub 项目",
    versionLabel: (version, badge) =>
      `版本 ${version ?? "—"}${badge ? ` · ${badge}` : ""}`,
    loadingSettings: "正在读取设置...",
    loadingRecoveryPoint: "正在读取恢复点...",
    unsavedConfirmTitle: "放弃未保存的修改？",
    unsavedConfirmBody: "当前设置的修改尚未保存。",
    unsavedConfirmDiscard: "放弃修改",
    unsavedConfirmContinue: "继续编辑",
    cancel: "取消",
    save: "保存",
    comboboxNoMatches: "无匹配模型",
    comboboxAdded: "已添加",
  },
  en: {
    shellLabel: (displayName) => `${displayName} Settings`,
    sectionRoutes: "Routes",
    sectionUsage: "Usage",
    sectionCodex: "Codex",
    sectionSystem: "System",
    indicatorImageCapacity:
      "Image generation reached the capacity warning threshold",
    indicatorUpdateAvailable: "An update is available",
    navItemLabel: (label, indicator) => `${label}, ${indicator}`,
    navigationLabel: "Settings sections",
    openRepositoryLabel: "Open GitHub project",
    versionLabel: (version, badge) =>
      `Version ${version ?? "—"}${badge ? ` · ${badge}` : ""}`,
    loadingSettings: "Loading settings...",
    loadingRecoveryPoint: "Loading recovery points...",
    unsavedConfirmTitle: "Discard unsaved changes?",
    unsavedConfirmBody: "Your settings changes have not been saved.",
    unsavedConfirmDiscard: "Discard changes",
    unsavedConfirmContinue: "Keep editing",
    cancel: "Cancel",
    save: "Save",
    comboboxNoMatches: "No matching models",
    comboboxAdded: "Added",
  },
};
