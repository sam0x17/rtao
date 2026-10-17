//! TOML configuration and serving for the `rtao-remote-signerd` daemon.
//!
//! ```toml
//! [server]
//! listen = "127.0.0.1:8730"     # plain HTTP only on loopback unless TLS is set
//! server_key = "5…"             # optional when exactly one account is served
//! tls_cert = "/path/cert.pem"   # optional PEM chain
//! tls_key = "/path/key.pem"     # optional PEM private key
//! body_limit = 1048576
//! nonce_store = "memory"        # or "redis://…" with the `redis` feature
//! rate_limit_burst = 64         # optional per-client limiter
//! rate_limit_refill_ms = 1000
//! max_connections = 256         # concurrent connections; further accepts wait
//!
//! [chain]
//! endpoint = "wss://…"          # online source; or offline:
//! # metadata = "/path/metadata_v15.scale"; spec_version = 461
//! # transaction_version = 1; ss58_format = 42; spec_name = "node-subtensor"
//! genesis_hash = "0x…"
//! decimals = 9
//! token_symbol = "TAO"
//!
//! [[accounts]]
//! keyfile = "/path/coldkey"
//! password_env = "RTAO_REMOTE_SIGNER_KEY_PASSWORD"
//! # or: secret_uri_env = "RTAO_REMOTE_SIGNER_URI"; crypto = "sr25519"
//!
//! [[clients]]
//! address = "5…"
//! accounts = ["5…"]
//!
//! [policy]
//! max_spend_tao = "10"          # refused together with allow_raw_calls
//! allow_raw_calls = false
//! allowed_calls = ["Balances.transfer_keep_alive"]
//! allowed_destinations = ["5…"]
//! max_era_period = 128
//! allow_immortal = false
//! allow_shielded = false
//! ```
//!
//! Secrets never appear in the file: a keyfile password or a secret URI is
//! read from the named environment variable.
//!
//! `allowed_netuids` is accepted by the parser but refused by validation: no
//! decoded call is modeled with a subnet, so it could never apply.
//!
//! Every connection, plain or TLS, is bounded by [`ConnectionLimits`](crate::daemon::ConnectionLimits): a
//! concurrent-connection cap, a TLS handshake timeout, a request-header read
//! timeout (which also closes idle keep-alive connections), a whole-request
//! deadline that covers slow bodies, and a connection lifetime.

use std::{
    collections::BTreeSet,
    fmt,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use rtao_btauth::{InMemoryNonceStore, NonceStore, server::LimiterConfig};
use rtao_primitives::{AccountId, Balance, CryptoType, NetUid, Tao};
use rtao_signer::{KeypairSigner, Signer};
use serde::Deserialize;

use crate::{
    server::{
        AuthorizedClient, ChainInfoConfig, ClientRuntimeSource, PinnedRuntimeSource,
        RemoteSignerServer, RuntimeSource, ServedAccount, ServerConfig,
    },
    wire::{DEFAULT_BODY_LIMIT, WireCrypto, from_hex},
};

/// A daemon configuration or startup failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonError(pub String);

impl fmt::Display for DaemonError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
impl std::error::Error for DaemonError {}

fn error(message: impl Into<String>) -> DaemonError {
    DaemonError(message.into())
}

/// `[server]` section.
#[derive(Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ServerSection {
    /// Listen socket address.
    pub listen: String,
    /// btauth receiver identity.
    #[serde(default)]
    pub server_key: Option<String>,
    /// PEM certificate chain.
    #[serde(default)]
    pub tls_cert: Option<PathBuf>,
    /// PEM private key.
    #[serde(default)]
    pub tls_key: Option<PathBuf>,
    /// Largest request body.
    #[serde(default)]
    pub body_limit: Option<usize>,
    /// `memory` or a `redis://` URL.
    #[serde(default)]
    pub nonce_store: Option<String>,
    /// Per-client burst.
    #[serde(default)]
    pub rate_limit_burst: Option<u32>,
    /// Per-client token refill interval in milliseconds.
    #[serde(default)]
    pub rate_limit_refill_ms: Option<u64>,
    /// Concurrent connection cap; default [`ConnectionLimits::default`].
    #[serde(default)]
    pub max_connections: Option<usize>,
}

/// `[chain]` section.
#[derive(Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ChainSection {
    /// Trusted node WebSocket endpoint (online source).
    #[serde(default)]
    pub endpoint: Option<String>,
    /// Offline V15 metadata file: raw `MetadataVersioned`, optionally hex or
    /// wrapped in `Option<OpaqueMetadata>`.
    #[serde(default)]
    pub metadata: Option<PathBuf>,
    /// Offline runtime spec version.
    #[serde(default)]
    pub spec_version: Option<u32>,
    /// Offline transaction version.
    #[serde(default)]
    pub transaction_version: Option<u32>,
    /// Offline SS58 format.
    #[serde(default)]
    pub ss58_format: Option<u16>,
    /// Offline runtime `specName`.
    #[serde(default)]
    pub spec_name: Option<String>,
    /// The only chain the server signs for.
    pub genesis_hash: String,
    /// Token decimals for metadata digests.
    #[serde(default)]
    pub decimals: Option<u8>,
    /// Token symbol for metadata digests.
    #[serde(default)]
    pub token_symbol: Option<String>,
}

/// One `[[accounts]]` entry.
#[derive(Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AccountSection {
    /// btcli-format keyfile.
    #[serde(default)]
    pub keyfile: Option<PathBuf>,
    /// Environment variable holding the keyfile password.
    #[serde(default)]
    pub password_env: Option<String>,
    /// Environment variable holding a secret URI or mnemonic (development).
    #[serde(default)]
    pub secret_uri_env: Option<String>,
    /// Algorithm for `secret_uri_env`; default sr25519.
    #[serde(default)]
    pub crypto: Option<WireCrypto>,
}

/// One `[[clients]]` entry.
#[derive(Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ClientSection {
    /// The client's btauth SS58 address.
    pub address: String,
    /// Granted account addresses.
    pub accounts: Vec<String>,
}

/// `[policy]` section.
#[derive(Deserialize, Debug, Clone, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields)]
pub struct PolicySection {
    /// Largest TAO amount moved by one call, as a decimal string.
    #[serde(default)]
    pub max_spend_tao: Option<String>,
    /// Largest estimated fee, as a decimal TAO string (online only).
    #[serde(default)]
    pub max_fee_tao: Option<String>,
    /// Subnet allowlist.
    #[serde(default)]
    pub allowed_netuids: Option<Vec<u16>>,
    /// Allow calls without a modeled safety contract.
    #[serde(default)]
    pub allow_raw_calls: bool,
    /// `Pallet.call` allowlist.
    #[serde(default)]
    pub allowed_calls: Option<Vec<String>>,
    /// Transfer destination allowlist.
    #[serde(default)]
    pub allowed_destinations: Option<Vec<String>>,
    /// Longest mortal period.
    #[serde(default)]
    pub max_era_period: Option<u64>,
    /// Accept immortal transactions.
    #[serde(default)]
    pub allow_immortal: bool,
    /// Accept shield purposes.
    #[serde(default)]
    pub allow_shielded: bool,
    /// Require CheckMetadataHash.
    #[serde(default)]
    pub require_metadata_hash: bool,
}

/// A complete daemon configuration file.
#[derive(Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DaemonConfig {
    /// Listener and transport.
    pub server: ServerSection,
    /// Trusted runtime source.
    pub chain: ChainSection,
    /// Served accounts.
    pub accounts: Vec<AccountSection>,
    /// Authorized clients.
    #[serde(default)]
    pub clients: Vec<ClientSection>,
    /// Operator policy.
    #[serde(default)]
    pub policy: PolicySection,
}

impl DaemonConfig {
    /// Parse TOML text.
    pub fn parse(text: &str) -> Result<Self, DaemonError> {
        toml::from_str(text).map_err(|e| error(format!("invalid remote signer config: {e}")))
    }

    /// Read and parse a TOML file.
    pub fn load(path: &Path) -> Result<Self, DaemonError> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| error(format!("cannot read {}: {e}", path.display())))?;
        Self::parse(&text)
    }
}

fn account(text: &str, what: &str) -> Result<AccountId, DaemonError> {
    text.parse()
        .map_err(|_| error(format!("{what} {text:?} is not an SS58 address")))
}

fn tao(text: &str, what: &str) -> Result<Balance<Tao>, DaemonError> {
    Balance::<Tao>::parse(text).map_err(|e| error(format!("{what}: {e}")))
}

/// Unwrap V15 metadata from raw, hex or `Option<OpaqueMetadata>` framing.
pub fn metadata_bytes(raw: &[u8]) -> Result<Vec<u8>, DaemonError> {
    let bytes = match std::str::from_utf8(raw).map(str::trim) {
        Ok(text) if text.starts_with("0x") => {
            from_hex(text).map_err(|e| error(format!("metadata hex: {e}")))?
        }
        _ => raw.to_vec(),
    };
    if bytes.starts_with(b"meta") {
        return Ok(bytes);
    }
    // Option<OpaqueMetadata>: 0x01, compact length, then MetadataVersioned.
    if let Some((&1, rest)) = bytes.split_first() {
        let (length, offset) = compact_prefix(rest)?;
        let inner = rest
            .get(offset..)
            .filter(|inner| inner.len() == length && inner.starts_with(b"meta"))
            .ok_or_else(|| error("metadata framing is inconsistent"))?;
        return Ok(inner.to_vec());
    }
    Err(error("metadata is not a MetadataVersioned blob"))
}

fn compact_prefix(bytes: &[u8]) -> Result<(usize, usize), DaemonError> {
    let bad = || error("metadata framing has an invalid length");
    let first = *bytes.first().ok_or_else(bad)?;
    let (value, used) = match first & 0b11 {
        0 => (u64::from(first >> 2), 1),
        1 => (
            u64::from(
                u16::from_le_bytes(
                    bytes
                        .get(..2)
                        .ok_or_else(bad)?
                        .try_into()
                        .map_err(|_| bad())?,
                ) >> 2,
            ),
            2,
        ),
        2 => (
            u64::from(
                u32::from_le_bytes(
                    bytes
                        .get(..4)
                        .ok_or_else(bad)?
                        .try_into()
                        .map_err(|_| bad())?,
                ) >> 2,
            ),
            4,
        ),
        _ => return Err(bad()),
    };
    Ok((usize::try_from(value).map_err(|_| bad())?, used))
}

async fn load_signer(entry: &AccountSection) -> Result<Arc<dyn Signer>, DaemonError> {
    match (&entry.keyfile, &entry.secret_uri_env) {
        (Some(path), None) => {
            let password = match &entry.password_env {
                Some(name) => Some(rtao_wallet::SecretString::new(
                    std::env::var(name).map_err(|_| {
                        error(format!("password environment variable {name} is not set"))
                    })?,
                )),
                None => None,
            };
            let keypair = rtao_wallet::Keyfile::new(path)
                .get_keypair(password)
                .await
                .map_err(|e| error(format!("cannot load keyfile {}: {e}", path.display())))?;
            let signer = KeypairSigner::new(keypair).map_err(|e| error(e.to_string()))?;
            Ok(Arc::new(signer))
        }
        (None, Some(name)) => {
            if entry.password_env.is_some() {
                return Err(error("password_env applies only to keyfile accounts"));
            }
            let uri = secrecy::SecretString::new(std::env::var(name).map_err(|_| {
                error(format!("secret URI environment variable {name} is not set"))
            })?);
            let crypto = CryptoType::from(entry.crypto.unwrap_or(WireCrypto::Sr25519));
            let signer =
                KeypairSigner::from_uri(secrecy::ExposeSecret::expose_secret(&uri), crypto)
                    .or_else(|_| {
                        KeypairSigner::from_mnemonic(
                            secrecy::ExposeSecret::expose_secret(&uri),
                            crypto,
                            None,
                        )
                    })
                    .map_err(|_| {
                        error(format!(
                            "the secret in {name} is not a valid URI or mnemonic"
                        ))
                    })?;
            Ok(Arc::new(signer))
        }
        _ => Err(error(
            "each account needs exactly one of keyfile or secret_uri_env",
        )),
    }
}

/// Resource bounds for every daemon connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConnectionLimits {
    /// Connections served at once; the accept loop waits for a free slot.
    pub max_connections: usize,
    /// Bound on a TLS handshake.
    pub tls_handshake: Duration,
    /// Bound on reading one request's headers, including the idle wait
    /// before the next request on a keep-alive connection.
    pub header_read: Duration,
    /// Bound on one request from its headers to the response, covering a
    /// slowly sent body and the signing work.
    pub request: Duration,
    /// Bound on a connection's total lifetime.
    pub connection: Duration,
}

impl Default for ConnectionLimits {
    fn default() -> Self {
        Self {
            max_connections: 256,
            tls_handshake: Duration::from_secs(10),
            header_read: Duration::from_secs(10),
            request: Duration::from_secs(60),
            connection: Duration::from_secs(300),
        }
    }
}

/// A validated daemon ready to serve.
pub struct Daemon {
    /// The configured server.
    pub server: RemoteSignerServer,
    /// The configured listen address.
    pub listen: SocketAddr,
    /// Per-connection resource bounds.
    pub limits: ConnectionLimits,
    tls: Option<tokio_rustls::TlsAcceptor>,
}

impl fmt::Debug for Daemon {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Daemon")
            .field("server", &self.server)
            .field("listen", &self.listen)
            .field("limits", &self.limits)
            .field("tls", &self.tls.is_some())
            .finish()
    }
}

fn tls_acceptor(cert: &Path, key: &Path) -> Result<tokio_rustls::TlsAcceptor, DaemonError> {
    use rustls_pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject};
    let chain = CertificateDer::pem_file_iter(cert)
        .map_err(|e| error(format!("cannot read {}: {e}", cert.display())))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| error(format!("invalid certificate PEM {}: {e}", cert.display())))?;
    let key = PrivateKeyDer::from_pem_file(key)
        .map_err(|e| error(format!("invalid private key PEM {}: {e}", key.display())))?;
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|e| error(e.to_string()))?
    .with_no_client_auth()
    .with_single_cert(chain, key)
    .map_err(|e| error(format!("invalid TLS certificate or key: {e}")))?;
    let mut config = config;
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(tokio_rustls::TlsAcceptor::from(Arc::new(config)))
}

impl Daemon {
    /// Validate the configuration, load keys and connect the runtime source.
    pub async fn build(config: DaemonConfig) -> Result<Self, DaemonError> {
        let listen: SocketAddr = config
            .server
            .listen
            .parse()
            .map_err(|_| error("server.listen must be an IP socket address"))?;
        let tls = match (&config.server.tls_cert, &config.server.tls_key) {
            (Some(cert), Some(key)) => Some(tls_acceptor(cert, key)?),
            (None, None) => None,
            _ => return Err(error("tls_cert and tls_key must be set together")),
        };
        if tls.is_none() && !listen.ip().is_loopback() {
            return Err(error(
                "plain HTTP is allowed only on a loopback listen address; configure tls_cert and tls_key",
            ));
        }
        let genesis: [u8; 32] = from_hex(&config.chain.genesis_hash)
            .map_err(|e| error(format!("chain.genesis_hash: {e}")))?
            .try_into()
            .map_err(|_| error("chain.genesis_hash must be 32 bytes"))?;
        let chain = &config.chain;
        let source: Arc<dyn RuntimeSource> = match (&chain.endpoint, &chain.metadata) {
            (Some(endpoint), None) => {
                if chain.spec_version.is_some()
                    || chain.transaction_version.is_some()
                    || chain.ss58_format.is_some()
                    || chain.spec_name.is_some()
                {
                    return Err(error(
                        "spec_version, transaction_version, ss58_format and spec_name apply only to offline metadata",
                    ));
                }
                Arc::new(
                    ClientRuntimeSource::connect(endpoint)
                        .await
                        .map_err(|e| error(format!("cannot connect to {endpoint}: {e}")))?,
                )
            }
            (None, Some(path)) => {
                let raw = std::fs::read(path)
                    .map_err(|e| error(format!("cannot read {}: {e}", path.display())))?;
                let missing = |field: &str| error(format!("offline chain.{field} is required"));
                Arc::new(
                    PinnedRuntimeSource::parse(
                        &metadata_bytes(&raw)?,
                        chain.spec_version.ok_or_else(|| missing("spec_version"))?,
                        chain
                            .transaction_version
                            .ok_or_else(|| missing("transaction_version"))?,
                        chain.ss58_format.unwrap_or(42),
                        chain
                            .spec_name
                            .clone()
                            .ok_or_else(|| missing("spec_name"))?,
                    )
                    .map_err(|e| error(e.to_string()))?,
                )
            }
            _ => return Err(error("set exactly one of chain.endpoint or chain.metadata")),
        };
        let mut accounts = Vec::new();
        for entry in &config.accounts {
            accounts.push(ServedAccount::new(load_signer(entry).await?));
        }
        let mut clients = Vec::new();
        for client in &config.clients {
            clients.push(AuthorizedClient {
                client: account(&client.address, "client address")?,
                accounts: client
                    .accounts
                    .iter()
                    .map(|text| account(text, "client grant"))
                    .collect::<Result<_, _>>()?,
            });
        }
        let mut cfg = ServerConfig::new(genesis);
        cfg.server_key = config
            .server
            .server_key
            .as_deref()
            .map(|text| account(text, "server_key"))
            .transpose()?;
        cfg.accounts = accounts;
        cfg.clients = clients;
        cfg.chain_info = ChainInfoConfig {
            decimals: chain.decimals.unwrap_or(9),
            token_symbol: chain.token_symbol.clone().unwrap_or_else(|| "TAO".into()),
        };
        cfg.body_limit = config.server.body_limit.unwrap_or(DEFAULT_BODY_LIMIT);
        cfg.limiter = match (
            config.server.rate_limit_burst,
            config.server.rate_limit_refill_ms,
        ) {
            (None, None) => None,
            (burst, refill) => Some(LimiterConfig {
                burst: burst.unwrap_or(LimiterConfig::default().burst),
                refill_interval: refill
                    .map(Duration::from_millis)
                    .unwrap_or(LimiterConfig::default().refill_interval),
                ..LimiterConfig::default()
            }),
        };
        let policy = &config.policy;
        cfg.policy.limits.max_spend = policy
            .max_spend_tao
            .as_deref()
            .map(|text| tao(text, "policy.max_spend_tao"))
            .transpose()?;
        cfg.policy.limits.max_fee = policy
            .max_fee_tao
            .as_deref()
            .map(|text| tao(text, "policy.max_fee_tao"))
            .transpose()?;
        cfg.policy.limits.allowed_netuids = policy
            .allowed_netuids
            .as_ref()
            .map(|uids| uids.iter().map(|uid| NetUid(*uid)).collect());
        cfg.policy.limits.allow_raw_calls = policy.allow_raw_calls;
        cfg.policy.allowed_calls = policy
            .allowed_calls
            .as_ref()
            .map(|calls| {
                calls
                    .iter()
                    .map(|call| {
                        call.split_once('.')
                            .filter(|(pallet, name)| !pallet.is_empty() && !name.is_empty())
                            .map(|(pallet, name)| (pallet.to_owned(), name.to_owned()))
                            .ok_or_else(|| {
                                error(format!("allowed_calls entry {call:?} must be Pallet.call"))
                            })
                    })
                    .collect::<Result<BTreeSet<_>, _>>()
            })
            .transpose()?;
        cfg.policy.allowed_destinations = policy
            .allowed_destinations
            .as_ref()
            .map(|list| {
                list.iter()
                    .map(|text| account(text, "allowed_destinations entry"))
                    .collect::<Result<BTreeSet<_>, _>>()
            })
            .transpose()?;
        if let Some(period) = policy.max_era_period {
            cfg.policy.max_era_period = period;
        }
        cfg.policy.allow_immortal = policy.allow_immortal;
        cfg.policy.allow_shielded = policy.allow_shielded;
        cfg.policy.require_metadata_hash = policy.require_metadata_hash;
        let mut limits = ConnectionLimits::default();
        if let Some(max) = config.server.max_connections {
            if max == 0 {
                return Err(error("server.max_connections must be positive"));
            }
            limits.max_connections = max;
        }
        let store = nonce_store(config.server.nonce_store.as_deref()).await?;
        let server = RemoteSignerServer::new(cfg, source, store)
            .map_err(|e| error(format!("invalid remote signer config: {e}")))?;
        Ok(Self {
            server,
            listen,
            limits,
            tls,
        })
    }

    /// Bind the configured address and serve until the process stops.
    pub async fn run(self) -> Result<(), DaemonError> {
        let listener = tokio::net::TcpListener::bind(self.listen)
            .await
            .map_err(|e| error(format!("cannot listen on {}: {e}", self.listen)))?;
        self.serve(listener).await
    }

    /// Serve on an already bound listener, plain HTTP or TLS, within [`ConnectionLimits`].
    pub async fn serve(self, listener: tokio::net::TcpListener) -> Result<(), DaemonError> {
        let limits = self.limits;
        if limits.max_connections == 0 {
            return Err(error("max_connections must be positive"));
        }
        let router = self
            .server
            .router()
            .layer(axum::middleware::from_fn_with_state(
                limits.request,
                request_deadline,
            ));
        let slots = Arc::new(tokio::sync::Semaphore::new(limits.max_connections));
        loop {
            // Wait for a free slot before accepting, so excess peers queue in
            // the kernel backlog instead of consuming tasks and buffers.
            let Ok(slot) = slots.clone().acquire_owned().await else {
                return Err(error("connection limiter closed"));
            };
            let stream = match listener.accept().await {
                Ok((stream, _)) => stream,
                Err(e) => {
                    tracing::warn!(error = %e, "accept failed");
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    continue;
                }
            };
            let tls = self.tls.clone();
            let router = router.clone();
            tokio::spawn(async move {
                let _slot = slot;
                let connection = async move {
                    match tls {
                        None => serve_connection(stream, router, limits).await,
                        Some(acceptor) => {
                            if let Ok(Ok(stream)) =
                                tokio::time::timeout(limits.tls_handshake, acceptor.accept(stream))
                                    .await
                            {
                                serve_connection(stream, router, limits).await;
                            }
                        }
                    }
                };
                let _ = tokio::time::timeout(limits.connection, connection).await;
            });
        }
    }
}

async fn serve_connection<I>(io: I, router: axum::Router, limits: ConnectionLimits)
where
    I: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let service = hyper_util::service::TowerToHyperService::new(router);
    // hyper ignores header_read_timeout unless a timer is installed.
    let _ = hyper::server::conn::http1::Builder::new()
        .timer(hyper_util::rt::TokioTimer::new())
        .header_read_timeout(limits.header_read)
        .serve_connection(hyper_util::rt::TokioIo::new(io), service)
        .await;
}

/// Outermost middleware: one deadline from the request head to the response.
async fn request_deadline(
    axum::extract::State(deadline): axum::extract::State<Duration>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    match tokio::time::timeout(deadline, next.run(request)).await {
        Ok(response) => response,
        Err(_) => (
            http::StatusCode::REQUEST_TIMEOUT,
            axum::Json(crate::wire::ErrorBody {
                detail: "the request was not completed in time".into(),
                ..crate::wire::ErrorBody::default()
            }),
        )
            .into_response(),
    }
}

async fn nonce_store(spec: Option<&str>) -> Result<Arc<dyn NonceStore>, DaemonError> {
    match spec.unwrap_or("memory") {
        "memory" => Ok(Arc::new(InMemoryNonceStore::new())),
        #[cfg(feature = "redis")]
        url if url.starts_with("redis://") || url.starts_with("rediss://") => Ok(Arc::new(
            rtao_btauth::RedisNonceStore::connect(url, Duration::from_secs(60))
                .await
                .map_err(|e| error(format!("cannot connect the Redis nonce store: {e}")))?,
        )),
        other if other.starts_with("redis://") || other.starts_with("rediss://") => Err(error(
            "this build has no Redis nonce store; rebuild with the redis feature",
        )),
        _ => Err(error(
            "server.nonce_store must be \"memory\" or a redis:// URL",
        )),
    }
}
