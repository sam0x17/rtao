//! Account, commitment and neuron behavior through live fixture metadata.
#![allow(clippy::unwrap_used, clippy::panic)] // Invalid fixtures must fail the assertions.

use std::collections::BTreeSet;

use rtao_client::{BlockRef, Client, ClientConfig, Network, ReadKind, View};
use rtao_primitives::{AccountId, NetUid, Uid};
use rtao_reads::{ReadError, ReadName, reads::m6_accounts as reads};
use rtao_testkit::{FakeChain, FakeState, RecordingView, rpc::RpcClient};
use serde_json::{Value, json};

const COLD: AccountId = AccountId([7; 32]);
const HOT: AccountId = AccountId([8; 32]);
const OTHER: AccountId = AccountId([9; 32]);
const NET: NetUid = NetUid(42);
const CALL_HASH: &str = "abababababababababababababababababababababababababababababababab";

async fn setup_client(state: FakeState) -> (Client, FakeChain) {
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

fn neuron(uid: u16, hotkey: AccountId, stake: Value) -> Value {
    json!({
        "uid": uid, "netuid": NET, "hotkey": hotkey, "coldkey": COLD,
        "active": true, "validator_permit": uid == 7, "last_update": 91,
        "stake": stake, "weights": [[3, 65535]], "bonds": [[4, 32768]],
    })
}

fn seed() -> FakeState {
    let mut state = FakeState::default();
    for (item, keys, value) in [
        (
            "ColdkeySwapAnnouncements",
            vec![json!(COLD)],
            json!([123, format!("0x{}", "ab".repeat(32))]),
        ),
        ("ColdkeySwapDisputes", vec![json!(COLD)], json!(99)),
        ("TxRateLimit", vec![], json!(u64::MAX)),
        ("Owner", vec![json!(HOT)], json!(COLD)),
        (
            "OwnedHotkeys",
            vec![json!(COLD)],
            json!([OTHER, HOT, OTHER]),
        ),
        ("IsNetworkMember", vec![json!(HOT), json!(300)], json!(true)),
        ("IsNetworkMember", vec![json!(HOT), json!(2)], json!(true)),
        ("IsNetworkMember", vec![json!(HOT), json!(1)], json!(false)),
        ("IsNetworkMember", vec![json!(OTHER), json!(7)], json!(true)),
        (
            "AssociatedEvmAddress",
            vec![json!(NET), json!(7)],
            json!([format!("0x{}", "12".repeat(20)), 88]),
        ),
        ("Uids", vec![json!(NET), json!(HOT)], json!(7)),
        ("TokenSymbol", vec![json!(NET)], json!("Ω")),
    ] {
        state
            .seed_python("SubtensorModule", item, &keys, value)
            .unwrap();
    }
    state
        .seed_python(
            "Multisig",
            "Multisigs",
            &[json!(COLD), json!(format!("0x{CALL_HASH}"))],
            json!({"when": {"height": 81, "index": 3}, "deposit": u64::MAX,
                "depositor": OTHER, "approvals": [HOT, OTHER]}),
        )
        .unwrap();
    state
        .seed_constant("Balances", "ExistentialDeposit", json!(u64::MAX))
        .unwrap();
    state
        .seed_constant("Aura", "SlotDuration", json!(250))
        .unwrap();
    state
        .seed_python("Timestamp", "Now", &[], json!(1700000000123u64))
        .unwrap();
    state
        .seed_python(
            "Commitments",
            "CommitmentOf",
            &[json!(NET), json!(HOT)],
            json!({"block": 80, "deposit": u64::MAX, "info": {"fields": [
                {"Raw2": "0xcf84"}, {"Sha256": format!("0x{}", "ab".repeat(32))},
                "ResetBondsFlag", {"Raw1": "0x21"},
            ]}}),
        )
        .unwrap();
    state
        .seed_python(
            "Commitments",
            "RevealedCommitments",
            &[json!(NET), json!(OTHER)],
            json!([["0x1064617461", 90], ["0x0c6f6c64", 70]]),
        )
        .unwrap();
    for method in ["get_neurons_lite", "get_neurons"] {
        let mut records = vec![
            neuron(7, HOT, json!([[COLD, u64::MAX - 1], [OTHER, 1]])),
            neuron(2, OTHER, json!([])),
        ];
        if method == "get_neurons_lite" {
            for record in &mut records {
                record.as_object_mut().unwrap().remove("weights");
                record.as_object_mut().unwrap().remove("bonds");
            }
        }
        state
            .seed_runtime("NeuronInfoRuntimeApi", method, json!(records))
            .unwrap();
    }
    state
}

async fn call<V: View + ?Sized>(view: &V, name: ReadName, block: i64) -> Result<Value, ReadError> {
    let value = match name {
        ReadName::ColdkeySwapAnnouncement => {
            json!(reads::coldkey_swap_announcement(view, COLD).await?)
        }
        ReadName::ExistentialDeposit => json!(reads::existential_deposit(view).await?),
        ReadName::Multisig => json!(reads::multisig(view, COLD, CALL_HASH.into()).await?),
        ReadName::BlockInfo => json!(reads::block_info(view, block).await?),
        ReadName::IsFastBlocks => json!(reads::is_fast_blocks(view).await?),
        ReadName::TxRateLimit => json!(reads::tx_rate_limit(view).await?),
        ReadName::Commitment => json!(reads::commitment(view, NET, HOT).await?),
        ReadName::Commitments => json!(reads::commitments(view, NET).await?),
        ReadName::RevealedCommitment => {
            json!(reads::revealed_commitment(view, NET, OTHER).await?)
        }
        ReadName::AssociatedEvmKey => json!(reads::associated_evm_key(view, NET, Uid(7)).await?),
        ReadName::HotkeyOwner => json!(reads::hotkey_owner(view, HOT).await?),
        ReadName::NetuidsForHotkey => json!(reads::netuids_for_hotkey(view, HOT).await?),
        ReadName::Neurons => {
            let lite = reads::neurons(view, NET, true).await?;
            let full = reads::neurons(view, NET, false).await?;
            assert_eq!(lite[1].uid, full[1].uid);
            assert_eq!(lite[1].total_stake, full[1].total_stake);
            json!(full)
        }
        ReadName::OwnedHotkeys => json!(reads::owned_hotkeys(view, COLD).await?),
        _ => panic!("unexpected fixture operation: {name}"),
    };
    Ok(value)
}

fn expected(name: ReadName) -> BTreeSet<(ReadKind, String, String)> {
    use ReadKind::{Constant, RuntimeApi, Storage};
    let hits = match name {
        ReadName::ColdkeySwapAnnouncement => vec![
            (Storage, "SubtensorModule", "ColdkeySwapAnnouncements"),
            (Storage, "SubtensorModule", "ColdkeySwapDisputes"),
        ],
        ReadName::ExistentialDeposit => vec![(Constant, "Balances", "ExistentialDeposit")],
        ReadName::Multisig => vec![(Storage, "Multisig", "Multisigs")],
        ReadName::BlockInfo => vec![(Storage, "Timestamp", "Now")],
        ReadName::IsFastBlocks => vec![(Constant, "Aura", "SlotDuration")],
        ReadName::TxRateLimit => vec![(Storage, "SubtensorModule", "TxRateLimit")],
        ReadName::Commitment => vec![(Storage, "Commitments", "CommitmentOf")],
        ReadName::Commitments => vec![
            (Storage, "Commitments", "CommitmentOf"),
            (Storage, "Commitments", "RevealedCommitments"),
            (Storage, "SubtensorModule", "Uids"),
        ],
        ReadName::RevealedCommitment => vec![(Storage, "Commitments", "RevealedCommitments")],
        ReadName::AssociatedEvmKey => vec![(Storage, "SubtensorModule", "AssociatedEvmAddress")],
        ReadName::HotkeyOwner => vec![(Storage, "SubtensorModule", "Owner")],
        ReadName::NetuidsForHotkey => vec![(Storage, "SubtensorModule", "IsNetworkMember")],
        ReadName::Neurons => vec![
            (RuntimeApi, "NeuronInfoRuntimeApi", "get_neurons_lite"),
            (RuntimeApi, "NeuronInfoRuntimeApi", "get_neurons"),
        ],
        ReadName::OwnedHotkeys => vec![(Storage, "SubtensorModule", "OwnedHotkeys")],
        _ => panic!("unexpected fixture operation: {name}"),
    };
    hits.into_iter()
        .map(|(kind, pallet, item)| (kind, pallet.into(), item.into()))
        .collect()
}

fn assert_observations<V>(
    view: &RecordingView<V>,
    chain: &FakeChain,
    name: ReadName,
    hash: rtao_primitives::H256,
    before: usize,
) {
    let observed = view
        .touches()
        .into_iter()
        .map(|touch| {
            assert_eq!(touch.at, Some(hash), "{name}: {touch:?}");
            assert!(
                name.spec().hits.iter().any(|hit| hit.matches(&touch)),
                "{name}: descriptor is absent from the generated contract: {touch:?}"
            );
            (touch.kind, touch.pallet_or_api, touch.item)
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(observed, expected(name), "{name}: exact descriptor set");
    assert_eq!(
        observed.len(),
        name.spec().hits.len(),
        "{name}: full generated contract"
    );
    assert_eq!(view.block_hashes(), BTreeSet::from([hash]), "{name}");
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

#[tokio::test]
async fn fourteen_reads_cover_exact_descriptors_at_head_and_pinned_history() {
    let (client, chain) = setup_client(seed()).await;
    let names = [
        ReadName::ColdkeySwapAnnouncement,
        ReadName::ExistentialDeposit,
        ReadName::Multisig,
        ReadName::BlockInfo,
        ReadName::IsFastBlocks,
        ReadName::TxRateLimit,
        ReadName::Commitment,
        ReadName::Commitments,
        ReadName::RevealedCommitment,
        ReadName::AssociatedEvmKey,
        ReadName::HotkeyOwner,
        ReadName::NetuidsForHotkey,
        ReadName::Neurons,
        ReadName::OwnedHotkeys,
    ];
    let head = client.at(BlockRef::Best).await.unwrap().block().clone();
    for name in names {
        let view = RecordingView::new(client.clone());
        let before = chain.requests().len();
        call(&view, name, head.number as i64).await.unwrap();
        assert_observations(&view, &chain, name, head.hash, before);
        let snapshot = client.at(BlockRef::Number(99)).await.unwrap();
        let hash = snapshot.block().hash;
        let view = RecordingView::new(snapshot);
        let before = chain.requests().len();
        call(&view, name, 99).await.unwrap();
        assert_observations(&view, &chain, name, hash, before);
    }
    client.close().await;
}

#[tokio::test]
async fn swap_dispute_truthiness_and_absence_match_python() {
    for dispute in [None, Some(0), Some(99)] {
        let mut state = seed();
        state
            .seed_python(
                "SubtensorModule",
                "ColdkeySwapDisputes",
                &[json!(COLD)],
                json!(dispute),
            )
            .unwrap();
        let (client, _) = setup_client(state).await;
        let value = reads::coldkey_swap_announcement(&client, COLD)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(value.execute_block, 123);
        assert_eq!(value.new_coldkey_hash, format!("0x{CALL_HASH}"));
        assert_eq!(value.disputed, dispute.is_some_and(|block| block != 0));
        assert_eq!(value.dispute_block, dispute.filter(|block| *block != 0));
        assert!(
            reads::coldkey_swap_announcement(&client, OTHER)
                .await
                .unwrap()
                .is_none()
        );
        client.close().await;
    }
}

#[tokio::test]
async fn native_deposits_and_rate_limits_keep_all_u64_bits() {
    let (client, _) = setup_client(seed()).await;
    assert_eq!(
        reads::existential_deposit(&client).await.unwrap().rao,
        u64::MAX
    );
    assert_eq!(reads::tx_rate_limit(&client).await.unwrap(), u64::MAX);
    let bare = reads::multisig(&client, COLD, CALL_HASH.into())
        .await
        .unwrap()
        .unwrap();
    let prefixed = reads::multisig(&client, COLD, format!("0x{CALL_HASH}"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(bare, prefixed);
    assert_eq!(bare.deposit.rao, u64::MAX);
    assert_eq!(bare.timepoint.height, 81);
    assert_eq!(bare.timepoint.index, 3);
    assert_eq!(bare.depositor, OTHER);
    assert_eq!(bare.approvals, [HOT, OTHER]);
    assert!(
        reads::multisig(&client, OTHER, CALL_HASH.into())
            .await
            .unwrap()
            .is_none()
    );
    for bad in ["0x", "zz", "0Xabab", "ab", &"aa".repeat(33)] {
        assert!(matches!(reads::multisig(&client, COLD, bad.into()).await,
            Err(ReadError::InvalidParam { name, .. }) if name == "call_hash"));
    }
    client.close().await;
}

#[tokio::test]
async fn commitment_read_raw_fields_differ_from_metagraph_hash_rendering() {
    let (client, _) = setup_client(seed()).await;
    let record = reads::commitment(&client, NET, HOT).await.unwrap().unwrap();
    assert_eq!(record.block, 80);
    assert_eq!(record.deposit.rao, u64::MAX);
    assert_eq!(record.data, "τ!");
    assert_eq!(record.fields[0], json!({"Raw2": "0xcf84"}));
    assert_eq!(record.fields[2], json!("ResetBondsFlag"));
    assert!(
        reads::commitment(&client, NET, OTHER)
            .await
            .unwrap()
            .is_none()
    );
    let revealed = reads::revealed_commitment(&client, NET, OTHER)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        revealed,
        [("\u{10}data".into(), 90), ("\u{c}old".into(), 70)]
    );
    assert!(
        reads::revealed_commitment(&client, NET, HOT)
            .await
            .unwrap()
            .is_none()
    );
    let rows = reads::commitments(&client, NET).await.unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["hotkey"], json!(OTHER));
    assert_eq!(rows[0]["commitment"], "data");
    assert_eq!(rows[0]["block"], 90);
    assert_eq!(rows[0]["uid"], Value::Null);
    assert_eq!(rows[0]["status"], "revealed");
    assert_eq!(rows[1]["uid"], 7);
    assert!(
        rows[1]["commitment"]
            .as_str()
            .unwrap()
            .contains("sha256:0x")
    );
    client.close().await;
}

#[tokio::test]
async fn raw_commitment_non_utf8_is_hex_and_hash_only_is_empty() {
    for (fields, content) in [
        (json!([{"Raw1": "0xff"}, {"Raw1": "0xfe"}]), "0xfffe"),
        (json!([{"Sha256": format!("0x{}", "ab".repeat(32))}]), ""),
        (json!([{"Raw0": []}, "None", "ResetBondsFlag"]), ""),
    ] {
        let mut state = seed();
        state
            .seed_python(
                "Commitments",
                "CommitmentOf",
                &[json!(NET), json!(HOT)],
                json!({"block": 0, "deposit": 0, "info": {"fields": fields}}),
            )
            .unwrap();
        let (client, _) = setup_client(state).await;
        assert_eq!(
            reads::commitment(&client, NET, HOT)
                .await
                .unwrap()
                .unwrap()
                .data,
            content
        );
        client.close().await;
    }
}

#[tokio::test]
async fn revealed_history_retains_core_utf8_hex_and_empty_vectors() {
    let mut state = seed();
    state
        .seed_python(
            "Commitments",
            "RevealedCommitments",
            &[json!(NET), json!(OTHER)],
            json!([["0x1064617461", 90], ["0xfffe", 91], ["", 92]]),
        )
        .unwrap();
    let (client, _) = setup_client(state).await;
    assert_eq!(
        reads::revealed_commitment(&client, NET, OTHER)
            .await
            .unwrap()
            .unwrap(),
        [
            ("\u{10}data".into(), 90),
            ("0xfffe".into(), 91),
            (String::new(), 92)
        ]
    );
    client.close().await;
}

#[tokio::test]
async fn owners_memberships_and_evm_preserve_defaults_and_order() {
    let (client, _) = setup_client(seed()).await;
    assert_eq!(
        reads::hotkey_owner(&client, HOT).await.unwrap().get(),
        Some(&COLD)
    );
    let absent = reads::hotkey_owner(&client, OTHER).await.unwrap();
    assert_eq!(absent.get(), None);
    assert_eq!(
        serde_json::to_value(absent).unwrap(),
        json!(AccountId::ZERO)
    );
    assert_eq!(
        reads::owned_hotkeys(&client, COLD).await.unwrap(),
        [OTHER, HOT, OTHER]
    );
    assert!(
        reads::owned_hotkeys(&client, OTHER)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        reads::netuids_for_hotkey(&client, HOT).await.unwrap(),
        [NetUid(2), NetUid(300)]
    );
    assert!(
        reads::netuids_for_hotkey(&client, COLD)
            .await
            .unwrap()
            .is_empty()
    );
    let evm = reads::associated_evm_key(&client, NET, Uid(7))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(evm.evm_address, format!("0x{}", "12".repeat(20)));
    assert_eq!(evm.block, 88);
    assert!(
        reads::associated_evm_key(&client, NET, Uid(0))
            .await
            .unwrap()
            .is_none()
    );
    client.close().await;
}

#[tokio::test]
async fn neurons_sort_by_uid_retain_raw_modes_and_check_native_stake_sum() {
    let (client, _) = setup_client(seed()).await;
    for lite in [true, false] {
        let records = reads::neurons(&client, NET, lite).await.unwrap();
        assert_eq!(
            records.iter().map(|record| record.uid).collect::<Vec<_>>(),
            [Uid(2), Uid(7)]
        );
        assert_eq!(records[0].total_stake.rao, 0);
        assert_eq!(records[1].hotkey, HOT);
        assert_eq!(records[1].coldkey, COLD);
        assert!(records[1].active && records[1].validator_permit);
        assert_eq!(records[1].last_update, 91);
        assert_eq!(records[1].total_stake.rao, u64::MAX);
        assert_eq!(records[1].total_stake.netuid, NET);
        assert_eq!(
            records[1].total_stake.symbol.as_ref().unwrap().as_str(),
            "Ω"
        );
        assert_eq!(records[1].raw.get("weights").is_none(), lite);
        if !lite {
            assert_eq!(records[1].raw["weights"], json!([[3, 65535]]));
            assert_eq!(records[1].raw["bonds"], json!([[4, 32768]]));
        }
    }
    client.close().await;
    let mut state = seed();
    state
        .seed_runtime(
            "NeuronInfoRuntimeApi",
            "get_neurons_lite",
            json!([neuron(1, HOT, json!([[COLD, u64::MAX], [OTHER, 1]]))]),
        )
        .unwrap();
    let (client, _) = setup_client(state).await;
    assert!(matches!(reads::neurons(&client, NET, true).await,
        Err(ReadError::SurfaceChanged { detail, .. }) if detail.contains("total stake exceeds native u64")));
    client.close().await;
}

#[tokio::test]
async fn fast_blocks_requires_exact_slot_duration() {
    for (millis, expected) in [(250, true), (249, false), (251, false), (12000, false)] {
        let mut state = seed();
        state
            .seed_constant("Aura", "SlotDuration", json!(millis))
            .unwrap();
        let (client, _) = setup_client(state).await;
        assert_eq!(reads::is_fast_blocks(&client).await.unwrap(), expected);
        client.close().await;
    }
}

#[tokio::test]
async fn block_summary_fallback_uses_requested_block_and_unknown_block_is_absent() {
    let (client, chain) = setup_client(seed()).await;
    let view = RecordingView::new(client.at(BlockRef::Number(99)).await.unwrap());
    let before = chain.requests().len();
    let summary = reads::block_info(&view, 100).await.unwrap().unwrap();
    assert_eq!(summary.number, 100);
    assert_eq!(summary.timestamp, "2023-11-14T22:13:20.123000+00:00");
    assert!(summary.extrinsics.is_empty());
    assert_eq!(summary.explorer_url, None);
    let hash = client.at(BlockRef::Number(100)).await.unwrap().block().hash;
    assert_eq!(summary.hash, Some(format!("0x{}", hex::encode(hash.0))));
    assert_observations(&view, &chain, ReadName::BlockInfo, hash, before);
    assert!(reads::block_info(&client, 999999).await.unwrap().is_none());
    assert!(matches!(reads::block_info(&client, -1).await,
        Err(ReadError::InvalidParam { name, .. }) if name == "block"));
    client.close().await;
}
