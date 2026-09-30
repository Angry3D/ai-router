//! One-click Codex `auth.json` provisioning domain.
//!
//! This module owns everything that does not need a browser engine, the macOS
//! Keychain, or the UI:
//!
//! * Chromium-family browser and profile discovery (data-driven table).
//! * Read-only access to the live cookie database plus `v10` decryption given
//!   the Keychain-derived key.
//! * Session JSON classification and structural validation.
//! * Byte-compatible `auth.json` synthesis (reference algorithm, see
//!   `research/01-reference-implementation.md`).
//! * `$CODEX_HOME` ownership: credential store-mode gate, symlink refusal,
//!   backup, atomic replacement, post-write re-validation and restore.
//! * The stable error taxonomy the command layer maps to Chinese UI copy.
//!
//! Secrets (cookie values, synthesized tokens, the Keychain key) are held in
//! [`zeroize::Zeroizing`] buffers and never leave memory: there is no logging,
//! history, or recovery serialization in this module. Only a SHA-256 digest and
//! the non-secret session metadata (masked email, plan, expiry) are persisted,
//! so external file drift can be reported without re-reading the credential.

use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

use aes::Aes128;
use cbc::cipher::{BlockModeDecrypt, KeyIvInit, block_padding::Pkcs7};
use pbkdf2::pbkdf2_hmac;
use rusqlite::{Connection, OpenFlags, params};
use serde::{Deserialize, Serialize};
use sha1::Sha1;
use sha2::{Digest, Sha256};
use thiserror::Error;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use ts_rs::TS;
use uuid::Uuid;
use zeroize::Zeroizing;

/// File name of the Codex credential inside `$CODEX_HOME`.
pub const AUTH_FILE_NAME: &str = "auth.json";
/// Codex configuration file name inside `$CODEX_HOME`.
const CONFIG_FILE_NAME: &str = "config.toml";
/// Directory (under the app data directory) holding `auth.json` backups.
const BACKUP_DIR_NAME: &str = "codex-auth-backups";
/// Metadata file (digest + non-secret session facts) next to the backups.
const META_FILE_NAME: &str = "codex-auth-meta.json";
/// Maximum number of retained backups; the newest wins.
const MAX_BACKUPS: usize = 5;
/// Prefix of the Chrome session-token cookie chunks.
const SESSION_COOKIE_PREFIX: &str = "__Secure-next-auth.session-token";
/// Host fragment that identifies the `ChatGPT` cookie scope.
const CHATGPT_HOST_FRAGMENT: &str = "chatgpt.com";
/// Number of leading plaintext bytes that carry `SHA256(host_key)`.
const HOST_HASH_LEN: usize = 32;
/// `v10` scheme marker on `encrypted_value` blobs.
const V10_PREFIX: &[u8; 3] = b"v10";
/// Escape character for the session-cookie name pattern: `SQLite` treats `_` as
/// a single-character wildcard, and the session cookie names contain them.
const LIKE_ESCAPE: char = '\\';
/// Chrome's PBKDF2 salt.
const PBKDF2_SALT: &[u8] = b"saltysalt";
/// Chrome's PBKDF2 iteration count on macOS.
const PBKDF2_ROUNDS: u32 = 1003;
/// Derived AES-128 key length.
const PBKDF2_KEY_LEN: usize = 16;
/// Fixed AES-CBC IV (16 spaces) used by Chrome.
const CBC_IV: [u8; 16] = *b"                ";
/// Backup/meta file mode.
const PRIVATE_FILE_MODE: u32 = 0o600;
/// Newly created `$CODEX_HOME` and backup directory mode.
const PRIVATE_DIR_MODE: u32 = 0o700;

/// Stable error taxonomy; [`CodexAuthError::code`] is the IPC code stem.
#[derive(Debug, Error)]
pub enum CodexAuthError {
    #[error("no Chromium-family browser was found")]
    BrowserMissing,
    #[error("the detected browser is not supported by this build")]
    BrowserUnsupported,
    #[error("the browser cookie encryption scheme is not supported")]
    UnsupportedEncryption,
    #[error("the Keychain authorization was denied")]
    KeychainDenied,
    #[error("the browser cookie store could not be read")]
    CookieStoreUnreadable,
    #[error("no signed-in ChatGPT session was found in the browser")]
    ProfileNotLoggedIn,
    #[error("the ChatGPT session request failed")]
    SessionFetchFailed,
    #[error("the ChatGPT session response is missing required fields")]
    SessionInvalid,
    #[error("the Codex credential store does not use auth.json")]
    StoreModeUnsupported,
    #[error("the auth.json target changed or is not an ordinary file")]
    TargetConflict,
    #[error("writing auth.json failed")]
    WriteFailed,
    #[error("restoring auth.json failed")]
    RestoreFailed,
}

impl From<std::io::Error> for CodexAuthError {
    fn from(_error: std::io::Error) -> Self {
        Self::WriteFailed
    }
}

impl CodexAuthError {
    /// The stable IPC code; `frontend-design.md` maps it to Chinese UI copy.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::BrowserMissing => "codex_auth_browser_missing",
            Self::BrowserUnsupported => "codex_auth_browser_unsupported",
            Self::UnsupportedEncryption => "codex_auth_unsupported_encryption",
            Self::KeychainDenied => "codex_auth_keychain_denied",
            Self::CookieStoreUnreadable => "codex_auth_cookie_store_unreadable",
            Self::ProfileNotLoggedIn => "codex_auth_profile_not_logged_in",
            Self::SessionFetchFailed => "codex_auth_session_fetch_failed",
            Self::SessionInvalid => "codex_auth_session_invalid",
            Self::StoreModeUnsupported => "codex_auth_store_mode_unsupported",
            Self::TargetConflict => "codex_auth_target_conflict",
            Self::WriteFailed => "codex_auth_write_failed",
            Self::RestoreFailed => "codex_auth_restore_failed",
        }
    }

    /// Whether retrying the same one-click action can succeed.
    #[must_use]
    pub const fn retryable(&self) -> bool {
        !matches!(
            self,
            Self::BrowserMissing | Self::BrowserUnsupported | Self::UnsupportedEncryption
        )
    }
}

/// The Codex credential store selected by `cli_auth_credentials_store`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum CredentialStoreModeDto {
    File,
    Keyring,
    Auto,
    Ephemeral,
    Unknown,
}

impl CredentialStoreModeDto {
    /// Parses the configuration value; anything unknown fails closed.
    #[must_use]
    pub fn parse(value: &str) -> Self {
        match value {
            "file" => Self::File,
            "keyring" => Self::Keyring,
            "auto" => Self::Auto,
            "ephemeral" => Self::Ephemeral,
            _ => Self::Unknown,
        }
    }

    /// The literal configuration spelling, for UI copy.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Keyring => "keyring",
            Self::Auto => "auto",
            Self::Ephemeral => "ephemeral",
            Self::Unknown => "unknown",
        }
    }

    /// Whether this mode reads and writes `$CODEX_HOME/auth.json`.
    #[must_use]
    pub const fn is_file(self) -> bool {
        matches!(self, Self::File)
    }
}

/// The resolved credential-store gate for one `$CODEX_HOME`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CredentialStoreStatus {
    /// Effective mode (a non-`file` managed override wins).
    pub mode: CredentialStoreModeDto,
    /// Whether a managed configuration locks the store away from `file`.
    pub managed_locked: bool,
}

impl CredentialStoreStatus {
    /// Whether `auth.json` may be replaced without being ignored by Codex.
    #[must_use]
    pub const fn supports_auth_file(&self) -> bool {
        self.mode.is_file() && !self.managed_locked
    }
}

/// Session facts the settings section renders (never the raw tokens).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
#[expect(
    clippy::struct_excessive_bools,
    reason = "the status matrix needs independent presence, support, lock and drift flags"
)]
pub struct CodexAuthStatusDto {
    /// Whether `$CODEX_HOME/auth.json` currently exists as an ordinary file.
    pub credential_present: bool,
    /// Effective credential store mode.
    pub store_mode: CredentialStoreModeDto,
    /// Whether the store mode allows replacing `auth.json`.
    pub store_mode_supported: bool,
    /// Whether a managed configuration locks the store away from `file`.
    pub managed_locked: bool,
    /// Masked account email from the last export, never the raw address.
    pub account_email: Option<String>,
    /// `ChatGPT` plan type from the last export.
    pub plan_type: Option<String>,
    /// Credential expiry (derived from the fetched session `expires`).
    #[ts(type = "number | null")]
    pub expires_at_ms: Option<i64>,
    /// When AI Router last exported the credential; `None` before the first
    /// export, after a restore, and whenever the credential file is not on disk.
    #[ts(type = "number | null")]
    pub exported_at_ms: Option<i64>,
    /// Whether a previous `auth.json` is available to restore.
    pub backup_available: bool,
    /// Whether `auth.json` changed on disk since the last export.
    pub drifted: bool,
    /// Whether the exported credential is already past its session expiry.
    pub expired: bool,
}

/// One decrypted session cookie chunk ready for the offscreen `WebView`.
pub struct SessionCookie {
    /// Cookie name, e.g. `__Secure-next-auth.session-token.0`.
    pub name: String,
    /// Cookie host, e.g. `.chatgpt.com`.
    pub host_key: String,
    /// Decrypted cookie value (zeroized on drop).
    pub value: Zeroizing<Vec<u8>>,
}

impl std::fmt::Debug for SessionCookie {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never render the decrypted value, even in panic messages.
        formatter
            .debug_struct("SessionCookie")
            .field("name", &self.name)
            .field("host_key", &self.host_key)
            .field("value", &"<redacted>")
            .finish()
    }
}

/// One discovered browser profile whose cookie database exists.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProfileCandidate {
    /// Stable browser id used in diagnostics.
    pub browser_id: &'static str,
    /// Human-readable browser name for the fail-closed message.
    pub browser_name: &'static str,
    /// Whether this build ships a verified cookie/Keychain recipe.
    pub verified: bool,
    /// macOS Keychain service holding the browser's safe-storage key.
    pub keychain_service: &'static str,
    /// Profile directory name, e.g. `Default`.
    pub label: String,
    /// Path to the read-only cookie database.
    pub cookie_db: PathBuf,
}

/// A verified `ChatGPT` web session parsed from the session endpoint.
#[derive(Clone, Eq, PartialEq)]
pub struct SessionMaterial {
    /// OAuth-style access token (verbatim web token).
    pub access_token: Zeroizing<String>,
    /// Web session token, used as the `refresh_token` or `placeholder`.
    pub session_token: Option<Zeroizing<String>>,
    /// `session.account.id`.
    pub account_id: String,
    /// `session.account.planType`, when present.
    pub plan_type: Option<String>,
    /// `session.user.id`.
    pub user_id: String,
    /// `session.user.email`, when present.
    pub email: Option<String>,
    /// `session.expires` as Unix milliseconds.
    pub expires_at_ms: i64,
}

impl std::fmt::Debug for SessionMaterial {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never render tokens, even in panic messages.
        formatter
            .debug_struct("SessionMaterial")
            .field("access_token", &"<redacted>")
            .field(
                "session_token",
                &self.session_token.as_ref().map(|_| "<redacted>"),
            )
            .field("account_id", &self.account_id)
            .field("plan_type", &self.plan_type)
            .field("user_id", &self.user_id)
            .field("email", &self.email)
            .field("expires_at_ms", &self.expires_at_ms)
            .finish()
    }
}

/// The classification of one session-endpoint body.
#[derive(Debug)]
pub enum SessionOutcome {
    /// A complete `ChatGPT` session.
    Authenticated(Box<SessionMaterial>),
    /// Valid JSON with no `accessToken`: the browser is not signed in.
    NotLoggedIn,
    /// Not JSON (challenge page, network error) or an unexpected shape.
    FetchFailed,
    /// JSON with an `accessToken` but missing other required fields.
    Invalid,
}

/// Browser discovery definitions.
struct BrowserDefinition {
    id: &'static str,
    name: &'static str,
    data_dir: &'static str,
    keychain_service: &'static str,
    verified: bool,
}

const BROWSERS: &[BrowserDefinition] = &[
    BrowserDefinition {
        id: "chrome",
        name: "Google Chrome",
        data_dir: "Library/Application Support/Google/Chrome",
        keychain_service: "Chrome Safe Storage",
        verified: true,
    },
    BrowserDefinition {
        id: "chromium",
        name: "Chromium",
        data_dir: "Library/Application Support/Chromium",
        keychain_service: "Chromium Safe Storage",
        verified: false,
    },
    BrowserDefinition {
        id: "edge",
        name: "Microsoft Edge",
        data_dir: "Library/Application Support/Microsoft Edge",
        keychain_service: "Microsoft Edge Safe Storage",
        verified: false,
    },
    BrowserDefinition {
        id: "brave",
        name: "Brave",
        data_dir: "Library/Application Support/BraveSoftware/Brave-Browser",
        keychain_service: "Brave Safe Storage",
        verified: false,
    },
    BrowserDefinition {
        id: "vivaldi",
        name: "Vivaldi",
        data_dir: "Library/Application Support/Vivaldi",
        keychain_service: "Vivaldi Safe Storage",
        verified: false,
    },
    BrowserDefinition {
        id: "opera",
        name: "Opera",
        data_dir: "Library/Application Support/com.operasoftware.Opera",
        keychain_service: "Opera Safe Storage",
        verified: false,
    },
    BrowserDefinition {
        id: "arc",
        name: "Arc",
        data_dir: "Library/Application Support/Arc",
        keychain_service: "Arc Safe Storage",
        verified: false,
    },
];

/// Resolves `$CODEX_HOME`: the environment override else `~/.codex`.
#[must_use]
pub fn resolve_codex_home(codex_home_env: Option<&str>, user_home: &Path) -> PathBuf {
    match codex_home_env.map(str::trim) {
        Some(value) if !value.is_empty() => PathBuf::from(value),
        _ => user_home.join(".codex"),
    }
}

/// Managed configuration paths that can lock the credential store.
#[must_use]
pub fn default_managed_config_paths() -> Vec<PathBuf> {
    vec![
        PathBuf::from("/Library/Application Support/Codex/managed_config.toml"),
        PathBuf::from("/etc/codex/managed_config.toml"),
    ]
}

/// Reads the effective credential-store gate for one `$CODEX_HOME`.
#[must_use]
pub fn read_credential_store_status(
    codex_home: &Path,
    managed_config_paths: &[PathBuf],
) -> CredentialStoreStatus {
    let user_mode = read_store_mode_file(&codex_home.join(CONFIG_FILE_NAME))
        .unwrap_or(CredentialStoreModeDto::File);
    let mut status = CredentialStoreStatus {
        mode: user_mode,
        managed_locked: false,
    };
    for path in managed_config_paths {
        if let Some(managed_mode) = read_store_mode_file(path)
            && !managed_mode.is_file()
        {
            status.mode = managed_mode;
            status.managed_locked = true;
        }
    }
    status
}

fn read_store_mode_file(path: &Path) -> Option<CredentialStoreModeDto> {
    let bytes = fs::read(path).ok()?;
    let document = std::str::from_utf8(&bytes)
        .ok()?
        .parse::<toml_edit::DocumentMut>()
        .ok()?;
    document
        .get("cli_auth_credentials_store")
        .and_then(toml_edit::Item::as_str)
        .map(CredentialStoreModeDto::parse)
}

/// Masks an account email for display; the raw address is never rendered.
#[must_use]
pub fn mask_email(email: &str) -> String {
    email.split_once('@').map_or_else(
        || "***".to_owned(),
        |(local, domain)| {
            let prefix: String = local.chars().take(2).collect();
            format!("{prefix}***@{domain}")
        },
    )
}

/// Enumerates browser profiles whose cookie database is present.
#[must_use]
pub fn discover_profiles(user_home: &Path) -> Vec<ProfileCandidate> {
    let mut candidates = Vec::new();
    for browser in BROWSERS {
        let root = user_home.join(browser.data_dir);
        if !root.is_dir() {
            continue;
        }
        let Ok(entries) = fs::read_dir(&root) else {
            continue;
        };
        let mut profile_names: Vec<String> = entries
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
            .filter_map(|entry| entry.file_name().into_string().ok())
            .collect();
        profile_names.sort();
        for label in profile_names {
            let profile_dir = root.join(&label);
            let Some(cookie_db) = cookie_database(&profile_dir) else {
                continue;
            };
            candidates.push(ProfileCandidate {
                browser_id: browser.id,
                browser_name: browser.name,
                verified: browser.verified,
                keychain_service: browser.keychain_service,
                label,
                cookie_db,
            });
        }
    }
    candidates
}

fn cookie_database(profile_dir: &Path) -> Option<PathBuf> {
    let network = profile_dir.join("Network").join("Cookies");
    if network.is_file() {
        return Some(network);
    }
    let legacy = profile_dir.join("Cookies");
    legacy.is_file().then_some(legacy)
}

/// Selects the profile to export from, failing closed on every ambiguity.
///
/// # Errors
///
/// Returns [`CodexAuthError::BrowserMissing`] when no browser data directory
/// exists, [`CodexAuthError::BrowserUnsupported`] when only unverified browsers
/// are present, [`CodexAuthError::CookieStoreUnreadable`] when every verified
/// cookie database is unreadable, and
/// [`CodexAuthError::ProfileNotLoggedIn`] when no verified profile carries a
/// `ChatGPT` session cookie.
pub fn select_profile(user_home: &Path) -> Result<ProfileCandidate, CodexAuthError> {
    let candidates = discover_profiles(user_home);
    select_from_candidates(&candidates)
}

fn select_from_candidates(
    candidates: &[ProfileCandidate],
) -> Result<ProfileCandidate, CodexAuthError> {
    let mut unreadable = false;
    for candidate in candidates.iter().filter(|entry| entry.verified) {
        match has_session_cookie(&candidate.cookie_db) {
            Ok(true) => return Ok(candidate.clone()),
            Ok(false) => {}
            Err(_) => unreadable = true,
        }
    }
    if candidates.iter().any(|entry| entry.verified) {
        return Err(if unreadable {
            CodexAuthError::CookieStoreUnreadable
        } else {
            CodexAuthError::ProfileNotLoggedIn
        });
    }
    if candidates.is_empty() {
        Err(CodexAuthError::BrowserMissing)
    } else {
        Err(CodexAuthError::BrowserUnsupported)
    }
}

fn open_cookie_store(path: &Path) -> Result<Connection, CodexAuthError> {
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|_| CodexAuthError::CookieStoreUnreadable)
}

/// Builds the `LIKE` pattern for the session cookie names.
///
/// The names carry their own `_` characters, so every wildcard metacharacter is
/// escaped and the queries pair the pattern with `LIKE_ESCAPE`; a lookalike name
/// such as `x_Secure-next-auth.session-token` must never be selected.
fn session_cookie_pattern() -> String {
    let mut pattern = String::with_capacity(SESSION_COOKIE_PREFIX.len() + 1);
    for character in SESSION_COOKIE_PREFIX.chars() {
        if matches!(character, '%' | '_' | LIKE_ESCAPE) {
            pattern.push(LIKE_ESCAPE);
        }
        pattern.push(character);
    }
    pattern.push('%');
    pattern
}

fn has_session_cookie(path: &Path) -> Result<bool, CodexAuthError> {
    let connection = open_cookie_store(path)?;
    let pattern = session_cookie_pattern();
    let host = format!("%{CHATGPT_HOST_FRAGMENT}");
    let mut statement = connection
        .prepare(
            "SELECT 1 FROM cookies WHERE host_key LIKE ?1 AND name LIKE ?2 ESCAPE '\\' LIMIT 1",
        )
        .map_err(|_| CodexAuthError::CookieStoreUnreadable)?;
    statement
        .exists(params![host, pattern])
        .map_err(|_| CodexAuthError::CookieStoreUnreadable)
}

/// Reads and decrypts the `ChatGPT` session-token cookie chunks.
///
/// Every sampled `encrypted_value` must carry the verified `v10` scheme; an
/// unknown prefix fails closed so a future browser change cannot silently
/// corrupt the output. A cleared (empty) value carries no session material and
/// is reported as not logged in.
///
/// # Errors
///
/// Returns [`CodexAuthError::CookieStoreUnreadable`] when the database or query
/// fails, [`CodexAuthError::UnsupportedEncryption`] on an unknown scheme or a
/// host-hash mismatch, and [`CodexAuthError::ProfileNotLoggedIn`] when no
/// usable session cookie is present.
pub fn read_session_cookies(
    candidate: &ProfileCandidate,
    key: &[u8],
) -> Result<Vec<SessionCookie>, CodexAuthError> {
    let connection = open_cookie_store(&candidate.cookie_db)?;
    let host = format!("%{CHATGPT_HOST_FRAGMENT}");
    let pattern = session_cookie_pattern();
    let mut statement = connection
        .prepare(
            "SELECT host_key, name, encrypted_value FROM cookies \
             WHERE host_key LIKE ?1 AND name LIKE ?2 ESCAPE '\\' ORDER BY name",
        )
        .map_err(|_| CodexAuthError::CookieStoreUnreadable)?;
    let rows = statement
        .query_map(params![host, pattern], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Vec<u8>>(2)?,
            ))
        })
        .map_err(|_| CodexAuthError::CookieStoreUnreadable)?;
    let derived = derive_chrome_key(key);
    let mut cookies = Vec::new();
    for row in rows {
        let (host_key, name, encrypted_value) =
            row.map_err(|_| CodexAuthError::CookieStoreUnreadable)?;
        // A cleared session cookie keeps its row with an empty ciphertext until
        // the browser collects it; that is no session material, not a scheme we
        // failed to understand.
        if encrypted_value.is_empty() {
            return Err(CodexAuthError::ProfileNotLoggedIn);
        }
        if !encrypted_value.starts_with(V10_PREFIX) {
            return Err(CodexAuthError::UnsupportedEncryption);
        }
        let value = decrypt_v10(&derived, &encrypted_value, &host_key)?;
        cookies.push(SessionCookie {
            name,
            host_key,
            value,
        });
    }
    if cookies.is_empty() {
        return Err(CodexAuthError::ProfileNotLoggedIn);
    }
    Ok(cookies)
}

fn derive_chrome_key(password: &[u8]) -> Zeroizing<[u8; PBKDF2_KEY_LEN]> {
    let mut key = Zeroizing::new([0_u8; PBKDF2_KEY_LEN]);
    pbkdf2_hmac::<Sha1>(password, PBKDF2_SALT, PBKDF2_ROUNDS, key.as_mut());
    key
}

fn decrypt_v10(
    key: &[u8; PBKDF2_KEY_LEN],
    encrypted: &[u8],
    host_key: &str,
) -> Result<Zeroizing<Vec<u8>>, CodexAuthError> {
    let ciphertext = encrypted
        .get(V10_PREFIX.len()..)
        .filter(|bytes| !bytes.is_empty() && bytes.len() % 16 == 0)
        .ok_or(CodexAuthError::UnsupportedEncryption)?;
    let mut buffer = Zeroizing::new(ciphertext.to_vec());
    let plaintext = cbc::Decryptor::<Aes128>::new_from_slices(key, &CBC_IV)
        .map_err(|_| CodexAuthError::UnsupportedEncryption)?
        .decrypt_padded::<Pkcs7>(buffer.as_mut_slice())
        .map_err(|_| CodexAuthError::UnsupportedEncryption)?;
    let expected = Sha256::digest(host_key.as_bytes());
    if plaintext.len() < HOST_HASH_LEN || plaintext[..HOST_HASH_LEN] != expected[..HOST_HASH_LEN] {
        return Err(CodexAuthError::UnsupportedEncryption);
    }
    Ok(Zeroizing::new(plaintext[HOST_HASH_LEN..].to_vec()))
}

/// Classifies one session-endpoint body without touching the credential.
#[must_use]
pub fn classify_session_body(body: &str) -> SessionOutcome {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(body) else {
        return SessionOutcome::FetchFailed;
    };
    let Some(access_token) = value.get("accessToken").and_then(|entry| entry.as_str()) else {
        return SessionOutcome::NotLoggedIn;
    };
    if access_token.is_empty() {
        return SessionOutcome::NotLoggedIn;
    }
    let Some(account_id) = value
        .get("account")
        .and_then(|entry| entry.get("id"))
        .and_then(|entry| entry.as_str())
        .filter(|entry| !entry.is_empty())
    else {
        return SessionOutcome::Invalid;
    };
    let Some(user_id) = value
        .get("user")
        .and_then(|entry| entry.get("id"))
        .and_then(|entry| entry.as_str())
        .filter(|entry| !entry.is_empty())
    else {
        return SessionOutcome::Invalid;
    };
    let Some(expires_at_ms) = value
        .get("expires")
        .and_then(|entry| entry.as_str())
        .and_then(parse_iso8601_ms)
    else {
        return SessionOutcome::Invalid;
    };
    let session_token = value
        .get("sessionToken")
        .and_then(|entry| entry.as_str())
        .filter(|entry| !entry.is_empty())
        .map(|entry| Zeroizing::new(entry.to_owned()));
    let plan_type = value
        .get("account")
        .and_then(|entry| entry.get("planType"))
        .and_then(|entry| entry.as_str())
        .filter(|entry| !entry.is_empty())
        .map(str::to_owned);
    let email = value
        .get("user")
        .and_then(|entry| entry.get("email"))
        .and_then(|entry| entry.as_str())
        .filter(|entry| !entry.is_empty())
        .map(str::to_owned);
    SessionOutcome::Authenticated(Box::new(SessionMaterial {
        access_token: Zeroizing::new(access_token.to_owned()),
        session_token,
        account_id: account_id.to_owned(),
        plan_type,
        user_id: user_id.to_owned(),
        email,
        expires_at_ms,
    }))
}

fn parse_iso8601_ms(value: &str) -> Option<i64> {
    let parsed = OffsetDateTime::parse(value, &Rfc3339).ok()?;
    i64::try_from(parsed.unix_timestamp_nanos() / 1_000_000).ok()
}

fn format_iso8601_ms(value: i64) -> String {
    OffsetDateTime::from_unix_timestamp_nanos(i128::from(value) * 1_000_000)
        .ok()
        .and_then(|instant| instant.format(&Rfc3339).ok())
        .unwrap_or_else(|| "1970-01-01T00:00:00Z".to_owned())
}

#[derive(Serialize)]
struct AuthTokenSet<'a> {
    id_token: &'a str,
    access_token: &'a str,
    refresh_token: &'a str,
    account_id: &'a str,
}

#[derive(Serialize)]
struct AuthFile<'a> {
    auth_mode: &'a str,
    #[serde(rename = "OPENAI_API_KEY")]
    openai_api_key: Option<&'a str>,
    tokens: AuthTokenSet<'a>,
    last_refresh: &'a str,
}

/// Synthesizes the Codex `auth.json` payload (reference algorithm).
#[must_use]
pub fn synthesize_auth_json(session: &SessionMaterial, now_ms: i64) -> Zeroizing<String> {
    let header = base64_url_no_pad(br#"{"alg":"none","typ":"JWT","cpa_synthetic":true}"#);
    let payload_json = format!(
        concat!(
            "{{\"iat\":{iat},\"exp\":{exp},",
            "\"https://api.openai.com/auth\":{{",
            "\"chatgpt_account_id\":{account},\"chatgpt_plan_type\":{plan},",
            "\"chatgpt_user_id\":{user},\"user_id\":{user}}},",
            "\"email\":{email}}}"
        ),
        iat = now_ms / 1000,
        exp = session.expires_at_ms / 1000,
        account = json_string(&session.account_id),
        plan = json_string(session.plan_type.as_deref().unwrap_or("")),
        user = json_string(&session.user_id),
        email = json_string(session.email.as_deref().unwrap_or("")),
    );
    let payload = base64_url_no_pad(payload_json.as_bytes());
    let id_token = format!("{header}.{payload}.synthetic");
    let refresh_token = session
        .session_token
        .as_deref()
        .map_or("placeholder", String::as_str);
    let last_refresh = format_iso8601_ms(now_ms);
    let document = AuthFile {
        auth_mode: "chatgpt",
        openai_api_key: None,
        tokens: AuthTokenSet {
            id_token: &id_token,
            access_token: session.access_token.as_str(),
            refresh_token,
            account_id: &session.account_id,
        },
        last_refresh: &last_refresh,
    };
    Zeroizing::new(serde_json::to_string(&document).unwrap_or_default())
}

fn json_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_owned())
}

fn base64_url_no_pad(bytes: &[u8]) -> String {
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Structurally validates synthesized `auth.json` bytes against the session.
///
/// # Errors
///
/// Returns [`CodexAuthError::SessionInvalid`] when any required field, the
/// three-segment `id_token`, or the `account_id`/`expires` mapping is wrong.
pub fn validate_auth_json(bytes: &[u8], session: &SessionMaterial) -> Result<(), CodexAuthError> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| CodexAuthError::SessionInvalid)?;
    if value.get("auth_mode").and_then(|entry| entry.as_str()) != Some("chatgpt") {
        return Err(CodexAuthError::SessionInvalid);
    }
    if !value
        .get("OPENAI_API_KEY")
        .is_some_and(serde_json::Value::is_null)
    {
        return Err(CodexAuthError::SessionInvalid);
    }
    if value
        .get("last_refresh")
        .and_then(|entry| entry.as_str())
        .is_none_or(str::is_empty)
    {
        return Err(CodexAuthError::SessionInvalid);
    }
    let tokens = value.get("tokens").ok_or(CodexAuthError::SessionInvalid)?;
    let non_empty = |key: &str| {
        tokens
            .get(key)
            .and_then(|entry| entry.as_str())
            .is_some_and(|entry| !entry.is_empty())
    };
    if !non_empty("access_token") || !non_empty("refresh_token") || !non_empty("id_token") {
        return Err(CodexAuthError::SessionInvalid);
    }
    if tokens.get("account_id").and_then(|entry| entry.as_str())
        != Some(session.account_id.as_str())
    {
        return Err(CodexAuthError::SessionInvalid);
    }
    let id_token = tokens
        .get("id_token")
        .and_then(|entry| entry.as_str())
        .ok_or(CodexAuthError::SessionInvalid)?;
    let mut segments = id_token.split('.');
    let (Some(header), Some(payload), Some(signature), None) = (
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
    ) else {
        return Err(CodexAuthError::SessionInvalid);
    };
    if header.is_empty() || payload.is_empty() || signature.is_empty() {
        return Err(CodexAuthError::SessionInvalid);
    }
    let decoded = decode_base64_url(payload).ok_or(CodexAuthError::SessionInvalid)?;
    let claims: serde_json::Value =
        serde_json::from_slice(&decoded).map_err(|_| CodexAuthError::SessionInvalid)?;
    if claims.get("exp").and_then(serde_json::Value::as_i64) != Some(session.expires_at_ms / 1000) {
        return Err(CodexAuthError::SessionInvalid);
    }
    let account = claims
        .get("https://api.openai.com/auth")
        .and_then(|entry| entry.get("chatgpt_account_id"))
        .and_then(|entry| entry.as_str());
    if account != Some(session.account_id.as_str()) {
        return Err(CodexAuthError::SessionInvalid);
    }
    Ok(())
}

fn decode_base64_url(value: &str) -> Option<Vec<u8>> {
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    URL_SAFE_NO_PAD.decode(value).ok()
}

/// Metadata persisted next to the backups (never contains secret material).
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct ExportMeta {
    /// When AI Router last exported a credential; `None` after a restore.
    exported_at_ms: Option<i64>,
    expires_at_ms: Option<i64>,
    account_email: Option<String>,
    plan_type: Option<String>,
    file_sha256: String,
    backup_file: Option<String>,
}

/// Owns `$CODEX_HOME/auth.json`, its backups and its non-secret metadata.
pub struct CodexAuthStore {
    codex_home: PathBuf,
    backup_dir: PathBuf,
    managed_config_paths: Vec<PathBuf>,
    #[cfg(test)]
    before_replace: Option<std::sync::Arc<dyn Fn() + Send + Sync>>,
}

impl CodexAuthStore {
    /// Builds a store for one `$CODEX_HOME` and app data directory.
    #[must_use]
    pub fn new(codex_home: PathBuf, app_data_dir: &Path) -> Self {
        Self {
            codex_home,
            backup_dir: app_data_dir.join(BACKUP_DIR_NAME),
            // Unit tests inject their own managed paths so they never read the
            // host's system-wide Codex configuration.
            #[cfg(not(test))]
            managed_config_paths: default_managed_config_paths(),
            #[cfg(test)]
            managed_config_paths: Vec::new(),
            #[cfg(test)]
            before_replace: None,
        }
    }

    #[cfg(test)]
    fn with_before_replace(mut self, hook: impl Fn() + Send + Sync + 'static) -> Self {
        self.before_replace = Some(std::sync::Arc::new(hook));
        self
    }

    #[cfg(test)]
    fn with_managed_paths(mut self, paths: Vec<PathBuf>) -> Self {
        self.managed_config_paths = paths;
        self
    }

    #[cfg(test)]
    fn run_before_replace(&self) {
        if let Some(hook) = &self.before_replace {
            hook();
        }
    }

    #[cfg(not(test))]
    fn run_before_replace(&self) {
        let _ = &self.codex_home;
    }

    fn auth_path(&self) -> PathBuf {
        self.codex_home.join(AUTH_FILE_NAME)
    }

    fn meta_path(&self) -> PathBuf {
        self.backup_dir.join(META_FILE_NAME)
    }

    /// Projects the status the settings section renders; never fails.
    #[must_use]
    pub fn status(&self, now_ms: i64) -> CodexAuthStatusDto {
        let store = read_credential_store_status(&self.codex_home, &self.managed_config_paths);
        let meta = read_meta(&self.meta_path());
        let snapshot = read_auth_snapshot(&self.codex_home, &self.auth_path());
        let credential_present = matches!(&snapshot, Ok(entry) if entry.exists);
        // Export facts describe the credential AI Router wrote. Without that
        // file on disk they are stale, so the section falls back to its idle
        // state instead of reporting a successful export.
        let exported = match (&snapshot, meta.as_ref()) {
            (Ok(entry), Some(meta)) if entry.exists => Some(meta),
            _ => None,
        };
        let drifted = match (&snapshot, exported) {
            (Ok(entry), Some(meta)) => hex::encode(entry.digest) != meta.file_sha256,
            _ => false,
        };
        CodexAuthStatusDto {
            credential_present,
            store_mode: store.mode,
            store_mode_supported: store.supports_auth_file(),
            managed_locked: store.managed_locked,
            account_email: exported.and_then(|entry| entry.account_email.clone()),
            plan_type: exported.and_then(|entry| entry.plan_type.clone()),
            expires_at_ms: exported.and_then(|entry| entry.expires_at_ms),
            exported_at_ms: exported.and_then(|entry| entry.exported_at_ms),
            backup_available: self.latest_backup().is_some(),
            drifted,
            expired: exported
                .and_then(|entry| entry.expires_at_ms)
                .is_some_and(|expires| expires <= now_ms),
        }
    }

    /// Guards a write: refuses anything that could silently corrupt Codex.
    ///
    /// # Errors
    ///
    /// Returns [`CodexAuthError::StoreModeUnsupported`] when
    /// `cli_auth_credentials_store` is not `file` (or a managed configuration
    /// locks it), and [`CodexAuthError::TargetConflict`] when `$CODEX_HOME` or
    /// `auth.json` is a symlink or not an ordinary file.
    pub fn preflight(&self) -> Result<CredentialStoreStatus, CodexAuthError> {
        let store = read_credential_store_status(&self.codex_home, &self.managed_config_paths);
        if !store.supports_auth_file() {
            return Err(CodexAuthError::StoreModeUnsupported);
        }
        let snapshot = read_auth_snapshot(&self.codex_home, &self.auth_path())?;
        if snapshot.exists && !snapshot.is_regular_file {
            return Err(CodexAuthError::TargetConflict);
        }
        Ok(store)
    }

    /// Synthesizes, backs up and atomically replaces `auth.json`.
    ///
    /// # Errors
    ///
    /// Returns the credential-store gate, target-conflict, or write errors;
    /// every failure leaves the original file untouched and writes nothing.
    pub fn export(
        &self,
        session: &SessionMaterial,
        now_ms: i64,
    ) -> Result<CodexAuthStatusDto, CodexAuthError> {
        self.preflight()?;
        let bytes = synthesize_auth_json(session, now_ms);
        validate_auth_json(bytes.as_bytes(), session)?;
        let existing = read_auth_snapshot(&self.codex_home, &self.auth_path())?;
        let backup_file = if existing.exists {
            Some(self.write_backup(existing.bytes.as_slice(), now_ms)?)
        } else {
            None
        };
        let meta = ExportMeta {
            exported_at_ms: Some(now_ms),
            expires_at_ms: Some(session.expires_at_ms),
            account_email: session.email.as_deref().map(mask_email),
            plan_type: session.plan_type.clone(),
            file_sha256: hex::encode(Sha256::digest(bytes.as_bytes())),
            backup_file,
        };
        // The fingerprint record is committed before the target is replaced: a
        // meta failure must abort the export, never leave a replaced credential
        // without the record that detects later external edits.
        self.write_auth_bytes(bytes.as_bytes(), || self.write_meta(&meta))?;
        let written =
            Zeroizing::new(fs::read(self.auth_path()).map_err(|_| CodexAuthError::WriteFailed)?);
        validate_auth_json(written.as_slice(), session)?;
        Ok(self.status(now_ms))
    }

    /// Restores the newest backup over `auth.json`, then clears account facts.
    ///
    /// # Errors
    ///
    /// Returns [`CodexAuthError::StoreModeUnsupported`] on a non-`file` store,
    /// [`CodexAuthError::RestoreFailed`] when no backup exists or it is
    /// unreadable, and the write errors otherwise; the original file is kept on
    /// failure.
    pub fn restore(&self, now_ms: i64) -> Result<CodexAuthStatusDto, CodexAuthError> {
        self.preflight()?;
        let (path, _) = self.latest_backup().ok_or(CodexAuthError::RestoreFailed)?;
        let bytes = Zeroizing::new(fs::read(&path).map_err(|_| CodexAuthError::RestoreFailed)?);
        let meta = ExportMeta {
            exported_at_ms: None,
            expires_at_ms: None,
            account_email: None,
            plan_type: None,
            file_sha256: hex::encode(Sha256::digest(bytes.as_slice())),
            backup_file: None,
        };
        self.write_auth_bytes(bytes.as_slice(), || self.write_meta(&meta))?;
        Ok(self.status(now_ms))
    }

    fn write_backup(&self, bytes: &[u8], now_ms: i64) -> Result<String, CodexAuthError> {
        fs::create_dir_all(&self.backup_dir).map_err(|_| CodexAuthError::WriteFailed)?;
        set_permissions(&self.backup_dir, PRIVATE_DIR_MODE)
            .map_err(|_| CodexAuthError::WriteFailed)?;
        let digest = hex::encode(Sha256::digest(bytes));
        let short = digest.get(..8).unwrap_or("00000000");
        let name = format!("auth-{now_ms}-{short}.json");
        let path = self.backup_dir.join(&name);
        write_private_file(&path, bytes)?;
        self.prune_backups();
        Ok(name)
    }

    fn latest_backup(&self) -> Option<(PathBuf, i64)> {
        let mut best: Option<(PathBuf, i64)> = None;
        for entry in fs::read_dir(&self.backup_dir).ok()?.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(timestamp) = backup_timestamp(&name) else {
                continue;
            };
            if best
                .as_ref()
                .is_none_or(|(_, current)| timestamp > *current)
            {
                best = Some((entry.path(), timestamp));
            }
        }
        best
    }

    fn prune_backups(&self) {
        let Ok(entries) = fs::read_dir(&self.backup_dir) else {
            return;
        };
        let mut named: Vec<(PathBuf, i64)> = entries
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                backup_timestamp(&name).map(|timestamp| (entry.path(), timestamp))
            })
            .collect();
        if named.len() <= MAX_BACKUPS {
            return;
        }
        named.sort_by_key(|entry| std::cmp::Reverse(entry.1));
        for (path, _) in named.into_iter().skip(MAX_BACKUPS) {
            let _ = fs::remove_file(path);
        }
    }

    fn write_meta(&self, meta: &ExportMeta) -> Result<(), CodexAuthError> {
        fs::create_dir_all(&self.backup_dir).map_err(|_| CodexAuthError::WriteFailed)?;
        set_permissions(&self.backup_dir, PRIVATE_DIR_MODE)
            .map_err(|_| CodexAuthError::WriteFailed)?;
        let bytes = serde_json::to_vec(meta).map_err(|_| CodexAuthError::WriteFailed)?;
        write_private_file(&self.meta_path(), &bytes)
    }

    /// Single-attempt atomic replacement with re-validation before the rename.
    ///
    /// `before_rename` runs once the staged bytes are verified and the target
    /// has been re-checked for drift, but before the rename publishes them; a
    /// failure there aborts with the target file untouched.
    fn write_auth_bytes(
        &self,
        bytes: &[u8],
        before_rename: impl FnOnce() -> Result<(), CodexAuthError>,
    ) -> Result<(), CodexAuthError> {
        ensure_normal_directory(&self.codex_home)?;
        let path = self.auth_path();
        let expected = read_auth_snapshot(&self.codex_home, &path)?;
        if expected.exists && !expected.is_regular_file {
            return Err(CodexAuthError::TargetConflict);
        }
        let temporary = self
            .codex_home
            .join(format!(".auth.json.ai-router-{}.tmp", Uuid::new_v4()));
        let result = (|| {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            set_open_mode(&mut options, PRIVATE_FILE_MODE);
            let mut file = options.open(&temporary)?;
            set_permissions(&temporary, PRIVATE_FILE_MODE)?;
            file.write_all(bytes)?;
            file.sync_all()?;
            let mut verified = Zeroizing::new(Vec::new());
            File::open(&temporary)?.read_to_end(verified.as_mut())?;
            if verified.as_slice() != bytes {
                return Err(CodexAuthError::WriteFailed);
            }
            self.run_before_replace();
            let current = read_auth_snapshot(&self.codex_home, &path)?;
            if !current.matches(&expected) {
                return Err(CodexAuthError::TargetConflict);
            }
            before_rename()?;
            fs::rename(&temporary, &path)?;
            sync_directory(&self.codex_home)?;
            let final_snapshot = read_auth_snapshot(&self.codex_home, &path)?;
            if !final_snapshot.exists || final_snapshot.bytes.as_slice() != bytes {
                return Err(CodexAuthError::WriteFailed);
            }
            Ok(())
        })();
        if temporary.exists() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }
}

struct AuthFileSnapshot {
    exists: bool,
    is_regular_file: bool,
    bytes: Zeroizing<Vec<u8>>,
    digest: [u8; 32],
    length: u64,
    file_identity: Option<(u64, u64)>,
}

impl AuthFileSnapshot {
    fn matches(&self, other: &Self) -> bool {
        self.exists == other.exists
            && self.digest == other.digest
            && self.length == other.length
            && self.file_identity == other.file_identity
    }
}

fn backup_timestamp(name: &str) -> Option<i64> {
    let rest = name.strip_prefix("auth-")?.strip_suffix(".json")?;
    let (timestamp, _) = rest.split_once('-')?;
    timestamp.parse::<i64>().ok()
}

fn read_auth_snapshot(home: &Path, path: &Path) -> Result<AuthFileSnapshot, CodexAuthError> {
    if home.exists() {
        let metadata = fs::symlink_metadata(home).map_err(|_| CodexAuthError::WriteFailed)?;
        if metadata.file_type().is_symlink() {
            return Err(CodexAuthError::TargetConflict);
        }
    }
    if !path.exists() {
        return Ok(AuthFileSnapshot {
            exists: false,
            is_regular_file: false,
            bytes: Zeroizing::new(Vec::new()),
            digest: Sha256::digest(b"").into(),
            length: 0,
            file_identity: None,
        });
    }
    let metadata = fs::symlink_metadata(path).map_err(|_| CodexAuthError::WriteFailed)?;
    if metadata.file_type().is_symlink() {
        return Err(CodexAuthError::TargetConflict);
    }
    let is_regular_file = metadata.is_file();
    let bytes = if is_regular_file {
        fs::read(path).map_err(|_| CodexAuthError::WriteFailed)?
    } else {
        Vec::new()
    };
    Ok(AuthFileSnapshot {
        exists: true,
        is_regular_file,
        digest: Sha256::digest(&bytes).into(),
        length: bytes.len() as u64,
        file_identity: file_identity(&metadata),
        bytes: Zeroizing::new(bytes),
    })
}

fn read_meta(path: &Path) -> Option<ExportMeta> {
    let bytes = fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn ensure_normal_directory(path: &Path) -> Result<(), CodexAuthError> {
    if path.exists() {
        let metadata = fs::symlink_metadata(path).map_err(|_| CodexAuthError::WriteFailed)?;
        if metadata.file_type().is_symlink() {
            return Err(CodexAuthError::TargetConflict);
        }
        if !metadata.is_dir() {
            return Err(CodexAuthError::TargetConflict);
        }
        Ok(())
    } else {
        fs::create_dir_all(path).map_err(|_| CodexAuthError::WriteFailed)?;
        set_permissions(path, PRIVATE_DIR_MODE).map_err(|_| CodexAuthError::WriteFailed)?;
        Ok(())
    }
}

fn write_private_file(path: &Path, bytes: &[u8]) -> Result<(), CodexAuthError> {
    let directory = path.parent().ok_or(CodexAuthError::WriteFailed)?;
    let temporary = directory.join(format!(
        ".{}.ai-router-{}.tmp",
        path.file_name()
            .and_then(std::ffi::OsStr::to_str)
            .unwrap_or("auth"),
        Uuid::new_v4()
    ));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        set_open_mode(&mut options, PRIVATE_FILE_MODE);
        let mut file = options.open(&temporary)?;
        set_permissions(&temporary, PRIVATE_FILE_MODE)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        sync_directory(directory)?;
        Ok(())
    })();
    if temporary.exists() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn sync_directory(path: &Path) -> Result<(), CodexAuthError> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| CodexAuthError::WriteFailed)
}

#[cfg(unix)]
#[expect(
    clippy::unnecessary_wraps,
    reason = "the non-Unix implementation has no portable device/inode identity"
)]
fn file_identity(metadata: &fs::Metadata) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    Some((metadata.dev(), metadata.ino()))
}

#[cfg(not(unix))]
fn file_identity(_metadata: &fs::Metadata) -> Option<(u64, u64)> {
    None
}

#[cfg(unix)]
fn set_open_mode(options: &mut OpenOptions, mode: u32) {
    use std::os::unix::fs::OpenOptionsExt;
    options.mode(mode);
}

#[cfg(not(unix))]
fn set_open_mode(_options: &mut OpenOptions, _mode: u32) {}

#[cfg(unix)]
fn set_permissions(path: &Path, mode: u32) -> Result<(), std::io::Error> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn set_permissions(_path: &Path, _mode: u32) -> Result<(), std::io::Error> {
    Ok(())
}

#[cfg(test)]
mod tests;
