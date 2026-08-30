import { QueryClientProvider } from "@tanstack/react-query";
import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { App } from "./App";
import { createRouterQueryClient, queryKeys } from "./api/query";
import type { MenuSnapshotDto, SettingsSnapshotDto } from "./generated";

const bootstrap = {
  revision: 0,
  routes: [],
  activeRouteId: null,
  fallback: {
    enabled: false,
    participantCount: 0,
    configRevision: 0,
    activePosition: null,
    hasNext: false,
  },
  proxyStatus: "running" as const,
  lifecycle: { phase: "running" as const, issue: null },
  appearancePreference: "system" as const,
};

function renderWithData(view: "menu" | "settings") {
  const client = createRouterQueryClient();
  client.setQueryData<MenuSnapshotDto>(queryKeys.menu, {
    bootstrap,
    balances: [],
    balanceEnabledRouteIds: [],
    balanceBatch: null,
    codexStatus: "not_connected",
    codexRestartNotice: null,
    mcpImageCapacity: {
      available: true,
      imageCount: 0,
      bytes: 0,
      thresholdMib: 1024,
      overThreshold: false,
      warningEpisodeId: null,
      warningVisible: false,
    },
  });
  client.setQueryData<SettingsSnapshotDto>(queryKeys.settings, {
    routes: [],
    activeRouteId: null,
    fallback: {
      enabled: false,
      participantCount: 0,
      configRevision: 0,
      activePosition: null,
      hasNext: false,
    },
    proxyPort: 32189,
    menuBar: { statusTextEnabled: true, activityAnimationEnabled: true },
    imagesGeneration: { enabled: false, routeId: null, timeoutSecs: 600 },
    mcpImageCapacity: {
      available: true,
      imageCount: 0,
      bytes: 0,
      thresholdMib: 1024,
      overThreshold: false,
      warningEpisodeId: null,
      warningVisible: false,
    },
    codexStatus: "not_connected",
    baseline: { exists: false, originalExists: null, capturedAtMs: null },
    originalBackup: { exists: false, originalExists: null, capturedAtMs: null },
    recoveryConfig: { exists: false, originalExists: null, updatedAtMs: null },
    balanceScriptRiskConfirmed: false,
    balanceQuery: { menuDebounceSeconds: 30, automaticRefreshMinutes: 30 },
    history: {
      requestCount: 0,
      earliestStartedAtMs: null,
      latestStartedAtMs: null,
      databaseBytes: 0,
      retentionDays: 365,
    },
    metadataFailure: { droppedRecords: 0, writeFailures: 0, lastError: null },
    recovery: {
      kind: "protected",
      latestSuccessAtMs: null,
      validPointCount: 1,
    },
  });
  return render(
    <QueryClientProvider client={client}>
      <App view={view} />
    </QueryClientProvider>,
  );
}

describe("P0 application routes", () => {
  it("renders the empty menu route", () => {
    renderWithData("menu");

    expect(
      screen.getByRole("main", { name: "AI Router 菜单" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("heading", { name: "还没有路由" }),
    ).toBeInTheDocument();
  });

  it("renders the empty settings route", () => {
    renderWithData("settings");

    expect(
      screen.getByRole("main", { name: "AI Router 设置" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("navigation", { name: "设置分区" }),
    ).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "路由" })).toBeInTheDocument();
  });
});
