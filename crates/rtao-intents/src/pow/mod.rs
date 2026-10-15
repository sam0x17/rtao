// Derived from RaoFoundation/subtensor sdk/python/bittensor/pow_registration.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Public registration mining on dedicated, cancellable CPU workers.
//!
//! `pow` activates the core's host capability. `pow-gpu` currently exposes the
//! same CPU solver; GPU selection is reserved for M7. No signer or wallet enters
//! this API. Submit a returned proof promptly through the coldkey intent path.

use std::{future::poll_fn, sync::Arc, time::Duration};

use rtao_client::{BlockRef, Client, ClientError, HeadError, HeadProvider, Snapshot, ViewExt};
use rtao_primitives::{AccountId, H256, NetUid};
use rtao_runtime::storage::subtensor_module as storage;
use tokio::time::Instant;
pub use tokio_util::sync::CancellationToken;

mod workers;
use workers::{CpuWorkers, Miner};

/// Runtime challenge domain, without subnet, block, keys or nonce.
pub const DOMAIN: &[u8] = b"subtensor-pow-register-v1";
/// A proof is rejected when its published work block is this many blocks old.
pub const MAX_WORK_AGE_BLOCKS: u64 = 5;
/// Refresh the published challenge at least this often during a long search.
pub const CHALLENGE_REFRESH: Duration = Duration::from_secs(12);

/// CPU worker count and total time allowed, including reads and validation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PowConfig {
    /// Requested CPU threads, in `1..=32`, capped by available parallelism.
    pub workers: u8,
    /// Positive deadline duration, at most one hour.
    pub timeout: Duration,
}

impl Default for PowConfig {
    fn default() -> Self {
        Self {
            workers: 4,
            timeout: Duration::from_secs(300),
        }
    }
}

/// Mining configuration, admission, transport or worker failure.
#[derive(Debug)]
pub enum PowError {
    /// Workers must be in `1..=32` and timeout in `(0, 3600]` seconds.
    InvalidConfig,
    /// Root has no PoW registration path.
    RootSubnet,
    /// The subnet owner disabled PoW registration.
    Disabled,
    /// The hotkey already has a UID, including UID zero.
    AlreadyRegistered,
    /// The per-block registration limit must be positive.
    ZeroRegistrationLimit,
    /// The target subnet is absent from the pinned runtime state.
    SubnetNotExists,
    /// The subnet cannot assign any neuron UID.
    NoNeuronIdAvailable,
    /// The pinned block has exhausted its registration allowance.
    RegistrationLimitReached,
    /// Work is stale, from the future, reused, or has no canonical hash.
    InvalidWorkBlock,
    /// The seal does not satisfy the current effective difficulty.
    InvalidDifficulty,
    /// Submitted work does not match the public registration challenge.
    InvalidSeal,
    /// The total mining deadline elapsed.
    Timeout,
    /// The caller cancelled the operation.
    Cancelled,
    /// A pinned chain read failed.
    Client(ClientError),
    /// Fresh canonical hash or head subscription failed.
    Head(HeadError),
    /// A core miner or seal operation failed.
    Core(String),
    /// A worker returned bytes that do not match the CPU seal.
    InvalidProof,
    /// A dedicated thread could not start or its result channel closed.
    Worker(String),
}

impl std::fmt::Display for PowError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidConfig => f.write_str(
                "workers must be 1..32 and timeout must be positive and at most 3600 seconds",
            ),
            Self::RootSubnet => f.write_str("PoW registration requires a non-root u16 netuid"),
            Self::Disabled => f.write_str("the subnet owner must enable PoW registration"),
            Self::AlreadyRegistered => {
                f.write_str("the hotkey is already registered on this subnet")
            }
            Self::ZeroRegistrationLimit => {
                f.write_str("the subnet's per-block registration limit must be positive")
            }
            Self::SubnetNotExists => f.write_str("the subnet does not exist"),
            Self::NoNeuronIdAvailable => f.write_str("the subnet cannot assign a neuron UID"),
            Self::RegistrationLimitReached => {
                f.write_str("the subnet's registration allowance for this block is exhausted")
            }
            Self::InvalidWorkBlock => {
                f.write_str("PoW work block is expired, in the future, reused, or unavailable")
            }
            Self::InvalidDifficulty => {
                f.write_str("PoW seal does not satisfy the subnet's current difficulty")
            }
            Self::InvalidSeal => {
                f.write_str("PoW seal does not match the public registration challenge")
            }
            Self::Timeout => {
                f.write_str("no fresh registration proof found within the mining time limit")
            }
            Self::Cancelled => f.write_str("PoW mining was cancelled"),
            Self::Client(error) => std::fmt::Display::fmt(error, f),
            Self::Head(error) => std::fmt::Display::fmt(error, f),
            Self::Core(error) | Self::Worker(error) => f.write_str(error),
            Self::InvalidProof => f.write_str("miner returned an invalid registration proof"),
        }
    }
}

impl std::error::Error for PowError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Client(error) => Some(error),
            Self::Head(error) => Some(error),
            _ => None,
        }
    }
}

impl From<ClientError> for PowError {
    fn from(error: ClientError) -> Self {
        Self::Client(error)
    }
}
impl From<HeadError> for PowError {
    fn from(error: HeadError) -> Self {
        Self::Head(error)
    }
}

/// A CPU-verified runtime proof binding the subnet, block and both public keys.
///
/// Submit promptly with the coldkey and hotkey recorded in this proof.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PowProof {
    /// Non-root target subnet.
    pub netuid: NetUid,
    /// Published head height used in the challenge.
    pub work_block: u64,
    /// Canonical work-block hash used in the challenge.
    pub block_hash: H256,
    /// Hotkey receiving the registration.
    pub hotkey: AccountId,
    /// Coldkey that must sign the registration intent.
    pub coldkey: AccountId,
    /// Unsigned nonce hashed in little-endian order.
    pub nonce: u64,
    /// Runtime seal returned and reverified by the core.
    pub work: [u8; 32],
}

/// Assemble the 123-byte runtime prefix from public chain identities.
pub fn registration_prefix(
    netuid: NetUid,
    block: H256,
    hotkey: AccountId,
    coldkey: AccountId,
) -> Result<[u8; rtao_core::pow::PREFIX_LEN], PowError> {
    if netuid.0 == 0 {
        return Err(PowError::RootSubnet);
    }
    let mut bytes = Vec::with_capacity(rtao_core::pow::PREFIX_LEN);
    bytes.extend_from_slice(DOMAIN);
    bytes.extend_from_slice(&netuid.0.to_le_bytes());
    bytes.extend_from_slice(&block.0);
    bytes.extend_from_slice(&hotkey.0);
    bytes.extend_from_slice(&coldkey.0);
    bytes
        .try_into()
        .map_err(|_| PowError::Core("runtime registration prefix has an invalid length".into()))
}

/// Clamp difficulty to the team's floor and the owner's ceiling.
pub fn effective_difficulty(value: u64, minimum: u64, maximum: u64) -> u64 {
    let minimum = minimum.max(1);
    value.max(minimum).min(maximum.max(minimum))
}

/// Mines bounded core batches on dedicated threads and retains no workers after
/// return, cancellation, timeout or a dropped solve future.
///
/// Cancellation completes after the running 1,024-attempt core batches finish.
/// Workers are joined on cleanup, so no detached blocking task survives timeout.
#[derive(Clone, Debug)]
pub struct PowSolver {
    config: PowConfig,
}

impl PowSolver {
    /// Validate the requested worker count and total deadline.
    pub fn new(config: PowConfig) -> Result<Self, PowError> {
        if !(1..=32).contains(&config.workers)
            || config.timeout.is_zero()
            || config.timeout > Duration::from_secs(3600)
        {
            return Err(PowError::InvalidConfig);
        }
        Ok(Self { config })
    }

    /// Return the validated CPU settings.
    pub fn config(&self) -> PowConfig {
        self.config
    }

    /// Mine against the client's published best chain using public AccountId32
    /// identities. Admission and winning difficulty reads use pinned snapshots.
    pub async fn solve(
        &self,
        client: &Client,
        netuid: NetUid,
        hotkey: AccountId,
        coldkey: AccountId,
        cancellation: &CancellationToken,
    ) -> Result<PowProof, PowError> {
        self.solve_with(
            client,
            netuid,
            hotkey,
            coldkey,
            cancellation,
            Arc::new(rtao_core::pow::mine_cpu),
        )
        .await
    }

    async fn solve_with(
        &self,
        client: &Client,
        netuid: NetUid,
        hotkey: AccountId,
        coldkey: AccountId,
        cancellation: &CancellationToken,
        miner: Miner,
    ) -> Result<PowProof, PowError> {
        if netuid.0 == 0 {
            return Err(PowError::RootSubnet);
        }
        let deadline = Instant::now() + self.config.timeout;
        tokio::select! {
            biased;
            () = cancellation.cancelled() => Err(PowError::Cancelled),
            () = tokio::time::sleep_until(deadline) => Err(PowError::Timeout),
            result = self.search(client, netuid, hotkey, coldkey, deadline, miner) => {
                if cancellation.is_cancelled() { Err(PowError::Cancelled) }
                else if Instant::now() >= deadline { Err(PowError::Timeout) }
                else { result }
            },
        }
    }

    async fn search(
        &self,
        client: &Client,
        netuid: NetUid,
        hotkey: AccountId,
        coldkey: AccountId,
        deadline: Instant,
        miner: Miner,
    ) -> Result<PowProof, PowError> {
        let mut heads = HeadProvider::subscribe(client.rpc(), rtao_client::Finality::Best).await?;
        let mut pool = None;
        let mut nonce_start = 0u64;
        loop {
            let snapshot = client.at(BlockRef::Best).await?;
            let work_block = snapshot.block().number;
            if work_block == 0 {
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            }
            let state = admission(&snapshot, netuid, hotkey).await?;
            if state.used >= state.limit || state.last.is_some_and(|last| work_block <= last) {
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            }
            let block_hash = snapshot.block().hash;
            let prefix = Arc::new(registration_prefix(netuid, block_hash, hotkey, coldkey)?);
            if pool.is_none() {
                pool = Some(CpuWorkers::new(self.config.workers, miner.clone())?);
            }
            let workers = pool
                .as_mut()
                .ok_or_else(|| PowError::Worker("CPU worker pool was not initialized".into()))?;
            let refresh = Instant::now() + CHALLENGE_REFRESH;
            'challenge: loop {
                let mut remaining =
                    workers.dispatch(prefix.clone(), state.difficulty, nonce_start)?;
                nonce_start =
                    nonce_start.wrapping_add(u64::from(remaining) * u64::from(workers::ATTEMPTS));
                while remaining > 0 {
                    let result = tokio::select! {
                        biased;
                        () = tokio::time::sleep_until(refresh) => { workers.drain(remaining).await?; break 'challenge; },
                        head = poll_fn(|cx| heads.as_mut().poll_next(cx)) => {
                            let header = head.ok_or_else(|| PowError::Worker("best-head subscription closed".into()))??;
                            if header.number < work_block || (header.number == work_block && header.hash != block_hash) || header.number.saturating_sub(work_block) >= MAX_WORK_AGE_BLOCKS - 1 {
                                workers.drain(remaining).await?;
                                break 'challenge;
                            }
                            continue;
                        },
                        result = workers.result() => result?,
                    };
                    remaining -= 1;
                    let Some((nonce, work)) = result.map_err(PowError::Core)? else {
                        continue;
                    };
                    if rtao_core::pow::seal(prefix.as_ref(), nonce).map_err(PowError::Core)? != work
                    {
                        return Err(PowError::InvalidProof);
                    }
                    let current = client.at(BlockRef::Best).await?;
                    let current_state = admission(&current, netuid, hotkey).await?;
                    if current.block().number < work_block
                        || current.block().number.saturating_sub(work_block) >= MAX_WORK_AGE_BLOCKS
                        || client.rpc().block_hash(work_block).await? != Some(block_hash)
                        || current_state.used >= current_state.limit
                        || current_state.last.is_some_and(|last| work_block <= last)
                    {
                        workers.drain(remaining).await?;
                        break 'challenge;
                    }
                    // A one-attempt core mine checks the current target and rehashes
                    // on CPU with exactly the runtime's byte definitions.
                    if rtao_core::pow::mine_cpu(prefix.as_ref(), current_state.difficulty, nonce, 1)
                        .map_err(PowError::Core)?
                        != Some((nonce, work))
                    {
                        workers.drain(remaining).await?;
                        break 'challenge;
                    }
                    // A newer head can make the pinned admission state obsolete
                    // while its RPC runs, even when the work hash is canonical.
                    let latest = client.at(BlockRef::Best).await?;
                    if latest.block().hash != current.block().hash {
                        workers.drain(remaining).await?;
                        break 'challenge;
                    }
                    // Reads and validation are part of the deadline, even if a
                    // ready future wins the surrounding select on this poll.
                    if Instant::now() >= deadline {
                        return Err(PowError::Timeout);
                    }
                    return Ok(PowProof {
                        netuid,
                        work_block,
                        block_hash,
                        hotkey,
                        coldkey,
                        nonce,
                        work,
                    });
                }
                tokio::task::yield_now().await;
            }
        }
    }
}

struct Admission {
    difficulty: u64,
    used: u16,
    limit: u16,
    last: Option<u64>,
}

async fn admission(
    snapshot: &Snapshot,
    netuid: NetUid,
    hotkey: AccountId,
) -> Result<Admission, PowError> {
    let (allowed, difficulty, minimum, maximum, uid, last, used, limit) = snapshot
        .fetch((
            storage::NETWORK_POW_REGISTRATION_ALLOWED.at((netuid,)),
            storage::DIFFICULTY.at((netuid,)),
            storage::MIN_DIFFICULTY.at((netuid,)),
            storage::MAX_DIFFICULTY.at((netuid,)),
            storage::UIDS.at((netuid, hotkey)),
            storage::LAST_POW_REGISTRATION_BLOCK.at((hotkey,)),
            storage::REGISTRATIONS_THIS_BLOCK.at((netuid,)),
            storage::MAX_REGISTRATIONS_PER_BLOCK.at((netuid,)),
        ))
        .await?;
    if !allowed {
        return Err(PowError::Disabled);
    }
    if uid.is_some() {
        return Err(PowError::AlreadyRegistered);
    }
    if limit == 0 {
        return Err(PowError::ZeroRegistrationLimit);
    }
    Ok(Admission {
        difficulty: effective_difficulty(difficulty, minimum, maximum),
        used,
        limit,
        last,
    })
}

/// Submitted work without the public challenge identities supplied by the caller.
#[cfg(feature = "registration")]
pub(crate) struct RegistrationWork {
    pub(crate) block: u64,
    pub(crate) nonce: u64,
    pub(crate) seal: [u8; 32],
}

/// Reuse the solver's admission and core challenge definitions before composing
/// a call. The published head itself is usable work: dispatch runs in the next
/// block, whose age limit is inclusive at five. Python likewise refreshes at
/// published-head age five, leaving one block for inclusion.
#[cfg(feature = "registration")]
pub(crate) async fn validate_registration(
    snapshot: &Snapshot,
    netuid: NetUid,
    hotkey: AccountId,
    coldkey: AccountId,
    work: &RegistrationWork,
) -> Result<(), PowError> {
    if netuid.0 == 0 {
        return Err(PowError::RootSubnet);
    }
    let (exists, max_uids) = snapshot
        .fetch((
            storage::NETWORKS_ADDED.at((netuid,)),
            storage::MAX_ALLOWED_UIDS.at((netuid,)),
        ))
        .await?;
    if !exists {
        return Err(PowError::SubnetNotExists);
    }
    let state = admission(snapshot, netuid, hotkey).await?;
    if max_uids == 0 {
        return Err(PowError::NoNeuronIdAvailable);
    }
    if state.used >= state.limit {
        return Err(PowError::RegistrationLimitReached);
    }
    let current = snapshot.block().number;
    if current < work.block
        || current.saturating_sub(work.block) >= MAX_WORK_AGE_BLOCKS
        || state.last.is_some_and(|last| work.block <= last)
    {
        return Err(PowError::InvalidWorkBlock);
    }
    let block_hash = snapshot
        .client()
        .rpc()
        .block_hash(work.block)
        .await?
        .filter(|hash| hash.0 != [0; 32])
        .ok_or(PowError::InvalidWorkBlock)?;
    let prefix = registration_prefix(netuid, block_hash, hotkey, coldkey)?;
    if rtao_core::pow::seal(&prefix, work.nonce).map_err(PowError::Core)? != work.seal {
        return Err(PowError::InvalidSeal);
    }
    if rtao_core::pow::mine_cpu(&prefix, state.difficulty, work.nonce, 1).map_err(PowError::Core)?
        != Some((work.nonce, work.seal))
    {
        return Err(PowError::InvalidDifficulty);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
