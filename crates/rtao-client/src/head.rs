// Derived from RaoFoundation/subtensor sdk/python/bittensor/client.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Best and finalized header followers with reconnect and gap recovery (D-012).

use blake2::{Blake2b, Digest, digest::consts::U32};
use futures::{Stream, StreamExt};
use parity_scale_codec::{Compact, Decode, Encode};
use rtao_primitives::H256;
use rtao_rpc::{ReadMethod, RpcError, Session, SubscriptionKind};
use serde_json::{Value, json};
use std::{collections::VecDeque, error::Error, fmt, future::Future, pin::Pin, sync::Arc};
use tokio::sync::{broadcast, watch};
use tokio_util::sync::CancellationToken;

/// Which chain head a block follower observes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Finality {
    /// The current best branch, including explicit reorganization notices.
    Best,
    /// Finalized history, with each advancing height emitted exactly once.
    Finalized,
}

/// A block subscription header, retaining Python's original JSON fields.
#[derive(Clone, Debug, PartialEq)]
pub struct BlockHeader {
    /// Native chain block height.
    pub number: u64,
    /// Header identity, independent of a later best-chain reorganization.
    pub hash: H256,
    /// Parent identity, when included by the source.
    pub parent_hash: Option<H256>,
    /// Original JSON header.
    pub raw: Value,
}

impl BlockHeader {
    /// Parse a legacy JSON header and derive its Blake2-256 SCALE header hash.
    ///
    /// A source-supplied `hash` field is used when present. Standard node
    /// subscriptions omit it; those require the roots and digest to derive it.
    pub fn from_json(raw: Value) -> Result<Self, HeadError> {
        let hash = match raw.get("hash") {
            Some(value) => parse_hash(value, "hash")?,
            None => header_hash(&raw)?,
        };
        Self::from_json_with_hash(raw, hash)
    }

    /// Parse a header retrieved by an already pinned RPC hash.
    ///
    /// This uses the requested identity rather than looking up the header's
    /// height again, which could resolve to another branch after a reorg.
    pub fn from_json_with_hash(raw: Value, hash: H256) -> Result<Self, HeadError> {
        if let Some(supplied) = raw.get("hash")
            && parse_hash(supplied, "hash")? != hash
        {
            return Err(HeadError::InvalidHeader(
                "source hash differs from the requested identity".into(),
            ));
        }
        let number = parse_number(
            raw.get("number")
                .ok_or_else(|| HeadError::InvalidHeader("missing number".into()))?,
        )?;
        let parent_hash = raw
            .get("parentHash")
            .filter(|value| !value.is_null())
            .map(|value| parse_hash(value, "parentHash"))
            .transpose()?;
        Ok(Self {
            number,
            hash,
            parent_hash,
            raw,
        })
    }
}

/// A reported change from one best branch to another.
#[derive(Clone, Debug, PartialEq)]
pub struct Reorg {
    /// Best head immediately before the branch change.
    pub previous: BlockHeader,
    /// New best head announced by the source.
    pub replacement: BlockHeader,
    /// Last shared block of the previous and replacement branches.
    pub common_ancestor: BlockHeader,
}

/// A head-tracking failure or nonterminal best-branch change notice.
#[derive(Clone, Debug, PartialEq)]
pub enum HeadError {
    /// Original transport error, including gateway policy metadata.
    Rpc(RpcError),
    /// A header has malformed or inconsistent fields.
    InvalidHeader(String),
    /// A requested height has no block hash.
    MissingBlock {
        /// Missing height.
        number: u64,
    },
    /// A requested header is unavailable, including pruned fork ancestry.
    MissingHeader {
        /// Requested identity.
        hash: H256,
    },
    /// A finalized source attempted to replace already observed history.
    FinalityViolation {
        /// Already observed finalized head.
        previous: H256,
        /// Conflicting head.
        replacement: H256,
    },
    /// A best follower changed branches; replacement headers follow this notice.
    Reorg(Arc<Reorg>),
}

impl fmt::Display for HeadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rpc(error) => fmt::Display::fmt(error, formatter),
            Self::InvalidHeader(detail) => write!(formatter, "invalid block header: {detail}"),
            Self::MissingBlock { number } => write!(formatter, "missing block at height {number}"),
            Self::MissingHeader { hash } => {
                write!(formatter, "missing block header {}", hex_hash(*hash))
            }
            Self::FinalityViolation { .. } => {
                formatter.write_str("finalized history changed or moved backwards")
            }
            Self::Reorg(reorg) => write!(
                formatter,
                "best chain reorganized at height {}",
                reorg.common_ancestor.number
            ),
        }
    }
}

impl Error for HeadError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Rpc(error) => Some(error),
            _ => None,
        }
    }
}

impl From<RpcError> for HeadError {
    fn from(value: RpcError) -> Self {
        Self::Rpc(value)
    }
}

/// A provider operation borrowing its transport.
pub type HeadFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, HeadError>> + Send + 'a>>;
/// A block stream; best-chain reorganizations are nonterminal error items.
pub type BlockStream = Pin<Box<dyn Stream<Item = Result<BlockHeader, HeadError>> + Send + 'static>>;

/// Minimal legacy head access, also implementable by custom in-process transports.
pub trait HeadProvider: Send + Sync {
    /// Subscribe without reconnect recovery, retaining session-level semantics.
    fn subscribe(&self, finality: Finality) -> HeadFuture<'_, BlockStream>;
    /// Resolve a canonical block height with `chain_getBlockHash`.
    fn block_hash(&self, number: u64) -> HeadFuture<'_, Option<H256>>;
    /// Read one header by immutable identity.
    fn header(&self, hash: H256) -> HeadFuture<'_, Option<BlockHeader>>;
}

impl HeadProvider for Session {
    fn subscribe(&self, finality: Finality) -> HeadFuture<'_, BlockStream> {
        Box::pin(async move {
            let kind = match finality {
                Finality::Best => SubscriptionKind::NewHeads,
                Finality::Finalized => SubscriptionKind::FinalizedHeads,
            };
            let subscription = Session::subscribe::<Value>(self, kind).await?;
            Ok(Box::pin(subscription.map(|result| {
                result
                    .map_err(HeadError::Rpc)
                    .and_then(BlockHeader::from_json)
            })) as BlockStream)
        })
    }

    fn block_hash(&self, number: u64) -> HeadFuture<'_, Option<H256>> {
        Box::pin(async move {
            let result: Option<rtao_rpc::H256> = self
                .request(ReadMethod::ChainGetBlockHash, json!([number]))
                .await?;
            Ok(result.map(|hash| H256(hash.0)))
        })
    }

    fn header(&self, hash: H256) -> HeadFuture<'_, Option<BlockHeader>> {
        Box::pin(async move {
            let raw: Option<Value> = self
                .request(ReadMethod::ChainGetHeader, json!([hex_hash(hash)]))
                .await?;
            raw.map(|raw| BlockHeader::from_json_with_hash(raw, hash))
                .transpose()
        })
    }
}

impl HeadProvider for subxt_rpcs::RpcClient {
    fn subscribe(&self, finality: Finality) -> HeadFuture<'_, BlockStream> {
        Box::pin(async move {
            let (subscribe, unsubscribe) = match finality {
                Finality::Best => ("chain_subscribeNewHeads", "chain_unsubscribeNewHeads"),
                Finality::Finalized => (
                    "chain_subscribeFinalizedHeads",
                    "chain_unsubscribeFinalizedHeads",
                ),
            };
            let stream = self
                .subscribe::<Value>(subscribe, subxt_rpcs::client::RpcParams::new(), unsubscribe)
                .await
                .map_err(rpc_client_error)?;
            Ok(Box::pin(stream.map(|result| {
                result
                    .map_err(rpc_client_error)
                    .and_then(BlockHeader::from_json)
            })) as BlockStream)
        })
    }

    fn block_hash(&self, number: u64) -> HeadFuture<'_, Option<H256>> {
        Box::pin(async move {
            let raw = crate::rpc_request(self, "chain_getBlockHash", json!([number]))
                .await
                .map_err(client_error)?;
            if raw.is_null() {
                Ok(None)
            } else {
                parse_hash(&raw, "block hash").map(Some)
            }
        })
    }

    fn header(&self, hash: H256) -> HeadFuture<'_, Option<BlockHeader>> {
        Box::pin(async move {
            let raw = crate::rpc_request(self, "chain_getHeader", json!([hex_hash(hash)]))
                .await
                .map_err(client_error)?;
            if raw.is_null() {
                Ok(None)
            } else {
                BlockHeader::from_json_with_hash(raw, hash).map(Some)
            }
        })
    }
}

fn rpc_client_error(error: subxt_rpcs::Error) -> HeadError {
    client_error(crate::ClientError::from(error))
}

fn client_error(error: crate::ClientError) -> HeadError {
    match error {
        crate::ClientError::Rpc(error) => HeadError::Rpc(error),
        error => HeadError::InvalidHeader(error.to_string()),
    }
}

/// Published head changes and terminal worker failures.
#[derive(Clone, Debug, PartialEq)]
pub enum HeadEvent {
    /// A new header, including recovered intermediate heights.
    Applied(BlockHeader),
    /// Best-chain change, published before its replacement headers.
    Reorg(Arc<Reorg>),
    /// Tracking ended with an error.
    Failed(HeadError),
}

/// Shared best and finalized watches, with one worker for each subscription.
#[derive(Clone)]
pub struct HeadTracker {
    provider: Arc<dyn HeadProvider>,
    best: watch::Receiver<BlockHeader>,
    finalized: watch::Receiver<BlockHeader>,
    best_events: broadcast::Sender<HeadEvent>,
    finalized_events: broadcast::Sender<HeadEvent>,
    best_failure: watch::Receiver<Option<HeadError>>,
    finalized_failure: watch::Receiver<Option<HeadError>>,
    cancellation: CancellationToken,
}

impl HeadTracker {
    /// Establish both subscriptions and start cancellable recovery workers.
    ///
    /// Bootstrap headers come from the client's pinned initialization reads.
    /// The supplied token belongs to the client lifetime, so dropping the last
    /// client cancels workers even if a returned watch or block stream survives.
    pub async fn start(
        provider: Arc<dyn HeadProvider>,
        best: BlockHeader,
        finalized: BlockHeader,
        cancellation: CancellationToken,
    ) -> Result<Self, HeadError> {
        if finalized.number > best.number {
            return Err(HeadError::InvalidHeader(
                "finalized height exceeds best height".into(),
            ));
        }
        let (best_stream, finalized_stream) = tokio::select! {
            biased;
            () = cancellation.cancelled() => return Err(RpcError::Closed.into()),
            streams = futures::future::try_join(
                subscribe_recovering(provider.as_ref(), Finality::Best, &cancellation),
                subscribe_recovering(provider.as_ref(), Finality::Finalized, &cancellation),
            ) => streams?,
        };
        let (best_tx, best_rx) = watch::channel(best.clone());
        let (finalized_tx, finalized_rx) = watch::channel(finalized.clone());
        let (best_events, _) = broadcast::channel(256);
        let (finalized_events, _) = broadcast::channel(256);
        let (best_failure_tx, best_failure) = watch::channel(None);
        let (finalized_failure_tx, finalized_failure) = watch::channel(None);
        for (finality, initial, stream, publisher) in [
            (
                Finality::Best,
                best,
                best_stream,
                Publisher {
                    heads: best_tx,
                    events: best_events.clone(),
                    failure: best_failure_tx,
                },
            ),
            (
                Finality::Finalized,
                finalized,
                finalized_stream,
                Publisher {
                    heads: finalized_tx,
                    events: finalized_events.clone(),
                    failure: finalized_failure_tx,
                },
            ),
        ] {
            tokio::spawn(follow(
                provider.clone(),
                finality,
                initial,
                stream,
                publisher,
                cancellation.clone(),
            ));
        }
        Ok(Self {
            provider,
            best: best_rx,
            finalized: finalized_rx,
            best_events,
            finalized_events,
            best_failure,
            finalized_failure,
            cancellation,
        })
    }

    /// Watch the latest successfully reconciled best header.
    pub fn best(&self) -> watch::Receiver<BlockHeader> {
        self.best.clone()
    }

    /// Watch the latest successfully reconciled finalized header.
    pub fn finalized(&self) -> watch::Receiver<BlockHeader> {
        self.finalized.clone()
    }

    /// The terminal worker error, retained even before a follower subscribes.
    pub fn failure(&self, finality: Finality) -> Option<HeadError> {
        match finality {
            Finality::Best => self.best_failure.borrow().clone(),
            Finality::Finalized => self.finalized_failure.borrow().clone(),
        }
    }

    /// Receive branch notices and recovered headers for one head policy.
    ///
    /// This is a bounded diagnostic channel. Use [`Self::blocks`] for a follower
    /// that recovers history when its consumer falls behind the channel.
    pub fn events(&self, finality: Finality) -> broadcast::Receiver<HeadEvent> {
        self.event_sender(finality).subscribe()
    }

    /// Follow headers, recovering reconnect gaps and slow-consumer gaps.
    ///
    /// The current head is emitted first. Finalized headers are emitted once
    /// per advancing height. Best followers receive a `HeadError::Reorg` notice
    /// before replacement-branch headers.
    pub fn blocks(&self, finality: Finality) -> BlockStream {
        let events = self.events(finality);
        let heads = self.heads(finality);
        let initial = heads.borrow().clone();
        let failure_watch = match finality {
            Finality::Best => self.best_failure.clone(),
            Finality::Finalized => self.finalized_failure.clone(),
        };
        let failure = self.failure(finality);
        let ended = failure.is_some();
        let mut pending = VecDeque::from([Ok(initial)]);
        if let Some(error) = failure {
            pending.push_back(Err(error));
        }
        let state = Follower {
            provider: self.provider.clone(),
            finality,
            events,
            heads,
            cancellation: self.cancellation.clone(),
            last: None,
            pending,
            ended,
            skip_applied: 0,
            failure: failure_watch,
        };
        Box::pin(futures::stream::unfold(state, |mut state| async move {
            loop {
                if state.cancellation.is_cancelled() {
                    return None;
                }
                if let Some(item) = state.pending.pop_front() {
                    if let Ok(header) = &item {
                        state.last = Some(header.clone());
                    }
                    return Some((item, state));
                }
                if state.ended || state.cancellation.is_cancelled() {
                    return None;
                }
                let event = tokio::select! {
                    biased;
                    () = state.cancellation.cancelled() => return None,
                    event = state.events.recv() => event,
                };
                match event {
                    Ok(HeadEvent::Applied(next)) => {
                        if state.skip_applied > 0 {
                            state.skip_applied -= 1;
                        } else if state
                            .last
                            .as_ref()
                            .is_none_or(|last| next.number > last.number)
                        {
                            state.recover_to(next).await;
                        }
                    }
                    Ok(HeadEvent::Reorg(reorg)) => {
                        // Reconcile on the replacement, rather than forwarding an
                        // event whose previous head this consumer may have missed.
                        state.recover_to(reorg.replacement.clone()).await;
                        state.skip_applied = usize::try_from(
                            reorg.replacement.number - reorg.common_ancestor.number,
                        )
                        .unwrap_or(usize::MAX);
                    }
                    Ok(HeadEvent::Failed(error)) => {
                        state.pending.push_back(Err(error));
                        state.ended = true;
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        // Discard the retained suffix before reading the watch.
                        // It would otherwise replay older best heads after recovery.
                        state.events = state.events.resubscribe();
                        state.skip_applied = 0;
                        let latest = state.heads.borrow().clone();
                        state.recover_to(latest).await;
                        let failure = state.failure.borrow().clone();
                        if let Some(error) = failure.filter(|_| !state.ended) {
                            state.pending.push_back(Err(error));
                            state.ended = true;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => return None,
                }
            }
        }))
    }

    /// Open a session-level stream that ends on loss rather than resubscribing.
    pub async fn blocks_strict(&self, finality: Finality) -> Result<BlockStream, HeadError> {
        let stream = tokio::select! {
            biased;
            () = self.cancellation.cancelled() => return Err(RpcError::Closed.into()),
            stream = self.provider.subscribe(finality) => stream?,
        };
        let cancellation = self.cancellation.clone();
        Ok(Box::pin(futures::stream::unfold(
            (stream, cancellation, false),
            |(mut stream, cancellation, ended)| async move {
                if ended {
                    return None;
                }
                let item = tokio::select! {
                    biased;
                    () = cancellation.cancelled() => return None,
                    item = stream.next() => item?,
                };
                let ended = item.is_err();
                Some((item, (stream, cancellation, ended)))
            },
        )))
    }

    fn heads(&self, finality: Finality) -> watch::Receiver<BlockHeader> {
        match finality {
            Finality::Best => self.best(),
            Finality::Finalized => self.finalized(),
        }
    }

    fn event_sender(&self, finality: Finality) -> &broadcast::Sender<HeadEvent> {
        match finality {
            Finality::Best => &self.best_events,
            Finality::Finalized => &self.finalized_events,
        }
    }
}

struct Follower {
    provider: Arc<dyn HeadProvider>,
    finality: Finality,
    events: broadcast::Receiver<HeadEvent>,
    heads: watch::Receiver<BlockHeader>,
    cancellation: CancellationToken,
    last: Option<BlockHeader>,
    pending: VecDeque<Result<BlockHeader, HeadError>>,
    ended: bool,
    skip_applied: usize,
    failure: watch::Receiver<Option<HeadError>>,
}

impl Follower {
    async fn recover_to(&mut self, next: BlockHeader) {
        let Some(previous) = &self.last else {
            self.pending.push_back(Ok(next));
            return;
        };
        let result = tokio::select! {
            biased;
            () = self.cancellation.cancelled() => {
                self.ended = true;
                return;
            }
            result = reconcile(self.provider.as_ref(), self.finality, previous, next) => result,
        };
        match result {
            Ok(changes) => {
                if let Some(reorg) = changes.reorg {
                    self.pending.push_back(Err(HeadError::Reorg(reorg)));
                }
                self.pending.extend(changes.headers.into_iter().map(Ok));
            }
            Err(error) => {
                self.pending.push_back(Err(error));
                self.ended = true;
            }
        }
    }
}

struct Publisher {
    heads: watch::Sender<BlockHeader>,
    events: broadcast::Sender<HeadEvent>,
    failure: watch::Sender<Option<HeadError>>,
}

impl Publisher {
    fn fail(&self, error: HeadError) {
        self.failure.send_replace(Some(error.clone()));
        let _ = self.events.send(HeadEvent::Failed(error));
    }
}

async fn follow(
    provider: Arc<dyn HeadProvider>,
    finality: Finality,
    mut current: BlockHeader,
    mut stream: BlockStream,
    publisher: Publisher,
    cancellation: CancellationToken,
) {
    loop {
        let update = tokio::select! {
            biased;
            () = cancellation.cancelled() => return,
            update = stream.next() => update,
        };
        match update {
            Some(Ok(next)) => {
                let result = tokio::select! {
                    biased;
                    () = cancellation.cancelled() => return,
                    changes = reconcile(provider.as_ref(), finality, &current, next) => changes,
                };
                match result {
                    Ok(changes) => {
                        if let Some(reorg) = changes.reorg {
                            let _ = publisher.events.send(HeadEvent::Reorg(reorg));
                        }
                        for header in changes.headers {
                            current = header.clone();
                            publisher.heads.send_replace(header.clone());
                            let _ = publisher.events.send(HeadEvent::Applied(header));
                        }
                    }
                    Err(error) => {
                        publisher.fail(error);
                        return;
                    }
                }
            }
            Some(Err(HeadError::Rpc(RpcError::SubscriptionLost))) => {
                let resubscribed = tokio::select! {
                    biased;
                    () = cancellation.cancelled() => return,
                    stream = subscribe_recovering(provider.as_ref(), finality, &cancellation) => stream,
                };
                match resubscribed {
                    Ok(replacement) => stream = replacement,
                    Err(error) => {
                        publisher.fail(error);
                        return;
                    }
                }
            }
            Some(Err(error)) => {
                publisher.fail(error);
                return;
            }
            None => {
                publisher.fail(RpcError::Closed.into());
                return;
            }
        }
    }
}

struct Changes {
    reorg: Option<Arc<Reorg>>,
    headers: Vec<BlockHeader>,
}

async fn subscribe_recovering(
    provider: &dyn HeadProvider,
    finality: Finality,
    cancellation: &CancellationToken,
) -> Result<BlockStream, HeadError> {
    loop {
        let subscription = tokio::select! {
            biased;
            () = cancellation.cancelled() => return Err(RpcError::Closed.into()),
            subscription = provider.subscribe(finality) => subscription,
        };
        match subscription {
            Err(HeadError::Rpc(RpcError::ConnectionLost { .. } | RpcError::SubscriptionLost)) => {
                // A second socket loss can interrupt subscription registration
                // itself. Session controls reconnect pacing; yielding also keeps
                // a custom transport's immediate errors cancellable.
                tokio::task::yield_now().await;
            }
            result => return result,
        }
    }
}

async fn reconcile(
    provider: &dyn HeadProvider,
    finality: Finality,
    previous: &BlockHeader,
    next: BlockHeader,
) -> Result<Changes, HeadError> {
    if next.hash == previous.hash {
        if next.number != previous.number || next.parent_hash != previous.parent_hash {
            return Err(HeadError::InvalidHeader(
                "same hash announced with different ancestry".into(),
            ));
        }
        return Ok(Changes {
            reorg: None,
            headers: Vec::new(),
        });
    }
    if finality == Finality::Finalized && next.number < previous.number {
        // A subscription can replay an old finalized header. Confirm its
        // canonical identity and leave the advancing-height cursor unchanged.
        if provider.block_hash(next.number).await? == Some(next.hash) {
            return Ok(Changes {
                reorg: None,
                headers: Vec::new(),
            });
        }
    }
    if next.number > previous.number {
        let mut headers = Vec::new();
        let mut parent = previous.hash;
        let mut continuous = true;
        for number in previous.number + 1..next.number {
            let Some(hash) = provider.block_hash(number).await? else {
                if finality == Finality::Finalized {
                    return Err(HeadError::MissingBlock { number });
                }
                continuous = false;
                break;
            };
            let header = match required_header(provider, hash).await {
                Err(HeadError::MissingHeader { .. }) if finality == Finality::Best => {
                    continuous = false;
                    break;
                }
                result => result?,
            };
            if header.number != number {
                return Err(HeadError::InvalidHeader(
                    "height lookup returned a different block number".into(),
                ));
            }
            if header.parent_hash != Some(parent) {
                continuous = false;
                break;
            }
            parent = header.hash;
            headers.push(header);
        }
        continuous &= next.parent_hash == Some(parent);
        if continuous {
            headers.push(next);
            return Ok(Changes {
                reorg: None,
                headers,
            });
        }
    }
    if finality == Finality::Finalized {
        return Err(HeadError::FinalityViolation {
            previous: previous.hash,
            replacement: next.hash,
        });
    }
    let mut left = previous.clone();
    let mut right = next.clone();
    let mut replacements = Vec::new();
    while left.hash != right.hash {
        if left.number >= right.number {
            left = parent_header(provider, &left).await?;
        } else {
            replacements.push(right.clone());
            right = parent_header(provider, &right).await?;
        }
    }
    // When heads have equal height, the left descent runs first, then the
    // right descent catches up and records each replacement exactly once.
    replacements.reverse();
    if replacements.is_empty() {
        // A best head can roll back directly to its own ancestor.
        replacements.push(next.clone());
    }
    let reorg = (left.hash != previous.hash).then(|| {
        Arc::new(Reorg {
            previous: previous.clone(),
            replacement: next,
            common_ancestor: left,
        })
    });
    Ok(Changes {
        reorg,
        headers: replacements,
    })
}

async fn parent_header(
    provider: &dyn HeadProvider,
    header: &BlockHeader,
) -> Result<BlockHeader, HeadError> {
    if header.number == 0 {
        return Err(HeadError::InvalidHeader(
            "branches have different genesis identities".into(),
        ));
    }
    let parent = header
        .parent_hash
        .ok_or_else(|| HeadError::InvalidHeader("missing parent identity".into()))?;
    let parent = required_header(provider, parent).await?;
    if parent.number.checked_add(1) != Some(header.number) {
        return Err(HeadError::InvalidHeader(
            "parent height is not one below its child".into(),
        ));
    }
    Ok(parent)
}

async fn required_header(
    provider: &dyn HeadProvider,
    hash: H256,
) -> Result<BlockHeader, HeadError> {
    let header = provider
        .header(hash)
        .await?
        .ok_or(HeadError::MissingHeader { hash })?;
    if header.hash != hash {
        return Err(HeadError::InvalidHeader(
            "hash lookup returned a different header identity".into(),
        ));
    }
    Ok(header)
}

fn hex_hash(hash: H256) -> String {
    format!("0x{}", hex::encode(hash.0))
}

fn parse_hash(value: &Value, name: &str) -> Result<H256, HeadError> {
    let value = value
        .as_str()
        .and_then(|value| value.strip_prefix("0x"))
        .filter(|value| value.len() == 64)
        .ok_or_else(|| HeadError::InvalidHeader(format!("{name} must be a 32-byte hex hash")))?;
    let mut bytes = [0; 32];
    hex::decode_to_slice(value, &mut bytes)
        .map_err(|error| HeadError::InvalidHeader(format!("invalid {name}: {error}")))?;
    Ok(H256(bytes))
}

fn parse_number(value: &Value) -> Result<u64, HeadError> {
    if let Some(value) = value.as_u64() {
        return Ok(value);
    }
    if let Some(value) = value.as_str() {
        let result = match value.strip_prefix("0x") {
            Some(value) => u64::from_str_radix(value, 16),
            None => value.parse(),
        };
        return result.map_err(|error| HeadError::InvalidHeader(error.to_string()));
    }
    Err(HeadError::InvalidHeader(
        "number must be an unsigned integer or hex string".into(),
    ))
}

fn header_hash(raw: &Value) -> Result<H256, HeadError> {
    let field = |name: &str| {
        raw.get(name)
            .ok_or_else(|| HeadError::InvalidHeader(format!("missing {name}")))
    };
    let parent = parse_hash(field("parentHash")?, "parentHash")?;
    let number = parse_number(field("number")?)?;
    let number = u32::try_from(number)
        .map_err(|_| HeadError::InvalidHeader("number exceeds native chain u32".into()))?;
    let state = parse_hash(field("stateRoot")?, "stateRoot")?;
    let extrinsics = parse_hash(field("extrinsicsRoot")?, "extrinsicsRoot")?;
    let logs = field("digest")?
        .get("logs")
        .and_then(Value::as_array)
        .ok_or_else(|| HeadError::InvalidHeader("digest.logs must be an array".into()))?;
    let count = u32::try_from(logs.len())
        .map_err(|_| HeadError::InvalidHeader("too many digest logs".into()))?;
    let mut encoded = Vec::new();
    parent.0.encode_to(&mut encoded);
    Compact(number).encode_to(&mut encoded);
    state.0.encode_to(&mut encoded);
    extrinsics.0.encode_to(&mut encoded);
    Compact(count).encode_to(&mut encoded);
    for log in logs {
        let value = log
            .as_str()
            .and_then(|value| value.strip_prefix("0x"))
            .ok_or_else(|| HeadError::InvalidHeader("digest log must be hex".into()))?;
        let log = hex::decode(value)
            .map_err(|error| HeadError::InvalidHeader(format!("invalid digest log: {error}")))?;
        validate_digest(&log)?;
        encoded.extend(log);
    }
    Ok(H256(Blake2b::<U32>::digest(encoded).into()))
}

fn validate_digest(encoded: &[u8]) -> Result<(), HeadError> {
    // Substrate DigestItem's SCALE discriminants are stable on the chain.
    // Reject truncated logs and trailing bytes before committing their hash.
    let mut bytes = encoded;
    let tag =
        u8::decode(&mut bytes).map_err(|error| HeadError::InvalidHeader(error.to_string()))?;
    match tag {
        0 => {
            let _ = Vec::<u8>::decode(&mut bytes)
                .map_err(|error| HeadError::InvalidHeader(error.to_string()))?;
        }
        4..=6 => {
            let _ = <[u8; 4]>::decode(&mut bytes)
                .map_err(|error| HeadError::InvalidHeader(error.to_string()))?;
            let _ = Vec::<u8>::decode(&mut bytes)
                .map_err(|error| HeadError::InvalidHeader(error.to_string()))?;
        }
        8 => {}
        _ => return Err(HeadError::InvalidHeader("unknown digest item kind".into())),
    }
    if !bytes.is_empty() {
        return Err(HeadError::InvalidHeader("digest has trailing bytes".into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::BTreeMap, sync::Mutex, time::Duration};
    use tokio::sync::mpsc;

    #[derive(Default)]
    struct FakeProvider {
        state: Mutex<FakeState>,
    }

    #[derive(Default)]
    struct FakeState {
        headers: BTreeMap<H256, BlockHeader>,
        canonical: BTreeMap<u64, H256>,
        subscriptions: Vec<(
            Finality,
            mpsc::UnboundedSender<Result<BlockHeader, HeadError>>,
        )>,
        height_lookups: Vec<u64>,
        next_subscribe_error: Option<HeadError>,
    }

    impl FakeProvider {
        fn with_chain(height: u64) -> Result<(Arc<Self>, Vec<BlockHeader>), HeadError> {
            let provider = Arc::new(Self::default());
            let mut chain = Vec::new();
            let mut parent = None;
            for number in 0..=height {
                let header = test_header(number, 0, parent);
                parent = Some(header.hash);
                provider.insert(header.clone(), true)?;
                chain.push(header);
            }
            Ok((provider, chain))
        }

        fn insert(&self, header: BlockHeader, canonical: bool) -> Result<(), HeadError> {
            let mut state = self.state.lock().map_err(lock_error)?;
            if canonical {
                state.canonical.insert(header.number, header.hash);
            }
            state.headers.insert(header.hash, header);
            Ok(())
        }

        fn emit(
            &self,
            finality: Finality,
            item: Result<BlockHeader, HeadError>,
        ) -> Result<(), HeadError> {
            let state = self.state.lock().map_err(lock_error)?;
            for (kind, sender) in &state.subscriptions {
                if *kind == finality {
                    let _ = sender.send(item.clone());
                }
            }
            Ok(())
        }

        async fn wait_for_subscriptions(&self, count: usize) -> Result<(), HeadError> {
            tokio::time::timeout(Duration::from_secs(2), async {
                loop {
                    if self.state.lock().map_err(lock_error)?.subscriptions.len() >= count {
                        return Ok(());
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .map_err(|_| HeadError::InvalidHeader("subscription wait timed out".into()))?
        }
    }

    impl HeadProvider for FakeProvider {
        fn subscribe(&self, finality: Finality) -> HeadFuture<'_, BlockStream> {
            Box::pin(async move {
                if let Some(error) = self
                    .state
                    .lock()
                    .map_err(lock_error)?
                    .next_subscribe_error
                    .take()
                {
                    return Err(error);
                }
                let (sender, receiver) = mpsc::unbounded_channel();
                self.state
                    .lock()
                    .map_err(lock_error)?
                    .subscriptions
                    .push((finality, sender));
                Ok(Box::pin(futures::stream::unfold(
                    (receiver, false),
                    |(mut receiver, ended)| async move {
                        if ended {
                            return None;
                        }
                        let item = receiver.recv().await?;
                        let ended = item.is_err();
                        Some((item, (receiver, ended)))
                    },
                )) as BlockStream)
            })
        }

        fn block_hash(&self, number: u64) -> HeadFuture<'_, Option<H256>> {
            Box::pin(async move {
                let mut state = self.state.lock().map_err(lock_error)?;
                state.height_lookups.push(number);
                Ok(state.canonical.get(&number).copied())
            })
        }

        fn header(&self, hash: H256) -> HeadFuture<'_, Option<BlockHeader>> {
            Box::pin(async move {
                Ok(self
                    .state
                    .lock()
                    .map_err(lock_error)?
                    .headers
                    .get(&hash)
                    .cloned())
            })
        }
    }

    fn lock_error<T>(_: std::sync::PoisonError<T>) -> HeadError {
        HeadError::InvalidHeader("fake lock poisoned".into())
    }

    fn test_header(number: u64, branch: u8, parent_hash: Option<H256>) -> BlockHeader {
        let mut bytes = [0; 32];
        bytes[..8].copy_from_slice(&number.to_le_bytes());
        bytes[8] = branch;
        let hash = H256(bytes);
        BlockHeader {
            number,
            hash,
            parent_hash,
            raw: json!({"number": format!("0x{number:x}"), "hash":hex_hash(hash)}),
        }
    }

    async fn head_at(
        mut receiver: watch::Receiver<BlockHeader>,
        hash: H256,
    ) -> Result<(), HeadError> {
        tokio::time::timeout(Duration::from_secs(2), async move {
            loop {
                if receiver.borrow_and_update().hash == hash {
                    return Ok(());
                }
                receiver
                    .changed()
                    .await
                    .map_err(|_| HeadError::InvalidHeader("head worker ended early".into()))?;
            }
        })
        .await
        .map_err(|_| HeadError::InvalidHeader("head wait timed out".into()))?
    }

    async fn next(stream: &mut BlockStream) -> Result<Result<BlockHeader, HeadError>, HeadError> {
        tokio::time::timeout(Duration::from_secs(2), stream.next())
            .await
            .map_err(|_| HeadError::InvalidHeader("block stream timed out".into()))?
            .ok_or_else(|| HeadError::InvalidHeader("block stream ended early".into()))
    }

    #[test]
    fn header_hash_matches_independent_pinned_chain_headers() -> Result<(), HeadError> {
        // Retrieved with chain_getHeader on 2026-10-14 from the generated
        // finney genesis and the already pinned 478 metadata block, respectively.
        let genesis = json!({
            "digest":{"logs":[]},
            "extrinsicsRoot":"0x03170a2e7597b7b7e3d84c05391d139a62b157e78786d8c082f29dcf4c111314",
            "number":"0x0",
            "parentHash":"0x0000000000000000000000000000000000000000000000000000000000000000",
            "stateRoot":"0x4015a36c6762db581cd56ec40af2a77ccd2a275fa96091358149d63293fc9643"
        });
        let expected = parse_hash(
            &json!(crate::generated::network_pins::FINNEY_GENESIS),
            "hash",
        )?;
        assert_eq!(BlockHeader::from_json(genesis)?.hash, expected);
        let finalized = json!({
            "digest":{"logs":[
                "0x0661757261202197e60800000000",
                "0x0466726f6e89010195e24fe88e1095c057f600ba8617b0523fe79002ae9389c7119ee7921b99e1c808c035992f34af177c7ec880a5edc8ab2b49c2bd8afaef5612a324a391bfa3a66e06c53a13d7d9bc8611165a366ce394bbf2abca9b252d69dac9a8ba5d120ce018",
                "0x056175726101010815d5a69ed09972c2f53f2de809d3354cc84ba25c4f7e05e48f353c69ded91b198f293e75813a134055c9c04708594d4e78d67746199a39ea5387d4b0eed78d"
            ]},
            "extrinsicsRoot":"0x0edd11a4c576bc48547e100d5c47f23acdc13e3bdaaad97b41ad7c6e7fbf57d1",
            "number":"0x8d97f7",
            "parentHash":"0x3f9b325503be5a64604850458aec55130ba16af1ba4ff4c9c8cbacf9233654fa",
            "stateRoot":"0xd941c17ef7b8c294a81607fd86a68c2dee7f23e4577680d2e1442142e71f8e70"
        });
        let expected = parse_hash(
            &json!("0x2bc979722ab7c83b9992900c5463dfd1daebbac77d5e64ae1e45413c69e5401f"),
            "hash",
        )?;
        let parsed = BlockHeader::from_json(finalized.clone())?;
        assert_eq!(parsed.hash, expected);
        assert_eq!(parsed.number, 9_279_479);
        assert_eq!(parsed.raw, finalized);
        Ok(())
    }

    #[test]
    fn malformed_header_and_digest_are_rejected() {
        for number in [json!(-1), json!("0x"), json!("not-a-height"), json!(null)] {
            assert!(parse_number(&number).is_err());
        }
        for digest in [vec![], vec![8, 0], vec![6, 1, 2], vec![9], vec![0, 4]] {
            assert!(validate_digest(&digest).is_err());
        }
        assert!(validate_digest(&[8]).is_ok());
        assert!(validate_digest(&[0, 4, 42]).is_ok());
        assert!(parse_hash(&json!("00"), "hash").is_err());
        assert!(BlockHeader::from_json(json!({"number": 5})).is_err());
    }

    #[tokio::test]
    async fn finalized_gap_backfills_each_missing_height_by_number() -> Result<(), HeadError> {
        let (provider, chain) = FakeProvider::with_chain(5)?;
        let changes = reconcile(
            provider.as_ref(),
            Finality::Finalized,
            &chain[1],
            chain[5].clone(),
        )
        .await?;
        assert!(changes.reorg.is_none());
        assert_eq!(changes.headers, chain[2..=5]);
        assert_eq!(
            provider.state.lock().map_err(lock_error)?.height_lookups,
            [2, 3, 4]
        );
        let duplicate = reconcile(
            provider.as_ref(),
            Finality::Finalized,
            &chain[5],
            chain[5].clone(),
        )
        .await?;
        assert!(duplicate.headers.is_empty());
        let replay = reconcile(
            provider.as_ref(),
            Finality::Finalized,
            &chain[5],
            chain[2].clone(),
        )
        .await?;
        assert!(replay.headers.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn finalized_fork_is_terminal_and_does_not_publish_replacements() -> Result<(), HeadError>
    {
        let (provider, chain) = FakeProvider::with_chain(3)?;
        let fork = test_header(3, 1, Some(chain[2].hash));
        provider.insert(fork.clone(), true)?;
        assert!(matches!(
            reconcile(provider.as_ref(), Finality::Finalized, &chain[3], fork).await,
            Err(HeadError::FinalityViolation { .. })
        ));
        Ok(())
    }

    #[tokio::test]
    async fn best_gap_uses_pinned_ancestry_when_canonical_lookup_has_changed()
    -> Result<(), HeadError> {
        let (provider, chain) = FakeProvider::with_chain(5)?;
        // A later best tip can retract a canonical height while a previously
        // delivered notification still identifies an independently retrievable branch.
        provider
            .state
            .lock()
            .map_err(lock_error)?
            .canonical
            .remove(&3);
        let changes = reconcile(
            provider.as_ref(),
            Finality::Best,
            &chain[1],
            chain[5].clone(),
        )
        .await?;
        assert!(changes.reorg.is_none());
        assert_eq!(changes.headers, chain[2..=5]);
        assert_eq!(
            provider.state.lock().map_err(lock_error)?.height_lookups,
            [2, 3]
        );
        Ok(())
    }

    #[tokio::test]
    async fn missing_gap_and_invalid_parent_height_fail_without_partial_publication()
    -> Result<(), HeadError> {
        let (provider, chain) = FakeProvider::with_chain(3)?;
        provider
            .state
            .lock()
            .map_err(lock_error)?
            .canonical
            .remove(&2);
        assert!(matches!(
            reconcile(
                provider.as_ref(),
                Finality::Finalized,
                &chain[0],
                chain[3].clone()
            )
            .await,
            Err(HeadError::MissingBlock { number: 2 })
        ));
        let invalid = test_header(4, 1, Some(chain[1].hash));
        assert!(matches!(
            parent_header(provider.as_ref(), &invalid).await,
            Err(HeadError::InvalidHeader(_))
        ));
        Ok(())
    }

    #[tokio::test]
    async fn reconnect_resubscribes_and_backfills_without_duplicate_finalized_heights()
    -> Result<(), HeadError> {
        let (provider, chain) = FakeProvider::with_chain(5)?;
        let cancellation = CancellationToken::new();
        let tracker = HeadTracker::start(
            provider.clone(),
            chain[0].clone(),
            chain[0].clone(),
            cancellation.clone(),
        )
        .await?;
        let mut blocks = tracker.blocks(Finality::Finalized);
        assert_eq!(next(&mut blocks).await??.number, 0);
        provider.emit(Finality::Finalized, Ok(chain[2].clone()))?;
        head_at(tracker.finalized(), chain[2].hash).await?;
        provider
            .state
            .lock()
            .map_err(lock_error)?
            .next_subscribe_error = Some(RpcError::ConnectionLost { resent: false }.into());
        provider.emit(Finality::Finalized, Err(RpcError::SubscriptionLost.into()))?;
        provider.wait_for_subscriptions(3).await?;
        provider.emit(Finality::Finalized, Ok(chain[5].clone()))?;
        head_at(tracker.finalized(), chain[5].hash).await?;
        for height in 1..=5 {
            assert_eq!(next(&mut blocks).await??.number, height);
        }
        cancellation.cancel();
        Ok(())
    }

    #[tokio::test]
    async fn slow_finalized_consumer_recovers_beyond_capacity_and_retains_terminal_failure()
    -> Result<(), HeadError> {
        let (provider, chain) = FakeProvider::with_chain(300)?;
        let cancellation = CancellationToken::new();
        let tracker = HeadTracker::start(
            provider.clone(),
            chain[0].clone(),
            chain[0].clone(),
            cancellation.clone(),
        )
        .await?;
        let mut blocks = tracker.blocks(Finality::Finalized);
        assert_eq!(next(&mut blocks).await??.number, 0);
        provider.emit(Finality::Finalized, Ok(chain[300].clone()))?;
        head_at(tracker.finalized(), chain[300].hash).await?;
        provider.emit(Finality::Finalized, Err(RpcError::Closed.into()))?;
        let mut finalized = tracker.finalized();
        let _ = finalized.borrow_and_update();
        assert!(
            tokio::time::timeout(Duration::from_secs(2), finalized.changed())
                .await
                .map_err(|_| HeadError::InvalidHeader("worker failure timed out".into()))?
                .is_err()
        );
        for height in 1..=300 {
            assert_eq!(next(&mut blocks).await??.number, height);
        }
        assert!(matches!(
            next(&mut blocks).await?,
            Err(HeadError::Rpc(RpcError::Closed))
        ));
        assert!(blocks.next().await.is_none());
        cancellation.cancel();
        Ok(())
    }

    #[tokio::test]
    async fn best_fork_reports_common_ancestor_then_each_replacement_once() -> Result<(), HeadError>
    {
        let (provider, chain) = FakeProvider::with_chain(3)?;
        let fork2 = test_header(2, 1, Some(chain[1].hash));
        let fork3 = test_header(3, 1, Some(fork2.hash));
        let fork4 = test_header(4, 1, Some(fork3.hash));
        let fork5 = test_header(5, 1, Some(fork4.hash));
        for header in [&fork2, &fork3, &fork4, &fork5] {
            provider.insert(header.clone(), true)?;
        }
        let cancellation = CancellationToken::new();
        let tracker = HeadTracker::start(
            provider.clone(),
            chain[3].clone(),
            chain[1].clone(),
            cancellation.clone(),
        )
        .await?;
        let mut blocks = tracker.blocks(Finality::Best);
        assert_eq!(next(&mut blocks).await??.hash, chain[3].hash);
        provider.emit(Finality::Best, Ok(fork4.clone()))?;
        head_at(tracker.best(), fork4.hash).await?;
        let error = next(&mut blocks).await?;
        assert!(
            matches!(error, Err(HeadError::Reorg(ref reorg)) if reorg.common_ancestor == chain[1] && reorg.replacement == fork4)
        );
        for header in [&fork2, &fork3, &fork4] {
            assert_eq!(next(&mut blocks).await??, *header);
        }
        provider.emit(Finality::Best, Ok(fork5.clone()))?;
        assert_eq!(next(&mut blocks).await??, fork5);
        cancellation.cancel();
        Ok(())
    }

    #[tokio::test]
    async fn best_rollback_updates_watch_and_reports_the_ancestor_head() -> Result<(), HeadError> {
        let (provider, chain) = FakeProvider::with_chain(3)?;
        let cancellation = CancellationToken::new();
        let tracker = HeadTracker::start(
            provider.clone(),
            chain[3].clone(),
            chain[0].clone(),
            cancellation.clone(),
        )
        .await?;
        let mut blocks = tracker.blocks(Finality::Best);
        let _ = next(&mut blocks).await??;
        provider.emit(Finality::Best, Ok(chain[1].clone()))?;
        head_at(tracker.best(), chain[1].hash).await?;
        assert!(matches!(next(&mut blocks).await?, Err(HeadError::Reorg(_))));
        assert_eq!(next(&mut blocks).await??, chain[1]);
        cancellation.cancel();
        Ok(())
    }

    #[tokio::test]
    async fn strict_stream_keeps_subscription_loss_terminal() -> Result<(), HeadError> {
        let (provider, chain) = FakeProvider::with_chain(0)?;
        let cancellation = CancellationToken::new();
        let tracker = HeadTracker::start(
            provider.clone(),
            chain[0].clone(),
            chain[0].clone(),
            cancellation.clone(),
        )
        .await?;
        let mut strict = tracker.blocks_strict(Finality::Finalized).await?;
        let sender = provider.state.lock().map_err(lock_error)?.subscriptions[2]
            .1
            .clone();
        sender
            .send(Err(RpcError::SubscriptionLost.into()))
            .map_err(|_| HeadError::InvalidHeader("strict receiver closed".into()))?;
        assert!(matches!(
            next(&mut strict).await?,
            Err(HeadError::Rpc(RpcError::SubscriptionLost))
        ));
        assert!(strict.next().await.is_none());
        assert_eq!(
            provider
                .state
                .lock()
                .map_err(lock_error)?
                .subscriptions
                .len(),
            3
        );
        cancellation.cancel();
        Ok(())
    }

    #[tokio::test]
    async fn cancellation_stops_workers_and_streams_even_when_tracker_is_retained()
    -> Result<(), HeadError> {
        let (provider, chain) = FakeProvider::with_chain(0)?;
        let cancellation = CancellationToken::new();
        let tracker = HeadTracker::start(
            provider.clone(),
            chain[0].clone(),
            chain[0].clone(),
            cancellation.clone(),
        )
        .await?;
        let mut best = tracker.best();
        let mut finalized = tracker.finalized();
        let mut blocks = tracker.blocks(Finality::Best);
        cancellation.cancel();
        assert!(blocks.next().await.is_none());
        for watch in [&mut best, &mut finalized] {
            assert!(
                tokio::time::timeout(Duration::from_secs(2), watch.changed())
                    .await
                    .map_err(|_| HeadError::InvalidHeader("cancellation timed out".into()))?
                    .is_err()
            );
        }
        assert!(
            provider
                .state
                .lock()
                .map_err(lock_error)?
                .subscriptions
                .iter()
                .all(|(_, sender)| sender.is_closed())
        );
        Ok(())
    }

    #[tokio::test]
    async fn terminal_policy_failure_is_retained_for_late_followers() -> Result<(), HeadError> {
        let (provider, chain) = FakeProvider::with_chain(0)?;
        let cancellation = CancellationToken::new();
        let tracker = HeadTracker::start(
            provider.clone(),
            chain[0].clone(),
            chain[0].clone(),
            cancellation.clone(),
        )
        .await?;
        let policy = rtao_rpc::policy::classify_rpc_error(&json!({
            "code":-32005, "message":"slow down", "data":{"policy":"work", "retry_after":"10"}
        }))
        .ok_or_else(|| HeadError::InvalidHeader("policy fixture not classified".into()))?;
        provider.emit(
            Finality::Finalized,
            Err(RpcError::Policy(Box::new(policy.clone())).into()),
        )?;
        let mut heads = tracker.finalized();
        assert!(
            tokio::time::timeout(Duration::from_secs(2), heads.changed())
                .await
                .map_err(|_| HeadError::InvalidHeader("failure timed out".into()))?
                .is_err()
        );
        let mut late = tracker.blocks(Finality::Finalized);
        assert_eq!(next(&mut late).await??.number, 0);
        assert!(
            matches!(next(&mut late).await?, Err(HeadError::Rpc(RpcError::Policy(found))) if *found == policy)
        );
        assert!(late.next().await.is_none());
        assert_eq!(
            provider
                .state
                .lock()
                .map_err(lock_error)?
                .subscriptions
                .len(),
            2
        );
        cancellation.cancel();
        Ok(())
    }
}
