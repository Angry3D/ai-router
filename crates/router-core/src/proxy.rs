use std::{
    collections::HashSet,
    io::Read,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use arc_swap::ArcSwap;
use async_trait::async_trait;
use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use futures_util::StreamExt;
use rmcp::transport::{
    StreamableHttpServerConfig, StreamableHttpService,
    streamable_http_server::session::local::LocalSessionManager,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use thiserror::Error;
use tokio::{
    net::TcpListener,
    sync::{Semaphore, oneshot, watch},
    task::JoinHandle,
};
use ts_rs::TS;
use uuid::Uuid;

use crate::{
    domain::{
        ApiKey, BaseUrl, CompletionState, ImagesGenerationTimeout, ReachabilityResult,
        ReachabilityStatus, RouteId, ServiceTierPolicy,
    },
    storage::RequestHistoryRecord,
};

use self::history::{
    NoopHistorySink, NoopRuntimeDiagnosticSink, bounded_string, now_millis, parse_turn_id,
};

mod fallback;
mod health;
mod history;
mod images;
mod sse;
pub(crate) mod upstream;

mod activity;

pub use activity::{
    LogicalRequestActivityPhase, LogicalRequestActivityReporter, LogicalRequestActivitySink,
    LogicalRequestActivityTracker, LogicalRequestActivityTransition,
    NoopLogicalRequestActivitySink, RequestActivityDisposition,
};
pub use health::{
    ACTIVATION_WRITE_RETRY_DELAY, ActivatedSkipHealth, ActivatedSkipKind, CandidateHealth,
    FAILURE_THRESHOLD, HealthActivationProof, HealthActivationReservation, HealthAttemptRef,
    HealthChangeSink, HealthChangeSink as RouteHealthChangeSink, HealthFailureClass,
    LaterProbeLeaseResult, MonotonicClock, NoopHealthChangeSink, PendingProof, ProbeCompletion,
    ProbeLease, ProbeLeaseResult, RECOVERY_EVIDENCE_DEADLINE, RecoveryOrigin, RouteHealthRegistry,
    RouteHealthSnapshot, StrikeResult, SystemMonotonicClock, TripLease,
};
pub use history::{
    AsyncHistoryRecorder, HistorySink, HistorySummaryChangeSink, InferenceStatusChangeSink,
    InferenceStatusService, MetadataFailureSnapshot, RuntimeDiagnosticCode,
    RuntimeDiagnosticComponent, RuntimeDiagnosticEvent, RuntimeDiagnosticSink,
};
pub use images::{
    ImageAssetChangeSink, ImagesGenerationService, McpImageAssetCleanupResult,
    McpImageAssetMaintenanceError, McpImageAssetManager, McpImageAssetSummary,
    NoopImageAssetChangeSink,
};
pub use upstream::{ResponsesForwarder, UpstreamForwarderConfig};

pub const MAX_REQUEST_WIRE_BYTES: usize = 200 * 1024 * 1024;
pub const MAX_REQUEST_DECODED_BYTES: usize = 200 * 1024 * 1024;

pub struct RouteSnapshot {
    pub route_id: RouteId,
    pub name: String,
    pub base_url: BaseUrl,
    pub api_key: Arc<ApiKey>,
    pub service_tier_policy: ServiceTierPolicy,
    pub fallback_excluded_models: Arc<HashSet<String>>,
}

pub struct RoutingSnapshot {
    pub active: Option<Arc<RouteSnapshot>>,
    pub participants: Vec<Arc<RouteSnapshot>>,
    pub enabled: bool,
    pub selection_generation: u64,
    pub health_generation: u64,
    pub config_revision: u64,
    pub images_generation_enabled: bool,
    pub images_route: Option<Arc<RouteSnapshot>>,
    pub images_generation_timeout: Duration,
}

impl RoutingSnapshot {
    #[must_use]
    pub fn active_participant_index(&self) -> Option<usize> {
        let active = self.active.as_ref()?;
        self.participants
            .iter()
            .position(|route| route.route_id == active.route_id)
    }

    #[must_use]
    pub fn next_after(&self, route_id: &RouteId) -> Option<Arc<RouteSnapshot>> {
        let index = self
            .participants
            .iter()
            .position(|route| &route.route_id == route_id)?;
        self.participants.get(index + 1).cloned()
    }
}

struct RoutingSnapshotStoreInner {
    snapshot: ArcSwap<RoutingSnapshot>,
    revision: AtomicU64,
    changed: watch::Sender<u64>,
}

#[derive(Clone)]
pub struct RoutingSnapshotStore {
    inner: Arc<RoutingSnapshotStoreInner>,
}

impl Default for RoutingSnapshotStore {
    fn default() -> Self {
        Self::new(RoutingSnapshot {
            active: None,
            participants: Vec::new(),
            enabled: false,
            selection_generation: 0,
            health_generation: 0,
            config_revision: 0,
            images_generation_enabled: false,
            images_route: None,
            images_generation_timeout: ImagesGenerationTimeout::default().duration(),
        })
    }
}

impl RoutingSnapshotStore {
    #[must_use]
    pub fn new(snapshot: RoutingSnapshot) -> Self {
        let (changed, _) = watch::channel(0);
        Self {
            inner: Arc::new(RoutingSnapshotStoreInner {
                snapshot: ArcSwap::from_pointee(snapshot),
                revision: AtomicU64::new(0),
                changed,
            }),
        }
    }

    #[must_use]
    pub fn load(&self) -> Arc<RoutingSnapshot> {
        self.inner.snapshot.load_full()
    }

    pub fn store(&self, snapshot: Arc<RoutingSnapshot>) {
        self.inner.snapshot.store(snapshot);
        let revision = self.inner.revision.fetch_add(1, Ordering::AcqRel) + 1;
        self.inner.changed.send_replace(revision);
    }

    #[must_use]
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.inner.changed.subscribe()
    }
}

#[derive(Clone)]
pub struct FallbackActivationRequest {
    pub request_id: String,
    pub routing: Arc<RoutingSnapshot>,
    pub current_route_id: RouteId,
    pub target_route: Arc<RouteSnapshot>,
    pub requested_model: String,
    pub skipped_routes: Vec<FallbackActivationSkip>,
    pub mode: FallbackActivationMode,
    pub health_proof: Option<HealthActivationProof>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FallbackActivationMode {
    Advance,
    AdvanceRecovered,
    Recover,
}

#[derive(Clone)]
pub struct FallbackActivationSkip {
    pub route: Arc<RouteSnapshot>,
    pub kind: ActivatedSkipKind,
}

pub trait RequestTransitionSink: Send + Sync {
    fn request_terminal(&self, request_id: &str);
}

pub struct NoopRequestTransitionSink;

impl RequestTransitionSink for NoopRequestTransitionSink {
    fn request_terminal(&self, _request_id: &str) {}
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FallbackActivationError {
    Persistence,
}

#[async_trait]
pub trait FallbackActivator: Send + Sync {
    async fn activate_next(
        &self,
        request: FallbackActivationRequest,
    ) -> Result<Option<Arc<RoutingSnapshot>>, FallbackActivationError>;
}

pub struct NoopFallbackActivator;

#[async_trait]
impl FallbackActivator for NoopFallbackActivator {
    async fn activate_next(
        &self,
        _request: FallbackActivationRequest,
    ) -> Result<Option<Arc<RoutingSnapshot>>, FallbackActivationError> {
        Ok(None)
    }
}

#[derive(Clone)]
pub struct ValidatedProxyRequest {
    pub request_id: String,
    pub started_at_ms: i64,
    pub request_started: Instant,
    pub turn_id: Option<String>,
    pub activity_reporter: Option<LogicalRequestActivityReporter>,
    pub request_declares_local_shell: bool,
    pub body: Bytes,
    pub body_without_service_tier: Option<Bytes>,
    pub model: String,
    pub reasoning_effort: Option<String>,
    pub service_tier: Option<String>,
    pub stream: bool,
    pub route: Arc<RouteSnapshot>,
    pub routing: Arc<RoutingSnapshot>,
    pub headers: HeaderMap,
}

#[async_trait]
pub trait UpstreamRequestHandler: Send + Sync {
    async fn handle(&self, request: ValidatedProxyRequest) -> Response;
}

#[derive(Clone)]
pub struct ProxyIngressState {
    gateway_token_digest: [u8; 32],
    routing: RoutingSnapshotStore,
    upstream: Arc<dyn UpstreamRequestHandler>,
    history: Arc<dyn HistorySink>,
    diagnostics: Arc<dyn RuntimeDiagnosticSink>,
    activity: LogicalRequestActivityTracker,
    wire_limit: usize,
    decoded_limit: usize,
    images: ImagesGenerationService,
    mcp_image_assets: Option<McpImageAssetManager>,
    image_asset_change_sink: Arc<dyn ImageAssetChangeSink>,
}

impl ProxyIngressState {
    #[must_use]
    pub fn new(gateway_token: &str, upstream: Arc<dyn UpstreamRequestHandler>) -> Self {
        let routing = RoutingSnapshotStore::default();
        Self {
            gateway_token_digest: Sha256::digest(gateway_token.as_bytes()).into(),
            images: ImagesGenerationService::new(routing.clone()),
            routing,
            upstream,
            history: Arc::new(NoopHistorySink),
            diagnostics: Arc::new(NoopRuntimeDiagnosticSink),
            activity: LogicalRequestActivityTracker::default(),
            wire_limit: MAX_REQUEST_WIRE_BYTES,
            decoded_limit: MAX_REQUEST_DECODED_BYTES,
            mcp_image_assets: None,
            image_asset_change_sink: Arc::new(NoopImageAssetChangeSink),
        }
    }

    pub fn set_active_route(&self, route: Option<Arc<RouteSnapshot>>) {
        let participants = route.iter().cloned().collect();
        self.routing.store(Arc::new(RoutingSnapshot {
            active: route,
            participants,
            enabled: false,
            selection_generation: 0,
            health_generation: 0,
            config_revision: 0,
            images_generation_enabled: false,
            images_route: None,
            images_generation_timeout: ImagesGenerationTimeout::default().duration(),
        }));
    }

    #[must_use]
    pub fn with_routing_store(mut self, routing: RoutingSnapshotStore) -> Self {
        self.images = ImagesGenerationService::new(routing.clone());
        self.routing = routing;
        self
    }

    #[must_use]
    pub fn with_mcp_image_asset_root(mut self, root: PathBuf) -> Self {
        self.mcp_image_assets = Some(McpImageAssetManager::new(root, Arc::new(Semaphore::new(1))));
        self
    }

    #[must_use]
    pub fn with_mcp_image_assets(mut self, assets: McpImageAssetManager) -> Self {
        self.mcp_image_assets = Some(assets);
        self
    }

    #[must_use]
    pub fn with_image_asset_change_sink(mut self, sink: Arc<dyn ImageAssetChangeSink>) -> Self {
        self.image_asset_change_sink = sink;
        self
    }

    pub fn set_routing_snapshot(&self, snapshot: Arc<RoutingSnapshot>) {
        self.routing.store(snapshot);
    }

    #[must_use]
    pub fn with_runtime_sinks(
        mut self,
        history: Arc<dyn HistorySink>,
        diagnostics: Arc<dyn RuntimeDiagnosticSink>,
    ) -> Self {
        self.history = history;
        self.diagnostics = diagnostics;
        self
    }

    #[must_use]
    pub fn with_activity_tracker(mut self, activity: LogicalRequestActivityTracker) -> Self {
        self.activity = activity;
        self
    }

    #[cfg(test)]
    fn with_limits(mut self, wire_limit: usize, decoded_limit: usize) -> Self {
        self.wire_limit = wire_limit;
        self.decoded_limit = decoded_limit;
        self
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct LocalErrorDto {
    pub error: LocalErrorBodyDto,
    pub request_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct LocalErrorBodyDto {
    pub message: String,
    #[serde(rename = "type")]
    #[ts(rename = "type")]
    pub error_type: String,
    pub code: String,
}

pub fn build_proxy_router(state: ProxyIngressState) -> Router {
    let images = state.images.clone();
    let image_assets = state.mcp_image_assets.clone();
    let image_asset_change_sink = Arc::clone(&state.image_asset_change_sink);
    let mcp_service = StreamableHttpService::new(
        move || {
            Ok(images::ImageMcpServer::new(
                images.clone(),
                image_assets.clone(),
                Arc::clone(&image_asset_change_sink),
            ))
        },
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default(),
    );
    Router::new()
        .route("/health", get(health_handler))
        .route("/v1/models", get(models_handler))
        .route("/v1/responses", post(responses_handler))
        .route("/v1/images/generations", post(images_handler))
        .nest_service("/mcp", mcp_service)
        .fallback(authenticated_not_found)
        .method_not_allowed_fallback(authenticated_method_not_allowed)
        .layer(middleware::from_fn_with_state(state.clone(), authenticate))
        .with_state(state)
}

async fn images_handler(State(state): State<ProxyIngressState>, request: Request) -> Response {
    let headers = request.headers().clone();
    let body = match read_images_body(&state, request).await {
        Ok(body) => body,
        Err(failure) => return images_failure_response(failure),
    };
    match state.images.forward(body, &headers).await {
        Ok(result) => {
            let mut response = (result.status, Body::from(result.body)).into_response();
            *response.headers_mut() = result.headers;
            response
        }
        Err(failure) => images_failure_response(failure),
    }
}

async fn read_images_body(
    state: &ProxyIngressState,
    request: Request,
) -> Result<Bytes, images::ImagesGenerationFailure> {
    use images::{ImagesGenerationFailure, ImagesGenerationFailureKind};
    if request
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .is_some_and(|length| length > state.wire_limit as u64)
    {
        return Err(ImagesGenerationFailure::new(
            ImagesGenerationFailureKind::InvalidRequest,
        ));
    }
    let encoding = request
        .headers()
        .get(header::CONTENT_ENCODING)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("identity")
        .trim()
        .to_ascii_lowercase();
    if !matches!(encoding.as_str(), "" | "identity" | "zstd") {
        return Err(ImagesGenerationFailure::new(
            ImagesGenerationFailureKind::InvalidRequest,
        ));
    }
    let mut wire = Vec::new();
    let mut stream = request.into_body().into_data_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| {
            ImagesGenerationFailure::new(ImagesGenerationFailureKind::InvalidRequest)
        })?;
        if wire.len().saturating_add(chunk.len()) > state.wire_limit {
            return Err(ImagesGenerationFailure::new(
                ImagesGenerationFailureKind::InvalidRequest,
            ));
        }
        wire.extend_from_slice(&chunk);
    }
    if encoding == "zstd" {
        decode_zstd_bounded(wire, state.decoded_limit)
            .await
            .map(Bytes::from)
            .map_err(|_| ImagesGenerationFailure::new(ImagesGenerationFailureKind::InvalidRequest))
    } else if wire.len() > state.decoded_limit {
        Err(ImagesGenerationFailure::new(
            ImagesGenerationFailureKind::InvalidRequest,
        ))
    } else {
        Ok(Bytes::from(wire))
    }
}

fn images_failure_response(failure: images::ImagesGenerationFailure) -> Response {
    local_error_with_request_id(
        failure.kind.status(),
        failure.kind.code(),
        failure.kind.message(),
        failure.request_id,
    )
}

async fn health_handler() -> impl IntoResponse {
    Json(serde_json::json!({ "status": "ok" }))
}

async fn models_handler() -> impl IntoResponse {
    Json(serde_json::json!({ "models": [] }))
}

async fn authenticate(
    State(state): State<ProxyIngressState>,
    request: Request,
    next: Next,
) -> Response {
    if request.method() == Method::GET && request.uri().path() == "/health" {
        return next.run(request).await;
    }
    let valid = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .is_some_and(|candidate| {
            let candidate_digest: [u8; 32] = Sha256::digest(candidate.as_bytes()).into();
            bool::from(candidate_digest.ct_eq(&state.gateway_token_digest))
        });
    if !valid {
        state.diagnostics.emit(RuntimeDiagnosticEvent {
            component: RuntimeDiagnosticComponent::ProxyIngress,
            code: RuntimeDiagnosticCode::InvalidLocalGatewayToken,
            request_id: None,
            route_id: None,
            http_status: Some(StatusCode::UNAUTHORIZED.as_u16()),
        });
        let mut response = local_error(
            StatusCode::UNAUTHORIZED,
            "invalid_local_gateway_token",
            "Local gateway authentication failed.",
        );
        response
            .headers_mut()
            .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
        return response;
    }
    next.run(request).await
}

async fn responses_handler(State(state): State<ProxyIngressState>, request: Request) -> Response {
    let request_id = Uuid::new_v4().to_string();
    let started_at_ms = now_millis();
    let request_started = Instant::now();
    let turn_id = parse_turn_id(request.headers());
    match validate_request(
        &state,
        request,
        request_id.clone(),
        started_at_ms,
        request_started,
        turn_id.clone(),
    )
    .await
    {
        Ok(mut request) => {
            let activity = state.activity.acquire_turn(request.turn_id.as_deref());
            request.activity_reporter = activity
                .as_ref()
                .map(activity::LogicalRequestActivityGuard::reporter);
            let response = state.upstream.handle(request).await;
            match activity {
                Some(activity) => response.map(|body| {
                    Body::new(activity::LogicalRequestActivityBody::new(body, activity))
                }),
                None => response,
            }
        }
        Err(failure) => {
            let finished_at_ms = now_millis();
            let completion_state = if failure.code == "no_upstream_route" {
                CompletionState::NoUpstream
            } else {
                CompletionState::Failed
            };
            let _ = state.history.try_record(RequestHistoryRecord {
                request_id: request_id.clone(),
                started_at_ms,
                finished_at_ms,
                turn_id,
                requested_model: failure.model.map(|model| bounded_string(model, 512)),
                reasoning_effort: failure.reasoning_effort,
                requested_service_tier: None,
                actual_model: None,
                actual_service_tier: None,
                final_route_id: None,
                final_route_name: None,
                streaming: failure.streaming,
                completion_state,
                http_status: Some(failure.status.as_u16()),
                error_category: Some(failure.code.to_owned()),
                input_tokens: None,
                output_tokens: None,
                total_tokens: None,
                cached_input_tokens: None,
                cache_write_input_tokens: None,
                total_latency_ms: Some(finished_at_ms.saturating_sub(started_at_ms)),
                first_output_latency_ms: None,
                metadata_complete: failure.metadata_complete,
                fallback_stop_reason: None,
                fallback_stop_target_route_id: None,
                fallback_stop_target_route_name: None,
                attempts: Vec::new(),
            });
            state.diagnostics.emit(RuntimeDiagnosticEvent {
                component: RuntimeDiagnosticComponent::ProxyIngress,
                code: if failure.code == "no_upstream_route" {
                    RuntimeDiagnosticCode::NoUpstreamRoute
                } else {
                    RuntimeDiagnosticCode::InvalidRequest
                },
                request_id: Some(request_id.clone()),
                route_id: None,
                http_status: Some(failure.status.as_u16()),
            });
            local_error_with_request_id(failure.status, failure.code, &failure.message, request_id)
        }
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "one ingress decoder keeps bounded body and metadata validation ordered"
)]
async fn validate_request(
    state: &ProxyIngressState,
    request: Request,
    request_id: String,
    started_at_ms: i64,
    request_started: Instant,
    turn_id: Option<String>,
) -> Result<ValidatedProxyRequest, IngressFailure> {
    if let Some(length) = request
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        && length > state.wire_limit as u64
    {
        return Err(ingress_failure(
            StatusCode::PAYLOAD_TOO_LARGE,
            "request_body_too_large",
            "Request body exceeds the local limit.",
        ));
    }
    let headers = request.headers().clone();
    let encoding = headers
        .get(header::CONTENT_ENCODING)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("identity")
        .trim()
        .to_ascii_lowercase();
    if !matches!(encoding.as_str(), "" | "identity" | "zstd") {
        return Err(ingress_failure(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported_content_encoding",
            "Request Content-Encoding is unsupported.",
        ));
    }

    let mut wire = Vec::new();
    let mut stream = request.into_body().into_data_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| {
            ingress_failure(
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "Request body could not be read.",
            )
        })?;
        if wire.len().saturating_add(chunk.len()) > state.wire_limit {
            return Err(ingress_failure(
                StatusCode::PAYLOAD_TOO_LARGE,
                "request_body_too_large",
                "Request body exceeds the local limit.",
            ));
        }
        wire.extend_from_slice(&chunk);
    }

    let decoded = if encoding == "zstd" {
        decode_zstd_bounded(wire, state.decoded_limit).await?
    } else {
        if wire.len() > state.decoded_limit {
            return Err(ingress_failure(
                StatusCode::PAYLOAD_TOO_LARGE,
                "request_body_too_large",
                "Decoded request body exceeds the local limit.",
            ));
        }
        wire
    };
    let metadata: RequestMetadata = serde_json::from_slice(&decoded).map_err(|_| {
        ingress_failure(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "Request body must be valid Responses JSON.",
        )
    })?;
    validate_request_model(&metadata.model)?;
    let request_declares_local_shell = metadata.request_declares_local_shell;
    let reasoning_effort = request_reasoning_effort(metadata.reasoning_effort);
    let routing = state.routing.load();
    let route = routing.active.clone().ok_or_else(|| {
        ingress_failure(
            StatusCode::SERVICE_UNAVAILABLE,
            "no_upstream_route",
            "No active upstream route is selected.",
        )
        .with_request_metadata(
            metadata.model.clone(),
            metadata.stream.unwrap_or(false),
            reasoning_effort.clone(),
        )
    })?;
    let body_without_service_tier = service_tier_omitted_body(&decoded, &routing)?;

    Ok(ValidatedProxyRequest {
        request_id,
        started_at_ms,
        request_started,
        turn_id,
        activity_reporter: None,
        request_declares_local_shell,
        body: Bytes::from(decoded),
        body_without_service_tier,
        model: metadata.model,
        reasoning_effort,
        service_tier: metadata.service_tier.map(|value| bounded_string(value, 64)),
        stream: metadata.stream.unwrap_or(false),
        route,
        routing,
        headers,
    })
}

fn service_tier_omitted_body(
    decoded: &[u8],
    routing: &RoutingSnapshot,
) -> Result<Option<Bytes>, IngressFailure> {
    if routing
        .active
        .iter()
        .chain(routing.participants.iter())
        .any(|route| route.service_tier_policy == ServiceTierPolicy::Omit)
    {
        remove_top_level_service_tier(decoded)
    } else {
        Ok(None)
    }
}

fn remove_top_level_service_tier(decoded: &[u8]) -> Result<Option<Bytes>, IngressFailure> {
    let mut value: serde_json::Value = serde_json::from_slice(decoded).map_err(|_| {
        ingress_failure(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "Request body must be valid Responses JSON.",
        )
    })?;
    let object = value.as_object_mut().ok_or_else(|| {
        ingress_failure(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "Request body must be valid Responses JSON.",
        )
    })?;
    if object.remove("service_tier").is_none() {
        return Ok(None);
    }
    serde_json::to_vec(&value)
        .map(Bytes::from)
        .map(Some)
        .map_err(|_| {
            ingress_failure(
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "Request body must be valid Responses JSON.",
            )
        })
}

fn validate_request_model(model: &str) -> Result<(), IngressFailure> {
    if model.trim().is_empty() {
        Err(ingress_failure(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "Request model is required.",
        ))
    } else {
        Ok(())
    }
}

async fn decode_zstd_bounded(wire: Vec<u8>, limit: usize) -> Result<Vec<u8>, IngressFailure> {
    let result = tokio::task::spawn_blocking(move || {
        let decompressor = zstd::stream::read::Decoder::new(wire.as_slice())
            .map_err(|_| ZstdDecodeError::Invalid)?;
        let mut decoded_body = Vec::new();
        decompressor
            .take((limit as u64).saturating_add(1))
            .read_to_end(&mut decoded_body)
            .map_err(|_| ZstdDecodeError::Invalid)?;
        if decoded_body.len() > limit {
            return Err(ZstdDecodeError::TooLarge);
        }
        Ok(decoded_body)
    })
    .await
    .map_err(|_| {
        ingress_failure(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            "Request decoding failed.",
        )
    })?;
    result.map_err(|error| match error {
        ZstdDecodeError::Invalid => ingress_failure(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "Request body contains invalid zstd data.",
        ),
        ZstdDecodeError::TooLarge => ingress_failure(
            StatusCode::PAYLOAD_TOO_LARGE,
            "request_body_too_large",
            "Decoded request body exceeds the local limit.",
        ),
    })
}

enum ZstdDecodeError {
    Invalid,
    TooLarge,
}

struct IngressFailure {
    status: StatusCode,
    code: &'static str,
    message: String,
    model: Option<String>,
    reasoning_effort: Option<String>,
    streaming: bool,
    metadata_complete: bool,
}

impl IngressFailure {
    fn with_request_metadata(
        mut self,
        model: String,
        streaming: bool,
        reasoning_effort: Option<String>,
    ) -> Self {
        self.model = Some(model);
        self.reasoning_effort = reasoning_effort;
        self.streaming = streaming;
        self.metadata_complete = true;
        self
    }
}

fn ingress_failure(status: StatusCode, code: &'static str, message: &str) -> IngressFailure {
    IngressFailure {
        status,
        code,
        message: message.to_owned(),
        model: None,
        reasoning_effort: None,
        streaming: false,
        metadata_complete: false,
    }
}

#[derive(Deserialize)]
struct RequestMetadata {
    model: String,
    service_tier: Option<String>,
    stream: Option<bool>,
    #[serde(
        default,
        rename = "tools",
        deserialize_with = "deserialize_declares_local_shell"
    )]
    request_declares_local_shell: bool,
    #[serde(
        default,
        rename = "reasoning",
        deserialize_with = "deserialize_reasoning_effort"
    )]
    reasoning_effort: Option<String>,
}

fn deserialize_declares_local_shell<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(value.as_array().is_some_and(|tools| {
        tools.iter().any(|tool| {
            tool.get("type").and_then(serde_json::Value::as_str) == Some("shell")
                && tool
                    .get("environment")
                    .and_then(|environment| environment.get("type"))
                    .and_then(serde_json::Value::as_str)
                    == Some("local")
        })
    }))
}

fn deserialize_reasoning_effort<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let reasoning = serde_json::Value::deserialize(deserializer)?;
    Ok(reasoning
        .as_object()
        .and_then(|value| value.get("effort"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned))
}

fn request_reasoning_effort(reasoning_effort: Option<String>) -> Option<String> {
    reasoning_effort.and_then(|value| {
        let value = value.trim();
        (!value.is_empty()).then(|| bounded_string(value.to_owned(), 64))
    })
}

async fn authenticated_not_found(State(state): State<ProxyIngressState>) -> Response {
    let request_id = Uuid::new_v4().to_string();
    state.diagnostics.emit(RuntimeDiagnosticEvent {
        component: RuntimeDiagnosticComponent::ProxyIngress,
        code: RuntimeDiagnosticCode::InvalidRequest,
        request_id: Some(request_id.clone()),
        route_id: None,
        http_status: Some(StatusCode::NOT_FOUND.as_u16()),
    });
    local_error_with_request_id(
        StatusCode::NOT_FOUND,
        "not_found",
        "Local route was not found.",
        request_id,
    )
}

async fn authenticated_method_not_allowed(
    State(state): State<ProxyIngressState>,
    method: Method,
) -> Response {
    let _ = method;
    let request_id = Uuid::new_v4().to_string();
    state.diagnostics.emit(RuntimeDiagnosticEvent {
        component: RuntimeDiagnosticComponent::ProxyIngress,
        code: RuntimeDiagnosticCode::InvalidRequest,
        request_id: Some(request_id.clone()),
        route_id: None,
        http_status: Some(StatusCode::METHOD_NOT_ALLOWED.as_u16()),
    });
    local_error_with_request_id(
        StatusCode::METHOD_NOT_ALLOWED,
        "method_not_allowed",
        "HTTP method is not allowed for this local route.",
        request_id,
    )
}

fn local_error(status: StatusCode, code: &str, message: &str) -> Response {
    local_error_with_request_id(status, code, message, Uuid::new_v4().to_string())
}

fn local_error_with_request_id(
    status: StatusCode,
    code: &str,
    message: &str,
    request_id: String,
) -> Response {
    (
        status,
        Json(LocalErrorDto {
            error: LocalErrorBodyDto {
                message: message.to_owned(),
                error_type: "invalid_request_error".to_owned(),
                code: code.to_owned(),
            },
            request_id,
        }),
    )
        .into_response()
}

pub struct ProxyServerHandle {
    address: SocketAddr,
    shutdown: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<std::io::Result<()>>>,
}

impl ProxyServerHandle {
    /// Binds the configured IPv4 loopback port and starts serving.
    ///
    /// # Errors
    ///
    /// Returns the listener bind error without selecting another port.
    pub async fn start(port: u16, router: Router) -> Result<Self, std::io::Error> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port)).await?;
        Ok(Self::from_listener(listener, router))
    }

    fn from_listener(listener: TcpListener, router: Router) -> Self {
        let address = listener
            .local_addr()
            .unwrap_or_else(|_| SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0));
        let (shutdown, receiver) = oneshot::channel();
        let task = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async {
                    let _ = receiver.await;
                })
                .await
        });
        Self {
            address,
            shutdown: Some(shutdown),
            task: Some(task),
        }
    }

    #[must_use]
    pub const fn address(&self) -> SocketAddr {
        self.address
    }

    pub async fn shutdown(mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}

impl Drop for ProxyServerHandle {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

#[async_trait]
pub trait ProxyPortStore: Send + Sync {
    async fn persist_port(&self, port: u16) -> Result<(), ProxyPortError>;
}

#[derive(Debug, Error)]
pub enum ProxyPortError {
    #[error("proxy port is invalid")]
    InvalidPort,
    #[error("proxy port is unavailable")]
    PortUnavailable,
    #[error("proxy port persistence failed")]
    PersistenceFailed,
}

/// Pre-binds and persists a new port before replacing the old listener.
///
/// # Errors
///
/// Returns validation, bind, or persistence errors while leaving the current
/// handle untouched.
pub async fn transition_proxy_port(
    current: &mut ProxyServerHandle,
    new_port: u16,
    router: Router,
    store: &dyn ProxyPortStore,
) -> Result<(), ProxyPortError> {
    transition_proxy_port_with_listener_replaced(current, new_port, router, store, || {}).await
}

/// Pre-binds and persists a new port, invokes `on_listener_replaced` after the
/// replacement becomes authoritative, then gracefully drains the old listener.
///
/// # Errors
///
/// Returns validation, bind, or persistence errors while leaving the current
/// handle untouched and without invoking `on_listener_replaced`.
pub async fn transition_proxy_port_with_listener_replaced<F>(
    current: &mut ProxyServerHandle,
    new_port: u16,
    router: Router,
    store: &dyn ProxyPortStore,
    on_listener_replaced: F,
) -> Result<(), ProxyPortError>
where
    F: FnOnce(),
{
    if new_port == 0 || new_port == current.address().port() {
        return Err(ProxyPortError::InvalidPort);
    }
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, new_port))
        .await
        .map_err(|_| ProxyPortError::PortUnavailable)?;
    store.persist_port(new_port).await?;
    let replacement = ProxyServerHandle::from_listener(listener, router);
    let old = std::mem::replace(current, replacement);
    on_listener_replaced();
    old.shutdown().await;
    Ok(())
}

pub struct ReachabilityProbe {
    client: reqwest::Client,
    attempt_timeout: Duration,
    slow_threshold: Duration,
}

impl ReachabilityProbe {
    /// Creates the no-key reachability client.
    ///
    /// # Errors
    ///
    /// Returns a client-construction error.
    pub fn new() -> Result<Self, reqwest::Error> {
        Ok(Self {
            client: reqwest::Client::builder().build()?,
            attempt_timeout: Duration::from_secs(8),
            slow_threshold: Duration::from_secs(6),
        })
    }

    #[cfg(test)]
    fn with_timing(mut self, attempt_timeout: Duration, slow_threshold: Duration) -> Self {
        self.attempt_timeout = attempt_timeout;
        self.slow_threshold = slow_threshold;
        self
    }

    /// Checks the final Responses endpoint without credentials or a body.
    ///
    /// # Errors
    ///
    /// Returns a field-specific validation error when the supplied Base URL is
    /// invalid or incompatible with the Responses API.
    pub async fn check(
        &self,
        base_url: &str,
    ) -> Result<ReachabilityResult, crate::domain::ValidationError> {
        let base_url = BaseUrl::parse(base_url)?;
        let inference_url = base_url.inference_url();
        for attempt in 0..2 {
            let started = Instant::now();
            let result = self
                .client
                .get(&inference_url)
                .header(header::ACCEPT, "*/*")
                .header(header::ACCEPT_ENCODING, "identity")
                .timeout(self.attempt_timeout)
                .send()
                .await;
            match result {
                Ok(response) => {
                    let path_not_found = response.status() == StatusCode::NOT_FOUND;
                    drop(response);
                    let elapsed = started.elapsed();
                    return Ok(ReachabilityResult {
                        status: if path_not_found {
                            ReachabilityStatus::PathNotFound
                        } else if elapsed > self.slow_threshold {
                            ReachabilityStatus::Slow
                        } else {
                            ReachabilityStatus::Reachable
                        },
                        ttfb_ms: Some(elapsed.as_millis().try_into().unwrap_or(u64::MAX)),
                        error_category: path_not_found.then(|| "path_not_found".to_owned()),
                    });
                }
                Err(error) if error.is_timeout() && attempt == 0 => {}
                Err(error) => {
                    return Ok(unreachable_result(if error.is_timeout() {
                        "timeout"
                    } else if error.is_connect() {
                        "connection"
                    } else {
                        "network"
                    }));
                }
            }
        }
        Ok(unreachable_result("timeout"))
    }
}

fn unreachable_result(category: &str) -> ReachabilityResult {
    ReachabilityResult {
        status: ReachabilityStatus::Unreachable,
        ttfb_ms: None,
        error_category: Some(category.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        convert::Infallible,
        sync::{
            Mutex, OnceLock,
            atomic::{AtomicUsize, Ordering},
        },
        task::Poll,
    };

    use axum::{
        body::{Body, to_bytes},
        http::{Request as HttpRequest, Uri},
    };
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use futures_util::stream::poll_fn;
    use tempfile::TempDir;
    use tower::ServiceExt;

    use super::*;

    const TOKEN: &str = "local-gateway-token";

    fn valid_png_fixture() -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, 1, 1);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().expect("PNG header");
            writer
                .write_image_data(&[0x12, 0x34, 0x56, 0xff])
                .expect("PNG pixels");
            writer.finish().expect("PNG end");
        }
        bytes
    }

    #[test]
    fn reasoning_effort_projection_trims_bounds_and_ignores_absent_values() {
        let bounded = request_reasoning_effort(Some(format!("  {}  ", "x".repeat(80))));
        assert_eq!(bounded.as_deref(), Some("x".repeat(64).as_str()));
        assert_eq!(request_reasoning_effort(Some("   ".to_owned())), None);
        assert_eq!(request_reasoning_effort(None), None);
    }

    #[test]
    fn reasoning_effort_projection_ignores_non_string_shapes_without_rejecting_the_request() {
        for reasoning in [
            serde_json::Value::Null,
            serde_json::json!("high"),
            serde_json::json!({ "effort": 42 }),
            serde_json::json!({ "effort": { "nested": "private" } }),
            serde_json::json!({ "summary": "ignored sibling" }),
        ] {
            let metadata = serde_json::from_value::<RequestMetadata>(serde_json::json!({
                "model": "gpt-5",
                "reasoning": reasoning,
            }))
            .expect("non-string reasoning metadata stays forward-compatible");
            assert_eq!(metadata.reasoning_effort, None);
        }

        let metadata = serde_json::from_value::<RequestMetadata>(serde_json::json!({
            "model": "gpt-5",
            "reasoning": { "effort": "high", "summary": "ignored sibling" },
        }))
        .expect("string reasoning effort");
        assert_eq!(metadata.reasoning_effort.as_deref(), Some("high"));
    }

    #[test]
    fn request_metadata_projects_only_explicit_local_shell_environment() {
        let local = serde_json::from_value::<RequestMetadata>(serde_json::json!({
            "model": "gpt-5",
            "tools": [
                {"type": "shell", "environment": {"type": "local"}},
                {"type": "function"}
            ]
        }))
        .expect("local shell request metadata");
        assert!(local.request_declares_local_shell);

        for environment in [
            serde_json::json!({"type": "container_auto"}),
            serde_json::json!({"type": "container_reference", "container_id": "synthetic"}),
            serde_json::json!({}),
        ] {
            let hosted = serde_json::from_value::<RequestMetadata>(serde_json::json!({
                "model": "gpt-5",
                "tools": [{"type": "shell", "environment": environment}]
            }))
            .expect("hosted shell request metadata");
            assert!(!hosted.request_declares_local_shell);
        }

        for tools in [
            serde_json::Value::Null,
            serde_json::json!({"type": "shell"}),
            serde_json::json!([null, "shell", 1, {"type": "shell", "environment": []}]),
        ] {
            let malformed = serde_json::from_value::<RequestMetadata>(serde_json::json!({
                "model": "gpt-5",
                "tools": tools
            }))
            .expect("unknown tool shapes stay request-compatible");
            assert!(!malformed.request_declares_local_shell);
        }
    }

    #[test]
    fn service_tier_omit_resolves_an_escaped_top_level_key() {
        let body = remove_top_level_service_tier(
            br#"{"model":"gpt-5","\u0073ervice_tier":"priority","keep":1}"#,
        )
        .unwrap_or_else(|_| panic!("valid JSON"))
        .expect("omitted body");
        let value: serde_json::Value = serde_json::from_slice(&body).expect("rewritten JSON");
        assert!(value.get("service_tier").is_none());
        assert_eq!(
            value.get("keep").and_then(serde_json::Value::as_i64),
            Some(1)
        );
    }

    #[test]
    fn service_tier_omit_preserves_nested_keys() {
        let body = remove_top_level_service_tier(
            br#"{"model":"gpt-5","service_tier":"priority","nested":{"service_tier":"keep"}}"#,
        )
        .unwrap_or_else(|_| panic!("valid JSON"))
        .expect("omitted body");
        let value: serde_json::Value = serde_json::from_slice(&body).expect("rewritten JSON");
        assert!(value.get("service_tier").is_none());
        assert_eq!(value["nested"]["service_tier"], "keep");
    }

    #[test]
    fn service_tier_omit_returns_no_alternate_when_absent() {
        assert!(
            remove_top_level_service_tier(br#"{"model":"gpt-5"}"#)
                .unwrap_or_else(|_| panic!("valid JSON"))
                .is_none()
        );
    }

    #[test]
    fn service_tier_omit_removes_a_null_top_level_value() {
        let body = remove_top_level_service_tier(br#"{"model":"gpt-5","service_tier":null}"#)
            .unwrap_or_else(|_| panic!("valid JSON"))
            .expect("omitted body");
        let value: serde_json::Value = serde_json::from_slice(&body).expect("rewritten JSON");
        assert!(value.get("service_tier").is_none());
    }

    #[derive(Default)]
    struct RecordingUpstream {
        calls: AtomicUsize,
        route_ids: Mutex<Vec<RouteId>>,
        state: OnceLock<ProxyIngressState>,
        replacement_route: OnceLock<Arc<RouteSnapshot>>,
    }

    #[derive(Default)]
    struct RecordingHistory(Mutex<Vec<RequestHistoryRecord>>);

    impl HistorySink for RecordingHistory {
        fn try_record(&self, record: RequestHistoryRecord) -> bool {
            self.0.lock().expect("history mutex").push(record);
            true
        }
    }

    #[derive(Default)]
    struct RecordingDiagnostics(Mutex<Vec<RuntimeDiagnosticEvent>>);

    impl RuntimeDiagnosticSink for RecordingDiagnostics {
        fn emit(&self, event: RuntimeDiagnosticEvent) {
            self.0.lock().expect("diagnostic mutex").push(event);
        }
    }

    #[derive(Default)]
    struct RecordingActivity(Mutex<Vec<LogicalRequestActivityTransition>>);

    impl LogicalRequestActivitySink for RecordingActivity {
        fn activity_changed(&self, transition: LogicalRequestActivityTransition) {
            self.0.lock().expect("activity mutex").push(transition);
        }
    }

    struct MultiAttemptUpstream {
        activity: LogicalRequestActivityTracker,
        snapshots: Mutex<Vec<(usize, u64)>>,
    }

    #[derive(Default)]
    struct ToolThenFinalUpstream(AtomicUsize);

    #[async_trait]
    impl UpstreamRequestHandler for ToolThenFinalUpstream {
        async fn handle(&self, request: ValidatedProxyRequest) -> Response {
            let disposition = if self.0.fetch_add(1, Ordering::SeqCst) == 0 {
                RequestActivityDisposition::ClientToolHandoff
            } else {
                RequestActivityDisposition::Final
            };
            request
                .activity_reporter
                .expect("validated request activity reporter")
                .mark_terminal(disposition);
            (StatusCode::OK, "upstream").into_response()
        }
    }

    #[async_trait]
    impl UpstreamRequestHandler for MultiAttemptUpstream {
        async fn handle(&self, _request: ValidatedProxyRequest) -> Response {
            self.snapshots
                .lock()
                .expect("attempt snapshot mutex")
                .push(self.activity.snapshot());
            tokio::task::yield_now().await;
            self.snapshots
                .lock()
                .expect("attempt snapshot mutex")
                .push(self.activity.snapshot());
            (StatusCode::OK, "upstream").into_response()
        }
    }

    #[async_trait]
    impl UpstreamRequestHandler for RecordingUpstream {
        async fn handle(&self, request: ValidatedProxyRequest) -> Response {
            if let (Some(state), Some(route)) = (self.state.get(), self.replacement_route.get()) {
                state.set_active_route(Some(Arc::clone(route)));
            }
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.route_ids
                .lock()
                .expect("route capture mutex")
                .push(request.route.route_id.clone());
            (StatusCode::OK, "upstream").into_response()
        }
    }

    fn route(name: &str) -> Arc<RouteSnapshot> {
        Arc::new(RouteSnapshot {
            route_id: RouteId::new(),
            name: name.to_owned(),
            base_url: BaseUrl::parse("https://api.example.test/v1").expect("valid base URL"),
            api_key: Arc::new(ApiKey::parse("upstream-key").expect("valid API key")),
            service_tier_policy: ServiceTierPolicy::Passthrough,
            fallback_excluded_models: Arc::new(HashSet::new()),
        })
    }

    fn authorized_request(method: Method, uri: &str, body: Body) -> HttpRequest<Body> {
        HttpRequest::builder()
            .method(method)
            .uri(uri)
            .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
            .body(body)
            .expect("valid request")
    }

    async fn error_code(response: Response) -> String {
        let bytes = to_bytes(response.into_body(), 16 * 1024)
            .await
            .expect("bounded response body");
        serde_json::from_slice::<LocalErrorDto>(&bytes)
            .expect("local error DTO")
            .error
            .code
    }

    async fn mcp_sse_json(response: Response) -> serde_json::Value {
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("text/event-stream")
        );
        let bytes = to_bytes(response.into_body(), 64 * 1024)
            .await
            .expect("bounded MCP response body");
        let body = std::str::from_utf8(&bytes).expect("MCP SSE is UTF-8");
        body.lines()
            .filter_map(|line| line.strip_prefix("data:"))
            .map(str::trim)
            .filter(|data| !data.is_empty())
            .find_map(|data| serde_json::from_str(data).ok())
            .expect("MCP SSE JSON data")
    }

    #[tokio::test]
    async fn proxy_auth_rejects_before_polling_body() {
        let upstream = Arc::new(RecordingUpstream::default());
        let history = Arc::new(RecordingHistory::default());
        let diagnostics = Arc::new(RecordingDiagnostics::default());
        let router = build_proxy_router(
            ProxyIngressState::new(TOKEN, upstream.clone())
                .with_runtime_sinks(history.clone(), diagnostics.clone()),
        );
        let polls = Arc::new(AtomicUsize::new(0));
        let body_polls = Arc::clone(&polls);
        let stream = poll_fn(move |_| {
            body_polls.fetch_add(1, Ordering::SeqCst);
            Poll::Ready(Some(Ok::<Bytes, Infallible>(Bytes::from_static(
                b"body-must-not-be-read",
            ))))
        });
        let request = HttpRequest::builder()
            .method(Method::POST)
            .uri("/v1/responses")
            .header(header::AUTHORIZATION, "Bearer wrong-token")
            .body(Body::from_stream(stream))
            .expect("valid request");

        let response = router.oneshot(request).await.expect("router response");

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            response.headers().get(header::WWW_AUTHENTICATE),
            Some(&HeaderValue::from_static("Bearer"))
        );
        assert_eq!(error_code(response).await, "invalid_local_gateway_token");
        assert_eq!(polls.load(Ordering::SeqCst), 0);
        assert_eq!(upstream.calls.load(Ordering::SeqCst), 0);

        let missing_polls = Arc::new(AtomicUsize::new(0));
        let missing_body_polls = Arc::clone(&missing_polls);
        let missing_stream = poll_fn(move |_| {
            missing_body_polls.fetch_add(1, Ordering::SeqCst);
            Poll::Ready(Some(Ok::<Bytes, Infallible>(Bytes::from_static(
                b"missing-token-body-must-not-be-read",
            ))))
        });
        let missing_request = HttpRequest::builder()
            .method(Method::POST)
            .uri("/v1/responses")
            .body(Body::from_stream(missing_stream))
            .expect("valid request");
        let missing_response = build_proxy_router(
            ProxyIngressState::new(TOKEN, upstream.clone())
                .with_runtime_sinks(history.clone(), diagnostics.clone()),
        )
        .oneshot(missing_request)
        .await
        .expect("missing-token response");
        assert_eq!(missing_response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(missing_polls.load(Ordering::SeqCst), 0);
        assert_eq!(upstream.calls.load(Ordering::SeqCst), 0);
        assert!(history.0.lock().expect("history mutex").is_empty());
        assert_eq!(diagnostics.0.lock().expect("diagnostic mutex").len(), 2);
    }

    #[tokio::test]
    async fn responses_activity_starts_after_validation_and_ends_with_body_lifetime() {
        let upstream = Arc::new(RecordingUpstream::default());
        let activity = Arc::new(RecordingActivity::default());
        let tracker = LogicalRequestActivityTracker::new(activity.clone());
        let state = ProxyIngressState::new(TOKEN, upstream).with_activity_tracker(tracker.clone());
        state.set_active_route(Some(route("primary")));
        let router = build_proxy_router(state);
        router
            .clone()
            .oneshot(
                HttpRequest::builder()
                    .method(Method::POST)
                    .uri("/v1/responses")
                    .body(Body::from(r#"{"model":"gpt-5"}"#))
                    .expect("unauthorized request"),
            )
            .await
            .expect("unauthorized response");
        assert_eq!(tracker.snapshot(), (0, 0));
        router
            .clone()
            .oneshot(authorized_request(
                Method::POST,
                "/v1/responses",
                Body::from("not-json"),
            ))
            .await
            .expect("invalid response");
        assert_eq!(tracker.snapshot(), (0, 0));
        assert!(activity.0.lock().expect("activity mutex").is_empty());

        let response = router
            .oneshot(authorized_request(
                Method::POST,
                "/v1/responses",
                Body::from(r#"{"model":"gpt-5"}"#),
            ))
            .await
            .expect("accepted response");
        assert_eq!(tracker.snapshot(), (1, 1));
        assert_eq!(
            to_bytes(response.into_body(), 1024)
                .await
                .expect("response body"),
            Bytes::from_static(b"upstream")
        );
        assert_eq!(tracker.snapshot(), (0, 2));
        assert_eq!(
            activity.0.lock().expect("activity mutex").as_slice(),
            [
                LogicalRequestActivityTransition {
                    phase: LogicalRequestActivityPhase::Live,
                    active: true,
                    count: 1,
                    revision: 1,
                },
                LogicalRequestActivityTransition {
                    phase: LogicalRequestActivityPhase::Idle,
                    active: false,
                    count: 0,
                    revision: 2,
                },
            ]
        );
    }

    #[tokio::test]
    async fn responses_activity_covers_overlapping_bodies_and_downstream_drop() {
        let upstream = Arc::new(RecordingUpstream::default());
        let activity = Arc::new(RecordingActivity::default());
        let tracker = LogicalRequestActivityTracker::new(activity.clone());
        let state = ProxyIngressState::new(TOKEN, upstream).with_activity_tracker(tracker.clone());
        state.set_active_route(Some(route("primary")));
        let router = build_proxy_router(state);
        let request = || {
            authorized_request(
                Method::POST,
                "/v1/responses",
                Body::from(r#"{"model":"gpt-5","stream":true}"#),
            )
        };

        let first = router
            .clone()
            .oneshot(request())
            .await
            .expect("first response");
        let second = router.oneshot(request()).await.expect("second response");
        assert_eq!(tracker.snapshot(), (2, 2));

        drop(first);
        assert_eq!(tracker.snapshot(), (1, 3));
        drop(second);
        assert_eq!(tracker.snapshot(), (0, 4));
        assert_eq!(activity.0.lock().expect("activity mutex").len(), 2);
    }

    #[tokio::test]
    async fn responses_activity_stays_single_across_logical_upstream_attempts() {
        let activity = Arc::new(RecordingActivity::default());
        let tracker = LogicalRequestActivityTracker::new(activity.clone());
        let upstream = Arc::new(MultiAttemptUpstream {
            activity: tracker.clone(),
            snapshots: Mutex::new(Vec::new()),
        });
        let state =
            ProxyIngressState::new(TOKEN, upstream.clone()).with_activity_tracker(tracker.clone());
        state.set_active_route(Some(route("primary")));
        let response = build_proxy_router(state)
            .oneshot(authorized_request(
                Method::POST,
                "/v1/responses",
                Body::from(r#"{"model":"gpt-5"}"#),
            ))
            .await
            .expect("accepted response");

        assert_eq!(
            *upstream.snapshots.lock().expect("attempt snapshot mutex"),
            [(1, 1), (1, 1)]
        );
        assert_eq!(tracker.snapshot(), (1, 1));
        drop(response);
        assert_eq!(tracker.snapshot(), (0, 2));
        assert_eq!(activity.0.lock().expect("activity mutex").len(), 2);
    }

    #[tokio::test]
    async fn responses_activity_continues_across_same_turn_tool_handoff_without_flicker() {
        let activity = Arc::new(RecordingActivity::default());
        let tracker = LogicalRequestActivityTracker::new(activity.clone());
        let state = ProxyIngressState::new(TOKEN, Arc::new(ToolThenFinalUpstream::default()))
            .with_activity_tracker(tracker.clone());
        state.set_active_route(Some(route("primary")));
        let router = build_proxy_router(state);
        let request = || {
            let mut request = authorized_request(
                Method::POST,
                "/v1/responses",
                Body::from(r#"{"model":"gpt-5"}"#),
            );
            request.headers_mut().insert(
                "x-codex-turn-metadata",
                HeaderValue::from_static(r#"{"turn_id":"turn-tool"}"#),
            );
            request
        };

        let first = router
            .clone()
            .oneshot(request())
            .await
            .expect("tool response");
        let _ = to_bytes(first.into_body(), 1024)
            .await
            .expect("tool response body");
        assert_eq!(tracker.snapshot().0, 1);
        assert_eq!(tracker.phase(), LogicalRequestActivityPhase::Waiting);
        assert_eq!(activity.0.lock().expect("activity mutex").len(), 2);

        let final_response = router.oneshot(request()).await.expect("final response");
        let _ = to_bytes(final_response.into_body(), 1024)
            .await
            .expect("final response body");
        assert_eq!(tracker.snapshot().0, 0);
        assert_eq!(tracker.phase(), LogicalRequestActivityPhase::Idle);
        assert_eq!(
            activity.0.lock().expect("activity mutex").as_slice(),
            [
                LogicalRequestActivityTransition {
                    phase: LogicalRequestActivityPhase::Live,
                    active: true,
                    count: 1,
                    revision: 1,
                },
                LogicalRequestActivityTransition {
                    phase: LogicalRequestActivityPhase::Waiting,
                    active: true,
                    count: 1,
                    revision: 2,
                },
                LogicalRequestActivityTransition {
                    phase: LogicalRequestActivityPhase::Live,
                    active: true,
                    count: 1,
                    revision: 3,
                },
                LogicalRequestActivityTransition {
                    phase: LogicalRequestActivityPhase::Idle,
                    active: false,
                    count: 0,
                    revision: 4,
                },
            ]
        );
    }

    #[tokio::test]
    async fn proxy_routes_expose_only_the_exact_authenticated_surface() {
        let upstream = Arc::new(RecordingUpstream::default());
        let router = build_proxy_router(ProxyIngressState::new(TOKEN, upstream));

        let health = router
            .clone()
            .oneshot(
                HttpRequest::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .expect("health request"),
            )
            .await
            .expect("health response");
        assert_eq!(health.status(), StatusCode::OK);

        let models_without_token = router
            .clone()
            .oneshot(
                HttpRequest::builder()
                    .uri("/v1/models")
                    .body(Body::empty())
                    .expect("models request"),
            )
            .await
            .expect("models response");
        assert_eq!(models_without_token.status(), StatusCode::UNAUTHORIZED);

        let models = router
            .clone()
            .oneshot(authorized_request(Method::GET, "/v1/models", Body::empty()))
            .await
            .expect("models response");
        assert_eq!(models.status(), StatusCode::OK);
        assert_eq!(
            to_bytes(models.into_body(), 1024)
                .await
                .expect("models body"),
            Bytes::from_static(br#"{"models":[]}"#)
        );

        let unknown_without_token = router
            .clone()
            .oneshot(
                HttpRequest::builder()
                    .uri("/v1/chat/completions")
                    .body(Body::empty())
                    .expect("unknown request"),
            )
            .await
            .expect("unknown response");
        assert_eq!(unknown_without_token.status(), StatusCode::UNAUTHORIZED);

        let unknown = router
            .clone()
            .oneshot(authorized_request(
                Method::POST,
                "/v1/chat/completions",
                Body::empty(),
            ))
            .await
            .expect("unknown response");
        assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
        assert_eq!(error_code(unknown).await, "not_found");

        let wrong_method = router
            .clone()
            .oneshot(authorized_request(
                Method::GET,
                "/v1/responses",
                Body::empty(),
            ))
            .await
            .expect("method response");
        assert_eq!(wrong_method.status(), StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(error_code(wrong_method).await, "method_not_allowed");

        let health_wrong_method = router
            .oneshot(
                HttpRequest::builder()
                    .method(Method::POST)
                    .uri("/health")
                    .body(Body::empty())
                    .expect("health method request"),
            )
            .await
            .expect("health method response");
        assert_eq!(health_wrong_method.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn images_http_and_mcp_surfaces_share_gateway_auth_and_stay_fail_closed() {
        let upstream = Arc::new(RecordingUpstream::default());
        let temporary = TempDir::new().expect("temporary app data");
        let router = build_proxy_router(
            ProxyIngressState::new(TOKEN, upstream.clone())
                .with_mcp_image_asset_root(temporary.path().join("mcp-images")),
        );

        let unauthorized_images = router
            .clone()
            .oneshot(
                HttpRequest::builder()
                    .method(Method::POST)
                    .uri("/v1/images/generations")
                    .header(header::AUTHORIZATION, "Bearer wrong")
                    .body(Body::from(r#"{"prompt":"private"}"#))
                    .expect("image request"),
            )
            .await
            .expect("unauthorized image response");
        assert_eq!(unauthorized_images.status(), StatusCode::UNAUTHORIZED);

        let disabled_images = router
            .clone()
            .oneshot(authorized_request(
                Method::POST,
                "/v1/images/generations",
                Body::from(r#"{"prompt":"private"}"#),
            ))
            .await
            .expect("disabled image response");
        assert_eq!(disabled_images.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            error_code(disabled_images).await,
            "images_generation_disabled"
        );
        assert_eq!(upstream.calls.load(Ordering::SeqCst), 0);

        let unauthorized_mcp = router
            .clone()
            .oneshot(
                HttpRequest::builder()
                    .method(Method::POST)
                    .uri("/mcp")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::ACCEPT, "application/json, text/event-stream")
                    .body(Body::from(
                        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}"#,
                    ))
                    .expect("MCP request"),
            )
            .await
            .expect("unauthorized MCP response");
        assert_eq!(unauthorized_mcp.status(), StatusCode::UNAUTHORIZED);

        let initialize = router
            .clone()
            .oneshot(
                HttpRequest::builder()
                    .method(Method::POST)
                    .uri("/mcp")
                    .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                    .header(header::HOST, "127.0.0.1")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::ACCEPT, "application/json, text/event-stream")
                    .body(Body::from(
                        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}"#,
                    ))
                    .expect("initialize request"),
            )
            .await
            .expect("initialize response");
        assert_eq!(initialize.status(), StatusCode::OK);
        let session_id = initialize
            .headers()
            .get("mcp-session-id")
            .and_then(|value| value.to_str().ok())
            .expect("MCP session ID")
            .to_owned();
        let initialize_body = mcp_sse_json(initialize).await;
        assert_eq!(
            initialize_body["result"]["capabilities"]["tools"],
            serde_json::json!({})
        );

        let list = router
            .clone()
            .oneshot(
                HttpRequest::builder()
                    .method(Method::POST)
                    .uri("/mcp")
                    .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                    .header(header::HOST, "127.0.0.1")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::ACCEPT, "application/json, text/event-stream")
                    .header("mcp-session-id", &session_id)
                    .header("mcp-protocol-version", "2025-06-18")
                    .body(Body::from(
                        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#,
                    ))
                    .expect("list request"),
            )
            .await
            .expect("list response");
        assert_eq!(list.status(), StatusCode::OK);
        let list_json = mcp_sse_json(list).await;
        assert_eq!(
            list_json["result"]["tools"].as_array().map(Vec::len),
            Some(1)
        );
        assert_eq!(list_json["result"]["tools"][0]["name"], "generate_image");
        assert!(list_json["result"].get("resultType").is_none());
        assert!(list_json["result"].get("ttlMs").is_none());
        assert!(list_json["result"].get("cacheScope").is_none());

        let call = router
            .oneshot(
                HttpRequest::builder()
                    .method(Method::POST)
                    .uri("/mcp")
                    .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                    .header(header::HOST, "127.0.0.1")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::ACCEPT, "application/json, text/event-stream")
                    .header("mcp-session-id", session_id)
                    .header("mcp-protocol-version", "2025-06-18")
                    .body(Body::from(
                        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"generate_image","arguments":{"prompt":"private"}}}"#,
                    ))
                    .expect("call request"),
            )
            .await
            .expect("call response");
        assert_eq!(call.status(), StatusCode::OK);
        let call_json = mcp_sse_json(call).await;
        assert_eq!(
            call_json["error"]["data"]["code"],
            "images_generation_disabled"
        );
        assert_eq!(call_json["error"]["data"]["stage"], "request_construction");
        assert_eq!(
            call_json["error"]["data"]["upstreamStatus"],
            serde_json::Value::Null
        );
        assert_eq!(call_json["error"]["data"]["category"], "unknown_upstream");
        assert_eq!(call_json["error"]["data"]["retryable"], false);
        Uuid::parse_str(
            call_json["error"]["data"]["requestId"]
                .as_str()
                .expect("local request ID"),
        )
        .expect("UUID request ID");
        assert_eq!(upstream.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn images_mcp_wire_exposes_content_policy_stage_and_safe_fields_once() {
        const PROVIDER_CODE: &str = "content_policy_violation";
        const PROVIDER_MESSAGE: &str = "PROVIDER_MESSAGE_SENTINEL_32aa";
        const PROVIDER_REQUEST_ID: &str = "PROVIDER_REQUEST_ID_SENTINEL_f10d";
        const PROVIDER_HEADER: &str = "PROVIDER_HEADER_SENTINEL_908c";
        let calls = Arc::new(AtomicUsize::new(0));
        let handler_calls = Arc::clone(&calls);
        let image_upstream = ProxyServerHandle::start(
            0,
            Router::new().route(
                "/openai/v1/images/generations",
                post(move || {
                    let calls = Arc::clone(&handler_calls);
                    async move {
                        calls.fetch_add(1, Ordering::SeqCst);
                        Response::builder()
                            .status(StatusCode::BAD_REQUEST)
                            .header("x-provider-request-id", PROVIDER_HEADER)
                            .header(header::CONTENT_TYPE, "application/json")
                            .body(Body::from(
                                serde_json::json!({
                                    "error": {
                                        "code": PROVIDER_CODE,
                                        "message": PROVIDER_MESSAGE,
                                        "request_id": PROVIDER_REQUEST_ID
                                    }
                                })
                                .to_string(),
                            ))
                            .expect("content policy response")
                    }
                }),
            ),
        )
        .await
        .expect("loopback image upstream");
        let image_route = Arc::new(RouteSnapshot {
            route_id: RouteId::new(),
            name: "Image route".to_owned(),
            base_url: BaseUrl::parse(&format!("http://{}/openai/v1", image_upstream.address()))
                .expect("image base URL"),
            api_key: Arc::new(ApiKey::parse("image-route-key").expect("image API key")),
            service_tier_policy: ServiceTierPolicy::Passthrough,
            fallback_excluded_models: Arc::new(HashSet::new()),
        });
        let routing = RoutingSnapshotStore::new(RoutingSnapshot {
            active: None,
            participants: Vec::new(),
            enabled: false,
            selection_generation: 0,
            health_generation: 0,
            config_revision: 0,
            images_generation_enabled: true,
            images_route: Some(image_route),
            images_generation_timeout: Duration::from_mins(10),
        });
        let temporary = TempDir::new().expect("temporary app data");
        let router = build_proxy_router(
            ProxyIngressState::new(TOKEN, Arc::new(RecordingUpstream::default()))
                .with_routing_store(routing)
                .with_mcp_image_asset_root(temporary.path().join("mcp-images")),
        );

        let initialize = router
            .clone()
            .oneshot(
                HttpRequest::builder()
                    .method(Method::POST)
                    .uri("/mcp")
                    .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                    .header(header::HOST, "127.0.0.1")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::ACCEPT, "application/json, text/event-stream")
                    .body(Body::from(
                        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}"#,
                    ))
                    .expect("initialize request"),
            )
            .await
            .expect("initialize response");
        let session_id = initialize
            .headers()
            .get("mcp-session-id")
            .and_then(|value| value.to_str().ok())
            .expect("MCP session ID")
            .to_owned();
        let _ = mcp_sse_json(initialize).await;
        let call = router
            .oneshot(
                HttpRequest::builder()
                    .method(Method::POST)
                    .uri("/mcp")
                    .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                    .header(header::HOST, "127.0.0.1")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::ACCEPT, "application/json, text/event-stream")
                    .header("mcp-session-id", session_id)
                    .header("mcp-protocol-version", "2025-06-18")
                    .body(Body::from(
                        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"generate_image","arguments":{"prompt":"private prompt"}}}"#,
                    ))
                    .expect("call request"),
            )
            .await
            .expect("call response");
        assert_eq!(call.status(), StatusCode::OK);
        let call_json = mcp_sse_json(call).await;
        assert_eq!(call_json["error"]["code"], -32603);
        assert_eq!(
            call_json["error"]["message"],
            "The image provider rejected the request under its content policy."
        );
        let data = &call_json["error"]["data"];
        assert_eq!(data.as_object().map(serde_json::Map::len), Some(6));
        assert_eq!(data["code"], "images_upstream_http_status");
        assert_eq!(data["stage"], "upstream_http_status");
        assert_eq!(data["upstreamStatus"], 400);
        assert_eq!(data["category"], "content_policy");
        assert_eq!(data["retryable"], false);
        Uuid::parse_str(data["requestId"].as_str().expect("local request ID"))
            .expect("UUID request ID");
        let serialized = serde_json::to_string(&call_json).expect("serialized MCP frame");
        for forbidden in [
            PROVIDER_CODE,
            PROVIDER_MESSAGE,
            PROVIDER_REQUEST_ID,
            PROVIDER_HEADER,
            "image-route-key",
            "private prompt",
        ] {
            assert!(!serialized.contains(forbidden), "leaked {forbidden}");
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        image_upstream.shutdown().await;
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn images_http_and_mcp_streamable_call_returns_only_local_asset_text_json() {
        let png = valid_png_fixture();
        let png_base64 = STANDARD.encode(&png);
        let upstream_body = Bytes::from(
            serde_json::to_vec(&serde_json::json!({
                "data": [{"b64_json": png_base64.clone()}]
            }))
            .expect("PNG response"),
        );
        let image_upstream = ProxyServerHandle::start(
            0,
            Router::new().route(
                "/openai/v1/images/generations",
                post(move || {
                    let response = upstream_body.clone();
                    async move {
                        (
                            StatusCode::OK,
                            [(header::CONTENT_TYPE, "application/json")],
                            response,
                        )
                    }
                }),
            ),
        )
        .await
        .expect("loopback image upstream");
        let image_route = Arc::new(RouteSnapshot {
            route_id: RouteId::new(),
            name: "Image route".to_owned(),
            base_url: BaseUrl::parse(&format!("http://{}/openai/v1", image_upstream.address()))
                .expect("image base URL"),
            api_key: Arc::new(ApiKey::parse("image-route-key").expect("image API key")),
            service_tier_policy: ServiceTierPolicy::Passthrough,
            fallback_excluded_models: Arc::new(HashSet::new()),
        });
        let routing = RoutingSnapshotStore::new(RoutingSnapshot {
            active: None,
            participants: Vec::new(),
            enabled: false,
            selection_generation: 0,
            health_generation: 0,
            config_revision: 0,
            images_generation_enabled: true,
            images_route: Some(image_route),
            images_generation_timeout: Duration::from_mins(10),
        });
        let temporary = TempDir::new().expect("temporary app data");
        let asset_root = temporary.path().join("mcp-images");
        let router = build_proxy_router(
            ProxyIngressState::new(TOKEN, Arc::new(RecordingUpstream::default()))
                .with_routing_store(routing)
                .with_mcp_image_asset_root(asset_root.clone()),
        );

        let initialize = router
            .clone()
            .oneshot(
                HttpRequest::builder()
                    .method(Method::POST)
                    .uri("/mcp")
                    .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                    .header(header::HOST, "127.0.0.1")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::ACCEPT, "application/json, text/event-stream")
                    .body(Body::from(
                        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}"#,
                    ))
                    .expect("initialize request"),
            )
            .await
            .expect("initialize response");
        assert_eq!(initialize.status(), StatusCode::OK);
        let session_id = initialize
            .headers()
            .get("mcp-session-id")
            .and_then(|value| value.to_str().ok())
            .expect("MCP session ID")
            .to_owned();
        let _ = mcp_sse_json(initialize).await;

        let call = router
            .oneshot(
                HttpRequest::builder()
                    .method(Method::POST)
                    .uri("/mcp")
                    .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                    .header(header::HOST, "127.0.0.1")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::ACCEPT, "application/json, text/event-stream")
                    .header("mcp-session-id", session_id)
                    .header("mcp-protocol-version", "2025-06-18")
                    .body(Body::from(
                        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"generate_image","arguments":{"prompt":"private prompt"}}}"#,
                    ))
                    .expect("call request"),
            )
            .await
            .expect("call response");
        assert_eq!(call.status(), StatusCode::OK);
        let call_json = mcp_sse_json(call).await;
        let result = &call_json["result"];
        assert_eq!(result["content"].as_array().map(Vec::len), Some(1));
        assert_eq!(result["content"][0]["type"], "text");
        assert!(result.get("structuredContent").is_none());
        let text = result["content"][0]["text"]
            .as_str()
            .expect("asset JSON text");
        let asset: serde_json::Value = serde_json::from_str(text).expect("asset JSON");
        assert_eq!(asset.as_object().map(serde_json::Map::len), Some(8));
        assert_eq!(asset["status"], "success");
        assert_eq!(asset["mimeType"], "image/png");
        assert_eq!(asset["width"], 1);
        assert_eq!(asset["height"], 1);
        assert_eq!(asset["bytes"], png.len());
        assert_eq!(asset["sha256"], hex::encode(Sha256::digest(&png)));
        let path = std::path::PathBuf::from(asset["path"].as_str().expect("asset path"));
        assert!(path.is_absolute());
        assert_eq!(
            path.parent(),
            Some(asset_root.canonicalize().expect("root").as_path())
        );
        assert_eq!(std::fs::read(path).expect("published PNG"), png);
        let serialized = serde_json::to_string(result).expect("serialized result");
        for forbidden in [
            "\"type\":\"image\"",
            png_base64.as_str(),
            "data:",
            "![",
            "https://",
        ] {
            assert!(!serialized.contains(forbidden), "leaked {forbidden}");
        }
        image_upstream.shutdown().await;
    }

    #[tokio::test]
    async fn proxy_ingress_enforces_wire_and_decoded_boundaries() {
        let body = br#"{"model":"gpt-test","stream":false}"#.to_vec();
        let upstream = Arc::new(RecordingUpstream::default());
        let state =
            ProxyIngressState::new(TOKEN, upstream.clone()).with_limits(body.len(), body.len());
        state.set_active_route(Some(route("primary")));
        let router = build_proxy_router(state);

        let exact = router
            .clone()
            .oneshot(authorized_request(
                Method::POST,
                "/v1/responses",
                Body::from(body.clone()),
            ))
            .await
            .expect("exact-limit response");
        assert_eq!(exact.status(), StatusCode::OK);

        let over_limit_state =
            ProxyIngressState::new(TOKEN, upstream.clone()).with_limits(body.len() - 1, body.len());
        over_limit_state.set_active_route(Some(route("primary")));
        let over_limit = build_proxy_router(over_limit_state)
            .oneshot(authorized_request(
                Method::POST,
                "/v1/responses",
                Body::from(body.clone()),
            ))
            .await
            .expect("over-limit response");
        assert_eq!(over_limit.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(error_code(over_limit).await, "request_body_too_large");

        let polls = Arc::new(AtomicUsize::new(0));
        let body_polls = Arc::clone(&polls);
        let stream = poll_fn(move |_| {
            body_polls.fetch_add(1, Ordering::SeqCst);
            Poll::Ready(Some(Ok::<Bytes, Infallible>(Bytes::from_static(
                b"ignored",
            ))))
        });
        let content_length_state =
            ProxyIngressState::new(TOKEN, upstream.clone()).with_limits(8, 64);
        content_length_state.set_active_route(Some(route("primary")));
        let content_length_request = HttpRequest::builder()
            .method(Method::POST)
            .uri("/v1/responses")
            .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
            .header(header::CONTENT_LENGTH, "9")
            .body(Body::from_stream(stream))
            .expect("content-length request");
        let content_length = build_proxy_router(content_length_state)
            .oneshot(content_length_request)
            .await
            .expect("content-length response");
        assert_eq!(content_length.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(polls.load(Ordering::SeqCst), 0);
        assert_eq!(upstream.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn proxy_ingress_bounds_zstd_decoding() {
        let body = br#"{"model":"gpt-test","stream":false}"#.to_vec();
        let upstream = Arc::new(RecordingUpstream::default());
        let compressed = zstd::stream::encode_all(body.as_slice(), 0).expect("zstd encode");
        let zstd_state = ProxyIngressState::new(TOKEN, upstream.clone())
            .with_limits(compressed.len(), body.len() - 1);
        zstd_state.set_active_route(Some(route("primary")));
        let zstd_request = HttpRequest::builder()
            .method(Method::POST)
            .uri("/v1/responses")
            .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
            .header(header::CONTENT_ENCODING, "zstd")
            .body(Body::from(compressed))
            .expect("zstd request");
        let zstd_response = build_proxy_router(zstd_state)
            .oneshot(zstd_request)
            .await
            .expect("zstd response");
        assert_eq!(zstd_response.status(), StatusCode::PAYLOAD_TOO_LARGE);

        let corrupt_zstd_state = ProxyIngressState::new(TOKEN, upstream.clone());
        corrupt_zstd_state.set_active_route(Some(route("primary")));
        let corrupt_zstd_request = HttpRequest::builder()
            .method(Method::POST)
            .uri("/v1/responses")
            .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
            .header(header::CONTENT_ENCODING, "zstd")
            .body(Body::from("not-zstd"))
            .expect("corrupt zstd request");
        let corrupt_zstd = build_proxy_router(corrupt_zstd_state)
            .oneshot(corrupt_zstd_request)
            .await
            .expect("corrupt zstd response");
        assert_eq!(corrupt_zstd.status(), StatusCode::BAD_REQUEST);
        assert_eq!(error_code(corrupt_zstd).await, "invalid_request");
        assert_eq!(upstream.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn proxy_ingress_rejects_unsupported_encoding_and_invalid_json() {
        let body = br#"{"model":"gpt-test","stream":false}"#.to_vec();
        let upstream = Arc::new(RecordingUpstream::default());
        let unsupported_state = ProxyIngressState::new(TOKEN, upstream.clone());
        unsupported_state.set_active_route(Some(route("primary")));
        let unsupported_request = HttpRequest::builder()
            .method(Method::POST)
            .uri("/v1/responses")
            .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
            .header(header::CONTENT_ENCODING, "gzip")
            .body(Body::from(body.clone()))
            .expect("unsupported request");
        let unsupported = build_proxy_router(unsupported_state)
            .oneshot(unsupported_request)
            .await
            .expect("unsupported response");
        assert_eq!(unsupported.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
        assert_eq!(
            error_code(unsupported).await,
            "unsupported_content_encoding"
        );

        let malformed_state = ProxyIngressState::new(TOKEN, upstream.clone());
        malformed_state.set_active_route(Some(route("primary")));
        let malformed = build_proxy_router(malformed_state)
            .oneshot(authorized_request(
                Method::POST,
                "/v1/responses",
                Body::from("{"),
            ))
            .await
            .expect("malformed response");
        assert_eq!(malformed.status(), StatusCode::BAD_REQUEST);
        assert_eq!(error_code(malformed).await, "invalid_request");

        let invalid_state = ProxyIngressState::new(TOKEN, upstream.clone());
        invalid_state.set_active_route(Some(route("primary")));
        let invalid = build_proxy_router(invalid_state)
            .oneshot(authorized_request(
                Method::POST,
                "/v1/responses",
                Body::from(Bytes::from_static(br#"{"model":""}"#)),
            ))
            .await
            .expect("invalid response");
        assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
        assert_eq!(error_code(invalid).await, "invalid_request");
        assert_eq!(upstream.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn proxy_routes_fail_closed_and_capture_an_immutable_route_snapshot() {
        let upstream = Arc::new(RecordingUpstream::default());
        let state = ProxyIngressState::new(TOKEN, upstream.clone());
        let router = build_proxy_router(state.clone());
        let request_body = Body::from(Bytes::from_static(br#"{"model":"gpt-test"}"#));

        let no_route = router
            .clone()
            .oneshot(authorized_request(
                Method::POST,
                "/v1/responses",
                request_body,
            ))
            .await
            .expect("no-route response");
        assert_eq!(no_route.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(error_code(no_route).await, "no_upstream_route");
        assert_eq!(upstream.calls.load(Ordering::SeqCst), 0);

        let original = route("original");
        let replacement = route("replacement");
        let original_id = original.route_id.clone();
        state.set_active_route(Some(original));
        assert!(upstream.state.set(state).is_ok());
        assert!(upstream.replacement_route.set(replacement).is_ok());

        let routed_response = router
            .oneshot(authorized_request(
                Method::POST,
                "/v1/responses",
                Body::from(Bytes::from_static(br#"{"model":"gpt-test"}"#)),
            ))
            .await
            .expect("routed response");
        assert_eq!(routed_response.status(), StatusCode::OK);
        assert_eq!(upstream.calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            upstream
                .route_ids
                .lock()
                .expect("route capture mutex")
                .as_slice(),
            &[original_id]
        );
    }

    #[tokio::test]
    async fn request_history_records_authenticated_local_failures_without_attempts() {
        let upstream = Arc::new(RecordingUpstream::default());
        let history = Arc::new(RecordingHistory::default());
        let diagnostics = Arc::new(RecordingDiagnostics::default());
        let state = ProxyIngressState::new(TOKEN, upstream.clone())
            .with_runtime_sinks(history.clone(), diagnostics.clone());
        let request = HttpRequest::builder()
            .method(Method::POST)
            .uri("/v1/responses")
            .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
            .header("x-codex-turn-metadata", r#"{"turn_id":"turn-history"}"#)
            .body(Body::from(
                br#"{"model":"gpt-history","stream":true,"reasoning":{"effort":"  high  ","summary":"must-not-project"}}"#.as_slice(),
            ))
            .expect("history request");

        let response = build_proxy_router(state)
            .oneshot(request)
            .await
            .expect("no-route response");

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(upstream.calls.load(Ordering::SeqCst), 0);
        let records = history.0.lock().expect("history mutex");
        assert_eq!(records.len(), 1);
        let record = &records[0];
        assert_eq!(record.turn_id.as_deref(), Some("turn-history"));
        assert_eq!(record.requested_model.as_deref(), Some("gpt-history"));
        assert_eq!(record.reasoning_effort.as_deref(), Some("high"));
        assert!(record.streaming);
        assert_eq!(record.completion_state, CompletionState::NoUpstream);
        assert_eq!(record.error_category.as_deref(), Some("no_upstream_route"));
        assert!(record.attempts.is_empty());
        drop(records);
        assert_eq!(
            diagnostics.0.lock().expect("diagnostic mutex")[0].code,
            RuntimeDiagnosticCode::NoUpstreamRoute
        );
    }

    struct TestPortStore {
        calls: AtomicUsize,
        fail: bool,
        persisted: Mutex<Vec<u16>>,
    }

    #[async_trait]
    impl ProxyPortStore for TestPortStore {
        async fn persist_port(&self, port: u16) -> Result<(), ProxyPortError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.fail {
                return Err(ProxyPortError::PersistenceFailed);
            }
            self.persisted.lock().expect("port store mutex").push(port);
            Ok(())
        }
    }

    async fn unused_listener_port() -> u16 {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("ephemeral listener");
        listener.local_addr().expect("listener address").port()
    }

    #[tokio::test]
    async fn proxy_port_transition_preserves_old_runtime_on_failures() {
        let current_listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("current listener");
        let current_address = current_listener.local_addr().expect("current address");
        let mut current = ProxyServerHandle::from_listener(current_listener, Router::new());
        let occupied = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("occupied listener");
        let occupied_port = occupied.local_addr().expect("occupied address").port();
        let store = TestPortStore {
            calls: AtomicUsize::new(0),
            fail: false,
            persisted: Mutex::new(Vec::new()),
        };
        let replacement_callbacks = AtomicUsize::new(0);

        let conflict = transition_proxy_port_with_listener_replaced(
            &mut current,
            occupied_port,
            Router::new(),
            &store,
            || {
                replacement_callbacks.fetch_add(1, Ordering::SeqCst);
            },
        )
        .await;
        assert!(matches!(conflict, Err(ProxyPortError::PortUnavailable)));
        assert_eq!(current.address(), current_address);
        assert_eq!(store.calls.load(Ordering::SeqCst), 0);
        assert_eq!(replacement_callbacks.load(Ordering::SeqCst), 0);

        let candidate_port = unused_listener_port().await;
        let failing_store = TestPortStore {
            calls: AtomicUsize::new(0),
            fail: true,
            persisted: Mutex::new(Vec::new()),
        };
        let persistence_failure = transition_proxy_port_with_listener_replaced(
            &mut current,
            candidate_port,
            Router::new(),
            &failing_store,
            || {
                replacement_callbacks.fetch_add(1, Ordering::SeqCst);
            },
        )
        .await;
        assert!(matches!(
            persistence_failure,
            Err(ProxyPortError::PersistenceFailed)
        ));
        assert_eq!(current.address(), current_address);
        assert_eq!(failing_store.calls.load(Ordering::SeqCst), 1);
        assert_eq!(replacement_callbacks.load(Ordering::SeqCst), 0);
        let rebound = TcpListener::bind((Ipv4Addr::LOCALHOST, candidate_port))
            .await
            .expect("failed candidate listener was closed");
        drop(rebound);

        let replacement_port = unused_listener_port().await;
        let successful_store = TestPortStore {
            calls: AtomicUsize::new(0),
            fail: false,
            persisted: Mutex::new(Vec::new()),
        };
        transition_proxy_port_with_listener_replaced(
            &mut current,
            replacement_port,
            Router::new(),
            &successful_store,
            || {
                replacement_callbacks.fetch_add(1, Ordering::SeqCst);
            },
        )
        .await
        .expect("successful port transition");
        assert_eq!(current.address().port(), replacement_port);
        assert_eq!(successful_store.calls.load(Ordering::SeqCst), 1);
        assert_eq!(replacement_callbacks.load(Ordering::SeqCst), 1);
        assert_eq!(
            successful_store
                .persisted
                .lock()
                .expect("port store mutex")
                .as_slice(),
            &[replacement_port]
        );
        let old_port_rebound = TcpListener::bind(current_address)
            .await
            .expect("old listener stopped after handoff");
        drop(old_port_rebound);

        current.shutdown().await;
    }

    #[tokio::test]
    async fn proxy_port_replacement_callback_runs_before_old_listener_drains() {
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let router = Router::new().route(
            "/hold",
            get({
                let entered = Arc::clone(&entered);
                let release = Arc::clone(&release);
                move || {
                    let entered = Arc::clone(&entered);
                    let release = Arc::clone(&release);
                    async move {
                        entered.notify_one();
                        release.notified().await;
                        "done"
                    }
                }
            }),
        );
        let current = ProxyServerHandle::start(0, router)
            .await
            .expect("current listener");
        let client = reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("local client");
        let request = tokio::spawn({
            let url = format!("http://{}/hold", current.address());
            async move { client.get(url).send().await }
        });
        tokio::time::timeout(Duration::from_secs(1), entered.notified())
            .await
            .expect("old listener request entered");

        let replacement_port = unused_listener_port().await;
        let store = TestPortStore {
            calls: AtomicUsize::new(0),
            fail: false,
            persisted: Mutex::new(Vec::new()),
        };
        let (replaced, replaced_rx) = oneshot::channel();
        let transition = tokio::spawn(async move {
            let mut current = current;
            let result = transition_proxy_port_with_listener_replaced(
                &mut current,
                replacement_port,
                Router::new(),
                &store,
                || {
                    let _ = replaced.send(());
                },
            )
            .await;
            (current, result)
        });

        tokio::time::timeout(Duration::from_secs(1), replaced_rx)
            .await
            .expect("replacement callback ran")
            .expect("replacement callback sender");
        assert!(
            !transition.is_finished(),
            "old listener must still be draining when the callback runs"
        );

        release.notify_waiters();
        request
            .await
            .expect("request task")
            .expect("old listener response");
        let (replacement, result) = transition.await.expect("transition task");
        result.expect("port transition");
        replacement.shutdown().await;
    }

    type CapturedProbeRequest = (Method, Uri, HeaderMap, usize);

    #[derive(Clone)]
    struct ProbeCapture {
        calls: Arc<AtomicUsize>,
        requests: Arc<Mutex<Vec<CapturedProbeRequest>>>,
        delay: Duration,
        status: StatusCode,
    }

    async fn probe_handler(
        State(capture): State<ProbeCapture>,
        method: Method,
        uri: Uri,
        headers: HeaderMap,
        body: Bytes,
    ) -> StatusCode {
        capture.calls.fetch_add(1, Ordering::SeqCst);
        capture.requests.lock().expect("probe request mutex").push((
            method,
            uri,
            headers,
            body.len(),
        ));
        tokio::time::sleep(capture.delay).await;
        capture.status
    }

    async fn start_probe_server(capture: ProbeCapture) -> ProxyServerHandle {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("probe listener");
        ProxyServerHandle::from_listener(
            listener,
            Router::new().fallback(probe_handler).with_state(capture),
        )
    }

    #[tokio::test]
    async fn route_probe_uses_exact_no_key_get_and_classifies_status_and_timing() {
        let capture = ProbeCapture {
            calls: Arc::new(AtomicUsize::new(0)),
            requests: Arc::new(Mutex::new(Vec::new())),
            delay: Duration::ZERO,
            status: StatusCode::IM_A_TEAPOT,
        };
        let server = start_probe_server(capture.clone()).await;
        let probe = ReachabilityProbe::new()
            .expect("probe client")
            .with_timing(Duration::from_secs(1), Duration::from_millis(100));

        let result = probe
            .check(&format!("http://{}/v1/responses", server.address()))
            .await
            .expect("valid probe URL");

        assert_eq!(result.status, ReachabilityStatus::Reachable);
        assert!(result.ttfb_ms.is_some());
        assert_eq!(result.error_category, None);
        assert_eq!(capture.calls.load(Ordering::SeqCst), 1);
        {
            let requests = capture.requests.lock().expect("probe request mutex");
            let (method, uri, headers, body_len) = &requests[0];
            assert_eq!(method, Method::GET);
            assert_eq!(uri.path(), "/v1/responses");
            assert_eq!(*body_len, 0);
            assert_eq!(
                headers.get(header::ACCEPT),
                Some(&HeaderValue::from_static("*/*"))
            );
            assert_eq!(
                headers.get(header::ACCEPT_ENCODING),
                Some(&HeaderValue::from_static("identity"))
            );
            assert!(!headers.contains_key(header::AUTHORIZATION));
            assert!(!headers.contains_key("x-api-key"));
            assert!(!headers.contains_key("x-goog-api-key"));
        }
        server.shutdown().await;

        let slow_capture = ProbeCapture {
            calls: Arc::new(AtomicUsize::new(0)),
            requests: Arc::new(Mutex::new(Vec::new())),
            delay: Duration::from_millis(25),
            status: StatusCode::NO_CONTENT,
        };
        let slow_server = start_probe_server(slow_capture).await;
        let slow_probe = ReachabilityProbe::new()
            .expect("probe client")
            .with_timing(Duration::from_secs(1), Duration::from_millis(5));
        let slow = slow_probe
            .check(&format!("http://{}", slow_server.address()))
            .await
            .expect("valid probe URL");
        assert_eq!(slow.status, ReachabilityStatus::Slow);
        slow_server.shutdown().await;
    }

    #[tokio::test]
    async fn route_probe_classifies_final_path_http_statuses() {
        for (status, expected_status, expected_category) in [
            (
                StatusCode::NOT_FOUND,
                ReachabilityStatus::PathNotFound,
                Some("path_not_found"),
            ),
            (
                StatusCode::UNAUTHORIZED,
                ReachabilityStatus::Reachable,
                None,
            ),
            (StatusCode::FORBIDDEN, ReachabilityStatus::Reachable, None),
            (
                StatusCode::METHOD_NOT_ALLOWED,
                ReachabilityStatus::Reachable,
                None,
            ),
        ] {
            let capture = ProbeCapture {
                calls: Arc::new(AtomicUsize::new(0)),
                requests: Arc::new(Mutex::new(Vec::new())),
                delay: Duration::ZERO,
                status,
            };
            let server = start_probe_server(capture).await;
            let probe = ReachabilityProbe::new()
                .expect("probe client")
                .with_timing(Duration::from_secs(1), Duration::from_millis(100));

            let result = probe
                .check(&format!("http://{}/custom/responses", server.address()))
                .await
                .expect("valid probe URL");

            assert_eq!(result.status, expected_status, "HTTP status: {status}");
            assert!(result.ttfb_ms.is_some());
            assert_eq!(result.error_category.as_deref(), expected_category);
            server.shutdown().await;
        }
    }

    #[tokio::test]
    async fn route_probe_rejects_invalid_endpoints_before_network_work() {
        let probe = ReachabilityProbe::new().expect("probe client");
        for (input, expected_code) in [
            (
                "https://example.test/v1/responses/responses",
                "base_url_duplicate_responses",
            ),
            (
                "https://example.test/v1/chat/completions",
                "base_url_unsupported_endpoint",
            ),
        ] {
            let error = probe.check(input).await.expect_err("invalid probe URL");
            assert_eq!(error.code, expected_code);
            assert_eq!(error.field, "baseUrl");
        }
    }

    #[tokio::test]
    async fn route_probe_retries_only_timeouts() {
        let timeout_capture = ProbeCapture {
            calls: Arc::new(AtomicUsize::new(0)),
            requests: Arc::new(Mutex::new(Vec::new())),
            delay: Duration::from_millis(80),
            status: StatusCode::OK,
        };
        let timeout_server = start_probe_server(timeout_capture.clone()).await;
        let timeout_probe = ReachabilityProbe::new()
            .expect("probe client")
            .with_timing(Duration::from_millis(20), Duration::from_millis(10));
        let timeout = timeout_probe
            .check(&format!("http://{}", timeout_server.address()))
            .await
            .expect("valid probe URL");
        assert_eq!(timeout.status, ReachabilityStatus::Unreachable);
        assert_eq!(timeout.error_category.as_deref(), Some("timeout"));
        assert_eq!(timeout_capture.calls.load(Ordering::SeqCst), 2);
        timeout_server.shutdown().await;

        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("reset listener");
        let address = listener.local_addr().expect("reset address");
        let resets = Arc::new(AtomicUsize::new(0));
        let reset_count = Arc::clone(&resets);
        let reset_task = tokio::spawn(async move {
            if let Ok((socket, _)) = listener.accept().await {
                reset_count.fetch_add(1, Ordering::SeqCst);
                drop(socket);
            }
        });
        let reset_probe = ReachabilityProbe::new()
            .expect("probe client")
            .with_timing(Duration::from_millis(200), Duration::from_millis(100));
        let reset = reset_probe
            .check(&format!("http://{address}"))
            .await
            .expect("valid probe URL");
        assert_eq!(reset.status, ReachabilityStatus::Unreachable);
        assert_ne!(reset.error_category.as_deref(), Some("timeout"));
        reset_task.await.expect("reset server task");
        tokio::time::sleep(Duration::from_millis(25)).await;
        assert_eq!(resets.load(Ordering::SeqCst), 1);
    }
}
