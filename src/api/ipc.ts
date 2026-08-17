import { Channel, invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import type {
  AppLifecycleSnapshot,
  AppearancePreference,
  ApplicationUpdateProgressDto,
  ApplicationUpdateSnapshotDto,
  BalanceDisplaySnapshot,
  BalanceQuerySettingsDto,
  BalanceRefreshBatchState,
  BalanceResult,
  BalanceTestInputDto,
  BootstrapSnapshotDto,
  CodexImagesMcpRepairPreviewDto,
  CodexRecoveryResetPreviewDto,
  CodexRecoveryUpdatePreviewDto,
  ConfigOperationResult,
  IpcErrorDto,
  MenuSnapshotDto,
  MutationResultDto,
  ReachabilityResult,
  RecoveryHealthDto,
  RecoverySnapshotDto,
  ReorderRoutesAndFallbackInputDto,
  RouteActivationPreviewDto,
  RouteActivationResultDto,
  RouteEditDto,
  RouteId,
  RouteSaveInputDto,
  RouteSaveResultDto,
  SettingsSnapshotDto,
  UpdateImagesGenerationSettingsInputDto,
  StateChangedEventDto,
  UsageHistoryPageDto,
  UsageHistoryQueryDto,
  UsageRequestDetailDto,
  UsageRouteOptionDto,
  UsageStatisticsDto,
  UsageStatisticsQueryDto,
} from "../generated";

export const IPC_COMMANDS = {
  getBootstrapSnapshot: "get_bootstrap_snapshot",
  getMenuSnapshot: "get_menu_snapshot",
  getSettingsSnapshot: "get_settings_snapshot",
  getApplicationUpdateSnapshot: "get_application_update_snapshot",
  checkApplicationUpdate: "check_application_update",
  downloadAndInstallApplicationUpdate:
    "download_and_install_application_update",
  openApplicationUpdateRelease: "open_application_update_release",
  restartForApplicationUpdate: "restart_for_application_update",
  getUsageHistory: "get_usage_history",
  getUsageStatistics: "get_usage_statistics",
  getUsageRouteOptions: "get_usage_route_options",
  getUsageRequestDetail: "get_usage_request_detail",
  getRecoverySnapshot: "get_recovery_snapshot",
  createRecoveryPoint: "create_recovery_point",
  restoreRecoveryPoint: "restore_recovery_point",
  startOverDatabase: "start_over_database",
  retryDatabaseStartup: "retry_database_startup",
  getRouteEdit: "get_route_edit",
  saveRoute: "save_route",
  deleteRoute: "delete_route",
  previewRouteActivation: "preview_route_activation",
  confirmRouteActivation: "confirm_route_activation",
  dismissCodexRestartNotice: "dismiss_codex_restart_notice",
  setFallbackEnabled: "set_fallback_enabled",
  reorderRoutesAndFallback: "reorder_routes_and_fallback",
  updateBalanceQuerySettings: "update_balance_query_settings",
  updateImagesGenerationSettings: "update_images_generation_settings",
  updateAppearancePreference: "update_appearance_preference",
  refreshBalance: "refresh_balance",
  refreshAllBalances: "refresh_all_balances",
  testBalanceQuery: "test_balance_query",
  checkRouteReachability: "check_route_reachability",
  applyProxyPort: "apply_proxy_port",
  connectCodex: "connect_codex",
  reconnectCodex: "reconnect_codex",
  previewCodexImagesMcpRepair: "preview_codex_images_mcp_repair",
  confirmCodexImagesMcpRepair: "confirm_codex_images_mcp_repair",
  previewUpdateCodexRecovery: "preview_update_codex_recovery",
  confirmUpdateCodexRecovery: "confirm_update_codex_recovery",
  previewResetCodexRecoveryToBaseline:
    "preview_reset_codex_recovery_to_baseline",
  confirmResetCodexRecoveryToBaseline:
    "confirm_reset_codex_recovery_to_baseline",
  restoreCodex: "restore_codex",
  clearRequestHistory: "clear_request_history",
  openCodexConfig: "open_codex_config",
  openRuntimeLogDirectory: "open_runtime_log_directory",
  clearRuntimeLogs: "clear_runtime_logs",
  showSettingsWindow: "show_settings_window",
  quitApplication: "quit_application",
  menuFrontendReady: "menu_frontend_ready",
  completeMenuShow: "complete_menu_show",
  setMenuUsagePreview: "set_menu_usage_preview",
  hideMenu: "hide_menu",
  hideSettingsWindow: "hide_settings_window",
} as const;

export const IPC_EVENTS = {
  stateChanged: "router-state-changed",
  menuPrepareShow: "menu-prepare-show",
  menuPositioned: "menu-positioned",
  settingsNavigate: "settings-navigate",
  settingsCloseRequested: "settings-close-requested",
} as const;

export interface MenuPrepareEvent {
  generation: number;
}

export interface MenuPositionedEvent extends MenuPrepareEvent {
  arrowOffsetX: number;
  previewSide: "left" | "right";
  previewWidth: number;
  previewHeight: number;
}

export interface SettingsNavigationEvent {
  section: "routes" | "usage" | "codex" | "system";
  createNewRoute: boolean;
}

export function isTauriRuntime(): boolean {
  return "__TAURI_INTERNALS__" in window;
}

export async function getBootstrapSnapshot(): Promise<BootstrapSnapshotDto> {
  return invoke<BootstrapSnapshotDto>(IPC_COMMANDS.getBootstrapSnapshot);
}

export async function getMenuSnapshot(): Promise<MenuSnapshotDto> {
  return invoke<MenuSnapshotDto>(IPC_COMMANDS.getMenuSnapshot);
}

export async function getSettingsSnapshot(): Promise<SettingsSnapshotDto> {
  return invoke<SettingsSnapshotDto>(IPC_COMMANDS.getSettingsSnapshot);
}

export async function getApplicationUpdateSnapshot(): Promise<ApplicationUpdateSnapshotDto> {
  if (import.meta.env.DEV && !isTauriRuntime()) {
    const { previewApplicationUpdateSnapshot } =
      await import("../previewFixtures");
    return structuredClone(previewApplicationUpdateSnapshot());
  }
  return invoke<ApplicationUpdateSnapshotDto>(
    IPC_COMMANDS.getApplicationUpdateSnapshot,
  );
}

export async function checkApplicationUpdate(): Promise<ApplicationUpdateSnapshotDto> {
  return invoke<ApplicationUpdateSnapshotDto>(
    IPC_COMMANDS.checkApplicationUpdate,
  );
}

export async function downloadAndInstallApplicationUpdate(
  onProgress: (progress: ApplicationUpdateProgressDto) => void,
): Promise<ApplicationUpdateSnapshotDto> {
  const channel = new Channel<ApplicationUpdateProgressDto>();
  channel.onmessage = onProgress;
  return invoke<ApplicationUpdateSnapshotDto>(
    IPC_COMMANDS.downloadAndInstallApplicationUpdate,
    { onProgress: channel },
  );
}

export async function openApplicationUpdateRelease(): Promise<void> {
  return invoke<void>(IPC_COMMANDS.openApplicationUpdateRelease);
}

export async function restartForApplicationUpdate(): Promise<void> {
  return invoke<void>(IPC_COMMANDS.restartForApplicationUpdate);
}

export async function getUsageHistory(
  query: UsageHistoryQueryDto,
): Promise<UsageHistoryPageDto> {
  if (import.meta.env.DEV && !isTauriRuntime()) {
    const { previewUsageHistoryForQuery } = await import("../previewFixtures");
    return structuredClone(previewUsageHistoryForQuery(query));
  }
  return invoke<UsageHistoryPageDto>(IPC_COMMANDS.getUsageHistory, { query });
}

export async function getUsageStatistics(
  query: UsageStatisticsQueryDto,
): Promise<UsageStatisticsDto> {
  if (import.meta.env.DEV && !isTauriRuntime()) {
    const { previewUsageStatisticsForQuery } =
      await import("../previewFixtures");
    return structuredClone(previewUsageStatisticsForQuery(query));
  }
  return invoke<UsageStatisticsDto>(IPC_COMMANDS.getUsageStatistics, { query });
}

export async function getUsageRouteOptions(): Promise<
  Array<UsageRouteOptionDto>
> {
  if (import.meta.env.DEV && !isTauriRuntime()) {
    const { previewUsageRouteOptions } = await import("../previewFixtures");
    return structuredClone(previewUsageRouteOptions);
  }
  return invoke<Array<UsageRouteOptionDto>>(IPC_COMMANDS.getUsageRouteOptions);
}

export async function getUsageRequestDetail(
  requestId: string,
): Promise<UsageRequestDetailDto> {
  if (import.meta.env.DEV && !isTauriRuntime()) {
    const { previewUsageRequestDetails } = await import("../previewFixtures");
    const detail = previewUsageRequestDetails.find(
      (candidate) => candidate.request.requestId === requestId,
    );
    if (detail) return structuredClone(detail);
  }
  return invoke<UsageRequestDetailDto>(IPC_COMMANDS.getUsageRequestDetail, {
    requestId,
  });
}

export async function getRecoverySnapshot(): Promise<RecoverySnapshotDto> {
  return invoke<RecoverySnapshotDto>(IPC_COMMANDS.getRecoverySnapshot);
}

export async function createRecoveryPoint(): Promise<RecoveryHealthDto> {
  return invoke<RecoveryHealthDto>(IPC_COMMANDS.createRecoveryPoint);
}

export async function restoreRecoveryPoint(
  pointId: string,
): Promise<AppLifecycleSnapshot> {
  return invoke<AppLifecycleSnapshot>(IPC_COMMANDS.restoreRecoveryPoint, {
    pointId,
  });
}

export async function startOverDatabase(): Promise<AppLifecycleSnapshot> {
  return invoke<AppLifecycleSnapshot>(IPC_COMMANDS.startOverDatabase);
}

export async function retryDatabaseStartup(): Promise<AppLifecycleSnapshot> {
  return invoke<AppLifecycleSnapshot>(IPC_COMMANDS.retryDatabaseStartup);
}

export async function getRouteEdit(routeId: RouteId): Promise<RouteEditDto> {
  if (import.meta.env.DEV && !isTauriRuntime()) {
    const { previewRouteEdits } = await import("../previewFixtures");
    const edit = previewRouteEdits.find(
      (candidate) => candidate.routeId === routeId,
    );
    if (edit) return structuredClone(edit);
  }
  return invoke<RouteEditDto>(IPC_COMMANDS.getRouteEdit, { routeId });
}

export async function saveRoute(
  input: RouteSaveInputDto,
): Promise<RouteSaveResultDto> {
  return invoke<RouteSaveResultDto>(IPC_COMMANDS.saveRoute, { input });
}

export async function deleteRoute(
  routeId: RouteId,
): Promise<MutationResultDto> {
  return invoke<MutationResultDto>(IPC_COMMANDS.deleteRoute, { routeId });
}

export async function previewRouteActivation(
  routeId: RouteId,
): Promise<RouteActivationPreviewDto> {
  return invoke<RouteActivationPreviewDto>(
    IPC_COMMANDS.previewRouteActivation,
    { routeId },
  );
}

export async function confirmRouteActivation(
  permit: string,
): Promise<RouteActivationResultDto> {
  return invoke<RouteActivationResultDto>(IPC_COMMANDS.confirmRouteActivation, {
    permit,
  });
}

export async function dismissCodexRestartNotice(
  noticeId: string,
): Promise<MutationResultDto> {
  return invoke<MutationResultDto>(IPC_COMMANDS.dismissCodexRestartNotice, {
    noticeId,
  });
}

export async function setFallbackEnabled(
  enabled: boolean,
): Promise<MutationResultDto> {
  return invoke<MutationResultDto>(IPC_COMMANDS.setFallbackEnabled, {
    enabled,
  });
}

export async function reorderRoutesAndFallback(
  input: ReorderRoutesAndFallbackInputDto,
): Promise<MutationResultDto> {
  return invoke<MutationResultDto>(IPC_COMMANDS.reorderRoutesAndFallback, {
    input,
  });
}

export async function updateBalanceQuerySettings(
  input: BalanceQuerySettingsDto,
): Promise<MutationResultDto> {
  return invoke<MutationResultDto>(IPC_COMMANDS.updateBalanceQuerySettings, {
    input,
  });
}

export async function updateAppearancePreference(
  appearancePreference: AppearancePreference,
): Promise<MutationResultDto> {
  return invoke<MutationResultDto>(IPC_COMMANDS.updateAppearancePreference, {
    appearancePreference,
  });
}

export async function updateImagesGenerationSettings(
  input: UpdateImagesGenerationSettingsInputDto,
): Promise<MutationResultDto> {
  return invoke<MutationResultDto>(
    IPC_COMMANDS.updateImagesGenerationSettings,
    {
      input,
    },
  );
}

export async function refreshBalance(
  routeId: RouteId,
): Promise<BalanceDisplaySnapshot> {
  return invoke<BalanceDisplaySnapshot>(IPC_COMMANDS.refreshBalance, {
    routeId,
  });
}

export async function refreshAllBalances(): Promise<BalanceRefreshBatchState> {
  return invoke<BalanceRefreshBatchState>(IPC_COMMANDS.refreshAllBalances);
}

export async function testBalanceQuery(
  input: BalanceTestInputDto,
): Promise<BalanceResult> {
  if (import.meta.env.DEV && !isTauriRuntime()) {
    return {
      isValid: true,
      remaining: 263.71,
      used: null,
      total: null,
      unit: "USD",
      planName: null,
      invalidMessage: null,
      extra: null,
    };
  }
  return invoke<BalanceResult>(IPC_COMMANDS.testBalanceQuery, { input });
}

export async function checkRouteReachability(
  baseUrl: string,
): Promise<ReachabilityResult> {
  if (import.meta.env.DEV && !isTauriRuntime()) {
    return { status: "reachable", ttfbMs: 186, errorCategory: null };
  }
  return invoke<ReachabilityResult>(IPC_COMMANDS.checkRouteReachability, {
    baseUrl,
  });
}

export async function applyProxyPort(
  port: number,
): Promise<AppLifecycleSnapshot> {
  return invoke<AppLifecycleSnapshot>(IPC_COMMANDS.applyProxyPort, { port });
}

export async function connectCodex(
  allowWithoutRoute: boolean,
): Promise<ConfigOperationResult> {
  return invoke<ConfigOperationResult>(IPC_COMMANDS.connectCodex, {
    allowWithoutRoute,
  });
}

export async function reconnectCodex(): Promise<ConfigOperationResult> {
  return invoke<ConfigOperationResult>(IPC_COMMANDS.reconnectCodex);
}

export async function previewCodexImagesMcpRepair(): Promise<CodexImagesMcpRepairPreviewDto> {
  return invoke<CodexImagesMcpRepairPreviewDto>(
    IPC_COMMANDS.previewCodexImagesMcpRepair,
  );
}

export async function confirmCodexImagesMcpRepair(
  permit: string,
): Promise<ConfigOperationResult> {
  return invoke<ConfigOperationResult>(
    IPC_COMMANDS.confirmCodexImagesMcpRepair,
    {
      permit,
    },
  );
}

export async function previewUpdateCodexRecovery(): Promise<CodexRecoveryUpdatePreviewDto> {
  return invoke<CodexRecoveryUpdatePreviewDto>(
    IPC_COMMANDS.previewUpdateCodexRecovery,
  );
}

export async function confirmUpdateCodexRecovery(
  permit: string,
): Promise<ConfigOperationResult> {
  return invoke<ConfigOperationResult>(
    IPC_COMMANDS.confirmUpdateCodexRecovery,
    { permit },
  );
}

export async function previewResetCodexRecoveryToBaseline(): Promise<CodexRecoveryResetPreviewDto> {
  return invoke<CodexRecoveryResetPreviewDto>(
    IPC_COMMANDS.previewResetCodexRecoveryToBaseline,
  );
}

export async function confirmResetCodexRecoveryToBaseline(
  permit: string,
): Promise<ConfigOperationResult> {
  return invoke<ConfigOperationResult>(
    IPC_COMMANDS.confirmResetCodexRecoveryToBaseline,
    { permit },
  );
}

export async function restoreCodex(): Promise<ConfigOperationResult> {
  return invoke<ConfigOperationResult>(IPC_COMMANDS.restoreCodex);
}

export async function clearRequestHistory(): Promise<MutationResultDto> {
  return invoke<MutationResultDto>(IPC_COMMANDS.clearRequestHistory);
}

export async function openCodexConfig(): Promise<void> {
  return invoke<void>(IPC_COMMANDS.openCodexConfig);
}

export async function openRuntimeLogDirectory(): Promise<void> {
  return invoke<void>(IPC_COMMANDS.openRuntimeLogDirectory);
}

export async function clearRuntimeLogs(): Promise<MutationResultDto> {
  return invoke<MutationResultDto>(IPC_COMMANDS.clearRuntimeLogs);
}

export async function showSettingsWindow(
  section: SettingsNavigationEvent["section"],
  createNewRoute = false,
): Promise<void> {
  return invoke<void>(IPC_COMMANDS.showSettingsWindow, {
    section,
    createNewRoute,
  });
}

export async function quitApplication(): Promise<void> {
  return invoke<void>(IPC_COMMANDS.quitApplication);
}

export async function menuFrontendReady(): Promise<void> {
  return invoke<void>(IPC_COMMANDS.menuFrontendReady);
}

export async function completeMenuShow(
  generation: number,
  height: number,
): Promise<void> {
  return invoke<void>(IPC_COMMANDS.completeMenuShow, { generation, height });
}

export async function setMenuUsagePreview(
  generation: number,
  revision: number,
  open: boolean,
): Promise<void> {
  return invoke<void>(IPC_COMMANDS.setMenuUsagePreview, {
    generation,
    revision,
    open,
  });
}

export async function hideMenu(): Promise<void> {
  return invoke<void>(IPC_COMMANDS.hideMenu);
}

export async function hideSettingsWindow(): Promise<void> {
  return invoke<void>(IPC_COMMANDS.hideSettingsWindow);
}

export async function listenStateChanged(
  listener: (event: StateChangedEventDto) => void,
): Promise<UnlistenFn> {
  return listen<StateChangedEventDto>(IPC_EVENTS.stateChanged, (event) =>
    listener(event.payload),
  );
}

export async function listenMenuPrepare(
  listener: (event: MenuPrepareEvent) => void,
): Promise<UnlistenFn> {
  return listen<MenuPrepareEvent>(IPC_EVENTS.menuPrepareShow, (event) =>
    listener(event.payload),
  );
}

export async function listenMenuPositioned(
  listener: (event: MenuPositionedEvent) => void,
): Promise<UnlistenFn> {
  return listen<MenuPositionedEvent>(IPC_EVENTS.menuPositioned, (event) =>
    listener(event.payload),
  );
}

export async function listenSettingsNavigation(
  listener: (event: SettingsNavigationEvent) => void,
): Promise<UnlistenFn> {
  return listen<SettingsNavigationEvent>(IPC_EVENTS.settingsNavigate, (event) =>
    listener(event.payload),
  );
}

export async function listenSettingsCloseRequested(
  listener: () => void,
): Promise<UnlistenFn> {
  return listen<void>(IPC_EVENTS.settingsCloseRequested, listener);
}

export function normalizeIpcError(error: unknown): IpcErrorDto {
  if (typeof error === "object" && error !== null) {
    const candidate = error as Partial<IpcErrorDto>;
    if (
      typeof candidate.code === "string" &&
      typeof candidate.message === "string"
    ) {
      return {
        code: candidate.code,
        message: candidate.message.slice(0, 512),
        retryable: candidate.retryable === true,
        field: typeof candidate.field === "string" ? candidate.field : null,
      };
    }
  }
  return {
    code: "ipc_failed",
    message: "Unable to complete the request.",
    retryable: true,
    field: null,
  };
}
