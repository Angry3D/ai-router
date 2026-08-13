import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { UsageHistoryPageDto, UsageHistoryRowDto } from "../../generated";
import { MenuUsagePreview } from "./MenuUsagePreview";

const row: UsageHistoryRowDto = {
  requestId: "preview-request",
  startedAtMs: 1_754_003_000_000,
  finishedAtMs: 1_754_003_012_345,
  routeId: "route-preview",
  routeName: "速览路由",
  requestedModel: "gpt-5.4",
  actualModel: "gpt-5.4",
  reasoningEffort: "high",
  streaming: true,
  completionState: "failed",
  httpStatus: 502,
  tokens: {
    input: 100,
    uncachedInput: 80,
    output: 20,
    total: 120,
    cachedInput: 20,
    cacheWriteInput: 0,
  },
  totalLatencyMs: 12_000,
  firstOutputLatencyMs: 10_000,
  cost: {
    state: "partial",
    amountPicoUsd: "1250000",
    currency: "USD",
    catalogVersion: null,
    serviceTier: null,
    fastStatus: null,
  },
};

function renderPreview(
  options: {
    data?: UsageHistoryPageDto;
    pending?: boolean;
    error?: boolean;
  } = {},
) {
  return render(
    <MenuUsagePreview
      routeName="速览路由"
      phase="open"
      data={options.data}
      pending={options.pending ?? false}
      error={options.error ?? false}
      onPointerEnter={vi.fn()}
      onPointerLeave={vi.fn()}
    />,
  );
}

describe("MenuUsagePreview", () => {
  it("keeps loading geometry bounded with stable skeleton rows", () => {
    const view = renderPreview({ pending: true });
    expect(screen.getByLabelText("正在读取请求记录").children).toHaveLength(10);
    expect(screen.getAllByRole("columnheader")).toHaveLength(4);
    expect(view.container.querySelector(".menu-usage-preview")).toHaveClass(
      "menu-usage-preview-open",
    );
  });

  it("renders safe empty and error states without commands", () => {
    const empty = renderPreview({
      data: { rows: [], nextCursor: null, totalRows: 0 },
    });
    expect(screen.getByText("暂无请求记录")).toBeInTheDocument();
    expect(screen.getAllByRole("columnheader")).toHaveLength(4);
    empty.unmount();

    renderPreview({ error: true });
    expect(screen.getByRole("alert")).toHaveTextContent("请求记录读取失败");
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
  });

  it("renders loaded shared records as a read-only table", () => {
    renderPreview({ data: { rows: [row], nextCursor: null, totalRows: 1 } });
    expect(screen.getByText("gpt-5.4")).toHaveClass(
      "usage-preview-request-model",
    );
    expect(screen.getByText("gpt-5.4").closest("tr")).toHaveClass(
      "usage-preview-row-failed",
    );
    expect(screen.getByText(/high · \d{2}-\d{2} \d{2}:\d{2}/)).toHaveClass(
      "usage-preview-request-time",
    );
    expect(screen.getByText("失败（502）")).toHaveClass("sr-only");
    expect(screen.getByText("10.00 s")).toHaveClass("is-warning");
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
  });
});
