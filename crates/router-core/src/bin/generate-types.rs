use std::{env, error::Error, fs, path::PathBuf};

use router_core::BuildInfoDto;
use router_core::{
    app_api::{
        ApplicationUpdateFailureDto, ApplicationUpdateOperationDto, ApplicationUpdateProgressDto,
        ApplicationUpdateReleaseDto, ApplicationUpdateSnapshotDto, BalanceQueryEditDto,
        BalanceQuerySettingsDto, BalanceTestInputDto, CodexBaselineSummaryDto,
        CodexImagesMcpRepairPreviewDto, CodexModelDto, CodexModelsActivation,
        CodexRecoveryResetPreviewDto, CodexRecoverySummaryDto, CodexRecoveryUpdatePreviewDto,
        CodexRestartNoticeDto, FallbackStopReasonDto, HistorySummaryDto,
        ImagesGenerationSettingsDto, MenuSnapshotDto, MetadataFailureDto, RecoveryCandidateDto,
        RecoveryHealthDto, RecoverySnapshotDto, ReorderRoutesAndFallbackInputDto,
        ReplaceCodexModelsResult, RouteActivationPreviewDto, RouteActivationResultDto,
        RouteCatalogMode, RouteEditDto, RouteSaveInputDto, RouteSaveResultDto, RoutingDecisionDto,
        SettingsSnapshotDto, UpdateImagesGenerationSettingsInputDto, UsageAttemptDto, UsageCostDto,
        UsageCostStateDto, UsageFastStatusDto, UsageHistoryCursorDto, UsageHistoryPageDto,
        UsageHistoryQueryDto, UsageHistoryRowDto, UsageRequestDetailDto, UsageRouteOptionDto,
        UsageStatisticsAttributionDimensionDto, UsageStatisticsAttributionDto,
        UsageStatisticsAttributionMetricDto, UsageStatisticsBucketDto, UsageStatisticsDto,
        UsageStatisticsGranularityDto, UsageStatisticsQueryDto, UsageStatisticsTokensDto,
        UsageTokensDto,
    },
    balance::{
        BalanceBatchPhase, BalanceDisplaySnapshot, BalanceDisplayStatus, BalanceError,
        BalanceErrorCategory, BalanceErrorStage, BalanceQueryMode, BalanceRefreshBatchState,
        BalanceResult, BalanceTrigger,
    },
    codex_config::{CodexConfigStatus, ConfigOperationResult},
    domain::{
        AppearancePreference, CompletionState, DeliveryState, InferenceFailureReason,
        InferenceOutcome, InferenceStatus, InferenceStatusKind, ProxyRequestMetadata,
        ProxyRuntimeStatus, ReachabilityResult, ReachabilityStatus, RouteMoveDirection,
        ServiceTierPolicy, UpstreamAttemptMetadata,
    },
    lifecycle::{AppLifecycleIssue, AppLifecyclePhase, AppLifecycleSnapshot},
    recovery::{DatabaseStartupIssue, RecoveryHealthKind},
    state::{
        BootstrapSnapshotDto, FallbackStateDto, IpcErrorDto, MutationResultDto, RouteSummaryDto,
        StateArea, StateChangedEventDto,
    },
};
use ts_rs::TS;

fn main() -> Result<(), Box<dyn Error>> {
    let output = env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .ok_or("usage: generate-types <output-directory>")?;

    fs::create_dir_all(&output)?;
    ApplicationUpdateFailureDto::export_all_to(&output)?;
    ApplicationUpdateOperationDto::export_all_to(&output)?;
    ApplicationUpdateProgressDto::export_all_to(&output)?;
    ApplicationUpdateReleaseDto::export_all_to(&output)?;
    ApplicationUpdateSnapshotDto::export_all_to(&output)?;
    BuildInfoDto::export_all_to(&output)?;
    AppLifecycleIssue::export_all_to(&output)?;
    AppLifecyclePhase::export_all_to(&output)?;
    AppLifecycleSnapshot::export_all_to(&output)?;
    BalanceQuerySettingsDto::export_all_to(&output)?;
    BalanceQueryMode::export_all_to(&output)?;
    BalanceQueryEditDto::export_all_to(&output)?;
    BalanceTestInputDto::export_all_to(&output)?;
    CodexBaselineSummaryDto::export_all_to(&output)?;
    CodexRecoveryResetPreviewDto::export_all_to(&output)?;
    CodexRecoverySummaryDto::export_all_to(&output)?;
    CodexRecoveryUpdatePreviewDto::export_all_to(&output)?;
    CodexImagesMcpRepairPreviewDto::export_all_to(&output)?;
    CodexModelDto::export_all_to(&output)?;
    CodexModelsActivation::export_all_to(&output)?;
    CodexRestartNoticeDto::export_all_to(&output)?;
    FallbackStopReasonDto::export_all_to(&output)?;
    HistorySummaryDto::export_all_to(&output)?;
    ImagesGenerationSettingsDto::export_all_to(&output)?;
    MenuSnapshotDto::export_all_to(&output)?;
    MetadataFailureDto::export_all_to(&output)?;
    RecoveryCandidateDto::export_all_to(&output)?;
    RecoveryHealthDto::export_all_to(&output)?;
    RecoverySnapshotDto::export_all_to(&output)?;
    ReplaceCodexModelsResult::export_all_to(&output)?;
    ReorderRoutesAndFallbackInputDto::export_all_to(&output)?;
    RouteEditDto::export_all_to(&output)?;
    RouteActivationPreviewDto::export_all_to(&output)?;
    RouteActivationResultDto::export_all_to(&output)?;
    RouteCatalogMode::export_all_to(&output)?;
    RouteSaveInputDto::export_all_to(&output)?;
    RouteSaveResultDto::export_all_to(&output)?;
    RoutingDecisionDto::export_all_to(&output)?;
    SettingsSnapshotDto::export_all_to(&output)?;
    UpdateImagesGenerationSettingsInputDto::export_all_to(&output)?;
    UsageAttemptDto::export_all_to(&output)?;
    UsageCostDto::export_all_to(&output)?;
    UsageCostStateDto::export_all_to(&output)?;
    UsageFastStatusDto::export_all_to(&output)?;
    UsageHistoryCursorDto::export_all_to(&output)?;
    UsageHistoryPageDto::export_all_to(&output)?;
    UsageHistoryQueryDto::export_all_to(&output)?;
    UsageHistoryRowDto::export_all_to(&output)?;
    UsageRequestDetailDto::export_all_to(&output)?;
    UsageRouteOptionDto::export_all_to(&output)?;
    UsageStatisticsAttributionDimensionDto::export_all_to(&output)?;
    UsageStatisticsAttributionDto::export_all_to(&output)?;
    UsageStatisticsAttributionMetricDto::export_all_to(&output)?;
    UsageStatisticsBucketDto::export_all_to(&output)?;
    UsageStatisticsDto::export_all_to(&output)?;
    UsageStatisticsGranularityDto::export_all_to(&output)?;
    UsageStatisticsQueryDto::export_all_to(&output)?;
    UsageStatisticsTokensDto::export_all_to(&output)?;
    UsageTokensDto::export_all_to(&output)?;
    BalanceBatchPhase::export_all_to(&output)?;
    BalanceDisplaySnapshot::export_all_to(&output)?;
    BalanceDisplayStatus::export_all_to(&output)?;
    BalanceError::export_all_to(&output)?;
    BalanceErrorCategory::export_all_to(&output)?;
    BalanceErrorStage::export_all_to(&output)?;
    BalanceRefreshBatchState::export_all_to(&output)?;
    BalanceResult::export_all_to(&output)?;
    BalanceTrigger::export_all_to(&output)?;
    CodexConfigStatus::export_all_to(&output)?;
    ConfigOperationResult::export_all_to(&output)?;
    CompletionState::export_all_to(&output)?;
    AppearancePreference::export_all_to(&output)?;
    DeliveryState::export_all_to(&output)?;
    InferenceFailureReason::export_all_to(&output)?;
    InferenceOutcome::export_all_to(&output)?;
    InferenceStatus::export_all_to(&output)?;
    InferenceStatusKind::export_all_to(&output)?;
    ProxyRequestMetadata::export_all_to(&output)?;
    ProxyRuntimeStatus::export_all_to(&output)?;
    ReachabilityResult::export_all_to(&output)?;
    ReachabilityStatus::export_all_to(&output)?;
    RouteMoveDirection::export_all_to(&output)?;
    ServiceTierPolicy::export_all_to(&output)?;
    UpstreamAttemptMetadata::export_all_to(&output)?;
    DatabaseStartupIssue::export_all_to(&output)?;
    RecoveryHealthKind::export_all_to(&output)?;
    BootstrapSnapshotDto::export_all_to(&output)?;
    FallbackStateDto::export_all_to(&output)?;
    IpcErrorDto::export_all_to(&output)?;
    MutationResultDto::export_all_to(&output)?;
    RouteSummaryDto::export_all_to(&output)?;
    StateArea::export_all_to(&output)?;
    StateChangedEventDto::export_all_to(&output)?;
    Ok(())
}
