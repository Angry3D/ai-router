import { act, fireEvent, render, screen } from "@testing-library/react";
import { useRef, useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { FallbackBoundary } from "./FallbackBoundary";

function Harness({
  onCancel = vi.fn(),
  onCommit = vi.fn(),
  onDraggingChange = vi.fn(),
  routeCount = 4,
  disabled = false,
  pending = false,
}: {
  onCancel?: () => void;
  onCommit?: (value: number) => void;
  onDraggingChange?: (dragging: boolean) => void;
  routeCount?: number;
  disabled?: boolean;
  pending?: boolean;
}) {
  const viewportRef = useRef<HTMLDivElement | null>(null);
  const [value, setValue] = useState(2);
  return (
    <div ref={viewportRef} data-testid="viewport">
      {Array.from({ length: routeCount }, (_, index) => (
        <div key={index} data-fallback-route-index={index} />
      ))}
      <FallbackBoundary
        value={value}
        confirmedValue={2}
        routeCount={routeCount}
        viewportRef={viewportRef}
        disabled={disabled}
        pending={pending}
        onPreview={setValue}
        onCancel={() => {
          setValue(2);
          onCancel();
        }}
        onCommit={onCommit}
        onDraggingChange={onDraggingChange}
      />
    </div>
  );
}

function setGeometry() {
  const viewport = screen.getByTestId("viewport");
  vi.spyOn(viewport, "getBoundingClientRect").mockReturnValue({
    top: 0,
    bottom: 200,
    left: 0,
    right: 240,
    width: 240,
    height: 200,
    x: 0,
    y: 0,
    toJSON: () => ({}),
  });
  for (const [index, row] of Array.from(
    viewport.querySelectorAll<HTMLElement>("[data-fallback-route-index]"),
  ).entries()) {
    vi.spyOn(row, "getBoundingClientRect").mockImplementation(() => {
      const top = index * 50 - viewport.scrollTop;
      return {
        top,
        bottom: top + 40,
        left: 0,
        right: 240,
        width: 240,
        height: 40,
        x: 0,
        y: top,
        toJSON: () => ({}),
      };
    });
  }
  return viewport;
}

afterEach(() => vi.restoreAllMocks());

describe("FallbackBoundary", () => {
  it("starts from the complete bar, previews gaps, and commits once on release", () => {
    const onCommit = vi.fn();
    render(<Harness onCommit={onCommit} />);
    setGeometry();
    const boundary = screen.getByRole("slider", { name: "Fallback 参与分界" });
    Object.assign(boundary, {
      matches: vi.fn().mockReturnValue(true),
      getBoundingClientRect: vi.fn().mockReturnValue({
        top: 80,
        bottom: 109,
        left: 12,
        right: 228,
        width: 216,
        height: 29,
        x: 12,
        y: 80,
        toJSON: () => ({}),
      }),
      setPointerCapture: vi.fn(),
      releasePointerCapture: vi.fn(),
    });

    fireEvent.pointerDown(screen.getByText("以下不参与 Fallback"), {
      button: 0,
      clientY: 100,
      isPrimary: true,
      pointerId: 7,
    });
    expect(screen.getByRole("slider", { name: "Fallback 参与分界" })).toBe(
      boundary,
    );
    expect(boundary).not.toHaveClass("is-detached-sensor");
    expect(boundary.style.cssText).toBe("");
    fireEvent.pointerMove(window, {
      buttons: 1,
      clientY: 96,
      isPrimary: true,
      pointerId: 7,
    });
    expect(boundary).toHaveClass("is-detached-sensor");
    expect(boundary).toHaveStyle({
      top: "80px",
      left: "12px",
      width: "216px",
      height: "29px",
    });
    expect(boundary).toHaveAttribute("aria-valuenow", "2");
    fireEvent.pointerMove(window, {
      buttons: 1,
      clientY: 50,
      isPrimary: true,
      pointerId: 7,
    });
    expect(boundary).toHaveAttribute("aria-valuenow", "1");
    fireEvent.pointerMove(window, {
      buttons: 1,
      clientY: 145,
      isPrimary: true,
      pointerId: 7,
    });
    expect(screen.getByRole("slider", { name: "Fallback 参与分界" })).toBe(
      boundary,
    );
    expect(boundary).toHaveAttribute("aria-valuenow", "3");
    fireEvent.pointerUp(window, {
      button: 0,
      clientY: 145,
      isPrimary: true,
      pointerId: 7,
    });

    expect(onCommit).toHaveBeenCalledTimes(1);
    expect(onCommit).toHaveBeenCalledWith(3);
    expect(boundary).not.toHaveFocus();
    expect(boundary).not.toHaveClass("is-detached-sensor");
    expect(boundary.style.cssText).toBe("");
    expect(boundary).toHaveClass("is-suppressing-hover");
    fireEvent.pointerLeave(boundary);
    expect(boundary).not.toHaveClass("is-suppressing-hover");
  });

  it("keeps clicks and sub-threshold movement out of drag mode", () => {
    const onCancel = vi.fn();
    const onCommit = vi.fn();
    const onDraggingChange = vi.fn();
    render(
      <Harness
        onCancel={onCancel}
        onCommit={onCommit}
        onDraggingChange={onDraggingChange}
      />,
    );
    setGeometry();
    const boundary = screen.getByRole("slider", { name: "Fallback 参与分界" });
    const setPointerCapture = vi.fn();
    const releasePointerCapture = vi.fn();
    Object.assign(boundary, { setPointerCapture, releasePointerCapture });

    fireEvent.pointerDown(boundary, {
      button: 0,
      clientY: 100,
      isPrimary: true,
      pointerId: 8,
    });
    fireEvent.pointerMove(window, {
      buttons: 1,
      clientY: 103,
      isPrimary: true,
      pointerId: 8,
    });

    expect(boundary).not.toHaveClass("is-detached-sensor");
    expect(boundary.style.cssText).toBe("");
    expect(boundary).toHaveAttribute("aria-valuenow", "2");
    expect(onDraggingChange).not.toHaveBeenCalled();

    fireEvent.pointerUp(window, {
      button: 0,
      clientY: 103,
      isPrimary: true,
      pointerId: 8,
    });

    expect(setPointerCapture).toHaveBeenCalledWith(8);
    expect(releasePointerCapture).toHaveBeenCalledWith(8);
    expect(onDraggingChange).not.toHaveBeenCalled();
    expect(onCancel).not.toHaveBeenCalled();
    expect(onCommit).not.toHaveBeenCalled();
  });

  it("uses the label, rail, and grip as the same whole-bar pointer source", () => {
    const onCancel = vi.fn();
    render(<Harness onCancel={onCancel} />);
    setGeometry();
    const boundary = screen.getByRole("slider", { name: "Fallback 参与分界" });
    const setPointerCapture = vi.fn();
    Object.assign(boundary, {
      setPointerCapture,
      releasePointerCapture: vi.fn(),
    });
    const targets = [
      boundary.querySelector(".fallback-boundary-label"),
      boundary.querySelector(".fallback-boundary-rail"),
      boundary.querySelector(".fallback-boundary-grip"),
    ];

    targets.forEach((target, index) => {
      expect(target).not.toBeNull();
      const pointerId = index + 30;
      fireEvent.pointerDown(target as Element, {
        button: 0,
        clientY: 100,
        isPrimary: true,
        pointerId,
      });
      fireEvent.pointerUp(window, {
        button: 0,
        clientY: 100,
        isPrimary: true,
        pointerId,
      });
    });

    expect(setPointerCapture.mock.calls).toEqual([[30], [31], [32]]);
    expect(onCancel).not.toHaveBeenCalled();
  });

  it("cancels a pointer preview without committing", () => {
    const onCancel = vi.fn();
    const onCommit = vi.fn();
    render(<Harness onCancel={onCancel} onCommit={onCommit} />);
    setGeometry();
    const boundary = screen.getByRole("slider", { name: "Fallback 参与分界" });
    Object.assign(boundary, { setPointerCapture: vi.fn() });

    fireEvent.pointerDown(boundary, {
      button: 0,
      clientY: 100,
      isPrimary: true,
      pointerId: 9,
    });
    fireEvent.pointerMove(window, {
      buttons: 1,
      clientY: 10,
      isPrimary: true,
      pointerId: 9,
    });
    expect(boundary).toHaveAttribute("aria-valuenow", "0");
    fireEvent.pointerCancel(boundary, { pointerId: 9 });

    expect(onCancel).toHaveBeenCalledTimes(1);
    expect(onCommit).not.toHaveBeenCalled();
  });

  it("cancels an active drag on Escape", () => {
    const onCancel = vi.fn();
    const onCommit = vi.fn();
    render(<Harness onCancel={onCancel} onCommit={onCommit} />);
    setGeometry();
    const boundary = screen.getByRole("slider", { name: "Fallback 参与分界" });
    Object.assign(boundary, {
      setPointerCapture: vi.fn(),
      releasePointerCapture: vi.fn(),
    });

    fireEvent.pointerDown(boundary, {
      button: 0,
      clientY: 10,
      isPrimary: true,
      pointerId: 12,
    });
    fireEvent.pointerMove(window, {
      buttons: 1,
      clientY: 6,
      isPrimary: true,
      pointerId: 12,
    });
    fireEvent.keyDown(boundary, { key: "Escape" });
    expect(onCancel).toHaveBeenCalledTimes(1);
    expect(onCommit).not.toHaveBeenCalled();
    expect(boundary).toHaveAttribute("aria-valuenow", "2");
  });

  it("keeps dragging when the moving boundary loses WebView pointer capture", () => {
    const onCancel = vi.fn();
    const onCommit = vi.fn();
    render(<Harness onCancel={onCancel} onCommit={onCommit} />);
    setGeometry();
    const boundary = screen.getByRole("slider", { name: "Fallback 参与分界" });
    Object.assign(boundary, {
      setPointerCapture: vi.fn(),
      releasePointerCapture: vi.fn(),
    });

    fireEvent.pointerDown(boundary, {
      button: 0,
      clientY: 100,
      isPrimary: true,
      pointerId: 16,
    });
    fireEvent.pointerMove(window, {
      buttons: 1,
      clientY: 145,
      isPrimary: true,
      pointerId: 16,
    });
    fireEvent.lostPointerCapture(boundary, {
      buttons: 0,
      pointerId: 16,
    });
    expect(onCancel).not.toHaveBeenCalled();
    expect(boundary).toHaveAttribute("aria-valuenow", "3");

    fireEvent.pointerUp(window, {
      button: 0,
      clientY: 145,
      isPrimary: true,
      pointerId: 16,
    });
    expect(onCommit).toHaveBeenCalledTimes(1);
    expect(onCommit).toHaveBeenCalledWith(3);
  });

  it("clears a stale capture before starting the next pointer drag", () => {
    const onCancel = vi.fn();
    const onCommit = vi.fn();
    render(<Harness onCancel={onCancel} onCommit={onCommit} />);
    setGeometry();
    const boundary = screen.getByRole("slider", { name: "Fallback 参与分界" });
    Object.assign(boundary, {
      setPointerCapture: vi.fn(),
      releasePointerCapture: vi.fn(),
    });

    fireEvent.pointerDown(boundary, {
      button: 0,
      clientY: 100,
      isPrimary: true,
      pointerId: 19,
    });
    fireEvent.pointerMove(window, {
      buttons: 1,
      clientY: 104,
      isPrimary: true,
      pointerId: 19,
    });
    fireEvent.lostPointerCapture(boundary, { pointerId: 19 });

    fireEvent.pointerDown(boundary, {
      button: 0,
      clientY: 100,
      isPrimary: true,
      pointerId: 20,
    });
    expect(onCancel).toHaveBeenCalledTimes(1);
    fireEvent.pointerMove(window, {
      buttons: 1,
      clientY: 145,
      isPrimary: true,
      pointerId: 20,
    });
    fireEvent.pointerUp(window, {
      button: 0,
      clientY: 145,
      isPrimary: true,
      pointerId: 20,
    });

    expect(onCommit).toHaveBeenCalledTimes(1);
    expect(onCommit).toHaveBeenCalledWith(3);
  });

  it("cancels on window blur and allows the next pointer drag to start", () => {
    const onCancel = vi.fn();
    const onCommit = vi.fn();
    render(<Harness onCancel={onCancel} onCommit={onCommit} />);
    setGeometry();
    const boundary = screen.getByRole("slider", { name: "Fallback 参与分界" });
    Object.assign(boundary, {
      setPointerCapture: vi.fn(),
      releasePointerCapture: vi.fn(),
    });

    fireEvent.pointerDown(boundary, {
      button: 0,
      clientY: 100,
      isPrimary: true,
      pointerId: 17,
    });
    fireEvent.pointerMove(window, {
      buttons: 1,
      clientY: 10,
      isPrimary: true,
      pointerId: 17,
    });
    expect(boundary).toHaveAttribute("aria-valuenow", "0");
    fireEvent.blur(window);
    expect(onCancel).toHaveBeenCalledTimes(1);
    expect(boundary).toHaveAttribute("aria-valuenow", "2");

    fireEvent.pointerDown(boundary, {
      button: 0,
      clientY: 100,
      isPrimary: true,
      pointerId: 18,
    });
    fireEvent.pointerMove(window, {
      buttons: 1,
      clientY: 145,
      isPrimary: true,
      pointerId: 18,
    });
    fireEvent.pointerUp(window, {
      button: 0,
      clientY: 145,
      isPrimary: true,
      pointerId: 18,
    });
    expect(onCommit).toHaveBeenCalledWith(3);
  });

  it("cancels when the document becomes hidden", () => {
    const onCancel = vi.fn();
    const onCommit = vi.fn();
    render(<Harness onCancel={onCancel} onCommit={onCommit} />);
    setGeometry();
    const boundary = screen.getByRole("slider", { name: "Fallback 参与分界" });
    Object.assign(boundary, {
      setPointerCapture: vi.fn(),
      releasePointerCapture: vi.fn(),
    });
    const visibilityState = vi
      .spyOn(document, "visibilityState", "get")
      .mockReturnValue("hidden");

    fireEvent.pointerDown(boundary, {
      button: 0,
      clientY: 100,
      isPrimary: true,
      pointerId: 23,
    });
    fireEvent.pointerMove(window, {
      buttons: 1,
      clientY: 96,
      isPrimary: true,
      pointerId: 23,
    });
    fireEvent(document, new Event("visibilitychange"));

    expect(onCancel).toHaveBeenCalledTimes(1);
    expect(onCommit).not.toHaveBeenCalled();
    expect(boundary).not.toHaveClass("is-detached-sensor");
    visibilityState.mockRestore();
  });

  it("rolls an active preview back when the boundary unmounts", () => {
    const onCancel = vi.fn();
    const onDraggingChange = vi.fn();
    const view = render(
      <Harness onCancel={onCancel} onDraggingChange={onDraggingChange} />,
    );
    setGeometry();
    const boundary = screen.getByRole("slider", { name: "Fallback 参与分界" });
    Object.assign(boundary, { setPointerCapture: vi.fn() });

    fireEvent.pointerDown(boundary, {
      button: 0,
      clientY: 100,
      isPrimary: true,
      pointerId: 14,
    });
    fireEvent.pointerMove(window, {
      buttons: 1,
      clientY: 10,
      isPrimary: true,
      pointerId: 14,
    });
    expect(boundary).toHaveAttribute("aria-valuenow", "0");
    view.unmount();

    expect(onDraggingChange).toHaveBeenNthCalledWith(1, true);
    expect(onDraggingChange).toHaveBeenNthCalledWith(2, false);
    expect(onCancel).toHaveBeenCalledTimes(1);
  });

  it("commits keyboard gaps, preserves keyboard focus, and ignores input while pending", () => {
    const onCommit = vi.fn();
    const view = render(<Harness onCommit={onCommit} />);
    const boundary = screen.getByRole("slider", { name: "Fallback 参与分界" });

    boundary.focus();
    expect(boundary).toHaveFocus();
    fireEvent.keyDown(boundary, { key: "ArrowUp" });
    fireEvent.keyDown(boundary, { key: "Home" });
    fireEvent.keyDown(boundary, { key: "End" });
    expect(onCommit.mock.calls).toEqual([[1], [0], [4]]);
    expect(boundary).toHaveFocus();
    expect(boundary).not.toHaveClass("is-suppressing-hover");

    view.rerender(
      <Harness key="pending" onCommit={onCommit} disabled pending />,
    );
    const pendingBoundary = screen.getByRole("slider", {
      name: "Fallback 参与分界",
    });
    expect(pendingBoundary).toHaveAttribute("aria-busy", "true");
    expect(pendingBoundary).toHaveAttribute(
      "aria-valuetext",
      "2 条路由参与 Fallback，正在保存",
    );
    fireEvent.keyDown(pendingBoundary, { key: "End" });
    fireEvent.pointerDown(pendingBoundary, {
      button: 0,
      isPrimary: true,
      pointerId: 15,
    });
    expect(onCommit).toHaveBeenCalledTimes(3);
  });

  it("scrolls only the supplied viewport and keeps advancing snapped gaps", () => {
    let frame: FrameRequestCallback | null = null;
    let frameId = 0;
    vi.spyOn(window, "requestAnimationFrame").mockImplementation((callback) => {
      frame = callback;
      frameId += 1;
      return frameId;
    });
    vi.spyOn(window, "cancelAnimationFrame").mockImplementation(() => {});
    render(<Harness routeCount={10} />);
    const viewport = setGeometry();
    Object.defineProperties(viewport, {
      clientHeight: { configurable: true, value: 200 },
      scrollHeight: { configurable: true, value: 500 },
    });
    const boundary = screen.getByRole("slider", { name: "Fallback 参与分界" });
    Object.assign(boundary, { setPointerCapture: vi.fn() });

    fireEvent.pointerDown(boundary, {
      button: 0,
      clientY: 100,
      isPrimary: true,
      pointerId: 11,
    });
    fireEvent.pointerMove(window, {
      buttons: 1,
      clientY: 190,
      isPrimary: true,
      pointerId: 11,
    });
    expect(frame).not.toBeNull();
    act(() => {
      for (let index = 0; index < 4; index += 1) {
        const runFrame = frame as FrameRequestCallback | null;
        runFrame?.(index);
      }
    });

    expect(viewport.scrollTop).toBe(32);
    expect(boundary).toHaveAttribute("aria-valuenow", "4");
    expect(document.scrollingElement?.scrollTop ?? 0).toBe(0);
  });
});
