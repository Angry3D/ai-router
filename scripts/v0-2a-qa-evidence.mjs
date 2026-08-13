import { lstat, mkdir, realpath } from "node:fs/promises";
import { dirname, join, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

import {
  QaAcceptanceError,
  assertExactKeys,
  optionValue,
  readJson,
  resolveRunRoot,
  writeTextAtomically,
} from "./v0-2a-qa-common.mjs";

const SCENARIO_IDS = new Set([
  "recent-failure-direct-switch",
  "disconnect-confirmation",
  "fallback-a-b-c",
  "fallback-429",
  "fast-failure-budget",
  "timeout-budget",
  "no-next",
  "restart-persistence",
  "runtime-switch-sse",
  "runtime-switch-backoff",
  "responses-first-output-boundary",
  "responses-pre-boundary-commit",
  "responses-post-commit-failure",
  "responses-terminal-completed",
  "responses-terminal-done",
  "responses-cancellation",
  "settings-bounds",
  "settings-atomic-save",
  "settings-restart",
  "balance-debounce",
  "balance-auto-refresh",
]);
const STATUS_VALUES = new Set(["passed", "failed", "blocked"]);
const ROUTE_LABELS = ["A", "B", "C", "D"];
const RECORD_KEYS = [
  "schemaVersion",
  "scenarioId",
  "status",
  "expectedAttempts",
  "actualAttempts",
  "attemptOrder",
  "elapsedMs",
  "clientClosed",
  "unexpectedTrafficCount",
];
const projectRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const canonicalEvidenceRoot = join(
  projectRoot,
  ".trellis",
  "tasks",
  "07-26-v0-2a-isolated-native-qa-release-prep",
  "research",
  "native-qa",
);

function sanitizeCounts(value, label) {
  assertExactKeys(value, ROUTE_LABELS, label);
  const counts = {};
  for (const routeLabel of ROUTE_LABELS) {
    const count = value[routeLabel];
    if (!Number.isSafeInteger(count) || count < 0 || count > 1_000) {
      throw new QaAcceptanceError(`${label}.${routeLabel} is invalid.`);
    }
    counts[routeLabel] = count;
  }
  return counts;
}

export function sanitizeScenarioEvidence(value) {
  assertExactKeys(value, RECORD_KEYS, "scenario evidence");
  if (value.schemaVersion !== 1 || !SCENARIO_IDS.has(value.scenarioId)) {
    throw new QaAcceptanceError("Scenario evidence identity is invalid.");
  }
  if (!STATUS_VALUES.has(value.status)) {
    throw new QaAcceptanceError("Scenario evidence status is invalid.");
  }
  if (
    !Array.isArray(value.attemptOrder) ||
    value.attemptOrder.length > 64 ||
    value.attemptOrder.some((label) => !ROUTE_LABELS.includes(label))
  ) {
    throw new QaAcceptanceError("Scenario evidence attempt order is invalid.");
  }
  if (
    value.elapsedMs !== null &&
    (!Number.isSafeInteger(value.elapsedMs) ||
      value.elapsedMs < 0 ||
      value.elapsedMs > 3_600_000)
  ) {
    throw new QaAcceptanceError("Scenario evidence elapsed time is invalid.");
  }
  if (value.clientClosed !== null && typeof value.clientClosed !== "boolean") {
    throw new QaAcceptanceError("Scenario evidence client closure is invalid.");
  }
  if (
    !Number.isSafeInteger(value.unexpectedTrafficCount) ||
    value.unexpectedTrafficCount < 0 ||
    value.unexpectedTrafficCount > 1_000
  ) {
    throw new QaAcceptanceError(
      "Scenario evidence unexpected traffic count is invalid.",
    );
  }
  return {
    schemaVersion: 1,
    scenarioId: value.scenarioId,
    status: value.status,
    expectedAttempts: sanitizeCounts(
      value.expectedAttempts,
      "expected attempts",
    ),
    actualAttempts: sanitizeCounts(value.actualAttempts, "actual attempts"),
    attemptOrder: [...value.attemptOrder],
    elapsedMs: value.elapsedMs,
    clientClosed: value.clientClosed,
    unexpectedTrafficCount: value.unexpectedTrafficCount,
  };
}

export async function persistScenarioEvidence(
  root,
  runId,
  { evidenceRoot = canonicalEvidenceRoot } = {},
) {
  if (!/^[A-Za-z0-9-]{1,64}$/u.test(runId)) {
    throw new QaAcceptanceError("Evidence run ID is invalid.");
  }
  const resolvedRoot = await resolveRunRoot(root);
  const inputPath = join(resolvedRoot.root, "scenario-evidence.pending.json");
  const inputMetadata = await lstat(inputPath);
  if (!inputMetadata.isFile() || inputMetadata.isSymbolicLink()) {
    throw new QaAcceptanceError(
      "Pending scenario evidence is not a regular file.",
    );
  }
  const inputCanonical = await realpath(inputPath);
  if (!inputCanonical.startsWith(`${resolvedRoot.root}${sep}`)) {
    throw new QaAcceptanceError(
      "Pending scenario evidence escaped the run root.",
    );
  }
  const input = await readJson(inputCanonical);
  if (!Array.isArray(input) || input.length === 0 || input.length > 100) {
    throw new QaAcceptanceError(
      "Pending scenario evidence must be a bounded array.",
    );
  }
  const sanitized = input.map(sanitizeScenarioEvidence);

  await mkdir(evidenceRoot, { recursive: true, mode: 0o700 });
  const evidenceRootMetadata = await lstat(evidenceRoot);
  const canonicalRoot = await realpath(evidenceRoot);
  if (
    !evidenceRootMetadata.isDirectory() ||
    evidenceRootMetadata.isSymbolicLink()
  ) {
    throw new QaAcceptanceError("Evidence root is not a canonical directory.");
  }
  const outputDirectory = join(canonicalRoot, runId);
  await mkdir(outputDirectory, { mode: 0o700 });
  const outputMetadata = await lstat(outputDirectory);
  const outputCanonical = await realpath(outputDirectory);
  if (
    !outputMetadata.isDirectory() ||
    outputMetadata.isSymbolicLink() ||
    !outputCanonical.startsWith(`${canonicalRoot}${sep}`)
  ) {
    throw new QaAcceptanceError("Evidence output directory is invalid.");
  }
  const outputPath = join(outputCanonical, "scenarios.sanitized.jsonl");
  await writeTextAtomically(
    outputPath,
    `${sanitized.map((record) => JSON.stringify(record)).join("\n")}\n`,
  );
  return { outputPath, recordCount: sanitized.length };
}

async function run() {
  const [command, ...arguments_] = process.argv.slice(2);
  const usage = "Usage: evidence write --root PATH --run-id ID";
  if (command !== "write") throw new QaAcceptanceError(usage);
  const result = await persistScenarioEvidence(
    optionValue(arguments_, "--root"),
    optionValue(arguments_, "--run-id"),
  );
  console.log(JSON.stringify(result, null, 2));
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

export { canonicalEvidenceRoot };
