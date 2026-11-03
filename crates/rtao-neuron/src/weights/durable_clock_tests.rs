//! An application-driven scheduler anchors its own timelock clock from the
//! chain, with no clock worker (D-094).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use super::super::WeightAttemptOutcome;
use super::public_tests::{close_cr_lab, cr_lab_with, plain_author, until_authored};
use super::*;
use crate::{ChainClock, Clock, ClockGuard, SystemClock, WeightPolicy};
use rtao_testkit::journal_shield::JournalLab;

pub(super) const NOW_MS: u64 = 1_800_000_000_000;

/// A lab whose blocks carry `Timestamp.Now = timestamp_ms`.
pub(super) async fn timed_lab(
    name: &str,
    timestamp_ms: u64,
) -> anyhow::Result<(JournalLab, crate::NeuronCx, AccountId)> {
    cr_lab_with(name, 4, move |state| {
        state.seed(&rtao_runtime::storage::timestamp::NOW, (), timestamp_ms)?;
        Ok(())
    })
    .await
}

pub(super) fn memory(clock: Arc<dyn Clock>, guard: ClockGuard) -> anyhow::Result<WeightScheduler> {
    WeightScheduler::new(WeightPolicy::default(), clock, guard)
}

/// An application that drives `on_finalized` without `NeuronRuntime`'s
/// clock worker commits with an unanchored `ChainClock`: the scheduler reads
/// `Timestamp.Now` at its preflight block and anchors the clock itself.
/// Without that, every commit refused with "no verified time anchor".
#[tokio::test]
async fn an_unanchored_chain_clock_is_anchored_from_the_preflight_block() -> anyhow::Result<()> {
    let (lab, cx, account) = timed_lab("anchor-chain", NOW_MS).await?;
    let clock = ChainClock::unanchored();
    let guard = ClockGuard::default();
    let mut scheduler = memory(Arc::new(clock.clone()), guard.clone())?;
    let tails = plain_author(&lab, account, |_, _| {});
    until_authored(&mut scheduler, &lab, &cx).await?;
    assert_eq!(lab.authors(), 1, "{:?}", scheduler.last_attempt(MechId(0)));
    assert!(matches!(
        scheduler.last_attempt(MechId(0)).unwrap().outcome,
        WeightAttemptOutcome::Submitted { .. }
    ));
    assert_eq!(tails.lock().unwrap().len(), 1);
    assert!(clock.is_anchored());
    let now = clock.now_unix();
    let anchor = NOW_MS as f64 / 1000.0;
    assert!((anchor..anchor + 60.0).contains(&now), "{now}");
    // The skew guard saw the same verified timestamp.
    assert!(guard.health().is_some());
    assert!(
        !cx.metrics.render().contains("rtao_clock_sync_errors_total"),
        "{}",
        cx.metrics.render()
    );
    close_cr_lab(scheduler, lab, cx).await;
    Ok(())
}

/// The system clock needs a skew estimate before it may commit. Without a
/// clock worker, the scheduler's own timestamp read provides it.
#[tokio::test]
async fn the_system_clock_commits_from_the_scheduler_skew_sample() -> anyhow::Result<()> {
    let host_ms = (SystemClock.now_unix() * 1000.0) as u64;
    let (lab, cx, account) = timed_lab("anchor-system", host_ms).await?;
    let guard = ClockGuard::default();
    let mut scheduler = memory(Arc::new(SystemClock), guard.clone())?;
    plain_author(&lab, account, |_, _| {});
    until_authored(&mut scheduler, &lab, &cx).await?;
    assert_eq!(lab.authors(), 1, "{:?}", scheduler.last_attempt(MechId(0)));
    let health = guard.health().expect("skew sample");
    assert!(!health.refuse_system_commit, "{health:?}");
    close_cr_lab(scheduler, lab, cx).await;
    Ok(())
}

/// A clock worker's anchor is never moved back by the scheduler's read of
/// an older block's timestamp.
#[tokio::test]
async fn a_fresher_anchor_is_kept() -> anyhow::Result<()> {
    let (lab, cx, account) = timed_lab("anchor-kept", NOW_MS).await?;
    let ahead = NOW_MS + 3_600_000;
    let clock = ChainClock::new(ahead);
    let mut scheduler = memory(Arc::new(clock.clone()), ClockGuard::default())?;
    plain_author(&lab, account, |_, _| {});
    until_authored(&mut scheduler, &lab, &cx).await?;
    assert_eq!(lab.authors(), 1);
    let anchor = ahead as f64 / 1000.0;
    assert!(
        (anchor..anchor + 60.0).contains(&clock.now_unix()),
        "{}",
        clock.now_unix()
    );
    close_cr_lab(scheduler, lab, cx).await;
    Ok(())
}
