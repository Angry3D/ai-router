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
import type {
  AppLifecycleIssue,
  DatabaseStartupIssue,
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

const databaseIssuePresentation: Record<
  DatabaseStartupIssue,
  { title: string; body: string; retryable: boolean }
> = {
  permission: {
    title: "数据库无法访问",
    body: "AI Router 无法访问数据库或恢复目录。请检查文件权限后重试。",
    retryable: true,
  },
  disk_full: {
    title: "磁盘空间不足",
    body: "释放磁盘空间后再重试。AI Router 不会在空间不足时替换数据库。",
    retryable: true,
  },
  future_schema: {
    title: "数据库版本过新",
    body: "此数据库由更高版本的 AI Router 创建，当前版本不会修改它。",
    retryable: false,
  },
  unsafe_path: {
    title: "数据库路径不安全",
    body: "数据库或恢复目录不是受支持的常规文件路径，AI Router 已停止启动。",
    retryable: false,
  },
  unavailable: {
    title: "数据库暂时不可用",
    body: "AI Router 无法安全打开数据库。代理仍保持停止状态。",
    retryable: true,
  },
};

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
  const [error, setError] = useState<string | null>(null);

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
      setError(normalizeIpcError(reason).message);
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
      setError(normalizeIpcError(reason).message);
    } finally {
      setPending(null);
    }
  };

  return (
    <SettingsPage
      title="恢复数据库"
      titleId="recovery-title"
      className="recovery-page"
    >
      <SettingsSection
        title="需要恢复"
        status={<SettingsStatus tone="danger">代理已停止</SettingsStatus>}
      >
        <p className="muted-text">
          主数据库无法使用。请选择一个本地恢复点，恢复完成并验证通过后代理才会重新启动。
        </p>
      </SettingsSection>

      {snapshot.candidates.length > 0 ? (
        <SettingsSection title="可用恢复点">
          <fieldset
            className="recovery-candidate-list"
            disabled={pending !== null}
          >
            <legend className="sr-only">选择恢复点</legend>
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
                  <strong>{formatDateTime(candidate.createdAtMs)}</strong>
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
              {pending === "restore" ? "正在恢复" : "恢复所选数据库"}
            </SettingsButton>
          </SettingsActionGroup>
        </SettingsSection>
      ) : (
        <SettingsSection title="没有可用恢复点">
          <p className="muted-text">
            未找到通过校验的恢复点。创建空数据库会丢失现有路由、Key 和设置。
          </p>
          {snapshot.canStartOver ? (
            <SettingsButton
              variant="danger"
              type="button"
              disabled={pending !== null}
              onClick={() => setConfirmation("start-over")}
            >
              {pending === "start-over" ? "正在创建" : "创建空数据库"}
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
          重新扫描恢复点
        </SettingsButton>
        <SettingsButton
          type="button"
          disabled={pending !== null}
          onClick={() => void quitApplication()}
        >
          <Power aria-hidden="true" size={16} />
          退出 AI Router
        </SettingsButton>
      </SettingsActionGroup>

      {error ? (
        <p className="settings-error" role="alert">
          {error}
        </p>
      ) : null}

      {confirmation === "restore" && effectiveSelectedPointId ? (
        <SettingsConfirmDialog
          confirmation={{
            title: "恢复所选数据库？",
            body: `将使用 ${formatDateTime(snapshot.candidates.find((candidate) => candidate.pointId === effectiveSelectedPointId)?.createdAtMs ?? null)} 的恢复点替换当前不可用数据库。`,
            confirmLabel: "恢复数据库",
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
            title: "创建空数据库？",
            body: "现有数据库将被隔离保存，但应用会从空路由和新设置开始。此操作不能从界面撤销。",
            confirmLabel: "确认创建空数据库",
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
  const [pending, setPending] = useState(false);
  return (
    <SettingsPage title="恢复数据库" titleId="recovery-load-error-title">
      <SettingsSection
        title="恢复点读取失败"
        status={<SettingsStatus tone="danger">代理已停止</SettingsStatus>}
      >
        <p className="muted-text">
          无法读取安全的恢复点列表。可以重试或退出应用。
        </p>
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
            重试
          </SettingsButton>
          <SettingsButton
            type="button"
            disabled={pending}
            onClick={() => void quitApplication()}
          >
            <Power aria-hidden="true" size={16} />
            退出 AI Router
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
  const presentation =
    databaseIssuePresentation[issue ?? lifecycleDatabaseIssue(lifecycleIssue)];
  const queryClient = useQueryClient();
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
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
      setError(normalizeIpcError(reason).message);
    } finally {
      setPending(false);
    }
  };
  return (
    <SettingsPage title={presentation.title} titleId="database-error-title">
      <SettingsSection
        title="数据库启动失败"
        status={<SettingsStatus tone="danger">代理已停止</SettingsStatus>}
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
              {pending ? "正在重试" : "重试数据库启动"}
            </SettingsButton>
          ) : null}
          <SettingsButton
            type="button"
            disabled={pending}
            onClick={() => void quitApplication()}
          >
            <Power aria-hidden="true" size={16} />
            退出 AI Router
          </SettingsButton>
        </SettingsActionGroup>
      </SettingsSection>
      {error ? (
        <p className="settings-error" role="alert">
          {error}
        </p>
      ) : null}
    </SettingsPage>
  );
}

function formatDateTime(value: number | null) {
  return value === null
    ? "-"
    : new Intl.DateTimeFormat("zh-CN", {
        dateStyle: "medium",
        timeStyle: "short",
      }).format(value);
}
