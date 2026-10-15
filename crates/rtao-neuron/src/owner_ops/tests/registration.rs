//! Public OwnerOps registration follower correlation tests.

use super::*;

fn added(netuid: u16, index: Option<u32>) -> Value {
    json!({"phase":if index.is_some(){"ApplyExtrinsic"}else{"Finalization"},"extrinsic_idx":index,"module_id":"SubtensorModule","event_id":"NetworkAdded","attributes":[netuid,0],"topics":[]})
}
fn queued(owner: AccountId, height: u64) -> Value {
    json!({"phase":"ApplyExtrinsic","extrinsic_idx":0,"module_id":"SubtensorModule","event_id":"NetworkRegistrationQueued","attributes":{"coldkey":owner,"hotkey":HOTKEY,"registration_block":height,"lock_amount":444},"topics":[]})
}
fn identities(state: &mut FakeState, owner: AccountId, netuid: NetUid) -> anyhow::Result<()> {
    state.seed(&st::NETWORKS_ADDED, (netuid,), true)?;
    state.seed(&st::SUBNET_OWNER, (netuid,), DefaultAccount::from(owner))?;
    state.seed(
        &st::SUBNET_OWNER_HOTKEY,
        (netuid,),
        DefaultAccount::from(HOTKEY),
    )?;
    state.seed(
        &st::SUBNET_LOCKED,
        (netuid,),
        rtao_primitives::TaoBalance(444),
    )?;
    Ok(())
}
#[tokio::test]
async fn create_subnet_returns_immediate_correlated_output() -> anyhow::Result<()> {
    let (ops, chain, keys) = fixture().await?;
    chain.mutate_state(|state| -> anyhow::Result<()> {
        identities(state, keys.signer.account(), NetUid(4))?;
        state.queue_submit(SubmitScript::InBlockThenFinalized {
            events: json!([success(), added(4, Some(0))]),
        });
        Ok(())
    })?;
    let result = ops
        .create_subnet(
            HOTKEY,
            RegistrationWait::UntilRegistered {
                timeout: Some(Duration::from_secs(2)),
            },
        )
        .await?;
    assert_eq!(result.netuid, Some(NetUid(4)));
    assert_eq!(result.mode.as_deref(), Some("immediate"));
    assert_eq!(chain.mutate_state(|s| s.submissions.len()), 1);
    ops.client().close().await;
    Ok(())
}
#[tokio::test]
async fn queued_creation_ignores_foreign_added_then_correlates_our_finalized_state()
-> anyhow::Result<()> {
    let (ops, chain, keys) = fixture().await?;
    let owner = keys.signer.account();
    chain.mutate_state(|state| -> anyhow::Result<()> {
        identities(state, HOTKEY, NetUid(4))?;
        state.queue_submit(SubmitScript::InBlockThenFinalized {
            events: json!([success(), queued(owner, 100), added(4, None)]),
        });
        Ok(())
    })?;
    let pending = ops.clone();
    let task = tokio::spawn(async move {
        pending
            .create_subnet(
                HOTKEY,
                RegistrationWait::UntilRegistered {
                    timeout: Some(Duration::from_secs(3)),
                },
            )
            .await
    });
    for _ in 0..1000 {
        if chain
            .requests()
            .iter()
            .any(|r| r.method == "chain_subscribeFinalizedHeads")
        {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(!task.is_finished());
    chain.mutate_state(|state| identities(state, owner, NetUid(5)))?;
    chain.advance(1)?;
    chain.mutate_state(|state| -> anyhow::Result<()> {
        let hash = state
            .block_hash(state.block)
            .ok_or_else(|| anyhow::anyhow!("head"))?;
        state.seed_events(hash, json!([added(5, None)]))?;
        Ok(())
    })?;
    chain.advance(1)?;
    let result = tokio::time::timeout(Duration::from_secs(2), task).await???;
    assert_eq!(result.netuid, Some(NetUid(5)));
    assert_eq!(result.mode.as_deref(), Some("after_deregistration"));
    assert_eq!(chain.mutate_state(|s| s.submissions.len()), 1);
    ops.client().close().await;
    Ok(())
}
#[tokio::test]
async fn receipt_without_correlated_added_never_claims_registered() -> anyhow::Result<()> {
    let (ops, chain, _) = fixture().await?;
    queue(&chain, success());
    assert!(
        ops.create_subnet(
            HOTKEY,
            RegistrationWait::UntilRegistered {
                timeout: Some(Duration::from_millis(20))
            }
        )
        .await
        .is_err()
    );
    assert_eq!(chain.mutate_state(|s| s.submissions.len()), 1);
    ops.client().close().await;
    Ok(())
}

#[tokio::test]
async fn create_timeout_returns_exact_broadcast_hash_for_reconciliation() -> anyhow::Result<()> {
    let (ops, chain, _) = fixture().await?;
    chain.mutate_state(|s| s.queue_submit(SubmitScript::Statuses(vec![])));
    let timed = ops.clone().with_config(OwnerConfig {
        operation_timeout: Some(Duration::from_millis(100)),
        ..Default::default()
    })?;
    let result = timed.create_subnet(HOTKEY, RegistrationWait::Receipt).await;
    let expected = attempted_hash(&chain);
    assert!(
        matches!(result, Err(TxError::Rpc(rtao_rpc::RpcError::FateUnknown { hash })) if hash.0 == expected.0)
    );
    ops.client().close().await;
    Ok(())
}

#[tokio::test]
async fn proxy_creation_correlates_real_coldkey_and_explicit_hotkey() -> anyhow::Result<()> {
    let (ops, chain, keys) = proxy::proxied(rtao_runtime::models::ProxyType::Any).await?;
    let hotkey = AccountId([11; 32]);
    chain.mutate_state(|s| -> anyhow::Result<()> {
        // A delegate-owned subnet and a real-owned subnet with the delegate's
        // default hotkey are both foreign to this registration.
        identities(s, keys.signer.account(), NetUid(4))?;
        identities(s, proxy::REAL, NetUid(5))?;
        identities(s, proxy::REAL, NetUid(6))?;
        s.seed(
            &st::SUBNET_OWNER_HOTKEY,
            (NetUid(6),),
            DefaultAccount::from(hotkey),
        )?;
        s.queue_submit(SubmitScript::InBlockThenFinalized {
            events: json!([
                added(4, Some(0)),
                added(5, Some(0)),
                added(6, Some(0)),
                proxy::executed(),
                success()
            ]),
        });
        Ok(())
    })?;
    let result = ops
        .create_subnet(
            hotkey,
            RegistrationWait::UntilRegistered {
                timeout: Some(Duration::from_secs(2)),
            },
        )
        .await?;
    assert_eq!(result.netuid, Some(NetUid(6)));
    assert_eq!(result.mode.as_deref(), Some("immediate"));
    assert_eq!(chain.mutate_state(|s| s.submissions.len()), 1);
    ops.client().close().await;
    Ok(())
}

#[tokio::test]
async fn proxy_queued_creation_ignores_delegate_and_wrong_hotkey_then_follows_real()
-> anyhow::Result<()> {
    let (ops, chain, keys) = proxy::proxied(rtao_runtime::models::ProxyType::Any).await?;
    let hotkey = AccountId([11; 32]);
    chain.mutate_state(|s| -> anyhow::Result<()> {
        identities(s, keys.signer.account(), NetUid(4))?;
        s.queue_submit(SubmitScript::InBlockThenFinalized {
            events: json!([queued(keys.signer.account(), 100), {
                "phase":"ApplyExtrinsic","extrinsic_idx":0,"module_id":"SubtensorModule",
                "event_id":"NetworkRegistrationQueued","attributes":{
                    "coldkey":proxy::REAL,"hotkey":hotkey,"registration_block":100,"lock_amount":444
                },"topics":[]
            }, proxy::executed(), success()]),
        });
        Ok(())
    })?;
    let pending = ops.clone();
    let task = tokio::spawn(async move {
        pending
            .create_subnet(
                hotkey,
                RegistrationWait::UntilRegistered {
                    timeout: Some(Duration::from_secs(3)),
                },
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(1), async {
        while !chain
            .requests()
            .iter()
            .any(|r| r.method == "chain_subscribeNewHeads")
        {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    assert!(!task.is_finished());
    chain.mutate_state(|s| -> anyhow::Result<()> {
        identities(s, proxy::REAL, NetUid(5))?;
        Ok(())
    })?;
    chain.advance(1)?;
    chain.mutate_state(|s| -> anyhow::Result<()> {
        let hash = s
            .block_hash(s.block)
            .ok_or_else(|| anyhow::anyhow!("head"))?;
        s.seed_events(hash, json!([added(4, None), added(5, None)]))?;
        Ok(())
    })?;
    chain.advance(1)?;
    // Let the follower consume the foreign events before publishing ours.
    tokio::task::yield_now().await;
    assert!(!task.is_finished());
    chain.mutate_state(|s| -> anyhow::Result<()> {
        identities(s, proxy::REAL, NetUid(6))?;
        s.seed(
            &st::SUBNET_OWNER_HOTKEY,
            (NetUid(6),),
            DefaultAccount::from(hotkey),
        )?;
        Ok(())
    })?;
    chain.advance(1)?;
    chain.mutate_state(|s| -> anyhow::Result<()> {
        let hash = s
            .block_hash(s.block)
            .ok_or_else(|| anyhow::anyhow!("head"))?;
        s.seed_events(hash, json!([added(6, None)]))?;
        Ok(())
    })?;
    chain.advance(1)?;
    let result = tokio::time::timeout(Duration::from_secs(2), task).await???;
    assert_eq!(result.netuid, Some(NetUid(6)));
    assert_eq!(result.mode.as_deref(), Some("after_deregistration"));
    assert_eq!(chain.mutate_state(|s| s.submissions.len()), 1);
    ops.client().close().await;
    Ok(())
}
