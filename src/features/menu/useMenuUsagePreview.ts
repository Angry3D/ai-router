import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type MutableRefObject,
} from "react";

import { isTauriRuntime, setMenuUsagePreview } from "../../api/ipc";
import { useUsageHistory } from "../../api/query";
import type { RouteId, UsageHistoryQueryDto } from "../../generated";

const OPEN_INTENT_MS = 440;
const SWITCH_DWELL_MS = 120;
const CLOSE_GRACE_MS = 180;
const CLOSE_FADE_MS = 100;
const REDUCED_MOTION_QUERY = "(prefers-reduced-motion: reduce)";

export type MenuUsagePreviewPhase =
  "closed" | "opening" | "open" | "switching" | "closing";

function clearPreviewTimers(timers: MutableRefObject<number[]>) {
  timers.current.forEach((timer) => window.clearTimeout(timer));
  timers.current = [];
}

function schedulePreviewTimer(
  timers: MutableRefObject<number[]>,
  callback: () => void,
  delay: number,
) {
  const timer = window.setTimeout(() => {
    timers.current = timers.current.filter((candidate) => candidate !== timer);
    callback();
  }, delay);
  timers.current.push(timer);
}

function usePrefersReducedMotion() {
  const [reduced, setReduced] = useState(
    () =>
      typeof window.matchMedia === "function" &&
      window.matchMedia(REDUCED_MOTION_QUERY).matches,
  );

  useEffect(() => {
    if (typeof window.matchMedia !== "function") return undefined;
    const media = window.matchMedia(REDUCED_MOTION_QUERY);
    const update = () => setReduced(media.matches);
    media.addEventListener("change", update);
    return () => media.removeEventListener("change", update);
  }, []);

  return reduced;
}

export function useMenuUsagePreview({
  generation,
  routeIds,
}: {
  generation: number;
  routeIds: readonly RouteId[];
}) {
  const [hoveredRouteId, setHoveredRouteId] = useState<RouteId | null>(null);
  const [targetRouteId, setTargetRouteId] = useState<RouteId | null>(null);
  const [targetGeneration, setTargetGeneration] = useState(generation);
  const [phase, setPhase] = useState<MenuUsagePreviewPhase>("closed");
  const [nativeReady, setNativeReady] = useState(false);
  const [revision, setRevision] = useState(0);
  const timers = useRef<number[]>([]);
  const latestRevision = useRef(0);
  const latestTransition = useRef(0);
  const reducedMotion = usePrefersReducedMotion();
  const visibleTargetRouteId =
    targetGeneration === generation &&
    targetRouteId !== null &&
    routeIds.includes(targetRouteId)
      ? targetRouteId
      : null;

  const beginTransition = useCallback(() => {
    clearPreviewTimers(timers);
    latestTransition.current += 1;
    return latestTransition.current;
  }, []);

  const resetLocalPreview = useCallback(() => {
    setHoveredRouteId(null);
    setTargetRouteId(null);
    setTargetGeneration(generation);
    setPhase("closed");
    setNativeReady(false);
  }, [generation]);

  const close = (transition: number) => {
    if (latestTransition.current !== transition) return;
    const finishClose = () => {
      if (latestTransition.current !== transition) return;
      resetLocalPreview();
      const nextRevision = latestRevision.current + 1;
      latestRevision.current = nextRevision;
      setRevision(nextRevision);
      if (isTauriRuntime()) {
        schedulePreviewTimer(
          timers,
          () => {
            if (latestTransition.current !== transition) return;
            void setMenuUsagePreview(generation, nextRevision, false).catch(
              () => undefined,
            );
          },
          0,
        );
      }
    };
    if (reducedMotion) {
      finishClose();
      return;
    }
    setPhase("closing");
    schedulePreviewTimer(timers, finishClose, CLOSE_FADE_MS);
  };

  const enterRoute = (routeId: RouteId) => {
    const transition = beginTransition();
    if (!routeIds.includes(routeId)) {
      setHoveredRouteId(null);
      return;
    }
    setHoveredRouteId(routeId);
    if (visibleTargetRouteId === null) {
      setPhase("opening");
      schedulePreviewTimer(
        timers,
        () => {
          if (latestTransition.current !== transition) return;
          const nextRevision = latestRevision.current + 1;
          latestRevision.current = nextRevision;
          setRevision(nextRevision);
          setTargetGeneration(generation);
          setTargetRouteId(routeId);
          setPhase(reducedMotion ? "open" : "opening");
          const revealPreview = () => {
            if (
              latestTransition.current !== transition ||
              latestRevision.current !== nextRevision
            )
              return;
            setNativeReady(true);
            if (!reducedMotion) {
              schedulePreviewTimer(
                timers,
                () => {
                  if (latestTransition.current === transition) setPhase("open");
                },
                0,
              );
            }
          };
          if (isTauriRuntime()) {
            void setMenuUsagePreview(generation, nextRevision, true).then(
              revealPreview,
              () => {
                if (
                  latestTransition.current !== transition ||
                  latestRevision.current !== nextRevision
                )
                  return;
                clearPreviewTimers(timers);
                resetLocalPreview();
              },
            );
          } else revealPreview();
        },
        OPEN_INTENT_MS,
      );
      return;
    }
    if (visibleTargetRouteId === routeId) {
      setPhase("open");
      return;
    }
    setPhase(reducedMotion ? "open" : "switching");
    schedulePreviewTimer(
      timers,
      () => {
        if (latestTransition.current !== transition) return;
        const nextRevision = latestRevision.current + 1;
        latestRevision.current = nextRevision;
        setRevision(nextRevision);
        setTargetGeneration(generation);
        setTargetRouteId(routeId);
        if (!reducedMotion) {
          schedulePreviewTimer(
            timers,
            () => {
              if (latestTransition.current === transition) setPhase("open");
            },
            0,
          );
        }
        if (isTauriRuntime()) {
          void setMenuUsagePreview(generation, nextRevision, true).catch(() => {
            if (
              latestTransition.current !== transition ||
              latestRevision.current !== nextRevision
            )
              return;
            clearPreviewTimers(timers);
            resetLocalPreview();
          });
        }
      },
      reducedMotion ? 0 : SWITCH_DWELL_MS,
    );
  };

  const leaveRegion = () => {
    const transition = beginTransition();
    setHoveredRouteId(null);
    if (visibleTargetRouteId !== null) {
      schedulePreviewTimer(timers, () => close(transition), CLOSE_GRACE_MS);
    } else {
      resetLocalPreview();
    }
  };

  useEffect(() => {
    const targetRemoved =
      targetRouteId !== null && !routeIds.includes(targetRouteId);
    const hoveredRouteRemoved =
      hoveredRouteId !== null && !routeIds.includes(hoveredRouteId);
    if (!targetRemoved && !hoveredRouteRemoved) return;
    if (targetRouteId !== null && targetGeneration !== generation) return;
    const transition = beginTransition();
    queueMicrotask(() => {
      if (latestTransition.current === transition) resetLocalPreview();
    });
    if (targetRouteId !== null) {
      const nextRevision = latestRevision.current + 1;
      latestRevision.current = nextRevision;
      setRevision(nextRevision);
      if (isTauriRuntime()) {
        schedulePreviewTimer(
          timers,
          () => {
            if (latestTransition.current !== transition) return;
            void setMenuUsagePreview(generation, nextRevision, false).catch(
              () => undefined,
            );
          },
          0,
        );
      }
    }
  }, [
    beginTransition,
    generation,
    hoveredRouteId,
    resetLocalPreview,
    routeIds,
    targetGeneration,
    targetRouteId,
  ]);

  useEffect(() => {
    const transition = beginTransition();
    latestRevision.current += 1;
    queueMicrotask(() => {
      if (latestTransition.current === transition) resetLocalPreview();
    });
    return () => {
      clearPreviewTimers(timers);
      latestTransition.current += 1;
    };
  }, [beginTransition, generation, resetLocalPreview]);

  const renderedTargetRouteId = nativeReady ? visibleTargetRouteId : null;
  const query = useMemo<UsageHistoryQueryDto>(
    () => ({
      finishedAtOrAfterMs: null,
      finishedAtOrBeforeMs: Number.MAX_SAFE_INTEGER,
      completionState: null,
      routeId: renderedTargetRouteId,
      modelContains: null,
      cursor: null,
      limit: 10,
    }),
    [renderedTargetRouteId],
  );
  const history = useUsageHistory(query, renderedTargetRouteId !== null);

  return {
    targetRouteId: renderedTargetRouteId,
    hoveredRouteId,
    phase,
    revision,
    history,
    enterRoute,
    leaveRegion,
    enterPreview: () => {
      beginTransition();
      setHoveredRouteId(visibleTargetRouteId);
      if (visibleTargetRouteId !== null) setPhase("open");
    },
    leavePreview: leaveRegion,
  };
}
