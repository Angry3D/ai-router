import { execFile } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdir, readFile, readdir, writeFile } from "node:fs/promises";
import { dirname, extname, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

const SCRIPT_ROOT = dirname(fileURLToPath(import.meta.url));
const DEFAULT_PROJECT_ROOT = resolve(SCRIPT_ROOT, "..");
const DEFAULT_POLICY_PATH = join(SCRIPT_ROOT, "license-policy.json");
const IMAGE_EXTENSIONS = new Set([
  ".gif",
  ".icns",
  ".ico",
  ".jpeg",
  ".jpg",
  ".png",
  ".svg",
  ".webp",
]);
const TEXT_EXTENSIONS = new Set([
  "",
  ".css",
  ".html",
  ".js",
  ".json",
  ".jsx",
  ".md",
  ".mjs",
  ".rs",
  ".sh",
  ".toml",
  ".ts",
  ".tsx",
  ".txt",
  ".yaml",
  ".yml",
]);
const SKIPPED_DIRECTORIES = new Set([
  ".git",
  "coverage",
  "dist",
  "node_modules",
  "target",
]);
const REVIEWED_PUBLIC_TREE_POLICY = {
  forbiddenExactPaths: [
    "design-qa.md",
    "scripts/public-snapshot-policy.json",
    "scripts/public-snapshot.mjs",
    "scripts/public-snapshot.test.mjs",
  ],
  forbiddenPathPrefixes: [".agents/", ".claude/", ".codex/", ".trellis/"],
  forbiddenTextMarkers: [
    "<!-- TRELLIS:START -->",
    "Managed by Trellis.",
    "@mindfoldhq/trellis",
    "AGPL-3.0-only",
    "CC Switch",
  ],
  markerExemptPaths: [
    "scripts/check-ci-policy.mjs",
    "scripts/check-ci-policy.test.mjs",
    "scripts/check-public-docs.mjs",
    "scripts/check-licenses.mjs",
    "scripts/check-licenses.test.mjs",
    "scripts/check-repository-security.mjs",
    "scripts/check-repository-security.test.mjs",
    "scripts/license-policy.json",
  ],
};
const SPDX_OPERATORS = new Set(["AND", "OR", "WITH"]);

export class LicenseAuditError extends Error {}

function stableJson(value) {
  return `${JSON.stringify(value, null, 2)}\n`;
}

function sha256(content) {
  return createHash("sha256").update(content).digest("hex");
}

async function readJson(path, label = path) {
  let value;
  try {
    value = JSON.parse(await readFile(path, "utf8"));
  } catch (error) {
    throw new LicenseAuditError(`${label} is not valid JSON: ${error.message}`);
  }
  return value;
}

function execute(command, args, { cwd } = {}) {
  return new Promise((resolvePromise, rejectPromise) => {
    execFile(
      command,
      args,
      { cwd, encoding: "utf8", maxBuffer: 128 * 1024 * 1024 },
      (error, stdout, stderr) => {
        if (error) {
          const detail = String(stderr || error.message)
            .trim()
            .split("\n")
            .slice(-8)
            .join("\n");
          rejectPromise(
            new LicenseAuditError(
              `${command} ${args.join(" ")} failed${detail ? `:\n${detail}` : "."}`,
            ),
          );
          return;
        }
        resolvePromise(stdout.trim());
      },
    );
  });
}

function assertRepositoryPath(path, label) {
  if (
    typeof path !== "string" ||
    !path ||
    path.includes("\0") ||
    path.includes("\\") ||
    path.startsWith("/") ||
    path
      .split("/")
      .some(
        (component) => !component || component === "." || component === "..",
      )
  ) {
    throw new LicenseAuditError(`${label} contains an unsafe repository path.`);
  }
}

function assertReviewedStringSet(actual, expected, label) {
  if (
    !Array.isArray(actual) ||
    actual.length !== expected.length ||
    new Set(actual).size !== actual.length ||
    expected.some((value) => !actual.includes(value))
  ) {
    throw new LicenseAuditError(
      `License policy ${label} drifted from the reviewed public-tree boundary.`,
    );
  }
}

export function validateLicensePolicy(policy) {
  if (
    policy?.schemaVersion !== 1 ||
    typeof policy.policyVersion !== "string" ||
    typeof policy.project !== "object" ||
    typeof policy.tools !== "object" ||
    typeof policy.dependencies !== "object" ||
    !Array.isArray(policy.thirdParty) ||
    !Array.isArray(policy.pricingCatalogs) ||
    !Array.isArray(policy.declaredVisualAssets) ||
    typeof policy.declaredVisualAssetHashes !== "object" ||
    policy.declaredVisualAssetHashes === null ||
    Array.isArray(policy.declaredVisualAssetHashes) ||
    typeof policy.publicTree !== "object"
  ) {
    throw new LicenseAuditError(
      "License policy must use schemaVersion 1 and contain every policy section.",
    );
  }

  for (const key of [
    "javascriptAllowedIdentifiers",
    "rustAllowedIdentifiers",
  ]) {
    const identifiers = policy.dependencies[key];
    if (
      !Array.isArray(identifiers) ||
      identifiers.length === 0 ||
      new Set(identifiers).size !== identifiers.length ||
      identifiers.some(
        (identifier) => typeof identifier !== "string" || !identifier,
      )
    ) {
      throw new LicenseAuditError(
        `${key} must contain unique, non-empty SPDX identifiers.`,
      );
    }
  }
  if (!Array.isArray(policy.dependencies.rustLicenseOverrides)) {
    throw new LicenseAuditError("rustLicenseOverrides must be an array.");
  }
  const overrideIds = new Set();
  for (const override of policy.dependencies.rustLicenseOverrides) {
    const id = `${override?.name}@${override?.version}`;
    if (
      typeof override?.name !== "string" ||
      typeof override.version !== "string" ||
      typeof override.source !== "string" ||
      typeof override.license !== "string" ||
      typeof override.disposition !== "string" ||
      !Array.isArray(override.licenseFiles) ||
      override.licenseFiles.length === 0 ||
      overrideIds.has(id)
    ) {
      throw new LicenseAuditError(
        "Rust license overrides must be unique and fully documented.",
      );
    }
    for (const licenseFile of override.licenseFiles) {
      if (
        typeof licenseFile.path !== "string" ||
        licenseFile.path.includes("/") ||
        !/^[a-f0-9]{64}$/.test(licenseFile.sha256)
      ) {
        throw new LicenseAuditError(
          `${id} has an invalid license-file binding.`,
        );
      }
    }
    overrideIds.add(id);
  }

  const ids = new Set();
  const declaredPaths = new Set();
  for (const record of policy.thirdParty) {
    if (
      typeof record.id !== "string" ||
      ids.has(record.id) ||
      typeof record.name !== "string" ||
      typeof record.version !== "string" ||
      typeof record.license !== "string" ||
      typeof record.upstream !== "string" ||
      typeof record.noticeMarker !== "string" ||
      !Array.isArray(record.localPaths) ||
      record.localPaths.length === 0 ||
      typeof record.sourceRecord?.path !== "string" ||
      !Array.isArray(record.sourceRecord.requiredStrings) ||
      typeof record.hashes !== "object"
    ) {
      throw new LicenseAuditError(
        "Every third-party record must be complete and use a unique id.",
      );
    }
    ids.add(record.id);
    for (const path of record.localPaths) {
      assertRepositoryPath(path, record.id);
      declaredPaths.add(path);
    }
    assertRepositoryPath(record.sourceRecord.path, record.id);
    for (const [path, digest] of Object.entries(record.hashes)) {
      assertRepositoryPath(path, record.id);
      if (!/^[a-f0-9]{64}$/.test(digest)) {
        throw new LicenseAuditError(
          `${record.id} has an invalid SHA-256 for ${path}.`,
        );
      }
    }
  }

  for (const asset of policy.declaredVisualAssets) {
    assertRepositoryPath(asset, "declaredVisualAssets");
    if (!declaredPaths.has(asset)) {
      throw new LicenseAuditError(
        `Declared visual asset lacks a third-party provenance record: ${asset}.`,
      );
    }
  }
  const declaredAssetHashes = Object.entries(policy.declaredVisualAssetHashes);
  if (
    declaredAssetHashes.length !== policy.declaredVisualAssets.length ||
    declaredAssetHashes.some(
      ([path, digest]) =>
        !policy.declaredVisualAssets.includes(path) ||
        !/^[a-f0-9]{64}$/.test(digest),
    )
  ) {
    throw new LicenseAuditError(
      "Every declared visual asset must have exactly one reviewed SHA-256 binding.",
    );
  }
  for (const [key, expected] of Object.entries(REVIEWED_PUBLIC_TREE_POLICY)) {
    assertReviewedStringSet(policy.publicTree[key], expected, key);
  }
  return policy;
}

export async function loadLicensePolicy(path = DEFAULT_POLICY_PATH) {
  return validateLicensePolicy(await readJson(path, "License policy"));
}

function section(text, name, label) {
  const header = `[${name}]`;
  const start = text.indexOf(`${header}\n`);
  if (start === -1) {
    throw new LicenseAuditError(`${label} is missing ${header}.`);
  }
  const contentStart = start + header.length + 1;
  const next = text.indexOf("\n[", contentStart);
  return text.slice(contentStart, next === -1 ? text.length : next);
}

function tomlString(sectionText, key) {
  const escaped = key.replaceAll(".", "\\.");
  return new RegExp(`^${escaped}\\s*=\\s*"([^"]+)"\\s*$`, "m").exec(
    sectionText,
  )?.[1];
}

function hasInheritedField(sectionText, key) {
  const escaped = key.replaceAll(".", "\\.");
  return new RegExp(`^${escaped}\\.workspace\\s*=\\s*true\\s*$`, "m").test(
    sectionText,
  );
}

async function requireText(path, message) {
  try {
    return await readFile(path, "utf8");
  } catch {
    throw new LicenseAuditError(message);
  }
}

export async function checkProjectMetadata(projectRoot, policy) {
  const [license, readme, notice, packageJson, cargoToml, tauriConfig] =
    await Promise.all([
      requireText(join(projectRoot, "LICENSE"), "Root LICENSE is missing."),
      requireText(join(projectRoot, "README.md"), "Root README.md is missing."),
      requireText(
        join(projectRoot, "THIRD_PARTY_NOTICES.md"),
        "Root THIRD_PARTY_NOTICES.md is missing.",
      ),
      readJson(join(projectRoot, "package.json"), "package.json"),
      requireText(join(projectRoot, "Cargo.toml"), "Cargo.toml is missing."),
      readJson(
        join(projectRoot, "src-tauri/tauri.conf.json"),
        "Tauri configuration",
      ),
    ]);
  const expected = policy.project;

  if (
    !license.startsWith("MIT License\n") ||
    !license.includes(expected.copyright)
  ) {
    throw new LicenseAuditError(
      "Root LICENSE is not the approved MIT text and copyright line.",
    );
  }
  if (
    !readme.includes("./LICENSE") ||
    !readme.includes("./THIRD_PARTY_NOTICES.md")
  ) {
    throw new LicenseAuditError(
      "README must link the project license and third-party notices.",
    );
  }
  if (
    packageJson.name !== expected.name ||
    packageJson.version !== expected.version
  ) {
    throw new LicenseAuditError(
      "package.json project name/version does not match license policy.",
    );
  }
  if (
    packageJson.author !== expected.author ||
    packageJson.license !== expected.license ||
    packageJson.repository?.url !== expected.repository ||
    packageJson.homepage !== expected.repository
  ) {
    throw new LicenseAuditError(
      "package.json license/author/repository metadata is inconsistent.",
    );
  }
  if (packageJson.private !== true) {
    throw new LicenseAuditError(
      "The application package must remain private to prevent npm publication.",
    );
  }
  if (tauriConfig.version !== expected.version) {
    throw new LicenseAuditError("Tauri version does not match license policy.");
  }

  const workspacePackage = section(
    cargoToml,
    "workspace.package",
    "Cargo.toml",
  );
  const expectedCargoFields = {
    description: packageJson.description,
    homepage: expected.repository,
    license: expected.license,
    repository: expected.repository,
    version: expected.version,
  };
  for (const [key, value] of Object.entries(expectedCargoFields)) {
    if (tomlString(workspacePackage, key) !== value) {
      throw new LicenseAuditError(
        `Cargo workspace ${key} metadata is inconsistent.`,
      );
    }
  }
  if (!workspacePackage.includes(`authors = ["${expected.author}"]`)) {
    throw new LicenseAuditError(
      "Cargo workspace authors metadata is inconsistent.",
    );
  }

  for (const manifestPath of [
    "crates/router-core/Cargo.toml",
    "src-tauri/Cargo.toml",
  ]) {
    const manifest = await requireText(
      join(projectRoot, manifestPath),
      `${manifestPath} is missing.`,
    );
    const packageSection = section(manifest, "package", manifestPath);
    for (const key of [
      "authors",
      "description",
      "homepage",
      "license",
      "repository",
      "version",
    ]) {
      if (!hasInheritedField(packageSection, key)) {
        throw new LicenseAuditError(
          `${manifestPath} must inherit workspace ${key} metadata.`,
        );
      }
    }
  }

  return {
    license: expected.license,
    project: expected.name,
    version: expected.version,
    notice,
  };
}

export async function checkThirdPartyProvenance(projectRoot, policy, notice) {
  const summaries = [];
  for (const record of policy.thirdParty) {
    if (!notice.includes(record.noticeMarker)) {
      throw new LicenseAuditError(
        `THIRD_PARTY_NOTICES.md is missing ${record.id}.`,
      );
    }
    for (const path of record.localPaths) {
      try {
        await readFile(join(projectRoot, path));
      } catch {
        throw new LicenseAuditError(
          `${record.id} references missing file ${path}.`,
        );
      }
    }
    const sourceRecord = await requireText(
      join(projectRoot, record.sourceRecord.path),
      `${record.id} source record is missing.`,
    );
    for (const value of record.sourceRecord.requiredStrings) {
      if (!sourceRecord.includes(value)) {
        throw new LicenseAuditError(
          `${record.id} source record is missing required value: ${value}.`,
        );
      }
    }
    for (const [path, expectedDigest] of Object.entries(record.hashes)) {
      const actualDigest = sha256(await readFile(join(projectRoot, path)));
      if (actualDigest !== expectedDigest) {
        throw new LicenseAuditError(
          `${record.id} provenance hash changed for ${path}.`,
        );
      }
    }
    summaries.push({
      id: record.id,
      license: record.license,
      localFileCount: record.localPaths.length,
      upstream: record.upstream,
      version: record.version,
    });
  }

  for (const expected of policy.pricingCatalogs) {
    const catalog = await readJson(
      join(projectRoot, expected.path),
      expected.path,
    );
    if (
      catalog.version !== expected.version ||
      catalog.captured_at !== expected.capturedAt ||
      catalog.unit !== expected.unit ||
      !Array.isArray(catalog.sources) ||
      expected.sources.some((source) => !catalog.sources.includes(source))
    ) {
      throw new LicenseAuditError(
        `Pricing provenance is incomplete for ${expected.path}.`,
      );
    }
  }
  return summaries;
}

export function licenseIdentifiers(expression) {
  if (
    typeof expression !== "string" ||
    !expression.trim() ||
    /unknown|noassertion|licenseref/i.test(expression) ||
    !/^[A-Za-z0-9().+\-/\s]+$/.test(expression)
  ) {
    throw new LicenseAuditError(
      `Missing, unknown, or malformed license expression: ${expression}.`,
    );
  }
  const identifiers = (
    expression.match(/[A-Za-z0-9][A-Za-z0-9.+-]*/g) ?? []
  ).filter((token) => !SPDX_OPERATORS.has(token));
  if (identifiers.length === 0) {
    throw new LicenseAuditError(
      `License expression has no identifiers: ${expression}.`,
    );
  }
  return identifiers;
}

function assertAllowedExpression(expression, allowedIdentifiers, packageId) {
  const normalized = expression.replaceAll("/", " OR ");
  const tokens =
    normalized.match(/\(|\)|AND|OR|WITH|[A-Za-z0-9][A-Za-z0-9.+-]*/g) ?? [];
  const identifiers = licenseIdentifiers(expression);
  let index = 0;

  function primary() {
    const token = tokens[index];
    if (token === "(") {
      index += 1;
      const value = orExpression();
      if (tokens[index] !== ")") {
        throw new LicenseAuditError(
          `Malformed license expression for ${packageId}: ${expression}.`,
        );
      }
      index += 1;
      return value;
    }
    if (!token || token === ")" || SPDX_OPERATORS.has(token)) {
      throw new LicenseAuditError(
        `Malformed license expression for ${packageId}: ${expression}.`,
      );
    }
    index += 1;
    return allowedIdentifiers.has(token);
  }

  function withExpression() {
    let value = primary();
    while (tokens[index] === "WITH") {
      index += 1;
      value = primary() && value;
    }
    return value;
  }

  function andExpression() {
    let value = withExpression();
    while (tokens[index] === "AND") {
      index += 1;
      const right = withExpression();
      value = value && right;
    }
    return value;
  }

  function orExpression() {
    let value = andExpression();
    while (tokens[index] === "OR") {
      index += 1;
      const right = andExpression();
      value = value || right;
    }
    return value;
  }

  const allowed = orExpression();
  if (index !== tokens.length) {
    throw new LicenseAuditError(
      `Malformed license expression for ${packageId}: ${expression}.`,
    );
  }
  if (!allowed) {
    const disallowed = identifiers.filter(
      (identifier) => !allowedIdentifiers.has(identifier),
    );
    throw new LicenseAuditError(
      `${packageId} uses unreviewed license identifier(s): ${[...new Set(disallowed)].join(", ")}.`,
    );
  }
}

export function evaluatePnpmLicenses(rawReport, allowedIdentifiers) {
  if (!rawReport || Array.isArray(rawReport) || typeof rawReport !== "object") {
    throw new LicenseAuditError(
      "pnpm license report must be an object keyed by license expression.",
    );
  }
  const packages = [];
  const seen = new Set();
  for (const [expression, entries] of Object.entries(rawReport).sort(
    ([left], [right]) => left.localeCompare(right),
  )) {
    if (!Array.isArray(entries) || entries.length === 0) {
      throw new LicenseAuditError(
        `pnpm returned an empty license group for ${expression}.`,
      );
    }
    assertAllowedExpression(
      expression,
      allowedIdentifiers,
      "pnpm dependency group",
    );
    for (const entry of entries) {
      if (
        typeof entry?.name !== "string" ||
        !Array.isArray(entry.versions) ||
        entry.versions.length === 0
      ) {
        throw new LicenseAuditError(
          `pnpm returned incomplete package metadata for ${expression}.`,
        );
      }
      for (const version of entry.versions) {
        if (typeof version !== "string" || !version) {
          throw new LicenseAuditError(
            `pnpm returned an invalid version for ${entry.name}.`,
          );
        }
        const id = `${entry.name}@${version}`;
        if (seen.has(id)) {
          throw new LicenseAuditError(
            `pnpm dependency is reported more than once: ${id}.`,
          );
        }
        seen.add(id);
        packages.push({ license: expression, name: entry.name, version });
      }
    }
  }
  if (packages.length === 0) {
    throw new LicenseAuditError(
      "pnpm license report contains no dependencies.",
    );
  }
  packages.sort((left, right) =>
    `${left.name}@${left.version}`.localeCompare(
      `${right.name}@${right.version}`,
    ),
  );
  return { packageCount: packages.length, packages };
}

function cargoSource(source) {
  if (source === null) return "workspace";
  if (source.startsWith("registry+")) return "registry";
  if (source.startsWith("git+")) return "git";
  return "other";
}

export async function evaluateCargoMetadata(
  metadata,
  allowedIdentifiers,
  expectedRepository,
  licenseOverrides = [],
) {
  if (!Array.isArray(metadata?.packages) || metadata.packages.length === 0) {
    throw new LicenseAuditError("cargo metadata contains no packages.");
  }
  const packages = [];
  const seen = new Set();
  const usedOverrides = new Set();
  for (const entry of metadata.packages) {
    if (typeof entry?.name !== "string" || typeof entry.version !== "string") {
      throw new LicenseAuditError(
        "cargo metadata contains an incomplete package entry.",
      );
    }
    const id = `${entry.name}@${entry.version}`;
    if (seen.has(id)) {
      throw new LicenseAuditError(
        `cargo metadata contains a duplicate package: ${id}.`,
      );
    }
    seen.add(id);
    let license = entry.license;
    let licenseSource = "manifest";
    if (!license) {
      const override = licenseOverrides.find(
        (candidate) =>
          candidate.name === entry.name &&
          candidate.version === entry.version &&
          candidate.source === entry.source,
      );
      if (!override) {
        licenseIdentifiers(license);
      }
      if (typeof entry.manifest_path !== "string") {
        throw new LicenseAuditError(
          `${id} cannot verify its license override without manifest_path.`,
        );
      }
      for (const licenseFile of override.licenseFiles) {
        const content = await readFile(
          join(dirname(entry.manifest_path), licenseFile.path),
        );
        if (sha256(content) !== licenseFile.sha256) {
          throw new LicenseAuditError(
            `${id} license override hash changed for ${licenseFile.path}.`,
          );
        }
      }
      license = override.license;
      licenseSource = "policy-override";
      usedOverrides.add(id);
    }
    assertAllowedExpression(license, allowedIdentifiers, id);
    if (
      entry.source === null &&
      (license !== "MIT" || entry.repository !== expectedRepository)
    ) {
      throw new LicenseAuditError(
        `${id} workspace license/repository metadata is inconsistent.`,
      );
    }
    packages.push({
      license,
      licenseSource,
      name: entry.name,
      source: cargoSource(entry.source),
      version: entry.version,
    });
  }
  for (const override of licenseOverrides) {
    const id = `${override.name}@${override.version}`;
    if (!usedOverrides.has(id)) {
      throw new LicenseAuditError(
        `Rust license override is stale or did not match metadata: ${id}.`,
      );
    }
  }
  packages.sort((left, right) =>
    `${left.name}@${left.version}`.localeCompare(
      `${right.name}@${right.version}`,
    ),
  );
  return { packageCount: packages.length, packages };
}

async function dependencyAudit(projectRoot, policy) {
  const nodeVersion = process.version.replace(/^v/, "");
  if (nodeVersion !== policy.tools.node) {
    throw new LicenseAuditError(
      `Node ${nodeVersion} does not match pinned ${policy.tools.node}; run nvm use before the audit.`,
    );
  }
  const [
    pnpmVersion,
    cargoVersion,
    pnpmOutput,
    cargoOutput,
    pnpmLock,
    cargoLock,
  ] = await Promise.all([
    execute("pnpm", ["--version"], { cwd: projectRoot }),
    execute("cargo", ["--version"], { cwd: projectRoot }),
    execute("pnpm", ["licenses", "list", "--json"], { cwd: projectRoot }),
    execute(
      "cargo",
      [
        "metadata",
        "--locked",
        "--format-version",
        String(policy.tools.cargoMetadataFormat),
      ],
      { cwd: projectRoot },
    ),
    readFile(join(projectRoot, "pnpm-lock.yaml")),
    readFile(join(projectRoot, "Cargo.lock")),
  ]);
  if (pnpmVersion !== policy.tools.pnpm) {
    throw new LicenseAuditError(
      `pnpm ${pnpmVersion} does not match pinned ${policy.tools.pnpm}.`,
    );
  }
  if (!cargoVersion.startsWith(`cargo ${policy.tools.cargo} `)) {
    throw new LicenseAuditError(
      `${cargoVersion} does not match pinned Cargo ${policy.tools.cargo}.`,
    );
  }

  let pnpmReport;
  let cargoMetadata;
  try {
    pnpmReport = JSON.parse(pnpmOutput);
    cargoMetadata = JSON.parse(cargoOutput);
  } catch (error) {
    throw new LicenseAuditError(
      `Dependency tool returned invalid JSON: ${error.message}`,
    );
  }
  return {
    cargo: await evaluateCargoMetadata(
      cargoMetadata,
      new Set(policy.dependencies.rustAllowedIdentifiers),
      policy.project.repository,
      policy.dependencies.rustLicenseOverrides,
    ),
    inputs: {
      cargoLockSha256: sha256(cargoLock),
      pnpmLockSha256: sha256(pnpmLock),
    },
    javascript: evaluatePnpmLicenses(
      pnpmReport,
      new Set(policy.dependencies.javascriptAllowedIdentifiers),
    ),
    tools: { cargo: cargoVersion, node: nodeVersion, pnpm: pnpmVersion },
  };
}

async function collectTreeFiles(root, current = root) {
  const files = [];
  const entries = await readdir(current, { withFileTypes: true });
  for (const entry of entries.sort((left, right) =>
    left.name.localeCompare(right.name),
  )) {
    if (entry.isSymbolicLink()) {
      throw new LicenseAuditError(
        `Public tree contains symlink: ${relative(root, join(current, entry.name))}.`,
      );
    }
    if (
      current === root &&
      entry.isDirectory() &&
      SKIPPED_DIRECTORIES.has(entry.name)
    )
      continue;
    const path = join(current, entry.name);
    if (entry.isDirectory())
      files.push(...(await collectTreeFiles(root, path)));
    else if (entry.isFile()) files.push(path);
  }
  return files;
}

function repositoryPath(root, path) {
  return relative(root, path).split(sep).join("/");
}

export async function scanPublicTree(publicRoot, policy) {
  const root = resolve(publicRoot);
  const files = await collectTreeFiles(root);
  const declaredAssets = new Set(policy.declaredVisualAssets);
  const markerExemptions = new Set(policy.publicTree.markerExemptPaths ?? []);
  for (const absolutePath of files) {
    const path = repositoryPath(root, absolutePath);
    if (
      policy.publicTree.forbiddenExactPaths.includes(path) ||
      policy.publicTree.forbiddenPathPrefixes.some((prefix) =>
        path.startsWith(prefix),
      )
    ) {
      throw new LicenseAuditError(
        `Private workflow file entered the public tree: ${path}.`,
      );
    }
    if (
      IMAGE_EXTENSIONS.has(extname(path).toLowerCase()) &&
      !declaredAssets.has(path)
    ) {
      throw new LicenseAuditError(
        `Public tree contains an undeclared visual asset: ${path}.`,
      );
    }
    if (declaredAssets.has(path)) {
      const actualDigest = sha256(await readFile(absolutePath));
      if (actualDigest !== policy.declaredVisualAssetHashes[path]) {
        throw new LicenseAuditError(
          `Public visual asset hash changed for ${path}.`,
        );
      }
    }
    if (
      markerExemptions.has(path) ||
      !TEXT_EXTENSIONS.has(extname(path).toLowerCase())
    )
      continue;
    const content = await readFile(absolutePath, "utf8");
    for (const marker of policy.publicTree.forbiddenTextMarkers) {
      if (content.includes(marker)) {
        throw new LicenseAuditError(
          `Public tree contains forbidden template marker in ${path}.`,
        );
      }
    }
  }
  return {
    fileCount: files.length,
    visualAssetCount: [...declaredAssets].length,
  };
}

export async function runLicenseAudit({
  policyPath = DEFAULT_POLICY_PATH,
  projectRoot = DEFAULT_PROJECT_ROOT,
  publicTreeRoot,
  skipDependencies = false,
} = {}) {
  const root = resolve(projectRoot);
  const policy = await loadLicensePolicy(policyPath);
  const project = await checkProjectMetadata(root, policy);
  const thirdParty = await checkThirdPartyProvenance(
    root,
    policy,
    project.notice,
  );
  const report = {
    policyVersion: policy.policyVersion,
    project: {
      license: project.license,
      name: project.project,
      version: project.version,
    },
    thirdParty,
  };
  if (!skipDependencies)
    report.dependencies = await dependencyAudit(root, policy);
  if (publicTreeRoot)
    report.publicTree = await scanPublicTree(publicTreeRoot, policy);
  return report;
}

function parseArgs(argv) {
  const options = {};
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    if (argument === "--skip-dependencies") options.skipDependencies = true;
    else if (
      ["--policy", "--report", "--root", "--scan-public-tree"].includes(
        argument,
      )
    ) {
      const value = argv[index + 1];
      if (!value) throw new LicenseAuditError(`${argument} requires a path.`);
      index += 1;
      if (argument === "--policy") options.policyPath = resolve(value);
      if (argument === "--report") options.reportPath = resolve(value);
      if (argument === "--root") options.projectRoot = resolve(value);
      if (argument === "--scan-public-tree")
        options.publicTreeRoot = resolve(value);
    } else {
      throw new LicenseAuditError(`Unknown argument: ${argument}.`);
    }
  }
  return options;
}

async function main() {
  const options = parseArgs(process.argv.slice(2));
  const report = await runLicenseAudit(options);
  if (options.reportPath) {
    await mkdir(dirname(options.reportPath), { recursive: true });
    await writeFile(options.reportPath, stableJson(report), "utf8");
  }
  const dependencySummary = report.dependencies
    ? `${report.dependencies.javascript.packageCount} JavaScript and ${report.dependencies.cargo.packageCount} Rust packages`
    : "dependency execution skipped";
  console.log(
    `License audit passed: ${report.project.license}; ${report.thirdParty.length} provenance records; ${dependencySummary}.`,
  );
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  main().catch((error) => {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = 1;
  });
}
