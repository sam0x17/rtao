//! Asynchronous weight encryption with an explicit clock and chain size limits.

use rtao_core::CoreError;
use rtao_primitives::{AccountId, Uid};
use std::fmt;

/// Maximum encrypted-commit size admitted by the subnet's consensus mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitSizeLimit {
    /// Yuma's 5,000-byte limit.
    Yuma,
    /// Null's 32 KiB allowance divided among active mechanisms.
    Null {
        /// Current mechanism count; must be nonzero.
        mechanisms: u16,
    },
}
impl CommitSizeLimit {
    /// Return the actual ciphertext byte limit.
    pub fn bytes(self) -> Result<usize, CommitError> {
        match self {
            Self::Yuma => Ok(5_000),
            Self::Null { mechanisms: 0 } => Err(CommitError::InvalidMechanismCount),
            Self::Null { mechanisms } => Ok(32 * 1024 / usize::from(mechanisms)),
        }
    }
}

/// Encrypted fields passed to `commit_timelocked_mechanism_weights`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedWeightCommit {
    /// Compressed upstream TLE ciphertext, without a UserData envelope.
    pub ciphertext: Vec<u8>,
    /// Drand Quicknet round that unlocks this payload.
    pub reveal_round: u64,
}

/// One conformed row and a coherent best-head schedule, with an explicit clock.
#[derive(Debug, Clone)]
pub struct TimelockCommitRequest {
    /// Unix seconds checked against chain time by the caller's ClockGuard.
    pub now: f64,
    /// Signing validator's public account.
    pub hotkey: AccountId,
    /// Conformed destinations and integer values.
    pub row: (Vec<Uid>, Vec<u16>),
    /// Weights version key included in the encrypted payload.
    pub version_key: u64,
    /// Schedule fields read at one best-head block.
    pub state: rtao_core::timelock::epoch_schedule::EpochScheduleState,
    /// Number of subnet epochs before reveal.
    pub reveal_period_epochs: u64,
    /// Chain slot duration in seconds, including fractional localnet slots.
    pub block_time: f64,
}

/// Schedule, encryption, task or size validation failed before submission.
#[derive(Debug)]
pub enum CommitError {
    /// Upstream payload or encryption failure.
    Core(CoreError),
    /// Null consensus cannot divide its allowance among zero mechanisms.
    InvalidMechanismCount,
    /// The compressed ciphertext exceeds the consensus-specific chain bound.
    CiphertextTooLarge {
        /// Actual compressed length.
        bytes: usize,
        /// Maximum admitted length.
        limit: usize,
    },
    /// Spawned encryption work panicked or was cancelled.
    Task(tokio::task::JoinError),
}
impl fmt::Display for CommitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Core(error) => error.fmt(f),
            Self::InvalidMechanismCount => {
                f.write_str("Null consensus requires at least one mechanism")
            }
            Self::CiphertextTooLarge { bytes, limit } => write!(
                f,
                "encrypted weights are {bytes} bytes; subnet limit is {limit}"
            ),
            Self::Task(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for CommitError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Core(error) => Some(error),
            Self::Task(error) => Some(error),
            _ => None,
        }
    }
}

/// Prepare commitments without network I/O or a hidden wall-clock read.
///
/// The scheduler supplies one coherent best-head schedule, its chain slot
/// duration and a clock reading already checked by ClockGuard. Encryption runs
/// on the blocking pool. A rejected pool submission can recompute with fresh
/// inputs by calling this method again; this helper never submits or retries.
#[derive(Debug, Clone, Copy)]
pub struct TimelockCommitter {
    limit: CommitSizeLimit,
}
impl TimelockCommitter {
    /// Select the subnet's consensus-specific ciphertext allowance.
    pub fn new(limit: CommitSizeLimit) -> Result<Self, CommitError> {
        limit.bytes()?;
        Ok(Self { limit })
    }

    /// Encrypt a conformed row to the pallet-faithful predicted reveal round.
    pub async fn commit_at(
        &self,
        request: TimelockCommitRequest,
    ) -> Result<PreparedWeightCommit, CommitError> {
        let limit = self.limit.bytes()?;
        let result = encrypt_at(request).await?;
        if result.ciphertext.len() > limit {
            return Err(CommitError::CiphertextTooLarge {
                bytes: result.ciphertext.len(),
                limit,
            });
        }
        Ok(result)
    }
}

// Standalone intents preserve Python's build-time reads. The validator's
// TimelockCommitter applies its explicit consensus bound to this same crypto.
pub(super) async fn encrypt_at(
    request: TimelockCommitRequest,
) -> Result<PreparedWeightCommit, CommitError> {
    let TimelockCommitRequest {
        now,
        hotkey,
        row,
        version_key,
        state,
        reveal_period_epochs,
        block_time,
    } = request;
    let result = rtao_client::offload(move || {
        rtao_core::timelock::timelock_commit::generate_commit_v2_at(
            now,
            row.0.into_iter().map(|uid| uid.0).collect(),
            row.1,
            version_key,
            state,
            reveal_period_epochs,
            block_time,
            hotkey.0.to_vec(),
        )
    })
    .await
    .map_err(CommitError::Task)?;
    let (ciphertext, reveal_round) = result.map_err(|error| CommitError::Core(error.into()))?;
    Ok(PreparedWeightCommit {
        ciphertext,
        reveal_round,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request(length: u16) -> TimelockCommitRequest {
        TimelockCommitRequest {
            now: 1_752_076_800.25,
            hotkey: AccountId([7; 32]),
            row: (
                (0..length).map(Uid).collect(),
                (0..length).map(|value| value.saturating_add(1)).collect(),
            ),
            version_key: 123,
            state: rtao_core::timelock::epoch_schedule::EpochScheduleState {
                last_epoch_block: 100,
                pending_epoch_at: 0,
                subnet_epoch_index: 5,
                tempo: 20,
                blocks_since_last_step: 1,
                current_block: 101,
            },
            reveal_period_epochs: 1,
            block_time: 0.25,
        }
    }
    #[test]
    fn consensus_limits_use_floor_division_and_refuse_zero_mechanisms() -> Result<(), CommitError> {
        assert_eq!(CommitSizeLimit::Yuma.bytes()?, 5_000);
        assert_eq!(CommitSizeLimit::Null { mechanisms: 3 }.bytes()?, 10_922);
        assert!(matches!(
            TimelockCommitter::new(CommitSizeLimit::Null { mechanisms: 0 }),
            Err(CommitError::InvalidMechanismCount)
        ));
        Ok(())
    }
    #[tokio::test(flavor = "current_thread")]
    async fn explicit_clock_and_chain_round_produce_a_real_ciphertext() -> Result<(), CommitError> {
        let input = request(3);
        let round = rtao_core::timelock::timelock_commit::reveal_round_at(
            input.now,
            &input.state,
            input.reveal_period_epochs,
            input.block_time,
        )
        .map_err(|error| CommitError::Core(error.into()))?;
        let prepared = TimelockCommitter::new(CommitSizeLimit::Yuma)?
            .commit_at(input)
            .await?;
        assert_eq!(prepared.reveal_round, round);
        assert!(!prepared.ciphertext.is_empty());
        Ok(())
    }
    #[tokio::test(flavor = "current_thread")]
    async fn oversized_and_malformed_rows_fail_before_submission() -> Result<(), CommitError> {
        let committer = TimelockCommitter::new(CommitSizeLimit::Yuma)?;
        assert!(matches!(
            committer.commit_at(request(2_500)).await,
            Err(CommitError::CiphertextTooLarge { limit: 5_000, .. })
        ));
        let mut malformed = request(1);
        malformed.row.1.clear();
        assert!(matches!(
            committer.commit_at(malformed).await,
            Err(CommitError::Core(_))
        ));
        Ok(())
    }
}
