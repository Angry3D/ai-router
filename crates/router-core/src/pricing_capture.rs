//! Contract of the pricing payload captured from the official pricing page.
//!
//! The manual "同步官网" action renders
//! <https://developers.openai.com/api/docs/pricing/> in a hidden, isolated
//! `WebView` (see `src-tauri/src/pricing_sync.rs`). The page runs a pure DOM
//! extraction script (`fixtures/pricing-capture-script.js`) that reports both
//! billed service tiers by navigating to
//! `airouter-pricing-capture://v1/<payload>`, where `<payload>` is the
//! base64url encoding of one JSON document.
//!
//! This module owns that contract: it decodes the navigation payload, applies
//! the exact decimal money grammar, keeps only `gpt-` model identifiers, pins
//! the context-band boundary, and turns everything into the
//! [`LocalPricingTable`] the local store persists. The official page stays the
//! authority for the amounts, so ratio inconsistencies are reported as hints
//! instead of rejecting the capture.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use thiserror::Error;
use url::Url;

use crate::pricing::{
    CatalogBand, CatalogTableRow, CatalogTier, EffectiveCatalog, MAX_MODEL_ID_BYTES, ModelRate,
};
use crate::pricing_local::{
    LOCAL_PRICING_SCHEMA_VERSION, LocalPricingTable, LocalPricingTier, LocalPricingTiers,
    parse_optional_rate_micro_usd, parse_rate_micro_usd,
};

/// Scheme the extraction script navigates to when it reports a capture.
pub const CAPTURE_SCHEME: &str = "airouter-pricing-capture";
/// Host segment of the capture URL, which also names the payload version.
pub const CAPTURE_VERSION: &str = "v1";
/// Upper bound of the encoded capture payload carried by one navigation.
pub const MAX_CAPTURE_PAYLOAD_BYTES: usize = 256 * 1024;
/// Last input-token count billed with the short context band.
pub const SHORT_BAND_MAXIMUM_TOKENS: i64 = 272_000;
/// Largest accepted multiple of the embedded baseline rate of the same model.
pub const MAX_BASELINE_MULTIPLE: i64 = 1000;
/// Upper bound of the diagnostic text a failed capture reports.
pub const MAX_CAPTURE_REASON_BYTES: usize = 200;
/// Version suffix that marks a catalog version as locally synchronized.
pub const SYNCED_VERSION_INFIX: &str = "synced-";

/// Why one capture navigation could not become a local pricing table.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum CaptureError {
    #[error("the navigation is not a pricing capture")]
    NotCapture,
    #[error("the pricing capture payload is larger than the accepted limit")]
    TooLarge,
    #[error("the pricing capture payload is malformed")]
    Malformed,
    #[error("the pricing capture reported a failure: {0}")]
    Failed(String),
    #[error("the pricing capture is missing a required price column")]
    MissingColumn,
    #[error("the pricing capture carries an amount outside the exact money grammar")]
    Money,
    #[error("the pricing capture repeats a model identifier")]
    Duplicate,
    #[error("the pricing capture reports an unsupported context threshold")]
    Threshold,
    #[error("the pricing capture bills no model for a service tier")]
    Empty,
    #[error("the pricing capture reports a rate far above the embedded baseline")]
    Magnitude,
    #[error("the pricing capture does not satisfy the local pricing table contract")]
    Structure,
    #[error("the pricing capture window could not be created")]
    Window,
    #[error("the pricing capture window closed before it reported")]
    Closed,
    #[error("the pricing capture did not report within the synchronization budget")]
    Timeout,
    #[error("the pricing capture could not be stored as the local pricing table")]
    Store,
}

/// One advisory ratio observation; the official page stays authoritative.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CaptureHint {
    pub tier: CatalogTier,
    pub band: CatalogBand,
    pub model_id: String,
    pub detail: &'static str,
}

impl fmt::Display for CaptureHint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "model={} tier={} band={} detail={}",
            self.model_id,
            self.tier.service_tier(),
            self.band.as_str(),
            self.detail
        )
    }
}

/// A validated capture and the advisory hints it recorded.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CaptureOutcome {
    pub table: LocalPricingTable,
    pub hints: Vec<CaptureHint>,
}

/// One price column set of a single context band, as the page renders it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CapturedBand {
    #[serde(default)]
    input: Option<String>,
    #[serde(default)]
    cached_input: Option<String>,
    #[serde(default)]
    cache_write: Option<String>,
    #[serde(default)]
    output: Option<String>,
}

/// One model row as the page renders it, with its optional long band.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CapturedModel {
    id: String,
    short: CapturedBand,
    #[serde(default)]
    long: Option<CapturedBand>,
}

/// The single JSON document the extraction script reports.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CapturePayload {
    ok: bool,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    thresholds: Vec<String>,
    #[serde(default)]
    standard: Vec<CapturedModel>,
    #[serde(default)]
    priority: Vec<CapturedModel>,
}

/// Exact amounts of one context band, already converted to micro-USD.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct BandValues {
    input: i64,
    cached_input: i64,
    cache_write: Option<i64>,
    output: i64,
}

/// The rate rows one tier contributes, plus what they revealed.
struct TierRows {
    rates: Vec<ModelRate>,
    hints: Vec<CaptureHint>,
}

/// Parses the base64url payload of a capture navigation.
///
/// # Errors
///
/// Returns [`CaptureError::NotCapture`] when the URL is not a versioned capture
/// URL, [`CaptureError::TooLarge`] when the encoded payload exceeds
/// [`MAX_CAPTURE_PAYLOAD_BYTES`], and [`CaptureError::Malformed`] when the
/// payload is empty or is not base64url.
pub fn decode_capture_navigation(link: &Url) -> Result<Vec<u8>, CaptureError> {
    if link.scheme() != CAPTURE_SCHEME || link.host_str() != Some(CAPTURE_VERSION) {
        return Err(CaptureError::NotCapture);
    }
    let encoded = link.path().strip_prefix('/').unwrap_or_default();
    if encoded.is_empty() {
        return Err(CaptureError::Malformed);
    }
    if encoded.len() > MAX_CAPTURE_PAYLOAD_BYTES {
        return Err(CaptureError::TooLarge);
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| CaptureError::Malformed)?;
    if bytes.len() > MAX_CAPTURE_PAYLOAD_BYTES {
        return Err(CaptureError::TooLarge);
    }
    Ok(bytes)
}

/// Parses and validates one captured payload into a local pricing table.
///
/// `synced_at_ms` stamps the table and derives both tier versions; `source_url`
/// records where the payload was captured from.
///
/// # Errors
///
/// Returns a stable category when the payload is oversized, malformed, reports
/// a failed capture, bills no model for a tier, omits a price column, repeats a
/// model, carries money outside the exact grammar, reports an unsupported
/// context threshold, reports a rate above [`MAX_BASELINE_MULTIPLE`] times the
/// embedded baseline, or fails structural validation.
pub fn parse_capture_payload(
    bytes: &[u8],
    synced_at_ms: i64,
    source_url: &str,
) -> Result<CaptureOutcome, CaptureError> {
    if bytes.len() > MAX_CAPTURE_PAYLOAD_BYTES {
        return Err(CaptureError::TooLarge);
    }
    let payload: CapturePayload =
        serde_json::from_slice(bytes).map_err(|_| CaptureError::Malformed)?;
    if !payload.ok {
        return Err(CaptureError::Failed(bounded_reason(
            payload.reason.as_deref(),
        )));
    }
    let thresholds = normalize_thresholds(&payload.thresholds)?;
    let date = capture_date(synced_at_ms)?;
    let baseline = EffectiveCatalog::baseline();
    let mut hints = Vec::new();
    let mut tiers = Vec::with_capacity(CatalogTier::ALL.len());
    for (tier, models) in [
        (CatalogTier::Standard, &payload.standard),
        (CatalogTier::Priority, &payload.priority),
    ] {
        let built = build_tier(&baseline, tier, models)?;
        hints.extend(built.hints);
        let version = format!("{}{SYNCED_VERSION_INFIX}{date}", tier.version_prefix());
        tiers.push((version, built.rates));
    }
    validate_thresholds(&thresholds)?;
    let [standard, priority] = <[_; 2]>::try_from(tiers).map_err(|_| CaptureError::Structure)?;
    let table = LocalPricingTable {
        schema_version: LOCAL_PRICING_SCHEMA_VERSION,
        synced_at_ms,
        source_url: source_url.to_owned(),
        tiers: LocalPricingTiers {
            standard: LocalPricingTier {
                version: standard.0,
                models: standard.1,
            },
            priority: LocalPricingTier {
                version: priority.0,
                models: priority.1,
            },
        },
    };
    table.validate().map_err(|_| CaptureError::Structure)?;
    Ok(CaptureOutcome { table, hints })
}

/// Builds one tier's rate rows from the captured models of that tier.
fn build_tier(
    baseline: &EffectiveCatalog,
    tier: CatalogTier,
    models: &[CapturedModel],
) -> Result<TierRows, CaptureError> {
    let guard = BaselineRates::new(baseline, tier);
    let mut seen = BTreeSet::new();
    let mut rates = Vec::new();
    let mut hints = Vec::new();
    for model in models {
        // Only GPT catalogs are synchronized; every other family on the page
        // keeps pricing with the bundled baseline.
        if !synchronized_model_id(&model.id) {
            continue;
        }
        if !seen.insert(model.id.as_str()) {
            return Err(CaptureError::Duplicate);
        }
        let short = band_values(&model.short)?;
        guard.check(&model.id, CatalogBand::Short, &short)?;
        hints.extend(ratio_hints(
            tier,
            &model.id,
            CatalogBand::Short,
            &short,
            None,
        ));
        match &model.long {
            Some(long) => {
                let long = band_values(long)?;
                guard.check(&model.id, CatalogBand::Long, &long)?;
                hints.extend(ratio_hints(
                    tier,
                    &model.id,
                    CatalogBand::Long,
                    &long,
                    Some(&short),
                ));
                rates.push(rate_row(
                    &model.id,
                    None,
                    Some(SHORT_BAND_MAXIMUM_TOKENS),
                    &short,
                ));
                rates.push(rate_row(
                    &model.id,
                    Some(SHORT_BAND_MAXIMUM_TOKENS.saturating_add(1)),
                    None,
                    &long,
                ));
            }
            None => rates.push(rate_row(&model.id, None, None, &short)),
        }
    }
    if rates.is_empty() {
        return Err(CaptureError::Empty);
    }
    Ok(TierRows { rates, hints })
}

fn rate_row(
    model_id: &str,
    minimum_input_tokens: Option<i64>,
    maximum_input_tokens: Option<i64>,
    values: &BandValues,
) -> ModelRate {
    ModelRate {
        model_id: model_id.to_owned(),
        minimum_input_tokens,
        maximum_input_tokens,
        input: values.input,
        cached_input: values.cached_input,
        cache_write: values.cache_write,
        output: values.output,
    }
}

/// Converts one rendered band into exact micro-USD amounts.
///
/// A band that omits `cacheWrite` is priced without a cache-write rate, which is
/// how the page renders a model (or a family) that charges no separate rate.
fn band_values(band: &CapturedBand) -> Result<BandValues, CaptureError> {
    Ok(BandValues {
        input: parse_rate_micro_usd(required_column(band.input.as_deref())?)
            .map_err(|_| CaptureError::Money)?,
        cached_input: parse_rate_micro_usd(required_column(band.cached_input.as_deref())?)
            .map_err(|_| CaptureError::Money)?,
        cache_write: match band.cache_write.as_deref() {
            None => None,
            Some(value) => parse_optional_rate_micro_usd(value).map_err(|_| CaptureError::Money)?,
        },
        output: parse_rate_micro_usd(required_column(band.output.as_deref())?)
            .map_err(|_| CaptureError::Money)?,
    })
}

/// Rejects a band that omitted one of the four required price columns.
fn required_column(value: Option<&str>) -> Result<&str, CaptureError> {
    value.ok_or(CaptureError::MissingColumn)
}

/// Requires the accepted model identifiers the first version synchronizes.
fn synchronized_model_id(model_id: &str) -> bool {
    let Some(suffix) = model_id.strip_prefix("gpt-") else {
        return false;
    };
    !suffix.is_empty()
        && model_id.len() <= MAX_MODEL_ID_BYTES
        && suffix.chars().all(|value| {
            value.is_ascii_lowercase() || value.is_ascii_digit() || matches!(value, '.' | '-')
        })
}

/// Reads the embedded baseline rates of one tier, keyed by model and band.
struct BaselineRates(BTreeMap<(String, CatalogBand), CatalogTableRow>);

impl BaselineRates {
    fn new(baseline: &EffectiveCatalog, tier: CatalogTier) -> Self {
        let mut rates = BTreeMap::new();
        for row in baseline.rows_for(tier) {
            rates.insert((row.model_id.clone(), row.band), row);
        }
        Self(rates)
    }

    /// Rejects a captured band that exceeds the embedded baseline by a
    /// magnitude no promotion could explain.
    fn check(
        &self,
        model_id: &str,
        band: CatalogBand,
        values: &BandValues,
    ) -> Result<(), CaptureError> {
        let Some(baseline) = self.0.get(&(model_id.to_owned(), band)) else {
            return Ok(());
        };
        let columns = [
            (values.input, baseline.input),
            (values.cached_input, baseline.cached_input),
            (values.output, baseline.output),
        ];
        let cache_write = match (values.cache_write, baseline.cache_write) {
            (Some(captured), Some(embedded)) => Some((captured, embedded)),
            _ => None,
        };
        for (captured, embedded) in columns.into_iter().chain(cache_write) {
            if embedded > 0
                && i128::from(captured) > i128::from(embedded) * i128::from(MAX_BASELINE_MULTIPLE)
            {
                return Err(CaptureError::Magnitude);
            }
        }
        Ok(())
    }
}

/// Records ratio inconsistencies without rejecting the capture.
fn ratio_hints(
    tier: CatalogTier,
    model_id: &str,
    band: CatalogBand,
    values: &BandValues,
    short: Option<&BandValues>,
) -> Vec<CaptureHint> {
    let mut hints = Vec::new();
    let hint = |detail| CaptureHint {
        tier,
        band,
        model_id: model_id.to_owned(),
        detail,
    };
    if i128::from(values.cached_input) * 10 != i128::from(values.input) {
        hints.push(hint("cached_input_is_not_10_percent_of_input"));
    }
    if let Some(cache_write) = values.cache_write
        && i128::from(cache_write) * 4 != i128::from(values.input) * 5
    {
        hints.push(hint("cache_write_is_not_1_25x_input"));
    }
    if let Some(short) = short {
        if i128::from(values.input) != i128::from(short.input) * 2 {
            hints.push(hint("long_input_is_not_2x_short_input"));
        }
        if i128::from(values.output) * 2 != i128::from(short.output) * 3 {
            hints.push(hint("long_output_is_not_1_5x_short_output"));
        }
    }
    hints
}

/// Normalizes the threshold tokens the page showed into token counts.
fn normalize_thresholds(tokens: &[String]) -> Result<BTreeSet<i64>, CaptureError> {
    let mut counts = BTreeSet::new();
    for token in tokens {
        counts.insert(parse_threshold_token(token).ok_or(CaptureError::Threshold)?);
    }
    Ok(counts)
}

/// Requires the page to confirm the fixed short-band boundary when it shows one.
///
/// The boundary is authored here (272,000/272,001 tokens); a page that renders
/// no threshold text at all is accepted, while a page that confirms a different
/// threshold fails the capture instead of applying misaligned bands.
fn validate_thresholds(counts: &BTreeSet<i64>) -> Result<(), CaptureError> {
    if counts
        .iter()
        .any(|count| *count != SHORT_BAND_MAXIMUM_TOKENS)
    {
        return Err(CaptureError::Threshold);
    }
    Ok(())
}

/// Parses one rendered threshold token such as `272K` or `272,000`.
fn parse_threshold_token(token: &str) -> Option<i64> {
    let trimmed = token.trim();
    let (body, scale) = match trimmed.strip_suffix(['K', 'k']) {
        Some(rest) => (rest.trim(), 1000_i64),
        None => (trimmed, 1_i64),
    };
    if body.is_empty() {
        return None;
    }
    let mut whole = 0_i128;
    let mut fraction = 0_i128;
    let mut decimals = 0_u32;
    let mut point = false;
    let mut digits = false;
    for character in body.chars() {
        match character {
            '0'..='9' => {
                digits = true;
                let digit = i128::from(character.to_digit(10)?);
                if point {
                    decimals = decimals.checked_add(1)?;
                    fraction = fraction.checked_mul(10)?.checked_add(digit)?;
                } else {
                    whole = whole.checked_mul(10)?.checked_add(digit)?;
                }
            }
            ',' | '_' => {}
            '.' if !point => point = true,
            _ => return None,
        }
    }
    if !digits || decimals > 3 {
        return None;
    }
    while decimals < 3 {
        fraction = fraction.checked_mul(10)?;
        decimals += 1;
    }
    let value = whole
        .checked_mul(i128::from(scale))?
        .checked_add(fraction.checked_mul(i128::from(scale))? / 1000)?;
    i64::try_from(value).ok()
}

/// Derives the `YYYY-MM-DD` of a synchronized catalog version.
fn capture_date(synced_at_ms: i64) -> Result<String, CaptureError> {
    DateTime::<Utc>::from_timestamp_millis(synced_at_ms)
        .map(|time| time.format("%Y-%m-%d").to_string())
        .ok_or(CaptureError::Structure)
}

/// Bounds and sanitizes the failure text the extraction script reported.
fn bounded_reason(reason: Option<&str>) -> String {
    let mut bounded = String::new();
    for character in reason.unwrap_or_default().chars() {
        if character.is_control() {
            continue;
        }
        if bounded.len().saturating_add(character.len_utf8()) > MAX_CAPTURE_REASON_BYTES {
            break;
        }
        bounded.push(character);
    }
    if bounded.is_empty() {
        return "capture-failed".to_owned();
    }
    bounded
}

#[cfg(test)]
mod tests {
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use serde_json::{Value, json};

    use super::{
        CAPTURE_SCHEME, CAPTURE_VERSION, CaptureError, CaptureHint, CaptureOutcome,
        MAX_CAPTURE_PAYLOAD_BYTES, SHORT_BAND_MAXIMUM_TOKENS, decode_capture_navigation,
        parse_capture_payload,
    };
    use crate::pricing::{CatalogBand, CatalogTier, ModelRate};

    /// Local time of 2026-09-21, so both synchronized versions are stable.
    const SYNCED_AT_MS: i64 = 1_790_000_000_000;
    const SOURCE_URL: &str = "https://developers.openai.com/api/docs/pricing/";
    const PAYLOAD: &str = include_str!("../../../fixtures/pricing-capture-payload.json");

    fn fixture() -> Value {
        serde_json::from_str(PAYLOAD).expect("the capture payload fixture is valid JSON")
    }

    fn parse(mutate: impl FnOnce(&mut Value)) -> Result<CaptureOutcome, CaptureError> {
        let mut payload = fixture();
        mutate(&mut payload);
        let bytes = serde_json::to_vec(&payload).expect("the fixture serializes");
        parse_capture_payload(&bytes, SYNCED_AT_MS, SOURCE_URL)
    }

    fn accepted(mutate: impl FnOnce(&mut Value)) -> CaptureOutcome {
        parse(mutate).expect("the capture is accepted")
    }

    #[test]
    fn fixture_payload_parses_into_the_synchronized_local_table() {
        let outcome = accepted(|_| {});

        assert_eq!(
            outcome.table.tiers.standard.version,
            "openai-standard-synced-2026-09-21"
        );
        assert_eq!(
            outcome.table.tiers.priority.version,
            "openai-priority-synced-2026-09-21"
        );
        assert_eq!(outcome.table.synced_at_ms, SYNCED_AT_MS);
        assert_eq!(outcome.table.source_url, SOURCE_URL);
        assert!(outcome.hints.is_empty(), "{:?}", outcome.hints);
        assert!(outcome.table.validate().is_ok());

        let standard = &outcome.table.tiers.standard.models;
        // `chat-latest` is not a GPT model, so the page's other families stay
        // bundled; every accepted id is synchronized.
        assert_eq!(
            standard
                .iter()
                .map(|rate| rate.model_id.as_str())
                .collect::<Vec<_>>(),
            [
                "gpt-6-astra",
                "gpt-6-astra",
                "gpt-6-sol",
                "gpt-6-sol",
                "gpt-6-luna",
                "gpt-6-luna",
                "gpt-5.3-codex",
                "gpt-rosalind-research"
            ]
        );
        assert_eq!(
            standard[0],
            ModelRate {
                model_id: "gpt-6-astra".to_owned(),
                minimum_input_tokens: None,
                maximum_input_tokens: Some(SHORT_BAND_MAXIMUM_TOKENS),
                input: 10_000_000,
                cached_input: 1_000_000,
                cache_write: Some(12_500_000),
                output: 50_000_000,
            }
        );
        assert_eq!(
            standard[1].minimum_input_tokens,
            Some(SHORT_BAND_MAXIMUM_TOKENS + 1)
        );
        assert_eq!(standard[1].input, 20_000_000);
        // The Specialized table renders three rates and no long band, so the
        // model prices `[0, ∞)` with one row and no cache-write rate.
        assert_eq!(
            standard[6],
            ModelRate {
                model_id: "gpt-5.3-codex".to_owned(),
                minimum_input_tokens: None,
                maximum_input_tokens: None,
                input: 1_750_000,
                cached_input: 175_000,
                cache_write: None,
                output: 14_000_000,
            }
        );
        assert_eq!(
            standard[7],
            ModelRate {
                model_id: "gpt-rosalind-research".to_owned(),
                minimum_input_tokens: None,
                maximum_input_tokens: None,
                input: 5_000_000,
                cached_input: 500_000,
                cache_write: None,
                output: 25_000_000,
            }
        );
        assert_eq!(
            outcome.table.tiers.priority.models[0].input, 20_000_000,
            "Fast mode is the Priority tier"
        );
    }

    #[test]
    fn capture_navigation_round_trips_and_rejects_foreign_or_oversized_links() {
        let encoded = URL_SAFE_NO_PAD.encode(PAYLOAD.as_bytes());
        let link = url::Url::parse(&format!("{CAPTURE_SCHEME}://{CAPTURE_VERSION}/{encoded}"))
            .expect("the capture link parses");
        assert_eq!(
            decode_capture_navigation(&link).expect("the capture decodes"),
            PAYLOAD.as_bytes()
        );

        let foreign = url::Url::parse("https://developers.openai.com/api/docs/pricing/")
            .expect("the pricing link parses");
        assert_eq!(
            decode_capture_navigation(&foreign),
            Err(CaptureError::NotCapture)
        );

        let empty = url::Url::parse(&format!("{CAPTURE_SCHEME}://{CAPTURE_VERSION}/"))
            .expect("the empty capture link parses");
        assert_eq!(
            decode_capture_navigation(&empty),
            Err(CaptureError::Malformed)
        );

        let oversized = url::Url::parse(&format!(
            "{CAPTURE_SCHEME}://{CAPTURE_VERSION}/{}",
            "A".repeat(MAX_CAPTURE_PAYLOAD_BYTES + 1)
        ))
        .expect("the oversized capture link parses");
        assert_eq!(
            decode_capture_navigation(&oversized),
            Err(CaptureError::TooLarge)
        );

        let not_base64 = url::Url::parse(&format!("{CAPTURE_SCHEME}://{CAPTURE_VERSION}/%%%"))
            .expect("the malformed capture link parses");
        assert_eq!(
            decode_capture_navigation(&not_base64),
            Err(CaptureError::Malformed)
        );
    }

    #[test]
    fn parse_rejects_missing_malformed_or_unknown_payloads() {
        for column in ["input", "cachedInput", "output"] {
            assert_eq!(
                parse(|payload| {
                    payload["standard"][0]["short"]
                        .as_object_mut()
                        .expect("the short band is an object")
                        .remove(column);
                }),
                Err(CaptureError::MissingColumn),
                "{column} is required"
            );
        }
        // A table that renders no cache-write column bills no cache-write rate.
        let without_cache_write = accepted(|payload| {
            payload["standard"][0]["short"]
                .as_object_mut()
                .expect("the short band is an object")
                .remove("cacheWrite");
            payload["standard"][0]["long"]
                .as_object_mut()
                .expect("the long band is an object")
                .remove("cacheWrite");
        });
        assert_eq!(
            without_cache_write.table.tiers.standard.models[0].cache_write,
            None
        );
        assert_eq!(
            without_cache_write.table.tiers.standard.models[1].cache_write,
            None
        );
        assert_eq!(
            parse(|payload| payload["standard"][0]["short"]["input"] = json!("$1e3")),
            Err(CaptureError::Money)
        );
        assert_eq!(
            parse(|payload| payload["standard"][0]["short"]["cachedInput"] = json!("-")),
            Err(CaptureError::Money)
        );
        assert_eq!(
            parse(|payload| payload["standard"][0]["short"]["volume"] = json!("$0.01")),
            Err(CaptureError::Malformed)
        );
        assert_eq!(
            parse(|payload| {
                payload["unexpected"] = json!(true);
            }),
            Err(CaptureError::Malformed)
        );
        assert_eq!(
            parse_capture_payload(b"{ not json", SYNCED_AT_MS, SOURCE_URL),
            Err(CaptureError::Malformed)
        );
        assert_eq!(
            parse_capture_payload(
                &vec![b' '; MAX_CAPTURE_PAYLOAD_BYTES + 1],
                SYNCED_AT_MS,
                SOURCE_URL
            ),
            Err(CaptureError::TooLarge)
        );
        assert_eq!(
            parse_capture_payload(b"{\"ok\":false}", SYNCED_AT_MS, SOURCE_URL),
            Err(CaptureError::Failed("capture-failed".to_owned()))
        );
        assert_eq!(
            parse_capture_payload(
                b"{\"ok\":false,\"reason\":\"pricing-table-not-found:standard\"}",
                SYNCED_AT_MS,
                SOURCE_URL
            ),
            Err(CaptureError::Failed(
                "pricing-table-not-found:standard".to_owned()
            ))
        );
    }

    #[test]
    fn parse_rejects_duplicate_or_missing_models() {
        assert_eq!(
            parse(|payload| payload["standard"][1]["id"] = json!("gpt-6-astra")),
            Err(CaptureError::Duplicate)
        );
        assert_eq!(
            parse(|payload| payload["priority"] = json!([])),
            Err(CaptureError::Empty)
        );
        assert_eq!(
            parse(|payload| {
                payload["standard"] = json!([
                    {
                        "id": "chat-latest",
                        "short": {
                            "input": "$5.00",
                            "cachedInput": "$0.50",
                            "output": "$30.00"
                        }
                    }
                ]);
            }),
            Err(CaptureError::Empty),
            "a tier without any GPT model is not synchronized"
        );
    }

    #[test]
    fn parse_requires_the_page_to_confirm_the_short_band_boundary_when_it_shows_one() {
        assert_eq!(
            parse(|payload| payload["thresholds"] = json!(["300K"])),
            Err(CaptureError::Threshold)
        );
        assert_eq!(
            parse(|payload| payload["thresholds"] = json!(["272K", "128K"])),
            Err(CaptureError::Threshold)
        );
        assert_eq!(
            parse(|payload| payload["thresholds"] = json!([])),
            Ok(accepted(|_| {})),
            "the live page renders no threshold text, so the authored boundary is used"
        );
        assert_eq!(
            parse(|payload| payload["thresholds"] = json!(["272K"])),
            Ok(accepted(|_| {})),
            "a page that confirms the authored boundary is accepted"
        );
        assert_eq!(
            parse(|payload| payload["thresholds"] = json!(["272,000"])),
            Ok(accepted(|_| {})),
            "the boundary may also be rendered with thousands separators"
        );
    }

    #[test]
    fn parse_rejects_amounts_far_above_the_embedded_baseline() {
        // The embedded `gpt-6-luna` short input is `$0.10`; `$100.01` is more
        // than 1000 times that, so the capture is rejected instead of applied.
        assert_eq!(
            parse(|payload| payload["standard"][2]["short"]["input"] = json!("$100.01")),
            Err(CaptureError::Magnitude)
        );
        // A model the embedded baseline does not know has nothing to compare.
        let unknown = accepted(|payload| {
            payload["standard"] = json!([
                {
                    "id": "gpt-9-unknown",
                    "short": {
                        "input": "$999.00",
                        "cachedInput": "$99.90",
                        "cacheWrite": "-",
                        "output": "$999.00"
                    }
                }
            ]);
        });
        assert_eq!(unknown.table.tiers.standard.models[0].input, 999_000_000);
    }

    #[test]
    fn parse_records_ratio_hints_without_rejecting_the_capture() {
        let outcome = accepted(|payload| {
            payload["priority"][2]["short"]["cachedInput"] = json!("$0.03");
        });

        assert_eq!(
            outcome.hints,
            vec![CaptureHint {
                tier: CatalogTier::Priority,
                band: CatalogBand::Short,
                model_id: "gpt-6-luna".to_owned(),
                detail: "cached_input_is_not_10_percent_of_input",
            }]
        );
        assert_eq!(outcome.table.tiers.priority.models[4].cached_input, 30_000);

        let long_band = accepted(|payload| {
            payload["standard"][0]["long"]["output"] = json!("$80.00");
        });
        assert_eq!(
            long_band.hints,
            vec![CaptureHint {
                tier: CatalogTier::Standard,
                band: CatalogBand::Long,
                model_id: "gpt-6-astra".to_owned(),
                detail: "long_output_is_not_1_5x_short_output",
            }]
        );
        assert_eq!(long_band.table.tiers.standard.models[1].output, 80_000_000);
    }
}
