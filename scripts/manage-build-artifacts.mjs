import { spawn } from "node:child_process";
import { rm } from "node:fs/promises";
import { dirname, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

const projectRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");

export class BuildArtifactError extends Error {}

export function resolveLegacyTarget(
  root,
  candidate = resolve(root, "src-tauri", "target"),
) {
  const resolvedRoot = resolve(root);
  const expected = resolve(resolvedRoot, "src-tauri", "target");
  const resolvedCandidate = resolve(candidate);
  if (
    resolvedCandidate !== expected ||
    resolvedCandidate === resolvedRoot ||
    !resolvedCandidate.startsWith(`${resolvedRoot}${sep}`)
  ) {
    throw new BuildArtifactError(
      `Refusing to clean unexpected path: ${resolvedCandidate}`,
    );
  }
  return expected;
}

export function buildInvocation(
  mode,
  root = projectRoot,
  baseEnvironment = process.env,
  platform = process.platform,
  releaseConfigPath,
) {
  if (
    mode !== "production" &&
    mode !== "qa" &&
    mode !== "source" &&
    mode !== "release"
  ) {
    throw new BuildArtifactError(`Unknown app build mode: ${mode}`);
  }

  const args = ["exec", "tauri", "build"];
  if (mode === "qa") {
    args.push("--config", "src-tauri/tauri.qa.conf.json");
  }
  if (mode === "release") {
    if (!releaseConfigPath) {
      throw new BuildArtifactError(
        "Release builds require a generated updater configuration.",
      );
    }
    args.push("--config", resolve(releaseConfigPath));
  }
  args.push("--bundles", mode === "release" ? "dmg" : "app");
  if (mode === "source") {
    args.push("--no-sign");
  }

  const resolvedRoot = resolve(root);
  return {
    args,
    command: platform === "win32" ? "pnpm.cmd" : "pnpm",
    cwd: resolvedRoot,
    env: {
      ...baseEnvironment,
      CARGO_TARGET_DIR: resolve(resolvedRoot, "target"),
    },
  };
}

export function runBuild(
  mode,
  {
    root = projectRoot,
    spawnImpl = spawn,
    releaseConfigPath,
    baseEnvironment = process.env,
  } = {},
) {
  const invocation = buildInvocation(
    mode,
    root,
    baseEnvironment,
    process.platform,
    releaseConfigPath,
  );
  return new Promise((resolvePromise, reject) => {
    const child = spawnImpl(invocation.command, invocation.args, {
      cwd: invocation.cwd,
      env: invocation.env,
      stdio: "inherit",
    });
    child.once("error", reject);
    child.once("exit", (code) => {
      if (code === 0) {
        resolvePromise();
        return;
      }
      reject(
        new BuildArtifactError(
          `${mode} app build failed with exit code ${code}.`,
        ),
      );
    });
  });
}

export async function cleanLegacyArtifacts(root = projectRoot, rmImpl = rm) {
  const legacyTarget = resolveLegacyTarget(root);
  await rmImpl(legacyTarget, { force: true, recursive: true });
  return legacyTarget;
}

async function run() {
  const [command, mode, ...extraArguments] = process.argv.slice(2);
  if (extraArguments.length > 0) {
    throw new BuildArtifactError(
      "Usage: node scripts/manage-build-artifacts.mjs <build production|build qa|build source|clean>",
    );
  }
  if (
    command === "build" &&
    (mode === "production" || mode === "qa" || mode === "source")
  ) {
    await runBuild(mode);
    return;
  }
  if (command === "clean" && mode === undefined) {
    const cleaned = await cleanLegacyArtifacts();
    console.log(`Cleaned legacy build artifacts: ${cleaned}`);
    return;
  }
  throw new BuildArtifactError(
    "Usage: node scripts/manage-build-artifacts.mjs <build production|build qa|build source|clean>",
  );
}

if (
  process.argv[1] &&
  resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  run().catch((error) => {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = 1;
  });
}
