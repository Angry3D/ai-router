const MAX_BASE_URL_BYTES = 2_048;

export type BaseUrlPreviewErrorCode =
  | "base_url_too_long"
  | "base_url_invalid"
  | "base_url_unsupported_endpoint"
  | "base_url_duplicate_responses";

export type BaseUrlPreview =
  | {
      valid: true;
      canonicalPrefix: string;
      inferenceUrl: string;
    }
  | {
      valid: false;
      code: BaseUrlPreviewErrorCode;
    };

export function previewBaseUrl(value: string): BaseUrlPreview {
  const trimmed = value.trim();
  if (new TextEncoder().encode(trimmed).byteLength > MAX_BASE_URL_BYTES) {
    return { valid: false, code: "base_url_too_long" };
  }

  let parsed: URL;
  try {
    parsed = new URL(trimmed);
  } catch {
    return { valid: false, code: "base_url_invalid" };
  }

  if (
    !["http:", "https:"].includes(parsed.protocol) ||
    !parsed.hostname ||
    parsed.username !== "" ||
    parsed.password !== "" ||
    parsed.href.includes("?") ||
    parsed.href.includes("#")
  ) {
    return { valid: false, code: "base_url_invalid" };
  }

  const normalized = parsed.toString().replace(/\/+$/, "");
  const normalizedPath = parsed.pathname.replace(/\/+$/, "");
  if (normalizedPath.endsWith("/chat/completions")) {
    return { valid: false, code: "base_url_unsupported_endpoint" };
  }

  const canonicalPath = normalizedPath.endsWith("/responses")
    ? normalizedPath.slice(0, -"/responses".length).replace(/\/+$/, "")
    : normalizedPath;
  if (canonicalPath.endsWith("/responses")) {
    return { valid: false, code: "base_url_duplicate_responses" };
  }

  const canonicalPrefix =
    normalizedPath === canonicalPath
      ? normalized
      : normalized.slice(0, -"/responses".length).replace(/\/+$/, "");
  return {
    valid: true,
    canonicalPrefix,
    inferenceUrl: `${canonicalPrefix}/responses`,
  };
}
