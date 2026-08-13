const PICO_USD_PER_USD = 1_000_000_000_000n;
const COMPACT_FIXED_DECIMAL_THRESHOLD = 1_000_000n;
const STATISTICS_MILLION = 1_000_000n;
const STATISTICS_BILLION = 1_000_000_000n;
const STATISTICS_RATIO_SCALE = 10_000n;

function groupInteger(value: string) {
  return value.replace(/\B(?=(\d{3})+(?!\d))/g, ",");
}

export function formatDecimalInteger(value: string) {
  return groupInteger(BigInt(value).toString());
}

export function compactDecimalInteger(value: string) {
  const integer = BigInt(value);
  if (integer < 1_000n) return groupInteger(integer.toString());
  const units = [
    [1_000_000_000n, "B"],
    [1_000_000n, "M"],
    [1_000n, "K"],
  ] as const;
  const [unit, suffix] =
    units.find(([threshold]) => integer >= threshold) ?? units[2];
  const whole = integer / unit;
  if (whole >= 100n) return `${groupInteger(whole.toString())}${suffix}`;
  const tenth = ((integer % unit) * 10n) / unit;
  return `${whole}${tenth === 0n ? "" : `.${tenth}`}${suffix}`;
}

export function formatUsd(picoUsd: string | null) {
  if (picoUsd === null) return "不可用";
  const value = BigInt(picoUsd);
  const whole = value / PICO_USD_PER_USD;
  const fraction = (value % PICO_USD_PER_USD)
    .toString()
    .padStart(12, "0")
    .replace(/0+$/, "");
  return `$${whole}${fraction ? `.${fraction}` : ""}`;
}

export function formatCompactUsd(
  picoUsd: string | null,
  rounding: "nearest" | "down" = "nearest",
) {
  if (picoUsd === null) return "不可用";
  const value = BigInt(picoUsd);
  if (value === 0n) return "$0";

  const decimalPlaces =
    value >= COMPACT_FIXED_DECIMAL_THRESHOLD
      ? 6
      : Math.min(12, 12 - value.toString().length + 4);
  const displayUnit = 10n ** BigInt(12 - decimalPlaces);
  const adjusted = rounding === "nearest" ? value + displayUnit / 2n : value;
  const displayedValue = (adjusted / displayUnit) * displayUnit;
  return formatUsd(displayedValue.toString());
}

export function formatStatisticsTokens(value: string) {
  const integer = BigInt(value);
  const [unit, suffix] =
    integer >= STATISTICS_BILLION
      ? ([STATISTICS_BILLION, "B"] as const)
      : ([STATISTICS_MILLION, "M"] as const);
  const roundedThousandths = (integer * 1_000n + unit / 2n) / unit;
  const whole = roundedThousandths / 1_000n;
  const fraction = (roundedThousandths % 1_000n)
    .toString()
    .padStart(3, "0")
    .replace(/0+$/, "");
  return `${whole}${fraction ? `.${fraction}` : ""}${suffix}`;
}

export function formatStatisticsUsd(picoUsd: string) {
  const cents =
    (BigInt(picoUsd) * 100n + PICO_USD_PER_USD / 2n) / PICO_USD_PER_USD;
  const whole = cents / 100n;
  const fraction = (cents % 100n).toString().padStart(2, "0");
  return `$${whole}.${fraction}`;
}

function nonNegativeDecimal(value: string) {
  const integer = BigInt(value);
  return integer > 0n ? integer : 0n;
}

export function boundedBigIntRatios(values: readonly string[]) {
  const integers = values.map(nonNegativeDecimal);
  const maximum = integers.reduce(
    (current, value) => (value > current ? value : current),
    0n,
  );
  if (maximum === 0n) return integers.map(() => 0);
  return integers.map((value) =>
    Number((value * STATISTICS_RATIO_SCALE) / maximum),
  );
}

export function boundedBigIntShares(values: readonly string[]) {
  const integers = values.map(nonNegativeDecimal);
  const total = integers.reduce((current, value) => current + value, 0n);
  if (total === 0n) return integers.map(() => 0);
  return integers.map((value) =>
    Number((value * STATISTICS_RATIO_SCALE) / total),
  );
}

export function exactToken(value: number | null) {
  return value === null ? "-" : value.toLocaleString();
}

export function compactToken(value: number | null) {
  if (value === null) return "-";
  if (value < 1_000) return value.toLocaleString();
  const divisor = value >= 1_000_000 ? 1_000_000 : 1_000;
  const suffix = value >= 1_000_000 ? "M" : "K";
  return `${(value / divisor).toFixed(1).replace(/\.0$/, "")}${suffix}`;
}

export function formatLatency(value: number | null) {
  return value === null ? "-" : `${(value / 1_000).toFixed(2)} s`;
}

export function formatCompactLatency(value: number | null) {
  if (value === null) return "-";
  if (value < 60_000) return formatLatency(value);

  const totalSeconds = Math.floor(value / 1_000);
  if (value < 3_600_000) {
    const minutes = Math.floor(totalSeconds / 60);
    const seconds = totalSeconds % 60;
    return `${minutes}m ${seconds.toString().padStart(2, "0")}s`;
  }

  const totalMinutes = Math.floor(value / 60_000);
  const hours = Math.floor(totalMinutes / 60);
  const minutes = totalMinutes % 60;
  return `${hours}h ${minutes.toString().padStart(2, "0")}m`;
}

export function latencyTone(value: number | null) {
  if (value === null) return "unknown" as const;
  return value >= 10_000 ? ("warning" as const) : ("normal" as const);
}
