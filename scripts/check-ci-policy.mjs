import { readFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { parse } from "yaml";

const SCRIPT_ROOT = dirname(fileURLToPath(import.meta.url));
const DEFAULT_PROJECT_ROOT = resolve(SCRIPT_ROOT, "..");
const WORKFLOW_FILES = [
  ".github/workflows/ci.yml",
  ".github/workflows/native-source-build.yml",
  ".github/workflows/security.yml",
];
const RELEASE_WORKFLOW_FILE = ".github/workflows/release.yml";
const RELEASE_SCRIPT_FILE = "scripts/manage-release.mjs";
const REQUIRED_CHECKS = new Map([
  ["node-quality", "Required / Node quality"],
  ["rust-quality", "Required / Rust quality"],
  ["generated-contracts", "Required / Generated and contracts"],
  ["protocol-compatibility", "Required / Protocol compatibility"],
  ["repository-policy", "Required / Repository policy"],
]);
const SECURITY_CHECKS = new Map([
  ["dependency-review", "Security / Dependency review"],
  ["codeql", "Security / CodeQL"],
]);
const DEPENDABOT_ECOSYSTEMS = new Map([
  ["npm", "09:00"],
  ["cargo", "09:15"],
  ["github-actions", "09:30"],
]);
const DEPENDABOT_ALLOWED_UPDATE_TYPES = [
  "version-update:semver-minor",
  "version-update:semver-patch",
];
const DEPENDABOT_GROUP_UPDATE_TYPES = ["minor", "patch"];
const REQUIRED_JOB_COMMANDS = new Map([
  [
    "node-quality",
    [
      "corepack enable",
      "pnpm install --frozen-lockfile",
      "pnpm lint",
      "pnpm typecheck",
      "pnpm test",
      "pnpm build",
    ],
  ],
  [
    "rust-quality",
    [
      "cargo fmt --check",
      "cargo clippy --workspace --all-targets -- -D warnings",
      "cargo test --workspace",
    ],
  ],
  [
    "generated-contracts",
    [
      "corepack enable",
      "pnpm install --frozen-lockfile",
      "pnpm contracts:check",
      "pnpm version:check",
    ],
  ],
  [
    "protocol-compatibility",
    [
      "corepack enable",
      "pnpm install --frozen-lockfile",
      "pnpm check:codex-retries",
    ],
  ],
  [
    "repository-policy",
    [
      "corepack enable",
      "pnpm install --frozen-lockfile",
      "pnpm docs:check",
      "pnpm ci:policy",
      "pnpm security:public:check",
      "pnpm license:public:check",
    ],
  ],
]);
const REVIEWED_ACTIONS = new Map([
  ["actions/checkout", ["d23441a48e516b6c34aea4fa41551a30e30af803", "v6.1.0"]],
  [
    "actions/setup-node",
    ["249970729cb0ef3589644e2896645e5dc5ba9c38", "v6.5.0"],
  ],
  [
    "actions/dependency-review-action",
    ["2031cfc080254a8a887f58cffee85186f0e49e48", "v4.9.0"],
  ],
  [
    "github/codeql-action/init",
    ["ff2f1c621b7f889edc0d3c761ac2e6a3f8cdb0dd", "v4.37.7"],
  ],
  [
    "github/codeql-action/analyze",
    ["ff2f1c621b7f889edc0d3c761ac2e6a3f8cdb0dd", "v4.37.7"],
  ],
  [
    "actions/attest-build-provenance",
    ["e8998f949152b193b063cb0ec769d69d929409be", "v3.0.0"],
  ],
]);
const ACTION_PIN =
  /^\s*-?\s*uses:\s+([A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+(?:\/[A-Za-z0-9_.-]+)?)@([a-f0-9]{40})\s+#\s+(v\d[^\s]*)\s*$/gm;
const ACTION_PIN_LINE =
  /^\s*-?\s*uses:\s+([A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+(?:\/[A-Za-z0-9_.-]+)?)@([a-f0-9]{40})\s+#\s+(v\d[^\s]*)\s*$/;
const ANY_EXTERNAL_ACTION =
  /^\s*-?\s*uses:\s+([A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+(?:\/[A-Za-z0-9_.-]+)?)@([^\s#]+).*$/gm;
const ANY_ACTION_REFERENCE = /^\s*-?\s*uses:\s+([^\s#]+).*$/gm;
const FORBIDDEN_WORKFLOW_TEXT = [
  ["secret context", /\bsecrets\s*(?:\.|\[)/i],
  ["artifact upload", /actions\/upload-artifact|upload-artifact/i],
  [
    "release publication",
    /\bgh\s+release\b|softprops\/action-gh-release|create-release/i,
  ],
  [
    "signing or notarization",
    /developer_id|apple_certificate|notar(?:y|ize)|codesign/i,
  ],
  [
    "production lifecycle control",
    /\/Applications\/AI Router\.app|\b(?:open|kill|pkill|launchctl|osascript)\b/i,
  ],
  [
    "production application data",
    /Library\/Application Support\/com\.relax\.airouter|(?:^|[/"'])\.codex\/|\bCODEX_HOME\b/im,
  ],
];

export class CiPolicyError extends Error {}

function fail(message) {
  throw new CiPolicyError(message);
}

function parseYaml(path, content) {
  try {
    return parse(content);
  } catch (error) {
    fail(`${path} is not valid YAML: ${error.message}`);
  }
}

function asObject(value, label) {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    fail(`${label} must be a mapping.`);
  }
  return value;
}

function assertTopLevelReadOnlyPermissions(path, workflow) {
  const permissions = asObject(workflow.permissions, `${path} permissions`);
  const entries = Object.entries(permissions);
  if (
    entries.length !== 1 ||
    entries[0][0] !== "contents" ||
    entries[0][1] !== "read"
  ) {
    fail(`${path} must default to contents: read and no other permission.`);
  }
}

function assertJobPermissions(path, workflow) {
  for (const [jobId, job] of Object.entries(
    asObject(workflow.jobs, `${path} jobs`),
  )) {
    if (job.permissions === undefined) continue;
    const permissions = asObject(
      job.permissions,
      `${path} job ${jobId} permissions`,
    );
    if (
      path !== ".github/workflows/security.yml" ||
      jobId !== "codeql" ||
      Object.keys(permissions).length !== 4 ||
      permissions.actions !== "read" ||
      permissions.contents !== "read" ||
      permissions.packages !== "read" ||
      permissions["security-events"] !== "write"
    ) {
      fail(
        `${path} job ${jobId} must not override permissions; only CodeQL may request its exact private-repository read scopes and security-events: write.`,
      );
    }
  }
}

function assertCheckoutPerJob(path, workflow) {
  for (const [jobId, job] of Object.entries(workflow.jobs)) {
    const checkoutSteps = (job.steps ?? []).filter(
      (step) =>
        typeof step?.uses === "string" &&
        step.uses.startsWith("actions/checkout@"),
    );
    if (checkoutSteps.length !== 1) {
      fail(
        `${path} job ${jobId} must use exactly one immutable checkout action.`,
      );
    }
    if (checkoutSteps[0].with?.["persist-credentials"] !== false) {
      fail(
        `${path} job ${jobId} checkout must set persist-credentials: false.`,
      );
    }
  }
}

function assertExactTriggerKeys(label, triggers, expected) {
  const actual = Object.keys(triggers).sort();
  const wanted = [...expected].sort();
  if (
    actual.length !== wanted.length ||
    actual.some((key, index) => key !== wanted[index])
  ) {
    fail(`${label} must define only these triggers: ${wanted.join(", ")}.`);
  }
}

function assertUnconditionalJob(path, jobId, job) {
  if (job.if !== undefined || job.needs !== undefined) {
    fail(
      `${path} required job ${jobId} must not be conditional or depend on another job.`,
    );
  }
}

function assertRunCommands(path, jobId, job, commands) {
  const runSteps = (job.steps ?? []).filter(
    (step) => typeof step?.run === "string",
  );
  for (const command of commands) {
    const matches = runSteps.filter((step) => step.run.trim() === command);
    if (matches.length !== 1) {
      fail(`${path} job ${jobId} must run exactly once: ${command}.`);
    }
  }
}

function requiredActionStep(path, jobId, job, action) {
  const matches = (job.steps ?? []).filter(
    (step) =>
      typeof step?.uses === "string" && step.uses.startsWith(`${action}@`),
  );
  if (matches.length !== 1) {
    fail(`${path} job ${jobId} must use exactly one ${action} action.`);
  }
  return matches[0];
}

function assertPinnedNodeSetup(path, jobId, job) {
  const setup = requiredActionStep(path, jobId, job, "actions/setup-node");
  if (setup.with?.["node-version-file"] !== ".nvmrc") {
    fail(`${path} job ${jobId} must select Node through .nvmrc.`);
  }
}

export function validateActionPins(path, content) {
  const references = [...content.matchAll(ANY_ACTION_REFERENCE)];
  const pinnedCount = [...content.matchAll(ACTION_PIN)].length;
  const external = [...content.matchAll(ANY_EXTERNAL_ACTION)];
  if (references.length !== external.length) {
    fail(`${path} contains an unsupported or mutable action reference.`);
  }
  if (external.length === 0) fail(`${path} must use an external action.`);
  if (pinnedCount !== external.length) {
    fail(`${path} action references must use a full SHA and version comment.`);
  }
  for (const match of external) {
    const pin = ACTION_PIN_LINE.exec(match[0]);
    if (!pin || pin[2] !== match[2]) {
      fail(
        `${path} action ${match[1]} must use a full SHA and version comment.`,
      );
    }
    const reviewed = REVIEWED_ACTIONS.get(match[1]);
    if (!reviewed || reviewed[0] !== pin[2] || reviewed[1] !== pin[3]) {
      fail(
        `${path} action ${match[1]} is not bound to a reviewed SHA/version pair.`,
      );
    }
  }
  return external.length;
}

export function validateWorkflowSafety(path, content, workflow) {
  assertTopLevelReadOnlyPermissions(path, workflow);
  assertJobPermissions(path, workflow);
  assertCheckoutPerJob(path, workflow);
  if (
    !workflow.concurrency?.group ||
    workflow.concurrency["cancel-in-progress"] !== true
  ) {
    fail(`${path} must cancel superseded concurrent runs.`);
  }
  for (const [jobId, job] of Object.entries(workflow.jobs)) {
    if (
      !Number.isInteger(job["timeout-minutes"]) ||
      job["timeout-minutes"] <= 0
    ) {
      fail(`${path} job ${jobId} must have a positive timeout-minutes.`);
    }
    if (job["continue-on-error"] !== undefined) {
      fail(`${path} job ${jobId} must not set continue-on-error.`);
    }
    for (const step of job.steps ?? []) {
      if (step?.if !== undefined || step?.["continue-on-error"] !== undefined) {
        fail(
          `${path} job ${jobId} steps must not be conditional or continue on error.`,
        );
      }
    }
  }
  for (const [label, pattern] of FORBIDDEN_WORKFLOW_TEXT) {
    if (pattern.test(content))
      fail(`${path} contains forbidden ${label} behavior.`);
  }
  return validateActionPins(path, content);
}

function assertPinnedEnvironment(project) {
  const nodeVersion = project.nvmrc.trim();
  const packageManager = project.packageJson.packageManager;
  const rustChannel = /channel\s*=\s*"([^"]+)"/.exec(
    project.rustToolchain,
  )?.[1];
  if (nodeVersion !== "22.22.3" || packageManager !== "pnpm@10.33.2") {
    fail("Node and pnpm must remain exactly pinned for CI.");
  }
  if (rustChannel !== "1.97.1") fail("Rust must remain exactly pinned for CI.");
  if (
    project.packageJson.scripts?.["tauri:source:build"] !==
    "node scripts/manage-build-artifacts.mjs build source"
  ) {
    fail(
      "Native source CI must use the dedicated unsigned source-build command.",
    );
  }
  if (
    project.packageJson.scripts?.["security:public:check"] !==
      "node scripts/check-repository-security.mjs --public-tree" ||
    project.packageJson.scripts?.["license:public:check"] !==
      "node scripts/check-licenses.mjs --scan-public-tree ."
  ) {
    fail(
      "Public CI must run the dedicated public-tree security and license gates.",
    );
  }
  const releaseScripts = {
    "release:validate": "node scripts/manage-release.mjs validate",
    "release:draft": "node scripts/manage-release.mjs draft",
    "release:build": "node scripts/manage-release.mjs build",
    "release:prepare": "node scripts/manage-release.mjs prepare",
    "release:publish": "node scripts/manage-release.mjs publish",
  };
  for (const [name, command] of Object.entries(releaseScripts)) {
    if (project.packageJson.scripts?.[name] !== command) {
      fail(`Protected release automation must retain the ${name} command.`);
    }
  }
}

function assertExactObject(value, expected, label) {
  const actual = asObject(value, label);
  const actualEntries = Object.entries(actual).sort(([left], [right]) =>
    left.localeCompare(right),
  );
  const expectedEntries = Object.entries(expected).sort(([left], [right]) =>
    left.localeCompare(right),
  );
  if (JSON.stringify(actualEntries) !== JSON.stringify(expectedEntries)) {
    fail(`${label} must retain its exact reviewed values.`);
  }
}

export function validateReleaseWorkflow(content, workflow) {
  const path = RELEASE_WORKFLOW_FILE;
  assertTopLevelReadOnlyPermissions(path, workflow);
  const triggers = asObject(workflow.on, "release workflow triggers");
  assertExactTriggerKeys("Stable release", triggers, ["push"]);
  if (JSON.stringify(triggers.push) !== JSON.stringify({ tags: ["v*.*.*"] })) {
    fail("Stable release must run only for canonical-looking version tags.");
  }
  if (
    workflow.concurrency?.group !== "release-${{ github.ref }}" ||
    workflow.concurrency?.["cancel-in-progress"] !== false
  ) {
    fail("Stable release must serialize one tag without cancelling a run.");
  }
  const jobs = asObject(workflow.jobs, `${path} jobs`);
  if (Object.keys(jobs).length !== 1 || !jobs.release) {
    fail("Stable release must contain exactly one protected release job.");
  }
  const job = jobs.release;
  if (
    job.name !== "Release / Protected macOS" ||
    job["runs-on"] !== "macos-15" ||
    job.environment !== "release" ||
    !Number.isInteger(job["timeout-minutes"]) ||
    job["timeout-minutes"] <= 0 ||
    job.if !== undefined ||
    job.needs !== undefined ||
    job["continue-on-error"] !== undefined
  ) {
    fail(
      "Stable release must retain the reviewed protected macOS job boundary.",
    );
  }
  assertExactObject(
    job.permissions,
    { contents: "write", "id-token": "write", attestations: "write" },
    "Stable release job permissions",
  );
  assertExactObject(
    job.env,
    { GH_TOKEN: "${{ github.token }}" },
    "Stable release job environment",
  );
  assertCheckoutPerJob(path, workflow);
  assertPinnedNodeSetup(path, "release", job);
  validateActionPins(path, content);

  const expectedCommands = [
    "corepack enable",
    "pnpm install --frozen-lockfile",
    "pnpm release:validate",
    "pnpm release:draft",
    "pnpm release:build",
    "pnpm release:prepare",
    "pnpm release:publish",
  ];
  const runSteps = (job.steps ?? []).filter(
    (step) => typeof step?.run === "string",
  );
  if (
    runSteps.length !== expectedCommands.length ||
    runSteps.some((step, index) => step.run.trim() !== expectedCommands[index])
  ) {
    fail(
      "Stable release commands must retain the reviewed immutable draft flow.",
    );
  }
  for (const step of job.steps ?? []) {
    if (step?.if !== undefined || step?.["continue-on-error"] !== undefined) {
      fail(
        "Stable release steps must not be conditional or continue on error.",
      );
    }
  }
  if (/\bgh\s+release\b|create-release|upload-artifact/i.test(content)) {
    fail(
      "Stable release publication must remain inside the reviewed release script.",
    );
  }
  if (
    /developer[_ -]?id|apple_certificate|notar(?:y|ize)|staple/i.test(content)
  ) {
    fail(
      "Stable release must not request Apple signing or notarization credentials.",
    );
  }

  const secretReferences = [
    ...content.matchAll(/\bsecrets\.([A-Z][A-Z0-9_]*)\b/g),
  ].map((match) => match[1]);
  const expectedSecrets = [
    "AI_ROUTER_UPDATER_PUBLIC_KEY",
    "AI_ROUTER_UPDATER_PUBLIC_KEY",
    "TAURI_SIGNING_PRIVATE_KEY",
    "TAURI_SIGNING_PRIVATE_KEY_PASSWORD",
    "AI_ROUTER_UPDATER_PUBLIC_KEY",
  ];
  if (JSON.stringify(secretReferences) !== JSON.stringify(expectedSecrets)) {
    fail(
      "Stable release may reference only the exact updater signing secrets.",
    );
  }
  const stepByCommand = new Map(
    runSteps.map((step) => [step.run.trim(), step]),
  );
  assertExactObject(
    stepByCommand.get("pnpm release:validate").env,
    {
      AI_ROUTER_UPDATER_PUBLIC_KEY:
        "${{ secrets.AI_ROUTER_UPDATER_PUBLIC_KEY }}",
    },
    "Release validation environment",
  );
  assertExactObject(
    stepByCommand.get("pnpm release:build").env,
    {
      AI_ROUTER_UPDATER_PUBLIC_KEY:
        "${{ secrets.AI_ROUTER_UPDATER_PUBLIC_KEY }}",
      TAURI_SIGNING_PRIVATE_KEY: "${{ secrets.TAURI_SIGNING_PRIVATE_KEY }}",
      TAURI_SIGNING_PRIVATE_KEY_PASSWORD:
        "${{ secrets.TAURI_SIGNING_PRIVATE_KEY_PASSWORD }}",
    },
    "Release build environment",
  );
  assertExactObject(
    stepByCommand.get("pnpm release:prepare").env,
    {
      AI_ROUTER_UPDATER_PUBLIC_KEY:
        "${{ secrets.AI_ROUTER_UPDATER_PUBLIC_KEY }}",
    },
    "Release preparation environment",
  );
  for (const command of [
    "corepack enable",
    "pnpm install --frozen-lockfile",
    "pnpm release:draft",
    "pnpm release:publish",
  ]) {
    if (stepByCommand.get(command).env !== undefined) {
      fail(
        `Stable release command ${command} must not receive signing secrets.`,
      );
    }
  }
  const attestation = requiredActionStep(
    path,
    "release",
    job,
    "actions/attest-build-provenance",
  );
  assertExactObject(
    attestation.with,
    { "subject-path": "target/release-distribution/*" },
    "Release attestation inputs",
  );
}

export function validateReleaseScript(content) {
  if (typeof content !== "string" || content.includes("--clobber")) {
    fail(
      `${RELEASE_SCRIPT_FILE} must not permit overwriting existing release assets.`,
    );
  }
}

export function validateCiWorkflow(workflow) {
  const triggers = asObject(workflow.on, "CI workflow triggers");
  assertExactTriggerKeys("Required CI", triggers, ["pull_request", "push"]);
  if (
    triggers.pull_request !== null ||
    JSON.stringify(triggers.push) !== JSON.stringify({ branches: ["main"] })
  ) {
    fail(
      "Required CI must run without filters for pull requests and main pushes.",
    );
  }
  for (const [jobId, stableName] of REQUIRED_CHECKS) {
    const job = workflow.jobs?.[jobId];
    if (job?.name !== stableName) {
      fail(
        `CI required check ${jobId} must retain the stable name ${stableName}.`,
      );
    }
    assertUnconditionalJob(".github/workflows/ci.yml", jobId, job);
    assertRunCommands(
      ".github/workflows/ci.yml",
      jobId,
      job,
      REQUIRED_JOB_COMMANDS.get(jobId),
    );
  }
  if (workflow.jobs?.["rust-quality"]?.["runs-on"] !== "macos-15") {
    fail(
      "Rust workspace quality must run on the supported arm64 macOS runner.",
    );
  }
  if (
    workflow.jobs?.["protocol-compatibility"]?.["runs-on"] !== "macos-15-intel"
  ) {
    fail("Protocol compatibility must use the reviewed Intel Codex binary.");
  }
  for (const jobId of [
    "node-quality",
    "generated-contracts",
    "repository-policy",
  ]) {
    if (workflow.jobs?.[jobId]?.["runs-on"] !== "ubuntu-24.04") {
      fail(`CI job ${jobId} must use the pinned Ubuntu 24.04 runner.`);
    }
  }
  for (const jobId of [
    "node-quality",
    "generated-contracts",
    "protocol-compatibility",
    "repository-policy",
  ]) {
    assertPinnedNodeSetup(
      ".github/workflows/ci.yml",
      jobId,
      workflow.jobs[jobId],
    );
  }
}

function assertNativeWorkflow(workflow, content) {
  const triggers = asObject(workflow.on, "native workflow triggers");
  assertExactTriggerKeys("Native source build", triggers, [
    "push",
    "workflow_dispatch",
  ]);
  if (
    Object.hasOwn(triggers, "pull_request") ||
    !Object.hasOwn(triggers, "workflow_dispatch") ||
    !triggers.push?.branches?.includes("main")
  ) {
    fail("Native source build must be manual and run for main revisions.");
  }
  const ignoredDocumentationPaths = new Set(
    triggers.push?.["paths-ignore"] ?? [],
  );
  if (
    ![
      "**/*.md",
      "docs/**",
      ".github/ISSUE_TEMPLATE/**",
      "LICENSE",
      "NOTICE",
    ].every((path) => ignoredDocumentationPaths.has(path))
  ) {
    fail("Native source build must skip documentation-only pushes.");
  }
  if (
    workflow.jobs?.["native-source-build"]?.name !== "Native / Source build" ||
    workflow.jobs?.["native-source-build"]?.["runs-on"] !== "macos-15" ||
    !content.includes("git rev-parse HEAD")
  ) {
    fail(
      "Native source build must record inputs and build the source-only bundle.",
    );
  }
  const job = workflow.jobs["native-source-build"];
  assertUnconditionalJob(
    ".github/workflows/native-source-build.yml",
    "native-source-build",
    job,
  );
  assertPinnedNodeSetup(
    ".github/workflows/native-source-build.yml",
    "native-source-build",
    job,
  );
  assertRunCommands(
    ".github/workflows/native-source-build.yml",
    "native-source-build",
    job,
    [
      "corepack enable",
      "pnpm install --frozen-lockfile",
      "pnpm tauri:source:build",
    ],
  );
}

export function validateSecurityWorkflow(workflow) {
  const triggers = asObject(workflow.on, "security workflow triggers");
  assertExactTriggerKeys("Security analysis", triggers, [
    "pull_request",
    "push",
    "schedule",
  ]);
  if (
    triggers.pull_request !== null ||
    JSON.stringify(triggers.push) !== JSON.stringify({ branches: ["main"] }) ||
    !Array.isArray(triggers.schedule) ||
    triggers.schedule.length === 0
  ) {
    fail(
      "Security analysis must run for pull requests, main pushes, and a schedule.",
    );
  }
  for (const [jobId, stableName] of SECURITY_CHECKS) {
    if (workflow.jobs?.[jobId]?.name !== stableName) {
      fail(
        `Security check ${jobId} must retain the stable name ${stableName}.`,
      );
    }
  }
  const codeql = workflow.jobs?.codeql;
  if (
    codeql.permissions?.actions !== "read" ||
    codeql.permissions?.contents !== "read" ||
    codeql.permissions?.packages !== "read" ||
    codeql.permissions?.["security-events"] !== "write"
  ) {
    fail("CodeQL must receive the exact private-repository permission set.");
  }
  if (
    workflow.jobs?.["dependency-review"]?.if !==
    "github.event_name == 'pull_request'"
  ) {
    fail("Dependency review must run only for pull requests.");
  }
  for (const jobId of SECURITY_CHECKS.keys()) {
    if (workflow.jobs?.[jobId]?.["runs-on"] !== "ubuntu-24.04") {
      fail(`Security job ${jobId} must use the pinned Ubuntu 24.04 runner.`);
    }
  }
  const dependencyReview = workflow.jobs["dependency-review"];
  if (dependencyReview.needs !== undefined) {
    fail("Dependency review must not depend on another job.");
  }
  const dependencyAction = requiredActionStep(
    ".github/workflows/security.yml",
    "dependency-review",
    dependencyReview,
    "actions/dependency-review-action",
  );
  if (
    dependencyAction.with?.["comment-summary-in-pr"] !== "never" ||
    dependencyAction.with?.["fail-on-severity"] !== "moderate"
  ) {
    fail(
      "Dependency review must fail at moderate severity without PR comments.",
    );
  }
  assertUnconditionalJob(".github/workflows/security.yml", "codeql", codeql);
  const codeqlInit = requiredActionStep(
    ".github/workflows/security.yml",
    "codeql",
    codeql,
    "github/codeql-action/init",
  );
  requiredActionStep(
    ".github/workflows/security.yml",
    "codeql",
    codeql,
    "github/codeql-action/analyze",
  );
  if (
    codeqlInit.with?.["build-mode"] !== "none" ||
    codeqlInit.with?.languages !== "javascript-typescript"
  ) {
    fail(
      "CodeQL must use the reviewed JavaScript/TypeScript no-build configuration.",
    );
  }
}

export function validateDependabotConfig(config) {
  if (config?.version !== 2 || !Array.isArray(config.updates)) {
    fail("Dependabot must use version 2 with an updates list.");
  }
  const ecosystems = new Map(
    config.updates.map((update) => [update["package-ecosystem"], update]),
  );
  for (const [ecosystem, scheduleTime] of DEPENDABOT_ECOSYSTEMS) {
    const update = ecosystems.get(ecosystem);
    const allow = update?.allow;
    const groups = update?.groups;
    const group = groups && Object.values(groups)[0];
    const hasExactAllowPolicy =
      Array.isArray(allow) &&
      allow.length === 1 &&
      allow[0]?.["dependency-name"] === "*" &&
      Array.isArray(allow[0]?.["update-types"]) &&
      allow[0]["update-types"].length ===
        DEPENDABOT_ALLOWED_UPDATE_TYPES.length &&
      DEPENDABOT_ALLOWED_UPDATE_TYPES.every((updateType) =>
        allow[0]["update-types"].includes(updateType),
      );
    const hasSecurityAffectingIgnoreRules =
      Array.isArray(update?.ignore) && update.ignore.length > 0;
    if (
      update?.directory !== "/" ||
      update.schedule?.interval !== "weekly" ||
      update.schedule?.day !== "monday" ||
      update.schedule?.time !== scheduleTime ||
      update.schedule?.timezone !== "Asia/Shanghai" ||
      update["open-pull-requests-limit"] !== 2 ||
      !hasExactAllowPolicy ||
      hasSecurityAffectingIgnoreRules ||
      !groups ||
      Object.keys(groups).length !== 1 ||
      !Array.isArray(group?.patterns) ||
      group.patterns.length !== 1 ||
      group.patterns[0] !== "*" ||
      group?.["applies-to"] !== "version-updates" ||
      !Array.isArray(group?.["update-types"]) ||
      group["update-types"].length !== DEPENDABOT_GROUP_UPDATE_TYPES.length ||
      !DEPENDABOT_GROUP_UPDATE_TYPES.every((updateType) =>
        group["update-types"].includes(updateType),
      )
    ) {
      fail(
        `Dependabot ${ecosystem} must keep the reviewed weekly schedule, allow only ordinary minor/patch updates, cap ordinary PRs at 2, group those updates, and avoid ignore rules that affect security updates.`,
      );
    }
  }
  if (ecosystems.size !== 3 || config.updates.length !== 3)
    fail("Dependabot must define exactly three ecosystems.");
  return ecosystems.size;
}

async function readProject(root) {
  const [nvmrc, packageJson, rustToolchain, releaseScript, ...workflows] =
    await Promise.all([
      readFile(join(root, ".nvmrc"), "utf8"),
      readFile(join(root, "package.json"), "utf8").then(JSON.parse),
      readFile(join(root, "rust-toolchain.toml"), "utf8"),
      readFile(join(root, RELEASE_SCRIPT_FILE), "utf8"),
      ...WORKFLOW_FILES.map((path) => readFile(join(root, path), "utf8")),
      readFile(join(root, RELEASE_WORKFLOW_FILE), "utf8"),
    ]);
  return { nvmrc, packageJson, releaseScript, rustToolchain, workflows };
}

export async function checkCiPolicy(projectRoot = DEFAULT_PROJECT_ROOT) {
  const root = resolve(projectRoot);
  const project = await readProject(root);
  assertPinnedEnvironment(project);
  validateReleaseScript(project.releaseScript);
  let actionPins = 0;
  const parsedWorkflows = new Map();
  for (const [index, path] of WORKFLOW_FILES.entries()) {
    const content = project.workflows[index];
    const workflow = asObject(parseYaml(path, content), path);
    parsedWorkflows.set(path, { content, workflow });
    actionPins += validateWorkflowSafety(path, content, workflow);
  }
  const releaseContent = project.workflows[WORKFLOW_FILES.length];
  const releaseWorkflow = asObject(
    parseYaml(RELEASE_WORKFLOW_FILE, releaseContent),
    RELEASE_WORKFLOW_FILE,
  );
  validateReleaseWorkflow(releaseContent, releaseWorkflow);
  actionPins += validateActionPins(RELEASE_WORKFLOW_FILE, releaseContent);
  validateCiWorkflow(parsedWorkflows.get(".github/workflows/ci.yml").workflow);
  assertNativeWorkflow(
    parsedWorkflows.get(".github/workflows/native-source-build.yml").workflow,
    parsedWorkflows.get(".github/workflows/native-source-build.yml").content,
  );
  validateSecurityWorkflow(
    parsedWorkflows.get(".github/workflows/security.yml").workflow,
  );
  const dependabotContent = await readFile(
    join(root, ".github/dependabot.yml"),
    "utf8",
  );
  const dependabotEcosystems = validateDependabotConfig(
    parseYaml(".github/dependabot.yml", dependabotContent),
  );
  return {
    actionPins,
    dependabotEcosystems,
    requiredChecks: [...REQUIRED_CHECKS.values(), ...SECURITY_CHECKS.values()],
    workflows: WORKFLOW_FILES.length + 1,
  };
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  checkCiPolicy()
    .then((result) => {
      console.log(
        `CI policy passed: ${result.workflows} workflows, ${result.actionPins} action pins, ${result.dependabotEcosystems} Dependabot ecosystems.`,
      );
    })
    .catch((error) => {
      console.error(error instanceof Error ? error.message : String(error));
      process.exitCode = 1;
    });
}
