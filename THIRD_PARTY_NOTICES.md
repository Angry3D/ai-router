# Third-party notices

AI Router's root `LICENSE` covers only the project's own content. The third-party content
listed in this file is not relicensed as MIT by entering this repository; using and
redistributing it must also comply with the corresponding upstream licenses.

<!-- provenance:openai-codex-base-instructions -->

## OpenAI Codex base instructions

- Local file: `fixtures/codex-base-instructions-v0.150.1.md`
- Upstream project: `openai/codex`
- Upstream version: `rust-v0.150.1`
- Upstream path: `codex-rs/protocol/src/prompts/base_instructions/default.md`
- Upstream address: <https://github.com/openai/codex>
- License: Apache License 2.0
- Local handling: content unmodified; the provenance JSON in the same directory binds the
  upstream version, path, and SHA-256.

The full Apache License 2.0 text is in
[`third-party/licenses/Apache-2.0.txt`](./third-party/licenses/Apache-2.0.txt).

<!-- provenance:lucide-icons -->

## Lucide icons

- Local files: `src-tauri/icons/app-icon*.{svg,png,icns}`, `icon.png`, and
  `src-tauri/icons/tray-*.{svg,png}`; `docs/images/readme/*.png` also contain Lucide
  icons rendered in the app UI
- Upstream project: `lucide-icons/lucide`
- Version used: `lucide-react 0.468.0`
- Upstream address: <https://github.com/lucide-icons/lucide>
- License: ISC
- Local handling: the Route, TriangleAlert, and CircleX geometry is scaled, stroked, and
  combined; the app icon's background, colors, QA markers, and overall composition are
  AI Router project designs. The PNG and ICNS files are generated from SVG by repository
  scripts. The README screenshots are QA run results of the first-party UI, and the
  Lucide icons in them render with the UI; see `docs/images/readme/README.md` for
  screenshot capture and SHA-256 records.

Lucide's copyright and the full ISC license text are in
[`third-party/licenses/ISC-Lucide.txt`](./third-party/licenses/ISC-Lucide.txt). See
[`src-tauri/icons/README.md`](./src-tauri/icons/README.md) for icon source files, the
generation chain, and output boundaries.

<!-- provenance:readme-product-screenshots -->

## README product screenshots

- Local files: `docs/images/readme/*.png`
- Source: first-party UI rendered by the AI Router project in `AI Router QA.app`
- License: the first-party UI composition uses the root MIT License; the Lucide icons
  rendered within it remain ISC
- Local handling: the screenshots are generated with isolated QA data and synthetic
  requests, re-encoded as PNG before release, and checked for visuals, OCR, metadata, and
  SHA-256.

See [`docs/images/readme/README.md`](./docs/images/readme/README.md) for the capture
process, synthetic data boundaries, and final hashes. Lucide's full ISC license text is
in [`third-party/licenses/ISC-Lucide.txt`](./third-party/licenses/ISC-Lucide.txt).

<!-- provenance:openai-pricing-snapshots -->

## OpenAI pricing snapshots

- Local files: `crates/router-core/pricing/catalogs/openai-standard-2026-07-27.json`,
  `crates/router-core/pricing/catalogs/openai-priority-2026-07-28.json`
- Data source: OpenAI's official pricing, Prompt Caching, and Fast mode documentation;
  each JSON file keeps the specific URL and capture date.
- Local handling: only public factual rates are recorded and converted to the integer
  `micro_usd_per_million_tokens`; no upstream code or explanatory prose is copied. The
  JSON schema and implementation code are AI Router project content under the root MIT
  License.
- Runtime sync: "Sync from website" in settings is triggered manually by the user, reads
  the same official pricing page in a hidden read-only WebView, and writes only GPT model
  price rows into the local price table; releases do not include sync results or
  distribute page content.

These snapshots are only for local estimation; they are not OpenAI's billing, a quote, or
an ongoing commitment. Upstream rates may change at any time, and users should rely on
actual billing and the current official documentation. See
[`crates/router-core/pricing/catalogs/README.md`](./crates/router-core/pricing/catalogs/README.md)
for detailed units and update rules.

<!-- provenance:cc-switch-chat-bridge -->

## Codex Chat bridge literals

- Local files: `crates/router-core/src/proxy/chat_bridge.rs`,
  `third-party/cc-switch-chat-bridge.provenance.json`
- Upstream project: `farion1231/cc-switch`
- Upstream commit: `0555c09d9aa06e5f93bf6d57ecf8faa9e6305c4b`
- Upstream path: `src-tauri/src/proxy/providers/transform_codex_chat.rs`
- Upstream address: <https://github.com/farion1231/cc-switch>
- License: MIT
- Copyright: Copyright (c) 2025 Jason Young
- Local handling: only model-visible short literals are copied verbatim from upstream —
  the synthetic `tool_search` function declaration (including its name, description, and
  `query`/`limit` parameter schema), the description text that wraps custom tool input,
  and the `Original tool definition:` heading and its code block format. The SHA-256 of
  the declared literals
  (`e684675e547c7e60317a9a43834d2133d3655704902296e0881619c13f48b06c`) is pinned by a
  unit test. No upstream source files are copied; the remaining transformation logic,
  error classification, and boundary handling are AI Router's own implementation.

The full MIT license text is in
[`third-party/licenses/MIT-cc-switch.txt`](./third-party/licenses/MIT-cc-switch.txt), and
the verbatim content, adaptation behavior, and intentional divergences are in
[`third-party/cc-switch-chat-bridge.provenance.json`](./third-party/cc-switch-chat-bridge.provenance.json).

## Dependencies

npm and Cargo transitive dependencies are not relicensed as MIT by appearing in the
lockfile. Run `pnpm license:check` to generate an audit summary from the current lockfile
and installed dependencies; the policy is in `scripts/license-policy.json`. Any missing,
unknown, or unreviewed license blocks the check.
