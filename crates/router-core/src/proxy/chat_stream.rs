//! Chat Completions → Responses response and stream translation.
//!
//! One item builder serves the non-streaming JSON path and every streaming
//! delta, so text, readable reasoning, tool restoration, usage, and status
//! mapping cannot drift between them. Framing is incremental and bounded: an
//! upstream that never sends a delimiter fails the turn instead of growing an
//! unbounded carry buffer.

use std::collections::{BTreeMap, HashMap};

use serde_json::{Map, Value, json};

use super::chat_bridge::{ToolOrigin, ToolOriginKind};
use super::history::bounded_string;

/// Maximum bytes buffered while waiting for one upstream frame delimiter.
pub(super) const MAX_CHAT_FRAME_BYTES: usize = 256 * 1024;

/// Maximum characters kept from an upstream error message.
const MAX_ERROR_MESSAGE_CHARS: usize = 512;

/// Maximum characters kept from an upstream error code.
const MAX_ERROR_CODE_CHARS: usize = 128;

/// Terminal Chat payload that ends a stream.
const DONE_PAYLOAD: &str = "[DONE]";

/// Bounded failure code for a stream that ended without a terminal event.
pub(super) const STREAM_TRUNCATED_CODE: &str = "stream_truncated";
/// Failure code for an upstream error frame without a usable code.
pub(super) const UPSTREAM_ERROR_CODE: &str = "upstream_error";
/// Failure code for a frame larger than [`MAX_CHAT_FRAME_BYTES`].
pub(super) const FRAME_TOO_LARGE_CODE: &str = "upstream_frame_too_large";
/// Failure code for a turn whose tool calls never carried a name.
pub(super) const TOOL_CALL_DROPPED_CODE: &str = "upstream_tool_call_dropped";

/// A bounded stream-level failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct StreamFailure {
    /// Bounded machine code.
    pub code: &'static str,
    /// Bounded human-readable message.
    pub message: &'static str,
}

impl StreamFailure {
    const fn new(code: &'static str, message: &'static str) -> Self {
        Self { code, message }
    }
}

/// Incremental `text/event-stream` decoder with a bounded carry buffer.
#[derive(Default)]
pub(super) struct ChatSseDecoder {
    carry: Vec<u8>,
}

impl ChatSseDecoder {
    /// Appends `chunk` and returns every frame payload it completes.
    ///
    /// # Errors
    ///
    /// Returns [`FRAME_TOO_LARGE_CODE`] when one incomplete frame exceeds
    /// [`MAX_CHAT_FRAME_BYTES`]; the caller must fail the turn and stop reading.
    pub(super) fn push(&mut self, chunk: &[u8]) -> Result<Vec<String>, StreamFailure> {
        self.carry.extend_from_slice(chunk);
        let mut payloads = Vec::new();
        while let Some((index, length)) = frame_delimiter(&self.carry) {
            // The bound covers one complete frame, not only the incomplete
            // tail: a single transport chunk may already contain a delimiter
            // far past it.
            if index > MAX_CHAT_FRAME_BYTES {
                return Err(StreamFailure::new(
                    FRAME_TOO_LARGE_CODE,
                    "Upstream sent a frame larger than the bounded carry buffer.",
                ));
            }
            let frame = self.carry[..index].to_vec();
            self.carry.drain(..index + length);
            if let Some(payload) = decode_frame(&frame) {
                payloads.push(payload);
            }
        }
        if self.carry.len() > MAX_CHAT_FRAME_BYTES {
            return Err(StreamFailure::new(
                FRAME_TOO_LARGE_CODE,
                "Upstream sent a frame larger than the bounded carry buffer.",
            ));
        }
        Ok(payloads)
    }

    /// Whether an incomplete frame is still buffered.
    #[cfg(test)]
    pub(super) fn has_pending(&self) -> bool {
        !self.carry.is_empty()
    }
}

fn frame_delimiter(buffer: &[u8]) -> Option<(usize, usize)> {
    let lf = buffer.windows(2).position(|window| window == b"\n\n");
    let crlf = buffer.windows(4).position(|window| window == b"\r\n\r\n");
    match (lf, crlf) {
        (Some(lf), Some(crlf)) => {
            if crlf < lf {
                Some((crlf, 4))
            } else {
                Some((lf, 2))
            }
        }
        (Some(lf), None) => Some((lf, 2)),
        (None, Some(crlf)) => Some((crlf, 4)),
        (None, None) => None,
    }
}

fn decode_frame(frame: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(frame).ok()?;
    let mut data = Vec::new();
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let Some(rest) = line.strip_prefix("data:") else {
            continue;
        };
        data.push(rest.strip_prefix(' ').unwrap_or(rest));
    }
    (!data.is_empty()).then(|| data.join("\n"))
}

#[derive(Default)]
struct TextItem {
    index: u64,
    text: String,
    added: bool,
}

#[derive(Default)]
struct ReasoningItem {
    index: u64,
    text: String,
    added: bool,
}

#[derive(Default)]
struct ToolCallItem {
    index: Option<u64>,
    call_id: String,
    chat_name: String,
    arguments: String,
    added: bool,
}

/// Translates one upstream Chat Completions turn into Responses SSE.
pub(super) struct ChatStreamBridge {
    response_id: String,
    created_at: u64,
    model: String,
    tool_names: HashMap<String, ToolOrigin>,
    started: bool,
    text: TextItem,
    reasoning: ReasoningItem,
    tools: BTreeMap<u64, ToolCallItem>,
    next_tool_to_add: u64,
    next_output_index: u64,
    output_items: Vec<(u64, Value)>,
    usage: Option<Value>,
    finish_reason: Option<String>,
    terminal: bool,
}

impl ChatStreamBridge {
    /// Creates a bridge for one turn.
    pub(super) fn new(
        response_id: String,
        created_at: u64,
        model: String,
        tool_names: HashMap<String, ToolOrigin>,
    ) -> Self {
        Self {
            response_id,
            created_at,
            model,
            tool_names,
            started: false,
            text: TextItem::default(),
            reasoning: ReasoningItem::default(),
            tools: BTreeMap::new(),
            next_tool_to_add: 0,
            next_output_index: 0,
            output_items: Vec::new(),
            usage: None,
            finish_reason: None,
            terminal: false,
        }
    }

    /// Whether a terminal event was already emitted.
    pub(super) const fn is_terminal(&self) -> bool {
        self.terminal
    }

    /// Handles one complete upstream frame payload.
    pub(super) fn handle_payload(&mut self, payload: &str, out: &mut Vec<u8>) {
        if self.terminal {
            return;
        }
        if payload == DONE_PAYLOAD {
            self.finish(out);
            return;
        }
        let Ok(chunk) = serde_json::from_str::<Value>(payload) else {
            // Non-JSON data frames carry no recoverable content.
            return;
        };
        if let Some(error) = chunk.get("error").filter(|error| !error.is_null()) {
            let (code, message) = chat_error_fields(error);
            self.fail(out, &code, &message);
            return;
        }
        self.handle_chunk(&chunk, out);
    }

    /// Handles an upstream body that ignored `stream: true`.
    ///
    /// # Errors
    ///
    /// Returns a bounded failure when the body carries no readable completion.
    pub(super) fn handle_json_response(
        &mut self,
        body: &[u8],
        out: &mut Vec<u8>,
    ) -> Result<(), StreamFailure> {
        let completion: Value = serde_json::from_slice(body).map_err(|_| {
            StreamFailure::new(
                UPSTREAM_ERROR_CODE,
                "Upstream returned an unreadable completion body.",
            )
        })?;
        let choice = completion
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
            .ok_or_else(|| {
                StreamFailure::new(
                    UPSTREAM_ERROR_CODE,
                    "Upstream completion body carries no choices.",
                )
            })?;
        if let Some(message) = choice.get("message") {
            let mut delta = Map::new();
            for key in ["reasoning_content", "content"] {
                if let Some(text) = message.get(key).and_then(Value::as_str)
                    && !text.is_empty()
                {
                    delta.insert(key.to_owned(), Value::String(text.to_owned()));
                }
            }
            if let Some(tool_calls) = message.get("tool_calls").and_then(Value::as_array)
                && !tool_calls.is_empty()
            {
                delta.insert(
                    "tool_calls".to_owned(),
                    Value::Array(
                        tool_calls
                            .iter()
                            .enumerate()
                            .map(|(index, call)| {
                                json!({
                                    "index": index,
                                    "id": call.get("id").cloned().unwrap_or(Value::Null),
                                    "type": "function",
                                    "function": {
                                        "name": call
                                            .pointer("/function/name")
                                            .cloned()
                                            .unwrap_or(Value::Null),
                                        "arguments": call
                                            .pointer("/function/arguments")
                                            .cloned()
                                            .unwrap_or(Value::Null),
                                    },
                                })
                            })
                            .collect(),
                    ),
                );
            }
            if !delta.is_empty() {
                self.handle_chunk(
                    &json!({ "choices": [{ "index": 0, "delta": Value::Object(delta) }] }),
                    out,
                );
            }
        }
        let mut terminal = Map::new();
        terminal.insert("index".to_owned(), json!(0));
        terminal.insert("delta".to_owned(), json!({}));
        terminal.insert(
            "finish_reason".to_owned(),
            choice
                .get("finish_reason")
                .cloned()
                .unwrap_or_else(|| json!("stop")),
        );
        let mut chunk = json!({ "choices": [Value::Object(terminal)] });
        if let Some(usage) = completion.get("usage").filter(|usage| !usage.is_null()) {
            chunk["usage"] = usage.clone();
        }
        self.handle_chunk(&chunk, out);
        self.handle_payload(DONE_PAYLOAD, out);
        Ok(())
    }

    /// Ends the turn after an explicit `[DONE]`.
    pub(super) fn finish(&mut self, out: &mut Vec<u8>) {
        if self.terminal {
            return;
        }
        let finish_reason = self.finish_reason.clone();
        self.finish_inner(out, finish_reason.as_deref(), false);
    }

    /// Ends the turn at upstream EOF, applying the truncation rules.
    pub(super) fn finish_on_eof(&mut self, out: &mut Vec<u8>) {
        if self.terminal {
            return;
        }
        if self.finish_reason.is_none() && !self.has_substantive_output() {
            self.fail(
                out,
                STREAM_TRUNCATED_CODE,
                "Upstream stream ended without a terminal event or output.",
            );
            return;
        }
        let finish_reason = self.finish_reason.clone();
        let synthesized = finish_reason.is_none();
        self.finish_inner(out, finish_reason.as_deref(), synthesized);
    }

    /// Emits a bounded `response.failed` terminal event.
    pub(super) fn fail(&mut self, out: &mut Vec<u8>, code: &str, message: &str) {
        if self.terminal {
            return;
        }
        self.terminal = true;
        let response = json!({
            "id": self.response_id,
            "object": "response",
            "created_at": self.created_at,
            "status": "failed",
            "model": self.model,
            "output": [],
            "error": {
                "code": bounded_string(code.to_owned(), MAX_ERROR_CODE_CHARS),
                "message": bounded_string(message.to_owned(), MAX_ERROR_MESSAGE_CHARS),
            },
        });
        push_event(
            out,
            "response.failed",
            &json!({ "type": "response.failed", "response": response }),
        );
        out.extend_from_slice(done_payload());
    }

    fn finish_inner(&mut self, out: &mut Vec<u8>, finish_reason: Option<&str>, synthesized: bool) {
        let status = if finish_reason == Some("length") || synthesized {
            "incomplete"
        } else {
            "completed"
        };
        if status == "completed" && self.has_unusable_tool_calls() {
            self.fail(
                out,
                TOOL_CALL_DROPPED_CODE,
                "Upstream returned a tool call without a function name.",
            );
            return;
        }
        self.terminal = true;
        self.close_items(out);
        let mut response = Map::new();
        response.insert("id".to_owned(), Value::String(self.response_id.clone()));
        response.insert("object".to_owned(), Value::String("response".to_owned()));
        response.insert("created_at".to_owned(), json!(self.created_at));
        response.insert("status".to_owned(), Value::String(status.to_owned()));
        response.insert("model".to_owned(), Value::String(self.model.clone()));
        let mut output = std::mem::take(&mut self.output_items);
        output.sort_by_key(|(index, _)| *index);
        response.insert(
            "output".to_owned(),
            Value::Array(output.into_iter().map(|(_, item)| item).collect()),
        );
        if let Some(usage) = self.usage.clone() {
            response.insert("usage".to_owned(), usage);
        }
        if status == "incomplete" {
            response.insert(
                "incomplete_details".to_owned(),
                json!({ "reason": "max_output_tokens" }),
            );
        }
        push_event(
            out,
            "response.completed",
            &json!({ "type": "response.completed", "response": Value::Object(response) }),
        );
        out.extend_from_slice(done_payload());
    }

    fn handle_chunk(&mut self, chunk: &Value, out: &mut Vec<u8>) {
        if let Some(id) = chunk.get("id").and_then(Value::as_str)
            && !id.is_empty()
        {
            self.response_id = response_id_from_chat_id(id);
        }
        if let Some(model) = chunk.get("model").and_then(Value::as_str)
            && !model.is_empty()
        {
            self.model.clear();
            self.model.push_str(model);
        }
        if let Some(created) = chunk.get("created").and_then(Value::as_u64) {
            self.created_at = created;
        }
        if let Some(usage) = chunk.get("usage").filter(|usage| !usage.is_null()) {
            self.usage = chat_usage_to_responses_usage(usage);
        }
        self.ensure_started(out);
        let Some(choice) = chunk
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
        else {
            return;
        };
        if let Some(delta) = choice.get("delta") {
            if let Some(reasoning) = delta.get("reasoning_content").and_then(Value::as_str)
                && !reasoning.is_empty()
            {
                self.push_reasoning_delta(reasoning, out);
            }
            if let Some(content) = delta.get("content").and_then(Value::as_str)
                && !content.is_empty()
            {
                self.push_text_delta(content, out);
            }
            if let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) {
                for tool_call in tool_calls {
                    self.push_tool_call_delta(tool_call, out);
                }
            }
        }
        if let Some(finish_reason) = choice.get("finish_reason").and_then(Value::as_str) {
            self.finish_reason = Some(finish_reason.to_owned());
        }
    }

    fn ensure_started(&mut self, out: &mut Vec<u8>) {
        if self.started {
            return;
        }
        self.started = true;
        let response = json!({
            "id": self.response_id,
            "object": "response",
            "created_at": self.created_at,
            "status": "in_progress",
            "model": self.model,
            "output": [],
        });
        push_event(
            out,
            "response.created",
            &json!({ "type": "response.created", "response": response }),
        );
    }

    fn push_text_delta(&mut self, delta: &str, out: &mut Vec<u8>) {
        if !self.text.added {
            self.text.added = true;
            self.text.index = self.next_output_index();
            let item_id = self.message_item_id();
            push_event(
                out,
                "response.output_item.added",
                &json!({
                    "type": "response.output_item.added",
                    "output_index": self.text.index,
                    "item": {
                        "id": item_id,
                        "type": "message",
                        "status": "in_progress",
                        "role": "assistant",
                        "content": [],
                    },
                }),
            );
            push_event(
                out,
                "response.content_part.added",
                &json!({
                    "type": "response.content_part.added",
                    "item_id": item_id,
                    "output_index": self.text.index,
                    "content_index": 0,
                    "part": { "type": "output_text", "text": "", "annotations": [] },
                }),
            );
        }
        self.text.text.push_str(delta);
        let item_id = self.message_item_id();
        push_event(
            out,
            "response.output_text.delta",
            &json!({
                "type": "response.output_text.delta",
                "item_id": item_id,
                "output_index": self.text.index,
                "content_index": 0,
                "delta": delta,
            }),
        );
    }

    fn push_reasoning_delta(&mut self, delta: &str, out: &mut Vec<u8>) {
        if !self.reasoning.added {
            self.reasoning.added = true;
            self.reasoning.index = self.next_output_index();
            let item_id = self.reasoning_item_id();
            push_event(
                out,
                "response.output_item.added",
                &json!({
                    "type": "response.output_item.added",
                    "output_index": self.reasoning.index,
                    "item": {
                        "id": item_id,
                        "type": "reasoning",
                        "status": "in_progress",
                        "summary": [],
                    },
                }),
            );
            push_event(
                out,
                "response.reasoning_summary_part.added",
                &json!({
                    "type": "response.reasoning_summary_part.added",
                    "item_id": item_id,
                    "output_index": self.reasoning.index,
                    "summary_index": 0,
                    "part": { "type": "summary_text", "text": "" },
                }),
            );
        }
        self.reasoning.text.push_str(delta);
        let item_id = self.reasoning_item_id();
        push_event(
            out,
            "response.reasoning_summary_text.delta",
            &json!({
                "type": "response.reasoning_summary_text.delta",
                "item_id": item_id,
                "output_index": self.reasoning.index,
                "summary_index": 0,
                "delta": delta,
            }),
        );
    }

    fn push_tool_call_delta(&mut self, tool_call: &Value, out: &mut Vec<u8>) {
        let key = match tool_call.get("index").and_then(Value::as_u64) {
            Some(index) => index,
            None => self.resolve_tool_key_without_index(tool_call),
        };
        let id = tool_call.get("id").and_then(Value::as_str);
        let name = tool_call.pointer("/function/name").and_then(Value::as_str);
        let arguments = tool_call
            .pointer("/function/arguments")
            .and_then(Value::as_str)
            .unwrap_or_default();
        {
            let state = self.tools.entry(key).or_default();
            if let Some(id) = id.filter(|id| !id.is_empty()) {
                id.clone_into(&mut state.call_id);
            }
            if let Some(name) = name.filter(|name| !name.is_empty()) {
                name.clone_into(&mut state.chat_name);
            }
            state.arguments.push_str(arguments);
        }
        if !arguments.is_empty() {
            let state = self.tools.get(&key);
            if let Some(state) = state
                && state.added
                && !self.is_custom_tool(&state.chat_name)
                && let Some(item_index) = state.index
            {
                let item_id = self.tool_item_id(&state.chat_name, &state.call_id);
                push_event(
                    out,
                    "response.function_call_arguments.delta",
                    &json!({
                        "type": "response.function_call_arguments.delta",
                        "item_id": item_id,
                        "output_index": item_index,
                        "delta": arguments,
                    }),
                );
            }
        }
        self.flush_ready_tool_calls(out);
    }

    fn resolve_tool_key_without_index(&self, tool_call: &Value) -> u64 {
        let last_key = self.tools.keys().next_back().copied();
        let Some(id) = tool_call
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
        else {
            return last_key.unwrap_or(0);
        };
        if let Some((key, _)) = self.tools.iter().find(|(_, state)| state.call_id == id) {
            return *key;
        }
        match last_key {
            Some(key) => key.checked_add(1).unwrap_or(key),
            None => 0,
        }
    }

    fn flush_ready_tool_calls(&mut self, out: &mut Vec<u8>) {
        loop {
            let key = self.next_tool_to_add;
            let ready = self.tools.get(&key).is_some_and(|state| {
                state.added || (!state.call_id.is_empty() && !state.chat_name.is_empty())
            });
            if !ready {
                break;
            }
            if self.tools.get(&key).is_some_and(|state| state.added) {
                self.next_tool_to_add += 1;
                continue;
            }
            let Some(state) = self.tools.get(&key) else {
                break;
            };
            let arguments = state.arguments.clone();
            let chat_name = state.chat_name.clone();
            let call_id = state.call_id.clone();
            let index = self.next_output_index();
            let item = self.tool_item("in_progress", &chat_name, &arguments, &call_id);
            if let Some(state) = self.tools.get_mut(&key) {
                state.added = true;
                state.index = Some(index);
            }
            push_event(
                out,
                "response.output_item.added",
                &json!({
                    "type": "response.output_item.added",
                    "output_index": index,
                    "item": item,
                }),
            );
            if !arguments.is_empty() && !self.is_custom_tool(&chat_name) {
                let item_id = self.tool_item_id(&chat_name, &call_id);
                push_event(
                    out,
                    "response.function_call_arguments.delta",
                    &json!({
                        "type": "response.function_call_arguments.delta",
                        "item_id": item_id,
                        "output_index": index,
                        "delta": arguments,
                    }),
                );
            }
            self.next_tool_to_add += 1;
        }
    }

    fn close_items(&mut self, out: &mut Vec<u8>) {
        let mut closes = Vec::new();
        if self.reasoning.added {
            let item_id = self.reasoning_item_id();
            let text = self.reasoning.text.clone();
            push_event(
                out,
                "response.reasoning_summary_text.done",
                &json!({
                    "type": "response.reasoning_summary_text.done",
                    "item_id": item_id,
                    "output_index": self.reasoning.index,
                    "summary_index": 0,
                    "text": text,
                }),
            );
            push_event(
                out,
                "response.reasoning_summary_part.done",
                &json!({
                    "type": "response.reasoning_summary_part.done",
                    "item_id": item_id,
                    "output_index": self.reasoning.index,
                    "summary_index": 0,
                    "part": { "type": "summary_text", "text": text },
                }),
            );
            closes.push((
                self.reasoning.index,
                json!({
                    "id": item_id,
                    "type": "reasoning",
                    "summary": [{ "type": "summary_text", "text": text }],
                }),
            ));
        }
        if self.text.added {
            let item_id = self.message_item_id();
            let text = self.text.text.clone();
            push_event(
                out,
                "response.output_text.done",
                &json!({
                    "type": "response.output_text.done",
                    "item_id": item_id,
                    "output_index": self.text.index,
                    "content_index": 0,
                    "text": text,
                }),
            );
            push_event(
                out,
                "response.content_part.done",
                &json!({
                    "type": "response.content_part.done",
                    "item_id": item_id,
                    "output_index": self.text.index,
                    "content_index": 0,
                    "part": { "type": "output_text", "text": text, "annotations": [] },
                }),
            );
            closes.push((
                self.text.index,
                json!({
                    "id": item_id,
                    "type": "message",
                    "status": "completed",
                    "role": "assistant",
                    "content": [{
                        "type": "output_text",
                        "text": text,
                        "annotations": [],
                    }],
                }),
            ));
        }
        let mut tool_closes = self.close_tools(out);
        closes.append(&mut tool_closes);
        closes.sort_by_key(|(index, _)| *index);
        for (index, item) in closes {
            push_event(
                out,
                "response.output_item.done",
                &json!({
                    "type": "response.output_item.done",
                    "output_index": index,
                    "item": item,
                }),
            );
            self.output_items.push((index, item));
        }
    }

    fn close_tools(&mut self, out: &mut Vec<u8>) -> Vec<(u64, Value)> {
        let mut closes = Vec::new();
        let keys: Vec<u64> = self.tools.keys().copied().collect();
        for key in keys {
            let Some(state) = self.tools.get(&key) else {
                continue;
            };
            let chat_name = state.chat_name.clone();
            if chat_name.trim().is_empty() {
                continue;
            }
            let call_id = state.call_id.clone();
            let mut index = state.index;
            if index.is_none() {
                let assigned = self.next_output_index();
                let item = self.tool_item("in_progress", &chat_name, "", &call_id);
                if let Some(state) = self.tools.get_mut(&key) {
                    state.added = true;
                    state.index = Some(assigned);
                }
                push_event(
                    out,
                    "response.output_item.added",
                    &json!({
                        "type": "response.output_item.added",
                        "output_index": assigned,
                        "item": item,
                    }),
                );
                index = Some(assigned);
            }
            let Some(index) = index else {
                continue;
            };
            let arguments = canonical_arguments(&state_arguments(&self.tools, key));
            let item = self.tool_item("completed", &chat_name, &arguments, &call_id);
            let item_id = self.tool_item_id(&chat_name, &call_id);
            if self.is_custom_tool(&chat_name) {
                let input = custom_tool_input(&arguments);
                if !input.is_empty() {
                    push_event(
                        out,
                        "response.custom_tool_call_input.delta",
                        &json!({
                            "type": "response.custom_tool_call_input.delta",
                            "item_id": item_id,
                            "output_index": index,
                            "delta": input,
                        }),
                    );
                }
                push_event(
                    out,
                    "response.custom_tool_call_input.done",
                    &json!({
                        "type": "response.custom_tool_call_input.done",
                        "item_id": item_id,
                        "output_index": index,
                        "input": input,
                    }),
                );
            } else {
                push_event(
                    out,
                    "response.function_call_arguments.done",
                    &json!({
                        "type": "response.function_call_arguments.done",
                        "item_id": item_id,
                        "output_index": index,
                        "arguments": arguments,
                    }),
                );
            }
            closes.push((index, item));
        }
        closes
    }

    fn tool_item(&self, status: &str, chat_name: &str, arguments: &str, call_id: &str) -> Value {
        let origin = self.tool_names.get(chat_name);
        match origin.map(|origin| origin.kind) {
            Some(ToolOriginKind::ToolSearch) => json!({
                "type": "tool_search_call",
                "call_id": call_id,
                "status": status,
                "execution": "client",
                "arguments": tool_arguments_object(arguments),
            }),
            Some(ToolOriginKind::Custom) => json!({
                "id": format!("ctc_{call_id}"),
                "type": "custom_tool_call",
                "status": status,
                "call_id": call_id,
                "name": chat_name,
                "input": custom_tool_input(arguments),
            }),
            _ => {
                let name = origin.map_or(chat_name, |origin| origin.name.as_str());
                let mut item = Map::new();
                item.insert("id".to_owned(), Value::String(format!("fc_{call_id}")));
                item.insert("type".to_owned(), Value::String("function_call".to_owned()));
                item.insert("status".to_owned(), Value::String(status.to_owned()));
                item.insert("call_id".to_owned(), Value::String(call_id.to_owned()));
                item.insert("name".to_owned(), Value::String(name.to_owned()));
                item.insert("arguments".to_owned(), Value::String(arguments.to_owned()));
                if let Some(namespace) = origin.and_then(|origin| origin.namespace.as_ref()) {
                    item.insert("namespace".to_owned(), Value::String(namespace.clone()));
                }
                Value::Object(item)
            }
        }
    }

    fn tool_item_id(&self, chat_name: &str, call_id: &str) -> String {
        let call_id = if call_id.is_empty() {
            "call_unknown"
        } else {
            call_id
        };
        if self.is_custom_tool(chat_name) {
            format!("ctc_{call_id}")
        } else {
            format!("fc_{call_id}")
        }
    }

    fn message_item_id(&self) -> String {
        format!("{}_msg", self.response_id)
    }

    fn reasoning_item_id(&self) -> String {
        format!("rs_{}", self.response_id)
    }

    fn is_custom_tool(&self, chat_name: &str) -> bool {
        self.tool_names
            .get(chat_name)
            .is_some_and(|origin| origin.kind == ToolOriginKind::Custom)
    }

    fn has_unusable_tool_calls(&self) -> bool {
        !self.tools.is_empty()
            && self
                .tools
                .values()
                .all(|state| state.chat_name.trim().is_empty() || !state.added)
    }

    fn has_substantive_output(&self) -> bool {
        !self.text.text.trim().is_empty()
            || !self.reasoning.text.trim().is_empty()
            || !self.output_items.is_empty()
            || self
                .tools
                .values()
                .any(|state| state.added || !state.call_id.trim().is_empty())
    }

    fn next_output_index(&mut self) -> u64 {
        let index = self.next_output_index;
        self.next_output_index += 1;
        index
    }
}

fn state_arguments(tools: &BTreeMap<u64, ToolCallItem>, key: u64) -> String {
    tools
        .get(&key)
        .map(|state| state.arguments.clone())
        .unwrap_or_default()
}

fn push_event(out: &mut Vec<u8>, name: &str, payload: &Value) {
    out.extend_from_slice(b"event: ");
    out.extend_from_slice(name.as_bytes());
    out.extend_from_slice(b"\ndata: ");
    out.extend_from_slice(payload.to_string().as_bytes());
    out.extend_from_slice(b"\n\n");
}

/// The single `[DONE]` payload that ends every bridged turn.
pub(super) fn done_payload() -> &'static [u8] {
    b"data: [DONE]\n\n"
}

fn response_id_from_chat_id(id: &str) -> String {
    if id.starts_with("resp_") {
        id.to_owned()
    } else {
        format!("resp_{id}")
    }
}

fn chat_error_fields(error: &Value) -> (String, String) {
    let code = error
        .get("code")
        .or_else(|| error.get("type"))
        .and_then(Value::as_str)
        .filter(|code| !code.is_empty())
        .unwrap_or(UPSTREAM_ERROR_CODE);
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .filter(|message| !message.is_empty())
        .unwrap_or("Upstream reported an error.");
    (
        bounded_string(code.to_owned(), MAX_ERROR_CODE_CHARS),
        bounded_string(message.to_owned(), MAX_ERROR_MESSAGE_CHARS),
    )
}

/// Maps Chat usage onto the Responses usage object.
///
/// `None` when the upstream reported no usage object at all: a missing usage
/// object must not become fabricated billable usage. When the upstream did
/// report usage, Codex 0.155.1 requires `input_tokens`, `output_tokens`,
/// `total_tokens`, and both `input_tokens_details` fields, so an omitted count
/// is completed with an explicit zero.
fn chat_usage_to_responses_usage(usage: &Value) -> Option<Value> {
    if !usage.is_object() {
        return None;
    }
    let token = |keys: [&str; 2]| {
        keys.iter()
            .find_map(|key| usage.get(key).and_then(Value::as_u64))
    };
    let input = token(["prompt_tokens", "input_tokens"]);
    let output = token(["completion_tokens", "output_tokens"]);
    let total = token(["total_tokens", "total"]);
    if input.is_none() && output.is_none() && total.is_none() {
        return None;
    }
    let input = input.unwrap_or(0);
    let output = output.unwrap_or(0);
    let total = total.unwrap_or_else(|| input.saturating_add(output));
    let cached = usage
        .pointer("/prompt_tokens_details/cached_tokens")
        .or_else(|| usage.pointer("/input_tokens_details/cached_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cache_write = usage
        .pointer("/prompt_tokens_details/cache_write_tokens")
        .or_else(|| usage.pointer("/input_tokens_details/cache_write_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let mut mapped = Map::new();
    mapped.insert("input_tokens".to_owned(), json!(input));
    mapped.insert("output_tokens".to_owned(), json!(output));
    mapped.insert("total_tokens".to_owned(), json!(total));
    mapped.insert(
        "input_tokens_details".to_owned(),
        json!({ "cached_tokens": cached, "cache_write_tokens": cache_write }),
    );
    if let Some(reasoning) = usage
        .pointer("/completion_tokens_details/reasoning_tokens")
        .and_then(Value::as_u64)
    {
        mapped.insert(
            "output_tokens_details".to_owned(),
            json!({ "reasoning_tokens": reasoning }),
        );
    }
    Some(Value::Object(mapped))
}

fn canonical_arguments(arguments: &str) -> String {
    if arguments.trim().is_empty() {
        return String::new();
    }
    match serde_json::from_str::<Value>(arguments) {
        Ok(value) => canonical_value(&value).to_string(),
        Err(_) => arguments.to_owned(),
    }
}

fn canonical_value(value: &Value) -> Value {
    match value {
        Value::Object(object) => {
            let sorted: BTreeMap<&String, Value> = object
                .iter()
                .map(|(key, value)| (key, canonical_value(value)))
                .collect();
            Value::Object(
                sorted
                    .into_iter()
                    .map(|(key, value)| (key.clone(), value))
                    .collect(),
            )
        }
        Value::Array(items) => Value::Array(items.iter().map(canonical_value).collect()),
        other => other.clone(),
    }
}

fn tool_arguments_object(arguments: &str) -> Value {
    if arguments.trim().is_empty() {
        return json!({});
    }
    match serde_json::from_str::<Value>(arguments) {
        Ok(Value::Object(object)) => Value::Object(object),
        Ok(_) | Err(_) => json!({ "query": arguments }),
    }
}

fn custom_tool_input(arguments: &str) -> String {
    match serde_json::from_str::<Value>(arguments) {
        Ok(Value::Object(object)) => object
            .get("input")
            .and_then(Value::as_str)
            .unwrap_or(arguments)
            .to_owned(),
        _ => arguments.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde_json::{Value, json};

    use super::{
        ChatSseDecoder, ChatStreamBridge, FRAME_TOO_LARGE_CODE, MAX_CHAT_FRAME_BYTES,
        STREAM_TRUNCATED_CODE, TOOL_CALL_DROPPED_CODE, UPSTREAM_ERROR_CODE,
    };
    use crate::proxy::chat_bridge::{ToolOrigin, translate_request};

    fn origins(request: &Value) -> HashMap<String, ToolOrigin> {
        translate_request(&serde_json::to_vec(request).expect("request serializes"))
            .expect("request translates")
            .tool_names
    }

    fn function_origins() -> HashMap<String, ToolOrigin> {
        origins(&json!({
            "model": "m",
            "tools": [{ "type": "function", "name": "exec_command", "description": "Run",
                        "parameters": { "type": "object" } }],
            "input": [],
        }))
    }

    fn new_bridge(origins: HashMap<String, ToolOrigin>) -> ChatStreamBridge {
        ChatStreamBridge::new(
            "resp_test".to_owned(),
            1_700_000_000,
            "gpt-test".to_owned(),
            origins,
        )
    }

    /// Feeds `bytes` through the decoder into the bridge in `split`-sized chunks.
    fn run(bytes: &[u8], split: usize, origins: HashMap<String, ToolOrigin>) -> Vec<u8> {
        let mut decoder = ChatSseDecoder::default();
        let mut bridge = new_bridge(origins);
        let mut out = Vec::new();
        for chunk in bytes.chunks(split.max(1)) {
            match decoder.push(chunk) {
                Ok(payloads) => {
                    for payload in payloads {
                        bridge.handle_payload(&payload, &mut out);
                    }
                }
                Err(failure) => {
                    bridge.fail(&mut out, failure.code, failure.message);
                    break;
                }
            }
        }
        if !bridge.is_terminal() {
            bridge.finish_on_eof(&mut out);
        }
        out
    }

    fn event_names(out: &[u8]) -> Vec<String> {
        String::from_utf8_lossy(out)
            .lines()
            .filter_map(|line| line.strip_prefix("event: ").map(str::to_owned))
            .collect()
    }

    fn payloads(out: &[u8]) -> Vec<Value> {
        String::from_utf8_lossy(out)
            .lines()
            .filter_map(|line| line.strip_prefix("data: "))
            .filter(|payload| *payload != "[DONE]")
            .map(|payload| serde_json::from_str(payload).expect("payload is JSON"))
            .collect()
    }

    fn terminal_payload(out: &[u8]) -> Value {
        payloads(out).pop().expect("terminal payload")
    }

    fn done_items(out: &[u8]) -> Vec<Value> {
        payloads(out)
            .into_iter()
            .filter(|payload| payload["type"] == json!("response.output_item.done"))
            .map(|payload| payload["item"].clone())
            .collect()
    }

    const TEXT_STREAM: &str = concat!(
        "data: {\"id\":\"chatcmpl-1\",\"model\":\"gpt-test\",\"created\":1700000001,",
        "\"choices\":[{\"index\":0,\"delta\":{\"reasoning_content\":\"think \"}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"reasoning_content\":\"hard\"}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hel\"}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"lo\"}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,",
        "\"id\":\"call_1\",\"function\":{\"name\":\"exec_command\",\"arguments\":\"{\\\"cmd\\\":\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,",
        "\"function\":{\"arguments\":\"\\\"ls\\\"}\"}}]}}]}\n\n",
        "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":4,",
        "\"total_tokens\":14,\"prompt_tokens_details\":{\"cached_tokens\":3},",
        "\"completion_tokens_details\":{\"reasoning_tokens\":2}}}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
        "data: [DONE]\n\n",
    );

    #[test]
    fn decoder_splits_coalesced_crlf_and_multi_line_frames() {
        let mut decoder = ChatSseDecoder::default();
        let payloads = decoder
            .push(b": keep-alive\r\n\r\ndata: {\"a\":\r\ndata: 1}\n\nevent: ignored\n\ndata: [DONE]\n\n")
            .expect("within bounds");
        assert_eq!(
            payloads,
            vec!["{\"a\":\n1}".to_owned(), "[DONE]".to_owned()]
        );
        assert!(!decoder.has_pending());
    }

    #[test]
    fn partial_and_oversized_usage_are_completed_without_fabricating_a_total() {
        // A usage frame with only prompt tokens still must satisfy the Codex
        // contract: every required field present, missing counts zero.
        let partial = run(
            b"data: {\"choices\":[],\"usage\":{\"prompt_tokens\":7}}\n\ndata: [DONE]\n\n",
            4,
            function_origins(),
        );
        assert_eq!(
            terminal_payload(&partial)["response"]["usage"],
            json!({
                "input_tokens": 7,
                "output_tokens": 0,
                "total_tokens": 7,
                "input_tokens_details": { "cached_tokens": 0, "cache_write_tokens": 0 },
            })
        );

        // A gateway that reports a total smaller than the parts is not trusted
        // to overflow the local sum either.
        let overflow = run(
            format!(
                "data: {{\"choices\":[],\"usage\":{{\"prompt_tokens\":{max},\
                 \"completion_tokens\":{max}}}}}\n\ndata: [DONE]\n\n",
                max = u64::MAX
            )
            .as_bytes(),
            4,
            function_origins(),
        );
        assert_eq!(
            terminal_payload(&overflow)["response"]["usage"]["total_tokens"],
            json!(u64::MAX)
        );
    }

    #[test]
    fn oversized_incomplete_frame_fails_closed() {
        let mut decoder = ChatSseDecoder::default();
        let oversized = vec![b'x'; MAX_CHAT_FRAME_BYTES + 1];
        let failure = decoder.push(&oversized).expect_err("bounded carry");
        assert_eq!(failure.code, FRAME_TOO_LARGE_CODE);
    }

    #[test]
    fn oversized_complete_frame_fails_closed_in_the_same_chunk() {
        let mut decoder = ChatSseDecoder::default();
        let mut oversized = vec![b'x'; MAX_CHAT_FRAME_BYTES + 1];
        oversized.extend_from_slice(b"\n\n");
        let failure = decoder
            .push(&oversized)
            .expect_err("a complete oversized frame must be bounded");
        assert_eq!(failure.code, FRAME_TOO_LARGE_CODE);

        // A frame exactly at the bound still decodes.
        let mut decoder = ChatSseDecoder::default();
        let mut boundary = b"data: ".to_vec();
        boundary.extend(std::iter::repeat_n(b'x', MAX_CHAT_FRAME_BYTES - 6));
        boundary.extend_from_slice(b"\n\n");
        let payloads = decoder.push(&boundary).expect("at the bound");
        assert_eq!(payloads.len(), 1);
        assert_eq!(payloads[0].len(), MAX_CHAT_FRAME_BYTES - 6);
    }

    #[test]
    fn text_stream_translates_reasoning_text_and_usage() {
        let bytes = TEXT_STREAM.as_bytes();
        let out = run(bytes, bytes.len(), function_origins());
        assert_eq!(
            event_names(&out),
            vec![
                "response.created",
                "response.output_item.added",
                "response.reasoning_summary_part.added",
                "response.reasoning_summary_text.delta",
                "response.reasoning_summary_text.delta",
                "response.output_item.added",
                "response.content_part.added",
                "response.output_text.delta",
                "response.output_text.delta",
                "response.output_item.added",
                "response.function_call_arguments.delta",
                "response.function_call_arguments.delta",
                "response.reasoning_summary_text.done",
                "response.reasoning_summary_part.done",
                "response.output_text.done",
                "response.content_part.done",
                "response.function_call_arguments.done",
                "response.output_item.done",
                "response.output_item.done",
                "response.output_item.done",
                "response.completed",
            ]
        );
        let items = done_items(&out);
        assert_eq!(items[0]["type"], json!("reasoning"));
        assert_eq!(items[0]["summary"][0]["text"], json!("think hard"));
        assert_eq!(items[1]["content"][0]["text"], json!("Hello"));
        assert_eq!(items[2]["name"], json!("exec_command"));
        assert_eq!(items[2]["arguments"], json!("{\"cmd\":\"ls\"}"));
        assert_eq!(items[2]["call_id"], json!("call_1"));
        assert_eq!(items[2]["id"], json!("fc_call_1"));
        let terminal = terminal_payload(&out);
        assert_eq!(terminal["response"]["status"], json!("completed"));
        assert_eq!(terminal["response"]["model"], json!("gpt-test"));
        assert_eq!(terminal["response"]["id"], json!("resp_chatcmpl-1"));
        assert_eq!(
            terminal["response"]["usage"],
            json!({
                "input_tokens": 10,
                "output_tokens": 4,
                "total_tokens": 14,
                "input_tokens_details": { "cached_tokens": 3, "cache_write_tokens": 0 },
                "output_tokens_details": { "reasoning_tokens": 2 },
            })
        );
        assert_eq!(
            String::from_utf8_lossy(&out)
                .matches("data: [DONE]")
                .count(),
            1
        );
    }

    #[test]
    fn every_byte_split_produces_the_same_stream() {
        let bytes = TEXT_STREAM.as_bytes();
        let reference = run(bytes, bytes.len(), function_origins());
        for split in 1..=bytes.len() {
            let candidate = run(bytes, split, function_origins());
            assert_eq!(candidate, reference, "split at {split}");
        }
    }

    #[test]
    fn length_finish_reason_completes_as_incomplete() {
        let stream = concat!(
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"partial\"}}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"length\"}]}\n\n",
            "data: [DONE]\n\n",
        );
        let out = run(stream.as_bytes(), usize::MAX, function_origins());
        let names = event_names(&out);
        assert!(!names.contains(&"response.incomplete".to_owned()));
        let terminal = terminal_payload(&out);
        assert_eq!(terminal["response"]["status"], json!("incomplete"));
        assert_eq!(
            terminal["response"]["incomplete_details"],
            json!({ "reason": "max_output_tokens" })
        );
    }

    #[test]
    fn eof_without_terminal_event_follows_the_truncation_rules() {
        let with_output = run(
            b"data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"}}]}\n\n",
            usize::MAX,
            function_origins(),
        );
        assert_eq!(
            terminal_payload(&with_output)["response"]["status"],
            json!("incomplete")
        );

        let without_output = run(b"", usize::MAX, function_origins());
        assert_eq!(event_names(&without_output), vec!["response.failed"]);
        let terminal = terminal_payload(&without_output);
        assert_eq!(
            terminal["response"]["error"]["code"],
            json!(STREAM_TRUNCATED_CODE)
        );

        let pending_frame = run(
            b"data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"}}]}",
            usize::MAX,
            function_origins(),
        );
        let terminal = terminal_payload(&pending_frame);
        assert_eq!(terminal["response"]["status"], json!("failed"));
        assert_eq!(
            terminal["response"]["error"]["code"],
            json!(STREAM_TRUNCATED_CODE)
        );
    }

    #[test]
    fn upstream_error_frames_and_bodies_fail_the_turn() {
        let stream = concat!(
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"}}]}\n\n",
            "data: {\"error\":{\"type\":\"rate_limit_error\",\"message\":\"slow down\"}}\n\n",
        );
        let out = run(stream.as_bytes(), usize::MAX, function_origins());
        let terminal = terminal_payload(&out);
        assert_eq!(terminal["response"]["status"], json!("failed"));
        assert_eq!(
            terminal["response"]["error"]["code"],
            json!("rate_limit_error")
        );
        assert_eq!(terminal["response"]["error"]["message"], json!("slow down"));

        let mut bridge = new_bridge(function_origins());
        let mut out = Vec::new();
        let failure = bridge
            .handle_json_response(b"not json", &mut out)
            .expect_err("unreadable body");
        assert_eq!(failure.code, UPSTREAM_ERROR_CODE);

        let mut bridge = new_bridge(function_origins());
        let mut out = Vec::new();
        let failure = bridge
            .handle_json_response(br#"{"choices":[]}"#, &mut out)
            .expect_err("missing choices");
        assert_eq!(failure.code, UPSTREAM_ERROR_CODE);
    }

    #[test]
    fn malformed_frames_are_ignored_and_missing_usage_is_not_fabricated() {
        let stream = concat!(
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"ok\"}}]}\n\n",
            "data: not json\n\n",
            "data: [DONE]\n\n",
        );
        let out = run(stream.as_bytes(), 3, function_origins());
        let terminal = terminal_payload(&out);
        assert_eq!(terminal["response"]["status"], json!("completed"));
        assert!(terminal["response"].get("usage").is_none());
        assert_eq!(done_items(&out)[0]["content"][0]["text"], json!("ok"));
    }

    #[test]
    fn tool_calls_restore_wrapped_tool_families() {
        let request = json!({
            "model": "m",
            "tools": [
                { "type": "function", "name": "exec_command", "description": "Run",
                  "parameters": { "type": "object" } },
                { "type": "namespace", "name": "multi_agent_v1", "description": "Agents", "tools": [
                    { "type": "function", "name": "close_agent", "description": "Close",
                      "parameters": { "type": "object" } }
                ]},
                { "type": "custom", "name": "apply_patch", "description": "Patch" },
                { "type": "tool_search" },
            ],
            "input": [],
        });
        let stream = concat!(
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,",
            "\"id\":\"c1\",\"function\":{\"name\":\"exec_command\",\"arguments\":\"{}\"}}]}}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":1,",
            "\"id\":\"c2\",\"function\":{\"name\":\"multi_agent_v1__close_agent\",\"arguments\":\"{\\\"agent\\\":1}\"}}]}}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":2,",
            "\"id\":\"c3\",\"function\":{\"name\":\"apply_patch\",\"arguments\":\"{\\\"input\\\":\\\"*** Begin Patch\\\"}\"}}]}}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":3,",
            "\"id\":\"c4\",\"function\":{\"name\":\"tool_search\",\"arguments\":\"{\\\"query\\\":\\\"linear\\\"}\"}}]}}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
            "data: [DONE]\n\n",
        );
        let out = run(stream.as_bytes(), 5, origins(&request));
        let items = done_items(&out);
        assert_eq!(items.len(), 4);
        assert_eq!(items[0]["type"], json!("function_call"));
        assert_eq!(items[0]["name"], json!("exec_command"));
        assert_eq!(items[1]["type"], json!("function_call"));
        assert_eq!(items[1]["name"], json!("close_agent"));
        assert_eq!(items[1]["namespace"], json!("multi_agent_v1"));
        assert_eq!(items[2]["type"], json!("custom_tool_call"));
        assert_eq!(items[2]["name"], json!("apply_patch"));
        assert_eq!(items[2]["input"], json!("*** Begin Patch"));
        assert_eq!(items[2]["id"], json!("ctc_c3"));
        assert_eq!(items[3]["type"], json!("tool_search_call"));
        assert_eq!(items[3]["arguments"], json!({ "query": "linear" }));
        assert_eq!(items[3]["execution"], json!("client"));
        assert!(items[3].get("id").is_none());
        let names = event_names(&out);
        assert!(names.contains(&"response.custom_tool_call_input.delta".to_owned()));
        assert!(names.contains(&"response.custom_tool_call_input.done".to_owned()));
        // Function and tool-search calls stream argument deltas; custom tools
        // only carry their unwrapped input.
        assert_eq!(
            names
                .iter()
                .filter(|name| *name == "response.function_call_arguments.delta")
                .count(),
            3
        );
    }

    #[test]
    fn tool_call_without_a_name_fails_the_turn() {
        let stream = concat!(
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,",
            "\"id\":\"c1\",\"function\":{\"arguments\":\"{}\"}}]}}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
            "data: [DONE]\n\n",
        );
        let out = run(stream.as_bytes(), usize::MAX, function_origins());
        let terminal = terminal_payload(&out);
        assert_eq!(terminal["response"]["status"], json!("failed"));
        assert_eq!(
            terminal["response"]["error"]["code"],
            json!(TOOL_CALL_DROPPED_CODE)
        );
    }

    #[test]
    fn non_stream_json_uses_the_same_item_builder() {
        let mut bridge = new_bridge(function_origins());
        let mut out = Vec::new();
        bridge
            .handle_json_response(
                br#"{"id":"chatcmpl-9","model":"gpt-test","created":1700000002,
                     "choices":[{"index":0,"message":{"content":"hi","reasoning_content":"why",
                       "tool_calls":[{"id":"c1","type":"function",
                         "function":{"name":"exec_command","arguments":"{\"cmd\":\"ls\"}"}}]},
                       "finish_reason":"tool_calls"}],
                     "usage":{"prompt_tokens":1,"completion_tokens":2}}"#,
                &mut out,
            )
            .expect("readable completion");
        assert!(bridge.is_terminal());
        let items = done_items(&out);
        assert_eq!(items[0]["type"], json!("reasoning"));
        assert_eq!(items[0]["summary"][0]["text"], json!("why"));
        assert_eq!(items[1]["content"][0]["text"], json!("hi"));
        assert_eq!(items[2]["arguments"], json!("{\"cmd\":\"ls\"}"));
        let terminal = terminal_payload(&out);
        assert_eq!(terminal["response"]["status"], json!("completed"));
        assert_eq!(terminal["response"]["usage"]["total_tokens"], json!(3));
        assert_eq!(
            String::from_utf8_lossy(&out)
                .matches("data: [DONE]")
                .count(),
            1
        );
    }

    #[test]
    fn a_terminal_turn_ignores_later_frames() {
        let mut bridge = new_bridge(function_origins());
        let mut out = Vec::new();
        bridge.handle_payload("[DONE]", &mut out);
        let after_terminal = out.clone();
        bridge.handle_payload(
            "{\"choices\":[{\"index\":0,\"delta\":{\"content\":\"late\"}}]}",
            &mut out,
        );
        bridge.handle_payload("[DONE]", &mut out);
        assert_eq!(out, after_terminal);
    }
}
