# Development and delivery lifecycle

AI Router uses the same delivery lifecycle for changes made with a task-planning tool and changes made
directly. Task records can provide requirements and verification evidence, but whether code has been
delivered and whether it enters a release candidate is decided only by the factual state of Git and
GitHub.

## Natural-language control

Use the following fixed phrases through the project AI entry point:

| Command                                                           | Result                                                                                                            |
| ----------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------- |
| `Start task: <item>`                                              | Create a focused branch from the latest `main`, select an appropriate planning path, and begin implementing.       |
| `Start task: <item>, without Trellis`                             | Create a focused branch and implement directly, without creating a task-planning record.                           |
| `Continue the current task`                                       | Resume the current branch, task, and verification context and continue unfinished work.                            |
| `Check the current task`                                          | Only review and verify; do not commit, push, create or modify a PR, merge, or create a tag.                        |
| `Deliver the current task`                                        | Review the full diff, organize logical commits, push the branch, and create or update a PR; do not merge the PR.   |
| `Park the current work; do not merge yet`                         | Commit and push, create or update a draft PR, and keep it out of `main`.                                           |
| `Bring the current task into main`                                | Check the PR and required checks, sync `main`, resolve conflicts, verify, and merge.                               |
| `Defer the current task`                                          | Keep a recoverable branch and PR, record the reason for the deferral, and do not enter `main`.                      |
| `Prepare the next release`                                        | Check open PRs, local branches, working-tree changes, and task evidence against the inventory from the stable tag to `main`. |
| `Include <item list> in <version>, defer everything else unmerged` | Verify the specified release scope against Git facts and report every item that cannot be proven.                  |
| `Release <version>`                                               | After release preparation, verify the target commit and create the stable tag that triggers the protected release pipeline. |
| `Show delivery status`                                            | Report local commit, push, PR, mainline, task, and release-candidate status.                                       |

Before pushing, merging, or creating a stable tag, the AI must report the target branch, commit, and
file scope. Read-only checks never perform these actions.

## State meanings

```text
local changes -> commit -> pushed branch -> PR -> merged into main -> stable tag
```

- A commit only means the change is saved in the local Git history.
- A push gives the remote branch a recoverable commit record.
- A PR only requests review and merge; it does not mean the code has entered `main`.
- A merge into `main` means the change is accepted into future release candidates.
- A stable tag identifies the exact commit officially delivered to users.

`main` only accepts changes that have completed the necessary verification and are confirmed ready for
future release candidates. Changes still in development or explicitly deferred stay on separate
branches and may keep an open draft PR. A task being marked complete does not prove that its branch
has been pushed or merged.

## Release scope

Before preparing a release, run:

```sh
pnpm delivery:status
pnpm release:inventory
```

Cross-check the output against open PRs, local branches, working-tree changes, and task evidence. Each
item must be marked `included`, `deferred`, `unconfirmed`, or `shipped with the code but not written
into the notes`. "Not written into the notes" means the code still ships with the release; "deferred"
requires the code to be absent from the release-candidate commit.

## Safety boundaries

Do not commit working-tree changes whose ownership you cannot confirm. When the GitHub CLI is
unresponsive, do not interpret it as an empty PR list. Create a stable tag only when the version
number, the reviewed release notes, required checks, and source-build evidence all point to the same
commit. Official assets and updater secrets are handled only by the protected release pipeline
described in [Stable release operations](./releasing.md).
