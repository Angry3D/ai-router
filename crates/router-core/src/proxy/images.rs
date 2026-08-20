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

    fn with_mcp_response_limits(mut self, wire_limit: usize, decoded_limit: usize) -> Self {
        self.config.response_wire_limit = wire_limit;
        self.config.response_decoded_limit = decoded_limit;
        self.config.exact_response_capacity = true;
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
            collect_wire(
                upstream,
                self.config.response_wire_limit,
                self.config.exact_response_capacity,
            ),
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
        let decode = if self.config.exact_response_capacity {
            decode_supported_exact
        } else {
            decode_supported
        };
        let body =
            decode(wire, &encodings, self.config.response_decoded_limit).map_err(|error| {
                ImagesGenerationFailure::new(match error {
                    DecodeError::TooLarge => ImagesGenerationFailureKind::ResponseTooLarge,
                    DecodeError::Unsupported | DecodeError::Invalid => {
                        ImagesGenerationFailureKind::InvalidEncoding
                    }
                })
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
) -> Result<Vec<u8>, ImagesGenerationFailure> {
    let mut wire = Vec::new();
    if exact_capacity {
        wire.try_reserve_exact(limit).map_err(|_| {
            ImagesGenerationFailure::new(ImagesGenerationFailureKind::ResponseTooLarge)
        })?;
    }
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
    asset_root: Option<std::path::PathBuf>,
    image_permit: Arc<tokio::sync::Semaphore>,
    publication_fault: PublicationFault,
    tool: Arc<Tool>,
}

impl ImageMcpServer {
    pub(super) fn new(
        service: ImagesGenerationService,
        asset_root: Option<std::path::PathBuf>,
        image_permit: Arc<tokio::sync::Semaphore>,
    ) -> Self {
        Self {
            service: service
                .with_mcp_response_limits(MCP_JSON_RESPONSE_LIMIT, MCP_JSON_RESPONSE_LIMIT),
            asset_root,
            image_permit,
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
        let permit = Arc::clone(&self.image_permit)
            .acquire_owned()
            .await
            .map_err(|_| image_asset_error(ImageAssetErrorKind::StorageUnavailable))?;
        let asset_root = self
            .asset_root
            .clone()
            .ok_or_else(|| image_asset_error(ImageAssetErrorKind::StorageUnavailable))?;
        let admitted_root =
            tokio::task::spawn_blocking(move || AdmittedAssetRoot::admit(asset_root))
                .await
                .map_err(|_| image_asset_error(ImageAssetErrorKind::StorageUnavailable))?
                .map_err(image_asset_error)?;
        let response = self
            .service
            .forward(Bytes::from(body), &HeaderMap::new())
            .await
            .map_err(|error| mcp_forwarding_error(&error))?;
        let fault = self.publication_fault;
        let asset = tokio::task::spawn_blocking(move || {
            // A cancelled MCP future cannot release the shared memory permit
            // while its non-cancellable blocking publication is still running.
            let _permit = permit;
            process_image_response(response.body, &admitted_root, fault)
        })
        .await
        .map_err(|_| image_asset_error(ImageAssetErrorKind::WriteFailed))?
        .map_err(image_asset_error)?;
        let text = serde_json::to_string(&asset)
            .map_err(|_| image_asset_error(ImageAssetErrorKind::WriteFailed))?;
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

fn mcp_forwarding_error(error: &ImagesGenerationFailure) -> McpError {
    McpError::internal_error(
        "image generation failed",
        Some(json!({
            "code": error.kind.code(),
            "requestId": &error.request_id,
        })),
    )
}

fn image_asset_error(kind: ImageAssetErrorKind) -> McpError {
    McpError::internal_error(
        kind.message(),
        Some(json!({
            "code": kind.code(),
            "requestId": Uuid::new_v4().to_string(),
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
        body::Bytes,
        extract::{Request, State},
        http::{HeaderMap, StatusCode, header},
        response::IntoResponse,
        routing::post,
    };
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use serde_json::{Value, json};
    use sha2::{Digest, Sha256};
    use tempfile::TempDir;
    use tokio::sync::Semaphore;

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
        ImageMcpServer::new(service, asset_root, Arc::new(Semaphore::new(1)))
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
        let adapter = mcp_adapter(
            ImagesGenerationService::new(routing(true, Some(selected))),
            Some(asset_root.clone()),
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
            assert_eq!(mock.calls.load(Ordering::Acquire), 1);
            assert_eq!(
                std::fs::read_dir(&asset_root).expect("asset root").count(),
                0
            );
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
        let first = ImageMcpServer::new(
            service.clone(),
            Some(asset_root.clone()),
            Arc::clone(&permit),
        );
        let second = ImageMcpServer::new(service, Some(asset_root), permit);

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
        let first = ImageMcpServer::new(
            service.clone(),
            Some(asset_root.clone()),
            Arc::clone(&permit),
        )
        .with_publication_fault(PublicationFault::with_delay(
            asset::PublicationStage::AfterCreate,
            Duration::from_millis(250),
        ));
        let second = ImageMcpServer::new(service, Some(asset_root.clone()), permit);

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
            assert_eq!(error.message, "unsupported image option", "{case}");
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
