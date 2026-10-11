import { ArrowDown, ArrowUp, Database } from "lucide-react";
import type { ReactNode } from "react";

import type {
  CompletionState,
  UsageCostDto,
  UsageFastStatusDto,
  UsageHistoryRowDto,
  UsageTokensDto,
} from "../../generated";
import type { UsageRecordColumn } from "./usageRecordColumns";
import { isRedirectedModel } from "./modelDisplay";
import { useLanguage } from "../language/useLanguage";
import type { UsageMessages } from "../language/catalog/usage";
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

function columnLabels(
  messages: UsageMessages,
): Record<UsageRecordColumn, string> {
  return {
    route: messages.routeLabel,
    model: messages.modelLabel,
    state: messages.columnState,
    tokens: messages.columnTokens,
    totalTokens: messages.columnTotalTokens,
    cost: messages.cost,
    latency: messages.columnLatency,
    firstOutputLatency: messages.columnFirstOutputLatency,
    completedAt: messages.completedAtColumn,
    request: messages.columnRequest,
  };
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

function fastLabels(
  messages: UsageMessages,
): Record<UsageFastStatusDto, string> {
  return {
    confirmed: messages.fastConfirmed,
    unconfirmed: messages.fastUnconfirmed,
  };
}

function stateLabel(
  state: CompletionState,
  httpStatus: number | null,
  messages: UsageMessages,
) {
  const label = completionLabels(messages)[state];
  if (state === "completed" || httpStatus === null) return label;
  return messages.completionWithStatus(label, httpStatus);
}

function compactCostLabel(cost: UsageCostDto, messages: UsageMessages) {
  switch (cost.state) {
    case "exact":
      return formatCompactUsd(cost.amountPicoUsd, messages.costUnavailable);
    case "partial":
      return messages.costAtLeast(
        formatCompactUsd(cost.amountPicoUsd, messages.costUnavailable, "down"),
      );
    case "pre_v0_3a":
      return messages.costNotPricedPreV03A;
    case "unavailable":
      return messages.costUnavailable;
    case "not_applicable":
      return messages.costNotApplicable;
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
  const {
    messages: { usage: usageMessages },
  } = useLanguage();
  return (
    <div className="usage-request-state">
      {includeStreaming ? (
        <span>
          {streaming ? usageMessages.streaming : usageMessages.synchronous}
        </span>
      ) : null}
      <span className={`usage-completion usage-completion-${state}`}>
        {stateLabel(state, httpStatus, usageMessages)}
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
  const { effectiveLocale } = useLanguage();
  if (mode === "total") {
    return (
      <div className="usage-metric-cell usage-token-cell usage-token-total-cell">
        <span>{compactToken(tokens.total, effectiveLocale)}</span>
      </div>
    );
  }

  return (
    <div className="usage-metric-cell usage-token-cell">
      <div className="usage-token-lines">
        <div className="usage-token-primary">
          <span>
            <ArrowDown aria-hidden="true" size={13} />
            {compactToken(tokens.uncachedInput, effectiveLocale)}
          </span>
          <span>
            <ArrowUp aria-hidden="true" size={13} />
            {compactToken(tokens.output, effectiveLocale)}
          </span>
        </div>
        <div className="usage-token-cache">
          <Database aria-hidden="true" size={12} />
          {compactToken(tokens.cachedInput, effectiveLocale)}
        </div>
      </div>
    </div>
  );
}

export function UsageCostCell({ cost }: { cost: UsageCostDto }) {
  const {
    messages: { usage: usageMessages },
  } = useLanguage();
  const fastLabel =
    cost.fastStatus === null
      ? null
      : fastLabels(usageMessages)[cost.fastStatus];
  return (
    <div
      className={`usage-metric-cell usage-cost-cell usage-cost-${cost.state}`}
    >
      <span>{compactCostLabel(cost, usageMessages)}</span>
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
  const {
    messages: { usage: usageMessages },
  } = useLanguage();
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
      <span className="usage-latency-label">{usageMessages.latencyFirst}</span>
      <span className={firstValueClass}>{formatCompactLatency(firstMs)}</span>
      <span className="usage-latency-label">{usageMessages.totalLatency}</span>
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
  const {
    messages: { usage: usageMessages },
  } = useLanguage();
  const requested = row.requestedModel ?? "-";
  const redirected = isRedirectedModel(row.requestedModel, row.actualModel);
  const reasoningEffort = row.reasoningEffort ?? "-";
  return (
    <div
      className="usage-model-cell"
      title={
        redirected
          ? usageMessages.modelRedirectTitle(
              requested,
              row.actualModel ?? "",
              reasoningEffort,
            )
          : usageMessages.modelTitle(requested, reasoningEffort)
      }
    >
      <span>{requested}</span>
      {redirected ? (
        <span className="usage-model-redirect">→{row.actualModel}</span>
      ) : null}
      <span>{usageMessages.reasoningInline(reasoningEffort)}</span>
    </div>
  );
}

function UsagePreviewRequestCell({ row }: { row: UsageHistoryRowDto }) {
  const {
    messages: { usage: usageMessages },
  } = useLanguage();
  const requested = row.requestedModel ?? "-";
  const redirected = isRedirectedModel(row.requestedModel, row.actualModel);
  const reasoningEffort = row.reasoningEffort ?? "-";
  const timestamp =
    row.finishedAtMs === null ? "-" : formatUsagePreviewTime(row.finishedAtMs);
  const completion = stateLabel(
    row.completionState,
    row.httpStatus,
    usageMessages,
  );
  return (
    <div
      className="usage-preview-request-cell"
      title={
        redirected
          ? `${requested} · →${row.actualModel} · ${reasoningEffort}`
          : `${requested} · ${reasoningEffort}`
      }
    >
      <span className="usage-preview-request-model">{requested}</span>
      {redirected ? (
        <span className="usage-preview-request-redirect">→{row.actualModel}</span>
      ) : null}
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
  const {
    messages: { usage: usageMessages },
    effectiveLocale,
  } = useLanguage();
  const labels = columnLabels(usageMessages);
  const preview = columns.includes("request");
  return (
    <table
      className={`usage-table${preview ? " usage-table-preview" : ""}${className ? ` ${className}` : ""}`}
    >
      <thead>
        <tr>
          {columns.map((column) => (
            <th key={column}>{labels[column]}</th>
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
                          title={formatUsageDateTime(row.finishedAtMs, effectiveLocale)}
                          aria-label={usageMessages.viewRequestAriaLabel(
                            formatUsageDateTime(row.finishedAtMs, effectiveLocale),
                          )}
                          onClick={() => onSelectRequest(row.requestId)}
                        >
                          {row.finishedAtMs === null ? (
                            <span>-</span>
                          ) : (
                            <>
                              <span>
                                {formatUsageCompactDate(
                                  row.finishedAtMs,
                                  effectiveLocale,
                                )}
                              </span>
                              <span>
                                {formatUsageCompactTime(
                                  row.finishedAtMs,
                                  effectiveLocale,
                                )}
                              </span>
                            </>
                          )}
                        </button>
                      ) : (
                        formatUsageDateTime(row.finishedAtMs, effectiveLocale)
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
