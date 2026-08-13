import { execFile } from "node:child_process";
import { lstat, readFile } from "node:fs/promises";
import { dirname, extname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const SCRIPT_ROOT = dirname(fileURLToPath(import.meta.url));
const DEFAULT_PROJECT_ROOT = resolve(SCRIPT_ROOT, "..");
const TEXT_DECODER = new TextDecoder("utf-8", { fatal: true });
const MANDATORY_PRIVATE_PREFIXES = [
  ".agents/",
  ".claude/",
  ".codex/",
  ".trellis/",
];
const MANDATORY_PRIVATE_PATHS = [
  "design-qa.md",
  "scripts/public-snapshot-policy.json",
  "scripts/public-snapshot.mjs",
  "scripts/public-snapshot.test.mjs",
];
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
const SECRET_PATTERNS = [
  [
    "private-key",
    new RegExp(["-----BEGIN", "(?:[A-Z0-9]+ )?PRIVATE KEY-----"].join(" ")),
  ],
  ["openai-style-key", /\bsk-[A-Za-z0-9_-]{20,}\b/],
  ["github-token", /\bgh[pousr]_[A-Za-z0-9]{20,}\b/],
  ["aws-access-key", /\b(?:AKIA|ASIA)[A-Z0-9]{16}\b/],
  ["slack-token", /\bxox[baprs]-[A-Za-z0-9-]{20,}\b/],
  [
    "authorization-header",
    /\bauthorization\s*:\s*(?:bearer|basic)\s+[^\s${}]{12,}/i,
  ],
  ["npm-auth-token", /(?:^|\s)(?:\/\/[^\s]+:)?_authToken\s*=\s*[^\s${}]{12,}/i],
];
const LOCAL_PATH_PATTERNS = [
  ["macos-home", /(?:^|[\s"'(=>])\/Users\/[^/\s"']+\//],
  ["linux-home", /(?:^|[\s"'(=>])\/home\/[^/\s"']+\//],
  ["macos-temporary", /\/var\/folders\/[A-Za-z0-9_-]+\//],
  ["windows-home", /\b[A-Za-z]:\\Users\\[^\\\s"']+\\/],
  ["local-file-url", /\bfile:\/\/\/(?:Users|home)\//],
];

export class RepositorySecurityError extends Error {}

function executeGit(root, args) {
  return new Promise((resolvePromise, rejectPromise) => {
    execFile(
      "git",
      args,
      { cwd: root, encoding: "buffer", maxBuffer: 64 * 1024 * 1024 },
      (error, stdout, stderr) => {
        if (error) {
          rejectPromise(
            new RepositorySecurityError(
              `git ${args.join(" ")} failed: ${stderr.toString("utf8").trim()}`,
            ),
          );
        } else {
          resolvePromise(stdout);
        }
      },
    );
  });
}

function privatePath(path, policy) {
  return (
    MANDATORY_PRIVATE_PATHS.includes(path) ||
    MANDATORY_PRIVATE_PREFIXES.some((prefix) => path.startsWith(prefix)) ||
    policy.publicTree.forbiddenExactPaths.includes(path) ||
    policy.publicTree.forbiddenPathPrefixes.some((prefix) =>
      path.startsWith(prefix),
    )
  );
}

export function scanRepositoryText(path, content) {
  const findings = [];
  for (const [index, line] of content.split("\n").entries()) {
    for (const [code, pattern] of [
      ...SECRET_PATTERNS,
      ...LOCAL_PATH_PATTERNS,
    ]) {
      if (pattern.test(line)) findings.push({ code, line: index + 1, path });
    }
  }
  return findings;
}

async function repositoryPaths(root) {
  const output = await executeGit(root, [
    "ls-files",
    "--cached",
    "--others",
    "--exclude-standard",
    "-z",
  ]);
  return output.toString("utf8").split("\0").filter(Boolean).sort();
}

export async function checkRepositorySecurity(
  projectRoot = DEFAULT_PROJECT_ROOT,
  { rejectPrivatePaths = false } = {},
) {
  const root = resolve(projectRoot);
  const policy = JSON.parse(
    await readFile(resolve(root, "scripts/license-policy.json"), "utf8"),
  );
  const declaredAssets = new Set(policy.declaredVisualAssets);
  const findings = [];
  let scannedFiles = 0;
  for (const path of await repositoryPaths(root)) {
    if (privatePath(path, policy)) {
      if (rejectPrivatePaths) findings.push({ code: "private-path", path });
      continue;
    }
    const absolutePath = resolve(root, path);
    const metadata = await lstat(absolutePath);
    if (metadata.isSymbolicLink()) {
      findings.push({ code: "symlink", path });
      continue;
    }
    const content = await readFile(absolutePath);
    const extension = extname(path).toLowerCase();
    if (IMAGE_EXTENSIONS.has(extension)) {
      if (!declaredAssets.has(path)) {
        findings.push({ code: "undeclared-visual-asset", path });
      }
      if (extension === ".svg") {
        try {
          findings.push(
            ...scanRepositoryText(path, TEXT_DECODER.decode(content)),
          );
        } catch {
          findings.push({ code: "invalid-svg-text", path });
        }
      }
      scannedFiles += 1;
      continue;
    }
    let text;
    try {
      text = TEXT_DECODER.decode(content);
    } catch {
      findings.push({ code: "unreviewed-binary", path });
      continue;
    }
    findings.push(...scanRepositoryText(path, text));
    scannedFiles += 1;
  }
  if (findings.length > 0) {
    const summary = findings
      .slice(0, 20)
      .map(
        (finding) =>
          `${finding.path}${finding.line ? `:${finding.line}` : ""} [${finding.code}]`,
      )
      .join("\n");
    throw new RepositorySecurityError(
      `Tracked public source scan found ${findings.length} unresolved finding(s):\n${summary}`,
    );
  }
  return { declaredAssets: declaredAssets.size, scannedFiles };
}

function parseArguments(values) {
  if (values.length === 0) return { rejectPrivatePaths: false };
  if (values.length === 1 && values[0] === "--public-tree") {
    return { rejectPrivatePaths: true };
  }
  throw new RepositorySecurityError(
    "Usage: node scripts/check-repository-security.mjs [--public-tree]",
  );
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  let options;
  try {
    options = parseArguments(process.argv.slice(2));
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = 1;
  }
  if (options)
    checkRepositorySecurity(DEFAULT_PROJECT_ROOT, options)
      .then((result) => {
        console.log(
          `Repository security check passed: ${result.scannedFiles} public source files, ${result.declaredAssets} reviewed visual assets.`,
        );
      })
      .catch((error) => {
        console.error(error instanceof Error ? error.message : String(error));
        process.exitCode = 1;
      });
}
