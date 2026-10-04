import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { EventEmitter } from "node:events";
import {
  chmod,
  mkdir,
  mkdtemp,
  readFile,
  realpath,
  rm,
  stat,
  writeFile,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";

import {
  buildInvocation,
  cleanLegacyArtifacts,
  resolveLegacyTarget,
  runBuild,
} from "./manage-build-artifacts.mjs";
import { captureQaSource, qaBuildPaths } from "./qa-build-provenance.mjs";
import {
  QA_IDENTIFIER,
  createRunRoot,
  runCommand,
} from "./v0-2a-qa-common.mjs";
import {
  inspectQaBundle,
  launchQa,
  verifyQaBuild,
  verifyQaProcess,
} from "./v0-2a-qa-identity.mjs";

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

async function fixtureGit(root, ...args) {
  const result = await runCommand(
    "git",
    [
      "-c",
      "user.name=Synthetic QA",
      "-c",
      "user.email=qa@example.invalid",
      "-c",
      "commit.gpgsign=false",
      "-c",
      "core.hooksPath=/dev/null",
      ...args,
    ],
    { cwd: root },
  );
  if (result.code !== 0) throw new Error("Synthetic repository setup failed.");
}

async function qaFixture() {
  const root = await realpath(await buildFixture());
  await mkdir(join(root, "src"));
  await writeFile(
    join(root, ".gitignore"),
    "/target/\n/src-tauri/target/\n/src/private-data/\n",
  );
  await writeFile(join(root, "src/main.rs"), "original synthetic source");
  await writeFile(join(root, "Cargo.lock"), "synthetic lock input");
  await fixtureGit(root, "init", "--quiet");
  await fixtureGit(root, "add", ".");
  await fixtureGit(root, "commit", "--quiet", "-m", "Synthetic inputs");
  const paths = qaBuildPaths(root);
  const executablePath = join(paths.bundlePath, "Contents/MacOS/ai-router-app");
  const commandRunner = async (command, args) => {
    if (command !== "/usr/bin/plutil")
      throw new Error("Unexpected fixture command.");
    const plist = JSON.parse(await readFile(args.at(-1), "utf8"));
    return { code: 0, stdout: plist[args[1]], stderr: "" };
  };
  // A real child publishes synthetic bytes derived from the current source.
  // Tests assert receipt lifecycle, not forwarding or mocked build output.
  const build = (effect = "success") =>
    runBuild("qa", {
      root,
      commandRunner,
      spawnImpl: () =>
        spawn(
          process.execPath,
          [
            "--input-type=module",
            "-e",
            `
      import { mkdir, readFile, writeFile } from 'node:fs/promises';
      import { dirname, join } from 'node:path';
      const [root, executable, effect] = process.argv.slice(1);
      if (effect === 'fail') process.exit(7);
      await mkdir(dirname(executable), { recursive: true });
      await writeFile(executable, await readFile(join(root, 'src/main.rs')));
      await writeFile(join(dirname(executable), '../Info.plist'), JSON.stringify({
        CFBundleIdentifier: 'com.relax.airouter.qa',
        CFBundleName: 'AI Router QA', CFBundleShortVersionString: '0.0.0',
        CFBundleExecutable: 'ai-router-app'
      }));
      if (effect === 'mutate') await writeFile(join(root, 'src/main.rs'), 'changed during build');
    `,
            root,
            executablePath,
            effect,
          ],
          { stdio: "pipe" },
        ),
    });
  const verify = () =>
    verifyQaBuild(paths.bundlePath, { sourceRoot: root, commandRunner });
  return { root, ...paths, executablePath, commandRunner, build, verify };
}

afterEach(async () => {
  vi.restoreAllMocks();
  await Promise.all(
    temporaryRoots
      .splice(0)
      .map((root) => rm(root, { force: true, recursive: true })),
  );
});

describe("macOS build artifact management", () => {
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
      "app,dmg",
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

describe("QA build provenance transitions", () => {
  it("issues private evidence only after a successful build, never by inspection", async () => {
    const fixture = await qaFixture();
    await fixture.build();
    const { receipt } = await fixture.verify();
    expect(receipt).toMatchObject({
      schemaVersion: 1,
      identifier: QA_IDENTIFIER,
      ...(await captureQaSource(fixture.root)),
      executableSha256: createHash("sha256")
        .update("original synthetic source")
        .digest("hex"),
    });
    expect((await stat(fixture.receiptPath)).mode & 0o777).toBe(0o600);
    expect(fixture.receiptPath.startsWith(fixture.bundlePath)).toBe(false);

    await rm(fixture.receiptPath);
    await expect(
      inspectQaBundle(fixture.bundlePath, {
        expectedBundlePath: fixture.bundlePath,
        commandRunner: fixture.commandRunner,
      }),
    ).resolves.toMatchObject({ identifier: QA_IDENTIFIER });
    await expect(fixture.verify()).rejects.toThrow(
      "receipt is missing or invalid",
    );
    await expect(stat(fixture.receiptPath)).rejects.toMatchObject({
      code: "ENOENT",
    });
  });

  it.each(["fail", "mutate"])(
    "invalidates earlier evidence when a new build ends with %s",
    async (effect) => {
      const fixture = await qaFixture();
      await fixture.build();
      await fixture.verify();
      await expect(fixture.build(effect)).rejects.toThrow(
        effect === "fail" ? "exit code 7" : "source changed during the build",
      );
      await expect(fixture.verify()).rejects.toThrow(
        "receipt is missing or invalid",
      );
      await expect(stat(fixture.receiptPath)).rejects.toMatchObject({
        code: "ENOENT",
      });
    },
  );

  it.each(["dirty", "untracked", "deleted", "lock"])(
    "rejects same-HEAD %s build-input changes",
    async (change) => {
      const fixture = await qaFixture();
      await fixture.build();
      const original = await captureQaSource(fixture.root);
      if (change === "deleted") await rm(join(fixture.root, "src/main.rs"));
      else
        await writeFile(
          join(
            fixture.root,
            change === "untracked"
              ? "src/new.rs"
              : change === "lock"
                ? "Cargo.lock"
                : "src/main.rs",
          ),
          "changed input",
        );
      const updated = await captureQaSource(fixture.root);
      expect(updated.revision).toBe(original.revision);
      expect(updated.sourceFingerprint).not.toBe(original.sourceFingerprint);
      await expect(fixture.verify()).rejects.toThrow("source is stale");
    },
  );

  it("excludes task/docs changes, ignored private files and build outputs", async () => {
    const fixture = await qaFixture();
    await fixture.build();
    const original = await captureQaSource(fixture.root);
    for (const path of [
      "docs/guide.md",
      ".trellis/tasks/task.json",
      "src/private-data/synthetic.json",
      "target/output",
      "src-tauri/target/cache",
    ]) {
      await mkdir(join(fixture.root, path, ".."), { recursive: true });
      await writeFile(join(fixture.root, path), "unrelated synthetic data");
    }
    expect(await captureQaSource(fixture.root)).toEqual(original);
    await expect(fixture.verify()).resolves.toHaveProperty("receipt.buildId");
  });

  it("rejects a different source revision even when build-input content is unchanged", async () => {
    const fixture = await qaFixture();
    await fixture.build();
    const original = await captureQaSource(fixture.root);
    await writeFile(join(fixture.root, "README.md"), "synthetic documentation");
    await fixtureGit(fixture.root, "add", "README.md");
    await fixtureGit(
      fixture.root,
      "commit",
      "--quiet",
      "-m",
      "Synthetic revision",
    );
    const current = await captureQaSource(fixture.root);
    expect(current.sourceFingerprint).toBe(original.sourceFingerprint);
    expect(current.revision).not.toBe(original.revision);
    await expect(fixture.verify()).rejects.toThrow("source is stale");
  });

  it("binds local inputs present before building and rejects replaced executable bytes", async () => {
    const fixture = await qaFixture();
    await writeFile(
      join(fixture.root, "src/main.rs"),
      "dirty source at build time",
    );
    await writeFile(
      join(fixture.root, "src/new.rs"),
      "untracked input at build time",
    );
    await fixture.build();
    await fixture.verify();
    await writeFile(fixture.executablePath, "replacement binary");
    await expect(fixture.verify()).rejects.toThrow("executable does not match");
    await fixture.build();
    await expect(fixture.verify()).resolves.toHaveProperty("receipt.buildId");
  });

  it("rejects invalid receipt shape, identity and public permissions", async () => {
    const fixture = await qaFixture();
    await fixture.build();
    const original = await readFile(fixture.receiptPath, "utf8");
    for (const invalid of [
      {},
      { ...JSON.parse(original), identifier: "com.relax.airouter" },
      { ...JSON.parse(original), schemaVersion: 2 },
    ]) {
      await writeFile(fixture.receiptPath, JSON.stringify(invalid));
      await expect(fixture.verify()).rejects.toThrow(
        "receipt is missing or invalid",
      );
    }
    await writeFile(fixture.receiptPath, original);
    await chmod(fixture.receiptPath, 0o644);
    await expect(fixture.verify()).rejects.toThrow(
      "receipt is missing or invalid",
    );
  });

  it("rejects a canonical bundle whose real identity changes", async () => {
    const fixture = await qaFixture();
    await fixture.build();
    const plistPath = join(fixture.bundlePath, "Contents/Info.plist");
    const plist = JSON.parse(await readFile(plistPath, "utf8"));
    await writeFile(
      plistPath,
      JSON.stringify({ ...plist, CFBundleIdentifier: "com.relax.airouter" }),
    );
    await expect(fixture.verify()).rejects.toThrow("exact QA identifier");
  });

  it("refuses stale launch before spawning and preserves identity-only inspection", async () => {
    const fixture = await qaFixture();
    await fixture.build();
    await rm(fixture.receiptPath);
    const runRoot = await createRunRoot();
    temporaryRoots.push(runRoot.root);
    await expect(
      launchQa(fixture.bundlePath, runRoot.root, {
        sourceRoot: fixture.root,
        commandRunner: (command, args) =>
          command === "/bin/ps"
            ? Promise.resolve({ code: 0, stdout: "" })
            : fixture.commandRunner(command, args),
        spawnImpl: () => {
          throw new Error("Unverified QA must not spawn.");
        },
      }),
    ).rejects.toThrow("receipt is missing or invalid");
  });

  it("accepts only a fresh process of the verified build, not an old mapping at the same path", async () => {
    const fixture = await qaFixture();
    await fixture.build();
    const { receipt } = await fixture.verify();
    vi.spyOn(Date, "now").mockReturnValue(receipt.completedAtMs + 5000);
    let startedAtMs = receipt.completedAtMs - 2000;
    let executable = fixture.executablePath;
    const commandRunner = (command, args) => {
      if (command === "/usr/sbin/lsof") {
        return Promise.resolve({ code: 0, stdout: `n${executable}\n` });
      }
      if (command === "/bin/ps") {
        const [weekday, day, month, year, time] = new Date(startedAtMs)
          .toUTCString()
          .split(" ");
        return Promise.resolve({
          code: 0,
          stdout: `${weekday.slice(0, -1)} ${month} ${day} ${time} ${year}`,
        });
      }
      return fixture.commandRunner(command, args);
    };
    const verify = () =>
      verifyQaProcess(42, undefined, fixture.bundlePath, {
        sourceRoot: fixture.root,
        commandRunner,
      });
    await expect(verify()).rejects.toThrow("not proven newer than its build");
    startedAtMs = receipt.completedAtMs;
    await expect(verify()).rejects.toThrow("not proven newer than its build");
    startedAtMs = receipt.completedAtMs + 2000;
    await expect(verify()).resolves.toMatchObject({ pid: 42, receipt });
    executable = join(fixture.bundlePath, "Contents/MacOS/another-app");
    await writeFile(executable, "different process");
    await expect(verify()).rejects.toThrow(
      "does not execute the inspected QA bundle",
    );
  });
});
