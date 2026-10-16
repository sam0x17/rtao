// Derived from RaoFoundation/subtensor sdk/bittensor-core/tests/e2e.rs @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! M4 owner reads through actual pinned metadata, SCALE state and dispatch.
#![allow(clippy::unwrap_used, clippy::panic)] // Invalid authenticated fixtures must fail these assertions.

use std::collections::BTreeSet;

use rtao_client::{BlockRef, Client, ClientConfig, Network, ViewExt};
use rtao_primitives::{AccountId, NetUid};
use rtao_reads::{ReadName, dispatch, reads::owner};
use rtao_testkit::{FakeChain, FakeState, RecordingView, rpc::RpcClient};
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

fn seed() -> FakeState {
    let mut state = FakeState::default();
    for (item, keys, value) in [
        ("Owner", vec![json!(HOT)], json!(COLD)),
        ("Owner", vec![json!(OTHER)], json!(COLD)),
        (
            "IdentitiesV2",
            vec![json!(COLD)],
            json!({"name": "τ owner", "url": "0xff"}),
        ),
        ("Delegates", vec![json!(HOT)], json!(32768)),
        ("MinDelegateTake", vec![], json!(0)),
        ("MaxDelegateTake", vec![], json!(65535)),
        ("Difficulty", vec![json!(1)], json!(u64::MAX)),
        ("SubnetEmissionEnabled", vec![json!(1)], json!(true)),
        ("Burn", vec![json!(1)], json!(1234567890)),
        ("MechanismEmissionSplit", vec![json!(1)], json!([8, 2, 6])),
        (
            "SubnetIdentitiesV3",
            vec![json!(1)],
            json!({"subnet_name": "τ1", "description": "0xff"}),
        ),
        (
            "SubnetIdentitiesV3",
            vec![json!(2)],
            json!({"subnet_name": ""}),
        ),
        ("NetworkRegisteredAt", vec![json!(1)], json!(81)),
        ("StartCallDelay", vec![], json!(5)),
        ("BasketRate", vec![json!(HOT)], json!({"bits": -1})),
        ("TokenSymbol", vec![json!(1)], json!("Ω")),
    ] {
        state
            .seed_python("SubtensorModule", item, &keys, value)
            .unwrap();
    }
    state
        .seed_python(
            "System",
            "Account",
            &[json!(COLD)],
            json!({"data": {"free": u64::MAX}}),
        )
        .unwrap();
    state
        .seed_python("MevShield", "NextKey", &[], json!(vec![171u8; 1184]))
        .unwrap();
    state
        .seed_constant("SubtensorModule", "InitialStartCallDelay", json!(5))
        .unwrap();
    let position =
        json!({"hotkey": HOT, "coldkey": COLD, "netuid": 1, "stake": 3, "is_registered": true});
    let root =
        json!({"hotkey": OTHER, "coldkey": COLD, "netuid": 0, "stake": 5, "is_registered": false});
    for (api, method, value) in [
        (
            "SubnetInfoRuntimeApi",
            "get_next_epoch_start_block",
            json!(104),
        ),
        (
            "StakeInfoRuntimeApi",
            "get_most_convicted_hotkey_on_subnet",
            json!(HOT),
        ),
        (
            "StakeInfoRuntimeApi",
            "get_stake_info_for_coldkey",
            json!([position.clone(), root.clone()]),
        ),
        (
            "StakeInfoRuntimeApi",
            "get_stake_info_for_coldkeys",
            json!([[COLD, [position, root]]]),
        ),
        (
            "SwapRuntimeApi",
            "current_alpha_price_all",
            json!([{"netuid": 1, "price": 500000000}, {"netuid": 2, "price": 250000000}]),
        ),
        (
            "DelegateInfoRuntimeApi",
            "get_delegated",
            json!([[{"delegate_ss58": HOT, "owner_ss58": COLD, "take": 32768, "registrations": [2,1], "validator_permits": [1], "nominators": [[OTHER, [[1, 3]]]], "return_per_1000": 17, "total_daily_return": 19}, [1, 3]]]),
        ),
        ("BetaBasketRuntimeApi", "get_root_basket_owed", json!(19)),
        (
            "BetaBasketRuntimeApi",
            "get_validator_basket_summary",
            json!({"hotkey": HOT, "nav_tao": 9, "spot_nav_tao": 10, "shares": 3, "deposited_tao": 5, "redeemed_tao": 1, "holdings": [{"netuid": 1, "alpha": 3, "spot_tao": 4, "realizable_tao": 2}]}),
        ),
        (
            "BetaBasketRuntimeApi",
            "get_beta_pricing",
            json!({"hotkey": HOT, "spot_price": {"bits": 18446744073709551616u128}, "display_price": {"bits": 9223372036854775808u128}, "stake_price": {"bits": 0}, "first_block": 1, "provisional": true}),
        ),
        (
            "SubnetInfoRuntimeApi",
            "get_subnet_hyperparams_v3",
            json!([{"name": "alpha_sigmoid_steepness", "value": {"I32F32": {"bits": -1}}}, {"name": "burn_increase_mult", "value": {"U64F64": {"bits": u128::MAX}}}, {"name": "future_name", "value": {"U128": u128::MAX}}]),
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
async fn all_twenty_two_owner_reads_observe_exact_hit_sets_at_one_state_hash() {
    let (client, chain) = client(seed()).await;
    let samples = [
        (ReadName::Balance, json!({"coldkey_ss58": COLD})),
        (ReadName::Balances, json!({"coldkey_ss58s": [COLD, OTHER]})),
        (ReadName::MevShieldNextKey, json!({})),
        (ReadName::DelegateTake, json!({"hotkey_ss58": HOT})),
        (ReadName::Delegated, json!({"coldkey_ss58": COLD})),
        (ReadName::BlocksUntilNextEpoch, json!({"netuid": 1})),
        (ReadName::NextEpochStartBlock, json!({"netuid": 1})),
        (ReadName::Difficulty, json!({"netuid": 1})),
        (ReadName::SubnetEmissionEnabled, json!({"netuid": 1})),
        (
            ReadName::HotkeyIdentities,
            json!({"hotkey_ss58s": [HOT, OTHER, HOT]}),
        ),
        (ReadName::Identity, json!({"coldkey_ss58": COLD})),
        (ReadName::MostConvictedHotkey, json!({"netuid": 1})),
        (ReadName::RootBasketOwed, json!({"coldkey_ss58": COLD})),
        (
            ReadName::StakeValueForColdkey,
            json!({"coldkey_ss58": COLD}),
        ),
        (
            ReadName::StakeValueForColdkeys,
            json!({"coldkey_ss58s": [COLD, OTHER]}),
        ),
        (
            ReadName::ValidatorBasketSummary,
            json!({"hotkey_ss58": HOT}),
        ),
        (ReadName::Burn, json!({"netuid": 1})),
        (ReadName::MechanismEmissionSplit, json!({"netuid": 1})),
        (ReadName::SubnetHyperparameters, json!({"netuid": 1})),
        (ReadName::SubnetIdentity, json!({"netuid": 1})),
        (ReadName::SubnetNames, json!({})),
        (ReadName::SubnetStartSchedule, json!({"netuid": 1})),
    ];
    for (name, args) in samples {
        let mut observed = BTreeSet::new();
        for block in [100, 99] {
            let snapshot = client.at(BlockRef::Number(block)).await.unwrap();
            let hash = snapshot.block().hash;
            let view = RecordingView::new(snapshot);
            let before = chain.requests().len();
            dispatch(&view, name, &params(args.clone())).await.unwrap();
            for touch in view.touches() {
                assert_eq!(touch.at, Some(hash), "{name}");
                assert!(
                    name.spec().hits.iter().any(|hit| hit.matches(&touch)),
                    "{name}: unexpected {touch:?}"
                );
                observed.insert((touch.kind, touch.pallet_or_api, touch.item));
            }
            assert_eq!(view.block_hashes().len(), 1, "{name}");
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
async fn balances_keep_u64_precision_and_batch_heterogeneous_duplicates() {
    let (client, chain) = client(seed()).await;
    let before = chain.requests().len();
    let amounts = owner::balances(&client, vec![COLD, OTHER, COLD])
        .await
        .unwrap();
    assert_eq!(amounts.len(), 2);
    assert_eq!(amounts[&COLD].rao, u64::MAX);
    assert_eq!(amounts[&OTHER].rao, 0);
    let requests = chain.requests();
    let queries: Vec<_> = requests[before..]
        .iter()
        .filter(|request| request.method == "state_queryStorageAt")
        .collect();
    assert_eq!(queries.len(), 1);
    assert_eq!(queries[0].params[0].as_array().unwrap().len(), 2);
    assert_eq!(owner::balance(&client, COLD).await.unwrap().rao, u64::MAX);
    assert_eq!(owner::balance(&client, OTHER).await.unwrap().rao, 0);
    let existential_deposit = client
        .constant(&rtao_runtime::constants::balances::EXISTENTIAL_DEPOSIT)
        .await
        .unwrap();
    assert!(existential_deposit.0 > 0);
    assert!(owner::balances(&client, vec![]).await.unwrap().is_empty());
    client.close().await;
}

#[tokio::test]
async fn delegate_take_and_hotkey_identities_pin_their_batches_and_deduplicate() {
    let (client, chain) = client(seed()).await;
    let view = RecordingView::new(client.clone());
    let before = chain.requests().len();
    let take = owner::delegate_take(&view, HOT).await.unwrap();
    assert_eq!(take.take_u16, 32768);
    assert_eq!(take.take, 32768.0 / 65535.0);
    assert_eq!((take.min, take.max), (0.0, 1.0));
    assert_eq!(view.block_hashes().len(), 1);
    assert_eq!(
        chain.requests()[before..]
            .iter()
            .filter(|request| request.method == "state_queryStorageAt")
            .count(),
        1
    );
    view.clear();
    let before = chain.requests().len();
    let identities = owner::hotkey_identities(&view, vec![HOT, OTHER, HOT, AccountId::ZERO])
        .await
        .unwrap();
    assert_eq!(identities.len(), 2);
    assert_eq!(identities[&HOT]["name"], json!("τ owner"));
    assert_eq!(identities[&HOT]["url"], json!("0xff"));
    assert_eq!(identities[&HOT], identities[&OTHER]);
    assert_eq!(view.block_hashes().len(), 1);
    let queries: Vec<_> = chain.requests()[before..]
        .iter()
        .filter(|request| request.method == "state_queryStorageAt")
        .map(|request| request.params[0].as_array().unwrap().len())
        .collect();
    assert_eq!(queries, [3, 2]);
    chain.mutate_state(|state| {
        state
            .seed_python(
                "SubtensorModule",
                "IdentitiesV2",
                &[json!(AccountId::ZERO)],
                json!({"name": "zero owner"}),
            )
            .unwrap();
    });
    let identities = owner::hotkey_identities(&view, vec![AccountId::ZERO])
        .await
        .unwrap();
    // D-001 makes the typed Owner accessor optional; the Python read's JSON
    // still follows the truthy SS58 zero-owner string to its identity.
    assert_eq!(identities[&AccountId::ZERO]["name"], json!("zero owner"));
    view.clear();
    let before = chain.requests().len();
    assert!(
        owner::hotkey_identities(&view, vec![])
            .await
            .unwrap()
            .is_empty()
    );
    assert!(view.touches().is_empty());
    assert_eq!(chain.requests().len(), before);
    client.close().await;
}

#[tokio::test]
async fn delegation_and_stake_valuation_keep_chain_order_units_and_symbols() {
    let (client, chain) = client(seed()).await;
    let delegated = owner::delegated(&client, COLD).await.unwrap();
    assert_eq!(delegated[0].delegate.registrations, [NetUid(2), NetUid(1)]);
    assert_eq!(delegated[0].delegate.nominators, 1);
    assert_eq!(delegated[0].stake.netuid, NetUid(1));
    assert_eq!(delegated[0].stake.symbol.as_ref().unwrap().as_str(), "Ω");
    let value = owner::stake_value_for_coldkey(&client, COLD).await.unwrap();
    assert_eq!(value.block, 100);
    assert_eq!(
        value
            .positions
            .iter()
            .map(|position| position.netuid)
            .collect::<Vec<_>>(),
        [NetUid(1), NetUid(0)]
    );
    assert_eq!(value.positions[0].stake.rao, 3);
    assert_eq!(
        value.positions[0].stake.symbol.as_ref().unwrap().as_str(),
        "Ω"
    );
    assert_eq!(value.positions[1].stake.netuid, NetUid::ROOT);
    assert_eq!(value.stake_value.rao, 6); // trunc(3 * 0.5) + root 5
    assert_eq!(value.tao_per_alpha[&NetUid(1)], 0.5);
    let values = owner::stake_value_for_coldkeys(&client, vec![COLD, OTHER, COLD])
        .await
        .unwrap();
    assert_eq!(values.len(), 2);
    assert_eq!(values[&COLD].stake_value.rao, 6);
    assert!(values[&OTHER].positions.is_empty());
    assert_eq!(values[&OTHER].stake_value.rao, 0);
    chain.mutate_state(|state| {
        state
            .seed_runtime("SwapRuntimeApi", "current_alpha_price_all", json!([]))
            .unwrap()
    });
    assert_eq!(
        owner::stake_value_for_coldkey(&client, COLD)
            .await
            .unwrap()
            .stake_value
            .rao,
        5
    );
    client.close().await;
}

#[tokio::test]
async fn spot_valuations_reject_native_overflow_without_wrapping_or_saturating() {
    let (client, chain) = client(seed()).await;
    chain.mutate_state(|state| {
        state
            .seed_runtime(
                "StakeInfoRuntimeApi",
                "get_stake_info_for_coldkey",
                json!([
                    {"hotkey": HOT, "coldkey": COLD, "netuid": 0, "stake": u64::MAX},
                    {"hotkey": OTHER, "coldkey": COLD, "netuid": 0, "stake": 1}
                ]),
            )
            .unwrap();
    });
    let error = owner::stake_value_for_coldkey(&client, COLD)
        .await
        .unwrap_err();
    assert_eq!(error.code(), rtao_catalog::ErrorCode::InvalidArgument);
    assert!(error.to_string().contains("sum of spot valuations"));
    chain.mutate_state(|state| {
        state
            .seed_runtime(
                "StakeInfoRuntimeApi",
                "get_stake_info_for_coldkey",
                json!([
                    {"hotkey": HOT, "coldkey": COLD, "netuid": 1, "stake": u64::MAX}
                ]),
            )
            .unwrap();
        state
            .seed_runtime(
                "SwapRuntimeApi",
                "current_alpha_price_all",
                json!([
                    {"netuid": 1, "price": 1000000000}
                ]),
            )
            .unwrap();
    });
    assert!(
        owner::stake_value_for_coldkey(&client, COLD)
            .await
            .unwrap_err()
            .to_string()
            .contains("spot valuation exceeds")
    );
    client.close().await;
}

#[tokio::test]
async fn owner_scalar_identity_epoch_and_hyperparameter_projections_match_python() {
    let (client, chain) = client(seed()).await;
    assert_eq!(
        owner::difficulty(&client, NetUid(1)).await.unwrap(),
        u64::MAX
    );
    assert_eq!(
        owner::subnet_emission_enabled(&client, NetUid(1))
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        owner::burn(&client, NetUid(1)).await.unwrap().rao,
        1234567890
    );
    assert_eq!(
        owner::mechanism_emission_split(&client, NetUid(1))
            .await
            .unwrap(),
        [8, 2, 6]
    );
    assert_eq!(
        owner::next_epoch_start_block(&client, NetUid(1))
            .await
            .unwrap(),
        Some(104)
    );
    assert_eq!(
        owner::blocks_until_next_epoch(&client, NetUid(1))
            .await
            .unwrap(),
        Some(4)
    );
    assert_eq!(
        owner::most_convicted_hotkey(&client, NetUid(1))
            .await
            .unwrap(),
        Some(HOT)
    );
    assert_eq!(
        owner::root_basket_owed(&client, COLD).await.unwrap().rao,
        19
    );
    assert_eq!(
        owner::subnet_identity(&client, NetUid(1))
            .await
            .unwrap()
            .unwrap()["subnet_name"],
        json!("τ1")
    );
    assert_eq!(
        owner::subnet_names(&client)
            .await
            .unwrap()
            .into_iter()
            .collect::<Vec<_>>(),
        [(NetUid(1), "τ1".to_owned())]
    );
    assert!(owner::identity(&client, HOT).await.unwrap().is_none());
    let hyperparameters = owner::subnet_hyperparameters(&client, NetUid(1))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(hyperparameters["alpha_sigmoid_steepness"], json!(-1));
    assert_eq!(hyperparameters["burn_increase_mult"], json!(u128::MAX));
    assert_eq!(hyperparameters["future_name"], json!(u128::MAX));
    let schedule = owner::subnet_start_schedule(&client, NetUid(1))
        .await
        .unwrap();
    assert_eq!(
        (
            schedule.registered_at,
            schedule.start_call_delay,
            schedule.earliest_start_block,
            schedule.current_block
        ),
        (81, 5, 86, 100)
    );
    assert_eq!(
        owner::mev_shield_next_key(&client).await.unwrap(),
        Some(format!("0x{}", "ab".repeat(1184)))
    );
    chain.mutate_state(|state| {
        state
            .seed_runtime(
                "SubnetInfoRuntimeApi",
                "get_next_epoch_start_block",
                json!(99),
            )
            .unwrap();
        state
            .seed_runtime(
                "SubnetInfoRuntimeApi",
                "get_subnet_hyperparams_v3",
                json!([]),
            )
            .unwrap();
        state
            .seed_python("MevShield", "NextKey", &[], json!([]))
            .unwrap();
    });
    assert_eq!(
        owner::blocks_until_next_epoch(&client, NetUid(1))
            .await
            .unwrap(),
        Some(0)
    );
    assert!(
        owner::subnet_hyperparameters(&client, NetUid(1))
            .await
            .unwrap()
            .unwrap()
            .is_empty()
    );
    assert_eq!(owner::mev_shield_next_key(&client).await.unwrap(), None);
    chain.mutate_state(|state| {
        state
            .seed_runtime(
                "SubnetInfoRuntimeApi",
                "get_next_epoch_start_block",
                json!(null),
            )
            .unwrap();
        state
            .seed_runtime(
                "SubnetInfoRuntimeApi",
                "get_subnet_hyperparams_v3",
                json!(null),
            )
            .unwrap();
        state
            .seed_runtime(
                "StakeInfoRuntimeApi",
                "get_most_convicted_hotkey_on_subnet",
                json!(null),
            )
            .unwrap();
    });
    assert_eq!(
        owner::blocks_until_next_epoch(&client, NetUid(1))
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        owner::subnet_hyperparameters(&client, NetUid(1))
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        owner::most_convicted_hotkey(&client, NetUid(1))
            .await
            .unwrap(),
        None
    );
    client.close().await;
}

#[tokio::test]
async fn validator_summary_flattens_pricing_and_omits_it_when_the_request_fails() {
    let (client, chain) = client(seed()).await;
    let summary = owner::validator_basket_summary(&client, HOT).await.unwrap();
    assert_eq!(summary.beta_total_raw, 3);
    assert_eq!(summary.beta_price_tao, 3.0);
    assert_eq!(summary.lifetime_return, Some(2.0));
    assert_eq!(summary.holdings[0].alpha.netuid, NetUid(1));
    assert_eq!(summary.holdings[0].spot_tao.netuid, NetUid::ROOT);
    assert_eq!(summary.basket_rate, -1.0 / 4294967296.0);
    let pricing = summary.chain_pricing.unwrap();
    assert_eq!(
        (
            pricing.spot_price,
            pricing.display_price,
            pricing.provisional
        ),
        (1.0, 0.5, true)
    );
    chain.queue_transport_error(
        "state_call",
        rtao_testkit::rpc::Error::Client(Box::new(std::io::Error::other(
            "summary transport failure",
        ))),
    );
    assert!(owner::validator_basket_summary(&client, HOT).await.is_err());
    chain.mutate_state(|state| {
        let method = rtao_core::ops::runtime_api::runtime_api_method(
            &state.runtime,
            "BetaBasketRuntimeApi",
            "get_validator_basket_summary",
        )
        .unwrap();
        let value = rtao_testkit::fakes::fill_defaults(
            &state.runtime,
            method.output,
            json!({"hotkey": HOT}),
        )
        .unwrap();
        let encoded = state
            .runtime
            .encode_spec(&rtao_core::runtime::TypeSpec::Id(method.output), &value)
            .unwrap();
        state.queue_reply(
            "state_call",
            rtao_testkit::fakes::Reply::result(json!(format!("0x{}", hex::encode(encoded)))),
        );
        state.queue_reply(
            "state_call",
            rtao_testkit::fakes::Reply::error(-32601, "not available", None),
        );
    });
    let summary = owner::validator_basket_summary(&client, HOT).await.unwrap();
    assert!(summary.chain_pricing.is_none());
    assert_eq!(summary.lifetime_return, None);
    assert_eq!(summary.beta_price_tao, 0.0);
    assert!(
        serde_json::to_value(summary)
            .unwrap()
            .get("chain_pricing")
            .is_none()
    );
    client.close().await;
}

#[tokio::test]
async fn basket_reads_keep_compatible_older_api_versions() {
    for version in [1, 3, 4] {
        let mut state = seed();
        let request = rtao_testkit::fakes::RpcRequest {
            id: json!(1),
            method: "state_getRuntimeVersion".into(),
            params: json!([]),
        };
        let mut advertised = state.dispatch(&request).outcome.unwrap().unwrap();
        // All API versions are lowered. These calls exercise only the basket
        // APIs and storage, preserving the fake's authenticated SCALE shapes.
        for api in advertised["apis"].as_array_mut().unwrap() {
            api[1] = json!(version);
        }
        state.set_result("state_getRuntimeVersion", advertised);
        let (client, chain) = client(state).await;
        let snapshot = client.at(BlockRef::Number(100)).await.unwrap();
        assert_eq!(
            snapshot
                .block()
                .runtime
                .runtime_api_version("BetaBasketRuntimeApi"),
            Some(version)
        );
        assert_eq!(
            owner::root_basket_owed(&snapshot, COLD).await.unwrap().rao,
            19
        );
        let summary = owner::validator_basket_summary(&snapshot, HOT)
            .await
            .unwrap();
        assert_eq!(summary.nav_tao.rao, 9);
        assert!(summary.chain_pricing.is_some());
        assert!(
            chain
                .requests()
                .iter()
                .any(|request| request.method == "state_call"
                    && request.params[0] == "BetaBasketRuntimeApi_get_root_basket_owed")
        );
        client.close().await;
    }
}

// D-040: pinned native source regressions; no live Python execution.
fn start_delay_state(registered_at: u64, delay: u64) -> FakeState {
    let mut state = seed();
    state
        .seed_python(
            "SubtensorModule",
            "NetworkRegisteredAt",
            &[json!(1)],
            json!(registered_at),
        )
        .unwrap();
    state
        .seed_python("SubtensorModule", "StartCallDelay", &[], json!(delay))
        .unwrap();
    state
}

#[tokio::test]
async fn d040_mutable_start_delay_overrides_initial_default() {
    let (client, _) = client(start_delay_state(81, 9)).await;
    let schedule = owner::subnet_start_schedule(&client, NetUid(1))
        .await
        .unwrap();
    assert_eq!(schedule.start_call_delay, 9);
    assert_eq!(schedule.earliest_start_block, 90u128);
    assert_eq!(
        serde_json::to_value(schedule).unwrap(),
        json!({"netuid":1,"registered_at":81,"start_call_delay":9,"earliest_start_block":90,"current_block":100})
    );
    client.close().await;
}

#[tokio::test]
async fn d040_lower_mutable_start_delay_overrides_initial_default() {
    let (client, _) = client(start_delay_state(81, 3)).await;
    let schedule = owner::subnet_start_schedule(&client, NetUid(1))
        .await
        .unwrap();
    assert_eq!(
        (schedule.start_call_delay, schedule.earliest_start_block),
        (3, 84)
    );
    client.close().await;
}

#[tokio::test]
async fn d040_zero_mutable_start_delay_overrides_initial_default() {
    let (client, _) = client(start_delay_state(81, 0)).await;
    let schedule = owner::subnet_start_schedule(&client, NetUid(1))
        .await
        .unwrap();
    assert_eq!(
        (schedule.start_call_delay, schedule.earliest_start_block),
        (0, 81)
    );
    client.close().await;
}

#[tokio::test]
async fn d040_start_schedule_saturates_u64_eligibility_bound() {
    let (client, _) = client(start_delay_state(u64::MAX - 2, 9)).await;
    let schedule = owner::subnet_start_schedule(&client, NetUid(1))
        .await
        .unwrap();
    assert_eq!(schedule.registered_at, u64::MAX - 2);
    assert_eq!(schedule.earliest_start_block, u128::from(u64::MAX));
    assert_eq!(
        serde_json::to_value(schedule).unwrap()["earliest_start_block"],
        json!(u64::MAX)
    );
    client.close().await;
}

#[tokio::test]
async fn d040_start_schedule_descriptor_coverage_is_exact_at_pinned_hashes() {
    use rtao_client::ReadKind;
    use rtao_reads::HitKind;
    let (client, chain) = client(start_delay_state(81, 9)).await;
    let expected = BTreeSet::from([
        (
            ReadKind::Storage,
            "SubtensorModule".to_owned(),
            "NetworkRegisteredAt".to_owned(),
        ),
        (
            ReadKind::Storage,
            "SubtensorModule".to_owned(),
            "StartCallDelay".to_owned(),
        ),
    ]);
    let declared: BTreeSet<_> = ReadName::SubnetStartSchedule
        .spec()
        .hits
        .iter()
        .map(|hit| {
            assert_eq!(hit.kind, HitKind::Storage);
            (
                ReadKind::Storage,
                hit.container.to_owned(),
                hit.name.to_owned(),
            )
        })
        .collect();
    assert_eq!(declared, expected);
    for block in [99, 100] {
        let snapshot = client.at(BlockRef::Number(block)).await.unwrap();
        let hash = snapshot.block().hash;
        let view = RecordingView::new(snapshot);
        let before = chain.requests().len();
        let result = dispatch(
            &view,
            ReadName::SubnetStartSchedule,
            &params(json!({"netuid":1})),
        )
        .await
        .unwrap();
        assert_eq!(result["earliest_start_block"], json!(90));
        let observed: BTreeSet<_> = view
            .touches()
            .iter()
            .map(|touch| {
                assert_eq!(touch.at, Some(hash));
                (touch.kind, touch.pallet_or_api.clone(), touch.item.clone())
            })
            .collect();
        assert_eq!(observed, expected);
        assert_eq!(view.block_hashes(), BTreeSet::from([hash]));
        let requests = chain.requests();
        let reads: Vec<_> = requests[before..]
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
        assert_eq!(reads.len(), 2);
        for request in reads {
            assert_eq!(
                request.params.as_array().unwrap().last(),
                Some(&json!(format!("0x{}", hex::encode(hash.0))))
            );
        }
    }
    client.close().await;
}

#[tokio::test]
async fn m5_proxies_keep_model_fields_chain_order_and_native_deposit() {
    use rtao_reads::Namespaces;
    use rtao_runtime::models::ProxyType;

    let mut state = FakeState::default();
    let rows = json!([
        {"delegate": OTHER, "proxy_type": "RootClaim", "delay": u32::MAX},
        {"delegate": HOT, "proxy_type": "Any", "delay": 0},
        {"delegate": OTHER, "proxy_type": "BasketTrading", "delay": 7}
    ]);
    state
        .seed_python(
            "Proxy",
            "Proxies",
            &[json!(COLD)],
            json!([rows, 1000000000]),
        )
        .unwrap();
    state
        .seed_python(
            "SubtensorModule",
            "TokenSymbol",
            &[json!(0)],
            json!("root-symbol"),
        )
        .unwrap();
    let (client, chain) = client(state).await;
    let result = owner::proxies(&client, COLD).await.unwrap();
    assert_eq!(
        result
            .proxies
            .iter()
            .map(|proxy| (proxy.delegate, proxy.proxy_type, proxy.delay))
            .collect::<Vec<_>>(),
        [
            (OTHER, ProxyType::RootClaim, u32::MAX),
            (HOT, ProxyType::Any, 0),
            (OTHER, ProxyType::BasketTrading, 7),
        ]
    );
    assert_eq!(result.deposit.rao, 1000000000);
    assert_eq!(result.deposit.netuid, NetUid::ROOT);
    assert!(result.deposit.symbol.is_none()); // Python uses Balance.from_rao, not view.balance.
    let expected = json!({"proxies": rows, "deposit": "τ1.000000000"});
    assert_eq!(serde_json::to_value(&result).unwrap(), expected);
    assert_eq!(
        serde_json::to_value(client.balances().proxies(COLD).await.unwrap()).unwrap(),
        expected
    );
    assert_eq!(
        dispatch(
            &client,
            ReadName::Proxies,
            &params(json!({"coldkey_ss58": COLD}))
        )
        .await
        .unwrap(),
        expected
    );
    chain.mutate_state(|state| {
        state
            .seed_python("Proxy", "Proxies", &[json!(COLD)], json!([[], u64::MAX]))
            .unwrap();
    });
    let result = owner::proxies(&client, COLD).await.unwrap();
    assert!(result.proxies.is_empty());
    assert_eq!(result.deposit.rao, u64::MAX);
    assert!(result.deposit.symbol.is_none());
    client.close().await;
}

#[tokio::test]
async fn m5_read_misses_decode_metadata_defaults() {
    let mut state = FakeState::default();
    for (pallet, item, keys) in [
        (
            "Proxy",
            "Proxies",
            vec![rtao_core::codec::Value::Str(COLD.to_string())],
        ),
        (
            "SubtensorModule",
            "RootClaimableThreshold",
            vec![rtao_core::codec::Value::Uint(0)],
        ),
    ] {
        let info = state.runtime.storage_entry(pallet, item).unwrap();
        let key = state.runtime.storage_key(info, &keys).unwrap();
        state.seed_encoded(key, None); // Explicit RPC miss, bypassing fake default seeds.
    }
    let (client, _) = client(state).await;
    let proxies = owner::proxies(&client, COLD).await.unwrap();
    assert!(proxies.proxies.is_empty());
    assert_eq!(proxies.deposit.rao, 0);
    assert_eq!(proxies.deposit.netuid, NetUid::ROOT);
    assert_eq!(
        owner::root_claim_threshold(&client).await.unwrap().rao,
        500000 // Pinned DefaultMinRootClaimAmount and metadata default, not a local zero.
    );
    client.close().await;
}

#[tokio::test]
async fn m5_root_claim_threshold_discards_fractional_rao_and_reads_only_root() {
    use rtao_reads::Namespaces;

    let mut state = FakeState::default();
    state
        .seed_python(
            "SubtensorModule",
            "TokenSymbol",
            &[json!(0)],
            json!("root-symbol"),
        )
        .unwrap();
    state
        .seed_python(
            "SubtensorModule",
            "RootClaimableThreshold",
            &[json!(1)],
            json!({"bits": 1000000000i128 << 32}),
        )
        .unwrap();
    let (client, chain) = client(state).await;
    for (bits, expected_rao) in [
        (0, 0),
        (1, 0),
        (i128::from(u32::MAX), 0),
        (1i128 << 32, 1),
        ((42i128 << 32) | i128::from(u32::MAX), 42),
        (
            (i128::from(u64::MAX) << 32) | i128::from(u32::MAX),
            u64::MAX,
        ),
    ] {
        chain.mutate_state(|state| {
            state
                .seed_python(
                    "SubtensorModule",
                    "RootClaimableThreshold",
                    &[json!(0)],
                    json!({"bits": bits}),
                )
                .unwrap();
        });
        let result = owner::root_claim_threshold(&client).await.unwrap();
        assert_eq!(result.rao, expected_rao, "bits={bits}");
        assert_eq!(result.netuid, NetUid::ROOT);
        assert_eq!(result.symbol.as_ref().unwrap().as_str(), "root-symbol");
        assert_eq!(
            client.staking().root_claim_threshold().await.unwrap(),
            result
        );
        assert_eq!(
            dispatch(&client, ReadName::RootClaimThreshold, &params(json!({})))
                .await
                .unwrap(),
            serde_json::to_value(result).unwrap()
        );
    }
    client.close().await;
}

#[tokio::test]
async fn m5_root_claim_threshold_rejects_negative_and_overflowing_native_balances() {
    let (client, chain) = client(FakeState::default()).await;
    for bits in [
        -1, // Arithmetic >> 32 is -1, even when the fraction is less than one rao.
        -(1i128 << 32),
        i128::MIN,
        (i128::from(u64::MAX) + 1) << 32,
        i128::MAX,
    ] {
        chain.mutate_state(|state| {
            state
                .seed_python(
                    "SubtensorModule",
                    "RootClaimableThreshold",
                    &[json!(0)],
                    json!({"bits": bits}),
                )
                .unwrap();
        });
        let error = owner::root_claim_threshold(&client).await.unwrap_err();
        assert!(
            matches!(
                error,
                rtao_reads::ReadError::SurfaceChanged {
                    descriptor: "SubtensorModule.RootClaimableThreshold",
                    ..
                }
            ),
            "bits={bits}: {error}"
        );
        assert!(error.to_string().contains("native u64 balance"));
        assert!(
            dispatch(&client, ReadName::RootClaimThreshold, &params(json!({})))
                .await
                .is_err()
        );
    }
    client.close().await;
}

#[tokio::test]
async fn m5_reads_reject_truncated_scale_instead_of_returning_defaults() {
    let mut state = FakeState::default();
    for (pallet, item, keys, bytes) in [
        (
            "Proxy",
            "Proxies",
            vec![rtao_core::codec::Value::Str(COLD.to_string())],
            vec![4, 0], // One delegation, with an incomplete account field.
        ),
        (
            "SubtensorModule",
            "RootClaimableThreshold",
            vec![rtao_core::codec::Value::Uint(0)],
            vec![0; 15], // I96F32 requires all 16 signed fixed-point bytes.
        ),
    ] {
        let info = state.runtime.storage_entry(pallet, item).unwrap();
        let key = state.runtime.storage_key(info, &keys).unwrap();
        state.seed_encoded(key, Some(bytes));
    }
    let (client, _) = client(state).await;
    assert!(matches!(
        owner::proxies(&client, COLD).await.unwrap_err(),
        rtao_reads::ReadError::Client(_)
    ));
    assert!(matches!(
        owner::root_claim_threshold(&client).await.unwrap_err(),
        rtao_reads::ReadError::Client(_)
    ));
    client.close().await;
}

#[tokio::test]
async fn m5_two_reads_observe_exact_descriptor_sets_at_pinned_hashes() {
    use rtao_client::ReadKind;
    use rtao_reads::HitKind;

    let (client, chain) = client(FakeState::default()).await;
    for (name, args, pallet, item) in [
        (
            ReadName::Proxies,
            json!({"coldkey_ss58": COLD}),
            "Proxy",
            "Proxies",
        ),
        (
            ReadName::RootClaimThreshold,
            json!({}),
            "SubtensorModule",
            "RootClaimableThreshold",
        ),
    ] {
        let expected = BTreeSet::from([(ReadKind::Storage, pallet.to_owned(), item.to_owned())]);
        let declared: BTreeSet<_> = name
            .spec()
            .hits
            .iter()
            .map(|hit| {
                assert_eq!(hit.kind, HitKind::Storage);
                (
                    ReadKind::Storage,
                    hit.container.to_owned(),
                    hit.name.to_owned(),
                )
            })
            .collect();
        assert_eq!(declared, expected);
        for block in [99, 100] {
            let snapshot = client.at(BlockRef::Number(block)).await.unwrap();
            let hash = snapshot.block().hash;
            let view = RecordingView::new(snapshot);
            let before = chain.requests().len();
            dispatch(&view, name, &params(args.clone())).await.unwrap();
            let touches = view.touches();
            assert_eq!(touches.len(), 1, "{name}");
            let observed: BTreeSet<_> = touches
                .iter()
                .map(|touch| {
                    assert_eq!(touch.at, Some(hash), "{name}");
                    (touch.kind, touch.pallet_or_api.clone(), touch.item.clone())
                })
                .collect();
            assert_eq!(observed, expected);
            assert_eq!(view.block_hashes(), BTreeSet::from([hash]));
            let requests = chain.requests();
            let reads: Vec<_> = requests[before..]
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
            assert_eq!(reads.len(), 1, "{name}");
            assert_eq!(reads[0].method, "state_queryStorageAt");
            assert_eq!(
                reads[0].params.as_array().unwrap().last(),
                Some(&json!(format!("0x{}", hex::encode(hash.0))))
            );
        }
    }
    client.close().await;
}

// Upstream e2e.rs::test_typed_reads. All values below are supplied state,
// not live subnet/registration/ownership or runtime availability evidence.
#[tokio::test]
async fn client_typed_reads_keep_all_five_core_assertions_at_one_snapshot() {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        use rtao_reads::generated::interfaces as reads;
        let mut state = seed();
        state
            .seed_runtime(
                "SubnetInfoRuntimeApi",
                "get_subnet_hyperparams_v3",
                json!([
                    {"name":"tempo","value":{"U16":20}},
                    {"name":"burn_half_life","value":{"U64":123}}
                ]),
            )
            .unwrap();
        state
            .seed_runtime(
                "SubnetInfoRuntimeApi",
                "get_metagraph",
                json!({
                    "netuid":1,"hotkeys":[HOT],"name":[],"symbol":[]
                }),
            )
            .unwrap();
        state
            .seed_python(
                "SubtensorModule",
                "WeightsSetRateLimit",
                &[json!(1)],
                json!(7),
            )
            .unwrap();
        state
            .seed_python(
                "SubtensorModule",
                "ChildKeys",
                &[json!(HOT), json!(1)],
                json!([[u64::MAX, OTHER]]),
            )
            .unwrap();
        let (client, chain) = client(state).await;
        let snapshot = client.at(BlockRef::Best).await.unwrap();
        let hash = format!("0x{}", hex::encode(snapshot.block().hash.0));
        let start = chain.requests().len();
        let hyperparameters = reads::subnet_hyperparameters(&snapshot, NetUid(1))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(hyperparameters["tempo"], json!(20));
        assert_eq!(hyperparameters["burn_half_life"], json!(123));
        let rate = reads::weights_rate_limit(&snapshot, Some(NetUid(1)))
            .await
            .unwrap();
        assert_eq!(rate, 7);
        let metagraph = reads::metagraph(&snapshot, NetUid(1)).await.unwrap();
        assert_eq!(metagraph["hotkeys"], json!([HOT]));
        let positions = reads::stake_for_coldkey(&snapshot, COLD).await.unwrap();
        assert_eq!(positions.len(), 2);
        assert_eq!(
            (
                positions[0].coldkey,
                positions[0].hotkey,
                positions[0].netuid
            ),
            (COLD, HOT, NetUid(1))
        );
        let children = reads::children(&snapshot, HOT, NetUid(1)).await.unwrap();
        assert_eq!(children, [(u64::MAX, OTHER)]);
        let requests = chain.requests();
        let observed = &requests[start..];
        let methods = observed
            .iter()
            .filter(|request| request.method == "state_call")
            .map(|request| request.params[0].as_str().unwrap().to_owned())
            .collect::<BTreeSet<_>>();
        assert_eq!(
            methods,
            BTreeSet::from([
                "SubnetInfoRuntimeApi_get_subnet_hyperparams_v3".to_owned(),
                "SubnetInfoRuntimeApi_get_metagraph".to_owned(),
                "StakeInfoRuntimeApi_get_stake_info_for_coldkey".to_owned()
            ])
        );
        for request in observed.iter().filter(|request| {
            matches!(
                request.method.as_str(),
                "state_call" | "state_queryStorageAt" | "state_getStorage"
            )
        }) {
            assert_eq!(
                request.params.as_array().unwrap().last(),
                Some(&json!(hash))
            );
        }
        assert!(
            !observed
                .iter()
                .any(|request| request.method.starts_with("author_"))
        );
        client.close().await;
    })
    .await
    .unwrap();
}
