import { readFile, rename, writeFile } from "node:fs/promises";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const SEMVER = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-(?:0|[1-9]\d*|[0-9A-Za-z-]*[A-Za-z-][0-9A-Za-z-]*)(?:\.(?:0|[1-9]\d*|[0-9A-Za-z-]*[A-Za-z-][0-9A-Za-z-]*))*)?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$/;

export class VersionError extends Error {}

function managedPaths(root) {
  return {
    cargo: resolve(root, "Cargo.toml"),
    packageJson: resolve(root, "package.json"),
    tauriConfig: resolve(root, "src-tauri/tauri.conf.json"),
  };
}

export function validateVersion(value, file) {
  if (typeof value !== "string" || !SEMVER.test(value)) {
    throw new VersionError(`${file} must declare a valid SemVer version.`);
  }
  return value;
}

function workspaceVersion(cargo) {
  const sectionStart = cargo.indexOf("[workspace.package]");
  if (sectionStart === -1) {
    throw new VersionError("Cargo.toml is missing the [workspace.package] section.");
  }
  const followingSection = cargo.slice(sectionStart + 1).search(/\n\[/);
  const sectionEnd = followingSection === -1 ? cargo.length : sectionStart + 1 + followingSection;
  const section = cargo.slice(sectionStart, sectionEnd);
  const versionPattern = /^version\s*=\s*"([^"]+)"\s*$/gm;
  const versions = [...section.matchAll(versionPattern)];
  if (versions.length !== 1) {
    throw new VersionError("Cargo.toml must have exactly one workspace package version.");
  }
  return {
    version: versions[0][1],
    replace(nextVersion) {
      return `${cargo.slice(0, sectionStart)}${section.replace(
        versionPattern,
        `version = "${nextVersion}"`,
      )}${cargo.slice(sectionEnd)}`;
    },
  };
}

async function readManagedFiles(root) {
  const paths = managedPaths(root);
  const [cargo, packageJson, tauriConfig] = await Promise.all([
    readFile(paths.cargo, "utf8"),
    readFile(paths.packageJson, "utf8"),
    readFile(paths.tauriConfig, "utf8"),
  ]);
  let packageManifest;
  let tauriManifest;
  try {
    packageManifest = JSON.parse(packageJson);
    tauriManifest = JSON.parse(tauriConfig);
  } catch {
    throw new VersionError("package.json and tauri.conf.json must contain valid JSON.");
  }
  return {
    cargo,
    packageJson,
    packageManifest,
    paths,
    tauriManifest,
    workspace: workspaceVersion(cargo),
  };
}

async function writeAtomically(path, content) {
  const temporaryPath = `${path}.${process.pid}.tmp`;
  await writeFile(temporaryPath, content, "utf8");
  await rename(temporaryPath, path);
}

export async function synchronizeVersions(root = process.cwd()) {
  const files = await readManagedFiles(root);
  const version = validateVersion(files.tauriManifest.version, "src-tauri/tauri.conf.json");
  files.packageManifest.version = version;
  const packageJson = `${JSON.stringify(files.packageManifest, null, 2)}\n`;
  const cargo = files.workspace.replace(version);
  await Promise.all([
    packageJson === files.packageJson
      ? undefined
      : writeAtomically(files.paths.packageJson, packageJson),
    cargo === files.cargo ? undefined : writeAtomically(files.paths.cargo, cargo),
  ]);
  return version;
}

export async function checkVersions(root = process.cwd()) {
  const files = await readManagedFiles(root);
  const sourceVersion = validateVersion(files.tauriManifest.version, "src-tauri/tauri.conf.json");
  const packageVersion = validateVersion(files.packageManifest.version, "package.json");
  const cargoVersion = validateVersion(files.workspace.version, "Cargo.toml");
  const mismatches = [
    ["package.json", packageVersion],
    ["Cargo.toml", cargoVersion],
  ].filter(([, version]) => version !== sourceVersion);
  if (mismatches.length > 0) {
    const details = mismatches.map(([file, version]) => `${file}=${version}`).join(", ");
    throw new VersionError(
      `Version drift detected: src-tauri/tauri.conf.json=${sourceVersion}; ${details}. Run pnpm version:sync.`,
    );
  }
  return sourceVersion;
}

async function run() {
  const command = process.argv[2];
  if (command !== "sync" && command !== "check") {
    throw new VersionError("Usage: node scripts/manage-version.mjs <sync|check>");
  }
  const version = command === "sync" ? await synchronizeVersions() : await checkVersions();
  console.log(`Version ${command} passed: ${version}`);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  run().catch((error) => {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = 1;
  });
}
