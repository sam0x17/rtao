// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/null_pruning.py and pallets/subtensor/src/subnets/uids.rs @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Finalized, restartable Null pruning with a durable unresolved-submission record.

use super::{OwnerOperation, OwnerOps};
use rtao_client::{BlockRef, Client, Inclusion, Snapshot, View, ViewExt};
use rtao_core::{codec::Value, value_ext::ValueExt};
use rtao_intents::{TrimNullSubnetBatch, hyperparameters::HyperparamPlanner};
use rtao_primitives::{AccountId, H256, NetUid};
use rtao_runtime::storage::{subtensor_module as st, system};
use rtao_signer::{Coldkey, KeySource};
use rtao_tx::{
    BuildCx, Built, ChainError, ExtrinsicResult, Intent, IntentError, IntentInterface, PlanOptions,
    PolicyError, PreflightCx, TxError,
};
use serde::Serialize;
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    time::Duration,
};

/// Null pruning completion observed in finalized storage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrimReport {
    /// Subnet whose participants were pruned.
    pub netuid: NetUid,
    /// Requested final population ceiling.
    pub target: u16,
    /// Finalized population at completion.
    pub population: u16,
    /// Successful bounded batches submitted by this invocation.
    pub batches: u32,
    /// UID removals reported by those finalized batches.
    pub removed_uids: u32,
    /// Weight commitments cancelled by those finalized batches.
    pub cleared_commits: u64,
    /// Last batch's inclusion identity, absent when already complete.
    pub last_extrinsic_id: Option<String>,
    /// The subnet already uses Yuma, so no Null pruning is performed.
    /// Its population may remain above the requested target within Yuma's ceiling.
    pub already_yuma: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Consensus {
    Yuma,
    Null,
}

#[derive(Clone, Debug)]
struct PruningState {
    block: u64,
    genesis: H256,
    owner: AccountId,
    population: u16,
    pending_target: Option<u16>,
    mechanisms: u8,
    minimum: u16,
    maximum: u16,
    consensus: Consensus,
}

impl PruningState {
    fn yuma_ceiling(&self) -> u16 {
        256 / u16::from(self.mechanisms)
    }

    fn validate(&self, initial: &Self, owner: AccountId, target: u16) -> Result<(), TxError> {
        if self.genesis != initial.genesis {
            return Err(invalid("RPC chain changed; stopping Null pruning"));
        }
        if self.owner != owner {
            return Err(TxError::Chain(ChainError::new(
                "signer is not the subnet owner",
                Some("BadOrigin".into()),
            )));
        }
        if self.mechanisms != initial.mechanisms {
            return Err(invalid(
                "mechanism count changed; restart to validate the new Yuma ceiling",
            ));
        }
        if self.pending_target.is_some_and(|pending| pending != target) {
            return Err(invalid("conflicting on-chain Null pruning target"));
        }
        if target < self.minimum || target > self.maximum.min(self.yuma_ceiling()) {
            return Err(invalid(format!(
                "Null pruning target must be between {} and {}",
                self.minimum,
                self.maximum.min(self.yuma_ceiling())
            )));
        }
        if self.consensus == Consensus::Yuma && self.population > self.yuma_ceiling() {
            return Err(invalid(
                "Yuma population exceeds its ceiling; Null batches cannot prune it",
            ));
        }
        Ok(())
    }

    fn complete(&self, target: u16) -> bool {
        self.pending_target.is_none()
            && (self.population <= target || self.consensus == Consensus::Yuma)
    }
}

async fn read_state(at: &Snapshot, netuid: NetUid) -> Result<PruningState, TxError> {
    let (exists, owner, population, pending_target, mechanisms, minimum, maximum, mode, genesis) =
        at.fetch((
            st::NETWORKS_ADDED.at((netuid,)),
            st::SUBNET_OWNER.at((netuid,)),
            st::SUBNETWORK_N.at((netuid,)),
            st::NULL_PRUNING_TARGET.at((netuid,)),
            st::MECHANISM_COUNT_CURRENT.at((netuid,)),
            st::MIN_ALLOWED_UIDS.at((netuid,)),
            st::MAX_ALLOWED_UIDS.at((netuid,)),
            st::SUBNET_EPOCH_CONSENSUS.at((netuid,)),
            system::BLOCK_HASH.at((0u32,)),
        ))
        .await?;
    if !exists {
        return Err(invalid(format!("subnet {netuid} does not exist")));
    }
    let owner = owner
        .get()
        .copied()
        .ok_or_else(|| invalid("subnet has no owner"))?;
    if !(1..=16).contains(&mechanisms.0) {
        return Err(invalid(
            "invalid mechanism count; cannot derive the Yuma ceiling",
        ));
    }
    let consensus = match mode.as_core() {
        Value::Str(mode) if mode == "Yuma" => Consensus::Yuma,
        Value::Str(mode) if mode == "Null" => Consensus::Null,
        _ => return Err(invalid("unsupported subnet epoch consensus")),
    };
    if genesis != at.client().genesis() {
        return Err(invalid("RPC genesis differs from the connected chain"));
    }
    Ok(PruningState {
        block: at.block().number,
        genesis,
        owner,
        population,
        pending_target,
        mechanisms: mechanisms.0,
        minimum,
        maximum,
        consensus,
    })
}

impl<K: KeySource<Coldkey>> OwnerOps<K> {
    /// Prune a Null subnet in transactions removing at most 64 UIDs each.
    ///
    /// Every successful batch is checked against finalized storage. Matching
    /// on-chain targets resume the first batch's cooldown; each batch still
    /// respects the admin freeze window. Survivor UIDs can change.
    ///
    /// `recovery` records an unresolved submission before any key is unlocked.
    /// A leftover record blocks another submission, including after a restart.
    /// Reconcile its inclusion or expiry before removing it. A sibling `.lock`
    /// file remains to keep advisory locking stable across invocations.
    pub(crate) async fn trim_null_inner(
        &self,
        operation: &OwnerOperation<'_, K>,
        netuid: NetUid,
        target: u16,
        recovery: &Path,
    ) -> Result<TrimReport, TxError> {
        if netuid.0 == 0 {
            return Err(invalid("root cannot select Null consensus"));
        }
        if self.options.shielded {
            return Err(invalid(
                "Null pruning requires direct finalized batch receipts",
            ));
        }
        operation.read(async { Ok(()) }).await?;
        let journal = SubmissionJournal::locked(recovery)?;
        journal.check()?;
        let owner = operation.read(async { self.effective_owner() }).await?;
        let initial_at = operation
            .read(async { Ok(self.client().at(BlockRef::Finalized).await?) })
            .await?;
        let initial = operation.read(read_state(&initial_at, netuid)).await?;
        initial.validate(&initial, owner, target)?;
        let planner = HyperparamPlanner::new(owner);
        let mut report = TrimReport {
            netuid,
            target,
            population: initial.population,
            batches: 0,
            removed_uids: 0,
            cleared_commits: 0,
            last_extrinsic_id: None,
            already_yuma: false,
        };
        let mut stalled = 0u8;
        loop {
            let at = operation
                .read(async { Ok(self.client().at(BlockRef::Finalized).await?) })
                .await?;
            let before = operation.read(read_state(&at, netuid)).await?;
            before.validate(&initial, owner, target)?;
            report.population = before.population;
            if before.complete(target) {
                report.already_yuma = before.consensus == Consensus::Yuma;
                return Ok(report);
            }
            if before.consensus != Consensus::Null {
                return Err(invalid(
                    "pruning target remains pending outside Null consensus",
                ));
            }
            if report.batches >= self.config.trim_max_batches {
                return Err(invalid(
                    "Null pruning batch limit reached; rerun to resume from finalized state",
                ));
            }
            let (_, reasons) = operation
                .read(planner.operation_window(
                    &at,
                    netuid,
                    "sudo_trim_null_uids_batch",
                    Some(target),
                ))
                .await?;
            if !reasons.is_empty() {
                operation
                    .read(async {
                        tokio::time::sleep(Duration::from_millis(250)).await;
                        Ok(())
                    })
                    .await?;
                continue;
            }
            // Best-state preflight must not overwrite another owner's target
            // while its finalized state is still catching up.
            let best_at = operation
                .read(async { Ok(self.client().at(BlockRef::Best).await?) })
                .await?;
            let best = operation.read(read_state(&best_at, netuid)).await?;
            best.validate(&initial, owner, target)?;
            if best_at.block().hash != at.block().hash
                && (best.population != before.population
                    || best.pending_target != before.pending_target
                    || best.consensus != before.consensus)
            {
                operation
                    .read(async {
                        tokio::time::sleep(Duration::from_millis(250)).await;
                        Ok(())
                    })
                    .await?;
                continue;
            }
            let intent = ResumingBatch {
                batch: TrimNullSubnetBatch { netuid, target },
                initial: initial.clone(),
                before: before.clone(),
                owner,
            };
            let plan = operation
                .read(self.executor.plan(
                    &self.routed(&intent),
                    self.keys.as_ref(),
                    &PlanOptions {
                        policy: self.options.policy.clone(),
                        proxy_for: self.options.proxy_for,
                        force_proxy_type: self.options.force_proxy_type,
                        deadline: self.options.deadline,
                    },
                ))
                .await?;
            if !plan.ok() {
                return Err(TxError::Policy(PolicyError {
                    violations: plan.violations,
                }));
            }
            journal.begin(&before, netuid, target, plan.call().as_bytes())?;
            let mut options = self.options.clone();
            options.wait_for_finalization = true;
            options.wait_for_inclusion = true;
            options.retries = 0;
            options.replan_owner_rate_limit = false;
            let receipt = operation.execute(&intent, &options).await?;
            if let Some(hash) = receipt.fate_unknown {
                return Err(TxError::Rpc(rtao_rpc::RpcError::FateUnknown {
                    hash: rtao_rpc::H256(hash.0),
                }));
            }
            if receipt.inclusion != Inclusion::Finalized
                || receipt.block_hash.is_none()
                || receipt.extrinsic_id.as_deref().is_none_or(str::is_empty)
            {
                return Err(invalid(format!(
                    "Null pruning delivery is unresolved; reconcile {} before restarting",
                    journal.path.display()
                )));
            }
            let after = operation
                .read(finalized_after(
                    self.client(),
                    &receipt,
                    netuid,
                    before.block,
                ))
                .await?;
            if !receipt.success {
                journal.finish()?;
                let error = receipt
                    .error
                    .unwrap_or_else(|| ChainError::new(receipt.message, None));
                if error.name.as_deref() == Some("AdminActionProhibitedDuringWeightsWindow") {
                    operation
                        .read(async {
                            tokio::time::sleep(Duration::from_millis(250)).await;
                            Ok(())
                        })
                        .await?;
                    continue;
                }
                return Err(TxError::Chain(error));
            }
            let progress = pruning_progress(&receipt, netuid, target)?;
            after.validate(&initial, owner, target)?;
            validate_progress(&before, &after, &progress, target)?;
            journal.finish()?;
            report.batches += 1;
            report.population = after.population;
            report.removed_uids += u32::from(progress.removed);
            report.cleared_commits += u64::from(progress.cleared_commits);
            report.last_extrinsic_id = receipt.extrinsic_id;
            stalled = if after.population >= before.population
                && progress.cleared_commits == 0
                && !after.complete(target)
            {
                stalled + 1
            } else {
                0
            };
            if stalled >= 3 {
                return Err(invalid(
                    "three finalized Null pruning batches made no progress; stopping",
                ));
            }
        }
    }
}

#[derive(Clone)]
struct ResumingBatch {
    batch: TrimNullSubnetBatch,
    initial: PruningState,
    before: PruningState,
    owner: AccountId,
}

impl IntentInterface for ResumingBatch {
    const SPEC: &'static rtao_catalog::IntentSpec = TrimNullSubnetBatch::SPEC;
    type Role = Coldkey;
    type Origin = rtao_tx::SubnetOwner;
    type Output = ();
    const WRAPS: &'static [&'static rtao_runtime::CallDescriptor] = TrimNullSubnetBatch::WRAPS;
    fn to_json(&self) -> serde_json::Value {
        self.batch.to_json()
    }
}

impl Intent for ResumingBatch {
    fn summary(&self) -> String {
        self.batch.summary()
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        self.batch.build(cx).await
    }

    async fn blocks<V: View + ?Sized>(
        &self,
        cx: &PreflightCx<'_, V>,
    ) -> Result<Vec<String>, IntentError> {
        let at = cx.view.pin().await?;
        let state = read_state(&at, self.batch.netuid)
            .await
            .map_err(|error| IntentError::InvalidArgument(error.to_string()))?;
        state
            .validate(&self.initial, self.owner, self.batch.target)
            .map_err(|error| IntentError::InvalidArgument(error.to_string()))?;
        if state.population != self.before.population
            || state.pending_target != self.before.pending_target
            || state.consensus != Consensus::Null
        {
            return Ok(vec![
                "Null pruning state changed before signing; restart from finalized state".into(),
            ]);
        }
        self.batch.blocks(cx).await
    }
}

struct PruningProgress {
    remaining: u16,
    removed: u16,
    cleared_commits: u32,
}

fn pruning_progress(
    receipt: &ExtrinsicResult,
    netuid: NetUid,
    target: u16,
) -> Result<PruningProgress, TxError> {
    let mut events = receipt
        .events
        .iter()
        .filter(|event| event.matches("SubtensorModule", "NullUidsPruningProgress"));
    let event = events
        .next()
        .ok_or_else(|| invalid("finalized receipt has no Null pruning progress event"))?;
    if events.next().is_some() {
        return Err(invalid(
            "finalized receipt has multiple Null pruning progress events",
        ));
    }
    let field = |name| -> Result<u32, TxError> {
        event
            .attributes
            .field(name)
            .and_then(ValueExt::as_u128)
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(|| invalid(format!("invalid Null pruning progress field {name}")))
    };
    if field("netuid")? != u32::from(netuid.0) || field("target")? != u32::from(target) {
        return Err(invalid(
            "receipt pruning target does not match this workflow",
        ));
    }
    Ok(PruningProgress {
        remaining: u16::try_from(field("remaining")?)
            .map_err(|_| invalid("invalid remaining population"))?,
        removed: u16::try_from(field("removed")?)
            .map_err(|_| invalid("invalid removed UID count"))?,
        cleared_commits: field("cleared_commits")?,
    })
}

fn validate_progress(
    before: &PruningState,
    after: &PruningState,
    progress: &PruningProgress,
    target: u16,
) -> Result<(), TxError> {
    if after.consensus != Consensus::Null
        || after.population != progress.remaining
        || progress.removed > 64
        || before.population.checked_sub(progress.removed) != Some(after.population)
        || (after.population > target && after.pending_target != Some(target))
        || (progress.removed > 0 && after.population < target)
    {
        return Err(invalid(
            "finalized storage does not match the bounded Null pruning progress",
        ));
    }
    Ok(())
}

async fn finalized_after(
    client: &Client,
    receipt: &ExtrinsicResult,
    netuid: NetUid,
    previous: u64,
) -> Result<PruningState, TxError> {
    let hash = receipt
        .block_hash
        .ok_or_else(|| invalid("missing finalized pruning block"))?;
    let included = client.at(BlockRef::Hash(hash)).await?;
    if included.block().number <= previous {
        return Err(invalid(
            "Null pruning receipt did not advance finalized state",
        ));
    }
    let finalized = client.at(BlockRef::Finalized).await?;
    if finalized.block().number < included.block().number {
        return Err(invalid(
            "Null pruning receipt is ahead of finalized storage",
        ));
    }
    let canonical = client.at(BlockRef::Number(included.block().number)).await?;
    if canonical.block().hash != hash {
        return Err(invalid(
            "Null pruning receipt is outside finalized canonical history",
        ));
    }
    read_state(&included, netuid).await
}

#[derive(Serialize)]
struct RecoveryRecord<'a> {
    version: u8,
    operation: &'static str,
    genesis: H256,
    owner: AccountId,
    netuid: NetUid,
    target: u16,
    block: u64,
    population: u16,
    call_hex: &'a str,
}

struct SubmissionJournal {
    path: PathBuf,
    _lock: File,
}

impl SubmissionJournal {
    fn locked(path: &Path) -> Result<Self, TxError> {
        let filename = path
            .file_name()
            .ok_or_else(|| invalid("Null pruning recovery path must name a file"))?;
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent).map_err(|e| journal_error(path, e))?;
        let parent = fs::canonicalize(parent).map_err(|e| journal_error(path, e))?;
        let path = parent.join(filename);
        let mut lock_name = filename.to_os_string();
        lock_name.push(".lock");
        let lock_path = parent.join(lock_name);
        ensure_regular_or_absent(&path)?;
        ensure_regular_or_absent(&lock_path)?;
        let mut options = OpenOptions::new();
        options.create(true).read(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let lock = options
            .open(&lock_path)
            .map_err(|e| journal_error(&lock_path, e))?;
        lock.try_lock().map_err(|e| {
            invalid(format!(
                "cannot lock Null pruning recovery {}: {e}",
                lock_path.display()
            ))
        })?;
        Ok(Self { path, _lock: lock })
    }

    fn check(&self) -> Result<(), TxError> {
        match fs::symlink_metadata(&self.path) {
            Ok(_) => Err(invalid(format!(
                "unresolved submission recorded in {}; reconcile its inclusion or expiry before removing the record",
                self.path.display()
            ))),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(journal_error(&self.path, error)),
        }
    }

    fn begin(
        &self,
        state: &PruningState,
        netuid: NetUid,
        target: u16,
        call: &[u8],
    ) -> Result<(), TxError> {
        let call_hex = format!("0x{}", hex::encode(call));
        let record = RecoveryRecord {
            version: 1,
            operation: "trim_null_subnet_batch",
            genesis: state.genesis,
            owner: state.owner,
            netuid,
            target,
            block: state.block,
            population: state.population,
            call_hex: &call_hex,
        };
        let encoded = serde_json::to_vec(&record).map_err(IntentError::from)?;
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&self.path)
            .map_err(|e| journal_error(&self.path, e))?;
        file.write_all(&encoded)
            .and_then(|()| file.write_all(b"\n"))
            .and_then(|()| file.sync_all())
            .map_err(|e| journal_error(&self.path, e))?;
        self.sync_directory()
    }

    fn finish(&self) -> Result<(), TxError> {
        fs::remove_file(&self.path).map_err(|e| journal_error(&self.path, e))?;
        self.sync_directory()
    }

    fn sync_directory(&self) -> Result<(), TxError> {
        #[cfg(unix)]
        {
            let parent = self
                .path
                .parent()
                .ok_or_else(|| invalid("recovery path has no parent"))?;
            File::open(parent)
                .and_then(|file| file.sync_all())
                .map_err(|e| journal_error(parent, e))?;
        }
        Ok(())
    }
}

fn ensure_regular_or_absent(path: &Path) -> Result<(), TxError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => Ok(()),
        Ok(_) => Err(invalid(format!(
            "Null pruning recovery path must be a regular file: {}",
            path.display()
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(journal_error(path, error)),
    }
}

fn journal_error(path: &Path, error: impl std::fmt::Display) -> TxError {
    invalid(format!("Null pruning recovery {}: {error}", path.display()))
}

fn invalid(message: impl Into<String>) -> TxError {
    IntentError::InvalidArgument(message.into()).into()
}

#[cfg(test)]
mod tests;
