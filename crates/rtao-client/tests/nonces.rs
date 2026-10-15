//! Pool-aware nonce allocation, cancellation and synchronous lease invalidation.
#![allow(clippy::unwrap_used)]

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use rtao_client::NonceManager;
use rtao_primitives::AccountId;
use rtao_testkit::{FakeChain, FakeState, Reply};
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
