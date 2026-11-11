// Derived from RaoFoundation/subtensor sdk/python/tests/unit/test_sp_core_keypair.py @ d967d13588834f4f5aaa8ef4c8fc1a865d581462 (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/tests/unit/test_sp_core_parity.py @ d967d13588834f4f5aaa8ef4c8fc1a865d581462 (Apache-2.0); modified by rtao.
//! The v490 core key surface rtao re-exports: ECDSA accounts and lengths,
//! the strict verifier and pure password encoding (D-179).

use std::error::Error;

use blake2::{Blake2b, Digest, digest::consts::U32};
use rtao_core::keyfile_env::encode_password_for_environment;
use rtao_core::keys::{
    self, CRYPTO_ECDSA, CRYPTO_ED25519, CRYPTO_SR25519, ECDSA_PUBLIC_KEY_LEN, ECDSA_SIGNATURE_LEN,
    ED25519_SIGNATURE_LEN, Keypair, SR25519_SIGNATURE_LEN,
};

/// The secp256k1 generator point G, compressed.
const ECDSA_PUBKEY: [u8; 33] = [
    0x02, 0x79, 0xbe, 0x66, 0x7e, 0xf9, 0xdc, 0xbb, 0xac, 0x55, 0xa0, 0x62, 0x95, 0xce, 0x87, 0x0b,
    0x07, 0x02, 0x9b, 0xfc, 0xdb, 0x2d, 0xce, 0x28, 0xd9, 0x59, 0xf2, 0x81, 0x5b, 0x16, 0xf8, 0x17,
    0x98,
];

// pyt: unit/test_sp_core_keypair.py::test_ecdsa_crypto_type_is_multisignature_index
// pyt: unit/test_sp_core_keypair.py::test_key_and_signature_lengths
#[test]
fn ecdsa_code_and_lengths_match_multisignature() {
    assert_eq!(CRYPTO_ECDSA, 2);
    assert_eq!(ECDSA_PUBLIC_KEY_LEN, 33);
    assert_eq!(ED25519_SIGNATURE_LEN, 64);
    assert_eq!(SR25519_SIGNATURE_LEN, 64);
    assert_eq!(ECDSA_SIGNATURE_LEN, 65);
}

// pyt: unit/test_sp_core_keypair.py::test_ecdsa_ss58_is_blake2_256_of_compressed_pubkey
#[test]
fn ecdsa_account_is_blake2_256_of_the_compressed_key() -> Result<(), Box<dyn Error>> {
    let account: [u8; 32] = Blake2b::<U32>::digest(ECDSA_PUBKEY).into();
    assert_eq!(keys::account_id_from_ecdsa_public(&ECDSA_PUBKEY)?, account);
    for format in [42, 0] {
        let address = keys::ss58_from_ecdsa_public(&ECDSA_PUBKEY, format)?;
        assert_eq!(address, keys::ss58_from_public(account, format));
        assert_eq!(keys::public_key_from_ss58(&address)?, account);
    }
    Ok(())
}

// pyt: unit/test_sp_core_keypair.py::test_ecdsa_ss58_rejects_non_compressed_keys
#[test]
fn ecdsa_account_refuses_keys_that_are_not_compressed() {
    let mut uncompressed = vec![0x04];
    uncompressed.extend_from_slice(&ECDSA_PUBKEY[1..]);
    let mut long = vec![0x04];
    long.extend_from_slice(&[0; 64]);
    for bad in [&ECDSA_PUBKEY[1..], uncompressed.as_slice(), long.as_slice()] {
        assert!(keys::ss58_from_ecdsa_public(bad, 42).is_err());
        assert!(keys::account_id_from_ecdsa_public(bad).is_err());
    }
}

// pyt: unit/test_sp_core_parity.py::test_strict_verifier_omits_only_legacy_wrapper_fallback
#[test]
fn strict_verifier_omits_only_the_legacy_wrapper_fallback() -> Result<(), Box<dyn Error>> {
    let phrase = "abandon abandon abandon abandon abandon abandon \
                  abandon abandon abandon abandon abandon about";
    for scheme in [CRYPTO_ED25519, CRYPTO_SR25519] {
        let key = Keypair::from_mnemonic(phrase, scheme, None)?;
        let address = key.ss58_address();
        let message = b"strict-verification";
        let signature = key.sign(message)?;
        assert!(keys::verify_strict(message, &signature, &address, scheme)?);
        assert!(!keys::verify_strict(
            b"changed", &signature, &address, scheme
        )?);
        let wrapped = [b"<Bytes>".as_slice(), message, b"</Bytes>"].concat();
        let signature = key.sign(&wrapped)?;
        assert!(keys::verify(message, &signature, &address, scheme)?);
        assert!(!keys::verify_strict(message, &signature, &address, scheme)?);
        assert!(keys::verify_strict(&wrapped, &signature, &address, scheme)?);
        let short = keys::verify_strict(message, &signature[..63], &address, scheme)
            .err()
            .ok_or("a short signature verified")?;
        assert!(short.to_string().contains("signature length"), "{short}");
    }
    Ok(())
}

// pyt: unit/test_sp_core_parity.py::test_pure_password_encoding_matches_legacy_bytes_without_environment_writes
#[test]
fn pure_password_encoding_matches_legacy_bytes_without_environment_writes()
-> Result<(), Box<dyn Error>> {
    let name = "WALLET_α";
    let before = std::env::var_os(name);
    assert_eq!(
        encode_password_for_environment(name, "pässwörd\0\n")?,
        "J4LoPzYjnHjDM0FG"
    );
    assert_eq!(std::env::var_os(name), before);
    let empty = encode_password_for_environment("", "pässwörd\0\n")
        .err()
        .ok_or("an empty name encoded")?;
    assert!(empty.to_string().contains("env var name must not be empty"));
    Ok(())
}
