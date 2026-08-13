use std::collections::{BTreeMap, HashSet};

use serde::Deserialize;

pub const CATALOG_VERSION: &str = "openai-standard-2026-07-27";
pub const PRIORITY_CATALOG_VERSION: &str = "openai-priority-2026-07-28";
const STANDARD_CATALOG_JSON: &str =
    include_str!("../pricing/catalogs/openai-standard-2026-07-27.json");
const PRIORITY_CATALOG_JSON: &str =
    include_str!("../pricing/catalogs/openai-priority-2026-07-28.json");

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CatalogTier {
    Standard,
    Priority,
}

impl CatalogTier {
    const fn version(self) -> &'static str {
        match self {
            Self::Standard => CATALOG_VERSION,
            Self::Priority => PRIORITY_CATALOG_VERSION,
        }
    }

    const fn service_tier(self) -> &'static str {
        match self {
            Self::Standard => "default",
            Self::Priority => "priority",
        }
    }

    const fn captured_at(self) -> &'static str {
        match self {
            Self::Standard => "2026-07-27",
            Self::Priority => "2026-07-28",
        }
    }

    const fn effective_at(self) -> Option<&'static str> {
        match self {
            Self::Standard => Some("2026-07-27"),
            Self::Priority => None,
        }
    }

    const fn sources(self) -> &'static [&'static str] {
        match self {
            Self::Standard => &[
                "https://developers.openai.com/api/docs/pricing/",
                "https://developers.openai.com/api/docs/guides/prompt-caching/",
            ],
            Self::Priority => &[
                "https://learn.chatgpt.com/docs/agent-configuration/speed#fast-mode",
                "https://developers.openai.com/api/docs/pricing/",
                "https://developers.openai.com/api/docs/guides/prompt-caching/",
            ],
        }
    }

    const fn model_ids(self) -> &'static [&'static str] {
        match self {
            Self::Standard => &[
                "gpt-5.6-sol",
                "gpt-5.6-terra",
                "gpt-5.6-luna",
                "gpt-5.5",
                "gpt-5.4",
                "gpt-5.4-mini",
                "gpt-5.4-nano",
                "gpt-5.2",
                "gpt-5.1",
                "gpt-5",
                "gpt-5-mini",
                "gpt-5-nano",
                "gpt-4.1",
                "gpt-4.1-mini",
                "gpt-4.1-nano",
                "o3",
                "o4-mini",
                "codex-mini-latest",
                "gpt-5-codex",
                "gpt-5.3-codex",
                "gpt-5.1-codex",
                "gpt-5.1-codex-mini",
                "gpt-5.2-codex",
            ],
            Self::Priority => &["gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna"],
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CostStatus {
    Exact,
    Partial,
    Unavailable,
    NotApplicable,
}

impl CostStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Exact => "exact",
            Self::Partial => "partial",
            Self::Unavailable => "unavailable",
            Self::NotApplicable => "not_applicable",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "exact" => Some(Self::Exact),
            "partial" => Some(Self::Partial),
            "unavailable" => Some(Self::Unavailable),
            "not_applicable" => Some(Self::NotApplicable),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UsageObservation<'a> {
    pub requested_model: Option<&'a str>,
    pub actual_model: Option<&'a str>,
    pub forwarded_service_tier: Option<&'a str>,
    pub actual_service_tier: Option<&'a str>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub total_tokens: Option<i64>,
    pub cached_input_tokens: Option<i64>,
    pub cache_write_input_tokens: Option<i64>,
    pub possible_model_work: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PricedUsage {
    pub catalog_version: Option<&'static str>,
    pub status: CostStatus,
    pub amount_pico_usd: Option<i64>,
}

#[derive(Clone, Debug, Deserialize)]
struct Catalog {
    version: String,
    captured_at: String,
    effective_at: Option<String>,
    currency: String,
    unit: String,
    service_tier: String,
    sources: Vec<String>,
    models: Vec<ModelRate>,
}

#[derive(Clone, Debug, Deserialize)]
struct ModelRate {
    model_id: String,
    minimum_input_tokens: Option<i64>,
    maximum_input_tokens: Option<i64>,
    input: i64,
    cached_input: i64,
    cache_write: Option<i64>,
    output: i64,
}

fn catalog(tier: CatalogTier) -> Catalog {
    let source = match tier {
        CatalogTier::Standard => STANDARD_CATALOG_JSON,
        CatalogTier::Priority => PRIORITY_CATALOG_JSON,
    };
    serde_json::from_str(source).expect("bundled pricing catalog must be valid")
}

/// Validates immutable catalog metadata and every exact model-rate row.
///
/// # Errors
///
/// Returns a stable category when metadata or any rate row is invalid.
pub fn validate_bundled_catalog() -> Result<(), &'static str> {
    validate_catalog(catalog(CatalogTier::Standard), CatalogTier::Standard)?;
    validate_catalog(catalog(CatalogTier::Priority), CatalogTier::Priority)
}

fn validate_catalog(catalog: Catalog, tier: CatalogTier) -> Result<(), &'static str> {
    if catalog.version != tier.version()
        || catalog.captured_at != tier.captured_at()
        || catalog.effective_at.as_deref() != tier.effective_at()
        || catalog.currency != "USD"
        || catalog.unit != "micro_usd_per_million_tokens"
        || catalog.service_tier != tier.service_tier()
        || catalog
            .sources
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            != tier.sources()
        || catalog.models.is_empty()
    {
        return Err("invalid catalog metadata");
    }
    let mut bands = HashSet::new();
    let mut models = BTreeMap::<String, Vec<(Option<i64>, Option<i64>)>>::new();
    for rate in catalog.models {
        if rate.model_id.is_empty()
            || !bands.insert((
                rate.model_id.clone(),
                rate.minimum_input_tokens,
                rate.maximum_input_tokens,
            ))
            || rate.minimum_input_tokens.is_some_and(|value| value < 0)
            || rate.maximum_input_tokens.is_some_and(|value| value < 0)
            || matches!(
                (rate.minimum_input_tokens, rate.maximum_input_tokens),
                (Some(minimum), Some(maximum)) if minimum > maximum
            )
            || rate.input < 0
            || rate.cached_input < 0
            || rate.cache_write.is_some_and(|value| value < 0)
            || rate.output < 0
        {
            return Err("invalid catalog rate");
        }
        models
            .entry(rate.model_id)
            .or_default()
            .push((rate.minimum_input_tokens, rate.maximum_input_tokens));
    }
    if models.len() != tier.model_ids().len()
        || !tier
            .model_ids()
            .iter()
            .all(|model_id| models.contains_key(*model_id))
    {
        return Err("invalid catalog models");
    }
    for (model_id, mut model_bands) in models {
        model_bands.sort_by_key(|(minimum, _)| minimum.unwrap_or(0));
        let expected = if matches!(
            model_id.as_str(),
            "gpt-5.6-sol" | "gpt-5.6-terra" | "gpt-5.6-luna" | "gpt-5.5" | "gpt-5.4"
        ) {
            &[(None, Some(272_000)), (Some(272_001), None)][..]
        } else {
            &[(None, None)][..]
        };
        if model_bands != expected {
            return Err("invalid catalog bands");
        }
    }
    Ok(())
}

#[must_use]
pub fn price_usage(observation: &UsageObservation<'_>) -> PricedUsage {
    if !observation.possible_model_work
        && observation.input_tokens.is_none()
        && observation.output_tokens.is_none()
        && observation.total_tokens.is_none()
    {
        return priced(CostStatus::NotApplicable, None, None);
    }
    let Some(tier) = resolve_catalog_tier(observation) else {
        return priced(CostStatus::Unavailable, None, None);
    };
    let Some(model_id) = observation.actual_model.or(observation.requested_model) else {
        return priced(CostStatus::Unavailable, None, None);
    };
    let catalog = catalog(tier);
    let Some(rate) = select_rate(&catalog, model_id, observation.input_tokens) else {
        return priced(CostStatus::Unavailable, None, None);
    };
    let Ok(input_amount) = input_amount(observation, rate) else {
        return priced(CostStatus::Unavailable, None, None);
    };
    let Ok(output_amount) = token_cost(observation.output_tokens, rate.output) else {
        return priced(CostStatus::Unavailable, None, None);
    };
    let totals_consistent = match (
        observation.input_tokens,
        observation.output_tokens,
        observation.total_tokens,
    ) {
        (Some(input), Some(output), Some(total)) if input >= 0 && output >= 0 && total >= 0 => {
            input.checked_add(output) == Some(total)
        }
        _ => false,
    };
    if totals_consistent
        && let (Some(input_amount), Some(output_amount)) = (input_amount, output_amount)
        && let Some(amount) = input_amount.checked_add(output_amount)
    {
        return priced(CostStatus::Exact, Some(amount), Some(tier.version()));
    }
    let amounts = [input_amount, output_amount];
    let has_known_amount = amounts.iter().any(Option::is_some);
    let known = amounts
        .into_iter()
        .flatten()
        .try_fold(0_i64, i64::checked_add);
    match (has_known_amount, known) {
        (true, Some(amount)) => priced(CostStatus::Partial, Some(amount), Some(tier.version())),
        (false, _) | (_, None) => priced(CostStatus::Unavailable, None, None),
    }
}

fn input_amount(observation: &UsageObservation<'_>, rate: &ModelRate) -> Result<Option<i64>, ()> {
    let (Some(input), Some(cached)) = (observation.input_tokens, observation.cached_input_tokens)
    else {
        return Ok(None);
    };
    let cache_write = match rate.cache_write {
        Some(_) => observation.cache_write_input_tokens.ok_or(())?,
        None => 0,
    };
    if input < 0 || cached < 0 || cache_write < 0 {
        return Err(());
    }
    let regular = input
        .checked_sub(cached)
        .and_then(|value| value.checked_sub(cache_write))
        .ok_or(())?;
    let amount = [
        (regular, rate.input),
        (cached, rate.cached_input),
        (cache_write, rate.cache_write.unwrap_or(0)),
    ]
    .into_iter()
    .try_fold(0_i128, |sum, (tokens, price)| {
        sum.checked_add(i128::from(tokens).checked_mul(i128::from(price))?)
    })
    .and_then(|value| i64::try_from(value).ok())
    .ok_or(())?;
    Ok(Some(amount))
}

fn select_rate<'a>(
    catalog: &'a Catalog,
    model_id: &str,
    input_tokens: Option<i64>,
) -> Option<&'a ModelRate> {
    let mut matching = catalog
        .models
        .iter()
        .filter(|rate| rate.model_id == model_id);
    let first = matching.next()?;
    if matching.next().is_none()
        && first.minimum_input_tokens.is_none()
        && first.maximum_input_tokens.is_none()
    {
        return Some(first);
    }
    let input_tokens = input_tokens.filter(|value| *value >= 0)?;
    catalog.models.iter().find(|rate| {
        rate.model_id == model_id
            && rate
                .minimum_input_tokens
                .is_none_or(|minimum| input_tokens >= minimum)
            && rate
                .maximum_input_tokens
                .is_none_or(|maximum| input_tokens <= maximum)
    })
}

fn token_cost(tokens: Option<i64>, rate: i64) -> Result<Option<i64>, ()> {
    let Some(tokens) = tokens else {
        return Ok(None);
    };
    if tokens < 0 {
        return Err(());
    }
    i128::from(tokens)
        .checked_mul(i128::from(rate))
        .and_then(|value| i64::try_from(value).ok())
        .map(Some)
        .ok_or(())
}

#[must_use]
pub fn fold_request_cost(costs: &[PricedUsage]) -> PricedUsage {
    let mut amount = 0_i128;
    let mut has_amount = false;
    let mut incomplete = false;
    let mut applicable = false;
    let mut common_catalog: Option<Option<&'static str>> = None;
    for cost in costs {
        match cost.status {
            CostStatus::Exact | CostStatus::Partial => {
                applicable = true;
                if let Some(value) = cost.amount_pico_usd {
                    let Some(next) = amount.checked_add(i128::from(value)) else {
                        return priced(CostStatus::Unavailable, None, None);
                    };
                    amount = next;
                    has_amount = true;
                    common_catalog = Some(match common_catalog {
                        None => cost.catalog_version,
                        Some(version) if version == cost.catalog_version => version,
                        Some(_) => None,
                    });
                }
                incomplete |= cost.status == CostStatus::Partial;
            }
            CostStatus::Unavailable => {
                applicable = true;
                incomplete = true;
            }
            CostStatus::NotApplicable => {}
        }
    }
    if !applicable {
        return priced(CostStatus::NotApplicable, None, None);
    }
    if !has_amount {
        return priced(CostStatus::Unavailable, None, None);
    }
    let Some(amount) = i64::try_from(amount).ok() else {
        return priced(CostStatus::Unavailable, None, None);
    };
    priced(
        if incomplete {
            CostStatus::Partial
        } else {
            CostStatus::Exact
        },
        Some(amount),
        common_catalog.flatten(),
    )
}

fn priced(
    status: CostStatus,
    amount_pico_usd: Option<i64>,
    catalog_version: Option<&'static str>,
) -> PricedUsage {
    PricedUsage {
        catalog_version,
        status,
        amount_pico_usd,
    }
}

fn resolve_catalog_tier(observation: &UsageObservation<'_>) -> Option<CatalogTier> {
    match (
        observation.forwarded_service_tier,
        observation.actual_service_tier,
    ) {
        (Some("priority"), None | Some("default" | "priority"))
        | (None | Some("auto" | "default"), Some("priority")) => Some(CatalogTier::Priority),
        (None | Some("default"), None | Some("default")) | (Some("auto"), Some("default")) => {
            Some(CatalogTier::Standard)
        }
        _ => None,
    }
}

#[must_use]
pub fn catalog_service_tier(catalog_version: &str) -> Option<&'static str> {
    match catalog_version {
        CATALOG_VERSION => Some("default"),
        PRIORITY_CATALOG_VERSION => Some("priority"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observation() -> UsageObservation<'static> {
        UsageObservation {
            requested_model: Some("gpt-5"),
            actual_model: None,
            forwarded_service_tier: None,
            actual_service_tier: Some("default"),
            input_tokens: Some(10),
            output_tokens: Some(2),
            total_tokens: Some(12),
            cached_input_tokens: Some(4),
            cache_write_input_tokens: None,
            possible_model_work: true,
        }
    }

    #[test]
    fn bundled_catalog_is_valid() {
        assert_eq!(validate_bundled_catalog(), Ok(()));
    }

    #[test]
    fn priority_rates_explicitly_double_every_supported_standard_dimension() {
        let standard = catalog(CatalogTier::Standard);
        let priority = catalog(CatalogTier::Priority);
        for priority_rate in &priority.models {
            let standard_rate = standard
                .models
                .iter()
                .find(|rate| {
                    rate.model_id == priority_rate.model_id
                        && rate.minimum_input_tokens == priority_rate.minimum_input_tokens
                        && rate.maximum_input_tokens == priority_rate.maximum_input_tokens
                })
                .expect("Priority row must have a matching Standard band");
            assert_eq!(
                priority_rate.input,
                standard_rate.input.checked_mul(2).unwrap()
            );
            assert_eq!(
                priority_rate.cached_input,
                standard_rate.cached_input.checked_mul(2).unwrap()
            );
            assert_eq!(
                priority_rate.cache_write,
                standard_rate
                    .cache_write
                    .and_then(|value| value.checked_mul(2))
            );
            assert_eq!(
                priority_rate.output,
                standard_rate.output.checked_mul(2).unwrap()
            );
        }
    }

    #[test]
    fn catalog_validation_rejects_invalid_metadata_rates_and_bands() {
        let mut unsupported_currency = catalog(CatalogTier::Standard);
        unsupported_currency.currency = "EUR".to_owned();
        assert_eq!(
            validate_catalog(unsupported_currency, CatalogTier::Standard),
            Err("invalid catalog metadata")
        );

        let mut unsupported_effective_date = catalog(CatalogTier::Standard);
        unsupported_effective_date.effective_at = None;
        assert_eq!(
            validate_catalog(unsupported_effective_date, CatalogTier::Standard),
            Err("invalid catalog metadata")
        );

        let mut invented_effective_date = catalog(CatalogTier::Priority);
        invented_effective_date.effective_at = Some("2026-07-28".to_owned());
        assert_eq!(
            validate_catalog(invented_effective_date, CatalogTier::Priority),
            Err("invalid catalog metadata")
        );

        let mut missing_model = catalog(CatalogTier::Priority);
        missing_model
            .models
            .retain(|rate| rate.model_id != "gpt-5.6-luna");
        assert_eq!(
            validate_catalog(missing_model, CatalogTier::Priority),
            Err("invalid catalog models")
        );

        let mut duplicate = catalog(CatalogTier::Standard);
        duplicate.models.push(duplicate.models[0].clone());
        assert_eq!(
            validate_catalog(duplicate, CatalogTier::Standard),
            Err("invalid catalog rate")
        );

        let mut negative = catalog(CatalogTier::Standard);
        negative.models[0].input = -1;
        assert_eq!(
            validate_catalog(negative, CatalogTier::Standard),
            Err("invalid catalog rate")
        );

        let mut gap = catalog(CatalogTier::Standard);
        let long_band = gap
            .models
            .iter_mut()
            .find(|rate| {
                rate.model_id == "gpt-5.6-sol" && rate.minimum_input_tokens == Some(272_001)
            })
            .expect("long-context catalog band");
        long_band.minimum_input_tokens = Some(272_002);
        assert_eq!(
            validate_catalog(gap, CatalogTier::Standard),
            Err("invalid catalog bands")
        );
    }

    #[test]
    fn exact_cost_uses_fixed_point_dimensions() {
        let result = price_usage(&observation());
        assert_eq!(result.status, CostStatus::Exact);
        assert_eq!(result.amount_pico_usd, Some(28_000_000));
    }

    #[test]
    fn preserves_unknown_and_non_standard_boundaries() {
        let mut unknown = observation();
        unknown.actual_model = Some("relay-alias");
        assert_eq!(price_usage(&unknown).status, CostStatus::Unavailable);
        let mut unsupported_priority_model = observation();
        unsupported_priority_model.forwarded_service_tier = Some("priority");
        unsupported_priority_model.actual_service_tier = None;
        assert_eq!(
            price_usage(&unsupported_priority_model).status,
            CostStatus::Unavailable
        );

        let mut unresolved_auto = observation();
        unresolved_auto.actual_service_tier = Some("auto");
        assert_eq!(
            price_usage(&unresolved_auto).status,
            CostStatus::Unavailable
        );
    }

    #[test]
    fn selects_long_context_rates_from_observed_input_tokens() {
        let mut short = observation();
        short.requested_model = Some("gpt-5.6-terra");
        short.input_tokens = Some(272_000);
        short.output_tokens = Some(1);
        short.total_tokens = Some(272_001);
        short.cached_input_tokens = Some(0);
        short.cache_write_input_tokens = Some(0);
        assert_eq!(price_usage(&short).amount_pico_usd, Some(680_015_000_000));

        let mut long = short;
        long.input_tokens = Some(272_001);
        long.total_tokens = Some(272_002);
        assert_eq!(price_usage(&long).amount_pico_usd, Some(1_360_027_500_000));
    }

    #[test]
    fn pre_cache_write_models_charge_observed_writes_as_uncached_input() {
        let mut value = observation();
        value.cached_input_tokens = Some(0);
        value.cache_write_input_tokens = Some(4);
        assert_eq!(price_usage(&value).amount_pico_usd, Some(32_500_000));
    }

    #[test]
    fn distinguishes_possible_model_work_from_pre_response_failure() {
        let mut value = observation();
        value.input_tokens = None;
        value.output_tokens = None;
        value.total_tokens = None;
        value.cached_input_tokens = None;
        assert_eq!(price_usage(&value).status, CostStatus::Unavailable);

        value.possible_model_work = false;
        assert_eq!(price_usage(&value).status, CostStatus::NotApplicable);
    }

    #[test]
    fn partial_and_overflow_costs_remain_explicit() {
        let mut partial = observation();
        partial.output_tokens = None;
        partial.total_tokens = None;
        let result = price_usage(&partial);
        assert_eq!(result.status, CostStatus::Partial);
        assert_eq!(result.amount_pico_usd, Some(8_000_000));
        assert_eq!(result.catalog_version, Some(CATALOG_VERSION));

        let mut overflow = observation();
        overflow.input_tokens = Some(i64::MAX);
        overflow.cached_input_tokens = Some(0);
        overflow.output_tokens = Some(0);
        overflow.total_tokens = Some(i64::MAX);
        let result = price_usage(&overflow);
        assert_eq!(result.status, CostStatus::Unavailable);
        assert_eq!(result.amount_pico_usd, None);
        assert_eq!(result.catalog_version, None);
    }

    #[test]
    fn folds_billable_fallback_attempts_as_lower_bound() {
        let result = fold_request_cost(&[
            price_usage(&observation()),
            PricedUsage {
                catalog_version: None,
                status: CostStatus::Unavailable,
                amount_pico_usd: None,
            },
        ]);
        assert_eq!(result.status, CostStatus::Partial);
        assert_eq!(result.amount_pico_usd, Some(28_000_000));
    }

    #[test]
    fn prices_verified_fast_request_with_priority_catalog() {
        let mut value = observation();
        value.requested_model = Some("gpt-5.6-sol");
        value.forwarded_service_tier = Some("priority");
        value.actual_service_tier = Some("default");
        value.input_tokens = Some(60_014);
        value.output_tokens = Some(40);
        value.total_tokens = Some(60_054);
        value.cached_input_tokens = Some(59_136);
        value.cache_write_input_tokens = Some(0);

        let result = price_usage(&value);
        assert_eq!(result.status, CostStatus::Exact);
        assert_eq!(result.amount_pico_usd, Some(70_316_000_000));
        assert_eq!(result.catalog_version, Some(PRIORITY_CATALOG_VERSION));

        value.forwarded_service_tier = Some("default");
        let standard = price_usage(&value);
        assert_eq!(standard.amount_pico_usd, Some(35_158_000_000));
        assert_eq!(standard.catalog_version, Some(CATALOG_VERSION));
    }

    #[test]
    fn tier_resolution_is_closed_and_explicit() {
        let mut value = observation();
        value.requested_model = Some("gpt-5.6-luna");
        value.cache_write_input_tokens = Some(0);

        for (requested, actual, expected) in [
            (Some("priority"), None, Some(PRIORITY_CATALOG_VERSION)),
            (
                Some("priority"),
                Some("priority"),
                Some(PRIORITY_CATALOG_VERSION),
            ),
            (
                Some("priority"),
                Some("default"),
                Some(PRIORITY_CATALOG_VERSION),
            ),
            (None, Some("priority"), Some(PRIORITY_CATALOG_VERSION)),
            (
                Some("auto"),
                Some("priority"),
                Some(PRIORITY_CATALOG_VERSION),
            ),
            (
                Some("default"),
                Some("priority"),
                Some(PRIORITY_CATALOG_VERSION),
            ),
            (None, None, Some(CATALOG_VERSION)),
            (None, Some("default"), Some(CATALOG_VERSION)),
            (Some("default"), None, Some(CATALOG_VERSION)),
            (Some("auto"), Some("default"), Some(CATALOG_VERSION)),
            (Some("auto"), None, None),
            (Some("auto"), Some("auto"), None),
            (Some("fast"), Some("default"), None),
            (Some("Priority"), None, None),
            (Some(" priority"), None, None),
            (Some("default"), Some("flex"), None),
        ] {
            value.forwarded_service_tier = requested;
            value.actual_service_tier = actual;
            assert_eq!(price_usage(&value).catalog_version, expected);
        }
    }

    #[test]
    fn folding_mixed_catalogs_suppresses_single_tier_provenance() {
        let result = fold_request_cost(&[
            PricedUsage {
                catalog_version: Some(CATALOG_VERSION),
                status: CostStatus::Exact,
                amount_pico_usd: Some(10),
            },
            PricedUsage {
                catalog_version: Some(PRIORITY_CATALOG_VERSION),
                status: CostStatus::Exact,
                amount_pico_usd: Some(20),
            },
        ]);
        assert_eq!(result.status, CostStatus::Exact);
        assert_eq!(result.amount_pico_usd, Some(30));
        assert_eq!(result.catalog_version, None);
    }
}
