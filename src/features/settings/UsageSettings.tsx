import {
  ArrowRight,
  ArrowLeft,
  CircleStop,
  ChevronLeft,
  ChevronRight,
  DatabaseZap,
  LockKeyhole,
  RefreshCw,
  RefreshCwOff,
  RotateCw,
  RotateCcw,
} from "lucide-react";
import { useMemo, useState } from "react";

import {
  useUsageHistory,
  useUsageRequestDetail,
  useUsageRouteOptions,
  useUsageStatistics,
} from "../../api/query";
import type {
  CompletionState,
  FallbackStopReasonDto,
  UsageCostDto,
  UsageHistoryCursorDto,
  UsageHistoryQueryDto,
  RoutingDecisionDto,
  UsageStatisticsAttributionMetricDto,
  UsageStatisticsDto,
  UsageStatisticsQueryDto,
  UsageTokensDto,
} from "../../generated";
import { UsageRecordTable } from "../shared/UsageRecordTable";
import { USAGE_SETTINGS_COLUMNS } from "../shared/usageRecordColumns";
import { formatUsageDateTime } from "../shared/usageRecordFormatting";
import {
  SettingsButton,
  SettingsIconButton,
  SettingsPage,
  SettingsStatus,
  SettingsTextInput,
} from "./SettingsPrimitives";
import { AppScrollArea } from "../shared/AppScrollArea";
import {
  UsageSourceChart,
  UsageTimeChart,
  UsageTokenCompositionChart,
} from "./UsageStatisticsCharts";
import {
  formatDecimalInteger,
  formatLatency,
  formatStatisticsTokens,
  formatStatisticsUsd,
  formatUsd,
} from "./usageFormatting";

type RollingTimeRange = "24h" | "7d" | "30d";
type TimeRange = "today" | "yesterday" | RollingTimeRange | "all";
type UsageTab = "records" | "statistics";

interface UsageFilters {
  range: TimeRange;
  status: CompletionState | "all";
  routeId: string;
  model: string;
}

const PAGE_SIZE = 50;
const DEFAULT_FILTERS: UsageFilters = {
  range: "7d",
  status: "all",
  routeId: "",
  model: "",
};
const RANGE_MS: Record<RollingTimeRange, number> = {
  "24h": 24 * 60 * 60 * 1_000,
  "7d": 7 * 24 * 60 * 60 * 1_000,
  "30d": 30 * 24 * 60 * 60 * 1_000,
};

function timeRangeBounds(range: TimeRange, anchorMs: number) {
  if (range === "all") {
    return { afterMs: null, beforeMs: anchorMs };
  }
  if (range === "today" || range === "yesterday") {
    const todayStart = new Date(anchorMs);
    todayStart.setHours(0, 0, 0, 0);

    if (range === "today") {
      return { afterMs: todayStart.getTime(), beforeMs: anchorMs };
    }

    const yesterdayStart = new Date(todayStart);
    yesterdayStart.setDate(yesterdayStart.getDate() - 1);
    return {
      afterMs: yesterdayStart.getTime(),
      beforeMs: todayStart.getTime() - 1,
    };
  }
  return { afterMs: anchorMs - RANGE_MS[range], beforeMs: anchorMs };
}

const completionLabels: Record<CompletionState, string> = {
  completed: "已完成",
  failed: "失败",
  cancelled: "已取消",
  no_upstream: "无上游",
};

function attemptStatusLabel(
  httpStatus: number | null,
  errorCategory: string | null,
) {
  const parts: string[] = [];
  if (httpStatus !== null) parts.push(`HTTP ${httpStatus}`);
  if (errorCategory !== null) {
    parts.push(
      errorCategory === "upstream_access_denied" ? "访问拒绝" : errorCategory,
    );
  }
  return parts.length > 0 ? parts.join(" · ") : "-";
}

export function UsageSettings() {
  const [draftFilters, setDraftFilters] =
    useState<UsageFilters>(DEFAULT_FILTERS);
  const [appliedFilters, setAppliedFilters] =
    useState<UsageFilters>(DEFAULT_FILTERS);
  const [anchorMs, setAnchorMs] = useState(() => Date.now());
  const [cursor, setCursor] = useState<UsageHistoryCursorDto | null>(null);
  const [previous, setPrevious] = useState<Array<UsageHistoryCursorDto | null>>(
    [],
  );
  const [selectedRequestId, setSelectedRequestId] = useState<string | null>(
    null,
  );
  const [activeTab, setActiveTab] = useState<UsageTab>("records");
  const [tabStop, setTabStop] = useState<UsageTab>("records");
  const [attributionMetric, setAttributionMetric] =
    useState<UsageStatisticsAttributionMetricDto>("requests");
  const [trendMetric, setTrendMetric] =
    useState<UsageStatisticsAttributionMetricDto>("requests");
  const [retainedStatistics, setRetainedStatistics] =
    useState<UsageStatisticsDto>();
  const routeOptions = useUsageRouteOptions();
  const rangeBounds = useMemo(
    () => timeRangeBounds(appliedFilters.range, anchorMs),
    [anchorMs, appliedFilters.range],
  );

  const resetPagination = () => {
    setCursor(null);
    setPrevious([]);
    setSelectedRequestId(null);
  };
  const applyFilters = (filters: UsageFilters) => {
    const normalized = { ...filters, model: filters.model.trim() };
    setAppliedFilters(normalized);
    setDraftFilters(normalized);
    setAnchorMs((current) => Math.max(Date.now(), current + 1));
    setRetainedStatistics(undefined);
    resetPagination();
  };
  const query = useMemo<UsageHistoryQueryDto>(
    () => ({
      finishedAtOrAfterMs: rangeBounds.afterMs,
      finishedAtOrBeforeMs: rangeBounds.beforeMs,
      completionState:
        appliedFilters.status === "all" ? null : appliedFilters.status,
      routeId: appliedFilters.routeId === "" ? null : appliedFilters.routeId,
      modelContains: appliedFilters.model === "" ? null : appliedFilters.model,
      cursor,
      limit: PAGE_SIZE,
    }),
    [appliedFilters, cursor, rangeBounds],
  );
  const history = useUsageHistory(query);
  const statisticsQuery = useMemo<UsageStatisticsQueryDto>(
    () => ({
      finishedAtOrAfterMs: rangeBounds.afterMs,
      finishedAtOrBeforeMs: rangeBounds.beforeMs,
      routeId: appliedFilters.routeId === "" ? null : appliedFilters.routeId,
      modelContains: appliedFilters.model === "" ? null : appliedFilters.model,
      timeZone: Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC",
      attributionDimension: "model",
      attributionMetric,
    }),
    [appliedFilters, attributionMetric, rangeBounds],
  );
  const statisticsDisabled =
    appliedFilters.status !== "all" && appliedFilters.status !== "completed";
  const statistics = useUsageStatistics(
    statisticsQuery,
    activeTab === "statistics" && !statisticsDisabled,
  );
  const visibleStatistics = statistics.data ?? retainedStatistics;
  const changeAttributionMetric = (
    value: UsageStatisticsAttributionMetricDto,
  ) => {
    setRetainedStatistics(statistics.data);
    setAttributionMetric(value);
  };
  const detail = useUsageRequestDetail(selectedRequestId);
  const currentPage = previous.length + 1;
  const totalRows = history.data?.totalRows ?? 0;
  const totalPages = Math.max(1, Math.ceil(totalRows / PAGE_SIZE));
  const activeViewIsRefreshing =
    activeTab === "records"
      ? history.isRefetching && history.data !== undefined
      : statistics.isFetching && visibleStatistics !== undefined;

  return (
    <SettingsPage title="用量" titleId="usage-title" className="usage-page">
      <form
        className="usage-filters"
        aria-label="用量筛选"
        onSubmit={(event) => {
          event.preventDefault();
          applyFilters(draftFilters);
        }}
      >
        <div className="usage-filter-grid">
          <label>
            <span>时间</span>
            <select
              aria-label="时间范围"
              value={draftFilters.range}
              onChange={(event) => {
                const range = event.currentTarget.value as TimeRange;
                setDraftFilters((filters) => ({
                  ...filters,
                  range,
                }));
              }}
            >
              <option value="today">今天</option>
              <option value="yesterday">昨天</option>
              <option value="24h">最近 24 小时</option>
              <option value="7d">最近 7 天</option>
              <option value="30d">最近 30 天</option>
              <option value="all">全部记录</option>
            </select>
          </label>
          <label>
            <span>状态</span>
            <select
              aria-label="完成状态"
              value={draftFilters.status}
              onChange={(event) => {
                const status = event.currentTarget.value as
                  CompletionState | "all";
                setDraftFilters((filters) => ({
                  ...filters,
                  status,
                }));
              }}
            >
              <option value="all">全部状态</option>
              {Object.entries(completionLabels).map(([value, label]) => (
                <option value={value} key={value}>
                  {label}
                </option>
              ))}
            </select>
          </label>
          <label>
            <span>路由</span>
            <select
              aria-label="路由"
              value={draftFilters.routeId}
              disabled={routeOptions.isPending || routeOptions.isError}
              onChange={(event) => {
                const routeId = event.currentTarget.value;
                setDraftFilters((filters) => ({
                  ...filters,
                  routeId,
                }));
              }}
            >
              {routeOptions.isPending ? (
                <option value="">正在读取路由...</option>
              ) : null}
              {routeOptions.isError ? (
                <option value="">路由读取失败</option>
              ) : null}
              {!routeOptions.isPending && !routeOptions.isError ? (
                <option value="">全部路由</option>
              ) : null}
              {(routeOptions.data ?? []).map((route) => (
                <option value={route.routeId} key={route.routeId}>
                  {route.name}
                  {route.retained ? "（已删除）" : ""}
                </option>
              ))}
            </select>
          </label>
          <label>
            <span>模型</span>
            <SettingsTextInput
              aria-label="模型包含"
              maxLength={256}
              value={draftFilters.model}
              placeholder="模型 ID 片段"
              onChange={(event) => {
                const model = event.currentTarget.value;
                setDraftFilters((filters) => ({
                  ...filters,
                  model,
                }));
              }}
            />
          </label>
        </div>
        {routeOptions.isError ? (
          <span className="usage-route-error" role="alert">
            路由选项读取失败。
            <button type="button" onClick={() => void routeOptions.refetch()}>
              重试
            </button>
          </span>
        ) : null}
        <div className="usage-filter-toolbar">
          <div className="usage-tabs" role="tablist" aria-label="用量视图">
            <button
              id="usage-records-tab"
              type="button"
              role="tab"
              aria-controls="usage-records-panel"
              aria-selected={activeTab === "records"}
              tabIndex={tabStop === "records" ? 0 : -1}
              className={activeTab === "records" ? "is-active" : ""}
              onFocus={() => setTabStop("records")}
              onClick={() => {
                setActiveTab("records");
                setTabStop("records");
              }}
              onKeyDown={(event) => {
                if (event.key !== "ArrowLeft" && event.key !== "ArrowRight")
                  return;
                event.preventDefault();
                setTabStop("statistics");
                document.getElementById("usage-statistics-tab")?.focus();
              }}
            >
              请求记录
            </button>
            <button
              id="usage-statistics-tab"
              type="button"
              role="tab"
              aria-controls="usage-statistics-panel"
              aria-selected={activeTab === "statistics"}
              tabIndex={tabStop === "statistics" ? 0 : -1}
              className={activeTab === "statistics" ? "is-active" : ""}
              onFocus={() => setTabStop("statistics")}
              onClick={() => {
                setActiveTab("statistics");
                setTabStop("statistics");
              }}
              onKeyDown={(event) => {
                if (event.key !== "ArrowLeft" && event.key !== "ArrowRight")
                  return;
                event.preventDefault();
                setTabStop("records");
                document.getElementById("usage-records-tab")?.focus();
              }}
            >
              用量统计
            </button>
          </div>
          <div className="usage-filter-actions">
            <SettingsButton
              type="button"
              onClick={() => applyFilters(DEFAULT_FILTERS)}
            >
              <RotateCcw aria-hidden="true" size={14} />
              重置
            </SettingsButton>
            <SettingsButton
              type="submit"
              variant="primary"
              aria-busy={activeViewIsRefreshing || undefined}
            >
              <RefreshCw
                aria-hidden="true"
                className={activeViewIsRefreshing ? "spin" : undefined}
                size={14}
              />
              刷新
            </SettingsButton>
            <span className="sr-only" role="status" aria-live="polite">
              {activeViewIsRefreshing ? "正在更新..." : ""}
            </span>
          </div>
        </div>
      </form>

      {activeTab === "records" ? (
        <div
          id="usage-records-panel"
          className="usage-tab-panel"
          role="tabpanel"
          aria-labelledby="usage-records-tab"
        >
          {selectedRequestId !== null ? (
            <>
              <div className="usage-detail-toolbar">
                <SettingsIconButton
                  type="button"
                  label="返回请求列表"
                  onClick={() => setSelectedRequestId(null)}
                >
                  <ArrowLeft aria-hidden="true" size={17} />
                </SettingsIconButton>
                <strong>请求详情</strong>
              </div>
              {detail.isPending ? (
                <UsageMessage>正在读取请求详情...</UsageMessage>
              ) : null}
              {detail.isError ? (
                <UsageMessage tone="danger">请求详情读取失败。</UsageMessage>
              ) : null}
              {detail.data ? <UsageDetail detail={detail.data} /> : null}
            </>
          ) : null}
          {selectedRequestId === null ? (
            <>
              {history.isPending ? (
                <UsageMessage>正在读取请求记录...</UsageMessage>
              ) : null}
              {history.isError ? (
                <UsageMessage tone="danger">
                  <span>请求记录读取失败。</span>
                  <SettingsButton
                    type="button"
                    onClick={() => void history.refetch()}
                  >
                    <RefreshCw aria-hidden="true" size={15} />
                    重试
                  </SettingsButton>
                </UsageMessage>
              ) : null}
              {history.data?.rows.length === 0 ? (
                <UsageMessage>当前筛选条件下没有请求记录。</UsageMessage>
              ) : null}
              {history.data && history.data.rows.length > 0 ? (
                <AppScrollArea
                  axis="both"
                  className="usage-table-wrap"
                  viewportClassName="usage-table-wrap-viewport"
                >
                  <UsageRecordTable
                    rows={history.data.rows}
                    columns={USAGE_SETTINGS_COLUMNS}
                    onSelectRequest={setSelectedRequestId}
                  />
                </AppScrollArea>
              ) : null}

              <nav className="usage-pagination" aria-label="请求记录分页">
                <SettingsButton
                  type="button"
                  disabled={previous.length === 0 || history.isFetching}
                  onClick={() => {
                    const next = previous.slice();
                    setCursor(next.pop() ?? null);
                    setPrevious(next);
                  }}
                >
                  <ChevronLeft aria-hidden="true" size={15} />
                  上一页
                </SettingsButton>
                <span className="usage-page-position" aria-live="polite">
                  第 {currentPage} / {totalPages} 页，共{" "}
                  {totalRows.toLocaleString()} 条
                </span>
                <SettingsButton
                  type="button"
                  disabled={
                    history.data?.nextCursor == null || history.isFetching
                  }
                  onClick={() => {
                    if (!history.data?.nextCursor) return;
                    setPrevious((values) => [...values, cursor]);
                    setCursor(history.data.nextCursor);
                  }}
                >
                  下一页
                  <ChevronRight aria-hidden="true" size={15} />
                </SettingsButton>
              </nav>
            </>
          ) : null}
        </div>
      ) : (
        <div
          id="usage-statistics-panel"
          className="usage-tab-panel usage-statistics-panel-shell"
          role="tabpanel"
          aria-labelledby="usage-statistics-tab"
        >
          <AppScrollArea
            className="usage-statistics-panel"
            viewportClassName="usage-statistics-panel-viewport"
          >
            <UsageStatisticsView
              data={visibleStatistics}
              isPending={
                statistics.isPending && visibleStatistics === undefined
              }
              isError={statistics.isError}
              disabled={statisticsDisabled}
              onRetry={() => void statistics.refetch()}
              metric={attributionMetric}
              trendMetric={trendMetric}
              onMetricChange={changeAttributionMetric}
              onTrendMetricChange={setTrendMetric}
            />
          </AppScrollArea>
        </div>
      )}
    </SettingsPage>
  );
}

function UsageStatisticsView({
  data,
  isPending,
  isError,
  disabled,
  onRetry,
  metric,
  trendMetric,
  onMetricChange,
  onTrendMetricChange,
}: {
  data: UsageStatisticsDto | undefined;
  isPending: boolean;
  isError: boolean;
  disabled: boolean;
  onRetry: () => void;
  metric: UsageStatisticsAttributionMetricDto;
  trendMetric: UsageStatisticsAttributionMetricDto;
  onMetricChange: (value: UsageStatisticsAttributionMetricDto) => void;
  onTrendMetricChange: (value: UsageStatisticsAttributionMetricDto) => void;
}) {
  if (disabled) {
    return <UsageMessage>用量统计只统计已完成请求。</UsageMessage>;
  }
  if (isPending) return <UsageMessage>正在读取用量统计...</UsageMessage>;
  if (isError) {
    return (
      <UsageMessage tone="danger">
        <span>用量统计读取失败。</span>
        <SettingsButton type="button" onClick={onRetry}>
          <RefreshCw aria-hidden="true" size={15} />
          重试
        </SettingsButton>
      </UsageMessage>
    );
  }
  if (!data) return null;
  return (
    <section className="usage-statistics" aria-label="用量统计">
      <div className="usage-statistics-summary">
        <div>
          <span>成功请求</span>
          <strong>
            {formatDecimalInteger(String(data.matchedRequestCount))}
          </strong>
          <small>匹配当前筛选条件</small>
        </div>
        <div>
          <span>Token</span>
          <strong>{formatStatisticsTokens(data.tokens.total)}</strong>
          <dl className="usage-statistics-token-breakdown">
            <div className="is-input">
              <dt>未缓存输入</dt>
              <dd>{formatStatisticsTokens(data.tokens.uncachedInput)}</dd>
            </div>
            <div className="is-cache">
              <dt>缓存输入</dt>
              <dd>{formatStatisticsTokens(data.tokens.cachedInput)}</dd>
            </div>
            <div className="is-write">
              <dt>写入缓存</dt>
              <dd>{formatStatisticsTokens(data.tokens.cacheWriteInput)}</dd>
            </div>
            <div className="is-output">
              <dt>输出</dt>
              <dd>{formatStatisticsTokens(data.tokens.output)}</dd>
            </div>
          </dl>
        </div>
        <div>
          <span>费用</span>
          <strong>{formatStatisticsUsd(data.costPicoUsd)}</strong>
          <small>已记录的上游费用</small>
        </div>
      </div>
      {data.trend.length === 0 ? (
        <UsageMessage>当前筛选条件下没有已完成请求。</UsageMessage>
      ) : (
        <div className="usage-statistics-analysis">
          <section
            className="usage-statistics-section usage-statistics-time-section"
            aria-labelledby="usage-trend-title"
          >
            <div className="usage-statistics-section-heading">
              <h3 id="usage-trend-title">按时间</h3>
              <StatisticsSegmentedControl
                label="趋势指标"
                name="usage-trend-metric"
                values={["requests", "tokens", "cost"]}
                selected={trendMetric}
                labelFor={metricLabel}
                onChange={onTrendMetricChange}
              />
            </div>
            <UsageTimeChart trend={data.trend} metric={trendMetric} />
          </section>

          <div className="usage-statistics-lower-grid">
            <section
              className="usage-statistics-section"
              aria-labelledby="usage-attribution-title"
            >
              <div className="usage-statistics-section-heading">
                <h3 id="usage-attribution-title">按来源</h3>
                <StatisticsSegmentedControl
                  label="来源指标"
                  name="usage-attribution-metric"
                  values={["requests", "tokens", "cost"]}
                  selected={metric}
                  labelFor={metricLabel}
                  onChange={onMetricChange}
                />
              </div>
              {data.attribution.length === 0 ? (
                <UsageMessage>当前筛选条件下没有来源数据。</UsageMessage>
              ) : (
                <UsageSourceChart
                  attribution={data.attribution}
                  metric={metric}
                />
              )}
            </section>

            <section
              className="usage-statistics-section"
              aria-labelledby="usage-token-composition-title"
            >
              <div className="usage-statistics-section-heading">
                <h3 id="usage-token-composition-title">Token 构成</h3>
              </div>
              <UsageTokenCompositionChart tokens={data.tokens} />
            </section>
          </div>
        </div>
      )}
    </section>
  );
}

function StatisticsSegmentedControl<T extends string>({
  label,
  name,
  values,
  selected,
  labelFor,
  onChange,
}: {
  label: string;
  name: string;
  values: readonly T[];
  selected: T;
  labelFor: (value: T) => string;
  onChange: (value: T) => void;
}) {
  return (
    <fieldset className="usage-segmented">
      <legend className="sr-only">{label}</legend>
      {values.map((value) => (
        <label key={value}>
          <input
            type="radio"
            name={name}
            value={value}
            checked={selected === value}
            onChange={() => onChange(value)}
          />
          <span>{labelFor(value)}</span>
        </label>
      ))}
    </fieldset>
  );
}

function metricLabel(value: UsageStatisticsAttributionMetricDto) {
  return value === "requests" ? "请求" : value === "tokens" ? "Token" : "费用";
}

function UsageDetail({
  detail,
}: {
  detail: import("../../generated").UsageRequestDetailDto;
}) {
  const request = detail.request;
  return (
    <AppScrollArea
      className="usage-detail"
      viewportClassName="usage-detail-viewport"
    >
      <dl className="usage-detail-summary">
        <div>
          <dt>开始时间</dt>
          <dd>{formatUsageDateTime(request.startedAtMs)}</dd>
        </div>
        <div>
          <dt>完成时间</dt>
          <dd>{formatUsageDateTime(request.finishedAtMs)}</dd>
        </div>
        <div>
          <dt>路由</dt>
          <dd>{request.routeName ?? "-"}</dd>
        </div>
        <div>
          <dt>实际模型</dt>
          <dd>{request.actualModel ?? request.requestedModel ?? "-"}</dd>
        </div>
        <div>
          <dt>推理强度</dt>
          <dd>{request.reasoningEffort ?? "-"}</dd>
        </div>
        <div>
          <dt>类型</dt>
          <dd>{request.streaming ? "流式" : "同步"}</dd>
        </div>
        <div>
          <dt>请求服务层级</dt>
          <dd>{detail.requestedServiceTier ?? "未指定"}</dd>
        </div>
        <div>
          <dt>实际服务层级</dt>
          <dd>{detail.actualServiceTier ?? "未报告"}</dd>
        </div>
        <div>
          <dt>最终 Tokens</dt>
          <dd>{tokenSummary(detail.tokens)}</dd>
        </div>
        <div>
          <dt>首字延迟</dt>
          <dd>{formatLatency(request.firstOutputLatencyMs)}</dd>
        </div>
        <div>
          <dt>总耗时</dt>
          <dd>{formatLatency(request.totalLatencyMs)}</dd>
        </div>
        <div>
          <dt>上游总费用</dt>
          <dd>{costLabel(request.cost)}</dd>
        </div>
        <div>
          <dt>计价目录</dt>
          <dd>{request.cost.catalogVersion ?? "-"}</dd>
        </div>
      </dl>
      <h3>上游尝试</h3>
      <ol className="usage-attempts">
        {detail.attempts.map((attempt) => (
          <li key={`${attempt.attemptIndex}-${attempt.routeId}`}>
            <header>
              <strong>
                {attempt.attemptRole === "recovery_probe"
                  ? "恢复验证"
                  : `尝试 ${attempt.attemptIndex + 1}`}
                {" · "}
                {attempt.routeName}
              </strong>
              <SettingsStatus
                tone={
                  attempt.deliveryState === "completed" ? "success" : "warning"
                }
              >
                {attempt.deliveryState === "completed" ? "已交付" : "未完成"}
              </SettingsStatus>
            </header>
            <dl>
              <div>
                <dt>模型</dt>
                <dd>{attempt.actualModel ?? "未报告"}</dd>
              </div>
              <div>
                <dt>发送服务层级</dt>
                <dd>{attempt.forwardedServiceTier ?? "未发送"}</dd>
              </div>
              <div>
                <dt>实际服务层级</dt>
                <dd>{attempt.actualServiceTier ?? "未报告"}</dd>
              </div>
              <div>
                <dt>状态</dt>
                <dd>
                  {attemptStatusLabel(
                    attempt.httpStatus,
                    attempt.errorCategory,
                  )}
                </dd>
              </div>
              <div>
                <dt>Tokens</dt>
                <dd>{tokenSummary(attempt.tokens)}</dd>
              </div>
              <div>
                <dt>费用</dt>
                <dd>{costLabel(attempt.cost)}</dd>
              </div>
            </dl>
            {attempt.routingDecision ? (
              <RoutingDecisionBand decision={attempt.routingDecision} />
            ) : null}
          </li>
        ))}
      </ol>
    </AppScrollArea>
  );
}

function RoutingDecisionBand({ decision }: { decision: RoutingDecisionDto }) {
  if (decision.kind === "retry_current") {
    return (
      <div className="usage-routing-decision usage-routing-decision-accent">
        <RotateCw aria-hidden="true" />
        <span>
          重试当前路由（第 {decision.attemptNumber}/{decision.maxAttempts} 次）
        </span>
      </div>
    );
  }
  if (decision.kind === "activate_next") {
    return (
      <div className="usage-routing-decision usage-routing-decision-accent">
        <ArrowRight aria-hidden="true" />
        <div className="usage-routing-decision-copy">
          <span>已自动切换至 {decision.targetRouteName}</span>
          {(decision.skippedRoutes ?? []).map((route) => (
            <small key={route.routeId}>
              已跳过 {route.routeName} · 该模型在此路由不参与 Fallback
            </small>
          ))}
        </div>
      </div>
    );
  }
  if (decision.kind === "resume_captured") {
    return (
      <div className="usage-routing-decision usage-routing-decision-neutral">
        <RotateCcw aria-hidden="true" />
        <span>恢复验证未通过 · 继续使用 {decision.targetRouteName}</span>
      </div>
    );
  }
  if (decision.kind === "recover") {
    return (
      <div className="usage-routing-decision usage-routing-decision-accent">
        <ArrowLeft aria-hidden="true" />
        <span>恢复验证完成 · 已恢复至 {decision.targetRouteName}</span>
      </div>
    );
  }

  const stopPresentation = {
    fallback_disabled: {
      icon: CircleStop,
      tone: "neutral",
      copy: "未切换 · Fallback 已关闭",
    },
    failure_not_eligible: {
      icon: CircleStop,
      tone: "neutral",
      copy: "未切换 · 当前错误不符合切换条件",
    },
    response_committed: {
      icon: LockKeyhole,
      tone: "neutral",
      copy: "未切换 · 响应已经开始交付",
    },
    all_participants_attempted: {
      icon: CircleStop,
      tone: "warning",
      copy: "未切换 · 已到最后一条参与路由",
    },
    stale_policy: {
      icon: RefreshCwOff,
      tone: "warning",
      copy: "未切换 · 路由配置已经变化",
    },
    activation_failed: {
      icon: DatabaseZap,
      tone: "danger",
      copy: `切换至 ${decision.targetRouteName ?? "目标路由"} 失败 · 状态未保存`,
    },
    attempt_index_exhausted: {
      icon: CircleStop,
      tone: "warning",
      copy: "未切换 · 请求尝试次数已达系统上限",
    },
    failure_threshold_not_reached: {
      icon: CircleStop,
      tone: "neutral",
      copy: "未切换 · 当前路由可归因失败尚未达到 5 次",
    },
    failure_threshold_reached_pending: {
      icon: CircleStop,
      tone: "warning",
      copy: "未继续切换 · 已达到失败阈值，等待下一次可执行机会",
    },
    recovery_confirmation_pending: {
      icon: CircleStop,
      tone: "neutral",
      copy: "未切换 · 恢复验证成功 1/2",
    },
    model_fallback_excluded: {
      icon: CircleStop,
      tone: "neutral",
      copy: "已停止 Fallback · 该模型在此路由不参与 Fallback",
    },
  } as const satisfies Record<
    FallbackStopReasonDto,
    {
      icon: typeof CircleStop;
      tone: "neutral" | "warning" | "danger";
      copy: string;
    }
  >;
  const presentation = stopPresentation[decision.reason];
  const Icon = presentation.icon;
  return (
    <div
      className={`usage-routing-decision usage-routing-decision-${presentation.tone}`}
    >
      <Icon aria-hidden="true" />
      <span>{presentation.copy}</span>
    </div>
  );
}

function UsageMessage({
  children,
  tone = "neutral",
}: {
  children: React.ReactNode;
  tone?: "neutral" | "danger";
}) {
  return (
    <div
      className={`usage-message usage-message-${tone}`}
      role={tone === "danger" ? "alert" : "status"}
    >
      {children}
    </div>
  );
}

function costLabel(cost: UsageCostDto) {
  switch (cost.state) {
    case "pre_v0_3a":
      return "未计价（V0.3A 前）";
    case "unavailable":
      return "不可用";
    case "not_applicable":
      return "不适用";
    case "exact":
      return formatUsd(cost.amountPicoUsd);
    case "partial":
      return `至少 ${formatUsd(cost.amountPicoUsd)}`;
  }
}

function tokenSummary(tokens: UsageTokensDto) {
  if (tokens.total === null) return "未知";
  const details = [`总计 ${tokens.total.toLocaleString()}`];
  if (tokens.input !== null)
    details.push(`输入 ${tokens.input.toLocaleString()}`);
  if (tokens.output !== null)
    details.push(`输出 ${tokens.output.toLocaleString()}`);
  if (tokens.cachedInput !== null)
    details.push(`缓存 ${tokens.cachedInput.toLocaleString()}`);
  if (tokens.cacheWriteInput !== null)
    details.push(`写入 ${tokens.cacheWriteInput.toLocaleString()}`);
  return details.join(" · ");
}
