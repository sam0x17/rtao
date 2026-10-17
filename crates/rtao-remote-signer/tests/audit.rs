//! Review finding 8: the audit event carries every field already parsed, also
//! on refusal. A separate target, so no other test thread races the
//! subscriber's callsite interest.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
};

use axum::body::Body;
use common::*;
use http::{Method, Request, StatusCode, header};
use rtao_btauth::InMemoryNonceStore;
use rtao_primitives::{Balance, Tao};
use rtao_remote_signer::server::{
    AuthorizedClient, PinnedRuntimeSource, RemoteSignerServer, ServedAccount, ServerConfig,
};
use rtao_signer::Signer;
use tower::ServiceExt;

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Capture {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn refusals_are_audited_with_the_parsed_fields() {
    let capture = Capture::default();
    let writer = capture.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let mut cfg = ServerConfig::new(genesis());
    cfg.accounts = vec![ServedAccount::new(Arc::new(alice()))];
    cfg.clients = vec![AuthorizedClient {
        client: charlie().account(),
        accounts: BTreeSet::from([alice().account()]),
    }];
    cfg.policy.limits.max_spend = Some(Balance::<Tao>::from_rao(1));
    let router = RemoteSignerServer::new(
        cfg,
        Arc::new(PinnedRuntimeSource::new(runtime(), SPEC_NAME)),
        Arc::new(InMemoryNonceStore::new()),
    )
    .unwrap()
    .with_clock(Arc::new(FixedClock(NOW + 1_000_000_000)))
    .router();

    let clock = StepClock::at(NOW);
    let post = |body: Vec<u8>| {
        let router = router.clone();
        let clock = clock.clone();
        async move {
            let headers = rtao_btauth::sign(
                &charlie(),
                &Method::POST,
                "/sign",
                &body,
                Some(&alice().account()),
                clock.as_ref(),
            )
            .await
            .unwrap();
            let mut request = Request::builder().method(Method::POST).uri("/sign");
            for (name, value) in &headers {
                request = request.header(name, value);
            }
            let request = request
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body))
                .unwrap();
            router.oneshot(request).await.unwrap().status()
        }
    };
    let log = || String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();
    let audit_line = |log: &str, result: &str| {
        log.lines()
            .find(|line| {
                line.contains("remote signing request")
                    && line.contains(&format!("result=\"{result}\""))
            })
            .unwrap_or_else(|| panic!("no {result} audit line in {log}"))
            .to_owned()
    };

    // A policy refusal logs everything parsed before the policy check.
    assert_eq!(post(v1().body_bytes()).await, StatusCode::FORBIDDEN);
    let line = audit_line(&log(), "policy_denied");
    let payload_hash = hex::encode(rtao_remote_signer::wire::blake2_256(&v1().payload));
    for field in [
        format!("client={}", charlie().account()),
        format!("account={}", alice().account()),
        "purpose=\"extrinsic\"".to_owned(),
        "call=Balances.transfer_keep_alive".to_owned(),
        "nonce=3".to_owned(),
        "era=mortal(64/2)".to_owned(),
        format!("payload_blake2_256={payload_hash}"),
    ] {
        assert!(line.contains(&field), "{field} missing from {line}");
    }

    // An early refusal logs the known fields and marks the rest unknown.
    let mut tampered = v1().body();
    tampered.payload.0[40] ^= 1;
    let status = post(serde_json::to_vec(&tampered).unwrap()).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let line = audit_line(&log(), "tampered_request");
    assert!(
        line.contains(&format!("account={}", alice().account())),
        "{line}"
    );
    for unknown in ["call=-", "nonce=-", "era=-", "payload_blake2_256=-"] {
        assert!(line.contains(unknown), "{unknown} missing from {line}");
    }
}
