// Derived from BitMind-AI/bitmind-subnet gas/protocol/epistula.py @ 5fd96727b8f41d8376348f6dd8a11cd5069bce45 (MIT); modified by rtao.
// Derived from subtrahend-labs/gobt boilerplate/epistula.go @ 9c5fa0cc4b1fdd53807233c89f9dc0ac66ca33cb (MIT); modified by rtao.
//! Epistula v2 signed HTTP headers with upstream dialect presets.
//!
//! Epistula signs `sha256(body).hexdigest() "." uuid "." timestamp_ms "."
//! receiver` with the sender's sr25519 hotkey and carries the result in
//! `Epistula-*` headers. Upstream implementations disagree on details, so a
//! [`Dialect`] selects the exact message, receiver, timestamp and secret
//! signature rules. Every function in this module is pure; signing awaits a
//! [`rtao_signer::Signer`], and verification never performs I/O until a
//! verified request is committed to a replay store.

#![doc = include_str!("../README.md")]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![deny(missing_docs)]

/// Python `json.dumps(sort_keys=True)` bodies for the SN1 Apex dialect.
pub mod apex;
/// Reqwest signing helpers.
#[cfg(feature = "client")]
#[cfg_attr(docsrs, doc(cfg(feature = "client")))]
pub mod client;
/// Optional pre-body secret-signature filter.
#[cfg(feature = "secret-filter")]
#[cfg_attr(docsrs, doc(cfg(feature = "secret-filter")))]
pub mod secret;
/// Tower authentication, admission and axum request extractors.
#[cfg(feature = "server")]
#[cfg_attr(docsrs, doc(cfg(feature = "server")))]
pub mod server;
mod sign;
mod verify;

pub use sign::{EpistulaHeaders, SignOptions, sign_headers, uuid4};
pub use verify::{Authenticated, ConfigError, EpistulaCaller, ReplayPolicy, Verifier};

#[cfg(feature = "redis")]
#[cfg_attr(docsrs, doc(cfg(feature = "redis")))]
pub use rtao_btauth::RedisNonceStore;
pub use rtao_btauth::{
    Clock, ClockError, InMemoryNonceStore, NonceStore, NonceStoreConfig, StoreError, SystemClock,
    WrappedPolicy,
};

use std::{borrow::Cow, fmt};

use rtao_signer::SignerError;
use sha2::{Digest, Sha256};

/// The only Epistula version implemented by this crate.
pub const VERSION: &str = "2";

/// Length of one secret-signature interval, in milliseconds.
pub const INTERVAL_MS: u64 = 10_000;

/// Header names, in the capitalization used by every upstream emitter.
pub mod header {
    /// Protocol version (`"2"`).
    pub const VERSION: &str = "Epistula-Version";
    /// Unix milliseconds, as decimal text.
    pub const TIMESTAMP: &str = "Epistula-Timestamp";
    /// Per-request uuid4 (absent in the Apex dialect).
    pub const UUID: &str = "Epistula-Uuid";
    /// Sender SS58 address.
    pub const SIGNED_BY: &str = "Epistula-Signed-By";
    /// Receiver SS58 address.
    pub const SIGNED_FOR: &str = "Epistula-Signed-For";
    /// `0x`-prefixed request signature.
    pub const REQUEST_SIGNATURE: &str = "Epistula-Request-Signature";
    /// Secret interval signatures for offsets -1, 0 and +1.
    pub const SECRET_SIGNATURE: [&str; 3] = [
        "Epistula-Secret-Signature-0",
        "Epistula-Secret-Signature-1",
        "Epistula-Secret-Signature-2",
    ];
    /// Unsigned Apex client specification version.
    pub const APEX_SPEC_VERSION: &str = "X-Spec-Version";
    /// Unsigned Apex request identifier.
    pub const APEX_REQUEST_ID: &str = "X-Request-Id";

    /// Authentication headers that are budget- and duplicate-checked.
    pub const AUTH: [&str; 6] = [
        VERSION,
        TIMESTAMP,
        UUID,
        SIGNED_BY,
        SIGNED_FOR,
        REQUEST_SIGNATURE,
    ];
}

/// Fields joined into the signed request message.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MessageForm {
    /// `H "." uuid "." ts "." receiver` (spec, BitMind, gobt).
    WithUuid,
    /// `H "." ts "." receiver` with no uuid (SN1 Apex).
    NoUuid,
}

/// Emission and verification of `Epistula-Version`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VersionRule {
    /// Emit `"2"` and require exactly `"2"` (BitMind).
    EmitRequire,
    /// Emit `"2"` and ignore the received header.
    EmitIgnore,
    /// Neither emit nor check the header (Apex).
    Omit,
}

/// Receiver used in the verified message.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ReceiverRule {
    /// `Epistula-Signed-For` must equal this server's SS58-42 string (BitMind).
    HeaderMustEqualSelf,
    /// Use this server's SS58-42 address and ignore the header (gobt).
    SelfAddress,
    /// Use the header, which must be present, without comparing it to self.
    ///
    /// The reference Python verifier does this. A request signed for another
    /// receiver verifies, so a relay can replay it here. Use only for tests.
    HeaderUncheckedInsecure,
    /// Always verify an unbound message (Apex).
    AlwaysEmpty,
}

/// Grammar of the received `Epistula-Timestamp`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TimestampParse {
    /// `0` or a decimal with no leading zero, at most `u64::MAX`; text is signed verbatim.
    Canonical,
    /// Python `int(text)`; the message uses `str(int(text))` (BitMind, spec Python).
    PythonInt,
    /// Go `strconv.ParseInt(text, 10, 64)`; the message uses the raw text (gobt).
    GoInt64,
    /// Python `int(float(text))`; the message uses the raw text (Apex).
    PythonFloat,
}

/// When secret interval signatures are emitted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SecretEmit {
    /// Never emit them.
    Never,
    /// Emit them only for a bound request (Python and JS references).
    WhenBound,
    /// Always emit them (gobt).
    Always,
}

/// Text of the interval number in a secret-signature message.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IntervalFormat {
    /// Python float milliseconds, `I ± 1` ms with a `.0` suffix.
    PyFloatMs,
    /// Integer milliseconds, `I ± 1` ms (spec JavaScript).
    IntMs,
    /// Ten-second interval index `ceil(ts / 10000) ± 1` (Go).
    Index,
}

/// Address appended to a secret-signature message.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SecretSubject {
    /// The receiver's SS58 address (spec).
    Receiver,
    /// The sender's SS58 address (gobt).
    Sender,
}

/// Secret interval signature emission rules.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SecretPolicy {
    /// When to emit the three headers.
    pub emit: SecretEmit,
    /// Interval text format.
    pub interval: IntervalFormat,
    /// Address after the interval text.
    pub subject: SecretSubject,
}

impl SecretPolicy {
    /// The Python reference form: bound only, float ms, receiver.
    pub const PYTHON: Self = Self {
        emit: SecretEmit::WhenBound,
        interval: IntervalFormat::PyFloatMs,
        subject: SecretSubject::Receiver,
    };
}

/// Unsigned extra headers sent by Apex clients.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ApexExtras {
    /// `X-Spec-Version` value.
    pub spec_version: Cow<'static, str>,
}

/// Wording of freshness diagnostics.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FreshnessDetail {
    /// BitMind's `Request is too stale: 20.0s old (limit: 15.0s)` form.
    BitMind,
    /// The spec and Apex fixed `Request is too stale` form.
    Plain,
}

/// The complete configuration of one Epistula dialect.
///
/// Start from a preset and adjust public fields as needed.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dialect {
    /// Signed message form.
    pub message: MessageForm,
    /// Version header rule.
    pub version: VersionRule,
    /// Verified receiver rule.
    pub receiver: ReceiverRule,
    /// Refuse to sign requests without `Signed-For`.
    pub require_bound_signing: bool,
    /// Received timestamp grammar.
    pub timestamp_parse: TimestampParse,
    /// Stale iff `ts + max_age_ms < now_ms`; `None` is unbounded.
    pub max_age_ms: Option<u64>,
    /// Future iff `ts - max_future_ms > now_ms`; `None` is unbounded.
    pub max_future_ms: Option<u64>,
    /// Secret signature emission.
    pub secret: SecretPolicy,
    /// Apex extras, emitted only by the Apex dialect.
    pub apex_extras: Option<ApexExtras>,
    /// Accept Polkadot-JS `<Bytes>`-wrapped signatures as Python does.
    pub wrapped: WrappedPolicy,
    /// Freshness diagnostic wording.
    pub freshness_detail: FreshnessDetail,
}

impl Dialect {
    /// rtao default: BitMind wire format, canonical timestamps, 2 s future bound.
    pub const RTAO: Self = Self {
        message: MessageForm::WithUuid,
        version: VersionRule::EmitRequire,
        receiver: ReceiverRule::HeaderMustEqualSelf,
        require_bound_signing: true,
        timestamp_parse: TimestampParse::Canonical,
        max_age_ms: Some(15_000),
        max_future_ms: Some(2_000),
        secret: SecretPolicy::PYTHON,
        apex_extras: None,
        wrapped: WrappedPolicy::AcceptLikePython,
        freshness_detail: FreshnessDetail::BitMind,
    };

    /// The epistula.sybil.com reference. Its verifier is relay-unsafe; use it to sign.
    pub const SPEC: Self = Self {
        message: MessageForm::WithUuid,
        version: VersionRule::EmitIgnore,
        receiver: ReceiverRule::HeaderUncheckedInsecure,
        require_bound_signing: false,
        timestamp_parse: TimestampParse::PythonInt,
        max_age_ms: Some(8_000),
        max_future_ms: None,
        secret: SecretPolicy::PYTHON,
        apex_extras: None,
        wrapped: WrappedPolicy::AcceptLikePython,
        freshness_detail: FreshnessDetail::Plain,
    };

    /// SN34 BitMind `gas/protocol/epistula.py`.
    pub const BITMIND: Self = Self {
        message: MessageForm::WithUuid,
        version: VersionRule::EmitRequire,
        receiver: ReceiverRule::HeaderMustEqualSelf,
        require_bound_signing: false,
        timestamp_parse: TimestampParse::PythonInt,
        max_age_ms: Some(15_000),
        max_future_ms: Some(15_000),
        secret: SecretPolicy::PYTHON,
        apex_extras: None,
        wrapped: WrappedPolicy::AcceptLikePython,
        freshness_detail: FreshnessDetail::BitMind,
    };

    /// The gobt boilerplate used by SN4 Targon, with rtao freshness bounds.
    pub const GOBT: Self = Self {
        message: MessageForm::WithUuid,
        version: VersionRule::EmitIgnore,
        receiver: ReceiverRule::SelfAddress,
        require_bound_signing: true,
        timestamp_parse: TimestampParse::GoInt64,
        max_age_ms: Some(15_000),
        max_future_ms: Some(2_000),
        secret: SecretPolicy {
            emit: SecretEmit::Always,
            interval: IntervalFormat::Index,
            subject: SecretSubject::Sender,
        },
        apex_extras: None,
        wrapped: WrappedPolicy::AcceptLikePython,
        freshness_detail: FreshnessDetail::BitMind,
    };

    /// SN1 Apex clients (no uuid; unsigned `X-Request-Id`). Client-side only.
    ///
    /// The orchestrator verifier is closed-source. This preset verifies the
    /// published `verify_signature_v2` message, which is always unbound.
    pub const APEX: Self = Self {
        message: MessageForm::NoUuid,
        version: VersionRule::Omit,
        receiver: ReceiverRule::AlwaysEmpty,
        require_bound_signing: false,
        timestamp_parse: TimestampParse::PythonFloat,
        max_age_ms: Some(15_000),
        max_future_ms: None,
        secret: SecretPolicy::PYTHON,
        apex_extras: Some(ApexExtras {
            spec_version: Cow::Borrowed("100000"),
        }),
        wrapped: WrappedPolicy::AcceptLikePython,
        freshness_detail: FreshnessDetail::Plain,
    };
}

impl Default for Dialect {
    fn default() -> Self {
        Self::RTAO
    }
}

/// The body digest input, or its precomputed sha256.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BodyDigest<'a> {
    /// Exact bytes to hash, usually the raw HTTP body.
    Raw(&'a [u8]),
    /// `sha256` of those bytes.
    Sha256([u8; 32]),
}

impl BodyDigest<'_> {
    /// Lowercase hex of the body's sha256.
    pub fn hex(&self) -> String {
        match self {
            Self::Raw(bytes) => hex::encode(Sha256::digest(bytes)),
            Self::Sha256(digest) => hex::encode(digest),
        }
    }
}

/// Build the exact request signature message.
///
/// `uuid` is ignored by [`MessageForm::NoUuid`]; `None` is the empty string
/// for [`MessageForm::WithUuid`]. `receiver` is empty for an unbound request.
pub fn request_message(
    form: MessageForm,
    body_sha256_hex: &str,
    uuid: Option<&str>,
    ts: &str,
    receiver: &str,
) -> String {
    match form {
        MessageForm::WithUuid => format!(
            "{body_sha256_hex}.{}.{ts}.{receiver}",
            uuid.unwrap_or_default()
        ),
        MessageForm::NoUuid => format!("{body_sha256_hex}.{ts}.{receiver}"),
    }
}

/// Return `(I_ms, I_idx)` with `I_idx = ceil(ts / 10000)` and `I_ms = I_idx * 10000`.
pub fn interval(ts_ms: u64) -> (u128, u64) {
    let index = ts_ms.div_ceil(INTERVAL_MS);
    (u128::from(index) * u128::from(INTERVAL_MS), index)
}

/// Build the secret-signature message for header index `k` (offset `k - 1`).
pub fn secret_message(
    format: IntervalFormat,
    ts_ms: u64,
    k: u8,
    subject_ss58: &str,
) -> Result<String, EpistulaError> {
    if k > 2 {
        return Err(EpistulaError::Usage(
            "secret signature index must be 0, 1 or 2",
        ));
    }
    let (milliseconds, index) = interval(ts_ms);
    let base = match format {
        IntervalFormat::PyFloatMs | IntervalFormat::IntMs => milliseconds,
        IntervalFormat::Index => u128::from(index),
    };
    let value = (base + u128::from(k))
        .checked_sub(1)
        .ok_or(EpistulaError::Usage(
            "secret signature interval precedes the Unix epoch",
        ))?;
    Ok(match format {
        IntervalFormat::PyFloatMs => format!("{value}.0.{subject_ss58}"),
        IntervalFormat::IntMs | IntervalFormat::Index => format!("{value}.{subject_ss58}"),
    })
}

/// Epistula signing or verification failure.
///
/// `Display` reproduces the BitMind `detail` strings verbatim.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum EpistulaError {
    /// `Epistula-Version` is missing or not `"2"`.
    UnknownVersion,
    /// `Epistula-Signed-For` does not name this receiver.
    NotForSelf,
    /// The request signature is missing or malformed.
    InvalidSignature,
    /// The timestamp is missing or malformed.
    InvalidTimestamp,
    /// The sender address is missing, oversized or not valid SS58.
    InvalidSenderKey,
    /// The receiver header is missing or duplicated.
    InvalidReceiverKey,
    /// The uuid is missing, duplicated or outside its grammar.
    InvalidUuid,
    /// Authentication headers exceed their size budget.
    HeaderBudget,
    /// The timestamp is older than the freshness window.
    Stale(String),
    /// The timestamp is further ahead than the freshness window.
    Future(String),
    /// A well-formed signature did not verify.
    SignatureMismatch,
    /// A secret interval signature did not verify.
    SecretSignatureMismatch,
    /// This request's identity was already accepted.
    Replayed,
    /// The body read or the replay commit came too long after the freshness check.
    DeadlineExceeded,
    /// The replay store is full or unavailable.
    Store(StoreError),
    /// The signer could not sign raw bytes.
    Signing(SignerError),
    /// The clock could not produce a time.
    Clock(ClockError),
    /// Epistula requires an sr25519 signer.
    UnsupportedCrypto,
    /// The API was called with arguments that would produce a non-compliant request.
    Usage(&'static str),
}

impl fmt::Display for EpistulaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::UnknownVersion => "Unknown Epistula version",
            Self::NotForSelf => "Bad Request, message is not intended for self",
            Self::InvalidSignature => "Invalid Signature",
            Self::InvalidTimestamp => "Invalid Timestamp",
            Self::InvalidSenderKey => "Invalid Sender key",
            Self::InvalidReceiverKey => "Invalid receiver key",
            Self::InvalidUuid => "Invalid uuid",
            Self::HeaderBudget => {
                "authentication fields exceed the 4096-byte field or 8192-byte total budget"
            }
            Self::Stale(message) | Self::Future(message) => message,
            Self::SignatureMismatch => "Signature Mismatch",
            Self::SecretSignatureMismatch => "Secret Signature Mismatch",
            Self::Replayed => "Request has already been processed",
            Self::DeadlineExceeded => "Request processing exceeded its deadline",
            Self::Store(error) => return error.fmt(f),
            Self::Signing(error) => return error.fmt(f),
            Self::Clock(error) => return error.fmt(f),
            Self::UnsupportedCrypto => "Epistula requires an sr25519 signer",
            Self::Usage(message) => message,
        })
    }
}

impl std::error::Error for EpistulaError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(error) => Some(error),
            Self::Signing(error) => Some(error),
            Self::Clock(error) => Some(error),
            _ => None,
        }
    }
}

/// Encode a signature as `0x` followed by lowercase hex.
pub(crate) fn signature_text(signature: &[u8; 64]) -> String {
    format!("0x{}", hex::encode(signature))
}

/// Decode exactly `0x` followed by 128 hex digits of either case.
pub fn decode_signature(text: &str) -> Option<[u8; 64]> {
    let digits = text.strip_prefix("0x")?;
    if digits.len() != 128 {
        return None;
    }
    let mut signature = [0; 64];
    hex::decode_to_slice(digits, &mut signature).ok()?;
    Some(signature)
}

/// Current Unix milliseconds from a btauth clock.
pub fn now_ms(clock: &dyn Clock) -> Result<u64, EpistulaError> {
    clock
        .now_ns()
        .map(|nanoseconds| nanoseconds / 1_000_000)
        .map_err(EpistulaError::Clock)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    #[test]
    fn interval_boundaries_match_python_ceil() {
        assert_eq!(
            interval(1_700_000_000_000),
            (1_700_000_000_000, 170_000_000)
        );
        assert_eq!(
            interval(1_700_000_000_001),
            (1_700_000_010_000, 170_000_001)
        );
        assert_eq!(
            interval(1_700_000_009_999),
            (1_700_000_010_000, 170_000_001)
        );
        assert_eq!(
            interval(1_700_000_010_000),
            (1_700_000_010_000, 170_000_001)
        );
        assert_eq!(interval(u64::MAX).1, u64::MAX.div_ceil(INTERVAL_MS));
    }

    #[test]
    fn secret_messages_reject_bad_index_and_epoch() {
        assert!(secret_message(IntervalFormat::PyFloatMs, 1, 3, "x").is_err());
        assert!(secret_message(IntervalFormat::PyFloatMs, 0, 0, "x").is_err());
        assert!(secret_message(IntervalFormat::Index, 0, 0, "x").is_err());
        assert_eq!(
            secret_message(IntervalFormat::PyFloatMs, 0, 1, "x").unwrap(),
            "0.0.x"
        );
        assert_eq!(
            secret_message(IntervalFormat::PyFloatMs, 1_700_000_000_000, 0, "x").unwrap(),
            "1699999999999.0.x"
        );
    }

    #[test]
    fn signature_grammar_is_exact() {
        let hex128 = "ab".repeat(64);
        assert!(decode_signature(&format!("0x{hex128}")).is_some());
        assert!(decode_signature(&format!("0x{}", hex128.to_uppercase())).is_some());
        assert!(decode_signature(&format!("0X{hex128}")).is_none());
        assert!(decode_signature(&hex128).is_none());
        assert!(decode_signature(&format!("0x0x{hex128}")).is_none());
        assert!(decode_signature(&format!("0x{}", &hex128[2..])).is_none());
        assert!(decode_signature(&format!("0x{hex128}ab")).is_none());
        assert!(decode_signature(&format!("0x {}", &hex128[1..])).is_none());
    }

    #[test]
    fn error_details_match_bitmind() {
        assert_eq!(
            EpistulaError::NotForSelf.to_string(),
            "Bad Request, message is not intended for self"
        );
        assert_eq!(
            EpistulaError::SignatureMismatch.to_string(),
            "Signature Mismatch"
        );
        assert_eq!(
            EpistulaError::InvalidSenderKey.to_string(),
            "Invalid Sender key"
        );
        assert_eq!(
            EpistulaError::InvalidReceiverKey.to_string(),
            "Invalid receiver key"
        );
    }
}
