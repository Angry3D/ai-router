import { describe, expect, it } from "vitest";

import {
  boundedBigIntRatios,
  boundedBigIntShares,
  compactToken,
  exactToken,
  formatCompactLatency,
  formatCompactUsd,
  formatLatency,
  formatStatisticsTokens,
  formatStatisticsUsd,
  formatUsd,
  latencyTone,
} from "./usageFormatting";

describe("Usage metric formatting", () => {
  it("preserves exact values and distinguishes unknown from zero", () => {
    expect(exactToken(null)).toBe("-");
    expect(exactToken(0)).toBe("0");
    expect(exactToken(12_345)).toBe("12,345");
  });

  it("compacts only the visible cached-input value", () => {
    expect(compactToken(null)).toBe("-");
    expect(compactToken(0)).toBe("0");
    expect(compactToken(999)).toBe("999");
    expect(compactToken(1_000)).toBe("1K");
    expect(compactToken(12_340)).toBe("12.3K");
    expect(compactToken(1_250_000)).toBe("1.3M");
  });

  it("formats latency around the ten-second visual boundary", () => {
    expect(formatLatency(null)).toBe("-");
    expect(formatLatency(0)).toBe("0.00 s");
    expect(formatLatency(9_999)).toBe("10.00 s");
    expect(formatLatency(10_000)).toBe("10.00 s");
    expect(latencyTone(null)).toBe("unknown");
    expect(latencyTone(9_999)).toBe("normal");
    expect(latencyTone(10_000)).toBe("warning");
  });

  it("compacts minute and hour latency only in the list", () => {
    expect(formatCompactLatency(null)).toBe("-");
    expect(formatCompactLatency(12_500)).toBe("12.50 s");
    expect(formatCompactLatency(60_000)).toBe("1m 00s");
    expect(formatCompactLatency(65_230)).toBe("1m 05s");
    expect(formatCompactLatency(65_999)).toBe("1m 05s");
    expect(formatCompactLatency(3_599_000)).toBe("59m 59s");
    expect(formatCompactLatency(3_599_999)).toBe("59m 59s");
    expect(formatCompactLatency(3_600_000)).toBe("1h 00m");
    expect(formatCompactLatency(3_720_000)).toBe("1h 02m");
    expect(formatCompactLatency(3_659_999)).toBe("1h 00m");
    expect(formatLatency(65_230)).toBe("65.23 s");
  });

  it("never rounds a tiny non-zero pico-USD amount to zero", () => {
    expect(formatUsd("50000")).toBe("$0.00000005");
    expect(formatUsd("1")).toBe("$0.000000000001");
  });

  it("compacts list costs without losing tiny non-zero amounts", () => {
    expect(formatCompactUsd(null)).toBe("不可用");
    expect(formatCompactUsd("0")).toBe("$0");
    expect(formatCompactUsd("20840000000")).toBe("$0.02084");
    expect(formatCompactUsd("3756375000")).toBe("$0.003756");
    expect(formatCompactUsd("275123")).toBe("$0.0000002751");
    expect(formatCompactUsd("1")).toBe("$0.000000000001");
  });

  it("rounds exact costs but truncates partial lower bounds", () => {
    expect(formatCompactUsd("2737900000")).toBe("$0.002738");
    expect(formatCompactUsd("2737900000", "down")).toBe("$0.002737");
  });

  it("formats statistics Tokens with only M and B units", () => {
    expect(formatStatisticsTokens("0")).toBe("0M");
    expect(formatStatisticsTokens("5000")).toBe("0.005M");
    expect(formatStatisticsTokens("110000")).toBe("0.11M");
    expect(formatStatisticsTokens("1390000")).toBe("1.39M");
    expect(formatStatisticsTokens("999499")).toBe("0.999M");
    expect(formatStatisticsTokens("999500")).toBe("1M");
    expect(formatStatisticsTokens("999999999")).toBe("1000M");
    expect(formatStatisticsTokens("1000000000")).toBe("1B");
    expect(formatStatisticsTokens("2410000000")).toBe("2.41B");
    expect(formatStatisticsTokens("9007199254740993000")).toBe(
      "9007199254.741B",
    );
  });

  it("rounds statistics cost to exactly two decimal places", () => {
    expect(formatStatisticsUsd("0")).toBe("$0.00");
    expect(formatStatisticsUsd("4999999999")).toBe("$0.00");
    expect(formatStatisticsUsd("5000000000")).toBe("$0.01");
    expect(formatStatisticsUsd("5100000000000")).toBe("$5.10");
    expect(formatStatisticsUsd("9007199254740993000")).toBe("$9007199.25");
  });

  it("bounds chart geometry without converting original integers to number", () => {
    expect(
      boundedBigIntRatios(["0", "9007199254740993", "18014398509481986"]),
    ).toEqual([0, 5000, 10000]);
    expect(boundedBigIntRatios(["0", "0"])).toEqual([0, 0]);
    expect(boundedBigIntShares(["1", "1", "2"])).toEqual([2500, 2500, 5000]);
    expect(boundedBigIntShares(["0", "0"])).toEqual([0, 0]);
  });
});
