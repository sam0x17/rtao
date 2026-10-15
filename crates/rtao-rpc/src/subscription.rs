// Derived from RaoFoundation/subtensor sdk/python/bittensor/_transport/rpc.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Bounded subscription queues.
use crate::{RpcError, Session, session::WireError};
use futures::{Stream, task::AtomicWaker};
use serde::de::DeserializeOwned;
use serde_json::{Value, value::RawValue};
use std::{
    collections::VecDeque,
    marker::PhantomData,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
};

pub(crate) struct Queue {
    inner: Mutex<QueueState>,
    waker: AtomicWaker,
    limit: usize,
}
struct QueueState {
    items: VecDeque<Result<Value, WireError>>,
    closed: bool,
}
impl Queue {
    pub(crate) fn new(limit: usize) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(QueueState {
                items: VecDeque::new(),
                closed: false,
            }),
            waker: AtomicWaker::new(),
            limit,
        })
    }
    pub(crate) fn push(&self, item: Result<Value, WireError>) -> bool {
        let Ok(mut state) = self.inner.lock() else {
            return false;
        };
        if state.closed {
            return false;
        }
        let dropped = state.items.len() >= self.limit;
        if dropped {
            state.items.pop_front();
        }
        if item.is_err() {
            state.closed = true;
        }
        state.items.push_back(item);
        drop(state);
        self.waker.wake();
        dropped
    }
    pub(crate) fn close(&self) {
        if let Ok(mut state) = self.inner.lock() {
            state.closed = true;
            state.items.clear();
        }
        self.waker.wake();
    }
    pub(crate) fn poll(&self, cx: &mut Context<'_>) -> Poll<Option<Result<Value, WireError>>> {
        self.waker.register(cx.waker());
        let Ok(mut state) = self.inner.lock() else {
            return Poll::Ready(Some(Err(WireError::Rpc(RpcError::Protocol(
                "subscription queue lock poisoned".into(),
            )))));
        };
        if let Some(item) = state.items.pop_front() {
            return Poll::Ready(Some(item));
        }
        if state.closed {
            Poll::Ready(None)
        } else {
            Poll::Pending
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
