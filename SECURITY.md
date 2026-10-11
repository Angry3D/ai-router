# Security policy

## Supported scope

AI Router is at an early `0.1.x` stage. Security fixes only target the latest `main` and
the latest stable official version; no long-term maintenance is promised for older
commits, unofficial forks, self-modified binaries, or unsupported platforms. The
official DMG is ad-hoc signed, contains no Apple Developer ID identity, and is not
Apple-notarized.

## Private reporting

Report security issues through GitHub's
[Private vulnerability reporting](https://github.com/Angry3D/ai-router/security/advisories/new).
A report should include at least: the affected version or commit, macOS/Codex version,
risk impact, minimal reproduction steps using synthetic data, and the stable error code
shown in the UI.

Do not submit real API keys, Authorization headers, complete configuration (including
Codex configuration), SQLite databases, raw requests/responses, raw logs, recovery
points, or user absolute paths in public issues, discussions, PRs, screenshots, or
attachments. Do not access accounts, upstreams, or devices that are not yours in order
to demonstrate a problem.

If GitHub's private reporting entry point is temporarily unavailable, you may create a
public issue that contains no technical details or sensitive material, asking only for
the maintainers to provide a private contact channel. Do not add vulnerability details
before the maintainers confirm the channel.

## Handling

Maintainers make a best effort to acknowledge receipt, assess impact, and coordinate a
fix, but a personally maintained project provides no fixed response time or bug bounty.
Public disclosure timing should be agreed jointly after a fix and after affected users
can act.

The following are usually in security scope:

- leaks of the loopback boundary, gateway token, or upstream credentials;
- unauthorized Codex configuration overwrites, recovery, or ownership misjudgment;
- unauthorized exposure of SQLite, recovery points, logs, or request metadata;
- credentials and undeclared third-party content in public source, the build, or the
  dependency chain;
- updater private key leaks, `latest.json`/archive replacement, signature bypass, or
  release assets inconsistent with the tag/commit;
- native lifecycle problems that cross the production/QA data or bundle isolation.

For ordinary crashes, UI problems, compatibility suggestions, and upstream errors
without security impact, use the public issue process per [support](./SUPPORT.md).

## Security facts

The local proxy only listens on loopback. API Keys are stored as raw bytes in a
private-permission SQLite file; Keychain or application-layer encryption is not currently
used, and recovery points also contain critical configuration. Run logs are designed to
record only bounded fixed codes, but the full file should still be treated as sensitive
material.

The "Codex credentials" feature intentionally introduces two local read surfaces: it
reads the Cookie database of Chrome-family browsers on this machine read-only, and reads
the browser's secure storage key (`Chrome Safe Storage`) through the system keychain. The
first read triggers a system authorization prompt; if the user declines, it fails closed
and shows a stable error code. Decrypted Cookie values and session tokens exist only in
memory and are zeroed there, never written to logs, diagnostics, recovery points, or
request history. The feature only handles session Cookies for `chatgpt.com` and requires
the user to already be signed in to chatgpt.com on this machine. See
[data, privacy, and recovery](./docs/engineering/data-privacy-recovery.md) for more
boundaries.

Official in-app updates rely on both the canonical GitHub HTTPS boundary and the project
updater signature. ad-hoc macOS signing, SHA-256, or provenance cannot each replace the
updater signature check. The signing private key and password exist only in the GitHub
`release` environment, which requires human approval, and in offline backups, and never
enter the repository, the app bundle, logs, or a pull request job. See
[releases and application updates](./docs/engineering/application-updates.md) for key
rotation and leak handling.
