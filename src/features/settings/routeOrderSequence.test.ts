import { describe, expect, it } from "vitest";

import type { RouteId } from "../../generated";
import {
  buildRouteOrderSequence,
  FALLBACK_BOUNDARY_ID,
  getVerticalEdgeScrollDirection,
  getRouteSequencePosition,
  moveRouteByDirection,
  moveRouteOrderItem,
  projectRouteOrderSequence,
  routeOrderSequencesEqual,
  toRouteOrderItemId,
} from "./routeOrderSequence";

const ROUTES = ["route-a", "route-b", "route-c", "route-d"] as RouteId[];

describe("routeOrderSequence", () => {
  it.each([
    [
      0,
      [
        FALLBACK_BOUNDARY_ID,
        "route:route-a",
        "route:route-b",
        "route:route-c",
        "route:route-d",
      ],
    ],
    [
      2,
      [
        "route:route-a",
        "route:route-b",
        FALLBACK_BOUNDARY_ID,
        "route:route-c",
        "route:route-d",
      ],
    ],
    [
      4,
      [
        "route:route-a",
        "route:route-b",
        "route:route-c",
        "route:route-d",
        FALLBACK_BOUNDARY_ID,
      ],
    ],
  ])("builds and projects a boundary at %i", (participantCount, expected) => {
    const sequence = buildRouteOrderSequence(ROUTES, participantCount);
    expect(sequence).toEqual(expected);
    expect(projectRouteOrderSequence(sequence)).toEqual({
      orderedRouteIds: ROUTES,
      participantCount,
    });
  });

  it("projects a route crossing the boundary from one sequence", () => {
    const confirmed = buildRouteOrderSequence(ROUTES, 2);
    const preview = moveRouteOrderItem(
      confirmed,
      toRouteOrderItemId(ROUTES[3]),
      toRouteOrderItemId(ROUTES[0]),
    );

    expect(projectRouteOrderSequence(preview)).toEqual({
      orderedRouteIds: [ROUTES[3], ROUTES[0], ROUTES[1], ROUTES[2]],
      participantCount: 3,
    });
    expect(getRouteSequencePosition(preview, ROUTES[3])).toEqual({
      routePosition: 1,
      participantPosition: 1,
      participantCount: 3,
    });
    expect(getRouteSequencePosition(preview, ROUTES[2])).toEqual({
      routePosition: 4,
      participantPosition: null,
      participantCount: 3,
    });
  });

  it("moves the boundary without changing route order", () => {
    const confirmed = buildRouteOrderSequence(ROUTES, 2);
    const preview = moveRouteOrderItem(
      confirmed,
      FALLBACK_BOUNDARY_ID,
      toRouteOrderItemId(ROUTES[2]),
    );

    expect(projectRouteOrderSequence(preview)).toEqual({
      orderedRouteIds: ROUTES,
      participantCount: 3,
    });
  });

  it("swaps only adjacent routes for toolbar moves", () => {
    expect(moveRouteByDirection(ROUTES, ROUTES[1], "up")).toEqual([
      ROUTES[1],
      ROUTES[0],
      ROUTES[2],
      ROUTES[3],
    ]);
    expect(moveRouteByDirection(ROUTES, ROUTES[1], "down")).toEqual([
      ROUTES[0],
      ROUTES[2],
      ROUTES[1],
      ROUTES[3],
    ]);
    expect(moveRouteByDirection(ROUTES, ROUTES[0], "up")).toEqual(ROUTES);
  });

  it("detects exact no-ops", () => {
    const confirmed = buildRouteOrderSequence(ROUTES, 2);
    expect(routeOrderSequencesEqual(confirmed, [...confirmed])).toBe(true);
    expect(
      routeOrderSequencesEqual(
        confirmed,
        moveRouteOrderItem(
          confirmed,
          toRouteOrderItemId(ROUTES[0]),
          toRouteOrderItemId(ROUTES[1]),
        ),
      ),
    ).toBe(false);
  });

  it("derives edge scrolling without addressing any outer scroll owner", () => {
    expect(getVerticalEdgeScrollDirection(20, 0, 240, 36)).toBe(-1);
    expect(getVerticalEdgeScrollDirection(120, 0, 240, 36)).toBe(0);
    expect(getVerticalEdgeScrollDirection(220, 0, 240, 36)).toBe(1);
  });
});
