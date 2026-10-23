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
            )?
            .expect("journal binding");
        let options = super::super::submit::SubmissionOptions {
            journal: Some(binding),
            shield_period: scheduler.shield_period,
        }
        .execution(0.001);
        let intent = super::super::submit::PreparedWeights {
            netuid: NetUid(1),
            mechanism: MechId(0),
            row: (vec![Uid(1)], vec![u16::MAX]),
            version_key: 0,
            commit: Some(commit.clone()),
        };
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
