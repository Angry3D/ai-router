use std::{env, fs, path::PathBuf};

use ai_router_app_lib::update_signature::{SignedVersionError, verify_signed_version};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use minisign_verify::{PublicKey, Signature};

/// Verifies the updater archive signature and its version binding.
///
/// Arguments: `<archive> <signature> <expected-version>`. The public key comes
/// from `AI_ROUTER_UPDATER_PUBLIC_KEY`. A non-zero exit keeps the release
/// unpublished, so a signature without the version record fails closed here
/// instead of failing every hardened client later.
fn verify() -> Result<(), &'static str> {
    let mut arguments = env::args_os().skip(1);
    let archive = arguments
        .next()
        .map(PathBuf::from)
        .ok_or("update signature verification needs the archive path")?;
    let signature = arguments
        .next()
        .map(PathBuf::from)
        .ok_or("update signature verification needs the signature path")?;
    let expected_version = arguments
        .next()
        .and_then(|value| value.into_string().ok())
        .filter(|value| !value.is_empty())
        .ok_or("update signature verification needs the expected version")?;
    if arguments.next().is_some() {
        return Err("update signature verification takes exactly three arguments");
    }

    let public_key = env::var("AI_ROUTER_UPDATER_PUBLIC_KEY")
        .map_err(|_| "the updater public key is unavailable")?;
    let public_key = STANDARD
        .decode(public_key)
        .map_err(|_| "the updater public key is not valid base64")?;
    let public_key = std::str::from_utf8(&public_key)
        .map_err(|_| "the updater public key is not valid UTF-8")?;
    let public_key = PublicKey::decode(public_key)
        .map_err(|_| "the updater public key is not a minisign key")?;

    let signature_text =
        fs::read_to_string(signature).map_err(|_| "the signature is unreadable")?;
    let signature_decoded = STANDARD
        .decode(signature_text.trim())
        .map_err(|_| "the signature is not valid base64")?;
    let signature_text =
        std::str::from_utf8(&signature_decoded).map_err(|_| "the signature is not valid UTF-8")?;
    let signature = Signature::decode(signature_text)
        .map_err(|_| "the signature is not a minisign signature")?;

    let archive = fs::read(archive).map_err(|_| "the archive is unreadable")?;
    public_key
        .verify(&archive, &signature, true)
        .map_err(|_| "the archive signature does not verify")?;

    match verify_signed_version(signature_text, &expected_version, true) {
        Ok(()) => Ok(()),
        Err(SignedVersionError::Missing) => {
            Err("the signature records no version, which requireSignedVersion rejects")
        }
        Err(SignedVersionError::Mismatch) => {
            Err("the signature was produced for a different version than the expected one")
        }
    }
}

fn main() {
    if let Err(reason) = verify() {
        eprintln!("update signature verification failed: {reason}");
        std::process::exit(1);
    }
}
