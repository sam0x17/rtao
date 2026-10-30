//! Pool-aware nonce allocation, cancellation and synchronous lease invalidation.
#![allow(clippy::unwrap_used)]

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use rtao_client::NonceManager;
use rtao_primitives::AccountId;
use rtao_testkit::{FakeChain, FakeState, Reply, SubmitScript};
use serde_json::{json, value::RawValue};
use subxt_rpcs::{
    RpcClient, RpcClientT,
    client::{RawRpcFuture, RawRpcSubscription},
};

#[tokio::test]
async fn concurrent_clones_reserve_unique_consecutive_nonces_with_one_query() {
    let raw = FakeChain::from_state(FakeState::default());
    raw.mutate_state(|state| state.queue_reply("account_nextIndex", Reply::result(json!(17))));
    let manager = NonceManager::new(RpcClient::new(raw.clone()));
    let who = AccountId([7; 32]);
    let tasks = (0..64).map(|_| {
        let manager = manager.clone();
        tokio::spawn(async move {
            let lease = manager.lease(&who).await.unwrap();
            let nonce = lease.value();
            lease.settle();
            nonce
        })
    });
    let mut values = futures::future::join_all(tasks)
        .await
        .into_iter()
        .map(Result::unwrap)
        .collect::<Vec<_>>();
    values.sort_unstable();
    assert_eq!(values, (17..81).collect::<Vec<_>>());
    assert_eq!(
        raw.requests()
            .iter()
            .filter(|request| request.method == "account_nextIndex")
            .count(),
        1
    );
}

#[tokio::test]
async fn unsettled_drop_requeries_pool_and_fresh_does_not_mutate_cache() {
    let raw = FakeChain::from_state(FakeState::default());
    let manager = NonceManager::new(RpcClient::new(raw.clone()));
    let who = AccountId([8; 32]);
    manager.pin(&who, 10);
    raw.mutate_state(|state| state.queue_reply("account_nextIndex", Reply::result(json!(25))));
    assert_eq!(manager.fresh(&who).await.unwrap(), 25);
    let lease = manager.lease(&who).await.unwrap();
    assert_eq!(lease.value(), 11);
    drop(lease);
    assert_eq!(manager.cached(&who.to_string()).await, None);
    raw.mutate_state(|state| state.queue_reply("account_nextIndex", Reply::result(json!(12))));
    let lease = manager.lease(&who).await.unwrap();
    assert_eq!(lease.value(), 12);
    lease.settle();
    assert_eq!(manager.cached(&who.to_string()).await, Some(12));
}

#[tokio::test]
async fn adjacent_pair_is_atomic_and_any_unsettled_half_invalidates_account() {
    let raw = FakeChain::from_state(FakeState::default());
    let manager = NonceManager::new(RpcClient::new(raw));
    let who = AccountId([9; 32]);
    manager.pin(&who, 20);
    let (inner, carrier) = manager.lease_pair(&who).await.unwrap();
    assert_eq!((inner.value(), carrier.value()), (21, 22));
    let later = manager.lease(&who).await.unwrap();
    assert_eq!(later.value(), 23);
    later.settle();
    inner.settle();
    drop(carrier);
    assert_eq!(manager.cached(&who.to_string()).await, None);
    manager.pin(&who, u64::MAX - 1);
    assert!(manager.lease_pair(&who).await.is_err());
    assert_eq!(manager.cached(&who.to_string()).await, Some(u64::MAX - 1));
}

#[tokio::test]
async fn addresses_with_different_ss58_prefixes_share_one_account_cache() {
    let raw = FakeChain::from_state(FakeState::default());
    let manager = NonceManager::new(RpcClient::new(raw));
    let who = AccountId([10; 32]);
    manager.pin(&who, 40);
    assert_eq!(manager.next_for(&who.to_ss58(0), true).await.unwrap(), 41);
    assert_eq!(manager.next_for(&who.to_ss58(42), true).await.unwrap(), 42);
    let lease = manager.lease(&who).await.unwrap();
    assert_eq!(lease.value(), 43);
    lease.settle();
}

struct PausedRpc {
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
    requests: AtomicUsize,
}
impl RpcClientT for PausedRpc {
    fn request_raw<'a>(
        &'a self,
        _method: &'a str,
        _params: Option<Box<RawValue>>,
    ) -> RawRpcFuture<'a, Box<RawValue>> {
        Box::pin(async move {
            let request = self.requests.fetch_add(1, Ordering::SeqCst);
            if request == 0 {
                self.entered.notify_one();
                self.release.notified().await;
            }
            RawValue::from_string((13 + request * 10).to_string())
                .map_err(subxt_rpcs::Error::Serialization)
        })
    }
    fn subscribe_raw<'a>(
        &'a self,
        _sub: &'a str,
        _params: Option<Box<RawValue>>,
        _unsub: &'a str,
    ) -> RawRpcFuture<'a, RawRpcSubscription> {
        Box::pin(async {
            Err(subxt_rpcs::Error::Client(Box::new(
                rtao_rpc::RpcError::Closed,
            )))
        })
    }
}
fn paused() -> Arc<PausedRpc> {
    Arc::new(PausedRpc {
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
        requests: AtomicUsize::new(0),
    })
}

#[tokio::test]
async fn explicit_pin_during_first_query_cannot_be_overwritten_by_stale_response() {
    let raw = paused();
    let manager = NonceManager::new(RpcClient::new(raw.clone()));
    let who = AccountId([11; 32]);
    let child = manager.clone();
    let task = tokio::spawn(async move { child.lease(&who).await });
    raw.entered.notified().await;
    manager.pin(&who, 100);
    raw.release.notify_one();
    let lease = task.await.unwrap().unwrap();
    assert_eq!(lease.value(), 101);
    lease.settle();
    assert_eq!(raw.requests.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn cancellation_of_first_query_releases_fetch_lock_without_reserving_nonce() {
    let raw = paused();
    let manager = NonceManager::new(RpcClient::new(raw.clone()));
    let who = AccountId([12; 32]);
    let child = manager.clone();
    let task = tokio::spawn(async move { child.lease(&who).await });
    raw.entered.notified().await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    let lease = manager.lease(&who).await.unwrap();
    assert_eq!(lease.value(), 23);
    lease.settle();
    assert_eq!(raw.requests.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn synchronous_clear_during_query_forces_a_new_pool_query() {
    let raw = paused();
    let manager = NonceManager::new(RpcClient::new(raw.clone()));
    let who = AccountId([13; 32]);
    let child = manager.clone();
    let task = tokio::spawn(async move { child.lease(&who).await });
    raw.entered.notified().await;
    manager.clear_account(&who);
    raw.release.notify_one();
    let lease = task.await.unwrap().unwrap();
    assert_eq!(lease.value(), 23);
    lease.settle();
    assert_eq!(raw.requests.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn an_account_waiting_for_its_first_rpc_does_not_block_other_accounts() {
    let raw = paused();
    let manager = NonceManager::new(RpcClient::new(raw.clone()));
    let first = AccountId([14; 32]);
    let second = AccountId([15; 32]);
    let child = manager.clone();
    let task = tokio::spawn(async move { child.lease(&first).await });
    raw.entered.notified().await;
    let lease = tokio::time::timeout(std::time::Duration::from_secs(1), manager.lease(&second))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(lease.value(), 23);
    lease.settle();
    raw.release.notify_one();
    let lease = task.await.unwrap().unwrap();
    assert_eq!(lease.value(), 13);
    lease.settle();
}

// A fresh nonce waits for this account's pending shield carriers, whose inner
// nonce account_nextIndex cannot see (D-084).

fn shield_state(slot_millis: u64) -> FakeState {
    let mut state = FakeState::default();
    state
        .seed_constant("Aura", "SlotDuration", json!(slot_millis))
        .unwrap();
    // An encoded 1184-byte ML-KEM-768 encapsulation key accepted by the core.
    state
        .seed_typed("MevShield", "NextKey", (), vec![0u8; 1184])
        .unwrap();
    state
}

async fn client_over(fake: &FakeChain) -> rtao_client::Client {
    let mut config = rtao_client::ClientConfig::new(
        rtao_client::Network::resolve_with("local", |_| None).unwrap(),
    )
    .unwrap();
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    rtao_client::Client::from_rpc(RpcClient::new(fake.clone()), config)
        .await
        .unwrap()
}

fn key(seed: u8) -> rtao_signer::KeypairSigner {
    rtao_signer::KeypairSigner::from_seed(&[seed; 32], rtao_signer::CryptoType::Ed25519).unwrap()
}

async fn remark(client: &rtao_client::Client) -> rtao_client::CallBytes {
    rtao_runtime::calls::system::remark(vec![1])
        .compose(&client.refresh_runtime().await.unwrap())
        .unwrap()
}

/// Leave one shield carrier from `signer` pending in the fake's ready pool.
async fn pool_carrier(
    client: &rtao_client::Client,
    fake: &FakeChain,
    signer: &rtao_signer::KeypairSigner,
) {
    fake.mutate_state(|state| state.queue_submit(SubmitScript::Pooled));
    let opts = rtao_client::ShieldOptions {
        wait: rtao_client::Wait::Pool,
        ..Default::default()
    };
    client
        .submit_shielded(remark(client).await, signer, &opts)
        .await
        .unwrap();
}

fn pool_reads(fake: &FakeChain) -> usize {
    fake.requests()
        .iter()
        .filter(|request| request.method == "author_pendingExtrinsics")
        .count()
}

#[tokio::test]
async fn cleared_cache_after_a_pending_carrier_never_returns_its_inner_nonce() {
    use rtao_signer::Signer;
    let fake = FakeChain::from_state(shield_state(1));
    let client = client_over(&fake).await;
    let signer = key(7);
    let who = signer.account();
    fake.mutate_state(|state| state.set_nonce(who.to_string(), 7));
    pool_carrier(&client, &fake, &signer).await;
    // The node counts the pooled carrier at 7 and cannot see its inner at 8.
    let reported: u64 = client
        .request("account_nextIndex", json!([who.to_string()]))
        .await
        .unwrap();
    assert_eq!(reported, 8);
    // An unsettled lease clears what this client knew about the pair.
    drop(client.nonces().lease(&who).await.unwrap());
    assert_eq!(client.nonces().cached(&who.to_string()).await, None);
    let reads = Arc::new(AtomicUsize::new(0));
    let counter = reads.clone();
    let address = who.to_string();
    fake.mutate_state(|state| {
        state.set_handler("author_pendingExtrinsics", move |state, request| {
            // The pair lands while the second pool read is being made.
            if counter.fetch_add(1, Ordering::SeqCst) == 1 {
                state.include_pool();
                state.set_nonce(address.clone(), 9);
            }
            state.dispatch(request)
        });
    });
    let lease = client.nonces().lease(&who).await.unwrap();
    assert_eq!(lease.value(), 9);
    lease.settle();
    assert_eq!(reads.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn pool_probe_ignores_other_accounts_plain_transactions_and_undecodable_entries() {
    use rtao_signer::Signer;
    let fake = FakeChain::from_state(shield_state(1));
    let client = client_over(&fake).await;
    let signer = key(7);
    let who = signer.account();
    // Another account's carrier and this account's ordinary pending call.
    pool_carrier(&client, &fake, &key(5)).await;
    fake.mutate_state(|state| state.queue_submit(SubmitScript::Pooled));
    client
        .submit(
            remark(&client).await,
            &signer,
            &rtao_client::SubmitOptions {
                wait: rtao_client::Wait::Pool,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    fake.mutate_state(|state| {
        state.pool.push(vec![0xde, 0xad]);
        // Carries this account's key but is not an extrinsic.
        let mut garbage = vec![0x84, 0, 0, 0];
        garbage.extend_from_slice(&who.0);
        state.pool.push(garbage);
    });
    let other = client_over(&fake).await;
    let before = pool_reads(&fake);
    let lease = other.nonces().lease(&who).await.unwrap();
    // The plain call at 0 is already counted by account_nextIndex.
    assert_eq!(lease.value(), 1);
    lease.settle();
    assert_eq!(pool_reads(&fake) - before, 1);
}

#[tokio::test]
async fn missing_pool_method_falls_back_and_is_not_asked_again() {
    let fake = FakeChain::from_state(shield_state(1));
    fake.mutate_state(|state| {
        state.set_handler("author_pendingExtrinsics", |_, _| {
            Reply::error(-32601, "Method not found", None)
        });
        state.set_nonce(AccountId([3; 32]).to_string(), 4);
    });
    let client = client_over(&fake).await;
    let who = AccountId([3; 32]);
    drop(client.nonces().lease(&who).await.unwrap());
    let lease = client.nonces().lease(&who).await.unwrap();
    assert_eq!(lease.value(), 4);
    lease.settle();
    assert_eq!(pool_reads(&fake), 1);
}

#[tokio::test]
async fn policy_refused_pool_read_falls_back_without_error() {
    let fake = FakeChain::from_state(shield_state(1));
    fake.mutate_state(|state| {
        state.set_handler("author_pendingExtrinsics", |_, _| {
            Reply::error(
                -32005,
                "method blocked",
                Some(json!({"policy": "deny-author"})),
            )
        });
        state.set_nonce(AccountId([4; 32]).to_string(), 6);
    });
    let client = client_over(&fake).await;
    let who = AccountId([4; 32]);
    assert_eq!(client.nonces().fresh(&who).await.unwrap(), 6);
    assert_eq!(client.nonces().fresh(&who).await.unwrap(), 6);
    // A policy refusal may be temporary, so it is asked again.
    assert_eq!(pool_reads(&fake), 2);
}

#[tokio::test(start_paused = true)]
async fn pending_carrier_wait_backs_off_and_stops_after_ten_pool_reads() {
    use rtao_signer::Signer;
    let fake = FakeChain::from_state(shield_state(1_000));
    let client = client_over(&fake).await;
    let signer = key(7);
    let who = signer.account();
    pool_carrier(&client, &fake, &signer).await;
    let other = client_over(&fake).await;
    let before = pool_reads(&fake);
    let started = tokio::time::Instant::now();
    let lease = other.nonces().lease(&who).await.unwrap();
    // The carrier never leaves the pool: after the bounded wait the node's
    // answer is used as reported, with no error.
    assert_eq!(lease.value(), 1);
    lease.settle();
    assert_eq!(
        pool_reads(&fake) - before,
        usize::try_from(rtao_client::SHIELD_CARRIER_POLLS).unwrap()
    );
    // One block doubling to eight: 1 + 2 + 4 + 8 * 6 blocks of one second.
    let waited = started.elapsed();
    assert!(waited >= std::time::Duration::from_secs(55), "{waited:?}");
    assert!(waited < std::time::Duration::from_secs(56), "{waited:?}");
}
