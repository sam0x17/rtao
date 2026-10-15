//! Production preflight and scheduling checks against encoded FakeChain state.

#![allow(clippy::expect_used)]
use super::*;
use rtao_client::{ClientConfig, Network};
use rtao_testkit::rpc::RpcClient;
use rtao_testkit::{FakeChain, FakeState};

pub(super) const HOTKEY: AccountId = AccountId([7; 32]);
pub(super) async fn fixture() -> anyhow::Result<(FakeChain, Client)> {
    let mut state = FakeState::try_default()?;
    state.set_finalized(97)?;
    state.seed_typed("SubtensorModule", "Uids", (NetUid(1), HOTKEY), 0u16)?;
    state.seed(
        &st::LAST_UPDATE,
        (NetUidStorageIndex(1),),
        vec![80u64, 0, 0],
    )?;
    state.seed(&st::WEIGHTS_SET_RATE_LIMIT, (NetUid(1),), 5u64)?;
    state.seed(&st::COMMIT_REVEAL_WEIGHTS_ENABLED, (NetUid(1),), true)?;
    state.seed(&st::COMMIT_REVEAL_WEIGHTS_VERSION, (), 4u16)?;
    state.seed(&st::REVEAL_PERIOD_EPOCHS, (NetUid(1),), 1u64)?;
    state.seed(&st::WEIGHTS_VERSION_KEY, (NetUid(1),), 0u64)?;
    state.seed(&st::MIN_ALLOWED_WEIGHTS, (NetUid(1),), 1u16)?;
    state.seed(&st::MAX_WEIGHTS_LIMIT, (NetUid(1),), u16::MAX)?;
    state.seed(&st::SUBNETWORK_N, (NetUid(1),), 3u16)?;
    state.seed_python(
        "SubtensorModule",
        "SubnetEpochConsensus",
        &[serde_json::json!(1)],
        serde_json::json!("Yuma"),
    )?;
    state.seed(
        &st::VALIDATOR_PERMIT,
        (NetUid(1),),
        vec![true, false, false],
    )?;
    state.seed(&st::TEMPO, (NetUid(1),), 10u16)?;
    state.seed(&st::LAST_EPOCH_BLOCK, (NetUid(1),), 100u64)?;
    state.seed(&st::PENDING_EPOCH_AT, (NetUid(1),), 0u64)?;
    state.seed(&st::SUBNET_EPOCH_INDEX, (NetUid(1),), 5u64)?;
    state.seed(&st::BLOCKS_SINCE_LAST_STEP, (NetUid(1),), 0u64)?;
    let chain = FakeChain::from_state(state);
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
    config.runtime_cache_dir = None;
    config.config_store = None;
    let client = Client::from_rpc(RpcClient::new(chain.clone()), config).await?;
    Ok((chain, client))
}

pub(super) fn seed_at<K: IntoCoreValue, V: IntoCoreValue>(
    state: &mut FakeState,
    block: u64,
    pallet: &str,
    item: &str,
    keys: K,
    value: V,
) -> anyhow::Result<()> {
    let hash = state
        .block_hash(block)
        .ok_or_else(|| anyhow::anyhow!("missing fixture block"))?;
    let info = state
        .runtime
        .storage_entry(pallet, item)
        .ok_or_else(|| anyhow::anyhow!("missing fixture storage"))?;
    let keys = match keys.into_core_value() {
        Value::List(keys) | Value::Tuple(keys) => keys,
        Value::Null => Vec::new(),
        key => vec![key],
    };
    let key = state
        .runtime
        .storage_key(info, &keys)
        .map_err(rtao_core::CoreError::from)?;
    let bytes = state
        .runtime
        .encode_spec(
            &rtao_core::runtime::TypeSpec::Id(info.value_type),
            &value.into_core_value(),
        )
        .map_err(rtao_core::CoreError::from)?;
    state.seed_at(hash, key, Some(bytes));
    Ok(())
}

pub(super) fn header(chain: &FakeChain, block: u64) -> anyhow::Result<BlockHeader> {
    let raw = chain
        .mutate_state(|state| state.block_hash(block).and_then(|hash| state.header(&hash)))
        .ok_or_else(|| anyhow::anyhow!("missing fixture header"))?;
    Ok(BlockHeader::from_json(raw)?)
}

#[tokio::test]
async fn finality_lag_three_preflight_uses_best_state_in_one_seventeen_key_rpc()
-> anyhow::Result<()> {
    let (chain, client) = fixture().await?;
    let finalized = client.at(BlockRef::Finalized).await?;
    let best = client.at(BlockRef::Best).await?;
    assert_eq!(best.block().number - finalized.block().number, 3);
    let before = chain.requests().len();
    let state = WeightPreflight::fetch(&best, NetUid(1), MechId(0), HOTKEY).await?;
    let requests = chain.requests();
    let storage = requests[before..]
        .iter()
        .filter(|request| request.method == "state_queryStorageAt")
        .collect::<Vec<_>>();
    assert_eq!(storage.len(), 1);
    assert_eq!(
        storage[0].params[0].as_array().expect("query keys").len(),
        17
    );
    assert_eq!(
        storage[0].params[1],
        serde_json::json!(format!("0x{}", hex::encode(best.block().hash.0)))
    );
    assert_eq!(state.schedule.current_block, 100);
    assert_eq!(state.schedule.last_epoch_block, 100);
    assert_eq!(state.schedule.subnet_epoch_index, 5);
    assert_eq!(
        rtao_core::timelock::epoch_schedule::predict_first_reveal_block(&state.schedule, 1)?,
        110
    );
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn preflight_distinguishes_null_refusal_yuma_warning_and_upgrade_gates() -> anyhow::Result<()>
{
    let (_, client) = fixture().await?;
    let best = client.at(BlockRef::Best).await?;
    let mut state = WeightPreflight::fetch(&best, NetUid(1), MechId(0), HOTKEY).await?;
    state.permits[0] = false;
    assert_eq!(state.check(0)?, Uid(0));
    state.null_consensus = true;
    assert_eq!(
        state
            .check(0)
            .expect_err("Null rejects no permit")
            .downcast_ref::<WeightRefusal>()
            .expect("typed refusal")
            .code,
        rtao_catalog::ErrorCode::NotAuthorized
    );
    state.permits[0] = true;
    state.commit_reveal_version = 5;
    assert_eq!(
        state
            .check(0)
            .expect_err("unsupported protocol")
            .downcast_ref::<WeightRefusal>()
            .expect("typed refusal")
            .name,
        "IncorrectCommitRevealVersion"
    );
    state.commit_reveal_version = 4;
    state.required_version_key = 42;
    assert!(state.check(0).is_err());
    assert_eq!(state.check(42)?, Uid(0));
    state.uid = None;
    assert_eq!(
        state
            .check(42)
            .expect_err("not registered")
            .downcast_ref::<WeightRefusal>()
            .expect("typed refusal")
            .code,
        rtao_catalog::ErrorCode::NotRegistered
    );
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn cadence_honors_each_mechanism_slot_rate_limit_and_actual_epoch() -> anyhow::Result<()> {
    let (_, client) = fixture().await?;
    let best = client.at(BlockRef::Best).await?;
    let mut state = WeightPreflight::fetch(&best, NetUid(1), MechId(0), HOTKEY).await?;
    let cadence = WeightCadence::OncePerEpoch { offset_blocks: 2 };
    let mut slot = Slot::default();
    assert!(!due(cadence, &slot, &state, Uid(0)));
    state.at.number = 102;
    assert!(due(cadence, &slot, &state, Uid(0)));
    slot.last_attempt_epoch = Some(5);
    slot.last_attempt_block = Some(102);
    assert!(!due(cadence, &slot, &state, Uid(0)));
    state.schedule.subnet_epoch_index = 6;
    state.schedule.last_epoch_block = 110;
    state.at.number = 112;
    assert!(due(cadence, &slot, &state, Uid(0)));
    state.last_updates[0] = 111;
    assert!(!due(cadence, &slot, &state, Uid(0)));
    assert!(due(cadence, &Slot::default(), &state, Uid(1)));
    state.last_updates[0] = 0;
    assert!(due(
        WeightCadence::RateLimited { margin: 500 },
        &Slot::default(),
        &state,
        Uid(0)
    ));
    client.close().await;
    Ok(())
}

#[test]
fn jitter_is_deterministic_and_bounded_even_with_disabled_tempo() {
    for tempo in [0, 1, 9, 10, 360, u16::MAX] {
        for byte in 0..30 {
            let key = AccountId([byte; 32]);
            let first = default_cadence(key, tempo, true);
            assert_eq!(first, default_cadence(key, tempo, true));
            assert!(matches!(first, WeightCadence::OncePerEpoch { .. }));
            let WeightCadence::OncePerEpoch { offset_blocks } = first else {
                continue;
            };
            assert!((2..2 + u64::from((tempo / 10).max(1))).contains(&offset_blocks));
        }
    }
    assert_eq!(
        default_cadence(HOTKEY, 360, false),
        WeightCadence::RateLimited { margin: 0 }
    );
}

fn graph() -> anyhow::Result<rtao_metagraph::Metagraph> {
    Ok(rtao_metagraph::Metagraph::from_dynamic(
        NetUid(1),
        Value::record(vec![
            ("netuid".into(), Value::Int(1)),
            ("block".into(), Value::Int(100)),
            ("tempo".into(), Value::Int(10)),
            ("last_step".into(), Value::Int(100)),
            ("blocks_since_last_step".into(), Value::Int(0)),
            ("network_registered_at".into(), Value::Int(1)),
            ("owner_hotkey".into(), Value::Bytes(vec![8; 32])),
            ("owner_coldkey".into(), Value::Bytes(vec![9; 32])),
            (
                "hotkeys".into(),
                Value::List(vec![
                    HOTKEY.into_core_value(),
                    AccountId([8; 32]).into_core_value(),
                    AccountId([10; 32]).into_core_value(),
                ]),
            ),
        ]),
        rtao_metagraph::SnapshotInfo::default(),
    )?)
}

#[tokio::test]
async fn raw_proof_and_validation_precede_clipping_and_preserve_exact_null_rows()
-> anyhow::Result<()> {
    let (_, client) = fixture().await?;
    let best = client.at(BlockRef::Best).await?;
    let mut state = WeightPreflight::fetch(&best, NetUid(1), MechId(0), HOTKEY).await?;
    let graph = graph()?;
    let raw = || WeightsInput::RawU16 {
        uids: vec![Uid(1), Uid(2)],
        values: vec![0, 17],
    };
    assert!(prepare_row(raw(), &state, Uid(0), NetUid(1), &graph).is_err());
    state.null_consensus = true;
    state.max_weights_limit = u16::MAX;
    assert_eq!(
        prepare_row(raw(), &state, Uid(0), NetUid(1), &graph)?,
        (vec![Uid(1), Uid(2)], vec![0, 17])
    );
    state.permits[1] = true;
    assert!(prepare_row(raw(), &state, Uid(0), NetUid(1), &graph).is_err());
    state.permits[1] = false;
    for bad in [f64::NAN, f64::INFINITY, -1.0] {
        assert!(
            prepare_row(
                WeightsInput::Parallel {
                    uids: vec![Uid(1), Uid(2)],
                    weights: vec![bad, 1.0]
                },
                &state,
                Uid(0),
                NetUid(1),
                &graph
            )
            .is_err()
        );
    }
    assert!(
        prepare_row(
            WeightsInput::Parallel {
                uids: vec![Uid(1), Uid(1)],
                weights: vec![1.0, 1.0]
            },
            &state,
            Uid(0),
            NetUid(1),
            &graph
        )
        .is_err()
    );
    assert!(
        prepare_row(
            WeightsInput::Parallel {
                uids: vec![Uid(3)],
                weights: vec![1.0]
            },
            &state,
            Uid(0),
            NetUid(1),
            &graph
        )
        .is_err()
    );
    assert!(hold_previous(
        &(vec![Uid(0), Uid(1), Uid(2)], vec![1, 1, 0]),
        Uid(0),
        AccountId([8; 32]),
        &graph
    ));
    assert!(!hold_previous(
        &(vec![Uid(0), Uid(1), Uid(2)], vec![1, 1, 1]),
        Uid(0),
        AccountId([8; 32]),
        &graph
    ));
    state.max_weights_limit = u16::MAX;
    assert_eq!(
        prepare_row(
            WeightsInput::ByHotkey(vec![(AccountId([10; 32]), 1.0)]),
            &state,
            Uid(0),
            NetUid(1),
            &graph
        )?,
        (vec![Uid(2)], vec![65535])
    );
    state.min_allowed_weights = 10;
    assert!(
        prepare_row(
            WeightsInput::Parallel {
                uids: vec![Uid(1), Uid(2)],
                weights: vec![1.0, 1.0]
            },
            &state,
            Uid(0),
            NetUid(1),
            &graph
        )
        .is_err()
    );
    assert_eq!(
        prepare_row(
            WeightsInput::Parallel {
                uids: vec![Uid(0)],
                weights: vec![1.0]
            },
            &state,
            Uid(0),
            NetUid(1),
            &graph
        )?,
        (vec![Uid(0)], vec![65535])
    );
    client.close().await;
    Ok(())
}

/// Pinned chain weights.rs:1050-1075 enforces capped vector length then Null
/// max/sum. The shared preparation preserves every pair, including zeros.
#[tokio::test]
async fn raw_shared_preparation_enforces_chain_count_and_exact_share() -> anyhow::Result<()> {
    let (_, client) = fixture().await?;
    let best = client.at(BlockRef::Best).await?;
    let mut state = WeightPreflight::fetch(&best, NetUid(1), MechId(0), HOTKEY).await?;
    state.null_consensus = true;
    state.min_allowed_weights = 2;
    let graph = graph()?;
    let row = |values| WeightsInput::RawU16 {
        uids: vec![Uid(1), Uid(2)],
        values,
    };
    assert_eq!(
        prepare_row(row(vec![0, 17]), &state, Uid(0), NetUid(1), &graph)?,
        (vec![Uid(1), Uid(2)], vec![0, 17]),
        "chain length counts zero pairs without mutating raw input"
    );
    assert_eq!(
        prepare_row(row(vec![3, 1]), &state, Uid(0), NetUid(1), &graph)?,
        (vec![Uid(1), Uid(2)], vec![3, 1]),
        "raw rows retain exact integers instead of max-upscaling them"
    );
    state.max_weights_limit = 32_767;
    let error = prepare_row(row(vec![100, 1]), &state, Uid(0), NetUid(1), &graph)
        .expect_err("raw preparation enforces the configured maximum share");
    assert!(
        error
            .to_string()
            .contains("raw weights exceed the configured maximum share")
    );
    // Exact equality is allowed; one greater max at the same sum is refused.
    // A one-unit smaller configured limit is refused too.
    state.max_weights_limit = 32_768;
    assert_eq!(
        prepare_row(row(vec![32_768, 32_767]), &state, Uid(0), NetUid(1), &graph)?,
        (vec![Uid(1), Uid(2)], vec![32_768, 32_767])
    );
    assert!(
        prepare_row(row(vec![32_769, 32_766]), &state, Uid(0), NetUid(1), &graph).is_err(),
        "one greater maximum at the same total exceeds the exact share boundary"
    );
    state.max_weights_limit = 32_767;
    assert!(prepare_row(row(vec![32_768, 32_767]), &state, Uid(0), NetUid(1), &graph).is_err());
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn raw_shared_preparation_caps_count_to_population_and_retains_zero_pairs()
-> anyhow::Result<()> {
    let (_, client) = fixture().await?;
    let best = client.at(BlockRef::Best).await?;
    let mut state = WeightPreflight::fetch(&best, NetUid(1), MechId(0), HOTKEY).await?;
    state.null_consensus = true;
    state.min_allowed_weights = 10;
    let graph = graph()?;
    let row = |values| WeightsInput::RawU16 {
        uids: vec![Uid(0), Uid(1), Uid(2)],
        values,
    };
    for values in [vec![65_534, 1, 1], vec![65_534, 1, 0], vec![17, 0, 0]] {
        assert_eq!(
            prepare_row(row(values.clone()), &state, Uid(0), NetUid(1), &graph)?,
            (vec![Uid(0), Uid(1), Uid(2)], values),
            "min(10, population=3) counts all three supplied pairs"
        );
    }
    assert!(
        prepare_row(
            WeightsInput::RawU16 {
                uids: vec![Uid(1), Uid(2)],
                values: vec![1, 1]
            },
            &state,
            Uid(0),
            NetUid(1),
            &graph
        )
        .is_err()
    );
    state.max_weights_limit = 0;
    assert_eq!(
        prepare_row(
            WeightsInput::RawU16 {
                uids: vec![Uid(0)],
                values: vec![7]
            },
            &state,
            Uid(0),
            NetUid(1),
            &graph
        )?,
        (vec![Uid(0)], vec![7])
    );
    // These test row constraints only; direct dispatch separately refuses root.
    // The length helper does not give root the single-self count exemption.
    assert!(
        prepare_row(
            WeightsInput::RawU16 {
                uids: vec![Uid(0)],
                values: vec![7]
            },
            &state,
            Uid(0),
            NetUid(0),
            &graph
        )
        .is_err()
    );
    state.min_allowed_weights = 1;
    assert_eq!(
        prepare_row(
            WeightsInput::RawU16 {
                uids: vec![Uid(0)],
                values: vec![7]
            },
            &state,
            Uid(0),
            NetUid(0),
            &graph
        )?,
        (vec![Uid(0)], vec![7]),
        "the chain maximum-share self exemption itself is root-independent"
    );
    for netuid in [NetUid(0), NetUid(1)] {
        assert!(
            prepare_row(
                WeightsInput::RawU16 {
                    uids: vec![Uid(0)],
                    values: vec![0]
                },
                &state,
                Uid(0),
                netuid,
                &graph
            )
            .is_err()
        );
    }
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn raw_shared_preparation_rejects_structurally_invalid_rows_without_rewriting()
-> anyhow::Result<()> {
    let (_, client) = fixture().await?;
    let best = client.at(BlockRef::Best).await?;
    let mut state = WeightPreflight::fetch(&best, NetUid(1), MechId(0), HOTKEY).await?;
    state.null_consensus = true;
    let graph = graph()?;
    for (uids, values, diagnostic) in [
        (vec![Uid(1)], vec![1, 2], "differ in length"),
        (vec![Uid(1), Uid(1)], vec![1, 2], "duplicate UIDs"),
        (
            vec![Uid(3)],
            vec![1],
            "outside the current subnet population",
        ),
        (vec![Uid(1), Uid(2)], vec![0, 0], "all raw weights are zero"),
        (vec![], vec![], "all raw weights are zero"),
    ] {
        let error = prepare_row(
            WeightsInput::RawU16 { uids, values },
            &state,
            Uid(0),
            NetUid(1),
            &graph,
        )
        .expect_err("invalid raw vector");
        assert!(error.to_string().contains(diagnostic));
    }
    client.close().await;
    Ok(())
}

#[test]
fn policy_update_preserves_pending_uncertainty_and_its_diagnostic() -> anyhow::Result<()> {
    let mut scheduler = WeightScheduler::new(
        WeightPolicy::default(),
        Arc::new(crate::ChainClock::new(1_000_000)),
        ClockGuard::default(),
    )?;
    scheduler.slots.insert(
        MechId(2),
        Slot {
            last_attempt_epoch: Some(4),
            last_attempt_block: Some(100),
            uncertain: true,
        },
    );
    scheduler.attempts.insert(
        MechId(2),
        WeightAttempt {
            mechanism: MechId(2),
            best_block: 100,
            outcome: WeightAttemptOutcome::Unresolved("reconcile this accepted hash".into()),
        },
    );
    scheduler.update_policy(WeightPolicy {
        version_key: VersionKey::Fixed(42),
        cadence: None,
    });
    assert_eq!(scheduler.policy.version_key.value(), 42);
    assert!(scheduler.slots.get(&MechId(2)).expect("slot").uncertain);
    assert!(matches!(
        scheduler
            .last_attempt(MechId(2))
            .expect("retained outcome")
            .outcome,
        WeightAttemptOutcome::Unresolved(_)
    ));
    scheduler.resolve_uncertainty(MechId(2));
    assert!(!scheduler.slots.get(&MechId(2)).expect("slot").uncertain);
    assert_eq!(
        scheduler
            .slots
            .get(&MechId(2))
            .expect("slot")
            .last_attempt_epoch,
        Some(4)
    );
    Ok(())
}
