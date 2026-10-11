# Release notes

Every stable release requires one reviewed file named `v<version>.md`. The
version heading must match the immutable stable tag exactly.

```markdown
# AI Router release v0.1.4

## Highlights

- Describe the most important user-visible change.
- Add no more than three priority-ordered highlights.

## Fixes

- Describe a user-relevant fix.

## Notices

- Describe an action, compatibility detail, or migration note.
```

`Highlights` is required and contains one to three flat bullets. `Fixes` and
`Notices` are optional. Across all sections, use at most 20 unique bullets and
240 characters per bullet. Do not add paragraphs, nested lists, links, images,
HTML, code blocks, secrets, logs, local paths, or private user data.

Tools or AI may draft the file, but the release owner must review it before the
stable tag is created. Review that every bullet is accurate, useful to an
updating user, ordered by importance, and free of internal-only implementation
details. The protected release job rejects missing, mismatched, placeholder,
malformed, or oversized content.

Before authoring or approving the file, fix the candidate scope, ensure local
stable tags are current, and run `pnpm release:inventory`. Review every listed
commit and changed path. Represent every user-visible addition, behavior
change, fix, compatibility concern, migration action, or restart implication
in the notes, or record a concise exclusion reason for internal-only work such
as tests, dependency maintenance, or refactoring. Cross-check merged PRs and
completed task evidence when available. The inventory is local Git evidence,
not a semantic completeness proof; the release owner remains responsible for
accuracy, priority ordering, completeness, and final approval before tagging.
