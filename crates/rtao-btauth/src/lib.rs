// Derived from RaoFoundation/subtensor sdk/python/bittensor/http_auth.py @ b6a81d144078eac18bd25c3af7cb31579bb670f3 (Apache-2.0); modified by rtao.
//! Signed HTTP authentication (btauth/1).
//!
//! The core uses no runtime or network I/O. Hash raw received body bytes and
//! pass the percent-encoded request target, including its query string.

#![doc = include_str!("../README.md")]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![deny(missing_docs)]

/// Signed HTTP requests and bounded concurrent axon queries.
#[cfg(feature = "client")]
#[cfg_attr(docsrs, doc(cfg(feature = "client")))]
pub mod client;
/// Temporary signature bridge for schema-specific v10 requests.
#[cfg(feature = "legacy-v10")]
#[cfg_attr(docsrs, doc(cfg(feature = "legacy-v10")))]
pub mod legacy;
#[cfg(feature = "redis")]
mod redis;
/// Tower authentication, caller admission and axum request extractors.
#[cfg(feature = "server")]
#[cfg_attr(docsrs, doc(cfg(feature = "server")))]
pub mod server;
#[cfg(feature = "redis")]
#[cfg_attr(docsrs, doc(cfg(feature = "redis")))]
pub use redis::RedisNonceStore;
mod nonce;
pub use nonce::{
    BoxFuture, InMemoryNonceStore, NonceStore, NonceStoreConfig, StoreConfigError, StoreError,
};

use http::{HeaderMap, HeaderValue, Method};
use num_bigint::BigInt;
use rtao_primitives::{AccountId, CryptoType, pyfmt};
use rtao_signer::{Signer, SignerError};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fmt,
    sync::{
        Arc, RwLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use unicode_general_category::{GeneralCategory, get_general_category};

/// Name of the signed HTTP protocol.
pub const PROTOCOL: &str = "btauth/1";
/// Current header version.
pub const VERSION: &str = "1";
/// Signed receiver line for an unbound request.
pub const NO_RECEIVER: &str = "-";
/// Default maximum age of a request nonce.
pub const DEFAULT_MAX_AGE: Duration = Duration::from_secs(10);
/// Default tolerated sender clock lead.
pub const DEFAULT_ALLOWED_SKEW: Duration = Duration::from_secs(2);
/// Protocol version header.
pub const HEADER_VERSION: &str = "X-Bittensor-Version";
/// Sender SS58 header.
pub const HEADER_HOTKEY: &str = "X-Bittensor-Hotkey";
/// Receiver SS58 header.
pub const HEADER_RECEIVER: &str = "X-Bittensor-Receiver";
/// Unix nanosecond nonce header.
pub const HEADER_NONCE: &str = "X-Bittensor-Nonce";
/// Signature algorithm header (absent means sr25519).
pub const HEADER_CRYPTO: &str = "X-Bittensor-Crypto";
/// Hexadecimal signature header.
pub const HEADER_SIGNATURE: &str = "X-Bittensor-Signature";

// AccountId32 SS58 has a one/two-byte prefix, 32-byte key and two-byte checksum.
// For the longest (36-byte) form its first byte is <=127: the value is <2^287,
// while 58^49 >2^287. The shorter prefix-zero form, including leading zero
// characters, takes at most 48 bytes. Hex public keys are not accepted by the
// pinned core's public_key_from_ss58/from_ss58check entry point.
/// Longest accepted AccountId32 SS58 sender spelling, checked before decoding.
pub const MAX_SENDER_SS58_BYTES: usize = 49;
const MAX_AUTH_FIELD_BYTES: usize = 4_096;
const MAX_AUTH_TOTAL_BYTES: usize = 8_192;
const AUTH_BUDGET_ERROR: &str =
    "authentication fields exceed the 4096-byte field or 8192-byte total budget";

/// Supplies Unix nanoseconds for fresh signing nonces.
pub trait Clock: Send + Sync {
    /// Return a nonce for this signing attempt. Retries must obtain a fresh value.
    fn now_ns(&self) -> Result<u64, ClockError>;
}

/// A wall clock cannot produce a fresh representable Unix-nanosecond nonce.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClockError {
    /// The system clock precedes the Unix epoch.
    BeforeUnixEpoch,
    /// Unix nanoseconds or the next monotonic nonce exceed `u64::MAX`.
    NonceOverflow,
}
impl fmt::Display for ClockError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::BeforeUnixEpoch => "system clock precedes the Unix epoch",
            Self::NonceOverflow => "Unix nanosecond nonce exceeds u64",
        })
    }
}
impl std::error::Error for ClockError {}

/// Wall-clock nonces, monotonically advanced across calls to the same clock.
///
/// Reuse a single clock for a client. A backward system clock adjustment is
/// reflected as monotonically increasing nonces and may cause a verifier's
/// freshness check to reject them until its clock catches up.
#[derive(Debug, Default)]
pub struct SystemClock {
    last: AtomicU64,
}

impl Clock for SystemClock {
    fn now_ns(&self) -> Result<u64, ClockError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| ClockError::BeforeUnixEpoch)?
            .as_nanos();
        let now = u64::try_from(now).map_err(|_| ClockError::NonceOverflow)?;
        let mut previous = self.last.load(Ordering::Relaxed);
        loop {
            let next = now.max(previous.checked_add(1).ok_or(ClockError::NonceOverflow)?);
            match self.last.compare_exchange_weak(
                previous,
                next,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return Ok(next),
                Err(actual) => previous = actual,
            }
        }
    }
}

/// Build exactly eight UTF-8 lines, with no trailing newline or wrapper.
#[doc(alias = "bittensor.http_auth.build_payload")]
pub fn build_payload(
    scheme: CryptoType,
    method: &Method,
    target: &str,
    body: &[u8],
    nonce_ns: u64,
    sender: &AccountId,
    receiver: Option<&AccountId>,
) -> Vec<u8> {
    build_payload_text(
        scheme_name(scheme),
        method,
        target,
        body,
        nonce_ns,
        &sender.to_string(),
        receiver.map(|receiver| receiver.to_string()).as_deref(),
    )
}

fn scheme_name(scheme: CryptoType) -> &'static str {
    match scheme {
        CryptoType::Ed25519 => "ed25519",
        CryptoType::Sr25519 => "sr25519",
    }
}

/// Python `wallets.CRYPTO_TYPE_NAMES` (v488): the header and payload name of
/// each crypto code.
pub const CRYPTO_TYPE_NAMES: [(u8, &str); 10] = [
    (0, "ed25519"),
    (1, "sr25519"),
    (4, "hashed"),
    (6, "hashed-ed25519"),
    (7, "mldsa65"),
    (8, "ms"),
    (9, "falcon512"),
    (10, "hashed-falcon512"),
    (11, "falcon1024"),
    (12, "hashed-falcon1024"),
];

fn code_name(code: u8) -> Option<&'static str> {
    CRYPTO_TYPE_NAMES
        .iter()
        .find(|(known, _)| *known == code)
        .map(|(_, name)| *name)
}

/// Python `_validate_trusted_crypto_type`: a known classical or classical
/// hashed (rotating SR/ED) scheme; post-quantum HTTP authentication is not
/// supported.
pub fn validate_trusted_crypto_type(code: u8) -> Result<(), ConfigError> {
    if code_name(code).is_none() {
        return Err(ConfigError("unsupported trusted sender crypto type".into()));
    }
    if code > 1 && code != 4 && code != 6 {
        return Err(ConfigError(
            "post-quantum HTTP authentication is not supported".into(),
        ));
    }
    Ok(())
}

/// Trusted signature schemes of senders (D-153), from configuration or a
/// registry, never from a request's crypto header.
///
/// Bare SS58 identities do not encode a scheme: Python v488 refuses a sender
/// without a trusted one. `scheme` returns the sender's crypto code for the
/// declared one, or `None` to refuse.
pub trait SenderSchemes: Send + Sync {
    /// The trusted crypto code of `sender` (its exact SS58 text), given the
    /// scheme the request declares.
    fn scheme(&self, sender: &str, declared: u8) -> Option<u8>;
}

/// Python `configure_verifier`'s mapping: exact sender SS58 strings to their
/// trusted crypto codes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrustedSchemes(HashMap<String, u8>);

impl TrustedSchemes {
    /// Validate and copy a mapping (Python's checks: nonempty senders,
    /// supported classical or hashed schemes).
    pub fn new(schemes: impl IntoIterator<Item = (String, u8)>) -> Result<Self, ConfigError> {
        let mut map = HashMap::new();
        for (sender, code) in schemes {
            if sender.is_empty() {
                return Err(ConfigError(
                    "sender hotkey must be a nonempty SS58 string".into(),
                ));
            }
            validate_trusted_crypto_type(code)?;
            map.insert(sender, code);
        }
        Ok(Self(map))
    }
}

impl SenderSchemes for TrustedSchemes {
    fn scheme(&self, sender: &str, _declared: u8) -> Option<u8> {
        self.0.get(sender).copied()
    }
}

/// An explicit classical-only policy: a declared Sr25519 or Ed25519 scheme
/// is trusted for any sender, and a hashed or post-quantum declaration is
/// refused (it needs a trusted scheme). This is rtao's pre-v488 behaviour,
/// for servers whose senders are classical keys; a classical signature can
/// verify only against a key's own public account.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DeclaredClassical;

impl SenderSchemes for DeclaredClassical {
    fn scheme(&self, _sender: &str, declared: u8) -> Option<u8> {
        (declared <= 1).then_some(declared)
    }
}

fn base_crypto(code: u8) -> CryptoType {
    if code == 0 || code == 6 {
        CryptoType::Ed25519
    } else {
        CryptoType::Sr25519
    }
}

fn build_payload_text<N: fmt::Display>(
    scheme: &str,
    method: &Method,
    target: &str,
    body: &[u8],
    nonce_ns: N,
    sender: &str,
    receiver: Option<&str>,
) -> Vec<u8> {
    format!(
        "{PROTOCOL}\n{}\n{}\n{target}\n{}\n{nonce_ns}\n{sender}\n{}",
        scheme,
        method.as_str().to_uppercase(),
        hex::encode(Sha256::digest(body)),
        receiver
            .filter(|receiver| !receiver.is_empty())
            .unwrap_or(NO_RECEIVER)
    )
    .into_bytes()
}

/// Sign exact payload bytes using the signer's raw-message capability.
///
/// Obtain a fresh nonce for every attempt, including retries of an identical
/// body. Sr25519 omits the crypto header; ed25519 explicitly declares it.
#[doc(alias = "bittensor.http_auth.sign")]
pub async fn sign(
    signer: &dyn Signer,
    method: &Method,
    target: &str,
    body: &[u8],
    receiver: Option<&AccountId>,
    now: &dyn Clock,
) -> Result<HeaderMap, AuthError> {
    if signer.authority().is_some() {
        return Err(AuthError::Signing(SignerError::InvalidSignature(
            "hashed HTTP signing requires a prepared signer (HashedHttpSigner); post-quantum HTTP signing is not supported".into(),
        )));
    }
    let nonce = now.now_ns().map_err(AuthError::Clock)?;
    let sender = signer.account();
    let crypto = signer.crypto_type();
    let payload = build_payload(crypto, method, target, body, nonce, &sender, receiver);
    let signature = signer
        .sign_raw(&payload)
        .await
        .map_err(AuthError::Signing)?;
    let mut headers = HeaderMap::new();
    for (name, value) in [
        (HEADER_VERSION, VERSION.to_string()),
        (HEADER_HOTKEY, sender.to_string()),
        (HEADER_NONCE, nonce.to_string()),
        (HEADER_SIGNATURE, format!("0x{}", hex::encode(signature))),
    ] {
        insert_header(&mut headers, name, value)?;
    }
    if crypto != CryptoType::Sr25519 {
        insert_header(&mut headers, HEADER_CRYPTO, scheme_name(crypto).into())?;
    }
    if let Some(receiver) = receiver {
        insert_header(&mut headers, HEADER_RECEIVER, receiver.to_string())?;
    }
    Ok(headers)
}

/// Python `prepare_signer`'s result for a hashed (rotating) SR/ED hotkey:
/// the signer at a generation the caller read from finalized chain state,
/// at least one (hashed signatures reveal the original identity key, so
/// generation zero must already be retired by a finalized transaction).
///
/// `rtao::btauth_prepare_signer` reads the generation from a client.
pub struct HashedHttpSigner {
    signer: Arc<dyn Signer>,
    generation: u64,
}

impl HashedHttpSigner {
    /// Bind a hashed SR/ED signer (a [`Signer`] whose `authority()` is set)
    /// to its finalized generation. Refuses a classical or post-quantum
    /// signer and generation zero.
    pub fn new(signer: Arc<dyn Signer>, generation: u64) -> Result<Self, SignerError> {
        let Some(authority) = signer.authority() else {
            return Err(SignerError::InvalidSignature(
                "a classical signer signs btauth requests directly".into(),
            ));
        };
        if authority.authority_type().is_post_quantum() {
            return Err(SignerError::InvalidSignature(
                "post-quantum HTTP signing is not supported".into(),
            ));
        }
        if generation < 1 {
            return Err(SignerError::InvalidSignature(
                "hashed HTTP signing requires generation zero to be retired by a finalized on-chain transaction first".into(),
            ));
        }
        Ok(Self { signer, generation })
    }
    /// The signing account.
    pub fn account(&self) -> AccountId {
        self.signer.account()
    }
    /// The finalized generation it signs at.
    pub fn generation(&self) -> u64 {
        self.generation
    }
    fn code(&self) -> u8 {
        self.signer
            .authority()
            .map_or(0, |authority| authority.authority_type().code())
    }
}

/// [`sign`] for a prepared hashed SR/ED hotkey: the crypto header names its
/// scheme (`hashed` or `hashed-ed25519`) and the signature is the core's
/// hashed message envelope.
#[doc(alias = "bittensor.http_auth.sign")]
pub async fn sign_hashed(
    signer: &HashedHttpSigner,
    method: &Method,
    target: &str,
    body: &[u8],
    receiver: Option<&AccountId>,
    now: &dyn Clock,
) -> Result<HeaderMap, AuthError> {
    let nonce = now.now_ns().map_err(AuthError::Clock)?;
    let sender = signer.account();
    let code = signer.code();
    let scheme = code_name(code).ok_or_else(|| {
        AuthError::Signing(SignerError::InvalidSignature(format!(
            "cannot sign btauth requests with crypto type {code}"
        )))
    })?;
    let payload = build_payload_text(
        scheme,
        method,
        target,
        body,
        nonce,
        &sender.to_string(),
        receiver.map(|receiver| receiver.to_string()).as_deref(),
    );
    let authority = signer.signer.authority().ok_or_else(|| {
        AuthError::Signing(SignerError::InvalidSignature(
            "hashed HTTP signer lost its authority".into(),
        ))
    })?;
    let signature = authority
        .sign_message(signer.generation, &payload)
        .await
        .map_err(AuthError::Signing)?;
    let mut headers = HeaderMap::new();
    for (name, value) in [
        (HEADER_VERSION, VERSION.to_string()),
        (HEADER_HOTKEY, sender.to_string()),
        (HEADER_NONCE, nonce.to_string()),
        (HEADER_SIGNATURE, format!("0x{}", hex::encode(signature))),
        (HEADER_CRYPTO, scheme.to_owned()),
    ] {
        insert_header(&mut headers, name, value)?;
    }
    if let Some(receiver) = receiver {
        insert_header(&mut headers, HEADER_RECEIVER, receiver.to_string())?;
    }
    Ok(headers)
}

fn insert_header(
    headers: &mut HeaderMap,
    name: &'static str,
    value: String,
) -> Result<(), AuthError> {
    let name = http::header::HeaderName::from_bytes(name.as_bytes())
        .map_err(|_| AuthError::MalformedAuth("invalid authentication header name".into()))?;
    let value = HeaderValue::from_str(&value)
        .map_err(|_| AuthError::MalformedAuth("invalid authentication header value".into()))?;
    headers.insert(name, value);
    Ok(())
}

/// Whether to accept Polkadot-JS `<Bytes>`-wrapped message signatures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WrappedPolicy {
    /// Upstream verification, accepting raw and wrapped forms (Python parity).
    #[default]
    AcceptLikePython,
    /// Verify only the exact raw payload bytes.
    Strict,
}

/// Freshness and receiver-binding policy for an HTTP verifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyConfig {
    /// Maximum age of the nonce.
    pub max_age: Duration,
    /// Maximum tolerated future nonce lead.
    pub allowed_skew: Duration,
    /// Reject requests with no receiver header.
    pub require_receiver: bool,
    /// Acceptance policy for wrapped signatures.
    pub wrapped: WrappedPolicy,
}

impl Default for VerifyConfig {
    fn default() -> Self {
        Self {
            max_age: DEFAULT_MAX_AGE,
            allowed_skew: DEFAULT_ALLOWED_SKEW,
            require_receiver: true,
            wrapped: WrappedPolicy::default(),
        }
    }
}

/// Authentication or signing failure, with verification messages matching Python.
#[derive(Debug, Clone, PartialEq, Eq)]
#[doc(alias = "bittensor.http_auth.AuthError")]
pub enum AuthError {
    /// Missing, malformed, or unsupported authentication fields.
    #[doc(alias = "bittensor.http_auth.MalformedAuth")]
    MalformedAuth(String),
    /// The signature is unbound or addressed to a different receiver.
    #[doc(alias = "bittensor.http_auth.WrongReceiver")]
    WrongReceiver(String),
    /// The nonce lies outside the configured freshness window.
    #[doc(alias = "bittensor.http_auth.StaleRequest")]
    StaleRequest(String),
    /// A verified nonce has already been committed.
    #[doc(alias = "bittensor.http_auth.ReplayedRequest")]
    ReplayedRequest(String),
    /// A well-formed signature failed cryptographic verification.
    #[doc(alias = "bittensor.http_auth.BadSignature")]
    BadSignature(String),
    /// The replay store is full or unavailable; adapters should fail closed.
    Store(StoreError),
    /// The client signer could not produce a raw-message signature.
    Signing(SignerError),
    /// The signing clock could not produce a fresh Unix-nanosecond nonce.
    Clock(ClockError),
}

impl fmt::Display for AuthError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MalformedAuth(message)
            | Self::WrongReceiver(message)
            | Self::StaleRequest(message)
            | Self::ReplayedRequest(message)
            | Self::BadSignature(message) => formatter.write_str(message),
            Self::Store(error) => error.fmt(formatter),
            Self::Signing(error) => error.fmt(formatter),
            Self::Clock(error) => error.fmt(formatter),
        }
    }
}
impl std::error::Error for AuthError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(error) => Some(error),
            Self::Signing(error) => Some(error),
            Self::Clock(error) => Some(error),
            _ => None,
        }
    }
}

/// Construction failure when replay retention does not cover freshness.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError(String);
impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
impl std::error::Error for ConfigError {}

/// The authenticated sender and signed nonce.
#[derive(Debug, Clone, PartialEq, Eq)]
#[doc(alias = "bittensor.http_auth.Caller")]
pub struct Caller {
    /// Public sender identity decoded from its signed SS58 address.
    pub hotkey: AccountId,
    /// Exact signed Unix nanosecond nonce, preserving Python integer inputs.
    ///
    /// Locally generated nonces fit `u64`. Verification also accepts signed
    /// and larger values when they are fresh, without truncation or wrapping.
    pub nonce_ns: BigInt,
    /// The verified signature algorithm: Sr25519 or Ed25519, the base of a
    /// hashed scheme.
    pub crypto: CryptoType,
    /// The exact verified crypto code (0, 1, or 4/6 for hashed SR/ED).
    pub scheme: u8,
}

/// A signature-verified request awaiting replay commitment and policy approval.
#[derive(Debug)]
pub struct Authenticated {
    caller: Caller,
}
impl Authenticated {
    /// Inspect the verified caller before changing any replay state.
    pub fn caller(&self) -> &Caller {
        &self.caller
    }
    /// Atomically commit the verified nonce. The value is consumed once.
    pub async fn commit<S: NonceStore + ?Sized>(self, store: &S) -> Result<Caller, AuthError> {
        if !store
            .check_and_store(&self.caller.hotkey, &self.caller.nonce_ns)
            .await
            .map_err(AuthError::Store)?
        {
            return Err(AuthError::ReplayedRequest(
                "this nonce was already accepted from this hotkey".into(),
            ));
        }
        Ok(self.caller)
    }
}

/// Stateless signature checks followed by optional atomic replay commitment.
pub struct Verifier<S: NonceStore + ?Sized> {
    me: AccountId,
    cfg: VerifyConfig,
    store: Arc<S>,
    schemes: RwLock<Option<Arc<dyn SenderSchemes>>>,
}
impl<S: NonceStore + ?Sized> Verifier<S> {
    /// Construct only when store retention exceeds the inclusive freshness window.
    pub fn new(me: AccountId, cfg: VerifyConfig, store: Arc<S>) -> Result<Self, ConfigError> {
        let freshness = cfg
            .max_age
            .checked_add(cfg.allowed_skew)
            .ok_or_else(|| ConfigError("freshness window duration overflow".into()))?;
        if store.retention() <= freshness {
            return Err(ConfigError(format!(
                "nonce store retains entries for {}s but the inclusive freshness window is {}s; use retention > max_age + allowed_skew",
                pyfmt::fmt_g(store.retention().as_secs_f64()),
                pyfmt::fmt_g(freshness.as_secs_f64())
            )));
        }
        Ok(Self {
            me,
            cfg,
            store,
            schemes: RwLock::new(None),
        })
    }

    /// Use `schemes` for the trusted sender scheme of calls that give none
    /// (D-153). Without a source, every such call fails closed.
    pub fn with_sender_schemes(self, schemes: Arc<dyn SenderSchemes>) -> Self {
        *self
            .schemes
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(schemes);
        self
    }

    /// Python `configure_verifier`: atomically replace the trusted sender
    /// schemes with a validated copy of `schemes`.
    #[doc(alias = "bittensor.http_auth.configure_verifier")]
    pub fn configure_verifier(
        &self,
        schemes: impl IntoIterator<Item = (String, u8)>,
    ) -> Result<(), ConfigError> {
        let schemes: Arc<dyn SenderSchemes> = Arc::new(TrustedSchemes::new(schemes)?);
        *self
            .schemes
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(schemes);
        Ok(())
    }

    /// Python `clear_verifier`: remove the trusted sender schemes; calls
    /// without an expected scheme then fail closed.
    #[doc(alias = "bittensor.http_auth.clear_verifier")]
    pub fn clear_verifier(&self) {
        *self
            .schemes
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }

    /// Check shape, receiver, freshness, and signature without touching a store.
    ///
    /// Authentication fields are limited to 4,096 bytes each and 8,192 bytes in
    /// total, checked before parsing or payload allocation. Larger spellings
    /// fail as malformed. Sender SS58 spellings must fit the native 49-byte
    /// maximum; valid prefixes and both supported cryptographic schemes retain
    /// their upstream verification rules.
    pub fn authenticate(
        &self,
        headers: &HeaderMap,
        method: &Method,
        target: &str,
        body: &[u8],
        now_ns: u64,
    ) -> Result<Authenticated, AuthError> {
        self.authenticate_expecting(headers, method, target, body, now_ns, None)
    }

    /// [`Self::authenticate`] with an `expected_crypto_type` from trusted
    /// configuration, taking precedence over the configured sender schemes
    /// (Python `verify(..., expected_crypto_type=)`).
    pub fn authenticate_expecting(
        &self,
        headers: &HeaderMap,
        method: &Method,
        target: &str,
        body: &[u8],
        now_ns: u64,
        expected_crypto_type: Option<u8>,
    ) -> Result<Authenticated, AuthError> {
        if let Some(expected) = expected_crypto_type {
            validate_trusted_crypto_type(expected)
                .map_err(|error| AuthError::MalformedAuth(error.to_string()))?;
        }
        let verify = match self.cfg.wrapped {
            WrappedPolicy::AcceptLikePython => rtao_core::keys::verify,
            WrappedPolicy::Strict => rtao_core::keys::verify_strict,
        };
        self.authenticate_with_verifier(
            headers,
            method,
            target,
            body,
            now_ns,
            expected_crypto_type,
            verify,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn authenticate_with_verifier<E>(
        &self,
        headers: &HeaderMap,
        method: &Method,
        target: &str,
        body: &[u8],
        now_ns: u64,
        expected_crypto_type: Option<u8>,
        verify: impl FnOnce(&[u8], &[u8], &str, u8) -> Result<bool, E>,
    ) -> Result<Authenticated, AuthError> {
        check_auth_header_budget(
            headers,
            &[
                HEADER_VERSION,
                HEADER_HOTKEY,
                HEADER_NONCE,
                HEADER_CRYPTO,
                HEADER_SIGNATURE,
                HEADER_RECEIVER,
            ],
            0,
        )?;
        if text_header(headers, HEADER_VERSION)? != Some(VERSION) {
            return Err(AuthError::MalformedAuth(
                "missing or unsupported X-Bittensor-Version (expected '1')".into(),
            ));
        }
        let sender = text_header(headers, HEADER_HOTKEY)?
            .filter(|sender| !sender.is_empty())
            .ok_or_else(|| AuthError::MalformedAuth("missing X-Bittensor-Hotkey".into()))?;
        let raw_nonce = text_header(headers, HEADER_NONCE)?
            .ok_or_else(|| AuthError::MalformedAuth("missing X-Bittensor-Nonce".into()))?;
        let nonce = parse_nonce(raw_nonce).ok_or_else(|| {
            AuthError::MalformedAuth("X-Bittensor-Nonce is not a decimal integer".into())
        })?;
        let crypto = parse_crypto(text_header(headers, HEADER_CRYPTO)?.unwrap_or("sr25519"))
            .ok_or_else(|| AuthError::MalformedAuth("unsupported X-Bittensor-Crypto".into()))?;
        // Rebuild the canonical scheme name rather than echoing the header.
        let scheme = code_name(crypto)
            .ok_or_else(|| AuthError::MalformedAuth("unsupported X-Bittensor-Crypto".into()))?;
        // D-153 (Python v488): the scheme comes from trusted configuration,
        // never from the request alone; there is no implicit fallback.
        let expected = match expected_crypto_type {
            Some(expected) => Some(expected),
            None => self
                .schemes
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_ref()
                .and_then(|schemes| schemes.scheme(sender, crypto)),
        };
        let Some(expected) = expected else {
            return Err(AuthError::BadSignature(
                "no trusted signature scheme configured for sender".into(),
            ));
        };
        if crypto != expected {
            return Err(AuthError::BadSignature(
                "declared signature scheme does not match the trusted sender scheme".into(),
            ));
        }
        if self.cfg.wrapped == WrappedPolicy::Strict && crypto > 1 {
            return Err(AuthError::MalformedAuth(
                "strict verification supports only sr25519 and ed25519 signatures".into(),
            ));
        }
        let signature = text_header(headers, HEADER_SIGNATURE)?
            .ok_or_else(|| AuthError::MalformedAuth("missing X-Bittensor-Signature".into()))?;
        let signature = decode_signature(signature)
            .ok_or_else(|| AuthError::MalformedAuth("X-Bittensor-Signature is not hex".into()))?;
        let receiver = text_header(headers, HEADER_RECEIVER)?;
        match receiver {
            None if self.cfg.require_receiver => {
                return Err(AuthError::WrongReceiver(
                    "missing X-Bittensor-Receiver: this server requires receiver-bound requests"
                        .into(),
                ));
            }
            Some(receiver) if receiver != self.me.to_string() => {
                return Err(AuthError::WrongReceiver(
                    "request was signed for a different receiving hotkey".into(),
                ));
            }
            _ => {}
        }
        self.check_freshness(&nonce, now_ns)?;
        // Preserve receiver/freshness diagnostics, but never copy an oversized
        // claimed address into the payload or pass it to quadratic base58 decode.
        if sender.len() > MAX_SENDER_SS58_BYTES {
            return Err(AuthError::MalformedAuth(
                "invalid hotkey address or signature encoding".into(),
            ));
        }
        let payload = build_payload_text(scheme, method, target, body, &nonce, sender, receiver);
        let valid = verify(&payload, &signature, sender, crypto).map_err(|_| {
            AuthError::MalformedAuth("invalid hotkey address or signature encoding".into())
        })?;
        if !valid {
            return Err(AuthError::BadSignature(
                "signature does not verify against the claimed hotkey".into(),
            ));
        }
        let hotkey = sender.parse().map_err(|_| {
            AuthError::MalformedAuth("invalid hotkey address or signature encoding".into())
        })?;
        Ok(Authenticated {
            caller: Caller {
                hotkey,
                nonce_ns: nonce,
                crypto: base_crypto(crypto),
                scheme: crypto,
            },
        })
    }

    fn check_freshness(&self, nonce: &BigInt, now_ns: u64) -> Result<(), AuthError> {
        let now = BigInt::from(now_ns);
        if &now - nonce > BigInt::from(self.cfg.max_age.as_nanos()) {
            return Err(AuthError::StaleRequest(format!(
                "nonce is older than the {}s freshness window",
                pyfmt::fmt_g(self.cfg.max_age.as_secs_f64())
            )));
        }
        if nonce - &now > BigInt::from(self.cfg.allowed_skew.as_nanos()) {
            return Err(AuthError::StaleRequest(format!(
                "nonce is more than {}s in the future",
                pyfmt::fmt_g(self.cfg.allowed_skew.as_secs_f64())
            )));
        }
        Ok(())
    }

    /// Authenticate then commit, preserving the reference Python check order.
    #[doc(alias = "bittensor.http_auth.verify")]
    pub async fn verify(
        &self,
        headers: &HeaderMap,
        method: &Method,
        target: &str,
        body: &[u8],
        now_ns: u64,
    ) -> Result<Caller, AuthError> {
        self.authenticate(headers, method, target, body, now_ns)?
            .commit(self.store.as_ref())
            .await
    }
}

fn text_header<'a>(headers: &'a HeaderMap, name: &str) -> Result<Option<&'a str>, AuthError> {
    headers
        .get(name)
        .map(|value| {
            std::str::from_utf8(value.as_bytes())
                .map_err(|_| AuthError::MalformedAuth(format!("{name} is not valid text")))
        })
        .transpose()
}

/// Refuse authentication headers above the size budgets before parsing.
///
/// Each named header value, and `additional_bytes`, must fit 4,096 bytes; all
/// of them together, counting duplicate values, must fit 8,192 bytes.
pub fn check_auth_header_budget(
    headers: &HeaderMap,
    names: &[&str],
    additional_bytes: usize,
) -> Result<(), AuthError> {
    let mut total = additional_bytes;
    if total > MAX_AUTH_FIELD_BYTES {
        return Err(AuthError::MalformedAuth(AUTH_BUDGET_ERROR.into()));
    }
    for name in names {
        for value in headers.get_all(*name).iter() {
            let length = value.as_bytes().len();
            // Each previous sum is <=8,192, so checking the field before addition
            // also rules out integer overflow without walking header contents.
            if length > MAX_AUTH_FIELD_BYTES || total + length > MAX_AUTH_TOTAL_BYTES {
                return Err(AuthError::MalformedAuth(AUTH_BUDGET_ERROR.into()));
            }
            total += length;
        }
    }
    Ok(())
}

/// Python `parse_crypto_type` for the header: a v488 scheme name, a short
/// or legacy alias, or a known numeric code.
fn parse_crypto(value: &str) -> Option<u8> {
    let value = value.trim().to_ascii_lowercase();
    if let Some((code, _)) = CRYPTO_TYPE_NAMES.iter().find(|(_, name)| *name == value) {
        return Some(*code);
    }
    match value.as_str() {
        "ed" => Some(0),
        "sr" => Some(1),
        "ml-dsa-65" | "mldsa" => Some(7),
        "ml-dsa" => Some(8),
        "falcon-512" => Some(9),
        "falcon-1024" => Some(11),
        _ if !value.is_empty()
            && value
                .chars()
                .all(|character| decimal_digit(character).is_some()) =>
        {
            let code = u8::try_from(parse_nonce(&value)?).ok()?;
            code_name(code).map(|_| code)
        }
        _ => None,
    }
}

fn parse_nonce(value: &str) -> Option<BigInt> {
    let value = value.trim_matches(python_integer_whitespace);
    let negative = value.starts_with('-');
    let value = value
        .strip_prefix('+')
        .or_else(|| value.strip_prefix('-'))
        .unwrap_or(value);
    if value.is_empty() {
        return None;
    }
    let mut previous_digit = false;
    let mut decimal = String::with_capacity(value.len());
    if negative {
        decimal.push('-');
    }
    for character in value.chars() {
        if character == '_' && previous_digit {
            previous_digit = false;
            continue;
        }
        let digit = decimal_digit(character)?;
        decimal.push(char::from(b'0' + digit));
        previous_digit = true;
    }
    if !previous_digit {
        return None;
    }
    decimal.parse().ok()
}

// Python int() strips Unicode whitespace after its non-ASCII decimal/space
// transform, but its ASCII parser accepts only these six ASCII characters.
// In particular, Python str.isspace()'s U+001C..U+001F do not parse as integers.
fn python_integer_whitespace(character: char) -> bool {
    matches!(character, ' ' | '\t' | '\n' | '\r' | '\x0b' | '\x0c')
        || (!character.is_ascii() && character.is_whitespace())
}

fn decimal_digit(character: char) -> Option<u8> {
    if character.is_ascii_digit() {
        return u8::try_from(u32::from(character) - u32::from('0')).ok();
    }
    if get_general_category(character) != GeneralCategory::DecimalNumber {
        return None;
    }
    let mut start = u32::from(character);
    while start > 0 {
        let previous = char::from_u32(start - 1)?;
        if get_general_category(previous) != GeneralCategory::DecimalNumber {
            break;
        }
        start -= 1;
    }
    // Unicode Nd sets contain ten consecutive decimal values. Some styled
    // sets are adjacent (the mathematical digits form a 50-character run).
    u8::try_from((u32::from(character) - start) % 10).ok()
}

fn decode_signature(signature: &str) -> Option<Vec<u8>> {
    let signature = signature.strip_prefix("0x").unwrap_or(signature);
    let mut compact = String::with_capacity(signature.len());
    for group in signature.split(|character: char| character.is_ascii_whitespace()) {
        if group.len() % 2 != 0 {
            return None;
        }
        compact.push_str(group);
    }
    hex::decode(compact).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oversized_sender_never_reaches_the_native_signature_decoder()
    -> Result<(), Box<dyn std::error::Error>> {
        let receiver = AccountId::ZERO;
        let verifier = Verifier::new(
            receiver,
            VerifyConfig::default(),
            Arc::new(InMemoryNonceStore::new()),
        )
        .map(|verifier| {
            verifier.with_sender_schemes(std::sync::Arc::new(crate::DeclaredClassical))
        })?;
        for length in [MAX_SENDER_SS58_BYTES + 1, 100_000] {
            let mut headers = HeaderMap::new();
            for (name, value) in [
                (HEADER_VERSION, VERSION.to_string()),
                (HEADER_HOTKEY, "z".repeat(length)),
                (HEADER_NONCE, "0".to_string()),
                (HEADER_SIGNATURE, "00".repeat(64)),
                (HEADER_RECEIVER, receiver.to_string()),
            ] {
                insert_header(&mut headers, name, value)?;
            }
            let calls = std::cell::Cell::new(0);
            let result = verifier.authenticate_with_verifier(
                &headers,
                &Method::POST,
                "/",
                b"body",
                0,
                Some(1),
                |_, _, _, _| {
                    calls.set(calls.get() + 1);
                    Ok::<bool, ()>(false)
                },
            );
            assert_eq!(
                result.err(),
                Some(AuthError::MalformedAuth(
                    if length > MAX_AUTH_FIELD_BYTES {
                        AUTH_BUDGET_ERROR
                    } else {
                        "invalid hotkey address or signature encoding"
                    }
                    .into()
                ))
            );
            assert_eq!(calls.get(), 0);
        }
        Ok(())
    }

    #[test]
    fn numeric_crypto_and_signature_budgets_precede_parsing_and_preserve_bounded_spellings()
    -> Result<(), Box<dyn std::error::Error>> {
        let receiver = AccountId::ZERO;
        let verifier = Verifier::new(
            receiver,
            VerifyConfig::default(),
            Arc::new(InMemoryNonceStore::new()),
        )
        .map(|verifier| {
            verifier.with_sender_schemes(std::sync::Arc::new(crate::DeclaredClassical))
        })?;
        for field in [
            HEADER_NONCE,
            HEADER_CRYPTO,
            HEADER_SIGNATURE,
            HEADER_RECEIVER,
        ] {
            let mut headers = HeaderMap::new();
            insert_header(&mut headers, field, "9".repeat(MAX_AUTH_FIELD_BYTES + 1))?;
            let result = verifier.authenticate_with_verifier(
                &headers,
                &Method::POST,
                "/",
                b"",
                0,
                Some(1),
                |_, _, _, _| Ok::<bool, ()>(false),
            );
            assert_eq!(
                result.err(),
                Some(AuthError::MalformedAuth(AUTH_BUDGET_ERROR.into()))
            );
        }
        let mut headers = HeaderMap::new();
        for field in [HEADER_NONCE, HEADER_CRYPTO, HEADER_SIGNATURE] {
            insert_header(&mut headers, field, "0".repeat(3_000))?;
        }
        assert_eq!(
            check_auth_header_budget(
                &headers,
                &[HEADER_NONCE, HEADER_CRYPTO, HEADER_SIGNATURE],
                0,
            ),
            Err(AuthError::MalformedAuth(AUTH_BUDGET_ERROR.into()))
        );
        let mut boundary = HeaderMap::new();
        insert_header(
            &mut boundary,
            HEADER_NONCE,
            "0".repeat(MAX_AUTH_FIELD_BYTES),
        )?;
        insert_header(
            &mut boundary,
            HEADER_SIGNATURE,
            "0".repeat(MAX_AUTH_FIELD_BYTES),
        )?;
        assert!(check_auth_header_budget(&boundary, &[HEADER_NONCE, HEADER_SIGNATURE], 0).is_ok());
        boundary.append(
            http::header::HeaderName::from_bytes(HEADER_NONCE.as_bytes())?,
            HeaderValue::from_static("0"),
        );
        assert!(check_auth_header_budget(&boundary, &[HEADER_NONCE, HEADER_SIGNATURE], 0).is_err());
        assert!(check_auth_header_budget(&HeaderMap::new(), &[], MAX_AUTH_FIELD_BYTES).is_ok());
        assert!(
            check_auth_header_budget(&HeaderMap::new(), &[], MAX_AUTH_FIELD_BYTES + 1).is_err()
        );
        let zeros = "0".repeat(3_000);
        assert_eq!(parse_nonce(&format!("{zeros}1")), Some(BigInt::from(1)));
        assert_eq!(parse_crypto(&format!("{zeros}1")), Some(1));
        assert_eq!(
            decode_signature(&format!("0x{zeros}")),
            Some(vec![0; 1_500])
        );
        Ok(())
    }

    #[test]
    fn python_nonce_spellings_parse_as_exact_signed_integers() {
        for spelling in ["0", "-0", "-0_0", "+0", " 0 ", "0000"] {
            assert_eq!(parse_nonce(spelling), Some(BigInt::from(0)));
        }
        for spelling in [
            "1752076800000000000",
            "+1752076800000000000",
            " 1_752_076_800_000_000_000 ",
        ] {
            assert_eq!(
                parse_nonce(spelling),
                Some(BigInt::from(1_752_076_800_000_000_000u64))
            );
        }
        for spelling in ["", " ", "+", "--0", "0__0", "0_", "_0"] {
            assert_eq!(parse_nonce(spelling), None);
        }
        for spelling in [
            "-1",
            "18446744073709551615",
            "18446744073709551616",
            "-18446744073709551616",
        ] {
            assert_eq!(
                parse_nonce(spelling)
                    .map(|nonce| nonce.to_string())
                    .as_deref(),
                Some(spelling)
            );
        }
        let huge = "9".repeat(300);
        assert_eq!(
            parse_nonce(&huge).map(|nonce| nonce.to_string()),
            Some(huge)
        );
    }

    #[test]
    fn unicode_decimal_digits_and_whitespace_follow_python_integer_syntax() {
        for (spelling, expected) in [
            ("١٧٥٢٠٧٦٨٠٠٠٠٠٠٠٠٠٠٠", "1752076800000000000"),
            ("１２３４５６７８９０", "1234567890"),
            ("𝟎𝟏𝟐𝟑𝟒𝟓𝟔𝟕𝟖𝟗", "123456789"),
            ("𝟘𝟙𝟚𝟛𝟜𝟝𝟞𝟟𝟠𝟡", "123456789"),
            ("𝟢𝟣𝟤𝟥𝟦𝟧𝟨𝟩𝟪𝟫", "123456789"),
            ("𝟬𝟭𝟮𝟯𝟰𝟱𝟲𝟳𝟴𝟵", "123456789"),
            ("𝟶𝟷𝟸𝟹𝟺𝟻𝟼𝟽𝟾𝟿", "123456789"),
            ("\u{a0}+١_２_𝟛\u{2007}\u{202f}", "123"),
            ("-٠_０_𝟢", "0"),
            ("\u{1e4f1}", "1"),
            ("\t\n\r\x0b\x0c -١\t\n\r\x0b\x0c ", "-1"),
        ] {
            assert_eq!(
                parse_nonce(spelling)
                    .map(|nonce| nonce.to_string())
                    .as_deref(),
                Some(expected)
            );
        }
        for spelling in [
            "²",
            "Ⅳ",
            "½",
            "①",
            "１＿２",
            "١__２",
            "_١",
            "١_",
            "١ ２",
            "＋١",
            "−١",
            "\x1c١",
            "١\x1f",
            "\u{10d41}",
            "\u{11bf1}",
            "\u{116d1}",
        ] {
            assert_eq!(parse_nonce(spelling), None, "{spelling:?}");
        }
        assert_eq!(parse_crypto("\u{1e4f1}"), Some(1));
        assert_eq!(parse_crypto("\u{10d41}"), None);
    }

    #[test]
    fn every_unicode_decimal_group_cycles_zero_through_nine() {
        let mut in_group = false;
        let mut group_len = 0;
        let mut digit_count = 0;
        for codepoint in 0..=0x10ffff {
            let Some(character) = char::from_u32(codepoint) else {
                continue;
            };
            if get_general_category(character) == GeneralCategory::DecimalNumber {
                if !in_group {
                    group_len = 0;
                }
                assert_eq!(
                    decimal_digit(character),
                    Some(group_len % 10),
                    "U+{codepoint:04X}"
                );
                in_group = true;
                group_len += 1;
                digit_count += 1;
            } else {
                if in_group {
                    assert_eq!(group_len % 10, 0);
                }
                assert_eq!(decimal_digit(character), None);
                in_group = false;
            }
        }
        assert!(!in_group);
        // The independent CPython 3.12.11 table has exactly 680 decimal scalars.
        assert_eq!(digit_count, 680);
    }

    #[test]
    fn exhausted_system_clock_refuses_to_reuse_the_last_nonce() {
        let clock = SystemClock {
            last: AtomicU64::new(u64::MAX),
        };
        assert_eq!(clock.now_ns(), Err(ClockError::NonceOverflow));
    }
}
