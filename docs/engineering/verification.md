# Verification strategy by risk

The goal of verification is to prove the behavior this change alters, not to run every command
unconditionally. First list the changed files, observable results, cross-layer contracts, and evidence
still missing, then choose the smallest command that closes the evidence gap.

| Change scope                                                                  | Minimum sufficient evidence                                                                                                                              |
| ----------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Docs, issue/PR templates                                                      | `pnpm docs:check`, `git diff --check`                                                                                                                    |
| A single React component/utility                                              | The matching Vitest, `pnpm typecheck`, `pnpm lint`                                                                                                       |
| A single Rust module                                                          | The matching `cargo test` filter, `cargo fmt --check`, Clippy for the affected crate                                                                     |
| Rust/TypeScript DTO                                                           | producer/consumer tests, `pnpm generate:types`, frontend and backend static checks                                                                       |
| Codex credentials (cookie decryption and `auth.json` writing)                 | Matching Rust module tests, frontend settings tests, `pnpm generate:types`, frontend and backend static checks; real Chrome and keychain authorization belong to manual QA |
| Web entry or bundler                                                          | Full frontend test suite, `pnpm build`                                                                                                                   |
| schema, recovery, routing concurrency, SSE, native lifecycle, release         | Full domain tests and workspace checks for the affected area; the QA `.app` when necessary                                                               |

Run a focused frontend test directly:

```sh
pnpm exec vitest run <test-file>
```

Focused Rust examples:

```sh
cargo test -p router-core <test-filter>
cargo clippy -p router-core --all-targets -- -D warnings
cargo fmt --check
```

Cross-layer or high-risk baseline:

```sh
pnpm lint
pnpm typecheck
pnpm test
pnpm build
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Domain evidence

- Database/schema: migrations, integrity, foreign keys, permissions, symlink/unsafe paths, transaction
  rollback, and legacy data compatibility.
- Recovery: sanitized round trip, sensitive/non-critical table boundaries, retention policy, a corrupt
  main database, publication/rollback failure.
- Routing/fallback: immutable snapshots, the response-commit boundary, retry classification, the
  attempt cap, concurrent generations, persisted activation.
- SSE: first output, tokens, terminal state, a disconnect before/after termination, and bounded errors.
- Outbound proxy: HTTP/HTTPS/SOCKS under a synthetic system configuration, native bypass rules,
  custom-over-system precedence, environment isolation, no direct connection on failure,
  loopback-redirect escape prevention, and policy switching/in-flight snapshots across update and
  download. Native reads/notifications are accepted only in isolated QA and do not modify workstation
  proxy configuration; the updater uses a temporary signing key with local metadata/archives to verify
  a valid signature and the rejection of a bad signature, without installing.
- Images MCP: base64/URL source precedence, system/custom-proxy asset GET, credential isolation,
  per-hop routing and the redirect/download budget, full PNG validation, the shared permit during
  cancellation, the six-field error status source, and the eight-field success compatibility and HTTP
  byte passthrough. Network tests use only synthetic responses and loopback services; do not restore
  the removed URL/DNS/public-address/remote-peer admission restrictions.
- Codex configuration: immutable baseline, provider identity, preservation of unrelated fields,
  symlinks, fingerprint races, and exact restoration.
- Codex credentials: a `v10` decryption round trip using a synthetic cookie database built with test
  keys (PBKDF2-SHA1/AES-128-CBC and the host-hash prefix check), rejection of unknown prefixes, the
  three kinds of fail-closed browser/config discovery, `auth.json` structure validation (field set,
  the three-part `id_token`, the `account_id`/`expires` mapping), rejection of
  `cli_auth_credentials_store` and managed locks, byte-equal backup and restore, the 5-most-recent
  retention, symlink rejection, fingerprint drift before writing and external-modification detection,
  and expiry status; the session-endpoint QA loopback override is limited to local loopback. Tests use
  only a temporary `CODEX_HOME` and synthetic data and do not access real Chrome, the keychain,
  `chatgpt.com`, or a real `~/.codex`. Real-site reachability, the keychain authorization prompt, the
  Codex TUI login gate, and secret scanning belong to manual QA. Manual QA on 2026-09-30 (dev/QA
  isolated profile + a real Chrome session): the export landed only in the isolated `codex-home` (the
  login/credential target paths are independent of the production `~/.codex`), `codex login status`
  reported a ChatGPT login on both `codex-cli` 0.157.1 and the current stable 0.159.2, and the
  interactive TUI captured over a pty had no login screen; secret scanning of the logs, recovery
  points, and `codex-auth-meta.json` found no token material, and `pnpm security:check` passed. The
  two items of keychain rejection and "Chrome not logged in" were not replayed on real hardware
  (equivalent error classifications are covered by automation). In the same QA, with
  `cli_auth_credentials_store = "keyring"` the export was rejected and the bytes and mtime of
  `auth.json`/metadata were completely unchanged (the rejection happened before reading the keychain
  and before any network request), and "restore original credentials" then wrote back bytes that were
  byte-equal to the selected backup, clearing the exported state and returning to not-exported.
- Pricing sync: the tier-matcher matrix (a stable `data-value` identifier takes precedence over
  visible copy, and the `Fast mode` shape from 2026-09-29 still matches), the script contract
  extracting from a refreshed replica page in jsdom and comparing byte-for-byte against a regenerated
  payload fixture, parse negatives (missing column/bad number/duplicate/threshold conflict/over-limit),
  end-to-end replacement of the loopback fixture and `StateArea::PricingTable` publication,
  single-flight waiter regression, and isolation assertions for the hidden window/navigation
  cancellation; failure classification and snapshot invariants are proven by unit tests that do not
  access the real site (all `CaptureError` variants → category and reason, only `error` exposes
  `failure`, cleared after success, a script failure before page load classified as `Transport`, and a
  budget timeout after page load classified as `Page` with reason `capture-no-report`). Automation does
  not access the real official site. Real-site reachability and DOM compatibility belong to manual QA:
  sync once against the real site with the QA bundle (isolated identity and data) and check one Page
  and one Transport failure message, confirming at `920 x 640` and `760 x 560` under both light and
  dark themes that the status line does not wrap and push the layout and that there is no horizontal
  overflow. Any page-structure change must fail closed and keep the old table.
- Native lifecycle: use only the QA identity and verify hidden/reopen, main-thread generation, menu
  focus, and production continuity.
- Release: pinned toolchain, stable tag/ref/version/commit, ad-hoc bundle identity, arm64/minimum OS,
  updater signature, DMG content, `latest.json`, checksums, draft read-back, provenance, secret/path
  scan, and publication atomicity.

The stable names and configuration of GitHub required checks, the native source build, and repository
security features are in [GitHub CI and security settings](./github-security-settings.md). When
changing workflows, Dependabot, or supply-chain policy, run:

```sh
pnpm ci:policy
pnpm security:public:check
pnpm license:public:check
pnpm contracts:check
pnpm check:codex-retries
```

A public GitHub checkout uses `security:public:check` and `license:public:check`, so any private
workflow path, forbidden template marker, undeclared visual asset, or sensitive text fails the
required check. The private maintenance repository uses `security:check` to scan its public projection
and `license:check` to audit metadata, provenance, and dependencies; neither misreports the
intentionally retained Trellis root as a file to publish. A public export made after committing must
still run both `public` commands.

CI native source verification uses `pnpm tauri:source:build` to disable signing explicitly; local
production builds still use `pnpm tauri:prod:build`. Both commands produce only a bundle inside the
workspace and neither installs nor launches the app.

Official releases run `release:validate`, `release:draft`, `release:build`, `release:prepare`, and
`release:publish` only in the protected `release` environment. Locally, run only the synthetic-fixture
release script tests; do not read real updater secrets, create tags, or operate GitHub Releases just to
verify the flow.

The last command starts only a temporary loopback fixture, using the lockfile-pinned
`codex-cli 0.160.0` and a temporary `CODEX_HOME`, and does not read the user's Codex configuration or a
real upstream.

## Avoiding duplication

A check that already passed may be reused when the related files have not changed again. After a
focused test passes, expand further only when the full suite can prove another specific risk. A
documentation change does not trigger the Rust workspace, the frontend build, or a native bundle; a
task status update also does not require rerunning product tests.

State-synchronization tests should mount the real QueryClient and query consumers and verify how the
UI data changes after an event, a missed event, and a failed subscription registration, rather than
only asserting the call arguments of `invalidateQueries`. Refocusing the Settings window must
compensate for independent snapshots such as the price table; the menu snapshot is instead re-fetched
by the native prepare-reveal event. A late registration that completes after unmount must release its
listener, and an invalidation callback must no longer modify the cache.

A stylesheet containing a given CSS declaration does not prove the actual layout is correct. Check
layout changes with synthetic data in a real browser for size, overflow, scrolling, and keyboard focus,
and state clearly the difference from macOS native window verification; do not maintain tests by
re-pinning CSS strings. The CSP nonce placeholder element in the entry HTML is part of the security
structure contract, so keep the DOM structure assertion instead of relying on the exact tag spelling.

Manual QA covers only the visual hierarchy, copy, focus, real OS integration, and external environment
that automation cannot reliably judge. Do not make people repeat behaviors already proven
automatically, such as database migrations, validation, races, or recovery state transitions.
