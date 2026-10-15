//! Independent strict acceptance and byte-level password compatibility audit.
#![allow(clippy::indexing_slicing, clippy::unwrap_used)]

use base64::{engine::general_purpose::STANDARD, Engine as _};
use bittensor_core::{
    keyfile_env::{decode_password_from_environment, encode_password_for_environment},
    keys,
};
use sp_core::{ed25519, sr25519, Pair};

#[allow(clippy::arithmetic_side_effects)] // Match the original nonempty-key implementation.
fn old_xor(name: &str, bytes: &[u8]) -> Vec<u8> {
    bytes
        .iter()
        .enumerate()
        .map(|(index, byte)| byte ^ name.as_bytes()[index % name.len()])
        .collect()
}

#[test]
fn independent_legacy_codec_matches_all_utf8_lengths_and_keys() {
    for name in [
        "A",
        "WALLET_α",
        "🔑",
        "=\0",
        "BT_PW_test_longer_than_password",
    ] {
        for password in [
            "",
            "short",
            "pässwörd\0\n",
            "🔑秘密é",
            "a very long UTF-8 password 🔑秘密é repeated beyond the key length",
        ] {
            let expected = STANDARD.encode(old_xor(name, password.as_bytes()));
            assert_eq!(
                encode_password_for_environment(name, password).unwrap(),
                expected
            );
            assert_eq!(
                decode_password_from_environment(name, &format!("\u{2003}\n{expected}\t")).unwrap(),
                password
            );
        }
    }
    for input in [
        "A",
        "AAAA=",
        "AAAA!!!!!",
        "YWJjZGVmZ2hpamtsbW5vcA==!",
        "AB==",
    ] {
        assert!(STANDARD.decode(input).is_err());
        assert!(decode_password_from_environment("BT_PW_TEST", input).is_err());
    }
}

#[test]
fn strict_agrees_with_actual_pinned_substrate_algorithms() {
    let messages = [
        b"".as_slice(),
        b"<Bytes>already wrapped</Bytes>",
        b"binary\0\xff\n",
        b"strict message",
    ];
    let sr = sr25519::Pair::from_seed(&[37; 32]);
    let ed = ed25519::Pair::from_seed(&[41; 32]);
    let sr_address = keys::ss58_from_public(sr.public().0, 42);
    let ed_address = keys::ss58_from_public(ed.public().0, 42);
    for message in messages {
        let sr_sig = sr.sign(message);
        let ed_sig = ed.sign(message);
        assert_eq!(
            keys::verify_strict(message, &sr_sig.0, &sr_address, keys::CRYPTO_SR25519).unwrap(),
            sr25519::Pair::verify(&sr_sig, message, &sr.public())
        );
        assert_eq!(
            keys::verify_strict(message, &ed_sig.0, &ed_address, keys::CRYPTO_ED25519).unwrap(),
            ed25519::Pair::verify(&ed_sig, message, &ed.public())
        );
        let wrapped = [b"<Bytes>".as_slice(), message, b"</Bytes>".as_slice()].concat();
        let sr_sig = sr.sign(&wrapped);
        let ed_sig = ed.sign(&wrapped);
        assert!(keys::verify(message, &sr_sig.0, &sr_address, keys::CRYPTO_SR25519).unwrap());
        assert!(keys::verify(message, &ed_sig.0, &ed_address, keys::CRYPTO_ED25519).unwrap());
        assert!(
            !keys::verify_strict(message, &sr_sig.0, &sr_address, keys::CRYPTO_SR25519).unwrap()
        );
        assert!(
            !keys::verify_strict(message, &ed_sig.0, &ed_address, keys::CRYPTO_ED25519).unwrap()
        );
    }
}

#[test]
fn base64_invalid_suffix_can_leave_partial_decoded_secret_bytes() {
    let name = "BT_PW_TEST";
    let password = "malformed input secret contains multiple decode blocks";
    let encoded = STANDARD.encode(old_xor(name, password.as_bytes()));
    let mut partial = vec![];
    assert!(STANDARD
        .decode_vec(format!("{encoded}!!!!"), &mut partial)
        .is_err());
    assert!(partial.starts_with(&old_xor(name, password.as_bytes())[..24]));
    assert!(decode_password_from_environment(name, &format!("{encoded}!!!!")).is_err());
}

#[test]
fn guarded_preallocation_covers_both_success_and_partial_error_without_growth() {
    for password in [
        "",
        "short",
        "long enough input to pass multiple decode blocks and expose a malformed suffix",
    ] {
        let encoded = STANDARD.encode(old_xor("BT_PW_TEST", password.as_bytes()));
        for suffix in ["", "!!!!"] {
            let input = format!("{encoded}{suffix}");
            let mut decoded = zeroize::Zeroizing::new(Vec::with_capacity(
                base64::decoded_len_estimate(input.len()),
            ));
            let pointer = decoded.as_ptr();
            let capacity = decoded.capacity();
            let result = STANDARD.decode_vec(&input, &mut decoded);
            assert_eq!(decoded.as_ptr(), pointer);
            assert_eq!(decoded.capacity(), capacity);
            assert_eq!(result.is_ok(), suffix.is_empty());
        }
    }
}

#[test]
fn pure_encoding_preserves_a_preexisting_child_environment_value() {
    const NAME: &str = "RTAO_QUALITY_PASSWORD_PARENT_VALUE";
    if std::env::var("RTAO_QUALITY_PASSWORD_CHILD").is_ok() {
        assert_eq!(std::env::var(NAME).unwrap(), "existing-value");
        let encoded = encode_password_for_environment(NAME, "password").unwrap();
        assert!(!encoded.is_empty());
        assert_eq!(
            decode_password_from_environment(NAME, &encoded).unwrap(),
            "password"
        );
        assert_eq!(std::env::var(NAME).unwrap(), "existing-value");
        return;
    }
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .arg("--exact")
        .arg("pure_encoding_preserves_a_preexisting_child_environment_value")
        .arg("--nocapture")
        .env("RTAO_QUALITY_PASSWORD_CHILD", "1")
        .env(NAME, "existing-value")
        .status()
        .unwrap();
    assert!(status.success());
}
