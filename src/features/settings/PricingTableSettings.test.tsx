import { QueryClientProvider, type QueryClient } from "@tanstack/react-query";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import {
  createRouterQueryClient,
  queryKeys,
  usePricingTableSnapshot,
} from "../../api/query";
import type { PricingTableDto } from "../../generated";
import { pricingPresentation } from "./pricingPresentation";
import { PricingTableSettings } from "./PricingTableSettings";

const ipc = vi.hoisted(() => ({
  openPricingSource: vi.fn(),
  getPricingTable: vi.fn(),
  syncPricingFromWeb: vi.fn(),
}));

vi.mock("../../api/ipc", () => ({
  isTauriRuntime: () => true,
  openPricingSource: ipc.openPricingSource,
  getPricingTable: ipc.getPricingTable,
  syncPricingFromWeb: ipc.syncPricingFromWeb,
}));

const ROWS: PricingTableDto["rows"] = [
  {
    modelId: "gpt-6-sol",
    band: "short",
    inputMicroUsd: 2_000_000,
    cachedInputMicroUsd: 200_000,
    cacheWriteMicroUsd: 2_500_000,
    outputMicroUsd: 10_000_000,
    source: "official",
  },
  {
    modelId: "gpt-6-sol",
    band: "long",
    inputMicroUsd: 4_000_000,
    cachedInputMicroUsd: 400_000,
    cacheWriteMicroUsd: null,
    outputMicroUsd: 15_000_000,
    source: "official",
  },
  {
    modelId: "gpt-4.1-nano",
    band: "short",
    inputMicroUsd: 200_000,
    cachedInputMicroUsd: 50_000,
    cacheWriteMicroUsd: null,
    outputMicroUsd: 800_000,
    source: "bundled",
  },
];

function snapshot(overrides: Partial<PricingTableDto> = {}): PricingTableDto {
  return {
    rows: ROWS,
    // Local time, so the formatted label is identical in every test time zone.
    syncedAtMs: new Date(2026, 8, 29, 21, 40, 0, 0).getTime(),
    sourceUrl: "https://developers.openai.com/api/docs/pricing/",
    localState: "loaded",
    status: "idle",
    ...overrides,
  };
}

/** The bundled baseline a fresh install renders before the first sync. */
function bundledSnapshot(): PricingTableDto {
  return {
    rows: [ROWS[2]],
    syncedAtMs: null,
    sourceUrl: null,
    localState: "missing",
    status: "idle",
  };
}

function renderSection(value: PricingTableDto | null) {
  const client = createRouterQueryClient();
  return render(
    <QueryClientProvider client={client}>
      <PricingTableSettings snapshot={value} />
    </QueryClientProvider>,
  );
}

/** Renders the section the way the settings window does: through its query. */
function renderLiveSection() {
  const client = createRouterQueryClient();
  function LiveSection() {
    const pricingTable = usePricingTableSnapshot(true);
    return <PricingTableSettings snapshot={pricingTable.data ?? null} />;
  }
  const result = render(
    <QueryClientProvider client={client}>
      <LiveSection />
    </QueryClientProvider>,
  );
  return { client, ...result };
}

function cachedTable(client: QueryClient): PricingTableDto | undefined {
  return client.getQueryData<PricingTableDto>(queryKeys.pricingTable);
}

beforeEach(() => {
  ipc.openPricingSource.mockReset();
  ipc.openPricingSource.mockResolvedValue(undefined);
  ipc.getPricingTable.mockReset();
  ipc.getPricingTable.mockResolvedValue(bundledSnapshot());
  ipc.syncPricingFromWeb.mockReset();
});

describe("pricing presentation", () => {
  it("maps every local and sync state to the approved copy", () => {
    expect(pricingPresentation(null)).toEqual({
      sectionLabel: "未同步",
      sectionTone: "neutral",
      actionText: "尚未同步，当前使用内置价格。",
      actionTone: "neutral",
    });
    expect(pricingPresentation(null, true)).toEqual({
      sectionLabel: "未同步",
      sectionTone: "neutral",
      actionText: "正在访问官网…",
      actionTone: "neutral",
    });
    expect(pricingPresentation(snapshot({ localState: "missing" }))).toEqual({
      sectionLabel: "未同步",
      sectionTone: "neutral",
      actionText: "尚未同步，当前使用内置价格。",
      actionTone: "neutral",
    });
    expect(pricingPresentation(snapshot({ localState: "corrupt" }))).toEqual({
      sectionLabel: "未同步",
      sectionTone: "neutral",
      actionText: "本地价格表不可用，已回退内置",
      actionTone: "neutral",
    });
    expect(pricingPresentation(snapshot())).toEqual({
      sectionLabel: "已同步 · 2026-09-29 21:40",
      sectionTone: "success",
      actionText: "",
      actionTone: "neutral",
    });
    expect(
      pricingPresentation(
        snapshot({ status: "syncing", localState: "missing", syncedAtMs: null }),
      ),
    ).toEqual({
      sectionLabel: "未同步",
      sectionTone: "neutral",
      actionText: "正在访问官网…",
      actionTone: "neutral",
    });
    expect(pricingPresentation(snapshot({ status: "syncing" }))).toEqual({
      sectionLabel: "已同步 · 2026-09-29 21:40",
      sectionTone: "neutral",
      actionText: "正在访问官网…",
      actionTone: "neutral",
    });
    expect(pricingPresentation(snapshot({ status: "error" }))).toEqual({
      sectionLabel: "已同步 · 2026-09-29 21:40",
      sectionTone: "neutral",
      actionText: "同步失败：官网暂时无法访问，已保留上次数据。",
      actionTone: "danger",
    });
  });
});

describe("pricing settings section", () => {
  it("renders the read-only table with exact money formatting", () => {
    renderSection(snapshot());

    expect(
      screen.getByRole("heading", { name: "模型价格" }),
    ).toBeInTheDocument();
    const table = screen.getByRole("table", { name: "模型价格表" });
    const headers = within(table)
      .getAllByRole("columnheader")
      .map((header) => header.textContent);
    expect(headers).toEqual([
      "模型",
      "上下文",
      "输入",
      "缓存读",
      "缓存写",
      "输出",
      "来源",
    ]);
    expect(
      within(table)
        .getAllByRole("columnheader")
        .every((header) => header.getAttribute("scope") === "col"),
    ).toBe(true);

    const rows = within(table).getAllByRole("row").slice(1);
    expect(rows).toHaveLength(3);
    expect(
      rows.map((row) =>
        Array.from(row.querySelectorAll("td")).map((cell) => cell.textContent),
      ),
    ).toEqual([
      ["gpt-6-sol", "短", "$2.00", "$0.20", "$2.50", "$10.00", "官网"],
      ["gpt-6-sol", "长", "$4.00", "$0.40", "—", "$15.00", "官网"],
      ["gpt-4.1-nano", "短", "$0.20", "$0.05", "—", "$0.80", "内置"],
    ]);
    expect(rows[0].querySelector(".num")?.textContent).toBe("$2.00");
  });

  it("uses sub-cent precision for exact official rates", () => {
    renderSection(
      snapshot({
        rows: [
          {
            modelId: "gpt-6-luna",
            band: "short",
            inputMicroUsd: 100_000,
            cachedInputMicroUsd: 10_000,
            cacheWriteMicroUsd: 125_000,
            outputMicroUsd: 500_000,
            source: "official",
          },
          {
            modelId: "gpt-6-luna-4",
            band: "short",
            inputMicroUsd: 62_500,
            cachedInputMicroUsd: 0,
            cacheWriteMicroUsd: null,
            outputMicroUsd: 1,
            source: "official",
          },
        ],
      }),
    );

    const rows = within(screen.getByRole("table")).getAllByRole("row").slice(1);
    expect(
      Array.from(rows[0].querySelectorAll("td")).map((cell) => cell.textContent),
    ).toEqual(["gpt-6-luna", "短", "$0.10", "$0.01", "$0.125", "$0.50", "官网"]);
    expect(
      Array.from(rows[1].querySelectorAll("td")).map((cell) => cell.textContent),
    ).toEqual([
      "gpt-6-luna-4",
      "短",
      "$0.0625",
      "$0.00",
      "—",
      "$0.000001",
      "官网",
    ]);
  });

  it("keeps the sync button disabled while a synchronization is already running", () => {
    renderSection(snapshot({ status: "syncing" }));

    const button = screen.getByRole("button", { name: "同步官网" });
    expect(button).toBeDisabled();
    expect(button.querySelector(".spin")).not.toBeNull();
    expect(
      screen.getByText("正在访问官网…").getAttribute("aria-live"),
    ).toBe("polite");
    expect(screen.getByText("正在访问官网…")).toHaveClass(
      "settings-status-neutral",
    );
  });

  it("announces bounded failure copy without changing the table", () => {
    renderSection(snapshot({ status: "error" }));

    expect(
      screen.getByText("同步失败：官网暂时无法访问，已保留上次数据。"),
    ).toHaveClass("settings-status-danger");
    expect(
      screen.getByText("同步失败：官网暂时无法访问，已保留上次数据。"),
    ).toHaveAttribute("aria-live", "polite");
    expect(
      within(screen.getByRole("table")).getAllByRole("row").slice(1),
    ).toHaveLength(3);
    expect(
      screen.getByRole("button", { name: "同步官网" }).querySelector(".spin"),
    ).toBeNull();
  });

  it("shows only bundled rows when the local table is unusable", () => {
    renderSection(
      snapshot({
        rows: [ROWS[2]],
        syncedAtMs: null,
        sourceUrl: null,
        localState: "corrupt",
      }),
    );

    expect(screen.getByText("未同步")).toHaveClass("settings-status-neutral");
    expect(
      screen.getByText("本地价格表不可用，已回退内置"),
    ).toBeInTheDocument();
    const rows = within(screen.getByRole("table")).getAllByRole("row").slice(1);
    expect(rows).toHaveLength(1);
    expect(within(rows[0]).getByText("内置")).toBeInTheDocument();
  });

  it("renders an empty bundled table before the first snapshot arrives", () => {
    renderSection(null);

    expect(screen.getByText("尚未同步，当前使用内置价格。")).toBeInTheDocument();
    expect(
      within(screen.getByRole("table")).queryAllByRole("row"),
    ).toHaveLength(1);
  });

  it("opens the fixed official pricing page without sending a URL", async () => {
    renderSection(snapshot());

    fireEvent.click(
      screen.getByRole("button", { name: "打开 OpenAI 官方定价页" }),
    );

    await waitFor(() =>
      expect(ipc.openPricingSource).toHaveBeenCalledOnce(),
    );
    expect(ipc.openPricingSource).toHaveBeenCalledWith();
    expect(
      screen.getByRole("button", { name: "打开 OpenAI 官方定价页" }),
    ).toHaveTextContent("developers.openai.com");
  });

  it("stays usable when the external browser cannot open", async () => {
    ipc.openPricingSource.mockRejectedValueOnce(new Error("opener unavailable"));
    renderSection(snapshot());

    const link = screen.getByRole("button", { name: "打开 OpenAI 官方定价页" });
    fireEvent.click(link);

    await waitFor(() => expect(ipc.openPricingSource).toHaveBeenCalledOnce());
    expect(link).toBeEnabled();
    expect(
      screen.getByText("数据来源：", { exact: false }),
    ).toBeInTheDocument();
  });
});

describe("pricing synchronization", () => {
  it("synchronizes once and renders the snapshot the backend answered with", async () => {
    let finishSync: ((value: PricingTableDto) => void) | undefined;
    ipc.syncPricingFromWeb.mockReturnValueOnce(
      new Promise<PricingTableDto>((resolve) => {
        finishSync = resolve;
      }),
    );
    const result = renderLiveSection();
    // Wait for the query to deliver the bundled snapshot before syncing.
    await screen.findByText("内置");

    fireEvent.click(screen.getByRole("button", { name: "同步官网" }));

    const button = screen.getByRole("button", { name: "同步官网" });
    expect(button).toBeDisabled();
    expect(button.querySelector(".spin")).not.toBeNull();
    expect(screen.getByText("正在访问官网…")).toBeInTheDocument();
    // The previous table stays visible while the capture runs.
    expect(within(screen.getByRole("table")).getAllByRole("row")).toHaveLength(
      2,
    );

    const answer = snapshot({
      syncedAtMs: new Date(2026, 8, 30, 8, 5, 0, 0).getTime(),
    });
    await act(async () => {
      finishSync?.(answer);
    });

    expect(ipc.syncPricingFromWeb).toHaveBeenCalledOnce();
    expect(ipc.syncPricingFromWeb).toHaveBeenCalledWith();
    await waitFor(() => expect(cachedTable(result.client)).toEqual(answer));
    expect(await screen.findByText("已同步 · 2026-09-30 08:05")).toHaveClass(
      "settings-status-success",
    );
    expect(screen.getByRole("button", { name: "同步官网" })).toBeEnabled();
    expect(
      screen.getByRole("button", { name: "同步官网" }).querySelector(".spin"),
    ).toBeNull();
    expect(
      within(screen.getByRole("table")).getAllByRole("row").slice(1),
    ).toHaveLength(3);
  });

  it("ignores a repeated click while the first synchronization runs", async () => {
    let finishSync: ((value: PricingTableDto) => void) | undefined;
    ipc.syncPricingFromWeb.mockReturnValueOnce(
      new Promise<PricingTableDto>((resolve) => {
        finishSync = resolve;
      }),
    );
    renderLiveSection();
    await screen.findByText("内置");

    const button = screen.getByRole("button", { name: "同步官网" });
    fireEvent.click(button);
    fireEvent.click(button);

    expect(ipc.syncPricingFromWeb).toHaveBeenCalledOnce();

    await act(async () => {
      finishSync?.(snapshot());
    });
    expect(
      await screen.findByText("已同步 · 2026-09-29 21:40"),
    ).toBeInTheDocument();
  });

  it("shows the reported failure and keeps the table that stays in effect", async () => {
    ipc.getPricingTable.mockReset();
    ipc.getPricingTable.mockResolvedValue(snapshot());
    ipc.syncPricingFromWeb.mockResolvedValue(snapshot({ status: "error" }));
    renderLiveSection();
    await screen.findByText("已同步 · 2026-09-29 21:40");

    fireEvent.click(screen.getByRole("button", { name: "同步官网" }));

    await waitFor(() =>
      expect(
        screen.getByText("同步失败：官网暂时无法访问，已保留上次数据。"),
      ).toHaveClass("settings-status-danger"),
    );
    expect(screen.getByRole("button", { name: "同步官网" })).toBeEnabled();
    expect(
      within(screen.getByRole("table")).getAllByRole("row").slice(1),
    ).toHaveLength(3);
  });
});
