use std::{
    collections::BTreeMap,
    str::FromStr,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use axum::http::{HeaderMap, HeaderName, HeaderValue, Method};
use rquickjs::{Context, Runtime};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use time::{Date, Month, OffsetDateTime, format_description::well_known::Rfc3339};
use ts_rs::TS;
use zeroize::Zeroizing;

use crate::{
    domain::{ApiKey, BaseUrl, MAX_BALANCE_SCRIPT_BYTES},
    proxy::upstream::{DecodeError, decode_supported, response_encodings},
};

mod scheduler;

pub use scheduler::{
    BalanceBatchPhase, BalanceClock, BalanceCoordinator, BalanceDisplaySnapshot,
    BalanceDisplayStatus, BalanceQueryEngine, BalanceQueryResult, BalanceRefreshBatchState,
    BalanceRouteConfig, BalanceRouteSource, BalanceStateChangeSink, BalanceTrigger,
    SystemBalanceClock,
};

const MAX_SUBSTITUTED_SOURCE_BYTES: usize = 1024 * 1024;
const MAX_SCRIPT_PHASE: Duration = Duration::from_millis(250);
const SCRIPT_MEMORY_LIMIT: usize = 16 * 1024 * 1024;
const SCRIPT_STACK_LIMIT: usize = 512 * 1024;
const MAX_BALANCE_URL_BYTES: usize = 8 * 1024;
const MAX_BALANCE_HEADERS: usize = 64;
const MAX_BALANCE_HEADER_BYTES: usize = 64 * 1024;
const MAX_BALANCE_BODY_BYTES: usize = 1024 * 1024;
const MAX_BALANCE_RESPONSE_BYTES: usize = 1024 * 1024;
const MAX_UNIT_CHARS: usize = 32;
const MAX_PLAN_NAME_CHARS: usize = 128;
const MAX_INVALID_MESSAGE_CHARS: usize = 512;
const MAX_EXTRA_BYTES: usize = 32 * 1024;

#[cfg(test)]
pub(crate) const LEGACY_GENERAL_V1_SOURCE: &str = r#"(() => {
  const apiBase = "{{baseUrl}}";
  const usageUrl = /\/v1$/i.test(apiBase)
    ? `${apiBase}/usage`
    : `${apiBase}/v1/usage`;
  const finiteNumber = (value) => {
    if (typeof value === "number") {
      return Number.isFinite(value) ? value : undefined;
    }
    if (typeof value !== "string" || value.trim() === "") {
      return undefined;
    }
    const number = Number(value);
    return Number.isFinite(number) ? number : undefined;
  };
  const firstFinite = (...values) => {
    for (const value of values) {
      const number = finiteNumber(value);
      if (number !== undefined) return number;
    }
    return undefined;
  };
  const firstText = (...values) =>
    values.find((value) => typeof value === "string" && value.trim() !== "");

  return {
    request: {
      url: usageUrl,
      method: "GET",
      headers: {
        Accept: "application/json",
        Authorization: "Bearer {{apiKey}}",
      },
    },
    extractor: function (response) {
      const root =
        response && typeof response === "object" && !Array.isArray(response)
          ? response
          : {};
      const quota =
        root.quota &&
        typeof root.quota === "object" &&
        !Array.isArray(root.quota)
          ? root.quota
          : {};
      const subscription =
        root.subscription &&
        typeof root.subscription === "object" &&
        !Array.isArray(root.subscription)
          ? root.subscription
          : {};
      const dailyLimit = finiteNumber(subscription.daily_limit_usd);
      const dailyUsage = finiteNumber(subscription.daily_usage_usd);
      const hasDailySubscription =
        dailyLimit !== undefined && dailyLimit > 0 && dailyUsage !== undefined;
      const used = hasDailySubscription
        ? dailyUsage
        : firstFinite(quota.used, root.used);
      const total = hasDailySubscription
        ? dailyLimit
        : firstFinite(quota.limit, quota.total, root.limit, root.total);
      const subscriptionRemaining = hasDailySubscription
        ? dailyLimit - dailyUsage
        : undefined;
      const directRemaining = firstFinite(
        root.remaining,
        quota.remaining,
        root.balance,
      );
      const remaining =
        subscriptionRemaining ??
        directRemaining ??
        (total !== undefined && used !== undefined ? total - used : undefined);

      if (remaining === undefined) {
        return {
          isValid: false,
          invalidMessage: "No supported balance field found",
        };
      }

      return {
        isValid: true,
        remaining,
        used,
        total,
        unit: firstText(root.unit, quota.unit) ?? "USD",
        planName: firstText(
          root.planName,
          root.plan_name,
          quota.planName,
          quota.plan_name,
        ),
      };
    },
  };
})()"#;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum BalanceQueryMode {
    GeneralV1,
    CustomJs,
}

impl BalanceQueryMode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::GeneralV1 => "general_v1",
            Self::CustomJs => "custom_js",
        }
    }

    #[must_use]
    pub fn parse_persisted(value: &str) -> Option<Self> {
        match value {
            "general_v1" => Some(Self::GeneralV1),
            "custom_js" => Some(Self::CustomJs),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BalanceQueryConfig {
    pub mode: BalanceQueryMode,
    pub custom_source: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum BalanceErrorStage {
    Substitution,
    RequestScript,
    RequestValidation,
    Http,
    Response,
    ExtractorScript,
    ResultValidation,
    Timeout,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum BalanceErrorCategory {
    InvalidPlaceholder,
    SourceTooLarge,
    CredentialEncoding,
    ScriptInterrupted,
    ScriptFailed,
    InvalidRequest,
    RequestTooLarge,
    Network,
    HttpStatus,
    ResponseTooLarge,
    InvalidResponse,
    InvalidResult,
}

#[derive(Clone, Debug, Deserialize, Eq, Error, PartialEq, Serialize, TS)]
#[error("balance operation failed")]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct BalanceError {
    pub stage: BalanceErrorStage,
    pub category: BalanceErrorCategory,
    pub transient: bool,
}

impl BalanceError {
    const fn deterministic(stage: BalanceErrorStage, category: BalanceErrorCategory) -> Self {
        Self {
            stage,
            category,
            transient: false,
        }
    }

    const fn retryable(stage: BalanceErrorStage, category: BalanceErrorCategory) -> Self {
        Self {
            stage,
            category,
            transient: true,
        }
    }
}

pub struct BalanceHttpRequest {
    pub url: url::Url,
    pub method: Method,
    pub headers: HeaderMap,
    pub body: Option<Vec<u8>>,
}

#[derive(Clone, Deserialize, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct BalanceResult {
    pub is_valid: bool,
    pub remaining: Option<f64>,
    pub used: Option<f64>,
    pub total: Option<f64>,
    pub unit: Option<String>,
    pub plan_name: Option<String>,
    pub invalid_message: Option<String>,
    #[ts(type = "unknown")]
    pub extra: Option<Value>,
}

pub struct PreparedBalanceScript {
    source: Zeroizing<String>,
}

struct PreparedGeneralBalanceQuery {
    url: url::Url,
    authorization: HeaderValue,
}

enum PreparedBalanceQuery {
    GeneralV1(PreparedGeneralBalanceQuery),
    CustomJs(PreparedBalanceScript),
}

pub struct BalanceExecutor {
    client: reqwest::Client,
    attempt_timeout: Duration,
    retry_delay: Duration,
}

impl BalanceExecutor {
    /// Creates the dedicated balance HTTP client.
    ///
    /// # Errors
    ///
    /// Returns a client-construction error.
    pub fn new() -> Result<Self, reqwest::Error> {
        Self::with_timing(Duration::from_secs(10), Duration::from_millis(1_500))
    }

    fn with_timing(
        attempt_timeout: Duration,
        retry_delay: Duration,
    ) -> Result<Self, reqwest::Error> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                if !matches!(attempt.url().scheme(), "http" | "https") {
                    return attempt.stop();
                }
                if attempt.previous().len() >= 10 {
                    attempt.stop()
                } else {
                    attempt.follow()
                }
            }))
            .build()?;
        Ok(Self {
            client,
            attempt_timeout,
            retry_delay,
        })
    }

    /// Runs one balance query chain with at most one transient retry.
    ///
    /// # Errors
    ///
    /// Returns a safe stage/category error without request or response content.
    pub async fn query(
        &self,
        query: &BalanceQueryConfig,
        api_key: &ApiKey,
        base_url: &BaseUrl,
    ) -> Result<BalanceResult, BalanceError> {
        let prepared = PreparedBalanceQuery::prepare(query, api_key, base_url)?;
        let first = self.attempt(&prepared).await;
        if first.as_ref().is_err_and(|error| error.transient) {
            tokio::time::sleep(self.retry_delay).await;
            self.attempt(&prepared).await
        } else {
            first
        }
    }

    async fn attempt(
        &self,
        prepared: &PreparedBalanceQuery,
    ) -> Result<BalanceResult, BalanceError> {
        let started = Instant::now();
        let request = prepared
            .build_request(remaining(self.attempt_timeout, started)?)
            .await?;
        let mut builder = self
            .client
            .request(request.method, request.url)
            .headers(request.headers)
            .header(axum::http::header::ACCEPT_ENCODING, "identity");
        if let Some(body) = request.body {
            builder = builder.body(body);
        }
        let response =
            tokio::time::timeout(remaining(self.attempt_timeout, started)?, builder.send())
                .await
                .map_err(|_| timeout_error())?
                .map_err(|_| {
                    BalanceError::retryable(BalanceErrorStage::Http, BalanceErrorCategory::Network)
                })?;
        let status = response.status();
        if !status.is_success() {
            return Err(
                if status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
                    BalanceError::retryable(
                        BalanceErrorStage::Http,
                        BalanceErrorCategory::HttpStatus,
                    )
                } else {
                    BalanceError::deterministic(
                        BalanceErrorStage::Http,
                        BalanceErrorCategory::HttpStatus,
                    )
                },
            );
        }
        let encodings = response_encodings(response.headers());
        let wire =
            collect_balance_response(response, remaining(self.attempt_timeout, started)?).await?;
        let decoded = match decode_supported(wire, &encodings, MAX_BALANCE_RESPONSE_BYTES) {
            Ok(decoded) => decoded,
            Err(DecodeError::TooLarge) => {
                return Err(BalanceError::deterministic(
                    BalanceErrorStage::Response,
                    BalanceErrorCategory::ResponseTooLarge,
                ));
            }
            Err(DecodeError::Unsupported | DecodeError::Invalid) => {
                return Err(BalanceError::deterministic(
                    BalanceErrorStage::Response,
                    BalanceErrorCategory::InvalidResponse,
                ));
            }
        };
        prepared
            .extract(&decoded, remaining(self.attempt_timeout, started)?)
            .await
    }
}

impl PreparedBalanceQuery {
    fn prepare(
        query: &BalanceQueryConfig,
        api_key: &ApiKey,
        base_url: &BaseUrl,
    ) -> Result<Self, BalanceError> {
        match query.mode {
            BalanceQueryMode::GeneralV1 => {
                PreparedGeneralBalanceQuery::prepare(api_key, base_url).map(Self::GeneralV1)
            }
            BalanceQueryMode::CustomJs => {
                PreparedBalanceScript::prepare(&query.custom_source, api_key, base_url)
                    .map(Self::CustomJs)
            }
        }
    }

    async fn build_request(
        &self,
        time_remaining: Duration,
    ) -> Result<BalanceHttpRequest, BalanceError> {
        match self {
            Self::GeneralV1(query) => Ok(query.build_request()),
            Self::CustomJs(script) => script.build_request(time_remaining).await,
        }
    }

    async fn extract(
        &self,
        response_json: &[u8],
        time_remaining: Duration,
    ) -> Result<BalanceResult, BalanceError> {
        match self {
            Self::GeneralV1(_) => normalize_general_response(response_json, now_millis()),
            Self::CustomJs(script) => script.extract(response_json, time_remaining).await,
        }
    }
}

impl PreparedGeneralBalanceQuery {
    fn prepare(api_key: &ApiKey, base_url: &BaseUrl) -> Result<Self, BalanceError> {
        let base = base_url.as_str().trim_end_matches('/');
        let usage_url = if base.to_ascii_lowercase().ends_with("/v1") {
            format!("{base}/usage")
        } else {
            format!("{base}/v1/usage")
        };
        let url = url::Url::parse(&usage_url).map_err(|_| invalid_request())?;
        let mut authorization = Zeroizing::new(Vec::with_capacity(api_key.expose().len() + 7));
        authorization.extend_from_slice(b"Bearer ");
        authorization.extend_from_slice(api_key.expose());
        let authorization = HeaderValue::from_bytes(&authorization).map_err(|_| {
            BalanceError::deterministic(
                BalanceErrorStage::RequestValidation,
                BalanceErrorCategory::CredentialEncoding,
            )
        })?;
        Ok(Self { url, authorization })
    }

    fn build_request(&self) -> BalanceHttpRequest {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::ACCEPT,
            HeaderValue::from_static("application/json"),
        );
        headers.insert(
            axum::http::header::AUTHORIZATION,
            self.authorization.clone(),
        );
        BalanceHttpRequest {
            url: self.url.clone(),
            method: Method::GET,
            headers,
            body: None,
        }
    }
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(i64::MAX)
}

fn remaining(limit: Duration, started: Instant) -> Result<Duration, BalanceError> {
    let remaining = limit.saturating_sub(started.elapsed());
    if remaining.is_zero() {
        Err(timeout_error())
    } else {
        Ok(remaining)
    }
}

const fn timeout_error() -> BalanceError {
    BalanceError::retryable(BalanceErrorStage::Timeout, BalanceErrorCategory::Network)
}

async fn collect_balance_response(
    response: reqwest::Response,
    timeout: Duration,
) -> Result<Vec<u8>, BalanceError> {
    use futures_util::StreamExt;

    tokio::time::timeout(timeout, async move {
        let mut stream = response.bytes_stream();
        let mut wire = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| {
                BalanceError::retryable(BalanceErrorStage::Response, BalanceErrorCategory::Network)
            })?;
            if wire.len().saturating_add(chunk.len()) > MAX_BALANCE_RESPONSE_BYTES {
                return Err(BalanceError::deterministic(
                    BalanceErrorStage::Response,
                    BalanceErrorCategory::ResponseTooLarge,
                ));
            }
            wire.extend_from_slice(&chunk);
        }
        Ok(wire)
    })
    .await
    .map_err(|_| timeout_error())?
}

impl PreparedBalanceScript {
    /// Substitutes the two credential placeholders in one lexical pass.
    ///
    /// # Errors
    ///
    /// Returns a safe validation error for invalid placeholder placement,
    /// credential encoding, or post-substitution size.
    pub fn prepare(
        source: &str,
        api_key: &ApiKey,
        base_url: &BaseUrl,
    ) -> Result<Self, BalanceError> {
        if source.len() > MAX_BALANCE_SCRIPT_BYTES {
            return Err(BalanceError::deterministic(
                BalanceErrorStage::Substitution,
                BalanceErrorCategory::SourceTooLarge,
            ));
        }
        let key = std::str::from_utf8(api_key.expose()).map_err(|_| {
            BalanceError::deterministic(
                BalanceErrorStage::Substitution,
                BalanceErrorCategory::CredentialEncoding,
            )
        })?;
        let source = substitute_placeholders(source, key, base_url.as_str())?;
        Ok(Self {
            source: Zeroizing::new(source),
        })
    }

    /// Runs isolated `QuickJS` phase A and validates its request description.
    ///
    /// # Errors
    ///
    /// Returns only bounded stage/category information.
    pub async fn build_request(
        &self,
        time_remaining: Duration,
    ) -> Result<BalanceHttpRequest, BalanceError> {
        let source = Zeroizing::new(self.source.to_string());
        let phase_timeout = time_remaining.min(MAX_SCRIPT_PHASE);
        let json = tokio::task::spawn_blocking(move || {
            evaluate_json(
                &source,
                "const c = __SOURCE__; if (!c || typeof c !== 'object') throw new Error(); return JSON.stringify(c.request);",
                None,
                phase_timeout,
                BalanceErrorStage::RequestScript,
            )
        })
        .await
        .map_err(|_| {
            BalanceError::deterministic(
                BalanceErrorStage::RequestScript,
                BalanceErrorCategory::ScriptFailed,
            )
        })??;
        validate_http_request(&json)
    }

    /// Runs isolated `QuickJS` phase B and validates its normalized result.
    ///
    /// # Errors
    ///
    /// Returns only bounded stage/category information.
    pub async fn extract(
        &self,
        response_json: &[u8],
        time_remaining: Duration,
    ) -> Result<BalanceResult, BalanceError> {
        if response_json.len() > MAX_BALANCE_RESPONSE_BYTES {
            return Err(BalanceError::deterministic(
                BalanceErrorStage::Response,
                BalanceErrorCategory::ResponseTooLarge,
            ));
        }
        serde_json::from_slice::<Value>(response_json).map_err(|_| {
            BalanceError::deterministic(
                BalanceErrorStage::Response,
                BalanceErrorCategory::InvalidResponse,
            )
        })?;
        let source = Zeroizing::new(self.source.to_string());
        let response = response_json.to_vec();
        let phase_timeout = time_remaining.min(MAX_SCRIPT_PHASE);
        let json = tokio::task::spawn_blocking(move || {
            evaluate_json(
                &source,
                "const c = __SOURCE__; if (!c || typeof c.extractor !== 'function') throw new Error(); const r = c.extractor(__RESPONSE__); for (const k of ['remaining','used','total']) { if (k in Object(r) && r[k] !== undefined && r[k] !== null && (typeof r[k] !== 'number' || !Number.isFinite(r[k]))) throw new Error(); } return JSON.stringify(r);",
                Some(&response),
                phase_timeout,
                BalanceErrorStage::ExtractorScript,
            )
        })
        .await
        .map_err(|_| {
            BalanceError::deterministic(
                BalanceErrorStage::ExtractorScript,
                BalanceErrorCategory::ScriptFailed,
            )
        })??;
        validate_balance_result(&json)
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum LexState {
    Normal,
    DoubleQuoted,
    SingleQuoted,
    Template,
    LineComment,
    BlockComment,
}

fn substitute_placeholders(
    source: &str,
    api_key: &str,
    base_url: &str,
) -> Result<String, BalanceError> {
    let escaped_key = json_string_content(api_key)?;
    let escaped_base = json_string_content(base_url)?;
    let mut output = String::with_capacity(source.len());
    let bytes = source.as_bytes();
    let mut state = LexState::Normal;
    let mut index = 0;
    while index < bytes.len() {
        if source[index..].starts_with("{{apiKey}}") || source[index..].starts_with("{{baseUrl}}") {
            if state != LexState::DoubleQuoted {
                return Err(invalid_placeholder());
            }
            let (placeholder_len, replacement) = if source[index..].starts_with("{{apiKey}}") {
                ("{{apiKey}}".len(), escaped_key.as_str())
            } else {
                ("{{baseUrl}}".len(), escaped_base.as_str())
            };
            push_bounded(&mut output, replacement)?;
            index += placeholder_len;
            continue;
        }
        if source[index..].starts_with("{{") && state == LexState::DoubleQuoted {
            return Err(invalid_placeholder());
        }
        let byte = bytes[index];
        match state {
            LexState::Normal => {
                if byte == b'"' {
                    state = LexState::DoubleQuoted;
                } else if byte == b'\'' {
                    state = LexState::SingleQuoted;
                } else if byte == b'`' {
                    state = LexState::Template;
                } else if bytes.get(index..index + 2) == Some(b"//") {
                    state = LexState::LineComment;
                } else if bytes.get(index..index + 2) == Some(b"/*") {
                    state = LexState::BlockComment;
                }
            }
            LexState::DoubleQuoted | LexState::SingleQuoted | LexState::Template => {
                if byte == b'\\' {
                    let end = next_character_end(source, index + 1);
                    push_bounded(&mut output, &source[index..end])?;
                    index = end;
                    continue;
                }
                if (state == LexState::DoubleQuoted && byte == b'"')
                    || (state == LexState::SingleQuoted && byte == b'\'')
                    || (state == LexState::Template && byte == b'`')
                {
                    state = LexState::Normal;
                }
            }
            LexState::LineComment => {
                if byte == b'\n' || byte == b'\r' {
                    state = LexState::Normal;
                }
            }
            LexState::BlockComment => {
                if bytes.get(index..index + 2) == Some(b"*/") {
                    push_bounded(&mut output, "*/")?;
                    index += 2;
                    state = LexState::Normal;
                    continue;
                }
            }
        }
        let end = next_character_end(source, index);
        push_bounded(&mut output, &source[index..end])?;
        index = end;
    }
    Ok(output)
}

fn next_character_end(source: &str, index: usize) -> usize {
    if index >= source.len() {
        return index;
    }
    index
        + source[index..]
            .chars()
            .next()
            .map(char::len_utf8)
            .unwrap_or_default()
}

fn json_string_content(value: &str) -> Result<String, BalanceError> {
    let encoded = serde_json::to_string(value).map_err(|_| invalid_placeholder())?;
    Ok(encoded[1..encoded.len() - 1].to_owned())
}

fn push_bounded(output: &mut String, value: &str) -> Result<(), BalanceError> {
    if output.len().saturating_add(value.len()) > MAX_SUBSTITUTED_SOURCE_BYTES {
        return Err(BalanceError::deterministic(
            BalanceErrorStage::Substitution,
            BalanceErrorCategory::SourceTooLarge,
        ));
    }
    output.push_str(value);
    Ok(())
}

const fn invalid_placeholder() -> BalanceError {
    BalanceError::deterministic(
        BalanceErrorStage::Substitution,
        BalanceErrorCategory::InvalidPlaceholder,
    )
}

fn evaluate_json(
    source: &str,
    template: &str,
    response: Option<&[u8]>,
    timeout: Duration,
    stage: BalanceErrorStage,
) -> Result<String, BalanceError> {
    let runtime = Runtime::new().map_err(|_| {
        BalanceError::deterministic(stage.clone(), BalanceErrorCategory::ScriptFailed)
    })?;
    runtime.set_memory_limit(SCRIPT_MEMORY_LIMIT);
    runtime.set_max_stack_size(SCRIPT_STACK_LIMIT);
    let started = Instant::now();
    runtime.set_interrupt_handler(Some(Box::new(move || started.elapsed() >= timeout)));
    let context = Context::full(&runtime).map_err(|_| {
        BalanceError::deterministic(stage.clone(), BalanceErrorCategory::ScriptFailed)
    })?;
    context.with(|context| {
        if let Some(response) = response {
            let value = context.json_parse(response).map_err(|_| {
                BalanceError::deterministic(
                    BalanceErrorStage::Response,
                    BalanceErrorCategory::InvalidResponse,
                )
            })?;
            context.globals().set("__RESPONSE__", value).map_err(|_| {
                BalanceError::deterministic(stage.clone(), BalanceErrorCategory::ScriptFailed)
            })?;
        }
        let mut program = Zeroizing::new(template.replace("__SOURCE__", source));
        if program.len() > MAX_SUBSTITUTED_SOURCE_BYTES.saturating_mul(2) {
            return Err(BalanceError::deterministic(
                stage,
                BalanceErrorCategory::SourceTooLarge,
            ));
        }
        program.insert_str(0, "(() => {");
        program.push_str("})()");
        context
            .eval::<String, _>(program.as_bytes())
            .map_err(|error| {
                let category = if error.is_exception() && started.elapsed() >= timeout {
                    BalanceErrorCategory::ScriptInterrupted
                } else {
                    BalanceErrorCategory::ScriptFailed
                };
                BalanceError::deterministic(stage, category)
            })
    })
}

#[derive(Deserialize)]
struct RawBalanceRequest {
    url: String,
    method: String,
    #[serde(default)]
    headers: BTreeMap<String, String>,
    body: Option<Value>,
}

fn validate_http_request(json: &str) -> Result<BalanceHttpRequest, BalanceError> {
    let raw: RawBalanceRequest = serde_json::from_str(json).map_err(|_| invalid_request())?;
    if raw.url.len() > MAX_BALANCE_URL_BYTES {
        return Err(invalid_request());
    }
    let url = url::Url::parse(&raw.url).map_err(|_| invalid_request())?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(invalid_request());
    }
    let method = Method::from_str(&raw.method).map_err(|_| invalid_request())?;
    if matches!(method, Method::CONNECT | Method::TRACE) {
        return Err(invalid_request());
    }
    if raw.headers.len() > MAX_BALANCE_HEADERS {
        return Err(invalid_request());
    }
    let mut headers = HeaderMap::new();
    let mut header_bytes = 0usize;
    for (name, value) in raw.headers {
        header_bytes = header_bytes
            .saturating_add(name.len())
            .saturating_add(value.len());
        if header_bytes > MAX_BALANCE_HEADER_BYTES {
            return Err(invalid_request());
        }
        let name = HeaderName::from_str(&name).map_err(|_| invalid_request())?;
        let value = HeaderValue::from_str(&value).map_err(|_| invalid_request())?;
        headers.append(name, value);
    }
    let body = raw
        .body
        .map(|body| serde_json::to_vec(&body).map_err(|_| invalid_request()))
        .transpose()?;
    if body
        .as_ref()
        .is_some_and(|body| body.len() > MAX_BALANCE_BODY_BYTES)
    {
        return Err(BalanceError::deterministic(
            BalanceErrorStage::RequestValidation,
            BalanceErrorCategory::RequestTooLarge,
        ));
    }
    Ok(BalanceHttpRequest {
        url,
        method,
        headers,
        body,
    })
}

const fn invalid_request() -> BalanceError {
    BalanceError::deterministic(
        BalanceErrorStage::RequestValidation,
        BalanceErrorCategory::InvalidRequest,
    )
}

fn normalize_general_response(
    response_json: &[u8],
    now_ms: i64,
) -> Result<BalanceResult, BalanceError> {
    let value: Value = serde_json::from_slice(response_json).map_err(|_| {
        BalanceError::deterministic(
            BalanceErrorStage::Response,
            BalanceErrorCategory::InvalidResponse,
        )
    })?;
    let root = value.as_object().ok_or_else(|| {
        BalanceError::deterministic(
            BalanceErrorStage::Response,
            BalanceErrorCategory::InvalidResponse,
        )
    })?;
    let result = match root.get("mode") {
        Some(Value::String(mode)) if mode == "quota_limited" => {
            normalize_quota_limited(root, now_ms)
        }
        Some(Value::String(mode)) if mode == "unrestricted" => normalize_unrestricted(root, now_ms),
        Some(_) => invalid_general_result("Unsupported balance response mode"),
        None => normalize_legacy(root),
    };
    validate_normalized_balance_result(result)
}

fn normalize_quota_limited(root: &serde_json::Map<String, Value>, now_ms: i64) -> BalanceResult {
    match root.get("status").and_then(Value::as_str) {
        Some("quota_exhausted") => {
            return valid_general_result(
                0.0,
                None,
                None,
                general_unit(root, root.get("quota").and_then(Value::as_object)),
                general_plan_name(root, root.get("quota").and_then(Value::as_object)),
            );
        }
        Some("active") => {}
        Some("expired") => return invalid_general_result("API Key has expired"),
        _ => return invalid_general_result("API Key is not active"),
    }

    if let Some(expires_at) = root.get("expires_at") {
        let Some(expires_at) = expires_at.as_str() else {
            return invalid_general_result("Invalid API Key expiry");
        };
        let Ok(expires_at) = OffsetDateTime::parse(expires_at, &Rfc3339) else {
            return invalid_general_result("Invalid API Key expiry");
        };
        let expires_at_ms = expires_at.unix_timestamp_nanos() / 1_000_000;
        if expires_at_ms <= i128::from(now_ms) {
            return invalid_general_result("API Key has expired");
        }
    }

    let quota = root.get("quota").and_then(Value::as_object);
    let quota_remaining = quota
        .and_then(|quota| quota.get("remaining"))
        .and_then(non_negative_number);
    let mut candidates = quota_remaining.into_iter().collect::<Vec<_>>();
    if let Some(rate_limits) = root.get("rate_limits").and_then(Value::as_array) {
        for rate_limit in rate_limits {
            let Some(rate_limit) = rate_limit.as_object() else {
                continue;
            };
            if !matches!(
                rate_limit.get("window").and_then(Value::as_str),
                Some("5h" | "1d" | "7d")
            ) || !rate_limit
                .get("limit")
                .and_then(finite_number)
                .is_some_and(|limit| limit > 0.0)
            {
                continue;
            }
            if let Some(remaining) = rate_limit.get("remaining").and_then(non_negative_number) {
                candidates.push(remaining);
            }
        }
    }
    let Some(remaining) = candidates.into_iter().reduce(f64::min) else {
        return invalid_general_result("No supported Key quota found");
    };
    let quota_won =
        quota_remaining.is_some_and(|candidate| candidate.total_cmp(&remaining).is_eq());
    valid_general_result(
        remaining,
        quota_won
            .then(|| {
                quota
                    .and_then(|quota| quota.get("used"))
                    .and_then(non_negative_number)
            })
            .flatten(),
        quota_won
            .then(|| {
                quota.and_then(|quota| {
                    quota
                        .get("limit")
                        .or_else(|| quota.get("total"))
                        .and_then(non_negative_number)
                })
            })
            .flatten(),
        general_unit(root, quota),
        general_plan_name(root, quota),
    )
}

fn normalize_unrestricted(root: &serde_json::Map<String, Value>, now_ms: i64) -> BalanceResult {
    let subscription = root.get("subscription").and_then(Value::as_object);
    if let Some(subscription) = subscription {
        let top_level_remaining = root.get("remaining").and_then(non_negative_number);
        let daily_reset = unrestricted_daily_reset(root, subscription, top_level_remaining, now_ms);
        let (remaining, used, total) = daily_reset.map_or_else(
            || {
                (
                    top_level_remaining.or_else(|| {
                        ["daily", "weekly", "monthly"]
                            .into_iter()
                            .filter_map(|period| {
                                let limit = subscription
                                    .get(&format!("{period}_limit_usd"))
                                    .and_then(finite_number)?;
                                let usage = subscription
                                    .get(&format!("{period}_usage_usd"))
                                    .and_then(non_negative_number)?;
                                (limit > 0.0).then_some((limit - usage).max(0.0))
                            })
                            .reduce(f64::min)
                    }),
                    None,
                    None,
                )
            },
            |(limit, usage)| (Some(limit), Some(usage), Some(limit)),
        );
        return remaining.map_or_else(
            || invalid_general_result("No supported subscription quota found"),
            |remaining| {
                valid_general_result(
                    remaining,
                    used,
                    total,
                    general_unit(root, None),
                    bounded_text(
                        subscription
                            .get("plan_name")
                            .or_else(|| subscription.get("planName")),
                        MAX_PLAN_NAME_CHARS,
                    )
                    .or_else(|| general_plan_name(root, None)),
                )
            },
        );
    }

    root.get("balance")
        .and_then(finite_number)
        .or_else(|| root.get("remaining").and_then(finite_number))
        .map_or_else(
            || invalid_general_result("No supported wallet balance found"),
            |remaining| {
                valid_general_result(
                    remaining.max(0.0),
                    None,
                    None,
                    general_unit(root, None),
                    general_plan_name(root, None),
                )
            },
        )
}

fn unrestricted_daily_reset(
    root: &serde_json::Map<String, Value>,
    subscription: &serde_json::Map<String, Value>,
    top_level_remaining: Option<f64>,
    now_ms: i64,
) -> Option<(f64, f64)> {
    if top_level_remaining? != 0.0 {
        return None;
    }
    let limit = subscription
        .get("daily_limit_usd")
        .and_then(finite_number)
        .filter(|limit| *limit > 0.0)?;
    let usage = subscription
        .get("daily_usage_usd")
        .and_then(finite_number)?;
    if usage == 0.0 {
        return Some((limit, usage));
    }
    if usage < limit {
        return None;
    }

    let current_date = provider_local_date(subscription, now_ms)?;
    let latest_usage_date = latest_daily_usage_date(root, current_date)?;
    (latest_usage_date < current_date).then_some((limit, 0.0))
}

fn provider_local_date(subscription: &serde_json::Map<String, Value>, now_ms: i64) -> Option<Date> {
    let response_timestamp = subscription.get("weekly_window_start")?.as_str()?;
    let response_timestamp = OffsetDateTime::parse(response_timestamp, &Rfc3339).ok()?;
    let now_nanos = i128::from(now_ms).checked_mul(1_000_000)?;
    OffsetDateTime::from_unix_timestamp_nanos(now_nanos)
        .ok()
        .map(|now| now.to_offset(response_timestamp.offset()).date())
}

fn latest_daily_usage_date(
    root: &serde_json::Map<String, Value>,
    current_date: Date,
) -> Option<Date> {
    root.get("daily_usage")?
        .as_array()?
        .iter()
        .try_fold(None, |latest: Option<Date>, item| {
            let date = parse_calendar_date(item.as_object()?.get("date")?.as_str()?)?;
            if date > current_date {
                return None;
            }
            Some(Some(latest.map_or(date, |latest| latest.max(date))))
        })?
}

fn parse_calendar_date(value: &str) -> Option<Date> {
    let bytes = value.as_bytes();
    if bytes.len() != 10
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes
            .iter()
            .enumerate()
            .any(|(index, byte)| index != 4 && index != 7 && !byte.is_ascii_digit())
    {
        return None;
    }

    let year = i32::from(bytes[0] - b'0') * 1_000
        + i32::from(bytes[1] - b'0') * 100
        + i32::from(bytes[2] - b'0') * 10
        + i32::from(bytes[3] - b'0');
    let month = (bytes[5] - b'0') * 10 + (bytes[6] - b'0');
    let day = (bytes[8] - b'0') * 10 + (bytes[9] - b'0');
    Date::from_calendar_date(year, Month::try_from(month).ok()?, day).ok()
}

fn normalize_legacy(root: &serde_json::Map<String, Value>) -> BalanceResult {
    let quota = root.get("quota").and_then(Value::as_object);
    let remaining = root
        .get("remaining")
        .and_then(non_negative_number)
        .or_else(|| {
            quota
                .and_then(|quota| quota.get("remaining"))
                .and_then(non_negative_number)
        })
        .or_else(|| {
            root.get("balance")
                .and_then(finite_number)
                .map(|value| value.max(0.0))
        })
        .or_else(|| {
            let total = quota
                .and_then(|quota| quota.get("total").or_else(|| quota.get("limit")))
                .or_else(|| root.get("total").or_else(|| root.get("limit")))
                .and_then(non_negative_number)?;
            let used = quota
                .and_then(|quota| quota.get("used"))
                .or_else(|| root.get("used"))
                .and_then(non_negative_number)?;
            Some((total - used).max(0.0))
        });
    remaining.map_or_else(
        || invalid_general_result("No supported balance field found"),
        |remaining| {
            valid_general_result(
                remaining,
                None,
                None,
                general_unit(root, quota),
                general_plan_name(root, quota),
            )
        },
    )
}

fn finite_number(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| {
            value
                .as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .and_then(|value| value.parse::<f64>().ok())
        })
        .filter(|value| value.is_finite())
}

fn non_negative_number(value: &Value) -> Option<f64> {
    finite_number(value).filter(|value| *value >= 0.0)
}

fn general_unit(
    root: &serde_json::Map<String, Value>,
    quota: Option<&serde_json::Map<String, Value>>,
) -> Option<String> {
    bounded_text(root.get("unit"), MAX_UNIT_CHARS)
        .or_else(|| quota.and_then(|quota| bounded_text(quota.get("unit"), MAX_UNIT_CHARS)))
        .or_else(|| Some("USD".to_owned()))
}

fn general_plan_name(
    root: &serde_json::Map<String, Value>,
    quota: Option<&serde_json::Map<String, Value>>,
) -> Option<String> {
    bounded_text(
        root.get("planName").or_else(|| root.get("plan_name")),
        MAX_PLAN_NAME_CHARS,
    )
    .or_else(|| {
        quota.and_then(|quota| {
            bounded_text(
                quota.get("planName").or_else(|| quota.get("plan_name")),
                MAX_PLAN_NAME_CHARS,
            )
        })
    })
}

fn bounded_text(value: Option<&Value>, maximum: usize) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.chars().count() <= maximum)
        .map(ToOwned::to_owned)
}

fn valid_general_result(
    remaining: f64,
    used: Option<f64>,
    total: Option<f64>,
    unit: Option<String>,
    plan_name: Option<String>,
) -> BalanceResult {
    BalanceResult {
        is_valid: true,
        remaining: Some(remaining),
        used,
        total,
        unit,
        plan_name,
        invalid_message: None,
        extra: None,
    }
}

fn invalid_general_result(message: &str) -> BalanceResult {
    BalanceResult {
        is_valid: false,
        remaining: None,
        used: None,
        total: None,
        unit: None,
        plan_name: None,
        invalid_message: Some(message.to_owned()),
        extra: None,
    }
}

fn validate_balance_result(json: &str) -> Result<BalanceResult, BalanceError> {
    let value: Value = serde_json::from_str(json).map_err(|_| invalid_result())?;
    let object = value.as_object().ok_or_else(invalid_result)?;
    let is_valid = object
        .get("isValid")
        .and_then(Value::as_bool)
        .ok_or_else(invalid_result)?;
    let remaining = optional_finite(object.get("remaining"))?;
    let used = optional_finite(object.get("used"))?;
    let total = optional_finite(object.get("total"))?;
    let unit = optional_bounded_text(object.get("unit"), MAX_UNIT_CHARS)?;
    let plan_name = optional_bounded_text(object.get("planName"), MAX_PLAN_NAME_CHARS)?;
    let invalid_message =
        optional_bounded_text(object.get("invalidMessage"), MAX_INVALID_MESSAGE_CHARS)?;
    let extra = object.get("extra").cloned();
    validate_normalized_balance_result(BalanceResult {
        is_valid,
        remaining,
        used,
        total,
        unit,
        plan_name,
        invalid_message,
        extra,
    })
}

fn validate_normalized_balance_result(
    result: BalanceResult,
) -> Result<BalanceResult, BalanceError> {
    if [result.remaining, result.used, result.total]
        .into_iter()
        .flatten()
        .any(|value| !value.is_finite())
        || result.used.is_some_and(|value| value < 0.0)
        || result.total.is_some_and(|value| value < 0.0)
        || (result.is_valid && result.remaining.is_none())
        || (!result.is_valid && result.invalid_message.as_deref().is_none_or(str::is_empty))
        || result
            .unit
            .as_deref()
            .is_some_and(|value| value.chars().count() > MAX_UNIT_CHARS)
        || result
            .plan_name
            .as_deref()
            .is_some_and(|value| value.chars().count() > MAX_PLAN_NAME_CHARS)
        || result
            .invalid_message
            .as_deref()
            .is_some_and(|value| value.chars().count() > MAX_INVALID_MESSAGE_CHARS)
        || result.extra.as_ref().is_some_and(|extra| {
            serde_json::to_vec(extra).map_or(true, |value| value.len() > MAX_EXTRA_BYTES)
        })
    {
        return Err(invalid_result());
    }
    Ok(result)
}

fn optional_finite(value: Option<&Value>) -> Result<Option<f64>, BalanceError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    value
        .as_f64()
        .filter(|value| value.is_finite())
        .map(Some)
        .ok_or_else(invalid_result)
}

fn optional_bounded_text(
    value: Option<&Value>,
    maximum: usize,
) -> Result<Option<String>, BalanceError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let value = value.as_str().ok_or_else(invalid_result)?.trim();
    if value.chars().count() > maximum {
        return Err(invalid_result());
    }
    Ok(Some(value.to_owned()))
}

const fn invalid_result() -> BalanceError {
    BalanceError::deterministic(
        BalanceErrorStage::ResultValidation,
        BalanceErrorCategory::InvalidResult,
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
        body::{Body, Bytes},
        extract::{Request, State},
        http::StatusCode,
        response::{IntoResponse, Response},
    };
    use sha2::{Digest, Sha256};
    use tokio::{net::TcpListener, sync::oneshot, task::JoinHandle};

    use super::*;

    fn key() -> ApiKey {
        ApiKey::parse("route-secret").expect("API key")
    }

    fn base(value: &str) -> BaseUrl {
        BaseUrl::parse(value).expect("base URL")
    }

    #[derive(Clone)]
    struct MockBalanceState {
        calls: Arc<AtomicUsize>,
        statuses: Arc<Vec<StatusCode>>,
        response: Bytes,
        delay: Duration,
        request_headers: Arc<Mutex<Vec<HeaderMap>>>,
    }

    async fn mock_balance_handler(
        State(state): State<MockBalanceState>,
        request: Request,
    ) -> Response {
        let call = state.calls.fetch_add(1, Ordering::SeqCst);
        state
            .request_headers
            .lock()
            .expect("request header mutex")
            .push(request.headers().clone());
        tokio::time::sleep(state.delay).await;
        let status = state
            .statuses
            .get(call)
            .copied()
            .or_else(|| state.statuses.last().copied())
            .unwrap_or(StatusCode::OK);
        (status, Body::from(state.response)).into_response()
    }

    struct MockBalanceServer {
        address: std::net::SocketAddr,
        shutdown: Option<oneshot::Sender<()>>,
        task: JoinHandle<std::io::Result<()>>,
    }

    impl MockBalanceServer {
        async fn start(state: MockBalanceState) -> Self {
            let listener = TcpListener::bind(("127.0.0.1", 0))
                .await
                .expect("mock balance listener");
            let address = listener.local_addr().expect("mock balance address");
            let (shutdown, receiver) = oneshot::channel();
            let router = Router::new()
                .fallback(mock_balance_handler)
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

        async fn shutdown(mut self) {
            if let Some(shutdown) = self.shutdown.take() {
                let _ = shutdown.send(());
            }
            let _ = self.task.await;
        }
    }

    fn mock_state(statuses: Vec<StatusCode>, response: &Value) -> MockBalanceState {
        MockBalanceState {
            calls: Arc::new(AtomicUsize::new(0)),
            statuses: Arc::new(statuses),
            response: Bytes::from(serde_json::to_vec(response).expect("mock response JSON")),
            delay: Duration::ZERO,
            request_headers: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn query_source(url: &str) -> String {
        format!(
            r#"(() => ({{
              request: {{
                url: "{url}",
                method: "GET",
                headers: {{ Authorization: "Bearer {{{{apiKey}}}}" }}
              }},
              extractor: (response) => ({{ isValid: true, remaining: response.remaining }})
            }}))()"#
        )
    }

    fn custom_query(source: String) -> BalanceQueryConfig {
        BalanceQueryConfig {
            mode: BalanceQueryMode::CustomJs,
            custom_source: source,
        }
    }

    #[test]
    fn legacy_general_v1_source_hash_is_stable() {
        let hash = format!("{:x}", Sha256::digest(LEGACY_GENERAL_V1_SOURCE.as_bytes()));
        assert_eq!(
            hash,
            "24cbea85c2fa635112e5915836e2a78144e0a6a21997b86ef5187c2665e14507"
        );
    }

    #[test]
    fn balance_query_modes_are_closed_and_use_stable_wire_values() {
        assert_eq!(
            serde_json::to_string(&BalanceQueryMode::GeneralV1).expect("serialize general mode"),
            "\"general_v1\""
        );
        assert_eq!(
            serde_json::to_string(&BalanceQueryMode::CustomJs).expect("serialize custom mode"),
            "\"custom_js\""
        );
        assert!(serde_json::from_str::<BalanceQueryMode>("\"future\"").is_err());
    }

    #[tokio::test]
    async fn custom_script_scaffold_uses_route_credentials_and_starts_invalid() {
        let source = include_str!("../../../src/features/settings/customBalanceScriptScaffold.txt");
        let prepared = PreparedBalanceScript::prepare(
            source,
            &ApiKey::parse("scaffold-key").expect("API key"),
            &base("https://example.test/v1"),
        )
        .expect("prepared scaffold");

        let request = prepared
            .build_request(Duration::from_millis(100))
            .await
            .expect("scaffold request");
        assert_eq!(request.url.as_str(), "https://example.test/v1");
        assert_eq!(request.method, Method::GET);
        assert_eq!(
            request.headers.get(axum::http::header::AUTHORIZATION),
            Some(&HeaderValue::from_static("Bearer scaffold-key"))
        );

        let result = prepared
            .extract(b"{}", Duration::from_millis(100))
            .await
            .expect("scaffold result");
        assert!(!result.is_valid);
        assert_eq!(result.remaining, None);
        assert_eq!(
            result.invalid_message.as_deref(),
            Some("请根据接口响应实现余额解析")
        );
    }

    const GENERAL_V1_NOW_MS: i64 = 1_786_293_000_000;

    fn normalize_general_at(fixture: impl Serialize, now_ms: i64) -> BalanceResult {
        normalize_general_response(&serde_json::to_vec(&fixture).expect("fixture JSON"), now_ms)
            .expect("general response")
    }

    fn normalize_general(fixture: impl Serialize) -> BalanceResult {
        normalize_general_at(fixture, GENERAL_V1_NOW_MS)
    }

    #[test]
    fn general_v1_uses_smallest_key_local_constraint_independent_of_order() {
        let result = normalize_general(serde_json::json!({
            "mode": "quota_limited",
            "status": "active",
            "expires_at": "2999-01-01T00:00:00Z",
            "quota": { "limit": 100, "used": 20, "remaining": 80, "unit": "USD" },
            "rate_limits": [
                { "window": "7d", "limit": 90, "remaining": 45 },
                { "window": "5h", "limit": 20, "remaining": 12 },
                { "window": "1d", "limit": 30, "remaining": 0 }
            ]
        }));
        assert!(result.is_valid);
        assert_eq!(result.remaining, Some(0.0));
        assert_eq!(result.used, None);
        assert_eq!(result.total, None);
    }

    #[test]
    fn general_v1_accepts_total_and_each_supported_rate_window() {
        for (fixture, expected) in [
            (
                serde_json::json!({
                    "mode": "quota_limited",
                    "status": "active",
                    "quota": { "limit": 20, "remaining": 11 }
                }),
                11.0,
            ),
            (
                serde_json::json!({
                    "mode": "quota_limited",
                    "status": "active",
                    "rate_limits": [{ "window": "5h", "limit": 20, "remaining": 9 }]
                }),
                9.0,
            ),
            (
                serde_json::json!({
                    "mode": "quota_limited",
                    "status": "active",
                    "rate_limits": [{ "window": "1d", "limit": 20, "remaining": 8 }]
                }),
                8.0,
            ),
            (
                serde_json::json!({
                    "mode": "quota_limited",
                    "status": "active",
                    "rate_limits": [{ "window": "7d", "limit": 20, "remaining": 7 }]
                }),
                7.0,
            ),
        ] {
            assert_eq!(normalize_general(fixture).remaining, Some(expected));
        }
    }

    #[test]
    fn general_v1_trusts_effective_window_remaining_and_ignores_invalid_candidates() {
        let result = normalize_general(serde_json::json!({
            "mode": "quota_limited",
            "status": "active",
            "quota": { "remaining": -1 },
            "rate_limits": [
                {
                    "window": "5h",
                    "limit": 20,
                    "used": 999,
                    "remaining": 20,
                    "window_start": null
                },
                { "window": "1d", "limit": 0, "remaining": 0 },
                { "window": "7d", "limit": 50, "remaining": "invalid" },
                { "window": "30d", "limit": 1, "remaining": 0 }
            ]
        }));
        assert_eq!(result.remaining, Some(20.0));
    }

    #[test]
    fn general_v1_supports_rate_only_and_rejects_inactive_keys() {
        let rate_only = normalize_general(serde_json::json!({
            "mode": "quota_limited",
            "status": "active",
            "rate_limits": [
                { "window": "1d", "limit": "25", "remaining": "7.5" }
            ]
        }));
        assert_eq!(rate_only.remaining, Some(7.5));

        let exhausted = normalize_general(serde_json::json!({
            "mode": "quota_limited",
            "status": "quota_exhausted",
            "quota": { "remaining": 10 }
        }));
        assert_eq!(exhausted.remaining, Some(0.0));

        for fixture in [
            serde_json::json!({
                "mode": "quota_limited",
                "status": "expired",
                "quota": { "remaining": 10 }
            }),
            serde_json::json!({
                "mode": "quota_limited",
                "status": "active",
                "expires_at": "2020-01-01T00:00:00Z",
                "quota": { "remaining": 10 }
            }),
        ] {
            let result = normalize_general(fixture);
            assert!(!result.is_valid);
            assert_eq!(result.remaining, None);
        }
    }

    #[test]
    fn general_v1_uses_daily_quota_at_exact_unrestricted_reset() {
        let result = normalize_general(serde_json::json!({
            "mode": "unrestricted",
            "remaining": 0,
            "subscription": {
                "daily_limit_usd": 300,
                "daily_usage_usd": 0
            }
        }));

        assert!(result.is_valid);
        assert_eq!(result.remaining, Some(300.0));
        assert_eq!(result.used, Some(0.0));
        assert_eq!(result.total, Some(300.0));
    }

    #[test]
    fn general_v1_resets_stale_unrestricted_usage_after_provider_midnight() {
        let result = normalize_general_at(
            serde_json::json!({
                "mode": "unrestricted",
                "remaining": 0,
                "subscription": {
                    "daily_limit_usd": 300,
                    "daily_usage_usd": 301.192_284_12,
                    "weekly_window_start": "2026-08-06T00:00:00+08:00"
                },
                "daily_usage": [
                    { "date": "2026-08-09" },
                    { "date": "2026-08-08" }
                ]
            }),
            GENERAL_V1_NOW_MS,
        );

        assert!(result.is_valid);
        assert_eq!(result.remaining, Some(300.0));
        assert_eq!(result.used, Some(0.0));
        assert_eq!(result.total, Some(300.0));
    }

    #[test]
    fn general_v1_rejects_incomplete_or_malformed_stale_reset_evidence() {
        let valid_timestamp = serde_json::json!("2026-08-06T00:00:00+08:00");
        let prior_day_history = serde_json::json!([{ "date": "2026-08-09" }]);
        for (timestamp, history) in [
            (None, Some(prior_day_history.clone())),
            (
                Some(serde_json::json!("not-an-rfc3339-timestamp")),
                Some(prior_day_history.clone()),
            ),
            (Some(valid_timestamp.clone()), None),
            (Some(valid_timestamp.clone()), Some(serde_json::json!([]))),
            (
                Some(valid_timestamp.clone()),
                Some(serde_json::json!({ "date": "2026-08-09" })),
            ),
            (
                Some(valid_timestamp.clone()),
                Some(serde_json::json!([
                    { "date": "2026-08-09" },
                    { "date": "2026-8-08" }
                ])),
            ),
            (
                Some(valid_timestamp.clone()),
                Some(serde_json::json!([{ "date": "2026-02-30" }])),
            ),
            (
                Some(valid_timestamp),
                Some(serde_json::json!([
                    { "date": "2026-08-09" },
                    { "date": "2026-08-11" }
                ])),
            ),
        ] {
            let mut fixture = serde_json::json!({
                "mode": "unrestricted",
                "remaining": 0,
                "subscription": {
                    "daily_limit_usd": 300,
                    "daily_usage_usd": 301
                }
            });
            if let Some(timestamp) = timestamp {
                fixture["subscription"]["weekly_window_start"] = timestamp;
            }
            if let Some(history) = history {
                fixture["daily_usage"] = history;
            }

            let result = normalize_general_at(fixture, GENERAL_V1_NOW_MS);
            assert!(result.is_valid);
            assert_eq!(result.remaining, Some(0.0));
            assert_eq!(result.used, None);
            assert_eq!(result.total, None);
        }
    }

    #[test]
    fn general_v1_uses_latest_date_from_unsorted_stale_reset_history() {
        let result = normalize_general_at(
            serde_json::json!({
                "mode": "unrestricted",
                "remaining": 0,
                "subscription": {
                    "daily_limit_usd": 300,
                    "daily_usage_usd": 301,
                    "weekly_window_start": "2026-08-06T00:00:00+08:00"
                },
                "daily_usage": [
                    { "date": "2026-08-09" },
                    { "date": "2026-08-10" },
                    { "date": "2026-08-08" }
                ]
            }),
            GENERAL_V1_NOW_MS,
        );

        assert!(result.is_valid);
        assert_eq!(result.remaining, Some(0.0));
        assert_eq!(result.used, None);
        assert_eq!(result.total, None);
    }

    #[test]
    fn general_v1_preserves_unrestricted_remaining_outside_exact_daily_reset() {
        for (fixture, expected_remaining) in [
            (
                serde_json::json!({
                    "mode": "unrestricted",
                    "remaining": 0.5,
                    "subscription": {
                        "daily_limit_usd": 20,
                        "daily_usage_usd": 22,
                        "weekly_window_start": "2026-08-06T00:00:00+08:00"
                    },
                    "daily_usage": [{ "date": "2026-08-09" }]
                }),
                0.5,
            ),
            (
                serde_json::json!({
                    "mode": "unrestricted",
                    "remaining": 0,
                    "subscription": {
                        "daily_limit_usd": 300,
                        "daily_usage_usd": 1
                    }
                }),
                0.0,
            ),
            (
                serde_json::json!({
                    "mode": "unrestricted",
                    "remaining": 0,
                    "subscription": { "daily_limit_usd": 300 }
                }),
                0.0,
            ),
            (
                serde_json::json!({
                    "mode": "unrestricted",
                    "remaining": 0,
                    "subscription": {
                        "daily_limit_usd": "invalid",
                        "daily_usage_usd": 0
                    }
                }),
                0.0,
            ),
            (
                serde_json::json!({
                    "mode": "unrestricted",
                    "remaining": 0,
                    "subscription": {
                        "daily_limit_usd": 0,
                        "daily_usage_usd": 0
                    }
                }),
                0.0,
            ),
            (
                serde_json::json!({
                    "mode": "unrestricted",
                    "remaining": 0,
                    "subscription": {
                        "daily_limit_usd": "NaN",
                        "daily_usage_usd": 300,
                        "weekly_window_start": "2026-08-06T00:00:00+08:00"
                    },
                    "daily_usage": [{ "date": "2026-08-09" }]
                }),
                0.0,
            ),
            (
                serde_json::json!({
                    "mode": "unrestricted",
                    "remaining": 0,
                    "subscription": {
                        "daily_limit_usd": 300,
                        "daily_usage_usd": "NaN",
                        "weekly_window_start": "2026-08-06T00:00:00+08:00"
                    },
                    "daily_usage": [{ "date": "2026-08-09" }]
                }),
                0.0,
            ),
        ] {
            let result = normalize_general(fixture);
            assert!(result.is_valid);
            assert_eq!(result.remaining, Some(expected_remaining));
            assert_eq!(result.used, None);
            assert_eq!(result.total, None);
        }
    }

    #[test]
    fn general_v1_covers_subscription_wallet_and_legacy_shapes() {
        let subscription = normalize_general(serde_json::json!({
            "mode": "unrestricted",
            "subscription": {
                "daily_limit_usd": 20,
                "daily_usage_usd": 2,
                "weekly_limit_usd": 50,
                "weekly_usage_usd": 49,
                "monthly_limit_usd": -1,
                "monthly_usage_usd": 100
            }
        }));
        assert_eq!(subscription.remaining, Some(1.0));

        let monthly_minimum = normalize_general(serde_json::json!({
            "mode": "unrestricted",
            "subscription": {
                "daily_limit_usd": 20,
                "daily_usage_usd": 2,
                "weekly_limit_usd": 50,
                "weekly_usage_usd": 10,
                "monthly_limit_usd": 100,
                "monthly_usage_usd": 99.75
            }
        }));
        assert_eq!(monthly_minimum.remaining, Some(0.25));

        let official_remaining = normalize_general(serde_json::json!({
            "mode": "unrestricted",
            "remaining": 0.5,
            "subscription": {
                "daily_limit_usd": 20,
                "daily_usage_usd": 2
            }
        }));
        assert_eq!(official_remaining.remaining, Some(0.5));

        let wallet = normalize_general(serde_json::json!({
            "mode": "unrestricted",
            "balance": -2
        }));
        assert_eq!(wallet.remaining, Some(0.0));

        let wallet_remaining = normalize_general(serde_json::json!({
            "mode": "unrestricted",
            "remaining": "4.5"
        }));
        assert_eq!(wallet_remaining.remaining, Some(4.5));

        for (fixture, expected, expected_plan) in [
            (serde_json::json!({ "remaining": "9" }), 9.0, None),
            (
                serde_json::json!({
                    "quota": { "remaining": 8, "plan_name": "Legacy quota" }
                }),
                8.0,
                Some("Legacy quota"),
            ),
            (serde_json::json!({ "balance": -2 }), 0.0, None),
            (
                serde_json::json!({ "total": "10", "used": "3.5" }),
                6.5,
                None,
            ),
        ] {
            let result = normalize_general(fixture);
            assert_eq!(result.remaining, Some(expected));
            assert_eq!(result.plan_name.as_deref(), expected_plan);
        }
    }

    #[test]
    fn general_v1_unknown_mode_and_missing_candidates_are_invalid() {
        for fixture in [
            serde_json::json!({ "mode": "future", "remaining": 10 }),
            serde_json::json!({ "mode": true, "remaining": 10 }),
            serde_json::json!({ "mode": null, "remaining": 10 }),
            serde_json::json!({ "mode": "quota_limited", "status": "active" }),
            serde_json::json!({ "remaining": "not-a-number" }),
        ] {
            let result = normalize_general(fixture);
            assert!(!result.is_valid);
            assert_eq!(result.remaining, None);
        }
    }

    #[test]
    fn general_v1_builds_both_usage_url_forms_natively() {
        for (base_url, expected) in [
            ("https://example.test", "https://example.test/v1/usage"),
            ("https://example.test/v1", "https://example.test/v1/usage"),
            (
                "https://example.test/v1/responses",
                "https://example.test/v1/usage",
            ),
        ] {
            let query = PreparedGeneralBalanceQuery::prepare(&key(), &base(base_url))
                .expect("prepared native query");
            let request = query.build_request();
            assert_eq!(request.url.as_str(), expected);
            assert_eq!(request.method, Method::GET);
            assert_eq!(
                request.headers.get(axum::http::header::AUTHORIZATION),
                Some(&HeaderValue::from_static("Bearer route-secret"))
            );
            assert_eq!(
                request.headers.get(axum::http::header::ACCEPT),
                Some(&HeaderValue::from_static("application/json"))
            );
            assert!(request.body.is_none());
        }
    }

    #[tokio::test]
    async fn balance_script_http_uses_exact_key_and_retries_5xx_once() {
        let state = mock_state(
            vec![StatusCode::INTERNAL_SERVER_ERROR, StatusCode::OK],
            &serde_json::json!({"remaining": 12}),
        );
        let calls = Arc::clone(&state.calls);
        let headers = Arc::clone(&state.request_headers);
        let server = MockBalanceServer::start(state).await;
        let source = query_source(&format!("http://{}/usage", server.address));
        let executor =
            BalanceExecutor::with_timing(Duration::from_millis(200), Duration::from_millis(1))
                .expect("balance executor");

        let result = executor
            .query(
                &custom_query(source),
                &ApiKey::parse("exact-route-key").expect("key"),
                &base("https://unused.test"),
            )
            .await
            .expect("retried balance query");

        assert_eq!(result.remaining, Some(12.0));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        for request_headers in headers.lock().expect("request header mutex").iter() {
            assert_eq!(
                request_headers.get("authorization"),
                Some(&HeaderValue::from_static("Bearer exact-route-key"))
            );
            assert_eq!(
                request_headers.get("accept-encoding"),
                Some(&HeaderValue::from_static("identity"))
            );
        }
        server.shutdown().await;
    }

    #[tokio::test]
    async fn balance_script_http_retries_429_but_not_other_4xx() {
        let retry_state = mock_state(
            vec![StatusCode::TOO_MANY_REQUESTS, StatusCode::OK],
            &serde_json::json!({"remaining": 5}),
        );
        let retry_calls = Arc::clone(&retry_state.calls);
        let retry_server = MockBalanceServer::start(retry_state).await;
        let retry_source = query_source(&format!("http://{}/usage", retry_server.address));
        let executor =
            BalanceExecutor::with_timing(Duration::from_millis(200), Duration::from_millis(1))
                .expect("balance executor");
        assert!(
            executor
                .query(
                    &custom_query(retry_source),
                    &key(),
                    &base("https://unused.test")
                )
                .await
                .is_ok()
        );
        assert_eq!(retry_calls.load(Ordering::SeqCst), 2);
        retry_server.shutdown().await;

        let deterministic_state = mock_state(
            vec![StatusCode::UNAUTHORIZED, StatusCode::OK],
            &serde_json::json!({"remaining": 5}),
        );
        let deterministic_calls = Arc::clone(&deterministic_state.calls);
        let deterministic_server = MockBalanceServer::start(deterministic_state).await;
        let deterministic_source =
            query_source(&format!("http://{}/usage", deterministic_server.address));
        let error = executor
            .query(
                &custom_query(deterministic_source),
                &key(),
                &base("https://unused.test"),
            )
            .await
            .err()
            .expect("deterministic 4xx");
        assert!(!error.transient);
        assert_eq!(deterministic_calls.load(Ordering::SeqCst), 1);
        deterministic_server.shutdown().await;
    }

    #[tokio::test]
    async fn balance_script_http_retries_timeout_once_and_bounds_response() {
        let mut timeout_state =
            mock_state(vec![StatusCode::OK], &serde_json::json!({"remaining": 5}));
        timeout_state.delay = Duration::from_millis(40);
        let timeout_calls = Arc::clone(&timeout_state.calls);
        let timeout_server = MockBalanceServer::start(timeout_state).await;
        let timeout_source = query_source(&format!("http://{}/usage", timeout_server.address));
        let timeout_executor =
            BalanceExecutor::with_timing(Duration::from_millis(10), Duration::from_millis(1))
                .expect("timeout executor");
        let timeout = timeout_executor
            .query(
                &custom_query(timeout_source),
                &key(),
                &base("https://unused.test"),
            )
            .await
            .err()
            .expect("timeout");
        assert!(timeout.transient);
        assert_eq!(timeout_calls.load(Ordering::SeqCst), 2);
        timeout_server.shutdown().await;

        let large_state = MockBalanceState {
            calls: Arc::new(AtomicUsize::new(0)),
            statuses: Arc::new(vec![StatusCode::OK]),
            response: Bytes::from(vec![b'x'; MAX_BALANCE_RESPONSE_BYTES + 1]),
            delay: Duration::ZERO,
            request_headers: Arc::new(Mutex::new(Vec::new())),
        };
        let large_calls = Arc::clone(&large_state.calls);
        let large_server = MockBalanceServer::start(large_state).await;
        let large_source = query_source(&format!("http://{}/usage", large_server.address));
        let error = BalanceExecutor::with_timing(Duration::from_secs(1), Duration::from_millis(1))
            .expect("balance executor")
            .query(
                &custom_query(large_source),
                &key(),
                &base("https://unused.test"),
            )
            .await
            .err()
            .expect("response limit");
        assert_eq!(error.category, BalanceErrorCategory::ResponseTooLarge);
        assert!(!error.transient);
        assert_eq!(large_calls.load(Ordering::SeqCst), 1);
        large_server.shutdown().await;
    }

    #[test]
    fn balance_security_substitution_is_escaped_and_non_recursive() {
        let key = ApiKey::from_stored(b"quote\" slash\\ control\n {{baseUrl}}".to_vec());
        let prepared = PreparedBalanceScript::prepare(
            r#"(() => ({ request: { url: "https://example.test", method: "GET", headers: { Authorization: "{{apiKey}}", Base: "{{baseUrl}}" } }, extractor: () => ({ isValid: true, remaining: 1 }) }))()"#,
            &key,
            &base("https://base.test/v1"),
        )
        .expect("prepared script");
        assert!(
            prepared
                .source
                .contains(r#"quote\" slash\\ control\n {{baseUrl}}"#)
        );
        assert!(prepared.source.contains("https://base.test/v1"));
        assert_eq!(prepared.source.matches("https://base.test/v1").count(), 1);
    }

    #[test]
    fn balance_security_rejects_placeholders_outside_plain_double_strings() {
        for source in [
            "const value = {{apiKey}};",
            "const value = '{{apiKey}}';",
            "const value = `{{apiKey}}`;",
            "// {{apiKey}}",
            "/* {{baseUrl}} */",
            r#"const value = "{{unknown}}";"#,
        ] {
            let error =
                PreparedBalanceScript::prepare(source, &key(), &base("https://example.test"))
                    .err()
                    .expect("invalid placeholder");
            assert_eq!(error.category, BalanceErrorCategory::InvalidPlaceholder);
        }
    }

    #[test]
    fn balance_security_enforces_source_and_substitution_limits() {
        let oversized_source = "x".repeat(MAX_BALANCE_SCRIPT_BYTES + 1);
        assert_eq!(
            PreparedBalanceScript::prepare(
                &oversized_source,
                &key(),
                &base("https://example.test"),
            )
            .err()
            .expect("source limit")
            .category,
            BalanceErrorCategory::SourceTooLarge
        );

        let large_key =
            ApiKey::parse(&"k".repeat(crate::domain::MAX_API_KEY_BYTES)).expect("maximum API key");
        let repeated = r#"const value = "{{apiKey}}";"#.repeat(140);
        assert_eq!(
            PreparedBalanceScript::prepare(&repeated, &large_key, &base("https://example.test"),)
                .err()
                .expect("substitution limit")
                .category,
            BalanceErrorCategory::SourceTooLarge
        );
    }

    #[test]
    fn balance_security_validates_request_url_method_headers_and_body_limits() {
        for request in [
            serde_json::json!({"url": "ftp://example.test", "method": "GET"}),
            serde_json::json!({"url": "https://example.test", "method": "CONNECT"}),
            serde_json::json!({"url": "https://example.test", "method": "TRACE"}),
            serde_json::json!({
                "url": "https://example.test",
                "method": "GET",
                "headers": (0..65)
                    .map(|index| (format!("x-{index}"), "v"))
                    .collect::<BTreeMap<_, _>>()
            }),
            serde_json::json!({
                "url": "https://example.test",
                "method": "POST",
                "body": "x".repeat(MAX_BALANCE_BODY_BYTES + 1)
            }),
        ] {
            assert!(validate_http_request(&request.to_string()).is_err());
        }
    }

    #[tokio::test]
    async fn balance_security_interrupts_dead_loops_and_recursion() {
        let dead_loop = PreparedBalanceScript::prepare(
            "(() => { while (true) {} })()",
            &key(),
            &base("https://example.test"),
        )
        .expect("prepared dead loop");
        let error = dead_loop
            .build_request(Duration::from_millis(20))
            .await
            .err()
            .expect("dead loop interrupted");
        assert_eq!(error.stage, BalanceErrorStage::RequestScript);
        assert!(matches!(
            error.category,
            BalanceErrorCategory::ScriptInterrupted | BalanceErrorCategory::ScriptFailed
        ));

        let recursion = PreparedBalanceScript::prepare(
            "(() => { const recurse = () => recurse(); return recurse(); })()",
            &key(),
            &base("https://example.test"),
        )
        .expect("prepared recursion");
        assert!(
            recursion
                .build_request(Duration::from_millis(50))
                .await
                .is_err()
        );

        let heap = PreparedBalanceScript::prepare(
            "(() => { const value = new ArrayBuffer(33554432); return value; })()",
            &key(),
            &base("https://example.test"),
        )
        .expect("prepared heap exhaustion");
        assert!(
            heap.build_request(Duration::from_millis(100))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn balance_security_phases_are_isolated_and_have_no_fetch_host_function() {
        let source = r#"(() => {
          globalThis.phaseCounter = (globalThis.phaseCounter || 0) + 1;
          return {
            request: { url: "https://example.test", method: "GET", headers: {} },
            extractor: () => ({
              isValid: globalThis.phaseCounter === 1 && typeof fetch === "undefined",
              remaining: 1,
              invalidMessage: "runtime leaked"
            })
          };
        })()"#;
        let script = PreparedBalanceScript::prepare(source, &key(), &base("https://example.test"))
            .expect("prepared script");
        script
            .build_request(Duration::from_secs(1))
            .await
            .expect("request phase");
        let result = script
            .extract(b"{}", Duration::from_secs(1))
            .await
            .expect("extractor phase");
        assert!(result.is_valid);
    }

    #[tokio::test]
    async fn balance_security_rejects_oversized_response_and_invalid_result_fields() {
        let source = r#"(() => ({
          request: { url: "https://example.test", method: "GET", headers: {} },
          extractor: (response) => response
        }))()"#;
        let script = PreparedBalanceScript::prepare(source, &key(), &base("https://example.test"))
            .expect("prepared script");
        assert_eq!(
            script
                .extract(
                    &vec![b' '; MAX_BALANCE_RESPONSE_BYTES + 1],
                    Duration::from_secs(1)
                )
                .await
                .err()
                .expect("response limit")
                .category,
            BalanceErrorCategory::ResponseTooLarge
        );
        for invalid in [
            serde_json::json!({"isValid": true, "remaining": null}),
            serde_json::json!({"isValid": true, "remaining": 1, "used": -1}),
            serde_json::json!({"isValid": false, "invalidMessage": ""}),
            serde_json::json!({"isValid": true, "remaining": 1, "unit": "x".repeat(33)}),
            serde_json::json!({"isValid": true, "remaining": 1, "extra": "x".repeat(MAX_EXTRA_BYTES + 1)}),
        ] {
            let response = serde_json::to_vec(&invalid).expect("invalid result JSON");
            assert!(
                script
                    .extract(&response, Duration::from_secs(1))
                    .await
                    .is_err()
            );
        }
    }
}
