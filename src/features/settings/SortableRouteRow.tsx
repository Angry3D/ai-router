import { useSortable } from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import { GripVertical } from "lucide-react";

import type { RouteId, SettingsSnapshotDto } from "../../generated";
import { toRouteOrderItemId } from "./routeOrderSequence";

type RouteSummary = SettingsSnapshotDto["routes"][number];

interface RouteRowVisualProps {
  route: RouteSummary;
  fallbackPosition: number | null;
  active: boolean;
  selected: boolean;
  overlay?: boolean;
}

interface SortableRouteRowProps extends RouteRowVisualProps {
  disabled: boolean;
  routeIndex: number;
  onSelectRoute: (routeId: RouteId) => void;
}

function RouteIdentity({ route }: { route: RouteSummary }) {
  return (
    <span className="settings-route-identity">
      <strong>{route.name}</strong>
      <small>{route.baseUrlHost}</small>
    </span>
  );
}

function RouteMarkers({
  fallbackPosition,
  active,
}: Pick<RouteRowVisualProps, "fallbackPosition" | "active">) {
  return (
    <span className="settings-route-markers">
      {fallbackPosition !== null ? <b>Fallback {fallbackPosition}</b> : null}
      {active ? <em>当前</em> : null}
    </span>
  );
}

export function RouteRowOverlay({
  route,
  fallbackPosition,
  active,
  selected,
}: RouteRowVisualProps) {
  return (
    <div
      className={`settings-route-row${selected ? " selected" : ""} is-drag-overlay`}
      aria-hidden="true"
      data-route-drag-overlay=""
    >
      <span className="settings-route-drag-handle">
        <GripVertical aria-hidden="true" size={15} />
      </span>
      <span className="settings-route-select">
        <RouteIdentity route={route} />
        <RouteMarkers fallbackPosition={fallbackPosition} active={active} />
      </span>
    </div>
  );
}

export function SortableRouteRow({
  route,
  fallbackPosition,
  active,
  selected,
  disabled,
  routeIndex,
  onSelectRoute,
}: SortableRouteRowProps) {
  const itemId = toRouteOrderItemId(route.routeId);
  const {
    attributes,
    listeners,
    setActivatorNodeRef,
    setNodeRef,
    transform,
    transition,
    isDragging,
  } = useSortable({ id: itemId, disabled });

  return (
    <div
      ref={setNodeRef}
      className={`settings-route-row${selected ? " selected" : ""}${isDragging ? " is-source-placeholder" : ""}`}
      data-fallback-route-index={routeIndex}
      data-sortable-item={itemId}
      style={{
        transform:
          !isDragging && transform
            ? CSS.Transform.toString({ ...transform, x: 0 })
            : undefined,
        transition,
      }}
    >
      <button
        ref={setActivatorNodeRef}
        type="button"
        className="settings-route-drag-handle"
        disabled={disabled}
        aria-label={`拖动调整路由顺序：${route.name}`}
        title="拖动调整路由顺序"
        data-sortable-handle={itemId}
        {...attributes}
        {...listeners}
      >
        <GripVertical aria-hidden="true" size={15} />
      </button>
      <button
        type="button"
        className="settings-route-select"
        onClick={() => onSelectRoute(route.routeId)}
      >
        <RouteIdentity route={route} />
        <RouteMarkers fallbackPosition={fallbackPosition} active={active} />
      </button>
    </div>
  );
}
