use std::{
    collections::{BTreeMap, HashSet},
    sync::{Arc, LazyLock},
};

use arc_swap::ArcSwap;
use serde::{Deserialize, Serialize};

use crate::pricing_local::{LocalPricingLoad, LocalPricingStatus, LocalPricingTable};

pub const CATALOG_VERSION: &str = "openai-standard-2026-09-29";
pub const PRIORITY_CATALOG_VERSION: &str = "openai-priority-2026-09-29";
const STANDARD_CATALOG_JSON: &str =
    include_str!("../pricing/catalogs/openai-standard-2026-09-29.json");
const PRIORITY_CATALOG_JSON: &str =
    include_str!("../pricing/catalogs/openai-priority-2026-09-29.json");

/// Version prefixes that bind every catalog version to its billing tier.
///
/// The bundled catalogs and locally synchronized catalogs share these prefixes,
/// so tier derivation never depends on a closed list of exact version strings.
pub const STANDARD_VERSION_PREFIX: &str = "openai-standard-";
pub const PRIORITY_VERSION_PREFIX: &str = "openai-priority-";
/// Upper bound for any persisted or generated catalog version.
pub const MAX_CATALOG_VERSION_BYTES: usize = 128;
/// Upper bound for the row count of one catalog tier.
pub const MAX_CATALOG_MODELS: usize = 512;
/// Upper bound for the context bands of one model.
pub const MAX_CATALOG_BANDS_PER_MODEL: usize = 2;
/// Upper bound for an exact model identifier.
pub const MAX_MODEL_ID_BYTES: usize = 128;
/// Upper bound for the provenance sources of one catalog.
pub const MAX_CATALOG_SOURCES: usize = 8;
/// Upper bound for one provenance source string.
pub const MAX_CATALOG_SOURCE_BYTES: usize = 512;
/// Upper bound for one rate: 1000 USD per million Tokens.
pub const MAX_RATE_MICRO_USD: i64 = 1_000_000_000;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum CatalogTier {
    Standard,
    Priority,
}

impl CatalogTier {
    pub const ALL: [Self; 2] = [Self::Standard, Self::Priority];

    const fn version(self) -> &'static str {
        match self {
            Self::Standard => CATALOG_VERSION,
            Self::Priority => PRIORITY_CATALOG_VERSION,
        }
    }

    /// Returns the raw service tier this catalog prices.
    #[must_use]
    pub const fn service_tier(self) -> &'static str {
        match self {
            Self::Standard => "default",
            Self::Priority => "priority",
        }
    }

    /// Returns the version prefix that identifies this tier.
    #[must_use]
    pub const fn version_prefix(self) -> &'static str {
        match self {
            Self::Standard => STANDARD_VERSION_PREFIX,
            Self::Priority => PRIORITY_VERSION_PREFIX,
        }
    }

    /// Derives the billing tier from a catalog version prefix.
    #[must_use]
    pub fn from_version(version: &str) -> Option<Self> {
        if version.starts_with(STANDARD_VERSION_PREFIX) {
            Some(Self::Standard)
        } else if version.starts_with(PRIORITY_VERSION_PREFIX) {
            Some(Self::Priority)
        } else {
            None
        }
    }

    const fn captured_at(self) -> &'static str {
        match self {
            Self::Standard | Self::Priority => "2026-09-29",
        }
    }

    const fn effective_at(self) -> Option<&'static str> {
        match self {
            Self::Standard => Some("2026-09-29"),
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
    /// Owned because a synchronized local catalog supplies its own version.
    pub catalog_version: Option<Arc<str>>,
    pub status: CostStatus,
    pub amount_pico_usd: Option<i64>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Catalog {
    pub version: String,
    pub captured_at: String,
    pub effective_at: Option<String>,
    pub currency: String,
    pub unit: String,
    pub service_tier: String,
    pub sources: Vec<String>,
    pub models: Vec<ModelRate>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ModelRate {
    pub model_id: String,
    #[serde(default)]
    pub minimum_input_tokens: Option<i64>,
    #[serde(default)]
    pub maximum_input_tokens: Option<i64>,
    pub input: i64,
    pub cached_input: i64,
    #[serde(default)]
    pub cache_write: Option<i64>,
    pub output: i64,
}

/// Where one priced row comes from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CatalogRowSource {
    /// The bundled baseline catalog.
    Bundled,
    /// The locally synchronized catalog (`local-pricing.json`).
    Official,
}

/// Which context band one priced row describes.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum CatalogBand {
    Short,
    Long,
}

impl CatalogBand {
    /// Stable lowercase label of this band.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Short => "short",
            Self::Long => "long",
        }
    }
}

/// One read-only pricing row for the settings pricing table.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogTableRow {
    pub model_id: String,
    pub band: CatalogBand,
    pub input: i64,
    pub cached_input: i64,
    pub cache_write: Option<i64>,
    pub output: i64,
    pub source: CatalogRowSource,
}

fn bundled_catalog(tier: CatalogTier) -> Catalog {
    let source = match tier {
        CatalogTier::Standard => STANDARD_CATALOG_JSON,
        CatalogTier::Priority => PRIORITY_CATALOG_JSON,
    };
    serde_json::from_str(source).expect("bundled pricing catalog must be valid")
}

/// Validates the structural invariants shared by bundled and local catalogs.
///
/// # Errors
///
/// Returns a stable category when metadata or any rate row is invalid.
pub fn validate_catalog_structure(catalog: &Catalog) -> Result<CatalogTier, &'static str> {
    let tier = CatalogTier::from_version(&catalog.version).ok_or("invalid catalog metadata")?;
    if catalog.version.len() > MAX_CATALOG_VERSION_BYTES
        || catalog.captured_at.is_empty()
        || catalog.captured_at.len() > 64
        || catalog
            .effective_at
            .as_deref()
            .is_some_and(|value| value.is_empty() || value.len() > 64)
        || catalog.currency != "USD"
        || catalog.unit != "micro_usd_per_million_tokens"
        || catalog.service_tier != tier.service_tier()
        || catalog.sources.len() > MAX_CATALOG_SOURCES
        || catalog
            .sources
            .iter()
            .any(|source| source.is_empty() || source.len() > MAX_CATALOG_SOURCE_BYTES)
    {
        return Err("invalid catalog metadata");
    }
    validate_model_rows(&catalog.models)?;
    Ok(tier)
}

/// Validates one exact catalog version against the tier it must describe.
///
/// # Errors
///
/// Returns a stable category when the version is empty, over-long, or carries a
/// prefix that belongs to another tier.
pub fn validate_tier_version(tier: CatalogTier, version: &str) -> Result<(), &'static str> {
    if version.is_empty()
        || version.len() > MAX_CATALOG_VERSION_BYTES
        || CatalogTier::from_version(version) != Some(tier)
    {
        return Err("invalid catalog version");
    }
    Ok(())
}

/// Validates the exact rate rows shared by bundled and local catalogs.
///
/// Every model must price the whole `[0, ∞)` context range with at most two
/// contiguous bands, every `(model_id, band)` pair must be unique, and every
/// rate must be a non-negative fixed-point amount.
///
/// # Errors
///
/// Returns a stable category when a row, rate, or band set is invalid.
pub fn validate_model_rows(models: &[ModelRate]) -> Result<(), &'static str> {
    if models.is_empty() || models.len() > MAX_CATALOG_MODELS {
        return Err("invalid catalog models");
    }
    let mut bands = HashSet::new();
    let mut by_model = BTreeMap::<&str, Vec<(Option<i64>, Option<i64>)>>::new();
    for rate in models {
        let model_id = rate.model_id.as_str();
        if model_id.is_empty() || model_id.len() > MAX_MODEL_ID_BYTES || !valid_model_id(model_id) {
            return Err("invalid catalog models");
        }
        if !bands.insert((
            model_id,
            rate.minimum_input_tokens,
            rate.maximum_input_tokens,
        )) {
            return Err("invalid catalog rate");
        }
        if rate.minimum_input_tokens.is_some_and(|value| value < 0)
            || rate.maximum_input_tokens.is_some_and(|value| value < 0)
            || matches!(
                (rate.minimum_input_tokens, rate.maximum_input_tokens),
                (Some(minimum), Some(maximum)) if minimum > maximum
            )
            || !valid_rate(rate.input)
            || !valid_rate(rate.cached_input)
            || !valid_rate(rate.output)
            || rate
                .cache_write
                .is_some_and(|value| value <= 0 || value > MAX_RATE_MICRO_USD)
        {
            return Err("invalid catalog rate");
        }
        by_model
            .entry(model_id)
            .or_default()
            .push((rate.minimum_input_tokens, rate.maximum_input_tokens));
    }
    for model_bands in by_model.values_mut() {
        validate_model_bands(model_bands)?;
    }
    Ok(())
}

fn valid_rate(rate: i64) -> bool {
    (0..=MAX_RATE_MICRO_USD).contains(&rate)
}

fn valid_model_id(model_id: &str) -> bool {
    let mut characters = model_id.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return false;
    }
    characters.all(|value| {
        value.is_ascii_lowercase() || value.is_ascii_digit() || matches!(value, '.' | '-' | '_')
    })
}

/// Requires one model to tile `[0, ∞)` with contiguous, non-overlapping bands.
fn validate_model_bands(bands: &mut [(Option<i64>, Option<i64>)]) -> Result<(), &'static str> {
    if bands.is_empty() || bands.len() > MAX_CATALOG_BANDS_PER_MODEL {
        return Err("invalid catalog bands");
    }
    bands.sort_unstable_by_key(|(minimum, _)| minimum.unwrap_or(0));
    let count = bands.len();
    let mut next_start = 0_i64;
    for (index, (minimum, maximum)) in bands.iter().enumerate() {
        if minimum.unwrap_or(0) != next_start {
            return Err("invalid catalog bands");
        }
        match maximum {
            Some(maximum) => {
                next_start = maximum.checked_add(1).ok_or("invalid catalog bands")?;
            }
            None if index + 1 == count => {}
            None => return Err("invalid catalog bands"),
        }
    }
    if bands[count - 1].1.is_some() {
        return Err("invalid catalog bands");
    }
    Ok(())
}

/// Validates the immutable bundled catalogs, including their authored provenance.
///
/// # Errors
///
/// Returns a stable category when metadata or any rate row is invalid.
pub fn validate_bundled_catalog() -> Result<(), &'static str> {
    for tier in CatalogTier::ALL {
        let catalog = bundled_catalog(tier);
        let derived = validate_catalog_structure(&catalog)?;
        if derived != tier
            || catalog.version != tier.version()
            || catalog.captured_at != tier.captured_at()
            || catalog.effective_at.as_deref() != tier.effective_at()
            || catalog
                .sources
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                != tier.sources()
        {
            return Err("invalid catalog metadata");
        }
    }
    Ok(())
}

/// One versioned row layer: the bundled baseline or the local override.
#[derive(Clone, Debug)]
struct CatalogLayer {
    version: Arc<str>,
    models: Arc<[ModelRate]>,
}

#[derive(Clone, Debug)]
struct EffectiveTier {
    bundled: CatalogLayer,
    local: Option<CatalogLayer>,
}

impl EffectiveTier {
    fn select(&self, model_id: &str, input_tokens: Option<i64>) -> Option<(&ModelRate, bool)> {
        if let Some(local) = &self.local
            && local.models.iter().any(|rate| rate.model_id == model_id)
        {
            return select_in_layer(local, model_id, input_tokens).map(|rate| (rate, true));
        }
        select_in_layer(&self.bundled, model_id, input_tokens).map(|rate| (rate, false))
    }

    fn version(&self, local: bool) -> &Arc<str> {
        match (local, &self.local) {
            (true, Some(layer)) => &layer.version,
            _ => &self.bundled.version,
        }
    }

    fn rows(&self) -> Vec<CatalogTableRow> {
        let mut rows = Vec::new();
        if let Some(local) = &self.local {
            rows.extend(table_rows(local.models.iter(), CatalogRowSource::Official));
        }
        let overridden: HashSet<&str> = self
            .local
            .as_ref()
            .map(|layer| {
                layer
                    .models
                    .iter()
                    .map(|rate| rate.model_id.as_str())
                    .collect()
            })
            .unwrap_or_default();
        rows.extend(table_rows(
            self.bundled
                .models
                .iter()
                .filter(|rate| !overridden.contains(rate.model_id.as_str())),
            CatalogRowSource::Bundled,
        ));
        rows
    }
}

fn select_in_layer<'a>(
    layer: &'a CatalogLayer,
    model_id: &str,
    input_tokens: Option<i64>,
) -> Option<&'a ModelRate> {
    let mut matching = layer.models.iter().filter(|rate| rate.model_id == model_id);
    let first = matching.next()?;
    if matching.next().is_none()
        && first.minimum_input_tokens.is_none()
        && first.maximum_input_tokens.is_none()
    {
        return Some(first);
    }
    let input_tokens = input_tokens.filter(|value| *value >= 0)?;
    layer.models.iter().find(|rate| {
        rate.model_id == model_id
            && rate
                .minimum_input_tokens
                .is_none_or(|minimum| input_tokens >= minimum)
            && rate
                .maximum_input_tokens
                .is_none_or(|maximum| input_tokens <= maximum)
    })
}

fn table_rows<'a>(
    models: impl Iterator<Item = &'a ModelRate>,
    source: CatalogRowSource,
) -> Vec<CatalogTableRow> {
    let mut by_model = BTreeMap::<&str, Vec<&ModelRate>>::new();
    for rate in models {
        by_model
            .entry(rate.model_id.as_str())
            .or_default()
            .push(rate);
    }
    let mut rows = Vec::new();
    for (model_id, mut rates) in by_model {
        rates.sort_unstable_by_key(|rate| rate.minimum_input_tokens.unwrap_or(0));
        for (index, rate) in rates.iter().enumerate() {
            rows.push(CatalogTableRow {
                model_id: model_id.to_owned(),
                band: if index == 0 {
                    CatalogBand::Short
                } else {
                    CatalogBand::Long
                },
                input: rate.input,
                cached_input: rate.cached_input,
                cache_write: rate.cache_write,
                output: rate.output,
                source,
            });
        }
    }
    rows
}

/// The effective catalog: the bundled baseline with the local override applied.
#[derive(Clone, Debug)]
pub struct EffectiveCatalog {
    standard: EffectiveTier,
    priority: EffectiveTier,
}

impl EffectiveCatalog {
    /// Builds the bundled baseline without any local override.
    #[must_use]
    pub fn baseline() -> Self {
        let tier = |tier: CatalogTier| EffectiveTier {
            bundled: CatalogLayer {
                version: Arc::from(tier.version()),
                models: bundled_catalog(tier).models.into(),
            },
            local: None,
        };
        Self {
            standard: tier(CatalogTier::Standard),
            priority: tier(CatalogTier::Priority),
        }
    }

    /// Applies one local tier as a complete per-model override of the baseline.
    ///
    /// # Errors
    ///
    /// Returns a stable category when the local tier rows or version are invalid.
    pub fn with_local(local: &LocalPricingTable) -> Result<Self, &'static str> {
        let mut catalog = Self::baseline();
        for tier in CatalogTier::ALL {
            let override_tier = local.tier(tier);
            validate_tier_version(tier, &override_tier.version)?;
            validate_model_rows(&override_tier.models)?;
        }
        for tier in CatalogTier::ALL {
            let target = catalog.tier_mut(tier);
            target.local = Some(CatalogLayer {
                version: Arc::from(local.tier(tier).version.as_str()),
                models: local.tier(tier).models.clone().into(),
            });
        }
        Ok(catalog)
    }

    fn tier(&self, tier: CatalogTier) -> &EffectiveTier {
        match tier {
            CatalogTier::Standard => &self.standard,
            CatalogTier::Priority => &self.priority,
        }
    }

    fn tier_mut(&mut self, tier: CatalogTier) -> &mut EffectiveTier {
        match tier {
            CatalogTier::Standard => &mut self.standard,
            CatalogTier::Priority => &mut self.priority,
        }
    }

    /// Prices one usage observation against the effective catalog.
    #[must_use]
    pub fn price(&self, observation: &UsageObservation<'_>) -> PricedUsage {
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
        let effective_tier = self.tier(tier);
        let Some((rate, local)) = effective_tier.select(model_id, observation.input_tokens) else {
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
        let version = effective_tier.version(local).clone();
        if totals_consistent
            && let (Some(input_amount), Some(output_amount)) = (input_amount, output_amount)
            && let Some(amount) = input_amount.checked_add(output_amount)
        {
            return priced(CostStatus::Exact, Some(amount), Some(version));
        }
        let amounts = [input_amount, output_amount];
        let has_known_amount = amounts.iter().any(Option::is_some);
        let known = amounts
            .into_iter()
            .flatten()
            .try_fold(0_i64, i64::checked_add);
        match (has_known_amount, known) {
            (true, Some(amount)) => priced(CostStatus::Partial, Some(amount), Some(version)),
            (false, _) | (_, None) => priced(CostStatus::Unavailable, None, None),
        }
    }

    /// Projects the default billing tier's effective rows in display order.
    ///
    /// Settings shows the Standard tier that ordinary requests are billed with;
    /// the Priority tier shares the same model list and only changes the rates
    /// of Fast requests. Synchronized models come first, then the bundled
    /// models, each ordered by model identifier and context band.
    #[must_use]
    pub fn rows(&self) -> Vec<CatalogTableRow> {
        self.rows_for(CatalogTier::Standard)
    }

    /// Projects one billing tier's effective rows in display order.
    #[must_use]
    pub fn rows_for(&self, tier: CatalogTier) -> Vec<CatalogTableRow> {
        self.tier(tier).rows()
    }
}

/// The current effective catalog plus the state of the local override.
#[derive(Clone, Debug)]
pub struct CatalogState {
    pub catalog: Arc<EffectiveCatalog>,
    pub local: LocalPricingStatus,
    pub synced_at_ms: Option<i64>,
    pub source_url: Option<String>,
}

/// Shared, hot-swappable effective catalog.
///
/// Inference history pricing reads the current snapshot for every attempt, so a
/// synchronized catalog takes effect for new requests without restarts.
#[derive(Clone)]
pub struct CatalogProvider {
    state: Arc<ArcSwap<CatalogState>>,
}

impl Default for CatalogProvider {
    fn default() -> Self {
        Self::baseline()
    }
}

impl CatalogProvider {
    /// Builds a provider that prices with the bundled baseline only.
    #[must_use]
    pub fn baseline() -> Self {
        Self {
            state: Arc::new(ArcSwap::from_pointee(CatalogState {
                catalog: Arc::new(EffectiveCatalog::baseline()),
                local: LocalPricingStatus::Missing,
                synced_at_ms: None,
                source_url: None,
            })),
        }
    }

    #[must_use]
    pub fn state(&self) -> Arc<CatalogState> {
        self.state.load_full()
    }

    /// Installs the result of loading `local-pricing.json`.
    ///
    /// A local table that fails structural validation is treated as corrupt
    /// rather than partially applied, so pricing never mixes two catalogs.
    pub fn install(&self, load: LocalPricingLoad) {
        match load {
            LocalPricingLoad::Loaded(table) => match EffectiveCatalog::with_local(&table) {
                Ok(catalog) => self.state.store(Arc::new(CatalogState {
                    catalog: Arc::new(catalog),
                    local: LocalPricingStatus::Loaded,
                    synced_at_ms: Some(table.synced_at_ms),
                    source_url: Some(table.source_url.clone()),
                })),
                Err(_) => self.install_corrupt(),
            },
            LocalPricingLoad::Corrupt => self.install_corrupt(),
            LocalPricingLoad::Missing => self.state.store(Arc::new(CatalogState {
                catalog: Arc::new(EffectiveCatalog::baseline()),
                local: LocalPricingStatus::Missing,
                synced_at_ms: None,
                source_url: None,
            })),
        }
    }

    fn install_corrupt(&self) {
        self.state.store(Arc::new(CatalogState {
            catalog: Arc::new(EffectiveCatalog::baseline()),
            local: LocalPricingStatus::Corrupt,
            synced_at_ms: None,
            source_url: None,
        }));
    }

    /// Prices one observation against the current effective catalog.
    #[must_use]
    pub fn price(&self, observation: &UsageObservation<'_>) -> PricedUsage {
        self.state.load().catalog.price(observation)
    }
}

/// Prices one observation against the bundled baseline catalog.
///
/// Use [`CatalogProvider::price`] when a synchronized local catalog may be in
/// effect; this entry point never reads local data.
#[must_use]
pub fn price_usage(observation: &UsageObservation<'_>) -> PricedUsage {
    static BASELINE: LazyLock<CatalogProvider> = LazyLock::new(CatalogProvider::baseline);
    BASELINE.price(observation)
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
    let mut common_catalog: Option<Option<Arc<str>>> = None;
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
                        None => cost.catalog_version.clone(),
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
    catalog_version: Option<Arc<str>>,
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
        (Some("priority" | "fast"), None | Some("default" | "priority" | "fast"))
        | (None | Some("auto" | "default"), Some("priority" | "fast")) => {
            Some(CatalogTier::Priority)
        }
        (None | Some("default"), None | Some("default")) | (Some("auto"), Some("default")) => {
            Some(CatalogTier::Standard)
        }
        _ => None,
    }
}

/// Derives the display service tier from a persisted catalog version prefix.
#[must_use]
pub fn catalog_service_tier(catalog_version: &str) -> Option<&'static str> {
    CatalogTier::from_version(catalog_version).map(CatalogTier::service_tier)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(value: &str) -> Arc<str> {
        Arc::from(value)
    }

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

    fn standard() -> Catalog {
        bundled_catalog(CatalogTier::Standard)
    }

    fn priority() -> Catalog {
        bundled_catalog(CatalogTier::Priority)
    }

    fn rate<'a>(
        catalog: &'a Catalog,
        model_id: &str,
        minimum_input_tokens: Option<i64>,
    ) -> &'a ModelRate {
        catalog
            .models
            .iter()
            .find(|row| {
                row.model_id == model_id && row.minimum_input_tokens == minimum_input_tokens
            })
            .expect("official catalog row")
    }

    fn band_rows(
        catalog: &Catalog,
        model_id: &str,
        minimum_input_tokens: Option<i64>,
    ) -> (i64, i64, Option<i64>, i64) {
        let row = rate(catalog, model_id, minimum_input_tokens);
        (row.input, row.cached_input, row.cache_write, row.output)
    }

    #[test]
    fn bundled_catalog_is_valid() {
        assert_eq!(validate_bundled_catalog(), Ok(()));
    }

    #[test]
    fn current_catalog_rates_match_the_official_standard_and_fast_tables() {
        let standard = standard();
        let priority = priority();

        for (model_name, short, long) in [
            (
                "gpt-6-astra",
                (10_000_000, 1_000_000, Some(12_500_000), 50_000_000),
                (20_000_000, 2_000_000, Some(25_000_000), 75_000_000),
            ),
            (
                "gpt-6-sol",
                (2_000_000, 200_000, Some(2_500_000), 10_000_000),
                (4_000_000, 400_000, Some(5_000_000), 15_000_000),
            ),
            (
                "gpt-6-luna",
                (100_000, 10_000, Some(125_000), 500_000),
                (200_000, 20_000, Some(250_000), 750_000),
            ),
            (
                "gpt-5.6-sol",
                (4_000_000, 400_000, Some(5_000_000), 20_000_000),
                (8_000_000, 800_000, Some(10_000_000), 30_000_000),
            ),
            (
                "gpt-5.6-terra",
                (2_000_000, 200_000, Some(2_500_000), 12_000_000),
                (4_000_000, 400_000, Some(5_000_000), 18_000_000),
            ),
            (
                "gpt-5.6-luna",
                (200_000, 20_000, Some(250_000), 1_200_000),
                (400_000, 40_000, Some(500_000), 1_800_000),
            ),
        ] {
            assert_eq!(band_rows(&standard, model_name, None), short);
            assert_eq!(band_rows(&standard, model_name, Some(272_001)), long);
        }

        for (model_name, short, long) in [
            (
                "gpt-6-astra",
                (20_000_000, 2_000_000, Some(25_000_000), 100_000_000),
                Some((40_000_000, 4_000_000, Some(50_000_000), 150_000_000)),
            ),
            (
                "gpt-6-sol",
                (4_000_000, 400_000, Some(5_000_000), 20_000_000),
                Some((8_000_000, 800_000, Some(10_000_000), 30_000_000)),
            ),
            (
                "gpt-6-luna",
                (200_000, 20_000, Some(250_000), 1_000_000),
                Some((400_000, 40_000, Some(500_000), 1_500_000)),
            ),
            (
                "gpt-5.6-sol",
                (8_000_000, 800_000, Some(10_000_000), 40_000_000),
                Some((16_000_000, 1_600_000, Some(20_000_000), 60_000_000)),
            ),
            (
                "gpt-5.6-terra",
                (4_000_000, 400_000, Some(5_000_000), 24_000_000),
                Some((8_000_000, 800_000, Some(10_000_000), 36_000_000)),
            ),
            (
                "gpt-5.6-luna",
                (400_000, 40_000, Some(500_000), 2_400_000),
                Some((800_000, 80_000, Some(1_000_000), 3_600_000)),
            ),
            ("gpt-5.5", (12_500_000, 1_250_000, None, 75_000_000), None),
            ("gpt-5.4", (5_000_000, 500_000, None, 30_000_000), None),
            ("gpt-5.4-mini", (1_500_000, 150_000, None, 9_000_000), None),
            ("gpt-5.2", (3_500_000, 350_000, None, 28_000_000), None),
            ("gpt-5.1", (2_500_000, 250_000, None, 20_000_000), None),
            ("gpt-5", (2_500_000, 250_000, None, 20_000_000), None),
            ("gpt-5-mini", (450_000, 45_000, None, 3_600_000), None),
            ("gpt-4.1", (3_500_000, 875_000, None, 14_000_000), None),
            ("gpt-4.1-mini", (700_000, 175_000, None, 2_800_000), None),
            ("gpt-4.1-nano", (200_000, 50_000, None, 800_000), None),
            ("o3", (3_500_000, 875_000, None, 14_000_000), None),
            ("o4-mini", (2_000_000, 500_000, None, 8_000_000), None),
        ] {
            assert_eq!(band_rows(&priority, model_name, None), short);
            if let Some(long) = long {
                assert_eq!(band_rows(&priority, model_name, Some(272_001)), long);
            }
        }
    }

    #[test]
    fn catalog_validation_rejects_invalid_metadata_rates_and_bands() {
        let mut unsupported_currency = standard();
        unsupported_currency.currency = "EUR".to_owned();
        assert_eq!(
            validate_catalog_structure(&unsupported_currency),
            Err("invalid catalog metadata")
        );

        let mut unknown_version = standard();
        unknown_version.version = "openai-flex-2026-09-29".to_owned();
        assert_eq!(
            validate_catalog_structure(&unknown_version),
            Err("invalid catalog metadata")
        );

        let mut mismatched_tier = standard();
        mismatched_tier.version = PRIORITY_CATALOG_VERSION.to_owned();
        assert_eq!(
            validate_catalog_structure(&mismatched_tier),
            Err("invalid catalog metadata")
        );

        let mut over_long_source = standard();
        over_long_source.sources = vec!["x".repeat(MAX_CATALOG_SOURCE_BYTES + 1)];
        assert_eq!(
            validate_catalog_structure(&over_long_source),
            Err("invalid catalog metadata")
        );

        let mut empty_models = standard();
        empty_models.models.clear();
        assert_eq!(
            validate_catalog_structure(&empty_models),
            Err("invalid catalog models")
        );

        let mut unsupported_model_id = standard();
        unsupported_model_id.models[0].model_id = "GPT 5".to_owned();
        assert_eq!(
            validate_catalog_structure(&unsupported_model_id),
            Err("invalid catalog models")
        );

        let mut duplicate = standard();
        duplicate.models.push(duplicate.models[0].clone());
        assert_eq!(
            validate_catalog_structure(&duplicate),
            Err("invalid catalog rate")
        );

        let mut negative = standard();
        negative.models[0].input = -1;
        assert_eq!(
            validate_catalog_structure(&negative),
            Err("invalid catalog rate")
        );

        let mut zero_cache_write = standard();
        zero_cache_write.models[0].cache_write = Some(0);
        assert_eq!(
            validate_catalog_structure(&zero_cache_write),
            Err("invalid catalog rate")
        );

        let mut gap = standard();
        let long_band = gap
            .models
            .iter_mut()
            .find(|rate| {
                rate.model_id == "gpt-5.6-sol" && rate.minimum_input_tokens == Some(272_001)
            })
            .expect("long-context catalog band");
        long_band.minimum_input_tokens = Some(272_002);
        assert_eq!(
            validate_catalog_structure(&gap),
            Err("invalid catalog bands")
        );

        let baseline = standard();
        let mut overlap = standard();
        overlap.models.retain(|rate| rate.model_id != "gpt-5");
        let template = rate(&baseline, "gpt-5", None);
        overlap.models.push(ModelRate {
            minimum_input_tokens: None,
            maximum_input_tokens: Some(272_000),
            ..template.clone()
        });
        overlap.models.push(ModelRate {
            minimum_input_tokens: Some(272_000),
            maximum_input_tokens: None,
            ..template.clone()
        });
        assert_eq!(
            validate_catalog_structure(&overlap),
            Err("invalid catalog bands")
        );

        let mut truncated = standard();
        truncated
            .models
            .retain(|rate| rate.model_id != "gpt-5.6-sol" || rate.maximum_input_tokens.is_none());
        assert_eq!(
            validate_catalog_structure(&truncated),
            Err("invalid catalog bands")
        );
    }

    #[test]
    fn catalog_validation_rejects_more_rows_than_the_row_limit() {
        let baseline = standard();
        let mut catalog = standard();
        let template = rate(&baseline, "gpt-5", None).clone();
        catalog.models = (0..=MAX_CATALOG_MODELS)
            .map(|index| ModelRate {
                model_id: format!("gpt-bulk-{index}"),
                minimum_input_tokens: None,
                maximum_input_tokens: None,
                input: template.input,
                cached_input: template.cached_input,
                cache_write: template.cache_write,
                output: template.output,
            })
            .collect();
        assert_eq!(
            validate_catalog_structure(&catalog),
            Err("invalid catalog models")
        );
    }

    #[test]
    fn tier_version_derivation_accepts_only_the_matching_prefix() {
        for (tier, accepted, rejected) in [
            (
                CatalogTier::Standard,
                "openai-standard-synced-2026-09-30",
                "openai-priority-synced-2026-09-30",
            ),
            (
                CatalogTier::Priority,
                "openai-priority-synced-2026-09-30",
                "openai-standard-synced-2026-09-30",
            ),
        ] {
            assert_eq!(validate_tier_version(tier, accepted), Ok(()));
            assert_eq!(
                validate_tier_version(tier, rejected),
                Err("invalid catalog version")
            );
            assert_eq!(
                validate_tier_version(tier, &"x".repeat(MAX_CATALOG_VERSION_BYTES + 1)),
                Err("invalid catalog version")
            );
            assert_eq!(
                validate_tier_version(tier, ""),
                Err("invalid catalog version")
            );
        }

        assert_eq!(
            catalog_service_tier("openai-standard-synced-2026-09-30"),
            Some("default")
        );
        assert_eq!(
            catalog_service_tier("openai-priority-synced-2026-09-30"),
            Some("priority")
        );
        assert_eq!(catalog_service_tier(CATALOG_VERSION), Some("default"));
        assert_eq!(
            catalog_service_tier(PRIORITY_CATALOG_VERSION),
            Some("priority")
        );
        assert_eq!(catalog_service_tier("openai-flex-2026-09-30"), None);
        assert_eq!(catalog_service_tier(""), None);
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
        unsupported_priority_model.requested_model = Some("gpt-5-nano");
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
        assert_eq!(price_usage(&short).amount_pico_usd, Some(544_012_000_000));

        let mut long = short;
        long.input_tokens = Some(272_001);
        long.total_tokens = Some(272_002);
        assert_eq!(price_usage(&long).amount_pico_usd, Some(1_088_022_000_000));
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
        assert_eq!(result.catalog_version, Some(version(CATALOG_VERSION)));

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
        assert_eq!(result.amount_pico_usd, Some(55_932_800_000));
        assert_eq!(
            result.catalog_version,
            Some(version(PRIORITY_CATALOG_VERSION))
        );

        value.forwarded_service_tier = Some("default");
        let standard = price_usage(&value);
        assert_eq!(standard.amount_pico_usd, Some(27_966_400_000));
        assert_eq!(standard.catalog_version, Some(version(CATALOG_VERSION)));
    }

    #[test]
    fn tier_resolution_is_closed_and_explicit() {
        let mut value = observation();
        value.requested_model = Some("gpt-5.6-luna");
        value.cache_write_input_tokens = Some(0);

        for (requested, actual, expected) in [
            (
                Some("priority"),
                None,
                Some(version(PRIORITY_CATALOG_VERSION)),
            ),
            (
                Some("priority"),
                Some("priority"),
                Some(version(PRIORITY_CATALOG_VERSION)),
            ),
            (
                Some("priority"),
                Some("default"),
                Some(version(PRIORITY_CATALOG_VERSION)),
            ),
            (
                None,
                Some("priority"),
                Some(version(PRIORITY_CATALOG_VERSION)),
            ),
            (
                Some("auto"),
                Some("priority"),
                Some(version(PRIORITY_CATALOG_VERSION)),
            ),
            (
                Some("default"),
                Some("priority"),
                Some(version(PRIORITY_CATALOG_VERSION)),
            ),
            (None, None, Some(version(CATALOG_VERSION))),
            (None, Some("default"), Some(version(CATALOG_VERSION))),
            (Some("default"), None, Some(version(CATALOG_VERSION))),
            (
                Some("auto"),
                Some("default"),
                Some(version(CATALOG_VERSION)),
            ),
            (Some("auto"), None, None),
            (Some("auto"), Some("auto"), None),
            (
                Some("fast"),
                Some("default"),
                Some(version(PRIORITY_CATALOG_VERSION)),
            ),
            (None, Some("fast"), Some(version(PRIORITY_CATALOG_VERSION))),
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
                catalog_version: Some(version(CATALOG_VERSION)),
                status: CostStatus::Exact,
                amount_pico_usd: Some(10),
            },
            PricedUsage {
                catalog_version: Some(version(PRIORITY_CATALOG_VERSION)),
                status: CostStatus::Exact,
                amount_pico_usd: Some(20),
            },
        ]);
        assert_eq!(result.status, CostStatus::Exact);
        assert_eq!(result.amount_pico_usd, Some(30));
        assert_eq!(result.catalog_version, None);
    }

    const SYNCED_STANDARD_VERSION: &str = "openai-standard-synced-2026-09-30";
    const SYNCED_PRIORITY_VERSION: &str = "openai-priority-synced-2026-09-30";

    fn local_row(model_id: &str, input: i64, cache_write: Option<i64>) -> ModelRate {
        ModelRate {
            model_id: model_id.to_owned(),
            minimum_input_tokens: None,
            maximum_input_tokens: None,
            input,
            cached_input: input / 10,
            cache_write,
            output: input * 5,
        }
    }

    /// A local table that overrides `gpt-5` and adds a synchronized-only model.
    fn local_table() -> LocalPricingTable {
        LocalPricingTable {
            schema_version: crate::pricing_local::LOCAL_PRICING_SCHEMA_VERSION,
            synced_at_ms: 1_790_000_000_000,
            source_url: "https://developers.openai.com/api/docs/pricing/".to_owned(),
            tiers: crate::pricing_local::LocalPricingTiers {
                standard: crate::pricing_local::LocalPricingTier {
                    version: SYNCED_STANDARD_VERSION.to_owned(),
                    models: vec![
                        local_row("gpt-5", 1_000_000, None),
                        local_row("gpt-9-synced", 300_000, None),
                    ],
                },
                priority: crate::pricing_local::LocalPricingTier {
                    version: SYNCED_PRIORITY_VERSION.to_owned(),
                    models: vec![local_row("gpt-9-synced", 600_000, None)],
                },
            },
        }
    }

    #[test]
    fn local_rows_replace_bundled_rows_and_add_synchronized_models() {
        let catalog = EffectiveCatalog::with_local(&local_table()).expect("valid local table");

        let result = catalog.price(&observation());
        assert_eq!(result.status, CostStatus::Exact);
        assert_eq!(
            result.catalog_version,
            Some(version(SYNCED_STANDARD_VERSION))
        );
        // 10 - 4 = 6 regular Tokens at $1.00, 4 cached at $0.10, 2 output at $5.00.
        assert_eq!(result.amount_pico_usd, Some(16_400_000));

        // A model the local table does not mention keeps its bundled rate.
        let mut bundled = observation();
        bundled.actual_model = Some("gpt-5-mini");
        let result = catalog.price(&bundled);
        assert_eq!(result.catalog_version, Some(version(CATALOG_VERSION)));
        assert_eq!(result.amount_pico_usd, Some(5_600_000));

        // A synchronized-only model is priced from the local tier.
        let mut synced = observation();
        synced.requested_model = Some("gpt-9-synced");
        let result = catalog.price(&synced);
        assert_eq!(result.status, CostStatus::Exact);
        assert_eq!(
            result.catalog_version,
            Some(version(SYNCED_STANDARD_VERSION))
        );
        assert_eq!(result.amount_pico_usd, Some(4_920_000));
    }

    #[test]
    fn local_priority_rows_apply_only_to_the_priority_tier() {
        let catalog = EffectiveCatalog::with_local(&local_table()).expect("valid local table");

        let mut fast = observation();
        fast.requested_model = Some("gpt-9-synced");
        fast.forwarded_service_tier = Some("priority");
        fast.actual_service_tier = Some("default");
        let result = catalog.price(&fast);
        assert_eq!(result.amount_pico_usd, Some(9_840_000));
        assert_eq!(
            result.catalog_version,
            Some(version(SYNCED_PRIORITY_VERSION))
        );

        fast.forwarded_service_tier = Some("default");
        let result = catalog.price(&fast);
        assert_eq!(result.amount_pico_usd, Some(4_920_000));
        assert_eq!(
            result.catalog_version,
            Some(version(SYNCED_STANDARD_VERSION))
        );
    }

    #[test]
    fn local_long_band_replaces_the_bundled_long_band_without_mixing() {
        let mut table = local_table();
        table.tiers.standard.models = vec![
            ModelRate {
                model_id: "gpt-6-sol".to_owned(),
                minimum_input_tokens: None,
                maximum_input_tokens: Some(272_000),
                input: 1_000_000,
                cached_input: 100_000,
                cache_write: None,
                output: 5_000_000,
            },
            ModelRate {
                model_id: "gpt-6-sol".to_owned(),
                minimum_input_tokens: Some(272_001),
                maximum_input_tokens: None,
                input: 2_000_000,
                cached_input: 200_000,
                cache_write: None,
                output: 10_000_000,
            },
        ];
        let catalog = EffectiveCatalog::with_local(&table).expect("valid local table");

        let mut long = observation();
        long.requested_model = Some("gpt-6-sol");
        long.input_tokens = Some(272_001);
        long.output_tokens = Some(1);
        long.total_tokens = Some(272_002);
        long.cached_input_tokens = Some(0);
        long.cache_write_input_tokens = Some(0);
        let result = catalog.price(&long);
        assert_eq!(result.status, CostStatus::Exact);
        // The bundled long band would price the same request at 1_088_019_000_000.
        assert_eq!(result.amount_pico_usd, Some(544_012_000_000));
        assert_eq!(
            result.catalog_version,
            Some(version(SYNCED_STANDARD_VERSION))
        );
    }

    #[test]
    fn effective_rows_order_synchronized_models_before_bundled_models() {
        let catalog = EffectiveCatalog::with_local(&local_table()).expect("valid local table");
        let rows = catalog.rows();
        let first_bundled = rows
            .iter()
            .position(|row| row.source == CatalogRowSource::Bundled)
            .expect("bundled rows");
        assert!(
            rows[..first_bundled]
                .iter()
                .all(|row| row.source == CatalogRowSource::Official)
        );
        assert!(
            rows[first_bundled..]
                .iter()
                .all(|row| row.source == CatalogRowSource::Bundled)
        );

        let official_models: Vec<&str> = rows[..first_bundled]
            .iter()
            .map(|row| row.model_id.as_str())
            .collect();
        assert_eq!(official_models, ["gpt-5", "gpt-9-synced"]);
        assert_eq!(rows[first_bundled].model_id, "codex-mini-latest");

        let mut bundled_models: Vec<&str> = Vec::new();
        for row in &rows[first_bundled..] {
            if bundled_models.last() != Some(&row.model_id.as_str()) {
                bundled_models.push(row.model_id.as_str());
            }
        }
        let mut expected = bundled_models.clone();
        expected.sort_unstable();
        assert_eq!(bundled_models, expected);
        assert!(!bundled_models.contains(&"gpt-5"));
        // The local table replaces the bundled rows of the model it overrides.
        assert_eq!(
            catalog
                .rows()
                .iter()
                .filter(|row| row.model_id == "gpt-5")
                .count(),
            1
        );
    }

    #[test]
    fn effective_rows_derive_bands_from_the_tiling() {
        let rows = EffectiveCatalog::baseline().rows();
        let sol: Vec<&CatalogTableRow> = rows
            .iter()
            .filter(|row| row.model_id == "gpt-6-sol")
            .collect();
        assert_eq!(sol.len(), 2);
        assert_eq!(
            (sol[0].band, sol[1].band),
            (CatalogBand::Short, CatalogBand::Long)
        );
        assert_eq!((sol[0].input, sol[1].input), (2_000_000, 4_000_000));
        assert_eq!(sol[0].cache_write, Some(2_500_000));

        let mini: Vec<&CatalogTableRow> = rows
            .iter()
            .filter(|row| row.model_id == "gpt-4.1-mini")
            .collect();
        assert_eq!(mini.len(), 1);
        assert_eq!(mini[0].band, CatalogBand::Short);
        assert_eq!(mini[0].cache_write, None);
        assert_eq!(mini[0].source, CatalogRowSource::Bundled);
    }

    #[test]
    fn provider_without_a_local_table_matches_baseline_pricing() {
        let provider = CatalogProvider::baseline();
        assert_eq!(provider.state().local, LocalPricingStatus::Missing);
        assert_eq!(provider.state().synced_at_ms, None);
        assert_eq!(provider.state().source_url, None);

        for candidate in [
            observation(),
            UsageObservation {
                requested_model: Some("gpt-5.6-terra"),
                input_tokens: Some(272_001),
                total_tokens: Some(272_003),
                output_tokens: Some(2),
                ..observation()
            },
            UsageObservation {
                forwarded_service_tier: Some("priority"),
                actual_service_tier: None,
                ..observation()
            },
        ] {
            assert_eq!(provider.price(&candidate), price_usage(&candidate));
        }
        assert_eq!(
            provider.state().catalog.rows(),
            EffectiveCatalog::baseline().rows()
        );
    }

    #[test]
    fn provider_installs_partial_loads_with_the_reported_local_state() {
        let provider = CatalogProvider::baseline();
        provider.install(LocalPricingLoad::Loaded(local_table()));
        let state = provider.state();
        assert_eq!(state.local, LocalPricingStatus::Loaded);
        assert_eq!(state.synced_at_ms, Some(1_790_000_000_000));
        assert_eq!(
            state.source_url.as_deref(),
            Some("https://developers.openai.com/api/docs/pricing/")
        );
        let mut value = observation();
        value.requested_model = Some("gpt-9-synced");
        assert_eq!(
            state.catalog.price(&value).catalog_version,
            Some(version(SYNCED_STANDARD_VERSION))
        );

        provider.install(LocalPricingLoad::Corrupt);
        let state = provider.state();
        assert_eq!(state.local, LocalPricingStatus::Corrupt);
        assert_eq!(state.synced_at_ms, None);
        assert_eq!(state.catalog.price(&value).status, CostStatus::Unavailable);
        assert_eq!(
            state.catalog.price(&observation()).catalog_version,
            Some(version(CATALOG_VERSION))
        );

        provider.install(LocalPricingLoad::Loaded(local_table()));
        provider.install(LocalPricingLoad::Missing);
        let state = provider.state();
        assert_eq!(state.local, LocalPricingStatus::Missing);
        assert_eq!(
            state.catalog.price(&observation()).catalog_version,
            Some(version(CATALOG_VERSION))
        );
    }

    #[test]
    fn provider_rejects_a_local_table_whose_rows_break_the_shared_invariants() {
        let mut table = local_table();
        let duplicated = table.tiers.standard.models[0].clone();
        table.tiers.standard.models.push(duplicated);
        assert!(matches!(
            EffectiveCatalog::with_local(&table),
            Err("invalid catalog rate")
        ));

        let provider = CatalogProvider::baseline();
        provider.install(LocalPricingLoad::Loaded(table));
        assert_eq!(provider.state().local, LocalPricingStatus::Corrupt);
        assert_eq!(
            provider
                .state()
                .catalog
                .price(&observation())
                .catalog_version,
            Some(version(CATALOG_VERSION))
        );
    }
}
