import { mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";

import {
  PRODUCTION_IDENTIFIER,
  QA_IDENTIFIER,
  createRunRoot,
} from "./v0-2a-qa-common.mjs";
import {
  assertProductionContinuity,
  inspectProductionContinuity,
  sanitizeRecoveryEvidence,
  sanitizeRecoverySummary,
} from "./v0-2b-qa-recovery.mjs";

const roots = [];

afterEach(async () => {
  await Promise.all(
    roots.splice(0).map((root) => rm(root, { force: true, recursive: true })),
  );
});

async function fakeProductionBundle() {
  const prepared = await createRunRoot();
  roots.push(prepared.root);
  const bundle = join(prepared.root, "AI Router.app");
  const executable = join(bundle, "Contents", "MacOS", "ai-router-app");
  await mkdir(join(bundle, "Contents", "MacOS"), { recursive: true });
  await Promise.all([
    writeFile(join(bundle, "Contents", "Info.plist"), "synthetic plist", "utf8"),
    writeFile(executable, "synthetic executable", "utf8"),
  ]);
  return { bundle, executable };
}

function commandRunner(fixture, identifier = PRODUCTION_IDENTIFIER) {
  return async (command, args) => {
    if (command === "/usr/bin/plutil") {
      return {
        code: 0,
        signal: null,
        stderr: "",
        stdout: `${args[1] === "CFBundleIdentifier" ? identifier : "ai-router-app"}\n`,
      };
    }
    if (command === "/bin/ps") {
      return {
        code: 0,
        signal: null,
        stderr: "",
        stdout: `42 ${fixture.executable}\n`,
      };
    }
    if (command === "/usr/sbin/lsof") {
      return {
        code: 0,
        signal: null,
        stderr: "",
        stdout: `p42\nftxt\nn${fixture.executable}\n`,
      };
    }
    throw new Error(`Unexpected command ${command}`);
  };
}

describe("V0.2B recovery QA safety tooling", () => {
  it("captures only an exact production PID and immutable bundle projection", async () => {
    const fixture = await fakeProductionBundle();
    const projection = await inspectProductionContinuity({
      bundlePath: fixture.bundle,
      commandRunner: commandRunner(fixture),
      expectedBundlePath: fixture.bundle,
    });
    expect(projection).toMatchObject({
      pid: 42,
      identifier: PRODUCTION_IDENTIFIER,
      bundlePath: fixture.bundle,
      executablePath: fixture.executable,
    });
    expect(projection.bundleInfoSha256).toHaveLength(64);

    await expect(
      inspectProductionContinuity({
        bundlePath: fixture.bundle,
        commandRunner: commandRunner(fixture, QA_IDENTIFIER),
        expectedBundlePath: fixture.bundle,
      }),
    ).rejects.toThrow("exact production identifier");
  });

  it("fails continuity when PID, path, or bundle bytes change", async () => {
    const fixture = await fakeProductionBundle();
    const baseline = await inspectProductionContinuity({
      bundlePath: fixture.bundle,
      commandRunner: commandRunner(fixture),
      expectedBundlePath: fixture.bundle,
    });
    expect(() => assertProductionContinuity(baseline, baseline)).not.toThrow();
    expect(() =>
      assertProductionContinuity(baseline, { ...baseline, pid: 43 }),
    ).toThrow("changed at allowlisted field pid");
    await writeFile(fixture.executable, "changed", "utf8");
    expect(await readFile(fixture.executable, "utf8")).toBe("changed");
  });

  it("accepts bounded secret-free recovery summaries and rejects extra fields", () => {
    const summary = {
      schemaVersion: 1,
      operation: "inspect",
      startup: "recovery_required",
      health: null,
      validPointCount: 1,
      invalidPointCount: 0,
      candidates: [
        {
          pointId: "00000000-0000-4000-8000-000000000000",
          createdAtMs: 1,
          criticalRevision: 2,
        },
      ],
      routeCount: null,
      requestCount: null,
      attemptCount: null,
      quarantineCount: 1,
      codexConfigUnchanged: true,
      retentionWithinLimit: true,
    };
    expect(sanitizeRecoverySummary(summary)).toEqual(summary);
    expect(() =>
      sanitizeRecoverySummary({ ...summary, apiKey: "forbidden" }),
    ).toThrow("non-allowlisted fields");
  });

  it("rejects non-allowlisted fields in previously recorded recovery evidence", () => {
    const action = {
      action: "restore",
      status: "passed",
      qaPid: 42,
      startup: "ready",
      health: "protected",
      validPointCount: 1,
      invalidPointCount: 0,
      quarantineCount: 1,
      codexConfigUnchanged: true,
      retentionWithinLimit: true,
    };
    expect(sanitizeRecoveryEvidence({ schemaVersion: 1, actions: [action] })).toEqual({
      schemaVersion: 1,
      actions: [action],
    });
    expect(() =>
      sanitizeRecoveryEvidence({
        schemaVersion: 1,
        actions: [{ ...action, apiKey: "forbidden" }],
      }),
    ).toThrow("non-allowlisted fields");
  });
});
