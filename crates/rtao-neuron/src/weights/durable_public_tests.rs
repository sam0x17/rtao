//! Actual public Executor -> Client signing/author journal, then scheduler/reveal
//! recovery from the original file. Synthetic queue/events are explicit; no
//! deployed runtime decryption, independent finality or full validator is claimed.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use super::*;
use crate::{ChainClock, ClockGuard, Metrics, WeightPolicy};
use rtao_client::{BlockHeader, BlockRef, ShieldPeriod, journal::JournalControl};
use rtao_primitives::{CryptoType, H256, NetUidStorageIndex};
use rtao_signer::Signer;
use rtao_testkit::journal_shield::{Inner, JournalLab, LabKeys};
use rtao_tx::{Executor, TxError};
use serde_json::json;

#[tokio::test]
async fn public_weight_pair_restarts_into_paused_reveal_and_retains_actual_native_outcome()
-> anyhow::Result<()> {
    for success in [true, false] {
        let mut state = super::super::tests::fixture_state()?;
        state.set_finalized(state.block)?;
        let root =
            std::path::PathBuf::from(std::env::var_os("RTAO_TEST_TMPDIR").expect("task scratch"))
                .join(format!("public-weight-{}-{success}", std::process::id()));
        let mut lab = JournalLab::new(state, root.join("journal.json"), CryptoType::Ed25519)
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        let account = lab.signer.account();
        lab.node.mutate_state(|state| -> anyhow::Result<()> {
            state.seed_typed("SubtensorModule", "Uids", (NetUid(1), account), 0u16)?;
            Ok(())
        })?;
        let make = || {
            WeightScheduler::new(
                WeightPolicy::default(),
                Arc::new(ChainClock::new(1_000_000)),
                ClockGuard::default(),
            )
        };
        let mut scheduler = make()?
            .with_journal(
                lab.journal().unwrap().clone(),
                lab.client.genesis(),
                NetUid(1),
                account,
            )?
            .with_shield_protocol(lab.protocol.clone())
            .with_shielded_submission(ShieldPeriod::P4);
        assert!(
            make()?
                .with_journal(
                    lab.journal().unwrap().clone(),
                    lab.client.genesis(),
                    NetUid(1),
                    account
                )
                .is_err()
        );
        scheduler.enqueue(
            "public-weights",
            Some("evaluation:retained".into()),
            MechId(0),
            WeightsInput::Parallel {
                uids: vec![Uid(1)],
                weights: vec![1.0],
            },
        )?;
        let snapshot = lab.client.at(BlockRef::Best).await?;
        let preflight = WeightPreflight::fetch(&snapshot, NetUid(1), MechId(0), account).await?;
        let commit = PreparedWeightCommit {
            ciphertext: vec![4, 5, 6],
            reveal_round: 500,
        };
        let binding = scheduler
            .prepare_binding(
                MechId(0),
                (vec![Uid(1)], vec![u16::MAX]),
                Some(commit.clone()),
                &preflight,
                None,
            )?
            .expect("journal binding");
        let options = super::super::submit::SubmissionOptions {
            journal: Some(binding),
            shield_period: scheduler.shield_period,
        }
        .execution(0.001);
        let intent =
            super::super::submit::PreparedWeights::<super::super::submit::CommitCall>::compiled(
                super::super::submit::WeightCallPlan {
                    netuid: NetUid(1),
                    mechanism: MechId(0),
                    row: (vec![Uid(1)], vec![u16::MAX]),
                    version_key: 0,
                    commit: Some(commit.clone()),
                    commit_reveal_version: super::super::NATIVE_CR_PAYLOAD_VERSION,
                },
            );
        let queue_commit = commit.clone();
        lab.after_inclusion(move |state, _record, number| {
            super::super::tests::seed_at(
                state,
                number,
                "SubtensorModule",
                "TimelockedWeightCommits",
                (NetUidStorageIndex(1), 5u64),
                vec![(account, number, queue_commit.ciphertext.clone(), 500u64)],
            )
            .expect("native queue model");
        });
        lab.script([Inner::LostAcknowledgment]);
        let keys = LabKeys::new(lab.signer.clone());
        let executor = Executor::new(lab.client.clone());
        let pending = {
            let submission = executor.execute(&intent, &keys, &options);
            tokio::pin!(submission);
            tokio::select! {
                result=&mut submission=>panic!("returned before lost acknowledgment: {}",result.is_ok()),
                _=lab.lost_acknowledgment()=>{lab.node.drop_connection()?;submission.await}
            }
        };
        assert!(matches!(pending, Err(TxError::PendingOperation { .. })));
        assert_eq!(scheduler.reveals.pending(), 0);
        assert_eq!(lab.authors(), 1);
        drop(options);
        drop(executor);
        drop(snapshot);
        drop(scheduler);
        lab.journal().unwrap().control(JournalControl {
            submissions_paused: true,
            allow_obligations: false,
            extensions: Default::default(),
        })?;
        lab.restart()
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        scheduler = make()?
            .with_journal(
                lab.journal().unwrap().clone(),
                lab.client.genesis(),
                NetUid(1),
                account,
            )?
            .with_shield_protocol(lab.protocol.clone());
        let head = lab.client.at(BlockRef::Finalized).await?.block().clone();
        let raw = lab
            .node
            .mutate_state(|s| s.header(&format!("0x{}", hex::encode(head.hash.0))))
            .expect("head");
        let head = BlockHeader::from_json(raw)?;
        for _ in 0..2 {
            scheduler
                .recover_finalized(&lab.client, &head, &Metrics::default())
                .await?;
            assert_eq!(scheduler.reveals.pending(), 1);
            assert!(scheduler.pending_work(MechId(0))?.is_none());
        }
        // Upgrade after inclusion; observation is not permission to use the
        // new unqualified code for signing. Full historical identity is pinned.
        lab.upgrade(482, 2, H256([43; 32]))
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        lab.node.mutate_state(|state|->anyhow::Result<()> {
            state.advance(120-state.block);state.set_finalized(120)?;
            for block in 102..=120 {
                let epoch=if block<110 {5}else if block<120 {6}else{7};
                let last=if block<110 {100}else if block<120 {110}else{120};
                super::super::tests::seed_at(state,block,"SubtensorModule","SubnetEpochIndex",(NetUid(1),),epoch as u64)?;
                super::super::tests::seed_at(state,block,"SubtensorModule","LastEpochBlock",(NetUid(1),),last)?;
                super::super::tests::seed_at(state,block,"SubtensorModule","BlocksSinceLastStep",(NetUid(1),),block-last)?;
                super::super::tests::seed_at(state,block,"SubtensorModule","SubnetOwnerHotkey",(NetUid(1),),account)?;
                super::super::tests::seed_at(state,block,"Drand","LastStoredRound",(),499u64)?;
            }
            super::super::tests::seed_at(state,109,"SubtensorModule","TimelockedWeightCommits",(NetUidStorageIndex(1),5u64),vec![(account,101u64,commit.ciphertext.clone(),500u64)])?;
            if success {
                let hash=state.block_hash(110).expect("reveal block");
                state.seed_events(hash,json!([{"phase":"Initialization","event":{"SubtensorModule":{"TimelockedWeightsRevealed":[1,account.to_string()]}},"topics":[]}]))?;
            }
            Ok(())
        })?;
        let raw = lab
            .node
            .mutate_state(|s| s.block_hash(120).and_then(|h| s.header(&h)))
            .expect("finalized head");
        let head = BlockHeader::from_json(raw)?;
        scheduler
            .recover_finalized(&lab.client, &head, &Metrics::default())
            .await?;
        if !success {
            // Passing the window alone cannot erase a still-present native
            // queue entry. Only its observed removal plus complete history
            // allows the SDK to report a native commitment lost.
            assert_eq!(scheduler.reveals.pending(), 1);
            lab.node.mutate_state(|state| -> anyhow::Result<()> {
                state.advance(1);
                state.set_finalized(121)?;
                super::super::tests::seed_at(
                    state,
                    121,
                    "SubtensorModule",
                    "TimelockedWeightCommits",
                    (NetUidStorageIndex(1), 5u64),
                    Vec::<(rtao_primitives::AccountId, u64, Vec<u8>, u64)>::new(),
                )?;
                Ok(())
            })?;
            let raw = lab
                .node
                .mutate_state(|s| s.block_hash(121).and_then(|h| s.header(&h)))
                .expect("removal finalized");
            scheduler
                .recover_finalized(
                    &lab.client,
                    &BlockHeader::from_json(raw)?,
                    &Metrics::default(),
                )
                .await?;
        }
        assert_eq!(scheduler.reveals.pending(), 0);
        let outcome = scheduler
            .reveals
            .outcomes()
            .front()
            .expect("actual reveal outcome");
        assert_eq!(outcome.missed.is_none(), success);
        assert_eq!(outcome.native_queue_absent, !success);
        let record = lab.journal().unwrap().bind("public-weights")?.record()?;
        assert_eq!(
            record.intent.correlation_id.as_deref(),
            Some("evaluation:retained")
        );
        assert_eq!(record.shield_pairs.len(), 1);
        assert_eq!(record.attempts.len(), 2);
        assert_eq!(keys.unlocks(), 1);
        assert_eq!(lab.signer.signatures(), 2);
        assert_eq!(lab.authors(), 1);
        drop(scheduler);
        lab.restart()
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        let restored = make()?.with_journal(
            lab.journal().unwrap().clone(),
            lab.client.genesis(),
            NetUid(1),
            account,
        )?;
        assert_eq!(restored.reveals.outcomes().len(), 1);
        assert_eq!(restored.reveals.pending(), 0);
        drop(restored);
        lab.close().await;
    }
    Ok(())
}

#[tokio::test]
async fn public_genesis_shielded_weights_bind_the_native_protocol_by_default() -> anyhow::Result<()>
{
    let mut state = super::super::tests::fixture_state()?;
    state.set_finalized(state.block)?;
    let root =
        std::path::PathBuf::from(std::env::var_os("RTAO_TEST_TMPDIR").expect("task scratch"))
            .join(format!("public-native-weight-{}", std::process::id()));
    let mut lab = JournalLab::public(state, root.join("journal.json"), CryptoType::Ed25519)
        .await
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    assert_ne!(
        lab.client.genesis(),
        rtao_client::Client::SYNTHETIC_JOURNAL_GENESIS
    );
    let account = lab.signer.account();
    lab.node.mutate_state(|state| -> anyhow::Result<()> {
        state.seed_typed("SubtensorModule", "Uids", (NetUid(1), account), 0u16)?;
        Ok(())
    })?;
    let make = |lab: &JournalLab| -> anyhow::Result<WeightScheduler> {
        Ok(WeightScheduler::new(
            WeightPolicy::default(),
            Arc::new(ChainClock::new(1_000_000)),
            ClockGuard::default(),
        )?
        .with_journal(
            lab.journal().unwrap().clone(),
            lab.client.genesis(),
            NetUid(1),
            account,
        )?
        .with_shielded_submission(ShieldPeriod::P4))
    };
    // No `with_shield_protocol`: the native protocol is the default.
    let scheduler = make(&lab)?;
    scheduler.enqueue(
        "native-weights",
        None,
        MechId(0),
        WeightsInput::Parallel {
            uids: vec![Uid(1)],
            weights: vec![1.0],
        },
    )?;
    let snapshot = lab.client.at(BlockRef::Best).await?;
    let preflight = WeightPreflight::fetch(&snapshot, NetUid(1), MechId(0), account).await?;
    let binding = scheduler
        .prepare_binding(
            MechId(0),
            (vec![Uid(1)], vec![u16::MAX]),
            None,
            &preflight,
            None,
        )?
        .expect("journal binding");
    let options = super::super::submit::SubmissionOptions {
        journal: Some(binding),
        shield_period: scheduler.shield_period,
    }
    .execution(0.001);
    let intent = super::super::submit::PreparedWeights::<super::super::submit::SetCall>::compiled(
        super::super::submit::WeightCallPlan {
            netuid: NetUid(1),
            mechanism: MechId(0),
            row: (vec![Uid(1)], vec![u16::MAX]),
            version_key: 0,
            commit: None,
            commit_reveal_version: super::super::NATIVE_CR_PAYLOAD_VERSION,
        },
    );
    let keys = LabKeys::new(lab.signer.clone());
    let report = Executor::new(lab.client.clone())
        .execute(&intent, &keys, &options)
        .await?;
    assert!(report.success);
    assert_eq!(lab.authors(), 1);
    let record = lab.journal().unwrap().bind("native-weights")?.record()?;
    assert!(matches!(
        &record.mode,
        rtao_client::journal::ExecutionMode::Shielded(policy)
            if policy.protocol == rtao_client::journal::NativeShieldProtocol::PROFILE
    ));
    drop(options);
    drop(snapshot);
    drop(scheduler);
    lab.restart()
        .await
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    // A restarted scheduler, again without an explicit adapter, recovers the
    // retained finalized effect instead of requiring adapter reinstallation.
    let scheduler = make(&lab)?;
    let (binding, _) = scheduler.pending_work(MechId(0))?.expect("retained work");
    assert!(matches!(
        binding.recover(&lab.client, 128).await?,
        rtao_client::journal::RecoveryStatus::Finalized(_)
    ));
    assert_eq!(lab.authors(), 1);
    assert_eq!(keys.unlocks(), 1);
    drop(binding);
    drop(scheduler);
    lab.close().await;
    Ok(())
}

/// Enter through the real finalized scheduler callback with native wallet,
/// metagraph worker and Executor. Direct rows avoid pretending the controlled
/// peer decrypts a real timelock; the paired-reveal test above covers handoff.
#[tokio::test]
async fn finalized_scheduler_callback_preserves_queued_work_across_pause_restart_and_parameter_change()
-> anyhow::Result<()> {
    use crate::{EpochClock, EpochStatus, NeuronCx};
    use rtao_metagraph::{MetagraphSync, Preset, SyncPolicy};
    use rtao_runtime::SelectiveMetagraphIndex;
    use rtao_wallet::{RegenerateFrom, SecretString, Wallet, WriteOptions};
    let root = std::path::PathBuf::from(std::env::var_os("RTAO_TEST_TMPDIR").expect("scratch"))
        .join(format!("scheduler-callback-{}", std::process::id()));

    let wallet = Wallet::new("synthetic", "validator", root.join("wallet"))?;
    let account = wallet
        .regenerate_hotkey(
            RegenerateFrom::PrivateKey(SecretString::new(format!("0x{}", "07".repeat(32)))),
            CryptoType::Ed25519,
            WriteOptions::hotkey(),
        )
        .await?;

    let mut state = super::super::tests::fixture_state()?;
    state.set_finalized(state.block)?;
    state.seed(
        &super::super::st::COMMIT_REVEAL_WEIGHTS_ENABLED,
        (NetUid(1),),
        false,
    )?;
    state.seed_typed("SubtensorModule", "Uids", (NetUid(1), account), 0u16)?;
    state.seed(
        &super::super::st::MECHANISM_COUNT_CURRENT,
        (NetUid(1),),
        MechId(1),
    )?;
    state.seed(
        &super::super::st::SUBNET_OWNER_HOTKEY,
        (NetUid(1),),
        rtao_primitives::AccountId([9; 32]).into(),
    )?;
    state.seed_runtime("SubnetInfoRuntimeApi","get_selective_mechagraph",json!({"netuid":1,"block":100,"tempo":10,"last_step":100,"blocks_since_last_step":0,"num_uids":3,"hotkeys":[account.to_string(),rtao_primitives::AccountId([8;32]).to_string(),rtao_primitives::AccountId([9;32]).to_string()]}))?;
    let mut lab =
        JournalLab::with_head_tracking(state, root.join("journal.json"), CryptoType::Ed25519)
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    assert_eq!(account, lab.signer.account());

    let context = async |client: rtao_client::Client| -> anyhow::Result<NeuronCx> {
        let shutdown = tokio_util::sync::CancellationToken::new();
        let (metagraph, _) = MetagraphSync::spawn(
            client.clone(),
            NetUid(1),
            MechId(0),
            SyncPolicy {
                preset: Preset::Custom(vec![SelectiveMetagraphIndex::Hotkeys]),
                ..SyncPolicy::validator()
            },
            shutdown.clone(),
        )
        .await?;
        Ok(NeuronCx {
            executor: Executor::new(client.clone()),
            client,
            netuid: NetUid(1),
            hotkey: wallet.hotkey_only(),
            metagraph,
            epochs: EpochClock::new(EpochStatus::default()),
            http: rtao_btauth::client::SignedClient::new(wallet.unlock_hotkey().await?)?,
            shutdown,
            metrics: Metrics::default(),
            state_dir: root.join("state"),
        })
    };
    let make = || {
        WeightScheduler::new(
            WeightPolicy::default(),
            Arc::new(ChainClock::new(1_000_000)),
            ClockGuard::default(),
        )
    };
    let cx = context(lab.client.clone()).await?;

    let mut scheduler = make()?
        .with_journal(
            lab.journal().unwrap().clone(),
            lab.client.genesis(),
            NetUid(1),
            account,
        )?
        .with_shield_protocol(lab.protocol.clone())
        .with_shielded_submission(ShieldPeriod::P4);
    let head = async || -> anyhow::Result<BlockHeader> {
        let tip = lab.client.at(BlockRef::Finalized).await?.block().clone();
        let raw = lab
            .node
            .mutate_state(|s| s.header(&format!("0x{}", hex::encode(tip.hash.0))))
            .expect("head");
        Ok(BlockHeader::from_json(raw)?)
    };

    Box::pin(scheduler.on_finalized(&cx, &head().await?, |_| {
        super::super::WeightsProposal::Operation {
            id: "callback-first".into(),
            correlation_id: Some("app:one".into()),
            input: WeightsInput::Parallel {
                uids: vec![Uid(1)],
                weights: vec![1.0],
            },
        }
    }))
    .await?;
    assert_eq!(lab.authors(), 1, "{:?}", scheduler.last_attempt(MechId(0)));
    assert!(matches!(
        scheduler.last_attempt(MechId(0)).unwrap().outcome,
        super::super::WeightAttemptOutcome::Submitted { .. }
    ));
    scheduler.enqueue(
        "callback-pending",
        Some("app:two".into()),
        MechId(0),
        WeightsInput::Parallel {
            uids: vec![Uid(1)],
            weights: vec![2.0],
        },
    )?;
    lab.journal().unwrap().control(JournalControl {
        submissions_paused: true,
        allow_obligations: false,
        extensions: Default::default(),
    })?;
    Box::pin(scheduler.on_finalized(&cx, &head().await?, |_| {
        panic!("paused callback must not acquire new work")
    }))
    .await?;
    assert!(scheduler.pending_work(MechId(0))?.is_some());
    assert_eq!(lab.authors(), 1);
    drop(scheduler);
    cx.metagraph.shutdown().await;
    cx.shutdown.cancel();
    drop(cx);
    lab.restart()
        .await
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    let mut scheduler = make()?
        .with_journal(
            lab.journal().unwrap().clone(),
            lab.client.genesis(),
            NetUid(1),
            account,
        )?
        .with_shield_protocol(lab.protocol.clone())
        .with_shielded_submission(ShieldPeriod::P4);
    lab.node.mutate_state(|s| -> anyhow::Result<()> {
        s.advance(20);
        s.seed(
            &super::super::st::WEIGHTS_SET_RATE_LIMIT,
            (NetUid(1),),
            100u64,
        )?;
        s.seed(&super::super::st::TEMPO, (NetUid(1),), 7u16)?;
        Ok(())
    })?;
    lab.journal().unwrap().control(Default::default())?;
    let current = lab.client.at(BlockRef::Best).await?;
    let preflight = WeightPreflight::fetch(&current, NetUid(1), MechId(0), account).await?;
    assert!(matches!(
        scheduler.next_action(MechId(0), account, &preflight)?,
        super::super::WeightOpportunity::RateLimited {
            retry_after_block: 201
        }
    ));
    let cx = context(lab.client.clone()).await?;
    let tip = lab.client.at(BlockRef::Finalized).await?.block().clone();
    let raw = lab
        .node
        .mutate_state(|s| s.header(&format!("0x{}", hex::encode(tip.hash.0))))
        .expect("head");
    Box::pin(
        scheduler.on_finalized(&cx, &BlockHeader::from_json(raw)?, |_| {
            panic!("queued application input must be reused")
        }),
    )
    .await?;
    assert_eq!(lab.authors(), 1);
    assert!(scheduler.pending_work(MechId(0))?.is_some());
    lab.node.mutate_state(|s| -> anyhow::Result<()> {
        s.advance(1);
        s.seed(
            &super::super::st::WEIGHTS_SET_RATE_LIMIT,
            (NetUid(1),),
            0u64,
        )?;
        Ok(())
    })?;
    let tip = lab.client.at(BlockRef::Finalized).await?.block().clone();
    let raw = lab
        .node
        .mutate_state(|s| s.header(&format!("0x{}", hex::encode(tip.hash.0))))
        .expect("head");
    Box::pin(
        scheduler.on_finalized(&cx, &BlockHeader::from_json(raw)?, |_| {
            panic!("restarted queued work must not be discarded")
        }),
    )
    .await?;
    assert_eq!(lab.authors(), 2, "{:?}", scheduler.last_attempt(MechId(0)));
    assert!(scheduler.pending_work(MechId(0))?.is_none());
    let record = lab.journal().unwrap().bind("callback-pending")?.record()?;
    assert_eq!(record.intent.correlation_id.as_deref(), Some("app:two"));
    assert_eq!(record.attempts.len(), 2);
    assert_eq!(record.shield_pairs.len(), 1);
    drop(scheduler);
    cx.metagraph.shutdown().await;
    cx.shutdown.cancel();
    drop(cx);
    lab.close().await;
    Ok(())
}

/// A validator context over a head-tracking lab whose stored
/// `CommitRevealWeightsVersion` is `tag`. The lab answers every `:code` read
/// with `H256([42; 32])`.
async fn cr_lab(
    name: &str,
    tag: u16,
) -> anyhow::Result<(JournalLab, crate::NeuronCx, rtao_primitives::AccountId)> {
    cr_lab_with(name, tag, |_| Ok(())).await
}

/// [`cr_lab`] whose fixture state `patch` edits first, such as its metadata.
pub(super) async fn cr_lab_with(
    name: &str,
    tag: u16,
    patch: impl FnOnce(&mut rtao_testkit::FakeState) -> anyhow::Result<()>,
) -> anyhow::Result<(JournalLab, crate::NeuronCx, rtao_primitives::AccountId)> {
    use crate::{EpochClock, EpochStatus, NeuronCx};
    use rtao_metagraph::{MetagraphSync, Preset, SyncPolicy};
    use rtao_runtime::SelectiveMetagraphIndex;
    use rtao_wallet::{RegenerateFrom, SecretString, Wallet, WriteOptions};
    let root = std::path::PathBuf::from(std::env::var_os("RTAO_TEST_TMPDIR").expect("scratch"))
        .join(format!("scheduler-cr-{name}-{}", std::process::id()));
    let wallet = Wallet::new("synthetic", "validator", root.join("wallet"))?;
    let account = wallet
        .regenerate_hotkey(
            RegenerateFrom::PrivateKey(SecretString::new(format!("0x{}", "07".repeat(32)))),
            CryptoType::Ed25519,
            WriteOptions::hotkey(),
        )
        .await?;
    let mut state = super::super::tests::fixture_state()?;
    patch(&mut state)?;
    state.set_finalized(state.block)?;
    state.seed(&super::super::st::COMMIT_REVEAL_WEIGHTS_VERSION, (), tag)?;
    state.seed_typed("SubtensorModule", "Uids", (NetUid(1), account), 0u16)?;
    state.seed(
        &super::super::st::MECHANISM_COUNT_CURRENT,
        (NetUid(1),),
        MechId(1),
    )?;
    state.seed(
        &super::super::st::SUBNET_OWNER_HOTKEY,
        (NetUid(1),),
        rtao_primitives::AccountId([9; 32]).into(),
    )?;
    state.seed_runtime("SubnetInfoRuntimeApi","get_selective_mechagraph",json!({"netuid":1,"block":100,"tempo":10,"last_step":100,"blocks_since_last_step":0,"num_uids":3,"hotkeys":[account.to_string(),rtao_primitives::AccountId([8;32]).to_string(),rtao_primitives::AccountId([9;32]).to_string()]}))?;
    let lab = JournalLab::with_head_tracking(state, root.join("journal.json"), CryptoType::Ed25519)
        .await
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    let shutdown = tokio_util::sync::CancellationToken::new();
    let (metagraph, _) = MetagraphSync::spawn(
        lab.client.clone(),
        NetUid(1),
        MechId(0),
        SyncPolicy {
            preset: Preset::Custom(vec![SelectiveMetagraphIndex::Hotkeys]),
            ..SyncPolicy::validator()
        },
        shutdown.clone(),
    )
    .await?;
    let cx = NeuronCx {
        executor: Executor::new(lab.client.clone()),
        client: lab.client.clone(),
        netuid: NetUid(1),
        hotkey: wallet.hotkey_only(),
        metagraph,
        epochs: EpochClock::new(EpochStatus::default()),
        http: rtao_btauth::client::SignedClient::new(wallet.unlock_hotkey().await?)?,
        shutdown,
        metrics: Metrics::default(),
        state_dir: root.join("state"),
    };
    Ok((lab, cx, account))
}

/// Finalize one more lab block and return its header.
pub(super) fn advance(lab: &JournalLab) -> anyhow::Result<BlockHeader> {
    let raw = lab
        .node
        .mutate_state(|s| -> anyhow::Result<_> {
            s.advance(1);
            s.set_finalized(s.block)?;
            Ok(s.block_hash(s.block).and_then(|h| s.header(&h)))
        })?
        .expect("finalized head");
    Ok(BlockHeader::from_json(raw)?)
}

pub(super) async fn close_cr_lab(
    scheduler: WeightScheduler,
    mut lab: JournalLab,
    cx: crate::NeuronCx,
) {
    drop(scheduler);
    cx.metagraph.shutdown().await;
    cx.shutdown.cancel();
    drop(cx);
    lab.close().await;
}

/// `(pallet index, error index)` of a SubtensorModule error.
pub(super) fn module_error(name: &str) -> (u8, u8) {
    let error = rtao_runtime::errors::MODULE_ERRORS
        .iter()
        .find(|error| error.pallet == "SubtensorModule" && error.name == name)
        .expect("known module error");
    (error.pallet_index, error.error_index)
}

/// `(ciphertext, reveal_round, commit_reveal_version)` of a timelocked commit.
pub(super) type CommitTail = (Vec<u8>, u64, u16);

/// The [`CommitTail`] closing a signed
/// `commit_timelocked_mechanism_weights(netuid 1, mecid 0, ..)` extrinsic.
pub(super) fn timelocked_tail(bytes: &[u8]) -> CommitTail {
    let n = bytes.len();
    let tag = u16::from_le_bytes(bytes[n - 2..].try_into().unwrap());
    let round = u64::from_le_bytes(bytes[n - 10..n - 2].try_into().unwrap());
    let body = &bytes[..n - 10];
    let compact = |len: usize| -> Vec<u8> {
        if len < 1 << 6 {
            vec![(len as u8) << 2]
        } else if len < 1 << 14 {
            (((len as u16) << 2) | 1).to_le_bytes().to_vec()
        } else {
            (((len as u32) << 2) | 2).to_le_bytes().to_vec()
        }
    };
    for len in (1..body.len()).rev() {
        let prefix = compact(len);
        let Some(start) = body.len().checked_sub(len + prefix.len()) else {
            continue;
        };
        if body[start..start + prefix.len()] == prefix[..] && body[..start].ends_with(&[1, 0, 0]) {
            return (body[body.len() - len..].to_vec(), round, tag);
        }
    }
    panic!("not a timelocked mechanism commit")
}

/// Route plain signed author requests to the built-in pool and record each
/// one's commit tail. Every included commit is placed in the native queue so
/// the reveal watcher can locate it; `on_submit` sees the submission ordinal.
pub(super) fn plain_author(
    lab: &JournalLab,
    account: rtao_primitives::AccountId,
    mut on_submit: impl FnMut(&mut rtao_testkit::FakeState, usize) + Send + 'static,
) -> Arc<std::sync::Mutex<Vec<CommitTail>>> {
    let tails = Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen = tails.clone();
    lab.node.mutate_state(|state| {
        state.set_handler("author_submitAndWatchExtrinsic", move |state, request| {
            let ordinal = seen.lock().unwrap().len();
            on_submit(state, ordinal);
            let reply = state.dispatch(request);
            let tail = timelocked_tail(state.submissions.last().expect("submission"));
            let number = state.block;
            super::super::tests::seed_at(
                state,
                number,
                "SubtensorModule",
                "TimelockedWeightCommits",
                (NetUidStorageIndex(1), 5u64),
                vec![(account, number, tail.0.clone(), tail.1)],
            )
            .expect("native queue model");
            seen.lock().unwrap().push(tail);
            reply
        })
    });
    tails
}

/// Feed finalized heads until the cadence slot opens and the scheduler
/// authors, at most one epoch of heads.
pub(super) async fn until_authored(
    scheduler: &mut WeightScheduler,
    lab: &JournalLab,
    cx: &crate::NeuronCx,
) -> anyhow::Result<()> {
    for _ in 0..12 {
        let head = advance(lab)?;
        Box::pin(scheduler.on_finalized(cx, &head, scored)).await?;
        if lab.authors() > 0 {
            return Ok(());
        }
    }
    anyhow::bail!(
        "no attempt was due: {:?}",
        scheduler.last_attempt(MechId(0))
    )
}

pub(super) fn scored(_: MechId) -> super::super::WeightsProposal {
    super::super::WeightsProposal::Set(WeightsInput::Parallel {
        uids: vec![Uid(1)],
        weights: vec![1.0],
    })
}

/// A changed `CommitRevealWeightsVersion` under unproven runtime code pauses
/// timelocked commits without errors or signing, every finalized head, until a
/// reveal under the live code proves it decodes the native payload. The commit
/// then carries the chain's tag (D-086).
#[tokio::test]
async fn changed_cr_version_pauses_quietly_then_follows_proven_code() -> anyhow::Result<()> {
    let (lab, cx, account) = cr_lab("version", 5).await?;
    let mut scheduler = WeightScheduler::new(
        WeightPolicy::default(),
        Arc::new(ChainClock::new(1_800_000_000_000)),
        ClockGuard::default(),
    )?
    .with_journal(
        lab.journal().unwrap().clone(),
        lab.client.genesis(),
        NetUid(1),
        account,
    )?
    .with_shield_protocol(lab.protocol.clone())
    .with_shielded_submission(ShieldPeriod::P4);
    // The lab answers every `:code` read with this synthetic identity.
    let code = H256([42; 32]);
    for _ in 0..3 {
        let head = advance(&lab)?;
        Box::pin(scheduler.on_finalized(&cx, &head, |_| {
            panic!("a paused commit-reveal mechanism must not score")
        }))
        .await?;
    }
    assert_eq!(lab.authors(), 0, "no commit is signed or submitted");
    let Some(super::super::WeightAttemptOutcome::Refused(refusal)) = scheduler
        .last_attempt(MechId(0))
        .map(|attempt| attempt.outcome.clone())
    else {
        panic!(
            "expected a refusal: {:?}",
            scheduler.last_attempt(MechId(0))
        )
    };
    assert_eq!(refusal.name, "IncorrectCommitRevealVersion");
    assert!(
        refusal
            .remediation
            .contains(&format!("0x{}", hex::encode(code.0)))
    );
    assert!(refusal.remediation.contains("upgrade"));
    let rendered = cx.metrics.render();
    assert!(
        rendered.contains("rtao_weights_cr_version_mismatch_total 3"),
        "{rendered}"
    );
    assert!(
        rendered.contains("rtao_weights_cr_version_paused{mech=\"0\"} 1"),
        "{rendered}"
    );
    assert!(
        !rendered.contains("rtao_weights_errors_total"),
        "{rendered}"
    );
    assert!(!cx.metrics.readiness().last_weight_succeeded);
    let best = lab.client.at(BlockRef::Best).await?;
    let preflight = WeightPreflight::fetch(&best, NetUid(1), MechId(0), account).await?;
    assert_eq!(preflight.runtime_code, Some(code));
    assert_eq!(
        scheduler.next_action(MechId(0), account, &preflight)?,
        super::super::WeightOpportunity::Paused
    );

    // An earlier commitment revealed under the live code while 5 was stored:
    // follow the tag.
    scheduler.reveals.remember_revealed_code(code, 5);
    let signed = Arc::new(std::sync::Mutex::new(None::<(Vec<u8>, u64)>));
    let captured = signed.clone();
    lab.after_inclusion(move |state, record, number| {
        let pair = record.shield_pairs.last().expect("shield pair");
        let inner = record.attempt(pair.inner).expect("inner attempt");
        let (ciphertext, round): (Vec<u8>, u64) =
            serde_json::from_value(record.material["commit"].clone()).expect("commit material");
        *captured.lock().unwrap() = Some((inner.bytes.clone(), round));
        super::super::tests::seed_at(
            state,
            number,
            "SubtensorModule",
            "TimelockedWeightCommits",
            (NetUidStorageIndex(1), 5u64),
            vec![(account, number, ciphertext, round)],
        )
        .expect("native queue model");
    });
    let head = advance(&lab)?;
    Box::pin(scheduler.on_finalized(&cx, &head, scored)).await?;
    assert_eq!(lab.authors(), 1, "{:?}", scheduler.last_attempt(MechId(0)));
    assert!(matches!(
        scheduler.last_attempt(MechId(0)).unwrap().outcome,
        super::super::WeightAttemptOutcome::Submitted { .. }
    ));
    let (bytes, round) = signed.lock().unwrap().take().expect("signed commit");
    // The call closes the signed extrinsic: `.., reveal_round: u64, version: u16`.
    let tail = [round.to_le_bytes().as_slice(), &5u16.to_le_bytes()].concat();
    assert!(
        bytes.ends_with(&tail),
        "commit is not tagged with version 5"
    );
    assert_eq!(scheduler.reveals.pending(), 1);
    let rendered = cx.metrics.render();
    assert!(
        rendered.contains("rtao_weights_cr_version_paused{mech=\"0\"} 0"),
        "{rendered}"
    );
    assert!(
        rendered.contains("rtao_weights_submitted_total 1"),
        "{rendered}"
    );
    close_cr_lab(scheduler, lab, cx).await;
    Ok(())
}

/// Root changes `CommitRevealWeightsVersion` between preflight and inclusion:
/// the chain rejects the commit fee-free at dispatch. Without a journal
/// binding the scheduler waits, re-reads the preflight and re-selects the tag
/// from fresh state, at most [`super::super::CR_VERSION_RETRIES`] times. It
/// re-signs with the new tag only when the code is proven for it, and
/// otherwise pauses without an error (D-086).
#[tokio::test]
async fn dispatch_rejected_cr_version_reselects_the_tag_from_fresh_state() -> anyhow::Result<()> {
    let (cr_pallet, cr_error) = module_error("IncorrectCommitRevealVersion");
    let rejection = json!({"phase":"ApplyExtrinsic","extrinsic_idx":0,"module_id":"System","event_id":"ExtrinsicFailed","attributes":{"dispatch_error":{"Module":{"index":cr_pallet,"error":[cr_error,0,0,0]}},"dispatch_info":{"weight":{"ref_time":1,"proof_size":0},"class":"Normal","pays_fee":"No"}},"topics":[]});
    let code = H256([42; 32]);
    for proven in [true, false] {
        let (lab, cx, account) = cr_lab(&format!("dispatch-{proven}"), 4).await?;
        let mut scheduler = WeightScheduler::new(
            WeightPolicy::default(),
            Arc::new(ChainClock::new(1_800_000_000_000)),
            ClockGuard::default(),
        )?;
        if proven {
            scheduler.reveals.remember_revealed_code(code, 5);
        }
        let rejection = rejection.clone();
        let tails = plain_author(&lab, account, move |state, ordinal| {
            if ordinal == 0 {
                // Root stored 5 after the preflight read 4.
                state.queue_submit(rtao_testkit::SubmitScript::InBlockThenFinalized {
                    events: json!([rejection.clone()]),
                });
                state
                    .seed(&super::super::st::COMMIT_REVEAL_WEIGHTS_VERSION, (), 5u16)
                    .expect("stored tag");
            }
        });
        until_authored(&mut scheduler, &lab, &cx).await?;
        let tails = tails.lock().unwrap().clone();
        let rendered = cx.metrics.render();
        assert!(
            !rendered.contains("rtao_weights_errors_total"),
            "{rendered}"
        );
        assert_eq!(tails[0].2, 4, "the first commit carries the preflight tag");
        assert!(!scheduler.slots[&MechId(0)].uncertain);
        if proven {
            assert_eq!(lab.authors(), 2, "exactly one retry");
            assert_eq!(tails[1].2, 5, "the retry carries the newly selected tag");
            assert_ne!(tails[1].0, tails[0].0, "the retry is a fresh commitment");
            assert!(matches!(
                scheduler.last_attempt(MechId(0)).unwrap().outcome,
                super::super::WeightAttemptOutcome::Submitted { .. }
            ));
            assert!(
                rendered.contains("rtao_weights_cr_version_recomputed_total 1"),
                "{rendered}"
            );
            assert!(
                rendered.contains("rtao_weights_submitted_total 1"),
                "{rendered}"
            );
            assert_eq!(scheduler.reveals.pending(), 1);
        } else {
            assert_eq!(lab.authors(), 1, "unproven fresh state never re-signs");
            let Some(super::super::WeightAttemptOutcome::Refused(refusal)) = scheduler
                .last_attempt(MechId(0))
                .map(|attempt| attempt.outcome.clone())
            else {
                panic!("expected a pause: {:?}", scheduler.last_attempt(MechId(0)))
            };
            assert_eq!(refusal.name, "IncorrectCommitRevealVersion");
            assert!(
                rendered.contains("rtao_weights_cr_version_paused{mech=\"0\"} 1"),
                "{rendered}"
            );
            assert!(
                !rendered.contains("rtao_weights_cr_version_recomputed_total"),
                "{rendered}"
            );
            assert_eq!(scheduler.reveals.pending(), 0);
        }
        close_cr_lab(scheduler, lab, cx).await;
    }
    Ok(())
}

/// Rejections that fresh state cannot explain stop after
/// [`super::super::CR_VERSION_RETRIES`] re-selections, each after a backoff.
#[tokio::test]
async fn persistent_cr_version_rejections_stop_after_the_retry_bound() -> anyhow::Result<()> {
    let (cr_pallet, cr_error) = module_error("IncorrectCommitRevealVersion");
    let rejection = json!({"phase":"ApplyExtrinsic","extrinsic_idx":0,"module_id":"System","event_id":"ExtrinsicFailed","attributes":{"dispatch_error":{"Module":{"index":cr_pallet,"error":[cr_error,0,0,0]}},"dispatch_info":{"weight":{"ref_time":1,"proof_size":0},"class":"Normal","pays_fee":"No"}},"topics":[]});
    let (lab, cx, account) = cr_lab("dispatch-bound", 4).await?;
    let mut scheduler = WeightScheduler::new(
        WeightPolicy::default(),
        Arc::new(ChainClock::new(1_800_000_000_000)),
        ClockGuard::default(),
    )?;
    let tails = plain_author(&lab, account, move |state, _| {
        state.queue_submit(rtao_testkit::SubmitScript::InBlockThenFinalized {
            events: json!([rejection.clone()]),
        });
    });
    until_authored(&mut scheduler, &lab, &cx).await?;
    let retries = usize::try_from(super::super::CR_VERSION_RETRIES)?;
    assert_eq!(lab.authors(), 1 + retries);
    assert!(tails.lock().unwrap().iter().all(|tail| tail.2 == 4));
    let rendered = cx.metrics.render();
    assert!(
        rendered.contains(&format!(
            "rtao_weights_cr_version_recomputed_total {retries}"
        )),
        "{rendered}"
    );
    assert!(
        rendered.contains("rtao_weights_errors_total 1"),
        "{rendered}"
    );
    let Some(super::super::WeightAttemptOutcome::Failed(error)) = scheduler
        .last_attempt(MechId(0))
        .map(|attempt| attempt.outcome.clone())
    else {
        panic!(
            "expected a failure: {:?}",
            scheduler.last_attempt(MechId(0))
        )
    };
    assert!(error.contains("Incorrect commit-reveal version"), "{error}");
    assert!(!scheduler.slots[&MechId(0)].uncertain);
    assert_eq!(scheduler.reveals.pending(), 0);
    close_cr_lab(scheduler, lab, cx).await;
    Ok(())
}

/// A journal-bound commit rejected for its tag is never re-signed in the same
/// attempt: the slot stays uncertain until durable recovery excludes the
/// signed bytes, and no tag is re-selected.
#[tokio::test]
async fn journal_bound_cr_version_rejection_goes_uncertain_without_resigning() -> anyhow::Result<()>
{
    let (cr_pallet, cr_error) = module_error("IncorrectCommitRevealVersion");
    let (lab, cx, account) = cr_lab("dispatch-journal", 4).await?;
    let mut scheduler = WeightScheduler::new(
        WeightPolicy::default(),
        Arc::new(ChainClock::new(1_800_000_000_000)),
        ClockGuard::default(),
    )?
    .with_journal(
        lab.journal().unwrap().clone(),
        lab.client.genesis(),
        NetUid(1),
        account,
    )?
    .with_shield_protocol(lab.protocol.clone())
    .with_shielded_submission(ShieldPeriod::P4);
    scheduler.reveals.remember_revealed_code(H256([42; 32]), 5);
    lab.script([Inner::Fail(cr_pallet, cr_error)]);
    lab.after_inclusion(|state, _record, _number| {
        state
            .seed(&super::super::st::COMMIT_REVEAL_WEIGHTS_VERSION, (), 5u16)
            .expect("stored tag");
    });
    until_authored(&mut scheduler, &lab, &cx).await?;
    assert_eq!(lab.authors(), 1, "{:?}", scheduler.last_attempt(MechId(0)));
    let outcome = format!("{:?}", scheduler.last_attempt(MechId(0)).unwrap().outcome);
    assert!(
        outcome.contains("Incorrect commit-reveal version"),
        "{outcome}"
    );
    assert!(scheduler.slots[&MechId(0)].uncertain);
    // Later heads never re-sign the uncertain slot's bytes in this attempt.
    let head = advance(&lab)?;
    Box::pin(scheduler.on_finalized(&cx, &head, scored)).await?;
    assert_eq!(lab.authors(), 1);
    let rendered = cx.metrics.render();
    assert!(
        !rendered.contains("rtao_weights_cr_version_recomputed_total"),
        "{rendered}"
    );
    assert!(
        !rendered.contains("rtao_weights_submitted_total"),
        "{rendered}"
    );
    close_cr_lab(scheduler, lab, cx).await;
    Ok(())
}
