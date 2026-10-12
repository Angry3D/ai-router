import { FolderOpen, LoaderCircle } from "lucide-react";
import { useState } from "react";
import { useQueryClient } from "@tanstack/react-query";

import {
  applyProxyPort,
  confirmCodexImagesMcpRepair,
  confirmResetCodexRecoveryToBaseline,
  confirmUpdateCodexRecovery,
  connectCodex,
  normalizeIpcError,
  openCodexConfig,
  previewCodexImagesMcpRepair,
  previewResetCodexRecoveryToBaseline,
  previewUpdateCodexRecovery,
  reconnectCodex,
} from "../../api/ipc";
import { queryKeys } from "../../api/query";
import type {
  CodexConfigStatus,
  IpcErrorDto,
  PricingTableDto,
  SettingsSnapshotDto,
} from "../../generated";
import { ipcErrorMessage, useLanguage } from "../language/useLanguage";
import { PricingTableSettings } from "./PricingTableSettings";
import { CodexAuthSettingsSection } from "./CodexAuthSettingsSection";
import { ImageGenerationSettingsSection } from "./ImageGenerationSettingsSection";
import { formatDateTime } from "./codexSettingsFormatting";
import {
  SettingsActionGroup,
  SettingsButton,
  SettingsConfirmDialog,
  SettingsFieldRow,
  SettingsPage,
  SettingsReadonlyRow,
  SettingsSection,
  SettingsStatus,
  SettingsTextInput,
  type SettingsConfirmation,
} from "./SettingsPrimitives";

export function CodexSettings({
  snapshot,
  proxyStatus,
  pricingTable = null,
  focusImageGeneration = false,
  onImageGenerationFocused,
}: {
  snapshot: SettingsSnapshotDto;
  proxyStatus: string;
  pricingTable?: PricingTableDto | null;
  focusImageGeneration?: boolean;
  onImageGenerationFocused?: () => void;
}) {
  const { messages, effectiveLocale } = useLanguage();
  const codex = messages.codex;
  const codexLabels: Record<CodexConfigStatus, string> = {
    checking: codex.statusChecking,
    connected: codex.statusConnected,
    not_connected: codex.statusNotConnected,
    changed: codex.statusChanged,
    images_mcp_name_conflict: codex.statusImagesMcpNameConflict,
    images_mcp_projection_conflict: codex.statusImagesMcpProjectionConflict,
    invalid: codex.statusInvalid,
    unreadable: codex.statusUnreadable,
    symlink_unsupported: codex.statusSymlinkUnsupported,
  };
  const queryClient = useQueryClient();
  const [port, setPort] = useState(String(snapshot.proxyPort));
  const [busyOperation, setBusyOperation] = useState<
    | "default"
    | "images_mcp_repair"
    | "recovery_update_preview"
    | "recovery_update"
    | "recovery_reset_preview"
    | "recovery_reset"
    | null
  >(null);
  const [error, setError] = useState<IpcErrorDto | null>(null);
  const [recoveryError, setRecoveryError] = useState<IpcErrorDto | null>(null);
  const [confirmation, setConfirmation] = useState<SettingsConfirmation | null>(
    null,
  );
  const refresh = () =>
    queryClient.invalidateQueries({ queryKey: queryKeys.settings });
  const busy = busyOperation !== null;
  const run = async (
    operation: () => Promise<unknown>,
    operationKind:
      | "default"
      | "images_mcp_repair"
      | "recovery_update"
      | "recovery_reset" = "default",
  ) => {
    setBusyOperation(operationKind);
    if (operationKind.startsWith("recovery_")) setRecoveryError(null);
    else setError(null);
    try {
      await operation();
      await refresh();
    } catch (reason) {
      const failure = normalizeIpcError(reason);
      if (operationKind.startsWith("recovery_")) setRecoveryError(failure);
      else setError(failure);
    } finally {
      setBusyOperation(null);
    }
  };
  const connect = () => {
    if (!snapshot.activeRouteId) {
      setConfirmation({
        title: codex.connectNoRouteTitle,
        body: codex.connectNoRouteBody,
        confirmLabel: codex.connectNoRouteConfirm,
        onConfirm: () => {
          setConfirmation(null);
          void run(() => connectCodex(true));
        },
      });
    } else void run(() => connectCodex(false));
  };
  const restore = () => void previewRecoveryReset();
  const previewImagesMcpRepair = async () => {
    if (busy) return;
    setBusyOperation("images_mcp_repair");
    setError(null);
    try {
      const preview = await previewCodexImagesMcpRepair();
      setConfirmation({
        title: codex.imagesRepairConfirmTitle,
        body: codex.imagesRepairConfirmBody,
        confirmLabel: codex.imagesRepairConfirmLabel,
        destructive: true,
        onConfirm: () => {
          setConfirmation(null);
          void run(
            () => confirmCodexImagesMcpRepair(preview.permit),
            "images_mcp_repair",
          );
        },
      });
    } catch (reason) {
      setError(normalizeIpcError(reason));
    } finally {
      setBusyOperation(null);
    }
  };
  const previewRecoveryUpdate = async () => {
    if (busy || snapshot.codexStatus !== "not_connected") return;
    setBusyOperation("recovery_update_preview");
    setRecoveryError(null);
    try {
      const preview = await previewUpdateCodexRecovery();
      setConfirmation({
        title: codex.recoveryUpdateConfirmTitle,
        body: codex.recoveryUpdateConfirmBody,
        details: (
          <SnapshotSummary
            rows={[
              [
                codex.detailCurrentFile,
                preview.currentExists ? codex.valueExists : codex.valueNotExists,
              ],
              [
                codex.detailComparedToRecovery,
                preview.bytesChanged ? codex.valueChanged : codex.valueUnchanged,
              ],
              [codex.detailFileMode, formatUnixMode(preview.currentUnixMode)],
              [
                codex.detailExistingRecoveryTarget,
                preview.recoveryTargetExists
                  ? codex.recoveryTargetExists
                  : codex.recoveryTargetDelete,
              ],
              [
                codex.detailRecoveryUpdatedAt,
                formatDateTime(preview.recoveryUpdatedAtMs, effectiveLocale),
              ],
            ]}
          />
        ),
        confirmLabel: codex.recoveryUpdateConfirmLabel,
        onConfirm: () => {
          setConfirmation(null);
          void run(
            () => confirmUpdateCodexRecovery(preview.permit),
            "recovery_update",
          );
        },
      });
    } catch (reason) {
      setRecoveryError(normalizeIpcError(reason));
    } finally {
      setBusyOperation(null);
    }
  };
  const previewRecoveryReset = async () => {
    if (busy || snapshot.codexStatus !== "not_connected") return;
    setBusyOperation("recovery_reset_preview");
    setRecoveryError(null);
    try {
      const preview = await previewResetCodexRecoveryToBaseline();
      setConfirmation({
        title: codex.recoveryResetConfirmTitle,
        body: codex.recoveryResetConfirmBody,
        details: (
          <SnapshotSummary
            rows={[
              [
                codex.detailCurrentFile,
                preview.currentExists ? codex.valueExists : codex.valueNotExists,
              ],
              [
                codex.detailOriginalFile,
                preview.originalExists
                  ? codex.valueExists
                  : codex.valueNotExists,
              ],
              [
                codex.detailRecoveryTarget,
                preview.recoveryTargetExists
                  ? codex.recoveryTargetExists
                  : codex.recoveryTargetDelete,
              ],
            ]}
          />
        ),
        confirmLabel: codex.recoveryResetConfirmLabel,
        destructive: true,
        onConfirm: () => {
          setConfirmation(null);
          void run(
            () => confirmResetCodexRecoveryToBaseline(preview.permit),
            "recovery_reset",
          );
        },
      });
    } catch (reason) {
      setRecoveryError(normalizeIpcError(reason));
    } finally {
      setBusyOperation(null);
    }
  };
  const imageMcpNameConflict =
    snapshot.codexStatus === "images_mcp_name_conflict";
  const imageMcpProjectionConflict =
    snapshot.codexStatus === "images_mcp_projection_conflict";
  const imageMcpConflict = imageMcpNameConflict || imageMcpProjectionConflict;
  const disconnected = snapshot.codexStatus === "not_connected";
  const hasOriginalBackup = snapshot.originalBackup.exists;
  const hasRecoveryConfig = snapshot.recoveryConfig.exists;
  const recoveryStatus = !hasOriginalBackup
    ? { label: codex.recoveryStatusUnavailable, tone: "warning" as const }
    : hasRecoveryConfig
      ? { label: codex.recoveryStatusAvailable, tone: "success" as const }
      : { label: codex.recoveryNotCreated, tone: "warning" as const };
  return (
    <SettingsPage title="Codex" titleId="codex-title">
      <SettingsSection
        title={codex.proxySectionTitle}
        status={
          <SettingsStatus
            tone={proxyStatus === "running" ? "success" : "danger"}
          >
            {proxyStatus === "running"
              ? codex.proxyStatusRunning
              : proxyStatus === "port_conflict"
                ? codex.proxyStatusPortConflict
                : codex.proxyStatusUnavailable}
          </SettingsStatus>
        }
      >
        <SettingsReadonlyRow label={codex.proxyAddressLabel}>
          127.0.0.1:{snapshot.proxyPort}
        </SettingsReadonlyRow>
        <SettingsFieldRow
          label={codex.proxyPortLabel}
          htmlFor="proxy-port"
          className="settings-field-row-with-action"
        >
          <SettingsTextInput
            id="proxy-port"
            type="number"
            min={1}
            max={65535}
            value={port}
            onChange={(event) => setPort(event.target.value)}
          />
          <SettingsButton
            type="button"
            disabled={busy || Number(port) === snapshot.proxyPort}
            onClick={() => void run(() => applyProxyPort(Number(port)))}
          >
            {codex.proxyPortApply}
          </SettingsButton>
        </SettingsFieldRow>
      </SettingsSection>
      <ImageGenerationSettingsSection
        key={imageSettingsKey(snapshot)}
        snapshot={snapshot}
        focusRequested={focusImageGeneration}
        onFocusConsumed={onImageGenerationFocused}
      />
      <SettingsSection
        title={codex.configSectionTitle}
        status={
          <SettingsStatus
            tone={
              snapshot.codexStatus === "connected"
                ? "success"
                : imageMcpConflict
                  ? "danger"
                  : "warning"
            }
          >
            {codexLabels[snapshot.codexStatus]}
          </SettingsStatus>
        }
      >
        <SettingsReadonlyRow label={codex.configFileLabel}>
          ~/.codex/config.toml
        </SettingsReadonlyRow>
        <SettingsActionGroup>
          {imageMcpProjectionConflict ? (
            <SettingsButton
              variant="primary"
              type="button"
              disabled={busy}
              onClick={() => void previewImagesMcpRepair()}
            >
              {busyOperation === "images_mcp_repair" ? (
                <LoaderCircle aria-hidden="true" className="spin" size={15} />
              ) : null}
              {codex.imagesRepairAction}
            </SettingsButton>
          ) : null}
          {snapshot.codexStatus === "not_connected" ? (
            <SettingsButton
              variant="primary"
              type="button"
              disabled={busy}
              onClick={connect}
            >
              {codex.connectAction}
            </SettingsButton>
          ) : null}
          {snapshot.codexStatus === "changed" ? (
            <SettingsButton
              variant="primary"
              type="button"
              disabled={busy}
              onClick={() => void run(reconnectCodex)}
            >
              {codex.reconnectAction}
            </SettingsButton>
          ) : null}
          <SettingsButton
            type="button"
            disabled={busy}
            onClick={() => void run(openCodexConfig)}
          >
            <FolderOpen aria-hidden="true" size={16} />
            {codex.openConfigAction}
          </SettingsButton>
        </SettingsActionGroup>
        {imageMcpProjectionConflict ? (
          <p className="settings-error codex-config-conflict-message">
            {codex.imagesProjectionConflictMessage}
          </p>
        ) : null}
        {imageMcpNameConflict ? (
          <p className="settings-error codex-config-conflict-message">
            {codex.imagesNameConflictMessage}
          </p>
        ) : null}
      </SettingsSection>
      <CodexAuthSettingsSection snapshot={snapshot} />
      <SettingsSection
        title={codex.recoverySectionTitle}
        status={
          <SettingsStatus tone={recoveryStatus.tone}>
            {recoveryStatus.label}
          </SettingsStatus>
        }
      >
        <SettingsReadonlyRow label={codex.recoveryTargetLabel}>
          {!hasRecoveryConfig
            ? codex.recoveryNotCreated
            : snapshot.recoveryConfig.originalExists
              ? codex.recoveryTargetExists
              : codex.recoveryTargetDelete}
        </SettingsReadonlyRow>
        <SettingsReadonlyRow label={codex.recoveryUpdatedAtLabel}>
          {hasRecoveryConfig
            ? formatDateTime(
                snapshot.recoveryConfig.updatedAtMs,
                effectiveLocale,
              )
            : "-"}
        </SettingsReadonlyRow>
        <SettingsActionGroup className="codex-recovery-actions">
          <SettingsButton
            variant="primary"
            type="button"
            disabled={busy || !disconnected || !hasOriginalBackup}
            onClick={() => void previewRecoveryUpdate()}
          >
            {busyOperation === "recovery_update_preview" ||
            busyOperation === "recovery_update" ? (
              <LoaderCircle aria-hidden="true" className="spin" size={15} />
            ) : null}
            {codex.recoveryUpdateAction}
          </SettingsButton>
        </SettingsActionGroup>
        <p className="muted-text codex-recovery-hint">
          {!hasOriginalBackup
            ? codex.recoveryHintNoBackup
            : !disconnected
              ? codex.recoveryHintNotDisconnected
              : codex.recoveryHintReady}
        </p>
        <div className="codex-recovery-advanced">
          <div>
            <strong>{codex.originalBackupTitle}</strong>
            <span>
              {hasOriginalBackup
                ? codex.originalBackupSummary(
                    formatDateTime(
                      snapshot.originalBackup.capturedAtMs,
                      effectiveLocale,
                    ),
                  )
                : codex.originalBackupMissing}
            </span>
          </div>
          <SettingsButton
            variant="danger"
            type="button"
            disabled={busy || !disconnected || !hasOriginalBackup}
            onClick={restore}
          >
            {codex.recoveryResetAction}
          </SettingsButton>
        </div>
        {recoveryError ? (
          <p className="settings-error codex-recovery-error" role="alert">
            {ipcErrorMessage(recoveryError, messages)}
          </p>
        ) : null}
      </SettingsSection>
      <PricingTableSettings snapshot={pricingTable} />
      {error ? (
        <p className="settings-error" role="alert">
          {ipcErrorMessage(error, messages)}
        </p>
      ) : null}
      {confirmation ? (
        <SettingsConfirmDialog
          confirmation={confirmation}
          onCancel={() => setConfirmation(null)}
        />
      ) : null}
    </SettingsPage>
  );
}

function imageSettingsKey(snapshot: SettingsSnapshotDto) {
  const routeIds = snapshot.routes.map((route) => route.routeId).join(",");
  return `${snapshot.imagesGeneration.enabled}:${snapshot.imagesGeneration.routeId ?? ""}:${snapshot.imagesGeneration.timeoutSecs}:${snapshot.imagesGeneration.model}:${routeIds}`;
}

function SnapshotSummary({
  rows,
}: {
  rows: ReadonlyArray<readonly [string, string]>;
}) {
  return (
    <div className="settings-confirm-details-grid">
      {rows.map(([label, value]) => (
        <div className="settings-confirm-details-row" key={label}>
          <span>{label}</span>
          <strong>{value}</strong>
        </div>
      ))}
    </div>
  );
}

function formatUnixMode(mode: number | null): string {
  return mode === null ? "-" : mode.toString(8).padStart(4, "0");
}
