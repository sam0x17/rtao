//! PLAN M10-04 exit check: `RemoteSigner` signs a localnet transfer through the
//! reference server, which refuses a replayed or tampered request.
//!
//! Ignored by default. `ci/interop/remote-signer/localnet.sh` starts and
//! bootstraps a disposable localnet, then runs this target with
//! `E2E_ENDPOINT` and `--ignored`. Accidental local runs fail before any I/O.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::{collections::BTreeSet, error::Error, sync::Arc, time::Duration};

use http::{Method, StatusCode};
use rtao_btauth::{InMemoryNonceStore, SystemClock};
use rtao_client::{Client, ClientConfig, PublicPrepareOptions, SubmitOptions, View, ViewExt, Wait};
use rtao_core::{codec::Value, value_ext::ValueExt};
use rtao_primitives::{AccountId, Balance, CryptoType, Tao, TaoBalance};
use rtao_remote_signer::{
    RemoteSigner, RemoteSignerConfig, RemoteSignerError,
    server::{
        AuthorizedClient, ClientRuntimeSource, RemoteSignerServer, ServedAccount, ServerConfig,
    },
    wire::{SignRequest, SignResponse},
};
use rtao_runtime::{calls::balances, storage::system};
use rtao_signer::{KeypairSigner, SignPurpose, Signer};
use url::Url;

type TestResult = Result<(), Box<dyn Error>>;

fn key(uri: &str) -> Result<KeypairSigner, Box<dyn Error>> {
    Ok(KeypairSigner::from_uri(uri, CryptoType::Sr25519)?)
}

fn options() -> SubmitOptions {
    SubmitOptions {
        wait: Wait::Inclusion,
        deadline: Some(Duration::from_secs(240)),
        ..Default::default()
    }
}

async fn free(client: &Client, account: AccountId) -> Result<u128, Box<dyn Error>> {
    let value = client.storage(&system::ACCOUNT, (account,)).await?;
    value
        .as_core()
        .field("data")
        .and_then(|data| data.field("free"))
        .and_then(ValueExt::as_u128)
        .ok_or_else(|| "System.Account.data.free missing".into())
}

async fn next_nonce(client: &Client, account: AccountId) -> Result<u64, Box<dyn Error>> {
    client.nonces().clear_account(&account);
    Ok(client.nonces().fresh(&account).await?)
}

async fn transfer(
    client: &Client,
    dest: AccountId,
    rao: u64,
) -> Result<rtao_runtime::CallBytes, Box<dyn Error>> {
    let runtime = client.runtime().await?;
    Ok(
        balances::transfer_keep_alive(Value::Bytes(dest.0.to_vec()).into(), TaoBalance(rao))
            .compose(&runtime)?,
    )
}

#[tokio::test]
#[ignore = "needs a bootstrapped localnet; run ci/interop/remote-signer/localnet.sh"]
async fn remote_signer_signs_a_localnet_transfer_and_refuses_replay_and_tamper() -> TestResult {
    let endpoint = std::env::var("E2E_ENDPOINT")
        .map_err(|_| "E2E_ENDPOINT is required; run ci/interop/remote-signer/localnet.sh")?;
    let _ = rustls::crypto::ring::default_provider().install_default();
    let alice = key("//Alice")?;
    let bob = key("//Bob")?.account();
    let charlie = key("//Charlie")?;

    // 1. The reference server, online against the same node, on loopback.
    let mut config = ClientConfig::for_network(&endpoint)?;
    config.runtime_cache_dir = None;
    config.config_store = None;
    let client = Client::connect(config).await?;
    let genesis = client.genesis().0;
    let mut cfg = ServerConfig::new(genesis);
    cfg.accounts = vec![ServedAccount::new(Arc::new(alice.clone()))];
    cfg.clients = vec![AuthorizedClient {
        client: charlie.account(),
        accounts: BTreeSet::from([alice.account()]),
    }];
    cfg.policy.limits.max_spend = Some(Balance::<Tao>::parse("1")?);
    cfg.policy.allowed_calls = Some(BTreeSet::from([(
        "Balances".to_owned(),
        "transfer_keep_alive".to_owned(),
    )]));
    let server = RemoteSignerServer::new(
        cfg,
        Arc::new(ClientRuntimeSource::new(client.clone())),
        Arc::new(InMemoryNonceStore::new()),
    )?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    tokio::spawn(server.serve(listener));

    // 2. The client, with a separate btauth key.
    let base = Url::parse(&format!("http://{address}/"))?;
    let (remote, accounts) = RemoteSigner::connect(RemoteSignerConfig::new(
        base.clone(),
        alice.account(),
        CryptoType::Sr25519,
        Arc::new(charlie.clone()),
    ))
    .await?;
    assert_eq!(accounts.genesis_hash.0, genesis);
    let bob_before = free(&client, bob).await?;
    let nonce_before = next_nonce(&client, alice.account()).await?;

    // 3. A 0.1 TAO transfer signed remotely and included.
    let report = client
        .submit(
            transfer(&client, bob, 100_000_000).await?,
            &remote,
            &options(),
        )
        .await?;
    assert!(report.success, "{report:?}");
    assert_eq!(free(&client, bob).await? - bob_before, 100_000_000);
    assert_eq!(
        next_nonce(&client, alice.account()).await?,
        nonce_before + 1
    );

    // 4. Replay: one btauth-signed request sent twice.
    let unsigned = client
        .prepare_public(
            transfer(&client, bob, 50_000_000).await?,
            alice.account(),
            &PublicPrepareOptions::default(),
        )
        .await?;
    let body = SignRequest::from_signing_request(
        &unsigned.signing_request(SignPurpose::Extrinsic),
        alice.account(),
        CryptoType::Sr25519,
    );
    let bytes = serde_json::to_vec(&body)?;
    let headers = rtao_btauth::sign(
        &charlie,
        &Method::POST,
        "/sign",
        &bytes,
        Some(&alice.account()),
        &SystemClock::default(),
    )
    .await?;
    let http = reqwest::Client::builder().build()?;
    let post = |body: Vec<u8>, headers: http::HeaderMap| {
        http.post(
            base.join("sign")
                .map(|url| url.to_string())
                .unwrap_or_default(),
        )
        .headers(headers)
        .header("content-type", "application/json")
        .body(body)
        .send()
    };
    let first = post(bytes.clone(), headers.clone()).await?;
    assert_eq!(first.status(), StatusCode::OK);
    let signed: SignResponse = serde_json::from_slice(&first.bytes().await?)?;
    let second = post(bytes.clone(), headers.clone()).await?;
    assert_eq!(second.status(), StatusCode::UNAUTHORIZED);
    let detail: serde_json::Value = serde_json::from_slice(&second.bytes().await?)?;
    assert_eq!(
        detail["detail"],
        "this nonce was already accepted from this hotkey"
    );
    let report = client
        .submit_signature(
            &unsigned,
            signed.signature(CryptoType::Sr25519)?,
            &options(),
        )
        .await?;
    assert!(report.success, "{report:?}");

    // 5. Tamper in transit, an inconsistent authenticated body, and policy.
    let mut flipped = bytes.clone();
    let middle = flipped.len() / 2;
    flipped[middle] ^= 1;
    let reply = post(flipped, headers).await?;
    assert_eq!(reply.status(), StatusCode::UNAUTHORIZED);
    let mut inconsistent = body.clone();
    inconsistent
        .payload_json
        .insert("nonce".into(), serde_json::json!("0x7fffffff"));
    let inconsistent = serde_json::to_vec(&inconsistent)?;
    let fresh = rtao_btauth::sign(
        &charlie,
        &Method::POST,
        "/sign",
        &inconsistent,
        Some(&alice.account()),
        &SystemClock::default(),
    )
    .await?;
    let reply = post(inconsistent, fresh).await?;
    assert_eq!(reply.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let detail: serde_json::Value = serde_json::from_slice(&reply.bytes().await?)?;
    assert_eq!(detail["code"], "tampered_request");
    let large = client
        .prepare_public(
            transfer(&client, bob, 2_000_000_000).await?,
            alice.account(),
            &PublicPrepareOptions::default(),
        )
        .await?;
    let refused = remote
        .sign_request(&large.signing_request(SignPurpose::Extrinsic))
        .await
        .unwrap_err();
    assert!(
        matches!(&refused, RemoteSignerError::Http { status: 403, code: Some(code), .. } if code == "policy_denied"),
        "{refused:?}"
    );

    // 6. Exactly the two accepted transfers reached the chain.
    assert_eq!(
        next_nonce(&client, alice.account()).await?,
        nonce_before + 2
    );
    assert_eq!(free(&client, bob).await? - bob_before, 150_000_000);
    Ok(())
}
