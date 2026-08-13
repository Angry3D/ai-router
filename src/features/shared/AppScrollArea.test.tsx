import { render, screen } from "@testing-library/react";
import { createRef } from "react";
import { describe, expect, it } from "vitest";

import { AppScrollArea } from "./AppScrollArea";

describe("AppScrollArea", () => {
  it("renders children inside the scrolling viewport with forwarded semantics", () => {
    const viewportRef = createRef<HTMLDivElement>();
    render(
      <AppScrollArea
        className="host-class"
        viewportClassName="viewport-class"
        viewportRef={viewportRef}
        viewportProps={{ role: "listbox", "aria-label": "路由" }}
      >
        <div role="option" aria-selected={false}>
          第一行
        </div>
      </AppScrollArea>,
    );

    const viewport = screen.getByRole("listbox", { name: "路由" });
    expect(viewportRef.current).toBe(viewport);
    expect(viewport).toHaveAttribute("data-radix-scroll-area-viewport");
    expect(viewport).toHaveClass("app-scroll-viewport", "viewport-class");
    expect(screen.getByRole("option", { name: "第一行" })).toBeInTheDocument();
  });

  it("applies the host class to the outer scroll area root", () => {
    const viewportRef = createRef<HTMLDivElement>();
    render(
      <AppScrollArea className="host-class" viewportRef={viewportRef}>
        <p>内容</p>
      </AppScrollArea>,
    );

    const root = viewportRef.current?.parentElement;
    expect(root).not.toBeNull();
    expect(root).toHaveClass("app-scroll-area", "host-class");
  });

  it("renders both overlay tracks only when both axes are requested", () => {
    const { container } = render(
      <AppScrollArea axis="both">
        <div>宽内容</div>
      </AppScrollArea>,
    );

    expect(
      container.querySelectorAll(
        '.app-scroll-scrollbar[data-orientation="vertical"]',
      ),
    ).toHaveLength(1);
    expect(
      container.querySelectorAll(
        '.app-scroll-scrollbar[data-orientation="horizontal"]',
      ),
    ).toHaveLength(1);
  });

  it("forwards the Tauri nonce instead of a consumer viewport override", () => {
    const sentinel = document.createElement("style");
    sentinel.id = "app-csp-style-nonce";
    sentinel.nonce = "tauri-style-nonce";
    document.head.append(sentinel);
    const viewportProps = {
      nonce: "consumer-nonce",
      role: "region" as const,
    };

    try {
      const { container } = render(
        <AppScrollArea viewportProps={viewportProps}>
          <div>内容</div>
        </AppScrollArea>,
      );

      expect(container.querySelector("style")).toHaveAttribute(
        "nonce",
        "tauri-style-nonce",
      );
    } finally {
      sentinel.remove();
    }
  });
});
