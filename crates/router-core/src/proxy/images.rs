use std::{borrow::Cow, sync::Arc, time::Duration};

use axum::{
    body::Bytes,
    http::{HeaderMap, HeaderValue, StatusCode, header},
};
use futures_util::StreamExt;
use rmcp::{
    ErrorData as McpError,
    handler::server::ServerHandler,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, JsonObject,
        ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool,
    },
    service::{RequestContext, RoleServer},
};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use self::asset::{
    AdmittedAssetRoot, ImageAssetErrorKind, MCP_JSON_RESPONSE_LIMIT, PublicationFault,
    process_image_response,
};

use super::{
    RoutingSnapshotStore,
    upstream::{
        DecodeError, connection_nominated_headers, decode_supported, decode_supported_exact,
        filtered_response_headers, remove_request_header, response_encodings,
    },
};

mod asset;

pub use asset::{
    McpImageAssetCleanupResult, McpImageAssetMaintenanceError, McpImageAssetManager,
    McpImageAssetSummary,
};

const DEFAULT_RESPONSE_LIMIT: usize = 200 * 1024 * 1024;
const MAX_IMAGES_UPSTREAM_ERROR_BODY_BYTES: usize = 64 * 1024;
const MAX_IMAGES_UPSTREAM_ERROR_CODE_CHARS: usize = 128;
const MAX_IMAGES_UPSTREAM_ERROR_MESSAGE_CHARS: usize = 240;
const MAX_PROMPT_BYTES: usize = 1024 * 1024;
const IMAGE_SIZE_MULTIPLE: u64 = 16;
const MAX_IMAGE_EDGE_EXCLUSIVE: u64 = 3_840;
const MAX_IMAGE_ASPECT_RATIO: u64 = 3;
const MIN_IMAGE_PIXELS: u64 = 655_360;
const MAX_IMAGE_PIXELS: u64 = 8_294_400;

pub trait ImageAssetChangeSink: Send + Sync {
    fn image_assets_changed(&self);
}

pub struct NoopImageAssetChangeSink;

impl ImageAssetChangeSink for NoopImageAssetChangeSink {
    fn image_assets_changed(&self) {}
}

#[derive(Clone)]
pub struct ImagesGenerationService {
    routing: RoutingSnapshotStore,
    config: ImagesGenerationConfig,
}

#[derive(Clone)]
struct ImagesGenerationConfig {
    body_timeout: Duration,
    response_wire_limit: usize,
    response_decoded_limit: usize,
    exact_response_capacity: bool,
}

impl Default for ImagesGenerationConfig {
    fn default() -> Self {
        Self {
            body_timeout: Duration::from_mins(10),
            response_wire_limit: DEFAULT_RESPONSE_LIMIT,
            response_decoded_limit: DEFAULT_RESPONSE_LIMIT,
            exact_response_capacity: false,
        }
    }
}

#[derive(Debug)]
pub struct ImagesGenerationResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Bytes,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImagesGenerationFailureKind {
    Disabled,
    RouteNotSelected,
    RouteUnavailable,
    InvalidRequest,
    RequestConstructionFailed,
    UpstreamConnectionFailed,
    UpstreamRequestFailed,
    UpstreamTimeout,
    ResponseBodyReadFailed,
    UpstreamHttpStatus,
    ResponseTooLarge,
    InvalidEncoding,
    InvalidResponse,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImagesFailureStage {
    RequestConstruction,
    Connection,
    RequestSend,
    UpstreamTimeout,
    ResponseBodyRead,
    UpstreamHttpStatus,
    ResponseDecode,
    ResultValidation,
    AssetStorage,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImagesUpstreamCategory {
    ContentPolicy,
    InvalidRequest,
    Authentication,
    Permission,
    RateLimit,
    Quota,
    ServerError,
    UnknownUpstream,
}

pub struct ImagesGenerationFailure {
    pub kind: ImagesGenerationFailureKind,
    pub request_id: String,
    pub stage: ImagesFailureStage,
    pub upstream_status: Option<StatusCode>,
    pub category: ImagesUpstreamCategory,
    pub retryable: bool,
    transient_provider_message: Option<String>,
}

impl ImagesGenerationFailureKind {
    pub const fn code(self) -> &'static str {
        match self {
            Self::Disabled => "images_generation_disabled",
            Self::RouteNotSelected => "images_route_not_selected",
            Self::RouteUnavailable => "images_route_unavailable",
            Self::InvalidRequest => "invalid_images_request",
            Self::RequestConstructionFailed => "images_request_construction_failed",
            Self::UpstreamConnectionFailed => "images_upstream_connection_failed",
            Self::UpstreamRequestFailed => "images_upstream_request_failed",
            Self::UpstreamTimeout => "images_upstream_timeout",
            Self::ResponseBodyReadFailed => "images_response_body_read_failed",
            Self::UpstreamHttpStatus => "images_upstream_http_status",
            Self::ResponseTooLarge => "images_response_too_large",
            Self::InvalidEncoding => "images_response_invalid_encoding",
            Self::InvalidResponse => "images_response_invalid",
        }
    }

    pub const fn status(self) -> StatusCode {
        match self {
            Self::InvalidRequest => StatusCode::BAD_REQUEST,
            Self::Disabled | Self::RouteNotSelected | Self::RouteUnavailable => {
                StatusCode::SERVICE_UNAVAILABLE
            }
            Self::UpstreamTimeout => StatusCode::GATEWAY_TIMEOUT,
            Self::RequestConstructionFailed
            | Self::UpstreamConnectionFailed
            | Self::UpstreamRequestFailed
            | Self::ResponseBodyReadFailed
            | Self::UpstreamHttpStatus
            | Self::ResponseTooLarge
            | Self::InvalidEncoding
            | Self::InvalidResponse => StatusCode::BAD_GATEWAY,
        }
    }

    pub const fn message(self) -> &'static str {
        match self {
            Self::Disabled => "Image generation is disabled.",
            Self::RouteNotSelected => "No image generation route is selected.",
            Self::RouteUnavailable => "The image generation route is unavailable.",
            Self::InvalidRequest => "The image generation request is invalid.",
            Self::RequestConstructionFailed => "The image request could not be prepared.",
            Self::UpstreamConnectionFailed => "The image provider could not be reached.",
            Self::UpstreamRequestFailed => "The image request could not be sent to the provider.",
            Self::UpstreamTimeout => "The image generation upstream request timed out.",
            Self::ResponseBodyReadFailed => "The image provider response could not be read.",
            Self::UpstreamHttpStatus => "The image provider request failed.",
            Self::ResponseTooLarge => "The image generation response exceeded the local limit.",
            Self::InvalidEncoding => "The image generation response encoding is invalid.",
            Self::InvalidResponse => "The image generation response is invalid.",
        }
    }
}

impl ImagesGenerationFailureKind {
    const fn stage(self) -> ImagesFailureStage {
        match self {
            Self::Disabled
            | Self::RouteNotSelected
            | Self::RouteUnavailable
            | Self::InvalidRequest
            | Self::RequestConstructionFailed => ImagesFailureStage::RequestConstruction,
            Self::UpstreamConnectionFailed => ImagesFailureStage::Connection,
            Self::UpstreamRequestFailed => ImagesFailureStage::RequestSend,
            Self::UpstreamTimeout => ImagesFailureStage::UpstreamTimeout,
            Self::ResponseBodyReadFailed => ImagesFailureStage::ResponseBodyRead,
            Self::UpstreamHttpStatus => ImagesFailureStage::UpstreamHttpStatus,
            Self::ResponseTooLarge | Self::InvalidEncoding | Self::InvalidResponse => {
                ImagesFailureStage::ResponseDecode
            }
        }
    }
}

impl ImagesFailureStage {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RequestConstruction => "request_construction",
            Self::Connection => "connection",
            Self::RequestSend => "request_send",
            Self::UpstreamTimeout => "upstream_timeout",
            Self::ResponseBodyRead => "response_body_read",
            Self::UpstreamHttpStatus => "upstream_http_status",
            Self::ResponseDecode => "response_decode",
            Self::ResultValidation => "result_validation",
            Self::AssetStorage => "asset_storage",
        }
    }
}

impl ImagesUpstreamCategory {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ContentPolicy => "content_policy",
            Self::InvalidRequest => "invalid_request",
            Self::Authentication => "authentication",
            Self::Permission => "permission",
            Self::RateLimit => "rate_limit",
            Self::Quota => "quota",
            Self::ServerError => "server_error",
            Self::UnknownUpstream => "unknown_upstream",
        }
    }

    const fn message(self) -> &'static str {
        match self {
            Self::ContentPolicy => {
                "The image provider rejected the request under its content policy."
            }
            Self::InvalidRequest => "The image provider rejected the image request as invalid.",
            Self::Authentication => {
                "The image provider could not authenticate the configured credentials."
            }
            Self::Permission => "The image provider denied permission for this request.",
            Self::RateLimit => "The image provider is rate limiting requests.",
            Self::Quota => "The image provider quota is exhausted.",
            Self::ServerError => "The image provider encountered a server error.",
            Self::UnknownUpstream => "The image provider returned an unrecognized error.",
        }
    }
}

impl std::fmt::Debug for ImagesGenerationFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ImagesGenerationFailure")
            .field("kind", &self.kind)
            .field("request_id", &self.request_id)
            .field("stage", &self.stage)
            .field("upstream_status", &self.upstream_status)
            .field("category", &self.category)
            .field("retryable", &self.retryable)
            .finish_non_exhaustive()
    }
}

impl ImagesGenerationFailure {
    pub fn new(kind: ImagesGenerationFailureKind) -> Self {
        Self::with_request_id(kind, Uuid::new_v4().to_string())
    }

    fn with_request_id(kind: ImagesGenerationFailureKind, request_id: String) -> Self {
        let stage = kind.stage();
        let category = ImagesUpstreamCategory::UnknownUpstream;
        Self {
            kind,
            request_id,
            stage,
            upstream_status: None,
            category,
            retryable: images_failure_is_retryable(stage, category, None),
            transient_provider_message: None,
        }
    }

    fn with_upstream_status(
        kind: ImagesGenerationFailureKind,
        request_id: String,
        status: StatusCode,
    ) -> Self {
        let mut failure = Self::with_request_id(kind, request_id);
        failure.upstream_status = Some(status);
        failure
    }

    fn upstream_http_status(
        request_id: String,
        status: StatusCode,
        projection: Option<ImagesErrorProjection>,
    ) -> Self {
        let (category, transient_provider_message) = match projection {
            Some(projection) => {
                let category = classify_upstream_category(projection.code.as_deref(), status);
                let message = (category == ImagesUpstreamCategory::UnknownUpstream)
                    .then_some(projection.message)
                    .flatten();
                (category, message)
            }
            None => (classify_upstream_category(None, status), None),
        };
        let stage = ImagesFailureStage::UpstreamHttpStatus;
        Self {
            kind: ImagesGenerationFailureKind::UpstreamHttpStatus,
            request_id,
            stage,
            upstream_status: Some(status),
            category,
            retryable: images_failure_is_retryable(stage, category, Some(status)),
            transient_provider_message,
        }
    }

    fn message(&self) -> String {
        if self.stage != ImagesFailureStage::UpstreamHttpStatus {
            return self.kind.message().to_owned();
        }
        let fixed = self.category.message();
        match self.transient_provider_message.as_deref() {
            Some(message) if self.category == ImagesUpstreamCategory::UnknownUpstream => {
                format!("{fixed} {message}")
            }
            _ => fixed.to_owned(),
        }
    }
}

#[derive(Deserialize)]
struct ImagesErrorEnvelope {
    error: Option<ImagesErrorBody>,
    code: Option<String>,
    message: Option<String>,
}

#[derive(Deserialize)]
struct ImagesErrorBody {
    code: Option<String>,
    message: Option<String>,
}

struct ImagesErrorProjection {
    code: Option<String>,
    message: Option<String>,
}

fn parse_images_error_projection(body: &[u8]) -> Option<ImagesErrorProjection> {
    let envelope: ImagesErrorEnvelope = serde_json::from_slice(body).ok()?;
    let nested_code = envelope
        .error
        .as_ref()
        .and_then(|error| error.code.as_deref());
    let nested_message = envelope
        .error
        .as_ref()
        .and_then(|error| error.message.as_deref());
    Some(ImagesErrorProjection {
        code: bounded_provider_code(nested_code.or(envelope.code.as_deref())),
        message: normalize_provider_message(nested_message.or(envelope.message.as_deref())),
    })
}

fn bounded_provider_code(code: Option<&str>) -> Option<String> {
    let code: String = code?
        .chars()
        .take(MAX_IMAGES_UPSTREAM_ERROR_CODE_CHARS)
        .collect();
    (!code.is_empty()).then_some(code)
}

fn normalize_provider_message(message: Option<&str>) -> Option<String> {
    let mut normalized = String::new();
    let mut pending_space = false;
    let mut characters = 0;
    for character in message?.chars() {
        if character.is_control() || character.is_whitespace() {
            pending_space = !normalized.is_empty();
            continue;
        }
        if characters >= MAX_IMAGES_UPSTREAM_ERROR_MESSAGE_CHARS {
            break;
        }
        if pending_space {
            if characters + 1 >= MAX_IMAGES_UPSTREAM_ERROR_MESSAGE_CHARS {
                break;
            }
            normalized.push(' ');
            characters += 1;
        }
        pending_space = false;
        normalized.push(character);
        characters += 1;
    }
    (!normalized.is_empty()).then_some(normalized)
}

fn transient_message_guards(
    prompt: Option<&str>,
    client_headers: &HeaderMap,
    route_api_key: &[u8],
) -> Vec<String> {
    let mut guards = Vec::new();
    if let Some(prompt) = prompt.and_then(|prompt| normalize_provider_message(Some(prompt))) {
        guards.push(prompt);
    }
    if let Ok(api_key) = std::str::from_utf8(route_api_key)
        && let Some(api_key) = normalize_provider_message(Some(api_key))
    {
        guards.push(api_key);
    }
    for value in client_headers.values() {
        let Ok(value) = value.to_str() else {
            continue;
        };
        if let Some(value) = normalize_provider_message(Some(value)) {
            guards.push(value.clone());
            if let Some(bearer) = value.strip_prefix("Bearer ")
                && !bearer.is_empty()
            {
                guards.push(bearer.to_owned());
            }
        }
    }
    guards
}

fn suppress_guarded_provider_message(
    mut projection: ImagesErrorProjection,
    guards: &[String],
) -> ImagesErrorProjection {
    if projection.message.as_ref().is_some_and(|message| {
        guards
            .iter()
            .any(|guard| !guard.is_empty() && message.contains(guard))
    }) {
        projection.message = None;
    }
    projection
}

fn classify_upstream_category(
    provider_code: Option<&str>,
    status: StatusCode,
) -> ImagesUpstreamCategory {
    if let Some(code) = provider_code {
        return match code {
            "content_policy_violation"
            | "content_policy"
            | "safety_violation"
            | "moderation_blocked" => ImagesUpstreamCategory::ContentPolicy,
            "invalid_request"
            | "invalid_request_error"
            | "invalid_parameter"
            | "invalid_value"
            | "bad_request" => ImagesUpstreamCategory::InvalidRequest,
            "invalid_api_key" | "authentication_error" | "unauthorized" => {
                ImagesUpstreamCategory::Authentication
            }
            "permission_denied" | "access_denied" | "AccessDenied" | "forbidden" => {
                ImagesUpstreamCategory::Permission
            }
            "rate_limit" | "rate_limit_exceeded" | "too_many_requests" => {
                ImagesUpstreamCategory::RateLimit
            }
            "insufficient_quota" | "credits_exhausted" | "billing_hard_limit_reached" => {
                ImagesUpstreamCategory::Quota
            }
            "server_error"
            | "server_overloaded"
            | "internal_server_error"
            | "service_unavailable" => ImagesUpstreamCategory::ServerError,
            _ => ImagesUpstreamCategory::UnknownUpstream,
        };
    }
    match status.as_u16() {
        400 | 422 => ImagesUpstreamCategory::InvalidRequest,
        401 => ImagesUpstreamCategory::Authentication,
        403 => ImagesUpstreamCategory::Permission,
        429 => ImagesUpstreamCategory::RateLimit,
        500..=599 => ImagesUpstreamCategory::ServerError,
        _ => ImagesUpstreamCategory::UnknownUpstream,
    }
}

const fn images_failure_is_retryable(
    stage: ImagesFailureStage,
    category: ImagesUpstreamCategory,
    status: Option<StatusCode>,
) -> bool {
    match stage {
        ImagesFailureStage::Connection
        | ImagesFailureStage::RequestSend
        | ImagesFailureStage::UpstreamTimeout
        | ImagesFailureStage::ResponseBodyRead => true,
        ImagesFailureStage::RequestConstruction
        | ImagesFailureStage::ResponseDecode
        | ImagesFailureStage::ResultValidation
        | ImagesFailureStage::AssetStorage => false,
        ImagesFailureStage::UpstreamHttpStatus => match category {
            ImagesUpstreamCategory::RateLimit => {
                matches!(status, Some(StatusCode::TOO_MANY_REQUESTS))
            }
            ImagesUpstreamCategory::ServerError => matches!(
                status,
                Some(
                    StatusCode::INTERNAL_SERVER_ERROR
                        | StatusCode::BAD_GATEWAY
                        | StatusCode::SERVICE_UNAVAILABLE
                        | StatusCode::GATEWAY_TIMEOUT
                )
            ),
            ImagesUpstreamCategory::ContentPolicy
            | ImagesUpstreamCategory::InvalidRequest
            | ImagesUpstreamCategory::Authentication
            | ImagesUpstreamCategory::Permission
            | ImagesUpstreamCategory::Quota
            | ImagesUpstreamCategory::UnknownUpstream => false,
        },
    }
}

impl ImagesGenerationService {
    #[must_use]
    pub fn new(routing: RoutingSnapshotStore) -> Self {
        Self {
            routing,
            config: ImagesGenerationConfig::default(),
        }
    }

    fn with_mcp_response_limits(mut self, wire_limit: usize, decoded_limit: usize) -> Self {
        self.config.response_wire_limit = wire_limit;
        self.config.response_decoded_limit = decoded_limit;
        self.config.exact_response_capacity = true;
        self
    }

    #[cfg(test)]
    fn with_body_timeout(mut self, body_timeout: Duration) -> Self {
        self.config.body_timeout = body_timeout;
        self
    }

    /// Forwards one bounded request through the dedicated image route.
    ///
    /// # Errors
    ///
    /// Returns a closed failure when local admission, request construction, the
    /// single upstream attempt, bounded decoding, or response validation fails.
    pub async fn forward(
        &self,
        body: Bytes,
        client_headers: &HeaderMap,
    ) -> Result<ImagesGenerationResponse, ImagesGenerationFailure> {
        self.forward_with_request_id(body, client_headers, Uuid::new_v4().to_string())
            .await
    }

    #[allow(
        clippy::too_many_lines,
        reason = "the single-attempt Images transport stays linear so no branch can hide a replay"
    )]
    async fn forward_with_request_id(
        &self,
        body: Bytes,
        client_headers: &HeaderMap,
        request_id: String,
    ) -> Result<ImagesGenerationResponse, ImagesGenerationFailure> {
        let request_json = serde_json::from_slice::<serde_json::Value>(&body)
            .ok()
            .filter(serde_json::Value::is_object)
            .ok_or_else(|| {
                ImagesGenerationFailure::with_request_id(
                    ImagesGenerationFailureKind::InvalidRequest,
                    request_id.clone(),
                )
            })?;
        let routing = self.routing.load();
        if !routing.images_generation_enabled {
            return Err(ImagesGenerationFailure::with_request_id(
                ImagesGenerationFailureKind::Disabled,
                request_id,
            ));
        }
        let route = routing.images_route.clone().ok_or_else(|| {
            ImagesGenerationFailure::with_request_id(
                ImagesGenerationFailureKind::RouteNotSelected,
                request_id.clone(),
            )
        })?;
        let message_guards = transient_message_guards(
            request_json
                .get("prompt")
                .and_then(serde_json::Value::as_str),
            client_headers,
            route.api_key.expose(),
        );
        drop(request_json);
        let headers =
            build_upstream_headers(client_headers, route.api_key.expose()).map_err(|()| {
                ImagesGenerationFailure::with_request_id(
                    ImagesGenerationFailureKind::RequestConstructionFailed,
                    request_id.clone(),
                )
            })?;
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .build()
            .map_err(|_| {
                ImagesGenerationFailure::with_request_id(
                    ImagesGenerationFailureKind::RequestConstructionFailed,
                    request_id.clone(),
                )
            })?;
        let request = client
            .post(route.base_url.images_generation_url())
            .headers(headers)
            .body(body)
            .build()
            .map_err(|_| {
                ImagesGenerationFailure::with_request_id(
                    ImagesGenerationFailureKind::RequestConstructionFailed,
                    request_id.clone(),
                )
            })?;
        let upstream =
            tokio::time::timeout(routing.images_generation_timeout, client.execute(request))
                .await
                .map_err(|_| {
                    ImagesGenerationFailure::with_request_id(
                        ImagesGenerationFailureKind::UpstreamTimeout,
                        request_id.clone(),
                    )
                })?
                .map_err(|error| {
                    let kind = if error.is_connect() {
                        ImagesGenerationFailureKind::UpstreamConnectionFailed
                    } else {
                        ImagesGenerationFailureKind::UpstreamRequestFailed
                    };
                    ImagesGenerationFailure::with_request_id(kind, request_id.clone())
                })?;
        let status = upstream.status();
        let source_headers = upstream.headers().clone();
        let wire_limit = if status.is_success() {
            self.config.response_wire_limit
        } else {
            MAX_IMAGES_UPSTREAM_ERROR_BODY_BYTES
        };
        let wire = tokio::time::timeout(
            self.config.body_timeout,
            collect_wire(
                upstream,
                wire_limit,
                self.config.exact_response_capacity || !status.is_success(),
            ),
        )
        .await
        .map_err(|_| ImagesGenerationFailure {
            kind: ImagesGenerationFailureKind::UpstreamTimeout,
            request_id: request_id.clone(),
            stage: ImagesFailureStage::UpstreamTimeout,
            upstream_status: Some(status),
            category: ImagesUpstreamCategory::UnknownUpstream,
            retryable: true,
            transient_provider_message: None,
        })?;
        let wire = match wire {
            Ok(wire) => Some(wire),
            Err(WireCollectError::Read) => {
                return Err(ImagesGenerationFailure {
                    kind: ImagesGenerationFailureKind::ResponseBodyReadFailed,
                    request_id,
                    stage: ImagesFailureStage::ResponseBodyRead,
                    upstream_status: Some(status),
                    category: ImagesUpstreamCategory::UnknownUpstream,
                    retryable: true,
                    transient_provider_message: None,
                });
            }
            Err(WireCollectError::TooLarge) if status.is_success() => {
                return Err(ImagesGenerationFailure::with_upstream_status(
                    ImagesGenerationFailureKind::ResponseTooLarge,
                    request_id,
                    status,
                ));
            }
            Err(WireCollectError::TooLarge) => None,
        };
        if !status.is_success() {
            let projection = wire.and_then(|wire| {
                decode_supported_exact(
                    wire,
                    &response_encodings(&source_headers),
                    MAX_IMAGES_UPSTREAM_ERROR_BODY_BYTES,
                )
                .ok()
                .and_then(|body| parse_images_error_projection(&body))
                .map(|projection| suppress_guarded_provider_message(projection, &message_guards))
            });
            return Err(ImagesGenerationFailure::upstream_http_status(
                request_id, status, projection,
            ));
        }
        let Some(wire) = wire else {
            return Err(ImagesGenerationFailure::with_upstream_status(
                ImagesGenerationFailureKind::InvalidResponse,
                request_id,
                status,
            ));
        };
        let encodings = response_encodings(&source_headers);
        let transformed = !encodings.is_empty();
        let decode = if self.config.exact_response_capacity {
            decode_supported_exact
        } else {
            decode_supported
        };
        let body =
            decode(wire, &encodings, self.config.response_decoded_limit).map_err(|error| {
                ImagesGenerationFailure::with_upstream_status(
                    match error {
                        DecodeError::TooLarge => ImagesGenerationFailureKind::ResponseTooLarge,
                        DecodeError::Unsupported | DecodeError::Invalid => {
                            ImagesGenerationFailureKind::InvalidEncoding
                        }
                    },
                    request_id.clone(),
                    status,
                )
            })?;
        Ok(ImagesGenerationResponse {
            status,
            headers: filtered_response_headers(&source_headers, transformed),
            body: Bytes::from(body),
        })
    }
}

fn build_upstream_headers(client: &HeaderMap, api_key: &[u8]) -> Result<HeaderMap, ()> {
    let mut headers = HeaderMap::new();
    let connection_tokens = connection_nominated_headers(client);
    for (name, value) in client {
        if !remove_request_header(name) && !connection_tokens.contains(name) {
            headers.append(name.clone(), value.clone());
        }
    }
    let mut bearer = Vec::with_capacity(7 + api_key.len());
    bearer.extend_from_slice(b"Bearer ");
    bearer.extend_from_slice(api_key);
    headers.insert(
        header::AUTHORIZATION,
        HeaderValue::from_bytes(&bearer).map_err(|_| ())?,
    );
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    headers.insert(
        header::ACCEPT_ENCODING,
        HeaderValue::from_static("identity"),
    );
    Ok(headers)
}

async fn collect_wire(
    response: reqwest::Response,
    limit: usize,
    exact_capacity: bool,
) -> Result<Vec<u8>, WireCollectError> {
    let mut wire = Vec::new();
    if exact_capacity {
        wire.try_reserve_exact(limit)
            .map_err(|_| WireCollectError::TooLarge)?;
    }
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| WireCollectError::Read)?;
        if wire.len().saturating_add(chunk.len()) > limit {
            return Err(WireCollectError::TooLarge);
        }
        wire.extend_from_slice(&chunk);
    }
    Ok(wire)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WireCollectError {
    Read,
    TooLarge,
}

#[derive(Clone)]
pub struct ImageMcpServer {
    service: ImagesGenerationService,
    asset_manager: Option<McpImageAssetManager>,
    change_sink: Arc<dyn ImageAssetChangeSink>,
    publication_fault: PublicationFault,
    tool: Arc<Tool>,
}

impl ImageMcpServer {
    pub(super) fn new(
        service: ImagesGenerationService,
        asset_manager: Option<McpImageAssetManager>,
        change_sink: Arc<dyn ImageAssetChangeSink>,
    ) -> Self {
        Self {
            service: service
                .with_mcp_response_limits(MCP_JSON_RESPONSE_LIMIT, MCP_JSON_RESPONSE_LIMIT),
            asset_manager,
            change_sink,
            publication_fault: PublicationFault::default(),
            tool: Arc::new(generate_image_tool()),
        }
    }

    #[cfg(test)]
    fn with_publication_fault(mut self, publication_fault: PublicationFault) -> Self {
        self.publication_fault = publication_fault;
        self
    }

    async fn generate_image(&self, args: GenerateImageArgs) -> Result<CallToolResult, McpError> {
        let request_id = Uuid::new_v4().to_string();
        if args.prompt.trim().is_empty() || args.prompt.len() > MAX_PROMPT_BYTES {
            return Err(mcp_request_error(
                ImagesGenerationFailureKind::InvalidRequest,
                request_id,
            ));
        }
        if !image_size_is_supported(args.size.as_deref())
            || !optional_argument_is_supported(
                args.quality.as_deref(),
                &["auto", "low", "medium", "high"],
            )
            || !optional_argument_is_supported(
                args.background.as_deref(),
                &["auto", "opaque", "transparent"],
            )
        {
            return Err(mcp_request_error(
                ImagesGenerationFailureKind::InvalidRequest,
                request_id,
            ));
        }
        let body = mcp_request_body(args).map_err(|_| {
            mcp_forwarding_error(&ImagesGenerationFailure::with_request_id(
                ImagesGenerationFailureKind::RequestConstructionFailed,
                request_id.clone(),
            ))
        })?;
        let asset_manager = self.asset_manager.clone().ok_or_else(|| {
            image_asset_error(
                ImageAssetErrorKind::StorageUnavailable,
                request_id.clone(),
                None,
            )
        })?;
        let permit = asset_manager
            .acquire_publication_permit()
            .await
            .map_err(|kind| image_asset_error(kind, request_id.clone(), None))?;
        let asset_root = asset_manager.configured_path();
        let admitted_root =
            tokio::task::spawn_blocking(move || AdmittedAssetRoot::admit(asset_root))
                .await
                .map_err(|_| {
                    image_asset_error(
                        ImageAssetErrorKind::StorageUnavailable,
                        request_id.clone(),
                        None,
                    )
                })?
                .map_err(|kind| image_asset_error(kind, request_id.clone(), None))?;
        let response = self
            .service
            .forward_with_request_id(Bytes::from(body), &HeaderMap::new(), request_id.clone())
            .await
            .map_err(|error| mcp_forwarding_error(&error))?;
        let upstream_status = Some(response.status);
        let fault = self.publication_fault;
        let asset = tokio::task::spawn_blocking(move || {
            // A cancelled MCP future cannot release the shared memory permit
            // while its non-cancellable blocking publication is still running.
            let _permit = permit;
            process_image_response(response.body, &admitted_root, fault)
        })
        .await
        .map_err(|_| {
            image_asset_error(
                ImageAssetErrorKind::WriteFailed,
                request_id.clone(),
                upstream_status,
            )
        })?
        .map_err(|kind| image_asset_error(kind, request_id.clone(), upstream_status))?;
        let text = serde_json::to_string(&asset).map_err(|_| {
            image_asset_error(
                ImageAssetErrorKind::WriteFailed,
                request_id.clone(),
                upstream_status,
            )
        })?;
        self.change_sink.image_assets_changed();
        Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
    }
}

impl ServerHandler for ImageMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListToolsResult, McpError>> + Send + '_ {
        let tool = self.tool.clone();
        async move { Ok(ListToolsResult::with_all_items(vec![(*tool).clone()])) }
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        if request.name.as_ref() != "generate_image" {
            return Err(McpError::invalid_params("unknown tool", None));
        }
        let arguments = request.arguments.ok_or_else(|| {
            mcp_request_error(
                ImagesGenerationFailureKind::InvalidRequest,
                Uuid::new_v4().to_string(),
            )
        })?;
        let args: GenerateImageArgs =
            serde_json::from_value(serde_json::Value::Object(arguments.into_iter().collect()))
                .map_err(|_| {
                    mcp_request_error(
                        ImagesGenerationFailureKind::InvalidRequest,
                        Uuid::new_v4().to_string(),
                    )
                })?;
        self.generate_image(args).await.map(Into::into)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GenerateImageArgs {
    prompt: String,
    size: Option<String>,
    quality: Option<String>,
    background: Option<String>,
}

fn mcp_request_body(args: GenerateImageArgs) -> Result<Vec<u8>, serde_json::Error> {
    let mut body = serde_json::Map::new();
    body.insert("model".to_owned(), json!("gpt-image-2"));
    body.insert("prompt".to_owned(), json!(args.prompt));
    body.insert("n".to_owned(), json!(1));
    body.insert("output_format".to_owned(), json!("png"));
    if let Some(value) = args.size {
        body.insert("size".to_owned(), json!(value));
    }
    if let Some(value) = args.quality {
        body.insert("quality".to_owned(), json!(value));
    }
    if let Some(value) = args.background {
        body.insert("background".to_owned(), json!(value));
    }
    serde_json::to_vec(&body)
}

fn optional_argument_is_supported(value: Option<&str>, supported: &[&str]) -> bool {
    value.is_none_or(|value| supported.contains(&value))
}

fn image_size_is_supported(value: Option<&str>) -> bool {
    let Some(value) = value else {
        return true;
    };
    if value == "auto" {
        return true;
    }
    let Some((width, height)) = value.split_once('x') else {
        return false;
    };
    if width.is_empty()
        || height.is_empty()
        || !width.bytes().all(|byte| byte.is_ascii_digit())
        || !height.bytes().all(|byte| byte.is_ascii_digit())
    {
        return false;
    }
    let (Ok(width), Ok(height)) = (width.parse::<u64>(), height.parse::<u64>()) else {
        return false;
    };
    if width == 0
        || height == 0
        || width % IMAGE_SIZE_MULTIPLE != 0
        || height % IMAGE_SIZE_MULTIPLE != 0
    {
        return false;
    }
    let short_edge = width.min(height);
    let long_edge = width.max(height);
    if long_edge >= MAX_IMAGE_EDGE_EXCLUSIVE
        || short_edge
            .checked_mul(MAX_IMAGE_ASPECT_RATIO)
            .is_none_or(|ratio_limit| long_edge > ratio_limit)
    {
        return false;
    }
    width
        .checked_mul(height)
        .is_some_and(|pixels| (MIN_IMAGE_PIXELS..=MAX_IMAGE_PIXELS).contains(&pixels))
}

fn mcp_forwarding_error(error: &ImagesGenerationFailure) -> McpError {
    McpError::internal_error(error.message(), Some(images_mcp_error_data(error)))
}

fn mcp_request_error(kind: ImagesGenerationFailureKind, request_id: String) -> McpError {
    let failure = ImagesGenerationFailure::with_request_id(kind, request_id);
    McpError::invalid_params(failure.message(), Some(images_mcp_error_data(&failure)))
}

fn image_asset_error(
    kind: ImageAssetErrorKind,
    request_id: String,
    upstream_status: Option<StatusCode>,
) -> McpError {
    let stage = match kind {
        ImageAssetErrorKind::InvalidResponse => ImagesFailureStage::ResponseDecode,
        ImageAssetErrorKind::InvalidBase64
        | ImageAssetErrorKind::InvalidPng
        | ImageAssetErrorKind::TooLarge => ImagesFailureStage::ResultValidation,
        ImageAssetErrorKind::StorageUnavailable | ImageAssetErrorKind::WriteFailed => {
            ImagesFailureStage::AssetStorage
        }
    };
    let failure = ImagesGenerationFailure {
        kind: ImagesGenerationFailureKind::InvalidResponse,
        request_id,
        stage,
        upstream_status,
        category: ImagesUpstreamCategory::UnknownUpstream,
        retryable: false,
        transient_provider_message: None,
    };
    McpError::internal_error(
        kind.message(),
        Some(images_mcp_error_data_with_code(&failure, kind.code())),
    )
}

struct ImagesMcpErrorData<'a> {
    code: &'static str,
    request_id: &'a str,
    stage: &'static str,
    upstream_status: Option<u16>,
    category: &'static str,
    retryable: bool,
}

impl ImagesMcpErrorData<'_> {
    fn into_value(self) -> serde_json::Value {
        let mut data = serde_json::Map::with_capacity(6);
        data.insert("code".to_owned(), self.code.into());
        data.insert("requestId".to_owned(), self.request_id.into());
        data.insert("stage".to_owned(), self.stage.into());
        data.insert(
            "upstreamStatus".to_owned(),
            self.upstream_status
                .map_or(serde_json::Value::Null, Into::into),
        );
        data.insert("category".to_owned(), self.category.into());
        data.insert("retryable".to_owned(), self.retryable.into());
        serde_json::Value::Object(data)
    }
}

fn images_mcp_error_data(error: &ImagesGenerationFailure) -> serde_json::Value {
    images_mcp_error_data_with_code(error, error.kind.code())
}

fn images_mcp_error_data_with_code(
    error: &ImagesGenerationFailure,
    code: &'static str,
) -> serde_json::Value {
    ImagesMcpErrorData {
        code,
        request_id: &error.request_id,
        stage: error.stage.as_str(),
        upstream_status: error.upstream_status.map(|status| status.as_u16()),
        category: error.category.as_str(),
        retryable: error.retryable,
    }
    .into_value()
}

fn generate_image_tool() -> Tool {
    let schema = json!({
        "type": "object",
        "properties": {
            "prompt": { "type": "string", "minLength": 1 },
            "size": {
                "type": "string",
                "pattern": "^(?:auto|[0-9]+x[0-9]+)$",
                "description": "Use `auto` or WIDTHxHEIGHT. Width and height must be positive decimal integers, multiples of 16, and each edge must be less than 3,840 pixels; the long-edge to short-edge ratio must be at most 3:1, and total pixels must be from 655,360 through 8,294,400 inclusive. Common examples include 1024x1024, 1536x1024, 1024x1536, 2048x2048, and 2048x1152. Sizes above 3,686,400 total pixels (2560x1440) are supported but experimental."
            },
            "quality": { "type": "string", "enum": ["auto", "low", "medium", "high"] },
            "background": { "type": "string", "enum": ["auto", "opaque", "transparent"] }
        },
        "required": ["prompt"],
        "additionalProperties": false
    });
    let schema: JsonObject = schema.as_object().cloned().unwrap_or_default();
    Tool::new(
        Cow::Borrowed("generate_image"),
        Cow::Borrowed(
            "Generate one PNG image, save it locally, and return its path and metadata as JSON.",
        ),
        Arc::new(schema),
    )
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    use axum::{
        Router,
        body::{Body, Bytes},
        extract::{Request, State},
        http::{HeaderMap, StatusCode, header},
        response::IntoResponse,
        routing::post,
    };
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use serde_json::{Value, json};
    use sha2::{Digest, Sha256};
    use tempfile::TempDir;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        sync::Semaphore,
    };

    use super::*;
    use crate::{
        domain::{ApiKey, BaseUrl, RouteId, ServiceTierPolicy},
        proxy::{ProxyServerHandle, RouteSnapshot, RoutingSnapshot},
    };

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

    fn valid_png_response() -> Bytes {
        Bytes::from(
            serde_json::to_vec(&json!({
                "data": [{"b64_json": STANDARD.encode(valid_png_fixture())}]
            }))
            .expect("PNG response"),
        )
    }

    fn mcp_adapter(
        service: ImagesGenerationService,
        asset_root: Option<PathBuf>,
    ) -> ImageMcpServer {
        let manager =
            asset_root.map(|root| McpImageAssetManager::new(root, Arc::new(Semaphore::new(1))));
        ImageMcpServer::new(service, manager, Arc::new(NoopImageAssetChangeSink))
    }

    #[derive(Default)]
    struct RecordingImageAssetChangeSink(AtomicUsize);

    impl ImageAssetChangeSink for RecordingImageAssetChangeSink {
        fn image_assets_changed(&self) {
            self.0.fetch_add(1, Ordering::AcqRel);
        }
    }

    fn default_generate_args() -> GenerateImageArgs {
        GenerateImageArgs {
            prompt: "private prompt sentinel".to_owned(),
            size: None,
            quality: None,
            background: None,
        }
    }

    fn asset_error_code(error: &McpError) -> Option<&str> {
        error
            .data
            .as_ref()
            .and_then(|data| data.get("code"))
            .and_then(Value::as_str)
    }

    fn mcp_error_field<'a>(error: &'a McpError, field: &str) -> &'a Value {
        error
            .data
            .as_ref()
            .and_then(|data| data.get(field))
            .unwrap_or_else(|| panic!("missing MCP error field {field}"))
    }

    #[test]
    fn upstream_error_envelopes_are_bounded_and_normalized() {
        let projection = parse_images_error_projection(
            br#"{"error":{"code":"content_policy_violation","message":"  blocked\n\tby\u0000 policy  "},"code":"invalid_request","message":"top"}"#,
        )
        .expect("nested projection");
        assert_eq!(projection.code.as_deref(), Some("content_policy_violation"));
        assert_eq!(projection.message.as_deref(), Some("blocked by policy"));

        let projection = parse_images_error_projection(
            br#"{"error":{"message":"nested"},"code":"invalid_parameter","message":"top"}"#,
        )
        .expect("independent nested precedence");
        assert_eq!(projection.code.as_deref(), Some("invalid_parameter"));
        assert_eq!(projection.message.as_deref(), Some("nested"));

        for invalid in [
            br#"{"code":400,"message":"ignored"}"#.as_slice(),
            br#"{"error":{"code":[],"message":"ignored"}}"#.as_slice(),
            br#"{"code":"invalid_request""#.as_slice(),
        ] {
            assert!(parse_images_error_projection(invalid).is_none());
        }

        let long = "界".repeat(MAX_IMAGES_UPSTREAM_ERROR_MESSAGE_CHARS + 10);
        let normalized = normalize_provider_message(Some(&long)).expect("bounded message");
        assert_eq!(
            normalized.chars().count(),
            MAX_IMAGES_UPSTREAM_ERROR_MESSAGE_CHARS
        );
        assert_eq!(
            bounded_provider_code(Some(&"x".repeat(MAX_IMAGES_UPSTREAM_ERROR_CODE_CHARS + 10)))
                .expect("bounded code")
                .chars()
                .count(),
            MAX_IMAGES_UPSTREAM_ERROR_CODE_CHARS
        );
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "the table enumerates every reviewed exact provider-code alias and retry rule"
    )]
    fn exact_codes_status_fallback_and_retryability_are_closed() {
        let aliases = [
            (
                "content_policy_violation",
                ImagesUpstreamCategory::ContentPolicy,
            ),
            ("content_policy", ImagesUpstreamCategory::ContentPolicy),
            ("safety_violation", ImagesUpstreamCategory::ContentPolicy),
            ("moderation_blocked", ImagesUpstreamCategory::ContentPolicy),
            ("invalid_request", ImagesUpstreamCategory::InvalidRequest),
            (
                "invalid_request_error",
                ImagesUpstreamCategory::InvalidRequest,
            ),
            ("invalid_parameter", ImagesUpstreamCategory::InvalidRequest),
            ("invalid_value", ImagesUpstreamCategory::InvalidRequest),
            ("bad_request", ImagesUpstreamCategory::InvalidRequest),
            ("invalid_api_key", ImagesUpstreamCategory::Authentication),
            (
                "authentication_error",
                ImagesUpstreamCategory::Authentication,
            ),
            ("unauthorized", ImagesUpstreamCategory::Authentication),
            ("permission_denied", ImagesUpstreamCategory::Permission),
            ("access_denied", ImagesUpstreamCategory::Permission),
            ("AccessDenied", ImagesUpstreamCategory::Permission),
            ("forbidden", ImagesUpstreamCategory::Permission),
            ("rate_limit", ImagesUpstreamCategory::RateLimit),
            ("rate_limit_exceeded", ImagesUpstreamCategory::RateLimit),
            ("too_many_requests", ImagesUpstreamCategory::RateLimit),
            ("insufficient_quota", ImagesUpstreamCategory::Quota),
            ("credits_exhausted", ImagesUpstreamCategory::Quota),
            ("billing_hard_limit_reached", ImagesUpstreamCategory::Quota),
            ("server_error", ImagesUpstreamCategory::ServerError),
            ("server_overloaded", ImagesUpstreamCategory::ServerError),
            ("internal_server_error", ImagesUpstreamCategory::ServerError),
            ("service_unavailable", ImagesUpstreamCategory::ServerError),
        ];
        for (code, expected) in aliases {
            assert_eq!(
                classify_upstream_category(Some(code), StatusCode::IM_A_TEAPOT),
                expected
            );
        }
        assert_eq!(
            classify_upstream_category(Some("CONTENT_POLICY"), StatusCode::BAD_REQUEST),
            ImagesUpstreamCategory::UnknownUpstream
        );
        assert_eq!(
            classify_upstream_category(Some("new_provider_code"), StatusCode::TOO_MANY_REQUESTS),
            ImagesUpstreamCategory::UnknownUpstream
        );

        for (status, expected) in [
            (
                StatusCode::BAD_REQUEST,
                ImagesUpstreamCategory::InvalidRequest,
            ),
            (
                StatusCode::UNPROCESSABLE_ENTITY,
                ImagesUpstreamCategory::InvalidRequest,
            ),
            (
                StatusCode::UNAUTHORIZED,
                ImagesUpstreamCategory::Authentication,
            ),
            (StatusCode::FORBIDDEN, ImagesUpstreamCategory::Permission),
            (
                StatusCode::TOO_MANY_REQUESTS,
                ImagesUpstreamCategory::RateLimit,
            ),
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                ImagesUpstreamCategory::ServerError,
            ),
            (
                StatusCode::IM_A_TEAPOT,
                ImagesUpstreamCategory::UnknownUpstream,
            ),
        ] {
            assert_eq!(classify_upstream_category(None, status), expected);
        }
        for status in [
            StatusCode::INTERNAL_SERVER_ERROR,
            StatusCode::BAD_GATEWAY,
            StatusCode::SERVICE_UNAVAILABLE,
            StatusCode::GATEWAY_TIMEOUT,
        ] {
            assert!(images_failure_is_retryable(
                ImagesFailureStage::UpstreamHttpStatus,
                ImagesUpstreamCategory::ServerError,
                Some(status)
            ));
        }
        assert!(!images_failure_is_retryable(
            ImagesFailureStage::UpstreamHttpStatus,
            ImagesUpstreamCategory::ServerError,
            Some(StatusCode::NOT_IMPLEMENTED)
        ));
        assert!(images_failure_is_retryable(
            ImagesFailureStage::UpstreamHttpStatus,
            ImagesUpstreamCategory::RateLimit,
            Some(StatusCode::TOO_MANY_REQUESTS)
        ));
        assert!(!images_failure_is_retryable(
            ImagesFailureStage::UpstreamHttpStatus,
            ImagesUpstreamCategory::Quota,
            Some(StatusCode::TOO_MANY_REQUESTS)
        ));
    }

    fn returned_asset_path(result: &CallToolResult) -> PathBuf {
        let serialized = serde_json::to_value(result).expect("serialized MCP result");
        let text = serialized["content"][0]["text"]
            .as_str()
            .expect("text asset result");
        let asset: Value = serde_json::from_str(text).expect("asset JSON");
        PathBuf::from(asset["path"].as_str().expect("asset path"))
    }

    #[derive(Clone)]
    struct MockImagesUpstream {
        status: StatusCode,
        response: Bytes,
        response_header_delay: Duration,
        calls: Arc<AtomicUsize>,
        in_flight: Arc<AtomicUsize>,
        max_in_flight: Arc<AtomicUsize>,
        captures: Arc<Mutex<Vec<(String, HeaderMap, Bytes)>>>,
    }

    async fn mock_images_handler(
        State(state): State<MockImagesUpstream>,
        request: Request,
    ) -> impl IntoResponse {
        state.calls.fetch_add(1, Ordering::AcqRel);
        let in_flight = state.in_flight.fetch_add(1, Ordering::AcqRel) + 1;
        state.max_in_flight.fetch_max(in_flight, Ordering::AcqRel);
        let path = request.uri().path().to_owned();
        let headers = request.headers().clone();
        let body = axum::body::to_bytes(request.into_body(), 4 * 1024 * 1024)
            .await
            .expect("mock request body");
        state
            .captures
            .lock()
            .expect("capture lock")
            .push((path, headers, body));
        tokio::time::sleep(state.response_header_delay).await;
        state.in_flight.fetch_sub(1, Ordering::AcqRel);
        (state.status, state.response)
    }

    fn route(base_url: &str, key: &str) -> Arc<RouteSnapshot> {
        Arc::new(RouteSnapshot {
            route_id: RouteId::new(),
            name: "Image route".to_owned(),
            base_url: BaseUrl::parse(base_url).expect("base URL"),
            api_key: Arc::new(ApiKey::parse(key).expect("API key")),
            service_tier_policy: ServiceTierPolicy::Passthrough,
            fallback_excluded_models: Arc::new(std::collections::HashSet::new()),
        })
    }

    fn routing(
        images_generation_enabled: bool,
        images_route: Option<Arc<RouteSnapshot>>,
    ) -> RoutingSnapshotStore {
        routing_with_timeout(
            images_generation_enabled,
            images_route,
            Duration::from_mins(10),
        )
    }

    fn routing_with_timeout(
        images_generation_enabled: bool,
        images_route: Option<Arc<RouteSnapshot>>,
        images_generation_timeout: Duration,
    ) -> RoutingSnapshotStore {
        RoutingSnapshotStore::new(RoutingSnapshot {
            active: None,
            participants: Vec::new(),
            enabled: false,
            selection_generation: 0,
            health_generation: 0,
            config_revision: 0,
            images_generation_enabled,
            images_route,
            images_generation_timeout,
        })
    }

    async fn start_mock(
        status: StatusCode,
        response: Bytes,
    ) -> (ProxyServerHandle, MockImagesUpstream) {
        start_mock_with_delay(status, response, Duration::ZERO).await
    }

    async fn start_mock_with_delay(
        status: StatusCode,
        response: Bytes,
        response_header_delay: Duration,
    ) -> (ProxyServerHandle, MockImagesUpstream) {
        let state = MockImagesUpstream {
            status,
            response,
            response_header_delay,
            calls: Arc::new(AtomicUsize::new(0)),
            in_flight: Arc::new(AtomicUsize::new(0)),
            max_in_flight: Arc::new(AtomicUsize::new(0)),
            captures: Arc::new(Mutex::new(Vec::new())),
        };
        let server = ProxyServerHandle::start(
            0,
            Router::new()
                .route("/openai/v1/images/generations", post(mock_images_handler))
                .with_state(state.clone()),
        )
        .await
        .expect("mock upstream");
        (server, state)
    }

    async fn start_manual_peer(
        response: Vec<u8>,
        delay_after_write: Duration,
    ) -> (
        std::net::SocketAddr,
        Arc<AtomicUsize>,
        tokio::task::JoinHandle<()>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("manual loopback listener");
        let address = listener.local_addr().expect("manual listener address");
        let calls = Arc::new(AtomicUsize::new(0));
        let task_calls = Arc::clone(&calls);
        let task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("manual peer accept");
            task_calls.fetch_add(1, Ordering::AcqRel);
            let mut request = vec![0_u8; 4096];
            let _ = stream.read(&mut request).await;
            if !response.is_empty() {
                stream
                    .write_all(&response)
                    .await
                    .expect("manual peer response");
            }
            tokio::time::sleep(delay_after_write).await;
        });
        (address, calls, task)
    }

    #[tokio::test]
    async fn disabled_and_missing_routes_make_zero_upstream_contacts() {
        let (server, mock) = start_mock(StatusCode::OK, Bytes::from_static(b"{}")).await;
        let base_url = format!("http://{}/openai/v1", server.address());
        let body = Bytes::from_static(br#"{"prompt":"private prompt"}"#);

        for (enabled, selected, expected) in [
            (
                false,
                Some(route(&base_url, "image-key")),
                ImagesGenerationFailureKind::Disabled,
            ),
            (true, None, ImagesGenerationFailureKind::RouteNotSelected),
        ] {
            let error = ImagesGenerationService::new(routing(enabled, selected))
                .forward(body.clone(), &HeaderMap::new())
                .await
                .expect_err("local gate");
            assert_eq!(error.kind.code(), expected.code());
        }
        assert_eq!(mock.calls.load(Ordering::Acquire), 0);
        server.shutdown().await;
    }

    #[tokio::test]
    async fn direct_forwarding_uses_one_dedicated_attempt_and_preserves_large_json() {
        let image_data = STANDARD.encode(vec![7_u8; 800_000]);
        assert!(image_data.len() > 1024 * 1024);
        let response = serde_json::to_vec(&json!({
            "created": 1,
            "data": [{"b64_json": image_data}],
            "provider_extension": {"kept": true}
        }))
        .expect("response fixture");
        let (server, mock) = start_mock(StatusCode::OK, Bytes::from(response.clone())).await;
        let selected = route(
            &format!("http://{}/openai/v1", server.address()),
            "selected-image-key",
        );
        let active = route("https://text-route.invalid/v1", "text-key");
        let store = RoutingSnapshotStore::new(RoutingSnapshot {
            active: Some(active.clone()),
            participants: vec![active],
            enabled: true,
            selection_generation: 9,
            health_generation: 0,
            config_revision: 4,
            images_generation_enabled: true,
            images_route: Some(selected),
            images_generation_timeout: Duration::from_mins(10),
        });
        let request_body = Bytes::from_static(
            br#"{"model":"caller-model","prompt":"private","n":3,"extension":true}"#,
        );
        let mut client_headers = HeaderMap::new();
        client_headers.insert(
            header::AUTHORIZATION,
            "Bearer local-token".parse().expect("header"),
        );
        client_headers.insert("x-api-key", "client-key".parse().expect("header"));
        client_headers.insert("x-forwarded-for", "127.0.0.1".parse().expect("header"));
        client_headers.insert("x-client-extension", "keep".parse().expect("header"));

        let result = ImagesGenerationService::new(store)
            .forward(request_body.clone(), &client_headers)
            .await
            .expect("image response");
        assert_eq!(result.body.as_ref(), response);
        assert_eq!(mock.calls.load(Ordering::Acquire), 1);
        {
            let captures = mock.captures.lock().expect("capture lock");
            let (path, headers, body) = captures.first().expect("captured request");
            assert_eq!(path, "/openai/v1/images/generations");
            assert_eq!(body, &request_body);
            assert_eq!(
                headers
                    .get(header::AUTHORIZATION)
                    .and_then(|value| value.to_str().ok()),
                Some("Bearer selected-image-key")
            );
            assert_eq!(
                headers
                    .get(header::ACCEPT_ENCODING)
                    .and_then(|value| value.to_str().ok()),
                Some("identity")
            );
            assert!(headers.get("x-api-key").is_none());
            assert!(headers.get("x-forwarded-for").is_none());
            assert_eq!(
                headers
                    .get("x-client-extension")
                    .and_then(|value| value.to_str().ok()),
                Some("keep")
            );
        }
        server.shutdown().await;
    }

    #[tokio::test]
    async fn changing_the_dedicated_image_route_affects_only_the_next_call() {
        let response = Bytes::from_static(br#"{"data":[{"b64_json":"AQ=="}]}"#);
        let (server, mock) = start_mock(StatusCode::OK, response).await;
        let base_url = format!("http://{}/openai/v1", server.address());
        let first = route(&base_url, "first-image-key");
        let second = route(&base_url, "second-image-key");
        let store = routing(true, Some(first));
        let service = ImagesGenerationService::new(store.clone());
        let body = Bytes::from_static(br#"{"prompt":"private"}"#);

        service
            .forward(body.clone(), &HeaderMap::new())
            .await
            .expect("first image call");
        let current = store.load();
        store.store(Arc::new(RoutingSnapshot {
            active: current.active.clone(),
            participants: current.participants.clone(),
            enabled: current.enabled,
            selection_generation: current.selection_generation,
            health_generation: current.health_generation,
            config_revision: current.config_revision,
            images_generation_enabled: current.images_generation_enabled,
            images_route: Some(second),
            images_generation_timeout: current.images_generation_timeout,
        }));
        service
            .forward(body, &HeaderMap::new())
            .await
            .expect("second image call");

        {
            let captures = mock.captures.lock().expect("capture lock");
            assert_eq!(mock.calls.load(Ordering::Acquire), 2);
            assert_eq!(
                captures[0]
                    .1
                    .get(header::AUTHORIZATION)
                    .and_then(|value| value.to_str().ok()),
                Some("Bearer first-image-key")
            );
            assert_eq!(
                captures[1]
                    .1
                    .get(header::AUTHORIZATION)
                    .and_then(|value| value.to_str().ok()),
                Some("Bearer second-image-key")
            );
        }
        server.shutdown().await;
    }

    #[tokio::test]
    async fn delayed_headers_inside_the_captured_budget_succeed_once() {
        let response = Bytes::from_static(br#"{"data":[{"b64_json":"AQ=="}],"kept":true}"#);
        let (server, mock) =
            start_mock_with_delay(StatusCode::OK, response.clone(), Duration::from_millis(80))
                .await;
        let selected = route(
            &format!("http://{}/openai/v1", server.address()),
            "selected-image-key",
        );

        let result = ImagesGenerationService::new(routing_with_timeout(
            true,
            Some(selected),
            Duration::from_millis(250),
        ))
        .forward(
            Bytes::from_static(br#"{"prompt":"private"}"#),
            &HeaderMap::new(),
        )
        .await
        .expect("slow image response");

        assert_eq!(result.body, response);
        assert_eq!(mock.calls.load(Ordering::Acquire), 1);
        server.shutdown().await;
    }

    #[tokio::test]
    async fn delayed_headers_beyond_the_captured_budget_time_out_once() {
        let (server, mock) = start_mock_with_delay(
            StatusCode::OK,
            Bytes::from_static(br#"{"data":[{"b64_json":"AQ=="}]}"#),
            Duration::from_millis(180),
        )
        .await;
        let selected = route(
            &format!("http://{}/openai/v1", server.address()),
            "selected-image-key",
        );

        let error = ImagesGenerationService::new(routing_with_timeout(
            true,
            Some(selected),
            Duration::from_millis(40),
        ))
        .forward(
            Bytes::from_static(br#"{"prompt":"private"}"#),
            &HeaderMap::new(),
        )
        .await
        .expect_err("header timeout");

        assert_eq!(error.kind.code(), "images_upstream_timeout");
        assert_eq!(mock.calls.load(Ordering::Acquire), 1);
        server.shutdown().await;
    }

    #[tokio::test]
    async fn connection_send_body_timeout_and_body_read_failures_have_distinct_stages_once() {
        let released = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("released loopback listener");
        let released_address = released.local_addr().expect("released address");
        drop(released);
        let connection = ImagesGenerationService::new(routing(
            true,
            Some(route(
                &format!("http://{released_address}/openai/v1"),
                "selected-image-key",
            )),
        ))
        .forward(
            Bytes::from_static(br#"{"prompt":"private"}"#),
            &HeaderMap::new(),
        )
        .await
        .expect_err("connection refusal");
        assert_eq!(connection.stage, ImagesFailureStage::Connection);
        assert!(connection.retryable);
        assert_eq!(connection.upstream_status, None);

        let (send_address, send_calls, send_task) =
            start_manual_peer(Vec::new(), Duration::ZERO).await;
        let send = ImagesGenerationService::new(routing(
            true,
            Some(route(
                &format!("http://{send_address}/openai/v1"),
                "selected-image-key",
            )),
        ))
        .forward(
            Bytes::from_static(br#"{"prompt":"private"}"#),
            &HeaderMap::new(),
        )
        .await
        .expect_err("pre-header disconnect");
        send_task.await.expect("send peer task");
        assert_eq!(send.stage, ImagesFailureStage::RequestSend);
        assert!(send.retryable);
        assert_eq!(send_calls.load(Ordering::Acquire), 1);

        let incomplete =
            b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 20\r\n\r\n{}"
                .to_vec();
        let (read_address, read_calls, read_task) =
            start_manual_peer(incomplete, Duration::ZERO).await;
        let read = ImagesGenerationService::new(routing(
            true,
            Some(route(
                &format!("http://{read_address}/openai/v1"),
                "selected-image-key",
            )),
        ))
        .forward(
            Bytes::from_static(br#"{"prompt":"private"}"#),
            &HeaderMap::new(),
        )
        .await
        .expect_err("incomplete response body");
        read_task.await.expect("read peer task");
        assert_eq!(read.stage, ImagesFailureStage::ResponseBodyRead);
        assert_eq!(read.upstream_status, Some(StatusCode::OK));
        assert!(read.retryable);
        assert_eq!(read_calls.load(Ordering::Acquire), 1);

        let stalled =
            b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 20\r\n\r\n{}"
                .to_vec();
        let (timeout_address, timeout_calls, timeout_task) =
            start_manual_peer(stalled, Duration::from_millis(150)).await;
        let timeout = ImagesGenerationService::new(routing(
            true,
            Some(route(
                &format!("http://{timeout_address}/openai/v1"),
                "selected-image-key",
            )),
        ))
        .with_body_timeout(Duration::from_millis(30))
        .forward(
            Bytes::from_static(br#"{"prompt":"private"}"#),
            &HeaderMap::new(),
        )
        .await
        .expect_err("stalled response body");
        timeout_task.await.expect("timeout peer task");
        assert_eq!(timeout.stage, ImagesFailureStage::UpstreamTimeout);
        assert_eq!(timeout.upstream_status, Some(StatusCode::OK));
        assert!(timeout.retryable);
        assert_eq!(timeout_calls.load(Ordering::Acquire), 1);
    }

    #[tokio::test]
    async fn incompatible_selected_route_is_safe_and_never_retried() {
        let sentinel = "PRIVATE_UPSTREAM_ERROR_SENTINEL";
        let (server, mock) = start_mock(
            StatusCode::NOT_FOUND,
            Bytes::from(json!({"error": sentinel}).to_string()),
        )
        .await;
        let selected = route(
            &format!("http://{}/openai/v1", server.address()),
            "selected-image-key",
        );
        let error = ImagesGenerationService::new(routing(true, Some(selected)))
            .forward(
                Bytes::from_static(br#"{"prompt":"private"}"#),
                &HeaderMap::new(),
            )
            .await
            .expect_err("safe upstream failure");
        assert_eq!(error.kind.code(), "images_upstream_http_status");
        assert_eq!(error.stage, ImagesFailureStage::UpstreamHttpStatus);
        assert_eq!(error.upstream_status, Some(StatusCode::NOT_FOUND));
        assert_eq!(error.category, ImagesUpstreamCategory::UnknownUpstream);
        assert!(!error.retryable);
        assert!(!error.kind.message().contains(sentinel));
        assert_eq!(mock.calls.load(Ordering::Acquire), 1);
        server.shutdown().await;
    }

    #[tokio::test]
    #[allow(
        clippy::too_many_lines,
        reason = "the table asserts the complete category and safe MCP field contract"
    )]
    async fn mcp_upstream_http_errors_expose_only_closed_safe_details_once() {
        let cases = [
            (
                StatusCode::BAD_REQUEST,
                json!({"error":{"code":"content_policy_violation","message":"RAW_POLICY_SENTINEL"}}),
                ImagesUpstreamCategory::ContentPolicy,
                false,
                "content policy",
            ),
            (
                StatusCode::BAD_REQUEST,
                json!({"code":"invalid_parameter","message":"RAW_INVALID_SENTINEL"}),
                ImagesUpstreamCategory::InvalidRequest,
                false,
                "invalid",
            ),
            (
                StatusCode::UNAUTHORIZED,
                json!({"error":{"code":"invalid_api_key","message":"RAW_AUTH_SENTINEL"}}),
                ImagesUpstreamCategory::Authentication,
                false,
                "authenticate",
            ),
            (
                StatusCode::FORBIDDEN,
                json!({"error":{"code":"permission_denied","message":"RAW_PERMISSION_SENTINEL"}}),
                ImagesUpstreamCategory::Permission,
                false,
                "permission",
            ),
            (
                StatusCode::TOO_MANY_REQUESTS,
                json!({"error":{"code":"rate_limit_exceeded","message":"RAW_RATE_SENTINEL"}}),
                ImagesUpstreamCategory::RateLimit,
                true,
                "rate limiting",
            ),
            (
                StatusCode::TOO_MANY_REQUESTS,
                json!({"error":{"code":"insufficient_quota","message":"RAW_QUOTA_SENTINEL"}}),
                ImagesUpstreamCategory::Quota,
                false,
                "quota",
            ),
            (
                StatusCode::SERVICE_UNAVAILABLE,
                json!({"error":{"code":"server_error","message":"RAW_SERVER_SENTINEL"}}),
                ImagesUpstreamCategory::ServerError,
                true,
                "server error",
            ),
            (
                StatusCode::NOT_IMPLEMENTED,
                json!({"message":"RAW_STATUS_FALLBACK_SENTINEL"}),
                ImagesUpstreamCategory::ServerError,
                false,
                "server error",
            ),
        ];

        for (status, response, category, retryable, message_fragment) in cases {
            let raw = response.to_string();
            let (server, mock) = start_mock(status, Bytes::from(raw.clone())).await;
            let selected = route(
                &format!("http://{}/openai/v1", server.address()),
                "selected-image-key",
            );
            let temporary = TempDir::new().expect("temporary app data");
            let error = mcp_adapter(
                ImagesGenerationService::new(routing(true, Some(selected))),
                Some(temporary.path().join("mcp-images")),
            )
            .generate_image(default_generate_args())
            .await
            .expect_err("MCP upstream status error");

            assert_eq!(
                mcp_error_field(&error, "code"),
                "images_upstream_http_status"
            );
            assert_eq!(mcp_error_field(&error, "stage"), "upstream_http_status");
            assert_eq!(mcp_error_field(&error, "upstreamStatus"), status.as_u16());
            assert_eq!(mcp_error_field(&error, "category"), category.as_str());
            assert_eq!(mcp_error_field(&error, "retryable"), retryable);
            Uuid::parse_str(
                mcp_error_field(&error, "requestId")
                    .as_str()
                    .expect("local request ID"),
            )
            .expect("UUID request ID");
            assert!(error.message.contains(message_fragment));
            let serialized = serde_json::to_string(&error).expect("serialized safe error");
            for forbidden in [
                raw.as_str(),
                "content_policy_violation",
                "invalid_parameter",
                "invalid_api_key",
                "permission_denied",
                "rate_limit_exceeded",
                "insufficient_quota",
                "RAW_",
            ] {
                assert!(!serialized.contains(forbidden), "leaked {forbidden}");
            }
            assert_eq!(mock.calls.load(Ordering::Acquire), 1);
            server.shutdown().await;
        }
    }

    #[tokio::test]
    async fn unknown_upstream_message_is_current_response_only_normalized_and_bounded() {
        let provider_message = format!(
            "  benign\nprovider\u{0} detail {}",
            "界".repeat(MAX_IMAGES_UPSTREAM_ERROR_MESSAGE_CHARS + 20)
        );
        let (server, mock) = start_mock(
            StatusCode::BAD_REQUEST,
            Bytes::from(
                json!({
                    "error": {
                        "code": "new_provider_code",
                        "message": provider_message,
                        "request_id": "PROVIDER_REQUEST_ID_SENTINEL"
                    },
                    "arbitrary": "ARBITRARY_FIELD_SENTINEL"
                })
                .to_string(),
            ),
        )
        .await;
        let selected = route(
            &format!("http://{}/openai/v1", server.address()),
            "selected-image-key",
        );
        let temporary = TempDir::new().expect("temporary app data");
        let error = mcp_adapter(
            ImagesGenerationService::new(routing(true, Some(selected))),
            Some(temporary.path().join("mcp-images")),
        )
        .generate_image(default_generate_args())
        .await
        .expect_err("unknown provider code");

        assert_eq!(mcp_error_field(&error, "category"), "unknown_upstream");
        assert_eq!(mcp_error_field(&error, "retryable"), false);
        assert!(error.message.starts_with(
            "The image provider returned an unrecognized error. benign provider detail"
        ));
        assert!(!error.message.contains('\n'));
        assert!(error.message.chars().count() <= 298);
        let data = serde_json::to_string(error.data.as_ref().expect("safe data"))
            .expect("serialized MCP data");
        for forbidden in [
            "new_provider_code",
            "benign provider detail",
            "PROVIDER_REQUEST_ID_SENTINEL",
            "ARBITRARY_FIELD_SENTINEL",
        ] {
            assert!(!data.contains(forbidden), "data leaked {forbidden}");
        }
        let serialized = serde_json::to_string(&error).expect("serialized MCP error");
        assert!(!serialized.contains("PROVIDER_REQUEST_ID_SENTINEL"));
        assert!(!serialized.contains("ARBITRARY_FIELD_SENTINEL"));
        assert_eq!(mock.calls.load(Ordering::Acquire), 1);
        server.shutdown().await;
    }

    #[tokio::test]
    async fn unknown_provider_message_cannot_echo_prompt_or_route_key() {
        let echoed = "private prompt sentinel selected-image-key";
        let (server, mock) = start_mock(
            StatusCode::IM_A_TEAPOT,
            Bytes::from(
                json!({
                    "error": {"code": "new_provider_code", "message": echoed}
                })
                .to_string(),
            ),
        )
        .await;
        let selected = route(
            &format!("http://{}/openai/v1", server.address()),
            "selected-image-key",
        );
        let temporary = TempDir::new().expect("temporary app data");
        let error = mcp_adapter(
            ImagesGenerationService::new(routing(true, Some(selected))),
            Some(temporary.path().join("mcp-images")),
        )
        .generate_image(default_generate_args())
        .await
        .expect_err("guarded provider echo");

        assert_eq!(
            error.message,
            "The image provider returned an unrecognized error."
        );
        let serialized = serde_json::to_string(&error).expect("serialized safe error");
        assert!(!serialized.contains(echoed));
        assert!(!serialized.contains("private prompt sentinel"));
        assert!(!serialized.contains("selected-image-key"));
        assert_eq!(mock.calls.load(Ordering::Acquire), 1);
        server.shutdown().await;
    }

    #[tokio::test]
    async fn malformed_empty_wrong_typed_and_oversized_error_bodies_stay_bounded() {
        let bodies = [
            Bytes::new(),
            Bytes::from_static(br#"{"error":{"code":400,"message":"private"}}"#),
            Bytes::from_static(br#"{"error":"malformed""#),
            Bytes::from(vec![b'x'; MAX_IMAGES_UPSTREAM_ERROR_BODY_BYTES + 1]),
        ];
        for body in bodies {
            let (server, mock) = start_mock(StatusCode::BAD_REQUEST, body).await;
            let selected = route(
                &format!("http://{}/openai/v1", server.address()),
                "selected-image-key",
            );
            let error = ImagesGenerationService::new(routing(true, Some(selected)))
                .forward(
                    Bytes::from_static(br#"{"prompt":"private"}"#),
                    &HeaderMap::new(),
                )
                .await
                .expect_err("bounded HTTP status failure");
            assert_eq!(error.stage, ImagesFailureStage::UpstreamHttpStatus);
            assert_eq!(error.category, ImagesUpstreamCategory::InvalidRequest);
            assert_eq!(error.transient_provider_message, None);
            assert_eq!(mock.calls.load(Ordering::Acquire), 1);
            server.shutdown().await;
        }
    }

    #[tokio::test]
    async fn unsupported_content_encoding_is_safe_for_success_and_error_statuses() {
        for (status, expected_stage, expected_category) in [
            (
                StatusCode::OK,
                ImagesFailureStage::ResponseDecode,
                ImagesUpstreamCategory::UnknownUpstream,
            ),
            (
                StatusCode::BAD_REQUEST,
                ImagesFailureStage::UpstreamHttpStatus,
                ImagesUpstreamCategory::InvalidRequest,
            ),
        ] {
            let calls = Arc::new(AtomicUsize::new(0));
            let handler_calls = Arc::clone(&calls);
            let server = ProxyServerHandle::start(
                0,
                Router::new().route(
                    "/openai/v1/images/generations",
                    post(move || {
                        let calls = Arc::clone(&handler_calls);
                        async move {
                            calls.fetch_add(1, Ordering::AcqRel);
                            axum::response::Response::builder()
                                .status(status)
                                .header(header::CONTENT_ENCODING, "private-unsupported")
                                .body(Body::from(
                                    br#"{"error":{"message":"ENCODING_BODY_SENTINEL"}}"#.as_slice(),
                                ))
                                .expect("unsupported encoding response")
                        }
                    }),
                ),
            )
            .await
            .expect("unsupported encoding upstream");
            let selected = route(
                &format!("http://{}/openai/v1", server.address()),
                "selected-image-key",
            );
            let error = ImagesGenerationService::new(routing(true, Some(selected)))
                .forward(
                    Bytes::from_static(br#"{"prompt":"private"}"#),
                    &HeaderMap::new(),
                )
                .await
                .expect_err("unsupported encoding");
            assert_eq!(error.stage, expected_stage);
            assert_eq!(error.upstream_status, Some(status));
            assert_eq!(error.category, expected_category);
            assert_eq!(error.transient_provider_message, None);
            assert_eq!(calls.load(Ordering::Acquire), 1);
            server.shutdown().await;
        }
    }

    #[tokio::test]
    #[allow(
        clippy::too_many_lines,
        reason = "the success contract test keeps payload, publication, and sink assertions together"
    )]
    async fn mcp_adapter_fixes_payload_and_returns_one_local_png_text_result() {
        let png = valid_png_fixture();
        let image_data = STANDARD.encode(&png);
        let response = serde_json::to_vec(&json!({"data": [{"b64_json": image_data.clone()}]}))
            .expect("response fixture");
        let (server, mock) = start_mock(StatusCode::OK, Bytes::from(response)).await;
        let selected = route(
            &format!("http://{}/openai/v1", server.address()),
            "selected-image-key",
        );
        let temporary = TempDir::new().expect("temporary app data");
        let asset_root = temporary.path().join("mcp-images");
        let change_sink = Arc::new(RecordingImageAssetChangeSink::default());
        let adapter = ImageMcpServer::new(
            ImagesGenerationService::new(routing(true, Some(selected))),
            Some(McpImageAssetManager::new(
                asset_root.clone(),
                Arc::new(Semaphore::new(1)),
            )),
            change_sink.clone(),
        );
        let result = adapter
            .generate_image(GenerateImageArgs {
                prompt: "line one\nline two".to_owned(),
                size: Some("1536x1024".to_owned()),
                quality: Some("high".to_owned()),
                background: Some("transparent".to_owned()),
            })
            .await
            .expect("MCP image result");
        let result: Value = serde_json::to_value(result).expect("serialized MCP result");
        assert_eq!(result["content"].as_array().map(Vec::len), Some(1));
        assert_eq!(result["content"][0]["type"], "text");
        let text = result["content"][0]["text"]
            .as_str()
            .expect("text JSON result");
        let asset: Value = serde_json::from_str(text).expect("asset JSON");
        assert_eq!(asset.as_object().map(serde_json::Map::len), Some(8));
        assert_eq!(asset["status"], "success");
        assert_eq!(asset["mimeType"], "image/png");
        assert_eq!(asset["width"], 1);
        assert_eq!(asset["height"], 1);
        assert_eq!(asset["bytes"], png.len());
        assert_eq!(asset["sha256"], hex::encode(Sha256::digest(&png)));
        let asset_id = asset["assetId"].as_str().expect("asset ID");
        Uuid::parse_str(asset_id).expect("UUID asset ID");
        let path = PathBuf::from(asset["path"].as_str().expect("asset path"));
        assert!(path.is_absolute());
        let expected_file_name = format!("{asset_id}.png");
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some(expected_file_name.as_str())
        );
        let canonical_root = asset_root.canonicalize().expect("canonical root");
        assert_eq!(path.parent(), Some(canonical_root.as_path()));
        assert_eq!(std::fs::read(&path).expect("published PNG"), png);
        assert_eq!(
            std::fs::metadata(&asset_root)
                .expect("asset root metadata")
                .permissions()
                .mode()
                & 0o7777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(&path)
                .expect("asset metadata")
                .permissions()
                .mode()
                & 0o7777,
            0o600
        );
        let expected_text = format!(
            r#"{{"status":"success","path":{},"mimeType":"image/png","width":1,"height":1,"bytes":{},"sha256":"{}","assetId":"{}"}}"#,
            serde_json::to_string(path.to_str().expect("UTF-8 path")).expect("JSON path"),
            png.len(),
            hex::encode(Sha256::digest(&png)),
            asset_id,
        );
        assert_eq!(text, expected_text);
        assert!(result.get("structuredContent").is_none());
        assert_eq!(change_sink.0.load(Ordering::Acquire), 1);
        let serialized = serde_json::to_string(&result).expect("serialized MCP result");
        for forbidden in [
            "\"type\":\"image\"",
            image_data.as_str(),
            "data:",
            "![",
            "https://",
            "http://",
        ] {
            assert!(!serialized.contains(forbidden), "leaked {forbidden}");
        }

        {
            let captures = mock.captures.lock().expect("capture lock");
            let request: Value = serde_json::from_slice(&captures[0].2).expect("captured JSON");
            assert_eq!(request["model"], "gpt-image-2");
            assert_eq!(request["n"], 1);
            assert_eq!(request["output_format"], "png");
            assert!(request.get("response_format").is_none());
            assert_eq!(request["size"], "1536x1024");
            assert_eq!(request["quality"], "high");
            assert_eq!(request["background"], "transparent");
            assert_eq!(mock.calls.load(Ordering::Acquire), 1);
        }
        server.shutdown().await;
    }

    #[tokio::test]
    async fn mcp_adapter_requires_safe_storage_before_upstream_contact() {
        let response = valid_png_response();
        let (server, mock) = start_mock(StatusCode::OK, response).await;
        let selected = route(
            &format!("http://{}/openai/v1", server.address()),
            "selected-image-key",
        );
        let service = ImagesGenerationService::new(routing(true, Some(selected)));

        let missing = mcp_adapter(service.clone(), None)
            .generate_image(default_generate_args())
            .await
            .expect_err("missing storage config");
        assert_eq!(
            asset_error_code(&missing),
            Some("image_asset_storage_unavailable")
        );
        assert_eq!(mcp_error_field(&missing, "stage"), "asset_storage");
        assert_eq!(mcp_error_field(&missing, "upstreamStatus"), &Value::Null);
        assert_eq!(mcp_error_field(&missing, "category"), "unknown_upstream");
        assert_eq!(mcp_error_field(&missing, "retryable"), false);

        let temporary = TempDir::new().expect("temporary app data");
        let non_directory = temporary.path().join("not-a-directory");
        std::fs::write(&non_directory, b"not a directory").expect("non-directory root");
        let real_root = temporary.path().join("real-root");
        std::fs::create_dir(&real_root).expect("real root");
        let linked_root = temporary.path().join("linked-root");
        std::os::unix::fs::symlink(&real_root, &linked_root).expect("root symlink");
        for unsafe_root in [
            PathBuf::from("relative/mcp-images"),
            temporary
                .path()
                .join("unsafe")
                .join("..")
                .join("mcp-images"),
            non_directory,
            linked_root,
        ] {
            let error = mcp_adapter(service.clone(), Some(unsafe_root))
                .generate_image(default_generate_args())
                .await
                .expect_err("unsafe storage root");
            assert_eq!(
                asset_error_code(&error),
                Some("image_asset_storage_unavailable")
            );
        }
        assert_eq!(mock.calls.load(Ordering::Acquire), 0);
        server.shutdown().await;
    }

    #[tokio::test]
    async fn mcp_adapter_rejects_invalid_image_results_without_publishing_files() {
        let valid_png = valid_png_fixture();
        let mut truncated_png = valid_png.clone();
        truncated_png.truncate(truncated_png.len() - 8);
        let cases = [
            (
                "not canonical base64".to_owned(),
                "image_result_invalid_base64",
            ),
            (STANDARD.encode(b"not a PNG"), "image_result_invalid_png"),
            (STANDARD.encode(truncated_png), "image_result_invalid_png"),
        ];

        for (encoded, expected_code) in cases {
            let response = Bytes::from(
                serde_json::to_vec(&json!({"data":[{"b64_json": encoded}]}))
                    .expect("image response"),
            );
            let (server, mock) = start_mock(StatusCode::OK, response).await;
            let selected = route(
                &format!("http://{}/openai/v1", server.address()),
                "selected-image-key",
            );
            let temporary = TempDir::new().expect("temporary app data");
            let asset_root = temporary.path().join("mcp-images");
            let error = mcp_adapter(
                ImagesGenerationService::new(routing(true, Some(selected))),
                Some(asset_root.clone()),
            )
            .generate_image(default_generate_args())
            .await
            .expect_err("invalid image result");
            assert_eq!(asset_error_code(&error), Some(expected_code));
            assert_eq!(mcp_error_field(&error, "stage"), "result_validation");
            assert_eq!(mcp_error_field(&error, "upstreamStatus"), 200);
            assert_eq!(mcp_error_field(&error, "category"), "unknown_upstream");
            assert_eq!(mcp_error_field(&error, "retryable"), false);
            assert_eq!(mock.calls.load(Ordering::Acquire), 1);
            assert_eq!(
                std::fs::read_dir(&asset_root).expect("asset root").count(),
                0
            );
            server.shutdown().await;
        }
    }

    #[tokio::test]
    async fn mcp_response_json_decode_failure_is_distinct_from_result_validation() {
        let (server, mock) = start_mock(
            StatusCode::OK,
            Bytes::from_static(br#"{"data":[{"b64_json":"unterminated"}"#),
        )
        .await;
        let selected = route(
            &format!("http://{}/openai/v1", server.address()),
            "selected-image-key",
        );
        let temporary = TempDir::new().expect("temporary app data");
        let error = mcp_adapter(
            ImagesGenerationService::new(routing(true, Some(selected))),
            Some(temporary.path().join("mcp-images")),
        )
        .generate_image(default_generate_args())
        .await
        .expect_err("invalid response JSON");

        assert_eq!(asset_error_code(&error), Some("images_response_invalid"));
        assert_eq!(mcp_error_field(&error, "stage"), "response_decode");
        assert_eq!(mcp_error_field(&error, "upstreamStatus"), 200);
        assert_eq!(mcp_error_field(&error, "category"), "unknown_upstream");
        assert_eq!(mcp_error_field(&error, "retryable"), false);
        assert_eq!(mock.calls.load(Ordering::Acquire), 1);
        server.shutdown().await;
    }

    #[tokio::test]
    async fn mcp_syntactically_valid_missing_or_unusable_base64_is_result_validation() {
        for response in [
            json!({}),
            json!({"data": []}),
            json!({"data": [{"b64_json": null}]}),
            json!({"data": [{"b64_json": 42}]}),
        ] {
            let (server, mock) =
                start_mock(StatusCode::OK, Bytes::from(response.to_string())).await;
            let selected = route(
                &format!("http://{}/openai/v1", server.address()),
                "selected-image-key",
            );
            let temporary = TempDir::new().expect("temporary app data");
            let error = mcp_adapter(
                ImagesGenerationService::new(routing(true, Some(selected))),
                Some(temporary.path().join("mcp-images")),
            )
            .generate_image(default_generate_args())
            .await
            .expect_err("missing or unusable Base64");

            assert_eq!(
                asset_error_code(&error),
                Some("image_result_invalid_base64")
            );
            assert_eq!(mcp_error_field(&error, "stage"), "result_validation");
            assert_eq!(mcp_error_field(&error, "upstreamStatus"), 200);
            assert_eq!(mcp_error_field(&error, "retryable"), false);
            assert_eq!(mock.calls.load(Ordering::Acquire), 1);
            server.shutdown().await;
        }
    }

    #[tokio::test]
    async fn mcp_adapter_publication_and_cleanup_failures_are_safe() {
        let png_base64 = STANDARD.encode(valid_png_fixture());
        let response = Bytes::from(
            serde_json::to_vec(&json!({"data":[{"b64_json": png_base64.clone()}]}))
                .expect("PNG response"),
        );
        let (server, mock) = start_mock(StatusCode::OK, response).await;
        let selected = route(
            &format!("http://{}/openai/v1", server.address()),
            "selected-image-key",
        );
        let service = ImagesGenerationService::new(routing(true, Some(selected)));

        let temporary = TempDir::new().expect("temporary app data");
        let asset_root = temporary.path().join("mcp-images");
        let error = mcp_adapter(service.clone(), Some(asset_root.clone()))
            .with_publication_fault(PublicationFault::at(asset::PublicationStage::AfterLink))
            .generate_image(default_generate_args())
            .await
            .expect_err("publication failure");
        assert_eq!(asset_error_code(&error), Some("image_asset_write_failed"));
        assert_eq!(error.message, "The generated image could not be saved.");
        assert_eq!(mcp_error_field(&error, "stage"), "asset_storage");
        assert_eq!(mcp_error_field(&error, "upstreamStatus"), 200);
        assert_eq!(mcp_error_field(&error, "retryable"), false);
        assert_eq!(
            std::fs::read_dir(&asset_root).expect("asset root").count(),
            0
        );

        let cleanup_temporary = TempDir::new().expect("cleanup app data");
        let cleanup_root = cleanup_temporary.path().join("mcp-images");
        let cleanup_error = mcp_adapter(service, Some(cleanup_root.clone()))
            .with_publication_fault(PublicationFault::with_cleanup_failure(
                asset::PublicationStage::AfterCreate,
            ))
            .generate_image(default_generate_args())
            .await
            .expect_err("cleanup operation failure");
        assert_eq!(
            asset_error_code(&cleanup_error),
            Some("image_asset_write_failed")
        );
        let serialized = serde_json::to_string(&cleanup_error).expect("serialized safe error");
        for forbidden in [
            "private prompt sentinel",
            png_base64.as_str(),
            cleanup_root.to_string_lossy().as_ref(),
            "assetId",
            ".png",
            ".tmp",
            "Operation not permitted",
        ] {
            assert!(!serialized.contains(forbidden), "leaked {forbidden}");
        }
        let orphan = std::fs::read_dir(&cleanup_root)
            .expect("cleanup root")
            .next()
            .expect("private orphan")
            .expect("orphan entry")
            .path();
        assert_eq!(
            std::fs::metadata(orphan)
                .expect("orphan metadata")
                .permissions()
                .mode()
                & 0o7777,
            0o600
        );
        assert_eq!(mock.calls.load(Ordering::Acquire), 2);
        server.shutdown().await;
    }

    #[tokio::test]
    async fn mcp_adapter_shared_permit_serializes_distinct_sessions() {
        let response = valid_png_response();
        let (server, mock) =
            start_mock_with_delay(StatusCode::OK, response, Duration::from_millis(80)).await;
        let selected = route(
            &format!("http://{}/openai/v1", server.address()),
            "selected-image-key",
        );
        let service = ImagesGenerationService::new(routing(true, Some(selected)));
        let temporary = TempDir::new().expect("temporary app data");
        let asset_root = temporary.path().join("mcp-images");
        let permit = Arc::new(Semaphore::new(1));
        let manager = McpImageAssetManager::new(asset_root.clone(), permit);
        let first = ImageMcpServer::new(
            service.clone(),
            Some(manager.clone()),
            Arc::new(NoopImageAssetChangeSink),
        );
        let second =
            ImageMcpServer::new(service, Some(manager), Arc::new(NoopImageAssetChangeSink));

        let (first_result, second_result) = tokio::join!(
            first.generate_image(default_generate_args()),
            second.generate_image(default_generate_args()),
        );
        let first_result = first_result.expect("first MCP asset");
        let second_result = second_result.expect("second MCP asset");
        let first_path = returned_asset_path(&first_result);
        let second_path = returned_asset_path(&second_result);
        assert_ne!(first_path, second_path);
        assert!(first_path.is_file());
        assert!(second_path.is_file());
        assert_eq!(mock.calls.load(Ordering::Acquire), 2);
        assert_eq!(mock.max_in_flight.load(Ordering::Acquire), 1);
        server.shutdown().await;
    }

    #[tokio::test]
    async fn mcp_adapter_cancellation_keeps_permit_until_blocking_publication_finishes() {
        let response = valid_png_response();
        let (server, mock) = start_mock(StatusCode::OK, response).await;
        let selected = route(
            &format!("http://{}/openai/v1", server.address()),
            "selected-image-key",
        );
        let service = ImagesGenerationService::new(routing(true, Some(selected)));
        let temporary = TempDir::new().expect("temporary app data");
        let asset_root = temporary.path().join("mcp-images");
        let permit = Arc::new(Semaphore::new(1));
        let manager = McpImageAssetManager::new(asset_root.clone(), permit);
        let first = ImageMcpServer::new(
            service.clone(),
            Some(manager.clone()),
            Arc::new(NoopImageAssetChangeSink),
        )
        .with_publication_fault(PublicationFault::with_delay(
            asset::PublicationStage::AfterCreate,
            Duration::from_millis(250),
        ));
        let second =
            ImageMcpServer::new(service, Some(manager), Arc::new(NoopImageAssetChangeSink));

        let first_call =
            tokio::spawn(async move { first.generate_image(default_generate_args()).await });
        for _ in 0..500 {
            let entry_exists = std::fs::read_dir(&asset_root)
                .ok()
                .is_some_and(|mut entries| entries.next().is_some());
            if entry_exists {
                break;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        assert_eq!(mock.calls.load(Ordering::Acquire), 1);
        assert!(
            std::fs::read_dir(&asset_root)
                .expect("asset root")
                .next()
                .is_some(),
            "blocking publication did not reach the injected pause"
        );

        first_call.abort();
        assert!(
            first_call
                .await
                .expect_err("cancelled first MCP call")
                .is_cancelled()
        );
        let second_call =
            tokio::spawn(async move { second.generate_image(default_generate_args()).await });
        tokio::time::sleep(Duration::from_millis(40)).await;
        assert_eq!(
            mock.calls.load(Ordering::Acquire),
            1,
            "cancelled caller released the permit before blocking publication ended"
        );

        second_call
            .await
            .expect("second MCP task")
            .expect("second MCP asset");
        assert_eq!(mock.calls.load(Ordering::Acquire), 2);
        assert_eq!(mock.max_in_flight.load(Ordering::Acquire), 1);
        assert_eq!(
            std::fs::read_dir(asset_root).expect("asset root").count(),
            2
        );
        server.shutdown().await;
    }

    #[tokio::test]
    async fn mcp_adapter_rejects_root_replacement_after_upstream_wait() {
        let response = valid_png_response();
        let (server, mock) =
            start_mock_with_delay(StatusCode::OK, response, Duration::from_millis(150)).await;
        let selected = route(
            &format!("http://{}/openai/v1", server.address()),
            "selected-image-key",
        );
        let temporary = TempDir::new().expect("temporary app data");
        let asset_root = temporary.path().join("mcp-images");
        let displaced_root = temporary.path().join("displaced-mcp-images");
        let adapter = mcp_adapter(
            ImagesGenerationService::new(routing(true, Some(selected))),
            Some(asset_root.clone()),
        );
        let call =
            tokio::spawn(async move { adapter.generate_image(default_generate_args()).await });
        for _ in 0..200 {
            if mock.calls.load(Ordering::Acquire) == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        assert_eq!(mock.calls.load(Ordering::Acquire), 1);
        std::fs::rename(&asset_root, &displaced_root).expect("displace admitted root");
        std::fs::create_dir(&asset_root).expect("replacement root");
        std::fs::set_permissions(&asset_root, std::fs::Permissions::from_mode(0o700))
            .expect("private replacement root");

        let error = call
            .await
            .expect("MCP call task")
            .expect_err("replaced root");
        assert_eq!(
            asset_error_code(&error),
            Some("image_asset_storage_unavailable")
        );
        assert_eq!(
            std::fs::read_dir(&asset_root)
                .expect("replacement root")
                .count(),
            0
        );
        assert_eq!(
            std::fs::read_dir(&displaced_root)
                .expect("displaced root")
                .count(),
            0
        );
        server.shutdown().await;
    }

    #[tokio::test]
    async fn mcp_adapter_forwards_omitted_auto_and_arbitrary_supported_sizes() {
        let response = valid_png_response();
        let (server, mock) = start_mock(StatusCode::OK, response).await;
        let selected = route(
            &format!("http://{}/openai/v1", server.address()),
            "selected-image-key",
        );
        let temporary = TempDir::new().expect("temporary app data");
        let adapter = mcp_adapter(
            ImagesGenerationService::new(routing(true, Some(selected))),
            Some(temporary.path().join("mcp-images")),
        );
        let supported_sizes = [
            None,
            Some("auto"),
            Some("1024x1024"),
            Some("1536x1024"),
            Some("1024x1536"),
            Some("2048x2048"),
            Some("2048x1152"),
            Some("1536x864"),
            Some("1024x640"),
            Some("1920x640"),
            Some("640x1920"),
            Some("3824x2160"),
            Some("2160x3824"),
        ];

        for size in supported_sizes.iter().copied() {
            adapter
                .generate_image(GenerateImageArgs {
                    prompt: "private".to_owned(),
                    size: size.map(str::to_owned),
                    quality: None,
                    background: None,
                })
                .await
                .unwrap_or_else(|error| panic!("supported size {size:?}: {error}"));
        }

        assert_eq!(mock.calls.load(Ordering::Acquire), supported_sizes.len());
        {
            let captures = mock.captures.lock().expect("capture lock");
            for ((_, _, body), expected_size) in
                captures.iter().zip(supported_sizes.iter().copied())
            {
                let request: Value = serde_json::from_slice(body).expect("captured JSON");
                assert_eq!(request["model"], "gpt-image-2");
                assert_eq!(request["n"], 1);
                assert_eq!(request["output_format"], "png");
                match expected_size {
                    Some(size) => assert_eq!(request["size"], size),
                    None => assert!(request.get("size").is_none()),
                }
            }
        }
        server.shutdown().await;
    }

    #[tokio::test]
    async fn mcp_adapter_rejects_invalid_sizes_before_upstream_contact() {
        let response = Bytes::from_static(br#"{"data":[{"b64_json":"AQ=="}]}"#);
        let (server, mock) = start_mock(StatusCode::OK, response).await;
        let selected = route(
            &format!("http://{}/openai/v1", server.address()),
            "selected-image-key",
        );
        let adapter = mcp_adapter(
            ImagesGenerationService::new(routing(true, Some(selected))),
            None,
        );

        for (size, case) in [
            ("", "empty"),
            ("1024", "missing separator"),
            ("1024X1024", "uppercase separator"),
            ("1024x1024x1024", "extra separator"),
            ("+1024x1024", "non-decimal width"),
            ("0x1024", "zero width"),
            ("1025x1024", "non-16-multiple edge"),
            ("3072x1008", "ratio above 3:1"),
            ("3840x2160", "edge at exclusive 3840 limit"),
            ("3856x2144", "edge above 3840"),
            ("1024x624", "pixel count below minimum"),
            ("3840x2176", "pixel count above maximum"),
            ("18446744073709551616x1024", "integer overflow"),
        ] {
            let error = adapter
                .generate_image(GenerateImageArgs {
                    prompt: "private".to_owned(),
                    size: Some(size.to_owned()),
                    quality: None,
                    background: None,
                })
                .await
                .expect_err(case);
            assert_eq!(
                error.message, "The image generation request is invalid.",
                "{case}"
            );
            assert_eq!(mcp_error_field(&error, "stage"), "request_construction");
            assert_eq!(mcp_error_field(&error, "upstreamStatus"), &Value::Null);
            assert_eq!(mcp_error_field(&error, "retryable"), false);
        }

        assert_eq!(mock.calls.load(Ordering::Acquire), 0);
        server.shutdown().await;
    }

    #[test]
    fn mcp_schema_and_option_validation_are_stable() {
        let tool = generate_image_tool();
        assert_eq!(tool.name.as_ref(), "generate_image");
        assert_eq!(
            tool.description.as_deref(),
            Some(
                "Generate one PNG image, save it locally, and return its path and metadata as JSON."
            )
        );
        assert_eq!(tool.input_schema["required"], json!(["prompt"]));
        let size = &tool.input_schema["properties"]["size"];
        assert!(size.get("enum").is_none());
        assert_eq!(size["pattern"], "^(?:auto|[0-9]+x[0-9]+)$");
        let description = size["description"].as_str().expect("size description");
        for expected in [
            "multiples of 16",
            "3,840",
            "3:1",
            "655,360",
            "8,294,400",
            "1024x1024",
            "1536x1024",
            "1024x1536",
            "2048x2048",
            "2048x1152",
            "3,686,400",
            "experimental",
        ] {
            assert!(description.contains(expected), "missing {expected}");
        }
        assert!(optional_argument_is_supported(
            Some("high"),
            &["auto", "low", "medium", "high"]
        ));
        assert!(!optional_argument_is_supported(
            Some("unsupported"),
            &["auto", "low", "medium", "high"]
        ));

        let adapter = mcp_adapter(
            ImagesGenerationService::new(RoutingSnapshotStore::default()),
            None,
        );
        assert_eq!(
            adapter.service.config.response_wire_limit,
            MCP_JSON_RESPONSE_LIMIT
        );
        assert_eq!(
            adapter.service.config.response_decoded_limit,
            MCP_JSON_RESPONSE_LIMIT
        );
        assert!(adapter.service.config.exact_response_capacity);
        let direct = ImagesGenerationService::new(RoutingSnapshotStore::default());
        assert_eq!(direct.config.response_wire_limit, DEFAULT_RESPONSE_LIMIT);
        assert_eq!(direct.config.response_decoded_limit, DEFAULT_RESPONSE_LIMIT);
        assert!(!direct.config.exact_response_capacity);
    }
}
