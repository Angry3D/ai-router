import { ArrowDown, ArrowUp, CircleAlert, Plus, Route } from "lucide-react";
import { useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";

import {
  moveRoute,
  normalizeIpcError,
  setFallbackEnabled,
  setFallbackParticipantCount,
} from "../../api/ipc";
import { queryKeys } from "../../api/query";
import type { RouteId, SettingsSnapshotDto } from "../../generated";
import { AppScrollArea } from "../shared/AppScrollArea";
import { FallbackBoundary, FallbackBoundaryPreview } from "./FallbackBoundary";
import { RouteEditor } from "./RouteEditor";
import {
  SettingsButton,
  SettingsHelpTooltip,
  SettingsIconButton,
  SettingsPageTitle,
  SettingsSwitch,
} from "./SettingsPrimitives";

export interface RoutesSettingsProps {
  snapshot: SettingsSnapshotDto;
  selectedRouteId: RouteId | null;
  newRoute: boolean;
  editorRevision: number;
  onBeginNewRoute: () => void;
  onSelectRoute: (routeId: RouteId) => void;
  onCancelEditor: () => void;
  onSaved: (routeId: RouteId) => void;
  onDeleted: (routeId: RouteId) => void;
  onDirtyChange: (dirty: boolean) => void;
}

export function RoutesSettings({
  snapshot,
  selectedRouteId,
  newRoute,
  editorRevision,
  onBeginNewRoute,
  onSelectRoute,
  onCancelEditor,
  onSaved,
  onDeleted,
  onDirtyChange,
}: RoutesSettingsProps) {
  const queryClient = useQueryClient();
  const [routeToolsBusy, setRouteToolsBusy] = useState(false);
  const [routeToolsError, setRouteToolsError] = useState<string | null>(null);
  const [fallbackBoundaryPending, setFallbackBoundaryPending] = useState(false);
  const [fallbackPreviewCount, setFallbackPreviewCount] = useState<
    number | null
  >(null);
  const [fallbackDragging, setFallbackDragging] = useState(false);
  const routeListViewportRef = useRef<HTMLDivElement | null>(null);
  const routes = snapshot.routes;
  const confirmedFallbackCount = snapshot.fallback.participantCount;
  const displayedFallbackCount = fallbackPreviewCount ?? confirmedFallbackCount;
  const selectedRouteIndex = routes.findIndex(
    (route) => route.routeId === selectedRouteId,
  );

  const refreshRouteState = async () => {
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: queryKeys.settings }),
      queryClient.invalidateQueries({ queryKey: queryKeys.bootstrap }),
    ]);
  };

  const moveSelectedRoute = async (direction: "up" | "down") => {
    if (!selectedRouteId) return;
    setRouteToolsBusy(true);
    setRouteToolsError(null);
    try {
      await moveRoute(selectedRouteId, direction);
      await refreshRouteState();
    } catch (reason) {
      setRouteToolsError(normalizeIpcError(reason).message);
    } finally {
      setRouteToolsBusy(false);
    }
  };

  const changeFallback = async (enabled: boolean) => {
    setRouteToolsBusy(true);
    setRouteToolsError(null);
    try {
      await setFallbackEnabled(enabled);
      await refreshRouteState();
    } catch (reason) {
      setRouteToolsError(normalizeIpcError(reason).message);
    } finally {
      setRouteToolsBusy(false);
    }
  };

  const changeFallbackParticipantCount = async (participantCount: number) => {
    if (participantCount === confirmedFallbackCount) {
      setFallbackPreviewCount(null);
      return;
    }
    setRouteToolsBusy(true);
    setFallbackBoundaryPending(true);
    setRouteToolsError(null);
    try {
      await setFallbackParticipantCount(participantCount);
      await refreshRouteState();
      setFallbackPreviewCount(null);
    } catch (reason) {
      setFallbackPreviewCount(null);
      setRouteToolsError(normalizeIpcError(reason).message);
    } finally {
      setFallbackBoundaryPending(false);
      setRouteToolsBusy(false);
    }
  };

  const routeToolsLocked = routeToolsBusy || fallbackDragging;
  const interactiveBoundaryCount = fallbackDragging
    ? confirmedFallbackCount
    : displayedFallbackCount;
  const dragPreviewCount = fallbackDragging ? displayedFallbackCount : null;
  const routeListItems = [];
  for (let index = 0; index <= routes.length; index += 1) {
    if (index === interactiveBoundaryCount) {
      routeListItems.push(
        <div
          className="fallback-boundary-slot"
          key="fallback-boundary-slot"
          style={{ order: index * 2 }}
        >
          <FallbackBoundary
            value={displayedFallbackCount}
            confirmedValue={confirmedFallbackCount}
            routeCount={routes.length}
            viewportRef={routeListViewportRef}
            disabled={routeToolsBusy}
            pending={fallbackBoundaryPending}
            onPreview={setFallbackPreviewCount}
            onCancel={() => setFallbackPreviewCount(null)}
            onCommit={(value) => void changeFallbackParticipantCount(value)}
            onDraggingChange={setFallbackDragging}
          />
        </div>,
      );
    }
    const route = routes[index];
    if (!route) continue;
    routeListItems.push(
      <button
        key={route.routeId}
        type="button"
        data-fallback-route-index={index}
        className={`settings-route-row${selectedRouteId === route.routeId && !newRoute ? " selected" : ""}`}
        style={{ order: index * 2 + 1 }}
        onClick={() => onSelectRoute(route.routeId)}
      >
        <span className="settings-route-identity">
          <strong>{route.name}</strong>
          <small>{route.baseUrlHost}</small>
        </span>
        <span className="settings-route-markers">
          {index < displayedFallbackCount ? <b>Fallback {index + 1}</b> : null}
          {route.routeId === snapshot.activeRouteId ? <em>当前</em> : null}
        </span>
      </button>,
    );
  }
  if (dragPreviewCount !== null) {
    routeListItems.push(
      <div
        className="fallback-boundary-slot"
        key="fallback-preview-slot"
        style={{ order: dragPreviewCount * 2 }}
      >
        <FallbackBoundaryPreview />
      </div>,
    );
  }

  return (
    <div className="routes-settings">
      <section className="route-list-pane" aria-label="路由列表">
        <header>
          <div
            className="route-list-top-drag-region"
            data-tauri-drag-region
            aria-hidden="true"
          />
          <SettingsPageTitle title="路由" />
          <SettingsIconButton
            type="button"
            label="新建路由"
            disabled={routeToolsLocked}
            onClick={onBeginNewRoute}
          >
            <Plus aria-hidden="true" size={18} />
          </SettingsIconButton>
        </header>
        <AppScrollArea
          className="settings-route-list"
          viewportClassName="settings-route-list-viewport"
          viewportRef={routeListViewportRef}
        >
          {routes.length === 0 ? (
            <div className="route-list-empty">
              <Route aria-hidden="true" size={22} />
              <strong>还没有路由</strong>
              <SettingsButton
                type="button"
                disabled={routeToolsLocked}
                onClick={onBeginNewRoute}
              >
                添加路由
              </SettingsButton>
            </div>
          ) : null}
          {routeListItems}
        </AppScrollArea>
        <div className="route-tools" aria-label="路由排序与自动 Fallback">
          <div className="route-order-tools">
            <SettingsIconButton
              type="button"
              label="上移所选路由"
              disabled={routeToolsLocked || selectedRouteIndex <= 0}
              onClick={() => void moveSelectedRoute("up")}
            >
              <ArrowUp aria-hidden="true" size={16} />
            </SettingsIconButton>
            <SettingsIconButton
              type="button"
              label="下移所选路由"
              disabled={
                routeToolsLocked ||
                selectedRouteIndex < 0 ||
                selectedRouteIndex >= routes.length - 1
              }
              onClick={() => void moveSelectedRoute("down")}
            >
              <ArrowDown aria-hidden="true" size={16} />
            </SettingsIconButton>
          </div>
          <span className="route-fallback-switch-help">
            <SettingsSwitch
              label="自动 Fallback"
              checked={snapshot.fallback.enabled}
              disabled={routeToolsLocked || confirmedFallbackCount < 2}
              onChange={(event) =>
                void changeFallback(event.currentTarget.checked)
              }
            />
            <SettingsHelpTooltip label="说明自动 Fallback 切换规则">
              请求失败且符合切换条件时，将按顺序尝试后续路由；到最后一条后停止，不会回到前面的路由。
            </SettingsHelpTooltip>
          </span>
          <span className="route-tools-error-slot">
            {routeToolsError ? (
              <span
                className="route-tools-error"
                role="alert"
                aria-label={`路由设置失败：${routeToolsError}`}
                title={routeToolsError}
              >
                <CircleAlert aria-hidden="true" size={13} />
              </span>
            ) : null}
          </span>
        </div>
      </section>
      <RouteEditor
        key={`${newRoute ? "new" : (selectedRouteId ?? "empty")}-${editorRevision}`}
        routeId={selectedRouteId}
        newRoute={newRoute}
        activeRouteId={snapshot.activeRouteId}
        riskConfirmed={snapshot.balanceScriptRiskConfirmed}
        externalBusy={routeToolsLocked}
        onDirtyChange={onDirtyChange}
        onCancel={onCancelEditor}
        onSaved={onSaved}
        onDeleted={onDeleted}
      />
    </div>
  );
}
