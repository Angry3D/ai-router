import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";

import { createRunRoot } from "./v0-2a-qa-common.mjs";
import {
  persistScenarioEvidence,
  sanitizeScenarioEvidence,
} from "./v0-2a-qa-evidence.mjs";

const cleanupPaths = [];

afterEach(async () => {
  await Promise.all(
    cleanupPaths
      .splice(0)
      .map((path) => rm(path, { force: true, recursive: true })),
  );
});

function scenario() {
  return {
    schemaVersion: 1,
    scenarioId: "fallback-a-b-c",
    status: "passed",
    expectedAttempts: { A: 4, B: 4, C: 1, D: 0 },
    actualAttempts: { A: 4, B: 4, C: 1, D: 0 },
    attemptOrder: ["A", "A", "A", "A", "B", "B", "B", "B", "C"],
    elapsedMs: 120,
    clientClosed: null,
    unexpectedTrafficCount: 0,
  };
}

describe("V0.2A QA evidence allowlist", () => {
  it("persists only the fixed scenario projection", async () => {
    const prepared = await createRunRoot();
    cleanupPaths.push(prepared.root);
    await writeFile(
      join(prepared.root, "scenario-evidence.pending.json"),
      JSON.stringify([scenario()]),
      "utf8",
    );
    const evidenceRoot = await mkdtemp(
      join(tmpdir(), "ai-router-evidence-test-"),
    );
    cleanupPaths.push(evidenceRoot);

    const result = await persistScenarioEvidence(prepared.root, "run-123", {
      evidenceRoot,
    });
    const persisted = await readFile(result.outputPath, "utf8");
    expect(result.recordCount).toBe(1);
    expect(persisted).toContain('"scenarioId":"fallback-a-b-c"');
    expect(persisted).not.toContain("apiKey");
    expect(persisted).not.toContain("authorization");
  });

  it("rejects extra secret-bearing or nested fields before persistence", () => {
    expect(() =>
      sanitizeScenarioEvidence({
        ...scenario(),
        apiKey: "must-not-be-recorded",
      }),
    ).toThrow("non-allowlisted fields");
    expect(() =>
      sanitizeScenarioEvidence({
        ...scenario(),
        actualAttempts: {
          ...scenario().actualAttempts,
          token: "must-not-be-recorded",
        },
      }),
    ).toThrow("non-allowlisted fields");
  });
});
