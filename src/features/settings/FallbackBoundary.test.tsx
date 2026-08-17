import { DndContext } from "@dnd-kit/core";
import {
  SortableContext,
  verticalListSortingStrategy,
} from "@dnd-kit/sortable";
import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { FallbackBoundary, FallbackBoundaryVisual } from "./FallbackBoundary";
import { FALLBACK_BOUNDARY_ID } from "./routeOrderSequence";

function renderBoundary({
  disabled = false,
  pending = false,
}: {
  disabled?: boolean;
  pending?: boolean;
} = {}) {
  return render(
    <DndContext>
      <SortableContext
        items={[FALLBACK_BOUNDARY_ID]}
        strategy={verticalListSortingStrategy}
      >
        <FallbackBoundary
          participantCount={2}
          routeCount={4}
          disabled={disabled}
          pending={pending}
        />
      </SortableContext>
    </DndContext>,
  );
}

describe("FallbackBoundary", () => {
  it("uses the complete 29px bar as the single sortable activator", () => {
    const { container } = renderBoundary();
    const boundary = screen.getByRole("button", {
      name: "拖动调整 Fallback 参与分界",
    });

    expect(boundary).toHaveAttribute(
      "data-sortable-handle",
      FALLBACK_BOUNDARY_ID,
    );
    expect(boundary).toHaveTextContent("以下不参与 Fallback");
    expect(boundary).toHaveAccessibleDescription(
      "当前 2 条路由参与 Fallback，共 4 条路由",
    );
    expect(container.querySelectorAll(".fallback-boundary")).toHaveLength(1);
    expect(boundary.querySelector(".fallback-boundary-rail")).toHaveAttribute(
      "aria-hidden",
      "true",
    );
  });

  it("exposes a stable pending and disabled state", () => {
    renderBoundary({ disabled: true, pending: true });
    const boundary = screen.getByRole("button", {
      name: "拖动调整 Fallback 参与分界",
    });

    expect(boundary).toBeDisabled();
    expect(boundary).toHaveAttribute("aria-busy", "true");
    expect(boundary).toHaveAccessibleDescription(
      "当前 2 条路由参与 Fallback，共 4 条路由，正在保存",
    );
    expect(boundary).toHaveClass("is-pending");
  });

  it("renders a non-interactive overlay with the same content", () => {
    render(
      <FallbackBoundaryVisual
        participantCount={3}
        routeCount={4}
        disabled={false}
        pending={false}
        dragging
        overlay
      />,
    );

    const overlay = document.querySelector("[data-fallback-boundary-overlay]");
    expect(overlay).toHaveClass("fallback-boundary", "is-dragging");
    expect(overlay).toHaveAttribute("aria-hidden", "true");
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
  });
});
