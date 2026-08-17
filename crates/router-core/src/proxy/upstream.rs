use std::{
    collections::HashSet,
    io::{Cursor, Read},
    pin::Pin,
    sync::Arc,
    time::{Duration, Instant},
};

use async_trait::async_trait;
use axum::{
    body::{Body, Bytes},
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use flate2::read::{DeflateDecoder, GzDecoder, ZlibDecoder};
use futures_util::{Stream, StreamExt, stream};
use serde::Deserialize;

use super::{
    FallbackActivationRequest, FallbackActivator, HistorySink, InferenceStatusService,
    NoopFallbackActivator, NoopRequestTransitionSink, RequestTransitionSink, RoutingSnapshot,
    RoutingSnapshotStore, RuntimeDiagnosticCode, RuntimeDiagnosticComponent,
    RuntimeDiagnosticEvent, RuntimeDiagnosticSink, UpstreamRequestHandler, ValidatedProxyRequest,
    fallback::{
        ClassifiedFailure, FIRST_MEANINGFUL_OUTPUT_TIMEOUT, FailurePolicy, SSE_PREFLIGHT_LIMIT,
        TransportFailure, classify_http, classify_semantic, classify_transport,
        normalize_semantic_error_code,
    },
    history::{NoopHistorySink, NoopRuntimeDiagnosticSink, bounded_string, now_millis},
    local_error_with_request_id,
    sse::{
        MAX_ERROR_CODE_CHARS, MAX_MODEL_CHARS, MAX_RESPONSE_ID_CHARS, MAX_STATUS_CHARS,
        ResponseMetadata, SseObserver, SsePreflightCommitReason, SsePreflightSignal,
        SseStreamOutcome, SseStreamResult, observe_sse_stream_started,
    },
};
use crate::{
    domain::{
        CompletionState, DeliveryState, InferenceFailureReason, InferenceOutcome,
        ServiceTierPolicy, UpstreamAttemptId,
    },
    storage::{AttemptHistoryRecord, FallbackStopReason, FallbackStopRecord, RequestHistoryRecord},
};

const DEFAULT_RESPONSE_LIMIT: usize = 200 * 1024 * 1024;
const MAX_UPSTREAM_ERROR_MESSAGE_CHARS: usize = 1_800;
const RESPONSE_TERMINAL_GRACE: Duration = Duration::from_secs(3);

const fn checked_next_attempt_index(attempt_index: u32) -> Option<u32> {
    attempt_index.checked_add(1)
}

#[derive(Clone)]
pub struct UpstreamForwarderConfig {
    pub connect_timeout: Duration,
    pub header_timeout: Duration,
    pub non_stream_timeout: Duration,
    pub response_wire_limit: usize,
    pub response_decoded_limit: usize,
    pub first_output_timeout: Duration,
    pub preflight_limit: usize,
}

impl Default for UpstreamForwarderConfig {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(30),
            header_timeout: Duration::from_mins(1),
            non_stream_timeout: Duration::from_mins(10),
            response_wire_limit: DEFAULT_RESPONSE_LIMIT,
            response_decoded_limit: DEFAULT_RESPONSE_LIMIT,
            first_output_timeout: FIRST_MEANINGFUL_OUTPUT_TIMEOUT,
            preflight_limit: SSE_PREFLIGHT_LIMIT,
        }
    }
}

pub struct ResponsesForwarder {
    client: reqwest::Client,
    config: UpstreamForwarderConfig,
    history: Arc<dyn HistorySink>,
    diagnostics: Arc<dyn RuntimeDiagnosticSink>,
    inference_status: Option<InferenceStatusService>,
    routing: RoutingSnapshotStore,
    activator: Arc<dyn FallbackActivator>,
    transitions: Arc<dyn RequestTransitionSink>,
}

impl ResponsesForwarder {
    /// Creates the single-attempt Responses forwarder.
    ///
    /// # Errors
    ///
    /// Returns a client construction error.
    pub fn new() -> Result<Self, reqwest::Error> {
        Self::with_config(UpstreamForwarderConfig::default())
    }

    /// Creates a forwarder with explicit response deadlines and limits.
    ///
    /// # Errors
    ///
    /// Returns a client construction error.
    pub fn with_config(config: UpstreamForwarderConfig) -> Result<Self, reqwest::Error> {
        let client = reqwest::Client::builder()
            .connect_timeout(config.connect_timeout)
            .build()?;
        Ok(Self {
            client,
            config,
            history: Arc::new(NoopHistorySink),
            diagnostics: Arc::new(NoopRuntimeDiagnosticSink),
            inference_status: None,
            routing: RoutingSnapshotStore::default(),
            activator: Arc::new(NoopFallbackActivator),
            transitions: Arc::new(NoopRequestTransitionSink),
        })
    }

    #[must_use]
    pub fn with_runtime_services(
        mut self,
        history: Arc<dyn HistorySink>,
        diagnostics: Arc<dyn RuntimeDiagnosticSink>,
        inference_status: InferenceStatusService,
    ) -> Self {
        self.history = history;
        self.diagnostics = diagnostics;
        self.inference_status = Some(inference_status);
        self
    }

    #[must_use]
    pub fn with_fallback_services(
        mut self,
        routing: RoutingSnapshotStore,
        activator: Arc<dyn FallbackActivator>,
    ) -> Self {
        self.routing = routing;
        self.activator = activator;
        self
    }

    #[must_use]
    pub fn with_request_transition_sink(
        mut self,
        transitions: Arc<dyn RequestTransitionSink>,
    ) -> Self {
        self.transitions = transitions;
        self
    }
}

#[async_trait]
impl UpstreamRequestHandler for ResponsesForwarder {
    async fn handle(&self, request: ValidatedProxyRequest) -> Response {
        self.forward(request).await
    }
}

impl ResponsesForwarder {
    fn terminal_response(&self, request_id: &str, response: Response) -> Response {
        if response
            .extensions()
            .get::<StreamingTerminalPending>()
            .is_none()
        {
            self.transitions.request_terminal(request_id);
        }
        response
    }

    fn stopped_response(
        &self,
        request_id: &str,
        attempt_index: u32,
        response: Response,
        reason: FallbackStopReason,
        target_route: Option<&Arc<super::RouteSnapshot>>,
    ) -> Response {
        self.record_fallback_stop(request_id, attempt_index, reason, target_route);
        self.terminal_response(request_id, response)
    }

    async fn forward(&self, request: ValidatedProxyRequest) -> Response {
        let mut routed_request = request;
        let mut routing = Arc::clone(&routed_request.routing);
        let participant_eligible = routing.active_participant_index().is_some();
        let mut attempt_index = 0_u32;
        loop {
            routed_request.routing = Arc::clone(&routing);
            let (response, failure) = match self.forward_once(&routed_request, attempt_index).await
            {
                AttemptResult::Committed(response) => {
                    return self.terminal_response(&routed_request.request_id, response);
                }
                AttemptResult::PrecommitFailure { response, failure } => (response, failure),
            };
            if !routing.enabled {
                return self.stopped_response(
                    &routed_request.request_id,
                    attempt_index,
                    response,
                    FallbackStopReason::FallbackDisabled,
                    None,
                );
            }
            if failure.policy == FailurePolicy::ReturnImmediately || !participant_eligible {
                return self.stopped_response(
                    &routed_request.request_id,
                    attempt_index,
                    response,
                    FallbackStopReason::FailureNotEligible,
                    None,
                );
            }
            let Some(target_route) = routing.next_after(&routed_request.route.route_id) else {
                return self.stopped_response(
                    &routed_request.request_id,
                    attempt_index,
                    response,
                    FallbackStopReason::AllParticipantsAttempted,
                    None,
                );
            };
            // Every upstream contact needs a distinct durable history key.
            let Some(next_attempt_index) = checked_next_attempt_index(attempt_index) else {
                return self.stopped_response(
                    &routed_request.request_id,
                    attempt_index,
                    response,
                    FallbackStopReason::AttemptIndexExhausted,
                    Some(&target_route),
                );
            };
            let activation = self
                .activator
                .activate_next(FallbackActivationRequest {
                    request_id: routed_request.request_id.clone(),
                    routing: Arc::clone(&routing),
                    current_route_id: routed_request.route.route_id.clone(),
                    target_route: Arc::clone(&target_route),
                })
                .await;
            let next_routing = match activation {
                Ok(Some(next_routing)) => next_routing,
                Ok(None) => {
                    return self.stopped_response(
                        &routed_request.request_id,
                        attempt_index,
                        response,
                        FallbackStopReason::StalePolicy,
                        Some(&target_route),
                    );
                }
                Err(super::FallbackActivationError::Persistence) => {
                    self.emit_activation_persistence_failure(
                        &routed_request.request_id,
                        &routed_request.route.route_id,
                    );
                    return self.stopped_response(
                        &routed_request.request_id,
                        attempt_index,
                        response,
                        FallbackStopReason::ActivationFailed,
                        Some(&target_route),
                    );
                }
            };
            routing = next_routing;
            routed_request.route = target_route;
            attempt_index = next_attempt_index;
        }
    }

    async fn forward_once(
        &self,
        request: &ValidatedProxyRequest,
        attempt_index: u32,
    ) -> AttemptResult {
        let context = self.history_context(request, attempt_index);
        let Ok(headers) = build_upstream_headers(request) else {
            return Self::invalid_route_credentials(request, context);
        };
        let endpoint = request.route.base_url.inference_url();
        let body = upstream_request_body(request);
        let started = Instant::now();
        let send = self
            .client
            .post(&endpoint)
            .headers(headers)
            .body(body)
            .send();
        let upstream = match tokio::time::timeout(self.config.header_timeout, send).await {
            Err(_) => {
                let failure = classify_transport(TransportFailure::ElapsedTimeout);
                context.finish_failure(
                    None,
                    DeliveryState::None,
                    ResponseMetadata::default(),
                    failure,
                    RuntimeDiagnosticCode::UpstreamTimeout,
                );
                return AttemptResult::failure(
                    local_error_with_request_id(
                        StatusCode::GATEWAY_TIMEOUT,
                        failure.category,
                        "The upstream response headers timed out.",
                        request.request_id.clone(),
                    ),
                    failure,
                );
            }
            Ok(Err(error)) => {
                let (transport, diagnostic, message) = if error.is_timeout() {
                    (
                        TransportFailure::ElapsedTimeout,
                        RuntimeDiagnosticCode::UpstreamTimeout,
                        "The upstream response headers timed out.",
                    )
                } else if error.is_connect() {
                    (
                        TransportFailure::FastConnection,
                        RuntimeDiagnosticCode::UpstreamConnectionFailed,
                        "The upstream connection could not be established.",
                    )
                } else {
                    (
                        TransportFailure::FastRequest,
                        RuntimeDiagnosticCode::UpstreamRequestFailed,
                        "The upstream request was interrupted before response headers were received.",
                    )
                };
                let failure = classify_transport(transport);
                context.finish_failure(
                    None,
                    DeliveryState::None,
                    ResponseMetadata::default(),
                    failure,
                    diagnostic,
                );
                return AttemptResult::failure(
                    local_error_with_request_id(
                        if transport == TransportFailure::ElapsedTimeout {
                            StatusCode::GATEWAY_TIMEOUT
                        } else {
                            StatusCode::BAD_GATEWAY
                        },
                        failure.category,
                        message,
                        request.request_id.clone(),
                    ),
                    failure,
                );
            }
            Ok(Ok(response)) => response,
        };

        if !upstream.status().is_success() {
            return self
                .handle_error_response(upstream, request, context, &endpoint, started)
                .await;
        }
        if request.stream {
            if request.routing.enabled
                && self.policy_current(&request.routing, &request.route.route_id)
            {
                return self
                    .preflight_stream(upstream, request, context, request.request_started)
                    .await;
            }
            return AttemptResult::Committed(streaming_response(
                upstream,
                request.request_started,
                context,
            ));
        }
        self.handle_non_streaming_response(upstream, request, context, started)
            .await
    }

    fn invalid_route_credentials(
        request: &ValidatedProxyRequest,
        context: RequestHistoryContext,
    ) -> AttemptResult {
        context.finish_local(
            CompletionState::Failed,
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            RuntimeDiagnosticCode::InvalidRequest,
        );
        AttemptResult::Committed(local_error_with_request_id(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            "The active route credentials could not be applied.",
            request.request_id.clone(),
        ))
    }

    fn history_context(
        &self,
        request: &ValidatedProxyRequest,
        attempt_index: u32,
    ) -> RequestHistoryContext {
        RequestHistoryContext::new(
            request,
            attempt_index,
            Arc::clone(&self.history),
            Arc::clone(&self.diagnostics),
            self.inference_status.clone(),
            Arc::clone(&self.transitions),
        )
    }

    fn policy_current(
        &self,
        expected: &RoutingSnapshot,
        route_id: &crate::domain::RouteId,
    ) -> bool {
        let current = self.routing.load();
        current.enabled
            && current.config_revision == expected.config_revision
            && current.selection_generation == expected.selection_generation
            && current.active.as_ref().map(|route| &route.route_id) == Some(route_id)
    }

    fn emit_activation_persistence_failure(
        &self,
        request_id: &str,
        route_id: &crate::domain::RouteId,
    ) {
        self.diagnostics.emit(RuntimeDiagnosticEvent {
            component: RuntimeDiagnosticComponent::Upstream,
            code: RuntimeDiagnosticCode::FallbackActivationPersistenceFailed,
            request_id: Some(request_id.to_owned()),
            route_id: Some(route_id.clone()),
            http_status: None,
        });
    }

    fn record_fallback_stop(
        &self,
        request_id: &str,
        attempt_index: u32,
        reason: FallbackStopReason,
        target_route: Option<&Arc<super::RouteSnapshot>>,
    ) {
        let _ = self.history.try_record_fallback_stop(FallbackStopRecord {
            request_id: request_id.to_owned(),
            attempt_index,
            reason,
            target_route_id: target_route.map(|route| route.route_id.clone()),
            target_route_name: target_route.map(|route| route.name.clone()),
        });
    }

    async fn handle_non_streaming_response(
        &self,
        upstream: reqwest::Response,
        request: &ValidatedProxyRequest,
        context: RequestHistoryContext,
        started: Instant,
    ) -> AttemptResult {
        let upstream_status = upstream.status();
        let remaining = self
            .config
            .non_stream_timeout
            .saturating_sub(started.elapsed());
        match tokio::time::timeout(remaining, self.non_streaming_response(upstream)).await {
            Err(_) => {
                let failure = classify_transport(TransportFailure::ElapsedTimeout);
                context.finish_failure(
                    Some(upstream_status),
                    DeliveryState::Started,
                    ResponseMetadata::default(),
                    failure,
                    RuntimeDiagnosticCode::UpstreamTimeout,
                );
                AttemptResult::failure(
                    local_error_with_request_id(
                        StatusCode::GATEWAY_TIMEOUT,
                        failure.category,
                        "The upstream response body timed out.",
                        request.request_id.clone(),
                    ),
                    failure,
                )
            }
            Ok(Err(error)) => {
                let failure = error.classification();
                let diagnostic = error.diagnostic_code();
                context.finish_failure(
                    Some(upstream_status),
                    DeliveryState::Started,
                    ResponseMetadata::default(),
                    failure,
                    diagnostic,
                );
                AttemptResult::failure(error.into_response(request.request_id.clone()), failure)
            }
            Ok(Ok(result)) => {
                let semantic_failure = metadata_indicates_failure(&result.metadata);
                if semantic_failure {
                    let failure = classify_semantic(
                        result.status,
                        result.metadata.status.as_deref(),
                        result.metadata.safe_error_code.as_deref(),
                    );
                    context.finish_failure(
                        Some(result.status),
                        DeliveryState::Completed,
                        result.metadata,
                        failure,
                        RuntimeDiagnosticCode::UpstreamSemanticFailure,
                    );
                    AttemptResult::failure(result.response, failure)
                } else {
                    context.finish_attempt(
                        CompletionState::Completed,
                        Some(result.status),
                        None,
                        DeliveryState::Completed,
                        result.metadata,
                        None,
                        Some(InferenceOutcome::Success),
                        None,
                    );
                    AttemptResult::Committed(result.response)
                }
            }
        }
    }

    async fn handle_error_response(
        &self,
        upstream: reqwest::Response,
        request: &ValidatedProxyRequest,
        context: RequestHistoryContext,
        endpoint: &str,
        started: Instant,
    ) -> AttemptResult {
        let status = upstream.status();
        let remaining = self
            .config
            .non_stream_timeout
            .saturating_sub(started.elapsed());
        match tokio::time::timeout(
            remaining,
            self.normalize_upstream_error(upstream, request, endpoint),
        )
        .await
        {
            Err(_) => {
                let failure = classify_transport(TransportFailure::ElapsedTimeout);
                context.finish_failure(
                    Some(status),
                    DeliveryState::None,
                    ResponseMetadata::default(),
                    failure,
                    RuntimeDiagnosticCode::UpstreamTimeout,
                );
                AttemptResult::failure(
                    local_error_with_request_id(
                        StatusCode::GATEWAY_TIMEOUT,
                        failure.category,
                        "The upstream error response timed out.",
                        request.request_id.clone(),
                    ),
                    failure,
                )
            }
            Ok(Ok(normalized)) => {
                let failure = classify_http(status, normalized.safe_error_code.as_deref());
                context.finish_failure(
                    Some(status),
                    DeliveryState::Completed,
                    ResponseMetadata::default(),
                    failure,
                    RuntimeDiagnosticCode::UpstreamHttpStatus,
                );
                let downstream_status = if status == StatusCode::FORBIDDEN
                    && failure.reason == Some(InferenceFailureReason::AccessDenied)
                {
                    StatusCode::BAD_REQUEST
                } else {
                    status
                };
                AttemptResult::failure(
                    local_error_with_request_id(
                        downstream_status,
                        "upstream_error",
                        &normalized.message,
                        request.request_id.clone(),
                    ),
                    failure,
                )
            }
            Ok(Err(error)) => {
                let failure = error.classification();
                let diagnostic = error.diagnostic_code();
                context.finish_failure(
                    Some(status),
                    DeliveryState::Started,
                    ResponseMetadata::default(),
                    failure,
                    diagnostic,
                );
                AttemptResult::failure(error.into_response(request.request_id.clone()), failure)
            }
        }
    }

    async fn preflight_stream(
        &self,
        upstream: reqwest::Response,
        request: &ValidatedProxyRequest,
        context: RequestHistoryContext,
        request_started: Instant,
    ) -> AttemptResult {
        let mut preflight = SsePreflight::new(upstream, context, request_started);
        let mut changed = self.routing.subscribe();
        let deadline = tokio::time::sleep(self.config.first_output_timeout);
        tokio::pin!(deadline);

        loop {
            tokio::select! {
                policy_change = changed.changed() => {
                    if policy_change.is_err()
                        || !self.policy_current(&request.routing, &request.route.route_id)
                    {
                        return preflight.commit(PreflightCommitReason::PolicyChanged);
                    }
                }
                next = preflight.stream.next() => {
                    match next {
                        Some(Ok(bytes)) => match preflight.push_chunk(bytes, self.config.preflight_limit) {
                            SsePreflightSignal::Continue => {}
                            SsePreflightSignal::Commit => return preflight.commit_observed(),
                            SsePreflightSignal::TerminalFailure => {
                                return preflight.terminal_failure();
                            }
                        },
                        Some(Err(_)) => return preflight.read_failure(
                            &request.request_id,
                            "The upstream response could not be read.",
                        ),
                        None => return preflight.finish_input(&request.request_id),
                    }
                }
                () = &mut deadline => return preflight.timeout(&request.request_id),
            }
        }
    }

    async fn normalize_upstream_error(
        &self,
        response: reqwest::Response,
        request: &ValidatedProxyRequest,
        endpoint: &str,
    ) -> Result<NormalizedUpstreamError, ForwardingError> {
        let status = response.status();
        let encodings = response_encodings(response.headers());
        let wire = collect_wire(response, self.config.response_wire_limit).await?;
        let details = match decode_supported(wire, &encodings, self.config.response_decoded_limit) {
            Ok(decoded) => extract_upstream_error_details(&decoded),
            Err(DecodeError::TooLarge) => return Err(ForwardingError::TooLarge),
            Err(DecodeError::Unsupported | DecodeError::Invalid) => None,
        };
        let mut message = format!(
            "Route '{}' returned HTTP {} for model '{}' at {}.",
            request.route.name, status, request.model, endpoint
        );
        if let Some(upstream_message) = details
            .as_ref()
            .and_then(|details| details.message.as_ref())
        {
            message.push(' ');
            message.push_str(upstream_message);
        }
        truncate_chars(&mut message, MAX_UPSTREAM_ERROR_MESSAGE_CHARS);
        Ok(NormalizedUpstreamError {
            message,
            safe_error_code: details.and_then(|details| details.safe_error_code),
        })
    }

    async fn non_streaming_response(
        &self,
        response: reqwest::Response,
    ) -> Result<NonStreamingResponse, ForwardingError> {
        let status = response.status();
        let source_headers = response.headers().clone();
        let encodings = response_encodings(&source_headers);
        let wire = collect_wire(response, self.config.response_wire_limit).await?;
        let (body, transformed, metadata) =
            match decode_supported(wire.clone(), &encodings, self.config.response_decoded_limit) {
                Ok(decoded) => {
                    let metadata = project_non_streaming_metadata(&decoded);
                    (decoded, !encodings.is_empty(), metadata)
                }
                Err(DecodeError::Unsupported) => {
                    let metadata = ResponseMetadata {
                        complete: false,
                        ..ResponseMetadata::default()
                    };
                    (wire, false, metadata)
                }
                Err(DecodeError::TooLarge) => return Err(ForwardingError::TooLarge),
                Err(DecodeError::Invalid) => return Err(ForwardingError::InvalidEncoding),
            };
        let headers = filtered_response_headers(&source_headers, transformed);
        Ok(NonStreamingResponse {
            response: response_with_headers(status, headers, Body::from(body)),
            status,
            metadata,
        })
    }
}

type BoxByteStream = Pin<Box<dyn Stream<Item = Result<axum::body::Bytes, std::io::Error>> + Send>>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PreflightCommitReason {
    ProtocolMismatch,
    MeaningfulOutput,
    TerminalSuccess,
    UnknownEvent,
    MalformedEvent,
    EventLimit,
    BufferLimit,
    PolicyChanged,
    SemanticFailure,
}

impl PreflightCommitReason {
    const fn diagnostic_code(self) -> RuntimeDiagnosticCode {
        match self {
            Self::ProtocolMismatch => RuntimeDiagnosticCode::UpstreamPreflightProtocolMismatch,
            Self::MeaningfulOutput => RuntimeDiagnosticCode::UpstreamPreflightMeaningfulOutput,
            Self::TerminalSuccess => RuntimeDiagnosticCode::UpstreamPreflightTerminalSuccess,
            Self::UnknownEvent => RuntimeDiagnosticCode::UpstreamPreflightUnknownEvent,
            Self::MalformedEvent => RuntimeDiagnosticCode::UpstreamPreflightMalformedEvent,
            Self::EventLimit => RuntimeDiagnosticCode::UpstreamPreflightEventLimit,
            Self::BufferLimit => RuntimeDiagnosticCode::UpstreamPreflightBufferLimit,
            Self::PolicyChanged => RuntimeDiagnosticCode::UpstreamPreflightPolicyChanged,
            Self::SemanticFailure => RuntimeDiagnosticCode::UpstreamPreflightSemanticFailure,
        }
    }
}

impl From<SsePreflightCommitReason> for PreflightCommitReason {
    fn from(reason: SsePreflightCommitReason) -> Self {
        match reason {
            SsePreflightCommitReason::MeaningfulOutput => Self::MeaningfulOutput,
            SsePreflightCommitReason::TerminalSuccess => Self::TerminalSuccess,
            SsePreflightCommitReason::UnknownEvent => Self::UnknownEvent,
            SsePreflightCommitReason::MalformedEvent => Self::MalformedEvent,
            SsePreflightCommitReason::EventLimit => Self::EventLimit,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProtocolProbeSignal {
    Pending,
    Confirmed,
    Rejected,
}

struct SseProtocolProbe {
    signal: ProtocolProbeSignal,
    line: Vec<u8>,
}

impl SseProtocolProbe {
    fn new(declared_sse: bool) -> Self {
        Self {
            signal: if declared_sse {
                ProtocolProbeSignal::Confirmed
            } else {
                ProtocolProbeSignal::Pending
            },
            line: Vec::new(),
        }
    }

    fn inspect(&mut self, bytes: &[u8]) -> ProtocolProbeSignal {
        if self.signal != ProtocolProbeSignal::Pending {
            return self.signal;
        }
        for &byte in bytes {
            if byte == b'\n' {
                if self.line.last() == Some(&b'\r') {
                    self.line.pop();
                }
                if self.line.is_empty() || self.line.first() == Some(&b':') {
                    self.line.clear();
                    continue;
                }
                self.signal = ProtocolProbeSignal::Rejected;
                break;
            }
            self.line.push(byte);
            if self.line.first() == Some(&b':') || self.line.as_slice() == b"\r" {
                continue;
            }
            if self.line.as_slice() == b"event:" || self.line.as_slice() == b"data:" {
                self.signal = ProtocolProbeSignal::Confirmed;
                break;
            }
            if !b"event:".starts_with(&self.line) && !b"data:".starts_with(&self.line) {
                self.signal = ProtocolProbeSignal::Rejected;
                break;
            }
        }
        self.signal
    }
}

struct SsePreflight {
    status: StatusCode,
    source_headers: HeaderMap,
    headers: HeaderMap,
    stream: BoxByteStream,
    observer: SseObserver,
    probe: SseProtocolProbe,
    declared_sse: bool,
    effective_sse: bool,
    commit_reason: Option<PreflightCommitReason>,
    buffered: Vec<axum::body::Bytes>,
    buffered_bytes: usize,
    started: Instant,
    context: RequestHistoryContext,
}

impl SsePreflight {
    fn new(upstream: reqwest::Response, context: RequestHistoryContext, started: Instant) -> Self {
        let status = upstream.status();
        let source_headers = upstream.headers().clone();
        let declared_sse = is_event_stream(&source_headers);
        let headers = streaming_response_headers(&source_headers, declared_sse);
        let stream = Box::pin(upstream.bytes_stream().map(|result| {
            result.map_err(|_| std::io::Error::other("upstream response stream failed"))
        }));
        Self {
            status,
            source_headers,
            headers,
            stream,
            observer: SseObserver::new(1024 * 1024),
            probe: SseProtocolProbe::new(declared_sse),
            declared_sse,
            effective_sse: declared_sse,
            commit_reason: None,
            buffered: Vec::new(),
            buffered_bytes: 0,
            started,
            context,
        }
    }

    fn push_chunk(&mut self, mut bytes: axum::body::Bytes, limit: usize) -> SsePreflightSignal {
        let remaining = limit.saturating_sub(self.buffered_bytes);
        let over_limit = bytes.len() > remaining;
        let newly_buffered = if over_limit {
            let prefix = if remaining > 0 {
                let prefix = bytes.split_to(remaining);
                self.buffered_bytes = self.buffered_bytes.saturating_add(prefix.len());
                self.buffered.push(prefix.clone());
                Some(prefix)
            } else {
                None
            };
            let rest = std::mem::replace(&mut self.stream, Box::pin(stream::empty()));
            self.stream =
                Box::pin(stream::once(async move { Ok::<_, std::io::Error>(bytes) }).chain(rest));
            prefix
        } else {
            self.buffered_bytes = self.buffered_bytes.saturating_add(bytes.len());
            self.buffered.push(bytes.clone());
            Some(bytes)
        };

        let was_confirmed = self.probe.signal == ProtocolProbeSignal::Confirmed;
        let probe_signal = newly_buffered
            .as_ref()
            .map_or(self.probe.signal, |chunk| self.probe.inspect(chunk));
        match probe_signal {
            ProtocolProbeSignal::Rejected => {
                self.commit_reason = Some(PreflightCommitReason::ProtocolMismatch);
                return SsePreflightSignal::Commit;
            }
            ProtocolProbeSignal::Pending => {
                if over_limit || self.buffered_bytes >= limit {
                    self.commit_reason = Some(PreflightCommitReason::BufferLimit);
                    return SsePreflightSignal::Commit;
                }
                return SsePreflightSignal::Continue;
            }
            ProtocolProbeSignal::Confirmed if !was_confirmed => {
                self.effective_sse = true;
                self.headers = streaming_response_headers(&self.source_headers, true);
                for chunk in &self.buffered {
                    let signal = self.observer.feed_preflight(chunk);
                    if !matches!(signal, SsePreflightSignal::Continue) {
                        self.commit_reason = self
                            .observer
                            .preflight_commit_reason()
                            .map(PreflightCommitReason::from);
                        return signal;
                    }
                }
            }
            ProtocolProbeSignal::Confirmed => {
                if let Some(chunk) = newly_buffered.as_ref() {
                    let signal = self.observer.feed_preflight(chunk);
                    if !matches!(signal, SsePreflightSignal::Continue) {
                        self.commit_reason = self
                            .observer
                            .preflight_commit_reason()
                            .map(PreflightCommitReason::from);
                        return signal;
                    }
                }
            }
        }
        if over_limit || self.buffered_bytes >= limit {
            self.commit_reason = Some(PreflightCommitReason::BufferLimit);
            SsePreflightSignal::Commit
        } else {
            SsePreflightSignal::Continue
        }
    }

    fn commit_observed(self) -> AttemptResult {
        let reason = self
            .commit_reason
            .or_else(|| {
                self.observer
                    .preflight_commit_reason()
                    .map(PreflightCommitReason::from)
            })
            .unwrap_or(PreflightCommitReason::MalformedEvent);
        self.commit(reason)
    }

    fn commit(self, reason: PreflightCommitReason) -> AttemptResult {
        self.commit_with_stop_reason(reason, None)
    }

    fn commit_with_stop_reason(
        self,
        reason: PreflightCommitReason,
        stop_reason: Option<FallbackStopReason>,
    ) -> AttemptResult {
        AttemptResult::Committed(streaming_response_from_parts(
            self.status,
            self.headers,
            self.buffered,
            self.stream,
            self.started,
            self.context,
            stop_reason,
            self.effective_sse,
            Some(reason),
        ))
    }

    fn terminal_failure(self) -> AttemptResult {
        let metadata = self.observer.metadata().clone();
        let failure = classify_semantic(
            self.status,
            metadata.status.as_deref(),
            metadata.safe_error_code.as_deref(),
        );
        if failure.policy == FailurePolicy::ReturnImmediately {
            return self.commit_with_stop_reason(
                PreflightCommitReason::SemanticFailure,
                Some(FallbackStopReason::FailureNotEligible),
            );
        }
        self.context.finish_failure(
            Some(self.status),
            DeliveryState::None,
            metadata,
            failure,
            RuntimeDiagnosticCode::UpstreamPreflightSemanticFailure,
        );
        AttemptResult::failure(
            observed_streaming_response_from_parts(
                self.status,
                self.headers,
                self.buffered,
                self.stream,
                self.started,
            ),
            failure,
        )
    }

    fn finish_input(mut self, request_id: &str) -> AttemptResult {
        if !self.effective_sse {
            return self.commit(PreflightCommitReason::ProtocolMismatch);
        }
        if !self.declared_sse
            && matches!(
                self.observer.preflight_signal(),
                SsePreflightSignal::Continue
            )
        {
            return self.commit(PreflightCommitReason::MalformedEvent);
        }
        self.observer.finish_input();
        match self.observer.preflight_signal() {
            SsePreflightSignal::Continue => self.read_failure(
                request_id,
                "The upstream response ended before producing output.",
            ),
            SsePreflightSignal::Commit => self.commit_observed(),
            SsePreflightSignal::TerminalFailure => self.terminal_failure(),
        }
    }

    fn read_failure(self, request_id: &str, message: &'static str) -> AttemptResult {
        let failure = classify_transport(TransportFailure::FastRead);
        self.context.finish_failure(
            Some(self.status),
            DeliveryState::None,
            self.observer.metadata().clone(),
            failure,
            RuntimeDiagnosticCode::UpstreamReadFailed,
        );
        AttemptResult::failure(
            local_error_with_request_id(
                StatusCode::BAD_GATEWAY,
                failure.category,
                message,
                request_id.to_owned(),
            ),
            failure,
        )
    }

    fn timeout(self, request_id: &str) -> AttemptResult {
        let failure = classify_transport(TransportFailure::ElapsedTimeout);
        self.context.finish_failure(
            Some(self.status),
            DeliveryState::None,
            self.observer.metadata().clone(),
            failure,
            RuntimeDiagnosticCode::UpstreamTimeout,
        );
        AttemptResult::failure(
            local_error_with_request_id(
                StatusCode::GATEWAY_TIMEOUT,
                failure.category,
                "The upstream stream did not produce meaningful output in time.",
                request_id.to_owned(),
            ),
            failure,
        )
    }
}

enum AttemptResult {
    Committed(Response),
    PrecommitFailure {
        response: Response,
        failure: ClassifiedFailure,
    },
}

impl AttemptResult {
    fn failure(response: Response, failure: ClassifiedFailure) -> Self {
        Self::PrecommitFailure { response, failure }
    }
}

struct NormalizedUpstreamError {
    message: String,
    safe_error_code: Option<String>,
}

struct NonStreamingResponse {
    response: Response,
    status: StatusCode,
    metadata: ResponseMetadata,
}

struct RequestHistoryContext {
    request_id: String,
    route_id: crate::domain::RouteId,
    route_name: String,
    requested_model: String,
    reasoning_effort: Option<String>,
    requested_service_tier: Option<String>,
    forwarded_service_tier: Option<String>,
    streaming: bool,
    turn_id: Option<String>,
    started_at_ms: i64,
    attempt_started_at_ms: i64,
    attempt_id: UpstreamAttemptId,
    attempt_index: u32,
    history: Arc<dyn HistorySink>,
    diagnostics: Arc<dyn RuntimeDiagnosticSink>,
    inference_status: Option<InferenceStatusService>,
    transitions: Arc<dyn RequestTransitionSink>,
}

impl RequestHistoryContext {
    fn new(
        request: &ValidatedProxyRequest,
        attempt_index: u32,
        history: Arc<dyn HistorySink>,
        diagnostics: Arc<dyn RuntimeDiagnosticSink>,
        inference_status: Option<InferenceStatusService>,
        transitions: Arc<dyn RequestTransitionSink>,
    ) -> Self {
        Self {
            request_id: request.request_id.clone(),
            route_id: request.route.route_id.clone(),
            route_name: request.route.name.clone(),
            requested_model: bounded_string(request.model.clone(), 512),
            reasoning_effort: request.reasoning_effort.clone(),
            requested_service_tier: request
                .service_tier
                .clone()
                .map(|value| bounded_string(value, 64)),
            forwarded_service_tier: match request.route.service_tier_policy {
                ServiceTierPolicy::Passthrough => request
                    .service_tier
                    .clone()
                    .map(|value| bounded_string(value, 64)),
                ServiceTierPolicy::Omit => None,
            },
            streaming: request.stream,
            turn_id: request.turn_id.clone(),
            started_at_ms: request.started_at_ms,
            attempt_started_at_ms: now_millis(),
            attempt_id: UpstreamAttemptId::new(),
            attempt_index,
            history,
            diagnostics,
            inference_status,
            transitions,
        }
    }

    fn finish_local(
        self,
        completion_state: CompletionState,
        status: StatusCode,
        error_category: &'static str,
        diagnostic: RuntimeDiagnosticCode,
    ) {
        self.finish(
            completion_state,
            Some(status),
            Some(error_category),
            None,
            ResponseMetadata::default(),
            Some(diagnostic),
            None,
            None,
        );
    }

    fn finish_failure(
        self,
        status: Option<StatusCode>,
        delivery_state: DeliveryState,
        metadata: ResponseMetadata,
        failure: ClassifiedFailure,
        diagnostic: RuntimeDiagnosticCode,
    ) {
        self.finish_attempt(
            CompletionState::Failed,
            status,
            Some(failure.category),
            delivery_state,
            metadata,
            Some(diagnostic),
            Some(InferenceOutcome::Failure),
            failure.reason,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn finish_attempt(
        self,
        completion_state: CompletionState,
        status: Option<StatusCode>,
        error_category: Option<&'static str>,
        delivery_state: DeliveryState,
        metadata: ResponseMetadata,
        diagnostic: Option<RuntimeDiagnosticCode>,
        inference_outcome: Option<InferenceOutcome>,
        failure_reason: Option<InferenceFailureReason>,
    ) {
        self.finish(
            completion_state,
            status,
            error_category,
            Some(delivery_state),
            metadata,
            diagnostic,
            inference_outcome,
            failure_reason,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn finish(
        self,
        completion_state: CompletionState,
        status: Option<StatusCode>,
        error_category: Option<&'static str>,
        delivery_state: Option<DeliveryState>,
        metadata: ResponseMetadata,
        diagnostic: Option<RuntimeDiagnosticCode>,
        inference_outcome: Option<InferenceOutcome>,
        failure_reason: Option<InferenceFailureReason>,
    ) {
        let finished_at_ms = now_millis();
        if let (Some(service), Some(outcome)) = (&self.inference_status, inference_outcome) {
            service.record_result_with_reason(
                &self.route_id,
                outcome,
                failure_reason,
                finished_at_ms,
            );
        }
        if let Some(code) = diagnostic {
            self.diagnostics.emit(RuntimeDiagnosticEvent {
                component: RuntimeDiagnosticComponent::Upstream,
                code,
                request_id: Some(self.request_id.clone()),
                route_id: Some(self.route_id.clone()),
                http_status: status.map(|status| status.as_u16()),
            });
        }
        let attempt = delivery_state.map(|delivery_state| AttemptHistoryRecord {
            attempt_id: self.attempt_id,
            attempt_index: self.attempt_index,
            route_id: self.route_id.clone(),
            route_name: self.route_name.clone(),
            started_at_ms: self.attempt_started_at_ms,
            finished_at_ms,
            http_status: status.map(|status| status.as_u16()),
            error_category: error_category.map(str::to_owned),
            delivery_state,
            input_tokens: metadata.input_tokens.and_then(u64_to_i64),
            output_tokens: metadata.output_tokens.and_then(u64_to_i64),
            total_tokens: metadata.total_tokens.and_then(u64_to_i64),
            actual_model: metadata
                .model
                .clone()
                .map(|model| bounded_string(model, MAX_MODEL_CHARS)),
            forwarded_service_tier: self.forwarded_service_tier,
            actual_service_tier: metadata
                .service_tier
                .clone()
                .map(|tier| bounded_string(tier, MAX_STATUS_CHARS)),
            cached_input_tokens: metadata.cached_input_tokens.and_then(u64_to_i64),
            cache_write_input_tokens: metadata.cache_write_input_tokens.and_then(u64_to_i64),
        });
        let _ = self.history.try_record(RequestHistoryRecord {
            request_id: self.request_id,
            started_at_ms: self.started_at_ms,
            finished_at_ms,
            turn_id: self.turn_id,
            requested_model: Some(self.requested_model),
            reasoning_effort: self.reasoning_effort,
            requested_service_tier: self.requested_service_tier,
            actual_model: metadata
                .model
                .map(|model| bounded_string(model, MAX_MODEL_CHARS)),
            final_route_id: Some(self.route_id),
            final_route_name: Some(self.route_name),
            streaming: self.streaming,
            completion_state,
            http_status: status.map(|status| status.as_u16()),
            error_category: error_category.map(str::to_owned),
            input_tokens: metadata.input_tokens.and_then(u64_to_i64),
            output_tokens: metadata.output_tokens.and_then(u64_to_i64),
            total_tokens: metadata.total_tokens.and_then(u64_to_i64),
            actual_service_tier: metadata
                .service_tier
                .map(|tier| bounded_string(tier, MAX_STATUS_CHARS)),
            cached_input_tokens: metadata.cached_input_tokens.and_then(u64_to_i64),
            cache_write_input_tokens: metadata.cache_write_input_tokens.and_then(u64_to_i64),
            total_latency_ms: Some(finished_at_ms.saturating_sub(self.started_at_ms)),
            first_output_latency_ms: metadata.first_output_latency_ms.and_then(u64_to_i64),
            metadata_complete: metadata.complete,
            fallback_stop_reason: None,
            fallback_stop_target_route_id: None,
            fallback_stop_target_route_name: None,
            attempts: attempt.into_iter().collect(),
        });
    }
}

fn u64_to_i64(value: u64) -> Option<i64> {
    value.try_into().ok()
}

fn metadata_indicates_failure(metadata: &ResponseMetadata) -> bool {
    metadata.safe_error_code.is_some()
        || metadata
            .status
            .as_deref()
            .is_some_and(|status| matches!(status, "failed" | "cancelled" | "incomplete"))
}

fn project_non_streaming_metadata(body: &[u8]) -> ResponseMetadata {
    let Ok(projection) = serde_json::from_slice::<NonStreamingProjection>(body) else {
        return ResponseMetadata {
            complete: false,
            ..ResponseMetadata::default()
        };
    };
    ResponseMetadata {
        response_id: projection
            .id
            .map(|value| bounded_string(value, MAX_RESPONSE_ID_CHARS)),
        model: projection
            .model
            .map(|value| bounded_string(value, MAX_MODEL_CHARS)),
        service_tier: projection
            .service_tier
            .map(|value| bounded_string(value, MAX_STATUS_CHARS)),
        status: projection
            .status
            .map(|value| bounded_string(value, MAX_STATUS_CHARS)),
        safe_error_code: projection
            .error
            .and_then(NonStreamingErrorProjection::safe_error_code)
            .map(|value| bounded_string(value, MAX_ERROR_CODE_CHARS)),
        input_tokens: projection.usage.as_ref().and_then(|usage| usage.input),
        output_tokens: projection.usage.as_ref().and_then(|usage| usage.output),
        total_tokens: projection.usage.as_ref().and_then(|usage| usage.total),
        cached_input_tokens: projection
            .usage
            .as_ref()
            .and_then(|usage| usage.input_details.as_ref())
            .and_then(|details| details.cached),
        cache_write_input_tokens: projection
            .usage
            .as_ref()
            .and_then(|usage| usage.input_details.as_ref())
            .and_then(|details| details.cache_write),
        first_output_latency_ms: None,
        complete: true,
    }
}

#[derive(Deserialize)]
struct NonStreamingProjection {
    id: Option<String>,
    model: Option<String>,
    service_tier: Option<String>,
    status: Option<String>,
    usage: Option<NonStreamingUsageProjection>,
    error: Option<NonStreamingErrorProjection>,
}

#[derive(Deserialize)]
struct NonStreamingUsageProjection {
    #[serde(rename = "input_tokens")]
    input: Option<u64>,
    #[serde(rename = "output_tokens")]
    output: Option<u64>,
    #[serde(rename = "total_tokens")]
    total: Option<u64>,
    #[serde(rename = "input_tokens_details")]
    input_details: Option<NonStreamingInputTokenDetailsProjection>,
}

#[derive(Deserialize)]
struct NonStreamingInputTokenDetailsProjection {
    #[serde(rename = "cached_tokens")]
    cached: Option<u64>,
    #[serde(rename = "cache_write_tokens")]
    cache_write: Option<u64>,
}

#[derive(Deserialize)]
struct NonStreamingErrorProjection {
    code: Option<String>,
    codex_error_info: Option<String>,
    message: Option<String>,
}

impl NonStreamingErrorProjection {
    fn safe_error_code(self) -> Option<String> {
        normalize_semantic_error_code(
            self.code.as_deref(),
            self.codex_error_info.as_deref(),
            self.message.as_deref(),
        )
    }
}

fn upstream_request_body(request: &ValidatedProxyRequest) -> Bytes {
    match request.route.service_tier_policy {
        ServiceTierPolicy::Passthrough => request.body.clone(),
        ServiceTierPolicy::Omit => request
            .body_without_service_tier
            .clone()
            .unwrap_or_else(|| request.body.clone()),
    }
}

fn build_upstream_headers(request: &ValidatedProxyRequest) -> Result<HeaderMap, ()> {
    let mut headers = HeaderMap::new();
    let connection_tokens = connection_nominated_headers(&request.headers);
    for (name, value) in &request.headers {
        if !remove_request_header(name) && !connection_tokens.contains(name) {
            headers.append(name.clone(), value.clone());
        }
    }
    let mut bearer = Vec::with_capacity(7 + request.route.api_key.expose().len());
    bearer.extend_from_slice(b"Bearer ");
    bearer.extend_from_slice(request.route.api_key.expose());
    headers.insert(
        header::AUTHORIZATION,
        HeaderValue::from_bytes(&bearer).map_err(|_| ())?,
    );
    if !headers.contains_key(header::CONTENT_TYPE) {
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
    }
    if request.stream {
        headers.insert(
            header::ACCEPT_ENCODING,
            HeaderValue::from_static("identity"),
        );
    }
    Ok(headers)
}

pub(super) fn remove_request_header(name: &HeaderName) -> bool {
    let name_text = name.as_str();
    matches!(
        name_text,
        "host"
            | "content-length"
            | "transfer-encoding"
            | "content-encoding"
            | "authorization"
            | "x-api-key"
            | "x-goog-api-key"
            | "forwarded"
            | "x-real-ip"
            | "true-client-ip"
            | "fastly-client-ip"
            | "x-client-ip"
            | "x-cluster-client-ip"
            | "x-original-forwarded-for"
            | "x-original-client-ip"
            | "cf-connecting-ip"
            | "cf-connecting-ipv6"
            | "cf-ipcountry"
            | "cf-ray"
            | "akamai-client-ip"
            | "x-azure-clientip"
            | "x-azure-ref"
            | "x-request-id"
            | "x-correlation-id"
            | "x-trace-id"
            | "x-amzn-trace-id"
            | "b3"
            | "x-b3-traceid"
            | "x-b3-spanid"
            | "x-b3-parentspanid"
            | "x-b3-sampled"
            | "x-b3-flags"
            | "traceparent"
            | "tracestate"
    ) || is_hop_by_hop(name)
        || name_text.starts_with("x-forwarded-")
        || name_text.starts_with("x-b3-")
        || name_text.starts_with("x-akamai-")
}

fn streaming_response(
    response: reqwest::Response,
    started: Instant,
    context: RequestHistoryContext,
) -> Response {
    let status = response.status();
    let is_sse = is_event_stream(response.headers());
    let headers = streaming_response_headers(response.headers(), is_sse);
    let stream: BoxByteStream = Box::pin(response.bytes_stream().map(|result| {
        result.map_err(|_| std::io::Error::other("upstream response stream failed"))
    }));
    streaming_response_from_parts(
        status,
        headers,
        Vec::new(),
        stream,
        started,
        context,
        None,
        is_sse,
        None,
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "stream reconstruction keeps transport parts and precommit context explicit"
)]
fn streaming_response_from_parts(
    status: StatusCode,
    headers: HeaderMap,
    prefix: Vec<axum::body::Bytes>,
    stream: BoxByteStream,
    started: Instant,
    context: RequestHistoryContext,
    precommit_stop_reason: Option<FallbackStopReason>,
    effective_sse: bool,
    preflight_commit_reason: Option<PreflightCommitReason>,
) -> Response {
    let combined = stream::iter(prefix.into_iter().map(Ok::<_, std::io::Error>)).chain(stream);
    let observed = observe_sse_stream_started(
        combined,
        effective_sse.then_some(RESPONSE_TERMINAL_GRACE),
        started,
        move |result| {
            finish_stream_attempt(
                context,
                status,
                result,
                precommit_stop_reason,
                preflight_commit_reason,
            );
        },
    );
    let mut response = response_with_headers(status, headers, Body::from_stream(observed));
    response.extensions_mut().insert(StreamingTerminalPending);
    response
}

#[derive(Clone)]
struct StreamingTerminalPending;

fn observed_streaming_response_from_parts(
    status: StatusCode,
    headers: HeaderMap,
    prefix: Vec<axum::body::Bytes>,
    stream: BoxByteStream,
    started: Instant,
) -> Response {
    let combined = stream::iter(prefix.into_iter().map(Ok::<_, std::io::Error>)).chain(stream);
    let observed =
        observe_sse_stream_started(combined, Some(RESPONSE_TERMINAL_GRACE), started, |_| {});
    response_with_headers(status, headers, Body::from_stream(observed))
}

fn finish_stream_attempt(
    context: RequestHistoryContext,
    status: StatusCode,
    result: SseStreamResult,
    precommit_stop_reason: Option<FallbackStopReason>,
    preflight_commit_reason: Option<PreflightCommitReason>,
) {
    let request_id = context.request_id.clone();
    let attempt_index = context.attempt_index;
    let history = Arc::clone(&context.history);
    let transitions = Arc::clone(&context.transitions);
    let semantic_failure = metadata_indicates_failure(&result.metadata);
    let response_committed =
        semantic_failure || matches!(result.outcome, SseStreamOutcome::UpstreamReadFailed);
    let semantic_classification = semantic_failure.then(|| {
        classify_semantic(
            status,
            result.metadata.status.as_deref(),
            result.metadata.safe_error_code.as_deref(),
        )
    });
    match result.outcome {
        SseStreamOutcome::Completed | SseStreamOutcome::TerminalGraceElapsed => context
            .finish_attempt(
                if semantic_failure {
                    CompletionState::Failed
                } else {
                    CompletionState::Completed
                },
                Some(status),
                semantic_classification.map(|failure| failure.category),
                DeliveryState::Completed,
                result.metadata,
                semantic_failure.then_some(preflight_commit_reason.map_or(
                    RuntimeDiagnosticCode::UpstreamSemanticFailure,
                    PreflightCommitReason::diagnostic_code,
                )),
                Some(if semantic_failure {
                    InferenceOutcome::Failure
                } else {
                    InferenceOutcome::Success
                }),
                semantic_classification.and_then(|failure| failure.reason),
            ),
        SseStreamOutcome::UpstreamReadFailed => context.finish_attempt(
            CompletionState::Failed,
            Some(status),
            Some("upstream_read_failed"),
            DeliveryState::Started,
            result.metadata,
            Some(preflight_commit_reason.map_or(
                RuntimeDiagnosticCode::UpstreamReadFailed,
                PreflightCommitReason::diagnostic_code,
            )),
            Some(InferenceOutcome::Failure),
            Some(InferenceFailureReason::Connection),
        ),
        SseStreamOutcome::DownstreamCancelled => context.finish_attempt(
            CompletionState::Cancelled,
            Some(status),
            None,
            DeliveryState::Started,
            result.metadata,
            None,
            None,
            None,
        ),
    }
    if let Some(reason) = precommit_stop_reason
        .or_else(|| response_committed.then_some(FallbackStopReason::ResponseCommitted))
    {
        let _ = history.try_record_fallback_stop(FallbackStopRecord {
            request_id: request_id.clone(),
            attempt_index,
            reason,
            target_route_id: None,
            target_route_name: None,
        });
    }
    transitions.request_terminal(&request_id);
}

fn streaming_response_headers(source: &HeaderMap, is_sse: bool) -> HeaderMap {
    let mut headers = filtered_response_headers(source, false);
    if is_sse {
        headers.remove(header::CONTENT_LENGTH);
    }
    headers
}

fn is_event_stream(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("text/event-stream"))
}

fn response_with_headers(status: StatusCode, headers: HeaderMap, body: Body) -> Response {
    let mut response = (status, body).into_response();
    *response.headers_mut() = headers;
    response
}

pub(super) fn filtered_response_headers(source: &HeaderMap, transformed: bool) -> HeaderMap {
    let connection_tokens = connection_nominated_headers(source);
    let mut result = HeaderMap::new();
    for (name, value) in source {
        if is_hop_by_hop(name)
            || connection_tokens.contains(name)
            || (transformed && matches!(name.as_str(), "content-encoding" | "content-length"))
        {
            continue;
        }
        result.append(name.clone(), value.clone());
    }
    result
}

pub(super) fn connection_nominated_headers(headers: &HeaderMap) -> HashSet<HeaderName> {
    headers
        .get_all(header::CONNECTION)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .filter_map(|name| name.trim().parse().ok())
        .collect()
}

fn is_hop_by_hop(name: &HeaderName) -> bool {
    matches!(
        name.as_str(),
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "proxy-connection"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
    )
}

async fn collect_wire(
    response: reqwest::Response,
    limit: usize,
) -> Result<Vec<u8>, ForwardingError> {
    let mut wire = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| ForwardingError::Read)?;
        if wire.len().saturating_add(chunk.len()) > limit {
            return Err(ForwardingError::TooLarge);
        }
        wire.extend_from_slice(&chunk);
    }
    Ok(wire)
}

pub(crate) fn response_encodings(headers: &HeaderMap) -> Vec<String> {
    headers
        .get_all(header::CONTENT_ENCODING)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .filter(|value| !value.is_empty() && *value != "identity")
        .map(str::to_ascii_lowercase)
        .collect()
}

pub(crate) fn decode_supported(
    mut body: Vec<u8>,
    encodings: &[String],
    limit: usize,
) -> Result<Vec<u8>, DecodeError> {
    if encodings.iter().any(|encoding| {
        !matches!(
            encoding.as_str(),
            "gzip" | "x-gzip" | "deflate" | "br" | "zstd" | "zst"
        )
    }) {
        return Err(DecodeError::Unsupported);
    }
    for encoding in encodings.iter().rev() {
        body = match encoding.as_str() {
            "gzip" | "x-gzip" => read_bounded(GzDecoder::new(body.as_slice()), limit)?,
            "deflate" => decode_deflate(&body, limit)?,
            "br" => read_bounded(brotli::Decompressor::new(body.as_slice(), 4096), limit)?,
            "zstd" | "zst" => {
                let decoder = zstd::stream::read::Decoder::new(body.as_slice())
                    .map_err(|_| DecodeError::Invalid)?;
                read_bounded(decoder, limit)?
            }
            _ => return Err(DecodeError::Unsupported),
        };
    }
    if body.len() > limit {
        return Err(DecodeError::TooLarge);
    }
    Ok(body)
}

/// Decodes an MCP image response without geometric `Vec` growth invalidating
/// its reviewed single-call allocation budget.
pub(crate) fn decode_supported_exact(
    mut body: Vec<u8>,
    encodings: &[String],
    limit: usize,
) -> Result<Vec<u8>, DecodeError> {
    if encodings.iter().any(|encoding| {
        !matches!(
            encoding.as_str(),
            "gzip" | "x-gzip" | "deflate" | "br" | "zstd" | "zst"
        )
    }) {
        return Err(DecodeError::Unsupported);
    }
    for encoding in encodings.iter().rev() {
        body = match encoding.as_str() {
            "gzip" | "x-gzip" => read_bounded_exact(GzDecoder::new(body.as_slice()), limit)?,
            "deflate" => decode_deflate_exact(&body, limit)?,
            "br" => read_bounded_exact(brotli::Decompressor::new(body.as_slice(), 4096), limit)?,
            "zstd" | "zst" => {
                let decoder = zstd::stream::read::Decoder::new(body.as_slice())
                    .map_err(|_| DecodeError::Invalid)?;
                read_bounded_exact(decoder, limit)?
            }
            _ => return Err(DecodeError::Unsupported),
        };
    }
    if body.len() > limit {
        return Err(DecodeError::TooLarge);
    }
    Ok(body)
}

fn decode_deflate(body: &[u8], limit: usize) -> Result<Vec<u8>, DecodeError> {
    match read_bounded(ZlibDecoder::new(body), limit) {
        Ok(decoded) => Ok(decoded),
        Err(DecodeError::TooLarge) => Err(DecodeError::TooLarge),
        Err(DecodeError::Invalid | DecodeError::Unsupported) => {
            read_bounded(DeflateDecoder::new(body), limit)
        }
    }
}

fn decode_deflate_exact(body: &[u8], limit: usize) -> Result<Vec<u8>, DecodeError> {
    match read_bounded_exact(ZlibDecoder::new(body), limit) {
        Ok(decoded) => Ok(decoded),
        Err(DecodeError::TooLarge) => Err(DecodeError::TooLarge),
        Err(DecodeError::Invalid | DecodeError::Unsupported) => {
            read_bounded_exact(DeflateDecoder::new(body), limit)
        }
    }
}

fn read_bounded(reader: impl Read, limit: usize) -> Result<Vec<u8>, DecodeError> {
    let mut decoded = Vec::new();
    reader
        .take((limit as u64).saturating_add(1))
        .read_to_end(&mut decoded)
        .map_err(|_| DecodeError::Invalid)?;
    if decoded.len() > limit {
        return Err(DecodeError::TooLarge);
    }
    Ok(decoded)
}

fn read_bounded_exact(mut reader: impl Read, limit: usize) -> Result<Vec<u8>, DecodeError> {
    const CHUNK_BYTES: usize = 64 * 1024;

    let mut decoded = Vec::new();
    decoded
        .try_reserve_exact(limit)
        .map_err(|_| DecodeError::TooLarge)?;
    let mut chunk = vec![0_u8; CHUNK_BYTES].into_boxed_slice();
    loop {
        let remaining_with_probe = limit.saturating_sub(decoded.len()).saturating_add(1);
        let read = reader
            .read(&mut chunk[..remaining_with_probe.min(CHUNK_BYTES)])
            .map_err(|_| DecodeError::Invalid)?;
        if read == 0 {
            break;
        }
        if decoded.len().saturating_add(read) > limit {
            return Err(DecodeError::TooLarge);
        }
        decoded.extend_from_slice(&chunk[..read]);
    }
    Ok(decoded)
}

#[derive(Deserialize)]
struct UpstreamErrorEnvelope {
    error: Option<UpstreamErrorBody>,
    code: Option<String>,
    message: Option<String>,
    detail: Option<String>,
    #[serde(rename = "Code")]
    pascal_code: Option<String>,
    #[serde(rename = "Message")]
    pascal_message: Option<String>,
    #[serde(rename = "Detail")]
    pascal_detail: Option<String>,
}

#[derive(Deserialize)]
struct UpstreamErrorBody {
    code: Option<String>,
    message: Option<String>,
}

struct UpstreamErrorDetails {
    message: Option<String>,
    safe_error_code: Option<String>,
}

fn extract_upstream_error_details(body: &[u8]) -> Option<UpstreamErrorDetails> {
    let envelope: UpstreamErrorEnvelope = serde_json::from_reader(Cursor::new(body)).ok()?;
    let (nested_message, nested_code) = envelope
        .error
        .map_or((None, None), |error| (error.message, error.code));
    let message = nested_message
        .as_deref()
        .or(envelope.message.as_deref())
        .or(envelope.pascal_message.as_deref())
        .or(envelope.detail.as_deref())
        .or(envelope.pascal_detail.as_deref())
        .and_then(normalize_upstream_error_message);
    let safe_error_code = nested_code
        .or(envelope.code)
        .or(envelope.pascal_code)
        .map(|mut code| {
            truncate_chars(&mut code, MAX_ERROR_CODE_CHARS);
            code
        });
    Some(UpstreamErrorDetails {
        message,
        safe_error_code,
    })
}

fn normalize_upstream_error_message(value: &str) -> Option<String> {
    let mut normalized = String::with_capacity(value.len().min(4_096));
    let mut scalars = 0_usize;
    let mut pending_space = false;
    for character in value.chars() {
        if character.is_control() || character.is_whitespace() {
            pending_space = !normalized.is_empty();
            continue;
        }
        if pending_space {
            if scalars.saturating_add(2) > MAX_UPSTREAM_ERROR_MESSAGE_CHARS {
                break;
            }
            normalized.push(' ');
            scalars += 1;
            pending_space = false;
        }
        if scalars >= MAX_UPSTREAM_ERROR_MESSAGE_CHARS {
            break;
        }
        normalized.push(character);
        scalars += 1;
    }
    (!normalized.is_empty()).then_some(normalized)
}

fn truncate_chars(value: &mut String, maximum: usize) {
    if let Some((index, _)) = value.char_indices().nth(maximum) {
        value.truncate(index);
    }
}

enum ForwardingError {
    Read,
    TooLarge,
    InvalidEncoding,
}

impl ForwardingError {
    const fn diagnostic_code(&self) -> RuntimeDiagnosticCode {
        match self {
            Self::Read => RuntimeDiagnosticCode::UpstreamReadFailed,
            Self::TooLarge => RuntimeDiagnosticCode::UpstreamResponseTooLarge,
            Self::InvalidEncoding => RuntimeDiagnosticCode::UpstreamInvalidEncoding,
        }
    }

    fn classification(&self) -> ClassifiedFailure {
        classify_transport(match self {
            Self::Read => TransportFailure::FastRead,
            Self::TooLarge => TransportFailure::ResponseTooLarge,
            Self::InvalidEncoding => TransportFailure::InvalidEncoding,
        })
    }

    fn into_response(self, request_id: String) -> Response {
        match self {
            Self::TooLarge => local_error_with_request_id(
                StatusCode::BAD_GATEWAY,
                "upstream_response_too_large",
                "The upstream response exceeded the local limit.",
                request_id,
            ),
            Self::Read => local_error_with_request_id(
                StatusCode::BAD_GATEWAY,
                "upstream_read_failed",
                "The upstream response could not be read.",
                request_id,
            ),
            Self::InvalidEncoding => local_error_with_request_id(
                StatusCode::BAD_GATEWAY,
                "upstream_invalid_encoding",
                "The upstream response could not be read.",
                request_id,
            ),
        }
    }
}

#[derive(Debug)]
pub(crate) enum DecodeError {
    Unsupported,
    Invalid,
    TooLarge,
}

#[cfg(test)]
mod tests {
    use std::{
        io,
        sync::{
            Arc, Mutex,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        },
    };

    use axum::{
        Router,
        body::{Body, Bytes, to_bytes},
        extract::{Request, State},
        routing::post,
    };
    use futures_util::stream;
    use tokio::{io::AsyncReadExt, net::TcpListener, sync::Notify};

    use super::*;
    use crate::{
        domain::{ApiKey, BaseUrl, InferenceStatusKind, RouteId},
        proxy::{InferenceStatusChangeSink, LocalErrorDto, ProxyServerHandle, RouteSnapshot},
    };

    fn request(stream: bool) -> ValidatedProxyRequest {
        request_for_base(stream, "https://example.test/v1")
    }

    fn request_for_base(stream: bool, base_url: &str) -> ValidatedProxyRequest {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("localhost"));
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer local"),
        );
        headers.insert("x-api-key", HeaderValue::from_static("local"));
        headers.insert("x-forwarded-for", HeaderValue::from_static("127.0.0.1"));
        headers.insert("traceparent", HeaderValue::from_static("trace"));
        headers.insert("x-codex-turn-state", HeaderValue::from_static("sticky"));
        headers.insert("chatgpt-account-id", HeaderValue::from_static("account"));
        headers.insert("x-oai-attestation", HeaderValue::from_static("attestation"));
        headers.insert(header::COOKIE, HeaderValue::from_static("session=value"));
        headers.insert("x-future-field", HeaderValue::from_static("future"));
        let route = Arc::new(RouteSnapshot {
            route_id: RouteId::new(),
            name: "Primary".to_owned(),
            base_url: BaseUrl::parse(base_url).expect("base URL"),
            api_key: Arc::new(ApiKey::parse("upstream-secret").expect("API key")),
            service_tier_policy: ServiceTierPolicy::Passthrough,
        });
        ValidatedProxyRequest {
            request_id: "request-id".to_owned(),
            started_at_ms: now_millis(),
            request_started: Instant::now(),
            turn_id: None,
            body: Bytes::from_static(br#"{"model":"gpt-test","reasoning":{"effort":"high"}}"#),
            body_without_service_tier: None,
            model: "gpt-test".to_owned(),
            reasoning_effort: Some("high".to_owned()),
            service_tier: None,
            stream,
            route: Arc::clone(&route),
            routing: Arc::new(RoutingSnapshot {
                active: Some(Arc::clone(&route)),
                participants: vec![route],
                enabled: false,
                selection_generation: 0,
                config_revision: 0,
                images_generation_enabled: false,
                images_route: None,
                images_generation_timeout: Duration::from_mins(10),
            }),
            headers,
        }
    }

    #[derive(Clone)]
    struct MockUpstream {
        status: StatusCode,
        headers: HeaderMap,
        body: Bytes,
        header_delay: Duration,
        body_delay: Duration,
        requests: Arc<Mutex<Vec<HeaderMap>>>,
        request_bodies: Arc<Mutex<Vec<Bytes>>>,
        request_paths: Arc<Mutex<Vec<String>>>,
    }

    async fn mock_upstream_handler(
        State(state): State<MockUpstream>,
        request: Request,
    ) -> Response {
        state
            .request_paths
            .lock()
            .expect("request path capture mutex")
            .push(request.uri().path().to_owned());
        state
            .requests
            .lock()
            .expect("request capture mutex")
            .push(request.headers().clone());
        let request_body = to_bytes(request.into_body(), 1024 * 1024)
            .await
            .expect("mock request body");
        state
            .request_bodies
            .lock()
            .expect("request body capture mutex")
            .push(request_body);
        tokio::time::sleep(state.header_delay).await;
        let body = state.body.clone();
        let delay = state.body_delay;
        let stream = stream::once(async move {
            tokio::time::sleep(delay).await;
            Ok::<_, io::Error>(body)
        });
        response_with_headers(state.status, state.headers, Body::from_stream(stream))
    }

    async fn start_mock_upstream(state: MockUpstream) -> ProxyServerHandle {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("mock listener");
        ProxyServerHandle::from_listener(
            listener,
            Router::new()
                .route("/v1/responses", post(mock_upstream_handler))
                .with_state(state),
        )
    }

    fn mock_upstream(status: StatusCode, body: impl Into<Bytes>) -> MockUpstream {
        MockUpstream {
            status,
            headers: HeaderMap::new(),
            body: body.into(),
            header_delay: Duration::ZERO,
            body_delay: Duration::ZERO,
            requests: Arc::new(Mutex::new(Vec::new())),
            request_bodies: Arc::new(Mutex::new(Vec::new())),
            request_paths: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn route_snapshot(name: &str, base_url: &str) -> Arc<RouteSnapshot> {
        Arc::new(RouteSnapshot {
            route_id: RouteId::new(),
            name: name.to_owned(),
            base_url: BaseUrl::parse(base_url).expect("base URL"),
            api_key: Arc::new(ApiKey::parse(&format!("{name}-key")).expect("API key")),
            service_tier_policy: ServiceTierPolicy::Passthrough,
        })
    }

    fn fallback_request(
        stream: bool,
        participants: Vec<Arc<RouteSnapshot>>,
    ) -> (ValidatedProxyRequest, RoutingSnapshotStore) {
        fallback_request_from_index(stream, participants, 0)
    }

    fn fallback_request_from_index(
        stream: bool,
        participants: Vec<Arc<RouteSnapshot>>,
        active_index: usize,
    ) -> (ValidatedProxyRequest, RoutingSnapshotStore) {
        let active = Arc::clone(
            participants
                .get(active_index)
                .expect("fallback participant"),
        );
        let routing = RoutingSnapshotStore::new(RoutingSnapshot {
            active: Some(Arc::clone(&active)),
            participants,
            enabled: true,
            selection_generation: 7,
            config_revision: 11,
            images_generation_enabled: false,
            images_route: None,
            images_generation_timeout: Duration::from_mins(10),
        });
        let mut request = request_for_base(stream, active.base_url.as_str());
        request.route = active;
        request.routing = routing.load();
        (request, routing)
    }

    struct InMemoryFallbackActivator {
        routing: RoutingSnapshotStore,
        activations: Mutex<Vec<(RouteId, RouteId)>>,
        fail_persistence: AtomicBool,
    }

    impl InMemoryFallbackActivator {
        fn new(routing: RoutingSnapshotStore) -> Self {
            Self {
                routing,
                activations: Mutex::new(Vec::new()),
                fail_persistence: AtomicBool::new(false),
            }
        }

        fn failing(routing: RoutingSnapshotStore) -> Self {
            Self {
                routing,
                activations: Mutex::new(Vec::new()),
                fail_persistence: AtomicBool::new(true),
            }
        }
    }

    #[async_trait]
    impl FallbackActivator for InMemoryFallbackActivator {
        async fn activate_next(
            &self,
            request: FallbackActivationRequest,
        ) -> Result<Option<Arc<RoutingSnapshot>>, super::super::FallbackActivationError> {
            if self.fail_persistence.load(Ordering::Acquire) {
                return Err(super::super::FallbackActivationError::Persistence);
            }
            let current = self.routing.load();
            let expected_target = current.next_after(&request.current_route_id);
            let current_matches = current.enabled
                && current.selection_generation == request.routing.selection_generation
                && current.config_revision == request.routing.config_revision
                && current.active.as_ref().map(|route| &route.route_id)
                    == Some(&request.current_route_id)
                && expected_target.as_ref().map(|route| &route.route_id)
                    == Some(&request.target_route.route_id);
            if !current_matches {
                return Ok(None);
            }
            let snapshot = Arc::new(RoutingSnapshot {
                active: Some(Arc::clone(&request.target_route)),
                participants: current.participants.clone(),
                enabled: true,
                selection_generation: current.selection_generation.saturating_add(1),
                config_revision: current.config_revision,
                images_generation_enabled: current.images_generation_enabled,
                images_route: current.images_route.clone(),
                images_generation_timeout: current.images_generation_timeout,
            });
            self.activations.lock().expect("activation mutex").push((
                request.current_route_id,
                request.target_route.route_id.clone(),
            ));
            self.routing.store(Arc::clone(&snapshot));
            Ok(Some(snapshot))
        }
    }

    #[derive(Clone)]
    struct SequenceUpstream {
        statuses: Arc<Vec<StatusCode>>,
        requests: Arc<AtomicUsize>,
    }

    async fn sequence_upstream_handler(
        State(state): State<SequenceUpstream>,
        request: Request,
    ) -> Response {
        let _ = to_bytes(request.into_body(), 1024 * 1024).await;
        let index = state.requests.fetch_add(1, Ordering::AcqRel);
        let status = state
            .statuses
            .get(index)
            .copied()
            .or_else(|| state.statuses.last().copied())
            .expect("sequence status");
        let body = if status.is_success() {
            Bytes::from_static(br#"{"id":"response-id","status":"completed"}"#)
        } else {
            Bytes::from_static(br#"{"error":{"code":"server_error"}}"#)
        };
        response_with_headers(status, HeaderMap::new(), Body::from(body))
    }

    async fn start_sequence_upstream(state: SequenceUpstream) -> ProxyServerHandle {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("sequence listener");
        ProxyServerHandle::from_listener(
            listener,
            Router::new()
                .route("/v1/responses", post(sequence_upstream_handler))
                .with_state(state),
        )
    }

    #[derive(Clone)]
    struct LifecycleUpstream {
        requests: Arc<AtomicUsize>,
        first_chunk: Arc<Notify>,
    }

    async fn lifecycle_upstream_handler(
        State(state): State<LifecycleUpstream>,
        request: Request,
    ) -> Response {
        let _ = to_bytes(request.into_body(), 1024 * 1024).await;
        state.requests.fetch_add(1, Ordering::AcqRel);
        let first_chunk = Arc::clone(&state.first_chunk);
        let body = stream::unfold((0_usize, first_chunk), |(index, first_chunk)| async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            if index == 0 {
                first_chunk.notify_waiters();
            }
            Some((
                Ok::<_, io::Error>(Bytes::from_static(
                    b"data: {\"type\":\"response.in_progress\"}\n\n",
                )),
                (index.saturating_add(1), first_chunk),
            ))
        });
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/event-stream"),
        );
        response_with_headers(StatusCode::OK, headers, Body::from_stream(body))
    }

    async fn start_lifecycle_upstream(state: LifecycleUpstream) -> ProxyServerHandle {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("lifecycle listener");
        ProxyServerHandle::from_listener(
            listener,
            Router::new()
                .route("/v1/responses", post(lifecycle_upstream_handler))
                .with_state(state),
        )
    }

    #[derive(Clone, Copy)]
    enum CommittedStreamOutcome {
        ReadFailure,
        Pending,
        SemanticOverload,
    }

    #[derive(Clone)]
    struct CommittedStreamUpstream {
        requests: Arc<AtomicUsize>,
        outcome: CommittedStreamOutcome,
    }

    async fn committed_stream_upstream_handler(
        State(state): State<CommittedStreamUpstream>,
        request: Request,
    ) -> Response {
        let _ = to_bytes(request.into_body(), 1024 * 1024).await;
        state.requests.fetch_add(1, Ordering::AcqRel);
        let meaningful = stream::once(async {
            Ok::<_, io::Error>(Bytes::from_static(
                b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"ok\"}\n\n",
            ))
        });
        let tail: BoxByteStream = match state.outcome {
            CommittedStreamOutcome::ReadFailure => Box::pin(stream::once(async {
                tokio::time::sleep(Duration::from_millis(10)).await;
                Err(io::Error::other("fixture read failure"))
            })),
            CommittedStreamOutcome::Pending => Box::pin(stream::pending()),
            CommittedStreamOutcome::SemanticOverload => Box::pin(stream::once(async {
                tokio::time::sleep(Duration::from_millis(10)).await;
                Ok(Bytes::from_static(
                    b"data: {\"type\":\"response.failed\",\"response\":{\"status\":\"failed\",\"error\":{\"codex_error_info\":\"server_overloaded\"}}}\n\n",
                ))
            })),
        };
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/event-stream"),
        );
        response_with_headers(
            StatusCode::OK,
            headers,
            Body::from_stream(meaningful.chain(tail)),
        )
    }

    async fn start_committed_stream_upstream(state: CommittedStreamUpstream) -> ProxyServerHandle {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("committed stream listener");
        ProxyServerHandle::from_listener(
            listener,
            Router::new()
                .route("/v1/responses", post(committed_stream_upstream_handler))
                .with_state(state),
        )
    }

    #[derive(Default)]
    struct HistoryCapture(Mutex<Vec<RequestHistoryRecord>>);

    impl HistorySink for HistoryCapture {
        fn try_record(&self, record: RequestHistoryRecord) -> bool {
            self.0.lock().expect("history mutex").push(record);
            true
        }
    }

    #[derive(Default)]
    struct DecisionHistoryCapture {
        records: Mutex<Vec<RequestHistoryRecord>>,
        stops: Mutex<Vec<FallbackStopRecord>>,
    }

    impl HistorySink for DecisionHistoryCapture {
        fn try_record(&self, record: RequestHistoryRecord) -> bool {
            self.records.lock().expect("history mutex").push(record);
            true
        }

        fn try_record_fallback_stop(&self, record: FallbackStopRecord) -> bool {
            self.stops.lock().expect("fallback stop mutex").push(record);
            true
        }
    }

    struct RejectHistory;

    impl HistorySink for RejectHistory {
        fn try_record(&self, _record: RequestHistoryRecord) -> bool {
            false
        }
    }

    #[derive(Default)]
    struct DiagnosticCapture(Mutex<Vec<RuntimeDiagnosticEvent>>);

    impl RuntimeDiagnosticSink for DiagnosticCapture {
        fn emit(&self, event: RuntimeDiagnosticEvent) {
            self.0.lock().expect("diagnostic mutex").push(event);
        }
    }

    struct NoopInferenceChanges;

    impl InferenceStatusChangeSink for NoopInferenceChanges {
        fn inference_statuses_changed(
            &self,
            _updates: Vec<(RouteId, crate::domain::InferenceStatus)>,
        ) {
        }
    }

    fn test_forwarder(
        config: UpstreamForwarderConfig,
        history: Arc<dyn HistorySink>,
        diagnostics: Arc<dyn RuntimeDiagnosticSink>,
        inference: InferenceStatusService,
    ) -> ResponsesForwarder {
        ResponsesForwarder::with_config(config)
            .expect("forwarder")
            .with_runtime_services(history, diagnostics, inference)
    }

    async fn local_error_code(response: Response) -> String {
        local_error(response).await.error.code
    }

    async fn local_error(response: Response) -> LocalErrorDto {
        let body = to_bytes(response.into_body(), 16 * 1024)
            .await
            .expect("local error body");
        serde_json::from_slice::<LocalErrorDto>(&body).expect("local error DTO")
    }

    #[test]
    fn attempt_indexes_are_wide_and_never_silently_saturate() {
        let wide_index = u32::from(u16::MAX) + 1;
        assert_eq!(
            checked_next_attempt_index(u32::from(u16::MAX)),
            Some(wide_index)
        );
        assert_eq!(checked_next_attempt_index(u32::MAX), None);

        let request = request(false);
        let history = Arc::new(HistoryCapture::default());
        RequestHistoryContext::new(
            &request,
            wide_index,
            history.clone(),
            Arc::new(NoopRuntimeDiagnosticSink),
            None,
            Arc::new(NoopRequestTransitionSink),
        )
        .finish_failure(
            None,
            DeliveryState::None,
            ResponseMetadata::default(),
            classify_transport(TransportFailure::FastConnection),
            RuntimeDiagnosticCode::UpstreamConnectionFailed,
        );

        let records = history.0.lock().expect("history mutex");
        assert_eq!(records[0].attempts[0].attempt_index, wide_index);
    }

    #[test]
    fn header_contract_removes_transport_identity_and_trace_headers() {
        let request = request(false);
        let headers = build_upstream_headers(&request).expect("upstream headers");

        assert_eq!(
            headers.get(header::AUTHORIZATION),
            Some(&HeaderValue::from_static("Bearer upstream-secret"))
        );
        assert!(!headers.contains_key(header::HOST));
        assert!(!headers.contains_key("x-api-key"));
        assert!(!headers.contains_key("x-forwarded-for"));
        assert!(!headers.contains_key("traceparent"));
        assert_eq!(
            headers.get("x-codex-turn-state"),
            Some(&HeaderValue::from_static("sticky"))
        );
        assert_eq!(
            headers.get("chatgpt-account-id"),
            Some(&HeaderValue::from_static("account"))
        );
        assert!(headers.contains_key("x-oai-attestation"));
        assert!(headers.contains_key(header::COOKIE));
        assert!(headers.contains_key("x-future-field"));
        assert_eq!(
            headers.get(header::CONTENT_TYPE),
            Some(&HeaderValue::from_static("application/json"))
        );
    }

    #[test]
    fn header_contract_forces_identity_only_for_streaming_requests() {
        let mut non_stream = request(false);
        non_stream.headers.insert(
            header::ACCEPT_ENCODING,
            HeaderValue::from_static("gzip, br"),
        );
        let non_stream_headers = build_upstream_headers(&non_stream).expect("non-stream headers");
        assert_eq!(
            non_stream_headers.get(header::ACCEPT_ENCODING),
            Some(&HeaderValue::from_static("gzip, br"))
        );

        let stream_headers = build_upstream_headers(&request(true)).expect("stream headers");
        assert_eq!(
            stream_headers.get(header::ACCEPT_ENCODING),
            Some(&HeaderValue::from_static("identity"))
        );
    }

    #[test]
    fn streaming_response_policy_detects_sse_and_removes_content_length() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("Text/Event-Stream; charset=utf-8"),
        );
        headers.insert(header::CONTENT_LENGTH, HeaderValue::from_static("1024"));
        headers.insert("x-upstream-keep", HeaderValue::from_static("kept"));

        assert!(is_event_stream(&headers));
        let filtered = streaming_response_headers(&headers, true);
        assert!(!filtered.contains_key(header::CONTENT_LENGTH));
        assert_eq!(
            filtered.get("x-upstream-keep"),
            Some(&HeaderValue::from_static("kept"))
        );

        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        assert!(!is_event_stream(&headers));
        assert_eq!(
            streaming_response_headers(&headers, false).get(header::CONTENT_LENGTH),
            Some(&HeaderValue::from_static("1024"))
        );
    }

    #[test]
    fn mislabeled_sse_probe_accepts_only_an_exact_first_effective_field() {
        for chunks in [
            vec![b"data:".as_slice()],
            vec![b"\r\n: keepalive\r\n", b"eve", b"nt: response.failed\n"],
            vec![b": comment\n\n", b"data", b": {}\n\n"],
        ] {
            let mut probe = SseProtocolProbe::new(false);
            let mut signal = ProtocolProbeSignal::Pending;
            for chunk in chunks {
                signal = probe.inspect(chunk);
            }
            assert_eq!(signal, ProtocolProbeSignal::Confirmed);
        }

        for source in [
            b"{\"type\":\"error\"}".as_slice(),
            b"text/plain".as_slice(),
            b"id: 1\n".as_slice(),
            b"datax: {}\n".as_slice(),
            b" event: response.failed\n".as_slice(),
        ] {
            let mut probe = SseProtocolProbe::new(false);
            assert_eq!(probe.inspect(source), ProtocolProbeSignal::Rejected);
        }

        let mut incomplete = SseProtocolProbe::new(false);
        assert_eq!(incomplete.inspect(b"dat"), ProtocolProbeSignal::Pending);
    }

    #[test]
    fn terminal_grace_projects_completed_history_without_a_failure() {
        let history = Arc::new(HistoryCapture::default());
        let diagnostics = Arc::new(DiagnosticCapture::default());
        let inference = InferenceStatusService::new(Arc::new(NoopInferenceChanges));
        let request = request(true);
        let route_id = request.route.route_id.clone();
        let context = RequestHistoryContext::new(
            &request,
            0,
            history.clone(),
            diagnostics.clone(),
            Some(inference.clone()),
            Arc::new(NoopRequestTransitionSink),
        );

        finish_stream_attempt(
            context,
            StatusCode::OK,
            SseStreamResult {
                outcome: SseStreamOutcome::TerminalGraceElapsed,
                metadata: ResponseMetadata {
                    status: Some("completed".to_owned()),
                    ..ResponseMetadata::default()
                },
            },
            None,
            None,
        );

        let records = history.0.lock().expect("history mutex");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].completion_state, CompletionState::Completed);
        assert_eq!(records[0].error_category, None);
        assert_eq!(
            records[0].attempts.first().expect("attempt").delivery_state,
            DeliveryState::Completed
        );
        assert!(diagnostics.0.lock().expect("diagnostic mutex").is_empty());
        assert_eq!(
            inference.status(&route_id, now_millis()).kind,
            InferenceStatusKind::RecentSuccess
        );
    }

    #[test]
    fn committed_failures_distinguish_closed_preflight_diagnostic_reasons() {
        let diagnostics = Arc::new(DiagnosticCapture::default());

        for (reason, expected) in [
            (
                PreflightCommitReason::ProtocolMismatch,
                RuntimeDiagnosticCode::UpstreamPreflightProtocolMismatch,
            ),
            (
                PreflightCommitReason::UnknownEvent,
                RuntimeDiagnosticCode::UpstreamPreflightUnknownEvent,
            ),
            (
                PreflightCommitReason::MalformedEvent,
                RuntimeDiagnosticCode::UpstreamPreflightMalformedEvent,
            ),
        ] {
            let request = request(true);
            let context = RequestHistoryContext::new(
                &request,
                0,
                Arc::new(HistoryCapture::default()),
                diagnostics.clone(),
                None,
                Arc::new(NoopRequestTransitionSink),
            );
            finish_stream_attempt(
                context,
                StatusCode::OK,
                SseStreamResult {
                    outcome: SseStreamOutcome::UpstreamReadFailed,
                    metadata: ResponseMetadata::default(),
                },
                None,
                Some(reason),
            );

            assert_eq!(
                diagnostics
                    .0
                    .lock()
                    .expect("diagnostic mutex")
                    .last()
                    .expect("diagnostic event")
                    .code,
                expected
            );
        }
    }

    #[tokio::test]
    async fn header_contract_applies_request_and_response_policy_over_http() {
        let mut state = mock_upstream(
            StatusCode::OK,
            br#"{"id":"response-id","status":"completed"}"#.as_slice(),
        );
        state
            .headers
            .insert("x-upstream-keep", HeaderValue::from_static("kept"));
        state
            .headers
            .insert(header::CONNECTION, HeaderValue::from_static("x-remove-me"));
        state
            .headers
            .insert("x-remove-me", HeaderValue::from_static("removed"));
        let captured = Arc::clone(&state.requests);
        let captured_paths = Arc::clone(&state.request_paths);
        let server = start_mock_upstream(state).await;
        let base_url = format!("http://{}/v1/responses", server.address());
        let forwarder = ResponsesForwarder::new().expect("forwarder");

        let response = forwarder.handle(request_for_base(false, &base_url)).await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get("x-upstream-keep"),
            Some(&HeaderValue::from_static("kept"))
        );
        assert!(!response.headers().contains_key(header::CONNECTION));
        assert!(!response.headers().contains_key("x-remove-me"));
        assert_eq!(
            captured_paths
                .lock()
                .expect("request path capture mutex")
                .as_slice(),
            ["/v1/responses"]
        );
        {
            let requests = captured.lock().expect("request capture mutex");
            let headers = &requests[0];
            assert_eq!(
                headers.get(header::AUTHORIZATION),
                Some(&HeaderValue::from_static("Bearer upstream-secret"))
            );
            assert!(!headers.contains_key("x-api-key"));
            assert!(!headers.contains_key("x-forwarded-for"));
            assert!(!headers.contains_key("traceparent"));
            assert_eq!(
                headers.get("x-codex-turn-state"),
                Some(&HeaderValue::from_static("sticky"))
            );
        }
        server.shutdown().await;
    }

    #[tokio::test]
    async fn upstream_errors_preserve_status_rebuild_headers_and_bound_message() {
        let raw_message = "s".repeat(3_000);
        let mut state = mock_upstream(
            StatusCode::TOO_MANY_REQUESTS,
            serde_json::to_vec(&serde_json::json!({
                "error": { "message": raw_message }
            }))
            .expect("error JSON"),
        );
        state
            .headers
            .insert(header::RETRY_AFTER, HeaderValue::from_static("30"));
        state
            .headers
            .insert("x-upstream-trace", HeaderValue::from_static("trace"));
        let server = start_mock_upstream(state).await;
        let base_url = format!("http://{}/v1", server.address());
        let history = Arc::new(HistoryCapture::default());
        let diagnostics = Arc::new(DiagnosticCapture::default());
        let inference = InferenceStatusService::new(Arc::new(NoopInferenceChanges));
        let forwarder = test_forwarder(
            UpstreamForwarderConfig::default(),
            history.clone(),
            diagnostics.clone(),
            inference,
        );

        let response = forwarder.handle(request_for_base(false, &base_url)).await;

        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(!response.headers().contains_key(header::RETRY_AFTER));
        assert!(!response.headers().contains_key("x-upstream-trace"));
        let body = to_bytes(response.into_body(), 16 * 1024)
            .await
            .expect("normalized error body");
        let error: LocalErrorDto = serde_json::from_slice(&body).expect("local error DTO");
        assert_eq!(error.error.code, "upstream_error");
        assert!(error.error.message.contains("Primary"));
        assert!(error.error.message.contains("gpt-test"));
        assert!(error.error.message.contains("429"));
        assert!(error.error.message.chars().count() <= 1_800);
        {
            let records = history.0.lock().expect("history mutex");
            assert_eq!(records.len(), 1);
            assert_eq!(records[0].http_status, Some(429));
            assert_eq!(
                records[0].error_category.as_deref(),
                Some("upstream_rate_limited")
            );
            assert!(!format!("{records:?}").contains(&raw_message));
        }
        assert_eq!(
            diagnostics.0.lock().expect("diagnostic mutex")[0].code,
            RuntimeDiagnosticCode::UpstreamHttpStatus
        );
        server.shutdown().await;
    }

    #[test]
    fn upstream_error_extraction_accepts_pascal_case_with_deterministic_precedence() {
        let pascal = extract_upstream_error_details(
            br#"{"Code":"AccessDenied","Message":"  Current\nuser\u0000 is\tin debt.  "}"#,
        )
        .expect("PascalCase envelope");
        assert_eq!(pascal.safe_error_code.as_deref(), Some("AccessDenied"));
        assert_eq!(pascal.message.as_deref(), Some("Current user is in debt."));

        let mixed = extract_upstream_error_details(
            br#"{
                "error":{"code":"nested_code","message":"nested message"},
                "code":"lower_code","message":"lower message","detail":"lower detail",
                "Code":"PascalCode","Message":"Pascal message","Detail":"Pascal detail"
            }"#,
        )
        .expect("mixed envelope");
        assert_eq!(mixed.safe_error_code.as_deref(), Some("nested_code"));
        assert_eq!(mixed.message.as_deref(), Some("nested message"));

        let top_level = extract_upstream_error_details(
            br#"{
                "code":"lower_code","message":"lower message",
                "Code":"PascalCode","Message":"Pascal message"
            }"#,
        )
        .expect("top-level envelope");
        assert_eq!(top_level.safe_error_code.as_deref(), Some("lower_code"));
        assert_eq!(top_level.message.as_deref(), Some("lower message"));
        assert!(extract_upstream_error_details(b"not-json").is_none());
    }

    #[tokio::test]
    async fn pascal_case_403_returns_actionable_bounded_error_without_forwarding_headers() {
        let mut state = mock_upstream(
            StatusCode::FORBIDDEN,
            br#"{"Code":"AccessDenied","Message":"Current user is in debt.","RequestId":"provider-request-sentinel"}"#.as_slice(),
        );
        state
            .headers
            .insert("x-fc-error-type", HeaderValue::from_static("FCCommonError"));
        let server = start_mock_upstream(state).await;
        let base_url = format!("http://{}/v1", server.address());
        let history = Arc::new(HistoryCapture::default());
        let diagnostics = Arc::new(DiagnosticCapture::default());
        let inference = InferenceStatusService::new(Arc::new(NoopInferenceChanges));
        let forwarder = test_forwarder(
            UpstreamForwarderConfig::default(),
            history.clone(),
            diagnostics.clone(),
            inference.clone(),
        );
        let request = request_for_base(false, &base_url);
        let route_id = request.route.route_id.clone();

        let response = forwarder.handle(request).await;

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(!response.headers().contains_key("x-fc-error-type"));
        let body = to_bytes(response.into_body(), 16 * 1024)
            .await
            .expect("normalized 403");
        let error: LocalErrorDto = serde_json::from_slice(&body).expect("local error DTO");
        assert_eq!(error.error.code, "upstream_error");
        assert!(error.error.message.contains("returned HTTP 403"));
        assert!(error.error.message.contains("Current user is in debt."));
        assert!(!error.error.message.contains("AccessDenied"));
        assert!(!error.error.message.contains("FCCommonError"));
        assert!(!error.error.message.contains("provider-request-sentinel"));
        {
            let records = history.0.lock().expect("history mutex");
            assert_eq!(records.len(), 1);
            assert_eq!(records[0].http_status, Some(403));
            assert_eq!(
                records[0].error_category.as_deref(),
                Some("upstream_access_denied")
            );
            let persisted = format!("{records:?}");
            for sentinel in [
                "Current user is in debt.",
                "AccessDenied",
                "FCCommonError",
                "provider-request-sentinel",
            ] {
                assert!(!persisted.contains(sentinel));
            }
        }
        assert_eq!(
            inference.status(&route_id, now_millis()).failure_reason,
            Some(InferenceFailureReason::AccessDenied)
        );
        let diagnostics = format!("{:?}", diagnostics.0.lock().expect("diagnostics"));
        for sentinel in [
            "Current user is in debt.",
            "AccessDenied",
            "FCCommonError",
            "provider-request-sentinel",
        ] {
            assert!(!diagnostics.contains(sentinel));
        }
        server.shutdown().await;
    }

    #[tokio::test]
    async fn exact_allowlisted_403_preserves_status_and_account_semantics() {
        let state = mock_upstream(
            StatusCode::FORBIDDEN,
            br#"{"error":{"code":"invalid_api_key","message":"Synthetic invalid key."}}"#
                .as_slice(),
        );
        let server = start_mock_upstream(state).await;
        let base_url = format!("http://{}/v1", server.address());
        let history = Arc::new(HistoryCapture::default());
        let diagnostics = Arc::new(DiagnosticCapture::default());
        let inference = InferenceStatusService::new(Arc::new(NoopInferenceChanges));
        let forwarder = test_forwarder(
            UpstreamForwarderConfig::default(),
            history.clone(),
            diagnostics.clone(),
            inference.clone(),
        );
        let request = request_for_base(false, &base_url);
        let route_id = request.route.route_id.clone();

        let response = forwarder.handle(request).await;

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        {
            let records = history.0.lock().expect("history mutex");
            assert_eq!(records.len(), 1);
            assert_eq!(records[0].http_status, Some(403));
            assert_eq!(
                records[0].error_category.as_deref(),
                Some("invalid_api_key")
            );
        }
        assert_eq!(
            inference.status(&route_id, now_millis()).failure_reason,
            Some(InferenceFailureReason::InvalidKey)
        );
        server.shutdown().await;
    }

    #[test]
    fn upstream_default_deadlines_match_the_transport_and_fallback_contracts() {
        let config = UpstreamForwarderConfig::default();
        assert_eq!(config.connect_timeout, Duration::from_secs(30));
        assert_eq!(config.header_timeout, Duration::from_mins(1));
        assert_eq!(config.non_stream_timeout, Duration::from_mins(10));
        assert_eq!(config.first_output_timeout.as_secs(), 300);
    }

    #[tokio::test]
    async fn upstream_errors_map_connection_and_response_header_timeouts() {
        let unused = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("unused listener");
        let unused_address = unused.local_addr().expect("unused address");
        drop(unused);
        let config = UpstreamForwarderConfig {
            connect_timeout: Duration::from_millis(20),
            header_timeout: Duration::from_millis(20),
            non_stream_timeout: Duration::from_millis(100),
            ..UpstreamForwarderConfig::default()
        };
        let forwarder = ResponsesForwarder::with_config(config.clone()).expect("forwarder");
        let connection = forwarder
            .handle(request_for_base(
                false,
                &format!("http://{unused_address}/v1"),
            ))
            .await;
        assert_eq!(connection.status(), StatusCode::BAD_GATEWAY);
        let connection_error = local_error(connection).await;
        assert_eq!(connection_error.error.code, "upstream_connection_failed");
        assert_eq!(
            connection_error.error.message,
            "The upstream connection could not be established."
        );

        let mut state = mock_upstream(StatusCode::OK, "late headers");
        state.header_delay = Duration::from_millis(60);
        let server = start_mock_upstream(state).await;
        let header_timeout = ResponsesForwarder::with_config(config).expect("forwarder");
        let response = header_timeout
            .handle(request_for_base(
                false,
                &format!("http://{}/v1", server.address()),
            ))
            .await;
        assert_eq!(response.status(), StatusCode::GATEWAY_TIMEOUT);
        assert_eq!(local_error_code(response).await, "upstream_timeout");
        server.shutdown().await;
    }

    #[tokio::test]
    async fn upstream_errors_map_pre_header_disconnect() {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("disconnect listener");
        let address = listener.local_addr().expect("disconnect address");
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accepted connection");
            let mut request = [0_u8; 4_096];
            let bytes_read = stream.read(&mut request).await.expect("request bytes");
            assert!(bytes_read > 0, "upstream should receive the request");
        });
        let history = Arc::new(HistoryCapture::default());
        let diagnostics = Arc::new(DiagnosticCapture::default());
        let inference = InferenceStatusService::new(Arc::new(NoopInferenceChanges));
        let routed_request = request_for_base(false, &format!("http://{address}/v1"));
        let route_id = routed_request.route.route_id.clone();
        let forwarder = test_forwarder(
            UpstreamForwarderConfig::default(),
            history.clone(),
            diagnostics.clone(),
            inference.clone(),
        );

        let response = forwarder.handle(routed_request).await;
        server.await.expect("disconnect server task");

        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        let error = local_error(response).await;
        assert_eq!(error.error.code, "upstream_request_failed");
        assert_eq!(
            error.error.message,
            "The upstream request was interrupted before response headers were received."
        );
        {
            let records = history.0.lock().expect("history mutex");
            assert_eq!(records.len(), 1);
            assert_eq!(records[0].completion_state, CompletionState::Failed);
            assert_eq!(records[0].http_status, None);
            assert_eq!(
                records[0].error_category.as_deref(),
                Some("upstream_request_failed")
            );
            let attempt = records[0].attempts.first().expect("attempt history");
            assert_eq!(attempt.http_status, None);
            assert_eq!(
                attempt.error_category.as_deref(),
                Some("upstream_request_failed")
            );
            assert_eq!(attempt.delivery_state, DeliveryState::None);
        }
        {
            let events = diagnostics.0.lock().expect("diagnostic mutex");
            assert_eq!(events.len(), 1);
            assert_eq!(events[0].code, RuntimeDiagnosticCode::UpstreamRequestFailed);
            assert_eq!(events[0].code.as_str(), "upstream_request_failed");
            assert_eq!(events[0].http_status, None);
        }
        assert_eq!(
            inference.status(&route_id, now_millis()).kind,
            InferenceStatusKind::RecentFailure
        );
    }

    #[tokio::test]
    async fn upstream_errors_map_non_stream_deadline_and_size_limit() {
        let mut delayed = mock_upstream(StatusCode::OK, "late body");
        delayed.body_delay = Duration::from_millis(60);
        let delayed_server = start_mock_upstream(delayed).await;
        let timeout_forwarder = ResponsesForwarder::with_config(UpstreamForwarderConfig {
            header_timeout: Duration::from_millis(100),
            non_stream_timeout: Duration::from_millis(20),
            ..UpstreamForwarderConfig::default()
        })
        .expect("forwarder");
        let timeout = timeout_forwarder
            .handle(request_for_base(
                false,
                &format!("http://{}/v1", delayed_server.address()),
            ))
            .await;
        assert_eq!(timeout.status(), StatusCode::GATEWAY_TIMEOUT);
        assert_eq!(local_error_code(timeout).await, "upstream_timeout");
        delayed_server.shutdown().await;

        let large_server = start_mock_upstream(mock_upstream(StatusCode::OK, "12345")).await;
        let bounded_forwarder = ResponsesForwarder::with_config(UpstreamForwarderConfig {
            response_wire_limit: 4,
            response_decoded_limit: 4,
            ..UpstreamForwarderConfig::default()
        })
        .expect("forwarder");
        let too_large = bounded_forwarder
            .handle(request_for_base(
                false,
                &format!("http://{}/v1", large_server.address()),
            ))
            .await;
        assert_eq!(too_large.status(), StatusCode::BAD_GATEWAY);
        assert_eq!(
            local_error_code(too_large).await,
            "upstream_response_too_large"
        );
        large_server.shutdown().await;
    }

    #[tokio::test]
    async fn sse_passthrough_has_no_non_stream_deadline_and_updates_on_completion() {
        let sse = "data: {\"type\":\"response.output_text.delta\",\"delta\":\"ok\"}\n\n\
                   data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\n\n";
        let mut state = mock_upstream(StatusCode::OK, sse);
        state.body_delay = Duration::from_millis(40);
        state.headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/event-stream"),
        );
        let server = start_mock_upstream(state).await;
        let history = Arc::new(HistoryCapture::default());
        let diagnostics = Arc::new(DiagnosticCapture::default());
        let inference = InferenceStatusService::new(Arc::new(NoopInferenceChanges));
        let forwarder = test_forwarder(
            UpstreamForwarderConfig {
                header_timeout: Duration::from_millis(100),
                non_stream_timeout: Duration::from_millis(10),
                ..UpstreamForwarderConfig::default()
            },
            history.clone(),
            diagnostics.clone(),
            inference.clone(),
        );
        let routed_request = request_for_base(true, &format!("http://{}/v1", server.address()));
        let route_id = routed_request.route.route_id.clone();

        let response = forwarder.handle(routed_request).await;
        let downstream = to_bytes(response.into_body(), sse.len() + 1)
            .await
            .expect("SSE response");

        assert_eq!(downstream.as_ref(), sse.as_bytes());
        assert_eq!(history.0.lock().expect("history mutex").len(), 1);
        assert_eq!(
            inference.status(&route_id, now_millis()).kind,
            InferenceStatusKind::RecentSuccess
        );
        assert!(diagnostics.0.lock().expect("diagnostic mutex").is_empty());
        server.shutdown().await;
    }

    #[tokio::test]
    async fn immediate_sse_latency_uses_supplied_attempt_clock() {
        let sse = b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"ok\"}\n\n\
                    data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\n\n";
        let history = Arc::new(HistoryCapture::default());
        let request = request(true);
        let context = RequestHistoryContext::new(
            &request,
            0,
            history.clone(),
            Arc::new(DiagnosticCapture::default()),
            None,
            Arc::new(NoopRequestTransitionSink),
        );
        let upstream: reqwest::Response = axum::http::Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/event-stream")
            .body(reqwest::Body::from(Bytes::from_static(sse)))
            .expect("upstream response")
            .into();
        let started = Instant::now()
            .checked_sub(Duration::from_millis(50))
            .expect("test latency is representable");

        let response = streaming_response(upstream, started, context);
        let downstream = to_bytes(response.into_body(), sse.len() + 1)
            .await
            .expect("SSE response");

        assert_eq!(downstream.as_ref(), sse);
        let records = history.0.lock().expect("history mutex");
        let first_output_latency_ms = records[0]
            .first_output_latency_ms
            .expect("first output latency");
        assert!(
            first_output_latency_ms >= 50,
            "supplied request clock should be retained, got {first_output_latency_ms} ms"
        );
    }

    #[tokio::test]
    async fn fallback_sse_latency_keeps_the_original_request_clock() {
        let a_state = mock_upstream(
            StatusCode::TOO_MANY_REQUESTS,
            br#"{"error":{"code":"rate_limit"}}"#.as_slice(),
        );
        let a_server = start_mock_upstream(a_state).await;
        let b_body = concat!(
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"ok\"}\n\n",
            "data: [DONE]\n\n"
        );
        let mut b_state = mock_upstream(StatusCode::OK, b_body);
        b_state.headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/event-stream"),
        );
        let b_server = start_mock_upstream(b_state).await;
        let a = route_snapshot("A", &format!("http://{}/v1", a_server.address()));
        let b = route_snapshot("B", &format!("http://{}/v1", b_server.address()));
        let (mut request, routing) = fallback_request(true, vec![a, b]);
        request.request_started = Instant::now();
        let history = Arc::new(HistoryCapture::default());
        let activator = Arc::new(InMemoryFallbackActivator::new(routing.clone()));
        let forwarder = test_forwarder(
            UpstreamForwarderConfig::default(),
            history.clone(),
            Arc::new(DiagnosticCapture::default()),
            InferenceStatusService::new(Arc::new(NoopInferenceChanges)),
        )
        .with_fallback_services(routing, activator);

        tokio::time::sleep(Duration::from_millis(35)).await;
        let response = forwarder.handle(request).await;
        let downstream = to_bytes(response.into_body(), b_body.len() + 1)
            .await
            .expect("fallback SSE body");

        assert_eq!(downstream.as_ref(), b_body.as_bytes());
        let latency = {
            let records = history.0.lock().expect("history mutex");
            records
                .iter()
                .rev()
                .find_map(|record| record.first_output_latency_ms)
                .expect("committed first output latency")
        };
        assert!(
            latency >= 35,
            "fallback must include pre-attempt request time, got {latency} ms"
        );
        a_server.shutdown().await;
        b_server.shutdown().await;
    }

    #[tokio::test]
    async fn sse_passthrough_cancellation_is_history_only_and_inference_neutral() {
        let mut state = mock_upstream(StatusCode::OK, "data: [DONE]\n\n");
        state.body_delay = Duration::from_millis(200);
        let server = start_mock_upstream(state).await;
        let history = Arc::new(HistoryCapture::default());
        let inference = InferenceStatusService::new(Arc::new(NoopInferenceChanges));
        let routed_request = request_for_base(true, &format!("http://{}/v1", server.address()));
        let route_id = routed_request.route.route_id.clone();
        let forwarder = test_forwarder(
            UpstreamForwarderConfig::default(),
            history.clone(),
            Arc::new(DiagnosticCapture::default()),
            inference.clone(),
        );

        let response = forwarder.handle(routed_request).await;
        drop(response);

        {
            let records = history.0.lock().expect("history mutex");
            assert_eq!(records.len(), 1);
            assert_eq!(records[0].completion_state, CompletionState::Cancelled);
        }
        assert_eq!(
            inference.status(&route_id, now_millis()).kind,
            InferenceStatusKind::Unverified
        );
        server.shutdown().await;
    }

    #[tokio::test]
    async fn request_history_failure_never_changes_delivered_http_or_sse() {
        let http_body = br#"{"id":"resp","status":"completed"}"#;
        let http_server =
            start_mock_upstream(mock_upstream(StatusCode::OK, http_body.as_slice())).await;
        let inference = InferenceStatusService::new(Arc::new(NoopInferenceChanges));
        let forwarder = test_forwarder(
            UpstreamForwarderConfig::default(),
            Arc::new(RejectHistory),
            Arc::new(DiagnosticCapture::default()),
            inference.clone(),
        );
        let http = forwarder
            .handle(request_for_base(
                false,
                &format!("http://{}/v1", http_server.address()),
            ))
            .await;
        assert_eq!(
            to_bytes(http.into_body(), 1024).await.expect("HTTP body"),
            Bytes::from_static(http_body)
        );
        http_server.shutdown().await;

        let sse_body = b"data: [DONE]\n\n";
        let sse_server =
            start_mock_upstream(mock_upstream(StatusCode::OK, sse_body.as_slice())).await;
        let sse = forwarder
            .handle(request_for_base(
                true,
                &format!("http://{}/v1", sse_server.address()),
            ))
            .await;
        assert_eq!(
            to_bytes(sse.into_body(), 1024).await.expect("SSE body"),
            Bytes::from_static(sse_body)
        );
        sse_server.shutdown().await;
    }

    #[test]
    fn non_streaming_decoders_support_aliases_stacks_and_raw_deflate() {
        let original = br#"{"id":"response-id","status":"completed"}"#;
        let gzip = {
            use std::io::Write;
            let mut encoder =
                flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
            encoder.write_all(original).expect("gzip write");
            encoder.finish().expect("gzip finish")
        };
        assert_eq!(
            decode_supported(gzip.clone(), &["x-gzip".to_owned()], 1024).expect("x-gzip decode"),
            original
        );

        let raw_deflate = {
            use std::io::Write;
            let mut encoder =
                flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
            encoder.write_all(original).expect("deflate write");
            encoder.finish().expect("deflate finish")
        };
        assert_eq!(
            decode_supported(raw_deflate, &["deflate".to_owned()], 1024)
                .expect("raw deflate decode"),
            original
        );

        let zstd_after_gzip = zstd::stream::encode_all(gzip.as_slice(), 0).expect("zstd encode");
        assert_eq!(
            decode_supported(
                zstd_after_gzip,
                &["gzip".to_owned(), "zstd".to_owned()],
                1024,
            )
            .expect("stacked decode"),
            original
        );

        let zlib = {
            use std::io::Write;
            let mut encoder =
                flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
            encoder.write_all(original).expect("zlib write");
            encoder.finish().expect("zlib finish")
        };
        assert_eq!(
            decode_supported(zlib, &["deflate".to_owned()], 1024).expect("zlib decode"),
            original
        );

        let brotli = {
            use std::io::Write;
            let mut encoded = Vec::new();
            {
                let mut compressor = brotli::CompressorWriter::new(&mut encoded, 4096, 5, 22);
                compressor.write_all(original).expect("brotli write");
            }
            encoded
        };
        assert_eq!(
            decode_supported(brotli, &["br".to_owned()], 1024).expect("brotli decode"),
            original
        );

        let zstd = zstd::stream::encode_all(original.as_slice(), 0).expect("zstd encode");
        assert_eq!(
            decode_supported(zstd, &["zst".to_owned()], 1024).expect("zst decode"),
            original
        );
        assert!(matches!(
            decode_supported(original.to_vec(), &["compress".to_owned()], 1024),
            Err(DecodeError::Unsupported)
        ));
        assert!(matches!(
            decode_supported(gzip, &["gzip".to_owned()], original.len() - 1),
            Err(DecodeError::TooLarge)
        ));
    }

    #[test]
    fn exact_capacity_decoder_preserves_supported_encoding_and_limit_semantics() {
        use std::io::Write;

        let original = br#"{"data":[{"b64_json":"AQ=="}]}"#;
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(original).expect("gzip write");
        let gzip = encoder.finish().expect("gzip finish");

        assert_eq!(
            decode_supported_exact(gzip.clone(), &["gzip".to_owned()], 1024)
                .expect("exact-capacity gzip decode"),
            original
        );
        assert!(matches!(
            decode_supported_exact(gzip, &["gzip".to_owned()], original.len() - 1),
            Err(DecodeError::TooLarge)
        ));
        assert!(matches!(
            decode_supported_exact(original.to_vec(), &["compress".to_owned()], 1024),
            Err(DecodeError::Unsupported)
        ));
    }

    #[test]
    fn non_streaming_metadata_is_narrow_and_parse_failures_are_neutral() {
        let metadata = project_non_streaming_metadata(
            br#"{"id":"resp_1","model":"gpt-test","service_tier":"default","status":"completed","usage":{"input_tokens":3,"output_tokens":5,"total_tokens":8,"input_tokens_details":{"cached_tokens":2,"cache_write_tokens":1}},"output":[{"content":"must-not-project"}]}"#,
        );
        assert_eq!(metadata.response_id.as_deref(), Some("resp_1"));
        assert_eq!(metadata.model.as_deref(), Some("gpt-test"));
        assert_eq!(metadata.status.as_deref(), Some("completed"));
        assert_eq!(metadata.input_tokens, Some(3));
        assert_eq!(metadata.output_tokens, Some(5));
        assert_eq!(metadata.total_tokens, Some(8));
        assert_eq!(metadata.service_tier.as_deref(), Some("default"));
        assert_eq!(metadata.cached_input_tokens, Some(2));
        assert_eq!(metadata.cache_write_input_tokens, Some(1));
        assert!(metadata.complete);

        let invalid = project_non_streaming_metadata(b"not-json");
        assert!(!invalid.complete);
        assert_eq!(invalid.response_id, None);
    }

    #[tokio::test]
    async fn non_streaming_decodes_supported_and_forwards_unsupported_encoding() {
        use std::io::Write;

        let original = br#"{"id":"resp","model":"actual","status":"completed","usage":{"input_tokens":1,"output_tokens":2,"total_tokens":3}}"#;
        let compressed = {
            let mut encoder =
                flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
            encoder.write_all(original).expect("gzip write");
            encoder.finish().expect("gzip finish")
        };
        let mut gzip_state = mock_upstream(StatusCode::CREATED, compressed.clone());
        gzip_state
            .headers
            .insert(header::CONTENT_ENCODING, HeaderValue::from_static("x-gzip"));
        gzip_state.headers.insert(
            header::CONTENT_LENGTH,
            HeaderValue::from_str(&compressed.len().to_string()).expect("content length"),
        );
        let gzip_server = start_mock_upstream(gzip_state).await;
        let history = Arc::new(HistoryCapture::default());
        let forwarder = test_forwarder(
            UpstreamForwarderConfig::default(),
            history.clone(),
            Arc::new(DiagnosticCapture::default()),
            InferenceStatusService::new(Arc::new(NoopInferenceChanges)),
        );

        let decoded = forwarder
            .handle(request_for_base(
                false,
                &format!("http://{}/v1", gzip_server.address()),
            ))
            .await;
        assert_eq!(decoded.status(), StatusCode::CREATED);
        assert!(!decoded.headers().contains_key(header::CONTENT_ENCODING));
        assert!(!decoded.headers().contains_key(header::CONTENT_LENGTH));
        assert_eq!(
            to_bytes(decoded.into_body(), 1024)
                .await
                .expect("decoded body"),
            Bytes::from_static(original)
        );
        gzip_server.shutdown().await;
        {
            let records = history.0.lock().expect("history mutex");
            assert_eq!(records[0].actual_model.as_deref(), Some("actual"));
            assert_eq!(records[0].reasoning_effort.as_deref(), Some("high"));
            assert_eq!(records[0].total_tokens, Some(3));
            assert!(records[0].metadata_complete);
        }

        let opaque = Bytes::from_static(b"opaque-wire-body");
        let mut unsupported_state = mock_upstream(StatusCode::OK, opaque.clone());
        unsupported_state.headers.insert(
            header::CONTENT_ENCODING,
            HeaderValue::from_static("compress"),
        );
        let unsupported_server = start_mock_upstream(unsupported_state).await;
        let unsupported = forwarder
            .handle(request_for_base(
                false,
                &format!("http://{}/v1", unsupported_server.address()),
            ))
            .await;
        assert_eq!(
            unsupported.headers().get(header::CONTENT_ENCODING),
            Some(&HeaderValue::from_static("compress"))
        );
        assert_eq!(
            to_bytes(unsupported.into_body(), 1024)
                .await
                .expect("opaque body"),
            opaque
        );
        unsupported_server.shutdown().await;
        assert!(!history.0.lock().expect("history mutex")[1].metadata_complete);
    }

    #[tokio::test]
    async fn fallback_contacts_each_participant_once_for_persistent_5xx() {
        let mut servers = Vec::new();
        let mut routes = Vec::new();
        let mut request_counts = Vec::new();
        for (name, status) in [
            ("A", StatusCode::INTERNAL_SERVER_ERROR),
            ("B", StatusCode::INTERNAL_SERVER_ERROR),
            ("C", StatusCode::INTERNAL_SERVER_ERROR),
            ("D", StatusCode::INTERNAL_SERVER_ERROR),
            ("E", StatusCode::INTERNAL_SERVER_ERROR),
        ] {
            let state = mock_upstream(status, br#"{"error":{"code":"server_error"}}"#.as_slice());
            request_counts.push(Arc::clone(&state.requests));
            let server = start_mock_upstream(state).await;
            routes.push(route_snapshot(
                name,
                &format!("http://{}/v1", server.address()),
            ));
            servers.push(server);
        }
        let participant_count = routes.len();
        let (request, routing) = fallback_request(false, routes.clone());
        let activator = Arc::new(InMemoryFallbackActivator::new(routing.clone()));
        let forwarder = ResponsesForwarder::new()
            .expect("forwarder")
            .with_fallback_services(routing.clone(), activator.clone());

        let response = forwarder.handle(request).await;

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            request_counts
                .iter()
                .map(|requests| requests.lock().expect("route requests").len())
                .collect::<Vec<_>>(),
            vec![1; participant_count]
        );
        let expected_activations = routes
            .windows(2)
            .map(|pair| (pair[0].route_id.clone(), pair[1].route_id.clone()))
            .collect::<Vec<_>>();
        assert_eq!(
            activator
                .activations
                .lock()
                .expect("activations")
                .as_slice(),
            expected_activations
        );
        assert_eq!(
            routing.load().active.as_ref().map(|route| &route.route_id),
            routes.last().map(|route| &route.route_id)
        );

        assert_eq!(
            request_counts
                .iter()
                .map(|requests| requests.lock().expect("route requests").len())
                .sum::<usize>(),
            participant_count
        );
        for server in servers {
            server.shutdown().await;
        }
    }

    #[tokio::test]
    async fn mixed_policy_fallback_selects_body_and_forwarded_tier_per_attempt() {
        let a_state = mock_upstream(
            StatusCode::INTERNAL_SERVER_ERROR,
            br#"{"error":{"code":"server_error"}}"#.as_slice(),
        );
        let a_bodies = Arc::clone(&a_state.request_bodies);
        let a_server = start_mock_upstream(a_state).await;
        let b_state = mock_upstream(
            StatusCode::OK,
            br#"{"status":"completed","model":"gpt-test","service_tier":"default"}"#.as_slice(),
        );
        let b_bodies = Arc::clone(&b_state.request_bodies);
        let b_server = start_mock_upstream(b_state).await;
        let mut a = route_snapshot("A", &format!("http://{}/v1", a_server.address()));
        Arc::get_mut(&mut a)
            .expect("unshared A route")
            .service_tier_policy = ServiceTierPolicy::Omit;
        let b = route_snapshot("B", &format!("http://{}/v1", b_server.address()));
        let (mut request, routing) = fallback_request(false, vec![a, b]);
        let original = Bytes::from_static(
            br#"{ "model":"gpt-test", "service_tier":"priority", "nested":{"service_tier":"keep"}, "values":[true,null,7] }"#,
        );
        let mut omitted_value: serde_json::Value =
            serde_json::from_slice(&original).expect("request JSON");
        omitted_value
            .as_object_mut()
            .expect("request object")
            .remove("service_tier");
        request.body = original.clone();
        request.body_without_service_tier = Some(Bytes::from(
            serde_json::to_vec(&omitted_value).expect("omitted request JSON"),
        ));
        request.service_tier = Some("priority".to_owned());
        let history = Arc::new(HistoryCapture::default());
        let activator = Arc::new(InMemoryFallbackActivator::new(routing.clone()));
        let forwarder = ResponsesForwarder::new()
            .expect("forwarder")
            .with_runtime_services(
                history.clone(),
                Arc::new(DiagnosticCapture::default()),
                InferenceStatusService::new(Arc::new(NoopInferenceChanges)),
            )
            .with_fallback_services(routing, activator);

        let response = forwarder.handle(request).await;

        assert_eq!(response.status(), StatusCode::OK);
        {
            let a_bodies = a_bodies.lock().expect("A bodies");
            assert_eq!(a_bodies.len(), 1);
            let value: serde_json::Value =
                serde_json::from_slice(&a_bodies[0]).expect("A request JSON");
            assert!(value.get("service_tier").is_none());
            assert_eq!(value["nested"]["service_tier"], "keep");
            assert_eq!(value["values"], serde_json::json!([true, null, 7]));
        }
        {
            let b_bodies = b_bodies.lock().expect("B bodies");
            assert_eq!(b_bodies.as_slice(), [original]);
        }
        {
            let records = history.0.lock().expect("history");
            assert_eq!(records.len(), 2);
            assert!(
                records
                    .iter()
                    .all(|record| { record.requested_service_tier.as_deref() == Some("priority") })
            );
            assert!(
                records[..1]
                    .iter()
                    .all(|record| { record.attempts[0].forwarded_service_tier.is_none() })
            );
            assert_eq!(
                records[1].attempts[0].forwarded_service_tier.as_deref(),
                Some("priority")
            );
        }
        a_server.shutdown().await;
        b_server.shutdown().await;
    }

    #[tokio::test]
    async fn fallback_forwards_429_once_across_five_participants() {
        let mut servers = Vec::new();
        let mut routes = Vec::new();
        let mut request_counts = Vec::new();
        for name in ["A", "B", "C", "D", "E"] {
            let state = mock_upstream(
                StatusCode::TOO_MANY_REQUESTS,
                br#"{"error":{"code":"rate_limit_exceeded"}}"#.as_slice(),
            );
            request_counts.push(Arc::clone(&state.requests));
            let server = start_mock_upstream(state).await;
            routes.push(route_snapshot(
                name,
                &format!("http://{}/v1", server.address()),
            ));
            servers.push(server);
        }
        let participant_count = routes.len();
        let (request, routing) = fallback_request(false, routes.clone());
        let activator = Arc::new(InMemoryFallbackActivator::new(routing.clone()));
        let forwarder = ResponsesForwarder::new()
            .expect("forwarder")
            .with_fallback_services(routing.clone(), activator.clone());

        let response = forwarder.handle(request).await;

        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        let counts = request_counts
            .iter()
            .map(|requests| requests.lock().expect("route requests").len())
            .collect::<Vec<_>>();
        assert_eq!(counts, vec![1; participant_count]);
        assert_eq!(counts.iter().sum::<usize>(), participant_count);
        assert_eq!(
            activator.activations.lock().expect("activations").len(),
            participant_count - 1
        );
        assert_eq!(
            routing.load().active.as_ref().map(|route| &route.route_id),
            routes.last().map(|route| &route.route_id)
        );
        for server in servers {
            server.shutdown().await;
        }
    }

    #[tokio::test]
    async fn fallback_moves_only_forward_from_each_active_participant() {
        let mut servers = Vec::new();
        let mut routes = Vec::new();
        let mut request_counts = Vec::new();
        for name in ["A", "B", "C"] {
            let state = mock_upstream(
                StatusCode::SERVICE_UNAVAILABLE,
                br#"{"error":{"code":"server_error"}}"#.as_slice(),
            );
            request_counts.push(Arc::clone(&state.requests));
            let server = start_mock_upstream(state).await;
            routes.push(route_snapshot(
                name,
                &format!("http://{}/v1", server.address()),
            ));
            servers.push(server);
        }
        for (active_index, expected_counts, expected_activations) in [
            (0, vec![1, 1, 1], 2),
            (1, vec![0, 1, 1], 1),
            (2, vec![0, 0, 1], 0),
        ] {
            for requests in &request_counts {
                requests.lock().expect("route requests").clear();
            }
            let (request, routing) =
                fallback_request_from_index(false, routes.clone(), active_index);
            let activator = Arc::new(InMemoryFallbackActivator::new(routing.clone()));
            let forwarder = ResponsesForwarder::new()
                .expect("forwarder")
                .with_fallback_services(routing.clone(), activator.clone());

            let response = forwarder.handle(request).await;

            assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(
                request_counts
                    .iter()
                    .map(|requests| requests.lock().expect("route requests").len())
                    .collect::<Vec<_>>(),
                expected_counts
            );
            assert_eq!(
                activator.activations.lock().expect("activations").len(),
                expected_activations
            );
            assert_eq!(
                routing.load().active.as_ref().map(|route| &route.route_id),
                Some(&routes[2].route_id)
            );
        }
        for server in servers {
            server.shutdown().await;
        }
    }

    #[tokio::test]
    async fn fallback_does_not_activate_from_an_active_route_outside_the_prefix() {
        let outside_state = mock_upstream(
            StatusCode::SERVICE_UNAVAILABLE,
            br#"{"error":{"code":"server_error"}}"#.as_slice(),
        );
        let outside_requests = Arc::clone(&outside_state.requests);
        let outside_server = start_mock_upstream(outside_state).await;
        let a_state = mock_upstream(StatusCode::OK, br#"{"status":"completed"}"#.as_slice());
        let a_requests = Arc::clone(&a_state.requests);
        let a_server = start_mock_upstream(a_state).await;
        let b_state = mock_upstream(StatusCode::OK, br#"{"status":"completed"}"#.as_slice());
        let b_requests = Arc::clone(&b_state.requests);
        let b_server = start_mock_upstream(b_state).await;
        let outside = route_snapshot(
            "Outside",
            &format!("http://{}/v1", outside_server.address()),
        );
        let a = route_snapshot("A", &format!("http://{}/v1", a_server.address()));
        let b = route_snapshot("B", &format!("http://{}/v1", b_server.address()));
        let routing = RoutingSnapshotStore::new(RoutingSnapshot {
            active: Some(Arc::clone(&outside)),
            participants: vec![a, b],
            enabled: true,
            selection_generation: 7,
            config_revision: 11,
            images_generation_enabled: false,
            images_route: None,
            images_generation_timeout: Duration::from_mins(10),
        });
        let mut request = request_for_base(false, outside.base_url.as_str());
        request.route = Arc::clone(&outside);
        request.routing = routing.load();
        let activator = Arc::new(InMemoryFallbackActivator::new(routing.clone()));
        let forwarder = ResponsesForwarder::new()
            .expect("forwarder")
            .with_fallback_services(routing.clone(), activator.clone());

        let response = forwarder.handle(request).await;

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(outside_requests.lock().expect("outside requests").len(), 1);
        assert!(a_requests.lock().expect("A requests").is_empty());
        assert!(b_requests.lock().expect("B requests").is_empty());
        assert!(
            activator
                .activations
                .lock()
                .expect("activations")
                .is_empty()
        );
        assert_eq!(
            routing.load().active.as_ref().map(|route| &route.route_id),
            Some(&outside.route_id)
        );
        outside_server.shutdown().await;
        a_server.shutdown().await;
        b_server.shutdown().await;
    }

    #[tokio::test]
    async fn fallback_scales_header_timeouts_across_five_participants() {
        let mut servers = Vec::new();
        let mut routes = Vec::new();
        let mut request_counts = Vec::new();
        for name in ["A", "B", "C", "D", "E"] {
            let mut state = mock_upstream(StatusCode::OK, br#"{"status":"completed"}"#.as_slice());
            state.header_delay = Duration::from_millis(100);
            request_counts.push(Arc::clone(&state.requests));
            let server = start_mock_upstream(state).await;
            routes.push(route_snapshot(
                name,
                &format!("http://{}/v1", server.address()),
            ));
            servers.push(server);
        }
        let participant_count = routes.len();
        let (request, routing) = fallback_request(false, routes.clone());
        let activator = Arc::new(InMemoryFallbackActivator::new(routing.clone()));
        let config = UpstreamForwarderConfig {
            header_timeout: Duration::from_millis(20),
            ..UpstreamForwarderConfig::default()
        };
        let forwarder = ResponsesForwarder::with_config(config)
            .expect("forwarder")
            .with_fallback_services(routing.clone(), activator.clone());

        let response = forwarder.handle(request).await;

        assert_eq!(response.status(), StatusCode::GATEWAY_TIMEOUT);
        assert_eq!(
            request_counts
                .iter()
                .map(|requests| requests.lock().expect("route requests").len())
                .collect::<Vec<_>>(),
            vec![1, 1, 1, 1, 1]
        );
        assert_eq!(
            activator.activations.lock().expect("activations").len(),
            participant_count - 1
        );
        assert_eq!(
            routing.load().active.as_ref().map(|route| &route.route_id),
            routes.last().map(|route| &route.route_id)
        );

        for server in servers {
            server.shutdown().await;
        }
    }

    #[tokio::test]
    async fn fallback_forwards_only_exact_allowlisted_account_errors() {
        for (status, code) in [
            (StatusCode::UNAUTHORIZED, "invalid_api_key"),
            (StatusCode::PAYMENT_REQUIRED, "insufficient_quota"),
        ] {
            let body = serde_json::to_vec(&serde_json::json!({
                "error": { "code": code }
            }))
            .expect("account error JSON");
            let a_state = mock_upstream(status, body.clone());
            let a_requests = Arc::clone(&a_state.requests);
            let a_server = start_mock_upstream(a_state).await;
            let b_state = mock_upstream(status, body);
            let b_requests = Arc::clone(&b_state.requests);
            let b_server = start_mock_upstream(b_state).await;
            let a = route_snapshot("A", &format!("http://{}/v1", a_server.address()));
            let b = route_snapshot("B", &format!("http://{}/v1", b_server.address()));
            let (request, routing) = fallback_request(false, vec![a, Arc::clone(&b)]);
            let activator = Arc::new(InMemoryFallbackActivator::new(routing.clone()));
            let forwarder = ResponsesForwarder::new()
                .expect("forwarder")
                .with_fallback_services(routing.clone(), activator);

            let response = forwarder.handle(request).await;

            assert_eq!(response.status(), status, "account code {code}");
            assert_eq!(a_requests.lock().expect("A requests").len(), 1);
            assert_eq!(b_requests.lock().expect("B requests").len(), 1);
            assert_eq!(
                routing.load().active.as_ref().map(|route| &route.route_id),
                Some(&b.route_id)
            );
            a_server.shutdown().await;
            b_server.shutdown().await;
        }
    }

    #[tokio::test]
    async fn fallback_forwards_exact_structured_overload_once_to_the_next_route() {
        const PROVIDER_MESSAGE: &str = "SYNTHETIC_PROVIDER_CAPACITY_MESSAGE";
        let overload_events = [
            "data: {\"type\":\"response.failed\",\"response\":{\"status\":\"failed\",\"error\":{\"code\":\"server_overloaded\",\"message\":\"SYNTHETIC_PROVIDER_CAPACITY_MESSAGE\"}}}\n\n",
            "data: {\"type\":\"response.failed\",\"response\":{\"status\":\"failed\",\"error\":{\"code\":\"server_error\",\"codex_error_info\":\"server_overloaded\",\"message\":\"SYNTHETIC_PROVIDER_CAPACITY_MESSAGE\"}}}\n\n",
            "data: {\"type\":\"response.failed\",\"response\":{\"status\":\"failed\",\"error\":{\"code\":\"server_error\",\"message\":\"Selected model is at capacity. Please try a different model.\"}}}\n\n",
        ];
        let success_event =
            "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\n\n";

        for overload_event in overload_events {
            let mut a_state = mock_upstream(StatusCode::OK, overload_event);
            a_state.headers.insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/event-stream"),
            );
            let a_requests = Arc::clone(&a_state.requests);
            let a_server = start_mock_upstream(a_state).await;
            let mut b_state = mock_upstream(StatusCode::OK, success_event);
            b_state.headers.insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/event-stream"),
            );
            let b_requests = Arc::clone(&b_state.requests);
            let b_server = start_mock_upstream(b_state).await;
            let a = route_snapshot("A", &format!("http://{}/v1", a_server.address()));
            let b = route_snapshot("B", &format!("http://{}/v1", b_server.address()));
            let (request, routing) = fallback_request(true, vec![Arc::clone(&a), Arc::clone(&b)]);
            let history = Arc::new(HistoryCapture::default());
            let diagnostics = Arc::new(DiagnosticCapture::default());
            let inference = InferenceStatusService::new(Arc::new(NoopInferenceChanges));
            let activator = Arc::new(InMemoryFallbackActivator::new(routing.clone()));
            let forwarder = test_forwarder(
                UpstreamForwarderConfig::default(),
                history.clone(),
                diagnostics.clone(),
                inference.clone(),
            )
            .with_fallback_services(routing.clone(), activator.clone());

            let response = forwarder.handle(request).await;
            let downstream = to_bytes(response.into_body(), success_event.len() + 1)
                .await
                .expect("next route SSE body");

            assert_eq!(downstream.as_ref(), success_event.as_bytes());
            assert_eq!(a_requests.lock().expect("A requests").len(), 1);
            assert_eq!(b_requests.lock().expect("B requests").len(), 1);
            assert_eq!(activator.activations.lock().expect("activations").len(), 1);
            assert_eq!(
                routing.load().active.as_ref().map(|route| &route.route_id),
                Some(&b.route_id)
            );
            assert_eq!(
                inference.status(&a.route_id, now_millis()).failure_reason,
                Some(InferenceFailureReason::Service)
            );
            {
                let records = history.0.lock().expect("history");
                assert_eq!(
                    records[0].error_category.as_deref(),
                    Some("upstream_overloaded")
                );
                assert!(!format!("{records:?}").contains(PROVIDER_MESSAGE));
                assert!(!format!("{records:?}").contains("Selected model is at capacity"));
            }
            assert!(
                !format!("{:?}", diagnostics.0.lock().expect("diagnostics"))
                    .contains(PROVIDER_MESSAGE)
            );

            a_server.shutdown().await;
            b_server.shutdown().await;
        }
    }

    #[tokio::test]
    async fn fallback_recognizes_a_flat_overload_in_mislabeled_sse() {
        let overload_event = concat!(
            ": provider comment\n\n",
            "event: error\n",
            "data: {\"type\":\"error\",\"code\":\"server_error\",\"message\":\"Selected model is at capacity. Please try a different model.\"}\n\n"
        );
        let success_event =
            "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\n\n";
        let mut a_state = mock_upstream(StatusCode::OK, overload_event);
        a_state.headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/octet-stream"),
        );
        let a_requests = Arc::clone(&a_state.requests);
        let a_server = start_mock_upstream(a_state).await;
        let mut b_state = mock_upstream(StatusCode::OK, success_event);
        b_state.headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/event-stream"),
        );
        let b_requests = Arc::clone(&b_state.requests);
        let b_server = start_mock_upstream(b_state).await;
        let a = route_snapshot("A", &format!("http://{}/v1", a_server.address()));
        let b = route_snapshot("B", &format!("http://{}/v1", b_server.address()));
        let (request, routing) = fallback_request(true, vec![Arc::clone(&a), Arc::clone(&b)]);
        let history = Arc::new(HistoryCapture::default());
        let activator = Arc::new(InMemoryFallbackActivator::new(routing.clone()));
        let forwarder = test_forwarder(
            UpstreamForwarderConfig::default(),
            history.clone(),
            Arc::new(DiagnosticCapture::default()),
            InferenceStatusService::new(Arc::new(NoopInferenceChanges)),
        )
        .with_fallback_services(routing.clone(), activator.clone());

        let response = forwarder.handle(request).await;
        let downstream = to_bytes(response.into_body(), success_event.len() + 1)
            .await
            .expect("next route body");

        assert_eq!(downstream.as_ref(), success_event.as_bytes());
        assert_eq!(a_requests.lock().expect("A requests").len(), 1);
        assert_eq!(b_requests.lock().expect("B requests").len(), 1);
        assert_eq!(activator.activations.lock().expect("activations").len(), 1);
        assert_eq!(
            history.0.lock().expect("history")[0]
                .error_category
                .as_deref(),
            Some("upstream_overloaded")
        );
        a_server.shutdown().await;
        b_server.shutdown().await;
    }

    #[tokio::test]
    async fn non_sse_stream_commits_unchanged_without_fallback() {
        let ordinary = b"{\"type\":\"error\",\"code\":\"server_overloaded\"}";
        let mut a_state = mock_upstream(StatusCode::OK, ordinary.as_slice());
        a_state.headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        let a_requests = Arc::clone(&a_state.requests);
        let a_server = start_mock_upstream(a_state).await;
        let b_state = mock_upstream(StatusCode::OK, b"unused".as_slice());
        let b_requests = Arc::clone(&b_state.requests);
        let b_server = start_mock_upstream(b_state).await;
        let a = route_snapshot("A", &format!("http://{}/v1", a_server.address()));
        let b = route_snapshot("B", &format!("http://{}/v1", b_server.address()));
        let (request, routing) = fallback_request(true, vec![Arc::clone(&a), b]);
        let activator = Arc::new(InMemoryFallbackActivator::new(routing.clone()));
        let forwarder = ResponsesForwarder::new()
            .expect("forwarder")
            .with_fallback_services(routing.clone(), activator.clone());

        let response = forwarder.handle(request).await;
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE),
            Some(&HeaderValue::from_static("application/json"))
        );
        let downstream = to_bytes(response.into_body(), ordinary.len() + 1)
            .await
            .expect("ordinary body");

        assert_eq!(downstream.as_ref(), ordinary);
        assert_eq!(a_requests.lock().expect("A requests").len(), 1);
        assert!(b_requests.lock().expect("B requests").is_empty());
        assert!(
            activator
                .activations
                .lock()
                .expect("activations")
                .is_empty()
        );
        a_server.shutdown().await;
        b_server.shutdown().await;
    }

    #[tokio::test]
    async fn ineligible_precommit_semantic_failure_records_the_actual_stop_reason() {
        let failure_event = "data: {\"type\":\"response.failed\",\"response\":{\"status\":\"failed\",\"error\":{\"code\":\"server_error\"}}}\n\n";
        let mut a_state = mock_upstream(StatusCode::OK, failure_event);
        a_state.headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/event-stream"),
        );
        let a_requests = Arc::clone(&a_state.requests);
        let a_server = start_mock_upstream(a_state).await;
        let b_state = mock_upstream(StatusCode::OK, br#"{"status":"completed"}"#.as_slice());
        let b_requests = Arc::clone(&b_state.requests);
        let b_server = start_mock_upstream(b_state).await;
        let a = route_snapshot("A", &format!("http://{}/v1", a_server.address()));
        let b = route_snapshot("B", &format!("http://{}/v1", b_server.address()));
        let (request, routing) = fallback_request(true, vec![Arc::clone(&a), b]);
        let history = Arc::new(DecisionHistoryCapture::default());
        let activator = Arc::new(InMemoryFallbackActivator::new(routing.clone()));
        let forwarder = test_forwarder(
            UpstreamForwarderConfig::default(),
            history.clone(),
            Arc::new(DiagnosticCapture::default()),
            InferenceStatusService::new(Arc::new(NoopInferenceChanges)),
        )
        .with_fallback_services(routing.clone(), activator.clone());

        let response = forwarder.handle(request).await;
        let downstream = to_bytes(response.into_body(), failure_event.len() + 1)
            .await
            .expect("semantic failure body");

        assert_eq!(downstream.as_ref(), failure_event.as_bytes());
        assert_eq!(a_requests.lock().expect("A requests").len(), 1);
        assert!(b_requests.lock().expect("B requests").is_empty());
        assert!(
            activator
                .activations
                .lock()
                .expect("activations")
                .is_empty()
        );
        assert_eq!(
            routing.load().active.as_ref().map(|route| &route.route_id),
            Some(&a.route_id)
        );
        {
            let stops = history.stops.lock().expect("fallback stops");
            assert_eq!(stops.len(), 1);
            assert_eq!(stops[0].attempt_index, 0);
            assert_eq!(stops[0].reason, FallbackStopReason::FailureNotEligible);
        }

        a_server.shutdown().await;
        b_server.shutdown().await;
    }

    #[tokio::test]
    async fn non_streaming_model_and_capacity_failures_forward_only_on_exact_http_200_failed() {
        for failure_body in [
            br#"{"status":"failed","error":{"code":"model_unavailable"}}"#.as_slice(),
            br#"{"status":"failed","error":{"code":"server_error","message":"Selected model is at capacity. Please try a different model."}}"#.as_slice(),
        ] {
            let a_state = mock_upstream(StatusCode::OK, failure_body);
            let a_requests = Arc::clone(&a_state.requests);
            let a_server = start_mock_upstream(a_state).await;
            let b_state = mock_upstream(StatusCode::OK, br#"{"status":"completed"}"#.as_slice());
            let b_requests = Arc::clone(&b_state.requests);
            let b_server = start_mock_upstream(b_state).await;
            let a = route_snapshot("A", &format!("http://{}/v1", a_server.address()));
            let b = route_snapshot("B", &format!("http://{}/v1", b_server.address()));
            let (request, routing) = fallback_request(false, vec![a, b]);
            let activator = Arc::new(InMemoryFallbackActivator::new(routing.clone()));
            let history = Arc::new(HistoryCapture::default());
            let forwarder = test_forwarder(
                UpstreamForwarderConfig::default(),
                history.clone(),
                Arc::new(DiagnosticCapture::default()),
                InferenceStatusService::new(Arc::new(NoopInferenceChanges)),
            )
            .with_fallback_services(routing, activator);

            let response = forwarder.handle(request).await;

            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(a_requests.lock().expect("A requests").len(), 1);
            assert_eq!(b_requests.lock().expect("B requests").len(), 1);
            assert!(
                !format!("{:?}", history.0.lock().expect("history"))
                    .contains("Selected model is at capacity")
            );
            a_server.shutdown().await;
            b_server.shutdown().await;
        }
    }

    #[tokio::test]
    async fn final_route_returns_the_original_overload_stream_after_one_attempt() {
        let overload_event =
            "data: {\"type\":\"error\",\"codex_error_info\":\"server_overloaded\"}\n\n";
        let mut state = mock_upstream(StatusCode::OK, overload_event);
        state.headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/event-stream"),
        );
        let requests = Arc::clone(&state.requests);
        let server = start_mock_upstream(state).await;
        let route = route_snapshot("Final", &format!("http://{}/v1", server.address()));
        let (request, routing) = fallback_request(true, vec![Arc::clone(&route)]);
        let history = Arc::new(HistoryCapture::default());
        let inference = InferenceStatusService::new(Arc::new(NoopInferenceChanges));
        let activator = Arc::new(InMemoryFallbackActivator::new(routing.clone()));
        let forwarder = test_forwarder(
            UpstreamForwarderConfig::default(),
            history.clone(),
            Arc::new(DiagnosticCapture::default()),
            inference.clone(),
        )
        .with_fallback_services(routing.clone(), activator.clone());

        let response = forwarder.handle(request).await;
        let downstream = to_bytes(response.into_body(), overload_event.len() + 1)
            .await
            .expect("final route overload body");

        assert_eq!(downstream.as_ref(), overload_event.as_bytes());
        assert_eq!(requests.lock().expect("requests").len(), 1);
        assert!(
            activator
                .activations
                .lock()
                .expect("activations")
                .is_empty()
        );
        assert_eq!(
            routing
                .load()
                .active
                .as_ref()
                .map(|active| &active.route_id),
            Some(&route.route_id)
        );
        assert_eq!(
            history.0.lock().expect("history")[0]
                .error_category
                .as_deref(),
            Some("upstream_overloaded")
        );
        assert_eq!(
            inference
                .status(&route.route_id, now_millis())
                .failure_reason,
            Some(InferenceFailureReason::Service)
        );
        server.shutdown().await;
    }

    #[tokio::test]
    async fn fallback_forwards_unrecognized_401_without_claiming_a_specific_cause() {
        let a_state = mock_upstream(
            StatusCode::UNAUTHORIZED,
            br#"{"error":{"code":"provider_specific_auth"}}"#.as_slice(),
        );
        let a_requests = Arc::clone(&a_state.requests);
        let a_server = start_mock_upstream(a_state).await;
        let b_state = mock_upstream(StatusCode::OK, br#"{"status":"completed"}"#.as_slice());
        let b_requests = Arc::clone(&b_state.requests);
        let b_server = start_mock_upstream(b_state).await;
        let a = route_snapshot("A", &format!("http://{}/v1", a_server.address()));
        let b = route_snapshot("B", &format!("http://{}/v1", b_server.address()));
        let (request, routing) = fallback_request(false, vec![Arc::clone(&a), Arc::clone(&b)]);
        let activator = Arc::new(InMemoryFallbackActivator::new(routing.clone()));
        let forwarder = ResponsesForwarder::new()
            .expect("forwarder")
            .with_fallback_services(routing.clone(), activator.clone());

        let response = forwarder.handle(request).await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(a_requests.lock().expect("A requests").len(), 1);
        assert_eq!(b_requests.lock().expect("B requests").len(), 1);
        assert_eq!(
            activator
                .activations
                .lock()
                .expect("activations")
                .as_slice(),
            [(a.route_id.clone(), b.route_id.clone())]
        );
        assert_eq!(
            routing.load().active.as_ref().map(|route| &route.route_id),
            Some(&b.route_id)
        );
        a_server.shutdown().await;
        b_server.shutdown().await;
    }

    #[tokio::test]
    async fn fallback_returns_unknown_403_as_access_denied_without_activation() {
        let a_state = mock_upstream(
            StatusCode::FORBIDDEN,
            br#"{"Code":"AccessDenied","Message":"Synthetic access restriction."}"#.as_slice(),
        );
        let a_requests = Arc::clone(&a_state.requests);
        let a_server = start_mock_upstream(a_state).await;
        let b_state = mock_upstream(StatusCode::OK, br#"{"status":"completed"}"#.as_slice());
        let b_requests = Arc::clone(&b_state.requests);
        let b_server = start_mock_upstream(b_state).await;
        let a = route_snapshot("A", &format!("http://{}/v1", a_server.address()));
        let b = route_snapshot("B", &format!("http://{}/v1", b_server.address()));
        let (request, routing) = fallback_request(false, vec![Arc::clone(&a), b]);
        let history = Arc::new(HistoryCapture::default());
        let inference = InferenceStatusService::new(Arc::new(NoopInferenceChanges));
        let activator = Arc::new(InMemoryFallbackActivator::new(routing.clone()));
        let forwarder = ResponsesForwarder::new()
            .expect("forwarder")
            .with_runtime_services(
                history.clone(),
                Arc::new(DiagnosticCapture::default()),
                inference.clone(),
            )
            .with_fallback_services(routing.clone(), activator.clone());

        let response = forwarder.handle(request).await;

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(a_requests.lock().expect("A requests").len(), 1);
        assert!(b_requests.lock().expect("B requests").is_empty());
        assert!(
            activator
                .activations
                .lock()
                .expect("activations")
                .is_empty()
        );
        assert_eq!(
            routing.load().active.as_ref().map(|route| &route.route_id),
            Some(&a.route_id)
        );
        assert_eq!(
            history.0.lock().expect("history")[0]
                .error_category
                .as_deref(),
            Some("upstream_access_denied")
        );
        assert_eq!(history.0.lock().expect("history")[0].http_status, Some(403));
        assert_eq!(
            inference.status(&a.route_id, now_millis()).failure_reason,
            Some(InferenceFailureReason::AccessDenied)
        );
        a_server.shutdown().await;
        b_server.shutdown().await;
    }

    #[tokio::test]
    async fn fallback_advances_after_the_first_generic_5xx_and_keeps_the_successful_target() {
        let a_requests = Arc::new(AtomicUsize::new(0));
        let a_server = start_sequence_upstream(SequenceUpstream {
            statuses: Arc::new(vec![StatusCode::INTERNAL_SERVER_ERROR, StatusCode::OK]),
            requests: Arc::clone(&a_requests),
        })
        .await;
        let b_state = mock_upstream(StatusCode::OK, br#"{"status":"completed"}"#.as_slice());
        let b_requests = Arc::clone(&b_state.requests);
        let b_server = start_mock_upstream(b_state).await;
        let a = route_snapshot("A", &format!("http://{}/v1", a_server.address()));
        let b = route_snapshot("B", &format!("http://{}/v1", b_server.address()));
        let (request, routing) = fallback_request(false, vec![Arc::clone(&a), Arc::clone(&b)]);
        let activator = Arc::new(InMemoryFallbackActivator::new(routing.clone()));
        let forwarder = ResponsesForwarder::new()
            .expect("forwarder")
            .with_fallback_services(routing.clone(), activator.clone());

        let response = forwarder.handle(request).await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(a_requests.load(Ordering::Acquire), 1);
        assert_eq!(b_requests.lock().expect("B requests").len(), 1);
        assert_eq!(
            activator
                .activations
                .lock()
                .expect("activations")
                .as_slice(),
            [(a.route_id.clone(), b.route_id.clone())]
        );
        assert_eq!(
            routing.load().active.as_ref().map(|route| &route.route_id),
            Some(&b.route_id)
        );
        a_server.shutdown().await;
        b_server.shutdown().await;
    }

    #[tokio::test]
    async fn lifecycle_sse_events_do_not_reset_the_first_output_deadline() {
        let a_requests = Arc::new(AtomicUsize::new(0));
        let a_server = start_lifecycle_upstream(LifecycleUpstream {
            requests: Arc::clone(&a_requests),
            first_chunk: Arc::new(Notify::new()),
        })
        .await;
        let b_body = concat!(
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"ok\"}\n\n",
            "data: [DONE]\n\n"
        );
        let mut b_state = mock_upstream(StatusCode::OK, b_body);
        b_state.headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/event-stream"),
        );
        let b_requests = Arc::clone(&b_state.requests);
        let b_server = start_mock_upstream(b_state).await;
        let a = route_snapshot("A", &format!("http://{}/v1", a_server.address()));
        let b = route_snapshot("B", &format!("http://{}/v1", b_server.address()));
        let (request, routing) = fallback_request(true, vec![a, Arc::clone(&b)]);
        let activator = Arc::new(InMemoryFallbackActivator::new(routing.clone()));
        let forwarder = ResponsesForwarder::with_config(UpstreamForwarderConfig {
            first_output_timeout: Duration::from_millis(45),
            ..UpstreamForwarderConfig::default()
        })
        .expect("forwarder")
        .with_fallback_services(routing.clone(), activator);

        let response = tokio::time::timeout(Duration::from_secs(1), forwarder.handle(request))
            .await
            .expect("fixed first-output deadline");
        let downstream = to_bytes(response.into_body(), b_body.len() + 1)
            .await
            .expect("B SSE body");

        assert_eq!(downstream.as_ref(), b_body.as_bytes());
        assert_eq!(a_requests.load(Ordering::Acquire), 1);
        assert_eq!(b_requests.lock().expect("B requests").len(), 1);
        assert_eq!(
            routing.load().active.as_ref().map(|route| &route.route_id),
            Some(&b.route_id)
        );
        a_server.shutdown().await;
        b_server.shutdown().await;
    }

    #[tokio::test]
    async fn fallback_bounds_persistent_first_output_timeouts_at_one_per_participant() {
        let mut servers = Vec::new();
        let mut routes = Vec::new();
        let mut request_counts = Vec::new();
        for name in ["A", "B", "C", "D"] {
            let requests = Arc::new(AtomicUsize::new(0));
            let server = start_lifecycle_upstream(LifecycleUpstream {
                requests: Arc::clone(&requests),
                first_chunk: Arc::new(Notify::new()),
            })
            .await;
            routes.push(route_snapshot(
                name,
                &format!("http://{}/v1", server.address()),
            ));
            request_counts.push(requests);
            servers.push(server);
        }
        let (request, routing) = fallback_request(true, routes.clone());
        let activator = Arc::new(InMemoryFallbackActivator::new(routing.clone()));
        let forwarder = ResponsesForwarder::with_config(UpstreamForwarderConfig {
            first_output_timeout: Duration::from_millis(25),
            ..UpstreamForwarderConfig::default()
        })
        .expect("forwarder")
        .with_fallback_services(routing.clone(), activator.clone());

        let response = forwarder.handle(request).await;

        assert_eq!(response.status(), StatusCode::GATEWAY_TIMEOUT);
        assert_eq!(
            request_counts
                .iter()
                .map(|requests| requests.load(Ordering::Acquire))
                .collect::<Vec<_>>(),
            vec![1, 1, 1, 1]
        );
        assert_eq!(activator.activations.lock().expect("activations").len(), 3);
        assert_eq!(
            routing.load().active.as_ref().map(|route| &route.route_id),
            Some(&routes[3].route_id)
        );

        for server in servers {
            server.shutdown().await;
        }
    }

    #[tokio::test]
    async fn disabling_fallback_during_sse_preflight_commits_the_current_stream() {
        let first_chunk = Arc::new(Notify::new());
        let first_chunk_seen = first_chunk.notified();
        let a_requests = Arc::new(AtomicUsize::new(0));
        let a_server = start_lifecycle_upstream(LifecycleUpstream {
            requests: Arc::clone(&a_requests),
            first_chunk: Arc::clone(&first_chunk),
        })
        .await;
        let b_state = mock_upstream(StatusCode::OK, br#"{"status":"completed"}"#.as_slice());
        let b_requests = Arc::clone(&b_state.requests);
        let b_server = start_mock_upstream(b_state).await;
        let a = route_snapshot("A", &format!("http://{}/v1", a_server.address()));
        let b = route_snapshot("B", &format!("http://{}/v1", b_server.address()));
        let (request, routing) = fallback_request(true, vec![Arc::clone(&a), b]);
        let activator = Arc::new(InMemoryFallbackActivator::new(routing.clone()));
        let forwarder = Arc::new(
            ResponsesForwarder::with_config(UpstreamForwarderConfig {
                first_output_timeout: Duration::from_secs(5),
                ..UpstreamForwarderConfig::default()
            })
            .expect("forwarder")
            .with_fallback_services(routing.clone(), activator.clone()),
        );
        let response_task = tokio::spawn({
            let forwarder = Arc::clone(&forwarder);
            async move { forwarder.handle(request).await }
        });
        first_chunk_seen.await;
        let current = routing.load();
        routing.store(Arc::new(RoutingSnapshot {
            active: current.active.clone(),
            participants: current.participants.clone(),
            enabled: false,
            selection_generation: current.selection_generation,
            config_revision: current.config_revision.saturating_add(1),
            images_generation_enabled: current.images_generation_enabled,
            images_route: current.images_route.clone(),
            images_generation_timeout: current.images_generation_timeout,
        }));

        let response = tokio::time::timeout(Duration::from_millis(500), response_task)
            .await
            .expect("disable should commit preflight")
            .expect("response task");

        assert_eq!(response.status(), StatusCode::OK);
        drop(response);
        assert_eq!(a_requests.load(Ordering::Acquire), 1);
        assert!(b_requests.lock().expect("B requests").is_empty());
        assert!(
            activator
                .activations
                .lock()
                .expect("activations")
                .is_empty()
        );
        a_server.shutdown().await;
        b_server.shutdown().await;
    }

    #[tokio::test]
    async fn sse_preflight_commits_a_single_oversized_chunk_at_the_strict_limit() {
        let forwarder = ResponsesForwarder::new().expect("forwarder");
        let request = request(true);
        let context = forwarder.history_context(&request, 0);
        let mut source = vec![b'x'; SSE_PREFLIGHT_LIMIT + 37];
        source[0] = b':';
        let source = Bytes::from(source);
        let upstream: reqwest::Response = axum::http::Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/event-stream")
            .body(reqwest::Body::from(source.clone()))
            .expect("upstream response")
            .into();
        let mut preflight = SsePreflight::new(upstream, context, Instant::now());
        let chunk = preflight
            .stream
            .next()
            .await
            .expect("upstream chunk")
            .expect("valid upstream chunk");
        assert_eq!(chunk, source);

        assert_eq!(
            preflight.push_chunk(chunk, SSE_PREFLIGHT_LIMIT),
            SsePreflightSignal::Commit
        );
        assert_eq!(preflight.buffered_bytes, SSE_PREFLIGHT_LIMIT);
        assert_eq!(
            preflight
                .buffered
                .iter()
                .map(axum::body::Bytes::len)
                .sum::<usize>(),
            SSE_PREFLIGHT_LIMIT
        );
        let AttemptResult::Committed(response) =
            preflight.commit(PreflightCommitReason::BufferLimit)
        else {
            panic!("preflight must commit at the byte limit");
        };
        let downstream = to_bytes(response.into_body(), SSE_PREFLIGHT_LIMIT * 2)
            .await
            .expect("committed SSE body");

        assert_eq!(downstream, source);
    }

    #[tokio::test]
    async fn probed_sse_commits_incomplete_and_invalid_input_without_fallback() {
        for (source, expected_reason) in [
            (Bytes::from_static(b"dat"), None),
            (Bytes::from_static(b"data:"), None),
            (
                Bytes::from_static(b"data: {\"type\":\"error\",\"code\":\"server_overloaded\"}"),
                None,
            ),
            (
                Bytes::from_static(b"data: {\"type\":\"error\",\"code\":\"server_overloaded\"}\n"),
                None,
            ),
            (
                Bytes::from_static(b"data: {\"type\":\"provider.unknown\"}\n\n"),
                Some(PreflightCommitReason::UnknownEvent),
            ),
            (
                Bytes::from_static(b"data: {\n\n"),
                Some(PreflightCommitReason::MalformedEvent),
            ),
        ] {
            let forwarder = ResponsesForwarder::new().expect("forwarder");
            let request = request(true);
            let context = forwarder.history_context(&request, 0);
            let upstream: reqwest::Response = axum::http::Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "application/octet-stream")
                .body(reqwest::Body::from(source.clone()))
                .expect("upstream response")
                .into();
            let mut preflight = SsePreflight::new(upstream, context, Instant::now());
            let chunk = preflight
                .stream
                .next()
                .await
                .expect("upstream chunk")
                .expect("valid upstream chunk");

            let signal = preflight.push_chunk(chunk, SSE_PREFLIGHT_LIMIT);
            if let Some(expected_reason) = expected_reason {
                assert_eq!(signal, SsePreflightSignal::Commit);
                assert_eq!(preflight.commit_reason, Some(expected_reason));
            } else {
                assert_eq!(signal, SsePreflightSignal::Continue);
            }
            let result = match signal {
                SsePreflightSignal::Continue => preflight.finish_input(&request.request_id),
                SsePreflightSignal::Commit => preflight.commit_observed(),
                SsePreflightSignal::TerminalFailure => {
                    panic!("incomplete or invalid probe input must not trigger fallback")
                }
            };
            let AttemptResult::Committed(response) = result else {
                panic!("incomplete or invalid probe input must commit")
            };
            let downstream = to_bytes(response.into_body(), source.len() + 1)
                .await
                .expect("committed probe body");

            assert_eq!(downstream, source);
        }
    }

    #[tokio::test]
    async fn sse_preflight_decides_in_wire_order_and_replays_the_whole_chunk() {
        let error = "data: {\"type\":\"error\",\"code\":\"server_overloaded\"}\n\n";
        let output = "data: {\"type\":\"response.output_text.delta\",\"delta\":\"visible\"}\n\n";

        for (source, expected_signal) in [
            (
                Bytes::from(format!("{error}{output}")),
                SsePreflightSignal::TerminalFailure,
            ),
            (
                Bytes::from(format!("{output}{error}")),
                SsePreflightSignal::Commit,
            ),
        ] {
            let forwarder = ResponsesForwarder::new().expect("forwarder");
            let request = request(true);
            let context = forwarder.history_context(&request, 0);
            let upstream: reqwest::Response = axum::http::Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "text/event-stream")
                .body(reqwest::Body::from(source.clone()))
                .expect("upstream response")
                .into();
            let mut preflight = SsePreflight::new(upstream, context, Instant::now());
            let chunk = preflight
                .stream
                .next()
                .await
                .expect("upstream chunk")
                .expect("valid upstream chunk");

            assert_eq!(
                preflight.push_chunk(chunk, SSE_PREFLIGHT_LIMIT),
                expected_signal
            );
            assert_eq!(preflight.buffered.as_slice(), std::slice::from_ref(&source));
            let response = match expected_signal {
                SsePreflightSignal::TerminalFailure => {
                    let AttemptResult::PrecommitFailure { response, .. } =
                        preflight.terminal_failure()
                    else {
                        panic!("error-first chunk must remain precommit")
                    };
                    response
                }
                SsePreflightSignal::Commit => {
                    let AttemptResult::Committed(response) = preflight.commit_observed() else {
                        panic!("output-first chunk must commit")
                    };
                    response
                }
                SsePreflightSignal::Continue => unreachable!("fixture is decisive"),
            };
            let downstream = to_bytes(response.into_body(), source.len() + 1)
                .await
                .expect("replayed stream body");
            assert_eq!(downstream, source);
        }
    }

    #[tokio::test]
    async fn committed_stream_failure_preserves_the_active_route() {
        let a_requests = Arc::new(AtomicUsize::new(0));
        let a_server = start_committed_stream_upstream(CommittedStreamUpstream {
            requests: Arc::clone(&a_requests),
            outcome: CommittedStreamOutcome::ReadFailure,
        })
        .await;
        let b_state = mock_upstream(StatusCode::OK, br#"{"status":"completed"}"#.as_slice());
        let b_requests = Arc::clone(&b_state.requests);
        let b_server = start_mock_upstream(b_state).await;
        let a = route_snapshot("A", &format!("http://{}/v1", a_server.address()));
        let b = route_snapshot("B", &format!("http://{}/v1", b_server.address()));
        let (request, routing) = fallback_request(true, vec![Arc::clone(&a), b]);
        let history = Arc::new(HistoryCapture::default());
        let activator = Arc::new(InMemoryFallbackActivator::new(routing.clone()));
        let forwarder = test_forwarder(
            UpstreamForwarderConfig::default(),
            history.clone(),
            Arc::new(DiagnosticCapture::default()),
            InferenceStatusService::new(Arc::new(NoopInferenceChanges)),
        )
        .with_fallback_services(routing.clone(), activator.clone());

        let response = forwarder.handle(request).await;
        let error = to_bytes(response.into_body(), 1024)
            .await
            .expect_err("committed upstream read failure");

        assert!(
            error
                .to_string()
                .contains("upstream response stream failed")
        );
        assert_eq!(a_requests.load(Ordering::Acquire), 1);
        assert!(b_requests.lock().expect("B requests").is_empty());
        assert!(
            activator
                .activations
                .lock()
                .expect("activations")
                .is_empty()
        );
        assert_eq!(
            routing.load().active.as_ref().map(|route| &route.route_id),
            Some(&a.route_id)
        );
        {
            let records = history.0.lock().expect("history mutex");
            assert_eq!(records.len(), 1);
            assert_eq!(records[0].completion_state, CompletionState::Failed);
            assert_eq!(
                records[0].error_category.as_deref(),
                Some("upstream_read_failed")
            );
        }
        a_server.shutdown().await;
        b_server.shutdown().await;
    }

    #[tokio::test]
    async fn overload_after_meaningful_output_preserves_the_active_route() {
        let a_requests = Arc::new(AtomicUsize::new(0));
        let a_server = start_committed_stream_upstream(CommittedStreamUpstream {
            requests: Arc::clone(&a_requests),
            outcome: CommittedStreamOutcome::SemanticOverload,
        })
        .await;
        let b_state = mock_upstream(StatusCode::OK, br#"{"status":"completed"}"#.as_slice());
        let b_requests = Arc::clone(&b_state.requests);
        let b_server = start_mock_upstream(b_state).await;
        let a = route_snapshot("A", &format!("http://{}/v1", a_server.address()));
        let b = route_snapshot("B", &format!("http://{}/v1", b_server.address()));
        let (request, routing) = fallback_request(true, vec![Arc::clone(&a), b]);
        let history = Arc::new(HistoryCapture::default());
        let diagnostics = Arc::new(DiagnosticCapture::default());
        let inference = InferenceStatusService::new(Arc::new(NoopInferenceChanges));
        let activator = Arc::new(InMemoryFallbackActivator::new(routing.clone()));
        let forwarder = test_forwarder(
            UpstreamForwarderConfig::default(),
            history.clone(),
            diagnostics.clone(),
            inference.clone(),
        )
        .with_fallback_services(routing.clone(), activator.clone());

        let response = forwarder.handle(request).await;
        let downstream = to_bytes(response.into_body(), 1024)
            .await
            .expect("committed overload SSE body");

        assert_eq!(
            downstream.as_ref(),
            concat!(
                "data: {\"type\":\"response.output_text.delta\",\"delta\":\"ok\"}\n\n",
                "data: {\"type\":\"response.failed\",\"response\":{\"status\":\"failed\",\"error\":{\"codex_error_info\":\"server_overloaded\"}}}\n\n"
            )
            .as_bytes()
        );
        assert_eq!(a_requests.load(Ordering::Acquire), 1);
        assert!(b_requests.lock().expect("B requests").is_empty());
        assert!(
            activator
                .activations
                .lock()
                .expect("activations")
                .is_empty()
        );
        assert_eq!(
            routing.load().active.as_ref().map(|route| &route.route_id),
            Some(&a.route_id)
        );
        assert_eq!(
            inference.status(&a.route_id, now_millis()).failure_reason,
            Some(InferenceFailureReason::Service)
        );
        {
            let records = history.0.lock().expect("history");
            assert_eq!(records.len(), 1);
            assert_eq!(records[0].completion_state, CompletionState::Failed);
            assert_eq!(
                records[0].error_category.as_deref(),
                Some("upstream_overloaded")
            );
            assert_eq!(
                records[0].attempts[0].delivery_state,
                DeliveryState::Completed
            );
        }
        assert_eq!(
            diagnostics.0.lock().expect("diagnostics")[0].code,
            RuntimeDiagnosticCode::UpstreamPreflightMeaningfulOutput
        );
        a_server.shutdown().await;
        b_server.shutdown().await;
    }

    #[tokio::test]
    async fn committed_stream_cancellation_preserves_the_active_route() {
        let a_requests = Arc::new(AtomicUsize::new(0));
        let a_server = start_committed_stream_upstream(CommittedStreamUpstream {
            requests: Arc::clone(&a_requests),
            outcome: CommittedStreamOutcome::Pending,
        })
        .await;
        let b_state = mock_upstream(StatusCode::OK, br#"{"status":"completed"}"#.as_slice());
        let b_requests = Arc::clone(&b_state.requests);
        let b_server = start_mock_upstream(b_state).await;
        let a = route_snapshot("A", &format!("http://{}/v1", a_server.address()));
        let b = route_snapshot("B", &format!("http://{}/v1", b_server.address()));
        let (request, routing) = fallback_request(true, vec![Arc::clone(&a), b]);
        let history = Arc::new(HistoryCapture::default());
        let activator = Arc::new(InMemoryFallbackActivator::new(routing.clone()));
        let forwarder = test_forwarder(
            UpstreamForwarderConfig::default(),
            history.clone(),
            Arc::new(DiagnosticCapture::default()),
            InferenceStatusService::new(Arc::new(NoopInferenceChanges)),
        )
        .with_fallback_services(routing.clone(), activator.clone());

        let response = forwarder.handle(request).await;
        drop(response);

        assert_eq!(a_requests.load(Ordering::Acquire), 1);
        assert!(b_requests.lock().expect("B requests").is_empty());
        assert!(
            activator
                .activations
                .lock()
                .expect("activations")
                .is_empty()
        );
        assert_eq!(
            routing.load().active.as_ref().map(|route| &route.route_id),
            Some(&a.route_id)
        );
        {
            let records = history.0.lock().expect("history mutex");
            assert_eq!(records.len(), 1);
            assert_eq!(records[0].completion_state, CompletionState::Cancelled);
            assert_eq!(records[0].error_category, None);
        }
        a_server.shutdown().await;
        b_server.shutdown().await;
    }

    #[tokio::test]
    async fn activation_persistence_failure_returns_current_result_and_emits_safe_diagnostic() {
        let a_state = mock_upstream(
            StatusCode::TOO_MANY_REQUESTS,
            br#"{"error":{"code":"rate_limit_exceeded"}}"#.as_slice(),
        );
        let a_server = start_mock_upstream(a_state).await;
        let b_state = mock_upstream(StatusCode::OK, br#"{"status":"completed"}"#.as_slice());
        let b_requests = Arc::clone(&b_state.requests);
        let b_server = start_mock_upstream(b_state).await;
        let a = route_snapshot("A", &format!("http://{}/v1", a_server.address()));
        let b = route_snapshot("B", &format!("http://{}/v1", b_server.address()));
        let (request, routing) = fallback_request(false, vec![Arc::clone(&a), b]);
        let diagnostics = Arc::new(DiagnosticCapture::default());
        let activator = Arc::new(InMemoryFallbackActivator::failing(routing.clone()));
        let forwarder = test_forwarder(
            UpstreamForwarderConfig::default(),
            Arc::new(HistoryCapture::default()),
            diagnostics.clone(),
            InferenceStatusService::new(Arc::new(NoopInferenceChanges)),
        )
        .with_fallback_services(routing.clone(), activator);

        let response = forwarder.handle(request).await;

        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(b_requests.lock().expect("B requests").is_empty());
        assert_eq!(
            routing.load().active.as_ref().map(|route| &route.route_id),
            Some(&a.route_id)
        );
        assert!(
            diagnostics
                .0
                .lock()
                .expect("diagnostics")
                .iter()
                .any(|event| {
                    event.code == RuntimeDiagnosticCode::FallbackActivationPersistenceFailed
                        && event.request_id.as_deref() == Some("request-id")
                        && event.route_id.as_ref() == Some(&a.route_id)
                        && event.http_status.is_none()
                })
        );
        a_server.shutdown().await;
        b_server.shutdown().await;
    }

    #[test]
    fn committed_semantic_failure_keeps_exact_account_reason() {
        let history = Arc::new(HistoryCapture::default());
        let inference = InferenceStatusService::new(Arc::new(NoopInferenceChanges));
        let request = request(true);
        let route_id = request.route.route_id.clone();
        let context = RequestHistoryContext::new(
            &request,
            0,
            history.clone(),
            Arc::new(DiagnosticCapture::default()),
            Some(inference.clone()),
            Arc::new(NoopRequestTransitionSink),
        );

        finish_stream_attempt(
            context,
            StatusCode::OK,
            SseStreamResult {
                outcome: SseStreamOutcome::Completed,
                metadata: ResponseMetadata {
                    status: Some("failed".to_owned()),
                    safe_error_code: Some("insufficient_quota".to_owned()),
                    ..ResponseMetadata::default()
                },
            },
            None,
            None,
        );

        let status = inference.status(&route_id, now_millis());
        assert_eq!(status.kind, InferenceStatusKind::RecentFailure);
        assert_eq!(
            status.failure_reason,
            Some(InferenceFailureReason::InsufficientQuota)
        );
        let records = history.0.lock().expect("history mutex");
        assert_eq!(records[0].completion_state, CompletionState::Failed);
        assert_eq!(
            records[0].error_category.as_deref(),
            Some("insufficient_quota")
        );
        assert_eq!(
            records[0].attempts[0].delivery_state,
            DeliveryState::Completed
        );
    }

    #[test]
    fn committed_incomplete_response_projects_semantic_failure() {
        let history = Arc::new(HistoryCapture::default());
        let request = request(true);
        let context = RequestHistoryContext::new(
            &request,
            0,
            history.clone(),
            Arc::new(DiagnosticCapture::default()),
            None,
            Arc::new(NoopRequestTransitionSink),
        );

        finish_stream_attempt(
            context,
            StatusCode::OK,
            SseStreamResult {
                outcome: SseStreamOutcome::Completed,
                metadata: ResponseMetadata {
                    status: Some("incomplete".to_owned()),
                    ..ResponseMetadata::default()
                },
            },
            None,
            None,
        );

        let records = history.0.lock().expect("history mutex");
        assert_eq!(records[0].completion_state, CompletionState::Failed);
        assert_eq!(
            records[0].error_category.as_deref(),
            Some("upstream_semantic_failure")
        );
        assert_eq!(
            records[0].attempts[0].delivery_state,
            DeliveryState::Completed
        );
    }
}
