import { describe, expect, it } from "vitest";

import { isRedirectedModel } from "./modelDisplay";

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
