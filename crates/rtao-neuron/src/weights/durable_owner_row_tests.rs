//! Owner-only weight rows through the real scheduler (D-093): held by
//! default, submitted under [`OwnerRowPolicy::Submit`] as a direct row or a
//! timelocked commit with its reveal tracking, and journaled with durable
//! work so restart, recovery and re-preparation decide the same way.
//!
//! The lab subnet has this validator at UID 0 and the owner hotkey `[9; 32]`
//! at UID 2, with a 360-block tempo and commit-reveal enabled unless noted.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use super::super::{OwnerRowPolicy, WeightAttemptOutcome, WeightCadence, WeightsProposal};
use super::public_tests::{advance, close_cr_lab, cr_lab_with, module_error, plain_author};
use super::*;
use crate::{ChainClock, ClockGuard, Metrics, NeuronCx, WeightPolicy};
use rtao_client::{BlockRef, ShieldPeriod, journal::JournalControl};
use rtao_primitives::{H256, NetUidStorageIndex};
use rtao_testkit::journal_shield::{Inner, JournalLab};
use serde_json::json;

/// The owner burn row: every positive weight on the owner hotkey's UID 2.
fn burn() -> WeightsInput {
    WeightsInput::ByUid(
        [(Uid(0), 0.0), (Uid(1), 0.0), (Uid(2), 65535.0)]
            .into_iter()
            .collect(),
    )
}

/// The conformed burn row the chain receives and stores.
fn burn_row() -> (Vec<Uid>, Vec<u16>) {
    (vec![Uid(2)], vec![65535])
}

fn by_uid(row: &[(u16, f64)]) -> WeightsInput {
    WeightsInput::ByUid(row.iter().map(|&(uid, w)| (Uid(uid), w)).collect())
}

/// A lab at the reported tempo; `patch` edits its state first.
async fn owner_lab(
    name: &str,
    patch: impl FnOnce(&mut rtao_testkit::FakeState) -> anyhow::Result<()>,
) -> anyhow::Result<(JournalLab, NeuronCx, AccountId)> {
    cr_lab_with(&format!("owner-row-{name}"), 4, |state| {
        state.seed(&super::super::st::TEMPO, (NetUid(1),), 360u16)?;
        patch(state)
    })
    .await
}

fn policy() -> WeightPolicy {
    WeightPolicy {
        cadence: Some(WeightCadence::OncePerEpoch { offset_blocks: 2 }),
        ..Default::default()
    }
}

fn memory() -> anyhow::Result<WeightScheduler> {
    WeightScheduler::new(
        policy(),
        Arc::new(ChainClock::new(1_800_000_000_000)),
        ClockGuard::default(),
    )
}

fn durable(lab: &JournalLab, account: AccountId) -> anyhow::Result<WeightScheduler> {
    Ok(memory()?
        .with_journal(
            lab.journal().unwrap().clone(),
            lab.client.genesis(),
            NetUid(1),
            account,
        )?
        .with_shield_protocol(lab.protocol.clone())
        .with_shielded_submission(ShieldPeriod::P4))
}

/// Feed `heads` finalized heads through [`WeightScheduler::on_finalized_with`].
async fn drive(
    scheduler: &mut WeightScheduler,
    lab: &JournalLab,
    cx: &NeuronCx,
    heads: usize,
    mut proposals: impl FnMut(MechId) -> (WeightsProposal, OwnerRowPolicy),
) -> anyhow::Result<()> {
    for _ in 0..heads {
        let head = advance(lab)?;
        Box::pin(scheduler.on_finalized_with(cx, &head, &mut proposals)).await?;
    }
    Ok(())
}

/// Feed finalized heads until the scheduler authors once more, at most one
/// lab epoch of heads.
async fn until_authored_with(
    scheduler: &mut WeightScheduler,
    lab: &JournalLab,
    cx: &NeuronCx,
    mut proposals: impl FnMut(MechId) -> (WeightsProposal, OwnerRowPolicy),
) -> anyhow::Result<()> {
    let before = lab.authors();
    for _ in 0..12 {
        let head = advance(lab)?;
        Box::pin(scheduler.on_finalized_with(cx, &head, &mut proposals)).await?;
        if lab.authors() > before {
            return Ok(());
        }
    }
    anyhow::bail!(
        "no attempt was authored: {:?}",
        scheduler.last_attempt(MechId(0))
    )
}

fn outcome(scheduler: &WeightScheduler) -> WeightAttemptOutcome {
    scheduler
        .last_attempt(MechId(0))
        .expect("an attempt")
        .outcome
        .clone()
}

fn submitted(scheduler: &WeightScheduler) -> bool {
    matches!(outcome(scheduler), WeightAttemptOutcome::Submitted { .. })
}

/// Destination UIDs of every commitment under reveal observation.
fn pending_rows(reveals: &RevealWatcher) -> Vec<Vec<Uid>> {
    let image = serde_json::to_value(reveals).unwrap();
    let located = image["pending"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| serde_json::from_value(p["uids"].clone()).unwrap());
    let unlocated = image["unlocated"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| serde_json::from_value(p["row"][0].clone()).unwrap());
    located.chain(unlocated).collect()
}

/// The prepared row of every signed attempt of a retained operation.
fn attempt_rows(record: &rtao_client::journal::OperationRecord) -> Vec<(Vec<Uid>, Vec<u16>)> {
    record
        .attempts
        .iter()
        .filter_map(|attempt| attempt.material.get("row"))
        .map(|row| serde_json::from_value(row.clone()).unwrap())
        .collect()
}

/// Model the native commit queue after each journaled shielded inclusion, so
/// reveal tracking can locate the commitment.
fn queue_model(lab: &JournalLab, account: AccountId) {
    lab.after_inclusion(move |state, record, number| {
        let (ciphertext, round): (Vec<u8>, u64) =
            serde_json::from_value(record.material["commit"].clone()).expect("commit material");
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
}

/// Restart the lab's journal and client, and rebuild the validator context
/// over the new client. Every scheduler must already be dropped.
async fn restart(lab: &mut JournalLab, cx: NeuronCx) -> anyhow::Result<NeuronCx> {
    use rtao_metagraph::{MetagraphSync, Preset, SyncPolicy};
    use rtao_runtime::SelectiveMetagraphIndex;
    cx.metagraph.shutdown().await;
    cx.shutdown.cancel();
    lab.restart()
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
    Ok(NeuronCx {
        executor: rtao_tx::Executor::new(lab.client.clone()),
        client: lab.client.clone(),
        metagraph,
        shutdown,
        metrics: Metrics::default(),
        ..cx
    })
}

/// The reported row, `[(owner_uid, 65535)]` with every other UID 0 under
/// commit-reveal at tempo 360, holds by default and is committed only under
/// [`OwnerRowPolicy::Submit`]. Self-only and self-plus-owner rows keep
/// holding; an owner validator's own row is the owner row.
#[tokio::test]
async fn owner_only_row_holds_by_default_and_submits_when_opted_in() -> anyhow::Result<()> {
    let (lab, cx, account) = owner_lab("opt-in", |_| Ok(())).await?;
    let tails = plain_author(&lab, account, |_, _| {});
    let mut scheduler = memory()?;
    for _ in 0..12 {
        let head = advance(&lab)?;
        Box::pin(scheduler.on_finalized(&cx, &head, |_| WeightsProposal::Set(burn()))).await?;
        if scheduler
            .last_attempt(MechId(0))
            .is_some_and(|attempt| attempt.outcome == WeightAttemptOutcome::HoldPrevious)
        {
            break;
        }
    }
    assert_eq!(outcome(&scheduler), WeightAttemptOutcome::HoldPrevious);
    assert_eq!(lab.authors(), 0);
    let rendered = cx.metrics.render();
    assert!(rendered.contains("rtao_weights_hold_total"), "{rendered}");
    assert!(
        !rendered.contains("rtao_weights_owner_row_total"),
        "{rendered}"
    );

    // A held row reserves no cadence slot: the opted-in proposal commits at
    // the next head.
    until_authored_with(&mut scheduler, &lab, &cx, |_| {
        (WeightsProposal::Set(burn()), OwnerRowPolicy::Submit)
    })
    .await?;
    assert_eq!(lab.authors(), 1);
    assert!(submitted(&scheduler), "{:?}", outcome(&scheduler));
    assert_eq!(tails.lock().unwrap()[0].2, 4, "a timelocked commit, tag 4");
    assert_eq!(scheduler.reveals.pending(), 1);
    assert_eq!(pending_rows(&scheduler.reveals), vec![burn_row().0]);
    let rendered = cx.metrics.render();
    assert!(
        rendered.contains("rtao_weights_owner_row_total 1"),
        "{rendered}"
    );
    assert!(
        rendered.contains("rtao_weights_submitted_total 1"),
        "{rendered}"
    );

    // Positive weight on this non-owner validator's own UID keeps holding.
    for row in [by_uid(&[(0, 1.0)]), by_uid(&[(0, 1.0), (2, 1.0)])] {
        let mut fresh = memory()?;
        drive(&mut fresh, &lab, &cx, 12, |_| {
            (WeightsProposal::Set(row.clone()), OwnerRowPolicy::Submit)
        })
        .await?;
        assert_eq!(outcome(&fresh), WeightAttemptOutcome::HoldPrevious);
        assert_eq!(lab.authors(), 1, "{row:?}");
    }

    // When this validator is the owner hotkey, its own row is the owner row.
    lab.node.mutate_state(|state| {
        state.seed(
            &super::super::st::SUBNET_OWNER_HOTKEY,
            (NetUid(1),),
            account.into(),
        )
    })?;
    let mut owner = memory()?;
    drive(&mut owner, &lab, &cx, 12, |_| {
        (
            WeightsProposal::Set(by_uid(&[(0, 1.0)])),
            OwnerRowPolicy::Hold,
        )
    })
    .await?;
    assert_eq!(outcome(&owner), WeightAttemptOutcome::HoldPrevious);
    assert_eq!(lab.authors(), 1);
    until_authored_with(&mut owner, &lab, &cx, |_| {
        (
            WeightsProposal::Set(by_uid(&[(0, 1.0)])),
            OwnerRowPolicy::Submit,
        )
    })
    .await?;
    assert_eq!(lab.authors(), 2);
    assert!(submitted(&owner), "{:?}", outcome(&owner));
    assert_eq!(pending_rows(&owner.reveals), vec![vec![Uid(0)]]);
    drop(owner);
    close_cr_lab(scheduler, lab, cx).await;
    Ok(())
}

/// With commit-reveal off, the opted-in burn row is a direct
/// `set_mechanism_weights` carrying exactly the owner UID and `u16::MAX`,
/// which is the row the chain stores in `Weights`.
#[tokio::test]
async fn owner_only_row_sets_direct_weights_when_commit_reveal_is_off() -> anyhow::Result<()> {
    let (lab, cx, _) = owner_lab("direct", |state| {
        state.seed(
            &super::super::st::COMMIT_REVEAL_WEIGHTS_ENABLED,
            (NetUid(1),),
            false,
        )?;
        Ok(())
    })
    .await?;
    let signed = Arc::new(std::sync::Mutex::new(Vec::<Vec<u8>>::new()));
    let seen = signed.clone();
    lab.node.mutate_state(|state| {
        state.set_handler("author_submitAndWatchExtrinsic", move |state, request| {
            let reply = state.dispatch(request);
            seen.lock()
                .unwrap()
                .push(state.submissions.last().expect("submission").clone());
            reply
        })
    });
    let mut scheduler = memory()?;
    drive(&mut scheduler, &lab, &cx, 4, |_| {
        (WeightsProposal::Set(burn()), OwnerRowPolicy::Hold)
    })
    .await?;
    assert_eq!(outcome(&scheduler), WeightAttemptOutcome::HoldPrevious);
    assert_eq!(lab.authors(), 0);
    until_authored_with(&mut scheduler, &lab, &cx, |_| {
        (WeightsProposal::Set(burn()), OwnerRowPolicy::Submit)
    })
    .await?;
    assert_eq!(lab.authors(), 1);
    assert!(submitted(&scheduler), "{:?}", outcome(&scheduler));
    assert_eq!(
        scheduler.reveals.pending(),
        0,
        "a direct row is not committed"
    );
    // `.., netuid 1, mecid 0, dests [2], weights [65535], version_key 0`.
    let tail = [
        &[1u8, 0, 0][..],
        &[4, 2, 0],
        &[4, 0xff, 0xff],
        &0u64.to_le_bytes(),
    ]
    .concat();
    let bytes = signed.lock().unwrap()[0].clone();
    assert!(bytes.ends_with(&tail), "not the exact burn row");
    close_cr_lab(scheduler, lab, cx).await;
    Ok(())
}

/// A burn row queued under `Submit` keeps that policy across a paused
/// restart, and a re-preparation after a crash before signing decides from
/// the journal. The same row queued under `Hold` never signs.
#[tokio::test]
async fn queued_owner_row_policy_survives_restart_and_rebuild() -> anyhow::Result<()> {
    // (a) Queue, pause, restart, resume: the queued policy commits.
    let (mut lab, cx, account) = owner_lab("restart", |_| Ok(())).await?;
    queue_model(&lab, account);
    let mut scheduler = durable(&lab, account)?;
    scheduler.enqueue_with_owner_rows(
        "burn-op",
        Some("app:burn".into()),
        MechId(0),
        burn(),
        OwnerRowPolicy::Submit,
    )?;
    lab.journal().unwrap().control(JournalControl {
        submissions_paused: true,
        allow_obligations: false,
        extensions: Default::default(),
    })?;
    let head = advance(&lab)?;
    Box::pin(scheduler.on_finalized(&cx, &head, |_| panic!("paused work must not score"))).await?;
    assert_eq!(lab.authors(), 0);
    drop(scheduler);
    let cx = restart(&mut lab, cx).await?;
    let mut scheduler = durable(&lab, account)?;
    lab.journal().unwrap().control(JournalControl::default())?;
    for _ in 0..12 {
        let head = advance(&lab)?;
        Box::pin(scheduler.on_finalized(&cx, &head, |_| panic!("queued work must be reused")))
            .await?;
        if lab.authors() > 0 {
            break;
        }
    }
    assert_eq!(lab.authors(), 1, "{:?}", scheduler.last_attempt(MechId(0)));
    assert!(submitted(&scheduler), "{:?}", outcome(&scheduler));
    assert_eq!(scheduler.reveals.pending(), 1);
    assert_eq!(pending_rows(&scheduler.reveals), vec![burn_row().0]);
    let record = lab.journal().unwrap().bind("burn-op")?.record()?;
    assert_eq!(record.intent.effects["owner_rows"], "submit");
    assert_eq!(record.intent.correlation_id.as_deref(), Some("app:burn"));
    let rows = attempt_rows(&record);
    assert!(!rows.is_empty());
    assert!(rows.iter().all(|row| *row == burn_row()), "{rows:?}");
    close_cr_lab(scheduler, lab, cx).await;

    // (b) Material persisted, then a crash before signing: recovery finds the
    // work rebuildable and the next head re-prepares a fresh commitment from
    // the journaled work and policy, on the same operation ID.
    let (mut lab, cx, account) = owner_lab("rebuild", |_| Ok(())).await?;
    queue_model(&lab, account);
    let scheduler = durable(&lab, account)?;
    scheduler.enqueue_with_owner_rows(
        "burn-rebuild",
        None,
        MechId(0),
        burn(),
        OwnerRowPolicy::Submit,
    )?;
    let best = lab.client.at(BlockRef::Best).await?;
    let preflight = WeightPreflight::fetch(&best, NetUid(1), MechId(0), account).await?;
    let stale = PreparedWeightCommit {
        ciphertext: vec![4, 5, 6],
        reveal_round: 500,
    };
    let binding = scheduler
        .prepare_binding(MechId(0), burn_row(), Some(stale), &preflight, None)?
        .expect("journal binding");
    drop(binding);
    drop(best);
    drop(scheduler);
    let cx = restart(&mut lab, cx).await?;
    let mut scheduler = durable(&lab, account)?;
    let (binding, work) = scheduler.pending_work(MechId(0))?.expect("retained work");
    assert_eq!(work.owner_rows, OwnerRowPolicy::Submit);
    assert!(matches!(
        binding.recover(&lab.client, 128).await?,
        rtao_client::journal::RecoveryStatus::Rebuildable
    ));
    drop(binding);
    for _ in 0..12 {
        let head = advance(&lab)?;
        Box::pin(scheduler.on_finalized(&cx, &head, |_| panic!("rebuildable work must be reused")))
            .await?;
        if lab.authors() > 0 {
            break;
        }
    }
    assert_eq!(lab.authors(), 1, "{:?}", scheduler.last_attempt(MechId(0)));
    assert!(submitted(&scheduler), "{:?}", outcome(&scheduler));
    let record = lab.journal().unwrap().bind("burn-rebuild")?.record()?;
    let (ciphertext, _): (Vec<u8>, u64) =
        serde_json::from_value(record.material["commit"].clone())?;
    assert_ne!(ciphertext, vec![4, 5, 6], "a fresh commitment");
    let rows = attempt_rows(&record);
    assert!(!rows.is_empty());
    assert!(rows.iter().all(|row| *row == burn_row()), "{rows:?}");
    close_cr_lab(scheduler, lab, cx).await;

    // (c) Control: the same row queued under Hold holds on every head.
    let (lab, cx, account) = owner_lab("queued-hold", |_| Ok(())).await?;
    let mut scheduler = durable(&lab, account)?;
    scheduler.enqueue("burn-held", None, MechId(0), burn())?;
    for _ in 0..12 {
        let head = advance(&lab)?;
        Box::pin(scheduler.on_finalized(&cx, &head, |_| panic!("queued work must be reused")))
            .await?;
    }
    assert_eq!(outcome(&scheduler), WeightAttemptOutcome::HoldPrevious);
    assert_eq!(lab.authors(), 0);
    assert!(scheduler.pending_work(MechId(0))?.is_some());
    close_cr_lab(scheduler, lab, cx).await;
    Ok(())
}

/// An operation an earlier release journaled without the policy key keeps
/// holding the burn row after restart, and rejoins idempotently.
#[tokio::test]
async fn legacy_journal_owner_row_keeps_holding() -> anyhow::Result<()> {
    let (mut lab, cx, account) = owner_lab("legacy", |_| Ok(())).await?;
    let scheduler = durable(&lab, account)?;
    let scope = scheduler.durable.as_ref().unwrap().scope.clone();
    lab.journal().unwrap().begin(OperationIntent {
        id: "legacy-burn".into(),
        correlation_id: None,
        genesis: lab.client.genesis(),
        account,
        scope,
        effects: json!({"netuid":1,"mechanism":0,"input":burn(),"version_key":0}),
        obligation: false,
        semantic_policy: None,
        extensions: Default::default(),
    })?;
    scheduler.enqueue("legacy-burn", None, MechId(0), burn())?;
    assert!(
        scheduler
            .enqueue_with_owner_rows(
                "legacy-burn",
                None,
                MechId(0),
                burn(),
                OwnerRowPolicy::Submit
            )
            .is_err()
    );
    drop(scheduler);
    let cx = restart(&mut lab, cx).await?;
    let mut scheduler = durable(&lab, account)?;
    for _ in 0..12 {
        let head = advance(&lab)?;
        Box::pin(scheduler.on_finalized_with(&cx, &head, |_| panic!("queued work must be reused")))
            .await?;
    }
    assert_eq!(outcome(&scheduler), WeightAttemptOutcome::HoldPrevious);
    assert_eq!(lab.authors(), 0);
    let record = lab.journal().unwrap().bind("legacy-burn")?.record()?;
    assert!(record.intent.effects.get("owner_rows").is_none());
    assert!(record.attempts.is_empty());
    close_cr_lab(scheduler, lab, cx).await;
    Ok(())
}

/// A burn commitment finalized before the scheduler handed it to reveal
/// tracking recovers its exact material once after restart.
#[tokio::test]
async fn crash_after_finalized_owner_row_commit_recovers_reveal_tracking_once() -> anyhow::Result<()>
{
    use rtao_client::SubmitOptions;
    use rtao_runtime::calls::subtensor_module as calls;
    use rtao_signer::{KeypairSigner, Signer};
    let (chain, client) = super::super::tests::fixture().await?;
    let signer = KeypairSigner::from_seed(&[7; 32], rtao_primitives::CryptoType::Ed25519)?;
    let hotkey = signer.account();
    let root = std::path::PathBuf::from(
        std::env::var_os("RTAO_TEST_TMPDIR").ok_or_else(|| anyhow::anyhow!("task scratch"))?,
    )
    .join(format!("owner-row-handoff-{}", std::process::id()));
    std::fs::create_dir_all(&root)?;
    let journal = OperationJournal::open(root.join("journal.json"))?;
    let make = || {
        WeightScheduler::new(
            WeightPolicy::default(),
            Arc::new(ChainClock::new(1_000_000)),
            ClockGuard::default(),
        )
    };
    let scheduler = make()?.with_journal(journal.clone(), client.genesis(), NetUid(1), hotkey)?;
    scheduler.enqueue_with_owner_rows(
        "burn-commit",
        Some("app:burn".into()),
        MechId(0),
        burn(),
        OwnerRowPolicy::Submit,
    )?;
    let best = client.at(BlockRef::Best).await?;
    let preparation =
        WeightPreflight::fetch(&best, NetUid(1), MechId(0), super::super::tests::HOTKEY).await?;
    let commit = PreparedWeightCommit {
        ciphertext: vec![4, 5, 6],
        reveal_round: 500,
    };
    let binding = scheduler
        .prepare_binding(
            MechId(0),
            burn_row(),
            Some(commit.clone()),
            &preparation,
            None,
        )?
        .ok_or_else(|| anyhow::anyhow!("binding"))?;
    let retained = commit.clone();
    chain.mutate_state(|state| {
        state.set_handler("author_submitAndWatchExtrinsic", move |state, request| {
            let reply = state.dispatch(request);
            let number = state.block;
            let result = super::super::tests::seed_at(
                state,
                number,
                "SubtensorModule",
                "TimelockedWeightCommits",
                (NetUidStorageIndex(1), 5u64),
                vec![(
                    hotkey,
                    number,
                    retained.ciphertext.clone(),
                    retained.reveal_round,
                )],
            );
            assert!(result.is_ok());
            reply
        })
    });
    let call = calls::commit_timelocked_mechanism_weights(
        NetUid(1),
        MechId(0),
        commit.ciphertext,
        commit.reveal_round,
        4,
    )
    .compose(&client.refresh_runtime().await?)?;
    let report = client
        .submit(
            call,
            &signer,
            &SubmitOptions {
                journal: Some(binding.clone()),
                ..Default::default()
            },
        )
        .await?;
    assert!(report.success);
    drop(binding);
    drop(scheduler);
    drop(journal);
    let journal = OperationJournal::open(root.join("journal.json"))?;
    journal.control(JournalControl {
        submissions_paused: true,
        allow_obligations: false,
        extensions: Default::default(),
    })?;
    let mut restored =
        make()?.with_journal(journal.clone(), client.genesis(), NetUid(1), hotkey)?;
    let number = report
        .block_number
        .ok_or_else(|| anyhow::anyhow!("inclusion height"))?;
    chain.mutate_state(|state| state.set_finalized(number))?;
    let head = super::super::tests::header(&chain, number)?;
    for _ in 0..2 {
        restored
            .recover_finalized(&client, &head, &Metrics::default())
            .await?;
        assert_eq!(restored.reveals.pending(), 1);
        assert_eq!(pending_rows(&restored.reveals), vec![burn_row().0]);
        assert!(restored.pending_work(MechId(0))?.is_none());
    }
    assert_eq!(chain.mutate_state(|state| state.submissions.len()), 1);
    let record = journal.bind("burn-commit")?.record()?;
    assert_eq!(record.intent.effects["owner_rows"], "submit");
    assert_eq!(record.intent.correlation_id.as_deref(), Some("app:burn"));
    drop(restored);
    drop(journal);
    client.close().await;
    std::fs::remove_dir_all(root)?;
    Ok(())
}

/// Without a journal binding, a commit rejected for its tag is re-prepared
/// from fresh state under the same policy. If the retry ignored the policy,
/// the burn row would hold there: one author and `HoldPrevious`.
#[tokio::test]
async fn rejected_owner_row_commit_reprepares_under_the_same_policy() -> anyhow::Result<()> {
    let (cr_pallet, cr_error) = module_error("IncorrectCommitRevealVersion");
    let rejection = json!({"phase":"ApplyExtrinsic","extrinsic_idx":0,"module_id":"System","event_id":"ExtrinsicFailed","attributes":{"dispatch_error":{"Module":{"index":cr_pallet,"error":[cr_error,0,0,0]}},"dispatch_info":{"weight":{"ref_time":1,"proof_size":0},"class":"Normal","pays_fee":"No"}},"topics":[]});
    let (lab, cx, account) = owner_lab("reprepare", |_| Ok(())).await?;
    let mut scheduler = memory()?;
    scheduler.reveals.remember_revealed_code(H256([42; 32]), 5);
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
    until_authored_with(&mut scheduler, &lab, &cx, |_| {
        (WeightsProposal::Set(burn()), OwnerRowPolicy::Submit)
    })
    .await?;
    let tails = tails.lock().unwrap().clone();
    assert_eq!(lab.authors(), 2, "{:?}", scheduler.last_attempt(MechId(0)));
    assert_eq!(tails[0].2, 4);
    assert_eq!(tails[1].2, 5, "the retry carries the newly selected tag");
    assert_ne!(tails[1].0, tails[0].0, "the retry is a fresh commitment");
    assert!(submitted(&scheduler), "{:?}", outcome(&scheduler));
    assert_eq!(scheduler.reveals.pending(), 1);
    assert_eq!(pending_rows(&scheduler.reveals), vec![burn_row().0]);
    let rendered = cx.metrics.render();
    assert!(
        rendered.contains("rtao_weights_cr_version_recomputed_total 1"),
        "{rendered}"
    );
    close_cr_lab(scheduler, lab, cx).await;
    Ok(())
}

/// A journal-bound burn commit rejected for its tag is signed once (the
/// guard is unlocked under a journal), then the slot goes uncertain and later
/// heads never re-sign it.
#[tokio::test]
async fn journal_bound_owner_row_rejection_goes_uncertain_without_resigning() -> anyhow::Result<()>
{
    let (cr_pallet, cr_error) = module_error("IncorrectCommitRevealVersion");
    let (lab, cx, account) = owner_lab("reject-journal", |_| Ok(())).await?;
    let mut scheduler = durable(&lab, account)?;
    scheduler.reveals.remember_revealed_code(H256([42; 32]), 5);
    lab.script([Inner::Fail(cr_pallet, cr_error)]);
    lab.after_inclusion(|state, _record, _number| {
        state
            .seed(&super::super::st::COMMIT_REVEAL_WEIGHTS_VERSION, (), 5u16)
            .expect("stored tag");
    });
    let burn_now = |_: MechId| (WeightsProposal::Set(burn()), OwnerRowPolicy::Submit);
    until_authored_with(&mut scheduler, &lab, &cx, burn_now).await?;
    assert_eq!(lab.authors(), 1, "{:?}", scheduler.last_attempt(MechId(0)));
    let rendered = format!("{:?}", outcome(&scheduler));
    assert!(
        rendered.contains("Incorrect commit-reveal version"),
        "{rendered}"
    );
    assert!(scheduler.slots[&MechId(0)].uncertain);
    let (_, work) = scheduler.pending_work(MechId(0))?.expect("retained work");
    assert_eq!(work.owner_rows, OwnerRowPolicy::Submit);
    drive(&mut scheduler, &lab, &cx, 2, burn_now).await?;
    assert_eq!(lab.authors(), 1);
    let rendered = cx.metrics.render();
    assert!(
        !rendered.contains("rtao_weights_submitted_total"),
        "{rendered}"
    );
    close_cr_lab(scheduler, lab, cx).await;
    Ok(())
}

/// A durable callback operation journals its policy; redelivering the same
/// ID under another policy is refused without signing.
#[tokio::test]
async fn durable_callback_operation_journals_its_policy() -> anyhow::Result<()> {
    let (lab, cx, account) = owner_lab("callback", |_| Ok(())).await?;
    queue_model(&lab, account);
    let mut scheduler = durable(&lab, account)?;
    let operation = || WeightsProposal::Operation {
        id: "burn-callback".into(),
        correlation_id: Some("app:callback".into()),
        input: burn(),
    };
    until_authored_with(&mut scheduler, &lab, &cx, |_| {
        (operation(), OwnerRowPolicy::Submit)
    })
    .await?;
    assert_eq!(lab.authors(), 1);
    assert!(submitted(&scheduler), "{:?}", outcome(&scheduler));
    assert!(scheduler.pending_work(MechId(0))?.is_none());
    let record = lab.journal().unwrap().bind("burn-callback")?.record()?;
    assert_eq!(record.intent.effects["owner_rows"], "submit");
    // Open the next epoch, then redeliver the completed ID.
    lab.node.mutate_state(|state| -> anyhow::Result<()> {
        let block = state.block;
        state.seed(&super::super::st::SUBNET_EPOCH_INDEX, (NetUid(1),), 6u64)?;
        state.seed(&super::super::st::LAST_EPOCH_BLOCK, (NetUid(1),), block)?;
        Ok(())
    })?;
    drive(&mut scheduler, &lab, &cx, 8, |_| {
        (operation(), OwnerRowPolicy::Hold)
    })
    .await?;
    let WeightAttemptOutcome::Failed(error) = outcome(&scheduler) else {
        panic!("expected an identity refusal: {:?}", outcome(&scheduler))
    };
    assert!(
        error.contains("journal operation identity differs"),
        "{error}"
    );
    drive(&mut scheduler, &lab, &cx, 2, |_| {
        (operation(), OwnerRowPolicy::Submit)
    })
    .await?;
    assert_eq!(lab.authors(), 1, "a completed operation is never re-signed");
    close_cr_lab(scheduler, lab, cx).await;
    Ok(())
}
