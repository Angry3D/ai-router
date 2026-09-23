import { describe, expect, it } from "vitest";

import { formatModelWithActual, isRedirectedModel } from "./modelDisplay";

describe("isRedirectedModel", () => {
  it("treats an identical reported model as matched", () => {
    expect(isRedirectedModel("gpt-5.6-sol", "gpt-5.6-sol")).toBe(false);
  });

  it("ignores surrounding whitespace and casing", () => {
    expect(isRedirectedModel("  gpt-5.6-SOL  ", "gpt-5.6-sol")).toBe(false);
  });

  it("treats a different reported model, including a snapshot suffix, as redirected", () => {
    expect(isRedirectedModel("gpt-5.6-sol", "gpt-5.6-luna")).toBe(true);
    expect(isRedirectedModel("gpt-5.6-sol", "gpt-5.6-sol-2026-07-30")).toBe(
      true,
    );
  });

  it("treats a missing or blank reported model as unreported", () => {
    expect(isRedirectedModel("gpt-5.6-sol", null)).toBe(false);
    expect(isRedirectedModel("gpt-5.6-sol", "   ")).toBe(false);
  });

  it("treats two empty models as unreported", () => {
    expect(isRedirectedModel(null, null)).toBe(false);
  });

  it("treats a blank or absent requested model with a reported model as redirected", () => {
    expect(isRedirectedModel(null, "gpt-5.6-luna")).toBe(true);
    expect(isRedirectedModel("   ", "gpt-5.6-luna")).toBe(true);
  });
});

describe("formatModelWithActual", () => {
  it("renders a redirected pair as requested（actual）", () => {
    expect(formatModelWithActual("gpt-5.6-sol", "gpt-5.6-luna")).toBe(
      "gpt-5.6-sol（gpt-5.6-luna）",
    );
    expect(formatModelWithActual("gpt-5.6-sol", "gpt-5.6-sol-2026-07-30")).toBe(
      "gpt-5.6-sol（gpt-5.6-sol-2026-07-30）",
    );
  });

  it("renders a matched pair as the requested model alone", () => {
    expect(formatModelWithActual("gpt-5.6-sol", "gpt-5.6-sol")).toBe(
      "gpt-5.6-sol",
    );
    expect(formatModelWithActual("  gpt-5.6-SOL  ", "gpt-5.6-sol")).toBe(
      "gpt-5.6-SOL",
    );
  });

  it("never adds a suffix when the upstream reported no model", () => {
    expect(formatModelWithActual("gpt-5.6-sol", null)).toBe("gpt-5.6-sol");
    expect(formatModelWithActual("gpt-5.6-sol", "   ")).toBe("gpt-5.6-sol");
  });

  it("falls back to the reported model without a requested name, then to a dash", () => {
    expect(formatModelWithActual(null, "gpt-5.6-luna")).toBe("gpt-5.6-luna");
    expect(formatModelWithActual("   ", "gpt-5.6-luna")).toBe("gpt-5.6-luna");
    expect(formatModelWithActual(null, null)).toBe("-");
    expect(formatModelWithActual(null, "   ")).toBe("-");
  });
});
