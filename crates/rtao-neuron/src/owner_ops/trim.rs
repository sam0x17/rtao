// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/null_pruning.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor pallets/subtensor/src/subnets/uids.rs @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Finalized, restartable Null pruning with a durable unresolved-submission record.

use super::{OwnerOperation, OwnerOps};
use rtao_client::{BlockRef, Client, Inclusion, Snapshot, View, ViewExt};
use rtao_core::{DynValue, codec::Value, value_ext::ValueExt};
use rtao_intents::{SetHyperparameter, TrimNullSubnetBatch, hyperparameters::HyperparamPlanner};
use rtao_primitives::{AccountId, H256, NetUid};
use rtao_runtime::{
    models::MultisigTimepoint,
    storage::{subtensor_module as st, system},
};
use rtao_signer::{
    BoxFuture, Coldkey, KeySource, KeyView, MultiSignature, Signer, SignerError, SigningRequest,
};
use rtao_tx::{
    BuildCx, Built, ChainError, DynIntent, ExtrinsicResult, Intent, IntentError, IntentInterface,
    Plan, PlanOptions, PolicyError, PreflightCx, TxError,
};
use serde_json::{Value as Json, json};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
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
    /// This invocation finalized the explicitly requested switch to Yuma.
    pub switched_to_yuma: bool,
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
        if self.consensus == Consensus::Null
            && (target < self.minimum || target > self.maximum.min(self.yuma_ceiling()))
        {
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

/// Bounds and optional behavior for a shared SDK/CLI pruning session.
#[derive(Clone, Debug)]
pub struct TrimNullConfig {
    /// Plan only; no broadcast journal may be created.
    pub preview: bool,
    /// Explicitly switch to Yuma after population and queues are ready.
    pub switch_to_yuma: bool,
    /// Maximum finalized pruning batches in this invocation.
    pub max_batches: u32,
    /// Delay between read-only window checks.
    pub poll_interval: Duration,
    /// Wait budget starting at the first wait, cleared after each accepted batch.
    pub wait_timeout: Duration,
}

impl Default for TrimNullConfig {
    fn default() -> Self {
        Self {
            preview: false,
            switch_to_yuma: false,
            max_batches: 128,
            poll_interval: Duration::from_millis(250),
            wait_timeout: Duration::from_secs(3600),
        }
    }
}

/// One decision made from pinned chain state; never submits by itself.
pub enum TrimNullStep {
    /// The requested finalized state is complete.
    Complete(TrimReport),
    /// Recheck state after a bounded read-only wait.
    Wait {
        /// Why signing is currently prohibited.
        reason: String,
        /// Suggested polling delay.
        delay: Duration,
    },
    /// Route this guarded semantic action through normal transaction review.
    Submit(TrimNullAction),
}

/// Opaque guarded operation retained across review, journaling and execution.
#[derive(Clone)]
pub struct TrimNullAction {
    kind: Arc<TrimActionKind>,
}

#[derive(Clone)]
enum TrimActionKind {
    Batch(ResumingBatch),
    Switch(GuardedYuma),
}

impl TrimNullAction {
    /// Guarded semantic intent to pass through SDK or CLI routing.
    pub fn intent(&self) -> &dyn DynIntent {
        match self.kind.as_ref() {
            TrimActionKind::Batch(intent) => intent,
            TrimActionKind::Switch(intent) => intent,
        }
    }

    /// Semantic operation name, independent of proxy or multisig wrappers.
    pub fn operation(&self) -> &'static str {
        match self.kind.as_ref() {
            TrimActionKind::Batch(_) => "trim_null_subnet_batch",
            TrimActionKind::Switch(_) => "set_hyperparameter",
        }
    }

    fn before(&self) -> &PruningState {
        match self.kind.as_ref() {
            TrimActionKind::Batch(intent) => &intent.before,
            TrimActionKind::Switch(intent) => &intent.before,
        }
    }
}

/// Durable Null pruning state machine shared by SDK and CLI drivers.
///
/// Read-only methods may be bounded or cancelled. Submission belongs to the
/// driver, which must drain its transaction future and record known receipts.
pub struct TrimNullSession {
    client: Client,
    owner: AccountId,
    initial: PruningState,
    config: TrimNullConfig,
    report: TrimReport,
    journal: SubmissionJournal,
    stalled: u8,
    wait_deadline: Option<tokio::time::Instant>,
    retry_window: Option<String>,
}

impl TrimNullSession {
    /// Lock recovery state, validate finalized prerequisites and choose a target.
    pub async fn begin(
        client: Client,
        owner: AccountId,
        netuid: NetUid,
        target: Option<u16>,
        recovery: &Path,
        config: TrimNullConfig,
    ) -> Result<Self, TxError> {
        if netuid.0 == 0 {
            return Err(invalid("root cannot select Null consensus"));
        }
        if config.max_batches == 0
            || config.poll_interval.is_zero()
            || config.wait_timeout.is_zero()
        {
            return Err(invalid(
                "Null pruning limits and polling interval must be positive",
            ));
        }
        let journal = SubmissionJournal::locked(recovery)?;
        journal.check_resume(&client).await?;
        let at = client.at(BlockRef::Finalized).await?;
        let initial = read_state(&at, netuid).await?;
        let target = target
            .or(initial.pending_target)
            .unwrap_or_else(|| initial.yuma_ceiling());
        initial.validate(&initial, owner, target)?;
        journal.check_identity(&initial, netuid, target)?;
        let report = TrimReport {
            netuid,
            target,
            population: initial.population,
            batches: 0,
            removed_uids: 0,
            cleared_commits: 0,
            last_extrinsic_id: None,
            already_yuma: false,
            switched_to_yuma: false,
        };
        Ok(Self {
            client,
            owner,
            initial,
            config,
            report,
            journal,
            stalled: 0,
            wait_deadline: None,
            retry_window: None,
        })
    }

    /// Decide one guarded action from finalized state and a consistent best head.
    pub async fn next_action(&mut self) -> Result<TrimNullStep, TxError> {
        if let Some(reason) = self.retry_window.take() {
            return Ok(self.wait_step(reason));
        }
        let netuid = self.report.netuid;
        let target = self.report.target;
        let at = self.client.at(BlockRef::Finalized).await?;
        let before = read_state(&at, netuid).await?;
        before.validate(&self.initial, self.owner, target)?;
        self.report.population = before.population;
        let queues = if self.config.switch_to_yuma
            && before.consensus == Consensus::Null
            && before.complete(target)
        {
            queued_commits(&at, netuid).await?
        } else {
            false
        };
        if before.complete(target)
            && (!self.config.switch_to_yuma || before.consensus == Consensus::Yuma)
        {
            self.journal.check()?;
            self.report.already_yuma =
                before.consensus == Consensus::Yuma && !self.report.switched_to_yuma;
            return Ok(TrimNullStep::Complete(self.report.clone()));
        }
        if before.consensus != Consensus::Null {
            return Err(invalid(
                "pruning target remains pending outside Null consensus",
            ));
        }
        let best_at = self.client.at(BlockRef::Best).await?;
        let best = read_state(&best_at, netuid).await?;
        best.validate(&self.initial, self.owner, target)?;
        if !self.config.preview
            && best_at.block().hash != at.block().hash
            && (best.population != before.population
                || best.pending_target != before.pending_target
                || best.consensus != before.consensus)
        {
            return Ok(self.wait_step("best pruning state is not finalized yet".into()));
        }
        let planner = HyperparamPlanner::new(self.owner);
        if self.config.switch_to_yuma && before.complete(target) && !queues {
            let plan = planner
                .plan(&at, netuid, "epoch_consensus", "Yuma".into())
                .await?;
            if !self.config.preview && !plan.reasons.is_empty() {
                return Ok(self.wait_step(plan.reasons.join("; ")));
            }
            return Ok(TrimNullStep::Submit(TrimNullAction {
                kind: Arc::new(TrimActionKind::Switch(GuardedYuma {
                    setter: SetHyperparameter {
                        netuid,
                        name: "epoch_consensus".into(),
                        value: json!("Yuma"),
                    },
                    target,
                    initial: self.initial.clone(),
                    before,
                    owner: self.owner,
                })),
            }));
        }
        if self.report.batches >= self.config.max_batches {
            return Err(invalid(
                "Null pruning batch limit reached; rerun to resume from finalized state",
            ));
        }
        let (_, reasons) = planner
            .operation_window(&at, netuid, "sudo_trim_null_uids_batch", Some(target))
            .await?;
        if !self.config.preview && !reasons.is_empty() {
            return Ok(self.wait_step(reasons.join("; ")));
        }
        // Even at the target population, pending queues require a bounded cleanup.
        Ok(TrimNullStep::Submit(TrimNullAction {
            kind: Arc::new(TrimActionKind::Batch(ResumingBatch {
                batch: TrimNullSubnetBatch { netuid, target },
                initial: self.initial.clone(),
                before,
                owner: self.owner,
            })),
        }))
    }

    fn wait_step(&self, reason: String) -> TrimNullStep {
        TrimNullStep::Wait {
            reason,
            delay: self.config.poll_interval,
        }
    }

    /// Wait without a submission future, sharing a deadline until a batch is accepted.
    pub async fn wait(&mut self, reason: &str, delay: Duration) -> Result<(), TxError> {
        let now = tokio::time::Instant::now();
        let deadline = match self.wait_deadline {
            Some(deadline) => deadline,
            None => {
                let deadline = now
                    .checked_add(self.config.wait_timeout)
                    .ok_or_else(|| invalid("Null pruning wait deadline overflow"))?;
                self.wait_deadline = Some(deadline);
                deadline
            }
        };
        if now >= deadline {
            return Err(invalid(format!("Null pruning wait timed out: {reason}")));
        }
        // Like Python's final partial sleep, return to the driver to reread state.
        // Only another required wait at or after the deadline is a timeout.
        tokio::time::sleep(delay.min(self.config.poll_interval).min(deadline - now)).await;
        Ok(())
    }

    /// Persist the exact reviewed outer bytes before signer acquisition.
    pub fn before_attempt(&self, action: &TrimNullAction, plan: &Plan) -> Result<(), TxError> {
        if self.config.preview {
            return Err(invalid(
                "preview cannot start a Null pruning submission record",
            ));
        }
        if !plan.ok() {
            return Err(TxError::Policy(PolicyError {
                violations: plan.violations.clone(),
            }));
        }
        self.validate_action(action)?;
        self.journal
            .before_attempt(action, self.report.netuid, self.report.target, plan)
    }

    fn validate_action(&self, action: &TrimNullAction) -> Result<(), TxError> {
        action
            .before()
            .validate(&self.initial, self.owner, self.report.target)?;
        let matches = match action.kind.as_ref() {
            TrimActionKind::Batch(intent) => {
                intent.batch.netuid == self.report.netuid
                    && intent.batch.target == self.report.target
            }
            TrimActionKind::Switch(intent) => {
                intent.setter.netuid == self.report.netuid && intent.target == self.report.target
            }
        };
        if !matches {
            return Err(invalid(
                "guarded action belongs to a different pruning session",
            ));
        }
        Ok(())
    }

    /// Durably attach all known receipt evidence, including raw event records.
    /// Repeating this after follow-up enrichment is supported for the same attempt.
    pub fn record_result(&self, receipt: &ExtrinsicResult) -> Result<(), TxError> {
        self.journal.record_result(receipt)
    }

    /// Persist a reviewed receipt and confirm a finalized pending approval.
    /// This permits another approval of the same opening before the driver
    /// returns from a multi-round submission, without counting pruning progress.
    pub async fn record_reviewed_result(
        &self,
        action: &TrimNullAction,
        receipt: &ExtrinsicResult,
    ) -> Result<(), TxError> {
        self.journal.record_result(receipt)?;
        self.validate_action(action)?;
        if receipt.success
            && receipt.fate_unknown.is_none()
            && receipt.inclusion == Inclusion::Finalized
            && receipt.block_hash.is_some()
            && receipt
                .extrinsic_id
                .as_deref()
                .is_some_and(|id| !id.is_empty())
            && self.journal.is_pending_multisig(receipt)?
        {
            let after = finalized_after(
                &self.client,
                receipt,
                self.report.netuid,
                action.before().block,
            )
            .await?;
            self.confirm_pending(action, receipt, &after).await?;
        }
        Ok(())
    }

    async fn confirm_pending(
        &self,
        action: &TrimNullAction,
        receipt: &ExtrinsicResult,
        after: &PruningState,
    ) -> Result<(), TxError> {
        after.validate(&self.initial, self.owner, self.report.target)?;
        let before = action.before();
        if after.population != before.population
            || after.pending_target != before.pending_target
            || after.consensus != before.consensus
        {
            return Err(invalid(
                "pruning state changed during a pending approval; retain recovery journal",
            ));
        }
        self.journal.confirm_pending(&self.client, receipt).await
    }

    /// Confirm canonical finality, semantic execution and exact storage progress.
    /// Incomplete multisig approvals retain the journal and never advance batches.
    pub async fn finish_action(
        &mut self,
        action: TrimNullAction,
        receipt: &ExtrinsicResult,
    ) -> Result<(), TxError> {
        self.journal.record_result(receipt)?;
        self.validate_action(&action)?;
        if let Some(hash) = receipt.fate_unknown {
            return Err(TxError::Rpc(rtao_rpc::RpcError::FateUnknown {
                hash: rtao_rpc::H256(hash.0),
            }));
        }
        if receipt.inclusion != Inclusion::Finalized
            || receipt.block_hash.is_none()
            || receipt.extrinsic_id.as_deref().is_none_or(str::is_empty)
        {
            return Err(self.journal.unresolved());
        }
        let before = action.before().clone();
        let after =
            finalized_after(&self.client, receipt, self.report.netuid, before.block).await?;
        if receipt.success && self.journal.is_pending_multisig(receipt)? {
            self.confirm_pending(&action, receipt, &after).await?;
            return Err(invalid(format!(
                "multisig approval finalized without semantic execution; retain {} and collect the remaining approvals for its exact call and timepoint",
                self.journal.path.display()
            )));
        }
        if !self.journal.read()?["multisig"].is_null()
            && !self.journal.is_pending_multisig(receipt)?
        {
            self.journal.ensure_consumed(&self.client, receipt).await?;
        }
        if !receipt.success {
            // A failed approval may leave the original pending operation alive.
            // Retain its earlier receipts and opening timepoint until resolution.
            let multisig = !self.journal.read()?["multisig"].is_null();
            if multisig && self.journal.is_pending_multisig(receipt)? {
                return Err(invalid(format!(
                    "failed multisig approval has no confirmed semantic execution; retain {} and reconcile its pending call",
                    self.journal.path.display()
                )));
            }
            self.journal.finish()?;
            let error = receipt
                .error
                .clone()
                .unwrap_or_else(|| ChainError::new(receipt.message.clone(), None));
            if !multisig
                && error.name.as_deref() == Some("AdminActionProhibitedDuringWeightsWindow")
            {
                self.retry_window = Some(error.message);
                return Ok(());
            }
            return Err(TxError::Chain(error));
        }
        after.validate(&self.initial, self.owner, self.report.target)?;
        match action.kind.as_ref() {
            TrimActionKind::Batch(_) => {
                let progress = pruning_progress(receipt, self.report.netuid, self.report.target)?;
                validate_progress(&before, &after, &progress, self.report.target)?;
                self.journal.finish()?;
                self.report.batches += 1;
                self.report.population = after.population;
                self.report.removed_uids += u32::from(progress.removed);
                self.report.cleared_commits += u64::from(progress.cleared_commits);
                let changed = after.population < before.population
                    || progress.cleared_commits != 0
                    || after.complete(self.report.target);
                self.stalled = if changed { 0 } else { self.stalled + 1 };
                if self.stalled >= 3 {
                    return Err(invalid(
                        "three finalized Null pruning batches made no progress; stopping",
                    ));
                }
            }
            TrimActionKind::Switch(_) => {
                if after.consensus != Consensus::Yuma
                    || after.population > self.report.target
                    || after.pending_target.is_some()
                {
                    return Err(invalid(
                        "finalized storage does not match the requested Yuma switch",
                    ));
                }
                let at = self
                    .client
                    .at_receipt(
                        receipt
                            .block_hash
                            .ok_or_else(|| invalid("missing switch inclusion"))?,
                    )
                    .await?;
                if queued_commits(&at, self.report.netuid).await? {
                    return Err(invalid(
                        "finalized Yuma switch still has queued commits; retain recovery evidence",
                    ));
                }
                self.journal.finish()?;
                self.report.switched_to_yuma = true;
            }
        }
        self.wait_deadline = None;
        self.report.last_extrinsic_id = receipt.extrinsic_id.clone();
        Ok(())
    }

    /// Bounded finalized progress accumulated by this invocation.
    pub fn report(&self) -> &TrimReport {
        &self.report
    }
}

impl<K: KeySource<Coldkey>> OwnerOps<K> {
    /// SDK driver over the same durable session used by CLI review and routing.
    pub(crate) async fn trim_null_inner(
        &self,
        operation: &OwnerOperation<'_, K>,
        netuid: NetUid,
        target: u16,
        recovery: &Path,
    ) -> Result<TrimReport, TxError> {
        if self.options.shielded {
            return Err(invalid(
                "Null pruning requires direct finalized batch receipts",
            ));
        }
        let owner = operation.read(async { self.effective_owner() }).await?;
        let mut session = operation
            .read(TrimNullSession::begin(
                self.client().clone(),
                owner,
                netuid,
                Some(target),
                recovery,
                TrimNullConfig {
                    max_batches: self.config.trim_max_batches,
                    ..Default::default()
                },
            ))
            .await?;
        loop {
            match operation.read(session.next_action()).await? {
                TrimNullStep::Complete(report) => return Ok(report),
                TrimNullStep::Wait { reason, delay } => {
                    operation.read(session.wait(&reason, delay)).await?
                }
                TrimNullStep::Submit(action) => {
                    let opts = PlanOptions {
                        policy: self.options.policy.clone(),
                        proxy_for: self.options.proxy_for,
                        force_proxy_type: self.options.force_proxy_type,
                        deadline: self.options.deadline,
                    };
                    let plan = match action.kind.as_ref() {
                        TrimActionKind::Batch(intent) => {
                            operation
                                .read(self.executor.plan(
                                    &self.routed(intent),
                                    self.keys.as_ref(),
                                    &opts,
                                ))
                                .await?
                        }
                        TrimActionKind::Switch(intent) => {
                            operation
                                .read(self.executor.plan(
                                    &self.routed(intent),
                                    self.keys.as_ref(),
                                    &opts,
                                ))
                                .await?
                        }
                    };
                    session.before_attempt(&action, &plan)?;
                    let mut options = self.options.clone();
                    options.wait_for_finalization = true;
                    options.wait_for_inclusion = true;
                    options.retries = 0;
                    options.replan_owner_rate_limit = false;
                    // Submission owns delivery; never wrap this in read/select/timeout.
                    let receipt = match action.kind.as_ref() {
                        TrimActionKind::Batch(intent) => {
                            operation
                                .execute_trim_reviewed(intent, &options, &plan)
                                .await?
                        }
                        TrimActionKind::Switch(intent) => {
                            operation
                                .execute_trim_reviewed(intent, &options, &plan)
                                .await?
                        }
                    };
                    session.record_result(&receipt)?;
                    operation
                        .read(session.finish_action(action, &receipt))
                        .await?;
                }
            }
        }
    }
}

/// Bind the SDK executor's internal replan to the already journaled outer call.
/// The signer refuses changed bytes or runtime versions before producing a signature.
struct ReviewedKeys<'a, K> {
    keys: &'a K,
    call: rtao_runtime::CallBytes,
    genesis: H256,
}

impl<K: KeyView> KeyView for ReviewedKeys<'_, K> {
    fn hotkey_address(&self) -> Result<AccountId, SignerError> {
        self.keys.hotkey_address()
    }
    fn hotkey_public_key(&self) -> Result<[u8; 32], SignerError> {
        self.keys.hotkey_public_key()
    }
    fn coldkey_address(&self) -> Result<AccountId, SignerError> {
        self.keys.coldkey_address()
    }
    fn bare_signer_account(&self) -> Option<AccountId> {
        self.keys.bare_signer_account()
    }
}

impl<K: KeySource<Coldkey>> KeySource<Coldkey> for ReviewedKeys<'_, K> {
    fn address(&self) -> Result<AccountId, SignerError> {
        self.keys.address()
    }
    async fn signer(&self) -> Result<Arc<dyn Signer>, SignerError> {
        Ok(Arc::new(ReviewedSigner {
            inner: self.keys.signer().await?,
            call: self.call.clone(),
            genesis: self.genesis,
        }))
    }
}

struct ReviewedSigner {
    inner: Arc<dyn Signer>,
    call: rtao_runtime::CallBytes,
    genesis: H256,
}

impl Signer for ReviewedSigner {
    fn account(&self) -> AccountId {
        self.inner.account()
    }
    fn crypto_type(&self) -> rtao_primitives::CryptoType {
        self.inner.crypto_type()
    }
    fn caps(&self) -> rtao_signer::SignerCaps {
        self.inner.caps()
    }
    fn sign<'a>(
        &'a self,
        request: &'a SigningRequest<'a>,
    ) -> BoxFuture<'a, Result<MultiSignature, SignerError>> {
        Box::pin(async move {
            let spec = request.payload_json.get("specVersion").and_then(|value| {
                value.as_u64().or_else(|| {
                    let text = value.as_str()?;
                    match text.strip_prefix("0x") {
                        Some(hex) => u64::from_str_radix(hex, 16).ok(),
                        None => text.parse().ok(),
                    }
                })
            });
            if request.call != self.call.as_bytes()
                || spec != Some(u64::from(self.call.spec_version()))
                || request.context.chain_info.spec_version != self.call.spec_version()
                || request.context.transaction_version != self.call.transaction_version()
                || request.context.genesis_hash != self.genesis.0
                || request.purpose != rtao_signer::SignPurpose::Extrinsic
            {
                return Err(SignerError::InvalidSignature("Null pruning signing call or runtime differs from durable review; retain recovery journal".into()));
            }
            self.inner.sign(request).await
        })
    }
}

impl<K: KeySource<Coldkey>> OwnerOperation<'_, K> {
    async fn execute_trim_reviewed<I: Intent<Role = Coldkey, Output = ()> + Clone>(
        &self,
        intent: &I,
        options: &rtao_tx::ExecuteOptions,
        plan: &Plan,
    ) -> Result<ExtrinsicResult, TxError> {
        self.check()?;
        let mut options = options.clone();
        options.expected_call = Some(plan.call().clone());
        options.deadline = self
            .deadline
            .map(|d| d.saturating_duration_since(tokio::time::Instant::now()));
        let keys = ReviewedKeys {
            keys: self.owner.keys.as_ref(),
            call: plan.call().clone(),
            genesis: self.owner.client().genesis(),
        };
        // As with OwnerOperation::execute, let Client own attempted delivery.
        self.owner
            .executor
            .execute(&self.owner.routed(intent), &keys, &options)
            .await
    }
}

#[derive(Clone)]
struct GuardedYuma {
    setter: SetHyperparameter,
    target: u16,
    initial: PruningState,
    before: PruningState,
    owner: AccountId,
}

impl IntentInterface for GuardedYuma {
    const SPEC: &'static rtao_catalog::IntentSpec = SetHyperparameter::SPEC;
    type Role = Coldkey;
    type Origin = rtao_tx::SubnetOwner;
    type Output = ();
    const WRAPS: &'static [&'static rtao_runtime::CallDescriptor] = SetHyperparameter::WRAPS;
    fn to_json(&self) -> Json {
        self.setter.to_json()
    }
}

impl Intent for GuardedYuma {
    fn summary(&self) -> String {
        self.setter.summary()
    }
    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        self.setter.build(cx).await
    }
    async fn blocks<V: View + ?Sized>(
        &self,
        cx: &PreflightCx<'_, V>,
    ) -> Result<Vec<String>, IntentError> {
        let at = cx.view.pin().await?;
        let validation = async {
            let state = read_state(&at, self.setter.netuid).await?;
            state.validate(&self.initial, self.owner, self.target)?;
            if state.consensus != Consensus::Null
                || state.population != self.before.population
                || !state.complete(self.target)
                || queued_commits(&at, self.setter.netuid).await?
            {
                return Err(invalid("Yuma switch prerequisites changed before signing"));
            }
            Ok::<_, TxError>(())
        }
        .await;
        validation.map_err(|error| IntentError::InvalidArgument(error.to_string()))?;
        self.setter.blocks(cx).await
    }
}

/// Probe at most one key per queue for every legal legacy mechanism index.
async fn queued_commits(at: &Snapshot, netuid: NetUid) -> Result<bool, TxError> {
    let runtime = &at.block().runtime;
    let mut queued = false;
    for mechanism in 0..16u16 {
        let index = u32::from(netuid.0) + u32::from(mechanism) * 4096;
        let index =
            u16::try_from(index).map_err(|_| invalid("invalid subnet mechanism storage index"))?;
        for name in ["WeightCommits", "TimelockedWeightCommits"] {
            let info = runtime
                .storage_entry("SubtensorModule", name)
                .ok_or_else(|| invalid(format!("missing queue metadata {name}")))?;
            let prefix = runtime
                .core()
                .storage_key(info, &[Value::Uint(u128::from(index))])
                .map_err(rtao_core::CoreError::from)
                .map_err(IntentError::from)?;
            queued |= !at.keys_paged(&prefix, 1, None).await?.is_empty();
        }
    }
    Ok(queued)
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
    let (included, finalized, canonical) = finalized_receipt(client, hash).await?;
    if included.block().number <= previous {
        return Err(invalid(
            "Null pruning receipt did not advance finalized state",
        ));
    }
    if finalized.block().number < included.block().number {
        return Err(invalid(
            "Null pruning receipt is ahead of finalized storage",
        ));
    }
    if canonical.block().hash != hash {
        return Err(invalid(
            "Null pruning receipt is outside finalized canonical history",
        ));
    }
    read_state(&included, netuid).await
}

/// A receipt's inclusion block with the finalized head and the canonical block
/// at its height, all read from one endpoint.
///
/// That is the read endpoint when it has finalized the block. Otherwise, with a
/// separate submission endpoint, it is the endpoint that reported the receipt:
/// the read endpoint may still lag behind it.
async fn finalized_receipt(
    client: &Client,
    hash: H256,
) -> Result<(Snapshot, Snapshot, Snapshot), TxError> {
    let mut included = client.at_receipt(hash).await?;
    let mut chain = included.client().clone();
    let mut finalized = chain.at(BlockRef::Finalized).await?;
    if finalized.block().number < included.block().number && client.has_submit_endpoint() {
        chain = client.receipt_client().await?;
        included = chain.at(BlockRef::Hash(hash)).await?;
        finalized = chain.at(BlockRef::Finalized).await?;
    }
    let canonical = chain.at(BlockRef::Number(included.block().number)).await?;
    Ok((included, finalized, canonical))
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

    fn unresolved(&self) -> TxError {
        invalid(format!(
            "unresolved submission recorded in {}; reconcile its inclusion or expiry before removing the record",
            self.path.display()
        ))
    }

    fn read(&self) -> Result<Json, TxError> {
        ensure_regular_or_absent(&self.path)?;
        let bytes = rtao_wallet::read_keyfile_data(&self.path)
            .map_err(|e| invalid(format!("cannot read Null pruning journal: {e}")))?;
        if bytes.len() > 2 * 1024 * 1024 {
            return Err(invalid(
                "Null pruning journal exceeds its two MiB recovery bound",
            ));
        }
        serde_json::from_slice(&bytes).map_err(|e| journal_error(&self.path, e))
    }

    fn exists(&self) -> Result<bool, TxError> {
        ensure_regular_or_absent(&self.path)?;
        match fs::symlink_metadata(&self.path) {
            Ok(_) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(journal_error(&self.path, e)),
        }
    }

    fn write(&self, record: &Json, first: bool) -> Result<(), TxError> {
        let encoded = serde_json::to_vec(record).map_err(IntentError::from)?;
        if encoded.len() > 2 * 1024 * 1024 {
            return Err(invalid(
                "Null pruning journal exceeds its two MiB recovery bound; retain existing evidence",
            ));
        }
        ensure_regular_or_absent(&self.path)?;
        let mut next_name = self
            .path
            .file_name()
            .ok_or_else(|| self.unresolved())?
            .to_os_string();
        next_name.push(".next");
        let next_path = self.path.with_file_name(next_name);
        let destination = if first { &self.path } else { &next_path };
        ensure_regular_or_absent(destination)?;
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(destination)
            .map_err(|e| journal_error(destination, e))?;
        let written = file
            .write_all(&encoded)
            .and_then(|()| file.write_all(b"\n"))
            .and_then(|()| file.sync_all());
        if let Err(error) = written {
            if !first {
                let _ = fs::remove_file(&next_path);
            }
            return Err(journal_error(destination, error));
        }
        drop(file);
        if !first {
            // Old evidence stays intact until the complete new record is synced.
            // A leftover .next after a crash blocks rather than overwrites it.
            if let Err(error) = fs::rename(&next_path, &self.path) {
                let _ = fs::remove_file(&next_path);
                return Err(journal_error(&self.path, error));
            }
        }
        self.sync_directory()
    }

    fn check_identity(
        &self,
        state: &PruningState,
        netuid: NetUid,
        target: u16,
    ) -> Result<(), TxError> {
        if !self.exists()? {
            return Ok(());
        }
        let record = self.read()?;
        if record["version"] != json!(2)
            || record["genesis"] != json!(state.genesis)
            || record["owner"] != json!(state.owner)
            || record["netuid"] != json!(netuid)
            || record["target"] != json!(target)
            || record["population"] != json!(state.population)
            || record["mechanisms"] != json!(state.mechanisms)
            || record["pending_target"] != json!(state.pending_target)
        {
            return Err(invalid(
                "retained Null pruning approval does not match this chain, owner, target and pruning state",
            ));
        }
        Ok(())
    }

    async fn check_resume(&self, client: &Client) -> Result<(), TxError> {
        if !self.exists()? {
            return Ok(());
        }
        let record = self.read().map_err(|_| self.unresolved())?;
        if record["approval_confirmed"] != json!(true)
            || record["receipt"]["success"] != json!(true)
            || record["receipt"]["inclusion"] != json!("Finalized")
            || !record["receipt"]["fate_unknown"].is_null()
        {
            return Err(self.unresolved());
        }
        let hash = receipt_block_hash(&record["receipt"]["block_hash"])
            .ok_or_else(|| self.unresolved())?;
        let (included, finalized, canonical) = finalized_receipt(client, hash).await?;
        if finalized.block().number < included.block().number || canonical.block().hash != hash {
            return Err(self.unresolved());
        }
        let (account, call_hash) = multisig_identity(&record)?;
        let (opening, signer) = self.approval_opening(&record, included.block().number)?;
        for at in [&included, &finalized] {
            let pending = at
                .storage(
                    &rtao_runtime::storage::multisig::MULTISIGS,
                    (account, call_hash),
                )
                .await?;
            let pending = pending.as_ref().ok_or_else(|| self.unresolved())?;
            if pending.when != opening
                || json!(opening) != record["pending_timepoint"]
                || !pending.approvals.contains(&signer)
            {
                return Err(self.unresolved());
            }
        }
        Ok(())
    }

    fn before_attempt(
        &self,
        action: &TrimNullAction,
        netuid: NetUid,
        target: u16,
        plan: &Plan,
    ) -> Result<(), TxError> {
        let state = action.before();
        let semantic = action.intent().to_json_dyn();
        let multisig = multisig_review(plan)?;
        let adapter = !multisig.is_null() || plan.op == "multisig_threshold_1";
        let mut reviewed_semantic = if adapter {
            plan.args["call"].clone()
        } else {
            plan.args.clone()
        };
        if !adapter {
            let args = reviewed_semantic
                .as_object_mut()
                .ok_or_else(|| invalid("reviewed pruning arguments must be an object"))?;
            args.insert("op".into(), json!(plan.op));
        }
        if reviewed_semantic != semantic {
            return Err(invalid(
                "reviewed plan does not contain this guarded pruning operation",
            ));
        }
        let call_hex = format!("0x{}", hex::encode(plan.call().as_bytes()));
        let attempt = json!({
            "call_hex": call_hex, "signer": plan.signer_address,
            "spec_version": plan.call().spec_version(), "transaction_version": plan.call().transaction_version(),
            "reviewed_op": plan.op, "reviewed_args": plan.args,
        });
        if !self.exists()? {
            let pending_timepoint = if multisig.is_null() {
                Json::Null
            } else {
                plan.args["timepoint"].clone()
            };
            let record = json!({
                "version":2, "operation":action.operation(), "semantic_args":semantic,
                "genesis":state.genesis, "owner":state.owner, "netuid":netuid, "target":target,
                "block":state.block, "population":state.population, "pending_target":state.pending_target,
                "mechanisms":state.mechanisms, "call_hex":call_hex, "multisig":multisig,
                "pending_timepoint":pending_timepoint,
                "approval_confirmed":false, "attempts":[attempt],
            });
            return self.write(&record, true);
        }
        self.check_identity(state, netuid, target)?;
        let mut record = self.read()?;
        let first = record["attempts"]
            .as_array()
            .and_then(|a| a.first())
            .ok_or_else(|| self.unresolved())?;
        if record["approval_confirmed"] != json!(true)
            || multisig.is_null()
            || record["multisig"] != multisig
            || record["semantic_args"] != semantic
            || record["operation"] != json!(action.operation())
            || record["pending_timepoint"].is_null()
            || plan.args["timepoint"] != record["pending_timepoint"]
            || first["spec_version"] != json!(plan.call().spec_version())
            || first["transaction_version"] != json!(plan.call().transaction_version())
        {
            return Err(invalid(
                "retained pruning approval requires the identical multisig group, inner call and non-null opening timepoint",
            ));
        }
        let attempts = record["attempts"]
            .as_array_mut()
            .ok_or_else(|| self.unresolved())?;
        let maximum_attempts = multisig["signatories"]
            .as_array()
            .ok_or_else(|| self.unresolved())?
            .len();
        if attempts.len() >= maximum_attempts {
            return Err(invalid(
                "Null pruning approval-attempt limit reached; retain recovery journal",
            ));
        }
        attempts.push(attempt);
        record["approval_confirmed"] = json!(false);
        record["call_hex"] = json!(call_hex);
        record
            .as_object_mut()
            .ok_or_else(|| self.unresolved())?
            .remove("receipt");
        self.write(&record, false)
    }

    fn record_result(&self, receipt: &ExtrinsicResult) -> Result<(), TxError> {
        let mut record = self.read()?;
        let mut observed = receipt.to_json();
        observed["inclusion"] = json!(match receipt.inclusion {
            Inclusion::Unobserved => "Unobserved",
            Inclusion::PoolAccepted => "PoolAccepted",
            Inclusion::InBlock => "InBlock",
            Inclusion::Finalized => "Finalized",
        });
        observed["fate_unknown"] = json!(receipt.fate_unknown);
        observed["events"] = Json::Array(
            receipt
                .events
                .iter()
                .map(|event| DynValue::new(event.value().clone()).to_json())
                .collect(),
        );
        let attempts = record["attempts"]
            .as_array_mut()
            .ok_or_else(|| self.unresolved())?;
        let attempt = attempts.last_mut().ok_or_else(|| self.unresolved())?;
        if let Some(previous) = attempt.get("receipt") {
            for field in ["block_hash", "extrinsic_id"] {
                if !previous[field].is_null() && previous[field] != observed[field] {
                    return Err(invalid(
                        "receipt identity changed for a retained Null pruning attempt",
                    ));
                }
            }
        } else {
            attempt["initial_receipt"] = observed.clone();
        }
        attempt["receipt"] = observed.clone();
        record["receipt"] = observed;
        self.write(&record, false)
    }

    fn is_pending_multisig(&self, receipt: &ExtrinsicResult) -> Result<bool, TxError> {
        let record = self.read()?;
        if record["multisig"].is_null() {
            return Ok(false);
        }
        let (account, hash) = multisig_identity(&record)?;
        let mut executed = receipt.events.iter().filter(|event| {
            event.matches("Multisig", "MultisigExecuted")
                && event_account(event, "multisig") == Some(account)
                && event_hash(event, "call_hash") == Some(hash)
        });
        let Some(event) = executed.next() else {
            return Ok(true);
        };
        if executed.next().is_some() {
            return Err(invalid(
                "multiple matching multisig executions in pruning receipt",
            ));
        }
        let outcome = event
            .attributes
            .field("result")
            .and_then(ValueExt::variant_name);
        if outcome.as_deref() != Some(if receipt.success { "Ok" } else { "Err" }) {
            return Err(invalid(
                "multisig execution event differs from its classified pruning receipt",
            ));
        }
        Ok(false)
    }

    // Authenticate the opening from the retained receipt, never from later
    // block-end pending state. Recheck durable evidence on restart as well.
    // NewMultisig height inference assumes pinned v478's BlockNumberProvider
    // is System, as in the CLI's receipt-opening authentication (D-041).
    fn approval_opening(
        &self,
        record: &Json,
        included_height: u64,
    ) -> Result<(MultisigTimepoint, AccountId), TxError> {
        let receipt = &record["receipt"];
        if receipt["success"] != json!(true)
            || receipt["inclusion"] != json!("Finalized")
            || !receipt["fate_unknown"].is_null()
        {
            return Err(self.unresolved());
        }
        let position = receipt["extrinsic_id"]
            .as_str()
            .and_then(|id| {
                let (height, index) = id.split_once('-')?;
                Some(MultisigTimepoint {
                    height: height.parse().ok()?,
                    index: index.parse().ok()?,
                })
            })
            .ok_or_else(|| self.unresolved())?;
        if u64::from(position.height) != included_height {
            return Err(self.unresolved());
        }
        let (account, hash) = multisig_identity(record)?;
        let attempt = record["attempts"]
            .as_array()
            .and_then(|attempts| attempts.last())
            .ok_or_else(|| self.unresolved())?;
        let signer: AccountId =
            serde_json::from_value(attempt["signer"].clone()).map_err(|_| self.unresolved())?;
        let events = receipt["events"]
            .as_array()
            .ok_or_else(|| self.unresolved())?;
        let mut opening = None;
        for event in events {
            if event["module_id"] != json!("Multisig")
                || !matches!(
                    event["event_id"].as_str(),
                    Some("NewMultisig" | "MultisigApproval")
                )
            {
                continue;
            }
            let attributes = &event["attributes"];
            let event_account: AccountId = serde_json::from_value(attributes["multisig"].clone())
                .map_err(|_| self.unresolved())?;
            let approving: AccountId = serde_json::from_value(attributes["approving"].clone())
                .map_err(|_| self.unresolved())?;
            let event_hash =
                receipt_block_hash(&attributes["call_hash"]).ok_or_else(|| self.unresolved())?;
            if event_account != account || event_hash.0 != hash || approving != signer {
                continue;
            }
            if event["phase"] != json!("ApplyExtrinsic")
                || event["extrinsic_idx"] != json!(position.index)
                || opening.is_some()
            {
                return Err(self.unresolved());
            }
            let reviewed = &attempt["reviewed_args"]["timepoint"];
            let actual = if event["event_id"] == json!("NewMultisig") {
                if !reviewed.is_null() {
                    return Err(self.unresolved());
                }
                position.clone()
            } else {
                let actual: MultisigTimepoint =
                    serde_json::from_value(attributes["timepoint"].clone())
                        .map_err(|_| self.unresolved())?;
                if *reviewed != json!(actual) {
                    return Err(self.unresolved());
                }
                actual
            };
            if !record["pending_timepoint"].is_null()
                && record["pending_timepoint"] != json!(actual)
            {
                return Err(self.unresolved());
            }
            opening = Some(actual);
        }
        Ok((opening.ok_or_else(|| self.unresolved())?, signer))
    }

    async fn confirm_pending(
        &self,
        client: &Client,
        receipt: &ExtrinsicResult,
    ) -> Result<(), TxError> {
        let mut record = self.read()?;
        let (account, hash) = multisig_identity(&record)?;
        let included = client
            .at_receipt(receipt.block_hash.ok_or_else(|| self.unresolved())?)
            .await?;
        let (opening, signer) = self.approval_opening(&record, included.block().number)?;
        let pending = included
            .storage(&rtao_runtime::storage::multisig::MULTISIGS, (account, hash))
            .await?;
        let pending = pending.as_ref().ok_or_else(|| self.unresolved())?;
        if !pending.approvals.contains(&signer) || pending.when != opening {
            return Err(self.unresolved());
        }
        record["pending_timepoint"] = json!(opening);
        record["approval_confirmed"] = json!(true);
        self.write(&record, false)
    }

    async fn ensure_consumed(
        &self,
        client: &Client,
        receipt: &ExtrinsicResult,
    ) -> Result<(), TxError> {
        let record = self.read()?;
        let (account, hash) = multisig_identity(&record)?;
        let included = client
            .at_receipt(receipt.block_hash.ok_or_else(|| self.unresolved())?)
            .await?;
        if included
            .storage(&rtao_runtime::storage::multisig::MULTISIGS, (account, hash))
            .await?
            .is_some()
        {
            return Err(invalid(
                "finalized multisig execution still has a pending call; retain pruning recovery evidence",
            ));
        }
        Ok(())
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

fn multisig_review(plan: &Plan) -> Result<Json, TxError> {
    let Some(hash) = plan.extras.get("multisig_call_hash") else {
        if plan.op == "multisig_execute" {
            return Err(invalid(
                "multisig pruning review has no retained inner call hash",
            ));
        }
        return Ok(Json::Null);
    };
    let data = plan
        .extras
        .get("multisig_call_data")
        .ok_or_else(|| invalid("multisig pruning review has no retained inner bytes"))?;
    let threshold = plan.args["threshold"]
        .as_u64()
        .and_then(|n| u16::try_from(n).ok())
        .filter(|n| *n >= 2)
        .ok_or_else(|| invalid("invalid pruning multisig threshold"))?;
    let others: Vec<AccountId> = serde_json::from_value(plan.args["other_signatories"].clone())
        .map_err(IntentError::from)?;
    if others.len() >= 100
        || others.contains(&plan.signer_address)
        || others.windows(2).any(|p| p[0].0 >= p[1].0)
    {
        return Err(invalid("pruning multisig signer set is not canonical"));
    }
    let mut signatories = others
        .into_iter()
        .map(|account| account.0)
        .collect::<Vec<_>>();
    signatories.push(plan.signer_address.0);
    if usize::from(threshold) > signatories.len() {
        return Err(invalid("invalid pruning multisig threshold"));
    }
    let (address, sorted) = rtao_core::extrinsic::multisig_account_id(&signatories, threshold)
        .map_err(rtao_core::CoreError::from)
        .map_err(IntentError::from)?;
    let data_text = data
        .as_str()
        .ok_or_else(|| invalid("invalid retained multisig call data"))?;
    let bytes = hex::decode(data_text.strip_prefix("0x").unwrap_or(data_text))
        .map_err(|e| invalid(e.to_string()))?;
    use blake2::{Blake2b, Digest, digest::consts::U32};
    let actual_hash: [u8; 32] = Blake2b::<U32>::digest(&bytes).into();
    if *hash != json!(format!("0x{}", hex::encode(actual_hash))) {
        return Err(invalid(
            "retained multisig call hash does not match its bytes",
        ));
    }
    Ok(
        json!({"threshold":threshold, "signatories":sorted.into_iter().map(AccountId).collect::<Vec<_>>(),
        "address":AccountId(address), "call_hash":hash, "call_data":data}),
    )
}

fn multisig_identity(record: &Json) -> Result<(AccountId, [u8; 32]), TxError> {
    let account =
        serde_json::from_value(record["multisig"]["address"].clone()).map_err(IntentError::from)?;
    let hash = record["multisig"]["call_hash"]
        .as_str()
        .ok_or_else(|| invalid("retained multisig call hash is not hexadecimal text"))?;
    let hash = hex::decode(hash.strip_prefix("0x").unwrap_or(hash))
        .map_err(|e| invalid(e.to_string()))?
        .try_into()
        .map_err(|_| invalid("retained multisig call hash must have 32 bytes"))?;
    Ok((account, hash))
}

// Current Python-shaped results use hex; older journals used H256's raw
// serde array. Accept both exact encodings without relaxing finality checks.
fn receipt_block_hash(value: &Json) -> Option<H256> {
    let mut bytes = [0u8; 32];
    if let Some(text) = value.as_str() {
        let text = text.strip_prefix("0x")?;
        if text.len() != 64 {
            return None;
        }
        hex::decode_to_slice(text, &mut bytes).ok()?;
    } else {
        let values = value.as_array()?;
        if values.len() != bytes.len() {
            return None;
        }
        for (byte, value) in bytes.iter_mut().zip(values) {
            *byte = u8::try_from(value.as_u64()?).ok()?;
        }
    }
    Some(H256(bytes))
}

fn event_account(event: &rtao_client::EventRecord, name: &str) -> Option<AccountId> {
    event.attributes.field(name)?.as_str()?.parse().ok()
}

fn event_hash(event: &rtao_client::EventRecord, name: &str) -> Option<[u8; 32]> {
    let value = event.attributes.field(name)?;
    value
        .value_bytes()
        .and_then(|bytes| bytes.try_into().ok())
        .or_else(|| {
            let text = value.as_str()?;
            hex::decode(text.strip_prefix("0x").unwrap_or(text))
                .ok()?
                .try_into()
                .ok()
        })
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
