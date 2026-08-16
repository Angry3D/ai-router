use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

use serde::Serialize;
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

use crate::storage::CodexModelRecord;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EffectiveCodexCatalog {
    Original,
    Custom(Vec<CodexModelRecord>),
}

impl EffectiveCodexCatalog {
    #[must_use]
    pub fn from_models(models: Vec<CodexModelRecord>) -> Self {
        if models.is_empty() {
            Self::Original
        } else {
            Self::Custom(models)
        }
    }

    #[must_use]
    pub fn models(&self) -> &[CodexModelRecord] {
        match self {
            Self::Original => &[],
            Self::Custom(models) => models,
        }
    }

    /// Returns a stable fingerprint for comparing effective Codex picker projections.
    ///
    /// # Errors
    ///
    /// Returns serialization or verification errors for a custom catalog.
    pub fn fingerprint(&self) -> Result<String, CodexCatalogError> {
        let bytes = match self {
            Self::Original => b"ai-router:codex-original-catalog:v1".to_vec(),
            Self::Custom(models) => generate_codex_model_catalog(models)?,
        };
        Ok(hex::encode(Sha256::digest(bytes)))
    }
}

pub const CODEX_MODEL_CATALOG_FILE_NAME: &str = "codex-model-catalog.json";
const CODEX_BASE_INSTRUCTIONS: &str =
    include_str!("../../../fixtures/codex-base-instructions-v0.147.0.md");

#[derive(Debug, Error)]
pub enum CodexCatalogError {
    #[error("a Codex model catalog cannot be empty")]
    Empty,
    #[error("the Codex model catalog path is unsafe")]
    UnsafePath,
    #[error("Codex model catalog serialization failed")]
    Serialization(#[from] serde_json::Error),
    #[error("Codex model catalog filesystem operation failed")]
    Filesystem(#[from] std::io::Error),
    #[error("the published Codex model catalog could not be verified")]
    Verification,
}

#[derive(Serialize)]
struct Catalog<'a> {
    models: Vec<CatalogModel<'a>>,
}

#[derive(Serialize)]
#[allow(clippy::struct_excessive_bools)]
struct CatalogModel<'a> {
    slug: &'a str,
    display_name: &'a str,
    description: &'a str,
    context_window: u64,
    max_context_window: u64,
    effective_context_window_percent: u8,
    supported_reasoning_levels: Vec<ReasoningLevel>,
    default_reasoning_level: &'static str,
    default_reasoning_summary: &'static str,
    support_verbosity: bool,
    default_verbosity: &'static str,
    input_modalities: [&'static str; 2],
    supports_parallel_tool_calls: bool,
    supports_reasoning_summary_parameter: bool,
    supports_image_detail_original: bool,
    supports_search_tool: bool,
    web_search_tool_type: &'static str,
    supported_in_api: bool,
    shell_type: &'static str,
    apply_patch_tool_type: &'static str,
    truncation_policy: TruncationPolicy,
    experimental_supported_tools: [String; 0],
    service_tiers: [String; 0],
    visibility: &'static str,
    priority: usize,
    base_instructions: &'static str,
}

#[derive(Serialize)]
struct ReasoningLevel {
    effort: &'static str,
    description: &'static str,
}

#[derive(Serialize)]
struct TruncationPolicy {
    mode: &'static str,
    limit: u64,
}

/// Serializes the authoritative non-empty custom model list for Codex.
///
/// # Errors
///
/// Returns `Empty` rather than producing the invalid `{ "models": [] }` shape.
pub fn generate_codex_model_catalog(
    models: &[CodexModelRecord],
) -> Result<Vec<u8>, CodexCatalogError> {
    if models.is_empty() {
        return Err(CodexCatalogError::Empty);
    }
    let entries = models
        .iter()
        .enumerate()
        .map(|(index, model)| {
            let display_name = model.display_name.as_deref().unwrap_or(&model.model_id);
            let context_window = model
                .context_window
                .unwrap_or(crate::domain::DEFAULT_CODEX_MODEL_CONTEXT_WINDOW);
            CatalogModel {
                slug: &model.model_id,
                display_name,
                description: display_name,
                context_window,
                max_context_window: context_window,
                effective_context_window_percent: 95,
                supported_reasoning_levels: reasoning_levels(),
                default_reasoning_level: "high",
                default_reasoning_summary: "none",
                support_verbosity: true,
                default_verbosity: "medium",
                input_modalities: ["text", "image"],
                supports_parallel_tool_calls: true,
                supports_reasoning_summary_parameter: true,
                supports_image_detail_original: true,
                supports_search_tool: true,
                web_search_tool_type: "text_and_image",
                supported_in_api: true,
                shell_type: "shell_command",
                apply_patch_tool_type: "freeform",
                truncation_policy: TruncationPolicy {
                    mode: "tokens",
                    limit: 10_000,
                },
                experimental_supported_tools: [],
                service_tiers: [],
                visibility: "list",
                priority: index + 1,
                base_instructions: CODEX_BASE_INSTRUCTIONS,
            }
        })
        .collect();
    let bytes = serde_json::to_vec_pretty(&Catalog { models: entries })?;
    let decoded: serde_json::Value = serde_json::from_slice(&bytes)?;
    if decoded
        .get("models")
        .and_then(serde_json::Value::as_array)
        .is_none_or(Vec::is_empty)
    {
        return Err(CodexCatalogError::Verification);
    }
    Ok(bytes)
}

fn reasoning_levels() -> Vec<ReasoningLevel> {
    [
        ("low", "Fast responses with lighter reasoning"),
        (
            "medium",
            "Balances speed and reasoning depth for everyday tasks",
        ),
        ("high", "Greater reasoning depth for complex problems"),
        ("xhigh", "Extra high reasoning depth for complex problems"),
    ]
    .into_iter()
    .map(|(effort, description)| ReasoningLevel {
        effort,
        description,
    })
    .collect()
}

#[derive(Clone)]
pub struct LocalCodexCatalog {
    app_data_dir: PathBuf,
}

impl LocalCodexCatalog {
    #[must_use]
    pub fn new(app_data_dir: PathBuf) -> Self {
        Self { app_data_dir }
    }

    #[must_use]
    pub fn path(&self) -> PathBuf {
        self.app_data_dir.join(CODEX_MODEL_CATALOG_FILE_NAME)
    }

    /// Checks whether the owned catalog contains the exact generated projection.
    ///
    /// # Errors
    ///
    /// Returns serialization, unsafe-path, or filesystem errors. A missing file
    /// is an ordinary mismatch so callers can expose a retryable projection state.
    pub fn matches(&self, models: &[CodexModelRecord]) -> Result<bool, CodexCatalogError> {
        let expected = generate_codex_model_catalog(models)?;
        let path = self.path();
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                Err(CodexCatalogError::UnsafePath)
            }
            Ok(_) => Ok(fs::read(path)? == expected),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(CodexCatalogError::Filesystem(error)),
        }
    }

    /// Publishes a complete catalog with sibling-temp atomic replacement.
    ///
    /// # Errors
    ///
    /// Returns serialization, unsafe-path, filesystem, or verification errors.
    pub fn publish(&self, models: &[CodexModelRecord]) -> Result<PathBuf, CodexCatalogError> {
        let bytes = generate_codex_model_catalog(models)?;
        ensure_private_directory(&self.app_data_dir)?;
        let path = self.path();
        reject_symlink(&path)?;
        let temporary = self.app_data_dir.join(format!(
            ".{CODEX_MODEL_CATALOG_FILE_NAME}.{}.tmp",
            Uuid::new_v4()
        ));
        let result = (|| {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            set_open_mode(&mut options, 0o600);
            let mut file = options.open(&temporary)?;
            set_permissions(&temporary, 0o600)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            let mut verified = Vec::new();
            File::open(&temporary)?.read_to_end(&mut verified)?;
            if verified != bytes || serde_json::from_slice::<serde_json::Value>(&verified).is_err()
            {
                return Err(CodexCatalogError::Verification);
            }
            reject_symlink(&path)?;
            fs::rename(&temporary, &path)?;
            File::open(&self.app_data_dir)?.sync_all()?;
            let final_bytes = fs::read(&path)?;
            if final_bytes != bytes {
                return Err(CodexCatalogError::Verification);
            }
            Ok(path.clone())
        })();
        if temporary.exists() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    /// Removes only AI Router's exact derived catalog path.
    ///
    /// # Errors
    ///
    /// Returns an unsafe-path or filesystem error.
    pub fn remove(&self) -> Result<(), CodexCatalogError> {
        let path = self.path();
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                Err(CodexCatalogError::UnsafePath)
            }
            Ok(_) => {
                fs::remove_file(path)?;
                File::open(&self.app_data_dir)?.sync_all()?;
                Ok(())
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}

fn ensure_private_directory(path: &Path) -> Result<(), CodexCatalogError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(CodexCatalogError::UnsafePath);
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(path)?;
        }
        Err(error) => return Err(error.into()),
    }
    set_permissions(path, 0o700)?;
    Ok(())
}

fn reject_symlink(path: &Path) -> Result<(), CodexCatalogError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err(CodexCatalogError::UnsafePath)
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
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
    use std::fs;

    use super::{
        CodexCatalogError, EffectiveCodexCatalog, LocalCodexCatalog, generate_codex_model_catalog,
    };
    use crate::storage::CodexModelRecord;

    fn models() -> Vec<CodexModelRecord> {
        vec![
            CodexModelRecord {
                model_id: "relay-a".to_owned(),
                display_name: Some("Relay A".to_owned()),
                context_window: Some(200_000),
            },
            CodexModelRecord {
                model_id: "relay-b".to_owned(),
                display_name: None,
                context_window: None,
            },
        ]
    }

    #[test]
    fn original_catalog_fingerprint_is_stable() {
        assert_eq!(
            EffectiveCodexCatalog::Original
                .fingerprint()
                .expect("original catalog fingerprint"),
            "6fef9a6e1caaade06fd3e1f31a7a5b858e5be8aa4916ac5ac381553abdfa67eb"
        );
    }

    #[test]
    fn catalog_is_complete_deterministic_and_uses_explicit_fallbacks() {
        let first = generate_codex_model_catalog(&models()).expect("catalog");
        let second = generate_codex_model_catalog(&models()).expect("catalog");
        assert_eq!(first, second);
        let value: serde_json::Value = serde_json::from_slice(&first).expect("JSON");
        let entries = value["models"].as_array().expect("models");
        assert_eq!(entries[0]["slug"], "relay-a");
        assert_eq!(entries[0]["display_name"], "Relay A");
        assert_eq!(entries[0]["context_window"], 200_000);
        assert_eq!(entries[1]["display_name"], "relay-b");
        assert_eq!(entries[1]["context_window"], 128_000);
        assert_eq!(entries[0]["priority"], 1);
        assert_eq!(entries[1]["priority"], 2);
        assert_eq!(entries[0]["default_reasoning_level"], "high");
        assert_eq!(entries[0]["default_reasoning_summary"], "none");
        assert_eq!(entries[0]["effective_context_window_percent"], 95);
        assert_eq!(entries[0]["supports_reasoning_summary_parameter"], true);
        assert!(
            entries[0]["experimental_supported_tools"]
                .as_array()
                .is_some_and(Vec::is_empty)
        );
        assert!(entries[0].get("supports_reasoning_summaries").is_none());
        assert_eq!(
            entries[0]["supported_reasoning_levels"]
                .as_array()
                .expect("levels")
                .iter()
                .map(|level| level["effort"].as_str().expect("effort"))
                .collect::<Vec<_>>(),
            vec!["low", "medium", "high", "xhigh"]
        );
        assert!(
            entries[0]["base_instructions"].as_str().is_some_and(
                |value| value.starts_with("You are a coding agent running in the Codex CLI")
            )
        );
        for excluded in ["availability_nux", "additional_speed_tiers", "upgrade"] {
            assert!(entries[0].get(excluded).is_none());
        }
    }

    #[test]
    fn empty_catalog_is_rejected() {
        assert!(matches!(
            generate_codex_model_catalog(&[]),
            Err(CodexCatalogError::Empty)
        ));
    }

    #[test]
    fn publication_is_atomic_and_empty_removal_is_owned() {
        let directory = tempfile::tempdir().expect("directory");
        let catalog = LocalCodexCatalog::new(directory.path().join("app"));
        let path = catalog.publish(&models()).expect("publish");
        serde_json::from_slice::<serde_json::Value>(&fs::read(&path).expect("read"))
            .expect("valid JSON");
        assert!(catalog.matches(&models()).expect("matching catalog"));
        let mut changed = models();
        changed[0].model_id = "relay-changed".to_owned();
        assert!(!catalog.matches(&changed).expect("different catalog"));
        catalog.remove().expect("remove");
        assert!(!path.exists());
        assert!(!catalog.matches(&models()).expect("missing catalog"));
    }
}
