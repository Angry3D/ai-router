import { access, readdir, readFile } from "node:fs/promises";
import { dirname, isAbsolute, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

import { format } from "prettier";

const SCRIPT_ROOT = dirname(fileURLToPath(import.meta.url));
const DEFAULT_PROJECT_ROOT = resolve(SCRIPT_ROOT, "..");

const REQUIRED_FILES = [
  "README.md",
  "README.zh-CN.md",
  "AGENTS.md",
  "CONTRIBUTING.md",
  "SECURITY.md",
  "CODE_OF_CONDUCT.md",
  "SUPPORT.md",
  "THIRD_PARTY_NOTICES.md",
  "docs/engineering/README.md",
  "docs/engineering/development-lifecycle.md",
  "docs/engineering/architecture.md",
  "docs/engineering/routing-resilience.md",
  "docs/engineering/data-privacy-recovery.md",
  "docs/engineering/native-lifecycle.md",
  "docs/engineering/application-updates.md",
  "docs/engineering/releasing.md",
  "docs/engineering/verification.md",
  "docs/engineering/github-security-settings.md",
  "docs/images/readme/README.md",
  "release-notes/README.md",
  ".github/ISSUE_TEMPLATE/bug_report.yml",
  ".github/ISSUE_TEMPLATE/feature_request.yml",
  ".github/ISSUE_TEMPLATE/config.yml",
  ".github/PULL_REQUEST_TEMPLATE.md",
];

const MARKDOWN_FILES = REQUIRED_FILES.filter((path) => path.endsWith(".md"));
const YAML_FILES = REQUIRED_FILES.filter((path) => path.endsWith(".yml"));
const SENSITIVE_WARNING_FILES = [
  "SECURITY.md",
  "SUPPORT.md",
  ".github/ISSUE_TEMPLATE/bug_report.yml",
  ".github/ISSUE_TEMPLATE/feature_request.yml",
  ".github/PULL_REQUEST_TEMPLATE.md",
];
const FORBIDDEN_TEXT_MARKERS = [
  "<!-- TRELLIS:START -->",
  "Managed by Trellis.",
  "@mindfoldhq/trellis",
  "AGPL-3.0-only",
  "CC Switch",
  "design-qa.md",
];
// Third-party license text is reproduced verbatim and must never be edited to
// satisfy the prose marker scan; local paths and links are still checked.
const MARKER_SCAN_EXEMPT_FILES = new Set(["THIRD_PARTY_NOTICES.md"]);
const RELEASE_NOTES_DIRECTORY = "release-notes";
const RELEASE_NOTE_NAME_PATTERN = /^v\d[^/]*\.md$/;
// Each versioned note ships a reviewed English source and a reviewed Chinese
// counterpart. The Chinese file is authored content, never generated at check
// time; this gate only proves the two projections describe the same release.
const ENGLISH_RELEASE_NOTE_PATTERN = /^v(\d+\.\d+\.\d+)\.md$/;
const CHINESE_RELEASE_NOTE_PATTERN = /^v(\d+\.\d+\.\d+)\.zh-CN\.md$/;
// Both projections are matched by position, so the compared headings arrays are
// identical only when the sections appear in the same canonical order in both
// files. The canonical order itself is enforced per file by `projection`.
const ENGLISH_RELEASE_NOTE_PROJECTION = {
  heading: (version) => `# AI Router release v${version}`,
  sections: ["Highlights", "Fixes", "Notices"],
};
const CHINESE_RELEASE_NOTE_PROJECTION = {
  heading: (version) => `# AI Router v${version}`,
  sections: ["重点更新", "问题修复", "注意事项"],
};

/**
 * Structural shape of one structured release note: the version heading, the
 * ordered section headings, and the bullet count per section.
 *
 * The checks here deliberately mirror the release producer's parser, so a
 * document that passes this gate cannot still be rejected later at release
 * time. Only the order-preserving comparison below proves that two projections
 * describe the same release: comparing counts by position alone would accept a
 * note whose sections are swapped.
 */
function releaseNoteShape(content, version, projection) {
  const lines = content.replace(/\r\n?/gu, "\n").trim().split("\n");
  if (lines.shift() !== projection.heading(version)) {
    return null;
  }
  const headings = [];
  const counts = [];
  let current = null;
  let lastSectionIndex = -1;
  for (const line of lines) {
    const trimmed = line.trim();
    if (trimmed === "") continue;
    if (trimmed.startsWith("## ")) {
      const heading = trimmed.slice(3).trim();
      const sectionIndex = projection.sections.indexOf(heading);
      if (sectionIndex < 0) return null;
      if (sectionIndex <= lastSectionIndex) return null;
      headings.push(heading);
      counts.push(0);
      current = counts.length - 1;
      lastSectionIndex = sectionIndex;
      continue;
    }
    if (!trimmed.startsWith("- ") || current === null) return null;
    counts[current] += 1;
  }
  if (headings.length === 0) return null;
  return { headings, counts };
}

/**
 * Every discovered English note requires a Chinese counterpart with the same
 * section set in the same order and the same item count per section. Divergence
 * means the two reviewed projections describe different releases, so it fails
 * before anything is published.
 */
export function validateReleaseNoteLanguagePairs(files) {
  const english = new Map();
  for (const path of files.keys()) {
    if (!path.startsWith(`${RELEASE_NOTES_DIRECTORY}/`)) continue;
    const name = path.slice(RELEASE_NOTES_DIRECTORY.length + 1);
    const match = ENGLISH_RELEASE_NOTE_PATTERN.exec(name);
    if (match) english.set(match[1], path);
  }
  if (english.size === 0) {
    throw new PublicDocsError(
      `No English release notes were discovered in ${RELEASE_NOTES_DIRECTORY}/.`,
    );
  }
  const chinese = new Map();
  for (const path of files.keys()) {
    if (!path.startsWith(`${RELEASE_NOTES_DIRECTORY}/`)) continue;
    const name = path.slice(RELEASE_NOTES_DIRECTORY.length + 1);
    const match = CHINESE_RELEASE_NOTE_PATTERN.exec(name);
    if (match) chinese.set(match[1], path);
  }
  for (const [version, englishPath] of english) {
    const chinesePath = chinese.get(version);
    if (!chinesePath) {
      throw new PublicDocsError(
        `Release v${version} has ${englishPath} but no reviewed Chinese counterpart (${RELEASE_NOTES_DIRECTORY}/v${version}.zh-CN.md).`,
      );
    }
    const englishShape = releaseNoteShape(
      files.get(englishPath),
      version,
      ENGLISH_RELEASE_NOTE_PROJECTION,
    );
    const chineseShape = releaseNoteShape(
      files.get(chinesePath),
      version,
      CHINESE_RELEASE_NOTE_PROJECTION,
    );
    if (!englishShape || !chineseShape) {
      throw new PublicDocsError(
        `Release v${version} does not use the documented heading, section order, or item layout expected by the release producer.`,
      );
    }
    if (
      englishShape.headings.length !== chineseShape.headings.length ||
      englishShape.headings.some(
        (_heading, index) => englishShape.counts[index] !== chineseShape.counts[index],
      )
    ) {
      throw new PublicDocsError(
        `Release v${version} English and Chinese notes disagree: ${englishPath} covers ${englishShape.headings.length} sections with ${englishShape.counts.join("/")} items and ${chinesePath} covers ${chineseShape.headings.length} sections with ${chineseShape.counts.join("/")} items.`,
      );
    }
  }
  for (const [version] of chinese) {
    if (!english.has(version)) {
      throw new PublicDocsError(
        `Release v${version} has a Chinese note without an English source in ${RELEASE_NOTES_DIRECTORY}/.`,
      );
    }
  }
  // Discovery accepts any `v….md` file, but only a canonical stable SemVer can
  // ever be published. A versioned note outside that set would otherwise pass
  // every public-document gate while never being paired, which is exactly the
  // silent-discard case this gate exists to prevent.
  for (const path of files.keys()) {
    if (!path.startsWith(`${RELEASE_NOTES_DIRECTORY}/`)) continue;
    const name = path.slice(RELEASE_NOTES_DIRECTORY.length + 1);
    // `README.md` and other unversioned files are prose, not release notes.
    if (!RELEASE_NOTE_NAME_PATTERN.test(name)) continue;
    if (
      ENGLISH_RELEASE_NOTE_PATTERN.test(name) ||
      CHINESE_RELEASE_NOTE_PATTERN.test(name)
    ) {
      continue;
    }
    throw new PublicDocsError(
      `${RELEASE_NOTES_DIRECTORY}/${name} is not a canonical stable release note (v<major>.<minor>.<patch>[.zh-CN].md).`,
    );
  }
  return english.size;
}

const REQUIRED_README_COMMANDS = [
  "pnpm install --frozen-lockfile",
  "pnpm docs:check",
  "pnpm lint",
  "pnpm typecheck",
  "pnpm test",
  "pnpm build",
  "cargo fmt --check",
  "cargo clippy --workspace --all-targets -- -D warnings",
  "cargo test --workspace",
  "pnpm tauri:prod:build",
];
const REQUIRED_CONTRIBUTING_COMMANDS = [
  "pnpm ci:policy",
  "pnpm contracts:check",
  "pnpm generate:types",
  "pnpm security:public:check",
  "pnpm version:check",
  "pnpm license:public:check",
];
const DELIVERY_CONTROL_COMMANDS = [
  "Start task: <item>",
  "Start task: <item>, without Trellis",
  "Continue the current task",
  "Check the current task",
  "Deliver the current task",
  "Park the current work; do not merge yet",
  "Bring the current task into main",
  "Defer the current task",
  "Prepare the next release",
  "Include <item list> in <version>, defer everything else unmerged",
  "Release <version>",
  "Show delivery status",
];

export class PublicDocsError extends Error {}

async function readRequired(projectRoot, path) {
  try {
    return await readFile(resolve(projectRoot, path), "utf8");
  } catch {
    throw new PublicDocsError(
      `Required public documentation file is missing: ${path}.`,
    );
  }
}

/**
 * Discovers versioned release notes directly from disk so that future notes are
 * covered without editing this checker. An absent or empty set is an error.
 */
export async function discoverReleaseNotes(projectRoot) {
  const directory = resolve(projectRoot, RELEASE_NOTES_DIRECTORY);
  let entries;
  try {
    entries = await readdir(directory, { withFileTypes: true });
  } catch {
    throw new PublicDocsError(
      `Required public documentation directory is missing: ${RELEASE_NOTES_DIRECTORY}/.`,
    );
  }
  entries.sort((left, right) =>
    left.name < right.name ? -1 : left.name > right.name ? 1 : 0,
  );
  const releaseNotes = [];
  for (const entry of entries) {
    if (!RELEASE_NOTE_NAME_PATTERN.test(entry.name)) continue;
    // Only regular repository files qualify. A symlink (or any other
    // non-regular entry) is rejected instead of followed: a versioned note
    // must be a reviewed file inside the tree, never a path to outside content.
    if (!entry.isFile()) {
      throw new PublicDocsError(
        `${RELEASE_NOTES_DIRECTORY}/${entry.name} is not a regular file.`,
      );
    }
    releaseNotes.push(`${RELEASE_NOTES_DIRECTORY}/${entry.name}`);
  }
  if (releaseNotes.length === 0) {
    throw new PublicDocsError(
      `${RELEASE_NOTES_DIRECTORY}/ contains no versioned release notes (${RELEASE_NOTES_DIRECTORY}/v<version>.md).`,
    );
  }
  return releaseNotes;
}

/**
 * Resolves the full public documentation surface: the fixed required inventory
 * plus every release note discovered on disk.
 */
export async function resolvePublicDocumentPaths(
  projectRoot = DEFAULT_PROJECT_ROOT,
) {
  const releaseNotes = await discoverReleaseNotes(projectRoot);
  return { required: [...REQUIRED_FILES], releaseNotes };
}

function assertContains(content, expected, path) {
  if (!content.includes(expected)) {
    throw new PublicDocsError(
      `${path} is missing required contract text: ${expected}.`,
    );
  }
}

export function validatePublicText(
  path,
  content,
  { skipForbiddenMarkers = false } = {},
) {
  if (!skipForbiddenMarkers) {
    for (const marker of FORBIDDEN_TEXT_MARKERS) {
      if (content.includes(marker)) {
        throw new PublicDocsError(`${path} contains a private workflow marker.`);
      }
    }
  }
  const localPathPatterns = [
    /(?:^|[\s"'(=])\/Users\/[^/\s"']+\//,
    /(?:^|[\s"'(=])\/home\/[^/\s"']+\//,
    /\/var\/folders\/[A-Za-z0-9_-]+\//,
    /\b[A-Za-z]:\\Users\\[^\\\s"']+\\/,
    /\bfile:\/\/\/(?:Users|home)\//,
  ];
  if (localPathPatterns.some((pattern) => pattern.test(content))) {
    throw new PublicDocsError(`${path} contains a local absolute path.`);
  }
}

function markdownTargets(content) {
  const targets = [];
  for (const match of content.matchAll(/\[[^\]]+\]\(([^)]+)\)/g)) {
    let target = match[1].trim();
    if (target.startsWith("<") && target.endsWith(">")) {
      target = target.slice(1, -1);
    }
    targets.push(target);
  }
  return targets;
}

function htmlImageTags(content) {
  return [...content.matchAll(/<img\b[^>]*>/g)].map((match) => match[0]);
}

function htmlAttribute(tag, name) {
  const match = tag.match(new RegExp(`\\b${name}="([^"]*)"`));
  return match ? match[1] : null;
}

export function markdownSlug(text) {
  return text
    .replace(/<[^>]*>/g, "")
    .replace(/[`*_~]/g, "")
    .toLowerCase()
    .trim()
    .replace(/[^\p{L}\p{N}\p{M}\- ]/gu, "")
    .replace(/ +/g, "-");
}

function headingAnchorSet(content) {
  const anchors = new Set();
  const counts = new Map();
  let inFence = false;
  for (const line of content.split("\n")) {
    if (/^\s*(```|~~~)/.test(line)) {
      inFence = !inFence;
      continue;
    }
    if (inFence) continue;
    const heading = line.match(/^(#{1,6})\s+(.+?)\s*#*\s*$/);
    if (heading) {
      const slug = markdownSlug(heading[2]);
      const count = counts.get(slug) ?? 0;
      counts.set(slug, count + 1);
      anchors.add(count === 0 ? slug : `${slug}-${count}`);
    }
    for (const match of line.matchAll(/<a[^>]+(?:id|name)="([^"]+)"/g)) {
      anchors.add(match[1]);
    }
  }
  return anchors;
}

function assertTargetInsideRoot(root, path, target, destination) {
  const escaped = relative(root, destination);
  if (
    escaped === ".." ||
    escaped.startsWith(`..${sep}`) ||
    isAbsolute(escaped)
  ) {
    throw new PublicDocsError(
      `${path} links outside the repository: ${target}.`,
    );
  }
}

function decodeLinkTarget(path, target, value) {
  try {
    return decodeURIComponent(value);
  } catch {
    throw new PublicDocsError(
      `${path} contains an invalid encoded link: ${target}.`,
    );
  }
}

async function resolveRelativeTarget(
  root,
  documentRoot,
  path,
  target,
  filePart,
) {
  const decoded = decodeLinkTarget(path, target, filePart);
  if (isAbsolute(decoded)) {
    throw new PublicDocsError(
      `${path} contains an absolute local link: ${target}.`,
    );
  }
  const destination = resolve(documentRoot, decoded);
  assertTargetInsideRoot(root, path, target, destination);
  try {
    await access(destination);
  } catch {
    throw new PublicDocsError(
      `${path} contains a broken relative link: ${target}.`,
    );
  }
  return destination;
}

async function validateFragment(path, target, destination, fragment) {
  const decoded = decodeLinkTarget(path, target, fragment);
  if (!decoded) return;
  if (!destination.endsWith(".md")) return;
  let content;
  try {
    content = await readFile(destination, "utf8");
  } catch {
    return;
  }
  if (!headingAnchorSet(content).has(decoded)) {
    throw new PublicDocsError(
      `${path} links to a missing fragment: ${target}.`,
    );
  }
}

export async function validateMarkdownLinks(projectRoot, path, content) {
  const documentRoot = dirname(resolve(projectRoot, path));
  const root = resolve(projectRoot);
  const ownAnchors = headingAnchorSet(content);
  for (const target of markdownTargets(content)) {
    if (/^(?:https?:|mailto:)/.test(target)) continue;
    const [filePart = "", fragment = ""] = target.split("#");
    const fileTarget = filePart.split("?", 1)[0];
    if (!fileTarget) {
      if (fragment && !ownAnchors.has(decodeLinkTarget(path, target, fragment))) {
        throw new PublicDocsError(
          `${path} links to a missing fragment: ${target}.`,
        );
      }
      continue;
    }
    const destination = await resolveRelativeTarget(
      root,
      documentRoot,
      path,
      target,
      fileTarget,
    );
    if (fragment) {
      await validateFragment(path, target, destination, fragment);
    }
  }
  for (const tag of htmlImageTags(content)) {
    const src = htmlAttribute(tag, "src");
    const alt = htmlAttribute(tag, "alt");
    if (!src || /^(?:https?:|data:)/.test(src)) continue;
    await resolveRelativeTarget(root, documentRoot, path, src, src);
    if (alt === null || alt.trim() === "") {
      throw new PublicDocsError(
        `${path} contains an image without alternative text: ${src}.`,
      );
    }
  }
}

const SENSITIVE_WARNING_TERMS = [
  ["API key"],
  ["complete configuration", "full configuration", "entire configuration", "完整配置"],
  ["database"],
  ["raw log", "原始日志"],
];

export function validateSensitiveWarning(path, content) {
  for (const variants of SENSITIVE_WARNING_TERMS) {
    if (
      !variants.some((term) => content.toLowerCase().includes(term.toLowerCase()))
    ) {
      throw new PublicDocsError(
        `${path} does not warn against sharing ${variants[0]}.`,
      );
    }
  }
}

/// Phase phrases that satisfy the early-stage project disclaimer. A bare
/// substring such as "early" would also match "nearly"/"clearly", so the check
/// requires one of the complete phrases while leaving the exact wording free.
const EARLY_STAGE_PHRASES = ["early-stage", "early stage", "early development"];

export function validateVersionIndependentProjectClaims(files) {
  for (const path of ["README.md", "CONTRIBUTING.md", "SUPPORT.md"]) {
    const content = files.get(path).toLowerCase();
    if (!EARLY_STAGE_PHRASES.some((phrase) => content.includes(phrase))) {
      throw new PublicDocsError(
        `${path} is missing the early-stage project disclaimer.`,
      );
    }
  }
  const engineeringIndex = files.get("docs/engineering/README.md");
  if (
    !engineeringIndex
      .toLowerCase()
      .includes("current product and release boundaries")
  ) {
    throw new PublicDocsError(
      "docs/engineering/README.md does not describe the current product and release boundaries.",
    );
  }
  assertContains(files.get("README.md"), "GitHub Releases", "README.md");
}

export function validateReadmeLanguageSwitches(files) {
  const english = files.get("README.md");
  const chinese = files.get("README.zh-CN.md");
  assertContains(english, "](./README.zh-CN.md)", "README.md");
  assertContains(chinese, "](./README.md)", "README.zh-CN.md");
}

export function validateReleaseInventoryContract(packageJson, files) {
  if (
    packageJson.scripts?.["release:inventory"] !==
    "node scripts/release-inventory.mjs"
  ) {
    throw new PublicDocsError(
      "package.json release:inventory does not invoke the release inventory checker.",
    );
  }
  for (const path of [
    "docs/engineering/releasing.md",
    "release-notes/README.md",
  ]) {
    assertContains(files.get(path), "pnpm release:inventory", path);
  }
  assertContains(
    files.get("docs/engineering/releasing.md"),
    "cannot automatically prove semantic completeness",
    "docs/engineering/releasing.md",
  );
  assertContains(
    files.get("release-notes/README.md"),
    "record a concise exclusion reason",
    "release-notes/README.md",
  );
}

export function validateDeliveryLifecycleContract(packageJson, files) {
  if (
    packageJson.scripts?.["delivery:status"] !==
    "node scripts/delivery-status.mjs"
  ) {
    throw new PublicDocsError(
      "package.json delivery:status does not invoke the delivery status checker.",
    );
  }
  const path = "docs/engineering/development-lifecycle.md";
  const lifecycle = files.get(path);
  for (const command of DELIVERY_CONTROL_COMMANDS) {
    assertContains(lifecycle, `\`${command}\``, path);
  }
  for (const command of ["pnpm delivery:status", "pnpm release:inventory"]) {
    assertContains(lifecycle, command, path);
  }
  for (const state of ["commit", "push", "PR", "main", "stable tag"]) {
    assertContains(lifecycle, state, path);
  }
}

async function validateYaml(path, content) {
  if (content.includes("\t")) {
    throw new PublicDocsError(
      `${path} contains a tab; GitHub forms require stable YAML indentation.`,
    );
  }
  try {
    await format(content, { filepath: path, parser: "yaml" });
  } catch (error) {
    throw new PublicDocsError(`${path} is not valid YAML: ${error.message}`);
  }
}

function validateIssueForms(files) {
  for (const path of [
    ".github/ISSUE_TEMPLATE/bug_report.yml",
    ".github/ISSUE_TEMPLATE/feature_request.yml",
  ]) {
    const content = files.get(path);
    assertContains(content, "name:", path);
    assertContains(content, "description:", path);
    assertContains(content, "body:", path);
    assertContains(content, "required: true", path);
  }
  const config = files.get(".github/ISSUE_TEMPLATE/config.yml");
  assertContains(
    config,
    "blank_issues_enabled: false",
    ".github/ISSUE_TEMPLATE/config.yml",
  );
  assertContains(
    config,
    "/security/advisories/new",
    ".github/ISSUE_TEMPLATE/config.yml",
  );
}

/**
 * Reads and validates public documents: private-marker/path text rules and
 * Markdown links/fragments/alt text. THIRD_PARTY_NOTICES.md is exempt from the
 * prose marker scan but still checked for local paths and links.
 */
export async function validatePublicDocuments(projectRoot, paths) {
  const root = resolve(projectRoot);
  const entries = await Promise.all(
    paths.map(async (path) => [path, await readRequired(root, path)]),
  );
  const documents = new Map(entries);

  for (const [path, content] of documents) {
    validatePublicText(path, content, {
      skipForbiddenMarkers: MARKER_SCAN_EXEMPT_FILES.has(path),
    });
  }
  await Promise.all(
    [...documents]
      .filter(([path]) => path.endsWith(".md"))
      .map(([path, content]) => validateMarkdownLinks(root, path, content)),
  );
  return documents;
}

export async function checkPublicDocs(projectRoot = DEFAULT_PROJECT_ROOT) {
  const root = resolve(projectRoot);
  const { required, releaseNotes } = await resolvePublicDocumentPaths(root);
  const files = await validatePublicDocuments(root, [
    ...required,
    ...releaseNotes,
  ]);

  await Promise.all(
    YAML_FILES.map((path) => validateYaml(path, files.get(path))),
  );
  for (const path of SENSITIVE_WARNING_FILES) {
    validateSensitiveWarning(path, files.get(path));
  }
  validateIssueForms(files);
  const bilingualReleaseNotes = validateReleaseNoteLanguagePairs(files);

  const [packageJson, cargoToml, nvmrc, rustToolchain, tauriConfig] =
    await Promise.all([
      readRequired(root, "package.json").then(JSON.parse),
      readRequired(root, "Cargo.toml"),
      readRequired(root, ".nvmrc"),
      readRequired(root, "rust-toolchain.toml"),
      readRequired(root, "src-tauri/tauri.conf.json").then(JSON.parse),
    ]);
  if (
    packageJson.scripts?.["docs:check"] !== "node scripts/check-public-docs.mjs"
  ) {
    throw new PublicDocsError(
      "package.json docs:check does not invoke the public documentation checker.",
    );
  }
  if (
    nvmrc.trim() !== "22.22.3" ||
    packageJson.packageManager !== "pnpm@10.33.2" ||
    !rustToolchain.includes('channel = "1.97.1"') ||
    !cargoToml.includes('rust-version = "1.97.1"') ||
    tauriConfig.bundle?.macOS?.minimumSystemVersion !== "13.0"
  ) {
    throw new PublicDocsError(
      "Pinned Node, pnpm, Rust, or macOS metadata drifted from the public docs contract.",
    );
  }

  validateVersionIndependentProjectClaims(files);
  validateReadmeLanguageSwitches(files);
  validateDeliveryLifecycleContract(packageJson, files);
  validateReleaseInventoryContract(packageJson, files);
  assertContains(
    files.get("release-notes/README.md"),
    "`v<version>.md`",
    "release-notes/README.md",
  );
  const readmeLower = files.get("README.md").toLowerCase();
  for (const value of [
    "macOS 13",
    "Apple Silicon",
    "22.22.3",
    "10.33.2",
    "1.97.1",
    "codex-cli 0.162.0",
    "./LICENSE",
    "./THIRD_PARTY_NOTICES.md",
    ...REQUIRED_README_COMMANDS,
  ]) {
    assertContains(files.get("README.md"), value, "README.md");
  }
  for (const value of ["official dmg", "ad-hoc", "notariz", "in-app update"]) {
    if (!readmeLower.includes(value)) {
      throw new PublicDocsError(
        `README.md is missing required contract text: ${value}.`,
      );
    }
  }
  const contributing = files.get("CONTRIBUTING.md");
  for (const value of [
    "22.22.3",
    "10.33.2",
    "1.97.1",
    ...REQUIRED_CONTRIBUTING_COMMANDS,
  ]) {
    assertContains(contributing, value, "CONTRIBUTING.md");
  }
  const supportLower = files.get("SUPPORT.md").toLowerCase();
  for (const value of [
    "macOS 13",
    "Apple Silicon",
    "codex-cli 0.162.0",
  ]) {
    assertContains(files.get("SUPPORT.md"), value, "SUPPORT.md");
  }
  for (const value of ["official dmg", "ad-hoc", "notariz", "in-app update"]) {
    if (!supportLower.includes(value)) {
      throw new PublicDocsError(
        `SUPPORT.md is missing required contract text: ${value}.`,
      );
    }
  }
  return {
    markdownFiles: MARKDOWN_FILES.length + releaseNotes.length,
    requiredFiles: REQUIRED_FILES.length,
    releaseNotes: releaseNotes.length,
    bilingualReleaseNotes,
    yamlFiles: YAML_FILES.length,
  };
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  checkPublicDocs()
    .then((result) => {
      console.log(
        `Public documentation check passed: ${result.requiredFiles} required files, ${result.releaseNotes} release notes, ${result.bilingualReleaseNotes} bilingual pairs, ${result.markdownFiles} Markdown, ${result.yamlFiles} YAML.`,
      );
    })
    .catch((error) => {
      console.error(error instanceof Error ? error.message : String(error));
      process.exitCode = 1;
    });
}
