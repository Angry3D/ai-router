export type AppVariant = Readonly<{
  kind: "production" | "qa";
  displayName: "AI Router" | "AI Router QA";
  badge: "QA" | null;
}>;

const PRODUCTION_VARIANT: AppVariant = {
  kind: "production",
  displayName: "AI Router",
  badge: null,
};

const QA_VARIANT: AppVariant = {
  kind: "qa",
  displayName: "AI Router QA",
  badge: "QA",
};

export function appVariantForMode(mode: string): AppVariant {
  return mode === "qa" ? QA_VARIANT : PRODUCTION_VARIANT;
}

export function appVersionLabel(version: string | null, variant: AppVariant) {
  return `版本 ${version ?? "—"}${variant.badge ? ` · ${variant.badge}` : ""}`;
}

export const appVariant = appVariantForMode(import.meta.env.MODE);
