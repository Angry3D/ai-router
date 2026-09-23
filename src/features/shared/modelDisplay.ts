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

/**
 * Requested-model-first label for the request-detail fact grid, which renders
 * the pair on one line as `requested（actual）` instead of the table's separate
 * redirect line. Rendered only when the pair is redirected, so a matched,
 * unreported, or blank reported model never gains a parenthesized suffix and an
 * unreported upstream never masquerades as the requested model. A row without a
 * client-requested model carries no pair to compare and falls back to the
 * reported model, then to `-`.
 */
export function formatModelWithActual(
  requested: string | null,
  actual: string | null,
): string {
  const requestedModel = (requested ?? "").trim();
  const actualModel = (actual ?? "").trim();
  if (requestedModel === "") {
    return actualModel === "" ? "-" : actualModel;
  }
  return isRedirectedModel(requested, actual)
    ? `${requestedModel}（${actualModel}）`
    : requestedModel;
}
