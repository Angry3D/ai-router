import { GripHorizontal } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import type { RefObject } from "react";

const EDGE_SCROLL_ZONE_PX = 36;
const EDGE_SCROLL_STEP_PX = 8;
const DRAG_ACTIVATION_THRESHOLD_PX = 4;

interface DragGeometry {
  gapOrigin: number;
  routeCenters: number[];
  startPointerY: number;
  startScrollTop: number;
}

interface DragSensorRect {
  top: number;
  left: number;
  width: number;
  height: number;
}

interface FallbackBoundaryProps {
  value: number;
  confirmedValue: number;
  routeCount: number;
  viewportRef: RefObject<HTMLDivElement | null>;
  disabled: boolean;
  pending: boolean;
  onPreview: (value: number) => void;
  onCancel: () => void;
  onCommit: (value: number) => void;
  onDraggingChange: (dragging: boolean) => void;
}

function FallbackBoundaryContent() {
  return (
    <>
      <span className="fallback-boundary-label">以下不参与 Fallback</span>
      <span className="fallback-boundary-rail" aria-hidden="true">
        <GripHorizontal className="fallback-boundary-grip" size={14} />
      </span>
    </>
  );
}

export function FallbackBoundaryPreview() {
  return (
    <div
      className="fallback-boundary is-dragging is-drag-preview"
      data-fallback-boundary-preview=""
      aria-hidden="true"
    >
      <FallbackBoundaryContent />
    </div>
  );
}

export function FallbackBoundary({
  value,
  confirmedValue,
  routeCount,
  viewportRef,
  disabled,
  pending,
  onPreview,
  onCancel,
  onCommit,
  onDraggingChange,
}: FallbackBoundaryProps) {
  const activePointer = useRef<number | null>(null);
  const activeTarget = useRef<HTMLElement | null>(null);
  const currentValue = useRef(value);
  const dragActivated = useRef(false);
  const dragGeometry = useRef<DragGeometry | null>(null);
  const dragSensorStartRect = useRef<DragSensorRect | null>(null);
  const [dragging, setDragging] = useState(false);
  const [dragSensorRect, setDragSensorRect] = useState<DragSensorRect | null>(
    null,
  );
  const [suppressHover, setSuppressHover] = useState(false);
  const lastPointerY = useRef(0);
  const startPointerY = useRef(0);
  const scrollDirection = useRef<-1 | 0 | 1>(0);
  const scrollFrame = useRef<number | null>(null);
  const removeWindowListeners = useRef<() => void>(() => {});
  const confirmedValueRef = useRef(confirmedValue);
  const onCancelRef = useRef(onCancel);
  const onCommitRef = useRef(onCommit);
  const onDraggingChangeRef = useRef(onDraggingChange);
  const onPreviewRef = useRef(onPreview);

  const stopEdgeScroll = () => {
    scrollDirection.current = 0;
    if (scrollFrame.current !== null) {
      window.cancelAnimationFrame(scrollFrame.current);
      scrollFrame.current = null;
    }
  };

  const measureDragGeometry = (clientY: number) => {
    const viewport = viewportRef.current;
    if (!viewport) return null;
    const viewportBounds = viewport.getBoundingClientRect();
    const rows = viewport.querySelectorAll<HTMLElement>(
      "[data-fallback-route-index]",
    );
    const routeGeometry = Array.from(rows, (row) => {
      const bounds = row.getBoundingClientRect();
      return {
        center:
          bounds.top -
          viewportBounds.top +
          viewport.scrollTop +
          bounds.height / 2,
        height: bounds.height,
      };
    });
    const routeCenters = routeGeometry.map(({ center }) => center);
    const boundedValue = Math.min(currentValue.current, routeCenters.length);
    let gapOrigin = clientY - viewportBounds.top + viewport.scrollTop;
    if (routeGeometry.length > 0) {
      if (boundedValue === 0) {
        gapOrigin = routeGeometry[0].center - routeGeometry[0].height / 2;
      } else if (boundedValue === routeGeometry.length) {
        const last = routeGeometry[routeGeometry.length - 1];
        gapOrigin = last.center + last.height / 2;
      } else {
        gapOrigin =
          (routeGeometry[boundedValue - 1].center +
            routeGeometry[boundedValue].center) /
          2;
      }
    }

    return {
      gapOrigin,
      routeCenters,
      startPointerY: clientY,
      startScrollTop: viewport.scrollTop,
    };
  };

  const previewDraggedGap = (clientY: number) => {
    const viewport = viewportRef.current;
    const geometry = dragGeometry.current;
    if (!viewport || !geometry) return;
    const candidate =
      geometry.gapOrigin +
      (clientY - geometry.startPointerY) +
      (viewport.scrollTop - geometry.startScrollTop);
    const nextValue = Math.min(
      routeCount,
      geometry.routeCenters.filter((center) => candidate >= center).length,
    );
    if (nextValue !== currentValue.current) {
      currentValue.current = nextValue;
      onPreviewRef.current(nextValue);
    }
  };

  const runEdgeScroll = () => {
    scrollFrame.current = null;
    const viewport = viewportRef.current;
    if (!viewport || activePointer.current === null) return;
    const direction = scrollDirection.current;
    if (direction === 0) return;

    const previousScrollTop = viewport.scrollTop;
    viewport.scrollTop += direction * EDGE_SCROLL_STEP_PX;
    previewDraggedGap(lastPointerY.current);
    if (viewport.scrollTop === previousScrollTop) {
      scrollDirection.current = 0;
      return;
    }
    scrollFrame.current = window.requestAnimationFrame(runEdgeScroll);
  };

  const updateEdgeScroll = (clientY: number) => {
    const viewport = viewportRef.current;
    if (!viewport) return;
    const bounds = viewport.getBoundingClientRect();
    const direction =
      clientY < bounds.top + EDGE_SCROLL_ZONE_PX
        ? -1
        : clientY > bounds.bottom - EDGE_SCROLL_ZONE_PX
          ? 1
          : 0;
    scrollDirection.current = direction;
    if (direction !== 0 && scrollFrame.current === null) {
      scrollFrame.current = window.requestAnimationFrame(runEdgeScroll);
    } else if (direction === 0 && scrollFrame.current !== null) {
      window.cancelAnimationFrame(scrollFrame.current);
      scrollFrame.current = null;
    }
  };

  const releaseActiveCapture = (pointerId: number) => {
    try {
      activeTarget.current?.releasePointerCapture?.(pointerId);
    } catch {
      // Capture can already be gone after a WebView or DOM transition.
    }
  };

  const finishDrag = (
    commit: boolean,
    clientY?: number,
    clientX?: number,
    restoreRestStyle = false,
  ) => {
    if (activePointer.current === null) return;
    const activated = dragActivated.current;
    if (activated && clientY !== undefined) previewDraggedGap(clientY);
    const pointerId = activePointer.current;
    const nextValue = currentValue.current;
    const preview = viewportRef.current?.querySelector<HTMLElement>(
      "[data-fallback-boundary-preview]",
    );
    const previewBounds = preview?.getBoundingClientRect();
    const pointerStillOverTarget =
      activated &&
      restoreRestStyle &&
      (previewBounds && clientX !== undefined && clientY !== undefined
        ? clientX >= previewBounds.left &&
          clientX <= previewBounds.right &&
          clientY >= previewBounds.top &&
          clientY <= previewBounds.bottom
        : activeTarget.current?.matches(":hover") === true);
    activePointer.current = null;
    dragActivated.current = false;
    dragGeometry.current = null;
    dragSensorStartRect.current = null;
    removeWindowListeners.current();
    removeWindowListeners.current = () => {};
    stopEdgeScroll();
    releaseActiveCapture(pointerId);
    if (!activated) {
      activeTarget.current = null;
      return;
    }
    activeTarget.current?.blur();
    activeTarget.current = null;
    setDragging(false);
    setDragSensorRect(null);
    setSuppressHover(pointerStillOverTarget);
    onDraggingChangeRef.current(false);
    if (!commit || nextValue === confirmedValueRef.current) {
      currentValue.current = confirmedValueRef.current;
      onCancelRef.current();
      return;
    }
    onCommitRef.current(nextValue);
  };

  const cancelDrag = () => {
    finishDrag(false);
  };

  const installWindowListeners = () => {
    const handlePointerMove = (event: PointerEvent) => {
      if (activePointer.current !== event.pointerId) return;
      lastPointerY.current = event.clientY;
      if (!dragActivated.current) {
        if (
          Math.abs(event.clientY - startPointerY.current) <
          DRAG_ACTIVATION_THRESHOLD_PX
        ) {
          return;
        }
        dragActivated.current = true;
        setDragSensorRect(dragSensorStartRect.current);
        setDragging(true);
        onDraggingChangeRef.current(true);
      }
      previewDraggedGap(event.clientY);
      updateEdgeScroll(event.clientY);
    };
    const handlePointerUp = (event: PointerEvent) => {
      if (activePointer.current !== event.pointerId) return;
      finishDrag(true, event.clientY, event.clientX, true);
    };
    const handlePointerCancel = (event: PointerEvent) => {
      if (activePointer.current === event.pointerId) cancelDrag();
    };
    const handleNextPointerDown = () => cancelDrag();
    const handleVisibilityChange = () => {
      if (document.visibilityState !== "visible") cancelDrag();
    };
    window.addEventListener("pointerdown", handleNextPointerDown, true);
    window.addEventListener("pointermove", handlePointerMove, true);
    window.addEventListener("pointerup", handlePointerUp, true);
    window.addEventListener("pointercancel", handlePointerCancel, true);
    window.addEventListener("blur", cancelDrag);
    document.addEventListener("visibilitychange", handleVisibilityChange);
    removeWindowListeners.current = () => {
      window.removeEventListener("pointerdown", handleNextPointerDown, true);
      window.removeEventListener("pointermove", handlePointerMove, true);
      window.removeEventListener("pointerup", handlePointerUp, true);
      window.removeEventListener("pointercancel", handlePointerCancel, true);
      window.removeEventListener("blur", cancelDrag);
      document.removeEventListener("visibilitychange", handleVisibilityChange);
    };
  };

  useEffect(() => {
    currentValue.current = value;
  }, [value]);

  useEffect(() => {
    confirmedValueRef.current = confirmedValue;
    onCancelRef.current = onCancel;
    onCommitRef.current = onCommit;
    onDraggingChangeRef.current = onDraggingChange;
    onPreviewRef.current = onPreview;
  }, [confirmedValue, onCancel, onCommit, onDraggingChange, onPreview]);

  useEffect(() => {
    return () => {
      removeWindowListeners.current();
      removeWindowListeners.current = () => {};
      if (scrollFrame.current !== null) {
        window.cancelAnimationFrame(scrollFrame.current);
        scrollFrame.current = null;
      }
      scrollDirection.current = 0;
      if (activePointer.current !== null) {
        const pointerId = activePointer.current;
        const activated = dragActivated.current;
        activePointer.current = null;
        dragActivated.current = false;
        dragGeometry.current = null;
        dragSensorStartRect.current = null;
        try {
          activeTarget.current?.releasePointerCapture?.(pointerId);
        } catch {
          // Capture may already be gone while the component is unmounting.
        }
        activeTarget.current?.blur();
        activeTarget.current = null;
        if (activated) {
          currentValue.current = confirmedValueRef.current;
          onDraggingChangeRef.current(false);
          onCancelRef.current();
        }
      }
    };
  }, []);

  return (
    <div
      className={`fallback-boundary${dragging ? " is-dragging is-detached-sensor" : ""}${suppressHover ? " is-suppressing-hover" : ""}${pending ? " is-pending" : ""}`}
      data-fallback-boundary-sensor=""
      style={
        dragSensorRect
          ? {
              top: dragSensorRect.top,
              left: dragSensorRect.left,
              width: dragSensorRect.width,
              height: dragSensorRect.height,
            }
          : undefined
      }
      role="slider"
      tabIndex={disabled ? -1 : 0}
      aria-label="Fallback 参与分界"
      aria-orientation="vertical"
      aria-valuemin={0}
      aria-valuemax={routeCount}
      aria-valuenow={value}
      aria-valuetext={`${value} 条路由参与 Fallback${pending ? "，正在保存" : ""}`}
      aria-disabled={disabled}
      aria-busy={pending}
      title="拖动以调整参与 Fallback 的路由数量"
      onPointerDown={(event) => {
        if (disabled || !event.isPrimary || event.button !== 0) return;
        event.preventDefault();
        if (activePointer.current !== null) cancelDrag();
        setSuppressHover(false);
        currentValue.current = value;
        activePointer.current = event.pointerId;
        activeTarget.current = event.currentTarget;
        const bounds = event.currentTarget.getBoundingClientRect();
        dragSensorStartRect.current = {
          top: bounds.top,
          left: bounds.left,
          width: bounds.width,
          height: bounds.height,
        };
        lastPointerY.current = event.clientY;
        startPointerY.current = event.clientY;
        dragGeometry.current = measureDragGeometry(event.clientY);
        event.currentTarget.focus();
        event.currentTarget.setPointerCapture?.(event.pointerId);
        installWindowListeners();
      }}
      onPointerLeave={() => {
        if (activePointer.current === null) setSuppressHover(false);
      }}
      onKeyDown={(event) => {
        if (disabled) return;
        if (event.key === "Escape" && activePointer.current !== null) {
          event.preventDefault();
          cancelDrag();
          return;
        }

        let nextValue: number | null = null;
        if (event.key === "ArrowUp")
          nextValue = Math.max(0, confirmedValue - 1);
        if (event.key === "ArrowDown") {
          nextValue = Math.min(routeCount, confirmedValue + 1);
        }
        if (event.key === "Home") nextValue = 0;
        if (event.key === "End") nextValue = routeCount;
        if (nextValue === null) return;
        event.preventDefault();
        if (nextValue === confirmedValue) return;
        currentValue.current = nextValue;
        onPreview(nextValue);
        onCommit(nextValue);
      }}
    >
      <FallbackBoundaryContent />
    </div>
  );
}
