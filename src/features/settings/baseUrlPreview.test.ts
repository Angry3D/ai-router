import { describe, expect, it } from "vitest";

import fixturesJson from "../../../fixtures/base-url-contract.json";
import type { RouteProtocol } from "../../generated/RouteProtocol";
import { previewBaseUrl } from "./baseUrlPreview";

type BaseUrlFixture = {
  input: string;
  protocol: RouteProtocol;
  canonical?: string;
  inference?: string;
  error?: string;
};

const fixtures = fixturesJson as BaseUrlFixture[];

describe("previewBaseUrl", () => {
  it.each(fixtures)(
    "matches the Rust contract for $protocol $input",
    (fixture) => {
      const result = previewBaseUrl(fixture.input, fixture.protocol);
      if (fixture.error !== undefined) {
        expect(result).toEqual({ valid: false, code: fixture.error });
        return;
      }

      expect(result).toEqual({
        valid: true,
        canonicalPrefix: fixture.canonical,
        inferenceUrl: fixture.inference,
      });
    },
  );

  it("uses the UTF-8 byte limit", () => {
    expect(previewBaseUrl(`https://example.com/${"界".repeat(700)}`, "responses")).toEqual({
      valid: false,
      code: "base_url_too_long",
    });
  });
});
