import type { RouteHealthDto } from "../../generated";
import type { RoutesHealthMessages } from "../language/catalog/routes";

export interface RouteHealthPresentation {
  marker: string | null;
  detail: string | null;
  tone: "warning" | "accent";
}

function remainingSeconds(seconds: number, elapsedSeconds: number) {
  return Math.max(0, Math.ceil(seconds) - elapsedSeconds);
}

export function routeHealthPresentation(
  health: RouteHealthDto | null,
  active: boolean,
  messages: RoutesHealthMessages,
  elapsedSeconds = 0,
): RouteHealthPresentation {
  if (!health) {
    return {
      marker: active ? messages.current : null,
      detail: null,
      tone: "accent",
    };
  }

  switch (health.kind) {
    case "striking":
      return {
        marker: active
          ? messages.strikingCurrent(health.failureCount)
          : messages.striking(health.failureCount),
        detail: messages.strikingDetail(health.failureCount),
        tone: "warning",
      };
    case "switching":
      return {
        marker: active ? messages.switchingCurrent : messages.switching,
        detail: messages.switchingDetail,
        tone: "warning",
      };
    case "switch_pending": {
      const remaining =
        health.retryAfterSeconds === null
          ? null
          : remainingSeconds(health.retryAfterSeconds, elapsedSeconds);
      return {
        marker: active ? messages.switchPendingCurrent : messages.switchPending,
        detail:
          remaining !== null && remaining > 0
            ? messages.switchPendingDetail(remaining)
            : messages.switchPendingWaiting,
        tone: "warning",
      };
    }
    case "open": {
      const remaining = remainingSeconds(
        health.retryAfterSeconds,
        elapsedSeconds,
      );
      if (health.recoverySuccesses > 0) {
        return {
          marker: messages.recovering,
          detail:
            remaining > 0
              ? messages.recoveringDetailRemaining(remaining)
              : messages.recoveringDetailWaiting,
          tone: "warning",
        };
      }
      if (remaining > 0) {
        return health.origin === "model_bypassed"
          ? {
              marker: messages.bypassedMarker(remaining),
              detail: messages.bypassedDetail(remaining),
              tone: "warning",
            }
          : {
              marker: messages.pausedMarker(remaining),
              detail: messages.pausedDetail(remaining),
              tone: "warning",
            };
      }
      return {
        marker: messages.pendingMarker,
        detail: messages.pendingDetail,
        tone: "warning",
      };
    }
    case "probing":
      return {
        marker: messages.probingMarker,
        detail: messages.probingDetail(
          health.recoverySuccesses > 0 ? 2 : 1,
        ),
        tone: "accent",
      };
  }
}

export function routeHealthCountdownSeconds(health: RouteHealthDto | null) {
  if (health?.kind === "open") return health.retryAfterSeconds;
  if (health?.kind === "switch_pending") {
    return health.retryAfterSeconds ?? 0;
  }
  return 0;
}
