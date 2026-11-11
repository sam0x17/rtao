// Derived from RaoFoundation/subtensor pallets/subtensor/src/utils/misc.rs @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Production preflight and scheduling checks against encoded FakeChain state.

#![allow(clippy::expect_used)]
use super::*;
use rtao_client::{ClientConfig, Network, ViewExt};
use rtao_testkit::rpc::RpcClient;
use rtao_testkit::{FakeChain, FakeState};

pub(super) const HOTKEY: AccountId = AccountId([7; 32]);
pub(super) async fn fixture() -> anyhow::Result<(FakeChain, Client)> {
    fixture_with(|_| {}).await
}

/// [`fixture`] serving metadata changed by `edit`, seeded afterwards so
/// every value is encoded with the changed types.
pub(super) async fn fixture_with(
    edit: impl FnOnce(&mut frame_metadata::v15::RuntimeMetadataV15),
) -> anyhow::Result<(FakeChain, Client)> {
    let mut state = FakeState::try_default()?;
    state.patch_metadata(edit)?;
    seed_fixture(&mut state)?;
    let chain = FakeChain::from_state(state);
    let client = Client::from_rpc(RpcClient::new(chain.clone()), fixture_config()?).await?;
    Ok((chain, client))
}

/// A read endpoint and a separate submission endpoint over the same state.
pub(super) async fn split_fixture() -> anyhow::Result<(FakeChain, FakeChain, Client)> {
    let read = FakeChain::from_state(fixture_state()?);
    let node = FakeChain::from_state(fixture_state()?);
    let client = Client::from_rpc_with_submit(
        RpcClient::new(read.clone()),
        RpcClient::new(node.clone()),
        fixture_config()?,
    )
    .await?;
    Ok((read, node, client))
}

fn fixture_config() -> anyhow::Result<ClientConfig> {
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
    config.runtime_cache_dir = None;
    config.config_store = None;
    Ok(config)
}

pub(super) fn fixture_state() -> anyhow::Result<FakeState> {
    let mut state = FakeState::try_default()?;
    seed_fixture(&mut state)?;
    Ok(state)
}

fn seed_fixture(state: &mut FakeState) -> anyhow::Result<()> {
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
    Ok(())
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
    assert!(
        !requests[before..]
            .iter()
            .any(|request| request.method == "state_getStorageHash"),
        "the native commit-reveal tag needs no runtime code identity"
    );
    assert_eq!(state.runtime_code, None);
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
    // A changed commit-reveal tag is never a preflight refusal (D-122).
    state.commit_reveal_version = 5;
    assert_eq!(state.check(0)?, Uid(0));
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

/// Seed `:code` at the fixture's best block and return its storage hash.
fn seed_code(chain: &FakeChain, code: &[u8]) -> rtao_primitives::H256 {
    chain.mutate_state(|state| {
        let hash = state.block_hash(state.block).expect("best block");
        state.seed_at(hash, b":code".to_vec(), Some(code.to_vec()));
    });
    rtao_primitives::H256(Blake2b::<U32>::digest(code).into())
}

pub(super) fn fresh_scheduler() -> anyhow::Result<WeightScheduler> {
    WeightScheduler::new(
        WeightPolicy::default(),
        Arc::new(crate::ChainClock::new(1_000_000)),
        ClockGuard::default(),
    )
}

#[tokio::test]
async fn changed_cr_version_reads_code_identity_and_follows_only_known_code() -> anyhow::Result<()>
{
    let (chain, client) = fixture().await?;
    chain.mutate_state(|state| state.seed(&st::COMMIT_REVEAL_WEIGHTS_VERSION, (), 5u16))?;
    let code = seed_code(&chain, b"runtime code A");
    let before = chain.requests().len();
    let best = client.at(BlockRef::Best).await?;
    let mut state = WeightPreflight::fetch(&best, NetUid(1), MechId(0), HOTKEY).await?;
    let identity = chain.requests()[before..]
        .iter()
        .filter(|request| request.method == "state_getStorageHash")
        .map(|request| request.params.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        identity,
        [serde_json::json!([
            "0x3a636f6465",
            format!("0x{}", hex::encode(best.block().hash.0))
        ])]
    );
    assert_eq!(state.commit_reveal_version, 5);
    assert_eq!(state.runtime_code, Some(code));

    // By default an unreviewed runtime is followed and reported (D-110).
    let following = fresh_scheduler()?;
    assert_eq!(following.commit_reveal_tag(&state)?, 5);
    assert_eq!(
        following.commit_reveal_decision(&state),
        rtao_core::commit_reveal::TagDecision::Follow {
            tag: 5,
            basis: rtao_core::commit_reveal::TagBasis::Unreviewed
        }
    );
    assert_ne!(
        following.next_action(MechId(0), HOTKEY, &state)?,
        WeightOpportunity::Paused
    );
    // An unexplained miss under this code and tag is reported, and the tag
    // is still followed (D-122).
    let mut refuted = fresh_scheduler()?;
    refuted.reveals.remember_missed_reveal(code, 5);
    assert_eq!(refuted.commit_reveal_tag(&state)?, 5);
    assert_eq!(
        refuted.commit_reveal_decision(&state),
        rtao_core::commit_reveal::TagDecision::Follow {
            tag: 5,
            basis: rtao_core::commit_reveal::TagBasis::Refuted
        }
    );
    assert_ne!(
        refuted.next_action(MechId(0), HOTKEY, &state)?,
        WeightOpportunity::Paused
    );

    // The earlier behavior, kept as an option: unproven code pauses.
    let mut scheduler =
        fresh_scheduler()?.with_unreviewed_cr_tags(rtao_core::commit_reveal::Unreviewed::Pause);
    let refusal = scheduler
        .commit_reveal_tag(&state)
        .expect_err("unproven code pauses");
    assert_eq!(refusal.name, "IncorrectCommitRevealVersion");
    for needle in [
        "version 5",
        "reviewed list",
        &format!("0x{}", hex::encode(code.0)),
        "metadata default 4",
        "Unreviewed::Pause",
    ] {
        assert!(
            refusal.remediation.contains(needle),
            "{needle} absent from {refusal}"
        );
    }
    assert_eq!(
        scheduler.next_action(MechId(0), HOTKEY, &state)?,
        WeightOpportunity::Paused
    );

    // Evidence collected while another tag was stored proves nothing for 5:
    // unreviewed code may decode by the stored version.
    scheduler.reveals.remember_revealed_code(code, 4);
    scheduler.reveals.remember_revealed_code(code, 6);
    assert!(scheduler.commit_reveal_tag(&state).is_err());
    // A reveal of this validator's own commitment under the same code while
    // 5 was stored proves it.
    scheduler.reveals.remember_revealed_code(code, 5);
    assert_eq!(scheduler.commit_reveal_tag(&state)?, 5);
    assert_ne!(
        scheduler.next_action(MechId(0), HOTKEY, &state)?,
        WeightOpportunity::Paused
    );
    // The public preflight check never refuses the chain's tag (D-122).
    assert_eq!(state.check(0)?, Uid(0));
    assert_eq!(state.commit_reveal_tag(), 5);

    // A runtime upgrade to unproven code pauses again.
    let upgraded = seed_code(&chain, b"runtime code B");
    let best = client.at(BlockRef::Best).await?;
    let upgraded_state = WeightPreflight::fetch(&best, NetUid(1), MechId(0), HOTKEY).await?;
    assert_eq!(upgraded_state.runtime_code, Some(upgraded));
    assert!(scheduler.commit_reveal_tag(&upgraded_state).is_err());
    assert_eq!(fresh_scheduler()?.commit_reveal_tag(&upgraded_state)?, 5);

    // Reviewed pinned runtimes are followed without prior evidence.
    let reviewed = REVIEWED_CR_RUNTIME_CODE[REVIEWED_CR_RUNTIME_CODE.len() - 1].1;
    let mut bytes = [0u8; 32];
    hex::decode_to_slice(&reviewed[2..], &mut bytes)?;
    state.runtime_code = Some(rtao_primitives::H256(bytes));
    assert_eq!(reviewed_cr_runtime(rtao_primitives::H256(bytes)), Some(486));
    assert_eq!(fresh_scheduler()?.commit_reveal_tag(&state)?, 5);
    let mut refuted = fresh_scheduler()?;
    refuted
        .reveals
        .remember_missed_reveal(rtao_primitives::H256(bytes), 5);
    assert_eq!(
        refuted.commit_reveal_decision(&state).basis(),
        Some(rtao_core::commit_reveal::TagBasis::Refuted),
        "refutation beats review in the reported basis"
    );
    assert_eq!(refuted.commit_reveal_tag(&state)?, 5);
    // Missing identity follows the tag and reports it (D-122); only the
    // opt-in pauses.
    state.runtime_code = None;
    assert_eq!(fresh_scheduler()?.commit_reveal_tag(&state)?, 5);
    assert_eq!(
        fresh_scheduler()?.commit_reveal_decision(&state).basis(),
        Some(rtao_core::commit_reveal::TagBasis::CodeUnknown)
    );
    assert!(
        fresh_scheduler()?
            .with_unreviewed_cr_tags(rtao_core::commit_reveal::Unreviewed::Pause)
            .commit_reveal_tag(&state)
            .is_err()
    );

    // The native tag and direct rows never depend on code identity.
    state.commit_reveal_version = NATIVE_CR_PAYLOAD_VERSION;
    assert_eq!(fresh_scheduler()?.commit_reveal_tag(&state)?, 4);
    state.commit_reveal_version = 5;
    state.commit_reveal = false;
    assert_eq!(fresh_scheduler()?.commit_reveal_tag(&state)?, 4);
    client.close().await;
    Ok(())
}

/// The review list is an observation (D-122): the scheduler follows the
/// stored tag under listed and unlisted code alike and reports the listed spec
/// version, an unreviewed code and a refuting miss in gauges.
#[tokio::test]
async fn the_reviewed_observation_is_reported_and_never_decides() -> anyhow::Result<()> {
    let (chain, client) = fixture().await?;
    chain.mutate_state(|state| state.seed(&st::COMMIT_REVEAL_WEIGHTS_VERSION, (), 5u16))?;
    let unlisted = seed_code(&chain, b"runtime code A");
    let best = client.at(BlockRef::Best).await?;
    let mut state = WeightPreflight::fetch(&best, NetUid(1), MechId(0), HOTKEY).await?;
    let reviewed = REVIEWED_CR_RUNTIME_CODE[REVIEWED_CR_RUNTIME_CODE.len() - 1];
    let mut bytes = [0u8; 32];
    hex::decode_to_slice(&reviewed.1[2..], &mut bytes)?;
    let listed = rtao_primitives::H256(bytes);
    let gauges = |metrics: &crate::Metrics| {
        let rendered = metrics.render();
        ["reviewed", "unreviewed", "refuted"].map(|name| {
            let prefix = format!("rtao_weights_cr_version_{name}{{mech=\"0\"}} ");
            rendered
                .lines()
                .find_map(|line| line.strip_prefix(&prefix))
                .map_or_else(|| format!("{prefix} absent"), str::to_owned)
        })
    };
    let metrics = crate::Metrics::default();
    let mut scheduler = fresh_scheduler()?;
    state.runtime_code = Some(listed);
    assert_eq!(
        scheduler.follow_cr_tag(&metrics, MechId(0), &state),
        Some(5)
    );
    assert_eq!(
        gauges(&metrics),
        [reviewed.0.to_string(), "0".into(), "0".into()]
    );
    state.runtime_code = Some(unlisted);
    assert_eq!(
        scheduler.follow_cr_tag(&metrics, MechId(0), &state),
        Some(5)
    );
    assert_eq!(gauges(&metrics), ["0".to_owned(), "1".into(), "0".into()]);
    scheduler.reveals.remember_missed_reveal(unlisted, 5);
    assert_eq!(
        scheduler.follow_cr_tag(&metrics, MechId(0), &state),
        Some(5)
    );
    assert_eq!(gauges(&metrics), ["0".to_owned(), "0".into(), "1".into()]);
    scheduler.reveals.remember_missed_reveal(listed, 5);
    state.runtime_code = Some(listed);
    assert_eq!(
        scheduler.follow_cr_tag(&metrics, MechId(0), &state),
        Some(5)
    );
    assert_eq!(
        gauges(&metrics),
        [reviewed.0.to_string(), "0".into(), "1".into()]
    );
    // The native tag clears every observation.
    state.commit_reveal_version = NATIVE_CR_PAYLOAD_VERSION;
    assert_eq!(
        scheduler.follow_cr_tag(&metrics, MechId(0), &state),
        Some(4)
    );
    assert_eq!(gauges(&metrics), ["0".to_owned(), "0".into(), "0".into()]);
    assert!(
        !metrics
            .render()
            .contains("rtao_weights_cr_version_mismatch_total")
    );
    client.close().await;
    Ok(())
}

/// Every reviewed entry comes from a pinned provenance record, and the pinned
/// reveal path still decodes the native payload without reading the stored
/// version. A pin that changes reveal decoding fails here and must revisit
/// [`NATIVE_CR_PAYLOAD_VERSION`]. A new pin need not be added to the list:
/// the list is an observation (D-122), so an automated adoption whose reveal
/// path is unchanged passes without a manual review entry.
#[test]
fn reviewed_cr_runtimes_match_pinned_provenance_and_reveal_source() -> anyhow::Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut pinned = Vec::new();
    for entry in std::fs::read_dir(root.join("docs/compat"))? {
        let path = entry?.path();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        if !name.ends_with("-provenance.json") {
            continue;
        }
        let record: serde_json::Value = serde_json::from_slice(&std::fs::read(&path)?)?;
        let finney = &record["finney"];
        let spec = finney["spec"]
            .as_u64()
            .or_else(|| finney["spec_version"].as_u64())
            .ok_or_else(|| anyhow::anyhow!("{name} has no Finney spec"))?;
        let code = finney["code_hash"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("{name} has no Finney code hash"))?;
        pinned.push((u32::try_from(spec)?, code.to_owned()));
    }
    anyhow::ensure!(!pinned.is_empty(), "pinned provenance");
    for (spec, code) in REVIEWED_CR_RUNTIME_CODE {
        assert!(
            pinned.iter().any(|entry| *entry == (spec, code.to_owned())),
            "reviewed runtime {spec} {code} has no provenance record"
        );
    }
    let reveal = std::fs::read_to_string(
        root.join("vendor/upstream/src/pallets/subtensor/src/coinbase/reveal_commits.rs"),
    )?;
    assert!(reveal.contains("#[freeze_struct(\"b6833b5029be4127\")]"));
    assert!(reveal.contains("WeightsTlockPayload::decode"));
    assert!(!reveal.contains("commit_reveal_weights_version"));
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

#[tokio::test]
async fn activity_alarm_clamps_zero_tempo_and_keeps_nonzero_boundaries() -> anyhow::Result<()> {
    for (tempo, factor, last, expected) in [
        (0u16, 1_000u32, 99u64, Some(1u64)),
        (0, 1_000, 100, Some(1)),
        (1, 1_000, 100, Some(1)),
        (10, 1_000, 93, None),
        (10, 1_000, 92, Some(10)),
        (10, 1_000, 91, Some(10)),
        (10, 2_501, 81, None),
        (10, 2_501, 80, Some(25)),
    ] {
        let (chain, client) = fixture().await?;
        chain.mutate_state(|state| -> anyhow::Result<()> {
            seed_at(state, 100, "SubtensorModule", "Tempo", (NetUid(1),), tempo)?;
            seed_at(
                state,
                100,
                "SubtensorModule",
                "ActivityCutoffFactorMilli",
                (NetUid(1),),
                factor,
            )?;
            seed_at(
                state,
                100,
                "SubtensorModule",
                "LastUpdate",
                (NetUidStorageIndex(1),),
                vec![last],
            )?;
            Ok(())
        })?;
        let best = client.at(BlockRef::Best).await?;
        let preflight = WeightPreflight::fetch(&best, NetUid(1), MechId(0), HOTKEY).await?;
        let factor = best
            .storage(&st::ACTIVITY_CUTOFF_FACTOR_MILLI, (NetUid(1),))
            .await?;
        assert_eq!(
            preflight.activity_alarm_cutoff(preflight.check(0)?, factor),
            expected,
            "{tempo}/{factor}/{last}"
        );
        client.close().await;
    }
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
    graph_with([HOTKEY, AccountId([8; 32]), AccountId([10; 32])])
}

/// A three-UID last-good graph whose UID `i` is `hotkeys[i]`.
fn graph_with(hotkeys: [AccountId; 3]) -> anyhow::Result<rtao_metagraph::Metagraph> {
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
                Value::List(hotkeys.iter().map(|key| key.into_core_value()).collect()),
            ),
        ]),
        rtao_metagraph::SnapshotInfo::default(),
    )?)
}

/// The self/owner guard of earlier releases, kept verbatim as the reference
/// for [`OwnerRowPolicy::Hold`].
fn previous_guard(
    row: &(Vec<Uid>, Vec<u16>),
    uid: Uid,
    owner: AccountId,
    graph: &rtao_metagraph::Metagraph,
) -> bool {
    row.0
        .iter()
        .zip(&row.1)
        .filter(|(_, value)| **value > 0)
        .all(|(dest, _)| *dest == uid || graph.hotkey(*dest) == Some(&owner))
}

/// The guard's view for this validator at UID 0 with `hotkey`, when the
/// owner hotkey `owner` holds `owner_uid` at the dispatch block.
fn view(hotkey: AccountId, owner: AccountId, owner_uid: Option<u16>) -> OwnerView {
    OwnerView {
        block: 100,
        uid: Uid(0),
        own_is_owner: hotkey == owner,
        owner_uid: owner_uid.map(Uid),
    }
}

/// The view a fresh graph gives: the owner's UID is where `graph` maps it.
fn view_of(graph: &rtao_metagraph::Metagraph, hotkey: AccountId, owner: AccountId) -> OwnerView {
    view(
        hotkey,
        owner,
        graph.by_hotkey(&owner).map(|neuron| neuron.uid.0),
    )
}

/// `Hold` is exactly the earlier guard over every row on three UIDs with
/// values in {0, 1, 65535}, including all-zero and empty rows, for an owner
/// at UID 1, at the validator's own UID, and absent from the graph, when
/// the graph agrees with the dispatch block.
#[test]
fn owner_row_policy_hold_is_the_previous_guard() -> anyhow::Result<()> {
    let graph = graph()?;
    let uids = [Uid(0), Uid(1), Uid(2)];
    let values = [0u16, 1, 65535];
    let mut rows = vec![(Vec::new(), Vec::new())];
    // Every subset of destinations, in order, with every value assignment.
    for mask in 1u8..8 {
        let dests: Vec<Uid> = (0..3)
            .filter(|i| mask & (1 << i) != 0)
            .map(|i| uids[i])
            .collect();
        let mut choice = vec![0usize; dests.len()];
        loop {
            rows.push((dests.clone(), choice.iter().map(|&c| values[c]).collect()));
            let Some(i) = choice.iter().position(|&c| c + 1 < values.len()) else {
                break;
            };
            choice[i] += 1;
            choice[..i].iter_mut().for_each(|c| *c = 0);
        }
    }
    assert_eq!(rows.len(), 1 + 3 * 3 + 3 * 9 + 27);
    for owner in [AccountId([8; 32]), HOTKEY, AccountId([99; 32])] {
        let view = view_of(&graph, HOTKEY, owner);
        for row in &rows {
            assert_eq!(
                row_held(OwnerRowPolicy::Hold, row, &view),
                previous_guard(row, Uid(0), owner, &graph),
                "{row:?} with owner {owner}"
            );
            // Submit can only remove rows from the held set.
            if row_held(OwnerRowPolicy::Submit, row, &view) {
                assert!(previous_guard(row, Uid(0), owner, &graph), "{row:?}");
            }
        }
    }
    Ok(())
}

/// `Submit` unlocks only rows whose every positive entry is the owner's UID.
#[test]
fn owner_row_policy_submit_unlocks_only_owner_only_rows() -> anyhow::Result<()> {
    use OwnerRowPolicy::{Hold, Submit};
    let graph = graph()?;
    let owner = AccountId([8; 32]);
    let held = |policy, uids: &[u16], values: &[u16], view: &OwnerView| {
        row_held(
            policy,
            &(uids.iter().copied().map(Uid).collect(), values.to_vec()),
            view,
        )
    };
    let at_one = view(HOTKEY, owner, Some(1));
    // (uids, values, held under Hold, held under Submit)
    let cases: [(&[u16], &[u16], bool, bool); 8] = [
        // Owner-only burn rows.
        (&[1], &[65535], true, false),
        (&[0, 1, 2], &[0, 65535, 0], true, false),
        // Self-only, and self plus owner.
        (&[0], &[65535], true, true),
        (&[0, 1], &[1, 1], true, true),
        // Ordinary rows.
        (&[1, 2], &[1, 1], false, false),
        (&[2], &[1], false, false),
        // No positive entry.
        (&[1], &[0], true, true),
        (&[], &[], true, true),
    ];
    for (uids, values, hold, submit) in cases {
        assert_eq!(held(Hold, uids, values, &at_one), hold, "{uids:?}");
        assert_eq!(held(Submit, uids, values, &at_one), submit, "{uids:?}");
        // The public predicate with a graph closure gives the same answers.
        let row = uids.iter().copied().map(Uid).zip(values.iter().copied());
        let is_owner = |uid| graph.hotkey(uid) == Some(&owner);
        assert_eq!(Hold.holds(row.clone(), Uid(0), is_owner), hold);
        assert_eq!(Submit.holds(row, Uid(0), is_owner), submit);
    }
    // Owner unregistered at the dispatch block: only self-only rows hold.
    let absent = view(HOTKEY, AccountId([99; 32]), None);
    for policy in [Hold, Submit] {
        assert!(held(policy, &[0], &[1], &absent));
        assert!(!held(policy, &[1], &[1], &absent));
    }
    // This validator is the owner hotkey: its own row is the owner row.
    let own = view(HOTKEY, HOTKEY, Some(0));
    assert!(held(Hold, &[0], &[65535], &own));
    assert!(!held(Submit, &[0], &[65535], &own));
    for policy in [Hold, Submit] {
        assert!(!held(policy, &[0, 2], &[1, 1], &own));
    }
    // A non-owner validator's own UID is never the owner's, whatever UID the
    // view carries for the owner.
    let confused = view(HOTKEY, owner, Some(0));
    for policy in [Hold, Submit] {
        assert!(held(policy, &[0], &[65535], &confused));
        assert!(held(policy, &[0, 2], &[65535, 0], &confused));
    }
    Ok(())
}

/// The owner moved from UID 1 to UID 2 after a metagraph snapshot (D-098):
/// the guard follows the dispatch block, so the old owner UID, now a miner,
/// is an ordinary destination and the new one is the owner's.
#[test]
fn owner_row_guard_follows_the_dispatch_block_not_the_graph() {
    use OwnerRowPolicy::{Hold, Submit};
    let owner = AccountId([8; 32]);
    let moved = view(HOTKEY, owner, Some(2));
    let row = |uid: u16| (vec![Uid(uid)], vec![65535]);
    // UID 1 was the owner's in the snapshot; it is not at the dispatch block.
    assert!(!row_held(Hold, &row(1), &moved));
    assert!(!row_held(Submit, &row(1), &moved));
    // UID 2 is the owner's now, although the snapshot mapped it elsewhere.
    assert!(row_held(Hold, &row(2), &moved));
    assert!(!row_held(Submit, &row(2), &moved));
    // Two other destinations never need the owner's UID.
    assert_eq!(
        single_other_destination(&(vec![Uid(0), Uid(1), Uid(2)], vec![1, 1, 1]), Uid(0)),
        None
    );
    assert_eq!(
        single_other_destination(&(vec![Uid(0), Uid(1), Uid(2)], vec![1, 0, 1]), Uid(0)),
        Some(Uid(2))
    );
    assert_eq!(
        single_other_destination(&(vec![Uid(0)], vec![1]), Uid(0)),
        None
    );
}

#[test]
fn owner_row_policy_serde_tokens() -> anyhow::Result<()> {
    assert_eq!(OwnerRowPolicy::default(), OwnerRowPolicy::Hold);
    assert_eq!(serde_json::to_value(OwnerRowPolicy::Hold)?, "hold");
    assert_eq!(serde_json::to_value(OwnerRowPolicy::Submit)?, "submit");
    assert_eq!(
        serde_json::from_value::<OwnerRowPolicy>(serde_json::json!("submit"))?,
        OwnerRowPolicy::Submit
    );
    assert_eq!(
        serde_json::from_value::<OwnerRowPolicy>(serde_json::json!("hold"))?,
        OwnerRowPolicy::Hold
    );
    assert!(serde_json::from_value::<OwnerRowPolicy>(serde_json::json!("submit_all")).is_err());
    assert!(serde_json::from_value::<OwnerRowPolicy>(serde_json::json!("Submit")).is_err());
    Ok(())
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
    let owner_at_one = view(HOTKEY, AccountId([8; 32]), Some(1));
    assert!(row_held(
        OwnerRowPolicy::Hold,
        &(vec![Uid(0), Uid(1), Uid(2)], vec![1, 1, 0]),
        &owner_at_one
    ));
    assert!(!row_held(
        OwnerRowPolicy::Hold,
        &(vec![Uid(0), Uid(1), Uid(2)], vec![1, 1, 1]),
        &owner_at_one
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
            ..Default::default()
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

#[test]
fn advisories_log_when_a_condition_starts_and_again_after_it_clears() {
    let mut reported = false;
    assert!(advisory_started(&mut reported, true));
    // A persistent condition does not repeat on later blocks.
    assert!(!advisory_started(&mut reported, true));
    assert!(!advisory_started(&mut reported, true));
    assert!(!advisory_started(&mut reported, false));
    assert!(!advisory_started(&mut reported, false));
    assert!(advisory_started(&mut reported, true));
}

mod validator_surface {
    use super::super::{ValidatorSurface, compose};
    use rtao_core::runtime::Runtime;
    use rtao_runtime::RuntimeHandle;
    use scale_info::{TypeDef, Variant, form::PortableForm};

    /// The pinned Finney codegen input, which moves with the pin (D-178).
    fn pinned() -> Runtime {
        rtao_testkit::fixtures::pinned_finney_runtime().expect("pinned metadata")
    }

    /// The client supplies live API versions; the metagraph fetch needs v2+.
    fn handle(core: Runtime) -> RuntimeHandle {
        RuntimeHandle::new_with_api_versions(
            core,
            std::collections::BTreeMap::from([("SubnetInfoRuntimeApi".to_owned(), 3)]),
        )
        .expect("runtime")
    }

    fn warned(surface: &ValidatorSurface, descriptor: &str) -> bool {
        surface
            .warnings
            .iter()
            .any(|change| change.descriptor == descriptor)
    }

    fn fails_on(surface: &ValidatorSurface, descriptor: &str) -> bool {
        !surface.is_compatible()
            && surface
                .incompatible
                .iter()
                .any(|line| line.starts_with(&format!("{descriptor}:")))
    }

    fn enum_variants(core: &mut Runtime, type_id: u32) -> &mut Vec<Variant<PortableForm>> {
        let ty = core
            .types
            .types
            .iter_mut()
            .find(|ty| ty.id == type_id)
            .expect("enum type");
        let TypeDef::Variant(variants) = &mut ty.ty.type_def else {
            unreachable!("pallet call and error types are enums")
        };
        &mut variants.variants
    }

    /// Point one call argument at the type of its first argument.
    fn reshape_call(core: &mut Runtime, pallet: &str, call: &str, field: &str) {
        let calls = core
            .pallet(pallet)
            .and_then(|pallet| pallet.calls_type)
            .expect("call enum");
        let variant = enum_variants(core, calls)
            .iter_mut()
            .find(|variant| variant.name == call)
            .expect("call variant");
        let first = variant.fields[0].ty;
        let target = variant
            .fields
            .iter_mut()
            .find(|candidate| candidate.name.as_deref() == Some(field))
            .expect("call field");
        assert_ne!(target.ty, first, "{pallet}.{call}.{field} must change type");
        target.ty = first;
    }

    #[test]
    fn pinned_runtime_satisfies_every_validator_dependency() {
        let runtime = handle(pinned());
        let surface = ValidatorSurface::check(&runtime);
        assert!(surface.is_compatible(), "{:#?}", surface.incompatible);
        assert!(surface.warnings.is_empty(), "{:#?}", surface.warnings);
        assert!(crate::runtime::weight_calls_usable(&runtime, false));
    }

    #[test]
    fn unrelated_breaking_drift_is_only_a_warning() {
        let mut core = pinned();
        reshape_call(&mut core, "SubtensorModule", "burned_register", "hotkey");
        let errors = core
            .pallet("Balances")
            .and_then(|pallet| pallet.errors_type)
            .expect("Balances errors");
        let catalogued = rtao_runtime::errors::MODULE_ERRORS
            .iter()
            .find(|error| error.pallet == "Balances")
            .expect("catalogued Balances error");
        enum_variants(&mut core, errors).retain(|variant| variant.name != catalogued.name);
        let runtime = handle(core);
        let report = runtime.surface_report();
        // The former global gate refused this runtime.
        assert!(report.breaking_items() >= 2, "{:#?}", report.changes);
        let surface = ValidatorSurface::check(&runtime);
        assert!(surface.is_compatible(), "{:#?}", surface.incompatible);
        let unrelated = surface
            .warnings
            .iter()
            .map(|change| change.descriptor.as_str())
            .collect::<Vec<_>>();
        assert!(unrelated.contains(&"call:SubtensorModule.burned_register"));
        let error = format!("error:Balances.{}", catalogued.name);
        assert!(unrelated.contains(&error.as_str()), "{unrelated:?}");
        assert!(crate::runtime::weight_calls_usable(&runtime, false));
    }

    #[test]
    fn changed_validator_weight_call_shape_fails_the_check() {
        for call in [
            "set_mechanism_weights",
            "commit_timelocked_mechanism_weights",
        ] {
            let mut core = pinned();
            let field = if call == "set_mechanism_weights" {
                "version_key"
            } else {
                "reveal_round"
            };
            reshape_call(&mut core, "SubtensorModule", call, field);
            let runtime = handle(core);
            assert!(!crate::runtime::weight_calls_usable(&runtime, false));
            let surface = ValidatorSurface::check(&runtime);
            assert!(!surface.is_compatible());
            let id = format!("call:SubtensorModule.{call}");
            assert!(
                surface
                    .incompatible
                    .iter()
                    .any(|line| line.starts_with(&id) && line.contains("name-resolved")),
                "{:#?}",
                surface.incompatible
            );
        }
    }

    /// A weight call whose shape changed without changing its encoding is
    /// composed by name: a warning, not a failure (D-088).
    #[test]
    fn a_weight_call_composed_by_name_is_only_a_warning() {
        let mut core = pinned();
        let calls = core
            .pallet("SubtensorModule")
            .and_then(|pallet| pallet.calls_type)
            .expect("call enum");
        let commit = enum_variants(&mut core, calls)
            .iter()
            .find(|variant| variant.name == "commit_timelocked_mechanism_weights")
            .and_then(|variant| {
                variant
                    .fields
                    .iter()
                    .find(|f| f.name.as_deref() == Some("commit"))
            })
            .expect("commit field")
            .ty
            .id;
        let TypeDef::Composite(bounded) = &core.resolve(commit).expect("type").type_def else {
            unreachable!("commit is a BoundedVec newtype")
        };
        let plain = bounded.fields[0].ty;
        enum_variants(&mut core, calls)
            .iter_mut()
            .find(|variant| variant.name == "commit_timelocked_mechanism_weights")
            .and_then(|variant| {
                variant
                    .fields
                    .iter_mut()
                    .find(|f| f.name.as_deref() == Some("commit"))
            })
            .expect("commit field")
            .ty = plain;
        let runtime = handle(core);
        let id = "call:SubtensorModule.commit_timelocked_mechanism_weights";
        assert!(!runtime.surface_report().shape_matches(id));
        assert!(crate::runtime::weight_calls_usable(&runtime, false));
        let surface = ValidatorSurface::check(&runtime);
        assert!(surface.is_compatible(), "{:#?}", surface.incompatible);
        let warnings = surface
            .warnings
            .iter()
            .filter(|change| change.descriptor == id)
            .collect::<Vec<_>>();
        assert_eq!(warnings.len(), 1, "{warnings:#?}");
        assert!(
            warnings[0].detail.contains("composes by name"),
            "{}",
            warnings[0].detail
        );
        // With an encoder installed the scheduler composes through it before
        // trying name resolution, and the surface check names it.
        let surface = ValidatorSurface::check_with_encoder(&runtime, true);
        assert!(surface.is_compatible(), "{:#?}", surface.incompatible);
        let warnings = surface
            .warnings
            .iter()
            .filter(|change| change.descriptor == id)
            .collect::<Vec<_>>();
        assert_eq!(warnings.len(), 1, "{warnings:#?}");
        assert!(
            warnings[0]
                .detail
                .contains("composes through the application call encoder"),
            "{}",
            warnings[0].detail
        );
        assert!(
            !warnings[0].detail.contains("by name"),
            "{}",
            warnings[0].detail
        );
        assert_eq!(
            compose::route(&runtime, true, true).ok(),
            Some(compose::Route::Application)
        );
        assert_eq!(
            compose::route(&runtime, true, false).ok(),
            Some(compose::Route::Resolved)
        );
        assert_eq!(
            compose::route(&runtime, false, true).ok(),
            Some(compose::Route::Compiled)
        );
    }

    /// An installed application encoder covers a weight call name
    /// resolution refuses.
    #[test]
    fn an_encoder_covers_an_incompatible_weight_call() {
        let mut core = pinned();
        reshape_call(
            &mut core,
            "SubtensorModule",
            "set_mechanism_weights",
            "version_key",
        );
        let runtime = handle(core);
        let id = "call:SubtensorModule.set_mechanism_weights";
        assert!(fails_on(&ValidatorSurface::check(&runtime), id));
        assert!(!crate::runtime::weight_calls_usable(&runtime, false));
        assert!(crate::runtime::weight_calls_usable(&runtime, true));
        let surface = ValidatorSurface::check_with_encoder(&runtime, true);
        assert!(surface.is_compatible(), "{:#?}", surface.incompatible);
        assert!(warned(&surface, id));
    }

    fn add_type(core: &mut Runtime, path: &[&str], type_def: TypeDef<PortableForm>) -> u32 {
        let id = u32::try_from(core.types.types.len()).expect("type id");
        core.types.types.push(scale_info::PortableType {
            id,
            ty: scale_info::Type {
                path: scale_info::Path {
                    segments: path.iter().map(|segment| (*segment).to_owned()).collect(),
                },
                type_params: vec![],
                type_def,
                docs: vec![],
            },
        });
        id
    }

    fn named(name: &str, ty: u32) -> scale_info::Field<PortableForm> {
        scale_info::Field {
            name: Some(name.into()),
            ty: ty.into(),
            type_name: None,
            docs: vec![],
        }
    }

    fn storage<'a>(core: &'a mut Runtime, name: &str) -> &'a mut rtao_core::runtime::StorageInfo {
        core.pallets
            .iter_mut()
            .find(|pallet| pallet.name == "SubtensorModule")
            .and_then(|pallet| pallet.storage.iter_mut().find(|entry| entry.name == name))
            .expect("storage entry")
    }

    /// One metadata change applied to the pinned runtime.
    type Edit = fn(&mut Runtime);

    fn widened_tempo(core: &mut Runtime) {
        let wide = add_type(
            core,
            &[],
            TypeDef::Primitive(scale_info::TypeDefPrimitive::U32),
        );
        let tempo = storage(core, "Tempo");
        tempo.value_type = wide;
        tempo.default_bytes.resize(4, 0);
    }

    fn bounded_last_update(core: &mut Runtime) {
        let inner = storage(core, "LastUpdate").value_type;
        let bounded = add_type(
            core,
            &["bounded_collections", "BoundedVec"],
            TypeDef::Composite(scale_info::TypeDefComposite {
                fields: vec![scale_info::Field {
                    name: None,
                    ty: inner.into(),
                    type_name: None,
                    docs: vec![],
                }],
            }),
        );
        storage(core, "LastUpdate").value_type = bounded;
    }

    /// The commit queue element as a named struct; `block` names the
    /// commit block field.
    fn commit_struct(core: &mut Runtime, block: &str) {
        let queue = storage(core, "TimelockedWeightCommits").value_type;
        let TypeDef::Sequence(sequence) = &core.resolve(queue).expect("queue").type_def else {
            unreachable!("the commit queue is a sequence")
        };
        let TypeDef::Tuple(tuple) = &core
            .resolve(sequence.type_param.id)
            .expect("commit")
            .type_def
        else {
            unreachable!("a commit is a tuple")
        };
        let [who, at, commit, round] = [0, 1, 2, 3].map(|index| tuple.fields[index].id);
        let element = add_type(
            core,
            &["pallet_subtensor", "TimelockedCommit"],
            TypeDef::Composite(scale_info::TypeDefComposite {
                fields: vec![
                    named(block, at),
                    named("who", who),
                    named("reveal_round", round),
                    named("commit", commit),
                ],
            }),
        );
        let queue = add_type(
            core,
            &[],
            TypeDef::Sequence(scale_info::TypeDefSequence {
                type_param: element.into(),
            }),
        );
        storage(core, "TimelockedWeightCommits").value_type = queue;
    }

    fn slot_duration_u32(core: &mut Runtime) {
        let narrow = add_type(
            core,
            &[],
            TypeDef::Primitive(scale_info::TypeDefPrimitive::U32),
        );
        let constant = core
            .pallets
            .iter_mut()
            .find(|pallet| pallet.name == "Aura")
            .and_then(|pallet| {
                pallet
                    .constants
                    .iter_mut()
                    .find(|constant| constant.name == "SlotDuration")
            })
            .expect("SlotDuration");
        constant.ty = narrow;
        constant.value = 12_000u32.to_le_bytes().to_vec();
    }

    /// Storage and constant shape changes the scheduler reads by name are
    /// warnings, as at run time (D-089).
    #[test]
    fn storage_and_constant_changes_read_by_name_are_warnings() {
        let cases: [(&str, Edit); 4] = [
            ("storage:SubtensorModule.Tempo", widened_tempo),
            ("storage:SubtensorModule.LastUpdate", bounded_last_update),
            ("storage:SubtensorModule.TimelockedWeightCommits", |core| {
                commit_struct(core, "commit_block")
            }),
            ("constant:Aura.SlotDuration", slot_duration_u32),
        ];
        for (id, edit) in cases {
            let mut core = pinned();
            edit(&mut core);
            let runtime = handle(core);
            assert!(!runtime.surface_report().shape_matches(id), "{id}");
            let surface = ValidatorSurface::check(&runtime);
            assert!(surface.is_compatible(), "{id}: {:#?}", surface.incompatible);
            let warnings = surface
                .warnings
                .iter()
                .filter(|change| change.descriptor == id)
                .collect::<Vec<_>>();
            assert_eq!(warnings.len(), 1, "{id}: {warnings:#?}");
            assert!(
                warnings[0].detail.contains("resolves by name"),
                "{}",
                warnings[0].detail
            );
            assert_eq!(warnings[0].class, rtao_runtime::SurfaceClass::Behavioural);
        }
    }

    /// Changes whose meaning a by-name read cannot recover still fail.
    #[test]
    fn storage_changes_without_a_recoverable_meaning_fail_the_check() {
        let cases: [(&str, Edit, &str); 4] = [
            (
                "storage:SubtensorModule.TimelockedWeightCommits",
                |core| commit_struct(core, "included_at"),
                "`commit_block` is absent",
            ),
            (
                "storage:SubtensorModule.Tempo",
                |core| {
                    let tempo = storage(core, "Tempo");
                    tempo.key_types.push(tempo.key_types[0]);
                    tempo.hashers.push("Identity".into());
                },
                "2 keys, expected 1",
            ),
            (
                "storage:SubtensorModule.ValidatorPermit",
                |core| {
                    let signed = add_type(
                        core,
                        &[],
                        TypeDef::Primitive(scale_info::TypeDefPrimitive::I8),
                    );
                    let list = add_type(
                        core,
                        &[],
                        TypeDef::Sequence(scale_info::TypeDefSequence {
                            type_param: signed.into(),
                        }),
                    );
                    storage(core, "ValidatorPermit").value_type = list;
                },
                "element is i8",
            ),
            (
                "storage:SubtensorModule.Uids",
                |core| storage(core, "Uids").modifier = "Default".into(),
                "no longer Optional",
            ),
        ];
        for (id, edit, reason) in cases {
            let mut core = pinned();
            edit(&mut core);
            let surface = ValidatorSurface::check(&handle(core));
            assert!(fails_on(&surface, id), "{id}: {:#?}", surface.incompatible);
            let line = surface
                .incompatible
                .iter()
                .find(|line| line.starts_with(&format!("{id}:")))
                .expect("failure line");
            assert!(line.contains("name-resolved read refused"), "{line}");
            assert!(line.contains(reason), "{line}");
        }
    }

    #[test]
    fn missing_preflight_storage_fails_the_check() {
        let mut core = pinned();
        let pallet = core
            .pallets
            .iter_mut()
            .find(|pallet| pallet.name == "SubtensorModule")
            .expect("SubtensorModule");
        pallet
            .storage
            .retain(|entry| entry.name != "WeightsSetRateLimit");
        let runtime = handle(core);
        let surface = ValidatorSurface::check(&runtime);
        assert!(
            surface
                .incompatible
                .iter()
                .any(|line| line.starts_with("storage:SubtensorModule.WeightsSetRateLimit")),
            "{:#?}",
            surface.incompatible
        );
    }

    #[test]
    fn missing_run_path_storage_and_constants_fail_the_check() {
        let mut core = pinned();
        core.pallets
            .iter_mut()
            .find(|pallet| pallet.name == "Drand")
            .expect("Drand")
            .storage
            .retain(|entry| entry.name != "LastStoredRound");
        let surface = ValidatorSurface::check(&handle(core));
        assert!(
            fails_on(&surface, "storage:Drand.LastStoredRound"),
            "{:#?}",
            surface.incompatible
        );
        for (pallet, name) in [
            ("SubtensorModule", "MechanismCountCurrent"),
            ("SubtensorModule", "ActivityCutoffFactorMilli"),
            ("SubtensorModule", "StakeThreshold"),
            ("SubtensorModule", "SubnetOwnerHotkey"),
            ("SubtensorModule", "TimelockedWeightCommits"),
        ] {
            let mut core = pinned();
            core.pallets
                .iter_mut()
                .find(|candidate| candidate.name == pallet)
                .expect("pallet")
                .storage
                .retain(|entry| entry.name != name);
            let surface = ValidatorSurface::check(&handle(core));
            let id = format!("storage:{pallet}.{name}");
            assert!(fails_on(&surface, &id), "{id}: {:#?}", surface.incompatible);
        }
        let mut core = pinned();
        core.pallets
            .iter_mut()
            .find(|pallet| pallet.name == "Aura")
            .expect("Aura")
            .constants
            .retain(|constant| constant.name != "SlotDuration");
        let surface = ValidatorSurface::check(&handle(core));
        assert!(
            fails_on(&surface, "constant:Aura.SlotDuration"),
            "{:#?}",
            surface.incompatible
        );
    }

    #[test]
    fn selective_metagraph_projection_change_warns_but_removal_fails() {
        const API: &str = "api:SubnetInfoRuntimeApi.get_selective_mechagraph";
        assert_eq!(
            format!(
                "api:{}.{}",
                super::super::METAGRAPH_API.api,
                super::super::METAGRAPH_API.name
            ),
            API
        );
        let mut core = pinned();
        let model = core
            .types
            .types
            .iter_mut()
            .find(|ty| {
                ty.ty
                    .path
                    .segments
                    .last()
                    .is_some_and(|name| name == "SelectiveMetagraph")
            })
            .expect("SelectiveMetagraph");
        let TypeDef::Composite(composite) = &mut model.ty.type_def else {
            unreachable!("SelectiveMetagraph is a struct")
        };
        let field = composite
            .fields
            .iter_mut()
            .find(|field| field.name.as_deref() == Some("hotkeys"))
            .expect("hotkeys field");
        field.name = Some("renamed_hotkeys".into());
        let runtime = handle(core);
        assert!(!runtime.surface_report().shape_matches(API));
        let surface = ValidatorSurface::check(&runtime);
        assert!(surface.is_compatible(), "{:#?}", surface.incompatible);
        assert!(warned(&surface, API), "{:#?}", surface.warnings);

        let mut core = pinned();
        core.apis
            .iter_mut()
            .find(|api| api.name == "SubnetInfoRuntimeApi")
            .expect("SubnetInfoRuntimeApi")
            .methods
            .retain(|method| method.name != "get_selective_mechagraph");
        let surface = ValidatorSurface::check(&handle(core));
        assert!(fails_on(&surface, API), "{:#?}", surface.incompatible);

        let mut core = pinned();
        core.apis
            .iter_mut()
            .find(|api| api.name == "SubnetInfoRuntimeApi")
            .expect("SubnetInfoRuntimeApi")
            .methods
            .iter_mut()
            .find(|method| method.name == "get_selective_mechagraph")
            .expect("get_selective_mechagraph")
            .inputs
            .swap(0, 1);
        let surface = ValidatorSurface::check(&handle(core));
        assert!(fails_on(&surface, API), "{:#?}", surface.incompatible);

        let surface = ValidatorSurface::check(&RuntimeHandle::new(pinned()).expect("runtime"));
        assert!(fails_on(&surface, API), "unknown API version must refuse");
    }
}
