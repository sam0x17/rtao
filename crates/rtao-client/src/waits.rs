// Derived from RaoFoundation/subtensor sdk/python/bittensor/client.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Chain-observed block, timestamp and epoch waits shared by both facades.

use std::{
    fmt,
    future::Future,
    time::{Duration, SystemTime},
};

use futures::StreamExt;
use rtao_primitives::NetUid;

use crate::{BlockHeader, BlockRef, Client, ClientError, Finality, HeadError, ViewExt};

/// A subnet epoch whose advancement was observed in chain storage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EpochEvent {
    /// Subnet whose epoch ran.
    pub netuid: NetUid,
    /// Observed LastEpochBlock after the run.
    pub block: u64,
    /// LastEpochBlock at the beginning of the wait.
    pub previous: u64,
}

/// Failure to observe the requested block, chain timestamp or subnet epoch.
#[derive(Debug)]
#[non_exhaustive]
pub enum WaitError {
    /// A chain read or runtime check failed.
    Client(ClientError),
    /// A block subscription failed or reported a chain reorganization.
    Head(HeadError),
    /// The caller's wait budget expired.
    Timeout,
    /// The head stream ended before the condition became true.
    StreamEnded,
    /// The subnet has tempo zero and never runs epochs.
    EpochDisabled(NetUid),
}

impl fmt::Display for WaitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Client(error) => error.fmt(formatter),
            Self::Head(error) => error.fmt(formatter),
            Self::Timeout => formatter.write_str("client wait timed out"),
            Self::StreamEnded => {
                formatter.write_str("block stream ended before the wait completed")
            }
            Self::EpochDisabled(netuid) => write!(
                formatter,
                "subnet {netuid} has tempo 0 and never runs epochs"
            ),
        }
    }
}

impl std::error::Error for WaitError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Client(error) => Some(error),
            Self::Head(error) => Some(error),
            _ => None,
        }
    }
}

impl From<ClientError> for WaitError {
    fn from(error: ClientError) -> Self {
        Self::Client(error)
    }
}
impl From<HeadError> for WaitError {
    fn from(error: HeadError) -> Self {
        Self::Head(error)
    }
}

impl Client {
    /// Wait for the requested height, or the next best block when omitted.
    ///
    /// `None` waits indefinitely. `Some(Duration::ZERO)` supplies a zero budget;
    /// any operation that remains pending expires. Translate Python's
    /// `timeout=0` to `None` when its indefinite-wait spelling is needed.
    pub async fn wait_for_block(
        &self,
        block: Option<u64>,
        timeout: Option<Duration>,
    ) -> Result<BlockHeader, WaitError> {
        bounded_wait(timeout, async {
            let target = match block {
                Some(block) => block,
                None => self
                    .at(BlockRef::Best)
                    .await?
                    .block()
                    .number
                    .checked_add(1)
                    .ok_or_else(|| ClientError::Protocol("next block number exceeds u64".into()))?,
            };
            let mut blocks = self.blocks(Finality::Best)?;
            while let Some(header) = blocks.next().await {
                let header = header?;
                if header.number >= target {
                    return Ok(header);
                }
            }
            Err(WaitError::StreamEnded)
        })
        .await
    }

    /// Wait for a UTC timestamp observed on chain, sleeping through long gaps.
    ///
    /// The target is a typed `SystemTime`; callers parse ISO/Unix inputs before
    /// this operation. The chain clock decides when the condition is met.
    /// Timeout `None` is indefinite; `Some(Duration::ZERO)` is a zero budget.
    pub async fn wait_for_timestamp(
        &self,
        when: SystemTime,
        timeout: Option<Duration>,
    ) -> Result<BlockHeader, WaitError> {
        bounded_wait(timeout, async {
            let slot = self.block_time().await?;
            let now = self.timestamp().await?;
            if let Ok(gap) = when.duration_since(now)
                && gap > slot.saturating_mul(4)
            {
                tokio::time::sleep(gap.saturating_sub(slot.saturating_mul(2))).await;
            }
            let mut blocks = self.blocks(Finality::Best)?;
            while let Some(header) = blocks.next().await {
                let header = header?;
                let snapshot = self.at(BlockRef::Hash(header.hash)).await?;
                if snapshot.timestamp().await? >= when {
                    return Ok(header);
                }
            }
            Err(WaitError::StreamEnded)
        })
        .await
    }

    /// Wait until the subnet's LastEpochBlock advances from one pinned baseline.
    ///
    /// Tempo zero refuses before following heads. This observes an actual epoch
    /// run rather than a predicted schedule. Timeout `None` is indefinite;
    /// `Some(Duration::ZERO)` is a zero budget.
    pub async fn wait_for_epoch(
        &self,
        netuid: NetUid,
        timeout: Option<Duration>,
    ) -> Result<EpochEvent, WaitError> {
        bounded_wait(timeout, async {
            use rtao_runtime::storage::subtensor_module as st;
            let (tempo, baseline) = self
                .fetch((st::TEMPO.at((netuid,)), st::LAST_EPOCH_BLOCK.at((netuid,))))
                .await?;
            if tempo == 0 {
                return Err(WaitError::EpochDisabled(netuid));
            }
            let mut blocks = self.blocks(Finality::Best)?;
            while let Some(header) = blocks.next().await {
                let header = header?;
                let snapshot = self.at(BlockRef::Hash(header.hash)).await?;
                let value = snapshot.storage(&st::LAST_EPOCH_BLOCK, (netuid,)).await?;
                if value > baseline {
                    return Ok(EpochEvent {
                        netuid,
                        block: value,
                        previous: baseline,
                    });
                }
            }
            Err(WaitError::StreamEnded)
        })
        .await
    }
}

async fn bounded_wait<T>(
    timeout: Option<Duration>,
    wait: impl Future<Output = Result<T, WaitError>>,
) -> Result<T, WaitError> {
    match timeout {
        Some(limit) => tokio::time::timeout(limit, wait)
            .await
            .map_err(|_| WaitError::Timeout)?,
        None => wait.await,
    }
}
