import { describe, expect, it } from "vitest";

import fixtures from "../../../fixtures/base-url-contract.json";
import { previewBaseUrl } from "./baseUrlPreview";

describe("previewBaseUrl", () => {
  it.each(fixtures)("matches the Rust contract for $input", (fixture) => {
    const result = previewBaseUrl(fixture.input);
    if ("error" in fixture) {
      expect(result).toEqual({ valid: false, code: fixture.error });
      return;
    }

    expect(result).toEqual({
      valid: true,
      canonicalPrefix: fixture.canonical,
      inferenceUrl: fixture.inference,
    });
  });

  it("uses the UTF-8 byte limit", () => {
    expect(previewBaseUrl(`https://example.com/${"界".repeat(700)}`)).toEqual({
      valid: false,
      code: "base_url_too_long",
    });
  });
});
