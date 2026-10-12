import type {
  AppLocale,
  UsageStatisticsGranularityDto,
} from "../../generated";

const bucketLabelFormatters = new Map<string, Intl.DateTimeFormat>();

function bucketLabelFormatter(
  granularity: UsageStatisticsGranularityDto,
  timeZone: string,
  includeUtcOffset: boolean,
  locale: AppLocale,
): Intl.DateTimeFormat {
  const key = `${locale}|${timeZone}|${granularity}|${includeUtcOffset}`;
  const cached = bucketLabelFormatters.get(key);
  if (cached) return cached;
  const options: Intl.DateTimeFormatOptions =
    granularity === "hour"
      ? {
          timeZone,
          month: "2-digit",
          day: "2-digit",
          hour: "2-digit",
          minute: "2-digit",
          hour12: false,
          ...(includeUtcOffset
            ? { timeZoneName: "shortOffset" as const }
            : undefined),
        }
      : granularity === "day"
        ? { timeZone, month: "2-digit", day: "2-digit" }
        : { timeZone, year: "numeric", month: "2-digit" };
  const formatter = new Intl.DateTimeFormat(locale, options);
  bucketLabelFormatters.set(key, formatter);
  return formatter;
}

/**
 * Renders a statistics bucket start in the snapshot's own time zone and
 * granularity. The repeated-hour flag appends the UTC offset so both DST
 * hours stay distinguishable.
 */
export function formatUsageBucketLabel(
  startedAtMs: number,
  granularity: UsageStatisticsGranularityDto,
  timeZone: string,
  includeUtcOffset: boolean,
  locale: AppLocale,
): string {
  return bucketLabelFormatter(granularity, timeZone, includeUtcOffset, locale).format(
    startedAtMs,
  );
}

const dateTimeFormatters = new Map<AppLocale, Intl.DateTimeFormat>();
const compactDateFormatters = new Map<AppLocale, Intl.DateTimeFormat>();
const compactTimeFormatters = new Map<AppLocale, Intl.DateTimeFormat>();

function cachedFormatter(
  cache: Map<AppLocale, Intl.DateTimeFormat>,
  locale: AppLocale,
  options: Intl.DateTimeFormatOptions,
): Intl.DateTimeFormat {
  let formatter = cache.get(locale);
  if (!formatter) {
    formatter = new Intl.DateTimeFormat(locale, options);
    cache.set(locale, formatter);
  }
  return formatter;
}

export function formatUsageDateTime(value: number | null, locale: AppLocale) {
  if (value === null) return "-";
  return cachedFormatter(dateTimeFormatters, locale, {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    fractionalSecondDigits: 3,
  }).format(value);
}

export function formatUsageCompactDate(value: number, locale: AppLocale) {
  return cachedFormatter(compactDateFormatters, locale, {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
  }).format(value);
}

export function formatUsageCompactTime(value: number, locale: AppLocale) {
  return cachedFormatter(compactTimeFormatters, locale, {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  }).format(value);
}

export function formatUsagePreviewTime(value: number) {
  const date = new Date(value);
  const part = (number: number) => String(number).padStart(2, "0");
  return `${part(date.getMonth() + 1)}-${part(date.getDate())} ${part(date.getHours())}:${part(date.getMinutes())}`;
}
