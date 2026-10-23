//! Read-only scheduling over real public paired submission and qualified history.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use super::super::{WeightCadence, WeightOpportunity};
use super::*;
use crate::{ChainClock, ClockGuard, Metrics, WeightPolicy};
use rtao_client::{
    BlockRef, ShieldPeriod,
    journal::{JournalControl, RecoveryStatus},
};
use rtao_primitives::CryptoType;
use rtao_signer::Signer;
use rtao_testkit::journal_shield::{Inner, JournalLab, LabKeys};
use rtao_tx::{Executor, TxError};
use std::time::Duration;

async fn setup(name: &str) -> anyhow::Result<(JournalLab, WeightScheduler)> {
    let mut state = super::super::tests::fixture_state()?;
    state.set_finalized(state.block)?;
    let root = std::path::PathBuf::from(std::env::var_os("RTAO_TEST_TMPDIR").unwrap())
        .join(format!("readiness-{}-{name}", std::process::id()));
    let lab = JournalLab::new(state, root.join("journal.json"), CryptoType::Ed25519)
        .await
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    let account = lab.signer.account();
    lab.node
        .mutate_state(|s| s.seed_typed("SubtensorModule", "Uids", (NetUid(1), account), 0u16))?;
    let scheduler = WeightScheduler::new(
        WeightPolicy {
            cadence: Some(WeightCadence::RateLimited { margin: 0 }),
            ..Default::default()
        },
        Arc::new(ChainClock::new(1_000_000)),
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
    scheduler.enqueue(
        name,
        Some("readiness:retained".into()),
        MechId(0),
        WeightsInput::Parallel {
            uids: vec![Uid(1)],
            weights: vec![1.0],
        },
    )?;
    Ok((lab, scheduler))
}
async fn preflight(lab: &JournalLab) -> anyhow::Result<WeightPreflight> {
    let snapshot = lab.client.at(BlockRef::Best).await?;
    WeightPreflight::fetch(&snapshot, NetUid(1), MechId(0), lab.signer.account()).await
}
fn intent() -> super::super::submit::PreparedWeights {
    super::super::submit::PreparedWeights {
        netuid: NetUid(1),
        mechanism: MechId(0),
        row: (vec![Uid(1)], vec![u16::MAX]),
        version_key: 0,
        commit: None,
    }
}
#[tokio::test(start_paused = true)]
async fn qualified_shield_recovery_exposes_readiness_without_authority() -> anyhow::Result<()> {
    let keepalive = tokio::spawn(async {
        loop {
            tokio::task::yield_now().await;
        }
    });
    let (mut lab, mut scheduler) = setup("spare").await?;
    let account = lab.signer.account();
    let state = preflight(&lab).await?;
    assert_eq!(
        scheduler.next_action(MechId(0), account, &state)?,
        WeightOpportunity::Ready
    );
    let binding = scheduler
        .prepare_binding(MechId(0), (vec![Uid(1)], vec![u16::MAX]), None, &state)?
        .unwrap();
    let mut options = super::super::submit::SubmissionOptions {
        journal: Some(binding.clone()),
        shield_period: Some(ShieldPeriod::P4),
    }
    .execution(0.001);
    options.deadline = Some(Duration::from_secs(1));
    let signal = Arc::new(tokio::sync::Notify::new());
    let sent = signal.clone();
    lab.after_author(move |_, _, _| {
        sent.notify_one();
    });
    lab.script([Inner::Pending]);
    let keys = Arc::new(LabKeys::new(lab.signer.clone()));
    let k = keys.clone();
    let executor = Executor::new(lab.client.clone());
    let mut task =
        tokio::spawn(async move { executor.execute(&intent(), k.as_ref(), &options).await });
    tokio::select! {_=signal.notified()=>{},early=&mut task=>panic!("before author: {early:?}")}
    // An authorized unknown inner is not a scheduling permission.
    assert_eq!(
        scheduler.next_action(MechId(0), account, &state)?,
        WeightOpportunity::Reconcile
    );
    tokio::time::advance(Duration::from_secs(1)).await;
    let pending = task.await?;
    assert!(
        matches!(pending, Err(TxError::PendingOperation { .. })),
        "{pending:?}"
    );
    scheduler
        .recover_work(&lab.client, &Metrics::default())
        .await?;
    assert_eq!(
        scheduler.next_action(MechId(0), account, &state)?,
        WeightOpportunity::Reconcile
    );
    let expiry = binding.record()?.attempts[0].expiry.unwrap();
    lab.node.mutate_state(|s| -> anyhow::Result<()> {
        s.advance(expiry + 1 - s.block);
        s.set_finalized(s.block)?;
        Ok(())
    })?;
    scheduler
        .recover_work(&lab.client, &Metrics::default())
        .await?;
    assert!(matches!(
        binding.recover(&lab.client, 128).await?,
        RecoveryStatus::Rebuildable
    ));
    assert!(
        binding.record()?.shield_pairs[0]
            .extensions
            .contains_key("rtao_shield_absence_v1")
    );
    assert!(
        binding.can_dispatch().is_err(),
        "ordinary author check remains shield-ineligible"
    );
    let state = preflight(&lab).await?;
    let before = serde_json::to_value(lab.journal().unwrap().snapshot()?)?;
    for _ in 0..2 {
        assert_eq!(
            scheduler.next_action(MechId(0), account, &state)?,
            WeightOpportunity::Ready
        );
    }
    assert_eq!(
        serde_json::to_value(lab.journal().unwrap().snapshot()?)?,
        before,
        "readiness must not claim, retire, mutate or authorize"
    );
    lab.journal().unwrap().control(JournalControl {
        submissions_paused: true,
        ..Default::default()
    })?;
    assert_eq!(
        scheduler.next_action(MechId(0), account, &state)?,
        WeightOpportunity::Paused
    );
    lab.journal().unwrap().control(JournalControl::default())?;
    scheduler.shield_period = Some(ShieldPeriod::P8);
    assert_eq!(
        scheduler.next_action(MechId(0), account, &state)?,
        WeightOpportunity::Reconcile,
        "original policy cannot change"
    );
    scheduler.shield_period = Some(ShieldPeriod::P4);
    scheduler.shield_protocol = None;
    assert_eq!(
        scheduler.next_action(MechId(0), account, &state)?,
        WeightOpportunity::Reconcile,
        "restart must reinstall trusted protocol"
    );
    scheduler = scheduler.with_shield_protocol(lab.protocol.clone());
    lab.upgrade(482, 2, rtao_primitives::H256([43; 32]))
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    let state = preflight(&lab).await?;
    assert_eq!(
        scheduler.next_action(MechId(0), account, &state)?,
        WeightOpportunity::Ready
    );
    let options = super::super::submit::SubmissionOptions {
        journal: Some(binding.clone()),
        shield_period: Some(ShieldPeriod::P4),
    }
    .execution(0.001);
    let refused = Executor::new(lab.client.clone())
        .execute(&intent(), keys.as_ref(), &options)
        .await;
    assert!(
        refused.is_err(),
        "readiness does not qualify a new runtime: {refused:?}"
    );
    assert_eq!(
        binding.record()?.incompatibility.unwrap().code,
        "synthetic-code-unqualified"
    );
    assert_eq!(keys.unlocks(), 1);
    assert_eq!(lab.signer.signatures(), 2);
    assert_eq!(lab.authors(), 1);
    drop(options);
    drop(binding);
    drop(scheduler);
    lab.close().await;
    keepalive.abort();
    Ok(())
}
#[tokio::test]
async fn exhausted_shield_recovery_stays_reconcile_without_renewing_budget() -> anyhow::Result<()> {
    let (mut lab, mut scheduler) = setup("exhausted").await?;
    let account = lab.signer.account();
    let state = preflight(&lab).await?;
    let binding = scheduler
        .prepare_binding(MechId(0), (vec![Uid(1)], vec![u16::MAX]), None, &state)?
        .unwrap();
    let options = super::super::submit::SubmissionOptions {
        journal: Some(binding.clone()),
        shield_period: Some(ShieldPeriod::P4),
    }
    .execution(0.001);
    let maximum = rtao_client::SHIELD_DROPPED_INNER_RETRIES + 1;
    lab.script((0..maximum).map(|_| Inner::Absent));
    let keys = LabKeys::new(lab.signer.clone());
    let result = Executor::new(lab.client.clone())
        .execute(&intent(), &keys, &options)
        .await;
    assert!(
        matches!(
            result,
            Err(TxError::PendingOperation {
                reason: rtao_client::journal::RecoveryReason::ShieldPairBudgetExhausted { .. },
                ..
            })
        ),
        "{result:?}"
    );
    let state = preflight(&lab).await?;
    for _ in 0..2 {
        scheduler
            .recover_work(&lab.client, &Metrics::default())
            .await?;
        assert_eq!(
            scheduler.next_action(MechId(0), account, &state)?,
            WeightOpportunity::Reconcile
        );
    }
    assert!(
        binding.can_prepare_shield(ShieldPeriod::P4).is_err(),
        "read-only readiness must independently reject exhausted original budget"
    );
    assert_eq!(lab.authors(), maximum as usize);
    assert_eq!(binding.record()?.shield_pairs.len(), maximum as usize);
    drop(options);
    drop(binding);
    drop(scheduler);
    lab.close().await;
    Ok(())
}

#[tokio::test]
async fn read_only_route_readiness_preserves_material_and_exclusion_rules() -> anyhow::Result<()> {
    let (mut lab, mut scheduler) = setup("route-material").await?;
    let account = lab.signer.account();
    let (old, _) = scheduler.pending_work(MechId(0))?.unwrap();
    let current = old.clone().material(serde_json::json!({"generation":2}))?;
    assert!(matches!(
        old.can_prepare_shield(ShieldPeriod::P4),
        Err(rtao_client::journal::JournalError::Conflict)
    ));
    let owner = current.reserve()?;
    let before = serde_json::to_value(lab.journal().unwrap().snapshot()?)?;
    assert!(current.can_prepare_shield(ShieldPeriod::P4).is_ok());
    assert!(
        current.reserve().is_err(),
        "readiness must not steal an existing owner"
    );
    assert_eq!(
        serde_json::to_value(lab.journal().unwrap().snapshot()?)?,
        before
    );
    drop(owner);
    // Explicitly selected ordinary mode is a different route even before its
    // first signed attempt. The read-only flag cannot convert it to shielded.
    lab.journal().unwrap().update(|image| {
        image.operations.get_mut(current.id()).unwrap().mode =
            rtao_client::journal::ExecutionMode::Ordinary;
        Ok(())
    })?;
    assert!(current.can_prepare_shield(ShieldPeriod::P4).is_err());
    assert!(current.can_dispatch().is_ok());
    let state = preflight(&lab).await?;
    assert_eq!(
        scheduler.next_action(MechId(0), account, &state)?,
        WeightOpportunity::Reconcile
    );
    scheduler.shield_period = None;
    assert_eq!(
        scheduler.next_action(MechId(0), account, &state)?,
        WeightOpportunity::Ready
    );
    assert_eq!(lab.authors(), 0);
    assert_eq!(lab.signer.signatures(), 0);
    drop(current);
    drop(old);
    drop(scheduler);
    lab.close().await;
    Ok(())
}
