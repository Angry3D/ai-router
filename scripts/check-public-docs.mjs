import { access, readFile } from "node:fs/promises";
import { dirname, isAbsolute, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

import { format } from "prettier";

const SCRIPT_ROOT = dirname(fileURLToPath(import.meta.url));
const DEFAULT_PROJECT_ROOT = resolve(SCRIPT_ROOT, "..");

const REQUIRED_FILES = [
  "README.md",
  "AGENTS.md",
  "CONTRIBUTING.md",
  "SECURITY.md",
  "CODE_OF_CONDUCT.md",
  "SUPPORT.md",
  "docs/engineering/README.md",
  "docs/engineering/architecture.md",
  "docs/engineering/routing-resilience.md",
  "docs/engineering/data-privacy-recovery.md",
  "docs/engineering/native-lifecycle.md",
  "docs/engineering/application-updates.md",
  "docs/engineering/releasing.md",
  "docs/engineering/verification.md",
  "docs/engineering/github-security-settings.md",
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

function assertContains(content, expected, path) {
  if (!content.includes(expected)) {
    throw new PublicDocsError(
      `${path} is missing required contract text: ${expected}.`,
    );
  }
}

export function validatePublicText(path, content) {
  for (const marker of FORBIDDEN_TEXT_MARKERS) {
    if (content.includes(marker)) {
      throw new PublicDocsError(`${path} contains a private workflow marker.`);
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

export async function validateMarkdownLinks(projectRoot, path, content) {
  const documentRoot = dirname(resolve(projectRoot, path));
  const root = resolve(projectRoot);
  for (const target of markdownTargets(content)) {
    if (/^(?:https?:|mailto:)/.test(target) || target.startsWith("#")) continue;
    const fileTarget = target.split("#", 1)[0].split("?", 1)[0];
    if (!fileTarget) continue;
    let decoded;
    try {
      decoded = decodeURIComponent(fileTarget);
    } catch {
      throw new PublicDocsError(
        `${path} contains an invalid encoded link: ${target}.`,
      );
    }
    if (isAbsolute(decoded)) {
      throw new PublicDocsError(
        `${path} contains an absolute local link: ${target}.`,
      );
    }
    const destination = resolve(documentRoot, decoded);
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
    try {
      await access(destination);
    } catch {
      throw new PublicDocsError(
        `${path} contains a broken relative link: ${target}.`,
      );
    }
  }
}

export function validateSensitiveWarning(path, content) {
  for (const term of ["API Key", "完整配置", "数据库", "原始日志"]) {
    if (!content.includes(term)) {
      throw new PublicDocsError(
        `${path} does not warn against sharing ${term}.`,
      );
    }
  }
}

export function validateVersionIndependentProjectClaims(files) {
  for (const [path, value] of [
    ["README.md", "项目仍处于早期开发阶段"],
    ["CONTRIBUTING.md", "项目仍处于早期阶段"],
    ["SUPPORT.md", "AI Router 是早期个人维护项目"],
    ["docs/engineering/README.md", "描述当前产品和发布边界"],
  ]) {
    assertContains(files.get(path), value, path);
  }
  assertContains(files.get("README.md"), "GitHub Releases", "README.md");
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

export async function checkPublicDocs(projectRoot = DEFAULT_PROJECT_ROOT) {
  const root = resolve(projectRoot);
  const entries = await Promise.all(
    REQUIRED_FILES.map(async (path) => [path, await readRequired(root, path)]),
  );
  const files = new Map(entries);

  for (const [path, content] of files) validatePublicText(path, content);
  await Promise.all(
    MARKDOWN_FILES.map((path) =>
      validateMarkdownLinks(root, path, files.get(path)),
    ),
  );
  await Promise.all(
    YAML_FILES.map((path) => validateYaml(path, files.get(path))),
  );
  for (const path of SENSITIVE_WARNING_FILES) {
    validateSensitiveWarning(path, files.get(path));
  }
  validateIssueForms(files);

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
  const readme = files.get("README.md");
  for (const value of [
    "macOS 13",
    "Apple Silicon",
    "22.22.3",
    "10.33.2",
    "1.97.1",
    "codex-cli 0.147.0",
    "官方 DMG",
    "ad-hoc",
    "Apple 公证",
    "应用内更新",
    "./LICENSE",
    "./THIRD_PARTY_NOTICES.md",
    ...REQUIRED_README_COMMANDS,
  ]) {
    assertContains(readme, value, "README.md");
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
  const support = files.get("SUPPORT.md");
  for (const value of [
    "官方 DMG",
    "ad-hoc",
    "Apple 公证",
    "应用内更新",
    "macOS 13",
    "Apple Silicon",
  ]) {
    assertContains(support, value, "SUPPORT.md");
  }
  return {
    markdownFiles: MARKDOWN_FILES.length,
    requiredFiles: REQUIRED_FILES.length,
    yamlFiles: YAML_FILES.length,
  };
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  checkPublicDocs()
    .then((result) => {
      console.log(
        `Public documentation check passed: ${result.requiredFiles} files, ${result.markdownFiles} Markdown, ${result.yamlFiles} YAML.`,
      );
    })
    .catch((error) => {
      console.error(error instanceof Error ? error.message : String(error));
      process.exitCode = 1;
    });
}
