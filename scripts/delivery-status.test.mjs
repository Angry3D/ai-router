import { execFile } from "node:child_process";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { promisify } from "node:util";

import { afterEach, describe, expect, it } from "vitest";

import {
  collectDeliveryStatus,
  DeliveryStatusError,
  renderDeliveryStatus,
} from "./delivery-status.mjs";

const execute = promisify(execFile);
const temporaryRoots = [];

function runnerFrom(map) {
  return async (_command, args) => {
    const key = args.join(" ");
    if (!(key in map)) throw new Error(`missing fixture: ${key}`);
    if (map[key] instanceof Error) throw map[key];
    return map[key];
  };
}

async function git(root, args) {
  await execute("git", args, { cwd: root });
}

afterEach(async () => {
  await Promise.all(
    temporaryRoots.splice(0).map((root) => rm(root, { recursive: true })),
  );
});

describe("delivery status", () => {
  it("collects local Git state and task records without GitHub", async () => {
    const status = await collectDeliveryStatus("/project", {
      includePullRequests: false,
      runner: runnerFrom({
        "branch --show-current": "codex/example\n",
        "rev-parse HEAD": "abc123\n",
        "status --porcelain=v1 -z": " M src/file.ts\0?? notes.md\0",
        "rev-parse --abbrev-ref --symbolic-full-name @{u}":
          "origin/codex/example\n",
        "rev-list --left-right --count @{u}...HEAD": "2\t1\n",
        "for-each-ref --format=%(refname:short)%09%(objectname:short)%09%(upstream:short)%09%(upstream:track) refs/heads":
          "main\teb3810b\torigin/main\t\ncodex/example\tabc123\torigin/codex/example\t[ahead 1, behind 2]\n",
      }),
    });

    expect(status.branch).toBe("codex/example");
    expect(status.divergence).toEqual({ ahead: 1, behind: 2 });
    expect(status.status).toEqual([
      { index: " M", path: "src/file.ts" },
      { index: "??", path: "notes.md" },
    ]);
    expect(status.branches).toHaveLength(2);
    expect(status.branches[1].tracking).toBe("[ahead 1, behind 2]");
    expect(status.pullRequests).toEqual([]);
  });

  it("reads rename, task, and unpushed state from a temporary Git repository", async () => {
    const root = await mkdtemp(join(tmpdir(), "ai-router-delivery-status-"));
    const remote = await mkdtemp(join(tmpdir(), "ai-router-delivery-remote-"));
    temporaryRoots.push(root, remote);

    await git(root, ["init", "--initial-branch=main"]);
    await git(root, ["config", "user.name", "Fixture"]);
    await git(root, ["config", "user.email", "fixture@example.invalid"]);
    await writeFile(join(root, "old.txt"), "old\n", "utf8");
    await mkdir(join(root, ".trellis/tasks/example"), { recursive: true });
    await writeFile(
      join(root, ".trellis/tasks/example/task.json"),
      JSON.stringify({
        id: "example",
        status: "deferred",
        branch: "main",
        commit: "",
      }),
      "utf8",
    );
    await git(root, ["add", "."]);
    await git(root, ["commit", "-m", "initial"]);
    await git(remote, ["init", "--bare"]);
    await git(root, ["remote", "add", "origin", remote]);
    await git(root, ["push", "--set-upstream", "origin", "main"]);

    await writeFile(join(root, "ahead.txt"), "ahead\n", "utf8");
    await git(root, ["add", "ahead.txt"]);
    await git(root, ["commit", "-m", "ahead"]);
    await git(root, ["mv", "old.txt", "renamed.txt"]);
    await writeFile(join(root, "untracked.txt"), "local\n", "utf8");

    const status = await collectDeliveryStatus(root, {
      includePullRequests: false,
    });

    expect(status.branch).toBe("main");
    expect(status.upstream).toBe("origin/main");
    expect(status.divergence).toEqual({ ahead: 1, behind: 0 });
    expect(status.status).toEqual([
      {
        index: "R ",
        originalPath: "old.txt",
        path: "renamed.txt",
      },
      { index: "??", path: "untracked.txt" },
    ]);
    expect(status.branches).toEqual([
      expect.objectContaining({
        name: "main",
        tracking: "[ahead 1]",
        upstream: "origin/main",
      }),
    ]);
    expect(status.tasks).toEqual([
      { branch: "main", commit: "", id: "example", status: "deferred" },
    ]);
  });

  it("preserves a known upstream when divergence cannot be read", async () => {
    const status = await collectDeliveryStatus("/project", {
      includePullRequests: false,
      runner: runnerFrom({
        "branch --show-current": "codex/example\n",
        "rev-parse HEAD": "abc123\n",
        "status --porcelain=v1 -z": "",
        "rev-parse --abbrev-ref --symbolic-full-name @{u}":
          "origin/codex/example\n",
        "rev-list --left-right --count @{u}...HEAD": new Error("failed"),
        "for-each-ref --format=%(refname:short)%09%(objectname:short)%09%(upstream:short)%09%(upstream:track) refs/heads":
          "codex/example\tabc123\torigin/codex/example\t\n",
      }),
    });

    expect(status.upstream).toBe("origin/codex/example");
    expect(status.divergence).toBeNull();
  });

  it("renders task and delivery boundaries", () => {
    const output = renderDeliveryStatus({
      branch: "codex/example",
      head: "abc123",
      upstream: "origin/codex/example",
      divergence: { ahead: 1, behind: 0 },
      status: [],
      branches: [
        {
          name: "codex/example",
          sha: "abc123",
          upstream: "origin/codex/example",
          tracking: "[ahead 1]",
        },
      ],
      pullRequests: [
        {
          number: 4,
          title: "Title with `code`\nand a newline",
          state: "OPEN",
          isDraft: false,
          headRefName: "codex/example",
          baseRefName: "main",
        },
      ],
      tasks: [
        {
          id: "example",
          status: "completed",
          branch: "codex/example",
          commit: "abc123",
        },
      ],
    });

    expect(output).toContain("当前分支：`codex/example`");
    expect(output).toContain(
      "`example`: completed on `codex/example` at `abc123`",
    );
    expect(output).toContain("`[ahead 1]`");
    expect(output).toContain("## Pull requests (1)");
    expect(output).toContain("\\u000a");
  });

  it("combines open PRs with the current branch's merged PR", async () => {
    const openPullRequest = {
      number: 8,
      title: "Open work",
      headRefName: "codex/open",
      baseRefName: "main",
      state: "OPEN",
      isDraft: false,
      url: "https://example.invalid/pull/8",
      mergedAt: null,
    };
    const mergedPullRequest = {
      number: 7,
      title: "Current work",
      headRefName: "codex/example",
      baseRefName: "main",
      state: "MERGED",
      isDraft: false,
      url: "https://example.invalid/pull/7",
      mergedAt: "2026-09-12T00:00:00Z",
    };
    const status = await collectDeliveryStatus("/project", {
      runner: runnerFrom({
        "branch --show-current": "codex/example\n",
        "rev-parse HEAD": "abc123\n",
        "status --porcelain=v1 -z": "",
        "for-each-ref --format=%(refname:short)%09%(objectname:short)%09%(upstream:short)%09%(upstream:track) refs/heads":
          "codex/example\tabc123\t\t\n",
        "pr list --state open --limit 1000 --json number,title,headRefName,baseRefName,state,isDraft,url,mergedAt":
          JSON.stringify([openPullRequest]),
        "pr list --head codex/example --state all --limit 1000 --json number,title,headRefName,baseRefName,state,isDraft,url,mergedAt":
          JSON.stringify([mergedPullRequest]),
      }),
    });

    expect(status.pullRequests).toEqual([mergedPullRequest, openPullRequest]);
  });

  it("fails instead of treating a GitHub query error as an empty PR list", async () => {
    await expect(
      collectDeliveryStatus("/project", {
        runner: runnerFrom({
          "branch --show-current": "main\n",
          "rev-parse HEAD": "abc123\n",
          "status --porcelain=v1 -z": "",
          "for-each-ref --format=%(refname:short)%09%(objectname:short)%09%(upstream:short)%09%(upstream:track) refs/heads":
            "main\tabc123\t\t\n",
          "pr list --state open --limit 1000 --json number,title,headRefName,baseRefName,state,isDraft,url,mergedAt":
            new Error("offline"),
          "pr list --head main --state all --limit 1000 --json number,title,headRefName,baseRefName,state,isDraft,url,mergedAt":
            "[]",
        }),
      }),
    ).rejects.toThrow(DeliveryStatusError);
  });

  it("does not expose a failing Git command's local path", async () => {
    const root = await mkdtemp(join(tmpdir(), "ai-router-delivery-error-"));
    temporaryRoots.push(root);
    const missing = join(root, "missing");

    let failure;
    try {
      await collectDeliveryStatus(missing, { includePullRequests: false });
    } catch (error) {
      failure = error;
    }

    expect(failure).toBeInstanceOf(DeliveryStatusError);
    expect(failure.message).not.toContain(root);
    expect(failure.message).toContain("Unable to read git branch");
  });
});
