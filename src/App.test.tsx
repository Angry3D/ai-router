import { QueryClientProvider } from "@tanstack/react-query";
import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { App } from "./App";
import { createRouterQueryClient, queryKeys } from "./api/query";
import type {
  BootstrapSnapshotDto,
  MenuSnapshotDto,
  SettingsSnapshotDto,
} from "./generated";
import { LanguageProvider } from "./features/language/LanguageProvider";

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
  language: { preference: "system" as const, effectiveLocale: "en" as const },
};

function renderWithData(view: "menu" | "settings") {
  const client = createRouterQueryClient();
  client.setQueryData<BootstrapSnapshotDto>(
    queryKeys.bootstrap,
    bootstrap as BootstrapSnapshotDto,
  );
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
    outboundProxy: { enabled: false, url: null },
    codexAuth: {
      credentialPresent: false,
      storeMode: "file",
      storeModeSupported: true,
      managedLocked: false,
      accountEmail: null,
      planType: null,
      expiresAtMs: null,
      exportedAtMs: null,
      backupAvailable: false,
      drifted: false,
      expired: false,
    },
    menuBar: { statusTextEnabled: true, activityAnimationEnabled: true },
    imagesGeneration: {
      enabled: false,
      routeId: null,
      timeoutSecs: 600,
      model: "gpt-image-2",
    },
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
      lastIncident: null,
    },
  });
  return render(
    <QueryClientProvider client={client}>
      <LanguageProvider view={view}>
        <App view={view} />
      </LanguageProvider>
    </QueryClientProvider>,
  );
}

describe("P0 application routes", () => {
  // The bootstrap fixture is committed as `en`, so these assertions only pass
  // when the real provider boundary drives the application surfaces.
  it("renders the empty menu route in the committed locale", () => {
    renderWithData("menu");

    expect(
      screen.getByRole("main", { name: "AI Router menu" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("heading", { name: "No routes" }),
    ).toBeInTheDocument();
    expect(screen.getByText("Proxy on")).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Codex off, Select route" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Fallback N/A, Need 2 routes" }),
    ).toBeInTheDocument();
  });

  it("renders the empty settings route in the committed locale", () => {
    renderWithData("settings");

    expect(
      screen.getByRole("main", { name: "AI Router Settings" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("navigation", { name: "Settings sections" }),
    ).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Routes" })).toBeInTheDocument();
  });
});
