use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    Router,
    body::{Body, Bytes, to_bytes},
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::Response,
    routing::post,
};
use router_core::{
    balance::BalanceQueryMode,
    domain::{
        ApiKey, BalanceQueryPolicy, BaseUrl, CompletionState, DeliveryState,
        ImagesGenerationTimeout, InferenceStatus, InferenceStatusKind, ServiceTierPolicy,
        UpstreamAttemptId,
    },
    proxy::{
        AsyncHistoryRecorder, HistorySummaryChangeSink, InferenceStatusChangeSink,
        InferenceStatusService, ProxyIngressState, ProxyServerHandle, ResponsesForwarder,
        RouteSnapshot, RoutingSnapshot, RoutingSnapshotStore, RuntimeDiagnosticEvent,
        RuntimeDiagnosticSink, build_proxy_router,
    },
    recovery::RecoveryManager,
    state::RouteSummaryDto,
    storage::{
        AppSettingsRecord, AttemptHistoryRecord, BalanceQueryInput, CreateRouteInput,
        DatabaseExecutor, RequestHistoryRecord, RouteRecord, RoutingStateRecord,
    },
};
use serde_json::json;
use tempfile::TempDir;

const API_KEY_SENTINEL: &str = "P9_API_KEY_SENTINEL_7d41";
const GATEWAY_TOKEN_SENTINEL: &str = "P9_GATEWAY_TOKEN_SENTINEL_27ac";
const HEADER_SENTINEL: &str = "P9_HEADER_SENTINEL_65af";
const REQUEST_BODY_SENTINEL: &str = "P9_REQUEST_BODY_SENTINEL_8b31";
const RESPONSE_BODY_SENTINEL: &str = "P9_RESPONSE_BODY_SENTINEL_2c19";
const SSE_SENTINEL: &str = "P9_SSE_SENTINEL_9e52";
const BALANCE_SCRIPT_SENTINEL: &str = "P9_BALANCE_SCRIPT_SENTINEL_41fd";
const CODEX_BASELINE_SENTINEL: &str = "P9_CODEX_BASELINE_SENTINEL_3a76";
const CODEX_RECOVERY_SENTINEL: &str = "P9_CODEX_RECOVERY_SENTINEL_b527";
const RECOVERY_EXCLUDED_SENTINEL: &str = "V02B_EXCLUDED_HISTORY_SENTINEL_91ce";
const RECOVERY_SECOND_KEY: &str = "V02B_SECOND_ROUTE_KEY_44da";
const IMAGE_PROMPT_SENTINEL: &str = "IMAGES_PROMPT_SENTINEL_4d8e";
const IMAGE_BASE64_PREFIX_SENTINEL: &str = "IMAGES_BASE64_PREFIX_SENTINEL_7a2f";
const IMAGE_BASE64_SUFFIX_SENTINEL: &str = "IMAGES_BASE64_SUFFIX_SENTINEL_19c3";
const IMAGE_ROUTE_KEY_SENTINEL: &str = "IMAGES_ROUTE_KEY_SENTINEL_b5d1";
const IMAGE_UPSTREAM_ERROR_SENTINEL: &str = "IMAGES_UPSTREAM_ERROR_SENTINEL_e26a";
const IMAGE_MCP_PROMPT_SENTINEL: &str = "IMAGES_MCP_PROMPT_SENTINEL_719e";
const IMAGE_PROVIDER_CODE_SENTINEL: &str = "IMAGES_PROVIDER_CODE_SENTINEL_d524";
const IMAGE_PROVIDER_REQUEST_ID_SENTINEL: &str = "IMAGES_PROVIDER_REQUEST_ID_SENTINEL_164b";
const IMAGE_PROVIDER_ARBITRARY_SENTINEL: &str = "IMAGES_PROVIDER_ARBITRARY_SENTINEL_791a";
const IMAGE_PROVIDER_HEADER_SENTINEL: &str = "IMAGES_PROVIDER_HEADER_SENTINEL_b83c";
const IMAGE_ALLOWED_TRANSIENT_MESSAGE: &str = "benign transient provider detail";

#[derive(Clone, Default)]
struct MockResponsesState {
    requests: Arc<Mutex<Vec<CapturedRequest>>>,
}

struct CapturedRequest {
    headers: HeaderMap,
    body: Bytes,
}

#[derive(Clone)]
struct MockImagesState {
    requests: Arc<Mutex<Vec<CapturedRequest>>>,
    success_body: Bytes,
}

async fn mock_images_handler(State(state): State<MockImagesState>, request: Request) -> Response {
    let headers = request.headers().clone();
    let body = to_bytes(request.into_body(), 2 * 1024 * 1024)
        .await
        .expect("mock image request body");
    let call_index = {
        let mut requests = state.requests.lock().expect("image capture mutex");
        requests.push(CapturedRequest { headers, body });
        requests.len()
    };
    if call_index == 1 {
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(state.success_body))
            .expect("image success response");
    }
    if call_index >= 3 {
        return Response::builder()
            .status(StatusCode::IM_A_TEAPOT)
            .header(header::CONTENT_TYPE, "application/json")
            .header("x-provider-request-id", IMAGE_PROVIDER_HEADER_SENTINEL)
            .body(Body::from(
                json!({
                    "error": {
                        "code": IMAGE_PROVIDER_CODE_SENTINEL,
                        "message": format!("  benign\ntransient\u{0} provider detail  "),
                        "request_id": IMAGE_PROVIDER_REQUEST_ID_SENTINEL
                    },
                    "arbitrary": IMAGE_PROVIDER_ARBITRARY_SENTINEL
                })
                .to_string(),
            ))
            .expect("unknown image error response");
    }
    Response::builder()
        .status(StatusCode::INTERNAL_SERVER_ERROR)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({"error": {"message": IMAGE_UPSTREAM_ERROR_SENTINEL}}).to_string(),
        ))
        .expect("image error response")
}

async fn mock_responses_handler(
    State(state): State<MockResponsesState>,
    request: Request,
) -> Response {
    let headers = request.headers().clone();
    let body = to_bytes(request.into_body(), 1024 * 1024)
        .await
        .expect("mock request body");
    let is_stream = serde_json::from_slice::<serde_json::Value>(&body)
        .ok()
        .and_then(|value| value.get("stream").and_then(serde_json::Value::as_bool))
        .unwrap_or(false);
    state
        .requests
        .lock()
        .expect("request capture mutex")
        .push(CapturedRequest { headers, body });

    if is_stream {
        let sse = format!(
            "event: response.output_text.delta\ndata: {{\"type\":\"response.output_text.delta\",\"delta\":\"{SSE_SENTINEL}\"}}\n\nevent: response.completed\ndata: {{\"type\":\"response.completed\",\"response\":{{\"id\":\"resp_stream\",\"model\":\"gpt-5\",\"status\":\"completed\",\"usage\":{{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}}}}\n\n"
        );
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/event-stream")
            .body(Body::from(sse))
            .expect("stream response");
    }

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({
                "id": "resp_non_stream",
                "model": "gpt-5",
                "status": "completed",
                "output": [{"type": "message", "content": [{"type": "output_text", "text": RESPONSE_BODY_SENTINEL}]}],
                "usage": {"input_tokens": 2, "output_tokens": 3, "total_tokens": 5}
            })
            .to_string(),
        ))
        .expect("JSON response")
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
        _updates: Vec<(router_core::domain::RouteId, InferenceStatus)>,
    ) {
    }
}

struct NoopHistoryChanges;

impl HistorySummaryChangeSink for NoopHistoryChanges {
    fn history_summary_changed(&self) {}
}

fn contains(bytes: &[u8], sentinel: &str) -> bool {
    bytes
        .windows(sentinel.len())
        .any(|window| window == sentinel.as_bytes())
}

async fn mcp_sse_json(response: reqwest::Response) -> serde_json::Value {
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("text/event-stream")
    );
    let body = response.text().await.expect("MCP SSE body");
    body.lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(str::trim)
        .filter(|data| !data.is_empty())
        .find_map(|data| serde_json::from_str(data).ok())
        .expect("MCP SSE JSON data")
}

async fn send_test_requests(endpoint: &str) -> (Bytes, Bytes) {
    let client = reqwest::Client::new();
    let request = |stream| {
        client
            .post(endpoint)
            .bearer_auth(GATEWAY_TOKEN_SENTINEL)
            .header("x-p9-private-header", HEADER_SENTINEL)
            .header(header::CONTENT_TYPE, "application/json")
            .body(
                json!({
                    "model": "gpt-5",
                    "stream": stream,
                    "input": REQUEST_BODY_SENTINEL
                })
                .to_string(),
            )
    };
    let non_stream = request(false).send().await.expect("non-stream request");
    assert_eq!(non_stream.status(), StatusCode::OK);
    let non_stream_bytes = non_stream.bytes().await.expect("non-stream body");
    let stream = request(true).send().await.expect("stream request");
    assert_eq!(stream.status(), StatusCode::OK);
    let stream_bytes = stream.bytes().await.expect("stream body");
    (non_stream_bytes, stream_bytes)
}

fn assert_upstream_capture(mock_state: &MockResponsesState) {
    let captured = mock_state.requests.lock().expect("request capture mutex");
    assert_eq!(captured.len(), 2);
    for request in captured.iter() {
        assert_eq!(
            request.headers.get(header::AUTHORIZATION),
            Some(&HeaderValue::from_str(&format!("Bearer {API_KEY_SENTINEL}")).expect("header"))
        );
        assert_eq!(
            request.headers.get("x-p9-private-header"),
            Some(&HeaderValue::from_static(HEADER_SENTINEL))
        );
        assert!(contains(&request.body, REQUEST_BODY_SENTINEL));
        assert!(!contains(&request.body, GATEWAY_TOKEN_SENTINEL));
    }
}

async fn assert_persistence_and_privacy(
    database: &DatabaseExecutor,
    database_path: &Path,
    route: &RouteRecord,
    diagnostics: &DiagnosticCapture,
) {
    let history_summary = database.history_summary().await.expect("history summary");
    assert_eq!(history_summary.request_count, 2);
    let latest = database
        .latest_inference_attempts()
        .await
        .expect("latest inference attempts");
    assert_eq!(latest.len(), 1);
    assert_eq!(latest[0].route_id, route.route_id);
    assert!(latest[0].succeeded);

    let edit = database
        .route_edit(route.route_id.clone())
        .await
        .expect("route edit");
    assert_eq!(edit.api_key.expose(), API_KEY_SENTINEL.as_bytes());
    assert!(
        edit.balance_query
            .expect("balance script")
            .custom_source
            .contains(BALANCE_SCRIPT_SENTINEL)
    );
    assert_eq!(
        database
            .codex_baseline()
            .await
            .expect("baseline query")
            .expect("baseline")
            .raw_bytes,
        CODEX_BASELINE_SENTINEL.as_bytes()
    );

    let normal_ipc = serde_json::to_vec(&RouteSummaryDto {
        route_id: route.route_id.clone(),
        name: route.name.clone(),
        base_url_host: "127.0.0.1".to_owned(),
        inference_status: InferenceStatus {
            kind: InferenceStatusKind::Unverified,
            last_outcome: None,
            failure_reason: None,
            observed_at_ms: None,
        },
        health: None,
    })
    .expect("normal route DTO");
    for forbidden in [
        API_KEY_SENTINEL,
        GATEWAY_TOKEN_SENTINEL,
        HEADER_SENTINEL,
        REQUEST_BODY_SENTINEL,
        RESPONSE_BODY_SENTINEL,
        SSE_SENTINEL,
        BALANCE_SCRIPT_SENTINEL,
        CODEX_BASELINE_SENTINEL,
    ] {
        assert!(!contains(&normal_ipc, forbidden));
    }

    let database_bytes = std::fs::read(database_path).expect("database bytes");
    for allowed in [
        API_KEY_SENTINEL,
        GATEWAY_TOKEN_SENTINEL,
        BALANCE_SCRIPT_SENTINEL,
        CODEX_BASELINE_SENTINEL,
    ] {
        assert!(
            contains(&database_bytes, allowed),
            "missing allowed {allowed}"
        );
    }
    for forbidden in [
        HEADER_SENTINEL,
        REQUEST_BODY_SENTINEL,
        RESPONSE_BODY_SENTINEL,
        SSE_SENTINEL,
    ] {
        assert!(
            !contains(&database_bytes, forbidden),
            "persisted forbidden {forbidden}"
        );
    }
    assert!(diagnostics.0.lock().expect("diagnostic mutex").is_empty());
}

#[tokio::test]
async fn responses_flow_preserves_transport_and_enforces_privacy_allowlist() {
    let mock_state = MockResponsesState::default();
    let upstream = ProxyServerHandle::start(
        0,
        Router::new()
            .route("/v1/responses", post(mock_responses_handler))
            .with_state(mock_state.clone()),
    )
    .await
    .expect("mock upstream");

    let temporary = TempDir::new().expect("temporary directory");
    let database_path = temporary.path().join("data/router.sqlite3");
    let database = DatabaseExecutor::open(&database_path).expect("database");
    let route = database
        .create_route(CreateRouteInput {
            name: "P9 integration route".to_owned(),
            base_url: format!("http://{}/v1", upstream.address()),
            api_key: ApiKey::parse(API_KEY_SENTINEL).expect("API Key"),
            service_tier_policy: ServiceTierPolicy::Passthrough,
            balance_query: Some(BalanceQueryInput {
                mode: BalanceQueryMode::CustomJs,
                enabled: true,
                custom_source: format!("(() => '{BALANCE_SCRIPT_SENTINEL}')()"),
            }),
            accept_script_risk: true,
        })
        .await
        .expect("route");
    database
        .get_or_create_singleton_secret(
            "gateway_token".to_owned(),
            ApiKey::parse(GATEWAY_TOKEN_SENTINEL).expect("gateway token"),
        )
        .await
        .expect("stored gateway token");
    database
        .capture_codex_baseline(
            true,
            CODEX_BASELINE_SENTINEL.as_bytes().to_vec(),
            Some(0o600),
        )
        .await
        .expect("Codex baseline");
    let diagnostics = Arc::new(DiagnosticCapture::default());
    let history = AsyncHistoryRecorder::new(
        database.clone(),
        diagnostics.clone(),
        Arc::new(NoopHistoryChanges),
    );
    let inference = InferenceStatusService::new(Arc::new(NoopInferenceChanges));
    let forwarder = ResponsesForwarder::new()
        .expect("forwarder")
        .with_runtime_services(history.clone(), diagnostics.clone(), inference);
    let proxy_state = ProxyIngressState::new(GATEWAY_TOKEN_SENTINEL, Arc::new(forwarder))
        .with_runtime_sinks(history.clone(), diagnostics.clone());
    proxy_state.set_active_route(Some(Arc::new(RouteSnapshot {
        route_id: route.route_id.clone(),
        name: route.name.clone(),
        base_url: BaseUrl::parse(&route.base_url).expect("base URL"),
        api_key: Arc::new(ApiKey::parse(API_KEY_SENTINEL).expect("API Key")),
        service_tier_policy: ServiceTierPolicy::Passthrough,
        fallback_excluded_models: Arc::new(std::collections::HashSet::new()),
    })));
    let proxy = ProxyServerHandle::start(0, build_proxy_router(proxy_state))
        .await
        .expect("local proxy");
    let endpoint = format!("http://{}/v1/responses", proxy.address());
    let (non_stream_bytes, stream_bytes) = send_test_requests(&endpoint).await;
    assert!(contains(&non_stream_bytes, RESPONSE_BODY_SENTINEL));
    assert!(contains(&stream_bytes, SSE_SENTINEL));

    history.shutdown().await;
    proxy.shutdown().await;
    upstream.shutdown().await;

    assert_upstream_capture(&mock_state);
    assert_persistence_and_privacy(&database, &database_path, &route, &diagnostics).await;
}

#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "the integration regression proves transport, one-attempt, privacy, and recovery boundaries together"
)]
async fn images_flow_is_single_attempt_large_body_and_private_outside_critical_config() {
    let image_data = format!(
        "{IMAGE_BASE64_PREFIX_SENTINEL}{}{IMAGE_BASE64_SUFFIX_SENTINEL}",
        "A".repeat(1024 * 1024)
    );
    let success_body = Bytes::from(
        serde_json::to_vec(&json!({
            "created": 1,
            "data": [{"b64_json": image_data}],
            "provider_extension": {"kept": true}
        }))
        .expect("large image response"),
    );
    assert!(success_body.len() > 1024 * 1024);
    let image_upstream_state = MockImagesState {
        requests: Arc::new(Mutex::new(Vec::new())),
        success_body: success_body.clone(),
    };
    let image_upstream = ProxyServerHandle::start(
        0,
        Router::new()
            .route("/v1/images/generations", post(mock_images_handler))
            .with_state(image_upstream_state.clone()),
    )
    .await
    .expect("mock image upstream");

    let temporary = TempDir::new().expect("temporary directory");
    let database_path = temporary.path().join("data/router.sqlite3");
    let database = DatabaseExecutor::open(&database_path).expect("database");
    let route = database
        .create_route(CreateRouteInput {
            name: "Image privacy route".to_owned(),
            base_url: format!("http://{}/v1", image_upstream.address()),
            api_key: ApiKey::parse(IMAGE_ROUTE_KEY_SENTINEL).expect("image route key"),
            service_tier_policy: ServiceTierPolicy::Passthrough,
            balance_query: None,
            accept_script_risk: false,
        })
        .await
        .expect("image route");
    database
        .set_images_generation_settings(
            true,
            Some(route.route_id.clone()),
            ImagesGenerationTimeout::default(),
        )
        .await
        .expect("image settings");
    database
        .get_or_create_singleton_secret(
            "gateway_token".to_owned(),
            ApiKey::parse(GATEWAY_TOKEN_SENTINEL).expect("gateway token"),
        )
        .await
        .expect("stored gateway token");

    let diagnostics = Arc::new(DiagnosticCapture::default());
    let history = AsyncHistoryRecorder::new(
        database.clone(),
        diagnostics.clone(),
        Arc::new(NoopHistoryChanges),
    );
    let inference = InferenceStatusService::new(Arc::new(NoopInferenceChanges));
    let forwarder = ResponsesForwarder::new()
        .expect("forwarder")
        .with_runtime_services(history.clone(), diagnostics.clone(), inference);
    let image_route = Arc::new(RouteSnapshot {
        route_id: route.route_id.clone(),
        name: route.name.clone(),
        base_url: BaseUrl::parse(&route.base_url).expect("base URL"),
        api_key: Arc::new(ApiKey::parse(IMAGE_ROUTE_KEY_SENTINEL).expect("image route key")),
        service_tier_policy: ServiceTierPolicy::Passthrough,
        fallback_excluded_models: Arc::new(std::collections::HashSet::new()),
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
    let proxy_state = ProxyIngressState::new(GATEWAY_TOKEN_SENTINEL, Arc::new(forwarder))
        .with_runtime_sinks(history.clone(), diagnostics.clone())
        .with_routing_store(routing)
        .with_mcp_image_asset_root(temporary.path().join("mcp-images"));
    let proxy = ProxyServerHandle::start(0, build_proxy_router(proxy_state))
        .await
        .expect("local proxy");
    let endpoint = format!("http://{}/v1/images/generations", proxy.address());
    let client = reqwest::Client::new();
    let send_image = || {
        client
            .post(&endpoint)
            .bearer_auth(GATEWAY_TOKEN_SENTINEL)
            .header("x-api-key", "CLIENT_IMAGE_CREDENTIAL_SENTINEL_70fa")
            .header(header::CONTENT_TYPE, "application/json")
            .body(
                json!({
                    "model": "caller-model",
                    "prompt": IMAGE_PROMPT_SENTINEL,
                    "n": 3,
                    "extension": true
                })
                .to_string(),
            )
    };

    let success = send_image().send().await.expect("image success request");
    assert_eq!(success.status(), StatusCode::OK);
    assert_eq!(
        success.bytes().await.expect("image success body"),
        success_body
    );
    let failure = send_image().send().await.expect("image failure request");
    assert_eq!(failure.status(), StatusCode::BAD_GATEWAY);
    let local_error = failure.bytes().await.expect("local image error");
    for forbidden in [
        IMAGE_PROMPT_SENTINEL,
        IMAGE_BASE64_PREFIX_SENTINEL,
        IMAGE_BASE64_SUFFIX_SENTINEL,
        IMAGE_ROUTE_KEY_SENTINEL,
        IMAGE_UPSTREAM_ERROR_SENTINEL,
        GATEWAY_TOKEN_SENTINEL,
    ] {
        assert!(
            !contains(&local_error, forbidden),
            "local error leaked {forbidden}"
        );
    }

    let mcp_endpoint = format!("http://{}/mcp", proxy.address());
    let initialize = client
        .post(&mcp_endpoint)
        .bearer_auth(GATEWAY_TOKEN_SENTINEL)
        .header(header::HOST, "127.0.0.1")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ACCEPT, "application/json, text/event-stream")
        .body(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"privacy-test","version":"1"}}}"#,
        )
        .send()
        .await
        .expect("MCP initialize");
    let session_id = initialize
        .headers()
        .get("mcp-session-id")
        .and_then(|value| value.to_str().ok())
        .expect("MCP session ID")
        .to_owned();
    let _ = mcp_sse_json(initialize).await;
    let mcp_call = client
        .post(&mcp_endpoint)
        .bearer_auth(GATEWAY_TOKEN_SENTINEL)
        .header(header::HOST, "127.0.0.1")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ACCEPT, "application/json, text/event-stream")
        .header("mcp-session-id", session_id)
        .header("mcp-protocol-version", "2025-06-18")
        .body(
            json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": {
                    "name": "generate_image",
                    "arguments": {"prompt": IMAGE_MCP_PROMPT_SENTINEL}
                }
            })
            .to_string(),
        )
        .send()
        .await
        .expect("MCP image call");
    let mcp_error = mcp_sse_json(mcp_call).await;
    assert_eq!(mcp_error["error"]["code"], -32603);
    assert_eq!(
        mcp_error["error"]["message"],
        format!(
            "The image provider returned an unrecognized error. {IMAGE_ALLOWED_TRANSIENT_MESSAGE}"
        )
    );
    let mcp_data = &mcp_error["error"]["data"];
    assert_eq!(mcp_data.as_object().map(serde_json::Map::len), Some(6));
    assert_eq!(mcp_data["code"], "images_upstream_http_status");
    assert_eq!(mcp_data["stage"], "upstream_http_status");
    assert_eq!(mcp_data["upstreamStatus"], 418);
    assert_eq!(mcp_data["category"], "unknown_upstream");
    assert_eq!(mcp_data["retryable"], false);
    uuid::Uuid::parse_str(mcp_data["requestId"].as_str().expect("local request ID"))
        .expect("UUID request ID");
    let serialized_mcp_error = serde_json::to_vec(&mcp_error).expect("serialized MCP error");
    assert!(contains(
        &serialized_mcp_error,
        IMAGE_ALLOWED_TRANSIENT_MESSAGE
    ));
    for forbidden in [
        IMAGE_PROMPT_SENTINEL,
        IMAGE_MCP_PROMPT_SENTINEL,
        IMAGE_BASE64_PREFIX_SENTINEL,
        IMAGE_BASE64_SUFFIX_SENTINEL,
        IMAGE_ROUTE_KEY_SENTINEL,
        IMAGE_PROVIDER_CODE_SENTINEL,
        IMAGE_PROVIDER_REQUEST_ID_SENTINEL,
        IMAGE_PROVIDER_ARBITRARY_SENTINEL,
        IMAGE_PROVIDER_HEADER_SENTINEL,
        GATEWAY_TOKEN_SENTINEL,
    ] {
        assert!(
            !contains(&serialized_mcp_error, forbidden),
            "MCP error leaked {forbidden}"
        );
    }

    history.shutdown().await;
    proxy.shutdown().await;
    image_upstream.shutdown().await;

    {
        let captured = image_upstream_state
            .requests
            .lock()
            .expect("image capture mutex");
        assert_eq!(captured.len(), 3, "each incoming request gets one attempt");
        for (index, request) in captured.iter().enumerate() {
            assert_eq!(
                request.headers.get(header::AUTHORIZATION),
                Some(
                    &HeaderValue::from_str(&format!("Bearer {IMAGE_ROUTE_KEY_SENTINEL}"))
                        .expect("image authorization")
                )
            );
            assert!(request.headers.get("x-api-key").is_none());
            if index < 2 {
                assert!(contains(&request.body, IMAGE_PROMPT_SENTINEL));
            } else {
                assert!(contains(&request.body, IMAGE_MCP_PROMPT_SENTINEL));
            }
            assert!(!contains(&request.body, GATEWAY_TOKEN_SENTINEL));
        }
    }

    assert_eq!(
        database
            .history_summary()
            .await
            .expect("history summary")
            .request_count,
        0
    );
    assert!(diagnostics.0.lock().expect("diagnostic mutex").is_empty());
    let normal_ipc = serde_json::to_vec(&RouteSummaryDto {
        route_id: route.route_id,
        name: route.name,
        base_url_host: "127.0.0.1".to_owned(),
        inference_status: InferenceStatus {
            kind: InferenceStatusKind::Unverified,
            last_outcome: None,
            failure_reason: None,
            observed_at_ms: None,
        },
        health: None,
    })
    .expect("normal route DTO");
    let recovery_manager = RecoveryManager::new(&database_path);
    let recovery = recovery_manager
        .create_point(&database)
        .await
        .expect("recovery point");
    let database_bytes = fs::read(&database_path).expect("database bytes");
    let recovery_path = recovery_manager.recovery_dir().join(format!(
        "point-{}-{}.sqlite3",
        recovery.created_at_ms,
        recovery.point_id.as_str()
    ));
    let recovery_bytes = fs::read(recovery_path).expect("recovery bytes");
    for forbidden in [
        IMAGE_PROMPT_SENTINEL,
        IMAGE_BASE64_PREFIX_SENTINEL,
        IMAGE_BASE64_SUFFIX_SENTINEL,
        IMAGE_UPSTREAM_ERROR_SENTINEL,
        IMAGE_MCP_PROMPT_SENTINEL,
        IMAGE_PROVIDER_CODE_SENTINEL,
        IMAGE_PROVIDER_REQUEST_ID_SENTINEL,
        IMAGE_PROVIDER_ARBITRARY_SENTINEL,
        IMAGE_PROVIDER_HEADER_SENTINEL,
        IMAGE_ALLOWED_TRANSIENT_MESSAGE,
    ] {
        assert!(
            !contains(&database_bytes, forbidden),
            "database leaked {forbidden}"
        );
        assert!(
            !contains(&recovery_bytes, forbidden),
            "recovery leaked {forbidden}"
        );
        assert!(!contains(&normal_ipc, forbidden), "IPC leaked {forbidden}");
    }
}

struct SyntheticCodexFixture {
    path: PathBuf,
    bytes: Vec<u8>,
    #[cfg(unix)]
    mode: u32,
}

struct RecoveryCriticalFixture {
    first: RouteRecord,
    second: RouteRecord,
    routes: Vec<RouteRecord>,
    routing: RoutingStateRecord,
    settings: AppSettingsRecord,
}

fn create_synthetic_codex_fixture(root: &Path) -> SyntheticCodexFixture {
    let path = root.join("synthetic-codex/config.toml");
    fs::create_dir_all(path.parent().expect("Codex parent")).expect("Codex parent");
    fs::write(
        &path,
        b"model_provider = \"synthetic-recovery-fixture\"\n[unrelated]\nkeep = true\n",
    )
    .expect("synthetic Codex config");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).expect("Codex config mode");
    }
    let bytes = fs::read(&path).expect("Codex bytes");
    #[cfg(unix)]
    let mode = {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(&path)
            .expect("Codex metadata")
            .permissions()
            .mode()
            & 0o777
    };
    SyntheticCodexFixture {
        path,
        bytes,
        #[cfg(unix)]
        mode,
    }
}

fn assert_synthetic_codex_unchanged(fixture: &SyntheticCodexFixture) {
    assert_eq!(
        fs::read(&fixture.path).expect("Codex bytes after"),
        fixture.bytes
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&fixture.path)
                .expect("Codex metadata after")
                .permissions()
                .mode()
                & 0o777,
            fixture.mode
        );
    }
}

async fn seed_recovery_critical_state(database: &DatabaseExecutor) -> RecoveryCriticalFixture {
    let first = database
        .create_route(CreateRouteInput {
            name: "Recovery first".to_owned(),
            base_url: "https://first.example.invalid/v1".to_owned(),
            api_key: ApiKey::parse(API_KEY_SENTINEL).expect("first Key"),
            service_tier_policy: ServiceTierPolicy::Passthrough,
            balance_query: Some(BalanceQueryInput {
                mode: BalanceQueryMode::CustomJs,
                enabled: true,
                custom_source: format!("(() => '{BALANCE_SCRIPT_SENTINEL}')()"),
            }),
            accept_script_risk: true,
        })
        .await
        .expect("first route");
    let second = database
        .create_route(CreateRouteInput {
            name: "Recovery second".to_owned(),
            base_url: "https://second.example.invalid/v1".to_owned(),
            api_key: ApiKey::parse(RECOVERY_SECOND_KEY).expect("second Key"),
            service_tier_policy: ServiceTierPolicy::Omit,
            balance_query: None,
            accept_script_risk: false,
        })
        .await
        .expect("second route");
    database
        .activate_route(second.route_id.clone())
        .await
        .expect("active route");
    database
        .set_fallback_enabled(true)
        .await
        .expect("fallback enabled");
    database.set_proxy_port(43_123).await.expect("proxy port");
    let balance_policy = BalanceQueryPolicy::parse(45, 120).expect("balance policy");
    database
        .set_balance_query_policy(balance_policy)
        .await
        .expect("balance settings");
    database
        .get_or_create_singleton_secret(
            "gateway_token".to_owned(),
            ApiKey::parse(GATEWAY_TOKEN_SENTINEL).expect("gateway token"),
        )
        .await
        .expect("stored gateway token");
    database
        .capture_codex_baseline(
            true,
            CODEX_BASELINE_SENTINEL.as_bytes().to_vec(),
            Some(0o600),
        )
        .await
        .expect("Codex baseline");
    database
        .update_codex_recovery_config(
            true,
            CODEX_RECOVERY_SENTINEL.as_bytes().to_vec(),
            Some(0o640),
        )
        .await
        .expect("Codex recovery config");

    RecoveryCriticalFixture {
        first,
        second,
        routes: database.list_routes().await.expect("routes before"),
        routing: database.routing_state().await.expect("routing before"),
        settings: database.app_settings().await.expect("settings before"),
    }
}

async fn seed_excluded_recovery_history(database: &DatabaseExecutor, route: &RouteRecord) {
    database
        .record_request_history(RequestHistoryRecord {
            request_id: "recovery-excluded-request".to_owned(),
            started_at_ms: 10,
            finished_at_ms: 20,
            turn_id: Some(RECOVERY_EXCLUDED_SENTINEL.to_owned()),
            requested_model: Some(RECOVERY_EXCLUDED_SENTINEL.to_owned()),
            reasoning_effort: Some(RECOVERY_EXCLUDED_SENTINEL.to_owned()),
            requested_service_tier: None,
            actual_model: None,
            actual_service_tier: None,
            final_route_id: Some(route.route_id.clone()),
            final_route_name: Some(RECOVERY_EXCLUDED_SENTINEL.to_owned()),
            streaming: true,
            completion_state: CompletionState::Failed,
            http_status: Some(502),
            error_category: Some(RECOVERY_EXCLUDED_SENTINEL.to_owned()),
            input_tokens: None,
            output_tokens: None,
            total_tokens: None,
            cached_input_tokens: None,
            cache_write_input_tokens: None,
            total_latency_ms: Some(10),
            first_output_latency_ms: None,
            metadata_complete: true,
            fallback_stop_reason: None,
            fallback_stop_target_route_id: None,
            fallback_stop_target_route_name: None,
            attempts: vec![AttemptHistoryRecord {
                attempt_id: UpstreamAttemptId::new(),
                attempt_index: 0,
                attempt_role: router_core::storage::AttemptRole::Ordinary,
                route_id: route.route_id.clone(),
                route_name: RECOVERY_EXCLUDED_SENTINEL.to_owned(),
                started_at_ms: 10,
                finished_at_ms: 20,
                http_status: Some(502),
                error_category: Some(RECOVERY_EXCLUDED_SENTINEL.to_owned()),
                delivery_state: DeliveryState::None,
                actual_model: None,
                forwarded_service_tier: None,
                actual_service_tier: None,
                input_tokens: None,
                output_tokens: None,
                total_tokens: None,
                cached_input_tokens: None,
                cache_write_input_tokens: None,
            }],
        })
        .await
        .expect("excluded history");
}

fn assert_recovery_point_privacy(point_bytes: &[u8]) {
    assert!(!contains(point_bytes, RECOVERY_EXCLUDED_SENTINEL));
    for allowed in [
        API_KEY_SENTINEL,
        RECOVERY_SECOND_KEY,
        GATEWAY_TOKEN_SENTINEL,
        BALANCE_SCRIPT_SENTINEL,
        CODEX_BASELINE_SENTINEL,
    ] {
        assert!(contains(point_bytes, allowed), "point omitted {allowed}");
    }
}

async fn assert_restored_critical_state(
    restored: &DatabaseExecutor,
    fixture: RecoveryCriticalFixture,
) {
    assert_eq!(
        restored.list_routes().await.expect("restored routes"),
        fixture.routes
    );
    assert_eq!(
        restored.routing_state().await.expect("restored routing"),
        fixture.routing
    );
    assert_eq!(
        restored.app_settings().await.expect("restored settings"),
        fixture.settings
    );
    let first_edit = restored
        .route_edit(fixture.first.route_id)
        .await
        .expect("first route edit");
    assert_eq!(first_edit.api_key.expose(), API_KEY_SENTINEL.as_bytes());
    assert!(
        first_edit
            .balance_query
            .expect("balance script")
            .custom_source
            .contains(BALANCE_SCRIPT_SENTINEL)
    );
    let second_edit = restored
        .route_edit(fixture.second.route_id)
        .await
        .expect("second route edit");
    assert_eq!(second_edit.api_key.expose(), RECOVERY_SECOND_KEY.as_bytes());
    assert_eq!(
        second_edit.route.service_tier_policy,
        ServiceTierPolicy::Omit
    );
    assert!(second_edit.balance_query.is_none());
    assert_eq!(
        restored
            .get_or_create_singleton_secret(
                "gateway_token".to_owned(),
                ApiKey::parse("unused-default").expect("unused token"),
            )
            .await
            .expect("restored gateway token")
            .expose(),
        GATEWAY_TOKEN_SENTINEL.as_bytes()
    );
    let baseline = restored
        .codex_baseline()
        .await
        .expect("baseline query")
        .expect("restored baseline");
    assert!(baseline.original_exists);
    assert_eq!(baseline.raw_bytes, CODEX_BASELINE_SENTINEL.as_bytes());
    assert_eq!(baseline.unix_mode, Some(0o600));
    let recovery = restored
        .codex_recovery_config()
        .await
        .expect("recovery config query")
        .expect("restored recovery config");
    assert!(recovery.original_exists);
    assert_eq!(recovery.raw_bytes, CODEX_RECOVERY_SENTINEL.as_bytes());
    assert_eq!(recovery.unix_mode, Some(0o640));
    assert_eq!(
        restored
            .history_summary()
            .await
            .expect("history after restore")
            .request_count,
        0
    );
    assert!(
        restored
            .latest_inference_attempts()
            .await
            .expect("derived inference after restore")
            .is_empty()
    );
}

#[tokio::test]
async fn recovery_round_trip_preserves_critical_state_and_removes_excluded_data() {
    let temporary = TempDir::new().expect("temporary directory");
    let database_path = temporary.path().join("data/router.sqlite3");
    let codex = create_synthetic_codex_fixture(temporary.path());
    let database = DatabaseExecutor::open(&database_path).expect("database");
    let critical = seed_recovery_critical_state(&database).await;
    seed_excluded_recovery_history(&database, &critical.first).await;
    let manager = RecoveryManager::new(&database_path);
    let point = manager
        .create_point(&database)
        .await
        .expect("recovery point");
    let point_path = manager.recovery_dir().join(format!(
        "point-{}-{}.sqlite3",
        point.created_at_ms,
        point.point_id.as_str()
    ));
    let point_before = fs::read(&point_path).expect("point bytes");
    assert_recovery_point_privacy(&point_before);

    drop(database);
    tokio::time::sleep(Duration::from_millis(30)).await;
    fs::write(&database_path, b"synthetic corrupt primary").expect("corrupt primary");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&database_path, fs::Permissions::from_mode(0o600))
            .expect("primary mode");
    }
    manager
        .restore_point(&point.point_id)
        .expect("restore point");

    assert_eq!(fs::read(&point_path).expect("point retained"), point_before);
    assert_synthetic_codex_unchanged(&codex);
    let restored = DatabaseExecutor::open(&database_path).expect("restored database");
    assert_restored_critical_state(&restored, critical).await;
    assert!(!contains(
        &fs::read(&database_path).expect("restored primary bytes"),
        RECOVERY_EXCLUDED_SENTINEL
    ));
}
