use std::{
    ffi::OsString,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const PRODUCTION_APP_IDENTIFIER: &str = "com.relax.airouter";
pub const QA_APP_IDENTIFIER: &str = "com.relax.airouter.qa";
pub const QA_ACCEPTANCE_ROOT_ENV: &str = "AI_ROUTER_QA_ACCEPTANCE_ROOT";
pub const QA_ACCEPTANCE_ROOT_PREFIX: &str = "ai-router-v0-2a-qa-";
pub const QA_ACCEPTANCE_MARKER_FILE: &str = ".ai-router-qa-acceptance-root";
pub const QA_RUNTIME_MARKER_FILE: &str = "runtime-marker.json";

#[derive(Debug, Error)]
pub enum QaAcceptanceError {
    #[error("QA acceptance root is allowed only for the exact QA identifier")]
    IdentifierNotAllowed,
    #[error("QA acceptance root must be an absolute path")]
    RootNotAbsolute,
    #[error("QA acceptance root is outside the OS temporary directory")]
    RootOutsideTemporaryDirectory,
    #[error("QA acceptance root name is invalid")]
    RootNameInvalid,
    #[error("QA acceptance root marker is invalid")]
    RootMarkerInvalid,
    #[error("QA acceptance path contains unsupported control characters")]
    PathInvalid,
    #[error("QA acceptance filesystem operation failed")]
    Filesystem(#[from] std::io::Error),
    #[error("QA acceptance marker serialization failed")]
    Serialization(#[from] serde_json::Error),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QaAcceptanceRoot {
    root: PathBuf,
    nonce: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QaRuntimeMarker {
    pub schema_version: u8,
    pub nonce: String,
    pub pid: u32,
    pub identifier: String,
    pub executable_path: PathBuf,
    pub app_data_dir: PathBuf,
    pub codex_home_dir: PathBuf,
    pub log_dir: PathBuf,
}

impl QaAcceptanceRoot {
    /// Resolves an optional acceptance root from an explicit environment value.
    ///
    /// # Errors
    ///
    /// Returns an error for every identifier except the exact QA identifier or
    /// when the path/marker does not prove a narrow temporary root.
    pub fn resolve(
        identifier: &str,
        value: Option<OsString>,
        temporary_directory: &Path,
    ) -> Result<Option<Self>, QaAcceptanceError> {
        let Some(value) = value else {
            return Ok(None);
        };
        if identifier != QA_APP_IDENTIFIER {
            return Err(QaAcceptanceError::IdentifierNotAllowed);
        }
        let requested = PathBuf::from(value);
        if !requested.is_absolute() {
            return Err(QaAcceptanceError::RootNotAbsolute);
        }
        if path_has_control_characters(&requested) {
            return Err(QaAcceptanceError::PathInvalid);
        }

        let temporary_directory = temporary_directory.canonicalize()?;
        let root_metadata = fs::symlink_metadata(&requested)?;
        if !root_metadata.is_dir() || root_metadata.file_type().is_symlink() {
            return Err(QaAcceptanceError::RootOutsideTemporaryDirectory);
        }
        let root = requested.canonicalize()?;
        if root == temporary_directory || !root.starts_with(&temporary_directory) {
            return Err(QaAcceptanceError::RootOutsideTemporaryDirectory);
        }

        let name = root
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or(QaAcceptanceError::RootNameInvalid)?;
        let nonce = name
            .strip_prefix(QA_ACCEPTANCE_ROOT_PREFIX)
            .filter(|nonce| {
                !nonce.is_empty()
                    && nonce.len() <= 64
                    && nonce
                        .chars()
                        .all(|character| character.is_ascii_alphanumeric() || character == '-')
            })
            .ok_or(QaAcceptanceError::RootNameInvalid)?
            .to_owned();

        let marker = root.join(QA_ACCEPTANCE_MARKER_FILE);
        let marker_metadata =
            fs::symlink_metadata(&marker).map_err(|_| QaAcceptanceError::RootMarkerInvalid)?;
        if !marker_metadata.is_file() || marker_metadata.file_type().is_symlink() {
            return Err(QaAcceptanceError::RootMarkerInvalid);
        }
        let marker_nonce =
            fs::read_to_string(marker).map_err(|_| QaAcceptanceError::RootMarkerInvalid)?;
        if marker_nonce.trim() != nonce {
            return Err(QaAcceptanceError::RootMarkerInvalid);
        }

        Ok(Some(Self { root, nonce }))
    }

    /// Resolves the process-level QA acceptance root.
    ///
    /// # Errors
    ///
    /// Returns the same validation errors as [`Self::resolve`].
    pub fn from_environment(identifier: &str) -> Result<Option<Self>, QaAcceptanceError> {
        Self::resolve(
            identifier,
            std::env::var_os(QA_ACCEPTANCE_ROOT_ENV),
            &std::env::temp_dir(),
        )
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    #[must_use]
    pub fn nonce(&self) -> &str {
        &self.nonce
    }

    #[must_use]
    pub fn app_data_dir(&self) -> PathBuf {
        self.root.join("app-data")
    }

    #[must_use]
    pub fn log_dir(&self) -> PathBuf {
        self.root.join("logs")
    }

    #[must_use]
    pub fn codex_home_dir(&self) -> PathBuf {
        self.app_data_dir().join("codex-home")
    }

    #[must_use]
    pub fn database_path(&self) -> PathBuf {
        self.app_data_dir().join("router.sqlite3")
    }

    #[must_use]
    pub fn runtime_marker_path(&self) -> PathBuf {
        self.root.join(QA_RUNTIME_MARKER_FILE)
    }

    /// Creates and validates every private runtime directory before consumers start.
    ///
    /// # Errors
    ///
    /// Returns an error when a directory cannot be created, is a symbolic link,
    /// or canonicalizes outside the validated run root.
    pub fn prepare_runtime_directories(&self) -> Result<(), QaAcceptanceError> {
        create_private_directory(&self.root, &self.app_data_dir())?;
        create_private_directory(&self.root, &self.codex_home_dir())?;
        create_private_directory(&self.root, &self.log_dir())?;
        Ok(())
    }

    /// Writes the non-secret runtime identity marker atomically.
    ///
    /// # Errors
    ///
    /// Returns an error when directories, serialization, permissions, or the
    /// final atomic rename fail.
    pub fn write_runtime_marker(
        &self,
        pid: u32,
        identifier: &str,
        executable_path: &Path,
    ) -> Result<QaRuntimeMarker, QaAcceptanceError> {
        if identifier != QA_APP_IDENTIFIER {
            return Err(QaAcceptanceError::IdentifierNotAllowed);
        }
        let app_data_dir = self.app_data_dir();
        let codex_home_dir = self.codex_home_dir();
        let log_dir = self.log_dir();
        self.prepare_runtime_directories()?;

        let marker = QaRuntimeMarker {
            schema_version: 1,
            nonce: self.nonce.clone(),
            pid,
            identifier: identifier.to_owned(),
            executable_path: executable_path.to_path_buf(),
            app_data_dir,
            codex_home_dir,
            log_dir,
        };
        let destination = self.runtime_marker_path();
        let temporary = self
            .root
            .join(format!(".{QA_RUNTIME_MARKER_FILE}.{pid}.tmp"));
        let encoded = serde_json::to_vec_pretty(&marker)?;
        let mut file = open_private_new(&temporary)?;
        file.write_all(&encoded)?;
        file.sync_all()?;
        drop(file);
        fs::rename(temporary, destination)?;
        Ok(marker)
    }
}

fn path_has_control_characters(path: &Path) -> bool {
    path.to_string_lossy().chars().any(char::is_control)
}

fn create_private_directory(root: &Path, path: &Path) -> Result<(), std::io::Error> {
    match fs::create_dir(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    let metadata = fs::symlink_metadata(path)?;
    let canonical = path.canonicalize()?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() || !canonical.starts_with(root) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "QA acceptance directory escaped the validated run root",
        ));
    }
    set_mode(&canonical, 0o700)
}

#[cfg(unix)]
fn open_private_new(path: &Path) -> Result<fs::File, std::io::Error> {
    use std::os::unix::fs::OpenOptionsExt;
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

#[cfg(not(unix))]
fn open_private_new(path: &Path) -> Result<fs::File, std::io::Error> {
    OpenOptions::new().write(true).create_new(true).open(path)
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

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;

    fn acceptance_root(temporary: &TempDir, nonce: &str) -> PathBuf {
        let root = temporary
            .path()
            .join(format!("{QA_ACCEPTANCE_ROOT_PREFIX}{nonce}"));
        fs::create_dir(&root).expect("acceptance root");
        fs::write(root.join(QA_ACCEPTANCE_MARKER_FILE), nonce).expect("root marker");
        root
    }

    #[test]
    fn absent_override_preserves_every_runtime_profile() {
        let temporary = TempDir::new().expect("temporary directory");
        assert_eq!(
            QaAcceptanceRoot::resolve(PRODUCTION_APP_IDENTIFIER, None, temporary.path())
                .expect("production default"),
            None
        );
        assert_eq!(
            QaAcceptanceRoot::resolve(QA_APP_IDENTIFIER, None, temporary.path())
                .expect("QA default"),
            None
        );
    }

    #[test]
    fn exact_qa_identifier_accepts_a_marked_temporary_root() {
        let temporary = TempDir::new().expect("temporary directory");
        let root = acceptance_root(&temporary, "run-123");
        let resolved = QaAcceptanceRoot::resolve(
            QA_APP_IDENTIFIER,
            Some(root.clone().into_os_string()),
            temporary.path(),
        )
        .expect("valid QA root")
        .expect("override");

        assert_eq!(
            resolved.root(),
            root.canonicalize().expect("canonical root")
        );
        assert_eq!(resolved.nonce(), "run-123");
        assert_eq!(
            resolved.database_path(),
            resolved.root().join("app-data/router.sqlite3")
        );
    }

    #[test]
    fn production_and_other_identifiers_fail_closed_when_override_is_present() {
        let temporary = TempDir::new().expect("temporary directory");
        let root = acceptance_root(&temporary, "identity");
        for identifier in [PRODUCTION_APP_IDENTIFIER, "com.example.other"] {
            assert!(matches!(
                QaAcceptanceRoot::resolve(
                    identifier,
                    Some(root.clone().into_os_string()),
                    temporary.path(),
                ),
                Err(QaAcceptanceError::IdentifierNotAllowed)
            ));
        }
    }

    #[test]
    fn root_requires_a_narrow_name_and_matching_regular_marker() {
        let temporary = TempDir::new().expect("temporary directory");
        let wrong_name = temporary.path().join("ordinary-directory");
        fs::create_dir(&wrong_name).expect("wrong-name root");
        fs::write(
            wrong_name.join(QA_ACCEPTANCE_MARKER_FILE),
            "ordinary-directory",
        )
        .expect("wrong-name marker");
        assert!(matches!(
            QaAcceptanceRoot::resolve(
                QA_APP_IDENTIFIER,
                Some(wrong_name.into_os_string()),
                temporary.path(),
            ),
            Err(QaAcceptanceError::RootNameInvalid)
        ));

        let wrong_marker = acceptance_root(&temporary, "wrong-marker");
        fs::write(wrong_marker.join(QA_ACCEPTANCE_MARKER_FILE), "different")
            .expect("replace marker");
        assert!(matches!(
            QaAcceptanceRoot::resolve(
                QA_APP_IDENTIFIER,
                Some(wrong_marker.into_os_string()),
                temporary.path(),
            ),
            Err(QaAcceptanceError::RootMarkerInvalid)
        ));
    }

    #[test]
    fn root_outside_the_declared_temporary_directory_is_rejected() {
        let temporary = TempDir::new().expect("temporary directory");
        let outside = TempDir::new().expect("outside directory");
        let root = acceptance_root(&outside, "outside");
        assert!(matches!(
            QaAcceptanceRoot::resolve(
                QA_APP_IDENTIFIER,
                Some(root.into_os_string()),
                temporary.path(),
            ),
            Err(QaAcceptanceError::RootOutsideTemporaryDirectory)
        ));
    }

    #[test]
    fn runtime_marker_contains_only_the_allowlisted_identity_projection() {
        let temporary = TempDir::new().expect("temporary directory");
        let root = acceptance_root(&temporary, "marker");
        let resolved = QaAcceptanceRoot::resolve(
            QA_APP_IDENTIFIER,
            Some(root.into_os_string()),
            temporary.path(),
        )
        .expect("valid root")
        .expect("override");
        let executable = resolved
            .root()
            .join("AI Router QA.app/Contents/MacOS/ai-router-app");
        let marker = resolved
            .write_runtime_marker(42, QA_APP_IDENTIFIER, &executable)
            .expect("runtime marker");
        let stored: QaRuntimeMarker = serde_json::from_slice(
            &fs::read(resolved.runtime_marker_path()).expect("stored marker"),
        )
        .expect("valid marker JSON");

        assert_eq!(stored, marker);
        assert_eq!(stored.pid, 42);
        assert_eq!(stored.identifier, QA_APP_IDENTIFIER);
        assert_eq!(stored.nonce, "marker");
        assert_eq!(stored.codex_home_dir, resolved.codex_home_dir());
    }

    #[cfg(unix)]
    #[test]
    fn runtime_marker_rejects_symlinked_private_directories() {
        use std::os::unix::fs::symlink;

        let temporary = TempDir::new().expect("temporary directory");
        let outside = TempDir::new().expect("outside directory");
        let root = acceptance_root(&temporary, "symlinked-private-dir");
        symlink(outside.path(), root.join("app-data")).expect("app-data symlink");
        let resolved = QaAcceptanceRoot::resolve(
            QA_APP_IDENTIFIER,
            Some(root.into_os_string()),
            temporary.path(),
        )
        .expect("valid root")
        .expect("override");

        assert!(matches!(
            resolved.write_runtime_marker(42, QA_APP_IDENTIFIER, Path::new("/tmp/qa")),
            Err(QaAcceptanceError::Filesystem(_))
        ));
    }
}
