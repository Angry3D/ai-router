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
non-documentation pushes to `main` and can be dispatched manually. Before the
first public release, retain one successful run for the final source revision;
the run summary records the revision, runner, tool versions, and source-build
command. The workflow uses `pnpm tauri:source:build`, which disables signing,
and does not upload the application bundle.

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

## Source-only release boundary

No first-release workflow receives signing, notarization, publishing, or
release secrets. CI does not upload an installer or application bundle, create
a GitHub Release, launch either app identity, read local application-support
data, or control `/Applications/AI Router.app`. A future binary distribution
workflow requires a separate security and provenance design.
