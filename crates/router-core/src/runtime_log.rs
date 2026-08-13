use std::{
    fs::{self, Metadata, OpenOptions},
    io,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use thiserror::Error;

use crate::proxy::RuntimeDiagnosticEvent;

pub const LOG_FILE_PREFIX: &str = "ai-router";
pub const ACTIVE_LOG_FILE_NAME: &str = "ai-router.log";
pub const MAX_LOG_RECORD_BYTES: usize = 8 * 1024;
pub const MAX_LOG_FILE_BYTES: u64 = 2 * 1024 * 1024;
pub const MAX_LOG_FILES: usize = 10;
pub const MAX_LOG_TOTAL_BYTES: u64 = 20 * 1024 * 1024;
pub const LOG_RETENTION: Duration = Duration::from_hours(7 * 24);
pub const LOG_MAINTENANCE_INTERVAL: Duration = Duration::from_hours(6);

#[derive(Debug, Error)]
pub enum RuntimeLogError {
    #[error("runtime log directory is unsafe")]
    UnsafeDirectory,
    #[error("runtime log filesystem operation failed")]
    Filesystem(#[from] io::Error),
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RuntimeLogMaintenanceReport {
    pub removed_expired: u32,
    pub removed_for_limits: u32,
    pub skipped_unsafe: u32,
    pub remaining_files: u32,
    pub remaining_bytes: u64,
}

#[derive(Clone)]
pub struct RuntimeLogMaintenance {
    directory: PathBuf,
}

struct LogFileCandidate {
    path: PathBuf,
    size: u64,
    modified: Option<SystemTime>,
    active: bool,
}

impl RuntimeLogMaintenance {
    #[must_use]
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
        }
    }

    #[must_use]
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    #[must_use]
    pub fn active_log_path(&self) -> PathBuf {
        self.directory.join(ACTIVE_LOG_FILE_NAME)
    }

    /// Applies private file permissions after the logging plugin opens its
    /// active segment.
    ///
    /// # Errors
    ///
    /// Returns an error when the active path exists but is not an ordinary
    /// file, or when permissions cannot be applied.
    pub fn secure_active_file(&self) -> Result<(), RuntimeLogError> {
        let path = self.active_log_path();
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_file() => {
                set_file_permissions(&path)?;
                Ok(())
            }
            Ok(_) => Err(RuntimeLogError::UnsafeDirectory),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    /// Creates and validates the private log directory without following a
    /// symlink at the destructive-operation boundary.
    ///
    /// # Errors
    ///
    /// Returns an error when the directory is a symlink, is not a directory,
    /// or cannot be created or permissioned.
    pub fn prepare_directory(&self) -> Result<(), RuntimeLogError> {
        match fs::symlink_metadata(&self.directory) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(RuntimeLogError::UnsafeDirectory);
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                fs::create_dir_all(&self.directory)?;
            }
            Err(error) => return Err(error.into()),
        }
        set_directory_permissions(&self.directory)?;
        Ok(())
    }

    /// Applies age, count, and total-size retention to ordinary AI Router log
    /// files. The optional active path is preserved during periodic cleanup.
    ///
    /// # Errors
    ///
    /// Returns an error only when the directory itself cannot be prepared or
    /// enumerated. Individual unreadable or unsafe entries are skipped.
    pub fn maintain(
        &self,
        now: SystemTime,
        active_path: Option<&Path>,
    ) -> Result<RuntimeLogMaintenanceReport, RuntimeLogError> {
        self.prepare_directory()?;
        let mut report = RuntimeLogMaintenanceReport::default();
        let mut candidates = self.collect_candidates(active_path, &mut report)?;

        let mut retained = Vec::with_capacity(candidates.len());
        for candidate in candidates.drain(..) {
            let expired = candidate
                .modified
                .and_then(|modified| now.duration_since(modified).ok())
                .is_some_and(|age| age > LOG_RETENTION);
            if expired && !candidate.active {
                if fs::remove_file(&candidate.path).is_ok() {
                    report.removed_expired = report.removed_expired.saturating_add(1);
                } else {
                    report.skipped_unsafe = report.skipped_unsafe.saturating_add(1);
                    retained.push(candidate);
                }
            } else {
                retained.push(candidate);
            }
        }

        retained.sort_by(|left, right| {
            left.modified
                .cmp(&right.modified)
                .then_with(|| left.path.cmp(&right.path))
        });
        let mut total_bytes = retained
            .iter()
            .fold(0_u64, |total, file| total.saturating_add(file.size));
        while retained.len() > MAX_LOG_FILES || total_bytes > MAX_LOG_TOTAL_BYTES {
            let Some(index) = retained.iter().position(|candidate| !candidate.active) else {
                break;
            };
            let mut candidate = retained.remove(index);
            if fs::remove_file(&candidate.path).is_ok() {
                total_bytes = total_bytes.saturating_sub(candidate.size);
                report.removed_for_limits = report.removed_for_limits.saturating_add(1);
            } else {
                report.skipped_unsafe = report.skipped_unsafe.saturating_add(1);
                candidate.active = true;
                retained.push(candidate);
            }
        }

        report.remaining_files = retained.len().try_into().unwrap_or(u32::MAX);
        report.remaining_bytes = total_bytes;
        Ok(report)
    }

    /// Deletes rotated logs and truncates the active ordinary file in place.
    /// Callers must serialize this operation with application log writes.
    ///
    /// # Errors
    ///
    /// Returns an error when a matching ordinary file cannot be removed or the
    /// active file cannot be truncated and verified.
    pub fn clear(&self, active_path: &Path) -> Result<(), RuntimeLogError> {
        self.prepare_directory()?;
        for entry in fs::read_dir(&self.directory)? {
            let Ok(entry) = entry else { continue };
            let path = entry.path();
            if !is_owned_log_path(&path) {
                continue;
            }
            let Ok(metadata) = fs::symlink_metadata(&path) else {
                continue;
            };
            if !metadata.file_type().is_file() {
                continue;
            }
            if path == active_path {
                truncate_file(&path)?;
            } else {
                fs::remove_file(path)?;
            }
        }
        if active_path.exists() {
            let metadata = fs::symlink_metadata(active_path)?;
            if !metadata.file_type().is_file() {
                return Err(RuntimeLogError::UnsafeDirectory);
            }
            if metadata.len() != 0 {
                return Err(RuntimeLogError::Filesystem(io::Error::other(
                    "active runtime log was not truncated",
                )));
            }
        }
        Ok(())
    }

    fn collect_candidates(
        &self,
        active_path: Option<&Path>,
        report: &mut RuntimeLogMaintenanceReport,
    ) -> Result<Vec<LogFileCandidate>, RuntimeLogError> {
        let mut candidates = Vec::new();
        for entry in fs::read_dir(&self.directory)? {
            let Ok(entry) = entry else {
                report.skipped_unsafe = report.skipped_unsafe.saturating_add(1);
                continue;
            };
            let path = entry.path();
            if !is_owned_log_path(&path) {
                continue;
            }
            let Some(metadata) = candidate_metadata(fs::symlink_metadata(&path)) else {
                report.skipped_unsafe = report.skipped_unsafe.saturating_add(1);
                continue;
            };
            if !metadata.file_type().is_file() {
                report.skipped_unsafe = report.skipped_unsafe.saturating_add(1);
                continue;
            }
            candidates.push(LogFileCandidate {
                active: active_path.is_some_and(|active| active == path),
                path,
                size: metadata.len(),
                modified: metadata.modified().ok(),
            });
        }
        Ok(candidates)
    }
}

#[must_use]
pub fn format_runtime_diagnostic(event: &RuntimeDiagnosticEvent) -> String {
    let request_id = event.request_id.as_deref().unwrap_or("-");
    let route_id = event
        .route_id
        .as_ref()
        .map_or("-", crate::domain::RouteId::as_str);
    let status = event
        .http_status
        .map_or_else(|| "-".to_owned(), |status| status.to_string());
    let line = format!(
        "component={} code={} request_id={request_id} route_id={route_id} http_status={status}",
        event.component.as_str(),
        event.code.as_str(),
    );
    truncate_utf8(line, MAX_LOG_RECORD_BYTES)
}

#[must_use]
pub fn truncate_log_record(record: &str) -> String {
    truncate_utf8(record.to_owned(), MAX_LOG_RECORD_BYTES)
}

fn candidate_metadata(metadata: io::Result<Metadata>) -> Option<Metadata> {
    metadata.ok()
}

fn is_owned_log_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with(LOG_FILE_PREFIX))
}

fn truncate_file(path: &Path) -> io::Result<()> {
    let file = OpenOptions::new().write(true).truncate(true).open(path)?;
    file.sync_all()?;
    set_file_permissions(path)
}

fn truncate_utf8(mut value: String, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value;
    }
    let mut boundary = max_bytes;
    while boundary > 0 && !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    value.truncate(boundary);
    value
}

#[cfg(unix)]
fn set_directory_permissions(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn set_directory_permissions(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn set_file_permissions(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn set_file_permissions(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        fs::{File, FileTimes, OpenOptions},
        io::Write,
    };

    use tempfile::TempDir;

    use super::*;
    use crate::domain::RouteId;
    use crate::proxy::{RuntimeDiagnosticCode, RuntimeDiagnosticComponent};

    fn create_file(path: &Path, size: u64, modified: SystemTime) {
        let file = File::create(path).expect("create fixture log");
        file.set_len(size).expect("size fixture log");
        file.set_times(FileTimes::new().set_modified(modified))
            .expect("set fixture timestamp");
    }

    #[test]
    fn runtime_log_maintenance_applies_age_size_order_and_keeps_future_files() {
        let directory = TempDir::new().expect("log directory");
        let maintenance = RuntimeLogMaintenance::new(directory.path());
        maintenance.prepare_directory().expect("prepare logs");
        let now = SystemTime::UNIX_EPOCH + Duration::from_hours(20 * 24);
        let expired = directory.path().join("ai-router.expired.log");
        let oldest = directory.path().join("ai-router.oldest.log");
        let newer = directory.path().join("ai-router.newer.log");
        let future = directory.path().join("ai-router.future.log");
        create_file(
            &expired,
            1,
            now.checked_sub(LOG_RETENTION + Duration::from_secs(1))
                .expect("old timestamp"),
        );
        create_file(
            &oldest,
            8 * 1024 * 1024,
            now.checked_sub(Duration::from_secs(3)).expect("timestamp"),
        );
        create_file(
            &newer,
            8 * 1024 * 1024,
            now.checked_sub(Duration::from_secs(2)).expect("timestamp"),
        );
        create_file(
            &future,
            8 * 1024 * 1024,
            now.checked_add(Duration::from_mins(1)).expect("future"),
        );

        let report = maintenance.maintain(now, None).expect("maintain logs");

        assert_eq!(report.removed_expired, 1);
        assert_eq!(report.removed_for_limits, 1);
        assert!(!expired.exists());
        assert!(!oldest.exists());
        assert!(newer.exists());
        assert!(future.exists());
        assert!(report.remaining_bytes <= MAX_LOG_TOTAL_BYTES);
    }

    #[cfg(unix)]
    #[test]
    fn runtime_log_maintenance_skips_symlinks_and_unreadable_metadata() {
        use std::os::unix::fs::symlink;

        let directory = TempDir::new().expect("log directory");
        let maintenance = RuntimeLogMaintenance::new(directory.path());
        maintenance.prepare_directory().expect("prepare logs");
        let target = directory.path().join("outside.txt");
        fs::write(&target, b"must remain").expect("write target");
        let link = directory.path().join("ai-router.link.log");
        symlink(&target, &link).expect("create symlink");

        let report = maintenance
            .maintain(SystemTime::now(), None)
            .expect("maintain logs");

        assert_eq!(report.skipped_unsafe, 1);
        assert!(link.symlink_metadata().is_ok());
        assert_eq!(fs::read(&target).expect("read target"), b"must remain");
        assert!(
            candidate_metadata(Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "fixture",
            )))
            .is_none()
        );
    }

    #[test]
    fn runtime_log_clear_truncates_active_and_removes_archives() {
        let directory = TempDir::new().expect("log directory");
        let maintenance = RuntimeLogMaintenance::new(directory.path());
        maintenance.prepare_directory().expect("prepare logs");
        let active = maintenance.active_log_path();
        let archive = directory.path().join("ai-router.1.log");
        let unrelated = directory.path().join("other.log");
        for path in [&active, &archive, &unrelated] {
            let mut file = OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .open(path)
                .expect("open fixture");
            file.write_all(b"content").expect("write fixture");
        }

        maintenance.clear(&active).expect("clear logs");

        assert_eq!(fs::metadata(&active).expect("active metadata").len(), 0);
        assert!(!archive.exists());
        assert!(unrelated.exists());
    }

    #[test]
    fn runtime_log_formatter_is_typed_and_bounded() {
        let event = RuntimeDiagnosticEvent {
            component: RuntimeDiagnosticComponent::Upstream,
            code: RuntimeDiagnosticCode::UpstreamHttpStatus,
            request_id: Some("x".repeat(MAX_LOG_RECORD_BYTES * 2)),
            route_id: Some(RouteId::new()),
            http_status: Some(503),
        };

        let line = format_runtime_diagnostic(&event);

        assert!(line.starts_with("component=upstream code=upstream_http_status"));
        assert!(line.len() <= MAX_LOG_RECORD_BYTES);
        assert!(line.is_char_boundary(line.len()));
    }
}
