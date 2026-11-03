//! A weight dispatch that provably signed nothing gives its cadence slot
//! back, while one that may have signed keeps it, in memory, in the journal
//! and across a restart (D-095).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use super::super::{WeightAttemptOutcome, WeightOpportunity};
use super::clock_tests::{NOW_MS, memory, timed_lab};
use super::public_tests::{advance, close_cr_lab, module_error, plain_author, scored};
use super::*;
use crate::{ChainClock, ClockGuard};
use rtao_client::BlockRef;
use rtao_testkit::journal_shield::JournalLab;
use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};

fn durable(lab: &JournalLab, account: AccountId) -> anyhow::Result<WeightScheduler> {
    memory(Arc::new(ChainClock::new(NOW_MS)), ClockGuard::default())?.with_journal(
        lab.journal().unwrap().clone(),
        lab.client.genesis(),
        NetUid(1),
        account,
    )
}

/// Fail every `account_nextIndex` read while `refuse` is set: the executor
/// needs the nonce before it can sign, so these attempts sign nothing.
fn refuse_nonces(lab: &JournalLab) -> Arc<AtomicBool> {
    let refuse = Arc::new(AtomicBool::new(true));
    let refusing = refuse.clone();
    lab.node.mutate_state(|state| {
        state.set_handler("account_nextIndex", move |state, request| {
            if refusing.load(Ordering::SeqCst) {
                rtao_testkit::Reply::error(-32000, "synthetic endpoint refusal", None)
            } else {
                state.dispatch(request)
            }
        })
    });
    refuse
}

fn nonce_reads(lab: &JournalLab) -> usize {
    lab.node
        .requests()
        .iter()
        .filter(|request| request.method == "account_nextIndex")
        .count()
}

/// Feed finalized heads until an attempt fails; return its best block.
async fn until_failed(
    scheduler: &mut WeightScheduler,
    lab: &JournalLab,
    cx: &crate::NeuronCx,
) -> anyhow::Result<u64> {
    for _ in 0..12 {
        let head = advance(lab)?;
        Box::pin(scheduler.on_finalized(cx, &head, scored)).await?;
        if let Some(attempt) = scheduler.last_attempt(MechId(0))
            && matches!(
                attempt.outcome,
                WeightAttemptOutcome::Failed(_) | WeightAttemptOutcome::Unresolved(_)
            )
        {
            return Ok(attempt.best_block);
        }
    }
    anyhow::bail!("no attempt failed: {:?}", scheduler.last_attempt(MechId(0)))
}

async fn head(lab: &JournalLab, cx: &crate::NeuronCx, scheduler: &mut WeightScheduler) {
    let head = advance(lab).unwrap();
    Box::pin(scheduler.on_finalized(cx, &head, scored))
        .await
        .unwrap();
}

/// The scheduler's own checkpoint of mechanism zero's slot.
fn checkpointed(lab: &JournalLab, account: AccountId) -> serde_json::Value {
    let snapshot = lab.journal().unwrap().snapshot().unwrap();
    let scope = format!(
        "weights:{}:{}:{account}",
        hex::encode(lab.client.genesis().0),
        NetUid(1)
    );
    snapshot.checkpoints[&scope]["slots"]["0"].clone()
}

/// The only pending weight operation's signed attempts.
fn journal_attempts(lab: &JournalLab) -> usize {
    let snapshot = lab.journal().unwrap().snapshot().unwrap();
    let mut records = snapshot.operations.values();
    let record = records.next().expect("one operation");
    assert!(records.next().is_none());
    record.attempts.len()
}

/// The context after [`JournalLab::restart`]: the same hotkey, metagraph
/// and metrics over the restarted client.
fn restarted(lab: &JournalLab, cx: &crate::NeuronCx) -> crate::NeuronCx {
    crate::NeuronCx {
        executor: rtao_tx::Executor::new(lab.client.clone()),
        client: lab.client.clone(),
        ..cx.clone()
    }
}

/// A failure before signing (here the endpoint refuses the nonce read, as
/// an admission timeout would) returns the epoch's slot instead of
/// consuming it, so the same epoch commits once the endpoint recovers.
/// Repeated failures back off one, two, then four blocks. Before D-095 the
/// first failure marked the in-memory slot uncertain for good.
#[tokio::test]
async fn a_failure_before_signing_returns_the_epoch_slot() -> anyhow::Result<()> {
    let (lab, cx, account) = timed_lab("unsigned-memory", NOW_MS).await?;
    let mut scheduler = memory(Arc::new(ChainClock::new(NOW_MS)), ClockGuard::default())?;
    plain_author(&lab, account, |_, _| {});
    let refuse = refuse_nonces(&lab);
    let failed = until_failed(&mut scheduler, &lab, &cx).await?;
    assert_eq!(lab.authors(), 0);
    let slot = &scheduler.slots[&MechId(0)];
    assert_eq!(slot.last_attempt_epoch, None, "the epoch is not consumed");
    assert_eq!(slot.last_attempt_block, None);
    assert!(
        !slot.uncertain,
        "nothing was signed, so nothing to reconcile"
    );
    assert_eq!(slot.unsigned_retry, Some((failed + 1, 1)));
    let snapshot = lab.client.at(BlockRef::Best).await?;
    let state = WeightPreflight::fetch(&snapshot, NetUid(1), MechId(0), account).await?;
    assert_eq!(state.at.number, failed);
    assert_eq!(
        scheduler.next_action(MechId(0), account, &state)?,
        WeightOpportunity::AwaitBlock {
            not_before_block: failed + 1
        }
    );

    // The next head retries; it fails again and waits two blocks.
    head(&lab, &cx, &mut scheduler).await;
    assert_eq!(
        scheduler.slots[&MechId(0)].unsigned_retry,
        Some((failed + 3, 2))
    );
    let reads = nonce_reads(&lab);
    head(&lab, &cx, &mut scheduler).await;
    assert_eq!(nonce_reads(&lab), reads, "the backoff skips this head");
    head(&lab, &cx, &mut scheduler).await;
    assert_eq!(
        scheduler.slots[&MechId(0)].unsigned_retry,
        Some((failed + 7, 3))
    );
    assert_eq!(lab.authors(), 0);

    // The endpoint recovers; the same epoch commits once.
    refuse.store(false, Ordering::SeqCst);
    for _ in 0..4 {
        head(&lab, &cx, &mut scheduler).await;
    }
    assert_eq!(lab.authors(), 1, "{:?}", scheduler.last_attempt(MechId(0)));
    assert!(matches!(
        scheduler.last_attempt(MechId(0)).unwrap().outcome,
        WeightAttemptOutcome::Submitted { .. }
    ));
    let slot = &scheduler.slots[&MechId(0)];
    assert_eq!(slot.last_attempt_epoch, Some(5), "the same epoch");
    assert_eq!(slot.unsigned_retry, None);
    for _ in 0..3 {
        head(&lab, &cx, &mut scheduler).await;
    }
    assert_eq!(lab.authors(), 1, "one commit per epoch");
    close_cr_lab(scheduler, lab, cx).await;
    Ok(())
}

/// A dispatch that signed and sent its bytes keeps the epoch's slot even
/// though it failed, so the epoch never carries a second commitment.
#[tokio::test]
async fn a_failure_after_signing_keeps_the_epoch_slot() -> anyhow::Result<()> {
    let (pallet, error) = module_error("SettingWeightsTooFast");
    let failure = json!({"phase":"ApplyExtrinsic","extrinsic_idx":0,"module_id":"System","event_id":"ExtrinsicFailed","attributes":{"dispatch_error":{"Module":{"index":pallet,"error":[error,0,0,0]}},"dispatch_info":{"weight":{"ref_time":1,"proof_size":0},"class":"Normal","pays_fee":"Yes"}},"topics":[]});
    let (lab, cx, account) = timed_lab("signed-memory", NOW_MS).await?;
    let mut scheduler = memory(Arc::new(ChainClock::new(NOW_MS)), ClockGuard::default())?;
    plain_author(&lab, account, move |state, ordinal| {
        if ordinal == 0 {
            state.queue_submit(rtao_testkit::SubmitScript::InBlockThenFinalized {
                events: json!([failure.clone()]),
            });
        }
    });
    until_failed(&mut scheduler, &lab, &cx).await?;
    assert_eq!(lab.authors(), 1);
    let slot = &scheduler.slots[&MechId(0)];
    assert_eq!(slot.last_attempt_epoch, Some(5));
    assert!(slot.last_attempt_block.is_some());
    assert_eq!(slot.unsigned_retry, None);
    for _ in 0..4 {
        head(&lab, &cx, &mut scheduler).await;
    }
    assert_eq!(lab.authors(), 1, "the failed epoch is not committed again");
    close_cr_lab(scheduler, lab, cx).await;
    Ok(())
}

/// Journaled work that failed before signing holds no signed attempt; the
/// released slot is checkpointed, so a restarted scheduler commits in the
/// same epoch once journal recovery confirms there is nothing to reconcile.
#[tokio::test]
async fn a_journaled_release_survives_restart_and_commits_in_the_same_epoch() -> anyhow::Result<()>
{
    let (mut lab, cx, account) = timed_lab("unsigned-durable", NOW_MS).await?;
    let mut scheduler = durable(&lab, account)?;
    plain_author(&lab, account, |_, _| {});
    let refuse = refuse_nonces(&lab);
    let failed = until_failed(&mut scheduler, &lab, &cx).await?;
    assert_eq!(lab.authors(), 0);
    assert_eq!(journal_attempts(&lab), 0);
    let slot = &scheduler.slots[&MechId(0)];
    assert_eq!(slot.last_attempt_epoch, None);
    assert_eq!(slot.last_attempt_block, None);
    // Durable work reconciles through the journal before it dispatches again.
    assert!(slot.uncertain);
    assert_eq!(slot.unsigned_retry, Some((failed + 1, 1)));
    let saved = checkpointed(&lab, account);
    assert_eq!(saved["last_attempt_epoch"], json!(null), "{saved}");
    assert_eq!(saved["unsigned_retry"], json!([failed + 1, 1]), "{saved}");

    refuse.store(false, Ordering::SeqCst);
    drop(scheduler);
    lab.restart()
        .await
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    let cx = restarted(&lab, &cx);
    let mut scheduler = durable(&lab, account)?;
    let slot = &scheduler.slots[&MechId(0)];
    assert_eq!(slot.last_attempt_epoch, None);
    assert_eq!(slot.unsigned_retry, Some((failed + 1, 1)));
    head(&lab, &cx, &mut scheduler).await;
    assert_eq!(lab.authors(), 1, "{:?}", scheduler.last_attempt(MechId(0)));
    assert!(matches!(
        scheduler.last_attempt(MechId(0)).unwrap().outcome,
        WeightAttemptOutcome::Submitted { .. }
    ));
    let slot = &scheduler.slots[&MechId(0)];
    assert_eq!(slot.last_attempt_epoch, Some(5), "the same epoch");
    assert!(!slot.uncertain);
    assert_eq!(slot.unsigned_retry, None);
    for _ in 0..3 {
        head(&lab, &cx, &mut scheduler).await;
    }
    assert_eq!(lab.authors(), 1);
    close_cr_lab(scheduler, lab, cx).await;
    Ok(())
}

/// Journaled work whose signed attempt failed keeps the reserved slot in the
/// checkpoint; after a restart the epoch is still consumed.
#[tokio::test]
async fn a_journaled_signed_failure_keeps_the_slot_across_restart() -> anyhow::Result<()> {
    let (pallet, error) = module_error("SettingWeightsTooFast");
    let failure = json!({"phase":"ApplyExtrinsic","extrinsic_idx":0,"module_id":"System","event_id":"ExtrinsicFailed","attributes":{"dispatch_error":{"Module":{"index":pallet,"error":[error,0,0,0]}},"dispatch_info":{"weight":{"ref_time":1,"proof_size":0},"class":"Normal","pays_fee":"Yes"}},"topics":[]});
    let (mut lab, cx, account) = timed_lab("signed-durable", NOW_MS).await?;
    let mut scheduler = durable(&lab, account)?;
    plain_author(&lab, account, move |state, ordinal| {
        if ordinal == 0 {
            state.queue_submit(rtao_testkit::SubmitScript::InBlockThenFinalized {
                events: json!([failure.clone()]),
            });
        }
    });
    until_failed(&mut scheduler, &lab, &cx).await?;
    assert_eq!(lab.authors(), 1);
    assert_eq!(journal_attempts(&lab), 1);
    let slot = &scheduler.slots[&MechId(0)];
    assert_eq!(slot.last_attempt_epoch, Some(5));
    assert!(slot.uncertain);
    assert_eq!(slot.unsigned_retry, None);
    assert_eq!(checkpointed(&lab, account)["last_attempt_epoch"], json!(5));

    drop(scheduler);
    lab.restart()
        .await
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    let cx = restarted(&lab, &cx);
    let mut scheduler = durable(&lab, account)?;
    assert_eq!(scheduler.slots[&MechId(0)].last_attempt_epoch, Some(5));
    for _ in 0..4 {
        head(&lab, &cx, &mut scheduler).await;
    }
    assert_eq!(lab.authors(), 1, "{:?}", scheduler.last_attempt(MechId(0)));
    assert_eq!(scheduler.slots[&MechId(0)].last_attempt_epoch, Some(5));
    close_cr_lab(scheduler, lab, cx).await;
    Ok(())
}
