import { execFile } from "node:child_process";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

import semver from "semver";

import { checkVersions, VersionError } from "./manage-version.mjs";

const MAX_GIT_OUTPUT_BYTES = 32 * 1024 * 1024;
const STABLE_TAG_PATTERN = /^v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/;
const FULL_OBJECT_ID_PATTERN = /^(?:[0-9a-f]{40}|[0-9a-f]{64})$/;

export class ReleaseInventoryError extends Error {}

function execute(command, args, options) {
  return new Promise((resolvePromise, rejectPromise) => {
    execFile(command, args, options, (error, stdout) => {
      if (error) {
        rejectPromise(error);
        return;
      }
      resolvePromise(stdout);
    });
  });
}

function assertSafeText(value, label) {
  if (
    typeof value !== "string" ||
    value.length === 0 ||
    value.includes("\0") ||
    value.includes("\n") ||
    value.includes("\r")
  ) {
    throw new ReleaseInventoryError(`${label} is malformed.`);
  }
  return value;
}

function parseObjectId(output, label) {
  const value = output.endsWith("\n") ? output.slice(0, -1) : output;
  assertSafeText(value, label);
  if (!FULL_OBJECT_ID_PATTERN.test(value)) {
    throw new ReleaseInventoryError(`${label} is not a full Git object ID.`);
  }
  return value;
}

function parseReachableRefs(output) {
  const records = output.endsWith("\n") ? output.slice(0, -1) : output;
  if (!records) return [];
  return records.split("\n").map((record) => {
    const fields = record.split("\0");
    if (fields.length !== 2) {
      throw new ReleaseInventoryError("Reachable tag metadata is malformed.");
    }
    return {
      objectType: assertSafeText(fields[1], "Reachable tag object type"),
      tag: assertSafeText(fields[0], "Reachable tag name"),
    };
  });
}

function stableVersion(tag) {
  const match = STABLE_TAG_PATTERN.exec(tag);
  if (!match) return undefined;
  const version = tag.slice(1);
  return semver.valid(version, { loose: false }) === version
    ? version
    : undefined;
}

export function selectStableBaseline(refs) {
  if (!Array.isArray(refs)) {
    throw new ReleaseInventoryError("Reachable tag metadata is malformed.");
  }

  const stableRefs = [];
  const seen = new Set();
  for (const ref of refs) {
    if (!ref || typeof ref !== "object") {
      throw new ReleaseInventoryError("Reachable tag metadata is malformed.");
    }
    const tag = assertSafeText(ref.tag, "Reachable tag name");
    const objectType = assertSafeText(
      ref.objectType,
      "Reachable tag object type",
    );
    const version = stableVersion(tag);
    if (!version) continue;
    if (objectType !== "tag") {
      throw new ReleaseInventoryError(
        `Stable baseline candidate ${tag} must be an annotated tag.`,
      );
    }
    if (seen.has(tag)) {
      throw new ReleaseInventoryError(`Stable tag ${tag} is duplicated.`);
    }
    seen.add(tag);
    stableRefs.push({ tag, version });
  }

  if (stableRefs.length === 0) {
    throw new ReleaseInventoryError(
      "No reachable annotated stable release tag was found.",
    );
  }
  stableRefs.sort((left, right) =>
    semver.rcompare(left.version, right.version),
  );
  return stableRefs[0];
}

function parseCommits(output) {
  const records = output.endsWith("\n") ? output.slice(0, -1) : output;
  if (!records) return [];
  const seen = new Set();
  return records.split("\n").map((record) => {
    const fields = record.split("\0");
    if (fields.length !== 2) {
      throw new ReleaseInventoryError("Commit metadata is malformed.");
    }
    const sha = parseObjectId(fields[0], "Commit object ID");
    const subject = assertSafeText(fields[1], "Commit subject");
    if (seen.has(sha)) {
      throw new ReleaseInventoryError(`Commit ${sha} is duplicated.`);
    }
    seen.add(sha);
    return { sha, subject };
  });
}

function parsePaths(output) {
  if (!output) return [];
  if (!output.endsWith("\0")) {
    throw new ReleaseInventoryError("Changed path metadata is malformed.");
  }
  const paths = output
    .slice(0, -1)
    .split("\0")
    .map((path) => assertSafeText(path, "Changed path"));
  if (new Set(paths).size !== paths.length) {
    throw new ReleaseInventoryError(
      "Changed path metadata contains duplicates.",
    );
  }
  return paths.sort((left, right) =>
    left < right ? -1 : left > right ? 1 : 0,
  );
}

async function runGit(runner, root, operation, args) {
  try {
    const output = await runner("git", args, {
      cwd: root,
      encoding: "utf8",
      maxBuffer: MAX_GIT_OUTPUT_BYTES,
    });
    if (typeof output !== "string") throw new Error("non-text output");
    return output;
  } catch {
    throw new ReleaseInventoryError(
      `Unable to read local Git metadata for ${operation}.`,
    );
  }
}

export async function collectReleaseInventory(
  root = process.cwd(),
  options = {},
) {
  const runner = options.runner ?? execute;
  const versionChecker = options.versionChecker ?? checkVersions;
  let candidateVersion;
  try {
    candidateVersion = await versionChecker(root);
  } catch (error) {
    if (error instanceof VersionError) throw error;
    throw new ReleaseInventoryError(
      "Unable to validate managed version projections.",
    );
  }
  const refs = parseReachableRefs(
    await runGit(runner, root, "reachable tags", [
      "for-each-ref",
      "--merged=HEAD",
      "--format=%(refname:strip=2)%00%(objecttype)",
      "refs/tags",
    ]),
  );
  const baseline = selectStableBaseline(refs);
  const baselineCommit = parseObjectId(
    await runGit(runner, root, "the baseline commit", [
      "rev-parse",
      `${baseline.tag}^{commit}`,
    ]),
    "Baseline commit",
  );
  const candidateCommit = parseObjectId(
    await runGit(runner, root, "the candidate commit", ["rev-parse", "HEAD"]),
    "Candidate commit",
  );
  const range = `${baseline.tag}..HEAD`;
  const commits = parseCommits(
    await runGit(runner, root, "candidate commits", [
      "log",
      "--reverse",
      "--format=%H%x00%s",
      range,
    ]),
  );
  if (commits.length === 0) {
    throw new ReleaseInventoryError(
      `Candidate range after ${baseline.tag} contains no commits.`,
    );
  }
  const paths = parsePaths(
    await runGit(runner, root, "changed paths", [
      "diff",
      "--name-only",
      "-z",
      range,
    ]),
  );
  if (paths.length === 0) {
    throw new ReleaseInventoryError(
      `Candidate range after ${baseline.tag} contains no changed paths.`,
    );
  }

  return {
    baselineCommit,
    baselineTag: baseline.tag,
    candidateCommit,
    candidateVersion,
    commits,
    paths,
  };
}

function escapeMarkdownText(value) {
  return value
    .replaceAll("&", "&amp;")
    .replace(/([\\`*_[\]{}()#+.!|<>~-])/g, "\\$1");
}

function inlineCode(value) {
  const longestRun = Math.max(
    0,
    ...[...value.matchAll(/`+/g)].map((match) => match[0].length),
  );
  const fence = "`".repeat(longestRun + 1);
  const pad = /^[` ]|[` ]$/.test(value) ? " " : "";
  return `${fence}${pad}${value}${pad}${fence}`;
}

export function renderReleaseInventory(inventory) {
  const baselineTag = assertSafeText(inventory?.baselineTag, "Baseline tag");
  const baselineCommit = parseObjectId(
    inventory?.baselineCommit,
    "Baseline commit",
  );
  const candidateCommit = parseObjectId(
    inventory?.candidateCommit,
    "Candidate commit",
  );
  const candidateVersion = assertSafeText(
    inventory?.candidateVersion,
    "Candidate version",
  );
  const commits = Array.isArray(inventory?.commits) ? inventory.commits : [];
  const paths = Array.isArray(inventory?.paths) ? inventory.paths : [];
  if (commits.length === 0 || paths.length === 0) {
    throw new ReleaseInventoryError(
      "Release inventory requires commits and changed paths.",
    );
  }

  const lines = [
    "# Release notes inventory",
    "",
    "> 此报告仅提供本地 Git 证据，不能自动证明版本说明在语义上完整。",
    "",
    `- 基线 tag：${inlineCode(baselineTag)}`,
    `- 基线 commit：${inlineCode(baselineCommit)}`,
    `- 候选 commit：${inlineCode(candidateCommit)}`,
    `- 候选版本：${inlineCode(candidateVersion)}`,
    "",
    `## Commits (${commits.length})`,
    "",
  ];
  for (const commit of commits) {
    const sha = parseObjectId(commit?.sha, "Commit object ID");
    const subject = assertSafeText(commit?.subject, "Commit subject");
    lines.push(`- [ ] ${inlineCode(sha)} ${escapeMarkdownText(subject)}`);
  }
  lines.push("", `## Changed paths (${paths.length})`, "");
  for (const path of paths) {
    lines.push(`- [ ] ${inlineCode(assertSafeText(path, "Changed path"))}`);
  }
  lines.push(
    "",
    "## 发布负责人核对",
    "",
    "- [ ] 已逐项审查以上 commit 和 changed path，并识别所有用户可见新增、行为变化与问题修复。",
    "- [ ] 已核对兼容性、迁移操作和安装后重启影响，并在需要时写入 `注意事项`。",
    "- [ ] 已与可用的 merged PR 和已完成任务证据交叉核对，补足仅凭 Git 元数据无法判断的产品意图。",
    "- [ ] 每项未写入版本说明的变化都有简明排除理由，例如测试、依赖或内部重构。",
    "- [ ] `重点更新` 已按用户价值排序，最终文案准确且不包含内部实现细节。",
    "- [ ] 发布负责人已对版本说明的准确性、重要性排序和完整性作最终批准。",
    "",
  );
  return lines.join("\n");
}

async function run() {
  if (process.argv.length !== 2) {
    throw new ReleaseInventoryError(
      "Usage: node scripts/release-inventory.mjs",
    );
  }
  const inventory = await collectReleaseInventory();
  process.stdout.write(renderReleaseInventory(inventory));
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
