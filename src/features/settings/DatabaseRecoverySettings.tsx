import { DatabaseBackup, Power, RefreshCw } from "lucide-react";
import { useState } from "react";
import { useQueryClient } from "@tanstack/react-query";

import {
  normalizeIpcError,
  quitApplication,
  restoreRecoveryPoint,
  retryDatabaseStartup,
  startOverDatabase,
} from "../../api/ipc";
import { queryKeys } from "../../api/query";
import { ipcErrorMessage, useLanguage } from "../language/useLanguage";
import type { SystemRecoveryMessages } from "../language/catalog/systemRecovery";
import type {
  AppLifecycleIssue,
  AppLocale,
  DatabaseStartupIssue,
  IpcErrorDto,
  RecoverySnapshotDto,
} from "../../generated";
import {
  SettingsActionGroup,
  SettingsButton,
  SettingsConfirmDialog,
  SettingsPage,
  SettingsSection,
  SettingsStatus,
} from "./SettingsPrimitives";

const retryableDatabaseIssues: Record<DatabaseStartupIssue, boolean> = {
  permission: true,
  disk_full: true,
  future_schema: false,
  unsafe_path: false,
  unavailable: true,
  directory_in_use: true,
};

function databaseIssuePresentation(
  issue: DatabaseStartupIssue,
  messages: SystemRecoveryMessages,
): { title: string; body: string; retryable: boolean } {
  const texts: Record<DatabaseStartupIssue, { title: string; body: string }> = {
    permission: {
      title: messages.databaseIssuePermissionTitle,
      body: messages.databaseIssuePermissionBody,
    },
    disk_full: {
      title: messages.databaseIssueDiskFullTitle,
      body: messages.databaseIssueDiskFullBody,
    },
    future_schema: {
      title: messages.databaseIssueFutureSchemaTitle,
      body: messages.databaseIssueFutureSchemaBody,
    },
    unsafe_path: {
      title: messages.databaseIssueUnsafePathTitle,
      body: messages.databaseIssueUnsafePathBody,
    },
    unavailable: {
      title: messages.databaseIssueUnavailableTitle,
      body: messages.databaseIssueUnavailableBody,
    },
    directory_in_use: {
      title: messages.databaseIssueDirectoryInUseTitle,
      body: messages.databaseIssueDirectoryInUseBody,
    },
  };
  return { ...texts[issue], retryable: retryableDatabaseIssues[issue] };
}

function lifecycleDatabaseIssue(
  issue: AppLifecycleIssue | null,
): DatabaseStartupIssue {
  return typeof issue === "object" && issue !== null
    ? issue.database
    : "unavailable";
}

export function RecoveryRequiredPage({
  snapshot,
  onRetry,
}: {
  snapshot: RecoverySnapshotDto;
  onRetry: () => Promise<unknown>;
}) {
  const { messages, effectiveLocale } = useLanguage();
  const systemMessages = messages.systemRecovery;
  const queryClient = useQueryClient();
  const [selectedPointId, setSelectedPointId] = useState<string | null>(
    snapshot.candidates[0]?.pointId ?? null,
  );
  const [confirmation, setConfirmation] = useState<
    "restore" | "start-over" | null
  >(null);
  const [pending, setPending] = useState<
    "restore" | "start-over" | "scan" | null
  >(null);
  const [error, setError] = useState<IpcErrorDto | null>(null);

  const effectiveSelectedPointId = snapshot.candidates.some(
    (candidate) => candidate.pointId === selectedPointId,
  )
    ? selectedPointId
    : (snapshot.candidates[0]?.pointId ?? null);

  const refreshState = async () => {
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: queryKeys.bootstrap }),
      queryClient.invalidateQueries({ queryKey: queryKeys.recovery }),
      queryClient.invalidateQueries({ queryKey: queryKeys.settings }),
      queryClient.invalidateQueries({ queryKey: queryKeys.menu }),
    ]);
  };

  const runRecovery = async (operation: "restore" | "start-over") => {
    if (pending !== null) return;
    setPending(operation);
    setError(null);
    try {
      if (operation === "restore") {
        if (!effectiveSelectedPointId) return;
        await restoreRecoveryPoint(effectiveSelectedPointId);
      } else {
        await startOverDatabase();
      }
      await refreshState();
    } catch (reason) {
      setError(normalizeIpcError(reason));
    } finally {
      setPending(null);
    }
  };

  const retryScan = async () => {
    if (pending !== null) return;
    setPending("scan");
    setError(null);
    try {
      await onRetry();
    } catch (reason) {
      setError(normalizeIpcError(reason));
    } finally {
      setPending(null);
    }
  };

  return (
    <SettingsPage
      title={systemMessages.recoveryPageTitle}
      titleId="recovery-title"
      className="recovery-page"
    >
      <SettingsSection
        title={systemMessages.recoveryRequiredSectionTitle}
        status={
          <SettingsStatus tone="danger">
            {systemMessages.proxyStoppedStatus}
          </SettingsStatus>
        }
      >
        <p className="muted-text">{systemMessages.recoveryRequiredBody}</p>
      </SettingsSection>

      {snapshot.candidates.length > 0 ? (
        <SettingsSection title={systemMessages.availableRecoveryPointsTitle}>
          <fieldset
            className="recovery-candidate-list"
            disabled={pending !== null}
          >
            <legend className="sr-only">
              {systemMessages.selectRecoveryPointLegend}
            </legend>
            {snapshot.candidates.map((candidate) => (
              <label className="recovery-candidate" key={candidate.pointId}>
                <input
                  type="radio"
                  name="recovery-point"
                  value={candidate.pointId}
                  checked={candidate.pointId === effectiveSelectedPointId}
                  onChange={() => setSelectedPointId(candidate.pointId)}
                />
                <span>
                  <strong>
                    {formatDateTime(candidate.createdAtMs, effectiveLocale)}
                  </strong>
                  <code>{candidate.pointId}</code>
                </span>
              </label>
            ))}
          </fieldset>
          <SettingsActionGroup>
            <SettingsButton
              variant="primary"
              type="button"
              disabled={pending !== null || effectiveSelectedPointId === null}
              onClick={() => setConfirmation("restore")}
            >
              <DatabaseBackup aria-hidden="true" size={16} />
              {pending === "restore"
                ? systemMessages.restoringButton
                : systemMessages.restoreSelectedButton}
            </SettingsButton>
          </SettingsActionGroup>
        </SettingsSection>
      ) : (
        <SettingsSection title={systemMessages.noAvailableRecoveryPointsTitle}>
          <p className="muted-text">
            {systemMessages.noAvailableRecoveryPointsBody}
          </p>
          {snapshot.canStartOver ? (
            <SettingsButton
              variant="danger"
              type="button"
              disabled={pending !== null}
              onClick={() => setConfirmation("start-over")}
            >
              {pending === "start-over"
                ? systemMessages.creatingStatus
                : systemMessages.createEmptyDatabaseButton}
            </SettingsButton>
          ) : null}
        </SettingsSection>
      )}

      <SettingsActionGroup className="recovery-page-actions">
        <SettingsButton
          type="button"
          disabled={pending !== null}
          onClick={() => void retryScan()}
        >
          <RefreshCw
            aria-hidden="true"
            size={16}
            className={pending === "scan" ? "spin" : ""}
          />
          {systemMessages.rescanButton}
        </SettingsButton>
        <SettingsButton
          type="button"
          disabled={pending !== null}
          onClick={() => void quitApplication()}
        >
          <Power aria-hidden="true" size={16} />
          {systemMessages.quitButton}
        </SettingsButton>
      </SettingsActionGroup>

      {error ? (
        <p className="settings-error" role="alert">
          {ipcErrorMessage(error, messages)}
        </p>
      ) : null}

      {confirmation === "restore" && effectiveSelectedPointId ? (
        <SettingsConfirmDialog
          confirmation={{
            title: systemMessages.restoreConfirmTitle,
            body: systemMessages.restoreConfirmBody(
              formatDateTime(
                snapshot.candidates.find(
                  (candidate) => candidate.pointId === effectiveSelectedPointId,
                )?.createdAtMs ?? null,
                effectiveLocale,
              ),
            ),
            confirmLabel: systemMessages.restoreConfirmLabel,
            onConfirm: () => {
              setConfirmation(null);
              void runRecovery("restore");
            },
          }}
          onCancel={() => setConfirmation(null)}
        />
      ) : null}

      {confirmation === "start-over" ? (
        <SettingsConfirmDialog
          confirmation={{
            title: systemMessages.startOverConfirmTitle,
            body: systemMessages.startOverConfirmBody,
            confirmLabel: systemMessages.startOverConfirmLabel,
            destructive: true,
            onConfirm: () => {
              setConfirmation(null);
              void runRecovery("start-over");
            },
          }}
          onCancel={() => setConfirmation(null)}
        />
      ) : null}
    </SettingsPage>
  );
}

export function RecoveryLoadErrorPage({
  onRetry,
}: {
  onRetry: () => Promise<unknown>;
}) {
  const { messages } = useLanguage();
  const systemMessages = messages.systemRecovery;
  const [pending, setPending] = useState(false);
  return (
    <SettingsPage
      title={systemMessages.recoveryPageTitle}
      titleId="recovery-load-error-title"
    >
      <SettingsSection
        title={systemMessages.recoveryLoadFailedTitle}
        status={
          <SettingsStatus tone="danger">
            {systemMessages.proxyStoppedStatus}
          </SettingsStatus>
        }
      >
        <p className="muted-text">{systemMessages.recoveryLoadFailedBody}</p>
        <SettingsActionGroup>
          <SettingsButton
            type="button"
            disabled={pending}
            onClick={() => {
              setPending(true);
              void onRetry().finally(() => setPending(false));
            }}
          >
            <RefreshCw
              aria-hidden="true"
              size={16}
              className={pending ? "spin" : ""}
            />
            {systemMessages.retryButton}
          </SettingsButton>
          <SettingsButton
            type="button"
            disabled={pending}
            onClick={() => void quitApplication()}
          >
            <Power aria-hidden="true" size={16} />
            {systemMessages.quitButton}
          </SettingsButton>
        </SettingsActionGroup>
      </SettingsSection>
    </SettingsPage>
  );
}

export function DatabaseStartupErrorPage({
  issue,
  lifecycleIssue,
}: {
  issue: DatabaseStartupIssue | null;
  lifecycleIssue: AppLifecycleIssue | null;
}) {
  const { messages } = useLanguage();
  const systemMessages = messages.systemRecovery;
  const presentation = databaseIssuePresentation(
    issue ?? lifecycleDatabaseIssue(lifecycleIssue),
    systemMessages,
  );
  const queryClient = useQueryClient();
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<IpcErrorDto | null>(null);
  const retry = async () => {
    if (pending) return;
    setPending(true);
    setError(null);
    try {
      await retryDatabaseStartup();
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: queryKeys.bootstrap }),
        queryClient.invalidateQueries({ queryKey: queryKeys.recovery }),
        queryClient.invalidateQueries({ queryKey: queryKeys.settings }),
      ]);
    } catch (reason) {
      setError(normalizeIpcError(reason));
    } finally {
      setPending(false);
    }
  };
  return (
    <SettingsPage title={presentation.title} titleId="database-error-title">
      <SettingsSection
        title={systemMessages.databaseStartupFailedTitle}
        status={
          <SettingsStatus tone="danger">
            {systemMessages.proxyStoppedStatus}
          </SettingsStatus>
        }
      >
        <p className="muted-text">{presentation.body}</p>
        <SettingsActionGroup>
          {presentation.retryable ? (
            <SettingsButton
              type="button"
              disabled={pending}
              onClick={() => void retry()}
            >
              <RefreshCw
                aria-hidden="true"
                size={16}
                className={pending ? "spin" : ""}
              />
              {pending
                ? systemMessages.retryingDatabaseStartup
                : systemMessages.retryDatabaseStartupButton}
            </SettingsButton>
          ) : null}
          <SettingsButton
            type="button"
            disabled={pending}
            onClick={() => void quitApplication()}
          >
            <Power aria-hidden="true" size={16} />
            {systemMessages.quitButton}
          </SettingsButton>
        </SettingsActionGroup>
      </SettingsSection>
      {error ? (
        <p className="settings-error" role="alert">
          {ipcErrorMessage(error, messages)}
        </p>
      ) : null}
    </SettingsPage>
  );
}

function formatDateTime(value: number | null, locale: AppLocale) {
  return value === null
    ? "-"
    : new Intl.DateTimeFormat(locale, {
        dateStyle: "medium",
        timeStyle: "short",
      }).format(value);
}
