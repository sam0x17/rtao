#![no_main]

use futures::executor::block_on;
use http::{HeaderMap, HeaderValue, Method};
use libfuzzer_sys::fuzz_target;
use rtao_btauth::{
    AuthError, Clock, ClockError, HEADER_CRYPTO, HEADER_HOTKEY, HEADER_NONCE, HEADER_RECEIVER,
    HEADER_SIGNATURE, HEADER_VERSION, InMemoryNonceStore, Verifier, VerifyConfig, WrappedPolicy,
    build_payload, sign,
};
use rtao_primitives::{AccountId, CryptoType};
use rtao_signer::{KeypairSigner, Signer};
use std::sync::{Arc, OnceLock};

const NOW: u64 = 1_752_076_800_000_000_000;
struct FixedClock;
impl Clock for FixedClock {
    fn now_ns(&self) -> Result<u64, ClockError> {
        Ok(NOW)
    }
}
struct Seed {
    receiver: AccountId,
    headers: Vec<HeaderMap>,
}

fuzz_target!(|bytes: &[u8]| {
    static SEED: OnceLock<Seed> = OnceLock::new();
    let seed = SEED.get_or_init(|| {
        let receiver = KeypairSigner::from_uri("//Bob", CryptoType::Sr25519)
            .expect("Bob")
            .account();
        let mut raw = Vec::new();
        let mut wrapped = Vec::new();
        for crypto in [CryptoType::Sr25519, CryptoType::Ed25519] {
            let signer = KeypairSigner::from_uri("//Alice", crypto).expect("Alice");
            let headers = block_on(sign(
                &signer,
                &Method::POST,
                "/generate?q=a%20b",
                b"body",
                Some(&receiver),
                &FixedClock,
            ))
            .expect("seed signature");
            let payload = build_payload(
                crypto,
                &Method::POST,
                "/generate?q=a%20b",
                b"body",
                NOW,
                &signer.account(),
                Some(&receiver),
            );
            let mut wrapped_payload = b"<Bytes>".to_vec();
            wrapped_payload.extend_from_slice(&payload);
            wrapped_payload.extend_from_slice(b"</Bytes>");
            let signature = block_on(signer.sign_raw(&wrapped_payload)).expect("wrapped signature");
            let mut wrapped_headers = headers.clone();
            wrapped_headers.insert(
                HEADER_SIGNATURE,
                HeaderValue::from_str(&format!("0x{}", hex::encode(signature)))
                    .expect("signature header"),
            );
            raw.push(headers);
            wrapped.push(wrapped_headers);
        }
        let headers = raw.into_iter().chain(wrapped).collect();
        Seed { receiver, headers }
    });
    let selector = bytes.first().copied().unwrap_or(0);
    let mutation = bytes.get(1..).unwrap_or_default();
    let wrapped_signature = selector & 128 != 0;
    let mut headers =
        seed.headers[usize::from(selector & 1) + usize::from(wrapped_signature) * 2].clone();
    let wrapped = if selector & 2 == 0 {
        WrappedPolicy::AcceptLikePython
    } else {
        WrappedPolicy::Strict
    };
    let mut body = b"body".as_slice();
    let mut target = "/generate?q=a%20b";
    let mut now = NOW;
    let operation = ((selector >> 2) & 31) % 10;
    match operation {
        0..=5 => {
            let field = [
                HEADER_VERSION,
                HEADER_HOTKEY,
                HEADER_NONCE,
                HEADER_CRYPTO,
                HEADER_RECEIVER,
                HEADER_SIGNATURE,
            ][usize::from(operation)];
            if let Ok(value) = HeaderValue::from_bytes(mutation) {
                headers.insert(field, value);
            } else {
                headers.remove(field);
            }
        }
        6 => body = mutation,
        7 => {
            if let Ok(text) = std::str::from_utf8(mutation) {
                target = text;
            }
        }
        8 => {
            let mut encoded = [0; 8];
            let length = mutation.len().min(8);
            encoded[..length].copy_from_slice(&mutation[..length]);
            now = u64::from_le_bytes(encoded);
        }
        _ => {}
    }
    let verifier = Verifier::new(
        seed.receiver,
        VerifyConfig {
            wrapped,
            ..VerifyConfig::default()
        },
        Arc::new(InMemoryNonceStore::new()),
    )
    .expect("verifier");
    let result = block_on(verifier.verify(&headers, &Method::POST, target, body, now));
    if operation == 9 {
        if wrapped_signature && wrapped == WrappedPolicy::Strict {
            assert!(matches!(result, Err(AuthError::BadSignature(_))));
        } else {
            assert!(result.is_ok());
        }
    }
    if result.is_ok() {
        assert!(matches!(
            block_on(verifier.verify(&headers, &Method::POST, target, body, now)),
            Err(AuthError::ReplayedRequest(_))
        ));
    }
});
