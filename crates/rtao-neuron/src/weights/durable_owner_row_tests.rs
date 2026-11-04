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

/// The `OwnerRowHeld` refusal queued work reports when the guard holds it.
fn owner_row_held(scheduler: &WeightScheduler) -> super::super::WeightRefusal {
    match outcome(scheduler) {
        WeightAttemptOutcome::Refused(refusal) if refusal.name == "OwnerRowHeld" => {
            assert_eq!(refusal.code, rtao_catalog::ErrorCode::PolicyViolation);
            refusal
        }
        other => panic!("expected OwnerRowHeld: {other:?}"),
    }
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
    queue_model_in(lab, account, Arc::new(std::sync::atomic::AtomicU64::new(5)));
}

/// [`queue_model`] into the queue of the epoch `epoch` holds at inclusion.
fn queue_model_in(lab: &JournalLab, account: AccountId, epoch: Arc<std::sync::atomic::AtomicU64>) {
    lab.after_inclusion(move |state, record, number| {
        let epoch = epoch.load(std::sync::atomic::Ordering::SeqCst);
        let (ciphertext, round): (Vec<u8>, u64) =
            serde_json::from_value(record.material["commit"].clone()).expect("commit material");
        super::super::tests::seed_at(
            state,
            number,
            "SubtensorModule",
            "TimelockedWeightCommits",
            (NetUidStorageIndex(1), epoch),
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
    // Queued work is re-proposed without the callback, so the hold names
    // the operation and the way out (D-098).
    let refusal = owner_row_held(&scheduler);
    assert!(refusal.remediation.contains("burn-held"), "{refusal:?}");
    assert!(
        refusal.remediation.contains("OwnerRowPolicy::Submit"),
        "{refusal:?}"
    );
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
    assert!(
        owner_row_held(&scheduler)
            .remediation
            .contains("legacy-burn")
    );
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
    // The propose site and the re-preparation each count the unlocked row.
    assert!(
        rendered.contains("rtao_weights_owner_row_total 2"),
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

/// A new durable callback row the guard holds is not journaled, so the
/// callback is consulted again at the next head and a later ordinary row is
/// signed. Journaling the held row would re-propose it without the callback
/// on every later head and never sign anything on the mechanism.
#[tokio::test]
async fn held_callback_row_is_not_journaled() -> anyhow::Result<()> {
    let (lab, cx, account) = owner_lab("held-callback", |_| Ok(())).await?;
    queue_model(&lab, account);
    let mut scheduler = durable(&lab, account)?;
    let calls = std::sync::atomic::AtomicUsize::new(0);
    let held_rows = [
        // Self-only: held even under Submit.
        (by_uid(&[(0, 1.0)]), OwnerRowPolicy::Submit),
        // An owner burn row under the default.
        (burn(), OwnerRowPolicy::Hold),
    ];
    let proposals = |_: MechId| {
        let n = calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        match held_rows.get(n) {
            Some((input, policy)) => (WeightsProposal::Set(input.clone()), *policy),
            None => (
                WeightsProposal::Set(by_uid(&[(1, 1.0)])),
                OwnerRowPolicy::Hold,
            ),
        }
    };
    until_authored_with(&mut scheduler, &lab, &cx, proposals).await?;
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 3);
    assert_eq!(lab.authors(), 1);
    assert!(submitted(&scheduler), "{:?}", outcome(&scheduler));
    let operations = lab.journal().unwrap().snapshot()?.operations;
    assert_eq!(operations.len(), 1, "only the signed row is journaled");
    let record = operations.values().next().unwrap();
    assert_eq!(
        record.intent.effects["input"],
        serde_json::to_value(by_uid(&[(1, 1.0)]))?
    );
    assert!(record.intent.effects.get("owner_rows").is_none());
    let rendered = cx.metrics.render();
    assert!(rendered.contains("rtao_weights_hold_total 2"), "{rendered}");
    close_cr_lab(scheduler, lab, cx).await;
    Ok(())
}

/// Feed finalized heads with queued work only until the scheduler authors
/// once more, at most one lab epoch of heads.
async fn until_queued_authored(
    scheduler: &mut WeightScheduler,
    lab: &JournalLab,
    cx: &NeuronCx,
) -> anyhow::Result<()> {
    until_authored_with(scheduler, lab, cx, |_| panic!("queued work must be reused")).await
}

/// Open the subnet epoch `index` at the lab's current block; `queue` follows.
fn next_epoch(
    lab: &JournalLab,
    queue: &std::sync::atomic::AtomicU64,
    index: u64,
) -> anyhow::Result<()> {
    queue.store(index, std::sync::atomic::Ordering::SeqCst);
    lab.node.mutate_state(|state| -> anyhow::Result<()> {
        let block = state.block;
        state.seed(&super::super::st::SUBNET_EPOCH_INDEX, (NetUid(1),), index)?;
        state.seed(&super::super::st::LAST_EPOCH_BLOCK, (NetUid(1),), block)?;
        Ok(())
    })
}

/// Register `hotkeys[i]` at UID `i` on chain and in the runtime API, as a
/// live metagraph refresh would see it.
fn reregister(lab: &JournalLab, hotkeys: [AccountId; 3]) -> anyhow::Result<()> {
    lab.node.mutate_state(|state| -> anyhow::Result<()> {
        for (uid, hotkey) in (0u16..).zip(hotkeys) {
            state.seed_typed("SubtensorModule", "Uids", (NetUid(1), hotkey), uid)?;
        }
        let names: Vec<String> = hotkeys.iter().map(ToString::to_string).collect();
        state.seed_runtime(
            "SubnetInfoRuntimeApi",
            "get_selective_mechagraph",
            json!({"netuid":1,"block":100,"tempo":10,"last_step":100,"blocks_since_last_step":0,"num_uids":3,"hotkeys":names}),
        )?;
        Ok(())
    })
}

/// The owner hotkey's UID changes after the application's one metagraph
/// snapshot (its refresh worker stopped, as a validator on a paced endpoint
/// does): the owner `[9; 32]` re-registers at UID 1 and a miner `[10; 32]`
/// takes UID 2, which the snapshot still maps to the owner (D-098).
///
/// Earlier releases mapped destinations through that snapshot: a queued row
/// for the miner at UID 2 held as an owner row on every head and its
/// operation stayed pending for good, while a queued `Hold` row for UID 1,
/// the owner's at the dispatch block, was signed as an ordinary row. The
/// guard now reads the owner's UID at the dispatch block; held queued work
/// reports `OwnerRowHeld` and `retire_pending` frees the mechanism.
#[tokio::test]
async fn owner_uid_change_after_the_snapshot_is_decided_at_the_dispatch_block() -> anyhow::Result<()>
{
    let (lab, cx, account) = owner_lab("owner-moved", |_| Ok(())).await?;
    let queue = Arc::new(std::sync::atomic::AtomicU64::new(5));
    queue_model_in(&lab, account, queue.clone());
    let (owner, miner) = (AccountId([9; 32]), AccountId([10; 32]));
    cx.metagraph.shutdown().await;
    assert_eq!(cx.metagraph.load().hotkey(Uid(2)), Some(&owner));
    reregister(&lab, [account, owner, miner])?;
    let mut scheduler = durable(&lab, account)?;

    // (1) UID 2 is a miner at the dispatch block: the row is committable.
    scheduler.enqueue("miner-row", None, MechId(0), by_uid(&[(2, 1.0)]))?;
    until_queued_authored(&mut scheduler, &lab, &cx).await?;
    assert!(submitted(&scheduler), "{:?}", outcome(&scheduler));
    assert_eq!(lab.authors(), 1);
    assert!(scheduler.pending_work(MechId(0))?.is_none());
    assert_eq!(pending_rows(&scheduler.reveals), vec![vec![Uid(2)]]);

    // (2) UID 1 is the owner's at the dispatch block: a queued Hold row
    // holds, names the operation, and stays pending.
    next_epoch(&lab, &queue, 6)?;
    let burn_at_one = by_uid(&[(0, 0.0), (1, 65535.0), (2, 0.0)]);
    scheduler.enqueue("owner-row", None, MechId(0), burn_at_one.clone())?;
    // Past the chain rate limit of the first row.
    drive(&mut scheduler, &lab, &cx, 12, |_| {
        panic!("queued work must be reused")
    })
    .await?;
    let refusal = owner_row_held(&scheduler);
    for part in [
        "owner-row",
        "the subnet owner hotkey's UID 1",
        "retire_pending",
    ] {
        assert!(refusal.remediation.contains(part), "{part}: {refusal:?}");
    }
    assert_eq!(lab.authors(), 1, "a held row signs nothing");
    assert!(scheduler.pending_work(MechId(0))?.is_some());
    let rendered = cx.metrics.render();
    assert!(rendered.contains("rtao_weights_hold_total"), "{rendered}");

    // Retiring it frees the mechanism; the ID stays idempotent.
    assert!(scheduler.retire_pending(MechId(0), "miner-row").is_err());
    scheduler.retire_pending(MechId(0), "owner-row")?;
    assert!(scheduler.pending_work(MechId(0))?.is_none());
    let record = lab.journal().unwrap().bind("owner-row")?.record()?;
    assert_eq!(record.extensions["scheduler_retired"], true);
    assert!(record.attempts.is_empty());
    scheduler.enqueue("owner-row", None, MechId(0), burn_at_one)?;
    assert!(scheduler.pending_work(MechId(0))?.is_none());
    assert!(scheduler.retire_pending(MechId(0), "owner-row").is_err());

    // (3) The same epoch accepts a replacement under a new ID.
    scheduler.enqueue_with_owner_rows(
        "owner-burn",
        None,
        MechId(0),
        by_uid(&[(1, 65535.0)]),
        OwnerRowPolicy::Submit,
    )?;
    until_queued_authored(&mut scheduler, &lab, &cx).await?;
    assert!(submitted(&scheduler), "{:?}", outcome(&scheduler));
    assert_eq!(lab.authors(), 2);
    assert!(pending_rows(&scheduler.reveals).contains(&vec![Uid(1)]));
    close_cr_lab(scheduler, lab, cx).await;
    Ok(())
}

/// This validator's own UID changes after the snapshot: it re-registers at
/// UID 1, replacing the miner `[8; 32]`, and `[11; 32]` takes UID 0. The
/// guard uses the dispatch block's UID for this validator, so a queued
/// `Submit` row for UID 1 plus the owner's UID 2 is now self-plus-owner and
/// holds with `OwnerRowHeld` instead of a silent `HoldPrevious`, and a
/// queued row for UID 0 (no longer this validator) is committable (D-098).
#[tokio::test]
async fn own_uid_change_after_the_snapshot_is_decided_at_the_dispatch_block() -> anyhow::Result<()>
{
    let (lab, cx, account) = owner_lab("own-moved", |_| Ok(())).await?;
    queue_model(&lab, account);
    let owner = AccountId([9; 32]);
    cx.metagraph.shutdown().await;
    reregister(&lab, [AccountId([11; 32]), account, owner])?;
    let mut scheduler = durable(&lab, account)?;

    scheduler.enqueue_with_owner_rows(
        "self-and-owner",
        None,
        MechId(0),
        by_uid(&[(1, 1.0), (2, 1.0)]),
        OwnerRowPolicy::Submit,
    )?;
    drive(&mut scheduler, &lab, &cx, 4, |_| {
        panic!("queued work must be reused")
    })
    .await?;
    let refusal = owner_row_held(&scheduler);
    assert!(
        refusal.remediation.contains("this validator's UID 1"),
        "{refusal:?}"
    );
    assert_eq!(lab.authors(), 0);
    scheduler.retire_pending(MechId(0), "self-and-owner")?;

    scheduler.enqueue("old-self", None, MechId(0), by_uid(&[(0, 1.0)]))?;
    until_queued_authored(&mut scheduler, &lab, &cx).await?;
    assert!(submitted(&scheduler), "{:?}", outcome(&scheduler));
    assert_eq!(pending_rows(&scheduler.reveals), vec![vec![Uid(0)]]);
    close_cr_lab(scheduler, lab, cx).await;
    Ok(())
}

/// A queued row's weight version below the chain's `WeightsVersionKey` is
/// refused before anything is signed, as `IncorrectWeightVersionKey`
/// (`invalid_argument`) carrying the observed key and block. The version is
/// part of the queued work's immutable effects, so raising the policy does
/// not rescue it; `retire_pending` frees the mechanism for a replacement
/// queued under the new version (D-098).
#[tokio::test]
async fn queued_version_below_weights_version_key_is_refused_before_signing() -> anyhow::Result<()>
{
    let (lab, cx, account) = owner_lab("version-key", |_| Ok(())).await?;
    queue_model(&lab, account);
    let mut scheduler = durable(&lab, account)?;
    scheduler.enqueue("old-version", None, MechId(0), by_uid(&[(1, 1.0)]))?;
    lab.node.mutate_state(|state| {
        state.seed(&super::super::st::WEIGHTS_VERSION_KEY, (NetUid(1),), 7u64)
    })?;
    drive(&mut scheduler, &lab, &cx, 4, |_| {
        panic!("queued work must be reused")
    })
    .await?;
    let refused = |scheduler: &WeightScheduler| match outcome(scheduler) {
        WeightAttemptOutcome::Refused(refusal) => refusal,
        other => panic!("expected a refusal: {other:?}"),
    };
    let refusal = refused(&scheduler);
    assert_eq!(refusal.name, "IncorrectWeightVersionKey");
    assert_eq!(refusal.code, rtao_catalog::ErrorCode::InvalidArgument);
    for part in [
        "requires weight version 7 (WeightsVersionKey at block",
        "this attempt carries 0",
        "queued operation old-version carries weight version 0",
        "retire_pending",
    ] {
        assert!(refusal.remediation.contains(part), "{part}: {refusal:?}");
    }
    assert_eq!(lab.authors(), 0, "nothing is signed");

    // Raising the policy does not change queued work.
    scheduler.update_policy(WeightPolicy {
        version_key: super::super::VersionKey::Fixed(7),
        ..policy()
    });
    drive(&mut scheduler, &lab, &cx, 2, |_| {
        panic!("queued work must be reused")
    })
    .await?;
    assert_eq!(refused(&scheduler).name, "IncorrectWeightVersionKey");
    assert_eq!(lab.authors(), 0);

    scheduler.retire_pending(MechId(0), "old-version")?;
    scheduler.enqueue("new-version", None, MechId(0), by_uid(&[(1, 1.0)]))?;
    until_queued_authored(&mut scheduler, &lab, &cx).await?;
    assert!(submitted(&scheduler), "{:?}", outcome(&scheduler));
    let record = lab.journal().unwrap().bind("new-version")?.record()?;
    assert_eq!(record.intent.effects["version_key"], 7);
    close_cr_lab(scheduler, lab, cx).await;
    Ok(())
}

/// `retire_pending` never drops work a signed attempt may have reached the
/// chain with: an uncertain slot refuses it, and so does an attempt the
/// journal has not proven unexecuted even once the slot is no longer
/// uncertain (D-098).
#[tokio::test]
async fn retire_pending_refuses_work_with_an_unresolved_signed_attempt() -> anyhow::Result<()> {
    let (cr_pallet, cr_error) = module_error("IncorrectCommitRevealVersion");
    let (lab, cx, account) = owner_lab("retire-unresolved", |_| Ok(())).await?;
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
    assert_eq!(lab.authors(), 1);
    assert!(scheduler.slots[&MechId(0)].uncertain);
    let (binding, _) = scheduler.pending_work(MechId(0))?.expect("retained work");
    let id = binding.id().to_owned();
    let error = scheduler.retire_pending(MechId(0), &id).unwrap_err();
    assert!(error.to_string().contains("uncertain"), "{error:#}");
    // Even with the slot cleared, the journal's attempt decides.
    scheduler.slots.get_mut(&MechId(0)).unwrap().uncertain = false;
    let record = binding.record()?;
    assert!(!record.attempts.is_empty());
    assert!(
        record
            .attempts
            .iter()
            .any(|a| a.outcome != rtao_client::journal::AttemptOutcome::ExpiredUnexecuted)
    );
    let error = scheduler.retire_pending(MechId(0), &id).unwrap_err();
    assert!(
        format!("{error:#}").contains("not proven unexecuted"),
        "{error:#}"
    );
    assert!(scheduler.pending_work(MechId(0))?.is_some());
    let record = binding.record()?;
    assert!(!record.extensions.contains_key("scheduler_retired"));
    close_cr_lab(scheduler, lab, cx).await;
    Ok(())
}
