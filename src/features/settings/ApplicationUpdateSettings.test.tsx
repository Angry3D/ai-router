import { QueryClientProvider } from "@tanstack/react-query";
import {
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { createRouterQueryClient, queryKeys } from "../../api/query";
import type {
  ApplicationUpdateProgressDto,
  ApplicationUpdateSnapshotDto,
} from "../../generated";
import { ApplicationUpdateSettings } from "./ApplicationUpdateSettings";

const ipc = vi.hoisted(() => ({
  check: vi.fn(),
  download: vi.fn(),
  openRelease: vi.fn(),
  restart: vi.fn(),
}));

vi.mock("../../api/ipc", () => ({
  checkApplicationUpdate: ipc.check,
  downloadAndInstallApplicationUpdate: ipc.download,
  normalizeIpcError: () => ({
    code: "test",
    message: "更新操作失败",
    retryable: true,
    field: null,
  }),
  openApplicationUpdateRelease: ipc.openRelease,
  restartForApplicationUpdate: ipc.restart,
}));

const available: ApplicationUpdateSnapshotDto = {
  currentVersion: "0.1.0",
  operation: "idle",
  available: {
    version: "0.2.0",
    notes: "第一项改进\n第二项改进",
    releaseUrl: "https://github.com/Angry3D/ai-router/releases/tag/v0.2.0",
  },
  lastSuccessfulCheckAtMs: 1_725_000_000_000,
  downloadedBytes: null,
  totalBytes: null,
  manualFailure: null,
};

function renderUpdate(snapshot: ApplicationUpdateSnapshotDto | null) {
  const client = createRouterQueryClient();
  if (snapshot) client.setQueryData(queryKeys.applicationUpdate, snapshot);
  return render(
    <QueryClientProvider client={client}>
      <ApplicationUpdateSettings snapshot={snapshot} />
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  ipc.check.mockReset();
  ipc.download.mockReset();
  ipc.openRelease.mockReset();
  ipc.restart.mockReset();
  ipc.openRelease.mockResolvedValue(undefined);
  ipc.restart.mockResolvedValue(undefined);
});

describe("ApplicationUpdateSettings", () => {
  it("shows a stable busy state while a manual check is pending", async () => {
    let finish!: (value: ApplicationUpdateSnapshotDto) => void;
    ipc.check.mockImplementation(
      () =>
        new Promise((resolve) => {
          finish = resolve;
        }),
    );
    const current = {
      ...available,
      available: null,
      lastSuccessfulCheckAtMs: null,
    };
    renderUpdate(current);

    fireEvent.click(screen.getByRole("button", { name: "检查更新" }));
    expect(screen.getByRole("button", { name: "正在检查" })).toBeDisabled();
    expect(screen.getByText("正在检查", { selector: "span" })).toHaveAttribute(
      "aria-live",
      "polite",
    );

    finish({
      ...current,
      lastSuccessfulCheckAtMs: 1_725_000_000_000,
    });
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "再次检查" })).toBeEnabled(),
    );
  });

  it("renders current and available versions as plain text", () => {
    renderUpdate(available);

    expect(
      screen.getByRole("heading", { name: "应用更新" }),
    ).toBeInTheDocument();
    expect(screen.getByText("发现新版本")).toHaveAttribute(
      "aria-live",
      "polite",
    );
    expect(screen.getByText("0.1.0")).toBeInTheDocument();
    expect(screen.getByText("0.2.0")).toBeInTheDocument();
    expect(screen.getByText(/第一项改进/u)).toHaveTextContent(
      "第一项改进 第二项改进",
    );
  });

  it("requires cancel-first confirmation before download and consumes progress", async () => {
    let finish!: (value: ApplicationUpdateSnapshotDto) => void;
    ipc.download.mockImplementation(
      (onProgress: (progress: unknown) => void) =>
        new Promise((resolve) => {
          onProgress({
            operation: "downloading",
            downloadedBytes: 50,
            totalBytes: 100,
          });
          finish = resolve;
        }),
    );
    renderUpdate(available);

    fireEvent.click(screen.getByRole("button", { name: "下载并安装" }));
    const dialog = screen.getByRole("alertdialog", {
      name: "下载并安装应用更新？",
    });
    expect(within(dialog).getByRole("button", { name: "取消" })).toHaveFocus();
    fireEvent.click(within(dialog).getByRole("button", { name: "取消" }));
    expect(ipc.download).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "下载并安装" }));
    fireEvent.click(
      within(
        screen.getByRole("alertdialog", { name: "下载并安装应用更新？" }),
      ).getByRole("button", { name: "下载并安装" }),
    );
    expect(ipc.download).toHaveBeenCalledTimes(1);
    expect(await screen.findByRole("progressbar")).toHaveAttribute(
      "aria-valuenow",
      "50",
    );

    finish({ ...available, operation: "restart_ready" });
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "重启并完成更新" }),
      ).toBeInTheDocument(),
    );
  });

  it("retains a known update on failure and offers the canonical fallback", async () => {
    renderUpdate({
      ...available,
      manualFailure: {
        code: "update_offline",
        message: "暂时无法连接更新服务，请稍后重试。",
        retryable: true,
      },
    });

    expect(screen.getByRole("alert")).toHaveTextContent("暂时无法连接更新服务");
    expect(screen.getByText("0.2.0")).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "重试下载" }),
    ).toBeEnabled();
    fireEvent.click(
      screen.getByRole("button", { name: "查看 GitHub Release" }),
    );
    await waitFor(() => expect(ipc.openRelease).toHaveBeenCalledOnce());
  });

  it("requires a second cancel-first confirmation before restart", async () => {
    renderUpdate({ ...available, operation: "restart_ready" });

    fireEvent.click(screen.getByRole("button", { name: "重启并完成更新" }));
    const dialog = screen.getByRole("alertdialog", {
      name: "重启并完成更新？",
    });
    expect(within(dialog).getByRole("button", { name: "稍后" })).toHaveFocus();
    expect(dialog).toHaveTextContent("正常关闭代理与后台服务");
    fireEvent.click(within(dialog).getByRole("button", { name: "重启应用" }));
    await waitFor(() => expect(ipc.restart).toHaveBeenCalledOnce());
  });

  it("uses textual progress without inventing a percentage for unknown totals or install", () => {
    const downloading = renderUpdate({
      ...available,
      operation: "downloading",
      downloadedBytes: 50,
      totalBytes: null,
    });
    expect(screen.getByRole("progressbar")).not.toHaveAttribute(
      "aria-valuenow",
    );
    expect(screen.getByText("正在下载", { selector: "p" })).toBeInTheDocument();
    downloading.unmount();

    renderUpdate({
      ...available,
      operation: "installing",
      downloadedBytes: 100,
      totalBytes: 100,
    });
    expect(
      screen.getByRole("progressbar", { name: "正在验证并安装" }),
    ).not.toHaveAttribute("aria-valuenow");
    expect(screen.getByText("正在验证并安装", { selector: "p" })).toBeInTheDocument();
  });

  it("ignores progress from an older completed download operation", async () => {
    let staleProgress!: (progress: ApplicationUpdateProgressDto) => void;
    let currentProgress!: (progress: ApplicationUpdateProgressDto) => void;
    let finishCurrent!: (snapshot: ApplicationUpdateSnapshotDto) => void;
    ipc.download
      .mockImplementationOnce(
        (onProgress: (progress: ApplicationUpdateProgressDto) => void) => {
          staleProgress = onProgress;
          return Promise.reject(new Error("synthetic failure"));
        },
      )
      .mockImplementationOnce(
        (onProgress: (progress: ApplicationUpdateProgressDto) => void) =>
          new Promise((resolve) => {
            currentProgress = onProgress;
            finishCurrent = resolve;
          }),
      );
    renderUpdate(available);

    fireEvent.click(screen.getByRole("button", { name: "下载并安装" }));
    fireEvent.click(
      within(
        screen.getByRole("alertdialog", { name: "下载并安装应用更新？" }),
      ).getByRole("button", { name: "下载并安装" }),
    );
    await screen.findByRole("alert");

    fireEvent.click(screen.getByRole("button", { name: "下载并安装" }));
    fireEvent.click(
      within(
        screen.getByRole("alertdialog", { name: "下载并安装应用更新？" }),
      ).getByRole("button", { name: "下载并安装" }),
    );
    currentProgress({
      operation: "downloading",
      downloadedBytes: 20,
      totalBytes: 100,
    });
    staleProgress({
      operation: "downloading",
      downloadedBytes: 90,
      totalBytes: 100,
    });
    expect(await screen.findByRole("progressbar")).toHaveAttribute(
      "aria-valuenow",
      "20",
    );

    finishCurrent({ ...available, operation: "restart_ready" });
    await screen.findByRole("button", { name: "重启并完成更新" });
  });

  it("does not retry a non-retryable signature failure in place", () => {
    renderUpdate({
      ...available,
      manualFailure: {
        code: "update_signature_invalid",
        message: "更新包签名校验失败，未安装任何内容。",
        retryable: false,
      },
    });

    expect(
      screen.queryByRole("button", { name: /下载|重试/u }),
    ).not.toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "查看 GitHub Release" }),
    ).toBeEnabled();
  });
});
