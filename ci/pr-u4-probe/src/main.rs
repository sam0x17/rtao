//! Isolated PR-U4 core build over crates.io w3f-bls 0.1.3.

#[path = "../deterministic.rs"]
mod deterministic;

use bittensor_core::timelock::{UserData, WeightsTlockPayload};
use parity_scale_codec::Encode;
use serde::{Deserialize, Serialize};
use std::{error::Error, fs, path::PathBuf};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    reveal_round: u64,
    hotkey_hex: String,
    uids: Vec<u16>,
    values: Vec<u16>,
    version_key: u64,
}

#[derive(Serialize)]
struct Vector {
    format: u8,
    bls_source: &'static str,
    tle_commit: &'static str,
    reveal_round: u64,
    hotkey_hex: String,
    uids: Vec<u16>,
    values: Vec<u16>,
    version_key: u64,
    ephemeral_key_hex: String,
    rng_seed_hex: String,
    payload_hex: String,
    ciphertext_hex: String,
    envelope_hex: String,
    registry_decrypted_payload_hex: Option<String>,
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args_os().skip(1);
    let usage = "usage: rtao-pr-u4-probe INPUT OUTPUT [QUICKNET_SIGNATURE_JSON]";
    let input = PathBuf::from(args.next().ok_or(usage)?);
    let output = PathBuf::from(args.next().ok_or(usage)?);
    let signature_path = args.next().map(PathBuf::from);
    if args.next().is_some() {
        return Err(usage.into());
    }
    let request: Request = serde_json::from_slice(&fs::read(input)?)?;
    let hotkey = hex::decode(&request.hotkey_hex)?;
    if hotkey.len() != 32 || request.uids.len() != request.values.len() {
        return Err(
            "hotkey must contain 32 bytes and weight vectors must have equal length".into(),
        );
    }
    if deterministic::QUICKNET_PUBLIC_KEY != bittensor_core::timelock::constants::DRAND_PUBLIC_KEY {
        return Err("probe public key differs from the pinned core".into());
    }
    let payload = WeightsTlockPayload {
        hotkey,
        uids: request.uids.clone(),
        values: request.values.clone(),
        version_key: request.version_key,
    }
    .encode();
    let ciphertext = deterministic::encrypt(&payload, request.reveal_round)?;
    let envelope = UserData {
        encrypted_data: ciphertext.clone(),
        reveal_round: request.reveal_round,
    }
    .encode();
    let registry_decrypted_payload_hex = if let Some(signature_path) = signature_path {
        let signature: serde_json::Value = serde_json::from_slice(&fs::read(signature_path)?)?;
        if signature["round"].as_u64() != Some(request.reveal_round) {
            return Err("Quicknet signature round differs from the requested round".into());
        }
        let signature = signature["signature"]
            .as_str()
            .ok_or("missing Quicknet signature")?;
        let opened = bittensor_core::timelock::decrypt_with_signature(&envelope, signature)?;
        if opened != payload {
            return Err("registry BLS decryption changed the canonical core payload".into());
        }
        Some(hex::encode(opened))
    } else {
        None
    };
    let vector = Vector {
        format: 1,
        bls_source: "registry+w3f-bls=0.1.3",
        tle_commit: "5416406cfd32799e31e1795393d4916894de4468",
        reveal_round: request.reveal_round,
        hotkey_hex: request.hotkey_hex,
        uids: request.uids,
        values: request.values,
        version_key: request.version_key,
        ephemeral_key_hex: hex::encode(deterministic::EPHEMERAL_KEY),
        rng_seed_hex: hex::encode(deterministic::RNG_SEED),
        payload_hex: hex::encode(payload),
        ciphertext_hex: hex::encode(ciphertext),
        envelope_hex: hex::encode(envelope),
        registry_decrypted_payload_hex,
    };
    fs::write(output, serde_json::to_vec_pretty(&vector)?)?;
    Ok(())
}
