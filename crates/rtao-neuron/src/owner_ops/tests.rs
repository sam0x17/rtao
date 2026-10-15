//! Owner orchestration regressions with real encoded FakeChain outcomes.

use super::*;
use rtao_client::{ClientConfig, Inclusion, Network};
use rtao_primitives::{CryptoType, DefaultAccount, H256};
use rtao_signer::{KeyView, KeypairSigner, MultiSignature, Signer, SignerError, SigningRequest};
use rtao_testkit::{FakeChain, FakeState, Reply, SubmitScript, rpc::RpcClient};
use serde_json::{Value, json};
use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};

const NET: NetUid = NetUid(3);
const HOTKEY: AccountId = AccountId([8; 32]);
type AfterSign = Box<dyn FnOnce() + Send>;
#[derive(Clone)]
struct Recorder {
    key: KeypairSigner,
    calls: Arc<Mutex<Vec<Vec<u8>>>>,
    after: Arc<Mutex<Option<AfterSign>>>,
}
impl Signer for Recorder {
    fn account(&self) -> AccountId {
        self.key.account()
    }
    fn crypto_type(&self) -> CryptoType {
        self.key.crypto_type()
    }
    fn sign<'a>(
        &'a self,
        request: &'a SigningRequest<'a>,
    ) -> futures::future::BoxFuture<'a, Result<MultiSignature, SignerError>> {
        Box::pin(async move {
            self.calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(request.call.to_vec());
            let result = self.key.sign(request).await;
            let after = self
                .after
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take();
            if let Some(after) = after {
                after();
            }
            result
        })
    }
}
struct Keys {
    signer: Recorder,
    unlocks: AtomicUsize,
}
impl KeyView for Keys {
    fn hotkey_address(&self) -> Result<AccountId, SignerError> {
        Ok(HOTKEY)
    }
    fn hotkey_public_key(&self) -> Result<[u8; 32], SignerError> {
        Ok(HOTKEY.0)
    }
    fn coldkey_address(&self) -> Result<AccountId, SignerError> {
        Ok(self.signer.account())
    }
}
impl KeySource<Coldkey> for Keys {
    fn address(&self) -> Result<AccountId, SignerError> {
        self.coldkey_address()
    }
    async fn signer(&self) -> Result<Arc<dyn Signer>, SignerError> {
        self.unlocks.fetch_add(1, Ordering::SeqCst);
        Ok(Arc::new(self.signer.clone()))
    }
}
async fn fixture() -> anyhow::Result<(OwnerOps<Keys>, FakeChain, Arc<Keys>)> {
    let key = KeypairSigner::from_seed(&[7; 32], CryptoType::Ed25519)?;
    let mut state = FakeState::try_default()?;
    state.seed(&st::NETWORKS_ADDED, (NET,), true)?;
    state.seed(
        &st::SUBNET_OWNER,
        (NET,),
        DefaultAccount::from(key.account()),
    )?;
    state.seed(
        &st::SUBNET_OWNER_HOTKEY,
        (NET,),
        DefaultAccount::from(HOTKEY),
    )?;
    state.seed(&st::TEMPO, (NET,), 360u16)?;
    state.seed(&st::LAST_EPOCH_BLOCK, (NET,), 100u64)?;
    state.seed(&st::PENDING_EPOCH_AT, (NET,), 0u64)?;
    state.seed(&st::ADMIN_FREEZE_WINDOW, (), 10u16)?;
    state.seed_python(
        "System",
        "Account",
        &[json!(key.account())],
        json!({"data":{"free":1_000_000_000_000u64}}),
    )?;
    state.seed_python(
        "SubtensorModule",
        "AlphaValues",
        &[json!(NET.0)],
        json!([1000, 60000]),
    )?;
    let chain = FakeChain::from_state(state);
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    let client = Client::from_rpc(RpcClient::new(chain.clone()), config).await?;
    let keys = Arc::new(Keys {
        signer: Recorder {
            key,
            calls: Arc::new(Mutex::new(vec![])),
            after: Arc::new(Mutex::new(None)),
        },
        unlocks: AtomicUsize::new(0),
    });
    let ops =
        OwnerOps::new(rtao_intents::executor(client), keys.clone()).with_config(OwnerConfig {
            operation_timeout: Some(Duration::from_secs(4)),
            ..Default::default()
        })?;
    Ok((ops, chain, keys))
}
fn success() -> Value {
    json!({"phase":"ApplyExtrinsic","extrinsic_idx":0,"module_id":"System","event_id":"ExtrinsicSuccess","attributes":{"dispatch_info":{"weight":{"ref_time":1,"proof_size":0},"class":"Normal","pays_fee":"Yes"}},"topics":[]})
}
fn failure(name: &str) -> anyhow::Result<Value> {
    let e = rtao_runtime::errors::MODULE_ERRORS
        .iter()
        .find(|e| e.pallet == "SubtensorModule" && e.name == name)
        .ok_or_else(|| anyhow::anyhow!("missing error"))?;
    Ok(
        json!({"phase":"ApplyExtrinsic","extrinsic_idx":0,"module_id":"System","event_id":"ExtrinsicFailed","attributes":{"dispatch_error":{"Module":{"index":e.pallet_index,"error":[e.error_index,0,0,0]}},"dispatch_info":{"weight":{"ref_time":1,"proof_size":0},"class":"Normal","pays_fee":"Yes"}},"topics":[]}),
    )
}
fn queue(chain: &FakeChain, event: Value) {
    chain.mutate_state(|s| {
        s.queue_submit(SubmitScript::InBlockThenFinalized {
            events: json!([event]),
        })
    });
}

#[tokio::test]
async fn hparam_and_policy_refusal_never_unlock() -> anyhow::Result<()> {
    let (ops, chain, keys) = fixture().await?;
    chain.mutate_state(|s| s.seed(&st::PENDING_EPOCH_AT, (NET,), 110u64))?;
    assert!(
        ops.set_hparam(NET, "rho", 1u16.into(), WindowWait::NoWait)
            .await
            .is_err()
    );
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    assert!(chain.mutate_state(|s| s.submissions.is_empty()));
    assert!(ops.set_mechanism_count(NET, 17).await.is_err());
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    ops.client().close().await;
    Ok(())
}
#[tokio::test]
async fn ownership_refusal_precedes_identity_unlock() -> anyhow::Result<()> {
    let (ops, chain, keys) = fixture().await?;
    chain.mutate_state(|s| s.seed(&st::SUBNET_OWNER, (NET,), DefaultAccount::from(HOTKEY)))?;
    assert!(
        ops.set_identity(
            NET,
            SubnetIdentity {
                subnet_name: "name".into(),
                ..Default::default()
            }
        )
        .await
        .is_err()
    );
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    ops.client().close().await;
    Ok(())
}
#[tokio::test]
async fn start_rereads_mutable_delay_before_unlock() -> anyhow::Result<()> {
    let (ops, chain, keys) = fixture().await?;
    chain.mutate_state(|s| -> anyhow::Result<()> {
        s.seed(&st::NETWORK_REGISTERED_AT, (NET,), 90u64)?;
        s.seed(&st::START_CALL_DELAY, (), 12u64)?;
        Ok(())
    })?;
    let pending = ops.clone();
    let task = tokio::spawn(async move { pending.start(NET).await });
    for _ in 0..100 {
        if chain
            .requests()
            .iter()
            .any(|r| r.method == "chain_subscribeNewHeads")
        {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    chain.mutate_state(|s| s.seed(&st::START_CALL_DELAY, (), 13u64))?;
    chain.advance(2)?;
    tokio::task::yield_now().await;
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    queue(&chain, success());
    chain.advance(1)?;
    let result = tokio::time::timeout(Duration::from_secs(2), task).await???;
    assert!(result.success);
    assert_eq!(chain.mutate_state(|s| s.submissions.len()), 1);
    ops.client().close().await;
    Ok(())
}
#[tokio::test]
async fn operation_cancel_and_deadline_bound_reads_and_waits() -> anyhow::Result<()> {
    let (ops, chain, keys) = fixture().await?;
    chain.mutate_state(|s| s.seed(&st::START_CALL_DELAY, (), 1000u64))?;
    let timed = ops.clone().with_config(OwnerConfig {
        operation_timeout: Some(Duration::from_millis(20)),
        ..Default::default()
    })?;
    assert!(timed.start(NET).await.is_err());
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(
        ops.clone()
            .with_cancellation(cancel)
            .update_symbol(NET, "α")
            .await
            .is_err()
    );
    chain.mutate_state(|s| s.queue_reply("state_queryStorageAt", Reply::silent()));
    assert!(
        timed
            .set_identity(NET, SubnetIdentity::default())
            .await
            .is_err()
    );
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    ops.client().close().await;
    Ok(())
}
#[tokio::test]
async fn exact_rate_retry_occurs_only_once_and_refreshes_alpha() -> anyhow::Result<()> {
    let (ops, chain, keys) = fixture().await?;
    queue(&chain, failure("TxRateLimitExceeded")?);
    queue(&chain, success());
    let fake = chain.clone();
    *keys
        .signer
        .after
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Box::new(move || {
        let _ = fake.mutate_state(|s| {
            s.seed_python(
                "SubtensorModule",
                "AlphaValues",
                &[json!(NET.0)],
                json!([1000, 61000]),
            )
        });
    }));
    let result = ops
        .set_hparam(NET, "alpha_low", 2000u16.into(), WindowWait::NoWait)
        .await?;
    assert!(result.success);
    assert_eq!(chain.mutate_state(|s| s.submissions.len()), 2);
    let calls = keys
        .signer
        .calls
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    assert_ne!(calls.first(), calls.last());
    let (ops2, chain2, _) = fixture().await?;
    queue(&chain2, failure("TxRateLimitExceeded")?);
    queue(&chain2, failure("TxRateLimitExceeded")?);
    queue(&chain2, success());
    let result = ops2
        .set_hparam(NET, "rho", 1u16.into(), WindowWait::NoWait)
        .await?;
    assert!(!result.success);
    assert_eq!(chain2.mutate_state(|s| s.submissions.len()), 2);
    ops.client().close().await;
    ops2.client().close().await;
    Ok(())
}
#[tokio::test]
async fn differently_named_dispatch_and_pool_errors_never_retry() -> anyhow::Result<()> {
    for event in [
        failure("AdminActionProhibitedDuringWeightsWindow")?,
        failure("SubnetNotExists")?,
    ] {
        let (ops, chain, _) = fixture().await?;
        queue(&chain, event);
        let result = ops
            .set_hparam(NET, "rho", 1u16.into(), WindowWait::NoWait)
            .await?;
        assert!(!result.success);
        assert_eq!(chain.mutate_state(|s| s.submissions.len()), 1);
        ops.client().close().await;
    }
    let (ops, chain, _) = fixture().await?;
    chain.mutate_state(|s| s.queue_submit(SubmitScript::Invalid("TxRateLimitExceeded".into())));
    let result = ops
        .set_hparam(NET, "rho", 1u16.into(), WindowWait::NoWait)
        .await?;
    assert!(!result.success);
    assert_eq!(chain.mutate_state(|s| s.submissions.len()), 1);
    ops.client().close().await;
    Ok(())
}
#[test]
fn receipt_only_completion_and_unknown_fate_are_not_rate_retries() {
    let base = ExtrinsicResult {
        success: false,
        message: "rate".into(),
        block_hash: Some(H256([1; 32])),
        extrinsic_id: Some("100-0".into()),
        explorer_url: None,
        fee: None,
        events: vec![],
        error: Some(ChainError::new("rate", Some("TxRateLimitExceeded".into()))),
        data: Default::default(),
        output: (),
        inclusion: Inclusion::Finalized,
        fate_unknown: None,
    };
    assert!(exact_rate_race(&base));
    let mut unknown = base.clone();
    unknown.fate_unknown = Some(H256([2; 32]));
    assert!(!exact_rate_race(&unknown));
    let mut accepted = base.clone();
    accepted.inclusion = Inclusion::PoolAccepted;
    assert!(!exact_rate_race(&accepted));
    let mut pool = base;
    pool.block_hash = None;
    assert!(!exact_rate_race(&pool));
}
#[path = "tests/registration.rs"]
mod registration;

#[tokio::test]
async fn live_mechanism_cap_and_uid_bounds_refuse_before_unlock() -> anyhow::Result<()> {
    let (ops, chain, keys) = fixture().await?;
    chain.mutate_state(|s| -> anyhow::Result<()> {
        s.seed(&st::MAX_MECHANISM_COUNT, (), MechId(2))?;
        s.seed(&st::MIN_ALLOWED_UIDS, (NET,), 100u16)?;
        s.seed(&st::MAX_ALLOWED_UIDS, (NET,), 200u16)?;
        Ok(())
    })?;
    for count in [0, 3, 17] {
        assert!(ops.set_mechanism_count(NET, count).await.is_err());
    }
    for target in [99, 201] {
        assert!(ops.trim(NET, target).await.is_err());
    }
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    assert!(chain.mutate_state(|s| s.submissions.is_empty()));
    ops.client().close().await;
    Ok(())
}

#[tokio::test]
async fn identity_symbol_count_and_trim_use_real_typed_intents() -> anyhow::Result<()> {
    let (ops, chain, keys) = fixture().await?;
    chain.mutate_state(|s| -> anyhow::Result<()> {
        s.seed(&st::MAX_MECHANISM_COUNT, (), MechId(2))?;
        s.seed(&st::MIN_ALLOWED_UIDS, (NET,), 1u16)?;
        s.seed(&st::MAX_ALLOWED_UIDS, (NET,), 128u16)?;
        Ok(())
    })?;
    queue(&chain, success());
    assert!(
        ops.set_identity(
            NET,
            SubnetIdentity {
                subnet_name: "test".into(),
                github_repo: "https://example.test".into(),
                ..Default::default()
            }
        )
        .await?
        .success
    );
    queue(&chain, success());
    assert!(ops.update_symbol(NET, "α").await?.success);
    queue(&chain, success());
    assert!(ops.set_mechanism_count(NET, 2).await?.success);
    queue(&chain, success());
    assert!(ops.trim(NET, 100).await?.success);
    assert_eq!(chain.mutate_state(|s| s.submissions.len()), 4);
    assert_eq!(
        keys.signer
            .calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len(),
        4
    );
    ops.client().close().await;
    Ok(())
}

#[tokio::test]
async fn cancellation_after_first_sign_never_retries_a_known_rate_race() -> anyhow::Result<()> {
    let (ops, chain, keys) = fixture().await?;
    queue(&chain, failure("TxRateLimitExceeded")?);
    let cancel = CancellationToken::new();
    let signal = cancel.clone();
    *keys
        .signer
        .after
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) =
        Some(Box::new(move || signal.cancel()));
    let result = ops
        .clone()
        .with_cancellation(cancel)
        .set_hparam(NET, "rho", 1u16.into(), WindowWait::NoWait)
        .await;
    assert!(result.is_err());
    assert!(chain.mutate_state(|s| s.submissions.len()) <= 1);
    assert_eq!(
        keys.signer
            .calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len(),
        1
    );
    ops.client().close().await;
    Ok(())
}

fn attempted_hash(chain: &FakeChain) -> H256 {
    use blake2::{Blake2b, Digest, digest::consts::U32};
    chain.mutate_state(|s| {
        assert_eq!(s.submissions.len(), 1);
        H256(Blake2b::<U32>::digest(&s.submissions[0]).into())
    })
}

#[tokio::test]
async fn postbroadcast_operation_timeout_preserves_exact_unknown_fate() -> anyhow::Result<()> {
    let (ops, chain, keys) = fixture().await?;
    chain.mutate_state(|s| s.queue_submit(SubmitScript::Statuses(vec![])));
    let timed = ops.clone().with_config(OwnerConfig {
        operation_timeout: Some(Duration::from_millis(100)),
        ..Default::default()
    })?;
    let outcome = timed
        .set_hparam(NET, "rho", 1u16.into(), WindowWait::NoWait)
        .await;
    let expected = attempted_hash(&chain);
    let result = outcome?;
    assert!(!result.success);
    assert_eq!(result.fate_unknown, Some(expected));
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 1);
    ops.client().close().await;
    Ok(())
}

#[tokio::test]
async fn postbroadcast_cancellation_drains_unknown_fate_without_retry() -> anyhow::Result<()> {
    let (ops, chain, keys) = fixture().await?;
    chain.mutate_state(|s| s.queue_submit(SubmitScript::Statuses(vec![])));
    let cancel = CancellationToken::new();
    let pending = ops
        .clone()
        .with_cancellation(cancel.clone())
        .with_config(OwnerConfig {
            operation_timeout: Some(Duration::from_millis(200)),
            ..Default::default()
        })?;
    let task = tokio::spawn(async move {
        pending
            .set_hparam(NET, "rho", 1u16.into(), WindowWait::NoWait)
            .await
    });
    tokio::time::timeout(Duration::from_secs(1), async {
        while chain.mutate_state(|s| s.submissions.is_empty()) {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    cancel.cancel();
    let result = tokio::time::timeout(Duration::from_secs(1), task).await???;
    assert!(!result.success);
    assert_eq!(result.fate_unknown, Some(attempted_hash(&chain)));
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 1);
    ops.client().close().await;
    Ok(())
}

#[tokio::test]
async fn typed_proxy_options_are_admitted() -> anyhow::Result<()> {
    let (ops, _, _) = fixture().await?;
    let configured = ops.clone().with_execute_options(ExecuteOptions {
        proxy_for: Some(AccountId([9; 32])),
        force_proxy_type: Some(rtao_runtime::models::ProxyType::Owner),
        ..Default::default()
    });
    assert!(
        configured.is_ok(),
        "typed proxy options must reach owner preflight"
    );
    ops.client().close().await;
    Ok(())
}
#[path = "tests/proxy.rs"]
pub(crate) mod proxy;
