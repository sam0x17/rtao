// Derived from RaoFoundation/subtensor sdk/python/tests/unit/test_normalize_signature.py @ d967d13588834f4f5aaa8ef4c8fc1a865d581462 (Apache-2.0); modified by rtao.
//! External MultiSignature replies keep their scheme and complete signature (D-180).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use blake2::{Blake2b, Digest, digest::consts::U32};
use k256::ecdsa::SigningKey;
use rtao_primitives::{AccountId, SignatureScheme};
use rtao_signer::{EcdsaSignature, ExternalSignature, KeypairSigner, Signer, SignerError};

const SCHEMES: [SignatureScheme; 3] = [
    SignatureScheme::Ed25519,
    SignatureScheme::Sr25519,
    SignatureScheme::Ecdsa,
];

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

// pyt: unit/test_normalize_signature.py::test_raw_and_prefixed_signature_forms
#[test]
fn raw_and_prefixed_forms_keep_the_scheme_and_complete_signature() {
    for scheme in SCHEMES {
        let raw = vec![0x7f; scheme.signature_len()];
        assert_eq!(
            scheme.signature_len(),
            if scheme == SignatureScheme::Ecdsa {
                65
            } else {
                64
            }
        );
        for prefixed in [false, true] {
            let mut signature = if prefixed {
                vec![scheme.code()]
            } else {
                vec![]
            };
            signature.extend_from_slice(&raw);
            for normalized in [
                ExternalSignature::normalize(&signature, scheme).unwrap(),
                ExternalSignature::normalize_hex(&hex(&signature), scheme).unwrap(),
                ExternalSignature::normalize_hex(&format!("0x{}", hex(&signature)), scheme)
                    .unwrap(),
            ] {
                assert_eq!(normalized.scheme(), scheme);
                assert_eq!(normalized.raw(), raw.as_slice());
                let mut encoded = vec![scheme.code()];
                encoded.extend_from_slice(&raw);
                assert_eq!(normalized.to_bytes(), encoded);
            }
        }
    }
}

// pyt: unit/test_normalize_signature.py::test_raw_ecdsa_leading_byte_is_not_a_variant
#[test]
fn a_raw_ecdsa_signature_s_first_byte_is_not_a_variant() {
    for first in [0u8, 1, 2, 255] {
        let mut raw = vec![first];
        raw.extend(0..64u8);
        let normalized = ExternalSignature::normalize(&raw, SignatureScheme::Ecdsa).unwrap();
        assert_eq!(
            normalized,
            ExternalSignature::Ecdsa(EcdsaSignature(raw.try_into().unwrap()))
        );
    }
}

// pyt: unit/test_normalize_signature.py::test_classical_signature_prefix_overrides_default
#[test]
fn a_variant_byte_for_another_scheme_is_refused() {
    // Python lets a classical variant byte override the prepared scheme;
    // rtao keeps MultiSignature::from_bytes_for's rule and refuses it.
    for (variant, expected) in [
        (SignatureScheme::Ed25519, SignatureScheme::Sr25519),
        (SignatureScheme::Sr25519, SignatureScheme::Ed25519),
    ] {
        let mut signature = vec![variant.code()];
        signature.extend_from_slice(&[0x7f; 64]);
        assert!(matches!(
            ExternalSignature::normalize(&signature, expected),
            Err(SignerError::InvalidSignature(message)) if message.contains("does not match")
        ));
    }
}

// pyt: unit/test_normalize_signature.py::test_unrecognized_signature_shape_is_not_reinterpreted
#[test]
fn unrecognized_shapes_are_refused_not_reinterpreted() {
    // Python passes these through for the chain to reject; rtao refuses them.
    for (signature, scheme) in [
        (vec![], SignatureScheme::Sr25519),
        (vec![1; 63], SignatureScheme::Sr25519),
        (vec![0xff; 65], SignatureScheme::Sr25519),
        (vec![0; 66], SignatureScheme::Ecdsa),
    ] {
        assert!(matches!(
            ExternalSignature::normalize(&signature, scheme),
            Err(SignerError::InvalidSignature(_))
        ));
    }
    assert!(ExternalSignature::normalize_hex("0xzz", SignatureScheme::Sr25519).is_err());
    assert!(ExternalSignature::normalize_hex("abc", SignatureScheme::Sr25519).is_err());
}

fn ecdsa_account(key: &SigningKey) -> AccountId {
    let compressed = key.verifying_key().to_encoded_point(true);
    AccountId(Blake2b::<U32>::digest(compressed.as_bytes()).into())
}

fn ecdsa_sign(key: &SigningKey, payload: &[u8]) -> [u8; 65] {
    let digest: [u8; 32] = Blake2b::<U32>::digest(payload).into();
    let (signature, id) = key.sign_prehash_recoverable(&digest).unwrap();
    let mut raw = [0; 65];
    raw[..64].copy_from_slice(&signature.to_bytes());
    raw[64] = id.to_byte();
    raw
}

#[test]
fn ecdsa_signatures_recover_the_signing_account_as_the_runtime_does() {
    let key = SigningKey::from_slice(&[1; 32]).unwrap();
    let account = ecdsa_account(&key);
    let payload = b"prepared payload";
    let raw = ecdsa_sign(&key, payload);
    let signature = ExternalSignature::Ecdsa(EcdsaSignature(raw));
    assert!(signature.verify(payload, &account).unwrap());
    assert!(!signature.verify(b"another payload", &account).unwrap());
    assert!(!signature.verify(payload, &AccountId([7; 32])).unwrap());
    // `v` may be 27/28 (Ethereum style); the runtime takes it less 27.
    let mut legacy = raw;
    legacy[64] += 27;
    assert_eq!(
        EcdsaSignature(legacy).recover_account(payload),
        Some(account)
    );
    // The runtime recovers a high-s signature too, to the same key.
    let mut high = raw;
    let signature = k256::ecdsa::Signature::from_slice(&raw[..64]).unwrap();
    let (r, s) = signature.split_scalars();
    let flipped = k256::ecdsa::Signature::from_scalars(r, -*s).unwrap();
    high[..64].copy_from_slice(&flipped.to_bytes());
    high[64] ^= 1;
    assert_eq!(EcdsaSignature(high).recover_account(payload), Some(account));
    // A recovery ID above 3 or zero scalars recover nothing.
    let mut bad = raw;
    bad[64] = 4;
    assert_eq!(EcdsaSignature(bad).recover_account(payload), None);
    assert_eq!(EcdsaSignature([0; 65]).recover_account(payload), None);
}

#[tokio::test]
async fn classical_signatures_verify_through_the_core() {
    let signer = KeypairSigner::from_seed(&[7; 32], rtao_primitives::CryptoType::Sr25519).unwrap();
    let payload = b"prepared payload";
    let raw = signer.sign_raw(payload).await.unwrap();
    let signature = ExternalSignature::normalize(&raw, SignatureScheme::Sr25519).unwrap();
    assert!(signature.verify(payload, &signer.account()).unwrap());
    assert!(!signature.verify(b"changed", &signer.account()).unwrap());
}
