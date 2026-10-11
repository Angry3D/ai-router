# Data, privacy, and recovery

AI Router has no accounts and no cloud sync. Data lives mainly in the current macOS user directory,
but "local" does not mean "encrypted" or "safe to share publicly".

## Data locations

The production bundle identifier is `com.relax.airouter`. On a standard macOS environment, Tauri
resolves app data and logs to the following locations:

| Content                 | Location                                                                                          |
| ----------------------- | ------------------------------------------------------------------------------------------------- |
| App data                | `~/Library/Application Support/com.relax.airouter/`                                               |
| Main database           | `router.sqlite3` in the app data directory                                                        |
| Recovery points         | `recovery/` in the app data directory                                                             |
| MCP image assets        | `mcp-images/<assetId>.png` in the app data directory                                              |
| Derived model catalog   | `codex-model-catalog.json` in the app data directory (when custom models exist)                   |
| Local price table       | `pricing/local-pricing.json` in the app data directory (after a manual official-site sync)        |
| Run logs                | `~/Library/Logs/com.relax.airouter/`                                                              |
| Incident records        | `incident-<millisecond timestamp>.json` in the run log directory                                   |
| Codex configuration     | `~/.codex/config.toml` (owned by Codex; AI Router only makes a protected projection)              |

The directory the system APIs finally resolve to may be affected by the sandbox or system policy.
"Open log directory" in Settings is the authoritative entry point for viewing run logs; do not
hard-code a maintainer machine's absolute path in code or in an issue.

The QA bundle uses a separate system data directory under `com.relax.airouter.qa`; automated
acceptance may also use a temporary root with a safety marker. QA data must not be copied from the
production database.

## Data classification

`router.sqlite3` stores routes, base URLs, API keys, balance scripts, the active route, fallback
settings, the gateway token, Codex original/recovery configuration snapshots, custom models,
request/attempt metadata, and usage statistics.

API keys and the gateway token are stored in SQLite as raw bytes. The database, WAL, recovery points,
and backups rely on private directory/file permissions (directory `0700`, key files `0600`), but
macOS Keychain is not used today and there is no application-layer encryption. Anyone with read
access to the user's files may still be able to read these secrets.

Request history does not store raw request bodies, prompts, full responses, the Authorization header,
or arbitrary upstream headers. Cost is an estimate computed from a dated local price snapshot, not a
bill or a current-price commitment. Price snapshots have two sources: the embedded baseline (shipped
with the app) and the manual "sync official site" triggered by the user in Settings. The sync visits a
fixed official pricing page only in a hidden, capability-less, incognito read-only WebView; the
returned content is limited to model IDs and prices and is written to `pricing/local-pricing.json`
(including the source URL and sync time); it does not upload local data, export cookies, or read any
other page content, and on failure it keeps the previous local price table. Requests already written
to history keep their own catalog version and are not repriced by a later sync.

The Images MCP writes a successfully generated and validated PNG as a local asset under
`mcp-images/`, then returns only the file path, actual dimensions, byte count, and SHA-256. An image
may come from upstream base64 or a controlled downloaded URL; both sources go through the same PNG
validation. PNGs, upstream base64, and raw responses never enter SQLite, recovery points, or the run
log. This directory holds generation results on the user's own machine, not database state, and it is
not the same as a backup.

Image URLs, signature query parameters, and redirect Locations exist only in memory for the current
call; they are not written to error descriptions, the database, recovery points, the run log, or
ordinary IPC, and a failed URL is not persistently cached. Download requests carry no generation-API
key, local gateway token, or client credential. Source resolution, download, PNG validation, and
publication share one high-memory permit; cancelling a call cannot release early a permit held by a
blocking task that is still running. A strictly bounded content decoder also limits the zstd window
so that decode memory stays bounded even though only the response bytes are capped.

An Images MCP failure returns only a stable local code, a local requestId, a closed stage/category, a
numeric-or-null upstream status, and retryable. Known upstream categories use fixed safe descriptions;
only an unknown category may append to that MCP message a single provider message cleaned of control
characters, whitespace-collapsed, and length-limited. This exception exists only in the current
response: provider codes/messages, provider request IDs, arbitrary upstream headers, raw
requests/responses, and underlying network, decode, and IO errors never enter error.data, the run log,
diagnostics, request/attempt history, SQLite, recovery points, or ordinary IPC.

Each run log line starts with local time and an explicit timezone offset (of the form
`2026-09-24T15:09:48.303+08:00`), followed by the level, target, and message; UTC is used when the
offset is unavailable. Each line is at most 8 KiB, each file at most 2 MiB, at most ten files/20 MiB,
and at most seven days are retained. Logs should contain only fixed codes, safe component names, and
bounded counts; even so, review them by hand before sharing, and do not upload full logs directly.

Incident records are separate diagnostic files: whenever the app detects database corruption, attempts
an automatic repair, quarantines an unusable main database, or starts the database over, it writes an
`incident-<millisecond timestamp>.json` file in the run log directory (directory `0700`, file `0600`,
at most 20 retained). The content is only the detection time, corruption classification, the
`pragma integrity_check` message text (at most 32 messages/4 KiB, with overflow marked
`<n> more messages truncated`), the app version, the main database byte count and SHA-256, the actions
taken, the recheck result, and the elapsed time. It contains no secrets, base URLs, request/response
content, data rows, or Codex configuration, and it is not part of recovery-point content; the log
cap/cleanup only handles `ai-router*` log files themselves and never deletes incident records.

## Codex configuration ownership

Before the first production connection, the app reads and saves the existence state, raw bytes, and
Unix mode of `~/.codex/config.toml` to form an immutable baseline. Connecting and reconnecting
preserve the current `model_provider` identifier and unrelated provider/MCP/permission settings,
managing only the selected provider's transport fields, the AI Router-owned model catalog pointer, and
the optional image MCP entry. Writes use a fingerprint, a temporary file in the same directory, fsync,
and a pre-replacement race check.

An ordinary disconnect can update the "disconnect target"; the full baseline is always retained as
evidence of ownership before the first takeover. External edits, symlinks, invalid TOML, an MCP entry
with a conflicting name, or a fingerprint change all fail closed and should not be resolved by
overwriting the file directly.

## Codex credentials (auth.json)

The "Codex credentials" feature in Settings read-only reads the cookie database of a local
Chrome-family browser and decrypts only the two `__Secure-next-auth.session-token.*` shards (the
browser's secure-storage key is read through the system keychain); it visits the fixed endpoint
`https://chatgpt.com/api/auth/session` in a hidden, capability-less, incognito read-only WebView
(following the system proxy), generates `$CODEX_HOME/auth.json` (default `~/.codex/auth.json`)
locally, and replaces it atomically. Cookie values, the session JSON, and the generated token exist
only briefly in memory and are zeroed; they are not written to the run log, diagnostics,
request/attempt history, SQLite, recovery points, or crash information; cookies injected into the
WebView are deleted at the end of each run. This is the only new outbound traffic the feature adds,
and it does not upload any local data.

Before writing, the existing `auth.json` is backed up to `codex-auth-backups/` in the app data
directory (directory `0700`, file `0600`, the 5 most recent kept, named by timestamp plus a short
fingerprint), which the user can retrieve with "restore original credentials" in Settings.
`codex-auth-meta.json` in the same directory stores only non-secret metadata (export time, expiry
time, redacted email, plan type, and the SHA-256 of the written file) to show status and detect
external modification; it contains no cookies, tokens, or session JSON. That directory is sensitive
and should be protected as key material. Recovery points, SQLite, and request history all contain no
credential content. Credentials are single-use: Codex's refresh endpoint (`auth.openai.com`) does not
accept a web session token, so they must be re-exported after expiry; the app does not automatically
restart Codex.

## Recovery points

After a critical configuration commit, a recovery worker produces a SQLite Backup API copy after a
short quiet period, deletes non-critical table content such as history/usage/logs, validates
integrity, foreign keys, schema, domain references, and permissions, then publishes atomically. At
most five valid recovery points and one quarantined corrupt main database are retained.

Recovery points contain route secrets, the gateway token, the Codex baseline/disconnect target, and
custom models, so they are likewise sensitive files. They do not contain request/attempt history,
usage rows, logs, balance caches, the external `config.toml` file, derived model JSON, or
`mcp-images/` image assets.

Normal settings only allow viewing protection status and creating a recovery point manually. When the
main database is missing, corrupt, or domain-invalid, startup enters a separate recovery flow, and
only a candidate that passes the current schema and full validation can be restored. Neither recovery
nor "start over" reads or writes `~/.codex/config.toml`.

## Automatic repair of index-level corruption

The startup check reads **all** messages from `pragma integrity_check`: only when the corruption is
entirely index inconsistency (an index missing rows, a mismatched entry count, or an index entry that
cannot be represented exactly) does the app rebuild the index after keeping a quarantined copy
(`recovery/quarantine-*.sqlite3`, the bytes from before the repair) and continue startup once the
recheck passes; any page-level/structural corruption, unreadable report, or uncertain classification
still enters the recovery flow above. The repair never silently deletes data: row counts are identical
before and after, and if the recheck fails the quarantined copy is kept and the app falls back to the
existing recovery flow.

At runtime, once every 6 hours (the first time is 10 minutes after startup) and provided there has been
no in-flight request in the last 5 minutes, a read-only self-check runs; on finding index-level
corruption it pauses forwarding, repairs in place, and resumes forwarding after the recheck passes,
and on finding unrepairable corruption it only marks the protection status without touching the data.

`router.sqlite3.lock` in the app data directory is a process-exclusive lock that prevents two
instances from writing the same database at once; a second instance gets a retryable "data directory
already in use" message instead of continuing to start.

## Backup and issue reporting

Backing up the whole app data directory also backs up credentials and Codex configuration snapshots,
so use encrypted storage and restrict access. Do not copy only `router.sqlite3` while the app is
running and ignore the WAL; prefer fully quitting the app and then backing up the whole directory.
Editing the database, recovery points, or `config.toml` by hand may break fingerprints and recovery
ownership, so keep the originals first.

The PNGs in `mcp-images/` are only local generation assets. Unless the user separately includes these
files in their own backup scheme, corrupted or deleted files or a lost app data directory cannot be
recovered from SQLite recovery points.

Public issue reports provide only a synthetic reproduction, a version, a stable error code, and the
necessary bounded fields. Databases, recovery points, full configuration, full logs, and real request
content should be discussed only through the private channels agreed in the
[security policy](../../SECURITY.md), and usually do not need to be uploaded.
