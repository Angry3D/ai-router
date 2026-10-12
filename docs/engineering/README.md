# AI Router engineering docs

This is where the project contracts that external contributors must follow live.
It describes the current product and release boundaries, not a future roadmap.

- [Architecture and code boundaries](./architecture.md): ownership of processes, modules, state, and generated types.
- [Routing and resilience](./routing-resilience.md): Responses forwarding, automatic fallback, and diagnostic boundaries.
- [Data, privacy, and recovery](./data-privacy-recovery.md): local files, secrets, history, logs, and recovery points.
- [macOS native lifecycle](./native-lifecycle.md): production/QA isolation, the menu panel, and build safety.
- [Release and app updates](./application-updates.md): assets, signing, the state machine, bridge versions, and key rotation.
- [Stable release operations](./releasing.md): the protected environment, draft verification, publication, and failure recovery.
- [Development and delivery lifecycle](./development-lifecycle.md): natural-language control, branches, PRs, mainline, and release candidate boundaries.
- [Verification strategy by risk](./verification.md): choosing the minimum sufficient evidence for a given change.
- [GitHub CI and security settings](./github-security-settings.md): required checks, the ruleset, and the security feature handoff checklist.

Product support scope, build entry points, and collaboration rules are covered by the
[README](../../README.md), the [contributing guide](../../CONTRIBUTING.md), and the
[security policy](../../SECURITY.md).
