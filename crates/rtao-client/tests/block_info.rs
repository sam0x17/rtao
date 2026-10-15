//! Public block lookups through metadata-backed fake chain transport.
#![allow(clippy::unwrap_used, clippy::panic)] // Invalid fixtures must fail assertions.

use std::sync::Arc;

use chrono::Timelike;
use parity_scale_codec::{Compact, Decode, Encode};
use rtao_client::{BlockRef, Client, ClientConfig, ClientError, Network};
use rtao_core::{DynValue, codec::Value as CoreValue, runtime::Runtime};
use rtao_testkit::{FakeChain, FakeState, RecordingView, Reply, rpc::RpcClient};
use serde_json::{Value, json};

fn unsigned(runtime: &Runtime, pallet: &str, function: &str, args: CoreValue) -> Vec<u8> {
    let call = runtime.compose_call(pallet, function, &args).unwrap();
    let mut body = vec![4];
    body.extend(call);
    let mut bytes = Compact(body.len() as u32).encode();
    bytes.extend(body);
    bytes
}

fn timestamp_extrinsic(state: &FakeState, millis: u64) -> Vec<u8> {
    unsigned(
        &state.runtime,
        "Timestamp",
        "set",
        CoreValue::record(vec![("now".into(), CoreValue::Int(i128::from(millis)))]),
    )
}

fn response(state: &FakeState, block: u64, extrinsics: Vec<Value>) -> Value {
    let hash = state.block_hash(block).unwrap();
    json!({"block": {"header": state.header(&hash).unwrap(), "extrinsics": extrinsics},
        "justifications": null})
}

fn serve(state: &mut FakeState, encoded: Vec<Vec<u8>>) {
    let extrinsics = encoded
        .into_iter()
        .map(|bytes| json!(format!("0x{}", hex::encode(bytes))))
        .collect();
    let response = response(state, 100, extrinsics);
    state.set_result("chain_getBlock", response);
}

async fn setup_client(state: FakeState) -> (Client, FakeChain) {
    let chain = FakeChain::from_state(state);
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None).unwrap()).unwrap();
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    let client = Client::from_rpc(RpcClient::new(chain.clone()), config)
        .await
        .unwrap();
    (client, chain)
}

#[tokio::test]
async fn closed_clients_and_snapshots_refuse_block_lookups_without_rpc() {
    let (client, chain) = setup_client(FakeState::default()).await;
    let snapshot = client.at(BlockRef::Number(99)).await.unwrap();
    client.close().await;
    let before = chain.requests().len();
    for result in [
        client.block_info(None).await,
        client.block_info(Some(100)).await,
        snapshot.block_info(None).await,
        snapshot.block_info(Some(100)).await,
    ] {
        assert!(result.is_err());
    }
    assert_eq!(chain.requests().len(), before);
}

#[tokio::test]
async fn full_decoded_calls_use_inherent_before_storage_and_normalize_header() {
    let mut state = FakeState::default();
    state
        .seed_python("Timestamp", "Now", &[], json!(1))
        .unwrap();
    let inherent = timestamp_extrinsic(&state, 1700000000123);
    let remark = unsigned(
        &state.runtime,
        "System",
        "remark",
        CoreValue::record(vec![("remark".into(), CoreValue::hex(b"hello"))]),
    );
    let expected = [&inherent, &remark]
        .into_iter()
        .map(|bytes| DynValue::new(state.runtime.decode_extrinsic(bytes, true).unwrap()).to_json())
        .collect::<Vec<_>>();
    let hash = state.block_hash(100).unwrap();
    serve(&mut state, vec![inherent, remark]);
    let (client, chain) = setup_client(state).await;
    let start = chain.requests().len();
    let info = client.block_info(Some(100)).await.unwrap().unwrap();
    assert_eq!(info.number, 100);
    assert_eq!(info.hash, Some(hash.clone()));
    assert_eq!(info.header["number"], 100);
    assert_eq!(info.header["hash"], hash);
    assert_eq!(info.extrinsics, expected);
    assert_eq!(info.extrinsics[0]["call"]["call_module"], "Timestamp");
    assert_eq!(info.extrinsics[1]["call"]["call_function"], "remark");
    assert_eq!(info.timestamp.timestamp_millis(), 1700000000123);
    assert_eq!(info.timestamp.nanosecond(), 123000000);
    assert_eq!(info.explorer_url, None);
    assert!(!chain.requests()[start..].iter().any(|request| {
        matches!(
            request.method.as_str(),
            "state_getStorage" | "state_queryStorageAt"
        )
    }));
    client.close().await;
}

#[tokio::test]
async fn missing_inherent_falls_back_at_the_requested_snapshot_and_keeps_observer() {
    let mut state = FakeState::default();
    state
        .seed_python("Timestamp", "Now", &[], json!(1700000000123u64))
        .unwrap();
    let (client, chain) = setup_client(state).await;
    let snapshot = client.at(BlockRef::Number(99)).await.unwrap();
    let expected = snapshot.block().clone();
    let observed = RecordingView::new(snapshot);
    use rtao_client::View;
    let snapshot = observed.pin().await.unwrap();
    chain.advance(1).unwrap();
    let start = chain.requests().len();
    let info = snapshot.block_info(None).await.unwrap().unwrap();
    assert_eq!(info.number, 99);
    assert_eq!(
        info.hash,
        Some(format!("0x{}", hex::encode(expected.hash.0)))
    );
    assert_eq!(info.timestamp.timestamp_millis(), 1700000000123);
    assert_eq!(
        observed.block_hashes(),
        [expected.hash].into_iter().collect()
    );
    assert_eq!(observed.touches().len(), 1);
    assert_eq!(observed.touches()[0].pallet_or_api, "Timestamp");
    for request in chain.requests()[start..].iter().filter(|request| {
        matches!(
            request.method.as_str(),
            "chain_getBlock" | "state_getStorage" | "state_queryStorageAt"
        )
    }) {
        assert_eq!(
            request.params.as_array().unwrap().last(),
            Some(&json!(format!("0x{}", hex::encode(expected.hash.0))))
        );
    }
    client.close().await;
}

#[tokio::test]
async fn explicit_snapshot_request_switches_identity_and_preserves_observer() {
    let mut state = FakeState::default();
    state
        .seed_python("Timestamp", "Now", &[], json!(1700000000000u64))
        .unwrap();
    let (client, _) = setup_client(state).await;
    let observed = RecordingView::new(client.at(BlockRef::Number(99)).await.unwrap());
    use rtao_client::View;
    let snapshot = observed.pin().await.unwrap();
    let target = client
        .at(BlockRef::Number(100))
        .await
        .unwrap()
        .block()
        .clone();
    let info = snapshot.block_info(Some(100)).await.unwrap().unwrap();
    assert_eq!(info.number, 100);
    assert_eq!(observed.block_hashes(), [target.hash].into_iter().collect());
    client.close().await;
}

#[tokio::test]
async fn absent_height_body_and_header_return_none() {
    let mut state = FakeState::default();
    state
        .seed_python("Timestamp", "Now", &[], json!(0))
        .unwrap();
    let (client, chain) = setup_client(state).await;
    assert!(client.block_info(Some(999999)).await.unwrap().is_none());
    chain.mutate_state(|state| state.set_result("chain_getBlock", Value::Null));
    assert!(client.block_info(Some(100)).await.unwrap().is_none());
    for header in [Value::Null, json!({})] {
        chain.mutate_state(|state| {
            state.set_result("chain_getBlock", json!({"block": {"header": header}}))
        });
        assert!(client.block_info(Some(100)).await.unwrap().is_none());
    }
    client.close().await;
}

#[tokio::test]
async fn malformed_extrinsics_fail_instead_of_silently_dropping_entries() {
    for bad in [json!("0x"), json!("0xff"), json!("0xgg"), Value::Null] {
        let mut state = FakeState::default();
        let inherent = timestamp_extrinsic(&state, 1700000000000);
        let response = response(
            &state,
            100,
            vec![json!(format!("0x{}", hex::encode(inherent))), bad],
        );
        state.set_result("chain_getBlock", response);
        let (client, _) = setup_client(state).await;
        assert!(client.block_info(Some(100)).await.is_err());
        client.close().await;
    }
    let mut state = FakeState::default();
    let mut framed = timestamp_extrinsic(&state, 1700000000000);
    framed.push(0); // strict core decode must reject trailing bytes after the declared body.
    serve(&mut state, vec![framed]);
    let (client, _) = setup_client(state).await;
    assert!(matches!(
        client.block_info(Some(100)).await,
        Err(ClientError::Core(_))
    ));
    client.close().await;
}

#[tokio::test]
async fn large_block_decode_retains_every_call_and_propagates_errors() {
    let mut state = FakeState::default();
    let inherent = timestamp_extrinsic(&state, 1700000000000);
    let remark = unsigned(
        &state.runtime,
        "System",
        "remark",
        CoreValue::record(vec![(
            "remark".into(),
            CoreValue::hex(b"bounded hosted fixture"),
        )]),
    );
    let mut encoded = vec![inherent];
    encoded.extend(std::iter::repeat_n(remark, 64));
    serve(&mut state, encoded.clone());
    let (client, chain) = setup_client(state).await;
    let info = client.block_info(Some(100)).await.unwrap().unwrap();
    assert_eq!(info.extrinsics.len(), 65);
    assert_eq!(info.extrinsics[64]["call"]["call_function"], "remark");
    encoded.push(vec![0xff]);
    chain.mutate_state(|state| serve(state, encoded));
    assert!(client.block_info(Some(100)).await.is_err());
    client.close().await;
}

#[tokio::test]
async fn header_body_cannot_disagree_with_requested_height_or_hash() {
    for field in ["number", "hash"] {
        let mut state = FakeState::default();
        let inherent = timestamp_extrinsic(&state, 0);
        let mut response = response(
            &state,
            100,
            vec![json!(format!("0x{}", hex::encode(inherent)))],
        );
        response["block"]["header"][field] = match field {
            "number" => json!("0x63"),
            _ => json!(format!("0x{}", "ab".repeat(32))),
        };
        state.set_result("chain_getBlock", response);
        let (client, _) = setup_client(state).await;
        assert!(matches!(
            client.block_info(Some(100)).await,
            Err(ClientError::Protocol(_))
        ));
        client.close().await;
    }
}

#[tokio::test]
async fn block_execution_uses_parent_runtime_during_upgrade() {
    let mut state = FakeState::default();
    let parent_hash = state.block_hash(99).unwrap();
    let block_hash = state.block_hash(100).unwrap();
    let old_call = timestamp_extrinsic(&state, 1700000000000);
    serve(&mut state, vec![old_call]);
    let parent_runtime = state.runtime.clone();
    state.set_runtime_at(parent_hash, parent_runtime);
    let mut metadata = frame_metadata::RuntimeMetadataPrefixed::decode(
        &mut state.runtime.metadata_bytes.as_slice(),
    )
    .unwrap();
    let frame_metadata::RuntimeMetadata::V15(ref mut v15) = metadata.1 else {
        panic!("expected V15")
    };
    let calls = v15
        .pallets
        .iter()
        .find(|pallet| pallet.name == "Timestamp")
        .unwrap()
        .calls
        .as_ref()
        .unwrap()
        .ty
        .id;
    let call_type = v15
        .types
        .types
        .iter_mut()
        .find(|ty| ty.id == calls)
        .unwrap();
    let scale_info::TypeDef::Variant(variants) = &mut call_type.ty.type_def else {
        panic!("call enum")
    };
    variants
        .variants
        .iter_mut()
        .find(|variant| variant.name == "set")
        .unwrap()
        .index = 200;
    state.runtime = Arc::new(Runtime::parse(&metadata.encode(), 479, 1, 42).unwrap());
    let upgraded_runtime = state.runtime.clone();
    state.set_runtime_at(block_hash, upgraded_runtime);
    let (client, chain) = setup_client(state).await;
    let start = chain.requests().len();
    let info = client.block_info(Some(100)).await.unwrap().unwrap();
    assert_eq!(info.extrinsics[0]["call"]["call_function"], "set");
    assert_eq!(info.timestamp.timestamp_millis(), 1700000000000);
    let parent = chain.mutate_state(|state| state.block_hash(99).unwrap());
    assert!(
        chain.requests()[start..]
            .iter()
            .any(|request| request.method == "state_getRuntimeVersion"
                && request.params == json!([parent]))
    );
    client.close().await;
}

#[tokio::test]
async fn timestamp_overflow_propagates_instead_of_rendering_outside_python_range() {
    let mut state = FakeState::default();
    let inherent = timestamp_extrinsic(&state, u64::MAX);
    serve(&mut state, vec![inherent]);
    let (client, _) = setup_client(state).await;
    assert!(matches!(client.block_info(Some(100)).await,
        Err(ClientError::Protocol(detail)) if detail.contains("Python datetime range")));
    client.close().await;
}

#[tokio::test]
async fn explorer_links_follow_extracted_network_templates() {
    for (network, link) in [
        ("finney", Some("https://www.tao.app/block/100")),
        ("archive", Some("https://www.tao.app/block/100")),
        ("test", None),
        ("local", None),
    ] {
        let mut state = FakeState::default();
        let inherent = timestamp_extrinsic(&state, 0);
        serve(&mut state, vec![inherent]);
        let chain = FakeChain::from_state(state);
        let mut config =
            ClientConfig::new(Network::resolve_with(network, |_| None).unwrap()).unwrap();
        config.track_heads = false;
        config.runtime_cache_dir = None;
        config.config_store = None;
        let client = Client::from_rpc(RpcClient::new(chain), config)
            .await
            .unwrap();
        assert_eq!(
            client
                .block_info(Some(100))
                .await
                .unwrap()
                .unwrap()
                .explorer_url
                .as_deref(),
            link
        );
        client.close().await;
    }
}

#[tokio::test]
async fn default_lookup_resolves_head_once_before_following_that_hash() {
    let mut state = FakeState::default();
    state
        .seed_python("Timestamp", "Now", &[], json!(0))
        .unwrap();
    state.set_handler("chain_getBlock", |state, request| {
        let hash = request.params[0].as_str().unwrap();
        let response = json!({"block": {"header": state.header(hash), "extrinsics": []}});
        state.advance(1);
        Reply::result(response)
    });
    let (client, _) = setup_client(state).await;
    let first = client.block_info(None).await.unwrap().unwrap();
    assert_eq!(first.number, 100);
    let second = client.block_info(None).await.unwrap().unwrap();
    assert_eq!(second.number, 101);
    assert_ne!(first.hash, second.hash);
    client.close().await;
}

#[tokio::test]
async fn default_lookup_honors_configured_finalized_head() {
    let mut state = FakeState::default();
    state
        .seed_python("Timestamp", "Now", &[], json!(0))
        .unwrap();
    state.set_finalized(99).unwrap();
    let chain = FakeChain::from_state(state);
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None).unwrap()).unwrap();
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    config.head_policy = rtao_client::HeadPolicy::Finalized;
    let client = Client::from_rpc(RpcClient::new(chain), config)
        .await
        .unwrap();
    assert_eq!(client.block_info(None).await.unwrap().unwrap().number, 99);
    assert_eq!(
        client.block_info(Some(100)).await.unwrap().unwrap().number,
        100
    );
    client.close().await;
}
