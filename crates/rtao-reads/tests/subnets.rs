// Derived from RaoFoundation/subtensor sdk/python/bittensor/reads/subnets.py @ 6a972693ae9580541225bd37696ac2fee26b8ae9 (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/tests/unit/test_emission_weights.py @ 6a972693ae9580541225bd37696ac2fee26b8ae9 (Apache-2.0); modified by rtao.
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

// pyt: unit/test_emission_weights.py::test_read_decodes_runtime_payload
#[tokio::test]
async fn subnet_emission_weights_decodes_the_runtime_payload_and_live_root_inputs() {
    use rtao_primitives::emission_weights::{self as ew, DEFAULT_OWNER, DEFAULT_VALIDATOR, ONE};
    use rtao_runtime::storage::{subtensor_module as st, swap};
    use rtao_testkit::fixtures::catalog_samples::{
        EMISSION_ALPHA_ISSUANCE, EMISSION_ROOT_TAO, EMISSION_TAO_WEIGHT,
        emission_root_baseline_share, emission_weights_info,
    };
    let netuid = NetUid(1);
    let mut state = FakeState::default();
    state
        .seed_runtime(
            "SubnetInfoRuntimeApi",
            "get_subnet_emission_weights",
            emission_weights_info(netuid),
        )
        .unwrap();
    state
        .seed(&st::TAO_WEIGHT, (), EMISSION_TAO_WEIGHT)
        .unwrap();
    state
        .seed(
            &st::SUBNET_TAO,
            (NetUid::ROOT,),
            rtao_primitives::TaoBalance(EMISSION_ROOT_TAO),
        )
        .unwrap();
    state
        .seed(
            &st::SUBNET_ALPHA_IN,
            (netuid,),
            rtao_primitives::AlphaBalance(0),
        )
        .unwrap();
    state
        .seed(
            &st::SUBNET_ALPHA_OUT,
            (netuid,),
            rtao_primitives::AlphaBalance(EMISSION_ALPHA_ISSUANCE),
        )
        .unwrap();
    state
        .seed(
            &swap::BALANCER_ALPHA_RESERVOIR,
            (netuid,),
            rtao_primitives::AlphaBalance(0),
        )
        .unwrap();
    let chain = FakeChain::from_state(state);
    let client = Client::from_rpc(RpcClient::new(chain.clone()), config())
        .await
        .unwrap();
    let info = subnets::subnet_emission_weights(&client, netuid)
        .await
        .unwrap()
        .unwrap();
    let baseline_parts = emission_root_baseline_share();
    let baseline = ew::share_to_float(baseline_parts);
    let miner = ONE - DEFAULT_OWNER - DEFAULT_VALIDATOR;
    assert_eq!(
        info.weights,
        [0, 0, DEFAULT_OWNER, DEFAULT_VALIDATOR, miner]
    );
    assert!((info.shares.owner - 0.18).abs() < 1e-4);
    assert!((info.global_tao_weight - 0.0527).abs() < 1e-4);
    assert_eq!(
        (info.root_tao, info.alpha_issuance),
        (EMISSION_ROOT_TAO, u128::from(EMISSION_ALPHA_ISSUANCE))
    );
    assert!((info.root_share - 0.0174).abs() < 1e-4);
    assert_eq!(info.root_share, baseline);
    assert_eq!(info.baseline_root_share, baseline);
    assert_eq!(info.baseline_root_parts, baseline_parts);
    assert_eq!(
        info.validator_share_after_root,
        ew::share_to_float(DEFAULT_VALIDATOR - baseline_parts)
    );
    assert!(!info.root_capped);
    assert_eq!(info.min_validator, baseline_parts + 1);
    assert_eq!(info.rules().min_validator, Some(baseline_parts + 1));
    assert!(!info.rules().created_via_lease);
    assert_eq!((info.block, info.last_change_block), (100, 0));
    assert!(info.rate_limit().owner_can_change_now);
    // The pure baseline formula agrees with the derived runtime payload.
    let pure = ew::baseline_root_share(
        info.global_tao_weight,
        EMISSION_ROOT_TAO as f64,
        EMISSION_ALPHA_ISSUANCE as f64,
    );
    assert!((pure - baseline).abs() <= baseline * 1e-9);
    // The record serializes Python's dataclass field order and names.
    let record = serde_json::to_value(&info).unwrap();
    assert_eq!(record["min_validator"], json!(baseline_parts + 1));
    assert_eq!(record["shares"]["validator"], json!(info.shares.validator));
    assert!(chain.requests().iter().any(|request| {
        request.method == "state_call"
            && request.params[0] == json!("SubnetInfoRuntimeApi_get_subnet_emission_weights")
    }));
    // A recorded owner change moves the next allowed block.
    let key = subnets::emission_weights_rate_limit_key(netuid);
    chain.mutate_state(|state| {
        state
            .seed(&st::LAST_RATE_LIMITED_BLOCK, (key,), 90u64)
            .unwrap();
    });
    let info = subnets::subnet_emission_weights(&client, netuid)
        .await
        .unwrap()
        .unwrap();
    let rate = info.rate_limit();
    assert_eq!(
        (
            rate.last_change_block,
            rate.next_allowed_block,
            rate.blocks_until_allowed
        ),
        (
            90,
            90 + ew::OWNER_RATE_LIMIT_BLOCKS,
            90 + ew::OWNER_RATE_LIMIT_BLOCKS - 100
        )
    );
    assert!(!rate.owner_can_change_now);
    // The root subnet and missing subnets have no vector.
    chain.mutate_state(|state| {
        state
            .seed_runtime(
                "SubnetInfoRuntimeApi",
                "get_subnet_emission_weights",
                json!(null),
            )
            .unwrap()
    });
    assert_eq!(
        subnets::subnet_emission_weights(&client, NetUid(9))
            .await
            .unwrap(),
        None
    );
    client.close().await;
}

#[test]
fn emission_weights_runtime_record_refuses_changed_shapes() {
    use rtao_core::{DynValue, codec::Value};
    let record = |weights: Vec<Value>| {
        DynValue::new(Value::record(vec![
            ("netuid".into(), Value::Uint(1)),
            ("weights".into(), Value::List(weights)),
            ("burn".into(), Value::Uint(0)),
            ("recycle".into(), Value::Uint(0)),
            ("owner".into(), Value::Uint(1)),
            ("validator".into(), Value::Uint(2)),
            ("miner".into(), Value::Uint(3)),
            ("root_baseline_share".into(), Value::Uint(4)),
            ("root_share".into(), Value::Uint(2)),
            ("created_via_lease".into(), Value::Bool(true)),
        ]))
    };
    let info = subnets::SubnetEmissionWeightsInfo::decode(&record(vec![
        Value::Uint(0),
        Value::Uint(0),
        Value::Uint(1),
    ]))
    .unwrap();
    assert_eq!(info.weights, [0, 0, 1, 0, 0]);
    assert!(info.created_via_lease);
    assert!(subnets::SubnetEmissionWeightsInfo::decode(&record(vec![Value::Uint(0); 6])).is_err());
    assert!(
        subnets::SubnetEmissionWeightsInfo::decode(&record(vec![Value::Str("x".into())])).is_err()
    );
}
