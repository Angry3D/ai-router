import {
  Box,
  ChartNoAxesColumn,
  Route,
  Settings as SettingsIcon,
} from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";

import { getRunningAppVersion } from "../../api/appVersion";
import {
  hideSettingsWindow,
  listenSettingsCloseRequested,
  listenSettingsNavigation,
} from "../../api/ipc";
import {
  isDatabaseSnapshotBlocked,
  useBootstrapSnapshot,
  useRecoverySnapshot,
  useSettingsSnapshot,
} from "../../api/query";
import { appVariant } from "../../appVariant";
import type { RouteId } from "../../generated";
import { CodexSettings } from "./CodexSettings";
import {
  DatabaseStartupErrorPage,
  RecoveryLoadErrorPage,
  RecoveryRequiredPage,
} from "./DatabaseRecoverySettings";
import { RoutesSettings } from "./RoutesSettings";
import {
  SettingsConfirmDialog,
  SettingsSidebar,
  type SettingsSectionId,
} from "./SettingsPrimitives";
import { SystemSettings } from "./SystemSettings";
import { UsageSettings } from "./UsageSettings";

export function SettingsWindow() {
  const bootstrap = useBootstrapSnapshot();
  const databaseBlocked = isDatabaseSnapshotBlocked(
    bootstrap.data?.lifecycle.phase,
  );
  const bootstrapReady = bootstrap.isSuccess;
  const settings = useSettingsSnapshot(bootstrapReady && !databaseBlocked);
  const recovery = useRecoverySnapshot(bootstrapReady && databaseBlocked);
  const [section, setSection] = useState<SettingsSectionId>(() => {
    if (!import.meta.env.DEV) return "routes";
    const requested = new URLSearchParams(window.location.search).get(
      "section",
    );
    return requested === "usage" ||
      requested === "codex" ||
      requested === "system"
      ? requested
      : "routes";
  });
  const [selectedRouteId, setSelectedRouteId] = useState<RouteId | null>(null);
  const [newRoute, setNewRoute] = useState(false);
  const previousSelection = useRef<RouteId | null>(null);
  const [editorKey, setEditorKey] = useState(0);
  const [dirty, setDirty] = useState(false);
  const [pendingAction, setPendingAction] = useState<(() => void) | null>(null);
  const [appVersion, setAppVersion] = useState<string | null>(null);

  const routes = settings.data?.routes ?? [];
  const effectiveSelection = newRoute
    ? null
    : (selectedRouteId ?? routes[0]?.routeId ?? null);

  useEffect(() => {
    let disposed = false;
    void getRunningAppVersion()
      .then((version) => {
        if (!disposed) setAppVersion(version);
      })
      .catch(() => {});
    return () => {
      disposed = true;
    };
  }, []);

  const runOrConfirmDiscard = useCallback(
    (action: () => void) => {
      if (dirty) setPendingAction(() => action);
      else action();
    },
    [dirty],
  );

  const selectSection = (next: SettingsSectionId) => {
    runOrConfirmDiscard(() => {
      setSection(next);
      setNewRoute(false);
      setDirty(false);
      setEditorKey((value) => value + 1);
    });
  };

  const beginNewRoute = () => {
    runOrConfirmDiscard(() => {
      previousSelection.current = effectiveSelection;
      setSelectedRouteId(null);
      setNewRoute(true);
      setDirty(false);
      setEditorKey((value) => value + 1);
    });
  };

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void Promise.all([
      listenSettingsNavigation((event) => {
        if (disposed) return;
        const navigate = () => {
          setSection(event.section);
          if (event.createNewRoute) {
            previousSelection.current = effectiveSelection;
            setSelectedRouteId(null);
            setNewRoute(true);
          }
          setDirty(false);
          setEditorKey((value) => value + 1);
        };
        runOrConfirmDiscard(navigate);
      }),
      listenSettingsCloseRequested(() => {
        if (!disposed) runOrConfirmDiscard(() => void hideSettingsWindow());
      }),
    ])
      .then((dispose) => {
        if (disposed) dispose.forEach((listener) => listener());
        else unlisten = () => dispose.forEach((listener) => listener());
      })
      .catch(() => undefined);
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [effectiveSelection, runOrConfirmDiscard]);

  const selectRoute = (routeId: RouteId) => {
    runOrConfirmDiscard(() => {
      setSelectedRouteId(routeId);
      setNewRoute(false);
      setDirty(false);
      setEditorKey((value) => value + 1);
    });
  };

  const cancelEditor = () => {
    runOrConfirmDiscard(() => {
      if (newRoute) setSelectedRouteId(previousSelection.current);
      setNewRoute(false);
      setDirty(false);
      setEditorKey((value) => value + 1);
    });
  };

  const afterSave = (routeId: RouteId) => {
    const created = newRoute;
    setSelectedRouteId(routeId);
    setNewRoute(false);
    setDirty(false);
    if (created) setEditorKey((value) => value + 1);
  };

  const afterDelete = (routeId: RouteId) => {
    const index = routes.findIndex((route) => route.routeId === routeId);
    const next = routes[index + 1] ?? routes[index - 1];
    setSelectedRouteId(next?.routeId ?? null);
    setNewRoute(false);
    setDirty(false);
    setEditorKey((value) => value + 1);
  };

  return (
    <main
      className="settings-shell"
      aria-label={`${appVariant.displayName} 设置`}
    >
      <SettingsSidebar
        activeSection={section}
        onSelect={selectSection}
        version={appVersion}
        items={[
          {
            id: "routes",
            label: "路由",
            icon: <Route aria-hidden="true" size={17} />,
          },
          {
            id: "usage",
            label: "用量",
            icon: <ChartNoAxesColumn aria-hidden="true" size={17} />,
          },
          {
            id: "codex",
            label: "Codex",
            icon: <Box aria-hidden="true" size={17} />,
          },
          {
            id: "system",
            label: "系统",
            icon: <SettingsIcon aria-hidden="true" size={17} />,
          },
        ]}
      />

      {!databaseBlocked && settings.isPending ? (
        <div className="settings-loading">正在读取设置...</div>
      ) : null}
      {!databaseBlocked && settings.isError ? (
        <div className="settings-loading settings-error">
          数据库设置读取失败。
        </div>
      ) : null}

      {!databaseBlocked && settings.data && section === "routes" ? (
        <RoutesSettings
          snapshot={settings.data}
          selectedRouteId={effectiveSelection}
          newRoute={newRoute}
          editorRevision={editorKey}
          onBeginNewRoute={beginNewRoute}
          onSelectRoute={selectRoute}
          onCancelEditor={cancelEditor}
          onSaved={afterSave}
          onDeleted={afterDelete}
          onDirtyChange={setDirty}
        />
      ) : null}

      {!databaseBlocked && settings.data && section === "codex" ? (
        <CodexSettings
          key={`codex-${editorKey}`}
          snapshot={settings.data}
          proxyStatus={bootstrap.data?.proxyStatus ?? "starting"}
        />
      ) : null}
      {!databaseBlocked && settings.data && section === "usage" ? (
        <UsageSettings />
      ) : null}
      {!databaseBlocked && settings.data && section === "system" ? (
        <SystemSettings
          key={`${settings.data.balanceQuery.menuDebounceSeconds}-${settings.data.balanceQuery.automaticRefreshMinutes}`}
          snapshot={settings.data}
        />
      ) : null}

      {databaseBlocked &&
      bootstrap.data?.lifecycle.phase === "recovery_required" ? (
        recovery.data ? (
          <RecoveryRequiredPage
            snapshot={recovery.data}
            onRetry={() => recovery.refetch()}
          />
        ) : recovery.isError ? (
          <RecoveryLoadErrorPage onRetry={() => recovery.refetch()} />
        ) : (
          <div className="settings-loading">正在读取恢复点...</div>
        )
      ) : null}

      {databaseBlocked &&
      bootstrap.data?.lifecycle.phase === "database_error" ? (
        <DatabaseStartupErrorPage
          issue={recovery.data?.startupIssue ?? null}
          lifecycleIssue={bootstrap.data.lifecycle.issue}
        />
      ) : null}

      {!databaseBlocked && pendingAction ? (
        <SettingsConfirmDialog
          confirmation={{
            title: "放弃未保存的修改？",
            body: "当前设置的修改尚未保存。",
            confirmLabel: "放弃修改",
            cancelLabel: "继续编辑",
            destructive: true,
            onConfirm: () => {
              const action = pendingAction;
              setPendingAction(null);
              setDirty(false);
              setEditorKey((value) => value + 1);
              action();
            },
          }}
          onCancel={() => setPendingAction(null)}
        />
      ) : null}
    </main>
  );
}
