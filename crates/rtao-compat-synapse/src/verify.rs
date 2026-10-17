// Derived from opentensor/bittensor bittensor/core/axon.py @ v10.5.0 (MIT); modified by rtao.
// Derived from opentensor/bittensor bittensor/utils/axon_utils.py @ v10.5.0 (MIT); modified by rtao.
//! Python's `Axon.default_verify`, with stricter defaults.

use std::{
    collections::HashMap,
    fmt,
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};

use num_bigint::BigInt;
use num_traits::FromPrimitive;
use rtao_btauth::{AuthError, Clock};
use rtao_primitives::{AccountId, pyfmt::float_repr};

use crate::{ALLOWED_DELTA_NS, RequestHead, V_7_2_0, ss58::decode_ss58_any_prefix};

/// A nonce table refused a nonce.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum V10NonceError {
    /// The nonce is not newer than the stored one.
    TooOld,
    /// The table is full of unexpired entries; the request fails closed.
    Full,
    /// This hotkey already holds its limit of unexpired `hotkey:uuid` keys.
    HotkeyLimit,
}

/// Python's `Axon.nonces`: the last verified nonce per `"{hotkey}:{uuid}"`.
pub trait V10NonceTable: Send + Sync {
    /// Read-only probe, used to pick Python's error precedence.
    fn peek(&self, key: &str) -> Option<BigInt>;
    /// Atomically store `nonce` when no entry exists or it exceeds the
    /// stored one. Called only after the signature verified.
    fn check_and_commit(&self, key: &str, nonce: &BigInt) -> Result<(), V10NonceError>;
}

/// Python's unbounded, never-evicted table. For differential tests only.
#[derive(Debug, Default)]
pub struct UnboundedNonceTable {
    entries: Mutex<HashMap<String, BigInt>>,
}

impl V10NonceTable for UnboundedNonceTable {
    fn peek(&self, key: &str) -> Option<BigInt> {
        self.entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(key)
            .cloned()
    }
    fn check_and_commit(&self, key: &str, nonce: &BigInt) -> Result<(), V10NonceError> {
        let mut entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        if entries.get(key).is_some_and(|stored| nonce <= stored) {
            return Err(V10NonceError::TooOld);
        }
        entries.insert(key.to_owned(), nonce.clone());
        Ok(())
    }
}

/// A capacity-bounded table that only drops entries no replay could use.
///
/// An entry is dropped once `stored_nonce < now - retention`, where
/// `retention = allowed_delta + max_timeout_widening`; every replay of such a
/// nonce already fails the window. The policy must also bound nonces ahead of
/// the clock (`max_future_skew`), so every entry expires at most
/// `max_future_skew + retention` after it was stored. Live entries are never
/// evicted, so a full table refuses new keys ([`V10NonceError::Full`]), and a
/// hotkey holding [`Self::DEFAULT_KEYS_PER_HOTKEY`] live keys cannot add more
/// ([`V10NonceError::HotkeyLimit`]).
pub struct BoundedNonceTable {
    state: Mutex<BoundedState>,
    capacity: usize,
    keys_per_hotkey: usize,
    retention_ns: u64,
    clock: Arc<dyn Clock>,
}

struct BoundedState {
    entries: HashMap<String, BigInt>,
    // Live keys per hotkey (the text before the first `:`).
    per_hotkey: HashMap<String, usize>,
    // No entry can expire before this Unix-nanosecond instant.
    next_expiry_ns: u64,
}

fn hotkey_of(key: &str) -> &str {
    key.split_once(':').map_or(key, |(hotkey, _)| hotkey)
}

impl fmt::Debug for BoundedNonceTable {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BoundedNonceTable")
            .field("capacity", &self.capacity)
            .field("retention_ns", &self.retention_ns)
            .finish_non_exhaustive()
    }
}

impl BoundedNonceTable {
    /// Default capacity: 100,000 `hotkey:uuid` keys.
    pub const DEFAULT_CAPACITY: usize = 100_000;
    /// Default limit of live `hotkey:uuid` keys per hotkey. A Python dendrite
    /// uses one uuid per process; `UuidMode::PerRequest` uses one per
    /// request.
    pub const DEFAULT_KEYS_PER_HOTKEY: usize = 1_024;

    /// Build a table for `policy`, which must always apply the window, cap
    /// timeout widening and bound future nonces; otherwise some entries could
    /// never be dropped safely.
    pub fn new(
        capacity: usize,
        policy: &V10VerifyPolicy,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, String> {
        let Some(widening) = policy.max_timeout_widening else {
            return Err("a bounded nonce table requires max_timeout_widening".into());
        };
        if policy.honour_legacy_version {
            return Err("a bounded nonce table requires the window on every request".into());
        }
        if policy.max_future_skew.is_none() {
            return Err("a bounded nonce table requires max_future_skew".into());
        }
        let retention = policy.allowed_delta.saturating_add(widening);
        Ok(Self {
            state: Mutex::new(BoundedState {
                entries: HashMap::new(),
                per_hotkey: HashMap::new(),
                next_expiry_ns: 0,
            }),
            capacity: capacity.max(1),
            keys_per_hotkey: Self::DEFAULT_KEYS_PER_HOTKEY,
            retention_ns: u64::try_from(retention.as_nanos()).unwrap_or(u64::MAX),
            clock,
        })
    }

    /// Limit the live keys one hotkey may hold (at least 1).
    pub fn with_keys_per_hotkey(mut self, limit: usize) -> Self {
        self.keys_per_hotkey = limit.max(1);
        self
    }

    /// Drop every entry no replay could use. Returns `false` when the clock
    /// failed or no entry can have expired yet.
    fn sweep(&self, state: &mut BoundedState) -> bool {
        let Ok(now) = self.clock.now_ns() else {
            return false;
        };
        if now < state.next_expiry_ns {
            return false;
        }
        let horizon = BigInt::from(now.saturating_sub(self.retention_ns));
        let BoundedState {
            entries,
            per_hotkey,
            ..
        } = state;
        entries.retain(|key, stored| {
            let live = *stored >= horizon;
            if !live && let Some(count) = per_hotkey.get_mut(hotkey_of(key)) {
                *count = count.saturating_sub(1);
            }
            live
        });
        per_hotkey.retain(|_, count| *count > 0);
        let oldest = entries.values().min().cloned();
        state.next_expiry_ns = oldest
            .and_then(|oldest| u64::try_from(oldest).ok())
            .map_or(0, |oldest| oldest.saturating_add(self.retention_ns));
        true
    }

    /// Number of stored keys.
    pub fn len(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entries
            .len()
    }

    /// Whether no key is stored.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl V10NonceTable for BoundedNonceTable {
    fn peek(&self, key: &str) -> Option<BigInt> {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entries
            .get(key)
            .cloned()
    }

    fn check_and_commit(&self, key: &str, nonce: &BigInt) -> Result<(), V10NonceError> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(stored) = state.entries.get_mut(key) {
            if nonce <= stored {
                return Err(V10NonceError::TooOld);
            }
            *stored = nonce.clone();
            return Ok(());
        }
        if state.entries.len() >= self.capacity
            && (!self.sweep(&mut state) || state.entries.len() >= self.capacity)
        {
            return Err(V10NonceError::Full);
        }
        let hotkey = hotkey_of(key);
        let held = |state: &BoundedState| state.per_hotkey.get(hotkey).copied().unwrap_or(0);
        if held(&state) >= self.keys_per_hotkey
            && (!self.sweep(&mut state) || held(&state) >= self.keys_per_hotkey)
        {
            return Err(V10NonceError::HotkeyLimit);
        }
        *state.per_hotkey.entry(hotkey.to_owned()).or_insert(0) += 1;
        // Keep `next_expiry_ns` a lower bound when an older nonce arrives.
        let expiry = u64::try_from(nonce)
            .unwrap_or(0)
            .saturating_add(self.retention_ns);
        state.next_expiry_ns = state.next_expiry_ns.min(expiry);
        state.entries.insert(key.to_owned(), nonce.clone());
        Ok(())
    }
}

/// Freshness rules applied by [`default_verify`].
#[derive(Clone, Debug, PartialEq)]
pub struct V10VerifyPolicy {
    /// Base window (Python: 4 s).
    pub allowed_delta: Duration,
    /// Python skips the window when `bt_header_dendrite_version` is absent
    /// or below 7.2.0. That header is unsigned, so the default (`false`)
    /// always applies the window.
    pub honour_legacy_version: bool,
    /// Refuse `nan`, `inf` and negative `timeout` headers, which disable or
    /// distort Python's window (default `true`).
    pub reject_nonfinite_timeout: bool,
    /// Cap on the dendrite-controlled timeout widening (default 300 s;
    /// `None` is Python's unbounded widening).
    pub max_timeout_widening: Option<Duration>,
    /// Bound on nonces ahead of the local clock (default 60 s; `None` is
    /// Python's unbounded future). A bounded nonce table requires a bound:
    /// without one a far-future nonce would never expire.
    pub max_future_skew: Option<Duration>,
}

impl Default for V10VerifyPolicy {
    fn default() -> Self {
        Self {
            allowed_delta: Duration::from_nanos(ALLOWED_DELTA_NS),
            honour_legacy_version: false,
            reject_nonfinite_timeout: true,
            max_timeout_widening: Some(Duration::from_secs(300)),
            max_future_skew: Some(Duration::from_secs(60)),
        }
    }
}

impl V10VerifyPolicy {
    /// Python 10.5.0's exact behaviour, including its replay bypasses.
    pub fn python() -> Self {
        Self {
            allowed_delta: Duration::from_nanos(ALLOWED_DELTA_NS),
            honour_legacy_version: true,
            reject_nonfinite_timeout: false,
            max_timeout_widening: None,
            max_future_skew: None,
        }
    }
}

/// A [`RequestHead`] whose signature and nonce passed [`default_verify`].
#[derive(Clone, Debug, PartialEq)]
pub struct VerifiedHead {
    head: RequestHead,
    caller: AccountId,
}

impl VerifiedHead {
    /// The verified request headers.
    pub fn head(&self) -> &RequestHead {
        &self.head
    }
    /// The verified sender.
    pub fn caller(&self) -> AccountId {
        self.caller
    }
    /// Consume into the request headers.
    pub fn into_head(self) -> RequestHead {
        self.head
    }
    /// Mutable access for the axon's own fields (not the identity).
    #[cfg(feature = "axon")]
    pub(crate) fn head_mut(&mut self) -> &mut RequestHead {
        &mut self.head
    }
}

/// Why [`default_verify`] refused a request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VerifyRejection {
    /// Python's 401: the message follows `Not Verified with error: `.
    NotVerified(String),
    /// The nonce table is full; the axon answers 503.
    Unavailable(String),
}

impl fmt::Display for VerifyRejection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotVerified(message) | Self::Unavailable(message) => formatter.write_str(message),
        }
    }
}

/// The v10 signed message: `"{nonce}.{sender}.{receiver}.{uuid}.{hash}"`.
///
/// `nonce` renders in canonical decimal; an absent uuid or hash is `None`.
pub fn v10_message(
    nonce: &BigInt,
    sender: &str,
    receiver: &str,
    uuid: Option<&str>,
    body_hash: Option<&str>,
) -> String {
    format!(
        "{nonce}.{sender}.{receiver}.{}.{}",
        uuid.unwrap_or("None"),
        body_hash.unwrap_or("None")
    )
}

const TOO_OLD: &str = "Nonce is too old, a newer one was last processed";

/// Python's `default_verify`, in its order and with its messages.
///
/// Order: decode the hotkey, check the nonce (presence, window, monotonicity),
/// require and strictly parse the signature, verify it against this axon's own
/// hotkey `me`, then atomically commit the nonce. The signed message uses the
/// canonical decimal nonce and the header `computed_body_hash`; the body is
/// bound to that hash later by the caller.
#[allow(clippy::result_large_err)]
pub fn default_verify(
    head: RequestHead,
    me: &AccountId,
    table: &dyn V10NonceTable,
    policy: &V10VerifyPolicy,
    clock: &dyn Clock,
) -> Result<VerifiedHead, (RequestHead, VerifyRejection)> {
    match verify_inner(&head, me, table, policy, clock) {
        Ok(caller) => Ok(VerifiedHead { head, caller }),
        Err(rejection) => Err((head, rejection)),
    }
}

fn not_verified(message: impl Into<String>) -> VerifyRejection {
    VerifyRejection::NotVerified(message.into())
}

fn verify_inner(
    head: &RequestHead,
    me: &AccountId,
    table: &dyn V10NonceTable,
    policy: &V10VerifyPolicy,
    clock: &dyn Clock,
) -> Result<AccountId, VerifyRejection> {
    let dendrite = &head.dendrite;
    let Some(hotkey) = dendrite.hotkey.as_deref() else {
        return Err(not_verified(
            "No SS58 formatted address or public key provided.",
        ));
    };
    decode_ss58_any_prefix(hotkey)
        .map_err(|error| not_verified(format!("Invalid SS58 address: {error}")))?;
    let uuid = dendrite.uuid.as_deref();
    let endpoint_key = format!("{hotkey}:{}", uuid.unwrap_or("None"));
    let Some(nonce) = head.nonce.as_ref() else {
        return Err(not_verified("Missing Nonce"));
    };
    let windowed =
        !policy.honour_legacy_version || dendrite.version.is_some_and(|version| version >= V_7_2_0);
    let stored = table.peek(&endpoint_key);
    if windowed {
        // Python: `synapse.timeout or 0`.
        let mut timeout = head
            .timeout
            .filter(|timeout| *timeout != 0.0)
            .unwrap_or(0.0);
        if policy.reject_nonfinite_timeout && !(timeout.is_finite() && timeout >= 0.0) {
            return Err(not_verified(format!(
                "Invalid timeout: {} is not a finite non-negative number",
                float_repr(timeout)
            )));
        }
        if let Some(cap) = policy.max_timeout_widening {
            timeout = timeout.min(cap.as_secs_f64());
        }
        let now = clock
            .now_ns()
            .map_err(|error| not_verified(format!("Clock error: {error}")))?;
        let delta_ns = u64::try_from(policy.allowed_delta.as_nanos()).unwrap_or(u64::MAX);
        let timeout_ns = timeout * 1_000_000_000.0;
        // Python: int - int - float, then an exact int/float comparison.
        let base = i128::from(now) - i128::from(delta_ns);
        #[allow(clippy::cast_precision_loss)]
        let window = base as f64 - timeout_ns;
        if stored.is_none() && nonce_at_or_below(nonce, window) {
            #[allow(clippy::cast_precision_loss)]
            let diff = (BigInt::from(now) - nonce)
                .to_string()
                .parse::<f64>()
                .unwrap_or(f64::INFINITY)
                / 1e9;
            #[allow(clippy::cast_precision_loss)]
            let allowed = (delta_ns as f64 + timeout_ns) / 1e9;
            return Err(not_verified(format!(
                "Nonce is too old: acceptable delta is {allowed:.2} seconds but request was {diff:.2} seconds old"
            )));
        }
        if let Some(skew) = policy.max_future_skew {
            let limit = BigInt::from(now) + BigInt::from(skew.as_nanos());
            if *nonce > limit {
                return Err(not_verified("Nonce is too far in the future"));
            }
        }
    }
    if stored.is_some_and(|stored| *nonce <= stored) {
        return Err(not_verified(TOO_OLD));
    }
    let signature = dendrite.signature.as_deref().unwrap_or("");
    if signature.is_empty() {
        return Err(not_verified("Missing Signature"));
    }
    let caller = rtao_btauth::legacy::verify_v10_for(
        hotkey,
        nonce,
        uuid,
        head.computed_body_hash.as_deref(),
        signature,
        me,
    )
    .map_err(|error| match error {
        AuthError::MalformedAuth(message) | AuthError::BadSignature(message) => {
            not_verified(message)
        }
        other => not_verified(other.to_string()),
    })?;
    table
        .check_and_commit(&endpoint_key, nonce)
        .map_err(|error| match error {
            V10NonceError::TooOld => not_verified(TOO_OLD),
            V10NonceError::Full => VerifyRejection::Unavailable("Nonce table full".into()),
            V10NonceError::HotkeyLimit => {
                VerifyRejection::Unavailable("Nonce table full for this hotkey".into())
            }
        })?;
    Ok(caller)
}

/// Python's exact `int <= float` comparison.
fn nonce_at_or_below(nonce: &BigInt, window: f64) -> bool {
    if window.is_nan() {
        return false;
    }
    if window.is_infinite() {
        return window > 0.0;
    }
    BigInt::from_f64(window.floor()).is_some_and(|floor| *nonce <= floor)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::net::SocketAddr;

    use rtao_btauth::ClockError;
    use rtao_signer::{KeypairSigner, Signer};

    use super::*;
    use crate::TerminalInfo;

    const ALICE: &str = "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY";
    const BOB: &str = "5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty";
    const UUID: &str = "5ecbd69c-1cec-11ee-b0dc-e29ce36fec1a";
    const NOW: u64 = 1_800_000_000_000_000_000;

    struct Fixed(u64);
    impl Clock for Fixed {
        fn now_ns(&self) -> Result<u64, ClockError> {
            Ok(self.0)
        }
    }

    fn head(nonce: u64, timeout: Option<f64>, version: Option<i64>) -> RequestHead {
        let message = v10_message(
            &BigInt::from(nonce),
            ALICE,
            BOB,
            Some(UUID),
            Some(crate::EMPTY_BODY_HASH),
        );
        let alice =
            KeypairSigner::from_uri("//Alice", rtao_primitives::CryptoType::Sr25519).unwrap();
        let signature = futures::executor::block_on(alice.sign_raw(message.as_bytes())).unwrap();
        RequestHead {
            name: "Synapse".into(),
            timeout,
            dendrite: TerminalInfo {
                hotkey: Some(ALICE.into()),
                uuid: Some(UUID.into()),
                nonce: i64::try_from(nonce).ok(),
                version,
                signature: Some(format!("0x{}", hex::encode(signature))),
                ..TerminalInfo::default()
            },
            axon: TerminalInfo::default(),
            computed_body_hash: Some(crate::EMPTY_BODY_HASH.into()),
            nonce: Some(BigInt::from(nonce)),
            header_size: None,
            total_size: None,
            peer: SocketAddr::from(([127, 0, 0, 1], 1)),
        }
    }

    fn run(
        head: RequestHead,
        policy: &V10VerifyPolicy,
        table: &dyn V10NonceTable,
    ) -> Result<AccountId, String> {
        let me: AccountId = BOB.parse().unwrap();
        default_verify(head, &me, table, policy, &Fixed(NOW))
            .map(|verified| verified.caller())
            .map_err(|(_, rejection)| rejection.to_string())
    }

    #[test]
    fn window_matches_python_upstream_test() {
        let python = V10VerifyPolicy::python();
        let version = Some(crate::V10_VERSION_AS_INT);
        for (offset, ok) in [
            (1, true),
            (3, true),
            (5, true),
            (15, true),
            (18, false),
            (19, false),
        ] {
            let table = UnboundedNonceTable::default();
            let result = run(
                head(NOW - offset * 1_000_000_000, Some(12.0), version),
                &python,
                &table,
            );
            assert_eq!(result.is_ok(), ok, "offset {offset}: {result:?}");
        }
        let table = UnboundedNonceTable::default();
        let error = run(
            head(NOW - 17_250_000_000, Some(12.0), version),
            &python,
            &table,
        )
        .unwrap_err();
        assert_eq!(
            error,
            "Nonce is too old: acceptable delta is 16.00 seconds but request was 17.25 seconds old"
        );
    }

    #[test]
    fn replay_and_bypasses() {
        let python = V10VerifyPolicy::python();
        let strict = V10VerifyPolicy::default();
        let table = UnboundedNonceTable::default();
        let fresh = head(NOW - 1, Some(12.0), Some(crate::V10_VERSION_AS_INT));
        assert_eq!(
            run(fresh.clone(), &strict, &table).unwrap().to_string(),
            ALICE
        );
        assert_eq!(run(fresh, &strict, &table).unwrap_err(), TOO_OLD);

        let stale = NOW - 3_600_000_000_000;
        // S12: `timeout: nan` disables Python's window.
        assert!(
            run(
                head(stale, Some(f64::NAN), Some(crate::V10_VERSION_AS_INT)),
                &python,
                &UnboundedNonceTable::default()
            )
            .is_ok()
        );
        assert!(
            run(
                head(stale, Some(f64::NAN), Some(crate::V10_VERSION_AS_INT)),
                &strict,
                &UnboundedNonceTable::default()
            )
            .unwrap_err()
            .starts_with("Invalid timeout: nan")
        );
        // I8: a missing version selects Python's legacy branch.
        assert!(
            run(
                head(stale, Some(12.0), None),
                &python,
                &UnboundedNonceTable::default()
            )
            .is_ok()
        );
        assert!(
            run(
                head(stale, Some(12.0), None),
                &strict,
                &UnboundedNonceTable::default()
            )
            .unwrap_err()
            .starts_with("Nonce is too old: acceptable delta is 16.00 seconds")
        );
    }

    #[test]
    fn signature_errors_follow_python() {
        let strict = V10VerifyPolicy::default();
        let mut missing = head(NOW, Some(12.0), None);
        missing.dendrite.signature = Some(String::new());
        assert_eq!(
            run(missing, &strict, &UnboundedNonceTable::default()).unwrap_err(),
            "Missing Signature"
        );
        let mut bare = head(NOW, Some(12.0), None);
        bare.dendrite.signature = Some("00".repeat(64));
        assert_eq!(
            run(bare, &strict, &UnboundedNonceTable::default()).unwrap_err(),
            "Invalid signature format. Expected hex string."
        );
        let mut zero = head(NOW, Some(12.0), None);
        zero.dendrite.signature = Some(format!("0x{}", "00".repeat(64)));
        let error = run(zero, &strict, &UnboundedNonceTable::default()).unwrap_err();
        assert!(error.starts_with(&format!(
            "Signature mismatch with {NOW}.{ALICE}.{BOB}.{UUID}."
        )));
        let mut nobody = head(NOW, Some(12.0), None);
        nobody.dendrite.hotkey = None;
        assert_eq!(
            run(nobody, &strict, &UnboundedNonceTable::default()).unwrap_err(),
            "No SS58 formatted address or public key provided."
        );
        let mut garbage = head(NOW, Some(12.0), None);
        garbage.dendrite.hotkey = Some("0OIl".into());
        assert_eq!(
            run(garbage, &strict, &UnboundedNonceTable::default()).unwrap_err(),
            "Invalid SS58 address: Base 58 requirement is violated"
        );
        let mut no_nonce = head(NOW, Some(12.0), None);
        no_nonce.nonce = None;
        assert_eq!(
            run(no_nonce, &strict, &UnboundedNonceTable::default()).unwrap_err(),
            "Missing Nonce"
        );
    }

    #[test]
    fn bounded_table_expires_only_dead_entries() {
        let policy = V10VerifyPolicy::default();
        assert!(
            BoundedNonceTable::new(1, &V10VerifyPolicy::python(), Arc::new(Fixed(NOW))).is_err()
        );
        let table = BoundedNonceTable::new(1, &policy, Arc::new(Fixed(NOW))).unwrap();
        table.check_and_commit("a", &BigInt::from(NOW)).unwrap();
        assert_eq!(
            table.check_and_commit("b", &BigInt::from(NOW)),
            Err(V10NonceError::Full)
        );
        assert_eq!(
            table.check_and_commit("a", &BigInt::from(NOW)),
            Err(V10NonceError::TooOld)
        );
        let later =
            BoundedNonceTable::new(1, &policy, Arc::new(Fixed(NOW + 305_000_000_000))).unwrap();
        later.check_and_commit("a", &BigInt::from(NOW)).unwrap();
        later
            .check_and_commit("b", &BigInt::from(NOW + 305_000_000_000))
            .unwrap();
        assert_eq!(later.len(), 1);
        assert_eq!(later.peek("a"), None);
    }

    #[test]
    fn future_nonces_are_bounded_and_expire() {
        let strict = V10VerifyPolicy::default();
        assert_eq!(strict.max_future_skew, Some(Duration::from_secs(60)));
        let ahead = run(
            head(NOW + 61_000_000_000, Some(12.0), None),
            &strict,
            &UnboundedNonceTable::default(),
        );
        assert_eq!(ahead.unwrap_err(), "Nonce is too far in the future");
        assert!(
            run(
                head(NOW + 59_000_000_000, Some(12.0), None),
                &strict,
                &UnboundedNonceTable::default()
            )
            .is_ok()
        );
        let unbounded = V10VerifyPolicy {
            max_future_skew: None,
            ..V10VerifyPolicy::default()
        };
        assert!(BoundedNonceTable::new(1, &unbounded, Arc::new(Fixed(NOW))).is_err());

        // H2: the attacker's newest admissible nonce (now + 60 s) occupies
        // the only slot; it expires 60 s + 304 s later, not never.
        let attack = NOW + 60_000_000_000;
        let table = BoundedNonceTable::new(1, &strict, Arc::new(Fixed(NOW))).unwrap();
        table
            .check_and_commit("evil:u", &BigInt::from(attack))
            .unwrap();
        assert_eq!(
            table.check_and_commit("honest:u", &BigInt::from(NOW)),
            Err(V10NonceError::Full)
        );
        let still = NOW + 60_000_000_000 + 303_000_000_000;
        let table = BoundedNonceTable::new(1, &strict, Arc::new(Fixed(still))).unwrap();
        table
            .check_and_commit("evil:u", &BigInt::from(attack))
            .unwrap();
        assert_eq!(
            table.check_and_commit("honest:u", &BigInt::from(still)),
            Err(V10NonceError::Full)
        );
        let later = NOW + 60_000_000_000 + 305_000_000_000;
        let table = BoundedNonceTable::new(1, &strict, Arc::new(Fixed(later))).unwrap();
        table
            .check_and_commit("evil:u", &BigInt::from(attack))
            .unwrap();
        table
            .check_and_commit("honest:u", &BigInt::from(later))
            .unwrap();
        assert_eq!(table.peek("evil:u"), None);
    }

    #[test]
    fn keys_per_hotkey_are_limited() {
        let policy = V10VerifyPolicy::default();
        let table = BoundedNonceTable::new(100, &policy, Arc::new(Fixed(NOW)))
            .unwrap()
            .with_keys_per_hotkey(2);
        table.check_and_commit("a:1", &BigInt::from(NOW)).unwrap();
        table.check_and_commit("a:2", &BigInt::from(NOW)).unwrap();
        assert_eq!(
            table.check_and_commit("a:3", &BigInt::from(NOW)),
            Err(V10NonceError::HotkeyLimit)
        );
        // Existing keys still advance, and other hotkeys are unaffected.
        table
            .check_and_commit("a:1", &BigInt::from(NOW + 1))
            .unwrap();
        table.check_and_commit("b:1", &BigInt::from(NOW)).unwrap();
        assert_eq!(table.len(), 3);

        let later = NOW + 305_000_000_000;
        let table = BoundedNonceTable::new(100, &policy, Arc::new(Fixed(later)))
            .unwrap()
            .with_keys_per_hotkey(1);
        table.check_and_commit("a:1", &BigInt::from(NOW)).unwrap();
        table.check_and_commit("a:2", &BigInt::from(later)).unwrap();
        assert_eq!((table.peek("a:1"), table.len()), (None, 1));

        // The axon maps the limit to 503.
        let table = BoundedNonceTable::new(100, &policy, Arc::new(Fixed(NOW)))
            .unwrap()
            .with_keys_per_hotkey(1);
        table
            .check_and_commit(&format!("{ALICE}:other"), &BigInt::from(NOW))
            .unwrap();
        assert_eq!(
            run(head(NOW, Some(12.0), None), &policy, &table).unwrap_err(),
            "Nonce table full for this hotkey"
        );
    }
}
