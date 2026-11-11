//! End-to-end miner runtime construction over a task-owned development wallet.

use super::*;
use axum::{Extension, http::Method, routing::post};
use rtao_btauth::{
    client::SignedClient,
    server::{Json, VerifiedCaller},
};
use rtao_core::keys::{CRYPTO_SR25519, Keypair};
use rtao_primitives::{CryptoType, NetUid};
use rtao_runtime::{models::AxonInfo, storage::subtensor_module as st};
use rtao_signer::{KeypairSigner, Signer};
use rtao_testkit::{FakeNode, FakeState};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

struct TestMiner {
    started: Arc<AtomicBool>,
    routed: Arc<tokio::sync::Notify>,
}
impl Miner for TestMiner {
    type State = Value;
    async fn on_start(&self, _cx: &NeuronCx, state: &StateCell<Value>) -> anyhow::Result<()> {
        anyhow::ensure!(
            state.read()["requests"] == 40,
            "checkpoint was not restored before on_start"
        );
        *state.write() = json!({"requests":41});
        self.started.store(true, Ordering::SeqCst);
        Ok(())
    }
    fn router(&self, _cx: &NeuronCx, state: StateCell<Value>) -> Router {
        assert!(self.started.load(Ordering::SeqCst));
        assert_eq!(state.read()["requests"], 41);
        self.routed.notify_one();
        Router::new()
            .route("/echo", post(echo))
            .layer(Extension(state))
    }
}
async fn echo(
    VerifiedCaller(_caller): VerifiedCaller,
    Extension(state): Extension<StateCell<Value>>,
    Json(body): Json<Value>,
) -> Json<Value> {
    let mut state = state.write();
    state["requests"] = json!(state["requests"].as_u64().unwrap_or(0) + 1);
    Json(json!({"bytes":body["payload"].as_str().map(str::len),"requests":state["requests"]}))
}

fn fixture_http_client(signer: Arc<dyn Signer>) -> anyhow::Result<SignedClient> {
    // Loopback HTTP needs no trust roots; keep the fixture independent of host stores.
    let tls = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()?
    .with_root_certificates(rustls::RootCertStore::empty())
    .with_no_client_auth();
    Ok(SignedClient::with_tls_config(
        signer,
        Arc::new(rtao_btauth::SystemClock::default()),
        tls,
    )?)
}

#[tokio::test]
async fn miner_runtime_restores_before_router_accepts_signed_three_mib_and_persists_shutdown()
-> anyhow::Result<()> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let base = std::env::var_os("RTAO_TEMP_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/neuron-tests")
        });
    let root = base.join("tmp").join(format!(
        "miner-runtime-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(
        root.parent()
            .ok_or_else(|| anyhow::anyhow!("fixture parent"))?,
    )?;
    std::fs::create_dir(&root)?;
    let result = run_fixture(&root).await;
    std::fs::remove_dir_all(&root)?;
    result
}
async fn run_fixture(root: &std::path::Path) -> anyhow::Result<()> {
    let keypair = Keypair::from_uri("//Alice", CRYPTO_SR25519)?;
    let miner = KeypairSigner::from_uri("//Alice", CryptoType::Sr25519)?;
    let validator = KeypairSigner::from_uri("//Bob", CryptoType::Sr25519)?;
    // A hashed Sr25519 validator hotkey, registered at generation 2 (D-171,
    // D-172): the miner's default registry trusts its hashed scheme.
    let hashed = rtao_signer::KeypairAuthoritySigner::new(Keypair::from_seed(
        &[42; 32],
        rtao_core::keys::CRYPTO_HASHED,
    )?)?;
    let hashed_account = rtao_signer::AuthoritySigner::account(&hashed);
    let mut config = NeuronConfig::load_with(
        root,
        &BTreeMap::from([
            ("BT_NETWORK".into(), std::ffi::OsString::from("local")),
            (
                "BT_WALLET_PATH".into(),
                root.join("wallets").into_os_string(),
            ),
            ("RTAO_STATE_DIR".into(), root.join("state").into_os_string()),
        ]),
        None,
        Some(NetUid(1)),
    )
    .await?;
    let keyfile = config.wallet_reference()?.hotkey_path();
    std::fs::create_dir_all(
        keyfile
            .parent()
            .ok_or_else(|| anyhow::anyhow!("keyfile parent"))?,
    )?;
    std::fs::write(
        &keyfile,
        rtao_core::keyfile_json::serialized_keypair_to_keyfile_data(&keypair)?,
    )?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    config.axon_port = address.port();
    config.axon_external_ip = Some("127.0.0.2".parse()?);
    let mut state = FakeState::try_default()?;
    let graph = json!({"netuid":1,"block":100,"tempo":10,"last_step":90,"num_uids":3,
        "hotkeys":[miner.account().to_string(),validator.account().to_string(),hashed_account.to_string()],
        "validator_permit":[false,true,true],"validators":[1,2],"total_stake":[0,1000000000,1000000000]});
    state.seed_python(
        "HashedAccounts",
        "Accounts",
        &[json!(hashed_account.to_string())],
        json!({
            "descriptor": rtao_primitives::receiving::descriptor_value(
                rtao_signer::AuthoritySigner::descriptor(&hashed)
            )?,
            "generation": 2,
            "commitment": format!("0x{}", hex::encode(rtao_signer::AuthoritySigner::commitment(&hashed, 2)?)),
        }),
    )?;
    state.seed_runtime(
        "SubnetInfoRuntimeApi",
        "get_selective_mechagraph",
        graph.clone(),
    )?;
    state.seed_runtime("SubnetInfoRuntimeApi", "get_mechagraph", graph)?;
    state.seed_typed(
        "SubtensorModule",
        "Uids",
        (NetUid(1), miner.account()),
        0u16,
    )?;
    state.seed(&st::SUBNETWORK_N, (NetUid(1),), 3u16)?;
    state.seed_typed("SubtensorModule", "Keys", (NetUid(1), 2u16), hashed_account)?;
    state.seed_typed(
        "SubtensorModule",
        "Keys",
        (NetUid(1), 0u16),
        miner.account(),
    )?;
    state.seed_typed(
        "SubtensorModule",
        "Keys",
        (NetUid(1), 1u16),
        validator.account(),
    )?;
    state.seed_typed(
        "SubtensorModule",
        "Axons",
        (NetUid(1), miner.account()),
        AxonInfo {
            block: 100,
            ip: u128::from(u32::from(Ipv4Addr::new(127, 0, 0, 2))),
            port: address.port(),
            version: 1,
            protocol: 4,
            ..Default::default()
        },
    )?;
    state.seed(&st::SERVING_RATE_LIMIT, (NetUid(1),), 0u64)?;
    let mut node = FakeNode::with_state(Arc::new(Mutex::new(state))).await?;
    config.chain_endpoint = Some(node.url().parse()?);
    config.fallback_endpoints = Some(vec!["none".into()]);
    config.archive_endpoints = Some(vec!["none".into()]);
    let mut chain_config = crate::client_config(&config)?;
    chain_config.runtime_cache_dir = None;
    chain_config.config_store = None;
    let client = rtao_client::Client::connect(chain_config).await;
    let context = match client {
        Ok(client) => {
            crate::runtime::initialize_with_http_factory(
                &config,
                Metrics::default(),
                SyncPolicy::miner(),
                "miner",
                client,
                fixture_http_client,
            )
            .await
        }
        Err(error) => {
            node.shutdown().await;
            return Err(error.into());
        }
    };
    let cx = match context {
        Ok(cx) => cx,
        Err(error) => {
            node.shutdown().await;
            return Err(error);
        }
    };
    let initial = StateStore::<Value>::new(cx.state_dir.clone(), NetUid(1), miner.account(), 1)
        .save(&StateCell::new(json!({"requests":40})), 99)
        .await;
    if let Err(error) = initial {
        cx.close().await;
        node.shutdown().await;
        return Err(error.into());
    }
    let routed = Arc::new(tokio::sync::Notify::new());
    let application = TestMiner {
        started: Arc::new(AtomicBool::new(false)),
        routed: routed.clone(),
    };
    let mut task = tokio::spawn(
        NeuronRuntime::miner(config, application)
            .with_context(cx.clone())
            .with_listener(listener)
            .run(),
    );
    let requests = async {
        tokio::time::timeout(Duration::from_secs(5), routed.notified()).await?;
        let http = fixture_http_client(Arc::new(validator))?;
        let response = http
            .request(
                Method::POST,
                format!("http://{address}/echo").parse()?,
                &miner.account(),
            )
            .json(&json!({"payload":"x".repeat(3*1024*1024)}))
            .send()
            .await?;
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let response: Value = serde_json::from_slice(&response.bytes().await?)?;
        assert_eq!(response["bytes"], 3 * 1024 * 1024);
        assert_eq!(response["requests"], 42);
        // A hashed validator signs only once bound to its finalized generation.
        let hashed_http = fixture_http_client(Arc::new(hashed))?;
        let refused = hashed_http
            .request(
                Method::POST,
                format!("http://{address}/echo").parse()?,
                &miner.account(),
            )
            .json(&json!({"payload":"y"}))
            .send()
            .await
            .err()
            .ok_or_else(|| anyhow::anyhow!("unprepared hashed signer signed"))?;
        assert!(
            refused.to_string().contains("finalized generation"),
            "{refused}"
        );
        crate::runtime::refresh_http_identity(
            &cx.client,
            &hashed_http,
            rtao_client::BlockRef::Finalized,
            &cx.metrics,
        )
        .await;
        assert_eq!(hashed_http.hashed_generation(), Some(2));
        let response = hashed_http
            .request(
                Method::POST,
                format!("http://{address}/echo").parse()?,
                &miner.account(),
            )
            .json(&json!({"payload":"y"}))
            .send()
            .await?;
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let response: Value = serde_json::from_slice(&response.bytes().await?)?;
        assert_eq!(response["requests"], 43);
        Ok::<_, anyhow::Error>(())
    }
    .await;
    cx.shutdown.cancel();
    let stopped = match tokio::time::timeout(Duration::from_secs(10), &mut task).await {
        Ok(result) => result
            .map_err(anyhow::Error::from)
            .and_then(|result| result),
        Err(error) => {
            task.abort();
            let _ = task.await;
            Err(error.into())
        }
    };
    cx.close().await;
    node.shutdown().await;
    requests?;
    stopped?;
    let saved = StateStore::<Value>::new(cx.state_dir.clone(), NetUid(1), miner.account(), 1)
        .load()
        .await?
        .ok_or_else(|| anyhow::anyhow!("final miner state was not saved"))?;
    assert_eq!(saved.state["requests"], 43);
    assert!(node.mutate_state(|state| state.submissions.is_empty()));
    Ok(())
}

/// Generation zero is never signed (D-171): the HTTP signer stays refused,
/// with the explicit retirement message, and nothing is submitted.
#[tokio::test]
async fn hashed_http_signer_waits_for_generation_zero_retirement() -> anyhow::Result<()> {
    use rtao_signer::{AuthoritySigner, KeypairAuthoritySigner};
    let hashed = KeypairAuthoritySigner::new(Keypair::from_seed(
        &[43; 32],
        rtao_core::keys::CRYPTO_HASHED_ED25519,
    )?)?;
    for (generation, ready) in [(0u64, false), (3, true)] {
        let mut state = FakeState::try_default()?;
        state.seed_python(
            "HashedAccounts",
            "Accounts",
            &[json!(AuthoritySigner::account(&hashed).to_string())],
            json!({
                "descriptor": rtao_primitives::receiving::descriptor_value(hashed.descriptor())?,
                "generation": generation,
                "commitment": format!("0x{}", hex::encode(hashed.commitment(generation)?)),
            }),
        )?;
        let chain = rtao_testkit::FakeChain::from_state(state);
        let mut config =
            rtao_client::ClientConfig::new(rtao_client::Network::resolve_with("local", |_| None)?)?;
        config.track_heads = false;
        config.runtime_cache_dir = None;
        config.config_store = None;
        let client =
            rtao_client::Client::from_rpc(rtao_testkit::rpc::RpcClient::new(chain.clone()), config)
                .await?;
        let http = fixture_http_client(Arc::new(hashed.clone()))?;
        let metrics = Metrics::default();
        crate::runtime::refresh_http_identity(
            &client,
            &http,
            rtao_client::BlockRef::Finalized,
            &metrics,
        )
        .await;
        assert_eq!(http.ready().is_ok(), ready);
        if ready {
            assert_eq!(http.hashed_generation(), Some(generation));
        } else {
            let Err(error) = http.ready() else {
                anyhow::bail!("generation zero was not refused");
            };
            assert!(
                error.to_string().contains("generation zero to be retired"),
                "{error}"
            );
        }
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        client.close().await;
    }
    Ok(())
}
