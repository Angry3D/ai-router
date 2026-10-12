import type { AppLocale } from "../../generated";

const mediumDateShortTimeFormatters = new Map<AppLocale, Intl.DateTimeFormat>();

/**
 * Pure formatting helper. The locale is passed explicitly because this module
 * must not read React context; callers hand in the effective app locale. The
 * formatter itself is cached per locale because table rows and status lines
 * reuse the same shape repeatedly.
 */
export function formatDateTime(value: number | null, locale: AppLocale) {
  if (value === null) return "-";
  let formatter = mediumDateShortTimeFormatters.get(locale);
  if (!formatter) {
    formatter = new Intl.DateTimeFormat(locale, {
      dateStyle: "medium",
      timeStyle: "short",
    });
    mediumDateShortTimeFormatters.set(locale, formatter);
  }
  return formatter.format(value);
}
