//! The reference `rtao-remote-signer/1` server.
//!
//! Requests pass the btauth/1 layer first (shape, receiver, freshness,
//! signature, client allowlist, replay commit, rate limit). The handler then
//! decodes the call with the server's trusted runtime, re-derives every signed
//! byte from the structured views, checks era and call policy, and signs only
//! the payload it derived itself.

mod handler;
pub mod policy;
pub mod rebuild;
pub mod runtime_source;

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    sync::Arc,
};

use axum::{
    Router,
    extract::DefaultBodyLimit,
    routing::{get, post},
};
use rtao_btauth::{
    Clock, NonceStore, Verifier, VerifyConfig, WrappedPolicy,
    server::{BtAuthLayer, LimiterConfig, PerHotkeyLimiter},
};
use rtao_primitives::{AccountId, CryptoType};
use rtao_signer::Signer;

pub use policy::{ClientAllowlist, DecodedCall, DecodedCallSubject, RemotePolicy, classify_call};
pub use rebuild::{EraInfo, RebuildError, Rebuilt, decode_era, rebuild};
#[cfg(feature = "server-online")]
#[cfg_attr(docsrs, doc(cfg(feature = "server-online")))]
pub use runtime_source::ClientRuntimeSource;
pub use runtime_source::{PinnedRuntimeSource, RuntimeSource, SourceError};

use crate::wire::DEFAULT_BODY_LIMIT;

/// One account the server signs for.
#[derive(Clone)]
pub struct ServedAccount {
    /// Any backend signer; its account and algorithm identify the entry.
    pub signer: Arc<dyn Signer>,
}

impl ServedAccount {
    /// Serve a signer.
    pub fn new(signer: Arc<dyn Signer>) -> Self {
        Self { signer }
    }
}

impl fmt::Debug for ServedAccount {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ServedAccount")
            .field("account", &self.signer.account())
            .field("crypto", &self.signer.crypto_type())
            .finish()
    }
}

/// One btauth client key and the accounts it may use.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthorizedClient {
    /// The client's btauth/1 sender identity.
    pub client: AccountId,
    /// Served accounts granted to it.
    pub accounts: BTreeSet<AccountId>,
}

/// RFC-0078 display constants used with the trusted runtime.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChainInfoConfig {
    /// Token decimals.
    pub decimals: u8,
    /// Token symbol.
    pub token_symbol: String,
}

impl Default for ChainInfoConfig {
    fn default() -> Self {
        Self {
            decimals: 9,
            token_symbol: "TAO".into(),
        }
    }
}

/// Server configuration; see [`RemoteSignerServer::new`] for validation.
#[derive(Clone, Debug)]
pub struct ServerConfig {
    /// btauth receiver identity; defaults to the sole served account.
    pub server_key: Option<AccountId>,
    /// Accounts the server signs for.
    pub accounts: Vec<ServedAccount>,
    /// Authorized client keys and their grants.
    pub clients: Vec<AuthorizedClient>,
    /// The only chain the server signs for.
    pub genesis_hash: [u8; 32],
    /// Display constants for metadata digests.
    pub chain_info: ChainInfoConfig,
    /// Operator policy.
    pub policy: RemotePolicy,
    /// btauth/1 verification; the receiver is required and signatures are strict.
    pub verify: VerifyConfig,
    /// Largest accepted request body.
    pub body_limit: usize,
    /// Optional per-client rate limiter.
    pub limiter: Option<LimiterConfig>,
}

impl ServerConfig {
    /// A configuration pinned to one chain, with secure defaults.
    pub fn new(genesis_hash: [u8; 32]) -> Self {
        Self {
            server_key: None,
            accounts: Vec::new(),
            clients: Vec::new(),
            genesis_hash,
            chain_info: ChainInfoConfig::default(),
            policy: RemotePolicy::default(),
            verify: VerifyConfig {
                wrapped: WrappedPolicy::Strict,
                ..VerifyConfig::default()
            },
            body_limit: DEFAULT_BODY_LIMIT,
            limiter: None,
        }
    }
}

/// A server configuration is unsafe or inconsistent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerConfigError(pub String);

impl fmt::Display for ServerConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
impl std::error::Error for ServerConfigError {}

fn config_error(message: impl Into<String>) -> ServerConfigError {
    ServerConfigError(message.into())
}

pub(crate) struct Inner {
    pub(crate) server_key: AccountId,
    pub(crate) accounts: BTreeMap<AccountId, Arc<dyn Signer>>,
    pub(crate) allowlist: ClientAllowlist,
    pub(crate) genesis_hash: [u8; 32],
    pub(crate) chain_info: ChainInfoConfig,
    pub(crate) policy: RemotePolicy,
    pub(crate) source: Arc<dyn RuntimeSource>,
}

/// The reference signing server.
#[derive(Clone)]
pub struct RemoteSignerServer {
    inner: Arc<Inner>,
    layer: BtAuthLayer,
    body_limit: usize,
}

impl fmt::Debug for RemoteSignerServer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RemoteSignerServer")
            .field("server_key", &self.inner.server_key)
            .field("accounts", &self.inner.accounts.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

impl RemoteSignerServer {
    /// Validate the configuration and build the authentication layer.
    ///
    /// Refuses: no accounts; several accounts without `server_key`; a client
    /// key equal to a served account or the server key; grants naming an
    /// unserved account; an optional receiver; `<Bytes>`-wrapped signature
    /// acceptance; replay retention not above the freshness window; a
    /// `max_era_period` outside the powers of two in `4..=65536`;
    /// `allowed_netuids` (no decoded call carries a subnet); `allow_raw_calls`
    /// together with `max_spend` or `max_fee` (raw calls cannot be measured);
    /// `max_fee` with an offline source; and an online source observing a
    /// different genesis.
    pub fn new(
        cfg: ServerConfig,
        source: Arc<dyn RuntimeSource>,
        store: Arc<dyn NonceStore>,
    ) -> Result<Self, ServerConfigError> {
        if cfg.accounts.is_empty() {
            return Err(config_error("the remote signer needs at least one account"));
        }
        let mut accounts = BTreeMap::new();
        for served in &cfg.accounts {
            if accounts
                .insert(served.signer.account(), served.signer.clone())
                .is_some()
            {
                return Err(config_error(format!(
                    "account {} is served twice",
                    served.signer.account()
                )));
            }
        }
        let server_key = match (cfg.server_key, accounts.keys().next()) {
            (Some(key), _) => key,
            (None, Some(only)) if accounts.len() == 1 => *only,
            _ => {
                return Err(config_error(
                    "server_key is required when more than one account is served",
                ));
            }
        };
        let mut grants = BTreeMap::new();
        for client in &cfg.clients {
            if accounts.contains_key(&client.client) || client.client == server_key {
                return Err(config_error(format!(
                    "client key {} must differ from every served account and the server key",
                    client.client
                )));
            }
            if let Some(account) = client
                .accounts
                .iter()
                .find(|account| !accounts.contains_key(account))
            {
                return Err(config_error(format!(
                    "client {} is granted account {account}, which is not served",
                    client.client
                )));
            }
            if grants
                .insert(client.client, client.accounts.clone())
                .is_some()
            {
                return Err(config_error(format!(
                    "client {} is configured twice",
                    client.client
                )));
            }
        }
        if !cfg.verify.require_receiver {
            return Err(config_error(
                "the remote signer requires receiver-bound btauth/1 requests",
            ));
        }
        if cfg.verify.wrapped != WrappedPolicy::Strict {
            return Err(config_error(
                "the remote signer verifies exact btauth/1 payload bytes; use WrappedPolicy::Strict",
            ));
        }
        cfg.policy.validate().map_err(config_error)?;
        if cfg.policy.limits.max_fee.is_some() && !source.is_online() {
            return Err(config_error(
                "max_fee needs an online runtime source to estimate fees",
            ));
        }
        if let Some(observed) = source.genesis()
            && observed != cfg.genesis_hash
        {
            return Err(config_error(
                "the runtime source is connected to a different chain than genesis_hash",
            ));
        }
        if cfg.body_limit == 0 {
            return Err(config_error("body_limit must be positive"));
        }
        let verifier = Verifier::new(server_key, cfg.verify.clone(), store)
            .map_err(|error| config_error(error.to_string()))?;
        let allowlist = ClientAllowlist::new(grants);
        let mut layer = BtAuthLayer::new(Arc::new(verifier))
            .with_policy(Arc::new(allowlist.clone()))
            .with_body_limit(cfg.body_limit);
        if let Some(limiter) = cfg.limiter {
            layer = layer.with_limiter(Arc::new(
                PerHotkeyLimiter::new(limiter).map_err(|error| config_error(error.to_string()))?,
            ));
        }
        Ok(Self {
            inner: Arc::new(Inner {
                server_key,
                accounts,
                allowlist,
                genesis_hash: cfg.genesis_hash,
                chain_info: cfg.chain_info,
                policy: cfg.policy,
                source,
            }),
            layer,
            body_limit: cfg.body_limit,
        })
    }

    /// Replace the verification clock (tests).
    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.layer = self.layer.with_clock(clock);
        self
    }

    /// The btauth receiver identity.
    pub fn server_key(&self) -> AccountId {
        self.inner.server_key
    }

    /// Served accounts and their algorithms.
    pub fn accounts(&self) -> Vec<(AccountId, CryptoType)> {
        self.inner
            .accounts
            .iter()
            .map(|(account, signer)| (*account, signer.crypto_type()))
            .collect()
    }

    /// `POST /sign` and `GET /accounts`, with btauth/1 outermost.
    pub fn router(&self) -> Router {
        Router::new()
            .route("/sign", post(handler::sign))
            .route("/accounts", get(handler::accounts))
            .with_state(self.inner.clone())
            .layer(DefaultBodyLimit::max(self.body_limit))
            .layer(self.layer.clone())
    }

    /// Serve plain HTTP on a listener until the process stops.
    ///
    /// This is for tests and trusted loopback use: it applies no connection
    /// cap or timeouts. Plain HTTP must be limited to loopback; the
    /// `rtao-remote-signerd` daemon (`server-bin`) adds TLS and connection
    /// limits.
    pub async fn serve(self, listener: tokio::net::TcpListener) -> std::io::Result<()> {
        axum::serve(listener, self.router()).await
    }
}
