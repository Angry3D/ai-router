# Release and app updates

AI Router ships stable releases only for macOS 13+ Apple Silicon. Official distribution does not use
Apple Developer ID, Apple notarization, stapling, or the App Store; the app inside the DMG uses an
explicit ad-hoc signature. An ad-hoc signature keeps the code structure inside the bundle consistent
but cannot prove the publisher's identity, nor does it mean Apple has verified the app.

## Release assets

Every stable Release must contain all five of these assets:

| File                              | Purpose                                                 | Needed by ordinary users              |
| --------------------------------- | ------------------------------------------------------- | ------------------------------------- |
| `AI.Router_<version>_aarch64.dmg` | First install from Finder and manual update recovery    | Yes; it is the only first-install download |
| `AI.Router.app.tar.gz`            | The app archive downloaded by Tauri in-app updates      | No; fetched automatically by the app  |
| `AI.Router.app.tar.gz.sig`        | The project updater signature for the archive           | No; verified by the updater           |
| `latest.json`                     | Stable version, notes, archive URL, and signature metadata | No; checked by the app             |
| `SHA256SUMS`                      | SHA-256 list for the first four of the five content assets | Optional; for manual verification  |

GitHub artifact provenance is not a sixth download file but a queryable attestation GitHub produces
for these assets. It binds the build output to the workflow, repository, and source revision. SHA-256
and provenance help manual auditing, but the in-app installation authorization is still based on the
updater signature.

## Trust boundary

`latest.json` and downloaded content are both treated as untrusted remote input. The Rust coordinator
accepts only the canonical repository, `darwin-aarch64`, a strictly stable SemVer higher than the
running version, a canonical archive URL, bounded release notes, and a bounded signature. These
checks decide whether to show an update; they do not grant installation permission. Once the user
confirms the download, the Tauri updater must verify the archive signature with the project public key
in the bundle and only then proceed to install. The signature must also record the version it
corresponds to: the app enables `requireSignedVersion`, which compares the version declared in
`latest.json` with the version in the signature's trusted comment and rejects installation if they
differ or are missing. This way, "pairing a larger version number with another legitimately signed
package" cannot cause a downgrade install. This check takes effect from the build that enables it;
earlier installed versions do not verify the version inside the signature.

New release notes are generated from the reviewed content committed at `release-notes/v<version>.md`.
Rust parses only restricted headings and flat bullet items and projects the three data groups
`Highlights`, `Fixes`, and `Notices` to the UI; React does not parse Markdown or HTML. The Settings
page shows at most three highlights by default and expands all categorized items on demand. Notes in
the legacy plain-text form are still displayed with bounded compatibility, and the canonical GitHub
Release entry point is retained.

Automatic checks run immediately after the app enters `Running`, writing the attempt time to SQLite
before performing the network request. After that the scheduler runs long-lived inside the same
process: after a success it waits 24 hours, after a network or metadata failure it waits 6 hours, and
when another update operation holds the mutex gate it retries after a bounded short delay without
recording an attempt. The wait is computed in wall-clock segments, so machine sleep does not lengthen
the actual period; a clock rollback of more than a period is treated as due. Every restart resets the
wait period for this process and checks immediately — the persisted attempt time is only a record and
no longer suppresses checks across restarts, and recovery points still clear that non-critical
timestamp. After a successful database recovery (restoring a recovery point, starting over, retrying
startup) the app likewise enters `Running` and therefore also starts the scheduler; the scheduler is
claimed only once per process and repeated calls are no-ops. Once an install completes and the app
enters `restart_ready`, this process stops scheduling and the new process starts over. Background
failures stay quiet. A manual check bypasses the cadence and shows a bounded, retryable error and the
canonical Release entry point.

Download, installation, and restart never happen automatically. Download/install and restart each
require confirmation; restart shuts down the proxy, balance, database, and recovery services through
the existing graceful shutdown path. Only one update operation is allowed at a time, an old generation
cannot overwrite newer state, and the progress channel does not trigger global query invalidation.

## Outbound proxy

In-app update checks (`latest.json` metadata) and update-package downloads both use Settings' "global
outbound proxy", sharing one set of proxy settings with inference, balance, model discovery, and
similar requests.

"Follow system" uses macOS's manual HTTP/HTTPS/SOCKS proxies and bypass rules; when no applicable
proxy is configured and automatic proxy is disabled it connects directly. "Custom proxy" overrides
the system configuration. Loopback targets (`localhost`, `127.0.0.1`, `::1`, and mapped forms) are
always direct, so the QA loopback updater endpoint (`AI_ROUTER_QA_UPDATER_ENDPOINT`) keeps its
semantics. Environment-variable proxies do not change the decision.

PAC/WPAD is not executed: a mixed configuration uses the applicable manual proxy or bypass rules; when
only automatic configuration is available, the system read fails, or the configuration is invalid,
external requests are rejected and the user is prompted to check system settings or switch to a custom
proxy. When the selected proxy is unreachable it also does not fall back to a direct connection.
Manual checks show a bounded error while automatic checks stay quiet and keep the existing failure
cadence. Settings' "Test connection" verifies only proxy-endpoint connectivity, not GitHub
reachability.

Update checks and downloads each capture an immutable policy and re-decide routing for the initial
target and each redirect. A cached `Update` adopts the current policy when downloaded later, and does
not re-check or replace the selected version; an in-progress operation keeps using the old snapshot.
The client is still injected through the updater plugin's `configure_client`, and downloading,
signature verification, and installation are still the plugin's responsibility.

This setting applies only to HTTP requests the app itself sends (updates, inference forwarding,
balance, model discovery, and so on). The "sync official site" in Settings loads the official pricing
page in a hidden read-only WebView and belongs to the platform WebView network stack: its egress is
decided by the macOS system network configuration (system HTTP/HTTPS/SOCKS proxies, the exception
list, and PAC) and it neither reads "custom proxy" nor reads environment-variable proxies. If the
system has no proxy configured while the target network requires one, the sync fails closed and keeps
the previous local price table, without silently falling back or reporting a degraded success.

## First install and Gatekeeper

Ordinary users download only the DMG. Because there is no Apple Developer ID and no notarization,
macOS may block the first open. Documentation and support recommend only the system path: "System
Settings -> Privacy & Security", confirm `AI Router.app` under "Security", then choose "Open Anyway".
Do not suggest `xattr`, `spctl --master-disable`, or any other terminal bypass.

A source build does not embed the official updater public key and is not part of the official update
chain. The first updater-capable stable version must be installed via the DMG as a manual bridge
version; only official versions that already embed the trusted public key can accept subsequent
in-app updates.

## Key backup and rotation

`TAURI_SIGNING_PRIVATE_KEY`, the password, and the release public-key value are configured only in the
protected GitHub `release` environment. The private key must not enter git, workflow arguments, build
output, the app bundle, logs, or pull-request jobs. Keep at least one encrypted, offline backup that
has been recovery-drilled, and separate backup access from GitHub environment approval.

Normal rotation uses a bridge version: sign with the old private key a stable version that is still
trusted by old versions but embeds the new public key; after the adoption window is confirmed, the
next version is signed with the new private key. The old private key stays offline and recoverable
until the bridge adoption window ends. If the old private key has already leaked, it can no longer be
used to sign a bridge version; stop in-app releases immediately, revoke the environment value, and
require users to upgrade manually through a new DMG that clearly states the hashes and provenance.

## Failure recovery

The workflow first creates or verifies an unpublished draft. If any step fails — build, bundle check,
updater signature verification, `latest.json`, checksums, remote read-back, or provenance — the
Release must stay a draft. A rerun of the same tag may only clean up and repair that draft; once
published, the tag and assets are treated as immutable, and any correction uses a higher patch
version.

Release operations and GitHub protection settings are in [Stable release operations](./releasing.md)
and [GitHub CI and security settings](./github-security-settings.md).
