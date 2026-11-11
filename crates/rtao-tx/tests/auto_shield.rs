// Derived from RaoFoundation/subtensor sdk/python/bittensor/shielding.py @ b6a81d144078eac18bd25c3af7cb31579bb670f3 (Apache-2.0); modified by rtao.
//! The registration Shield default (D-170, superseding D-140's opt-in):
//! Python's `shielded=None`, on unless the executor opts out.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use rtao_client::{Client, ClientConfig, Network};
use rtao_intents::CreatePureProxy;
use rtao_primitives::CryptoType;
use rtao_signer::{KeypairSigner, Signer};
use rtao_testkit::rpc::RpcClient;
use rtao_testkit::{FakeChain, FakeState};
use rtao_tx::{Executor, PlanOptions};
use serde_json::json;

async fn executor(next_key: bool, signer: &KeypairSigner) -> Executor {
    let mut state = FakeState::try_default().unwrap();
    // A quantum-resistant payer creates a protected pure proxy, an account
    // registration (Proxy.create_protected_pure).
    state
        .seed_python(
            "HashedAccounts",
            "MlDsaAccounts",
            &[json!(signer.account().to_string())],
            json!({}),
        )
        .unwrap();
    state
        .seed_python(
            "HashedAccounts",
            "HashedAccountRegistrationDeposit",
            &[],
            json!(250_000_000u64),
        )
        .unwrap();
    if next_key {
        state
            .seed_typed("MevShield", "NextKey", (), vec![0u8; 1184])
            .unwrap();
    }
    let fake = FakeChain::from_state(state);
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None).unwrap()).unwrap();
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    Executor::new(
        Client::from_rpc(RpcClient::new(fake), config)
            .await
            .unwrap(),
    )
}

fn intent() -> CreatePureProxy {
    CreatePureProxy {
        account_mode: None,
        proxy_type: "Any".into(),
        delay: 0,
        index: 0,
    }
}

#[tokio::test]
async fn registration_takes_shield_by_default_when_available() {
    let signer = KeypairSigner::from_seed(&[7; 32], CryptoType::Sr25519).unwrap();
    // On by default, as Python's `shielded=None`.
    let auto = executor(true, &signer).await;
    let plan = auto
        .plan(&intent(), &signer, &PlanOptions::default())
        .await
        .unwrap();
    assert!(plan.shielded, "{:?}", plan.warnings);
    assert_eq!(plan.to_json()["shielded"], true);

    // Python's explicit `shielded=False`: the executor opts out.
    let plain = executor(true, &signer).await.with_auto_shield(false);
    let plan = plain
        .plan(&intent(), &signer, &PlanOptions::default())
        .await
        .unwrap();
    assert!(!plan.shielded);
    assert_eq!(plan.to_json()["shielded"], false);
    // An explicit request still shields with the default off.
    let plan = plain
        .plan(
            &intent(),
            &signer,
            &PlanOptions {
                shielded: true,
                ..PlanOptions::default()
            },
        )
        .await
        .unwrap();
    assert!(plan.shielded);

    // Without a shield key the registration is planned in the clear, with
    // Python's warning, never silently.
    let unavailable = executor(false, &signer).await;
    let plan = unavailable
        .plan(&intent(), &signer, &PlanOptions::default())
        .await
        .unwrap();
    assert!(!plan.shielded);
    assert!(
        plan.warnings
            .iter()
            .any(|warning| warning.starts_with("MEV Shield is not active on this network")),
        "{:?}",
        plan.warnings
    );
}

#[tokio::test]
async fn ordinary_calls_never_take_the_automatic_route() {
    let signer = KeypairSigner::from_seed(&[8; 32], CryptoType::Sr25519).unwrap();
    let auto = executor(
        true,
        &KeypairSigner::from_seed(&[9; 32], CryptoType::Sr25519).unwrap(),
    )
    .await;
    // This payer is not protected, so the call is the plain Proxy.create_pure.
    let plan = auto
        .plan(&intent(), &signer, &PlanOptions::default())
        .await
        .unwrap();
    assert!(!plan.shielded);
}
