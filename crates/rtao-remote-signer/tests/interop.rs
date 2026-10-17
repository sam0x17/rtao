//! The Rust `RemoteSigner` client against the independent Python peer in
//! `ci/interop/remote-signer/py_server.py`.
//!
//! Ignored by default; `ci/interop/remote-signer/run.sh` starts the Python
//! peer and runs this target with `RTAO_REMOTE_SIGNER_PY_URL` and `--ignored`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::{sync::Arc, time::Duration};

use common::*;
use rtao_primitives::CryptoType;
use rtao_remote_signer::{RemoteSigner, RemoteSignerConfig, RemoteSignerError};
use rtao_signer::Signer;
use url::Url;

fn url() -> Url {
    Url::parse(
        &std::env::var("RTAO_REMOTE_SIGNER_PY_URL")
            .expect("RTAO_REMOTE_SIGNER_PY_URL is required; run ci/interop/remote-signer/run.sh"),
    )
    .unwrap()
}

fn config() -> RemoteSignerConfig {
    let mut cfg = RemoteSignerConfig::new(
        url(),
        alice().account(),
        CryptoType::Sr25519,
        Arc::new(charlie()),
    );
    cfg.timeout = Duration::from_secs(10);
    cfg
}

#[tokio::test]
#[ignore = "needs the Python peer; run ci/interop/remote-signer/run.sh"]
async fn rust_client_signs_through_the_python_peer() {
    let (signer, accounts) = RemoteSigner::connect(config()).await.unwrap();
    assert_eq!(accounts.server_key.0, alice().account());
    let vector = v1();
    for _ in 0..2 {
        // Every attempt carries a fresh btauth nonce; the peer's replay store accepts both.
        let signature = signer.sign(&vector.signing_request()).await.unwrap();
        assert!(verify(&vector.payload, &signature.bytes, alice().account()));
    }
    let long = with_call(remark(&[9; 300]));
    let signature = signer.sign(&long.signing_request()).await.unwrap();
    assert!(verify(&long.payload, &signature.bytes, alice().account()));
}

#[tokio::test]
#[ignore = "needs the Python peer; run ci/interop/remote-signer/run.sh"]
async fn python_peer_refusals_carry_upstream_btauth_messages() {
    let mut cfg = config();
    cfg.receiver = Some(bob().account());
    let error = RemoteSigner::new(cfg)
        .unwrap()
        .sign_request(&v1().signing_request())
        .await
        .unwrap_err();
    assert!(
        matches!(&error, RemoteSignerError::Http { status: 401, detail, .. } if detail == "request was signed for a different receiving hotkey"),
        "{error:?}"
    );
    let mut cfg = config();
    cfg.client_key = Arc::new(dave());
    let error = RemoteSigner::new(cfg)
        .unwrap()
        .sign_request(&v1().signing_request())
        .await
        .unwrap_err();
    assert!(
        matches!(&error, RemoteSignerError::Http { status: 403, detail, .. } if detail == "client key is not authorized"),
        "{error:?}"
    );
    let mut tampered = v1();
    tampered.payload[0] ^= 1;
    let (signer, _) = RemoteSigner::connect(config()).await.unwrap();
    let error = signer
        .sign_request(&tampered.signing_request())
        .await
        .unwrap_err();
    assert!(
        matches!(&error, RemoteSignerError::Http { status: 422, code: Some(code), .. } if code == "tampered_request"),
        "{error:?}"
    );
}
