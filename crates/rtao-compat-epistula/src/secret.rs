//! A pre-body filter on `Epistula-Secret-Signature-1`.
//!
//! No upstream receiver verifies secret signatures. This filter is an
//! optional denial-of-service guard. Every upstream sender derives its
//! secret signatures from the same timestamp it sends in
//! `Epistula-Timestamp`, so the filter checks that timestamp's freshness and
//! then verifies `Secret-Signature-1` for that timestamp's ten-second
//! interval: at most one sr25519 verification per request head. A verified
//! `(sender, signature, interval)` triple is remembered until the end of the
//! interval after the matched one, and only a request presenting the same
//! signature bytes for the same sender and interval skips verification. The
//! filter never authenticates a request; the request signature is always
//! verified afterwards.

use std::{collections::HashMap, sync::Mutex};

use http::HeaderMap;
use num_traits::ToPrimitive;
use rtao_btauth::{MAX_SENDER_SS58_BYTES, WrappedPolicy};
use rtao_primitives::AccountId;

use crate::{
    Dialect, EpistulaError, INTERVAL_MS, SecretPolicy, SecretSubject, decode_signature, header,
    interval, secret_message,
    verify::{check_freshness, parse_timestamp, single},
};

/// Default number of remembered signatures.
pub const DEFAULT_MAX_ENTRIES: usize = 4_096;

type CacheKey = (String, [u8; 64]);

/// Verifies `Epistula-Secret-Signature-1` for the claimed timestamp's interval.
#[derive(Debug)]
pub struct SecretSigFilter {
    me_ss58: String,
    dialect: Dialect,
    max_entries: usize,
    /// `(signed_by, signature)` to `(interval index, expiry in Unix ms)`.
    cache: Mutex<HashMap<CacheKey, (u64, u128)>>,
}

impl SecretSigFilter {
    /// Filter requests addressed to `me`, using the senders' secret format.
    ///
    /// Timestamps are parsed as Python `int()` and must lie within BitMind's
    /// ±15 s window. Use [`Self::for_dialect`] to match a verifier exactly.
    pub fn new(me: AccountId, policy: SecretPolicy) -> Self {
        let mut dialect = Dialect::BITMIND;
        dialect.secret = policy;
        Self::for_dialect(me, &dialect)
    }

    /// Filter with the secret format, timestamp grammar, freshness window and
    /// wrapped-signature rule of `dialect`.
    pub fn for_dialect(me: AccountId, dialect: &Dialect) -> Self {
        Self {
            me_ss58: rtao_core::keys::ss58_from_public(me.0, rtao_core::keys::DEFAULT_SS58_FORMAT),
            dialect: dialect.clone(),
            max_entries: DEFAULT_MAX_ENTRIES,
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// Bound the number of remembered signatures (at least one).
    pub fn with_max_entries(mut self, max_entries: usize) -> Self {
        self.max_entries = max_entries.max(1);
        self
    }

    /// Accept only raw, unwrapped secret signatures.
    pub fn with_wrapped(mut self, wrapped: WrappedPolicy) -> Self {
        self.dialect.wrapped = wrapped;
        self
    }

    /// Check a request head at receiver time `now_ms`.
    ///
    /// A stale or future timestamp fails with the dialect's freshness error.
    pub fn check(&self, headers: &HeaderMap, now_ms: u64) -> Result<(), EpistulaError> {
        let signed_by = single(headers, header::SIGNED_BY, EpistulaError::InvalidSenderKey)?
            .filter(|sender| sender.len() <= MAX_SENDER_SS58_BYTES)
            .ok_or(EpistulaError::InvalidSenderKey)?;
        let (timestamp, _) = single(headers, header::TIMESTAMP, EpistulaError::InvalidTimestamp)?
            .and_then(|text| parse_timestamp(self.dialect.timestamp_parse, text))
            .ok_or(EpistulaError::InvalidTimestamp)?;
        check_freshness(&self.dialect, &timestamp, now_ms)?;
        // A sender signs with its own u64 millisecond clock; nothing else can match.
        let timestamp = timestamp
            .to_u64()
            .ok_or(EpistulaError::SecretSignatureMismatch)?;
        let signature = single(
            headers,
            header::SECRET_SIGNATURE[1],
            EpistulaError::SecretSignatureMismatch,
        )?
        .and_then(decode_signature)
        .ok_or(EpistulaError::SecretSignatureMismatch)?;
        let (anchor, index) = interval(timestamp);
        let key = (signed_by.to_owned(), signature);
        if let Ok(cache) = self.cache.lock()
            && cache
                .get(&key)
                .is_some_and(|(cached, expiry)| *cached == index && u128::from(now_ms) < *expiry)
        {
            return Ok(());
        }
        let subject = match self.dialect.secret.subject {
            SecretSubject::Receiver => self.me_ss58.as_str(),
            SecretSubject::Sender => signed_by,
        };
        let verify = match self.dialect.wrapped {
            WrappedPolicy::AcceptLikePython => rtao_core::keys::verify,
            WrappedPolicy::Strict => rtao_core::keys::verify_strict,
        };
        let message = secret_message(self.dialect.secret.interval, timestamp, 1, subject)
            .map_err(|_| EpistulaError::SecretSignatureMismatch)?;
        let valid = verify(
            message.as_bytes(),
            &signature,
            signed_by,
            rtao_core::keys::CRYPTO_SR25519,
        )
        .map_err(|_| EpistulaError::InvalidSenderKey)?;
        if !valid {
            return Err(EpistulaError::SecretSignatureMismatch);
        }
        self.remember(key, index, anchor + u128::from(INTERVAL_MS), now_ms);
        Ok(())
    }

    fn remember(&self, key: CacheKey, index: u64, expiry: u128, now_ms: u64) {
        let Ok(mut cache) = self.cache.lock() else {
            return;
        };
        if !cache.contains_key(&key) && cache.len() >= self.max_entries {
            cache.retain(|_, (_, entry)| u128::from(now_ms) < *entry);
            if cache.len() >= self.max_entries {
                return;
            }
        }
        cache.insert(key, (index, expiry));
    }
}
