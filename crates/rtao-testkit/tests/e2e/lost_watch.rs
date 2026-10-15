//! Force unknown fate on the real transport, then prove finalized inclusion.

use super::{TestResult, framework, watch_proxy::WatchProxy};
use blake2::{Blake2b, Digest, digest::consts::U32};
use futures::StreamExt;
use rtao_client::{BlockRef, Inclusion, View};
use rtao_core::value_ext::ValueExt;
use rtao_neuron::weights::WeightAttemptOutcome;
use rtao_primitives::{H256, MechId};
use rtao_runtime::calls::system;
use rtao_signer::KeyView;
use rtao_testkit::Localnet;
use std::time::Duration;

pub async fn run(ln: &Localnet) -> TestResult {
    // Setup uses the original Localnet's cross-process Alice lock. Only the
    // validator connection passes through the fault-injection proxy.
    let mut fixture = framework::Fixture::new(ln, "lost-watch", true).await?;
    let original_client = fixture.cx.client.clone();
    let outcome = async {
        let mut proxy = WatchProxy::start(ln.endpoint()).await?;
        let proxied = Localnet::connect(proxy.endpoint(), ln.profile()).await?;
        fixture.cx.client = proxied.client().clone();
        fixture.cx.executor = rtao_tx::Executor::new(fixture.cx.client.clone());
        let outcome = exercise(&fixture, &proxy).await;
        proxied.client().close().await;
        proxy.close().await;
        outcome
    }
    .await;
    let closed = fixture.close().await;
    original_client.close().await;
    outcome?;
    closed?;
    Ok(())
}

async fn exercise(fixture: &framework::Fixture, proxy: &WatchProxy) -> TestResult {
    let client = &fixture.cx.client;
    let account = fixture.cx.hotkey.hotkey_address()?;
    let before = client.nonces().fresh(&account).await?;
    let mut scheduler = framework::scheduler(&fixture.cx).await?;
    let mut blocks = client.blocks(rtao_client::Finality::Finalized)?;
    proxy.arm();
    let deadline = std::time::Instant::now() + Duration::from_secs(240);
    let inclusion = loop {
        let header = tokio::time::timeout(Duration::from_secs(120), blocks.next())
            .await?
            .ok_or("finalized stream ended")??;
        scheduler
            .on_finalized(&fixture.cx, &header, |_| {
                framework::proposal(fixture.miner_uid)
            })
            .await?;
        if let Some(attempt) = scheduler.last_attempt(MechId(0)) {
            match &attempt.outcome {
                WeightAttemptOutcome::Submitted {
                    block_hash,
                    block_number,
                } => break (*block_hash, *block_number),
                WeightAttemptOutcome::Unresolved(message)
                | WeightAttemptOutcome::Failed(message) => {
                    return Err(format!(
                        "lost-watch commit was not definitively reconciled: {message}"
                    )
                    .into());
                }
                _ => {}
            }
        }
        if std::time::Instant::now() >= deadline {
            return Err("lost-watch scheduler did not submit a commit".into());
        }
    };
    assert_eq!(
        proxy.drops(),
        1,
        "production session socket was not dropped during its commit watch"
    );
    let watches = proxy.watches();
    assert_eq!(
        watches.len(),
        1,
        "commit bytes were resubmitted after the socket loss"
    );
    let commit_hash = H256(Blake2b::<U32>::digest(&watches[0]).into());
    let report = client
        .resolve_extrinsic(commit_hash, inclusion.0, true)
        .await?;
    assert!(
        report.success,
        "reconciled commit dispatch failed: {report:?}"
    );
    assert_eq!(report.inclusion, Inclusion::Finalized);
    assert_eq!(report.block_number, Some(inclusion.1));
    assert!(
        report
            .events
            .iter()
            .any(|event| event.matches("SubtensorModule", "TimelockedWeightsCommitted"))
    );
    let snapshot = client.at(BlockRef::Hash(inclusion.0)).await?;
    let decoded = snapshot
        .block()
        .runtime
        .core()
        .decode_extrinsic(&watches[0], true)?;
    assert_eq!(
        decoded.field("nonce").and_then(ValueExt::as_u128),
        Some(u128::from(before))
    );
    assert_eq!(client.nonces().fresh(&account).await?, before + 1);
    assert_eq!(
        scheduler.reveals().pending_for(MechId(0)),
        1,
        "production reveal watcher did not retain the reconciled commit"
    );
    // A different valid call must use the immediately following nonce, with
    // neither an unknown-fate lease nor a hidden resend consuming it.
    let signer = <rtao_wallet::HotkeyFile as rtao_signer::KeySource<rtao_signer::Hotkey>>::signer(
        &fixture.cx.hotkey,
    )
    .await?;
    let next = client
        .submit(
            system::remark(vec![7]).compose(&client.runtime().await?)?,
            signer.as_ref(),
            &rtao_client::SubmitOptions {
                wait: rtao_client::Wait::Finalization,
                reconcile: true,
                deadline: Some(Duration::from_secs(240)),
                ..Default::default()
            },
        )
        .await?;
    assert!(
        next.success,
        "submission after reconciled commit failed: {next:?}"
    );
    let watches = proxy.watches();
    assert_eq!(
        watches.len(),
        2,
        "later submission did not use the recovered session exactly once"
    );
    let snapshot = client
        .at(BlockRef::Hash(
            next.block_hash.ok_or("later submission block absent")?,
        ))
        .await?;
    let decoded = snapshot
        .block()
        .runtime
        .core()
        .decode_extrinsic(&watches[1], true)?;
    assert_eq!(
        decoded.field("nonce").and_then(ValueExt::as_u128),
        Some(u128::from(before + 1))
    );
    assert_eq!(client.nonces().fresh(&account).await?, before + 2);
    Ok(())
}
