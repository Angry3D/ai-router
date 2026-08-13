use std::{
    error::Error,
    ffi::OsString,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    time::Duration,
};

use router_core::{
    balance::BalanceQueryMode,
    domain::{
        ApiKey, BalanceQueryPolicy, CompletionState, DeliveryState, RouteId, ServiceTierPolicy,
        UpstreamAttemptId,
    },
    qa_acceptance::{QA_APP_IDENTIFIER, QA_RUNTIME_MARKER_FILE, QaAcceptanceRoot, QaRuntimeMarker},
    recovery::{DatabaseStartupClassification, MAX_VALID_POINTS, RecoveryManager, RecoveryPointId},
    storage::{
        AttemptHistoryRecord, BalanceQueryInput, CreateRouteInput, DatabaseExecutor,
        RequestHistoryRecord,
    },
};
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};

const SCHEMA_VERSION: u8 = 1;
const PERMIT_FILE: &str = "recovery-action-permit.json";
const SYNTHETIC_CODEX_CONFIG: &[u8] = b"model = \"qa-synthetic-model\"\n";
const SYNTHETIC_HISTORY_SENTINEL: &str = "qa-recovery-excluded-history";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ActionPermit {
    schema_version: u8,
    nonce: String,
    action: String,
    pid: u32,
    identifier: String,
    bundle_path: PathBuf,
    executable_path: PathBuf,
    app_data_dir: PathBuf,
    codex_home_dir: PathBuf,
    log_dir: PathBuf,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CandidateSummary {
    point_id: String,
    created_at_ms: i64,
    critical_revision: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RecoverySummary {
    schema_version: u8,
    operation: String,
    startup: String,
    health: Option<String>,
    valid_point_count: usize,
    invalid_point_count: usize,
    candidates: Vec<CandidateSummary>,
    route_count: Option<usize>,
    request_count: Option<u64>,
    attempt_count: Option<i64>,
    quarantine_count: usize,
    codex_config_unchanged: bool,
    retention_within_limit: bool,
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

fn optional_option(arguments: &[String], name: &str) -> Option<String> {
    arguments
        .iter()
        .position(|argument| argument == name)
        .and_then(|index| arguments.get(index + 1))
        .cloned()
}

fn resolve_root(path: &str) -> Result<QaAcceptanceRoot, Box<dyn Error>> {
    QaAcceptanceRoot::resolve(
        QA_APP_IDENTIFIER,
        Some(OsString::from(path)),
        &std::env::temp_dir(),
    )?
    .ok_or_else(|| "QA acceptance root is required".into())
}

fn canonical_qa_bundle() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("target/release/bundle/macos/AI Router QA.app")
}

fn validate_action_permit(
    root: &QaAcceptanceRoot,
    action: &str,
    expected_bundle: &Path,
) -> Result<ActionPermit, Box<dyn Error>> {
    let permit_path = root.root().join(PERMIT_FILE);
    let metadata = fs::symlink_metadata(&permit_path)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("recovery action permit must be a regular run-root file".into());
    }
    let permit: ActionPermit = serde_json::from_slice(&fs::read(&permit_path)?)?;
    let marker_path = root.root().join(QA_RUNTIME_MARKER_FILE);
    let marker_metadata = fs::symlink_metadata(&marker_path)?;
    if !marker_metadata.is_file() || marker_metadata.file_type().is_symlink() {
        return Err("QA runtime marker must be a regular run-root file".into());
    }
    let marker: QaRuntimeMarker = serde_json::from_slice(&fs::read(marker_path)?)?;
    let bundle = permit.bundle_path.canonicalize()?;
    let executable = permit.executable_path.canonicalize()?;
    let expected_bundle = expected_bundle.canonicalize()?;
    let expected_executable_root = expected_bundle.join("Contents/MacOS");
    if permit.schema_version != SCHEMA_VERSION
        || permit.nonce != root.nonce()
        || permit.action != action
        || permit.pid == 0
        || permit.identifier != QA_APP_IDENTIFIER
        || bundle != expected_bundle
        || !bundle.ends_with("AI Router QA.app")
        || !executable.starts_with(expected_executable_root)
        || permit.app_data_dir != root.app_data_dir()
        || permit.codex_home_dir != root.codex_home_dir()
        || permit.log_dir != root.log_dir()
        || marker.schema_version != SCHEMA_VERSION
        || marker.nonce != root.nonce()
        || marker.pid != permit.pid
        || marker.identifier != QA_APP_IDENTIFIER
        || marker.executable_path.canonicalize()? != executable
        || marker.app_data_dir != root.app_data_dir()
        || marker.codex_home_dir != root.codex_home_dir()
        || marker.log_dir != root.log_dir()
    {
        return Err("recovery action permit does not match the exact QA runtime".into());
    }
    fs::remove_file(permit_path)?;
    Ok(permit)
}

fn create_private_file(path: &Path, bytes: &[u8]) -> Result<(), Box<dyn Error>> {
    #[cfg(unix)]
    use std::os::unix::fs::OpenOptionsExt;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<(), Box<dyn Error>> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> Result<(), Box<dyn Error>> {
    Ok(())
}

fn ensure_private_regular_file(path: &Path) -> Result<(), Box<dyn Error>> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("recovery fixture target must be a regular file".into());
    }
    Ok(())
}

fn codex_snapshot(root: &QaAcceptanceRoot) -> Result<(Vec<u8>, Option<u32>), Box<dyn Error>> {
    let path = root.codex_home_dir().join("config.toml");
    ensure_private_regular_file(&path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        Ok((
            fs::read(&path)?,
            Some(fs::metadata(path)?.permissions().mode() & 0o777),
        ))
    }
    #[cfg(not(unix))]
    {
        Ok((fs::read(path)?, None))
    }
}

async fn record_synthetic_history(
    database: &DatabaseExecutor,
    route_id: RouteId,
) -> Result<(), Box<dyn Error>> {
    database
        .record_request_history(RequestHistoryRecord {
            request_id: "qa-recovery-request".to_owned(),
            started_at_ms: 10,
            finished_at_ms: 20,
            turn_id: Some(SYNTHETIC_HISTORY_SENTINEL.to_owned()),
            requested_model: Some(SYNTHETIC_HISTORY_SENTINEL.to_owned()),
            reasoning_effort: Some(SYNTHETIC_HISTORY_SENTINEL.to_owned()),
            requested_service_tier: None,
            actual_model: None,
            actual_service_tier: None,
            final_route_id: Some(route_id.clone()),
            final_route_name: Some(SYNTHETIC_HISTORY_SENTINEL.to_owned()),
            streaming: false,
            completion_state: CompletionState::Failed,
            http_status: Some(502),
            error_category: Some(SYNTHETIC_HISTORY_SENTINEL.to_owned()),
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
                route_id,
                route_name: SYNTHETIC_HISTORY_SENTINEL.to_owned(),
                started_at_ms: 10,
                finished_at_ms: 20,
                http_status: Some(502),
                error_category: Some(SYNTHETIC_HISTORY_SENTINEL.to_owned()),
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
        .await?;
    Ok(())
}

async fn seed(root: &QaAcceptanceRoot) -> Result<RecoverySummary, Box<dyn Error>> {
    root.prepare_runtime_directories()?;
    let database_path = root.database_path();
    let codex_path = root.codex_home_dir().join("config.toml");
    if database_path.try_exists()? || codex_path.try_exists()? {
        return Err("refusing to seed existing QA database or Codex configuration".into());
    }
    create_private_file(&codex_path, SYNTHETIC_CODEX_CONFIG)?;
    let database = DatabaseExecutor::open(&database_path)?;
    let first = database
        .create_route(CreateRouteInput {
            name: "Synthetic Recovery A".to_owned(),
            base_url: "http://127.0.0.1:39001/v1".to_owned(),
            api_key: ApiKey::parse("qa-recovery-key-a")?,
            service_tier_policy: ServiceTierPolicy::Passthrough,
            balance_query: Some(BalanceQueryInput {
                mode: BalanceQueryMode::CustomJs,
                enabled: false,
                custom_source: "(() => ({ request: { url: '{{baseUrl}}/usage' } }))()".to_owned(),
            }),
            accept_script_risk: false,
        })
        .await?;
    let second = database
        .create_route(CreateRouteInput {
            name: "Synthetic Recovery B".to_owned(),
            base_url: "http://127.0.0.1:39002/v1".to_owned(),
            api_key: ApiKey::parse("qa-recovery-key-b")?,
            service_tier_policy: ServiceTierPolicy::Omit,
            balance_query: None,
            accept_script_risk: false,
        })
        .await?;
    database.activate_route(second.route_id.clone()).await?;
    database.set_fallback_enabled(true).await?;
    database
        .set_balance_query_policy(BalanceQueryPolicy::parse(45, 120)?)
        .await?;
    database
        .get_or_create_singleton_secret(
            "gateway_token".to_owned(),
            ApiKey::parse("qa-recovery-gateway-token")?,
        )
        .await?;
    database
        .capture_codex_baseline(true, SYNTHETIC_CODEX_CONFIG.to_vec(), Some(0o600))
        .await?;
    record_synthetic_history(&database, first.route_id).await?;
    RecoveryManager::new(&database_path)
        .create_point(&database)
        .await?;
    drop(database);
    tokio::time::sleep(Duration::from_millis(30)).await;
    inspect(root, "seed").await
}

async fn inspect(
    root: &QaAcceptanceRoot,
    operation: &str,
) -> Result<RecoverySummary, Box<dyn Error>> {
    let manager = RecoveryManager::new(root.database_path());
    let inventory = manager.scan()?;
    let candidates = inventory
        .valid_points
        .iter()
        .map(|point| CandidateSummary {
            point_id: point.point_id.as_str().to_owned(),
            created_at_ms: point.created_at_ms,
            critical_revision: point.critical_revision,
        })
        .collect::<Vec<_>>();
    let classification = manager.classify_startup()?;
    let (startup, health, route_count, request_count, attempt_count) = match classification {
        DatabaseStartupClassification::NewInstall => {
            ("new_install".to_owned(), None, None, None, None)
        }
        DatabaseStartupClassification::RecoveryRequired(_) => {
            ("recovery_required".to_owned(), None, None, None, None)
        }
        DatabaseStartupClassification::Fatal(issue) => (
            format!("fatal_{issue:?}").to_lowercase(),
            None,
            None,
            None,
            None,
        ),
        DatabaseStartupClassification::Ready => {
            let database = DatabaseExecutor::open(root.database_path())?;
            let live_revision = database.critical_revision().await?;
            let covered = inventory
                .valid_points
                .first()
                .map(|point| point.critical_revision);
            let health = if covered.is_some_and(|revision| revision >= live_revision) {
                "protected"
            } else {
                "degraded"
            };
            let route_count = database.list_routes().await?.len();
            let request_count = database.history_summary().await?.request_count;
            drop(database);
            let connection = Connection::open_with_flags(
                root.database_path(),
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )?;
            let attempt_count =
                connection.query_row("SELECT COUNT(*) FROM upstream_attempts", [], |row| {
                    row.get(0)
                })?;
            (
                "ready".to_owned(),
                Some(health.to_owned()),
                Some(route_count),
                Some(request_count),
                Some(attempt_count),
            )
        }
    };
    let quarantine_count = if manager.recovery_dir().exists() {
        fs::read_dir(manager.recovery_dir())?
            .filter_map(Result::ok)
            .filter(|entry| {
                entry.file_name().to_str().is_some_and(|name| {
                    name.starts_with("quarantine-") && name.ends_with(".sqlite3")
                })
            })
            .count()
    } else {
        0
    };
    Ok(RecoverySummary {
        schema_version: SCHEMA_VERSION,
        operation: operation.to_owned(),
        startup,
        health,
        valid_point_count: inventory.valid_points.len(),
        invalid_point_count: inventory.invalid_point_count,
        candidates,
        route_count,
        request_count,
        attempt_count,
        quarantine_count,
        codex_config_unchanged: codex_snapshot(root)?.0 == SYNTHETIC_CODEX_CONFIG,
        retention_within_limit: inventory.valid_points.len() <= MAX_VALID_POINTS,
    })
}

async fn mutate(
    root: &QaAcceptanceRoot,
    action: &str,
    point_id: Option<&str>,
) -> Result<RecoverySummary, Box<dyn Error>> {
    let codex_before = codex_snapshot(root)?;
    let primary = root.database_path();
    let manager = RecoveryManager::new(&primary);
    match action {
        "degrade" => {
            let database = DatabaseExecutor::open(&primary)?;
            let port = database.app_settings().await?.proxy_port;
            database.set_proxy_port(port.saturating_add(1)).await?;
            drop(database);
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
        "corrupt-primary" => {
            ensure_private_regular_file(&primary)?;
            fs::write(&primary, b"synthetic corrupt QA primary")?;
            set_mode(&primary, 0o600)?;
        }
        "delete-primary" => {
            ensure_private_regular_file(&primary)?;
            fs::remove_file(&primary)?;
        }
        "future-schema" => {
            ensure_private_regular_file(&primary)?;
            let connection = Connection::open(&primary)?;
            let version: i64 =
                connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
            connection.pragma_update(None, "user_version", version.saturating_add(1))?;
        }
        "permission-primary" => {
            ensure_private_regular_file(&primary)?;
            set_mode(&primary, 0o000)?;
        }
        "invalidate-points" => {
            for entry in fs::read_dir(manager.recovery_dir())? {
                let entry = entry?;
                let name = entry.file_name();
                let Some(name) = name.to_str() else { continue };
                if name.starts_with("point-") && name.ends_with(".sqlite3") {
                    ensure_private_regular_file(&entry.path())?;
                    fs::write(entry.path(), b"synthetic invalid QA point")?;
                    set_mode(&entry.path(), 0o600)?;
                }
            }
        }
        "restore" => {
            let point = RecoveryPointId::parse(point_id.ok_or("restore requires --point-id")?)?;
            manager.restore_point(&point)?;
        }
        "start-over" => {
            manager.start_over()?;
            let database = DatabaseExecutor::open(&primary)?;
            manager.create_point(&database).await?;
            drop(database);
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
        "publish-retention" => {
            let database = DatabaseExecutor::open(&primary)?;
            for offset in 0..=MAX_VALID_POINTS {
                database
                    .set_proxy_port(40_000 + u16::try_from(offset)?)
                    .await?;
                manager.create_point(&database).await?;
            }
            drop(database);
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
        _ => return Err("unsupported recovery fixture action".into()),
    }
    if codex_snapshot(root)? != codex_before {
        return Err("recovery fixture action changed the QA Codex configuration".into());
    }
    inspect(root, action).await
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    let command = arguments
        .first()
        .ok_or("usage: v0_2b_qa_recovery <seed|inspect|apply> --root PATH [--action ACTION]")?;
    let root = resolve_root(&required_option(&arguments, "--root")?)?;
    let summary = match command.as_str() {
        "seed" => seed(&root).await?,
        "inspect" => inspect(&root, "inspect").await?,
        "apply" => {
            let action = required_option(&arguments, "--action")?;
            validate_action_permit(&root, &action, &canonical_qa_bundle())?;
            mutate(
                &root,
                &action,
                optional_option(&arguments, "--point-id").as_deref(),
            )
            .await?
        }
        _ => return Err("unsupported recovery fixture command".into()),
    };
    println!("{}", serde_json::to_string_pretty(&summary)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use router_core::qa_acceptance::{QA_ACCEPTANCE_MARKER_FILE, QA_ACCEPTANCE_ROOT_PREFIX};

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
        .expect("root override")
    }

    #[tokio::test]
    async fn synthetic_fixture_covers_protected_degraded_restore_and_retention() {
        let temporary = TempDir::new().expect("temporary");
        let root = root(&temporary, "recovery-states");
        let seeded = seed(&root).await.expect("seed");
        assert_eq!(seeded.startup, "ready");
        assert_eq!(seeded.health.as_deref(), Some("protected"));
        assert_eq!(seeded.route_count, Some(2));
        assert_eq!(seeded.request_count, Some(1));
        assert_eq!(seeded.valid_point_count, 1);

        let degraded = mutate(&root, "degrade", None).await.expect("degrade");
        assert_eq!(degraded.health.as_deref(), Some("degraded"));
        let retained = mutate(&root, "publish-retention", None)
            .await
            .expect("retention");
        assert_eq!(retained.health.as_deref(), Some("protected"));
        assert_eq!(retained.valid_point_count, MAX_VALID_POINTS);

        let point_id = retained.candidates[0].point_id.clone();
        let corrupt = mutate(&root, "corrupt-primary", None)
            .await
            .expect("corrupt");
        assert_eq!(corrupt.startup, "recovery_required");
        let restored = mutate(&root, "restore", Some(&point_id))
            .await
            .expect("restore");
        assert_eq!(restored.startup, "ready");
        assert_eq!(restored.request_count, Some(0));
        assert_eq!(restored.attempt_count, Some(0));
        assert_eq!(restored.quarantine_count, 1);
        assert!(restored.codex_config_unchanged);

        let second_corrupt = mutate(&root, "corrupt-primary", None)
            .await
            .expect("second corrupt");
        assert_eq!(second_corrupt.startup, "recovery_required");
        let second_restore = mutate(&root, "restore", Some(&point_id))
            .await
            .expect("second restore");
        assert_eq!(second_restore.quarantine_count, 1);
    }

    #[tokio::test]
    async fn synthetic_fixture_covers_missing_no_candidate_start_over_and_fatal_states() {
        let temporary = TempDir::new().expect("temporary");
        let root = root(&temporary, "recovery-errors");
        seed(&root).await.expect("seed");
        let missing = mutate(&root, "delete-primary", None)
            .await
            .expect("missing");
        assert_eq!(missing.startup, "recovery_required");
        let invalid = mutate(&root, "invalidate-points", None)
            .await
            .expect("invalid points");
        assert_eq!(invalid.startup, "recovery_required");
        assert_eq!(invalid.valid_point_count, 0);
        assert!(invalid.invalid_point_count > 0);
        let started = mutate(&root, "start-over", None).await.expect("start over");
        assert_eq!(started.startup, "ready");
        assert_eq!(started.route_count, Some(0));
        assert_eq!(started.quarantine_count, 0);

        let future = mutate(&root, "future-schema", None)
            .await
            .expect("future schema");
        assert_eq!(future.startup, "fatal_futureschema");
        #[cfg(unix)]
        {
            set_mode(&root.database_path(), 0o600).expect("restore mode");
            let permission = mutate(&root, "permission-primary", None)
                .await
                .expect("permission");
            assert_eq!(permission.startup, "fatal_permission");
            set_mode(&root.database_path(), 0o600).expect("cleanup mode");
        }
    }

    #[test]
    fn action_permit_requires_exact_runtime_paths_and_consumes_once() {
        let temporary = TempDir::new().expect("temporary");
        let root = root(&temporary, "permit");
        root.prepare_runtime_directories().expect("directories");
        let bundle = root.root().join("AI Router QA.app");
        let executable = bundle.join("Contents/MacOS/ai-router-app");
        fs::create_dir_all(executable.parent().expect("executable parent")).expect("bundle");
        fs::write(bundle.join("Contents/Info.plist"), b"fixture").expect("plist");
        fs::write(&executable, b"fixture").expect("executable");
        root.write_runtime_marker(42, QA_APP_IDENTIFIER, &executable)
            .expect("runtime marker");
        let permit = serde_json::json!({
            "schemaVersion": 1,
            "nonce": root.nonce(),
            "action": "corrupt-primary",
            "pid": 42,
            "identifier": QA_APP_IDENTIFIER,
            "bundlePath": bundle,
            "executablePath": executable,
            "appDataDir": root.app_data_dir(),
            "codexHomeDir": root.codex_home_dir(),
            "logDir": root.log_dir(),
        });
        fs::write(
            root.root().join(PERMIT_FILE),
            serde_json::to_vec(&permit).expect("permit JSON"),
        )
        .expect("permit");
        validate_action_permit(&root, "corrupt-primary", &bundle).expect("valid permit");
        assert!(!root.root().join(PERMIT_FILE).exists());
    }
}
