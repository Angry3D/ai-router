import {
  AlertCircle,
  Circle,
  CircleCheck,
  LoaderCircle,
  Plus,
  Power,
  RefreshCw,
  Settings,
  TriangleAlert,
  X,
} from "lucide-react";
import { useMemo, useRef, useState, type ReactNode } from "react";
import { useQueryClient } from "@tanstack/react-query";

import {
  confirmRouteActivation,
  connectCodex,
  dismissCodexRestartNotice,
  dismissMcpImageCapacityWarning,
  hideMenu,
  normalizeIpcError,
  quitApplication,
  reconnectCodex,
  refreshAllBalances,
  refreshBalance,
  previewRouteActivation,
  restoreCodex,
  setFallbackEnabled,
  showSettingsWindow,
} from "../../api/ipc";
import {
  isDatabaseSnapshotBlocked,
  queryKeys,
  useBootstrapSnapshot,
  useApplicationUpdateSnapshot,
  useMenuSnapshot,
} from "../../api/query";
import { appVariant } from "../../appVariant";
import type { MenuMessages } from "../language/catalog/menu";
import { ipcErrorMessage, useLanguage } from "../language/useLanguage";
import { AppScrollArea } from "../shared/AppScrollArea";
import type {
  AppLocale,
  BalanceDisplaySnapshot,
  BootstrapSnapshotDto,
  CodexConfigStatus,
  FallbackStateDto,
  IpcErrorDto,
  MenuSnapshotDto,
  ProxyRuntimeStatus,
  RouteActivationPreviewDto,
  RouteId,
} from "../../generated";
import { useMenuPopover } from "./useMenuPopover";
import { MenuUsagePreview } from "./MenuUsagePreview";
import { useMenuUsagePreview } from "./useMenuUsagePreview";

function databaseIssueLabel(
  snapshot: BootstrapSnapshotDto,
  messages: MenuMessages,
) {
  const issue = snapshot.lifecycle.issue;
  return typeof issue === "object" && issue !== null
    ? messages.databaseIssue[issue.database]
    : messages.databaseIssue.unavailable;
}

function inferenceLabel(
  status: MenuSnapshotDto["bootstrap"]["routes"][number]["inferenceStatus"],
  messages: MenuMessages,
) {
  if (status.kind === "recent_failure" && status.failureReason) {
    return messages.failureReason[status.failureReason];
  }
  return messages.inferenceStatus[status.kind];
}

type RuntimeStatusTone = "good" | "warning" | "bad";

function proxyLabel(
  status: ProxyRuntimeStatus,
  messages: MenuMessages["proxyStatus"],
): readonly [string, RuntimeStatusTone] {
  switch (status) {
    case "running":
      return [messages.running, "good"];
    case "starting":
      return [messages.starting, "warning"];
    case "port_conflict":
      return [messages.port_conflict, "bad"];
    case "database_error":
      return [messages.database_error, "bad"];
    case "shutting_down":
      return [messages.shutting_down, "warning"];
    case "stopped":
      return [messages.stopped, "bad"];
    case "error":
      return [messages.error, "bad"];
  }
}

type CodexControlAction =
  "connect" | "reconnect" | "disconnect" | "settings" | null;

function codexControlPresentation(
  status: CodexConfigStatus,
  hasActiveRoute: boolean,
  messages: MenuMessages["codexControl"],
) {
  switch (status) {
    case "connected":
      return {
        state: messages.connectedState,
        action: messages.disconnectAction,
        pending: messages.disconnecting,
        tone: "good" as RuntimeStatusTone,
        activation: "disconnect" as CodexControlAction,
        unavailable: false,
      };
    case "checking":
      return {
        state: messages.checkingState,
        action: messages.checkingAction,
        pending: null,
        tone: "warning" as RuntimeStatusTone,
        activation: null,
        unavailable: true,
      };
    case "changed":
      return {
        state: messages.changedState,
        action: hasActiveRoute
          ? messages.reconnectAction
          : messages.selectRouteFirst,
        pending: messages.connecting,
        tone: "warning" as RuntimeStatusTone,
        activation: "reconnect" as CodexControlAction,
        unavailable: !hasActiveRoute,
      };
    case "not_connected":
      return {
        state: messages.notConnectedState,
        action: hasActiveRoute
          ? messages.connectAction
          : messages.selectRouteFirst,
        pending: messages.connecting,
        tone: "warning" as RuntimeStatusTone,
        activation: "connect" as CodexControlAction,
        unavailable: !hasActiveRoute,
      };
    case "images_mcp_name_conflict":
    case "images_mcp_projection_conflict":
      return {
        state: messages.imagesConflictState,
        action: messages.imagesConflictAction,
        pending: null,
        tone: "bad" as RuntimeStatusTone,
        activation: "settings" as CodexControlAction,
        unavailable: false,
      };
    case "invalid":
    case "unreadable":
    case "symlink_unsupported":
      return {
        state: messages.configErrorState,
        action: messages.configErrorAction,
        pending: null,
        tone: "bad" as RuntimeStatusTone,
        activation: "settings" as CodexControlAction,
        unavailable: false,
      };
  }
}

function fallbackControlPresentation(
  fallback: FallbackStateDto,
  effectiveParticipantCount: number,
  messages: MenuMessages["fallbackControl"],
) {
  if (effectiveParticipantCount < 2) {
    return {
      state: messages.unavailableState,
      action: messages.minimumRoutesAction,
      tone: "warning" as RuntimeStatusTone,
      unavailable: true,
    };
  }
  if (!fallback.enabled) {
    return {
      state: messages.disabledState,
      action: messages.enableAction,
      tone: "warning" as RuntimeStatusTone,
      unavailable: false,
    };
  }
  return {
    state: messages.enabledState,
    action: messages.disableAction,
    tone: "good" as RuntimeStatusTone,
    unavailable: false,
  };
}

function MenuFallbackBoundary({
  edge,
  enabled,
  messages,
}: {
  edge: "start" | "end";
  enabled: boolean;
  messages: MenuMessages["fallbackControl"];
}) {
  return (
    <span
      className={`menu-fallback-boundary menu-fallback-boundary-${edge}${enabled ? "" : " is-disabled"}`}
      aria-hidden="true"
    >
      <span>
        {enabled ? messages.boundaryIncluded : messages.boundaryDisabled}
      </span>
    </span>
  );
}

function balanceLabel(
  balance: BalanceDisplaySnapshot | undefined,
  enabled: boolean,
  messages: MenuMessages["balance"],
) {
  if (!enabled) return messages.notConfigured;
  if (!balance || balance.status === "unavailable") return messages.none;
  if (balance.status === "failed") return messages.queryFailed;
  if (!balance.value) return messages.none;
  const value = balance.value.remaining;
  const unit = balance.value?.unit ?? "";
  const amount =
    value === null || value === undefined
      ? messages.available
      : ["$", "¥", "€", "£"].includes(unit)
        ? `${unit}${value.toFixed(2)}`
        : unit === "USD"
          ? `$${value.toFixed(2)}`
          : `${value.toFixed(2)}${unit ? ` ${unit}` : ""}`;
  return amount;
}

function formatBalanceTime(timestamp: number, locale: AppLocale) {
  return new Intl.DateTimeFormat(locale, {
    hour: "2-digit",
    minute: "2-digit",
  }).format(timestamp);
}

function balanceMetaLabel(
  balance: BalanceDisplaySnapshot | undefined,
  messages: MenuMessages["balance"],
  locale: AppLocale,
) {
  if (!balance) return null;
  if (balance.status === "stale") {
    const refreshedAt = balance.lastCompletionAtMs ?? balance.lastSuccessAtMs;
    return refreshedAt
      ? `${formatBalanceTime(refreshedAt, locale)} · ${messages.pendingRefresh}`
      : messages.pendingRefresh;
  }
  if (balance.status === "last_good") {
    return balance.lastSuccessAtMs
      ? `${formatBalanceTime(balance.lastSuccessAtMs, locale)} · ${messages.lastResult}`
      : messages.lastResult;
  }
  const refreshedAt = balance.lastCompletionAtMs ?? balance.lastSuccessAtMs;
  return refreshedAt
    ? messages.refreshedAt(formatBalanceTime(refreshedAt, locale))
    : null;
}

function batchLabel(
  snapshot: MenuSnapshotDto,
  messages: MenuMessages["balanceBatch"],
) {
  const batch = snapshot.balanceBatch;
  if (!batch) {
    return snapshot.balanceEnabledRouteIds.length === 0
      ? messages.nothingToRefresh
      : messages.refreshAll;
  }
  if (batch.phase === "running")
    return messages.refreshing(batch.completedCount, batch.eligibleCount);
  if (batch.eligibleCount === 0) return messages.nothingToRefresh;
  if (batch.failureCount === 0) return messages.allSucceeded;
  return messages.partial(
    batch.successCount,
    batch.eligibleCount,
    batch.failureCount,
  );
}

function MenuConfirmDialog({
  titleId,
  title,
  body,
  cancelLabel,
  confirmLabel,
  pending = false,
  destructive = false,
  onCancel,
  onConfirm,
}: {
  titleId: string;
  title: string;
  body: ReactNode;
  cancelLabel: string;
  confirmLabel: string;
  pending?: boolean;
  destructive?: boolean;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  return (
    <div className="dialog-backdrop" role="presentation">
      <section
        className="confirm-dialog"
        role="alertdialog"
        aria-modal="true"
        aria-labelledby={titleId}
      >
        <h2 id={titleId}>{title}</h2>
        <p>{body}</p>
        <div className="dialog-actions">
          <button
            type="button"
            className="secondary-button"
            autoFocus
            disabled={pending}
            onClick={onCancel}
          >
            {cancelLabel}
          </button>
          <button
            type="button"
            className={destructive ? "danger-button" : "primary-button"}
            disabled={pending}
            onClick={onConfirm}
          >
            {confirmLabel}
          </button>
        </div>
      </section>
    </div>
  );
}

export function MenuPopover() {
  const applicationUpdate = useApplicationUpdateSnapshot();
  const shellRef = useRef<HTMLElement>(null);
  const queryClient = useQueryClient();
  const bootstrap = useBootstrapSnapshot();
  const databaseBlocked = isDatabaseSnapshotBlocked(
    bootstrap.data?.lifecycle.phase,
  );
  const query = useMenuSnapshot(bootstrap.isSuccess && !databaseBlocked);
  const { messages, effectiveLocale } = useLanguage();
  const menuMessages = messages.menu;
  const { generation, previewLayout } = useMenuPopover(shellRef);
  const [pendingRoute, setPendingRoute] = useState<RouteId | null>(null);
  const [activationPreview, setActivationPreview] =
    useState<RouteActivationPreviewDto | null>(null);
  const [disconnectConfirmation, setDisconnectConfirmation] = useState(false);
  const [dismissedNoticeId, setDismissedNoticeId] = useState<string | null>(
    null,
  );
  const [dismissingNoticeId, setDismissingNoticeId] = useState<string | null>(
    null,
  );
  const [dismissedCapacityEpisodeId, setDismissedCapacityEpisodeId] = useState<
    string | null
  >(null);
  const [dismissingCapacityEpisodeId, setDismissingCapacityEpisodeId] =
    useState<string | null>(null);
  const [refreshingRoute, setRefreshingRoute] = useState<RouteId | null>(null);
  const [refreshingAll, setRefreshingAll] = useState(false);
  const [codexOperation, setCodexOperation] = useState<
    "connect" | "disconnect" | null
  >(null);
  const [fallbackOperation, setFallbackOperation] = useState<
    "enable" | "disable" | null
  >(null);
  const [error, setError] = useState<IpcErrorDto | null>(null);
  const [incompleteSwitch, setIncompleteSwitch] = useState(false);

  const balances = useMemo(
    () =>
      new Map(
        query.data?.balances.map((balance) => [balance.routeId, balance]) ?? [],
      ),
    [query.data?.balances],
  );
  const routeIds = useMemo(
    () => query.data?.bootstrap.routes.filter((route) => route.menuVisible !== false).map((route) => route.routeId) ?? [],
    [query.data?.bootstrap.routes],
  );
  const visibleRoutes = useMemo(
    () => query.data?.bootstrap.routes.filter((route) => route.menuVisible !== false) ?? [],
    [query.data?.bootstrap.routes],
  );
  const usagePreview = useMenuUsagePreview({ generation, routeIds });
  const usagePreviewRoute = query.data?.bootstrap.routes.find(
    (route) => route.routeId === usagePreview.targetRouteId,
  );
  const balanceEnabledRoutes = useMemo(
    () => new Set(query.data?.balanceEnabledRouteIds ?? []),
    [query.data?.balanceEnabledRouteIds],
  );

  const refreshMenu = async () => {
    await queryClient.invalidateQueries({ queryKey: queryKeys.menu });
  };

  // The inline alert slot shows either a typed IPC error or the partial
  // route-switch notice, so both clear together before each operation.
  const clearError = () => {
    setError(null);
    setIncompleteSwitch(false);
  };

  const finishActivation = async (preview: RouteActivationPreviewDto) => {
    setPendingRoute(preview.targetRouteId);
    clearError();
    try {
      const result = await confirmRouteActivation(preview.permit);
      setActivationPreview(null);
      await refreshMenu();
      if (!result.catalog.projectionApplied) {
        setIncompleteSwitch(true);
        return;
      }
      await hideMenu();
    } catch (reason) {
      const normalized = normalizeIpcError(reason);
      setError(normalized);
      if (
        normalized.code === "route_activation_permit_stale" ||
        normalized.code === "route_activation_permit_invalid"
      ) {
        try {
          setActivationPreview(
            await previewRouteActivation(preview.targetRouteId),
          );
        } catch {
          // Keep the stale confirmation visible with the authoritative error.
        }
      }
    } finally {
      setPendingRoute(null);
    }
  };

  const performActivate = async (routeId: RouteId) => {
    setPendingRoute(routeId);
    clearError();
    try {
      const preview = await previewRouteActivation(routeId);
      if (preview.confirmationRequired) {
        setActivationPreview(preview);
      } else {
        await finishActivation(preview);
      }
    } catch (reason) {
      setError(normalizeIpcError(reason));
    } finally {
      setPendingRoute(null);
    }
  };

  const dismissRestartNotice = async (noticeId: string) => {
    setDismissedNoticeId(noticeId);
    setDismissingNoticeId(noticeId);
    clearError();
    try {
      await dismissCodexRestartNotice(noticeId);
      await refreshMenu();
    } catch (reason) {
      setDismissedNoticeId((current) =>
        current === noticeId ? null : current,
      );
      setError(normalizeIpcError(reason));
    } finally {
      setDismissingNoticeId((current) =>
        current === noticeId ? null : current,
      );
    }
  };

  const dismissCapacityWarning = async (episodeId: string) => {
    setDismissedCapacityEpisodeId(episodeId);
    setDismissingCapacityEpisodeId(episodeId);
    clearError();
    try {
      await dismissMcpImageCapacityWarning(episodeId);
      await refreshMenu();
    } catch (reason) {
      setDismissedCapacityEpisodeId((current) =>
        current === episodeId ? null : current,
      );
      setError(normalizeIpcError(reason));
    } finally {
      setDismissingCapacityEpisodeId((current) =>
        current === episodeId ? null : current,
      );
    }
  };

  const refreshOne = async (routeId: RouteId) => {
    setRefreshingRoute(routeId);
    clearError();
    try {
      await refreshBalance(routeId);
      await refreshMenu();
    } catch (reason) {
      setError(normalizeIpcError(reason));
    } finally {
      setRefreshingRoute(null);
    }
  };

  const refreshAll = async () => {
    setRefreshingAll(true);
    clearError();
    try {
      await refreshAllBalances();
      await refreshMenu();
    } catch (reason) {
      setError(normalizeIpcError(reason));
    } finally {
      setRefreshingAll(false);
    }
  };

  const connectCodexFromMenu = async () => {
    const status = query.data?.codexStatus;
    if (
      codexOperation !== null ||
      (status !== "not_connected" && status !== "changed")
    )
      return;
    setCodexOperation("connect");
    clearError();
    try {
      if (status === "changed") await reconnectCodex();
      else await connectCodex(false);
      await refreshMenu();
    } catch (reason) {
      setError(normalizeIpcError(reason));
    } finally {
      setCodexOperation(null);
    }
  };

  const disconnectCodexFromMenu = async () => {
    if (codexOperation !== null) return;
    setCodexOperation("disconnect");
    clearError();
    try {
      await restoreCodex();
      await refreshMenu();
    } catch (reason) {
      setError(normalizeIpcError(reason));
    } finally {
      setCodexOperation(null);
    }
  };

  const changeFallbackFromMenu = async () => {
    const fallback = query.data?.bootstrap.fallback;
    if (
      !fallback ||
      fallbackOperation !== null ||
      effectiveFallbackParticipantCount < 2
    ) {
      return;
    }
    const nextEnabled = !fallback.enabled;
    setFallbackOperation(nextEnabled ? "enable" : "disable");
    clearError();
    try {
      await setFallbackEnabled(nextEnabled);
      await refreshMenu();
    } catch (reason) {
      setError(normalizeIpcError(reason));
    } finally {
      setFallbackOperation(null);
    }
  };

  const batchRefreshing =
    refreshingAll || query.data?.balanceBatch?.phase === "running";
  const codexControl = query.data
    ? codexControlPresentation(
        query.data.codexStatus,
        query.data.bootstrap.activeRouteId !== null,
        menuMessages.codexControl,
      )
    : null;
  const fallbackBoundaryIndex = query.data
    ? Math.min(
        Math.max(query.data.bootstrap.fallback.participantCount, 0),
        query.data.bootstrap.routes.length,
      )
    : null;
  const effectiveFallbackParticipantCount =
    query.data && fallbackBoundaryIndex !== null
      ? query.data.bootstrap.routes
          .slice(0, fallbackBoundaryIndex)
          .filter((route) => route.menuVisible !== false).length
      : 0;
  const fallbackControl = query.data
    ? fallbackControlPresentation(
        query.data.bootstrap.fallback,
        effectiveFallbackParticipantCount,
        menuMessages.fallbackControl,
      )
    : null;
  const fallbackMissingNext =
    query.data !== undefined &&
    query.data.bootstrap.fallback.enabled &&
    !query.data.bootstrap.fallback.hasNext;
  const fallbackBoundaryOwner = (() => {
    if (fallbackBoundaryIndex === null || visibleRoutes.length === 0) return null;
    if (fallbackBoundaryIndex === 0) {
      return { routeId: visibleRoutes[0].routeId, edge: "start" as const };
    }
    for (let index = visibleRoutes.length - 1; index >= 0; index -= 1) {
      const route = visibleRoutes[index];
      const fullIndex = query.data!.bootstrap.routes.findIndex(
        (item) => item.routeId === route.routeId,
      );
      if (fullIndex < fallbackBoundaryIndex) {
        return { routeId: route.routeId, edge: "end" as const };
      }
    }
    return { routeId: visibleRoutes[0].routeId, edge: "start" as const };
  })();
  const fallbackBoundaryEnabled = query.data
    ? query.data.bootstrap.fallback.enabled &&
      query.data.bootstrap.fallback.participantCount >= 2
    : false;
  const codexPendingLabel =
    codexOperation === "disconnect"
      ? menuMessages.codexControl.disconnecting
      : codexOperation === "connect"
        ? menuMessages.codexControl.connecting
        : null;
  const codexReservedPendingLabel = codexPendingLabel ?? codexControl?.pending;
  const fallbackPendingLabel =
    fallbackOperation === "enable"
      ? menuMessages.fallbackControl.enabling
      : menuMessages.fallbackControl.disabling;
  const restartNotice =
    query.data?.codexRestartNotice?.noticeId === dismissedNoticeId
      ? null
      : query.data?.codexRestartNotice;
  const capacity = query.data?.mcpImageCapacity;
  const capacityEpisodeId = capacity?.warningEpisodeId ?? null;
  const capacityWarningVisible =
    capacity?.available === true &&
    capacity.warningVisible &&
    capacityEpisodeId !== null &&
    capacityEpisodeId !== dismissedCapacityEpisodeId;
  const confirming = disconnectConfirmation || activationPreview !== null;

  return (
    <div
      className={`menu-window-layout menu-window-layout-${previewLayout.side}`}
    >
      <main
        className={`menu-shell${confirming ? " menu-shell-confirming" : ""}`}
        aria-label={menuMessages.shellLabel(appVariant.displayName)}
        ref={shellRef}
      >
        <div className="menu-arrow" aria-hidden="true" />
        <header className="menu-header">
          <div className="menu-header-primary">
            <div className="app-identity">
              <h1>AI Router</h1>
              {appVariant.badge ? (
                <span className="app-variant-badge">{appVariant.badge}</span>
              ) : null}
            </div>
          </div>
          {!databaseBlocked && query.data && codexControl && fallbackControl ? (
            <div
              className="menu-runtime-status"
              aria-label={menuMessages.runtimeStatusLabel}
            >
              <span
                className={`runtime-status runtime-status-${proxyLabel(query.data.bootstrap.proxyStatus, menuMessages.proxyStatus)[1]}`}
              >
                <i aria-hidden="true" />
                {proxyLabel(query.data.bootstrap.proxyStatus, menuMessages.proxyStatus)[0]}
              </span>
              <button
                className={`runtime-status runtime-control runtime-status-${codexControl.tone}${codexOperation ? " is-pending" : ""}`}
                type="button"
                disabled={codexOperation !== null}
                aria-disabled={codexControl.unavailable || undefined}
                aria-label={menuMessages.codexControl.controlLabel(
                  codexControl.state,
                  codexPendingLabel ?? codexControl.action,
                )}
                title={codexControl.action}
                onClick={() => {
                  if (codexOperation !== null || codexControl.unavailable)
                    return;
                  switch (codexControl.activation) {
                    case "connect":
                    case "reconnect":
                      void connectCodexFromMenu();
                      break;
                    case "disconnect":
                      setDisconnectConfirmation(true);
                      break;
                    case "settings":
                      void showSettingsWindow("codex");
                      break;
                    case null:
                      break;
                  }
                }}
              >
                <i aria-hidden="true" />
                <span className="runtime-control-copy" aria-hidden="true">
                  <span className="runtime-control-state">
                    {codexControl.state}
                  </span>
                  <span className="runtime-control-action">
                    {codexControl.action}
                  </span>
                  {codexReservedPendingLabel ? (
                    <span className="runtime-control-pending">
                      <LoaderCircle size={12} className="spin" />
                      {codexReservedPendingLabel}
                    </span>
                  ) : null}
                </span>
              </button>
              <button
                className={`runtime-status runtime-control runtime-status-${fallbackControl.tone}${fallbackOperation ? " is-pending" : ""}`}
                type="button"
                disabled={fallbackOperation !== null}
                aria-disabled={fallbackControl.unavailable || undefined}
                aria-pressed={query.data.bootstrap.fallback.enabled}
                aria-label={menuMessages.fallbackControl.controlLabel(
                  fallbackControl.state,
                  fallbackOperation ? fallbackPendingLabel : fallbackControl.action,
                  fallbackMissingNext ? menuMessages.fallbackControl.noNextRoute : null,
                )}
                title={
                  fallbackMissingNext
                    ? menuMessages.fallbackControl.noNextRoute
                    : fallbackControl.action
                }
                onClick={() => void changeFallbackFromMenu()}
              >
                <i aria-hidden="true" />
                <span className="runtime-control-copy" aria-hidden="true">
                  <span className="runtime-control-state">
                    {fallbackControl.state}
                  </span>
                  <span className="runtime-control-action">
                    {fallbackControl.action}
                  </span>
                  <span className="runtime-control-pending">
                    <LoaderCircle size={12} className="spin" />
                    {fallbackPendingLabel}
                  </span>
                </span>
              </button>
            </div>
          ) : bootstrap.data ? (
            <span
              className={`runtime-status runtime-status-${proxyLabel(bootstrap.data.proxyStatus, menuMessages.proxyStatus)[1]}`}
            >
              <i aria-hidden="true" />
              {proxyLabel(bootstrap.data.proxyStatus, menuMessages.proxyStatus)[0]}
            </span>
          ) : null}
        </header>

        {!databaseBlocked && restartNotice ? (
          <section
            className="menu-restart-notice"
            aria-label={menuMessages.restartNotice.label}
          >
            <TriangleAlert aria-hidden="true" size={17} />
            <p>
              <strong>
                {menuMessages.restartNotice.switchedTo(restartNotice.routeName)}
              </strong>
              <span>{menuMessages.restartNotice.body}</span>
            </p>
            <button
              type="button"
              aria-label={menuMessages.restartNotice.dismissLabel}
              title={menuMessages.restartNotice.dismissTitle}
              disabled={dismissingNoticeId === restartNotice.noticeId}
              onClick={() => void dismissRestartNotice(restartNotice.noticeId)}
            >
              <X aria-hidden="true" size={15} />
            </button>
          </section>
        ) : null}

        {!databaseBlocked && capacityWarningVisible && capacity ? (
          <section
            className="menu-restart-notice menu-capacity-notice"
            aria-label={menuMessages.capacityNotice.label}
          >
            <TriangleAlert aria-hidden="true" size={17} />
            <p>
              <strong>
                {menuMessages.capacityNotice.usage(
                  formatCapacityBytes(capacity.bytes),
                )}
              </strong>
              <span>
                {menuMessages.capacityNotice.threshold(
                  formatCapacityBytes(capacity.thresholdMib * 1024 ** 2),
                )}
              </span>
            </p>
            <button
              className="menu-capacity-manage"
              type="button"
              onClick={() =>
                void showSettingsWindow("codex", false, "image_generation")
              }
            >
              {menuMessages.capacityNotice.manage}
            </button>
            <button
              className="menu-capacity-dismiss"
              type="button"
              aria-label={menuMessages.capacityNotice.dismissLabel}
              title={menuMessages.capacityNotice.dismissLabel}
              disabled={dismissingCapacityEpisodeId === capacityEpisodeId}
              onClick={() => void dismissCapacityWarning(capacityEpisodeId)}
            >
              {dismissingCapacityEpisodeId === capacityEpisodeId ? (
                <LoaderCircle aria-hidden="true" className="spin" size={14} />
              ) : (
                <X aria-hidden="true" size={15} />
              )}
            </button>
          </section>
        ) : null}

        {!databaseBlocked && query.isPending ? (
          <div className="menu-message">{menuMessages.menuStates.loading}</div>
        ) : null}
        {!databaseBlocked && query.isError ? (
          <div className="menu-message menu-message-error">
            {bootstrap.data?.proxyStatus === "database_error"
              ? menuMessages.menuStates.proxyDatabaseError
              : bootstrap.data?.proxyStatus === "port_conflict"
                ? menuMessages.menuStates.proxyPortConflict
                : menuMessages.menuStates.loadFailed}
          </div>
        ) : null}
        {databaseBlocked && bootstrap.data ? (
          <section
            className="menu-empty"
            aria-labelledby="database-status-title"
          >
            <AlertCircle aria-hidden="true" size={20} />
            <h2 id="database-status-title">
              {bootstrap.data.lifecycle.phase === "recovery_required"
                ? menuMessages.databaseBlocked.recoveryRequiredTitle
                : databaseIssueLabel(bootstrap.data, menuMessages)}
            </h2>
            <p>
              {bootstrap.data.lifecycle.phase === "recovery_required"
                ? menuMessages.databaseBlocked.recoveryRequiredBody
                : menuMessages.databaseBlocked.issueBody}
            </p>
            <button
              className="primary-button"
              type="button"
              onClick={() => void showSettingsWindow("system")}
            >
              <Settings aria-hidden="true" size={16} />
              {menuMessages.databaseBlocked.openSettings}
            </button>
          </section>
        ) : null}
        {!databaseBlocked && query.data?.bootstrap.routes.length === 0 ? (
          <section className="menu-empty" aria-labelledby="routes-title">
            <Circle aria-hidden="true" size={20} />
            <h2 id="routes-title">{menuMessages.menuStates.noRoutesTitle}</h2>
            <p>{menuMessages.menuStates.noRoutesBody}</p>
            <button
              className="primary-button"
              type="button"
              onClick={() => void showSettingsWindow("routes", true)}
            >
              <Plus aria-hidden="true" size={16} />
              {menuMessages.menuStates.noRoutesAction}
            </button>
          </section>
        ) : null}

        {!databaseBlocked &&
        query.data &&
        query.data.bootstrap.routes.length > 0 &&
        visibleRoutes.length === 0 ? (
          <section className="menu-empty" aria-labelledby="hidden-routes-title">
            <Circle aria-hidden="true" size={20} />
            <h2 id="hidden-routes-title">
              {menuMessages.menuStates.hiddenRoutesTitle}
            </h2>
            <p>{menuMessages.menuStates.hiddenRoutesBody}</p>
            <button className="primary-button" type="button" onClick={() => void showSettingsWindow("routes")}>
              <Settings aria-hidden="true" size={16} />
              {menuMessages.menuStates.hiddenRoutesAction}
            </button>
          </section>
        ) : null}

        {!databaseBlocked &&
        query.data &&
        query.data.bootstrap.routes.length > 0 && visibleRoutes.length > 0 ? (
          <AppScrollArea
            className="menu-routes"
            viewportClassName="menu-routes-viewport"
            viewportProps={{
              role: "listbox",
              "aria-label": menuMessages.routeList.label,
            }}
          >
            {visibleRoutes.map((route) => {
              const active =
                route.routeId === query.data.bootstrap.activeRouteId;
              const balance = balances.get(route.routeId);
              const balanceEnabled = balanceEnabledRoutes.has(route.routeId);
              const refreshing =
                refreshingRoute === route.routeId ||
                balance?.status === "refreshing";
              const balanceMeta = balanceMetaLabel(
                balance,
                menuMessages.balance,
                effectiveLocale,
              );
              const fallbackBoundaryEdge =
                fallbackBoundaryOwner?.routeId === route.routeId
                  ? fallbackBoundaryOwner.edge
                  : null;
              return (
                <div
                  className={`menu-route-row${active ? " menu-route-row-active" : ""}${fallbackBoundaryEdge ? ` menu-route-row-fallback-boundary-${fallbackBoundaryEdge}` : ""}`}
                  data-route-id={route.routeId}
                  key={route.routeId}
                  role="option"
                  aria-selected={active}
                >
                  <button
                    className="route-select"
                    type="button"
                    aria-label={menuMessages.routeList.switchTo(route.name)}
                    disabled={pendingRoute !== null}
                    onClick={() => void performActivate(route.routeId)}
                  >
                    <span className="route-check" aria-hidden="true">
                      {active ? (
                        <CircleCheck size={15} strokeWidth={2.25} />
                      ) : (
                        <Circle size={15} />
                      )}
                    </span>
                    <span
                      className="route-identity"
                      onPointerEnter={() =>
                        usagePreview.enterRoute(route.routeId)
                      }
                      onPointerLeave={usagePreview.leaveRegion}
                    >
                      <strong>{route.name}</strong>
                    </span>
                    <span
                      className={`inference inference-${route.inferenceStatus.kind}`}
                    >
                      {inferenceLabel(route.inferenceStatus, menuMessages)}
                    </span>
                    <span
                      className={`balance balance-${balance?.status ?? "unavailable"}`}
                    >
                      <span>
                        {balanceLabel(
                          balance,
                          balanceEnabled,
                          menuMessages.balance,
                        )}
                      </span>
                      {balanceMeta ? <small>{balanceMeta}</small> : null}
                    </span>
                  </button>
                  <button
                    className="menu-refresh-button"
                    type="button"
                    disabled={refreshing || !balanceEnabled}
                    aria-label={menuMessages.routeList.refreshBalanceLabel(
                      route.name,
                    )}
                    title={menuMessages.routeList.refreshBalanceTitle}
                    onClick={() => void refreshOne(route.routeId)}
                  >
                    <RefreshCw
                      aria-hidden="true"
                      size={16}
                      className={refreshing ? "spin" : ""}
                    />
                  </button>
                  {fallbackBoundaryEdge ? (
                    <MenuFallbackBoundary
                      edge={fallbackBoundaryEdge}
                      enabled={fallbackBoundaryEnabled}
                      messages={menuMessages.fallbackControl}
                    />
                  ) : null}
                </div>
              );
            })}
          </AppScrollArea>
        ) : null}

        {!databaseBlocked && (error || incompleteSwitch) ? (
          <div className="menu-inline-error" role="alert">
            <AlertCircle aria-hidden="true" size={14} />
            {error
              ? ipcErrorMessage(error, messages)
              : menuMessages.routeSwitch.activationIncomplete}
          </div>
        ) : null}

        {!databaseBlocked && disconnectConfirmation ? (
          <MenuConfirmDialog
            titleId="disconnect-codex-title"
            title={menuMessages.codexDisconnect.title}
            body={menuMessages.codexDisconnect.body}
            cancelLabel={messages.shell.cancel}
            confirmLabel={menuMessages.codexDisconnect.confirm}
            destructive
            onCancel={() => setDisconnectConfirmation(false)}
            onConfirm={() => {
              setDisconnectConfirmation(false);
              void disconnectCodexFromMenu();
            }}
          />
        ) : null}

        {!databaseBlocked && activationPreview ? (
          <MenuConfirmDialog
            titleId="activate-route-title"
            title={menuMessages.routeSwitch.title(
              activationPreview.targetRouteName,
            )}
            body={
              <>
                {activationPreview.targetCatalogMode === "custom"
                  ? menuMessages.routeSwitch.customCatalog
                  : menuMessages.routeSwitch.officialCatalog}
                <br />
                {menuMessages.routeSwitch.restartHint}
              </>
            }
            cancelLabel={messages.shell.cancel}
            confirmLabel={
              pendingRoute
                ? menuMessages.routeSwitch.pending
                : menuMessages.routeSwitch.confirm
            }
            pending={pendingRoute !== null}
            onCancel={() => {
              setActivationPreview(null);
              clearError();
            }}
            onConfirm={() => void finishActivation(activationPreview)}
          />
        ) : null}

        <footer className="menu-footer">
          <button
            className="footer-command"
            type="button"
            disabled={
              databaseBlocked ||
              batchRefreshing ||
              query.isPending ||
              query.data?.balanceEnabledRouteIds.length === 0
            }
            onClick={() => void refreshAll()}
          >
            <RefreshCw
              aria-hidden="true"
              size={16}
              className={batchRefreshing ? "spin" : ""}
            />
            {databaseBlocked
              ? menuMessages.footer.proxyStopped
              : query.data
                ? batchLabel(query.data, menuMessages.balanceBatch)
                : menuMessages.balanceBatch.refreshAll}
          </button>
          <div className="menu-footer-actions">
            <button
              className="icon-button menu-settings-button"
              type="button"
              aria-label={
                applicationUpdate.data?.available
                  ? menuMessages.footer.settingsLabelWithUpdate
                  : menuMessages.footer.settingsLabel
              }
              title={menuMessages.footer.settingsTitle}
              onClick={() => void showSettingsWindow("routes")}
            >
              <Settings aria-hidden="true" size={17} />
              {applicationUpdate.data?.available ? (
                <span
                  className="application-update-indicator"
                  aria-hidden="true"
                />
              ) : null}
            </button>
            <button
              className="icon-button"
              type="button"
              aria-label={menuMessages.footer.quitLabel(
                appVariant.displayName,
              )}
              title={menuMessages.footer.quitTitle}
              onClick={() => void quitApplication()}
            >
              <Power aria-hidden="true" size={17} />
            </button>
          </div>
        </footer>
      </main>
      {usagePreview.targetRouteId !== null && usagePreviewRoute ? (
        <MenuUsagePreview
          routeName={usagePreviewRoute.name}
          phase={usagePreview.phase}
          data={usagePreview.history.data}
          pending={usagePreview.history.isPending}
          error={usagePreview.history.isError}
          onPointerEnter={usagePreview.enterPreview}
          onPointerLeave={usagePreview.leavePreview}
        />
      ) : null}
    </div>
  );
}

function formatCapacityBytes(bytes: number): string {
  const gib = 1024 ** 3;
  const mib = 1024 ** 2;
  if (bytes >= gib) return `${trimCapacityDecimal(bytes / gib, 2)} GB`;
  return `${trimCapacityDecimal(bytes / mib, 1)} MB`;
}

function trimCapacityDecimal(value: number, digits: number): string {
  return value.toFixed(digits).replace(/\.0+$|(?<=\.[0-9]*)0+$/, "");
}
