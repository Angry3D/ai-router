use std::time::{Duration, Instant};

use axum::http::{HeaderValue, header};
use serde_json::Value;
use thiserror::Error;
use zeroize::Zeroizing;

use crate::{
    domain::{ApiKey, BaseUrl},
    proxy::{
        OutboundHttpClient, OutboundProxyTransport,
        upstream::{DecodeError, decode_supported, response_encodings},
    },
};

const MAX_MODELS_RESPONSE_BYTES: usize = 1024 * 1024;
const MAX_MODELS_ENTRIES: usize = 1024;
const MAX_MODEL_ID_BYTES: usize = 256;

/// Safe failure category for one upstream model-list query.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UpstreamModelsErrorKind {
    Unauthorized,
    NotFound,
    Network,
    Timeout,
    HttpStatus,
    TooLarge,
    InvalidResponse,
}

/// Bounded model discovery failure without upstream bodies or credentials.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[error("upstream model discovery failed")]
pub struct UpstreamModelsError {
    pub kind: UpstreamModelsErrorKind,
    pub transient: bool,
}

impl UpstreamModelsError {
    const fn deterministic(kind: UpstreamModelsErrorKind) -> Self {
        Self {
            kind,
            transient: false,
        }
    }

    const fn retryable(kind: UpstreamModelsErrorKind) -> Self {
        Self {
            kind,
            transient: true,
        }
    }
}

/// Queries the configured upstream's `/models` endpoint for one route.
pub struct UpstreamModelsClient {
    client: OutboundHttpClient,
    attempt_timeout: Duration,
    retry_delay: Duration,
}

impl UpstreamModelsClient {
    /// Creates the dedicated model discovery HTTP client.
    ///
    /// # Errors
    ///
    /// Returns a client-construction error.
    pub fn new() -> Result<Self, reqwest::Error> {
        Self::new_with_outbound_proxy(&OutboundProxyTransport::default())
    }

    /// Creates the model discovery client backed by the shared outbound proxy.
    ///
    /// # Errors
    ///
    /// Returns a client-construction error.
    pub fn new_with_outbound_proxy(
        outbound_proxy: &OutboundProxyTransport,
    ) -> Result<Self, reqwest::Error> {
        Self::with_timing_and_outbound_proxy(
            Duration::from_secs(10),
            Duration::from_millis(1_500),
            outbound_proxy,
        )
    }

    #[cfg(test)]
    fn with_timing(
        attempt_timeout: Duration,
        retry_delay: Duration,
    ) -> Result<Self, reqwest::Error> {
        Self::with_timing_and_outbound_proxy(
            attempt_timeout,
            retry_delay,
            &OutboundProxyTransport::default(),
        )
    }

    fn with_timing_and_outbound_proxy(
        attempt_timeout: Duration,
        retry_delay: Duration,
        outbound_proxy: &OutboundProxyTransport,
    ) -> Result<Self, reqwest::Error> {
        let client = OutboundHttpClient::new(outbound_proxy.clone(), || {
            reqwest::Client::builder().redirect(reqwest::redirect::Policy::custom(|attempt| {
                if !matches!(attempt.url().scheme(), "http" | "https") {
                    return attempt.stop();
                }
                if attempt.previous().len() >= 10 {
                    attempt.stop()
                } else {
                    attempt.follow()
                }
            }))
        })?;
        Ok(Self {
            client,
            attempt_timeout,
            retry_delay,
        })
    }

    /// Lists the upstream model identifiers with at most one transient retry.
    ///
    /// # Errors
    ///
    /// Returns a bounded category error without request or response content.
    pub async fn list(
        &self,
        api_key: &ApiKey,
        base_url: &BaseUrl,
    ) -> Result<Vec<String>, UpstreamModelsError> {
        let first = self.attempt(api_key, base_url).await;
        if first.as_ref().is_err_and(|error| error.transient) {
            tokio::time::sleep(self.retry_delay).await;
            self.attempt(api_key, base_url).await
        } else {
            first
        }
    }

    async fn attempt(
        &self,
        api_key: &ApiKey,
        base_url: &BaseUrl,
    ) -> Result<Vec<String>, UpstreamModelsError> {
        let started = Instant::now();
        // Both the canonical prefix and a parsed API key always yield a valid
        // request, so the bounded failure here stays unreachable in practice.
        let url = url::Url::parse(&base_url.models_url()).map_err(|_| invalid_response())?;
        let authorization = authorization_header(api_key)?;
        let client = self
            .client
            .client()
            .map_err(|_| UpstreamModelsError::retryable(UpstreamModelsErrorKind::Network))?;
        let response = tokio::time::timeout(
            remaining(self.attempt_timeout, started)?,
            client
                .get(url)
                .header(header::AUTHORIZATION, authorization)
                .header(header::ACCEPT, "application/json")
                .header(header::ACCEPT_ENCODING, "identity")
                .send(),
        )
        .await
        .map_err(|_| timeout_error())?
        .map_err(|_| UpstreamModelsError::retryable(UpstreamModelsErrorKind::Network))?;
        let status = response.status();
        if !status.is_success() {
            return Err(match status {
                reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN => {
                    UpstreamModelsError::deterministic(UpstreamModelsErrorKind::Unauthorized)
                }
                reqwest::StatusCode::NOT_FOUND => {
                    UpstreamModelsError::deterministic(UpstreamModelsErrorKind::NotFound)
                }
                _ if status == reqwest::StatusCode::TOO_MANY_REQUESTS
                    || status.is_server_error() =>
                {
                    UpstreamModelsError::retryable(UpstreamModelsErrorKind::HttpStatus)
                }
                _ => UpstreamModelsError::deterministic(UpstreamModelsErrorKind::HttpStatus),
            });
        }
        let encodings = response_encodings(response.headers());
        let wire =
            collect_models_response(response, remaining(self.attempt_timeout, started)?).await?;
        let decoded = match decode_supported(wire, &encodings, MAX_MODELS_RESPONSE_BYTES) {
            Ok(decoded) => decoded,
            Err(DecodeError::TooLarge) => {
                return Err(UpstreamModelsError::deterministic(
                    UpstreamModelsErrorKind::TooLarge,
                ));
            }
            Err(DecodeError::Unsupported | DecodeError::Invalid) => {
                return Err(invalid_response());
            }
        };
        parse_models(&decoded)
    }
}

/// Builds the single-request credential header.
///
/// A parsed `ApiKey` rejects everything a header value cannot carry, so the
/// bounded failure below stays unreachable for validated input.
fn authorization_header(api_key: &ApiKey) -> Result<HeaderValue, UpstreamModelsError> {
    let mut authorization = Zeroizing::new(Vec::with_capacity(api_key.expose().len() + 7));
    authorization.extend_from_slice(b"Bearer ");
    authorization.extend_from_slice(api_key.expose());
    HeaderValue::from_bytes(&authorization).map_err(|_| invalid_response())
}

fn remaining(limit: Duration, started: Instant) -> Result<Duration, UpstreamModelsError> {
    let remaining = limit.saturating_sub(started.elapsed());
    if remaining.is_zero() {
        Err(timeout_error())
    } else {
        Ok(remaining)
    }
}

const fn timeout_error() -> UpstreamModelsError {
    UpstreamModelsError::retryable(UpstreamModelsErrorKind::Timeout)
}

const fn invalid_response() -> UpstreamModelsError {
    UpstreamModelsError::deterministic(UpstreamModelsErrorKind::InvalidResponse)
}

async fn collect_models_response(
    response: reqwest::Response,
    timeout: Duration,
) -> Result<Vec<u8>, UpstreamModelsError> {
    use futures_util::StreamExt;

    tokio::time::timeout(timeout, async move {
        let mut stream = response.bytes_stream();
        let mut wire = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk
                .map_err(|_| UpstreamModelsError::retryable(UpstreamModelsErrorKind::Network))?;
            if wire.len().saturating_add(chunk.len()) > MAX_MODELS_RESPONSE_BYTES {
                return Err(UpstreamModelsError::deterministic(
                    UpstreamModelsErrorKind::TooLarge,
                ));
            }
            wire.extend_from_slice(&chunk);
        }
        Ok(wire)
    })
    .await
    .map_err(|_| timeout_error())?
}

fn parse_models(decoded: &[u8]) -> Result<Vec<String>, UpstreamModelsError> {
    let value = serde_json::from_slice::<Value>(decoded).map_err(|_| invalid_response())?;
    let entries = value
        .get("data")
        .and_then(Value::as_array)
        .or_else(|| value.get("models").and_then(Value::as_array))
        .ok_or_else(invalid_response)?;
    let mut models: Vec<String> = Vec::new();
    for entry in entries {
        let Some(id) = model_entry_id(entry) else {
            continue;
        };
        let id = id.trim();
        if id.is_empty() || id.len() > MAX_MODEL_ID_BYTES || id.chars().any(char::is_control) {
            continue;
        }
        if models.iter().any(|existing| existing == id) {
            continue;
        }
        models.push(id.to_owned());
        if models.len() == MAX_MODELS_ENTRIES {
            break;
        }
    }
    Ok(models)
}

fn model_entry_id(entry: &Value) -> Option<&str> {
    match entry {
        Value::String(id) => Some(id),
        Value::Object(object) => object.get("id").and_then(Value::as_str),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    use axum::{
        Router,
        body::{Body, Bytes},
        extract::{Request, State},
        http::{HeaderMap, StatusCode},
        response::{IntoResponse, Response},
    };
    use tokio::{net::TcpListener, sync::oneshot, task::JoinHandle};

    use super::*;
    use crate::domain::RouteProtocol;

    fn key() -> ApiKey {
        ApiKey::parse("route-secret").expect("API key")
    }

    fn base(value: &str) -> BaseUrl {
        BaseUrl::parse(value, RouteProtocol::Responses).expect("base URL")
    }

    #[derive(Clone)]
    struct MockModelsState {
        calls: Arc<AtomicUsize>,
        statuses: Arc<Vec<StatusCode>>,
        response: Bytes,
        delay: Duration,
        requests: Arc<Mutex<Vec<(String, String, HeaderMap)>>>,
    }

    impl MockModelsState {
        fn ok(response: impl Into<Bytes>) -> Self {
            Self {
                calls: Arc::new(AtomicUsize::new(0)),
                statuses: Arc::new(vec![StatusCode::OK]),
                response: response.into(),
                delay: Duration::ZERO,
                requests: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn with_statuses(statuses: Vec<StatusCode>) -> Self {
            Self {
                statuses: Arc::new(statuses),
                ..Self::ok(Bytes::new())
            }
        }
    }

    async fn mock_models_handler(
        State(state): State<MockModelsState>,
        request: Request,
    ) -> Response {
        let call = state.calls.fetch_add(1, Ordering::SeqCst);
        state.requests.lock().expect("request mutex").push((
            request.method().to_string(),
            request.uri().to_string(),
            request.headers().clone(),
        ));
        tokio::time::sleep(state.delay).await;
        let status = state
            .statuses
            .get(call)
            .copied()
            .or_else(|| state.statuses.last().copied())
            .unwrap_or(StatusCode::OK);
        (status, Body::from(state.response.clone())).into_response()
    }

    struct MockModelsServer {
        address: std::net::SocketAddr,
        shutdown: Option<oneshot::Sender<()>>,
        task: JoinHandle<std::io::Result<()>>,
    }

    impl MockModelsServer {
        async fn start(state: MockModelsState) -> Self {
            let listener = TcpListener::bind(("127.0.0.1", 0))
                .await
                .expect("mock models listener");
            let address = listener.local_addr().expect("mock models address");
            let (shutdown, receiver) = oneshot::channel();
            let router = Router::new()
                .fallback(mock_models_handler)
                .with_state(state);
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
                task,
            }
        }

        fn base_url(&self) -> String {
            format!("http://{}/v1", self.address)
        }

        async fn shutdown(mut self) {
            if let Some(shutdown) = self.shutdown.take() {
                let _ = shutdown.send(());
            }
            let _ = self.task.await;
        }
    }

    fn client() -> UpstreamModelsClient {
        UpstreamModelsClient::with_timing(Duration::from_secs(1), Duration::from_millis(1))
            .expect("models client")
    }

    fn json_response(value: &Value) -> Bytes {
        Bytes::from(serde_json::to_vec(value).expect("mock response JSON"))
    }

    #[tokio::test]
    async fn upstream_models_uses_the_canonical_prefix_and_exact_key() {
        let state = MockModelsState::ok(json_response(&serde_json::json!({
            "object": "list",
            "data": [{"id": "gpt-5", "object": "model"}],
        })));
        let requests = Arc::clone(&state.requests);
        let calls = Arc::clone(&state.calls);
        let server = MockModelsServer::start(state).await;

        let models = client()
            .list(
                &ApiKey::parse("exact-route-key").expect("key"),
                &base(&server.base_url()),
            )
            .await
            .expect("model list");

        assert_eq!(models, vec!["gpt-5".to_owned()]);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        {
            let requests = requests.lock().expect("request mutex");
            let (method, uri, headers) = requests.first().expect("one request");
            assert_eq!(method, "GET");
            assert_eq!(uri, "/v1/models");
            assert_eq!(
                headers.get("authorization"),
                Some(&HeaderValue::from_static("Bearer exact-route-key"))
            );
            assert_eq!(
                headers.get("accept"),
                Some(&HeaderValue::from_static("application/json"))
            );
            assert_eq!(
                headers.get("accept-encoding"),
                Some(&HeaderValue::from_static("identity"))
            );
        }
        server.shutdown().await;
    }

    #[tokio::test]
    async fn upstream_models_reads_both_supported_shapes() {
        for (response, expected) in [
            (
                serde_json::json!({
                    "object": "list",
                    "data": [
                        {"id": "gpt-5"},
                        {"id": "  gpt-4o  "},
                        {"id": "gpt-5"},
                        {"id": ""},
                        {"id": 7},
                        {"name": "missing-id"},
                        42,
                        "gpt-4.1",
                    ],
                }),
                vec!["gpt-5", "gpt-4o", "gpt-4.1"],
            ),
            (
                serde_json::json!({"data": [{"id": "a"}, "b", " a "] }),
                vec!["a", "b"],
            ),
            (
                serde_json::json!({"models": ["a", "b", "a"]}),
                vec!["a", "b"],
            ),
            (
                serde_json::json!({"models": [{"id": "a"}, {"id": "b"}, {"id": "a"}]}),
                vec!["a", "b"],
            ),
            (
                // A present `data` array wins over `models`, even when no entry is usable.
                serde_json::json!({"data": [1], "models": ["a"]}),
                Vec::new(),
            ),
            (serde_json::json!({"data": []}), Vec::new()),
            (serde_json::json!({"models": []}), Vec::new()),
        ] {
            let server =
                MockModelsServer::start(MockModelsState::ok(json_response(&response))).await;
            let models = client()
                .list(&key(), &base(&server.base_url()))
                .await
                .expect("model list");
            assert_eq!(models, expected, "response: {response}");
            server.shutdown().await;
        }
    }

    #[tokio::test]
    async fn upstream_models_rejects_unsupported_shapes() {
        for response in [
            Bytes::from_static(b"not json"),
            Bytes::from_static(b"[]"),
            Bytes::from_static(b"\"models\""),
            json_response(&serde_json::json!({"object": "list"})),
            json_response(&serde_json::json!({"data": {"id": "a"}})),
            json_response(&serde_json::json!({"data": {}, "models": {}})),
        ] {
            let state = MockModelsState::ok(response);
            let calls = Arc::clone(&state.calls);
            let server = MockModelsServer::start(state).await;
            let error = client()
                .list(&key(), &base(&server.base_url()))
                .await
                .expect_err("unsupported shape");
            assert_eq!(error.kind, UpstreamModelsErrorKind::InvalidResponse);
            assert!(!error.transient);
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            server.shutdown().await;
        }
    }

    #[tokio::test]
    async fn upstream_models_filters_entries_and_truncates_the_list() {
        let oversized = "m".repeat(MAX_MODEL_ID_BYTES + 1);
        let boundary = "n".repeat(MAX_MODEL_ID_BYTES);
        let mut entries = vec![
            Value::String(oversized),
            Value::String(boundary.clone()),
            Value::String("  ".to_owned()),
            Value::String("bad\u{0007}id".to_owned()),
            Value::String("first".to_owned()),
            Value::String("FIRST".to_owned()),
        ];
        entries
            .extend((0..MAX_MODELS_ENTRIES).map(|index| Value::String(format!("model-{index}"))));
        entries.push(Value::String("after-capacity".to_owned()));

        let server = MockModelsServer::start(MockModelsState::ok(json_response(
            &serde_json::json!({"data": entries}),
        )))
        .await;
        let models = client()
            .list(&key(), &base(&server.base_url()))
            .await
            .expect("model list");

        assert_eq!(models.len(), MAX_MODELS_ENTRIES);
        assert_eq!(models[0], boundary);
        assert_eq!(models[1], "first");
        assert_eq!(models[2], "FIRST");
        assert_eq!(models[MAX_MODELS_ENTRIES - 1], "model-1020");
        assert!(!models.iter().any(|model| model == "after-capacity"));
        server.shutdown().await;
    }

    #[tokio::test]
    async fn upstream_models_maps_status_codes_without_retrying_deterministic_failures() {
        for (status, kind, transient) in [
            (
                StatusCode::UNAUTHORIZED,
                UpstreamModelsErrorKind::Unauthorized,
                false,
            ),
            (
                StatusCode::FORBIDDEN,
                UpstreamModelsErrorKind::Unauthorized,
                false,
            ),
            (
                StatusCode::NOT_FOUND,
                UpstreamModelsErrorKind::NotFound,
                false,
            ),
            (
                StatusCode::BAD_REQUEST,
                UpstreamModelsErrorKind::HttpStatus,
                false,
            ),
            (
                StatusCode::TOO_MANY_REQUESTS,
                UpstreamModelsErrorKind::HttpStatus,
                true,
            ),
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                UpstreamModelsErrorKind::HttpStatus,
                true,
            ),
            (
                StatusCode::BAD_GATEWAY,
                UpstreamModelsErrorKind::HttpStatus,
                true,
            ),
        ] {
            let mut state = MockModelsState::with_statuses(vec![status]);
            let calls = Arc::clone(&state.calls);
            state.response = json_response(&serde_json::json!({"data": [{"id": "gpt-5"}]}));
            let server = MockModelsServer::start(state).await;
            let error = client()
                .list(&key(), &base(&server.base_url()))
                .await
                .expect_err("status failure");
            assert_eq!(error.kind, kind, "status: {status}");
            assert_eq!(error.transient, transient, "status: {status}");
            assert_eq!(
                calls.load(Ordering::SeqCst),
                if transient { 2 } else { 1 },
                "status: {status}"
            );
            server.shutdown().await;
        }
    }

    #[tokio::test]
    async fn upstream_models_retries_a_transient_status_once() {
        let mut state =
            MockModelsState::with_statuses(vec![StatusCode::INTERNAL_SERVER_ERROR, StatusCode::OK]);
        let calls = Arc::clone(&state.calls);
        state.response = json_response(&serde_json::json!({"data": [{"id": "gpt-5"}]}));
        let server = MockModelsServer::start(state).await;
        let models = client()
            .list(&key(), &base(&server.base_url()))
            .await
            .expect("retried model list");
        assert_eq!(models, vec!["gpt-5".to_owned()]);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        server.shutdown().await;

        let mut repeated = MockModelsState::with_statuses(vec![
            StatusCode::INTERNAL_SERVER_ERROR,
            StatusCode::INTERNAL_SERVER_ERROR,
        ]);
        let repeated_calls = Arc::clone(&repeated.calls);
        repeated.response = json_response(&serde_json::json!({"data": [{"id": "gpt-5"}]}));
        let repeated_server = MockModelsServer::start(repeated).await;
        let error = client()
            .list(&key(), &base(&repeated_server.base_url()))
            .await
            .expect_err("repeated 5xx");
        assert_eq!(error.kind, UpstreamModelsErrorKind::HttpStatus);
        assert!(error.transient);
        assert_eq!(repeated_calls.load(Ordering::SeqCst), 2);
        repeated_server.shutdown().await;
    }

    #[tokio::test]
    async fn upstream_models_bounds_timeouts_network_failures_and_response_size() {
        let mut slow = MockModelsState::ok(json_response(&serde_json::json!({"data": []})));
        slow.delay = Duration::from_millis(60);
        let slow_calls = Arc::clone(&slow.calls);
        let slow_server = MockModelsServer::start(slow).await;
        let timeout =
            UpstreamModelsClient::with_timing(Duration::from_millis(10), Duration::from_millis(1))
                .expect("models client")
                .list(&key(), &base(&slow_server.base_url()))
                .await
                .expect_err("timeout");
        assert_eq!(timeout.kind, UpstreamModelsErrorKind::Timeout);
        assert!(timeout.transient);
        assert_eq!(slow_calls.load(Ordering::SeqCst), 2);
        slow_server.shutdown().await;

        let released = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("released listener");
        let released_address = released.local_addr().expect("released address");
        drop(released);
        let network = client()
            .list(&key(), &base(&format!("http://{released_address}/v1")))
            .await
            .expect_err("connection refused");
        assert_eq!(network.kind, UpstreamModelsErrorKind::Network);
        assert!(network.transient);

        let large = MockModelsState::ok(Bytes::from(vec![b'x'; MAX_MODELS_RESPONSE_BYTES + 1]));
        let large_calls = Arc::clone(&large.calls);
        let large_server = MockModelsServer::start(large).await;
        let oversized = client()
            .list(&key(), &base(&large_server.base_url()))
            .await
            .expect_err("oversized response");
        assert_eq!(oversized.kind, UpstreamModelsErrorKind::TooLarge);
        assert!(!oversized.transient);
        assert_eq!(large_calls.load(Ordering::SeqCst), 1);
        large_server.shutdown().await;
    }
}
