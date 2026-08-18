import { execFile } from "node:child_process";
import { createHash } from "node:crypto";
import {
  chmod,
  cp,
  lstat,
  mkdir,
  mkdtemp,
  readFile,
  readdir,
  rm,
  writeFile,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { basename, dirname, join, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

import semver from "semver";

import { RELEASE_BUNDLE_TARGETS, runBuild } from "./manage-build-artifacts.mjs";
import { checkVersions } from "./manage-version.mjs";

const SCRIPT_ROOT = dirname(fileURLToPath(import.meta.url));
const DEFAULT_PROJECT_ROOT = resolve(SCRIPT_ROOT, "..");
const REPOSITORY = "Angry3D/ai-router";
const UPDATER_PUBLIC_KEY_PLACEHOLDER = "__AI_ROUTER_UPDATER_PUBLIC_KEY__";
const PUBLIC_KEY_ENV = "AI_ROUTER_UPDATER_PUBLIC_KEY";
const PRIVATE_KEY_ENV = "TAURI_SIGNING_PRIVATE_KEY";
const PRIVATE_KEY_PASSWORD_ENV = "TAURI_SIGNING_PRIVATE_KEY_PASSWORD";
const GENERATED_RELEASE_CONFIG = "src-tauri/tauri.release.conf.json";
const RELEASE_DIRECTORY = "release-distribution";
const DRAFT_DOWNLOAD_DIRECTORY = "release-draft-download";
const MAX_PUBLIC_KEY_LENGTH = 8_192;
const MAX_PRIVATE_KEY_LENGTH = 65_536;
const MAX_SIGNATURE_LENGTH = 16_384;
const MAX_RELEASE_NOTE_ITEM_CHARS = 240;
const MAX_RELEASE_NOTE_ITEMS = 20;
const MAX_RELEASE_NOTE_LINES = 80;
const MAX_RELEASE_NOTE_CHARS = 4_000;
const RELEASE_NOTE_SECTIONS = new Map([
  ["重点更新", "highlights"],
  ["问题修复", "fixes"],
  ["注意事项", "notices"],
]);

export class ReleaseError extends Error {}

function fail(message) {
  throw new ReleaseError(message);
}

function strictBase64(value, label) {
  if (
    typeof value !== "string" ||
    value.length === 0 ||
    value.length % 4 !== 0 ||
    !/^[A-Za-z0-9+/]+={0,2}$/.test(value)
  ) {
    fail(`${label} must be canonical base64.`);
  }
  const decoded = Buffer.from(value, "base64");
  if (decoded.toString("base64") !== value) {
    fail(`${label} must be canonical base64.`);
  }
  return decoded;
}

export function validateUpdaterPublicKey(value) {
  if (
    typeof value !== "string" ||
    value !== value.trim() ||
    value.length > MAX_PUBLIC_KEY_LENGTH ||
    value === UPDATER_PUBLIC_KEY_PLACEHOLDER
  ) {
    fail("The updater public key is missing or malformed.");
  }
  const decoded = strictBase64(value, "The updater public key").toString(
    "utf8",
  );
  if (decoded.includes("PRIVATE KEY")) {
    fail("The updater public key input must not contain private key material.");
  }
  const lines = decoded.trimEnd().split(/\r?\n/);
  if (
    lines.length !== 2 ||
    !lines[0].startsWith("untrusted comment: minisign public key") ||
    !/^[A-Za-z0-9+/]{56}$/.test(lines[1]) ||
    strictBase64(lines[1], "The minisign public key").length !== 42
  ) {
    fail("The updater public key is not a supported minisign public key.");
  }
  return value;
}

function validatePrivateSigningEnvironment(environment) {
  const privateKey = environment[PRIVATE_KEY_ENV];
  const password = environment[PRIVATE_KEY_PASSWORD_ENV];
  if (
    typeof privateKey !== "string" ||
    privateKey.trim().length === 0 ||
    privateKey.length > MAX_PRIVATE_KEY_LENGTH ||
    privateKey.includes("__AI_ROUTER_") ||
    typeof password !== "string" ||
    password.length === 0 ||
    password.length > MAX_PRIVATE_KEY_LENGTH
  ) {
    fail("The protected updater signing secrets are missing or malformed.");
  }
}

export function validateReleaseIdentity(
  { repository, ref, refName, refType, sha },
  version,
) {
  const parsedVersion = semver.parse(version, { loose: false });
  if (
    !parsedVersion ||
    parsedVersion.version !== version ||
    parsedVersion.prerelease.length > 0 ||
    parsedVersion.build.length > 0
  ) {
    fail("The release version must be a canonical stable SemVer.");
  }
  const tag = `v${version}`;
  if (
    repository !== REPOSITORY ||
    refType !== "tag" ||
    ref !== `refs/tags/${tag}` ||
    refName !== tag ||
    typeof sha !== "string" ||
    !/^[a-f0-9]{40}$/.test(sha)
  ) {
    fail(
      "The GitHub tag, ref, repository, commit, and app version must agree.",
    );
  }
  return { repository, sha, tag, version };
}

function identityFromEnvironment(environment, version) {
  return validateReleaseIdentity(
    {
      repository: environment.GITHUB_REPOSITORY,
      ref: environment.GITHUB_REF,
      refName: environment.GITHUB_REF_NAME,
      refType: environment.GITHUB_REF_TYPE,
      sha: environment.GITHUB_SHA,
    },
    version,
  );
}

export function expectedAssetNames(version) {
  return [
    `AI.Router_${version}_aarch64.dmg`,
    "AI.Router.app.tar.gz",
    "AI.Router.app.tar.gz.sig",
    "latest.json",
    "SHA256SUMS",
  ];
}

function checksumAssetNames(version) {
  return expectedAssetNames(version).slice(0, 4);
}

export function releaseNotes(version, document) {
  return releaseNotesFromDocument(version, document);
}

function releaseNotesFromDocument(version, document) {
  const authored = parseReleaseNotes(document?.markdown, version).markdown;
  return [
    authored,
    "",
    "首次安装请下载 DMG。该应用使用 ad-hoc 签名，未经 Apple Developer ID 验证或公证。",
    "已安装支持应用内更新的版本可在“设置 -> 系统 -> 应用更新”中检查并确认安装。",
    "",
    `版本详情：https://github.com/${REPOSITORY}/releases/tag/v${version}`,
  ].join("\n");
}

function boundedReleaseError(message) {
  fail(`Release notes are invalid: ${message}`);
}

function normalizeNoteItem(value) {
  if (
    [...value].some((character) => {
      const codePoint = character.codePointAt(0);
      return codePoint !== undefined && (codePoint < 32 || codePoint === 127);
    })
  ) {
    boundedReleaseError("items must be short plain text without markup.");
  }
  const item = value.trim().replace(/ +/g, " ");
  if (
    item.length === 0 ||
    [...item].length > MAX_RELEASE_NOTE_ITEM_CHARS ||
    ["`", "*", "_", "~", "<", ">", "[", "]"].some((marker) =>
      item.includes(marker),
    ) ||
    /https?:\/\/|www\./iu.test(item)
  ) {
    boundedReleaseError("items must be short plain text without markup.");
  }
  return item;
}

export function parseReleaseNotes(markdown, version) {
  if (typeof markdown !== "string" || markdown.length === 0) {
    boundedReleaseError("the version file is missing or empty.");
  }
  const normalized = markdown.replace(/\r\n?/g, "\n").trim();
  if (
    [...normalized].length > MAX_RELEASE_NOTE_CHARS ||
    normalized.split("\n").length > MAX_RELEASE_NOTE_LINES
  ) {
    boundedReleaseError("the document exceeds its size limit.");
  }
  const lines = normalized.split("\n");
  if (lines.shift() !== `# AI Router v${version}`) {
    boundedReleaseError("the version heading does not match the release.");
  }
  const sections = { highlights: [], fixes: [], notices: [] };
  let current = null;
  const seenSections = new Set();
  let lastSectionIndex = -1;
  for (const line of lines) {
    if (line === "") continue;
    if (line.startsWith("## ")) {
      const heading = line.slice(3);
      current = RELEASE_NOTE_SECTIONS.get(heading);
      if (!current)
        boundedReleaseError("only supported section headings are allowed.");
      const sectionIndex = [...RELEASE_NOTE_SECTIONS.keys()].indexOf(heading);
      if (seenSections.has(current) || sectionIndex <= lastSectionIndex) {
        boundedReleaseError(
          "sections must be unique and in the documented order.",
        );
      }
      seenSections.add(current);
      lastSectionIndex = sectionIndex;
      continue;
    }
    if (!current || !line.startsWith("- ") || line.startsWith("-  ")) {
      boundedReleaseError("sections must contain flat unordered-list items.");
    }
    sections[current].push(normalizeNoteItem(line.slice(2)));
  }
  if (sections.highlights.length < 1 || sections.highlights.length > 3) {
    boundedReleaseError("重点更新 must contain one to three items.");
  }
  const items = [
    ...sections.highlights,
    ...sections.fixes,
    ...sections.notices,
  ];
  if (
    items.length > MAX_RELEASE_NOTE_ITEMS ||
    new Set(items).size !== items.length
  ) {
    boundedReleaseError("items must be unique and within the item limit.");
  }
  if (
    [...seenSections].some(
      (section) => section !== "highlights" && sections[section].length === 0,
    )
  ) {
    boundedReleaseError("included sections must contain at least one item.");
  }
  if (
    items.some((item) =>
      /^(?:todo|tbd|placeholder|待补充|待定|稍后补充|待完善)(?:$|\s|[:：.!。！])/iu.test(
        item,
      ),
    )
  ) {
    boundedReleaseError("placeholder content is not allowed.");
  }
  if (
    items.some((item) =>
      /AI_ROUTER_|TAURI_SIGNING_|PRIVATE KEY|Authorization:|Bearer\s+\S+|sk-[a-z0-9_-]{12,}|file:\/\/|\/Users\/|\/home\/|[a-z]:\\Users\\/iu.test(
        item,
      ),
    )
  ) {
    boundedReleaseError("secret-like or local-path content is not allowed.");
  }
  const genericOnly = items.every((item) =>
    /^(?:AI Router v\d+\.\d+\.\d+ (?:已发布|released)|首次安装.*DMG|.*updater.*签名|.*更新.*签名)/iu.test(
      item,
    ),
  );
  if (genericOnly) {
    boundedReleaseError("generic placeholder content is not sufficient.");
  }
  const renderSection = (heading, values) =>
    values.length > 0
      ? [`## ${heading}`, ...values.map((item) => `- ${item}`)]
      : [];
  const canonical = [
    `# AI Router v${version}`,
    "",
    ...renderSection("重点更新", sections.highlights),
    ...(sections.fixes.length
      ? ["", ...renderSection("问题修复", sections.fixes)]
      : []),
    ...(sections.notices.length
      ? ["", ...renderSection("注意事项", sections.notices)]
      : []),
  ].join("\n");
  return { ...sections, markdown: canonical };
}

export async function loadReleaseNotes(root, version) {
  const parsedVersion = semver.parse(version, { loose: false });
  if (
    !parsedVersion ||
    parsedVersion.version !== version ||
    parsedVersion.prerelease.length > 0 ||
    parsedVersion.build.length > 0
  ) {
    boundedReleaseError(
      "the requested version is not a canonical stable SemVer.",
    );
  }
  const path = resolve(root, "release-notes", `v${version}.md`);
  let markdown;
  try {
    markdown = await readFile(path, "utf8");
  } catch {
    boundedReleaseError(`missing reviewed file for v${version}.`);
  }
  return parseReleaseNotes(markdown, version);
}

export function createLatestManifest(version, publicDate, signature, document) {
  const authoredNotes = parseReleaseNotes(document?.markdown, version).markdown;
  let normalizedPublicDate;
  try {
    normalizedPublicDate = new Date(publicDate).toISOString();
  } catch {
    fail("The release timestamp must be a canonical ISO-8601 value.");
  }
  if (normalizedPublicDate !== publicDate) {
    fail("The release timestamp must be a canonical ISO-8601 value.");
  }
  if (
    typeof signature !== "string" ||
    signature.length === 0 ||
    signature.length > MAX_SIGNATURE_LENGTH ||
    signature !== signature.trim()
  ) {
    fail("The updater archive signature is missing or malformed.");
  }
  strictBase64(signature, "The updater archive signature");
  return {
    version,
    notes: authoredNotes,
    pub_date: publicDate,
    platforms: {
      "darwin-aarch64": {
        signature,
        url: `https://github.com/${REPOSITORY}/releases/download/v${version}/AI.Router.app.tar.gz`,
      },
    },
  };
}

export function renderReleaseConfig(template, publicKey) {
  validateUpdaterPublicKey(publicKey);
  if (!template.includes(UPDATER_PUBLIC_KEY_PLACEHOLDER)) {
    fail("The release config is missing its updater public-key placeholder.");
  }
  const rendered = template.replace(UPDATER_PUBLIC_KEY_PLACEHOLDER, publicKey);
  if (rendered.includes(UPDATER_PUBLIC_KEY_PLACEHOLDER)) {
    fail("The release config contains more than one public-key placeholder.");
  }
  const parsed = JSON.parse(rendered);
  if (
    JSON.stringify(parsed.bundle?.targets) !==
      JSON.stringify(RELEASE_BUNDLE_TARGETS) ||
    parsed.bundle?.createUpdaterArtifacts !== true ||
    parsed.bundle?.resources?.["../LICENSE"] !== "LICENSE" ||
    parsed.bundle?.resources?.["../THIRD_PARTY_NOTICES.md"] !==
      "THIRD_PARTY_NOTICES.md" ||
    parsed.bundle?.macOS?.signingIdentity !== "-" ||
    parsed.plugins?.updater?.pubkey !== publicKey
  ) {
    fail(
      "The generated release config does not enforce the distribution contract.",
    );
  }
  return `${JSON.stringify(parsed, null, 2)}\n`;
}

function execute(
  command,
  args,
  { cwd, env = process.env, allowFailure = false } = {},
) {
  return new Promise((resolvePromise, rejectPromise) => {
    execFile(
      command,
      args,
      { cwd, env, encoding: "utf8", maxBuffer: 16 * 1024 * 1024 },
      (error, stdout, stderr) => {
        const result = { code: error?.code ?? 0, stderr, stdout };
        if (!error || allowFailure) {
          resolvePromise(result);
          return;
        }
        rejectPromise(
          new ReleaseError(`${command} failed during release verification.`),
        );
      },
    );
  });
}

async function releaseContext(root, environment, runner = execute) {
  const version = await checkVersions(root);
  const identity = identityFromEnvironment(environment, version);
  const tagCommit = (
    await runner("git", ["rev-parse", `${identity.tag}^{commit}`], {
      cwd: root,
    })
  ).stdout.trim();
  const headCommit = (
    await runner("git", ["rev-parse", "HEAD"], { cwd: root })
  ).stdout.trim();
  if (tagCommit !== identity.sha || headCommit !== identity.sha) {
    fail("The checked-out commit is not the immutable release tag commit.");
  }
  return identity;
}

function generatedDirectory(root, name) {
  const target = resolve(root, "target");
  const directory = resolve(target, name);
  if (!directory.startsWith(`${target}${sep}`) || directory === target) {
    fail("Refusing an unsafe generated release directory.");
  }
  return directory;
}

async function sha256(path) {
  return createHash("sha256")
    .update(await readFile(path))
    .digest("hex");
}

async function assertRegularFile(path, { maxBytes } = {}) {
  const metadata = await lstat(path);
  if (!metadata.isFile() || metadata.isSymbolicLink() || metadata.size <= 0) {
    fail(`Release asset is not a non-empty regular file: ${basename(path)}.`);
  }
  if (maxBytes !== undefined && metadata.size > maxBytes) {
    fail(`Release asset exceeds its size bound: ${basename(path)}.`);
  }
  return metadata.size;
}

function exactKeys(value, expected, label) {
  const actual = Object.keys(value ?? {}).sort();
  const wanted = [...expected].sort();
  if (
    actual.length !== wanted.length ||
    actual.some((key, index) => key !== wanted[index])
  ) {
    fail(`${label} contains an unexpected field.`);
  }
}

export async function verifyReleaseDirectory(
  directory,
  version,
  expectedDocument = null,
) {
  const expected = expectedAssetNames(version);
  const entries = (await readdir(directory)).sort();
  const wanted = [...expected].sort();
  if (
    entries.length !== wanted.length ||
    entries.some((name, index) => name !== wanted[index])
  ) {
    fail(
      "The release asset inventory is incomplete or contains unexpected files.",
    );
  }
  for (const name of expected) {
    await assertRegularFile(join(directory, name), {
      maxBytes:
        name === "latest.json"
          ? 32_768
          : name.endsWith(".sig")
            ? MAX_SIGNATURE_LENGTH
            : undefined,
    });
  }

  const signature = (
    await readFile(join(directory, "AI.Router.app.tar.gz.sig"), "utf8")
  ).trim();
  const manifestText = await readFile(join(directory, "latest.json"), "utf8");
  let manifest;
  try {
    manifest = JSON.parse(manifestText);
  } catch {
    fail("latest.json is not valid JSON.");
  }
  exactKeys(
    manifest,
    ["version", "notes", "pub_date", "platforms"],
    "latest.json",
  );
  exactKeys(manifest.platforms, ["darwin-aarch64"], "latest.json platforms");
  const platform = manifest.platforms["darwin-aarch64"];
  exactKeys(platform, ["signature", "url"], "latest.json platform entry");
  let normalizedPublicDate;
  try {
    normalizedPublicDate = new Date(manifest.pub_date).toISOString();
  } catch {
    fail("latest.json does not contain a canonical release timestamp.");
  }
  if (
    manifest.version !== version ||
    typeof manifest.notes !== "string" ||
    manifest.notes.length > 4_000 ||
    normalizedPublicDate !== manifest.pub_date ||
    platform.signature !== signature ||
    platform.url !==
      `https://github.com/${REPOSITORY}/releases/download/v${version}/AI.Router.app.tar.gz`
  ) {
    fail("latest.json does not match the canonical release assets.");
  }
  if (expectedDocument && manifest.notes !== expectedDocument.markdown) {
    fail("latest.json release notes do not match the reviewed source.");
  }

  const checksumLines = (await readFile(join(directory, "SHA256SUMS"), "utf8"))
    .trimEnd()
    .split("\n");
  const checksumNames = checksumAssetNames(version);
  if (checksumLines.length !== checksumNames.length) {
    fail("SHA256SUMS must cover every content asset exactly once.");
  }
  for (const [index, name] of checksumNames.entries()) {
    const expectedHash = await sha256(join(directory, name));
    if (checksumLines[index] !== `${expectedHash}  ${name}`) {
      fail(`SHA256SUMS does not match ${name}.`);
    }
  }
  return { assets: expected, manifest };
}

async function plistValue(bundle, key, runner) {
  return (
    await runner(
      "/usr/libexec/PlistBuddy",
      ["-c", `Print :${key}`, join(bundle, "Contents", "Info.plist")],
      {},
    )
  ).stdout.trim();
}

export async function inspectAppBundle(bundle, version, runner = execute) {
  const [identifier, name, bundleVersion, minimumSystemVersion, executable] =
    await Promise.all([
      plistValue(bundle, "CFBundleIdentifier", runner),
      plistValue(bundle, "CFBundleName", runner),
      plistValue(bundle, "CFBundleShortVersionString", runner),
      plistValue(bundle, "LSMinimumSystemVersion", runner),
      plistValue(bundle, "CFBundleExecutable", runner),
    ]);
  if (
    identifier !== "com.relax.airouter" ||
    name !== "AI Router" ||
    executable !== "ai-router-app" ||
    bundleVersion !== version ||
    minimumSystemVersion !== "13.0"
  ) {
    fail(
      "The application bundle metadata does not match the release contract.",
    );
  }
  const architecture = (
    await runner(
      "lipo",
      ["-archs", join(bundle, "Contents", "MacOS", executable)],
      {},
    )
  ).stdout.trim();
  if (architecture !== "arm64") {
    fail("The release application must contain only the arm64 architecture.");
  }
  await runner("codesign", ["--verify", "--deep", "--strict", bundle], {});
  const signature = await runner("codesign", ["-dvvv", bundle], {});
  const signatureDescription = `${signature.stdout}\n${signature.stderr}`;
  if (
    !signatureDescription.includes("Signature=adhoc") ||
    signatureDescription.includes("Authority=") ||
    signatureDescription.includes("Developer ID") ||
    (!signatureDescription.includes("TeamIdentifier=not set") &&
      !signatureDescription.includes("TeamIdentifier=not set\n"))
  ) {
    fail("The application must have only an explicit ad-hoc signature.");
  }
  await Promise.all([
    assertRegularFile(join(bundle, "Contents", "Resources", "LICENSE")),
    assertRegularFile(
      join(bundle, "Contents", "Resources", "THIRD_PARTY_NOTICES.md"),
    ),
  ]);
}

async function inspectDmg(dmg, version, runner = execute) {
  const mountRoot = await mkdtemp(join(tmpdir(), "ai-router-release-dmg-"));
  try {
    await runner(
      "hdiutil",
      ["attach", dmg, "-readonly", "-nobrowse", "-mountpoint", mountRoot],
      {},
    );
    const app = join(mountRoot, "AI Router.app");
    await inspectAppBundle(app, version, runner);
  } finally {
    await runner("hdiutil", ["detach", mountRoot], { allowFailure: true });
    await rm(mountRoot, { force: true, recursive: true });
  }
}

async function findSingleFile(directory, predicate, label) {
  const matches = (await readdir(directory)).filter(predicate);
  if (matches.length !== 1)
    fail(`Expected exactly one ${label} build artifact.`);
  return join(directory, matches[0]);
}

async function stageReleaseArtifacts(
  root,
  identity,
  publicKey,
  runner = execute,
) {
  const bundleRoot = resolve(root, "target", "release", "bundle");
  const macosRoot = join(bundleRoot, "macos");
  const dmgRoot = join(bundleRoot, "dmg");
  const app = join(macosRoot, "AI Router.app");
  const dmg = await findSingleFile(
    dmgRoot,
    (name) => name.endsWith(`_${identity.version}_aarch64.dmg`),
    "versioned arm64 DMG",
  );
  const archive = await findSingleFile(
    macosRoot,
    (name) => name.endsWith(".app.tar.gz"),
    "application updater archive",
  );
  const archiveSignature = await findSingleFile(
    macosRoot,
    (name) => name.endsWith(".app.tar.gz.sig"),
    "application updater signature",
  );
  await inspectAppBundle(app, identity.version, runner);
  await inspectDmg(dmg, identity.version, runner);

  const directory = generatedDirectory(root, RELEASE_DIRECTORY);
  await rm(directory, { force: true, recursive: true });
  await mkdir(directory, { recursive: true, mode: 0o700 });
  const names = expectedAssetNames(identity.version);
  await Promise.all([
    cp(dmg, join(directory, names[0])),
    cp(archive, join(directory, names[1])),
    cp(archiveSignature, join(directory, names[2])),
  ]);
  const signature = (await readFile(join(directory, names[2]), "utf8")).trim();
  const commitDate = (
    await runner("git", ["show", "-s", "--format=%cI", identity.sha], {
      cwd: root,
    })
  ).stdout.trim();
  const publicDate = new Date(commitDate).toISOString();
  const notes = await loadReleaseNotes(root, identity.version);
  const manifest = createLatestManifest(
    identity.version,
    publicDate,
    signature,
    notes,
  );
  await writeFile(
    join(directory, "latest.json"),
    `${JSON.stringify(manifest, null, 2)}\n`,
    "utf8",
  );
  const checksumLines = [];
  for (const name of checksumAssetNames(identity.version)) {
    checksumLines.push(`${await sha256(join(directory, name))}  ${name}`);
  }
  await writeFile(
    join(directory, "SHA256SUMS"),
    `${checksumLines.join("\n")}\n`,
    "utf8",
  );
  await verifyReleaseDirectory(directory, identity.version, notes);
  await runner(
    "cargo",
    [
      "run",
      "--quiet",
      "-p",
      "ai-router-app",
      "--example",
      "verify_update_signature",
      "--",
      join(directory, names[1]),
      join(directory, names[2]),
    ],
    { cwd: root, env: { ...process.env, [PUBLIC_KEY_ENV]: publicKey } },
  );
  return directory;
}

async function readDraft(identity, runner = execute) {
  const result = await runner(
    "gh",
    [
      "release",
      "view",
      identity.tag,
      "--repo",
      identity.repository,
      "--json",
      "isDraft,tagName,targetCommitish,assets,body",
    ],
    { allowFailure: true },
  );
  if (result.code !== 0) {
    if (/release not found|HTTP 404/i.test(result.stderr)) return null;
    fail("GitHub could not confirm the current draft release state.");
  }
  let release;
  try {
    release = JSON.parse(result.stdout);
  } catch {
    fail("GitHub returned malformed draft release metadata.");
  }
  return validateRepairableDraft(release, identity);
}

export function validateRepairableDraft(release, identity) {
  if (
    release?.tagName !== identity.tag ||
    release?.targetCommitish !== identity.sha ||
    release?.isDraft !== true ||
    !Array.isArray(release?.assets)
  ) {
    fail("The release already exists outside the repairable draft boundary.");
  }
  return release;
}

export function validateDraftBody(release, expectedBody) {
  if (release?.body !== expectedBody) {
    fail("The draft release body does not match the reviewed release notes.");
  }
}

async function ensureEmptyDraft(identity, document, runner = execute) {
  const expectedBody = releaseNotesFromDocument(identity.version, document);
  let release = await readDraft(identity, runner);
  if (!release) {
    await runner(
      "gh",
      [
        "release",
        "create",
        identity.tag,
        "--repo",
        identity.repository,
        "--draft",
        "--verify-tag",
        "--target",
        identity.sha,
        "--title",
        `AI Router ${identity.version}`,
        "--notes",
        expectedBody,
      ],
      {},
    );
    release = await readDraft(identity, runner);
  }
  if (!release) fail("GitHub did not create the draft release.");
  if (release.body !== expectedBody) {
    await runner(
      "gh",
      [
        "release",
        "edit",
        identity.tag,
        "--repo",
        identity.repository,
        "--notes",
        expectedBody,
      ],
      {},
    );
    release = await readDraft(identity, runner);
    if (!release) fail("GitHub did not retain the repaired draft release.");
  }
  validateDraftBody(release, expectedBody);
  for (const asset of release.assets) {
    if (typeof asset?.name !== "string" || asset.name.length > 256) {
      fail("The draft contains an invalid asset name.");
    }
    await runner(
      "gh",
      [
        "release",
        "delete-asset",
        identity.tag,
        asset.name,
        "--repo",
        identity.repository,
        "--yes",
      ],
      {},
    );
  }
}

async function compareDirectories(left, right, version) {
  for (const name of expectedAssetNames(version)) {
    if (
      (await sha256(join(left, name))) !== (await sha256(join(right, name)))
    ) {
      fail(`The uploaded draft asset does not match local bytes: ${name}.`);
    }
  }
}

export function validateRemoteAssetInventory(assets, version) {
  const expected = new Set(expectedAssetNames(version));
  const seen = new Set();
  if (!Array.isArray(assets) || assets.length !== expected.size) {
    fail("The draft release asset inventory is incomplete or unexpected.");
  }
  for (const asset of assets) {
    if (
      typeof asset?.name !== "string" ||
      !expected.has(asset.name) ||
      seen.has(asset.name) ||
      !Number.isSafeInteger(asset.size) ||
      asset.size <= 0
    ) {
      fail("The draft release asset inventory is incomplete or unexpected.");
    }
    seen.add(asset.name);
  }
}

async function verifyRemoteDraft(root, identity, runner = execute) {
  const document = await loadReleaseNotes(root, identity.version);
  const release = await readDraft(identity, runner);
  if (!release) fail("The draft release is missing.");
  validateDraftBody(
    release,
    releaseNotesFromDocument(identity.version, document),
  );
  validateRemoteAssetInventory(release.assets, identity.version);
  const downloadDirectory = generatedDirectory(root, DRAFT_DOWNLOAD_DIRECTORY);
  await rm(downloadDirectory, { force: true, recursive: true });
  await mkdir(downloadDirectory, { recursive: true, mode: 0o700 });
  await runner(
    "gh",
    [
      "release",
      "download",
      identity.tag,
      "--repo",
      identity.repository,
      "--dir",
      downloadDirectory,
    ],
    {},
  );
  await verifyReleaseDirectory(downloadDirectory, identity.version, document);
  await compareDirectories(
    generatedDirectory(root, RELEASE_DIRECTORY),
    downloadDirectory,
    identity.version,
  );
}

export async function uploadPreparedDraft(
  identity,
  directory,
  runner = execute,
) {
  const release = await readDraft(identity, runner);
  if (!release) fail("The draft release is missing before asset upload.");
  if (release.assets.length !== 0) {
    fail("The draft release must be empty immediately before asset upload.");
  }
  await runner(
    "gh",
    [
      "release",
      "upload",
      identity.tag,
      ...expectedAssetNames(identity.version).map((name) =>
        join(directory, name),
      ),
      "--repo",
      identity.repository,
    ],
    {},
  );
}

export async function buildRelease(
  root = DEFAULT_PROJECT_ROOT,
  environment = process.env,
  { runBuildImpl = runBuild, runner = execute } = {},
) {
  const identity = await releaseContext(root, environment, runner);
  await loadReleaseNotes(root, identity.version);
  const publicKey = validateUpdaterPublicKey(environment[PUBLIC_KEY_ENV]);
  validatePrivateSigningEnvironment(environment);
  const template = await readFile(
    resolve(root, GENERATED_RELEASE_CONFIG),
    "utf8",
  );
  const temporaryRoot = await mkdtemp(
    join(tmpdir(), "ai-router-release-config-"),
  );
  const temporaryConfig = join(temporaryRoot, "tauri.release.conf.json");
  try {
    await writeFile(temporaryConfig, renderReleaseConfig(template, publicKey), {
      encoding: "utf8",
      mode: 0o600,
    });
    await chmod(temporaryConfig, 0o600);
    await runBuildImpl("release", {
      root,
      releaseConfigPath: temporaryConfig,
      baseEnvironment: environment,
    });
  } finally {
    await rm(temporaryRoot, { force: true, recursive: true });
  }
}

async function validateCommand(root, environment, runner) {
  const identity = await releaseContext(root, environment, runner);
  await loadReleaseNotes(root, identity.version);
  validateUpdaterPublicKey(environment[PUBLIC_KEY_ENV]);
}

async function draftCommand(root, environment, runner) {
  const identity = await releaseContext(root, environment, runner);
  const document = await loadReleaseNotes(root, identity.version);
  await ensureEmptyDraft(identity, document, runner);
}

async function prepareCommand(root, environment, runner) {
  const identity = await releaseContext(root, environment, runner);
  const publicKey = validateUpdaterPublicKey(environment[PUBLIC_KEY_ENV]);
  const directory = await stageReleaseArtifacts(
    root,
    identity,
    publicKey,
    runner,
  );
  await uploadPreparedDraft(identity, directory, runner);
  await verifyRemoteDraft(root, identity, runner);
}

async function publishCommand(root, environment, runner) {
  const identity = await releaseContext(root, environment, runner);
  await verifyRemoteDraft(root, identity, runner);
  await runner(
    "gh",
    [
      "release",
      "edit",
      identity.tag,
      "--repo",
      identity.repository,
      "--draft=false",
    ],
    {},
  );
  const published = await runner(
    "gh",
    [
      "release",
      "view",
      identity.tag,
      "--repo",
      identity.repository,
      "--json",
      "isDraft,tagName",
    ],
    {},
  );
  const state = JSON.parse(published.stdout);
  if (state.isDraft !== false || state.tagName !== identity.tag) {
    fail("GitHub did not atomically publish the verified draft release.");
  }
}

async function run() {
  const [command, ...extra] = process.argv.slice(2);
  if (
    extra.length > 0 ||
    !["validate", "draft", "build", "prepare", "publish"].includes(command)
  ) {
    fail(
      "Usage: node scripts/manage-release.mjs <validate|draft|build|prepare|publish>",
    );
  }
  if (command === "validate")
    await validateCommand(DEFAULT_PROJECT_ROOT, process.env, execute);
  if (command === "draft")
    await draftCommand(DEFAULT_PROJECT_ROOT, process.env, execute);
  if (command === "build") await buildRelease();
  if (command === "prepare")
    await prepareCommand(DEFAULT_PROJECT_ROOT, process.env, execute);
  if (command === "publish")
    await publishCommand(DEFAULT_PROJECT_ROOT, process.env, execute);
  console.log(`Release ${command} passed.`);
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
