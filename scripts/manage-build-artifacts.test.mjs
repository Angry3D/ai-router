import { EventEmitter } from "node:events";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { afterEach, describe, expect, it } from "vitest";

import {
  buildInvocation,
  cleanLegacyArtifacts,
  resolveLegacyTarget,
  runBuild,
} from "./manage-build-artifacts.mjs";

const temporaryRoots = [];

async function buildFixture() {
  const root = await mkdtemp(join(tmpdir(), "ai-router-artifacts-"));
  temporaryRoots.push(root);
  await Promise.all([
    mkdir(join(root, "src-tauri", "target", "old-build"), { recursive: true }),
    mkdir(join(root, "target", "release"), { recursive: true }),
  ]);
  await Promise.all([
    writeFile(
      join(root, "src-tauri", "target", "old-build", "artifact"),
      "legacy",
    ),
    writeFile(join(root, "target", "release", "artifact"), "canonical"),
  ]);
  return root;
}

afterEach(async () => {
  await Promise.all(
    temporaryRoots
      .splice(0)
      .map((root) => rm(root, { force: true, recursive: true })),
  );
});

describe("macOS build artifact management", () => {
  it("pins the Tauri application binary independently of helper targets", async () => {
    const config = JSON.parse(
      await readFile(
        resolve("src-tauri/tauri.conf.json"),
        "utf8",
      ),
    );

    expect(config.mainBinaryName).toBe("ai-router-app");
  });

  it("builds production and QA into the canonical workspace target", () => {
    const root = resolve("/tmp/ai-router-fixture");
    const production = buildInvocation(
      "production",
      root,
      { CARGO_TARGET_DIR: "/tmp/wrong" },
      "darwin",
    );
    const qa = buildInvocation("qa", root, {}, "darwin");
    const source = buildInvocation("source", root, {}, "darwin");
    const releaseConfig = join(root, "temporary-release.json");
    const release = buildInvocation(
      "release",
      root,
      {},
      "darwin",
      releaseConfig,
    );

    expect(production).toMatchObject({
      args: ["exec", "tauri", "build", "--bundles", "app"],
      command: "pnpm",
      cwd: root,
    });
    expect(production.env.CARGO_TARGET_DIR).toBe(join(root, "target"));
    expect(qa.args).toEqual([
      "exec",
      "tauri",
      "build",
      "--config",
      "src-tauri/tauri.qa.conf.json",
      "--bundles",
      "app",
    ]);
    expect(source.args).toEqual([
      "exec",
      "tauri",
      "build",
      "--bundles",
      "app",
      "--no-sign",
    ]);
    expect(source.env.CARGO_TARGET_DIR).toBe(join(root, "target"));
    expect(release.args).toEqual([
      "exec",
      "tauri",
      "build",
      "--config",
      releaseConfig,
      "--bundles",
      "dmg",
    ]);
  });

  it("rejects unknown build modes", () => {
    expect(() => buildInvocation("preview")).toThrow("Unknown app build mode");
    expect(() => buildInvocation("release")).toThrow(
      "generated updater configuration",
    );
  });

  it("propagates a failed native build", async () => {
    const spawnImpl = () => {
      const child = new EventEmitter();
      queueMicrotask(() => child.emit("exit", 7));
      return child;
    };

    await expect(runBuild("production", { spawnImpl })).rejects.toThrow(
      "exit code 7",
    );
  });

  it("cleans only the legacy target and remains idempotent", async () => {
    const root = await buildFixture();

    await expect(cleanLegacyArtifacts(root)).resolves.toBe(
      join(root, "src-tauri", "target"),
    );
    await expect(cleanLegacyArtifacts(root)).resolves.toBe(
      join(root, "src-tauri", "target"),
    );
    await expect(
      readFile(join(root, "target", "release", "artifact"), "utf8"),
    ).resolves.toBe("canonical");
  });

  it("rejects any cleanup candidate outside the fixed legacy root", () => {
    const root = resolve("/tmp/ai-router-fixture");

    expect(() => resolveLegacyTarget(root, join(root, "target"))).toThrow(
      "Refusing to clean unexpected path",
    );
    expect(() => resolveLegacyTarget(root, root)).toThrow(
      "Refusing to clean unexpected path",
    );
  });
});
