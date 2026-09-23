/**
 * Model-forwarding display helpers shared by the Usage records table and the
 * tray preview. Rust owns the same classification for statistics; this
 * predicate only drives presentation and must stay aligned with
 * `model_verdict` in `crates/router-core/src/domain.rs`.
 */

/**
 * Reports whether the upstream response reported a different model than the
 * client requested. Both sides are trimmed and compared case-insensitively so
 * casing or whitespace noise stays matched, while a different identifier such
 * as a dated snapshot suffix counts as redirected. A missing or blank actual
 * model is unreported, never redirected.
 *
 * Rust compares with `eq_ignore_ascii_case`; JavaScript lowercasing is
 * Unicode-aware. Real model identifiers are ASCII, so both sides agree on any
 * reachable input.
 */
export function isRedirectedModel(
  requested: string | null,
  actual: string | null,
): boolean {
  const actualModel = actual?.trim() ?? "";
  if (actualModel === "") return false;
  return (requested?.trim() ?? "").toLowerCase() !== actualModel.toLowerCase();
}
