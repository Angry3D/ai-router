//! Version binding for release-side updater signature verification.
//!
//! `tauri-plugin-updater` is able to reject an update whose announced version
//! differs from the version recorded in the signature's trusted comment
//! (`requireSignedVersion`). A release that carries no such record is rejected
//! by every hardened client, so the release pipeline asserts the same
//! relationship while the release is still an unpublished draft.

use semver::Version;

/// Why a signature's version binding was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignedVersionError {
    /// The signature carries no `version:` field, so a hardened client rejects it.
    Missing,
    /// The signature was produced for a different version than the announced one.
    Mismatch,
}

/// Reads the `version:` field of a minisign signature's trusted comment.
///
/// The Tauri CLI writes the trusted comment as tab separated `key:value` pairs,
/// for example `timestamp:1700000000` then `file:app.tar.gz` then `version:1.2.3`,
/// and that comment is covered by the signature's global signature. Call this
/// only after the signature itself verified.
#[must_use]
pub fn signed_version(signature_text: &str) -> Option<&str> {
    let trusted_comment = signature_text
        .lines()
        .find_map(|line| line.strip_prefix("trusted comment: "))?;
    trusted_comment
        .split('\t')
        .find_map(|field| field.strip_prefix("version:"))
}

/// Checks that a verified signature belongs to `announced_version`.
///
/// Comparison mirrors `tauri-plugin-updater`: semver equality with an optional
/// `v` prefix, falling back to a literal comparison for values that are not
/// valid semver. `require` mirrors `requireSignedVersion`, so a signature
/// without a version is rejected when it is set and accepted when it is not.
///
/// # Errors
///
/// Returns [`SignedVersionError::Missing`] when the signature carries no
/// version and `require` is set, and [`SignedVersionError::Mismatch`] when the
/// recorded version differs from `announced_version`.
pub fn verify_signed_version(
    signature_text: &str,
    announced_version: &str,
    require: bool,
) -> Result<(), SignedVersionError> {
    let Some(signed) = signed_version(signature_text) else {
        return if require {
            Err(SignedVersionError::Missing)
        } else {
            Ok(())
        };
    };
    if version_matches(signed, announced_version) {
        Ok(())
    } else {
        Err(SignedVersionError::Mismatch)
    }
}

fn version_matches(signed: &str, announced: &str) -> bool {
    match (
        Version::parse(signed.trim_start_matches('v')),
        Version::parse(announced.trim_start_matches('v')),
    ) {
        (Ok(signed), Ok(announced)) => signed == announced,
        _ => signed == announced,
    }
}

#[cfg(test)]
mod tests {
    use super::{SignedVersionError, signed_version, verify_signed_version};

    // Shape of a signature produced by `tauri build` / `tauri signer sign`
    // since CLI 2.11.5, after the `.sig` file is base64 decoded.
    const CURRENT: &str = "untrusted comment: signature from tauri secret key\nAAAAAAAA\ntrusted comment: timestamp:1700000000\tfile:AI Router.app.tar.gz\tversion:0.4.3\nBBBBBBBB";
    // Shape produced before the CLI recorded the version.
    const LEGACY: &str = "untrusted comment: signature from tauri secret key\nAAAAAAAA\ntrusted comment: timestamp:1600000000\tfile:AI Router.app.tar.gz\nBBBBBBBB";

    #[test]
    fn reads_the_signed_version_and_ignores_lookalike_fields() {
        assert_eq!(signed_version(CURRENT), Some("0.4.3"));
        assert_eq!(signed_version(LEGACY), None);
        assert_eq!(
            signed_version(
                "untrusted comment: x\nAAAAAAAA\ntrusted comment: timestamp:1\tfile:app-version:2.tar.gz\nBBBBBBBB"
            ),
            None
        );
    }

    #[test]
    fn accepts_a_matching_signed_version() {
        assert_eq!(verify_signed_version(CURRENT, "0.4.3", true), Ok(()));
        assert_eq!(verify_signed_version(CURRENT, "0.4.3", false), Ok(()));
        assert_eq!(verify_signed_version(CURRENT, "v0.4.3", true), Ok(()));
    }

    #[test]
    fn rejects_a_signature_produced_for_another_version() {
        assert_eq!(
            verify_signed_version(CURRENT, "9.9.9", true),
            Err(SignedVersionError::Mismatch)
        );
        assert_eq!(
            verify_signed_version(CURRENT, "9.9.9", false),
            Err(SignedVersionError::Mismatch)
        );
    }

    #[test]
    fn rejects_a_signature_without_a_version_when_required() {
        assert_eq!(
            verify_signed_version(LEGACY, "0.4.3", true),
            Err(SignedVersionError::Missing)
        );
        assert_eq!(verify_signed_version(LEGACY, "0.4.3", false), Ok(()));
    }

    // The plugin falls back to a literal comparison when either side is not
    // semver, and that comparison sees the raw values, `v` prefix included.
    #[test]
    fn compares_literally_when_a_value_is_not_semver() {
        let non_semver =
            "untrusted comment: x\nAAAAAAAA\ntrusted comment: timestamp:1\tversion:1.2\nBBBBBBBB";
        assert_eq!(verify_signed_version(non_semver, "1.2", true), Ok(()));
        assert_eq!(
            verify_signed_version(non_semver, "v1.2", true),
            Err(SignedVersionError::Mismatch)
        );
        assert_eq!(
            verify_signed_version(non_semver, "1.2.0", true),
            Err(SignedVersionError::Mismatch)
        );
    }
}
