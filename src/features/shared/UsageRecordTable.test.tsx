import { render, screen, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import type { CompletionState, UsageHistoryRowDto } from "../../generated";
import {
  USAGE_PREVIEW_COLUMNS,
  USAGE_SETTINGS_COLUMNS,
  UsageRecordTable,
} from "./UsageRecordTable";

function usageRow(
  overrides: Partial<UsageHistoryRowDto> = {},
): UsageHistoryRowDto {
  return {
    requestId: "request-1",
    startedAtMs: 1_754_003_000_000,
    finishedAtMs: 1_754_003_012_345,
    routeId: "route-1",
    routeName: "测试路由",
    requestedModel: "gpt-5.4",
    actualModel: "gpt-5.4",
    reasoningEffort: "high",
    streaming: true,
    completionState: "completed",
    httpStatus: 200,
    tokens: {
      input: 1_500,
      uncachedInput: 1_000,
      output: 250,
      total: 1_750,
      cachedInput: 500,
      cacheWriteInput: 0,
    },
    totalLatencyMs: 12_500,
    firstOutputLatencyMs: 10_000,
    cost: {
      state: "exact",
      amountPicoUsd: "35758000000",
      currency: "USD",
      catalogVersion: null,
      serviceTier: null,
      fastStatus: "confirmed",
    },
    ...overrides,
  };
}

describe("UsageRecordTable", () => {
  it("keeps the Settings preset and row selection contract", () => {
    let selected = "";
    render(
      <UsageRecordTable
        rows={[usageRow()]}
        columns={USAGE_SETTINGS_COLUMNS}
        onSelectRequest={(requestId) => {
          selected = requestId;
        }}
      />,
    );

    expect(
      screen.getAllByRole("columnheader").map((cell) => cell.textContent),
    ).toEqual([
      "路由",
      "模型",
      "类型/状态",
      "TOKEN",
      "费用",
      "延迟",
      "完成时间",
    ]);
    expect(screen.getByText("流式")).toBeInTheDocument();
    expect(screen.getByText("1K")).toBeInTheDocument();
    expect(screen.getByText("Fast")).toBeInTheDocument();
    expect(screen.getByText("测试路由").closest("tr")).not.toHaveClass(
      "usage-preview-row",
    );
    const link = screen.getByRole("button", { name: /查看请求/ });
    link.click();
    expect(selected).toBe("request-1");
  });

  it("renders the read-only four-column preview without streaming copy", () => {
    render(
      <UsageRecordTable rows={[usageRow()]} columns={USAGE_PREVIEW_COLUMNS} />,
    );

    expect(
      screen.getAllByRole("columnheader").map((cell) => cell.textContent),
    ).toEqual(["请求", "Token", "费用", "首 Token"]);
    expect(screen.getByText("gpt-5.4")).toBeInTheDocument();
    expect(screen.getByTitle("gpt-5.4 · high")).toBeInTheDocument();
    expect(screen.getByText("1.8K")).toBeInTheDocument();
    expect(screen.queryByText("1K")).not.toBeInTheDocument();
    expect(screen.queryByText("250")).not.toBeInTheDocument();
    expect(screen.queryByText("500")).not.toBeInTheDocument();
    expect(screen.getByText("Fast")).toBeInTheDocument();
    expect(screen.getByText("10.00 s")).toHaveClass("is-warning");
    expect(screen.queryByText("首字")).not.toBeInTheDocument();
    expect(screen.queryByText("总耗时")).not.toBeInTheDocument();
    expect(document.querySelector(".usage-latency-bar")).toBeNull();
    expect(screen.queryByText("流式")).not.toBeInTheDocument();
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
    expect(screen.getByText("gpt-5.4")).toHaveClass(
      "usage-preview-request-model",
    );
    expect(screen.getByText("gpt-5.4")).not.toHaveClass(
      "usage-completion-completed",
    );
    expect(screen.getByText("gpt-5.4").closest("tr")).toHaveClass(
      "usage-preview-row-completed",
    );
    expect(screen.getByText("已完成")).toHaveClass("sr-only");
    expect(
      document.querySelector(".usage-preview-request-time"),
    ).toHaveTextContent(/^high · \d{2}-\d{2} \d{2}:\d{2}$/);
  });

  it.each([
    ["completed", 200, "已完成"],
    ["failed", 502, "失败（502）"],
    ["cancelled", null, "已取消"],
    ["no_upstream", 503, "无上游（503）"],
  ] as const)(
    "renders %s through the row rail and hidden canonical status",
    (state, httpStatus, label) => {
      render(
        <UsageRecordTable
          rows={[
            usageRow({ completionState: state as CompletionState, httpStatus }),
          ]}
          columns={USAGE_PREVIEW_COLUMNS}
        />,
      );
      expect(screen.getByText("gpt-5.4")).not.toHaveClass(
        `usage-completion-${state}`,
      );
      expect(screen.getByText("gpt-5.4").closest("tr")).toHaveClass(
        `usage-preview-row-${state}`,
      );
      expect(
        screen.getByText(
          new RegExp(label.replace(/[（）]/g, (value) => `\\${value}`)),
        ),
      ).toHaveClass("sr-only");
      expect(
        document.querySelector(".usage-preview-request-time"),
      ).not.toHaveClass(`usage-completion-${state}`);
    },
  );

  it("preserves missing values, long model text, and the ten-second warning", () => {
    const longModel = "model-with-a-very-long-exact-identifier-for-preview";
    render(
      <UsageRecordTable
        rows={[
          usageRow({
            actualModel: null,
            requestedModel: longModel,
            reasoningEffort: null,
            firstOutputLatencyMs: 10_000,
            totalLatencyMs: null,
            cost: {
              state: "unavailable",
              amountPicoUsd: null,
              currency: "USD",
              catalogVersion: null,
              serviceTier: null,
              fastStatus: null,
            },
          }),
        ]}
        columns={USAGE_PREVIEW_COLUMNS}
      />,
    );

    expect(screen.getByTitle(`${longModel} · -`)).toBeInTheDocument();
    expect(screen.getByText("不可用")).toBeInTheDocument();
    const latency = screen
      .getByText("10.00 s")
      .closest(".usage-first-output-cell");
    expect(latency).not.toBeNull();
    expect(within(latency as HTMLElement).getByText("10.00 s")).toHaveClass(
      "is-warning",
    );
    expect(within(latency as HTMLElement).queryByText("-")).toBeNull();
  });

  it("renders missing total Token and first output without restoring breakdown rows", () => {
    render(
      <UsageRecordTable
        rows={[
          usageRow({
            tokens: {
              input: null,
              uncachedInput: null,
              output: null,
              total: null,
              cachedInput: null,
              cacheWriteInput: null,
            },
            firstOutputLatencyMs: null,
          }),
        ]}
        columns={USAGE_PREVIEW_COLUMNS}
      />,
    );

    expect(
      within(
        document.querySelector(".usage-token-total-cell") as HTMLElement,
      ).getByText("-"),
    ).toBeInTheDocument();
    expect(
      within(
        document.querySelector(".usage-first-output-cell") as HTMLElement,
      ).getByText("-"),
    ).toHaveClass("is-unknown");
    expect(document.querySelector(".usage-token-lines")).toBeNull();
    expect(document.querySelector(".usage-latency-bar")).toBeNull();
  });
});
