#![no_main]

use axum::{body::Body, response::IntoResponse};
use futures::executor::block_on;
use http::{HeaderMap, HeaderValue, Method, Request, StatusCode};
use libfuzzer_sys::fuzz_target;
use rtao_btauth::{
    BoxFuture, Caller, Clock, ClockError, HEADER_CRYPTO, HEADER_HOTKEY, HEADER_NONCE,
    HEADER_RECEIVER, HEADER_SIGNATURE, HEADER_VERSION, InMemoryNonceStore, NonceStore, Verifier,
    VerifyConfig,
    server::{Admission, BtAuthLayer, CallerPolicy, LimiterConfig, PerHotkeyLimiter, PolicyDenied},
    sign,
};
use rtao_primitives::{AccountId, CryptoType};
use rtao_signer::{KeypairSigner, Signer};
use std::{
    sync::{Arc, OnceLock},
    time::Duration,
};
use tower::{Layer, ServiceExt};

const NOW: u64 = 1_752_076_800_000_000_000;
const TARGET: &str = "/generate%2Fprompt?text=a%20b";
const BODY_LIMIT: usize = 65_536;
struct FixedClock(u64);
impl Clock for FixedClock {
    fn now_ns(&self) -> Result<u64, ClockError> {
        Ok(self.0)
    }
}

struct Policy(bool);
impl CallerPolicy for Policy {
    fn admit<'a>(&'a self, _: &'a Caller) -> BoxFuture<'a, Result<Admission, PolicyDenied>> {
        Box::pin(async move {
            if self.0 {
                Ok(Admission::default())
            } else {
                Err(PolicyDenied("denied".into()))
            }
        })
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
        let mut headers = Vec::new();
        for crypto in [CryptoType::Sr25519, CryptoType::Ed25519] {
            let signer = KeypairSigner::from_uri("//Alice", crypto).expect("Alice");
            headers.push(
                block_on(sign(
                    &signer,
                    &Method::POST,
                    TARGET,
                    b"body",
                    Some(&receiver),
                    &FixedClock(NOW),
                ))
                .expect("seed signature"),
            );
        }
        Seed { receiver, headers }
    });
    let selector = bytes.first().copied().unwrap_or(0);
    let mutation = bytes.get(1..).unwrap_or_default();
    let operation = usize::from((selector >> 1) % 12);
    let mut headers = seed.headers[usize::from(selector & 1)].clone();
    let mut body = b"body".as_slice();
    let mut target = TARGET;
    match operation {
        0..=5 => {
            let name = [
                HEADER_VERSION,
                HEADER_HOTKEY,
                HEADER_NONCE,
                HEADER_CRYPTO,
                HEADER_RECEIVER,
                HEADER_SIGNATURE,
            ][operation];
            if let Ok(value) = HeaderValue::from_bytes(mutation) {
                headers.insert(name, value);
            } else {
                headers.remove(name);
            }
        }
        6 => body = mutation,
        7 => {
            if let Ok(text) = std::str::from_utf8(mutation) {
                target = text;
            }
        }
        _ => {}
    }
    let uri = match target.parse::<http::Uri>() {
        Ok(uri) => uri,
        Err(_) => return,
    };
    let store: Arc<dyn NonceStore> = Arc::new(InMemoryNonceStore::new());
    let verifier =
        Arc::new(Verifier::new(seed.receiver, VerifyConfig::default(), store).expect("verifier"));
    let limiter = Arc::new(
        PerHotkeyLimiter::new(LimiterConfig {
            burst: 1,
            refill_interval: Duration::from_secs(3600),
            max_hotkeys: 1,
        })
        .expect("limiter"),
    );
    let layer = BtAuthLayer::new(verifier)
        .with_clock(Arc::new(FixedClock(NOW)))
        .with_policy(Arc::new(Policy(operation != 10)))
        .with_limiter(limiter)
        .with_body_limit(BODY_LIMIT);
    let service = layer.layer(tower::service_fn(|req: Request<Body>| async move {
        assert!(req.extensions().get::<Caller>().is_some());
        Ok::<_, std::convert::Infallible>(StatusCode::OK.into_response())
    }));
    let request = || {
        let mut request = Request::new(Body::from(body.to_vec()));
        *request.method_mut() = Method::POST;
        *request.uri_mut() = uri.clone();
        *request.headers_mut() = headers.clone();
        if operation == 11 {
            request.headers_mut().insert(
                http::header::CONTENT_LENGTH,
                HeaderValue::from_static("65537"),
            );
        }
        request
    };
    let response = block_on(service.clone().oneshot(request())).expect("infallible service");
    if operation == 8 || operation == 9 {
        assert_eq!(response.status(), StatusCode::OK);
    }
    if operation == 10 {
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
    if operation == 11 || body.len() > BODY_LIMIT {
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }
    if response.status() == StatusCode::OK {
        assert_eq!(
            block_on(service.oneshot(request()))
                .expect("infallible replay service")
                .status(),
            StatusCode::UNAUTHORIZED
        );
    }
});
