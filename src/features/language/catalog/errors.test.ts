import { describe, expect, it } from "vitest";

import type { IpcErrorDto } from "../../../generated";
import { languageMessages } from "../languageMessages";
import { ipcErrorMessage } from "../useLanguage";
import { routesMessages } from "./routes";

const LOCALES = ["zh-CN", "en"] as const;

/** App-owned codes produced by Rust validation/lifecycle sites. */
const APP_OWNED_CODES = [
  "appearance_preference_invalid",
  "automatic_balance_refresh_out_of_range",
  "balance_script_required",
  "balance_script_too_large",
  "mcp_image_capacity_warning_out_of_range",
  "menu_balance_debounce_out_of_range",
  "database_retry_not_available",
  "database_recovery_not_required",
] as const;

function ipcError(code: string): IpcErrorDto {
  return { code, message: "raw diagnostic must not be rendered", retryable: true, field: null };
}

describe("ipcErrorMessage resolves app-owned codes", () => {
  it("renders a specific message, not the generic fallback, in both locales", () => {
    for (const locale of LOCALES) {
      const messages = languageMessages[locale];
      for (const code of APP_OWNED_CODES) {
        const rendered = ipcErrorMessage(ipcError(code), messages);
        expect(rendered, `${locale} ${code}`).not.toBe(messages.errors.genericFailure);
        expect(rendered.trim().length, `${locale} ${code}`).toBeGreaterThan(0);
      }
    }
  });

  it("keeps retry-unavailable distinct from recovery-not-required", () => {
    for (const locale of LOCALES) {
      const messages = languageMessages[locale];
      expect(
        ipcErrorMessage(ipcError("database_retry_not_available"), messages),
      ).not.toBe(ipcErrorMessage(ipcError("database_recovery_not_required"), messages));
    }
  });

  it("maps a code to its exact localized text", () => {
    expect(
      ipcErrorMessage(ipcError("appearance_preference_invalid"), languageMessages["zh-CN"]),
    ).toBe("外观偏好无效。");
    expect(
      ipcErrorMessage(ipcError("balance_script_required"), languageMessages.en),
    ).toBe("A script is required when using a custom balance source.");
  });

  it("falls back to genericFailure for unknown codes and never the raw message", () => {
    for (const locale of LOCALES) {
      const messages = languageMessages[locale];
      const rendered = ipcErrorMessage(ipcError("totally_unknown_code"), messages);
      expect(rendered).toBe(messages.errors.genericFailure);
      expect(rendered).not.toContain("raw diagnostic");
    }
  });
});

describe("routes fallback announcements pluralize correctly", () => {
  it("reads correctly for 0, 1 and N participants in English", () => {
    const list = routesMessages.en.list;
    expect(list.fallbackBoundaryItem(0)).toBe("Fallback boundary, 0 participating routes");
    expect(list.fallbackBoundaryItem(1)).toBe("Fallback boundary, 1 participating route");
    expect(list.fallbackBoundaryItem(5)).toBe("Fallback boundary, 5 participating routes");
  });

  it("keeps the real participant and route counts in the boundary description", () => {
    const boundary = routesMessages.en.boundary;
    expect(boundary.description(0, 0)).toBe("0 of 0 routes participating in fallback");
    expect(boundary.description(1, 1)).toBe("1 of 1 route participating in fallback");
    expect(boundary.description(2, 5)).toBe("2 of 5 routes participating in fallback");
  });

  it("leaves the Chinese fallback strings byte-identical", () => {
    expect(routesMessages["zh-CN"].list.fallbackBoundaryItem(2)).toBe(
      "Fallback 分界，2 条路由参与 Fallback",
    );
    expect(routesMessages["zh-CN"].boundary.description(2, 5)).toBe(
      "当前 2 条路由参与 Fallback，共 5 条路由",
    );
  });
});
