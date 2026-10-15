#![no_main]

use http::{HeaderMap, HeaderValue, Method, header::HeaderName};
use libfuzzer_sys::fuzz_target;
use rtao_btauth::{
    HEADER_CRYPTO, HEADER_HOTKEY, HEADER_NONCE, HEADER_RECEIVER, HEADER_SIGNATURE, HEADER_VERSION,
    InMemoryNonceStore, Verifier, VerifyConfig,
};
use rtao_primitives::AccountId;
use std::sync::{Arc, OnceLock};

fuzz_target!(|bytes: &[u8]| {
    static VERIFIER: OnceLock<Verifier<InMemoryNonceStore>> = OnceLock::new();
    let verifier = VERIFIER.get_or_init(|| {
        Verifier::new(
            AccountId([0; 32]),
            VerifyConfig {
                require_receiver: false,
                ..VerifyConfig::default()
            },
            Arc::new(InMemoryNonceStore::new()),
        )
        .expect("default verifier")
    });
    let mut headers = HeaderMap::new();
    let mut remaining = bytes;
    // A little-endian u16 length before each field permits large nonce/SS58
    // and signature inputs as well as missing, empty and invalid UTF-8 fields.
    for name in [
        HEADER_VERSION,
        HEADER_HOTKEY,
        HEADER_NONCE,
        HEADER_CRYPTO,
        HEADER_RECEIVER,
        HEADER_SIGNATURE,
    ] {
        if remaining.len() < 2 {
            break;
        }
        let length = usize::from(u16::from_le_bytes([remaining[0], remaining[1]]));
        remaining = &remaining[2..];
        let length = length.min(remaining.len());
        if let Ok(value) = HeaderValue::from_bytes(&remaining[..length]) {
            headers.insert(
                HeaderName::from_bytes(name.as_bytes()).expect("static header"),
                value,
            );
        }
        remaining = &remaining[length..];
    }
    let _ = verifier.authenticate(
        &headers,
        &Method::POST,
        "/generate?q=a%20b",
        remaining,
        1_752_076_800_000_000_000,
    );
});
