## Change

Explain the problem solved, the observable behavior, and anything out of scope.

## Delivery status

- Entry: Trellis task / direct edit
- Current state: in development / committed / pushed / PR created / awaiting merge
- Follow-up release: included / deferred / to be confirmed

## Risks

Describe the impact on data, routing, concurrency, recovery, Codex configuration, native lifecycle, or release boundaries; write "none" when there is no impact.

## Verification

List the commands actually run and their results, and explain why any checks were not run.

## Checklist

- [ ] The change is focused and does not overwrite unrelated workspace modifications.
- [ ] Tests/documentation have been added or updated in proportion to the behavioral risk.
- [ ] Generated files, lockfiles, versions, and third-party sources have been checked for required synchronization.
- [ ] No real API keys, Authorization headers, complete configuration (including Codex configuration), SQLite databases, raw requests/responses, raw logs, recovery points, or user absolute paths are submitted.
- [ ] Native automation targets only `AI Router QA.app` / `com.relax.airouter.qa` and does not control the production app.
- [ ] The PR does not read the release environment or updater private keys, and neither creates, publishes, nor modifies GitHub Releases.
- [ ] User-visible or support-boundary changes are reflected in the README, SUPPORT, or engineering documentation.
