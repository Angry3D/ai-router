import { readFile, rename, unlink, writeFile } from "node:fs/promises";
import { randomUUID } from "node:crypto";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

import semver from "semver";

const MAX_MANIFEST_VERSION_LENGTH = 256;
const LOCAL_WORKSPACE_PACKAGES = ["ai-router-app", "router-core"];

export class VersionError extends Error {}

function managedPaths(root) {
  return {
    cargo: resolve(root, "Cargo.toml"),
    cargoLock: resolve(root, "Cargo.lock"),
    packageJson: resolve(root, "package.json"),
    tauriConfig: resolve(root, "src-tauri/tauri.conf.json"),
  };
}

export function validateVersion(value, file) {
  if (typeof value !== "string" || value.length > MAX_MANIFEST_VERSION_LENGTH) {
    throw new VersionError(`${file} must declare a valid SemVer version.`);
  }

  const parsed = semver.parse(value, { loose: false });
  if (
    !parsed ||
    parsed.version !== value ||
    parsed.prerelease.length > 0 ||
    parsed.build.length > 0
  ) {
    throw new VersionError(
      `${file} must declare a valid stable SemVer version.`,
    );
  }

  return value;
}

function readJsonString(text, start) {
  if (text[start] !== '"') return undefined;
  let escaped = false;
  for (let index = start + 1; index < text.length; index += 1) {
    const character = text[index];
    if (escaped) {
      escaped = false;
      continue;
    }
    if (character === "\\") {
      escaped = true;
      continue;
    }
    if (character === '"') {
      const end = index + 1;
      return {
        end,
        value: JSON.parse(text.slice(start, end)),
      };
    }
  }
  return undefined;
}

function skipJsonWhitespace(text, start) {
  let index = start;
  while (index < text.length && /\s/.test(text[index])) index += 1;
  return index;
}

function packageJsonVersion(packageJson) {
  let candidate;
  const stack = [];
  for (let index = 0; index < packageJson.length; index += 1) {
    const character = packageJson[index];
    if (character === '"') {
      const token = readJsonString(packageJson, index);
      if (!token)
        throw new VersionError("package.json contains an unterminated string.");
      const afterToken = skipJsonWhitespace(packageJson, token.end);
      if (
        stack.length === 1 &&
        stack[0] === "{" &&
        packageJson[afterToken] === ":" &&
        token.value === "version"
      ) {
        const valueStart = skipJsonWhitespace(packageJson, afterToken + 1);
        const value = readJsonString(packageJson, valueStart);
        if (!value) {
          throw new VersionError(
            "package.json top-level version must be a string.",
          );
        }
        if (candidate) {
          throw new VersionError(
            "package.json must contain exactly one top-level version.",
          );
        }
        candidate = {
          end: value.end,
          start: valueStart,
          value: value.value,
        };
      }
      index = token.end - 1;
      continue;
    }
    if (character === "{" || character === "[") {
      stack.push(character);
    } else if (character === "}" || character === "]") {
      stack.pop();
    }
  }
  if (!candidate) {
    throw new VersionError(
      "package.json must contain exactly one top-level version.",
    );
  }
  return candidate;
}

function replaceSpan(text, span, value) {
  return `${text.slice(0, span.start)}${value}${text.slice(span.end)}`;
}

function packageProjection(packageJson) {
  let manifest;
  try {
    manifest = JSON.parse(packageJson);
  } catch {
    throw new VersionError("package.json must contain valid JSON.");
  }
  const span = packageJsonVersion(packageJson);
  if (manifest.version !== span.value) {
    throw new VersionError(
      "package.json top-level version could not be mapped safely.",
    );
  }
  return {
    manifest,
    version: span.value,
    replace(version) {
      return replaceSpan(packageJson, span, JSON.stringify(version));
    },
  };
}

function sectionRange(text, sectionName, file) {
  const sectionPattern = new RegExp(
    `^\\[${sectionName.replaceAll(".", "\\.")}\\][ \\t]*\\r?$`,
    "gm",
  );
  const sections = [...text.matchAll(sectionPattern)];
  if (sections.length !== 1) {
    throw new VersionError(
      `${file} must contain exactly one [${sectionName}] section.`,
    );
  }
  const start = sections[0].index;
  const nextSectionPattern = /^\[[^\r\n]+\][ \t]*\r?$/gm;
  nextSectionPattern.lastIndex = start + sections[0][0].length;
  const nextSection = nextSectionPattern.exec(text);
  return {
    end: nextSection?.index ?? text.length,
    start,
  };
}

function workspaceVersion(cargo) {
  const range = sectionRange(cargo, "workspace.package", "Cargo.toml");
  const section = cargo.slice(range.start, range.end);
  const versionPattern =
    /^([ \t]*)version([ \t]*)=([ \t]*)"([^"\r\n]*)"([ \t]*)(?:#.*)?\r?$/gm;
  const versions = [...section.matchAll(versionPattern)];
  if (versions.length !== 1) {
    throw new VersionError(
      "Cargo.toml must have exactly one workspace package version.",
    );
  }
  const match = versions[0];
  const quoteStart = range.start + match.index + match[0].indexOf('"') + 1;
  const span = {
    end: quoteStart + match[4].length,
    start: quoteStart,
  };
  return {
    version: match[4],
    replace(nextVersion) {
      return replaceSpan(cargo, span, nextVersion);
    },
  };
}

function packageBlocks(cargoLock) {
  const headerPattern = /^\[\[package\]\][ \t]*\r?$/gm;
  const headers = [...cargoLock.matchAll(headerPattern)];
  return headers.map((header, index) => ({
    end: headers[index + 1]?.index ?? cargoLock.length,
    start: header.index,
  }));
}

function lockField(block, field) {
  const fieldPattern = new RegExp(
    `^${field}([ \\t]*)=([ \\t]*)"([^"\\r\\n]*)"([ \\t]*)(?:#.*)?\\r?$`,
    "gm",
  );
  return [...block.matchAll(fieldPattern)];
}

function cargoLockVersions(cargoLock) {
  const blocks = packageBlocks(cargoLock);
  if (blocks.length === 0) {
    throw new VersionError("Cargo.lock must contain package stanzas.");
  }
  const packages = new Map();
  for (const packageName of LOCAL_WORKSPACE_PACKAGES) {
    const matches = blocks
      .map((range) => ({
        names: lockField(cargoLock.slice(range.start, range.end), "name"),
        range,
        text: cargoLock.slice(range.start, range.end),
      }))
      .filter(({ names }) => names.some((match) => match[3] === packageName));
    if (matches.length !== 1) {
      throw new VersionError(
        `Cargo.lock must contain exactly one local package stanza for ${packageName}.`,
      );
    }
    const { names, range, text } = matches[0];
    if (names.length !== 1) {
      throw new VersionError(
        `Cargo.lock package ${packageName} must contain exactly one name.`,
      );
    }
    if (lockField(text, "source").length > 0) {
      throw new VersionError(
        `Cargo.lock package ${packageName} must be source-less.`,
      );
    }
    const versions = lockField(text, "version");
    if (versions.length !== 1) {
      throw new VersionError(
        `Cargo.lock package ${packageName} must contain exactly one version.`,
      );
    }
    const versionMatch = versions[0];
    const quoteStart =
      range.start + versionMatch.index + versionMatch[0].indexOf('"') + 1;
    packages.set(packageName, {
      span: {
        end: quoteStart + versionMatch[3].length,
        start: quoteStart,
      },
      version: versionMatch[3],
    });
  }
  return {
    packages,
    replace(nextVersion) {
      return [...packages.values()]
        .sort((left, right) => right.span.start - left.span.start)
        .reduce(
          (result, packageInfo) =>
            replaceSpan(result, packageInfo.span, nextVersion),
          cargoLock,
        );
    },
  };
}

function defaultFileSystem() {
  return { readFile, rename, unlink, writeFile };
}

async function readManagedFiles(root, fileSystem) {
  const paths = managedPaths(root);
  const fs = { ...defaultFileSystem(), ...fileSystem };
  const [cargo, cargoLock, packageJson, tauriConfig] = await Promise.all([
    fs.readFile(paths.cargo, "utf8"),
    fs.readFile(paths.cargoLock, "utf8"),
    fs.readFile(paths.packageJson, "utf8"),
    fs.readFile(paths.tauriConfig, "utf8"),
  ]);
  let tauriManifest;
  try {
    tauriManifest = JSON.parse(tauriConfig);
  } catch {
    throw new VersionError(
      "src-tauri/tauri.conf.json must contain valid JSON.",
    );
  }
  return {
    cargo,
    cargoLock,
    lock: cargoLockVersions(cargoLock),
    packageJson,
    package: packageProjection(packageJson),
    paths,
    tauriManifest,
    workspace: workspaceVersion(cargo),
  };
}

async function stageFile(path, content, fileSystem) {
  const temporaryPath = `${path}.${process.pid}.${randomUUID()}.tmp`;
  try {
    await fileSystem.writeFile(temporaryPath, content, {
      encoding: "utf8",
      flag: "wx",
    });
  } catch (error) {
    await fileSystem.unlink(temporaryPath).catch(() => undefined);
    throw error;
  }
  return { path, temporaryPath };
}

async function publishFiles(files, fileSystem) {
  const staged = [];
  try {
    for (const file of files)
      staged.push(await stageFile(file.path, file.content, fileSystem));
    for (const file of staged) {
      await fileSystem.rename(file.temporaryPath, file.path);
      file.published = true;
    }
  } catch {
    await Promise.all(
      staged
        .filter((file) => !file.published)
        .map((file) =>
          fileSystem.unlink(file.temporaryPath).catch(() => undefined),
        ),
    );
    throw new VersionError(
      "Version synchronization could not publish projected files.",
    );
  }
}

function projectedFiles(files, version) {
  const packageJson = files.package.replace(version);
  const cargo = files.workspace.replace(version);
  const cargoLock = files.lock.replace(version);
  const projectedPackage = packageProjection(packageJson);
  const projectedWorkspace = workspaceVersion(cargo);
  const projectedLock = cargoLockVersions(cargoLock);
  if (
    projectedPackage.version !== version ||
    projectedWorkspace.version !== version ||
    [...projectedLock.packages.values()].some(
      (entry) => entry.version !== version,
    )
  ) {
    throw new VersionError(
      "Version projections could not be validated before publication.",
    );
  }
  return { cargo, cargoLock, packageJson };
}

export async function synchronizeVersions(root = process.cwd(), options = {}) {
  const fileSystem = { ...defaultFileSystem(), ...options.fileSystem };
  const files = await readManagedFiles(root, fileSystem);
  const version = validateVersion(
    files.tauriManifest.version,
    "src-tauri/tauri.conf.json",
  );
  const projections = projectedFiles(files, version);
  const destinations = [
    [files.paths.packageJson, files.packageJson, projections.packageJson],
    [files.paths.cargo, files.cargo, projections.cargo],
    [files.paths.cargoLock, files.cargoLock, projections.cargoLock],
  ]
    .filter(([, previous, next]) => previous !== next)
    .map(([path, , content]) => ({ content, path }));
  await publishFiles(destinations, fileSystem);
  await checkVersions(root, { fileSystem });
  return version;
}

export async function checkVersions(root = process.cwd(), options = {}) {
  const files = await readManagedFiles(root, options.fileSystem);
  const sourceVersion = validateVersion(
    files.tauriManifest.version,
    "src-tauri/tauri.conf.json",
  );
  const packageVersion = validateVersion(files.package.version, "package.json");
  const cargoVersion = validateVersion(files.workspace.version, "Cargo.toml");
  const lockVersions = [...files.lock.packages.entries()].map(
    ([name, entry]) => [
      `Cargo.lock:${name}`,
      validateVersion(entry.version, `Cargo.lock:${name}`),
    ],
  );
  const mismatches = [
    ["package.json", packageVersion],
    ["Cargo.toml", cargoVersion],
    ...lockVersions,
  ].filter(([, version]) => version !== sourceVersion);
  if (mismatches.length > 0) {
    const details = mismatches
      .map(([file, version]) => `${file}=${version}`)
      .join(", ");
    throw new VersionError(
      `Version drift detected: src-tauri/tauri.conf.json=${sourceVersion}; ${details}. Run pnpm version:sync.`,
    );
  }
  return sourceVersion;
}

async function run() {
  const command = process.argv[2];
  if (command !== "sync" && command !== "check") {
    throw new VersionError(
      "Usage: node scripts/manage-version.mjs <sync|check>",
    );
  }
  const version =
    command === "sync" ? await synchronizeVersions() : await checkVersions();
  console.log(`Version ${command} passed: ${version}`);
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
