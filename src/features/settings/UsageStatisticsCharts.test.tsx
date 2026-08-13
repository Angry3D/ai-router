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
    startedAtMs: 1,
    finishedAtMs: 2,
    label: "08/04",
    requestCount: 5,
    tokens,
    costPicoUsd: "500000000000",
  },
  {
    startedAtMs: 2,
    finishedAtMs: 3,
    label: "08/05",
    requestCount: 10,
    tokens: { ...tokens, total: "16" },
    costPicoUsd: "1000000000000",
  },
];

const attribution: UsageStatisticsAttributionDto[] = [
  {
    key: "gpt-5",
    label: "gpt-5",
    isOther: false,
    value: "5100000000000",
    sharePercent: "66.7",
  },
  {
    key: "other",
    label: "其他",
    isOther: true,
    value: "2500000000000",
    sharePercent: "33.3",
  },
];

describe("Usage statistics chart options", () => {
  it("builds bounded time bars while preserving formatted tooltip values", () => {
    const option = buildUsageTimeChartOption(trend, "requests", colors);
    expect(option).toMatchObject({
      aria: { enabled: false },
      tooltip: { renderMode: "richText" },
      xAxis: { data: ["08/04", "08/05"] },
      series: [{ type: "bar", data: [5000, 10000] }],
    });
  });

  it("builds a model donut from bounded backend percentages", () => {
    const option = buildUsageSourceChartOption(attribution, "cost", colors);
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
    );
    const formatter = (
      option.tooltip as { formatter: (value: unknown) => string }
    ).formatter;
    expect(formatter({ dataIndex: 0 })).toBe(
      "relay-synthetic-\npreview\n$5.10 · 66.7%",
    );
  });

  it("builds a four-part Token donut with shared geometry and stable colors", () => {
    const option = buildUsageTokenCompositionChartOption(tokens, colors);
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
        <UsageTimeChart trend={trend} metric="requests" />,
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
        <UsageTimeChart trend={trend} metric="tokens" />,
        "light",
      ),
    );
    expect(echarts.init).toHaveBeenCalledTimes(1);
    expect(echarts.setOption).toHaveBeenCalledTimes(2);

    rendered.rerender(
      withAppearance(
        <UsageTimeChart trend={trend} metric="tokens" />,
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
        <UsageSourceChart attribution={attribution} metric="cost" />
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
      screen.getByRole("list", { name: "Token 构成图表数据" }),
    ).toHaveTextContent("未缓存输入: 0M");
  });
});
