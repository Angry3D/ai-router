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
  AppLocale,
  CompletionState,
  FallbackStopReasonDto,
  UsageCostDto,
  UsageHistoryCursorDto,
  UsageHistoryQueryDto,
  UsageRequestDetailDto,
  RoutingDecisionDto,
  UsageStatisticsAttributionDimensionDto,
  UsageStatisticsAttributionMetricDto,
  UsageStatisticsDto,
  UsageStatisticsQueryDto,
  UsageTokensDto,
} from "../../generated";
import { UsageRecordTable } from "../shared/UsageRecordTable";
import { USAGE_SETTINGS_COLUMNS } from "../shared/usageRecordColumns";
import { formatUsageDateTime } from "../shared/usageRecordFormatting";
import { formatModelWithActual } from "../shared/modelDisplay";
import {
  SettingsButton,
  SettingsIconButton,
  SettingsPage,
  SettingsStatus,
  SettingsTextInput,
} from "./SettingsPrimitives";
import { AppScrollArea } from "../shared/AppScrollArea";
import { useLanguage } from "../language/useLanguage";
import type { UsageMessages } from "../language/catalog/usage";
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

/**
 * Attribution dimension of the statistics query; the source chart resolves a
 * missing identity to the matching unknown-label text.
 */
const ATTRIBUTION_DIMENSION: UsageStatisticsAttributionDimensionDto = "model";

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

function completionLabels(
  messages: UsageMessages,
): Record<CompletionState, string> {
  return {
    completed: messages.completionCompleted,
    failed: messages.completionFailed,
    cancelled: messages.completionCancelled,
    no_upstream: messages.completionNoUpstream,
  };
}

function attemptStatusLabel(
  httpStatus: number | null,
  errorCategory: string | null,
  messages: UsageMessages,
) {
  const parts: string[] = [];
  if (httpStatus !== null) parts.push(`HTTP ${httpStatus}`);
  if (errorCategory !== null) {
    parts.push(
      errorCategory === "upstream_access_denied"
        ? messages.accessDenied
        : errorCategory,
    );
  }
  return parts.length > 0 ? parts.join(" · ") : "-";
}

export function UsageSettings() {
  const {
    messages: { usage: usageMessages },
    effectiveLocale,
  } = useLanguage();
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
      attributionDimension: ATTRIBUTION_DIMENSION,
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
    <SettingsPage
      title={usageMessages.pageTitle}
      titleId="usage-title"
      className="usage-page"
    >
      <form
        className="usage-filters"
        aria-label={usageMessages.filtersAriaLabel}
        onSubmit={(event) => {
          event.preventDefault();
          applyFilters(draftFilters);
        }}
      >
        <div className="usage-filter-grid">
          <label>
            <span>{usageMessages.rangeLabel}</span>
            <select
              aria-label={usageMessages.rangeAriaLabel}
              value={draftFilters.range}
              onChange={(event) => {
                const range = event.currentTarget.value as TimeRange;
                setDraftFilters((filters) => ({
                  ...filters,
                  range,
                }));
              }}
            >
              <option value="today">{usageMessages.rangeToday}</option>
              <option value="yesterday">{usageMessages.rangeYesterday}</option>
              <option value="24h">{usageMessages.range24h}</option>
              <option value="7d">{usageMessages.range7d}</option>
              <option value="30d">{usageMessages.range30d}</option>
              <option value="all">{usageMessages.rangeAll}</option>
            </select>
          </label>
          <label>
            <span>{usageMessages.statusLabel}</span>
            <select
              aria-label={usageMessages.statusAriaLabel}
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
              <option value="all">{usageMessages.statusAll}</option>
              {Object.entries(completionLabels(usageMessages)).map(
                ([value, label]) => (
                  <option value={value} key={value}>
                    {label}
                  </option>
                ),
              )}
            </select>
          </label>
          <label>
            <span>{usageMessages.routeLabel}</span>
            <select
              aria-label={usageMessages.routeLabel}
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
                <option value="">{usageMessages.routesLoading}</option>
              ) : null}
              {routeOptions.isError ? (
                <option value="">{usageMessages.routesLoadFailedOption}</option>
              ) : null}
              {!routeOptions.isPending && !routeOptions.isError ? (
                <option value="">{usageMessages.routesAll}</option>
              ) : null}
              {(routeOptions.data ?? []).map((route) => (
                <option value={route.routeId} key={route.routeId}>
                  {route.name ?? usageMessages.deletedRouteLabel}
                  {route.retained && route.name !== null
                    ? usageMessages.routeRetainedSuffix
                    : ""}
                </option>
              ))}
            </select>
          </label>
          <label>
            <span>{usageMessages.modelLabel}</span>
            <SettingsTextInput
              aria-label={usageMessages.modelContainsAriaLabel}
              maxLength={256}
              value={draftFilters.model}
              placeholder={usageMessages.modelPlaceholder}
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
            {usageMessages.routeOptionsLoadFailed}
            <button type="button" onClick={() => void routeOptions.refetch()}>
              {usageMessages.retry}
            </button>
          </span>
        ) : null}
        <div className="usage-filter-toolbar">
          <div
            className="usage-tabs"
            role="tablist"
            aria-label={usageMessages.tabsAriaLabel}
          >
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
              {usageMessages.recordsTab}
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
              {usageMessages.statisticsTab}
            </button>
          </div>
          <div className="usage-filter-actions">
            <SettingsButton
              type="button"
              onClick={() => applyFilters(DEFAULT_FILTERS)}
            >
              <RotateCcw aria-hidden="true" size={14} />
              {usageMessages.reset}
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
              {usageMessages.refresh}
            </SettingsButton>
            <span className="sr-only" role="status" aria-live="polite">
              {activeViewIsRefreshing ? usageMessages.refreshing : ""}
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
                  label={usageMessages.backToList}
                  onClick={() => setSelectedRequestId(null)}
                >
                  <ArrowLeft aria-hidden="true" size={17} />
                </SettingsIconButton>
                <strong>{usageMessages.requestDetailTitle}</strong>
              </div>
              {detail.isPending ? (
                <UsageMessage>{usageMessages.detailLoading}</UsageMessage>
              ) : null}
              {detail.isError ? (
                <UsageMessage tone="danger">
                  {usageMessages.detailLoadFailed}
                </UsageMessage>
              ) : null}
              {detail.data ? <UsageDetail detail={detail.data} /> : null}
            </>
          ) : null}
          {selectedRequestId === null ? (
            <>
              {history.isPending ? (
                <UsageMessage>{usageMessages.recordsLoading}</UsageMessage>
              ) : null}
              {history.isError ? (
                <UsageMessage tone="danger">
                  <span>{usageMessages.recordsLoadFailed}</span>
                  <SettingsButton
                    type="button"
                    onClick={() => void history.refetch()}
                  >
                    <RefreshCw aria-hidden="true" size={15} />
                    {usageMessages.retry}
                  </SettingsButton>
                </UsageMessage>
              ) : null}
              {history.data?.rows.length === 0 ? (
                <UsageMessage>{usageMessages.noRecords}</UsageMessage>
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

              <nav
                className="usage-pagination"
                aria-label={usageMessages.paginationAriaLabel}
              >
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
                  {usageMessages.previousPage}
                </SettingsButton>
                <span className="usage-page-position" aria-live="polite">
                  {usageMessages.pagePosition(
                    currentPage,
                    totalPages,
                    totalRows.toLocaleString(effectiveLocale),
                  )}
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
                  {usageMessages.nextPage}
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
              dimension={ATTRIBUTION_DIMENSION}
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
  dimension,
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
  dimension: UsageStatisticsAttributionDimensionDto;
}) {
  const {
    messages: { usage: usageMessages },
  } = useLanguage();
  if (disabled) {
    return (
      <UsageMessage>{usageMessages.statisticsCompletedOnly}</UsageMessage>
    );
  }
  if (isPending) {
    return <UsageMessage>{usageMessages.statisticsLoading}</UsageMessage>;
  }
  if (isError) {
    return (
      <UsageMessage tone="danger">
        <span>{usageMessages.statisticsLoadFailed}</span>
        <SettingsButton type="button" onClick={onRetry}>
          <RefreshCw aria-hidden="true" size={15} />
          {usageMessages.retry}
        </SettingsButton>
      </UsageMessage>
    );
  }
  if (!data) return null;
  return (
    <section
      className="usage-statistics"
      aria-label={usageMessages.statisticsTab}
    >
      <div className="usage-statistics-summary">
        <div>
          <span>{usageMessages.successfulRequests}</span>
          <strong>
            {formatDecimalInteger(String(data.matchedRequestCount))}
          </strong>
          <small>{usageMessages.matchedFilterHint}</small>
        </div>
        <div>
          <span>{usageMessages.tokenLabel}</span>
          <strong>{formatStatisticsTokens(data.tokens.total)}</strong>
          <dl className="usage-statistics-token-breakdown">
            <div className="is-input">
              <dt>{usageMessages.uncachedInput}</dt>
              <dd>{formatStatisticsTokens(data.tokens.uncachedInput)}</dd>
            </div>
            <div className="is-cache">
              <dt>{usageMessages.cachedInput}</dt>
              <dd>{formatStatisticsTokens(data.tokens.cachedInput)}</dd>
            </div>
            <div className="is-write">
              <dt>{usageMessages.cacheWriteInput}</dt>
              <dd>{formatStatisticsTokens(data.tokens.cacheWriteInput)}</dd>
            </div>
            <div className="is-output">
              <dt>{usageMessages.output}</dt>
              <dd>{formatStatisticsTokens(data.tokens.output)}</dd>
            </div>
          </dl>
        </div>
        <div>
          <span>{usageMessages.cost}</span>
          <strong>{formatStatisticsUsd(data.costPicoUsd)}</strong>
          <small>{usageMessages.recordedUpstreamCost}</small>
        </div>
      </div>
      {data.trend.length === 0 ? (
        <UsageMessage>{usageMessages.noCompletedRequests}</UsageMessage>
      ) : (
        <div className="usage-statistics-analysis">
          <section
            className="usage-statistics-section usage-statistics-time-section"
            aria-labelledby="usage-trend-title"
          >
            <div className="usage-statistics-section-heading">
              <h3 id="usage-trend-title">{usageMessages.byTime}</h3>
              <StatisticsSegmentedControl
                label={usageMessages.trendMetricLabel}
                name="usage-trend-metric"
                values={["requests", "tokens", "cost"]}
                selected={trendMetric}
                labelFor={metricLabel}
                onChange={onTrendMetricChange}
              />
            </div>
            <UsageTimeChart
              trend={data.trend}
              metric={trendMetric}
              granularity={data.granularity}
              timeZone={data.timeZone}
            />
          </section>

          <div className="usage-statistics-lower-grid">
            <section
              className="usage-statistics-section"
              aria-labelledby="usage-attribution-title"
            >
              <div className="usage-statistics-section-heading">
                <h3 id="usage-attribution-title">{usageMessages.bySource}</h3>
                <StatisticsSegmentedControl
                  label={usageMessages.sourceMetricLabel}
                  name="usage-attribution-metric"
                  values={["requests", "tokens", "cost"]}
                  selected={metric}
                  labelFor={metricLabel}
                  onChange={onMetricChange}
                />
              </div>
              {data.attribution.length === 0 ? (
                <UsageMessage>{usageMessages.noSourceData}</UsageMessage>
              ) : (
                <UsageSourceChart
                  attribution={data.attribution}
                  metric={metric}
                  dimension={dimension}
                />
              )}
            </section>

            <section
              className="usage-statistics-section"
              aria-labelledby="usage-token-composition-title"
            >
              <div className="usage-statistics-section-heading">
                <h3 id="usage-token-composition-title">
                  {usageMessages.tokenComposition}
                </h3>
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
  labelFor: (value: T, messages: UsageMessages) => string;
  onChange: (value: T) => void;
}) {
  const {
    messages: { usage: usageMessages },
  } = useLanguage();
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
          <span>{labelFor(value, usageMessages)}</span>
        </label>
      ))}
    </fieldset>
  );
}

function metricLabel(
  value: UsageStatisticsAttributionMetricDto,
  messages: UsageMessages,
) {
  return value === "requests"
    ? messages.metricRequests
    : value === "tokens"
      ? messages.metricTokens
      : messages.metricCost;
}

function UsageDetail({ detail }: { detail: UsageRequestDetailDto }) {
  const {
    messages: { usage: usageMessages },
    effectiveLocale,
  } = useLanguage();
  const request = detail.request;
  return (
    <AppScrollArea
      className="usage-detail"
      viewportClassName="usage-detail-viewport"
    >
      <dl className="usage-detail-summary">
        <div>
          <dt>{usageMessages.startedAt}</dt>
          <dd>{formatUsageDateTime(request.startedAtMs, effectiveLocale)}</dd>
        </div>
        <div>
          <dt>{usageMessages.finishedAt}</dt>
          <dd>{formatUsageDateTime(request.finishedAtMs, effectiveLocale)}</dd>
        </div>
        <div>
          <dt>{usageMessages.routeLabel}</dt>
          <dd>{request.routeName ?? "-"}</dd>
        </div>
        <div>
          <dt>{usageMessages.modelLabel}</dt>
          <dd>
            {formatModelWithActual(
              request.requestedModel,
              request.actualModel,
              usageMessages.modelPair,
            )}
          </dd>
        </div>
        <div>
          <dt>{usageMessages.reasoningEffort}</dt>
          <dd>{request.reasoningEffort ?? "-"}</dd>
        </div>
        <div>
          <dt>{usageMessages.requestKind}</dt>
          <dd>
            {request.streaming
              ? usageMessages.streaming
              : usageMessages.synchronous}
          </dd>
        </div>
        <div>
          <dt>{usageMessages.requestedServiceTier}</dt>
          <dd>{detail.requestedServiceTier ?? usageMessages.notSpecified}</dd>
        </div>
        <div>
          <dt>{usageMessages.actualServiceTier}</dt>
          <dd>{detail.actualServiceTier ?? usageMessages.notReported}</dd>
        </div>
        <div>
          <dt>{usageMessages.finalTokens}</dt>
          <dd>{tokenSummary(detail.tokens, usageMessages, effectiveLocale)}</dd>
        </div>
        <div>
          <dt>{usageMessages.firstOutputLatency}</dt>
          <dd>{formatLatency(request.firstOutputLatencyMs)}</dd>
        </div>
        <div>
          <dt>{usageMessages.totalLatency}</dt>
          <dd>{formatLatency(request.totalLatencyMs)}</dd>
        </div>
        <div>
          <dt>{usageMessages.upstreamTotalCost}</dt>
          <dd>{costLabel(request.cost, usageMessages)}</dd>
        </div>
        <div>
          <dt>{usageMessages.pricingCatalog}</dt>
          <dd>{request.cost.catalogVersion ?? "-"}</dd>
        </div>
      </dl>
      <h3>{usageMessages.upstreamAttempts}</h3>
      <ol className="usage-attempts">
        {detail.attempts.map((attempt) => (
          <li key={`${attempt.attemptIndex}-${attempt.routeId}`}>
            <header>
              <strong>
                {attempt.attemptRole === "recovery_probe"
                  ? usageMessages.recoveryProbe
                  : usageMessages.attemptNumbered(attempt.attemptIndex + 1)}
                {" · "}
                {attempt.routeName}
              </strong>
              <SettingsStatus
                tone={
                  attempt.deliveryState === "completed" ? "success" : "warning"
                }
              >
                {attempt.deliveryState === "completed"
                  ? usageMessages.delivered
                  : usageMessages.incomplete}
              </SettingsStatus>
            </header>
            <dl>
              <div>
                <dt>{usageMessages.modelLabel}</dt>
                <dd>{attempt.actualModel ?? usageMessages.notReported}</dd>
              </div>
              <div>
                <dt>{usageMessages.forwardedServiceTier}</dt>
                <dd>{attempt.forwardedServiceTier ?? usageMessages.notSent}</dd>
              </div>
              <div>
                <dt>{usageMessages.actualServiceTier}</dt>
                <dd>{attempt.actualServiceTier ?? usageMessages.notReported}</dd>
              </div>
              <div>
                <dt>{usageMessages.statusLabel}</dt>
                <dd>
                  {attemptStatusLabel(
                    attempt.httpStatus,
                    attempt.errorCategory,
                    usageMessages,
                  )}
                </dd>
              </div>
              <div>
                <dt>{usageMessages.tokensLabel}</dt>
                <dd>
                  {tokenSummary(
                    attempt.tokens,
                    usageMessages,
                    effectiveLocale,
                  )}
                </dd>
              </div>
              <div>
                <dt>{usageMessages.cost}</dt>
                <dd>{costLabel(attempt.cost, usageMessages)}</dd>
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
  const {
    messages: { usage: usageMessages },
  } = useLanguage();
  if (decision.kind === "retry_current") {
    return (
      <div className="usage-routing-decision usage-routing-decision-accent">
        <RotateCw aria-hidden="true" />
        <span>
          {usageMessages.retryCurrentRoute(
            decision.attemptNumber,
            decision.maxAttempts,
          )}
        </span>
      </div>
    );
  }
  if (decision.kind === "activate_next") {
    return (
      <div className="usage-routing-decision usage-routing-decision-accent">
        <ArrowRight aria-hidden="true" />
        <div className="usage-routing-decision-copy">
          <span>
            {usageMessages.switchedToRoute(decision.targetRouteName)}
          </span>
          {(decision.skippedRoutes ?? []).map((route) => (
            <small key={route.routeId}>
              {usageMessages.skippedRouteFallbackExcluded(route.routeName)}
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
        <span>
          {usageMessages.recoveryProbeFailedContinue(
            decision.targetRouteName,
          )}
        </span>
      </div>
    );
  }
  if (decision.kind === "recover") {
    return (
      <div className="usage-routing-decision usage-routing-decision-accent">
        <ArrowLeft aria-hidden="true" />
        <span>
          {usageMessages.recoveryProbeCompleted(decision.targetRouteName)}
        </span>
      </div>
    );
  }

  const stopPresentation = {
    fallback_disabled: {
      icon: CircleStop,
      tone: "neutral",
      copy: usageMessages.stopFallbackDisabled,
    },
    failure_not_eligible: {
      icon: CircleStop,
      tone: "neutral",
      copy: usageMessages.stopFailureNotEligible,
    },
    response_committed: {
      icon: LockKeyhole,
      tone: "neutral",
      copy: usageMessages.stopResponseCommitted,
    },
    all_participants_attempted: {
      icon: CircleStop,
      tone: "warning",
      copy: usageMessages.stopAllParticipantsAttempted,
    },
    stale_policy: {
      icon: RefreshCwOff,
      tone: "warning",
      copy: usageMessages.stopStalePolicy,
    },
    activation_failed: {
      icon: DatabaseZap,
      tone: "danger",
      copy: usageMessages.stopActivationFailed(
        decision.targetRouteName ?? usageMessages.unknownTargetRoute,
      ),
    },
    attempt_index_exhausted: {
      icon: CircleStop,
      tone: "warning",
      copy: usageMessages.stopAttemptIndexExhausted,
    },
    failure_threshold_not_reached: {
      icon: CircleStop,
      tone: "neutral",
      copy: usageMessages.stopFailureThresholdNotReached,
    },
    failure_threshold_reached_pending: {
      icon: CircleStop,
      tone: "warning",
      copy: usageMessages.stopFailureThresholdReachedPending,
    },
    recovery_confirmation_pending: {
      icon: CircleStop,
      tone: "neutral",
      copy: usageMessages.stopRecoveryConfirmationPending,
    },
    model_fallback_excluded: {
      icon: CircleStop,
      tone: "neutral",
      copy: usageMessages.stopModelFallbackExcluded,
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

function costLabel(cost: UsageCostDto, messages: UsageMessages) {
  switch (cost.state) {
    case "pre_v0_3a":
      return messages.costNotPricedPreV03A;
    case "unavailable":
      return messages.costUnavailable;
    case "not_applicable":
      return messages.costNotApplicable;
    case "exact":
      return formatUsd(cost.amountPicoUsd, messages.costUnavailable);
    case "partial":
      return messages.costAtLeast(
        formatUsd(cost.amountPicoUsd, messages.costUnavailable),
      );
  }
}

function tokenSummary(
  tokens: UsageTokensDto,
  messages: UsageMessages,
  locale: AppLocale,
) {
  if (tokens.total === null) return messages.tokenUnknown;
  const details = [messages.tokenTotal(tokens.total.toLocaleString(locale))];
  if (tokens.input !== null)
    details.push(messages.tokenInput(tokens.input.toLocaleString(locale)));
  if (tokens.output !== null)
    details.push(messages.tokenOutput(tokens.output.toLocaleString(locale)));
  if (tokens.cachedInput !== null)
    details.push(
      messages.tokenCached(tokens.cachedInput.toLocaleString(locale)),
    );
  if (tokens.cacheWriteInput !== null)
    details.push(
      messages.tokenCacheWrite(tokens.cacheWriteInput.toLocaleString(locale)),
    );
  return details.join(" · ");
}
