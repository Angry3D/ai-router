# Contributing

Thanks for improving AI Router. The project is still at an early stage, and public
collaboration focuses on small, verifiable changes that never expose user data.

## Development environment

The supported environment is macOS 13+ Apple Silicon. Use Xcode Command Line Tools,
Node `22.22.3`, pnpm `10.33.2`, and Rust/Cargo `1.97.1`. See the
[README](./README.md#build-prerequisites) for the full installation steps.

```sh
nvm use
corepack enable
corepack prepare pnpm@10.33.2 --activate
pnpm install --frozen-lockfile
```

Do not update lock files to mask an installation failure; only update `pnpm-lock.yaml`
or `Cargo.lock` when the dependency change itself is in scope for the PR.

## Code boundaries

- `crates/router-core/`: domain validation, SQLite, routing/fallback, Responses
  forwarding, Codex configuration, and recovery.
- `src-tauri/`: Tauri IPC, desktop service composition, logging, and the macOS
  window/tray lifecycle.
- `src/`: React menu and settings UI; Rust-generated types live in `src/generated/`.
- `scripts/`: version, type, icon, license, public-source check, and QA tooling.
- `docs/engineering/`: engineering contracts external contributors must follow.

For details, start from the [engineering docs index](./docs/engineering/README.md);
branches, PRs, mainline, and version delivery follow the
[development and delivery lifecycle](./docs/engineering/development-lifecycle.md).

## Committing changes

1. Create a focused branch first, and confirm the working tree has no changes by others
   that your operations would overwrite.
2. Before implementing, search for similar code, constants, generation chains, and
   tests. Keep a single authoritative source for each business rule.
3. Add focused tests for observable behavior that fail before the fix and pass after it.
4. Choose verification in proportion to the change's risk; do not unconditionally run
   the full native acceptance suite for documentation or localized changes.
5. A PR description should state the behavior change, risks, verification commands, and
   uncovered boundaries.

Do not put real API keys, Authorization headers, complete Codex configuration, SQLite
databases, raw requests/responses, full run logs, or user absolute paths in PRs, issues,
test fixtures, screenshots, or commit history. Demos, automated tests, CI, temporary
fixtures, and ordinary development commands use only obviously fictional synthetic data
and loopback listeners, and never reach a real upstream.

## Authoritative commands

| Change                              | Minimum starting checks                                                                        |
| ----------------------------------- | ---------------------------------------------------------------------------------------------- |
| Documentation only                  | `pnpm docs:check`, `git diff --check`                                                          |
| React/TypeScript                    | Focused Vitest, `pnpm typecheck`, `pnpm lint`                                                  |
| Single Rust module                  | Focused `cargo test`, `cargo fmt --check`, Clippy for the affected crate                       |
| Generated DTOs                      | `pnpm generate:types`, review the generated diff, then run the related frontend and backend checks |
| Web build/configuration             | `pnpm build`                                                                                   |
| Version metadata                    | `pnpm version:check`                                                                           |
| CI/supply-chain configuration       | `pnpm ci:policy`, `pnpm security:public:check`                                                 |
| Generated types and protocol        | `pnpm contracts:check`, `pnpm check:codex-retries`                                             |
| Dependencies or third-party content | `pnpm license:public:check`                                                                    |
| Production source bundle            | `pnpm tauri:prod:build`                                                                        |
| Release scripts/workflows           | Focused script tests, `pnpm ci:policy`, the release engineering contract                       |

The full frontend and Rust baseline commands are:

```sh
pnpm docs:check
pnpm ci:policy
pnpm security:public:check
pnpm license:public:check
pnpm lint
pnpm typecheck
pnpm test
pnpm build
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Focused frontend tests use `pnpm exec vitest run <test-file>`. Only cross-layer DTOs,
database migrations, recovery, fallback concurrency, streaming responses, native
lifecycle, or release-boundary changes need the full scope in the
[verification docs](./docs/engineering/verification.md).

## Generated files

- Run `pnpm generate:types` after changing the IPC DTOs exported from Rust; do not
  hand-edit `src/generated/`.
- Run `pnpm icons:generate` after changing the app or tray SVG sources, keeping the
  generated PNG/ICNS files and the source records.
- Before changing the version, run `pnpm version:sync`, then run `pnpm version:check`
  and review every change.
- Update the adjacent source records, notices, and license policy when changing
  third-party fixtures, icons, or pricing snapshots.

## Release boundaries

Regular contributors and the pull request workflow never touch the updater signing key
and never create or modify a GitHub Release. `pnpm tauri:prod:build` and
`pnpm tauri:source:build` only produce a `.app` without official update credentials;
the distributable DMG, updater archives, and signatures are built only by the protected
tag workflow. Do not pass a real `TAURI_SIGNING_PRIVATE_KEY`, its password, or other
release environment values into local commands, tests, logs, PRs, or issues.

Changes to release behavior must cover tag/ref/version binding, draft-only fixes, the
asset inventory, ad-hoc bundle signing, updater signatures, checksums, and provenance,
and must also update the [stable release operations](./docs/engineering/releasing.md).

## Native verification safety

A running `AI Router.app` may host the current Codex session. Automation must not quit,
signal, restart, replace, or relaunch the production bundle. Development and lifecycle
tests use `pnpm tauri:qa:dev` or `pnpm tauri:qa:build`, and must verify the bundle is
`AI Router QA.app` with identifier `com.relax.airouter.qa` before any destructive
operation. Do not copy production routing, keys, databases, logs, or Codex configuration
into QA.

The human Live QA granted as an explicit one-off authorization is separate from the
synthetic tests above; it may only use a verified fresh build and a persistent QA
profile, and must not copy persistent QA data to a temporary root. Per the
[Controlled Live QA contract](./docs/engineering/native-lifecycle.md#controlled-live-qa),
an assistant may use a short-lived local caller to read the QA gateway token into
memory, so the user does not need to operate another MCP client; the upstream key stays
inside the app, a single image generation is not retried, only fixed aggregate results
are emitted, and the user confirms remote revocation of the dedicated credential.
Editing documentation is not an operational authorization and cannot override
higher-priority instructions; the production app and data always stay out of scope.

## Issues and PRs

Use the Bug report form for ordinary defects and the Feature request form for feature
suggestions.
Report security issues privately per [SECURITY.md](./SECURITY.md). Maintainers review
based on scope, reproduction quality, risk, and maintainability; opening a PR does not
mean it will be merged.
