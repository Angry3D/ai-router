import { FolderOpen, LoaderCircle, RefreshCw, ShieldCheck } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";

import {
  clearRequestHistory,
  clearRuntimeLogs,
  createRecoveryPoint,
  normalizeIpcError,
  openRuntimeLogDirectory,
  testOutboundProxy,
  updateBalanceQuerySettings,
  updateMenuBarSettings,
  updateOutboundProxySettings,
} from "../../api/ipc";
import { queryKeys } from "../../api/query";
import { useAppearance } from "../appearance/useAppearance";
import {
  ipcErrorMessage,
  languageSyncIncompleteCode,
  useLanguage,
} from "../language/useLanguage";
import type { SystemRecoveryMessages } from "../language/catalog/systemRecovery";
import type {
  AppearancePreference,
  AppLocale,
  IpcErrorDto,
  LanguagePreference,
} from "../../generated";
import type {
  ApplicationUpdateSnapshotDto,
  BalanceQuerySettingsDto,
  MenuBarSettingsDto,
  OutboundProxySettingsDto,
  RecoveryHealthKind,
  RecoveryIncidentDto,
  SettingsSnapshotDto,
} from "../../generated";
import { ApplicationUpdateSettings } from "./ApplicationUpdateSettings";
import {
  SettingsActionGroup,
  SettingsButton,
  SettingsConfirmDialog,
  SettingsFieldRow,
  SettingsHelpTooltip,
  SettingsPage,
  SettingsReadonlyRow,
  SettingsSection,
  SettingsStatus,
  SettingsSwitch,
  SettingsTextInput,
  type SettingsConfirmation,
  type SettingsTone,
} from "./SettingsPrimitives";

function recoveryIncidentSummary(
  incident: RecoveryIncidentDto,
  messages: SystemRecoveryMessages,
  locale: AppLocale,
) {
  const detectedAt = formatDateTime(incident.detectedAtMs, locale);
  switch (incident.action) {
    case "repaired":
      return messages.incidentRepaired(detectedAt);
    // `quarantined` belongs to the recovery flow that isolates the unusable
    // primary; incidents written by older versions still carry it.
    case "quarantined":
    case "recovery_required":
      return messages.incidentRecoveryFlow(detectedAt);
    case "start_over":
      return messages.incidentStartedOver(detectedAt);
  }
}

export function SystemSettings({
  snapshot,
  applicationUpdate = null,
}: {
  snapshot: SettingsSnapshotDto;
  applicationUpdate?: ApplicationUpdateSnapshotDto | null;
}) {
  const { messages } = useLanguage();
  return (
    <SettingsPage
      title={messages.systemRecovery.pageTitle}
      titleId="system-title"
    >
      <AppearanceSettings />
      <MenuBarSettings snapshot={snapshot} />
      <OutboundProxySettings settings={snapshot.outboundProxy} />
      <ApplicationUpdateSettings snapshot={applicationUpdate} />
      <ParameterSettings snapshot={snapshot} />
      <DataLogSettings snapshot={snapshot} />
    </SettingsPage>
  );
}

type OutboundProxyMode = "system" | "proxy";
type OutboundProxyTestState = "idle" | "pending" | "success" | "failure";

const OUTBOUND_PROXY_URL_MAX_LENGTH = 2048;

function validateOutboundProxyDraft(
  raw: string,
  messages: SystemRecoveryMessages,
): string | null {
  const value = raw.trim();
  if (!value) return messages.outboundProxyUrlRequired;
  if (
    new TextEncoder().encode(value).byteLength > OUTBOUND_PROXY_URL_MAX_LENGTH
  )
    return messages.outboundProxyUrlTooLong;
  if (
    Array.from(value).some((character) => {
      const codePoint = character.codePointAt(0) ?? 0;
      return codePoint <= 31 || (codePoint >= 127 && codePoint <= 159);
    })
  )
    return messages.outboundProxyUrlControlCharacter;

  let parsed: URL;
  try {
    parsed = new URL(value);
  } catch {
    return messages.outboundProxyUrlInvalid;
  }
  if (!["http:", "https:", "socks5:", "socks5h:"].includes(parsed.protocol))
    return messages.outboundProxyUrlUnsupportedScheme;
  if (parsed.username || parsed.password)
    return messages.outboundProxyUrlCredentials;
  if (parsed.search || parsed.hash)
    return messages.outboundProxyUrlQueryOrFragment;
  return null;
}

function OutboundProxySettings({
  settings,
}: {
  settings: OutboundProxySettingsDto;
}) {
  const { messages } = useLanguage();
  const systemMessages = messages.systemRecovery;
  const queryClient = useQueryClient();
  const [confirmed, setConfirmed] = useState(settings);
  const [mode, setMode] = useState<OutboundProxyMode>(
    settings.enabled ? "proxy" : "system",
  );
  const [draftUrl, setDraftUrl] = useState(settings.url ?? "");
  const [syncedSnapshotKey, setSyncedSnapshotKey] = useState(
    `${settings.enabled}:${settings.url ?? ""}`,
  );
  const [savePending, setSavePending] = useState(false);
  const [saveError, setSaveError] = useState<IpcErrorDto | null>(null);
  const [modeError, setModeError] = useState<IpcErrorDto | null>(null);
  const [testState, setTestState] = useState<OutboundProxyTestState>("idle");
  const snapshotKey = `${settings.enabled}:${settings.url ?? ""}`;
  const draftValidation = validateOutboundProxyDraft(draftUrl, systemMessages);
  const pending = savePending || testState === "pending";

  if (!pending && syncedSnapshotKey !== snapshotKey) {
    setSyncedSnapshotKey(snapshotKey);
    setConfirmed(settings);
    setMode(settings.enabled ? "proxy" : "system");
    setDraftUrl(settings.url ?? "");
    setSaveError(null);
    setModeError(null);
    setTestState("idle");
  }

  async function refreshSettings() {
    await queryClient.invalidateQueries({ queryKey: queryKeys.settings });
  }

  async function selectMode(nextMode: OutboundProxyMode) {
    if (pending || nextMode === mode) return;
    setMode(nextMode);
    setSaveError(null);
    setModeError(null);
    setTestState("idle");

    if (nextMode === "proxy" && confirmed.url === null) return;
    const next: OutboundProxySettingsDto = {
      enabled: nextMode === "proxy",
      url: confirmed.url,
    };
    if (next.enabled === confirmed.enabled) return;

    setSavePending(true);
    try {
      await updateOutboundProxySettings(next);
      setConfirmed(next);
      if (nextMode === "system") setDraftUrl(next.url ?? "");
      await refreshSettings();
    } catch (reason) {
      setMode(confirmed.enabled ? "proxy" : "system");
      setModeError(normalizeIpcError(reason));
    } finally {
      setSavePending(false);
    }
  }

  async function saveDraft() {
    if (pending || mode !== "proxy") return;
    const value = draftUrl.trim();
    if (validateOutboundProxyDraft(value, systemMessages)) {
      setSaveError(null);
      return;
    }
    if (confirmed.enabled && confirmed.url === value) {
      setDraftUrl(value);
      setSaveError(null);
      return;
    }

    const next: OutboundProxySettingsDto = { enabled: true, url: value };
    setSavePending(true);
    setSaveError(null);
    setModeError(null);
    setTestState("idle");
    try {
      await updateOutboundProxySettings(next);
      setConfirmed(next);
      setDraftUrl(value);
      await refreshSettings();
    } catch (reason) {
      setSaveError(normalizeIpcError(reason));
    } finally {
      setSavePending(false);
    }
  }

  async function runConnectionTest() {
    if (pending) return;
    const value = draftUrl.trim();
    if (validateOutboundProxyDraft(value, systemMessages)) {
      setSaveError(null);
      return;
    }
    setSaveError(null);
    setModeError(null);
    setTestState("pending");
    try {
      await testOutboundProxy(value);
      setTestState("success");
    } catch {
      setTestState("failure");
    }
  }

  return (
    <SettingsSection title={systemMessages.outboundProxySectionTitle}>
      <SettingsFieldRow label={systemMessages.outboundProxyModeLabel}>
        <div
          className="settings-segments outbound-proxy-mode"
          role="radiogroup"
          aria-label={systemMessages.outboundProxyModeAriaLabel}
          aria-busy={savePending}
        >
          {(
            [
              {
                value: "system",
                label: systemMessages.outboundProxyModeSystem,
              },
              { value: "proxy", label: systemMessages.outboundProxyModeCustom },
            ] as const
          ).map((option) => (
            <label className="settings-segment-option" key={option.value}>
              <input
                type="radio"
                name="outbound-proxy-mode"
                value={option.value}
                checked={mode === option.value}
                disabled={pending}
                data-outbound-proxy-no-blur-save="true"
                onChange={() => void selectMode(option.value)}
              />
              <span>{option.label}</span>
            </label>
          ))}
        </div>
      </SettingsFieldRow>
      {mode === "proxy" ? (
        <SettingsFieldRow
          label={systemMessages.outboundProxyUrlLabel}
          htmlFor="outbound-proxy-url"
          required
          className="outbound-proxy-url-row"
        >
          <div className="outbound-proxy-url-control">
            <SettingsTextInput
              id="outbound-proxy-url"
              type="url"
              inputMode="url"
              autoCapitalize="none"
              autoCorrect="off"
              spellCheck={false}
              placeholder="http://127.0.0.1:7890"
              value={draftUrl}
              disabled={pending}
              aria-required="true"
              aria-invalid={saveError !== null || draftValidation !== null}
              aria-describedby={
                saveError || draftValidation
                  ? "outbound-proxy-url-error"
                  : "outbound-proxy-help"
              }
              onChange={(event) => {
                setDraftUrl(event.currentTarget.value);
                setSaveError(null);
                setModeError(null);
                setTestState("idle");
              }}
              onKeyDown={(event) => {
                if (event.key !== "Enter") return;
                event.preventDefault();
                void saveDraft();
              }}
              onBlur={(event) => {
                const nextTarget = event.relatedTarget;
                if (
                  nextTarget instanceof HTMLElement &&
                  nextTarget.closest(
                    '[data-outbound-proxy-no-blur-save="true"]',
                  )
                ) {
                  return;
                }
                void saveDraft();
              }}
            />
            <SettingsButton
              type="button"
              disabled={pending || draftValidation !== null}
              data-outbound-proxy-no-blur-save="true"
              onClick={() => void runConnectionTest()}
            >
              {systemMessages.outboundProxyTestButton}
            </SettingsButton>
            <div
              className="outbound-proxy-test-status"
              aria-live="polite"
              aria-busy={testState === "pending"}
            >
              {testState === "pending" ? (
                <SettingsStatus>
                  <LoaderCircle aria-hidden="true" className="spin" size={14} />
                  {systemMessages.outboundProxyTesting}
                </SettingsStatus>
              ) : null}
              {testState === "success" ? (
                <SettingsStatus tone="success">
                  {systemMessages.outboundProxyTestSuccess}
                </SettingsStatus>
              ) : null}
              {testState === "failure" ? (
                <SettingsStatus tone="danger">
                  {systemMessages.outboundProxyTestFailure}
                </SettingsStatus>
              ) : null}
            </div>
            {saveError || draftValidation ? (
              <p
                id="outbound-proxy-url-error"
                className="parameter-field-error outbound-proxy-field-error"
                role="alert"
              >
                {saveError
                  ? ipcErrorMessage(saveError, messages)
                  : draftValidation}
              </p>
            ) : null}
          </div>
        </SettingsFieldRow>
      ) : null}
      <p id="outbound-proxy-help" className="outbound-proxy-help">
        {mode === "proxy"
          ? systemMessages.outboundProxyHelpProxy
          : systemMessages.outboundProxyHelpSystem}
      </p>
      <p
        className="outbound-proxy-mode-error"
        role={modeError ? "alert" : undefined}
        aria-live="polite"
      >
        {modeError ? ipcErrorMessage(modeError, messages) : "\u00a0"}
      </p>
    </SettingsSection>
  );
}

function MenuBarSettings({ snapshot }: { snapshot: SettingsSnapshotDto }) {
  const { messages } = useLanguage();
  const systemMessages = messages.systemRecovery;
  const queryClient = useQueryClient();
  const [confirmed, setConfirmed] = useState<MenuBarSettingsDto>(
    snapshot.menuBar,
  );
  const [draft, setDraft] = useState<MenuBarSettingsDto>(snapshot.menuBar);
  const snapshotKey = `${snapshot.menuBar.statusTextEnabled}:${snapshot.menuBar.activityAnimationEnabled}`;
  const [syncedSnapshotKey, setSyncedSnapshotKey] = useState(snapshotKey);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<IpcErrorDto | null>(null);

  if (!pending && syncedSnapshotKey !== snapshotKey) {
    setSyncedSnapshotKey(snapshotKey);
    setConfirmed(snapshot.menuBar);
    setDraft(snapshot.menuBar);
  }

  async function submit(next: MenuBarSettingsDto) {
    setDraft(next);
    setPending(true);
    setError(null);
    try {
      await updateMenuBarSettings(next);
      setConfirmed(next);
      await queryClient.invalidateQueries({ queryKey: queryKeys.settings });
    } catch (reason) {
      setDraft(confirmed);
      setError(normalizeIpcError(reason));
    } finally {
      setPending(false);
    }
  }

  return (
    <SettingsSection title={systemMessages.menuBarSectionTitle}>
      <div className="menu-bar-settings-list" aria-busy={pending}>
        <SettingsSwitch
          label={systemMessages.menuBarStatusTextLabel}
          checked={draft.statusTextEnabled}
          disabled={pending}
          onChange={(event) =>
            void submit({
              ...draft,
              statusTextEnabled: event.currentTarget.checked,
            })
          }
        />
        <SettingsSwitch
          label={systemMessages.menuBarActivityAnimationLabel}
          checked={draft.activityAnimationEnabled}
          disabled={pending}
          onChange={(event) =>
            void submit({
              ...draft,
              activityAnimationEnabled: event.currentTarget.checked,
            })
          }
        />
      </div>
      <p
        className="menu-bar-settings-error"
        role={error ? "alert" : undefined}
        aria-live="polite"
      >
        {error ? ipcErrorMessage(error, messages) : "\u00a0"}
      </p>
    </SettingsSection>
  );
}

function AppearanceSettings() {
  const { preference, pending, error, setPreference } = useAppearance();
  const {
    messages,
    preference: languagePreference,
    pending: languagePending,
    error: languageError,
    setPreference: setLanguagePreference,
  } = useLanguage();
  const systemMessages = messages.systemRecovery;
  const themeOptions: Array<{ value: AppearancePreference; label: string }> = [
    { value: "system", label: systemMessages.themeSystem },
    { value: "light", label: systemMessages.themeLight },
    { value: "dark", label: systemMessages.themeDark },
  ];
  // Autonyms stay stable so a user can recover from an unfamiliar language.
  const languageOptions: Array<{
    value: LanguagePreference;
    label: string;
    lang?: string;
  }> = [
    { value: "system", label: systemMessages.languageSystem },
    { value: "zh-CN", label: "简体中文", lang: "zh-CN" },
    { value: "en", label: "English", lang: "en" },
  ];
  const originatingOptionRef = useRef<HTMLInputElement | null>(null);
  const wasLanguagePendingRef = useRef(false);
  useEffect(() => {
    if (wasLanguagePendingRef.current && !languagePending) {
      originatingOptionRef.current?.focus();
    }
    wasLanguagePendingRef.current = languagePending;
  }, [languagePending]);

  return (
    <SettingsSection title={systemMessages.appearanceSectionTitle}>
      <SettingsFieldRow label={systemMessages.themeLabel}>
        <div className="appearance-field">
          <div
            className="settings-segments settings-segments-three"
            role="radiogroup"
            aria-label={systemMessages.themeLabel}
            aria-describedby={error ? "appearance-preference-error" : undefined}
            aria-busy={pending}
          >
            {themeOptions.map((option) => (
              <label className="settings-segment-option" key={option.value}>
                <input
                  type="radio"
                  name="appearance-preference"
                  value={option.value}
                  checked={preference === option.value}
                  disabled={pending}
                  onChange={() => void setPreference(option.value)}
                />
                <span>{option.label}</span>
              </label>
            ))}
          </div>
          <p
            id="appearance-preference-error"
            className="appearance-field-error"
            role={error ? "alert" : undefined}
            aria-live="polite"
          >
            {error ?? "\u00a0"}
          </p>
        </div>
      </SettingsFieldRow>
      <SettingsFieldRow label={systemMessages.languageLabel}>
        <div className="appearance-field">
          <div
            className="settings-segments settings-segments-three"
            role="radiogroup"
            aria-label={systemMessages.languageLabel}
            aria-describedby={
              languageError ? "language-preference-error" : undefined
            }
            aria-busy={languagePending}
          >
            {languageOptions.map((option) => (
              <label className="settings-segment-option" key={option.value}>
                <input
                  type="radio"
                  name="language-preference"
                  value={option.value}
                  checked={languagePreference === option.value}
                  disabled={languagePending}
                  onChange={(event) => {
                    originatingOptionRef.current = event.currentTarget;
                    void setLanguagePreference(option.value);
                  }}
                />
                <span lang={option.lang}>{option.label}</span>
              </label>
            ))}
          </div>
          <p
            id="language-preference-error"
            className="appearance-field-error"
            role={languageError ? "alert" : undefined}
            aria-live="polite"
          >
            {languageError
              ? languageError.code === languageSyncIncompleteCode
                ? systemMessages.languageSyncIncomplete
                : systemMessages.languageSaveFailed
              : "\u00a0"}
          </p>
        </div>
      </SettingsFieldRow>
    </SettingsSection>
  );
}

function ParameterSettings({ snapshot }: { snapshot: SettingsSnapshotDto }) {
  const { messages } = useLanguage();
  const systemMessages = messages.systemRecovery;
  const queryClient = useQueryClient();
  const [baseline, setBaseline] = useState<BalanceQuerySettingsDto>(
    snapshot.balanceQuery,
  );
  const [menuDebounce, setMenuDebounce] = useState(
    String(snapshot.balanceQuery.menuDebounceSeconds),
  );
  const [automaticRefresh, setAutomaticRefresh] = useState(
    String(snapshot.balanceQuery.automaticRefreshMinutes),
  );
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<IpcErrorDto | null>(null);
  const [saved, setSaved] = useState(false);
  const menu = boundedInteger(menuDebounce, 10, 600, systemMessages);
  const automatic = boundedInteger(automaticRefresh, 5, 1_440, systemMessages);
  const input =
    menu.value === null || automatic.value === null
      ? null
      : {
          menuDebounceSeconds: menu.value,
          automaticRefreshMinutes: automatic.value,
        };
  const unchanged =
    input !== null &&
    input.menuDebounceSeconds === baseline.menuDebounceSeconds &&
    input.automaticRefreshMinutes === baseline.automaticRefreshMinutes;

  const apply = async () => {
    if (!input) return;
    setBusy(true);
    setError(null);
    setSaved(false);
    try {
      await updateBalanceQuerySettings(input);
      setBaseline(input);
      setMenuDebounce(String(input.menuDebounceSeconds));
      setAutomaticRefresh(String(input.automaticRefreshMinutes));
      setSaved(true);
      await queryClient.invalidateQueries({ queryKey: queryKeys.settings });
    } catch (reason) {
      setError(normalizeIpcError(reason));
    } finally {
      setBusy(false);
    }
  };

  return (
    <SettingsSection title={systemMessages.balanceQuerySectionTitle}>
      <SettingsFieldRow
        label={systemMessages.balanceMenuDebounceLabel}
        htmlFor="menu-balance-debounce"
      >
        <div className="parameter-field">
          <div className="parameter-input-control">
            <SettingsTextInput
              id="menu-balance-debounce"
              type="number"
              inputMode="numeric"
              min={10}
              max={600}
              step={1}
              value={menuDebounce}
              disabled={busy}
              aria-invalid={menu.error !== null}
              aria-describedby={
                menu.error ? "menu-balance-debounce-error" : undefined
              }
              onChange={(event) => {
                setMenuDebounce(event.currentTarget.value);
                setError(null);
                setSaved(false);
              }}
            />
            <span>{systemMessages.secondsUnit}</span>
          </div>
          {menu.error ? (
            <p
              id="menu-balance-debounce-error"
              className="parameter-field-error"
              role="alert"
            >
              {menu.error}
            </p>
          ) : null}
        </div>
      </SettingsFieldRow>
      <SettingsFieldRow
        label={systemMessages.balanceAutomaticRefreshLabel}
        htmlFor="automatic-balance-refresh"
      >
        <div className="parameter-field">
          <div className="parameter-input-control">
            <SettingsTextInput
              id="automatic-balance-refresh"
              type="number"
              inputMode="numeric"
              min={5}
              max={1440}
              step={1}
              value={automaticRefresh}
              disabled={busy}
              aria-invalid={automatic.error !== null}
              aria-describedby={
                automatic.error ? "automatic-balance-refresh-error" : undefined
              }
              onChange={(event) => {
                setAutomaticRefresh(event.currentTarget.value);
                setError(null);
                setSaved(false);
              }}
            />
            <span>{systemMessages.minutesUnit}</span>
          </div>
          {automatic.error ? (
            <p
              id="automatic-balance-refresh-error"
              className="parameter-field-error"
              role="alert"
            >
              {automatic.error}
            </p>
          ) : null}
        </div>
      </SettingsFieldRow>
      <SettingsActionGroup className="parameter-actions">
        <SettingsButton
          type="button"
          variant="primary"
          disabled={busy || input === null || unchanged}
          onClick={() => void apply()}
        >
          {busy ? (
            <LoaderCircle aria-hidden="true" className="spin" size={15} />
          ) : null}
          {systemMessages.applyButton}
        </SettingsButton>
        {saved ? (
          <SettingsStatus tone="success">
            {systemMessages.savedStatus}
          </SettingsStatus>
        ) : null}
        {error ? (
          <span className="settings-error" role="alert">
            {ipcErrorMessage(error, messages)}
          </span>
        ) : null}
      </SettingsActionGroup>
    </SettingsSection>
  );
}

function DataLogSettings({ snapshot }: { snapshot: SettingsSnapshotDto }) {
  const { messages, effectiveLocale } = useLanguage();
  const systemMessages = messages.systemRecovery;
  const queryClient = useQueryClient();
  const [busy, setBusy] = useState(false);
  const [recoveryBusy, setRecoveryBusy] = useState(false);
  const [error, setError] = useState<IpcErrorDto | null>(null);
  const [recoveryError, setRecoveryError] = useState<IpcErrorDto | null>(null);
  const [confirmation, setConfirmation] = useState<SettingsConfirmation | null>(
    null,
  );
  const run = async (operation: () => Promise<unknown>) => {
    setBusy(true);
    setError(null);
    try {
      await operation();
      await queryClient.invalidateQueries({ queryKey: queryKeys.settings });
    } catch (reason) {
      setError(normalizeIpcError(reason));
    } finally {
      setBusy(false);
    }
  };
  const historyConfirmation: SettingsConfirmation = {
    title: systemMessages.clearHistoryConfirmTitle,
    body: systemMessages.clearHistoryConfirmBody,
    confirmLabel: systemMessages.clearHistoryConfirmLabel,
    destructive: true,
    onConfirm: () => {
      setConfirmation(null);
      void run(clearRequestHistory);
    },
  };
  const logConfirmation: SettingsConfirmation = {
    title: systemMessages.clearLogsConfirmTitle,
    body: systemMessages.clearLogsConfirmBody,
    confirmLabel: systemMessages.clearLogsConfirmLabel,
    destructive: true,
    onConfirm: () => {
      setConfirmation(null);
      void run(clearRuntimeLogs);
    },
  };
  const failures =
    snapshot.metadataFailure.droppedRecords +
    snapshot.metadataFailure.writeFailures;
  const recoveryHealthPresentation: Record<
    RecoveryHealthKind,
    { label: string; tone: SettingsTone }
  > = {
    protected: {
      label: systemMessages.recoveryHealthProtected,
      tone: "success",
    },
    updating: {
      label: systemMessages.recoveryHealthUpdating,
      tone: "neutral",
    },
    degraded: {
      label: systemMessages.recoveryHealthDegraded,
      tone: "danger",
    },
  };
  const recoveryPresentation =
    recoveryHealthPresentation[snapshot.recovery.kind];
  const createPoint = async () => {
    if (recoveryBusy) return;
    setRecoveryBusy(true);
    setRecoveryError(null);
    try {
      const health = await createRecoveryPoint();
      queryClient.setQueryData<SettingsSnapshotDto>(
        queryKeys.settings,
        (current) => (current ? { ...current, recovery: health } : current),
      );
      await queryClient.invalidateQueries({ queryKey: queryKeys.settings });
    } catch (reason) {
      setRecoveryError(normalizeIpcError(reason));
    } finally {
      setRecoveryBusy(false);
    }
  };
  return (
    <>
      <SettingsSection
        title={systemMessages.databaseRecoverySectionTitle}
        titleAccessory={
          <SettingsHelpTooltip label={systemMessages.databaseRecoveryHelpLabel}>
            <span>{systemMessages.databaseRecoveryHelpBody}</span>
          </SettingsHelpTooltip>
        }
      >
        <SettingsReadonlyRow label={systemMessages.recoveryStatusLabel}>
          <SettingsStatus tone={recoveryPresentation.tone}>
            {recoveryPresentation.label}
          </SettingsStatus>
        </SettingsReadonlyRow>
        <SettingsReadonlyRow label={systemMessages.latestRecoveryPointLabel}>
          {snapshot.recovery.latestSuccessAtMs === null
            ? systemMessages.noRecoveryPoint
            : formatDateTime(
                snapshot.recovery.latestSuccessAtMs,
                effectiveLocale,
              )}
        </SettingsReadonlyRow>
        <SettingsReadonlyRow label={systemMessages.validRecoveryPointLabel}>
          {systemMessages.recoveryPointCount(snapshot.recovery.validPointCount)}
        </SettingsReadonlyRow>
        {snapshot.recovery.lastIncident ? (
          <SettingsReadonlyRow label={systemMessages.latestIncidentLabel}>
            {recoveryIncidentSummary(
              snapshot.recovery.lastIncident,
              systemMessages,
              effectiveLocale,
            )}
          </SettingsReadonlyRow>
        ) : null}
        <SettingsButton
          type="button"
          disabled={recoveryBusy || snapshot.recovery.kind === "updating"}
          onClick={() => void createPoint()}
        >
          {snapshot.recovery.kind === "protected" ? (
            <ShieldCheck aria-hidden="true" size={16} />
          ) : (
            <RefreshCw
              aria-hidden="true"
              size={16}
              className={
                recoveryBusy || snapshot.recovery.kind === "updating"
                  ? "spin"
                  : ""
              }
            />
          )}
          {recoveryBusy
            ? systemMessages.creatingStatus
            : systemMessages.createRecoveryPointButton}
        </SettingsButton>
        {recoveryError ? (
          <p className="settings-error" role="alert">
            {ipcErrorMessage(recoveryError, messages)}
          </p>
        ) : null}
      </SettingsSection>
      <SettingsSection title={systemMessages.requestHistorySectionTitle}>
        <SettingsReadonlyRow label={systemMessages.recordCountLabel}>
          {snapshot.history.requestCount.toLocaleString(effectiveLocale)}
        </SettingsReadonlyRow>
        <SettingsReadonlyRow label={systemMessages.coverageDateLabel}>
          {coverageLabel(snapshot, systemMessages, effectiveLocale)}
        </SettingsReadonlyRow>
        <SettingsReadonlyRow label={systemMessages.databaseUsageLabel}>
          {formatBytes(snapshot.history.databaseBytes)}
        </SettingsReadonlyRow>
        <SettingsReadonlyRow label={systemMessages.retentionLabel}>
          {systemMessages.retentionDays(snapshot.history.retentionDays)}
        </SettingsReadonlyRow>
        {failures > 0 ? (
          <p className="inline-warning" role="status">
            {systemMessages.metadataFailureWarning(failures)}
          </p>
        ) : null}
        <SettingsButton
          variant="danger"
          type="button"
          disabled={busy}
          onClick={() => setConfirmation(historyConfirmation)}
        >
          {systemMessages.clearHistoryButton}
        </SettingsButton>
      </SettingsSection>
      <SettingsSection title={systemMessages.runtimeLogSectionTitle}>
        <SettingsActionGroup>
          <SettingsButton
            type="button"
            disabled={busy}
            onClick={() => void run(openRuntimeLogDirectory)}
          >
            <FolderOpen aria-hidden="true" size={16} />
            {systemMessages.openLogDirectoryButton}
          </SettingsButton>
          <SettingsButton
            variant="danger"
            type="button"
            disabled={busy}
            onClick={() => setConfirmation(logConfirmation)}
          >
            {systemMessages.clearLogsButton}
          </SettingsButton>
        </SettingsActionGroup>
      </SettingsSection>
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
    </>
  );
}

function boundedInteger(
  raw: string,
  minimum: number,
  maximum: number,
  messages: SystemRecoveryMessages,
): { value: number | null; error: string | null } {
  if (raw.trim() === "") return { value: null, error: messages.numberRequired };
  const value = Number(raw);
  if (!Number.isSafeInteger(value))
    return { value: null, error: messages.integerRequired };
  if (value < minimum || value > maximum) {
    return {
      value: null,
      error: messages.integerRange(minimum, maximum),
    };
  }
  return { value, error: null };
}

function formatDateTime(value: number | null, locale: AppLocale) {
  return value === null
    ? "-"
    : new Intl.DateTimeFormat(locale, {
        dateStyle: "medium",
        timeStyle: "short",
      }).format(value);
}

function coverageLabel(
  snapshot: SettingsSnapshotDto,
  messages: SystemRecoveryMessages,
  locale: AppLocale,
) {
  const { earliestStartedAtMs, latestStartedAtMs } = snapshot.history;
  if (earliestStartedAtMs === null || latestStartedAtMs === null)
    return messages.noRecords;
  const format = new Intl.DateTimeFormat(locale, { dateStyle: "medium" });
  return messages.coverageRange(
    format.format(earliestStartedAtMs),
    format.format(latestStartedAtMs),
  );
}

function formatBytes(bytes: number) {
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}
