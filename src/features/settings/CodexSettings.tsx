import { FolderOpen, LoaderCircle, Trash2 } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";

import {
  applyProxyPort,
  confirmCodexImagesMcpRepair,
  confirmResetCodexRecoveryToBaseline,
  confirmUpdateCodexRecovery,
  clearMcpImages,
  connectCodex,
  normalizeIpcError,
  openCodexConfig,
  openMcpImageDirectory,
  previewCodexImagesMcpRepair,
  previewResetCodexRecoveryToBaseline,
  previewUpdateCodexRecovery,
  reconnectCodex,
  updateImagesGenerationSettings,
  updateMcpImageCapacityThreshold,
} from "../../api/ipc";
import { queryKeys } from "../../api/query";
import type { CodexConfigStatus, SettingsSnapshotDto } from "../../generated";
import {
  SettingsActionGroup,
  SettingsButton,
  SettingsConfirmDialog,
  SettingsDivider,
  SettingsFieldRow,
  SettingsHelpTooltip,
  SettingsPage,
  SettingsReadonlyRow,
  SettingsSelect,
  SettingsSection,
  SettingsStatus,
  SettingsSwitch,
  SettingsTextInput,
  type SettingsConfirmation,
} from "./SettingsPrimitives";

const codexLabels: Record<CodexConfigStatus, string> = {
  checking: "检查中",
  connected: "已连接",
  not_connected: "未连接",
  changed: "待重新连接",
  images_mcp_name_conflict: "图片 MCP 名称冲突",
  images_mcp_projection_conflict: "图片 MCP 配置已修改",
  invalid: "配置无效",
  unreadable: "配置不可读",
  symlink_unsupported: "不支持符号链接",
};

export function CodexSettings({
  snapshot,
  proxyStatus,
  focusImageGeneration = false,
  onImageGenerationFocused,
}: {
  snapshot: SettingsSnapshotDto;
  proxyStatus: string;
  focusImageGeneration?: boolean;
  onImageGenerationFocused?: () => void;
}) {
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
  const [error, setError] = useState<string | null>(null);
  const [recoveryError, setRecoveryError] = useState<string | null>(null);
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
      const message = normalizeIpcError(reason).message;
      if (operationKind.startsWith("recovery_")) setRecoveryError(message);
      else setError(message);
    } finally {
      setBusyOperation(null);
    }
  };
  const connect = () => {
    if (!snapshot.activeRouteId) {
      setConfirmation({
        title: "当前没有活动路由",
        body: "连接后，新请求会失败，直到你添加或选择路由。",
        confirmLabel: "继续连接 Codex",
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
        title: "替换 ai_router_images 配置？",
        body: "只会替换图片工具配置，其他 Codex 配置不会改动。",
        confirmLabel: "替换并重新连接",
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
      setError(normalizeIpcError(reason).message);
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
        title: "更新断开恢复配置？",
        body: "当前 config.toml 将成为以后断开连接时的恢复目标。更新后 Codex 仍保持断开。",
        details: (
          <SnapshotSummary
            rows={[
              ["当前文件", preview.currentExists ? "存在" : "不存在"],
              [
                "与现有恢复配置相比",
                preview.bytesChanged ? "内容已更改" : "内容未更改",
              ],
              ["文件权限", formatUnixMode(preview.currentUnixMode)],
              [
                "现有恢复目标",
                preview.recoveryTargetExists
                  ? "config.toml 存在"
                  : "断开后删除 config.toml",
              ],
              ["恢复配置更新时间", formatDateTime(preview.recoveryUpdatedAtMs)],
            ]}
          />
        ),
        confirmLabel: "更新恢复配置",
        onConfirm: () => {
          setConfirmation(null);
          void run(
            () => confirmUpdateCodexRecovery(preview.permit),
            "recovery_update",
          );
        },
      });
    } catch (reason) {
      setRecoveryError(normalizeIpcError(reason).message);
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
        title: "恢复首次连接前状态？",
        body: "当前 config.toml 和断开恢复配置都会被原始备份替换，断开后的手动修改将丢失。",
        details: (
          <SnapshotSummary
            rows={[
              ["当前文件", preview.currentExists ? "存在" : "不存在"],
              ["原始配置文件", preview.originalExists ? "存在" : "不存在"],
              [
                "当前恢复目标",
                preview.recoveryTargetExists
                  ? "config.toml 存在"
                  : "断开后删除 config.toml",
              ],
            ]}
          />
        ),
        confirmLabel: "恢复原始备份",
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
      setRecoveryError(normalizeIpcError(reason).message);
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
    ? { label: "不可用", tone: "warning" as const }
    : hasRecoveryConfig
      ? { label: "可用", tone: "success" as const }
      : { label: "尚未创建", tone: "warning" as const };
  return (
    <SettingsPage title="Codex" titleId="codex-title">
      <SettingsSection
        title="本地代理"
        status={
          <SettingsStatus
            tone={proxyStatus === "running" ? "success" : "danger"}
          >
            {proxyStatus === "running"
              ? "运行中"
              : proxyStatus === "port_conflict"
                ? "端口冲突"
                : "不可用"}
          </SettingsStatus>
        }
      >
        <SettingsReadonlyRow label="地址">
          127.0.0.1:{snapshot.proxyPort}
        </SettingsReadonlyRow>
        <SettingsFieldRow
          label="端口"
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
            应用端口
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
        title="Codex 配置"
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
        <SettingsReadonlyRow label="配置文件">
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
              修复图片配置
            </SettingsButton>
          ) : null}
          {snapshot.codexStatus === "not_connected" ? (
            <SettingsButton
              variant="primary"
              type="button"
              disabled={busy}
              onClick={connect}
            >
              一键连接 Codex
            </SettingsButton>
          ) : null}
          {snapshot.codexStatus === "changed" ? (
            <SettingsButton
              variant="primary"
              type="button"
              disabled={busy}
              onClick={() => void run(reconnectCodex)}
            >
              重新连接 Codex
            </SettingsButton>
          ) : null}
          <SettingsButton
            type="button"
            disabled={busy}
            onClick={() => void run(openCodexConfig)}
          >
            <FolderOpen aria-hidden="true" size={16} />
            打开 config.toml
          </SettingsButton>
        </SettingsActionGroup>
        {imageMcpProjectionConflict ? (
          <p className="settings-error codex-config-conflict-message">
            图片工具配置已被修改，自动重连无法继续。
          </p>
        ) : null}
        {imageMcpNameConflict ? (
          <p className="settings-error codex-config-conflict-message">
            首次连接前已存在同名配置，请先重命名或移除。
          </p>
        ) : null}
      </SettingsSection>
      <SettingsSection
        title="断开恢复配置"
        status={
          <SettingsStatus tone={recoveryStatus.tone}>
            {recoveryStatus.label}
          </SettingsStatus>
        }
      >
        <SettingsReadonlyRow label="恢复目标">
          {!hasRecoveryConfig
            ? "尚未创建"
            : snapshot.recoveryConfig.originalExists
              ? "config.toml 存在"
              : "断开后删除 config.toml"}
        </SettingsReadonlyRow>
        <SettingsReadonlyRow label="更新时间">
          {hasRecoveryConfig
            ? formatDateTime(snapshot.recoveryConfig.updatedAtMs)
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
            更新恢复配置
          </SettingsButton>
        </SettingsActionGroup>
        <p className="muted-text codex-recovery-hint">
          {!hasOriginalBackup
            ? "首次连接前没有可用的原始备份。"
            : !disconnected
              ? "断开 Codex 后才能从当前 config.toml 更新。"
              : "断开连接时，将完整恢复这份配置。更新后仍保持断开。"}
        </p>
        <div className="codex-recovery-advanced">
          <div>
            <strong>原始备份</strong>
            <span>
              {hasOriginalBackup
                ? `首次连接前 · ${formatDateTime(snapshot.originalBackup.capturedAtMs)} · 永久保留`
                : "首次连接前没有可用备份"}
            </span>
          </div>
          <SettingsButton
            variant="danger"
            type="button"
            disabled={busy || !disconnected || !hasOriginalBackup}
            onClick={restore}
          >
            恢复首次连接前状态
          </SettingsButton>
        </div>
        {recoveryError ? (
          <p className="settings-error codex-recovery-error" role="alert">
            {recoveryError}
          </p>
        ) : null}
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
    </SettingsPage>
  );
}

function ImageGenerationSettingsSection({
  snapshot,
  focusRequested,
  onFocusConsumed,
}: {
  snapshot: SettingsSnapshotDto;
  focusRequested: boolean;
  onFocusConsumed?: () => void;
}) {
  const queryClient = useQueryClient();
  const titleRef = useRef<HTMLHeadingElement>(null);
  const [enabled, setEnabled] = useState(snapshot.imagesGeneration.enabled);
  const [routeId, setRouteId] = useState(snapshot.imagesGeneration.routeId);
  const [timeoutDraft, setTimeoutDraft] = useState(
    String(snapshot.imagesGeneration.timeoutSecs),
  );
  const [busy, setBusy] = useState(false);
  const [saved, setSaved] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [capacityDraft, setCapacityDraft] = useState(
    String(snapshot.mcpImageCapacity.thresholdMib),
  );
  const [capacityOperation, setCapacityOperation] = useState<
    "saving" | "opening" | "clearing" | null
  >(null);
  const [capacitySaved, setCapacitySaved] = useState(false);
  const [capacityError, setCapacityError] = useState<string | null>(null);
  const [clearConfirmation, setClearConfirmation] =
    useState<SettingsConfirmation | null>(null);
  const capacityThreshold = parseImageCapacityThreshold(capacityDraft);
  const capacityThresholdError =
    capacityThreshold === null ? "请输入 128 至 102400 的整数。" : null;
  const capacityUnchanged =
    capacityThreshold === snapshot.mcpImageCapacity.thresholdMib;
  const capacityBusy = capacityOperation !== null;

  useEffect(() => {
    // Keep the editable draft aligned when the authoritative threshold changes.
    // This is an external snapshot synchronization, not an interaction update.
    // eslint-disable-next-line react-hooks/set-state-in-effect
    setCapacityDraft(String(snapshot.mcpImageCapacity.thresholdMib));
    setCapacitySaved(false);
  }, [snapshot.mcpImageCapacity.thresholdMib]);

  useEffect(() => {
    if (!focusRequested || !titleRef.current) return;
    titleRef.current.scrollIntoView?.({ block: "start" });
    titleRef.current.focus({ preventScroll: true });
    onFocusConsumed?.();
  }, [focusRequested, onFocusConsumed]);
  const selectedRouteExists =
    routeId !== null &&
    snapshot.routes.some((route) => route.routeId === routeId);
  const timeoutSecs = parseImagesGenerationTimeout(timeoutDraft);
  const timeoutError =
    enabled && timeoutSecs === null ? "请输入 600 至 3600 的整数。" : null;
  const unchanged =
    enabled === snapshot.imagesGeneration.enabled &&
    routeId === snapshot.imagesGeneration.routeId &&
    timeoutSecs === snapshot.imagesGeneration.timeoutSecs;
  const persistedRouteExists =
    snapshot.imagesGeneration.routeId !== null &&
    snapshot.routes.some(
      (route) => route.routeId === snapshot.imagesGeneration.routeId,
    );
  const status = !snapshot.imagesGeneration.enabled
    ? { label: "未启用", tone: "neutral" as const }
    : persistedRouteExists
      ? { label: "已启用", tone: "success" as const }
      : { label: "需要选择路由", tone: "warning" as const };

  const updateDraft = (next: {
    enabled?: boolean;
    routeId?: string | null;
  }) => {
    if (next.enabled !== undefined) {
      setEnabled(next.enabled);
      if (!next.enabled) {
        setTimeoutDraft(String(snapshot.imagesGeneration.timeoutSecs));
      }
    }
    if (next.routeId !== undefined) setRouteId(next.routeId);
    setSaved(false);
    setError(null);
  };

  const apply = async () => {
    if (
      busy ||
      unchanged ||
      timeoutSecs === null ||
      (enabled && !selectedRouteExists)
    )
      return;
    setBusy(true);
    setSaved(false);
    setError(null);
    try {
      await updateImagesGenerationSettings({
        enabled,
        routeId: selectedRouteExists ? routeId : null,
        timeoutSecs,
      });
      setTimeoutDraft(String(timeoutSecs));
      setSaved(true);
      await queryClient.invalidateQueries({ queryKey: queryKeys.settings });
    } catch (reason) {
      setError(normalizeIpcError(reason).message);
    } finally {
      setBusy(false);
    }
  };

  const refreshCapacity = async () => {
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: queryKeys.settings }),
      queryClient.invalidateQueries({ queryKey: queryKeys.menu }),
    ]);
  };

  const saveCapacityThreshold = async () => {
    if (capacityBusy || capacityUnchanged || capacityThreshold === null) return;
    setCapacityOperation("saving");
    setCapacitySaved(false);
    setCapacityError(null);
    try {
      await updateMcpImageCapacityThreshold(capacityThreshold);
      setCapacityDraft(String(capacityThreshold));
      setCapacitySaved(true);
      await refreshCapacity();
    } catch (reason) {
      setCapacityError(normalizeIpcError(reason).message);
    } finally {
      setCapacityOperation(null);
    }
  };

  const openImageDirectory = async () => {
    if (capacityBusy) return;
    setCapacityOperation("opening");
    setCapacitySaved(false);
    setCapacityError(null);
    try {
      await openMcpImageDirectory();
    } catch (reason) {
      setCapacityError(normalizeIpcError(reason).message);
    } finally {
      setCapacityOperation(null);
    }
  };

  const clearImages = async () => {
    if (capacityBusy) return;
    setClearConfirmation(null);
    setCapacityOperation("clearing");
    setCapacitySaved(false);
    setCapacityError(null);
    try {
      await clearMcpImages();
      await refreshCapacity();
    } catch (reason) {
      setCapacityError(normalizeIpcError(reason).message);
      await refreshCapacity();
    } finally {
      setCapacityOperation(null);
    }
  };

  return (
    <SettingsSection
      title="图片生成"
      titleId="codex-image-generation-title"
      titleRef={titleRef}
      titleTabIndex={focusRequested ? -1 : undefined}
      titleAccessory={
        <SettingsHelpTooltip label="图片生成说明">
          <strong>模型：gpt-image-2</strong>
          <span>每次生成一张 PNG。</span>
          <span>
            尺寸支持 auto 或 宽x高；两条边都是 16 的倍数，最长边小于
            3,840px，比例不超过 3:1，总像素为 655,360–8,294,400。
          </span>
          <span>
            超过 3,686,400
            像素属于实验性范围。常用尺寸：1024x1024、1536x1024、1024x1536、2048x1152。
          </span>
        </SettingsHelpTooltip>
      }
      status={
        <SettingsStatus tone={status.tone}>{status.label}</SettingsStatus>
      }
    >
      <SettingsFieldRow label="Codex 图片工具">
        <SettingsSwitch
          label="启用"
          checked={enabled}
          disabled={busy}
          onChange={(event) =>
            updateDraft({ enabled: event.currentTarget.checked })
          }
        />
      </SettingsFieldRow>
      <SettingsFieldRow label="图片路由" htmlFor="images-generation-route">
        <SettingsSelect
          id="images-generation-route"
          className="images-generation-route-select"
          aria-label="图片路由"
          value={selectedRouteExists ? routeId : ""}
          disabled={busy || !enabled}
          onChange={(event) =>
            updateDraft({ routeId: event.currentTarget.value || null })
          }
        >
          <option value="">选择路由</option>
          {snapshot.routes.map((route) => (
            <option key={route.routeId} value={route.routeId}>
              {route.name}
            </option>
          ))}
        </SettingsSelect>
      </SettingsFieldRow>
      <SettingsFieldRow
        label="生成等待上限"
        htmlFor="images-generation-timeout"
      >
        <div className="parameter-field">
          <div className="parameter-input-control images-generation-timeout-control">
            <SettingsTextInput
              id="images-generation-timeout"
              type="number"
              min={600}
              max={3600}
              step={1}
              inputMode="numeric"
              value={timeoutDraft}
              disabled={busy || !enabled}
              aria-invalid={timeoutError ? "true" : undefined}
              aria-describedby={
                timeoutError ? "images-generation-timeout-error" : undefined
              }
              onChange={(event) => {
                setTimeoutDraft(event.currentTarget.value);
                setSaved(false);
                setError(null);
              }}
            />
            <span>秒</span>
          </div>
          {timeoutError ? (
            <p
              id="images-generation-timeout-error"
              className="parameter-field-error"
              role="alert"
            >
              {timeoutError}
            </p>
          ) : null}
        </div>
      </SettingsFieldRow>
      <SettingsActionGroup>
        <SettingsButton
          type="button"
          variant="primary"
          disabled={
            busy ||
            unchanged ||
            timeoutSecs === null ||
            (enabled && !selectedRouteExists)
          }
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
      <SettingsDivider />
      <div className="mcp-image-storage-settings">
        <SettingsReadonlyRow label="本地图片">
          {snapshot.mcpImageCapacity.available
            ? formatLocalImageSummary(snapshot.mcpImageCapacity)
            : "—"}
        </SettingsReadonlyRow>
        <SettingsFieldRow
          label="容量提醒"
          htmlFor="mcp-image-capacity-threshold"
          className="mcp-image-capacity-row"
        >
          <div className="mcp-image-capacity-control">
            <SettingsTextInput
              id="mcp-image-capacity-threshold"
              type="number"
              min={128}
              max={102400}
              step={1}
              inputMode="numeric"
              value={capacityDraft}
              disabled={capacityBusy}
              aria-invalid={capacityThresholdError ? "true" : undefined}
              aria-describedby={
                capacityThresholdError
                  ? "mcp-image-capacity-threshold-error"
                  : undefined
              }
              onChange={(event) => {
                setCapacityDraft(event.currentTarget.value);
                setCapacitySaved(false);
                setCapacityError(null);
              }}
            />
            <span className="mcp-image-capacity-unit">MB</span>
            <SettingsButton
              type="button"
              disabled={
                capacityBusy ||
                capacityUnchanged ||
                capacityThreshold === null
              }
              onClick={() => void saveCapacityThreshold()}
            >
              {capacityOperation === "saving" ? (
                <LoaderCircle aria-hidden="true" className="spin" size={15} />
              ) : null}
              保存
            </SettingsButton>
            {capacitySaved ? (
              <SettingsStatus tone="success">已保存</SettingsStatus>
            ) : null}
          </div>
          {capacityThresholdError ? (
            <p
              id="mcp-image-capacity-threshold-error"
              className="parameter-field-error"
              role="alert"
            >
              {capacityThresholdError}
            </p>
          ) : null}
        </SettingsFieldRow>
        {snapshot.mcpImageCapacity.overThreshold ? (
          <p className="mcp-image-capacity-warning" role="status">
            已达到容量提醒阈值，生成图片仍可继续使用。
          </p>
        ) : null}
        {!snapshot.mcpImageCapacity.available ? (
          <p className="settings-error mcp-image-capacity-message" role="alert">
            图片目录暂时无法读取。
          </p>
        ) : null}
        {capacityError ? (
          <p className="settings-error mcp-image-capacity-message" role="alert">
            {capacityError}
          </p>
        ) : null}
        <SettingsActionGroup className="mcp-image-storage-actions">
          <SettingsButton
            type="button"
            disabled={capacityBusy}
            onClick={() => void openImageDirectory()}
          >
            {capacityOperation === "opening" ? (
              <LoaderCircle aria-hidden="true" className="spin" size={15} />
            ) : (
              <FolderOpen aria-hidden="true" size={16} />
            )}
            打开图片目录
          </SettingsButton>
          <SettingsButton
            type="button"
            variant="danger"
            disabled={
              capacityBusy ||
              !snapshot.mcpImageCapacity.available ||
              snapshot.mcpImageCapacity.imageCount === 0
            }
            onClick={() =>
              setClearConfirmation({
                title: "清除生成图片？",
                body: "这些图片会被永久删除。历史任务中仅保存在此目录的图片可能无法再预览、处理或复用。",
                details: `将清除 ${snapshot.mcpImageCapacity.imageCount} 张图片，占用 ${formatImageBytes(snapshot.mcpImageCapacity.bytes, true)}。`,
                confirmLabel: "清除图片",
                destructive: true,
                onConfirm: () => void clearImages(),
              })
            }
          >
            {capacityOperation === "clearing" ? (
              <LoaderCircle aria-hidden="true" className="spin" size={15} />
            ) : (
              <Trash2 aria-hidden="true" size={16} />
            )}
            清除生成图片
          </SettingsButton>
        </SettingsActionGroup>
      </div>
      {clearConfirmation ? (
        <SettingsConfirmDialog
          confirmation={clearConfirmation}
          onCancel={() => setClearConfirmation(null)}
        />
      ) : null}
    </SettingsSection>
  );
}

function imageSettingsKey(snapshot: SettingsSnapshotDto) {
  const routeIds = snapshot.routes.map((route) => route.routeId).join(",");
  return `${snapshot.imagesGeneration.enabled}:${snapshot.imagesGeneration.routeId ?? ""}:${snapshot.imagesGeneration.timeoutSecs}:${routeIds}`;
}

function parseImageCapacityThreshold(value: string): number | null {
  if (!/^\d+$/.test(value)) return null;
  const parsed = Number(value);
  return Number.isSafeInteger(parsed) && parsed >= 128 && parsed <= 102400
    ? parsed
    : null;
}

function formatLocalImageSummary(
  capacity: SettingsSnapshotDto["mcpImageCapacity"],
) {
  return `${capacity.imageCount}张（${formatImageBytes(capacity.bytes)}）`;
}

function formatImageBytes(bytes: number, spaced = false): string {
  const separator = spaced ? " " : "";
  const gib = 1024 ** 3;
  const mib = 1024 ** 2;
  if (bytes >= gib) return `${trimDecimal(bytes / gib, 2)}${separator}G${spaced ? "B" : ""}`;
  if (bytes >= mib) return `${trimDecimal(bytes / mib, 1)}${separator}M${spaced ? "B" : ""}`;
  return `${trimDecimal(bytes / 1024, 1)}${separator}K${spaced ? "B" : ""}`;
}

function trimDecimal(value: number, digits: number): string {
  return value.toFixed(digits).replace(/\.0+$|(?<=\.[0-9]*)0+$/, "");
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

function parseImagesGenerationTimeout(value: string): number | null {
  if (!/^\d+$/.test(value)) return null;
  const parsed = Number(value);
  return Number.isSafeInteger(parsed) && parsed >= 600 && parsed <= 3600
    ? parsed
    : null;
}

function formatDateTime(value: number | null) {
  return value === null
    ? "-"
    : new Intl.DateTimeFormat("zh-CN", {
        dateStyle: "medium",
        timeStyle: "short",
      }).format(value);
}
