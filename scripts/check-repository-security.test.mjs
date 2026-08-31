import { execFile } from "node:child_process";
import { mkdtemp, mkdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { promisify } from "node:util";

import { afterEach, describe, expect, it } from "vitest";

import {
  checkRepositorySecurity,
  scanRepositoryText,
} from "./check-repository-security.mjs";

const execute = promisify(execFile);
const temporaryRoots = [];

afterEach(async () => {
  await Promise.all(
    temporaryRoots
      .splice(0)
      .map((root) => rm(root, { force: true, recursive: true })),
  );
});

describe("tracked public source security gate", () => {
  it("accepts the repository public-source projection", async () => {
    await expect(checkRepositorySecurity()).resolves.toMatchObject({
      declaredAssets: 29,
    });
  });

  it("reports secret and local-path fixture codes without retaining values", () => {
    const credential = ["sk", "fixturecredential012345678901"].join("-");
    const localPath = ["", "Users", "example", "private", "data"].join("/");
    const findings = scanRepositoryText(
      "fixture.txt",
      `token=${credential}\npath=${localPath}\n`,
    );

    expect(findings).toEqual([
      { code: "openai-style-key", line: 1, path: "fixture.txt" },
      { code: "macos-home", line: 2, path: "fixture.txt" },
    ]);
    expect(JSON.stringify(findings)).not.toContain("fixturecredential");
    expect(JSON.stringify(findings)).not.toContain(localPath);
  });

  it("scans textual SVG content with the same bounded findings", () => {
    const localPath = ["", "Users", "example", "private", "icon"].join("/");
    const findings = scanRepositoryText(
      "fixture.svg",
      `<svg><metadata>${localPath}</metadata></svg>`,
    );

    expect(findings).toEqual([
      { code: "macos-home", line: 1, path: "fixture.svg" },
    ]);
    expect(JSON.stringify(findings)).not.toContain(localPath);
  });

  it("rejects private workflow paths in public-tree mode", async () => {
    const root = await mkdtemp(join(tmpdir(), "ai-router-security-tree-"));
    temporaryRoots.push(root);
    await Promise.all([
      mkdir(join(root, "scripts")),
      mkdir(join(root, ".trellis")),
    ]);
    await Promise.all([
      writeFile(join(root, "README.md"), "Public source fixture.\n"),
      writeFile(join(root, ".trellis", "private.md"), "private fixture\n"),
      writeFile(
        join(root, "scripts", "license-policy.json"),
        JSON.stringify({
          declaredVisualAssets: [],
          publicTree: {
            forbiddenExactPaths: [],
            forbiddenPathPrefixes: [],
          },
        }),
      ),
    ]);
    await execute("git", ["init", "--initial-branch=main"], { cwd: root });
    await execute("git", ["add", "--all"], { cwd: root });

    await expect(
      checkRepositorySecurity(root, { rejectPrivatePaths: true }),
    ).rejects.toThrow(".trellis/private.md [private-path]");
  });
});
