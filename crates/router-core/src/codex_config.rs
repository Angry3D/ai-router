use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use toml_edit::{Array, DocumentMut, InlineTable, Item, Table, Value, value};
use ts_rs::TS;
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::{
    domain::ApiKey,
    storage::{DatabaseExecutor, StorageError},
};

const CONFIG_FILE_NAME: &str = "config.toml";
const PROVIDER_NAME: &str = "AI Router";
const DEFAULT_PROVIDER_KEY: &str = "custom";
const IMAGES_MCP_SERVER_NAME: &str = "ai_router_images";
pub(crate) const STREAM_IDLE_TIMEOUT_MS: i64 = 300_000;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum CodexConfigStatus {
    Checking,
    Connected,
    NotConnected,
    Changed,
    ImagesMcpNameConflict,
    ImagesMcpProjectionConflict,
    Invalid,
    Unreadable,
    SymlinkUnsupported,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct ConfigOperationResult {
    pub changed: bool,
    pub status: CodexConfigStatus,
}

pub struct FileSnapshot {
    pub exists: bool,
    pub bytes: Vec<u8>,
    pub mode: Option<u32>,
    fingerprint: ConfigFingerprint,
}

#[derive(Clone, Eq, PartialEq)]
struct ConfigFingerprint {
    exists: bool,
    digest: [u8; 32],
    length: u64,
    mode: Option<u32>,
    file_identity: Option<(u64, u64)>,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ImagesMcpOwnership {
    Uninitialized,
    BaselineOwned,
    RouterOwned,
}

impl ImagesMcpOwnership {
    const fn preserves_baseline_entry(self) -> bool {
        matches!(self, Self::BaselineOwned)
    }
}

/// Opaque proof of the exact Codex configuration snapshot approved for a
/// later guarded reconciliation.
#[derive(Clone)]
pub struct CodexConfigGuard {
    fingerprint: ConfigFingerprint,
}

#[derive(Clone)]
pub struct CodexRecoveryPreview {
    pub guard: CodexConfigGuard,
    pub current_exists: bool,
    pub current_unix_mode: Option<u32>,
    pub recovery_target_exists: bool,
    pub bytes_changed: bool,
    pub recovery_updated_at_ms: Option<i64>,
}

#[derive(Debug, Error)]
pub enum CodexConfigError {
    #[error("Codex configuration is invalid")]
    Invalid,
    #[error("Codex configuration is unreadable")]
    Unreadable,
    #[error("symbolic links are unsupported for Codex configuration")]
    SymlinkUnsupported,
    #[error("Codex configuration changed during the operation")]
    ChangedDuringOperation,
    #[error("the immutable Codex baseline does not exist")]
    BaselineMissing,
    #[error("the Codex disconnect recovery snapshot does not exist")]
    RecoveryUnavailable,
    #[error("Codex recovery is only available while disconnected")]
    RecoveryNotDisconnected,
    #[error("the Codex recovery preview is stale")]
    RecoveryPreviewStale,
    #[error("Codex recovery reset completed only partially and is retryable")]
    RecoveryResetPartial,
    #[error("the local gateway token is invalid")]
    GatewayTokenInvalid,
    #[error("the reserved AI Router image MCP server name is already owned")]
    ImagesMcpNameConflict,
    #[error("the AI Router image MCP repair is not available for this configuration")]
    ImagesMcpRepairNotAllowed,
    #[error("Codex configuration filesystem operation failed")]
    Filesystem(#[source] std::io::Error),
    #[error("Codex configuration storage operation failed")]
    Storage(#[from] StorageError),
}

impl From<std::io::Error> for CodexConfigError {
    fn from(error: std::io::Error) -> Self {
        Self::Filesystem(error)
    }
}

pub trait CodexConfigPort: Send + Sync {
    /// Reads the current ordinary-file snapshot and fingerprint.
    ///
    /// # Errors
    ///
    /// Returns a safe filesystem or symlink error.
    fn read(&self) -> Result<FileSnapshot, CodexConfigError>;

    /// Atomically writes bytes only if the original fingerprint still matches.
    ///
    /// # Errors
    ///
    /// Returns a safe filesystem, symlink, or concurrent-change error.
    fn write_atomic(
        &self,
        expected: &FileSnapshot,
        bytes: &[u8],
        mode: u32,
    ) -> Result<(), CodexConfigError>;

    /// Deletes the file only if the original fingerprint still matches.
    ///
    /// # Errors
    ///
    /// Returns a safe filesystem, symlink, or concurrent-change error.
    fn delete_atomic(&self, expected: &FileSnapshot) -> Result<(), CodexConfigError>;
}

pub struct LocalCodexFilesystem {
    codex_home: PathBuf,
    #[cfg(test)]
    before_replace: Option<std::sync::Arc<dyn Fn() + Send + Sync>>,
    #[cfg(test)]
    fail_temp_write: bool,
}

impl LocalCodexFilesystem {
    #[must_use]
    pub fn new(codex_home: PathBuf) -> Self {
        Self {
            codex_home,
            #[cfg(test)]
            before_replace: None,
            #[cfg(test)]
            fail_temp_write: false,
        }
    }

    fn config_path(&self) -> PathBuf {
        self.codex_home.join(CONFIG_FILE_NAME)
    }

    #[cfg(test)]
    fn with_before_replace(mut self, hook: impl Fn() + Send + Sync + 'static) -> Self {
        self.before_replace = Some(std::sync::Arc::new(hook));
        self
    }

    #[cfg(test)]
    fn with_temp_write_failure(mut self) -> Self {
        self.fail_temp_write = true;
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
}

impl CodexConfigPort for LocalCodexFilesystem {
    fn read(&self) -> Result<FileSnapshot, CodexConfigError> {
        read_snapshot(&self.codex_home, &self.config_path())
    }

    fn write_atomic(
        &self,
        expected: &FileSnapshot,
        bytes: &[u8],
        mode: u32,
    ) -> Result<(), CodexConfigError> {
        ensure_normal_codex_home(&self.codex_home)?;
        let path = self.config_path();
        let temporary = self
            .codex_home
            .join(format!(".config.toml.ai-router-{}.tmp", Uuid::new_v4()));
        let result = (|| {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            set_open_mode(&mut options, mode);
            let mut file = options.open(&temporary)?;
            set_permissions(&temporary, mode)?;
            #[cfg(test)]
            if self.fail_temp_write {
                return Err(CodexConfigError::Filesystem(std::io::Error::other(
                    "injected temporary write failure",
                )));
            }
            file.write_all(bytes)?;
            file.sync_all()?;
            let mut verified = Vec::new();
            File::open(&temporary)?.read_to_end(&mut verified)?;
            if verified != bytes {
                return Err(CodexConfigError::Unreadable);
            }

            self.run_before_replace();
            let current = self.read()?;
            if current.fingerprint != expected.fingerprint {
                return Err(CodexConfigError::ChangedDuringOperation);
            }
            fs::rename(&temporary, &path)?;
            sync_directory(&self.codex_home)?;
            let final_snapshot = self.read()?;
            if !final_snapshot.exists || final_snapshot.bytes != bytes {
                return Err(CodexConfigError::Unreadable);
            }
            Ok(())
        })();
        if temporary.exists() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    fn delete_atomic(&self, expected: &FileSnapshot) -> Result<(), CodexConfigError> {
        ensure_normal_codex_home(&self.codex_home)?;
        self.run_before_replace();
        let current = self.read()?;
        if current.fingerprint != expected.fingerprint {
            return Err(CodexConfigError::ChangedDuringOperation);
        }
        if current.exists {
            fs::remove_file(self.config_path())?;
            sync_directory(&self.codex_home)?;
        }
        Ok(())
    }
}

pub struct CodexConfigService<P> {
    database: DatabaseExecutor,
    filesystem: P,
    images_generation_enabled: bool,
}

impl<P: CodexConfigPort> CodexConfigService<P> {
    #[must_use]
    pub const fn new(database: DatabaseExecutor, filesystem: P) -> Self {
        Self {
            database,
            filesystem,
            images_generation_enabled: false,
        }
    }

    #[must_use]
    pub const fn with_images_generation_enabled(mut self, enabled: bool) -> Self {
        self.images_generation_enabled = enabled;
        self
    }

    /// Connects Codex to AI Router and freezes the initial baseline first.
    ///
    /// # Errors
    ///
    /// Returns validation, storage, filesystem, symlink, or race errors.
    pub async fn connect(
        &self,
        port: u16,
        gateway_token: &str,
    ) -> Result<ConfigOperationResult, CodexConfigError> {
        self.connect_with_catalog(port, gateway_token, None).await
    }

    /// Connects Codex while projecting an optional AI Router-owned model catalog.
    ///
    /// # Errors
    ///
    /// Returns validation, storage, filesystem, symlink, or race errors.
    pub async fn connect_with_catalog(
        &self,
        port: u16,
        gateway_token: &str,
        catalog_path: Option<&Path>,
    ) -> Result<ConfigOperationResult, CodexConfigError> {
        validate_gateway_token(gateway_token)?;
        let snapshot = self.filesystem.read()?;
        let document = parse_snapshot(&snapshot)?;
        validate_projection_shape(&document)?;
        let baseline = self.database.codex_baseline().await?;
        let preserve_baseline_images_mcp = match baseline.as_ref() {
            Some(baseline) => {
                baseline.original_exists
                    && reserved_mcp_item(&parse_document(&baseline.raw_bytes)?).is_some()
            }
            None => reserved_mcp_item(&document).is_some(),
        };
        if self.images_generation_enabled
            && (preserve_baseline_images_mcp
                || (reserved_mcp_item(&document).is_some()
                    && !images_mcp_entry_matches(&document, port, gateway_token)))
        {
            return Err(CodexConfigError::ImagesMcpNameConflict);
        }
        self.database
            .capture_codex_baseline(snapshot.exists, snapshot.bytes.clone(), snapshot.mode)
            .await?;
        let expected_catalog_path = self.expected_catalog_path(catalog_path).await?;
        self.project_and_write(
            &snapshot,
            document,
            port,
            gateway_token,
            expected_catalog_path.as_deref(),
            preserve_baseline_images_mcp,
        )
    }

    /// Repairs managed fields without changing the immutable baseline.
    ///
    /// # Errors
    ///
    /// Returns `BaselineMissing` before first connection, plus validation,
    /// filesystem, symlink, or race errors.
    pub async fn reconnect(
        &self,
        port: u16,
        gateway_token: &str,
    ) -> Result<ConfigOperationResult, CodexConfigError> {
        self.reconnect_with_catalog(port, gateway_token, None).await
    }

    /// Reconciles managed transport and optional catalog fields without changing the baseline.
    ///
    /// # Errors
    ///
    /// Returns `BaselineMissing` before first connection, plus validation,
    /// filesystem, symlink, or race errors.
    pub async fn reconnect_with_catalog(
        &self,
        port: u16,
        gateway_token: &str,
        catalog_path: Option<&Path>,
    ) -> Result<ConfigOperationResult, CodexConfigError> {
        validate_gateway_token(gateway_token)?;
        if self.database.codex_baseline().await?.is_none() {
            return Err(CodexConfigError::BaselineMissing);
        }
        let snapshot = self.filesystem.read()?;
        let document = parse_snapshot(&snapshot)?;
        validate_projection_shape(&document)?;
        let images_mcp_ownership = self.images_mcp_ownership().await?;
        let expected_catalog_path = self.expected_catalog_path(catalog_path).await?;
        self.project_and_write(
            &snapshot,
            document,
            port,
            gateway_token,
            expected_catalog_path.as_deref(),
            images_mcp_ownership.preserves_baseline_entry(),
        )
    }

    /// Reconciles only if the current file is still the exact snapshot bound to
    /// `guard`.
    ///
    /// # Errors
    ///
    /// Returns `ChangedDuringOperation` when the file changed after the guard
    /// was issued, plus the ordinary reconnect errors.
    pub async fn reconnect_with_catalog_guarded(
        &self,
        port: u16,
        gateway_token: &str,
        catalog_path: Option<&Path>,
        guard: &CodexConfigGuard,
    ) -> Result<ConfigOperationResult, CodexConfigError> {
        validate_gateway_token(gateway_token)?;
        if self.database.codex_baseline().await?.is_none() {
            return Err(CodexConfigError::BaselineMissing);
        }
        let snapshot = self.filesystem.read()?;
        if snapshot.fingerprint != guard.fingerprint {
            return Err(CodexConfigError::ChangedDuringOperation);
        }
        let document = parse_snapshot(&snapshot)?;
        validate_projection_shape(&document)?;
        let images_mcp_ownership = self.images_mcp_ownership().await?;
        let expected_catalog_path = self.expected_catalog_path(catalog_path).await?;
        self.project_and_write(
            &snapshot,
            document,
            port,
            gateway_token,
            expected_catalog_path.as_deref(),
            images_mcp_ownership.preserves_baseline_entry(),
        )
    }

    /// Issues a guard for an exact post-baseline image MCP projection conflict.
    ///
    /// # Errors
    ///
    /// Returns a safe ownership, validation, filesystem, or storage error when
    /// the current projection cannot be explicitly repaired.
    pub async fn preview_images_mcp_repair_with_catalog(
        &self,
        port: u16,
        gateway_token: &str,
        catalog_path: Option<&Path>,
    ) -> Result<CodexConfigGuard, CodexConfigError> {
        validate_gateway_token(gateway_token)?;
        let ownership = self.images_mcp_ownership().await?;
        let expected_catalog_path = self.expected_catalog_path(catalog_path).await?;
        let snapshot = self.filesystem.read()?;
        let status = status_for_snapshot(
            &snapshot,
            port,
            gateway_token,
            expected_catalog_path.as_deref(),
            self.images_generation_enabled,
            ownership,
        );
        match status {
            CodexConfigStatus::ImagesMcpProjectionConflict => Ok(CodexConfigGuard {
                fingerprint: snapshot.fingerprint,
            }),
            CodexConfigStatus::ImagesMcpNameConflict => {
                Err(CodexConfigError::ImagesMcpNameConflict)
            }
            _ => Err(CodexConfigError::ImagesMcpRepairNotAllowed),
        }
    }

    /// Replaces only an explicitly authorized post-baseline image MCP table,
    /// then completes the ordinary managed Codex projection.
    ///
    /// # Errors
    ///
    /// Returns `ChangedDuringOperation` when the guarded file changed, plus
    /// safe ownership, validation, filesystem, or storage errors.
    pub async fn repair_images_mcp_with_catalog_guarded(
        &self,
        port: u16,
        gateway_token: &str,
        catalog_path: Option<&Path>,
        guard: &CodexConfigGuard,
    ) -> Result<ConfigOperationResult, CodexConfigError> {
        validate_gateway_token(gateway_token)?;
        let ownership = self.images_mcp_ownership().await?;
        if ownership == ImagesMcpOwnership::Uninitialized {
            return Err(CodexConfigError::BaselineMissing);
        }
        let expected_catalog_path = self.expected_catalog_path(catalog_path).await?;
        let snapshot = self.filesystem.read()?;
        if snapshot.fingerprint != guard.fingerprint {
            return Err(CodexConfigError::ChangedDuringOperation);
        }
        let mut document = parse_snapshot(&snapshot)?;
        validate_projection_shape(&document)?;
        match status_for_snapshot(
            &snapshot,
            port,
            gateway_token,
            expected_catalog_path.as_deref(),
            self.images_generation_enabled,
            ownership,
        ) {
            CodexConfigStatus::ImagesMcpProjectionConflict => {}
            CodexConfigStatus::ImagesMcpNameConflict => {
                return Err(CodexConfigError::ImagesMcpNameConflict);
            }
            _ => return Err(CodexConfigError::ImagesMcpRepairNotAllowed),
        }
        replace_images_mcp_projection(&mut document, port, gateway_token)?;
        apply_managed_projection(
            &mut document,
            port,
            gateway_token,
            expected_catalog_path.as_deref(),
            self.images_generation_enabled,
            ownership.preserves_baseline_entry(),
        )?;
        let output = document.to_string().into_bytes();
        parse_document(&output)?;
        self.filesystem
            .write_atomic(&snapshot, &output, snapshot.mode.unwrap_or(0o600))?;
        Ok(ConfigOperationResult {
            changed: true,
            status: CodexConfigStatus::Connected,
        })
    }

    /// Restores the exact disconnect recovery bytes/mode or deletes an absent target.
    ///
    /// # Errors
    ///
    /// Returns `BaselineMissing`, filesystem, symlink, or concurrent-change
    /// errors.
    pub async fn restore(&self) -> Result<ConfigOperationResult, CodexConfigError> {
        let recovery = match self.database.codex_recovery_config().await? {
            Some(recovery) => recovery,
            None if self.database.codex_baseline().await?.is_none() => {
                return Err(CodexConfigError::BaselineMissing);
            }
            None => return Err(CodexConfigError::RecoveryUnavailable),
        };
        let current = self.filesystem.read()?;
        if recovery.original_exists {
            self.filesystem.write_atomic(
                &current,
                &recovery.raw_bytes,
                recovery.unix_mode.unwrap_or(0o600),
            )?;
        } else {
            self.filesystem.delete_atomic(&current)?;
        }
        Ok(ConfigOperationResult {
            changed: current.exists != recovery.original_exists
                || (recovery.original_exists
                    && (current.bytes != recovery.raw_bytes || current.mode != recovery.unix_mode)),
            status: CodexConfigStatus::NotConnected,
        })
    }

    /// Captures the current ordinary config as a guarded recovery-update preview.
    ///
    /// # Errors
    ///
    /// Returns baseline, recovery, filesystem, symlink, or TOML validation errors.
    pub async fn preview_recovery_update(&self) -> Result<CodexRecoveryPreview, CodexConfigError> {
        if self.database.codex_baseline().await?.is_none() {
            return Err(CodexConfigError::BaselineMissing);
        }
        let recovery = self
            .database
            .codex_recovery_config()
            .await?
            .ok_or(CodexConfigError::RecoveryUnavailable)?;
        let current = self.filesystem.read()?;
        if current.exists {
            parse_snapshot(&current)?;
        }
        Ok(CodexRecoveryPreview {
            guard: CodexConfigGuard {
                fingerprint: current.fingerprint.clone(),
            },
            current_exists: current.exists,
            current_unix_mode: current.mode,
            recovery_target_exists: recovery.original_exists,
            bytes_changed: current.exists != recovery.original_exists
                || (current.exists
                    && (current.bytes != recovery.raw_bytes || current.mode != recovery.unix_mode)),
            recovery_updated_at_ms: Some(recovery.updated_at_ms),
        })
    }

    /// Persists a previously previewed current config as the disconnect target.
    ///
    /// # Errors
    ///
    /// Returns a stale-preview, baseline, filesystem, validation, or storage error.
    pub async fn update_recovery_guarded(
        &self,
        guard: &CodexConfigGuard,
    ) -> Result<ConfigOperationResult, CodexConfigError> {
        let current = self.filesystem.read()?;
        if current.fingerprint != guard.fingerprint {
            return Err(CodexConfigError::RecoveryPreviewStale);
        }
        if current.exists {
            parse_snapshot(&current)?;
        }
        let recovery = self
            .database
            .codex_recovery_config()
            .await?
            .ok_or(CodexConfigError::RecoveryUnavailable)?;
        let unchanged = recovery.original_exists == current.exists
            && (!current.exists
                || (recovery.raw_bytes == current.bytes && recovery.unix_mode == current.mode));
        if unchanged {
            return Ok(ConfigOperationResult {
                changed: false,
                status: CodexConfigStatus::NotConnected,
            });
        }
        self.database
            .update_codex_recovery_config(current.exists, current.bytes, current.mode)
            .await
            .map_err(|error| match error {
                StorageError::NotFound => CodexConfigError::BaselineMissing,
                other => CodexConfigError::Storage(other),
            })?;
        Ok(ConfigOperationResult {
            changed: true,
            status: CodexConfigStatus::NotConnected,
        })
    }

    /// Captures a guarded preview for replacing the mutable target with the baseline.
    ///
    /// # Errors
    ///
    /// Returns baseline, recovery, filesystem, or symlink errors.
    pub async fn preview_reset_recovery_to_baseline(
        &self,
    ) -> Result<CodexRecoveryPreview, CodexConfigError> {
        let baseline = self
            .database
            .codex_baseline()
            .await?
            .ok_or(CodexConfigError::BaselineMissing)?;
        let recovery = self
            .database
            .codex_recovery_config()
            .await?
            .ok_or(CodexConfigError::RecoveryUnavailable)?;
        let current = self.filesystem.read()?;
        Ok(CodexRecoveryPreview {
            guard: CodexConfigGuard {
                fingerprint: current.fingerprint.clone(),
            },
            current_exists: current.exists,
            current_unix_mode: current.mode,
            recovery_target_exists: recovery.original_exists,
            bytes_changed: current.exists != baseline.original_exists
                || (baseline.original_exists
                    && (current.bytes != baseline.raw_bytes || current.mode != baseline.unix_mode)),
            recovery_updated_at_ms: Some(recovery.updated_at_ms),
        })
    }

    /// Restores the file and then publishes the immutable baseline as recovery target.
    ///
    /// # Errors
    ///
    /// Returns a stale-preview, baseline, filesystem, or retryable partial-reset error.
    pub async fn reset_recovery_to_baseline_guarded(
        &self,
        guard: &CodexConfigGuard,
    ) -> Result<ConfigOperationResult, CodexConfigError> {
        let baseline = self
            .database
            .codex_baseline()
            .await?
            .ok_or(CodexConfigError::BaselineMissing)?;
        let current = self.filesystem.read()?;
        if current.fingerprint != guard.fingerprint {
            return Err(CodexConfigError::RecoveryPreviewStale);
        }
        let recovery = self
            .database
            .codex_recovery_config()
            .await?
            .ok_or(CodexConfigError::RecoveryUnavailable)?;
        let file_unchanged = current.exists == baseline.original_exists
            && (!current.exists
                || (current.bytes == baseline.raw_bytes && current.mode == baseline.unix_mode));
        let recovery_unchanged = recovery.original_exists == baseline.original_exists
            && (!baseline.original_exists
                || (recovery.raw_bytes == baseline.raw_bytes
                    && recovery.unix_mode == baseline.unix_mode));
        if file_unchanged && recovery_unchanged {
            return Ok(ConfigOperationResult {
                changed: false,
                status: CodexConfigStatus::NotConnected,
            });
        }
        if !file_unchanged && baseline.original_exists {
            self.filesystem
                .write_atomic(
                    &current,
                    &baseline.raw_bytes,
                    baseline.unix_mode.unwrap_or(0o600),
                )
                .map_err(|error| match error {
                    CodexConfigError::ChangedDuringOperation => {
                        CodexConfigError::RecoveryPreviewStale
                    }
                    other => other,
                })?;
        } else if !file_unchanged {
            self.filesystem
                .delete_atomic(&current)
                .map_err(|error| match error {
                    CodexConfigError::ChangedDuringOperation => {
                        CodexConfigError::RecoveryPreviewStale
                    }
                    other => other,
                })?;
        }
        if !recovery_unchanged {
            self.database
                .reset_codex_recovery_config_to_baseline()
                .await
                .map_err(|_error| CodexConfigError::RecoveryResetPartial)?;
        }
        Ok(ConfigOperationResult {
            changed: !file_unchanged || !recovery_unchanged,
            status: CodexConfigStatus::NotConnected,
        })
    }

    /// Performs a read-only managed-projection check.
    ///
    pub fn status(&self, port: u16, gateway_token: &str) -> CodexConfigStatus {
        self.status_for_expected_catalog(port, gateway_token, None)
    }

    /// Checks the complete managed projection including baseline/owned catalog state.
    pub async fn status_with_catalog(
        &self,
        port: u16,
        gateway_token: &str,
        catalog_path: Option<&Path>,
    ) -> CodexConfigStatus {
        let Ok(images_mcp_ownership) = self.images_mcp_ownership().await else {
            return CodexConfigStatus::Invalid;
        };
        let Ok(expected) = self.expected_catalog_path(catalog_path).await else {
            return CodexConfigStatus::Unreadable;
        };
        self.status_for_expected_catalog_with_images_ownership(
            port,
            gateway_token,
            expected.as_deref(),
            images_mcp_ownership,
        )
    }

    /// Checks the complete projection and returns a guard for the exact file
    /// snapshot used by that check.
    pub async fn status_with_catalog_guard(
        &self,
        port: u16,
        gateway_token: &str,
        catalog_path: Option<&Path>,
    ) -> (CodexConfigStatus, Option<CodexConfigGuard>) {
        let Ok(images_mcp_ownership) = self.images_mcp_ownership().await else {
            return (CodexConfigStatus::Invalid, None);
        };
        let Ok(expected) = self.expected_catalog_path(catalog_path).await else {
            return (CodexConfigStatus::Unreadable, None);
        };
        let snapshot = match self.filesystem.read() {
            Ok(snapshot) => snapshot,
            Err(CodexConfigError::SymlinkUnsupported) => {
                return (CodexConfigStatus::SymlinkUnsupported, None);
            }
            Err(_) => return (CodexConfigStatus::Unreadable, None),
        };
        let status = status_for_snapshot(
            &snapshot,
            port,
            gateway_token,
            expected.as_deref(),
            self.images_generation_enabled,
            images_mcp_ownership,
        );
        let guard = (status == CodexConfigStatus::Connected).then_some(CodexConfigGuard {
            fingerprint: snapshot.fingerprint,
        });
        (status, guard)
    }

    /// Confirms that a retry-bound configuration snapshot remains current.
    ///
    /// # Errors
    ///
    /// Returns a safe filesystem or symlink error when the file cannot be read.
    pub fn guard_is_current(&self, guard: &CodexConfigGuard) -> Result<bool, CodexConfigError> {
        Ok(self.filesystem.read()?.fingerprint == guard.fingerprint)
    }

    fn status_for_expected_catalog(
        &self,
        port: u16,
        gateway_token: &str,
        expected_catalog_path: Option<&str>,
    ) -> CodexConfigStatus {
        let snapshot = match self.filesystem.read() {
            Ok(snapshot) => snapshot,
            Err(CodexConfigError::SymlinkUnsupported) => {
                return CodexConfigStatus::SymlinkUnsupported;
            }
            Err(_) => return CodexConfigStatus::Unreadable,
        };
        status_for_snapshot(
            &snapshot,
            port,
            gateway_token,
            expected_catalog_path,
            self.images_generation_enabled,
            ImagesMcpOwnership::RouterOwned,
        )
    }

    fn status_for_expected_catalog_with_images_ownership(
        &self,
        port: u16,
        gateway_token: &str,
        expected_catalog_path: Option<&str>,
        images_mcp_ownership: ImagesMcpOwnership,
    ) -> CodexConfigStatus {
        let snapshot = match self.filesystem.read() {
            Ok(snapshot) => snapshot,
            Err(CodexConfigError::SymlinkUnsupported) => {
                return CodexConfigStatus::SymlinkUnsupported;
            }
            Err(_) => return CodexConfigStatus::Unreadable,
        };
        status_for_snapshot(
            &snapshot,
            port,
            gateway_token,
            expected_catalog_path,
            self.images_generation_enabled,
            images_mcp_ownership,
        )
    }

    fn project_and_write(
        &self,
        snapshot: &FileSnapshot,
        mut document: DocumentMut,
        port: u16,
        gateway_token: &str,
        expected_catalog_path: Option<&str>,
        preserve_baseline_images_mcp: bool,
    ) -> Result<ConfigOperationResult, CodexConfigError> {
        if managed_projection_matches(
            &document,
            port,
            gateway_token,
            expected_catalog_path,
            self.images_generation_enabled,
            preserve_baseline_images_mcp,
        ) {
            return Ok(ConfigOperationResult {
                changed: false,
                status: CodexConfigStatus::Connected,
            });
        }
        apply_managed_projection(
            &mut document,
            port,
            gateway_token,
            expected_catalog_path,
            self.images_generation_enabled,
            preserve_baseline_images_mcp,
        )?;
        let output = document.to_string().into_bytes();
        parse_document(&output)?;
        self.filesystem
            .write_atomic(snapshot, &output, snapshot.mode.unwrap_or(0o600))?;
        Ok(ConfigOperationResult {
            changed: true,
            status: CodexConfigStatus::Connected,
        })
    }

    async fn expected_catalog_path(
        &self,
        catalog_path: Option<&Path>,
    ) -> Result<Option<String>, CodexConfigError> {
        if let Some(path) = catalog_path {
            if !path.is_absolute() {
                return Err(CodexConfigError::Invalid);
            }
            return path
                .to_str()
                .map(|path| Some(path.to_owned()))
                .ok_or(CodexConfigError::Invalid);
        }
        let Some(baseline) = self.database.codex_baseline().await? else {
            return Ok(None);
        };
        if !baseline.original_exists {
            return Ok(None);
        }
        let baseline = parse_document(&baseline.raw_bytes)?;
        root_catalog_path(&baseline).map(|path| path.map(str::to_owned))
    }

    async fn images_mcp_ownership(&self) -> Result<ImagesMcpOwnership, CodexConfigError> {
        let Some(baseline) = self.database.codex_baseline().await? else {
            return Ok(ImagesMcpOwnership::Uninitialized);
        };
        let baseline_owns_images_mcp = baseline.original_exists
            && reserved_mcp_item(&parse_document(&baseline.raw_bytes)?).is_some();
        Ok(if baseline_owns_images_mcp {
            ImagesMcpOwnership::BaselineOwned
        } else {
            ImagesMcpOwnership::RouterOwned
        })
    }
}

fn status_for_snapshot(
    snapshot: &FileSnapshot,
    port: u16,
    gateway_token: &str,
    expected_catalog_path: Option<&str>,
    images_generation_enabled: bool,
    images_mcp_ownership: ImagesMcpOwnership,
) -> CodexConfigStatus {
    if images_generation_enabled && images_mcp_ownership == ImagesMcpOwnership::BaselineOwned {
        return CodexConfigStatus::ImagesMcpNameConflict;
    }
    if !snapshot.exists {
        return CodexConfigStatus::NotConnected;
    }
    let Ok(document) = parse_snapshot(snapshot) else {
        return CodexConfigStatus::Invalid;
    };
    if validate_projection_shape(&document).is_err() {
        return CodexConfigStatus::Invalid;
    }
    if images_generation_enabled {
        if images_mcp_ownership == ImagesMcpOwnership::Uninitialized
            && reserved_mcp_item(&document).is_some()
        {
            return CodexConfigStatus::ImagesMcpNameConflict;
        }
        if images_mcp_ownership == ImagesMcpOwnership::RouterOwned
            && reserved_mcp_item(&document).is_some()
            && !images_mcp_entry_matches(&document, port, gateway_token)
        {
            return CodexConfigStatus::ImagesMcpProjectionConflict;
        }
    }
    if managed_projection_matches(
        &document,
        port,
        gateway_token,
        expected_catalog_path,
        images_generation_enabled,
        images_mcp_ownership.preserves_baseline_entry(),
    ) {
        CodexConfigStatus::Connected
    } else if managed_projection_has_marker(&document) {
        CodexConfigStatus::Changed
    } else {
        CodexConfigStatus::NotConnected
    }
}

/// Creates or loads the stable 32-byte local gateway token.
///
/// # Errors
///
/// Returns an OS-randomness, token-validation, or storage error.
pub async fn load_or_create_gateway_token(
    database: &DatabaseExecutor,
) -> Result<String, CodexConfigError> {
    let mut random = Zeroizing::new([0_u8; 32]);
    getrandom::fill(random.as_mut()).map_err(|_| CodexConfigError::GatewayTokenInvalid)?;
    let candidate = URL_SAFE_NO_PAD.encode(random.as_ref());
    let stored = database
        .get_or_create_singleton_secret(
            "gateway_token".to_owned(),
            ApiKey::parse(&candidate).map_err(|_| CodexConfigError::GatewayTokenInvalid)?,
        )
        .await?;
    String::from_utf8(stored.expose().to_vec()).map_err(|_| CodexConfigError::GatewayTokenInvalid)
}

fn validate_gateway_token(token: &str) -> Result<(), CodexConfigError> {
    if token.is_empty() || token.chars().any(char::is_control) {
        return Err(CodexConfigError::GatewayTokenInvalid);
    }
    Ok(())
}

fn parse_snapshot(snapshot: &FileSnapshot) -> Result<DocumentMut, CodexConfigError> {
    if snapshot.exists {
        parse_document(&snapshot.bytes)
    } else {
        Ok(DocumentMut::new())
    }
}

fn parse_document(bytes: &[u8]) -> Result<DocumentMut, CodexConfigError> {
    let text = std::str::from_utf8(bytes).map_err(|_| CodexConfigError::Invalid)?;
    text.parse().map_err(|_| CodexConfigError::Invalid)
}

fn apply_managed_projection(
    document: &mut DocumentMut,
    port: u16,
    token: &str,
    expected_catalog_path: Option<&str>,
    images_generation_enabled: bool,
    preserve_baseline_images_mcp: bool,
) -> Result<(), CodexConfigError> {
    let provider_key = selected_provider_key(document)?.to_owned();
    if document.get("model_provider").is_none() {
        document["model_provider"] = value(provider_key.as_str());
    }
    if let Some(path) = expected_catalog_path {
        document["model_catalog_json"] = value(path);
    } else {
        document.as_table_mut().remove("model_catalog_json");
    }
    let providers = document
        .as_table_mut()
        .entry("model_providers")
        .or_insert_with(|| Item::Table(Table::new()))
        .as_table_like_mut()
        .ok_or(CodexConfigError::Invalid)?;
    let managed = providers
        .entry(provider_key.as_str())
        .or_insert_with(|| Item::Table(Table::new()))
        .as_table_like_mut()
        .ok_or(CodexConfigError::Invalid)?;
    managed.insert("name", value(PROVIDER_NAME));
    managed.insert("base_url", value(format!("http://127.0.0.1:{port}/v1")));
    managed.insert("wire_api", value("responses"));
    managed.insert("requires_openai_auth", value(true));
    managed.insert("supports_websockets", value(false));
    managed.remove("request_max_retries");
    managed.remove("stream_max_retries");
    managed.insert("stream_idle_timeout_ms", value(STREAM_IDLE_TIMEOUT_MS));
    managed.insert("experimental_bearer_token", value(token));
    for removed in ["env_key", "env_key_instructions", "auth", "aws"] {
        managed.remove(removed);
    }
    apply_images_mcp_projection(
        document,
        port,
        token,
        images_generation_enabled,
        preserve_baseline_images_mcp,
    )?;
    Ok(())
}

fn validate_projection_shape(document: &DocumentMut) -> Result<(), CodexConfigError> {
    root_catalog_path(document)?;
    if document
        .get("mcp_servers")
        .is_some_and(|servers| servers.as_table_like().is_none())
    {
        return Err(CodexConfigError::Invalid);
    }
    let provider_key = selected_provider_key(document)?;
    let Some(providers) = document.get("model_providers") else {
        return Ok(());
    };
    let providers = providers.as_table_like().ok_or(CodexConfigError::Invalid)?;
    if let Some(managed) = providers.get(provider_key)
        && managed.as_table_like().is_none()
    {
        return Err(CodexConfigError::Invalid);
    }
    Ok(())
}

fn managed_projection_matches(
    document: &DocumentMut,
    port: u16,
    token: &str,
    expected_catalog_path: Option<&str>,
    images_generation_enabled: bool,
    preserve_baseline_images_mcp: bool,
) -> bool {
    let Ok(provider_key) = selected_provider_key(document) else {
        return false;
    };
    if document.get("model_provider").and_then(Item::as_str) != Some(provider_key) {
        return false;
    }
    let Some(providers) = document
        .get("model_providers")
        .and_then(Item::as_table_like)
    else {
        return false;
    };
    let Some(managed) = providers.get(provider_key).and_then(Item::as_table_like) else {
        return false;
    };
    root_catalog_path(document).ok().flatten() == expected_catalog_path
        && managed.get("name").and_then(Item::as_str) == Some(PROVIDER_NAME)
        && managed.get("base_url").and_then(Item::as_str)
            == Some(format!("http://127.0.0.1:{port}/v1").as_str())
        && managed.get("wire_api").and_then(Item::as_str) == Some("responses")
        && managed.get("requires_openai_auth").and_then(Item::as_bool) == Some(true)
        && managed.get("supports_websockets").and_then(Item::as_bool) == Some(false)
        && !managed.contains_key("request_max_retries")
        && !managed.contains_key("stream_max_retries")
        && managed
            .get("stream_idle_timeout_ms")
            .and_then(Item::as_integer)
            == Some(STREAM_IDLE_TIMEOUT_MS)
        && managed
            .get("experimental_bearer_token")
            .and_then(Item::as_str)
            == Some(token)
        && ["env_key", "env_key_instructions", "auth", "aws"]
            .iter()
            .all(|key| !managed.contains_key(key))
        && images_mcp_projection_matches(
            document,
            port,
            token,
            images_generation_enabled,
            preserve_baseline_images_mcp,
        )
}

fn reserved_mcp_item(document: &DocumentMut) -> Option<&Item> {
    document
        .get("mcp_servers")
        .and_then(Item::as_table_like)
        .and_then(|servers| servers.get(IMAGES_MCP_SERVER_NAME))
}

fn apply_images_mcp_projection(
    document: &mut DocumentMut,
    port: u16,
    token: &str,
    enabled: bool,
    preserve_baseline_images_mcp: bool,
) -> Result<(), CodexConfigError> {
    if preserve_baseline_images_mcp {
        return if enabled {
            Err(CodexConfigError::ImagesMcpNameConflict)
        } else {
            Ok(())
        };
    }
    let existing_matches = images_mcp_entry_matches(document, port, token);
    if enabled {
        if reserved_mcp_item(document).is_some() && !existing_matches {
            return Err(CodexConfigError::ImagesMcpNameConflict);
        }
        let servers = document
            .as_table_mut()
            .entry("mcp_servers")
            .or_insert_with(|| Item::Table(Table::new()))
            .as_table_like_mut()
            .ok_or(CodexConfigError::Invalid)?;
        if !existing_matches {
            servers.insert(
                IMAGES_MCP_SERVER_NAME,
                Item::Table(images_mcp_table(port, token)),
            );
        }
    } else if existing_matches {
        let servers = document
            .get_mut("mcp_servers")
            .and_then(Item::as_table_like_mut)
            .ok_or(CodexConfigError::Invalid)?;
        servers.remove(IMAGES_MCP_SERVER_NAME);
    }
    Ok(())
}

fn replace_images_mcp_projection(
    document: &mut DocumentMut,
    port: u16,
    token: &str,
) -> Result<(), CodexConfigError> {
    let servers = document
        .get_mut("mcp_servers")
        .and_then(Item::as_table_like_mut)
        .ok_or(CodexConfigError::ImagesMcpRepairNotAllowed)?;
    if !servers.contains_key(IMAGES_MCP_SERVER_NAME) {
        return Err(CodexConfigError::ImagesMcpRepairNotAllowed);
    }
    servers.insert(
        IMAGES_MCP_SERVER_NAME,
        Item::Table(images_mcp_table(port, token)),
    );
    Ok(())
}

fn images_mcp_table(port: u16, token: &str) -> Table {
    let mut headers = InlineTable::new();
    headers.insert("Authorization", Value::from(format!("Bearer {token}")));
    let mut tools = Array::new();
    tools.push("generate_image");
    let mut table = Table::new();
    table.insert("url", value(format!("http://127.0.0.1:{port}/mcp")));
    table.insert("http_headers", Item::Value(Value::InlineTable(headers)));
    table.insert("enabled_tools", Item::Value(Value::Array(tools)));
    table.insert("default_tools_approval_mode", value("prompt"));
    table
}

fn images_mcp_projection_matches(
    document: &DocumentMut,
    port: u16,
    token: &str,
    enabled: bool,
    preserve_baseline_images_mcp: bool,
) -> bool {
    preserve_baseline_images_mcp && !enabled
        || !preserve_baseline_images_mcp
            && images_mcp_entry_matches(document, port, token) == enabled
}

fn images_mcp_entry_matches(document: &DocumentMut, port: u16, token: &str) -> bool {
    let Some(entry) = reserved_mcp_item(document).and_then(Item::as_table_like) else {
        return false;
    };
    let headers_match = entry
        .get("http_headers")
        .and_then(Item::as_inline_table)
        .is_some_and(|headers| {
            headers.len() == 1
                && headers.get("Authorization").and_then(Value::as_str)
                    == Some(format!("Bearer {token}").as_str())
        });
    let tools_match = entry
        .get("enabled_tools")
        .and_then(Item::as_array)
        .is_some_and(|tools| {
            tools.len() == 1 && tools.get(0).and_then(Value::as_str) == Some("generate_image")
        });
    entry.len() == 4
        && entry.get("url").and_then(Item::as_str)
            == Some(format!("http://127.0.0.1:{port}/mcp").as_str())
        && headers_match
        && tools_match
        && entry
            .get("default_tools_approval_mode")
            .and_then(Item::as_str)
            == Some("prompt")
}

fn root_catalog_path(document: &DocumentMut) -> Result<Option<&str>, CodexConfigError> {
    document
        .get("model_catalog_json")
        .map(|item| item.as_str().ok_or(CodexConfigError::Invalid))
        .transpose()
}

fn selected_provider_key(document: &DocumentMut) -> Result<&str, CodexConfigError> {
    match document.get("model_provider") {
        None => Ok(DEFAULT_PROVIDER_KEY),
        Some(item) => item
            .as_str()
            .filter(|provider_key| !provider_key.is_empty())
            .ok_or(CodexConfigError::Invalid),
    }
}

fn managed_projection_has_marker(document: &DocumentMut) -> bool {
    let Ok(provider_key) = selected_provider_key(document) else {
        return false;
    };
    let Some(managed) = document
        .get("model_providers")
        .and_then(Item::as_table_like)
        .and_then(|providers| providers.get(provider_key))
        .and_then(Item::as_table_like)
    else {
        return false;
    };
    managed.get("name").and_then(Item::as_str) == Some(PROVIDER_NAME)
        && managed.get("wire_api").and_then(Item::as_str) == Some("responses")
}

fn read_snapshot(home: &Path, path: &Path) -> Result<FileSnapshot, CodexConfigError> {
    if home.exists() {
        let metadata = fs::symlink_metadata(home)?;
        if metadata.file_type().is_symlink() {
            return Err(CodexConfigError::SymlinkUnsupported);
        }
        if !metadata.is_dir() {
            return Err(CodexConfigError::Unreadable);
        }
    }
    if !path.exists() {
        return Ok(absent_snapshot());
    }
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Err(CodexConfigError::SymlinkUnsupported);
    }
    if !metadata.is_file() {
        return Err(CodexConfigError::Unreadable);
    }
    let bytes = fs::read(path)?;
    let mode = file_mode(&metadata);
    let file_identity = file_identity(&metadata);
    Ok(FileSnapshot {
        exists: true,
        fingerprint: fingerprint(true, &bytes, mode, file_identity),
        bytes,
        mode,
    })
}

fn absent_snapshot() -> FileSnapshot {
    FileSnapshot {
        exists: false,
        bytes: Vec::new(),
        mode: None,
        fingerprint: fingerprint(false, &[], None, None),
    }
}

fn fingerprint(
    exists: bool,
    bytes: &[u8],
    mode: Option<u32>,
    file_identity: Option<(u64, u64)>,
) -> ConfigFingerprint {
    let digest: [u8; 32] = Sha256::digest(bytes).into();
    ConfigFingerprint {
        exists,
        digest,
        length: bytes.len() as u64,
        mode,
        file_identity,
    }
}

fn ensure_normal_codex_home(home: &Path) -> Result<(), CodexConfigError> {
    if home.exists() {
        let metadata = fs::symlink_metadata(home)?;
        if metadata.file_type().is_symlink() {
            return Err(CodexConfigError::SymlinkUnsupported);
        }
        if !metadata.is_dir() {
            return Err(CodexConfigError::Unreadable);
        }
    } else {
        fs::create_dir_all(home)?;
        set_permissions(home, 0o700)?;
    }
    Ok(())
}

fn sync_directory(path: &Path) -> Result<(), CodexConfigError> {
    File::open(path)?.sync_all()?;
    Ok(())
}

#[cfg(unix)]
#[expect(
    clippy::unnecessary_wraps,
    reason = "the non-Unix implementation has no portable mode value"
)]
fn file_mode(metadata: &fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::MetadataExt;
    Some(metadata.mode() & 0o777)
}

#[cfg(not(unix))]
fn file_mode(_metadata: &fs::Metadata) -> Option<u32> {
    None
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
mod tests {
    use std::{fs, sync::Arc};

    use tempfile::TempDir;

    use super::{
        CodexConfigError, CodexConfigPort, CodexConfigService, CodexConfigStatus,
        LocalCodexFilesystem, load_or_create_gateway_token,
    };
    use crate::storage::DatabaseExecutor;

    const TOKEN: &str = "local_gateway_token";

    fn fixture() -> (TempDir, DatabaseExecutor, std::path::PathBuf) {
        let directory = tempfile::tempdir().expect("temporary directory");
        let database =
            DatabaseExecutor::open(directory.path().join("app/router.sqlite3")).expect("database");
        let codex_home = directory.path().join("codex");
        (directory, database, codex_home)
    }

    fn write_config(home: &std::path::Path, contents: &[u8], mode: u32) {
        fs::create_dir_all(home).expect("Codex home");
        fs::write(home.join("config.toml"), contents).expect("config fixture");
        super::set_permissions(&home.join("config.toml"), mode).expect("mode");
    }

    #[tokio::test]
    async fn codex_config_preserves_unmanaged_content_and_is_idempotent() {
        let (_directory, database, home) = fixture();
        let original = br#"# keep this comment
model_provider = "custom"
model = "gpt-5"
[mcp_servers.local]
command = "tool"
[model_providers.other]
name = "Other"
[model_providers.custom]
name = "Existing Custom"
base_url = "https://existing.example/v1"
experimental_bearer_token = "EXISTING_SECRET"
env_key = "EXISTING_ENV"
auth = { mode = "existing" }
[model_providers.ai_router]
extra = "keep"
experimental_bearer_token = "SIBLING_SECRET"
"#;
        write_config(&home, original, 0o640);
        let service =
            CodexConfigService::new(database.clone(), LocalCodexFilesystem::new(home.clone()));
        assert_eq!(
            service.status(32_189, TOKEN),
            CodexConfigStatus::NotConnected
        );
        let first = service.connect(32_189, TOKEN).await.expect("connect");
        assert!(first.changed);
        let connected = fs::read_to_string(home.join("config.toml")).expect("connected config");
        assert!(connected.contains("# keep this comment"));
        assert!(connected.contains("[mcp_servers.local]"));
        assert!(connected.contains("[model_providers.other]"));
        assert!(connected.contains("extra = \"keep\""));
        let original_document = std::str::from_utf8(original)
            .expect("original UTF-8")
            .parse::<toml_edit::DocumentMut>()
            .expect("original TOML");
        let connected_document = connected
            .parse::<toml_edit::DocumentMut>()
            .expect("connected TOML");
        assert_eq!(
            connected_document["model_provider"].as_str(),
            Some("custom")
        );
        assert_eq!(
            connected_document["model_providers"]["ai_router"].to_string(),
            original_document["model_providers"]["ai_router"].to_string()
        );
        let managed = connected_document["model_providers"]["custom"]
            .as_table_like()
            .expect("managed table");
        assert!(!managed.contains_key("env_key"));
        assert!(!managed.contains_key("auth"));
        assert_eq!(
            managed.get("name").and_then(toml_edit::Item::as_str),
            Some("AI Router")
        );
        let before = connected.into_bytes();
        let second = service
            .connect(32_189, TOKEN)
            .await
            .expect("idempotent connect");
        assert!(!second.changed);
        assert_eq!(fs::read(home.join("config.toml")).expect("config"), before);
        assert_eq!(service.status(32_189, TOKEN), CodexConfigStatus::Connected);
        assert_eq!(service.status(32_190, TOKEN), CodexConfigStatus::Changed);
    }

    #[tokio::test]
    async fn image_mcp_projection_enable_survives_restart_and_disable_removes_only_owned_entry() {
        let (_directory, database, home) = fixture();
        let original = br#"model = "gpt-5"
[mcp_servers.user_tool]
command = "keep"
"#;
        write_config(&home, original, 0o600);
        let initial =
            CodexConfigService::new(database.clone(), LocalCodexFilesystem::new(home.clone()));
        initial
            .connect(32_189, TOKEN)
            .await
            .expect("initial connect");

        let enabled =
            CodexConfigService::new(database.clone(), LocalCodexFilesystem::new(home.clone()))
                .with_images_generation_enabled(true);
        assert_eq!(enabled.status(32_189, TOKEN), CodexConfigStatus::Changed);
        enabled
            .reconnect(32_189, TOKEN)
            .await
            .expect("enable image MCP");
        let document = fs::read_to_string(home.join("config.toml"))
            .expect("enabled config")
            .parse::<toml_edit::DocumentMut>()
            .expect("enabled TOML");
        let image = document["mcp_servers"]["ai_router_images"]
            .as_table_like()
            .expect("image MCP table");
        assert_eq!(
            image.get("url").and_then(toml_edit::Item::as_str),
            Some("http://127.0.0.1:32189/mcp")
        );
        assert_eq!(
            image
                .get("default_tools_approval_mode")
                .and_then(toml_edit::Item::as_str),
            Some("prompt")
        );
        assert!(document["mcp_servers"].get("user_tool").is_some());

        let restarted =
            CodexConfigService::new(database.clone(), LocalCodexFilesystem::new(home.clone()))
                .with_images_generation_enabled(true);
        assert_eq!(
            restarted.status(32_189, TOKEN),
            CodexConfigStatus::Connected
        );
        assert!(
            !restarted
                .connect(32_189, TOKEN)
                .await
                .expect("idempotent reconnect after restart")
                .changed
        );

        let disabled = CodexConfigService::new(database, LocalCodexFilesystem::new(home.clone()));
        assert_eq!(disabled.status(32_189, TOKEN), CodexConfigStatus::Changed);
        disabled
            .reconnect(32_189, TOKEN)
            .await
            .expect("disable image MCP");
        let disabled_document = fs::read_to_string(home.join("config.toml"))
            .expect("disabled config")
            .parse::<toml_edit::DocumentMut>()
            .expect("disabled TOML");
        assert!(
            disabled_document["mcp_servers"]
                .get("ai_router_images")
                .is_none()
        );
        assert!(disabled_document["mcp_servers"].get("user_tool").is_some());
    }

    #[tokio::test]
    async fn image_mcp_reserved_name_conflicts_fail_without_overwrite() {
        let (_directory, database, home) = fixture();
        let original = br#"[mcp_servers.ai_router_images]
command = "user-owned"
"#;
        write_config(&home, original, 0o600);
        let service =
            CodexConfigService::new(database.clone(), LocalCodexFilesystem::new(home.clone()))
                .with_images_generation_enabled(true);
        assert!(matches!(
            service.connect(32_189, TOKEN).await,
            Err(CodexConfigError::ImagesMcpNameConflict)
        ));
        assert_eq!(
            fs::read(home.join("config.toml")).expect("config"),
            original
        );
        assert!(database.codex_baseline().await.expect("baseline").is_none());
    }

    #[tokio::test]
    async fn disabled_projection_preserves_an_exact_looking_baseline_mcp_entry() {
        let (_directory, database, home) = fixture();
        let original = br#"[mcp_servers.ai_router_images]
url = "http://127.0.0.1:32189/mcp"
http_headers = { Authorization = "Bearer local_gateway_token" }
enabled_tools = ["generate_image"]
default_tools_approval_mode = "prompt"
"#;
        write_config(&home, original, 0o600);
        let disabled =
            CodexConfigService::new(database.clone(), LocalCodexFilesystem::new(home.clone()));
        disabled
            .connect(32_189, TOKEN)
            .await
            .expect("connect without owning image MCP");

        let baseline = database
            .codex_baseline()
            .await
            .expect("baseline query")
            .expect("captured baseline");
        assert_eq!(baseline.raw_bytes, original);
        let connected = fs::read_to_string(home.join("config.toml"))
            .expect("connected config")
            .parse::<toml_edit::DocumentMut>()
            .expect("connected TOML");
        let baseline_document = std::str::from_utf8(original)
            .expect("baseline UTF-8")
            .parse::<toml_edit::DocumentMut>()
            .expect("baseline TOML");
        assert_eq!(
            connected["mcp_servers"]["ai_router_images"].to_string(),
            baseline_document["mcp_servers"]["ai_router_images"].to_string()
        );
        assert_eq!(
            disabled.status_with_catalog(32_189, TOKEN, None).await,
            CodexConfigStatus::Connected
        );

        let enabled = CodexConfigService::new(database, LocalCodexFilesystem::new(home.clone()))
            .with_images_generation_enabled(true);
        assert_eq!(
            enabled.status_with_catalog(32_189, TOKEN, None).await,
            CodexConfigStatus::ImagesMcpNameConflict
        );
        assert!(matches!(
            enabled
                .preview_images_mcp_repair_with_catalog(32_189, TOKEN, None)
                .await,
            Err(CodexConfigError::ImagesMcpNameConflict)
        ));
        assert!(matches!(
            enabled.reconnect(32_189, TOKEN).await,
            Err(CodexConfigError::ImagesMcpNameConflict)
        ));
        write_config(&home, b"", 0o600);
        assert_eq!(
            enabled.status_with_catalog(32_189, TOKEN, None).await,
            CodexConfigStatus::ImagesMcpNameConflict
        );
        assert!(matches!(
            enabled.reconnect(32_189, TOKEN).await,
            Err(CodexConfigError::ImagesMcpNameConflict)
        ));
        assert_eq!(fs::read(home.join("config.toml")).expect("config"), b"");
    }

    #[tokio::test]
    async fn image_mcp_projection_drift_requires_guarded_targeted_repair() {
        let (_directory, database, home) = fixture();
        let original = br#"model = "gpt-5"
[mcp_servers.user_tool]
command = "keep"
"#;
        write_config(&home, original, 0o640);
        let service =
            CodexConfigService::new(database.clone(), LocalCodexFilesystem::new(home.clone()))
                .with_images_generation_enabled(true);
        service.connect(32_189, TOKEN).await.expect("connect");
        let baseline = database
            .codex_baseline()
            .await
            .expect("baseline query")
            .expect("baseline");
        assert_eq!(baseline.raw_bytes, original);

        let mut drifted = fs::read_to_string(home.join("config.toml"))
            .expect("connected config")
            .parse::<toml_edit::DocumentMut>()
            .expect("connected TOML");
        drifted["mcp_servers"]["ai_router_images"]
            .as_table_like_mut()
            .expect("image MCP table")
            .remove("http_headers");
        drifted["permissions"] = toml_edit::value("keep");
        write_config(&home, drifted.to_string().as_bytes(), 0o640);
        let before_repair = fs::read(home.join("config.toml")).expect("drifted config");

        assert_eq!(
            service.status_with_catalog(32_189, TOKEN, None).await,
            CodexConfigStatus::ImagesMcpProjectionConflict
        );
        assert!(matches!(
            service.reconnect(32_189, TOKEN).await,
            Err(CodexConfigError::ImagesMcpNameConflict)
        ));
        assert_eq!(
            fs::read(home.join("config.toml")).expect("unchanged conflict"),
            before_repair
        );

        let guard = service
            .preview_images_mcp_repair_with_catalog(32_189, TOKEN, None)
            .await
            .expect("repair preview");
        let repaired = service
            .repair_images_mcp_with_catalog_guarded(32_189, TOKEN, None, &guard)
            .await
            .expect("repair");
        assert!(repaired.changed);
        assert_eq!(repaired.status, CodexConfigStatus::Connected);
        let repaired_document = fs::read_to_string(home.join("config.toml"))
            .expect("repaired config")
            .parse::<toml_edit::DocumentMut>()
            .expect("repaired TOML");
        assert!(super::images_mcp_entry_matches(
            &repaired_document,
            32_189,
            TOKEN
        ));
        assert_eq!(repaired_document["permissions"].as_str(), Some("keep"));
        assert!(repaired_document["mcp_servers"].get("user_tool").is_some());
        assert_eq!(
            database
                .codex_baseline()
                .await
                .expect("baseline query")
                .expect("baseline")
                .raw_bytes,
            original
        );
    }

    #[tokio::test]
    async fn image_mcp_repair_guard_and_atomic_write_reject_external_edits() {
        let (_directory, database, home) = fixture();
        let initial =
            CodexConfigService::new(database.clone(), LocalCodexFilesystem::new(home.clone()))
                .with_images_generation_enabled(true);
        initial.connect(32_189, TOKEN).await.expect("connect");
        let config_path = home.join("config.toml");
        let mut drifted = fs::read_to_string(&config_path)
            .expect("connected config")
            .parse::<toml_edit::DocumentMut>()
            .expect("connected TOML");
        drifted["mcp_servers"]["ai_router_images"]
            .as_table_like_mut()
            .expect("image MCP table")
            .remove("http_headers");
        write_config(&home, drifted.to_string().as_bytes(), 0o600);
        let guard = initial
            .preview_images_mcp_repair_with_catalog(32_189, TOKEN, None)
            .await
            .expect("preview");

        let external = b"# external replacement\n";
        write_config(&home, external, 0o600);
        assert!(matches!(
            initial
                .repair_images_mcp_with_catalog_guarded(32_189, TOKEN, None, &guard)
                .await,
            Err(CodexConfigError::ChangedDuringOperation)
        ));
        assert_eq!(fs::read(&config_path).expect("external config"), external);

        write_config(&home, drifted.to_string().as_bytes(), 0o600);
        let racing_path = config_path.clone();
        let racing = CodexConfigService::new(
            database,
            LocalCodexFilesystem::new(home).with_before_replace(move || {
                fs::write(&racing_path, b"# raced replacement\n").expect("race edit");
            }),
        )
        .with_images_generation_enabled(true);
        let guard = racing
            .preview_images_mcp_repair_with_catalog(32_189, TOKEN, None)
            .await
            .expect("race preview");
        assert!(matches!(
            racing
                .repair_images_mcp_with_catalog_guarded(32_189, TOKEN, None, &guard)
                .await,
            Err(CodexConfigError::ChangedDuringOperation)
        ));
        assert_eq!(
            fs::read(config_path).expect("raced config"),
            b"# raced replacement\n"
        );
    }

    #[test]
    fn codex_retry_projection_removes_retry_overrides_and_keeps_idle_timeout() {
        let mut document = "".parse().expect("document");
        super::apply_managed_projection(&mut document, 32_189, TOKEN, None, false, false)
            .expect("projection");
        assert_eq!(document["model_provider"].as_str(), Some("custom"));
        let managed = document["model_providers"]["custom"]
            .as_table_like()
            .expect("managed table");
        assert!(!managed.contains_key("request_max_retries"));
        assert!(!managed.contains_key("stream_max_retries"));
        assert_eq!(
            managed
                .get("stream_idle_timeout_ms")
                .and_then(toml_edit::Item::as_integer),
            Some(300_000)
        );
    }

    #[tokio::test]
    async fn legacy_retry_projection_is_changed_until_reconnect_removes_both_keys() {
        let (_directory, database, home) = fixture();
        let original = b"model = \"before\"\n";
        write_config(&home, original, 0o640);
        let service =
            CodexConfigService::new(database.clone(), LocalCodexFilesystem::new(home.clone()));
        service.connect(32_189, TOKEN).await.expect("connect");
        let config_path = home.join("config.toml");
        let mut legacy = fs::read_to_string(&config_path)
            .expect("connected config")
            .parse::<toml_edit::DocumentMut>()
            .expect("connected TOML");
        let managed = legacy["model_providers"]["custom"]
            .as_table_like_mut()
            .expect("managed table");
        managed.insert("request_max_retries", toml_edit::value(0));
        managed.insert("stream_max_retries", toml_edit::value(3));
        write_config(&home, legacy.to_string().as_bytes(), 0o640);

        assert_eq!(service.status(32_189, TOKEN), CodexConfigStatus::Changed);
        service.reconnect(32_189, TOKEN).await.expect("reconnect");

        let reconnected = fs::read_to_string(&config_path)
            .expect("reconnected config")
            .parse::<toml_edit::DocumentMut>()
            .expect("reconnected TOML");
        let managed = reconnected["model_providers"]["custom"]
            .as_table_like()
            .expect("managed table");
        assert!(!managed.contains_key("request_max_retries"));
        assert!(!managed.contains_key("stream_max_retries"));
        assert_eq!(service.status(32_189, TOKEN), CodexConfigStatus::Connected);
        assert_eq!(
            database
                .codex_baseline()
                .await
                .expect("baseline")
                .expect("captured baseline")
                .raw_bytes,
            original
        );
        assert_eq!(
            database
                .codex_recovery_config()
                .await
                .expect("recovery")
                .expect("captured recovery")
                .raw_bytes,
            original
        );
    }

    #[tokio::test]
    async fn first_write_failure_keeps_the_frozen_baseline() {
        let (_directory, database, home) = fixture();
        let original = b"model = \"before\"\n";
        write_config(&home, original, 0o600);
        let service = CodexConfigService::new(
            database.clone(),
            LocalCodexFilesystem::new(home.clone()).with_temp_write_failure(),
        );
        assert!(service.connect(32_189, TOKEN).await.is_err());
        let baseline = database
            .codex_baseline()
            .await
            .expect("baseline")
            .expect("captured");
        assert_eq!(baseline.raw_bytes, original);
        assert_eq!(
            fs::read(home.join("config.toml")).expect("config"),
            original
        );
    }

    #[tokio::test]
    async fn concurrent_external_edit_aborts_without_overwrite() {
        let (_directory, database, home) = fixture();
        write_config(&home, b"model = \"before\"\n", 0o600);
        let path = home.join("config.toml");
        let hook_path = path.clone();
        let filesystem = LocalCodexFilesystem::new(home).with_before_replace(move || {
            fs::write(&hook_path, b"model = \"external\"\n").expect("external edit");
        });
        let service = CodexConfigService::new(database, filesystem);
        assert!(matches!(
            service.connect(32_189, TOKEN).await,
            Err(CodexConfigError::ChangedDuringOperation)
        ));
        assert_eq!(fs::read(path).expect("config"), b"model = \"external\"\n");
    }

    #[tokio::test]
    async fn reconnect_never_changes_baseline_and_restore_is_byte_exact() {
        let (_directory, database, home) = fixture();
        let original = b"# original\nmodel_provider = \"custom\"\nmodel = \"gpt-5\"\n[model_providers.custom]\nname = \"Original\"\nbase_url = \"https://original.example/v1\"\n";
        write_config(&home, original, 0o640);
        let service =
            CodexConfigService::new(database.clone(), LocalCodexFilesystem::new(home.clone()));
        service.connect(32_189, TOKEN).await.expect("connect");
        write_config(
            &home,
            b"model_provider = \"custom\"\nmodel = \"external\"\n[model_providers.custom]\nname = \"AI Router\"\nwire_api = \"responses\"\n",
            0o600,
        );
        service.reconnect(32_190, TOKEN).await.expect("reconnect");
        let reconnected = fs::read_to_string(home.join("config.toml")).expect("reconnected");
        let reconnected_document = reconnected
            .parse::<toml_edit::DocumentMut>()
            .expect("reconnected TOML");
        assert_eq!(
            reconnected_document["model_provider"].as_str(),
            Some("custom")
        );
        assert_eq!(
            database
                .codex_baseline()
                .await
                .expect("baseline")
                .expect("exists")
                .raw_bytes,
            original
        );
        service.restore().await.expect("restore");
        assert_eq!(
            fs::read(home.join("config.toml")).expect("restored"),
            original
        );
        assert_eq!(
            service.status(32_190, TOKEN),
            CodexConfigStatus::NotConnected
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(home.join("config.toml"))
                    .expect("metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o640
            );
        }
    }

    #[tokio::test]
    async fn owned_catalog_projection_restores_the_baseline_pointer_when_empty() {
        let (directory, database, home) = fixture();
        let original = b"model_catalog_json = \"/user/catalog.json\"\n";
        write_config(&home, original, 0o640);
        let owned_catalog = directory.path().join("app/codex-model-catalog.json");
        fs::create_dir_all(owned_catalog.parent().expect("parent")).expect("app data");
        let service =
            CodexConfigService::new(database.clone(), LocalCodexFilesystem::new(home.clone()));
        service
            .connect_with_catalog(32_189, TOKEN, Some(&owned_catalog))
            .await
            .expect("connect with catalog");
        let connected = fs::read_to_string(home.join("config.toml"))
            .expect("config")
            .parse::<toml_edit::DocumentMut>()
            .expect("TOML");
        assert_eq!(
            connected["model_catalog_json"].as_str(),
            owned_catalog.to_str()
        );
        assert_eq!(
            service
                .status_with_catalog(32_189, TOKEN, Some(&owned_catalog))
                .await,
            CodexConfigStatus::Connected
        );

        service
            .reconnect_with_catalog(32_189, TOKEN, None)
            .await
            .expect("empty projection");
        let emptied = fs::read_to_string(home.join("config.toml"))
            .expect("config")
            .parse::<toml_edit::DocumentMut>()
            .expect("TOML");
        assert_eq!(
            emptied["model_catalog_json"].as_str(),
            Some("/user/catalog.json")
        );
        assert_eq!(
            database
                .codex_baseline()
                .await
                .expect("baseline")
                .expect("captured")
                .raw_bytes,
            original
        );
        service.restore().await.expect("restore");
        assert_eq!(fs::read(home.join("config.toml")).expect("bytes"), original);
    }

    #[tokio::test]
    async fn external_catalog_pointer_change_is_reported_without_repair() {
        let (directory, database, home) = fixture();
        let owned_catalog = directory.path().join("app/codex-model-catalog.json");
        let service = CodexConfigService::new(database, LocalCodexFilesystem::new(home.clone()));
        service
            .connect_with_catalog(32_189, TOKEN, Some(&owned_catalog))
            .await
            .expect("connect");
        let mut document = fs::read_to_string(home.join("config.toml"))
            .expect("config")
            .parse::<toml_edit::DocumentMut>()
            .expect("TOML");
        document["model_catalog_json"] = toml_edit::value("/external/catalog.json");
        write_config(&home, document.to_string().as_bytes(), 0o600);

        assert_eq!(
            service
                .status_with_catalog(32_189, TOKEN, Some(&owned_catalog))
                .await,
            CodexConfigStatus::Changed
        );
        assert_eq!(
            fs::read_to_string(home.join("config.toml")).expect("unchanged"),
            document.to_string()
        );
    }

    #[tokio::test]
    async fn guarded_reconnect_rejects_every_change_after_the_safe_status_snapshot() {
        let (directory, database, home) = fixture();
        let service = CodexConfigService::new(database, LocalCodexFilesystem::new(home.clone()));
        service.connect(32_189, TOKEN).await.expect("connect");
        let (status, guard) = service.status_with_catalog_guard(32_189, TOKEN, None).await;
        assert_eq!(status, CodexConfigStatus::Connected);
        let guard = guard.expect("connected snapshot guard");
        let config_path = home.join("config.toml");
        let connected = fs::read_to_string(&config_path).expect("connected config");
        let externally_changed = connected.replace(
            "stream_idle_timeout_ms = 300000",
            "stream_idle_timeout_ms = 1",
        );
        assert_ne!(externally_changed, connected);
        write_config(&home, externally_changed.as_bytes(), 0o600);
        let owned_catalog = directory.path().join("app/codex-model-catalog.json");

        assert!(matches!(
            service
                .reconnect_with_catalog_guarded(32_189, TOKEN, Some(&owned_catalog), &guard,)
                .await,
            Err(CodexConfigError::ChangedDuringOperation)
        ));
        assert_eq!(
            fs::read_to_string(config_path).expect("unchanged config"),
            externally_changed
        );
    }

    #[tokio::test]
    async fn absent_baseline_restore_removes_only_config_file() {
        let (_directory, database, home) = fixture();
        let service = CodexConfigService::new(database, LocalCodexFilesystem::new(home.clone()));
        service
            .connect(32_189, TOKEN)
            .await
            .expect("connect absent");
        assert!(home.join("config.toml").exists());
        service.restore().await.expect("restore absent");
        assert!(!home.join("config.toml").exists());
        assert!(home.exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlink_paths_are_rejected() {
        use std::os::unix::fs::symlink;

        let (directory, database, home) = fixture();
        let target = directory.path().join("target.toml");
        fs::write(&target, b"model = \"target\"\n").expect("target");
        fs::create_dir_all(&home).expect("home");
        symlink(target, home.join("config.toml")).expect("symlink");
        let service = CodexConfigService::new(database, LocalCodexFilesystem::new(home));
        assert!(matches!(
            service.connect(32_189, TOKEN).await,
            Err(CodexConfigError::SymlinkUnsupported)
        ));
    }

    #[tokio::test]
    async fn gateway_token_is_stable_and_url_safe() {
        let (_directory, database, _home) = fixture();
        let (first, second) = tokio::join!(
            load_or_create_gateway_token(&database),
            load_or_create_gateway_token(&database)
        );
        let first = first.expect("first token");
        let second = second.expect("second token");
        assert_eq!(first, second);
        assert_eq!(first.len(), 43);
        assert!(
            first
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        );
    }

    #[test]
    fn filesystem_port_reads_only_the_explicit_fixture_home() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let home = directory.path().join("codex");
        let filesystem = Arc::new(LocalCodexFilesystem::new(home));
        assert!(!filesystem.read().expect("snapshot").exists);
    }

    #[tokio::test]
    async fn invalid_configs_fail_before_baseline_capture() {
        for invalid in [b"not = [valid".as_slice(), b"\xff\xfe".as_slice()] {
            let (_directory, database, home) = fixture();
            write_config(&home, invalid, 0o600);
            let service =
                CodexConfigService::new(database.clone(), LocalCodexFilesystem::new(home));
            assert!(matches!(
                service.connect(32_189, TOKEN).await,
                Err(CodexConfigError::Invalid)
            ));
            assert!(database.codex_baseline().await.expect("baseline").is_none());
        }

        let (_directory, database, home) = fixture();
        write_config(
            &home,
            b"model_provider = \"custom\"\nmodel_providers = { custom = \"wrong type\" }\n",
            0o600,
        );
        let service = CodexConfigService::new(database.clone(), LocalCodexFilesystem::new(home));
        assert!(matches!(
            service.connect(32_189, TOKEN).await,
            Err(CodexConfigError::Invalid)
        ));
        assert!(database.codex_baseline().await.expect("baseline").is_none());

        let (_directory, database, home) = fixture();
        write_config(&home, b"model_provider = 42\n", 0o600);
        let service = CodexConfigService::new(database.clone(), LocalCodexFilesystem::new(home));
        assert!(matches!(
            service.connect(32_189, TOKEN).await,
            Err(CodexConfigError::Invalid)
        ));
        assert!(database.codex_baseline().await.expect("baseline").is_none());
    }

    #[tokio::test]
    async fn inline_provider_tables_are_supported() {
        let (_directory, database, home) = fixture();
        write_config(
            &home,
            b"model_provider = \"custom\"\nmodel_providers = { ai_router = { extra = \"keep\" }, custom = { name = \"Existing\", extra = \"selected-keep\" }, other = { name = \"Other\" } }\n",
            0o600,
        );
        let service = CodexConfigService::new(database, LocalCodexFilesystem::new(home.clone()));
        service.connect(32_189, TOKEN).await.expect("connect");
        let document = fs::read_to_string(home.join("config.toml"))
            .expect("config")
            .parse::<toml_edit::DocumentMut>()
            .expect("TOML");
        assert_eq!(
            document["model_providers"]["ai_router"]["extra"].as_str(),
            Some("keep")
        );
        assert_eq!(
            document["model_providers"]["custom"]["name"].as_str(),
            Some("AI Router")
        );
        assert_eq!(
            document["model_providers"]["custom"]["extra"].as_str(),
            Some("selected-keep")
        );
        assert_eq!(
            document["model_providers"]["other"]["name"].as_str(),
            Some("Other")
        );
        assert_eq!(document["model_provider"].as_str(), Some("custom"));
    }

    #[tokio::test]
    async fn updated_recovery_is_restored_without_changing_the_baseline() {
        let (_directory, database, home) = fixture();
        let original = b"model = \"original\"\n";
        let recovery = b"model = \"disconnect-target\"\n";
        write_config(&home, original, 0o640);
        let service =
            CodexConfigService::new(database.clone(), LocalCodexFilesystem::new(home.clone()));
        service.connect(32_189, TOKEN).await.expect("connect");
        service.restore().await.expect("initial disconnect");

        write_config(&home, recovery, 0o600);
        let preview = service
            .preview_recovery_update()
            .await
            .expect("update preview");
        assert!(preview.bytes_changed);
        service
            .update_recovery_guarded(&preview.guard)
            .await
            .expect("update recovery");
        write_config(&home, b"model = \"later-edit\"\n", 0o600);
        service.restore().await.expect("disconnect restore");

        assert_eq!(
            fs::read(home.join("config.toml")).expect("restored"),
            recovery
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(home.join("config.toml"))
                    .expect("restored metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        assert_eq!(
            database
                .codex_baseline()
                .await
                .expect("baseline query")
                .expect("baseline")
                .raw_bytes,
            original
        );
    }

    #[tokio::test]
    async fn absent_recovery_target_deletes_on_disconnect() {
        let (_directory, database, home) = fixture();
        write_config(&home, b"model = \"original\"\n", 0o600);
        let service = CodexConfigService::new(database, LocalCodexFilesystem::new(home.clone()));
        service.connect(32_189, TOKEN).await.expect("connect");
        service.restore().await.expect("disconnect");
        fs::remove_file(home.join("config.toml")).expect("remove current config");
        let preview = service
            .preview_recovery_update()
            .await
            .expect("absent preview");
        assert!(!preview.current_exists);
        service
            .update_recovery_guarded(&preview.guard)
            .await
            .expect("save absent recovery");
        write_config(&home, b"model = \"temporary\"\n", 0o600);

        service.restore().await.expect("restore absent target");

        assert!(!home.join("config.toml").exists());
    }

    #[tokio::test]
    async fn recovery_preview_rejects_invalid_and_stale_current_files() {
        let (_directory, database, home) = fixture();
        write_config(&home, b"model = \"original\"\n", 0o600);
        let service = CodexConfigService::new(database, LocalCodexFilesystem::new(home.clone()));
        service.connect(32_189, TOKEN).await.expect("connect");
        service.restore().await.expect("disconnect");
        let preview = service.preview_recovery_update().await.expect("preview");
        write_config(&home, b"model = \"external\"\n", 0o600);
        assert!(matches!(
            service.update_recovery_guarded(&preview.guard).await,
            Err(CodexConfigError::RecoveryPreviewStale)
        ));
        write_config(&home, b"not = [valid", 0o600);
        assert!(matches!(
            service.preview_recovery_update().await,
            Err(CodexConfigError::Invalid)
        ));
    }

    #[tokio::test]
    async fn reset_recovery_restores_file_and_mutable_snapshot_from_baseline() {
        let (_directory, database, home) = fixture();
        let original = b"model = \"original\"\n";
        write_config(&home, original, 0o640);
        let service =
            CodexConfigService::new(database.clone(), LocalCodexFilesystem::new(home.clone()));
        service.connect(32_189, TOKEN).await.expect("connect");
        service.restore().await.expect("disconnect");
        write_config(&home, b"model = \"new-recovery\"\n", 0o600);
        let update = service
            .preview_recovery_update()
            .await
            .expect("update preview");
        service
            .update_recovery_guarded(&update.guard)
            .await
            .expect("update recovery");
        write_config(&home, b"model = \"discarded-edit\"\n", 0o600);
        let reset = service
            .preview_reset_recovery_to_baseline()
            .await
            .expect("reset preview");

        service
            .reset_recovery_to_baseline_guarded(&reset.guard)
            .await
            .expect("reset");

        assert_eq!(
            fs::read(home.join("config.toml")).expect("reset file"),
            original
        );
        assert_eq!(
            database
                .codex_recovery_config()
                .await
                .expect("recovery query")
                .expect("recovery")
                .raw_bytes,
            original
        );
    }

    #[tokio::test]
    async fn reset_reports_change_when_only_mutable_recovery_differs() {
        let (_directory, database, home) = fixture();
        let original = b"model = \"original\"\n";
        write_config(&home, original, 0o640);
        let service =
            CodexConfigService::new(database.clone(), LocalCodexFilesystem::new(home.clone()));
        service.connect(32_189, TOKEN).await.expect("connect");
        service.restore().await.expect("disconnect");
        write_config(&home, b"model = \"new-recovery\"\n", 0o600);
        let update = service
            .preview_recovery_update()
            .await
            .expect("update preview");
        service
            .update_recovery_guarded(&update.guard)
            .await
            .expect("update recovery");
        write_config(&home, original, 0o640);
        let reset = service
            .preview_reset_recovery_to_baseline()
            .await
            .expect("reset preview");

        let result = service
            .reset_recovery_to_baseline_guarded(&reset.guard)
            .await
            .expect("reset recovery only");

        assert!(result.changed);
        assert_eq!(
            database
                .codex_recovery_config()
                .await
                .expect("recovery query")
                .expect("recovery")
                .raw_bytes,
            original
        );
    }
}
