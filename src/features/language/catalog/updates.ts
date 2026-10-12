import type { AppLocale } from "../../../generated";

/**
 * Application-update settings surface. Release-note bodies come from the
 * updater feed and stay English-authored; only the surrounding chrome is
 * localized here.
 */
export interface UpdatesMessages {
  sectionTitle: string;
  statusChecking: string;
  statusDownloading: string;
  statusInstalling: string;
  statusRestartReady: string;
  statusAvailable: string;
  statusCheckFailed: string;
  statusUpToDate: string;
  statusNotChecked: string;
  currentVersionLabel: string;
  lastCheckedLabel: string;
  availableVersionLabel: string;
  notesHeading: string;
  notesCollapse: string;
  notesViewAll: (count: number) => string;
  notesHighlightsHeading: string;
  notesFixesHeading: string;
  notesNoticesHeading: string;
  releaseNotesHeading: string;
  releaseNotesCollapse: string;
  releaseNotesExpand: string;
  releaseNotesLegacyHint: string;
  releaseNotesFallback: string;
  progressDownloadLabel: string;
  progressDownloading: string;
  progressInstalling: string;
  progressDetail: (
    percentage: number,
    downloaded: string,
    total: string,
  ) => string;
  checkButton: string;
  checkingButton: string;
  recheckButton: string;
  downloadButton: string;
  retryDownloadButton: string;
  restartButton: string;
  viewReleaseButton: string;
  confirmDownloadTitle: string;
  confirmDownloadBody: string;
  confirmDownloadConfirm: string;
  confirmRestartTitle: string;
  confirmRestartBody: string;
  confirmRestartConfirm: string;
  confirmRestartCancel: string;
}

export const updatesMessages: Record<AppLocale, UpdatesMessages> = {
  "zh-CN": {
    sectionTitle: "应用更新",
    statusChecking: "正在检查",
    statusDownloading: "正在下载",
    statusInstalling: "正在安装",
    statusRestartReady: "更新已安装，等待重启",
    statusAvailable: "发现新版本",
    statusCheckFailed: "检查失败",
    statusUpToDate: "已是最新版本",
    statusNotChecked: "尚未检查",
    currentVersionLabel: "当前版本",
    lastCheckedLabel: "最近检查",
    availableVersionLabel: "可用版本",
    notesHeading: "本次更新",
    notesCollapse: "收起更新内容",
    notesViewAll: (count) => `查看全部 ${count} 项`,
    notesHighlightsHeading: "重点更新",
    notesFixesHeading: "问题修复",
    notesNoticesHeading: "注意事项",
    releaseNotesHeading: "发行说明",
    releaseNotesCollapse: "收起发行说明",
    releaseNotesExpand: "展开发行说明",
    releaseNotesLegacyHint:
      "此说明来自旧版格式，完整内容请查看 GitHub Release。",
    releaseNotesFallback:
      "此版本未提供可验证的更新摘要，请查看 GitHub Release。",
    progressDownloadLabel: "更新下载进度",
    progressDownloading: "正在下载",
    progressInstalling: "正在验证并安装",
    progressDetail: (percentage, downloaded, total) =>
      `${percentage}% · ${downloaded} / ${total}`,
    checkButton: "检查更新",
    checkingButton: "正在检查",
    recheckButton: "再次检查",
    downloadButton: "下载并安装",
    retryDownloadButton: "重试下载",
    restartButton: "重启并完成更新",
    viewReleaseButton: "查看 GitHub Release",
    confirmDownloadTitle: "下载并安装应用更新？",
    confirmDownloadBody:
      "应用会先校验项目签名，再替换应用文件；安装完成后不会自动重启。",
    confirmDownloadConfirm: "下载并安装",
    confirmRestartTitle: "重启并完成更新？",
    confirmRestartBody:
      "应用会先正常关闭代理与后台服务，然后重新打开更新后的版本。",
    confirmRestartConfirm: "重启应用",
    confirmRestartCancel: "稍后",
  },
  en: {
    sectionTitle: "Application update",
    statusChecking: "Checking",
    statusDownloading: "Downloading",
    statusInstalling: "Installing",
    statusRestartReady: "Update installed, restart to finish",
    statusAvailable: "New version available",
    statusCheckFailed: "Check failed",
    statusUpToDate: "Up to date",
    statusNotChecked: "Not checked yet",
    currentVersionLabel: "Current version",
    lastCheckedLabel: "Last checked",
    availableVersionLabel: "Available version",
    notesHeading: "What's new",
    notesCollapse: "Collapse update details",
    notesViewAll: (count) =>
      count === 1 ? `View all ${count} item` : `View all ${count} items`,
    notesHighlightsHeading: "Highlights",
    notesFixesHeading: "Fixes",
    notesNoticesHeading: "Notices",
    releaseNotesHeading: "Release notes",
    releaseNotesCollapse: "Collapse release notes",
    releaseNotesExpand: "Expand release notes",
    releaseNotesLegacyHint:
      "These notes use the legacy format; see the GitHub Release for the full content.",
    releaseNotesFallback:
      "This version has no verifiable update summary; see the GitHub Release.",
    progressDownloadLabel: "Update download progress",
    progressDownloading: "Downloading",
    progressInstalling: "Verifying and installing",
    progressDetail: (percentage, downloaded, total) =>
      `${percentage}% · ${downloaded} / ${total}`,
    checkButton: "Check for updates",
    checkingButton: "Checking",
    recheckButton: "Check again",
    downloadButton: "Download and install",
    retryDownloadButton: "Retry download",
    restartButton: "Restart and finish update",
    viewReleaseButton: "View GitHub Release",
    confirmDownloadTitle: "Download and install the application update?",
    confirmDownloadBody:
      "The app verifies the project signature first, then replaces the application files; it will not restart automatically once the install finishes.",
    confirmDownloadConfirm: "Download and install",
    confirmRestartTitle: "Restart and finish the update?",
    confirmRestartBody:
      "The app shuts down the proxy and background services cleanly, then reopens the updated version.",
    confirmRestartConfirm: "Restart app",
    confirmRestartCancel: "Later",
  },
};
