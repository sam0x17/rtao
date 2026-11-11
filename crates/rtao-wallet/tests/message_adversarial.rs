//! Adversarial message verification against receiving addresses (D-173).
//!
//! A receiving address binds its descriptor's scheme and account: no other
//! key, scheme, tampered envelope or classical signature verifies under it.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use rtao_core::keys::{
    CRYPTO_HASHED, CRYPTO_HASHED_ED25519, CRYPTO_SR25519, Keypair, encode_hashed_receiving_address,
};
use rtao_signer::{AuthoritySigner, KeypairAuthoritySigner};
use rtao_wallet::verify_message_as;

fn hashed(seed: u8, crypto: u8) -> KeypairAuthoritySigner {
    KeypairAuthoritySigner::new(Keypair::from_seed(&[seed; 32], crypto).unwrap()).unwrap()
}

fn receiving(key: &KeypairAuthoritySigner) -> String {
    encode_hashed_receiving_address(key.descriptor()).unwrap()
}

#[test]
fn receiving_addresses_accept_only_their_own_key_and_scheme() {
    for (crypto, code, other_code) in [(CRYPTO_HASHED, 4u8, 6u8), (CRYPTO_HASHED_ED25519, 6, 4)] {
        let victim = hashed(1, crypto);
        let attacker = hashed(2, crypto);
        let address = receiving(&victim);
        let genuine = hex::encode(victim.sign_message_at(2, b"hello").unwrap());
        let forged = hex::encode(attacker.sign_message_at(2, b"hello").unwrap());
        assert!(verify_message_as("hello", &genuine, &address, None).unwrap());
        assert!(verify_message_as("hello", &genuine, &address, Some(code)).unwrap());
        assert!(!verify_message_as("hello", &forged, &address, None).unwrap());
        // A trusted scheme other than the descriptor's never verifies.
        for wrong in [0u8, 1, other_code] {
            assert!(!verify_message_as("hello", &genuine, &address, Some(wrong)).unwrap());
        }
        // The victim's envelope under the victim's bare SS58 address verifies
        // only with the hashed scheme, never a classical one.
        let ss58 = AuthoritySigner::account(&victim).to_string();
        assert!(verify_message_as("hello", &genuine, &ss58, Some(code)).unwrap());
        for wrong in [0u8, 1, other_code] {
            assert!(!verify_message_as("hello", &genuine, &ss58, Some(wrong)).unwrap_or(false));
        }
        // Every single-byte change fails.
        let raw = victim.sign_message_at(3, b"hello").unwrap();
        for index in 0..raw.len() {
            let mut tampered = raw.clone();
            tampered[index] ^= 0x01;
            assert!(
                !verify_message_as("hello", &hex::encode(&tampered), &address, None).unwrap(),
                "byte {index}"
            );
        }
    }
}

#[test]
fn classical_signatures_never_verify_as_a_receiving_address() {
    let victim = hashed(3, CRYPTO_HASHED);
    let address = receiving(&victim);
    let classical = Keypair::from_seed(&[3; 32], CRYPTO_SR25519).unwrap();
    let raw = hex::encode(classical.sign(b"hello").unwrap());
    assert!(!verify_message_as("hello", &raw, &address, None).unwrap());
    assert!(!verify_message_as("hello", &raw, &address, Some(1)).unwrap());
}
