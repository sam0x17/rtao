use super::*;
use rtao_client::{ClientConfig, Network, ReadKind, ReadObserver, ReadTouch};
use rtao_primitives::{IntoCoreValue, Uid};
use rtao_runtime::models::{ProxyDefinition, ProxyType};
use rtao_testkit::{FakeChain, FakeState, rpc::RpcClient};
use serde_json::json;
use std::sync::{Arc, Mutex};

const NETUID: NetUid = NetUid(3);
const OWNER: AccountId = AccountId([7; 32]);
const OWNER_HOTKEY: AccountId = AccountId([8; 32]);
const CHALLENGER: AccountId = AccountId([9; 32]);
const DELEGATE: AccountId = AccountId([10; 32]);

#[derive(Debug, Default)]
struct Touches(Mutex<Vec<ReadTouch>>);
impl ReadObserver for Touches {
    fn on_read(&self, touch: ReadTouch) {
        if let Ok(mut touches) = self.0.lock() {
            touches.push(touch);
        }
    }
}
async fn fixture(
    setup: impl FnOnce(&mut FakeState) -> anyhow::Result<()>,
) -> anyhow::Result<(FakeChain, Client, OwnerMonitor)> {
    let mut state = FakeState::try_default()?;
    state.seed(&st::NETWORKS_ADDED, (NETUID,), true)?;
    state.seed(&st::SUBNET_OWNER, (NETUID,), OWNER.into())?;
    state.seed(&st::SUBNET_OWNER_HOTKEY, (NETUID,), OWNER_HOTKEY.into())?;
    state.seed(&st::SUBNET_ALPHA_OUT, (NETUID,), AlphaBalance(12_000))?;
    state.seed(&st::SUBNET_PROTOCOL_ALPHA, (NETUID,), AlphaBalance(1_000))?;
    state.seed_typed("AlphaAssets", "AlphaBurned", (NETUID,), AlphaBalance(1_000))?;
    state.seed(&st::SUBTOKEN_ENABLED, (NETUID,), true)?;
    state.seed(&st::SUBNET_EMISSION_ENABLED, (NETUID,), false)?;
    state.seed(&st::SUBNETWORK_N, (NETUID,), 2u16)?;
    state.seed(&st::MAX_ALLOWED_UIDS, (NETUID,), 2u16)?;
    state.seed(&st::IMMUNITY_PERIOD, (NETUID,), 0u16)?;
    state.seed(&st::MIN_NON_IMMUNE_UIDS, (NETUID,), 0u16)?;
    state.seed(&st::IMMUNE_OWNER_UIDS_LIMIT, (NETUID,), 1u16)?;
    state.seed(&st::KEYS, (NETUID, 0u16), OWNER_HOTKEY.into())?;
    state.seed(&st::KEYS, (NETUID, 1u16), CHALLENGER.into())?;
    state.seed(&st::OWNER, (OWNER_HOTKEY,), OWNER.into())?;
    state.seed(&st::OWNER, (CHALLENGER,), AccountId([12; 32]).into())?;
    state.seed(
        &st::EMISSION,
        (NETUID,),
        vec![AlphaBalance(100), AlphaBalance(0)],
    )?;
    state.seed_runtime(
        "StakeInfoRuntimeApi",
        "get_most_convicted_hotkey_on_subnet",
        json!(CHALLENGER.to_string()),
    )?;
    state.seed_runtime(
        "StakeInfoRuntimeApi",
        "get_hotkey_conviction",
        json!({"bits": 1800u128 << 64}),
    )?;
    setup(&mut state)?;
    let chain = FakeChain::from_state(state);
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    let client = Client::from_rpc(RpcClient::new(chain.clone()), config).await?;
    let monitor = OwnerMonitor::new(client.clone(), NETUID)?.with_hotkeys([CHALLENGER]);
    Ok((chain, client, monitor))
}
fn conviction(eligible: u64, bits: u128, block: u64) -> OwnerConviction {
    OwnerConviction::evaluate(
        block,
        0,
        OWNER_HOTKEY,
        Some(CHALLENGER),
        U64F64::from_bits(bits),
        AlphaBalance(eligible),
        AlphaBalance(0),
        AlphaBalance(0),
    )
}
#[test]
fn conviction_strict_boundary_keeps_fractional_bits_and_native_maximum() {
    for (amount, clears) in [(1799, false), (1800, false), (1801, true)] {
        assert_eq!(
            conviction(10_000, amount << 64, OWNERSHIP_AGE).clears_threshold,
            clears
        );
    }
    assert!(conviction(10_000, (1800u128 << 64) + 1, OWNERSHIP_AGE).clears_threshold);
    assert!(!conviction(0, u128::MAX, OWNERSHIP_AGE).clears_threshold);
    let state = conviction(u64::MAX, u128::MAX, OWNERSHIP_AGE);
    assert!(state.clears_threshold);
    // An independent quotient/remainder comparison against the rational gate.
    let numerator = u128::from(u64::MAX) * 18;
    let quotient = numerator / 100;
    let remainder = numerator % 100;
    let fractional_floor = (remainder << 64) / 100;
    let boundary = (quotient << 64) + fractional_floor;
    assert_eq!(state.threshold.bits, boundary);
    assert!(!conviction(u64::MAX, boundary, OWNERSHIP_AGE).clears_threshold);
    assert!(conviction(u64::MAX, boundary + 1, OWNERSHIP_AGE).clears_threshold);
}
#[test]
fn conviction_saturates_exclusions_and_uses_source_age_gate() {
    let state = OwnerConviction::evaluate(
        OWNERSHIP_AGE - 1,
        0,
        OWNER_HOTKEY,
        Some(CHALLENGER),
        U64F64::from_bits(1801u128 << 64),
        AlphaBalance(12_000),
        AlphaBalance(1_000),
        AlphaBalance(1_000),
    );
    assert_eq!(state.eligible_alpha, AlphaBalance(10_000));
    assert!(state.clears_threshold && !state.old_enough && !state.challenger_clears_gates);
    assert!(conviction(10_000, 1801u128 << 64, OWNERSHIP_AGE).challenger_clears_gates);
    let owner = OwnerConviction::evaluate(
        OWNERSHIP_AGE,
        0,
        OWNER_HOTKEY,
        Some(OWNER_HOTKEY),
        U64F64::from_bits(u128::MAX),
        AlphaBalance(10_000),
        AlphaBalance(0),
        AlphaBalance(0),
    );
    assert!(owner.clears_threshold && !owner.challenger_clears_gates);
    let overdrawn = OwnerConviction::evaluate(
        u64::MAX,
        u64::MAX - 1,
        OWNER_HOTKEY,
        None,
        U64F64::default(),
        AlphaBalance(1),
        AlphaBalance(u64::MAX),
        AlphaBalance(u64::MAX),
    );
    assert_eq!(overdrawn.eligible_alpha, AlphaBalance(0));
    assert_eq!(overdrawn.ownership_changeable_at, u64::MAX);
    assert!(overdrawn.old_enough);
}
#[tokio::test]
async fn sample_keeps_flags_independent_and_reads_all_inputs_at_one_hash() -> anyhow::Result<()> {
    let (chain, client, monitor) = fixture(|_| Ok(())).await?;
    let touches = Arc::new(Touches::default());
    let snapshot = client
        .at(BlockRef::Best)
        .await?
        .with_read_observer(touches.clone());
    chain.advance(1)?;
    let start = chain.requests().len();
    let sample = monitor.sample_at(&snapshot).await?;
    assert!(sample.subtoken_enabled && !sample.subnet_emission_enabled);
    assert_eq!(sample.conviction.eligible_alpha, AlphaBalance(10_000));
    assert_eq!(sample.conviction.leader_hotkey, Some(CHALLENGER));
    assert!(!sample.conviction.clears_threshold);
    assert_eq!(
        sample
            .pruning
            .iter()
            .find(|risk| risk.hotkey == CHALLENGER)
            .map(|risk| (risk.uid, risk.prune_risk)),
        Some((Some(Uid(1)), true))
    );
    let expected_hash = json!(format!("0x{}", hex::encode(snapshot.block().hash.0)));
    for request in &chain.requests()[start..] {
        if request.method == "state_queryStorageAt" {
            assert_eq!(request.params[1], expected_hash);
        }
        if request.method == "state_call" {
            assert_eq!(request.params[2], expected_hash);
        }
        if request.method == "state_getStorage" {
            assert_eq!(request.params[1], expected_hash);
        }
    }
    {
        let observed = touches
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("observer lock"))?;
        assert!(
            observed
                .iter()
                .any(|touch| touch.item == "AlphaBurned" && touch.kind == ReadKind::Storage)
        );
        assert!(
            observed
                .iter()
                .all(|touch| touch.at == Some(snapshot.block().hash))
        );
    }
    client.close().await;
    Ok(())
}
#[tokio::test]
async fn announcements_are_delegate_keyed_owner_filtered_and_preserve_multiple_definitions()
-> anyhow::Result<()> {
    let (_chain, client, monitor) = fixture(|state| {
        state.seed(
            &proxy::PROXIES,
            (OWNER,),
            (
                vec![
                    ProxyDefinition {
                        delegate: DELEGATE,
                        proxy_type: ProxyType::Owner,
                        delay: 20,
                    },
                    ProxyDefinition {
                        delegate: DELEGATE,
                        proxy_type: ProxyType::Any,
                        delay: 5,
                    },
                ],
                TaoBalance(88),
            ),
        )?;
        state.seed(
            &proxy::ANNOUNCEMENTS,
            (DELEGATE,),
            (
                vec![
                    DynValue::new(Value::record(vec![
                        ("real".into(), OWNER.into_core_value()),
                        ("call_hash".into(), Value::hex(&[4; 32])),
                        ("height".into(), 95u32.into_core_value()),
                    ])),
                    DynValue::new(Value::record(vec![
                        ("real".into(), OWNER.into_core_value()),
                        ("call_hash".into(), Value::hex(&[4; 32])),
                        ("height".into(), 96u32.into_core_value()),
                    ])),
                    DynValue::new(Value::record(vec![
                        ("real".into(), CHALLENGER.into_core_value()),
                        ("call_hash".into(), Value::hex(&[5; 32])),
                        ("height".into(), 1u32.into_core_value()),
                    ])),
                ],
                TaoBalance(123),
            ),
        )?;
        Ok(())
    })
    .await?;
    let sample = monitor.sample().await?;
    assert_eq!(sample.proxy_announcements.len(), 2);
    let first = &sample.proxy_announcements[0];
    assert_eq!(
        (first.delegate, first.real, first.call_hash),
        (DELEGATE, OWNER, H256([4; 32]))
    );
    assert_eq!(first.definitions.len(), 2);
    assert_eq!(first.earliest_execution, Some(100));
    assert!(first.delay_elapsed);
    assert_eq!(first.delegate_deposit, TaoBalance(123));
    assert!(!sample.proxy_announcements[1].delay_elapsed);
    client.close().await;
    Ok(())
}
fn event(name: &str, attributes: serde_json::Value) -> serde_json::Value {
    json!({"phase": {"Initialization": null}, "event": {"SubtensorModule": {name: attributes}}, "topics": []})
}
fn header(state: &FakeState, number: u64) -> anyhow::Result<BlockHeader> {
    let hash = state
        .block_hash(number)
        .ok_or_else(|| anyhow::anyhow!("missing block"))?;
    BlockHeader::from_json(
        state
            .header(&hash)
            .ok_or_else(|| anyhow::anyhow!("missing header"))?,
    )
    .map_err(Into::into)
}
#[tokio::test]
async fn events_filter_named_positional_and_queued_owner_records() -> anyhow::Result<()> {
    let (chain, client, monitor) = fixture(|state| {
        let hash = state.block_hash(100).ok_or_else(|| anyhow::anyhow!("block"))?;
        state.seed_events(hash, json!([
            event("NetworkAdded", json!([NETUID.0, 0])),
            event("NetworkAdded", json!([4, 0])),
            event("EpochDeferred", json!({"netuid": NETUID.0, "from_block": 100, "to_block": 101})),
            event("SubnetOwnerHotkeySet", json!([NETUID.0, CHALLENGER.to_string()])),
            event("NetworkRegistrationQueued", json!({"coldkey": OWNER.to_string(), "hotkey": CHALLENGER.to_string(), "mechid": 0, "identity": null, "lock_amount": 1, "median_subnet_alpha_price": {"bits":0}, "registration_block":100})),
            event("NetworkRegistrationQueued", json!({"coldkey": CHALLENGER.to_string(), "hotkey": CHALLENGER.to_string(), "mechid": 0, "identity": null, "lock_amount": 1, "median_subnet_alpha_price": {"bits":0}, "registration_block":100})),
        ]))?;
        Ok(())
    }).await?;
    let snapshot = client.at(BlockRef::Number(100)).await?;
    let events = monitor.events_at(&snapshot).await?;
    assert_eq!(
        events
            .iter()
            .map(|event| event.cursor.event_index)
            .collect::<Vec<_>>(),
        [0, 2, 3, 4]
    );
    assert_eq!(events[0].record.phase, "Initialization");
    assert!(
        events
            .iter()
            .all(|event| event.cursor.block_hash == snapshot.block().hash)
    );
    assert!(
        chain
            .requests()
            .iter()
            .any(|request| request.method == "state_getStorage")
    );
    client.close().await;
    Ok(())
}
#[tokio::test]
async fn finalized_stream_backfills_resume_without_duplicates_and_cancels_pending_items()
-> anyhow::Result<()> {
    let (chain, client, monitor) = fixture(|state| {
        for number in 98..=100 {
            let hash = state
                .block_hash(number)
                .ok_or_else(|| anyhow::anyhow!("block"))?;
            state.seed_events(
                hash,
                json!([
                    event("TempoSet", json!([NETUID.0, 360])),
                    event("FirstEmissionBlockNumberSet", json!([NETUID.0, number + 1])),
                ]),
            )?;
        }
        Ok(())
    })
    .await?;
    let headers = chain.mutate_state(|state| -> anyhow::Result<Vec<BlockHeader>> {
        Ok(vec![header(state, 100)?])
    })?;
    let cursor = monitor
        .events_at(&client.at(BlockRef::Number(98)).await?)
        .await?[0]
        .cursor;
    let cancel = CancellationToken::new();
    let mut stream = event_stream(
        monitor,
        Box::pin(futures::stream::iter(headers.into_iter().map(Ok))),
        Some(cursor),
        cancel.clone(),
    );
    let mut observed = Vec::new();
    for _ in 0..4 {
        observed.push(
            stream
                .next()
                .await
                .ok_or_else(|| anyhow::anyhow!("stream ended"))??
                .cursor,
        );
    }
    assert_eq!(
        observed
            .iter()
            .map(|cursor| (cursor.block, cursor.event_index))
            .collect::<Vec<_>>(),
        [(98, 1), (99, 0), (99, 1), (100, 0)]
    );
    cancel.cancel();
    assert!(stream.next().await.is_none());
    client.close().await;
    Ok(())
}
#[tokio::test]
async fn event_follower_errors_are_terminal_and_cursor_identity_is_checked() -> anyhow::Result<()> {
    let (chain, client, monitor) = fixture(|state| {
        let hash = state
            .block_hash(100)
            .ok_or_else(|| anyhow::anyhow!("block"))?;
        state.seed_events(hash, json!([event("TempoSet", json!([NETUID.0, 360]))]))?;
        Ok(())
    })
    .await?;
    let current = chain.mutate_state(|state| header(state, 100))?;
    for cursor in [
        OwnerEventCursor {
            block: 101,
            block_hash: current.hash,
            event_index: 0,
        },
        OwnerEventCursor {
            block: 100,
            block_hash: H256([0; 32]),
            event_index: 0,
        },
        OwnerEventCursor {
            block: 100,
            block_hash: current.hash,
            event_index: 1,
        },
    ] {
        let mut stream = event_stream(
            monitor.clone(),
            Box::pin(futures::stream::iter([Ok(current.clone())])),
            Some(cursor),
            CancellationToken::new(),
        );
        assert!(matches!(
            stream.next().await,
            Some(Err(OwnerMonitorError::InvalidCursor(_)))
        ));
        assert!(stream.next().await.is_none());
    }
    let mut ended = event_stream(
        monitor.clone(),
        Box::pin(futures::stream::empty()),
        None,
        CancellationToken::new(),
    );
    assert!(matches!(
        ended.next().await,
        Some(Err(OwnerMonitorError::FollowerEnded))
    ));
    assert!(ended.next().await.is_none());
    let cancellation = CancellationToken::new();
    let mut waiting = event_stream(
        monitor,
        Box::pin(futures::stream::pending()),
        None,
        cancellation.clone(),
    );
    let mut cancel = waiting.as_mut();
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(5), cancel.next())
            .await
            .is_err()
    );
    cancellation.cancel();
    assert!(waiting.next().await.is_none());
    let mut failed = event_stream(
        OwnerMonitor::new(client.clone(), NETUID)?,
        Box::pin(futures::stream::iter([Err(HeadError::InvalidHeader(
            "source failure".into(),
        ))])),
        None,
        CancellationToken::new(),
    );
    assert!(matches!(
        failed.next().await,
        Some(Err(OwnerMonitorError::Head(_)))
    ));
    assert!(failed.next().await.is_none());
    client.close().await;
    Ok(())
}
#[test]
fn malformed_native_values_are_refused() {
    // Native parsing helpers also reject malformed account/hash/event shapes.
    assert!(account(&Value::Str("bad".into())).is_err());
    assert!(hash(&Value::Bytes(vec![1; 31])).is_err());
    assert!(uint(&Value::Uint(u128::MAX), "height").is_err());
    assert!(event_field(&Value::Null, "netuid", 0).is_err());
}

#[tokio::test]
async fn pinned_burn_surface_matches_source_contract_and_absent_keys_default_to_zero()
-> anyhow::Result<()> {
    let (_chain, client, monitor) = fixture(|state| {
        let entry = state
            .runtime
            .storage_entry("AlphaAssets", "AlphaBurned")
            .ok_or_else(|| anyhow::anyhow!("burn entry"))?;
        let key = state
            .runtime
            .storage_key(entry, &[NETUID.into_core_value()])?;
        state.seed_encoded(key, None);
        Ok(())
    })
    .await?;
    let snapshot = client.at(BlockRef::Best).await?;
    let report = rtao_runtime::verify_surface_with(
        &snapshot.block().runtime,
        &[SurfaceDescriptor::Storage(ALPHA_BURNED.surface())],
    );
    assert!(
        report.shapes_match() && report.changes.is_empty(),
        "{report:?}"
    );
    let entry = snapshot
        .block()
        .runtime
        .storage_entry("AlphaAssets", "AlphaBurned")
        .ok_or_else(|| anyhow::anyhow!("burn entry"))?;
    assert_eq!(entry.hashers, ["Twox64Concat"]);
    assert_eq!(entry.modifier, "Default");
    assert_eq!(entry.default_bytes, [0; 8]);
    let sample = monitor.sample_at(&snapshot).await?;
    assert_eq!(sample.conviction.alpha_burned, AlphaBalance(0));
    assert_eq!(sample.conviction.eligible_alpha, AlphaBalance(11_000));
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn malformed_burn_and_announcement_wire_data_fail_closed() -> anyhow::Result<()> {
    for item in ["AlphaBurned", "Announcements"] {
        let (_chain, client, monitor) = fixture(|state| {
            let (pallet, keys) = if item == "AlphaBurned" {
                ("AlphaAssets", vec![NETUID.into_core_value()])
            } else {
                state.seed(
                    &proxy::PROXIES,
                    (OWNER,),
                    (
                        vec![ProxyDefinition {
                            delegate: DELEGATE,
                            proxy_type: ProxyType::Owner,
                            delay: 1,
                        }],
                        TaoBalance(1),
                    ),
                )?;
                ("Proxy", vec![DELEGATE.into_core_value()])
            };
            let entry = state
                .runtime
                .storage_entry(pallet, item)
                .ok_or_else(|| anyhow::anyhow!("entry"))?;
            let key = state.runtime.storage_key(entry, &keys)?;
            state.seed_encoded(key, Some(vec![255]));
            Ok(())
        })
        .await?;
        assert!(
            monitor.sample().await.is_err(),
            "malformed {item} was accepted"
        );
        client.close().await;
    }
    Ok(())
}

#[tokio::test]
async fn samples_refresh_silent_owner_changes_and_absent_leader_without_fake_api_values()
-> anyhow::Result<()> {
    let (chain, client, monitor) = fixture(|state| {
        state.seed_runtime(
            "StakeInfoRuntimeApi",
            "get_most_convicted_hotkey_on_subnet",
            serde_json::Value::Null,
        )?;
        Ok(())
    })
    .await?;
    let sample = monitor.sample().await?;
    assert_eq!(sample.conviction.leader_hotkey, None);
    assert_eq!(sample.conviction.leader_conviction.bits, 0);
    assert!(
        !chain
            .requests()
            .iter()
            .any(|request| request.method == "state_call"
                && request.params[0].as_str() == Some("StakeInfoRuntimeApi_get_hotkey_conviction"))
    );
    chain.advance(1)?;
    chain.mutate_state(|state| state.seed(&st::SUBNET_OWNER, (NETUID,), CHALLENGER.into()))?;
    let changed = monitor.sample().await?;
    assert_eq!(changed.owner_coldkey, CHALLENGER);
    assert_ne!(changed.block_hash, sample.block_hash);
    assert!(OwnerMonitor::new(client.clone(), NetUid::ROOT).is_err());
    assert!(OwnerMonitor::new(client.clone(), NetUid(u16::MAX)).is_err());
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn removed_delegate_pending_rows_remain_visible_without_execution_readiness()
-> anyhow::Result<()> {
    let (_chain, client, monitor) = fixture(|state| {
        // No Proxy.Proxies definition remains for DELEGATE.
        state.seed(
            &proxy::ANNOUNCEMENTS,
            (DELEGATE,),
            (
                vec![DynValue::new(Value::record(vec![
                    ("real".into(), OWNER.into_core_value()),
                    ("call_hash".into(), Value::hex(&[6; 32])),
                    ("height".into(), 1u32.into_core_value()),
                ]))],
                TaoBalance(23),
            ),
        )?;
        Ok(())
    })
    .await?;
    let sample = monitor.sample().await?;
    assert_eq!(sample.proxy_announcements.len(), 1);
    let row = &sample.proxy_announcements[0];
    assert_eq!(row.delegate, DELEGATE);
    assert!(row.definitions.is_empty());
    assert_eq!(row.earliest_execution, None);
    assert!(!row.delay_elapsed);
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn owner_events_cover_bare_identity_lease_admin_and_proxy_shapes() -> anyhow::Result<()> {
    let (_chain, client, monitor) = fixture(|state| {
        let hash = state.block_hash(100).ok_or_else(|| anyhow::anyhow!("block"))?;
        state.seed_events(hash, json!([
            event("SubnetIdentitySet", json!(NETUID.0)),
            event("SubnetLeaseCreated", json!({"beneficiary": OWNER.to_string(), "lease_id":8, "netuid":NETUID.0, "end_block":null})),
            {"phase":{"Initialization":null}, "event":{"AdminUtils":{"CollateralLockShareSet":{"netuid":NETUID.0,"lock_share":1}}}, "topics":[]},
            {"phase":{"Initialization":null}, "event":{"Proxy":{"Announced":{"real":OWNER.to_string(),"proxy":DELEGATE.to_string(),"call_hash":format!("0x{}",hex::encode([4;32]))}}}, "topics":[]},
            {"phase":{"Initialization":null}, "event":{"Proxy":{"Announced":{"real":CHALLENGER.to_string(),"proxy":DELEGATE.to_string(),"call_hash":format!("0x{}",hex::encode([4;32]))}}}, "topics":[]},
            {"phase":{"Initialization":null}, "event":{"Proxy":{"ProxyAdded":{"delegator":OWNER.to_string(),"delegatee":DELEGATE.to_string(),"proxy_type":"Owner","delay":5}}}, "topics":[]},
            {"phase":{"Initialization":null}, "event":{"Proxy":{"ProxyRemoved":{"delegator":CHALLENGER.to_string(),"delegatee":DELEGATE.to_string(),"proxy_type":"Owner","delay":5}}}, "topics":[]},
        ]))?;
        Ok(())
    }).await?;
    let events = monitor
        .events_at(&client.at(BlockRef::Number(100)).await?)
        .await?;
    assert_eq!(
        events
            .iter()
            .map(|event| event.cursor.event_index)
            .collect::<Vec<_>>(),
        [0, 1, 2, 3, 5]
    );
    assert_eq!(
        events[0].record.attributes.as_u128(),
        Some(u128::from(NETUID.0))
    );
    assert_eq!(events[2].record.module_id, "AdminUtils");
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn resume_waits_for_a_stale_tracker_to_reach_an_already_finalized_cursor()
-> anyhow::Result<()> {
    let (chain, client, monitor) = fixture(|state| {
        let hash = state
            .block_hash(100)
            .ok_or_else(|| anyhow::anyhow!("block"))?;
        state.seed_events(
            hash,
            json!([
                event("TempoSet", json!([NETUID.0, 360])),
                event("FirstEmissionBlockNumberSet", json!([NETUID.0, 101])),
            ]),
        )?;
        Ok(())
    })
    .await?;
    let headers = chain.mutate_state(|state| -> anyhow::Result<Vec<BlockHeader>> {
        Ok(vec![header(state, 99)?, header(state, 100)?])
    })?;
    let cursor = monitor
        .events_at(&client.at(BlockRef::Number(100)).await?)
        .await?[0]
        .cursor;
    let cancel = CancellationToken::new();
    let mut stream = event_stream(
        monitor,
        Box::pin(futures::stream::iter(headers.into_iter().map(Ok))),
        Some(cursor),
        cancel.clone(),
    );
    assert_eq!(
        stream
            .next()
            .await
            .ok_or_else(|| anyhow::anyhow!("ended"))??
            .cursor
            .event_index,
        1
    );
    cancel.cancel();
    assert!(stream.next().await.is_none());
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn finalized_backfill_uses_target_ancestry_before_delivering_any_stale_height_fork()
-> anyhow::Result<()> {
    let (chain, client, monitor) = fixture(|state| {
        for number in 98..=100 {
            let hash = state
                .block_hash(number)
                .ok_or_else(|| anyhow::anyhow!("block"))?;
            state.seed_events(hash, json!([event("TempoSet", json!([NETUID.0, 360]))]))?;
        }
        Ok(())
    })
    .await?;
    let cursor = monitor
        .events_at(&client.at(BlockRef::Number(98)).await?)
        .await?[0]
        .cursor;
    let (target, canonical_99) = chain.mutate_state(|state| -> anyhow::Result<_> {
        let canonical_99 = header(state, 99)?.hash;
        let ancestor = state
            .block_hash(98)
            .ok_or_else(|| anyhow::anyhow!("ancestor"))?;
        let stale = state.fork(&ancestor)?;
        state.seed_events(
            stale.clone(),
            json!([event("TempoSet", json!([NETUID.0, 777]))]),
        )?;
        state.set_handler("chain_getBlockHash", move |state, request| {
            let number = request
                .params
                .get(0)
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(state.block);
            let hash = if number == 99 {
                Some(stale.clone())
            } else {
                state.block_hash(number)
            };
            rtao_testkit::Reply::result(json!(hash))
        });
        Ok((header(state, 100)?, canonical_99))
    })?;
    let cancellation = CancellationToken::new();
    let mut stream = event_stream(
        monitor,
        Box::pin(futures::stream::iter([Ok(target)])),
        Some(cursor),
        cancellation.clone(),
    );
    let first = stream
        .next()
        .await
        .ok_or_else(|| anyhow::anyhow!("ended"))??;
    assert_eq!(
        (first.cursor.block, first.cursor.block_hash),
        (99, canonical_99)
    );
    assert_eq!(
        event_field(&first.record.attributes, "tempo", 1)?.as_u128(),
        Some(360)
    );
    let second = stream
        .next()
        .await
        .ok_or_else(|| anyhow::anyhow!("ended"))??;
    assert_eq!(second.cursor.block, 100);
    cancellation.cancel();
    assert!(stream.next().await.is_none());
    client.close().await;
    Ok(())
}
