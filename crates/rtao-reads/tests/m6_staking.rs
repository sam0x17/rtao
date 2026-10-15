//! Staking and price reads over authenticated metadata and SCALE fake state.
#![allow(clippy::unwrap_used, clippy::panic)]

use std::collections::BTreeSet;

use rtao_client::{BlockRef, Client, ClientConfig, Network, ReadKind, View};
use rtao_core::{codec::Value as CoreValue, runtime::TypeSpec};
use rtao_primitives::{AccountId, NetUid};
use rtao_reads::{ReadName, dispatch, reads::m6_staking as reads};
use rtao_testkit::{FakeChain, FakeState, RecordingView, fakes::RpcRequest, rpc::RpcClient};
use serde_json::{Map, Value, json};

const COLD: AccountId = AccountId([7; 32]);
const HOT: AccountId = AccountId([8; 32]);
const OTHER: AccountId = AccountId([9; 32]);

async fn client(state: FakeState) -> (Client, FakeChain) {
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

fn args(api: &str, method: &str, values: &[CoreValue], state: &FakeState) -> Vec<u8> {
    let info =
        rtao_core::ops::runtime_api::runtime_api_method(&state.runtime, api, method).unwrap();
    rtao_core::ops::runtime_api::encode_params(&state.runtime, api, info, values).unwrap()
}

fn seed_availability(state: &mut FakeState) {
    let row = |total, locked, available| {
        CoreValue::record(vec![
            ("total".into(), CoreValue::Uint(total)),
            ("locked".into(), CoreValue::Uint(locked)),
            ("available".into(), CoreValue::Uint(available)),
        ])
    };
    let value = CoreValue::Dict(vec![(
        CoreValue::Str(COLD.to_string()),
        CoreValue::Dict(vec![
            (CoreValue::Int(0), row(19, 2, 17)),
            (
                CoreValue::Int(1),
                row(u128::from(u64::MAX), u128::from(u64::MAX - 3), 3),
            ),
        ]),
    )]);
    let api = "StakeInfoRuntimeApi";
    let method = "get_stake_availability_for_coldkeys";
    let info =
        rtao_core::ops::runtime_api::runtime_api_method(&state.runtime, api, method).unwrap();
    let output = state
        .runtime
        .encode_spec(&TypeSpec::Id(info.output), &value)
        .unwrap();
    for netuids in [vec![0], vec![1], vec![2], vec![0, 1, 2]] {
        for coldkey in [COLD, OTHER] {
            let input = args(
                api,
                method,
                &[
                    CoreValue::List(vec![CoreValue::Bytes(coldkey.0.to_vec())]),
                    CoreValue::List(netuids.iter().copied().map(CoreValue::Int).collect()),
                ],
                state,
            );
            state.seed_runtime_encoded(format!("{api}_{method}"), input, output.clone());
        }
    }
}

fn seed() -> FakeState {
    let mut state = FakeState::default();
    for (item, keys, value) in [
        ("TokenSymbol", vec![json!(0)], json!("root-symbol")),
        ("TokenSymbol", vec![json!(1)], json!("Ω")),
        (
            "StakingHotkeys",
            vec![json!(COLD)],
            json!([OTHER, HOT, OTHER]),
        ),
        (
            "AutoStakeDestination",
            vec![json!(COLD), json!(256)],
            json!(OTHER),
        ),
        (
            "AutoStakeDestination",
            vec![json!(COLD), json!(1)],
            json!(HOT),
        ),
        (
            "AutoStakeDestination",
            vec![json!(COLD), json!(0)],
            json!(OTHER),
        ),
        (
            "AutoStakeDestination",
            vec![json!(OTHER), json!(2)],
            json!(COLD),
        ),
        ("TotalAlphaStaked", vec![json!(0)], json!(u64::MAX)),
        ("TotalAlphaStaked", vec![json!(1)], json!(42)),
    ] {
        state
            .seed_python("SubtensorModule", item, &keys, value)
            .unwrap();
    }
    let alpha = json!({"hotkey": HOT, "coldkey": COLD, "netuid": 1, "stake": u64::MAX, "is_registered": true});
    let root =
        json!({"hotkey": OTHER, "coldkey": COLD, "netuid": 0, "stake": 17, "is_registered": false});
    for (api, method, value) in [
        (
            "StakeInfoRuntimeApi",
            "get_stake_info_for_hotkey_coldkey_netuid",
            alpha.clone(),
        ),
        (
            "StakeInfoRuntimeApi",
            "get_stake_info_for_coldkey",
            json!([alpha, root.clone(), alpha]),
        ),
        (
            "StakeInfoRuntimeApi",
            "get_stake_info_for_coldkeys",
            json!([[COLD, [root.clone(), alpha]], [HOT, []], [COLD, [root]]]),
        ),
        ("SwapRuntimeApi", "current_alpha_price", json!(u64::MAX)),
        (
            "SwapRuntimeApi",
            "current_alpha_price_all",
            json!([
                {"netuid": 1, "price": 123456789},
                {"netuid": 0, "price": 1000000000},
                {"netuid": 2, "price": u64::MAX},
                {"netuid": 1, "price": 500000001}
            ]),
        ),
        (
            "SwapRuntimeApi",
            "sim_swap_tao_for_alpha",
            json!({"tao_amount": 1234567890, "alpha_amount": 2345678901u64, "tao_fee": 3, "alpha_fee": 5, "tao_slippage": 7, "alpha_slippage": 11}),
        ),
        (
            "SwapRuntimeApi",
            "sim_swap_alpha_for_tao",
            json!({"tao_amount": 3456789012u64, "alpha_amount": 4567890123u64, "tao_fee": 13, "alpha_fee": 17, "tao_slippage": 19, "alpha_slippage": 23}),
        ),
    ] {
        state.seed_runtime(api, method, value).unwrap();
    }
    seed_availability(&mut state);
    state
}

fn params(value: Value) -> Map<String, Value> {
    value.as_object().unwrap().clone()
}

fn check_observations<V: View>(name: ReadName, view: &RecordingView<V>, requests: &[RpcRequest]) {
    let touches = view.touches();
    assert!(!touches.is_empty(), "{name}");
    let observed: BTreeSet<_> = touches
        .iter()
        .map(|touch| {
            assert!(touch.at.is_some(), "{name}");
            assert!(
                name.spec().hits.iter().any(|hit| hit.matches(touch)),
                "{name}: {touch:?}"
            );
            (touch.kind, touch.pallet_or_api.clone(), touch.item.clone())
        })
        .collect();
    assert_eq!(observed.len(), 1, "{name}");
    assert_eq!(observed.len(), name.spec().hits.len(), "{name}");
    let hashes = view.block_hashes();
    assert_eq!(hashes.len(), 1, "{name}");
    let hash = hashes.first().unwrap();
    let reads: Vec<_> = requests
        .iter()
        .filter(|request| {
            [
                "state_getStorage",
                "state_queryStorageAt",
                "state_call",
                "state_getKeysPaged",
            ]
            .contains(&request.method.as_str())
        })
        .collect();
    assert!(!reads.is_empty(), "{name}");
    for request in reads {
        assert_eq!(
            request.params.as_array().unwrap().last(),
            Some(&json!(format!("0x{}", hex::encode(hash.0)))),
            "{name}: {request:?}"
        );
    }
    assert!(
        touches
            .iter()
            .all(|touch| { matches!(touch.kind, ReadKind::Storage | ReadKind::RuntimeApi) })
    );
}

#[tokio::test]
async fn all_thirteen_reads_have_exact_descriptor_hits_and_one_head_or_pinned_hash() {
    let (client, chain) = client(seed()).await;
    for (name, arguments) in [
        (
            ReadName::Stake,
            json!({"coldkey_ss58": COLD, "hotkey_ss58": HOT, "netuid": 1}),
        ),
        (ReadName::StakeForColdkey, json!({"coldkey_ss58": COLD})),
        (
            ReadName::StakeForColdkeys,
            json!({"coldkey_ss58s": [COLD, OTHER]}),
        ),
        (
            ReadName::StakeAvailability,
            json!({"coldkey_ss58": COLD, "netuid": 1}),
        ),
        (
            ReadName::StakeAvailabilityForColdkey,
            json!({"coldkey_ss58": COLD, "netuids": [2, 0, 1, 2]}),
        ),
        (ReadName::TotalAlphaStaked, json!({"netuid": 1})),
        (ReadName::StakingHotkeys, json!({"coldkey_ss58": COLD})),
        (
            ReadName::AutoStake,
            json!({"coldkey_ss58": COLD, "netuid": 1}),
        ),
        (ReadName::AutoStakeAll, json!({"coldkey_ss58": COLD})),
        (ReadName::AlphaPrice, json!({"netuid": 1})),
        (ReadName::AlphaPrices, json!({})),
        (
            ReadName::QuoteStake,
            json!({"netuid": 1, "amount_tao": 1.0000000015}),
        ),
        (
            ReadName::QuoteUnstake,
            json!({"netuid": 1, "amount_alpha": 2.0}),
        ),
    ] {
        let view = RecordingView::new(client.clone());
        let before = chain.requests().len();
        dispatch(&view, name, &params(arguments.clone()))
            .await
            .unwrap();
        check_observations(name, &view, &chain.requests()[before..]);
        for block in [99, 100] {
            let snapshot = client.at(BlockRef::Number(block)).await.unwrap();
            let hash = snapshot.block().hash;
            let view = RecordingView::new(snapshot);
            let before = chain.requests().len();
            dispatch(&view, name, &params(arguments.clone()))
                .await
                .unwrap();
            check_observations(name, &view, &chain.requests()[before..]);
            assert_eq!(view.block_hashes(), BTreeSet::from([hash]));
        }
    }
    client.close().await;
}

#[tokio::test]
async fn positions_keep_runtime_order_exact_native_amounts_and_only_reported_coldkeys() {
    let (client, chain) = client(seed()).await;
    let positions = reads::stake_for_coldkey(&client, COLD).await.unwrap();
    assert_eq!(
        positions.iter().map(|row| row.netuid).collect::<Vec<_>>(),
        [NetUid(1), NetUid::ROOT, NetUid(1)]
    );
    assert_eq!(
        positions.iter().map(|row| row.hotkey).collect::<Vec<_>>(),
        [HOT, OTHER, HOT]
    );
    assert_eq!(positions[0].stake.rao, u64::MAX);
    assert_eq!(positions[0].stake.symbol.as_ref().unwrap().as_str(), "Ω");
    assert_eq!(positions[1].stake.rao, 17);
    assert_eq!(positions[1].stake.netuid, NetUid::ROOT);
    assert_eq!(
        positions[1].stake.symbol.as_ref().unwrap().as_str(),
        "root-symbol"
    );
    assert!(!positions[1].is_registered);
    let by_coldkey = reads::stake_for_coldkeys(&client, vec![COLD, OTHER, COLD])
        .await
        .unwrap();
    assert_eq!(by_coldkey.len(), 2);
    assert_eq!(by_coldkey[&COLD].len(), 1); // Last runtime record wins, rather than input reshaping.
    assert_eq!(by_coldkey[&COLD][0].stake.rao, 17);
    assert!(by_coldkey[&HOT].is_empty());
    assert!(!by_coldkey.contains_key(&OTHER));
    let amount = reads::stake(&client, COLD, HOT, NetUid(1)).await.unwrap();
    assert_eq!((amount.rao, amount.netuid), (u64::MAX, NetUid(1)));
    chain.mutate_state(|state| {
        state
            .seed_runtime(
                "StakeInfoRuntimeApi",
                "get_stake_info_for_hotkey_coldkey_netuid",
                Value::Null,
            )
            .unwrap();
        state
            .seed_runtime(
                "StakeInfoRuntimeApi",
                "get_stake_info_for_coldkey",
                json!([]),
            )
            .unwrap();
        state
            .seed_runtime(
                "StakeInfoRuntimeApi",
                "get_stake_info_for_coldkeys",
                json!([]),
            )
            .unwrap();
    });
    let root = reads::stake(&client, COLD, HOT, NetUid::ROOT)
        .await
        .unwrap();
    assert_eq!((root.rao, root.netuid), (0, NetUid::ROOT));
    assert_eq!(root.symbol.as_ref().unwrap().as_str(), "root-symbol");
    assert!(
        reads::stake_for_coldkey(&client, COLD)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        reads::stake_for_coldkeys(&client, vec![COLD])
            .await
            .unwrap()
            .is_empty()
    );
    client.close().await;
}

#[tokio::test]
async fn availability_sorts_deduplicates_and_fills_only_requested_missing_rows() {
    let (client, chain) = client(seed()).await;
    let rows = reads::stake_availability_for_coldkey(
        &client,
        COLD,
        vec![NetUid(2), NetUid(1), NetUid::ROOT, NetUid(1)],
    )
    .await
    .unwrap();
    assert_eq!(
        rows.iter().map(|row| row.netuid).collect::<Vec<_>>(),
        [NetUid::ROOT, NetUid(1), NetUid(2)]
    );
    assert_eq!(
        (rows[0].total.rao, rows[0].locked.rao, rows[0].available.rao),
        (19, 2, 17)
    );
    assert_eq!(
        (rows[1].total.rao, rows[1].locked.rao, rows[1].available.rao),
        (u64::MAX, u64::MAX - 3, 3)
    );
    assert_eq!(
        (rows[2].total.rao, rows[2].locked.rao, rows[2].available.rao),
        (0, 0, 0)
    );
    assert_eq!(rows[0].available.netuid, NetUid::ROOT);
    assert_eq!(rows[1].available.netuid, NetUid(1));
    assert_eq!(rows[1].total.symbol.as_ref().unwrap().as_str(), "Ω");
    let missing_cold = reads::stake_availability(&client, OTHER, NetUid(1))
        .await
        .unwrap();
    assert_eq!(
        (
            missing_cold.total.rao,
            missing_cold.locked.rao,
            missing_cold.available.rao
        ),
        (0, 0, 0)
    );
    let missing_netuid = reads::stake_availability(&client, COLD, NetUid(2))
        .await
        .unwrap();
    assert_eq!(missing_netuid.available.rao, 0);
    let before = chain.requests().len();
    assert!(
        reads::stake_availability_for_coldkey(&client, COLD, Vec::new())
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(chain.requests().len(), before);
    client.close().await;
}

#[tokio::test]
async fn destination_scan_is_coldkey_scoped_and_numerically_sorted_with_native_storage_defaults() {
    let (client, _) = client(seed()).await;
    let destinations = reads::auto_stake_all(&client, COLD).await.unwrap();
    assert_eq!(
        destinations
            .iter()
            .map(|row| (row.netuid.0, row.hotkey))
            .collect::<Vec<_>>(),
        [(0, OTHER), (1, HOT), (256, OTHER)]
    );
    assert_eq!(
        reads::auto_stake(&client, COLD, NetUid(1)).await.unwrap(),
        Some(HOT)
    );
    assert_eq!(
        reads::auto_stake(&client, COLD, NetUid(2)).await.unwrap(),
        None
    );
    assert!(
        reads::auto_stake_all(&client, HOT)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        reads::staking_hotkeys(&client, COLD).await.unwrap(),
        [OTHER, HOT, OTHER]
    );
    assert!(
        reads::staking_hotkeys(&client, HOT)
            .await
            .unwrap()
            .is_empty()
    );
    let root = reads::total_alpha_staked(&client, NetUid::ROOT)
        .await
        .unwrap();
    assert_eq!((root.rao, root.netuid), (u64::MAX, NetUid::ROOT));
    assert_eq!(root.symbol.as_ref().unwrap().as_str(), "root-symbol");
    assert_eq!(
        reads::total_alpha_staked(&client, NetUid(1))
            .await
            .unwrap()
            .rao,
        42
    );
    let missing = reads::total_alpha_staked(&client, NetUid(2)).await.unwrap();
    assert_eq!((missing.rao, missing.netuid), (0, NetUid(2)));
    client.close().await;
}

#[tokio::test]
async fn prices_keep_native_rao_and_python_integer_division_with_last_duplicate_winning() {
    let (client, chain) = client(seed()).await;
    let price = reads::alpha_price(&client, NetUid::ROOT).await.unwrap();
    assert_eq!((price.netuid, price.price_rao), (NetUid::ROOT, u64::MAX));
    assert_eq!(price.tao_per_alpha.to_bits(), 0x4211_2e0b_e826_d695);
    let prices = reads::alpha_prices(&client).await.unwrap();
    assert_eq!(prices.len(), 3);
    assert_eq!(prices[&NetUid::ROOT], 1.0);
    assert_eq!(prices[&NetUid(1)], 0.500000001);
    assert_eq!(prices[&NetUid(2)].to_bits(), price.tao_per_alpha.to_bits());
    chain.mutate_state(|state| {
        state
            .seed_runtime(
                "SwapRuntimeApi",
                "current_alpha_price",
                json!(9_007_199_254_740_995u64),
            )
            .unwrap();
    });
    assert_eq!(
        reads::alpha_price(&client, NetUid(1))
            .await
            .unwrap()
            .tao_per_alpha
            .to_bits(),
        0x4161_2e0b_e826_d696 // Independent rational regression; f64 numerator conversion rounds one ULP higher.
    );
    chain.mutate_state(|state| {
        state
            .seed_runtime("SwapRuntimeApi", "current_alpha_price", json!(0))
            .unwrap();
        state
            .seed_runtime("SwapRuntimeApi", "current_alpha_price_all", json!([]))
            .unwrap();
    });
    assert_eq!(
        reads::alpha_price(&client, NetUid(1))
            .await
            .unwrap()
            .tao_per_alpha,
        0.0
    );
    assert!(reads::alpha_prices(&client).await.unwrap().is_empty());
    client.close().await;
}

#[tokio::test]
async fn quote_stake_slippage_and_fee_preserve_exact_fields_currencies_and_decimal_input_rounding()
{
    let (client, chain) = client(seed()).await;
    let before = chain.requests().len();
    let quote = reads::quote_stake(&client, NetUid(1), 1.0000000015)
        .await
        .unwrap();
    assert_eq!(
        (quote.tao.rao, quote.alpha.rao),
        (1_234_567_890, 2_345_678_901)
    );
    assert_eq!((quote.tao_fee.rao, quote.alpha_fee.rao), (3, 5));
    assert_eq!((quote.tao_slippage.rao, quote.alpha_slippage.rao), (7, 11));
    for amount in [&quote.tao, &quote.tao_fee, &quote.tao_slippage] {
        assert_eq!(amount.netuid, NetUid::ROOT);
        assert!(amount.symbol.is_none());
    }
    for amount in [&quote.alpha, &quote.alpha_fee, &quote.alpha_slippage] {
        assert_eq!(amount.netuid, NetUid(1));
        assert_eq!(amount.symbol.as_ref().unwrap().as_str(), "Ω");
    }
    let requests = chain.requests();
    let request = requests[before..]
        .iter()
        .find(|request| {
            request.method == "state_call"
                && request.params[0] == "SwapRuntimeApi_sim_swap_tao_for_alpha"
        })
        .unwrap();
    let expected = args(
        "SwapRuntimeApi",
        "sim_swap_tao_for_alpha",
        &[CoreValue::Int(1), CoreValue::Int(1_000_000_002)],
        &seed(),
    );
    assert_eq!(
        request.params[1],
        json!(format!("0x{}", hex::encode(expected)))
    );
    let quote = reads::quote_unstake(&client, NetUid::ROOT, 0.0000000025)
        .await
        .unwrap();
    assert_eq!(
        (quote.tao.rao, quote.alpha.rao),
        (3_456_789_012, 4_567_890_123)
    );
    assert_eq!((quote.tao_fee.rao, quote.alpha_fee.rao), (13, 17));
    assert_eq!((quote.tao_slippage.rao, quote.alpha_slippage.rao), (19, 23));
    assert_eq!(quote.alpha.netuid, NetUid::ROOT);
    assert_eq!(quote.alpha.symbol.as_ref().unwrap().as_str(), "root-symbol");
    assert!(quote.tao.symbol.is_none());
    let request = chain
        .requests()
        .into_iter()
        .rev()
        .find(|request| {
            request.method == "state_call"
                && request.params[0] == "SwapRuntimeApi_sim_swap_alpha_for_tao"
        })
        .unwrap();
    let expected = args(
        "SwapRuntimeApi",
        "sim_swap_alpha_for_tao",
        &[CoreValue::Int(0), CoreValue::Int(2)],
        &seed(),
    );
    assert_eq!(
        request.params[1],
        json!(format!("0x{}", hex::encode(expected)))
    );
    client.close().await;
}

#[tokio::test]
async fn invalid_quote_amounts_fail_before_any_runtime_or_head_request() {
    let (client, chain) = client(seed()).await;
    let before = chain.requests().len();
    for amount in [
        -1.0,
        -f64::MIN_POSITIVE,
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::MAX,
        18_446_744_073.709_553,
    ] {
        assert!(matches!(
            reads::quote_stake(&client, NetUid(1), amount).await,
            Err(rtao_reads::ReadError::InvalidParam { .. })
        ));
        assert!(matches!(
            reads::quote_unstake(&client, NetUid::ROOT, amount).await,
            Err(rtao_reads::ReadError::InvalidParam { .. })
        ));
    }
    assert_eq!(chain.requests().len(), before);
    client.close().await;
}

#[tokio::test]
async fn truncated_scale_fails_instead_of_inventing_zero_stake_or_swap_values() {
    let mut state = seed();
    let info = state
        .runtime
        .storage_entry("SubtensorModule", "TotalAlphaStaked")
        .unwrap();
    let key = state
        .runtime
        .storage_key(info, &[CoreValue::Int(1)])
        .unwrap();
    state.seed_encoded(key, Some(vec![0; 7]));
    let (client, chain) = client(state).await;
    assert!(reads::total_alpha_staked(&client, NetUid(1)).await.is_err());
    chain.mutate_state(|state| {
        state.queue_reply(
            "state_call",
            rtao_testkit::fakes::Reply::result(json!("0x00")),
        );
    });
    assert!(reads::quote_stake(&client, NetUid(1), 1.0).await.is_err());
    client.close().await;
}
