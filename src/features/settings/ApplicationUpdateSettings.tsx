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
  ApplicationUpdateProgressDto,
  ApplicationUpdateSnapshotDto,
} from "../../generated";
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
  const [actionError, setActionError] = useState<string | null>(null);
  const [notesExpanded, setNotesExpanded] = useState(false);
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
  const presentation = updatePresentation({
    ...current,
    operation: effectiveOperation,
  });
  const busy =
    checkPending ||
    downloadPending ||
    effectiveOperation === "checking" ||
    effectiveOperation === "downloading" ||
    effectiveOperation === "installing";

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
      setActionError(normalizeIpcError(reason).message);
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
        setActionError(normalizeIpcError(reason).message);
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
      setActionError(normalizeIpcError(reason).message);
    }
  };

  const restart = async () => {
    setConfirmation(null);
    setActionError(null);
    try {
      await restartForApplicationUpdate();
    } catch (reason) {
      setActionError(normalizeIpcError(reason).message);
    }
  };

  const confirmDownload = () => {
    setConfirmation({
      title: "下载并安装应用更新？",
      body: "应用会先校验项目签名，再替换应用文件；安装完成后不会自动重启。",
      confirmLabel: "下载并安装",
      cancelLabel: "取消",
      onConfirm: () => void download(),
    });
  };

  const confirmRestart = () => {
    setConfirmation({
      title: "重启并完成更新？",
      body: "应用会先正常关闭代理与后台服务，然后重新打开更新后的版本。",
      confirmLabel: "重启应用",
      cancelLabel: "稍后",
      onConfirm: () => void restart(),
    });
  };

  return (
    <>
      <SettingsSection
        title="应用更新"
        status={
          <SettingsStatus tone={presentation.tone} aria-live="polite">
            {presentation.label}
          </SettingsStatus>
        }
      >
        <SettingsReadonlyRow label="当前版本">
          {current.currentVersion || "—"}
        </SettingsReadonlyRow>
        {current.lastSuccessfulCheckAtMs !== null ? (
          <SettingsReadonlyRow label="最近检查">
            {formatDateTime(current.lastSuccessfulCheckAtMs)}
          </SettingsReadonlyRow>
        ) : null}
        {current.available ? (
          <SettingsReadonlyRow label="可用版本">
            {current.available.version}
          </SettingsReadonlyRow>
        ) : null}

        {current.available?.notes ? (
          <div className="application-update-notes">
            <span className="settings-field-label">发行说明</span>
            <div className="application-update-notes-content">
              <p className={notesExpanded ? "is-expanded" : ""}>
                {current.available.notes}
              </p>
              {current.available.notes.split("\n").length > 6 ||
              current.available.notes.length > 360 ? (
                <button
                  type="button"
                  className="application-update-notes-toggle"
                  onClick={() => setNotesExpanded((value) => !value)}
                >
                  {notesExpanded ? "收起发行说明" : "展开发行说明"}
                </button>
              ) : null}
            </div>
          </div>
        ) : null}

        {effectiveOperation === "downloading" ||
        effectiveOperation === "installing" ? (
          <UpdateProgress progress={effectiveProgress} />
        ) : null}

        {current.manualFailure ? (
          <p className="application-update-error" role="alert">
            {current.manualFailure.message}
          </p>
        ) : null}
        {actionError ? (
          <p className="application-update-error" role="alert">
            {actionError}
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
              重启并完成更新
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
              {current.manualFailure ? "重试下载" : "下载并安装"}
            </SettingsButton>
          ) : !current.available && effectiveOperation !== "installing" ? (
            <SettingsButton type="button" disabled={busy} onClick={check}>
              {effectiveOperation === "checking" ? (
                <LoaderCircle aria-hidden="true" className="spin" size={15} />
              ) : (
                <RefreshCw aria-hidden="true" size={15} />
              )}
              {effectiveOperation === "checking"
                ? "正在检查"
                : current.lastSuccessfulCheckAtMs === null
                  ? "检查更新"
                  : "再次检查"}
            </SettingsButton>
          ) : null}
          {current.available || current.manualFailure || actionError ? (
            <SettingsButton type="button" onClick={() => void openRelease()}>
              <ExternalLink aria-hidden="true" size={15} />
              查看 GitHub Release
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

function UpdateProgress({
  progress,
}: {
  progress: ApplicationUpdateProgressDto;
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
            ? "正在验证并安装"
            : "更新下载进度"
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
          ? "正在验证并安装"
          : percentage === null
            ? "正在下载"
            : `${percentage}% · ${formatBytes(progress.downloadedBytes ?? 0)} / ${formatBytes(progress.totalBytes ?? 0)}`}
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

function updatePresentation(snapshot: ApplicationUpdateSnapshotDto): {
  label: string;
  tone: SettingsTone;
} {
  if (snapshot.operation === "checking")
    return { label: "正在检查", tone: "neutral" };
  if (snapshot.operation === "downloading")
    return { label: "正在下载", tone: "neutral" };
  if (snapshot.operation === "installing")
    return { label: "正在安装", tone: "neutral" };
  if (snapshot.operation === "restart_ready")
    return { label: "更新已安装，等待重启", tone: "success" };
  if (snapshot.available) return { label: "发现新版本", tone: "warning" };
  if (snapshot.manualFailure) return { label: "检查失败", tone: "danger" };
  if (snapshot.lastSuccessfulCheckAtMs !== null)
    return { label: "已是最新版本", tone: "success" };
  return { label: "尚未检查", tone: "neutral" };
}

function formatDateTime(value: number) {
  return new Intl.DateTimeFormat("zh-CN", {
    dateStyle: "medium",
    timeStyle: "short",
  }).format(value);
}

function formatBytes(bytes: number) {
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}
