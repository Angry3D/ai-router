//! Persisted corruption-incident records.
//!
//! Every corruption decision, self-heal attempt, quarantine, and start-over
//! writes one bounded JSON record next to the runtime logs so the next
//! occurrence can be reconstructed after the ordinary logs have rotated. The
//! records contain only classification metadata, integrity message text, and
//! file size/hash facts: never secrets, Base URLs, or request/response content.

use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Number of incident records retained in the log directory.
pub const MAX_INCIDENT_FILES: usize = 20;
/// Maximum number of integrity messages persisted in one record.
pub const MAX_INCIDENT_MESSAGES: usize = 32;
/// Maximum serialized size of the persisted integrity message list.
pub const MAX_INCIDENT_MESSAGE_BYTES: usize = 4 * 1024;
/// Prefix shared by every persisted incident record file name.
pub const INCIDENT_FILE_PREFIX: &str = "incident-";

const INCIDENT_FILE_SUFFIX: &str = ".json";
const MAX_FILE_NAME_ATTEMPTS: u32 = 64;

#[derive(Debug, Error)]
pub enum IncidentError {
    #[error("incident directory is unsafe")]
    UnsafeDirectory,
    #[error("incident filesystem operation failed")]
    Filesystem(#[from] io::Error),
    #[error("incident serialization failed")]
    Serialization(#[from] serde_json::Error),
}

/// Classification of the corruption that produced an incident.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IncidentKind {
    IndexOnly,
    Unrecoverable,
}

/// Recovery action taken for a detected corruption.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IncidentAction {
    Repaired,
    Quarantined,
    RecoveryRequired,
    StartOver,
}

/// Result of the post-repair full validation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IncidentRecheck {
    Ok,
    Failed,
}

/// One persisted corruption incident.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct IncidentRecord {
    pub detected_at_ms: i64,
    pub kind: IncidentKind,
    pub integrity_messages: Vec<String>,
    pub app_version: String,
    pub db_bytes: u64,
    pub db_sha256: String,
    pub action: IncidentAction,
    pub recheck: IncidentRecheck,
    pub duration_ms: i64,
}

/// Writes one private incident record and applies the file-count retention.
///
/// The record is written with `0600` inside a `0700` directory and the oldest
/// records are removed once [`MAX_INCIDENT_FILES`] is exceeded.
///
/// # Errors
///
/// Returns an error when the directory is an unsafe object, cannot be created,
/// or the record cannot be serialized or durably written.
pub fn write_incident(directory: &Path, record: &IncidentRecord) -> Result<PathBuf, IncidentError> {
    ensure_private_directory(directory)?;
    let mut bounded = record.clone();
    bounded.integrity_messages = bound_integrity_messages(&record.integrity_messages);
    let payload = serde_json::to_vec(&bounded)?;
    let (path, mut file) = create_incident_file(directory, record.detected_at_ms)?;
    let written = (|| -> io::Result<()> {
        file.write_all(&payload)?;
        file.flush()?;
        file.sync_all()
    })();
    drop(file);
    if let Err(error) = written {
        let _ = fs::remove_file(&path);
        return Err(error.into());
    }
    sync_directory(directory)?;
    apply_incident_retention(directory)?;
    Ok(path)
}

/// Returns the newest readable incident record, or `None` when none exists.
///
/// Unreadable, unsafe, or unparsable records are skipped so a damaged file can
/// never make the diagnostic surface panic or fail.
///
/// # Errors
///
/// Returns an error only when the directory itself is an unsafe object or
/// cannot be enumerated.
pub fn read_latest_incident(directory: &Path) -> Result<Option<IncidentRecord>, IncidentError> {
    match fs::symlink_metadata(directory) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(IncidentError::UnsafeDirectory);
        }
        Ok(_) => {}
    }
    for (_, path) in collect_incident_files(directory)? {
        let Ok(payload) = fs::read(&path) else {
            continue;
        };
        let Ok(record) = serde_json::from_slice::<IncidentRecord>(&payload) else {
            continue;
        };
        return Ok(Some(record));
    }
    Ok(None)
}

/// Bounds integrity messages to the persisted count and byte budget.
///
/// Messages are trimmed, blanks are dropped, and any omitted message is
/// reported through a trailing marker entry.
#[must_use]
pub fn bound_integrity_messages(messages: &[String]) -> Vec<String> {
    let candidates: Vec<&str> = messages
        .iter()
        .map(|message| message.trim())
        .filter(|message| !message.is_empty())
        .collect();
    let mut bounded: Vec<String> = Vec::new();
    let mut bytes = 0_usize;
    for (index, message) in candidates.iter().enumerate() {
        let remaining = candidates.len() - index;
        let truncating = remaining > 1;
        let slot_limit = MAX_INCIDENT_MESSAGES - usize::from(truncating);
        let marker_reserve = if truncating {
            marker_len(remaining - 1)
        } else {
            0
        };
        if bounded.len() >= slot_limit
            || bytes + message.len() + 1 + marker_reserve > MAX_INCIDENT_MESSAGE_BYTES
        {
            break;
        }
        bytes += message.len() + 1;
        bounded.push((*message).to_owned());
    }
    let dropped = candidates.len() - bounded.len();
    if dropped > 0 {
        let marker = truncation_marker(dropped);
        loop {
            if bounded.len() < MAX_INCIDENT_MESSAGES
                && bytes + marker.len() < MAX_INCIDENT_MESSAGE_BYTES
            {
                break;
            }
            let Some(removed) = bounded.pop() else {
                break;
            };
            bytes = bytes.saturating_sub(removed.len() + 1);
        }
        if bytes + marker.len() < MAX_INCIDENT_MESSAGE_BYTES {
            bounded.push(marker);
        }
    }
    bounded
}

/// Hashes one file with SHA-256 for incident evidence.
///
/// # Errors
///
/// Returns an error when the file cannot be opened or read.
pub fn file_sha256(path: &Path) -> Result<String, io::Error> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn truncation_marker(dropped: usize) -> String {
    format!("<{dropped} more messages truncated>")
}

fn marker_len(dropped: usize) -> usize {
    truncation_marker(dropped).len() + 1
}

fn ensure_private_directory(directory: &Path) -> Result<(), IncidentError> {
    match fs::symlink_metadata(directory) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(IncidentError::UnsafeDirectory);
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir_all(directory)?;
        }
        Err(error) => return Err(error.into()),
    }
    set_mode(directory, 0o700)?;
    Ok(())
}

fn create_incident_file(
    directory: &Path,
    detected_at_ms: i64,
) -> Result<(PathBuf, File), IncidentError> {
    for attempt in 0..MAX_FILE_NAME_ATTEMPTS {
        let path = directory.join(incident_file_name(detected_at_ms, attempt));
        match open_private_new(&path) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
    }
    Err(io::Error::other("incident file name is exhausted").into())
}

fn incident_file_name(detected_at_ms: i64, attempt: u32) -> String {
    if attempt == 0 {
        format!("{INCIDENT_FILE_PREFIX}{detected_at_ms}{INCIDENT_FILE_SUFFIX}")
    } else {
        format!("{INCIDENT_FILE_PREFIX}{detected_at_ms}-{attempt}{INCIDENT_FILE_SUFFIX}")
    }
}

fn parse_incident_file_name(name: &str) -> Option<i64> {
    let body = name
        .strip_prefix(INCIDENT_FILE_PREFIX)?
        .strip_suffix(INCIDENT_FILE_SUFFIX)?;
    let (timestamp, suffix) = body.split_once('-').unwrap_or((body, ""));
    if !suffix.is_empty() && suffix.parse::<u32>().is_err() {
        return None;
    }
    timestamp.parse().ok()
}

fn open_private_new(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path)?;
    set_mode(path, 0o600)?;
    Ok(file)
}

fn apply_incident_retention(directory: &Path) -> Result<(), IncidentError> {
    let files = collect_incident_files(directory)?;
    if files.len() <= MAX_INCIDENT_FILES {
        return Ok(());
    }
    let mut removed = false;
    for (_, path) in files.into_iter().skip(MAX_INCIDENT_FILES) {
        match fs::remove_file(&path) {
            Ok(()) => removed = true,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(_) => {}
        }
    }
    if removed {
        sync_directory(directory)?;
    }
    Ok(())
}

/// Collects incident files newest first by modification time, then name.
fn collect_incident_files(directory: &Path) -> Result<Vec<(SystemTime, PathBuf)>, IncidentError> {
    let mut files = Vec::new();
    for entry in fs::read_dir(directory)? {
        let Ok(entry) = entry else { continue };
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if parse_incident_file_name(&name).is_none() {
            continue;
        }
        let Ok(metadata) = fs::symlink_metadata(entry.path()) else {
            continue;
        };
        if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
            continue;
        }
        let modified = metadata.modified().unwrap_or(UNIX_EPOCH);
        files.push((modified, entry.path()));
    }
    files.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| right.1.cmp(&left.1)));
    Ok(files)
}

fn sync_directory(directory: &Path) -> Result<(), IncidentError> {
    File::open(directory)?.sync_all()?;
    Ok(())
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        time::{Duration, SystemTime},
    };

    use super::{
        INCIDENT_FILE_PREFIX, IncidentAction, IncidentKind, IncidentRecheck, IncidentRecord,
        MAX_INCIDENT_FILES, MAX_INCIDENT_MESSAGE_BYTES, MAX_INCIDENT_MESSAGES,
        bound_integrity_messages, read_latest_incident, write_incident,
    };

    fn record(detected_at_ms: i64, messages: Vec<String>) -> IncidentRecord {
        IncidentRecord {
            detected_at_ms,
            kind: IncidentKind::IndexOnly,
            integrity_messages: messages,
            app_version: "0.4.2-test".to_owned(),
            db_bytes: 140_029_952,
            db_sha256: "0".repeat(64),
            action: IncidentAction::Repaired,
            recheck: IncidentRecheck::Ok,
            duration_ms: 1_030,
        }
    }

    fn incident_files(directory: &std::path::Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(directory)
            .expect("incident directory")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with(INCIDENT_FILE_PREFIX))
            .collect();
        names.sort();
        names
    }

    #[test]
    fn write_incident_persists_private_bounded_json() {
        let root = tempfile::tempdir().expect("root");
        let directory = root.path().join("logs");
        let messages = vec![
            "row 5195 missing from index proxy_requests_model_keyset_idx".to_owned(),
            "row 118113 missing from index proxy_requests_keyset_idx".to_owned(),
        ];
        let record = record(1_788_744_560_303, messages.clone());
        let path = write_incident(&directory, &record).expect("incident written");
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some("incident-1788744560303.json")
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&directory)
                    .expect("directory metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(&path)
                    .expect("file metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }

        let payload = fs::read_to_string(&path).expect("incident payload");
        let parsed: serde_json::Value = serde_json::from_str(&payload).expect("valid JSON");
        let mut keys: Vec<&str> = parsed
            .as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec![
                "action",
                "app_version",
                "db_bytes",
                "db_sha256",
                "detected_at_ms",
                "duration_ms",
                "integrity_messages",
                "kind",
                "recheck",
            ]
        );
        assert_eq!(parsed["integrity_messages"], serde_json::json!(messages));
        for absent in ["http", "sk-", "Bearer", "/Users/", "api_key"] {
            assert!(
                !payload.contains(absent),
                "incident payload must not contain {absent}"
            );
        }

        assert_eq!(
            read_latest_incident(&directory).expect("latest incident"),
            Some(record)
        );
        assert_eq!(
            read_latest_incident(&root.path().join("missing")).expect("absent directory"),
            None
        );
    }

    #[test]
    fn integrity_messages_are_bounded_by_count_and_bytes() {
        let many: Vec<String> = (0..MAX_INCIDENT_MESSAGES * 2)
            .map(|index| format!("row {} missing from index any_idx", index + 1))
            .collect();
        let bounded = bound_integrity_messages(&many);
        assert_eq!(bounded.len(), MAX_INCIDENT_MESSAGES);
        assert!(bounded.last().expect("marker").starts_with('<'));
        assert!(
            bounded
                .iter()
                .map(|message| message.len() + 1)
                .sum::<usize>()
                <= MAX_INCIDENT_MESSAGE_BYTES
        );
        assert_eq!(
            bound_integrity_messages(&[String::new()]),
            Vec::<String>::new()
        );

        let huge: Vec<String> = (0..8)
            .map(|index| format!("row {index} missing from index {}", "a".repeat(2_000)))
            .collect();
        let bounded = bound_integrity_messages(&huge);
        assert!(
            bounded
                .iter()
                .map(|message| message.len() + 1)
                .sum::<usize>()
                <= MAX_INCIDENT_MESSAGE_BYTES
        );
        assert!(bounded.last().expect("marker").starts_with('<'));

        let root = tempfile::tempdir().expect("root");
        let directory = root.path().join("logs");
        let path = write_incident(&directory, &record(1, huge.clone())).expect("bounded incident");
        let parsed: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).expect("payload")).expect("valid JSON");
        assert!(
            parsed["integrity_messages"]
                .as_array()
                .expect("array")
                .len()
                <= MAX_INCIDENT_MESSAGES
        );
    }

    #[test]
    fn incident_retention_keeps_the_newest_records() {
        let root = tempfile::tempdir().expect("root");
        let directory = root.path().join("logs");
        let newest = SystemTime::now();
        for index in 0..(MAX_INCIDENT_FILES + 5) {
            let detected_at_ms = 1_788_744_000_000 + i64::try_from(index).expect("index");
            let path =
                write_incident(&directory, &record(detected_at_ms, Vec::new())).expect("incident");
            let modified = newest
                .checked_sub(Duration::from_secs(
                    u64::try_from(MAX_INCIDENT_FILES + 5 - index).expect("seconds"),
                ))
                .expect("modified time");
            fs::File::options()
                .write(true)
                .open(&path)
                .expect("incident file")
                .set_modified(modified)
                .expect("modified");
        }
        let files = incident_files(&directory);
        assert_eq!(files.len(), MAX_INCIDENT_FILES);
        assert!(!files.contains(&"incident-1788744000000.json".to_owned()));
        assert!(files.contains(&format!(
            "incident-{}.json",
            1_788_744_000_000 + i64::try_from(MAX_INCIDENT_FILES + 4).expect("index")
        )));
    }

    #[test]
    fn unreadable_records_are_skipped_without_failing() {
        let root = tempfile::tempdir().expect("root");
        let directory = root.path().join("logs");
        let valid = write_incident(&directory, &record(1_788_744_000_000, Vec::new()))
            .expect("valid incident");
        let damaged = directory.join("incident-1788744000001.json");
        fs::write(&damaged, b"{ not json").expect("damaged incident");
        assert_eq!(
            read_latest_incident(&directory)
                .expect("latest incident")
                .expect("record")
                .detected_at_ms,
            1_788_744_000_000
        );
        fs::remove_file(&valid).expect("remove valid");
        assert_eq!(
            read_latest_incident(&directory).expect("latest incident"),
            None
        );
    }

    #[test]
    fn runtime_log_maintenance_never_removes_incident_records() {
        let root = tempfile::tempdir().expect("root");
        let directory = root.path().join("logs");
        let maintenance = crate::runtime_log::RuntimeLogMaintenance::new(&directory);
        maintenance.prepare_directory().expect("log directory");
        let active = maintenance.active_log_path();
        fs::write(&active, b"current\n").expect("active log");
        let rotated = directory.join("ai-router.2026-08-01.log");
        fs::write(&rotated, b"rotated\n").expect("rotated log");
        let expired = SystemTime::now()
            .checked_sub(Duration::from_hours(24 * 30))
            .expect("expired time");
        fs::File::options()
            .write(true)
            .open(&rotated)
            .expect("log file")
            .set_modified(expired)
            .expect("modified");
        let incident = directory.join("incident-1788744560303.json");
        fs::write(&incident, br#"{"detected_at_ms":1}"#).expect("incident record");
        fs::File::options()
            .write(true)
            .open(&incident)
            .expect("incident file")
            .set_modified(expired)
            .expect("modified");

        let report = maintenance
            .maintain(SystemTime::now(), Some(&active))
            .expect("maintenance");
        assert!(report.removed_expired >= 1);
        assert!(!rotated.exists());
        assert!(incident.exists());
        maintenance.clear(&active).expect("clear logs");
        assert!(incident.exists());
    }
}
