//! M4 owner reads through actual pinned metadata, SCALE state and dispatch.
#![allow(clippy::unwrap_used, clippy::panic)] // Invalid authenticated fixtures must fail these assertions.

use std::collections::BTreeSet;

use rtao_client::{BlockRef, Client, ClientConfig, Network};
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
