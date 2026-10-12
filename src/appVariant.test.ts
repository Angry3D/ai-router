import { describe, expect, it } from "vitest";

import { appVariantForMode } from "./appVariant";

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
});
