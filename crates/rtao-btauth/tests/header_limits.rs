//! Authentication input limits at the native SS58 decoder boundary.

use std::{error::Error, sync::Arc};

use futures::executor::block_on;
use http::{HeaderMap, HeaderValue, Method, header::HeaderName};
use rtao_btauth::{
    AuthError, Clock, ClockError, HEADER_HOTKEY, HEADER_SIGNATURE, InMemoryNonceStore, Verifier,
    VerifyConfig, WrappedPolicy, build_payload, sign,
};
use rtao_primitives::{AccountId, CryptoType};
use rtao_signer::{KeypairSigner, Signer};

type TestResult = Result<(), Box<dyn Error>>;

struct FixedClock;
impl Clock for FixedClock {
    fn now_ns(&self) -> Result<u64, ClockError> {
        Ok(1)
    }
}

fn replace(headers: &mut HeaderMap, name: &str, value: &str) -> TestResult {
    headers.insert(
        HeaderName::from_bytes(name.as_bytes())?,
        HeaderValue::from_str(value)?,
    );
    Ok(())
}

#[test]
fn maximum_length_native_sender_addresses_verify_under_both_policies_and_algorithms() -> TestResult
{
    let receiver = AccountId::ZERO;
    for crypto in [CryptoType::Sr25519, CryptoType::Ed25519] {
        let sender = KeypairSigner::from_uri("//Alice", crypto)?;
        for prefix in [0, 42, 65] {
            //65 is a registered two-byte prefix (Aventus); custom-prefix
            // acceptance remains entirely with the pinned upstream decoder.
            let address = sender.account().to_ss58(prefix);
            assert!(address.len() <= 49);
            if prefix == 65 {
                assert_eq!(address.len(), 49);
            }
            assert_eq!(address.parse::<AccountId>()?, sender.account());
            let canonical = sender.account().to_string();
            let payload = String::from_utf8(build_payload(
                crypto,
                &Method::POST,
                "/",
                b"body",
                1,
                &sender.account(),
                Some(&receiver),
            ))?;
            let payload = payload.replace(&format!("\n{canonical}\n"), &format!("\n{address}\n"));
            let signature = block_on(sender.sign_raw(payload.as_bytes()))?;
            let mut headers = block_on(sign(
                &sender,
                &Method::POST,
                "/",
                b"body",
                Some(&receiver),
                &FixedClock,
            ))?;
            replace(&mut headers, HEADER_HOTKEY, &address)?;
            replace(&mut headers, HEADER_SIGNATURE, &hex::encode(signature))?;
            for wrapped in [WrappedPolicy::AcceptLikePython, WrappedPolicy::Strict] {
                let verifier = Verifier::new(
                    receiver,
                    VerifyConfig {
                        wrapped,
                        ..Default::default()
                    },
                    Arc::new(InMemoryNonceStore::new()),
                )?;
                assert_eq!(
                    verifier
                        .authenticate(&headers, &Method::POST, "/", b"body", 1)?
                        .caller()
                        .hotkey,
                    sender.account(),
                );
            }
        }
    }
    Ok(())
}

#[test]
fn oversized_sender_and_hex_key_claims_fail_without_changing_the_native_address_contract()
-> TestResult {
    let receiver = AccountId::ZERO;
    let sender = KeypairSigner::from_uri("//Alice", CryptoType::Sr25519)?;
    let original = block_on(sign(
        &sender,
        &Method::POST,
        "/",
        b"",
        Some(&receiver),
        &FixedClock,
    ))?;
    for wrapped in [WrappedPolicy::AcceptLikePython, WrappedPolicy::Strict] {
        let verifier = Verifier::new(
            receiver,
            VerifyConfig {
                wrapped,
                ..Default::default()
            },
            Arc::new(InMemoryNonceStore::new()),
        )?;
        for address in [
            "z".repeat(50),
            format!("0x{}", hex::encode(sender.account().0)),
        ] {
            let mut headers = original.clone();
            replace(&mut headers, HEADER_HOTKEY, &address)?;
            assert_eq!(
                verifier
                    .authenticate(&headers, &Method::POST, "/", b"", 1)
                    .err(),
                Some(AuthError::MalformedAuth(
                    "invalid hotkey address or signature encoding".into()
                )),
            );
        }
        let mut headers = original.clone();
        replace(&mut headers, HEADER_HOTKEY, &"z".repeat(100_000))?;
        assert_eq!(
            verifier
                .authenticate(&headers, &Method::POST, "/", b"", 1)
                .err(),
            Some(AuthError::MalformedAuth(
                "authentication fields exceed the 4096-byte field or 8192-byte total budget".into(),
            )),
        );
    }
    Ok(())
}

#[test]
#[cfg(feature = "legacy-v10")]
fn legacy_sender_limits_preserve_maximum_native_addresses_and_reject_huge_inputs() -> TestResult {
    let sender = KeypairSigner::from_uri("//Alice", CryptoType::Sr25519)?;
    let address = sender.account().to_ss58(65);
    assert_eq!(address.len(), 49);
    let receiver = AccountId::ZERO.to_string();
    let body_hash = hex::encode([0u8; 32]);
    let message = format!("1.{address}.{receiver}.uuid.{body_hash}");
    let signature = block_on(sender.sign_raw(message.as_bytes()))?;
    let mut headers = HeaderMap::new();
    for (name, value) in [
        ("bt_header_dendrite_hotkey", address),
        ("bt_header_dendrite_nonce", "1".into()),
        ("bt_header_axon_hotkey", receiver),
        ("bt_header_dendrite_uuid", "uuid".into()),
        ("bt_header_dendrite_signature", hex::encode(signature)),
    ] {
        replace(&mut headers, name, &value)?;
    }
    assert_eq!(
        rtao_btauth::legacy::verify_v10(&headers, &body_hash)?,
        sender.account()
    );
    replace(&mut headers, "bt_header_dendrite_hotkey", &"z".repeat(50))?;
    assert_eq!(
        rtao_btauth::legacy::verify_v10(&headers, &body_hash).err(),
        Some(AuthError::MalformedAuth(
            "invalid hotkey address or signature encoding".into()
        )),
    );
    replace(
        &mut headers,
        "bt_header_dendrite_hotkey",
        &"z".repeat(100_000),
    )?;
    assert_eq!(
        rtao_btauth::legacy::verify_v10(&headers, &body_hash).err(),
        Some(AuthError::MalformedAuth(
            "authentication fields exceed the 4096-byte field or 8192-byte total budget".into(),
        )),
    );
    Ok(())
}
