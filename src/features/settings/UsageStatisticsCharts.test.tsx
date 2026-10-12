import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type {
  UsageStatisticsAttributionDto,
  UsageStatisticsBucketDto,
  UsageStatisticsTokensDto,
} from "../../generated";
import {
  AppearanceContext,
  type AppearanceContextValue,
} from "../appearance/useAppearance";
import { usageMessages } from "../language/catalog/usage";

const echarts = vi.hoisted(() => ({
  use: vi.fn(),
  init: vi.fn(),
  setOption: vi.fn(),
  resize: vi.fn(),
  dispose: vi.fn(),
}));

vi.mock("echarts/core", () => ({
  use: echarts.use,
  init: echarts.init,
}));

import {
  UsageSourceChart,
  UsageTimeChart,
  UsageTokenCompositionChart,
  buildUsageSourceChartOption,
  buildUsageTimeChartOption,
  buildUsageTokenCompositionChartOption,
  type UsageChartColors,
} from "./UsageStatisticsCharts";

const usage = usageMessages["zh-CN"];

const colors: UsageChartColors = {
  primary: "primary",
  axis: "axis",
  grid: "grid",
  text: "text",
  muted: "muted",
  surface: "surface",
  input: "input",
  cachedInput: "cached",
  cacheWrite: "write",
  output: "output",
  attribution: ["one", "two", "three", "four", "five", "other"],
};

function withAppearance(
  children: React.ReactNode,
  resolvedAppearance: "light" | "dark",
) {
  const value: AppearanceContextValue = {
    preference: resolvedAppearance,
    resolvedAppearance,
    pending: false,
    error: null,
    setPreference: async () => undefined,
  };
  return (
    <AppearanceContext.Provider value={value}>
      {children}
    </AppearanceContext.Provider>
  );
}

const tokens: UsageStatisticsTokensDto = {
  total: "8",
  uncachedInput: "4",
  cachedInput: "2",
  cacheWriteInput: "1",
  output: "1",
};

const trend: UsageStatisticsBucketDto[] = [
  {
    startedAtMs: Date.UTC(2024, 7, 4),
    finishedAtMs: Date.UTC(2024, 7, 5),
    includeUtcOffset: false,
    requestCount: 5,
    tokens,
    costPicoUsd: "500000000000",
  },
  {
    startedAtMs: Date.UTC(2024, 7, 5),
    finishedAtMs: Date.UTC(2024, 7, 6),
    includeUtcOffset: false,
    requestCount: 10,
    tokens: { ...tokens, total: "16" },
    costPicoUsd: "1000000000000",
  },
];

const timeLabels: Record<number, string> = {
  [Date.UTC(2024, 7, 4)]: "08/04",
  [Date.UTC(2024, 7, 5)]: "08/05",
};

const timeLabelFor = (bucket: UsageStatisticsBucketDto) =>
  timeLabels[bucket.startedAtMs] ?? "";

const attributionLabelFor = (item: UsageStatisticsAttributionDto) =>
  item.label ?? "其他";

const attribution: UsageStatisticsAttributionDto[] = [
  {
    key: "gpt-5",
    label: "gpt-5",
    isOther: false,
    value: "5100000000000",
    sharePercent: "66.7",
    redirectedRequestCount: 0,
    redirectedTotalTokens: "0",
  },
  {
    key: "other",
    label: null,
    isOther: true,
    value: "2500000000000",
    sharePercent: "33.3",
    redirectedRequestCount: 0,
    redirectedTotalTokens: "0",
  },
];

describe("Usage statistics chart options", () => {
  it("builds bounded time bars while preserving formatted tooltip values", () => {
    const option = buildUsageTimeChartOption(
      trend,
      "requests",
      colors,
      usage,
      timeLabelFor,
    );
    expect(option).toMatchObject({
      aria: { enabled: false },
      tooltip: { renderMode: "richText" },
      xAxis: { data: ["08/04", "08/05"] },
      series: [{ type: "bar", data: [5000, 10000] }],
    });
  });

  it("builds a model donut from bounded backend percentages", () => {
    const option = buildUsageSourceChartOption(
      attribution,
      "cost",
      colors,
      usage,
      attributionLabelFor,
    );
    expect(option).toMatchObject({
      aria: { enabled: false },
      tooltip: { renderMode: "richText" },
      legend: {
        orient: "vertical",
        top: "middle",
        right: 0,
        width: "51%",
        itemWidth: 8,
        itemHeight: 8,
        itemGap: 7,
        textStyle: { width: 105, overflow: "truncate" },
      },
      series: [
        {
          type: "pie",
          radius: [40, 64],
          center: ["25%", "50%"],
          minAngle: 1,
          stillShowZeroSum: false,
          data: [{ value: 66.7 }, { value: 33.3 }],
        },
      ],
    });
    const formatter = (
      option.tooltip as { formatter: (value: unknown) => string }
    ).formatter;
    expect(formatter({ dataIndex: 0 })).toBe("gpt-5\n$5.10 · 66.7%");
  });

  it("appends the redirect count to a redirected attribution summary", () => {
    const option = buildUsageSourceChartOption(
      [{ ...attribution[0], redirectedRequestCount: 3 }],
      "cost",
      colors,
      usage,
      attributionLabelFor,
    );
    const legendFormatter = (
      option.legend as { formatter: (name: string) => string }
    ).formatter;
    expect(legendFormatter("0:gpt-5")).toBe("gpt-5\n$5.10 · 66.7% · 转发 3 次");
    const tooltipFormatter = (
      option.tooltip as { formatter: (value: unknown) => string }
    ).formatter;
    expect(tooltipFormatter({ dataIndex: 0 })).toBe(
      "gpt-5\n$5.10 · 66.7% ·\n转发 3 次",
    );

    const withoutRedirects = buildUsageSourceChartOption(
      [attribution[0]],
      "cost",
      colors,
      usage,
      attributionLabelFor,
    );
    const plainFormatter = (
      withoutRedirects.legend as { formatter: (name: string) => string }
    ).formatter;
    expect(plainFormatter("0:gpt-5")).toBe("gpt-5\n$5.10 · 66.7%");
  });

  it("wraps long model names inside the rich-text tooltip", () => {
    const option = buildUsageSourceChartOption(
      [
        {
          ...attribution[0],
          key: "relay-synthetic-preview",
          label: "relay-synthetic-preview",
        },
      ],
      "cost",
      colors,
      usage,
      attributionLabelFor,
    );
    const formatter = (
      option.tooltip as { formatter: (value: unknown) => string }
    ).formatter;
    expect(formatter({ dataIndex: 0 })).toBe(
      "relay-synthetic-\npreview\n$5.10 · 66.7%",
    );
  });

  it("builds a four-part Token donut with shared geometry and stable colors", () => {
    const option = buildUsageTokenCompositionChartOption(
      tokens,
      colors,
      usage,
    );
    expect(option).toMatchObject({
      aria: { enabled: false },
      color: ["input", "cached", "write", "output"],
      tooltip: { renderMode: "richText" },
      legend: {
        orient: "vertical",
        top: "middle",
        right: 0,
        width: "51%",
        itemWidth: 8,
        itemHeight: 8,
        itemGap: 7,
        textStyle: { width: 105, overflow: "truncate" },
      },
      series: [
        {
          type: "pie",
          name: "Token 构成",
          radius: [40, 64],
          center: ["25%", "50%"],
          minAngle: 1,
          stillShowZeroSum: false,
          data: [
            { name: "未缓存输入", value: 5000, itemStyle: { color: "input" } },
            { name: "缓存输入", value: 2500, itemStyle: { color: "cached" } },
            { name: "写入缓存", value: 1250, itemStyle: { color: "write" } },
            { name: "输出", value: 1250, itemStyle: { color: "output" } },
          ],
        },
      ],
    });
    const formatter = (
      option.tooltip as { formatter: (value: unknown) => string }
    ).formatter;
    expect(formatter({ dataIndex: 0 })).toBe("未缓存输入\n0M");
  });

  it("keeps real zero Token values without introducing synthetic slices", () => {
    const option = buildUsageTokenCompositionChartOption(
      {
        total: "0",
        uncachedInput: "0",
        cachedInput: "0",
        cacheWriteInput: "0",
        output: "0",
      },
      colors,
      usage,
    );
    expect(option).toMatchObject({
      series: [
        {
          type: "pie",
          minAngle: 0,
          stillShowZeroSum: false,
          data: [{ value: 0 }, { value: 0 }, { value: 0 }, { value: 0 }],
        },
      ],
    });
  });
});

describe("Usage statistics chart lifecycle", () => {
  beforeEach(() => {
    echarts.init.mockReset();
    echarts.setOption.mockReset();
    echarts.resize.mockReset();
    echarts.dispose.mockReset();
    echarts.init.mockReturnValue({
      setOption: echarts.setOption,
      resize: echarts.resize,
      dispose: echarts.dispose,
    });
  });

  it("updates one chart instance, observes resize, and disposes on unmount", () => {
    let invokeResize = () => {};
    const observe = vi.fn();
    const disconnect = vi.fn();
    vi.stubGlobal(
      "ResizeObserver",
      class {
        constructor(callback: ResizeObserverCallback) {
          invokeResize = () => callback([], this as unknown as ResizeObserver);
        }
        observe = observe;
        disconnect = disconnect;
      },
    );

    const rendered = render(
      withAppearance(
        <UsageTimeChart
          trend={trend}
          metric="requests"
          granularity="day"
          timeZone="UTC"
        />,
        "light",
      ),
    );
    expect(
      screen.getByRole("img", { name: "成功请求数量时间柱状图" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("list", { name: "时间图表数据" }),
    ).toHaveTextContent("08/04: 5");
    expect(echarts.init).toHaveBeenCalledTimes(1);
    expect(echarts.setOption).toHaveBeenCalledTimes(1);
    expect(observe).toHaveBeenCalledTimes(1);

    rendered.rerender(
      withAppearance(
        <UsageTimeChart
          trend={trend}
          metric="tokens"
          granularity="day"
          timeZone="UTC"
        />,
        "light",
      ),
    );
    expect(echarts.init).toHaveBeenCalledTimes(1);
    expect(echarts.setOption).toHaveBeenCalledTimes(2);

    rendered.rerender(
      withAppearance(
        <UsageTimeChart
          trend={trend}
          metric="tokens"
          granularity="day"
          timeZone="UTC"
        />,
        "dark",
      ),
    );
    expect(echarts.init).toHaveBeenCalledTimes(1);
    expect(echarts.setOption).toHaveBeenCalledTimes(3);

    invokeResize();
    expect(echarts.resize).toHaveBeenCalledTimes(1);

    rendered.unmount();
    expect(disconnect).toHaveBeenCalledTimes(1);
    expect(echarts.dispose).toHaveBeenCalledTimes(1);
    vi.unstubAllGlobals();
  });

  it("gives both donuts matching hosts and hidden formatted data lists", () => {
    render(
      <>
        <UsageSourceChart
          attribution={attribution}
          metric="cost"
          dimension="model"
        />
        <UsageTokenCompositionChart tokens={tokens} />
      </>,
    );

    const source = screen.getByRole("img", {
      name: "模型费用占比环形图",
    });
    const token = screen.getByRole("img", {
      name: "成功请求 Token 构成环形图",
    });
    expect(source).toHaveClass("usage-donut-chart");
    expect(token).toHaveClass("usage-donut-chart");
    expect(source.parentElement).toHaveClass("usage-chart-wrap");
    expect(token.parentElement).toHaveClass("usage-chart-wrap");
    expect(
      screen.getByRole("list", { name: "来源图表数据" }),
    ).toHaveTextContent("gpt-5: $5.10，占比 66.7%");
    expect(
      screen.getByRole("list", { name: "来源图表数据" }),
    ).toHaveTextContent("其他: $2.50，占比 33.3%");
    expect(
      screen.getByRole("list", { name: "Token 构成图表数据" }),
    ).toHaveTextContent("未缓存输入: 0M");
  });

  it("resolves a missing attribution identity from the query dimension", () => {
    const missing: UsageStatisticsAttributionDto[] = [
      {
        key: "model:unknown",
        label: null,
        isOther: false,
        value: "1",
        sharePercent: "100.0",
        redirectedRequestCount: 0,
        redirectedTotalTokens: "0",
      },
    ];
    const rendered = render(
      <UsageSourceChart attribution={missing} metric="cost" dimension="model" />,
    );
    expect(
      screen.getByRole("list", { name: "来源图表数据" }),
    ).toHaveTextContent("未知模型: $0.00，占比 100.0%");
    rendered.unmount();
    render(
      <UsageSourceChart
        attribution={missing}
        metric="cost"
        dimension="route"
      />,
    );
    expect(
      screen.getByRole("list", { name: "来源图表数据" }),
    ).toHaveTextContent("未知路由: ");
  });
});
