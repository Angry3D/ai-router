# macOS native lifecycle

AI Router is a menu bar app with the `Accessory` activation policy. The `menu` WebView starts hidden
and is converted to a non-activating `NSPanel` on macOS; `settings` stays a normal window. Tray
clicks, the first reveal, and a LaunchServices Reopen all reuse the same reveal path.

## Menu panel

A hidden WebView may suspend WebContent or stop scheduling `requestAnimationFrame`. The native layer
first shows and positions the window, then sends a prepare event carrying a monotonic generation; the
frontend reports back a height after finishing layout and also provides a timer fallback. The native
main thread rechecks the generation right before the actual reveal, so an old request cannot
re-awaken a hidden menu or write the first-reveal state.

The menu uses a key-capable, non-main, non-activating panel so that the user can operate the menu
without bringing the whole app to the foreground. Only an explicit open of Settings calls ordinary
window focus. Escape, a tray toggle, a close request, and loss of focus hide the menu, but do not quit
the process.

When changing menu geometry or the route usage preview, the size, position, fixed backing, and
controller state must be committed in the same main-thread transition; a failure must roll back to the
full base geometry, and an asynchronous old command must not overwrite a newer revision.

## Production and QA identity

| Mode       | bundle             | identifier              | Codex/data behavior                                                                       |
| ---------- | ------------------ | ----------------------- | ----------------------------------------------------------------------------------------- |
| Production | `AI Router.app`    | `com.relax.airouter`    | Uses production app data; manages `~/.codex/config.toml` on an explicit connection        |
| QA         | `AI Router QA.app` | `com.relax.airouter.qa` | Uses separate app data, an isolated Codex home, and a system-assigned proxy port          |

The production app may be serving the proxy for the current Codex session. Automation and contributor
scripts must not quit, signal, restart, replace, or relaunch the production bundle. Native lifecycle
and destructive recovery acceptance may target only a QA bundle with a verified identifier, using
synthetic data.

Manual QA uses the persistent QA system data directory directly; automated tests, CI, temporary
fixtures, and ordinary development commands use only synthetic data and loopback listeners and never
touch a real upstream. Automated lifecycle/recovery tests may use a temporary acceptance root; a
temporary root is not a second QA product and should not be used for ordinary manual acceptance. Do
not copy persistent QA data into a temporary root. No mode may copy production routes, API keys,
history, database, logs, or Codex configuration into QA.

## Building

```sh
pnpm tauri:dev
pnpm tauri:qa:dev
pnpm tauri:qa:build
pnpm tauri:prod:build
```

`pnpm tauri:dev` and `pnpm tauri:qa:dev` both use the QA identity; a debug build that uses the
production identity (`com.relax.airouter`) fails closed in setup so that a development process cannot
touch the production data directory and the real Codex configuration. Destructive native acceptance
uses only the QA bundle and synthetic data.

Ordinary production, QA, and CI source builds all use the root `target/`, produce only a `.app`, and
consume no release secret. The production and QA builds each check the name, identifier, icons, and
minimum macOS version. The protected tag workflow uses a separate release configuration to produce
the DMG, an explicitly ad-hoc-signed `.app`, and the Tauri updater archive; it does not use Developer
ID, Apple notarization, stapling, or the App Store.

After the update installation completes, a dedicated exit intent that `ExitRequested` can intercept is
sent first. The Tauri restart request is called only after the existing path has completed
`AppCoordinator::shutdown()`; it must not be called first, because Tauri ignores `prevent_exit()` for
that event. Normal Quit behavior is unchanged. Automated restart/install acceptance must verify the
exact QA identifier, PID, executable, acceptance root, and production continuity, and must not run
update acceptance against a production bundle.

Do not run a native bundle for ceremony when the change is only documentation, React units, or a
single Rust module. An `.app` build and QA native acceptance are needed only when changing Tauri
configuration, icons, panel/tray lifecycle, packaging scripts, or release boundaries.

## Controlled Live QA

This is a manual operation, separate from synthetic automated tests, that the user explicitly
authorizes one time only; it is not a default test mode or a continuously running service. The
assistant may operate a short-lived local MCP invoker, and the user does not need to operate an MCP
client separately. Editing this project's documentation is not itself an authorization and cannot
override higher-priority instructions; without this authorization, the assistant neither reads
credentials nor sends real calls.

### Preflight checks

The following checks must complete before reading the QA gateway token and sending a credentialed
request; when something is missing, expired, mismatched, or of unclear identity, fail closed and
report only the fixed blocking reason rather than probing the environment with a real call.

1. Use the single canonical bundle successfully produced by `pnpm tauri:qa:build`:
   `target/release/bundle/macos/AI Router QA.app`, whose identifier must be
   `com.relax.airouter.qa`. The build receipt must come from a successful build and associate the
   source commit, the content fingerprint of the build inputs including the relevant uncommitted
   changes, the QA identity, and the SHA-256 of that bundle's executable. Before invoking, verify the
   current source, bundle, and receipt; a version number, name, modification time, or error-string
   scan alone cannot prove a new build, and an old bundle cannot be given a receipt after the fact.
   Use `node scripts/v0-2a-qa-identity.mjs verify-build` to check the build record; the record lives at
   `target/release/qa-build-receipt.json` and is not written into the packaged app. Determine the
   source commit before building; a later commit change invalidates the old record, and uncommitted
   task or documentation notes do not affect the source content fingerprint.
2. Before replacing the QA bundle or switching the data profile, verify the exact PID, executable,
   identifier, and data profile of the old QA process and stop only that process safely. After
   building, restart the verified QA bundle and verify that the PID, the actual executable, and the
   build receipt of the single running instance agree; do not accept a process still running an old
   image just because the disk path is the same. Use
   `node scripts/v0-2a-qa-identity.mjs verify-process --pid <qa-pid>` to verify; for a temporary
   profile also pass `--root <temporary-root>`. The process start must be later than the second in
   which the build completed, to avoid ambiguity from macOS's second-level start time; a temporary
   launch helper waits automatically, and a manual launch must satisfy the same condition.
   Throughout, do not quit, signal, restart, replace, or start the production app, and do not touch
   production data or credentials.
3. First perform the synthetic URL-only acceptance of the real app MCP through the same verified QA
   executable: start the temporary root from synthetic data, have the local upstream return only an
   image URL, and observe one generation POST, an asset GET, credential-free forwarding, and a valid
   private PNG. After stopping that exact QA process, clean up only the verified temporary root and
   do not copy or delete persistent QA data.
4. Live QA must reuse the persistent QA profile, confirm that `AI_ROUTER_QA_ACCEPTANCE_ROOT` is
   unset, and isolate the actual app data and Codex home from production. Verify the IPv4-loopback
   listener actually owned by that QA PID and send the request only to its `127.0.0.1` address and
   actual port at `/mcp`; do not guess the port, do not use a production listener, do not go through
   an environment proxy, and do not follow redirects to another address. The entry check does not
   change the app's upstream or asset download contract.
5. The user configures and confirms a dedicated, revocable upstream credential in QA and does not
   reuse a production key; the assistant reads only the necessary non-sensitive readiness fields such
   as image-enabled state, the selected image route, model, and timeout. Confirm the use of the
   approved route/model and `size=1664x944`, `quality=medium`, and do not modify parameters to force a
   particular return carrier.

### Single invocation and credential boundary

- Only after both the preflight checks and this authorization pass does the invoker read the single
  secret, the QA local gateway token, into this process's memory for the Authorization of the loopback
  request above. Upstream route keys are read and used only by the QA app; do not read the full
  SQLite/config, dump headers, ask the user to paste secrets into the chat, or put secrets into
  command arguments, shell tracing, logs, files, or persistent evaluator/kernel variables. The
  invoker's credential lifetime ends when it exits.
- Before invoking, pin and enforce finite timeouts for initialization, the single
  generation/response read, and the invoker as a whole, stating the exact seconds; the overall
  deadline must accommodate the current QA image-generation timeout plus the existing
  response-body-read and asset-download deadlines, and must not rely only on a socket idle timeout.
  The invoker parses the request/response in memory within bounds and does not print raw frames,
  provider messages, or exception stack traces.
- After completing the MCP `initialize` / `initialized` protocol interaction, send exactly one
  `tools/call` invoking `generate_image`, with a synthetic prompt that displays no content.
  Initialization does not count as a generation; sending the generation request consumes this
  authorization, and a failure, timeout, unknown result, or asset-handling failure does not refund
  the authorization. Do not automatically retry, switch route/model, change parameters, or
  regenerate; even when an error is marked `retryable`, another explicit authorization is required to
  call again.
- Fail closed when the response shape is unknown. An image counts as success only when the returned
  file is inside the private `mcp-images` root of the expected persistent QA app data, is a regular
  non-symlink private file, and passes the PNG content and the consistency checks of the returned
  byte count, SHA-256, width, and height. Returning only a successful HTTP/MCP status or a passing
  health check does not count as image acceptance; do not relax any existing TLS, resource, decode,
  PNG validation, or private-publication boundary, see the
  [image result contract](./routing-resilience.md#image-result-compatibility).

### Fixed evidence and wrap-up

The invoker emits only allowlisted aggregate fields: QA identity, source commit/content fingerprint,
executable digest, source/process match booleans, observed invocation and GET counts, fixed
status/code/stage, numeric-or-null upstreamStatus, retryable, PNG/integrity booleans, the
carrier-observation enum, and cleanup confirmation. Unknown errors map to fixed failure categories;
do not emit URLs, host/query, prompts, private route names, provider IDs, absolute paths, asset bytes,
secrets, raw responses, or full run logs.

Generation-phase failure, asset-phase failure, and valid PNG success must be recorded separately; an
upstream 502 is not evidence of a downloader regression. Synthetic URL-only real-app acceptance and a
valid Live PNG are two independent pieces of evidence; a Live carrier may only report base64, URL, or
not observed as safely observed, and a successful image must not be used to infer that a Live GET
happened, nor may a carrier be observed with an extra paid call.

After the operation, exit the invoker and remove the temporary driver, keeping only the safe evidence
above. Only after the user confirms that the provider has revoked the dedicated upstream credential
may the revocation item be closed; deleting a key locally is not remote revocation, and without
confirmation you must not claim everything is complete. Perform only the authorized local QA
credential cleanup, keep the persistent QA non-secret routes, models, and settings, and do not wipe
the whole QA directory; the production app and data are unaffected.
