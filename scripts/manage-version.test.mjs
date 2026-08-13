import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";

import { checkVersions, synchronizeVersions } from "./manage-version.mjs";

const temporaryRoots = [];

async function projectFixture({
  cargoVersion = "0.1.0",
  packageVersion = "0.1.0",
  tauriVersion = "0.1.1",
} = {}) {
  const root = await mkdtemp(join(tmpdir(), "ai-router-version-"));
  temporaryRoots.push(root);
  await mkdir(join(root, "src-tauri"));
  await Promise.all([
    writeFile(
      join(root, "Cargo.toml"),
      `[workspace]\nmembers = []\n\n[workspace.package]\nversion = "${cargoVersion}"\nedition = "2024"\n`,
    ),
    writeFile(
      join(root, "package.json"),
      `${JSON.stringify({ name: "fixture", version: packageVersion }, null, 2)}\n`,
    ),
    writeFile(
      join(root, "src-tauri", "tauri.conf.json"),
      `${JSON.stringify({ version: tauriVersion }, null, 2)}\n`,
    ),
  ]);
  return root;
}

afterEach(async () => {
  await Promise.all(temporaryRoots.splice(0).map((root) => rm(root, { force: true, recursive: true })));
});

describe("managed app versions", () => {
  it("synchronizes derived manifests from the Tauri version", async () => {
    const root = await projectFixture();

    await expect(synchronizeVersions(root)).resolves.toBe("0.1.1");
    await expect(checkVersions(root)).resolves.toBe("0.1.1");
    await expect(readFile(join(root, "Cargo.toml"), "utf8")).resolves.toContain('version = "0.1.1"');
    await expect(readFile(join(root, "package.json"), "utf8")).resolves.toContain('"version": "0.1.1"');
  });

  it("rejects derived version drift with the synchronization command", async () => {
    const root = await projectFixture({ cargoVersion: "0.1.1", packageVersion: "0.1.0" });

    await expect(checkVersions(root)).rejects.toThrow("Run pnpm version:sync");
  });

  it("rejects a malformed authoritative version", async () => {
    const root = await projectFixture({ tauriVersion: "v0.1.1" });

    await expect(synchronizeVersions(root)).rejects.toThrow("valid SemVer");
  });

  it("rejects an unsafe Cargo workspace version layout", async () => {
    const root = await projectFixture();
    await writeFile(join(root, "Cargo.toml"), "[workspace]\nmembers = []\n");

    await expect(checkVersions(root)).rejects.toThrow("[workspace.package]");
  });
});
