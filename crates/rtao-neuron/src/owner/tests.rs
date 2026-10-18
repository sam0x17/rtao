//! Authenticated setter coverage and pinned-state boundary regressions.

use super::*;
use rtao_catalog::owner_map::{OWNER_HYPERPARAMETERS, OwnerValueShape};
use rtao_client::{ClientConfig, Network};
use rtao_testkit::{FakeChain, FakeState, Reply, rpc::RpcClient};
use serde_json::json;
use std::time::Duration;

const NET: NetUid = NetUid(1);
const OWNER: AccountId = AccountId([7; 32]);

async fn fixture(
    setup: impl FnOnce(&mut FakeState) -> anyhow::Result<()>,
) -> anyhow::Result<(FakeChain, Client, HyperparamPlanner)> {
    let mut state = FakeState::try_default()?;
    state.seed(
        &st::SUBNET_OWNER,
        (NET,),
        rtao_primitives::DefaultAccount::from(OWNER),
    )?;
    state.seed(&st::TEMPO, (NET,), 360u16)?;
    state.seed(&st::LAST_EPOCH_BLOCK, (NET,), 100u64)?;
    state.seed(&st::PENDING_EPOCH_AT, (NET,), 0u64)?;
    state.seed(&st::ADMIN_FREEZE_WINDOW, (), 10u16)?;
    setup(&mut state)?;
    let chain = FakeChain::from_state(state);
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
    config.runtime_cache_dir = None;
    config.config_store = None;
    config.track_heads = false;
    let client = Client::from_rpc(RpcClient::new(chain.clone()), config).await?;
    Ok((chain, client, HyperparamPlanner::new(OWNER)))
}

fn input(name: &str) -> HparamArg {
    if name == "epoch_consensus" {
        return "Yuma".into();
    }
    let (minimum, _) = rtao_catalog::hyperparams::TABLE.raw_bounds(name);
    minimum.to_string().into()
}

fn seed_last(
    state: &mut FakeState,
    row: &rtao_catalog::owner_map::OwnerHyperparam,
    last: u64,
) -> anyhow::Result<()> {
    match row.cooldown {
        OwnerCooldown::None => {}
        OwnerCooldown::OwnerHyperparameter(h) => state.seed(
            &st::LAST_RATE_LIMITED_BLOCK,
            (owner_rate_key(NET, h),),
            last,
        )?,
        OwnerCooldown::Transaction(code, _) => {
            state.seed(&st::TRANSACTION_KEY_LAST_BLOCK, (OWNER, NET, code), last)?
        }
    }
    Ok(())
}

async fn setter_row(name: &str) -> anyhow::Result<()> {
    let row = owner_parameter(name)?;
    let (_, client, planner) = fixture(|state| {
        seed_last(state, row, 80)?;
        state.seed(&st::OWNER_HYPERPARAM_RATE_LIMIT, (), 2u16)?;
        state.seed(&st::WEIGHTS_VERSION_KEY_RATE_LIMIT, (), 5u64)?;
        state.seed_python(
            "SubtensorModule",
            "AlphaValues",
            &[json!(NET.0)],
            json!([1000, 60000]),
        )?;
        Ok(())
    })
    .await?;
    let plan = planner.plan(&client, NET, name, input(name)).await?;
    let expected = match row.cooldown {
        OwnerCooldown::None => 100,
        OwnerCooldown::OwnerHyperparameter(_) => 800,
        OwnerCooldown::Transaction(4, _) => 1880,
        OwnerCooldown::Transaction(11, _) => 440,
        _ => anyhow::bail!("unexpected hyperparameter cooldown"),
    };
    assert_eq!(plan.earliest_block, expected, "{name}");
    assert_eq!(
        plan.reasons.is_empty(),
        matches!(row.cooldown, OwnerCooldown::None),
        "{name}"
    );
    let descriptor = rtao_runtime::calls::ALL
        .iter()
        .find(|call| call.pallet == "AdminUtils" && call.name == row.setter)
        .ok_or_else(|| anyhow::anyhow!("missing descriptor"))?;
    assert_eq!(
        plan.call.as_bytes().get(..2),
        Some([descriptor.pallet_index, descriptor.call_index].as_slice()),
        "{name}"
    );
    match row.shape {
        OwnerValueShape::Bool => assert_eq!(plan.raw.as_bool(), Some(false), "{name}"),
        OwnerValueShape::FixedBits => assert!(plan.raw.to_json().get("bits").is_some(), "{name}"),
        OwnerValueShape::EpochConsensus => assert_eq!(plan.raw.as_str(), Some("Yuma")),
        _ => assert!(plan.raw.as_u128().is_some(), "{name}"),
    }
    client.close().await;
    Ok(())
}

macro_rules! setter_tests {
    ($($name:ident),+ $(,)?) => {$(
        #[tokio::test]
        async fn $name() -> anyhow::Result<()> { setter_row(stringify!($name)).await }
    )+};
}
setter_tests!(
    epoch_consensus,
    tempo,
    immunity_period,
    min_allowed_weights,
    weights_version,
    activity_cutoff_factor,
    min_burn,
    max_burn,
    bonds_moving_avg,
    bonds_penalty,
    serving_rate_limit,
    commit_reveal_period,
    max_allowed_uids,
    burn_increase_mult,
    burn_half_life,
    collateral_lock_share,
    collateral_drain_ratio,
    adjustment_alpha,
    rho,
    max_difficulty,
    alpha_sigmoid_steepness,
    min_childkey_take,
    owner_immune_neuron_limit,
    alpha_low,
    alpha_high,
    commit_reveal_weights_enabled,
    liquid_alpha_enabled,
    registration_allowed,
    network_pow_registration_allowed,
    yuma3_enabled,
    bonds_reset_enabled,
    transfers_enabled,
    owner_cut_auto_lock_enabled
);

#[test]
fn allowlist_has_exact_row_and_setter_counts() {
    assert_eq!(OWNER_HYPERPARAMETERS.len(), 33);
    assert_eq!(
        OWNER_HYPERPARAMETERS
            .iter()
            .map(|row| row.setter)
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        32
    );
    for (name, variant) in [
        ("commit_reveal_period", "WeightCommitInterval"),
        ("registration_allowed", "BurnRegistrationAllowed"),
        ("network_pow_registration_allowed", "PowRegistrationAllowed"),
        ("transfers_enabled", "TransferEnabled"),
        ("owner_immune_neuron_limit", "ImmuneNeuronLimit"),
        ("alpha_low", "AlphaValues"),
        ("alpha_high", "AlphaValues"),
        ("activity_cutoff_factor", "ActivityCutoffFactorMilli"),
    ] {
        assert!(
            matches!(owner_parameter(name).map(|row| row.cooldown), Ok(OwnerCooldown::OwnerHyperparameter(h)) if h == variant)
        );
    }
}

#[tokio::test]
async fn first_use_current_tempo_and_inclusive_cooldown() -> anyhow::Result<()> {
    for (tempo, last, expected, closed) in [
        (100u16, 0u64, 100u64, false),
        (100, 1, 201, true),
        (20, 61, 101, true),
        (20, 60, 100, false),
        (20, 200, 240, true),
        (0, 200, 100, false),
    ] {
        let (_, client, planner) = fixture(|state| {
            state.seed(&st::TEMPO, (NET,), tempo)?;
            state.seed(&st::ADMIN_FREEZE_WINDOW, (), 0u16)?;
            state.seed(&st::OWNER_HYPERPARAM_RATE_LIMIT, (), 2u16)?;
            seed_last(state, owner_parameter("rho")?, last)?;
            Ok(())
        })
        .await?;
        let plan = planner.plan(&client, NET, "rho", 1u16.into()).await?;
        assert_eq!(
            (plan.earliest_block, !plan.reasons.is_empty()),
            (expected, closed)
        );
        client.close().await;
    }
    Ok(())
}

#[tokio::test]
async fn alpha_shared_rate_key_preserves_other_side() -> anyhow::Result<()> {
    let (chain, client, planner) = fixture(|state| {
        state.seed_python(
            "SubtensorModule",
            "AlphaValues",
            &[json!(NET.0)],
            json!([1000, 60000]),
        )?;
        Ok(())
    })
    .await?;
    let low = planner
        .plan(&client, NET, "alpha_low", 2000u16.into())
        .await?;
    let expected = rtao_runtime::calls::admin_utils::sudo_set_alpha_values(NET, 2000, 60000)
        .compose(&client.runtime().await?)?;
    assert_eq!(low.call, expected);
    assert_eq!(low.current.as_u128(), Some(1000));
    chain.mutate_state(|state| -> anyhow::Result<()> {
        state.seed_python(
            "SubtensorModule",
            "AlphaValues",
            &[json!(NET.0)],
            json!([2000, 60000]),
        )?;
        seed_last(state, owner_parameter("alpha_low")?, 100)?;
        Ok(())
    })?;
    let high = planner
        .plan(&client, NET, "alpha_high", 61000u16.into())
        .await?;
    let expected = rtao_runtime::calls::admin_utils::sudo_set_alpha_values(NET, 2000, 61000)
        .compose(&client.runtime().await?)?;
    assert_eq!(high.call, expected);
    assert_eq!(high.earliest_block, 820);
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn freeze_equality_pending_expiry_and_zero_tempo() -> anyhow::Result<()> {
    for (tempo, epoch, pending, expected, closed) in [
        (360u16, 0u64, 0u64, 100u64, false),
        (360, 0, 110, 110, true),
        (360, 0, 100, 100, false),
        (360, 0, 99, 100, false),
        (360, 0, 0, 100, false),
        (100, 10, 0, 100, false),
        (100, 9, 0, 109, true),
        (100, 0, 0, 101, true),
        (0, 0, 200, 100, false),
    ] {
        let (_, client, planner) = fixture(|state| {
            state.seed(&st::TEMPO, (NET,), tempo)?;
            state.seed(&st::LAST_EPOCH_BLOCK, (NET,), epoch)?;
            state.seed(&st::PENDING_EPOCH_AT, (NET,), pending)?;
            Ok(())
        })
        .await?;
        for name in ["transfers_enabled", "owner_cut_auto_lock_enabled"] {
            let plan = planner.plan(&client, NET, name, true.into()).await?;
            assert_eq!(
                (plan.earliest_block, !plan.reasons.is_empty()),
                (expected, closed),
                "{tempo}/{epoch}/{pending}"
            );
        }
        client.close().await;
    }
    Ok(())
}

#[tokio::test]
async fn operation_rate_rows_and_aura_threshold() -> anyhow::Result<()> {
    for slot in [11999u64, 12000, 12001] {
        for (setter, code, production) in [
            ("sudo_set_mechanism_count", 7u16, 7200u64),
            ("sudo_set_mechanism_emission_split", 8, 7200),
            ("sudo_trim_to_max_allowed_uids", 9, 216000),
            ("sudo_trim_null_uids_batch", 9, 216000),
        ] {
            for last in [0u64, 99, 100] {
                let (_, client, planner) = fixture(|state| {
                    state.seed_constant("Aura", "SlotDuration", json!(slot))?;
                    state.seed(&st::TRANSACTION_KEY_LAST_BLOCK, (OWNER, NET, code), last)?;
                    Ok(())
                })
                .await?;
                let (earliest, reasons) = planner
                    .operation_window(&client, NET, setter, Some(10))
                    .await?;
                let limit = if slot < 12000 { 1 } else { production };
                let expected = if last == 0 || 100u64.saturating_sub(last) >= limit {
                    100
                } else {
                    last + limit
                };
                assert_eq!(earliest, expected, "{setter}/{slot}/{last}");
                assert_eq!(reasons.is_empty(), expected == 100);
                client.close().await;
            }
        }
    }
    Ok(())
}

#[tokio::test]
async fn null_continuation_bypasses_cooldown_but_keeps_freeze() -> anyhow::Result<()> {
    let (chain, client, planner) = fixture(|state| {
        state.seed_python(
            "SubtensorModule",
            "NullPruningTarget",
            &[json!(NET.0)],
            json!(10),
        )?;
        state.seed(&st::TRANSACTION_KEY_LAST_BLOCK, (OWNER, NET, 9), 100u64)?;
        Ok(())
    })
    .await?;
    assert_eq!(
        planner
            .operation_window(&client, NET, "sudo_trim_null_uids_batch", Some(10))
            .await?,
        (100, vec![])
    );
    assert_eq!(
        planner
            .operation_window(&client, NET, "sudo_trim_null_uids_batch", Some(11))
            .await?
            .0,
        216100
    );
    chain.mutate_state(|state| state.seed(&st::PENDING_EPOCH_AT, (NET,), 110u64))?;
    let frozen = planner
        .operation_window(&client, NET, "sudo_trim_null_uids_batch", Some(10))
        .await?;
    assert_eq!(frozen.0, 110);
    assert_eq!(frozen.1.len(), 1);
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn invalid_owner_name_value_and_legacy_calls_fail() -> anyhow::Result<()> {
    let (_, client, planner) = fixture(|_| Ok(())).await?;
    for name in [
        "kappa",
        "activity_cutoff",
        "sn_owner_hotkey",
        "recycle_or_burn",
        "set_tempo",
        // Derived from the emission weight vector since spec 481.
        "owner_cut_enabled",
    ] {
        assert!(planner.plan(&client, NET, name, 1u16.into()).await.is_err());
    }
    for (name, value) in [
        ("tempo", HparamArg::from(359u16)),
        ("tempo", 50401u16.into()),
        ("activity_cutoff_factor", 999u16.into()),
        ("activity_cutoff_factor", 50001u32.into()),
        ("collateral_lock_share", 62259u16.into()),
        ("epoch_consensus", "yuma".into()),
        ("owner_cut_auto_lock_enabled", 2u16.into()),
    ] {
        assert!(
            planner.plan(&client, NET, name, value).await.is_err(),
            "{name}"
        );
    }
    assert!(
        HyperparamPlanner::new(AccountId([8; 32]))
            .plan(&client, NET, "rho", 1u16.into())
            .await
            .is_err()
    );
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn deadline_and_cancel_bound_hanging_reads() -> anyhow::Result<()> {
    let (chain, client, planner) = fixture(|_| Ok(())).await?;
    chain.mutate_state(|state| state.queue_reply("state_queryStorageAt", Reply::silent()));
    let result = planner
        .plan_with_wait(
            &client,
            NET,
            "rho",
            1u16.into(),
            WindowWait::UntilOpen {
                deadline: Instant::now() + Duration::from_millis(30),
            },
            &CancellationToken::new(),
        )
        .await;
    assert!(result.is_err());
    let cancel = CancellationToken::new();
    cancel.cancel();
    let result = planner
        .plan_with_wait(
            &client,
            NET,
            "rho",
            1u16.into(),
            WindowWait::NoWait,
            &cancel,
        )
        .await;
    assert!(result.is_err());
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn cooldown_opening_inside_freeze_waits_for_actual_epoch_and_refreshes_call()
-> anyhow::Result<()> {
    let (chain, client, planner) = fixture(|state| {
        state.seed(&st::TEMPO, (NET,), 20u16)?;
        state.seed(&st::LAST_EPOCH_BLOCK, (NET,), 85u64)?;
        state.seed(&st::ADMIN_FREEZE_WINDOW, (), 5u16)?;
        state.seed(&st::OWNER_HYPERPARAM_RATE_LIMIT, (), 1u16)?;
        state.seed_python(
            "SubtensorModule",
            "AlphaValues",
            &[json!(NET.0)],
            json!([1000, 60000]),
        )?;
        seed_last(state, owner_parameter("alpha_low")?, 82)?;
        Ok(())
    })
    .await?;
    let initial = planner
        .plan(&client, NET, "alpha_low", 2000u16.into())
        .await?;
    assert_eq!(initial.earliest_block, 102);
    let pending_client = client.clone();
    let task = tokio::spawn(async move {
        planner
            .plan_with_wait(
                &pending_client,
                NET,
                "alpha_low",
                2000u16.into(),
                WindowWait::UntilOpen {
                    deadline: Instant::now() + Duration::from_secs(5),
                },
                &CancellationToken::new(),
            )
            .await
    });
    for _ in 0..100 {
        if chain
            .requests()
            .iter()
            .any(|request| request.method == "chain_subscribeNewHeads")
        {
            break;
        }
        tokio::task::yield_now().await;
    }
    chain.advance(2)?;
    let frozen = HyperparamPlanner::new(OWNER)
        .plan(&client, NET, "alpha_low", 2000u16.into())
        .await?;
    assert_eq!(frozen.earliest_block, 105);
    assert!(
        frozen
            .reasons
            .iter()
            .any(|reason| reason.contains("freeze"))
    );
    assert!(!task.is_finished());
    chain.mutate_state(|state| -> anyhow::Result<()> {
        state.seed(&st::LAST_EPOCH_BLOCK, (NET,), 102u64)?;
        state.seed_python(
            "SubtensorModule",
            "AlphaValues",
            &[json!(NET.0)],
            json!([1000, 61000]),
        )?;
        Ok(())
    })?;
    chain.advance(1)?;
    let plan = tokio::time::timeout(Duration::from_secs(2), task).await???;
    assert!(plan.reasons.is_empty());
    assert_eq!(plan.earliest_block, 103);
    let expected = rtao_runtime::calls::admin_utils::sudo_set_alpha_values(NET, 2000, 61000)
        .compose(&client.runtime().await?)?;
    assert_eq!(plan.call, expected);
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn production_operation_cooldown_endpoints_are_inclusive() -> anyhow::Result<()> {
    for (setter, code, limit) in [
        ("sudo_set_mechanism_count", 7u16, 7200u64),
        ("sudo_set_mechanism_emission_split", 8, 7200),
        ("sudo_trim_to_max_allowed_uids", 9, 216000),
        ("sudo_trim_null_uids_batch", 9, 216000),
    ] {
        let (chain, client, planner) = fixture(|state| {
            *state = FakeState::from_python_seed(state.runtime.clone(), &json!({"block":limit+100, "constants":[{"pallet":"Aura", "item":"SlotDuration", "value":12000}]}))?;
            state.seed(&st::SUBNET_OWNER, (NET,), rtao_primitives::DefaultAccount::from(OWNER))?;
            state.seed(&st::TEMPO, (NET,), 0u16)?;
            state.seed(&st::TRANSACTION_KEY_LAST_BLOCK, (OWNER, NET, code), 101u64)?;
            Ok(())
        }).await?;
        let closed = planner
            .operation_window(&client, NET, setter, Some(10))
            .await?;
        assert_eq!(closed.0, limit + 101);
        assert_eq!(closed.1.len(), 1);
        chain.advance(1)?;
        let open = planner
            .operation_window(&client, NET, setter, Some(10))
            .await?;
        assert_eq!(open.0, limit + 101);
        assert!(open.1.is_empty());
        client.close().await;
    }
    Ok(())
}

#[tokio::test]
async fn version_key_uses_current_tempo_and_saturating_multiplier() -> anyhow::Result<()> {
    for (tempo, periods, last, expected, closed) in [
        (10u16, 2u64, 0u64, 100u64, false),
        (10, 2, 81, 101, true),
        (10, 2, 80, 100, false),
        (0, 2, 200, 100, false),
        (360, u64::MAX, 1, u64::MAX, true),
    ] {
        let (_, client, planner) = fixture(|state| {
            state.seed(&st::TEMPO, (NET,), tempo)?;
            state.seed(&st::ADMIN_FREEZE_WINDOW, (), 0u16)?;
            state.seed(&st::WEIGHTS_VERSION_KEY_RATE_LIMIT, (), periods)?;
            state.seed(&st::TRANSACTION_KEY_LAST_BLOCK, (OWNER, NET, 4), last)?;
            Ok(())
        })
        .await?;
        let plan = planner
            .plan(&client, NET, "weights_version", 1u64.into())
            .await?;
        assert_eq!(
            (plan.earliest_block, !plan.reasons.is_empty()),
            (expected, closed)
        );
        client.close().await;
    }
    Ok(())
}

#[tokio::test]
async fn tempo_transaction_limit_is_fixed_and_opens_at_equality() -> anyhow::Result<()> {
    let (chain, client, planner) = fixture(|state| {
        *state = FakeState::from_python_seed(state.runtime.clone(), &json!({"block":460}))?;
        state.seed(
            &st::SUBNET_OWNER,
            (NET,),
            rtao_primitives::DefaultAccount::from(OWNER),
        )?;
        state.seed(&st::TEMPO, (NET,), 0u16)?;
        state.seed(&st::TRANSACTION_KEY_LAST_BLOCK, (OWNER, NET, 11), 101u64)?;
        Ok(())
    })
    .await?;
    let closed = planner.plan(&client, NET, "tempo", 720u16.into()).await?;
    assert_eq!(closed.earliest_block, 461);
    assert_eq!(closed.reasons.len(), 1);
    chain.advance(1)?;
    let open = planner.plan(&client, NET, "tempo", 720u16.into()).await?;
    assert_eq!(open.earliest_block, 461);
    assert!(open.reasons.is_empty());
    client.close().await;
    Ok(())
}
