use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex},
};

use futures::StreamExt;
use serde_json::{Value, value::RawValue};
use subxt_rpcs::{
    Error, UserError,
    client::{RawRpcFuture, RawRpcSubscription, RpcClientT},
};
use tokio::sync::mpsc;

use super::{FakeState, Notification, RpcFailure, RpcRequest, node::lock};

/// A direct `RpcClientT` transport over the same encoded state as [`super::FakeNode`].
#[derive(Clone)]
pub struct FakeChain {
    state: Arc<Mutex<FakeState>>,
    subscribers: Arc<Mutex<Vec<Subscriber>>>,
    transport_errors: Arc<Mutex<BTreeMap<String, VecDeque<Error>>>>,
}

struct Subscriber {
    id: Option<String>,
    method: String,
    sender: mpsc::UnboundedSender<Result<Box<RawValue>, Error>>,
}

struct SubscriptionGuard {
    state: Arc<Mutex<FakeState>>,
    subscribers: Arc<Mutex<Vec<Subscriber>>>,
    sender: mpsc::UnboundedSender<Result<Box<RawValue>, Error>>,
    id: Option<String>,
    unsubscribe: String,
}

impl Drop for SubscriptionGuard {
    fn drop(&mut self) {
        let mut state = lock(&self.state);
        lock(&self.subscribers).retain(|subscriber| !subscriber.sender.same_channel(&self.sender));
        if let Some(id) = &self.id {
            let request = RpcRequest {
                id: Value::from(0),
                method: self.unsubscribe.clone(),
                params: Value::Array(vec![Value::String(id.clone())]),
            };
            let reply = state.dispatch(&request);
            let _ = route_notifications(&self.subscribers, &reply.notifications);
        }
    }
}

impl FakeChain {
    /// Construct a direct transport sharing state with a socket node.
    pub fn new(state: Arc<Mutex<FakeState>>) -> Self {
        Self {
            state,
            subscribers: Arc::new(Mutex::new(Vec::new())),
            transport_errors: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    /// Construct a direct transport from a state owned by the caller.
    pub fn from_state(state: FakeState) -> Self {
        Self::new(Arc::new(Mutex::new(state)))
    }

    /// Access the chain state for seeding or assertions.
    pub fn state(&self) -> Arc<Mutex<FakeState>> {
        self.state.clone()
    }

    /// Change chain data or handlers atomically.
    pub fn mutate_state<R>(&self, update: impl FnOnce(&mut FakeState) -> R) -> R {
        update(&mut lock(&self.state))
    }

    /// Every request recorded by direct or socket clients sharing this state.
    pub fn requests(&self) -> Vec<RpcRequest> {
        lock(&self.state).requests.clone()
    }

    /// Publish a head or status update to all active direct subscriptions.
    ///
    /// Use [`Self::publish_to`] to target one subscription by its ID.
    pub fn publish(&self, result: Value) -> Result<(), serde_json::Error> {
        let _state = lock(&self.state);
        publish_matching(&self.subscribers, |_| true, &result)
    }

    /// Publish an update only to the subscription with the given ID.
    pub fn publish_to(&self, id: &str, result: Value) -> Result<(), serde_json::Error> {
        let _state = lock(&self.state);
        publish_matching(
            &self.subscribers,
            |subscriber| subscriber.id.as_deref() == Some(id),
            &result,
        )
    }

    /// Publish an update to subscriptions created with this method.
    pub fn publish_method(&self, method: &str, result: Value) -> Result<(), serde_json::Error> {
        let _state = lock(&self.state);
        publish_matching(
            &self.subscribers,
            |subscriber| subscriber.method == method,
            &result,
        )
    }

    /// Return a client-side transport error on the next call to this method.
    ///
    /// A caller can supply `Error::Client(Box::new(SubmitError::FateUnknown { .. }))`
    /// to exercise the real executor's error downcast and nonce handling.
    pub fn queue_transport_error(&self, method: impl Into<String>, error: Error) {
        lock(&self.transport_errors)
            .entry(method.into())
            .or_default()
            .push_back(error);
    }

    fn next_transport_error(&self, method: &str) -> Option<Error> {
        lock(&self.transport_errors)
            .get_mut(method)
            .and_then(VecDeque::pop_front)
    }

    /// Advance the best chain and publish each new header to active subscriptions.
    pub fn advance(&self, count: u64) -> Result<(), serde_json::Error> {
        let mut state = lock(&self.state);
        let previous_finalized = state.finalized;
        let headers = state.advance(count);
        let finalized_headers = if previous_finalized < state.finalized {
            (previous_finalized + 1..=state.finalized)
                .filter_map(|number| {
                    state
                        .block_hash(number)
                        .and_then(|hash| state.header(&hash))
                })
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        for header in headers {
            publish_matching(
                &self.subscribers,
                |subscriber| subscriber.method == "chain_subscribeNewHeads",
                &header,
            )?;
            publish_matching(
                &self.subscribers,
                |subscriber| subscriber.method == "chain_subscribeAllHeads",
                &header,
            )?;
        }
        for header in finalized_headers {
            publish_matching(
                &self.subscribers,
                |subscriber| subscriber.method == "chain_subscribeFinalizedHeads",
                &header,
            )?;
        }
        Ok(())
    }

    fn dispatch_request(&self, request: &RpcRequest) -> Result<Option<Box<RawValue>>, Error> {
        let mut state = lock(&self.state);
        let reply = state.dispatch(request);
        route_notifications(&self.subscribers, &reply.notifications)
            .map_err(Error::Serialization)?;
        match reply.outcome {
            Some(Ok(result)) => serde_json::value::to_raw_value(&result)
                .map(Some)
                .map_err(Error::Serialization),
            Some(Err(error)) => Err(rpc_failure(error)),
            None => Ok(None),
        }
    }

    fn dispatch_subscription(
        &self,
        request: &RpcRequest,
        unsub: &str,
    ) -> Result<Option<RawRpcSubscription>, Error> {
        let mut state = lock(&self.state);
        let reply = state.dispatch(request);
        let id = match reply.outcome {
            Some(Ok(result)) => result
                .as_str()
                .map(str::to_owned)
                .or_else(|| result.as_u64().map(|id| id.to_string())),
            Some(Err(error)) => {
                route_notifications(&self.subscribers, &reply.notifications)
                    .map_err(Error::Serialization)?;
                return Err(rpc_failure(error));
            }
            None => {
                route_notifications(&self.subscribers, &reply.notifications)
                    .map_err(Error::Serialization)?;
                return Ok(None);
            }
        };
        let (sender, receiver) = mpsc::unbounded_channel();
        lock(&self.subscribers).push(Subscriber {
            id: id.clone(),
            method: request.method.clone(),
            sender: sender.clone(),
        });
        route_notifications(&self.subscribers, &reply.notifications)
            .map_err(Error::Serialization)?;
        let guard = SubscriptionGuard {
            state: self.state.clone(),
            subscribers: self.subscribers.clone(),
            sender,
            id: id.clone(),
            unsubscribe: unsub.to_owned(),
        };
        let stream =
            futures::stream::unfold((receiver, guard), |(mut receiver, guard)| async move {
                receiver.recv().await.map(|item| (item, (receiver, guard)))
            })
            .boxed();
        Ok(Some(RawRpcSubscription { stream, id }))
    }
}

fn publish_matching(
    subscribers: &Mutex<Vec<Subscriber>>,
    matches: impl Fn(&Subscriber) -> bool,
    result: &Value,
) -> Result<(), serde_json::Error> {
    let raw = serde_json::value::to_raw_value(result)?;
    lock(subscribers).retain(|subscriber| {
        if matches(subscriber) {
            subscriber.sender.send(Ok(raw.clone())).is_ok()
        } else {
            !subscriber.sender.is_closed()
        }
    });
    Ok(())
}

fn route_notifications(
    subscribers: &Mutex<Vec<Subscriber>>,
    notifications: &[Notification],
) -> Result<(), serde_json::Error> {
    for notification in notifications {
        publish_matching(
            subscribers,
            |subscriber| subscriber.id.as_deref() == Some(&notification.subscription),
            &notification.result,
        )?;
    }
    Ok(())
}

impl Default for FakeChain {
    fn default() -> Self {
        Self::from_state(FakeState::default())
    }
}

fn raw_params(params: Option<Box<RawValue>>) -> Result<Value, Error> {
    params.map_or(Ok(Value::Array(Vec::new())), |params| {
        serde_json::from_str(params.get()).map_err(Error::Deserialization)
    })
}

fn rpc_failure(error: RpcFailure) -> Error {
    let Ok(code) = i32::try_from(error.code) else {
        return Error::Client(Box::new(std::io::Error::other(
            "scripted JSON-RPC error code does not fit i32",
        )));
    };
    let data = match error
        .data
        .as_ref()
        .map(serde_json::value::to_raw_value)
        .transpose()
    {
        Ok(data) => data,
        Err(error) => return Error::Serialization(error),
    };
    Error::User(UserError {
        code,
        message: error.message,
        data,
    })
}

impl RpcClientT for FakeChain {
    fn request_raw<'a>(
        &'a self,
        method: &'a str,
        params: Option<Box<RawValue>>,
    ) -> RawRpcFuture<'a, Box<RawValue>> {
        Box::pin(async move {
            let request = RpcRequest {
                id: Value::from(0),
                method: method.to_owned(),
                params: raw_params(params)?,
            };
            if let Some(error) = self.next_transport_error(method) {
                lock(&self.state).requests.push(request);
                return Err(error);
            }
            match self.dispatch_request(&request)? {
                Some(result) => Ok(result),
                None => futures::future::pending().await,
            }
        })
    }

    fn subscribe_raw<'a>(
        &'a self,
        sub: &'a str,
        params: Option<Box<RawValue>>,
        unsub: &'a str,
    ) -> RawRpcFuture<'a, RawRpcSubscription> {
        Box::pin(async move {
            let request = RpcRequest {
                id: Value::from(0),
                method: sub.to_owned(),
                params: raw_params(params)?,
            };
            if let Some(error) = self.next_transport_error(sub) {
                lock(&self.state).requests.push(request);
                return Err(error);
            }
            match self.dispatch_subscription(&request, unsub)? {
                Some(subscription) => Ok(subscription),
                None => futures::future::pending().await,
            }
        })
    }
}
