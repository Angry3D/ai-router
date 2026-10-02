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
import type { CodexAuthStatusDto, SettingsSnapshotDto } from "../../generated";
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

type CodexAuthFailure = { code: string; message: string };

type CodexAuthRequest = "export" | "restore";

type CodexAuthErrorCopy = {
  tone: SettingsTone;
  label: string;
  message: string;
  retryKind: "primary" | "secondary" | "none";
};

const codexAuthErrorCopy: Record<string, CodexAuthErrorCopy> = {
  codex_auth_browser_missing: {
    tone: "danger",
    label: "未检测到浏览器",
    message: "未检测到 Chrome 系浏览器。请安装或打开 Google Chrome 后重试。",
    retryKind: "secondary",
  },
  codex_auth_browser_unsupported: {
    tone: "warning",
    label: "浏览器不支持",
    message:
      "检测到未验证的浏览器，当前版本仅支持已验证的 Google Chrome。未做任何修改。",
    retryKind: "none",
  },
  codex_auth_unsupported_encryption: {
    tone: "warning",
    label: "加密格式不支持",
    message:
      "浏览器 Cookie 加密格式暂不支持（可能是浏览器新版本变更）。未做任何修改。",
    retryKind: "none",
  },
  codex_auth_keychain_denied: {
    tone: "warning",
    label: "等待钥匙串授权",
    message:
      "钥匙串授权被拒绝。请重试并在系统弹窗中选择「始终允许」。",
    retryKind: "primary",
  },
  codex_auth_cookie_store_unreadable: {
    tone: "danger",
    label: "无法读取 Cookie",
    message: "无法读取浏览器 Cookie 数据库（权限不足）。",
    retryKind: "secondary",
  },
  codex_auth_profile_not_logged_in: {
    tone: "danger",
    label: "未登录",
    message:
      "未检测到已登录的 Chrome 会话。请先在 Chrome 中登录 chatgpt.com，然后重试。",
    retryKind: "secondary",
  },
  codex_auth_session_fetch_failed: {
    tone: "danger",
    label: "会话获取失败",
    message: "获取 ChatGPT 会话失败（网络异常或被拦截）。未做任何修改。",
    retryKind: "secondary",
  },
  codex_auth_session_invalid: {
    tone: "danger",
    label: "会话无效",
    message: "会话响应缺少必需字段，无法生成凭证。",
    retryKind: "secondary",
  },
  codex_auth_store_mode_unsupported: {
    tone: "warning",
    label: "存储模式不支持",
    message:
      'Codex 当前未使用 file 存储凭证；请把 cli_auth_credentials_store 改为 "file"，保存后重试。',
    retryKind: "none",
  },
  codex_auth_target_conflict: {
    tone: "warning",
    label: "目标已占用",
    message: "目标文件状态异常（符号链接或在写入前被修改），已停止且未覆盖。",
    retryKind: "secondary",
  },
  codex_auth_write_failed: {
    tone: "danger",
    label: "写入失败",
    message: "写入失败，原文件保持不变。",
    retryKind: "secondary",
  },
  codex_auth_restore_failed: {
    tone: "danger",
    label: "还原失败",
    message: "还原失败，原文件保持不变。",
    retryKind: "secondary",
  },
};

function codexAuthStoreModeLabel(mode: CodexAuthStatusDto["storeMode"]) {
  switch (mode) {
    case "keyring":
      return "keyring（系统钥匙串）";
    case "auto":
      return "auto（自动选择，可能不使用 auth.json）";
    case "ephemeral":
      return "ephemeral（临时，退出即失效）";
    default:
      return mode;
  }
}

function codexAuthStoreMessage(status: CodexAuthStatusDto) {
  const mode = codexAuthStoreModeLabel(status.storeMode);
  if (status.managedLocked) {
    return `无法导出：凭证存储方式由企业配置锁定，本机不能改为文件存储（当前：${mode}）。如需在 Codex 中登录，请使用官方登录方式，或联系管理员。`;
  }
  return `无法导出：Codex 当前的凭证存储方式是 ${mode}，不会读取 auth.json。请点下方「打开 Codex 配置」，把 cli_auth_credentials_store 改为 "file"（或删除这一行，默认即 file），保存后回到本窗口即可重新导出。`;
}

function CodexAuthRows({
  status,
  expired,
  showPlan,
}: {
  status: CodexAuthStatusDto;
  expired: boolean;
  showPlan: boolean;
}) {
  return (
    <>
      <SettingsReadonlyRow label="账号">
        {status.accountEmail ?? "—"}
      </SettingsReadonlyRow>
      {showPlan ? (
        <SettingsReadonlyRow label="套餐">
          {status.planType ?? "—"}
        </SettingsReadonlyRow>
      ) : null}
      <SettingsReadonlyRow label="凭证有效期">
        {status.expiresAtMs === null
          ? "—"
          : expired
            ? `已过期（${formatDateTime(status.expiresAtMs)}）`
            : formatDateTime(status.expiresAtMs)}
      </SettingsReadonlyRow>
      <SettingsReadonlyRow label="上次导出">
        {formatDateTime(status.exportedAtMs)}
      </SettingsReadonlyRow>
    </>
  );
}

export function CodexAuthSettingsSection({
  snapshot,
}: CodexAuthSettingsSectionProps) {
  const queryClient = useQueryClient();
  const status = snapshot.codexAuth;
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<CodexAuthFailure | null>(null);
  const [pendingRequest, setPendingRequest] =
    useState<CodexAuthRequest>("export");
  const [confirmation, setConfirmation] = useState<SettingsConfirmation | null>(
    null,
  );

  const run = async (request: CodexAuthRequest) => {
    if (busy) return;
    setBusy(true);
    setFailure(null);
    // 重试 repeats the request that failed — never a neighbouring one.
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
    ? (codexAuthErrorCopy[failure.code] ?? {
        tone: "danger",
        label: "操作失败",
        message: failure.message,
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
      heading = { tone: "neutral", label: "正在读取 Chrome 会话…" };
      break;
    case "store":
      heading = { tone: "warning", label: "存储模式不支持" };
      break;
    case "notice":
    case "error":
      heading = { tone: copy?.tone ?? "danger", label: copy?.label ?? "操作失败" };
      break;
    case "keychain":
      heading = { tone: "warning", label: "等待钥匙串授权" };
      break;
    case "drifted":
      heading = { tone: "warning", label: "检测到外部修改" };
      break;
    case "expired":
      heading = { tone: "warning", label: "凭证已过期" };
      break;
    case "success":
      heading = { tone: "success", label: "已写入" };
      break;
    default:
      heading = { tone: "neutral", label: "未检测" };
  }

  return (
    <SettingsSection
      title="Codex 凭证"
      titleAccessory={
        <SettingsHelpTooltip label="Codex 凭证说明">
          <strong>仅支持 Chrome 系浏览器。</strong>
          <span>
            本功能读取本机 Chrome 的登录 Cookie，在本地生成并替换 Codex
            凭证文件，不上传任何数据。
          </span>
          <span>凭证为一次性：过期后需重新导出。</span>
        </SettingsHelpTooltip>
      }
      status={
        <SettingsStatus tone={heading.tone}>{heading.label}</SettingsStatus>
      }
    >
      {mode === "idle" ? (
        <>
          <p className="muted-text codex-auth-hint">
            从 Chrome 读取已登录的 ChatGPT 会话，并在本地生成 Codex 凭证。
          </p>
          <SettingsActionGroup>
            <SettingsButton
              variant="primary"
              type="button"
              disabled={busy}
              onClick={() => void run("export")}
            >
              获取并替换 Codex 凭证
            </SettingsButton>
          </SettingsActionGroup>
          <p className="muted-text codex-auth-hint">
            首次使用会请求钥匙串授权，请在系统弹窗中选择「始终允许」。
          </p>
        </>
      ) : null}
      {mode === "store" ? (
        <>
          <p className="inline-warning codex-auth-message">
            {codexAuthStoreMessage(status)}
          </p>
          <SettingsActionGroup>
            <SettingsButton variant="primary" type="button" disabled>
              获取并替换 Codex 凭证
            </SettingsButton>
            {status.managedLocked ? null : (
              <SettingsButton
                type="button"
                onClick={() => void openConfigFile()}
              >
                <FolderOpen aria-hidden="true" size={16} />
                打开 Codex 配置
              </SettingsButton>
            )}
          </SettingsActionGroup>
        </>
      ) : null}
      {mode === "keychain" ? (
        <>
          <p className="inline-warning codex-auth-message">
            钥匙串授权被拒绝。请重新点击下方按钮，并在系统弹窗中选择「始终允许」。
          </p>
          <SettingsActionGroup>
            <SettingsButton
              variant="primary"
              type="button"
              disabled={busy}
              onClick={() => void run("export")}
            >
              重试
            </SettingsButton>
          </SettingsActionGroup>
        </>
      ) : null}
      {mode === "notice" ? (
        <>
          <p className="inline-warning codex-auth-message">
            {failure?.code === "codex_auth_store_mode_unsupported"
              ? codexAuthStoreMessage(status)
              : (copy?.message ?? failure?.message)}
          </p>
          {failure?.code === "codex_auth_store_mode_unsupported" &&
          !status.managedLocked ? (
            <SettingsActionGroup>
              <SettingsButton
                type="button"
                onClick={() => void openConfigFile()}
              >
                <FolderOpen aria-hidden="true" size={16} />
                打开 Codex 配置
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
            {copy?.message ?? failure?.message}
          </p>
          <SettingsActionGroup>
            <SettingsButton
              type="button"
              disabled={busy}
              onClick={() => void run(pendingRequest)}
            >
              重试
            </SettingsButton>
          </SettingsActionGroup>
        </>
      ) : null}
      {mode === "busy" ? (
        <SettingsActionGroup>
          <SettingsButton variant="primary" type="button" disabled>
            <LoaderCircle aria-hidden="true" className="spin" size={15} />
            正在读取…
          </SettingsButton>
        </SettingsActionGroup>
      ) : null}
      {mode === "success" || mode === "drifted" || mode === "expired" ? (
        <>
          {mode === "drifted" ? (
            <p className="inline-warning codex-auth-message">
              检测到 ~/.codex/auth.json
              已被外部修改。重新导出将覆盖当前文件；原文件会先备份。
            </p>
          ) : null}
          <CodexAuthRows
            status={status}
            expired={mode === "expired"}
            showPlan={mode === "success"}
          />
          <SettingsActionGroup>
            <SettingsButton
              variant="primary"
              type="button"
              disabled={busy}
              onClick={() => void run("export")}
            >
              重新导出
            </SettingsButton>
            {mode === "success" && status.backupAvailable ? (
              <SettingsButton
                variant="danger"
                type="button"
                disabled={busy}
                onClick={() =>
                  setConfirmation({
                    title: "还原原凭证？",
                    body: "将用上次导出的备份覆盖当前 ~/.codex/auth.json。当前文件不会被保留。",
                    confirmLabel: "还原",
                    destructive: true,
                    onConfirm: () => {
                      setConfirmation(null);
                      void run("restore");
                    },
                  })
                }
              >
                还原原凭证
              </SettingsButton>
            ) : null}
          </SettingsActionGroup>
          {mode === "success" || mode === "expired" ? (
            <p className="muted-text codex-auth-hint">
              请重启 Codex
              使新凭证生效。凭证为一次性：过期后需重新导出。
            </p>
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
