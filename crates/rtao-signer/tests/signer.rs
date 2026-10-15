//! Cryptographic evidence for the portable signer contract.
#![allow(clippy::expect_used)]

use futures::executor::block_on;
use rtao_core::keys::{self, Keypair};
use rtao_signer::{
    AccountId, BoxFuture, CryptoType, KeypairSigner, MultiSignature, SignPurpose, Signer,
    SignerCaps, SignerError, SignerPayloadJson, SigningContext, SigningRequest,
};

fn context() -> SigningContext {
    SigningContext {
        metadata_bytes: vec![0x6d, 0x65, 0x74, 0x61],
        chain_info: rtao_core::digest::ChainInfo {
            spec_version: 478,
            spec_name: "node-subtensor".into(),
            base58_prefix: 42,
            decimals: 9,
            token_symbol: "TAO".into(),
        },
        transaction_version: 1,
        genesis_hash: [7; 32],
    }
}

#[test]
fn raw_signatures_use_exact_bytes_and_declared_scheme() {
    for crypto in [CryptoType::Ed25519, CryptoType::Sr25519] {
        let signer = KeypairSigner::from_uri("//Alice", crypto).expect("dev URI");
        let signer: &dyn Signer = &signer;
        let message = b"exact message";
        let signature = block_on(signer.sign_raw(message)).expect("raw signature");
        assert_eq!(signer.crypto_type(), crypto);
        assert_eq!(signer.caps(), SignerCaps::RAW_PAYLOAD);
        assert!(
            keys::verify_strict(
                message,
                &signature,
                &signer.account().to_string(),
                u8::from(crypto)
            )
            .expect("strict verification")
        );
        assert!(
            !keys::verify_strict(
                b"modified message",
                &signature,
                &signer.account().to_string(),
                u8::from(crypto)
            )
            .expect("strict verification")
        );
        assert!(
            !keys::verify_strict(
                &[b"<Bytes>".as_slice(), message, b"</Bytes>"].concat(),
                &signature,
                &signer.account().to_string(),
                u8::from(crypto)
            )
            .expect("strict verification")
        );
        let other = if crypto == CryptoType::Ed25519 {
            CryptoType::Sr25519
        } else {
            CryptoType::Ed25519
        };
        assert!(
            !keys::verify_strict(
                message,
                &signature,
                &signer.account().to_string(),
                u8::from(other)
            )
            .expect("strict verification")
        );
    }
}

#[test]
fn prepared_signing_uses_payload_without_reassembling_or_hashing() {
    for crypto in [CryptoType::Ed25519, CryptoType::Sr25519] {
        let signer = KeypairSigner::from_uri("//Alice", crypto).expect("dev URI");
        let context = context();
        let json = SignerPayloadJson::new();
        for payload in [vec![1, 2, 3], vec![0x42; 300]] {
            let request = SigningRequest {
                call: b"different-call",
                included_in_extrinsic: b"extra",
                included_in_signed_data: b"additional",
                payload: &payload,
                payload_json: &json,
                context: &context,
                purpose: SignPurpose::Extrinsic,
            };
            let signature =
                block_on((&signer as &dyn Signer).sign(&request)).expect("prepared signature");
            assert_eq!(signature.crypto, crypto);
            assert!(
                keys::verify_strict(
                    &payload,
                    &signature.bytes,
                    &signer.account().to_string(),
                    u8::from(crypto)
                )
                .expect("strict verification")
            );
            assert!(
                !keys::verify_strict(
                    b"different-callextraadditional",
                    &signature.bytes,
                    &signer.account().to_string(),
                    u8::from(crypto)
                )
                .expect("strict verification")
            );
        }
    }
}

#[test]
fn constructors_delegate_derivation_and_public_only_errors() {
    let mnemonic = "bottom drive obey lake curtain smoke basket hold race lonely fit walk";
    for crypto in [CryptoType::Ed25519, CryptoType::Sr25519] {
        let seed = [0x31; 32];
        let signer = KeypairSigner::from_seed(&seed, crypto).expect("seed signer");
        let upstream = Keypair::from_seed(&seed, u8::from(crypto)).expect("upstream seed");
        assert_eq!(signer.account(), AccountId(upstream.public_key_bytes()));
        let signer = KeypairSigner::from_mnemonic(mnemonic, crypto, Some("secret password"))
            .expect("mnemonic signer");
        let upstream = Keypair::from_mnemonic(mnemonic, u8::from(crypto), Some("secret password"))
            .expect("upstream mnemonic");
        assert_eq!(signer.account(), AccountId(upstream.public_key_bytes()));
        let public = Keypair::new(None, Some(signer.account().as_ref()), u8::from(crypto), 42)
            .expect("public-only keypair");
        let public_signer = KeypairSigner::new(public).expect("public signer");
        assert_eq!(public_signer.account(), signer.account());
        assert!(matches!(
            block_on(public_signer.sign_raw(b"message")),
            Err(SignerError::Core(_))
        ));
    }
    assert!(KeypairSigner::from_seed(&[1; 3], CryptoType::Sr25519).is_err());
    assert!(KeypairSigner::from_mnemonic("invalid mnemonic", CryptoType::Sr25519, None).is_err());
    assert!(KeypairSigner::from_uri("invalid secret URI", CryptoType::Sr25519).is_err());
}

fn assert_secret_error_is_redacted(error: &dyn std::error::Error, secrets: &[&str]) {
    let mut current = Some(error);
    while let Some(error) = current {
        for rendered in [error.to_string(), format!("{error:?}")] {
            for secret in secrets {
                assert!(
                    !rendered.contains(secret),
                    "constructor error retained public test sentinel"
                );
            }
        }
        current = error.source();
    }
}

#[test]
fn constructor_error_chains_redact_uri_phrase_password_and_seed_sentinels() {
    // These are public disposable fixtures, never recovery material from a wallet.
    let phrase = "bottom drive obey lake curtain smoke basket hold race lonely fit walk";
    let password = "rtao-public-disposable-password-sentinel";
    let malformed_uri = format!("{phrase}///{password}\n");
    let malformed_mnemonic = "rtao-public-disposable-mnemonic-sentinel";
    let seed = b"rtao-public-disposable-invalid-seed-sentinel";
    for crypto in [CryptoType::Ed25519, CryptoType::Sr25519] {
        // Negative control: the pinned URI error contains both parts of this fixture.
        let upstream = Keypair::from_uri(&malformed_uri, u8::from(crypto))
            .err()
            .expect("invalid URI must fail upstream");
        assert!(upstream.to_string().contains(phrase));
        assert!(upstream.to_string().contains(password));
        let uri = KeypairSigner::from_uri(&malformed_uri, crypto)
            .expect_err("invalid URI must fail at the signer");
        assert_eq!(
            uri.to_string(),
            "crypto error: could not derive keypair from secret URI"
        );
        assert_secret_error_is_redacted(&uri, &[phrase, password, &malformed_uri]);
        let mnemonic = KeypairSigner::from_mnemonic(malformed_mnemonic, crypto, Some(password))
            .expect_err("invalid mnemonic must fail");
        assert_eq!(
            mnemonic.to_string(),
            "crypto error: could not derive keypair from mnemonic"
        );
        assert_secret_error_is_redacted(&mnemonic, &[malformed_mnemonic, password]);
        let seed =
            KeypairSigner::from_seed(seed, crypto).expect_err("invalid seed length must fail");
        assert_eq!(
            seed.to_string(),
            "crypto error: could not derive keypair from seed"
        );
        assert_secret_error_is_redacted(&seed, &["rtao-public-disposable-invalid-seed-sentinel"]);
    }
}

#[test]
fn valid_password_uri_derivation_remains_identical_to_the_pinned_core() {
    let phrase = "bottom drive obey lake curtain smoke basket hold race lonely fit walk";
    let uri = format!("{phrase}//42///rtao-public-disposable-derivation-password");
    for crypto in [CryptoType::Ed25519, CryptoType::Sr25519] {
        let upstream =
            Keypair::from_uri(&uri, u8::from(crypto)).expect("public derivation fixture");
        let signer =
            KeypairSigner::from_uri(&uri, crypto).expect("redacted constructor valid path");
        assert_eq!(signer.account().as_bytes(), &upstream.public_key_bytes());
        let signature =
            block_on(signer.sign_raw(b"valid derivation parity")).expect("sign fixture");
        assert!(
            upstream
                .verify(b"valid derivation parity", &signature)
                .expect("verify fixture")
        );
    }
}

#[test]
fn debug_redacts_secret_sources_and_owned_keypair() {
    let mnemonic = "bottom drive obey lake curtain smoke basket hold race lonely fit walk";
    let signers = [
        KeypairSigner::from_uri("//Alice///top-secret-password", CryptoType::Sr25519)
            .expect("URI signer"),
        KeypairSigner::from_mnemonic(mnemonic, CryptoType::Ed25519, None).expect("mnemonic signer"),
        KeypairSigner::from_seed(&[0x31; 32], CryptoType::Sr25519).expect("seed signer"),
    ];
    for signer in signers {
        let rendered = format!("{signer:?}");
        assert!(rendered.contains("[REDACTED]"));
        for secret in [
            "Alice",
            "top-secret-password",
            mnemonic,
            "3131313131313131",
            "49, 49, 49, 49",
        ] {
            assert!(!rendered.contains(secret));
        }
    }
}

#[test]
fn multisignature_encoding_rejects_length_variant_and_algorithm_mismatch() {
    for crypto in [CryptoType::Ed25519, CryptoType::Sr25519] {
        let signature = MultiSignature {
            crypto,
            bytes: [0x23; 64],
        };
        let encoded = signature.to_bytes();
        assert_eq!(encoded[0], u8::from(crypto));
        assert_eq!(&encoded[1..], &signature.bytes);
        assert_eq!(
            MultiSignature::from_bytes(&encoded).expect("valid signature"),
            signature
        );
        assert_eq!(
            MultiSignature::from_bytes_for(&encoded, crypto).expect("matching algorithm"),
            signature
        );
        let other = if crypto == CryptoType::Ed25519 {
            CryptoType::Sr25519
        } else {
            CryptoType::Ed25519
        };
        assert!(MultiSignature::from_bytes_for(&encoded, other).is_err());
    }
    assert!(MultiSignature::from_bytes(&[]).is_err());
    assert!(MultiSignature::from_bytes(&[0; 64]).is_err());
    assert!(MultiSignature::from_bytes(&[0; 66]).is_err());
    assert!(MultiSignature::from_bytes(&[2; 65]).is_err());
}

struct ContextOnlySigner;
impl Signer for ContextOnlySigner {
    fn account(&self) -> AccountId {
        AccountId([0; 32])
    }
    fn crypto_type(&self) -> CryptoType {
        CryptoType::Ed25519
    }
    fn sign<'a>(
        &'a self,
        _request: &'a SigningRequest<'a>,
    ) -> BoxFuture<'a, Result<MultiSignature, SignerError>> {
        Box::pin(async { Err(SignerError::RawSigningUnsupported) })
    }
}

#[test]
fn context_only_signers_refuse_raw_messages_by_default() {
    let signer: &dyn Signer = &ContextOnlySigner;
    assert_eq!(
        block_on(signer.sign_raw(b"blind message")),
        Err(SignerError::RawSigningUnsupported)
    );
}
