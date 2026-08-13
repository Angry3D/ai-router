import type { RefObject } from "react";
import { useEffect, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";

import {
  completeMenuShow,
  getMenuSnapshot,
  hideMenu,
  isTauriRuntime,
  listenMenuPositioned,
  listenMenuPrepare,
  menuFrontendReady,
} from "../../api/ipc";
import { queryKeys } from "../../api/query";

const HIDDEN_WINDOW_LAYOUT_FALLBACK_MS = 100;
export interface MenuPreviewLayout {
  side: "left" | "right";
  width: number;
  height: number;
}

function afterLayout(callback: () => void) {
  let completed = false;
  const completeOnce = () => {
    if (completed) return;
    completed = true;
    window.clearTimeout(fallbackId);
    callback();
  };

  const fallbackId = window.setTimeout(
    completeOnce,
    HIDDEN_WINDOW_LAYOUT_FALLBACK_MS,
  );
  requestAnimationFrame(() => requestAnimationFrame(completeOnce));
}

export function useMenuPopover(shellRef: RefObject<HTMLElement | null>) {
  const queryClient = useQueryClient();
  const latestGeneration = useRef(0);
  const [showGeneration, setShowGeneration] = useState(0);
  const [previewLayout, setPreviewLayout] = useState<MenuPreviewLayout>({
    side: "left",
    width: 384,
    height: 480,
  });

  useEffect(() => {
    if (!isTauriRuntime()) return undefined;
    let disposed = false;
    const unlisteners: Array<() => void> = [];

    void Promise.all([
      listenMenuPrepare(({ generation }) => {
        latestGeneration.current = generation;
        setShowGeneration(generation);
        void queryClient
          .fetchQuery({
            queryKey: queryKeys.menu,
            queryFn: getMenuSnapshot,
            staleTime: 0,
          })
          .catch(() => undefined)
          .finally(() => {
            afterLayout(() => {
              if (disposed || latestGeneration.current !== generation) return;
              const height = shellRef.current?.scrollHeight ?? 320;
              void completeMenuShow(generation, height);
            });
          });
      }),
      listenMenuPositioned(
        ({
          generation,
          arrowOffsetX,
          previewSide,
          previewWidth,
          previewHeight,
        }) => {
          if (generation !== latestGeneration.current) return;
          const layout = {
            side: previewSide,
            width: previewWidth,
            height: previewHeight,
          };
          setPreviewLayout(layout);
          document.documentElement.style.setProperty(
            "--menu-preview-width",
            `${previewWidth}px`,
          );
          document.documentElement.style.setProperty(
            "--menu-preview-height",
            `${previewHeight}px`,
          );
          document.documentElement.style.setProperty(
            "--menu-arrow-x",
            `${arrowOffsetX}px`,
          );
        },
      ),
    ])
      .then((listeners) => {
        if (disposed) listeners.forEach((unlisten) => unlisten());
        else {
          unlisteners.push(...listeners);
          void menuFrontendReady();
        }
      })
      .catch(() => undefined);

    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      void hideMenu();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => {
      disposed = true;
      unlisteners.forEach((unlisten) => unlisten());
      window.removeEventListener("keydown", onKeyDown);
    };
  }, [queryClient, shellRef]);

  return { generation: showGeneration, previewLayout };
}
