// Derived from BitMind-AI/bitmind-subnet gas/protocol/epistula.py @ 5fd96727b8f41d8376348f6dd8a11cd5069bce45 (MIT); modified by rtao.
//! Stateless verification followed by optional replay commitment.

use std::{borrow::Cow, fmt, sync::Arc};

use http::HeaderMap;
use num_bigint::{BigInt, Sign};
use num_traits::{FromPrimitive, ToPrimitive};
use rtao_btauth::{Caller, Clock, MAX_SENDER_SS58_BYTES, NonceStore, WrappedPolicy};
use rtao_primitives::{AccountId, CryptoType, pyfmt};
use sha2::{Digest, Sha256};

use crate::{
    BodyDigest, Dialect, EpistulaError, FreshnessDetail, MessageForm, ReceiverRule, TimestampParse,
    VERSION, VersionRule, decode_signature, header, now_ms, request_message,
};

const MAX_UUID_BYTES: usize = 128;
const REPLAY_DOMAIN: &[u8] = b"rtao-epistula/2\0";

/// Whether accepted requests are committed to a replay store.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplayPolicy {
    /// Reject a second request with the same uuid (or, without a uuid, signature).
    Enforce,
    /// Accept replays within the freshness window, as every upstream receiver does.
    Disabled,
}

/// A verifier configuration that cannot be sound.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigError(String);

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for ConfigError {}

impl ConfigError {
    #[cfg(feature = "server")]
    pub(crate) fn new(message: String) -> Self {
        Self(message)
    }
}

/// The verified sender and signed fields of an Epistula request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EpistulaCaller {
    /// Sender public key.
    pub hotkey: AccountId,
    /// Sender SS58 text exactly as signed.
    pub signed_by: String,
    /// Parsed signed timestamp, in Unix milliseconds.
    pub timestamp_ms: i128,
    /// Signed uuid, absent for the Apex dialect.
    pub uuid: Option<String>,
    /// Receiver address used in the verified message, absent when unbound.
    pub signed_for: Option<String>,
}

/// A signature-verified request awaiting policy approval and replay commitment.
#[derive(Debug)]
pub struct Authenticated {
    caller: EpistulaCaller,
    replay_key: BigInt,
    /// The receiver time at which freshness was checked.
    checked_at_ms: u64,
    /// `max_age_ms + max_future_ms`, or `None` when either is unbounded.
    window_ms: Option<u128>,
}

impl Authenticated {
    /// Inspect the verified caller before changing any replay state.
    pub fn caller(&self) -> &EpistulaCaller {
        &self.caller
    }

    /// The replay identity stored as the nonce of [`NonceStore`].
    ///
    /// It is `sha256("rtao-epistula/2\0" kind "\0" id)` as a positive integer,
    /// where kind is `uuid` (id = uuid text) or `sig` (id = signature bytes).
    /// These values exceed 2^64 except with negligible probability, so they
    /// never collide with btauth nanosecond nonces in a shared store.
    pub fn replay_key(&self) -> &BigInt {
        &self.replay_key
    }

    /// The receiver time, in Unix milliseconds, at which freshness was checked.
    pub fn checked_at_ms(&self) -> u64 {
        self.checked_at_ms
    }

    /// Atomically commit the replay identity. The value is consumed once.
    ///
    /// Freshness was measured at [`Self::checked_at_ms`], which may be the
    /// time the request head arrived. `clock` must give the current time: the
    /// commit is refused with [`EpistulaError::DeadlineExceeded`] unless,
    /// both before and after the store insert,
    /// `elapsed + max_age_ms + max_future_ms < store.retention()`, where
    /// `elapsed` is the time since the freshness check. Every acceptance of
    /// one identity then falls within a single retention period, so a slow
    /// body or a slow policy cannot outlive the first entry and be replayed.
    /// Dialects with an unbounded age or future skew cannot commit.
    pub async fn commit<S: NonceStore + ?Sized>(
        self,
        store: &S,
        clock: &dyn Clock,
    ) -> Result<EpistulaCaller, EpistulaError> {
        let window = self.window_ms.ok_or(EpistulaError::Usage(
            "replay commitment requires bounded max_age_ms and max_future_ms",
        ))?;
        let retention = store.retention().as_millis();
        self.check_commit_time(window, retention, now_ms(clock)?)?;
        if !store
            .check_and_store(&self.caller.hotkey, &self.replay_key)
            .await
            .map_err(EpistulaError::Store)?
        {
            return Err(EpistulaError::Replayed);
        }
        // A shared store may await I/O; the insert must still fall in time.
        self.check_commit_time(window, retention, now_ms(clock)?)?;
        Ok(self.caller)
    }

    fn check_commit_time(
        &self,
        window: u128,
        retention: u128,
        now_ms: u64,
    ) -> Result<(), EpistulaError> {
        let elapsed = u128::from(now_ms.saturating_sub(self.checked_at_ms));
        if window + elapsed < retention {
            Ok(())
        } else {
            Err(EpistulaError::DeadlineExceeded)
        }
    }

    /// Accept without replay protection, as upstream receivers do.
    pub fn into_caller_without_replay(self) -> EpistulaCaller {
        self.caller
    }

    /// The caller as a btauth identity, so btauth caller policies apply unchanged.
    ///
    /// `nonce_ns` is the signed timestamp in nanoseconds.
    pub fn btauth_caller(&self) -> Caller {
        btauth_caller(&self.caller)
    }
}

pub(crate) fn btauth_caller(caller: &EpistulaCaller) -> Caller {
    Caller {
        hotkey: caller.hotkey,
        nonce_ns: BigInt::from(caller.timestamp_ms) * 1_000_000,
        crypto: CryptoType::Sr25519,
    }
}

/// Header checks and signature verification for one receiving hotkey.
pub struct Verifier<S: NonceStore + ?Sized = dyn NonceStore> {
    me: AccountId,
    me_ss58: String,
    dialect: Dialect,
    replay: ReplayPolicy,
    store: Option<Arc<S>>,
}

impl<S: NonceStore + ?Sized> fmt::Debug for Verifier<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Verifier")
            .field("me", &self.me_ss58)
            .field("dialect", &self.dialect)
            .field("replay", &self.replay)
            .finish_non_exhaustive()
    }
}

impl Verifier<dyn NonceStore> {
    /// A verifier that never commits replay identities.
    ///
    /// Required for dialects with an unbounded freshness window (`SPEC`,
    /// `APEX`), where no finite store retention can be sound.
    pub fn without_replay(me: AccountId, dialect: Dialect) -> Self {
        Self {
            me_ss58: ss58(&me),
            me,
            dialect,
            replay: ReplayPolicy::Disabled,
            store: None,
        }
    }
}

struct Parsed<'h> {
    signature: [u8; 64],
    timestamp: BigInt,
    timestamp_text: Cow<'h, str>,
    signed_by: &'h str,
    hotkey: AccountId,
    uuid: Option<&'h str>,
    receiver: Cow<'h, str>,
}

impl<S: NonceStore + ?Sized> Verifier<S> {
    /// Verify with replay enforcement through `store`.
    ///
    /// Fails unless the dialect bounds both age and future skew and the
    /// store retains entries strictly longer than their sum.
    pub fn new(me: AccountId, dialect: Dialect, store: Arc<S>) -> Result<Self, ConfigError> {
        Self::with_store(me, dialect, store, ReplayPolicy::Enforce)
    }

    /// Keep a store with an explicit replay policy.
    ///
    /// [`ReplayPolicy::Disabled`] never touches the store, which then only
    /// gives a server layer a uniform type.
    pub fn with_store(
        me: AccountId,
        dialect: Dialect,
        store: Arc<S>,
        replay: ReplayPolicy,
    ) -> Result<Self, ConfigError> {
        if replay == ReplayPolicy::Enforce {
            let (Some(max_age), Some(max_future)) = (dialect.max_age_ms, dialect.max_future_ms)
            else {
                return Err(ConfigError(
                    "replay enforcement requires bounded max_age_ms and max_future_ms; \
                     state ReplayPolicy::Disabled for an unbounded dialect"
                        .into(),
                ));
            };
            let window = u128::from(max_age) + u128::from(max_future);
            if store.retention().as_millis() <= window {
                return Err(ConfigError(format!(
                    "nonce store retains entries for {}ms but the inclusive freshness window is \
                     {window}ms; use retention > max_age_ms + max_future_ms",
                    store.retention().as_millis()
                )));
            }
        }
        Ok(Self {
            me_ss58: ss58(&me),
            me,
            dialect,
            replay,
            store: Some(store),
        })
    }

    /// This receiver's account.
    pub fn me(&self) -> &AccountId {
        &self.me
    }

    /// This receiver's SS58-42 address, compared byte-for-byte with `Signed-For`.
    pub fn me_ss58(&self) -> &str {
        &self.me_ss58
    }

    /// The configured dialect.
    pub fn dialect(&self) -> &Dialect {
        &self.dialect
    }

    /// The configured replay policy.
    pub fn replay(&self) -> ReplayPolicy {
        self.replay
    }

    /// The replay store, if any.
    pub fn store(&self) -> Option<&Arc<S>> {
        self.store.as_ref()
    }

    /// Run every header-only check, without the body, the clock or curve operations.
    ///
    /// Returns the *claimed*, unauthenticated sender. Use it to reject junk
    /// before buffering a body; never use it for admission.
    pub fn check_headers(&self, headers: &HeaderMap) -> Result<AccountId, EpistulaError> {
        self.parse(headers).map(|parsed| parsed.hotkey)
    }

    /// [`Self::check_headers`] plus freshness at `now_ms`, still without curve operations.
    ///
    /// Call it with the time the request head arrived, so that stale or
    /// future requests are refused before their body is read.
    pub fn check_head(&self, headers: &HeaderMap, now_ms: u64) -> Result<AccountId, EpistulaError> {
        self.claimed(headers, now_ms).map(|(hotkey, _)| hotkey)
    }

    /// The claimed sender and parsed timestamp of a fresh request head.
    pub(crate) fn claimed(
        &self,
        headers: &HeaderMap,
        now_ms: u64,
    ) -> Result<(AccountId, BigInt), EpistulaError> {
        let parsed = self.parse(headers)?;
        self.check_freshness_big(&parsed.timestamp, now_ms)?;
        Ok((parsed.hotkey, parsed.timestamp))
    }

    /// The longest time a commit may follow the freshness check.
    ///
    /// `None` without replay enforcement; otherwise
    /// `retention - (max_age_ms + max_future_ms) - 1 ms`.
    pub fn commit_slack(&self) -> Option<std::time::Duration> {
        let store = self.store.as_ref()?;
        if self.replay != ReplayPolicy::Enforce {
            return None;
        }
        let window = window_ms(&self.dialect)?;
        let slack = store.retention().as_millis().checked_sub(window + 1)?;
        Some(std::time::Duration::from_millis(
            u64::try_from(slack).unwrap_or(u64::MAX),
        ))
    }

    /// Check shape, version, receiver, freshness and signature.
    ///
    /// Steps in order: header budget and duplicates, version, receiver,
    /// signature grammar, timestamp, sender, uuid, freshness, signature. No
    /// replay state changes; commit the result after admission policy.
    ///
    /// `now_ms` is the receiver time used for freshness. A server may pass the
    /// time the request head arrived, as BitMind does, and then must commit
    /// with a live clock ([`Authenticated::commit`] bounds the gap).
    pub fn authenticate(
        &self,
        headers: &HeaderMap,
        digest: BodyDigest<'_>,
        now_ms: u64,
    ) -> Result<Authenticated, EpistulaError> {
        let parsed = self.parse(headers)?;
        self.check_freshness_big(&parsed.timestamp, now_ms)?;
        let message = request_message(
            self.dialect.message,
            &digest.hex(),
            parsed.uuid,
            &parsed.timestamp_text,
            &parsed.receiver,
        );
        let verify = match self.dialect.wrapped {
            WrappedPolicy::AcceptLikePython => rtao_core::keys::verify,
            WrappedPolicy::Strict => rtao_core::keys::verify_strict,
        };
        let valid = verify(
            message.as_bytes(),
            &parsed.signature,
            parsed.signed_by,
            rtao_core::keys::CRYPTO_SR25519,
        )
        .map_err(|_| EpistulaError::InvalidSenderKey)?;
        if !valid {
            return Err(EpistulaError::SignatureMismatch);
        }
        let timestamp_ms = parsed
            .timestamp
            .to_i128()
            .ok_or(EpistulaError::InvalidTimestamp)?;
        let replay_key = match parsed.uuid {
            Some(uuid) => replay_key(b"uuid", uuid.as_bytes()),
            None => replay_key(b"sig", &parsed.signature),
        };
        Ok(Authenticated {
            caller: EpistulaCaller {
                hotkey: parsed.hotkey,
                signed_by: parsed.signed_by.to_owned(),
                timestamp_ms,
                uuid: parsed.uuid.map(str::to_owned),
                signed_for: (!parsed.receiver.is_empty()).then(|| parsed.receiver.into_owned()),
            },
            replay_key,
            checked_at_ms: now_ms,
            window_ms: window_ms(&self.dialect),
        })
    }

    /// Recheck freshness of a verified timestamp, for example after an awaited policy.
    pub fn check_freshness(&self, timestamp_ms: i128, now_ms: u64) -> Result<(), EpistulaError> {
        self.check_freshness_big(&BigInt::from(timestamp_ms), now_ms)
    }

    /// Authenticate at the current time, then commit unless replay protection is disabled.
    ///
    /// Freshness is measured when this is called, normally after the body was
    /// buffered. To measure it at the request head instead, call
    /// [`Self::authenticate`] with the head time and then
    /// [`Authenticated::commit`].
    pub async fn verify(
        &self,
        headers: &HeaderMap,
        digest: BodyDigest<'_>,
        clock: &dyn Clock,
    ) -> Result<EpistulaCaller, EpistulaError> {
        let authenticated = self.authenticate(headers, digest, now_ms(clock)?)?;
        match (&self.store, self.replay) {
            (Some(store), ReplayPolicy::Enforce) => {
                authenticated.commit(store.as_ref(), clock).await
            }
            _ => Ok(authenticated.into_caller_without_replay()),
        }
    }

    fn parse<'h>(&self, headers: &'h HeaderMap) -> Result<Parsed<'h>, EpistulaError> {
        rtao_btauth::check_auth_header_budget(headers, &header::AUTH, 0)
            .map_err(|_| EpistulaError::HeaderBudget)?;
        let version = single(headers, header::VERSION, EpistulaError::UnknownVersion)?;
        let timestamp = single(headers, header::TIMESTAMP, EpistulaError::InvalidTimestamp)?;
        let uuid = single(headers, header::UUID, EpistulaError::InvalidUuid)?;
        let signed_by = single(headers, header::SIGNED_BY, EpistulaError::InvalidSenderKey)?;
        let signed_for = single(
            headers,
            header::SIGNED_FOR,
            EpistulaError::InvalidReceiverKey,
        )?;
        let signature = single(
            headers,
            header::REQUEST_SIGNATURE,
            EpistulaError::InvalidSignature,
        )?;

        if self.dialect.version == VersionRule::EmitRequire && version != Some(VERSION) {
            return Err(EpistulaError::UnknownVersion);
        }
        let receiver = match self.dialect.receiver {
            ReceiverRule::HeaderMustEqualSelf => {
                if signed_for != Some(self.me_ss58.as_str()) {
                    return Err(EpistulaError::NotForSelf);
                }
                Cow::Owned(self.me_ss58.clone())
            }
            ReceiverRule::SelfAddress => Cow::Owned(self.me_ss58.clone()),
            ReceiverRule::HeaderUncheckedInsecure => {
                Cow::Borrowed(signed_for.ok_or(EpistulaError::InvalidReceiverKey)?)
            }
            ReceiverRule::AlwaysEmpty => Cow::Borrowed(""),
        };
        let signature = signature
            .and_then(decode_signature)
            .ok_or(EpistulaError::InvalidSignature)?;
        let timestamp_text = timestamp.ok_or(EpistulaError::InvalidTimestamp)?;
        let (timestamp, timestamp_text) =
            parse_timestamp(self.dialect.timestamp_parse, timestamp_text)
                .ok_or(EpistulaError::InvalidTimestamp)?;
        let signed_by = signed_by
            .filter(|sender| sender.len() <= MAX_SENDER_SS58_BYTES)
            .ok_or(EpistulaError::InvalidSenderKey)?;
        let hotkey = signed_by
            .parse::<AccountId>()
            .map_err(|_| EpistulaError::InvalidSenderKey)?;
        let uuid = match self.dialect.message {
            MessageForm::WithUuid => Some(
                uuid.filter(|uuid| valid_uuid(uuid))
                    .ok_or(EpistulaError::InvalidUuid)?,
            ),
            MessageForm::NoUuid => None,
        };
        Ok(Parsed {
            signature,
            timestamp,
            timestamp_text,
            signed_by,
            hotkey,
            uuid,
            receiver,
        })
    }

    fn check_freshness_big(&self, timestamp: &BigInt, now_ms: u64) -> Result<(), EpistulaError> {
        check_freshness(&self.dialect, timestamp, now_ms)
    }
}

/// `max_age_ms + max_future_ms`, or `None` when either is unbounded.
fn window_ms(dialect: &Dialect) -> Option<u128> {
    Some(u128::from(dialect.max_age_ms?) + u128::from(dialect.max_future_ms?))
}

/// The inclusive-accept freshness rule of a dialect.
pub(crate) fn check_freshness(
    dialect: &Dialect,
    timestamp: &BigInt,
    now_ms: u64,
) -> Result<(), EpistulaError> {
    let now = BigInt::from(now_ms);
    if let Some(max_age) = dialect.max_age_ms
        && timestamp + max_age < now
    {
        return Err(EpistulaError::Stale(match dialect.freshness_detail {
            FreshnessDetail::BitMind => format!(
                "Request is too stale: {:.1}s old (limit: {}s)",
                seconds(&(&now - timestamp)),
                pyfmt::float_repr(max_age as f64 / 1000.0)
            ),
            FreshnessDetail::Plain => "Request is too stale".into(),
        }));
    }
    if let Some(max_future) = dialect.max_future_ms
        && timestamp - max_future > now
    {
        return Err(EpistulaError::Future(match dialect.freshness_detail {
            FreshnessDetail::BitMind => format!(
                "Request timestamp is in the future: {:.1}s ahead (limit: {}s)",
                seconds(&(timestamp - &now)),
                pyfmt::float_repr(max_future as f64 / 1000.0)
            ),
            FreshnessDetail::Plain => "Request timestamp is in the future".into(),
        }));
    }
    Ok(())
}

/// Python `int_ms / 1000.0`.
fn seconds(milliseconds: &BigInt) -> f64 {
    milliseconds.to_f64().unwrap_or(f64::INFINITY) / 1000.0
}

fn ss58(account: &AccountId) -> String {
    rtao_core::keys::ss58_from_public(account.0, rtao_core::keys::DEFAULT_SS58_FORMAT)
}

fn replay_key(kind: &[u8], id: &[u8]) -> BigInt {
    let digest = Sha256::new()
        .chain_update(REPLAY_DOMAIN)
        .chain_update(kind)
        .chain_update(b"\0")
        .chain_update(id)
        .finalize();
    BigInt::from_bytes_be(Sign::Plus, &digest)
}

/// The rtao uuid grammar: 1 to 128 bytes of visible ASCII (`%x21-7E`).
pub(crate) fn valid_uuid(uuid: &str) -> bool {
    (1..=MAX_UUID_BYTES).contains(&uuid.len()) && uuid.bytes().all(|byte| byte.is_ascii_graphic())
}

/// The single value of `name`, or `error` when repeated or not UTF-8.
pub(crate) fn single<'h>(
    headers: &'h HeaderMap,
    name: &str,
    error: EpistulaError,
) -> Result<Option<&'h str>, EpistulaError> {
    let mut values = headers.get_all(name).iter();
    let Some(first) = values.next() else {
        return Ok(None);
    };
    if values.next().is_some() {
        return Err(error);
    }
    std::str::from_utf8(first.as_bytes())
        .map(Some)
        .map_err(|_| error)
}

/// Parse a timestamp and return it with the text used in the signed message.
pub(crate) fn parse_timestamp(mode: TimestampParse, text: &str) -> Option<(BigInt, Cow<'_, str>)> {
    match mode {
        TimestampParse::Canonical => {
            let bytes = text.as_bytes();
            if bytes.is_empty()
                || bytes.len() > 20
                || !bytes.iter().all(u8::is_ascii_digit)
                || (bytes.len() > 1 && bytes[0] == b'0')
            {
                return None;
            }
            let value = text.parse::<u64>().ok()?;
            Some((BigInt::from(value), Cow::Borrowed(text)))
        }
        TimestampParse::PythonInt => {
            let value = pyfmt::parse_integer(text)?;
            let canonical = value.to_string();
            Some((value, Cow::Owned(canonical)))
        }
        TimestampParse::GoInt64 => {
            let digits = text
                .strip_prefix('+')
                .or_else(|| text.strip_prefix('-'))
                .unwrap_or(text);
            if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            let value = text.parse::<i64>().ok()?;
            Some((BigInt::from(value), Cow::Borrowed(text)))
        }
        TimestampParse::PythonFloat => {
            let value = pyfmt::parse_float(text)?;
            if !value.is_finite() {
                return None;
            }
            Some((BigInt::from_f64(value.trunc())?, Cow::Borrowed(text)))
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    #[test]
    fn timestamp_grammars() {
        use TimestampParse::*;
        let ok = |mode, text: &str| {
            parse_timestamp(mode, text).map(|(v, t)| (v.to_string(), t.into_owned()))
        };
        assert_eq!(ok(Canonical, "0"), Some(("0".into(), "0".into())));
        assert_eq!(
            ok(Canonical, "18446744073709551615").map(|x| x.0),
            Some(u64::MAX.to_string())
        );
        assert_eq!(ok(Canonical, "18446744073709551616"), None);
        assert_eq!(ok(Canonical, "01"), None);
        assert_eq!(ok(Canonical, "+1"), None);
        assert_eq!(ok(Canonical, "1_0"), None);
        assert_eq!(ok(Canonical, ""), None);
        assert_eq!(
            ok(PythonInt, "+1700000000123"),
            Some(("1700000000123".into(), "1700000000123".into()))
        );
        assert_eq!(
            ok(PythonInt, "01700000000123").map(|x| x.1),
            Some("1700000000123".into())
        );
        assert_eq!(ok(PythonInt, "1_000").map(|x| x.1), Some("1000".into()));
        assert_eq!(ok(PythonInt, "1700000000123.0"), None);
        assert_eq!(ok(GoInt64, "+12"), Some(("12".into(), "+12".into())));
        assert_eq!(ok(GoInt64, "-12").map(|x| x.0), Some("-12".into()));
        assert_eq!(ok(GoInt64, "0012").map(|x| x.1), Some("0012".into()));
        assert_eq!(ok(GoInt64, "1_2"), None);
        assert_eq!(ok(GoInt64, "+"), None);
        assert_eq!(ok(GoInt64, "9223372036854775808"), None);
        assert_eq!(
            ok(PythonFloat, "1700000000123.9"),
            Some(("1700000000123".into(), "1700000000123.9".into()))
        );
        assert_eq!(
            ok(PythonFloat, "1.7e12").map(|x| x.0),
            Some("1700000000000".into())
        );
        assert_eq!(ok(PythonFloat, "inf"), None);
        assert_eq!(ok(PythonFloat, "nan"), None);
    }

    #[test]
    fn uuid_grammar() {
        assert!(valid_uuid("8b0e5f5e-3f6c-4a7e-9c1d-2b9a4f6d7e10"));
        assert!(valid_uuid("x"));
        assert!(!valid_uuid(""));
        assert!(!valid_uuid("a b"));
        assert!(!valid_uuid(&"a".repeat(129)));
        assert!(valid_uuid(&"a".repeat(128)));
        assert!(!valid_uuid("é"));
    }

    #[test]
    fn replay_keys_are_domain_separated() {
        let uuid = replay_key(b"uuid", b"abc");
        let sig = replay_key(b"sig", b"abc");
        assert_ne!(uuid, sig);
        assert!(uuid.bits() > 64 && uuid.sign() == Sign::Plus);
    }
}
