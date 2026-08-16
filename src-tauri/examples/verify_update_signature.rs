use std::{env, fs, path::PathBuf};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use minisign_verify::{PublicKey, Signature};

#[derive(Debug)]
struct VerificationFailed;

fn verify() -> Result<(), VerificationFailed> {
    let mut arguments = env::args_os().skip(1);
    let archive = arguments
        .next()
        .map(PathBuf::from)
        .ok_or(VerificationFailed)?;
    let signature = arguments
        .next()
        .map(PathBuf::from)
        .ok_or(VerificationFailed)?;
    if arguments.next().is_some() {
        return Err(VerificationFailed);
    }
    let public_key = env::var("AI_ROUTER_UPDATER_PUBLIC_KEY").map_err(|_| VerificationFailed)?;
    let public_key = STANDARD
        .decode(public_key)
        .map_err(|_| VerificationFailed)?;
    let public_key = std::str::from_utf8(&public_key).map_err(|_| VerificationFailed)?;
    let public_key = PublicKey::decode(public_key).map_err(|_| VerificationFailed)?;

    let signature = fs::read_to_string(signature).map_err(|_| VerificationFailed)?;
    let signature = STANDARD
        .decode(signature.trim())
        .map_err(|_| VerificationFailed)?;
    let signature = std::str::from_utf8(&signature).map_err(|_| VerificationFailed)?;
    let signature = Signature::decode(signature).map_err(|_| VerificationFailed)?;
    let archive = fs::read(archive).map_err(|_| VerificationFailed)?;
    public_key
        .verify(&archive, &signature, true)
        .map_err(|_| VerificationFailed)
}

fn main() {
    if verify().is_err() {
        eprintln!("update signature verification failed");
        std::process::exit(1);
    }
}
