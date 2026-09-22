// Frontend mirror of `ImageModelProfile::for_model` in
// `crates/router-core/src/proxy/images.rs`. Both implementations are pinned to
// `fixtures/image-model-profile-contract.json`; Rust validation stays
// authoritative at call time, this table only drives the settings tooltip.

export const IMAGE_MODEL_PRESETS = [
  "gpt-image-2.5-sunburst",
  "gpt-image-2.5-flare",
  "gpt-image-2",
] as const;

export type ImageModelPreset = (typeof IMAGE_MODEL_PRESETS)[number];

export type ImageModelProfile = "flexible_extended" | "flexible" | "standard";

const PNG_LINE = "每次生成一张 PNG。";
const FLEXIBLE_SIZE_LINE =
  "尺寸支持 auto 或 宽x高；两条边都是 16 的倍数，最长边不超过 3,840px，比例不超过 3:1，总像素为 655,360–8,294,400。";
const FLEXIBLE_EXPERIMENTAL_LINE =
  "超过 3,686,400 像素属于实验性范围。常用尺寸：1024x1024、1536x1024、1024x1536、2048x1152、3840x2160。";
const STANDARD_SIZE_LINE = "尺寸仅支持 auto、1024x1024、1536x1024、1024x1536。";
const BASE_QUALITY_LINE = "质量：low、medium、high、auto。";
const EXTENDED_QUALITY_LINE = "质量：low、medium、high、xhigh、max、auto。";

export function isImageModelPreset(model: string): model is ImageModelPreset {
  return (IMAGE_MODEL_PRESETS as readonly string[]).includes(model);
}

function matchesFamily(model: string, family: string): boolean {
  return model === family || model.startsWith(`${family}-`);
}

export function profileForModel(model: string): ImageModelProfile {
  const trimmed = model.trim();
  // `gpt-image-2.5` is checked before `gpt-image-2` on purpose, matching Rust.
  if (matchesFamily(trimmed, "gpt-image-2.5")) return "flexible_extended";
  if (matchesFamily(trimmed, "gpt-image-2")) return "flexible";
  if (
    matchesFamily(trimmed, "gpt-image-1.5") ||
    matchesFamily(trimmed, "gpt-image-1") ||
    trimmed === "chatgpt-image-latest"
  ) {
    return "standard";
  }
  return "flexible_extended";
}

export function imageModelProfileLines(
  profile: ImageModelProfile,
): readonly string[] {
  switch (profile) {
    case "flexible_extended":
      return [
        PNG_LINE,
        FLEXIBLE_SIZE_LINE,
        FLEXIBLE_EXPERIMENTAL_LINE,
        EXTENDED_QUALITY_LINE,
      ];
    case "flexible":
      return [
        PNG_LINE,
        FLEXIBLE_SIZE_LINE,
        FLEXIBLE_EXPERIMENTAL_LINE,
        BASE_QUALITY_LINE,
      ];
    case "standard":
      return [PNG_LINE, STANDARD_SIZE_LINE, BASE_QUALITY_LINE];
  }
}

/**
 * Tooltip copy for the drafted model: the first line names the effective model
 * (`—` while the custom draft is empty) and the rest describe its profile.
 */
export function imageModelTooltipLines(
  model: string,
): readonly [string, ...string[]] {
  const trimmed = model.trim();
  return [
    `模型：${trimmed || "—"}`,
    ...imageModelProfileLines(profileForModel(trimmed)),
  ];
}
