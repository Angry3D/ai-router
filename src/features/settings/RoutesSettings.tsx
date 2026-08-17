import {
  closestCenter,
  DndContext,
  DragOverlay,
  KeyboardSensor,
  MeasuringStrategy,
  PointerSensor,
  useSensor,
  useSensors,
  type Announcements,
  type DragEndEvent,
  type DragMoveEvent,
  type DragOverEvent,
  type DragStartEvent,
} from "@dnd-kit/core";
import {
  SortableContext,
  sortableKeyboardCoordinates,
  verticalListSortingStrategy,
} from "@dnd-kit/sortable";
import { useQueryClient } from "@tanstack/react-query";
import { ArrowDown, ArrowUp, CircleAlert, Plus, Route } from "lucide-react";
import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
} from "react";

import {
  normalizeIpcError,
  reorderRoutesAndFallback,
  setFallbackEnabled,
} from "../../api/ipc";
import { queryKeys } from "../../api/query";
import type { RouteId, SettingsSnapshotDto } from "../../generated";
import { AppScrollArea } from "../shared/AppScrollArea";
import { FallbackBoundary, FallbackBoundaryVisual } from "./FallbackBoundary";
import { RouteEditor } from "./RouteEditor";
import {
  buildRouteOrderSequence,
  FALLBACK_BOUNDARY_ID,
  fromRouteOrderItemId,
  getVerticalEdgeScrollDirection,
  getRouteSequencePosition,
  moveRouteByDirection,
  moveRouteOrderItem,
  projectRouteOrderSequence,
  routeOrderSequencesEqual,
  type RouteOrderItemId,
} from "./routeOrderSequence";
import { RouteRowOverlay, SortableRouteRow } from "./SortableRouteRow";
import {
  SettingsButton,
  SettingsHelpTooltip,
  SettingsIconButton,
  SettingsPageTitle,
  SettingsSwitch,
} from "./SettingsPrimitives";

const EDGE_SCROLL_ZONE_PX = 36;
const EDGE_SCROLL_STEP_PX = 8;

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

function asRouteOrderItemId(id: string | number): RouteOrderItemId {
  return String(id) as RouteOrderItemId;
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
  const [routeOrderPending, setRouteOrderPending] = useState(false);
  const [routeToolsError, setRouteToolsError] = useState<string | null>(null);
  const [activeItemId, setActiveItemId] = useState<RouteOrderItemId | null>(
    null,
  );
  const [previewSequence, setPreviewSequence] = useState<
    RouteOrderItemId[] | null
  >(null);
  const [dndSession, setDndSession] = useState(0);
  const [overlayWidth, setOverlayWidth] = useState<number | null>(null);
  const routeListViewportRef = useRef<HTMLDivElement | null>(null);
  const previewSequenceRef = useRef<RouteOrderItemId[] | null>(null);
  const activeItemIdRef = useRef<RouteOrderItemId | null>(null);
  const focusAfterResetItemRef = useRef<RouteOrderItemId | null>(null);
  const dragStartSequenceRef = useRef<RouteOrderItemId[]>([]);
  const dragStartSignatureRef = useRef<string | null>(null);
  const dragInputRef = useRef<"keyboard" | "pointer" | null>(null);
  const lastPreviewMotionRef = useRef<string | null>(null);
  const edgeScrollDirectionRef = useRef<-1 | 0 | 1>(0);
  const edgeScrollFrameRef = useRef<number | null>(null);

  const routes = snapshot.routes;
  const confirmedFallbackCount = snapshot.fallback.participantCount;
  const confirmedSequence = useMemo(
    () =>
      buildRouteOrderSequence(
        routes.map((route) => route.routeId),
        confirmedFallbackCount,
      ),
    [confirmedFallbackCount, routes],
  );
  const confirmedSignature = `${snapshot.fallback.configRevision}:${confirmedSequence.join("|")}`;
  const displayedSequence = previewSequence ?? confirmedSequence;
  const displayedProjection = useMemo(
    () => projectRouteOrderSequence(displayedSequence),
    [displayedSequence],
  );
  const routeById = useMemo(
    () => new Map(routes.map((route) => [route.routeId, route])),
    [routes],
  );
  const selectedRouteIndex = routes.findIndex(
    (route) => route.routeId === selectedRouteId,
  );

  const sensors = useSensors(
    useSensor(PointerSensor, {
      activationConstraint: { distance: 8 },
    }),
    useSensor(KeyboardSensor, {
      coordinateGetter: sortableKeyboardCoordinates,
    }),
  );

  const setPreview = useCallback((sequence: RouteOrderItemId[] | null) => {
    previewSequenceRef.current = sequence;
    setPreviewSequence(sequence);
  }, []);

  const stopEdgeScroll = useCallback(() => {
    edgeScrollDirectionRef.current = 0;
    if (edgeScrollFrameRef.current !== null) {
      window.cancelAnimationFrame(edgeScrollFrameRef.current);
      edgeScrollFrameRef.current = null;
    }
  }, []);

  const runEdgeScroll = useCallback(function scrollRouteViewport() {
    edgeScrollFrameRef.current = null;
    const viewport = routeListViewportRef.current;
    const direction = edgeScrollDirectionRef.current;
    if (!viewport || direction === 0 || activeItemIdRef.current === null)
      return;

    const previousScrollTop = viewport.scrollTop;
    viewport.scrollTop += direction * EDGE_SCROLL_STEP_PX;
    if (viewport.scrollTop === previousScrollTop) {
      edgeScrollDirectionRef.current = 0;
      return;
    }
    edgeScrollFrameRef.current =
      window.requestAnimationFrame(scrollRouteViewport);
  }, []);

  const updateEdgeScroll = useCallback(
    (event: DragMoveEvent) => {
      const viewport = routeListViewportRef.current;
      const translated = event.active.rect.current.translated;
      if (!viewport || !translated) {
        stopEdgeScroll();
        return;
      }
      const bounds = viewport.getBoundingClientRect();
      const centerY = translated.top + translated.height / 2;
      const direction = getVerticalEdgeScrollDirection(
        centerY,
        bounds.top,
        bounds.bottom,
        EDGE_SCROLL_ZONE_PX,
      );
      edgeScrollDirectionRef.current = direction;
      if (direction === 0) {
        stopEdgeScroll();
      } else if (edgeScrollFrameRef.current === null) {
        edgeScrollFrameRef.current =
          window.requestAnimationFrame(runEdgeScroll);
      }
    },
    [runEdgeScroll, stopEdgeScroll],
  );

  const restoreSortableFocus = useCallback((itemId: RouteOrderItemId) => {
    focusAfterResetItemRef.current = itemId;
    setDndSession((value) => value + 1);
  }, []);

  const resetDrag = useCallback(
    (
      remountContext: boolean,
      focusItemId = activeItemIdRef.current,
    ) => {
      const activeItemId = activeItemIdRef.current;
      stopEdgeScroll();
      activeItemIdRef.current = null;
      dragStartSignatureRef.current = null;
      dragInputRef.current = null;
      lastPreviewMotionRef.current = null;
      setActiveItemId(null);
      setOverlayWidth(null);
      setPreview(null);
      if (remountContext) {
        const itemId = focusItemId ?? activeItemId;
        if (itemId !== null) restoreSortableFocus(itemId);
      }
    },
    [restoreSortableFocus, setPreview, stopEdgeScroll],
  );

  useEffect(() => {
    const itemId = focusAfterResetItemRef.current;
    if (itemId === null) return;
    focusAfterResetItemRef.current = null;
    const handles = routeListViewportRef.current?.querySelectorAll<HTMLElement>(
      "[data-sortable-handle]",
    );
    for (const handle of handles ?? []) {
      if (handle.dataset.sortableHandle === itemId) {
        handle.focus();
        break;
      }
    }
  }, [dndSession]);

  useEffect(() => {
    if (
      activeItemIdRef.current !== null &&
      dragStartSignatureRef.current !== confirmedSignature
    ) {
      resetDrag(true);
    }
  }, [confirmedSignature, resetDrag]);

  useEffect(() => {
    const cancelForExternalEvent = () => {
      if (activeItemIdRef.current !== null) resetDrag(true);
    };
    const cancelWhenHidden = () => {
      if (document.visibilityState !== "visible") cancelForExternalEvent();
    };
    window.addEventListener("blur", cancelForExternalEvent);
    document.addEventListener("visibilitychange", cancelWhenHidden);
    return () => {
      window.removeEventListener("blur", cancelForExternalEvent);
      document.removeEventListener("visibilitychange", cancelWhenHidden);
      stopEdgeScroll();
    };
  }, [resetDrag, stopEdgeScroll]);

  const refreshRouteState = useCallback(async () => {
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: queryKeys.settings }),
      queryClient.invalidateQueries({ queryKey: queryKeys.bootstrap }),
    ]);
  }, [queryClient]);

  const persistSequence = useCallback(
    async (
      candidate: RouteOrderItemId[],
      focusItemId: RouteOrderItemId | null = null,
    ) => {
      if (routeOrderSequencesEqual(candidate, confirmedSequence)) {
        setPreview(null);
        if (focusItemId !== null) restoreSortableFocus(focusItemId);
        return;
      }

      const projection = projectRouteOrderSequence(candidate);
      setPreview(candidate);
      setRouteToolsBusy(true);
      setRouteOrderPending(true);
      setRouteToolsError(null);
      try {
        await reorderRoutesAndFallback({
          orderedRouteIds: projection.orderedRouteIds,
          participantCount: projection.participantCount,
          expectedConfigRevision: snapshot.fallback.configRevision,
        });
        await refreshRouteState();
        setPreview(null);
      } catch (reason) {
        setPreview(null);
        setRouteToolsError(normalizeIpcError(reason).message);
        try {
          await refreshRouteState();
        } catch {
          // The mutation error remains the stable user-facing result.
        }
      } finally {
        setRouteOrderPending(false);
        setRouteToolsBusy(false);
        if (focusItemId !== null) restoreSortableFocus(focusItemId);
      }
    },
    [
      confirmedSequence,
      refreshRouteState,
      restoreSortableFocus,
      setPreview,
      snapshot.fallback.configRevision,
    ],
  );

  const moveSelectedRoute = async (direction: "up" | "down") => {
    if (!selectedRouteId) return;
    const orderedRouteIds = moveRouteByDirection(
      routes.map((route) => route.routeId),
      selectedRouteId,
      direction,
    );
    const candidate = buildRouteOrderSequence(
      orderedRouteIds,
      confirmedFallbackCount,
    );
    await persistSequence(candidate);
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

  const handleDragStart = (event: DragStartEvent) => {
    const itemId = asRouteOrderItemId(event.active.id);
    if (!confirmedSequence.includes(itemId)) return;
    dragStartSequenceRef.current = [...confirmedSequence];
    dragStartSignatureRef.current = confirmedSignature;
    dragInputRef.current =
      event.activatorEvent instanceof KeyboardEvent ? "keyboard" : "pointer";
    lastPreviewMotionRef.current = null;
    activeItemIdRef.current = itemId;
    setActiveItemId(itemId);
    setPreview([...confirmedSequence]);
    setOverlayWidth(routeListViewportRef.current?.clientWidth ?? null);
    setRouteToolsError(null);
  };

  const handleDragOver = (event: DragOverEvent) => {
    if (!event.over || activeItemIdRef.current === null) return;
    if (dragInputRef.current === "keyboard") return;
    const motionKey = `${event.delta.x}:${event.delta.y}:${routeListViewportRef.current?.scrollTop ?? 0}`;
    if (lastPreviewMotionRef.current === motionKey) return;
    lastPreviewMotionRef.current = motionKey;
    const overId = asRouteOrderItemId(event.over.id);
    const current = previewSequenceRef.current ?? dragStartSequenceRef.current;
    const next = moveRouteOrderItem(current, activeItemIdRef.current, overId);
    if (!routeOrderSequencesEqual(current, next)) {
      setPreview(next);
    }
  };

  const handleDragEnd = (event: DragEndEvent) => {
    stopEdgeScroll();
    const activeId = activeItemIdRef.current;
    const candidate =
      previewSequenceRef.current ?? dragStartSequenceRef.current;
    activeItemIdRef.current = null;
    dragStartSignatureRef.current = null;
    dragInputRef.current = null;
    lastPreviewMotionRef.current = null;
    setActiveItemId(null);
    setOverlayWidth(null);
    if (!activeId || !event.over) {
      setPreview(null);
      if (activeId !== null) restoreSortableFocus(activeId);
      return;
    }
    void persistSequence(candidate, activeId);
  };

  const handleDragCancel = () => resetDrag(false);

  const handleRouteListKeyDown = (event: ReactKeyboardEvent<HTMLElement>) => {
    const activeId = activeItemIdRef.current;
    if (dragInputRef.current !== "keyboard" || activeId === null) return;
    if (event.code === "Escape") {
      event.preventDefault();
      queueMicrotask(() => {
        resetDrag(true, activeId);
      });
      return;
    }
    if (event.code !== "ArrowUp" && event.code !== "ArrowDown") return;

    const current = previewSequenceRef.current ?? dragStartSequenceRef.current;
    const activeIndex = current.indexOf(activeId);
    const nextIndex =
      event.code === "ArrowUp" ? activeIndex - 1 : activeIndex + 1;
    const overId = current[nextIndex];
    if (activeIndex < 0 || !overId) return;
    const next = moveRouteOrderItem(current, activeId, overId);
    if (!routeOrderSequencesEqual(current, next)) {
      setPreview(next);
    }
  };

  const describeItem = useCallback(
    (itemId: RouteOrderItemId, sequence: readonly RouteOrderItemId[]) => {
      const projection = projectRouteOrderSequence(sequence);
      if (itemId === FALLBACK_BOUNDARY_ID) {
        return `Fallback 分界，${projection.participantCount} 条路由参与 Fallback`;
      }
      const routeId = fromRouteOrderItemId(itemId);
      if (!routeId) return "路由";
      const route = routeById.get(routeId);
      const position = getRouteSequencePosition(sequence, routeId);
      if (!position) return route?.name ?? "路由";
      const fallbackDescription =
        position.participantPosition === null
          ? "不参与 Fallback"
          : `参与 Fallback，序号 ${position.participantPosition}`;
      return `${route?.name ?? "路由"}，路由第 ${position.routePosition} 位，${fallbackDescription}`;
    },
    [routeById],
  );

  const announcements = useMemo<Announcements>(
    () => ({
      onDragStart: ({ active }) =>
        `已提起${describeItem(asRouteOrderItemId(active.id), dragStartSequenceRef.current)}。使用上下方向键移动，空格或回车放置，Escape 取消。`,
      onDragOver: ({ active }) =>
        describeItem(
          asRouteOrderItemId(active.id),
          previewSequenceRef.current ?? dragStartSequenceRef.current,
        ),
      onDragMove: ({ active }) =>
        describeItem(
          asRouteOrderItemId(active.id),
          previewSequenceRef.current ?? dragStartSequenceRef.current,
        ),
      onDragEnd: ({ active }) =>
        `已放置${describeItem(
          asRouteOrderItemId(active.id),
          previewSequenceRef.current ?? dragStartSequenceRef.current,
        )}。`,
      onDragCancel: ({ active }) =>
        `已取消移动${describeItem(
          asRouteOrderItemId(active.id),
          dragStartSequenceRef.current,
        )}。`,
    }),
    [describeItem],
  );

  const routeToolsLocked = routeToolsBusy || activeItemId !== null;
  const activeRouteId = activeItemId
    ? fromRouteOrderItemId(activeItemId)
    : null;
  const overlayRoute = activeRouteId ? routeById.get(activeRouteId) : null;
  const overlayPosition = activeRouteId
    ? getRouteSequencePosition(displayedSequence, activeRouteId)
    : null;

  return (
    <div className="routes-settings">
      <DndContext
        key={dndSession}
        sensors={sensors}
        collisionDetection={closestCenter}
        measuring={{
          droppable: { strategy: MeasuringStrategy.Always },
        }}
        autoScroll={false}
        accessibility={{
          announcements,
          screenReaderInstructions: {
            draggable:
              "按空格提起，使用上下方向键移动，按空格或回车放置，按 Escape 取消。",
          },
        }}
        onDragStart={handleDragStart}
        onDragMove={updateEdgeScroll}
        onDragOver={handleDragOver}
        onDragEnd={handleDragEnd}
        onDragCancel={handleDragCancel}
      >
        <section
          className="route-list-pane"
          aria-label="路由列表"
          onKeyDownCapture={handleRouteListKeyDown}
        >
          <span
            className="visually-hidden"
            aria-live="assertive"
            aria-atomic="true"
            data-route-drag-announcement=""
          >
            {activeItemId ? describeItem(activeItemId, displayedSequence) : ""}
          </span>
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
            <SortableContext
              items={displayedSequence}
              strategy={verticalListSortingStrategy}
            >
              {displayedSequence.map((itemId) => {
                if (itemId === FALLBACK_BOUNDARY_ID) {
                  return (
                    <FallbackBoundary
                      key={itemId}
                      participantCount={displayedProjection.participantCount}
                      routeCount={routes.length}
                      disabled={routeToolsBusy}
                      pending={routeOrderPending}
                    />
                  );
                }
                const routeId = fromRouteOrderItemId(itemId);
                const route = routeId ? routeById.get(routeId) : null;
                if (!route || !routeId) return null;
                const routeIndex =
                  displayedProjection.orderedRouteIds.indexOf(routeId);
                return (
                  <SortableRouteRow
                    key={itemId}
                    route={route}
                    routeIndex={routeIndex}
                    fallbackPosition={
                      routeIndex < displayedProjection.participantCount
                        ? routeIndex + 1
                        : null
                    }
                    active={routeId === snapshot.activeRouteId}
                    selected={selectedRouteId === routeId && !newRoute}
                    disabled={routeToolsBusy}
                    onSelectRoute={onSelectRoute}
                  />
                );
              })}
            </SortableContext>
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
        <DragOverlay
          className="route-drag-overlay"
          style={{ width: overlayWidth ?? undefined }}
        >
          {activeItemId === FALLBACK_BOUNDARY_ID ? (
            <FallbackBoundaryVisual
              participantCount={displayedProjection.participantCount}
              routeCount={routes.length}
              disabled={false}
              pending={false}
              dragging
              overlay
            />
          ) : overlayRoute ? (
            <RouteRowOverlay
              route={overlayRoute}
              fallbackPosition={overlayPosition?.participantPosition ?? null}
              active={overlayRoute.routeId === snapshot.activeRouteId}
              selected={overlayRoute.routeId === selectedRouteId && !newRoute}
            />
          ) : null}
        </DragOverlay>
      </DndContext>
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
