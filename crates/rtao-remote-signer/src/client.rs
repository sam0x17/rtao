//! A [`Signer`](rtao_signer::Signer) that asks a remote signing server over btauth/1.

use std::{fmt, path::Path, sync::Arc, time::Duration};

use http::Method;
use rtao_btauth::{
    AuthError, SystemClock,
    client::{ClientError, SignedClient},
};
use rtao_primitives::{AccountId, CryptoType};
use rtao_signer::{BoxFuture, MultiSignature, Signer, SignerCaps, SignerError, SigningRequest};
pub use rustls_pki_types::CertificateDer;
use tokio::time::Instant;
use url::Url;

use crate::wire::{
    ACCOUNTS_PATH, AccountsResponse, ErrorBody, MAX_RESPONSE_BYTES, PROTOCOL, SIGN_PATH,
    SignRequest, SignResponse, WireCrypto,
};

/// Default bound on one signing or discovery exchange.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Configuration for one remote signing account.
#[derive(Clone)]
pub struct RemoteSignerConfig {
    /// Base URL; `sign` and `accounts` are joined to it.
    pub url: Url,
    /// The reviewed account to sign for.
    pub account: AccountId,
    /// Expected MultiSignature variant of `account`.
    pub crypto: CryptoType,
    /// The server's btauth receiver identity; `None` uses `account`.
    pub receiver: Option<AccountId>,
    /// The btauth/1 client identity; it must support raw signing.
    pub client_key: Arc<dyn Signer>,
    /// Bound on signing, transport and response reading.
    pub timeout: Duration,
    /// TLS trust roots for an `https` URL. When non-empty they are the only
    /// roots trusted, replacing the platform roots, so a private CA pins the
    /// signer; when empty the platform's native roots are used.
    pub trust_roots: Vec<CertificateDer<'static>>,
}

impl RemoteSignerConfig {
    /// A configuration with the default receiver, timeout and trust roots.
    pub fn new(
        url: Url,
        account: AccountId,
        crypto: CryptoType,
        client_key: Arc<dyn Signer>,
    ) -> Self {
        Self {
            url,
            account,
            crypto,
            receiver: None,
            client_key,
            timeout: DEFAULT_TIMEOUT,
            trust_roots: Vec::new(),
        }
    }
}

impl fmt::Debug for RemoteSignerConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RemoteSignerConfig")
            .field("url", &self.url.as_str())
            .field("account", &self.account)
            .field("crypto", &self.crypto)
            .field("receiver", &self.receiver)
            .field("client_key", &self.client_key.account())
            .field("timeout", &self.timeout)
            .field("trust_roots", &self.trust_roots.len())
            .finish()
    }
}

/// A remote signing failure.
#[derive(Debug)]
pub enum RemoteSignerError {
    /// The configuration is unusable; no request was sent.
    Config(String),
    /// The signed HTTP exchange failed before a response arrived.
    Transport(ClientError),
    /// The server answered with a non-success status.
    Http {
        /// HTTP status code.
        status: u16,
        /// Server diagnostic, with control characters replaced and truncated.
        detail: String,
        /// Stable handler error code, if any.
        code: Option<String>,
        /// Policy violations, in check order.
        violations: Vec<String>,
    },
    /// The response is malformed, oversized, late or does not verify.
    Protocol(String),
}

impl fmt::Display for RemoteSignerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(message) | Self::Protocol(message) => formatter.write_str(message),
            Self::Transport(error) => write!(formatter, "remote signer request failed: {error}"),
            Self::Http {
                status,
                detail,
                code,
                violations,
            } => {
                write!(
                    formatter,
                    "remote signer refused the request (HTTP {status}"
                )?;
                if let Some(code) = code {
                    write!(formatter, ", {code}")?;
                }
                write!(formatter, "): {detail}")?;
                if let Some(first) = violations.first() {
                    write!(formatter, ": {first}")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for RemoteSignerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Transport(error) => Some(error),
            _ => None,
        }
    }
}

impl From<RemoteSignerError> for SignerError {
    fn from(error: RemoteSignerError) -> Self {
        let message = error.to_string();
        match error {
            RemoteSignerError::Protocol(_) => SignerError::InvalidSignature(message),
            RemoteSignerError::Config(_) => SignerError::WithHelp {
                message,
                help: "check the remote signer URL, client key and receiver options".into(),
            },
            RemoteSignerError::Transport(_) => SignerError::WithHelp {
                message,
                help: "check that the remote signer is reachable and its TLS trust root is configured"
                    .into(),
            },
            RemoteSignerError::Http { status, .. } => SignerError::WithHelp {
                message,
                help: match status {
                    401 => "the server rejected the btauth/1 authentication; check the client key, the receiver (server key) and the local clock",
                    403 => "the server's policy or client allowlist refused this request; no transaction was signed",
                    409 => "the server signs for a different runtime; prepare the transaction again against the server's chain",
                    413 => "the request exceeds the server's body limit",
                    422 => "the server rejected the prepared transaction views; prepare the transaction again",
                    429 | 503 => "the remote signer is busy or its chain source is unavailable; retry later",
                    _ => "inspect the remote signer's logs",
                }
                .into(),
            },
        }
    }
}

/// Largest server diagnostic kept from an error response, in characters.
const MAX_DIAGNOSTIC_CHARS: usize = 512;
/// Most policy violations kept from an error response.
const MAX_VIOLATIONS: usize = 16;

/// Make a server-supplied diagnostic safe for a terminal: control
/// characters (including escape sequences) become spaces and the text is
/// truncated.
fn sanitize(text: &str) -> String {
    let mut clean: String = text
        .chars()
        .take(MAX_DIAGNOSTIC_CHARS)
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect();
    if text.chars().nth(MAX_DIAGNOSTIC_CHARS).is_some() {
        clean.push('…');
    }
    clean
}

/// Parse PEM certificates for use as the remote signer's trust roots.
pub fn load_pem_roots(path: &Path) -> Result<Vec<CertificateDer<'static>>, RemoteSignerError> {
    use rustls_pki_types::pem::PemObject;
    let roots = CertificateDer::pem_file_iter(path)
        .map_err(|error| {
            RemoteSignerError::Config(format!(
                "cannot read remote signer CA {}: {error}",
                path.display()
            ))
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| {
            RemoteSignerError::Config(format!(
                "invalid PEM in remote signer CA {}: {error}",
                path.display()
            ))
        })?;
    if roots.is_empty() {
        return Err(RemoteSignerError::Config(format!(
            "remote signer CA {} contains no certificates",
            path.display()
        )));
    }
    Ok(roots)
}

#[derive(Clone)]
struct Endpoint {
    http: SignedClient,
    sign_url: Url,
    accounts_url: Url,
}

impl Endpoint {
    fn new(
        url: &Url,
        client_key: Arc<dyn Signer>,
        trust_roots: Vec<CertificateDer<'static>>,
        timeout: Duration,
    ) -> Result<Self, RemoteSignerError> {
        if timeout.is_zero() || timeout > Duration::from_secs(3600) {
            return Err(RemoteSignerError::Config(
                "the remote signer timeout must be between 1 ms and one hour".into(),
            ));
        }
        let base = normalize_base(url)?;
        let clock = Arc::new(SystemClock::default());
        let http = if base.scheme() == "https" && trust_roots.is_empty() {
            SignedClient::with_extra_roots(client_key, clock, Vec::new())
        } else {
            // Explicit roots replace the platform roots. Plain HTTP is
            // loopback-only and never negotiates TLS, so it gets an empty
            // store and does not need native roots in minimal containers.
            let mut roots = rustls::RootCertStore::empty();
            if base.scheme() == "https" {
                for root in trust_roots {
                    roots.add(root).map_err(|error| {
                        RemoteSignerError::Config(format!(
                            "invalid remote signer trust root: {error}"
                        ))
                    })?;
                }
            }
            let tls = rustls::ClientConfig::builder_with_provider(Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .map_err(|error| RemoteSignerError::Config(error.to_string()))?
            .with_root_certificates(roots)
            .with_no_client_auth();
            SignedClient::with_tls_config(client_key, clock, tls)
        }
        .map_err(RemoteSignerError::Transport)?;
        let join = |path: &str| {
            base.join(path).map_err(|error| {
                RemoteSignerError::Config(format!("invalid remote signer URL: {error}"))
            })
        };
        Ok(Self {
            http,
            sign_url: join(SIGN_PATH)?,
            accounts_url: join(ACCOUNTS_PATH)?,
        })
    }

    async fn accounts(
        &self,
        receiver: &AccountId,
        timeout: Duration,
    ) -> Result<AccountsResponse, RemoteSignerError> {
        let deadline = Instant::now() + timeout;
        let response = self
            .http
            .request(Method::GET, self.accounts_url.clone(), receiver)
            .timeout(timeout)
            .send()
            .await
            .map_err(transport_error)?;
        let body = read_response(response, deadline).await?;
        let accounts: AccountsResponse =
            rtao_primitives::json::from_slice(&body).map_err(|error| {
                RemoteSignerError::Protocol(format!(
                    "malformed remote signer accounts response: {error}"
                ))
            })?;
        if accounts.version != PROTOCOL {
            return Err(RemoteSignerError::Protocol(format!(
                "remote signer speaks {:?}, expected {PROTOCOL:?}",
                accounts.version
            )));
        }
        if accounts.server_key.0 != *receiver {
            return Err(RemoteSignerError::Protocol(
                "remote signer reported a different server key than the authenticated receiver"
                    .into(),
            ));
        }
        Ok(accounts)
    }
}

fn normalize_base(url: &Url) -> Result<Url, RemoteSignerError> {
    if !url.username().is_empty() || url.password().is_some() {
        return Err(RemoteSignerError::Config(
            "the remote signer URL must not contain credentials".into(),
        ));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(RemoteSignerError::Config(
            "the remote signer URL must not contain a query or fragment".into(),
        ));
    }
    match url.scheme() {
        "https" => {}
        "http" => {
            let loopback = match url.host() {
                Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
                Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
                // RFC 6761 section 6.3: `localhost` names loopback. It is
                // accepted by name, with no blocking lookup in async code.
                Some(url::Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
                _ => false,
            };
            if !loopback {
                return Err(RemoteSignerError::Config(
                    "plain http is allowed only for a loopback remote signer (127.0.0.1, [::1] or localhost); use https".into(),
                ));
            }
        }
        _ => {
            return Err(RemoteSignerError::Config(
                "the remote signer URL must use https (or http on loopback)".into(),
            ));
        }
    }
    let mut base = url.clone();
    if !base.path().ends_with('/') {
        let path = format!("{}/", base.path());
        base.set_path(&path);
    }
    Ok(base)
}

fn transport_error(error: ClientError) -> RemoteSignerError {
    match error {
        ClientError::Auth(AuthError::Signing(SignerError::RawSigningUnsupported)) => {
            RemoteSignerError::Config(
                "the remote signer client key cannot sign raw btauth/1 messages; use a software keypair"
                    .into(),
            )
        }
        error => RemoteSignerError::Transport(error),
    }
}

async fn read_response(
    mut response: reqwest::Response,
    deadline: Instant,
) -> Result<Vec<u8>, RemoteSignerError> {
    let status = response.status();
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
    {
        return Err(RemoteSignerError::Protocol(format!(
            "remote signer response exceeds {MAX_RESPONSE_BYTES} bytes"
        )));
    }
    let mut body = Vec::new();
    loop {
        let chunk = tokio::time::timeout_at(deadline, response.chunk())
            .await
            .map_err(|_| RemoteSignerError::Transport(ClientError::Timeout))?
            .map_err(|error| RemoteSignerError::Transport(ClientError::Http(error)))?;
        let Some(chunk) = chunk else { break };
        if chunk.len() > MAX_RESPONSE_BYTES.saturating_sub(body.len()) {
            return Err(RemoteSignerError::Protocol(format!(
                "remote signer response exceeds {MAX_RESPONSE_BYTES} bytes"
            )));
        }
        body.extend_from_slice(&chunk);
    }
    if !status.is_success() {
        let error: ErrorBody =
            rtao_primitives::json::from_slice(&body).unwrap_or_else(|_| ErrorBody {
                detail: status
                    .canonical_reason()
                    .unwrap_or("unexpected response")
                    .to_owned(),
                ..ErrorBody::default()
            });
        return Err(RemoteSignerError::Http {
            status: status.as_u16(),
            detail: sanitize(&error.detail),
            code: error.code.as_deref().map(sanitize),
            violations: error
                .violations
                .unwrap_or_default()
                .iter()
                .take(MAX_VIOLATIONS)
                .map(|violation| sanitize(violation))
                .collect(),
        });
    }
    if status != reqwest::StatusCode::OK {
        return Err(RemoteSignerError::Protocol(format!(
            "remote signer answered with unexpected status {status}"
        )));
    }
    Ok(body)
}

/// A signer whose key lives on a policy-checking remote server.
///
/// Raw message signing is refused: the server signs only transactions it
/// has decoded and approved, so a remote key cannot act as a btauth hotkey.
#[derive(Clone)]
pub struct RemoteSigner {
    endpoint: Endpoint,
    receiver: AccountId,
    account: AccountId,
    crypto: CryptoType,
    timeout: Duration,
}

impl fmt::Debug for RemoteSigner {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RemoteSigner")
            .field("url", &self.endpoint.sign_url.as_str())
            .field("account", &self.account)
            .field("crypto", &self.crypto)
            .field("receiver", &self.receiver)
            .finish_non_exhaustive()
    }
}

impl RemoteSigner {
    /// Validate the configuration without contacting the server.
    ///
    /// Plain `http` is accepted only for loopback hosts: a loopback IP
    /// literal, or the name `localhost` (RFC 6761).
    pub fn new(cfg: RemoteSignerConfig) -> Result<Self, RemoteSignerError> {
        if cfg.client_key.account() == cfg.account {
            return Err(RemoteSignerError::Config(
                "the remote signer client key must differ from the signing account".into(),
            ));
        }
        let endpoint = Endpoint::new(&cfg.url, cfg.client_key, cfg.trust_roots, cfg.timeout)?;
        Ok(Self {
            endpoint,
            receiver: cfg.receiver.unwrap_or(cfg.account),
            account: cfg.account,
            crypto: cfg.crypto,
            timeout: cfg.timeout,
        })
    }

    /// Construct, then require the account to be listed with the same algorithm.
    ///
    /// The returned discovery response carries the server's genesis hash for an
    /// optional caller check.
    pub async fn connect(
        cfg: RemoteSignerConfig,
    ) -> Result<(Self, AccountsResponse), RemoteSignerError> {
        let signer = Self::new(cfg)?;
        let accounts = signer.accounts().await?;
        let listed = accounts
            .accounts
            .iter()
            .find(|entry| entry.account.0 == signer.account)
            .ok_or_else(|| {
                RemoteSignerError::Config(format!(
                    "the remote signer does not serve account {} for this client key",
                    signer.account
                ))
            })?;
        if listed.crypto != WireCrypto::from(signer.crypto) {
            return Err(RemoteSignerError::Config(format!(
                "the remote signer serves account {} as {:?}, not {:?}",
                signer.account, listed.crypto, signer.crypto
            )));
        }
        Ok((signer, accounts))
    }

    /// List the accounts this client key may use.
    pub async fn accounts(&self) -> Result<AccountsResponse, RemoteSignerError> {
        self.endpoint.accounts(&self.receiver, self.timeout).await
    }

    /// Account-free discovery bound to an explicit receiver (server key).
    pub async fn discover(
        url: Url,
        client_key: Arc<dyn Signer>,
        receiver: AccountId,
        timeout: Duration,
        trust_roots: Vec<CertificateDer<'static>>,
    ) -> Result<AccountsResponse, RemoteSignerError> {
        Endpoint::new(&url, client_key, trust_roots, timeout)?
            .accounts(&receiver, timeout)
            .await
    }

    /// The btauth receiver this signer addresses.
    pub fn receiver(&self) -> AccountId {
        self.receiver
    }

    /// The signing endpoint URL.
    pub fn sign_url(&self) -> &Url {
        &self.endpoint.sign_url
    }

    /// Send one signing request and verify the returned signature locally.
    ///
    /// A single attempt is made; a retry must call this again, which signs
    /// a fresh btauth/1 nonce.
    pub async fn sign_request(
        &self,
        request: &SigningRequest<'_>,
    ) -> Result<MultiSignature, RemoteSignerError> {
        let body = SignRequest::from_signing_request(request, self.account, self.crypto);
        let deadline = Instant::now() + self.timeout;
        let exchange = async {
            let response = self
                .endpoint
                .http
                .request(Method::POST, self.endpoint.sign_url.clone(), &self.receiver)
                .json(&body)
                .timeout(self.timeout)
                .send()
                .await
                .map_err(transport_error)?;
            read_response(response, deadline).await
        };
        let bytes = tokio::time::timeout_at(deadline, exchange)
            .await
            .map_err(|_| RemoteSignerError::Transport(ClientError::Timeout))??;
        let response: SignResponse =
            rtao_primitives::json::from_slice(&bytes).map_err(|error| {
                RemoteSignerError::Protocol(format!("malformed remote signer response: {error}"))
            })?;
        if response.version != PROTOCOL {
            return Err(RemoteSignerError::Protocol(format!(
                "remote signer speaks {:?}, expected {PROTOCOL:?}",
                response.version
            )));
        }
        if response.account.0 != self.account {
            return Err(RemoteSignerError::Protocol(
                "remote signer answered for a different account".into(),
            ));
        }
        let signature = response
            .signature(self.crypto)
            .map_err(|error| RemoteSignerError::Protocol(error.to_string()))?;
        let valid = rtao_core::keys::verify_strict(
            request.payload,
            &signature.bytes,
            &self.account.to_ss58(crate::wire::WIRE_SS58_PREFIX),
            u8::from(self.crypto),
        )
        .unwrap_or(false);
        if !valid {
            return Err(RemoteSignerError::Protocol(
                "remote signer returned a signature that does not verify for the prepared payload"
                    .into(),
            ));
        }
        Ok(signature)
    }
}

impl Signer for RemoteSigner {
    fn account(&self) -> AccountId {
        self.account
    }
    fn crypto_type(&self) -> CryptoType {
        self.crypto
    }
    fn caps(&self) -> SignerCaps {
        SignerCaps::RAW_PAYLOAD
    }
    fn sign<'a>(
        &'a self,
        request: &'a SigningRequest<'a>,
    ) -> BoxFuture<'a, Result<MultiSignature, SignerError>> {
        Box::pin(async move { self.sign_request(request).await.map_err(SignerError::from) })
    }
}
