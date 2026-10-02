use std::{fs, path::Path};

use rusqlite::{Connection, params};
use sha2::{Digest, Sha256};

use super::{SCHEMA_VERSION, StorageError, read_fallback_config};
use crate::balance::BalanceQueryMode;
const GENERAL_BALANCE_SOURCE_HASHES: [&str; 3] = [
    "24cbea85c2fa635112e5915836e2a78144e0a6a21997b86ef5187c2665e14507",
    "be1d8023ddf04aa987b91d856637eeb86a21a6d504f4475ca2f2945b3132ff6c",
    "f60ff5d32ac946ac0fb8dd616aff15673710f534631464bfb0517833d9170390",
];

pub(super) fn is_general_balance_source_hash(source_hash: &str) -> bool {
    GENERAL_BALANCE_SOURCE_HASHES.contains(&source_hash)
}
pub(super) fn open_connection(path: &Path) -> Result<Connection, StorageError> {
    let mut connection = Connection::open(path)?;
    connection.busy_timeout(std::time::Duration::from_secs(5))?;
    connection.pragma_update(None, "journal_mode", "DELETE")?;
    connection.pragma_update(None, "synchronous", "FULL")?;
    connection.pragma_update(None, "foreign_keys", true)?;
    migrate(&mut connection)?;
    verify_connection(&connection)?;
    Ok(connection)
}

fn migrate(connection: &mut Connection) -> Result<(), StorageError> {
    let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version > SCHEMA_VERSION {
        return Err(StorageError::FutureSchema);
    }
    if version == 0 {
        migrate_v1(connection)?;
    }
    if version < 2 {
        migrate_v2(connection)?;
    }
    if version < 3 {
        migrate_v3(connection)?;
    }
    if version < 4 {
        migrate_v4(connection)?;
    }
    if version < 5 {
        migrate_v5(connection)?;
    }
    if version < 6 {
        migrate_v6(connection)?;
    }
    if version < 7 {
        migrate_v7(connection)?;
    }
    if version < 8 {
        migrate_v8(connection)?;
    }
    if version < 9 {
        migrate_v9(connection)?;
    }
    if version < 10 {
        migrate_v10(connection)?;
    }
    if version < 11 {
        migrate_v11(connection)?;
    }
    if version < 12 {
        migrate_v12(connection)?;
    }
    if version < 13 {
        migrate_v13(connection)?;
    }
    if version < 14 {
        migrate_v14(connection)?;
    }
    if version < 15 {
        migrate_v15(connection)?;
    }
    if version < 16 {
        migrate_v16(connection)?;
    }
    if version < 17 {
        migrate_v17(connection)?;
    }
    if version < 18 {
        migrate_v18(connection)?;
    }
    if version < 19 {
        migrate_v19(connection)?;
    }
    if version < 20 {
        migrate_v20(connection)?;
    }
    if version < 21 {
        migrate_v21(connection)?;
    }
    if version < 22 {
        migrate_v22(connection)?;
    }
    if version < 23 {
        migrate_v23(connection)?;
    }
    if version < 24 {
        migrate_v24(connection)?;
    }
    if version < 25 {
        migrate_v25(connection)?;
    }
    if version < 26 {
        migrate_v26(connection)?;
    }
    if version < 27 {
        migrate_v27(connection)?;
    }
    Ok(())
}

pub(super) fn migrate_v1(connection: &mut Connection) -> Result<(), StorageError> {
    connection.pragma_update(None, "auto_vacuum", "INCREMENTAL")?;
    let transaction = connection.transaction()?;
    transaction.execute_batch(
    "
    CREATE TABLE secrets (
        secret_id TEXT PRIMARY KEY,
        kind TEXT NOT NULL,
        value BLOB NOT NULL,
        created_at_ms INTEGER NOT NULL,
        updated_at_ms INTEGER NOT NULL
    );
    CREATE UNIQUE INDEX secrets_singleton_kind_idx ON secrets(kind) WHERE kind = 'gateway_token';
    CREATE TABLE routes (
        route_id TEXT PRIMARY KEY,
        display_name TEXT NOT NULL,
        display_name_key TEXT NOT NULL UNIQUE,
        base_url TEXT NOT NULL,
        secret_id TEXT NOT NULL UNIQUE REFERENCES secrets(secret_id) ON DELETE RESTRICT,
        sort_order INTEGER NOT NULL,
        created_at_ms INTEGER NOT NULL,
        updated_at_ms INTEGER NOT NULL
    );
    CREATE TABLE balance_scripts (
        route_id TEXT PRIMARY KEY REFERENCES routes(route_id) ON DELETE CASCADE,
        contract_version TEXT NOT NULL,
        enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
        source TEXT NOT NULL,
        updated_at_ms INTEGER NOT NULL
    );
    CREATE TABLE route_state (
        singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
        route_id TEXT REFERENCES routes(route_id) ON DELETE SET NULL,
        updated_at_ms INTEGER NOT NULL
    );
    CREATE TABLE app_settings (
        singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
        proxy_port INTEGER NOT NULL,
        first_run_presented INTEGER NOT NULL CHECK (first_run_presented IN (0, 1)),
        balance_script_risk_confirmed INTEGER NOT NULL CHECK (balance_script_risk_confirmed IN (0, 1))
    );
    CREATE TABLE codex_baseline (
        singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
        original_exists INTEGER NOT NULL CHECK (original_exists IN (0, 1)),
        raw_bytes BLOB,
        unix_mode INTEGER,
        captured_at_ms INTEGER NOT NULL
    );
    CREATE TABLE proxy_requests (
        request_id TEXT PRIMARY KEY,
        started_at_ms INTEGER NOT NULL,
        finished_at_ms INTEGER,
        turn_id TEXT,
        turn_sequence INTEGER,
        reconnect_sequence INTEGER,
        requested_model TEXT,
        actual_model TEXT,
        final_route_id TEXT,
        final_route_name TEXT,
        streaming INTEGER NOT NULL,
        completion_state TEXT NOT NULL,
        http_status INTEGER,
        error_category TEXT,
        input_tokens INTEGER,
        output_tokens INTEGER,
        total_tokens INTEGER,
        total_latency_ms INTEGER,
        first_output_latency_ms INTEGER,
        metadata_complete INTEGER NOT NULL
    );
    CREATE INDEX proxy_requests_started_at_idx ON proxy_requests(started_at_ms);
    CREATE TABLE upstream_attempts (
        attempt_id TEXT PRIMARY KEY,
        request_id TEXT NOT NULL REFERENCES proxy_requests(request_id) ON DELETE CASCADE,
        attempt_index INTEGER NOT NULL,
        route_id TEXT NOT NULL,
        route_name TEXT NOT NULL,
        started_at_ms INTEGER NOT NULL,
        finished_at_ms INTEGER,
        http_status INTEGER,
        error_category TEXT,
        delivery_state TEXT NOT NULL,
        input_tokens INTEGER,
        output_tokens INTEGER,
        total_tokens INTEGER,
        UNIQUE(request_id, attempt_index)
    );
    INSERT INTO route_state (singleton, route_id, updated_at_ms) VALUES (1, NULL, 0);
    INSERT INTO app_settings (singleton, proxy_port, first_run_presented, balance_script_risk_confirmed) VALUES (1, 32189, 0, 0);
    PRAGMA user_version = 1;
    ",
)?;
    transaction.commit()?;
    Ok(())
}

pub(super) fn migrate_v2(connection: &mut Connection) -> Result<(), StorageError> {
    let transaction = connection.transaction()?;
    transaction.execute_batch(
        "
    ALTER TABLE route_state ADD COLUMN selection_generation INTEGER NOT NULL DEFAULT 0;
    CREATE TABLE fallback_config (
        singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
        enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
        config_revision INTEGER NOT NULL,
        updated_at_ms INTEGER NOT NULL
    );
    INSERT INTO fallback_config (singleton, enabled, config_revision, updated_at_ms)
    VALUES (1, 0, 0, 0);
    PRAGMA user_version = 2;
    ",
    )?;
    transaction.commit()?;
    Ok(())
}

pub(super) fn migrate_v3(connection: &mut Connection) -> Result<(), StorageError> {
    let transaction = connection.transaction()?;
    transaction.execute_batch(
    "
    ALTER TABLE app_settings ADD COLUMN menu_balance_debounce_seconds INTEGER NOT NULL DEFAULT 30 CHECK (menu_balance_debounce_seconds BETWEEN 10 AND 600);
    ALTER TABLE app_settings ADD COLUMN automatic_balance_refresh_minutes INTEGER NOT NULL DEFAULT 30 CHECK (automatic_balance_refresh_minutes BETWEEN 5 AND 1440);
    PRAGMA user_version = 3;
    ",
)?;
    transaction.commit()?;
    Ok(())
}

pub(super) fn migrate_v4(connection: &mut Connection) -> Result<(), StorageError> {
    let transaction = connection.transaction()?;
    transaction.execute_batch(
        "
    CREATE TABLE recovery_revision (
        singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
        critical_revision INTEGER NOT NULL CHECK (critical_revision >= 0),
        updated_at_ms INTEGER NOT NULL
    );
    CREATE TABLE recovery_point_metadata (
        singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
        format_version INTEGER NOT NULL,
        point_id TEXT NOT NULL,
        created_at_ms INTEGER NOT NULL,
        critical_revision INTEGER NOT NULL CHECK (critical_revision >= 0)
    );
    INSERT INTO recovery_revision (singleton, critical_revision, updated_at_ms)
    VALUES (1, 0, 0);
    PRAGMA user_version = 4;
    ",
    )?;
    transaction.commit()?;
    Ok(())
}

pub(super) fn migrate_v5(connection: &mut Connection) -> Result<(), StorageError> {
    let transaction = connection.transaction()?;
    transaction.execute_batch(
    "
    ALTER TABLE proxy_requests ADD COLUMN requested_service_tier TEXT;
    ALTER TABLE proxy_requests ADD COLUMN actual_service_tier TEXT;
    ALTER TABLE proxy_requests ADD COLUMN cached_input_tokens INTEGER CHECK (cached_input_tokens >= 0);
    ALTER TABLE proxy_requests ADD COLUMN cache_write_input_tokens INTEGER CHECK (cache_write_input_tokens >= 0);
    ALTER TABLE proxy_requests ADD COLUMN pricing_catalog_version TEXT;
    ALTER TABLE proxy_requests ADD COLUMN cost_status TEXT CHECK (cost_status IN ('exact', 'partial', 'unavailable', 'not_applicable'));
    ALTER TABLE proxy_requests ADD COLUMN upstream_cost_pico_usd INTEGER CHECK (upstream_cost_pico_usd >= 0);
    ALTER TABLE upstream_attempts ADD COLUMN actual_model TEXT;
    ALTER TABLE upstream_attempts ADD COLUMN actual_service_tier TEXT;
    ALTER TABLE upstream_attempts ADD COLUMN cached_input_tokens INTEGER CHECK (cached_input_tokens >= 0);
    ALTER TABLE upstream_attempts ADD COLUMN cache_write_input_tokens INTEGER CHECK (cache_write_input_tokens >= 0);
    ALTER TABLE upstream_attempts ADD COLUMN pricing_catalog_version TEXT;
    ALTER TABLE upstream_attempts ADD COLUMN cost_status TEXT CHECK (cost_status IN ('exact', 'partial', 'unavailable', 'not_applicable'));
    ALTER TABLE upstream_attempts ADD COLUMN cost_pico_usd INTEGER CHECK (cost_pico_usd >= 0);
    DROP INDEX proxy_requests_started_at_idx;
    CREATE INDEX proxy_requests_keyset_idx ON proxy_requests(started_at_ms DESC, request_id DESC);
    CREATE INDEX proxy_requests_status_keyset_idx ON proxy_requests(completion_state, started_at_ms DESC, request_id DESC);
    CREATE INDEX proxy_requests_route_keyset_idx ON proxy_requests(final_route_id, started_at_ms DESC, request_id DESC);
    CREATE INDEX proxy_requests_model_keyset_idx ON proxy_requests(COALESCE(actual_model, requested_model), started_at_ms DESC, request_id DESC);
    PRAGMA user_version = 5;
    ",
)?;
    transaction.commit()?;
    Ok(())
}

pub(super) fn migrate_v6(connection: &mut Connection) -> Result<(), StorageError> {
    let transaction = connection.transaction()?;
    transaction.execute_batch(
        "
    ALTER TABLE proxy_requests ADD COLUMN reasoning_effort TEXT;
    PRAGMA user_version = 6;
    ",
    )?;
    transaction.commit()?;
    Ok(())
}

pub(super) fn migrate_v7(connection: &mut Connection) -> Result<(), StorageError> {
    let transaction = connection.transaction()?;
    transaction.execute_batch(
        "
    ALTER TABLE proxy_requests ADD COLUMN first_text_output_latency_ms INTEGER
        CHECK (first_text_output_latency_ms >= 0);
    PRAGMA user_version = 7;
    ",
    )?;
    transaction.commit()?;
    Ok(())
}

pub(super) fn migrate_v8(connection: &mut Connection) -> Result<(), StorageError> {
    let transaction = connection.transaction()?;
    transaction.execute_batch(
        "
    ALTER TABLE routes ADD COLUMN service_tier_policy TEXT NOT NULL DEFAULT 'passthrough'
        CHECK (service_tier_policy IN ('passthrough', 'omit'));
    ALTER TABLE upstream_attempts ADD COLUMN forwarded_service_tier TEXT;
    UPDATE upstream_attempts
    SET forwarded_service_tier = (
        SELECT proxy_requests.requested_service_tier
        FROM proxy_requests
        WHERE proxy_requests.request_id = upstream_attempts.request_id
    );
    PRAGMA user_version = 8;
    ",
    )?;
    transaction.commit()?;
    Ok(())
}

pub(super) fn migrate_v9(connection: &mut Connection) -> Result<(), StorageError> {
    let transaction = connection.transaction()?;
    transaction.execute_batch(
    "
    UPDATE proxy_requests
    SET first_output_latency_ms = first_text_output_latency_ms
    WHERE first_output_latency_ms IS NULL
      AND first_text_output_latency_ms IS NOT NULL;
    DROP INDEX proxy_requests_keyset_idx;
    DROP INDEX proxy_requests_status_keyset_idx;
    DROP INDEX proxy_requests_route_keyset_idx;
    DROP INDEX proxy_requests_model_keyset_idx;
    ALTER TABLE proxy_requests DROP COLUMN first_text_output_latency_ms;
    CREATE INDEX proxy_requests_keyset_idx
        ON proxy_requests(finished_at_ms DESC, request_id DESC)
        WHERE finished_at_ms IS NOT NULL;
    CREATE INDEX proxy_requests_status_keyset_idx
        ON proxy_requests(completion_state, finished_at_ms DESC, request_id DESC)
        WHERE finished_at_ms IS NOT NULL;
    CREATE INDEX proxy_requests_route_keyset_idx
        ON proxy_requests(final_route_id, finished_at_ms DESC, request_id DESC)
        WHERE finished_at_ms IS NOT NULL;
    CREATE INDEX proxy_requests_model_keyset_idx
        ON proxy_requests(COALESCE(actual_model, requested_model), finished_at_ms DESC, request_id DESC)
        WHERE finished_at_ms IS NOT NULL;
    PRAGMA user_version = 9;
    ",
)?;
    transaction.commit()?;
    Ok(())
}

pub(super) fn migrate_v10(connection: &mut Connection) -> Result<(), StorageError> {
    let transaction = connection.transaction()?;
    transaction.execute_batch(
        "
    CREATE TABLE codex_models (
        model_id TEXT PRIMARY KEY COLLATE BINARY NOT NULL,
        display_name TEXT,
        context_window INTEGER,
        sort_order INTEGER NOT NULL UNIQUE,
        CHECK (length(trim(model_id)) > 0),
        CHECK (context_window IS NULL OR context_window > 0)
    );
    PRAGMA user_version = 10;
    ",
    )?;
    transaction.commit()?;
    Ok(())
}

pub(super) fn migrate_v11(connection: &mut Connection) -> Result<(), StorageError> {
    let transaction = connection.transaction()?;
    transaction.execute_batch(
        "
    DROP TABLE codex_models;
    CREATE TABLE codex_models (
        route_id TEXT NOT NULL REFERENCES routes(route_id) ON DELETE CASCADE,
        model_id TEXT COLLATE BINARY NOT NULL,
        display_name TEXT,
        context_window INTEGER,
        sort_order INTEGER NOT NULL,
        PRIMARY KEY (route_id, model_id),
        UNIQUE (route_id, sort_order),
        CHECK (length(trim(model_id)) > 0),
        CHECK (context_window IS NULL OR context_window > 0)
    );
    CREATE TABLE codex_restart_notice (
        singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
        notice_id TEXT NOT NULL UNIQUE CHECK (length(notice_id) > 0),
        route_id TEXT NOT NULL REFERENCES routes(route_id) ON DELETE CASCADE,
        selection_generation INTEGER NOT NULL CHECK (selection_generation >= 0),
        catalog_fingerprint TEXT NOT NULL CHECK (length(catalog_fingerprint) > 0),
        created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0)
    );
    PRAGMA user_version = 11;
    ",
    )?;
    transaction.commit()?;
    Ok(())
}

pub(super) fn migrate_v12(connection: &mut Connection) -> Result<(), StorageError> {
    let transaction = connection.transaction()?;
    transaction.execute_batch(
        "
    CREATE TABLE balance_queries (
        route_id TEXT PRIMARY KEY REFERENCES routes(route_id) ON DELETE CASCADE,
        mode TEXT NOT NULL CHECK (mode IN ('general_v1', 'custom_js')),
        enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
        custom_source TEXT NOT NULL DEFAULT '',
        updated_at_ms INTEGER NOT NULL,
        CHECK (mode != 'custom_js' OR enabled = 0 OR length(trim(custom_source)) > 0)
    );
    ",
    )?;
    let rows = {
        let mut statement = transaction
            .prepare("SELECT route_id, enabled, source, updated_at_ms FROM balance_scripts")?;
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, bool>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?
    };
    for (route_id, enabled, source, updated_at_ms) in rows {
        let source_hash = hex::encode(Sha256::digest(source.as_bytes()));
        let (mode, custom_source) = if is_general_balance_source_hash(&source_hash) {
            (BalanceQueryMode::GeneralV1, "")
        } else {
            (BalanceQueryMode::CustomJs, source.as_str())
        };
        transaction.execute(
        "INSERT INTO balance_queries (route_id, mode, enabled, custom_source, updated_at_ms) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![route_id, mode.as_str(), enabled, custom_source, updated_at_ms],
    )?;
    }
    transaction.execute_batch(
        "
    DROP TABLE balance_scripts;
    PRAGMA user_version = 12;
    ",
    )?;
    transaction.commit()?;
    Ok(())
}

pub(super) fn migrate_v13(connection: &mut Connection) -> Result<(), StorageError> {
    let transaction = connection.transaction()?;
    transaction.execute_batch(
        "
    ALTER TABLE fallback_config
        ADD COLUMN participant_count INTEGER NOT NULL DEFAULT 0
        CHECK (participant_count >= 0);
    UPDATE fallback_config
    SET participant_count = MIN(4, (SELECT COUNT(*) FROM routes)),
        enabled = CASE
            WHEN MIN(4, (SELECT COUNT(*) FROM routes)) < 2 THEN 0
            ELSE enabled
        END;
    PRAGMA user_version = 13;
    ",
    )?;
    transaction.commit()?;
    Ok(())
}

pub(super) fn migrate_v14(connection: &mut Connection) -> Result<(), StorageError> {
    let transaction = connection.transaction()?;
    transaction.execute_batch(
        "
    ALTER TABLE routes
        ADD COLUMN supports_images_generation INTEGER NOT NULL DEFAULT 0
        CHECK (supports_images_generation IN (0, 1));
    ALTER TABLE app_settings
        ADD COLUMN images_generation_enabled INTEGER NOT NULL DEFAULT 0
        CHECK (images_generation_enabled IN (0, 1));
    ALTER TABLE app_settings
        ADD COLUMN images_generation_route_id TEXT DEFAULT NULL
        REFERENCES routes(route_id) ON DELETE SET NULL;
    PRAGMA user_version = 14;
    ",
    )?;
    transaction.commit()?;
    Ok(())
}

pub(super) fn migrate_v15(connection: &mut Connection) -> Result<(), StorageError> {
    let transaction = connection.transaction()?;
    transaction.execute_batch(
        "
    ALTER TABLE app_settings
        ADD COLUMN images_generation_timeout_secs INTEGER NOT NULL DEFAULT 600
        CHECK (images_generation_timeout_secs BETWEEN 600 AND 3600);
    ALTER TABLE routes DROP COLUMN supports_images_generation;
    PRAGMA user_version = 15;
    ",
    )?;
    transaction.commit()?;
    Ok(())
}

pub(super) fn migrate_v16(connection: &mut Connection) -> Result<(), StorageError> {
    let transaction = connection.transaction()?;
    transaction.execute_batch(
        "
    ALTER TABLE app_settings ADD COLUMN appearance_preference TEXT NOT NULL DEFAULT 'system'
        CHECK (appearance_preference IN ('system', 'light', 'dark'));
    PRAGMA user_version = 16;
    ",
    )?;
    transaction.commit()?;
    Ok(())
}

pub(super) fn migrate_v17(connection: &mut Connection) -> Result<(), StorageError> {
    let transaction = connection.transaction()?;
    transaction.execute_batch(
        "
    ALTER TABLE proxy_requests ADD COLUMN fallback_stop_reason TEXT
        CHECK (fallback_stop_reason IS NULL OR fallback_stop_reason IN (
            'fallback_disabled',
            'failure_not_eligible',
            'response_committed',
            'all_participants_attempted',
            'stale_policy',
            'activation_failed',
            'attempt_index_exhausted'
        ));
    ALTER TABLE proxy_requests ADD COLUMN fallback_stop_target_route_id TEXT;
    ALTER TABLE proxy_requests ADD COLUMN fallback_stop_target_route_name TEXT;
    PRAGMA user_version = 17;
    ",
    )?;
    transaction.commit()?;
    Ok(())
}

pub(super) fn migrate_v18(connection: &mut Connection) -> Result<(), StorageError> {
    let transaction = connection.transaction()?;
    transaction.execute_batch(
    "
    CREATE TABLE codex_recovery_config (
        singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
        original_exists INTEGER NOT NULL CHECK (original_exists IN (0, 1)),
        raw_bytes BLOB,
        unix_mode INTEGER,
        updated_at_ms INTEGER NOT NULL,
        CHECK ((original_exists = 1 AND raw_bytes IS NOT NULL) OR
               (original_exists = 0 AND raw_bytes IS NULL AND unix_mode IS NULL))
    );
    INSERT INTO codex_recovery_config (singleton, original_exists, raw_bytes, unix_mode, updated_at_ms)
    SELECT singleton, original_exists,
           CASE WHEN original_exists = 1 THEN raw_bytes ELSE NULL END,
           CASE WHEN original_exists = 1 THEN unix_mode ELSE NULL END,
           captured_at_ms
    FROM codex_baseline
    WHERE singleton = 1;
    PRAGMA user_version = 18;
    ",
)?;
    transaction.commit()?;
    Ok(())
}

pub(super) fn migrate_v19(connection: &mut Connection) -> Result<(), StorageError> {
    let transaction = connection.transaction()?;
    transaction.execute_batch(
        "
    ALTER TABLE app_settings ADD COLUMN last_automatic_update_check_at_ms INTEGER
        CHECK (last_automatic_update_check_at_ms IS NULL OR last_automatic_update_check_at_ms >= 0);
    PRAGMA user_version = 19;
    ",
    )?;
    transaction.commit()?;
    Ok(())
}

pub(super) fn migrate_v20(connection: &mut Connection) -> Result<(), StorageError> {
    let transaction = connection.transaction()?;
    transaction.execute_batch(
        "
    CREATE TABLE route_fallback_excluded_models (
        route_id TEXT NOT NULL REFERENCES routes(route_id) ON DELETE CASCADE,
        model_id TEXT COLLATE BINARY NOT NULL,
        sort_order INTEGER NOT NULL CHECK (sort_order >= 0),
        PRIMARY KEY (route_id, model_id),
        UNIQUE (route_id, sort_order),
        CHECK (length(trim(model_id)) > 0)
    );

    ALTER TABLE upstream_attempts ADD COLUMN attempt_role TEXT NOT NULL
        DEFAULT 'ordinary'
        CHECK (attempt_role IN ('ordinary', 'recovery_probe'));
    ALTER TABLE upstream_attempts ADD COLUMN routing_transition_kind TEXT
        CHECK (routing_transition_kind IS NULL OR routing_transition_kind IN (
            'activate_next', 'resume_captured', 'recover'
        ));
    ALTER TABLE upstream_attempts ADD COLUMN routing_transition_target_route_id TEXT;
    ALTER TABLE upstream_attempts ADD COLUMN routing_transition_target_route_name TEXT;

    CREATE TABLE upstream_attempt_routing_skips (
        attempt_id TEXT NOT NULL REFERENCES upstream_attempts(attempt_id) ON DELETE CASCADE,
        skip_order INTEGER NOT NULL CHECK (skip_order >= 0),
        route_id TEXT NOT NULL,
        route_name TEXT NOT NULL,
        reason TEXT NOT NULL CHECK (reason IN ('model_fallback_excluded')),
        PRIMARY KEY (attempt_id, skip_order)
    );

    ALTER TABLE proxy_requests ADD COLUMN fallback_stop_reason_v20 TEXT;
    UPDATE proxy_requests
    SET fallback_stop_reason_v20 = fallback_stop_reason;
    ALTER TABLE proxy_requests DROP COLUMN fallback_stop_reason;
    ALTER TABLE proxy_requests ADD COLUMN fallback_stop_reason TEXT
        CHECK (fallback_stop_reason IS NULL OR fallback_stop_reason IN (
            'fallback_disabled',
            'failure_not_eligible',
            'response_committed',
            'all_participants_attempted',
            'stale_policy',
            'activation_failed',
            'attempt_index_exhausted',
            'failure_threshold_not_reached',
            'failure_threshold_reached_pending',
            'recovery_confirmation_pending',
            'model_fallback_excluded'
        ));
    UPDATE proxy_requests
    SET fallback_stop_reason = fallback_stop_reason_v20;
    ALTER TABLE proxy_requests DROP COLUMN fallback_stop_reason_v20;

    PRAGMA user_version = 20;
    ",
    )?;
    transaction.commit()?;
    Ok(())
}

pub(super) fn migrate_v21(connection: &mut Connection) -> Result<(), StorageError> {
    let transaction = connection.transaction()?;
    transaction.execute_batch(
    "
    ALTER TABLE app_settings ADD COLUMN menu_bar_status_text_enabled INTEGER NOT NULL DEFAULT 1
        CHECK (menu_bar_status_text_enabled IN (0, 1));
    ALTER TABLE app_settings ADD COLUMN menu_bar_activity_animation_enabled INTEGER NOT NULL DEFAULT 1
        CHECK (menu_bar_activity_animation_enabled IN (0, 1));
    PRAGMA user_version = 21;
    ",
)?;
    transaction.commit()?;
    Ok(())
}

pub(super) fn migrate_v22(connection: &mut Connection) -> Result<(), StorageError> {
    let transaction = connection.transaction()?;
    transaction.execute_batch(
        "
    ALTER TABLE app_settings ADD COLUMN mcp_image_capacity_warning_mib INTEGER NOT NULL DEFAULT 1024
        CHECK (mcp_image_capacity_warning_mib BETWEEN 128 AND 102400);
    ALTER TABLE app_settings ADD COLUMN mcp_image_capacity_active_episode TEXT;
    ALTER TABLE app_settings ADD COLUMN mcp_image_capacity_dismissed_episode TEXT;
    PRAGMA user_version = 22;
    ",
    )?;
    transaction.commit()?;
    Ok(())
}

pub(super) fn migrate_v23(connection: &mut Connection) -> Result<(), StorageError> {
    let transaction = connection.transaction()?;
    transaction.execute_batch(
        "ALTER TABLE routes DROP COLUMN service_tier_policy; PRAGMA user_version = 23;",
    )?;
    transaction.commit()?;
    Ok(())
}

pub(super) fn migrate_v24(connection: &mut Connection) -> Result<(), StorageError> {
    let transaction = connection.transaction()?;
    transaction.execute_batch(
    "ALTER TABLE routes ADD COLUMN menu_visible INTEGER NOT NULL DEFAULT 1 CHECK (menu_visible IN (0, 1)); PRAGMA user_version = 24;",
)?;
    transaction.commit()?;
    Ok(())
}

pub(super) fn migrate_v25(connection: &mut Connection) -> Result<(), StorageError> {
    let transaction = connection.transaction()?;
    transaction.execute_batch(
        "
    ALTER TABLE app_settings ADD COLUMN outbound_proxy_enabled INTEGER NOT NULL DEFAULT 0
        CHECK (outbound_proxy_enabled IN (0, 1));
    ALTER TABLE app_settings ADD COLUMN outbound_proxy_url TEXT;
    PRAGMA user_version = 25;
    ",
    )?;
    transaction.commit()?;
    Ok(())
}

pub(super) fn migrate_v27(connection: &mut Connection) -> Result<(), StorageError> {
    let transaction = connection.transaction()?;
    transaction.execute_batch(
    "ALTER TABLE routes ADD COLUMN protocol TEXT NOT NULL DEFAULT 'responses' CHECK (protocol IN ('responses', 'chat_completions')); PRAGMA user_version = 27;",
)?;
    transaction.commit()?;
    Ok(())
}

pub(super) fn migrate_v26(connection: &mut Connection) -> Result<(), StorageError> {
    let transaction = connection.transaction()?;
    transaction.execute_batch(
        "
    ALTER TABLE app_settings ADD COLUMN images_generation_model TEXT NOT NULL DEFAULT 'gpt-image-2'
        CHECK (length(images_generation_model) BETWEEN 1 AND 256);
    PRAGMA user_version = 26;
    ",
    )?;
    transaction.commit()?;
    Ok(())
}
/// Verifies the complete live-database contract on an open connection.
///
/// Recovery reuses this helper so a post-repair recheck has exactly the same
/// PRAGMA, integrity, foreign-key, and domain-entry contract as the executor
/// open path.
///
/// # Errors
///
/// Returns [`StorageError::Initialization`] when any required PRAGMA, the
/// `pragma integrity_check` report, the foreign-key check, or the persisted
/// fallback configuration is not exactly as required.
pub(crate) fn verify_connection(connection: &Connection) -> Result<(), StorageError> {
    let journal_mode: String =
        connection.pragma_query_value(None, "journal_mode", |row| row.get(0))?;
    let synchronous: i64 = connection.pragma_query_value(None, "synchronous", |row| row.get(0))?;
    let foreign_keys: i64 =
        connection.pragma_query_value(None, "foreign_keys", |row| row.get(0))?;
    let auto_vacuum: i64 = connection.pragma_query_value(None, "auto_vacuum", |row| row.get(0))?;
    let integrity = crate::recovery::read_integrity_report(connection)?;
    let foreign_key_violation: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_foreign_key_check)",
        [],
        |row| row.get(0),
    )?;
    let _fallback = read_fallback_config(connection)?;

    if !journal_mode.eq_ignore_ascii_case("delete")
        || synchronous != 2
        || foreign_keys != 1
        || auto_vacuum != 2
        || integrity.classification != crate::recovery::IntegrityClassification::Ok
        || foreign_key_violation
    {
        return Err(StorageError::Initialization);
    }
    Ok(())
}

pub(super) fn prepare_database_path(path: &Path) -> Result<(), StorageError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
        set_mode(parent, 0o700)?;
    }
    Ok(())
}

pub(super) fn enforce_database_file_permissions(path: &Path) -> Result<(), StorageError> {
    if path.exists() {
        set_mode(path, 0o600)?;
    }
    Ok(())
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<(), std::io::Error> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> Result<(), std::io::Error> {
    Ok(())
}
