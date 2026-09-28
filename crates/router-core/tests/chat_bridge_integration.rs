//! Loopback integration for Chat Completions upstreams and mixed-protocol fallback.
//!
//! Each case drives the real ingress (`build_proxy_router`) against local
//! synthetic upstreams and asserts the Responses contract the client sees, the
//! bytes each upstream receives, and the history/diagnostic side effects.

use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;

use axum::{
    Router,
    body::{Body, Bytes, to_bytes},
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::Response,
    routing::post,
};
use router_core::{
    domain::{ApiKey, BaseUrl, CompletionState, RouteId, RouteProtocol},
    proxy::{
        AsyncHistoryRecorder, FallbackActivationError, FallbackActivationRequest,
        FallbackActivator, HistorySummaryChangeSink, InferenceStatusChangeSink,
        InferenceStatusService, ProxyIngressState, ProxyServerHandle, ResponsesForwarder,
        RouteSnapshot, RoutingSnapshot, RoutingSnapshotStore, RuntimeDiagnosticEvent,
        RuntimeDiagnosticSink, build_proxy_router,
    },
    storage::{CreateRouteInput, DatabaseExecutor},
};
use serde_json::{Value, json};
use tempfile::TempDir;

const GATEWAY_TOKEN: &str = "CHAT_BRIDGE_GATEWAY_TOKEN_4b1d";
const ROUTE_API_KEY: &str = "CHAT_BRIDGE_ROUTE_KEY_9f27";
const UPSTREAM_ERROR_MESSAGE: &str = "synthetic upstream rejection";

#[derive(Clone)]
struct ChatUpstreamState {
    requests: Arc<Mutex<Vec<CapturedRequest>>>,
    plan: Arc<ChatPlan>,
}

struct CapturedRequest {
    path: String,
    headers: HeaderMap,
    body: Bytes,
}

/// What the synthetic Chat upstream should answer with.
#[derive(Clone)]
struct ChatPlan {
    /// `text/event-stream` frames, or a JSON body when `json` is set.
    sse: String,
    json: Option<String>,
    status: StatusCode,
}

impl ChatPlan {
    fn sse(body: &str) -> Self {
        Self {
            sse: body.to_owned(),
            json: None,
            status: StatusCode::OK,
        }
    }
}

async fn chat_upstream_handler(
    State(state): State<ChatUpstreamState>,
    request: Request,
) -> Response {
    let path = request.uri().path().to_owned();
    let headers = request.headers().clone();
    let body = to_bytes(request.into_body(), 1024 * 1024)
        .await
        .expect("mock chat request body");
    state
        .requests
        .lock()
        .expect("request capture mutex")
        .push(CapturedRequest {
            path,
            headers,
            body,
        });
    let plan = &state.plan;
    if plan.status != StatusCode::OK {
        return Response::builder()
            .status(plan.status)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({ "error": { "code": "invalid_request_error", "message": UPSTREAM_ERROR_MESSAGE } })
                    .to_string(),
            ))
            .expect("chat error response");
    }
    match &plan.json {
        Some(body) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.clone()))
            .expect("chat json response"),
        None => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/event-stream")
            .body(Body::from(plan.sse.clone()))
            .expect("chat sse response"),
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
        _updates: Vec<(
            router_core::domain::RouteId,
            router_core::domain::InferenceStatus,
        )>,
    ) {
    }
}

struct NoopHistoryChanges;

impl HistorySummaryChangeSink for NoopHistoryChanges {
    fn history_summary_changed(&self) {}
}

struct Harness {
    proxy: ProxyServerHandle,
    upstream: ProxyServerHandle,
    state: ChatUpstreamState,
    history: Arc<AsyncHistoryRecorder>,
    database: DatabaseExecutor,
    database_path: std::path::PathBuf,
    diagnostics: Arc<DiagnosticCapture>,
    _temporary: TempDir,
}

impl Harness {
    async fn start(plan: ChatPlan) -> Self {
        let state = ChatUpstreamState {
            requests: Arc::new(Mutex::new(Vec::new())),
            plan: Arc::new(plan),
        };
        let upstream = ProxyServerHandle::start(
            0,
            Router::new()
                .route("/v1/chat/completions", post(chat_upstream_handler))
                .with_state(state.clone()),
        )
        .await
        .expect("mock chat upstream");

        let temporary = TempDir::new().expect("temporary directory");
        let database_path = temporary.path().join("data/router.sqlite3");
        let database = DatabaseExecutor::open(&database_path).expect("database");
        let route = database
            .create_route(CreateRouteInput {
                name: "chat bridge route".to_owned(),
                base_url: format!("http://{}/v1", upstream.address()),
                protocol: Some(RouteProtocol::ChatCompletions),
                api_key: ApiKey::parse(ROUTE_API_KEY).expect("route API Key"),
                menu_visible: None,
                balance_query: None,
                accept_script_risk: false,
            })
            .await
            .expect("route");
        database
            .get_or_create_singleton_secret(
                "gateway_token".to_owned(),
                ApiKey::parse(GATEWAY_TOKEN).expect("gateway token"),
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

        let snapshot = Arc::new(RouteSnapshot {
            route_id: route.route_id.clone(),
            name: route.name.clone(),
            protocol: RouteProtocol::ChatCompletions,
            base_url: BaseUrl::parse(&route.base_url, RouteProtocol::ChatCompletions)
                .expect("base URL"),
            api_key: Arc::new(ApiKey::parse(ROUTE_API_KEY).expect("route API Key")),
            fallback_excluded_models: Arc::new(HashSet::new()),
        });
        let routing = RoutingSnapshotStore::new(RoutingSnapshot {
            active: Some(snapshot.clone()),
            participants: vec![snapshot],
            configured_participant_count: 1,
            enabled: false,
            selection_generation: 0,
            health_generation: 0,
            config_revision: 0,
            images_generation_enabled: false,
            images_route: None,
            images_generation_timeout: std::time::Duration::from_secs(5),
            images_generation_model: Arc::from("gpt-image-1"),
        });
        let proxy_state = ProxyIngressState::new(GATEWAY_TOKEN, Arc::new(forwarder))
            .with_routing_store(routing)
            .with_runtime_sinks(history.clone(), diagnostics.clone());
        let proxy = ProxyServerHandle::start(0, build_proxy_router(proxy_state))
            .await
            .expect("local proxy");

        Self {
            proxy,
            upstream,
            state,
            history,
            database,
            database_path,
            diagnostics,
            _temporary: temporary,
        }
    }

    fn endpoint(&self) -> String {
        format!("http://{}/v1/responses", self.proxy.address())
    }

    async fn send(&self, body: Value) -> (StatusCode, String) {
        let response = reqwest::Client::new()
            .post(self.endpoint())
            .bearer_auth(GATEWAY_TOKEN)
            .header(header::CONTENT_TYPE, "application/json")
            .body(body.to_string())
            .send()
            .await
            .expect("proxy request");
        let status = response.status();
        let text = response.text().await.expect("proxy body");
        (status, text)
    }

    async fn send_with_headers(&self, body: Value) -> (StatusCode, Option<String>, String) {
        let response = reqwest::Client::new()
            .post(self.endpoint())
            .bearer_auth(GATEWAY_TOKEN)
            .header(header::CONTENT_TYPE, "application/json")
            .body(body.to_string())
            .send()
            .await
            .expect("proxy request");
        let status = response.status();
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let text = response.text().await.expect("proxy body");
        (status, content_type, text)
    }

    fn captured(&self) -> Vec<(String, HeaderMap, Bytes)> {
        self.state
            .requests
            .lock()
            .expect("request capture mutex")
            .iter()
            .map(|request| {
                (
                    request.path.clone(),
                    request.headers.clone(),
                    request.body.clone(),
                )
            })
            .collect()
    }

    fn diagnostic_codes(&self) -> Vec<String> {
        self.diagnostics
            .0
            .lock()
            .expect("diagnostic mutex")
            .iter()
            .map(|event| event.code.as_str().to_owned())
            .collect()
    }

    async fn usage_history(&self) -> router_core::storage::UsageHistoryPage {
        self.database
            .usage_history(router_core::storage::UsageHistoryQuery {
                finished_at_or_after_ms: None,
                finished_at_or_before_ms: i64::MAX,
                completion_state: None,
                route_id: None,
                model_contains: None,
                cursor: None,
                limit: 50,
            })
            .await
            .expect("usage history")
    }

    async fn shutdown(self) {
        self.history.shutdown().await;
        self.proxy.shutdown().await;
        self.upstream.shutdown().await;
    }
}

fn chat_request(stream: bool) -> Value {
    json!({
        "model": "gpt-5.1-codex",
        "instructions": "Be terse.",
        "stream": stream,
        "tools": [
            { "type": "function", "name": "exec_command", "description": "Run",
              "parameters": { "type": "object", "properties": {} } },
            { "type": "custom", "name": "apply_patch", "description": "Patch files" },
            { "type": "tool_search" },
        ],
        "tool_choice": "auto",
        "input": [{ "type": "message", "role": "user",
                    "content": [{ "type": "input_text", "text": "list files" }] }],
    })
}

fn chat_sse(text: &str, tool_call: bool, finish_reason: &str) -> String {
    use std::fmt::Write as _;

    let mut frames = String::from(
        "data: {\"id\":\"chatcmpl-1\",\"model\":\"gpt-5.1\",\"created\":1700000000,\
         \"choices\":[{\"index\":0,\"delta\":{\"reasoning_content\":\"thinking\"}}]}\n\n",
    );
    let _ = write!(
        frames,
        "data: {{\"choices\":[{{\"index\":0,\"delta\":{{\"content\":\"{text}\"}}}}]}}\n\n"
    );
    if tool_call {
        frames.push_str(
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\
             \"id\":\"call_1\",\"function\":{\"name\":\"exec_command\",\"arguments\":\"{\\\"cmd\\\":\\\"ls\\\"}\"}}]}}]}\n\n",
        );
    }
    let _ = write!(
        frames,
        "data: {{\"choices\":[],\"usage\":{{\"prompt_tokens\":11,\"completion_tokens\":5,\
         \"total_tokens\":16,\"prompt_tokens_details\":{{\"cached_tokens\":2}}}}}}\n\n\
         data: {{\"choices\":[{{\"index\":0,\"delta\":{{}},\"finish_reason\":\"{finish_reason}\"}}]}}\n\n\
         data: [DONE]\n\n"
    );
    frames
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

fn event_names(body: &str) -> Vec<String> {
    body.lines()
        .filter_map(|line| line.strip_prefix("event: ").map(str::to_owned))
        .collect()
}

fn data_payloads(body: &str) -> Vec<Value> {
    body.lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter(|payload| *payload != "[DONE]")
        .map(|payload| serde_json::from_str(payload).expect("payload is JSON"))
        .collect()
}

fn terminal_payload(body: &str) -> Value {
    data_payloads(body).pop().expect("terminal payload")
}

#[tokio::test]
async fn chat_route_translates_request_and_streams_responses_events() {
    let harness = Harness::start(ChatPlan::sse(&chat_sse("hello", true, "tool_calls"))).await;
    let (status, body) = harness.send(chat_request(true)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("event: response.created"));
    assert!(body.contains("event: response.output_text.delta"));
    assert!(body.contains("event: response.output_item.done"));
    assert!(body.contains("event: response.completed"));
    assert_eq!(body.matches("data: [DONE]").count(), 1);
    assert_eq!(
        terminal_payload(&body)["response"]["status"],
        json!("completed")
    );

    let captured = harness.captured();
    assert_eq!(captured.len(), 1, "exactly one upstream attempt");
    let (path, headers, request_body) = &captured[0];
    assert_eq!(path, "/v1/chat/completions");
    assert_eq!(
        headers.get(header::AUTHORIZATION),
        Some(&HeaderValue::from_str(&format!("Bearer {ROUTE_API_KEY}")).expect("header"))
    );
    let translated: Value = serde_json::from_slice(request_body).expect("translated body");
    assert_eq!(translated["stream"], json!(true));
    assert_eq!(translated["stream_options"]["include_usage"], json!(true));
    assert_eq!(translated["messages"][0]["role"], json!("system"));
    assert_eq!(translated["messages"][1]["role"], json!("user"));
    assert_eq!(
        translated["tools"][0]["function"]["name"],
        json!("exec_command")
    );
    assert!(
        translated.get("input").is_none(),
        "Responses field must not leak"
    );

    let history = harness.database.history_summary().await.expect("history");
    assert_eq!(history.request_count, 1);
    assert_eq!(
        harness.diagnostic_codes(),
        vec!["chat_bridge_compatibility".to_owned()]
    );
    harness.shutdown().await;
}

#[tokio::test]
async fn chat_route_reports_readable_reasoning_and_usage() {
    let harness = Harness::start(ChatPlan::sse(&chat_sse("hi", false, "stop"))).await;
    let (status, body) = harness.send(chat_request(true)).await;
    assert_eq!(status, StatusCode::OK);
    let names = event_names(&body);
    assert!(names.contains(&"response.reasoning_summary_text.delta".to_owned()));
    let terminal = terminal_payload(&body);
    assert_eq!(
        terminal["response"]["usage"],
        json!({
            "input_tokens": 11,
            "output_tokens": 5,
            "total_tokens": 16,
            "input_tokens_details": { "cached_tokens": 2, "cache_write_tokens": 0 },
        })
    );

    // The translated usage must land in the existing history/usage projection.
    harness.history.shutdown().await;
    let page = harness.usage_history().await;
    assert_eq!(page.rows.len(), 1);
    let row = &page.rows[0];
    assert_eq!(row.completion_state, CompletionState::Completed);
    assert_eq!(row.input_tokens, Some(11));
    assert_eq!(row.output_tokens, Some(5));
    assert_eq!(row.total_tokens, Some(16));
    assert_eq!(row.cached_input_tokens, Some(2));
    // Codex 0.155.1 requires the cache-write field whenever usage is present,
    // so an upstream that omits it is completed with an explicit zero.
    assert_eq!(row.cache_write_input_tokens, Some(0));
    harness.shutdown().await;
}

#[tokio::test]
async fn missing_chat_usage_is_not_fabricated_in_history() {
    let frames = concat!(
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n",
    );
    let harness = Harness::start(ChatPlan::sse(frames)).await;
    let (status, body) = harness.send(chat_request(true)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        terminal_payload(&body)["response"]["status"],
        json!("completed")
    );
    assert!(
        terminal_payload(&body)["response"].get("usage").is_none(),
        "a gateway without usage must not produce a usage object"
    );

    harness.history.shutdown().await;
    let page = harness.usage_history().await;
    assert_eq!(page.rows.len(), 1);
    let row = &page.rows[0];
    assert_eq!(row.completion_state, CompletionState::Completed);
    assert_eq!(row.input_tokens, None);
    assert_eq!(row.output_tokens, None);
    assert_eq!(row.total_tokens, None);
    assert_eq!(row.cached_input_tokens, None);
    assert_eq!(row.cache_write_input_tokens, None);
    harness.shutdown().await;
}

#[tokio::test]
async fn chat_route_round_trips_the_synthetic_tool_search_tool() {
    let frames = concat!(
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"c4\",",
        "\"function\":{\"name\":\"tool_search\",\"arguments\":\"{\\\"query\\\":\\\"linear\\\"}\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
        "data: [DONE]\n\n",
    );
    let harness = Harness::start(ChatPlan::sse(frames)).await;
    let (status, body) = harness.send(chat_request(true)).await;
    assert_eq!(status, StatusCode::OK);

    let items = data_payloads(&body)
        .into_iter()
        .filter(|payload| payload["type"] == json!("response.output_item.done"))
        .filter_map(|payload| payload.get("item").cloned())
        .collect::<Vec<_>>();
    let tool_search = items
        .iter()
        .find(|item| item["type"] == json!("tool_search_call"))
        .expect("the hosted tool must come back as tool_search_call");
    assert_eq!(tool_search["arguments"], json!({ "query": "linear" }));
    assert_eq!(tool_search["execution"], json!("client"));
    assert!(
        tool_search.get("id").is_none(),
        "a hosted tool_search_call carries no client id"
    );

    let captured = harness.captured();
    assert_eq!(captured.len(), 1);
    let translated: Value = serde_json::from_slice(&captured[0].2).expect("translated body");
    let declared = translated["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .find(|tool| tool["function"]["name"] == json!("tool_search"))
        .expect("the synthetic tool_search declaration must be sent upstream");
    assert!(
        declared["function"]["parameters"]["properties"]["query"].is_object(),
        "the evidenced synthetic schema declares a query property"
    );
    assert_eq!(
        harness.diagnostic_codes(),
        vec!["chat_bridge_compatibility".to_owned()]
    );
    harness.shutdown().await;
}

#[tokio::test]
async fn chat_bridge_keeps_bodies_and_secrets_out_of_the_database() {
    let harness = Harness::start(ChatPlan::sse(&chat_sse("hello", true, "tool_calls"))).await;
    let (status, _) = harness.send(chat_request(true)).await;
    assert_eq!(status, StatusCode::OK);
    harness.history.shutdown().await;

    let database_bytes = std::fs::read(&harness.database_path).expect("database bytes");
    // Route credentials are stored by design; request/response content is not.
    for forbidden in ["list files", "Be terse.", "exec_command", "chatcmpl-1"] {
        assert!(
            !contains_bytes(&database_bytes, forbidden.as_bytes()),
            "the database must not persist {forbidden}"
        );
    }
    harness.shutdown().await;
}

#[tokio::test]
async fn chat_route_accepts_a_gateway_that_ignored_streaming() {
    let json_body = json!({
        "id": "chatcmpl-2",
        "model": "gpt-5.1",
        "choices": [{ "index": 0, "message": { "content": "non streamed" }, "finish_reason": "stop" }],
        "usage": { "prompt_tokens": 3, "completion_tokens": 4, "total_tokens": 7 }
    })
    .to_string();
    let harness = Harness::start(ChatPlan {
        sse: String::new(),
        json: Some(json_body),
        status: StatusCode::OK,
    })
    .await;
    let (status, content_type, body) = harness.send_with_headers(chat_request(true)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(content_type.as_deref(), Some("text/event-stream"));
    assert!(body.contains("event: response.completed"));
    assert_eq!(
        terminal_payload(&body)["response"]["status"],
        json!("completed")
    );
    harness.shutdown().await;
}

#[tokio::test]
async fn chat_route_preserves_upstream_error_classification() {
    let harness = Harness::start(ChatPlan {
        sse: String::new(),
        json: None,
        status: StatusCode::TOO_MANY_REQUESTS,
    })
    .await;
    let (status, body) = harness.send(chat_request(true)).await;
    assert_ne!(status, StatusCode::OK);
    assert!(body.contains(UPSTREAM_ERROR_MESSAGE));
    let captured = harness.captured();
    assert_eq!(captured.len(), 1);
    harness.shutdown().await;
}

#[tokio::test]
async fn unsupported_request_fails_closed_without_sending_or_striking() {
    let harness = Harness::start(ChatPlan::sse(&chat_sse("unused", false, "stop"))).await;
    let mut request = chat_request(true);
    request["input"] = json!([{ "type": "compaction_trigger" }]);
    let (status, body) = harness.send(request).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body.contains("chat_bridge_unsupported_request"));
    assert!(body.contains("compaction"));
    assert!(
        harness.captured().is_empty(),
        "no upstream send for an untranslatable request"
    );
    assert_eq!(
        harness.diagnostic_codes(),
        vec!["chat_bridge_unsupported_request".to_owned()]
    );
    let history = harness.database.history_summary().await.expect("history");
    assert_eq!(history.request_count, 1);
    let attempts = harness
        .database
        .latest_inference_attempts()
        .await
        .expect("attempts");
    assert!(
        attempts.iter().all(|attempt| !attempt.succeeded),
        "a local translation error must not record a successful inference attempt"
    );
    assert!(
        attempts
            .iter()
            .all(|attempt| attempt.error_category.as_deref()
                == Some("chat_bridge_unsupported_request")),
        "the attempt must carry the bounded local code, never an upstream failure class"
    );
    harness.shutdown().await;
}

#[tokio::test]
async fn non_streaming_client_request_on_a_chat_route_fails_closed() {
    let harness = Harness::start(ChatPlan::sse(&chat_sse("unused", false, "stop"))).await;
    let (status, body) = harness.send(chat_request(false)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body.contains("client_non_streaming"));
    assert!(harness.captured().is_empty());
    harness.shutdown().await;
}

#[tokio::test]
async fn chat_and_responses_routes_keep_their_own_wire_format() {
    // A Responses route keeps the client body byte-identical.
    let responses_state = Arc::new(Mutex::new(Vec::<(String, Bytes)>::new()));
    let capture = responses_state.clone();
    let upstream = ProxyServerHandle::start(
        0,
        Router::new()
            .route(
                "/v1/responses",
                post(move |request: Request| {
                    let capture = capture.clone();
                    async move {
                        let path = request.uri().path().to_owned();
                        let body = to_bytes(request.into_body(), 1024 * 1024)
                            .await
                            .expect("body");
                        capture.lock().expect("capture mutex").push((path, body));
                        Response::builder()
                            .status(StatusCode::OK)
                            .header(header::CONTENT_TYPE, "text/event-stream")
                            .body(Body::from(
                                "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\n\ndata: [DONE]\n\n",
                            ))
                            .expect("responses sse")
                    }
                }),
            ),
    )
    .await
    .expect("responses upstream");

    let temporary = TempDir::new().expect("temporary directory");
    let database =
        DatabaseExecutor::open(temporary.path().join("router.sqlite3")).expect("database");
    let route = database
        .create_route(CreateRouteInput {
            name: "responses route".to_owned(),
            base_url: format!("http://{}/v1", upstream.address()),
            protocol: Some(RouteProtocol::Responses),
            api_key: ApiKey::parse(ROUTE_API_KEY).expect("route API Key"),
            menu_visible: None,
            balance_query: None,
            accept_script_risk: false,
        })
        .await
        .expect("route");
    database
        .get_or_create_singleton_secret(
            "gateway_token".to_owned(),
            ApiKey::parse(GATEWAY_TOKEN).expect("gateway token"),
        )
        .await
        .expect("gateway token");
    let diagnostics = Arc::new(DiagnosticCapture::default());
    let history = AsyncHistoryRecorder::new(
        database.clone(),
        diagnostics.clone(),
        Arc::new(NoopHistoryChanges),
    );
    let forwarder = ResponsesForwarder::new()
        .expect("forwarder")
        .with_runtime_services(
            history.clone(),
            diagnostics.clone(),
            InferenceStatusService::new(Arc::new(NoopInferenceChanges)),
        );
    let snapshot = Arc::new(RouteSnapshot {
        route_id: route.route_id.clone(),
        name: route.name.clone(),
        protocol: RouteProtocol::Responses,
        base_url: BaseUrl::parse(&route.base_url, RouteProtocol::Responses).expect("base URL"),
        api_key: Arc::new(ApiKey::parse(ROUTE_API_KEY).expect("route API Key")),
        fallback_excluded_models: Arc::new(HashSet::new()),
    });
    let proxy_state = ProxyIngressState::new(GATEWAY_TOKEN, Arc::new(forwarder))
        .with_runtime_sinks(history.clone(), diagnostics.clone());
    proxy_state.set_active_route(Some(snapshot));
    let proxy = ProxyServerHandle::start(0, build_proxy_router(proxy_state))
        .await
        .expect("local proxy");

    let request = chat_request(true);
    let response = reqwest::Client::new()
        .post(format!("http://{}/v1/responses", proxy.address()))
        .bearer_auth(GATEWAY_TOKEN)
        .header(header::CONTENT_TYPE, "application/json")
        .body(request.to_string())
        .send()
        .await
        .expect("proxy request");
    assert_eq!(response.status(), StatusCode::OK);
    let _ = response.text().await.expect("body");

    let captured = responses_state.lock().expect("capture mutex").clone();
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].0, "/v1/responses");
    let forwarded_body: Value = serde_json::from_slice(&captured[0].1).expect("forwarded body");
    assert_eq!(
        forwarded_body, request,
        "a Responses attempt must forward the client body unchanged"
    );

    history.shutdown().await;
    proxy.shutdown().await;
    upstream.shutdown().await;
}

/// Synthetic body of a hop that fails with a JSON error envelope.
const HOP_ERROR_BODY: &str =
    r#"{"error":{"code":"invalid_request_error","message":"synthetic upstream rejection"}}"#;
/// Marker only the Responses hop emits, so the committed body is unambiguous.
const RESPONSES_SENTINEL: &str = "RESPONSES_HOP_SENTINEL_7d3a";

/// What one synthetic hop answers with.
struct HopPlan {
    status: StatusCode,
    content_type: &'static str,
    body: String,
}

#[derive(Clone)]
struct HopState {
    requests: Arc<Mutex<Vec<CapturedRequest>>>,
    status: StatusCode,
    content_type: &'static str,
    body: Arc<String>,
}

struct HopUpstream {
    server: ProxyServerHandle,
    state: HopState,
}

impl HopUpstream {
    async fn start(plan: HopPlan) -> Self {
        let state = HopState {
            requests: Arc::new(Mutex::new(Vec::new())),
            status: plan.status,
            content_type: plan.content_type,
            body: Arc::new(plan.body),
        };
        let server = ProxyServerHandle::start(
            0,
            Router::new()
                .route("/v1/responses", post(hop_upstream_handler))
                .route("/v1/chat/completions", post(hop_upstream_handler))
                .with_state(state.clone()),
        )
        .await
        .expect("mock hop upstream");
        Self { server, state }
    }

    fn captured(&self) -> Vec<(String, Bytes)> {
        self.state
            .requests
            .lock()
            .expect("hop capture mutex")
            .iter()
            .map(|request| (request.path.clone(), request.body.clone()))
            .collect()
    }

    async fn shutdown(self) {
        self.server.shutdown().await;
    }
}

async fn hop_upstream_handler(State(state): State<HopState>, request: Request) -> Response {
    let path = request.uri().path().to_owned();
    let headers = request.headers().clone();
    let body = to_bytes(request.into_body(), 1024 * 1024)
        .await
        .expect("mock hop request body");
    state
        .requests
        .lock()
        .expect("hop capture mutex")
        .push(CapturedRequest {
            path,
            headers,
            body,
        });
    Response::builder()
        .status(state.status)
        .header(header::CONTENT_TYPE, state.content_type)
        .body(Body::from(state.body.as_str().to_owned()))
        .expect("hop response")
}

/// Test-local `FallbackActivator`: the production `InMemoryFallbackActivator`
/// is `#[cfg(test)]` inside `proxy/upstream.rs` and is not importable here.
struct LocalFallbackActivator {
    routing: RoutingSnapshotStore,
    activations: Mutex<Vec<(RouteId, RouteId)>>,
}

impl LocalFallbackActivator {
    fn new(routing: RoutingSnapshotStore) -> Self {
        Self {
            routing,
            activations: Mutex::new(Vec::new()),
        }
    }

    fn activations(&self) -> Vec<(RouteId, RouteId)> {
        self.activations.lock().expect("activation mutex").clone()
    }
}

#[async_trait]
impl FallbackActivator for LocalFallbackActivator {
    async fn activate_next(
        &self,
        request: FallbackActivationRequest,
    ) -> Result<Option<Arc<RoutingSnapshot>>, FallbackActivationError> {
        let current = self.routing.load();
        let current_index = current
            .participants
            .iter()
            .position(|route| route.route_id == request.current_route_id);
        let target_index = current
            .participants
            .iter()
            .position(|route| route.route_id == request.target_route.route_id);
        let advances = matches!(
            (current_index, target_index),
            (Some(current_index), Some(target_index)) if target_index > current_index
        );
        let active_matches =
            current.active.as_ref().map(|route| &route.route_id) == Some(&request.current_route_id);
        if !current.enabled || !advances || !active_matches {
            return Ok(None);
        }
        let snapshot = Arc::new(RoutingSnapshot {
            active: Some(Arc::clone(&request.target_route)),
            participants: current.participants.clone(),
            configured_participant_count: current.configured_participant_count,
            enabled: true,
            selection_generation: current.selection_generation.saturating_add(1),
            health_generation: current.health_generation.saturating_add(1),
            config_revision: current.config_revision,
            images_generation_enabled: current.images_generation_enabled,
            images_route: current.images_route.clone(),
            images_generation_timeout: current.images_generation_timeout,
            images_generation_model: Arc::clone(&current.images_generation_model),
        });
        self.activations.lock().expect("activation mutex").push((
            request.current_route_id,
            request.target_route.route_id.clone(),
        ));
        self.routing.store(Arc::clone(&snapshot));
        Ok(Some(snapshot))
    }
}

/// Two-route harness: participant A is active, participant B is its successor.
struct MixedHarness {
    proxy: ProxyServerHandle,
    a: HopUpstream,
    b: HopUpstream,
    a_route_id: RouteId,
    b_route_id: RouteId,
    history: Arc<AsyncHistoryRecorder>,
    database: DatabaseExecutor,
    activator: Arc<LocalFallbackActivator>,
    _temporary: TempDir,
}

impl MixedHarness {
    async fn start(
        a_protocol: RouteProtocol,
        a_plan: HopPlan,
        b_protocol: RouteProtocol,
        b_plan: HopPlan,
    ) -> Self {
        let a = HopUpstream::start(a_plan).await;
        let b = HopUpstream::start(b_plan).await;

        let temporary = TempDir::new().expect("temporary directory");
        let database_path = temporary.path().join("data/router.sqlite3");
        let database = DatabaseExecutor::open(&database_path).expect("database");
        let a_route = database
            .create_route(CreateRouteInput {
                name: "fallback participant A".to_owned(),
                base_url: format!("http://{}/v1", a.server.address()),
                protocol: Some(a_protocol),
                api_key: ApiKey::parse(ROUTE_API_KEY).expect("route API Key"),
                menu_visible: None,
                balance_query: None,
                accept_script_risk: false,
            })
            .await
            .expect("route A");
        let b_route = database
            .create_route(CreateRouteInput {
                name: "fallback participant B".to_owned(),
                base_url: format!("http://{}/v1", b.server.address()),
                protocol: Some(b_protocol),
                api_key: ApiKey::parse(ROUTE_API_KEY).expect("route API Key"),
                menu_visible: None,
                balance_query: None,
                accept_script_risk: false,
            })
            .await
            .expect("route B");
        database
            .get_or_create_singleton_secret(
                "gateway_token".to_owned(),
                ApiKey::parse(GATEWAY_TOKEN).expect("gateway token"),
            )
            .await
            .expect("stored gateway token");

        let diagnostics = Arc::new(DiagnosticCapture::default());
        let history = AsyncHistoryRecorder::new(
            database.clone(),
            diagnostics.clone(),
            Arc::new(NoopHistoryChanges),
        );
        let a_snapshot = Arc::new(RouteSnapshot {
            route_id: a_route.route_id.clone(),
            name: a_route.name.clone(),
            protocol: a_protocol,
            base_url: BaseUrl::parse(&a_route.base_url, a_protocol).expect("base URL"),
            api_key: Arc::new(ApiKey::parse(ROUTE_API_KEY).expect("route API Key")),
            fallback_excluded_models: Arc::new(HashSet::new()),
        });
        let b_snapshot = Arc::new(RouteSnapshot {
            route_id: b_route.route_id.clone(),
            name: b_route.name.clone(),
            protocol: b_protocol,
            base_url: BaseUrl::parse(&b_route.base_url, b_protocol).expect("base URL"),
            api_key: Arc::new(ApiKey::parse(ROUTE_API_KEY).expect("route API Key")),
            fallback_excluded_models: Arc::new(HashSet::new()),
        });
        let routing = RoutingSnapshotStore::new(RoutingSnapshot {
            active: Some(Arc::clone(&a_snapshot)),
            participants: vec![a_snapshot, b_snapshot],
            configured_participant_count: 2,
            enabled: true,
            selection_generation: 7,
            health_generation: 7,
            config_revision: 11,
            images_generation_enabled: false,
            images_route: None,
            images_generation_timeout: Duration::from_secs(5),
            images_generation_model: Arc::from("gpt-image-1"),
        });
        let activator = Arc::new(LocalFallbackActivator::new(routing.clone()));
        let forwarder = ResponsesForwarder::new()
            .expect("forwarder")
            .with_runtime_services(
                history.clone(),
                diagnostics.clone(),
                InferenceStatusService::new(Arc::new(NoopInferenceChanges)),
            )
            .with_fallback_services(routing.clone(), activator.clone());
        let proxy_state = ProxyIngressState::new(GATEWAY_TOKEN, Arc::new(forwarder))
            .with_routing_store(routing)
            .with_runtime_sinks(history.clone(), diagnostics.clone());
        let proxy = ProxyServerHandle::start(0, build_proxy_router(proxy_state))
            .await
            .expect("local proxy");

        Self {
            proxy,
            a,
            b,
            a_route_id: a_route.route_id,
            b_route_id: b_route.route_id,
            history,
            database,
            activator,
            _temporary: temporary,
        }
    }

    async fn send(&self, body: Value) -> (StatusCode, String) {
        let response = reqwest::Client::new()
            .post(format!("http://{}/v1/responses", self.proxy.address()))
            .bearer_auth(GATEWAY_TOKEN)
            .header(header::CONTENT_TYPE, "application/json")
            .body(body.to_string())
            .send()
            .await
            .expect("proxy request");
        let status = response.status();
        let text = response.text().await.expect("proxy body");
        (status, text)
    }

    async fn shutdown(self) {
        self.history.shutdown().await;
        self.proxy.shutdown().await;
        self.a.shutdown().await;
        self.b.shutdown().await;
    }
}

fn responses_sse_sentinel() -> String {
    format!(
        "event: response.output_text.delta\n\
         data: {{\"type\":\"response.output_text.delta\",\"delta\":\"{RESPONSES_SENTINEL}\"}}\n\n\
         data: {{\"type\":\"response.completed\",\"response\":{{\"status\":\"completed\"}}}}\n\n\
         data: [DONE]\n\n"
    )
}

/// Proves the client sees B's Responses SSE while each hop receives its own
/// wire format: A (Chat) got the translated Chat body, B (Responses) the
/// original Responses body.
#[tokio::test]
async fn chat_then_responses_fallback_keeps_each_wire_format() {
    let harness = MixedHarness::start(
        RouteProtocol::ChatCompletions,
        HopPlan {
            status: StatusCode::TOO_MANY_REQUESTS,
            content_type: "application/json",
            body: HOP_ERROR_BODY.to_owned(),
        },
        RouteProtocol::Responses,
        HopPlan {
            status: StatusCode::OK,
            content_type: "text/event-stream",
            body: responses_sse_sentinel(),
        },
    )
    .await;

    let client_body = chat_request(true);
    let (status, body) = harness.send(client_body.clone()).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains(RESPONSES_SENTINEL),
        "the client must receive B's Responses SSE, got {body}"
    );
    assert_eq!(
        terminal_payload(&body)["response"]["status"],
        json!("completed"),
        "B's Responses SSE must complete the turn"
    );

    // Ordering: A was attempted first and only then did activation move to B.
    assert_eq!(
        harness.activator.activations(),
        vec![(harness.a_route_id.clone(), harness.b_route_id.clone())],
        "exactly one A -> B activation"
    );

    let a_captured = harness.a.captured();
    assert_eq!(a_captured.len(), 1, "A must be attempted exactly once");
    assert_eq!(a_captured[0].0, "/v1/chat/completions");
    let a_body: Value = serde_json::from_slice(&a_captured[0].1).expect("A translated body");
    assert!(
        a_body.get("messages").is_some(),
        "A is a Chat route and must receive the translated Chat body"
    );
    assert!(
        a_body.get("input").is_none(),
        "the Responses `input` field must not leak into a Chat attempt"
    );

    let b_captured = harness.b.captured();
    assert_eq!(b_captured.len(), 1, "B must be attempted exactly once");
    assert_eq!(b_captured[0].0, "/v1/responses");
    let b_body: Value = serde_json::from_slice(&b_captured[0].1).expect("B forwarded body");
    assert!(
        b_body.get("input").is_some() && b_body.get("tools").is_some(),
        "B is a Responses route and must receive the original Responses body"
    );
    assert!(
        b_body.get("messages").is_none(),
        "a Responses attempt must never receive a Chat `messages` array"
    );
    assert_eq!(
        b_body, client_body,
        "a Responses attempt forwards the client body unchanged"
    );

    // The recorder persists on a worker task; drain it before reading back.
    harness.history.shutdown().await;
    let attempts = harness
        .database
        .latest_inference_attempts()
        .await
        .expect("attempts");
    assert!(
        attempts
            .iter()
            .any(|attempt| attempt.route_id == harness.a_route_id && !attempt.succeeded),
        "A must be recorded as a failed attempt"
    );
    assert!(
        attempts
            .iter()
            .any(|attempt| attempt.route_id == harness.b_route_id && attempt.succeeded),
        "B must be recorded as the successful attempt"
    );

    harness.shutdown().await;
}

/// Mirrors the first case: A (Responses) fails, B (Chat) answers, and the
/// client still sees a Responses SSE produced from B's translated Chat body.
#[tokio::test]
async fn responses_then_chat_fallback_keeps_each_wire_format() {
    let harness = MixedHarness::start(
        RouteProtocol::Responses,
        HopPlan {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            content_type: "application/json",
            body: HOP_ERROR_BODY.to_owned(),
        },
        RouteProtocol::ChatCompletions,
        HopPlan {
            status: StatusCode::OK,
            content_type: "text/event-stream",
            body: chat_sse("bridged fallback", false, "stop"),
        },
    )
    .await;

    let client_body = chat_request(true);
    let (status, body) = harness.send(client_body.clone()).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("event: response.completed"),
        "the client must see the Responses SSE translated from B's Chat stream"
    );
    assert_eq!(
        body.matches("data: [DONE]").count(),
        1,
        "the bridged stream must terminate with exactly one [DONE]"
    );

    assert_eq!(
        harness.activator.activations(),
        vec![(harness.a_route_id.clone(), harness.b_route_id.clone())],
        "exactly one A -> B activation"
    );

    let a_captured = harness.a.captured();
    assert_eq!(a_captured.len(), 1, "A must be attempted exactly once");
    assert_eq!(a_captured[0].0, "/v1/responses");
    let a_body: Value = serde_json::from_slice(&a_captured[0].1).expect("A forwarded body");
    assert_eq!(
        a_body, client_body,
        "the Responses hop forwards the client body unchanged"
    );

    let b_captured = harness.b.captured();
    assert_eq!(b_captured.len(), 1, "B must be attempted exactly once");
    assert_eq!(b_captured[0].0, "/v1/chat/completions");
    let b_body: Value = serde_json::from_slice(&b_captured[0].1).expect("B translated body");
    assert!(
        b_body.get("messages").is_some(),
        "B is a Chat route and must receive the translated Chat body"
    );
    assert!(
        b_body.get("input").is_none(),
        "the Responses `input` field must not leak into a Chat attempt"
    );

    harness.shutdown().await;
}
