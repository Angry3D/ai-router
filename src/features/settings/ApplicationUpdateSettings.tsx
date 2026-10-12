import { Download, ExternalLink, LoaderCircle, RefreshCw } from "lucide-react";
import { useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";

import {
  checkApplicationUpdate,
  downloadAndInstallApplicationUpdate,
  normalizeIpcError,
  openApplicationUpdateRelease,
  restartForApplicationUpdate,
} from "../../api/ipc";
import { queryKeys } from "../../api/query";
import type {
  ApplicationUpdateFailureDto,
  ApplicationUpdateProgressDto,
  ApplicationUpdateSnapshotDto,
  IpcErrorDto,
} from "../../generated";
import type { UpdatesMessages } from "../language/catalog/updates";
import { ipcErrorMessage, useLanguage } from "../language/useLanguage";
import { formatDateTime } from "./codexSettingsFormatting";
import {
  SettingsActionGroup,
  SettingsButton,
  SettingsConfirmDialog,
  SettingsReadonlyRow,
  SettingsSection,
  SettingsStatus,
  type SettingsConfirmation,
  type SettingsTone,
} from "./SettingsPrimitives";

export function ApplicationUpdateSettings({
  snapshot,
}: {
  snapshot: ApplicationUpdateSnapshotDto | null;
}) {
  const queryClient = useQueryClient();
  const { messages, effectiveLocale } = useLanguage();
  const updatesMessages = messages.updates;
  const [localResult, setLocalResult] = useState<{
    source: ApplicationUpdateSnapshotDto | null;
    snapshot: ApplicationUpdateSnapshotDto;
  } | null>(null);
  const [progress, setProgress] = useState<ApplicationUpdateProgressDto | null>(
    null,
  );
  const [checkPending, setCheckPending] = useState(false);
  const [downloadPending, setDownloadPending] = useState(false);
  const [confirmation, setConfirmation] = useState<SettingsConfirmation | null>(
    null,
  );
  const [actionError, setActionError] = useState<IpcErrorDto | null>(null);
  const [expandedNotesVersion, setExpandedNotesVersion] = useState<
    string | null
  >(null);
  const downloadGeneration = useRef(0);

  const current =
    (localResult?.source === snapshot ? localResult.snapshot : snapshot) ??
    emptyUpdateSnapshot;
  const effectiveProgress = (downloadPending ? progress : null) ?? {
    operation: current.operation,
    downloadedBytes: current.downloadedBytes,
    totalBytes: current.totalBytes,
  };
  const effectiveOperation = checkPending
    ? "checking"
    : downloadPending
      ? (progress?.operation ?? "downloading")
      : current.operation;
  const presentation = updatePresentation(
    {
      ...current,
      operation: effectiveOperation,
    },
    updatesMessages,
  );
  const busy =
    checkPending ||
    downloadPending ||
    effectiveOperation === "checking" ||
    effectiveOperation === "downloading" ||
    effectiveOperation === "installing";

  const notesExpanded = expandedNotesVersion === current.available?.version;

  const applySnapshot = (next: ApplicationUpdateSnapshotDto) => {
    setLocalResult({ source: snapshot, snapshot: next });
    queryClient.setQueryData(queryKeys.applicationUpdate, next);
  };

  const check = async () => {
    setActionError(null);
    setCheckPending(true);
    try {
      applySnapshot(await checkApplicationUpdate());
    } catch (reason) {
      setActionError(normalizeIpcError(reason));
      await queryClient.invalidateQueries({
        queryKey: queryKeys.applicationUpdate,
      });
    } finally {
      setCheckPending(false);
    }
  };

  const download = async () => {
    const generation = downloadGeneration.current + 1;
    downloadGeneration.current = generation;
    setConfirmation(null);
    setActionError(null);
    setDownloadPending(true);
    setProgress({
      operation: "downloading",
      downloadedBytes: current.downloadedBytes ?? 0,
      totalBytes: current.totalBytes,
    });
    try {
      const next = await downloadAndInstallApplicationUpdate((nextProgress) => {
        if (downloadGeneration.current === generation) {
          setProgress(nextProgress);
        }
      });
      if (downloadGeneration.current === generation) {
        applySnapshot(next);
      }
    } catch (reason) {
      if (downloadGeneration.current === generation) {
        setActionError(normalizeIpcError(reason));
        await queryClient.invalidateQueries({
          queryKey: queryKeys.applicationUpdate,
        });
      }
    } finally {
      if (downloadGeneration.current === generation) {
        setDownloadPending(false);
        setProgress(null);
      }
    }
  };

  const openRelease = async () => {
    setActionError(null);
    try {
      await openApplicationUpdateRelease();
    } catch (reason) {
      setActionError(normalizeIpcError(reason));
    }
  };

  const restart = async () => {
    setConfirmation(null);
    setActionError(null);
    try {
      await restartForApplicationUpdate();
    } catch (reason) {
      setActionError(normalizeIpcError(reason));
    }
  };

  const confirmDownload = () => {
    setConfirmation({
      title: updatesMessages.confirmDownloadTitle,
      body: updatesMessages.confirmDownloadBody,
      confirmLabel: updatesMessages.confirmDownloadConfirm,
      onConfirm: () => void download(),
    });
  };

  const confirmRestart = () => {
    setConfirmation({
      title: updatesMessages.confirmRestartTitle,
      body: updatesMessages.confirmRestartBody,
      confirmLabel: updatesMessages.confirmRestartConfirm,
      cancelLabel: updatesMessages.confirmRestartCancel,
      onConfirm: () => void restart(),
    });
  };

  return (
    <>
      <SettingsSection
        title={updatesMessages.sectionTitle}
        status={
          <SettingsStatus tone={presentation.tone} aria-live="polite">
            {presentation.label}
          </SettingsStatus>
        }
      >
        <SettingsReadonlyRow label={updatesMessages.currentVersionLabel}>
          {current.currentVersion || "—"}
        </SettingsReadonlyRow>
        {current.lastSuccessfulCheckAtMs !== null ? (
          <SettingsReadonlyRow label={updatesMessages.lastCheckedLabel}>
            {formatDateTime(current.lastSuccessfulCheckAtMs, effectiveLocale)}
          </SettingsReadonlyRow>
        ) : null}
        {current.available ? (
          <SettingsReadonlyRow label={updatesMessages.availableVersionLabel}>
            {current.available.version}
          </SettingsReadonlyRow>
        ) : null}

        {current.available?.notes ? (
          <div className="application-update-notes">
            <span className="settings-field-label">
              {updatesMessages.notesHeading}
            </span>
            <div className="application-update-notes-content">
              <div id="application-update-release-details">
                {notesExpanded ? (
                  <ApplicationUpdateNoteGroups
                    highlights={current.available.notes.highlights}
                    fixes={current.available.notes.fixes}
                    notices={current.available.notes.notices}
                    highlightsHeading={updatesMessages.notesHighlightsHeading}
                    fixesHeading={updatesMessages.notesFixesHeading}
                    noticesHeading={updatesMessages.notesNoticesHeading}
                  />
                ) : (
                  <ul className="application-update-notes-summary">
                    {current.available.notes.highlights.map((item) => (
                      <li key={item}>{item}</li>
                    ))}
                  </ul>
                )}
              </div>
              {releaseNoteItemCount(current.available.notes) >
              current.available.notes.highlights.length ? (
                <button
                  type="button"
                  className="application-update-notes-toggle"
                  aria-expanded={notesExpanded}
                  aria-controls="application-update-release-details"
                  onClick={() =>
                    setExpandedNotesVersion(
                      notesExpanded
                        ? null
                        : (current.available?.version ?? null),
                    )
                  }
                >
                  {notesExpanded
                    ? updatesMessages.notesCollapse
                    : updatesMessages.notesViewAll(
                        releaseNoteItemCount(current.available.notes),
                      )}
                </button>
              ) : null}
            </div>
          </div>
        ) : current.available ? (
          <div className="application-update-notes application-update-notes-legacy">
            <span className="settings-field-label">
              {updatesMessages.releaseNotesHeading}
            </span>
            <div className="application-update-notes-content">
              {current.available.legacyNotes ? (
                <>
                  <p
                    id="application-update-legacy-notes"
                    className={notesExpanded ? "is-expanded" : ""}
                  >
                    {current.available.legacyNotes}
                  </p>
                  {legacyNotesAreExpandable(current.available.legacyNotes) ? (
                    <button
                      type="button"
                      className="application-update-notes-toggle"
                      aria-expanded={notesExpanded}
                      aria-controls="application-update-legacy-notes"
                      onClick={() =>
                        setExpandedNotesVersion(
                          notesExpanded
                            ? null
                            : (current.available?.version ?? null),
                        )
                      }
                    >
                      {notesExpanded
                        ? updatesMessages.releaseNotesCollapse
                        : updatesMessages.releaseNotesExpand}
                    </button>
                  ) : null}
                </>
              ) : null}
              <p className="application-update-notes-fallback">
                {current.available.legacyNotes
                  ? updatesMessages.releaseNotesLegacyHint
                  : updatesMessages.releaseNotesFallback}
              </p>
            </div>
          </div>
        ) : null}

        {effectiveOperation === "downloading" ||
        effectiveOperation === "installing" ? (
          <UpdateProgress
            progress={effectiveProgress}
            updatesMessages={updatesMessages}
          />
        ) : null}

        {current.manualFailure ? (
          <p className="application-update-error" role="alert">
            {ipcErrorMessage(
              updateFailureError(current.manualFailure),
              messages,
            )}
          </p>
        ) : null}
        {actionError ? (
          <p className="application-update-error" role="alert">
            {ipcErrorMessage(actionError, messages)}
          </p>
        ) : null}

        <SettingsActionGroup className="application-update-actions">
          {effectiveOperation === "restart_ready" ? (
            <SettingsButton
              type="button"
              variant="primary"
              onClick={confirmRestart}
            >
              <RefreshCw aria-hidden="true" size={15} />
              {updatesMessages.restartButton}
            </SettingsButton>
          ) : current.available &&
            !busy &&
            current.manualFailure?.retryable !== false ? (
            <SettingsButton
              type="button"
              variant="primary"
              onClick={confirmDownload}
            >
              <Download aria-hidden="true" size={15} />
              {current.manualFailure
                ? updatesMessages.retryDownloadButton
                : updatesMessages.downloadButton}
            </SettingsButton>
          ) : !current.available && effectiveOperation !== "installing" ? (
            <SettingsButton type="button" disabled={busy} onClick={check}>
              {effectiveOperation === "checking" ? (
                <LoaderCircle aria-hidden="true" className="spin" size={15} />
              ) : (
                <RefreshCw aria-hidden="true" size={15} />
              )}
              {effectiveOperation === "checking"
                ? updatesMessages.checkingButton
                : current.lastSuccessfulCheckAtMs === null
                  ? updatesMessages.checkButton
                  : updatesMessages.recheckButton}
            </SettingsButton>
          ) : null}
          {current.available || current.manualFailure || actionError ? (
            <SettingsButton type="button" onClick={() => void openRelease()}>
              <ExternalLink aria-hidden="true" size={15} />
              {updatesMessages.viewReleaseButton}
            </SettingsButton>
          ) : null}
        </SettingsActionGroup>
      </SettingsSection>
      {confirmation ? (
        <SettingsConfirmDialog
          confirmation={confirmation}
          onCancel={() => setConfirmation(null)}
        />
      ) : null}
    </>
  );
}

type ReleaseNoteGroupsProps = {
  highlights: string[];
  fixes: string[];
  notices: string[];
};

function ApplicationUpdateNoteGroups({
  highlights,
  fixes,
  notices,
  highlightsHeading,
  fixesHeading,
  noticesHeading,
}: ReleaseNoteGroupsProps & {
  highlightsHeading: string;
  fixesHeading: string;
  noticesHeading: string;
}) {
  return (
    <div className="application-update-note-groups">
      <ApplicationUpdateNoteGroup
        heading={highlightsHeading}
        items={highlights}
      />
      <ApplicationUpdateNoteGroup heading={fixesHeading} items={fixes} />
      <ApplicationUpdateNoteGroup heading={noticesHeading} items={notices} />
    </div>
  );
}

function ApplicationUpdateNoteGroup({
  heading,
  items,
}: {
  heading: string;
  items: string[];
}) {
  if (items.length === 0) return null;
  return (
    <section>
      <h4>{heading}</h4>
      <ul>
        {items.map((item) => (
          <li key={item}>{item}</li>
        ))}
      </ul>
    </section>
  );
}

function releaseNoteItemCount(notes: ReleaseNoteGroupsProps) {
  return notes.highlights.length + notes.fixes.length + notes.notices.length;
}

function legacyNotesAreExpandable(notes: string) {
  return notes.split("\n").length > 6 || notes.length > 360;
}

function UpdateProgress({
  progress,
  updatesMessages,
}: {
  progress: ApplicationUpdateProgressDto;
  updatesMessages: UpdatesMessages;
}) {
  const determinate =
    progress.operation === "downloading" &&
    progress.downloadedBytes !== null &&
    progress.totalBytes !== null &&
    progress.totalBytes > 0;
  const percentage = determinate
    ? Math.min(
        100,
        Math.floor(
          ((progress.downloadedBytes ?? 0) / (progress.totalBytes ?? 1)) * 100,
        ),
      )
    : null;
  return (
    <div className="application-update-progress" aria-live="polite">
      <div
        className={determinate ? "" : "is-indeterminate"}
        role="progressbar"
        aria-label={
          progress.operation === "installing"
            ? updatesMessages.progressInstalling
            : updatesMessages.progressDownloadLabel
        }
        aria-valuemin={determinate ? 0 : undefined}
        aria-valuemax={determinate ? 100 : undefined}
        aria-valuenow={percentage ?? undefined}
      >
        <span
          style={{ width: percentage === null ? undefined : `${percentage}%` }}
        />
      </div>
      <p>
        {progress.operation === "installing"
          ? updatesMessages.progressInstalling
          : percentage === null
            ? updatesMessages.progressDownloading
            : updatesMessages.progressDetail(
                percentage,
                formatBytes(progress.downloadedBytes ?? 0),
                formatBytes(progress.totalBytes ?? 0),
              )}
      </p>
    </div>
  );
}

const emptyUpdateSnapshot: ApplicationUpdateSnapshotDto = {
  currentVersion: "",
  operation: "idle",
  available: null,
  lastSuccessfulCheckAtMs: null,
  downloadedBytes: null,
  totalBytes: null,
  manualFailure: null,
};

function updatePresentation(
  snapshot: ApplicationUpdateSnapshotDto,
  updatesMessages: UpdatesMessages,
): {
  label: string;
  tone: SettingsTone;
} {
  if (snapshot.operation === "checking")
    return { label: updatesMessages.statusChecking, tone: "neutral" };
  if (snapshot.operation === "downloading")
    return { label: updatesMessages.statusDownloading, tone: "neutral" };
  if (snapshot.operation === "installing")
    return { label: updatesMessages.statusInstalling, tone: "neutral" };
  if (snapshot.operation === "restart_ready")
    return { label: updatesMessages.statusRestartReady, tone: "success" };
  if (snapshot.available)
    return { label: updatesMessages.statusAvailable, tone: "warning" };
  if (snapshot.manualFailure)
    return { label: updatesMessages.statusCheckFailed, tone: "danger" };
  if (snapshot.lastSuccessfulCheckAtMs !== null)
    return { label: updatesMessages.statusUpToDate, tone: "success" };
  return { label: updatesMessages.statusNotChecked, tone: "neutral" };
}

function updateFailureError(failure: ApplicationUpdateFailureDto): IpcErrorDto {
  return { ...failure, field: null };
}

function formatBytes(bytes: number) {
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}
