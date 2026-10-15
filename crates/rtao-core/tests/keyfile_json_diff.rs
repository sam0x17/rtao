//! The staged portable codec must retain every pinned host-codec branch.
#![cfg(feature = "host")]
#![allow(clippy::unwrap_used, clippy::panic)]

use rtao_core::{keyfile_json, keyfiles, keys::Keypair};

fn compare(bytes: &[u8]) {
    match (
        keyfile_json::deserialize_keypair_from_keyfile_data(bytes),
        keyfiles::deserialize_keypair_from_keyfile_data(bytes),
    ) {
        (Ok(staged), Ok(core)) => {
            assert_eq!(staged.public_key_bytes(), core.public_key_bytes());
            assert_eq!(staged.crypto_type(), core.crypto_type());
            assert_eq!(staged.ss58_address(), core.ss58_address());
            assert_eq!(staged.mnemonic(), core.mnemonic());
            assert_eq!(staged.seed_bytes(), core.seed_bytes());
            match (
                staged.sign(b"keyfile differential"),
                core.sign(b"keyfile differential"),
            ) {
                (Ok(signature), Ok(_)) => {
                    assert!(core.verify(b"keyfile differential", &signature).unwrap())
                }
                (Err(left), Err(right)) => assert_eq!(left.to_string(), right.to_string()),
                _ => panic!("signing capability differs"),
            }
            let a: serde_json::Value = serde_json::from_slice(
                &keyfile_json::serialized_keypair_to_keyfile_data(&staged).unwrap(),
            )
            .unwrap();
            let b: serde_json::Value = serde_json::from_slice(
                &keyfiles::serialized_keypair_to_keyfile_data(&core).unwrap(),
            )
            .unwrap();
            assert_eq!(a, b);
        }
        (Err(left), Err(right)) => assert_eq!(left.to_string(), right.to_string()),
        _ => panic!("portable and host codec disagree"),
    }
}
#[test]
fn all_keyfile_fields_precedence_crypto_retry_and_historical_text_match_core() {
    let phrase = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
    for crypto in [0, 1] {
        let key = Keypair::from_mnemonic(phrase, crypto, None).unwrap();
        let data = keyfiles::serialized_keypair_to_keyfile_data(&key).unwrap();
        compare(&data);
        let mut json: serde_json::Value = serde_json::from_slice(&data).unwrap();
        json["cryptoType"] = serde_json::json!(crypto.to_string());
        compare(&serde_json::to_vec(&json).unwrap());
        json["cryptoType"] = serde_json::json!(1 - crypto);
        compare(&serde_json::to_vec(&json).unwrap());
        let object = json.as_object_mut().unwrap();
        let address = object.remove("ss58Address").unwrap();
        object.insert(" ss58Address".into(), address);
        compare(&serde_json::to_vec(&json).unwrap());
        json.as_object_mut().unwrap().remove("secretPhrase");
        compare(&serde_json::to_vec(&json).unwrap());
        json.as_object_mut().unwrap().remove("secretSeed");
        compare(&serde_json::to_vec(&json).unwrap());
        let public = Keypair::new(None, Some(&key.public_key_bytes()), crypto, 42).unwrap();
        compare(&keyfiles::serialized_keypair_to_keyfile_data(&public).unwrap());
    }
    compare(phrase.as_bytes());
    compare(format!("  0x{}\n", hex_seed()).as_bytes());
    let key = Keypair::from_seed(&[7; 32], 1).unwrap();
    compare(
        format!(
            "0x{}",
            key.private_key_bytes()
                .unwrap()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        )
        .as_bytes(),
    );
    for malformed in [
        b"".as_slice(),
        b"not json",
        b"{}",
        b"null",
        b"{\"encoded\":\"export\",\"encoding\":{}}",
        b"\xff",
        b"$NACL\xff",
    ] {
        compare(malformed);
    }
}

#[test]
fn secret_keyfile_errors_have_exact_documented_host_differences() {
    // D-036 redacts derivation diagnostics at the owned SDK boundary. Each
    // differing branch stays explicit; all other errors use compare() above.
    for (input, portable, pinned) in [
        (
            b"{\"secretSeed\":\"bad\"}".as_slice(),
            "keyfile error: invalid secret seed hex string",
            "keyfile error: invalid secret seed: Odd number of digits",
        ),
        (
            b"{\"cryptoType\":99,\"secretSeed\":\"00\"}".as_slice(),
            "crypto error: could not derive keypair from seed",
            "crypto error: unknown crypto type 99",
        ),
        (
            b"{\"secretSeed\":\"00\"}".as_slice(),
            "crypto error: could not derive keypair from seed",
            "crypto error: invalid seed: InvalidSeedLength",
        ),
        (
            b"{\"secretPhrase\":\"invalid\",\"secretSeed\":\"00\"}".as_slice(),
            "crypto error: could not derive keypair from mnemonic",
            "crypto error: invalid mnemonic: InvalidPhrase",
        ),
    ] {
        let left = keyfile_json::deserialize_keypair_from_keyfile_data(input)
            .err()
            .unwrap();
        let right = keyfiles::deserialize_keypair_from_keyfile_data(input)
            .err()
            .unwrap();
        assert_eq!(left.to_string(), portable);
        assert_eq!(right.to_string(), pinned);
    }
}
fn hex_seed() -> String {
    [7; 32].iter().map(|byte| format!("{byte:02x}")).collect()
}
#[test]
fn encryption_prefix_detection_matches_host_for_every_truncation() {
    for bytes in [
        b"$NACLanything".as_slice(),
        b"$ANSIBLE_VAULT;1.1;AES256",
        b"gAAAAAgarbage",
        b"{}",
        b"\xff\x00",
    ] {
        for len in 0..=bytes.len() {
            let bytes = &bytes[..len];
            assert_eq!(
                keyfile_json::keyfile_data_is_encrypted(bytes),
                keyfiles::keyfile_data_is_encrypted(bytes)
            );
            assert_eq!(
                keyfile_json::keyfile_data_encryption_method(bytes),
                keyfiles::keyfile_data_encryption_method(bytes)
            );
        }
    }
}
#[test]
#[ignore = "child process supplies a disposable encoded password variable"]
fn encoded_environment_password_matches_host_reader() {
    assert_eq!(
        keyfiles::get_password_from_environment("BT_PW_RTAO_DIFFERENTIAL_ONLY")
            .unwrap()
            .as_deref(),
        Some("disposable π password")
    );
}
#[test]
fn pure_environment_encoding_round_trips_through_core_in_child_process() {
    let name = "BT_PW_RTAO_DIFFERENTIAL_ONLY";
    let encoded =
        rtao_core::keyfile_env::encode_password_for_environment(name, "disposable π password")
            .unwrap();
    assert_eq!(
        rtao_core::keyfile_env::decode_password_from_environment(name, &encoded).unwrap(),
        "disposable π password"
    );
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .env(name, encoded)
        .args([
            "--ignored",
            "--exact",
            "encoded_environment_password_matches_host_reader",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
