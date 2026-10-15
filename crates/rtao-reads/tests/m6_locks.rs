//! Lock and collateral reads through authenticated metadata and fake SCALE state.
#![allow(clippy::unwrap_used, clippy::panic)]

use std::collections::BTreeSet;

use rtao_client::{BlockRef, Client, ClientConfig, Network, ReadKind};
use rtao_primitives::{AccountId, NetUid};
use rtao_reads::{HitKind, ReadName, dispatch, reads::m6_locks as locks};
use rtao_testkit::{FakeChain, FakeState, RecordingView, rpc::RpcClient};
use serde_json::{Value, json};

const COLD: AccountId = AccountId([7; 32]);
const HOT: AccountId = AccountId([8; 32]);
const OTHER: AccountId = AccountId([9; 32]);
const OWNER: AccountId = AccountId([10; 32]);

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

fn lock(mass: u64, conviction: u64, last_update: u64) -> Value {
    json!({"locked_mass": mass, "conviction": {"bits": u128::from(conviction) << 64}, "last_update": last_update})
}

fn seed() -> FakeState {
    let mut state = FakeState::default();
    for (item, keys, value) in [
        ("AccountFlags", vec![json!(COLD)], json!(u128::MAX)),
        ("Owner", vec![json!(HOT)], json!(COLD)),
        (
            "Lock",
            vec![json!(COLD), json!(1), json!(HOT)],
            lock(1, 1, 0),
        ),
        (
            "Lock",
            vec![json!(COLD), json!(2), json!(OTHER)],
            lock(2, 2, 0),
        ),
        (
            "LockingColdkeys",
            vec![json!(1), json!(HOT), json!(COLD)],
            json!([]),
        ),
        (
            "LockingColdkeys",
            vec![json!(1), json!(HOT), json!(OTHER)],
            json!([]),
        ),
        ("DecayingLock", vec![json!(COLD), json!(1)], json!(false)),
        ("DecayingLock", vec![json!(COLD), json!(2)], json!(true)),
        (
            "HotkeyLock",
            vec![json!(1), json!(HOT)],
            lock(1000, 1000, 100),
        ),
        (
            "DecayingHotkeyLock",
            vec![json!(1), json!(OTHER)],
            lock(1000, 1000, 100),
        ),
        ("OwnerLock", vec![json!(1)], lock(1000, 1, 100)),
        ("DecayingOwnerLock", vec![json!(1)], lock(500, 1, 100)),
        ("SubnetOwnerHotkey", vec![json!(1)], json!(OWNER)),
        ("SubnetAlphaOut", vec![json!(1)], json!(12000)),
        ("SubnetProtocolAlpha", vec![json!(1)], json!(1000)),
        ("UnlockRate", vec![], json!(1000)),
        ("MaturityRate", vec![], json!(1000)),
        ("NetworkRegisteredAt", vec![json!(1)], json!(81)),
        ("TokenSymbol", vec![json!(1)], json!("Ω")),
        ("CollateralLockShare", vec![json!(1)], json!(32768)),
        (
            "CollateralDrainRatio",
            vec![json!(1)],
            json!({"bits": 2u128 << 64}),
        ),
        (
            "MinerCollateral",
            vec![json!(1), json!(HOT), json!(COLD)],
            json!({"locked":300,"min_locked":100,"earned":17,"drain_ratio":{"bits":2u128<<64}}),
        ),
        (
            "MinerCollateral",
            vec![json!(1), json!(HOT), json!(OTHER)],
            json!({"locked":150,"min_locked":250,"earned":u64::MAX,"drain_ratio":{"bits":0}}),
        ),
        (
            "MinerCollateral",
            vec![json!(1), json!(OTHER), json!(COLD)],
            json!({"locked":450,"min_locked":50,"earned":27,"drain_ratio":{"bits":1u128<<64}}),
        ),
        ("Uids", vec![json!(1), json!(HOT)], json!(0)),
    ] {
        state
            .seed_python("SubtensorModule", item, &keys, value)
            .unwrap();
    }
    state
        .seed_python("AlphaAssets", "AlphaBurned", &[json!(1)], json!(1000))
        .unwrap();
    state
        .seed_runtime("StakeInfoRuntimeApi", "get_coldkey_lock", lock(77, 12, 100))
        .unwrap();
    state
        .seed_runtime(
            "StakeInfoRuntimeApi",
            "get_hotkey_conviction",
            json!({"bits":(42u128 << 64) | u128::from(u64::MAX)}),
        )
        .unwrap();
    // Uids is OptionalStorage: force a genuine miss rather than fake extracted defaults.
    let info = state
        .runtime
        .storage_entry("SubtensorModule", "Uids")
        .unwrap();
    let key = state
        .runtime
        .storage_key(
            info,
            &[
                rtao_core::codec::Value::Uint(1),
                rtao_core::codec::Value::Str(OTHER.to_string()),
            ],
        )
        .unwrap();
    state.seed_encoded(key, None);
    state
}

#[tokio::test]
async fn nine_reads_observe_the_exact_reviewed_descriptors_at_head_and_pinned_blocks() {
    let (client, chain) = client(seed()).await;
    let samples = [
        (
            ReadName::AcceptsLockedAlpha,
            json!({"coldkey_ss58":COLD}),
            vec![(ReadKind::Storage, "SubtensorModule", "AccountFlags")],
        ),
        (
            ReadName::ColdkeyLock,
            json!({"coldkey_ss58":COLD,"netuid":1}),
            vec![
                (ReadKind::Storage, "SubtensorModule", "Lock"),
                (ReadKind::Storage, "SubtensorModule", "DecayingLock"),
                (
                    ReadKind::RuntimeApi,
                    "StakeInfoRuntimeApi",
                    "get_coldkey_lock",
                ),
            ],
        ),
        (
            ReadName::LocksForColdkey,
            json!({"coldkey_ss58":COLD}),
            vec![
                (ReadKind::Storage, "SubtensorModule", "Lock"),
                (ReadKind::Storage, "SubtensorModule", "DecayingLock"),
                (
                    ReadKind::RuntimeApi,
                    "StakeInfoRuntimeApi",
                    "get_coldkey_lock",
                ),
            ],
        ),
        (
            ReadName::LocksForHotkey,
            json!({"hotkey_ss58":HOT,"netuid":1}),
            vec![
                (ReadKind::Storage, "SubtensorModule", "LockingColdkeys"),
                (ReadKind::Storage, "SubtensorModule", "DecayingLock"),
                (
                    ReadKind::RuntimeApi,
                    "StakeInfoRuntimeApi",
                    "get_coldkey_lock",
                ),
            ],
        ),
        (
            ReadName::HotkeyConviction,
            json!({"hotkey_ss58":HOT,"netuid":1}),
            vec![(
                ReadKind::RuntimeApi,
                "StakeInfoRuntimeApi",
                "get_hotkey_conviction",
            )],
        ),
        (
            ReadName::SubnetConvictions,
            json!({"netuid":1}),
            vec![
                (ReadKind::Storage, "SubtensorModule", "HotkeyLock"),
                (ReadKind::Storage, "SubtensorModule", "DecayingHotkeyLock"),
                (ReadKind::Storage, "SubtensorModule", "OwnerLock"),
                (ReadKind::Storage, "SubtensorModule", "DecayingOwnerLock"),
                (ReadKind::Storage, "SubtensorModule", "SubnetOwnerHotkey"),
                (ReadKind::Storage, "SubtensorModule", "SubnetAlphaOut"),
                (ReadKind::Storage, "SubtensorModule", "SubnetProtocolAlpha"),
                (ReadKind::Storage, "AlphaAssets", "AlphaBurned"),
                (ReadKind::Storage, "SubtensorModule", "UnlockRate"),
                (ReadKind::Storage, "SubtensorModule", "MaturityRate"),
                (ReadKind::Storage, "SubtensorModule", "NetworkRegisteredAt"),
            ],
        ),
        (
            ReadName::MinerCollateral,
            json!({"netuid":1,"hotkey_ss58":HOT}),
            vec![
                (ReadKind::Storage, "SubtensorModule", "Owner"),
                (ReadKind::Storage, "SubtensorModule", "MinerCollateral"),
            ],
        ),
        (
            ReadName::SubnetCollateral,
            json!({"netuid":1}),
            vec![
                (ReadKind::Storage, "SubtensorModule", "MinerCollateral"),
                (ReadKind::Storage, "SubtensorModule", "Uids"),
            ],
        ),
        (
            ReadName::CollateralPolicy,
            json!({"netuid":1}),
            vec![
                (ReadKind::Storage, "SubtensorModule", "CollateralLockShare"),
                (ReadKind::Storage, "SubtensorModule", "CollateralDrainRatio"),
            ],
        ),
    ];
    for (name, args, expected) in samples {
        let expected: BTreeSet<_> = expected
            .into_iter()
            .map(|(kind, pallet, item)| (kind, pallet.to_owned(), item.to_owned()))
            .collect();
        let declared: BTreeSet<_> = name
            .spec()
            .hits
            .iter()
            .map(|hit| {
                let kind = match hit.kind {
                    HitKind::Storage => ReadKind::Storage,
                    HitKind::RuntimeApi => ReadKind::RuntimeApi,
                    HitKind::Constant => ReadKind::Constant,
                };
                (kind, hit.container.to_owned(), hit.name.to_owned())
            })
            .collect();
        assert_eq!(declared, expected, "{name}");
        for block in [None, Some(99), Some(100)] {
            let before = chain.requests().len();
            let (touches, hashes) = if let Some(block) = block {
                let snapshot = client.at(BlockRef::Number(block)).await.unwrap();
                let view = RecordingView::new(snapshot);
                dispatch(&view, name, args.as_object().unwrap())
                    .await
                    .unwrap();
                (view.touches(), view.block_hashes())
            } else {
                let view = RecordingView::new(client.clone());
                dispatch(&view, name, args.as_object().unwrap())
                    .await
                    .unwrap();
                (view.touches(), view.block_hashes())
            };
            assert_eq!(hashes.len(), 1, "{name}");
            let hash = *hashes.first().unwrap();
            let observed: BTreeSet<_> = touches
                .into_iter()
                .map(|touch| {
                    assert_eq!(touch.at, Some(hash), "{name}");
                    (touch.kind, touch.pallet_or_api, touch.item)
                })
                .collect();
            assert_eq!(observed, expected, "{name}");
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
                        "{name}: {request:?}"
                    );
                }
            }
        }
    }
    client.close().await;
}

#[tokio::test]
async fn lock_records_use_runtime_roll_forward_and_distinguish_absent_decay_mode() {
    let (client, chain) = client(seed()).await;
    let record = locks::coldkey_lock(&client, COLD, NetUid(1))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.hotkey, HOT);
    assert_eq!(record.locked_alpha.rao, 77); // Stored mass is 1: runtime API supplies the rolled mass.
    assert_eq!(record.locked_alpha.symbol.as_ref().unwrap().as_str(), "Ω");
    assert!(record.is_perpetual);
    let records = locks::locks_for_coldkey(&client, COLD).await.unwrap();
    assert_eq!(
        records.iter().map(|r| r.netuid).collect::<Vec<_>>(),
        [NetUid(1), NetUid(2)]
    );
    assert!(!records[1].is_perpetual);
    let records = locks::locks_for_hotkey(&client, HOT, NetUid(1))
        .await
        .unwrap();
    let mut expected = [COLD, OTHER];
    expected.sort_by_key(ToString::to_string);
    assert_eq!(
        records.iter().map(|r| r.coldkey).collect::<Vec<_>>(),
        expected
    );
    let other = records.iter().find(|r| r.coldkey == OTHER).unwrap();
    assert!(!other.lock.is_perpetual); // No DecayingLock entry.
    assert_eq!(other.lock.hotkey, HOT);
    assert!(
        locks::coldkey_lock(&client, OTHER, NetUid(1))
            .await
            .unwrap()
            .is_none()
    );
    chain.mutate_state(|state| {
        state
            .seed_runtime("StakeInfoRuntimeApi", "get_coldkey_lock", json!(null))
            .unwrap()
    });
    assert!(
        locks::locks_for_coldkey(&client, COLD)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        locks::locks_for_hotkey(&client, HOT, NetUid(1))
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        locks::coldkey_lock(&client, COLD, NetUid(1))
            .await
            .unwrap()
            .is_none()
    );
    client.close().await;
}

#[tokio::test]
async fn acceptance_uses_only_bit_zero_and_runtime_conviction_discards_fraction_without_float() {
    let (client, chain) = client(seed()).await;
    for flags in [0, 1, 2, u128::MAX - 1, u128::MAX] {
        chain.mutate_state(|state| {
            state
                .seed_python(
                    "SubtensorModule",
                    "AccountFlags",
                    &[json!(COLD)],
                    json!(flags),
                )
                .unwrap()
        });
        assert_eq!(
            locks::accepts_locked_alpha(&client, COLD).await.unwrap(),
            flags & 1 == 1
        );
    }
    for (bits, expected) in [
        (0, 0),
        (u128::from(u64::MAX), 0),
        (1u128 << 64, 1),
        (u128::MAX, u64::MAX),
    ] {
        chain.mutate_state(|state| {
            state
                .seed_runtime(
                    "StakeInfoRuntimeApi",
                    "get_hotkey_conviction",
                    json!({"bits":bits}),
                )
                .unwrap()
        });
        let record = locks::hotkey_conviction(&client, HOT, NetUid(1))
            .await
            .unwrap();
        assert_eq!(record.conviction_alpha.rao, expected);
        assert_eq!(record.conviction_alpha.netuid, NetUid(1));
        assert_eq!(
            record.conviction_alpha.symbol.as_ref().unwrap().as_str(),
            "Ω"
        );
    }
    client.close().await;
}

#[tokio::test]
async fn subnet_accounting_gates_each_hotkey_instead_of_the_global_conviction_sum() {
    let (client, chain) = client(seed()).await;
    let record = locks::subnet_convictions(&client, NetUid(1)).await.unwrap();
    assert_eq!(
        (
            record.alpha_out.rao,
            record.protocol_alpha.rao,
            record.alpha_burned.rao,
            record.eligible_alpha.rao
        ),
        (12000, 1000, 1000, 10000)
    );
    assert_eq!(record.threshold_alpha.rao, 1800);
    assert_eq!(
        (
            record.total_locked_alpha.rao,
            record.total_conviction_alpha.rao
        ),
        (3500, 3500)
    );
    assert_eq!(
        (record.leader_hotkey, record.leader_blocks_to_threshold),
        (Some(OWNER), None)
    );
    assert_eq!(record.hotkeys[0].locked_alpha.rao, 1500);
    assert_eq!(record.hotkeys[0].conviction_alpha.rao, 1500); // Owner ignores stale conviction of 1.
    assert!(record.hotkeys[0].is_owner);
    assert!(
        record
            .hotkeys
            .iter()
            .all(|r| r.blocks_to_threshold.is_none())
    );
    assert_eq!(
        (record.registered_at, record.ownership_changeable_at_block),
        (81, 81 + 2_629_800)
    );
    chain.mutate_state(|state| {
        state
            .seed_python(
                "SubtensorModule",
                "SubnetProtocolAlpha",
                &[json!(1)],
                json!(11900),
            )
            .unwrap();
        state
            .seed_python(
                "SubtensorModule",
                "NetworkRegisteredAt",
                &[json!(1)],
                json!(u64::MAX),
            )
            .unwrap();
        state
            .seed_python("SubtensorModule", "UnlockRate", &[], json!(0))
            .unwrap();
        state
            .seed_python("SubtensorModule", "MaturityRate", &[], json!(0))
            .unwrap();
    });
    let zero = locks::subnet_convictions(&client, NetUid(1)).await.unwrap();
    assert_eq!((zero.eligible_alpha.rao, zero.threshold_alpha.rao), (0, 0));
    assert!(
        zero.hotkeys
            .iter()
            .all(|r| r.pct_of_threshold.is_none() && r.blocks_to_threshold.is_none())
    );
    assert_eq!((zero.unlock_rate, zero.maturity_rate), (934_866, 934_866));
    assert_eq!(
        zero.ownership_changeable_at_block,
        u128::from(u64::MAX) + 2_629_800
    );
    client.close().await;
}

#[tokio::test]
async fn collateral_positions_keep_nominators_separate_and_survive_deregistration() {
    let (client, _) = client(seed()).await;
    let owner = locks::miner_collateral(&client, NetUid(1), HOT, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(owner.coldkey, COLD);
    assert_eq!(
        (
            owner.locked_alpha.rao,
            owner.min_locked_alpha.rao,
            owner.earned_alpha.rao
        ),
        (300, 100, 17)
    );
    assert_eq!(
        (
            owner.headroom_alpha.rao,
            owner.shortfall_alpha.rao,
            owner.releasable_work_alpha.rao,
            owner.drain_ratio
        ),
        (200, 0, 100, 2.0)
    );
    assert_eq!(owner.locked_alpha.symbol.as_ref().unwrap().as_str(), "Ω");
    let explicit = RecordingView::new(client.clone());
    let nominator = locks::miner_collateral(&explicit, NetUid(1), HOT, Some(OTHER))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(nominator.coldkey, OTHER);
    assert_eq!(nominator.earned_alpha.rao, u64::MAX);
    assert_eq!(
        (
            nominator.headroom_alpha.rao,
            nominator.shortfall_alpha.rao,
            nominator.releasable_work_alpha.rao,
            nominator.drain_ratio
        ),
        (0, 100, 0, 0.0)
    );
    assert!(
        explicit
            .touches()
            .iter()
            .all(|t| t.item == "MinerCollateral")
    );
    let records = locks::subnet_collateral(&client, NetUid(1)).await.unwrap();
    assert_eq!(
        records
            .iter()
            .map(|r| r.collateral.locked_alpha.rao)
            .collect::<Vec<_>>(),
        [450, 300, 150]
    );
    assert_eq!(records[0].collateral.hotkey, OTHER);
    assert_eq!(records[0].uid, None);
    assert_eq!(records[1].uid, Some(0));
    assert_eq!(records[2].uid, Some(0));
    assert_eq!(
        serde_json::to_value(&records[0]).unwrap()["uid"],
        json!(null)
    );
    assert!(
        locks::miner_collateral(&client, NetUid(2), HOT, Some(COLD))
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        locks::subnet_collateral(&client, NetUid(2))
            .await
            .unwrap()
            .is_empty()
    );
    let policy = locks::collateral_policy(&client, NetUid(1)).await.unwrap();
    assert_eq!(
        (policy.netuid, policy.lock_share, policy.drain_ratio),
        (NetUid(1), 32768.0 / 65535.0, 2.0)
    );
    client.close().await;
}

#[tokio::test]
async fn derived_collateral_work_rejects_native_overflow_and_preserves_zero_ratio() {
    let (client, chain) = client(seed()).await;
    chain.mutate_state(|state| state.seed_python("SubtensorModule", "MinerCollateral", &[json!(1),json!(HOT),json!(COLD)], json!({"locked":u64::MAX,"min_locked":0,"earned":u64::MAX,"drain_ratio":{"bits":1}})).unwrap());
    let error = locks::miner_collateral(&client, NetUid(1), HOT, Some(COLD))
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        rtao_reads::ReadError::SurfaceChanged {
            descriptor: "SubtensorModule.MinerCollateral",
            ..
        }
    ));
    chain.mutate_state(|state| state.seed_python("SubtensorModule", "MinerCollateral", &[json!(1),json!(HOT),json!(COLD)], json!({"locked":u64::MAX,"min_locked":0,"earned":u64::MAX,"drain_ratio":{"bits":0}})).unwrap());
    let record = locks::miner_collateral(&client, NetUid(1), HOT, Some(COLD))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.locked_alpha.rao, u64::MAX);
    assert_eq!(record.headroom_alpha.rao, u64::MAX);
    assert_eq!(record.releasable_work_alpha.rao, 0);
    client.close().await;
}

#[tokio::test]
async fn metadata_misses_decode_defaults_and_truncated_records_fail() {
    let mut state = FakeState::default();
    for item in ["CollateralLockShare", "CollateralDrainRatio"] {
        let info = state
            .runtime
            .storage_entry("SubtensorModule", item)
            .unwrap();
        let key = state
            .runtime
            .storage_key(info, &[rtao_core::codec::Value::Uint(1)])
            .unwrap();
        state.seed_encoded(key, None);
    }
    let info = state
        .runtime
        .storage_entry("SubtensorModule", "MinerCollateral")
        .unwrap();
    let key = state
        .runtime
        .storage_key(
            info,
            &[
                rtao_core::codec::Value::Uint(1),
                rtao_core::codec::Value::Str(HOT.to_string()),
                rtao_core::codec::Value::Str(COLD.to_string()),
            ],
        )
        .unwrap();
    state.seed_encoded(key, Some(vec![1]));
    let (client, _) = client(state).await;
    let policy = locks::collateral_policy(&client, NetUid(1)).await.unwrap();
    assert_eq!((policy.lock_share, policy.drain_ratio), (0.0, 1.0));
    assert!(
        locks::miner_collateral(&client, NetUid(1), HOT, Some(COLD))
            .await
            .is_err()
    );
    client.close().await;
}

#[tokio::test]
async fn perpetual_owner_projection_keeps_maximum_mass_and_refuses_aggregate_overflow() {
    let mut state = FakeState::default();
    for (item, value) in [
        ("SubnetOwnerHotkey", json!(HOT)),
        ("OwnerLock", lock(u64::MAX, 0, 100)),
        ("SubnetAlphaOut", json!(u64::MAX)),
        ("SubnetProtocolAlpha", json!(0)),
    ] {
        state
            .seed_python("SubtensorModule", item, &[json!(1)], value)
            .unwrap();
    }
    state
        .seed_python("AlphaAssets", "AlphaBurned", &[json!(1)], json!(0))
        .unwrap();
    let (client, chain) = client(state).await;
    let result = locks::subnet_convictions(&client, NetUid(1)).await.unwrap();
    assert_eq!(result.hotkeys.len(), 1);
    assert_eq!(result.hotkeys[0].locked_alpha.rao, u64::MAX);
    assert_eq!(result.hotkeys[0].conviction_alpha.rao, u64::MAX);
    assert_eq!(result.total_locked_alpha.rao, u64::MAX);
    assert_eq!(result.total_conviction_alpha.rao, u64::MAX);
    assert_eq!(result.leader_blocks_to_threshold, Some(0));
    chain.mutate_state(|state| {
        state
            .seed_python(
                "SubtensorModule",
                "HotkeyLock",
                &[json!(1), json!(OTHER)],
                lock(1, 0, 100),
            )
            .unwrap();
    });
    assert!(matches!(
        locks::subnet_convictions(&client, NetUid(1))
            .await
            .unwrap_err(),
        rtao_reads::ReadError::SurfaceChanged {
            descriptor: "subnet_convictions",
            ..
        }
    ));
    client.close().await;
}

#[tokio::test]
async fn bucket_sums_compensate_globally_and_per_hotkey_without_reusing_rounded_totals() {
    let large = 1u64 << 53;
    let mut state = FakeState::default();
    for (item, keys, value) in [
        ("SubnetOwnerHotkey", vec![json!(1)], json!(HOT)),
        ("SubnetAlphaOut", vec![json!(1)], json!(0)),
        ("SubnetProtocolAlpha", vec![json!(1)], json!(0)),
        (
            "HotkeyLock",
            vec![json!(1), json!(HOT)],
            lock(large, large, 100),
        ),
        ("HotkeyLock", vec![json!(1), json!(OTHER)], lock(1, 1, 100)),
        (
            "DecayingHotkeyLock",
            vec![json!(1), json!(HOT)],
            lock(1, 1, 100),
        ),
    ] {
        state
            .seed_python("SubtensorModule", item, &keys, value)
            .unwrap();
    }
    state
        .seed_python("AlphaAssets", "AlphaBurned", &[json!(1)], json!(0))
        .unwrap();
    let (client, chain) = client(state).await;
    let global = locks::subnet_convictions(&client, NetUid(1)).await.unwrap();
    let hotkey = global
        .hotkeys
        .iter()
        .find(|entry| entry.hotkey == HOT)
        .unwrap();
    // This per-hotkey sum rounds 2^53+1 to 2^53, but the separate global
    // sum consumes all three floats and recovers 2^53+2.
    assert_eq!(hotkey.conviction_alpha.rao, large);
    assert_eq!(global.total_conviction_alpha.rao, large + 2);
    // Locked mass has an integer prefix and an integer item after the first
    // float; the pinned mixed-type sum deliberately differs from conviction.
    assert_eq!(global.total_locked_alpha.rao, large);
    chain.mutate_state(|state| {
        state
            .seed_python(
                "SubtensorModule",
                "DecayingOwnerLock",
                &[json!(1)],
                lock(1, 0, 100),
            )
            .unwrap();
    });
    let per_hotkey = locks::subnet_convictions(&client, NetUid(1)).await.unwrap();
    let hotkey = per_hotkey
        .hotkeys
        .iter()
        .find(|entry| entry.hotkey == HOT)
        .unwrap();
    // Three float buckets for HOT now independently recover the two small rao.
    assert_eq!(hotkey.conviction_alpha.rao, large + 2);
    assert_eq!(hotkey.locked_alpha.rao, large);
    assert_eq!(per_hotkey.total_conviction_alpha.rao, large + 4);
    client.close().await;
}

#[tokio::test]
async fn elapsed_buckets_roll_mass_and_conviction_forward_and_future_updates_clamp_to_now() {
    let mut state = FakeState::default();
    for (item, keys, value) in [
        ("HotkeyLock", vec![json!(1), json!(HOT)], lock(3600, 0, 0)),
        (
            "DecayingHotkeyLock",
            vec![json!(1), json!(OTHER)],
            lock(1000, 100, 0),
        ),
        ("OwnerLock", vec![json!(1)], lock(1000, 0, 0)),
        ("DecayingOwnerLock", vec![json!(1)], lock(500, 0, 0)),
        ("SubnetOwnerHotkey", vec![json!(1)], json!(OWNER)),
        ("SubnetAlphaOut", vec![json!(1)], json!(12000)),
        ("SubnetProtocolAlpha", vec![json!(1)], json!(1000)),
        ("UnlockRate", vec![], json!(100)),
        ("MaturityRate", vec![], json!(100)),
    ] {
        state
            .seed_python("SubtensorModule", item, &keys, value)
            .unwrap();
    }
    state
        .seed_python("AlphaAssets", "AlphaBurned", &[json!(1)], json!(1000))
        .unwrap();
    let (client, chain) = client(state).await;
    let result = locks::subnet_convictions(&client, NetUid(1)).await.unwrap();
    assert_eq!(result.block, 100);
    let hotkey = result
        .hotkeys
        .iter()
        .find(|entry| entry.hotkey == HOT)
        .unwrap();
    assert_eq!(
        (hotkey.locked_alpha.rao, hotkey.conviction_alpha.rao),
        (3600, 2275)
    );
    assert!((hotkey.pct_of_threshold.unwrap() - 2.0 * (1.0 - (-1.0f64).exp())).abs() < 1e-12);
    assert_eq!(hotkey.blocks_to_threshold, Some(0));
    let decaying = result
        .hotkeys
        .iter()
        .find(|entry| entry.hotkey == OTHER)
        .unwrap();
    assert_eq!(
        (decaying.locked_alpha.rao, decaying.conviction_alpha.rao),
        (367, 404)
    );
    let owner = result
        .hotkeys
        .iter()
        .find(|entry| entry.hotkey == OWNER)
        .unwrap();
    assert_eq!(
        (owner.locked_alpha.rao, owner.conviction_alpha.rao),
        (1183, 1183)
    );
    assert_eq!(
        (
            result.total_locked_alpha.rao,
            result.total_conviction_alpha.rao
        ),
        (5151, 3864)
    );
    chain.mutate_state(|state| {
        state
            .seed_python(
                "SubtensorModule",
                "HotkeyLock",
                &[json!(1), json!(HOT)],
                lock(3600, 0, 200),
            )
            .unwrap();
    });
    let future = locks::subnet_convictions(&client, NetUid(1)).await.unwrap();
    let hotkey = future
        .hotkeys
        .iter()
        .find(|entry| entry.hotkey == HOT)
        .unwrap();
    assert_eq!(
        (hotkey.locked_alpha.rao, hotkey.conviction_alpha.rao),
        (3600, 0)
    );
    assert_eq!(hotkey.blocks_to_threshold, Some(70)); // First integer block above 100 ln 2.
    client.close().await;
}

#[tokio::test]
async fn metadata_default_owner_is_the_raw_account_and_keeps_present_owner_buckets() {
    let mut state = FakeState::default();
    for (item, value) in [
        ("OwnerLock", lock(123, 1, 100)),
        ("SubnetAlphaOut", json!(1000)),
        ("SubnetProtocolAlpha", json!(0)),
    ] {
        state
            .seed_python("SubtensorModule", item, &[json!(1)], value)
            .unwrap();
    }
    state
        .seed_python("AlphaAssets", "AlphaBurned", &[json!(1)], json!(0))
        .unwrap();
    let info = state
        .runtime
        .storage_entry("SubtensorModule", "SubnetOwnerHotkey")
        .unwrap();
    let key = state
        .runtime
        .storage_key(info, &[rtao_core::codec::Value::Uint(1)])
        .unwrap();
    state.seed_encoded(key, None);
    let (client, _) = client(state).await;
    let result = locks::subnet_convictions(&client, NetUid(1)).await.unwrap();
    assert_eq!(result.owner_hotkey, Some(AccountId::ZERO));
    assert_eq!(result.hotkeys.len(), 1);
    assert_eq!(result.hotkeys[0].hotkey, AccountId::ZERO);
    assert!(result.hotkeys[0].is_owner);
    assert_eq!(
        (
            result.hotkeys[0].locked_alpha.rao,
            result.hotkeys[0].conviction_alpha.rao
        ),
        (123, 123)
    );
    assert_eq!(result.leader_blocks_to_threshold, None);
    client.close().await;
}
