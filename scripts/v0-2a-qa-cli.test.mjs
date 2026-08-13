import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

import { runCommand } from "./v0-2a-qa-common.mjs";

const scripts = ["v0-2a-qa-fixture.mjs", "v0-2a-qa-identity.mjs"];

describe("V0.2A QA command usage", () => {
  for (const script of scripts) {
    it(`${script} rejects a missing command with usage`, async () => {
      const path = fileURLToPath(new URL(script, import.meta.url));
      const result = await runCommand(process.execPath, [path]);
      expect(result.code).not.toBe(0);
      expect(result.stderr).toContain("Usage:");
    });
  }
});
