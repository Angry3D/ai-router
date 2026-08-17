import type { RouteId } from "../../generated";

export const FALLBACK_BOUNDARY_ID = "fallback-boundary" as const;

export type RouteOrderItemId = `route:${string}` | typeof FALLBACK_BOUNDARY_ID;

export interface RouteOrderProjection {
  orderedRouteIds: RouteId[];
  participantCount: number;
}

export interface RouteSequencePosition {
  routePosition: number;
  participantPosition: number | null;
  participantCount: number;
}

export function getVerticalEdgeScrollDirection(
  centerY: number,
  viewportTop: number,
  viewportBottom: number,
  edgeZone: number,
): -1 | 0 | 1 {
  if (centerY < viewportTop + edgeZone) return -1;
  if (centerY > viewportBottom - edgeZone) return 1;
  return 0;
}

export function toRouteOrderItemId(routeId: RouteId): RouteOrderItemId {
  return `route:${routeId}`;
}

export function fromRouteOrderItemId(itemId: RouteOrderItemId): RouteId | null {
  return itemId === FALLBACK_BOUNDARY_ID
    ? null
    : (itemId.slice("route:".length) as RouteId);
}

export function buildRouteOrderSequence(
  routeIds: readonly RouteId[],
  participantCount: number,
): RouteOrderItemId[] {
  const boundaryIndex = Math.max(
    0,
    Math.min(routeIds.length, Math.trunc(participantCount)),
  );
  const routeItems = routeIds.map(toRouteOrderItemId);
  return [
    ...routeItems.slice(0, boundaryIndex),
    FALLBACK_BOUNDARY_ID,
    ...routeItems.slice(boundaryIndex),
  ];
}

export function projectRouteOrderSequence(
  sequence: readonly RouteOrderItemId[],
): RouteOrderProjection {
  const participantCount = sequence.indexOf(FALLBACK_BOUNDARY_ID);
  if (participantCount < 0) {
    throw new Error("Fallback boundary is missing from the route sequence");
  }

  return {
    orderedRouteIds: sequence.flatMap((itemId) => {
      const routeId = fromRouteOrderItemId(itemId);
      return routeId === null ? [] : [routeId];
    }),
    participantCount,
  };
}

export function moveRouteOrderItem(
  sequence: readonly RouteOrderItemId[],
  activeId: RouteOrderItemId,
  overId: RouteOrderItemId,
): RouteOrderItemId[] {
  const activeIndex = sequence.indexOf(activeId);
  const overIndex = sequence.indexOf(overId);
  if (activeIndex < 0 || overIndex < 0 || activeIndex === overIndex) {
    return [...sequence];
  }

  const next = [...sequence];
  const [moved] = next.splice(activeIndex, 1);
  next.splice(overIndex, 0, moved);
  return next;
}

export function moveRouteByDirection(
  routeIds: readonly RouteId[],
  routeId: RouteId,
  direction: "up" | "down",
): RouteId[] {
  const routeIndex = routeIds.indexOf(routeId);
  const nextIndex = direction === "up" ? routeIndex - 1 : routeIndex + 1;
  if (routeIndex < 0 || nextIndex < 0 || nextIndex >= routeIds.length) {
    return [...routeIds];
  }

  const next = [...routeIds];
  [next[routeIndex], next[nextIndex]] = [next[nextIndex], next[routeIndex]];
  return next;
}

export function routeOrderSequencesEqual(
  left: readonly RouteOrderItemId[],
  right: readonly RouteOrderItemId[],
): boolean {
  return (
    left.length === right.length &&
    left.every((itemId, index) => itemId === right[index])
  );
}

export function getRouteSequencePosition(
  sequence: readonly RouteOrderItemId[],
  routeId: RouteId,
): RouteSequencePosition | null {
  const projection = projectRouteOrderSequence(sequence);
  const routePosition = projection.orderedRouteIds.indexOf(routeId);
  if (routePosition < 0) return null;

  return {
    routePosition: routePosition + 1,
    participantPosition:
      routePosition < projection.participantCount ? routePosition + 1 : null,
    participantCount: projection.participantCount,
  };
}
