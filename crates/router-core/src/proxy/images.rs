use std::{borrow::Cow, sync::Arc, time::Duration};

use axum::{
    body::Bytes,
    http::{HeaderMap, HeaderValue, StatusCode, header},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
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

use super::{
    RoutingSnapshotStore,
    upstream::{
        DecodeError, connection_nominated_headers, decode_supported, filtered_response_headers,
        remove_request_header, response_encodings,
    },
};

const DEFAULT_RESPONSE_LIMIT: usize = 200 * 1024 * 1024;
const MAX_PROMPT_BYTES: usize = 1024 * 1024;
const IMAGE_SIZE_MULTIPLE: u64 = 16;
const MAX_IMAGE_EDGE_EXCLUSIVE: u64 = 3_840;
const MAX_IMAGE_ASPECT_RATIO: u64 = 3;
const MIN_IMAGE_PIXELS: u64 = 655_360;
const MAX_IMAGE_PIXELS: u64 = 8_294_400;

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
}

impl Default for ImagesGenerationConfig {
    fn default() -> Self {
        Self {
            body_timeout: Duration::from_mins(10),
            response_wire_limit: DEFAULT_RESPONSE_LIMIT,
            response_decoded_limit: DEFAULT_RESPONSE_LIMIT,
        }
    }
}

#[derive(Debug)]
pub struct ImagesGenerationResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Bytes,
}

#[derive(Clone, Copy, Debug)]
pub enum ImagesGenerationFailureKind {
    Disabled,
    RouteNotSelected,
    RouteUnavailable,
    InvalidRequest,
    UpstreamFailed,
    UpstreamTimeout,
    ResponseTooLarge,
    InvalidEncoding,
    InvalidResponse,
}

#[derive(Debug)]
pub struct ImagesGenerationFailure {
    pub kind: ImagesGenerationFailureKind,
    pub request_id: String,
}

impl ImagesGenerationFailureKind {
    pub const fn code(self) -> &'static str {
        match self {
            Self::Disabled => "images_generation_disabled",
            Self::RouteNotSelected => "images_route_not_selected",
            Self::RouteUnavailable => "images_route_unavailable",
            Self::InvalidRequest => "invalid_images_request",
            Self::UpstreamFailed => "images_upstream_failed",
            Self::UpstreamTimeout => "images_upstream_timeout",
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
            Self::UpstreamFailed
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
            Self::UpstreamFailed => "The image generation upstream request failed.",
            Self::UpstreamTimeout => "The image generation upstream request timed out.",
            Self::ResponseTooLarge => "The image generation response exceeded the local limit.",
            Self::InvalidEncoding => "The image generation response encoding is invalid.",
            Self::InvalidResponse => "The image generation response is invalid.",
        }
    }
}

impl ImagesGenerationFailure {
    pub fn new(kind: ImagesGenerationFailureKind) -> Self {
        Self {
            kind,
            request_id: Uuid::new_v4().to_string(),
        }
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
        if serde_json::from_slice::<serde_json::Value>(&body)
            .ok()
            .and_then(|value| value.as_object().cloned())
            .is_none()
        {
            return Err(ImagesGenerationFailure::new(
                ImagesGenerationFailureKind::InvalidRequest,
            ));
        }
        let routing = self.routing.load();
        if !routing.images_generation_enabled {
            return Err(ImagesGenerationFailure::new(
                ImagesGenerationFailureKind::Disabled,
            ));
        }
        let route = routing.images_route.clone().ok_or_else(|| {
            ImagesGenerationFailure::new(ImagesGenerationFailureKind::RouteNotSelected)
        })?;
        let headers =
            build_upstream_headers(client_headers, route.api_key.expose()).map_err(|()| {
                ImagesGenerationFailure::new(ImagesGenerationFailureKind::UpstreamFailed)
            })?;
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .build()
            .map_err(|_| {
                ImagesGenerationFailure::new(ImagesGenerationFailureKind::UpstreamFailed)
            })?;
        let request = client
            .post(route.base_url.images_generation_url())
            .headers(headers)
            .body(body);
        let upstream = tokio::time::timeout(routing.images_generation_timeout, request.send())
            .await
            .map_err(|_| {
                ImagesGenerationFailure::new(ImagesGenerationFailureKind::UpstreamTimeout)
            })?
            .map_err(|_| {
                ImagesGenerationFailure::new(ImagesGenerationFailureKind::UpstreamFailed)
            })?;
        let status = upstream.status();
        let source_headers = upstream.headers().clone();
        let wire = tokio::time::timeout(
            self.config.body_timeout,
            collect_wire(upstream, self.config.response_wire_limit),
        )
        .await
        .map_err(|_| {
            ImagesGenerationFailure::new(ImagesGenerationFailureKind::UpstreamTimeout)
        })??;
        if !status.is_success() {
            return Err(ImagesGenerationFailure::new(
                ImagesGenerationFailureKind::UpstreamFailed,
            ));
        }
        let encodings = response_encodings(&source_headers);
        let transformed = !encodings.is_empty();
        let body = decode_supported(wire, &encodings, self.config.response_decoded_limit).map_err(
            |error| {
                ImagesGenerationFailure::new(match error {
                    DecodeError::TooLarge => ImagesGenerationFailureKind::ResponseTooLarge,
                    DecodeError::Unsupported | DecodeError::Invalid => {
                        ImagesGenerationFailureKind::InvalidEncoding
                    }
                })
            },
        )?;
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
) -> Result<Vec<u8>, ImagesGenerationFailure> {
    let mut wire = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| {
            ImagesGenerationFailure::new(ImagesGenerationFailureKind::UpstreamFailed)
        })?;
        if wire.len().saturating_add(chunk.len()) > limit {
            return Err(ImagesGenerationFailure::new(
                ImagesGenerationFailureKind::ResponseTooLarge,
            ));
        }
        wire.extend_from_slice(&chunk);
    }
    Ok(wire)
}

#[derive(Clone)]
pub struct ImageMcpServer {
    service: ImagesGenerationService,
    tool: Arc<Tool>,
}

impl ImageMcpServer {
    pub fn new(service: ImagesGenerationService) -> Self {
        Self {
            service,
            tool: Arc::new(generate_image_tool()),
        }
    }

    async fn generate_image(&self, args: GenerateImageArgs) -> Result<CallToolResult, McpError> {
        if args.prompt.trim().is_empty() || args.prompt.len() > MAX_PROMPT_BYTES {
            return Err(McpError::invalid_params("invalid prompt", None));
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
            return Err(McpError::invalid_params("unsupported image option", None));
        }
        let body = mcp_request_body(args)
            .map_err(|_| McpError::internal_error("image request construction failed", None))?;
        let response = self
            .service
            .forward(Bytes::from(body), &HeaderMap::new())
            .await
            .map_err(|error| mcp_forwarding_error(&error))?;
        let data = first_valid_image(&response.body).ok_or_else(|| {
            McpError::internal_error("image generation returned an invalid result", None)
        })?;
        Ok(CallToolResult::success(vec![ContentBlock::image(
            data,
            "image/png",
        )]))
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
        let arguments = request
            .arguments
            .ok_or_else(|| McpError::invalid_params("missing tool arguments", None))?;
        let args: GenerateImageArgs =
            serde_json::from_value(serde_json::Value::Object(arguments.into_iter().collect()))
                .map_err(|_| McpError::invalid_params("invalid tool arguments", None))?;
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

#[derive(Deserialize)]
struct ImagesResponse {
    data: Vec<ImagesResponseItem>,
}

#[derive(Deserialize)]
struct ImagesResponseItem {
    b64_json: Option<String>,
}

fn first_valid_image(body: &[u8]) -> Option<String> {
    serde_json::from_slice::<ImagesResponse>(body)
        .ok()?
        .data
        .into_iter()
        .filter_map(|item| item.b64_json)
        .find(|data| !data.is_empty() && STANDARD.decode(data).is_ok())
}

fn mcp_forwarding_error(error: &ImagesGenerationFailure) -> McpError {
    McpError::internal_error(
        "image generation failed",
        Some(json!({
            "code": error.kind.code(),
            "requestId": &error.request_id,
        })),
    )
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
        Cow::Borrowed("Generate one PNG image from a text prompt."),
        Arc::new(schema),
    )
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    use axum::{
        Router,
        body::Bytes,
        extract::{Request, State},
        http::{HeaderMap, StatusCode, header},
        response::IntoResponse,
        routing::post,
    };
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use serde_json::{Value, json};

    use super::*;
    use crate::{
        domain::{ApiKey, BaseUrl, RouteId, ServiceTierPolicy},
        proxy::{ProxyServerHandle, RouteSnapshot, RoutingSnapshot},
    };

    #[derive(Clone)]
    struct MockImagesUpstream {
        status: StatusCode,
        response: Bytes,
        response_header_delay: Duration,
        calls: Arc<AtomicUsize>,
        captures: Arc<Mutex<Vec<(String, HeaderMap, Bytes)>>>,
    }

    async fn mock_images_handler(
        State(state): State<MockImagesUpstream>,
        request: Request,
    ) -> impl IntoResponse {
        state.calls.fetch_add(1, Ordering::AcqRel);
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
        (state.status, state.response)
    }

    fn route(base_url: &str, key: &str) -> Arc<RouteSnapshot> {
        Arc::new(RouteSnapshot {
            route_id: RouteId::new(),
            name: "Image route".to_owned(),
            base_url: BaseUrl::parse(base_url).expect("base URL"),
            api_key: Arc::new(ApiKey::parse(key).expect("API key")),
            service_tier_policy: ServiceTierPolicy::Passthrough,
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
        assert_eq!(error.kind.code(), "images_upstream_failed");
        assert!(!error.kind.message().contains(sentinel));
        assert_eq!(mock.calls.load(Ordering::Acquire), 1);
        server.shutdown().await;
    }

    #[tokio::test]
    async fn mcp_adapter_fixes_payload_and_returns_one_large_png_block() {
        let image_data = STANDARD.encode(vec![11_u8; 800_000]);
        let response = serde_json::to_vec(&json!({"data": [{"b64_json": image_data}]}))
            .expect("response fixture");
        let (server, mock) = start_mock(StatusCode::OK, Bytes::from(response)).await;
        let selected = route(
            &format!("http://{}/openai/v1", server.address()),
            "selected-image-key",
        );
        let adapter =
            ImageMcpServer::new(ImagesGenerationService::new(routing(true, Some(selected))));
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
        assert_eq!(result["content"][0]["type"], "image");
        assert_eq!(result["content"][0]["mimeType"], "image/png");
        assert!(
            result["content"][0]["data"]
                .as_str()
                .is_some_and(|data| data.len() > 1024 * 1024)
        );
        assert!(result.get("structuredContent").is_none());

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
    async fn mcp_adapter_forwards_omitted_auto_and_arbitrary_supported_sizes() {
        let response = Bytes::from_static(br#"{"data":[{"b64_json":"AQ=="}]}"#);
        let (server, mock) = start_mock(StatusCode::OK, response).await;
        let selected = route(
            &format!("http://{}/openai/v1", server.address()),
            "selected-image-key",
        );
        let adapter =
            ImageMcpServer::new(ImagesGenerationService::new(routing(true, Some(selected))));
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
        let adapter =
            ImageMcpServer::new(ImagesGenerationService::new(routing(true, Some(selected))));

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
            assert_eq!(error.message, "unsupported image option", "{case}");
        }

        assert_eq!(mock.calls.load(Ordering::Acquire), 0);
        server.shutdown().await;
    }

    #[test]
    fn mcp_schema_and_option_validation_are_stable() {
        let tool = generate_image_tool();
        assert_eq!(tool.name.as_ref(), "generate_image");
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
    }
}
