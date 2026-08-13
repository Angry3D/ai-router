import {
  BarChart,
  PieChart,
  type BarSeriesOption,
  type PieSeriesOption,
} from "echarts/charts";
import {
  AriaComponent,
  GridComponent,
  LegendComponent,
  TooltipComponent,
  type AriaComponentOption,
  type GridComponentOption,
  type LegendComponentOption,
  type TooltipComponentOption,
} from "echarts/components";
import {
  init,
  use as registerEChartsModules,
  type ComposeOption,
  type EChartsType,
} from "echarts/core";
import { CanvasRenderer } from "echarts/renderers";
import { useCallback, useEffect, useRef } from "react";

import type {
  UsageStatisticsAttributionDto,
  UsageStatisticsAttributionMetricDto,
  UsageStatisticsBucketDto,
  UsageStatisticsTokensDto,
} from "../../generated";
import {
  boundedBigIntRatios,
  boundedBigIntShares,
  formatDecimalInteger,
  formatStatisticsTokens,
  formatStatisticsUsd,
} from "./usageFormatting";
import { useAppearance } from "../appearance/useAppearance";

registerEChartsModules([
  BarChart,
  PieChart,
  GridComponent,
  TooltipComponent,
  LegendComponent,
  AriaComponent,
  CanvasRenderer,
]);

export type UsageChartOption = ComposeOption<
  | BarSeriesOption
  | PieSeriesOption
  | GridComponentOption
  | TooltipComponentOption
  | LegendComponentOption
  | AriaComponentOption
>;

export interface UsageChartColors {
  primary: string;
  axis: string;
  grid: string;
  text: string;
  muted: string;
  surface: string;
  input: string;
  cachedInput: string;
  cacheWrite: string;
  output: string;
  attribution: string[];
}

const FALLBACK_COLORS: UsageChartColors = {
  primary: "#7eafef",
  axis: "#cdd1d6",
  grid: "#edf0f3",
  text: "#30343a",
  muted: "#7f858b",
  surface: "#ffffff",
  input: "#1f9d62",
  cachedInput: "#0878ee",
  cacheWrite: "#db8b2b",
  output: "#7551a8",
  attribution: [
    "#0878ee",
    "#1f9d62",
    "#7551a8",
    "#db8b2b",
    "#d15d5d",
    "#a7adb5",
  ],
};

const RATIO_MAX = 10_000;
const DONUT_TOOLTIP_LINE_WIDTH = 18;
const DONUT_RADIUS: [number, number] = [40, 64];
const DONUT_CENTER: [string, string] = ["25%", "50%"];
const DONUT_LEGEND_GEOMETRY = {
  orient: "vertical" as const,
  top: "middle",
  right: 0,
  width: "51%",
  itemWidth: 8,
  itemHeight: 8,
  itemGap: 7,
};

function donutLegendTextStyle(colors: UsageChartColors) {
  return {
    color: colors.text,
    fontSize: 10,
    lineHeight: 14,
    width: 105,
    overflow: "truncate" as const,
  };
}

function tooltipTextWidth(value: string) {
  return Array.from(value).reduce(
    (width, character) => width + (/^[\x20-\x7e]$/u.test(character) ? 1 : 2),
    0,
  );
}

function hardWrapTooltipToken(value: string) {
  const lines: string[] = [];
  let line = "";
  let width = 0;

  for (const character of Array.from(value)) {
    const characterWidth = tooltipTextWidth(character);
    if (line && width + characterWidth > DONUT_TOOLTIP_LINE_WIDTH) {
      lines.push(line);
      line = "";
      width = 0;
    }
    line += character;
    width += characterWidth;
  }
  if (line) lines.push(line);
  return lines;
}

function wrapDonutTooltipLabel(value: string) {
  const tokens = value.split(/(?<=[\s\-_/.:])/u);
  const lines: string[] = [];
  let line = "";

  for (const token of tokens) {
    if (tooltipTextWidth(line + token) <= DONUT_TOOLTIP_LINE_WIDTH) {
      line += token;
      continue;
    }
    if (line) lines.push(line.trimEnd());
    const wrappedToken = hardWrapTooltipToken(token);
    lines.push(...wrappedToken.slice(0, -1));
    line = wrappedToken.at(-1) ?? "";
  }
  if (line) lines.push(line.trimEnd());
  return lines.join("\n");
}

function formatDonutTooltip(...lines: string[]) {
  return lines.map(wrapDonutTooltipLabel).join("\n");
}

function donutSeriesGeometry(
  colors: UsageChartColors,
  hasPositiveShare: boolean,
) {
  return {
    radius: DONUT_RADIUS,
    center: DONUT_CENTER,
    // ECharts applies minAngle after its zero-sum check, so zero data must use 0
    // to avoid rendering synthetic slices.
    minAngle: hasPositiveShare ? 1 : 0,
    stillShowZeroSum: false,
    avoidLabelOverlap: true,
    label: { show: false },
    labelLine: { show: false },
    itemStyle: {
      borderColor: colors.surface,
      borderWidth: 1,
    },
  };
}

function cssColor(style: CSSStyleDeclaration, name: string, fallback: string) {
  return style.getPropertyValue(name).trim() || fallback;
}

function resolveUsageChartColors(element: HTMLElement): UsageChartColors {
  const style = getComputedStyle(element);
  return {
    primary: cssColor(
      style,
      "--settings-chart-primary",
      FALLBACK_COLORS.primary,
    ),
    axis: cssColor(style, "--settings-chart-axis", FALLBACK_COLORS.axis),
    grid: cssColor(style, "--settings-chart-grid", FALLBACK_COLORS.grid),
    text: cssColor(style, "--settings-chart-text", FALLBACK_COLORS.text),
    muted: cssColor(style, "--settings-chart-muted", FALLBACK_COLORS.muted),
    surface: cssColor(
      style,
      "--settings-chart-surface",
      FALLBACK_COLORS.surface,
    ),
    input: cssColor(style, "--settings-chart-input", FALLBACK_COLORS.input),
    cachedInput: cssColor(
      style,
      "--settings-chart-cached-input",
      FALLBACK_COLORS.cachedInput,
    ),
    cacheWrite: cssColor(
      style,
      "--settings-chart-cache-write",
      FALLBACK_COLORS.cacheWrite,
    ),
    output: cssColor(style, "--settings-chart-output", FALLBACK_COLORS.output),
    attribution: FALLBACK_COLORS.attribution.map((fallback, index) =>
      cssColor(style, `--settings-chart-attribution-${index + 1}`, fallback),
    ),
  };
}

function metricLabel(metric: UsageStatisticsAttributionMetricDto) {
  return metric === "requests"
    ? "请求"
    : metric === "tokens"
      ? "Token"
      : "费用";
}

function timeChartLabel(metric: UsageStatisticsAttributionMetricDto) {
  return metric === "requests"
    ? "成功请求数量时间柱状图"
    : `成功请求${metricLabel(metric)}时间柱状图`;
}

// Pure builders are exported for focused precision and option-contract tests.
// eslint-disable-next-line react-refresh/only-export-components
export function formatStatisticsMetric(
  value: string,
  metric: UsageStatisticsAttributionMetricDto,
) {
  return metric === "requests"
    ? formatDecimalInteger(value)
    : metric === "tokens"
      ? formatStatisticsTokens(value)
      : formatStatisticsUsd(value);
}

function trendValue(
  bucket: UsageStatisticsBucketDto,
  metric: UsageStatisticsAttributionMetricDto,
) {
  return metric === "requests"
    ? String(bucket.requestCount)
    : metric === "tokens"
      ? bucket.tokens.total
      : bucket.costPicoUsd;
}

// eslint-disable-next-line react-refresh/only-export-components
export function buildUsageTimeChartOption(
  trend: UsageStatisticsBucketDto[],
  metric: UsageStatisticsAttributionMetricDto,
  colors: UsageChartColors,
): UsageChartOption {
  const values = trend.map((bucket) => trendValue(bucket, metric));
  const ratios = boundedBigIntRatios(values);
  const maximum = values.reduce((current, value) => {
    const integer = BigInt(value);
    return integer > current ? integer : current;
  }, 0n);
  const labelInterval = Math.max(0, Math.ceil(trend.length / 6) - 1);

  return {
    animationDuration: 180,
    aria: { enabled: false },
    grid: { left: 44, right: 8, top: 12, bottom: 30 },
    tooltip: {
      trigger: "axis",
      renderMode: "richText",
      confine: true,
      formatter: (params) => {
        const first = Array.isArray(params) ? params[0] : params;
        const index = first?.dataIndex ?? -1;
        const bucket = trend[index];
        const value = values[index];
        return bucket && value !== undefined
          ? `${bucket.label}\n${metricLabel(metric)} ${formatStatisticsMetric(value, metric)}`
          : "";
      },
    },
    xAxis: {
      type: "category",
      data: trend.map((bucket) => bucket.label),
      axisTick: { show: false },
      axisLine: { lineStyle: { color: colors.axis } },
      axisLabel: {
        color: colors.muted,
        fontSize: 10,
        interval: labelInterval,
        hideOverlap: true,
      },
    },
    yAxis: {
      type: "value",
      min: 0,
      max: RATIO_MAX,
      splitNumber: 3,
      axisLabel: {
        color: colors.muted,
        fontSize: 10,
        formatter: (value: number) =>
          formatStatisticsMetric(
            (
              (maximum * BigInt(Math.round(value))) /
              BigInt(RATIO_MAX)
            ).toString(),
            metric,
          ),
      },
      axisLine: { show: true, lineStyle: { color: colors.axis } },
      axisTick: { show: false },
      splitLine: { lineStyle: { color: colors.grid } },
    },
    series: [
      {
        type: "bar",
        name: metricLabel(metric),
        data: ratios,
        barMaxWidth: 24,
        barMinHeight: 3,
        itemStyle: {
          color: colors.primary,
          borderRadius: [3, 3, 0, 0],
        },
      },
    ],
  };
}

function boundedSharePercent(value: string) {
  const parsed = Number.parseFloat(value);
  if (!Number.isFinite(parsed)) return 0;
  return Math.min(100, Math.max(0, parsed));
}

// eslint-disable-next-line react-refresh/only-export-components
export function buildUsageSourceChartOption(
  attribution: UsageStatisticsAttributionDto[],
  metric: UsageStatisticsAttributionMetricDto,
  colors: UsageChartColors,
): UsageChartOption {
  const legendNames = attribution.map((item, index) => `${index}:${item.key}`);
  const shares = attribution.map((item) =>
    boundedSharePercent(item.sharePercent),
  );
  return {
    animationDuration: 180,
    aria: { enabled: false },
    color: colors.attribution,
    tooltip: {
      trigger: "item",
      renderMode: "richText",
      confine: true,
      formatter: (params) => {
        const first = Array.isArray(params) ? params[0] : params;
        const item = attribution[first?.dataIndex ?? -1];
        return item
          ? formatDonutTooltip(
              item.label,
              `${formatStatisticsMetric(item.value, metric)} · ${item.sharePercent}%`,
            )
          : "";
      },
    },
    legend: {
      data: legendNames,
      ...DONUT_LEGEND_GEOMETRY,
      textStyle: donutLegendTextStyle(colors),
      formatter: (name) => {
        const index = legendNames.indexOf(name);
        const item = attribution[index];
        return item
          ? `${item.label}\n${formatStatisticsMetric(item.value, metric)} · ${item.sharePercent}%`
          : name;
      },
    },
    series: [
      {
        type: "pie",
        name: `模型${metricLabel(metric)}`,
        ...donutSeriesGeometry(
          colors,
          shares.some((value) => value > 0),
        ),
        data: attribution.map((item, index) => ({
          name: legendNames[index],
          value: shares[index],
        })),
      },
    ],
  };
}

const TOKEN_PARTS = [
  ["uncachedInput", "未缓存输入", "input"],
  ["cachedInput", "缓存输入", "cachedInput"],
  ["cacheWriteInput", "写入缓存", "cacheWrite"],
  ["output", "输出", "output"],
] as const;

// eslint-disable-next-line react-refresh/only-export-components
export function buildUsageTokenCompositionChartOption(
  tokens: UsageStatisticsTokensDto,
  colors: UsageChartColors,
): UsageChartOption {
  const values = TOKEN_PARTS.map(([key]) => tokens[key]);
  const shares = boundedBigIntShares(values);
  return {
    animationDuration: 180,
    aria: { enabled: false },
    color: TOKEN_PARTS.map(([, , colorKey]) => colors[colorKey]),
    tooltip: {
      trigger: "item",
      renderMode: "richText",
      confine: true,
      formatter: (params) => {
        const first = Array.isArray(params) ? params[0] : params;
        const part = TOKEN_PARTS[first?.dataIndex ?? -1];
        return part
          ? formatDonutTooltip(part[1], formatStatisticsTokens(tokens[part[0]]))
          : "";
      },
    },
    legend: {
      data: TOKEN_PARTS.map(([, label]) => label),
      ...DONUT_LEGEND_GEOMETRY,
      textStyle: donutLegendTextStyle(colors),
      formatter: (name) => {
        const part = TOKEN_PARTS.find(([, label]) => label === name);
        return part
          ? `${part[1]}\n${formatStatisticsTokens(tokens[part[0]])}`
          : name;
      },
    },
    series: [
      {
        type: "pie",
        name: "Token 构成",
        ...donutSeriesGeometry(
          colors,
          shares.some((value) => value > 0),
        ),
        data: TOKEN_PARTS.map(([, label, colorKey], index) => ({
          name: label,
          value: shares[index],
          itemStyle: { color: colors[colorKey] },
        })),
      },
    ],
  };
}

function EChartsHost({
  className,
  label,
  buildOption,
}: {
  className: string;
  label: string;
  buildOption: (colors: UsageChartColors) => UsageChartOption;
}) {
  const elementRef = useRef<HTMLDivElement>(null);
  const chartRef = useRef<EChartsType | null>(null);
  const { resolvedAppearance } = useAppearance();

  useEffect(() => {
    const element = elementRef.current;
    if (!element) return;
    const chart = init(element, undefined, { renderer: "canvas" });
    chartRef.current = chart;
    const observer =
      typeof ResizeObserver === "undefined"
        ? null
        : new ResizeObserver(() => chart.resize());
    observer?.observe(element);
    return () => {
      observer?.disconnect();
      chartRef.current = null;
      chart.dispose();
    };
  }, []);

  useEffect(() => {
    const element = elementRef.current;
    const chart = chartRef.current;
    if (!element || !chart) return;
    chart.setOption(buildOption(resolveUsageChartColors(element)), {
      notMerge: true,
    });
  }, [buildOption, resolvedAppearance]);

  return (
    <div ref={elementRef} className={className} role="img" aria-label={label} />
  );
}

export function UsageTimeChart({
  trend,
  metric,
}: {
  trend: UsageStatisticsBucketDto[];
  metric: UsageStatisticsAttributionMetricDto;
}) {
  const buildOption = useCallback(
    (colors: UsageChartColors) =>
      buildUsageTimeChartOption(trend, metric, colors),
    [metric, trend],
  );
  return (
    <div className="usage-chart-wrap">
      <EChartsHost
        className="usage-chart usage-time-chart"
        label={timeChartLabel(metric)}
        buildOption={buildOption}
      />
      <ul className="sr-only" aria-label="时间图表数据">
        {trend.map((bucket) => {
          const value = trendValue(bucket, metric);
          return (
            <li key={`${bucket.startedAtMs}-${bucket.finishedAtMs}`}>
              {bucket.label}: {formatStatisticsMetric(value, metric)}
            </li>
          );
        })}
      </ul>
    </div>
  );
}

export function UsageSourceChart({
  attribution,
  metric,
}: {
  attribution: UsageStatisticsAttributionDto[];
  metric: UsageStatisticsAttributionMetricDto;
}) {
  const buildOption = useCallback(
    (colors: UsageChartColors) =>
      buildUsageSourceChartOption(attribution, metric, colors),
    [attribution, metric],
  );
  return (
    <div className="usage-chart-wrap">
      <EChartsHost
        className="usage-chart usage-donut-chart usage-source-chart"
        label={`模型${metricLabel(metric)}占比环形图`}
        buildOption={buildOption}
      />
      <ul className="sr-only" aria-label="来源图表数据">
        {attribution.map((item) => (
          <li key={item.key}>
            {item.label}: {formatStatisticsMetric(item.value, metric)}，占比{" "}
            {item.sharePercent}%
          </li>
        ))}
      </ul>
    </div>
  );
}

export function UsageTokenCompositionChart({
  tokens,
}: {
  tokens: UsageStatisticsTokensDto;
}) {
  const buildOption = useCallback(
    (colors: UsageChartColors) =>
      buildUsageTokenCompositionChartOption(tokens, colors),
    [tokens],
  );
  return (
    <div className="usage-chart-wrap">
      <EChartsHost
        className="usage-chart usage-donut-chart usage-token-composition-chart"
        label="成功请求 Token 构成环形图"
        buildOption={buildOption}
      />
      <ul className="sr-only" aria-label="Token 构成图表数据">
        {TOKEN_PARTS.map(([key, label]) => (
          <li key={key}>
            {label}: {formatStatisticsTokens(tokens[key])}
          </li>
        ))}
      </ul>
    </div>
  );
}
