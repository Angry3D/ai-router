# GitHub CI and security settings

This document is the handoff contract for configuring the public GitHub
repository. Workflow and job names are external interfaces once a ruleset
references them. Update this document and the ruleset together before renaming
one.

## Required pull-request checks

Require these exact checks before `main` can be updated:

- `Required / Node quality`
- `Required / Rust quality`
- `Required / Generated and contracts`
- `Required / Protocol compatibility`
- `Required / Repository policy`
- `Security / Dependency review`
- `Security / CodeQL`

`Required / Protocol compatibility` uses the reviewed Intel build of the
pinned Codex fixture. This is test coverage only and does not add Intel Mac to
the supported product matrix; Rust quality and native source builds run on the
Apple Silicon runner.

Do not require `Native / Source build` for every pull request. It runs for
non-documentation pushes to `main` and can be dispatched manually. Retain one
successful run for each release source revision; the summary records the
revision, runner, tool versions, and source-build command. The workflow uses
`pnpm tauri:source:build`, which disables signing and updater artifacts, and
does not upload the application bundle.

## Repository ruleset

Configure a `main` branch ruleset with the following settings:

- require a pull request and the seven checks above;
- require the branch to be current before merge;
- block force pushes and branch deletion;
- require conversation resolution;
- allow no broad bypass; record and periodically review any maintainer bypass;
- use squash or rebase merge so the public history remains linear.

GitHub treats a Dependabot-authored squash commit as untrusted on the subsequent
`main` push, which can downgrade CodeQL's upload token and produce a 403. Verify
the Dependabot merge path in staging; if the push check is affected, use a merge
commit for that PR rather than weakening CodeQL permissions or switching to
`pull_request_target`.

Stage the ruleset while the repository is private. Open a dependency-changing
test pull request and verify that dependency review appears before making the
ruleset active. Check names are not considered proven until GitHub reports them
on the target repository.

## Actions and security features

Set the default `GITHUB_TOKEN` permission to read-only and do not allow Actions
to create or approve pull requests. Allow only actions required by the pinned
workflows. Every external action reference must remain a full commit SHA with a
version comment; Dependabot keeps the GitHub Actions ecosystem visible for
reviewed updates. Every checkout sets `persist-credentials: false`, so the
workflow token is not retained in Git configuration while repository-controlled
commands run.

Enable these repository features before public launch:

- dependency graph, Dependabot alerts, and Dependabot security updates;
- secret scanning, push protection, and validity checks when available;
- private vulnerability reporting and the security advisory workflow;
- CodeQL default or advanced setup, using the committed advanced workflow as
  the single JavaScript/TypeScript configuration;
- branch protection notifications and ruleset audit visibility.

Dependency review needs the dependency graph. On a private repository it also
requires the applicable GitHub Code Security entitlement; without that
entitlement, validate it after public visibility or in an entitled private
staging repository. A transient service or network failure is a failed gate,
not evidence that dependencies or licenses passed.

The CodeQL job grants only `contents: read`, `actions: read`, `packages: read`,
and `security-events: write`; the latter three are isolated to that job. CodeQL
advanced setup on a private repository also requires the applicable GitHub Code
Security entitlement. If the personal private staging repository does not have
it, validate the five non-Code-Security checks there and validate both dependency
review and CodeQL immediately after public visibility (or in an entitled private
staging repository) before making the seven-check ruleset active.

## Protected stable releases

Create a `v*` tag ruleset that blocks tag deletion, force updates, and moving an
existing tag. Restrict tag creation to maintainers after the source commit has
passed the `main` ruleset. Stable tags use exact `vMAJOR.MINOR.PATCH`; the
workflow additionally rejects prerelease/build metadata and any mismatch among
tag, ref, version, checkout, and target commit.

Create an environment named `release` with required reviewers and deployment
branch/tag rules limited to protected `v*` tags. Store exactly these environment
secrets:

- `AI_ROUTER_UPDATER_PUBLIC_KEY`
- `TAURI_SIGNING_PRIVATE_KEY`
- `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`

The public key is not confidential, but keeping all three values at the same
reviewed environment boundary prevents source/QA builds from accidentally
becoming official updater builds. Keep a separately encrypted and tested
offline private-key backup. No repository, organization, or pull-request secret
with the same names is part of the supported release path.

`.github/workflows/release.yml` is push-tag-only. It defaults to
`contents: read`; its single protected job receives only `contents: write`,
`id-token: write`, and `attestations: write`. Checkout credentials remain
disabled. The job creates or repairs only an unpublished draft, uploads and
downloads the complete five-asset inventory, attests the verified bytes, and
publishes only as its final step. A published release or changed tag/commit is
immutable and fails closed; corrections use a higher patch version.

Ordinary CI, source-build, and security workflows keep their no-secret and
no-publication policy. They do not upload an installer or application bundle,
create a GitHub Release, launch either app identity, read local
application-support data, or control `/Applications/AI Router.app`. The release
job also builds only inside the workspace and never installs, launches, quits,
or replaces the production application.

Run `pnpm ci:policy` after every workflow or release-command change. The checker
validates ordinary workflows separately from the exact allowlisted release
workflow, including triggers, environment, permissions, secrets, action pins,
commands, attestation path, and draft-publication order.
