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
import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { useQueryClient } from "@tanstack/react-query";

import {
  confirmRouteActivation,
  connectCodex,
  dismissCodexRestartNotice,
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
import { AppScrollArea } from "../shared/AppScrollArea";
import type {
  BalanceDisplaySnapshot,
  BootstrapSnapshotDto,
  CodexConfigStatus,
  DatabaseStartupIssue,
  FallbackStateDto,
  InferenceFailureReason,
  InferenceStatusKind,
  MenuSnapshotDto,
  ProxyRuntimeStatus,
  RouteActivationPreviewDto,
  RouteId,
} from "../../generated";
import { useMenuPopover } from "./useMenuPopover";
import { MenuUsagePreview } from "./MenuUsagePreview";
import { useMenuUsagePreview } from "./useMenuUsagePreview";

const inferenceLabels: Record<InferenceStatusKind, string> = {
  unverified: "未验证",
  recent_success: "最近成功",
  recent_failure: "最近失败",
  expired: "状态已过期",
};

const failureLabels: Record<InferenceFailureReason, string> = {
  connection: "连接失败",
  timeout: "请求超时",
  service: "服务失败",
  rate_limit: "限流失败",
  invalid_key: "Key无效",
  insufficient_quota: "余额不足",
  billing_limit: "额度受限",
  authentication: "鉴权失败",
  access_denied: "访问拒绝",
};

const databaseIssueMenuLabels: Record<DatabaseStartupIssue, string> = {
  permission: "数据库权限异常",
  disk_full: "磁盘空间不足",
  future_schema: "数据库版本过新",
  unsafe_path: "数据库路径不安全",
  unavailable: "数据库不可用",
};

function databaseIssueLabel(snapshot: BootstrapSnapshotDto) {
  const issue = snapshot.lifecycle.issue;
  return typeof issue === "object" && issue !== null
    ? databaseIssueMenuLabels[issue.database]
    : databaseIssueMenuLabels.unavailable;
}

function inferenceLabel(
  status: MenuSnapshotDto["bootstrap"]["routes"][number]["inferenceStatus"],
) {
  if (status.kind === "recent_failure" && status.failureReason) {
    return failureLabels[status.failureReason];
  }
  return inferenceLabels[status.kind];
}

function proxyLabel(status: ProxyRuntimeStatus) {
  switch (status) {
    case "running":
      return ["代理运行中", "good"] as const;
    case "starting":
      return ["代理启动中", "warning"] as const;
    case "port_conflict":
      return ["端口冲突", "bad"] as const;
    case "database_error":
      return ["数据库不可用", "bad"] as const;
    case "shutting_down":
      return ["正在退出", "warning"] as const;
    case "stopped":
    case "error":
      return ["代理故障", "bad"] as const;
  }
}

type RuntimeStatusTone = "good" | "warning" | "bad";

type CodexControlAction =
  "connect" | "reconnect" | "disconnect" | "settings" | null;

function codexControlPresentation(
  status: CodexConfigStatus,
  hasActiveRoute: boolean,
) {
  switch (status) {
    case "connected":
      return {
        state: "Codex 已连接",
        action: "断开 Codex",
        pending: "断开中",
        tone: "good" as RuntimeStatusTone,
        activation: "disconnect" as CodexControlAction,
        unavailable: false,
      };
    case "checking":
      return {
        state: "Codex 检查中",
        action: "正在检查",
        pending: null,
        tone: "warning" as RuntimeStatusTone,
        activation: null,
        unavailable: true,
      };
    case "changed":
      return {
        state: "Codex 待重新连接",
        action: hasActiveRoute ? "重新连接" : "请先选择路由",
        pending: "连接中",
        tone: "warning" as RuntimeStatusTone,
        activation: "reconnect" as CodexControlAction,
        unavailable: !hasActiveRoute,
      };
    case "not_connected":
      return {
        state: "Codex 未连接",
        action: hasActiveRoute ? "连接 Codex" : "请先选择路由",
        pending: "连接中",
        tone: "warning" as RuntimeStatusTone,
        activation: "connect" as CodexControlAction,
        unavailable: !hasActiveRoute,
      };
    case "images_mcp_name_conflict":
    case "images_mcp_projection_conflict":
      return {
        state: "Codex 图片配置冲突",
        action: "前往处理",
        pending: null,
        tone: "bad" as RuntimeStatusTone,
        activation: "settings" as CodexControlAction,
        unavailable: false,
      };
    case "invalid":
    case "unreadable":
    case "symlink_unsupported":
      return {
        state: "Codex 配置异常",
        action: "修复配置",
        pending: null,
        tone: "bad" as RuntimeStatusTone,
        activation: "settings" as CodexControlAction,
        unavailable: false,
      };
  }
}

function fallbackControlPresentation(fallback: FallbackStateDto) {
  if (fallback.participantCount < 2) {
    return {
      state: "Fallback 不可用",
      action: "至少需 2 条路由",
      tone: "warning" as RuntimeStatusTone,
      unavailable: true,
    };
  }
  if (!fallback.enabled) {
    return {
      state: "Fallback 已关闭",
      action: "开启 Fallback",
      tone: "warning" as RuntimeStatusTone,
      unavailable: false,
    };
  }
  return {
    state: "Fallback 已开启",
    action: "关闭 Fallback",
    tone: "good" as RuntimeStatusTone,
    unavailable: false,
  };
}

function MenuFallbackBoundary({
  edge,
  enabled,
}: {
  edge: "start" | "end";
  enabled: boolean;
}) {
  return (
    <span
      className={`menu-fallback-boundary menu-fallback-boundary-${edge}${enabled ? "" : " is-disabled"}`}
      aria-hidden="true"
    >
      <span>{enabled ? "以上参与 Fallback" : "Fallback 范围 · 已关闭"}</span>
    </span>
  );
}

function balanceLabel(
  balance: BalanceDisplaySnapshot | undefined,
  enabled: boolean,
) {
  if (!enabled) return "未配置余额";
  if (!balance || balance.status === "unavailable") return "尚无余额";
  if (balance.status === "failed") return "余额查询失败";
  if (!balance.value) return "尚无余额";
  const value = balance.value.remaining;
  const unit = balance.value?.unit ?? "";
  const amount =
    value === null || value === undefined
      ? "余额可用"
      : ["$", "¥", "€", "£"].includes(unit)
        ? `${unit}${value.toFixed(2)}`
        : unit === "USD"
          ? `$${value.toFixed(2)}`
          : `${value.toFixed(2)}${unit ? ` ${unit}` : ""}`;
  return amount;
}

function formatBalanceTime(timestamp: number) {
  return new Intl.DateTimeFormat("zh-CN", {
    hour: "2-digit",
    minute: "2-digit",
  }).format(timestamp);
}

function balanceMetaLabel(balance: BalanceDisplaySnapshot | undefined) {
  if (!balance) return null;
  if (balance.status === "stale") {
    const refreshedAt = balance.lastCompletionAtMs ?? balance.lastSuccessAtMs;
    return refreshedAt
      ? `${formatBalanceTime(refreshedAt)} · 待刷新`
      : "待刷新";
  }
  if (balance.status === "last_good") {
    return balance.lastSuccessAtMs
      ? `${formatBalanceTime(balance.lastSuccessAtMs)} · 上次结果`
      : "上次结果";
  }
  const refreshedAt = balance.lastCompletionAtMs ?? balance.lastSuccessAtMs;
  return refreshedAt ? `刷新于 ${formatBalanceTime(refreshedAt)}` : null;
}

function batchLabel(snapshot: MenuSnapshotDto) {
  const batch = snapshot.balanceBatch;
  if (!batch) {
    return snapshot.balanceEnabledRouteIds.length === 0
      ? "没有可更新的余额"
      : "更新全部余额";
  }
  if (batch.phase === "running")
    return `正在更新 ${batch.completedCount}/${batch.eligibleCount}`;
  if (batch.eligibleCount === 0) return "没有可更新的余额";
  if (batch.failureCount === 0) return "全部更新成功";
  return `已更新 ${batch.successCount}/${batch.eligibleCount}，${batch.failureCount} 项失败`;
}

function MenuConfirmDialog({
  titleId,
  title,
  body,
  confirmLabel,
  pending = false,
  destructive = false,
  onCancel,
  onConfirm,
}: {
  titleId: string;
  title: string;
  body: ReactNode;
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
            取消
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
  const routeScrollerRef = useRef<HTMLDivElement>(null);
  const queryClient = useQueryClient();
  const bootstrap = useBootstrapSnapshot();
  const databaseBlocked = isDatabaseSnapshotBlocked(
    bootstrap.data?.lifecycle.phase,
  );
  const query = useMenuSnapshot(bootstrap.isSuccess && !databaseBlocked);
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
  const [refreshingRoute, setRefreshingRoute] = useState<RouteId | null>(null);
  const [refreshingAll, setRefreshingAll] = useState(false);
  const [codexOperation, setCodexOperation] = useState<
    "connect" | "disconnect" | null
  >(null);
  const [fallbackOperation, setFallbackOperation] = useState<
    "enable" | "disable" | null
  >(null);
  const [error, setError] = useState<string | null>(null);

  const balances = useMemo(
    () =>
      new Map(
        query.data?.balances.map((balance) => [balance.routeId, balance]) ?? [],
      ),
    [query.data?.balances],
  );
  const routeIds = useMemo(
    () => query.data?.bootstrap.routes.map((route) => route.routeId) ?? [],
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

  useEffect(() => {
    if (generation === 0) return;
    const scroller = routeScrollerRef.current;
    if (!scroller || !query.data?.bootstrap.activeRouteId) {
      if (scroller) scroller.scrollTop = 0;
      return;
    }
    const active = scroller.querySelector<HTMLElement>(
      `[data-route-id="${CSS.escape(query.data.bootstrap.activeRouteId)}"]`,
    );
    if (!active) return;
    const row = active.getBoundingClientRect();
    const viewport = scroller.getBoundingClientRect();
    if (row.top < viewport.top || row.bottom > viewport.bottom) {
      active.scrollIntoView({ block: "nearest", behavior: "instant" });
    }
  }, [generation, query.data]);

  const refreshMenu = async () => {
    await queryClient.invalidateQueries({ queryKey: queryKeys.menu });
  };

  const finishActivation = async (preview: RouteActivationPreviewDto) => {
    setPendingRoute(preview.targetRouteId);
    setError(null);
    try {
      const result = await confirmRouteActivation(preview.permit);
      setActivationPreview(null);
      await refreshMenu();
      if (!result.catalog.projectionApplied) {
        setError(
          "中转已切换，但 Codex 模型列表尚未完整应用。请重新连接 Codex 后重试。",
        );
        return;
      }
      await hideMenu();
    } catch (reason) {
      const normalized = normalizeIpcError(reason);
      setError(normalized.message);
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
    setError(null);
    try {
      const preview = await previewRouteActivation(routeId);
      if (preview.confirmationRequired) {
        setActivationPreview(preview);
      } else {
        await finishActivation(preview);
      }
    } catch (reason) {
      setError(normalizeIpcError(reason).message);
    } finally {
      setPendingRoute(null);
    }
  };

  const dismissRestartNotice = async (noticeId: string) => {
    setDismissedNoticeId(noticeId);
    setDismissingNoticeId(noticeId);
    setError(null);
    try {
      await dismissCodexRestartNotice(noticeId);
      await refreshMenu();
    } catch (reason) {
      setDismissedNoticeId((current) =>
        current === noticeId ? null : current,
      );
      setError(normalizeIpcError(reason).message);
    } finally {
      setDismissingNoticeId((current) =>
        current === noticeId ? null : current,
      );
    }
  };

  const refreshOne = async (routeId: RouteId) => {
    setRefreshingRoute(routeId);
    setError(null);
    try {
      await refreshBalance(routeId);
      await refreshMenu();
    } catch (reason) {
      setError(normalizeIpcError(reason).message);
    } finally {
      setRefreshingRoute(null);
    }
  };

  const refreshAll = async () => {
    setRefreshingAll(true);
    setError(null);
    try {
      await refreshAllBalances();
      await refreshMenu();
    } catch (reason) {
      setError(normalizeIpcError(reason).message);
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
    setError(null);
    try {
      if (status === "changed") await reconnectCodex();
      else await connectCodex(false);
      await refreshMenu();
    } catch (reason) {
      setError(normalizeIpcError(reason).message);
    } finally {
      setCodexOperation(null);
    }
  };

  const disconnectCodexFromMenu = async () => {
    if (codexOperation !== null) return;
    setCodexOperation("disconnect");
    setError(null);
    try {
      await restoreCodex();
      await refreshMenu();
    } catch (reason) {
      setError(normalizeIpcError(reason).message);
    } finally {
      setCodexOperation(null);
    }
  };

  const changeFallbackFromMenu = async () => {
    const fallback = query.data?.bootstrap.fallback;
    if (
      !fallback ||
      fallbackOperation !== null ||
      fallback.participantCount < 2
    ) {
      return;
    }
    const nextEnabled = !fallback.enabled;
    setFallbackOperation(nextEnabled ? "enable" : "disable");
    setError(null);
    try {
      await setFallbackEnabled(nextEnabled);
      await refreshMenu();
    } catch (reason) {
      setError(normalizeIpcError(reason).message);
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
      )
    : null;
  const fallbackControl = query.data
    ? fallbackControlPresentation(query.data.bootstrap.fallback)
    : null;
  const fallbackBoundaryIndex = query.data
    ? Math.min(
        Math.max(query.data.bootstrap.fallback.participantCount, 0),
        query.data.bootstrap.routes.length,
      )
    : null;
  const fallbackBoundaryEnabled = query.data
    ? query.data.bootstrap.fallback.enabled &&
      query.data.bootstrap.fallback.participantCount >= 2
    : false;
  const codexPendingLabel =
    codexOperation === "disconnect"
      ? "断开中"
      : codexOperation === "connect"
        ? "连接中"
        : null;
  const codexReservedPendingLabel = codexPendingLabel ?? codexControl?.pending;
  const fallbackPendingLabel =
    fallbackOperation === "enable" ? "开启中" : "关闭中";
  const restartNotice =
    query.data?.codexRestartNotice?.noticeId === dismissedNoticeId
      ? null
      : query.data?.codexRestartNotice;
  const confirming = disconnectConfirmation || activationPreview !== null;

  return (
    <div
      className={`menu-window-layout menu-window-layout-${previewLayout.side}`}
    >
      <main
        className={`menu-shell${confirming ? " menu-shell-confirming" : ""}`}
        aria-label={`${appVariant.displayName} 菜单`}
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
            <div className="menu-runtime-status" aria-label="运行状态">
              <span
                className={`runtime-status runtime-status-${proxyLabel(query.data.bootstrap.proxyStatus)[1]}`}
              >
                <i aria-hidden="true" />
                {proxyLabel(query.data.bootstrap.proxyStatus)[0]}
              </span>
              <button
                className={`runtime-status runtime-control runtime-status-${codexControl.tone}${codexOperation ? " is-pending" : ""}`}
                type="button"
                disabled={codexOperation !== null}
                aria-disabled={codexControl.unavailable || undefined}
                aria-label={`${codexControl.state}，${codexPendingLabel ?? codexControl.action}`}
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
                aria-label={`${fallbackControl.state}，${fallbackOperation ? fallbackPendingLabel : fallbackControl.action}${query.data.bootstrap.fallback.enabled && !query.data.bootstrap.fallback.hasNext ? "；当前路由之后没有可用的 Fallback 路由" : ""}`}
                title={
                  query.data.bootstrap.fallback.enabled &&
                  !query.data.bootstrap.fallback.hasNext
                    ? "当前路由之后没有可用的 Fallback 路由"
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
              className={`runtime-status runtime-status-${proxyLabel(bootstrap.data.proxyStatus)[1]}`}
            >
              <i aria-hidden="true" />
              {proxyLabel(bootstrap.data.proxyStatus)[0]}
            </span>
          ) : null}
        </header>

        {!databaseBlocked && restartNotice ? (
          <section
            className="menu-restart-notice"
            aria-label="Codex 模型列表更新提醒"
          >
            <TriangleAlert aria-hidden="true" size={17} />
            <p>
              <strong>已自动切换至 {restartNotice.routeName}</strong>
              <span>重启 Codex 后更新模型列表</span>
            </p>
            <button
              type="button"
              aria-label="关闭 Codex 模型列表更新提醒"
              title="关闭提醒"
              disabled={dismissingNoticeId === restartNotice.noticeId}
              onClick={() => void dismissRestartNotice(restartNotice.noticeId)}
            >
              <X aria-hidden="true" size={15} />
            </button>
          </section>
        ) : null}

        {!databaseBlocked && query.isPending ? (
          <div className="menu-message">正在读取状态...</div>
        ) : null}
        {!databaseBlocked && query.isError ? (
          <div className="menu-message menu-message-error">
            {bootstrap.data?.proxyStatus === "database_error"
              ? "数据库不可用，代理未启动。"
              : bootstrap.data?.proxyStatus === "port_conflict"
                ? "本地代理端口已被占用。"
                : "状态读取失败"}
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
                ? "需要恢复数据库"
                : databaseIssueLabel(bootstrap.data)}
            </h2>
            <p>
              {bootstrap.data.lifecycle.phase === "recovery_required"
                ? "代理已停止。请在设置中选择恢复点或创建空数据库。"
                : "代理已停止。请在设置中查看数据库启动状态。"}
            </p>
            <button
              className="primary-button"
              type="button"
              onClick={() => void showSettingsWindow("system")}
            >
              <Settings aria-hidden="true" size={16} />
              打开恢复设置
            </button>
          </section>
        ) : null}
        {!databaseBlocked && query.data?.bootstrap.routes.length === 0 ? (
          <section className="menu-empty" aria-labelledby="routes-title">
            <Circle aria-hidden="true" size={20} />
            <h2 id="routes-title">还没有路由</h2>
            <p>添加一条自定义 Responses 路由。</p>
            <button
              className="primary-button"
              type="button"
              onClick={() => void showSettingsWindow("routes", true)}
            >
              <Plus aria-hidden="true" size={16} />
              添加自定义 Responses 路由
            </button>
          </section>
        ) : null}

        {!databaseBlocked &&
        query.data &&
        query.data.bootstrap.routes.length > 0 ? (
          <AppScrollArea
            className="menu-routes"
            viewportClassName="menu-routes-viewport"
            viewportRef={routeScrollerRef}
            viewportProps={{ role: "listbox", "aria-label": "路由" }}
          >
            {query.data.bootstrap.routes.map((route, index) => {
              const active =
                route.routeId === query.data.bootstrap.activeRouteId;
              const balance = balances.get(route.routeId);
              const balanceEnabled = balanceEnabledRoutes.has(route.routeId);
              const refreshing =
                refreshingRoute === route.routeId ||
                balance?.status === "refreshing";
              const balanceMeta = balanceMetaLabel(balance);
              const fallbackBoundaryEdge =
                fallbackBoundaryIndex === 0 && index === 0
                  ? "start"
                  : fallbackBoundaryIndex === index + 1
                    ? "end"
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
                    aria-label={`切换到 ${route.name}`}
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
                      {inferenceLabel(route.inferenceStatus)}
                    </span>
                    <span
                      className={`balance balance-${balance?.status ?? "unavailable"}`}
                    >
                      <span>{balanceLabel(balance, balanceEnabled)}</span>
                      {balanceMeta ? <small>{balanceMeta}</small> : null}
                    </span>
                  </button>
                  <button
                    className="menu-refresh-button"
                    type="button"
                    disabled={refreshing || !balanceEnabled}
                    aria-label={`刷新 ${route.name} 的余额`}
                    title="刷新余额"
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
                    />
                  ) : null}
                </div>
              );
            })}
          </AppScrollArea>
        ) : null}

        {!databaseBlocked && error ? (
          <div className="menu-inline-error" role="alert">
            <AlertCircle aria-hidden="true" size={14} />
            {error}
          </div>
        ) : null}

        {!databaseBlocked && disconnectConfirmation ? (
          <MenuConfirmDialog
            titleId="disconnect-codex-title"
            title="断开 Codex？"
            body="当前 config.toml 将被断开恢复配置替换；更新恢复配置后保留的修改不会丢失。"
            confirmLabel="断开连接"
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
            title={`切换到“${activationPreview.targetRouteName}”？`}
            body={
              <>
                {activationPreview.targetCatalogMode === "custom"
                  ? "该中转使用自定义模型。"
                  : "该中转使用 Codex 官方模型。"}
                <br />
                切换后需要重启 Codex，模型列表才会更新。
              </>
            }
            confirmLabel={pendingRoute ? "切换中" : "切换中转"}
            pending={pendingRoute !== null}
            onCancel={() => {
              setActivationPreview(null);
              setError(null);
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
              ? "代理已停止"
              : query.data
                ? batchLabel(query.data)
                : "更新全部余额"}
          </button>
          <div className="menu-footer-actions">
            <button
              className="icon-button menu-settings-button"
              type="button"
              aria-label={
                applicationUpdate.data?.available
                  ? "打开设置，有可用更新"
                  : "打开设置"
              }
              title="设置"
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
              aria-label={`退出 ${appVariant.displayName}`}
              title="退出"
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
