import { createHash } from "node:crypto";
import { readFile, realpath, unlink } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import {
  PRODUCTION_IDENTIFIER,
  QA_BUNDLE_NAME,
  QA_IDENTIFIER,
  QaAcceptanceError,
  assertExactKeys,
  createRunRoot,
  optionValue,
  readJson,
  resolveRunRoot,
  runCommand,
  writeJsonAtomically,
} from "./v0-2a-qa-common.mjs";
import {
  canonicalQaBundle,
  executableFromLsof,
  inspectQaProcess,
  quitQa,
} from "./v0-2a-qa-identity.mjs";

const projectRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const canonicalProductionBundle = "/Applications/AI Router.app";
const productionBaselineFile = "production-continuity-baseline.json";
const actionPermitFile = "recovery-action-permit.json";
const evidenceFile = "recovery-evidence.json";
const allowedActions = new Set([
  "degrade",
  "corrupt-primary",
  "delete-primary",
  "future-schema",
  "permission-primary",
  "invalidate-points",
  "restore",
  "start-over",
  "publish-retention",
]);
const PRODUCTION_KEYS = [
  "schemaVersion",
  "pid",
  "identifier",
  "bundlePath",
  "executablePath",
  "bundleInfoSha256",
  "executableSha256",
];
const SUMMARY_KEYS = [
  "schemaVersion",
  "operation",
  "startup",
  "health",
  "validPointCount",
  "invalidPointCount",
  "candidates",
  "routeCount",
  "requestCount",
  "attemptCount",
  "quarantineCount",
  "codexConfigUnchanged",
  "retentionWithinLimit",
];
const CANDIDATE_KEYS = ["pointId", "createdAtMs", "criticalRevision"];
const EVIDENCE_ACTION_KEYS = [
  "action",
  "status",
  "qaPid",
  "startup",
  "health",
  "validPointCount",
  "invalidPointCount",
  "quarantineCount",
  "codexConfigUnchanged",
  "retentionWithinLimit",
];
const summaryOperations = new Set(["seed", "inspect", ...allowedActions]);
const startupStates = new Set([
  "new_install",
  "ready",
  "recovery_required",
  "fatal_permission",
  "fatal_diskfull",
  "fatal_futureschema",
  "fatal_unsafepath",
  "fatal_unavailable",
]);

function assertRequiredKeys(record, keys, label) {
  assertExactKeys(record, keys, label);
  const missing = keys.filter(
    (key) => !Object.prototype.hasOwnProperty.call(record, key),
  );
  if (missing.length > 0) {
    throw new QaAcceptanceError(
      `${label} is missing required fields: ${missing.join(", ")}.`,
    );
  }
}

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
    throw new QaAcceptanceError(`Unable to read ${key} from production bundle.`);
  }
  return result.stdout.trim();
}

async function sha256(path) {
  return createHash("sha256").update(await readFile(path)).digest("hex");
}

export async function inspectProductionContinuity(
  {
    bundlePath = canonicalProductionBundle,
    commandRunner = runCommand,
    expectedBundlePath = canonicalProductionBundle,
  } = {},
) {
  const [bundle, expected] = await Promise.all([
    realpath(bundlePath),
    realpath(expectedBundlePath),
  ]);
  if (bundle !== expected || !bundle.endsWith("/AI Router.app")) {
    throw new QaAcceptanceError(
      "Production continuity target is not the exact installed production bundle.",
    );
  }
  const plist = join(bundle, "Contents", "Info.plist");
  const [identifier, executableName] = await Promise.all([
    plistValue(plist, "CFBundleIdentifier", commandRunner),
    plistValue(plist, "CFBundleExecutable", commandRunner),
  ]);
  if (identifier !== PRODUCTION_IDENTIFIER || identifier === QA_IDENTIFIER) {
    throw new QaAcceptanceError(
      "Production continuity target does not have the exact production identifier.",
    );
  }
  const executablePath = await realpath(
    join(bundle, "Contents", "MacOS", executableName),
  );
  const processes = await commandRunner("/bin/ps", ["-axo", "pid=,comm="]);
  if (processes.code !== 0) {
    throw new QaAcceptanceError("Unable to inspect production process continuity.");
  }
  const candidates = processes.stdout
    .split("\n")
    .map((line) => /^\s*(\d+)\s+(.+)$/u.exec(line))
    .filter((match) => match?.[2] === executablePath)
    .map((match) => Number(match[1]));
  if (
    candidates.length !== 1 ||
    !Number.isSafeInteger(candidates[0]) ||
    candidates[0] <= 0
  ) {
    throw new QaAcceptanceError(
      "Production continuity requires one exact installed production PID.",
    );
  }
  const pid = candidates[0];
  const lsof = await commandRunner("/usr/sbin/lsof", [
    "-a",
    "-p",
    String(pid),
    "-d",
    "txt",
    "-Fn",
  ]);
  if (lsof.code !== 0) {
    throw new QaAcceptanceError("Unable to cross-check production PID executable.");
  }
  const processExecutable = await realpath(executableFromLsof(lsof.stdout));
  if (processExecutable !== executablePath) {
    throw new QaAcceptanceError(
      "Production PID does not execute the exact installed production bundle.",
    );
  }
  const projection = {
    schemaVersion: 1,
    pid,
    identifier,
    bundlePath: bundle,
    executablePath,
    bundleInfoSha256: await sha256(plist),
    executableSha256: await sha256(executablePath),
  };
  assertRequiredKeys(projection, PRODUCTION_KEYS, "production continuity");
  return projection;
}

export function assertProductionContinuity(baseline, current) {
  assertRequiredKeys(baseline, PRODUCTION_KEYS, "production continuity baseline");
  assertRequiredKeys(current, PRODUCTION_KEYS, "production continuity current");
  for (const key of PRODUCTION_KEYS) {
    if (baseline[key] !== current[key]) {
      throw new QaAcceptanceError(
        `Production continuity changed at allowlisted field ${key}.`,
      );
    }
  }
}

export function sanitizeRecoverySummary(value) {
  assertRequiredKeys(value, SUMMARY_KEYS, "recovery summary");
  if (
    value.schemaVersion !== 1 ||
    !summaryOperations.has(value.operation) ||
    !startupStates.has(value.startup) ||
    !(value.health === null || ["protected", "degraded"].includes(value.health)) ||
    !Array.isArray(value.candidates) ||
    value.candidates.length > 5
  ) {
    throw new QaAcceptanceError("Recovery summary identity is invalid.");
  }
  for (const candidate of value.candidates) {
    assertRequiredKeys(candidate, CANDIDATE_KEYS, "recovery candidate");
    if (
      typeof candidate.pointId !== "string" ||
      !/^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/u.test(
        candidate.pointId,
      ) ||
      !Number.isSafeInteger(candidate.createdAtMs) ||
      !Number.isSafeInteger(candidate.criticalRevision)
    ) {
      throw new QaAcceptanceError("Recovery candidate projection is invalid.");
    }
  }
  for (const key of [
    "validPointCount",
    "invalidPointCount",
    "quarantineCount",
  ]) {
    if (!Number.isSafeInteger(value[key]) || value[key] < 0) {
      throw new QaAcceptanceError(`Recovery summary ${key} is invalid.`);
    }
  }
  for (const key of ["routeCount", "requestCount", "attemptCount"]) {
    if (!(value[key] === null || (Number.isSafeInteger(value[key]) && value[key] >= 0))) {
      throw new QaAcceptanceError(`Recovery summary ${key} is invalid.`);
    }
  }
  if (
    typeof value.codexConfigUnchanged !== "boolean" ||
    typeof value.retentionWithinLimit !== "boolean"
  ) {
    throw new QaAcceptanceError("Recovery summary safety projection is invalid.");
  }
  return value;
}

export function sanitizeRecoveryEvidence(value) {
  assertRequiredKeys(value, ["schemaVersion", "actions"], "recovery evidence");
  if (value.schemaVersion !== 1 || !Array.isArray(value.actions)) {
    throw new QaAcceptanceError("Recovery evidence is invalid.");
  }
  for (const entry of value.actions) {
    assertRequiredKeys(entry, EVIDENCE_ACTION_KEYS, "recovery evidence action");
    if (
      !allowedActions.has(entry.action) ||
      entry.status !== "passed" ||
      !Number.isSafeInteger(entry.qaPid) ||
      entry.qaPid <= 0 ||
      !startupStates.has(entry.startup) ||
      !(entry.health === null || ["protected", "degraded"].includes(entry.health)) ||
      !["validPointCount", "invalidPointCount", "quarantineCount"].every(
        (key) => Number.isSafeInteger(entry[key]) && entry[key] >= 0,
      ) ||
      typeof entry.codexConfigUnchanged !== "boolean" ||
      typeof entry.retentionWithinLimit !== "boolean"
    ) {
      throw new QaAcceptanceError("Recovery evidence action is invalid.");
    }
  }
  return value;
}

async function runFixture(command, root, action, pointId, commandRunner) {
  const args = [
    "run",
    "-q",
    "-p",
    "router-core",
    "--example",
    "v0_2b_qa_recovery",
    "--",
    command,
    "--root",
    root,
  ];
  if (action) args.push("--action", action);
  if (pointId) args.push("--point-id", pointId);
  const result = await commandRunner("cargo", args, { cwd: projectRoot });
  if (result.code !== 0) {
    throw new QaAcceptanceError(
      `V0.2B recovery fixture failed without publishing evidence: ${result.stderr.trim()}`,
    );
  }
  return sanitizeRecoverySummary(JSON.parse(result.stdout));
}

async function loadProductionBaseline(root) {
  const value = await readJson(join(root, productionBaselineFile));
  assertRequiredKeys(value, PRODUCTION_KEYS, "production continuity baseline");
  return value;
}

async function verifyProductionBaseline(root, options) {
  const baseline = await loadProductionBaseline(root);
  const current = await inspectProductionContinuity(options);
  assertProductionContinuity(baseline, current);
  return current;
}

export async function issueRecoveryActionPermit(
  action,
  projection,
  { writeJson = writeJsonAtomically } = {},
) {
  if (!allowedActions.has(action)) {
    throw new QaAcceptanceError("Recovery action is not allowlisted.");
  }
  const root = await resolveRunRoot(projection.runRoot);
  if (
    projection.identifier !== QA_IDENTIFIER ||
    projection.bundlePath !== (await realpath(canonicalQaBundle)) ||
    !projection.bundlePath.endsWith(`/${QA_BUNDLE_NAME}`) ||
    projection.appDataDir !== join(root.root, "app-data") ||
    projection.codexHomeDir !== join(root.root, "app-data", "codex-home") ||
    projection.logDir !== join(root.root, "logs") ||
    projection.nonce !== root.nonce ||
    !Number.isSafeInteger(projection.pid) ||
    projection.pid <= 0
  ) {
    throw new QaAcceptanceError(
      "Refusing recovery action permit for a non-exact QA runtime projection.",
    );
  }
  const permitPath = join(root.root, actionPermitFile);
  await writeJson(permitPath, {
    schemaVersion: 1,
    nonce: root.nonce,
    action,
    pid: projection.pid,
    identifier: projection.identifier,
    bundlePath: projection.bundlePath,
    executablePath: projection.executablePath,
    appDataDir: projection.appDataDir,
    codexHomeDir: projection.codexHomeDir,
    logDir: projection.logDir,
  });
  return permitPath;
}

async function recordEvidence(root, pid, summary) {
  const path = join(root, evidenceFile);
  const existing = await readJson(path).catch((error) => {
    if (error.code === "ENOENT") {
      return { schemaVersion: 1, actions: [] };
    }
    throw error;
  });
  sanitizeRecoveryEvidence(existing);
  existing.actions.push({
    action: summary.operation,
    status: "passed",
    qaPid: pid,
    startup: summary.startup,
    health: summary.health,
    validPointCount: summary.validPointCount,
    invalidPointCount: summary.invalidPointCount,
    quarantineCount: summary.quarantineCount,
    codexConfigUnchanged: summary.codexConfigUnchanged,
    retentionWithinLimit: summary.retentionWithinLimit,
  });
  await writeJsonAtomically(path, existing);
}

export async function applyGuardedRecoveryAction(
  { action, bundle = canonicalQaBundle, pid, pointId, root },
  {
    commandRunner = runCommand,
    productionOptions = {},
    quitOptions = {},
  } = {},
) {
  if (!allowedActions.has(action)) {
    throw new QaAcceptanceError("Recovery action is not allowlisted.");
  }
  const resolved = await resolveRunRoot(root);
  await verifyProductionBaseline(resolved.root, productionOptions);
  const projection = await inspectQaProcess(pid, resolved.root, bundle, quitOptions);
  await quitQa(pid, bundle, resolved.root, quitOptions);
  await verifyProductionBaseline(resolved.root, productionOptions);
  const permitPath = await issueRecoveryActionPermit(action, projection);
  let summary;
  try {
    summary = await runFixture(
      "apply",
      resolved.root,
      action,
      pointId,
      commandRunner,
    );
  } finally {
    await unlink(permitPath).catch((error) => {
      if (error.code !== "ENOENT") throw error;
    });
  }
  await verifyProductionBaseline(resolved.root, productionOptions);
  await recordEvidence(resolved.root, pid, summary);
  return summary;
}

async function run() {
  const [command, ...arguments_] = process.argv.slice(2);
  const usage =
    "Usage: recovery <root|baseline|seed|inspect|apply> [--root PATH] [--pid PID] [--action ACTION]";
  if (!["root", "baseline", "seed", "inspect", "apply"].includes(command)) {
    throw new QaAcceptanceError(usage);
  }
  if (command === "root") {
    console.log(JSON.stringify(await createRunRoot(), null, 2));
    return;
  }
  const root = (await resolveRunRoot(optionValue(arguments_, "--root"))).root;
  if (command === "baseline") {
    const baseline = await inspectProductionContinuity();
    await writeJsonAtomically(join(root, productionBaselineFile), baseline);
    console.log(JSON.stringify(baseline, null, 2));
    return;
  }
  if (command === "seed" || command === "inspect") {
    console.log(JSON.stringify(await runFixture(command, root, null, null, runCommand), null, 2));
    return;
  }
  const action = optionValue(arguments_, "--action");
  const pid = Number(optionValue(arguments_, "--pid"));
  const pointId = arguments_.includes("--point-id")
    ? optionValue(arguments_, "--point-id")
    : null;
  console.log(
    JSON.stringify(
      await applyGuardedRecoveryAction({ action, pid, pointId, root }),
      null,
      2,
    ),
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

export {
  actionPermitFile,
  allowedActions,
  canonicalProductionBundle,
  evidenceFile,
  productionBaselineFile,
};
