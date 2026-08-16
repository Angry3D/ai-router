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
