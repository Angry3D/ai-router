import { FolderOpen, LoaderCircle, RefreshCw, ShieldCheck } from "lucide-react";
import { useState } from "react";
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
import type { AppearancePreference } from "../../generated";
import type {
  ApplicationUpdateSnapshotDto,
  BalanceQuerySettingsDto,
  MenuBarSettingsDto,
  OutboundProxySettingsDto,
  RecoveryHealthKind,
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

const recoveryHealthPresentation: Record<
  RecoveryHealthKind,
  { label: string; tone: SettingsTone }
> = {
  protected: { label: "已保护", tone: "success" },
  updating: { label: "正在更新", tone: "neutral" },
  degraded: { label: "保护已降级", tone: "danger" },
};

export function SystemSettings({
  snapshot,
  applicationUpdate = null,
}: {
  snapshot: SettingsSnapshotDto;
  applicationUpdate?: ApplicationUpdateSnapshotDto | null;
}) {
  return (
    <SettingsPage title="系统" titleId="system-title">
      <AppearanceSettings />
      <MenuBarSettings snapshot={snapshot} />
      <OutboundProxySettings settings={snapshot.outboundProxy} />
      <ApplicationUpdateSettings snapshot={applicationUpdate} />
      <ParameterSettings snapshot={snapshot} />
      <DataLogSettings snapshot={snapshot} />
    </SettingsPage>
  );
}

type OutboundProxyMode = "direct" | "proxy";
type OutboundProxyTestState = "idle" | "pending" | "success" | "failure";

const OUTBOUND_PROXY_URL_MAX_LENGTH = 2048;

function validateOutboundProxyDraft(raw: string): string | null {
  const value = raw.trim();
  if (!value) return "请输入代理地址。";
  if (
    new TextEncoder().encode(value).byteLength > OUTBOUND_PROXY_URL_MAX_LENGTH
  )
    return "代理地址过长。";
  if (
    Array.from(value).some((character) => {
      const codePoint = character.codePointAt(0) ?? 0;
      return codePoint <= 31 || (codePoint >= 127 && codePoint <= 159);
    })
  )
    return "代理地址不能包含控制字符。";

  let parsed: URL;
  try {
    parsed = new URL(value);
  } catch {
    return "请输入有效的代理地址。";
  }
  if (!["http:", "https:", "socks5:", "socks5h:"].includes(parsed.protocol))
    return "支持 HTTP、HTTPS、SOCKS5 和 SOCKS5H 代理。";
  if (parsed.username || parsed.password)
    return "代理地址不能包含用户名或密码。";
  if (parsed.search || parsed.hash) return "代理地址不能包含查询参数或片段。";
  return null;
}

function OutboundProxySettings({
  settings,
}: {
  settings: OutboundProxySettingsDto;
}) {
  const queryClient = useQueryClient();
  const [confirmed, setConfirmed] = useState(settings);
  const [mode, setMode] = useState<OutboundProxyMode>(
    settings.enabled ? "proxy" : "direct",
  );
  const [draftUrl, setDraftUrl] = useState(settings.url ?? "");
  const [syncedSnapshotKey, setSyncedSnapshotKey] = useState(
    `${settings.enabled}:${settings.url ?? ""}`,
  );
  const [savePending, setSavePending] = useState(false);
  const [fieldError, setFieldError] = useState<string | null>(null);
  const [modeError, setModeError] = useState<string | null>(null);
  const [testState, setTestState] = useState<OutboundProxyTestState>("idle");
  const snapshotKey = `${settings.enabled}:${settings.url ?? ""}`;
  const draftValidation = validateOutboundProxyDraft(draftUrl);
  const pending = savePending || testState === "pending";

  if (!pending && syncedSnapshotKey !== snapshotKey) {
    setSyncedSnapshotKey(snapshotKey);
    setConfirmed(settings);
    setMode(settings.enabled ? "proxy" : "direct");
    setDraftUrl(settings.url ?? "");
    setFieldError(null);
    setModeError(null);
    setTestState("idle");
  }

  async function refreshSettings() {
    await queryClient.invalidateQueries({ queryKey: queryKeys.settings });
  }

  async function selectMode(nextMode: OutboundProxyMode) {
    if (pending || nextMode === mode) return;
    setMode(nextMode);
    setFieldError(null);
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
      if (nextMode === "direct") setDraftUrl(next.url ?? "");
      await refreshSettings();
    } catch (reason) {
      setMode(confirmed.enabled ? "proxy" : "direct");
      setModeError(normalizeIpcError(reason).message);
    } finally {
      setSavePending(false);
    }
  }

  async function saveDraft() {
    if (pending || mode !== "proxy") return;
    const value = draftUrl.trim();
    const validation = validateOutboundProxyDraft(value);
    if (validation) {
      setFieldError(validation);
      return;
    }
    if (confirmed.enabled && confirmed.url === value) {
      setDraftUrl(value);
      setFieldError(null);
      return;
    }

    const next: OutboundProxySettingsDto = { enabled: true, url: value };
    setSavePending(true);
    setFieldError(null);
    setModeError(null);
    setTestState("idle");
    try {
      await updateOutboundProxySettings(next);
      setConfirmed(next);
      setDraftUrl(value);
      await refreshSettings();
    } catch (reason) {
      setFieldError(normalizeIpcError(reason).message);
    } finally {
      setSavePending(false);
    }
  }

  async function runConnectionTest() {
    if (pending) return;
    const value = draftUrl.trim();
    const validation = validateOutboundProxyDraft(value);
    if (validation) {
      setFieldError(validation);
      return;
    }
    setFieldError(null);
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
    <SettingsSection title="全局出站代理">
      <SettingsFieldRow label="模式">
        <div
          className="settings-segments outbound-proxy-mode"
          role="radiogroup"
          aria-label="全局出站代理模式"
          aria-busy={savePending}
        >
          {(
            [
              { value: "direct", label: "直连" },
              { value: "proxy", label: "代理" },
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
          label="代理地址"
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
              aria-invalid={fieldError !== null || draftValidation !== null}
              aria-describedby={
                fieldError || draftValidation
                  ? "outbound-proxy-url-error"
                  : "outbound-proxy-help"
              }
              onChange={(event) => {
                setDraftUrl(event.currentTarget.value);
                setFieldError(null);
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
              测试连接
            </SettingsButton>
            <div
              className="outbound-proxy-test-status"
              aria-live="polite"
              aria-busy={testState === "pending"}
            >
              {testState === "pending" ? (
                <SettingsStatus>
                  <LoaderCircle aria-hidden="true" className="spin" size={14} />
                  正在测试
                </SettingsStatus>
              ) : null}
              {testState === "success" ? (
                <SettingsStatus tone="success">连接正常</SettingsStatus>
              ) : null}
              {testState === "failure" ? (
                <SettingsStatus tone="danger">连接失败</SettingsStatus>
              ) : null}
            </div>
            {fieldError || draftValidation ? (
              <p
                id="outbound-proxy-url-error"
                className="parameter-field-error outbound-proxy-field-error"
                role="alert"
              >
                {fieldError ?? draftValidation}
              </p>
            ) : null}
          </div>
        </SettingsFieldRow>
      ) : null}
      <p id="outbound-proxy-help" className="outbound-proxy-help">
        {mode === "proxy"
          ? "仅本地回环地址直连，外部请求使用此代理。"
          : "外部请求将直接连接目标服务。"}
      </p>
      <p
        className="outbound-proxy-mode-error"
        role={modeError ? "alert" : undefined}
        aria-live="polite"
      >
        {modeError ?? "\u00a0"}
      </p>
    </SettingsSection>
  );
}

function MenuBarSettings({ snapshot }: { snapshot: SettingsSnapshotDto }) {
  const queryClient = useQueryClient();
  const [confirmed, setConfirmed] = useState<MenuBarSettingsDto>(snapshot.menuBar);
  const [draft, setDraft] = useState<MenuBarSettingsDto>(snapshot.menuBar);
  const snapshotKey = `${snapshot.menuBar.statusTextEnabled}:${snapshot.menuBar.activityAnimationEnabled}`;
  const [syncedSnapshotKey, setSyncedSnapshotKey] = useState(snapshotKey);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);

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
      setError(normalizeIpcError(reason).message);
    } finally {
      setPending(false);
    }
  }

  return (
    <SettingsSection title="菜单栏">
      <div className="menu-bar-settings-list" aria-busy={pending}>
        <SettingsSwitch
          label="菜单栏状态文字"
          checked={draft.statusTextEnabled}
          disabled={pending}
          onChange={(event) =>
            void submit({ ...draft, statusTextEnabled: event.currentTarget.checked })
          }
        />
        <SettingsSwitch
          label="菜单栏活动动画"
          checked={draft.activityAnimationEnabled}
          disabled={pending}
          onChange={(event) =>
            void submit({ ...draft, activityAnimationEnabled: event.currentTarget.checked })
          }
        />
      </div>
      <p className="menu-bar-settings-error" role={error ? "alert" : undefined} aria-live="polite">
        {error ?? "\u00a0"}
      </p>
    </SettingsSection>
  );
}

const appearanceOptions: Array<{ value: AppearancePreference; label: string }> =
  [
    { value: "system", label: "跟随系统" },
    { value: "light", label: "浅色" },
    { value: "dark", label: "深色" },
  ];

function AppearanceSettings() {
  const { preference, pending, error, setPreference } = useAppearance();
  return (
    <SettingsSection title="外观">
      <SettingsFieldRow label="主题">
        <div className="appearance-field">
          <div
            className="settings-segments settings-segments-three"
            role="radiogroup"
            aria-label="外观主题"
            aria-describedby={error ? "appearance-preference-error" : undefined}
            aria-busy={pending}
          >
            {appearanceOptions.map((option) => (
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
    </SettingsSection>
  );
}

function ParameterSettings({ snapshot }: { snapshot: SettingsSnapshotDto }) {
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
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);
  const menu = boundedInteger(menuDebounce, 10, 600);
  const automatic = boundedInteger(automaticRefresh, 5, 1_440);
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
      setError(normalizeIpcError(reason).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <SettingsSection title="余额查询">
      <SettingsFieldRow label="菜单查询防抖" htmlFor="menu-balance-debounce">
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
            <span>秒</span>
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
        label="自动查询间隔"
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
            <span>分钟</span>
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
          应用
        </SettingsButton>
        {saved ? <SettingsStatus tone="success">已保存</SettingsStatus> : null}
        {error ? (
          <span className="settings-error" role="alert">
            {error}
          </span>
        ) : null}
      </SettingsActionGroup>
    </SettingsSection>
  );
}

function DataLogSettings({ snapshot }: { snapshot: SettingsSnapshotDto }) {
  const queryClient = useQueryClient();
  const [busy, setBusy] = useState(false);
  const [recoveryBusy, setRecoveryBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [recoveryError, setRecoveryError] = useState<string | null>(null);
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
      setError(normalizeIpcError(reason).message);
    } finally {
      setBusy(false);
    }
  };
  const historyConfirmation: SettingsConfirmation = {
    title: "清除全部请求记录？",
    body: "请求记录将被永久删除，路由状态会重置为“未验证”。",
    confirmLabel: "清除记录",
    destructive: true,
    onConfirm: () => {
      setConfirmation(null);
      void run(clearRequestHistory);
    },
  };
  const logConfirmation: SettingsConfirmation = {
    title: "清除运行日志？",
    body: "现有日志将被清除，不影响代理运行。",
    confirmLabel: "清除日志",
    destructive: true,
    onConfirm: () => {
      setConfirmation(null);
      void run(clearRuntimeLogs);
    },
  };
  const failures =
    snapshot.metadataFailure.droppedRecords +
    snapshot.metadataFailure.writeFailures;
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
      setRecoveryError(normalizeIpcError(reason).message);
    } finally {
      setRecoveryBusy(false);
    }
  };
  return (
    <>
      <SettingsSection
        title="数据库恢复"
        titleAccessory={
          <SettingsHelpTooltip label="数据库恢复说明">
            <span>
              主数据库无法安全启动时，应用会进入独立恢复流程；仅检测到可用恢复点时才允许恢复。
            </span>
          </SettingsHelpTooltip>
        }
      >
        <SettingsReadonlyRow label="恢复状态">
          <SettingsStatus tone={recoveryPresentation.tone}>
            {recoveryPresentation.label}
          </SettingsStatus>
        </SettingsReadonlyRow>
        <SettingsReadonlyRow label="最近恢复点">
          {snapshot.recovery.latestSuccessAtMs === null
            ? "尚无恢复点"
            : formatDateTime(snapshot.recovery.latestSuccessAtMs)}
        </SettingsReadonlyRow>
        <SettingsReadonlyRow label="有效恢复点">
          {snapshot.recovery.validPointCount} 个
        </SettingsReadonlyRow>
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
          {recoveryBusy ? "正在创建" : "创建恢复点"}
        </SettingsButton>
        {recoveryError ? (
          <p className="settings-error" role="alert">
            {recoveryError}
          </p>
        ) : null}
      </SettingsSection>
      <SettingsSection title="请求记录">
        <SettingsReadonlyRow label="记录数">
          {snapshot.history.requestCount.toLocaleString()}
        </SettingsReadonlyRow>
        <SettingsReadonlyRow label="覆盖日期">
          {coverageLabel(snapshot)}
        </SettingsReadonlyRow>
        <SettingsReadonlyRow label="数据库占用">
          {formatBytes(snapshot.history.databaseBytes)}
        </SettingsReadonlyRow>
        <SettingsReadonlyRow label="保留期限">
          {snapshot.history.retentionDays} 天
        </SettingsReadonlyRow>
        {failures > 0 ? (
          <p className="inline-warning" role="status">
            本次运行有 {failures} 条请求元数据未记录，推理结果不受影响。
          </p>
        ) : null}
        <SettingsButton
          variant="danger"
          type="button"
          disabled={busy}
          onClick={() => setConfirmation(historyConfirmation)}
        >
          清除全部请求记录
        </SettingsButton>
      </SettingsSection>
      <SettingsSection title="运行日志">
        <SettingsActionGroup>
          <SettingsButton
            type="button"
            disabled={busy}
            onClick={() => void run(openRuntimeLogDirectory)}
          >
            <FolderOpen aria-hidden="true" size={16} />
            打开日志目录
          </SettingsButton>
          <SettingsButton
            variant="danger"
            type="button"
            disabled={busy}
            onClick={() => setConfirmation(logConfirmation)}
          >
            清除日志
          </SettingsButton>
        </SettingsActionGroup>
      </SettingsSection>
      {error ? (
        <p className="settings-error" role="alert">
          {error}
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
): { value: number | null; error: string | null } {
  if (raw.trim() === "") return { value: null, error: "请输入数值。" };
  const value = Number(raw);
  if (!Number.isSafeInteger(value))
    return { value: null, error: "请输入整数。" };
  if (value < minimum || value > maximum) {
    return {
      value: null,
      error: `请输入 ${minimum} 到 ${maximum} 之间的整数。`,
    };
  }
  return { value, error: null };
}

function formatDateTime(value: number | null) {
  return value === null
    ? "-"
    : new Intl.DateTimeFormat("zh-CN", {
        dateStyle: "medium",
        timeStyle: "short",
      }).format(value);
}

function coverageLabel(snapshot: SettingsSnapshotDto) {
  const { earliestStartedAtMs, latestStartedAtMs } = snapshot.history;
  if (earliestStartedAtMs === null || latestStartedAtMs === null)
    return "暂无记录";
  const format = new Intl.DateTimeFormat("zh-CN", { dateStyle: "medium" });
  return `${format.format(earliestStartedAtMs)} 至 ${format.format(latestStartedAtMs)}`;
}

function formatBytes(bytes: number) {
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}
