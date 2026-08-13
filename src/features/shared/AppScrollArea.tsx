import type { ComponentPropsWithoutRef, ReactNode, Ref } from "react";
import * as ScrollAreaPrimitive from "@radix-ui/react-scroll-area";

type ViewportProps = Omit<
  ComponentPropsWithoutRef<typeof ScrollAreaPrimitive.Viewport>,
  "className" | "children" | "asChild" | "nonce"
>;

interface AppScrollAreaProps {
  children: ReactNode;
  className?: string;
  viewportClassName?: string;
  viewportRef?: Ref<HTMLDivElement>;
  viewportProps?: ViewportProps;
  axis?: "vertical" | "both";
}

function classes(...names: Array<string | undefined>): string {
  return names.filter(Boolean).join(" ");
}

function getStyleNonce(): string | undefined {
  const nonce = document.getElementById("app-csp-style-nonce")?.nonce?.trim();
  return nonce || undefined;
}

export function AppScrollArea({
  children,
  className,
  viewportClassName,
  viewportRef,
  viewportProps,
  axis = "vertical",
}: AppScrollAreaProps) {
  return (
    <ScrollAreaPrimitive.Root
      type="scroll"
      scrollHideDelay={600}
      className={classes("app-scroll-area", className)}
    >
      <ScrollAreaPrimitive.Viewport
        {...viewportProps}
        nonce={getStyleNonce()}
        ref={viewportRef}
        className={classes("app-scroll-viewport", viewportClassName)}
      >
        {children}
      </ScrollAreaPrimitive.Viewport>
      <ScrollAreaPrimitive.Scrollbar
        orientation="vertical"
        forceMount
        className="app-scroll-scrollbar"
      >
        <ScrollAreaPrimitive.Thumb className="app-scroll-thumb" />
      </ScrollAreaPrimitive.Scrollbar>
      {axis === "both" ? (
        <>
          <ScrollAreaPrimitive.Scrollbar
            orientation="horizontal"
            forceMount
            className="app-scroll-scrollbar"
          >
            <ScrollAreaPrimitive.Thumb className="app-scroll-thumb" />
          </ScrollAreaPrimitive.Scrollbar>
          <ScrollAreaPrimitive.Corner />
        </>
      ) : null}
    </ScrollAreaPrimitive.Root>
  );
}
