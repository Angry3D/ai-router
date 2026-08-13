import { ArrowDown, ArrowUp, Database } from "lucide-react";
import type { ReactNode } from "react";

import type {
  CompletionState,
  UsageCostDto,
  UsageFastStatusDto,
  UsageHistoryRowDto,
  UsageTokensDto,
} from "../../generated";
import {
  compactToken,
  formatCompactLatency,
  formatCompactUsd,
  latencyTone,
} from "./usageFormatting";
import {
  formatUsageCompactDate,
  formatUsageCompactTime,
  formatUsageDateTime,
  formatUsagePreviewTime,
} from "./usageRecordFormatting";

export const USAGE_SETTINGS_COLUMNS = [
  "route",
  "model",
  "state",
  "tokens",
  "cost",
  "latency",
  "completedAt",
] as const;

export const USAGE_PREVIEW_COLUMNS = [
  "request",
  "totalTokens",
  "cost",
  "firstOutputLatency",
] as const;

export type UsageRecordColumn =
  | (typeof USAGE_SETTINGS_COLUMNS)[number]
  | (typeof USAGE_PREVIEW_COLUMNS)[number];

const COLUMN_LABELS: Record<UsageRecordColumn, string> = {
  route: "路由",
  model: "模型",
  state: "类型/状态",
  tokens: "TOKEN",
  totalTokens: "Token",
  cost: "费用",
  latency: "延迟",
  firstOutputLatency: "首 Token",
  completedAt: "完成时间",
  request: "请求",
};

const COMPLETION_LABELS: Record<CompletionState, string> = {
  completed: "已完成",
  failed: "失败",
  cancelled: "已取消",
  no_upstream: "无上游",
};

const FAST_LABELS: Record<UsageFastStatusDto, string> = {
  confirmed: "Fast",
  unconfirmed: "Fast 未确认",
};

function stateLabel(state: CompletionState, httpStatus: number | null) {
  if (state === "completed" || httpStatus === null)
    return COMPLETION_LABELS[state];
  return `${COMPLETION_LABELS[state]}（${httpStatus}）`;
}

function compactCostLabel(cost: UsageCostDto) {
  switch (cost.state) {
    case "exact":
      return formatCompactUsd(cost.amountPicoUsd);
    case "partial":
      return `至少 ${formatCompactUsd(cost.amountPicoUsd, "down")}`;
    case "pre_v0_3a":
      return "未计价（V0.3A 前）";
    case "unavailable":
      return "不可用";
    case "not_applicable":
      return "不适用";
  }
}

export function UsageRequestState({
  state,
  httpStatus,
  streaming,
  includeStreaming = true,
}: {
  state: CompletionState;
  httpStatus: number | null;
  streaming: boolean;
  includeStreaming?: boolean;
}) {
  return (
    <div className="usage-request-state">
      {includeStreaming ? <span>{streaming ? "流式" : "同步"}</span> : null}
      <span className={`usage-completion usage-completion-${state}`}>
        {stateLabel(state, httpStatus)}
      </span>
    </div>
  );
}

export function UsageTokenCell({
  tokens,
  mode = "breakdown",
}: {
  tokens: UsageTokensDto;
  mode?: "breakdown" | "total";
}) {
  if (mode === "total") {
    return (
      <div className="usage-metric-cell usage-token-cell usage-token-total-cell">
        <span>{compactToken(tokens.total)}</span>
      </div>
    );
  }

  return (
    <div className="usage-metric-cell usage-token-cell">
      <div className="usage-token-lines">
        <div className="usage-token-primary">
          <span>
            <ArrowDown aria-hidden="true" size={13} />
            {compactToken(tokens.uncachedInput)}
          </span>
          <span>
            <ArrowUp aria-hidden="true" size={13} />
            {compactToken(tokens.output)}
          </span>
        </div>
        <div className="usage-token-cache">
          <Database aria-hidden="true" size={12} />
          {compactToken(tokens.cachedInput)}
        </div>
      </div>
    </div>
  );
}

export function UsageCostCell({ cost }: { cost: UsageCostDto }) {
  const fastLabel =
    cost.fastStatus === null ? null : FAST_LABELS[cost.fastStatus];
  return (
    <div
      className={`usage-metric-cell usage-cost-cell usage-cost-${cost.state}`}
    >
      <span>{compactCostLabel(cost)}</span>
      {fastLabel === null ? null : (
        <span className={`usage-cost-tier usage-cost-tier-${cost.fastStatus}`}>
          {fastLabel}
        </span>
      )}
    </div>
  );
}

export function UsageLatencyCell({
  firstMs,
  totalMs,
  mode = "complete",
}: {
  firstMs: number | null;
  totalMs: number | null;
  mode?: "complete" | "firstOutput";
}) {
  const firstTone = latencyTone(firstMs);
  const slowFirst = firstTone === "warning";
  const firstValueClass = slowFirst
    ? "usage-latency-value is-warning"
    : firstMs === null
      ? "usage-latency-value is-unknown"
      : "usage-latency-value";

  if (mode === "firstOutput") {
    return (
      <div className="usage-latency-cell usage-first-output-cell">
        <span className={firstValueClass}>{formatCompactLatency(firstMs)}</span>
      </div>
    );
  }

  return (
    <div className="usage-latency-cell">
      <span className="usage-latency-bar" aria-hidden="true">
        <span
          className={
            slowFirst
              ? "is-warning"
              : firstTone === "unknown"
                ? "is-unknown"
                : ""
          }
        />
        <span className={totalMs === null ? "is-unknown" : ""} />
      </span>
      <span className="usage-latency-label">首字</span>
      <span className={firstValueClass}>{formatCompactLatency(firstMs)}</span>
      <span className="usage-latency-label">总耗时</span>
      <span
        className={
          totalMs === null
            ? "usage-latency-value is-unknown"
            : "usage-latency-value"
        }
      >
        {formatCompactLatency(totalMs)}
      </span>
    </div>
  );
}

function UsageModelCell({ row }: { row: UsageHistoryRowDto }) {
  const model = row.actualModel ?? row.requestedModel ?? "-";
  return (
    <div
      className="usage-model-cell"
      title={`${model}\n推理 ${row.reasoningEffort ?? "-"}`}
    >
      <span>{model}</span>
      <span>推理 {row.reasoningEffort ?? "-"}</span>
    </div>
  );
}

function UsagePreviewRequestCell({ row }: { row: UsageHistoryRowDto }) {
  const model = row.actualModel ?? row.requestedModel ?? "-";
  const reasoningEffort = row.reasoningEffort ?? "-";
  const timestamp =
    row.finishedAtMs === null ? "-" : formatUsagePreviewTime(row.finishedAtMs);
  const completion = stateLabel(row.completionState, row.httpStatus);
  return (
    <div
      className="usage-preview-request-cell"
      title={`${model} · ${reasoningEffort}`}
    >
      <span className="usage-preview-request-model">{model}</span>
      <span className="usage-preview-request-time">
        {reasoningEffort} · {timestamp}
      </span>
      <span className="sr-only">{completion}</span>
    </div>
  );
}

export function UsageRecordTable({
  rows,
  columns,
  onSelectRequest,
  bodyFallback,
  className = "",
}: {
  rows: UsageHistoryRowDto[];
  columns: readonly UsageRecordColumn[];
  onSelectRequest?: (requestId: string) => void;
  bodyFallback?: ReactNode;
  className?: string;
}) {
  const preview = columns.includes("request");
  return (
    <table
      className={`usage-table${preview ? " usage-table-preview" : ""}${className ? ` ${className}` : ""}`}
    >
      <thead>
        <tr>
          {columns.map((column) => (
            <th key={column}>{COLUMN_LABELS[column]}</th>
          ))}
        </tr>
      </thead>
      <tbody>
        {rows.length === 0 && bodyFallback ? (
          <tr className="usage-table-fallback-row">
            <td colSpan={columns.length}>{bodyFallback}</td>
          </tr>
        ) : null}
        {rows.map((row) => (
          <tr
            key={row.requestId}
            className={
              preview
                ? `usage-preview-row usage-preview-row-${row.completionState}`
                : undefined
            }
          >
            {columns.map((column) => {
              switch (column) {
                case "route":
                  return (
                    <td key={column} title={row.routeName ?? ""}>
                      {row.routeName ?? "-"}
                    </td>
                  );
                case "model":
                  return (
                    <td key={column}>
                      <UsageModelCell row={row} />
                    </td>
                  );
                case "state":
                  return (
                    <td key={column}>
                      <UsageRequestState
                        streaming={row.streaming}
                        state={row.completionState}
                        httpStatus={row.httpStatus}
                      />
                    </td>
                  );
                case "tokens":
                  return (
                    <td key={column}>
                      <UsageTokenCell tokens={row.tokens} />
                    </td>
                  );
                case "totalTokens":
                  return (
                    <td key={column}>
                      <UsageTokenCell tokens={row.tokens} mode="total" />
                    </td>
                  );
                case "cost":
                  return (
                    <td key={column}>
                      <UsageCostCell cost={row.cost} />
                    </td>
                  );
                case "latency":
                  return (
                    <td key={column}>
                      <UsageLatencyCell
                        firstMs={row.firstOutputLatencyMs}
                        totalMs={row.totalLatencyMs}
                      />
                    </td>
                  );
                case "firstOutputLatency":
                  return (
                    <td key={column}>
                      <UsageLatencyCell
                        firstMs={row.firstOutputLatencyMs}
                        totalMs={row.totalLatencyMs}
                        mode="firstOutput"
                      />
                    </td>
                  );
                case "completedAt":
                  return (
                    <td key={column} className="usage-time-cell">
                      {onSelectRequest ? (
                        <button
                          type="button"
                          className="usage-row-link"
                          title={formatUsageDateTime(row.finishedAtMs)}
                          aria-label={`查看请求 ${formatUsageDateTime(row.finishedAtMs)}`}
                          onClick={() => onSelectRequest(row.requestId)}
                        >
                          {row.finishedAtMs === null ? (
                            <span>-</span>
                          ) : (
                            <>
                              <span>
                                {formatUsageCompactDate(row.finishedAtMs)}
                              </span>
                              <span>
                                {formatUsageCompactTime(row.finishedAtMs)}
                              </span>
                            </>
                          )}
                        </button>
                      ) : (
                        formatUsageDateTime(row.finishedAtMs)
                      )}
                    </td>
                  );
                case "request":
                  return (
                    <td key={column} className="usage-preview-request">
                      <UsagePreviewRequestCell row={row} />
                    </td>
                  );
              }
            })}
          </tr>
        ))}
      </tbody>
    </table>
  );
}
