# Support

AI Router is an early-stage, personally maintained project that ships one official
Apple Silicon DMG and source code. The same official DMG provides a Simplified Chinese
and English interface with in-app language selection (Follow system / Simplified
Chinese / English); other languages are not supported, and there are no per-language
installers. Maintainers make a best effort on reproducible issues, but provide no
service level, response time, paid support, or long-term maintenance for older
versions.

## Supported usage

- macOS 13 or later on Apple Silicon;
- Build per the [README](./README.md) with Node `22.22.3`, pnpm `10.33.2`, Rust
  `1.97.1`, and the lock file;
- Codex CLI/App and upstreams compatible with the OpenAI Responses API; the automated
  compatibility evidence is pinned to `codex-cli 0.162.0`;
- Routing can select a Responses or Chat Completions upstream protocol. Chat Completions
  routing still presents the Responses contract to Codex, but only covers the documented
  compatibility behavior; opaque reasoning, richer `web_search` declarations, remote
  compaction, and non-streaming client requests fail closed, as detailed in
  [routing and resilience](./docs/engineering/routing-resilience.md#chat-completions-upstream-compatibility);
- "Sync from website" in settings is a purely manual action: it visits the public pricing
  page on `developers.openai.com` and only stores GPT model prices; it does not sign in
  or collect account information, and keeps the previous local price table on failure;
- The official DMG from the canonical GitHub Release, or the project's original source
  and unmodified production/QA configuration.

Windows, Linux, Intel Macs, languages other than Simplified Chinese and English, WAN
proxies, multi-user shared gateways, Developer ID signing, Apple notarization, the App
Store, silent updates, and Gatekeeper bypass instructions are all outside the current
support scope. The official DMG is ad-hoc signed only, so it is not Apple-verified or
notarized; source builds also do not embed the official updater public key.

## Getting help

Before filing a bug, run `pnpm docs:check`, check
[README troubleshooting](./README.md#troubleshooting), and search existing issues. A
public report should include the version/commit, macOS, hardware, Codex version, build
method, minimal reproduction steps, expected result, actual result, and stable error
code.

Do not submit real API keys, Authorization headers, complete configuration (including
Codex configuration), SQLite databases, raw requests/responses, raw logs, recovery
points, or user absolute paths in public issues, PRs, or attachments. When you must
share diagnostics, quote only the fixed error codes directly relevant to the
reproduction, and review the content again.

Use the private channel in the [security policy](./SECURITY.md) for security issues; do
not create a public vulnerability report. Feature suggestions should state the current
pain point, target users, verifiable outcome, and explicit non-goals, and should not
treat the full feature set of some external product as the default scope.

## Source-build boundaries

Maintainers can help triage reproducible problems in the repository scripts or the
supported toolchain, but cannot investigate unknown shell configurations, third-party
package managers, corporate device policies, unofficial signing tools, or modified
forks. Local apps produced by a source build are the builder's own to store and
evaluate.

## Installation and update support

For a first install, just download the DMG. The `.app.tar.gz`, `.sig`, and `latest.json`
files in a Release are what in-app updates need; `SHA256SUMS` and GitHub provenance are
for independently verifying content and origin, not alternative packages to install by
hand. When macOS blocks the first launch, only the system authorization path of
"System Settings -> Privacy & Security -> Open Anyway" is supported.

In-app updates only support stable versions published from the canonical repository with
a correct signature and a strict upgrade. A failure keeps the current installation; you
can open the canonical Release from the UI and install the DMG manually. The first
updater-capable version must be installed manually via the DMG.
