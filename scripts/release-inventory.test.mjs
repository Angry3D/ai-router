import { describe, expect, it, vi } from "vitest";

import {
  ReleaseInventoryError,
  collectReleaseInventory,
  renderReleaseInventory,
  selectStableBaseline,
} from "./release-inventory.mjs";
import { VersionError } from "./manage-version.mjs";

const SHA = {
  baseline: "1".repeat(40),
  candidate: "2".repeat(40),
  first: "3".repeat(40),
  second: "4".repeat(40),
};

function syntheticRunner(overrides = {}) {
  const outputs = new Map([
    ["for-each-ref", "v0.9.0\0tag\nv1.0.0\0tag\nfeature\0commit\n"],
    ["rev-parse:v1.0.0^{commit}", `${SHA.baseline}\n`],
    ["rev-parse:HEAD", `${SHA.candidate}\n`],
    [
      "log",
      `${SHA.first}\0First user change\n${SHA.second}\0Fix [unsafe] *markup* and ~~strike~~\n`,
    ],
    ["diff", "src/z.ts\0README (draft).md\0"],
    ...Object.entries(overrides),
  ]);
  return vi.fn(async (command, args) => {
    expect(command).toBe("git");
    const key = args[0] === "rev-parse" ? `rev-parse:${args[1]}` : args[0];
    if (!outputs.has(key)) throw new Error(`unexpected operation: ${key}`);
    const value = outputs.get(key);
    if (value instanceof Error) throw value;
    return value;
  });
}

describe("release-note inventory", () => {
  it("selects the highest reachable annotated stable SemVer tag", () => {
    expect(
      selectStableBaseline([
        { objectType: "tag", tag: "v1.9.9" },
        { objectType: "tag", tag: "v2.0.0-rc.1" },
        { objectType: "commit", tag: "other" },
        { objectType: "tag", tag: "v10.0.0" },
      ]),
    ).toEqual({ tag: "v10.0.0", version: "10.0.0" });
  });

  it("ignores non-canonical stable tag shapes", () => {
    expect(
      selectStableBaseline([
        { objectType: "tag", tag: "1.2.3" },
        { objectType: "tag", tag: "v01.2.3" },
        { objectType: "tag", tag: "v1.2.3.4" },
        { objectType: "tag", tag: "v2.0.0+build.1" },
        { objectType: "tag", tag: "v2.0.0-rc.1" },
        { objectType: "tag", tag: "v1.2.3" },
      ]),
    ).toEqual({ tag: "v1.2.3", version: "1.2.3" });
  });

  it("rejects missing baselines and lightweight stable tags", () => {
    expect(() =>
      selectStableBaseline([{ objectType: "commit", tag: "feature" }]),
    ).toThrow("No reachable annotated stable release tag");
    expect(() =>
      selectStableBaseline([{ objectType: "commit", tag: "v1.2.3" }]),
    ).toThrow("must be an annotated tag");
    expect(() =>
      selectStableBaseline([
        { objectType: "tag", tag: "v2.0.0" },
        { objectType: "commit", tag: "v1.0.0" },
      ]),
    ).toThrow("Stable baseline candidate v1.0.0 must be an annotated tag");
  });

  it("collects the exact range through read-only local Git operations", async () => {
    const runner = syntheticRunner();
    const versionChecker = vi.fn(async () => "1.1.0");
    const inventory = await collectReleaseInventory("/synthetic/repository", {
      runner,
      versionChecker,
    });

    expect(inventory).toEqual({
      baselineCommit: SHA.baseline,
      baselineTag: "v1.0.0",
      candidateCommit: SHA.candidate,
      candidateVersion: "1.1.0",
      commits: [
        { sha: SHA.first, subject: "First user change" },
        { sha: SHA.second, subject: "Fix [unsafe] *markup* and ~~strike~~" },
      ],
      paths: ["README (draft).md", "src/z.ts"],
    });
    expect(versionChecker).toHaveBeenCalledWith("/synthetic/repository");
    expect(runner.mock.calls.map(([, args]) => args)).toEqual([
      [
        "for-each-ref",
        "--merged=HEAD",
        "--format=%(refname:strip=2)%00%(objecttype)",
        "refs/tags",
      ],
      ["rev-parse", "v1.0.0^{commit}"],
      ["rev-parse", "HEAD"],
      ["log", "--reverse", "--format=%H%x00%s", "v1.0.0..HEAD"],
      ["diff", "--name-only", "-z", "v1.0.0..HEAD"],
    ]);
    for (const [, args, options] of runner.mock.calls) {
      expect(options).toMatchObject({
        cwd: "/synthetic/repository",
        encoding: "utf8",
        maxBuffer: 32 * 1024 * 1024,
      });
      expect(args.join(" ")).not.toMatch(
        /fetch|push|tag\s|update-ref|release|build|tauri|open|write/,
      );
    }
  });

  it("renders every commit and path once with deterministic safe Markdown", async () => {
    const inventory = await collectReleaseInventory("/synthetic/repository", {
      runner: syntheticRunner(),
      versionChecker: async () => "1.1.0",
    });
    const first = renderReleaseInventory(inventory);
    const second = renderReleaseInventory(inventory);

    expect(second).toBe(first);
    for (const sha of [SHA.first, SHA.second]) {
      expect(first.split(sha)).toHaveLength(2);
    }
    for (const path of ["README (draft).md", "src/z.ts"]) {
      expect(first.split(path)).toHaveLength(2);
    }
    expect(first.split("First user change")).toHaveLength(2);
    expect(first).toContain(
      "Fix \\[unsafe\\] \\*markup\\* and \\~\\~strike\\~\\~",
    );
    expect(first).toContain("兼容性、迁移操作和安装后重启影响");
    expect(first).toContain("merged PR 和已完成任务证据");
    expect(first).toContain("简明排除理由");
    expect(first).toContain("发布负责人已对版本说明");
  });

  it("propagates managed-version drift without invoking Git", async () => {
    const runner = syntheticRunner();
    await expect(
      collectReleaseInventory("/synthetic/repository", {
        runner,
        versionChecker: async () => {
          throw new VersionError("Version drift detected");
        },
      }),
    ).rejects.toThrow("Version drift detected");
    expect(runner).not.toHaveBeenCalled();
  });

  it("bounds unexpected version-check failures without exposing paths", async () => {
    const secret = ["", "Users", "example", "private", "package.json"].join(
      "/",
    );
    await expect(
      collectReleaseInventory(secret, {
        runner: syntheticRunner(),
        versionChecker: async () => {
          throw new Error(`EACCES: ${secret}`);
        },
      }),
    ).rejects.toSatisfy((error) => {
      expect(error).toBeInstanceOf(ReleaseInventoryError);
      expect(error.message).toBe(
        "Unable to validate managed version projections.",
      );
      expect(error.message).not.toContain(secret);
      return true;
    });
  });

  it("rejects empty ranges and malformed Git records", async () => {
    await expect(
      collectReleaseInventory("/synthetic/repository", {
        runner: syntheticRunner({ log: "" }),
        versionChecker: async () => "1.1.0",
      }),
    ).rejects.toThrow("contains no commits");
    await expect(
      collectReleaseInventory("/synthetic/repository", {
        runner: syntheticRunner({ diff: "src/a.ts\n" }),
        versionChecker: async () => "1.1.0",
      }),
    ).rejects.toThrow("Changed path metadata is malformed");
  });

  it("bounds Git failures without exposing command output or local paths", async () => {
    const secret = ["", "Users", "example", "private token=secret"].join("/");
    await expect(
      collectReleaseInventory(secret, {
        runner: syntheticRunner({
          "for-each-ref": new Error(`fatal: ${secret}`),
        }),
        versionChecker: async () => "1.1.0",
      }),
    ).rejects.toSatisfy((error) => {
      expect(error).toBeInstanceOf(ReleaseInventoryError);
      expect(error.message).toBe(
        "Unable to read local Git metadata for reachable tags.",
      );
      expect(error.message).not.toContain(secret);
      return true;
    });
  });
});
