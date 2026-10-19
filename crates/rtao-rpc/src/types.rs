// Derived from RaoFoundation/subtensor sdk/python/bittensor/_transport/rpc.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Session configuration, errors, and resend-safe methods.
use crate::{PolicyRefusal, generated::py_rpc};
use serde_json::Value;
use std::{fmt, str::FromStr, time::Duration};
use url::Url;

/// A 32-byte chain hash, serialized using the Substrate hexadecimal wire format.
pub use primitive_types::H256;
/// A transaction-pool watch update.
pub type TxStatus = subxt_rpcs::methods::legacy::TransactionStatus<H256>;

/// A method permitted to be resent after connection loss.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ReadMethod {
    /// Runtime API invocation.
    StateCall,
    /// Read a storage key.
    StateGetStorage,
    /// Read a storage key at a block.
    StateGetStorageAt,
    /// Read the hash of a storage value.
    StateGetStorageHash,
    /// Read several storage keys.
    StateQueryStorageAt,
    /// Read a page of keys.
    StateGetKeysPaged,
    /// Read runtime metadata.
    StateGetMetadata,
    /// Read runtime versions.
    StateGetRuntimeVersion,
    /// Read the best block hash.
    ChainGetHead,
    /// Read the finalized block hash.
    ChainGetFinalizedHead,
    /// Resolve a block number.
    ChainGetBlockHash,
    /// Read a block header.
    ChainGetHeader,
    /// Read a block and its extrinsics.
    ChainGetBlock,
    /// Python's account nonce method.
    AccountNextIndex,
    /// Standard account nonce method.
    SystemAccountNextIndex,
    /// Read chain properties.
    SystemProperties,
    /// Read chain name.
    SystemChain,
    /// Read node health.
    SystemHealth,
    /// Discover RPC methods.
    RpcMethods,
}
impl ReadMethod {
    /// The exact JSON-RPC spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::StateCall => "state_call",
            Self::StateGetStorage => "state_getStorage",
            Self::StateGetStorageAt => "state_getStorageAt",
            Self::StateGetStorageHash => "state_getStorageHash",
            Self::StateQueryStorageAt => "state_queryStorageAt",
            Self::StateGetKeysPaged => "state_getKeysPaged",
            Self::StateGetMetadata => "state_getMetadata",
            Self::StateGetRuntimeVersion => "state_getRuntimeVersion",
            Self::ChainGetHead => "chain_getHead",
            Self::ChainGetFinalizedHead => "chain_getFinalizedHead",
            Self::ChainGetBlockHash => "chain_getBlockHash",
            Self::ChainGetHeader => "chain_getHeader",
            Self::ChainGetBlock => "chain_getBlock",
            Self::AccountNextIndex => "account_nextIndex",
            Self::SystemAccountNextIndex => "system_accountNextIndex",
            Self::SystemProperties => "system_properties",
            Self::SystemChain => "system_chain",
            Self::SystemHealth => "system_health",
            Self::RpcMethods => "rpc_methods",
        }
    }
}
impl FromStr for ReadMethod {
    type Err = RpcError;
    fn from_str(method: &str) -> Result<Self, Self::Err> {
        const METHODS: &[ReadMethod] = &[
            ReadMethod::StateCall,
            ReadMethod::StateGetStorage,
            ReadMethod::StateGetStorageAt,
            ReadMethod::StateGetStorageHash,
            ReadMethod::StateQueryStorageAt,
            ReadMethod::StateGetKeysPaged,
            ReadMethod::StateGetMetadata,
            ReadMethod::StateGetRuntimeVersion,
            ReadMethod::ChainGetHead,
            ReadMethod::ChainGetFinalizedHead,
            ReadMethod::ChainGetBlockHash,
            ReadMethod::ChainGetHeader,
            ReadMethod::ChainGetBlock,
            ReadMethod::AccountNextIndex,
            ReadMethod::SystemAccountNextIndex,
            ReadMethod::SystemProperties,
            ReadMethod::SystemChain,
            ReadMethod::SystemHealth,
            ReadMethod::RpcMethods,
        ];
        METHODS
            .iter()
            .copied()
            .find(|m| m.as_str() == method)
            .ok_or_else(|| RpcError::Protocol(format!("method is not resend-safe: {method}")))
    }
}
/// A legacy node subscription.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SubscriptionKind {
    /// Best block headers.
    NewHeads,
    /// Finalized block headers.
    FinalizedHeads,
    /// Runtime version updates.
    RuntimeVersion,
}
impl SubscriptionKind {
    pub(crate) fn methods(self) -> (&'static str, &'static str) {
        match self {
            Self::NewHeads => ("chain_subscribeNewHeads", "chain_unsubscribeNewHeads"),
            Self::FinalizedHeads => (
                "chain_subscribeFinalizedHeads",
                "chain_unsubscribeFinalizedHeads",
            ),
            Self::RuntimeVersion => (
                "state_subscribeRuntimeVersion",
                "state_unsubscribeRuntimeVersion",
            ),
        }
    }
}
/// Ordered ordinary and lazy archive endpoints.
#[derive(Clone, Debug, Default)]
pub struct EndpointPool {
    /// Primary first, followed by failover endpoints.
    pub urls: Vec<Url>,
    /// Archive endpoints, connected only after a discarded-state error.
    pub archive: Vec<Url>,
}
impl EndpointPool {
    /// Pin a session to one endpoint.
    pub fn new(primary: Url) -> Self {
        Self {
            urls: vec![primary],
            archive: Vec::new(),
        }
    }
    /// Append fallback endpoints, excluding repetitions of the primary URL.
    pub fn with_fallbacks(mut self, fallbacks: impl IntoIterator<Item = Url>) -> Self {
        let primary = self.urls.first().cloned();
        self.urls.extend(
            fallbacks
                .into_iter()
                .filter(|url| Some(url) != primary.as_ref()),
        );
        self
    }
    /// Attach the lazy archive endpoint pool.
    pub fn with_archives(mut self, archive: impl IntoIterator<Item = Url>) -> Self {
        self.archive = archive.into_iter().collect();
        self
    }
}
/// Policy for unencrypted WebSockets.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PlaintextPolicy {
    /// Permit plaintext and log a warning.
    #[default]
    AllowWithWarning,
    /// Permit plaintext only for loopback hosts.
    LoopbackOnly,
    /// Reject every plaintext endpoint.
    Refuse,
}
/// Token-bucket request pacing.
#[derive(Clone, Copy, Debug)]
pub struct RateLimit {
    /// Sustained number of request tokens replenished each second.
    pub per_second: f64,
    /// Maximum number of accumulated tokens.
    pub burst: u32,
}
impl RateLimit {
    /// Public endpoint preset: one request per second sustained, with ten
    /// burst tokens so a short command's startup reads are not serialized.
    ///
    /// Bounded observations of the public pools support this pacing: each
    /// accepted a back-to-back burst of twelve light reads. Providers do not
    /// publish a quota in successful upgrade headers, so this is a conservative
    /// per-session preset rather than a provider quota guarantee. Multiple
    /// sessions sharing an IP must coordinate their combined traffic.
    pub const PUBLIC: Self = Self {
        per_second: 1.0,
        burst: 10,
    };
}
/// Configuration for one supervised WebSocket session.
#[derive(Clone)]
pub struct SessionConfig {
    /// Primary, fallback and archive pools.
    pub pool: EndpointPool,
    /// Maximum WebSocket handshake duration.
    pub handshake_timeout: Duration,
    /// Silence with unanswered requests before reconnecting.
    pub silence_timeout: Duration,
    /// Consecutive losses per endpoint before rotation.
    pub max_retries: u32,
    /// Continue cycling endpoints indefinitely.
    pub retry_forever: bool,
    /// Per-subscription retained notifications; overflow drops the oldest.
    pub subscription_buffer: usize,
    /// Session-wide active update budget, including streams from earlier connections.
    /// Defaults to 32 MiB. Charges encoded JSON plus conservative node/container
    /// storage without a second encoded allocation; overflow evicts oldest updates
    /// across streams. A single larger update is dropped without flushing queues.
    /// This bounds queued values, not transient frame parsing or total process RSS.
    pub subscription_buffer_bytes: usize,
    /// Maximum distinct unclaimed early-notification IDs.
    pub early_notification_ids: usize,
    /// Aggregate early-update budget (default 8 MiB), using the same JSON/storage
    /// charge plus identity bookkeeping. Only live, transmitted subscription
    /// establishment requests admit early updates; oldest updates are evicted first.
    pub early_notification_bytes: usize,
    /// Both maximum message and frame size.
    pub max_message_bytes: usize,
    /// Certificates appended to native root certificates.
    pub extra_roots: Vec<rustls_pki_types::CertificateDer<'static>>,
    /// Maximum transmitted unanswered requests.
    pub max_in_flight: usize,
    /// Optional request token bucket.
    pub rate_limit: Option<RateLimit>,
    /// Additional handshake headers, redacted by the transport logger.
    pub headers: http::HeaderMap,
    /// Permission for plaintext endpoints.
    pub plaintext_ws: PlaintextPolicy,
}
impl fmt::Debug for SessionConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SessionConfig")
            .field(
                "endpoints",
                &self
                    .pool
                    .urls
                    .iter()
                    .map(|url| crate::policy::safe_endpoint(url.as_str()))
                    .collect::<Vec<_>>(),
            )
            .field(
                "archive_endpoints",
                &self
                    .pool
                    .archive
                    .iter()
                    .map(|url| crate::policy::safe_endpoint(url.as_str()))
                    .collect::<Vec<_>>(),
            )
            .field("handshake_timeout", &self.handshake_timeout)
            .field("silence_timeout", &self.silence_timeout)
            .field("max_retries", &self.max_retries)
            .field("retry_forever", &self.retry_forever)
            .field("subscription_buffer_bytes", &self.subscription_buffer_bytes)
            .field("early_notification_bytes", &self.early_notification_bytes)
            .field("max_message_bytes", &self.max_message_bytes)
            .field("max_in_flight", &self.max_in_flight)
            .field("rate_limit", &self.rate_limit)
            .field("header_count", &self.headers.len())
            .field("extra_root_count", &self.extra_roots.len())
            .finish_non_exhaustive()
    }
}
impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            pool: EndpointPool::default(),
            handshake_timeout: Duration::from_secs_f64(py_rpc::HANDSHAKE_TIMEOUT),
            silence_timeout: Duration::from_secs_f64(py_rpc::DEFAULT_RESPONSE_TIMEOUT),
            max_retries: py_rpc::DEFAULT_MAX_RETRIES as u32,
            retry_forever: py_rpc::DEFAULT_RETRY_FOREVER,
            subscription_buffer: py_rpc::SUBSCRIPTION_QUEUE_MAXSIZE,
            subscription_buffer_bytes: 32 * 1024 * 1024,
            early_notification_ids: py_rpc::MAX_EARLY_BUFFERS,
            early_notification_bytes: 8 * 1024 * 1024,
            max_message_bytes: 256 * 1024 * 1024,
            extra_roots: Vec::new(),
            max_in_flight: 64,
            rate_limit: None,
            headers: http::HeaderMap::new(),
            plaintext_ws: PlaintextPolicy::default(),
        }
    }
}
impl SessionConfig {
    /// Configure a custom endpoint, without a public-endpoint rate limit.
    pub fn new(url: Url) -> Self {
        Self {
            pool: EndpointPool {
                urls: vec![url],
                archive: Vec::new(),
            },
            ..Self::default()
        }
    }
    /// Configure a public endpoint pool with the shared public rate preset.
    /// Network names and endpoint lists are resolved by the client layer.
    pub fn public(pool: EndpointPool) -> Self {
        Self {
            pool,
            rate_limit: Some(RateLimit::PUBLIC),
            ..Self::default()
        }
    }
}
/// A recoverable or terminal transport failure.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum RpcError {
    /// A terminal gateway policy refusal.
    Policy(Box<PolicyRefusal>),
    /// Historical state was pruned from this endpoint.
    StateDiscarded {
        /// Requested block, as reported by the node.
        block: String,
    },
    /// All permitted consecutive endpoint retries were exhausted.
    MaxRetriesExceeded {
        /// Bounded final diagnostic.
        detail: String,
    },
    /// Session was explicitly shut down.
    Closed,
    /// Server-side subscription identity was lost on reconnect.
    SubscriptionLost,
    /// A non-resendable method lost its response.
    ConnectionLost {
        /// Whether this method was resent.
        resent: bool,
    },
    /// A transmitted watched extrinsic lost its connection and may already be on chain.
    FateUnknown {
        /// Blake2-256 hash of the submitted extrinsic.
        hash: H256,
    },
    /// A non-policy HTTP handshake rejection.
    Handshake {
        /// HTTP response status.
        status: u16,
        /// Bounded response detail.
        detail: String,
    },
    /// Handshake or caller deadline expired.
    Timeout {
        /// Endpoint-safe diagnostic.
        detail: String,
    },
    /// Ordinary server JSON-RPC error.
    JsonRpc {
        /// Server error code.
        code: i64,
        /// Server diagnostic.
        message: String,
        /// Optional server data.
        data: Option<Value>,
        /// Original error object, retained for exact Python-compatible diagnostics.
        payload: Box<Value>,
    },
    /// Invalid framing, parameters, configuration, or decoding.
    Protocol(String),
    /// Network or TLS failure.
    Transport(String),
}
impl fmt::Display for RpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Policy(p)=>fmt::Display::fmt(p,f), Self::StateDiscarded{block}=>write!(f,"State discarded for {block}. This indicates the block is too old, and you should instead make this request using an archive node."),
            Self::MaxRetriesExceeded{detail}|Self::Timeout{detail}=>f.write_str(detail), Self::Closed=>f.write_str("RPC session closed"),
            Self::SubscriptionLost=>f.write_str("Connection lost while waiting for a subscription response (e.g. extrinsic finalization). The transaction may already be on chain — verify balances before retrying."),
            Self::ConnectionLost{..}=>f.write_str("Connection lost while an RPC request was in flight"),
            Self::FateUnknown{hash}=>fmt::Display::fmt(&SubmitError::FateUnknown{hash:*hash},f),
            Self::Handshake{status,detail}=> {
                write!(f,"RPC endpoint rejected the WebSocket handshake (HTTP {status})")?;
                if !detail.is_empty() {write!(f,": {detail}")?;}
                Ok(())
            },
            Self::JsonRpc{payload,..}=>f.write_str(&crate::policy::format_rpc_error(payload)),
            Self::Protocol(detail)|Self::Transport(detail)=>f.write_str(detail),
        }
    }
}
impl std::error::Error for RpcError {}
/// A submission failure that distinguishes an unknown transaction outcome.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum SubmitError {
    /// Failure with a known transport cause.
    Rpc(RpcError),
    /// The transmitted transaction may already be in the pool or on chain.
    FateUnknown {
        /// Blake2-256 hash of the submitted extrinsic.
        hash: H256,
    },
}
impl fmt::Display for SubmitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {Self::Rpc(error)=>fmt::Display::fmt(error,f),Self::FateUnknown{..}=>f.write_str("Connection dropped while an extrinsic submission was in flight; the transaction may already be in the pool or on chain. Verify chain state before retrying.") }
    }
}
impl std::error::Error for SubmitError {}
impl From<RpcError> for SubmitError {
    fn from(error: RpcError) -> Self {
        Self::Rpc(error)
    }
}
/// Current supervisor connection state.
#[derive(Clone, Debug, PartialEq)]
pub enum SessionState {
    /// Active connection after resends have completed.
    Connected {
        /// Current endpoint.
        url: Url,
    },
    /// Connecting or backing off after consecutive failures.
    Reconnecting {
        /// Consecutive failure count.
        attempt: u32,
    },
    /// Permanent failure; pending callers have been released.
    Failed {
        /// Terminal reason.
        error: RpcError,
    },
    /// Explicit shutdown has completed.
    Closed,
}
/// Connection event for application telemetry.
#[derive(Clone, Debug)]
pub enum SessionEvent {
    /// A connection was established.
    Connected {
        /// Sanitized endpoint.
        endpoint: String,
    },
    /// A connection was lost and will be retried.
    Reconnecting {
        /// Consecutive failure count.
        attempt: u32,
        /// Sanitized endpoint.
        endpoint: String,
    },
    /// Terminal policy refusal.
    PolicyRefused(PolicyRefusal),
    /// A subscription update was discarded by a retention limit.
    SubscriptionOverflow {
        /// Subscription identity.
        id: String,
    },
}
/// A bounded snapshot of session routing state.
#[derive(Clone, Copy, Debug, Default)]
pub struct SessionStatistics {
    /// Unanswered requests, including queued requests.
    pub pending: usize,
    /// Active server subscriptions.
    pub subscriptions: usize,
    /// Unclaimed early notification identities.
    pub early_notification_ids: usize,
    /// Charged bytes held for early updates, including identity bookkeeping.
    pub early_notification_bytes: usize,
    /// Charged active-update bytes, including streams from earlier connections.
    pub subscription_retained_bytes: usize,
    /// Remembered recently closed subscription identities.
    pub closed_subscription_ids: usize,
}
