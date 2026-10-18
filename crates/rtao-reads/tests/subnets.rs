// Derived from RaoFoundation/subtensor sdk/python/bittensor/reads/subnets.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! First catalog reads through the public client and real SCALE fixtures.

#![allow(clippy::unwrap_used)] // Assertions must fail when fake-chain fixtures or responses are invalid.

use rtao_client::{Client, ClientConfig, Network};
use rtao_primitives::NetUid;
use rtao_reads::{Namespaces, reads::subnets};
use rtao_testkit::rpc::RpcClient;
use rtao_testkit::{FakeChain, FakeState};
use serde_json::json;

fn config() -> ClientConfig {
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None).unwrap()).unwrap();
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    config
}

#[tokio::test]
async fn raw_metagraph_preserves_runtime_record_and_decodes_compact_text() {
    let mut state = FakeState::default();
    state
        .seed_runtime(
            "SubnetInfoRuntimeApi",
            "get_metagraph",
            json!({"netuid": 42, "name": [207, 132], "symbol": [255], "block": 100}),
        )
        .unwrap();
    let chain = FakeChain::from_state(state);
    let client = Client::from_rpc(RpcClient::new(chain.clone()), config())
        .await
        .unwrap();
    let graph = subnets::metagraph(&client, NetUid(42)).await.unwrap();
    assert_eq!(graph["name"], json!("τ"));
    assert_eq!(graph["symbol"], json!("0xff"));
    assert_eq!(graph["netuid"], json!(42));
    assert_eq!(graph["block"], json!(100));
    assert!(graph["hotkeys"].is_array());
    assert!(
        chain
            .requests()
            .iter()
            .any(|request| request.method == "state_call"
                && request.params[0] == json!("SubnetInfoRuntimeApi_get_metagraph"))
    );
    chain.mutate_state(|state| {
        state
            .seed_runtime("SubnetInfoRuntimeApi", "get_metagraph", json!(null))
            .unwrap()
    });
    assert!(
        subnets::metagraph(&client, NetUid(65000))
            .await
            .unwrap()
            .is_null()
    );
    client.close().await;
}

#[tokio::test]
async fn token_symbols_retains_invalid_utf8_as_hex() {
    let mut state = FakeState::default();
    state
        .seed_python("SubtensorModule", "TokenSymbol", &[json!(1)], json!("τ"))
        .unwrap();
    state
        .seed_python(
            "SubtensorModule",
            "TokenSymbol",
            &[json!(42)],
            json!("0xff00"),
        )
        .unwrap();
    let chain = FakeChain::from_state(state);
    let client = Client::from_rpc(RpcClient::new(chain), config())
        .await
        .unwrap();
    let symbols = client.subnets().token_symbols().await.unwrap();
    assert_eq!(symbols.get(&NetUid(1)).map(String::as_str), Some("τ"));
    assert_eq!(symbols.get(&NetUid(42)).map(String::as_str), Some("0xff00"));
    client.close().await;
}
