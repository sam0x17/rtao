// Derived from RaoFoundation/subtensor sdk/python/bittensor/timelock.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Portable Quicknet timelock envelopes and asynchronous beacon decryption.
//!
//! Encryption and decryption use the maintained core's crypto and wire format.
//! Only decryption makes network requests. [`seal`] offloads encryption, while
//! [`open`] waits and retries without blocking a Tokio worker thread.

use std::{
    borrow::Cow,
    fmt,
    future::Future,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use chrono::{DateTime, SecondsFormat, Utc};
use rtao_core::timelock as crypto;
use tokio::time::{Instant, sleep, timeout_at};
use zeroize::Zeroizing;

pub use rtao_primitives::drand::{
    DurationInput, MomentInput, current_round, format_duration, parse_duration, parse_when,
    reveal_time, round_at,
};

mod beacon;
pub use beacon::{DrandClient, DrandConfig, DrandRound};

#[cfg(test)]
mod tests;

const ROUND_SUFFIX: &[u8; 8] = b"AES_GCM_";
const FETCH_GRACE: Duration = Duration::from_secs(30);
const RETRY_INTERVAL: Duration = Duration::from_secs(1);
/// Nominal chain block duration used by [`Reveal::InBlocks`].
///
/// Use [`seal_with_block_time`] for a chain with a different slot duration.
pub const DEFAULT_BLOCK_TIME: Duration = Duration::from_secs(12);

/// A reveal moment, specified exactly one way.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Reveal {
    /// A positive wall-clock duration from the encryption clock.
    In(Duration),
    /// An absolute system-clock time.
    At(SystemTime),
    /// A native Quicknet round.
    Round(u64),
    /// A number of nominal chain blocks from now (addition D-033).
    ///
    /// This estimates wall-clock time; beacon rounds continue independently
    /// of chain progress. Override the nominal duration with
    /// [`seal_with_block_time`] when the chain has a different slot duration.
    InBlocks(u64),
}

impl Reveal {
    /// Parse Python's composable duration grammar, such as `"1h30m"`.
    pub fn after<'a>(value: impl Into<DurationInput<'a>>) -> Result<Self, TimelockError> {
        let seconds = parse_duration(value)?;
        if seconds <= 0.0 {
            return Err(TimelockError::Input(
                "reveal_in must be a positive duration".into(),
            ));
        }
        Duration::try_from_secs_f64(seconds)
            .map(Self::In)
            .map_err(|_| TimelockError::Input("reveal_in must be a finite duration".into()))
    }

    /// Parse a timestamp or Python-compatible ISO moment (naive means UTC).
    pub fn at<'a>(value: impl Into<MomentInput<'a>>) -> Result<Self, TimelockError> {
        let seconds = parse_when(value)?;
        let delta = Duration::try_from_secs_f64(seconds.abs()).map_err(|_| {
            TimelockError::Input("reveal_at is outside the system clock range".into())
        })?;
        let when = if seconds.is_sign_negative() {
            UNIX_EPOCH.checked_sub(delta)
        } else {
            UNIX_EPOCH.checked_add(delta)
        }
        .ok_or_else(|| {
            TimelockError::Input("reveal_at is outside the system clock range".into())
        })?;
        Ok(Self::At(when))
    }
}

/// A malformed envelope, unavailable beacon, or failed crypto operation.
#[derive(Clone, Debug, PartialEq, Eq)]
#[doc(alias = "bittensor.timelock.TimelockError")]
#[doc(alias = "bittensor.timelock.TimelockNotReady")]
pub enum TimelockError {
    /// An invalid time input or malformed portable envelope.
    Input(String),
    /// The selected reveal time has not arrived.
    #[doc(alias = "TimelockNotReady")]
    NotReady {
        /// Quicknet round needed for decryption.
        reveal_round: u64,
        /// Its UTC publication time.
        reveal_at: DateTime<Utc>,
        /// Time remaining according to the supplied wall clock.
        remaining: Duration,
    },
    /// An invalid HTTP/TLS client configuration.
    Configuration(String),
    /// A beacon HTTP, response-validation or decoding failure.
    Beacon(String),
    /// The total operation deadline expired.
    Deadline {
        /// Quicknet round the caller was trying to open.
        reveal_round: u64,
    },
    /// Core cryptography rejected the envelope or round signature.
    Crypto(String),
    /// A blocking crypto task failed to join.
    Offload(String),
    /// The beacon grace period elapsed without decrypting the payload.
    Unavailable {
        /// Quicknet round needed for decryption.
        reveal_round: u64,
        /// Last fetch or cryptography diagnostic.
        detail: String,
    },
}

impl fmt::Display for TimelockError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Input(message)
            | Self::Configuration(message)
            | Self::Beacon(message)
            | Self::Crypto(message)
            | Self::Offload(message) => formatter.write_str(message),
            Self::NotReady {
                reveal_round,
                reveal_at,
                remaining,
            } => {
                let duration = format_duration(*remaining).map_err(|_| fmt::Error)?;
                write!(
                    formatter,
                    "not decryptable yet: reveals at {} (round {reveal_round}, {duration} from now)",
                    reveal_at.to_rfc3339_opts(SecondsFormat::Secs, false)
                )
            }
            Self::Deadline { reveal_round } => {
                write!(
                    formatter,
                    "timelock deadline exceeded for round {reveal_round}"
                )
            }
            Self::Unavailable {
                reveal_round,
                detail,
            } => write!(
                formatter,
                "failed to decrypt round {reveal_round} (beacon unreachable or ciphertext corrupt): {detail}"
            ),
        }
    }
}
impl std::error::Error for TimelockError {}
impl From<rtao_primitives::drand::TimelockError> for TimelockError {
    fn from(error: rtao_primitives::drand::TimelockError) -> Self {
        Self::Input(error.to_string())
    }
}

fn crypto_error(error: rtao_core::codec::Error) -> TimelockError {
    TimelockError::Crypto(rtao_core::CoreError::from(error).to_string())
}

/// Borrowed or owned input accepted by [`Timelocked::parse`].
pub enum Ciphertext<'a> {
    /// Portable core envelope bytes.
    Bytes(Cow<'a, [u8]>),
    /// Portable envelope hex; an optional lower-case `0x` prefix is accepted.
    Hex(Cow<'a, str>),
    /// An already parsed payload.
    Sealed(&'a Timelocked),
}

impl<'a> From<&'a [u8]> for Ciphertext<'a> {
    fn from(value: &'a [u8]) -> Self {
        Self::Bytes(Cow::Borrowed(value))
    }
}
impl<'a, const N: usize> From<&'a [u8; N]> for Ciphertext<'a> {
    fn from(value: &'a [u8; N]) -> Self {
        Self::Bytes(Cow::Borrowed(value))
    }
}
impl From<Vec<u8>> for Ciphertext<'_> {
    fn from(value: Vec<u8>) -> Self {
        Self::Bytes(Cow::Owned(value))
    }
}
impl<'a> From<&'a str> for Ciphertext<'a> {
    fn from(value: &'a str) -> Self {
        Self::Hex(Cow::Borrowed(value))
    }
}
impl From<String> for Ciphertext<'_> {
    fn from(value: String) -> Self {
        Self::Hex(Cow::Owned(value))
    }
}
impl<'a> From<&'a Timelocked> for Ciphertext<'a> {
    fn from(value: &'a Timelocked) -> Self {
        Self::Sealed(value)
    }
}

/// A portable core `UserData` envelope and the round that opens it.
///
/// The inner TLE serialization ends with `AES_GCM_`; the core envelope appends
/// the little-endian round. This is the same trailer Python parses. Full
/// cryptographic decoding is deferred to the core on a blocking thread.
#[derive(Clone, PartialEq, Eq)]
#[doc(alias = "bittensor.timelock.Timelocked")]
pub struct Timelocked {
    ciphertext: Vec<u8>,
    reveal_round: u64,
}

impl Timelocked {
    /// Rehydrate from bytes, hex, or an already parsed payload.
    ///
    /// As in Python, this validates the trailer, rather than proving the
    /// ciphertext decrypts. [`encrypted`](Self::encrypted) and [`open`] perform
    /// the core's complete envelope/cryptography validation.
    pub fn parse<'a>(input: impl Into<Ciphertext<'a>>) -> Result<Self, TimelockError> {
        let ciphertext = match input.into() {
            Ciphertext::Bytes(bytes) => bytes.into_owned(),
            Ciphertext::Hex(text) => {
                let text = text.trim_matches(python_whitespace);
                let text = text.strip_prefix("0x").unwrap_or(text);
                // Python bytes.fromhex ignores ASCII whitespace between byte
                // pairs, but rejects whitespace between the two nibbles.
                let mut joined = String::with_capacity(text.len());
                let mut nibbles = 0usize;
                for ch in text.chars() {
                    if matches!(ch, '\t'..='\r' | ' ') {
                        if !nibbles.is_multiple_of(2) {
                            return Err(hex_input_error());
                        }
                    } else {
                        joined.push(ch);
                        nibbles += 1;
                    }
                }
                hex::decode(joined).map_err(|_| hex_input_error())?
            }
            Ciphertext::Sealed(sealed) => return Ok(sealed.clone()),
        };
        let trailer = ciphertext
            .get(ciphertext.len().saturating_sub(16)..)
            .filter(|trailer| trailer.len() == 16 && trailer[..8] == ROUND_SUFFIX[..])
            .ok_or_else(|| {
                TimelockError::Input(
                    "not a timelock ciphertext (missing the reveal-round trailer)".into(),
                )
            })?;
        let round = trailer[8..]
            .try_into()
            .map(u64::from_le_bytes)
            .map_err(|_| hex_input_error())?;
        Ok(Self {
            ciphertext,
            reveal_round: round,
        })
    }

    /// Portable envelope bytes; publish these or [`hex`](Self::hex).
    pub fn ciphertext(&self) -> &[u8] {
        &self.ciphertext
    }

    /// The embedded Quicknet reveal round.
    pub fn reveal_round(&self) -> u64 {
        self.reveal_round
    }

    /// Consume the payload and return its portable envelope bytes.
    pub fn into_bytes(self) -> Vec<u8> {
        self.ciphertext
    }

    /// UTC publication time of the required beacon round.
    pub fn reveal_at(&self) -> Result<DateTime<Utc>, TimelockError> {
        Ok(reveal_time(self.reveal_round)?)
    }

    /// Time until reveal according to the process clock, clamped to zero.
    pub fn remaining(&self) -> Result<Duration, TimelockError> {
        self.remaining_at(SystemTime::now())
    }

    /// Time until reveal at an injected clock reading, clamped to zero.
    pub fn remaining_at(&self, now: SystemTime) -> Result<Duration, TimelockError> {
        let target: SystemTime = self.reveal_at()?.into();
        Ok(target.duration_since(now).unwrap_or(Duration::ZERO))
    }

    /// Whether the process clock has reached the reveal round.
    ///
    /// This is local round arithmetic; it does not promise that an HTTP relay
    /// has published the signature. [`open`] allows a bounded grace period.
    pub fn revealed(&self) -> Result<bool, TimelockError> {
        self.revealed_at(SystemTime::now())
    }

    /// Whether an injected wall clock has reached the reveal round.
    pub fn revealed_at(&self, now: SystemTime) -> Result<bool, TimelockError> {
        Ok(current_round(Some(parse_when(now)?))? >= self.reveal_round)
    }

    /// Portable lower-case envelope hex, without a prefix.
    pub fn hex(&self) -> String {
        hex::encode(&self.ciphertext)
    }

    /// Extract the core's inner TLE bytes for a chain commitment.
    ///
    /// This offloads the SCALE decode even for a large payload. The bytes are
    /// distinct from the portable envelope and do not include its round.
    pub async fn encrypted(&self) -> Result<Vec<u8>, TimelockError> {
        let bytes = self.ciphertext.clone();
        rtao_core::offload(move || crypto::inner_ciphertext(&bytes).map_err(crypto_error))
            .await
            .map_err(|error| TimelockError::Offload(error.to_string()))?
    }

    /// Construct the native commitment variant from inner bytes and its round.
    pub async fn commitment_field(&self) -> Result<rtao_runtime::models::Data, TimelockError> {
        Ok(rtao_runtime::models::Data::TimelockEncrypted {
            encrypted: self.encrypted().await?,
            reveal_round: self.reveal_round,
        })
    }

    /// Open this payload with the configured asynchronous beacon client.
    pub async fn decrypt(
        &self,
        drand: &DrandClient,
        options: OpenOptions,
    ) -> Result<Vec<u8>, TimelockError> {
        open(self, drand, options).await
    }
}

impl AsRef<[u8]> for Timelocked {
    fn as_ref(&self) -> &[u8] {
        self.ciphertext()
    }
}

impl fmt::Debug for Timelocked {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Timelocked")
            .field("bytes", &self.ciphertext.len())
            .field("reveal_round", &self.reveal_round)
            .finish()
    }
}

fn hex_input_error() -> TimelockError {
    TimelockError::Input("expected timelock ciphertext as raw bytes or a hex string".into())
}

fn python_whitespace(ch: char) -> bool {
    // Python str.strip includes the four C0 information separators as well
    // as Unicode's White_Space property (Rust's is_whitespace).
    ch.is_whitespace() || matches!(ch, '\u{1c}'..='\u{1f}')
}

/// Seal binary or UTF-8 data to a future Quicknet round.
///
/// The core's local cryptography runs on a blocking task; no network is used.
#[doc(alias = "encrypt")]
#[doc(alias = "bittensor.timelock.encrypt")]
pub async fn seal(
    data: impl AsRef<[u8]> + Send,
    reveal: Reveal,
) -> Result<Timelocked, TimelockError> {
    seal_at(data, reveal, SystemTime::now()).await
}

/// Seal with an injected wall clock and the nominal chain block duration.
pub async fn seal_at(
    data: impl AsRef<[u8]> + Send,
    reveal: Reveal,
    now: SystemTime,
) -> Result<Timelocked, TimelockError> {
    seal_with_block_time_at(data, reveal, DEFAULT_BLOCK_TIME, now).await
}

/// Seal with an explicit block duration for [`Reveal::InBlocks`].
pub async fn seal_with_block_time(
    data: impl AsRef<[u8]> + Send,
    reveal: Reveal,
    block_time: Duration,
) -> Result<Timelocked, TimelockError> {
    seal_with_block_time_at(data, reveal, block_time, SystemTime::now()).await
}

/// Seal with injected wall time and block duration, without consulting a node.
pub async fn seal_with_block_time_at(
    data: impl AsRef<[u8]> + Send,
    reveal: Reveal,
    block_time: Duration,
    now: SystemTime,
) -> Result<Timelocked, TimelockError> {
    let target = target_round(reveal, block_time, now)?;
    let payload = Zeroizing::new(data.as_ref().to_vec());
    let (bytes, actual_round) = rtao_core::offload(move || {
        crypto::encrypt_at_round(&payload, target).map_err(crypto_error)
    })
    .await
    .map_err(|error| TimelockError::Offload(error.to_string()))??;
    let sealed = Timelocked::parse(bytes)?;
    if actual_round != target || sealed.reveal_round != target {
        return Err(TimelockError::Crypto(
            "core returned a different timelock reveal round".into(),
        ));
    }
    Ok(sealed)
}

fn target_round(
    reveal: Reveal,
    block_time: Duration,
    now: SystemTime,
) -> Result<u64, TimelockError> {
    let now = parse_when(now)?;
    let target = match reveal {
        Reveal::Round(round) => round,
        Reveal::At(when) => round_at(when)?,
        Reveal::In(duration) => {
            if duration.is_zero() {
                return Err(TimelockError::Input(
                    "reveal_in must be a positive duration".into(),
                ));
            }
            round_at(now + duration.as_secs_f64())?
        }
        Reveal::InBlocks(blocks) => {
            if blocks == 0 || block_time.is_zero() {
                return Err(TimelockError::Input(
                    "reveal blocks and block duration must be positive".into(),
                ));
            }
            let nanos = block_time
                .as_nanos()
                .checked_mul(u128::from(blocks))
                .ok_or_else(|| TimelockError::Input("reveal duration overflow".into()))?;
            let seconds = u64::try_from(nanos / 1_000_000_000)
                .map_err(|_| TimelockError::Input("reveal duration overflow".into()))?;
            let subsec = u32::try_from(nanos % 1_000_000_000)
                .map_err(|_| TimelockError::Input("reveal duration overflow".into()))?;
            round_at(now + Duration::new(seconds, subsec).as_secs_f64())?
        }
    };
    let latest = current_round(Some(now))?;
    if target <= latest {
        return Err(TimelockError::Input(format!(
            "reveal moment is not in the future (round {target}, but the beacon is already at round {latest})"
        )));
    }
    // Check Python's datetime bounds before doing expensive cryptography.
    reveal_time(target)?;
    Ok(target)
}

/// Decryption wait and total operation deadline options.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OpenOptions {
    /// Wait until the reveal time; defaults to true, as Python decrypt_async.
    pub wait: bool,
    /// Total time allowed, including reveal wait, HTTP and crypto.
    ///
    /// A reveal farther away returns [`TimelockError::NotReady`]. After the
    /// reveal, expiration returns [`TimelockError::Deadline`]. Independently,
    /// beacon lag retries are bounded to 30 seconds.
    pub timeout: Option<Duration>,
}

impl Default for OpenOptions {
    fn default() -> Self {
        Self {
            wait: true,
            timeout: None,
        }
    }
}

/// Open an envelope using asynchronous, bounded beacon HTTP and core crypto.
///
/// Fetch and crypto failures retry every second for at most 30 seconds after
/// reveal. A caller timeout caps the complete operation. Dropping the future
/// cancels waits and HTTP; an already running blocking crypto task finishes
/// independently, as with Tokio `spawn_blocking`.
#[doc(alias = "decrypt_async")]
#[doc(alias = "decrypt")]
#[doc(alias = "bittensor.timelock.decrypt_async")]
#[doc(alias = "bittensor.timelock.decrypt")]
pub async fn open(
    sealed: &Timelocked,
    drand: &DrandClient,
    options: OpenOptions,
) -> Result<Vec<u8>, TimelockError> {
    open_with_attempt(sealed, drand.clock.as_ref(), options, || async {
        let round = drand.fetch_round(sealed.reveal_round).await?;
        let bytes = sealed.ciphertext.clone();
        let mut plaintext = rtao_core::offload(move || {
            crypto::decrypt_with_signature(&bytes, &round.signature)
                .map(Zeroizing::new)
                .map_err(crypto_error)
        })
        .await
        .map_err(|error| TimelockError::Offload(error.to_string()))??;
        // The blocking result stays guarded if its waiter is cancelled. Transfer
        // ownership only when this attempt delivers the caller's plaintext.
        Ok(std::mem::take(&mut *plaintext))
    })
    .await
}

async fn open_with_attempt<F, Fut>(
    sealed: &Timelocked,
    clock: &dyn crate::Clock,
    options: OpenOptions,
    mut attempt: F,
) -> Result<Vec<u8>, TimelockError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<Vec<u8>, TimelockError>>,
{
    let started = Instant::now();
    let deadline = options
        .timeout
        .map(|duration| {
            started.checked_add(duration).ok_or_else(|| {
                TimelockError::Configuration("timelock timeout is outside the clock range".into())
            })
        })
        .transpose()?;
    let remaining = sealed.remaining_at(clock.now())?;
    if !remaining.is_zero() {
        let not_ready = || -> Result<TimelockError, TimelockError> {
            Ok(TimelockError::NotReady {
                reveal_round: sealed.reveal_round,
                reveal_at: sealed.reveal_at()?,
                remaining,
            })
        };
        let reveal = started.checked_add(remaining).ok_or_else(|| {
            TimelockError::Input("reveal moment is outside the monotonic clock range".into())
        })?;
        if !options.wait || deadline.is_some_and(|deadline| reveal > deadline) {
            return Err(not_ready()?);
        }
        sleep(remaining).await;
    }
    let grace = Instant::now()
        .checked_add(FETCH_GRACE)
        .ok_or_else(|| TimelockError::Configuration("beacon grace clock overflow".into()))?;
    let until = deadline.map_or(grace, |deadline| deadline.min(grace));
    let mut last_error = String::from("signature not available");
    loop {
        if Instant::now() >= until {
            break;
        }
        match timeout_at(until, attempt()).await {
            Ok(Ok(plaintext)) => return Ok(plaintext),
            Ok(Err(error)) => last_error = error.to_string(),
            Err(_) => break,
        }
        let remaining = until.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        sleep(RETRY_INTERVAL.min(remaining)).await;
    }
    if deadline.is_some_and(|deadline| deadline <= grace && Instant::now() >= deadline) {
        return Err(TimelockError::Deadline {
            reveal_round: sealed.reveal_round,
        });
    }
    Err(TimelockError::Unavailable {
        reveal_round: sealed.reveal_round,
        detail: last_error,
    })
}

/// Typed construction of the raw timelocked commitment call.
pub mod commitments {
    use super::{TimelockError, Timelocked};

    /// Build `Commitments.set_commitment` using inner TLE bytes per field.
    ///
    /// Submit with the hotkey role through `Executor::submit_call_dyn` and an
    /// explicit policy allowing raw calls. This helper performs no signing or
    /// chain I/O; execution retains the normal fee, nonce and policy checks.
    pub async fn set_timelocked(
        netuid: rtao_primitives::NetUid,
        fields: &[Timelocked],
    ) -> Result<rtao_runtime::Call, TimelockError> {
        let mut encrypted = Vec::with_capacity(fields.len());
        for field in fields {
            encrypted.push(field.commitment_field().await?);
        }
        Ok(rtao_runtime::calls::commitments::set_commitment(
            netuid,
            rtao_runtime::models::CommitmentInfo { fields: encrypted },
        ))
    }
}
