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
  PricingTableDto,
  SettingsSnapshotDto,
} from "../../generated";
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
      <CodexAuthSettingsSection snapshot={snapshot} />
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
      <PricingTableSettings snapshot={pricingTable} />
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
