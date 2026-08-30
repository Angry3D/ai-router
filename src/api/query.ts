import { QueryClient, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef } from "react";

import type {
  AppLifecyclePhase,
  StateArea,
  UsageHistoryQueryDto,
  UsageStatisticsQueryDto,
} from "../generated";
import {
  getBootstrapSnapshot,
  getApplicationUpdateSnapshot,
  getMenuSnapshot,
  getRecoverySnapshot,
  getSettingsSnapshot,
  getUsageHistory,
  getUsageRequestDetail,
  getUsageRouteOptions,
  getUsageStatistics,
  isTauriRuntime,
  listenStateChanged,
} from "./ipc";

export const queryKeys = {
  bootstrap: ["bootstrap"] as const,
  menu: ["menu"] as const,
  settings: ["settings"] as const,
  routes: ["routes"] as const,
  route: ["route"] as const,
  balance: ["balance"] as const,
  balanceSettings: ["balance-settings"] as const,
  proxy: ["proxy"] as const,
  codexConnection: ["codex-connection"] as const,
  requestHistorySummary: ["request-history-summary"] as const,
  usageHistory: ["usage-history"] as const,
  usageHistoryPage: (query: UsageHistoryQueryDto) =>
    ["usage-history", query] as const,
  usageStatistics: ["usage-statistics"] as const,
  usageStatisticsResult: (query: UsageStatisticsQueryDto) =>
    ["usage-statistics", query] as const,
  usageRouteOptions: ["usage-route-options"] as const,
  usageRequestDetail: ["usage-request-detail"] as const,
  usageRequestDetailById: (requestId: string | null) =>
    ["usage-request-detail", requestId] as const,
  runtimeLogs: ["runtime-logs"] as const,
  recovery: ["recovery"] as const,
  applicationUpdate: ["application-update"] as const,
};

const keysByArea: Record<StateArea, ReadonlyArray<readonly unknown[]>> = {
  routes: [
    queryKeys.routes,
    queryKeys.bootstrap,
    queryKeys.menu,
    queryKeys.settings,
  ],
  route: [queryKeys.route, queryKeys.menu, queryKeys.settings],
  fallback: [
    queryKeys.routes,
    queryKeys.bootstrap,
    queryKeys.menu,
    queryKeys.settings,
  ],
  balance: [queryKeys.balance, queryKeys.menu],
  balance_settings: [queryKeys.balanceSettings, queryKeys.settings],
  images_generation: [queryKeys.routes, queryKeys.route, queryKeys.settings],
  mcp_image_assets: [queryKeys.menu, queryKeys.settings],
  proxy: [
    queryKeys.proxy,
    queryKeys.bootstrap,
    queryKeys.menu,
    queryKeys.settings,
  ],
  codex_connection: [
    queryKeys.codexConnection,
    queryKeys.bootstrap,
    queryKeys.menu,
    queryKeys.settings,
  ],
  codex_catalog: [queryKeys.route, queryKeys.menu, queryKeys.settings],
  codex_restart_notice: [queryKeys.menu],
  request_history_summary: [
    queryKeys.requestHistorySummary,
    queryKeys.settings,
    queryKeys.usageHistory,
    queryKeys.usageStatistics,
    queryKeys.usageRouteOptions,
    queryKeys.usageRequestDetail,
  ],
  runtime_logs: [queryKeys.runtimeLogs, queryKeys.settings],
  recovery: [
    queryKeys.recovery,
    queryKeys.bootstrap,
    queryKeys.menu,
    queryKeys.settings,
  ],
  appearance: [queryKeys.bootstrap],
  menu_bar: [queryKeys.settings],
  application_update: [queryKeys.applicationUpdate],
};

export function isDatabaseSnapshotBlocked(
  phase: AppLifecyclePhase | undefined,
) {
  return phase === "recovery_required" || phase === "database_error";
}

export function createRouterQueryClient(): QueryClient {
  return new QueryClient({
    defaultOptions: {
      queries: {
        retry: false,
        staleTime: 10_000,
      },
    },
  });
}

export function useBootstrapSnapshot() {
  return useQuery({
    queryKey: queryKeys.bootstrap,
    queryFn: getBootstrapSnapshot,
    enabled: isTauriRuntime(),
  });
}

export function useMenuSnapshot(enabled = true) {
  return useQuery({
    queryKey: queryKeys.menu,
    queryFn: getMenuSnapshot,
    enabled: isTauriRuntime() && enabled,
  });
}

export function useSettingsSnapshot(enabled = true) {
  return useQuery({
    queryKey: queryKeys.settings,
    queryFn: getSettingsSnapshot,
    enabled: isTauriRuntime() && enabled,
  });
}

export function useApplicationUpdateSnapshot(enabled = true) {
  return useQuery({
    queryKey: queryKeys.applicationUpdate,
    queryFn: getApplicationUpdateSnapshot,
    enabled: isTauriRuntime() && enabled,
  });
}

export function useRecoverySnapshot(enabled = true) {
  return useQuery({
    queryKey: queryKeys.recovery,
    queryFn: getRecoverySnapshot,
    enabled: isTauriRuntime() && enabled,
  });
}

export function useUsageHistory(query: UsageHistoryQueryDto, enabled = true) {
  return useQuery({
    queryKey: queryKeys.usageHistoryPage(query),
    queryFn: () => getUsageHistory(query),
    enabled: (isTauriRuntime() || import.meta.env.DEV) && enabled,
  });
}

export function useUsageStatistics(
  query: UsageStatisticsQueryDto,
  enabled = true,
) {
  return useQuery({
    queryKey: queryKeys.usageStatisticsResult(query),
    queryFn: () => getUsageStatistics(query),
    enabled: (isTauriRuntime() || import.meta.env.DEV) && enabled,
  });
}

export function useUsageRouteOptions(enabled = true) {
  return useQuery({
    queryKey: queryKeys.usageRouteOptions,
    queryFn: getUsageRouteOptions,
    enabled: (isTauriRuntime() || import.meta.env.DEV) && enabled,
  });
}

export function useUsageRequestDetail(
  requestId: string | null,
  enabled = true,
) {
  return useQuery({
    queryKey: queryKeys.usageRequestDetailById(requestId),
    queryFn: () => getUsageRequestDetail(requestId ?? ""),
    enabled:
      (isTauriRuntime() || import.meta.env.DEV) &&
      enabled &&
      requestId !== null,
  });
}

export function useRouterStateSync(view: "menu" | "settings") {
  const queryClient = useQueryClient();
  const latestRevision = useRef(0);

  useEffect(() => {
    if (!isTauriRuntime()) return undefined;
    let disposed = false;
    let unlisten: (() => void) | undefined;

    void listenStateChanged((event) => {
      if (disposed || event.revision <= latestRevision.current) return;
      latestRevision.current = event.revision;
      const uniqueKeys = new Map<string, readonly unknown[]>();
      for (const area of event.areas) {
        for (const key of keysByArea[area])
          uniqueKeys.set(JSON.stringify(key), key);
      }
      for (const key of uniqueKeys.values()) {
        void queryClient.invalidateQueries({ queryKey: key });
      }
    })
      .then((dispose) => {
        if (disposed) dispose();
        else unlisten = dispose;
      })
      .catch(() => undefined);

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [queryClient]);

  useEffect(() => {
    const refresh = () => {
      if (!isTauriRuntime()) return;
      const keys =
        view === "menu"
          ? [queryKeys.bootstrap, queryKeys.applicationUpdate]
          : [
              queryKeys.bootstrap,
              queryKeys.settings,
              queryKeys.recovery,
              queryKeys.applicationUpdate,
              queryKeys.usageHistory,
              queryKeys.usageStatistics,
              queryKeys.usageRouteOptions,
              queryKeys.usageRequestDetail,
            ];
      for (const queryKey of keys) {
        void queryClient.invalidateQueries({ queryKey, refetchType: "active" });
      }
    };
    const onVisibility = () => {
      if (view === "menu" && document.visibilityState === "visible") refresh();
    };
    window.addEventListener("focus", refresh);
    document.addEventListener("visibilitychange", onVisibility);
    return () => {
      window.removeEventListener("focus", refresh);
      document.removeEventListener("visibilitychange", onVisibility);
    };
  }, [queryClient, view]);
}
