import { execFile } from "node:child_process";
import { readFile, readdir } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const SCRIPT_ROOT = dirname(fileURLToPath(import.meta.url));
const DEFAULT_ROOT = resolve(SCRIPT_ROOT, "..");
const MAX_OUTPUT = 8 * 1024 * 1024;
const PULL_REQUEST_LIMIT = "1000";

export class DeliveryStatusError extends Error {}

function execute(command, args, options) {
  return new Promise((resolvePromise, rejectPromise) => {
    execFile(command, args, options, (error, stdout) => {
      if (error) {
        rejectPromise(error);
        return;
      }
      resolvePromise(stdout.toString("utf8"));
    });
  });
}

async function run(runner, command, args, cwd) {
  try {
    return await runner(command, args, {
      cwd,
      encoding: "buffer",
      maxBuffer: MAX_OUTPUT,
    });
  } catch (error) {
    if (error instanceof DeliveryStatusError) throw error;
    throw new DeliveryStatusError(
      `Unable to read ${command} ${args.join(" ")} from the local project.`,
    );
  }
}

function trim(value) {
  return value.replace(/\r?\n$/, "");
}

function parseStatus(output) {
  const records = output.split("\0");
  if (records.at(-1) === "") records.pop();
  const status = [];
  for (let index = 0; index < records.length; index += 1) {
    const record = records[index];
    if (record.length < 4 || record[2] !== " ") {
      throw new DeliveryStatusError(
        "Git worktree status metadata is malformed.",
      );
    }
    const state = record.slice(0, 2);
    const item = { index: state, path: record.slice(3) };
    if (state.includes("R") || state.includes("C")) {
      const originalPath = records[index + 1];
      if (!originalPath) {
        throw new DeliveryStatusError(
          "Git rename status metadata is malformed.",
        );
      }
      item.originalPath = originalPath;
      index += 1;
    }
    status.push(item);
  }
  return status;
}

function parseAheadBehind(output) {
  const values = trim(output).trim().split(/\s+/).map(Number);
  if (
    values.length !== 2 ||
    values.some((value) => !Number.isInteger(value) || value < 0)
  ) {
    throw new DeliveryStatusError(
      "Git upstream divergence metadata is malformed.",
    );
  }
  const [behind, ahead] = values;
  return { ahead, behind };
}

function parseBranches(output) {
  return trim(output)
    .split("\n")
    .filter(Boolean)
    .map((line) => {
      const fields = line.split("\t");
      if (fields.length !== 4) {
        throw new DeliveryStatusError(
          "Git local branch metadata is malformed.",
        );
      }
      const [name, sha, upstream, tracking] = fields;
      return { name, sha, upstream, tracking };
    });
}

async function readTaskRecords(root) {
  const tasksRoot = resolve(root, ".trellis/tasks");
  const records = [];

  async function visit(directory) {
    let entries;
    try {
      entries = await readdir(directory, { withFileTypes: true });
    } catch {
      return;
    }
    for (const entry of entries) {
      const path = resolve(directory, entry.name);
      if (entry.isDirectory()) {
        await visit(path);
      } else if (entry.isFile() && entry.name === "task.json") {
        try {
          const task = JSON.parse(await readFile(path, "utf8"));
          if (task && typeof task === "object") {
            records.push({
              id: typeof task.id === "string" ? task.id : entry.name,
              status: typeof task.status === "string" ? task.status : "unknown",
              branch: typeof task.branch === "string" ? task.branch : "",
              commit: typeof task.commit === "string" ? task.commit : "",
            });
          }
        } catch {
          records.push({
            id: entry.name,
            status: "invalid",
            branch: "",
            commit: "",
          });
        }
      }
    }
  }

  await visit(tasksRoot);
  return records.sort((left, right) => left.id.localeCompare(right.id));
}

function parsePullRequests(output) {
  const parsed = JSON.parse(output);
  if (!Array.isArray(parsed)) throw new Error("not an array");
  for (const pullRequest of parsed) {
    if (
      !Number.isInteger(pullRequest?.number) ||
      pullRequest.number <= 0 ||
      typeof pullRequest.title !== "string" ||
      typeof pullRequest.headRefName !== "string" ||
      typeof pullRequest.baseRefName !== "string" ||
      typeof pullRequest.state !== "string" ||
      typeof pullRequest.isDraft !== "boolean" ||
      typeof pullRequest.url !== "string" ||
      !(
        pullRequest.mergedAt === null ||
        typeof pullRequest.mergedAt === "string"
      )
    ) {
      throw new Error("invalid pull request record");
    }
  }
  return parsed;
}

async function queryPullRequests(root, runner, args) {
  return parsePullRequests(
    await run(
      runner,
      "gh",
      [
        "pr",
        "list",
        ...args,
        "--json",
        "number,title,headRefName,baseRefName,state,isDraft,url,mergedAt",
      ],
      root,
    ),
  );
}

async function readPullRequests(root, runner, branch) {
  try {
    const [open, currentBranch] = await Promise.all([
      queryPullRequests(root, runner, [
        "--state",
        "open",
        "--limit",
        PULL_REQUEST_LIMIT,
      ]),
      queryPullRequests(root, runner, [
        "--head",
        branch,
        "--state",
        "all",
        "--limit",
        PULL_REQUEST_LIMIT,
      ]),
    ]);
    const combined = new Map();
    for (const pullRequest of [...open, ...currentBranch]) {
      combined.set(pullRequest.number, pullRequest);
    }
    return [...combined.values()].sort(
      (left, right) => left.number - right.number,
    );
  } catch (error) {
    if (error instanceof DeliveryStatusError) {
      throw new DeliveryStatusError(
        "Unable to query GitHub pull requests; no empty PR result was inferred.",
      );
    }
    throw new DeliveryStatusError(
      "GitHub pull request metadata is malformed; no empty PR result was inferred.",
    );
  }
}

export async function collectDeliveryStatus(
  root = DEFAULT_ROOT,
  { runner = execute, includePullRequests = true } = {},
) {
  const projectRoot = resolve(root);
  const branch = trim(
    await run(runner, "git", ["branch", "--show-current"], projectRoot),
  );
  if (!branch) {
    throw new DeliveryStatusError(
      "The current worktree is detached or has no branch.",
    );
  }
  const head = trim(
    await run(runner, "git", ["rev-parse", "HEAD"], projectRoot),
  );
  const status = parseStatus(
    await run(runner, "git", ["status", "--porcelain=v1", "-z"], projectRoot),
  );
  let upstream = "";
  let divergence = null;
  try {
    upstream = trim(
      await run(
        runner,
        "git",
        ["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"],
        projectRoot,
      ),
    );
  } catch {
    upstream = "";
  }
  if (upstream) {
    try {
      divergence = parseAheadBehind(
        await run(
          runner,
          "git",
          ["rev-list", "--left-right", "--count", "@{u}...HEAD"],
          projectRoot,
        ),
      );
    } catch {
      divergence = null;
    }
  }
  const branches = parseBranches(
    await run(
      runner,
      "git",
      [
        "for-each-ref",
        "--format=%(refname:short)%09%(objectname:short)%09%(upstream:short)%09%(upstream:track)",
        "refs/heads",
      ],
      projectRoot,
    ),
  );
  const tasks = await readTaskRecords(projectRoot);
  const pullRequests = includePullRequests
    ? await readPullRequests(projectRoot, runner, branch)
    : [];
  return {
    branch,
    head,
    status,
    upstream,
    divergence,
    branches,
    tasks,
    pullRequests,
  };
}

function inline(value) {
  const text = [...String(value)]
    .map((character) => {
      const codePoint = character.codePointAt(0);
      return codePoint <= 31 || codePoint === 127
        ? `\\u${codePoint.toString(16).padStart(4, "0")}`
        : character;
    })
    .join("");
  const longestRun = Math.max(
    0,
    ...[...text.matchAll(/`+/g)].map((match) => match[0].length),
  );
  const fence = "`".repeat(longestRun + 1);
  const padding = /^[` ]|[` ]$/.test(text) ? " " : "";
  return `${fence}${padding}${text}${padding}${fence}`;
}

export function renderDeliveryStatus(status) {
  const lines = [
    "# Delivery status",
    "",
    `- 当前分支：${inline(status.branch)}`,
    `- 当前提交：${inline(status.head)}`,
    `- 远端跟踪：${status.upstream ? inline(status.upstream) : "未配置"}`,
    `- 分支差异：${status.divergence ? `ahead ${status.divergence.ahead}, behind ${status.divergence.behind}` : "无法确认"}`,
    "",
    `## Worktree (${status.status.length})`,
    "",
  ];
  if (status.status.length === 0) lines.push("- clean");
  for (const item of status.status) {
    lines.push(
      `- [${item.index}] ${inline(item.path)}${item.originalPath ? ` from ${inline(item.originalPath)}` : ""}`,
    );
  }
  lines.push("", `## Local branches (${status.branches.length})`, "");
  for (const branch of status.branches) {
    lines.push(
      `- ${inline(branch.name)} ${inline(branch.sha)}${branch.upstream ? ` -> ${inline(branch.upstream)}` : ""}${branch.tracking ? ` ${inline(branch.tracking)}` : ""}`,
    );
  }
  lines.push("", `## Pull requests (${status.pullRequests.length})`, "");
  if (status.pullRequests.length === 0) lines.push("- none");
  for (const pr of status.pullRequests) {
    lines.push(
      `- #${pr.number} ${inline(pr.title)} (${pr.state}${pr.isDraft ? ", draft" : ""}) ${inline(pr.headRefName)} -> ${inline(pr.baseRefName)}`,
    );
  }
  lines.push("", `## Trellis records (${status.tasks.length})`, "");
  if (status.tasks.length === 0) lines.push("- none");
  for (const task of status.tasks) {
    lines.push(
      `- ${inline(task.id)}: ${task.status}${task.branch ? ` on ${inline(task.branch)}` : ""}${task.commit ? ` at ${inline(task.commit)}` : ""}`,
    );
  }
  return `${lines.join("\n")}\n`;
}

async function main() {
  const args = process.argv.slice(2);
  if (args.some((arg) => !["--json", "--no-github"].includes(arg))) {
    throw new DeliveryStatusError(
      "Usage: node scripts/delivery-status.mjs [--json] [--no-github]",
    );
  }
  const status = await collectDeliveryStatus(DEFAULT_ROOT, {
    includePullRequests: !args.includes("--no-github"),
  });
  process.stdout.write(
    args.includes("--json")
      ? `${JSON.stringify(status, null, 2)}\n`
      : renderDeliveryStatus(status),
  );
}

if (
  process.argv[1] &&
  resolve(process.argv[1]) === resolve(fileURLToPath(import.meta.url))
) {
  main().catch((error) => {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = 1;
  });
}
