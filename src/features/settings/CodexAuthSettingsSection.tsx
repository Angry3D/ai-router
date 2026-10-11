import { FolderOpen, LoaderCircle } from "lucide-react";
import { useState } from "react";
import { useQueryClient } from "@tanstack/react-query";

import {
  codexAuthExport,
  codexAuthRestore,
  normalizeIpcError,
  openCodexConfig,
} from "../../api/ipc";
import { queryKeys } from "../../api/query";
import type {
  AppLocale,
  CodexAuthStatusDto,
  IpcErrorDto,
  SettingsSnapshotDto,
} from "../../generated";
import type {
  CodexAuthErrorCopy,
  CodexMessages,
} from "../language/catalog/codex";
import { ipcErrorMessage, useLanguage } from "../language/useLanguage";
import { formatDateTime } from "./codexSettingsFormatting";
import {
  SettingsActionGroup,
  SettingsButton,
  SettingsConfirmDialog,
  SettingsHelpTooltip,
  SettingsReadonlyRow,
  SettingsSection,
  SettingsStatus,
  type SettingsConfirmation,
  type SettingsTone,
} from "./SettingsPrimitives";

interface CodexAuthSettingsSectionProps {
  snapshot: SettingsSnapshotDto;
}

type CodexAuthRequest = "export" | "restore";

function codexAuthStoreModeLabel(
  mode: CodexAuthStatusDto["storeMode"],
  codex: CodexMessages,
) {
  switch (mode) {
    case "keyring":
      return codex.storeModeKeyring;
    case "auto":
      return codex.storeModeAuto;
    case "ephemeral":
      return codex.storeModeEphemeral;
    default:
      return mode;
  }
}

function codexAuthStoreMessage(
  status: CodexAuthStatusDto,
  codex: CodexMessages,
) {
  const mode = codexAuthStoreModeLabel(status.storeMode, codex);
  if (status.managedLocked) {
    return codex.authStoreModeMessageManaged(mode);
  }
  return codex.authStoreModeMessageDefault(mode);
}

function CodexAuthRows({
  status,
  expired,
  showPlan,
  codex,
  locale,
}: {
  status: CodexAuthStatusDto;
  expired: boolean;
  showPlan: boolean;
  codex: CodexMessages;
  locale: AppLocale;
}) {
  return (
    <>
      <SettingsReadonlyRow label={codex.authAccountLabel}>
        {status.accountEmail ?? "—"}
      </SettingsReadonlyRow>
      {showPlan ? (
        <SettingsReadonlyRow label={codex.authPlanLabel}>
          {status.planType ?? "—"}
        </SettingsReadonlyRow>
      ) : null}
      <SettingsReadonlyRow label={codex.authExpiryLabel}>
        {status.expiresAtMs === null
          ? "—"
          : expired
            ? codex.authExpiredValue(
                formatDateTime(status.expiresAtMs, locale),
              )
            : formatDateTime(status.expiresAtMs, locale)}
      </SettingsReadonlyRow>
      <SettingsReadonlyRow label={codex.authExportedAtLabel}>
        {formatDateTime(status.exportedAtMs, locale)}
      </SettingsReadonlyRow>
    </>
  );
}

export function CodexAuthSettingsSection({
  snapshot,
}: CodexAuthSettingsSectionProps) {
  const { messages, effectiveLocale } = useLanguage();
  const codex = messages.codex;
  const queryClient = useQueryClient();
  const status = snapshot.codexAuth;
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<IpcErrorDto | null>(null);
  const [pendingRequest, setPendingRequest] =
    useState<CodexAuthRequest>("export");
  const [confirmation, setConfirmation] = useState<SettingsConfirmation | null>(
    null,
  );

  const run = async (request: CodexAuthRequest) => {
    if (busy) return;
    setBusy(true);
    setFailure(null);
    // Retry repeats the request that failed — never a neighbouring one.
    setPendingRequest(request);
    try {
      await (request === "restore" ? codexAuthRestore() : codexAuthExport());
    } catch (reason) {
      setFailure(normalizeIpcError(reason));
    } finally {
      // Refresh either way: a refusal can mean the target changed underneath
      // us (store mode, drift), and the section must show the current truth.
      await queryClient.invalidateQueries({ queryKey: queryKeys.settings });
      setBusy(false);
    }
  };

  const openConfigFile = async () => {
    try {
      await openCodexConfig();
    } catch (reason) {
      setFailure(normalizeIpcError(reason));
    }
  };

  const storeUnsupported = !status.storeModeSupported;
  const copy: CodexAuthErrorCopy | null = failure
    ? (codex.authErrors[failure.code] ?? {
        tone: "danger",
        label: codex.authErrorFallbackLabel,
        message: ipcErrorMessage(failure, messages),
        retryKind: "secondary",
      })
    : null;
  const expired = status.expired;
  const drifted = status.drifted;
  const exported = status.exportedAtMs != null;

  const mode = busy
    ? "busy"
    : storeUnsupported
      ? "store"
      : copy
        ? copy.retryKind === "none"
          ? "notice"
          : failure?.code === "codex_auth_keychain_denied"
            ? "keychain"
            : "error"
        : drifted
          ? "drifted"
          : expired
            ? "expired"
            : exported
              ? "success"
              : "idle";

  let heading: { tone: SettingsTone; label: string };
  switch (mode) {
    case "busy":
      heading = { tone: "neutral", label: codex.authHeadingBusy };
      break;
    case "store":
      heading = { tone: "warning", label: codex.authStoreModeUnsupportedLabel };
      break;
    case "notice":
    case "error":
      heading = {
        tone: copy?.tone ?? "danger",
        label: copy?.label ?? codex.authHeadingOperationFailed,
      };
      break;
    case "keychain":
      heading = { tone: "warning", label: codex.authKeychainDeniedLabel };
      break;
    case "drifted":
      heading = { tone: "warning", label: codex.authHeadingDrifted };
      break;
    case "expired":
      heading = { tone: "warning", label: codex.authHeadingExpired };
      break;
    case "success":
      heading = { tone: "success", label: codex.authHeadingSuccess };
      break;
    default:
      heading = { tone: "neutral", label: codex.authHeadingIdle };
  }

  return (
    <SettingsSection
      title={codex.authSectionTitle}
      titleAccessory={
        <SettingsHelpTooltip label={codex.authHelpLabel}>
          <strong>{codex.authHelpStrong}</strong>
          <span>{codex.authHelpBody}</span>
          <span>{codex.authHelpNote}</span>
        </SettingsHelpTooltip>
      }
      status={
        <SettingsStatus tone={heading.tone}>{heading.label}</SettingsStatus>
      }
    >
      {mode === "idle" ? (
        <>
          <p className="muted-text codex-auth-hint">{codex.authIdleHint}</p>
          <SettingsActionGroup>
            <SettingsButton
              variant="primary"
              type="button"
              disabled={busy}
              onClick={() => void run("export")}
            >
              {codex.authExportAction}
            </SettingsButton>
          </SettingsActionGroup>
          <p className="muted-text codex-auth-hint">{codex.authKeychainHint}</p>
        </>
      ) : null}
      {mode === "store" ? (
        <>
          <p className="inline-warning codex-auth-message">
            {codexAuthStoreMessage(status, codex)}
          </p>
          <SettingsActionGroup>
            <SettingsButton variant="primary" type="button" disabled>
              {codex.authExportAction}
            </SettingsButton>
            {status.managedLocked ? null : (
              <SettingsButton
                type="button"
                onClick={() => void openConfigFile()}
              >
                <FolderOpen aria-hidden="true" size={16} />
                {codex.authOpenConfigAction}
              </SettingsButton>
            )}
          </SettingsActionGroup>
        </>
      ) : null}
      {mode === "keychain" ? (
        <>
          <p className="inline-warning codex-auth-message">
            {codex.authKeychainDeniedMessage}
          </p>
          <SettingsActionGroup>
            <SettingsButton
              variant="primary"
              type="button"
              disabled={busy}
              onClick={() => void run("export")}
            >
              {codex.authRetryAction}
            </SettingsButton>
          </SettingsActionGroup>
        </>
      ) : null}
      {mode === "notice" ? (
        <>
          <p className="inline-warning codex-auth-message">
            {failure?.code === "codex_auth_store_mode_unsupported"
              ? codexAuthStoreMessage(status, codex)
              : copy?.message}
          </p>
          {failure?.code === "codex_auth_store_mode_unsupported" &&
          !status.managedLocked ? (
            <SettingsActionGroup>
              <SettingsButton
                type="button"
                onClick={() => void openConfigFile()}
              >
                <FolderOpen aria-hidden="true" size={16} />
                {codex.authOpenConfigAction}
              </SettingsButton>
            </SettingsActionGroup>
          ) : null}
        </>
      ) : null}
      {mode === "error" ? (
        <>
          <p
            className={
              copy?.tone === "warning"
                ? "inline-warning codex-auth-message"
                : "settings-error codex-auth-message"
            }
            role="alert"
          >
            {copy?.message}
          </p>
          <SettingsActionGroup>
            <SettingsButton
              type="button"
              disabled={busy}
              onClick={() => void run(pendingRequest)}
            >
              {codex.authRetryAction}
            </SettingsButton>
          </SettingsActionGroup>
        </>
      ) : null}
      {mode === "busy" ? (
        <SettingsActionGroup>
          <SettingsButton variant="primary" type="button" disabled>
            <LoaderCircle aria-hidden="true" className="spin" size={15} />
            {codex.authBusyAction}
          </SettingsButton>
        </SettingsActionGroup>
      ) : null}
      {mode === "success" || mode === "drifted" || mode === "expired" ? (
        <>
          {mode === "drifted" ? (
            <p className="inline-warning codex-auth-message">
              {codex.authDriftedMessage}
            </p>
          ) : null}
          <CodexAuthRows
            status={status}
            expired={mode === "expired"}
            showPlan={mode === "success"}
            codex={codex}
            locale={effectiveLocale}
          />
          <SettingsActionGroup>
            <SettingsButton
              variant="primary"
              type="button"
              disabled={busy}
              onClick={() => void run("export")}
            >
              {codex.authReexportAction}
            </SettingsButton>
            {mode === "success" && status.backupAvailable ? (
              <SettingsButton
                variant="danger"
                type="button"
                disabled={busy}
                onClick={() =>
                  setConfirmation({
                    title: codex.authRestoreConfirmTitle,
                    body: codex.authRestoreConfirmBody,
                    confirmLabel: codex.authRestoreConfirmLabel,
                    destructive: true,
                    onConfirm: () => {
                      setConfirmation(null);
                      void run("restore");
                    },
                  })
                }
              >
                {codex.authRestoreAction}
              </SettingsButton>
            ) : null}
          </SettingsActionGroup>
          {mode === "success" || mode === "expired" ? (
            <p className="muted-text codex-auth-hint">{codex.authRestartHint}</p>
          ) : null}
        </>
      ) : null}
      {confirmation ? (
        <SettingsConfirmDialog
          confirmation={confirmation}
          onCancel={() => setConfirmation(null)}
        />
      ) : null}
    </SettingsSection>
  );
}
