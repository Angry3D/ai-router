import { spawn } from "node:child_process";
import { lstat, realpath, rm } from "node:fs/promises";
import { dirname, join, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

import {
  PRODUCTION_IDENTIFIER,
  QA_ACCEPTANCE_ROOT_ENV,
  QA_BUNDLE_NAME,
  QA_IDENTIFIER,
  QA_RUNTIME_MARKER_FILE,
  QaAcceptanceError,
  assertExactKeys,
  optionValue,
  readJson,
  resolveRunRoot,
  runCommand,
} from "./v0-2a-qa-common.mjs";

const projectRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const canonicalQaBundle = join(
  projectRoot,
  "target",
  "release",
  "bundle",
  "macos",
  QA_BUNDLE_NAME,
);
const BUNDLE_KEYS = [
  "bundlePath",
  "identifier",
  "bundleName",
  "version",
  "executablePath",
];
const PROCESS_KEYS = [
  ...BUNDLE_KEYS,
  "pid",
  "runRoot",
  "nonce",
  "appDataDir",
  "codexHomeDir",
  "logDir",
];
const RUNTIME_MARKER_KEYS = [
  "schemaVersion",
  "nonce",
  "pid",
  "identifier",
  "executablePath",
  "appDataDir",
  "codexHomeDir",
  "logDir",
];

async function plistValue(plist, key, commandRunner) {
  const result = await commandRunner("/usr/bin/plutil", [
    "-extract",
    key,
    "raw",
    "-o",
    "-",
    plist,
  ]);
  if (result.code !== 0) {
    throw new QaAcceptanceError(`Unable to read ${key} from the QA bundle.`);
  }
  return result.stdout.trim();
}

export async function inspectQaBundle(
  candidate,
  { commandRunner = runCommand, expectedBundlePath = canonicalQaBundle } = {},
) {
  const [bundlePath, expectedPath] = await Promise.all([
    realpath(candidate),
    realpath(expectedBundlePath),
  ]);
  if (
    bundlePath !== expectedPath ||
    !bundlePath.endsWith(`${sep}${QA_BUNDLE_NAME}`)
  ) {
    throw new QaAcceptanceError(
      "Lifecycle target is not the canonical QA bundle.",
    );
  }
  const plist = join(bundlePath, "Contents", "Info.plist");
  const [identifier, bundleName, version, executableName] = await Promise.all([
    plistValue(plist, "CFBundleIdentifier", commandRunner),
    plistValue(plist, "CFBundleName", commandRunner),
    plistValue(plist, "CFBundleShortVersionString", commandRunner),
    plistValue(plist, "CFBundleExecutable", commandRunner),
  ]);
  if (identifier === PRODUCTION_IDENTIFIER || identifier !== QA_IDENTIFIER) {
    throw new QaAcceptanceError(
      "Lifecycle target does not have the exact QA identifier.",
    );
  }
  if (bundleName !== "AI Router QA") {
    throw new QaAcceptanceError(
      "Lifecycle target does not have the exact QA product name.",
    );
  }
  const executablePath = await realpath(
    join(bundlePath, "Contents", "MacOS", executableName),
  );
  if (
    !executablePath.startsWith(`${join(bundlePath, "Contents", "MacOS")}${sep}`)
  ) {
    throw new QaAcceptanceError("QA executable escapes its bundle.");
  }
  const projection = {
    bundlePath,
    identifier,
    bundleName,
    version,
    executablePath,
  };
  assertExactKeys(projection, BUNDLE_KEYS, "QA bundle projection");
  return projection;
}

function executableFromLsof(output) {
  const candidates = output
    .split("\n")
    .filter((line) => line.startsWith("n"))
    .map((line) => line.slice(1))
    .filter((path) => path.includes(".app/Contents/MacOS/"));
  if (candidates.length !== 1) {
    throw new QaAcceptanceError(
      "Unable to resolve one exact QA process executable.",
    );
  }
  return candidates[0];
}

export async function inspectQaProcess(
  pid,
  root,
  bundlePath = canonicalQaBundle,
  { commandRunner = runCommand, expectedBundlePath = canonicalQaBundle } = {},
) {
  if (!Number.isSafeInteger(pid) || pid <= 0) {
    throw new QaAcceptanceError("QA PID must be a positive integer.");
  }
  const [bundle, runRoot] = await Promise.all([
    inspectQaBundle(bundlePath, { commandRunner, expectedBundlePath }),
    resolveRunRoot(root),
  ]);
  const lsof = await commandRunner("/usr/sbin/lsof", [
    "-a",
    "-p",
    String(pid),
    "-d",
    "txt",
    "-Fn",
  ]);
  if (lsof.code !== 0) {
    throw new QaAcceptanceError(
      "QA process is not running or cannot be inspected.",
    );
  }
  const processExecutable = await realpath(executableFromLsof(lsof.stdout));
  if (processExecutable !== bundle.executablePath) {
    throw new QaAcceptanceError(
      "QA PID does not execute the inspected QA bundle.",
    );
  }
  const marker = await readJson(join(runRoot.root, QA_RUNTIME_MARKER_FILE));
  assertExactKeys(marker, RUNTIME_MARKER_KEYS, "QA runtime marker");
  const [markerExecutable, markerAppData, markerCodexHome, markerLog] =
    await Promise.all([
      realpath(marker.executablePath),
      realpath(marker.appDataDir),
      realpath(marker.codexHomeDir),
      realpath(marker.logDir),
    ]);
  if (
    marker.schemaVersion !== 1 ||
    marker.nonce !== runRoot.nonce ||
    marker.pid !== pid ||
    marker.identifier !== QA_IDENTIFIER ||
    markerExecutable !== bundle.executablePath ||
    markerAppData !== join(runRoot.root, "app-data") ||
    markerCodexHome !== join(runRoot.root, "app-data", "codex-home") ||
    markerLog !== join(runRoot.root, "logs")
  ) {
    throw new QaAcceptanceError(
      "QA runtime marker does not match the target process.",
    );
  }
  const projection = {
    ...bundle,
    pid,
    runRoot: runRoot.root,
    nonce: runRoot.nonce,
    appDataDir: markerAppData,
    codexHomeDir: markerCodexHome,
    logDir: markerLog,
  };
  assertExactKeys(projection, PROCESS_KEYS, "QA process projection");
  return projection;
}

async function findRunningQaPids(commandRunner = runCommand) {
  const result = await commandRunner("/bin/ps", ["-axo", "pid=,comm="]);
  if (result.code !== 0) {
    throw new QaAcceptanceError(
      "Unable to inspect the exact QA application identity.",
    );
  }
  const candidates = result.stdout
    .split("\n")
    .map((line) => /^\s*(\d+)\s+(.+)$/u.exec(line))
    .filter((match) =>
      match?.[2].endsWith(".app/Contents/MacOS/ai-router-app"),
    );
  const pids = [];
  for (const match of candidates) {
    const pid = Number(match[1]);
    const executable = match[2];
    if (!Number.isSafeInteger(pid) || pid <= 0) {
      throw new QaAcceptanceError(
        "Exact QA application PID metadata is invalid.",
      );
    }
    const marker = ".app/Contents/MacOS/";
    const markerIndex = executable.lastIndexOf(marker);
    const bundlePath = executable.slice(0, markerIndex + 4);
    const identifier = await plistValue(
      join(bundlePath, "Contents", "Info.plist"),
      "CFBundleIdentifier",
      commandRunner,
    );
    if (identifier === QA_IDENTIFIER) pids.push(pid);
  }
  return [...new Set(pids)];
}

export async function launchQa(
  bundlePath,
  root,
  {
    commandRunner = runCommand,
    expectedBundlePath = canonicalQaBundle,
    spawnImpl = spawn,
  } = {},
) {
  const [bundle, runRoot, runningPids] = await Promise.all([
    inspectQaBundle(bundlePath, { commandRunner, expectedBundlePath }),
    resolveRunRoot(root),
    findRunningQaPids(commandRunner),
  ]);
  if (runningPids.length > 0) {
    throw new QaAcceptanceError(
      `Refusing to launch while QA candidate PID(s) are running: ${runningPids.join(", ")}.`,
    );
  }
  const child = spawnImpl(bundle.executablePath, [], {
    cwd: runRoot.root,
    detached: true,
    env: { ...process.env, [QA_ACCEPTANCE_ROOT_ENV]: runRoot.root },
    stdio: "ignore",
  });
  child.unref();
  return {
    pid: child.pid,
    bundlePath: bundle.bundlePath,
    runRoot: runRoot.root,
  };
}

export async function cleanupRunRoot(
  root,
  { processExistsImpl = processExists } = {},
) {
  const resolved = await resolveRunRoot(root);
  const markerPath = join(resolved.root, QA_RUNTIME_MARKER_FILE);
  const markerMetadata = await lstat(markerPath).catch((error) => {
    if (error.code === "ENOENT") return null;
    throw error;
  });
  if (markerMetadata !== null) {
    if (!markerMetadata.isFile() || markerMetadata.isSymbolicLink()) {
      throw new QaAcceptanceError("QA runtime marker is invalid for cleanup.");
    }
    const marker = await readJson(markerPath);
    assertExactKeys(marker, RUNTIME_MARKER_KEYS, "QA runtime marker");
    if (
      marker.schemaVersion !== 1 ||
      marker.nonce !== resolved.nonce ||
      marker.identifier !== QA_IDENTIFIER ||
      !Number.isSafeInteger(marker.pid) ||
      marker.pid <= 0
    ) {
      throw new QaAcceptanceError("QA runtime marker is invalid for cleanup.");
    }
    if (processExistsImpl(marker.pid)) {
      throw new QaAcceptanceError(
        "Refusing to clean a run root whose recorded PID still exists.",
      );
    }
  }
  await rm(resolved.root, {
    force: false,
    recursive: true,
    maxRetries: 2,
    retryDelay: 50,
  });
  return { nonce: resolved.nonce, removedRoot: resolved.root };
}

async function waitForExit(pid, processExists, timeoutMs = 10_000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (!processExists(pid)) return;
    await new Promise((resolvePromise) => setTimeout(resolvePromise, 100));
  }
  throw new QaAcceptanceError(
    "Exact QA process did not exit within the graceful timeout.",
  );
}

function processExists(pid) {
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    if (error.code === "ESRCH") return false;
    throw error;
  }
}

export async function quitQa(
  pid,
  bundlePath,
  root,
  {
    commandRunner = runCommand,
    processExistsImpl = processExists,
    expectedBundlePath = canonicalQaBundle,
    timeoutMs = 10_000,
  } = {},
) {
  const projection = await inspectQaProcess(pid, root, bundlePath, {
    commandRunner,
    expectedBundlePath,
  });
  const result = await commandRunner("/usr/bin/osascript", [
    "-e",
    `tell application id "${QA_IDENTIFIER}" to quit`,
  ]);
  if (result.code !== 0)
    throw new QaAcceptanceError("Exact QA graceful Quit failed.");
  await waitForExit(pid, processExistsImpl, timeoutMs);
  return projection;
}

async function run() {
  const [command, ...arguments_] = process.argv.slice(2);
  const usage =
    "Usage: identity <inspect-bundle|inspect-process|launch|quit|restart|cleanup> [options]";
  if (
    ![
      "inspect-bundle",
      "inspect-process",
      "launch",
      "quit",
      "restart",
      "cleanup",
    ].includes(command)
  ) {
    throw new QaAcceptanceError(usage);
  }
  const bundle = arguments_.includes("--bundle")
    ? optionValue(arguments_, "--bundle")
    : canonicalQaBundle;
  if (command === "inspect-bundle") {
    console.log(JSON.stringify(await inspectQaBundle(bundle), null, 2));
    return;
  }
  const root = optionValue(arguments_, "--root");
  if (command === "launch") {
    console.log(JSON.stringify(await launchQa(bundle, root), null, 2));
    return;
  }
  if (command === "cleanup") {
    console.log(JSON.stringify(await cleanupRunRoot(root), null, 2));
    return;
  }
  const pid = Number(optionValue(arguments_, "--pid"));
  if (command === "inspect-process") {
    console.log(
      JSON.stringify(await inspectQaProcess(pid, root, bundle), null, 2),
    );
    return;
  }
  if (command === "quit") {
    console.log(JSON.stringify(await quitQa(pid, bundle, root), null, 2));
    return;
  }
  if (command === "restart") {
    await quitQa(pid, bundle, root);
    console.log(JSON.stringify(await launchQa(bundle, root), null, 2));
    return;
  }
  throw new QaAcceptanceError(usage);
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

export { canonicalQaBundle, executableFromLsof, findRunningQaPids };
