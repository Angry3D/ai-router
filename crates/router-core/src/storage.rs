use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    thread,
    time::{SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use chrono::{
    DateTime, Datelike, Duration as ChronoDuration, LocalResult, NaiveDate, NaiveDateTime,
    TimeZone, Timelike, Utc,
};
use chrono_tz::Tz;
use rusqlite::{Connection, OptionalExtension, Transaction, backup::Backup, params};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::sync::{mpsc, oneshot, watch};
use uuid::Uuid;

use crate::balance::{
    BalanceError, BalanceErrorCategory, BalanceErrorStage, BalanceQueryConfig, BalanceQueryMode,
    BalanceRouteConfig, BalanceRouteSource,
};
use crate::domain::{
    ApiKey, AppearancePreference, BalanceQueryPolicy, BalanceScriptSource, BaseUrl, CodexModel,
    CodexModelValidationError, CompletionState, DeliveryState,
    FallbackExcludedModelValidationError, ImagesGenerationTimeout,
    McpImageCapacityWarningThreshold, RouteId, RouteMoveDirection, RouteName, SecretId,
    ServiceTierPolicy, UpstreamAttemptId, ValidationError,
};
use crate::pricing::{CostStatus, PricedUsage, UsageObservation, fold_request_cost, price_usage};

const DATABASE_QUEUE_CAPACITY: usize = 1_024;
pub const SCHEMA_VERSION: i64 = 22;

const GENERAL_BALANCE_SOURCE_HASHES: [&str; 3] = [
    "24cbea85c2fa635112e5915836e2a78144e0a6a21997b86ef5187c2665e14507",
    "be1d8023ddf04aa987b91d856637eeb86a21a6d504f4475ca2f2945b3132ff6c",
    "f60ff5d32ac946ac0fb8dd616aff15673710f534631464bfb0517833d9170390",
];

fn is_general_balance_source_hash(source_hash: &str) -> bool {
    GENERAL_BALANCE_SOURCE_HASHES.contains(&source_hash)
}

type DatabaseJob = Box<dyn FnOnce(&mut Connection) + Send + 'static>;
type AppSettingsRow = (
    i64,
    bool,
    bool,
    i64,
    i64,
    i64,
    Option<String>,
    i64,
    String,
    Option<i64>,
    i64,
    i64,
    i64,
    Option<String>,
    Option<String>,
);

#[derive(Clone)]
pub struct DatabaseExecutor {
    sender: mpsc::Sender<DatabaseJob>,
    path: Arc<PathBuf>,
    critical_revision_sender: watch::Sender<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RouteRecord {
    pub route_id: RouteId,
    pub name: String,
    pub base_url: String,
    pub secret_id: SecretId,
    pub service_tier_policy: ServiceTierPolicy,
    pub sort_order: i64,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FallbackConfigRecord {
    pub enabled: bool,
    pub participant_count: u32,
    pub config_revision: u64,
    pub updated_at_ms: i64,
}

struct ValidatedFallbackConfig {
    record: FallbackConfigRecord,
    route_count: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RoutingStateRecord {
    pub active_route_id: Option<RouteId>,
    pub selection_generation: u64,
    pub fallback: FallbackConfigRecord,
}

pub struct CreateRouteInput {
    pub name: String,
    pub base_url: String,
    pub api_key: ApiKey,
    pub service_tier_policy: ServiceTierPolicy,
    pub balance_query: Option<BalanceQueryInput>,
    pub accept_script_risk: bool,
}

pub struct UpdateRouteInput {
    pub route_id: RouteId,
    pub name: String,
    pub base_url: String,
    pub api_key: ApiKey,
    pub service_tier_policy: ServiceTierPolicy,
    pub balance_query: Option<BalanceQueryInput>,
    pub accept_script_risk: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BalanceQueryInput {
    pub mode: BalanceQueryMode,
    pub enabled: bool,
    pub custom_source: String,
}

pub struct RouteEditRecord {
    pub route: RouteRecord,
    pub api_key: ApiKey,
    pub balance_query: Option<BalanceQueryInput>,
    pub fallback_excluded_models: Vec<String>,
    pub models: Vec<CodexModelRecord>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CodexRestartNoticeRecord {
    pub notice_id: String,
    pub route_id: RouteId,
    pub selection_generation: u64,
    pub catalog_fingerprint: String,
    pub created_at_ms: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeleteRouteResult {
    pub deleted_route_id: RouteId,
    pub cleared_active_route: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistorySummary {
    pub request_count: u64,
    pub earliest_started_at_ms: Option<i64>,
    pub latest_started_at_ms: Option<i64>,
    pub database_bytes: u64,
    pub retention_days: u16,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppSettingsRecord {
    pub proxy_port: u16,
    pub first_run_presented: bool,
    pub balance_script_risk_confirmed: bool,
    pub balance_query_policy: BalanceQueryPolicy,
    pub images_generation_enabled: bool,
    pub images_generation_route_id: Option<RouteId>,
    pub images_generation_timeout: ImagesGenerationTimeout,
    pub appearance_preference: AppearancePreference,
    pub last_automatic_update_check_at_ms: Option<i64>,
    pub menu_bar: MenuBarSettingsRecord,
    pub mcp_image_capacity: McpImageCapacitySettingsRecord,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MenuBarSettingsRecord {
    pub status_text_enabled: bool,
    pub activity_animation_enabled: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct McpImageCapacitySettingsRecord {
    pub threshold: McpImageCapacityWarningThreshold,
    pub active_episode_id: Option<String>,
    pub dismissed_episode_id: Option<String>,
}

impl McpImageCapacitySettingsRecord {
    #[must_use]
    pub fn over_threshold(&self) -> bool {
        self.active_episode_id.is_some()
    }

    #[must_use]
    pub fn warning_visible(&self) -> bool {
        self.active_episode_id.is_some() && self.active_episode_id != self.dismissed_episode_id
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClearHistoryResult {
    pub deleted_requests: u64,
    pub reclaim_succeeded: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequestHistoryRecord {
    pub request_id: String,
    pub started_at_ms: i64,
    pub finished_at_ms: i64,
    pub turn_id: Option<String>,
    pub requested_model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub requested_service_tier: Option<String>,
    pub actual_model: Option<String>,
    pub actual_service_tier: Option<String>,
    pub final_route_id: Option<RouteId>,
    pub final_route_name: Option<String>,
    pub streaming: bool,
    pub completion_state: CompletionState,
    pub http_status: Option<u16>,
    pub error_category: Option<String>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub total_tokens: Option<i64>,
    pub cached_input_tokens: Option<i64>,
    pub cache_write_input_tokens: Option<i64>,
    pub total_latency_ms: Option<i64>,
    pub first_output_latency_ms: Option<i64>,
    pub metadata_complete: bool,
    pub fallback_stop_reason: Option<FallbackStopReason>,
    pub fallback_stop_target_route_id: Option<RouteId>,
    pub fallback_stop_target_route_name: Option<String>,
    pub attempts: Vec<AttemptHistoryRecord>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FallbackStopReason {
    FallbackDisabled,
    FailureNotEligible,
    ResponseCommitted,
    AllParticipantsAttempted,
    StalePolicy,
    ActivationFailed,
    AttemptIndexExhausted,
    FailureThresholdNotReached,
    FailureThresholdReachedPending,
    RecoveryConfirmationPending,
    ModelFallbackExcluded,
}

impl FallbackStopReason {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FallbackDisabled => "fallback_disabled",
            Self::FailureNotEligible => "failure_not_eligible",
            Self::ResponseCommitted => "response_committed",
            Self::AllParticipantsAttempted => "all_participants_attempted",
            Self::StalePolicy => "stale_policy",
            Self::ActivationFailed => "activation_failed",
            Self::AttemptIndexExhausted => "attempt_index_exhausted",
            Self::FailureThresholdNotReached => "failure_threshold_not_reached",
            Self::FailureThresholdReachedPending => "failure_threshold_reached_pending",
            Self::RecoveryConfirmationPending => "recovery_confirmation_pending",
            Self::ModelFallbackExcluded => "model_fallback_excluded",
        }
    }

    fn parse(value: &str) -> Result<Self, StorageError> {
        match value {
            "fallback_disabled" => Ok(Self::FallbackDisabled),
            "failure_not_eligible" => Ok(Self::FailureNotEligible),
            "response_committed" => Ok(Self::ResponseCommitted),
            "all_participants_attempted" => Ok(Self::AllParticipantsAttempted),
            "stale_policy" => Ok(Self::StalePolicy),
            "activation_failed" => Ok(Self::ActivationFailed),
            "attempt_index_exhausted" => Ok(Self::AttemptIndexExhausted),
            "failure_threshold_not_reached" => Ok(Self::FailureThresholdNotReached),
            "failure_threshold_reached_pending" => Ok(Self::FailureThresholdReachedPending),
            "recovery_confirmation_pending" => Ok(Self::RecoveryConfirmationPending),
            "model_fallback_excluded" => Ok(Self::ModelFallbackExcluded),
            _ => Err(StorageError::Initialization),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum AttemptRole {
    #[default]
    Ordinary,
    RecoveryProbe,
}

impl AttemptRole {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ordinary => "ordinary",
            Self::RecoveryProbe => "recovery_probe",
        }
    }

    fn parse(value: &str) -> Result<Self, StorageError> {
        match value {
            "ordinary" => Ok(Self::Ordinary),
            "recovery_probe" => Ok(Self::RecoveryProbe),
            _ => Err(StorageError::Initialization),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RoutingTransitionKind {
    ActivateNext,
    ResumeCaptured,
    Recover,
}

impl RoutingTransitionKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ActivateNext => "activate_next",
            Self::ResumeCaptured => "resume_captured",
            Self::Recover => "recover",
        }
    }

    fn parse(value: &str) -> Result<Self, StorageError> {
        match value {
            "activate_next" => Ok(Self::ActivateNext),
            "resume_captured" => Ok(Self::ResumeCaptured),
            "recover" => Ok(Self::Recover),
            _ => Err(StorageError::Initialization),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RoutingTransitionSkip {
    pub route_id: RouteId,
    pub route_name: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttemptRoutingTransition {
    pub kind: RoutingTransitionKind,
    pub target_route_id: RouteId,
    pub target_route_name: String,
    pub skipped_routes: Vec<RoutingTransitionSkip>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttemptRoutingTransitionRecord {
    pub request_id: String,
    pub attempt_index: u32,
    pub transition: AttemptRoutingTransition,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FallbackStopRecord {
    pub request_id: String,
    pub attempt_index: u32,
    pub reason: FallbackStopReason,
    pub target_route_id: Option<RouteId>,
    pub target_route_name: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttemptHistoryRecord {
    pub attempt_id: UpstreamAttemptId,
    pub attempt_index: u32,
    pub attempt_role: AttemptRole,
    pub route_id: RouteId,
    pub route_name: String,
    pub started_at_ms: i64,
    pub finished_at_ms: i64,
    pub http_status: Option<u16>,
    pub error_category: Option<String>,
    pub delivery_state: DeliveryState,
    pub actual_model: Option<String>,
    pub forwarded_service_tier: Option<String>,
    pub actual_service_tier: Option<String>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub total_tokens: Option<i64>,
    pub cached_input_tokens: Option<i64>,
    pub cache_write_input_tokens: Option<i64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UsageHistoryCursor {
    pub finished_at_ms: i64,
    pub request_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UsageHistoryQuery {
    pub finished_at_or_after_ms: Option<i64>,
    pub finished_at_or_before_ms: i64,
    pub completion_state: Option<CompletionState>,
    pub route_id: Option<RouteId>,
    pub model_contains: Option<String>,
    pub cursor: Option<UsageHistoryCursor>,
    pub limit: u16,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UsageHistoryRow {
    pub request_id: String,
    pub started_at_ms: i64,
    pub finished_at_ms: Option<i64>,
    pub final_route_id: Option<RouteId>,
    pub final_route_name: Option<String>,
    pub requested_model: Option<String>,
    pub actual_model: Option<String>,
    pub actual_service_tier: Option<String>,
    pub reasoning_effort: Option<String>,
    pub streaming: bool,
    pub completion_state: CompletionState,
    pub http_status: Option<u16>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub total_tokens: Option<i64>,
    pub cached_input_tokens: Option<i64>,
    pub cache_write_input_tokens: Option<i64>,
    pub total_latency_ms: Option<i64>,
    pub first_output_latency_ms: Option<i64>,
    pub pricing_catalog_version: Option<String>,
    pub cost_status: Option<CostStatus>,
    pub upstream_cost_pico_usd: Option<i64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UsageHistoryPage {
    pub rows: Vec<UsageHistoryRow>,
    pub next_cursor: Option<UsageHistoryCursor>,
    pub total_rows: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UsageStatisticsGranularity {
    Hour,
    Day,
    Month,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UsageStatisticsAttributionDimension {
    Route,
    Model,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UsageStatisticsAttributionMetric {
    Requests,
    Tokens,
    Cost,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UsageStatisticsQuery {
    pub finished_at_or_after_ms: Option<i64>,
    pub finished_at_or_before_ms: i64,
    pub route_id: Option<RouteId>,
    pub model_contains: Option<String>,
    pub time_zone: String,
    pub attribution_dimension: UsageStatisticsAttributionDimension,
    pub attribution_metric: UsageStatisticsAttributionMetric,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UsageStatisticsTokens {
    pub total: u64,
    pub uncached_input: u64,
    pub cached_input: u64,
    pub cache_write_input: u64,
    pub output: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UsageStatisticsBucket {
    pub started_at_ms: i64,
    pub finished_at_ms: i64,
    pub label: String,
    pub request_count: u64,
    pub tokens: UsageStatisticsTokens,
    pub cost_pico_usd: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UsageStatisticsAttribution {
    pub key: String,
    pub label: String,
    pub is_other: bool,
    pub value: u64,
    pub share_percent: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UsageStatistics {
    pub matched_request_count: u64,
    pub tokens: UsageStatisticsTokens,
    pub cost_pico_usd: u64,
    pub granularity: UsageStatisticsGranularity,
    pub trend: Vec<UsageStatisticsBucket>,
    pub attribution: Vec<UsageStatisticsAttribution>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UsageRouteOption {
    pub route_id: RouteId,
    pub name: String,
    pub retained: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UsageAttemptDetail {
    pub attempt_index: u32,
    pub attempt_role: AttemptRole,
    pub route_id: RouteId,
    pub route_name: String,
    pub started_at_ms: i64,
    pub finished_at_ms: Option<i64>,
    pub http_status: Option<u16>,
    pub error_category: Option<String>,
    pub delivery_state: DeliveryState,
    pub actual_model: Option<String>,
    pub forwarded_service_tier: Option<String>,
    pub actual_service_tier: Option<String>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub total_tokens: Option<i64>,
    pub cached_input_tokens: Option<i64>,
    pub cache_write_input_tokens: Option<i64>,
    pub pricing_catalog_version: Option<String>,
    pub cost_status: Option<CostStatus>,
    pub cost_pico_usd: Option<i64>,
    pub routing_transition: Option<AttemptRoutingTransition>,
    pub routing_decision: Option<RoutingDecision>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RoutingDecision {
    RetryCurrent {
        attempt_number: u32,
        max_attempts: u32,
    },
    ActivateNext {
        target_route_id: RouteId,
        target_route_name: String,
        skipped_routes: Vec<RoutingTransitionSkip>,
    },
    ResumeCaptured {
        target_route_id: RouteId,
        target_route_name: String,
    },
    Recover {
        target_route_id: RouteId,
        target_route_name: String,
    },
    Stop {
        reason: FallbackStopReason,
        target_route_id: Option<RouteId>,
        target_route_name: Option<String>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UsageRequestDetail {
    pub request: UsageHistoryRow,
    pub requested_service_tier: Option<String>,
    pub actual_service_tier: Option<String>,
    pub cached_input_tokens: Option<i64>,
    pub cache_write_input_tokens: Option<i64>,
    pub attempts: Vec<UsageAttemptDetail>,
}

fn materialize_routing_decisions(
    attempts: &mut [UsageAttemptDetail],
    stop_reason: Option<FallbackStopReason>,
    stop_target_route_id: Option<&RouteId>,
    stop_target_route_name: Option<&str>,
) {
    const LEGACY_ROUTER_MAX_ATTEMPTS: u32 = 4;

    for index in 0..attempts.len() {
        if let Some(transition) = attempts[index].routing_transition.clone() {
            attempts[index].routing_decision = Some(match transition.kind {
                RoutingTransitionKind::ActivateNext => RoutingDecision::ActivateNext {
                    target_route_id: transition.target_route_id,
                    target_route_name: transition.target_route_name,
                    skipped_routes: transition.skipped_routes,
                },
                RoutingTransitionKind::ResumeCaptured => RoutingDecision::ResumeCaptured {
                    target_route_id: transition.target_route_id,
                    target_route_name: transition.target_route_name,
                },
                RoutingTransitionKind::Recover => RoutingDecision::Recover {
                    target_route_id: transition.target_route_id,
                    target_route_name: transition.target_route_name,
                },
            });
            continue;
        }
        let decision = attempts.get(index + 1).map_or_else(
            || {
                stop_reason.map(|reason| RoutingDecision::Stop {
                    reason,
                    target_route_id: stop_target_route_id.cloned(),
                    target_route_name: stop_target_route_name.map(str::to_owned),
                })
            },
            |next| {
                if next.route_id == attempts[index].route_id {
                    let completed_on_route = attempts[..=index]
                        .iter()
                        .filter(|attempt| attempt.route_id == attempts[index].route_id)
                        .count();
                    Some(RoutingDecision::RetryCurrent {
                        attempt_number: u32::try_from(completed_on_route)
                            .unwrap_or(u32::MAX)
                            .saturating_add(1),
                        max_attempts: LEGACY_ROUTER_MAX_ATTEMPTS,
                    })
                } else {
                    Some(RoutingDecision::ActivateNext {
                        target_route_id: next.route_id.clone(),
                        target_route_name: next.route_name.clone(),
                        skipped_routes: Vec::new(),
                    })
                }
            },
        );
        attempts[index].routing_decision = decision;
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LatestInferenceAttempt {
    pub route_id: RouteId,
    pub finished_at_ms: i64,
    pub succeeded: bool,
    pub error_category: Option<String>,
}

pub struct CodexBaseline {
    pub original_exists: bool,
    pub raw_bytes: Vec<u8>,
    pub unix_mode: Option<u32>,
    pub captured_at_ms: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CodexRecoveryConfig {
    pub original_exists: bool,
    pub raw_bytes: Vec<u8>,
    pub unix_mode: Option<u32>,
    pub updated_at_ms: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CodexModelRecord {
    pub model_id: String,
    pub display_name: Option<String>,
    pub context_window: Option<u64>,
}

impl From<CodexModel> for CodexModelRecord {
    fn from(model: CodexModel) -> Self {
        Self {
            model_id: model.model_id().to_owned(),
            display_name: model.display_name().map(str::to_owned),
            context_window: model.context_window(),
        }
    }
}

/// Normalizes and validates a complete candidate model list before persistence.
///
/// # Errors
///
/// Returns the first row-addressable field or duplicate error.
pub fn normalize_codex_model_records(
    models: Vec<CodexModelRecord>,
) -> Result<Vec<CodexModelRecord>, CodexModelValidationError> {
    let mut normalized = Vec::with_capacity(models.len());
    let mut model_ids = std::collections::HashSet::with_capacity(models.len());
    for (index, model) in models.into_iter().enumerate() {
        let model = CodexModel::parse(
            index,
            &model.model_id,
            model.display_name.as_deref(),
            model.context_window,
        )?;
        if !model_ids.insert(model.model_id().to_owned()) {
            return Err(CodexModelValidationError::duplicate(index));
        }
        normalized.push(CodexModelRecord::from(model));
    }
    Ok(normalized)
}

/// Normalizes a complete route-owned Fallback exclusion list.
///
/// # Errors
///
/// Returns a field-addressable error for blank, control-bearing, or duplicate
/// model identifiers.
pub fn normalize_fallback_excluded_models(
    models: Vec<String>,
) -> Result<Vec<String>, FallbackExcludedModelValidationError> {
    let mut normalized = Vec::with_capacity(models.len());
    let mut model_ids = std::collections::HashSet::with_capacity(models.len());
    for (index, model) in models.into_iter().enumerate() {
        let model = model.trim();
        if model.is_empty() {
            return Err(FallbackExcludedModelValidationError::required(index));
        }
        if model.chars().any(char::is_control) {
            return Err(FallbackExcludedModelValidationError::control_character(
                index,
            ));
        }
        if !model_ids.insert(model.to_owned()) {
            return Err(FallbackExcludedModelValidationError::duplicate(index));
        }
        normalized.push(model.to_owned());
    }
    Ok(normalized)
}

#[derive(Debug, Error)]
pub enum StorageError {
    #[error(transparent)]
    Validation(#[from] ValidationError),
    #[error(transparent)]
    CodexModelValidation(#[from] CodexModelValidationError),
    #[error(transparent)]
    FallbackExcludedModelValidation(#[from] FallbackExcludedModelValidationError),
    #[error("database executor is closed")]
    ExecutorClosed,
    #[error("database initialization failed")]
    Initialization,
    #[error("database schema is newer than this application")]
    FutureSchema,
    #[error("entity not found")]
    NotFound,
    #[error("invalid usage query")]
    InvalidUsageQuery,
    #[error("usage statistics overflow")]
    UsageStatisticsOverflow,
    #[error("invalid fallback participant count")]
    InvalidFallbackParticipantCount,
    #[error("routing configuration is stale")]
    StaleRoutingConfiguration,
    #[error("invalid route permutation")]
    InvalidRoutePermutation,
    #[error("a capable image generation route is required")]
    InvalidImagesGenerationRoute,
    #[error("balance script risk confirmation is required")]
    BalanceScriptRiskConfirmationRequired,
    #[error("database operation failed")]
    Database(#[from] rusqlite::Error),
    #[error("filesystem operation failed")]
    Filesystem(#[from] std::io::Error),
}

impl DatabaseExecutor {
    /// Opens, migrates, and validates a database on its dedicated thread.
    ///
    /// # Errors
    ///
    /// Returns an error when private paths cannot be prepared, `SQLite` cannot be
    /// opened or migrated, or required PRAGMA/integrity checks fail.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StorageError> {
        let path = path.as_ref().to_path_buf();
        prepare_database_path(&path)?;

        let (sender, mut receiver) = mpsc::channel::<DatabaseJob>(DATABASE_QUEUE_CAPACITY);
        let (critical_revision_sender, _) = watch::channel(0);
        let (ready_sender, ready_receiver) = std::sync::mpsc::sync_channel(1);
        let thread_path = path.clone();

        thread::Builder::new()
            .name("ai-router-database".to_owned())
            .spawn(move || match open_connection(&thread_path) {
                Ok(mut connection) => {
                    let revision = current_critical_revision(&connection);
                    let _ = ready_sender.send(revision);
                    while let Some(job) = receiver.blocking_recv() {
                        job(&mut connection);
                    }
                }
                Err(error) => {
                    let _ = ready_sender.send(Err(error));
                }
            })?;

        let critical_revision = ready_receiver
            .recv()
            .map_err(|_| StorageError::Initialization)??;
        critical_revision_sender.send_replace(critical_revision);
        enforce_database_file_permissions(&path)?;

        Ok(Self {
            sender,
            path: Arc::new(path),
            critical_revision_sender,
        })
    }

    async fn call<T, F>(&self, operation: F) -> Result<T, StorageError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T, StorageError> + Send + 'static,
    {
        let (reply_sender, reply_receiver) = oneshot::channel();
        self.sender
            .send(Box::new(move |connection| {
                let _ = reply_sender.send(operation(connection));
            }))
            .await
            .map_err(|_| StorageError::ExecutorClosed)?;
        reply_receiver
            .await
            .map_err(|_| StorageError::ExecutorClosed)?
    }

    async fn call_critical<T, F>(&self, operation: F) -> Result<T, StorageError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<(T, Option<u64>), StorageError> + Send + 'static,
    {
        let (reply_sender, reply_receiver) = oneshot::channel();
        let revision_sender = self.critical_revision_sender.clone();
        self.sender
            .send(Box::new(move |connection| {
                let result = operation(connection);
                if let Ok((_, Some(revision))) = &result {
                    revision_sender.send_replace(*revision);
                }
                let _ = reply_sender.send(result.map(|(value, _)| value));
            }))
            .await
            .map_err(|_| StorageError::ExecutorClosed)?;
        reply_receiver
            .await
            .map_err(|_| StorageError::ExecutorClosed)?
    }

    /// Subscribes to the latest revision committed by a critical-state mutation.
    #[must_use]
    pub fn subscribe_critical_revisions(&self) -> watch::Receiver<u64> {
        self.critical_revision_sender.subscribe()
    }

    /// Returns the durable revision represented by the current live database.
    ///
    /// # Errors
    ///
    /// Returns an executor or `SQLite` query error.
    pub async fn critical_revision(&self) -> Result<u64, StorageError> {
        self.call(|connection| current_critical_revision(connection))
            .await
    }

    /// Loads one route's Codex model list in its explicit user order.
    ///
    /// # Errors
    ///
    /// Returns an executor, database, or persisted-domain validation error.
    pub async fn list_codex_models(
        &self,
        route_id: RouteId,
    ) -> Result<Vec<CodexModelRecord>, StorageError> {
        self.call(move |connection| read_codex_models(connection, &route_id))
            .await
    }

    /// Loads one route's exact, case-sensitive Fallback exclusion list.
    ///
    /// # Errors
    ///
    /// Returns an executor, database, or persisted-domain validation error.
    pub async fn list_fallback_excluded_models(
        &self,
        route_id: RouteId,
    ) -> Result<Vec<String>, StorageError> {
        self.call(move |connection| read_fallback_excluded_models(connection, &route_id))
            .await
    }

    /// Loads all route-owned exclusion lists in stable route order.
    ///
    /// # Errors
    ///
    /// Returns an executor, database, or persisted-domain validation error.
    pub async fn all_fallback_excluded_models(
        &self,
    ) -> Result<std::collections::HashMap<RouteId, Vec<String>>, StorageError> {
        self.call(|connection| {
            let mut statement = connection
                .prepare("SELECT route_id FROM routes ORDER BY sort_order, created_at_ms")?;
            let route_ids = statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            drop(statement);
            route_ids
                .into_iter()
                .map(|route_id| {
                    let route_id = RouteId::from_string(route_id);
                    let models = read_fallback_excluded_models(connection, &route_id)?;
                    Ok((route_id, models))
                })
                .collect()
        })
        .await
    }

    /// Loads the active route's catalog, or an empty list when no route is active.
    ///
    /// # Errors
    ///
    /// Returns executor, database, or persisted-domain validation errors.
    pub async fn active_codex_models(&self) -> Result<Vec<CodexModelRecord>, StorageError> {
        self.call(|connection| {
            let route_id = connection
                .query_row(
                    "SELECT route_id FROM route_state WHERE singleton = 1",
                    [],
                    |row| row.get::<_, Option<String>>(0),
                )?
                .map(RouteId::from_string);
            route_id.map_or_else(
                || Ok(Vec::new()),
                |route_id| read_codex_models(connection, &route_id),
            )
        })
        .await
    }

    /// Validates and transactionally replaces one route's complete Codex model list.
    ///
    /// # Errors
    ///
    /// Returns a row-addressable validation error before mutation, or an
    /// executor/database error without leaving a partial list.
    pub async fn replace_codex_models(
        &self,
        route_id: RouteId,
        models: Vec<CodexModelRecord>,
    ) -> Result<Vec<CodexModelRecord>, StorageError> {
        let normalized = normalize_codex_model_records(models)?;
        let persisted = normalized.clone();
        self.call_critical(move |connection| {
            let transaction = connection.transaction()?;
            let exists: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM routes WHERE route_id = ?1)",
                [route_id.as_str()],
                |row| row.get(0),
            )?;
            if !exists {
                return Err(StorageError::NotFound);
            }
            let existing = read_codex_models(&transaction, &route_id)?;
            if existing == persisted {
                transaction.commit()?;
                return Ok((persisted, None));
            }
            write_codex_models(&transaction, &route_id, &persisted)?;
            transaction.execute(
                "DELETE FROM codex_restart_notice WHERE route_id = ?1 AND EXISTS (SELECT 1 FROM route_state WHERE singleton = 1 AND route_id = ?1)",
                [route_id.as_str()],
            )?;
            let revision = mark_critical_change(&transaction)?;
            transaction.commit()?;
            Ok((persisted, Some(revision)))
        })
        .await
    }

    /// Loads the latest persisted automatic-fallback restart notice.
    ///
    /// # Errors
    ///
    /// Returns executor, database, or persisted conversion errors.
    pub async fn codex_restart_notice(
        &self,
    ) -> Result<Option<CodexRestartNoticeRecord>, StorageError> {
        self.call(|connection| {
            connection
                .query_row(
                    "SELECT notice_id, route_id, selection_generation, catalog_fingerprint, created_at_ms FROM codex_restart_notice WHERE singleton = 1",
                    [],
                    |row| {
                        let selection_generation = u64::try_from(row.get::<_, i64>(2)?)
                            .map_err(|error| rusqlite::Error::FromSqlConversionFailure(
                                2,
                                rusqlite::types::Type::Integer,
                                Box::new(error),
                            ))?;
                        Ok(CodexRestartNoticeRecord {
                            notice_id: row.get(0)?,
                            route_id: RouteId::from_string(row.get(1)?),
                            selection_generation,
                            catalog_fingerprint: row.get(3)?,
                            created_at_ms: row.get(4)?,
                        })
                    },
                )
                .optional()
                .map_err(StorageError::from)
        })
        .await
    }

    /// Replaces the singleton notice only when its route selection remains current.
    ///
    /// # Errors
    ///
    /// Returns executor, database, or numeric conversion errors.
    pub async fn upsert_codex_restart_notice(
        &self,
        notice: CodexRestartNoticeRecord,
    ) -> Result<bool, StorageError> {
        self.call_critical(move |connection| {
            let transaction = connection.transaction()?;
            let current: (Option<String>, i64) = transaction.query_row(
                "SELECT route_id, selection_generation FROM route_state WHERE singleton = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            if current.0.as_deref() != Some(notice.route_id.as_str())
                || u64::try_from(current.1).ok() != Some(notice.selection_generation)
            {
                transaction.commit()?;
                return Ok((false, None));
            }
            transaction.execute(
                "INSERT INTO codex_restart_notice (singleton, notice_id, route_id, selection_generation, catalog_fingerprint, created_at_ms) VALUES (1, ?1, ?2, ?3, ?4, ?5) ON CONFLICT(singleton) DO UPDATE SET notice_id = excluded.notice_id, route_id = excluded.route_id, selection_generation = excluded.selection_generation, catalog_fingerprint = excluded.catalog_fingerprint, created_at_ms = excluded.created_at_ms",
                params![
                    notice.notice_id,
                    notice.route_id.as_str(),
                    i64::try_from(notice.selection_generation).map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?,
                    notice.catalog_fingerprint,
                    notice.created_at_ms,
                ],
            )?;
            let revision = mark_critical_change(&transaction)?;
            transaction.commit()?;
            Ok((true, Some(revision)))
        })
        .await
    }

    /// Deletes only the notice identified by the caller's opaque ID.
    ///
    /// # Errors
    ///
    /// Returns executor or database errors.
    pub async fn dismiss_codex_restart_notice(
        &self,
        notice_id: String,
    ) -> Result<bool, StorageError> {
        self.call_critical(move |connection| {
            let transaction = connection.transaction()?;
            let changed = transaction.execute(
                "DELETE FROM codex_restart_notice WHERE singleton = 1 AND notice_id = ?1",
                [notice_id],
            )? == 1;
            let revision = changed
                .then(|| mark_critical_change(&transaction))
                .transpose()?;
            transaction.commit()?;
            Ok((changed, revision))
        })
        .await
    }

    /// Copies the live database through `SQLite`'s Backup API on its owning thread.
    ///
    /// # Errors
    ///
    /// Returns an executor, source database, or destination database error.
    pub async fn backup_to(&self, destination: PathBuf) -> Result<(), StorageError> {
        self.call(move |source| {
            let mut destination_connection = Connection::open(&destination)?;
            let backup = Backup::new(source, &mut destination_connection)?;
            backup.run_to_completion(128, std::time::Duration::from_millis(1), None)?;
            drop(backup);
            destination_connection.close().map_err(|(_, error)| error)?;
            Ok(())
        })
        .await
    }

    /// Creates and validates a current-schema database at an absent path.
    ///
    /// # Errors
    ///
    /// Returns a typed storage error when private creation, migration, or validation fails.
    pub fn create_validated(path: impl AsRef<Path>) -> Result<(), StorageError> {
        let path = path.as_ref();
        if path.exists() {
            return Err(StorageError::Initialization);
        }
        prepare_database_path(path)?;
        let connection = open_connection(path)?;
        drop(connection);
        enforce_database_file_permissions(path)
    }

    /// Migrates and validates a closed recovery copy in place.
    ///
    /// # Errors
    ///
    /// Returns a typed storage error when the copy cannot be migrated or validated.
    pub fn migrate_and_validate_closed(path: impl AsRef<Path>) -> Result<(), StorageError> {
        let path = path.as_ref();
        enforce_database_file_permissions(path)?;
        let connection = open_connection(path)?;
        drop(connection);
        enforce_database_file_permissions(path)
    }

    /// Creates a route, owned secret, optional script, and first-route state in
    /// one transaction.
    ///
    /// # Errors
    ///
    /// Returns validation, uniqueness, executor, or `SQLite` transaction errors.
    pub async fn create_route(&self, input: CreateRouteInput) -> Result<RouteRecord, StorageError> {
        self.create_route_with_models(input, Vec::new()).await
    }

    /// Creates a route and its ordered custom models in one transaction.
    ///
    /// # Errors
    ///
    /// Returns route/model validation, uniqueness, executor, or database errors.
    pub async fn create_route_with_models(
        &self,
        input: CreateRouteInput,
        models: Vec<CodexModelRecord>,
    ) -> Result<RouteRecord, StorageError> {
        self.create_route_with_models_and_fallback_exclusions(input, models, Vec::new())
            .await
    }

    /// Creates a route, its ordered custom models, and its Fallback exclusions
    /// in one critical transaction.
    ///
    /// # Errors
    ///
    /// Returns route/model validation, uniqueness, executor, or database errors.
    pub async fn create_route_with_models_and_fallback_exclusions(
        &self,
        input: CreateRouteInput,
        models: Vec<CodexModelRecord>,
        fallback_excluded_models: Vec<String>,
    ) -> Result<RouteRecord, StorageError> {
        let name = RouteName::parse(&input.name)?;
        let base_url = BaseUrl::parse(&input.base_url)?;
        let script = validate_balance_query(input.balance_query)?;
        let models = normalize_codex_model_records(models)?;
        let fallback_excluded_models =
            normalize_fallback_excluded_models(fallback_excluded_models)?;
        let route_id = RouteId::new();
        let secret_id = SecretId::new();
        let timestamp = now_millis();
        let key = input.api_key.expose().to_vec();

        self.call_critical(move |connection| {
            let transaction = connection.transaction()?;
            let fallback = read_fallback_config(&transaction)?;
            let extend_participant_boundary =
                u64::from(fallback.record.participant_count) == fallback.route_count;
            let next_participant_count = if extend_participant_boundary {
                Some(
                    fallback
                        .record
                        .participant_count
                        .checked_add(1)
                        .ok_or(StorageError::InvalidFallbackParticipantCount)?,
                )
            } else {
                None
            };
            let _risk_confirmed = confirm_script_risk_if_needed(
                &transaction,
                script.as_ref(),
                input.accept_script_risk,
            )?;
            let sort_order: i64 = transaction.query_row(
                "SELECT COALESCE(MAX(sort_order), -1) + 1 FROM routes",
                [],
                |row| row.get(0),
            )?;
            transaction.execute(
                "INSERT INTO secrets (secret_id, kind, value, created_at_ms, updated_at_ms) VALUES (?1, 'route_api_key', ?2, ?3, ?3)",
                params![secret_id.as_str(), key, timestamp],
            )?;
            transaction.execute(
                "INSERT INTO routes (route_id, display_name, display_name_key, base_url, secret_id, service_tier_policy, sort_order, created_at_ms, updated_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
                params![route_id.as_str(), name.as_str(), name.comparison_key(), base_url.as_str(), secret_id.as_str(), input.service_tier_policy.as_str(), sort_order, timestamp],
            )?;
            write_balance_query(&transaction, &route_id, script.as_ref(), timestamp)?;
            write_codex_models(&transaction, &route_id, &models)?;
            write_fallback_excluded_models(
                &transaction,
                &route_id,
                &fallback_excluded_models,
            )?;
            transaction.execute(
                "UPDATE route_state SET route_id = ?1, selection_generation = selection_generation + 1, updated_at_ms = ?2 WHERE singleton = 1 AND route_id IS NULL",
                params![route_id.as_str(), timestamp],
            )?;
            if let Some(next_participant_count) = next_participant_count {
                transaction.execute(
                    "UPDATE fallback_config SET participant_count = ?1, config_revision = config_revision + 1, updated_at_ms = ?2 WHERE singleton = 1",
                    params![next_participant_count, timestamp],
                )?;
            }
            let revision = mark_critical_change(&transaction)?;
            transaction.commit()?;

            Ok((
                RouteRecord {
                    route_id,
                    name: name.as_str().to_owned(),
                    base_url: base_url.as_str().to_owned(),
                    secret_id,
                    service_tier_policy: input.service_tier_policy,
                    sort_order,
                    created_at_ms: timestamp,
                    updated_at_ms: timestamp,
                },
                Some(revision),
            ))
        })
        .await
    }

    /// Replaces a route, its owned secret, and optional script atomically.
    ///
    /// # Errors
    ///
    /// Returns validation or database errors, including `NotFound` for an
    /// unknown route.
    pub async fn update_route(&self, input: UpdateRouteInput) -> Result<(), StorageError> {
        let route_id = input.route_id.clone();
        let models = self.list_codex_models(route_id).await?;
        self.update_route_with_models(input, models)
            .await
            .map(|_| ())
    }

    /// Replaces a route and its ordered custom models in one transaction,
    /// returning whether route-owned configuration changed.
    ///
    /// # Errors
    ///
    /// Returns route/model validation, not-found, executor, or database errors.
    pub async fn update_route_with_models(
        &self,
        input: UpdateRouteInput,
        models: Vec<CodexModelRecord>,
    ) -> Result<bool, StorageError> {
        let fallback_excluded_models = self
            .list_fallback_excluded_models(input.route_id.clone())
            .await?;
        self.update_route_with_models_and_fallback_exclusions(
            input,
            models,
            fallback_excluded_models,
        )
        .await
    }

    /// Replaces a route, its custom models, and its Fallback exclusions in one
    /// critical transaction, returning whether route-owned configuration changed.
    ///
    /// # Errors
    ///
    /// Returns route/model validation, not-found, executor, or database errors.
    pub async fn update_route_with_models_and_fallback_exclusions(
        &self,
        input: UpdateRouteInput,
        models: Vec<CodexModelRecord>,
        fallback_excluded_models: Vec<String>,
    ) -> Result<bool, StorageError> {
        let name = RouteName::parse(&input.name)?;
        let base_url = BaseUrl::parse(&input.base_url)?;
        let script = validate_balance_query(input.balance_query)?;
        let models = normalize_codex_model_records(models)?;
        let fallback_excluded_models =
            normalize_fallback_excluded_models(fallback_excluded_models)?;
        let key = input.api_key.expose().to_vec();
        let timestamp = now_millis();

        self.call_critical(move |connection| {
            let transaction = connection.transaction()?;
            let risk_confirmed = confirm_script_risk_if_needed(
                &transaction,
                script.as_ref(),
                input.accept_script_risk,
            )?;
            let stored: Option<(String, String, String, String, Vec<u8>)> = transaction
                .query_row(
                    "SELECT r.display_name, r.base_url, r.secret_id, r.service_tier_policy, s.value FROM routes r JOIN secrets s ON s.secret_id = r.secret_id WHERE r.route_id = ?1",
                    [input.route_id.as_str()],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
                )
                .optional()?;
            let (stored_name, stored_base_url, secret_id, stored_policy, stored_key) =
                stored.ok_or(StorageError::NotFound)?;
            let stored_policy = ServiceTierPolicy::parse_persisted(&stored_policy)?;
            let stored_query = read_balance_query(&transaction, &input.route_id)?;
            let stored_models = read_codex_models(&transaction, &input.route_id)?;
            let stored_fallback_excluded_models =
                read_fallback_excluded_models(&transaction, &input.route_id)?;
            if stored_name == name.as_str()
                && stored_base_url == base_url.as_str()
                && stored_policy == input.service_tier_policy
                && stored_key == key
                && stored_query == script
                && stored_models == models
                && stored_fallback_excluded_models == fallback_excluded_models
            {
                let revision = risk_confirmed
                    .then(|| mark_critical_change(&transaction))
                    .transpose()?;
                transaction.commit()?;
                return Ok((false, revision));
            }
            transaction.execute(
                "UPDATE secrets SET value = ?1, updated_at_ms = ?2 WHERE secret_id = ?3",
                params![key, timestamp, secret_id],
            )?;
            transaction.execute(
                "UPDATE routes SET display_name = ?1, display_name_key = ?2, base_url = ?3, service_tier_policy = ?4, updated_at_ms = ?5 WHERE route_id = ?6",
                params![name.as_str(), name.comparison_key(), base_url.as_str(), input.service_tier_policy.as_str(), timestamp, input.route_id.as_str()],
            )?;
            write_balance_query(&transaction, &input.route_id, script.as_ref(), timestamp)?;
            write_codex_models(&transaction, &input.route_id, &models)?;
            write_fallback_excluded_models(
                &transaction,
                &input.route_id,
                &fallback_excluded_models,
            )?;
            if stored_models != models {
                transaction.execute(
                    "DELETE FROM codex_restart_notice WHERE route_id = ?1 AND EXISTS (SELECT 1 FROM route_state WHERE singleton = 1 AND route_id = ?1)",
                    [input.route_id.as_str()],
                )?;
            }
            let revision = mark_critical_change(&transaction)?;
            transaction.commit()?;
            Ok((true, Some(revision)))
        })
        .await
    }

    /// Deletes a route and exactly its owned secret, clearing active state when
    /// necessary.
    ///
    /// # Errors
    ///
    /// Returns `NotFound` for an unknown route or a database transaction error.
    pub async fn delete_route(&self, route_id: RouteId) -> Result<DeleteRouteResult, StorageError> {
        self.call_critical(move |connection| {
            let transaction = connection.transaction()?;
            let fallback = read_fallback_config(&transaction)?;
            let secret_id: Option<String> = transaction
                .query_row(
                    "SELECT secret_id FROM routes WHERE route_id = ?1",
                    [route_id.as_str()],
                    |row| row.get(0),
                )
                .optional()?;
            let secret_id = secret_id.ok_or(StorageError::NotFound)?;
            let deleted_participant: bool = transaction.query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM (
                        SELECT route_id FROM routes
                        ORDER BY sort_order, created_at_ms
                        LIMIT ?1
                    ) AS participants
                    WHERE route_id = ?2
                )",
                params![fallback.record.participant_count, route_id.as_str()],
                |row| row.get(0),
            )?;
            let participant_count = if deleted_participant {
                fallback
                    .record
                    .participant_count
                    .checked_sub(1)
                    .ok_or(StorageError::Initialization)?
            } else {
                fallback.record.participant_count
            };
            let timestamp = now_millis();
            let cleared_active_route = transaction.execute(
                "UPDATE route_state SET route_id = NULL, selection_generation = selection_generation + 1, updated_at_ms = ?1 WHERE singleton = 1 AND route_id = ?2",
                params![timestamp, route_id.as_str()],
            )? == 1;
            transaction.execute("DELETE FROM routes WHERE route_id = ?1", [route_id.as_str()])?;
            transaction.execute("DELETE FROM secrets WHERE secret_id = ?1", [secret_id])?;
            transaction.execute(
                "UPDATE fallback_config
                 SET participant_count = ?1,
                     enabled = CASE WHEN ?1 < 2 THEN 0 ELSE enabled END,
                     config_revision = config_revision + 1,
                     updated_at_ms = ?2
                 WHERE singleton = 1",
                params![participant_count, timestamp],
            )?;
            let revision = mark_critical_change(&transaction)?;
            transaction.commit()?;
            Ok((
                DeleteRouteResult {
                    deleted_route_id: route_id,
                    cleared_active_route,
                },
                Some(revision),
            ))
        })
        .await
    }

    /// Makes an existing route active.
    ///
    /// # Errors
    ///
    /// Returns `NotFound` for an unknown route or a database transaction error.
    pub async fn activate_route(&self, route_id: RouteId) -> Result<(), StorageError> {
        self.call_critical(move |connection| {
            let transaction = connection.transaction()?;
            let exists: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM routes WHERE route_id = ?1)",
                [route_id.as_str()],
                |row| row.get(0),
            )?;
            if !exists {
                return Err(StorageError::NotFound);
            }
            let changed = transaction.execute(
                "UPDATE route_state SET route_id = ?1, selection_generation = selection_generation + 1, updated_at_ms = ?2 WHERE singleton = 1 AND route_id IS NOT ?1",
                params![route_id.as_str(), now_millis()],
            )? == 1;
            if changed {
                transaction.execute("DELETE FROM codex_restart_notice", [])?;
            }
            let revision = changed
                .then(|| mark_critical_change(&transaction))
                .transpose()?;
            transaction.commit()?;
            Ok(((), revision))
        })
        .await
    }

    /// Reads the nullable active route ID.
    ///
    /// # Errors
    ///
    /// Returns an executor or `SQLite` query error.
    pub async fn active_route_id(&self) -> Result<Option<RouteId>, StorageError> {
        self.call(|connection| {
            let value = connection.query_row(
                "SELECT route_id FROM route_state WHERE singleton = 1",
                [],
                |row| row.get::<_, Option<String>>(0),
            )?;
            Ok(value.map(RouteId::from_string))
        })
        .await
    }

    /// Reads the active selection generation and durable fallback policy.
    ///
    /// # Errors
    ///
    /// Returns an executor, validation, or `SQLite` query error.
    pub async fn routing_state(&self) -> Result<RoutingStateRecord, StorageError> {
        self.call(|connection| {
            let (active_route_id, selection_generation): (Option<String>, i64) = connection
                .query_row(
                    "SELECT route_id, selection_generation FROM route_state WHERE singleton = 1",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )?;
            let fallback = read_fallback_config(connection)?.record;
            Ok(RoutingStateRecord {
                active_route_id: active_route_id.map(RouteId::from_string),
                selection_generation: u64::try_from(selection_generation)
                    .map_err(|_| StorageError::Initialization)?,
                fallback,
            })
        })
        .await
    }

    /// Changes durable automatic fallback enablement when at least two participants exist.
    ///
    /// # Errors
    ///
    /// Returns an executor or `SQLite` transaction error.
    pub async fn set_fallback_enabled(
        &self,
        enabled: bool,
    ) -> Result<FallbackConfigRecord, StorageError> {
        self.call_critical(move |connection| {
            let transaction = connection.transaction()?;
            let current = read_fallback_config(&transaction)?.record;
            let effective = enabled && current.participant_count >= 2;
            if current.enabled != effective {
                transaction.execute(
                    "UPDATE fallback_config SET enabled = ?1, config_revision = config_revision + 1, updated_at_ms = ?2 WHERE singleton = 1",
                    params![effective, now_millis()],
                )?;
            }
            let revision = (current.enabled != effective)
                .then(|| mark_critical_change(&transaction))
                .transpose()?;
            let stored = read_fallback_config(&transaction)?.record;
            transaction.commit()?;
            Ok((stored, revision))
        })
        .await
    }

    /// Changes the durable fallback participant boundary without changing route order.
    ///
    /// # Errors
    ///
    /// Returns `InvalidFallbackParticipantCount` when the requested boundary is
    /// outside the current route list, or an executor/transaction error.
    pub async fn set_fallback_participant_count(
        &self,
        participant_count: u32,
    ) -> Result<bool, StorageError> {
        self.call_critical(move |connection| {
            let transaction = connection.transaction()?;
            let current = read_fallback_config(&transaction)?;
            if u64::from(participant_count) > current.route_count {
                return Err(StorageError::InvalidFallbackParticipantCount);
            }
            if participant_count == current.record.participant_count {
                transaction.commit()?;
                return Ok((false, None));
            }
            let timestamp = now_millis();
            transaction.execute(
                "UPDATE fallback_config
                 SET participant_count = ?1,
                     enabled = CASE WHEN ?1 < 2 THEN 0 ELSE enabled END,
                     config_revision = config_revision + 1,
                     updated_at_ms = ?2
                 WHERE singleton = 1",
                params![participant_count, timestamp],
            )?;
            let revision = mark_critical_change(&transaction)?;
            transaction.commit()?;
            Ok((true, Some(revision)))
        })
        .await
    }

    /// Swaps one route with its adjacent neighbor without activating either route.
    ///
    /// # Errors
    ///
    /// Returns `NotFound`, executor, or `SQLite` transaction errors.
    pub async fn move_route(
        &self,
        route_id: RouteId,
        direction: RouteMoveDirection,
    ) -> Result<bool, StorageError> {
        self.call_critical(move |connection| {
            let transaction = connection.transaction()?;
            let _fallback = read_fallback_config(&transaction)?;
            let current_order: Option<i64> = transaction
                .query_row(
                    "SELECT sort_order FROM routes WHERE route_id = ?1",
                    [route_id.as_str()],
                    |row| row.get(0),
                )
                .optional()?;
            let current_order = current_order.ok_or(StorageError::NotFound)?;
            let neighbor: Option<(String, i64)> = match direction {
                RouteMoveDirection::Up => transaction
                    .query_row(
                        "SELECT route_id, sort_order FROM routes WHERE sort_order < ?1 ORDER BY sort_order DESC, created_at_ms DESC LIMIT 1",
                        [current_order],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .optional()?,
                RouteMoveDirection::Down => transaction
                    .query_row(
                        "SELECT route_id, sort_order FROM routes WHERE sort_order > ?1 ORDER BY sort_order, created_at_ms LIMIT 1",
                        [current_order],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .optional()?,
            };
            let Some((neighbor_id, neighbor_order)) = neighbor else {
                transaction.commit()?;
                return Ok((false, None));
            };
            let timestamp = now_millis();
            transaction.execute(
                "UPDATE routes SET sort_order = ?1, updated_at_ms = ?2 WHERE route_id = ?3",
                params![neighbor_order, timestamp, route_id.as_str()],
            )?;
            transaction.execute(
                "UPDATE routes SET sort_order = ?1, updated_at_ms = ?2 WHERE route_id = ?3",
                params![current_order, timestamp, neighbor_id],
            )?;
            transaction.execute(
                "UPDATE fallback_config SET config_revision = config_revision + 1, updated_at_ms = ?1 WHERE singleton = 1",
                [timestamp],
            )?;
            let revision = mark_critical_change(&transaction)?;
            transaction.commit()?;
            Ok((true, Some(revision)))
        })
        .await
    }

    /// Atomically replaces route order and the fallback participant boundary.
    ///
    /// # Errors
    ///
    /// Returns `StaleRoutingConfiguration` when the expected revision is no
    /// longer current, `InvalidRoutePermutation` when the candidate is not an
    /// exact permutation of durable routes, or an executor/transaction error.
    pub async fn reorder_routes_and_fallback(
        &self,
        ordered_route_ids: Vec<RouteId>,
        participant_count: u32,
        expected_config_revision: u64,
    ) -> Result<bool, StorageError> {
        self.call_critical(move |connection| {
            let transaction = connection.transaction()?;
            let fallback = read_fallback_config(&transaction)?;
            if fallback.record.config_revision != expected_config_revision {
                return Err(StorageError::StaleRoutingConfiguration);
            }

            let mut statement = transaction
                .prepare("SELECT route_id FROM routes ORDER BY sort_order, created_at_ms")?;
            let current_route_ids = statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            drop(statement);

            if ordered_route_ids.len() != current_route_ids.len()
                || u64::from(participant_count) > fallback.route_count
            {
                return Err(StorageError::InvalidRoutePermutation);
            }
            let mut submitted_ids =
                std::collections::HashSet::with_capacity(ordered_route_ids.len());
            for route_id in &ordered_route_ids {
                if !submitted_ids.insert(route_id.as_str()) {
                    return Err(StorageError::InvalidRoutePermutation);
                }
            }
            if !current_route_ids
                .iter()
                .all(|route_id| submitted_ids.contains(route_id.as_str()))
            {
                return Err(StorageError::InvalidRoutePermutation);
            }

            let order_changed = ordered_route_ids
                .iter()
                .map(RouteId::as_str)
                .ne(current_route_ids.iter().map(String::as_str));
            let boundary_changed = participant_count != fallback.record.participant_count;
            if !order_changed && !boundary_changed {
                transaction.commit()?;
                return Ok((false, None));
            }

            let timestamp = now_millis();
            for (sort_order, route_id) in ordered_route_ids.iter().enumerate() {
                let sort_order =
                    i64::try_from(sort_order).map_err(|_| StorageError::InvalidRoutePermutation)?;
                if transaction.execute(
                    "UPDATE routes SET sort_order = ?1, updated_at_ms = ?2 WHERE route_id = ?3",
                    params![sort_order, timestamp, route_id.as_str()],
                )? != 1
                {
                    return Err(StorageError::InvalidRoutePermutation);
                }
            }
            transaction.execute(
                "UPDATE fallback_config
                 SET participant_count = ?1,
                     enabled = CASE WHEN ?1 < 2 THEN 0 ELSE enabled END,
                     config_revision = config_revision + 1,
                     updated_at_ms = ?2
                 WHERE singleton = 1",
                params![participant_count, timestamp],
            )?;
            let revision = mark_critical_change(&transaction)?;
            transaction.commit()?;
            Ok((true, Some(revision)))
        })
        .await
    }

    /// Activates the immediate next fallback participant only when the captured
    /// routing generations still match durable state.
    ///
    /// # Errors
    ///
    /// Returns an executor or `SQLite` transaction error. Stale or invalid
    /// activation attempts return `false` without mutating state.
    pub async fn conditional_activate_next(
        &self,
        expected_route_id: RouteId,
        expected_selection_generation: u64,
        expected_config_revision: u64,
        target_route_id: RouteId,
    ) -> Result<bool, StorageError> {
        self.conditional_activate_forward(
            expected_route_id,
            expected_selection_generation,
            expected_config_revision,
            target_route_id,
            Vec::new(),
            Vec::new(),
            String::new(),
            false,
        )
        .await
    }

    /// Activates a later participant after revalidating the exact skipped span.
    /// Model-exclusion skips are checked against the latest persisted policy;
    /// runtime-health skips are supplied by the versioned health registry.
    #[allow(clippy::too_many_arguments)]
    ///
    /// # Errors
    ///
    /// Returns a database or transaction error when the activation cannot be evaluated.
    pub async fn conditional_activate_forward(
        &self,
        expected_route_id: RouteId,
        expected_selection_generation: u64,
        expected_config_revision: u64,
        target_route_id: RouteId,
        skipped_route_ids: Vec<RouteId>,
        model_excluded_route_ids: Vec<RouteId>,
        requested_model: String,
        allow_earlier_recovery: bool,
    ) -> Result<bool, StorageError> {
        self.call_critical(move |connection| {
            let transaction = connection.transaction()?;
            let (active_route_id, selection_generation): (Option<String>, i64) = transaction
                .query_row(
                    "SELECT route_id, selection_generation FROM route_state WHERE singleton = 1",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )?;
            let fallback = read_fallback_config(&transaction)?.record;
            let selection_matches = u64::try_from(selection_generation).ok()
                == Some(expected_selection_generation);
            let revision_matches = fallback.config_revision == expected_config_revision;
            if !fallback.enabled
                || active_route_id.as_deref() != Some(expected_route_id.as_str())
                || !selection_matches
                || !revision_matches
            {
                transaction.commit()?;
                return Ok((false, None));
            }
            let mut statement = transaction.prepare(
                "SELECT route_id FROM routes ORDER BY sort_order, created_at_ms LIMIT ?1",
            )?;
            let participants = statement
                .query_map([fallback.participant_count], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            drop(statement);
            let Some(source_index) = participants
                .iter()
                .position(|route_id| route_id == expected_route_id.as_str()) else {
                transaction.commit()?;
                return Ok((false, None));
            };
            let Some(target_index) = participants
                .iter()
                .position(|route_id| route_id == target_route_id.as_str()) else {
                transaction.commit()?;
                return Ok((false, None));
            };
            let valid_span = if allow_earlier_recovery {
                target_index < source_index && skipped_route_ids.is_empty()
            } else {
                let expected_skipped = participants
                    .get(source_index + 1..target_index)
                    .unwrap_or_default();
                target_index > source_index
                    && expected_skipped.len() == skipped_route_ids.len()
                    && expected_skipped
                        .iter()
                        .zip(&skipped_route_ids)
                        .all(|(expected, actual)| expected == actual.as_str())
            };
            if !valid_span {
                transaction.commit()?;
                return Ok((false, None));
            }
            for route_id in &model_excluded_route_ids {
                if !skipped_route_ids.contains(route_id) {
                    transaction.commit()?;
                    return Ok((false, None));
                }
                let matches = transaction.query_row(
                    "SELECT EXISTS(SELECT 1 FROM route_fallback_excluded_models WHERE route_id = ?1 AND model_id = ?2)",
                    params![route_id.as_str(), requested_model],
                    |row| row.get::<_, bool>(0),
                )?;
                if !matches {
                    transaction.commit()?;
                    return Ok((false, None));
                }
            }
            let changed = transaction.execute(
                "UPDATE route_state SET route_id = ?1, selection_generation = selection_generation + 1, updated_at_ms = ?2 WHERE singleton = 1 AND route_id = ?3 AND selection_generation = ?4",
                params![
                    target_route_id.as_str(),
                    now_millis(),
                    expected_route_id.as_str(),
                    i64::try_from(expected_selection_generation)
                        .map_err(|_| StorageError::Initialization)?,
                ],
            )? == 1;
            if changed {
                transaction.execute("DELETE FROM codex_restart_notice", [])?;
            }
            let revision = changed
                .then(|| mark_critical_change(&transaction))
                .transpose()?;
            transaction.commit()?;
            Ok((changed, revision))
        })
        .await
    }

    /// Lists routes in stable creation/form order.
    ///
    /// # Errors
    ///
    /// Returns an executor or `SQLite` query error.
    pub async fn list_routes(&self) -> Result<Vec<RouteRecord>, StorageError> {
        self.call(|connection| {
            let mut statement = connection.prepare(
                "SELECT route_id, display_name, base_url, secret_id, service_tier_policy, sort_order, created_at_ms, updated_at_ms FROM routes ORDER BY sort_order, created_at_ms",
            )?;
            let stored = statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, i64>(5)?,
                        row.get::<_, i64>(6)?,
                        row.get::<_, i64>(7)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            let routes = stored
                .into_iter()
                .map(
                    |(
                        route_id,
                        name,
                        base_url,
                        secret_id,
                        service_tier_policy,
                        sort_order,
                        created_at_ms,
                        updated_at_ms,
                    )| {
                        Ok(RouteRecord {
                            route_id: RouteId::from_string(route_id),
                            name,
                            base_url,
                            secret_id: SecretId::from_string(secret_id),
                            service_tier_policy: ServiceTierPolicy::parse_persisted(
                                &service_tier_policy,
                            )?,
                            sort_order,
                            created_at_ms,
                            updated_at_ms,
                        })
                    },
                )
                .collect::<Result<Vec<_>, StorageError>>()?;
            Ok(routes)
        })
        .await
    }

    /// Loads one authorized route-edit record, including its complete Key and
    /// optional script. This is not used by list or menu projections.
    ///
    /// # Errors
    ///
    /// Returns `NotFound`, executor, or `SQLite` errors.
    pub async fn route_edit(&self, route_id: RouteId) -> Result<RouteEditRecord, StorageError> {
        self.call(move |connection| {
            let stored = connection
                .query_row(
                    "SELECT r.display_name, r.base_url, r.secret_id, r.service_tier_policy, r.sort_order, r.created_at_ms, r.updated_at_ms, s.value, b.mode, b.enabled, b.custom_source FROM routes r JOIN secrets s ON s.secret_id = r.secret_id LEFT JOIN balance_queries b ON b.route_id = r.route_id WHERE r.route_id = ?1",
                    [route_id.as_str()],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, i64>(4)?,
                            row.get::<_, i64>(5)?,
                            row.get::<_, i64>(6)?,
                            row.get::<_, Vec<u8>>(7)?,
                            row.get::<_, Option<String>>(8)?,
                            row.get::<_, Option<bool>>(9)?,
                            row.get::<_, Option<String>>(10)?,
                        ))
                    },
                )
                .optional()?
                .ok_or(StorageError::NotFound)?;
            let (
                name,
                base_url,
                secret_id,
                service_tier_policy,
                sort_order,
                created_at_ms,
                updated_at_ms,
                api_key,
                mode,
                enabled,
                custom_source,
            ) = stored;
            let balance_query = mode
                .map(|mode| {
                    Ok::<BalanceQueryInput, StorageError>(BalanceQueryInput {
                        mode: BalanceQueryMode::parse_persisted(&mode)
                            .ok_or(StorageError::Initialization)?,
                        enabled: enabled.unwrap_or(false),
                        custom_source: custom_source.unwrap_or_default(),
                    })
                })
                .transpose()?;
            let models = read_codex_models(connection, &route_id)?;
            let fallback_excluded_models =
                read_fallback_excluded_models(connection, &route_id)?;
            Ok(RouteEditRecord {
                route: RouteRecord {
                    route_id,
                    name,
                    base_url,
                    secret_id: SecretId::from_string(secret_id),
                    service_tier_policy: ServiceTierPolicy::parse_persisted(
                        &service_tier_policy,
                    )?,
                    sort_order,
                    created_at_ms,
                    updated_at_ms,
                },
                api_key: ApiKey::from_stored(api_key),
                balance_query,
                fallback_excluded_models,
                models,
            })
        })
        .await
    }

    /// Reads the singleton application settings row.
    ///
    /// # Errors
    ///
    /// Returns an executor, validation, or `SQLite` query error.
    #[expect(
        clippy::too_many_lines,
        reason = "the singleton loader validates every persisted settings domain together"
    )]
    pub async fn app_settings(&self) -> Result<AppSettingsRecord, StorageError> {
        self.call(|connection| {
            let (
                proxy_port,
                first_run_presented,
                balance_script_risk_confirmed,
                menu_debounce_seconds,
                automatic_refresh_minutes,
                images_generation_enabled,
                images_generation_route_id,
                images_generation_timeout_secs,
                appearance_preference,
                last_automatic_update_check_at_ms,
                menu_bar_status_text_enabled,
                menu_bar_activity_animation_enabled,
                mcp_image_capacity_warning_mib,
                mcp_image_capacity_active_episode,
                mcp_image_capacity_dismissed_episode,
            ): AppSettingsRow = connection.query_row(
                "SELECT proxy_port, first_run_presented, balance_script_risk_confirmed, menu_balance_debounce_seconds, automatic_balance_refresh_minutes, images_generation_enabled, images_generation_route_id, images_generation_timeout_secs, appearance_preference, last_automatic_update_check_at_ms, menu_bar_status_text_enabled, menu_bar_activity_animation_enabled, mcp_image_capacity_warning_mib, mcp_image_capacity_active_episode, mcp_image_capacity_dismissed_episode FROM app_settings WHERE singleton = 1",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                        row.get(8)?,
                        row.get(9)?,
                        row.get(10)?,
                        row.get(11)?,
                        row.get(12)?,
                        row.get(13)?,
                        row.get(14)?,
                    ))
                },
            )?;
            let proxy_port = u16::try_from(proxy_port)
                .ok()
                .filter(|port| *port != 0)
                .ok_or(StorageError::Initialization)?;
            let menu_debounce_seconds = u16::try_from(menu_debounce_seconds)
                .map_err(|_| StorageError::Initialization)?;
            let automatic_refresh_minutes = u16::try_from(automatic_refresh_minutes)
                .map_err(|_| StorageError::Initialization)?;
            let balance_query_policy =
                BalanceQueryPolicy::parse(menu_debounce_seconds, automatic_refresh_minutes)?;
            let images_generation_enabled = parse_persisted_bool(images_generation_enabled)?;
            let images_generation_timeout = ImagesGenerationTimeout::parse(
                u16::try_from(images_generation_timeout_secs)
                    .map_err(|_| StorageError::Initialization)?,
            )?;
            if last_automatic_update_check_at_ms.is_some_and(|timestamp| timestamp < 0) {
                return Err(StorageError::Initialization);
            }
            let threshold = McpImageCapacityWarningThreshold::parse(
                u32::try_from(mcp_image_capacity_warning_mib)
                    .map_err(|_| StorageError::Initialization)?,
            )?;
            validate_capacity_episodes(
                mcp_image_capacity_active_episode.as_deref(),
                mcp_image_capacity_dismissed_episode.as_deref(),
            )?;
            if let Some(route_id) = images_generation_route_id.as_deref() {
                let valid: bool = connection.query_row(
                    "SELECT EXISTS(SELECT 1 FROM routes WHERE route_id = ?1)",
                    [route_id],
                    |row| row.get(0),
                )?;
                if !valid {
                    return Err(StorageError::Initialization);
                }
            }
            Ok(AppSettingsRecord {
                proxy_port,
                first_run_presented,
                balance_script_risk_confirmed,
                balance_query_policy,
                images_generation_enabled,
                images_generation_route_id: images_generation_route_id.map(RouteId::from_string),
                images_generation_timeout,
                appearance_preference: AppearancePreference::parse_persisted(&appearance_preference)?,
                last_automatic_update_check_at_ms,
                menu_bar: MenuBarSettingsRecord {
                    status_text_enabled: parse_persisted_bool(menu_bar_status_text_enabled)?,
                    activity_animation_enabled: parse_persisted_bool(
                        menu_bar_activity_animation_enabled,
                    )?,
                },
                mcp_image_capacity: McpImageCapacitySettingsRecord {
                    threshold,
                    active_episode_id: mcp_image_capacity_active_episode,
                    dismissed_episode_id: mcp_image_capacity_dismissed_episode,
                },
            })
        })
        .await
    }

    /// Atomically persists the two non-critical menu bar presentation preferences.
    ///
    /// # Errors
    ///
    /// Returns an executor, transaction, or persisted-value validation error.
    pub async fn set_menu_bar_settings(
        &self,
        status_text_enabled: bool,
        activity_animation_enabled: bool,
    ) -> Result<bool, StorageError> {
        self.call(move |connection| {
            let transaction = connection.transaction()?;
            let current: (i64, i64) = transaction.query_row(
                "SELECT menu_bar_status_text_enabled, menu_bar_activity_animation_enabled FROM app_settings WHERE singleton = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            let current = (parse_persisted_bool(current.0)?, parse_persisted_bool(current.1)?);
            let next = (status_text_enabled, activity_animation_enabled);
            if current == next {
                transaction.commit()?;
                return Ok(false);
            }
            transaction.execute(
                "UPDATE app_settings SET menu_bar_status_text_enabled = ?1, menu_bar_activity_animation_enabled = ?2 WHERE singleton = 1",
                params![status_text_enabled, activity_animation_enabled],
            )?;
            transaction.commit()?;
            Ok(true)
        })
        .await
    }

    /// Persists a validated threshold and reconciles its reminder episode
    /// against the current aggregate bytes in one non-critical transaction.
    ///
    /// # Errors
    ///
    /// Returns an executor, transaction, or persisted-value validation error.
    pub async fn set_mcp_image_capacity_threshold(
        &self,
        threshold: McpImageCapacityWarningThreshold,
        observed_bytes: Option<u64>,
    ) -> Result<McpImageCapacitySettingsRecord, StorageError> {
        self.call(move |connection| {
            let transaction = connection.transaction()?;
            let mut next = read_mcp_image_capacity_settings(&transaction)?;
            next.threshold = threshold;
            if let Some(observed_bytes) = observed_bytes {
                next = reconcile_mcp_image_capacity_settings(next, threshold, observed_bytes);
            }
            write_mcp_image_capacity_settings_if_changed(&transaction, &next)?;
            transaction.commit()?;
            Ok(next)
        })
        .await
    }

    /// Reconciles the durable reminder episode with current aggregate bytes.
    ///
    /// # Errors
    ///
    /// Returns an executor, transaction, or persisted-value validation error.
    pub async fn reconcile_mcp_image_capacity(
        &self,
        observed_bytes: u64,
    ) -> Result<McpImageCapacitySettingsRecord, StorageError> {
        self.call(move |connection| {
            let transaction = connection.transaction()?;
            let current = read_mcp_image_capacity_settings(&transaction)?;
            let threshold = current.threshold;
            let next = reconcile_mcp_image_capacity_settings(current, threshold, observed_bytes);
            write_mcp_image_capacity_settings_if_changed(&transaction, &next)?;
            transaction.commit()?;
            Ok(next)
        })
        .await
    }

    /// Dismisses only the exact active warning episode.
    ///
    /// A stale or malformed ID is a no-op so delayed menu actions cannot hide
    /// a later episode.
    ///
    /// # Errors
    ///
    /// Returns an executor, transaction, or persisted-value validation error.
    pub async fn dismiss_mcp_image_capacity_warning(
        &self,
        episode_id: &str,
    ) -> Result<bool, StorageError> {
        let episode_id = episode_id.to_owned();
        self.call(move |connection| {
            let transaction = connection.transaction()?;
            let current = read_mcp_image_capacity_settings(&transaction)?;
            if current.active_episode_id.as_deref() != Some(episode_id.as_str())
                || current.dismissed_episode_id.as_deref() == Some(episode_id.as_str())
            {
                transaction.commit()?;
                return Ok(false);
            }
            transaction.execute(
                "UPDATE app_settings SET mcp_image_capacity_dismissed_episode = ?1 WHERE singleton = 1",
                [episode_id],
            )?;
            transaction.commit()?;
            Ok(true)
        })
        .await
    }

    /// Persists the time at which an automatic update check was attempted.
    ///
    /// This operational cadence value is intentionally non-critical and does
    /// not advance the recovery revision.
    ///
    /// # Errors
    ///
    /// Returns a validation error for a negative timestamp or a database error
    /// when the singleton row cannot be updated.
    pub async fn set_last_automatic_update_check_at_ms(
        &self,
        timestamp_ms: i64,
    ) -> Result<(), StorageError> {
        if timestamp_ms < 0 {
            return Err(StorageError::Initialization);
        }
        self.call(move |connection| {
            connection.execute(
                "UPDATE app_settings SET last_automatic_update_check_at_ms = ?1 WHERE singleton = 1",
                [timestamp_ms],
            )?;
            Ok(())
        })
        .await
    }

    /// Persists a closed appearance preference and reports whether it changed.
    ///
    /// # Errors
    ///
    /// Returns an executor, transaction, or persisted-value validation error.
    pub async fn set_appearance_preference(
        &self,
        preference: AppearancePreference,
    ) -> Result<bool, StorageError> {
        self.call(move |connection| {
            let transaction = connection.transaction()?;
            let current: String = transaction.query_row(
                "SELECT appearance_preference FROM app_settings WHERE singleton = 1",
                [],
                |row| row.get(0),
            )?;
            let current = AppearancePreference::parse_persisted(&current)?;
            if current == preference {
                transaction.commit()?;
                return Ok(false);
            }
            transaction.execute(
                "UPDATE app_settings SET appearance_preference = ?1 WHERE singleton = 1",
                [preference.as_str()],
            )?;
            transaction.commit()?;
            Ok(true)
        })
        .await
    }

    /// Atomically updates global image generation admission and its dedicated route.
    ///
    /// # Errors
    ///
    /// Returns `InvalidImagesGenerationRoute` when enablement has no selected
    /// route or the selected route is absent.
    pub async fn set_images_generation_settings(
        &self,
        enabled: bool,
        route_id: Option<RouteId>,
        timeout: ImagesGenerationTimeout,
    ) -> Result<bool, StorageError> {
        self.call_critical(move |connection| {
            let transaction = connection.transaction()?;
            if enabled && route_id.is_none() {
                return Err(StorageError::InvalidImagesGenerationRoute);
            }
            if let Some(route_id) = route_id.as_ref() {
                let valid: bool = transaction.query_row(
                    "SELECT EXISTS(SELECT 1 FROM routes WHERE route_id = ?1)",
                    [route_id.as_str()],
                    |row| row.get(0),
                )?;
                if !valid {
                    return Err(StorageError::InvalidImagesGenerationRoute);
                }
            }
            let current: (i64, Option<String>, i64) = transaction.query_row(
                "SELECT images_generation_enabled, images_generation_route_id, images_generation_timeout_secs FROM app_settings WHERE singleton = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?;
            let current_enabled = parse_persisted_bool(current.0)?;
            if current_enabled == enabled
                && current.1.as_deref() == route_id.as_ref().map(RouteId::as_str)
                && current.2 == i64::from(timeout.seconds())
            {
                transaction.commit()?;
                return Ok((false, None));
            }
            transaction.execute(
                "UPDATE app_settings SET images_generation_enabled = ?1, images_generation_route_id = ?2, images_generation_timeout_secs = ?3 WHERE singleton = 1",
                params![enabled, route_id.as_ref().map(RouteId::as_str), timeout.seconds()],
            )?;
            let revision = mark_critical_change(&transaction)?;
            transaction.commit()?;
            Ok((true, Some(revision)))
        })
        .await
    }

    /// Atomically persists both validated global balance-query timings.
    ///
    /// # Errors
    ///
    /// Returns executor or `SQLite` transaction errors. An unchanged policy
    /// returns `false` without issuing an update.
    pub async fn set_balance_query_policy(
        &self,
        policy: BalanceQueryPolicy,
    ) -> Result<bool, StorageError> {
        self.call_critical(move |connection| {
            let transaction = connection.transaction()?;
            let current: (i64, i64) = transaction.query_row(
                "SELECT menu_balance_debounce_seconds, automatic_balance_refresh_minutes FROM app_settings WHERE singleton = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            let requested = (
                i64::from(policy.menu_debounce_seconds()),
                i64::from(policy.automatic_refresh_minutes()),
            );
            if current == requested {
                transaction.commit()?;
                return Ok((false, None));
            }
            transaction.execute(
                "UPDATE app_settings SET menu_balance_debounce_seconds = ?1, automatic_balance_refresh_minutes = ?2 WHERE singleton = 1",
                params![requested.0, requested.1],
            )?;
            let revision = mark_critical_change(&transaction)?;
            transaction.commit()?;
            Ok((true, Some(revision)))
        })
        .await
    }

    /// Persists a validated proxy port without changing Codex configuration.
    ///
    /// # Errors
    ///
    /// Returns validation, executor, or `SQLite` errors.
    pub async fn set_proxy_port(&self, proxy_port: u16) -> Result<(), StorageError> {
        if proxy_port == 0 {
            return Err(StorageError::Initialization);
        }
        self.call_critical(move |connection| {
            let transaction = connection.transaction()?;
            let changed = transaction.execute(
                "UPDATE app_settings SET proxy_port = ?1 WHERE singleton = 1 AND proxy_port != ?1",
                [i64::from(proxy_port)],
            )? == 1;
            let revision = changed
                .then(|| mark_critical_change(&transaction))
                .transpose()?;
            transaction.commit()?;
            Ok(((), revision))
        })
        .await
    }

    /// Marks the first-run menu presentation after the shell shows it.
    ///
    /// # Errors
    ///
    /// Returns executor or `SQLite` errors.
    pub async fn mark_first_run_presented(&self) -> Result<(), StorageError> {
        self.call_critical(|connection| {
            let transaction = connection.transaction()?;
            let changed = transaction.execute(
                "UPDATE app_settings SET first_run_presented = 1 WHERE singleton = 1 AND first_run_presented = 0",
                [],
            )? == 1;
            let revision = changed
                .then(|| mark_critical_change(&transaction))
                .transpose()?;
            transaction.commit()?;
            Ok(((), revision))
        })
        .await
    }

    async fn enabled_balance_route(
        &self,
        route_id: RouteId,
    ) -> Result<Option<BalanceRouteConfig>, StorageError> {
        self.call(move |connection| {
            let stored = connection
                .query_row(
                    "SELECT r.base_url, s.value, b.mode, b.custom_source FROM routes r JOIN secrets s ON s.secret_id = r.secret_id JOIN balance_queries b ON b.route_id = r.route_id WHERE r.route_id = ?1 AND b.enabled = 1",
                    [route_id.as_str()],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, Vec<u8>>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                        ))
                    },
                )
                .optional()?;
            let Some((base_url, api_key, mode, custom_source)) = stored else {
                return Ok(None);
            };
            let mode = BalanceQueryMode::parse_persisted(&mode)
                .ok_or(StorageError::Initialization)?;
            let query_revision = balance_revision(&base_url, &api_key, mode, &custom_source);
            Ok(Some(BalanceRouteConfig {
                route_id,
                base_url: BaseUrl::parse(&base_url)?,
                api_key: ApiKey::from_stored(api_key),
                query: BalanceQueryConfig {
                    mode,
                    custom_source,
                },
                query_revision,
            }))
        })
        .await
    }

    async fn enabled_balance_route_ids(&self) -> Result<Vec<RouteId>, StorageError> {
        self.call(|connection| {
            let mut statement = connection.prepare(
                "SELECT r.route_id FROM routes r JOIN balance_queries b ON b.route_id = r.route_id WHERE b.enabled = 1 ORDER BY r.sort_order, r.created_at_ms",
            )?;
            Ok(statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .map(RouteId::from_string)
                .collect())
        })
        .await
    }

    /// Deletes requests older than the UTC cutoff and reclaims free pages.
    ///
    /// # Errors
    ///
    /// Returns an executor, deletion, or incremental-vacuum error.
    pub async fn cleanup_history(&self, cutoff_ms: i64) -> Result<u64, StorageError> {
        self.call(move |connection| {
            let transaction = connection.transaction()?;
            let deleted = transaction.execute(
                "DELETE FROM proxy_requests WHERE started_at_ms < ?1",
                [cutoff_ms],
            )?;
            transaction.commit()?;
            connection.execute_batch("PRAGMA incremental_vacuum")?;
            Ok(deleted as u64)
        })
        .await
    }

    /// Deletes all request metadata and reclaims free pages.
    ///
    /// # Errors
    ///
    /// Returns an executor, deletion, or incremental-vacuum error.
    pub async fn clear_history(&self) -> Result<ClearHistoryResult, StorageError> {
        self.call(|connection| {
            let transaction = connection.transaction()?;
            let deleted = transaction.execute("DELETE FROM proxy_requests", [])?;
            transaction.commit()?;
            let reclaim_succeeded = connection
                .execute_batch("PRAGMA incremental_vacuum")
                .is_ok();
            Ok(ClearHistoryResult {
                deleted_requests: deleted as u64,
                reclaim_succeeded,
            })
        })
        .await
    }

    /// Returns the bounded aggregate used by the settings screen.
    ///
    /// # Errors
    ///
    /// Returns an executor or `SQLite` aggregate query error.
    pub async fn history_summary(&self) -> Result<HistorySummary, StorageError> {
        let database_bytes = fs::metadata(self.path.as_ref()).map_or(0, |metadata| metadata.len());
        self.call(move |connection| {
            let (count, earliest, latest) = connection.query_row(
                "SELECT COUNT(*), MIN(started_at_ms), MAX(started_at_ms) FROM proxy_requests",
                [],
                |row| Ok((row.get::<_, i64>(0)?, row.get(1)?, row.get(2)?)),
            )?;
            Ok(HistorySummary {
                request_count: u64::try_from(count).unwrap_or_default(),
                earliest_started_at_ms: earliest,
                latest_started_at_ms: latest,
                database_bytes,
                retention_days: 365,
            })
        })
        .await
    }

    /// Returns one validated, newest-first keyset page of retained usage.
    ///
    /// # Errors
    ///
    /// Returns an invalid-query, executor, or database error.
    #[expect(
        clippy::too_many_lines,
        reason = "validation, count, and keyset selection intentionally share one query contract"
    )]
    pub async fn usage_history(
        &self,
        query: UsageHistoryQuery,
    ) -> Result<UsageHistoryPage, StorageError> {
        if query.limit == 0
            || query.limit > 100
            || query.finished_at_or_before_ms < 0
            || query
                .finished_at_or_after_ms
                .is_some_and(|lower| lower < 0 || lower > query.finished_at_or_before_ms)
        {
            return Err(StorageError::InvalidUsageQuery);
        }
        if query
            .model_contains
            .as_ref()
            .is_some_and(|model| model.is_empty() || model.len() > 256)
        {
            return Err(StorageError::InvalidUsageQuery);
        }
        if query.cursor.as_ref().is_some_and(|cursor| {
            cursor.finished_at_ms < 0
                || cursor.request_id.is_empty()
                || cursor.request_id.len() > 128
        }) {
            return Err(StorageError::InvalidUsageQuery);
        }
        self.call(move |connection| {
            let completion = query.completion_state.as_ref().map(completion_state_value);
            let route = query.route_id.as_ref().map(RouteId::as_str);
            let model_pattern = query
                .model_contains
                .as_deref()
                .map(literal_contains_pattern);
            let cursor_time = query.cursor.as_ref().map(|cursor| cursor.finished_at_ms);
            let cursor_id = query.cursor.as_ref().map(|cursor| cursor.request_id.as_str());
            let total_rows: i64 = connection.query_row(
                "SELECT COUNT(*) FROM proxy_requests
                 WHERE finished_at_ms IS NOT NULL
                   AND (?1 IS NULL OR finished_at_ms >= ?1)
                   AND finished_at_ms <= ?2
                   AND (?3 IS NULL OR completion_state = ?3)
                   AND (?4 IS NULL OR final_route_id = ?4)
                   AND (?5 IS NULL OR COALESCE(actual_model, requested_model) LIKE ?5 ESCAPE '\\' COLLATE NOCASE)",
                params![
                    query.finished_at_or_after_ms,
                    query.finished_at_or_before_ms,
                    completion,
                    route,
                    model_pattern.as_deref(),
                ],
                |row| row.get(0),
            )?;
            let mut statement = connection.prepare(
                "SELECT request_id, started_at_ms, finished_at_ms, final_route_id, final_route_name,
                        requested_model, actual_model, reasoning_effort, streaming,
                        completion_state, http_status,
                        input_tokens, output_tokens, total_tokens, cached_input_tokens,
                        cache_write_input_tokens, total_latency_ms, first_output_latency_ms,
                        pricing_catalog_version, cost_status, upstream_cost_pico_usd,
                        actual_service_tier
                 FROM proxy_requests
                 WHERE finished_at_ms IS NOT NULL
                   AND (?1 IS NULL OR finished_at_ms >= ?1)
                   AND finished_at_ms <= ?2
                   AND (?3 IS NULL OR completion_state = ?3)
                   AND (?4 IS NULL OR final_route_id = ?4)
                   AND (?5 IS NULL OR COALESCE(actual_model, requested_model) LIKE ?5 ESCAPE '\\' COLLATE NOCASE)
                   AND (?6 IS NULL OR finished_at_ms < ?6 OR (finished_at_ms = ?6 AND request_id < ?7))
                 ORDER BY finished_at_ms DESC, request_id DESC LIMIT ?8",
            )?;
            let fetch_limit = i64::from(query.limit) + 1;
            let rows = statement.query_map(
                params![
                    query.finished_at_or_after_ms,
                    query.finished_at_or_before_ms,
                    completion,
                    route,
                    model_pattern,
                    cursor_time,
                    cursor_id,
                    fetch_limit,
                ],
                |row| usage_history_row(row, row.get(21)?),
            )?;
            let mut rows = rows.collect::<Result<Vec<_>, _>>()?;
            let has_more = rows.len() > usize::from(query.limit);
            rows.truncate(usize::from(query.limit));
            let next_cursor = if has_more {
                rows.last().and_then(|last| {
                    last.finished_at_ms.map(|finished_at_ms| UsageHistoryCursor {
                        finished_at_ms,
                        request_id: last.request_id.clone(),
                    })
                })
            } else {
                None
            };
            Ok(UsageHistoryPage {
                rows,
                next_cursor,
                total_rows: u64::try_from(total_rows).unwrap_or_default(),
            })
        })
        .await
    }

    /// Returns successful-request aggregates for one anchored Usage filter snapshot.
    ///
    /// # Errors
    ///
    /// Returns an invalid-query, checked-overflow, executor, or database error.
    #[expect(
        clippy::too_many_lines,
        reason = "the bounded query and its single-pass aggregate stay in one executor closure"
    )]
    pub async fn usage_statistics(
        &self,
        query: UsageStatisticsQuery,
    ) -> Result<UsageStatistics, StorageError> {
        validate_usage_statistics_query(&query)?;
        let time_zone = query
            .time_zone
            .parse::<Tz>()
            .map_err(|_| StorageError::InvalidUsageQuery)?;
        let granularity = statistics_granularity(&query);
        self.call(move |connection| {
            let route = query.route_id.as_ref().map(RouteId::as_str);
            let model_pattern = query
                .model_contains
                .as_deref()
                .map(literal_contains_pattern);
            let mut statement = connection.prepare(
                "SELECT finished_at_ms, request_id, final_route_id, final_route_name,
                        requested_model, actual_model, input_tokens, cached_input_tokens,
                        cache_write_input_tokens, output_tokens, total_tokens,
                        upstream_cost_pico_usd, MIN(finished_at_ms) OVER ()
                 FROM proxy_requests
                 WHERE finished_at_ms IS NOT NULL
                   AND completion_state = 'completed'
                   AND (?1 IS NULL OR finished_at_ms >= ?1)
                   AND finished_at_ms <= ?2
                   AND (?3 IS NULL OR final_route_id = ?3)
                   AND (?4 IS NULL OR COALESCE(actual_model, requested_model) LIKE ?4 ESCAPE '\\' COLLATE NOCASE)
                 ORDER BY finished_at_ms DESC, request_id DESC",
            )?;
            let mut rows = statement.query(params![
                query.finished_at_or_after_ms,
                query.finished_at_or_before_ms,
                route,
                model_pattern,
            ])?;
            let mut totals = StatisticsTotals::default();
            let mut bucket_windows = Vec::new();
            let mut bucket_totals = Vec::new();
            let mut attribution = BTreeMap::<String, AttributionAggregate>::new();
            while let Some(row) = rows.next()? {
                let finished_at_ms = row.get::<_, i64>(0)?;
                if bucket_windows.is_empty() {
                    let earliest = row.get::<_, i64>(12)?;
                    let lower = query.finished_at_or_after_ms.unwrap_or(earliest);
                    bucket_windows = statistics_bucket_windows(
                        lower,
                        query.finished_at_or_before_ms,
                        time_zone,
                        granularity,
                    )?;
                    bucket_totals.resize(bucket_windows.len(), StatisticsTotals::default());
                }
                let observation = StatisticsObservation {
                    input_tokens: row.get(6)?,
                    cached_input_tokens: row.get(7)?,
                    cache_write_input_tokens: row.get(8)?,
                    output_tokens: row.get(9)?,
                    total_tokens: row.get(10)?,
                    cost_pico_usd: row.get(11)?,
                };
                totals.add(&observation)?;
                if let Some(index) = bucket_windows.iter().position(|window| {
                    finished_at_ms >= window.started_at_ms
                        && (finished_at_ms < window.finished_at_ms
                            || (window.finished_at_ms == query.finished_at_or_before_ms
                                && finished_at_ms == window.finished_at_ms))
                }) {
                    bucket_totals[index].add(&observation)?;
                }
                let identity = attribution_identity(
                    query.attribution_dimension,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                );
                attribution
                    .entry(identity.key)
                    .and_modify(|aggregate| {
                        if aggregate.label.starts_with("未知") && !identity.label.starts_with("未知")
                        {
                            aggregate.label.clone_from(&identity.label);
                        }
                    })
                    .or_insert_with(|| AttributionAggregate {
                        label: identity.label,
                        totals: StatisticsTotals::default(),
                    })
                    .totals
                    .add(&observation)?;
            }
            let trend = bucket_windows
                .into_iter()
                .zip(bucket_totals)
                .map(|(window, totals)| UsageStatisticsBucket {
                    started_at_ms: window.started_at_ms,
                    finished_at_ms: window.finished_at_ms,
                    label: window.label,
                    request_count: totals.request_count,
                    tokens: totals.tokens,
                    cost_pico_usd: totals.cost_pico_usd,
                })
                .collect();
            let attribution = statistics_attribution(
                attribution,
                query.attribution_metric,
                &totals,
            )?;
            Ok(UsageStatistics {
                matched_request_count: totals.request_count,
                tokens: totals.tokens,
                cost_pico_usd: totals.cost_pico_usd,
                granularity,
                trend,
                attribution,
            })
        })
        .await
    }

    /// Returns current and retained route snapshots for the bounded route filter.
    ///
    /// # Errors
    ///
    /// Returns an executor or database error.
    pub async fn usage_route_options(&self) -> Result<Vec<UsageRouteOption>, StorageError> {
        self.call(|connection| {
            let mut statement = connection.prepare(
                "SELECT route_id, name, retained FROM (
                    SELECT route_id, display_name AS name, 0 AS retained, sort_order AS ordering FROM routes
                    UNION ALL
                    SELECT historical.final_route_id,
                           COALESCE((SELECT latest.final_route_name FROM proxy_requests latest
                            WHERE latest.final_route_id = historical.final_route_id
                              AND latest.final_route_name IS NOT NULL
                            ORDER BY latest.started_at_ms DESC, latest.request_id DESC LIMIT 1), '已删除路由'),
                           1, 1000000
                    FROM proxy_requests historical
                    WHERE historical.final_route_id IS NOT NULL
                      AND historical.final_route_id NOT IN (SELECT route_id FROM routes)
                    GROUP BY historical.final_route_id
                 ) ORDER BY ordering, name",
            )?;
            statement
                .query_map([], |row| {
                    Ok(UsageRouteOption {
                        route_id: RouteId::from_string(row.get(0)?),
                        name: row.get(1)?,
                        retained: row.get::<_, i64>(2)? != 0,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()
                .map_err(StorageError::from)
        })
        .await
    }

    /// Loads one privacy-safe request and its ordered attempts.
    ///
    /// # Errors
    ///
    /// Returns invalid-query, not-found, executor, or database errors.
    #[allow(clippy::too_many_lines)]
    pub async fn usage_request_detail(
        &self,
        request_id: String,
    ) -> Result<UsageRequestDetail, StorageError> {
        if request_id.is_empty() || request_id.len() > 128 {
            return Err(StorageError::InvalidUsageQuery);
        }
        self.call(move |connection| {
            let request = connection
                .query_row(
                    "SELECT request_id, started_at_ms, finished_at_ms, final_route_id, final_route_name,
                            requested_model, actual_model, reasoning_effort, streaming,
                            completion_state, http_status,
                            input_tokens, output_tokens, total_tokens, cached_input_tokens,
                            cache_write_input_tokens, total_latency_ms, first_output_latency_ms,
                            pricing_catalog_version, cost_status, upstream_cost_pico_usd,
                            requested_service_tier, actual_service_tier,
                            fallback_stop_reason, fallback_stop_target_route_id,
                            fallback_stop_target_route_name
                     FROM proxy_requests WHERE request_id = ?1",
                    [&request_id],
                    |row| {
                        let actual_service_tier = row.get::<_, Option<String>>(22)?;
                        Ok((
                            usage_history_row(row, actual_service_tier.clone())?,
                            row.get::<_, Option<String>>(21)?,
                            actual_service_tier,
                            row.get::<_, Option<String>>(23)?,
                            row.get::<_, Option<String>>(24)?.map(RouteId::from_string),
                            row.get::<_, Option<String>>(25)?,
                        ))
                    },
                )
                .optional()?
                .ok_or(StorageError::NotFound)?;
            let mut statement = connection.prepare(
                "SELECT attempt_id, attempt_index, attempt_role, route_id, route_name,
                        started_at_ms, finished_at_ms,
                        http_status, error_category, delivery_state, actual_model,
                        forwarded_service_tier, actual_service_tier, input_tokens, output_tokens, total_tokens,
                        cached_input_tokens, cache_write_input_tokens,
                        pricing_catalog_version, cost_status, cost_pico_usd,
                        routing_transition_kind, routing_transition_target_route_id,
                        routing_transition_target_route_name
                 FROM upstream_attempts WHERE request_id = ?1 ORDER BY attempt_index",
            )?;
            let mut attempts = statement
                .query_map([&request_id], |row| {
                    let attempt_id = row.get::<_, String>(0)?;
                    let transition = match (
                        row.get::<_, Option<String>>(21)?,
                        row.get::<_, Option<String>>(22)?,
                        row.get::<_, Option<String>>(23)?,
                    ) {
                        (Some(kind), Some(target_route_id), Some(target_route_name)) => {
                            let kind = RoutingTransitionKind::parse(&kind).map_err(|error| {
                                rusqlite::Error::FromSqlConversionFailure(
                                    21,
                                    rusqlite::types::Type::Text,
                                    Box::new(error),
                                )
                            })?;
                            let mut skips = connection.prepare(
                                "SELECT route_id, route_name, reason
                                 FROM upstream_attempt_routing_skips
                                 WHERE attempt_id = ?1 ORDER BY skip_order",
                            )?;
                            let skipped_routes = skips
                                .query_map([&attempt_id], |skip| {
                                    let reason = skip.get::<_, String>(2)?;
                                    if reason != "model_fallback_excluded" {
                                        return Err(rusqlite::Error::InvalidQuery);
                                    }
                                    Ok(RoutingTransitionSkip {
                                        route_id: RouteId::from_string(skip.get(0)?),
                                        route_name: skip.get(1)?,
                                    })
                                })?
                                .collect::<Result<Vec<_>, _>>()?;
                            Some(AttemptRoutingTransition {
                                kind,
                                target_route_id: RouteId::from_string(target_route_id),
                                target_route_name,
                                skipped_routes,
                            })
                        }
                        (None, None, None) => None,
                        _ => return Err(rusqlite::Error::InvalidQuery),
                    };
                    Ok(UsageAttemptDetail {
                        attempt_index: row.get(1)?,
                        attempt_role: AttemptRole::parse(&row.get::<_, String>(2)?)
                            .map_err(|error| rusqlite::Error::FromSqlConversionFailure(
                                2,
                                rusqlite::types::Type::Text,
                                Box::new(error),
                            ))?,
                        route_id: RouteId::from_string(row.get(3)?),
                        route_name: row.get(4)?,
                        started_at_ms: row.get(5)?,
                        finished_at_ms: row.get(6)?,
                        http_status: row.get(7)?,
                        error_category: row.get(8)?,
                        delivery_state: parse_delivery_state(&row.get::<_, String>(9)?),
                        actual_model: row.get(10)?,
                        forwarded_service_tier: row.get(11)?,
                        actual_service_tier: row.get(12)?,
                        input_tokens: row.get(13)?,
                        output_tokens: row.get(14)?,
                        total_tokens: row.get(15)?,
                        cached_input_tokens: row.get(16)?,
                        cache_write_input_tokens: row.get(17)?,
                        pricing_catalog_version: row.get(18)?,
                        cost_status: row
                            .get::<_, Option<String>>(19)?
                            .and_then(|value| CostStatus::parse(&value)),
                        cost_pico_usd: row.get(20)?,
                        routing_transition: transition,
                        routing_decision: None,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            let fallback_stop_reason = request
                .3
                .as_deref()
                .map(FallbackStopReason::parse)
                .transpose()?;
            materialize_routing_decisions(
                &mut attempts,
                fallback_stop_reason,
                request.4.as_ref(),
                request.5.as_deref(),
            );
            let cached_input_tokens = request.0.cached_input_tokens;
            let cache_write_input_tokens = request.0.cache_write_input_tokens;
            Ok(UsageRequestDetail {
                request: request.0,
                requested_service_tier: request.1,
                actual_service_tier: request.2,
                cached_input_tokens,
                cache_write_input_tokens,
                attempts,
            })
        })
        .await
    }

    /// Persists the latest request projection and any newly completed attempts.
    ///
    /// # Errors
    ///
    /// Returns an executor or `SQLite` transaction error.
    #[expect(
        clippy::too_many_lines,
        reason = "the request and attempt cost transaction is intentionally one atomic SQL operation"
    )]
    pub async fn record_request_history(
        &self,
        record: RequestHistoryRecord,
    ) -> Result<(), StorageError> {
        self.call(move |connection| {
            let transaction = connection.transaction()?;
            let request_id = record.request_id;
            transaction.execute(
                "INSERT INTO proxy_requests (
                    request_id, started_at_ms, finished_at_ms, turn_id,
                    requested_model, reasoning_effort, requested_service_tier,
                    actual_model, actual_service_tier,
                    final_route_id, final_route_name,
                    streaming, completion_state, http_status, error_category,
                    input_tokens, output_tokens, total_tokens, cached_input_tokens,
                    cache_write_input_tokens, total_latency_ms,
                    first_output_latency_ms, metadata_complete,
                    fallback_stop_reason, fallback_stop_target_route_id,
                    fallback_stop_target_route_name
                ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
                    ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20,
                    ?21, ?22, ?23, ?24, ?25, ?26
                ) ON CONFLICT(request_id) DO UPDATE SET
                    finished_at_ms = excluded.finished_at_ms,
                    turn_id = excluded.turn_id,
                    requested_model = excluded.requested_model,
                    reasoning_effort = excluded.reasoning_effort,
                    requested_service_tier = excluded.requested_service_tier,
                    actual_model = excluded.actual_model,
                    actual_service_tier = excluded.actual_service_tier,
                    final_route_id = excluded.final_route_id,
                    final_route_name = excluded.final_route_name,
                    streaming = excluded.streaming,
                    completion_state = excluded.completion_state,
                    http_status = excluded.http_status,
                    error_category = excluded.error_category,
                    input_tokens = excluded.input_tokens,
                    output_tokens = excluded.output_tokens,
                    total_tokens = excluded.total_tokens,
                    cached_input_tokens = excluded.cached_input_tokens,
                    cache_write_input_tokens = excluded.cache_write_input_tokens,
                    total_latency_ms = excluded.total_latency_ms,
                    first_output_latency_ms = excluded.first_output_latency_ms,
                    metadata_complete = excluded.metadata_complete,
                    fallback_stop_reason = excluded.fallback_stop_reason,
                    fallback_stop_target_route_id = excluded.fallback_stop_target_route_id,
                    fallback_stop_target_route_name = excluded.fallback_stop_target_route_name",
                params![
                    request_id,
                    record.started_at_ms,
                    record.finished_at_ms,
                    record.turn_id,
                    record.requested_model,
                    record.reasoning_effort,
                    record.requested_service_tier,
                    record.actual_model,
                    record.actual_service_tier,
                    record.final_route_id.as_ref().map(RouteId::as_str),
                    record.final_route_name,
                    record.streaming,
                    completion_state_value(&record.completion_state),
                    record.http_status,
                    record.error_category,
                    record.input_tokens,
                    record.output_tokens,
                    record.total_tokens,
                    record.cached_input_tokens,
                    record.cache_write_input_tokens,
                    record.total_latency_ms,
                    record.first_output_latency_ms,
                    record.metadata_complete,
                    record.fallback_stop_reason.map(FallbackStopReason::as_str),
                    record
                        .fallback_stop_target_route_id
                        .as_ref()
                        .map(RouteId::as_str),
                    record.fallback_stop_target_route_name,
                ],
            )?;
            for attempt in record.attempts {
                let priced = price_usage(&UsageObservation {
                    requested_model: record.requested_model.as_deref(),
                    actual_model: attempt.actual_model.as_deref(),
                    forwarded_service_tier: attempt.forwarded_service_tier.as_deref(),
                    actual_service_tier: attempt.actual_service_tier.as_deref(),
                    input_tokens: attempt.input_tokens,
                    output_tokens: attempt.output_tokens,
                    total_tokens: attempt.total_tokens,
                    cached_input_tokens: attempt.cached_input_tokens,
                    cache_write_input_tokens: attempt.cache_write_input_tokens,
                    possible_model_work: attempt.delivery_state != DeliveryState::None
                        || attempt.input_tokens.is_some()
                        || attempt.output_tokens.is_some(),
                });
                transaction.execute(
                    "INSERT INTO upstream_attempts (
                        attempt_id, request_id, attempt_index, attempt_role, route_id, route_name,
                        started_at_ms, finished_at_ms, http_status, error_category,
                        delivery_state, actual_model, forwarded_service_tier, actual_service_tier, input_tokens,
                        output_tokens, total_tokens, cached_input_tokens,
                        cache_write_input_tokens, pricing_catalog_version, cost_status,
                        cost_pico_usd
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11,
                              ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22)",
                    params![
                        attempt.attempt_id.as_str(),
                        request_id,
                        attempt.attempt_index,
                        attempt.attempt_role.as_str(),
                        attempt.route_id.as_str(),
                        attempt.route_name,
                        attempt.started_at_ms,
                        attempt.finished_at_ms,
                        attempt.http_status,
                        attempt.error_category,
                        delivery_state_value(&attempt.delivery_state),
                        attempt.actual_model,
                        attempt.forwarded_service_tier,
                        attempt.actual_service_tier,
                        attempt.input_tokens,
                        attempt.output_tokens,
                        attempt.total_tokens,
                        attempt.cached_input_tokens,
                        attempt.cache_write_input_tokens,
                        priced.catalog_version,
                        priced.status.as_str(),
                        priced.amount_pico_usd,
                    ],
                )?;
            }
            let attempt_costs = {
                let mut statement = transaction.prepare(
                    "SELECT cost_status, cost_pico_usd, pricing_catalog_version
                     FROM upstream_attempts WHERE request_id = ?1 ORDER BY attempt_index",
                )?;
                statement
                    .query_map([&request_id], |row| {
                        let status = row.get::<_, String>(0)?;
                        Ok(PricedUsage {
                            catalog_version: match row.get::<_, Option<String>>(2)?.as_deref() {
                                Some(crate::pricing::CATALOG_VERSION) => {
                                    Some(crate::pricing::CATALOG_VERSION)
                                }
                                Some(crate::pricing::PRIORITY_CATALOG_VERSION) => {
                                    Some(crate::pricing::PRIORITY_CATALOG_VERSION)
                                }
                                _ => None,
                            },
                            status: CostStatus::parse(&status).unwrap_or(CostStatus::Unavailable),
                            amount_pico_usd: row.get(1)?,
                        })
                    })?
                    .collect::<Result<Vec<_>, _>>()?
            };
            let request_cost = fold_request_cost(&attempt_costs);
            transaction.execute(
                "UPDATE proxy_requests SET pricing_catalog_version = ?2,
                    cost_status = ?3, upstream_cost_pico_usd = ?4 WHERE request_id = ?1",
                params![
                    request_id,
                    request_cost.catalog_version,
                    request_cost.status.as_str(),
                    request_cost.amount_pico_usd,
                ],
            )?;
            transaction.commit()?;
            Ok(())
        })
        .await
    }

    /// Persists one terminal automatic-Fallback decision after its owning
    /// attempt has already been queued.
    ///
    /// # Errors
    ///
    /// Returns an executor or database error. A missing request is a quiet
    /// metadata no-op so routing never depends on history availability.
    pub async fn record_fallback_stop(
        &self,
        record: FallbackStopRecord,
    ) -> Result<bool, StorageError> {
        self.call(move |connection| {
            let changed = connection.execute(
                "UPDATE proxy_requests
                 SET fallback_stop_reason = ?1,
                     fallback_stop_target_route_id = ?2,
                     fallback_stop_target_route_name = ?3
                 WHERE request_id = ?4
                   AND (SELECT MAX(attempt_index)
                        FROM upstream_attempts
                        WHERE request_id = ?4) = ?5",
                params![
                    record.reason.as_str(),
                    record.target_route_id.as_ref().map(RouteId::as_str),
                    record.target_route_name,
                    record.request_id,
                    record.attempt_index,
                ],
            )? == 1;
            Ok(changed)
        })
        .await
    }

    /// Persists one explicit routing transition on its owning attempt.
    ///
    /// # Errors
    ///
    /// Returns an executor or database error. A transition whose attempt has
    /// not been persisted is ignored without attaching it to another attempt.
    pub async fn record_attempt_routing_transition(
        &self,
        record: AttemptRoutingTransitionRecord,
    ) -> Result<bool, StorageError> {
        self.call(move |connection| {
            let transaction = connection.transaction()?;
            let attempt_id = transaction
                .query_row(
                    "SELECT attempt_id FROM upstream_attempts
                     WHERE request_id = ?1 AND attempt_index = ?2",
                    params![record.request_id, record.attempt_index],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            let Some(attempt_id) = attempt_id else {
                transaction.commit()?;
                return Ok(false);
            };
            transaction.execute(
                "UPDATE upstream_attempts
                 SET routing_transition_kind = ?1,
                     routing_transition_target_route_id = ?2,
                     routing_transition_target_route_name = ?3
                 WHERE attempt_id = ?4",
                params![
                    record.transition.kind.as_str(),
                    record.transition.target_route_id.as_str(),
                    record.transition.target_route_name,
                    attempt_id,
                ],
            )?;
            transaction.execute(
                "DELETE FROM upstream_attempt_routing_skips WHERE attempt_id = ?1",
                [&attempt_id],
            )?;
            for (skip_order, skipped) in record.transition.skipped_routes.iter().enumerate() {
                let skip_order = i64::try_from(skip_order)
                    .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
                transaction.execute(
                    "INSERT INTO upstream_attempt_routing_skips
                     (attempt_id, skip_order, route_id, route_name, reason)
                     VALUES (?1, ?2, ?3, ?4, 'model_fallback_excluded')",
                    params![
                        attempt_id,
                        skip_order,
                        skipped.route_id.as_str(),
                        skipped.route_name,
                    ],
                )?;
            }
            transaction.commit()?;
            Ok(true)
        })
        .await
    }

    /// Returns the latest non-cancelled retained upstream result per route.
    ///
    /// # Errors
    ///
    /// Returns an executor or `SQLite` query error.
    pub async fn latest_inference_attempts(
        &self,
    ) -> Result<Vec<LatestInferenceAttempt>, StorageError> {
        self.call(|connection| {
            let mut statement = connection.prepare(
                "SELECT a.route_id, a.finished_at_ms, a.http_status, a.delivery_state, a.error_category
                 FROM upstream_attempts a
                 JOIN proxy_requests r ON r.request_id = a.request_id
                 WHERE a.finished_at_ms IS NOT NULL AND r.completion_state != 'cancelled'
                 ORDER BY a.finished_at_ms DESC, a.attempt_index DESC, a.attempt_id DESC",
            )?;
            let rows = statement.query_map([], |row| {
                let status = row.get::<_, Option<u16>>(2)?;
                let delivery = row.get::<_, String>(3)?;
                Ok(LatestInferenceAttempt {
                    route_id: RouteId::from_string(row.get(0)?),
                    finished_at_ms: row.get(1)?,
                    succeeded: status.is_some_and(|status| (200..300).contains(&status))
                        && delivery == "completed",
                    error_category: row.get(4)?,
                })
            })?;
            let mut seen = std::collections::HashSet::new();
            let mut latest = Vec::new();
            for row in rows {
                let row = row?;
                if seen.insert(row.route_id.clone()) {
                    latest.push(row);
                }
            }
            Ok(latest)
        })
        .await
    }

    /// Freezes the first pre-takeover Codex configuration.
    ///
    /// Existing baseline bytes are returned unchanged on every later call.
    ///
    /// # Errors
    ///
    /// Returns an executor or `SQLite` transaction error.
    pub async fn capture_codex_baseline(
        &self,
        original_exists: bool,
        raw_bytes: Vec<u8>,
        unix_mode: Option<u32>,
    ) -> Result<CodexBaseline, StorageError> {
        let raw_bytes = original_exists.then_some(raw_bytes);
        let unix_mode = if original_exists { unix_mode } else { None };
        self.call_critical(move |connection| {
            let transaction = connection.transaction()?;
            let timestamp = now_millis();
            let inserted = transaction.execute(
                "INSERT OR IGNORE INTO codex_baseline (singleton, original_exists, raw_bytes, unix_mode, captured_at_ms) VALUES (1, ?1, ?2, ?3, ?4)",
                params![original_exists, raw_bytes, unix_mode, timestamp],
            )? == 1;
            let baseline = read_codex_baseline(&transaction)?.ok_or(StorageError::NotFound)?;
            transaction.execute(
                "INSERT OR IGNORE INTO codex_recovery_config (singleton, original_exists, raw_bytes, unix_mode, updated_at_ms) VALUES (1, ?1, ?2, ?3, ?4)",
                params![baseline.original_exists, baseline.original_exists.then_some(&baseline.raw_bytes), baseline.unix_mode, timestamp],
            )?;
            let revision = inserted
                .then(|| mark_critical_change(&transaction))
                .transpose()?;
            transaction.commit()?;
            Ok((baseline, revision))
        })
        .await
    }

    /// Reads the immutable Codex baseline when takeover has occurred.
    ///
    /// # Errors
    ///
    /// Returns an executor or `SQLite` query error.
    pub async fn codex_baseline(&self) -> Result<Option<CodexBaseline>, StorageError> {
        self.call(|connection| read_codex_baseline(connection))
            .await
    }

    /// Reads the mutable snapshot restored when Codex disconnects.
    ///
    /// # Errors
    ///
    /// Returns an executor or `SQLite` query error.
    pub async fn codex_recovery_config(&self) -> Result<Option<CodexRecoveryConfig>, StorageError> {
        self.call(|connection| read_codex_recovery_config(connection))
            .await
    }

    /// Replaces the complete disconnect recovery snapshot.
    ///
    /// # Errors
    ///
    /// Returns an executor or `SQLite` transaction error.
    pub async fn update_codex_recovery_config(
        &self,
        original_exists: bool,
        raw_bytes: Vec<u8>,
        unix_mode: Option<u32>,
    ) -> Result<CodexRecoveryConfig, StorageError> {
        let raw_bytes = original_exists.then_some(raw_bytes);
        let unix_mode = if original_exists { unix_mode } else { None };
        self.call_critical(move |connection| {
            let transaction = connection.transaction()?;
            let baseline_exists: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM codex_baseline WHERE singleton = 1)",
                [],
                |row| row.get(0),
            )?;
            if !baseline_exists {
                return Err(StorageError::NotFound);
            }
            transaction.execute(
                "INSERT INTO codex_recovery_config (singleton, original_exists, raw_bytes, unix_mode, updated_at_ms) VALUES (1, ?1, ?2, ?3, ?4)
                 ON CONFLICT(singleton) DO UPDATE SET original_exists = excluded.original_exists, raw_bytes = excluded.raw_bytes, unix_mode = excluded.unix_mode, updated_at_ms = excluded.updated_at_ms",
                params![original_exists, raw_bytes, unix_mode, now_millis()],
            )?;
            let snapshot =
                read_codex_recovery_config(&transaction)?.ok_or(StorageError::NotFound)?;
            let revision = mark_critical_change(&transaction)?;
            transaction.commit()?;
            Ok((snapshot, Some(revision)))
        })
        .await
    }

    /// Resets the disconnect recovery snapshot to the immutable baseline.
    ///
    /// # Errors
    ///
    /// Returns `NotFound` when no baseline exists, or an executor/transaction error.
    pub async fn reset_codex_recovery_config_to_baseline(
        &self,
    ) -> Result<CodexRecoveryConfig, StorageError> {
        self.call_critical(|connection| {
            let transaction = connection.transaction()?;
            let baseline = read_codex_baseline(&transaction)?.ok_or(StorageError::NotFound)?;
            transaction.execute(
                "INSERT INTO codex_recovery_config (singleton, original_exists, raw_bytes, unix_mode, updated_at_ms) VALUES (1, ?1, ?2, ?3, ?4)
                 ON CONFLICT(singleton) DO UPDATE SET original_exists = excluded.original_exists, raw_bytes = excluded.raw_bytes, unix_mode = excluded.unix_mode, updated_at_ms = excluded.updated_at_ms",
                params![baseline.original_exists, baseline.original_exists.then_some(&baseline.raw_bytes), baseline.unix_mode, now_millis()],
            )?;
            let snapshot =
                read_codex_recovery_config(&transaction)?.ok_or(StorageError::NotFound)?;
            let revision = mark_critical_change(&transaction)?;
            transaction.commit()?;
            Ok((snapshot, Some(revision)))
        })
        .await
    }

    /// Returns an existing singleton secret or inserts the supplied value.
    ///
    /// # Errors
    ///
    /// Returns an executor or `SQLite` transaction error.
    pub async fn get_or_create_singleton_secret(
        &self,
        kind: String,
        value: ApiKey,
    ) -> Result<ApiKey, StorageError> {
        let bytes = value.expose().to_vec();
        self.call_critical(move |connection| {
            let transaction = connection.transaction()?;
            let secret_id = SecretId::new();
            let timestamp = now_millis();
            let inserted = transaction.execute(
                "INSERT OR IGNORE INTO secrets (secret_id, kind, value, created_at_ms, updated_at_ms) VALUES (?1, ?2, ?3, ?4, ?4)",
                params![secret_id.as_str(), &kind, bytes, timestamp],
            )? == 1;
            let stored = transaction.query_row(
                "SELECT value FROM secrets WHERE kind = ?1 LIMIT 1",
                [&kind],
                |row| row.get(0),
            )?;
            let revision = inserted
                .then(|| mark_critical_change(&transaction))
                .transpose()?;
            transaction.commit()?;
            Ok((ApiKey::from_stored(stored), revision))
        })
        .await
    }

    #[cfg(test)]
    async fn test_execute<T, F>(&self, operation: F) -> Result<T, StorageError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T, StorageError> + Send + 'static,
    {
        self.call(operation).await
    }
}

#[derive(Clone)]
pub struct SqliteBalanceRouteSource {
    database: DatabaseExecutor,
}

impl SqliteBalanceRouteSource {
    #[must_use]
    pub const fn new(database: DatabaseExecutor) -> Self {
        Self { database }
    }
}

#[async_trait]
impl BalanceRouteSource for SqliteBalanceRouteSource {
    async fn load_enabled_route(
        &self,
        route_id: &RouteId,
    ) -> Result<Option<BalanceRouteConfig>, BalanceError> {
        self.database
            .enabled_balance_route(route_id.clone())
            .await
            .map_err(|_| balance_source_error())
    }

    async fn is_current(&self, route_id: &RouteId, query_revision: u64) -> bool {
        self.database
            .enabled_balance_route(route_id.clone())
            .await
            .ok()
            .flatten()
            .is_some_and(|route| route.query_revision == query_revision)
    }

    async fn eligible_route_ids(&self) -> Result<Vec<RouteId>, BalanceError> {
        self.database
            .enabled_balance_route_ids()
            .await
            .map_err(|_| balance_source_error())
    }

    async fn active_route_id(&self) -> Option<RouteId> {
        self.database.active_route_id().await.ok().flatten()
    }
}

#[async_trait]
pub trait SecretStore: Send + Sync {
    async fn get(&self, secret_id: SecretId) -> Result<ApiKey, StorageError>;
    async fn put(&self, kind: String, value: ApiKey) -> Result<SecretId, StorageError>;
    async fn replace(&self, secret_id: SecretId, value: ApiKey) -> Result<(), StorageError>;
    async fn delete(&self, secret_id: SecretId) -> Result<(), StorageError>;
}

#[derive(Clone)]
pub struct SqliteSecretStore {
    database: DatabaseExecutor,
}

impl SqliteSecretStore {
    #[must_use]
    pub const fn new(database: DatabaseExecutor) -> Self {
        Self { database }
    }
}

#[async_trait]
impl SecretStore for SqliteSecretStore {
    async fn get(&self, secret_id: SecretId) -> Result<ApiKey, StorageError> {
        self.database
            .call(move |connection| {
                let value = connection
                    .query_row(
                        "SELECT value FROM secrets WHERE secret_id = ?1",
                        [secret_id.as_str()],
                        |row| row.get(0),
                    )
                    .optional()?
                    .ok_or(StorageError::NotFound)?;
                Ok(ApiKey::from_stored(value))
            })
            .await
    }

    async fn put(&self, kind: String, value: ApiKey) -> Result<SecretId, StorageError> {
        let secret_id = SecretId::new();
        let result_id = secret_id.clone();
        let bytes = value.expose().to_vec();
        self.database
            .call_critical(move |connection| {
                let transaction = connection.transaction()?;
                let timestamp = now_millis();
                transaction.execute(
                    "INSERT INTO secrets (secret_id, kind, value, created_at_ms, updated_at_ms) VALUES (?1, ?2, ?3, ?4, ?4)",
                    params![secret_id.as_str(), kind, bytes, timestamp],
                )?;
                let revision = mark_critical_change(&transaction)?;
                transaction.commit()?;
                Ok(((), Some(revision)))
            })
            .await?;
        Ok(result_id)
    }

    async fn replace(&self, secret_id: SecretId, value: ApiKey) -> Result<(), StorageError> {
        let bytes = value.expose().to_vec();
        self.database
            .call_critical(move |connection| {
                let transaction = connection.transaction()?;
                let current: Option<Vec<u8>> = transaction
                    .query_row(
                        "SELECT value FROM secrets WHERE secret_id = ?1",
                        [secret_id.as_str()],
                        |row| row.get(0),
                    )
                    .optional()?;
                let current = current.ok_or(StorageError::NotFound)?;
                if current == bytes {
                    transaction.commit()?;
                    return Ok(((), None));
                }
                transaction.execute(
                    "UPDATE secrets SET value = ?1, updated_at_ms = ?2 WHERE secret_id = ?3",
                    params![bytes, now_millis(), secret_id.as_str()],
                )?;
                let revision = mark_critical_change(&transaction)?;
                transaction.commit()?;
                Ok(((), Some(revision)))
            })
            .await
    }

    async fn delete(&self, secret_id: SecretId) -> Result<(), StorageError> {
        self.database
            .call_critical(move |connection| {
                let transaction = connection.transaction()?;
                let changed = transaction.execute(
                    "DELETE FROM secrets WHERE secret_id = ?1",
                    [secret_id.as_str()],
                )?;
                if changed == 0 {
                    return Err(StorageError::NotFound);
                }
                let revision = mark_critical_change(&transaction)?;
                transaction.commit()?;
                Ok(((), Some(revision)))
            })
            .await
    }
}

fn validate_balance_query(
    query: Option<BalanceQueryInput>,
) -> Result<Option<BalanceQueryInput>, ValidationError> {
    if let Some(query) = &query {
        if query.mode == BalanceQueryMode::CustomJs
            && query.enabled
            && query.custom_source.trim().is_empty()
        {
            BalanceScriptSource::parse_required(&query.custom_source)?;
        }
        if !query.custom_source.is_empty() {
            BalanceScriptSource::parse(&query.custom_source)?;
        }
    }
    Ok(query)
}

fn balance_revision(
    base_url: &str,
    api_key: &[u8],
    mode: BalanceQueryMode,
    custom_source: &str,
) -> u64 {
    let mut hash = Sha256::new();
    hash.update(base_url.as_bytes());
    hash.update(api_key);
    hash.update(mode.as_str().as_bytes());
    if mode == BalanceQueryMode::CustomJs {
        hash.update(custom_source.as_bytes());
    }
    let digest = hash.finalize();
    let mut revision = [0_u8; 8];
    revision.copy_from_slice(&digest[..8]);
    u64::from_le_bytes(revision)
}

fn balance_source_error() -> BalanceError {
    BalanceError {
        stage: BalanceErrorStage::RequestValidation,
        category: BalanceErrorCategory::InvalidRequest,
        transient: false,
    }
}

fn read_codex_baseline(connection: &Connection) -> Result<Option<CodexBaseline>, StorageError> {
    Ok(connection
        .query_row(
            "SELECT original_exists, raw_bytes, unix_mode, captured_at_ms FROM codex_baseline WHERE singleton = 1",
            [],
            |row| {
                Ok(CodexBaseline {
                    original_exists: row.get(0)?,
                    raw_bytes: row.get::<_, Option<Vec<u8>>>(1)?.unwrap_or_default(),
                    unix_mode: row.get(2)?,
                    captured_at_ms: row.get(3)?,
                })
            },
        )
        .optional()?)
}

fn read_codex_recovery_config(
    connection: &Connection,
) -> Result<Option<CodexRecoveryConfig>, StorageError> {
    Ok(connection
        .query_row(
            "SELECT original_exists, raw_bytes, unix_mode, updated_at_ms FROM codex_recovery_config WHERE singleton = 1",
            [],
            |row| {
                Ok(CodexRecoveryConfig {
                    original_exists: row.get(0)?,
                    raw_bytes: row.get::<_, Option<Vec<u8>>>(1)?.unwrap_or_default(),
                    unix_mode: row.get(2)?,
                    updated_at_ms: row.get(3)?,
                })
            },
        )
        .optional()?)
}

fn write_balance_query(
    transaction: &Transaction<'_>,
    route_id: &RouteId,
    query: Option<&BalanceQueryInput>,
    timestamp: i64,
) -> Result<(), rusqlite::Error> {
    if let Some(query) = query {
        transaction.execute(
            "INSERT INTO balance_queries (route_id, mode, enabled, custom_source, updated_at_ms) VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT(route_id) DO UPDATE SET mode = excluded.mode, enabled = excluded.enabled, custom_source = excluded.custom_source, updated_at_ms = excluded.updated_at_ms",
            params![route_id.as_str(), query.mode.as_str(), query.enabled, query.custom_source, timestamp],
        )?;
    } else {
        transaction.execute(
            "DELETE FROM balance_queries WHERE route_id = ?1",
            [route_id.as_str()],
        )?;
    }
    Ok(())
}

fn read_balance_query(
    connection: &Connection,
    route_id: &RouteId,
) -> Result<Option<BalanceQueryInput>, StorageError> {
    connection
        .query_row(
            "SELECT mode, enabled, custom_source FROM balance_queries WHERE route_id = ?1",
            [route_id.as_str()],
            |row| {
                let mode = row.get::<_, String>(0)?;
                Ok((mode, row.get::<_, bool>(1)?, row.get::<_, String>(2)?))
            },
        )
        .optional()?
        .map(|(mode, enabled, custom_source)| {
            Ok::<BalanceQueryInput, StorageError>(BalanceQueryInput {
                mode: BalanceQueryMode::parse_persisted(&mode)
                    .ok_or(StorageError::Initialization)?,
                enabled,
                custom_source,
            })
        })
        .transpose()
}

fn read_codex_models(
    connection: &Connection,
    route_id: &RouteId,
) -> Result<Vec<CodexModelRecord>, StorageError> {
    let mut statement = connection.prepare(
        "SELECT model_id, display_name, context_window FROM codex_models WHERE route_id = ?1 ORDER BY sort_order",
    )?;
    let raw = statement
        .query_map([route_id.as_str()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<i64>>(2)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    raw.into_iter()
        .enumerate()
        .map(|(index, (model_id, display_name, context_window))| {
            let context_window = context_window
                .map(u64::try_from)
                .transpose()
                .map_err(|_| StorageError::Initialization)?;
            CodexModel::parse(index, &model_id, display_name.as_deref(), context_window)
                .map(CodexModelRecord::from)
                .map_err(StorageError::from)
        })
        .collect()
}

fn write_codex_models(
    transaction: &Transaction<'_>,
    route_id: &RouteId,
    models: &[CodexModelRecord],
) -> Result<(), StorageError> {
    transaction.execute(
        "DELETE FROM codex_models WHERE route_id = ?1",
        [route_id.as_str()],
    )?;
    for (sort_order, model) in models.iter().enumerate() {
        let context_window = model
            .context_window
            .map(i64::try_from)
            .transpose()
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
        let sort_order = i64::try_from(sort_order)
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
        transaction.execute(
            "INSERT INTO codex_models (route_id, model_id, display_name, context_window, sort_order) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                route_id.as_str(),
                model.model_id,
                model.display_name,
                context_window,
                sort_order,
            ],
        )?;
    }
    Ok(())
}

fn read_fallback_excluded_models(
    connection: &Connection,
    route_id: &RouteId,
) -> Result<Vec<String>, StorageError> {
    let mut statement = connection.prepare(
        "SELECT model_id FROM route_fallback_excluded_models
         WHERE route_id = ?1 ORDER BY sort_order",
    )?;
    let models = statement
        .query_map([route_id.as_str()], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    normalize_fallback_excluded_models(models).map_err(StorageError::from)
}

fn write_fallback_excluded_models(
    transaction: &Transaction<'_>,
    route_id: &RouteId,
    models: &[String],
) -> Result<(), StorageError> {
    transaction.execute(
        "DELETE FROM route_fallback_excluded_models WHERE route_id = ?1",
        [route_id.as_str()],
    )?;
    for (sort_order, model) in models.iter().enumerate() {
        let sort_order = i64::try_from(sort_order)
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
        transaction.execute(
            "INSERT INTO route_fallback_excluded_models
             (route_id, model_id, sort_order) VALUES (?1, ?2, ?3)",
            params![route_id.as_str(), model, sort_order],
        )?;
    }
    Ok(())
}

fn confirm_script_risk_if_needed(
    transaction: &Transaction<'_>,
    query: Option<&BalanceQueryInput>,
    accepted: bool,
) -> Result<bool, StorageError> {
    if !query.is_some_and(|query| query.enabled && query.mode == BalanceQueryMode::CustomJs) {
        return Ok(false);
    }
    let confirmed: bool = transaction.query_row(
        "SELECT balance_script_risk_confirmed FROM app_settings WHERE singleton = 1",
        [],
        |row| row.get(0),
    )?;
    if confirmed {
        return Ok(false);
    }
    if !accepted {
        return Err(StorageError::BalanceScriptRiskConfirmationRequired);
    }
    transaction.execute(
        "UPDATE app_settings SET balance_script_risk_confirmed = 1 WHERE singleton = 1",
        [],
    )?;
    Ok(true)
}

fn open_connection(path: &Path) -> Result<Connection, StorageError> {
    let mut connection = Connection::open(path)?;
    connection.busy_timeout(std::time::Duration::from_secs(5))?;
    connection.pragma_update(None, "journal_mode", "DELETE")?;
    connection.pragma_update(None, "synchronous", "FULL")?;
    connection.pragma_update(None, "foreign_keys", true)?;
    migrate(&mut connection)?;
    verify_connection(&connection)?;
    Ok(connection)
}

fn migrate(connection: &mut Connection) -> Result<(), StorageError> {
    let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version > SCHEMA_VERSION {
        return Err(StorageError::FutureSchema);
    }
    if version == 0 {
        migrate_v1(connection)?;
    }
    if version < 2 {
        migrate_v2(connection)?;
    }
    if version < 3 {
        migrate_v3(connection)?;
    }
    if version < 4 {
        migrate_v4(connection)?;
    }
    if version < 5 {
        migrate_v5(connection)?;
    }
    if version < 6 {
        migrate_v6(connection)?;
    }
    if version < 7 {
        migrate_v7(connection)?;
    }
    if version < 8 {
        migrate_v8(connection)?;
    }
    if version < 9 {
        migrate_v9(connection)?;
    }
    if version < 10 {
        migrate_v10(connection)?;
    }
    if version < 11 {
        migrate_v11(connection)?;
    }
    if version < 12 {
        migrate_v12(connection)?;
    }
    if version < 13 {
        migrate_v13(connection)?;
    }
    if version < 14 {
        migrate_v14(connection)?;
    }
    if version < 15 {
        migrate_v15(connection)?;
    }
    if version < 16 {
        migrate_v16(connection)?;
    }
    if version < 17 {
        migrate_v17(connection)?;
    }
    if version < 18 {
        migrate_v18(connection)?;
    }
    if version < 19 {
        migrate_v19(connection)?;
    }
    if version < 20 {
        migrate_v20(connection)?;
    }
    if version < 21 {
        migrate_v21(connection)?;
    }
    if version < 22 {
        migrate_v22(connection)?;
    }
    Ok(())
}

fn migrate_v1(connection: &mut Connection) -> Result<(), StorageError> {
    connection.pragma_update(None, "auto_vacuum", "INCREMENTAL")?;
    let transaction = connection.transaction()?;
    transaction.execute_batch(
        "
        CREATE TABLE secrets (
            secret_id TEXT PRIMARY KEY,
            kind TEXT NOT NULL,
            value BLOB NOT NULL,
            created_at_ms INTEGER NOT NULL,
            updated_at_ms INTEGER NOT NULL
        );
        CREATE UNIQUE INDEX secrets_singleton_kind_idx ON secrets(kind) WHERE kind = 'gateway_token';
        CREATE TABLE routes (
            route_id TEXT PRIMARY KEY,
            display_name TEXT NOT NULL,
            display_name_key TEXT NOT NULL UNIQUE,
            base_url TEXT NOT NULL,
            secret_id TEXT NOT NULL UNIQUE REFERENCES secrets(secret_id) ON DELETE RESTRICT,
            sort_order INTEGER NOT NULL,
            created_at_ms INTEGER NOT NULL,
            updated_at_ms INTEGER NOT NULL
        );
        CREATE TABLE balance_scripts (
            route_id TEXT PRIMARY KEY REFERENCES routes(route_id) ON DELETE CASCADE,
            contract_version TEXT NOT NULL,
            enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
            source TEXT NOT NULL,
            updated_at_ms INTEGER NOT NULL
        );
        CREATE TABLE route_state (
            singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
            route_id TEXT REFERENCES routes(route_id) ON DELETE SET NULL,
            updated_at_ms INTEGER NOT NULL
        );
        CREATE TABLE app_settings (
            singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
            proxy_port INTEGER NOT NULL,
            first_run_presented INTEGER NOT NULL CHECK (first_run_presented IN (0, 1)),
            balance_script_risk_confirmed INTEGER NOT NULL CHECK (balance_script_risk_confirmed IN (0, 1))
        );
        CREATE TABLE codex_baseline (
            singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
            original_exists INTEGER NOT NULL CHECK (original_exists IN (0, 1)),
            raw_bytes BLOB,
            unix_mode INTEGER,
            captured_at_ms INTEGER NOT NULL
        );
        CREATE TABLE proxy_requests (
            request_id TEXT PRIMARY KEY,
            started_at_ms INTEGER NOT NULL,
            finished_at_ms INTEGER,
            turn_id TEXT,
            turn_sequence INTEGER,
            reconnect_sequence INTEGER,
            requested_model TEXT,
            actual_model TEXT,
            final_route_id TEXT,
            final_route_name TEXT,
            streaming INTEGER NOT NULL,
            completion_state TEXT NOT NULL,
            http_status INTEGER,
            error_category TEXT,
            input_tokens INTEGER,
            output_tokens INTEGER,
            total_tokens INTEGER,
            total_latency_ms INTEGER,
            first_output_latency_ms INTEGER,
            metadata_complete INTEGER NOT NULL
        );
        CREATE INDEX proxy_requests_started_at_idx ON proxy_requests(started_at_ms);
        CREATE TABLE upstream_attempts (
            attempt_id TEXT PRIMARY KEY,
            request_id TEXT NOT NULL REFERENCES proxy_requests(request_id) ON DELETE CASCADE,
            attempt_index INTEGER NOT NULL,
            route_id TEXT NOT NULL,
            route_name TEXT NOT NULL,
            started_at_ms INTEGER NOT NULL,
            finished_at_ms INTEGER,
            http_status INTEGER,
            error_category TEXT,
            delivery_state TEXT NOT NULL,
            input_tokens INTEGER,
            output_tokens INTEGER,
            total_tokens INTEGER,
            UNIQUE(request_id, attempt_index)
        );
        INSERT INTO route_state (singleton, route_id, updated_at_ms) VALUES (1, NULL, 0);
        INSERT INTO app_settings (singleton, proxy_port, first_run_presented, balance_script_risk_confirmed) VALUES (1, 32189, 0, 0);
        PRAGMA user_version = 1;
        ",
    )?;
    transaction.commit()?;
    Ok(())
}

fn migrate_v2(connection: &mut Connection) -> Result<(), StorageError> {
    let transaction = connection.transaction()?;
    transaction.execute_batch(
        "
        ALTER TABLE route_state ADD COLUMN selection_generation INTEGER NOT NULL DEFAULT 0;
        CREATE TABLE fallback_config (
            singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
            enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
            config_revision INTEGER NOT NULL,
            updated_at_ms INTEGER NOT NULL
        );
        INSERT INTO fallback_config (singleton, enabled, config_revision, updated_at_ms)
        VALUES (1, 0, 0, 0);
        PRAGMA user_version = 2;
        ",
    )?;
    transaction.commit()?;
    Ok(())
}

fn migrate_v3(connection: &mut Connection) -> Result<(), StorageError> {
    let transaction = connection.transaction()?;
    transaction.execute_batch(
        "
        ALTER TABLE app_settings ADD COLUMN menu_balance_debounce_seconds INTEGER NOT NULL DEFAULT 30 CHECK (menu_balance_debounce_seconds BETWEEN 10 AND 600);
        ALTER TABLE app_settings ADD COLUMN automatic_balance_refresh_minutes INTEGER NOT NULL DEFAULT 30 CHECK (automatic_balance_refresh_minutes BETWEEN 5 AND 1440);
        PRAGMA user_version = 3;
        ",
    )?;
    transaction.commit()?;
    Ok(())
}

fn migrate_v4(connection: &mut Connection) -> Result<(), StorageError> {
    let transaction = connection.transaction()?;
    transaction.execute_batch(
        "
        CREATE TABLE recovery_revision (
            singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
            critical_revision INTEGER NOT NULL CHECK (critical_revision >= 0),
            updated_at_ms INTEGER NOT NULL
        );
        CREATE TABLE recovery_point_metadata (
            singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
            format_version INTEGER NOT NULL,
            point_id TEXT NOT NULL,
            created_at_ms INTEGER NOT NULL,
            critical_revision INTEGER NOT NULL CHECK (critical_revision >= 0)
        );
        INSERT INTO recovery_revision (singleton, critical_revision, updated_at_ms)
        VALUES (1, 0, 0);
        PRAGMA user_version = 4;
        ",
    )?;
    transaction.commit()?;
    Ok(())
}

fn migrate_v5(connection: &mut Connection) -> Result<(), StorageError> {
    let transaction = connection.transaction()?;
    transaction.execute_batch(
        "
        ALTER TABLE proxy_requests ADD COLUMN requested_service_tier TEXT;
        ALTER TABLE proxy_requests ADD COLUMN actual_service_tier TEXT;
        ALTER TABLE proxy_requests ADD COLUMN cached_input_tokens INTEGER CHECK (cached_input_tokens >= 0);
        ALTER TABLE proxy_requests ADD COLUMN cache_write_input_tokens INTEGER CHECK (cache_write_input_tokens >= 0);
        ALTER TABLE proxy_requests ADD COLUMN pricing_catalog_version TEXT;
        ALTER TABLE proxy_requests ADD COLUMN cost_status TEXT CHECK (cost_status IN ('exact', 'partial', 'unavailable', 'not_applicable'));
        ALTER TABLE proxy_requests ADD COLUMN upstream_cost_pico_usd INTEGER CHECK (upstream_cost_pico_usd >= 0);
        ALTER TABLE upstream_attempts ADD COLUMN actual_model TEXT;
        ALTER TABLE upstream_attempts ADD COLUMN actual_service_tier TEXT;
        ALTER TABLE upstream_attempts ADD COLUMN cached_input_tokens INTEGER CHECK (cached_input_tokens >= 0);
        ALTER TABLE upstream_attempts ADD COLUMN cache_write_input_tokens INTEGER CHECK (cache_write_input_tokens >= 0);
        ALTER TABLE upstream_attempts ADD COLUMN pricing_catalog_version TEXT;
        ALTER TABLE upstream_attempts ADD COLUMN cost_status TEXT CHECK (cost_status IN ('exact', 'partial', 'unavailable', 'not_applicable'));
        ALTER TABLE upstream_attempts ADD COLUMN cost_pico_usd INTEGER CHECK (cost_pico_usd >= 0);
        DROP INDEX proxy_requests_started_at_idx;
        CREATE INDEX proxy_requests_keyset_idx ON proxy_requests(started_at_ms DESC, request_id DESC);
        CREATE INDEX proxy_requests_status_keyset_idx ON proxy_requests(completion_state, started_at_ms DESC, request_id DESC);
        CREATE INDEX proxy_requests_route_keyset_idx ON proxy_requests(final_route_id, started_at_ms DESC, request_id DESC);
        CREATE INDEX proxy_requests_model_keyset_idx ON proxy_requests(COALESCE(actual_model, requested_model), started_at_ms DESC, request_id DESC);
        PRAGMA user_version = 5;
        ",
    )?;
    transaction.commit()?;
    Ok(())
}

fn migrate_v6(connection: &mut Connection) -> Result<(), StorageError> {
    let transaction = connection.transaction()?;
    transaction.execute_batch(
        "
        ALTER TABLE proxy_requests ADD COLUMN reasoning_effort TEXT;
        PRAGMA user_version = 6;
        ",
    )?;
    transaction.commit()?;
    Ok(())
}

fn migrate_v7(connection: &mut Connection) -> Result<(), StorageError> {
    let transaction = connection.transaction()?;
    transaction.execute_batch(
        "
        ALTER TABLE proxy_requests ADD COLUMN first_text_output_latency_ms INTEGER
            CHECK (first_text_output_latency_ms >= 0);
        PRAGMA user_version = 7;
        ",
    )?;
    transaction.commit()?;
    Ok(())
}

fn migrate_v8(connection: &mut Connection) -> Result<(), StorageError> {
    let transaction = connection.transaction()?;
    transaction.execute_batch(
        "
        ALTER TABLE routes ADD COLUMN service_tier_policy TEXT NOT NULL DEFAULT 'passthrough'
            CHECK (service_tier_policy IN ('passthrough', 'omit'));
        ALTER TABLE upstream_attempts ADD COLUMN forwarded_service_tier TEXT;
        UPDATE upstream_attempts
        SET forwarded_service_tier = (
            SELECT proxy_requests.requested_service_tier
            FROM proxy_requests
            WHERE proxy_requests.request_id = upstream_attempts.request_id
        );
        PRAGMA user_version = 8;
        ",
    )?;
    transaction.commit()?;
    Ok(())
}

fn migrate_v9(connection: &mut Connection) -> Result<(), StorageError> {
    let transaction = connection.transaction()?;
    transaction.execute_batch(
        "
        UPDATE proxy_requests
        SET first_output_latency_ms = first_text_output_latency_ms
        WHERE first_output_latency_ms IS NULL
          AND first_text_output_latency_ms IS NOT NULL;
        DROP INDEX proxy_requests_keyset_idx;
        DROP INDEX proxy_requests_status_keyset_idx;
        DROP INDEX proxy_requests_route_keyset_idx;
        DROP INDEX proxy_requests_model_keyset_idx;
        ALTER TABLE proxy_requests DROP COLUMN first_text_output_latency_ms;
        CREATE INDEX proxy_requests_keyset_idx
            ON proxy_requests(finished_at_ms DESC, request_id DESC)
            WHERE finished_at_ms IS NOT NULL;
        CREATE INDEX proxy_requests_status_keyset_idx
            ON proxy_requests(completion_state, finished_at_ms DESC, request_id DESC)
            WHERE finished_at_ms IS NOT NULL;
        CREATE INDEX proxy_requests_route_keyset_idx
            ON proxy_requests(final_route_id, finished_at_ms DESC, request_id DESC)
            WHERE finished_at_ms IS NOT NULL;
        CREATE INDEX proxy_requests_model_keyset_idx
            ON proxy_requests(COALESCE(actual_model, requested_model), finished_at_ms DESC, request_id DESC)
            WHERE finished_at_ms IS NOT NULL;
        PRAGMA user_version = 9;
        ",
    )?;
    transaction.commit()?;
    Ok(())
}

fn migrate_v10(connection: &mut Connection) -> Result<(), StorageError> {
    let transaction = connection.transaction()?;
    transaction.execute_batch(
        "
        CREATE TABLE codex_models (
            model_id TEXT PRIMARY KEY COLLATE BINARY NOT NULL,
            display_name TEXT,
            context_window INTEGER,
            sort_order INTEGER NOT NULL UNIQUE,
            CHECK (length(trim(model_id)) > 0),
            CHECK (context_window IS NULL OR context_window > 0)
        );
        PRAGMA user_version = 10;
        ",
    )?;
    transaction.commit()?;
    Ok(())
}

fn migrate_v11(connection: &mut Connection) -> Result<(), StorageError> {
    let transaction = connection.transaction()?;
    transaction.execute_batch(
        "
        DROP TABLE codex_models;
        CREATE TABLE codex_models (
            route_id TEXT NOT NULL REFERENCES routes(route_id) ON DELETE CASCADE,
            model_id TEXT COLLATE BINARY NOT NULL,
            display_name TEXT,
            context_window INTEGER,
            sort_order INTEGER NOT NULL,
            PRIMARY KEY (route_id, model_id),
            UNIQUE (route_id, sort_order),
            CHECK (length(trim(model_id)) > 0),
            CHECK (context_window IS NULL OR context_window > 0)
        );
        CREATE TABLE codex_restart_notice (
            singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
            notice_id TEXT NOT NULL UNIQUE CHECK (length(notice_id) > 0),
            route_id TEXT NOT NULL REFERENCES routes(route_id) ON DELETE CASCADE,
            selection_generation INTEGER NOT NULL CHECK (selection_generation >= 0),
            catalog_fingerprint TEXT NOT NULL CHECK (length(catalog_fingerprint) > 0),
            created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0)
        );
        PRAGMA user_version = 11;
        ",
    )?;
    transaction.commit()?;
    Ok(())
}

fn migrate_v12(connection: &mut Connection) -> Result<(), StorageError> {
    let transaction = connection.transaction()?;
    transaction.execute_batch(
        "
        CREATE TABLE balance_queries (
            route_id TEXT PRIMARY KEY REFERENCES routes(route_id) ON DELETE CASCADE,
            mode TEXT NOT NULL CHECK (mode IN ('general_v1', 'custom_js')),
            enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
            custom_source TEXT NOT NULL DEFAULT '',
            updated_at_ms INTEGER NOT NULL,
            CHECK (mode != 'custom_js' OR enabled = 0 OR length(trim(custom_source)) > 0)
        );
        ",
    )?;
    let rows = {
        let mut statement = transaction
            .prepare("SELECT route_id, enabled, source, updated_at_ms FROM balance_scripts")?;
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, bool>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?
    };
    for (route_id, enabled, source, updated_at_ms) in rows {
        let source_hash = hex::encode(Sha256::digest(source.as_bytes()));
        let (mode, custom_source) = if is_general_balance_source_hash(&source_hash) {
            (BalanceQueryMode::GeneralV1, "")
        } else {
            (BalanceQueryMode::CustomJs, source.as_str())
        };
        transaction.execute(
            "INSERT INTO balance_queries (route_id, mode, enabled, custom_source, updated_at_ms) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![route_id, mode.as_str(), enabled, custom_source, updated_at_ms],
        )?;
    }
    transaction.execute_batch(
        "
        DROP TABLE balance_scripts;
        PRAGMA user_version = 12;
        ",
    )?;
    transaction.commit()?;
    Ok(())
}

fn migrate_v13(connection: &mut Connection) -> Result<(), StorageError> {
    let transaction = connection.transaction()?;
    transaction.execute_batch(
        "
        ALTER TABLE fallback_config
            ADD COLUMN participant_count INTEGER NOT NULL DEFAULT 0
            CHECK (participant_count >= 0);
        UPDATE fallback_config
        SET participant_count = MIN(4, (SELECT COUNT(*) FROM routes)),
            enabled = CASE
                WHEN MIN(4, (SELECT COUNT(*) FROM routes)) < 2 THEN 0
                ELSE enabled
            END;
        PRAGMA user_version = 13;
        ",
    )?;
    transaction.commit()?;
    Ok(())
}

fn migrate_v14(connection: &mut Connection) -> Result<(), StorageError> {
    let transaction = connection.transaction()?;
    transaction.execute_batch(
        "
        ALTER TABLE routes
            ADD COLUMN supports_images_generation INTEGER NOT NULL DEFAULT 0
            CHECK (supports_images_generation IN (0, 1));
        ALTER TABLE app_settings
            ADD COLUMN images_generation_enabled INTEGER NOT NULL DEFAULT 0
            CHECK (images_generation_enabled IN (0, 1));
        ALTER TABLE app_settings
            ADD COLUMN images_generation_route_id TEXT DEFAULT NULL
            REFERENCES routes(route_id) ON DELETE SET NULL;
        PRAGMA user_version = 14;
        ",
    )?;
    transaction.commit()?;
    Ok(())
}

fn migrate_v15(connection: &mut Connection) -> Result<(), StorageError> {
    let transaction = connection.transaction()?;
    transaction.execute_batch(
        "
        ALTER TABLE app_settings
            ADD COLUMN images_generation_timeout_secs INTEGER NOT NULL DEFAULT 600
            CHECK (images_generation_timeout_secs BETWEEN 600 AND 3600);
        ALTER TABLE routes DROP COLUMN supports_images_generation;
        PRAGMA user_version = 15;
        ",
    )?;
    transaction.commit()?;
    Ok(())
}

fn migrate_v16(connection: &mut Connection) -> Result<(), StorageError> {
    let transaction = connection.transaction()?;
    transaction.execute_batch(
        "
        ALTER TABLE app_settings ADD COLUMN appearance_preference TEXT NOT NULL DEFAULT 'system'
            CHECK (appearance_preference IN ('system', 'light', 'dark'));
        PRAGMA user_version = 16;
        ",
    )?;
    transaction.commit()?;
    Ok(())
}

fn migrate_v17(connection: &mut Connection) -> Result<(), StorageError> {
    let transaction = connection.transaction()?;
    transaction.execute_batch(
        "
        ALTER TABLE proxy_requests ADD COLUMN fallback_stop_reason TEXT
            CHECK (fallback_stop_reason IS NULL OR fallback_stop_reason IN (
                'fallback_disabled',
                'failure_not_eligible',
                'response_committed',
                'all_participants_attempted',
                'stale_policy',
                'activation_failed',
                'attempt_index_exhausted'
            ));
        ALTER TABLE proxy_requests ADD COLUMN fallback_stop_target_route_id TEXT;
        ALTER TABLE proxy_requests ADD COLUMN fallback_stop_target_route_name TEXT;
        PRAGMA user_version = 17;
        ",
    )?;
    transaction.commit()?;
    Ok(())
}

fn migrate_v18(connection: &mut Connection) -> Result<(), StorageError> {
    let transaction = connection.transaction()?;
    transaction.execute_batch(
        "
        CREATE TABLE codex_recovery_config (
            singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
            original_exists INTEGER NOT NULL CHECK (original_exists IN (0, 1)),
            raw_bytes BLOB,
            unix_mode INTEGER,
            updated_at_ms INTEGER NOT NULL,
            CHECK ((original_exists = 1 AND raw_bytes IS NOT NULL) OR
                   (original_exists = 0 AND raw_bytes IS NULL AND unix_mode IS NULL))
        );
        INSERT INTO codex_recovery_config (singleton, original_exists, raw_bytes, unix_mode, updated_at_ms)
        SELECT singleton, original_exists,
               CASE WHEN original_exists = 1 THEN raw_bytes ELSE NULL END,
               CASE WHEN original_exists = 1 THEN unix_mode ELSE NULL END,
               captured_at_ms
        FROM codex_baseline
        WHERE singleton = 1;
        PRAGMA user_version = 18;
        ",
    )?;
    transaction.commit()?;
    Ok(())
}

fn migrate_v19(connection: &mut Connection) -> Result<(), StorageError> {
    let transaction = connection.transaction()?;
    transaction.execute_batch(
        "
        ALTER TABLE app_settings ADD COLUMN last_automatic_update_check_at_ms INTEGER
            CHECK (last_automatic_update_check_at_ms IS NULL OR last_automatic_update_check_at_ms >= 0);
        PRAGMA user_version = 19;
        ",
    )?;
    transaction.commit()?;
    Ok(())
}

fn migrate_v20(connection: &mut Connection) -> Result<(), StorageError> {
    let transaction = connection.transaction()?;
    transaction.execute_batch(
        "
        CREATE TABLE route_fallback_excluded_models (
            route_id TEXT NOT NULL REFERENCES routes(route_id) ON DELETE CASCADE,
            model_id TEXT COLLATE BINARY NOT NULL,
            sort_order INTEGER NOT NULL CHECK (sort_order >= 0),
            PRIMARY KEY (route_id, model_id),
            UNIQUE (route_id, sort_order),
            CHECK (length(trim(model_id)) > 0)
        );

        ALTER TABLE upstream_attempts ADD COLUMN attempt_role TEXT NOT NULL
            DEFAULT 'ordinary'
            CHECK (attempt_role IN ('ordinary', 'recovery_probe'));
        ALTER TABLE upstream_attempts ADD COLUMN routing_transition_kind TEXT
            CHECK (routing_transition_kind IS NULL OR routing_transition_kind IN (
                'activate_next', 'resume_captured', 'recover'
            ));
        ALTER TABLE upstream_attempts ADD COLUMN routing_transition_target_route_id TEXT;
        ALTER TABLE upstream_attempts ADD COLUMN routing_transition_target_route_name TEXT;

        CREATE TABLE upstream_attempt_routing_skips (
            attempt_id TEXT NOT NULL REFERENCES upstream_attempts(attempt_id) ON DELETE CASCADE,
            skip_order INTEGER NOT NULL CHECK (skip_order >= 0),
            route_id TEXT NOT NULL,
            route_name TEXT NOT NULL,
            reason TEXT NOT NULL CHECK (reason IN ('model_fallback_excluded')),
            PRIMARY KEY (attempt_id, skip_order)
        );

        ALTER TABLE proxy_requests ADD COLUMN fallback_stop_reason_v20 TEXT;
        UPDATE proxy_requests
        SET fallback_stop_reason_v20 = fallback_stop_reason;
        ALTER TABLE proxy_requests DROP COLUMN fallback_stop_reason;
        ALTER TABLE proxy_requests ADD COLUMN fallback_stop_reason TEXT
            CHECK (fallback_stop_reason IS NULL OR fallback_stop_reason IN (
                'fallback_disabled',
                'failure_not_eligible',
                'response_committed',
                'all_participants_attempted',
                'stale_policy',
                'activation_failed',
                'attempt_index_exhausted',
                'failure_threshold_not_reached',
                'failure_threshold_reached_pending',
                'recovery_confirmation_pending',
                'model_fallback_excluded'
            ));
        UPDATE proxy_requests
        SET fallback_stop_reason = fallback_stop_reason_v20;
        ALTER TABLE proxy_requests DROP COLUMN fallback_stop_reason_v20;

        PRAGMA user_version = 20;
        ",
    )?;
    transaction.commit()?;
    Ok(())
}

fn migrate_v21(connection: &mut Connection) -> Result<(), StorageError> {
    let transaction = connection.transaction()?;
    transaction.execute_batch(
        "
        ALTER TABLE app_settings ADD COLUMN menu_bar_status_text_enabled INTEGER NOT NULL DEFAULT 1
            CHECK (menu_bar_status_text_enabled IN (0, 1));
        ALTER TABLE app_settings ADD COLUMN menu_bar_activity_animation_enabled INTEGER NOT NULL DEFAULT 1
            CHECK (menu_bar_activity_animation_enabled IN (0, 1));
        PRAGMA user_version = 21;
        ",
    )?;
    transaction.commit()?;
    Ok(())
}

fn migrate_v22(connection: &mut Connection) -> Result<(), StorageError> {
    let transaction = connection.transaction()?;
    transaction.execute_batch(
        "
        ALTER TABLE app_settings ADD COLUMN mcp_image_capacity_warning_mib INTEGER NOT NULL DEFAULT 1024
            CHECK (mcp_image_capacity_warning_mib BETWEEN 128 AND 102400);
        ALTER TABLE app_settings ADD COLUMN mcp_image_capacity_active_episode TEXT;
        ALTER TABLE app_settings ADD COLUMN mcp_image_capacity_dismissed_episode TEXT;
        PRAGMA user_version = 22;
        ",
    )?;
    transaction.commit()?;
    Ok(())
}

fn parse_persisted_bool(value: i64) -> Result<bool, StorageError> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(StorageError::Initialization),
    }
}

fn read_mcp_image_capacity_settings(
    connection: &Connection,
) -> Result<McpImageCapacitySettingsRecord, StorageError> {
    let (threshold_mib, active_episode_id, dismissed_episode_id): (
        i64,
        Option<String>,
        Option<String>,
    ) = connection.query_row(
        "SELECT mcp_image_capacity_warning_mib, mcp_image_capacity_active_episode, mcp_image_capacity_dismissed_episode FROM app_settings WHERE singleton = 1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    let threshold = McpImageCapacityWarningThreshold::parse(
        u32::try_from(threshold_mib).map_err(|_| StorageError::Initialization)?,
    )?;
    validate_capacity_episodes(
        active_episode_id.as_deref(),
        dismissed_episode_id.as_deref(),
    )?;
    Ok(McpImageCapacitySettingsRecord {
        threshold,
        active_episode_id,
        dismissed_episode_id,
    })
}

fn reconcile_mcp_image_capacity_settings(
    mut current: McpImageCapacitySettingsRecord,
    threshold: McpImageCapacityWarningThreshold,
    observed_bytes: u64,
) -> McpImageCapacitySettingsRecord {
    current.threshold = threshold;
    if observed_bytes < threshold.bytes() {
        current.active_episode_id = None;
        current.dismissed_episode_id = None;
    } else if current.active_episode_id.is_none() {
        current.active_episode_id = Some(Uuid::new_v4().to_string());
        current.dismissed_episode_id = None;
    }
    current
}

fn write_mcp_image_capacity_settings_if_changed(
    transaction: &Transaction<'_>,
    next: &McpImageCapacitySettingsRecord,
) -> Result<bool, StorageError> {
    let current = read_mcp_image_capacity_settings(transaction)?;
    if current == *next {
        return Ok(false);
    }
    transaction.execute(
        "UPDATE app_settings SET mcp_image_capacity_warning_mib = ?1, mcp_image_capacity_active_episode = ?2, mcp_image_capacity_dismissed_episode = ?3 WHERE singleton = 1",
        params![
            i64::from(next.threshold.mebibytes()),
            next.active_episode_id,
            next.dismissed_episode_id,
        ],
    )?;
    Ok(true)
}

fn validate_capacity_episodes(
    active_episode_id: Option<&str>,
    dismissed_episode_id: Option<&str>,
) -> Result<(), StorageError> {
    if active_episode_id.is_some_and(|value| !is_canonical_uuid(value))
        || dismissed_episode_id.is_some_and(|value| !is_canonical_uuid(value))
        || dismissed_episode_id.is_some_and(|dismissed| Some(dismissed) != active_episode_id)
    {
        return Err(StorageError::Initialization);
    }
    Ok(())
}

fn is_canonical_uuid(value: &str) -> bool {
    Uuid::parse_str(value).is_ok_and(|parsed| parsed.hyphenated().to_string() == value)
}

fn read_fallback_config(connection: &Connection) -> Result<ValidatedFallbackConfig, StorageError> {
    let (enabled, participant_count, config_revision, updated_at_ms, route_count): (
        i64,
        i64,
        i64,
        i64,
        i64,
    ) = connection.query_row(
        "SELECT enabled, participant_count, config_revision, updated_at_ms,
                (SELECT COUNT(*) FROM routes)
         FROM fallback_config WHERE singleton = 1",
        [],
        |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        },
    )?;
    let enabled = match enabled {
        0 => false,
        1 => true,
        _ => return Err(StorageError::Initialization),
    };
    let participant_count =
        u32::try_from(participant_count).map_err(|_| StorageError::Initialization)?;
    let config_revision =
        u64::try_from(config_revision).map_err(|_| StorageError::Initialization)?;
    let route_count = u64::try_from(route_count).map_err(|_| StorageError::Initialization)?;
    if u64::from(participant_count) > route_count || (enabled && participant_count < 2) {
        return Err(StorageError::Initialization);
    }
    Ok(ValidatedFallbackConfig {
        record: FallbackConfigRecord {
            enabled,
            participant_count,
            config_revision,
            updated_at_ms,
        },
        route_count,
    })
}

fn mark_critical_change(transaction: &Transaction<'_>) -> Result<u64, StorageError> {
    transaction.execute(
        "UPDATE recovery_revision SET critical_revision = critical_revision + 1, updated_at_ms = ?1 WHERE singleton = 1",
        [now_millis()],
    )?;
    let revision: i64 = transaction.query_row(
        "SELECT critical_revision FROM recovery_revision WHERE singleton = 1",
        [],
        |row| row.get(0),
    )?;
    u64::try_from(revision).map_err(|_| StorageError::Initialization)
}

fn current_critical_revision(connection: &Connection) -> Result<u64, StorageError> {
    let revision: i64 = connection.query_row(
        "SELECT critical_revision FROM recovery_revision WHERE singleton = 1",
        [],
        |row| row.get(0),
    )?;
    u64::try_from(revision).map_err(|_| StorageError::Initialization)
}

fn verify_connection(connection: &Connection) -> Result<(), StorageError> {
    let journal_mode: String =
        connection.pragma_query_value(None, "journal_mode", |row| row.get(0))?;
    let synchronous: i64 = connection.pragma_query_value(None, "synchronous", |row| row.get(0))?;
    let foreign_keys: i64 =
        connection.pragma_query_value(None, "foreign_keys", |row| row.get(0))?;
    let auto_vacuum: i64 = connection.pragma_query_value(None, "auto_vacuum", |row| row.get(0))?;
    let integrity: String =
        connection.pragma_query_value(None, "integrity_check", |row| row.get(0))?;
    let foreign_key_violation: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_foreign_key_check)",
        [],
        |row| row.get(0),
    )?;
    let _fallback = read_fallback_config(connection)?;

    if !journal_mode.eq_ignore_ascii_case("delete")
        || synchronous != 2
        || foreign_keys != 1
        || auto_vacuum != 2
        || integrity != "ok"
        || foreign_key_violation
    {
        return Err(StorageError::Initialization);
    }
    Ok(())
}

fn prepare_database_path(path: &Path) -> Result<(), StorageError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
        set_mode(parent, 0o700)?;
    }
    Ok(())
}

fn enforce_database_file_permissions(path: &Path) -> Result<(), StorageError> {
    if path.exists() {
        set_mode(path, 0o600)?;
    }
    Ok(())
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<(), std::io::Error> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> Result<(), std::io::Error> {
    Ok(())
}

fn now_millis() -> i64 {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
}

const fn completion_state_value(state: &CompletionState) -> &'static str {
    match state {
        CompletionState::NoUpstream => "no_upstream",
        CompletionState::Completed => "completed",
        CompletionState::Failed => "failed",
        CompletionState::Cancelled => "cancelled",
    }
}

fn parse_completion_state(value: &str) -> CompletionState {
    match value {
        "completed" => CompletionState::Completed,
        "failed" => CompletionState::Failed,
        "cancelled" => CompletionState::Cancelled,
        _ => CompletionState::NoUpstream,
    }
}

const fn delivery_state_value(state: &DeliveryState) -> &'static str {
    match state {
        DeliveryState::None => "none",
        DeliveryState::Started => "started",
        DeliveryState::Completed => "completed",
    }
}

fn parse_delivery_state(value: &str) -> DeliveryState {
    match value {
        "completed" => DeliveryState::Completed,
        "started" => DeliveryState::Started,
        _ => DeliveryState::None,
    }
}

fn literal_contains_pattern(value: &str) -> String {
    let mut pattern = String::with_capacity(value.len().saturating_add(2));
    pattern.push('%');
    for character in value.chars() {
        if matches!(character, '\\' | '%' | '_') {
            pattern.push('\\');
        }
        pattern.push(character);
    }
    pattern.push('%');
    pattern
}

#[derive(Clone, Debug, Default)]
struct StatisticsTotals {
    request_count: u64,
    tokens: UsageStatisticsTokens,
    cost_pico_usd: u64,
}

impl StatisticsTotals {
    fn add(&mut self, observation: &StatisticsObservation) -> Result<(), StorageError> {
        self.request_count = self
            .request_count
            .checked_add(1)
            .ok_or(StorageError::UsageStatisticsOverflow)?;
        checked_statistics_add(&mut self.tokens.total, observation.total_tokens)?;
        checked_statistics_add(
            &mut self.tokens.uncached_input,
            statistics_uncached_input(observation.input_tokens, observation.cached_input_tokens),
        )?;
        checked_statistics_add(
            &mut self.tokens.cached_input,
            observation.cached_input_tokens,
        )?;
        checked_statistics_add(
            &mut self.tokens.cache_write_input,
            observation.cache_write_input_tokens,
        )?;
        checked_statistics_add(&mut self.tokens.output, observation.output_tokens)?;
        checked_statistics_add(&mut self.cost_pico_usd, observation.cost_pico_usd)?;
        Ok(())
    }

    fn selected_value(&self, metric: UsageStatisticsAttributionMetric) -> u64 {
        match metric {
            UsageStatisticsAttributionMetric::Requests => self.request_count,
            UsageStatisticsAttributionMetric::Tokens => self.tokens.total,
            UsageStatisticsAttributionMetric::Cost => self.cost_pico_usd,
        }
    }
}

struct StatisticsObservation {
    input_tokens: Option<i64>,
    cached_input_tokens: Option<i64>,
    cache_write_input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    total_tokens: Option<i64>,
    cost_pico_usd: Option<i64>,
}

struct AttributionIdentity {
    key: String,
    label: String,
}

struct AttributionAggregate {
    label: String,
    totals: StatisticsTotals,
}

struct StatisticsBucketWindow {
    started_at_ms: i64,
    finished_at_ms: i64,
    label: String,
}

fn validate_usage_statistics_query(query: &UsageStatisticsQuery) -> Result<(), StorageError> {
    if query.finished_at_or_before_ms < 0
        || query
            .finished_at_or_after_ms
            .is_some_and(|lower| lower < 0 || lower > query.finished_at_or_before_ms)
        || query.time_zone.is_empty()
        || query.time_zone.len() > 128
        || Utc
            .timestamp_millis_opt(query.finished_at_or_before_ms)
            .single()
            .is_none()
        || query
            .finished_at_or_after_ms
            .is_some_and(|lower| Utc.timestamp_millis_opt(lower).single().is_none())
    {
        return Err(StorageError::InvalidUsageQuery);
    }
    if query
        .model_contains
        .as_ref()
        .is_some_and(|model| model.is_empty() || model.len() > 256)
    {
        return Err(StorageError::InvalidUsageQuery);
    }
    Ok(())
}

fn statistics_granularity(query: &UsageStatisticsQuery) -> UsageStatisticsGranularity {
    let Some(lower) = query.finished_at_or_after_ms else {
        return UsageStatisticsGranularity::Month;
    };
    let duration = query.finished_at_or_before_ms.saturating_sub(lower);
    if duration <= 24 * 60 * 60 * 1_000 {
        UsageStatisticsGranularity::Hour
    } else if duration <= 30 * 24 * 60 * 60 * 1_000 {
        UsageStatisticsGranularity::Day
    } else {
        UsageStatisticsGranularity::Month
    }
}

fn checked_statistics_add(target: &mut u64, value: Option<i64>) -> Result<(), StorageError> {
    let Some(value) = value else {
        return Ok(());
    };
    let value = u64::try_from(value).map_err(|_| StorageError::UsageStatisticsOverflow)?;
    *target = target
        .checked_add(value)
        .ok_or(StorageError::UsageStatisticsOverflow)?;
    Ok(())
}

const fn statistics_uncached_input(input: Option<i64>, cached: Option<i64>) -> Option<i64> {
    match (input, cached) {
        (Some(input), Some(cached)) if input >= 0 && cached >= 0 && cached <= input => {
            Some(input - cached)
        }
        _ => None,
    }
}

fn attribution_identity(
    dimension: UsageStatisticsAttributionDimension,
    route_id: Option<String>,
    route_name: Option<String>,
    requested_model: Option<String>,
    actual_model: Option<String>,
) -> AttributionIdentity {
    match dimension {
        UsageStatisticsAttributionDimension::Route => match route_id {
            Some(route_id) => AttributionIdentity {
                key: format!("route:{route_id}"),
                label: route_name.unwrap_or_else(|| "未知路由".to_owned()),
            },
            None => AttributionIdentity {
                key: "route:unknown".to_owned(),
                label: route_name.unwrap_or_else(|| "未知路由".to_owned()),
            },
        },
        UsageStatisticsAttributionDimension::Model => {
            let model = actual_model.or(requested_model);
            AttributionIdentity {
                key: model.as_ref().map_or_else(
                    || "model:unknown".to_owned(),
                    |model| format!("model:{model}"),
                ),
                label: model.unwrap_or_else(|| "未知模型".to_owned()),
            }
        }
    }
}

fn statistics_attribution(
    aggregates: BTreeMap<String, AttributionAggregate>,
    metric: UsageStatisticsAttributionMetric,
    summary: &StatisticsTotals,
) -> Result<Vec<UsageStatisticsAttribution>, StorageError> {
    let total = summary.selected_value(metric);
    let mut values = aggregates
        .into_iter()
        .map(|(key, aggregate)| {
            (
                key,
                aggregate.label,
                aggregate.totals.selected_value(metric),
            )
        })
        .collect::<Vec<_>>();
    values.sort_by(|left, right| {
        right
            .2
            .cmp(&left.2)
            .then_with(|| left.1.cmp(&right.1))
            .then_with(|| left.0.cmp(&right.0))
    });
    let other = if values.len() > 5 {
        let remainder = values.split_off(5);
        Some(
            remainder
                .into_iter()
                .try_fold(0_u64, |sum, (_, _, value)| {
                    sum.checked_add(value)
                        .ok_or(StorageError::UsageStatisticsOverflow)
                })?,
        )
    } else {
        None
    };
    let mut result = values
        .into_iter()
        .map(|(key, label, value)| UsageStatisticsAttribution {
            key,
            label,
            is_other: false,
            value,
            share_percent: statistics_share_percent(value, total),
        })
        .collect::<Vec<_>>();
    if let Some(value) = other {
        result.push(UsageStatisticsAttribution {
            key: "other".to_owned(),
            label: "其他".to_owned(),
            is_other: true,
            value,
            share_percent: statistics_share_percent(value, total),
        });
    }
    Ok(result)
}

fn statistics_share_percent(value: u64, total: u64) -> String {
    if total == 0 {
        return "0".to_owned();
    }
    let tenths = (u128::from(value) * 1_000 + u128::from(total) / 2) / u128::from(total);
    format!("{}.{:01}", tenths / 10, tenths % 10)
}

fn statistics_bucket_windows(
    lower_ms: i64,
    upper_ms: i64,
    time_zone: Tz,
    granularity: UsageStatisticsGranularity,
) -> Result<Vec<StatisticsBucketWindow>, StorageError> {
    let lower = Utc
        .timestamp_millis_opt(lower_ms)
        .single()
        .ok_or(StorageError::InvalidUsageQuery)?;
    let upper = Utc
        .timestamp_millis_opt(upper_ms)
        .single()
        .ok_or(StorageError::InvalidUsageQuery)?;
    let local_lower = lower.with_timezone(&time_zone);
    let local_upper = upper.with_timezone(&time_zone);
    let mut civil = floor_statistics_civil(local_lower, granularity)?;
    let upper_civil = local_upper.naive_local();
    let mut boundaries = vec![lower_ms, upper_ms];
    let mut iterations = 0_u16;
    while civil <= upper_civil {
        boundaries.extend(resolve_statistics_boundary(time_zone, civil, granularity));
        civil = next_statistics_civil(civil, granularity).ok_or(StorageError::InvalidUsageQuery)?;
        iterations = iterations
            .checked_add(1)
            .ok_or(StorageError::InvalidUsageQuery)?;
        if iterations > 2_000 {
            return Err(StorageError::InvalidUsageQuery);
        }
    }
    boundaries.retain(|boundary| *boundary >= lower_ms && *boundary <= upper_ms);
    boundaries.sort_unstable();
    boundaries.dedup();
    if boundaries.len() == 1 {
        return Ok(vec![StatisticsBucketWindow {
            started_at_ms: lower_ms,
            finished_at_ms: upper_ms,
            label: statistics_bucket_label(lower, time_zone, granularity, false),
        }]);
    }
    let duplicate_hour_labels = if granularity == UsageStatisticsGranularity::Hour {
        let mut labels = BTreeMap::<String, u8>::new();
        for boundary in boundaries.iter().take(boundaries.len() - 1) {
            let instant = Utc
                .timestamp_millis_opt(*boundary)
                .single()
                .ok_or(StorageError::InvalidUsageQuery)?;
            let label = statistics_bucket_label(instant, time_zone, granularity, false);
            *labels.entry(label).or_default() += 1;
        }
        labels
    } else {
        BTreeMap::new()
    };
    boundaries
        .windows(2)
        .map(|window| {
            let instant = Utc
                .timestamp_millis_opt(window[0])
                .single()
                .ok_or(StorageError::InvalidUsageQuery)?;
            let base_label = statistics_bucket_label(instant, time_zone, granularity, false);
            let include_offset = duplicate_hour_labels.get(&base_label).copied().unwrap_or(0) > 1;
            Ok(StatisticsBucketWindow {
                started_at_ms: window[0],
                finished_at_ms: window[1],
                label: statistics_bucket_label(instant, time_zone, granularity, include_offset),
            })
        })
        .collect()
}

fn floor_statistics_civil(
    local: DateTime<Tz>,
    granularity: UsageStatisticsGranularity,
) -> Result<NaiveDateTime, StorageError> {
    let date = NaiveDate::from_ymd_opt(local.year(), local.month(), local.day())
        .ok_or(StorageError::InvalidUsageQuery)?;
    match granularity {
        UsageStatisticsGranularity::Hour => date
            .and_hms_opt(local.hour(), 0, 0)
            .ok_or(StorageError::InvalidUsageQuery),
        UsageStatisticsGranularity::Day => date
            .and_hms_opt(0, 0, 0)
            .ok_or(StorageError::InvalidUsageQuery),
        UsageStatisticsGranularity::Month => {
            NaiveDate::from_ymd_opt(local.year(), local.month(), 1)
                .and_then(|date| date.and_hms_opt(0, 0, 0))
                .ok_or(StorageError::InvalidUsageQuery)
        }
    }
}

fn next_statistics_civil(
    value: NaiveDateTime,
    granularity: UsageStatisticsGranularity,
) -> Option<NaiveDateTime> {
    match granularity {
        UsageStatisticsGranularity::Hour => value.checked_add_signed(ChronoDuration::hours(1)),
        UsageStatisticsGranularity::Day => value.checked_add_signed(ChronoDuration::days(1)),
        UsageStatisticsGranularity::Month => {
            let (year, month) = if value.month() == 12 {
                (value.year().checked_add(1)?, 1)
            } else {
                (value.year(), value.month() + 1)
            };
            NaiveDate::from_ymd_opt(year, month, 1)?.and_hms_opt(0, 0, 0)
        }
    }
}

fn resolve_statistics_boundary(
    time_zone: Tz,
    civil: NaiveDateTime,
    granularity: UsageStatisticsGranularity,
) -> Vec<i64> {
    if granularity == UsageStatisticsGranularity::Hour {
        return resolved_boundary_instants(time_zone, civil, true);
    }
    for minute in 0..24 * 60 {
        let Some(candidate) = civil.checked_add_signed(ChronoDuration::minutes(minute)) else {
            return Vec::new();
        };
        if candidate.date() != civil.date() {
            return Vec::new();
        }
        let resolved = resolved_boundary_instants(time_zone, candidate, false);
        if !resolved.is_empty() {
            return resolved;
        }
    }
    Vec::new()
}

fn resolved_boundary_instants(time_zone: Tz, civil: NaiveDateTime, both: bool) -> Vec<i64> {
    let mut values = match time_zone.from_local_datetime(&civil) {
        LocalResult::None => Vec::new(),
        LocalResult::Single(value) => vec![value.with_timezone(&Utc).timestamp_millis()],
        LocalResult::Ambiguous(first, second) if both => vec![
            first.with_timezone(&Utc).timestamp_millis(),
            second.with_timezone(&Utc).timestamp_millis(),
        ],
        LocalResult::Ambiguous(first, second) => {
            vec![first.min(second).with_timezone(&Utc).timestamp_millis()]
        }
    };
    values.sort_unstable();
    values.dedup();
    values
}

fn statistics_bucket_label(
    instant: DateTime<Utc>,
    time_zone: Tz,
    granularity: UsageStatisticsGranularity,
    include_offset: bool,
) -> String {
    let local = instant.with_timezone(&time_zone);
    let local = match granularity {
        UsageStatisticsGranularity::Hour if include_offset => local.format("%m/%d %H:00 %:z"),
        UsageStatisticsGranularity::Hour => local.format("%m/%d %H:00"),
        UsageStatisticsGranularity::Day => local.format("%m/%d"),
        UsageStatisticsGranularity::Month => local.format("%Y/%m"),
    };
    local.to_string()
}

fn usage_history_row(
    row: &rusqlite::Row<'_>,
    actual_service_tier: Option<String>,
) -> rusqlite::Result<UsageHistoryRow> {
    Ok(UsageHistoryRow {
        request_id: row.get(0)?,
        started_at_ms: row.get(1)?,
        finished_at_ms: row.get(2)?,
        final_route_id: row.get::<_, Option<String>>(3)?.map(RouteId::from_string),
        final_route_name: row.get(4)?,
        requested_model: row.get(5)?,
        actual_model: row.get(6)?,
        actual_service_tier,
        reasoning_effort: row.get(7)?,
        streaming: row.get(8)?,
        completion_state: parse_completion_state(&row.get::<_, String>(9)?),
        http_status: row.get(10)?,
        input_tokens: row.get(11)?,
        output_tokens: row.get(12)?,
        total_tokens: row.get(13)?,
        cached_input_tokens: row.get(14)?,
        cache_write_input_tokens: row.get(15)?,
        total_latency_ms: row.get(16)?,
        first_output_latency_ms: row.get(17)?,
        pricing_catalog_version: row.get(18)?,
        cost_status: row
            .get::<_, Option<String>>(19)?
            .and_then(|value| CostStatus::parse(&value)),
        upstream_cost_pico_usd: row.get(20)?,
    })
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{BTreeMap, BTreeSet},
        fs,
        path::Path,
        sync::Arc,
        time::Duration,
    };

    use chrono::{TimeZone, Utc};
    use chrono_tz::Tz;
    use rusqlite::{Connection, params};
    use tempfile::TempDir;

    use super::{
        AttributionAggregate, BalanceQueryInput, CodexModelRecord, CodexRestartNoticeRecord,
        CreateRouteInput, DatabaseExecutor, FallbackStopReason, RoutingDecision, SCHEMA_VERSION,
        SecretStore, SqliteBalanceRouteSource, SqliteSecretStore, StatisticsTotals, StorageError,
        UpdateRouteInput, UsageAttemptDetail, UsageStatisticsAttributionDimension,
        UsageStatisticsAttributionMetric, UsageStatisticsGranularity, UsageStatisticsQuery,
        is_general_balance_source_hash, materialize_routing_decisions, migrate_v1, migrate_v2,
        migrate_v3, migrate_v4, migrate_v5, migrate_v6, migrate_v7, migrate_v8, migrate_v9,
        migrate_v10, migrate_v11, migrate_v12, migrate_v13, migrate_v14, migrate_v15, migrate_v16,
        migrate_v17, migrate_v18, migrate_v19, migrate_v20, migrate_v21, migrate_v22,
        statistics_attribution, statistics_bucket_windows, validate_balance_query,
    };
    use crate::{
        balance::{BalanceQueryMode, BalanceRouteSource, LEGACY_GENERAL_V1_SOURCE},
        domain::{
            ApiKey, AppearancePreference, BalanceQueryPolicy, BaseUrl, ImagesGenerationTimeout,
            McpImageCapacityWarningThreshold, RouteId, RouteMoveDirection, ServiceTierPolicy,
        },
        recovery::{
            NoopRecoveryEventSink, RecoveryCoordinator, RecoveryFailureCode, RecoveryHealthKind,
            RecoveryManager,
        },
    };

    fn database() -> (TempDir, DatabaseExecutor) {
        let directory = tempfile::tempdir().expect("temporary directory");
        let database = DatabaseExecutor::open(directory.path().join("data/router.sqlite3"))
            .expect("database opens");
        (directory, database)
    }

    #[tokio::test]
    async fn menu_bar_settings_default_update_and_noop_are_non_critical() {
        let (_directory, database) = database();
        let settings = database.app_settings().await.expect("settings");
        assert!(settings.menu_bar.status_text_enabled);
        assert!(settings.menu_bar.activity_animation_enabled);
        let revision = database.critical_revision().await.expect("revision");

        assert!(
            database
                .set_menu_bar_settings(false, true)
                .await
                .expect("change")
        );
        assert!(
            !database
                .set_menu_bar_settings(false, true)
                .await
                .expect("no-op")
        );
        assert_eq!(
            database.critical_revision().await.expect("revision"),
            revision
        );
        let settings = database.app_settings().await.expect("updated settings");
        assert!(!settings.menu_bar.status_text_enabled);
        assert!(settings.menu_bar.activity_animation_enabled);
        database
            .test_execute(|connection| {
                connection.pragma_update(None, "ignore_check_constraints", true)?;
                connection.execute(
                    "UPDATE app_settings SET menu_bar_activity_animation_enabled = 2",
                    [],
                )?;
                Ok(())
            })
            .await
            .expect("corrupt fixture");
        assert!(matches!(
            database.app_settings().await,
            Err(StorageError::Initialization)
        ));
    }

    fn migrate_test_database_to_v20(connection: &mut Connection) {
        migrate_v1(connection).expect("v1");
        migrate_v2(connection).expect("v2");
        migrate_v3(connection).expect("v3");
        migrate_v4(connection).expect("v4");
        migrate_v5(connection).expect("v5");
        migrate_v6(connection).expect("v6");
        migrate_v7(connection).expect("v7");
        migrate_v8(connection).expect("v8");
        migrate_v9(connection).expect("v9");
        migrate_v10(connection).expect("v10");
        migrate_v11(connection).expect("v11");
        migrate_v12(connection).expect("v12");
        migrate_v13(connection).expect("v13");
        migrate_v14(connection).expect("v14");
        migrate_v15(connection).expect("v15");
        migrate_v16(connection).expect("v16");
        migrate_v17(connection).expect("v17");
        migrate_v18(connection).expect("v18");
        migrate_v19(connection).expect("v19");
        migrate_v20(connection).expect("v20");
    }

    #[test]
    fn migration_v21_defaults_both_menu_bar_preferences_and_rolls_back_atomically() {
        let mut connection = Connection::open_in_memory().expect("database");
        migrate_test_database_to_v20(&mut connection);
        migrate_v21(&mut connection).expect("v21");
        let values: (i64, i64) = connection
            .query_row(
                "SELECT menu_bar_status_text_enabled, menu_bar_activity_animation_enabled FROM app_settings WHERE singleton = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("menu bar settings");
        assert_eq!(values, (1, 1));
        assert_eq!(
            connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .expect("version"),
            21
        );

        let mut rollback = Connection::open_in_memory().expect("rollback database");
        migrate_test_database_to_v20(&mut rollback);
        rollback
            .execute(
                "ALTER TABLE app_settings ADD COLUMN menu_bar_activity_animation_enabled INTEGER",
                [],
            )
            .expect("collision column");
        assert!(migrate_v21(&mut rollback).is_err());
        assert_eq!(
            rollback
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .expect("version"),
            20
        );
        assert!(
            rollback
                .prepare("SELECT menu_bar_status_text_enabled FROM app_settings")
                .is_err()
        );
    }

    fn migrate_test_database_to_v21(connection: &mut Connection) {
        migrate_test_database_to_v20(connection);
        migrate_v21(connection).expect("v21");
    }

    #[test]
    fn migration_v22_defaults_capacity_policy_and_rolls_back_atomically() {
        let mut connection = Connection::open_in_memory().expect("database");
        migrate_test_database_to_v21(&mut connection);
        migrate_v22(&mut connection).expect("v22");
        let values: (i64, Option<String>, Option<String>) = connection
            .query_row(
                "SELECT mcp_image_capacity_warning_mib, mcp_image_capacity_active_episode, mcp_image_capacity_dismissed_episode FROM app_settings WHERE singleton = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("capacity settings");
        assert_eq!(values, (1_024, None, None));
        assert_eq!(
            connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .expect("version"),
            22
        );

        let mut rollback = Connection::open_in_memory().expect("rollback database");
        migrate_test_database_to_v21(&mut rollback);
        rollback
            .execute(
                "ALTER TABLE app_settings ADD COLUMN mcp_image_capacity_dismissed_episode TEXT",
                [],
            )
            .expect("collision column");
        assert!(migrate_v22(&mut rollback).is_err());
        assert_eq!(
            rollback
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .expect("version"),
            21
        );
        assert!(
            rollback
                .prepare("SELECT mcp_image_capacity_warning_mib FROM app_settings")
                .is_err()
        );
        assert!(
            rollback
                .prepare("SELECT mcp_image_capacity_active_episode FROM app_settings")
                .is_err()
        );
    }

    #[tokio::test]
    async fn capacity_warning_episode_transitions_are_exact_durable_and_non_critical() {
        let (_directory, database) = database();
        let settings = database.app_settings().await.expect("settings");
        assert_eq!(settings.mcp_image_capacity.threshold.mebibytes(), 1_024);
        assert!(!settings.mcp_image_capacity.over_threshold());
        assert!(!settings.mcp_image_capacity.warning_visible());
        let critical_revision = database.critical_revision().await.expect("revision");

        let threshold_bytes = settings.mcp_image_capacity.threshold.bytes();
        let active = database
            .reconcile_mcp_image_capacity(threshold_bytes)
            .await
            .expect("cross threshold");
        let episode = active.active_episode_id.clone().expect("active episode");
        assert!(active.warning_visible());
        assert!(
            !database
                .dismiss_mcp_image_capacity_warning("stale")
                .await
                .expect("stale dismissal")
        );
        assert!(
            database
                .dismiss_mcp_image_capacity_warning(&episode)
                .await
                .expect("exact dismissal")
        );

        let persisted = database.app_settings().await.expect("persisted settings");
        assert_eq!(
            persisted.mcp_image_capacity.active_episode_id.as_deref(),
            Some(episode.as_str())
        );
        assert_eq!(
            persisted.mcp_image_capacity.dismissed_episode_id.as_deref(),
            Some(episode.as_str())
        );
        assert!(!persisted.mcp_image_capacity.warning_visible());
        let still_over = database
            .reconcile_mcp_image_capacity(threshold_bytes + 1)
            .await
            .expect("preserve episode");
        assert_eq!(still_over, persisted.mcp_image_capacity);

        let unavailable_threshold =
            McpImageCapacityWarningThreshold::parse(1_536).expect("threshold");
        let saved_without_summary = database
            .set_mcp_image_capacity_threshold(unavailable_threshold, None)
            .await
            .expect("save without aggregate summary");
        assert_eq!(
            saved_without_summary.active_episode_id.as_deref(),
            Some(episode.as_str())
        );
        assert_eq!(
            saved_without_summary.dismissed_episode_id.as_deref(),
            Some(episode.as_str())
        );
        assert_eq!(saved_without_summary.threshold, unavailable_threshold);

        let raised = McpImageCapacityWarningThreshold::parse(2_048).expect("threshold");
        let below = database
            .set_mcp_image_capacity_threshold(raised, Some(threshold_bytes))
            .await
            .expect("raise threshold");
        assert!(!below.over_threshold());
        let lowered = McpImageCapacityWarningThreshold::parse(512).expect("threshold");
        let new_episode = database
            .set_mcp_image_capacity_threshold(lowered, Some(threshold_bytes))
            .await
            .expect("lower threshold");
        assert!(new_episode.warning_visible());
        assert_ne!(
            new_episode.active_episode_id.as_deref(),
            Some(episode.as_str())
        );
        assert_eq!(
            database.critical_revision().await.expect("revision"),
            critical_revision
        );
    }

    #[tokio::test]
    async fn capacity_settings_fail_closed_on_constraint_bypassed_values() {
        let (_directory, database) = database();
        database
            .test_execute(|connection| {
                connection.pragma_update(None, "ignore_check_constraints", true)?;
                connection.execute(
                    "UPDATE app_settings SET mcp_image_capacity_warning_mib = 127",
                    [],
                )?;
                Ok(())
            })
            .await
            .expect("corrupt threshold");
        assert!(matches!(
            database.app_settings().await,
            Err(StorageError::Validation(_) | StorageError::Initialization)
        ));
    }

    #[tokio::test]
    async fn appearance_preference_defaults_persists_and_noops() {
        let (_directory, database) = database();
        assert_eq!(
            database
                .app_settings()
                .await
                .expect("settings")
                .appearance_preference,
            AppearancePreference::System
        );
        assert!(
            database
                .set_appearance_preference(AppearancePreference::Dark)
                .await
                .expect("change")
        );
        assert!(
            !database
                .set_appearance_preference(AppearancePreference::Dark)
                .await
                .expect("no-op")
        );
        assert_eq!(
            database
                .app_settings()
                .await
                .expect("updated settings")
                .appearance_preference,
            AppearancePreference::Dark
        );
    }

    #[tokio::test]
    async fn appearance_preference_migrates_v15_and_rejects_unknown_values() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("router.sqlite3");
        let mut connection = Connection::open(&path).expect("legacy connection");
        migrate_v1(&mut connection).expect("v1");
        migrate_v2(&mut connection).expect("v2");
        migrate_v3(&mut connection).expect("v3");
        migrate_v4(&mut connection).expect("v4");
        migrate_v5(&mut connection).expect("v5");
        migrate_v6(&mut connection).expect("v6");
        migrate_v7(&mut connection).expect("v7");
        migrate_v8(&mut connection).expect("v8");
        migrate_v9(&mut connection).expect("v9");
        migrate_v10(&mut connection).expect("v10");
        migrate_v11(&mut connection).expect("v11");
        migrate_v12(&mut connection).expect("v12");
        migrate_v13(&mut connection).expect("v13");
        migrate_v14(&mut connection).expect("v14");
        migrate_v15(&mut connection).expect("v15");
        migrate_v16(&mut connection).expect("v16");
        drop(connection);
        let database = DatabaseExecutor::open(&path).expect("current database");
        assert_eq!(
            database
                .app_settings()
                .await
                .expect("settings")
                .appearance_preference,
            AppearancePreference::System
        );
        database
            .test_execute(|connection| {
                connection.pragma_update(None, "ignore_check_constraints", true)?;
                connection.execute(
                    "UPDATE app_settings SET appearance_preference = 'sepia'",
                    [],
                )?;
                Ok(())
            })
            .await
            .expect("corrupt fixture");
        assert!(database.app_settings().await.is_err());
    }

    #[test]
    fn fallback_decision_columns_migrate_from_v16_as_nullable_closed_metadata() {
        let mut connection = Connection::open_in_memory().expect("database");
        migrate_v1(&mut connection).expect("v1");
        migrate_v2(&mut connection).expect("v2");
        migrate_v3(&mut connection).expect("v3");
        migrate_v4(&mut connection).expect("v4");
        migrate_v5(&mut connection).expect("v5");
        migrate_v6(&mut connection).expect("v6");
        migrate_v7(&mut connection).expect("v7");
        migrate_v8(&mut connection).expect("v8");
        migrate_v9(&mut connection).expect("v9");
        migrate_v10(&mut connection).expect("v10");
        migrate_v11(&mut connection).expect("v11");
        migrate_v12(&mut connection).expect("v12");
        migrate_v13(&mut connection).expect("v13");
        migrate_v14(&mut connection).expect("v14");
        migrate_v15(&mut connection).expect("v15");
        migrate_v16(&mut connection).expect("v16");

        migrate_v17(&mut connection).expect("v17");

        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("schema version");
        let columns = connection
            .prepare("PRAGMA table_info(proxy_requests)")
            .expect("table info")
            .query_map([], |row| row.get::<_, String>(1))
            .expect("columns")
            .collect::<Result<Vec<_>, _>>()
            .expect("column names");
        assert_eq!(version, 17);
        assert!(columns.contains(&"fallback_stop_reason".to_owned()));
        assert!(columns.contains(&"fallback_stop_target_route_id".to_owned()));
        assert!(columns.contains(&"fallback_stop_target_route_name".to_owned()));
    }

    fn utc_ms(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> i64 {
        Utc.with_ymd_and_hms(year, month, day, hour, minute, 0)
            .single()
            .expect("valid UTC fixture")
            .timestamp_millis()
    }

    fn statistics_query(lower: Option<i64>, upper: i64, time_zone: &str) -> UsageStatisticsQuery {
        UsageStatisticsQuery {
            finished_at_or_after_ms: lower,
            finished_at_or_before_ms: upper,
            route_id: None,
            model_contains: None,
            time_zone: time_zone.to_owned(),
            attribution_dimension: UsageStatisticsAttributionDimension::Route,
            attribution_metric: UsageStatisticsAttributionMetric::Requests,
        }
    }

    async fn assert_usage_statistics_365k(database: &DatabaseExecutor) {
        let mut query = statistics_query(
            Some(1_700_000_000_000_i64 - 365 * 24 * 60 * 60 * 1_000),
            1_700_000_364_999,
            "UTC",
        );
        query.attribution_metric = UsageStatisticsAttributionMetric::Cost;
        let _ = database
            .usage_statistics(query.clone())
            .await
            .expect("statistics warmup");
        let started = std::time::Instant::now();
        let statistics = database
            .usage_statistics(query)
            .await
            .expect("measured statistics");
        let elapsed = started.elapsed();
        let failed_rows = 365_000_u64.div_ceil(11);
        assert_eq!(statistics.matched_request_count, 365_000 - failed_rows);
        assert_eq!(statistics.granularity, UsageStatisticsGranularity::Month);
        assert_eq!(statistics.attribution.len(), 6);
        eprintln!(
            "V0.3B 365k warm successful-request statistics: {elapsed:?}, total {}",
            statistics.matched_request_count
        );
    }

    fn migrate_test_database_to_v11(connection: &mut Connection) {
        migrate_v1(connection).expect("v1");
        migrate_v2(connection).expect("v2");
        migrate_v3(connection).expect("v3");
        migrate_v4(connection).expect("v4");
        migrate_v5(connection).expect("v5");
        migrate_v6(connection).expect("v6");
        migrate_v7(connection).expect("v7");
        migrate_v8(connection).expect("v8");
        migrate_v9(connection).expect("v9");
        migrate_v10(connection).expect("v10");
        migrate_v11(connection).expect("v11");
    }

    fn migrate_test_database_to_v12(connection: &mut Connection) {
        migrate_test_database_to_v11(connection);
        migrate_v12(connection).expect("v12");
    }

    fn migrate_test_database_to_v13(connection: &mut Connection) {
        migrate_test_database_to_v12(connection);
        migrate_v13(connection).expect("v13");
    }

    fn migrate_test_database_to_v14(connection: &mut Connection) {
        migrate_test_database_to_v13(connection);
        migrate_v14(connection).expect("v14");
    }

    struct MigratedV15Values {
        name: String,
        policy: String,
        sort_order: i64,
        created_at_ms: i64,
        updated_at_ms: i64,
        secret: Vec<u8>,
        images_enabled: i64,
        images_route_id: Option<String>,
        timeout_secs: i64,
        schema_version: i64,
    }

    fn read_migrated_v15_values(connection: &Connection) -> MigratedV15Values {
        connection
            .query_row(
                "SELECT r.display_name, r.service_tier_policy, r.sort_order,
                        r.created_at_ms, r.updated_at_ms, s.value,
                        a.images_generation_enabled, a.images_generation_route_id,
                        a.images_generation_timeout_secs,
                        (SELECT user_version FROM pragma_user_version)
                 FROM routes r
                 JOIN secrets s ON s.secret_id = r.secret_id
                 CROSS JOIN app_settings a
                 WHERE r.route_id = 'image-route' AND a.singleton = 1",
                [],
                |row| {
                    Ok(MigratedV15Values {
                        name: row.get(0)?,
                        policy: row.get(1)?,
                        sort_order: row.get(2)?,
                        created_at_ms: row.get(3)?,
                        updated_at_ms: row.get(4)?,
                        secret: row.get(5)?,
                        images_enabled: row.get(6)?,
                        images_route_id: row.get(7)?,
                        timeout_secs: row.get(8)?,
                        schema_version: row.get(9)?,
                    })
                },
            )
            .expect("migrated values")
    }

    fn insert_v11_balance_row(
        connection: &Connection,
        route_id: &str,
        source: &str,
        enabled: bool,
    ) {
        let secret_id = format!("secret-{route_id}");
        connection
            .execute(
                "INSERT INTO secrets (secret_id, kind, value, created_at_ms, updated_at_ms) VALUES (?1, 'route_api_key', X'01', 1, 1)",
                [secret_id.as_str()],
            )
            .expect("legacy secret");
        connection
            .execute(
                "INSERT INTO routes (route_id, display_name, display_name_key, base_url, secret_id, service_tier_policy, sort_order, created_at_ms, updated_at_ms) VALUES (?1, ?1, ?1, 'https://example.test/v1', ?2, 'passthrough', 0, 1, 1)",
                params![route_id, secret_id],
            )
            .expect("legacy route");
        connection
            .execute(
                "INSERT INTO balance_scripts (route_id, contract_version, enabled, source, updated_at_ms) VALUES (?1, 'general-v1', ?2, ?3, 1)",
                params![route_id, enabled, source],
            )
            .expect("legacy balance script");
    }

    #[test]
    fn v12_migration_recognizes_only_reviewed_hashes_and_preserves_unknown_source() {
        let reviewed_production_inventory = [
            "24cbea85c2fa635112e5915836e2a78144e0a6a21997b86ef5187c2665e14507",
            "24cbea85c2fa635112e5915836e2a78144e0a6a21997b86ef5187c2665e14507",
            "be1d8023ddf04aa987b91d856637eeb86a21a6d504f4475ca2f2945b3132ff6c",
            "f60ff5d32ac946ac0fb8dd616aff15673710f534631464bfb0517833d9170390",
        ];
        assert_eq!(
            reviewed_production_inventory
                .iter()
                .filter(|source_hash| is_general_balance_source_hash(source_hash))
                .count(),
            4
        );

        for source_hash in [
            "24cbea85c2fa635112e5915836e2a78144e0a6a21997b86ef5187c2665e14507",
            "be1d8023ddf04aa987b91d856637eeb86a21a6d504f4475ca2f2945b3132ff6c",
            "f60ff5d32ac946ac0fb8dd616aff15673710f534631464bfb0517833d9170390",
        ] {
            assert!(is_general_balance_source_hash(source_hash));
        }
        assert!(!is_general_balance_source_hash(
            "04d6658dd4df92e556f4c70bd1d26b19732a3145ca27c97b380ecd4243a0c2e1"
        ));

        let mut connection = Connection::open_in_memory().expect("database");
        migrate_test_database_to_v11(&mut connection);
        insert_v11_balance_row(&connection, "stock", LEGACY_GENERAL_V1_SOURCE, true);
        insert_v11_balance_row(
            &connection,
            "stock-disabled",
            LEGACY_GENERAL_V1_SOURCE,
            false,
        );
        let unknown = "(() => ({ request: {}, extractor: () => ({ remaining: 9 }) }))()";
        insert_v11_balance_row(&connection, "unknown", unknown, true);

        migrate_v12(&mut connection).expect("v12");
        let stock: (String, bool, String, i64) = connection
            .query_row(
                "SELECT mode, enabled, custom_source, updated_at_ms FROM balance_queries WHERE route_id = 'stock'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("stock row");
        assert_eq!(stock, ("general_v1".to_owned(), true, String::new(), 1));
        let disabled: (String, bool, String) = connection
            .query_row(
                "SELECT mode, enabled, custom_source FROM balance_queries WHERE route_id = 'stock-disabled'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("disabled stock row");
        assert_eq!(disabled, ("general_v1".to_owned(), false, String::new()));
        let custom: (String, bool, String) = connection
            .query_row(
                "SELECT mode, enabled, custom_source FROM balance_queries WHERE route_id = 'unknown'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("custom row");
        assert_eq!(custom, ("custom_js".to_owned(), true, unknown.to_owned()));
    }

    #[test]
    fn v12_migration_handles_an_empty_legacy_database() {
        let mut connection = Connection::open_in_memory().expect("database");
        migrate_test_database_to_v11(&mut connection);

        migrate_v12(&mut connection).expect("v12");
        let count: i64 = connection
            .query_row("SELECT COUNT(*) FROM balance_queries", [], |row| row.get(0))
            .expect("query count");
        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("schema version");
        assert_eq!(count, 0);
        assert_eq!(version, 12);
    }

    #[test]
    fn balance_query_validation_enforces_mode_and_source_invariants() {
        let disabled_empty = BalanceQueryInput {
            mode: BalanceQueryMode::CustomJs,
            enabled: false,
            custom_source: String::new(),
        };
        assert!(validate_balance_query(Some(disabled_empty)).is_ok());

        let enabled_empty = BalanceQueryInput {
            mode: BalanceQueryMode::CustomJs,
            enabled: true,
            custom_source: String::new(),
        };
        let error =
            validate_balance_query(Some(enabled_empty)).expect_err("custom source required");
        assert_eq!(error.code, "balance_script_required");
        assert_eq!(error.field, "balanceQuery.customSource");

        let retained = BalanceQueryInput {
            mode: BalanceQueryMode::GeneralV1,
            enabled: true,
            custom_source: "({ request: {}, extractor: () => ({ remaining: 1 }) })".to_owned(),
        };
        assert!(validate_balance_query(Some(retained)).is_ok());

        let oversized_retained = BalanceQueryInput {
            mode: BalanceQueryMode::GeneralV1,
            enabled: true,
            custom_source: "x".repeat(crate::domain::MAX_BALANCE_SCRIPT_BYTES + 1),
        };
        assert!(validate_balance_query(Some(oversized_retained)).is_err());
    }

    #[test]
    fn v12_migration_failure_rolls_back_legacy_schema_and_data() {
        let mut connection = Connection::open_in_memory().expect("database");
        migrate_test_database_to_v11(&mut connection);
        insert_v11_balance_row(&connection, "invalid", "", true);

        assert!(migrate_v12(&mut connection).is_err());
        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("schema version");
        assert_eq!(version, 11);
        let source: String = connection
            .query_row(
                "SELECT source FROM balance_scripts WHERE route_id = 'invalid'",
                [],
                |row| row.get(0),
            )
            .expect("legacy row");
        assert!(source.is_empty());
        let replacement_exists: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = 'balance_queries')",
                [],
                |row| row.get(0),
            )
            .expect("replacement table lookup");
        assert!(!replacement_exists);
    }

    #[test]
    fn v13_migration_preserves_the_existing_first_four_participant_range_and_enablement() {
        for (route_count, expected_count) in [(0, 0), (1, 1), (2, 2), (3, 3), (5, 4)] {
            let mut connection = Connection::open_in_memory().expect("database");
            migrate_test_database_to_v12(&mut connection);
            for index in 0..route_count {
                let route_id = format!("route-{index}");
                let secret_id = format!("secret-{index}");
                connection
                    .execute(
                        "INSERT INTO secrets (secret_id, kind, value, created_at_ms, updated_at_ms)
                         VALUES (?1, 'route_api_key', X'01', ?2, ?2)",
                        params![secret_id, index],
                    )
                    .expect("legacy secret");
                connection
                    .execute(
                        "INSERT INTO routes (
                            route_id, display_name, display_name_key, base_url, secret_id,
                            service_tier_policy, sort_order, created_at_ms, updated_at_ms
                         ) VALUES (?1, ?1, ?1, 'https://example.test/v1', ?2,
                                   'passthrough', ?3, ?3, ?3)",
                        params![route_id, secret_id, index],
                    )
                    .expect("legacy route");
            }
            connection
                .execute("UPDATE fallback_config SET enabled = 1", [])
                .expect("legacy enablement");

            migrate_v13(&mut connection).expect("v13");
            let (participant_count, enabled, version): (i64, bool, i64) = connection
                .query_row(
                    "SELECT participant_count, enabled,
                            (SELECT user_version FROM pragma_user_version)
                     FROM fallback_config WHERE singleton = 1",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .expect("migrated fallback config");
            assert_eq!(participant_count, expected_count);
            assert_eq!(enabled, route_count >= 2);
            assert_eq!(version, 13);
        }
    }

    #[test]
    fn v13_migration_failure_rolls_back_the_schema_and_version() {
        let mut connection = Connection::open_in_memory().expect("database");
        migrate_test_database_to_v12(&mut connection);
        connection
            .execute_batch(
                "CREATE TRIGGER reject_v13_fallback_update
                 BEFORE UPDATE ON fallback_config
                 BEGIN
                     SELECT RAISE(ABORT, 'blocked migration');
                 END;",
            )
            .expect("failure trigger");

        assert!(migrate_v13(&mut connection).is_err());
        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("schema version");
        let participant_column_exists: bool = connection
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM pragma_table_info('fallback_config')
                    WHERE name = 'participant_count'
                )",
                [],
                |row| row.get(0),
            )
            .expect("participant column lookup");
        assert_eq!(version, 12);
        assert!(!participant_column_exists);
    }

    #[test]
    fn v14_migration_defaults_image_generation_closed() {
        let mut connection = Connection::open_in_memory().expect("database");
        migrate_test_database_to_v13(&mut connection);
        connection
            .execute(
                "INSERT INTO secrets (secret_id, kind, value, created_at_ms, updated_at_ms)
                 VALUES ('legacy-secret', 'route_api_key', X'01', 1, 1)",
                [],
            )
            .expect("legacy secret");
        connection
            .execute(
                "INSERT INTO routes (
                    route_id, display_name, display_name_key, base_url, secret_id,
                    service_tier_policy, sort_order, created_at_ms, updated_at_ms
                 ) VALUES ('legacy-route', 'Legacy', 'legacy', 'https://example.test/v1',
                           'legacy-secret', 'passthrough', 0, 1, 1)",
                [],
            )
            .expect("legacy route");

        migrate_v14(&mut connection).expect("v14");
        let values: (i64, i64, Option<String>, i64) = connection
            .query_row(
                "SELECT r.supports_images_generation, s.images_generation_enabled,
                        s.images_generation_route_id,
                        (SELECT user_version FROM pragma_user_version)
                 FROM routes r CROSS JOIN app_settings s
                 WHERE r.route_id = 'legacy-route' AND s.singleton = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("migrated image settings");
        assert_eq!(values, (0, 0, None, 14));
        assert!(
            connection
                .execute("UPDATE routes SET supports_images_generation = 2", [])
                .is_err()
        );
        assert!(
            connection
                .execute("UPDATE app_settings SET images_generation_enabled = -1", [])
                .is_err()
        );
    }

    #[test]
    fn v15_migration_preserves_routes_and_image_selection() {
        let mut connection = Connection::open_in_memory().expect("database");
        migrate_test_database_to_v14(&mut connection);
        connection
            .execute(
                "INSERT INTO secrets (secret_id, kind, value, created_at_ms, updated_at_ms)
                 VALUES ('image-secret', 'route_api_key', X'0102', 1, 2)",
                [],
            )
            .expect("legacy secret");
        connection
            .execute(
                "INSERT INTO routes (
                    route_id, display_name, display_name_key, base_url, secret_id,
                    service_tier_policy, sort_order, created_at_ms, updated_at_ms,
                    supports_images_generation
                 ) VALUES ('image-route', 'Images', 'images', 'https://example.test/v1',
                           'image-secret', 'omit', 7, 11, 12, 1)",
                [],
            )
            .expect("legacy route");
        connection
            .execute(
                "UPDATE app_settings
                 SET images_generation_enabled = 1,
                     images_generation_route_id = 'image-route'",
                [],
            )
            .expect("legacy image settings");

        migrate_v15(&mut connection).expect("v15");

        let values = read_migrated_v15_values(&connection);
        assert_eq!(values.name, "Images");
        assert_eq!(values.policy, "omit");
        assert_eq!(
            (
                values.sort_order,
                values.created_at_ms,
                values.updated_at_ms
            ),
            (7, 11, 12)
        );
        assert_eq!(values.secret, vec![1, 2]);
        assert_eq!(
            (
                values.images_enabled,
                values.images_route_id.as_deref(),
                values.timeout_secs,
                values.schema_version,
            ),
            (1, Some("image-route"), 600, 15)
        );
        let capability_exists: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM pragma_table_info('routes') WHERE name = 'supports_images_generation')",
                [],
                |row| row.get(0),
            )
            .expect("capability lookup");
        assert!(!capability_exists);
        assert!(
            connection
                .execute(
                    "UPDATE app_settings SET images_generation_timeout_secs = 599",
                    [],
                )
                .is_err()
        );
    }

    #[test]
    fn v15_migration_rolls_back_both_schema_changes_on_failure() {
        let mut connection = Connection::open_in_memory().expect("database");
        migrate_test_database_to_v14(&mut connection);
        connection
            .execute(
                "CREATE INDEX block_capability_drop ON routes(supports_images_generation)",
                [],
            )
            .expect("blocking index");

        assert!(migrate_v15(&mut connection).is_err());

        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("schema version");
        let columns: (bool, bool) = connection
            .query_row(
                "SELECT
                    EXISTS(SELECT 1 FROM pragma_table_info('routes') WHERE name = 'supports_images_generation'),
                    EXISTS(SELECT 1 FROM pragma_table_info('app_settings') WHERE name = 'images_generation_timeout_secs')",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("column lookup");
        assert_eq!(version, 14);
        assert_eq!(columns, (true, false));
    }

    #[tokio::test]
    async fn image_settings_round_trip_clear_and_skip_no_op_revisions() {
        let (_directory, database) = database();
        database
            .create_route(route("Images", "images-key"))
            .await
            .expect("image route");
        let ordinary = database
            .create_route(route("Ordinary", "ordinary-key"))
            .await
            .expect("ordinary route");
        assert!(matches!(
            database
                .set_images_generation_settings(true, None, ImagesGenerationTimeout::default(),)
                .await,
            Err(StorageError::InvalidImagesGenerationRoute)
        ));
        assert!(matches!(
            database
                .set_images_generation_settings(
                    true,
                    Some(RouteId::from_string("missing-route".to_owned())),
                    ImagesGenerationTimeout::default(),
                )
                .await,
            Err(StorageError::InvalidImagesGenerationRoute)
        ));

        assert!(
            database
                .set_images_generation_settings(
                    true,
                    Some(ordinary.route_id.clone()),
                    ImagesGenerationTimeout::parse(900).expect("timeout"),
                )
                .await
                .expect("enable images")
        );
        let settings = database.app_settings().await.expect("settings");
        assert!(settings.images_generation_enabled);
        assert_eq!(
            settings.images_generation_route_id,
            Some(ordinary.route_id.clone())
        );
        assert_eq!(settings.images_generation_timeout.seconds(), 900);
        let enabled_revision = database.critical_revision().await.expect("revision");
        assert!(
            !database
                .set_images_generation_settings(
                    true,
                    Some(ordinary.route_id.clone()),
                    ImagesGenerationTimeout::parse(900).expect("timeout"),
                )
                .await
                .expect("no-op settings")
        );
        assert_eq!(
            database.critical_revision().await.expect("no-op revision"),
            enabled_revision
        );

        let replacement = database
            .create_route(route("Replacement", "replacement-key"))
            .await
            .expect("replacement route");
        database
            .set_images_generation_settings(
                false,
                Some(replacement.route_id.clone()),
                ImagesGenerationTimeout::parse(1_200).expect("timeout"),
            )
            .await
            .expect("select while disabled");
        database
            .delete_route(replacement.route_id)
            .await
            .expect("delete selected route");
        let settings = database.app_settings().await.expect("delete clearing");
        assert!(!settings.images_generation_enabled);
        assert_eq!(settings.images_generation_route_id, None);
        assert_eq!(settings.images_generation_timeout.seconds(), 1_200);
    }

    #[tokio::test]
    async fn malformed_persisted_image_settings_fail_closed() {
        let (_directory, database) = database();
        database
            .test_execute(move |connection| {
                connection.pragma_update(None, "ignore_check_constraints", true)?;
                connection.execute("UPDATE app_settings SET images_generation_enabled = 2", [])?;
                Ok(())
            })
            .await
            .expect("inject malformed settings boolean");
        assert!(matches!(
            database.app_settings().await,
            Err(StorageError::Initialization)
        ));
        database
            .test_execute(|connection| {
                connection.execute("UPDATE app_settings SET images_generation_enabled = 0, images_generation_timeout_secs = 599", [])?;
                Ok(())
            })
            .await
            .expect("inject malformed timeout");
        assert!(matches!(
            database.app_settings().await,
            Err(StorageError::Validation(_))
        ));
    }

    fn route(name: &str, key: &str) -> CreateRouteInput {
        CreateRouteInput {
            name: name.to_owned(),
            base_url: "https://example.com/v1".to_owned(),
            api_key: ApiKey::parse(key).expect("valid key"),
            service_tier_policy: ServiceTierPolicy::Passthrough,
            balance_query: Some(BalanceQueryInput {
                mode: BalanceQueryMode::CustomJs,
                enabled: true,
                custom_source: "({ request: {}, extractor: () => ({}) })".to_owned(),
            }),
            accept_script_risk: true,
        }
    }

    async fn reorder_routes(
        database: &DatabaseExecutor,
        ordered_route_ids: Vec<RouteId>,
        participant_count: u32,
        expected_config_revision: u64,
    ) -> bool {
        database
            .reorder_routes_and_fallback(
                ordered_route_ids,
                participant_count,
                expected_config_revision,
            )
            .await
            .expect("route reorder")
    }

    #[tokio::test]
    async fn migration_has_recovery_tables_and_required_pragmas() {
        let (_directory, database) = database();
        let (tables, pragmas) = database
            .test_execute(|connection| {
                let mut statement = connection.prepare(
                    "SELECT name FROM sqlite_schema WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
                )?;
                let tables = statement
                    .query_map([], |row| row.get::<_, String>(0))?
                    .collect::<Result<BTreeSet<_>, _>>()?;
                let pragmas = (
                    connection.pragma_query_value(None, "journal_mode", |row| {
                        row.get::<_, String>(0)
                    })?,
                    connection.pragma_query_value(None, "synchronous", |row| row.get::<_, i64>(0))?,
                    connection.pragma_query_value(None, "foreign_keys", |row| row.get::<_, i64>(0))?,
                    connection.pragma_query_value(None, "auto_vacuum", |row| row.get::<_, i64>(0))?,
                );
                Ok((tables, pragmas))
            })
            .await
            .expect("schema query");

        let expected = BTreeSet::from([
            "app_settings".to_owned(),
            "balance_queries".to_owned(),
            "codex_baseline".to_owned(),
            "codex_recovery_config".to_owned(),
            "codex_models".to_owned(),
            "codex_restart_notice".to_owned(),
            "fallback_config".to_owned(),
            "proxy_requests".to_owned(),
            "recovery_point_metadata".to_owned(),
            "recovery_revision".to_owned(),
            "route_fallback_excluded_models".to_owned(),
            "route_state".to_owned(),
            "routes".to_owned(),
            "secrets".to_owned(),
            "upstream_attempt_routing_skips".to_owned(),
            "upstream_attempts".to_owned(),
        ]);
        assert_eq!(tables, expected);
        assert_eq!(pragmas, ("delete".to_owned(), 2, 1, 2));
        let settings = database.app_settings().await.expect("settings");
        assert_eq!(settings.balance_query_policy, BalanceQueryPolicy::default());
    }

    #[tokio::test]
    async fn migration_from_v3_adds_recovery_revision_without_losing_settings() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("router.sqlite3");
        let mut connection = Connection::open(&path).expect("legacy connection");
        migrate_v1(&mut connection).expect("v1 migration");
        migrate_v2(&mut connection).expect("v2 migration");
        migrate_v3(&mut connection).expect("v3 migration");
        drop(connection);

        let database = DatabaseExecutor::open(path).expect("migrated database");
        let settings = database.app_settings().await.expect("settings");
        assert_eq!(settings.balance_query_policy, BalanceQueryPolicy::default());
        let version = database
            .test_execute(|connection| {
                Ok(connection
                    .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))?)
            })
            .await
            .expect("schema version");
        assert_eq!(version, SCHEMA_VERSION);
        assert_eq!(database.critical_revision().await.expect("revision"), 0);
    }

    #[tokio::test]
    async fn codex_models_replace_transactionally_and_round_trip_explicit_order() {
        let (_directory, database) = database();
        let route = database
            .create_route(route("Models", "models-key"))
            .await
            .expect("route");
        let models = vec![
            CodexModelRecord {
                model_id: "  Relay-B  ".to_owned(),
                display_name: Some("  Relay B  ".to_owned()),
                context_window: Some(200_000),
            },
            CodexModelRecord {
                model_id: "relay-a".to_owned(),
                display_name: Some("  ".to_owned()),
                context_window: None,
            },
        ];
        let saved = database
            .replace_codex_models(route.route_id.clone(), models)
            .await
            .expect("replace");
        assert_eq!(saved[0].model_id, "Relay-B");
        assert_eq!(saved[0].display_name.as_deref(), Some("Relay B"));
        assert_eq!(saved[1].display_name, None);
        assert_eq!(
            database
                .list_codex_models(route.route_id.clone())
                .await
                .expect("load"),
            saved
        );
        let revision = database.critical_revision().await.expect("revision");
        database
            .replace_codex_models(route.route_id.clone(), saved.clone())
            .await
            .expect("no-op replace");
        assert_eq!(
            database.critical_revision().await.expect("revision"),
            revision
        );

        let invalid = vec![saved[0].clone(), saved[0].clone()];
        let error = database
            .replace_codex_models(route.route_id.clone(), invalid)
            .await
            .expect_err("duplicate");
        assert!(matches!(error, StorageError::CodexModelValidation(_)));
        assert_eq!(
            database
                .list_codex_models(route.route_id.clone())
                .await
                .expect("unchanged"),
            saved
        );
    }

    #[tokio::test]
    async fn codex_model_ids_are_case_sensitive_and_blank_context_stays_absent() {
        let (_directory, database) = database();
        let route = database
            .create_route(route("Models", "models-key"))
            .await
            .expect("route");
        database
            .replace_codex_models(
                route.route_id.clone(),
                vec![
                    CodexModelRecord {
                        model_id: "Relay".to_owned(),
                        display_name: None,
                        context_window: None,
                    },
                    CodexModelRecord {
                        model_id: "relay".to_owned(),
                        display_name: None,
                        context_window: Some(128_000),
                    },
                ],
            )
            .await
            .expect("case-sensitive IDs");
        let loaded = database
            .list_codex_models(route.route_id)
            .await
            .expect("load");
        assert_eq!(loaded[0].context_window, None);
        assert_eq!(loaded[1].context_window, Some(128_000));
    }

    #[tokio::test]
    async fn fallback_excluded_models_round_trip_in_order_and_reject_duplicates_atomically() {
        let (_directory, database) = database();
        let route = database
            .create_route_with_models_and_fallback_exclusions(
                route("Fallback", "fallback-key"),
                Vec::new(),
                vec![" luna ".to_owned(), "sol".to_owned()],
            )
            .await
            .expect("route with exclusions");
        assert_eq!(
            database
                .list_fallback_excluded_models(route.route_id.clone())
                .await
                .expect("stored exclusions"),
            vec!["luna".to_owned(), "sol".to_owned()]
        );

        let error = database
            .update_route_with_models_and_fallback_exclusions(
                UpdateRouteInput {
                    route_id: route.route_id.clone(),
                    name: "Fallback renamed".to_owned(),
                    base_url: "https://changed.example/v1".to_owned(),
                    api_key: ApiKey::parse("changed-key").expect("key"),
                    service_tier_policy: ServiceTierPolicy::Passthrough,
                    balance_query: None,
                    accept_script_risk: false,
                },
                Vec::new(),
                vec!["duplicate".to_owned(), " duplicate ".to_owned()],
            )
            .await
            .expect_err("duplicate exclusion");
        assert!(matches!(
            error,
            StorageError::FallbackExcludedModelValidation(_)
        ));
        assert_eq!(
            database
                .list_fallback_excluded_models(route.route_id)
                .await
                .expect("unchanged exclusions"),
            vec!["luna".to_owned(), "sol".to_owned()]
        );
    }

    #[tokio::test]
    async fn route_update_reports_exact_no_op_without_critical_change() {
        let (_directory, database) = database();
        let route = database
            .create_route_with_models_and_fallback_exclusions(
                route("Fallback", "fallback-key"),
                Vec::new(),
                vec!["luna".to_owned(), "sol".to_owned()],
            )
            .await
            .expect("route with exclusions");
        let revision = database.critical_revision().await.expect("revision");

        let changed = database
            .update_route_with_models_and_fallback_exclusions(
                UpdateRouteInput {
                    route_id: route.route_id,
                    name: "Fallback".to_owned(),
                    base_url: "https://example.com/v1".to_owned(),
                    api_key: ApiKey::parse("fallback-key").expect("key"),
                    service_tier_policy: ServiceTierPolicy::Passthrough,
                    balance_query: Some(BalanceQueryInput {
                        mode: BalanceQueryMode::CustomJs,
                        enabled: true,
                        custom_source: "({ request: {}, extractor: () => ({}) })".to_owned(),
                    }),
                    accept_script_risk: false,
                },
                Vec::new(),
                vec!["luna".to_owned(), "sol".to_owned()],
            )
            .await
            .expect("route no-op");

        assert!(!changed);
        assert_eq!(
            database.critical_revision().await.expect("no-op revision"),
            revision
        );
    }

    #[tokio::test]
    async fn route_owned_codex_models_are_isolated_and_invalid_updates_are_atomic() {
        let (_directory, database) = database();
        let first = database
            .create_route_with_models(
                route("First", "first-key"),
                vec![CodexModelRecord {
                    model_id: "first-model".to_owned(),
                    display_name: None,
                    context_window: None,
                }],
            )
            .await
            .expect("first route");
        let second = database
            .create_route_with_models(
                route("Second", "second-key"),
                vec![CodexModelRecord {
                    model_id: "second-model".to_owned(),
                    display_name: Some("Second".to_owned()),
                    context_window: Some(128_000),
                }],
            )
            .await
            .expect("second route");

        assert_eq!(
            database
                .list_codex_models(first.route_id.clone())
                .await
                .expect("first models")[0]
                .model_id,
            "first-model"
        );
        assert_eq!(
            database
                .list_codex_models(second.route_id)
                .await
                .expect("second models")[0]
                .model_id,
            "second-model"
        );

        let duplicate = CodexModelRecord {
            model_id: "duplicate".to_owned(),
            display_name: None,
            context_window: None,
        };
        let update = UpdateRouteInput {
            route_id: first.route_id.clone(),
            name: "Renamed".to_owned(),
            base_url: "https://changed.example/v1".to_owned(),
            api_key: ApiKey::parse("changed-key").expect("key"),
            service_tier_policy: ServiceTierPolicy::Omit,
            balance_query: None,
            accept_script_risk: false,
        };
        let error = database
            .update_route_with_models(update, vec![duplicate.clone(), duplicate])
            .await
            .expect_err("duplicate models reject the complete update");
        assert!(matches!(error, StorageError::CodexModelValidation(_)));
        let unchanged = database
            .route_edit(first.route_id.clone())
            .await
            .expect("unchanged edit");
        assert_eq!(unchanged.route.name.as_str(), "First");
        assert_eq!(unchanged.models[0].model_id, "first-model");
    }

    #[tokio::test]
    async fn restart_notice_is_generation_bound_and_dismissal_is_id_scoped() {
        let (_directory, database) = database();
        let first = database
            .create_route(route("First", "first-key"))
            .await
            .expect("first route");
        let routing = database.routing_state().await.expect("routing");
        let notice = CodexRestartNoticeRecord {
            notice_id: "notice-current".to_owned(),
            route_id: first.route_id.clone(),
            selection_generation: routing.selection_generation,
            catalog_fingerprint: "catalog-a".to_owned(),
            created_at_ms: 1,
        };
        assert!(
            database
                .upsert_codex_restart_notice(notice.clone())
                .await
                .expect("current notice")
        );
        assert!(
            !database
                .upsert_codex_restart_notice(CodexRestartNoticeRecord {
                    notice_id: "notice-stale".to_owned(),
                    selection_generation: routing.selection_generation + 1,
                    ..notice.clone()
                })
                .await
                .expect("stale notice")
        );
        assert_eq!(
            database
                .codex_restart_notice()
                .await
                .expect("persisted notice")
                .expect("notice")
                .notice_id,
            "notice-current"
        );
        assert!(
            !database
                .dismiss_codex_restart_notice("different-notice".to_owned())
                .await
                .expect("wrong dismissal")
        );
        assert!(
            database
                .dismiss_codex_restart_notice("notice-current".to_owned())
                .await
                .expect("matching dismissal")
        );
        assert_eq!(
            database
                .codex_restart_notice()
                .await
                .expect("dismissed notice"),
            None
        );
    }

    #[tokio::test]
    async fn v10_global_codex_models_are_discarded_by_v11_migration() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("router.sqlite3");
        let mut connection = Connection::open(&path).expect("legacy connection");
        migrate_v1(&mut connection).expect("v1");
        migrate_v2(&mut connection).expect("v2");
        migrate_v3(&mut connection).expect("v3");
        migrate_v4(&mut connection).expect("v4");
        migrate_v5(&mut connection).expect("v5");
        migrate_v6(&mut connection).expect("v6");
        migrate_v7(&mut connection).expect("v7");
        migrate_v8(&mut connection).expect("v8");
        migrate_v9(&mut connection).expect("v9");
        migrate_v10(&mut connection).expect("v10");
        connection
            .execute(
                "INSERT INTO codex_models (model_id, display_name, context_window, sort_order) VALUES ('legacy-global', 'Legacy', 128000, 0)",
                [],
            )
            .expect("legacy global model");
        drop(connection);

        let database = DatabaseExecutor::open(path).expect("current database");
        let (version, model_count, has_route_id): (i64, i64, bool) = database
            .test_execute(|connection| {
                let version =
                    connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
                let model_count =
                    connection
                        .query_row("SELECT COUNT(*) FROM codex_models", [], |row| row.get(0))?;
                let has_route_id = connection
                    .prepare("SELECT route_id FROM codex_models")
                    .is_ok();
                Ok((version, model_count, has_route_id))
            })
            .await
            .expect("migrated schema");
        assert_eq!(version, SCHEMA_VERSION);
        assert_eq!(model_count, 0);
        assert!(has_route_id);
    }

    #[tokio::test]
    async fn critical_notifications_follow_commits_and_skip_no_ops_and_history() {
        let (_directory, database) = database();
        let mut revisions = database.subscribe_critical_revisions();
        let created = database
            .create_route(route("Work", "first-key"))
            .await
            .expect("route");
        revisions.changed().await.expect("committed revision");
        assert_eq!(*revisions.borrow_and_update(), 1);
        assert_eq!(database.critical_revision().await.expect("durable"), 1);

        database
            .activate_route(created.route_id)
            .await
            .expect("same route is a no-op");
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), revisions.changed())
                .await
                .is_err()
        );
        assert_eq!(database.critical_revision().await.expect("unchanged"), 1);

        let secrets = SqliteSecretStore::new(database.clone());
        let gateway = secrets
            .put(
                "gateway_token".to_owned(),
                ApiKey::parse("gateway-one").expect("gateway"),
            )
            .await
            .expect("insert gateway");
        revisions.changed().await.expect("secret insert revision");
        assert_eq!(*revisions.borrow_and_update(), 2);
        secrets
            .replace(
                gateway.clone(),
                ApiKey::parse("gateway-one").expect("same gateway"),
            )
            .await
            .expect("same secret is a no-op");
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), revisions.changed())
                .await
                .is_err()
        );
        secrets
            .replace(
                gateway.clone(),
                ApiKey::parse("gateway-two").expect("replacement gateway"),
            )
            .await
            .expect("replace secret");
        revisions.changed().await.expect("secret replace revision");
        assert_eq!(*revisions.borrow_and_update(), 3);
        secrets.delete(gateway).await.expect("delete gateway");
        revisions.changed().await.expect("secret delete revision");
        assert_eq!(*revisions.borrow_and_update(), 4);
    }

    #[tokio::test]
    #[expect(
        clippy::too_many_lines,
        reason = "the regression traces custom, general, retained-source, revision, and disable states"
    )]
    async fn storage_balance_source_tracks_exact_route_revision_and_enabled_state() {
        let (_directory, database) = database();
        let created = database
            .create_route(route("Work", "first-key"))
            .await
            .expect("route");
        let source = SqliteBalanceRouteSource::new(database.clone());
        let first = source
            .load_enabled_route(&created.route_id)
            .await
            .expect("balance source")
            .expect("enabled route");
        assert_eq!(first.api_key.expose(), b"first-key");
        assert_eq!(
            source.eligible_route_ids().await.expect("eligible"),
            vec![created.route_id.clone()]
        );

        database
            .update_route(UpdateRouteInput {
                route_id: created.route_id.clone(),
                name: "Work".to_owned(),
                base_url: "https://example.com/v1".to_owned(),
                api_key: ApiKey::parse("second-key").expect("key"),
                service_tier_policy: ServiceTierPolicy::Passthrough,
                balance_query: Some(BalanceQueryInput {
                    mode: BalanceQueryMode::CustomJs,
                    enabled: true,
                    custom_source: "({ request: { method: \"GET\" }, extractor: () => ({}) })"
                        .to_owned(),
                }),
                accept_script_risk: true,
            })
            .await
            .expect("update route");
        let second = source
            .load_enabled_route(&created.route_id)
            .await
            .expect("balance source")
            .expect("enabled route");
        assert_eq!(second.api_key.expose(), b"second-key");
        assert_ne!(first.query_revision, second.query_revision);
        assert!(
            !source
                .is_current(&created.route_id, first.query_revision)
                .await
        );

        database
            .update_route(UpdateRouteInput {
                route_id: created.route_id.clone(),
                name: "Work".to_owned(),
                base_url: "https://example.com/v1".to_owned(),
                api_key: ApiKey::parse("second-key").expect("key"),
                service_tier_policy: ServiceTierPolicy::Passthrough,
                balance_query: Some(BalanceQueryInput {
                    mode: BalanceQueryMode::GeneralV1,
                    enabled: true,
                    custom_source: "({ request: {}, extractor: () => ({ remaining: 1 }) })"
                        .to_owned(),
                }),
                accept_script_risk: false,
            })
            .await
            .expect("switch to general query");
        let general = source
            .load_enabled_route(&created.route_id)
            .await
            .expect("balance source")
            .expect("enabled route");
        assert_ne!(second.query_revision, general.query_revision);

        database
            .update_route(UpdateRouteInput {
                route_id: created.route_id.clone(),
                name: "Work".to_owned(),
                base_url: "https://example.com/v1".to_owned(),
                api_key: ApiKey::parse("second-key").expect("key"),
                service_tier_policy: ServiceTierPolicy::Passthrough,
                balance_query: Some(BalanceQueryInput {
                    mode: BalanceQueryMode::GeneralV1,
                    enabled: true,
                    custom_source: "({ request: {}, extractor: () => ({ remaining: 2 }) })"
                        .to_owned(),
                }),
                accept_script_risk: false,
            })
            .await
            .expect("edit inactive custom draft");
        let retained_edit = source
            .load_enabled_route(&created.route_id)
            .await
            .expect("balance source")
            .expect("enabled route");
        assert_eq!(general.query_revision, retained_edit.query_revision);
        assert_eq!(retained_edit.query.mode, BalanceQueryMode::GeneralV1);
        assert_eq!(
            retained_edit.query.custom_source,
            "({ request: {}, extractor: () => ({ remaining: 2 }) })"
        );

        database
            .update_route(UpdateRouteInput {
                route_id: created.route_id.clone(),
                name: "Work".to_owned(),
                base_url: "https://example.com/v1".to_owned(),
                api_key: ApiKey::parse("second-key").expect("key"),
                service_tier_policy: ServiceTierPolicy::Passthrough,
                balance_query: Some(BalanceQueryInput {
                    mode: BalanceQueryMode::GeneralV1,
                    enabled: false,
                    custom_source: retained_edit.query.custom_source,
                }),
                accept_script_risk: false,
            })
            .await
            .expect("disable script");
        assert!(
            source
                .load_enabled_route(&created.route_id)
                .await
                .expect("balance source")
                .is_none()
        );
        assert!(
            source
                .eligible_route_ids()
                .await
                .expect("eligible")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn storage_app_settings_persist_only_the_proxy_port() {
        let (_directory, database) = database();
        let initial = database.app_settings().await.expect("settings");
        assert_eq!(initial.proxy_port, 32_189);
        assert!(!initial.first_run_presented);
        assert!(!initial.balance_script_risk_confirmed);

        database.set_proxy_port(32_190).await.expect("set port");
        let changed = database.app_settings().await.expect("settings");
        assert_eq!(changed.proxy_port, 32_190);
        assert_eq!(changed.first_run_presented, initial.first_run_presented);
        assert_eq!(
            changed.balance_script_risk_confirmed,
            initial.balance_script_risk_confirmed
        );
        assert_eq!(changed.balance_query_policy, initial.balance_query_policy);
    }

    #[tokio::test]
    async fn application_update_cadence_is_non_critical_and_fails_closed() {
        let (_directory, database) = database();
        let initial_revision = database.critical_revision().await.expect("revision");
        assert_eq!(
            database
                .app_settings()
                .await
                .expect("settings")
                .last_automatic_update_check_at_ms,
            None
        );

        database
            .set_last_automatic_update_check_at_ms(1_725_000_000_000)
            .await
            .expect("persist cadence");
        assert_eq!(
            database
                .app_settings()
                .await
                .expect("updated settings")
                .last_automatic_update_check_at_ms,
            Some(1_725_000_000_000)
        );
        assert_eq!(
            database
                .critical_revision()
                .await
                .expect("revision unchanged"),
            initial_revision
        );
        assert!(
            database
                .set_last_automatic_update_check_at_ms(-1)
                .await
                .is_err()
        );

        database
            .test_execute(|connection| {
                connection.pragma_update(None, "ignore_check_constraints", true)?;
                connection.execute(
                    "UPDATE app_settings SET last_automatic_update_check_at_ms = -1",
                    [],
                )?;
                Ok(())
            })
            .await
            .expect("corrupt fixture");
        assert!(database.app_settings().await.is_err());
    }

    #[test]
    fn migration_v19_adds_nullable_application_update_cadence() {
        let mut connection = Connection::open_in_memory().expect("database");
        migrate_v1(&mut connection).expect("v1");
        migrate_v2(&mut connection).expect("v2");
        migrate_v3(&mut connection).expect("v3");
        migrate_v4(&mut connection).expect("v4");
        migrate_v5(&mut connection).expect("v5");
        migrate_v6(&mut connection).expect("v6");
        migrate_v7(&mut connection).expect("v7");
        migrate_v8(&mut connection).expect("v8");
        migrate_v9(&mut connection).expect("v9");
        migrate_v10(&mut connection).expect("v10");
        migrate_v11(&mut connection).expect("v11");
        migrate_v12(&mut connection).expect("v12");
        migrate_v13(&mut connection).expect("v13");
        migrate_v14(&mut connection).expect("v14");
        migrate_v15(&mut connection).expect("v15");
        migrate_v16(&mut connection).expect("v16");
        migrate_v17(&mut connection).expect("v17");
        migrate_v18(&mut connection).expect("v18");

        migrate_v19(&mut connection).expect("v19");

        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("version");
        let value: Option<i64> = connection
            .query_row(
                "SELECT last_automatic_update_check_at_ms FROM app_settings WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .expect("cadence");
        assert_eq!(version, 19);
        assert_eq!(value, None);
    }

    #[derive(Debug, Eq, PartialEq)]
    struct MigratedV20Route {
        name: String,
        base_url: String,
        sort_order: i64,
        created_at_ms: i64,
        updated_at_ms: i64,
    }

    #[derive(Debug, Eq, PartialEq)]
    struct MigratedV20Request {
        started_at_ms: i64,
        finished_at_ms: Option<i64>,
        requested_model: Option<String>,
        actual_model: Option<String>,
        final_route_id: Option<String>,
        final_route_name: Option<String>,
        completion_state: String,
        http_status: Option<u16>,
        error_category: Option<String>,
        total_tokens: Option<i64>,
        cost_pico_usd: Option<i64>,
        fallback_stop_reason: Option<String>,
    }

    #[derive(Debug, Eq, PartialEq)]
    struct MigratedV20Attempt {
        attempt_id: String,
        request_id: String,
        attempt_index: u32,
        route_id: String,
        route_name: String,
        started_at_ms: i64,
        finished_at_ms: Option<i64>,
        http_status: Option<u16>,
        error_category: Option<String>,
        delivery_state: String,
        total_tokens: Option<i64>,
        cost_pico_usd: Option<i64>,
    }

    #[derive(Debug, Eq, PartialEq)]
    struct MigratedV20Values {
        version: i64,
        tables: Vec<String>,
        route: MigratedV20Route,
        request: MigratedV20Request,
        attempt: MigratedV20Attempt,
        transition: (String, Option<String>, Option<String>, Option<String>),
        exclusions: i64,
        skips: i64,
    }

    fn prepare_v19_fallback_history_fixture(path: &Path) {
        let mut connection = Connection::open(path).expect("v19 database");
        migrate_test_database_to_v14(&mut connection);
        migrate_v15(&mut connection).expect("v15");
        migrate_v16(&mut connection).expect("v16");
        migrate_v17(&mut connection).expect("v17");
        migrate_v18(&mut connection).expect("v18");
        migrate_v19(&mut connection).expect("v19");
        connection
            .execute_batch(
                "
                INSERT INTO secrets (secret_id, kind, value, created_at_ms, updated_at_ms)
                VALUES ('secret-1', 'route_api_key', X'01', 1, 2);
                INSERT INTO routes (
                    route_id, display_name, display_name_key, base_url, secret_id,
                    service_tier_policy, sort_order, created_at_ms, updated_at_ms
                ) VALUES (
                    'route-1', 'Route', 'route', 'https://example.invalid/v1',
                    'secret-1', 'passthrough', 0, 3, 4
                );
                INSERT INTO proxy_requests (
                    request_id, started_at_ms, finished_at_ms, requested_model, actual_model,
                    final_route_id, final_route_name, streaming, completion_state, http_status,
                    error_category, input_tokens, output_tokens, total_tokens, total_latency_ms,
                    first_output_latency_ms, metadata_complete, requested_service_tier,
                    actual_service_tier, cached_input_tokens, cache_write_input_tokens,
                    pricing_catalog_version, cost_status, upstream_cost_pico_usd,
                    reasoning_effort, fallback_stop_reason
                ) VALUES (
                    'request-1', 10, 40, 'luna', 'luna-actual', 'route-1', 'Route',
                    0, 'failed', 503, 'upstream_http_status', 11, 7, 18, 30, 9, 1,
                    'priority', 'default', 3, 2, 'catalog-v1', 'partial', 1234,
                    'high', 'all_participants_attempted'
                );
                INSERT INTO upstream_attempts (
                    attempt_id, request_id, attempt_index, route_id, route_name,
                    started_at_ms, finished_at_ms, http_status, error_category,
                    delivery_state, input_tokens, output_tokens, total_tokens, actual_model,
                    forwarded_service_tier, actual_service_tier, cached_input_tokens,
                    cache_write_input_tokens, pricing_catalog_version, cost_status,
                    cost_pico_usd
                ) VALUES (
                    'attempt-1', 'request-1', 0, 'route-1', 'Route', 10, 40, 503,
                    'upstream_http_status', 'completed', 11, 7, 18, 'luna-actual',
                    'priority', 'default', 3, 2, 'catalog-v1', 'partial', 1234
                );
                ",
            )
            .expect("v19 rows");
        assert_eq!(
            connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .expect("v19 version"),
            19
        );
    }

    fn read_migrated_v20_request(
        connection: &Connection,
    ) -> Result<MigratedV20Request, StorageError> {
        Ok(connection.query_row(
            "SELECT started_at_ms, finished_at_ms, requested_model, actual_model,
                    final_route_id, final_route_name, completion_state, http_status,
                    error_category, total_tokens, upstream_cost_pico_usd,
                    fallback_stop_reason
             FROM proxy_requests WHERE request_id = 'request-1'",
            [],
            |row| {
                Ok(MigratedV20Request {
                    started_at_ms: row.get(0)?,
                    finished_at_ms: row.get(1)?,
                    requested_model: row.get(2)?,
                    actual_model: row.get(3)?,
                    final_route_id: row.get(4)?,
                    final_route_name: row.get(5)?,
                    completion_state: row.get(6)?,
                    http_status: row.get(7)?,
                    error_category: row.get(8)?,
                    total_tokens: row.get(9)?,
                    cost_pico_usd: row.get(10)?,
                    fallback_stop_reason: row.get(11)?,
                })
            },
        )?)
    }

    fn read_migrated_v20_attempt(
        connection: &Connection,
    ) -> Result<MigratedV20Attempt, StorageError> {
        Ok(connection.query_row(
            "SELECT attempt_id, request_id, attempt_index, route_id, route_name,
                    started_at_ms, finished_at_ms, http_status, error_category,
                    delivery_state, total_tokens, cost_pico_usd
             FROM upstream_attempts WHERE attempt_id = 'attempt-1'",
            [],
            |row| {
                Ok(MigratedV20Attempt {
                    attempt_id: row.get(0)?,
                    request_id: row.get(1)?,
                    attempt_index: row.get(2)?,
                    route_id: row.get(3)?,
                    route_name: row.get(4)?,
                    started_at_ms: row.get(5)?,
                    finished_at_ms: row.get(6)?,
                    http_status: row.get(7)?,
                    error_category: row.get(8)?,
                    delivery_state: row.get(9)?,
                    total_tokens: row.get(10)?,
                    cost_pico_usd: row.get(11)?,
                })
            },
        )?)
    }

    fn read_migrated_v20_values(
        connection: &Connection,
    ) -> Result<MigratedV20Values, StorageError> {
        let mut table_query = connection.prepare(
            "SELECT name FROM sqlite_master
             WHERE type = 'table' AND name IN (
                 'route_fallback_excluded_models',
                 'upstream_attempt_routing_skips'
             ) ORDER BY name",
        )?;
        let tables = table_query
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(MigratedV20Values {
            version: connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))?,
            tables,
            route: connection.query_row(
                "SELECT display_name, base_url, sort_order, created_at_ms, updated_at_ms
                 FROM routes WHERE route_id = 'route-1'",
                [],
                |row| {
                    Ok(MigratedV20Route {
                        name: row.get(0)?,
                        base_url: row.get(1)?,
                        sort_order: row.get(2)?,
                        created_at_ms: row.get(3)?,
                        updated_at_ms: row.get(4)?,
                    })
                },
            )?,
            request: read_migrated_v20_request(connection)?,
            attempt: read_migrated_v20_attempt(connection)?,
            transition: connection.query_row(
                "SELECT attempt_role, routing_transition_kind,
                        routing_transition_target_route_id,
                        routing_transition_target_route_name
                 FROM upstream_attempts WHERE attempt_id = 'attempt-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )?,
            exclusions: connection.query_row(
                "SELECT COUNT(*) FROM route_fallback_excluded_models
                 WHERE route_id = 'route-1'",
                [],
                |row| row.get(0),
            )?,
            skips: connection.query_row(
                "SELECT COUNT(*) FROM upstream_attempt_routing_skips",
                [],
                |row| row.get(0),
            )?,
        })
    }

    #[tokio::test]
    async fn migration_v20_preserves_history_and_defaults_new_route_and_attempt_metadata() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("router.sqlite3");
        prepare_v19_fallback_history_fixture(&path);
        let database = DatabaseExecutor::open(&path).expect("migrate v19 database");
        let values = database
            .test_execute(|connection| read_migrated_v20_values(connection))
            .await
            .expect("migrated rows");

        assert_eq!(
            values,
            MigratedV20Values {
                version: SCHEMA_VERSION,
                tables: vec![
                    "route_fallback_excluded_models".to_owned(),
                    "upstream_attempt_routing_skips".to_owned(),
                ],
                route: MigratedV20Route {
                    name: "Route".to_owned(),
                    base_url: "https://example.invalid/v1".to_owned(),
                    sort_order: 0,
                    created_at_ms: 3,
                    updated_at_ms: 4,
                },
                request: MigratedV20Request {
                    started_at_ms: 10,
                    finished_at_ms: Some(40),
                    requested_model: Some("luna".to_owned()),
                    actual_model: Some("luna-actual".to_owned()),
                    final_route_id: Some("route-1".to_owned()),
                    final_route_name: Some("Route".to_owned()),
                    completion_state: "failed".to_owned(),
                    http_status: Some(503),
                    error_category: Some("upstream_http_status".to_owned()),
                    total_tokens: Some(18),
                    cost_pico_usd: Some(1234),
                    fallback_stop_reason: Some("all_participants_attempted".to_owned()),
                },
                attempt: MigratedV20Attempt {
                    attempt_id: "attempt-1".to_owned(),
                    request_id: "request-1".to_owned(),
                    attempt_index: 0,
                    route_id: "route-1".to_owned(),
                    route_name: "Route".to_owned(),
                    started_at_ms: 10,
                    finished_at_ms: Some(40),
                    http_status: Some(503),
                    error_category: Some("upstream_http_status".to_owned()),
                    delivery_state: "completed".to_owned(),
                    total_tokens: Some(18),
                    cost_pico_usd: Some(1234),
                },
                transition: ("ordinary".to_owned(), None, None, None),
                exclusions: 0,
                skips: 0,
            }
        );
    }

    #[tokio::test]
    async fn storage_balance_query_policy_is_durable_atomic_and_write_free_when_unchanged() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("data/router.sqlite3");
        let database = DatabaseExecutor::open(&path).expect("database opens");
        let policy = BalanceQueryPolicy::parse(45, 120).expect("custom policy");
        assert!(
            database
                .set_balance_query_policy(policy)
                .await
                .expect("set policy")
        );
        assert_eq!(
            database
                .app_settings()
                .await
                .expect("changed settings")
                .balance_query_policy,
            policy
        );
        drop(database);

        let reopened = DatabaseExecutor::open(&path).expect("reopened database");
        assert_eq!(
            reopened
                .app_settings()
                .await
                .expect("reopened settings")
                .balance_query_policy,
            policy
        );
        reopened
            .test_execute(|connection| {
                connection.execute_batch(
                    "CREATE TRIGGER fail_balance_policy_update BEFORE UPDATE OF menu_balance_debounce_seconds, automatic_balance_refresh_minutes ON app_settings BEGIN SELECT RAISE(ABORT, 'injected'); END;",
                )?;
                Ok(())
            })
            .await
            .expect("failure trigger");
        assert!(
            !reopened
                .set_balance_query_policy(policy)
                .await
                .expect("unchanged policy performs no update")
        );
        let rejected = BalanceQueryPolicy::parse(60, 180).expect("rejected policy");
        assert!(reopened.set_balance_query_policy(rejected).await.is_err());
        assert_eq!(
            reopened
                .app_settings()
                .await
                .expect("settings after failure")
                .balance_query_policy,
            policy
        );
    }

    #[tokio::test]
    async fn storage_balance_query_columns_reject_direct_out_of_range_writes() {
        let (_directory, database) = database();
        assert!(
            database
                .test_execute(|connection| {
                    connection.execute(
                        "UPDATE app_settings SET menu_balance_debounce_seconds = 9 WHERE singleton = 1",
                        [],
                    )?;
                    Ok(())
                })
                .await
                .is_err()
        );
        assert!(
            database
                .test_execute(|connection| {
                    connection.execute(
                        "UPDATE app_settings SET automatic_balance_refresh_minutes = 1441 WHERE singleton = 1",
                        [],
                    )?;
                    Ok(())
                })
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn rejected_script_risk_confirmation_writes_nothing() {
        let (_directory, database) = database();
        let mut input = route("Needs confirmation", "never-stored");
        input.accept_script_risk = false;

        let error = database
            .create_route(input)
            .await
            .expect_err("risk confirmation is required");
        assert!(matches!(
            error,
            StorageError::BalanceScriptRiskConfirmationRequired
        ));

        let (routes, secrets, scripts) = database
            .test_execute(|connection| {
                Ok((
                    connection.query_row("SELECT COUNT(*) FROM routes", [], |row| row.get(0))?,
                    connection.query_row("SELECT COUNT(*) FROM secrets", [], |row| row.get(0))?,
                    connection
                        .query_row("SELECT COUNT(*) FROM balance_queries", [], |row| row.get(0))?,
                ))
            })
            .await
            .expect("row counts");
        assert_eq!((routes, secrets, scripts), (0_i64, 0_i64, 0_i64));
        assert_eq!(
            database.active_route_id().await.expect("active route"),
            None
        );
        assert!(
            !database
                .app_settings()
                .await
                .expect("settings")
                .balance_script_risk_confirmed
        );
    }

    #[tokio::test]
    async fn accepted_script_enablement_commits_route_secret_script_and_risk_together() {
        let (_directory, database) = database();
        let created = database
            .create_route(route("Confirmed", "stored-key"))
            .await
            .expect("confirmed route");

        let edit = database
            .route_edit(created.route_id.clone())
            .await
            .expect("route edit");
        assert_eq!(edit.api_key.expose(), b"stored-key");
        assert!(edit.balance_query.is_some_and(|script| script.enabled));
        assert_eq!(
            database.active_route_id().await.expect("active route"),
            Some(created.route_id)
        );
        assert!(
            database
                .app_settings()
                .await
                .expect("settings")
                .balance_script_risk_confirmed
        );
    }

    #[tokio::test]
    async fn route_storage_canonicalizes_complete_responses_endpoints_on_real_saves() {
        let (_directory, database) = database();
        let mut input = route("Canonical", "stored-key");
        input.base_url = " https://example.test/openai/v1/responses/ ".to_owned();
        let created = database.create_route(input).await.expect("create route");
        assert_eq!(created.base_url, "https://example.test/openai/v1");

        database
            .test_execute({
                let route_id = created.route_id.clone();
                move |connection| {
                    connection.execute(
                        "UPDATE routes SET base_url = ?1 WHERE route_id = ?2",
                        rusqlite::params!["https://legacy.example/v1/responses", route_id.as_str()],
                    )?;
                    Ok(())
                }
            })
            .await
            .expect("seed legacy complete endpoint");

        let legacy = database
            .route_edit(created.route_id.clone())
            .await
            .expect("legacy route remains readable");
        assert_eq!(legacy.route.base_url, "https://legacy.example/v1/responses");
        let parsed = BaseUrl::parse(&legacy.route.base_url).expect("legacy endpoint parses");
        assert_eq!(parsed.as_str(), "https://legacy.example/v1");
        assert_eq!(
            parsed.inference_url(),
            "https://legacy.example/v1/responses"
        );

        database
            .update_route(UpdateRouteInput {
                route_id: created.route_id.clone(),
                name: legacy.route.name,
                base_url: legacy.route.base_url,
                api_key: legacy.api_key,
                service_tier_policy: legacy.route.service_tier_policy,
                balance_query: legacy.balance_query,
                accept_script_risk: true,
            })
            .await
            .expect("real route save");
        assert_eq!(
            database
                .route_edit(created.route_id)
                .await
                .expect("canonical route edit")
                .route
                .base_url,
            "https://legacy.example/v1"
        );
    }

    #[tokio::test]
    async fn first_route_is_active_and_names_are_ascii_case_insensitive() {
        let (_directory, database) = database();
        let first = database
            .create_route(route("Work", "first"))
            .await
            .expect("first route");
        database
            .create_route(route("Personal", "second"))
            .await
            .expect("second route");
        assert_eq!(
            database.active_route_id().await.expect("active"),
            Some(first.route_id)
        );

        let error = database
            .create_route(route("work", "duplicate"))
            .await
            .expect_err("duplicate name must fail");
        assert!(matches!(error, StorageError::Database(_)));
    }

    #[tokio::test]
    async fn fallback_enablement_requires_two_routes_and_revisions_only_on_changes() {
        let (_directory, database) = database();
        let initial = database.routing_state().await.expect("initial routing");
        assert!(!initial.fallback.enabled);
        assert_eq!(initial.fallback.participant_count, 0);

        database
            .create_route(route("Only", "only-key"))
            .await
            .expect("first route");
        let after_first = database.routing_state().await.expect("first routing");
        let unavailable = database
            .set_fallback_enabled(true)
            .await
            .expect("forced off");
        assert!(!unavailable.enabled);
        assert_eq!(
            unavailable.config_revision,
            after_first.fallback.config_revision
        );

        database
            .create_route(route("Second", "second-key"))
            .await
            .expect("second route");
        let enabled = database.set_fallback_enabled(true).await.expect("enable");
        assert!(enabled.enabled);
        let unchanged = database
            .set_fallback_enabled(true)
            .await
            .expect("same value");
        assert_eq!(unchanged.config_revision, enabled.config_revision);
        let disabled = database.set_fallback_enabled(false).await.expect("disable");
        assert_eq!(disabled.config_revision, enabled.config_revision + 1);
        let reenabled = database.set_fallback_enabled(true).await.expect("reenable");
        assert_eq!(reenabled.config_revision, disabled.config_revision + 1);
    }

    #[tokio::test]
    async fn participant_count_changes_are_revisioned_no_ops_and_do_not_auto_enable() {
        let (_directory, database) = database();
        database
            .create_route(route("First", "first-key"))
            .await
            .expect("first");
        database
            .create_route(route("Second", "second-key"))
            .await
            .expect("second");
        database.set_fallback_enabled(true).await.expect("enable");
        let before = database.routing_state().await.expect("before boundary");
        let critical_before = database.critical_revision().await.expect("critical before");

        assert!(
            database
                .set_fallback_participant_count(1)
                .await
                .expect("shrink boundary")
        );
        let shrunk = database.routing_state().await.expect("shrunk boundary");
        assert_eq!(shrunk.fallback.participant_count, 1);
        assert!(!shrunk.fallback.enabled);
        assert_eq!(
            shrunk.fallback.config_revision,
            before.fallback.config_revision + 1
        );
        assert_eq!(
            database.critical_revision().await.expect("critical shrink"),
            critical_before + 1
        );

        assert!(
            !database
                .set_fallback_participant_count(1)
                .await
                .expect("same boundary")
        );
        assert_eq!(
            database.critical_revision().await.expect("critical no-op"),
            critical_before + 1
        );

        assert!(
            database
                .set_fallback_participant_count(2)
                .await
                .expect("expand boundary")
        );
        let expanded = database.routing_state().await.expect("expanded boundary");
        assert_eq!(expanded.fallback.participant_count, 2);
        assert!(!expanded.fallback.enabled);
        assert!(
            database
                .set_fallback_participant_count(0)
                .await
                .expect("move boundary before first route")
        );
        let empty_prefix = database.routing_state().await.expect("empty prefix");
        assert_eq!(empty_prefix.fallback.participant_count, 0);
        assert!(!empty_prefix.fallback.enabled);
        assert!(matches!(
            database.set_fallback_participant_count(3).await,
            Err(StorageError::InvalidFallbackParticipantCount)
        ));
        assert_eq!(
            database
                .critical_revision()
                .await
                .expect("critical invalid"),
            critical_before + 3
        );
    }

    #[tokio::test]
    async fn route_list_mutations_maintain_the_participant_boundary_atomically() {
        let (_directory, database) = database();
        let initial = database.routing_state().await.expect("initial boundary");
        let first = database
            .create_route(route("First", "first-key"))
            .await
            .expect("first");
        let after_first = database.routing_state().await.expect("first boundary");
        assert_eq!(after_first.fallback.participant_count, 1);
        assert!(!after_first.fallback.enabled);
        assert_eq!(
            after_first.fallback.config_revision,
            initial.fallback.config_revision + 1
        );
        let second = database
            .create_route(route("Second", "second-key"))
            .await
            .expect("second");
        let after_second = database.routing_state().await.expect("tail boundary");
        assert_eq!(after_second.fallback.participant_count, 2);
        assert_eq!(
            after_second.fallback.config_revision,
            after_first.fallback.config_revision + 1
        );

        database
            .set_fallback_participant_count(1)
            .await
            .expect("middle boundary");
        let before_middle_append = database.routing_state().await.expect("middle boundary");
        let third = database
            .create_route(route("Third", "third-key"))
            .await
            .expect("third");
        let after_middle_append = database.routing_state().await.expect("middle append");
        assert_eq!(after_middle_append.fallback.participant_count, 1);
        assert_eq!(
            after_middle_append.fallback.config_revision,
            before_middle_append.fallback.config_revision
        );

        database
            .set_fallback_participant_count(3)
            .await
            .expect("move to tail");
        let before_tail_append = database.routing_state().await.expect("tail before append");
        let fourth = database
            .create_route(route("Fourth", "fourth-key"))
            .await
            .expect("fourth");
        let after_tail_append = database.routing_state().await.expect("tail append");
        assert_eq!(after_tail_append.fallback.participant_count, 4);
        assert_eq!(
            after_tail_append.fallback.config_revision,
            before_tail_append.fallback.config_revision + 1
        );

        database
            .set_fallback_participant_count(2)
            .await
            .expect("two participants");
        database.set_fallback_enabled(true).await.expect("enable");
        database
            .move_route(third.route_id.clone(), RouteMoveDirection::Up)
            .await
            .expect("cross boundary");
        let after_reorder = database.routing_state().await.expect("after reorder");
        assert_eq!(after_reorder.fallback.participant_count, 2);
        assert!(after_reorder.fallback.enabled);
        assert_eq!(
            database
                .list_routes()
                .await
                .expect("reordered routes")
                .into_iter()
                .take(2)
                .map(|route| route.route_id)
                .collect::<Vec<_>>(),
            vec![first.route_id.clone(), third.route_id]
        );

        database
            .delete_route(fourth.route_id)
            .await
            .expect("delete below boundary");
        let after_below = database.routing_state().await.expect("after below delete");
        assert_eq!(after_below.fallback.participant_count, 2);
        assert!(after_below.fallback.enabled);

        database
            .delete_route(first.route_id)
            .await
            .expect("delete above boundary");
        let after_above = database.routing_state().await.expect("after above delete");
        assert_eq!(after_above.fallback.participant_count, 1);
        assert!(!after_above.fallback.enabled);
        assert_eq!(
            database.list_routes().await.expect("remaining routes")[1].route_id,
            second.route_id
        );
    }

    #[tokio::test]
    async fn invalid_persisted_participant_boundary_fails_closed() {
        let (_directory, database) = database();
        database
            .create_route(route("Only", "only-key"))
            .await
            .expect("route");
        database
            .test_execute(|connection| {
                connection.execute(
                    "UPDATE fallback_config SET participant_count = 2 WHERE singleton = 1",
                    [],
                )?;
                Ok(())
            })
            .await
            .expect("inject invalid boundary");
        assert!(matches!(
            database.routing_state().await,
            Err(StorageError::Initialization)
        ));
    }

    #[tokio::test]
    async fn adjacent_reorder_changes_shared_order_without_changing_active_selection() {
        let (_directory, database) = database();
        let first = database
            .create_route(route("First", "first-key"))
            .await
            .expect("first");
        let second = database
            .create_route(route("Second", "second-key"))
            .await
            .expect("second");
        let before = database.routing_state().await.expect("routing before");

        assert!(
            database
                .move_route(second.route_id.clone(), RouteMoveDirection::Up)
                .await
                .expect("move")
        );
        assert_eq!(
            database
                .list_routes()
                .await
                .expect("ordered routes")
                .into_iter()
                .map(|route| route.route_id)
                .collect::<Vec<_>>(),
            vec![second.route_id, first.route_id.clone()]
        );
        let after = database.routing_state().await.expect("routing after");
        assert_eq!(after.active_route_id, Some(first.route_id));
        assert_eq!(after.selection_generation, before.selection_generation);
        assert_eq!(
            after.fallback.config_revision,
            before.fallback.config_revision + 1
        );
    }

    #[tokio::test]
    async fn atomic_route_and_fallback_reorder_updates_each_revision_once_and_noops() {
        let (_directory, database) = database();
        let first = database
            .create_route(route("First", "first-key"))
            .await
            .expect("first");
        let second = database
            .create_route(route("Second", "second-key"))
            .await
            .expect("second");
        let third = database
            .create_route(route("Third", "third-key"))
            .await
            .expect("third");
        database.set_fallback_enabled(true).await.expect("enable");
        let before = database.routing_state().await.expect("routing before");
        let critical_before = database.critical_revision().await.expect("critical before");

        let order_only = vec![
            third.route_id.clone(),
            first.route_id.clone(),
            second.route_id.clone(),
        ];
        let revision = before.fallback.config_revision;
        assert!(reorder_routes(&database, order_only.clone(), 3, revision).await);
        let after_order = database.routing_state().await.expect("after order");
        assert_eq!(after_order.fallback.participant_count, 3);
        assert!(after_order.fallback.enabled);
        assert_eq!(
            after_order.fallback.config_revision,
            before.fallback.config_revision + 1
        );
        assert_eq!(
            database.critical_revision().await.expect("critical order"),
            critical_before + 1
        );

        let revision = after_order.fallback.config_revision;
        assert!(reorder_routes(&database, order_only.clone(), 1, revision).await);
        let after_boundary = database.routing_state().await.expect("after boundary");
        assert_eq!(after_boundary.fallback.participant_count, 1);
        assert!(!after_boundary.fallback.enabled);
        assert_eq!(
            after_boundary.fallback.config_revision,
            after_order.fallback.config_revision + 1
        );

        let combined = vec![second.route_id, third.route_id, first.route_id];
        let revision = after_boundary.fallback.config_revision;
        assert!(reorder_routes(&database, combined.clone(), 2, revision).await);
        let after_combined = database.routing_state().await.expect("after combined");
        assert_eq!(after_combined.fallback.participant_count, 2);
        assert!(!after_combined.fallback.enabled);
        assert_eq!(
            after_combined.fallback.config_revision,
            after_boundary.fallback.config_revision + 1
        );
        assert_eq!(
            database
                .list_routes()
                .await
                .expect("ordered routes")
                .into_iter()
                .map(|route| route.route_id)
                .collect::<Vec<_>>(),
            combined
        );
        let critical_after = database
            .critical_revision()
            .await
            .expect("critical combined");
        assert_eq!(critical_after, critical_before + 3);

        let revision = after_combined.fallback.config_revision;
        assert!(!reorder_routes(&database, combined, 2, revision).await);
        assert_eq!(
            database.critical_revision().await.expect("critical no-op"),
            critical_after
        );
        assert_eq!(
            database
                .routing_state()
                .await
                .expect("routing no-op")
                .fallback
                .config_revision,
            after_combined.fallback.config_revision
        );
    }

    #[tokio::test]
    async fn atomic_route_and_fallback_reorder_rejects_stale_or_invalid_candidates() {
        let (_directory, database) = database();
        let first = database
            .create_route(route("First", "first-key"))
            .await
            .expect("first");
        let second = database
            .create_route(route("Second", "second-key"))
            .await
            .expect("second");
        let third = database
            .create_route(route("Third", "third-key"))
            .await
            .expect("third");
        let route_ids = vec![
            first.route_id.clone(),
            second.route_id.clone(),
            third.route_id.clone(),
        ];
        let before = database.routing_state().await.expect("routing before");
        let critical_before = database.critical_revision().await.expect("critical before");

        let invalid_candidates = [
            (vec![first.route_id.clone(), second.route_id.clone()], 2),
            (
                vec![
                    first.route_id.clone(),
                    first.route_id.clone(),
                    third.route_id.clone(),
                ],
                2,
            ),
            (
                vec![
                    first.route_id.clone(),
                    second.route_id.clone(),
                    RouteId::from_string("unknown-route".to_owned()),
                ],
                2,
            ),
            (route_ids.clone(), 4),
        ];
        for (candidate, participant_count) in invalid_candidates {
            assert!(matches!(
                database
                    .reorder_routes_and_fallback(
                        candidate,
                        participant_count,
                        before.fallback.config_revision,
                    )
                    .await,
                Err(StorageError::InvalidRoutePermutation)
            ));
        }

        assert!(matches!(
            database
                .reorder_routes_and_fallback(
                    vec![third.route_id, second.route_id, first.route_id],
                    2,
                    before.fallback.config_revision + 1,
                )
                .await,
            Err(StorageError::StaleRoutingConfiguration)
        ));
        assert_eq!(
            database
                .list_routes()
                .await
                .expect("unchanged routes")
                .into_iter()
                .map(|route| route.route_id)
                .collect::<Vec<_>>(),
            route_ids
        );
        assert_eq!(
            database.routing_state().await.expect("unchanged routing"),
            before
        );
        assert_eq!(
            database
                .critical_revision()
                .await
                .expect("unchanged critical"),
            critical_before
        );
    }

    #[tokio::test]
    async fn atomic_route_and_fallback_reorder_rolls_back_on_sql_failure() {
        let (_directory, database) = database();
        let first = database
            .create_route(route("First", "first-key"))
            .await
            .expect("first");
        let second = database
            .create_route(route("Second", "second-key"))
            .await
            .expect("second");
        let before_routes = database.list_routes().await.expect("routes before");
        let before_routing = database.routing_state().await.expect("routing before");
        let critical_before = database.critical_revision().await.expect("critical before");
        database
            .test_execute(|connection| {
                connection.execute_batch(
                    "CREATE TRIGGER fail_route_reorder
                     BEFORE UPDATE OF sort_order ON routes
                     BEGIN SELECT RAISE(ABORT, 'blocked'); END;",
                )?;
                Ok(())
            })
            .await
            .expect("failure trigger");

        assert!(matches!(
            database
                .reorder_routes_and_fallback(
                    vec![second.route_id, first.route_id],
                    1,
                    before_routing.fallback.config_revision,
                )
                .await,
            Err(StorageError::Database(_))
        ));
        assert_eq!(
            database.list_routes().await.expect("routes after"),
            before_routes
        );
        assert_eq!(
            database.routing_state().await.expect("routing after"),
            before_routing
        );
        assert_eq!(
            database.critical_revision().await.expect("critical after"),
            critical_before
        );
    }

    #[tokio::test]
    async fn concurrent_atomic_reorders_allow_one_revision_winner() {
        let (_directory, database) = database();
        let first = database
            .create_route(route("First", "first-key"))
            .await
            .expect("first");
        let second = database
            .create_route(route("Second", "second-key"))
            .await
            .expect("second");
        let third = database
            .create_route(route("Third", "third-key"))
            .await
            .expect("third");
        let before = database.routing_state().await.expect("routing before");
        let critical_before = database.critical_revision().await.expect("critical before");
        let first_candidate = vec![
            second.route_id.clone(),
            first.route_id.clone(),
            third.route_id.clone(),
        ];
        let second_candidate = vec![third.route_id, second.route_id, first.route_id];

        let (first_result, second_result) = tokio::join!(
            database.reorder_routes_and_fallback(
                first_candidate.clone(),
                2,
                before.fallback.config_revision,
            ),
            database.reorder_routes_and_fallback(
                second_candidate.clone(),
                2,
                before.fallback.config_revision,
            )
        );
        let results = [&first_result, &second_result];
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(result, Ok(true)))
                .count(),
            1
        );
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(result, Err(StorageError::StaleRoutingConfiguration)))
                .count(),
            1
        );
        let stored = database
            .list_routes()
            .await
            .expect("winner order")
            .into_iter()
            .map(|route| route.route_id)
            .collect::<Vec<_>>();
        assert!(stored == first_candidate || stored == second_candidate);
        let after = database.routing_state().await.expect("routing after");
        assert_eq!(
            after.fallback.config_revision,
            before.fallback.config_revision + 1
        );
        assert_eq!(
            database.critical_revision().await.expect("critical after"),
            critical_before + 1
        );
    }

    #[tokio::test]
    async fn conditional_activation_accepts_only_current_immediate_next_candidate() {
        let (_directory, database) = database();
        let first = database
            .create_route(route("First", "first-key"))
            .await
            .expect("first");
        let second = database
            .create_route(route("Second", "second-key"))
            .await
            .expect("second");
        let third = database
            .create_route(route("Third", "third-key"))
            .await
            .expect("third");
        database
            .set_fallback_enabled(true)
            .await
            .expect("enable fallback");
        let captured = database.routing_state().await.expect("captured routing");

        assert!(
            database
                .conditional_activate_next(
                    first.route_id.clone(),
                    captured.selection_generation,
                    captured.fallback.config_revision,
                    second.route_id.clone(),
                )
                .await
                .expect("conditional activation")
        );
        let activated = database.routing_state().await.expect("activated routing");
        assert_eq!(activated.active_route_id, Some(second.route_id.clone()));
        assert_eq!(
            activated.selection_generation,
            captured.selection_generation + 1
        );

        assert!(
            !database
                .conditional_activate_next(
                    first.route_id,
                    captured.selection_generation,
                    captured.fallback.config_revision,
                    third.route_id,
                )
                .await
                .expect("stale activation")
        );
        assert_eq!(
            database
                .routing_state()
                .await
                .expect("unchanged")
                .active_route_id,
            Some(second.route_id)
        );
    }

    #[tokio::test]
    async fn conditional_activation_rejects_wraparound_at_the_boundary() {
        let (_directory, database) = database();
        let first = database
            .create_route(route("First", "first-key"))
            .await
            .expect("first");
        let second = database
            .create_route(route("Second", "second-key"))
            .await
            .expect("second");
        let third = database
            .create_route(route("Third", "third-key"))
            .await
            .expect("third");
        database.set_fallback_enabled(true).await.expect("enable");
        database
            .activate_route(third.route_id.clone())
            .await
            .expect("activate third");
        let captured = database.routing_state().await.expect("captured routing");

        assert!(
            !database
                .conditional_activate_next(
                    third.route_id.clone(),
                    captured.selection_generation,
                    captured.fallback.config_revision,
                    second.route_id,
                )
                .await
                .expect("reject non-successor")
        );
        assert!(
            !database
                .conditional_activate_next(
                    third.route_id.clone(),
                    captured.selection_generation,
                    captured.fallback.config_revision,
                    first.route_id,
                )
                .await
                .expect("reject wrap activation")
        );
        assert_eq!(
            database
                .routing_state()
                .await
                .expect("routing")
                .active_route_id,
            Some(third.route_id)
        );
    }

    #[test]
    fn routing_decisions_are_derived_from_ordered_attempts_and_terminal_metadata() {
        fn attempt(index: u32, route_id: RouteId, route_name: &str) -> UsageAttemptDetail {
            UsageAttemptDetail {
                attempt_index: index,
                attempt_role: super::AttemptRole::Ordinary,
                route_id,
                route_name: route_name.to_owned(),
                started_at_ms: i64::from(index),
                finished_at_ms: Some(i64::from(index) + 1),
                http_status: Some(500),
                error_category: Some("upstream_http_status".to_owned()),
                delivery_state: crate::domain::DeliveryState::Completed,
                actual_model: None,
                forwarded_service_tier: None,
                actual_service_tier: None,
                input_tokens: None,
                output_tokens: None,
                total_tokens: None,
                cached_input_tokens: None,
                cache_write_input_tokens: None,
                pricing_catalog_version: None,
                cost_status: None,
                cost_pico_usd: None,
                routing_transition: None,
                routing_decision: None,
            }
        }

        let first = RouteId::new();
        let second = RouteId::new();
        let mut attempts = vec![
            attempt(0, first.clone(), "First"),
            attempt(1, first.clone(), "First"),
            attempt(2, second.clone(), "Second"),
        ];
        materialize_routing_decisions(
            &mut attempts,
            Some(FallbackStopReason::AllParticipantsAttempted),
            None,
            None,
        );

        assert_eq!(
            attempts[0].routing_decision,
            Some(RoutingDecision::RetryCurrent {
                attempt_number: 2,
                max_attempts: 4,
            })
        );
        assert_eq!(
            attempts[1].routing_decision,
            Some(RoutingDecision::ActivateNext {
                target_route_id: second.clone(),
                target_route_name: "Second".to_owned(),
                skipped_routes: Vec::new(),
            })
        );
        assert_eq!(
            attempts[2].routing_decision,
            Some(RoutingDecision::Stop {
                reason: FallbackStopReason::AllParticipantsAttempted,
                target_route_id: None,
                target_route_name: None,
            })
        );

        let third = RouteId::new();
        let mut forward_only = vec![
            attempt(0, first, "First"),
            attempt(1, second.clone(), "Second"),
            attempt(2, third.clone(), "Third"),
        ];
        materialize_routing_decisions(
            &mut forward_only,
            Some(FallbackStopReason::AllParticipantsAttempted),
            None,
            None,
        );
        assert_eq!(
            forward_only
                .into_iter()
                .map(|attempt| attempt.routing_decision)
                .collect::<Vec<_>>(),
            vec![
                Some(RoutingDecision::ActivateNext {
                    target_route_id: second,
                    target_route_name: "Second".to_owned(),
                    skipped_routes: Vec::new(),
                }),
                Some(RoutingDecision::ActivateNext {
                    target_route_id: third,
                    target_route_name: "Third".to_owned(),
                    skipped_routes: Vec::new(),
                }),
                Some(RoutingDecision::Stop {
                    reason: FallbackStopReason::AllParticipantsAttempted,
                    target_route_id: None,
                    target_route_name: None,
                }),
            ]
        );
    }

    #[tokio::test]
    async fn conditional_activation_reads_the_current_participant_count_in_transaction() {
        let (_directory, database) = database();
        let first = database
            .create_route(route("First", "first-key"))
            .await
            .expect("first");
        let second = database
            .create_route(route("Second", "second-key"))
            .await
            .expect("second");
        let third = database
            .create_route(route("Third", "third-key"))
            .await
            .expect("third");
        database
            .set_fallback_participant_count(2)
            .await
            .expect("two participants");
        database.set_fallback_enabled(true).await.expect("enable");
        database
            .activate_route(second.route_id.clone())
            .await
            .expect("activate second");
        let excluded = database.routing_state().await.expect("excluded capture");
        assert!(
            !database
                .conditional_activate_next(
                    second.route_id.clone(),
                    excluded.selection_generation,
                    excluded.fallback.config_revision,
                    third.route_id.clone(),
                )
                .await
                .expect("excluded target")
        );

        database
            .set_fallback_participant_count(3)
            .await
            .expect("include third");
        let included = database.routing_state().await.expect("included capture");
        assert!(
            database
                .conditional_activate_next(
                    second.route_id,
                    included.selection_generation,
                    included.fallback.config_revision,
                    third.route_id.clone(),
                )
                .await
                .expect("included target")
        );
        assert_eq!(
            database.active_route_id().await.expect("active"),
            Some(third.route_id)
        );
        assert_ne!(
            database.active_route_id().await.expect("active"),
            Some(first.route_id)
        );
    }

    #[tokio::test]
    async fn reorder_disable_and_delete_make_captured_activation_stale() {
        let (_directory, database) = database();
        let first = database
            .create_route(route("First", "first-key"))
            .await
            .expect("first");
        let second = database
            .create_route(route("Second", "second-key"))
            .await
            .expect("second");
        database
            .set_fallback_enabled(true)
            .await
            .expect("enable fallback");
        let captured = database.routing_state().await.expect("captured routing");

        database
            .move_route(second.route_id.clone(), RouteMoveDirection::Up)
            .await
            .expect("reorder");
        assert!(
            !database
                .conditional_activate_next(
                    first.route_id.clone(),
                    captured.selection_generation,
                    captured.fallback.config_revision,
                    second.route_id.clone(),
                )
                .await
                .expect("stale reorder")
        );

        database
            .move_route(second.route_id.clone(), RouteMoveDirection::Down)
            .await
            .expect("restore order");
        let after_reorder = database.routing_state().await.expect("after reorder");
        database.set_fallback_enabled(false).await.expect("disable");
        assert!(
            !database
                .conditional_activate_next(
                    first.route_id.clone(),
                    after_reorder.selection_generation,
                    after_reorder.fallback.config_revision,
                    second.route_id.clone(),
                )
                .await
                .expect("stale disable")
        );

        database.set_fallback_enabled(true).await.expect("reenable");
        database
            .delete_route(second.route_id)
            .await
            .expect("delete");
        let after_delete = database.routing_state().await.expect("after delete");
        assert!(!after_delete.fallback.enabled);
        assert_eq!(after_delete.active_route_id, Some(first.route_id));
    }

    #[tokio::test]
    async fn create_and_update_failures_roll_back_secret_changes() {
        let (_directory, database) = database();
        database
            .test_execute(|connection| {
                connection.execute_batch(
                    "CREATE TRIGGER fail_route_insert BEFORE INSERT ON routes BEGIN SELECT RAISE(ABORT, 'injected'); END;",
                )?;
                Ok(())
            })
            .await
            .expect("trigger");
        assert!(
            database
                .create_route(route("Broken", "leaked"))
                .await
                .is_err()
        );
        let secret_count = database
            .test_execute(|connection| {
                Ok(
                    connection.query_row("SELECT COUNT(*) FROM secrets", [], |row| {
                        row.get::<_, i64>(0)
                    })?,
                )
            })
            .await
            .expect("secret count");
        assert_eq!(secret_count, 0);

        database
            .test_execute(|connection| {
                connection.execute_batch("DROP TRIGGER fail_route_insert")?;
                Ok(())
            })
            .await
            .expect("drop trigger");
        let stored = database
            .create_route(route("Working", "original"))
            .await
            .expect("route");
        database
            .test_execute(|connection| {
                connection.execute_batch(
                    "CREATE TRIGGER fail_route_update BEFORE UPDATE ON routes BEGIN SELECT RAISE(ABORT, 'injected'); END;",
                )?;
                Ok(())
            })
            .await
            .expect("trigger");
        let update = UpdateRouteInput {
            route_id: stored.route_id,
            name: "Changed".to_owned(),
            base_url: "https://example.com/v1".to_owned(),
            api_key: ApiKey::parse("replacement").expect("key"),
            service_tier_policy: ServiceTierPolicy::Passthrough,
            balance_query: None,
            accept_script_risk: false,
        };
        assert!(database.update_route(update).await.is_err());
        let secret = SqliteSecretStore::new(database.clone())
            .get(stored.secret_id)
            .await
            .expect("secret");
        assert_eq!(secret.expose(), b"original");
    }

    #[tokio::test]
    async fn late_create_and_delete_failures_roll_back_the_whole_transaction() {
        let (_directory, database) = database();
        database
            .test_execute(|connection| {
                connection.execute_batch(
                    "CREATE TRIGGER fail_query_insert BEFORE INSERT ON balance_queries BEGIN SELECT RAISE(ABORT, 'injected'); END;",
                )?;
                Ok(())
            })
            .await
            .expect("trigger");
        assert!(
            database
                .create_route(route("Broken", "secret"))
                .await
                .is_err()
        );
        assert!(
            !database
                .app_settings()
                .await
                .expect("settings")
                .balance_script_risk_confirmed
        );
        let counts = database
            .test_execute(|connection| {
                Ok((
                    connection.query_row("SELECT COUNT(*) FROM routes", [], |row| {
                        row.get::<_, i64>(0)
                    })?,
                    connection.query_row("SELECT COUNT(*) FROM secrets", [], |row| {
                        row.get::<_, i64>(0)
                    })?,
                ))
            })
            .await
            .expect("counts");
        assert_eq!(counts, (0, 0));
        assert_eq!(
            database
                .routing_state()
                .await
                .expect("fallback after failed create")
                .fallback
                .participant_count,
            0
        );

        database
            .test_execute(|connection| {
                connection.execute_batch("DROP TRIGGER fail_query_insert")?;
                Ok(())
            })
            .await
            .expect("drop trigger");
        let stored = database
            .create_route(route("Current", "secret"))
            .await
            .expect("route");
        let route_id = stored.route_id.clone();
        database
            .test_execute(|connection| {
                connection.execute_batch(
                    "CREATE TRIGGER fail_secret_delete BEFORE DELETE ON secrets BEGIN SELECT RAISE(ABORT, 'injected'); END;",
                )?;
                Ok(())
            })
            .await
            .expect("trigger");
        assert!(database.delete_route(stored.route_id).await.is_err());
        assert_eq!(
            database.active_route_id().await.expect("active"),
            Some(route_id)
        );
        assert_eq!(database.list_routes().await.expect("routes").len(), 1);
        assert_eq!(
            database
                .routing_state()
                .await
                .expect("fallback after failed delete")
                .fallback
                .participant_count,
            1
        );
        assert_eq!(
            SqliteSecretStore::new(database)
                .get(stored.secret_id)
                .await
                .expect("secret")
                .expose(),
            b"secret"
        );
    }

    #[tokio::test]
    async fn active_delete_is_atomic_and_preserves_history_snapshot() {
        let (_directory, database) = database();
        let stored = database
            .create_route(route("Current", "owned"))
            .await
            .expect("route");
        let other_secret = SqliteSecretStore::new(database.clone())
            .put(
                "gateway_token".to_owned(),
                ApiKey::parse("gateway").expect("key"),
            )
            .await
            .expect("gateway secret");
        let route_id = stored.route_id.clone();
        database
            .test_execute(move |connection| {
                connection.execute(
                    "INSERT INTO proxy_requests (request_id, started_at_ms, final_route_id, final_route_name, streaming, completion_state, metadata_complete) VALUES ('request-1', 1, ?1, 'Current', 0, 'completed', 1)",
                    [route_id.as_str()],
                )?;
                Ok(())
            })
            .await
            .expect("history");
        let deleted = database
            .delete_route(stored.route_id)
            .await
            .expect("delete");
        assert!(deleted.cleared_active_route);
        assert_eq!(database.active_route_id().await.expect("active"), None);
        assert_eq!(
            database
                .history_summary()
                .await
                .expect("summary")
                .request_count,
            1
        );
        assert_eq!(
            SqliteSecretStore::new(database.clone())
                .get(other_secret)
                .await
                .expect("gateway")
                .expose(),
            b"gateway"
        );
        let (secret_count, script_count) = database
            .test_execute(|connection| {
                Ok((
                    connection.query_row("SELECT COUNT(*) FROM secrets", [], |row| {
                        row.get::<_, i64>(0)
                    })?,
                    connection.query_row("SELECT COUNT(*) FROM balance_queries", [], |row| {
                        row.get::<_, i64>(0)
                    })?,
                ))
            })
            .await
            .expect("owned rows");
        assert_eq!((secret_count, script_count), (1, 0));
    }

    #[tokio::test]
    async fn absent_codex_baseline_is_stored_without_bytes_or_mode() {
        let (_directory, database) = database();
        let baseline = database
            .capture_codex_baseline(false, Vec::new(), Some(0o600))
            .await
            .expect("baseline");
        assert!(!baseline.original_exists);
        assert!(baseline.raw_bytes.is_empty());
        assert_eq!(baseline.unix_mode, None);

        let stored = database
            .test_execute(|connection| {
                Ok(connection.query_row(
                    "SELECT raw_bytes IS NULL, unix_mode IS NULL FROM codex_baseline WHERE singleton = 1",
                    [],
                    |row| Ok((row.get::<_, bool>(0)?, row.get::<_, bool>(1)?)),
                )?)
            })
            .await
            .expect("stored baseline shape");
        assert_eq!(stored, (true, true));
    }

    #[tokio::test]
    async fn baseline_capture_initializes_recovery_once_and_updates_are_independent() {
        let (_directory, database) = database();
        let original = b"model = \"original\"\n".to_vec();
        database
            .capture_codex_baseline(true, original.clone(), Some(0o640))
            .await
            .expect("baseline");
        let initial = database
            .codex_recovery_config()
            .await
            .expect("recovery query")
            .expect("recovery");
        assert_eq!(initial.raw_bytes, original);
        assert_eq!(initial.unix_mode, Some(0o640));

        let updated = database
            .update_codex_recovery_config(false, b"ignored".to_vec(), Some(0o777))
            .await
            .expect("update recovery");
        assert!(!updated.original_exists);
        assert!(updated.raw_bytes.is_empty());
        assert_eq!(updated.unix_mode, None);
        database
            .capture_codex_baseline(true, b"model = \"later-connect\"\n".to_vec(), Some(0o600))
            .await
            .expect("repeat baseline capture");
        assert_eq!(
            database
                .codex_recovery_config()
                .await
                .expect("recovery after repeat capture")
                .expect("recovery"),
            updated
        );
        assert_eq!(
            database
                .codex_baseline()
                .await
                .expect("baseline query")
                .expect("baseline")
                .raw_bytes,
            original
        );
    }

    #[test]
    fn v18_migration_backfills_recovery_from_existing_baseline() {
        let mut connection = Connection::open_in_memory().expect("database");
        migrate_v1(&mut connection).expect("v1");
        migrate_v2(&mut connection).expect("v2");
        migrate_v3(&mut connection).expect("v3");
        migrate_v4(&mut connection).expect("v4");
        migrate_v5(&mut connection).expect("v5");
        migrate_v6(&mut connection).expect("v6");
        migrate_v7(&mut connection).expect("v7");
        migrate_v8(&mut connection).expect("v8");
        migrate_v9(&mut connection).expect("v9");
        migrate_v10(&mut connection).expect("v10");
        migrate_v11(&mut connection).expect("v11");
        migrate_v12(&mut connection).expect("v12");
        migrate_v13(&mut connection).expect("v13");
        migrate_v14(&mut connection).expect("v14");
        migrate_v15(&mut connection).expect("v15");
        migrate_v16(&mut connection).expect("v16");
        migrate_v17(&mut connection).expect("v17");
        connection
            .execute(
                "INSERT INTO codex_baseline (singleton, original_exists, raw_bytes, unix_mode, captured_at_ms) VALUES (1, 1, ?1, 416, 42)",
                [b"model = \"legacy\"\n".as_slice()],
            )
            .expect("legacy baseline");

        migrate_v18(&mut connection).expect("v18");

        let recovered: (bool, Vec<u8>, u32, i64) = connection
            .query_row(
                "SELECT original_exists, raw_bytes, unix_mode, updated_at_ms FROM codex_recovery_config WHERE singleton = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("recovery row");
        assert_eq!(
            recovered,
            (true, b"model = \"legacy\"\n".to_vec(), 0o640, 42)
        );
    }

    #[tokio::test]
    async fn retention_cleanup_and_clear_cascade_to_attempts() {
        let (_directory, database) = database();
        database
            .test_execute(|connection| {
                connection.execute_batch(
                    "
                    INSERT INTO proxy_requests (request_id, started_at_ms, streaming, completion_state, metadata_complete) VALUES ('old', 10, 0, 'failed', 1);
                    INSERT INTO proxy_requests (request_id, started_at_ms, streaming, completion_state, metadata_complete) VALUES ('new', 30, 0, 'completed', 1);
                    INSERT INTO upstream_attempts (attempt_id, request_id, attempt_index, route_id, route_name, started_at_ms, delivery_state) VALUES ('attempt-old', 'old', 0, 'route-old', 'Old', 10, 'none');
                    ",
                )?;
                Ok(())
            })
            .await
            .expect("fixtures");
        assert_eq!(database.cleanup_history(20).await.expect("cleanup"), 1);
        let (requests, attempts) = database
            .test_execute(|connection| {
                Ok((
                    connection.query_row("SELECT COUNT(*) FROM proxy_requests", [], |row| {
                        row.get::<_, i64>(0)
                    })?,
                    connection.query_row("SELECT COUNT(*) FROM upstream_attempts", [], |row| {
                        row.get::<_, i64>(0)
                    })?,
                ))
            })
            .await
            .expect("counts");
        assert_eq!((requests, attempts), (1, 0));
        assert_eq!(
            database.clear_history().await.expect("clear"),
            super::ClearHistoryResult {
                deleted_requests: 1,
                reclaim_succeeded: true,
            }
        );
        assert_eq!(
            database
                .history_summary()
                .await
                .expect("summary")
                .request_count,
            0
        );
    }

    #[tokio::test]
    async fn request_history_persists_allowlisted_metadata_and_reconstructs_latest_attempt() {
        let (_directory, database) = database();
        let route_id = RouteId::new();
        database
            .record_request_history(super::RequestHistoryRecord {
                request_id: "request-history-1".to_owned(),
                started_at_ms: 100,
                finished_at_ms: 150,
                turn_id: Some("turn-1".to_owned()),
                requested_model: Some("requested-model".to_owned()),
                reasoning_effort: Some("high".to_owned()),
                requested_service_tier: None,
                actual_model: Some("actual-model".to_owned()),
                actual_service_tier: None,
                final_route_id: Some(route_id.clone()),
                final_route_name: Some("Route snapshot".to_owned()),
                streaming: true,
                completion_state: crate::domain::CompletionState::Completed,
                http_status: Some(200),
                error_category: None,
                input_tokens: Some(3),
                output_tokens: Some(5),
                total_tokens: Some(8),
                cached_input_tokens: None,
                cache_write_input_tokens: None,
                total_latency_ms: Some(50),
                first_output_latency_ms: Some(10),
                metadata_complete: true,
                fallback_stop_reason: None,
                fallback_stop_target_route_id: None,
                fallback_stop_target_route_name: None,
                attempts: vec![super::AttemptHistoryRecord {
                    attempt_id: crate::domain::UpstreamAttemptId::new(),
                    attempt_index: 0,
                    attempt_role: super::AttemptRole::Ordinary,
                    route_id: route_id.clone(),
                    route_name: "Route snapshot".to_owned(),
                    started_at_ms: 100,
                    finished_at_ms: 150,
                    http_status: Some(200),
                    error_category: None,
                    delivery_state: crate::domain::DeliveryState::Completed,
                    actual_model: Some("actual-model".to_owned()),
                    forwarded_service_tier: None,
                    actual_service_tier: None,
                    input_tokens: Some(3),
                    output_tokens: Some(5),
                    total_tokens: Some(8),
                    cached_input_tokens: None,
                    cache_write_input_tokens: None,
                }],
            })
            .await
            .expect("history record");

        assert_eq!(
            database
                .history_summary()
                .await
                .expect("history summary")
                .request_count,
            1
        );
        let detail = database
            .usage_request_detail("request-history-1".to_owned())
            .await
            .expect("usage detail");
        assert_eq!(detail.request.reasoning_effort.as_deref(), Some("high"));
        assert!(detail.request.streaming);
        assert_eq!(detail.request.first_output_latency_ms, Some(10));
        assert_eq!(detail.request.finished_at_ms, Some(150));
        assert_eq!(
            database
                .latest_inference_attempts()
                .await
                .expect("latest attempts"),
            vec![super::LatestInferenceAttempt {
                route_id,
                finished_at_ms: 150,
                succeeded: true,
                error_category: None,
            }]
        );
    }

    #[tokio::test]
    async fn v4_migrates_through_v7_without_repricing_legacy_history() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("router.sqlite3");
        let mut connection = Connection::open(&path).expect("legacy connection");
        migrate_v1(&mut connection).expect("v1");
        migrate_v2(&mut connection).expect("v2");
        migrate_v3(&mut connection).expect("v3");
        migrate_v4(&mut connection).expect("v4");
        connection
            .execute(
                "INSERT INTO proxy_requests (
                    request_id, started_at_ms, finished_at_ms, requested_model, streaming,
                    completion_state, input_tokens, output_tokens, total_tokens,
                    metadata_complete
                 ) VALUES ('legacy', 10, 10, 'gpt-5', 0, 'completed', 1, 1, 2, 1)",
                [],
            )
            .expect("legacy history");
        drop(connection);

        let database = DatabaseExecutor::open(path).expect("migrate v6");
        let page = database
            .usage_history(super::UsageHistoryQuery {
                finished_at_or_after_ms: None,
                finished_at_or_before_ms: 10,
                completion_state: None,
                route_id: None,
                model_contains: None,
                cursor: None,
                limit: 50,
            })
            .await
            .expect("usage page");
        assert_eq!(page.rows.len(), 1);
        assert_eq!(page.rows[0].cost_status, None);
        assert_eq!(page.rows[0].upstream_cost_pico_usd, None);
        assert_eq!(page.rows[0].pricing_catalog_version, None);
        assert_eq!(page.rows[0].reasoning_effort, None);
    }

    #[tokio::test]
    async fn v5_migrates_through_v9_with_null_new_fields() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("router.sqlite3");
        let mut connection = Connection::open(&path).expect("legacy connection");
        migrate_v1(&mut connection).expect("v1");
        migrate_v2(&mut connection).expect("v2");
        migrate_v3(&mut connection).expect("v3");
        migrate_v4(&mut connection).expect("v4");
        super::migrate_v5(&mut connection).expect("v5");
        connection
            .execute(
                "INSERT INTO proxy_requests (
                    request_id, started_at_ms, streaming, completion_state, metadata_complete
                 ) VALUES ('v5-row', 10, 0, 'completed', 1)",
                [],
            )
            .expect("v5 row");
        drop(connection);

        let database = DatabaseExecutor::open(path).expect("migrate v9");
        let (reasoning, first_output, has_first_text): (Option<String>, Option<i64>, bool) =
            database
                .test_execute(|connection| {
                    let values = connection.query_row(
                        "SELECT reasoning_effort, first_output_latency_ms
                     FROM proxy_requests WHERE request_id = 'v5-row'",
                        [],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )?;
                    let has_first_text = connection
                        .prepare("SELECT first_text_output_latency_ms FROM proxy_requests")
                        .is_ok();
                    Ok((values.0, values.1, has_first_text))
                })
                .await
                .expect("v9 columns");
        assert_eq!(reasoning, None);
        assert_eq!(first_output, None);
        assert!(!has_first_text);
    }

    #[tokio::test]
    async fn v6_migrates_through_v11_with_null_first_output_latency() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("router.sqlite3");
        let mut connection = Connection::open(&path).expect("legacy connection");
        migrate_v1(&mut connection).expect("v1");
        migrate_v2(&mut connection).expect("v2");
        migrate_v3(&mut connection).expect("v3");
        migrate_v4(&mut connection).expect("v4");
        super::migrate_v5(&mut connection).expect("v5");
        super::migrate_v6(&mut connection).expect("v6");
        connection
            .execute(
                "INSERT INTO proxy_requests (
                    request_id, started_at_ms, streaming, completion_state, metadata_complete
                 ) VALUES ('v6-row', 10, 1, 'completed', 1)",
                [],
            )
            .expect("v6 row");
        drop(connection);

        let database = DatabaseExecutor::open(path).expect("migrate v9");
        let (version, first_output): (i64, Option<i64>) = database
            .test_execute(|connection| {
                Ok((
                    connection.pragma_query_value(None, "user_version", |row| row.get(0))?,
                    connection.query_row(
                        "SELECT first_output_latency_ms FROM proxy_requests
                         WHERE request_id = 'v6-row'",
                        [],
                        |row| row.get(0),
                    )?,
                ))
            })
            .await
            .expect("v9 migration result");
        assert_eq!(version, SCHEMA_VERSION);
        assert_eq!(first_output, None);
    }

    #[tokio::test]
    #[expect(
        clippy::too_many_lines,
        reason = "the migration regression verifies data, physical schema, and all query indexes"
    )]
    async fn v8_migrates_to_v11_preserving_latency_precedence_and_completion_indexes() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("router.sqlite3");
        let mut connection = Connection::open(&path).expect("legacy connection");
        migrate_v1(&mut connection).expect("v1");
        migrate_v2(&mut connection).expect("v2");
        migrate_v3(&mut connection).expect("v3");
        migrate_v4(&mut connection).expect("v4");
        super::migrate_v5(&mut connection).expect("v5");
        super::migrate_v6(&mut connection).expect("v6");
        super::migrate_v7(&mut connection).expect("v7");
        super::migrate_v8(&mut connection).expect("v8");
        connection
            .execute_batch(
                "
                INSERT INTO secrets (secret_id, kind, value, created_at_ms, updated_at_ms)
                VALUES ('v9-secret', 'route_api_key', X'6B6579', 1, 1);
                INSERT INTO routes (
                    route_id, display_name, display_name_key, base_url, secret_id,
                    service_tier_policy, sort_order, created_at_ms, updated_at_ms
                ) VALUES (
                    'v9-route', 'V9 Route', 'v9 route', 'https://example.invalid/v1',
                    'v9-secret', 'omit', 0, 1, 1
                );
                INSERT INTO proxy_requests (
                    request_id, started_at_ms, finished_at_ms, requested_model,
                    final_route_id, final_route_name, streaming, completion_state,
                    first_output_latency_ms, first_text_output_latency_ms,
                    pricing_catalog_version, cost_status, upstream_cost_pico_usd,
                    metadata_complete
                ) VALUES
                    ('authoritative', 100, 150, 'gpt-5', 'v9-route', 'V9 Route', 1,
                     'completed', 10, 20, 'catalog-a', 'exact', 123, 1),
                    ('copied', 110, 160, 'gpt-5', 'v9-route', 'V9 Route', 1,
                     'completed', NULL, 30, 'catalog-b', 'partial', 456, 1),
                    ('no-completion', 120, NULL, 'gpt-5', 'v9-route', 'V9 Route', 1,
                     'completed', NULL, 40, 'catalog-c', 'unavailable', NULL, 1);
                ",
            )
            .expect("v8 fixtures");
        drop(connection);

        let database = DatabaseExecutor::open(path).expect("migrate v9");
        let page = database
            .usage_history(super::UsageHistoryQuery {
                finished_at_or_after_ms: None,
                finished_at_or_before_ms: 160,
                completion_state: None,
                route_id: None,
                model_contains: None,
                cursor: None,
                limit: 50,
            })
            .await
            .expect("completion history");
        assert_eq!(
            page.rows
                .iter()
                .map(|row| (
                    row.request_id.as_str(),
                    row.finished_at_ms,
                    row.first_output_latency_ms
                ))
                .collect::<Vec<_>>(),
            vec![
                ("copied", Some(160), Some(30)),
                ("authoritative", Some(150), Some(10)),
            ]
        );
        assert_eq!(
            page.rows[0].pricing_catalog_version.as_deref(),
            Some("catalog-b")
        );
        let hidden = database
            .usage_request_detail("no-completion".to_owned())
            .await
            .expect("exact legacy detail");
        assert_eq!(hidden.request.finished_at_ms, None);
        assert_eq!(hidden.request.first_output_latency_ms, Some(40));
        assert_eq!(
            database
                .list_routes()
                .await
                .expect("route survives migration")[0]
                .service_tier_policy,
            ServiceTierPolicy::Omit
        );
        let (version, columns, indexes): (i64, Vec<String>, Vec<String>) = database
            .test_execute(|connection| {
                let version =
                    connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
                let mut column_statement =
                    connection.prepare("PRAGMA table_info(proxy_requests)")?;
                let columns = column_statement
                    .query_map([], |row| row.get(1))?
                    .collect::<Result<Vec<_>, _>>()?;
                let mut index_statement = connection.prepare(
                    "SELECT sql FROM sqlite_schema
                     WHERE type = 'index' AND name IN (
                         'proxy_requests_keyset_idx',
                         'proxy_requests_status_keyset_idx',
                         'proxy_requests_route_keyset_idx',
                         'proxy_requests_model_keyset_idx'
                     )
                     ORDER BY name",
                )?;
                let indexes = index_statement
                    .query_map([], |row| row.get(0))?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok((version, columns, indexes))
            })
            .await
            .expect("v9 schema");
        assert_eq!(version, SCHEMA_VERSION);
        assert!(
            !columns
                .iter()
                .any(|column| column == "first_text_output_latency_ms")
        );
        assert_eq!(indexes.len(), 4);
        assert!(indexes.iter().all(|sql| {
            sql.contains("finished_at_ms DESC, request_id DESC")
                && sql.contains("WHERE finished_at_ms IS NOT NULL")
        }));
    }

    #[tokio::test]
    async fn v7_migrates_routes_and_backfills_forwarded_service_tiers() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("router.sqlite3");
        let mut connection = Connection::open(&path).expect("legacy connection");
        migrate_v1(&mut connection).expect("v1");
        migrate_v2(&mut connection).expect("v2");
        migrate_v3(&mut connection).expect("v3");
        migrate_v4(&mut connection).expect("v4");
        super::migrate_v5(&mut connection).expect("v5");
        super::migrate_v6(&mut connection).expect("v6");
        super::migrate_v7(&mut connection).expect("v7");
        connection
            .execute_batch(
                "
                INSERT INTO secrets (secret_id, kind, value, created_at_ms, updated_at_ms)
                VALUES ('legacy-secret', 'route_api_key', X'6B6579', 1, 1);
                INSERT INTO routes (
                    route_id, display_name, display_name_key, base_url, secret_id,
                    sort_order, created_at_ms, updated_at_ms
                ) VALUES (
                    'legacy-route', 'Legacy', 'legacy', 'https://example.invalid/v1',
                    'legacy-secret', 0, 1, 1
                );
                INSERT INTO proxy_requests (
                    request_id, started_at_ms, requested_service_tier, streaming,
                    completion_state, metadata_complete
                ) VALUES
                    ('legacy-priority', 10, 'priority', 0, 'failed', 1),
                    ('legacy-no-tier', 20, NULL, 0, 'failed', 1);
                INSERT INTO upstream_attempts (
                    attempt_id, request_id, attempt_index, route_id, route_name,
                    started_at_ms, delivery_state
                ) VALUES
                    ('attempt-priority', 'legacy-priority', 0, 'legacy-route', 'Legacy', 10, 'none'),
                    ('attempt-no-tier', 'legacy-no-tier', 0, 'legacy-route', 'Legacy', 20, 'none');
                ",
            )
            .expect("v7 fixture");
        drop(connection);

        let database = DatabaseExecutor::open(path.clone()).expect("migrate v11");
        let routes = database.list_routes().await.expect("migrated routes");
        assert_eq!(routes.len(), 1);
        assert_eq!(
            routes[0].service_tier_policy,
            ServiceTierPolicy::Passthrough
        );
        let (version, forwarded): (i64, Vec<Option<String>>) = database
            .test_execute(|connection| {
                let version =
                    connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
                let mut statement = connection.prepare(
                    "SELECT forwarded_service_tier FROM upstream_attempts ORDER BY started_at_ms",
                )?;
                let forwarded = statement
                    .query_map([], |row| row.get(0))?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok((version, forwarded))
            })
            .await
            .expect("v9 values");
        assert_eq!(version, SCHEMA_VERSION);
        assert_eq!(forwarded, vec![Some("priority".to_owned()), None]);

        let manager = RecoveryManager::new(&path);
        fs::write(manager.recovery_dir(), b"blocks recovery publication")
            .expect("blocking recovery file");
        let coordinator =
            RecoveryCoordinator::start(manager, database.clone(), Arc::new(NoopRecoveryEventSink))
                .await;
        let health = coordinator.health();
        assert_eq!(health.kind, RecoveryHealthKind::Degraded);
        assert_eq!(
            health.last_failure,
            Some(RecoveryFailureCode::PublicationFailed)
        );
        assert_eq!(
            database
                .list_routes()
                .await
                .expect("usable current-schema routes")
                .len(),
            1
        );
        let version: i64 = database
            .test_execute(|connection| {
                Ok(connection.pragma_query_value(None, "user_version", |row| row.get(0))?)
            })
            .await
            .expect("usable current-schema version");
        assert_eq!(version, SCHEMA_VERSION);
        assert!(coordinator.shutdown(Duration::from_secs(1)).await);
    }

    #[tokio::test]
    async fn service_tier_policy_is_critical_and_unknown_storage_fails_closed() {
        let (_directory, database) = database();
        let created = database
            .create_route(route("Policy", "policy-key"))
            .await
            .expect("route");
        let created_revision = database
            .critical_revision()
            .await
            .expect("created revision");
        let update = || UpdateRouteInput {
            route_id: created.route_id.clone(),
            name: "Policy".to_owned(),
            base_url: "https://example.com/v1".to_owned(),
            api_key: ApiKey::parse("policy-key").expect("key"),
            service_tier_policy: ServiceTierPolicy::Omit,
            balance_query: Some(BalanceQueryInput {
                mode: BalanceQueryMode::CustomJs,
                enabled: true,
                custom_source: "({ request: {}, extractor: () => ({}) })".to_owned(),
            }),
            accept_script_risk: true,
        };
        database
            .update_route(update())
            .await
            .expect("policy update");
        assert_eq!(
            database.critical_revision().await.expect("policy revision"),
            created_revision + 1
        );
        assert_eq!(
            database
                .route_edit(created.route_id.clone())
                .await
                .expect("route edit")
                .route
                .service_tier_policy,
            ServiceTierPolicy::Omit
        );

        database.update_route(update()).await.expect("policy no-op");
        assert_eq!(
            database.critical_revision().await.expect("no-op revision"),
            created_revision + 1
        );

        database
            .test_execute(move |connection| {
                connection.pragma_update(None, "ignore_check_constraints", true)?;
                connection.execute(
                    "UPDATE routes SET service_tier_policy = 'unknown' WHERE route_id = ?1",
                    [created.route_id.as_str()],
                )?;
                Ok(())
            })
            .await
            .expect("inject corrupt policy");
        assert!(matches!(
            database.list_routes().await,
            Err(StorageError::Validation(error))
                if error.code == "service_tier_policy_invalid"
        ));
    }

    #[tokio::test]
    async fn usage_history_is_keyset_bounded_and_folds_attempt_costs() {
        let (_directory, database) = database();
        let route_id = RouteId::new();
        for (request_id, output) in [("request-b", 2), ("request-a", 1)] {
            database
                .record_request_history(super::RequestHistoryRecord {
                    request_id: request_id.to_owned(),
                    started_at_ms: 100,
                    finished_at_ms: 110,
                    turn_id: None,
                    requested_model: Some("gpt-5".to_owned()),
                    reasoning_effort: None,
                    requested_service_tier: None,
                    actual_model: Some("gpt-5".to_owned()),
                    actual_service_tier: Some("default".to_owned()),
                    final_route_id: Some(route_id.clone()),
                    final_route_name: Some("Retained route".to_owned()),
                    streaming: false,
                    completion_state: crate::domain::CompletionState::Completed,
                    http_status: Some(200),
                    error_category: None,
                    input_tokens: Some(3),
                    output_tokens: Some(output),
                    total_tokens: Some(3 + output),
                    cached_input_tokens: Some(1),
                    cache_write_input_tokens: None,
                    total_latency_ms: Some(10),
                    first_output_latency_ms: None,
                    metadata_complete: true,
                    fallback_stop_reason: None,
                    fallback_stop_target_route_id: None,
                    fallback_stop_target_route_name: None,
                    attempts: vec![super::AttemptHistoryRecord {
                        attempt_id: crate::domain::UpstreamAttemptId::new(),
                        attempt_index: if request_id == "request-b" { 65_536 } else { 0 },
                        attempt_role: super::AttemptRole::Ordinary,
                        route_id: route_id.clone(),
                        route_name: "Retained route".to_owned(),
                        started_at_ms: 100,
                        finished_at_ms: 110,
                        http_status: Some(200),
                        error_category: None,
                        delivery_state: crate::domain::DeliveryState::Completed,
                        actual_model: Some("gpt-5".to_owned()),
                        forwarded_service_tier: None,
                        actual_service_tier: Some("default".to_owned()),
                        input_tokens: Some(3),
                        output_tokens: Some(output),
                        total_tokens: Some(3 + output),
                        cached_input_tokens: Some(1),
                        cache_write_input_tokens: None,
                    }],
                })
                .await
                .expect("history");
        }
        let first = database
            .usage_history(super::UsageHistoryQuery {
                finished_at_or_after_ms: None,
                finished_at_or_before_ms: 110,
                completion_state: Some(crate::domain::CompletionState::Completed),
                route_id: Some(route_id.clone()),
                model_contains: Some("GPT-5".to_owned()),
                cursor: None,
                limit: 1,
            })
            .await
            .expect("first page");
        assert_eq!(first.rows[0].request_id, "request-b");
        assert_eq!(first.rows[0].cached_input_tokens, Some(1));
        assert_eq!(first.rows[0].cache_write_input_tokens, None);
        assert_eq!(first.rows[0].reasoning_effort, None);
        assert!(!first.rows[0].streaming);
        assert_eq!(first.rows[0].first_output_latency_ms, None);
        assert_eq!(first.total_rows, 2);
        assert_eq!(
            first.rows[0].cost_status,
            Some(crate::pricing::CostStatus::Exact)
        );
        let second = database
            .usage_history(super::UsageHistoryQuery {
                finished_at_or_after_ms: None,
                finished_at_or_before_ms: 110,
                completion_state: None,
                route_id: None,
                model_contains: None,
                cursor: first.next_cursor,
                limit: 1,
            })
            .await
            .expect("second page");
        assert_eq!(second.rows[0].request_id, "request-a");
        assert_eq!(second.next_cursor, None);
        assert_eq!(second.total_rows, 2);
        let detail = database
            .usage_request_detail("request-b".to_owned())
            .await
            .expect("detail");
        assert_eq!(detail.attempts.len(), 1);
        assert_eq!(detail.attempts[0].attempt_index, 65_536);
        assert_eq!(detail.attempts[0].cached_input_tokens, Some(1));
    }

    #[tokio::test]
    async fn usage_history_persists_priority_pricing_and_first_output() {
        let (_directory, database) = database();
        let route_id = RouteId::new();
        database
            .record_request_history(super::RequestHistoryRecord {
                request_id: "priority-request".to_owned(),
                started_at_ms: 100,
                finished_at_ms: 6_284,
                turn_id: None,
                requested_model: Some("gpt-5.6-sol".to_owned()),
                reasoning_effort: Some("high".to_owned()),
                requested_service_tier: Some("priority".to_owned()),
                actual_model: Some("gpt-5.6-sol".to_owned()),
                actual_service_tier: Some("default".to_owned()),
                final_route_id: Some(route_id.clone()),
                final_route_name: Some("Priority route".to_owned()),
                streaming: true,
                completion_state: crate::domain::CompletionState::Completed,
                http_status: Some(200),
                error_category: None,
                input_tokens: Some(60_014),
                output_tokens: Some(40),
                total_tokens: Some(60_054),
                cached_input_tokens: Some(59_136),
                cache_write_input_tokens: Some(0),
                total_latency_ms: Some(6_184),
                first_output_latency_ms: Some(1_720),
                metadata_complete: true,
                fallback_stop_reason: None,
                fallback_stop_target_route_id: None,
                fallback_stop_target_route_name: None,
                attempts: vec![super::AttemptHistoryRecord {
                    attempt_id: crate::domain::UpstreamAttemptId::new(),
                    attempt_index: 0,
                    attempt_role: super::AttemptRole::Ordinary,
                    route_id,
                    route_name: "Priority route".to_owned(),
                    started_at_ms: 100,
                    finished_at_ms: 6_284,
                    http_status: Some(200),
                    error_category: None,
                    delivery_state: crate::domain::DeliveryState::Completed,
                    actual_model: Some("gpt-5.6-sol".to_owned()),
                    forwarded_service_tier: Some("priority".to_owned()),
                    actual_service_tier: Some("default".to_owned()),
                    input_tokens: Some(60_014),
                    output_tokens: Some(40),
                    total_tokens: Some(60_054),
                    cached_input_tokens: Some(59_136),
                    cache_write_input_tokens: Some(0),
                }],
            })
            .await
            .expect("Priority history");

        let page = database
            .usage_history(super::UsageHistoryQuery {
                finished_at_or_after_ms: None,
                finished_at_or_before_ms: 6_284,
                completion_state: None,
                route_id: None,
                model_contains: None,
                cursor: None,
                limit: 50,
            })
            .await
            .expect("Priority usage page");
        assert_eq!(page.rows.len(), 1);
        assert_eq!(page.rows[0].first_output_latency_ms, Some(1_720));
        assert_eq!(page.rows[0].finished_at_ms, Some(6_284));
        assert_eq!(page.rows[0].actual_service_tier.as_deref(), Some("default"));
        assert_eq!(
            page.rows[0].pricing_catalog_version.as_deref(),
            Some(crate::pricing::PRIORITY_CATALOG_VERSION)
        );
        assert_eq!(
            page.rows[0].cost_status,
            Some(crate::pricing::CostStatus::Exact)
        );
        assert_eq!(page.rows[0].upstream_cost_pico_usd, Some(70_316_000_000));

        let detail = database
            .usage_request_detail("priority-request".to_owned())
            .await
            .expect("Priority usage detail");
        assert_eq!(detail.requested_service_tier.as_deref(), Some("priority"));
        assert_eq!(detail.actual_service_tier.as_deref(), Some("default"));
        assert_eq!(detail.attempts.len(), 1);
        assert_eq!(
            detail.attempts[0].pricing_catalog_version.as_deref(),
            Some(crate::pricing::PRIORITY_CATALOG_VERSION)
        );
        assert_eq!(detail.attempts[0].cost_pico_usd, Some(70_316_000_000));
    }

    #[tokio::test]
    async fn usage_history_prices_omitted_priority_from_forwarded_tier() {
        let (_directory, database) = database();
        let route_id = RouteId::new();
        database
            .record_request_history(super::RequestHistoryRecord {
                request_id: "omitted-priority-request".to_owned(),
                started_at_ms: 100,
                finished_at_ms: 200,
                turn_id: None,
                requested_model: Some("gpt-5".to_owned()),
                reasoning_effort: None,
                requested_service_tier: Some("priority".to_owned()),
                actual_model: Some("gpt-5".to_owned()),
                actual_service_tier: Some("default".to_owned()),
                final_route_id: Some(route_id.clone()),
                final_route_name: Some("Omit route".to_owned()),
                streaming: false,
                completion_state: crate::domain::CompletionState::Completed,
                http_status: Some(200),
                error_category: None,
                input_tokens: Some(3),
                output_tokens: Some(2),
                total_tokens: Some(5),
                cached_input_tokens: Some(1),
                cache_write_input_tokens: None,
                total_latency_ms: Some(100),
                first_output_latency_ms: None,
                metadata_complete: true,
                fallback_stop_reason: None,
                fallback_stop_target_route_id: None,
                fallback_stop_target_route_name: None,
                attempts: vec![super::AttemptHistoryRecord {
                    attempt_id: crate::domain::UpstreamAttemptId::new(),
                    attempt_index: 0,
                    attempt_role: super::AttemptRole::Ordinary,
                    route_id,
                    route_name: "Omit route".to_owned(),
                    started_at_ms: 100,
                    finished_at_ms: 200,
                    http_status: Some(200),
                    error_category: None,
                    delivery_state: crate::domain::DeliveryState::Completed,
                    actual_model: Some("gpt-5".to_owned()),
                    forwarded_service_tier: None,
                    actual_service_tier: Some("default".to_owned()),
                    input_tokens: Some(3),
                    output_tokens: Some(2),
                    total_tokens: Some(5),
                    cached_input_tokens: Some(1),
                    cache_write_input_tokens: None,
                }],
            })
            .await
            .expect("omitted Priority history");

        let page = database
            .usage_history(super::UsageHistoryQuery {
                finished_at_or_after_ms: None,
                finished_at_or_before_ms: 200,
                completion_state: None,
                route_id: None,
                model_contains: None,
                cursor: None,
                limit: 50,
            })
            .await
            .expect("usage page");
        assert_eq!(
            page.rows[0].pricing_catalog_version.as_deref(),
            Some(crate::pricing::CATALOG_VERSION)
        );

        let detail = database
            .usage_request_detail("omitted-priority-request".to_owned())
            .await
            .expect("usage detail");
        assert_eq!(detail.requested_service_tier.as_deref(), Some("priority"));
        assert_eq!(detail.attempts[0].forwarded_service_tier, None);
        assert_eq!(
            detail.attempts[0].actual_service_tier.as_deref(),
            Some("default")
        );
        assert_eq!(
            detail.attempts[0].pricing_catalog_version.as_deref(),
            Some(crate::pricing::CATALOG_VERSION)
        );
    }

    #[tokio::test]
    async fn usage_route_options_include_current_and_retained_deleted_routes() {
        let (_directory, database) = database();
        let current = database
            .create_route(route("Current route", "current-key"))
            .await
            .expect("current route");
        let deleted_route_id = RouteId::from_string("deleted-route".to_owned());
        database
            .test_execute({
                let deleted_route_id = deleted_route_id.clone();
                move |connection| {
                    connection.execute(
                        "INSERT INTO proxy_requests (
                            request_id, started_at_ms, final_route_id, final_route_name,
                            streaming, completion_state, metadata_complete
                         ) VALUES ('retained-request', 10, ?1, 'Deleted route', 0, 'completed', 1)",
                        [deleted_route_id.as_str()],
                    )?;
                    Ok(())
                }
            })
            .await
            .expect("retained history");

        let options = database.usage_route_options().await.expect("route options");
        assert_eq!(options.len(), 2);
        assert_eq!(options[0].route_id, current.route_id);
        assert_eq!(options[0].name, "Current route");
        assert!(!options[0].retained);
        assert_eq!(options[1].route_id, deleted_route_id);
        assert_eq!(options[1].name, "Deleted route");
        assert!(options[1].retained);
    }

    #[tokio::test]
    async fn usage_history_rejects_invalid_cursor_request_ids() {
        let (_directory, database) = database();
        for request_id in [String::new(), "x".repeat(129)] {
            let result = database
                .usage_history(super::UsageHistoryQuery {
                    finished_at_or_after_ms: None,
                    finished_at_or_before_ms: 100,
                    completion_state: None,
                    route_id: None,
                    model_contains: None,
                    cursor: Some(super::UsageHistoryCursor {
                        finished_at_ms: 100,
                        request_id,
                    }),
                    limit: 50,
                })
                .await;
            assert!(matches!(result, Err(StorageError::InvalidUsageQuery)));
        }
    }

    #[tokio::test]
    async fn usage_history_uses_anchored_literal_case_insensitive_model_contains_and_stable_count()
    {
        let (_directory, database) = database();
        database
            .test_execute(|connection| {
                for (request_id, finished_at_ms, model) in [
                    ("older", 90_i64, "plain-model"),
                    ("anchored", 100_i64, r"Prefix-Model_%\Path-Suffix"),
                    ("newer", 110_i64, r"Prefix-Model_%\Path-Suffix"),
                ] {
                    connection.execute(
                        "INSERT INTO proxy_requests (
                            request_id, started_at_ms, finished_at_ms, requested_model, streaming,
                            completion_state, metadata_complete
                         ) VALUES (?1, ?2, ?2, ?3, 0, 'completed', 1)",
                        rusqlite::params![request_id, finished_at_ms, model],
                    )?;
                }
                Ok(())
            })
            .await
            .expect("seed literal models");

        for fragment in ["PREFIX-MODEL", "%", "_", r"\path"] {
            let page = database
                .usage_history(super::UsageHistoryQuery {
                    finished_at_or_after_ms: None,
                    finished_at_or_before_ms: 100,
                    completion_state: None,
                    route_id: None,
                    model_contains: Some(fragment.to_owned()),
                    cursor: None,
                    limit: 50,
                })
                .await
                .expect("literal contains query");
            assert_eq!(page.total_rows, 1, "fragment {fragment}");
            assert_eq!(page.rows[0].request_id, "anchored");
        }

        let cursor_page = database
            .usage_history(super::UsageHistoryQuery {
                finished_at_or_after_ms: None,
                finished_at_or_before_ms: 100,
                completion_state: None,
                route_id: None,
                model_contains: Some("PREFIX-MODEL".to_owned()),
                cursor: Some(super::UsageHistoryCursor {
                    finished_at_ms: 100,
                    request_id: "anchored".to_owned(),
                }),
                limit: 50,
            })
            .await
            .expect("cursor page");
        assert!(cursor_page.rows.is_empty());
        assert_eq!(cursor_page.total_rows, 1);

        let empty = database
            .usage_history(super::UsageHistoryQuery {
                finished_at_or_after_ms: None,
                finished_at_or_before_ms: 100,
                completion_state: None,
                route_id: None,
                model_contains: Some("missing".to_owned()),
                cursor: None,
                limit: 50,
            })
            .await
            .expect("empty page");
        assert!(empty.rows.is_empty());
        assert_eq!(empty.total_rows, 0);
    }

    #[tokio::test]
    async fn usage_history_rejects_invalid_bounds_models_and_limits() {
        let (_directory, database) = database();
        for query in [
            super::UsageHistoryQuery {
                finished_at_or_after_ms: Some(101),
                finished_at_or_before_ms: 100,
                completion_state: None,
                route_id: None,
                model_contains: None,
                cursor: None,
                limit: 50,
            },
            super::UsageHistoryQuery {
                finished_at_or_after_ms: None,
                finished_at_or_before_ms: -1,
                completion_state: None,
                route_id: None,
                model_contains: None,
                cursor: None,
                limit: 50,
            },
            super::UsageHistoryQuery {
                finished_at_or_after_ms: None,
                finished_at_or_before_ms: 100,
                completion_state: None,
                route_id: None,
                model_contains: Some(String::new()),
                cursor: None,
                limit: 50,
            },
            super::UsageHistoryQuery {
                finished_at_or_after_ms: None,
                finished_at_or_before_ms: 100,
                completion_state: None,
                route_id: None,
                model_contains: Some("x".repeat(257)),
                cursor: None,
                limit: 101,
            },
        ] {
            assert!(matches!(
                database.usage_history(query).await,
                Err(StorageError::InvalidUsageQuery)
            ));
        }
    }

    #[tokio::test]
    #[ignore = "deterministic 365k-row performance evidence fixture"]
    async fn usage_history_and_statistics_365k_benchmark_and_explain_plan() {
        let (_directory, database) = database();
        database
            .test_execute(|connection| {
                let transaction = connection.transaction()?;
                {
                    let mut insert = transaction.prepare(
                        "INSERT INTO proxy_requests (
                            request_id, started_at_ms, finished_at_ms, requested_model, actual_model,
                            final_route_id, final_route_name, streaming, completion_state,
                            input_tokens, output_tokens, total_tokens, total_latency_ms,
                            metadata_complete, pricing_catalog_version, cost_status,
                            upstream_cost_pico_usd
                         ) VALUES (?1, ?2, ?2, ?3, ?3, ?4, ?5, 0, ?6, 10, 2, 12, 100,
                                   1, ?7, 'exact', 28000000)",
                    )?;
                    for index in 0..365_000_i64 {
                        let status = if index % 11 == 0 {
                            "failed"
                        } else {
                            "completed"
                        };
                        let route = format!("route-{}", index % 8);
                        insert.execute(rusqlite::params![
                            format!("benchmark-{index:06}"),
                            1_700_000_000_000_i64 + index,
                            if index % 3 == 0 {
                                "gpt-5"
                            } else {
                                "gpt-5-mini"
                            },
                            route,
                            format!("Route {}", index % 8),
                            status,
                            crate::pricing::CATALOG_VERSION,
                        ])?;
                    }
                }
                transaction.commit()?;
                Ok(())
            })
            .await
            .expect("seed benchmark fixture");

        for (label, cursor) in [
            ("first", None),
            (
                "middle",
                Some(super::UsageHistoryCursor {
                    finished_at_ms: 1_700_000_350_000,
                    request_id: "benchmark-350000".to_owned(),
                }),
            ),
            (
                "late",
                Some(super::UsageHistoryCursor {
                    finished_at_ms: 1_700_000_300_100,
                    request_id: "benchmark-300100".to_owned(),
                }),
            ),
        ] {
            let query = super::UsageHistoryQuery {
                finished_at_or_after_ms: Some(1_700_000_300_000),
                finished_at_or_before_ms: 1_700_000_364_999,
                completion_state: Some(crate::domain::CompletionState::Completed),
                route_id: None,
                model_contains: Some("gpt-5".to_owned()),
                cursor,
                limit: 100,
            };
            let _ = database.usage_history(query.clone()).await.expect("warmup");
            let started = std::time::Instant::now();
            let page = database.usage_history(query).await.expect("measured page");
            let elapsed = started.elapsed();
            assert!(page.rows.len() <= 100);
            assert!(page.total_rows > 0);
            eprintln!(
                "V0.3B 365k warm anchored {label} page + count: {elapsed:?}, total {}",
                page.total_rows
            );
        }

        assert_usage_statistics_365k(&database).await;

        let plan = database
            .test_execute(|connection| {
                let mut statement = connection.prepare(
                    "EXPLAIN QUERY PLAN SELECT request_id FROM proxy_requests
                     WHERE completion_state = 'completed' AND finished_at_ms IS NOT NULL
                     ORDER BY finished_at_ms DESC, request_id DESC LIMIT 100",
                )?;
                Ok(statement
                    .query_map([], |row| row.get::<_, String>(3))?
                    .collect::<Result<Vec<_>, _>>()?)
            })
            .await
            .expect("explain plan");
        eprintln!("V0.3B explain: {plan:?}");
        assert!(
            plan.iter()
                .any(|line| line.contains("proxy_requests_status_keyset_idx"))
        );
    }

    #[test]
    fn future_schema_versions_fail_closed_without_rewriting_the_database() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("router.sqlite3");
        let connection = rusqlite::Connection::open(&path).expect("fixture database");
        let future_version = SCHEMA_VERSION + 1;
        connection
            .pragma_update(None, "user_version", future_version)
            .expect("future version");
        drop(connection);

        assert!(matches!(
            DatabaseExecutor::open(&path),
            Err(StorageError::FutureSchema)
        ));
        let connection = rusqlite::Connection::open(path).expect("reopen fixture");
        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("version");
        assert_eq!(version, future_version);
    }

    #[cfg(unix)]
    #[test]
    fn database_paths_receive_private_modes() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().expect("temporary directory");
        let parent = directory.path().join("private");
        let path = parent.join("router.sqlite3");
        let _database = DatabaseExecutor::open(&path).expect("database opens");
        assert_eq!(
            std::fs::metadata(parent)
                .expect("parent")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(path)
                .expect("database")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    #[tokio::test]
    async fn activation_rejects_unknown_routes() {
        let (_directory, database) = database();
        assert!(matches!(
            database.activate_route(RouteId::new()).await,
            Err(StorageError::NotFound)
        ));
    }

    #[tokio::test]
    async fn usage_statistics_counts_only_completed_rows_and_preserves_recorded_values() {
        let (_directory, database) = database();
        database
            .test_execute(|connection| {
                connection.execute_batch(
                    "
                    INSERT INTO proxy_requests (
                        request_id, started_at_ms, finished_at_ms, requested_model, actual_model,
                        final_route_id, final_route_name, streaming, completion_state,
                        input_tokens, output_tokens, total_tokens, cached_input_tokens,
                        cache_write_input_tokens, cost_status, upstream_cost_pico_usd,
                        metadata_complete
                    ) VALUES
                        ('completed-a', 1000, 3600000, 'model-a', 'model-a', 'route-a', 'Route A', 0,
                         'completed', 10, 4, 14, 2, 3, 'unavailable', 123, 1),
                        ('completed-b', 1000, 7200000, 'model-b', 'model-b', 'route-b', 'Route B', 1,
                         'completed', NULL, NULL, NULL, NULL, NULL, NULL, NULL, 1),
                        ('failed', 1000, 10800000, 'model-a', 'model-a', 'route-a', 'Route A', 0,
                         'failed', 100, 100, 200, 10, 0, 'exact', 999, 1),
                        ('cancelled', 1000, 10800000, 'model-a', 'model-a', 'route-a', 'Route A', 0,
                         'cancelled', 100, 100, 200, 10, 0, 'exact', 999, 1),
                        ('no-upstream', 1000, 10800000, 'model-a', 'model-a', NULL, NULL, 0,
                         'no_upstream', 100, 100, 200, 10, 0, 'exact', 999, 1);
                    ",
                )?;
                Ok(())
            })
            .await
            .expect("fixtures");

        let result = database
            .usage_statistics(statistics_query(Some(3_600_000), 10_800_000, "UTC"))
            .await
            .expect("statistics");

        assert_eq!(result.matched_request_count, 2);
        assert_eq!(result.tokens.total, 14);
        assert_eq!(result.tokens.uncached_input, 8);
        assert_eq!(result.tokens.cached_input, 2);
        assert_eq!(result.tokens.cache_write_input, 3);
        assert_eq!(result.tokens.output, 4);
        assert_eq!(result.cost_pico_usd, 123);
        assert_eq!(result.trend.len(), 2);
        assert_eq!(result.trend[0].request_count, 1);
        assert_eq!(result.trend[1].request_count, 1);
        assert_eq!(result.attribution.len(), 2);
        assert_eq!(result.attribution[0].label, "Route A");
        assert_eq!(result.attribution[0].share_percent, "50.0");
        assert_eq!(result.attribution[1].label, "Route B");
        assert_eq!(result.attribution[1].share_percent, "50.0");

        let mut model_query = statistics_query(Some(3_600_000), 10_800_000, "UTC");
        model_query.attribution_dimension = UsageStatisticsAttributionDimension::Model;
        model_query.attribution_metric = UsageStatisticsAttributionMetric::Tokens;
        let model_result = database
            .usage_statistics(model_query)
            .await
            .expect("model statistics");
        assert_eq!(model_result.attribution[0].label, "model-a");
        assert_eq!(model_result.attribution[0].value, 14);
        assert_eq!(model_result.attribution[0].share_percent, "100.0");
        assert_eq!(model_result.attribution[1].label, "model-b");
        assert_eq!(model_result.attribution[1].value, 0);
        assert_eq!(model_result.attribution[1].share_percent, "0.0");

        let mut filtered_query = statistics_query(Some(3_600_000), 10_800_000, "UTC");
        filtered_query.route_id = Some(RouteId::from_string("route-a".to_owned()));
        filtered_query.model_contains = Some("MODEL-A".to_owned());
        let filtered = database
            .usage_statistics(filtered_query)
            .await
            .expect("filtered statistics");
        assert_eq!(filtered.matched_request_count, 1);

        let mut literal_query = statistics_query(Some(3_600_000), 10_800_000, "UTC");
        literal_query.model_contains = Some("%".to_owned());
        let literal = database
            .usage_statistics(literal_query)
            .await
            .expect("literal model statistics");
        assert_eq!(literal.matched_request_count, 0);
    }

    #[tokio::test]
    async fn usage_statistics_preserves_zero_and_empty_sums() {
        let (_directory, database) = database();
        database
            .test_execute(|connection| {
                connection.execute_batch(
                    "
                    INSERT INTO proxy_requests (
                        request_id, started_at_ms, finished_at_ms, streaming,
                        completion_state, input_tokens, output_tokens, total_tokens,
                        cached_input_tokens, cache_write_input_tokens,
                        upstream_cost_pico_usd, metadata_complete
                    ) VALUES
                        ('zero-values', 1, 10, 0, 'completed', 0, 0, 0, 0, 0, 0, 1),
                        ('empty-values', 2, 20, 0, 'completed', NULL, NULL, NULL, NULL, NULL, NULL, 1);
                    ",
                )?;
                Ok(())
            })
            .await
            .expect("zero fixtures");

        let result = database
            .usage_statistics(statistics_query(Some(1), 20, "UTC"))
            .await
            .expect("zero statistics");
        assert_eq!(result.matched_request_count, 2);
        assert_eq!(result.tokens, super::UsageStatisticsTokens::default());
        assert_eq!(result.cost_pico_usd, 0);
    }

    #[tokio::test]
    async fn usage_statistics_returns_empty_trend_and_attribution_without_successes() {
        let (_directory, database) = database();
        let result = database
            .usage_statistics(statistics_query(Some(1), 20, "UTC"))
            .await
            .expect("empty statistics");
        assert_eq!(result.matched_request_count, 0);
        assert_eq!(result.tokens, super::UsageStatisticsTokens::default());
        assert_eq!(result.cost_pico_usd, 0);
        assert!(result.trend.is_empty());
        assert!(result.attribution.is_empty());
    }

    #[test]
    fn usage_statistics_handles_fall_back_dst_hour_labels() {
        let lower = utc_ms(2024, 11, 3, 4, 0);
        let upper = utc_ms(2024, 11, 3, 8, 0);
        let windows = statistics_bucket_windows(
            lower,
            upper,
            "America/New_York".parse().expect("time zone"),
            UsageStatisticsGranularity::Hour,
        )
        .expect("windows");

        assert_eq!(windows.len(), 4);
        assert!(windows[1].label.contains("-04:00"));
        assert!(windows[2].label.contains("-05:00"));
        assert_eq!(windows[0].started_at_ms, lower);
        assert_eq!(windows[3].finished_at_ms, upper);
    }

    #[test]
    fn usage_statistics_handles_spring_forward_and_partial_calendar_edges() {
        let spring = statistics_bucket_windows(
            utc_ms(2024, 3, 10, 5, 0),
            utc_ms(2024, 3, 10, 9, 0),
            "America/New_York".parse().expect("time zone"),
            UsageStatisticsGranularity::Hour,
        )
        .expect("spring windows");
        assert_eq!(
            spring
                .iter()
                .map(|window| window.label.as_str())
                .collect::<Vec<_>>(),
            ["03/10 00:00", "03/10 01:00", "03/10 03:00", "03/10 04:00"]
        );

        let daily_lower = utc_ms(2025, 12, 31, 12, 0);
        let daily_upper = utc_ms(2026, 1, 2, 6, 0);
        let daily = statistics_bucket_windows(
            daily_lower,
            daily_upper,
            Tz::UTC,
            UsageStatisticsGranularity::Day,
        )
        .expect("daily windows");
        assert_eq!(daily.first().expect("first").started_at_ms, daily_lower);
        assert_eq!(daily.last().expect("last").finished_at_ms, daily_upper);
        assert_eq!(
            daily
                .iter()
                .map(|window| window.label.as_str())
                .collect::<Vec<_>>(),
            ["12/31", "01/01", "01/02"]
        );

        let monthly = statistics_bucket_windows(
            utc_ms(2025, 12, 15, 12, 0),
            utc_ms(2026, 2, 10, 6, 0),
            Tz::UTC,
            UsageStatisticsGranularity::Month,
        )
        .expect("monthly windows");
        assert_eq!(
            monthly
                .iter()
                .map(|window| window.label.as_str())
                .collect::<Vec<_>>(),
            ["2025/12", "2026/01", "2026/02"]
        );
    }

    #[tokio::test]
    async fn usage_statistics_rejects_invalid_bounds_models_and_time_zones() {
        let (_directory, database) = database();
        let mut queries = vec![
            statistics_query(None, -1, "UTC"),
            statistics_query(Some(2), 1, "UTC"),
            statistics_query(None, i64::MAX, "UTC"),
            statistics_query(None, 1, ""),
            statistics_query(None, 1, "Not/A_Time_Zone"),
            statistics_query(None, 1, &"x".repeat(129)),
        ];
        let mut empty_model = statistics_query(None, 1, "UTC");
        empty_model.model_contains = Some(String::new());
        queries.push(empty_model);
        let mut long_model = statistics_query(None, 1, "UTC");
        long_model.model_contains = Some("x".repeat(257));
        queries.push(long_model);

        for query in queries {
            assert!(matches!(
                database.usage_statistics(query).await,
                Err(StorageError::InvalidUsageQuery)
            ));
        }
    }

    #[test]
    fn usage_statistics_attribution_is_top_five_plus_other_with_stable_ties() {
        let mut aggregates = BTreeMap::new();
        for (key, label, count) in [
            ("route:a", "A", 1),
            ("route:b", "B", 1),
            ("route:c", "C", 1),
            ("route:d", "D", 1),
            ("route:e", "E", 1),
            ("route:f", "F", 1),
        ] {
            let totals = StatisticsTotals {
                request_count: count,
                ..StatisticsTotals::default()
            };
            aggregates.insert(
                key.to_owned(),
                AttributionAggregate {
                    label: label.to_owned(),
                    totals,
                },
            );
        }
        let summary = StatisticsTotals {
            request_count: 6,
            ..StatisticsTotals::default()
        };

        let result = statistics_attribution(
            aggregates,
            UsageStatisticsAttributionMetric::Requests,
            &summary,
        )
        .expect("attribution");

        assert_eq!(result.len(), 6);
        assert_eq!(result[0].label, "A");
        assert_eq!(result[4].label, "E");
        assert!(result[5].is_other);
        assert_eq!(result[5].value, "1".parse::<u64>().unwrap());
    }

    #[test]
    fn usage_statistics_overflow_is_rejected() {
        let mut total = u64::MAX;
        assert!(matches!(
            super::checked_statistics_add(&mut total, Some(1)),
            Err(StorageError::UsageStatisticsOverflow)
        ));
    }

    #[tokio::test]
    async fn usage_statistics_rejects_overflow_from_persisted_rows() {
        let (_directory, database) = database();
        database
            .test_execute(|connection| {
                connection.execute_batch(
                    "
                    INSERT INTO proxy_requests (
                        request_id, started_at_ms, finished_at_ms, streaming,
                        completion_state, total_tokens, metadata_complete
                    ) VALUES
                        ('large-a', 1, 1, 0, 'completed', 9223372036854775807, 1),
                        ('large-b', 2, 2, 0, 'completed', 9223372036854775807, 1),
                        ('large-c', 3, 3, 0, 'completed', 9223372036854775807, 1);
                    ",
                )?;
                Ok(())
            })
            .await
            .expect("large fixtures");

        assert!(matches!(
            database
                .usage_statistics(statistics_query(Some(1), 3, "UTC"))
                .await,
            Err(StorageError::UsageStatisticsOverflow)
        ));
    }
}
