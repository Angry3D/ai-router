use std::{
    future::Future,
    io,
    pin::Pin,
    task::{Context, Poll},
    time::{Duration, Instant},
};

use axum::body::Bytes;
use futures_util::Stream;
use serde::{Deserialize, Deserializer, de::Visitor};

use super::fallback::normalize_semantic_error_code;

const MAX_SSE_EVENT_BYTES: usize = 1024 * 1024;
pub(super) const MAX_RESPONSE_ID_CHARS: usize = 512;
pub(super) const MAX_MODEL_CHARS: usize = 256;
pub(super) const MAX_STATUS_CHARS: usize = 64;
pub(super) const MAX_ERROR_CODE_CHARS: usize = 128;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ResponseToolHandoff {
    #[default]
    None,
    Client,
    Shell,
    ClientAndShell,
}

impl ResponseToolHandoff {
    pub(super) const fn with_client(self) -> Self {
        match self {
            Self::None | Self::Client => Self::Client,
            Self::Shell | Self::ClientAndShell => Self::ClientAndShell,
        }
    }

    pub(super) const fn with_shell(self) -> Self {
        match self {
            Self::None | Self::Shell => Self::Shell,
            Self::Client | Self::ClientAndShell => Self::ClientAndShell,
        }
    }

    pub(super) const fn has_client(self) -> bool {
        matches!(self, Self::Client | Self::ClientAndShell)
    }

    pub(super) const fn has_shell(self) -> bool {
        matches!(self, Self::Shell | Self::ClientAndShell)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResponseMetadata {
    pub response_id: Option<String>,
    pub model: Option<String>,
    pub service_tier: Option<String>,
    pub status: Option<String>,
    pub safe_error_code: Option<String>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub cached_input_tokens: Option<u64>,
    pub cache_write_input_tokens: Option<u64>,
    pub first_output_latency_ms: Option<u64>,
    pub tool_handoff: ResponseToolHandoff,
    pub terminal_success: bool,
    pub complete: bool,
}

impl Default for ResponseMetadata {
    fn default() -> Self {
        Self {
            response_id: None,
            model: None,
            service_tier: None,
            status: None,
            safe_error_code: None,
            input_tokens: None,
            output_tokens: None,
            total_tokens: None,
            cached_input_tokens: None,
            cache_write_input_tokens: None,
            first_output_latency_ms: None,
            tool_handoff: ResponseToolHandoff::None,
            terminal_success: false,
            complete: true,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SseStreamOutcome {
    Completed,
    TerminalGraceElapsed,
    UpstreamReadFailed,
    DownstreamCancelled,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SseStreamResult {
    pub outcome: SseStreamOutcome,
    pub metadata: ResponseMetadata,
}

#[cfg(test)]
pub fn observe_sse_stream<S, F>(
    stream: S,
    terminal_grace: Option<Duration>,
    on_finish: F,
) -> ObservedSseStream<S, F>
where
    S: Stream<Item = Result<Bytes, io::Error>> + Send + 'static,
    F: FnOnce(SseStreamResult) + Send + 'static,
{
    observe_sse_stream_started(stream, terminal_grace, Instant::now(), on_finish)
}

pub(super) fn observe_sse_stream_started<S, F>(
    stream: S,
    terminal_grace: Option<Duration>,
    started: Instant,
    on_finish: F,
) -> ObservedSseStream<S, F>
where
    S: Stream<Item = Result<Bytes, io::Error>> + Send + 'static,
    F: FnOnce(SseStreamResult) + Send + 'static,
{
    ObservedSseStream {
        stream: Box::pin(stream),
        observer: SseObserver::new_started(MAX_SSE_EVENT_BYTES, started),
        on_finish: Some(on_finish),
        terminal_grace,
        terminal_deadline: None,
        finished: false,
    }
}

pub struct ObservedSseStream<S, F>
where
    S: Stream<Item = Result<Bytes, io::Error>>,
    F: FnOnce(SseStreamResult),
{
    stream: Pin<Box<S>>,
    observer: SseObserver,
    on_finish: Option<F>,
    terminal_grace: Option<Duration>,
    terminal_deadline: Option<Pin<Box<tokio::time::Sleep>>>,
    finished: bool,
}

impl<S, F> Stream for ObservedSseStream<S, F>
where
    S: Stream<Item = Result<Bytes, io::Error>>,
    F: FnOnce(SseStreamResult),
{
    type Item = Result<Bytes, io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.as_mut().get_mut();
        if this.finished {
            return Poll::Ready(None);
        }
        if this
            .terminal_deadline
            .as_mut()
            .is_some_and(|deadline| deadline.as_mut().poll(context).is_ready())
        {
            this.finish(SseStreamOutcome::TerminalGraceElapsed);
            return Poll::Ready(None);
        }
        match this.stream.as_mut().poll_next(context) {
            Poll::Ready(Some(Ok(bytes))) => {
                let terminal_before = this.observer.terminal_seen();
                this.observer.feed(&bytes);
                if !terminal_before
                    && this.observer.terminal_seen()
                    && let Some(grace) = this.terminal_grace
                {
                    this.terminal_deadline = Some(Box::pin(tokio::time::sleep(grace)));
                }
                Poll::Ready(Some(Ok(bytes)))
            }
            Poll::Ready(Some(Err(error))) => {
                this.finish(SseStreamOutcome::UpstreamReadFailed);
                Poll::Ready(Some(Err(error)))
            }
            Poll::Ready(None) => {
                this.observer.finish_input();
                this.finish(SseStreamOutcome::Completed);
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<S, F> Unpin for ObservedSseStream<S, F>
where
    S: Stream<Item = Result<Bytes, io::Error>>,
    F: FnOnce(SseStreamResult),
{
}

impl<S, F> Drop for ObservedSseStream<S, F>
where
    S: Stream<Item = Result<Bytes, io::Error>>,
    F: FnOnce(SseStreamResult),
{
    fn drop(&mut self) {
        let outcome = if self.observer.terminal_outcome_seen() {
            SseStreamOutcome::Completed
        } else {
            SseStreamOutcome::DownstreamCancelled
        };
        self.finish(outcome);
    }
}

impl<S, F> ObservedSseStream<S, F>
where
    S: Stream<Item = Result<Bytes, io::Error>>,
    F: FnOnce(SseStreamResult),
{
    fn finish(&mut self, outcome: SseStreamOutcome) {
        if self.finished {
            return;
        }
        self.finished = true;
        if let Some(on_finish) = self.on_finish.take() {
            on_finish(SseStreamResult {
                outcome,
                metadata: self.observer.metadata.clone(),
            });
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SsePreflightSignal {
    Continue,
    Commit,
    TerminalFailure,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SsePreflightCommitReason {
    MeaningfulOutput,
    TerminalSuccess,
    UnknownEvent,
    MalformedEvent,
    EventLimit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SseObservationState {
    Enabled,
    Disabled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SseTerminalState {
    None,
    Success,
    Failure,
    FailureWithTerminal,
}

impl SseTerminalState {
    const fn success(self) -> Self {
        match self {
            Self::Failure | Self::FailureWithTerminal => Self::FailureWithTerminal,
            Self::None | Self::Success => Self::Success,
        }
    }

    const fn failure(self) -> Self {
        match self {
            Self::Success | Self::FailureWithTerminal => Self::FailureWithTerminal,
            Self::None | Self::Failure => Self::Failure,
        }
    }

    const fn terminal_seen(self) -> bool {
        matches!(self, Self::Success | Self::FailureWithTerminal)
    }

    const fn outcome_seen(self) -> bool {
        !matches!(self, Self::None)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SsePreflightState {
    Pending,
    Meaningful,
    ConservativeCommit,
}

pub(super) struct SseObserver {
    started: Instant,
    event_limit: usize,
    line: Vec<u8>,
    data: Vec<u8>,
    event_name: Option<String>,
    event_size: usize,
    observation_state: SseObservationState,
    terminal_state: SseTerminalState,
    preflight_state: SsePreflightState,
    preflight_failure: bool,
    preflight_commit_reason: Option<SsePreflightCommitReason>,
    metadata: ResponseMetadata,
}

impl SseObserver {
    pub(super) fn new(event_limit: usize) -> Self {
        Self::new_started(event_limit, Instant::now())
    }

    fn new_started(event_limit: usize, started: Instant) -> Self {
        Self {
            started,
            event_limit,
            line: Vec::new(),
            data: Vec::new(),
            event_name: None,
            event_size: 0,
            observation_state: SseObservationState::Enabled,
            terminal_state: SseTerminalState::None,
            preflight_state: SsePreflightState::Pending,
            preflight_failure: false,
            preflight_commit_reason: None,
            metadata: ResponseMetadata::default(),
        }
    }

    pub(super) const fn terminal_seen(&self) -> bool {
        self.terminal_state.terminal_seen()
    }

    const fn terminal_outcome_seen(&self) -> bool {
        self.terminal_state.outcome_seen()
    }

    pub(super) const fn preflight_signal(&self) -> SsePreflightSignal {
        if self.preflight_failure {
            SsePreflightSignal::TerminalFailure
        } else if self.terminal_state.terminal_seen()
            || matches!(
                self.preflight_state,
                SsePreflightState::Meaningful | SsePreflightState::ConservativeCommit
            )
        {
            SsePreflightSignal::Commit
        } else {
            SsePreflightSignal::Continue
        }
    }

    pub(super) fn metadata(&self) -> &ResponseMetadata {
        &self.metadata
    }

    pub(super) const fn preflight_commit_reason(&self) -> Option<SsePreflightCommitReason> {
        self.preflight_commit_reason
    }

    pub(super) fn feed(&mut self, bytes: &[u8]) {
        self.feed_internal(bytes, false);
    }

    pub(super) fn feed_preflight(&mut self, bytes: &[u8]) -> SsePreflightSignal {
        self.feed_internal(bytes, true);
        self.preflight_signal()
    }

    fn feed_internal(&mut self, bytes: &[u8], stop_on_decision: bool) {
        if self.observation_state == SseObservationState::Disabled {
            return;
        }
        for &byte in bytes {
            self.event_size = self.event_size.saturating_add(1);
            if self.event_size > self.event_limit {
                self.disable();
                self.preflight_commit_reason = Some(SsePreflightCommitReason::EventLimit);
                return;
            }
            if byte == b'\n' {
                let mut line = std::mem::take(&mut self.line);
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                self.process_line(&line);
                if self.observation_state == SseObservationState::Disabled {
                    return;
                }
                if stop_on_decision
                    && !matches!(self.preflight_signal(), SsePreflightSignal::Continue)
                {
                    return;
                }
            } else {
                self.line.push(byte);
            }
        }
    }

    pub(super) fn finish_input(&mut self) {
        if self.observation_state == SseObservationState::Disabled {
            return;
        }
        if !self.line.is_empty() {
            let mut line = std::mem::take(&mut self.line);
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            self.process_line(&line);
        }
        if !self.data.is_empty() || self.event_name.is_some() {
            self.dispatch_event();
        }
    }

    fn process_line(&mut self, line: &[u8]) {
        if line.is_empty() {
            self.dispatch_event();
            self.event_size = 0;
            return;
        }
        if line.first() == Some(&b':') {
            return;
        }
        let (field, value) =
            line.iter()
                .position(|byte| *byte == b':')
                .map_or((line, &[][..]), |separator| {
                    let (field, remainder) = line.split_at(separator);
                    let value = &remainder[1..];
                    (field, value.strip_prefix(b" ").unwrap_or(value))
                });
        match field {
            b"data" => {
                if !self.data.is_empty() {
                    self.data.push(b'\n');
                }
                self.data.extend_from_slice(value);
            }
            b"event" => match std::str::from_utf8(value) {
                Ok(value) => self.event_name = Some(value.to_owned()),
                Err(_) => self.disable(),
            },
            _ => {}
        }
    }

    fn dispatch_event(&mut self) {
        if self.data.is_empty() {
            self.event_name = None;
            return;
        }
        if self.data == b"[DONE]" {
            self.terminal_state = self.terminal_state.success();
            self.metadata.terminal_success = true;
            if self.preflight_state == SsePreflightState::Pending {
                self.preflight_commit_reason = Some(SsePreflightCommitReason::TerminalSuccess);
            }
            if !self
                .metadata
                .status
                .as_deref()
                .is_some_and(|status| matches!(status, "failed" | "cancelled"))
            {
                self.metadata.status = Some("completed".to_owned());
            }
            self.reset_event();
            return;
        }
        let Ok(projection) = serde_json::from_slice::<SseEventProjection>(&self.data) else {
            self.disable();
            return;
        };
        self.apply_projection(projection);
        self.reset_event();
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one ordered projection keeps preflight decisions and metadata extraction aligned"
    )]
    fn apply_projection(&mut self, projection: SseEventProjection) {
        let event_type = projection
            .event_type
            .as_deref()
            .or(self.event_name.as_deref());
        let terminal_failure_status = match event_type {
            Some("response.failed") => Some("failed"),
            Some("response.cancelled") => Some("cancelled"),
            Some("response.incomplete") => Some("incomplete"),
            _ => None,
        };
        let accepts_semantic_error = event_type == Some("response.failed");
        let flat_error = (event_type == Some("error")).then(|| SseErrorProjection {
            code: projection.code.clone(),
            codex_error_info: projection.codex_error_info.clone(),
            message: projection.message.clone(),
        });
        if event_type == Some("error") {
            if matches!(self.preflight_state, SsePreflightState::Pending) {
                self.preflight_failure = true;
            }
            self.metadata.status = Some("failed".to_owned());
            self.terminal_state = self.terminal_state.failure();
        }
        if terminal_failure_status.is_some() {
            self.metadata.safe_error_code = None;
        }
        match event_type {
            Some("response.completed") => {
                self.terminal_state = self.terminal_state.success();
                self.metadata.terminal_success = true;
                if self.preflight_state == SsePreflightState::Pending {
                    self.preflight_commit_reason = Some(SsePreflightCommitReason::TerminalSuccess);
                }
            }
            Some("response.failed" | "response.cancelled" | "response.incomplete") => {
                if matches!(self.preflight_state, SsePreflightState::Pending) {
                    self.preflight_failure = true;
                }
                self.terminal_state = self.terminal_state.failure();
            }
            Some("error") => {}
            Some(event)
                if !is_known_lifecycle_event(event) && !is_known_output_delta_event(event) =>
            {
                self.preflight_state = SsePreflightState::ConservativeCommit;
                self.preflight_commit_reason = Some(SsePreflightCommitReason::UnknownEvent);
            }
            None => {
                self.preflight_state = SsePreflightState::ConservativeCommit;
                self.preflight_commit_reason = Some(SsePreflightCommitReason::UnknownEvent);
            }
            _ => {}
        }
        if projection.meaningful_delta && event_type.is_some_and(is_response_output_delta_event) {
            if self.preflight_state == SsePreflightState::Pending {
                self.preflight_state = SsePreflightState::Meaningful;
                self.preflight_commit_reason = Some(SsePreflightCommitReason::MeaningfulOutput);
            }
            let latency_ms = self
                .started
                .elapsed()
                .as_millis()
                .try_into()
                .unwrap_or(u64::MAX);
            if self.metadata.first_output_latency_ms.is_none() {
                self.metadata.first_output_latency_ms = Some(latency_ms);
            }
        }
        observe_output_item(&mut self.metadata, projection.item.as_ref());
        if let Some(response) = projection.response {
            for item in &response.output {
                observe_output_item(&mut self.metadata, Some(item));
            }
            replace_bounded(
                &mut self.metadata.response_id,
                response.id,
                MAX_RESPONSE_ID_CHARS,
            );
            replace_bounded(&mut self.metadata.model, response.model, MAX_MODEL_CHARS);
            replace_bounded(
                &mut self.metadata.service_tier,
                response.service_tier,
                MAX_STATUS_CHARS,
            );
            replace_bounded(&mut self.metadata.status, response.status, MAX_STATUS_CHARS);
            if let Some(usage) = response.usage {
                self.metadata.input_tokens = usage.input;
                self.metadata.output_tokens = usage.output;
                self.metadata.total_tokens = usage.total;
                if let Some(details) = usage.input_details {
                    self.metadata.cached_input_tokens = details.cached;
                    self.metadata.cache_write_input_tokens = details.cache_write;
                }
            }
            if accepts_semantic_error && let Some(error) = response.error {
                replace_bounded(
                    &mut self.metadata.safe_error_code,
                    error.safe_error_code(),
                    MAX_ERROR_CODE_CHARS,
                );
            }
        }
        if accepts_semantic_error && let Some(error) = projection.error {
            replace_bounded(
                &mut self.metadata.safe_error_code,
                error.safe_error_code(),
                MAX_ERROR_CODE_CHARS,
            );
        }
        if let Some(error) = flat_error {
            replace_bounded(
                &mut self.metadata.safe_error_code,
                error.safe_error_code(),
                MAX_ERROR_CODE_CHARS,
            );
        }
        if let Some(status) = terminal_failure_status {
            self.metadata.status = Some(status.to_owned());
        }
    }

    fn reset_event(&mut self) {
        self.data.clear();
        self.event_name = None;
    }

    fn disable(&mut self) {
        self.observation_state = SseObservationState::Disabled;
        self.preflight_state = SsePreflightState::ConservativeCommit;
        self.preflight_commit_reason = Some(SsePreflightCommitReason::MalformedEvent);
        self.metadata.complete = false;
        self.line.clear();
        self.data.clear();
        self.event_name = None;
    }
}

fn is_known_lifecycle_event(event_type: &str) -> bool {
    matches!(
        event_type,
        "response.created"
            | "response.queued"
            | "response.in_progress"
            | "response.output_item.added"
            | "response.content_part.added"
            | "response.reasoning_summary_part.added"
            | "response.output_text.done"
            | "response.reasoning_text.done"
            | "response.reasoning_summary_text.done"
            | "response.function_call_arguments.done"
            | "response.content_part.done"
            | "response.output_item.done"
            | "response.reasoning_summary_part.done"
    )
}

fn is_known_output_delta_event(event_type: &str) -> bool {
    matches!(
        event_type,
        "response.output_text.delta"
            | "response.reasoning_text.delta"
            | "response.reasoning_summary_text.delta"
            | "response.function_call_arguments.delta"
    )
}

fn is_response_output_delta_event(event_type: &str) -> bool {
    event_type
        .strip_prefix("response.")
        .and_then(|event| event.strip_suffix(".delta"))
        .is_some_and(|event| !event.is_empty())
}

fn replace_bounded(target: &mut Option<String>, source: Option<String>, maximum: usize) {
    if let Some(mut source) = source {
        if let Some((index, _)) = source.char_indices().nth(maximum) {
            source.truncate(index);
        }
        *target = Some(source);
    }
}

#[derive(Deserialize)]
struct SseEventProjection {
    #[serde(rename = "type")]
    event_type: Option<String>,
    response: Option<SseResponseProjection>,
    item: Option<SseOutputItemProjection>,
    error: Option<SseErrorProjection>,
    code: Option<String>,
    codex_error_info: Option<String>,
    message: Option<String>,
    #[serde(default, rename = "delta", deserialize_with = "deserialize_non_empty")]
    meaningful_delta: bool,
}

#[derive(Deserialize)]
struct SseResponseProjection {
    id: Option<String>,
    model: Option<String>,
    service_tier: Option<String>,
    status: Option<String>,
    usage: Option<SseUsageProjection>,
    error: Option<SseErrorProjection>,
    #[serde(default)]
    output: Vec<SseOutputItemProjection>,
}

#[derive(Deserialize)]
struct SseOutputItemProjection {
    #[serde(rename = "type")]
    item_type: Option<String>,
}

fn observe_output_item(metadata: &mut ResponseMetadata, item: Option<&SseOutputItemProjection>) {
    match item.and_then(|item| item.item_type.as_deref()) {
        Some("function_call" | "custom_tool_call" | "local_shell_call" | "computer_call") => {
            metadata.tool_handoff = metadata.tool_handoff.with_client();
        }
        Some("shell_call") => metadata.tool_handoff = metadata.tool_handoff.with_shell(),
        _ => {}
    }
}

#[derive(Deserialize)]
struct SseUsageProjection {
    #[serde(rename = "input_tokens")]
    input: Option<u64>,
    #[serde(rename = "output_tokens")]
    output: Option<u64>,
    #[serde(rename = "total_tokens")]
    total: Option<u64>,
    #[serde(rename = "input_tokens_details")]
    input_details: Option<InputTokenDetailsProjection>,
}

#[derive(Deserialize)]
struct InputTokenDetailsProjection {
    #[serde(rename = "cached_tokens")]
    cached: Option<u64>,
    #[serde(rename = "cache_write_tokens")]
    cache_write: Option<u64>,
}

#[derive(Deserialize)]
struct SseErrorProjection {
    code: Option<String>,
    codex_error_info: Option<String>,
    message: Option<String>,
}

impl SseErrorProjection {
    fn safe_error_code(self) -> Option<String> {
        normalize_semantic_error_code(
            self.code.as_deref(),
            self.codex_error_info.as_deref(),
            self.message.as_deref(),
        )
    }
}

fn deserialize_non_empty<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: Deserializer<'de>,
{
    struct NonEmptyVisitor;

    impl Visitor<'_> for NonEmptyVisitor {
        type Value = bool;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("a string delta")
        }

        fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
            Ok(!value.is_empty())
        }

        fn visit_borrowed_str<E: serde::de::Error>(self, value: &'_ str) -> Result<Self::Value, E> {
            Ok(!value.is_empty())
        }
    }

    deserializer.deserialize_str(NonEmptyVisitor)
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{Arc, Mutex},
        time::Duration,
    };

    use axum::body::{Body, to_bytes};
    use futures_util::{StreamExt, stream};

    use super::*;

    #[tokio::test]
    async fn sse_passthrough_preserves_arbitrary_chunks_and_projects_metadata() {
        let source = concat!(
            ": comment\r\n\r\n",
            "event: response.created\n",
            "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_1\",\"model\":\"gpt-test\",\"status\":\"in_progress\"}}\n\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"你好\"}\r\n\r\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"service_tier\":\"default\",\"usage\":{\"input_tokens\":3,\"output_tokens\":5,\"total_tokens\":8,\"input_tokens_details\":{\"cached_tokens\":2,\"cache_write_tokens\":1}}}}\n\n",
            "data: [DONE]\n\n"
        )
        .as_bytes()
        .to_vec();
        let chunks = source
            .iter()
            .map(|byte| Ok::<_, io::Error>(Bytes::copy_from_slice(&[*byte])))
            .collect::<Vec<_>>();
        let result = Arc::new(Mutex::new(None));
        let result_capture = Arc::clone(&result);
        let observed = observe_sse_stream(
            stream::iter(chunks),
            Some(Duration::from_secs(3)),
            move |value| {
                *result_capture.lock().expect("result mutex") = Some(value);
            },
        );

        let downstream = to_bytes(Body::from_stream(observed), source.len() + 1)
            .await
            .expect("SSE body");

        assert_eq!(downstream.as_ref(), source.as_slice());
        let result = result
            .lock()
            .expect("result mutex")
            .clone()
            .expect("stream result");
        assert_eq!(result.outcome, SseStreamOutcome::Completed);
        assert!(result.metadata.complete);
        assert_eq!(result.metadata.response_id.as_deref(), Some("resp_1"));
        assert_eq!(result.metadata.model.as_deref(), Some("gpt-test"));
        assert_eq!(result.metadata.status.as_deref(), Some("completed"));
        assert_eq!(result.metadata.input_tokens, Some(3));
        assert_eq!(result.metadata.output_tokens, Some(5));
        assert_eq!(result.metadata.total_tokens, Some(8));
        assert_eq!(result.metadata.service_tier.as_deref(), Some("default"));
        assert_eq!(result.metadata.cached_input_tokens, Some(2));
        assert_eq!(result.metadata.cache_write_input_tokens, Some(1));
        assert!(result.metadata.first_output_latency_ms.is_some());
    }

    #[test]
    fn first_output_accepts_text_reasoning_summary_and_tool_deltas_once() {
        let started = Instant::now()
            .checked_sub(Duration::from_millis(1_720))
            .expect("test latency is representable");
        let mut observer = SseObserver::new_started(MAX_SSE_EVENT_BYTES, started);
        observer
            .feed(b"data: {\"type\":\"response.reasoning_text.delta\",\"delta\":\"thinking\"}\n\n");
        let first_output_latency_ms = observer
            .metadata()
            .first_output_latency_ms
            .expect("reasoning delta is meaningful output");
        assert!(first_output_latency_ms >= 1_720);
        assert_eq!(observer.preflight_signal(), SsePreflightSignal::Commit);

        observer.feed(b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"\"}\n\n");
        std::thread::sleep(Duration::from_millis(10));
        observer.feed(b"data: {\"type\":\"response.output_text.");
        observer.feed(b"delta\",\"delta\":\"visible\"}\n\n");
        assert_eq!(
            observer.metadata().first_output_latency_ms,
            Some(first_output_latency_ms)
        );

        let mut tool_only = SseObserver::new(MAX_SSE_EVENT_BYTES);
        tool_only.feed(
            b"data: {\"type\":\"response.function_call_arguments.delta\",\"delta\":\"{}\"}\n\n",
        );
        assert!(tool_only.metadata().first_output_latency_ms.is_some());

        let mut custom_tool = SseObserver::new(MAX_SSE_EVENT_BYTES);
        custom_tool.feed(b"data: {\"type\":\"response.custom_tool_call_input.");
        assert_eq!(custom_tool.metadata().first_output_latency_ms, None);
        custom_tool.feed(b"delta\",\"delta\":\"command\"}\n\n");
        assert!(custom_tool.metadata().first_output_latency_ms.is_some());

        let mut reasoning_summary = SseObserver::new(MAX_SSE_EVENT_BYTES);
        reasoning_summary.feed(
            b"data: {\"type\":\"response.reasoning_summary_text.delta\",\"delta\":\"summary\"}\n\n",
        );
        assert!(
            reasoning_summary
                .metadata()
                .first_output_latency_ms
                .is_some()
        );

        let mut empty_custom_tool = SseObserver::new(MAX_SSE_EVENT_BYTES);
        empty_custom_tool
            .feed(b"data: {\"type\":\"response.custom_tool_call_input.delta\",\"delta\":\"\"}\n\n");
        assert_eq!(empty_custom_tool.metadata().first_output_latency_ms, None);
        assert_eq!(
            empty_custom_tool.preflight_signal(),
            SsePreflightSignal::Commit
        );

        let mut whitespace = SseObserver::new(MAX_SSE_EVENT_BYTES);
        whitespace.feed(b"data: {\"type\":\"response.output_text.delta\",\"delta\":\" \"}\n\n");
        assert!(whitespace.metadata().first_output_latency_ms.is_some());
    }

    #[tokio::test(start_paused = true)]
    async fn sse_terminal_event_bounds_a_lingering_upstream() {
        let source = concat!(
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"ok\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\n\n",
            ": coalesced trailing comment\n\n"
        )
        .as_bytes();
        let started = tokio::time::Instant::now();
        let stream =
            stream::iter([Ok::<_, io::Error>(Bytes::from_static(source))]).chain(stream::pending());
        let result = Arc::new(Mutex::new(None));
        let result_capture = Arc::clone(&result);
        let observed = observe_sse_stream(stream, Some(Duration::from_secs(3)), move |value| {
            *result_capture.lock().expect("result mutex") = Some(value);
        });

        let downstream = tokio::time::timeout(
            Duration::from_secs(4),
            to_bytes(Body::from_stream(observed), source.len() + 1),
        )
        .await
        .expect("terminal stream should finish within the grace bound")
        .expect("SSE body");

        assert_eq!(downstream.as_ref(), source);
        assert_eq!(started.elapsed(), Duration::from_secs(3));
        assert_eq!(
            result
                .lock()
                .expect("result mutex")
                .as_ref()
                .expect("stream result")
                .outcome,
            SseStreamOutcome::TerminalGraceElapsed
        );
    }

    #[tokio::test(start_paused = true)]
    async fn sse_done_sentinel_bounds_a_lingering_upstream() {
        let source = b"data: [DONE]\n\n";
        let stream =
            stream::iter([Ok::<_, io::Error>(Bytes::from_static(source))]).chain(stream::pending());
        let result = Arc::new(Mutex::new(None));
        let result_capture = Arc::clone(&result);
        let observed = observe_sse_stream(stream, Some(Duration::from_secs(3)), move |value| {
            *result_capture.lock().expect("result mutex") = Some(value);
        });

        let downstream = to_bytes(Body::from_stream(observed), source.len() + 1)
            .await
            .expect("SSE body");

        assert_eq!(downstream.as_ref(), source);
        assert_eq!(
            result
                .lock()
                .expect("result mutex")
                .as_ref()
                .expect("stream result")
                .outcome,
            SseStreamOutcome::TerminalGraceElapsed
        );
    }

    #[tokio::test(start_paused = true)]
    async fn sse_fragmented_terminal_arms_grace_only_after_the_event_boundary() {
        let source =
            b"data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\r\n\r\n";
        let chunks = source
            .iter()
            .map(|byte| Ok::<_, io::Error>(Bytes::copy_from_slice(&[*byte])));
        let stream = stream::iter(chunks).chain(stream::pending());
        let started = tokio::time::Instant::now();
        let observed = observe_sse_stream(stream, Some(Duration::from_secs(3)), |_| {});

        let downstream = to_bytes(Body::from_stream(observed), source.len() + 1)
            .await
            .expect("SSE body");

        assert_eq!(downstream.as_ref(), source);
        assert_eq!(started.elapsed(), Duration::from_secs(3));
    }

    #[tokio::test(start_paused = true)]
    async fn sse_event_name_alone_does_not_arm_terminal_grace() {
        let source = concat!(
            "event: response.completed\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"still running\"}\n\n"
        );
        let stream = stream::iter([Ok::<_, io::Error>(Bytes::copy_from_slice(
            source.as_bytes(),
        ))])
        .chain(stream::pending());
        let result = Arc::new(Mutex::new(None));
        let result_capture = Arc::clone(&result);
        let observed = observe_sse_stream(stream, Some(Duration::from_secs(3)), move |value| {
            *result_capture.lock().expect("result mutex") = Some(value);
        });

        let downstream = tokio::time::timeout(
            Duration::from_secs(4),
            to_bytes(Body::from_stream(observed), source.len() + 1),
        )
        .await;

        assert!(downstream.is_err());
        assert_eq!(
            result
                .lock()
                .expect("result mutex")
                .as_ref()
                .expect("stream result")
                .outcome,
            SseStreamOutcome::DownstreamCancelled
        );
    }

    #[tokio::test(start_paused = true)]
    async fn trusted_terminal_without_grace_remains_eof_driven_but_completes_on_drop() {
        let source = b"data: {\"type\":\"response.completed\"}\n\n";
        let stream =
            stream::iter([Ok::<_, io::Error>(Bytes::from_static(source))]).chain(stream::pending());
        let result = Arc::new(Mutex::new(None));
        let result_capture = Arc::clone(&result);
        let observed = observe_sse_stream(stream, None, move |value| {
            *result_capture.lock().expect("result mutex") = Some(value);
        });

        let downstream = tokio::time::timeout(
            Duration::from_secs(4),
            to_bytes(Body::from_stream(observed), source.len() + 1),
        )
        .await;

        assert!(downstream.is_err());
        assert_eq!(
            result
                .lock()
                .expect("result mutex")
                .as_ref()
                .expect("stream result")
                .outcome,
            SseStreamOutcome::Completed
        );
    }

    #[tokio::test]
    async fn sse_passthrough_disables_only_observation_on_overflow_or_malformed_json() {
        for source in [
            format!("data: {{\"delta\":\"{}\"}}\n\n", "x".repeat(128)),
            "data: {not-json}\n\n".to_owned(),
        ] {
            let result = Arc::new(Mutex::new(None));
            let result_capture = Arc::clone(&result);
            let stream = stream::iter([Ok::<_, io::Error>(Bytes::copy_from_slice(
                source.as_bytes(),
            ))]);
            let observed = ObservedSseStream {
                stream: Box::pin(stream),
                observer: SseObserver::new(64),
                on_finish: Some(move |value| {
                    *result_capture.lock().expect("result mutex") = Some(value);
                }),
                terminal_grace: Some(Duration::from_secs(3)),
                terminal_deadline: None,
                finished: false,
            };

            let downstream = to_bytes(Body::from_stream(observed), source.len() + 1)
                .await
                .expect("SSE body");

            assert_eq!(downstream.as_ref(), source.as_bytes());
            assert!(
                !result
                    .lock()
                    .expect("result mutex")
                    .as_ref()
                    .expect("stream result")
                    .metadata
                    .complete
            );
        }
    }

    #[tokio::test]
    async fn sse_passthrough_reports_downstream_cancellation_without_consuming_upstream() {
        let callback = Arc::new(Mutex::new(None));
        let callback_capture = Arc::clone(&callback);
        let stream = stream::pending::<Result<Bytes, io::Error>>();
        let observed = observe_sse_stream(stream, Some(Duration::from_secs(3)), move |result| {
            *callback_capture.lock().expect("callback mutex") = Some(result);
        });

        drop(observed);

        assert_eq!(
            callback
                .lock()
                .expect("callback mutex")
                .as_ref()
                .expect("stream result")
                .outcome,
            SseStreamOutcome::DownstreamCancelled
        );
    }

    #[tokio::test]
    async fn sse_passthrough_drop_before_terminal_frame_boundary_remains_cancelled() {
        let source = Bytes::from_static(
            b"data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\n",
        );
        let callback = Arc::new(Mutex::new(None));
        let callback_capture = Arc::clone(&callback);
        let stream = stream::iter([Ok::<_, io::Error>(source.clone())]).chain(stream::pending());
        let mut observed =
            observe_sse_stream(stream, Some(Duration::from_secs(3)), move |result| {
                *callback_capture.lock().expect("callback mutex") = Some(result);
            });

        assert_eq!(
            observed
                .next()
                .await
                .expect("partial terminal chunk")
                .expect("SSE chunk"),
            source
        );
        drop(observed);

        assert_eq!(
            callback
                .lock()
                .expect("callback mutex")
                .as_ref()
                .expect("stream result")
                .outcome,
            SseStreamOutcome::DownstreamCancelled
        );
    }

    #[tokio::test]
    async fn sse_passthrough_drop_after_success_terminal_reports_completion() {
        let source = Bytes::from_static(
            b"data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\n\n",
        );
        let callback = Arc::new(Mutex::new(None));
        let callback_capture = Arc::clone(&callback);
        let stream = stream::iter([Ok::<_, io::Error>(source.clone())]).chain(stream::pending());
        let mut observed =
            observe_sse_stream(stream, Some(Duration::from_secs(3)), move |result| {
                *callback_capture.lock().expect("callback mutex") = Some(result);
            });

        assert_eq!(
            observed
                .next()
                .await
                .expect("terminal chunk")
                .expect("SSE chunk"),
            source
        );
        drop(observed);

        let result = callback
            .lock()
            .expect("callback mutex")
            .clone()
            .expect("stream result");
        assert_eq!(result.outcome, SseStreamOutcome::Completed);
        assert_eq!(result.metadata.status.as_deref(), Some("completed"));
    }

    #[tokio::test]
    async fn sse_passthrough_drop_after_failure_terminal_preserves_failure() {
        let source = Bytes::from_static(
            b"data: {\"type\":\"response.failed\",\"response\":{\"status\":\"failed\",\"error\":{\"code\":\"insufficient_quota\"}}}\n\n",
        );
        let callback = Arc::new(Mutex::new(None));
        let callback_capture = Arc::clone(&callback);
        let stream = stream::iter([Ok::<_, io::Error>(source.clone())]).chain(stream::pending());
        let mut observed =
            observe_sse_stream(stream, Some(Duration::from_secs(3)), move |result| {
                *callback_capture.lock().expect("callback mutex") = Some(result);
            });

        assert_eq!(
            observed
                .next()
                .await
                .expect("terminal chunk")
                .expect("SSE chunk"),
            source
        );
        drop(observed);

        let result = callback
            .lock()
            .expect("callback mutex")
            .clone()
            .expect("stream result");
        assert_eq!(result.outcome, SseStreamOutcome::Completed);
        assert_eq!(result.metadata.status.as_deref(), Some("failed"));
        assert_eq!(
            result.metadata.safe_error_code.as_deref(),
            Some("insufficient_quota")
        );
    }

    #[tokio::test]
    async fn sse_passthrough_done_does_not_overwrite_a_failed_terminal_state() {
        let source = concat!(
            "data: {\"type\":\"response.failed\",\"response\":{\"status\":\"failed\"}}\n\n",
            "data: [DONE]\n\n"
        );
        let result = Arc::new(Mutex::new(None));
        let result_capture = Arc::clone(&result);
        let observed = observe_sse_stream(
            stream::iter([Ok::<_, io::Error>(Bytes::copy_from_slice(
                source.as_bytes(),
            ))]),
            Some(Duration::from_secs(3)),
            move |value| {
                *result_capture.lock().expect("result mutex") = Some(value);
            },
        );

        let downstream = to_bytes(Body::from_stream(observed), source.len() + 1)
            .await
            .expect("SSE body");

        assert_eq!(downstream.as_ref(), source.as_bytes());
        assert_eq!(
            result
                .lock()
                .expect("result mutex")
                .as_ref()
                .expect("stream result")
                .metadata
                .status
                .as_deref(),
            Some("failed")
        );
    }

    #[test]
    fn preflight_signals_commit_conservatively_and_keep_lifecycle_activity_pending() {
        let mut lifecycle = SseObserver::new(MAX_SSE_EVENT_BYTES);
        lifecycle.feed(b"data: {\"type\":\"response.created\"}\n\n");
        assert_eq!(lifecycle.preflight_signal(), SsePreflightSignal::Continue);
        assert_eq!(lifecycle.metadata().first_output_latency_ms, None);

        let mut meaningful = SseObserver::new(MAX_SSE_EVENT_BYTES);
        meaningful.feed(b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"ok\"}\n\n");
        assert_eq!(meaningful.preflight_signal(), SsePreflightSignal::Commit);
        assert!(meaningful.metadata().first_output_latency_ms.is_some());

        let mut unknown = SseObserver::new(MAX_SSE_EVENT_BYTES);
        unknown.feed(b"data: {\"type\":\"provider.unknown\"}\n\n");
        assert_eq!(unknown.preflight_signal(), SsePreflightSignal::Commit);
        assert_eq!(unknown.metadata().first_output_latency_ms, None);

        let mut compatible_delta = SseObserver::new(MAX_SSE_EVENT_BYTES);
        compatible_delta.feed(
            b"data: {\"type\":\"response.future_tool_input.delta\",\"delta\":\"payload\"}\n\n",
        );
        assert_eq!(
            compatible_delta.preflight_signal(),
            SsePreflightSignal::Commit
        );
        assert!(
            compatible_delta
                .metadata()
                .first_output_latency_ms
                .is_some()
        );

        let mut empty_compatible_delta = SseObserver::new(MAX_SSE_EVENT_BYTES);
        empty_compatible_delta
            .feed(b"data: {\"type\":\"response.future_tool_input.delta\",\"delta\":\"\"}\n\n");
        assert_eq!(
            empty_compatible_delta.preflight_signal(),
            SsePreflightSignal::Commit
        );
        assert_eq!(
            empty_compatible_delta.metadata().first_output_latency_ms,
            None
        );

        let mut malformed = SseObserver::new(MAX_SSE_EVENT_BYTES);
        malformed.feed(b"data: {not-json}\n\n");
        assert_eq!(malformed.preflight_signal(), SsePreflightSignal::Commit);
        assert_eq!(malformed.metadata().first_output_latency_ms, None);

        let mut failed = SseObserver::new(MAX_SSE_EVENT_BYTES);
        failed.feed(
            b"data: {\"type\":\"response.failed\",\"response\":{\"status\":\"failed\",\"error\":{\"code\":\"insufficient_quota\"}}}\n\n",
        );
        assert_eq!(
            failed.preflight_signal(),
            SsePreflightSignal::TerminalFailure
        );
        assert_eq!(
            failed.metadata().safe_error_code.as_deref(),
            Some("insufficient_quota")
        );
        assert_eq!(failed.metadata().first_output_latency_ms, None);
    }

    #[test]
    fn preflight_projects_only_exact_structured_overload_signals() {
        for source in [
            r#"data: {"type":"response.failed","response":{"status":"failed","error":{"code":"server_overloaded"}}}

"#,
            r#"data: {"type":"response.failed","response":{"status":"failed","error":{"code":"server_error","codex_error_info":"server_overloaded"}}}

"#,
            r#"data: {"type":"response.failed","error":{"codex_error_info":"server_overloaded"}}

"#,
            r#"data: {"type":"response.failed","response":{"status":"failed","error":{"code":"server_error","message":"Selected model is at capacity. Please try a different model."}}}

"#,
        ] {
            let mut observer = SseObserver::new(MAX_SSE_EVENT_BYTES);
            observer.feed(source.as_bytes());

            assert_eq!(
                observer.preflight_signal(),
                SsePreflightSignal::TerminalFailure
            );
            assert_eq!(
                observer.metadata().safe_error_code.as_deref(),
                Some("server_overloaded")
            );
        }

        for (source, expected_code) in [
            (
                r#"data: {"type":"response.failed","response":{"status":"failed","error":{"code":"server_error","message":"Selected model is at capacity."}}}

"#,
                Some("server_error"),
            ),
            (
                r#"data: {"type":"response.failed","response":{"status":"failed","error":{"code":"server_error","codex_error_info":"capacity"}}}

"#,
                Some("server_error"),
            ),
            (
                r#"data: {"type":"response.failed","response":{"status":"failed","error":{"codex_error_info":"server_overloaded_extra"}}}

"#,
                None,
            ),
        ] {
            let mut observer = SseObserver::new(MAX_SSE_EVENT_BYTES);
            observer.feed(source.as_bytes());

            assert_eq!(
                observer.metadata().safe_error_code.as_deref(),
                expected_code
            );
        }

        for (event_type, status) in [
            ("response.cancelled", "cancelled"),
            ("response.incomplete", "incomplete"),
        ] {
            let source = format!(
                "data: {{\"type\":\"{event_type}\",\"response\":{{\"status\":\"{status}\",\"error\":{{\"codex_error_info\":\"server_overloaded\"}}}}}}\n\n"
            );
            let mut observer = SseObserver::new(MAX_SSE_EVENT_BYTES);
            observer.feed(source.as_bytes());

            assert_eq!(observer.metadata().status.as_deref(), Some(status));
            assert_eq!(observer.metadata().safe_error_code, None);
        }

        let mut observer = SseObserver::new(MAX_SSE_EVENT_BYTES);
        observer.feed(
            concat!(
                "data: {\"type\":\"response.created\",\"response\":{\"error\":{\"code\":\"server_error\",\"message\":\"Selected model is at capacity. Please try a different model.\"}}}\n\n",
                "data: {\"type\":\"response.failed\",\"response\":{\"status\":\"failed\"}}\n\n"
            )
            .as_bytes(),
        );

        assert_eq!(
            observer.preflight_signal(),
            SsePreflightSignal::TerminalFailure
        );
        assert_eq!(observer.metadata().safe_error_code, None);
    }

    #[test]
    fn preflight_projects_only_exact_flat_error_overload_signals() {
        for source in [
            r#"data: {"type":"error","code":"server_overloaded"}

"#,
            r#"data: {"type":"error","code":"server_error","codex_error_info":"server_overloaded"}

"#,
            r#"data: {"type":"error","code":"server_error","message":"Selected model is at capacity. Please try a different model."}

"#,
        ] {
            let mut observer = SseObserver::new(MAX_SSE_EVENT_BYTES);
            assert_eq!(
                observer.feed_preflight(source.as_bytes()),
                SsePreflightSignal::TerminalFailure
            );
            assert_eq!(observer.metadata().status.as_deref(), Some("failed"));
            assert_eq!(
                observer.metadata().safe_error_code.as_deref(),
                Some("server_overloaded")
            );
        }

        for (source, expected_code) in [
            (
                r#"data: {"type":"error","code":"server_error","message":"Selected model is at capacity."}

"#,
                Some("server_error"),
            ),
            (
                r#"data: {"type":"error","codex_error_info":"server_overloaded_extra"}

"#,
                None,
            ),
            (
                r#"data: {"type":"response.failed","code":"server_overloaded"}

"#,
                None,
            ),
        ] {
            let mut observer = SseObserver::new(MAX_SSE_EVENT_BYTES);
            assert_eq!(
                observer.feed_preflight(source.as_bytes()),
                SsePreflightSignal::TerminalFailure
            );
            assert_eq!(
                observer.metadata().safe_error_code.as_deref(),
                expected_code
            );
        }
    }

    #[test]
    fn preflight_stops_at_the_first_decisive_event_in_a_transport_chunk() {
        let error = "data: {\"type\":\"error\",\"code\":\"server_overloaded\"}\n\n";
        let output = "data: {\"type\":\"response.output_text.delta\",\"delta\":\"visible\"}\n\n";

        let mut error_first = SseObserver::new(MAX_SSE_EVENT_BYTES);
        let error_then_output = format!("{error}{output}");
        assert_eq!(
            error_first.feed_preflight(error_then_output.as_bytes()),
            SsePreflightSignal::TerminalFailure
        );
        assert_eq!(
            error_first.metadata().safe_error_code.as_deref(),
            Some("server_overloaded")
        );
        assert_eq!(error_first.metadata().first_output_latency_ms, None);

        let mut output_first = SseObserver::new(MAX_SSE_EVENT_BYTES);
        let output_then_error = format!("{output}{error}");
        assert_eq!(
            output_first.feed_preflight(output_then_error.as_bytes()),
            SsePreflightSignal::Commit
        );
        assert!(output_first.metadata().first_output_latency_ms.is_some());
        assert_eq!(output_first.metadata().safe_error_code, None);
        assert_eq!(
            output_first.preflight_commit_reason(),
            Some(SsePreflightCommitReason::MeaningfulOutput)
        );
    }

    #[test]
    fn preflight_preserves_hard_commit_before_a_later_failure() {
        for commit_event in [
            r#"data: {"type":"response.output_text.delta","delta":"visible"}

"#,
            r#"data: {"type":"provider.unknown"}

"#,
        ] {
            let mut observer = SseObserver::new(MAX_SSE_EVENT_BYTES);
            observer.feed(commit_event.as_bytes());
            observer.feed(
                b"data: {\"type\":\"response.failed\",\"response\":{\"status\":\"failed\",\"error\":{\"codex_error_info\":\"server_overloaded\"}}}\n\n",
            );

            assert_eq!(observer.preflight_signal(), SsePreflightSignal::Commit);
            assert_eq!(
                observer.metadata().safe_error_code.as_deref(),
                Some("server_overloaded")
            );
        }
    }

    #[test]
    fn structured_output_items_project_only_supported_client_handoffs() {
        let mut observer = SseObserver::new(MAX_SSE_EVENT_BYTES);
        observer.feed(
            br#"data: {"type":"response.output_item.added","item":{"type":"function_call"}}

data: {"type":"response.output_item.done","item":{"type":"shell_call"}}

data: {"type":"response.output_item.done","item":{"type":"web_search_call"}}

data: {"type":"response.completed","response":{"status":"completed"}}

"#,
        );

        assert!(observer.metadata().tool_handoff.has_client());
        assert!(observer.metadata().tool_handoff.has_shell());
        assert!(observer.metadata().terminal_success);

        let mut terminal_output = SseObserver::new(MAX_SSE_EVENT_BYTES);
        terminal_output.feed(
            br#"data: {"type":"response.completed","response":{"status":"completed","output":[{"type":"custom_tool_call"},{"type":"computer_call"}]}}

"#,
        );
        assert!(terminal_output.metadata().tool_handoff.has_client());
        assert!(!terminal_output.metadata().tool_handoff.has_shell());
        assert!(terminal_output.metadata().terminal_success);
    }
}
