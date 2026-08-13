import { mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";

import {
  QA_IDENTIFIER,
  createRunRoot,
  writeJsonAtomically,
} from "./v0-2a-qa-common.mjs";
import {
  cleanupRunRoot,
  executableFromLsof,
  findRunningQaPids,
  inspectQaBundle,
  inspectQaProcess,
  launchQa,
} from "./v0-2a-qa-identity.mjs";

const roots = [];

afterEach(async () => {
  await Promise.all(
    roots.splice(0).map((root) => rm(root, { force: true, recursive: true })),
  );
});

async function fakeBundle() {
  const prepared = await createRunRoot();
  roots.push(prepared.root);
  const bundle = join(prepared.root, "AI Router QA.app");
  const executable = join(bundle, "Contents", "MacOS", "ai-router-app");
  await mkdir(join(bundle, "Contents", "MacOS"), { recursive: true });
  await Promise.all([
    writeFile(join(bundle, "Contents", "Info.plist"), "fixture", "utf8"),
    writeFile(executable, "fixture", "utf8"),
  ]);
  return { ...prepared, bundle, executable };
}

function plistRunner(values, lsof = null, runningProcesses = []) {
  return async (command, args) => {
    if (command === "/usr/bin/plutil") {
      return {
        code: 0,
        stdout: `${values[args[1]]}\n`,
        stderr: "",
        signal: null,
      };
    }
    if (command === "/usr/sbin/lsof" && lsof !== null) {
      return {
        code: 0,
        stdout: `p42\nfcwd\nn${lsof}\n`,
        stderr: "",
        signal: null,
      };
    }
    if (command === "/bin/ps") {
      return {
        code: 0,
        stdout: runningProcesses
          .map(({ pid, executable }) => `${pid} ${executable}`)
          .join("\n"),
        stderr: "",
        signal: null,
      };
    }
    throw new Error(`Unexpected command: ${command}`);
  };
}

const qaPlist = {
  CFBundleIdentifier: QA_IDENTIFIER,
  CFBundleName: "AI Router QA",
  CFBundleShortVersionString: "0.1.1",
  CFBundleExecutable: "ai-router-app",
};

describe("V0.2A QA identity guard", () => {
  it("accepts only the exact canonical QA bundle projection", async () => {
    const fixture = await fakeBundle();
    await expect(
      inspectQaBundle(fixture.bundle, {
        commandRunner: plistRunner(qaPlist),
        expectedBundlePath: fixture.bundle,
      }),
    ).resolves.toMatchObject({
      identifier: QA_IDENTIFIER,
      bundleName: "AI Router QA",
      executablePath: fixture.executable,
    });

    await expect(
      inspectQaBundle(fixture.bundle, {
        commandRunner: plistRunner({
          ...qaPlist,
          CFBundleIdentifier: "com.relax.airouter",
        }),
        expectedBundlePath: fixture.bundle,
      }),
    ).rejects.toThrow("exact QA identifier");
  });

  it("cross-checks PID executable and the QA runtime marker", async () => {
    const fixture = await fakeBundle();
    await Promise.all([
      mkdir(join(fixture.root, "app-data"), { recursive: true }),
      mkdir(join(fixture.root, "app-data", "codex-home"), {
        recursive: true,
      }),
      mkdir(join(fixture.root, "logs"), { recursive: true }),
    ]);
    await writeJsonAtomically(join(fixture.root, "runtime-marker.json"), {
      schemaVersion: 1,
      nonce: fixture.nonce,
      pid: 42,
      identifier: QA_IDENTIFIER,
      executablePath: fixture.executable,
      appDataDir: join(fixture.root, "app-data"),
      codexHomeDir: join(fixture.root, "app-data", "codex-home"),
      logDir: join(fixture.root, "logs"),
    });

    await expect(
      inspectQaProcess(42, fixture.root, fixture.bundle, {
        commandRunner: plistRunner(qaPlist, fixture.executable),
        expectedBundlePath: fixture.bundle,
      }),
    ).resolves.toMatchObject({ pid: 42, nonce: fixture.nonce });

    const marker = JSON.parse(
      await readFile(join(fixture.root, "runtime-marker.json"), "utf8"),
    );
    marker.pid = 43;
    await writeJsonAtomically(
      join(fixture.root, "runtime-marker.json"),
      marker,
    );
    await expect(
      inspectQaProcess(42, fixture.root, fixture.bundle, {
        commandRunner: plistRunner(qaPlist, fixture.executable),
        expectedBundlePath: fixture.bundle,
      }),
    ).rejects.toThrow("runtime marker does not match");

    marker.pid = 42;
    marker.codexHomeDir = join(fixture.root, "app-data", "other-home");
    await mkdir(marker.codexHomeDir, { recursive: true });
    await writeJsonAtomically(
      join(fixture.root, "runtime-marker.json"),
      marker,
    );
    await expect(
      inspectQaProcess(42, fixture.root, fixture.bundle, {
        commandRunner: plistRunner(qaPlist, fixture.executable),
        expectedBundlePath: fixture.bundle,
      }),
    ).rejects.toThrow("runtime marker does not match");
  });

  it("requires exactly one app-bundle executable from process metadata", () => {
    expect(
      executableFromLsof("p42\nn/tmp/AI Router QA.app/Contents/MacOS/app\n"),
    ).toBe("/tmp/AI Router QA.app/Contents/MacOS/app");
    expect(() => executableFromLsof("p42\nn/usr/lib/dyld\n")).toThrow(
      "one exact QA process executable",
    );
  });

  it("refuses launch when the exact QA identifier already owns a process", async () => {
    const fixture = await fakeBundle();
    const commandRunner = plistRunner(qaPlist, null, [
      { pid: 41, executable: fixture.executable },
      { pid: 42, executable: fixture.executable },
    ]);
    await expect(findRunningQaPids(commandRunner)).resolves.toEqual([41, 42]);
    await expect(
      launchQa(fixture.bundle, fixture.root, {
        commandRunner,
        expectedBundlePath: fixture.bundle,
        spawnImpl: () => {
          throw new Error("spawn must not run");
        },
      }),
    ).rejects.toThrow("QA candidate PID(s) are running: 41, 42");
  });

  it("cleans up only a validated nonce-marked run root", async () => {
    const fixture = await fakeBundle();
    await expect(cleanupRunRoot(fixture.root)).resolves.toEqual({
      nonce: fixture.nonce,
      removedRoot: fixture.root,
    });
    const index = roots.indexOf(fixture.root);
    if (index !== -1) roots.splice(index, 1);

    await expect(cleanupRunRoot(fixture.root)).rejects.toThrow();
  });

  it("refuses cleanup while the runtime marker PID still exists", async () => {
    const fixture = await fakeBundle();
    await Promise.all([
      mkdir(join(fixture.root, "app-data", "codex-home"), {
        recursive: true,
      }),
      mkdir(join(fixture.root, "logs"), { recursive: true }),
    ]);
    await writeJsonAtomically(join(fixture.root, "runtime-marker.json"), {
      schemaVersion: 1,
      nonce: fixture.nonce,
      pid: 42,
      identifier: QA_IDENTIFIER,
      executablePath: fixture.executable,
      appDataDir: join(fixture.root, "app-data"),
      codexHomeDir: join(fixture.root, "app-data", "codex-home"),
      logDir: join(fixture.root, "logs"),
    });

    await expect(
      cleanupRunRoot(fixture.root, { processExistsImpl: () => true }),
    ).rejects.toThrow("recorded PID still exists");
  });
});
