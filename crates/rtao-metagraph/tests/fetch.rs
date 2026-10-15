//! Curated graph reads retain one block across RPC and commitment scans.
#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use rtao_client::{BlockRef, Client, ClientConfig, Network};
use rtao_core::{
    codec::{Value, decode::Cursor},
    runtime::Runtime,
};
use rtao_metagraph::{DecodePath, DecodePreference, MetagraphOptions, SubnetsMetagraphExt};
use rtao_primitives::{AccountId, NetUid, Uid};
use rtao_reads::{Namespaces, ReadError};
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
fn seed() -> (FakeChain, AccountId, AccountId) {
    let runtime = Arc::new(
        Runtime::parse(
            include_bytes!("../../../vendor/metadata/finney/478.v15.scale"),
            478,
            1,
            42,
        )
        .unwrap(),
    );
    let method = runtime
        .apis
        .iter()
        .find(|api| api.name == "SubnetInfoRuntimeApi")
        .unwrap()
        .methods
        .iter()
        .find(|method| method.name == "get_metagraph")
        .unwrap();
    let mut cursor = Cursor::new(include_bytes!("../../../vendor/bench/finney-1.scale"));
    let value = runtime.decode_id(method.output, &mut cursor).unwrap();
    let Value::Dict(fields) = value else {
        unreachable!()
    };
    let Value::List(hotkeys) = &fields
        .iter()
        .find(|(key, _)| key == &Value::Str("hotkeys".into()))
        .unwrap()
        .1
    else {
        unreachable!()
    };
    let Value::Str(hotkey) = hotkeys.first().unwrap() else {
        unreachable!()
    };
    let registered: AccountId = hotkey.parse().unwrap();
    let unregistered = AccountId([197; 32]);
    let mut state = FakeState::new(runtime);
    state.seed_runtime_encoded(
        "SubnetInfoRuntimeApi_get_metagraph",
        vec![1, 0],
        include_bytes!("../../../vendor/bench/finney-1.scale").to_vec(),
    );
    for hotkey in [registered, unregistered] {
        state
            .seed_python(
                "Commitments",
                "CommitmentOf",
                &[json!(1), json!(hotkey.to_string())],
                json!({"block":0,"deposit":7,"info":{"fields":[{"Raw5":"0x706c61696e"}]}}),
            )
            .unwrap();
    }
    (FakeChain::from_state(state), registered, unregistered)
}

#[tokio::test]
async fn curated_default_fetches_commitments_and_graph_at_one_block() {
    let (chain, registered, unregistered) = seed();
    let client = Client::from_rpc(RpcClient::new(chain.clone()), config())
        .await
        .unwrap();
    let start = chain.requests().len();
    let graph = client
        .subnets()
        .metagraph(NetUid(1))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(graph.decode_path(), DecodePath::View);
    assert_eq!(graph.by_hotkey(&registered).unwrap().uid, Uid(0));
    assert_eq!(
        graph
            .commitments
            .as_ref()
            .unwrap()
            .get(&Uid(0))
            .unwrap()
            .value(),
        Some("plain")
    );
    assert_eq!(
        graph
            .unregistered_commitments
            .as_ref()
            .unwrap()
            .get(&unregistered.to_string())
            .unwrap()
            .uid,
        None
    );
    assert_eq!(
        graph.to_owned_neurons().unwrap()[0]
            .commitment
            .as_ref()
            .unwrap()
            .uid,
        Some(Uid(0))
    );
    assert_eq!(
        graph.to_json().unwrap()["neurons"][0]["commitment"]["data"],
        "plain"
    );
    let requests = chain
        .requests()
        .into_iter()
        .skip(start)
        .filter(|request| {
            matches!(
                request.method.as_str(),
                "state_call" | "state_getKeysPaged" | "state_queryStorageAt"
            )
        })
        .collect::<Vec<_>>();
    assert!(
        requests
            .iter()
            .any(|request| request.method == "state_call")
    );
    assert!(
        requests
            .iter()
            .any(|request| request.method == "state_getKeysPaged")
    );
    let hash = format!("0x{}", hex::encode(graph.at.hash.0));
    for request in requests {
        assert_eq!(
            request.params.as_array().unwrap().last().unwrap(),
            &json!(hash)
        );
    }
    client.close().await;
}

#[tokio::test]
async fn snapshot_retains_block_and_commitment_opt_out_avoids_scans() {
    let (chain, _, _) = seed();
    let client = Client::from_rpc(RpcClient::new(chain.clone()), config())
        .await
        .unwrap();
    let snapshot = client.at(BlockRef::Best).await.unwrap();
    chain.advance(2).unwrap();
    let start = chain.requests().len();
    let graph = snapshot
        .subnets()
        .metagraph_with(
            NetUid(1),
            MetagraphOptions {
                commitments: false,
                ..MetagraphOptions::default()
            },
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(graph.at.hash, snapshot.block().hash);
    assert!(graph.commitments.is_none());
    assert!(
        graph
            .to_owned_neurons()
            .unwrap()
            .iter()
            .all(|neuron| neuron.commitment.is_none())
    );
    let state_requests = chain
        .requests()
        .into_iter()
        .skip(start)
        .filter(|request| request.method.starts_with("state_"))
        .collect::<Vec<_>>();
    assert_eq!(state_requests.len(), 1);
    assert_eq!(state_requests[0].method, "state_call");
    assert_eq!(
        state_requests[0].params[2],
        json!(format!("0x{}", hex::encode(snapshot.block().hash.0)))
    );
    client.close().await;
}

#[tokio::test]
async fn missing_subnet_returns_none() {
    let (chain, _, _) = seed();
    chain.mutate_state(|state| {
        state.seed_runtime_encoded("SubnetInfoRuntimeApi_get_metagraph", vec![1, 0], vec![0])
    });
    let client = Client::from_rpc(RpcClient::new(chain), config())
        .await
        .unwrap();
    assert!(
        client
            .subnets()
            .metagraph(NetUid(1))
            .await
            .unwrap()
            .is_none()
    );
    client.close().await;
}

#[tokio::test]
async fn client_view_opt_out_uses_typed_decode_for_client_and_snapshot() {
    let (chain, _, _) = seed();
    let mut config = config();
    config.metagraph_view = false;
    let client = Client::from_rpc(RpcClient::new(chain.clone()), config)
        .await
        .unwrap();
    let options = MetagraphOptions {
        commitments: false,
        view: true,
        decoder: DecodePreference::Auto,
    };
    let graph = client
        .subnets()
        .metagraph_with(NetUid(1), options)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(graph.decode_path(), DecodePath::Typed);
    for (decoder, expected) in [
        (DecodePreference::Typed, DecodePath::Typed),
        (DecodePreference::Dynamic, DecodePath::Dynamic),
    ] {
        let graph = client
            .subnets()
            .metagraph_with(NetUid(1), MetagraphOptions { decoder, ..options })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(graph.decode_path(), expected);
    }
    let snapshot = client.at(BlockRef::Best).await.unwrap();
    let graph = snapshot
        .subnets()
        .metagraph_with(NetUid(1), options)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(graph.decode_path(), DecodePath::Typed);
    let start = chain.requests().len();
    let error = client
        .subnets()
        .metagraph_with(
            NetUid(1),
            MetagraphOptions {
                commitments: true,
                decoder: DecodePreference::View,
                ..options
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(error, ReadError::InvalidParam { name, reason }
        if name == "decoder" && reason == "metagraph view is disabled"));
    assert!(
        !chain
            .requests()
            .into_iter()
            .skip(start)
            .any(|request| matches!(
                request.method.as_str(),
                "state_call" | "state_getKeysPaged" | "state_queryStorageAt"
            ))
    );
    client.close().await;
}

#[tokio::test]
async fn per_call_view_opt_out_uses_typed_decode_and_refuses_forced_view() {
    let (chain, _, _) = seed();
    let client = Client::from_rpc(RpcClient::new(chain.clone()), config())
        .await
        .unwrap();
    let options = MetagraphOptions {
        commitments: false,
        view: false,
        decoder: DecodePreference::Auto,
    };
    let graph = client
        .subnets()
        .metagraph_with(NetUid(1), options)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(graph.decode_path(), DecodePath::Typed);
    let start = chain.requests().len();
    let error = client
        .subnets()
        .metagraph_with(
            NetUid(1),
            MetagraphOptions {
                commitments: true,
                decoder: DecodePreference::View,
                ..options
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(error, ReadError::InvalidParam { name, reason }
        if name == "decoder" && reason == "metagraph view is disabled"));
    assert!(
        !chain
            .requests()
            .into_iter()
            .skip(start)
            .any(|request| matches!(
                request.method.as_str(),
                "state_call" | "state_getKeysPaged" | "state_queryStorageAt"
            ))
    );
    client.close().await;
}

#[tokio::test]
#[ignore = "run in a separate process with RTAO_METAGRAPH_VIEW=0"]
async fn environment_view_opt_out_overrides_explicit_options() {
    assert_eq!(std::env::var("RTAO_METAGRAPH_VIEW").as_deref(), Ok("0"));
    let (chain, _, _) = seed();
    let client = Client::from_rpc(RpcClient::new(chain.clone()), config())
        .await
        .unwrap();
    let options = MetagraphOptions {
        commitments: false,
        view: true,
        decoder: DecodePreference::Auto,
    };
    let graph = client
        .subnets()
        .metagraph_with(NetUid(1), options)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(graph.decode_path(), DecodePath::Typed);
    let start = chain.requests().len();
    let error = client
        .subnets()
        .metagraph_with(
            NetUid(1),
            MetagraphOptions {
                commitments: true,
                decoder: DecodePreference::View,
                ..options
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(error, ReadError::InvalidParam { name, reason }
        if name == "decoder" && reason == "metagraph view is disabled"));
    assert!(
        !chain
            .requests()
            .into_iter()
            .skip(start)
            .any(|request| matches!(
                request.method.as_str(),
                "state_call" | "state_getKeysPaged" | "state_queryStorageAt"
            ))
    );
    client.close().await;
}

#[test]
fn fetch_environment_opt_out_is_checked_in_a_child_process() {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .env("RTAO_METAGRAPH_VIEW", "0")
        .args([
            "--ignored",
            "--exact",
            "environment_view_opt_out_overrides_explicit_options",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
