// Frontend mirror of `ImageModelProfile::for_model` in
// `crates/router-core/src/proxy/images.rs`. Both implementations are pinned to
// `fixtures/image-model-profile-contract.json`; Rust validation stays
// authoritative at call time, this table only drives the settings tooltip.
//
// The profile classification is locale-independent; every user-visible line
// comes from the caller's `ImageModelProfileMessages` segment.

import type { ImageModelProfileMessages } from "../language/catalog/imagesPricing";

export const IMAGE_MODEL_PRESETS = [
  "gpt-image-2.5-sunburst",
  "gpt-image-2.5-flare",
  "gpt-image-2",
] as const;

export type ImageModelPreset = (typeof IMAGE_MODEL_PRESETS)[number];

export type ImageModelProfile = "flexible_extended" | "flexible" | "standard";

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
  messages: ImageModelProfileMessages,
): readonly string[] {
  switch (profile) {
    case "flexible_extended":
      return [
        messages.png,
        messages.flexibleSize,
        messages.flexibleExperimental,
        messages.extendedQuality,
      ];
    case "flexible":
      return [
        messages.png,
        messages.flexibleSize,
        messages.flexibleExperimental,
        messages.baseQuality,
      ];
    case "standard":
      return [messages.png, messages.standardSize, messages.baseQuality];
  }
}

/**
 * Tooltip copy for the drafted model: the first line names the effective model
 * (`—` while the custom draft is empty) and the rest describe its profile.
 */
export function imageModelTooltipLines(
  model: string,
  messages: ImageModelProfileMessages,
): readonly [string, ...string[]] {
  const trimmed = model.trim();
  return [
    messages.modelTooltipTitle(trimmed || "—"),
    ...imageModelProfileLines(profileForModel(trimmed), messages),
  ];
}
