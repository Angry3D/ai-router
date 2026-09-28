import type { RouteProtocol } from "../../generated/RouteProtocol";

const MAX_BASE_URL_BYTES = 2_048;

const TERMINAL_ENDPOINTS: Record<RouteProtocol, string> = {
  responses: "/responses",
  chat_completions: "/chat/completions",
};

const FOREIGN_ENDPOINTS: Record<RouteProtocol, string> = {
  responses: "/chat/completions",
  chat_completions: "/responses",
};

const DUPLICATE_ERROR_CODES: Record<RouteProtocol, BaseUrlPreviewErrorCode> = {
  responses: "base_url_duplicate_responses",
  chat_completions: "base_url_duplicate_chat_completions",
};

export type BaseUrlPreviewErrorCode =
  | "base_url_too_long"
  | "base_url_invalid"
  | "base_url_unsupported_endpoint"
  | "base_url_duplicate_responses"
  | "base_url_duplicate_chat_completions";

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

export function previewBaseUrl(
  value: string,
  protocol: RouteProtocol,
): BaseUrlPreview {
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
  if (normalizedPath.endsWith(FOREIGN_ENDPOINTS[protocol])) {
    return { valid: false, code: "base_url_unsupported_endpoint" };
  }

  const endpoint = TERMINAL_ENDPOINTS[protocol];
  const canonicalPath = normalizedPath.endsWith(endpoint)
    ? normalizedPath.slice(0, -endpoint.length).replace(/\/+$/, "")
    : normalizedPath;
  if (canonicalPath.endsWith(endpoint)) {
    return { valid: false, code: DUPLICATE_ERROR_CODES[protocol] };
  }

  const canonicalPrefix =
    normalizedPath === canonicalPath
      ? normalized
      : normalized.slice(0, -endpoint.length).replace(/\/+$/, "");
  return {
    valid: true,
    canonicalPrefix,
    inferenceUrl: `${canonicalPrefix}${endpoint}`,
  };
}
