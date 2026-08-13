import { QueryClientProvider } from "@tanstack/react-query";
import { useEffect, useState } from "react";

import { App, type AppView } from "./App";
import {
  createRouterQueryClient,
  useBootstrapSnapshot,
  useRouterStateSync,
} from "./api/query";
import { isTauriRuntime } from "./api/ipc";
import { queryKeys } from "./api/query";
import { AppearanceProvider } from "./features/appearance/AppearanceProvider";

function DevelopmentPreviewData({
  queryClient,
}: {
  queryClient: ReturnType<typeof createRouterQueryClient>;
}) {
  useEffect(() => {
    if (!import.meta.env.DEV || isTauriRuntime()) return undefined;
    let disposed = false;
    void import("./previewFixtures").then((fixtures) => {
      if (disposed) return;
      const parameters = new URLSearchParams(window.location.search);
      const mode = parameters.get("recovery");
      const imagesMode = parameters.get("images");
      const fatalIssue = mode?.startsWith("fatal-")
        ? mode.slice("fatal-".length)
        : null;
      const isFatalIssue = (
        value: string | null,
      ): value is keyof typeof fixtures.previewFatalDatabaseBootstraps =>
        value !== null && value in fixtures.previewFatalDatabaseBootstraps;
      const bootstrap =
        mode === "required" || mode === "empty"
          ? fixtures.previewRecoveryRequiredBootstrap
          : isFatalIssue(fatalIssue)
            ? fixtures.previewFatalDatabaseBootstraps[fatalIssue]
            : fixtures.previewMenuSnapshot.bootstrap;
      const settings =
        mode === null && imagesMode === "missing"
          ? fixtures.previewMissingImageRouteSettingsSnapshot
          : mode === null && parameters.get("routes") === "long"
            ? fixtures.previewLongRoutesSettingsSnapshot
            : mode === "updating"
              ? fixtures.previewUpdatingSettingsSnapshot
              : mode === "degraded"
                ? fixtures.previewDegradedSettingsSnapshot
                : fixtures.previewSettingsSnapshot;
      queryClient.setQueryData(queryKeys.bootstrap, bootstrap);
      queryClient.setQueryData(queryKeys.menu, fixtures.previewMenuSnapshot);
      queryClient.setQueryData(queryKeys.settings, settings);
      if (mode === "required") {
        queryClient.setQueryData(
          queryKeys.recovery,
          fixtures.previewRecoveryWithCandidates,
        );
      } else if (mode === "empty") {
        queryClient.setQueryData(
          queryKeys.recovery,
          fixtures.previewRecoveryWithoutCandidates,
        );
      } else if (isFatalIssue(fatalIssue)) {
        queryClient.setQueryData(
          queryKeys.recovery,
          fixtures.previewFatalRecoverySnapshots[fatalIssue],
        );
      }
    });
    return () => {
      disposed = true;
    };
  }, [queryClient]);
  return null;
}

function RuntimeBindings({ view }: { view: AppView }) {
  useBootstrapSnapshot();
  useRouterStateSync(view);
  return (
    <AppearanceProvider>
      <App view={view} />
    </AppearanceProvider>
  );
}

export function AppRoot({ view }: { view: AppView }) {
  const [queryClient] = useState(createRouterQueryClient);
  return (
    <QueryClientProvider client={queryClient}>
      <DevelopmentPreviewData queryClient={queryClient} />
      <RuntimeBindings view={view} />
    </QueryClientProvider>
  );
}
