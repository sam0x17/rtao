// Derived from RaoFoundation/subtensor sdk/python/bittensor/_transport/rpc.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Bounded subscription queues.
use crate::{RpcError, Session, session::WireError};
use futures::{Stream, task::AtomicWaker};
use serde::de::DeserializeOwned;
use serde_json::{Value, value::RawValue};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    io::{self, Write},
    marker::PhantomData,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
};

/// Count JSON bytes through serde's real encoder without building another string.
struct JsonCounter(usize);
impl Write for JsonCounter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0 = self.0.saturating_add(bytes.len());
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
/// Retained charge includes encoded JSON and conservative container/string storage.
/// This is a queue budget, not a whole-process allocator/RSS measurement.
pub(crate) fn notification_bytes(value: &Value) -> usize {
    fn storage(value: &Value) -> usize {
        let extra = match value {
            Value::String(text) => text.capacity(),
            Value::Array(items) => items.iter().fold(
                items
                    .capacity()
                    .saturating_mul(std::mem::size_of::<Value>()),
                |sum, item| sum.saturating_add(storage(item)),
            ),
            Value::Object(items) => items.iter().fold(0usize, |sum, (key, item)| {
                sum.saturating_add(key.capacity())
                    // preserve_order uses IndexMap: allow spare entry/index
                    // capacity per live member, not just the compact pair.
                    .saturating_add(
                        4 * (std::mem::size_of::<String>()
                            + std::mem::size_of::<Value>()
                            + 4 * std::mem::size_of::<usize>()),
                    )
                    .saturating_add(storage(item))
            }),
            _ => 0,
        };
        std::mem::size_of::<Value>().saturating_add(extra)
    }
    let mut counter = JsonCounter(0);
    if serde_json::to_writer(&mut counter, value).is_err() {
        return usize::MAX;
    }
    counter
        .0
        .saturating_add(storage(value))
        // Queue backing storage is kept below four slots per retained item.
        .saturating_add(4 * std::mem::size_of::<Queued>())
}

/// One mutex owns all active queue accounting, so consumer polls and global
/// oldest-first eviction cannot race or acquire locks in opposite orders.
pub(crate) struct QueueBudget {
    inner: Mutex<BudgetState>,
    limit: usize,
}
struct BudgetState {
    queues: BTreeMap<u64, QueueState>,
    bytes: usize,
    next_queue: u64,
    next_order: u128,
}
struct QueueState {
    items: VecDeque<Queued>,
    closed: bool,
}
struct Queued {
    item: Result<Value, WireError>,
    bytes: usize,
    order: u128,
}
impl QueueBudget {
    pub(crate) fn new(limit: usize) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(BudgetState {
                queues: BTreeMap::new(),
                bytes: 0,
                next_queue: 0,
                next_order: 0,
            }),
            limit,
        })
    }
    pub(crate) fn retained_bytes(&self) -> usize {
        self.inner
            .lock()
            .map(|state| state.bytes)
            .unwrap_or(self.limit)
    }
}
impl BudgetState {
    fn pop(&mut self, key: u64) -> Option<Queued> {
        let queue = self.queues.get_mut(&key)?;
        let item = queue.items.pop_front()?;
        // Release high-water backing storage as consumers/global eviction drain
        // a queue. Otherwise empty streams could retain uncharged allocations.
        if queue.items.len() <= queue.items.capacity() / 4 {
            queue.items.shrink_to_fit();
        }
        self.bytes -= item.bytes;
        Some(item)
    }
    fn clear(&mut self, key: u64) {
        if let Some(queue) = self.queues.get_mut(&key) {
            self.bytes -= queue.items.iter().map(|item| item.bytes).sum::<usize>();
            queue.items = VecDeque::new();
        }
    }
    fn evict_oldest(&mut self) -> Option<u64> {
        let key = self
            .queues
            .iter()
            .filter_map(|(key, queue)| {
                queue
                    .items
                    .front()
                    // Terminal errors must remain observable and carry no JSON charge.
                    .filter(|item| item.bytes != 0)
                    .map(|item| (*key, item.order))
            })
            .min_by_key(|(_, order)| *order)?
            .0;
        self.pop(key);
        Some(key)
    }
}
pub(crate) struct Queue {
    budget: Arc<QueueBudget>,
    key: u64,
    waker: AtomicWaker,
    limit: usize,
}
impl Queue {
    pub(crate) fn new(limit: usize, budget: Arc<QueueBudget>) -> Arc<Self> {
        let key = if let Ok(mut state) = budget.inner.lock() {
            let mut key = state.next_queue;
            while state.queues.contains_key(&key) {
                key = key.wrapping_add(1);
            }
            state.next_queue = key.wrapping_add(1);
            state.queues.insert(
                key,
                QueueState {
                    items: VecDeque::new(),
                    closed: false,
                },
            );
            key
        } else {
            0
        };
        Arc::new(Self {
            budget,
            key,
            waker: AtomicWaker::new(),
            limit,
        })
    }
    pub(crate) fn key(&self) -> u64 {
        self.key
    }
    /// Return the affected queue identities for accurate overflow events.
    pub(crate) fn push(&self, item: Result<Value, WireError>) -> BTreeSet<u64> {
        let bytes = item.as_ref().map(notification_bytes).unwrap_or(0);
        let mut dropped = BTreeSet::new();
        let Ok(mut state) = self.budget.inner.lock() else {
            return dropped;
        };
        if state.queues.get(&self.key).is_none_or(|queue| queue.closed) {
            return dropped;
        }
        if bytes > self.budget.limit {
            dropped.insert(self.key);
            return dropped;
        }
        if state
            .queues
            .get(&self.key)
            .is_some_and(|queue| queue.items.len() >= self.limit)
        {
            state.pop(self.key);
            dropped.insert(self.key);
        }
        while state.bytes > self.budget.limit - bytes {
            let Some(key) = state.evict_oldest() else {
                return dropped;
            };
            dropped.insert(key);
        }
        let order = state.next_order;
        state.next_order = state.next_order.saturating_add(1);
        state.bytes += bytes;
        if let Some(queue) = state.queues.get_mut(&self.key) {
            if item.is_err() {
                queue.closed = true;
            }
            queue.items.push_back(Queued { item, bytes, order });
        }
        drop(state);
        self.waker.wake();
        dropped
    }
    pub(crate) fn close(&self) {
        if let Ok(mut state) = self.budget.inner.lock() {
            state.clear(self.key);
            if let Some(queue) = state.queues.get_mut(&self.key) {
                queue.closed = true;
            }
        }
        self.waker.wake();
    }
    pub(crate) fn poll(&self, cx: &mut Context<'_>) -> Poll<Option<Result<Value, WireError>>> {
        self.waker.register(cx.waker());
        let Ok(mut state) = self.budget.inner.lock() else {
            return Poll::Ready(Some(Err(WireError::Rpc(RpcError::Protocol(
                "subscription queue lock poisoned".into(),
            )))));
        };
        if let Some(item) = state.pop(self.key) {
            return Poll::Ready(Some(item.item));
        }
        if state.queues.get(&self.key).is_none_or(|queue| queue.closed) {
            Poll::Ready(None)
        } else {
            Poll::Pending
        }
    }
}
impl Drop for Queue {
    fn drop(&mut self) {
        if let Ok(mut state) = self.budget.inner.lock() {
            state.clear(self.key);
            state.queues.remove(&self.key);
        }
    }
}
/// An active typed subscription. Slow consumers retain the newest updates.
pub struct Subscription<T> {
    pub(crate) session: Session,
    pub(crate) id: String,
    pub(crate) unsubscribe: String,
    pub(crate) queue: Arc<Queue>,
    pub(crate) epoch: u64,
    pub(crate) closed: bool,
    pub(crate) marker: PhantomData<T>,
}
impl<T> Unpin for Subscription<T> {}
impl<T> Subscription<T> {
    /// The normalized server subscription identity.
    pub fn id(&self) -> &str {
        &self.id
    }
    /// Forget local identity before sending the unsubscribe round-trip.
    pub async fn unsubscribe(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        self.queue.close();
        self.session.forget(self.id.clone(), self.epoch);
        self.session
            .unsubscribe_scoped(&self.unsubscribe, &self.id, self.epoch)
            .await;
    }
}
impl<T> Drop for Subscription<T> {
    fn drop(&mut self) {
        if !self.closed {
            self.queue.close();
            self.session.forget(self.id.clone(), self.epoch);
            self.session.unsubscribe_detached(
                self.unsubscribe.clone(),
                self.id.clone(),
                self.epoch,
            );
        }
    }
}
impl<T: DeserializeOwned> Stream for Subscription<T> {
    type Item = Result<T, RpcError>;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        if this.closed {
            return Poll::Ready(None);
        }
        match this.queue.poll(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(None) => {
                this.closed = true;
                Poll::Ready(None)
            }
            Poll::Ready(Some(Ok(value))) => Poll::Ready(Some(
                serde_json::from_value(value)
                    .map_err(|error| RpcError::Protocol(error.to_string())),
            )),
            Poll::Ready(Some(Err(error))) => {
                this.closed = true;
                Poll::Ready(Some(Err(error.into_rpc())))
            }
        }
    }
}
/// Raw subscription stream used by the subxt adapter.
pub(crate) struct RawSubscription(pub(crate) Subscription<Value>);
impl Stream for RawSubscription {
    type Item = Result<Box<RawValue>, subxt_rpcs::Error>;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        if this.0.closed {
            return Poll::Ready(None);
        }
        match this.0.queue.poll(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(None) => {
                this.0.closed = true;
                Poll::Ready(None)
            }
            Poll::Ready(Some(Ok(value))) => Poll::Ready(Some(
                serde_json::value::to_raw_value(&value).map_err(subxt_rpcs::Error::Serialization),
            )),
            Poll::Ready(Some(Err(error))) => {
                this.0.closed = true;
                Poll::Ready(Some(Err(error.into_subxt())))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use serde_json::json;
    use std::task::Waker;

    fn next(queue: &Queue) -> Poll<Option<Result<Value, WireError>>> {
        queue.poll(&mut Context::from_waker(Waker::noop()))
    }
    fn value(queue: &Queue) -> Option<Value> {
        match next(queue) {
            Poll::Ready(Some(Ok(value))) => Some(value),
            _ => None,
        }
    }

    #[test]
    fn retention_charge_counts_real_json_escaping_and_container_storage() {
        let value = json!({"control\n": ["\u{1b}\n\\\"é", false, null, 123]});
        let mut counter = JsonCounter(0);
        serde_json::to_writer(&mut counter, &value).unwrap();
        assert_eq!(counter.0, serde_json::to_vec(&value).unwrap().len());
        assert!(notification_bytes(&value) > counter.0);
        let array = json!([null, null, null, null]);
        assert!(notification_bytes(&array) >= 5 * std::mem::size_of::<Value>());
    }

    #[test]
    fn flat_object_budget_includes_preserve_order_capacity_growth_slack() {
        // 57 entries crosses IndexMap's 56-entry table-growth boundary. Large
        // keys/values are unnecessary: the spare entry/index slots matter here.
        let mut object = serde_json::Map::new();
        for n in 0..57 {
            object.insert(format!("k{n}"), Value::Null);
        }
        let value = Value::Object(object);
        let encoded = serde_json::to_vec(&value).unwrap().len();
        let spare_storage = 57
            * 2
            * (std::mem::size_of::<String>()
                + std::mem::size_of::<Value>()
                + 2 * std::mem::size_of::<usize>());
        assert!(notification_bytes(&value) >= encoded + spare_storage);
        // A wire-only sized budget must refuse this value before retaining it.
        let budget = QueueBudget::new(encoded);
        let queue = Queue::new(2, budget.clone());
        assert_eq!(queue.push(Ok(value)), BTreeSet::from([queue.key()]));
        assert_eq!(budget.retained_bytes(), 0);
    }

    #[test]
    fn active_byte_budget_evicts_oldest_across_streams_and_poll_releases_charge() {
        let first = json!("a".repeat(128));
        let second = json!("b".repeat(128));
        let newest = json!("c".repeat(128));
        let charge = notification_bytes(&first);
        let budget = QueueBudget::new(2 * charge);
        let a = Queue::new(8, budget.clone());
        let b = Queue::new(8, budget.clone());
        assert!(a.push(Ok(first)).is_empty());
        assert!(b.push(Ok(second.clone())).is_empty());
        assert_eq!(a.push(Ok(newest.clone())), BTreeSet::from([a.key()]));
        assert_eq!(budget.retained_bytes(), 2 * charge);
        assert_eq!(value(&b), Some(second));
        assert_eq!(budget.retained_bytes(), charge);
        assert_eq!(value(&a), Some(newest));
        assert_eq!(budget.retained_bytes(), 0);
        assert!(matches!(next(&a), Poll::Pending));
    }

    #[test]
    fn count_cap_retains_newest_and_oversized_value_does_not_flush_useful_updates() {
        let budget = QueueBudget::new(4096);
        let queue = Queue::new(2, budget.clone());
        queue.push(Ok(json!(1)));
        queue.push(Ok(json!(2)));
        assert_eq!(queue.push(Ok(json!(3))), BTreeSet::from([queue.key()]));
        let retained = budget.retained_bytes();
        assert_eq!(
            queue.push(Ok(json!("x".repeat(4096)))),
            BTreeSet::from([queue.key()])
        );
        assert_eq!(budget.retained_bytes(), retained);
        assert_eq!(value(&queue), Some(json!(2)));
        assert_eq!(value(&queue), Some(json!(3)));
        assert_eq!(budget.retained_bytes(), 0);
    }

    #[test]
    fn global_eviction_keeps_terminal_errors_and_close_and_drop_release_storage() {
        let item = json!(7);
        let charge = notification_bytes(&item);
        let budget = QueueBudget::new(charge);
        let old = Queue::new(2, budget.clone());
        old.push(Ok(item.clone()));
        old.push(Err(WireError::Rpc(RpcError::SubscriptionLost)));
        let current = Queue::new(2, budget.clone());
        assert_eq!(current.push(Ok(item.clone())), BTreeSet::from([old.key()]));
        assert!(matches!(
            next(&old),
            Poll::Ready(Some(Err(WireError::Rpc(RpcError::SubscriptionLost))))
        ));
        assert!(matches!(next(&old), Poll::Ready(None)));
        current.close();
        assert_eq!(budget.retained_bytes(), 0);
        assert!(current.push(Ok(item.clone())).is_empty());
        assert!(matches!(next(&current), Poll::Ready(None)));
        let dropped = Queue::new(2, budget.clone());
        dropped.push(Ok(item));
        assert_eq!(budget.retained_bytes(), charge);
        drop(dropped);
        assert_eq!(budget.retained_bytes(), 0);
        assert_eq!(budget.inner.lock().unwrap().queues.len(), 2);
    }

    #[test]
    fn drained_and_closed_queues_release_high_water_backing_allocations() {
        let budget = QueueBudget::new(1024 * 1024);
        let queue = Queue::new(2048, budget.clone());
        for n in 0..1024 {
            queue.push(Ok(json!(n)));
        }
        assert!(budget.retained_bytes() > 0);
        for n in 0..1024 {
            assert_eq!(value(&queue), Some(json!(n)));
        }
        assert_eq!(budget.retained_bytes(), 0);
        assert_eq!(
            budget.inner.lock().unwrap().queues[&queue.key()]
                .items
                .capacity(),
            0
        );
        for n in 0..1024 {
            queue.push(Ok(json!(n)));
        }
        queue.close();
        assert_eq!(budget.retained_bytes(), 0);
        assert_eq!(
            budget.inner.lock().unwrap().queues[&queue.key()]
                .items
                .capacity(),
            0
        );
    }
}
