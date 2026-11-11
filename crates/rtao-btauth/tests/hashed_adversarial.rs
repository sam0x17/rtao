//! Adversarial btauth verification of hashed SR/ED senders (D-171, D-172).
//!
//! A hashed sender's trusted scheme comes from configuration or finalized
//! state; these cases check that no other key, descriptor, scheme or tampered
//! envelope verifies as that sender.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use http::{HeaderMap, HeaderValue, Method};
use rtao_btauth::{
    Clock, ClockError, HEADER_CRYPTO, HEADER_HOTKEY, HEADER_NONCE, HEADER_RECEIVER,
    HEADER_SIGNATURE, HEADER_VERSION, HashedHttpSigner, InMemoryNonceStore, PROTOCOL, VERSION,
    Verifier, VerifyConfig, WrappedPolicy, sign_hashed,
};
use rtao_core::keys::{CRYPTO_HASHED, CRYPTO_HASHED_ED25519, CRYPTO_SR25519, Keypair};
use rtao_primitives::AccountId;
use rtao_signer::{AuthoritySigner, KeypairAuthoritySigner, Signer};
use sha2::{Digest, Sha256};

const NOW: u64 = 1_752_076_800_000_000_000;
const BODY: &[u8] = b"{\"prompt\": \"hello\"}";
const TARGET: &str = "/generate";

struct Fixed;
impl Clock for Fixed {
    fn now_ns(&self) -> Result<u64, ClockError> {
        Ok(NOW)
    }
}

fn receiver() -> AccountId {
    "5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty"
        .parse()
        .unwrap()
}

fn hashed(seed: u8, crypto: u8) -> KeypairAuthoritySigner {
    KeypairAuthoritySigner::new(Keypair::from_seed(&[seed; 32], crypto).unwrap()).unwrap()
}

fn payload(scheme: &str, sender: &AccountId, receiver: &AccountId) -> Vec<u8> {
    let body = hex::encode(Sha256::digest(BODY));
    format!("{PROTOCOL}\n{scheme}\nPOST\n{TARGET}\n{body}\n{NOW}\n{sender}\n{receiver}")
        .into_bytes()
}

fn headers(scheme: &str, sender: &AccountId, receiver: &AccountId, signature: &[u8]) -> HeaderMap {
    let mut headers = HeaderMap::new();
    for (name, value) in [
        (HEADER_VERSION, VERSION.to_string()),
        (HEADER_HOTKEY, sender.to_string()),
        (HEADER_NONCE, NOW.to_string()),
        (HEADER_CRYPTO, scheme.to_owned()),
        (HEADER_SIGNATURE, format!("0x{}", hex::encode(signature))),
        (HEADER_RECEIVER, receiver.to_string()),
    ] {
        headers.insert(
            http::header::HeaderName::from_bytes(name.as_bytes()).unwrap(),
            HeaderValue::from_str(&value).unwrap(),
        );
    }
    headers
}

fn verifier(policy: WrappedPolicy, trusted: &[(AccountId, u8)]) -> Verifier<InMemoryNonceStore> {
    let verifier = Verifier::new(
        receiver(),
        VerifyConfig {
            wrapped: policy,
            ..VerifyConfig::default()
        },
        Arc::new(InMemoryNonceStore::new()),
    )
    .unwrap();
    verifier
        .configure_verifier(
            trusted
                .iter()
                .map(|(account, code)| (account.to_string(), *code)),
        )
        .unwrap();
    verifier
}

fn accepted(verifier: &Verifier<InMemoryNonceStore>, headers: &HeaderMap) -> bool {
    verifier
        .authenticate(headers, &Method::POST, TARGET, BODY, NOW)
        .is_ok()
}

#[test]
fn payload_builder_matches_the_library() {
    let victim = hashed(1, CRYPTO_HASHED);
    let prepared = HashedHttpSigner::new(Arc::new(victim.clone()), 2).unwrap();
    let request = futures::executor::block_on(sign_hashed(
        &prepared,
        &Method::POST,
        TARGET,
        BODY,
        Some(&receiver()),
        &Fixed,
    ))
    .unwrap();
    let signature = request.get(HEADER_SIGNATURE).unwrap().to_str().unwrap();
    let rebuilt = headers(
        "hashed",
        &Signer::account(&victim),
        &receiver(),
        &hex::decode(signature.trim_start_matches("0x")).unwrap(),
    );
    let trusted = verifier(
        WrappedPolicy::AcceptLikePython,
        &[(Signer::account(&victim), 4)],
    );
    assert!(accepted(&trusted, &rebuilt));
}

/// Another hashed key, its own descriptor in the envelope, signing a payload
/// that names the victim, never verifies as the victim.
#[test]
fn another_hashed_key_never_verifies_as_the_sender() {
    for (crypto, scheme, code) in [
        (CRYPTO_HASHED, "hashed", 4u8),
        (CRYPTO_HASHED_ED25519, "hashed-ed25519", 6),
    ] {
        let victim = hashed(1, crypto);
        let attacker = hashed(2, crypto);
        let victim_account = Signer::account(&victim);
        let message = payload(scheme, &victim_account, &receiver());
        let forged = attacker.sign_message_at(2, &message).unwrap();
        let control = victim.sign_message_at(2, &message).unwrap();
        let headers_forged = headers(scheme, &victim_account, &receiver(), &forged);
        let headers_control = headers(scheme, &victim_account, &receiver(), &control);
        let verifier = verifier(WrappedPolicy::AcceptLikePython, &[(victim_account, code)]);
        assert!(accepted(&verifier, &headers_control), "control {scheme}");
        assert!(!accepted(&verifier, &headers_forged), "forged {scheme}");
    }
}

/// The trusted scheme is never swapped: an ED envelope does not verify as a
/// hashed-SR sender, and a declared scheme other than the trusted one fails.
#[test]
fn wrong_scheme_envelopes_never_verify() {
    let ed = hashed(3, CRYPTO_HASHED_ED25519);
    let sr = hashed(4, CRYPTO_HASHED);
    let ed_account = Signer::account(&ed);
    let sr_account = Signer::account(&sr);
    // The ED key's genuine envelope over a payload claiming the SR scheme.
    let as_sr = ed
        .sign_message_at(2, &payload("hashed", &ed_account, &receiver()))
        .unwrap();
    let misconfigured = verifier(WrappedPolicy::AcceptLikePython, &[(ed_account, 4)]);
    assert!(!accepted(
        &misconfigured,
        &headers("hashed", &ed_account, &receiver(), &as_sr)
    ));
    // A genuine SR envelope relabelled as ED, and as classical sr25519.
    let genuine = sr
        .sign_message_at(2, &payload("hashed", &sr_account, &receiver()))
        .unwrap();
    let trusted = verifier(WrappedPolicy::AcceptLikePython, &[(sr_account, 4)]);
    assert!(accepted(
        &trusted,
        &headers("hashed", &sr_account, &receiver(), &genuine)
    ));
    for scheme in ["hashed-ed25519", "sr25519", "ed25519"] {
        assert!(!accepted(
            &trusted,
            &headers(scheme, &sr_account, &receiver(), &genuine)
        ));
    }
    // A classical sr25519 signature over the payload never verifies as a
    // hashed account, even under a classical trust entry.
    let classical = Keypair::from_seed(&[4; 32], CRYPTO_SR25519).unwrap();
    let raw = classical
        .sign(&payload("sr25519", &sr_account, &receiver()))
        .unwrap();
    let classical_trust = verifier(WrappedPolicy::AcceptLikePython, &[(sr_account, 1)]);
    assert!(!accepted(
        &classical_trust,
        &headers("sr25519", &sr_account, &receiver(), &raw)
    ));
}

/// Every single-byte change, truncation or extension of a genuine envelope
/// fails, including the generation and descriptor fields it carries.
#[test]
fn tampered_envelopes_never_verify() {
    for (crypto, scheme, code) in [
        (CRYPTO_HASHED, "hashed", 4u8),
        (CRYPTO_HASHED_ED25519, "hashed-ed25519", 6),
    ] {
        let key = hashed(5, crypto);
        let account = Signer::account(&key);
        let message = payload(scheme, &account, &receiver());
        let genuine = key.sign_message_at(3, &message).unwrap();
        for policy in [WrappedPolicy::AcceptLikePython, WrappedPolicy::Strict] {
            let verifier = verifier(policy, &[(account, code)]);
            if policy == WrappedPolicy::AcceptLikePython {
                assert!(accepted(
                    &verifier,
                    &headers(scheme, &account, &receiver(), &genuine)
                ));
            }
            for index in 0..genuine.len() {
                for flip in [0x01u8, 0x80] {
                    let mut tampered = genuine.clone();
                    tampered[index] ^= flip;
                    assert!(
                        !accepted(
                            &verifier,
                            &headers(scheme, &account, &receiver(), &tampered)
                        ),
                        "{scheme}: byte {index} ^ {flip:#x} verified"
                    );
                }
            }
            for length in [0, 1, genuine.len() - 1] {
                assert!(!accepted(
                    &verifier,
                    &headers(scheme, &account, &receiver(), &genuine[..length])
                ));
            }
            let mut extended = genuine.clone();
            extended.push(0);
            assert!(!accepted(
                &verifier,
                &headers(scheme, &account, &receiver(), &extended)
            ));
        }
    }
}

/// A genuine envelope for one receiver, body or sender never verifies for
/// another.
#[test]
fn envelopes_are_bound_to_their_payload() {
    let key = hashed(6, CRYPTO_HASHED);
    let other = hashed(7, CRYPTO_HASHED);
    let account = Signer::account(&key);
    let other_account = AuthoritySigner::account(&other);
    let genuine = key
        .sign_message_at(2, &payload("hashed", &account, &receiver()))
        .unwrap();
    let verifier = verifier(
        WrappedPolicy::AcceptLikePython,
        &[(account, 4), (other_account, 4)],
    );
    // Replayed under another trusted hashed sender.
    assert!(!accepted(
        &verifier,
        &headers("hashed", &other_account, &receiver(), &genuine)
    ));
    // Replayed with another body.
    let headers = headers("hashed", &account, &receiver(), &genuine);
    assert!(
        verifier
            .authenticate(&headers, &Method::POST, TARGET, b"{}", NOW)
            .is_err()
    );
    assert!(
        verifier
            .authenticate(&headers, &Method::PUT, TARGET, BODY, NOW)
            .is_err()
    );
}
