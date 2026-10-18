// Derived from RaoFoundation/subtensor sdk/python/tests/e2e/test_emission_weights_cli.py @ 6a972693ae9580541225bd37696ac2fee26b8ae9 (Apache-2.0); modified by rtao.
//! Live emission weights: the read, root's baseline share paid off the top of
//! the validator slot, the owner intent's floor and the staking-growth flag.

use rtao_client::{BlockRef, View, ViewExt};
use rtao_intents::{SetRejectStakingHotkeyGrowth, SetSubnetEmissionWeights};
use rtao_primitives::{NetUid, TaoBalance, emission_weights as ew};
use rtao_reads::reads::subnets::subnet_emission_weights;
use rtao_runtime::{
    calls::{admin_utils, subtensor_module},
    storage::subtensor_module as st,
};
use rtao_signer::Signer;
use rtao_testkit::{Localnet, Scenario};
use rtao_tx::{ExecuteOptions, Executor, PlanOptions};
use serde_json::{Value, json};
use std::{error::Error, time::Duration};

type TestResult = Result<(), Box<dyn Error>>;

/// Root TAO that gives root a positive baseline share of the new subnet.
const ROOT_STAKE: TaoBalance = TaoBalance(1_000_000_000_000);

fn options() -> ExecuteOptions {
    ExecuteOptions {
        deadline: Some(Duration::from_secs(240)),
        ..Default::default()
    }
}

fn close(value: &Value, expected: f64) -> bool {
    value
        .as_f64()
        .is_some_and(|value| (value - expected).abs() <= 1e-9)
}

fn intent(args: Value) -> Result<SetSubnetEmissionWeights, Box<dyn Error>> {
    match rtao_intents::AnyIntent::from_tool("set_subnet_emission_weights", args)? {
        rtao_intents::AnyIntent::SetSubnetEmissionWeights(intent) => Ok(intent),
        other => Err(format!("unexpected intent {other:?}").into()),
    }
}

// pyt: e2e/test_emission_weights_cli.py::test_root_is_paid_its_baseline_off_the_top_via_python_sdk
pub async fn run(ln: &Localnet) -> TestResult {
    let setup = Scenario::new(ln).owner().subnet().start().build().await?;
    let netuid = setup.netuid.ok_or("scenario did not register a subnet")?;
    let owner = &setup.slot.owner;
    let client = ln.client();
    // Root staking needs its subtoken; enabling it is idempotent.
    let report = ln
        .sudo(
            admin_utils::sudo_set_subtoken_enabled(NetUid::ROOT, true)
                .compose(&client.runtime().await?)?,
        )
        .await?;
    assert!(report.success, "enable root subtoken: {report:?}");
    assert!(
        client
            .at(BlockRef::Best)
            .await?
            .storage(&st::SUBTOKEN_ENABLED, (NetUid::ROOT,))
            .await?,
        "root subtoken stays disabled after sudo"
    );
    let root_tao = client
        .at(BlockRef::Best)
        .await?
        .storage(&st::SUBNET_TAO, (NetUid::ROOT,))
        .await?;
    if root_tao.0 < ROOT_STAKE.0 {
        let call =
            subtensor_module::add_stake(setup.slot.hotkey.account(), NetUid::ROOT, ROOT_STAKE)
                .compose(&client.runtime().await?)?;
        let report = ln.submit_admin(call, owner).await?;
        assert!(report.success, "root stake: {report:?}");
    }

    let fresh = subnet_emission_weights(&client.at(BlockRef::Best).await?, netuid)
        .await?
        .ok_or("new subnet has no emission weights")?;
    assert_eq!(fresh.weights, ew::default_weights(ew::DEFAULT_OWNER));
    assert!(
        fresh.baseline_root_parts > 0,
        "root stake gives a positive baseline"
    );
    assert_eq!(fresh.min_validator, fresh.baseline_root_parts + 1);
    // On the default vector root earns exactly the baseline and is not capped.
    assert!((fresh.root_share - fresh.baseline_root_share).abs() <= 1e-9);
    assert!(!fresh.root_capped);
    assert!(fresh.rate_limit().owner_can_change_now);

    let executor = Executor::new(client.clone());
    // Below the floor: the plan names the chain rule before anything is signed.
    let below = intent(json!({
        "netuid": netuid.0,
        "slots": ["validator"],
        "values": [fresh.min_validator.saturating_sub(1_000_000_000_000_000)],
        "renormalize": true,
    }))?;
    let plan = executor
        .plan_dyn(&below, owner, &PlanOptions::default())
        .await?;
    assert!(!plan.ok());
    assert!(
        plan.violations[0].starts_with("ValidatorEmissionTooLowToPreserveRoot"),
        "{:?}",
        plan.violations
    );
    assert_eq!(plan.preview["root"]["share_after"], Value::Null);

    // Raise the validator slot: root keeps its baseline, validators get the rest.
    let raise = intent(json!({
        "netuid": netuid.0,
        "slots": ["validator"],
        "values": ["45%"],
        "renormalize": true,
    }))?;
    let plan = executor
        .plan_dyn(&raise, owner, &PlanOptions::default())
        .await?;
    assert!(plan.ok(), "{:?}", plan.violations);
    assert!(close(
        &plan.preview["root"]["share_after"],
        fresh.baseline_root_share
    ));
    assert!(close(
        &plan.preview["root"]["validators_after"],
        0.45 - fresh.baseline_root_share
    ));
    let result = executor.execute(&raise, owner, &options()).await?;
    assert!(result.success, "raise validator slot: {result:?}");

    let after = subnet_emission_weights(&client.at(BlockRef::Best).await?, netuid)
        .await?
        .ok_or("subnet vanished")?;
    assert!((after.shares.validator - 0.45).abs() <= 1e-9);
    // The baseline moves with root TAO, alpha issuance and TaoWeight, never with
    // the vector; a new subnet's alpha issuance grows quickly every block, so
    // check the baseline invariants at the block of each read instead.
    assert!(after.baseline_root_parts > 0);
    assert!((after.root_share - after.baseline_root_share).abs() <= 1e-9);
    assert!(
        (after.validator_share_after_root - (0.45 - after.root_share)).abs() <= 1e-9,
        "validators receive the validator slot after root's share"
    );
    assert!(!after.root_capped);
    assert!(after.last_change_block > 0);
    // The owner's weekly window now blocks a second change before signing.
    let restore = intent(json!({
        "netuid": netuid.0,
        "slots": ["validator"],
        "values": [ew::default_weights(ew::DEFAULT_OWNER)[ew::VALIDATOR]],
        "renormalize": true,
    }))?;
    let plan = executor
        .plan_dyn(&restore, owner, &PlanOptions::default())
        .await?;
    if !after.rate_limit().owner_can_change_now {
        assert!(
            !plan.ok(),
            "rate-limited owner change was planned: {plan:?}"
        );
    }

    // The staking-growth flag is an account setting on the signer's coldkey.
    let flag = SetRejectStakingHotkeyGrowth { enabled: true };
    let result = executor.execute(&flag, owner, &options()).await?;
    assert!(result.success, "reject staking hotkey growth: {result:?}");
    Ok(())
}
