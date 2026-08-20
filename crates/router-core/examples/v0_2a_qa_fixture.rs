use std::{
    collections::HashSet,
    error::Error,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use futures_util::StreamExt;
use reqwest::Url;
use router_core::{
    balance::BalanceQueryMode,
    codex_config::load_or_create_gateway_token,
    domain::{
        ApiKey, BalanceQueryPolicy, CompletionState, DeliveryState, RouteId, ServiceTierPolicy,
        UpstreamAttemptId,
    },
    qa_acceptance::{QA_APP_IDENTIFIER, QaAcceptanceRoot},
    storage::{
        AttemptHistoryRecord, BalanceQueryInput, CreateRouteInput, DatabaseExecutor,
        RequestHistoryRecord, SCHEMA_VERSION,
    },
};
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};

const BALANCE_SCRIPT: &str = r#"(() => ({
  request: {
    url: "{{baseUrl}}/usage",
    method: "GET",
    headers: { Authorization: "Bearer {{apiKey}}" }
  },
  extractor: (response) => ({
    isValid: true,
    remaining: response.remaining,
    unit: response.unit
  })
}))()"#;
const SYNTHETIC_USAGE_COUNT: usize = 320;
const QA_APPLICATION_SUPPORT_DIRECTORY: &str = "com.relax.airouter.qa";
const SYNTHETIC_MODELS: [&str; 8] = [
    "gpt-5.6-sol",
    "gpt-5.6-terra",
    "gpt-5.4-mini",
    "gpt-5.2-codex",
    "gpt-4.1",
    "o4-mini",
    "codex-mini-latest",
    "relay-synthetic-preview",
];
const SYNTHETIC_REASONING_EFFORTS: [Option<&str>; 6] = [
    Some("low"),
    Some("medium"),
    Some("high"),
    Some("xhigh"),
    Some("max"),
    None,
];

struct SyntheticTokens {
    input: Option<i64>,
    output: Option<i64>,
    total: Option<i64>,
    cached_input: Option<i64>,
    cache_write_input: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FixtureManifest {
    schema_version: u8,
    nonce: String,
    controller_url: String,
    routes: Vec<FixtureRoute>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FixtureRoute {
    label: String,
    base_url: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SeedRouteSummary {
    label: String,
    route_id: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SeedSummary {
    schema_version: u8,
    route_count: usize,
    request_count: usize,
    active_route_label: String,
    fallback_enabled: bool,
    routes: Vec<SeedRouteSummary>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UsageSeedSummary {
    schema_version: u8,
    added_request_count: u64,
    total_request_count: u64,
    route_count: usize,
    route_source: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct InspectionSummary {
    schema_version: u8,
    integrity: String,
    database_schema_version: i64,
    route_count: i64,
    request_count: i64,
    attempt_count: i64,
    codex_baseline_count: i64,
    active_route_label: Option<String>,
    fallback_enabled: bool,
    menu_debounce_seconds: u16,
    automatic_refresh_minutes: u16,
    proxy_port: u16,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RequestSummary {
    schema_version: u8,
    status: u16,
    elapsed_ms: u128,
    received_bytes: usize,
    cancelled: bool,
}

fn required_option(arguments: &[String], name: &str) -> Result<String, Box<dyn Error>> {
    let index = arguments
        .iter()
        .position(|argument| argument == name)
        .ok_or_else(|| format!("missing required option {name}"))?;
    arguments
        .get(index + 1)
        .cloned()
        .ok_or_else(|| format!("missing value for {name}").into())
}

fn optional_u64(arguments: &[String], name: &str) -> Result<Option<u64>, Box<dyn Error>> {
    let Some(index) = arguments.iter().position(|argument| argument == name) else {
        return Ok(None);
    };
    let value = arguments
        .get(index + 1)
        .ok_or_else(|| format!("missing value for {name}"))?
        .parse::<u64>()?;
    Ok(Some(value))
}

fn resolve_root(path: &str) -> Result<QaAcceptanceRoot, Box<dyn Error>> {
    QaAcceptanceRoot::resolve(
        QA_APP_IDENTIFIER,
        Some(OsString::from(path)),
        &std::env::temp_dir(),
    )?
    .ok_or_else(|| "QA acceptance root is required".into())
}

fn read_manifest(path: &Path, root: &QaAcceptanceRoot) -> Result<FixtureManifest, Box<dyn Error>> {
    let metadata = fs::symlink_metadata(path)?;
    let canonical = path.canonicalize()?;
    let expected = root.root().join("fixture-manifest.json").canonicalize()?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || canonical != expected {
        return Err("fixture manifest must be the regular run-root manifest".into());
    }
    let manifest: FixtureManifest = serde_json::from_slice(&fs::read(canonical)?)?;
    validate_manifest(&manifest, root)?;
    Ok(manifest)
}

fn validate_manifest(
    manifest: &FixtureManifest,
    root: &QaAcceptanceRoot,
) -> Result<(), Box<dyn Error>> {
    if manifest.schema_version != 1 || manifest.nonce != root.nonce() {
        return Err("fixture manifest identity mismatch".into());
    }
    let controller = validate_loopback_url(&manifest.controller_url, "controller URL")?;
    let controller_port = controller
        .port()
        .ok_or("fixture controller URL must use an explicit loopback port")?;
    if controller.path() != "/" {
        return Err("fixture controller URL must contain only an explicit loopback port".into());
    }
    if manifest.routes.len() != 4 {
        return Err("fixture manifest must contain exactly four routes".into());
    }
    let mut labels = HashSet::new();
    let mut route_ports = HashSet::new();
    for route in &manifest.routes {
        if !matches!(route.label.as_str(), "A" | "B" | "C" | "D")
            || !labels.insert(route.label.as_str())
        {
            return Err("fixture route labels must be unique A through D".into());
        }
        let url = validate_loopback_url(&route.base_url, "route URL")?;
        let port = url
            .port()
            .ok_or("fixture route URL must use an explicit loopback port")?;
        if port == controller_port || !route_ports.insert(port) {
            return Err("fixture routes and controller must use distinct loopback ports".into());
        }
        let expected_suffix = "/v1";
        if url.path() != expected_suffix {
            return Err("fixture route path does not match its label".into());
        }
    }
    if labels.len() != 4 {
        return Err("fixture route labels must include A through D".into());
    }
    Ok(())
}

fn validate_loopback_url(value: &str, label: &str) -> Result<Url, Box<dyn Error>> {
    let url = Url::parse(value)?;
    if url.scheme() != "http"
        || url.host_str() != Some("127.0.0.1")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(format!("{label} must be a credential-free IPv4 loopback HTTP URL").into());
    }
    Ok(url)
}

async fn seed(root: &QaAcceptanceRoot, manifest_path: &Path) -> Result<(), Box<dyn Error>> {
    let manifest = read_manifest(manifest_path, root)?;
    let database_path = root.database_path();
    if database_path.try_exists()? {
        return Err("refusing to seed an existing QA database".into());
    }
    let database = DatabaseExecutor::open(database_path)?;

    let mut seeded_routes = Vec::with_capacity(manifest.routes.len());
    for route in manifest.routes {
        let label = route.label;
        let created = database
            .create_route(CreateRouteInput {
                name: format!("Synthetic {label}"),
                base_url: route.base_url,
                api_key: ApiKey::parse(&format!("qa-synthetic-route-{label}"))?,
                service_tier_policy: ServiceTierPolicy::Passthrough,
                balance_query: (label == "A").then(|| BalanceQueryInput {
                    mode: BalanceQueryMode::CustomJs,
                    enabled: false,
                    custom_source: BALANCE_SCRIPT.to_owned(),
                }),
                accept_script_risk: false,
            })
            .await?;
        seeded_routes.push((format!("Synthetic {label}"), created.route_id));
    }
    let fallback = database.set_fallback_enabled(true).await?;
    database
        .set_balance_query_policy(BalanceQueryPolicy::default())
        .await?;
    let _gateway_token = load_or_create_gateway_token(&database).await?;
    seed_usage_history(&database, &seeded_routes, "fixture").await?;
    let routes = seeded_routes
        .into_iter()
        .map(|(name, route_id)| SeedRouteSummary {
            label: name.strip_prefix("Synthetic ").unwrap_or(&name).to_owned(),
            route_id: route_id.as_str().to_owned(),
        })
        .collect::<Vec<_>>();
    let summary = SeedSummary {
        schema_version: 1,
        route_count: routes.len(),
        request_count: SYNTHETIC_USAGE_COUNT,
        active_route_label: "A".to_owned(),
        fallback_enabled: fallback.enabled,
        routes,
    };
    println!("{}", serde_json::to_string_pretty(&summary)?);
    Ok(())
}

async fn seed_usage_history(
    database: &DatabaseExecutor,
    routes: &[(String, RouteId)],
    batch_id: &str,
) -> Result<(), Box<dyn Error>> {
    let seeded_at_ms = i64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis())?;
    for index in 0..SYNTHETIC_USAGE_COUNT {
        let index_i64 = i64::try_from(index)?;
        let (route_name, route_id) = &routes[index % routes.len()];
        let started_at_ms = seeded_at_ms.saturating_sub(index_i64.saturating_mul(1_200_000));
        let total_latency_ms = match index % 40 {
            10 => 65_230,
            20 => 3_720_000,
            _ => 650 + i64::try_from(index % 18)? * 1_850,
        };
        let completion_state = match index % 16 {
            0 => CompletionState::NoUpstream,
            1 => CompletionState::Cancelled,
            2 | 3 => CompletionState::Failed,
            _ => CompletionState::Completed,
        };
        let completed = completion_state == CompletionState::Completed;
        let has_attempt = completion_state != CompletionState::NoUpstream;
        let streaming = index.is_multiple_of(2);
        let model = SYNTHETIC_MODELS[index % SYNTHETIC_MODELS.len()];
        let SyntheticTokens {
            input: input_tokens,
            output: output_tokens,
            total: total_tokens,
            cached_input: cached_input_tokens,
            cache_write_input: cache_write_input_tokens,
        } = synthetic_tokens(index, completed)?;
        let service_tier = synthetic_service_tier(completed, model);
        let (http_status, error_category, delivery_state) =
            synthetic_status_metadata(index, &completion_state);
        let first_output_latency_ms =
            synthetic_first_output_latency(index, completed, streaming, total_latency_ms)?;
        let attempts = has_attempt
            .then(|| AttemptHistoryRecord {
                attempt_id: UpstreamAttemptId::from_string(format!(
                    "qa-attempt-{batch_id}-{index:04}"
                )),
                attempt_role: router_core::storage::AttemptRole::Ordinary,
                attempt_index: 0,
                route_id: route_id.clone(),
                route_name: route_name.clone(),
                started_at_ms,
                finished_at_ms: started_at_ms.saturating_add(total_latency_ms),
                http_status,
                error_category: error_category.clone(),
                delivery_state,
                actual_model: completed.then(|| model.to_owned()),
                forwarded_service_tier: service_tier.map(str::to_owned),
                actual_service_tier: service_tier.map(str::to_owned),
                input_tokens,
                output_tokens,
                total_tokens,
                cached_input_tokens,
                cache_write_input_tokens,
            })
            .into_iter()
            .collect();
        database
            .record_request_history(RequestHistoryRecord {
                request_id: format!("qa-usage-{batch_id}-{index:04}"),
                started_at_ms,
                finished_at_ms: started_at_ms.saturating_add(total_latency_ms),
                turn_id: None,
                requested_model: Some(model.to_owned()),
                reasoning_effort: SYNTHETIC_REASONING_EFFORTS
                    [index % SYNTHETIC_REASONING_EFFORTS.len()]
                .map(str::to_owned),
                requested_service_tier: service_tier.map(str::to_owned),
                actual_model: completed.then(|| model.to_owned()),
                actual_service_tier: service_tier.map(str::to_owned),
                final_route_id: (completion_state != CompletionState::NoUpstream)
                    .then(|| route_id.clone()),
                final_route_name: (completion_state != CompletionState::NoUpstream)
                    .then(|| route_name.clone()),
                streaming,
                completion_state,
                http_status,
                error_category,
                input_tokens,
                output_tokens,
                total_tokens,
                cached_input_tokens,
                cache_write_input_tokens,
                total_latency_ms: Some(total_latency_ms),
                first_output_latency_ms,
                metadata_complete: completed,
                fallback_stop_reason: None,
                fallback_stop_target_route_id: None,
                fallback_stop_target_route_name: None,
                attempts,
            })
            .await?;
    }
    Ok(())
}

fn synthetic_service_tier(completed: bool, model: &str) -> Option<&'static str> {
    completed.then_some(if matches!(model, "gpt-5.6-sol" | "gpt-5.6-terra") {
        "priority"
    } else {
        "default"
    })
}

fn synthetic_first_output_latency(
    index: usize,
    completed: bool,
    streaming: bool,
    total_latency_ms: i64,
) -> Result<Option<i64>, Box<dyn Error>> {
    Ok((completed && streaming).then_some(
        if index.is_multiple_of(20) && total_latency_ms >= 65_000 {
            65_000
        } else if index.is_multiple_of(9) && total_latency_ms >= 12_500 {
            12_500
        } else {
            (350 + i64::try_from(index % 12)? * 420).min(total_latency_ms)
        },
    ))
}

fn synthetic_tokens(index: usize, completed: bool) -> Result<SyntheticTokens, Box<dyn Error>> {
    let input = completed.then_some(720 + i64::try_from(index % 37)? * 113);
    let output = completed
        .then_some(96 + i64::try_from(index % 19)? * 41)
        .filter(|_| !index.is_multiple_of(13));
    Ok(SyntheticTokens {
        input,
        output,
        total: input.zip(output).map(|(input, output)| input + output),
        cached_input: input.map(|value| {
            if index.is_multiple_of(3) {
                value / 4
            } else {
                0
            }
        }),
        cache_write_input: input.map(|_| if index.is_multiple_of(5) { 48 } else { 0 }),
    })
}

fn synthetic_status_metadata(
    index: usize,
    completion_state: &CompletionState,
) -> (Option<u16>, Option<String>, DeliveryState) {
    match completion_state {
        CompletionState::Completed => (Some(200), None, DeliveryState::Completed),
        CompletionState::Failed if index.is_multiple_of(2) => (
            Some(502),
            Some("upstream_request_failed".to_owned()),
            DeliveryState::None,
        ),
        CompletionState::Failed => (
            Some(429),
            Some("upstream_rate_limited".to_owned()),
            DeliveryState::None,
        ),
        CompletionState::Cancelled => (
            None,
            Some("downstream_cancelled".to_owned()),
            DeliveryState::Started,
        ),
        CompletionState::NoUpstream => (None, None, DeliveryState::None),
    }
}

fn expected_qa_database_path(home: &Path) -> PathBuf {
    home.join("Library")
        .join("Application Support")
        .join(QA_APPLICATION_SUPPORT_DIRECTORY)
        .join("router.sqlite3")
}

fn validate_qa_usage_database_path(
    candidate: &Path,
    home: &Path,
) -> Result<PathBuf, Box<dyn Error>> {
    if !home.is_absolute() || home.canonicalize()? != home {
        return Err("QA Usage seed requires a canonical absolute home directory".into());
    }
    let expected = expected_qa_database_path(home);
    if candidate != expected {
        return Err("QA Usage seed database path does not match the exact QA path".into());
    }

    let mut directory = home.to_path_buf();
    for component in [
        "Library",
        "Application Support",
        QA_APPLICATION_SUPPORT_DIRECTORY,
    ] {
        directory.push(component);
        let metadata = fs::symlink_metadata(&directory)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err("QA Usage seed database parent must be a regular directory".into());
        }
    }
    let metadata = fs::symlink_metadata(candidate)?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || candidate.canonicalize()? != expected
    {
        return Err("QA Usage seed database must be the regular exact QA database file".into());
    }

    let connection = Connection::open_with_flags(
        candidate,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let schema_version: i64 =
        connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if schema_version != SCHEMA_VERSION {
        return Err("QA Usage seed requires the current QA database schema".into());
    }
    Ok(expected)
}

fn resolve_live_qa_usage_database_path() -> Result<PathBuf, Box<dyn Error>> {
    let home = std::env::var_os("HOME").ok_or("HOME is required for QA Usage seed")?;
    let home = PathBuf::from(home);
    validate_qa_usage_database_path(&expected_qa_database_path(&home), &home)
}

fn retained_usage_routes() -> Vec<(String, RouteId)> {
    ["A", "B", "C", "D"]
        .into_iter()
        .map(|label| {
            (
                format!("QA Historical Route {label}"),
                RouteId::from_string(format!("qa-retained-usage-{label}")),
            )
        })
        .collect()
}

async fn append_usage_history(database_path: &Path) -> Result<UsageSeedSummary, Box<dyn Error>> {
    let database = DatabaseExecutor::open(database_path)?;
    let configured_routes = database.list_routes().await?;
    let has_configured_routes = !configured_routes.is_empty();
    let routes = if has_configured_routes {
        configured_routes
            .into_iter()
            .map(|route| (route.name, route.route_id))
            .collect::<Vec<_>>()
    } else {
        retained_usage_routes()
    };
    let before = database.history_summary().await?.request_count;
    let batch_id = UpstreamAttemptId::new();
    seed_usage_history(&database, &routes, batch_id.as_str()).await?;
    let total = database.history_summary().await?.request_count;
    let added = total
        .checked_sub(before)
        .ok_or("QA Usage seed request count regressed")?;
    if added != SYNTHETIC_USAGE_COUNT as u64 {
        return Err("QA Usage seed did not append the complete synthetic batch".into());
    }
    Ok(UsageSeedSummary {
        schema_version: 1,
        added_request_count: added,
        total_request_count: total,
        route_count: routes.len(),
        route_source: if has_configured_routes {
            "configured"
        } else {
            "retained_history"
        },
    })
}

async fn usage_seed() -> Result<(), Box<dyn Error>> {
    let database_path = resolve_live_qa_usage_database_path()?;
    let summary = append_usage_history(&database_path).await?;
    println!("{}", serde_json::to_string_pretty(&summary)?);
    Ok(())
}

async fn inspect(root: &QaAcceptanceRoot) -> Result<(), Box<dyn Error>> {
    let database_path = root.database_path();
    let database = DatabaseExecutor::open(&database_path)?;
    let routes = database.list_routes().await?;
    let routing = database.routing_state().await?;
    let settings = database.app_settings().await?;
    let active_route_label = routing
        .active_route_id
        .as_ref()
        .map(|active| {
            routes
                .iter()
                .find(|route| &route.route_id == active)
                .and_then(|route| route.name.strip_prefix("Synthetic "))
                .filter(|label| matches!(*label, "A" | "B" | "C" | "D"))
                .map(str::to_owned)
                .ok_or("active route is not one of the synthetic fixture routes")
        })
        .transpose()?;

    let connection = Connection::open_with_flags(
        database_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let integrity = connection.pragma_query_value(None, "integrity_check", |row| row.get(0))?;
    let database_schema_version =
        connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let route_count = count(&connection, "routes")?;
    let request_count = count(&connection, "proxy_requests")?;
    let attempt_count = count(&connection, "upstream_attempts")?;
    let codex_baseline_count = count(&connection, "codex_baseline")?;
    let summary = InspectionSummary {
        schema_version: 1,
        integrity,
        database_schema_version,
        route_count,
        request_count,
        attempt_count,
        codex_baseline_count,
        active_route_label,
        fallback_enabled: routing.fallback.enabled,
        menu_debounce_seconds: settings.balance_query_policy.menu_debounce_seconds(),
        automatic_refresh_minutes: settings.balance_query_policy.automatic_refresh_minutes(),
        proxy_port: settings.proxy_port,
    };
    println!("{}", serde_json::to_string_pretty(&summary)?);
    Ok(())
}

fn count(connection: &Connection, table: &str) -> Result<i64, Box<dyn Error>> {
    if !matches!(
        table,
        "routes" | "proxy_requests" | "upstream_attempts" | "codex_baseline"
    ) {
        return Err("non-allowlisted inspection table".into());
    }
    Ok(
        connection.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })?,
    )
}

async fn request(
    root: &QaAcceptanceRoot,
    stream: bool,
    cancel_after_ms: Option<u64>,
    timeout_ms: u64,
) -> Result<(), Box<dyn Error>> {
    if !(100..=360_000).contains(&timeout_ms) {
        return Err("request timeout must be between 100 and 360000 ms".into());
    }
    let database = DatabaseExecutor::open(root.database_path())?;
    let settings = database.app_settings().await?;
    let token = load_or_create_gateway_token(&database).await?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(timeout_ms))
        .build()?;
    let started = Instant::now();
    let response = client
        .post(format!(
            "http://127.0.0.1:{}/v1/responses",
            settings.proxy_port
        ))
        .bearer_auth(token)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(serde_json::to_vec(&serde_json::json!({
            "model": "qa-synthetic-model",
            "input": "qa-synthetic-input",
            "stream": stream,
        }))?)
        .send()
        .await?;
    let status = response.status().as_u16();
    let mut bytes = 0_usize;
    let mut body = response.bytes_stream();
    let cancelled = if let Some(cancel_after_ms) = cancel_after_ms {
        let cancellation = tokio::time::sleep(Duration::from_millis(cancel_after_ms));
        tokio::pin!(cancellation);
        loop {
            tokio::select! {
                () = &mut cancellation => break true,
                next = body.next() => match next {
                    Some(chunk) => bytes = bytes.saturating_add(chunk?.len()),
                    None => break false,
                }
            }
        }
    } else {
        while let Some(chunk) = body.next().await {
            bytes = bytes.saturating_add(chunk?.len());
        }
        false
    };
    let summary = RequestSummary {
        schema_version: 1,
        status,
        elapsed_ms: started.elapsed().as_millis(),
        received_bytes: bytes,
        cancelled,
    };
    println!("{}", serde_json::to_string_pretty(&summary)?);
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    let command = arguments
        .first()
        .ok_or("usage: v0_2a_qa_fixture <seed|request|inspect|usage-seed> [--root PATH]")?;
    if command == "usage-seed" {
        return usage_seed().await;
    }
    let root = resolve_root(&required_option(&arguments, "--root")?)?;
    match command.as_str() {
        "seed" => {
            let manifest = PathBuf::from(required_option(&arguments, "--manifest")?);
            seed(&root, &manifest).await
        }
        "inspect" => inspect(&root).await,
        "request" => {
            let stream = arguments.iter().any(|argument| argument == "--stream");
            let cancel_after_ms = optional_u64(&arguments, "--cancel-after-ms")?;
            let timeout_ms = optional_u64(&arguments, "--timeout-ms")?.unwrap_or(90_000);
            request(&root, stream, cancel_after_ms, timeout_ms).await
        }
        _ => Err("usage: v0_2a_qa_fixture <seed|request|inspect|usage-seed> [--root PATH]".into()),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use router_core::qa_acceptance::{QA_ACCEPTANCE_MARKER_FILE, QA_ACCEPTANCE_ROOT_PREFIX};
    use tempfile::TempDir;

    use super::*;

    fn root(temporary: &TempDir, nonce: &str) -> QaAcceptanceRoot {
        let root = temporary
            .path()
            .join(format!("{QA_ACCEPTANCE_ROOT_PREFIX}{nonce}"));
        fs::create_dir(&root).expect("root");
        fs::write(root.join(QA_ACCEPTANCE_MARKER_FILE), nonce).expect("marker");
        QaAcceptanceRoot::resolve(
            QA_APP_IDENTIFIER,
            Some(root.into_os_string()),
            temporary.path(),
        )
        .expect("valid root")
        .expect("override")
    }

    fn manifest(root: &QaAcceptanceRoot) -> FixtureManifest {
        FixtureManifest {
            schema_version: 1,
            nonce: root.nonce().to_owned(),
            controller_url: "http://127.0.0.1:12345".to_owned(),
            routes: [("A", 12346), ("B", 12347), ("C", 12348), ("D", 12349)]
                .into_iter()
                .map(|(label, port)| FixtureRoute {
                    label: label.to_owned(),
                    base_url: format!("http://127.0.0.1:{port}/v1"),
                })
                .collect(),
        }
    }

    fn qa_database(temporary: &TempDir) -> (PathBuf, PathBuf) {
        let home = temporary.path().canonicalize().expect("canonical home");
        let path = expected_qa_database_path(&home);
        fs::create_dir_all(path.parent().expect("database parent")).expect("QA support path");
        let database = DatabaseExecutor::open(path.clone()).expect("QA database");
        drop(database);
        (home, path)
    }

    #[test]
    fn manifest_requires_four_unique_loopback_routes_and_matching_nonce() {
        let temporary = TempDir::new().expect("temporary");
        let root = root(&temporary, "manifest");
        let mut fixture = manifest(&root);
        validate_manifest(&fixture, &root).expect("valid manifest");

        fixture.routes[0].base_url = "https://provider.example/v1".to_owned();
        assert!(validate_manifest(&fixture, &root).is_err());
    }

    #[test]
    fn usage_seed_path_accepts_only_the_regular_exact_qa_database() {
        let temporary = TempDir::new().expect("temporary");
        let (home, path) = qa_database(&temporary);
        assert_eq!(
            validate_qa_usage_database_path(&path, &home).expect("exact QA path"),
            path
        );

        let production = home.join("Library/Application Support/com.relax.airouter/router.sqlite3");
        assert!(validate_qa_usage_database_path(&production, &home).is_err());

        let connection = Connection::open(&path).expect("schema edit fixture");
        connection
            .pragma_update(None, "user_version", 6)
            .expect("old schema fixture");
        drop(connection);
        assert!(validate_qa_usage_database_path(&path, &home).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn usage_seed_path_rejects_a_symlinked_database() {
        use std::os::unix::fs::symlink;

        let temporary = TempDir::new().expect("temporary");
        let (home, path) = qa_database(&temporary);
        let target = home.join("other.sqlite3");
        fs::rename(&path, &target).expect("move database fixture");
        symlink(&target, &path).expect("database symlink fixture");
        assert!(validate_qa_usage_database_path(&path, &home).is_err());
    }

    #[tokio::test]
    async fn usage_seed_appends_unique_representative_history_without_routes() {
        let temporary = TempDir::new().expect("temporary");
        let (home, path) = qa_database(&temporary);
        validate_qa_usage_database_path(&path, &home).expect("validated QA database");

        let first = append_usage_history(&path).await.expect("first append");
        assert_eq!(first.added_request_count, SYNTHETIC_USAGE_COUNT as u64);
        assert_eq!(first.total_request_count, SYNTHETIC_USAGE_COUNT as u64);
        assert_eq!(first.route_count, 4);
        assert_eq!(first.route_source, "retained_history");

        let second = append_usage_history(&path).await.expect("second append");
        assert_eq!(second.added_request_count, SYNTHETIC_USAGE_COUNT as u64);
        assert_eq!(
            second.total_request_count,
            (SYNTHETIC_USAGE_COUNT * 2) as u64
        );

        let connection = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .expect("read appended database");
        assert_eq!(count(&connection, "routes").expect("route count"), 0);
        assert_eq!(
            count(&connection, "proxy_requests").expect("request count"),
            (SYNTHETIC_USAGE_COUNT * 2) as i64
        );
        let retained_names = {
            let mut statement = connection
                .prepare(
                    "SELECT DISTINCT final_route_name FROM proxy_requests
                     WHERE final_route_name IS NOT NULL",
                )
                .expect("retained name query");
            statement
                .query_map([], |row| row.get::<_, String>(0))
                .expect("retained names")
                .collect::<Result<HashSet<_>, _>>()
                .expect("retained name rows")
        };
        assert_eq!(
            retained_names,
            ["A", "B", "C", "D"]
                .into_iter()
                .map(|label| format!("QA Historical Route {label}"))
                .collect()
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(DISTINCT completion_state) FROM proxy_requests",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .expect("completion states"),
            4
        );
        for predicate in [
            "streaming = 0",
            "streaming = 1",
            "cached_input_tokens > 0",
            "cache_write_input_tokens > 0",
            "total_latency_ms > 60000",
            "total_latency_ms >= 3600000",
            "first_output_latency_ms IS NOT NULL",
            "pricing_catalog_version = 'openai-standard-2026-07-27'",
            "pricing_catalog_version = 'openai-priority-2026-07-28'",
        ] {
            let sql = format!("SELECT COUNT(*) FROM proxy_requests WHERE {predicate}");
            let count: i64 = connection
                .query_row(&sql, [], |row| row.get(0))
                .expect("representative row count");
            assert!(count > 0, "missing representative rows for {predicate}");
        }
    }

    #[tokio::test]
    async fn usage_seed_preserves_configured_route_names_verbatim() {
        let temporary = TempDir::new().expect("temporary");
        let (home, path) = qa_database(&temporary);
        let database = DatabaseExecutor::open(path.clone()).expect("QA database");
        database
            .create_route(CreateRouteInput {
                name: "Configured Route Name".to_owned(),
                base_url: "https://qa-provider.example/v1".to_owned(),
                api_key: ApiKey::parse("qa-configured-route-key").expect("QA key"),
                service_tier_policy: ServiceTierPolicy::Passthrough,
                balance_query: None,
                accept_script_risk: false,
            })
            .await
            .expect("configured route");
        drop(database);
        validate_qa_usage_database_path(&path, &home).expect("validated QA database");

        let summary = append_usage_history(&path)
            .await
            .expect("append Usage history");
        assert_eq!(summary.route_count, 1);
        assert_eq!(summary.route_source, "configured");

        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .expect("read appended database");
        let names: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM proxy_requests
                 WHERE final_route_name IS NOT NULL
                   AND final_route_name != 'Configured Route Name'",
                [],
                |row| row.get(0),
            )
            .expect("configured name count");
        assert_eq!(names, 0);
    }

    #[tokio::test]
    async fn seed_uses_storage_contracts_and_refuses_a_second_seed() {
        let temporary = TempDir::new().expect("temporary");
        let root = root(&temporary, "seed");
        let path = root.root().join("fixture-manifest.json");
        fs::write(
            &path,
            serde_json::to_vec(&manifest(&root)).expect("manifest JSON"),
        )
        .expect("manifest file");

        seed(&root, &path).await.expect("first seed");
        let database = DatabaseExecutor::open(root.database_path()).expect("database");
        assert_eq!(database.list_routes().await.expect("routes").len(), 4);
        assert_eq!(
            database
                .history_summary()
                .await
                .expect("history summary")
                .request_count,
            SYNTHETIC_USAGE_COUNT as u64
        );
        let first_page = database
            .usage_history(router_core::storage::UsageHistoryQuery {
                finished_at_or_after_ms: Some(0),
                finished_at_or_before_ms: i64::MAX,
                completion_state: None,
                route_id: None,
                model_contains: None,
                cursor: None,
                limit: 50,
            })
            .await
            .expect("first usage page");
        assert_eq!(first_page.total_rows, SYNTHETIC_USAGE_COUNT as u64);
        assert_eq!(first_page.rows.len(), 50);
        assert!(first_page.next_cursor.is_some());
        assert!(first_page.rows.iter().all(|row| {
            row.first_output_latency_ms
                .zip(row.total_latency_ms)
                .is_none_or(|(first_output, total)| first_output <= total)
        }));
        assert!(
            first_page
                .rows
                .iter()
                .filter_map(|row| row.total_latency_ms)
                .any(|latency| latency > 60_000)
        );
        assert!(
            first_page
                .rows
                .iter()
                .filter_map(|row| row.total_latency_ms)
                .any(|latency| latency >= 3_600_000)
        );
        assert_eq!(
            first_page
                .rows
                .iter()
                .filter_map(|row| row.final_route_id.as_ref())
                .collect::<HashSet<_>>()
                .len(),
            4
        );
        assert!(
            database
                .routing_state()
                .await
                .expect("routing")
                .fallback
                .enabled
        );
        assert!(seed(&root, &path).await.is_err());
    }
}
