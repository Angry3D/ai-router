use std::{fs, path::Path};

use aes::Aes128;
use cbc::cipher::{BlockModeEncrypt, KeyIvInit, block_padding::Pkcs7};
use rusqlite::{Connection, params};
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use zeroize::Zeroizing;

use super::{
    AUTH_FILE_NAME, CBC_IV, CodexAuthError, CodexAuthStore, CredentialStoreModeDto, MAX_BACKUPS,
    META_FILE_NAME, SessionMaterial, SessionOutcome, V10_PREFIX, classify_session_body,
    discover_profiles, mask_email, read_credential_store_status, read_session_cookies,
    resolve_codex_home, select_profile, synthesize_auth_json, validate_auth_json,
};

const TEST_KEY: &[u8] = b"test-safe-storage-secret";
const NOW_MS: i64 = 1_790_000_000_000;
const EXPIRES_MS: i64 = 1_800_000_000_000;

fn encrypt_for_test(key: &[u8], host: &str, value: &[u8]) -> Vec<u8> {
    let derived = super::derive_chrome_key(key);
    let mut plain = Vec::new();
    plain.extend_from_slice(&Sha256::digest(host.as_bytes()));
    plain.extend_from_slice(value);
    let mut buffer = vec![0_u8; plain.len() + 16];
    buffer[..plain.len()].copy_from_slice(&plain);
    let ciphertext = cbc::Encryptor::<Aes128>::new_from_slices(&derived[..], &CBC_IV)
        .expect("test key/iv")
        .encrypt_padded::<Pkcs7>(&mut buffer, plain.len())
        .expect("test buffer");
    let mut out = V10_PREFIX.to_vec();
    out.extend_from_slice(ciphertext);
    out
}

fn create_cookie_db(
    home: &Path,
    profile: &str,
    rows: &[(&str, &str, Vec<u8>)],
) -> std::path::PathBuf {
    let directory = home
        .join("Library/Application Support/Google/Chrome")
        .join(profile);
    fs::create_dir_all(&directory).expect("profile directory");
    let database = directory.join("Cookies");
    let connection = Connection::open(&database).expect("cookie db");
    connection
        .execute_batch(
            "CREATE TABLE cookies (host_key TEXT NOT NULL, name TEXT NOT NULL, \
             encrypted_value BLOB, expires_utc INTEGER);",
        )
        .expect("schema");
    for (host, name, encrypted) in rows {
        connection
            .execute(
                "INSERT INTO cookies (host_key, name, encrypted_value, expires_utc) \
                 VALUES (?1, ?2, ?3, 0)",
                params![host, name, encrypted],
            )
            .expect("insert cookie");
    }
    drop(connection);
    database
}

fn session_fixture() -> SessionMaterial {
    SessionMaterial {
        access_token: Zeroizing::new("access-token-123".to_owned()),
        session_token: Some(Zeroizing::new("session-token-123".to_owned())),
        account_id: "acc-123".to_owned(),
        plan_type: Some("plus".to_owned()),
        user_id: "user-123".to_owned(),
        email: Some("tester@example.com".to_owned()),
        expires_at_ms: EXPIRES_MS,
    }
}

fn store(home: &Path, app_data: &Path) -> CodexAuthStore {
    CodexAuthStore::new(home.to_path_buf(), app_data)
}

#[test]
fn discovers_chrome_profile_and_decrypts_session_cookies() {
    let home = TempDir::new().expect("home");
    create_cookie_db(
        home.path(),
        "Default",
        &[
            (
                ".chatgpt.com",
                "__Secure-next-auth.session-token.0",
                encrypt_for_test(TEST_KEY, ".chatgpt.com", b"chunk-zero"),
            ),
            (
                ".chatgpt.com",
                "__Secure-next-auth.session-token.1",
                encrypt_for_test(TEST_KEY, ".chatgpt.com", b"chunk-one"),
            ),
            (
                ".chatgpt.com",
                "other",
                encrypt_for_test(TEST_KEY, ".chatgpt.com", b"unrelated"),
            ),
        ],
    );

    let candidate = select_profile(home.path()).expect("profile");
    assert_eq!(candidate.label, "Default");
    assert!(candidate.verified);
    assert_eq!(candidate.keychain_service, "Chrome Safe Storage");

    let cookies = read_session_cookies(&candidate, TEST_KEY).expect("cookies");
    assert_eq!(cookies.len(), 2);
    assert_eq!(cookies[0].name, "__Secure-next-auth.session-token.0");
    assert_eq!(cookies[0].value.as_slice(), b"chunk-zero");
    assert_eq!(cookies[1].value.as_slice(), b"chunk-one");
}

#[test]
fn rejects_cookies_with_an_unknown_encryption_prefix() {
    let home = TempDir::new().expect("home");
    create_cookie_db(
        home.path(),
        "Default",
        &[(
            ".chatgpt.com",
            "__Secure-next-auth.session-token.0",
            b"v20-not-a-supported-scheme".to_vec(),
        )],
    );
    let candidate = select_profile(home.path()).expect("profile");
    let error = read_session_cookies(&candidate, TEST_KEY).expect_err("unsupported");
    assert_eq!(error.code(), "codex_auth_unsupported_encryption");
}

#[test]
fn selection_fails_closed_per_condition() {
    let empty = TempDir::new().expect("home");
    assert!(matches!(
        select_profile(empty.path()),
        Err(CodexAuthError::BrowserMissing)
    ));

    let unsupported = TempDir::new().expect("home");
    create_cookie_db(
        unsupported.path(),
        "Default",
        &[(
            ".chatgpt.com",
            "__Secure-next-auth.session-token.0",
            encrypt_for_test(TEST_KEY, ".chatgpt.com", b"x"),
        )],
    );
    // Rename the verified Chrome directory into an unverified browser.
    let chrome = unsupported
        .path()
        .join("Library/Application Support/Google/Chrome");
    let chromium = unsupported
        .path()
        .join("Library/Application Support/Chromium");
    fs::rename(&chrome, &chromium).expect("rename to chromium");
    assert!(matches!(
        select_profile(unsupported.path()),
        Err(CodexAuthError::BrowserUnsupported)
    ));

    let logged_out = TempDir::new().expect("home");
    create_cookie_db(
        logged_out.path(),
        "Default",
        &[(
            ".chatgpt.com",
            "not-a-session-cookie",
            encrypt_for_test(TEST_KEY, ".chatgpt.com", b"x"),
        )],
    );
    assert!(matches!(
        select_profile(logged_out.path()),
        Err(CodexAuthError::ProfileNotLoggedIn)
    ));
}

#[test]
fn discovery_is_empty_without_browser_data() {
    let home = TempDir::new().expect("home");
    assert!(discover_profiles(home.path()).is_empty());
}

#[test]
fn classifies_session_bodies() {
    let authenticated = r#"{"WARNING_BANNER":"x","accessToken":"a","sessionToken":"b","expires":"2027-01-15T08:00:00Z","user":{"id":"u","email":"tester@example.com"},"account":{"id":"acc-123","planType":"plus"}}"#
        .to_owned();
    match classify_session_body(&authenticated) {
        SessionOutcome::Authenticated(session) => {
            assert_eq!(session.account_id, "acc-123");
            assert_eq!(session.expires_at_ms, 1_800_000_000_000);
            assert_eq!(session.plan_type.as_deref(), Some("plus"));
        }
        other => panic!("expected authenticated, got {other:?}"),
    }

    assert!(matches!(
        classify_session_body(r#"{"WARNING_BANNER":"x"}"#),
        SessionOutcome::NotLoggedIn
    ));
    assert!(matches!(
        classify_session_body("<html>Enable JavaScript</html>"),
        SessionOutcome::FetchFailed
    ));
    assert!(matches!(
        classify_session_body(r#"{"accessToken":"a"}"#),
        SessionOutcome::Invalid
    ));
}

#[test]
fn synthesizes_and_validates_reference_shape() {
    let session = session_fixture();
    let bytes = synthesize_auth_json(&session, NOW_MS);
    validate_auth_json(bytes.as_bytes(), &session).expect("valid");

    let value: serde_json::Value = serde_json::from_str(&bytes).expect("json");
    assert_eq!(value["auth_mode"], "chatgpt");
    assert!(value["OPENAI_API_KEY"].is_null());
    assert_eq!(value["tokens"]["access_token"], "access-token-123");
    assert_eq!(value["tokens"]["refresh_token"], "session-token-123");
    assert_eq!(value["tokens"]["account_id"], "acc-123");
    let id_token = value["tokens"]["id_token"].as_str().expect("id_token");
    assert_eq!(id_token.split('.').count(), 3);

    let mut tampered = session.clone();
    tampered.account_id = "acc-999".to_owned();
    assert!(matches!(
        validate_auth_json(bytes.as_bytes(), &tampered),
        Err(CodexAuthError::SessionInvalid)
    ));
}

#[test]
fn synthesizes_placeholder_refresh_token_without_session_token() {
    let mut session = session_fixture();
    session.session_token = None;
    let bytes = synthesize_auth_json(&session, NOW_MS);
    let value: serde_json::Value = serde_json::from_str(&bytes).expect("json");
    assert_eq!(value["tokens"]["refresh_token"], "placeholder");
}

#[test]
fn writes_replaces_and_restores_the_previous_file() {
    let home = TempDir::new().expect("home");
    let app_data = TempDir::new().expect("app data");
    fs::write(home.path().join(AUTH_FILE_NAME), b"ORIGINAL-CREDENTIAL").expect("seed");
    let store = store(home.path(), app_data.path());
    let session = session_fixture();

    let status = store.export(&session, NOW_MS).expect("export");
    assert!(status.credential_present);
    assert!(status.backup_available);
    assert!(!status.drifted);
    assert!(!status.expired);
    assert_eq!(status.exported_at_ms, Some(NOW_MS));
    assert_eq!(status.account_email.as_deref(), Some("te***@example.com"));
    assert_eq!(status.plan_type.as_deref(), Some("plus"));
    assert_eq!(status.expires_at_ms, Some(EXPIRES_MS));
    assert_ne!(
        fs::read(home.path().join(AUTH_FILE_NAME)).expect("written"),
        b"ORIGINAL-CREDENTIAL"
    );

    let restored = store.restore(NOW_MS + 1).expect("restore");
    assert!(restored.credential_present);
    // A restore puts the user's own file back: no export facts remain, so the
    // settings section returns to its idle state.
    assert_eq!(restored.account_email, None);
    assert_eq!(restored.exported_at_ms, None);
    assert_eq!(restored.expires_at_ms, None);
    assert_eq!(
        fs::read(home.path().join(AUTH_FILE_NAME)).expect("restored"),
        b"ORIGINAL-CREDENTIAL"
    );
}

#[test]
fn export_without_existing_file_creates_private_codex_home() {
    let home = TempDir::new().expect("home");
    let codex_home = home.path().join(".codex");
    let app_data = TempDir::new().expect("app data");
    let store = store(&codex_home, app_data.path());

    let status = store.export(&session_fixture(), NOW_MS).expect("export");
    assert!(status.credential_present);
    assert!(!status.backup_available);
}

#[test]
fn refuses_non_file_credential_store() {
    let home = TempDir::new().expect("home");
    let app_data = TempDir::new().expect("app data");
    fs::write(
        home.path().join("config.toml"),
        "cli_auth_credentials_store = \"keyring\"\n",
    )
    .expect("config");
    let store = store(home.path(), app_data.path());

    let error = store
        .export(&session_fixture(), NOW_MS)
        .expect_err("store mode");
    assert_eq!(error.code(), "codex_auth_store_mode_unsupported");
    assert!(!home.path().join(AUTH_FILE_NAME).exists());
}

#[test]
fn refuses_managed_locked_store() {
    let home = TempDir::new().expect("home");
    let app_data = TempDir::new().expect("app data");
    let managed = home.path().join("managed_config.toml");
    fs::write(&managed, "cli_auth_credentials_store = \"ephemeral\"\n").expect("managed");
    let store = store(home.path(), app_data.path()).with_managed_paths(vec![managed]);

    let error = store
        .export(&session_fixture(), NOW_MS)
        .expect_err("managed lock");
    assert_eq!(error.code(), "codex_auth_store_mode_unsupported");
    let status = store.status(NOW_MS);
    assert!(status.managed_locked);
    assert!(!status.store_mode_supported);
}

#[test]
fn missing_config_defaults_to_file_store() {
    let home = TempDir::new().expect("home");
    let status = read_credential_store_status(home.path(), &[]);
    assert_eq!(status.mode, CredentialStoreModeDto::File);
    assert!(status.supports_auth_file());
}

#[test]
fn refuses_a_symlinked_auth_file() {
    let home = TempDir::new().expect("home");
    let app_data = TempDir::new().expect("app data");
    let outside = home.path().join("outside.json");
    fs::write(&outside, b"outside").expect("outside");
    std::os::unix::fs::symlink(&outside, home.path().join(AUTH_FILE_NAME)).expect("symlink");
    let store = store(home.path(), app_data.path());

    let error = store
        .export(&session_fixture(), NOW_MS)
        .expect_err("symlink");
    assert_eq!(error.code(), "codex_auth_target_conflict");
    assert_eq!(fs::read(&outside).expect("outside"), b"outside");
}

#[test]
fn reports_fingerprint_drift_without_overwriting() {
    let home = TempDir::new().expect("home");
    let app_data = TempDir::new().expect("app data");
    let path = home.path().join(AUTH_FILE_NAME);
    fs::write(&path, b"first").expect("seed");
    let store = store(home.path(), app_data.path()).with_before_replace({
        let path = path.clone();
        move || fs::write(&path, b"external-edit").expect("external edit")
    });

    let error = store.export(&session_fixture(), NOW_MS).expect_err("drift");
    assert_eq!(error.code(), "codex_auth_target_conflict");
    assert_eq!(fs::read(&path).expect("kept"), b"external-edit");
}

#[test]
fn status_reports_an_expired_credential() {
    let home = TempDir::new().expect("home");
    let app_data = TempDir::new().expect("app data");
    let store = store(home.path(), app_data.path());
    store
        .export(&session_fixture(), EXPIRES_MS + 1)
        .expect("export");
    assert!(store.status(EXPIRES_MS + 2).expired);
}

#[test]
fn status_reports_external_drift_after_a_successful_export() {
    let home = TempDir::new().expect("home");
    let app_data = TempDir::new().expect("app data");
    let store = store(home.path(), app_data.path());
    store.export(&session_fixture(), NOW_MS).expect("export");
    assert!(!store.status(NOW_MS).drifted);

    fs::write(home.path().join(AUTH_FILE_NAME), b"tampered").expect("tamper");
    assert!(store.status(NOW_MS).drifted);
}

#[test]
fn retains_only_the_newest_backups() {
    let home = TempDir::new().expect("home");
    let app_data = TempDir::new().expect("app data");
    let store = store(home.path(), app_data.path());
    let steps = i64::try_from(MAX_BACKUPS).expect("backup count fits") + 3;
    for step in 0..steps {
        fs::write(home.path().join(AUTH_FILE_NAME), format!("file-{step}")).expect("seed");
        store
            .export(&session_fixture(), NOW_MS + step)
            .expect("export");
    }
    let backups = fs::read_dir(app_data.path().join("codex-auth-backups"))
        .expect("backups")
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("auth-"))
        .count();
    assert_eq!(backups, MAX_BACKUPS);
}

#[test]
fn restore_without_backup_fails_closed() {
    let home = TempDir::new().expect("home");
    let app_data = TempDir::new().expect("app data");
    let store = store(home.path(), app_data.path());
    let error = store.restore(NOW_MS).expect_err("no backup");
    assert_eq!(error.code(), "codex_auth_restore_failed");
}

#[test]
fn resolves_codex_home_with_environment_override() {
    let home = Path::new("/Users/tester");
    assert_eq!(
        resolve_codex_home(Some("/tmp/isolated"), home),
        Path::new("/tmp/isolated")
    );
    assert_eq!(resolve_codex_home(Some("  "), home), home.join(".codex"));
    assert_eq!(resolve_codex_home(None, home), home.join(".codex"));
}

#[test]
fn masks_emails_for_display() {
    assert_eq!(mask_email("xyz@gmail.com"), "xy***@gmail.com");
    assert_eq!(mask_email("ab@example.com"), "ab***@example.com");
    assert_eq!(mask_email("nonsense"), "***");
}

#[test]
fn discovery_sorts_profiles_and_selection_is_deterministic() {
    let home = TempDir::new().expect("home");
    let session = encrypt_for_test(TEST_KEY, ".chatgpt.com", b"chunk");
    for profile in ["Profile 1", "Default"] {
        create_cookie_db(
            home.path(),
            profile,
            &[(
                ".chatgpt.com",
                "__Secure-next-auth.session-token.0",
                session.clone(),
            )],
        );
    }

    let labels: Vec<String> = discover_profiles(home.path())
        .into_iter()
        .map(|candidate| candidate.label)
        .collect();
    assert_eq!(labels, vec!["Default".to_owned(), "Profile 1".to_owned()]);

    // With two signed-in profiles the first sorted label wins every time, so a
    // repeated export never flip-flops between accounts.
    for _ in 0..3 {
        assert_eq!(
            select_profile(home.path()).expect("profile").label,
            "Default"
        );
    }
}

#[test]
fn meta_write_failure_leaves_the_target_file_untouched() {
    let home = TempDir::new().expect("home");
    let app_data = TempDir::new().expect("app data");
    let path = home.path().join(AUTH_FILE_NAME);
    fs::write(&path, b"ORIGINAL-CREDENTIAL").expect("seed");
    // A directory where the meta file belongs makes the atomic meta publish
    // fail, so the credential must never be replaced without its record.
    fs::create_dir_all(
        app_data
            .path()
            .join("codex-auth-backups")
            .join(META_FILE_NAME),
    )
    .expect("blocked meta path");
    let store = store(home.path(), app_data.path());

    let error = store
        .export(&session_fixture(), NOW_MS)
        .expect_err("meta write");
    assert_eq!(error.code(), "codex_auth_write_failed");
    assert_eq!(fs::read(&path).expect("kept"), b"ORIGINAL-CREDENTIAL");
    assert!(
        fs::read_dir(home.path())
            .expect("home")
            .flatten()
            .all(|entry| !entry.file_name().to_string_lossy().ends_with(".tmp")),
        "the staged temporary file must be cleaned up"
    );
}

#[test]
fn status_drops_export_facts_when_the_credential_file_is_gone() {
    let home = TempDir::new().expect("home");
    let app_data = TempDir::new().expect("app data");
    let store = store(home.path(), app_data.path());
    let exported = store.export(&session_fixture(), NOW_MS).expect("export");
    assert_eq!(exported.exported_at_ms, Some(NOW_MS));
    assert_eq!(exported.account_email.as_deref(), Some("te***@example.com"));

    fs::remove_file(home.path().join(AUTH_FILE_NAME)).expect("remove credential");
    let status = store.status(NOW_MS);
    assert!(!status.credential_present);
    assert_eq!(status.exported_at_ms, None);
    assert_eq!(status.account_email, None);
    assert_eq!(status.expires_at_ms, None);
    assert!(!status.drifted);
    assert!(!status.expired);
}

#[test]
fn a_lookalike_cookie_name_is_not_a_session_cookie() {
    let home = TempDir::new().expect("home");
    create_cookie_db(
        home.path(),
        "Default",
        &[(
            ".chatgpt.com",
            // One character off: the real name starts with two underscores.
            "x_Secure-next-auth.session-token",
            encrypt_for_test(TEST_KEY, ".chatgpt.com", b"lookalike"),
        )],
    );

    assert!(matches!(
        select_profile(home.path()),
        Err(CodexAuthError::ProfileNotLoggedIn)
    ));
}

#[test]
fn cleared_session_cookie_values_report_not_logged_in() {
    let home = TempDir::new().expect("home");
    create_cookie_db(
        home.path(),
        "Default",
        &[(
            ".chatgpt.com",
            "__Secure-next-auth.session-token.0",
            Vec::new(),
        )],
    );
    let candidate = select_profile(home.path()).expect("profile");

    let error = read_session_cookies(&candidate, TEST_KEY).expect_err("cleared value");
    assert_eq!(error.code(), "codex_auth_profile_not_logged_in");
}
