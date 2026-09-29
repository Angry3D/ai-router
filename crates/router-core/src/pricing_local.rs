use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;
use uuid::Uuid;

use crate::pricing::{
    CatalogTier, MAX_CATALOG_SOURCE_BYTES, MAX_RATE_MICRO_USD, ModelRate, validate_model_rows,
    validate_tier_version,
};

/// Directory under the application data directory that owns the local table.
pub const LOCAL_PRICING_DIRECTORY: &str = "pricing";
/// Exactly one local pricing table path is ever written or removed.
pub const LOCAL_PRICING_FILE_NAME: &str = "local-pricing.json";
/// The only supported local pricing table schema.
pub const LOCAL_PRICING_SCHEMA_VERSION: u32 = 1;
/// Upper bound for the serialized local pricing table.
pub const MAX_LOCAL_PRICING_BYTES: usize = 512 * 1024;
/// Upper bound for the accepted money precision.
pub const MAX_RATE_DECIMALS: usize = 6;
const RATE_SCALE: i64 = 1_000_000;

#[derive(Debug, Error)]
pub enum LocalPricingError {
    #[error("the local pricing amount is not an exact non-negative money string")]
    Money,
    #[error("the local pricing table is invalid: {0}")]
    Invalid(&'static str),
    #[error("the local pricing table is larger than the accepted size limit")]
    TooLarge,
    #[error("the local pricing table path is unsafe")]
    UnsafePath,
    #[error("the local pricing table could not be serialized")]
    Serialization(#[from] serde_json::Error),
    #[error("the local pricing table filesystem operation failed")]
    Filesystem(#[from] std::io::Error),
    #[error("the published local pricing table could not be verified")]
    Verification,
}

/// State of the local override, as reported by the settings pricing table.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LocalPricingStatus {
    /// No local table exists; only the bundled baseline is in effect.
    Missing,
    /// A validated local table is in effect.
    Loaded,
    /// A local table exists but was rejected; only the bundled baseline is in effect.
    Corrupt,
}

/// Outcome of loading the local pricing table.
#[derive(Clone, Debug)]
pub enum LocalPricingLoad {
    Missing,
    Loaded(LocalPricingTable),
    Corrupt,
}

/// One tier of the locally synchronized pricing table.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LocalPricingTier {
    pub version: String,
    pub models: Vec<ModelRate>,
}

/// Both local tiers, keyed exactly as the persisted schema names them.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LocalPricingTiers {
    #[serde(rename = "default")]
    pub standard: LocalPricingTier,
    pub priority: LocalPricingTier,
}

/// The local pricing table stored at `<app_data>/pricing/local-pricing.json`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LocalPricingTable {
    pub schema_version: u32,
    pub synced_at_ms: i64,
    pub source_url: String,
    pub tiers: LocalPricingTiers,
}

impl LocalPricingTable {
    #[must_use]
    pub fn tier(&self, tier: CatalogTier) -> &LocalPricingTier {
        match tier {
            CatalogTier::Standard => &self.tiers.standard,
            CatalogTier::Priority => &self.tiers.priority,
        }
    }

    /// Validates the schema version, provenance, and both tier row sets.
    ///
    /// # Errors
    ///
    /// Returns a stable category when the schema, sync time, source URL, tier
    /// version, model rows, rates, or context bands are invalid.
    pub fn validate(&self) -> Result<(), LocalPricingError> {
        if self.schema_version != LOCAL_PRICING_SCHEMA_VERSION {
            return Err(LocalPricingError::Invalid("schema_version"));
        }
        if self.synced_at_ms <= 0 {
            return Err(LocalPricingError::Invalid("synced_at_ms"));
        }
        if self.source_url.is_empty()
            || self.source_url.len() > MAX_CATALOG_SOURCE_BYTES
            || Url::parse(&self.source_url).is_err()
        {
            return Err(LocalPricingError::Invalid("source_url"));
        }
        for tier in CatalogTier::ALL {
            let value = self.tier(tier);
            validate_tier_version(tier, &value.version).map_err(LocalPricingError::Invalid)?;
            validate_model_rows(&value.models).map_err(LocalPricingError::Invalid)?;
        }
        Ok(())
    }
}

/// Parses one exact non-negative money string into micro-USD per million Tokens.
///
/// Accepts `$10.00`, `$0.125`, and `$0.0625` with at most
/// [`MAX_RATE_DECIMALS`] decimals. Scientific notation, `NaN`, signs,
/// thousands separators, and non-finite values are rejected, so no floating
/// point value ever participates in pricing.
///
/// # Errors
///
/// Returns [`LocalPricingError::Money`] when the text is not an exact amount in
/// the accepted range.
pub fn parse_rate_micro_usd(value: &str) -> Result<i64, LocalPricingError> {
    let body = value.trim();
    let Some(body) = body.strip_prefix('$') else {
        return Err(LocalPricingError::Money);
    };
    let mut whole = 0_i64;
    let mut fraction = 0_i64;
    let mut decimals = 0_usize;
    let mut digits = false;
    let mut point = false;
    for character in body.chars() {
        match character {
            '0'..='9' => {
                let digit = i64::from(character.to_digit(10).ok_or(LocalPricingError::Money)?);
                digits = true;
                if point {
                    if decimals == MAX_RATE_DECIMALS {
                        return Err(LocalPricingError::Money);
                    }
                    decimals += 1;
                    fraction = fraction * 10 + digit;
                } else {
                    whole = whole
                        .checked_mul(10)
                        .and_then(|value| value.checked_add(digit))
                        .ok_or(LocalPricingError::Money)?;
                }
            }
            '.' if !point => point = true,
            _ => return Err(LocalPricingError::Money),
        }
    }
    if !digits || whole > MAX_RATE_MICRO_USD / RATE_SCALE {
        return Err(LocalPricingError::Money);
    }
    while decimals < MAX_RATE_DECIMALS {
        fraction *= 10;
        decimals += 1;
    }
    let amount = whole
        .checked_mul(RATE_SCALE)
        .and_then(|value| value.checked_add(fraction))
        .ok_or(LocalPricingError::Money)?;
    if amount > MAX_RATE_MICRO_USD {
        return Err(LocalPricingError::Money);
    }
    Ok(amount)
}

/// Parses an optional money string where `-` means "no separate rate".
///
/// # Errors
///
/// Returns [`LocalPricingError::Money`] when the text is neither `-` nor a valid
/// exact amount.
pub fn parse_optional_rate_micro_usd(value: &str) -> Result<Option<i64>, LocalPricingError> {
    if value.trim() == "-" {
        return Ok(None);
    }
    parse_rate_micro_usd(value).map(Some)
}

/// Owns the fixed local pricing table path under the application data directory.
#[derive(Clone)]
pub struct LocalPricingStore {
    app_data_dir: PathBuf,
}

impl LocalPricingStore {
    #[must_use]
    pub fn new(app_data_dir: PathBuf) -> Self {
        Self { app_data_dir }
    }

    #[must_use]
    pub fn directory(&self) -> PathBuf {
        self.app_data_dir.join(LOCAL_PRICING_DIRECTORY)
    }

    #[must_use]
    pub fn path(&self) -> PathBuf {
        self.directory().join(LOCAL_PRICING_FILE_NAME)
    }

    /// Loads and validates the local table without ever failing the caller.
    ///
    /// A missing, oversized, unreadable, unparsable, or structurally invalid
    /// table reports [`LocalPricingLoad::Corrupt`] or
    /// [`LocalPricingLoad::Missing`] so startup can fall back to the bundled
    /// baseline and surface the state in Settings.
    #[must_use]
    pub fn load(&self) -> LocalPricingLoad {
        let path = self.path();
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return LocalPricingLoad::Missing;
            }
            Err(_) => return LocalPricingLoad::Corrupt,
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return LocalPricingLoad::Corrupt;
        }
        let limit = u64::try_from(MAX_LOCAL_PRICING_BYTES).unwrap_or(u64::MAX);
        if metadata.len() > limit {
            return LocalPricingLoad::Corrupt;
        }
        let Ok(bytes) = fs::read(&path) else {
            return LocalPricingLoad::Corrupt;
        };
        if bytes.len() > MAX_LOCAL_PRICING_BYTES {
            return LocalPricingLoad::Corrupt;
        }
        match serde_json::from_slice::<LocalPricingTable>(&bytes) {
            Ok(table) if table.validate().is_ok() => LocalPricingLoad::Loaded(table),
            Ok(_) | Err(_) => LocalPricingLoad::Corrupt,
        }
    }

    /// Publishes a complete local table with sibling-temp atomic replacement.
    ///
    /// The temporary file is created with `0600`, flushed with `sync_all`, read
    /// back and re-validated, then renamed over the fixed path followed by a
    /// directory `sync_all`, so a crash never exposes a partial table.
    ///
    /// # Errors
    ///
    /// Returns validation, size, serialization, unsafe-path, filesystem, or
    /// verification errors.
    pub fn publish(&self, table: &LocalPricingTable) -> Result<PathBuf, LocalPricingError> {
        table.validate()?;
        let bytes = serde_json::to_vec_pretty(table)?;
        if bytes.len() > MAX_LOCAL_PRICING_BYTES {
            return Err(LocalPricingError::TooLarge);
        }
        let directory = self.directory();
        ensure_private_directory(&directory)?;
        let path = self.path();
        reject_symlink(&path)?;
        let temporary =
            directory.join(format!(".{LOCAL_PRICING_FILE_NAME}.{}.tmp", Uuid::new_v4()));
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
            if verified != bytes {
                return Err(LocalPricingError::Verification);
            }
            if serde_json::from_slice::<LocalPricingTable>(&verified)
                .map_err(|_| LocalPricingError::Verification)?
                .validate()
                .is_err()
            {
                return Err(LocalPricingError::Verification);
            }
            reject_symlink(&path)?;
            fs::rename(&temporary, &path)?;
            File::open(&directory)?.sync_all()?;
            if fs::read(&path)? != bytes {
                return Err(LocalPricingError::Verification);
            }
            Ok(path.clone())
        })();
        if temporary.exists() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    /// Removes only the owned local pricing table path.
    ///
    /// # Errors
    ///
    /// Returns an unsafe-path or filesystem error.
    pub fn remove(&self) -> Result<(), LocalPricingError> {
        let path = self.path();
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                Err(LocalPricingError::UnsafePath)
            }
            Ok(_) => {
                fs::remove_file(path)?;
                File::open(self.directory())?.sync_all()?;
                Ok(())
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}

fn ensure_private_directory(path: &Path) -> Result<(), LocalPricingError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(LocalPricingError::UnsafePath);
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

fn reject_symlink(path: &Path) -> Result<(), LocalPricingError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err(LocalPricingError::UnsafePath)
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

    use tempfile::tempdir;

    use super::{
        LOCAL_PRICING_FILE_NAME, LOCAL_PRICING_SCHEMA_VERSION, LocalPricingError, LocalPricingLoad,
        LocalPricingStore, LocalPricingTable, LocalPricingTier, LocalPricingTiers,
        MAX_LOCAL_PRICING_BYTES, MAX_RATE_MICRO_USD, parse_optional_rate_micro_usd,
        parse_rate_micro_usd,
    };
    use crate::pricing::{ModelRate, catalog_service_tier};

    fn row(model_id: &str, input: i64, cache_write: Option<i64>) -> ModelRate {
        ModelRate {
            model_id: model_id.to_owned(),
            minimum_input_tokens: None,
            maximum_input_tokens: None,
            input,
            cached_input: input / 10,
            cache_write,
            output: input * 5,
        }
    }

    fn tier(version: &str, models: Vec<ModelRate>) -> LocalPricingTier {
        LocalPricingTier {
            version: version.to_owned(),
            models,
        }
    }

    fn table() -> LocalPricingTable {
        LocalPricingTable {
            schema_version: LOCAL_PRICING_SCHEMA_VERSION,
            synced_at_ms: 1_790_000_000_000,
            source_url: "https://developers.openai.com/api/docs/pricing/".to_owned(),
            tiers: LocalPricingTiers {
                standard: tier(
                    "openai-standard-synced-2026-09-30",
                    vec![row("gpt-6-sol", 2_000_000, Some(2_500_000))],
                ),
                priority: tier(
                    "openai-priority-synced-2026-09-30",
                    vec![row("gpt-6-sol", 4_000_000, Some(5_000_000))],
                ),
            },
        }
    }

    #[test]
    fn money_parser_accepts_the_exact_decimal_grammar() {
        for (text, expected) in [
            ("$10.00", 10_000_000),
            ("$0.125", 125_000),
            ("$0.0625", 62_500),
            ("$2.50", 2_500_000),
            ("$1", 1_000_000),
            ("$0", 0),
            (" $.50 ", 500_000),
            ("$1000", MAX_RATE_MICRO_USD),
        ] {
            assert_eq!(parse_rate_micro_usd(text).unwrap(), expected, "{text}");
        }
    }

    #[test]
    fn money_parser_rejects_inexact_signed_or_oversized_amounts() {
        for text in [
            "-",
            "",
            "$",
            "10.00",
            "1e3",
            "$1e3",
            "$1E3",
            "NaN",
            "inf",
            "-$1.00",
            "$-1",
            "$+1",
            "$1,000",
            "$0.0000001",
            "$1.2345678",
            "$1000.01",
            "$abc",
            "$1.2.3",
            "$.",
        ] {
            assert!(
                matches!(parse_rate_micro_usd(text), Err(LocalPricingError::Money)),
                "{text}"
            );
        }
    }

    #[test]
    fn optional_money_parser_maps_only_the_dash_to_an_absent_rate() {
        assert_eq!(parse_optional_rate_micro_usd("-").unwrap(), None);
        assert_eq!(
            parse_optional_rate_micro_usd(" $0.125 ").unwrap(),
            Some(125_000)
        );
        assert!(matches!(
            parse_optional_rate_micro_usd(""),
            Err(LocalPricingError::Money)
        ));
    }

    #[test]
    fn local_table_validation_rejects_structural_violations() {
        assert!(table().validate().is_ok());

        let mut schema = table();
        schema.schema_version = 2;
        assert!(matches!(
            schema.validate(),
            Err(LocalPricingError::Invalid("schema_version"))
        ));

        let mut negative_sync = table();
        negative_sync.synced_at_ms = -1;
        assert!(matches!(
            negative_sync.validate(),
            Err(LocalPricingError::Invalid("synced_at_ms"))
        ));

        let mut unresolvable_source = table();
        unresolvable_source.source_url = "not a url".to_owned();
        assert!(matches!(
            unresolvable_source.validate(),
            Err(LocalPricingError::Invalid("source_url"))
        ));

        let mut empty_source = table();
        empty_source.source_url = String::new();
        assert!(matches!(
            empty_source.validate(),
            Err(LocalPricingError::Invalid("source_url"))
        ));

        let mut mismatched_tier = table();
        mismatched_tier.tiers.priority.version = "openai-standard-synced-2026-09-30".to_owned();
        assert!(matches!(
            mismatched_tier.validate(),
            Err(LocalPricingError::Invalid("invalid catalog version"))
        ));

        let mut empty_models = table();
        empty_models.tiers.standard.models.clear();
        assert!(matches!(
            empty_models.validate(),
            Err(LocalPricingError::Invalid("invalid catalog models"))
        ));

        let mut zero_cache_write = table();
        zero_cache_write.tiers.standard.models[0].cache_write = Some(0);
        assert!(matches!(
            zero_cache_write.validate(),
            Err(LocalPricingError::Invalid("invalid catalog rate"))
        ));

        let mut three_bands = table();
        let template = three_bands.tiers.standard.models[0].clone();
        three_bands.tiers.standard.models = vec![
            ModelRate {
                maximum_input_tokens: Some(272_000),
                ..template.clone()
            },
            ModelRate {
                minimum_input_tokens: Some(272_001),
                maximum_input_tokens: Some(300_000),
                ..template.clone()
            },
            ModelRate {
                minimum_input_tokens: Some(300_001),
                ..template.clone()
            },
        ];
        assert!(matches!(
            three_bands.validate(),
            Err(LocalPricingError::Invalid("invalid catalog bands"))
        ));

        let mut duplicated = table();
        let duplicate = duplicated.tiers.standard.models[0].clone();
        duplicated.tiers.standard.models.push(duplicate);
        assert!(matches!(
            duplicated.validate(),
            Err(LocalPricingError::Invalid("invalid catalog rate"))
        ));
    }

    #[test]
    fn store_publishes_a_private_atomic_table_and_reads_it_back() {
        let directory = tempdir().expect("temporary directory");
        let store = LocalPricingStore::new(directory.path().to_path_buf());

        assert!(matches!(store.load(), LocalPricingLoad::Missing));

        let published = store.publish(&table()).expect("publish");
        assert_eq!(published, store.path());
        assert_eq!(
            store.path().file_name().and_then(|name| name.to_str()),
            Some(LOCAL_PRICING_FILE_NAME)
        );
        let directory_path = store.directory();
        let file = fs::read_to_string(store.path()).expect("published table");
        assert!(file.contains("\"schema_version\": 1"));
        assert!(file.contains("\"default\""));
        assert!(file.contains("\"priority\""));
        let leftover = fs::read_dir(&directory_path)
            .expect("directory listing")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .count();
        assert_eq!(leftover, 0);

        match store.load() {
            LocalPricingLoad::Loaded(loaded) => {
                assert_eq!(loaded, table());
                assert_eq!(
                    catalog_service_tier(
                        &loaded.tier(crate::pricing::CatalogTier::Standard).version
                    ),
                    Some("default")
                );
            }
            other => panic!("expected a loaded table, got {other:?}"),
        }

        let mut replacement = table();
        replacement.synced_at_ms += 60_000;
        replacement.tiers.standard.version = "openai-standard-synced-2026-10-01".to_owned();
        store.publish(&replacement).expect("replace");
        match store.load() {
            LocalPricingLoad::Loaded(loaded) => assert_eq!(loaded, replacement),
            other => panic!("expected a loaded table, got {other:?}"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn store_publishes_private_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempdir().expect("temporary directory");
        let store = LocalPricingStore::new(directory.path().to_path_buf());
        store.publish(&table()).expect("publish");

        let file_mode = fs::metadata(store.path())
            .expect("file metadata")
            .permissions()
            .mode()
            & 0o777;
        let directory_mode = fs::metadata(store.directory())
            .expect("directory metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(file_mode, 0o600);
        assert_eq!(directory_mode, 0o700);

        store.remove().expect("remove");
        assert!(matches!(store.load(), LocalPricingLoad::Missing));
    }

    #[test]
    fn store_reports_corrupt_content_without_panicking() {
        let directory = tempdir().expect("temporary directory");
        let store = LocalPricingStore::new(directory.path().to_path_buf());
        fs::create_dir_all(store.directory()).expect("directory");
        fs::write(store.path(), b"{ not json").expect("garbage");
        assert!(matches!(store.load(), LocalPricingLoad::Corrupt));

        fs::write(
            store.path(),
            serde_json::to_vec(&table()).expect("serialize"),
        )
        .expect("write table");
        assert!(matches!(store.load(), LocalPricingLoad::Loaded(_)));

        let mut broken = table();
        broken.tiers.standard.models.clear();
        fs::write(
            store.path(),
            serde_json::to_vec(&broken).expect("serialize"),
        )
        .expect("write invalid table");
        assert!(matches!(store.load(), LocalPricingLoad::Corrupt));

        fs::write(store.path(), vec![b'x'; MAX_LOCAL_PRICING_BYTES + 1])
            .expect("write oversized table");
        assert!(matches!(store.load(), LocalPricingLoad::Corrupt));

        fs::remove_file(store.path()).expect("remove oversized table");
        fs::create_dir(store.path()).expect("directory in place of the file");
        assert!(matches!(store.load(), LocalPricingLoad::Corrupt));
    }

    #[cfg(unix)]
    #[test]
    fn store_refuses_a_symlinked_table_path() {
        let directory = tempdir().expect("temporary directory");
        let store = LocalPricingStore::new(directory.path().to_path_buf());
        fs::create_dir_all(store.directory()).expect("directory");
        let target = directory.path().join("elsewhere.json");
        fs::write(&target, serde_json::to_vec(&table()).expect("serialize")).expect("target");
        std::os::unix::fs::symlink(&target, store.path()).expect("symlink");

        assert!(matches!(store.load(), LocalPricingLoad::Corrupt));
        assert!(matches!(
            store.publish(&table()),
            Err(LocalPricingError::UnsafePath)
        ));
        assert!(matches!(store.remove(), Err(LocalPricingError::UnsafePath)));
    }

    #[test]
    fn publish_rejects_an_invalid_table_before_touching_the_filesystem() {
        let directory = tempdir().expect("temporary directory");
        let store = LocalPricingStore::new(directory.path().to_path_buf());
        let mut broken = table();
        broken.tiers.priority.version = "openai-standard-synced-2026-09-30".to_owned();

        assert!(matches!(
            store.publish(&broken),
            Err(LocalPricingError::Invalid(_))
        ));
        assert!(!store.directory().exists());
        assert!(matches!(store.load(), LocalPricingLoad::Missing));
    }
}
