use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use ts_rs::TS;
use url::Url;
use uuid::Uuid;
use zeroize::Zeroizing;

pub const MAX_ROUTE_NAME_CHARS: usize = 30;
pub const MAX_BASE_URL_BYTES: usize = 2_048;
pub const MAX_API_KEY_BYTES: usize = 8_192;
pub const MAX_BALANCE_SCRIPT_BYTES: usize = 256 * 1024;
pub const MIN_MENU_BALANCE_DEBOUNCE_SECONDS: u16 = 10;
pub const MAX_MENU_BALANCE_DEBOUNCE_SECONDS: u16 = 600;
pub const DEFAULT_MENU_BALANCE_DEBOUNCE_SECONDS: u16 = 30;
pub const MIN_AUTOMATIC_BALANCE_REFRESH_MINUTES: u16 = 5;
pub const MAX_AUTOMATIC_BALANCE_REFRESH_MINUTES: u16 = 1_440;
pub const DEFAULT_AUTOMATIC_BALANCE_REFRESH_MINUTES: u16 = 30;
pub const MIN_IMAGES_GENERATION_TIMEOUT_SECS: u16 = 600;
pub const DEFAULT_IMAGES_GENERATION_TIMEOUT_SECS: u16 = 600;
pub const MAX_IMAGES_GENERATION_TIMEOUT_SECS: u16 = 3_600;
pub const MIN_MCP_IMAGE_CAPACITY_WARNING_MIB: u32 = 128;
pub const DEFAULT_MCP_IMAGE_CAPACITY_WARNING_MIB: u32 = 1_024;
pub const MAX_MCP_IMAGE_CAPACITY_WARNING_MIB: u32 = 102_400;
pub const DEFAULT_CODEX_MODEL_CONTEXT_WINDOW: u64 = 128_000;
pub const MAX_CODEX_MODEL_CONTEXT_WINDOW: u64 = 9_007_199_254_740_991;

macro_rules! string_id {
    ($name:ident) => {
        #[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize, TS)]
        pub struct $name(String);

        impl $name {
            #[must_use]
            pub fn new() -> Self {
                Self(Uuid::new_v4().to_string())
            }

            #[must_use]
            pub fn from_string(value: String) -> Self {
                Self(value)
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }
    };
}

string_id!(RouteId);
string_id!(SecretId);
string_id!(ProxyRequestId);
string_id!(UpstreamAttemptId);

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum RouteMoveDirection {
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum ServiceTierPolicy {
    #[default]
    Passthrough,
    Omit,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum AppearancePreference {
    #[default]
    System,
    Light,
    Dark,
}

impl AppearancePreference {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    /// Parses a persisted preference without silently substituting a default.
    ///
    /// # Errors
    ///
    /// Returns a field-specific validation error for an unknown value.
    pub fn parse_persisted(value: &str) -> Result<Self, ValidationError> {
        match value {
            "system" => Ok(Self::System),
            "light" => Ok(Self::Light),
            "dark" => Ok(Self::Dark),
            _ => Err(ValidationError::new(
                "appearance_preference_invalid",
                "appearancePreference",
            )),
        }
    }
}

impl ServiceTierPolicy {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Passthrough => "passthrough",
            Self::Omit => "omit",
        }
    }

    /// Parses a persisted policy without silently substituting a default.
    ///
    /// # Errors
    ///
    /// Returns a field-specific validation error for an unknown value.
    pub fn parse_persisted(value: &str) -> Result<Self, ValidationError> {
        match value {
            "passthrough" => Ok(Self::Passthrough),
            "omit" => Ok(Self::Omit),
            _ => Err(ValidationError::new(
                "service_tier_policy_invalid",
                "serviceTierPolicy",
            )),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RouteName(String);

impl RouteName {
    /// Normalizes and validates a user-visible route name.
    ///
    /// # Errors
    ///
    /// Returns a field-specific error when the trimmed name is empty, contains
    /// control characters, or exceeds the scalar-value limit.
    pub fn parse(value: &str) -> Result<Self, ValidationError> {
        let value = value.trim();
        if value.is_empty() {
            return Err(ValidationError::new("route_name_required", "name"));
        }
        if value.chars().count() > MAX_ROUTE_NAME_CHARS {
            return Err(ValidationError::new("route_name_too_long", "name"));
        }
        if value.chars().any(char::is_control) {
            return Err(ValidationError::new("route_name_control_character", "name"));
        }
        Ok(Self(value.to_owned()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn comparison_key(&self) -> String {
        self.0.to_ascii_lowercase()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BaseUrl(String);

impl BaseUrl {
    /// Validates and normalizes the API prefix before `/responses`.
    ///
    /// # Errors
    ///
    /// Returns a field-specific error for an oversized or non-HTTP(S) absolute
    /// URL, when credentials, query parameters, or fragments are present, or
    /// when the supplied endpoint is incompatible with the Responses API.
    pub fn parse(value: &str) -> Result<Self, ValidationError> {
        let value = value.trim();
        if value.len() > MAX_BASE_URL_BYTES {
            return Err(ValidationError::new("base_url_too_long", "baseUrl"));
        }
        let parsed =
            Url::parse(value).map_err(|_| ValidationError::new("base_url_invalid", "baseUrl"))?;
        if !matches!(parsed.scheme(), "http" | "https")
            || parsed.host_str().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
        {
            return Err(ValidationError::new("base_url_invalid", "baseUrl"));
        }

        let normalized = parsed.as_str().trim_end_matches('/');
        let normalized_path = parsed.path().trim_end_matches('/');
        if normalized_path.ends_with("/chat/completions") {
            return Err(ValidationError::new(
                "base_url_unsupported_endpoint",
                "baseUrl",
            ));
        }

        let canonical_path = normalized_path
            .strip_suffix("/responses")
            .map_or(normalized_path, |prefix| prefix.trim_end_matches('/'));
        if canonical_path.ends_with("/responses") {
            return Err(ValidationError::new(
                "base_url_duplicate_responses",
                "baseUrl",
            ));
        }

        let canonical = if normalized_path == canonical_path {
            normalized
        } else {
            normalized
                .strip_suffix("/responses")
                .unwrap_or(normalized)
                .trim_end_matches('/')
        };

        Ok(Self(canonical.to_owned()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn inference_url(&self) -> String {
        format!("{}/responses", self.0)
    }

    #[must_use]
    pub fn images_generation_url(&self) -> String {
        format!("{}/images/generations", self.0)
    }

    #[must_use]
    pub fn host(&self) -> String {
        Url::parse(&self.0)
            .ok()
            .and_then(|url| url.host_str().map(str::to_owned))
            .unwrap_or_default()
    }
}

pub struct ApiKey(Zeroizing<Vec<u8>>);

impl ApiKey {
    /// Validates a route API key and stores it in zeroizing memory.
    ///
    /// # Errors
    ///
    /// Returns a field-specific error when the trimmed key is empty, oversized,
    /// or contains a control character.
    pub fn parse(value: &str) -> Result<Self, ValidationError> {
        let value = value.trim();
        if value.is_empty() {
            return Err(ValidationError::new("api_key_required", "apiKey"));
        }
        if value.len() > MAX_API_KEY_BYTES {
            return Err(ValidationError::new("api_key_too_long", "apiKey"));
        }
        if value.chars().any(char::is_control) {
            return Err(ValidationError::new("api_key_control_character", "apiKey"));
        }
        Ok(Self(Zeroizing::new(value.as_bytes().to_vec())))
    }

    #[must_use]
    pub fn expose(&self) -> &[u8] {
        self.0.as_slice()
    }

    #[must_use]
    pub fn from_stored(value: Vec<u8>) -> Self {
        Self(Zeroizing::new(value))
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum ReachabilityStatus {
    Reachable,
    Slow,
    PathNotFound,
    Unreachable,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct ReachabilityResult {
    pub status: ReachabilityStatus,
    #[ts(type = "number")]
    pub ttfb_ms: Option<u64>,
    pub error_category: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BalanceScriptSource(String);

impl BalanceScriptSource {
    /// Validates the raw balance-script source size.
    ///
    /// # Errors
    ///
    /// Returns a field-specific error when the UTF-8 source exceeds 256 KiB.
    pub fn parse(value: &str) -> Result<Self, ValidationError> {
        if value.len() > MAX_BALANCE_SCRIPT_BYTES {
            return Err(ValidationError::new(
                "balance_script_too_large",
                "balanceScript",
            ));
        }
        Ok(Self(value.to_owned()))
    }

    /// Validates a custom balance script that must be present for execution.
    ///
    /// # Errors
    ///
    /// Returns a field-specific error for an empty or oversized source.
    pub fn parse_required(value: &str) -> Result<Self, ValidationError> {
        if value.trim().is_empty() {
            return Err(ValidationError::new(
                "balance_script_required",
                "balanceQuery.customSource",
            ));
        }
        Self::parse(value)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BalanceQueryPolicy {
    menu_debounce_seconds: u16,
    automatic_refresh_minutes: u16,
}

impl BalanceQueryPolicy {
    /// Validates the durable user-facing balance-query timings.
    ///
    /// # Errors
    ///
    /// Returns a field-specific error when either integer is outside its
    /// supported inclusive range.
    pub fn parse(
        menu_debounce_seconds: u16,
        automatic_refresh_minutes: u16,
    ) -> Result<Self, ValidationError> {
        if !(MIN_MENU_BALANCE_DEBOUNCE_SECONDS..=MAX_MENU_BALANCE_DEBOUNCE_SECONDS)
            .contains(&menu_debounce_seconds)
        {
            return Err(ValidationError::new(
                "menu_balance_debounce_out_of_range",
                "menuDebounceSeconds",
            ));
        }
        if !(MIN_AUTOMATIC_BALANCE_REFRESH_MINUTES..=MAX_AUTOMATIC_BALANCE_REFRESH_MINUTES)
            .contains(&automatic_refresh_minutes)
        {
            return Err(ValidationError::new(
                "automatic_balance_refresh_out_of_range",
                "automaticRefreshMinutes",
            ));
        }
        Ok(Self {
            menu_debounce_seconds,
            automatic_refresh_minutes,
        })
    }

    #[must_use]
    pub const fn menu_debounce_seconds(self) -> u16 {
        self.menu_debounce_seconds
    }

    #[must_use]
    pub const fn automatic_refresh_minutes(self) -> u16 {
        self.automatic_refresh_minutes
    }

    #[must_use]
    pub fn menu_debounce_millis(self) -> i64 {
        i64::from(self.menu_debounce_seconds) * 1_000
    }

    #[must_use]
    pub fn automatic_refresh_millis(self) -> i64 {
        i64::from(self.automatic_refresh_minutes) * 60 * 1_000
    }
}

impl Default for BalanceQueryPolicy {
    fn default() -> Self {
        Self {
            menu_debounce_seconds: DEFAULT_MENU_BALANCE_DEBOUNCE_SECONDS,
            automatic_refresh_minutes: DEFAULT_AUTOMATIC_BALANCE_REFRESH_MINUTES,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ImagesGenerationTimeout(u16);

impl ImagesGenerationTimeout {
    /// Validates the durable image response-header wait budget.
    ///
    /// # Errors
    ///
    /// Returns a field-specific error when the value is outside 600..=3,600
    /// seconds.
    pub fn parse(seconds: u16) -> Result<Self, ValidationError> {
        if !(MIN_IMAGES_GENERATION_TIMEOUT_SECS..=MAX_IMAGES_GENERATION_TIMEOUT_SECS)
            .contains(&seconds)
        {
            return Err(ValidationError::new(
                "images_generation_timeout_out_of_range",
                "timeoutSecs",
            ));
        }
        Ok(Self(seconds))
    }

    #[must_use]
    pub const fn seconds(self) -> u16 {
        self.0
    }

    #[must_use]
    pub fn duration(self) -> Duration {
        Duration::from_secs(u64::from(self.0))
    }
}

impl Default for ImagesGenerationTimeout {
    fn default() -> Self {
        Self(DEFAULT_IMAGES_GENERATION_TIMEOUT_SECS)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct McpImageCapacityWarningThreshold(u32);

impl McpImageCapacityWarningThreshold {
    /// Validates the advisory MCP image capacity warning threshold.
    ///
    /// # Errors
    ///
    /// Returns a field-specific error outside 128..=102,400 MiB.
    pub fn parse(mebibytes: u32) -> Result<Self, ValidationError> {
        if !(MIN_MCP_IMAGE_CAPACITY_WARNING_MIB..=MAX_MCP_IMAGE_CAPACITY_WARNING_MIB)
            .contains(&mebibytes)
        {
            return Err(ValidationError::new(
                "mcp_image_capacity_warning_out_of_range",
                "thresholdMib",
            ));
        }
        Ok(Self(mebibytes))
    }

    #[must_use]
    pub const fn mebibytes(self) -> u32 {
        self.0
    }

    #[must_use]
    pub fn bytes(self) -> u64 {
        u64::from(self.0) * 1_024 * 1_024
    }
}

impl Default for McpImageCapacityWarningThreshold {
    fn default() -> Self {
        Self(DEFAULT_MCP_IMAGE_CAPACITY_WARNING_MIB)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CodexModel {
    model_id: String,
    display_name: Option<String>,
    context_window: Option<u64>,
}

impl CodexModel {
    /// Normalizes one user-authored Codex model row.
    ///
    /// # Errors
    ///
    /// Returns a row-addressable error for an empty/control-bearing model ID,
    /// invalid display name, or an unsafe context-window value.
    pub fn parse(
        row_index: usize,
        model_id: &str,
        display_name: Option<&str>,
        context_window: Option<u64>,
    ) -> Result<Self, CodexModelValidationError> {
        let field = |name: &str| format!("models.{row_index}.{name}");
        let model_id = model_id.trim();
        if model_id.is_empty() {
            return Err(CodexModelValidationError::new(
                "codex_model_id_required",
                field("modelId"),
            ));
        }
        if model_id.chars().any(char::is_control) {
            return Err(CodexModelValidationError::new(
                "codex_model_id_control_character",
                field("modelId"),
            ));
        }
        let display_name = display_name
            .map(str::trim)
            .filter(|value| !value.is_empty());
        if display_name.is_some_and(|value| value.chars().any(char::is_control)) {
            return Err(CodexModelValidationError::new(
                "codex_model_display_name_control_character",
                field("displayName"),
            ));
        }
        if context_window.is_some_and(|value| value == 0 || value > MAX_CODEX_MODEL_CONTEXT_WINDOW)
        {
            return Err(CodexModelValidationError::new(
                "codex_model_context_window_invalid",
                field("contextWindow"),
            ));
        }
        Ok(Self {
            model_id: model_id.to_owned(),
            display_name: display_name.map(str::to_owned),
            context_window,
        })
    }

    #[must_use]
    pub fn model_id(&self) -> &str {
        &self.model_id
    }

    #[must_use]
    pub fn display_name(&self) -> Option<&str> {
        self.display_name.as_deref()
    }

    #[must_use]
    pub const fn context_window(&self) -> Option<u64> {
        self.context_window
    }
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
#[error("invalid field {field}: {code}")]
pub struct CodexModelValidationError {
    pub code: &'static str,
    pub field: String,
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
#[error("invalid field {field}: {code}")]
pub struct FallbackExcludedModelValidationError {
    pub code: &'static str,
    pub field: String,
}

impl FallbackExcludedModelValidationError {
    #[must_use]
    pub fn required(index: usize) -> Self {
        Self {
            code: "fallback_excluded_model_required",
            field: format!("fallbackExcludedModels.{index}"),
        }
    }

    #[must_use]
    pub fn control_character(index: usize) -> Self {
        Self {
            code: "fallback_excluded_model_control_character",
            field: format!("fallbackExcludedModels.{index}"),
        }
    }

    #[must_use]
    pub fn duplicate(index: usize) -> Self {
        Self {
            code: "fallback_excluded_model_duplicate",
            field: format!("fallbackExcludedModels.{index}"),
        }
    }
}

impl CodexModelValidationError {
    fn new(code: &'static str, field: String) -> Self {
        Self { code, field }
    }

    #[must_use]
    pub fn duplicate(row_index: usize) -> Self {
        Self::new(
            "codex_model_id_duplicate",
            format!("models.{row_index}.modelId"),
        )
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum ProxyRuntimeStatus {
    Stopped,
    Starting,
    Running,
    PortConflict,
    Error,
    DatabaseError,
    ShuttingDown,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum CompletionState {
    NoUpstream,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum DeliveryState {
    None,
    Started,
    Completed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum InferenceOutcome {
    Success,
    Failure,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum InferenceFailureReason {
    Connection,
    Timeout,
    Service,
    RateLimit,
    InvalidKey,
    InsufficientQuota,
    BillingLimit,
    Authentication,
    AccessDenied,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum InferenceStatusKind {
    Unverified,
    RecentSuccess,
    RecentFailure,
    Expired,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct InferenceStatus {
    pub kind: InferenceStatusKind,
    pub last_outcome: Option<InferenceOutcome>,
    pub failure_reason: Option<InferenceFailureReason>,
    #[ts(type = "number | null")]
    pub observed_at_ms: Option<i64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct ProxyRequestMetadata {
    pub request_id: ProxyRequestId,
    pub route_id: Option<RouteId>,
    pub model: Option<String>,
    pub streaming: bool,
    pub completion_state: CompletionState,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct UpstreamAttemptMetadata {
    pub attempt_id: UpstreamAttemptId,
    pub request_id: ProxyRequestId,
    pub route_id: RouteId,
    pub attempt_index: u32,
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
#[error("invalid field {field}: {code}")]
pub struct ValidationError {
    pub code: &'static str,
    pub field: &'static str,
}

impl ValidationError {
    const fn new(code: &'static str, field: &'static str) -> Self {
        Self { code, field }
    }
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::{
        ApiKey, BalanceQueryPolicy, BalanceScriptSource, BaseUrl, CodexModel,
        ImagesGenerationTimeout, MAX_BASE_URL_BYTES, MAX_CODEX_MODEL_CONTEXT_WINDOW,
        McpImageCapacityWarningThreshold, RouteName, ServiceTierPolicy,
    };

    #[derive(Deserialize)]
    struct BaseUrlFixture {
        input: String,
        canonical: Option<String>,
        inference: Option<String>,
        error: Option<String>,
    }

    fn base_url_fixtures() -> Vec<BaseUrlFixture> {
        serde_json::from_str(include_str!("../../../fixtures/base-url-contract.json"))
            .expect("base URL fixtures")
    }

    #[test]
    fn route_name_normalizes_and_rejects_invalid_values() {
        let name = RouteName::parse("  Work Key  ").expect("valid name");
        assert_eq!(name.as_str(), "Work Key");
        assert_eq!(name.comparison_key(), "work key");
        assert!(RouteName::parse("\n").is_err());
        assert!(RouteName::parse(&"x".repeat(30)).is_ok());
        assert!(RouteName::parse(&"x".repeat(31)).is_err());
    }

    #[test]
    fn base_url_matches_the_shared_cross_layer_contract() {
        for fixture in base_url_fixtures() {
            if let Some(expected_code) = fixture.error {
                let error = BaseUrl::parse(&fixture.input).expect_err("invalid URL");
                assert_eq!(error.code, expected_code, "input: {}", fixture.input);
                assert_eq!(error.field, "baseUrl", "input: {}", fixture.input);
            } else {
                let base = BaseUrl::parse(&fixture.input).expect("valid URL");
                assert_eq!(
                    Some(base.as_str()),
                    fixture.canonical.as_deref(),
                    "input: {}",
                    fixture.input
                );
                assert_eq!(
                    base.inference_url(),
                    fixture.inference.expect("valid fixture inference URL"),
                    "input: {}",
                    fixture.input
                );
            }
        }
    }

    #[test]
    fn base_url_enforces_the_utf8_byte_limit() {
        let oversized = format!("https://example.com/{}", "x".repeat(MAX_BASE_URL_BYTES));
        let error = BaseUrl::parse(&oversized).expect_err("oversized URL");
        assert_eq!(error.code, "base_url_too_long");
        assert_eq!(error.field, "baseUrl");
    }

    #[test]
    fn balance_query_policy_accepts_only_the_configured_inclusive_ranges() {
        assert!(BalanceQueryPolicy::parse(10, 5).is_ok());
        assert!(BalanceQueryPolicy::parse(600, 1_440).is_ok());
        assert_eq!(
            BalanceQueryPolicy::parse(9, 30)
                .expect_err("short debounce")
                .field,
            "menuDebounceSeconds"
        );
        assert_eq!(
            BalanceQueryPolicy::parse(30, 1_441)
                .expect_err("long refresh")
                .field,
            "automaticRefreshMinutes"
        );
    }

    #[test]
    fn images_generation_timeout_accepts_only_its_inclusive_range() {
        let default = ImagesGenerationTimeout::default();
        assert_eq!(default.seconds(), 600);
        assert_eq!(default.duration().as_secs(), 600);
        assert!(ImagesGenerationTimeout::parse(600).is_ok());
        assert!(ImagesGenerationTimeout::parse(3_600).is_ok());
        for invalid in [0, 599, 3_601, u16::MAX] {
            let error = ImagesGenerationTimeout::parse(invalid).expect_err("invalid timeout");
            assert_eq!(error.code, "images_generation_timeout_out_of_range");
            assert_eq!(error.field, "timeoutSecs");
        }
    }

    #[test]
    fn mcp_image_capacity_warning_accepts_only_its_inclusive_range() {
        let default = McpImageCapacityWarningThreshold::default();
        assert_eq!(default.mebibytes(), 1_024);
        assert_eq!(default.bytes(), 1_073_741_824);
        assert!(McpImageCapacityWarningThreshold::parse(128).is_ok());
        assert!(McpImageCapacityWarningThreshold::parse(102_400).is_ok());
        for invalid in [0, 127, 102_401, u32::MAX] {
            let error = McpImageCapacityWarningThreshold::parse(invalid)
                .expect_err("invalid capacity warning");
            assert_eq!(error.code, "mcp_image_capacity_warning_out_of_range");
            assert_eq!(error.field, "thresholdMib");
        }
    }

    #[test]
    fn key_and_script_limits_are_enforced() {
        let key = ApiKey::parse("  secret  ").expect("valid key");
        assert_eq!(key.expose(), b"secret");
        assert!(ApiKey::parse("secret\nvalue").is_err());
        assert!(BalanceScriptSource::parse(&"x".repeat(256 * 1024 + 1)).is_err());
    }

    #[test]
    fn service_tier_policy_parsing_is_closed_and_defaults_to_passthrough() {
        assert_eq!(ServiceTierPolicy::default(), ServiceTierPolicy::Passthrough);
        assert_eq!(
            ServiceTierPolicy::parse_persisted("passthrough"),
            Ok(ServiceTierPolicy::Passthrough)
        );
        assert_eq!(
            ServiceTierPolicy::parse_persisted("omit"),
            Ok(ServiceTierPolicy::Omit)
        );
        let error =
            ServiceTierPolicy::parse_persisted("default").expect_err("unknown persisted policy");
        assert_eq!(error.code, "service_tier_policy_invalid");
        assert_eq!(error.field, "serviceTierPolicy");
    }

    #[test]
    fn codex_models_normalize_optional_fields_and_report_the_owning_row() {
        let model =
            CodexModel::parse(2, "  relay-model  ", Some("  Relay  "), None).expect("valid model");
        assert_eq!(model.model_id(), "relay-model");
        assert_eq!(model.display_name(), Some("Relay"));
        assert_eq!(model.context_window(), None);
        assert_eq!(
            CodexModel::parse(4, "\n", None, None)
                .expect_err("blank model")
                .field,
            "models.4.modelId"
        );
        assert_eq!(
            CodexModel::parse(1, "relay", None, Some(0))
                .expect_err("zero context")
                .field,
            "models.1.contextWindow"
        );
        assert!(
            CodexModel::parse(0, "relay", None, Some(MAX_CODEX_MODEL_CONTEXT_WINDOW + 1),).is_err()
        );
    }
}
