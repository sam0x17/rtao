// Derived from RaoFoundation/subtensor sdk/python/bittensor/http_auth.py @ b6a81d144078eac18bd25c3af7cb31579bb670f3 (Apache-2.0); modified by rtao.
//! `prepare_signer` reads a hashed hotkey's finalized generation (D-153).
#![cfg(feature = "http-auth")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use rtao::{
    client::{Client, ClientConfig, Network},
    http_auth::{PreparedHttpSigner, prepare_signer},
};
use rtao_core::keys::{CRYPTO_HASHED, CRYPTO_MLDSA65_FIXED, Keypair};
use rtao_primitives::receiving::descriptor_value;
use rtao_signer::{AuthoritySigner, KeypairAuthoritySigner, KeypairSigner, Signer};
use rtao_testkit::{FakeChain, FakeState, rpc::RpcClient};
use serde_json::json;

async fn connect(state: FakeState) -> Client {
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None).unwrap()).unwrap();
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    Client::from_rpc(RpcClient::new(FakeChain::from_state(state)), config)
        .await
        .unwrap()
}

fn registered(signer: &KeypairAuthoritySigner, generation: u64) -> FakeState {
    let mut state = FakeState::default();
    state
        .seed_python(
            "HashedAccounts",
            "Accounts",
            &[json!(AuthoritySigner::account(signer).to_string())],
            json!({
                "descriptor": descriptor_value(signer.descriptor()).unwrap(),
                "generation": generation,
                "commitment": format!("0x{}", signer.commitment(generation).unwrap().iter().map(|byte| format!("{byte:02x}")).collect::<String>()),
            }),
        )
        .unwrap();
    state
}

#[tokio::test]
async fn hashed_hotkeys_sign_at_their_finalized_generation_only() {
    let classical: Arc<dyn Signer> =
        Arc::new(KeypairSigner::from_seed(&[1; 32], rtao_primitives::CryptoType::Sr25519).unwrap());
    let client = connect(FakeState::default()).await;
    assert!(matches!(
        prepare_signer(&client, classical).await.unwrap(),
        PreparedHttpSigner::Classical(_)
    ));
    let hashed =
        KeypairAuthoritySigner::new(Keypair::from_seed(&[2; 32], CRYPTO_HASHED).unwrap()).unwrap();
    // Unregistered: refused.
    let error = prepare_signer(&client, Arc::new(hashed.clone()))
        .await
        .err()
        .expect("unregistered");
    assert!(error.to_string().contains("not registered"), "{error}");
    // Generation zero not yet retired: refused.
    let client = connect(registered(&hashed, 0)).await;
    let error = prepare_signer(&client, Arc::new(hashed.clone()))
        .await
        .err()
        .expect("generation zero");
    assert!(error.to_string().contains("generation zero"), "{error}");
    let client = connect(registered(&hashed, 3)).await;
    match prepare_signer(&client, Arc::new(hashed)).await.unwrap() {
        PreparedHttpSigner::Hashed(signer) => assert_eq!(signer.generation(), 3),
        PreparedHttpSigner::Classical(_) => panic!("hashed signer prepared as classical"),
    }
    // Post-quantum HTTP signing is not supported.
    let pq =
        KeypairAuthoritySigner::new(Keypair::from_seed(&[4; 32], CRYPTO_MLDSA65_FIXED).unwrap())
            .unwrap();
    assert!(prepare_signer(&client, Arc::new(pq)).await.is_err());
}
