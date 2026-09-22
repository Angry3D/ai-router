import { describe, expect, it } from "vitest";

import fixtures from "../../../fixtures/image-model-profile-contract.json";
import {
  IMAGE_MODEL_PRESETS,
  imageModelProfileLines,
  imageModelTooltipLines,
  isImageModelPreset,
  profileForModel,
} from "./imageModelProfile";

describe("profileForModel", () => {
  it.each(fixtures)("matches the Rust contract for $model", (fixture) => {
    expect(profileForModel(fixture.model)).toBe(fixture.profile);
  });

  it("exercises every profile through the shared fixture", () => {
    expect(new Set(fixtures.map((fixture) => fixture.profile))).toEqual(
      new Set(["flexible_extended", "flexible", "standard"]),
    );
  });
});

describe("IMAGE_MODEL_PRESETS", () => {
  it("lists exactly the current-generation models in order", () => {
    expect(IMAGE_MODEL_PRESETS).toEqual([
      "gpt-image-2.5-sunburst",
      "gpt-image-2.5-flare",
      "gpt-image-2",
    ]);
    expect(
      IMAGE_MODEL_PRESETS.map((preset) => profileForModel(preset)),
    ).toEqual(["flexible_extended", "flexible_extended", "flexible"]);
  });

  it("recognizes presets by exact identity only", () => {
    for (const preset of IMAGE_MODEL_PRESETS) {
      expect(isImageModelPreset(preset)).toBe(true);
    }
    expect(isImageModelPreset(" gpt-image-2")).toBe(false);
    expect(isImageModelPreset("gpt-image-1.5")).toBe(false);
    expect(isImageModelPreset("custom")).toBe(false);
    expect(isImageModelPreset("")).toBe(false);
  });
});

describe("imageModelTooltipLines", () => {
  const flexibleSizeLine =
    "尺寸支持 auto 或 宽x高；两条边都是 16 的倍数，最长边不超过 3,840px，比例不超过 3:1，总像素为 655,360–8,294,400。";
  const flexibleExperimentalLine =
    "超过 3,686,400 像素属于实验性范围。常用尺寸：1024x1024、1536x1024、1024x1536、2048x1152、3840x2160。";

  it("describes the flexible-extended profile with the extra quality levels", () => {
    expect(imageModelTooltipLines("gpt-image-2.5-flare")).toEqual([
      "模型：gpt-image-2.5-flare",
      "每次生成一张 PNG。",
      flexibleSizeLine,
      flexibleExperimentalLine,
      "质量：low、medium、high、xhigh、max、auto。",
    ]);
  });

  it("describes the flexible profile with the base quality levels", () => {
    expect(imageModelTooltipLines("gpt-image-2")).toEqual([
      "模型：gpt-image-2",
      "每次生成一张 PNG。",
      flexibleSizeLine,
      flexibleExperimentalLine,
      "质量：low、medium、high、auto。",
    ]);
  });

  it("describes the standard profile with the fixed size list", () => {
    expect(imageModelTooltipLines("gpt-image-1.5")).toEqual([
      "模型：gpt-image-1.5",
      "每次生成一张 PNG。",
      "尺寸仅支持 auto、1024x1024、1536x1024、1024x1536。",
      "质量：low、medium、high、auto。",
    ]);
    expect(imageModelProfileLines("standard")).not.toContain(
      flexibleExperimentalLine,
    );
  });

  it("trims the model line and falls back to a placeholder when empty", () => {
    expect(imageModelTooltipLines("  relay-image-2.5  ")[0]).toBe(
      "模型：relay-image-2.5",
    );
    const [modelLine, ...rules] = imageModelTooltipLines("   ");
    expect(modelLine).toBe("模型：—");
    expect(rules).toEqual(imageModelProfileLines("flexible_extended"));
  });
});
