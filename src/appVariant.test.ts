import { describe, expect, it } from "vitest";

import { appVariantForMode, appVersionLabel } from "./appVariant";

describe("appVariantForMode", () => {
  it.each(["production", "test", "development"])(
    "keeps %s mode on the production identity",
    (mode) => {
      expect(appVariantForMode(mode)).toEqual({
        kind: "production",
        displayName: "AI Router",
        badge: null,
      });
    },
  );

  it("projects an unmistakable QA identity only for qa mode", () => {
    expect(appVariantForMode("qa")).toEqual({
      kind: "qa",
      displayName: "AI Router QA",
      badge: "QA",
    });
  });

  it("adds the QA designation to the shared version label", () => {
    expect(appVersionLabel("0.1.1", appVariantForMode("production"))).toBe("版本 0.1.1");
    expect(appVersionLabel("0.1.1", appVariantForMode("qa"))).toBe("版本 0.1.1 · QA");
  });
});
