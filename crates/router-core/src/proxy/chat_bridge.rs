//! Responses → Chat Completions request translation.
//!
//! Pure shape conversion for a per-route bridge: no transport, database,
//! logging, or route-health access. Every shape the Chat Completions wire
//! format cannot express either fails closed with a bounded [`BridgeError`] or
//! is omitted with an explicit [`CompatibilityMarker`]; nothing is dropped
//! silently and no request content is copied into diagnostics.
//!
//! Unknown *input item* types and unknown *top-level fields* fail closed
//! because their semantics are unknown: a request that reaches the wrong wire
//! shape must surface as a typed, non-striking client error instead of a
//! silently weakened conversation.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;

use bytes::Bytes;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

/// Stable error code returned for a request the Chat upstream cannot express.
pub(super) const UNSUPPORTED_REQUEST_CODE: &str = "chat_bridge_unsupported_request";

/// Chat Completions tool names are commonly capped at 64 bytes.
const MAX_CHAT_TOOL_NAME_BYTES: usize = 64;

/// Synthetic hosted-tool declaration name used when a request declares the
/// hosted `tool_search` tool.
const TOOL_SEARCH_NAME: &str = "tool_search";

/// Model-visible synthetic declaration text.
///
/// The three constants below and the `tool_search` parameter schema in
/// [`ToolRegistry::declare_tool_search`] are reproduced verbatim (MIT) from the
/// reference bridge at <https://github.com/farion1231/cc-switch>, commit
/// `0555c09d9aa06e5f93bf6d57ecf8faa9e6305c4b`,
/// `src-tauri/src/proxy/providers/transform_codex_chat.rs`. Copyright (c) 2025
/// Jason Young. The literal is stable, tested, and defines the model-visible
/// tool surface, so changing any word changes upstream behaviour. See
/// `THIRD_PARTY_NOTICES.md` (`provenance:cc-switch-chat-bridge`).
const TOOL_SEARCH_DESCRIPTION: &str =
    "Search and load Codex tools, plugins, connectors, and MCP namespaces for the current task.";

/// Chat-schema description of the wrapped string input of a custom tool.
const CUSTOM_TOOL_INPUT_DESCRIPTION: &str = "Raw string input for the original custom tool. Preserve formatting exactly and follow the original tool definition embedded in the description.";

/// Heading that prefixes an embedded custom-tool definition.
const CUSTOM_TOOL_DEFINITION_HEADING: &str = "Original tool definition:";

/// Effort values the fixed MVP `OpenAICompatible` profile accepts.
const REASONING_EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

/// Top-level Responses request fields this translator understands.
const KNOWN_TOP_LEVEL_FIELDS: [&str; 15] = [
    "model",
    "instructions",
    "input",
    "tools",
    "tool_choice",
    "parallel_tool_calls",
    "reasoning",
    "store",
    "stream",
    "include",
    "service_tier",
    "prompt_cache_key",
    "text",
    "client_metadata",
    "max_output_tokens",
];

/// A bounded, content-free reason a field was represented differently.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CompatibilityMarker {
    /// The synthetic `tool_search` function replaced a hosted-tool declaration.
    ToolSearchSynthetic,
    /// A custom tool's original definition was embedded in its description.
    CustomToolDescriptionEmbedded,
    /// Readable reasoning was replayed into an assistant turn.
    ReadableReasoningReplay,
    /// `reasoning.effort` was mapped to the top-level Chat field.
    ReasoningEffortMapped,
    /// `reasoning.summary` has no Chat representation and was omitted.
    ReasoningSummaryOmitted,
    /// An empty reasoning carrier was omitted.
    EmptyReasoningOmitted,
    /// `include` has no Chat representation and was omitted.
    IncludeOmitted,
    /// `store` requested server-side storage the Chat shape cannot express.
    StoreOmitted,
    /// `service_tier` has no Chat representation and was omitted.
    ServiceTierOmitted,
    /// Codex client metadata has no Chat representation and was omitted.
    ClientMetadataOmitted,
    /// The default-form `web_search` declaration was omitted.
    WebSearchDeclarationOmitted,
    /// `tool_choice`/`parallel_tool_calls` were omitted because no tools remain.
    ToolControlsOmitted,
    /// Non-format `text` options were omitted.
    TextOptionsOmitted,
    /// Tools carried by `additional_tools` were extracted; the carrier was dropped.
    AdditionalToolsExtracted,
}

impl CompatibilityMarker {
    /// Bounded identifier for the intentional representation change.
    ///
    /// Production emits only the single bounded compatibility event; the
    /// identifiers exist so tests can assert exactly which shapes were
    /// represented differently.
    #[cfg(test)]
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::ToolSearchSynthetic => "tool_search_synthetic",
            Self::CustomToolDescriptionEmbedded => "custom_tool_description_embedded",
            Self::ReadableReasoningReplay => "readable_reasoning_replay",
            Self::ReasoningEffortMapped => "reasoning_effort_mapped",
            Self::ReasoningSummaryOmitted => "reasoning_summary_omitted",
            Self::EmptyReasoningOmitted => "empty_reasoning_omitted",
            Self::IncludeOmitted => "include_omitted",
            Self::StoreOmitted => "store_omitted",
            Self::ServiceTierOmitted => "service_tier_omitted",
            Self::ClientMetadataOmitted => "client_metadata_omitted",
            Self::WebSearchDeclarationOmitted => "web_search_declaration_omitted",
            Self::ToolControlsOmitted => "tool_controls_omitted",
            Self::TextOptionsOmitted => "text_options_omitted",
            Self::AdditionalToolsExtracted => "additional_tools_extracted",
        }
    }
}

/// Typed, bounded translation failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct BridgeError {
    /// Stable machine code; always [`UNSUPPORTED_REQUEST_CODE`].
    pub code: &'static str,
    /// Bounded feature identifier naming the unsupported shape.
    pub feature: &'static str,
}

impl BridgeError {
    /// A client that asked for a non-streaming Responses body on a Chat route.
    ///
    /// The bridge only produces Responses SSE in this release; synthesizing a
    /// non-streaming Responses body is out of scope, so the attempt fails closed
    /// before any upstream send.
    pub(super) const CLIENT_NON_STREAMING: Self = Self {
        code: UNSUPPORTED_REQUEST_CODE,
        feature: "client_non_streaming",
    };

    const fn new(feature: &'static str) -> Self {
        Self {
            code: UNSUPPORTED_REQUEST_CODE,
            feature,
        }
    }
}

/// Where a translated Chat tool came from.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) enum ToolOriginKind {
    /// A Responses `function` tool.
    Function,
    /// A Responses `custom` tool.
    Custom,
    /// The synthetic hosted-tool bridge for `tool_search`.
    ToolSearch,
}

/// Reversible identity of one translated Chat tool.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ToolOrigin {
    /// Original tool family.
    pub kind: ToolOriginKind,
    /// Original Responses tool name.
    pub name: String,
    /// Declaring namespace, when the tool came from a `namespace` tool.
    pub namespace: Option<String>,
}

/// Result of translating one Responses request body.
#[derive(Debug)]
pub(super) struct ChatRequestTranslation {
    /// Serialized Chat Completions request body.
    pub body: Bytes,
    /// Bounded compatibility markers for this translation.
    pub compatibility: Vec<CompatibilityMarker>,
    /// Chat tool name → original origin, used to reverse response items.
    pub tool_names: HashMap<String, ToolOrigin>,
}

/// Translates one Responses request body into a Chat Completions body.
///
/// # Errors
///
/// Returns a bounded [`BridgeError`] when the request contains a shape or field
/// the Chat Completions wire format cannot express safely.
pub(super) fn translate_request(body: &[u8]) -> Result<ChatRequestTranslation, BridgeError> {
    let request: Value =
        serde_json::from_slice(body).map_err(|_| BridgeError::new("invalid_json"))?;
    let object = request
        .as_object()
        .ok_or_else(|| BridgeError::new("invalid_json"))?;
    reject_unknown_top_level_fields(object)?;

    let model = object
        .get("model")
        .and_then(Value::as_str)
        .ok_or_else(|| BridgeError::new("model"))?;

    let mut translator = RequestTranslator::default();
    if let Some(instructions) = optional(object, "instructions").and_then(Value::as_str)
        && !instructions.is_empty()
    {
        translator.push_system_text(instructions);
    }
    if let Some(tools) = optional(object, "tools") {
        translator.declare_tools(tools)?;
    }
    let input = object
        .get("input")
        .and_then(Value::as_array)
        .ok_or_else(|| BridgeError::new("input"))?;
    for item in input {
        translator.collect_declared_tools(item)?;
    }
    for item in input {
        translator.push_input_item(item)?;
    }
    translator.flush_tool_calls();
    translator.attach_leftover_reasoning();
    if reasoning_summary_omitted(optional(object, "reasoning")) {
        translator.push_marker(CompatibilityMarker::ReasoningSummaryOmitted);
    }
    if optional(object, "include").is_some() {
        translator.push_marker(CompatibilityMarker::IncludeOmitted);
    }
    if optional(object, "store").and_then(Value::as_bool) == Some(true) {
        translator.push_marker(CompatibilityMarker::StoreOmitted);
    }
    if optional(object, "service_tier").is_some() {
        translator.push_marker(CompatibilityMarker::ServiceTierOmitted);
    }
    if optional(object, "client_metadata").is_some() {
        translator.push_marker(CompatibilityMarker::ClientMetadataOmitted);
    }

    let mut chat = Map::new();
    chat.insert("model".to_owned(), Value::String(model.to_owned()));
    chat.insert(
        "messages".to_owned(),
        Value::Array(std::mem::take(&mut translator.messages)),
    );
    chat.insert("stream".to_owned(), Value::Bool(true));
    chat.insert(
        "stream_options".to_owned(),
        json!({ "include_usage": true }),
    );
    translator.finish_tools(&mut chat, object)?;
    if let Some(effort) = reasoning_effort(optional(object, "reasoning"))? {
        chat.insert("reasoning_effort".to_owned(), Value::String(effort));
        translator.push_marker(CompatibilityMarker::ReasoningEffortMapped);
    }
    if let Some(cache_key) = optional(object, "prompt_cache_key").and_then(Value::as_str) {
        chat.insert(
            "prompt_cache_key".to_owned(),
            Value::String(cache_key.to_owned()),
        );
    }
    if let Some(limit) = optional(object, "max_output_tokens") {
        chat.insert("max_completion_tokens".to_owned(), limit.clone());
    }
    if let Some(text) = optional(object, "text") {
        apply_text_options(&mut chat, text, &mut translator.compatibility)?;
    }

    let serialized =
        serde_json::to_vec(&Value::Object(chat)).map_err(|_| BridgeError::new("invalid_json"))?;
    Ok(ChatRequestTranslation {
        body: Bytes::from(serialized),
        compatibility: translator.compatibility,
        tool_names: translator.registry.chat_names,
    })
}

fn reject_unknown_top_level_fields(object: &Map<String, Value>) -> Result<(), BridgeError> {
    if object
        .keys()
        .any(|key| !KNOWN_TOP_LEVEL_FIELDS.contains(&key.as_str()))
    {
        return Err(BridgeError::new("unknown_field"));
    }
    Ok(())
}

/// Reads an optional field, treating an explicit `null` as absent.
fn optional<'a>(object: &'a Map<String, Value>, key: &str) -> Option<&'a Value> {
    object.get(key).filter(|value| !value.is_null())
}

fn reasoning_summary_omitted(reasoning: Option<&Value>) -> bool {
    reasoning
        .and_then(Value::as_object)
        .and_then(|object| object.get("summary"))
        .is_some_and(|summary| !summary.is_null() && summary.as_str() != Some("auto"))
}

fn reasoning_effort(reasoning: Option<&Value>) -> Result<Option<String>, BridgeError> {
    let Some(reasoning) = reasoning else {
        return Ok(None);
    };
    let Some(object) = reasoning.as_object() else {
        return Err(BridgeError::new("reasoning_shape"));
    };
    for key in object.keys() {
        if key != "effort" && key != "summary" {
            return Err(BridgeError::new("reasoning_shape"));
        }
    }
    let Some(effort) = object.get("effort") else {
        return Ok(None);
    };
    let Some(effort) = effort.as_str() else {
        return Err(BridgeError::new("reasoning_shape"));
    };
    if !REASONING_EFFORTS.contains(&effort) {
        return Err(BridgeError::new("reasoning_effort"));
    }
    Ok(Some(effort.to_owned()))
}

fn apply_text_options(
    chat: &mut Map<String, Value>,
    text: &Value,
    markers: &mut Vec<CompatibilityMarker>,
) -> Result<(), BridgeError> {
    let object = text
        .as_object()
        .ok_or_else(|| BridgeError::new("text_shape"))?;
    let mut omitted = false;
    for key in object.keys() {
        if key != "format" {
            omitted = true;
        }
    }
    if omitted {
        markers.push(CompatibilityMarker::TextOptionsOmitted);
    }
    let Some(format) = object.get("format") else {
        return Ok(());
    };
    match format.get("type").and_then(Value::as_str) {
        Some("text") => Ok(()),
        Some("json_object") => {
            chat.insert(
                "response_format".to_owned(),
                json!({ "type": "json_object" }),
            );
            Ok(())
        }
        Some("json_schema") => {
            let name = format
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| BridgeError::new("text_format"))?;
            let schema = format
                .get("schema")
                .cloned()
                .ok_or_else(|| BridgeError::new("text_format"))?;
            let mut json_schema = Map::new();
            json_schema.insert("name".to_owned(), Value::String(name.to_owned()));
            if let Some(strict) = format.get("strict") {
                json_schema.insert("strict".to_owned(), strict.clone());
            }
            json_schema.insert("schema".to_owned(), schema);
            chat.insert(
                "response_format".to_owned(),
                json!({ "type": "json_schema", "json_schema": Value::Object(json_schema) }),
            );
            Ok(())
        }
        _ => Err(BridgeError::new("text_format")),
    }
}

fn canonical_json(value: &Value) -> String {
    canonical_value(value).to_string()
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

#[derive(Default)]
struct RequestTranslator {
    messages: Vec<Value>,
    pending_tool_calls: Vec<Value>,
    pending_reasoning: String,
    system_index: Option<usize>,
    registry: ToolRegistry,
    compatibility: Vec<CompatibilityMarker>,
}

impl RequestTranslator {
    fn push_marker(&mut self, marker: CompatibilityMarker) {
        if !self.compatibility.contains(&marker) {
            self.compatibility.push(marker);
        }
    }

    fn push_system_text(&mut self, text: &str) {
        if let Some(index) = self.system_index {
            if let Some(Value::Object(message)) = self.messages.get_mut(index) {
                let existing = message
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                let merged = if existing.is_empty() {
                    text.to_owned()
                } else {
                    format!("{existing}\n\n{text}")
                };
                message.insert("content".to_owned(), Value::String(merged));
            }
            return;
        }
        self.system_index = Some(self.messages.len());
        self.messages
            .push(json!({ "role": "system", "content": text }));
    }

    fn declare_tools(&mut self, tools: &Value) -> Result<(), BridgeError> {
        let tools = tools
            .as_array()
            .ok_or_else(|| BridgeError::new("tools_shape"))?;
        for tool in tools {
            self.declare_tool(tool)?;
        }
        Ok(())
    }

    fn declare_tool(&mut self, tool: &Value) -> Result<(), BridgeError> {
        let kind = tool
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| BridgeError::new("tools_shape"))?;
        match kind {
            "function" => {
                let name = tool_name(tool)?;
                let strict = optional_value(tool, "strict");
                let description = optional_value(tool, "description");
                self.registry.declare_function(
                    &name,
                    None,
                    description,
                    &normalize_function_parameters(tool.get("parameters")),
                    strict,
                    false,
                )?;
                Ok(())
            }
            "custom" => {
                let name = tool_name(tool)?;
                self.registry.declare_custom(&name, None, tool)?;
                self.push_marker(CompatibilityMarker::CustomToolDescriptionEmbedded);
                Ok(())
            }
            "namespace" => {
                let namespace = tool_name(tool)?;
                let inner = tool
                    .get("tools")
                    .and_then(Value::as_array)
                    .ok_or_else(|| BridgeError::new("namespace_shape"))?;
                for child in inner {
                    let child_kind = child
                        .get("type")
                        .and_then(Value::as_str)
                        .ok_or_else(|| BridgeError::new("namespace_shape"))?;
                    match child_kind {
                        "function" => {
                            let name = tool_name(child)?;
                            let strict = optional_value(child, "strict");
                            let description = optional_value(child, "description");
                            self.registry.declare_function(
                                &name,
                                Some(&namespace),
                                description,
                                &normalize_function_parameters(child.get("parameters")),
                                strict,
                                false,
                            )?;
                        }
                        "custom" => {
                            let name = tool_name(child)?;
                            self.registry
                                .declare_custom(&name, Some(&namespace), child)?;
                            self.push_marker(CompatibilityMarker::CustomToolDescriptionEmbedded);
                        }
                        _ => return Err(BridgeError::new("namespace_tool")),
                    }
                }
                Ok(())
            }
            "tool_search" => {
                self.registry.declare_tool_search()?;
                self.push_marker(CompatibilityMarker::ToolSearchSynthetic);
                Ok(())
            }
            "web_search" => {
                if is_default_web_search(tool) {
                    self.push_marker(CompatibilityMarker::WebSearchDeclarationOmitted);
                    Ok(())
                } else {
                    Err(BridgeError::new("web_search_options"))
                }
            }
            _ => Err(BridgeError::new("tool_type")),
        }
    }

    /// Declares tools carried by history items before any item is translated,
    /// so tool resolution never depends on item order.
    fn collect_declared_tools(&mut self, item: &Value) -> Result<(), BridgeError> {
        let Some(object) = item.as_object() else {
            return Ok(());
        };
        match object.get("type").and_then(Value::as_str) {
            Some("additional_tools") => self.declare_carried_tools(object, "additional_tools"),
            Some("tool_search_output") => match object.get("tools") {
                Some(tools) => self.declare_tools(tools),
                None => Ok(()),
            },
            _ => Ok(()),
        }
    }

    fn push_input_item(&mut self, item: &Value) -> Result<(), BridgeError> {
        let object = item
            .as_object()
            .ok_or_else(|| BridgeError::new("item_shape"))?;
        let kind = object
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| BridgeError::new("item_shape"))?;
        match kind {
            "message" => self.push_message(object),
            "reasoning" => self.push_reasoning(object),
            "function_call" => self.push_tool_call(object, ToolOriginKind::Function),
            "custom_tool_call" => self.push_tool_call(object, ToolOriginKind::Custom),
            "tool_search_call" => self.push_tool_search_call(object),
            "function_call_output" => self.push_tool_output(object, ToolOriginKind::Function),
            "custom_tool_call_output" => self.push_tool_output(object, ToolOriginKind::Custom),
            "tool_search_output" => self.push_tool_search_output(object),
            "additional_tools" => {
                self.push_additional_tools();
                Ok(())
            }
            other => Err(BridgeError::new(unsupported_item_feature(other))),
        }
    }

    fn push_message(&mut self, object: &Map<String, Value>) -> Result<(), BridgeError> {
        let role = object
            .get("role")
            .and_then(Value::as_str)
            .ok_or_else(|| BridgeError::new("message_role"))?;
        let content = object
            .get("content")
            .ok_or_else(|| BridgeError::new("message_content"))?;
        match role {
            "system" | "developer" => {
                let text = text_content(content)?;
                self.push_system_text(&text);
                Ok(())
            }
            "user" => {
                self.attach_leftover_reasoning();
                let content = chat_content(content)?;
                self.messages
                    .push(json!({ "role": "user", "content": content }));
                Ok(())
            }
            "assistant" => {
                self.flush_tool_calls();
                let content = text_content(content)?;
                let mut message = Map::new();
                message.insert("role".to_owned(), Value::String("assistant".to_owned()));
                message.insert("content".to_owned(), Value::String(content));
                if let Some(reasoning) = self.take_pending_reasoning(None) {
                    message.insert("reasoning_content".to_owned(), Value::String(reasoning));
                }
                self.messages.push(Value::Object(message));
                Ok(())
            }
            _ => Err(BridgeError::new("message_role")),
        }
    }

    fn take_pending_reasoning(&mut self, existing: Option<&str>) -> Option<String> {
        if self.pending_reasoning.is_empty() {
            return None;
        }
        let pending = std::mem::take(&mut self.pending_reasoning);
        let merged = append_reasoning_text(existing.unwrap_or_default(), &pending);
        if merged.is_some() {
            self.push_marker(CompatibilityMarker::ReadableReasoningReplay);
        }
        merged
    }

    /// Replays reasoning that has no following assistant turn on the previous
    /// assistant message, mirroring the evidenced reference behavior.
    fn attach_leftover_reasoning(&mut self) {
        if self.pending_reasoning.is_empty() {
            return;
        }
        let Some(index) = self
            .messages
            .iter()
            .rposition(|message| message.get("role").and_then(Value::as_str) == Some("assistant"))
        else {
            return;
        };
        let existing = self
            .messages
            .get(index)
            .and_then(|message| message.get("reasoning_content"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        if let Some(reasoning) = self.take_pending_reasoning(existing.as_deref())
            && let Some(Value::Object(message)) = self.messages.get_mut(index)
        {
            message.insert("reasoning_content".to_owned(), Value::String(reasoning));
        }
    }

    fn push_reasoning(&mut self, object: &Map<String, Value>) -> Result<(), BridgeError> {
        for key in ["summary", "content"] {
            for text in reasoning_texts(object.get(key)) {
                self.pending_reasoning = append_reasoning_text(&self.pending_reasoning, &text)
                    .unwrap_or_else(|| text.clone());
            }
        }
        if self.pending_reasoning.is_empty() {
            if object
                .get("encrypted_content")
                .and_then(Value::as_str)
                .is_some_and(|value| !value.is_empty())
            {
                return Err(BridgeError::new("opaque_reasoning"));
            }
            self.push_marker(CompatibilityMarker::EmptyReasoningOmitted);
        }
        Ok(())
    }

    fn push_tool_call(
        &mut self,
        object: &Map<String, Value>,
        kind: ToolOriginKind,
    ) -> Result<(), BridgeError> {
        let name = object
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| BridgeError::new("tool_call_shape"))?;
        let call_id = object
            .get("call_id")
            .or_else(|| object.get("id"))
            .and_then(Value::as_str)
            .ok_or_else(|| BridgeError::new("tool_call_shape"))?;
        let namespace = object.get("namespace").and_then(Value::as_str);
        let chat_name = self.resolve_history_tool_name(namespace, name, kind)?;
        let arguments = match kind {
            ToolOriginKind::Custom => {
                let input = object.get("input").and_then(Value::as_str).unwrap_or("");
                canonical_json(&json!({ "input": input }))
            }
            _ => object
                .get("arguments")
                .and_then(Value::as_str)
                .unwrap_or("{}")
                .to_owned(),
        };
        self.pending_tool_calls.push(json!({
            "id": call_id,
            "type": "function",
            "function": { "name": chat_name, "arguments": arguments },
        }));
        Ok(())
    }

    fn push_tool_search_call(&mut self, object: &Map<String, Value>) -> Result<(), BridgeError> {
        let call_id = object
            .get("call_id")
            .or_else(|| object.get("id"))
            .and_then(Value::as_str)
            .ok_or_else(|| BridgeError::new("tool_search_shape"))?;
        let arguments = match object.get("arguments") {
            Some(Value::Object(_)) => canonical_json(&object["arguments"]),
            Some(Value::String(text)) => canonical_json(&json!({ "query": text })),
            _ => return Err(BridgeError::new("tool_search_shape")),
        };
        self.pending_tool_calls.push(json!({
            "id": call_id,
            "type": "function",
            "function": { "name": TOOL_SEARCH_NAME, "arguments": arguments },
        }));
        Ok(())
    }

    fn push_tool_output(
        &mut self,
        object: &Map<String, Value>,
        kind: ToolOriginKind,
    ) -> Result<(), BridgeError> {
        let call_id = object
            .get("call_id")
            .or_else(|| object.get("id"))
            .and_then(Value::as_str)
            .ok_or_else(|| BridgeError::new("tool_output_shape"))?;
        let name = object
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !name.is_empty() {
            let namespace = object.get("namespace").and_then(Value::as_str);
            self.resolve_history_tool_name(namespace, name, kind)?;
        }
        self.flush_tool_calls();
        let content = tool_output_text(object.get("output"))?;
        self.messages.push(json!({
            "role": "tool",
            "tool_call_id": call_id,
            "content": content,
        }));
        Ok(())
    }

    fn push_tool_search_output(&mut self, object: &Map<String, Value>) -> Result<(), BridgeError> {
        self.flush_tool_calls();
        let call_id = object
            .get("call_id")
            .or_else(|| object.get("id"))
            .and_then(Value::as_str)
            .ok_or_else(|| BridgeError::new("tool_search_shape"))?;
        self.messages.push(json!({
            "role": "tool",
            "tool_call_id": call_id,
            "content": canonical_json(&Value::Object(object.clone())),
        }));
        Ok(())
    }

    fn push_additional_tools(&mut self) {
        self.push_marker(CompatibilityMarker::AdditionalToolsExtracted);
    }

    fn declare_carried_tools(
        &mut self,
        object: &Map<String, Value>,
        feature: &'static str,
    ) -> Result<(), BridgeError> {
        let tools = object
            .get("tools")
            .ok_or_else(|| BridgeError::new(feature))?;
        self.declare_tools(tools)
    }

    fn flush_tool_calls(&mut self) {
        if self.pending_tool_calls.is_empty() {
            return;
        }
        let merge_into_commentary = self.messages.last().is_some_and(|message| {
            message.get("role").and_then(Value::as_str) == Some("assistant")
                && message.get("tool_calls").is_none()
        });
        let index = if merge_into_commentary {
            self.messages.len() - 1
        } else {
            self.messages.push(json!({
                "role": "assistant",
                "content": Value::Null,
            }));
            self.messages.len() - 1
        };
        let existing = self
            .messages
            .get(index)
            .and_then(|message| message.get("reasoning_content"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        let reasoning = self.take_pending_reasoning(existing.as_deref());
        let calls = std::mem::take(&mut self.pending_tool_calls);
        if let Some(Value::Object(message)) = self.messages.get_mut(index) {
            message.insert("tool_calls".to_owned(), Value::Array(calls));
            if let Some(reasoning) = reasoning {
                message.insert("reasoning_content".to_owned(), Value::String(reasoning));
            }
        }
    }

    fn resolve_chat_name(
        &self,
        namespace: Option<&str>,
        name: &str,
        kind: ToolOriginKind,
    ) -> Result<String, BridgeError> {
        if let Some(namespace) = namespace {
            let flattened = flatten_tool_name(namespace, name);
            return match self.registry.chat_names.get(&flattened) {
                Some(origin) if origin.kind == kind => Ok(flattened),
                _ => Err(BridgeError::new("tool_name_unknown")),
            };
        }
        match self.registry.chat_names.get(name) {
            Some(origin) if origin.kind == kind => Ok(name.to_owned()),
            _ => self.unique_origin_match(name, kind),
        }
    }

    /// Resolves a history item's tool name.
    ///
    /// An ordinary function call whose declaration is absent keeps its plain
    /// name so the conversation stays routable and the upstream surface keeps
    /// reporting the mismatch. Wrapped tool families fail closed: their
    /// arguments are only reversible with a known origin.
    fn resolve_history_tool_name(
        &self,
        namespace: Option<&str>,
        name: &str,
        kind: ToolOriginKind,
    ) -> Result<String, BridgeError> {
        let declared = match namespace {
            Some(namespace) => {
                let flattened = flatten_tool_name(namespace, name);
                self.registry
                    .chat_names
                    .get(&flattened)
                    .is_some_and(|origin| origin.kind == kind)
                    .then_some(flattened)
            }
            None => self
                .registry
                .chat_names
                .get(name)
                .is_some_and(|origin| origin.kind == kind)
                .then(|| name.to_owned()),
        };
        if let Some(chat_name) = declared {
            return Ok(chat_name);
        }
        match self.unique_origin_match(name, kind) {
            Ok(chat_name) => Ok(chat_name),
            Err(error) if kind != ToolOriginKind::Function => Err(error),
            Err(_) => Ok(name.to_owned()),
        }
    }

    /// Codex history items do not always repeat the declaring namespace, so an
    /// unqualified name resolves when exactly one tool matches it.
    fn unique_origin_match(&self, name: &str, kind: ToolOriginKind) -> Result<String, BridgeError> {
        let mut matches = self
            .registry
            .chat_names
            .iter()
            .filter(|(_, origin)| origin.name == name && origin.kind == kind);
        match (matches.next(), matches.next()) {
            (Some((chat_name, _)), None) => Ok(chat_name.clone()),
            _ => Err(BridgeError::new("tool_name_unknown")),
        }
    }

    fn finish_tools(
        &mut self,
        chat: &mut Map<String, Value>,
        object: &Map<String, Value>,
    ) -> Result<(), BridgeError> {
        if self.registry.declarations.is_empty() {
            if optional(object, "tool_choice").is_some()
                || optional(object, "parallel_tool_calls").is_some()
            {
                self.push_marker(CompatibilityMarker::ToolControlsOmitted);
            }
            return Ok(());
        }
        chat.insert(
            "tools".to_owned(),
            Value::Array(self.registry.declarations.clone()),
        );
        if let Some(choice) = optional(object, "tool_choice") {
            chat.insert(
                "tool_choice".to_owned(),
                self.translate_tool_choice(choice)?,
            );
        }
        if let Some(parallel) = optional(object, "parallel_tool_calls") {
            chat.insert("parallel_tool_calls".to_owned(), parallel.clone());
        }
        Ok(())
    }

    fn translate_tool_choice(&self, choice: &Value) -> Result<Value, BridgeError> {
        if let Some(selector) = choice.as_str() {
            return match selector {
                "auto" | "none" | "required" => Ok(Value::String(selector.to_owned())),
                _ => Err(BridgeError::new("tool_choice")),
            };
        }
        let object = choice
            .as_object()
            .ok_or_else(|| BridgeError::new("tool_choice"))?;
        let kind = object
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| BridgeError::new("tool_choice"))?;
        let chat_name = match kind {
            "function" | "custom" => {
                let name = object
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| BridgeError::new("tool_choice"))?;
                self.resolve_chat_name(
                    object.get("namespace").and_then(Value::as_str),
                    name,
                    if kind == "custom" {
                        ToolOriginKind::Custom
                    } else {
                        ToolOriginKind::Function
                    },
                )?
            }
            "tool_search" => TOOL_SEARCH_NAME.to_owned(),
            _ => return Err(BridgeError::new("tool_choice")),
        };
        Ok(json!({
            "type": "function",
            "function": { "name": chat_name },
        }))
    }
}

#[derive(Default)]
struct ToolRegistry {
    chat_names: HashMap<String, ToolOrigin>,
    declarations: Vec<Value>,
}

impl ToolRegistry {
    fn declare_function(
        &mut self,
        name: &str,
        namespace: Option<&str>,
        description: Option<&Value>,
        parameters: &Value,
        strict: Option<&Value>,
        tool_search: bool,
    ) -> Result<String, BridgeError> {
        let chat_name = match namespace {
            Some(namespace) => flatten_tool_name(namespace, name),
            None => name.to_owned(),
        };
        let kind = if tool_search {
            ToolOriginKind::ToolSearch
        } else {
            ToolOriginKind::Function
        };
        self.insert(
            &chat_name,
            ToolOrigin {
                kind,
                name: name.to_owned(),
                namespace: namespace.map(str::to_owned),
            },
        )?;
        let mut function = Map::new();
        function.insert("name".to_owned(), Value::String(chat_name.clone()));
        if let Some(description) = description {
            function.insert("description".to_owned(), description.clone());
        }
        function.insert("parameters".to_owned(), parameters.clone());
        if let Some(strict) = strict {
            function.insert("strict".to_owned(), strict.clone());
        }
        self.declarations
            .push(json!({ "type": "function", "function": Value::Object(function) }));
        Ok(chat_name)
    }

    fn declare_custom(
        &mut self,
        name: &str,
        namespace: Option<&str>,
        declaration: &Value,
    ) -> Result<String, BridgeError> {
        let chat_name = match namespace {
            Some(namespace) => flatten_tool_name(namespace, name),
            None => name.to_owned(),
        };
        self.insert(
            &chat_name,
            ToolOrigin {
                kind: ToolOriginKind::Custom,
                name: name.to_owned(),
                namespace: namespace.map(str::to_owned),
            },
        )?;
        self.declarations.push(json!({
            "type": "function",
            "function": {
                "name": chat_name.clone(),
                "description": custom_tool_description(declaration),
                "parameters": {
                    "type": "object",
                    "properties": {
                        "input": {
                            "type": "string",
                            "description": CUSTOM_TOOL_INPUT_DESCRIPTION,
                        }
                    },
                    "required": ["input"],
                },
            },
        }));
        Ok(chat_name)
    }

    fn declare_tool_search(&mut self) -> Result<String, BridgeError> {
        if self.chat_names.contains_key(TOOL_SEARCH_NAME) {
            return Err(BridgeError::new("tool_name_collision"));
        }
        self.declare_function(
            TOOL_SEARCH_NAME,
            None,
            Some(&Value::String(TOOL_SEARCH_DESCRIPTION.to_owned())),
            &json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Search query for tools or connectors to load.",
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of tool groups to return.",
                    },
                },
                "required": ["query"],
            }),
            None,
            true,
        )
    }

    fn insert(&mut self, chat_name: &str, origin: ToolOrigin) -> Result<(), BridgeError> {
        if chat_name.is_empty() || self.chat_names.contains_key(chat_name) {
            return Err(BridgeError::new("tool_name_collision"));
        }
        self.chat_names.insert(chat_name.to_owned(), origin);
        Ok(())
    }
}

/// Reads an optional tool field, treating an explicit `null` as absent.
fn optional_value<'a>(object: &'a Value, key: &str) -> Option<&'a Value> {
    object.get(key).filter(|value| !value.is_null())
}

/// Keeps a function schema acceptable to strict Chat upstreams: `parameters`
/// must be an object whose `type` is `"object"`.
fn normalize_function_parameters(parameters: Option<&Value>) -> Value {
    let mut parameters = match parameters {
        Some(Value::Object(object)) => Value::Object(object.clone()),
        _ => json!({ "type": "object", "properties": {} }),
    };
    if let Some(object) = parameters.as_object_mut()
        && object.get("type").and_then(Value::as_str) != Some("object")
    {
        object.insert("type".to_owned(), Value::String("object".to_owned()));
    }
    parameters
}

/// Embeds the original custom-tool declaration so the call can be reversed.
fn custom_tool_description(declaration: &Value) -> String {
    format!(
        "{CUSTOM_TOOL_DEFINITION_HEADING}\n```json\n{}\n```",
        canonical_json(declaration)
    )
}

/// Extracts readable text from a Responses reasoning field.
fn reasoning_texts(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::String(text)) => vec![text.clone()],
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|part| {
                part.get("text")
                    .or_else(|| part.get("content"))
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn unsupported_item_feature(kind: &str) -> &'static str {
    match kind {
        "local_shell_call" => "local_shell_call",
        "image_generation_call" => "image_generation_call",
        "web_search_call" => "web_search_call",
        "compaction" | "context_compaction" | "compaction_trigger" => "compaction",
        "agent_message" => "agent_message",
        "configuration_update" => "configuration_update",
        _ => "unknown_item",
    }
}

fn is_default_web_search(tool: &Value) -> bool {
    let Some(object) = tool.as_object() else {
        return false;
    };
    object.len() == 2
        && object.get("type").and_then(Value::as_str) == Some("web_search")
        && object
            .get("external_web_access")
            .is_some_and(Value::is_boolean)
}

fn tool_name(tool: &Value) -> Result<String, BridgeError> {
    tool.get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| BridgeError::new("tool_name"))
}

fn text_content(content: &Value) -> Result<String, BridgeError> {
    if let Some(text) = content.as_str() {
        return Ok(text.to_owned());
    }
    let parts = content
        .as_array()
        .ok_or_else(|| BridgeError::new("message_content"))?;
    let mut text = String::new();
    for part in parts {
        match part.get("type").and_then(Value::as_str) {
            Some("input_text" | "output_text" | "text") => {
                let value = part
                    .get("text")
                    .and_then(Value::as_str)
                    .ok_or_else(|| BridgeError::new("message_content"))?;
                if !text.is_empty() {
                    text.push_str("\n\n");
                }
                text.push_str(value);
            }
            _ => return Err(BridgeError::new("message_content")),
        }
    }
    Ok(text)
}

fn chat_content(content: &Value) -> Result<Value, BridgeError> {
    if content.is_string() {
        return Ok(content.clone());
    }
    let parts = content
        .as_array()
        .ok_or_else(|| BridgeError::new("message_content"))?;
    let mut translated = Vec::with_capacity(parts.len());
    let mut has_image = false;
    for part in parts {
        match part.get("type").and_then(Value::as_str) {
            Some("input_text" | "output_text" | "text") => {
                translated.push(json!({
                    "type": "text",
                    "text": part.get("text").and_then(Value::as_str).unwrap_or_default(),
                }));
            }
            Some("input_image") => {
                has_image = true;
                let url = part
                    .get("image_url")
                    .and_then(Value::as_str)
                    .ok_or_else(|| BridgeError::new("message_content"))?;
                let mut image = Map::new();
                image.insert("url".to_owned(), Value::String(url.to_owned()));
                if let Some(detail) = part.get("detail") {
                    image.insert("detail".to_owned(), detail.clone());
                }
                translated.push(json!({
                    "type": "image_url",
                    "image_url": Value::Object(image),
                }));
            }
            Some("input_audio") => return Err(BridgeError::new("input_audio")),
            _ => return Err(BridgeError::new("message_content")),
        }
    }
    if has_image {
        return Ok(Value::Array(translated));
    }
    Ok(Value::String(
        translated
            .iter()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n\n"),
    ))
}

fn tool_output_text(output: Option<&Value>) -> Result<String, BridgeError> {
    match output {
        Some(Value::String(text)) => Ok(text.clone()),
        Some(Value::Array(parts)) => Ok(parts
            .iter()
            .map(|part| match part {
                Value::String(text) => Ok(text.clone()),
                Value::Object(object) => object
                    .get("text")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .ok_or_else(|| BridgeError::new("tool_output")),
                _ => Err(BridgeError::new("tool_output")),
            })
            .collect::<Result<Vec<_>, _>>()?
            .join("\n\n")),
        Some(other) => Ok(canonical_json(other)),
        None => Err(BridgeError::new("tool_output")),
    }
}

fn append_reasoning_text(existing: &str, addition: &str) -> Option<String> {
    if addition.is_empty() || existing.contains(addition) {
        return None;
    }
    if existing.is_empty() {
        return Some(addition.to_owned());
    }
    Some(format!("{existing}\n\n{addition}"))
}

/// First `take` bytes of the SHA-256 of `bytes`, hex encoded.
fn sha256_hex_prefix(bytes: &[u8], take: usize) -> String {
    Sha256::digest(bytes)
        .iter()
        .take(take)
        .fold(String::new(), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}

fn flatten_tool_name(namespace: &str, name: &str) -> String {
    let full = format!("{namespace}__{name}");
    if full.len() <= MAX_CHAT_TOOL_NAME_BYTES {
        return full;
    }
    let suffix = sha256_hex_prefix(full.as_bytes(), 4);
    let budget = MAX_CHAT_TOOL_NAME_BYTES - suffix.len() - 2;
    let mut prefix = String::new();
    for character in full.chars() {
        if prefix.len() + character.len_utf8() > budget {
            break;
        }
        prefix.push(character);
    }
    format!("{prefix}__{suffix}")
}

#[cfg(test)]
mod tests {
    use super::{
        BridgeError, ChatRequestTranslation, ToolOrigin, ToolOriginKind, UNSUPPORTED_REQUEST_CODE,
        translate_request,
    };
    use serde_json::{Value, json};

    fn translate(request: &Value) -> Result<ChatRequestTranslation, BridgeError> {
        translate_request(&serde_json::to_vec(request).expect("request serializes"))
    }

    fn chat_body(request: &Value) -> Value {
        let translated = translate(request).expect("request translates");
        serde_json::from_slice(&translated.body).expect("translated body is JSON")
    }

    fn failure(request: &Value) -> BridgeError {
        translate(request).expect_err("translation fails")
    }

    fn markers(request: &Value) -> Vec<&'static str> {
        translate(request)
            .expect("request translates")
            .compatibility
            .iter()
            .map(|marker| marker.as_str())
            .collect()
    }

    fn request(input: &Value) -> Value {
        json!({ "model": "gpt-5.1-codex", "input": input })
    }

    fn user_message(text: &str) -> Value {
        json!({
            "type": "message",
            "role": "user",
            "content": [{ "type": "input_text", "text": text }],
        })
    }

    fn exec_command_tool() -> Value {
        json!({
            "type": "function",
            "name": "exec_command",
            "description": "Runs a command",
            "parameters": { "type": "object", "properties": { "cmd": { "type": "string" } } },
        })
    }

    #[test]
    fn minimal_request_sets_stream_and_translates_messages() {
        let translated = translate(&json!({
            "model": "gpt-5.1-codex",
            "instructions": "Be terse.",
            "input": [user_message("hello")],
        }))
        .expect("request translates");
        assert!(translated.compatibility.is_empty());
        assert!(translated.tool_names.is_empty());
        let body: Value = serde_json::from_slice(&translated.body).expect("valid json");
        assert_eq!(body["model"], json!("gpt-5.1-codex"));
        assert_eq!(body["stream"], json!(true));
        assert_eq!(body["stream_options"], json!({ "include_usage": true }));
        assert_eq!(
            body["messages"],
            json!([
                { "role": "system", "content": "Be terse." },
                { "role": "user", "content": "hello" },
            ])
        );
        assert!(body.get("tools").is_none());
        assert!(body.get("tool_choice").is_none());
    }

    #[test]
    fn system_and_developer_messages_collapse_into_one_leading_message() {
        let body = chat_body(&json!({
            "model": "m",
            "instructions": "top",
            "input": [
                { "type": "message", "role": "developer", "content": [
                    { "type": "input_text", "text": "dev" }
                ]},
                user_message("hi"),
                { "type": "message", "role": "system", "content": "late" },
            ],
        }));
        assert_eq!(
            body["messages"],
            json!([
                { "role": "system", "content": "top\n\ndev\n\nlate" },
                { "role": "user", "content": "hi" },
            ])
        );
    }

    #[test]
    fn image_content_becomes_chat_parts() {
        let body = chat_body(&json!({
            "model": "m",
            "input": [{ "type": "message", "role": "user", "content": [
                { "type": "input_text", "text": "look" },
                { "type": "input_image", "image_url": "data:image/png;base64,AAAA", "detail": "high" },
            ]}],
        }));
        assert_eq!(
            body["messages"][0]["content"],
            json!([
                { "type": "text", "text": "look" },
                { "type": "image_url", "image_url": {
                    "url": "data:image/png;base64,AAAA",
                    "detail": "high",
                }},
            ])
        );
    }

    #[test]
    fn function_calls_merge_into_the_assistant_turn_and_outputs_follow() {
        let body = chat_body(&json!({
            "model": "m",
            "tools": [exec_command_tool()],
            "tool_choice": "auto",
            "parallel_tool_calls": false,
            "input": [
                { "type": "message", "role": "assistant", "content": [
                    { "type": "output_text", "text": "running" }
                ]},
                { "type": "function_call", "call_id": "c1", "name": "exec_command",
                  "arguments": "{\"cmd\":\"ls\"}" },
                { "type": "function_call_output", "call_id": "c1", "output": "ok" },
                user_message("next"),
            ],
        }));
        assert_eq!(
            body["tools"],
            json!([{
                "type": "function",
                "function": {
                    "name": "exec_command",
                    "description": "Runs a command",
                    "parameters": { "type": "object", "properties": { "cmd": { "type": "string" } } },
                },
            }])
        );
        assert_eq!(body["tool_choice"], json!("auto"));
        assert_eq!(body["parallel_tool_calls"], json!(false));
        assert_eq!(
            body["messages"],
            json!([
                { "role": "assistant", "content": "running", "tool_calls": [{
                    "id": "c1",
                    "type": "function",
                    "function": { "name": "exec_command", "arguments": "{\"cmd\":\"ls\"}" },
                }]},
                { "role": "tool", "tool_call_id": "c1", "content": "ok" },
                { "role": "user", "content": "next" },
            ])
        );
    }

    #[test]
    fn custom_tools_embed_their_definition_and_reverse_their_calls() {
        let declaration = json!({
            "type": "custom",
            "name": "apply_patch",
            "description": "Patch files",
        });
        let body = chat_body(&json!({
            "model": "m",
            "tools": [declaration.clone()],
            "input": [
                { "type": "custom_tool_call", "call_id": "c2", "name": "apply_patch",
                  "input": "*** Begin Patch" },
                { "type": "custom_tool_call_output", "call_id": "c2", "output": "done" },
            ],
        }));
        let function = &body["tools"][0]["function"];
        assert_eq!(function["name"], json!("apply_patch"));
        assert!(function.get("strict").is_none());
        assert_eq!(
            function["parameters"],
            json!({
                "type": "object",
                "properties": { "input": {
                    "type": "string",
                    "description": "Raw string input for the original custom tool. Preserve formatting exactly and follow the original tool definition embedded in the description.",
                }},
                "required": ["input"],
            })
        );
        let description = function["description"].as_str().expect("description");
        let embedded = description
            .strip_prefix("Original tool definition:\n```json\n")
            .and_then(|rest| rest.strip_suffix("\n```"))
            .expect("original definition embedded");
        assert_eq!(
            serde_json::from_str::<Value>(embedded).expect("canonical json"),
            declaration
        );
        assert_eq!(
            body["messages"],
            json!([
                { "role": "assistant", "content": null, "tool_calls": [{
                    "id": "c2",
                    "type": "function",
                    "function": { "name": "apply_patch", "arguments": "{\"input\":\"*** Begin Patch\"}" },
                }]},
                { "role": "tool", "tool_call_id": "c2", "content": "done" },
            ])
        );
        assert_eq!(
            markers(&json!({ "model": "m", "tools": [declaration], "input": [] })),
            vec!["custom_tool_description_embedded"]
        );
    }

    #[test]
    fn namespace_tools_flatten_and_keep_their_origin() {
        let request = json!({
            "model": "m",
            "tools": [{
                "type": "namespace",
                "name": "multi_agent_v1",
                "description": "Agents",
                "tools": [{
                    "type": "function",
                    "name": "close_agent",
                    "description": "Close an agent",
                    "parameters": { "type": "object", "properties": {} },
                }],
            }],
            "input": [{ "type": "function_call", "call_id": "c3", "namespace": "multi_agent_v1",
                        "name": "close_agent", "arguments": "{}" }],
        });
        let translated = translate(&request).expect("translates");
        assert_eq!(
            translated.tool_names.get("multi_agent_v1__close_agent"),
            Some(&ToolOrigin {
                kind: ToolOriginKind::Function,
                name: "close_agent".to_owned(),
                namespace: Some("multi_agent_v1".to_owned()),
            })
        );
        let body: Value = serde_json::from_slice(&translated.body).expect("valid json");
        assert_eq!(
            body["messages"][0]["tool_calls"][0]["function"]["name"],
            json!("multi_agent_v1__close_agent")
        );
    }

    #[test]
    fn long_namespace_names_are_capped_deterministically() {
        let request = json!({
            "model": "m",
            "tools": [{
                "type": "namespace",
                "name": "n".repeat(80),
                "description": "Wide",
                "tools": [{
                    "type": "function",
                    "name": "t".repeat(20),
                    "description": "Deep",
                    "parameters": { "type": "object" },
                }],
            }],
            "input": [],
        });
        let first = translate(&request).expect("translates");
        let second = translate(&request).expect("translates");
        assert_eq!(first.body, second.body);
        assert_eq!(first.tool_names.len(), 1);
        let name = first.tool_names.keys().next().expect("one tool");
        assert_eq!(name.len(), 64);
        assert_eq!(&name[54..56], "__");
        assert!(
            name[56..]
                .chars()
                .all(|character| character.is_ascii_hexdigit())
        );
    }

    #[test]
    fn colliding_tool_names_fail_closed() {
        let request = json!({
            "model": "m",
            "tools": [
                { "type": "function", "name": "exec__command", "description": "Already flat",
                  "parameters": { "type": "object" } },
                { "type": "namespace", "name": "exec", "description": "x", "tools": [{
                    "type": "function",
                    "name": "command",
                    "description": "Runs a command",
                    "parameters": { "type": "object" },
                }]},
            ],
            "input": [],
        });
        let error = failure(&request);
        assert_eq!(error.code, UNSUPPORTED_REQUEST_CODE);
        assert_eq!(error.feature, "tool_name_collision");
    }

    #[test]
    fn tool_search_declares_the_evidenced_synthetic_schema_and_round_trips() {
        let request = json!({
            "model": "m",
            "tools": [{ "type": "tool_search" }, exec_command_tool()],
            "input": [
                { "type": "tool_search_call", "call_id": "c4", "execution": "client",
                  "arguments": { "query": "linear" } },
                { "type": "tool_search_output", "call_id": "c4", "status": "completed",
                  "execution": "client", "tools": [{
                      "type": "function",
                      "name": "linear_create_issue",
                      "description": "Create an issue",
                      "parameters": { "type": "object", "properties": {} },
                  }]},
            ],
        });
        let translated = translate(&request).expect("translates");
        let body: Value = serde_json::from_slice(&translated.body).expect("valid json");
        assert_eq!(
            body["tools"][0],
            json!({
                "type": "function",
                "function": {
                    "name": "tool_search",
                    "description": "Search and load Codex tools, plugins, connectors, and MCP namespaces for the current task.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "query": {
                                "type": "string",
                                "description": "Search query for tools or connectors to load.",
                            },
                            "limit": {
                                "type": "integer",
                                "description": "Maximum number of tool groups to return.",
                            },
                        },
                        "required": ["query"],
                    },
                },
            })
        );
        assert_eq!(body["tools"][1]["function"]["name"], json!("exec_command"));
        assert_eq!(
            body["tools"][2]["function"]["name"],
            json!("linear_create_issue")
        );
        // The synthetic declaration is reproduced verbatim from the reference
        // bridge, so its exact bytes are pinned (see `THIRD_PARTY_NOTICES.md`).
        assert_eq!(
            super::sha256_hex_prefix(body["tools"][0].to_string().as_bytes(), 32),
            "e684675e547c7e60317a9a43834d2133d3655704902296e0881619c13f48b06c"
        );
        assert_eq!(
            body["messages"][0],
            json!({
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": "c4",
                    "type": "function",
                    "function": { "name": "tool_search", "arguments": "{\"query\":\"linear\"}" },
                }],
            })
        );
        let content = body["messages"][1]["content"]
            .as_str()
            .expect("tool content is a string");
        let restored: Value = serde_json::from_str(content).expect("canonical json");
        assert_eq!(restored["call_id"], json!("c4"));
        assert_eq!(restored["tools"][0]["name"], json!("linear_create_issue"));
        assert_eq!(
            translated.tool_names.get("linear_create_issue"),
            Some(&ToolOrigin {
                kind: ToolOriginKind::Function,
                name: "linear_create_issue".to_owned(),
                namespace: None,
            })
        );
    }

    #[test]
    fn additional_tools_are_extracted_and_the_carrier_is_dropped() {
        let request = json!({
            "model": "m",
            "input": [
                { "type": "additional_tools", "tools": [{
                    "type": "function",
                    "name": "extra",
                    "description": "Extra tool",
                    "parameters": { "type": "object" },
                }]},
                user_message("hi"),
            ],
        });
        let translated = translate(&request).expect("translates");
        let body: Value = serde_json::from_slice(&translated.body).expect("valid json");
        assert_eq!(body["tools"][0]["function"]["name"], json!("extra"));
        assert_eq!(
            body["messages"],
            json!([{ "role": "user", "content": "hi" }])
        );
        assert_eq!(
            translated.compatibility,
            vec![super::CompatibilityMarker::AdditionalToolsExtracted]
        );
        assert_eq!(
            failure(&request_with_input(
                &json!([{ "type": "additional_tools" }])
            ))
            .feature,
            "additional_tools"
        );

        // A call may precede the carrier that declares its tool.
        let ordered = json!({
            "model": "m",
            "input": [
                { "type": "function_call", "call_id": "c5", "name": "extra", "arguments": "{}" },
                { "type": "function_call_output", "call_id": "c5", "output": "ok" },
                { "type": "additional_tools", "tools": [{
                    "type": "function",
                    "name": "extra",
                    "description": "Extra tool",
                    "parameters": { "type": "object" },
                }]},
            ],
        });
        let body = chat_body(&ordered);
        assert_eq!(
            body["messages"][0]["tool_calls"][0]["function"]["name"],
            json!("extra")
        );
        assert_eq!(body["messages"].as_array().map(Vec::len), Some(2));
    }

    #[test]
    fn function_schemas_are_normalized_and_absent_descriptions_are_omitted() {
        let body = chat_body(&json!({
            "model": "m",
            "tools": [
                { "type": "function", "name": "no_schema" },
                { "type": "function", "name": "array_schema", "description": null,
                  "parameters": { "type": "array" } },
                { "type": "function", "name": " full ", "description": "Trimmed",
                  "parameters": { "type": "object", "properties": {} } },
            ],
            "input": [
                { "type": "function_call", "call_id": "c1", "name": "gone_tool",
                  "arguments": "{}" },
            ],
        }));
        assert_eq!(
            body["tools"][0]["function"],
            json!({ "name": "no_schema", "parameters": { "type": "object", "properties": {} } })
        );
        assert_eq!(
            body["tools"][1]["function"]["parameters"],
            json!({ "type": "object" })
        );
        assert_eq!(body["tools"][2]["function"]["name"], json!("full"));
        assert_eq!(
            body["messages"][0]["tool_calls"][0]["function"]["name"],
            json!("gone_tool")
        );
    }

    fn request_with_input(input: &Value) -> Value {
        json!({ "model": "m", "input": input })
    }

    #[test]
    fn web_search_default_form_is_omitted_and_richer_forms_fail_closed() {
        let omitted = json!({ "type": "web_search", "external_web_access": false });
        let body = chat_body(
            &json!({ "model": "m", "tools": [exec_command_tool(), omitted], "input": [] }),
        );
        assert_eq!(body["tools"].as_array().map(Vec::len), Some(1));
        assert_eq!(
            markers(&json!({ "model": "m", "tools": [omitted], "input": [] })),
            vec!["web_search_declaration_omitted"]
        );
        let rich = json!({
            "type": "web_search",
            "external_web_access": true,
            "search_context_size": "high",
        });
        let error = failure(&json!({ "model": "m", "tools": [rich], "input": [] }));
        assert_eq!(error.code, UNSUPPORTED_REQUEST_CODE);
        assert_eq!(error.feature, "web_search_options");
    }

    #[test]
    fn reasoning_effort_maps_and_unsupported_values_fail_closed() {
        let body = chat_body(&json!({
            "model": "m",
            "reasoning": { "effort": "high", "summary": "auto" },
            "input": [],
        }));
        assert_eq!(body["reasoning_effort"], json!("high"));
        assert_eq!(
            markers(&json!({
                "model": "m",
                "reasoning": { "effort": "high", "summary": "auto" },
                "input": [],
            })),
            vec!["reasoning_effort_mapped"]
        );
        assert_eq!(
            markers(&json!({
                "model": "m",
                "reasoning": { "effort": "high", "summary": "concise" },
                "input": [],
            })),
            vec!["reasoning_summary_omitted", "reasoning_effort_mapped"]
        );
        for effort in ["low", "medium", "high", "xhigh", "max"] {
            let body =
                chat_body(&json!({ "model": "m", "reasoning": { "effort": effort }, "input": [] }));
            assert_eq!(body["reasoning_effort"], json!(effort));
        }
        let error =
            failure(&json!({ "model": "m", "reasoning": { "effort": "minimal" }, "input": [] }));
        assert_eq!(error.feature, "reasoning_effort");
        let error = failure(&json!({ "model": "m", "reasoning": { "effort": 3 }, "input": [] }));
        assert_eq!(error.feature, "reasoning_shape");
        let error = failure(&json!({
            "model": "m",
            "reasoning": { "effort": "high", "budget_tokens": 10 },
            "input": [],
        }));
        assert_eq!(error.feature, "reasoning_shape");
    }

    #[test]
    fn readable_reasoning_replays_on_assistant_turns() {
        let request = json!({
            "model": "m",
            "tools": [exec_command_tool()],
            "input": [
                { "type": "reasoning",
                  "summary": [{ "type": "summary_text", "text": "thinking" }],
                  "encrypted_content": "opaque" },
                { "type": "function_call", "call_id": "c1", "name": "exec_command", "arguments": "{}" },
            ],
        });
        let translated = translate(&request).expect("translates");
        let body: Value = serde_json::from_slice(&translated.body).expect("valid json");
        assert_eq!(body["messages"][0]["reasoning_content"], json!("thinking"));
        assert_eq!(body["messages"][0]["content"], Value::Null);
        assert_eq!(
            translated.compatibility,
            vec![super::CompatibilityMarker::ReadableReasoningReplay]
        );

        let boundary = json!({
            "model": "m",
            "tools": [exec_command_tool()],
            "input": [
                { "type": "reasoning", "summary": [{ "type": "summary_text", "text": "r1" }] },
                { "type": "function_call", "call_id": "c1", "name": "exec_command", "arguments": "{}" },
                { "type": "function_call_output", "call_id": "c1", "output": "done" },
                { "type": "reasoning", "summary": [{ "type": "summary_text", "text": "r2" }] },
                user_message("next"),
            ],
        });
        let body = chat_body(&boundary);
        assert_eq!(body["messages"][0]["reasoning_content"], json!("r1\n\nr2"));

        let opaque = json!({
            "model": "m",
            "input": [{ "type": "reasoning", "summary": [], "encrypted_content": "opaque" }],
        });
        assert_eq!(failure(&opaque).feature, "opaque_reasoning");

        let empty = json!({ "model": "m", "input": [{ "type": "reasoning", "summary": [] }] });
        assert_eq!(chat_body(&empty)["messages"], json!([]));
        assert_eq!(markers(&empty), vec!["empty_reasoning_omitted"]);
    }

    #[test]
    fn text_format_maps_to_response_format() {
        let body = chat_body(&json!({
            "model": "m",
            "text": {
                "verbosity": "low",
                "format": {
                    "type": "json_schema",
                    "name": "answer",
                    "strict": true,
                    "schema": { "type": "object", "properties": { "a": { "type": "string" } } },
                },
            },
            "input": [],
        }));
        assert_eq!(
            body["response_format"],
            json!({
                "type": "json_schema",
                "json_schema": {
                    "name": "answer",
                    "strict": true,
                    "schema": { "type": "object", "properties": { "a": { "type": "string" } } },
                },
            })
        );
        assert_eq!(
            markers(&json!({
                "model": "m",
                "text": { "verbosity": "low", "format": { "type": "text" } },
                "input": [],
            })),
            vec!["text_options_omitted"]
        );
        let json_object = chat_body(&json!({
            "model": "m",
            "text": { "format": { "type": "json_object" } },
            "input": [],
        }));
        assert_eq!(
            json_object["response_format"],
            json!({ "type": "json_object" })
        );
        let error = failure(&json!({
            "model": "m",
            "text": { "format": { "type": "json_schema", "schema": {} } },
            "input": [],
        }));
        assert_eq!(error.feature, "text_format");
    }

    #[test]
    fn tool_controls_follow_the_translated_tools() {
        let without_tools = json!({
            "model": "m",
            "tool_choice": "auto",
            "parallel_tool_calls": true,
            "input": [],
        });
        let body = chat_body(&without_tools);
        assert!(body.get("tool_choice").is_none());
        assert_eq!(markers(&without_tools), vec!["tool_controls_omitted"]);

        let named = json!({
            "model": "m",
            "tools": [exec_command_tool()],
            "tool_choice": { "type": "function", "name": "exec_command" },
            "input": [],
        });
        assert_eq!(
            chat_body(&named)["tool_choice"],
            json!({ "type": "function", "function": { "name": "exec_command" } })
        );

        let hosted = json!({
            "model": "m",
            "tools": [{ "type": "tool_search" }],
            "tool_choice": { "type": "tool_search" },
            "input": [],
        });
        assert_eq!(
            chat_body(&hosted)["tool_choice"],
            json!({ "type": "function", "function": { "name": "tool_search" } })
        );

        let web = json!({
            "model": "m",
            "tools": [exec_command_tool()],
            "tool_choice": { "type": "web_search" },
            "input": [],
        });
        assert_eq!(failure(&web).feature, "tool_choice");

        let unknown = json!({
            "model": "m",
            "tools": [exec_command_tool()],
            "tool_choice": { "type": "function", "name": "missing" },
            "input": [],
        });
        assert_eq!(failure(&unknown).feature, "tool_name_unknown");
    }

    #[test]
    fn unsupported_items_fail_closed_with_bounded_features() {
        for (item, feature) in [
            (json!({ "type": "compaction_trigger" }), "compaction"),
            (json!({ "type": "context_compaction" }), "compaction"),
            (
                json!({ "type": "web_search_call", "call_id": "w1" }),
                "web_search_call",
            ),
            (
                json!({ "type": "image_generation_call", "id": "i1" }),
                "image_generation_call",
            ),
            (
                json!({ "type": "local_shell_call", "call_id": "l1" }),
                "local_shell_call",
            ),
            (
                json!({ "type": "agent_message", "content": "hi" }),
                "agent_message",
            ),
            (
                json!({ "type": "configuration_update" }),
                "configuration_update",
            ),
            (json!({ "type": "something_new" }), "unknown_item"),
        ] {
            let error = failure(&request(&json!([item.clone()])));
            assert_eq!(error.code, UNSUPPORTED_REQUEST_CODE);
            assert_eq!(error.feature, feature, "item {item}");
        }
    }

    #[test]
    fn unknown_top_level_fields_fail_closed() {
        let error = failure(&json!({
            "model": "m",
            "input": [],
            "magically_new_option": true,
        }));
        assert_eq!(error.code, UNSUPPORTED_REQUEST_CODE);
        assert_eq!(error.feature, "unknown_field");
    }

    #[test]
    fn unrepresentable_top_level_fields_are_omitted_with_markers() {
        let request = json!({
            "model": "m",
            "store": true,
            "include": ["reasoning.encrypted_content"],
            "service_tier": "priority",
            "prompt_cache_key": "cache-key",
            "client_metadata": { "session": "abc" },
            "max_output_tokens": 2048,
            "input": [],
        });
        let translated = translate(&request).expect("translates");
        let body: Value = serde_json::from_slice(&translated.body).expect("valid json");
        assert_eq!(body["prompt_cache_key"], json!("cache-key"));
        assert_eq!(body["max_completion_tokens"], json!(2048));
        for absent in [
            "store",
            "include",
            "service_tier",
            "client_metadata",
            "max_output_tokens",
        ] {
            assert!(body.get(absent).is_none(), "{absent} must not reach chat");
        }
        assert_eq!(
            translated.compatibility,
            vec![
                super::CompatibilityMarker::IncludeOmitted,
                super::CompatibilityMarker::StoreOmitted,
                super::CompatibilityMarker::ServiceTierOmitted,
                super::CompatibilityMarker::ClientMetadataOmitted,
            ]
        );

        let stored_false = json!({ "model": "m", "store": false, "input": [] });
        assert!(markers(&stored_false).is_empty());
        let nulls = json!({
            "model": "m",
            "store": null,
            "include": null,
            "service_tier": null,
            "prompt_cache_key": null,
            "client_metadata": null,
            "max_output_tokens": null,
            "reasoning": null,
            "text": null,
            "tools": null,
            "tool_choice": null,
            "parallel_tool_calls": null,
            "input": [],
        });
        assert!(markers(&nulls).is_empty());
    }

    #[test]
    fn malformed_requests_fail_closed() {
        assert_eq!(
            translate_request(b"not json").expect_err("invalid").feature,
            "invalid_json"
        );
        for (request, feature) in [
            (json!({ "input": [] }), "model"),
            (json!({ "model": "m" }), "input"),
            (json!({ "model": "m", "input": "x" }), "input"),
            (
                json!({ "model": "m", "tools": {}, "input": [] }),
                "tools_shape",
            ),
            (
                json!({ "model": "m", "tools": [{ "type": "function" }], "input": [] }),
                "tool_name",
            ),
            (
                json!({ "model": "m", "tools": [{ "type": "computer_use" }], "input": [] }),
                "tool_type",
            ),
            (
                json!({ "model": "m", "tools": [{ "type": "namespace", "name": "ns", "tools": [
                    { "type": "tool_search" }
                ]}], "input": [] }),
                "namespace_tool",
            ),
            (
                json!({ "model": "m", "tools": [{ "type": "namespace", "name": "ns" }], "input": [] }),
                "namespace_shape",
            ),
            (
                json!({ "model": "m", "input": [{ "type": "message", "role": "tool", "content": "x" }] }),
                "message_role",
            ),
            (
                json!({ "model": "m", "input": [{ "type": "message", "role": "user", "content": [
                    { "type": "input_file", "file_url": "file://x" }
                ]}] }),
                "message_content",
            ),
            (
                json!({ "model": "m", "input": [{ "type": "message", "role": "user", "content": [
                    { "type": "input_audio", "audio_url": "x" }
                ]}] }),
                "input_audio",
            ),
            (
                json!({ "model": "m", "input": [{ "role": "user" }] }),
                "item_shape",
            ),
            (
                json!({ "model": "m", "tools": [{ "type": "custom", "name": "patch" }], "input": [
                    { "type": "custom_tool_call", "call_id": "c1", "name": "missing", "input": "x" }
                ]}),
                "tool_name_unknown",
            ),
            (
                json!({ "model": "m", "tools": [exec_command_tool()], "input": [
                    { "type": "function_call", "name": "exec_command", "arguments": "{}" }
                ]}),
                "tool_call_shape",
            ),
        ] {
            let error = failure(&request);
            assert_eq!(error.code, UNSUPPORTED_REQUEST_CODE);
            assert_eq!(error.feature, feature, "request {request}");
        }
    }

    #[test]
    fn translation_is_deterministic() {
        let request = json!({
            "model": "m",
            "instructions": "top",
            "tools": [
                { "type": "tool_search" },
                { "type": "function", "name": "z_tool", "description": "Z",
                  "parameters": { "type": "object", "properties": {} } },
                { "type": "namespace", "name": "ns", "description": "N", "tools": [{
                    "type": "custom", "name": "patch", "description": "Patch",
                }]},
            ],
            "tool_choice": "auto",
            "reasoning": { "effort": "medium", "summary": "auto" },
            "text": { "format": { "type": "text" } },
            "input": [
                { "type": "reasoning", "summary": [{ "type": "summary_text", "text": "r" }] },
                { "type": "custom_tool_call", "name": "patch",
                  "call_id": "c9", "input": "patch-body" },
                { "type": "custom_tool_call_output", "call_id": "c9", "output": "ok" },
                user_message("go"),
            ],
        });
        let first = translate(&request).expect("translates");
        let second = translate(&request).expect("translates");
        assert_eq!(first.body, second.body);
        assert_eq!(first.compatibility, second.compatibility);
        assert_eq!(first.tool_names, second.tool_names);
    }
}
