// Derived from RaoFoundation/subtensor sdk/python/bittensor/_transport/rpc.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/_substrate.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Single-owner WebSocket supervision and request correlation.
use crate::{
    policy,
    subscription::{Queue, QueueBudget, RawSubscription, notification_bytes},
    *,
};
use blake2::{Blake2b, Digest, digest::consts::U32};
use futures::{Sink, SinkExt, Stream, StreamExt};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json, value::RawValue};
use std::{
    collections::{BTreeMap, VecDeque},
    marker::PhantomData,
    str::FromStr,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use subxt_rpcs::client::{RawRpcFuture, RawRpcSubscription, RpcClientT};
use tokio::{
    sync::{broadcast, mpsc, oneshot, watch},
    time::{Instant, sleep, sleep_until, timeout},
};
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
#[cfg(any(test, feature = "fuzzing"))]
use url::Url;
mod handshake;
use handshake::{Socket, connect_socket};
mod ownership;
pub use ownership::{AuthorOwnership, AuthorRetirement};
use ownership::{Custody, OwnedSocket, retire_socket};
type TransportSocket = OwnedSocket<Socket>;
type Response = Result<Reply, WireError>;

#[cfg(any(test, feature = "fuzzing"))]
mod frame_fuzz;
#[cfg(feature = "fuzzing")]
pub use frame_fuzz::fuzz_json_rpc_frame;

#[derive(Clone, Debug)]
pub(crate) enum WireError {
    Rpc(RpcError),
    Submit(SubmitError),
}
impl WireError {
    pub(crate) fn into_rpc(self) -> RpcError {
        match self {
            Self::Rpc(error) => error,
            Self::Submit(SubmitError::Rpc(error)) => error,
            Self::Submit(SubmitError::FateUnknown { hash }) => RpcError::FateUnknown { hash },
        }
    }
    pub(crate) fn into_subxt(self) -> subxt_rpcs::Error {
        match self {
            Self::Rpc(error) => subxt_rpcs::Error::Client(Box::new(error)),
            Self::Submit(error) => subxt_rpcs::Error::Client(Box::new(error)),
        }
    }
    fn into_submit(self) -> SubmitError {
        match self {
            Self::Rpc(error) => SubmitError::Rpc(error),
            Self::Submit(error) => error,
        }
    }
}
struct Reply {
    value: Value,
    queue: Option<Arc<Queue>>,
    epoch: u64,
}
struct Pending {
    id: u64,
    group: u64,
    frame: Value,
    method: String,
    sender: oneshot::Sender<Response>,
    sent: bool,
    subscription: Option<SubscriptionRequest>,
    hash: Option<H256>,
    started: Instant,
    waited: Duration,
    required_epoch: Option<u64>,
    last_sent_epoch: u64,
    custody: Option<Arc<Custody>>,
}
struct SubscriptionRequest {
    unsubscribe: String,
}
struct ActiveSubscription {
    queue: Arc<Queue>,
    hash: Option<H256>,
    request_id: u64,
    unsubscribe: String,
}
enum Command {
    Request(Vec<Pending>),
    Cancel(Vec<u64>),
    Forget { id: String, epoch: u64 },
    Unsubscribe { frame: Value, epoch: u64 },
    Shutdown(Option<oneshot::Sender<()>>),
    Snapshot(oneshot::Sender<SessionStatistics>),
}
struct RequestGuard {
    commands: mpsc::UnboundedSender<Command>,
    ids: Vec<u64>,
}
impl Drop for RequestGuard {
    fn drop(&mut self) {
        let _ = self
            .commands
            .send(Command::Cancel(std::mem::take(&mut self.ids)));
    }
}
struct Inner {
    commands: mpsc::UnboundedSender<Command>,
    state: watch::Receiver<SessionState>,
    events: broadcast::Sender<SessionEvent>,
    ids: AtomicU64,
    archive: ArchivePool,
}
impl Drop for Inner {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Shutdown(None));
    }
}
/// A cloneable session; dropping its last owner stops its supervisor and socket.
#[derive(Clone)]
pub struct Session {
    inner: Arc<Inner>,
}
impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = self.inner.state.borrow();
        let mut debug = f.debug_struct("Session");
        match &*state {
            SessionState::Connected { url } => {
                debug.field("endpoint", &policy::safe_endpoint(url.as_str()));
            }
            state => {
                debug.field("state", state);
            }
        }
        debug.finish_non_exhaustive()
    }
}
/// Lazy archive-session pool used to retry discarded historical reads once.
pub struct ArchivePool {
    config: SessionConfig,
    session: tokio::sync::OnceCell<Session>,
    closing: watch::Sender<bool>,
}
impl ArchivePool {
    /// Whether archive endpoints are configured.
    pub fn is_empty(&self) -> bool {
        self.config.pool.urls.is_empty()
    }
    async fn session(&self) -> Result<&Session, RpcError> {
        let mut closing = self.closing.subscribe();
        if *closing.borrow() {
            return Err(RpcError::Closed);
        }
        if self.is_empty() {
            return Err(RpcError::Protocol("no archive endpoint configured".into()));
        }
        tokio::select! {
            biased;
            _=closing.changed()=>Err(RpcError::Closed),
            session=self.session.get_or_try_init(||Session::connect(self.config.clone()))=>session,
        }
    }
    /// Read directly from the lazy archive pool, without another archive retry.
    pub async fn request<R: DeserializeOwned>(
        &self,
        method: ReadMethod,
        params: impl Serialize + Send,
    ) -> Result<R, RpcError> {
        let value = serde_json::to_value(params).map_err(|e| RpcError::Protocol(e.to_string()))?;
        let raw = self
            .session()
            .await?
            .call(method.as_str(), value, None)
            .await
            .map_err(WireError::into_rpc)?;
        serde_json::from_value(raw.value).map_err(|e| RpcError::Protocol(e.to_string()))
    }
}
impl Session {
    /// Start the supervisor and wait until connected or terminally refused.
    pub async fn connect(config: SessionConfig) -> Result<Self, RpcError> {
        validate(&config)?;
        let mut archive_config = config.clone();
        archive_config.pool.urls = config.pool.archive.clone();
        archive_config.pool.archive.clear();
        let tls =
            tls_config_for_endpoints(&config, || rustls_native_certs::load_native_certs().certs)?;
        let (commands, receiver) = mpsc::unbounded_channel();
        let (state, state_rx) = watch::channel(SessionState::Reconnecting { attempt: 0 });
        let (events, _) = broadcast::channel(256);
        let session = Self {
            inner: Arc::new(Inner {
                commands,
                state: state_rx,
                events: events.clone(),
                ids: AtomicU64::new(1),
                archive: ArchivePool {
                    config: archive_config,
                    session: tokio::sync::OnceCell::new(),
                    closing: watch::channel(false).0,
                },
            }),
        };
        tokio::spawn(Supervisor::new(config, tls, receiver, state, events).run());
        let mut states = session.state();
        loop {
            let current = states.borrow_and_update().clone();
            match current {
                SessionState::Connected { .. } => return Ok(session),
                SessionState::Failed { error } => return Err(error),
                SessionState::Closed => return Err(RpcError::Closed),
                _ => {}
            }
            if states.changed().await.is_err() {
                return Err(RpcError::Closed);
            }
        }
    }
    /// Execute a resend-safe read, retrying discarded state once on an archive.
    pub async fn request<R: DeserializeOwned>(
        &self,
        method: ReadMethod,
        params: impl Serialize + Send,
    ) -> Result<R, RpcError> {
        let value = serde_json::to_value(params).map_err(|e| RpcError::Protocol(e.to_string()))?;
        let reply = match self.call(method.as_str(), value.clone(), None).await {
            Err(WireError::Rpc(RpcError::StateDiscarded { .. })) if !self.archive().is_empty() => {
                self.archive()
                    .session()
                    .await?
                    .call(method.as_str(), value, None)
                    .await
            }
            result => result,
        }
        .map_err(WireError::into_rpc)?;
        serde_json::from_value(reply.value).map_err(|e| RpcError::Protocol(e.to_string()))
    }
    /// Execute a raw method. Unknown methods fail if a transmitted frame loses its connection.
    pub async fn request_method<R: DeserializeOwned>(
        &self,
        method: &str,
        params: impl Serialize + Send,
    ) -> Result<R, RpcError> {
        let value = serde_json::to_value(params).map_err(|e| RpcError::Protocol(e.to_string()))?;
        let reply = self
            .call(method, value, None)
            .await
            .map_err(WireError::into_rpc)?;
        serde_json::from_value(reply.value).map_err(|e| RpcError::Protocol(e.to_string()))
    }
    /// Execute a read with a caller deadline; expiration cancels its pending entry.
    pub async fn request_with_timeout<R: DeserializeOwned>(
        &self,
        method: ReadMethod,
        params: impl Serialize + Send,
        deadline: Duration,
    ) -> Result<R, RpcError> {
        timeout(deadline, self.request(method, params))
            .await
            .map_err(|_| RpcError::Timeout {
                detail: format!(
                    "RPC request deadline expired after {}s",
                    deadline.as_secs_f64()
                ),
            })?
    }
    /// Send a JSON-RPC batch and return individual results in request order.
    pub async fn request_many(
        &self,
        calls: Vec<(ReadMethod, Value)>,
    ) -> Result<Vec<Result<Box<RawValue>, RpcError>>, RpcError> {
        if let Some(error) = self.terminal_error("") {
            return Err(error.into_rpc());
        }
        if calls.is_empty() {
            return Ok(Vec::new());
        }
        let retry_calls = calls.clone();
        let group = self.next_id();
        let mut receivers = Vec::with_capacity(calls.len());
        let mut pending = Vec::with_capacity(calls.len());
        for (method, params) in calls {
            let (entry, receiver) = self.pending(method.as_str(), params, None, group)?;
            pending.push(entry);
            receivers.push(receiver);
        }
        let mut guard = RequestGuard {
            commands: self.inner.commands.clone(),
            ids: pending.iter().map(|entry| entry.id).collect(),
        };
        self.inner
            .commands
            .send(Command::Request(pending))
            .map_err(|_| RpcError::Closed)?;
        let responses = futures::future::join_all(receivers).await;
        guard.ids.clear();
        let mut results = Vec::with_capacity(responses.len());
        for (response, (method, params)) in responses.into_iter().zip(retry_calls) {
            let reply = match response {
                Ok(reply) => reply,
                Err(_) => Err(self
                    .terminal_error(method.as_str())
                    .unwrap_or(WireError::Rpc(RpcError::Closed))),
            };
            let reply = match reply {
                Err(WireError::Rpc(RpcError::StateDiscarded { .. }))
                    if !self.archive().is_empty() =>
                {
                    match self.archive().session().await {
                        Ok(archive) => archive.call(method.as_str(), params, None).await,
                        Err(error) => Err(WireError::Rpc(error)),
                    }
                }
                reply => reply,
            };
            results.push(match reply {
                Ok(reply) => serde_json::value::to_raw_value(&reply.value)
                    .map_err(|error| RpcError::Protocol(error.to_string())),
                Err(error) => Err(error.into_rpc()),
            });
        }
        Ok(results)
    }
    /// Subscribe to a standard legacy stream; items decode its result field.
    pub async fn subscribe<T: DeserializeOwned>(
        &self,
        kind: SubscriptionKind,
    ) -> Result<Subscription<T>, RpcError> {
        let (sub, unsub) = kind.methods();
        self.subscribe_method(sub, Value::Array(Vec::new()), unsub)
            .await
    }
    /// Subscribe to a named legacy method, decoding notification result fields.
    pub async fn subscribe_method<T: DeserializeOwned>(
        &self,
        method: &str,
        params: Value,
        unsubscribe: &str,
    ) -> Result<Subscription<T>, RpcError> {
        self.subscription(method, params, unsubscribe)
            .await
            .map_err(WireError::into_rpc)
    }
    /// Submit an extrinsic exactly once after it is transmitted.
    pub async fn submit(&self, extrinsic: &[u8]) -> Result<H256, SubmitError> {
        let reply = self
            .call(
                "author_submitExtrinsic",
                json!([format!("0x{}", hex::encode(extrinsic))]),
                None,
            )
            .await
            .map_err(WireError::into_submit)?;
        serde_json::from_value(reply.value)
            .map_err(|e| SubmitError::Rpc(RpcError::Protocol(e.to_string())))
    }
    /// Submit and watch without resubscribing an extrinsic after connection loss.
    pub async fn submit_and_watch(
        &self,
        extrinsic: &[u8],
    ) -> Result<Subscription<TxStatus>, SubmitError> {
        self.subscription(
            "author_submitAndWatchExtrinsic",
            json!([format!("0x{}", hex::encode(extrinsic))]),
            "author_unwatchExtrinsic",
        )
        .await
        .map_err(WireError::into_submit)
    }
    /// Submit exact signed bytes while retaining strong ownership through every
    /// native queue/frame reference. Local retirement is not chain finality.
    pub async fn submit_owned(
        &self,
        extrinsic: &[u8],
        ownership: AuthorOwnership,
    ) -> Result<H256, SubmitError> {
        let custody = ownership.for_bytes(extrinsic)?;
        let reply = self
            .call_with_ownership(
                "author_submitExtrinsic",
                json!([format!("0x{}", hex::encode(extrinsic))]),
                None,
                None,
                Some(custody),
            )
            .await
            .map_err(WireError::into_submit)?;
        serde_json::from_value(reply.value)
            .map_err(|e| SubmitError::Rpc(RpcError::Protocol(e.to_string())))
    }
    /// Submit and watch once with native queue/write ownership. Dropping the
    /// outer future cannot release captured-write ownership prematurely.
    pub async fn submit_and_watch_owned(
        &self,
        extrinsic: &[u8],
        ownership: AuthorOwnership,
    ) -> Result<Subscription<TxStatus>, SubmitError> {
        let custody = ownership.for_bytes(extrinsic)?;
        self.subscription_with_ownership(
            "author_submitAndWatchExtrinsic",
            json!([format!("0x{}", hex::encode(extrinsic))]),
            "author_unwatchExtrinsic",
            Some(custody),
        )
        .await
        .map_err(WireError::into_submit)
    }
    /// Watch connection state changes.
    pub fn state(&self) -> watch::Receiver<SessionState> {
        self.inner.state.clone()
    }
    /// Receive bounded connection events.
    pub fn events(&self) -> broadcast::Receiver<SessionEvent> {
        self.inner.events.subscribe()
    }
    /// Lazy archive pool.
    pub fn archive(&self) -> &ArchivePool {
        &self.inner.archive
    }
    /// Inspect bounded routing counts without accessing private supervisor state.
    pub async fn statistics(&self) -> Result<SessionStatistics, RpcError> {
        let (sender, receiver) = oneshot::channel();
        self.inner
            .commands
            .send(Command::Snapshot(sender))
            .map_err(|_| RpcError::Closed)?;
        receiver.await.map_err(|_| RpcError::Closed)
    }
    /// Close the socket, fail pending callers, and end subscriptions.
    pub async fn close(&self) {
        self.archive().closing.send_replace(true);
        let (sender, receiver) = oneshot::channel();
        if self
            .inner
            .commands
            .send(Command::Shutdown(Some(sender)))
            .is_ok()
        {
            let _ = receiver.await;
        }
        if let Some(archive) = self.archive().session.get() {
            Box::pin(archive.close()).await;
        }
    }
    pub(crate) fn forget(&self, id: String, epoch: u64) {
        let _ = self.inner.commands.send(Command::Forget { id, epoch });
    }
    pub(crate) fn unsubscribe_detached(&self, method: String, id: String, epoch: u64) {
        let frame = json!({"jsonrpc":"2.0","id":self.next_id(),"method":method,"params":[id]});
        let _ = self
            .inner
            .commands
            .send(Command::Unsubscribe { frame, epoch });
    }
    fn next_id(&self) -> u64 {
        self.inner.ids.fetch_add(1, Ordering::Relaxed)
    }
    fn pending(
        &self,
        method: &str,
        params: Value,
        unsubscribe: Option<&str>,
        group: u64,
    ) -> Result<(Pending, oneshot::Receiver<Response>), RpcError> {
        if !params.is_array() {
            return Err(RpcError::Protocol(
                "JSON-RPC params must be an array".into(),
            ));
        }
        let id = self.next_id();
        let (sender, receiver) = oneshot::channel();
        let hash = if generated::py_rpc::NON_IDEMPOTENT_METHODS.contains(&method) {
            params
                .get(0)
                .and_then(Value::as_str)
                .and_then(|s| hex::decode(s.strip_prefix("0x").unwrap_or(s)).ok())
                .map(|bytes| H256::from_slice(&Blake2b::<U32>::digest(bytes)))
        } else {
            None
        };
        Ok((
            Pending {
                id,
                group,
                frame: json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}),
                method: method.into(),
                sender,
                sent: false,
                subscription: unsubscribe.map(|method| SubscriptionRequest {
                    unsubscribe: method.into(),
                }),
                hash,
                started: Instant::now(),
                waited: Duration::ZERO,
                required_epoch: None,
                last_sent_epoch: 0,
                custody: None,
            },
            receiver,
        ))
    }
    async fn call(
        &self,
        method: &str,
        params: Value,
        unsubscribe: Option<&str>,
    ) -> Result<Reply, WireError> {
        self.call_scoped(method, params, unsubscribe, None).await
    }
    async fn call_scoped(
        &self,
        method: &str,
        params: Value,
        unsubscribe: Option<&str>,
        epoch: Option<u64>,
    ) -> Result<Reply, WireError> {
        self.call_with_ownership(method, params, unsubscribe, epoch, None)
            .await
    }
    async fn call_with_ownership(
        &self,
        method: &str,
        params: Value,
        unsubscribe: Option<&str>,
        epoch: Option<u64>,
        custody: Option<Arc<Custody>>,
    ) -> Result<Reply, WireError> {
        if let Some(error) = self.terminal_error(method) {
            return Err(error);
        }
        let group = self.next_id();
        let (mut pending, receiver) = self
            .pending(method, params, unsubscribe, group)
            .map_err(WireError::Rpc)?;
        pending.required_epoch = epoch;
        pending.custody = custody;
        let mut guard = RequestGuard {
            commands: self.inner.commands.clone(),
            ids: vec![pending.id],
        };
        self.inner
            .commands
            .send(Command::Request(vec![pending]))
            .map_err(|_| WireError::Rpc(RpcError::Closed))?;
        let result = receiver.await.map_err(|_| {
            self.terminal_error(method)
                .unwrap_or(WireError::Rpc(RpcError::Closed))
        })?;
        if result.is_ok() {
            guard.ids.clear();
        }
        result
    }
    pub(crate) async fn unsubscribe_scoped(&self, method: &str, id: &str, epoch: u64) {
        let _ = self
            .call_scoped(method, json!([id]), None, Some(epoch))
            .await;
    }
    fn terminal_error(&self, method: &str) -> Option<WireError> {
        let error = match &*self.inner.state.borrow() {
            SessionState::Failed { error } => error.clone(),
            SessionState::Closed => RpcError::Closed,
            _ => return None,
        };
        Some(
            if generated::py_rpc::NON_IDEMPOTENT_METHODS.contains(&method) {
                WireError::Submit(SubmitError::Rpc(error))
            } else {
                WireError::Rpc(error)
            },
        )
    }
    async fn subscription<T>(
        &self,
        method: &str,
        params: Value,
        unsubscribe: &str,
    ) -> Result<Subscription<T>, WireError> {
        self.subscription_with_ownership(method, params, unsubscribe, None)
            .await
    }
    async fn subscription_with_ownership<T>(
        &self,
        method: &str,
        params: Value,
        unsubscribe: &str,
        custody: Option<Arc<Custody>>,
    ) -> Result<Subscription<T>, WireError> {
        let reply = self
            .call_with_ownership(method, params, Some(unsubscribe), None, custody)
            .await?;
        let id = subscription_id(&reply.value).ok_or_else(|| {
            WireError::Rpc(RpcError::Protocol(format!(
                "unexpected subscription id: {}",
                reply.value
            )))
        })?;
        let queue = reply.queue.ok_or_else(|| {
            WireError::Rpc(RpcError::Protocol(
                "subscription response has no routing queue".into(),
            ))
        })?;
        Ok(Subscription {
            session: self.clone(),
            id,
            unsubscribe: unsubscribe.into(),
            queue,
            epoch: reply.epoch,
            closed: false,
            marker: PhantomData,
        })
    }
}
impl RpcClientT for Session {
    fn request_raw<'a>(
        &'a self,
        method: &'a str,
        params: Option<Box<RawValue>>,
    ) -> RawRpcFuture<'a, Box<RawValue>> {
        Box::pin(async move {
            let params = parse_params(params)?;
            let reply = match self.call(method, params.clone(), None).await {
                Err(WireError::Rpc(RpcError::StateDiscarded { .. }))
                    if ReadMethod::from_str(method).is_ok() && !self.archive().is_empty() =>
                {
                    self.archive()
                        .session()
                        .await
                        .map_err(|error| subxt_rpcs::Error::Client(Box::new(error)))?
                        .call(method, params, None)
                        .await
                }
                result => result,
            }
            .map_err(WireError::into_subxt)?;
            serde_json::value::to_raw_value(&reply.value).map_err(subxt_rpcs::Error::Serialization)
        })
    }
    fn subscribe_raw<'a>(
        &'a self,
        method: &'a str,
        params: Option<Box<RawValue>>,
        unsubscribe: &'a str,
    ) -> RawRpcFuture<'a, RawRpcSubscription> {
        Box::pin(async move {
            let params = parse_params(params)?;
            let subscription = self
                .subscription(method, params, unsubscribe)
                .await
                .map_err(WireError::into_subxt)?;
            let id = Some(subscription.id.clone());
            Ok(RawRpcSubscription {
                stream: Box::pin(RawSubscription(subscription)),
                id,
            })
        })
    }
}
fn parse_params(raw: Option<Box<RawValue>>) -> Result<Value, subxt_rpcs::Error> {
    raw.map(|value| serde_json::from_str(value.get()).map_err(subxt_rpcs::Error::Serialization))
        .unwrap_or_else(|| Ok(json!([])))
}
fn subscription_id(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) if n.as_i64().is_some() || n.as_u64().is_some() => Some(n.to_string()),
        _ => None,
    }
}

/// Log-only escaping: the parser and WebSocket send path keep the original bytes.
struct DiagnosticFrame<'a>(&'a str);
impl std::fmt::Display for DiagnosticFrame<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use std::fmt::Write;
        for ch in self.0.chars() {
            if ch.is_control() {
                write!(f, "{}", ch.escape_default())?;
            } else {
                f.write_char(ch)?;
            }
        }
        Ok(())
    }
}

struct BufferedNotification {
    value: Value,
    bytes: usize,
    order: u128,
}

struct Supervisor {
    config: SessionConfig,
    tls: Arc<rustls::ClientConfig>,
    commands: mpsc::UnboundedReceiver<Command>,
    state: watch::Sender<SessionState>,
    events: broadcast::Sender<SessionEvent>,
    pending: BTreeMap<u64, Pending>,
    subscriptions: BTreeMap<String, ActiveSubscription>,
    early: VecDeque<(String, VecDeque<BufferedNotification>)>,
    early_bytes: usize,
    early_order: u128,
    subscription_budget: Arc<QueueBudget>,
    closed_ids: VecDeque<String>,
    detached: VecDeque<(u64, Value)>,
    epoch: u64,
    last_inbound: Instant,
    #[cfg(feature = "metrics")]
    gauge_reported: std::sync::atomic::AtomicUsize,
    index: usize,
    failures: u32,
    bucket: Bucket,
    shutdown_ack: Option<oneshot::Sender<()>>,
    stopping: bool,
}
impl Supervisor {
    fn new(
        config: SessionConfig,
        tls: Arc<rustls::ClientConfig>,
        commands: mpsc::UnboundedReceiver<Command>,
        state: watch::Sender<SessionState>,
        events: broadcast::Sender<SessionEvent>,
    ) -> Self {
        let bucket = Bucket::new(config.rate_limit);
        let subscription_budget = QueueBudget::new(config.subscription_buffer_bytes);
        Self {
            config,
            tls,
            commands,
            state,
            events,
            pending: BTreeMap::new(),
            subscriptions: BTreeMap::new(),
            early: VecDeque::new(),
            early_bytes: 0,
            early_order: 0,
            subscription_budget,
            closed_ids: VecDeque::new(),
            detached: VecDeque::new(),
            epoch: 0,
            last_inbound: Instant::now(),
            #[cfg(feature = "metrics")]
            gauge_reported: std::sync::atomic::AtomicUsize::new(0),
            index: 0,
            failures: 0,
            bucket,
            shutdown_ack: None,
            stopping: false,
        }
    }
    async fn run(mut self) {
        loop {
            if self.stopping {
                self.finish(RpcError::Closed);
                return;
            }
            let mut socket = match self.dial().await {
                Ok(socket) => socket,
                Err(error) => {
                    self.finish(error);
                    return;
                }
            };
            self.epoch = self.epoch.wrapping_add(1);
            self.last_inbound = Instant::now();
            if let Err(error) = self.resend(&mut socket).await {
                retire_socket(socket).await;
                if let Err(error) = self.record_loss(error) {
                    self.finish(error);
                    return;
                }
                if !self.backoff().await {
                    self.finish(RpcError::Closed);
                    return;
                }
                continue;
            }
            // Discovery does not carry a user request and its result is diagnostic only.
            let discovery = json!({"jsonrpc":"2.0","id":0,"method":"rpc_methods","params":[]});
            let discovery_result = async {
                self.wait_for_token(&mut socket).await?;
                self.write_frame(&mut socket, discovery).await
            }
            .await;
            if let Err(error) = discovery_result {
                retire_socket(socket).await;
                if let Err(error) = self.record_loss(error) {
                    self.finish(error);
                    return;
                }
                if !self.backoff().await {
                    self.finish(RpcError::Closed);
                    return;
                }
                continue;
            }
            let endpoint = policy::safe_endpoint(self.config.pool.urls[self.index].as_str());
            self.state.send_replace(SessionState::Connected {
                url: self.config.pool.urls[self.index].clone(),
            });
            let _ = self.events.send(SessionEvent::Connected { endpoint });
            let reason = self.pump(&mut socket).await;
            retire_socket(socket).await;
            if self.stopping {
                self.finish(RpcError::Closed);
                return;
            }
            if matches!(reason, RpcError::Policy(_)) {
                self.finish(reason);
                return;
            }
            if let Err(error) = self.record_loss(reason) {
                self.finish(error);
                return;
            }
            if !self.backoff().await {
                self.finish(RpcError::Closed);
                return;
            }
        }
    }
    async fn dial(&mut self) -> Result<TransportSocket, RpcError> {
        let mut cycle = 0u32;
        loop {
            let mut last_error = RpcError::Closed;
            for _ in 0..self.config.pool.urls.len() {
                self.state.send_replace(SessionState::Reconnecting {
                    attempt: self.failures,
                });
                let config = self.config.clone();
                let url = config.pool.urls[self.index].clone();
                let tls = self.tls.clone();
                let connect = connect_socket(&config, &url, tls);
                tokio::pin!(connect);
                let result = loop {
                    tokio::select! {result=&mut connect=>break result, command=self.commands.recv()=>{
                        self.command(command);if self.stopping {return Err(RpcError::Closed);}
                    }}
                };
                match result {
                    Ok(socket) => return Ok(OwnedSocket::new(socket)),
                    Err(error @ RpcError::Policy(_)) => return Err(error),
                    Err(error) => {
                        last_error = error;
                        self.rotate();
                    }
                }
            }
            if !self.config.retry_forever {
                return match last_error {
                    error @ RpcError::Handshake { .. } => Err(error),
                    error => Err(RpcError::MaxRetriesExceeded {
                        detail: format!(
                            "could not connect to any endpoint: {}",
                            policy::safe_diagnostic(
                                &error.to_string(),
                                generated::py_rpc::POLICY_DETAIL_LIMIT
                            )
                        ),
                    }),
                };
            }
            cycle = cycle.saturating_add(1);
            if !self
                .delay(Duration::from_secs(
                    u64::from(cycle).saturating_mul(2).min(30),
                ))
                .await
            {
                return Err(RpcError::Closed);
            }
        }
    }
    fn rotate(&mut self) {
        self.index = (self.index + 1) % self.config.pool.urls.len();
    }
    fn record_loss(&mut self, reason: RpcError) -> Result<(), RpcError> {
        // Refusals received while pacing reconnect setup are terminal too.
        if matches!(reason, RpcError::Policy(_)) {
            return Err(reason);
        }
        self.connection_lost(reason.clone());
        self.failures = self.failures.saturating_add(1);
        if self.failures.is_multiple_of(self.config.max_retries) {
            self.rotate();
        }
        let endpoints = u32::try_from(self.config.pool.urls.len()).unwrap_or(u32::MAX);
        if !self.config.retry_forever
            && self.failures >= self.config.max_retries.saturating_mul(endpoints)
        {
            return Err(RpcError::MaxRetriesExceeded {
                detail: format!(
                    "RPC request failed after exhausting endpoint retries: {}",
                    policy::safe_diagnostic(
                        &reason.to_string(),
                        generated::py_rpc::POLICY_DETAIL_LIMIT
                    )
                ),
            });
        }
        Ok(())
    }
    async fn delay(&mut self, duration: Duration) -> bool {
        let wait = sleep(duration);
        tokio::pin!(wait);
        loop {
            tokio::select! {_=&mut wait=>return true,command=self.commands.recv()=>{self.command(command);if self.stopping {return false;}}}
        }
    }
    async fn backoff(&mut self) -> bool {
        self.state.send_replace(SessionState::Reconnecting {
            attempt: self.failures,
        });
        let endpoint = policy::safe_endpoint(self.config.pool.urls[self.index].as_str());
        let _ = self.events.send(SessionEvent::Reconnecting {
            attempt: self.failures,
            endpoint: endpoint.clone(),
        });
        #[cfg(feature = "metrics")]
        metrics::counter!("rtao_rpc_reconnects_total","endpoint"=>endpoint).increment(1);
        self.delay(Duration::from_secs_f64(
            (0.25 * f64::from(self.failures)).min(5.0),
        ))
        .await
    }
    fn command(&mut self, command: Option<Command>) {
        match command {
            Some(Command::Request(entries)) => {
                for pending in entries {
                    if !pending.sender.is_closed() {
                        self.pending.insert(pending.id, pending);
                    }
                }
            }
            Some(Command::Cancel(ids)) => {
                for id in ids {
                    if let Some(pending) = self.pending.remove(&id) {
                        self.observe(&pending, "cancelled");
                    }
                    let subscribed = self
                        .subscriptions
                        .iter()
                        .find(|(_, active)| active.request_id == id)
                        .map(|(key, active)| (key.clone(), active.unsubscribe.clone()));
                    if let Some((sub_id, unsubscribe)) = subscribed {
                        if let Some(active) = self.subscriptions.remove(&sub_id) {
                            active.queue.close();
                        }
                        self.remember_closed(&sub_id);
                        self.detached.push_back((
                            self.epoch,
                            json!({"jsonrpc":"2.0","id":0,"method":unsubscribe,"params":[sub_id]}),
                        ));
                    }
                }
            }
            Some(Command::Forget { id, epoch }) if epoch == self.epoch => {
                self.subscriptions.remove(&id);
                self.remember_closed(&id);
            }
            Some(Command::Forget { .. }) => {}
            Some(Command::Unsubscribe { frame, epoch }) => self.detached.push_back((epoch, frame)),
            Some(Command::Snapshot(sender)) => {
                self.prune();
                let _ = sender.send(SessionStatistics {
                    pending: self.pending.len(),
                    subscriptions: self.subscriptions.len(),
                    early_notification_ids: self.early.len(),
                    early_notification_bytes: self.early_bytes,
                    subscription_retained_bytes: self.subscription_budget.retained_bytes(),
                    closed_subscription_ids: self.closed_ids.len(),
                });
            }
            Some(Command::Shutdown(ack)) => {
                self.stopping = true;
                self.shutdown_ack = ack;
            }
            None => self.stopping = true,
        }
        self.prune_early();
        self.update_gauge();
    }
    fn remember_closed(&mut self, id: &str) {
        if let Some(position) = self.early.iter().position(|(key, _)| key == id) {
            self.take_early(position);
        }
        self.closed_ids.retain(|key| key != id);
        self.closed_ids.push_back(id.into());
        while self.closed_ids.len() > generated::py_rpc::CLOSED_IDS_REMEMBERED {
            self.closed_ids.pop_front();
        }
    }
    fn prune(&mut self) {
        let closed: Vec<u64> = self
            .pending
            .values()
            .filter(|pending| {
                pending.sender.is_closed()
                    || pending
                        .required_epoch
                        .is_some_and(|epoch| epoch != self.epoch)
            })
            .map(|pending| pending.id)
            .collect();
        for id in closed {
            if let Some(pending) = self.pending.remove(&id) {
                self.observe(&pending, "cancelled");
                let _ = pending.sender.send(Err(WireError::Rpc(RpcError::Closed)));
            }
        }
        self.prune_early();
        self.update_gauge();
    }
    fn in_flight(&self) -> usize {
        self.pending
            .values()
            .filter(|pending| pending.sent && pending.last_sent_epoch == self.epoch)
            .count()
    }
    fn update_gauge(&self) {
        #[cfg(feature = "metrics")]
        {
            let current = self.in_flight();
            let previous = self.gauge_reported.swap(current, Ordering::Relaxed);
            if current > previous {
                metrics::gauge!("rtao_rpc_inflight").increment((current - previous) as f64);
            } else if previous > current {
                metrics::gauge!("rtao_rpc_inflight").decrement((previous - current) as f64);
            }
        }
    }
    fn inbound(&mut self, frame: Message) -> Result<(), RpcError> {
        let message = match frame {
            Message::Text(raw) => {
                tracing::debug!(target:"rtao::rpc::raw",frame=%DiagnosticFrame(&raw),"WEBSOCKET_RECEIVE");
                serde_json::from_str::<Value>(&raw).ok()
            }
            Message::Binary(raw) => serde_json::from_slice::<Value>(&raw).ok(),
            Message::Close(close) => {
                if let Some(close) = close
                    && let Some(policy) =
                        policy::classify_close(u16::from(close.code), close.reason.as_str())
                {
                    return Err(RpcError::Policy(Box::new(policy)));
                }
                return Err(RpcError::Transport("WebSocket connection closed".into()));
            }
            Message::Frame(_) => return Ok(()),
            Message::Ping(_) | Message::Pong(_) => None,
        };
        // Diagnostic discovery must not hide unanswered user requests or reset
        // their retry budget, whether the node encodes it as text or binary.
        if message
            .as_ref()
            .is_some_and(|value| value.get("id").and_then(Value::as_u64) == Some(0))
        {
            if let Some(error) = message.and_then(|message| self.dispatch(message)) {
                return Err(error);
            }
            return Ok(());
        }
        self.last_inbound = Instant::now();
        self.failures = 0;
        if let Some(error) = message.and_then(|message| self.dispatch(message)) {
            return Err(error);
        }
        Ok(())
    }
    async fn write_frame<S>(
        &mut self,
        socket: &mut OwnedSocket<S>,
        frame: Value,
    ) -> Result<(), RpcError>
    where
        S: Sink<Message, Error = WsError> + Unpin,
    {
        self.write_frame_owned(socket, frame, Vec::new()).await
    }
    async fn write_frame_owned<S>(
        &mut self,
        socket: &mut OwnedSocket<S>,
        frame: Value,
        owners: Vec<Arc<Custody>>,
    ) -> Result<(), RpcError>
    where
        S: Sink<Message, Error = WsError> + Unpin,
    {
        let raw = frame.to_string();
        tracing::debug!(target:"rtao::rpc::raw",frame=%DiagnosticFrame(&raw),"WEBSOCKET_SEND");
        let retained = socket.writers.len();
        socket.writers.extend(owners);
        let result = {
            let write = socket.io.send(Message::Text(raw.into()));
            tokio::pin!(write);
            let deadline = sleep(self.config.silence_timeout);
            tokio::pin!(deadline);
            loop {
                tokio::select! {
                    result=&mut write=>break result.map_err(|error|RpcError::Transport(policy::safe_diagnostic(&error.to_string(),generated::py_rpc::POLICY_DETAIL_LIMIT))),
                    _=&mut deadline=>break Err(RpcError::Timeout {detail:format!("no progress writing to {} in {}s",policy::safe_endpoint(self.config.pool.urls[self.index].as_str()),self.config.silence_timeout.as_secs_f64())}),
                    command=self.commands.recv()=>{self.command(command);if self.stopping {break Err(RpcError::Closed);}},
                }
            }
        };
        if result.is_ok() {
            // send() has flushed this frame; no future socket action can emit it.
            socket.writers.truncate(retained);
        }
        // An error/cancellation can leave bytes buffered in the socket. Retain
        // ownership until the socket is actually dropped, including close flush.
        result
    }
    async fn wait_for_token<S>(&mut self, socket: &mut OwnedSocket<S>) -> Result<(), RpcError>
    where
        S: Stream<Item = Result<Message, WsError>> + Unpin,
    {
        loop {
            if self.stopping {
                return Err(RpcError::Closed);
            }
            if self.bucket.take() {
                return Ok(());
            }
            let wait = self.bucket.wait();
            let silence = self.last_inbound + self.config.silence_timeout;
            let active = self.in_flight() > 0;
            tokio::select! {
                _=sleep(wait)=>{},
                _=sleep_until(silence),if active=>return Err(RpcError::Timeout {detail:format!("no response from {} in {}s",policy::safe_endpoint(self.config.pool.urls[self.index].as_str()),self.config.silence_timeout.as_secs_f64())}),
                command=self.commands.recv()=>self.command(command),
                incoming=socket.io.next()=>{match incoming {Some(Ok(frame))=>self.inbound(frame)?,Some(Err(error))=>return Err(RpcError::Transport(error.to_string())),None=>return Err(RpcError::Transport("WebSocket connection ended".into()))}},
            }
        }
    }
    async fn resend<S>(&mut self, socket: &mut OwnedSocket<S>) -> Result<(), RpcError>
    where
        S: Sink<Message, Error = WsError> + Stream<Item = Result<Message, WsError>> + Unpin,
    {
        self.prune();
        let ids: Vec<u64> = self
            .pending
            .values()
            .filter(|pending| pending.sent)
            .map(|pending| pending.id)
            .collect();
        for id in ids {
            if !self.pending.contains_key(&id) {
                continue;
            }
            self.wait_for_token(socket).await?;
            self.prune();
            let idle = self.in_flight() == 0;
            let Some(pending) = self.pending.get_mut(&id) else {
                continue;
            };
            if pending.sender.is_closed() {
                self.pending.remove(&id);
                continue;
            }
            if ReadMethod::from_str(&pending.method).is_err() {
                if let Some(pending) = self.pending.remove(&id) {
                    let error = match pending.hash {
                        Some(hash) => WireError::Submit(SubmitError::FateUnknown { hash }),
                        None => WireError::Rpc(RpcError::ConnectionLost { resent: false }),
                    };
                    self.observe(&pending, "connection_lost");
                    let _ = pending.sender.send(Err(error));
                }
                continue;
            }
            pending.last_sent_epoch = self.epoch;
            let frame = pending.frame.clone();
            let owners = pending.custody.iter().cloned().collect();
            if idle {
                self.last_inbound = Instant::now();
            }
            self.update_gauge();
            self.write_frame_owned(socket, frame, owners).await?;
        }
        Ok(())
    }
    async fn release_queue<S>(&mut self, socket: &mut OwnedSocket<S>) -> Result<(), RpcError>
    where
        S: Sink<Message, Error = WsError> + Stream<Item = Result<Message, WsError>> + Unpin,
    {
        while let Some((epoch, frame)) = self.detached.pop_front() {
            if epoch != self.epoch {
                continue;
            }
            self.wait_for_token(socket).await?;
            self.write_frame(socket, frame).await?;
        }
        loop {
            self.prune();
            let active = self.in_flight();
            if active >= self.config.max_in_flight {
                return Ok(());
            }
            let Some(first) = self
                .pending
                .values()
                .filter(|p| !p.sent)
                .min_by_key(|p| (p.hash.is_none(), p.id))
            else {
                return Ok(());
            };
            if !self.bucket.available() {
                return Ok(());
            }
            let group = first.group;
            let capacity = self.config.max_in_flight - active;
            let ids: Vec<u64> = self
                .pending
                .values()
                .filter(|p| !p.sent && p.group == group && !p.sender.is_closed())
                .take(capacity)
                .map(|p| p.id)
                .collect();
            let mut frames = Vec::new();
            let mut owners = Vec::new();
            for id in ids {
                if !self.bucket.take() {
                    break;
                }
                let Some(entry) = self.pending.get_mut(&id) else {
                    continue;
                };
                // Every transmission, including queue release, checks caller liveness.
                if entry.sender.is_closed() {
                    continue;
                }
                entry.sent = true;
                entry.last_sent_epoch = self.epoch;
                entry.waited = entry.started.elapsed();
                frames.push(entry.frame.clone());
                if let Some(custody) = &entry.custody {
                    owners.push(custody.clone());
                }
                tracing::debug!(target:"rtao::rpc",method=%entry.method,wait_seconds=entry.waited.as_secs_f64(),"RPC request queue released");
                #[cfg(feature = "metrics")]
                metrics::histogram!("rtao_rpc_ratelimit_wait_seconds")
                    .record(entry.waited.as_secs_f64());
            }
            if frames.is_empty() {
                return Ok(());
            }
            if active == 0 {
                self.last_inbound = Instant::now();
            }
            self.update_gauge();
            let frame = if frames.len() == 1 {
                frames.remove(0)
            } else {
                Value::Array(frames)
            };
            self.write_frame_owned(socket, frame, owners).await?;
        }
    }
    async fn pump(&mut self, socket: &mut TransportSocket) -> RpcError {
        loop {
            if self.stopping {
                return RpcError::Closed;
            }
            if let Err(error) = self.release_queue(socket).await {
                return error;
            }
            let silence = self.last_inbound + self.config.silence_timeout;
            let has_pending = self.in_flight() > 0;
            let queue_wait = if self.pending.values().any(|pending| !pending.sent) {
                self.bucket.wait()
            } else {
                Duration::from_secs(3600)
            };
            tokio::select! {
                command=self.commands.recv()=>self.command(command),
                frame=socket.io.next()=>{
                    match frame {
                        Some(Ok(frame))=>if let Err(error)=self.inbound(frame) {return error;},
                        Some(Err(error))=>return RpcError::Transport(policy::safe_diagnostic(&error.to_string(),generated::py_rpc::POLICY_DETAIL_LIMIT)),
                        None=>return RpcError::Transport("WebSocket connection ended".into()),
                    }
                },
                _=sleep_until(silence),if has_pending=>return RpcError::Timeout {detail:format!("no response from {} in {}s",policy::safe_endpoint(self.config.pool.urls[self.index].as_str()),self.config.silence_timeout.as_secs_f64())},
                _=sleep(queue_wait),if !queue_wait.is_zero()=>{},
            }
        }
    }
    fn dispatch(&mut self, message: Value) -> Option<RpcError> {
        let result = self.dispatch_inner(message);
        self.prune_early();
        result
    }
    fn dispatch_inner(&mut self, mut message: Value) -> Option<RpcError> {
        if let Value::Array(items) = message {
            for item in items {
                if let Some(error) = self.dispatch(item) {
                    return Some(error);
                }
            }
            return None;
        }
        let object = message.as_object()?;
        if let Some(id) = object.get("id").and_then(Value::as_u64) {
            if id == 0 {
                if let Err(error @ RpcError::Policy(_)) = unwrap_response(&message) {
                    return Some(error);
                }
                tracing::debug!(target:"rtao::rpc",methods=?object.get("result"),"RPC methods discovered");
                return None;
            }
            let pending = self.pending.remove(&id)?;
            if pending.sender.is_closed() {
                return None;
            }
            let response = unwrap_response(&message);
            self.observe(&pending, if response.is_ok() { "ok" } else { "error" });
            if let Err(error) = response {
                let policy = matches!(error, RpcError::Policy(_));
                let wire = if pending.hash.is_some() {
                    WireError::Submit(SubmitError::Rpc(error.clone()))
                } else {
                    WireError::Rpc(error.clone())
                };
                let _ = pending.sender.send(Err(wire));
                return policy.then_some(error);
            }
            let Ok(value) = response else {
                return None;
            };
            let queue = if let Some(sub) = pending.subscription {
                let Some(id) = subscription_id(&value) else {
                    let _ = pending
                        .sender
                        .send(Err(WireError::Rpc(RpcError::Protocol(format!(
                            "unexpected subscription id: {value}"
                        )))));
                    return None;
                };
                self.closed_ids.retain(|closed| closed != &id);
                let queue = Queue::new(
                    self.config.subscription_buffer,
                    self.subscription_budget.clone(),
                );
                let early = self
                    .early
                    .iter()
                    .position(|(key, _)| key == &id)
                    .and_then(|position| self.take_early(position));
                self.subscriptions.insert(
                    id,
                    ActiveSubscription {
                        queue: queue.clone(),
                        hash: pending.hash,
                        request_id: pending.id,
                        unsubscribe: sub.unsubscribe,
                    },
                );
                if let Some((_, items)) = early {
                    for item in items {
                        self.publish_notification(&queue, item.value);
                    }
                }
                Some(queue)
            } else {
                None
            };
            let _ = pending.sender.send(Ok(Reply {
                value,
                queue,
                epoch: self.epoch,
            }));
            return None;
        }
        let params = message.get_mut("params")?.as_object_mut()?;
        let id = subscription_id(params.get("subscription")?)?;
        // Move the decoded payload; cloning a large notification doubles its peak.
        let value = params.remove("result").unwrap_or(Value::Null);
        if let Some(active) = self.subscriptions.get(&id) {
            let queue = active.queue.clone();
            self.publish_notification(&queue, value);
        } else if !self.closed_ids.contains(&id) && self.pending_subscription() {
            self.buffer_early(id, value);
        }
        None
    }
    fn publish_notification(&self, queue: &Queue, value: Value) {
        let dropped = queue.push(Ok(value));
        for (id, active) in &self.subscriptions {
            if dropped.contains(&active.queue.key()) {
                let _ = self
                    .events
                    .send(SessionEvent::SubscriptionOverflow { id: id.clone() });
            }
        }
    }
    fn pending_subscription(&self) -> bool {
        self.pending.values().any(|pending| {
            pending.subscription.is_some()
                && pending.sent
                && pending.last_sent_epoch == self.epoch
                && !pending.sender.is_closed()
                && pending
                    .required_epoch
                    .is_none_or(|epoch| epoch == self.epoch)
        })
    }
    fn prune_early(&mut self) {
        if !self.pending_subscription() {
            self.clear_early();
        }
    }
    fn clear_early(&mut self) {
        self.early.clear();
        self.early_bytes = 0;
        self.early_order = 0;
    }
    fn early_identity_bytes(id: &str) -> usize {
        // Include retained identity bytes and conservative queue/key bookkeeping.
        id.len().saturating_mul(2).saturating_add(128)
    }
    fn take_early(&mut self, position: usize) -> Option<(String, VecDeque<BufferedNotification>)> {
        let (id, items) = self.early.remove(position)?;
        self.early_bytes -=
            Self::early_identity_bytes(&id) + items.iter().map(|item| item.bytes).sum::<usize>();
        Some((id, items))
    }
    fn pop_early(&mut self, position: usize) {
        if let Some((_, items)) = self.early.get_mut(position)
            && let Some(item) = items.pop_front()
        {
            self.early_bytes -= item.bytes;
            if items.is_empty() {
                self.take_early(position);
            } else if items.len() <= items.capacity() / 4 {
                items.shrink_to_fit();
            }
        }
    }
    fn buffer_early(&mut self, id: String, value: Value) {
        let bytes = notification_bytes(&value);
        let identity = Self::early_identity_bytes(&id);
        let limit = self.config.early_notification_bytes;
        // An unretainable message must not flush otherwise useful queued updates.
        if identity > limit || bytes > limit - identity {
            return;
        }
        if let Some(position) = self.early.iter().position(|(key, _)| key == &id) {
            if self.early[position].1.len() >= self.config.subscription_buffer {
                self.pop_early(position);
            }
        } else if self.early.len() >= self.config.early_notification_ids {
            self.take_early(0);
        }
        loop {
            let exists = self.early.iter().any(|(key, _)| key == &id);
            let extra = bytes + if exists { 0 } else { identity };
            if self.early_bytes <= limit - extra {
                break;
            }
            let Some(position) = self
                .early
                .iter()
                .enumerate()
                .filter_map(|(position, (_, items))| {
                    items.front().map(|item| (position, item.order))
                })
                .min_by_key(|(_, order)| *order)
                .map(|(position, _)| position)
            else {
                return;
            };
            self.pop_early(position);
        }
        let order = self.early_order;
        self.early_order = self.early_order.saturating_add(1);
        let item = BufferedNotification {
            value,
            bytes,
            order,
        };
        self.early_bytes += bytes;
        if let Some((_, items)) = self.early.iter_mut().find(|(key, _)| key == &id) {
            items.push_back(item);
        } else {
            self.early_bytes += identity;
            self.early.push_back((id, VecDeque::from([item])));
        }
    }
    fn observe(&self, pending: &Pending, outcome: &'static str) {
        #[cfg(feature = "metrics")]
        {
            let endpoint = policy::safe_endpoint(self.config.pool.urls[self.index].as_str());
            metrics::counter!("rtao_rpc_requests_total","method"=>pending.method.clone(),"endpoint"=>endpoint,"outcome"=>outcome).increment(1);
            metrics::histogram!("rtao_rpc_request_seconds","method"=>pending.method.clone())
                .record(pending.started.elapsed().as_secs_f64());
            self.update_gauge();
        }
        #[cfg(not(feature = "metrics"))]
        let _ = (pending, outcome);
    }
    fn connection_lost(&mut self, error: RpcError) {
        for pending in self.pending.values_mut() {
            pending.last_sent_epoch = 0;
        }
        self.update_gauge();
        self.clear_early();
        self.closed_ids.clear();
        self.detached.clear();
        for (_, active) in std::mem::take(&mut self.subscriptions) {
            let error = if let Some(hash) = active.hash {
                WireError::Submit(SubmitError::FateUnknown { hash })
            } else if matches!(error, RpcError::Policy(_)) {
                WireError::Rpc(error.clone())
            } else {
                WireError::Rpc(RpcError::SubscriptionLost)
            };
            active.queue.push(Err(error));
        }
        let doomed: Vec<u64> = self
            .pending
            .values()
            .filter(|p| p.sent && ReadMethod::from_str(&p.method).is_err())
            .map(|p| p.id)
            .collect();
        for id in doomed {
            if let Some(pending) = self.pending.remove(&id) {
                self.observe(
                    &pending,
                    if pending.hash.is_some() {
                        "fate_unknown"
                    } else {
                        "connection_lost"
                    },
                );
                let error = match pending.hash {
                    Some(hash) => WireError::Submit(SubmitError::FateUnknown { hash }),
                    None => WireError::Rpc(RpcError::ConnectionLost { resent: false }),
                };
                let _ = pending.sender.send(Err(error));
            }
        }
    }
    fn finish(&mut self, error: RpcError) {
        if let RpcError::Policy(ref policy) = error {
            let _ = self
                .events
                .send(SessionEvent::PolicyRefused((**policy).clone()));
            #[cfg(feature = "metrics")]
            metrics::counter!("rtao_rpc_policy_refusals_total","kind"=>format!("{:?}",policy.kind))
                .increment(1);
        }
        let closed = error == RpcError::Closed;
        for (_, pending) in std::mem::take(&mut self.pending) {
            self.observe(&pending, if closed { "closed" } else { "error" });
            let wire = if let Some(hash) = pending.hash {
                if pending.sent && !closed && !matches!(error, RpcError::Policy(_)) {
                    WireError::Submit(SubmitError::FateUnknown { hash })
                } else {
                    WireError::Submit(SubmitError::Rpc(error.clone()))
                }
            } else {
                WireError::Rpc(error.clone())
            };
            let _ = pending.sender.send(Err(wire));
        }
        for (_, active) in std::mem::take(&mut self.subscriptions) {
            if closed {
                active.queue.close();
            } else {
                active.queue.push(Err(WireError::Rpc(error.clone())));
            }
        }
        self.clear_early();
        self.closed_ids.clear();
        self.state.send_replace(if closed {
            SessionState::Closed
        } else {
            SessionState::Failed { error }
        });
        if let Some(ack) = self.shutdown_ack.take() {
            let _ = ack.send(());
        }
    }
}
struct Bucket {
    rate: Option<RateLimit>,
    tokens: f64,
    updated: Instant,
}
impl Bucket {
    fn new(rate: Option<RateLimit>) -> Self {
        #[cfg(not(feature = "rate-limit"))]
        let rate = {
            let _ = rate;
            None
        };
        Self {
            rate,
            tokens: rate.map_or(f64::INFINITY, |r| f64::from(r.burst)),
            updated: Instant::now(),
        }
    }
    fn refill(&mut self) {
        if let Some(rate) = self.rate {
            let now = Instant::now();
            self.tokens = (self.tokens
                + now.duration_since(self.updated).as_secs_f64() * rate.per_second)
                .min(f64::from(rate.burst));
            self.updated = now;
        }
    }
    fn available(&mut self) -> bool {
        self.refill();
        self.tokens >= 1.0
    }
    fn take(&mut self) -> bool {
        if !self.available() {
            return false;
        }
        self.tokens -= 1.0;
        true
    }
    fn wait(&mut self) -> Duration {
        self.refill();
        match self.rate {
            Some(rate) if self.tokens < 1.0 => {
                Duration::from_secs_f64((1.0 - self.tokens) / rate.per_second)
            }
            _ => Duration::from_secs(3600),
        }
    }
}
pub(crate) fn unwrap_response(message: &Value) -> Result<Value, RpcError> {
    if let Some(error) = message.get("error") {
        let text = error
            .get("data")
            .and_then(Value::as_str)
            .filter(|data| data.contains(generated::py_rpc::STATE_DISCARDED_NEEDLE))
            .or_else(|| error.get("message").and_then(Value::as_str));
        if let Some(block) = text
            .and_then(|text| text.split_once(generated::py_rpc::STATE_DISCARDED_NEEDLE))
            .map(|(_, block)| {
                block
                    .split(generated::py_rpc::STATE_DISCARDED_NEEDLE)
                    .next()
                    .unwrap_or(block)
                    .trim()
                    .to_owned()
            })
        {
            return Err(RpcError::StateDiscarded { block });
        }
        if let Some(policy) = policy::classify_rpc_error(error) {
            return Err(RpcError::Policy(Box::new(policy)));
        }
        return Err(RpcError::JsonRpc {
            code: error.get("code").and_then(Value::as_i64).unwrap_or(0),
            message: error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("")
                .into(),
            data: error.get("data").cloned(),
            payload: Box::new(error.clone()),
        });
    }
    message
        .get("result")
        .cloned()
        .ok_or_else(|| RpcError::Protocol(message.to_string()))
}
fn validate(config: &SessionConfig) -> Result<(), RpcError> {
    if config.pool.urls.is_empty() {
        return Err(RpcError::Protocol("RPC endpoint pool is empty".into()));
    }
    if config.max_retries == 0
        || config.max_in_flight == 0
        || config.subscription_buffer == 0
        || config.early_notification_ids == 0
        || config.early_notification_bytes == 0
        || config.subscription_buffer_bytes == 0
        || config.max_message_bytes == 0
        || config.handshake_timeout.is_zero()
        || config.silence_timeout.is_zero()
    {
        return Err(RpcError::Protocol(
            "RPC limits and timeouts must be positive".into(),
        ));
    }
    if let Some(rate) = config.rate_limit
        && (!rate.per_second.is_finite() || rate.per_second <= 0.0 || rate.burst == 0)
    {
        return Err(RpcError::Protocol(
            "RPC rate limit must have a finite positive rate and nonzero burst".into(),
        ));
    }
    for url in config.pool.urls.iter().chain(&config.pool.archive) {
        match url.scheme() {
            "wss" => {}
            "ws" => {
                let loopback = url.host_str().is_some_and(|host| {
                    host == "localhost"
                        || host
                            .parse::<std::net::IpAddr>()
                            .is_ok_and(|ip| ip.is_loopback())
                });
                if config.plaintext_ws == PlaintextPolicy::Refuse
                    || (config.plaintext_ws == PlaintextPolicy::LoopbackOnly && !loopback)
                {
                    return Err(RpcError::Protocol(format!(
                        "plaintext WebSocket endpoint refused: {}",
                        policy::safe_endpoint(url.as_str())
                    )));
                }
                if config.plaintext_ws == PlaintextPolicy::AllowWithWarning {
                    tracing::warn!(target:"rtao::rpc",endpoint=%policy::safe_endpoint(url.as_str()),"RPC uses an unencrypted WebSocket");
                }
            }
            _ => {
                return Err(RpcError::Protocol(
                    "RPC endpoint must use ws:// or wss://".into(),
                ));
            }
        }
    }
    Ok(())
}
/// Build an explicit ring TLS client configuration with native and extra roots.
/// This function never consults or installs the process-default crypto provider.
pub fn tls_config(config: &SessionConfig) -> Result<Arc<rustls::ClientConfig>, RpcError> {
    tls_config_with_roots(config, rustls_native_certs::load_native_certs().certs)
}
fn tls_config_for_endpoints(
    config: &SessionConfig,
    native_roots: impl FnOnce() -> Vec<rustls_pki_types::CertificateDer<'static>>,
) -> Result<Arc<rustls::ClientConfig>, RpcError> {
    // Archive sessions are lazy, but their secure endpoints still require the
    // native trust policy. Plaintext-only pools have no TLS handshake to trust.
    let native = if config
        .pool
        .urls
        .iter()
        .chain(&config.pool.archive)
        .any(|url| url.scheme() == "wss")
    {
        native_roots()
    } else {
        Vec::new()
    };
    tls_config_with_roots(config, native)
}
fn tls_config_with_roots(
    config: &SessionConfig,
    native: Vec<rustls_pki_types::CertificateDer<'static>>,
) -> Result<Arc<rustls::ClientConfig>, RpcError> {
    let mut roots = rustls::RootCertStore::empty();
    let _ = roots.add_parsable_certificates(native);
    for certificate in &config.extra_roots {
        roots.add(certificate.clone()).map_err(|error| {
            RpcError::Protocol(format!("invalid extra RPC root certificate: {error}"))
        })?;
    }
    let mut tls = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|error| RpcError::Protocol(error.to_string()))?
    .with_root_certificates(roots)
    .with_no_client_auth();
    tls.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(tls))
}
#[cfg(test)]
mod tls_tests {
    use super::*;
    use std::{cell::Cell, error::Error};

    type TestResult = Result<(), Box<dyn Error + Send + Sync>>;

    #[test]
    fn plaintext_primary_fallback_and_archive_do_not_load_native_roots() -> TestResult {
        let config = SessionConfig {
            pool: EndpointPool::new(Url::parse("ws://localhost:9944")?)
                .with_fallbacks([Url::parse("ws://127.0.0.1:9945")?])
                .with_archives([Url::parse("ws://[::1]:9946")?]),
            ..SessionConfig::default()
        };
        validate(&config)?;
        let loaded = Cell::new(false);
        let tls = tls_config_for_endpoints(&config, || {
            loaded.set(true);
            Vec::new()
        })?;
        assert!(!loaded.get());
        assert_eq!(tls.alpn_protocols, [b"http/1.1".to_vec()]);
        assert!(tls.crypto_provider().cipher_suites.iter().all(|suite| {
            rustls::crypto::ring::default_provider()
                .cipher_suites
                .contains(suite)
        }));
        Ok(())
    }

    #[test]
    fn secure_primary_fallback_and_archive_each_load_native_roots() -> TestResult {
        for position in 0..3 {
            let mut config = SessionConfig {
                pool: EndpointPool::new(Url::parse("ws://localhost:9944")?)
                    .with_fallbacks([Url::parse("ws://127.0.0.1:9945")?])
                    .with_archives([Url::parse("ws://localhost:9946")?]),
                ..SessionConfig::default()
            };
            let secure = Url::parse("wss://localhost:9947")?;
            match position {
                0 => config.pool.urls[0] = secure,
                1 => config.pool.urls[1] = secure,
                _ => config.pool.archive[0] = secure,
            }
            validate(&config)?;
            let loaded = Cell::new(false);
            let tls = tls_config_for_endpoints(&config, || {
                loaded.set(true);
                Vec::new()
            })?;
            assert!(
                loaded.get(),
                "secure endpoint at position {position} ignored"
            );
            assert_eq!(tls.alpn_protocols, [b"http/1.1".to_vec()]);
        }
        Ok(())
    }

    #[test]
    fn invalid_extra_root_is_rejected_with_or_without_secure_endpoints() -> TestResult {
        for scheme in ["ws", "wss"] {
            let mut config = SessionConfig::new(Url::parse(&format!("{scheme}://localhost:9944"))?);
            config.extra_roots.push(vec![0_u8; 8].into());
            let error = tls_config_for_endpoints(&config, Vec::new)
                .err()
                .ok_or("invalid root accepted")?;
            assert!(
                matches!(error, RpcError::Protocol(ref detail) if detail.starts_with("invalid extra RPC root certificate:"))
            );
        }
        Ok(())
    }
}
