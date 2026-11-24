// Derived from RaoFoundation/subtensor sdk/python/tests/unit/test_emission_weights.py @ d967d13588834f4f5aaa8ef4c8fc1a865d581462 (Apache-2.0); modified by rtao.
//! Emission weight intents planned through the real executor on FakeChain.
#![cfg(all(feature = "emission_weights", feature = "staking"))]
#![allow(clippy::unwrap_used, clippy::panic)] // Assertions must fail when fake-chain fixtures are invalid.

use anyhow::Result;
use rtao_client::{BlockRef, Client, ClientConfig, Network};
use rtao_intents::{AnyIntent, SetRejectStakingHotkeyGrowth, SetSubnetEmissionWeights};
use rtao_primitives::{
    AccountId, AlphaBalance, DefaultAccount, NetUid, TaoBalance,
    emission_weights::{self as ew, DEFAULT_OWNER, ONE},
};
use rtao_reads::reads::subnets::emission_weights_rate_limit_key;
use rtao_runtime::{
    calls::{admin_utils, subtensor_module},
    storage::{subtensor_module as st, swap},
};
use rtao_signer::{BoxFuture, DynKeySource, KeyView, Signer, SignerError, SignerRole};
use rtao_testkit::{
    FakeChain, FakeState,
    fixtures::catalog_samples::{
        COLD, EMISSION_ALPHA_ISSUANCE, EMISSION_ROOT_TAO, EMISSION_TAO_WEIGHT, HOT, SUBNET,
        emission_root_baseline_share, emission_weights_info, intent_state,
    },
    rpc::RpcClient,
};
use rtao_tx::{Executor, Intent, IntentInterface, Plan, PlanOptions, PreflightCx, SystemClock};
use serde_json::{Value, json};
use std::sync::Arc;

struct PublicKeys;

impl KeyView for PublicKeys {
    fn hotkey_address(&self) -> Result<AccountId, SignerError> {
        Ok(HOT)
    }
    fn hotkey_public_key(&self) -> Result<[u8; 32], SignerError> {
        Ok(HOT.0)
    }
    fn coldkey_address(&self) -> Result<AccountId, SignerError> {
        Ok(COLD)
    }
}

impl DynKeySource for PublicKeys {
    #[allow(clippy::panic)] // Planning must never acquire a private signer.
    fn signer_for<'a>(
        &'a self,
        role: SignerRole,
    ) -> BoxFuture<'a, Result<Arc<dyn Signer>, SignerError>> {
        panic!("planning attempted to acquire a private {role} signer");
    }
}

fn baseline() -> f64 {
    ew::share_to_float(emission_root_baseline_share())
}

fn min_validator() -> u64 {
    ew::min_validator_parts(emission_root_baseline_share())
}

fn share(text: &str) -> u64 {
    ew::parse_share(text).unwrap()
}

fn rate_key() -> rtao_core::DynValue {
    emission_weights_rate_limit_key(SUBNET)
}

// py: tests/unit/test_emission_weights.py:408-417 (_client)
fn state() -> Result<FakeState> {
    let runtime = FakeState::try_default()?.runtime;
    let mut state = intent_state(runtime, "set_subnet_emission_weights")?;
    state.seed(&st::TAO_WEIGHT, (), EMISSION_TAO_WEIGHT)?;
    state.seed(
        &st::SUBNET_TAO,
        (NetUid::ROOT,),
        TaoBalance(EMISSION_ROOT_TAO),
    )?;
    state.seed(&st::SUBNET_ALPHA_IN, (SUBNET,), AlphaBalance(0))?;
    state.seed(
        &st::SUBNET_ALPHA_OUT,
        (SUBNET,),
        AlphaBalance(EMISSION_ALPHA_ISSUANCE),
    )?;
    state.seed(&swap::BALANCER_ALPHA_RESERVOIR, (SUBNET,), AlphaBalance(0))?;
    state.seed(&st::LAST_RATE_LIMITED_BLOCK, (rate_key(),), 0u64)?;
    Ok(state)
}

async fn client(state: FakeState) -> Result<(Client, FakeChain)> {
    let fake = FakeChain::from_state(state);
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    Ok((
        Client::from_rpc(RpcClient::new(fake.clone()), config).await?,
        fake,
    ))
}

fn intent(args: Value) -> SetSubnetEmissionWeights {
    match AnyIntent::from_tool("set_subnet_emission_weights", args).unwrap() {
        AnyIntent::SetSubnetEmissionWeights(intent) => intent,
        other => panic!("unexpected intent {other:?}"),
    }
}

fn expected_emission_call(
    runtime: &rtao_runtime::RuntimeHandle,
    uids: Vec<u16>,
    values: Vec<u64>,
    base: ew::Weights,
    gamma: &[(u16, u64)],
) -> Result<Vec<u8>> {
    if !<SetSubnetEmissionWeights as rtao_tx::IntentInterface>::SPEC
        .wraps
        .contains(&("AdminUtils", "sudo_set_subnet_emission_weights_v2"))
    {
        return Ok(
            admin_utils::sudo_set_subnet_emission_weights(SUBNET, uids, values)
                .compose(runtime)?
                .as_bytes()
                .to_vec(),
        );
    }
    let mut weights = base;
    for (slot, value) in uids.into_iter().zip(values) {
        weights[usize::from(slot)] = value;
    }
    let mut bytes = vec![
        runtime.pallet_index("AdminUtils").unwrap(),
        runtime
            .call_index("AdminUtils", "sudo_set_subnet_emission_weights_v2")
            .unwrap(),
    ];
    bytes.extend(SUBNET.0.to_le_bytes());
    bytes.push(u8::try_from((5 + gamma.len()) << 2)?);
    for (slot, value) in weights.into_iter().enumerate() {
        bytes.push(u8::try_from(slot)?);
        bytes.extend(value.to_le_bytes());
    }
    for (target, share) in gamma {
        bytes.push(5);
        bytes.extend(target.to_le_bytes());
        bytes.extend(share.to_le_bytes());
    }
    Ok(bytes)
}

async fn plan(client: &Client, intent: &SetSubnetEmissionWeights) -> Plan {
    Executor::new(client.clone())
        .plan_dyn(intent, &PublicKeys, &PlanOptions::default())
        .await
        .unwrap()
}

// pyt: unit/test_emission_weights.py::test_intent_normalises_slots_and_shares
#[test]
fn intent_normalises_slots_and_shares() {
    let intent = intent(json!({"netuid": 1, "slots": ["owner", "3"], "values": ["10%", 0.45]}));
    assert_eq!(intent.slots, [json!(2), json!(3)]);
    assert_eq!(
        intent.values,
        [json!(share("10%")), json!(ew::parse_share(0.45).unwrap())]
    );
    assert!(!intent.renormalize);
    assert!(intent.summary().contains("owner=10.00%"));
    assert_eq!(
        intent.summary(),
        "set emission weights on netuid 1: owner=10.00%, validator=45.00%"
    );
    // The canonical arguments roundtrip unchanged.
    let json = rtao_tx::IntentInterface::to_json(&intent);
    assert_eq!(json["slots"], json!([2, 3]));
    assert_eq!(
        AnyIntent::from_tool("set_subnet_emission_weights", json).unwrap(),
        AnyIntent::SetSubnetEmissionWeights(intent)
    );
}

// pyt: unit/test_emission_weights.py::test_intent_rejects_malformed_input
#[test]
fn intent_rejects_malformed_input() {
    for (args, expected) in [
        (json!({"slots": ["owner"], "values": []}), "pair up"),
        (json!({"slots": [], "values": []}), "at least one"),
        (
            json!({"slots": ["owner", 2], "values": [1, 2]}),
            "at most once",
        ),
        (json!({"slots": ["gamma"], "values": [1]}), "slots are"),
        (
            json!({"slots": [], "values": [], "gamma": {"x": "5%"}}),
            "not a netuid",
        ),
        (
            json!({"slots": [], "values": [], "gamma": {"70000": "5%"}}),
            "not a netuid",
        ),
        (
            json!({"slots": [], "values": [], "gamma": {"3.7": "5%"}}),
            "not a netuid",
        ),
        (
            json!({"slots": [], "values": [], "gamma": {"03": "5%", "3": "1%"}}),
            "at most once",
        ),
        (
            json!({"slots": ["owner"], "values": [true]}),
            "not a boolean",
        ),
    ] {
        let mut args = args;
        args["netuid"] = json!(1);
        let error = AnyIntent::from_tool("set_subnet_emission_weights", args)
            .unwrap_err()
            .to_string();
        assert!(error.contains(expected), "{error}");
    }
    let unchecked: SetSubnetEmissionWeights = serde_json::from_value(json!({
        "netuid": 1, "slots": ["owner"], "values": ["lots"], "renormalize": false,
    }))
    .unwrap();
    assert!(unchecked.validate().is_err());
}

// pyt: unit/test_emission_weights.py::test_intent_accepts_nonzero_burn
#[test]
fn intent_accepts_nonzero_burn() {
    let intent =
        intent(json!({"netuid": 1, "slots": ["burn"], "values": ["10%"], "renormalize": true}));
    assert_eq!(intent.slots, [json!(0)]);
    assert_eq!(intent.values, [json!(share("10%"))]);
    assert!(intent.summary().contains("burn=10.00%"));
}

// pyt: unit/test_emission_weights.py::test_intent_plan_renormalizes_and_reports_root_baseline
#[tokio::test]
async fn intent_plan_renormalizes_and_reports_root_baseline() -> Result<()> {
    let (client, fake) = client(state()?).await?;
    let baseline = baseline();
    let set = intent(
        json!({"netuid": SUBNET.0, "slots": ["validator"], "values": ["30%"], "renormalize": true}),
    );
    let plan = plan(&client, &set).await;
    assert!(plan.ok(), "{:?}", plan.violations);
    assert!(
        plan.effects
            .iter()
            .any(|line| line.contains("validator 41.00%") && line.contains("validator 30.00%")),
        "{:?}",
        plan.effects
    );
    let validators_before =
        ew::share_to_float(ew::default_weights(DEFAULT_OWNER)[ew::VALIDATOR]) - baseline;
    assert!(
        plan.effects.iter().any(|line| {
            line.contains(&format!(
                "root keeps its baseline {:.4}% of this subnet's emission",
                baseline * 100.0
            )) && line.contains(&format!(
                "alpha validators get {:.4}% -> {:.4}%",
                validators_before * 100.0,
                (0.30 - baseline) * 100.0
            ))
        }),
        "{:?}",
        plan.effects
    );
    let preview = &plan.preview;
    assert_eq!(
        preview["after"]["weights"][ew::VALIDATOR],
        json!(share("30%"))
    );
    let close = |value: &Value, expected: f64| (value.as_f64().unwrap() - expected).abs() < 1e-9;
    assert!(close(&preview["root"]["share_after"], baseline));
    assert!(close(&preview["root"]["baseline_share"], baseline));
    assert!(close(&preview["root"]["validators_after"], 0.30 - baseline));
    assert_eq!(preview["root"]["capped_after"], json!(false));
    assert_eq!(preview["rate_limit"]["owner_can_change_now"], json!(true));
    assert_eq!(
        preview["limits"]["validator"]["min_parts"],
        json!(min_validator())
    );
    assert_eq!(plan.to_json()["preview"], Value::Object(preview.clone()));

    // At or below the live baseline the plan is blocked before signing.
    let too_low = intent(
        json!({"netuid": SUBNET.0, "slots": ["validator"], "values": ["1%"], "renormalize": true}),
    );
    let plan = self::plan(&client, &too_low).await;
    assert!(!plan.ok());
    assert!(plan.violations[0].starts_with("ValidatorEmissionTooLowToPreserveRoot"));
    let floor = ew::format_share(min_validator() - 1, 4);
    assert!(plan.violations[0].contains(&format!("must be above {floor}")));
    assert!(
        plan.effects
            .iter()
            .any(|line| line.contains("validators would get nothing (the chain rejects)"))
    );
    assert_eq!(plan.preview["root"]["share_after"], Value::Null);

    // One part above the baseline is the lowest the chain takes.
    let lowest = intent(
        json!({"netuid": SUBNET.0, "slots": ["validator"], "values": [min_validator()], "renormalize": true}),
    );
    let plan = self::plan(&client, &lowest).await;
    assert!(plan.ok(), "{:?}", plan.violations);
    assert!(close(&plan.preview["root"]["share_after"], baseline));
    assert!(
        plan.preview["root"]["validators_after"]
            .as_f64()
            .unwrap()
            .abs()
            < 1e-12
    );
    assert!(fake.mutate_state(|state| state.submissions.is_empty()));
    client.close().await;
    Ok(())
}

// pyt: unit/test_emission_weights.py::test_intent_plan_blocks_on_chain_rules
#[tokio::test]
async fn intent_plan_blocks_on_chain_rules() -> Result<()> {
    let (client, _) = client(state()?).await?;
    let owner = intent(
        json!({"netuid": SUBNET.0, "slots": ["owner"], "values": ["40%"], "renormalize": true}),
    );
    let plan = plan(&client, &owner).await;
    assert!(!plan.ok());
    assert!(plan.violations[0].starts_with("OwnerEmissionAboveMaximum"));
    let verbatim = intent(json!({"netuid": SUBNET.0, "slots": ["owner"], "values": ["10%"]}));
    let plan = self::plan(&client, &verbatim).await;
    assert_eq!(
        plan.violations
            .iter()
            .map(|violation| violation.split(':').next().unwrap())
            .collect::<Vec<_>>(),
        ["EmissionWeightsDoNotSumToOne"]
    );
    client.close().await;
    Ok(())
}

// pyt: unit/test_emission_weights.py::test_intent_plan_blocks_inside_the_owner_rate_limit
#[tokio::test]
async fn intent_plan_blocks_inside_the_owner_rate_limit() -> Result<()> {
    let mut state = state()?;
    // A head beyond one window lets the passed-window case stay positive.
    state.block = 60_000;
    state.set_finalized(60_000)?;
    let head = state.block;
    state.seed(&st::SUBNET_EMISSION_WEIGHTS_RATE_LIMIT, (), 360u64)?;
    state.seed(&st::LAST_RATE_LIMITED_BLOCK, (rate_key(),), head - 10)?;
    let (client, fake) = client(state).await?;
    let set = intent(
        json!({"netuid": SUBNET.0, "slots": ["owner"], "values": ["10%"], "renormalize": true}),
    );
    let plan = plan(&client, &set).await;
    assert!(!plan.ok());
    let [blocked] = plan.violations.as_slice() else {
        panic!("expected one block: {:?}", plan.violations);
    };
    assert!(
        blocked.contains(&format!(
            "block {}",
            rtao_primitives::pyfmt::fmt_grouped(u128::from(head - 10))
        )),
        "{blocked}"
    );
    assert!(blocked.contains(&format!(
        "block {}",
        rtao_primitives::pyfmt::fmt_grouped(u128::from(head - 10 + 360))
    )));
    assert!(blocked.contains("root is exempt"));
    assert_eq!(
        plan.preview["rate_limit"]["blocks_until_allowed"],
        json!(350)
    );

    // The window has passed: allowed again.
    fake.mutate_state(|state| {
        state
            .seed(&st::LAST_RATE_LIMITED_BLOCK, (rate_key(),), head - 360)
            .unwrap();
    });
    let plan = self::plan(&client, &set).await;
    assert!(plan.ok(), "{:?}", plan.violations);
    fake.mutate_state(|state| {
        state
            .seed(&st::LAST_RATE_LIMITED_BLOCK, (rate_key(),), head)
            .unwrap();
        state
            .seed(&st::SUBNET_EMISSION_WEIGHTS_RATE_LIMIT, (), 0u64)
            .unwrap();
    });
    let plan = self::plan(&client, &set).await;
    assert!(plan.ok(), "{:?}", plan.violations);
    assert_eq!(plan.preview["rate_limit"]["limit_blocks"], json!(0));
    client.close().await;
    Ok(())
}

// pyt: unit/test_emission_weights.py::test_intent_plan_blocks_a_signer_who_is_not_the_owner
#[tokio::test]
async fn intent_plan_blocks_a_signer_who_is_not_the_owner() -> Result<()> {
    let other = AccountId([9; 32]);
    let mut state = state()?;
    state.seed(&st::SUBNET_OWNER, (SUBNET,), DefaultAccount::from(other))?;
    let (client, fake) = client(state).await?;
    let set = intent(
        json!({"netuid": SUBNET.0, "slots": ["owner"], "values": ["10%"], "renormalize": true}),
    );
    let plan = plan(&client, &set).await;
    assert!(!plan.ok());
    let [blocked] = plan.violations.as_slice() else {
        panic!("expected one block: {:?}", plan.violations);
    };
    assert!(blocked.contains(&COLD.to_string()) && blocked.contains(&other.to_string()));
    assert!(blocked.contains(&format!("is not the owner of subnet {SUBNET}")));
    assert!(blocked.contains("root through sudo"));
    fake.mutate_state(|state| {
        state
            .seed(&st::SUBNET_OWNER, (SUBNET,), DefaultAccount::from(COLD))
            .unwrap();
    });
    let plan = self::plan(&client, &set).await;
    assert!(plan.ok(), "{:?}", plan.violations);
    client.close().await;
    Ok(())
}

// pyt: unit/test_emission_weights.py::test_intent_plan_names_a_missing_subnet
#[tokio::test]
async fn intent_plan_names_a_missing_subnet() -> Result<()> {
    let mut state = state()?;
    state.seed_runtime(
        "SubnetInfoRuntimeApi",
        "get_subnet_emission_weights",
        Value::Null,
    )?;
    let (client, _) = client(state).await?;
    let set =
        intent(json!({"netuid": 9, "slots": ["owner"], "values": ["10%"], "renormalize": true}));
    let plan = plan(&client, &set).await;
    assert!(!plan.ok());
    assert!(plan.violations[0].contains("subnet 9 does not exist"));
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn intent_plan_rejects_root_even_when_runtime_answers() -> Result<()> {
    let (client, _) = client(state()?).await?;
    let set = intent(json!({
        "netuid": 0, "slots": ["owner"], "values": ["10%"], "renormalize": true
    }));
    let plan = self::plan(&client, &set).await;
    assert_eq!(
        plan.violations,
        ["subnet 0 does not exist or has no emission vector"]
    );
    client.close().await;
    Ok(())
}

// pyt: unit/test_emission_weights.py::test_intent_build_sends_full_vector_when_renormalizing
#[tokio::test]
async fn intent_build_sends_full_vector_when_renormalizing() -> Result<()> {
    let (client, _) = client(state()?).await?;
    let runtime = client.refresh_runtime().await?;
    let owner = share("10%");
    let set = intent(
        json!({"netuid": SUBNET.0, "slots": ["owner"], "values": ["10%"], "renormalize": true}),
    );
    let expected = ew::renormalize(&ew::default_weights(DEFAULT_OWNER), &[(ew::OWNER, owner)])?;
    assert_eq!(
        expected.iter().map(|w| u128::from(*w)).sum::<u128>(),
        u128::from(ONE)
    );
    let plan = plan(&client, &set).await;
    assert_eq!(
        plan.call().as_bytes(),
        expected_emission_call(
            &runtime,
            vec![0, 1, 2, 3, 4],
            expected.to_vec(),
            ew::default_weights(DEFAULT_OWNER),
            &[],
        )?
    );
    let sparse =
        intent(json!({"netuid": SUBNET.0, "slots": ["miner", "owner"], "values": [ONE / 2, 1]}));
    let plan = self::plan(&client, &sparse).await;
    assert_eq!(
        plan.call().as_bytes(),
        expected_emission_call(
            &runtime,
            vec![2, 4],
            vec![1, ONE / 2],
            ew::default_weights(DEFAULT_OWNER),
            &[],
        )?
    );
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn intent_accepts_full_percentages_with_or_without_renormalization() -> Result<()> {
    let (client, _) = client(state()?).await?;
    for renormalize in [false, true] {
        let set = intent(json!({
            "netuid": SUBNET.0,
            "slots": ["burn", "recycle", "owner", "validator", "miner"],
            "values": ["0", "0", "18%", "41%", "41%"],
            "renormalize": renormalize,
        }));
        assert!(!set.summary().contains("renormalizing"));
        let plan = self::plan(&client, &set).await;
        assert!(plan.ok(), "{:?}", plan.violations);
        let weights: ew::Weights =
            serde_json::from_value(plan.preview["after"]["weights"].clone())?;
        assert_eq!(total(&weights), u128::from(ONE));
    }
    client.close().await;
    Ok(())
}

/// The fixture subnet after its owner gave 10% of its emission, from the
/// validator slot, to subnet 2's gamma.
fn gamma_share() -> u64 {
    share("10%")
}

fn gamma_weights() -> ew::Weights {
    let mut weights = ew::default_weights(DEFAULT_OWNER);
    weights[ew::VALIDATOR] -= gamma_share();
    weights
}

// py: tests/unit/test_emission_weights.py:579-591 (GAMMA_SHARE, _seed_gamma_share)
fn seed_gamma_share(state: &mut FakeState) -> Result<()> {
    let weights = gamma_weights();
    let mut info = emission_weights_info(SUBNET);
    info["weights"] = json!(weights);
    info["validator"] = json!(weights[ew::VALIDATOR]);
    state.seed_runtime("SubnetInfoRuntimeApi", "get_subnet_emission_weights", info)?;
    state.seed(
        &st::SUBNET_GAMMA_EMISSION,
        (SUBNET,),
        vec![(NetUid(2), gamma_share())],
    )?;
    state.seed(&st::SUBNET_GAMMA_EMISSION_WHITELISTED, (NetUid(2),), true)?;
    state.seed(&st::SUBNET_GAMMA_ENABLED, (NetUid(2),), true)?;
    Ok(())
}

fn seed_gamma_target(state: &mut FakeState, target: NetUid, enabled: bool) -> Result<()> {
    state.seed(&st::SUBNET_GAMMA_EMISSION_WHITELISTED, (target,), true)?;
    state.seed(&st::SUBNET_GAMMA_ENABLED, (target,), enabled)?;
    Ok(())
}

fn total(weights: &[u64]) -> u128 {
    weights.iter().map(|w| u128::from(*w)).sum()
}

// pyt: unit/test_emission_weights.py::test_intent_counts_the_gamma_shares_in_the_sum
#[tokio::test]
async fn intent_counts_the_gamma_shares_in_the_sum() -> Result<()> {
    let mut state = state()?;
    seed_gamma_share(&mut state)?;
    let (client, _) = client(state).await?;
    let runtime = client.refresh_runtime().await?;
    let owner = intent(
        json!({"netuid": SUBNET.0, "slots": ["owner"], "values": ["10%"], "renormalize": true}),
    );
    let plan = plan(&client, &owner).await;
    assert!(plan.ok(), "{:?}", plan.violations);
    let after: Vec<u64> = serde_json::from_value(plan.preview["after"]["weights"].clone())?;
    assert_eq!(total(&after), u128::from(ONE - gamma_share()));
    let expected = ew::renormalize_with_gamma(
        &gamma_weights(),
        &[(ew::OWNER, share("10%"))],
        gamma_share(),
    )?;
    assert_eq!(after, expected);
    assert_eq!(
        plan.call().as_bytes(),
        expected_emission_call(
            &runtime,
            vec![0, 1, 2, 3, 4],
            expected.to_vec(),
            gamma_weights(),
            &[(2, gamma_share())],
        )?
    );

    // A verbatim update that keeps the sum passes; one that fills 100% alone does not.
    let weights = gamma_weights();
    let moved = intent(json!({
        "netuid": SUBNET.0,
        "slots": ["validator", "miner"],
        "values": [weights[ew::VALIDATOR] + 1, weights[ew::MINER] - 1],
    }));
    let plan = self::plan(&client, &moved).await;
    assert!(plan.ok(), "{:?}", plan.violations);
    let full = intent(json!({
        "netuid": SUBNET.0,
        "slots": ["validator"],
        "values": [ew::default_weights(DEFAULT_OWNER)[ew::VALIDATOR]],
    }));
    let plan = self::plan(&client, &full).await;
    assert_eq!(
        plan.violations
            .iter()
            .map(|violation| violation.split(':').next().unwrap())
            .collect::<Vec<_>>(),
        ["EmissionWeightsDoNotSumToOne"]
    );
    assert!(
        plan.violations[0].contains("and the gamma shares (10.00%)"),
        "{}",
        plan.violations[0]
    );
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn intent_adds_and_removes_gamma_shares_in_the_full_split() -> Result<()> {
    let mut state = state()?;
    seed_gamma_share(&mut state)?;
    seed_gamma_target(&mut state, NetUid(3), true)?;
    let (client, fake) = client(state).await?;
    let runtime = client.refresh_runtime().await?;
    let add = intent(json!({
        "netuid": SUBNET.0, "slots": [], "values": [], "renormalize": true,
        "gamma": {"3": "5%"}
    }));
    assert_eq!(add.touches_netuids().as_slice(), &[SUBNET, NetUid(3)]);
    let mut self_args = json!({
        "netuid": SUBNET.0, "slots": [], "values": [], "gamma": {"3": "1%"}
    });
    self_args["gamma"][SUBNET.0.to_string()] = json!("5%");
    let self_target = intent(self_args);
    assert_eq!(
        self_target.touches_netuids().as_slice(),
        &[SUBNET, NetUid(3)]
    );
    let plan = self::plan(&client, &add).await;
    assert!(plan.ok(), "{:?}", plan.violations);
    let weights: ew::Weights = serde_json::from_value(plan.preview["after"]["weights"].clone())?;
    assert_eq!(
        total(&weights),
        u128::from(ONE - gamma_share() - share("5%"))
    );
    assert_eq!(plan.preview["gamma"]["before"], json!({"2": gamma_share()}));
    assert_eq!(
        plan.preview["gamma"]["after"],
        json!({"2": gamma_share(), "3": share("5%")})
    );
    assert_eq!(
        plan.call().as_bytes(),
        expected_emission_call(
            &runtime,
            (0..5).collect(),
            weights.to_vec(),
            gamma_weights(),
            &[(2, gamma_share()), (3, share("5%"))],
        )?
    );
    let remove = intent(json!({
        "netuid": SUBNET.0, "slots": [], "values": [], "renormalize": true,
        "gamma": {"2": 0}
    }));
    let plan = self::plan(&client, &remove).await;
    assert!(plan.ok(), "{:?}", plan.violations);
    assert_eq!(plan.preview["gamma"]["after"], json!({}));
    assert!(plan.effects.iter().any(|line| {
        line.contains("removed gamma shares: their unconverted alpha goes to the miners")
    }));
    assert_eq!(
        plan.call().as_bytes(),
        expected_emission_call(
            &runtime,
            (0..5).collect(),
            serde_json::from_value::<ew::Weights>(plan.preview["after"]["weights"].clone())?
                .to_vec(),
            gamma_weights(),
            &[],
        )?
    );
    fake.mutate_state(|state| {
        state
            .seed(&st::SUBNET_GAMMA_ENABLED, (NetUid(2),), false)
            .unwrap();
    });
    let plan = self::plan(&client, &add).await;
    assert!(
        plan.violations
            .iter()
            .any(|line| line.contains("GammaDisabled"))
    );
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn intent_preflight_rejects_a_split_changed_after_build() -> Result<()> {
    let (client, fake) = client(state()?).await?;
    let set = intent(json!({
        "netuid": SUBNET.0, "slots": ["owner"], "values": ["10%"], "renormalize": true
    }));
    let plan = self::plan(&client, &set).await;
    assert!(plan.ok(), "{:?}", plan.violations);
    let mut changed = ew::default_weights(DEFAULT_OWNER);
    changed[ew::VALIDATOR] -= share("1%");
    changed[ew::MINER] += share("1%");
    let mut info = emission_weights_info(SUBNET);
    info["weights"] = json!(changed);
    info["validator"] = json!(changed[ew::VALIDATOR]);
    fake.mutate_state(|state| {
        state
            .seed_runtime("SubnetInfoRuntimeApi", "get_subnet_emission_weights", info)
            .unwrap();
    });
    let snapshot = client.at(BlockRef::Finalized).await?;
    let runtime = client.refresh_runtime().await?;
    let preview = set
        .preflight(&PreflightCx {
            view: &snapshot,
            runtime: &runtime,
            clock: &SystemClock,
            spec: SetSubnetEmissionWeights::SPEC,
            client: &client,
            dispatch_origin: COLD,
            fee_payer: COLD,
            call: plan.call(),
        })
        .await?;
    assert_eq!(
        preview.blocks,
        [format!(
            "the emission split of subnet {SUBNET} changed after this call was built, so the call no longer matches this preview; prepare it again"
        )]
    );
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn reject_staking_hotkey_growth_builds_the_account_flag_call() -> Result<()> {
    let (client, _) = client(state()?).await?;
    let runtime = client.refresh_runtime().await?;
    for enabled in [true, false] {
        let flag = SetRejectStakingHotkeyGrowth { enabled };
        assert_eq!(
            flag.summary(),
            format!(
                "{} staking-hotkey list growth caused by other accounts",
                if enabled { "reject" } else { "allow" }
            )
        );
        let plan = Executor::new(client.clone())
            .plan_dyn(&flag, &PublicKeys, &PlanOptions::default())
            .await?;
        assert!(plan.ok(), "{:?}", plan.violations);
        assert_eq!(
            plan.call().as_bytes(),
            subtensor_module::set_reject_staking_hotkey_growth(enabled)
                .compose(&runtime)?
                .as_bytes()
        );
    }
    client.close().await;
    Ok(())
}
