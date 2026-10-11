import { describe, expect, it } from "vitest";

import { formatUsageBucketLabel } from "./usageRecordFormatting";

// The 2024 fall-back hour in America/New_York: one civil hour, two instants.
const fallBackEdt = Date.UTC(2024, 10, 3, 5, 0);
const fallBackEst = Date.UTC(2024, 10, 3, 6, 0);

describe("usage bucket labels", () => {
  it("keeps the repeated DST hour distinguishable with a UTC offset", () => {
    const plain = (startedAtMs: number) =>
      formatUsageBucketLabel(
        startedAtMs,
        "hour",
        "America/New_York",
        false,
        "zh-CN",
      );
    expect(plain(fallBackEdt)).toBe(plain(fallBackEst));

    const withOffset = (startedAtMs: number) =>
      formatUsageBucketLabel(
        startedAtMs,
        "hour",
        "America/New_York",
        true,
        "en",
      );
    expect(withOffset(fallBackEdt)).toContain("GMT-4");
    expect(withOffset(fallBackEst)).toContain("GMT-5");
    expect(withOffset(fallBackEdt)).not.toBe(withOffset(fallBackEst));
  });

  it("renders day and month buckets in the snapshot time zone", () => {
    expect(
      formatUsageBucketLabel(Date.UTC(2024, 7, 4), "day", "UTC", false, "zh-CN"),
    ).toBe("08/04");
    expect(
      formatUsageBucketLabel(
        Date.UTC(2024, 7, 1, 23, 0),
        "day",
        "UTC",
        false,
        "zh-CN",
      ),
    ).toBe("08/01");
    expect(
      formatUsageBucketLabel(
        Date.UTC(2025, 11, 1),
        "month",
        "UTC",
        false,
        "en",
      ),
    ).toBe("12/2025");
  });
});
