//! Basket, subnet and mechanism reads through real metadata and SCALE fixtures.
#![allow(clippy::unwrap_used, clippy::panic)] // Authenticated fixtures must fail assertions when invalid.

use std::collections::BTreeSet;

use rtao_client::{BlockRef, Client, ClientConfig, Network};
use rtao_core::runtime::TypeSpec;
use rtao_primitives::{AccountId, MechId, NetUid, Uid};
use rtao_reads::{ReadName, dispatch, reads::m6_root as reads};
use rtao_testkit::{
    FakeChain, FakeState, RecordingView, Reply, fakes::fill_defaults, rpc::RpcClient,
};
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

fn position(hotkey: AccountId, beta: u64, total: u64, value: u64) -> Value {
    json!({"hotkey": hotkey, "beta": beta, "beta_total": total, "nav_tao": 23, "value_tao": value, "spot_value_tao": 29})
}

fn summary(hotkey: AccountId, nav: u64) -> Value {
    json!({"hotkey": hotkey, "nav_tao": nav, "spot_nav_tao": 19, "shares": 3, "deposited_tao": 5, "redeemed_tao": 2,
        "holdings": [{"netuid": 1, "alpha": 7, "spot_tao": 11, "realizable_tao": 13}, {"netuid": 0, "alpha": 17, "spot_tao": 17, "realizable_tao": 17}]})
}

fn pricing(hotkey: AccountId, price: u128) -> Value {
    json!({"hotkey": hotkey, "spot_price": {"bits": price}, "display_price": {"bits": price}, "stake_price": {"bits": 0},
        "staker_yield": {"bits": 0}, "staker_twr": {"bits": 0}, "bag_index": {"bits": 0}, "stake_index": {"bits": 0},
        "first_block": 3, "provisional": true, "display_shares": {"bits": 18446744073709551616u128}})
}

fn seed() -> FakeState {
    let mut state = FakeState::default();
    for (name, keys, value) in [
        ("NetworksAdded", vec![json!(2)], json!(true)),
        ("NetworksAdded", vec![json!(1)], json!(true)),
        ("NetworksAdded", vec![json!(3)], json!(false)),
        ("Tempo", vec![json!(1)], json!(u16::MAX)),
        ("Burn", vec![json!(1)], json!(u64::MAX)),
        ("SubnetworkN", vec![json!(1)], json!(u16::MAX)),
        ("Tempo", vec![json!(3)], json!(9)),
        ("Burn", vec![json!(3)], json!(9)),
        ("SubnetworkN", vec![json!(3)], json!(9)),
        (
            "SubnetEmaTaoFlow",
            vec![json!(1)],
            json!([100, {"bits": -9223372036854775808i128}]),
        ),
        (
            "SubnetEmaTaoFlow",
            vec![json!(2)],
            json!([90, {"bits": i128::MAX}]),
        ),
        (
            "Bonds",
            vec![json!(4097), json!(3)],
            json!([[2, 32768], [1, 65535], [2, 1]]),
        ),
        ("Bonds", vec![json!(4097), json!(1)], json!([])),
        ("Bonds", vec![json!(1), json!(8)], json!([[1, 65535]])),
        (
            "TimelockedWeightCommits",
            vec![json!(4097), json!(9)],
            json!([[HOT, u64::MAX, "0xff0001", 0], [OTHER, 90, "0x", 1]]),
        ),
        (
            "TimelockedWeightCommits",
            vec![json!(4097), json!(2)],
            json!([]),
        ),
        (
            "TimelockedWeightCommits",
            vec![json!(1), json!(1)],
            json!([[OTHER, 1, "0xaa", 1]]),
        ),
        ("BasketRate", vec![json!(HOT)], json!({"bits": -1})),
        (
            "BasketRate",
            vec![json!(OTHER)],
            json!({"bits": 6442450944i64}),
        ),
        ("TokenSymbol", vec![json!(1)], json!("Ω")),
    ] {
        state
            .seed_python("SubtensorModule", name, &keys, value)
            .unwrap();
    }
    for (api, method, value) in [
        (
            "BetaBasketRuntimeApi",
            "get_basket_position",
            position(HOT, u64::MAX, u64::MAX, u64::MAX),
        ),
        (
            "BetaBasketRuntimeApi",
            "get_root_basket_portfolio",
            json!([
                position(HOT, 2, 3, 5),
                position(OTHER, 3, 0, 7),
                position(COLD, 0, 0, 7)
            ]),
        ),
        (
            "BetaBasketRuntimeApi",
            "get_root_basket_positions",
            json!([[HOT, u64::MAX, 3], [OTHER, 4, 9], [COLD, 2, 9]]),
        ),
        (
            "BetaBasketRuntimeApi",
            "get_validator_basket",
            json!([[1, u64::MAX, 5], [0, 17, 17]]),
        ),
        (
            "BetaBasketRuntimeApi",
            "get_validator_basket_nav",
            json!(u64::MAX),
        ),
        (
            "BetaBasketRuntimeApi",
            "get_root_basket_total_nav",
            json!(u64::MAX),
        ),
        (
            "BetaBasketRuntimeApi",
            "get_all_validator_baskets",
            json!([summary(HOT, 7), summary(OTHER, 11)]),
        ),
        (
            "BetaBasketRuntimeApi",
            "get_all_beta_pricing",
            json!({"pricing": [pricing(HOT, 18446744073709551616u128)], "next": null}),
        ),
        (
            "BetaBasketRuntimeApi",
            "get_basket_trading_status",
            json!({"enabled": true, "frozen": true, "refill_blocks": 7200, "tao_available": 2, "budget_tao": u64::MAX}),
        ),
        (
            "SubnetRegistrationRuntimeApi",
            "get_network_registration_cost",
            json!(u64::MAX),
        ),
    ] {
        state.seed_runtime(api, method, value).unwrap();
    }
    state
}

fn params(value: Value) -> Map<String, Value> {
    value.as_object().unwrap().clone()
}

#[tokio::test]
async fn all_fourteen_reads_have_exact_descriptor_hits_and_one_head_or_pinned_state() {
    let (client, chain) = client(seed()).await;
    for (name, args) in [
        (
            ReadName::BasketPosition,
            json!({"hotkey_ss58": HOT, "coldkey_ss58": COLD}),
        ),
        (ReadName::BasketTradingStatus, json!({"hotkey_ss58": HOT})),
        (
            ReadName::RootBasketOwedBreakdown,
            json!({"coldkey_ss58": COLD}),
        ),
        (ReadName::RootBasketPortfolio, json!({"coldkey_ss58": COLD})),
        (ReadName::RootBasketTotalNav, json!({})),
        (ReadName::RootBaskets, json!({})),
        (ReadName::ValidatorBasket, json!({"hotkey_ss58": HOT})),
        (ReadName::ValidatorBasketNav, json!({"hotkey_ss58": HOT})),
        (ReadName::Subnet, json!({"netuid": 1})),
        (ReadName::Subnets, json!({})),
        (ReadName::SubnetTaoFlows, json!({})),
        (ReadName::SubnetRegistrationCost, json!({})),
        (ReadName::Bonds, json!({"netuid": 1, "mechid": 1})),
        (
            ReadName::TimelockedWeightCommits,
            json!({"netuid": 1, "mechid": 1}),
        ),
    ] {
        let mut observed = BTreeSet::new();
        for block in [None, Some(99)] {
            let before = chain.requests().len();
            let touches = if let Some(block) = block {
                let view = RecordingView::new(client.at(BlockRef::Number(block)).await.unwrap());
                dispatch(&view, name, &params(args.clone())).await.unwrap();
                assert_eq!(view.block_hashes().len(), 1, "{name}");
                view.touches()
            } else {
                let view = RecordingView::new(client.clone());
                dispatch(&view, name, &params(args.clone())).await.unwrap();
                assert_eq!(view.block_hashes().len(), 1, "{name}");
                view.touches()
            };
            let hashes: BTreeSet<_> = touches.iter().map(|touch| touch.at.unwrap()).collect();
            assert_eq!(hashes.len(), 1, "{name}");
            let hash = hashes.first().unwrap();
            for touch in touches {
                assert!(
                    name.spec().hits.iter().any(|hit| hit.matches(&touch)),
                    "{name}: unexpected {touch:?}"
                );
                observed.insert((touch.kind, touch.pallet_or_api, touch.item));
            }
            for request in &chain.requests()[before..] {
                if [
                    "state_getStorage",
                    "state_queryStorageAt",
                    "state_call",
                    "state_getKeysPaged",
                ]
                .contains(&request.method.as_str())
                {
                    assert_eq!(
                        request.params.as_array().unwrap().last(),
                        Some(&json!(format!("0x{}", hex::encode(hash.0)))),
                        "{name} {request:?}"
                    );
                }
            }
        }
        assert_eq!(
            observed.len(),
            name.spec().hits.len(),
            "{name}: complete descriptor coverage"
        );
    }
    client.close().await;
}

#[tokio::test]
async fn basket_positions_keep_native_beta_and_stable_value_order_with_zero_supply() {
    let (client, chain) = client(seed()).await;
    let single = reads::basket_position(&client, HOT, COLD)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(single.beta_raw, u64::MAX);
    assert_eq!(single.beta_total_raw, u64::MAX);
    assert_eq!(single.beta, rtao_primitives::pyfmt::rao_to_f64(u64::MAX));
    assert_eq!(single.fund_fraction, 1.0);
    assert_eq!(single.value_tao.rao, u64::MAX);
    assert_eq!(single.value_tao.netuid, NetUid::ROOT);
    let input = chain
        .requests()
        .into_iter()
        .find(|request| {
            request.method == "state_call"
                && request.params[0] == json!("BetaBasketRuntimeApi_get_basket_position")
        })
        .unwrap();
    assert_eq!(
        input.params[1],
        json!(format!("0x{}{}", hex::encode(HOT.0), hex::encode(COLD.0)))
    );
    let rows = reads::root_basket_portfolio(&client, COLD).await.unwrap();
    assert_eq!(
        rows.iter().map(|row| row.hotkey).collect::<Vec<_>>(),
        vec![OTHER, COLD, HOT]
    );
    assert_eq!(rows[0].fund_fraction, 0.0);
    assert_eq!(rows[0].beta_price_tao, 0.0);
    assert_eq!(rows[2].fund_fraction, 2.0 / 3.0);
    assert_eq!(rows[2].beta_price_tao, 23.0 / 3.0);
    assert_eq!(rows[2].spot_value_tao.rao, 29);
    chain.mutate_state(|state| {
        state
            .seed_runtime("BetaBasketRuntimeApi", "get_basket_position", Value::Null)
            .unwrap()
    });
    assert!(
        reads::basket_position(&client, HOT, COLD)
            .await
            .unwrap()
            .is_none()
    );
    client.close().await;
}

#[tokio::test]
async fn basket_payouts_and_holdings_preserve_chain_values_and_currencies() {
    let (client, chain) = client(seed()).await;
    let rows = reads::root_basket_owed_breakdown(&client, COLD)
        .await
        .unwrap();
    assert_eq!(
        rows.iter()
            .map(|row| (row.hotkey, row.owed_tao.rao))
            .collect::<Vec<_>>(),
        vec![(OTHER, 9), (COLD, 9), (HOT, 3)]
    );
    let holdings = reads::validator_basket(&client, HOT).await.unwrap();
    assert_eq!(
        holdings.iter().map(|row| row.netuid).collect::<Vec<_>>(),
        vec![NetUid(1), NetUid::ROOT]
    );
    assert_eq!(holdings[0].alpha.rao, u64::MAX);
    assert_eq!(holdings[0].alpha.netuid, NetUid(1));
    assert_eq!(
        holdings[0]
            .alpha
            .symbol
            .as_ref()
            .map(|symbol| symbol.as_str()),
        Some("Ω")
    );
    assert_eq!(holdings[0].value_tao.netuid, NetUid::ROOT);
    assert_eq!(holdings[1].alpha.rao, holdings[1].value_tao.rao);
    assert_eq!(
        reads::validator_basket_nav(&client, HOT).await.unwrap().rao,
        u64::MAX
    );
    assert_eq!(
        reads::root_basket_total_nav(&client).await.unwrap().rao,
        u64::MAX
    );
    chain.mutate_state(|state| {
        state
            .seed_runtime(
                "BetaBasketRuntimeApi",
                "get_root_basket_positions",
                json!([[HOT, 1, 0]]),
            )
            .unwrap();
        state
            .seed_runtime("BetaBasketRuntimeApi", "get_validator_basket", json!([]))
            .unwrap();
    });
    assert_eq!(
        reads::root_basket_owed_breakdown(&client, COLD)
            .await
            .unwrap()[0]
            .owed_tao
            .rao,
        0
    );
    assert!(
        reads::validator_basket(&client, HOT)
            .await
            .unwrap()
            .is_empty()
    );
    client.close().await;
}

fn runtime_output(state: &FakeState, method: &str, value: Value) -> Vec<u8> {
    let info = rtao_core::ops::runtime_api::runtime_api_method(
        &state.runtime,
        "BetaBasketRuntimeApi",
        method,
    )
    .unwrap();
    let value = fill_defaults(&state.runtime, info.output, value).unwrap();
    state
        .runtime
        .encode_spec(&TypeSpec::Id(info.output), &value)
        .unwrap()
}

#[tokio::test]
async fn basket_board_batches_rates_and_prices_all_pages_before_stable_nav_sort() {
    let mut state = seed();
    let first = runtime_output(
        &state,
        "get_all_beta_pricing",
        json!({"pricing": [pricing(HOT, 9223372036854775808u128)], "next": HOT}),
    );
    let second = runtime_output(
        &state,
        "get_all_beta_pricing",
        json!({"pricing": [pricing(OTHER, 18446744073709551616u128)], "next": null}),
    );
    state.seed_runtime_encoded(
        "BetaBasketRuntimeApi_get_all_beta_pricing",
        vec![0, 0, 0, 0, 0],
        first,
    );
    let mut next_input = vec![1];
    next_input.extend(HOT.0);
    next_input.extend([0; 4]);
    state.seed_runtime_encoded(
        "BetaBasketRuntimeApi_get_all_beta_pricing",
        next_input,
        second,
    );
    let (client, chain) = client(state).await;
    let before = chain.requests().len();
    let board = reads::root_baskets(&client).await.unwrap();
    assert_eq!(
        board.iter().map(|row| row.hotkey).collect::<Vec<_>>(),
        vec![OTHER, HOT]
    );
    assert_eq!(board[0].basket_rate, 1.5);
    assert_eq!(board[1].basket_rate, -1.0 / 4294967296.0);
    assert_eq!(board[0].chain_pricing.as_ref().unwrap().spot_price, 1.0);
    assert_eq!(board[1].chain_pricing.as_ref().unwrap().spot_price, 0.5);
    assert_eq!(board[0].lifetime_return, Some(13.0 / 5.0));
    assert_eq!(board[0].holdings[0].alpha.netuid, NetUid(1));
    assert_eq!(board[0].holdings[0].realizable_tao.rao, 13);
    let requests = chain.requests();
    let batches: Vec<_> = requests[before..]
        .iter()
        .filter(|request| request.method == "state_queryStorageAt")
        .collect();
    assert_eq!(batches.len(), 1);
    assert_eq!(batches[0].params[0].as_array().unwrap().len(), 2);
    assert_eq!(
        requests[before..]
            .iter()
            .filter(|request| request.method == "state_call"
                && request.params[0] == json!("BetaBasketRuntimeApi_get_all_beta_pricing"))
            .count(),
        2
    );
    client.close().await;
}

#[tokio::test]
async fn basket_board_discards_partial_pricing_on_failure_and_skips_empty_enrichment() {
    let (client, chain) = client(seed()).await;
    chain.mutate_state(|state| {
        let baskets = runtime_output(
            state,
            "get_all_validator_baskets",
            json!([summary(HOT, 7), summary(OTHER, 7)]),
        );
        let page = runtime_output(
            state,
            "get_all_beta_pricing",
            json!({"pricing": [pricing(HOT, 1)], "next": HOT}),
        );
        state.queue_reply(
            "state_call",
            Reply::result(json!(format!("0x{}", hex::encode(baskets)))),
        );
        state.queue_reply(
            "state_call",
            Reply::result(json!(format!("0x{}", hex::encode(page)))),
        );
        state.queue_reply(
            "state_call",
            Reply::error(-32601, "historical pricing method unavailable", None),
        );
    });
    let board = reads::root_baskets(&client).await.unwrap();
    assert_eq!(
        board.iter().map(|row| row.hotkey).collect::<Vec<_>>(),
        vec![HOT, OTHER]
    );
    assert!(board.iter().all(|row| row.chain_pricing.is_none()));
    assert_eq!(board[0].nav_tao.rao, 7);
    chain.mutate_state(|state| {
        state
            .seed_runtime(
                "BetaBasketRuntimeApi",
                "get_all_validator_baskets",
                json!([]),
            )
            .unwrap()
    });
    let view = RecordingView::new(client.clone());
    assert!(reads::root_baskets(&view).await.unwrap().is_empty());
    assert_eq!(
        view.touches()
            .iter()
            .map(|touch| touch.item.as_str())
            .collect::<Vec<_>>(),
        vec!["get_all_validator_baskets"]
    );
    client.close().await;
}

#[tokio::test]
async fn turnover_bucket_uses_integer_refills_and_clamps_excess_availability() {
    let (client, chain) = client(seed()).await;
    let status = reads::basket_trading_status(&client, HOT).await.unwrap();
    assert!(status.enabled);
    assert!(status.frozen);
    assert_eq!(status.budget_tao.rao, u64::MAX);
    assert_eq!(status.refill_per_block_tao.rao, u64::MAX / 7200);
    assert_eq!(status.used_tao.rao, u64::MAX - 2);
    assert_eq!(status.remaining_tao.rao, 2);
    chain.mutate_state(|state| state.seed_runtime("BetaBasketRuntimeApi", "get_basket_trading_status", json!({"enabled": false, "frozen": false, "refill_blocks": 0, "tao_available": u64::MAX, "budget_tao": 5})).unwrap());
    let status = reads::basket_trading_status(&client, HOT).await.unwrap();
    assert!(!status.enabled);
    assert!(!status.frozen);
    assert_eq!(status.refill_per_block_tao.rao, 0);
    assert_eq!(status.used_tao.rao, 0);
    assert_eq!(status.remaining_tao.rao, 5);
    client.close().await;
}

#[tokio::test]
// upstream-e2e: sdk/bittensor-core/tests/e2e.rs::test_subnets_all_batched
async fn subnet_list_uses_four_map_scans_and_zero_for_missing_join_rows() {
    let (client, chain) = client(seed()).await;
    let before = chain.requests().len();
    let rows = reads::subnets(&client).await.unwrap();
    assert_eq!(
        rows.iter().map(|row| row.netuid).collect::<Vec<_>>(),
        vec![NetUid(1), NetUid(2)]
    );
    assert_eq!(rows[0].tempo, u16::MAX);
    assert_eq!(rows[0].burn.rao, u64::MAX);
    assert_eq!(rows[0].burn.netuid, NetUid::ROOT);
    assert_eq!(rows[0].neuron_count, u16::MAX);
    assert_eq!(
        (rows[1].tempo, rows[1].burn.rao, rows[1].neuron_count),
        (0, 0, 0)
    );
    let requests = chain.requests();
    assert_eq!(
        requests[before..]
            .iter()
            .filter(|request| request.method == "state_getKeysPaged")
            .count(),
        4
    );
    assert_eq!(
        requests[before..]
            .iter()
            .filter(|request| request.method == "state_queryStorageAt")
            .count(),
        4
    );
    assert_eq!(
        requests[before..]
            .iter()
            .filter(|request| request.method == "state_getStorage")
            .count(),
        0
    );
    let before = chain.requests().len();
    assert_eq!(reads::subnet(&client, NetUid(1)).await.unwrap(), rows[0]);
    let requests = chain.requests();
    let batches: Vec<_> = requests[before..]
        .iter()
        .filter(|request| request.method == "state_queryStorageAt")
        .collect();
    assert_eq!(batches.len(), 1);
    assert_eq!(batches[0].params[0].as_array().unwrap().len(), 3);
    assert_eq!(
        reads::subnet_registration_cost(&client).await.unwrap().rao,
        u64::MAX
    );
    client.close().await;
}

#[tokio::test]
async fn signed_flow_ema_retains_fractional_rao_and_omits_unrecorded_subnets() {
    let (client, _) = client(seed()).await;
    let flows = reads::subnet_tao_flows(&client).await.unwrap();
    assert_eq!(
        flows.keys().copied().collect::<Vec<_>>(),
        vec![NetUid(1), NetUid(2)]
    );
    assert_eq!(flows[&NetUid(1)], -0.5);
    assert_eq!(flows[&NetUid(2)], i128::MAX as f64 / 18446744073709551616.0);
    assert!(!flows.contains_key(&NetUid(3)));
    client.close().await;
}

#[tokio::test]
async fn mechanism_bonds_scale_each_element_and_keep_empty_rows_and_last_duplicate() {
    let (client, chain) = client(seed()).await;
    let bonds = reads::bonds(&client, NetUid(1), MechId::new(1).unwrap())
        .await
        .unwrap();
    assert_eq!(
        bonds.keys().copied().collect::<Vec<_>>(),
        vec![Uid(1), Uid(3)]
    );
    assert!(bonds[&Uid(1)].is_empty());
    assert_eq!(bonds[&Uid(3)][&Uid(1)], 1.0);
    assert_eq!(bonds[&Uid(3)][&Uid(2)], 1.0 / 65535.0);
    assert!(bonds[&Uid(3)].values().sum::<f64>() > 1.0);
    assert_eq!(
        reads::bonds(&client, NetUid(1), MechId::new(0).unwrap())
            .await
            .unwrap()
            .keys()
            .copied()
            .collect::<Vec<_>>(),
        vec![Uid(8)]
    );
    assert!(
        reads::bonds(&client, NetUid(4095), MechId::new(15).unwrap())
            .await
            .unwrap()
            .is_empty()
    );
    let before = chain.requests().len();
    assert!(
        reads::bonds(&client, NetUid(4096), MechId(0))
            .await
            .is_err()
    );
    assert!(reads::bonds(&client, NetUid(1), MechId(16)).await.is_err());
    assert_eq!(chain.requests().len(), before);
    client.close().await;
}

#[tokio::test]
async fn pending_weight_commits_keep_empty_epochs_order_ciphertext_and_round_zero_time() {
    let (client, chain) = client(seed()).await;
    let commits = reads::timelocked_weight_commits(&client, NetUid(1), MechId::new(1).unwrap())
        .await
        .unwrap();
    assert_eq!(commits.keys().copied().collect::<Vec<_>>(), vec![2, 9]);
    assert!(commits[&2].is_empty());
    assert_eq!(commits[&9][0].hotkey, HOT);
    assert_eq!(commits[&9][0].commit_block, u64::MAX);
    assert_eq!(commits[&9][0].ciphertext, "0xff0001");
    assert_eq!(commits[&9][0].reveals_at.timestamp(), 1692803364);
    assert_eq!(commits[&9][1].hotkey, OTHER);
    assert_eq!(commits[&9][1].ciphertext, "0x");
    assert_eq!(commits[&9][1].reveals_at.timestamp(), 1692803367);
    assert_eq!(
        serde_json::to_value(&commits[&9][1]).unwrap()["reveals_at"],
        json!("2023-08-23T15:09:27+00:00")
    );
    assert!(
        reads::timelocked_weight_commits(&client, NetUid(2), MechId::new(1).unwrap())
            .await
            .unwrap()
            .is_empty()
    );
    chain.mutate_state(|state| {
        state
            .seed_python(
                "SubtensorModule",
                "TimelockedWeightCommits",
                &[json!(4097), json!(9)],
                json!([[HOT, 1, "0xff", u64::MAX]]),
            )
            .unwrap()
    });
    let error = reads::timelocked_weight_commits(&client, NetUid(1), MechId::new(1).unwrap())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("range"));
    client.close().await;
}
