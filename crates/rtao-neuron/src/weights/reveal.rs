// Derived from RaoFoundation/subtensor pallets/subtensor/src/subnets/weights.rs @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Follow the included commit's actual epoch and retain reveal diagnostics.

use super::WeightPreflight;
use crate::Metrics;
use rtao_client::{BlockHeader, BlockRef, Client, Snapshot, ViewExt};
use rtao_core::value_ext::ValueExt;
use rtao_core::{
    codec::Value,
    timelock::epoch_schedule::{
        EpochScheduleState, current_epoch_pre_run_coinbase, predict_first_reveal_block,
        simulate_run_coinbase,
    },
};
use rtao_intents::weights::PreparedWeightCommit;
use rtao_metagraph::{MetagraphOptions, SubnetsMetagraphExt};
use rtao_primitives::{AccountId, MechId, NetUid, NetUidStorageIndex, Uid};
use rtao_reads::Namespaces;
use rtao_runtime::storage::subtensor_module as st;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// Chain epoch changes retained beside reveal outcomes for attribution.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum EpochNotice {
    /// Global epoch capacity delayed this subnet's processing.
    Deferred(u64),
    /// Inconsistent input skipped an epoch boundary.
    Skipped(u64),
    /// An owner requested an early epoch after commitment.
    Triggered(u64),
}

/// First observed reason a committed row could not reveal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RevealMissReason {
    /// The committing hotkey was deregistered.
    NotRegistered,
    /// The first reveal snapshot was below the minimum stake threshold.
    Stake,
    /// Owner forced a newer weight implementation before the reveal.
    VersionKey,
    /// The validator permit was absent when a nonself row revealed.
    Permit,
    /// A destination UID no longer existed.
    Uid,
    /// The row no longer met population or minimum-count checks.
    Length,
    /// Chain drand ingestion had not reached the encrypted round.
    DrandRound,
    /// An owner-triggered early epoch shortened the cryptographic window.
    EpochTriggered,
    /// The runtime emitted no success event despite all observed checks passing.
    Unknown,
}
impl RevealMissReason {
    /// Stable Prometheus label for the miss reason.
    pub const fn label(self) -> &'static str {
        match self {
            Self::NotRegistered => "not_registered",
            Self::Stake => "stake",
            Self::VersionKey => "version_key",
            Self::Permit => "permit",
            Self::Uid => "uid",
            Self::Length => "length",
            Self::DrandRound => "drand_round",
            Self::EpochTriggered => "epoch_triggered",
            Self::Unknown => "unknown",
        }
    }
}

/// A bounded, completed reveal outcome for operator inspection.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RevealOutcome {
    /// Mechanism whose row was committed.
    pub mechanism: MechId,
    /// Epoch that actually received the commit, rather than the prediction.
    pub commit_epoch: u64,
    /// Actual reveal epoch followed by the watcher.
    pub reveal_epoch: u64,
    /// Predicted first reveal block at preparation time.
    pub predicted_first_block: u64,
    /// Drand round embedded in the commitment.
    pub round: u64,
    /// Finalized block where success or epoch expiration was observed.
    pub observed_block: u64,
    /// No miss reason when a matching success event was observed.
    pub missed: Option<RevealMissReason>,
    /// The exact native queue entry was absent after its observed processing
    /// window, with no attributable success in complete replayed history.
    /// This does not claim the application operation was completed or replaced.
    pub native_queue_absent: bool,
    /// Relevant epoch events observed between inclusion and completion.
    pub epoch_notices: Vec<EpochNotice>,
}

/// Redacted pending-commit diagnostics, including unresolved event attribution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingReveal {
    /// Mechanism whose native commitment remains under observation.
    pub mechanism: MechId,
    /// Actual finalized inclusion height.
    pub inclusion_block: u64,
    /// Current observed reveal epoch, unknown until inclusion is located.
    pub reveal_epoch: Option<u64>,
    /// A finalized event could not uniquely identify this ciphertext. It stays
    /// unresolved even when its old queue disappears.
    pub attribution_unresolved: bool,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Pending {
    netuid: NetUid,
    mechanism: MechId,
    hotkey: AccountId,
    uid: Uid,
    uids: Vec<Uid>,
    version_key: u64,
    commit_epoch: u64,
    reveal_epoch: u64,
    predicted_first_block: u64,
    round: u64,
    ciphertext: Vec<u8>,
    inclusion_block: u64,
    first_check: Option<RevealMissReason>,
    first_checked: bool,
    first_reveal_hash: Option<rtao_primitives::H256>,
    last_round: u64,
    notices: Vec<EpochNotice>,
    warned_permit: bool,
    ambiguous_reveal: bool,
    /// Non-native tag this commitment carried, with the code it followed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    followed_cr: Option<FollowedCr>,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Unlocated {
    netuid: NetUid,
    mechanism: MechId,
    hotkey: AccountId,
    row: (Vec<Uid>, Vec<u16>),
    version_key: u64,
    state: RevealPreparation,
    #[serde(with = "super::durable::commit_codec")]
    commit: PreparedWeightCommit,
    report: super::submit::WeightSubmission,
}

/// A non-native `CommitRevealWeightsVersion` a commitment was tagged with,
/// and the runtime code under which the scheduler followed it (D-086).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FollowedCr {
    tag: u16,
    code: rtao_primitives::H256,
}

/// What one of this validator's reveals showed about one runtime code under
/// one stored commit-reveal tag (D-086).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RevealedCode {
    /// Payload format the revealed commitment used.
    payload_version: u16,
    /// `:code` storage hash at the reveal block's parent.
    code: rtao_primitives::H256,
    /// `CommitRevealWeightsVersion` stored at that parent. Evidence written
    /// before the tag was recorded has none and proves no tag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stored_tag: Option<u16>,
    /// A commitment tagged `stored_tag` under `code` missed its reveal with
    /// no drand, epoch or chain-check explanation. This refutes following
    /// that tag under that code, even for a reviewed runtime.
    #[serde(default, skip_serializing_if = "is_false")]
    missed: bool,
}
#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_false(value: &bool) -> bool {
    !*value
}

/// Most runtime code identities retained as reveal evidence.
const REVEALED_CODE_LIMIT: usize = 16;

/// Retains outstanding reveals and the last 256 completed outcomes.
#[derive(Default, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RevealWatcher {
    pending: Vec<Pending>,
    unlocated: Vec<Unlocated>,
    outcomes: VecDeque<RevealOutcome>,
    last_observed_block: Option<u64>,
    /// Absent from checkpoints written before D-086 and while empty.
    #[serde(default, skip_serializing_if = "VecDeque::is_empty")]
    revealed_code: VecDeque<RevealedCode>,
    /// Verified chain state at the observation cursor, which predicts the
    /// blocks after it (D-096). Never checkpointed: a restored watcher reads
    /// it again at its cursor.
    #[serde(skip)]
    anchor: Option<Anchor>,
}

/// Blocks a single [`RevealWatcher::observe`] call may read before it retains
/// its cursor for the next call.
const REVEAL_BLOCK_READS: usize = 256;

/// The stored values that decide one subnet's reveal processing, as the
/// chain holds them at the end of a block: the parent state of the next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct WatchState {
    tempo: u16,
    last_epoch_block: u64,
    pending_epoch_at: u64,
    subnet_epoch_index: u64,
    blocks_since_last_step: u64,
    reveal_period: u64,
}
impl WatchState {
    fn schedule(self, block: u64) -> EpochScheduleState {
        EpochScheduleState {
            tempo: self.tempo,
            last_epoch_block: self.last_epoch_block,
            pending_epoch_at: self.pending_epoch_at,
            subnet_epoch_index: self.subnet_epoch_index,
            blocks_since_last_step: self.blocks_since_last_step,
            current_block: block,
        }
    }
    /// The epoch reveal processing uses in `block`, whose parent holds `self`.
    fn epoch_at(self, block: u64) -> u64 {
        current_epoch_pre_run_coinbase(&self.schedule(block.saturating_sub(1)), block)
    }
    /// Whether an epoch is due in `block`: the only blocks that can carry an
    /// `EpochDeferred` or `EpochSkipped` event, advance the epoch index, or
    /// open or close a reveal window.
    fn epoch_due(self, block: u64) -> bool {
        self.epoch_at(block) != self.subnet_epoch_index
            || self.step(block).subnet_epoch_index != self.subnet_epoch_index
    }
    /// The state after `block` when its epoch, if due, runs successfully.
    fn step(self, block: u64) -> Self {
        let next = simulate_run_coinbase(&self.schedule(block.saturating_sub(1)), block);
        Self {
            tempo: next.tempo,
            last_epoch_block: next.last_epoch_block,
            pending_epoch_at: next.pending_epoch_at,
            subnet_epoch_index: next.subnet_epoch_index,
            blocks_since_last_step: next.blocks_since_last_step,
            reveal_period: self.reveal_period,
        }
    }
}

/// One commit-epoch queue a pending commitment is followed through.
type QueueKey = (NetUid, MechId, u64);
type CommitQueue = Vec<(AccountId, u64, Vec<u8>, u64)>;

/// Verified state at one finalized block: each pending subnet's schedule and
/// reveal period, and each queue whose change could settle a commitment.
#[derive(Clone, Debug)]
struct Anchor {
    snapshot: Snapshot,
    states: BTreeMap<NetUid, WatchState>,
    queues: BTreeMap<QueueKey, CommitQueue>,
}
impl Anchor {
    fn block(&self) -> u64 {
        self.snapshot.block().number
    }
    /// The states at `block` predicted from this anchor, if no owner trigger,
    /// deferral, skipped epoch or parameter change occurred in between.
    fn predict(&self, block: u64) -> BTreeMap<NetUid, WatchState> {
        self.states
            .iter()
            .map(|(netuid, state)| {
                let mut state = *state;
                for number in self.block().saturating_add(1)..=block {
                    state = state.step(number);
                }
                (*netuid, state)
            })
            .collect()
    }
    /// Whether `other`, read later, is what this anchor predicts: then no
    /// block in between could have changed a pending outcome.
    fn predicts(&self, other: &Self) -> bool {
        self.predict(other.block()) == other.states && self.queues == other.queues
    }
    /// The first block in `(self.block(), last]` that runs an epoch.
    fn next_epoch_block(&self, last: u64) -> Option<u64> {
        self.states
            .values()
            .filter_map(|state| {
                let mut state = *state;
                for number in self.block().saturating_add(1)..=last {
                    if state.epoch_due(number) {
                        return Some(number);
                    }
                    state = state.step(number);
                }
                None
            })
            .min()
    }
}
impl RevealWatcher {
    /// Whether runtime code `code` revealed one of this validator's commitments
    /// in the [`super::NATIVE_CR_PAYLOAD_VERSION`] payload while
    /// `CommitRevealWeightsVersion` stored `tag`, with no later refuting miss.
    ///
    /// The evidence is a success event bound to the exact ciphertext in the
    /// parent queue, under the parent block's `:code` and stored tag. Unreviewed
    /// code may decode differently by stored tag, so evidence under one tag
    /// never proves another.
    pub fn reveals_native_payload(&self, code: rtao_primitives::H256, tag: u16) -> bool {
        self.evidence(code, tag).is_some_and(|known| !known.missed)
    }
    /// Whether a commitment tagged `tag` under runtime code `code` last missed
    /// its reveal with no other explanation. The scheduler then pauses that
    /// tag under that code until a later reveal proves it again.
    pub fn native_reveal_refuted(&self, code: rtao_primitives::H256, tag: u16) -> bool {
        self.evidence(code, tag).is_some_and(|known| known.missed)
    }
    fn evidence(&self, code: rtao_primitives::H256, tag: u16) -> Option<&RevealedCode> {
        self.revealed_code.iter().find(|known| {
            known.code == code
                && known.stored_tag == Some(tag)
                && known.payload_version == super::NATIVE_CR_PAYLOAD_VERSION
        })
    }
    /// Retain evidence that `code` revealed the native payload under `tag`.
    pub(super) fn remember_revealed_code(&mut self, code: rtao_primitives::H256, tag: u16) {
        self.remember_reveal(code, tag, false);
    }
    /// Retain that a commitment tagged `tag` under `code` missed its reveal.
    pub(super) fn remember_missed_reveal(&mut self, code: rtao_primitives::H256, tag: u16) {
        self.remember_reveal(code, tag, true);
    }
    /// The latest outcome for a code and tag replaces the earlier one.
    fn remember_reveal(&mut self, code: rtao_primitives::H256, tag: u16, missed: bool) {
        self.revealed_code.retain(|known| {
            !(known.code == code
                && known.stored_tag == Some(tag)
                && known.payload_version == super::NATIVE_CR_PAYLOAD_VERSION)
        });
        if self.revealed_code.len() >= REVEALED_CODE_LIMIT {
            // Refutations outlive positive evidence: dropping one would
            // resume a tag whose reveals were seen failing.
            let oldest = self
                .revealed_code
                .iter()
                .position(|known| !known.missed)
                .unwrap_or(0);
            self.revealed_code.remove(oldest);
        }
        self.revealed_code.push_back(RevealedCode {
            payload_version: super::NATIVE_CR_PAYLOAD_VERSION,
            code,
            stored_tag: Some(tag),
            missed,
        });
    }
    /// Inspect pending native obligations without exposing ciphertext or keys.
    pub fn outstanding(&self) -> Vec<PendingReveal> {
        self.pending
            .iter()
            .map(|p| PendingReveal {
                mechanism: p.mechanism,
                inclusion_block: p.inclusion_block,
                reveal_epoch: Some(p.reveal_epoch),
                attribution_unresolved: p.ambiguous_reveal,
            })
            .chain(self.unlocated.iter().map(|p| PendingReveal {
                mechanism: p.mechanism,
                inclusion_block: p.report.block_number,
                reveal_epoch: None,
                attribution_unresolved: false,
            }))
            .collect()
    }
    /// Whether retained finalized replay reached the requested observation height.
    pub fn caught_up(&self, height: u64) -> bool {
        self.last_observed_block.is_some_and(|last| last >= height)
    }
    /// Number of included commitments still awaiting an outcome.
    pub fn pending(&self) -> usize {
        self.pending.len() + self.unlocated.len()
    }
    /// Included or temporarily unlocated commitments for one mechanism.
    pub fn pending_for(&self, mechanism: MechId) -> usize {
        self.pending
            .iter()
            .filter(|pending| pending.mechanism == mechanism)
            .count()
            + self
                .unlocated
                .iter()
                .filter(|pending| pending.mechanism == mechanism)
                .count()
    }
    /// Completed outcomes, oldest retained first.
    pub fn outcomes(&self) -> &VecDeque<RevealOutcome> {
        &self.outcomes
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn record(
        &mut self,
        client: &Client,
        netuid: NetUid,
        mechanism: MechId,
        hotkey: AccountId,
        row: (Vec<Uid>, Vec<u16>),
        version_key: u64,
        state: WeightPreflight,
        commit: PreparedWeightCommit,
        report: &super::submit::WeightSubmission,
        metrics: &Metrics,
    ) -> anyhow::Result<()> {
        self.record_prepared(
            client,
            netuid,
            mechanism,
            hotkey,
            row,
            version_key,
            RevealPreparation::from(&state),
            commit,
            report,
            metrics,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn record_prepared(
        &mut self,
        client: &Client,
        netuid: NetUid,
        mechanism: MechId,
        hotkey: AccountId,
        row: (Vec<Uid>, Vec<u16>),
        version_key: u64,
        state: RevealPreparation,
        commit: PreparedWeightCommit,
        report: &super::submit::WeightSubmission,
        metrics: &Metrics,
    ) -> anyhow::Result<()> {
        if self.pending.iter().any(|p| {
            p.netuid == netuid
                && p.mechanism == mechanism
                && p.hotkey == hotkey
                && p.inclusion_block == report.block_number
                && p.ciphertext == commit.ciphertext
                && p.round == commit.reveal_round
        }) {
            return Ok(());
        }
        self.unlocated.retain(|p| {
            !(p.netuid == netuid
                && p.mechanism == mechanism
                && p.hotkey == hotkey
                && p.report.block_hash == report.block_hash
                && p.commit == commit)
        });
        self.unlocated.push(Unlocated {
            netuid,
            mechanism,
            hotkey,
            row: row.clone(),
            version_key,
            state: state.clone(),
            commit: commit.clone(),
            report: report.clone(),
        });
        let result = self
            .try_record(
                client,
                netuid,
                mechanism,
                hotkey,
                row,
                version_key,
                state,
                commit,
                report,
                metrics,
            )
            .await;
        if result.is_ok() {
            self.unlocated.pop();
        }
        result
    }

    #[allow(clippy::too_many_arguments)]
    async fn try_record(
        &mut self,
        client: &Client,
        netuid: NetUid,
        mechanism: MechId,
        hotkey: AccountId,
        row: (Vec<Uid>, Vec<u16>),
        version_key: u64,
        state: RevealPreparation,
        commit: PreparedWeightCommit,
        report: &super::submit::WeightSubmission,
        metrics: &Metrics,
    ) -> anyhow::Result<()> {
        let hash = report.block_hash;
        let inclusion = report.block_number;
        let index = NetUidStorageIndex::new(netuid, mechanism)?;
        let assumed_inclusion = state
            .prepared_block
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("commit prediction height overflow"))?;
        let schedule = state.schedule();
        let after = simulate_run_coinbase(&schedule, assumed_inclusion);
        let predicted_epoch = current_epoch_pre_run_coinbase(&after, assumed_inclusion);
        let next_epoch = predicted_epoch
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("commit epoch overflow"))?;
        // The commit's inclusion block may so far exist only on a separate
        // submission endpoint; read it where it exists.
        let snapshot = client.at_receipt(hash).await?;
        let (first, second) = super::reads::fetch(
            &snapshot,
            (
                st::TIMELOCKED_WEIGHT_COMMITS.at((index, predicted_epoch)),
                st::TIMELOCKED_WEIGHT_COMMITS.at((index, next_epoch)),
            ),
        )
        .await?;
        let matches = |entry: &(AccountId, u64, Vec<u8>, u64)| {
            entry.0 == hotkey
                && entry.1 == inclusion
                && entry.2 == commit.ciphertext
                && entry.3 == commit.reveal_round
        };
        let actual = if first.iter().any(matches) {
            predicted_epoch
        } else if second.iter().any(matches) {
            next_epoch
        } else {
            // Delayed inclusion can span more than one epoch. Read the actual
            // inclusion index and probe it rather than guessing or abandoning
            // an already accepted commitment.
            let current =
                super::reads::storage(&snapshot, &st::SUBNET_EPOCH_INDEX, (netuid,)).await?;
            let queue =
                super::reads::storage(&snapshot, &st::TIMELOCKED_WEIGHT_COMMITS, (index, current))
                    .await?;
            anyhow::ensure!(
                queue.iter().any(matches),
                "included commit is absent from predicted and actual inclusion epoch queues"
            );
            current
        };
        if actual != predicted_epoch {
            metrics.increment("rtao_weights_commit_epoch_slip_total");
        }
        let reveal_epoch = actual
            .checked_add(state.reveal_period)
            .ok_or_else(|| anyhow::anyhow!("reveal epoch overflow"))?;
        self.pending.push(Pending {
            netuid,
            mechanism,
            hotkey,
            uid: state
                .uid
                .ok_or_else(|| anyhow::anyhow!("included commit lost its preflight UID"))?,
            uids: row.0,
            version_key,
            commit_epoch: actual,
            reveal_epoch,
            predicted_first_block: predict_first_reveal_block(&schedule, state.reveal_period)?,
            round: commit.reveal_round,
            ciphertext: commit.ciphertext,
            inclusion_block: inclusion,
            first_check: None,
            first_checked: false,
            first_reveal_hash: None,
            last_round: 0,
            notices: Vec::new(),
            warned_permit: false,
            ambiguous_reveal: false,
            followed_cr: state.followed_cr,
        });
        // Reconciliation may discover an inclusion older than the head that
        // triggered submission. Replay finalized history from that inclusion
        // so a reveal already emitted during reconciliation is still observed.
        let resume = inclusion.saturating_sub(1);
        self.last_observed_block = Some(
            self.last_observed_block
                .map_or(resume, |last| last.min(resume)),
        );
        Ok(())
    }

    /// Follow finalized history to `head` and attribute outcomes at actual
    /// epochs.
    ///
    /// Only blocks that can change a pending outcome are read (D-096): each
    /// block where an epoch is due (the only blocks that can carry an
    /// `EpochDeferred` or `EpochSkipped` event, or open or close a reveal
    /// window), and, while a reveal window is open, the block where a
    /// commitment's queue changes. Every other block is covered by one read
    /// at the end of the span that confirms the stored schedule, reveal
    /// period and open queues are what the last verified block predicts. A
    /// mismatch (an owner-triggered epoch, a deferral, a skipped epoch or a
    /// parameter change) is located by bisection and that block is read in
    /// full. An owner trigger that fires exactly at the automatic epoch
    /// leaves no stored trace and is not reported as an epoch notice; it did
    /// not shorten any reveal window. A call reads at most 256 blocks and
    /// otherwise keeps its cursor for the next call.
    pub async fn observe(
        &mut self,
        client: &Client,
        head: &BlockHeader,
        metrics: &Metrics,
    ) -> anyhow::Result<()> {
        self.observe_at(client, head, None, metrics).await
    }

    /// [`Self::observe`] with the head's snapshot when the caller already
    /// resolved it.
    pub(super) async fn observe_at(
        &mut self,
        client: &Client,
        head: &BlockHeader,
        head_snapshot: Option<Snapshot>,
        metrics: &Metrics,
    ) -> anyhow::Result<()> {
        let mut index = 0;
        while index < self.unlocated.len() {
            let unlocated = self.unlocated[index].clone();
            if let Err(error) = self
                .try_record(
                    client,
                    unlocated.netuid,
                    unlocated.mechanism,
                    unlocated.hotkey,
                    unlocated.row,
                    unlocated.version_key,
                    unlocated.state,
                    unlocated.commit,
                    &unlocated.report,
                    metrics,
                )
                .await
            {
                tracing::warn!(mechanism = unlocated.mechanism.0, %error, "retaining accepted commitment until its inclusion queue can be read");
                index += 1;
            } else {
                self.unlocated.remove(index);
            }
        }
        let mut reads = 0;
        let mut head_snapshot = head_snapshot.filter(|snapshot| snapshot.block().hash == head.hash);
        // After a block that broke the prediction, read each block until one
        // step is predicted again, so a schedule the port cannot predict
        // costs no more than reading every block.
        let mut stepwise = false;
        loop {
            if self.pending.is_empty() {
                self.anchor = None;
                if self
                    .last_observed_block
                    .is_none_or(|last| last < head.number)
                {
                    self.last_observed_block = Some(head.number);
                }
                return Ok(());
            }
            let cursor = match self.last_observed_block {
                Some(cursor) => cursor,
                None => head.number.saturating_sub(1),
            };
            if cursor >= head.number || reads >= REVEAL_BLOCK_READS {
                // A capped call keeps its cursor; the next continues.
                return Ok(());
            }
            let anchor = match self.anchor.take() {
                Some(anchor) if anchor.block() == cursor && self.covers(&anchor) => anchor,
                _ => {
                    reads += 1;
                    let snapshot = block_snapshot(client, head, &mut head_snapshot, cursor).await?;
                    self.read_anchor(snapshot).await?
                }
            };
            let next = if stepwise {
                Some(cursor + 1)
            } else {
                anchor.next_epoch_block(head.number)
            };
            // The span before the next epoch block, or to the head, must hold
            // what the anchor predicts before it is skipped.
            let end = next.map_or(head.number, |block| block - 1);
            let observed = if end == cursor {
                anchor.clone()
            } else {
                reads += 1;
                let snapshot = block_snapshot(client, head, &mut head_snapshot, end).await?;
                self.read_state(snapshot, anchor.queues.keys()).await?
            };
            if !anchor.predicts(&observed) {
                // Bisect to the first block that differs from the prediction.
                let mut before = anchor.clone();
                let mut after = observed;
                while after.block() - before.block() > 1 {
                    let middle = before.block() + (after.block() - before.block()) / 2;
                    reads += 1;
                    let snapshot = block_snapshot(client, head, &mut head_snapshot, middle).await?;
                    let probe = self.read_state(snapshot, anchor.queues.keys()).await?;
                    if anchor.predicts(&probe) {
                        before = probe;
                    } else {
                        after = probe;
                    }
                }
                stepwise = after.block() == anchor.block() + 1;
                self.skip_to(before.block());
                self.process_block(client, &after.snapshot, &before, head.number, metrics)
                    .await?;
                self.anchor = Some(self.anchor_at(after).await?);
                continue;
            }
            let Some(block) = next else {
                self.last_observed_block = Some(head.number);
                self.anchor = Some(observed);
                return Ok(());
            };
            reads += 1;
            let snapshot = block_snapshot(client, head, &mut head_snapshot, block).await?;
            self.skip_to(observed.block());
            self.process_block(client, &snapshot, &observed, head.number, metrics)
                .await?;
            if !self.pending.is_empty() {
                let next = self.read_anchor(snapshot).await?;
                stepwise &= observed.predict(next.block()) != next.states;
                self.anchor = Some(next);
            }
        }
    }

    /// Advance the cursor over blocks verified to change no pending outcome.
    fn skip_to(&mut self, block: u64) {
        if self.last_observed_block.is_none_or(|last| last < block) {
            self.last_observed_block = Some(block);
        }
    }

    /// Whether `anchor` holds every pending subnet and every queue the
    /// pending commitments are followed through after it.
    fn covers(&self, anchor: &Anchor) -> bool {
        self.pending
            .iter()
            .all(|pending| anchor.states.contains_key(&pending.netuid))
            && self
                .span_queues(anchor)
                .iter()
                .all(|key| anchor.queues.contains_key(key))
    }

    /// Queues to verify over the span after `anchor`. When the next block
    /// runs an epoch the span is empty: that block is read in full.
    fn span_queues(&self, anchor: &Anchor) -> BTreeSet<QueueKey> {
        let next = anchor.block().saturating_add(1);
        if anchor.next_epoch_block(next) == Some(next) {
            return BTreeSet::new();
        }
        self.followed_queues(&anchor.states, next)
    }

    /// Queues whose change can settle a pending commitment in `block`: its
    /// reveal window is open or has passed, unless an ambiguous reveal past
    /// its window can no longer settle.
    fn followed_queues(
        &self,
        states: &BTreeMap<NetUid, WatchState>,
        block: u64,
    ) -> BTreeSet<QueueKey> {
        self.pending
            .iter()
            .filter(|pending| block >= pending.inclusion_block)
            .filter_map(|pending| {
                let state = states.get(&pending.netuid)?;
                let reveal = pending.commit_epoch.saturating_add(state.reveal_period);
                let epoch = state.epoch_at(block);
                (epoch == reveal || (epoch > reveal && !pending.ambiguous_reveal)).then_some((
                    pending.netuid,
                    pending.mechanism,
                    pending.commit_epoch,
                ))
            })
            .collect()
    }

    /// Read a new anchor at `snapshot`.
    async fn read_anchor(&self, snapshot: Snapshot) -> anyhow::Result<Anchor> {
        let anchor = self.read_state(snapshot, std::iter::empty()).await?;
        self.anchor_at(anchor).await
    }

    /// Complete `anchor`'s queues with every queue followed after it.
    async fn anchor_at(&self, mut anchor: Anchor) -> anyhow::Result<Anchor> {
        let followed = self.span_queues(&anchor);
        anchor.queues.retain(|key, _| followed.contains(key));
        for key in followed {
            if !anchor.queues.contains_key(&key) {
                let queue = read_queue(&anchor.snapshot, key).await?;
                anchor.queues.insert(key, queue);
            }
        }
        Ok(anchor)
    }

    /// Read each pending subnet's state and the named queues at `snapshot`.
    async fn read_state<'a>(
        &self,
        snapshot: Snapshot,
        queues: impl Iterator<Item = &'a QueueKey>,
    ) -> anyhow::Result<Anchor> {
        let mut states = BTreeMap::new();
        for netuid in self
            .pending
            .iter()
            .map(|pending| pending.netuid)
            .collect::<BTreeSet<_>>()
        {
            let (tempo, last_epoch_block, pending_epoch_at, subnet_epoch_index, since, period) =
                super::reads::fetch(
                    &snapshot,
                    (
                        st::TEMPO.at((netuid,)),
                        st::LAST_EPOCH_BLOCK.at((netuid,)),
                        st::PENDING_EPOCH_AT.at((netuid,)),
                        st::SUBNET_EPOCH_INDEX.at((netuid,)),
                        st::BLOCKS_SINCE_LAST_STEP.at((netuid,)),
                        st::REVEAL_PERIOD_EPOCHS.at((netuid,)),
                    ),
                )
                .await?;
            states.insert(
                netuid,
                WatchState {
                    tempo,
                    last_epoch_block,
                    pending_epoch_at,
                    subnet_epoch_index,
                    blocks_since_last_step: since,
                    reveal_period: period,
                },
            );
        }
        let mut read = BTreeMap::new();
        for key in queues {
            read.insert(*key, read_queue(&snapshot, *key).await?);
        }
        Ok(Anchor {
            snapshot,
            states,
            queues: read,
        })
    }

    /// Attribute one finalized block, given its parent's verified state.
    async fn process_block(
        &mut self,
        client: &Client,
        snapshot: &Snapshot,
        parent: &Anchor,
        head: u64,
        metrics: &Metrics,
    ) -> anyhow::Result<()> {
        let number = snapshot.block().number;
        let hash = snapshot.block().hash;
        if self.last_observed_block.is_some_and(|last| number <= last) {
            return Ok(());
        }
        if self.pending.is_empty() {
            self.last_observed_block = Some(number);
            return Ok(());
        }
        anyhow::ensure!(
            parent.block().checked_add(1) == Some(number),
            "reveal parent state is not the block's parent"
        );
        if number < head {
            metrics.increment("rtao_weights_reveal_history_replayed_total");
        }
        let events = snapshot.events().await?;
        // Reveal processing runs before coinbase. Using the parent schedule
        // captures its lookahead even when coinbase is deferred or skipped.
        let parent_state = &parent.snapshot;
        let mut completed = Vec::new();
        let mut stored_round = None;
        for pending in &mut self.pending {
            if number < pending.inclusion_block {
                continue;
            }
            for event in events.records() {
                if !event.matches("SubtensorModule", "EpochDeferred")
                    && !event.matches("SubtensorModule", "EpochSkipped")
                    && !event.matches("SubtensorModule", "EpochTriggered")
                {
                    continue;
                }
                if event_netuid(&event.attributes) != Some(pending.netuid.0) {
                    continue;
                }
                let notice = match event.event_id.as_str() {
                    "EpochDeferred" => EpochNotice::Deferred(number),
                    "EpochSkipped" => EpochNotice::Skipped(number),
                    _ => EpochNotice::Triggered(number),
                };
                if pending.notices.len() < 64 {
                    pending.notices.push(notice);
                }
            }
            let index = NetUidStorageIndex::new(pending.netuid, pending.mechanism)?;
            let state = parent
                .states
                .get(&pending.netuid)
                .ok_or_else(|| anyhow::anyhow!("reveal parent state lacks a pending subnet"))?;
            // Runtime reveal processing selects the queue using its current
            // period, not a period frozen when our process prepared the commit.
            pending.reveal_epoch = pending
                .commit_epoch
                .checked_add(state.reveal_period)
                .ok_or_else(|| anyhow::anyhow!("reveal epoch overflow"))?;
            let epoch = state.epoch_at(number);
            if epoch == pending.reveal_epoch
                && events.records().iter().any(|event| {
                    event.matches("SubtensorModule", "TimelockedWeightsRevealed")
                        && reveal_matches(&event.attributes, index, pending.hotkey)
                })
            {
                let queue = super::reads::storage(
                    parent_state,
                    &st::TIMELOCKED_WEIGHT_COMMITS,
                    (index, pending.commit_epoch),
                )
                .await?;
                let candidates = queue
                    .iter()
                    .filter(|entry| entry.0 == pending.hotkey)
                    .collect::<Vec<_>>();
                // The event identifies account/mechanism, not ciphertext. Only
                // a unique matching parent-queue entry binds it to this attempt.
                if candidates.len() == 1
                    && candidates[0].1 == pending.inclusion_block
                    && candidates[0].2 == pending.ciphertext
                    && candidates[0].3 == pending.round
                {
                    completed.push((pending.mechanism, pending.inclusion_block, None));
                } else {
                    pending.ambiguous_reveal = true;
                }
                continue;
            }
            if epoch < pending.reveal_epoch {
                if !pending.warned_permit && epoch > pending.commit_epoch {
                    let permits =
                        super::reads::storage(snapshot, &st::VALIDATOR_PERMIT, (pending.netuid,))
                            .await?;
                    if permits.get(usize::from(pending.uid.0)) != Some(&true) {
                        tracing::warn!(
                            mechanism = pending.mechanism.0,
                            reveal_epoch = pending.reveal_epoch,
                            "validator permit lost before reveal epoch"
                        );
                        metrics.increment("rtao_weights_permit_lost_before_reveal_total");
                        pending.warned_permit = true;
                    }
                }
                continue;
            }
            if epoch > pending.reveal_epoch {
                if pending.ambiguous_reveal {
                    continue;
                }
                let queue = super::reads::storage(
                    snapshot,
                    &st::TIMELOCKED_WEIGHT_COMMITS,
                    (index, pending.commit_epoch),
                )
                .await?;
                if queue.iter().any(|entry| {
                    entry.0 == pending.hotkey
                        && entry.1 == pending.inclusion_block
                        && entry.2 == pending.ciphertext
                        && entry.3 == pending.round
                }) {
                    continue;
                }
            }
            if !pending.first_checked {
                let first = *pending.first_reveal_hash.get_or_insert(hash);
                let first_snapshot = if first == hash {
                    snapshot.clone()
                } else {
                    client.at(BlockRef::Hash(first)).await?
                };
                pending.first_check = first_check(&first_snapshot, pending).await?;
                pending.first_checked = true;
            }
            if epoch == pending.reveal_epoch {
                pending.last_round = if let Some(round) = stored_round {
                    round
                } else {
                    let round = super::reads::storage(
                        snapshot,
                        &rtao_runtime::storage::drand::LAST_STORED_ROUND,
                        (),
                    )
                    .await?;
                    stored_round = Some(round);
                    round
                };
            }
            if epoch > pending.reveal_epoch {
                // The parent is the last block inside the reveal window. Its
                // drand state stays valid even if this block ingested the round
                // too late, or an earlier watcher RPC temporarily failed.
                pending.last_round = super::reads::storage(
                    parent_state,
                    &rtao_runtime::storage::drand::LAST_STORED_ROUND,
                    (),
                )
                .await?;
                let reason = pending.first_check.unwrap_or_else(|| {
                    if pending.last_round < pending.round {
                        RevealMissReason::DrandRound
                    } else if pending
                        .notices
                        .iter()
                        .any(|event| matches!(event, EpochNotice::Triggered(_)))
                    {
                        RevealMissReason::EpochTriggered
                    } else {
                        RevealMissReason::Unknown
                    }
                });
                completed.push((pending.mechanism, pending.inclusion_block, Some(reason)));
            }
        }
        if completed.iter().any(|(_, _, missed)| missed.is_none()) {
            // The reveal ran under the parent's code and stored tag. Evidence
            // is optional: a failed read only delays following a changed tag.
            // The native tag needs no evidence, so nothing is retained for it.
            match reveal_identity(client, parent_state).await {
                Ok((code, tag)) if tag != super::NATIVE_CR_PAYLOAD_VERSION => {
                    self.remember_revealed_code(code, tag);
                }
                Ok(_) => {}
                Err(error) => tracing::debug!(%error, "reveal code identity unavailable"),
            }
        }
        for (mechanism, inclusion, missed) in completed {
            if let Some(position) = self.pending.iter().position(|pending| {
                pending.mechanism == mechanism && pending.inclusion_block == inclusion
            }) {
                let pending = self.pending.remove(position);
                if let Some(reason) = missed {
                    metrics.increment(&format!(
                        "rtao_weights_reveal_missed_total{{mech=\"{}\",reason=\"{}\"}}",
                        mechanism.0,
                        reason.label()
                    ));
                    metrics.update_readiness(|ready| ready.last_weight_succeeded = false);
                    tracing::warn!(mechanism = mechanism.0, reason = reason.label(), round = pending.round, predicted_first_block = pending.predicted_first_block, last_stored_round = pending.last_round, epoch_notices = ?pending.notices, "weight reveal epoch ended without a success event");
                    if reason == RevealMissReason::Unknown
                        && let Some(followed) = pending.followed_cr
                    {
                        // Nothing else explains the miss of a commitment
                        // tagged with a followed version: stop following it.
                        self.remember_missed_reveal(followed.code, followed.tag);
                        metrics.increment("rtao_weights_cr_version_refuted_total");
                        tracing::warn!(
                            mechanism = mechanism.0,
                            commit_reveal_version = followed.tag,
                            runtime_code = ?followed.code,
                            "a commitment tagged with a followed commit-reveal version missed its reveal; timelocked commits pause under this code and version"
                        );
                    }
                } else {
                    metrics.increment("rtao_weights_revealed_total");
                }
                if self.outcomes.len() == 256 {
                    self.outcomes.pop_front();
                }
                self.outcomes.push_back(RevealOutcome {
                    mechanism,
                    commit_epoch: pending.commit_epoch,
                    reveal_epoch: pending.reveal_epoch,
                    predicted_first_block: pending.predicted_first_block,
                    round: pending.round,
                    observed_block: number,
                    missed,
                    native_queue_absent: missed.is_some(),
                    epoch_notices: pending.notices,
                });
            }
        }
        self.last_observed_block = Some(number);
        Ok(())
    }
}

/// A snapshot at `number`, reusing the head's when it is the head.
async fn block_snapshot(
    client: &Client,
    head: &BlockHeader,
    head_snapshot: &mut Option<Snapshot>,
    number: u64,
) -> anyhow::Result<Snapshot> {
    if number == head.number {
        if let Some(snapshot) = head_snapshot.clone() {
            return Ok(snapshot);
        }
        let snapshot = client.at(BlockRef::Hash(head.hash)).await?;
        *head_snapshot = Some(snapshot.clone());
        return Ok(snapshot);
    }
    Ok(client.at(BlockRef::Number(number)).await?)
}

async fn read_queue(
    snapshot: &Snapshot,
    (netuid, mechanism, epoch): QueueKey,
) -> anyhow::Result<CommitQueue> {
    let index = NetUidStorageIndex::new(netuid, mechanism)?;
    super::reads::storage(snapshot, &st::TIMELOCKED_WEIGHT_COMMITS, (index, epoch)).await
}

/// `:code` hash and stored `CommitRevealWeightsVersion` at `parent`, the
/// state a reveal in its child ran under.
async fn reveal_identity(
    client: &Client,
    parent: &Snapshot,
) -> anyhow::Result<(rtao_primitives::H256, u16)> {
    let code = super::runtime_code_hash(client, parent.block().hash).await?;
    let tag = super::reads::storage(parent, &st::COMMIT_REVEAL_WEIGHTS_VERSION, ()).await?;
    Ok((code, tag))
}

async fn first_check(
    snapshot: &Snapshot,
    pending: &Pending,
) -> anyhow::Result<Option<RevealMissReason>> {
    let (uid, permits, threshold, version, population, min, owner, consensus) =
        super::reads::fetch(
            snapshot,
            (
                st::UIDS.at((pending.netuid, pending.hotkey)),
                st::VALIDATOR_PERMIT.at((pending.netuid,)),
                st::STAKE_THRESHOLD.at(()),
                st::WEIGHTS_VERSION_KEY.at((pending.netuid,)),
                st::SUBNETWORK_N.at((pending.netuid,)),
                st::MIN_ALLOWED_WEIGHTS.at((pending.netuid,)),
                st::SUBNET_OWNER_HOTKEY.at((pending.netuid,)),
                st::SUBNET_EPOCH_CONSENSUS.at((pending.netuid,)),
            ),
        )
        .await?;
    let Some(uid) = uid else {
        return Ok(Some(RevealMissReason::NotRegistered));
    };
    if owner.raw() != &pending.hotkey {
        let graph = snapshot
            .subnets()
            .selective_metagraph_with(
                pending.netuid,
                pending.mechanism,
                &[
                    rtao_runtime::SelectiveMetagraphIndex::Hotkeys,
                    rtao_runtime::SelectiveMetagraphIndex::TotalStake,
                ],
                MetagraphOptions::default(),
            )
            .await?;
        if let Some(graph) = graph {
            if let Some(neuron) = graph.by_hotkey(&pending.hotkey) {
                if neuron.total_stake()?.0 < threshold {
                    return Ok(Some(RevealMissReason::Stake));
                }
            } else {
                return Ok(Some(RevealMissReason::NotRegistered));
            }
        } else {
            anyhow::bail!("reveal stake snapshot is unavailable");
        }
    }
    if pending.version_key < version {
        return Ok(Some(RevealMissReason::VersionKey));
    }
    let self_only = pending.uids == [Uid(uid)];
    let null_consensus = super::consensus_null(&consensus)?;
    let yuma_owner = !null_consensus && owner.raw() == &pending.hotkey;
    if (null_consensus || (!self_only && !yuma_owner))
        && permits.get(usize::from(uid)) != Some(&true)
    {
        return Ok(Some(RevealMissReason::Permit));
    }
    if pending.uids.iter().any(|uid| uid.0 >= population) {
        return Ok(Some(RevealMissReason::Uid));
    }
    if pending.uids.len() > usize::from(population)
        || (!self_only && pending.uids.len() < usize::from(min.min(population)))
    {
        return Ok(Some(RevealMissReason::Length));
    }
    Ok(None)
}
fn event_netuid(attributes: &Value) -> Option<u16> {
    let value = attributes.field("netuid").or_else(|| match attributes {
        Value::List(fields) | Value::Tuple(fields) => fields.first(),
        _ => None,
    })?;
    u16::try_from(value.as_u128()?).ok()
}
fn reveal_matches(attributes: &Value, index: NetUidStorageIndex, hotkey: AccountId) -> bool {
    let (netuid, who) = match attributes {
        Value::List(fields) | Value::Tuple(fields) if fields.len() == 2 => (&fields[0], &fields[1]),
        Value::Dict(_) => match (
            attributes
                .field("netuid_index")
                .or_else(|| attributes.field("netuid")),
            attributes.field("who"),
        ) {
            (Some(index), Some(who)) => (index, who),
            _ => return false,
        },
        _ => return false,
    };
    let index_matches = netuid.as_u128() == Some(u128::from(index.0));
    let key_matches = who.value_bytes().as_deref() == Some(hotkey.0.as_slice())
        || who.as_str().is_some_and(|text| text == hotkey.to_string());
    index_matches && key_matches
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::panic)]
    use super::*;
    use crate::weights::tests::{HOTKEY, fixture, header, seed_at, split_fixture};
    use rtao_primitives::IntoCoreValue;

    async fn slipped() -> anyhow::Result<(rtao_testkit::FakeChain, Client, RevealWatcher, Metrics)>
    {
        let (chain, client) = fixture().await?;
        let (watcher, metrics) = record_slipped(&chain, &client, |_, hash| async move {
            Ok(crate::weights::submit::WeightSubmission {
                block_hash: hash,
                block_number: 110,
            })
        })
        .await?;
        Ok((chain, client, watcher, metrics))
    }

    /// Seed a commit included at block 110 on `chain` (one epoch after the
    /// predicted one), locate it with `locate` and record it.
    async fn record_slipped<F, Fut>(
        chain: &rtao_testkit::FakeChain,
        client: &Client,
        locate: F,
    ) -> anyhow::Result<(RevealWatcher, Metrics)>
    where
        F: FnOnce(Client, rtao_primitives::H256) -> Fut,
        Fut: std::future::Future<Output = anyhow::Result<crate::weights::submit::WeightSubmission>>,
    {
        record_slipped_as(chain, client, |_| {}, locate).await
    }

    /// [`record_slipped`] with the preflight adjusted before recording.
    async fn record_slipped_as<F, Fut>(
        chain: &rtao_testkit::FakeChain,
        client: &Client,
        adjust: impl FnOnce(&mut WeightPreflight),
        locate: F,
    ) -> anyhow::Result<(RevealWatcher, Metrics)>
    where
        F: FnOnce(Client, rtao_primitives::H256) -> Fut,
        Fut: std::future::Future<Output = anyhow::Result<crate::weights::submit::WeightSubmission>>,
    {
        let best = client.at(BlockRef::Best).await?;
        let mut state = WeightPreflight::fetch(&best, NetUid(1), MechId(0), HOTKEY).await?;
        adjust(&mut state);
        let commit = PreparedWeightCommit {
            ciphertext: vec![4, 5, 6],
            reveal_round: 500,
        };
        chain.mutate_state(|state| -> anyhow::Result<()> {
            state.advance(30);
            for block in [110, 119] {
                let queue = commit_queue(state, &[(HOTKEY, 110, commit.ciphertext.clone(), 500)])?;
                seed_at(
                    state,
                    block,
                    "SubtensorModule",
                    "TimelockedWeightCommits",
                    (NetUidStorageIndex(1), 6u64),
                    queue,
                )?;
            }
            for block in [110, 119, 120] {
                seed_at(
                    state,
                    block,
                    "SubtensorModule",
                    "SubnetEpochIndex",
                    (NetUid(1),),
                    6u64,
                )?;
                seed_at(
                    state,
                    block,
                    "SubtensorModule",
                    "LastEpochBlock",
                    (NetUid(1),),
                    110u64,
                )?;
                seed_at(
                    state,
                    block,
                    "SubtensorModule",
                    "BlocksSinceLastStep",
                    (NetUid(1),),
                    block.saturating_sub(110),
                )?;
            }
            seed_at(
                state,
                129,
                "SubtensorModule",
                "SubnetEpochIndex",
                (NetUid(1),),
                7u64,
            )?;
            seed_at(
                state,
                129,
                "SubtensorModule",
                "LastEpochBlock",
                (NetUid(1),),
                120u64,
            )?;
            seed_at(
                state,
                129,
                "SubtensorModule",
                "BlocksSinceLastStep",
                (NetUid(1),),
                9u64,
            )?;
            seed_at(
                state,
                120,
                "SubtensorModule",
                "SubnetOwnerHotkey",
                (NetUid(1),),
                HOTKEY,
            )?;
            seed_at(state, 120, "Drand", "LastStoredRound", (), 499u64)?;
            seed_at(state, 129, "Drand", "LastStoredRound", (), 499u64)?;
            seed_at(state, 130, "Drand", "LastStoredRound", (), 501u64)?;
            Ok(())
        })?;
        let included = header(chain, 110)?;
        let report = locate(client.clone(), included.hash).await?;
        let metrics = Metrics::default();
        let mut watcher = RevealWatcher::default();
        watcher
            .record(
                client,
                NetUid(1),
                MechId(0),
                HOTKEY,
                (vec![Uid(1)], vec![65535]),
                0,
                state,
                commit,
                &report,
                &metrics,
            )
            .await?;
        Ok((watcher, metrics))
    }

    /// A `TimelockedWeightCommits` queue value for the live entry type: a
    /// list of tuples, or of records keyed by the live field names when the
    /// entry is a named struct.
    pub(super) fn commit_queue(
        state: &rtao_testkit::FakeState,
        entries: &[(AccountId, u64, Vec<u8>, u64)],
    ) -> anyhow::Result<Value> {
        use scale_info::TypeDef;
        let info = state
            .runtime
            .storage_entry("SubtensorModule", "TimelockedWeightCommits")
            .ok_or_else(|| anyhow::anyhow!("missing commit queue"))?;
        let mut id = info.value_type;
        let element = loop {
            match &state
                .runtime
                .resolve(id)
                .map_err(rtao_core::CoreError::from)?
                .type_def
            {
                TypeDef::Composite(composite) if composite.fields.len() == 1 => {
                    id = composite.fields[0].ty.id;
                }
                TypeDef::Sequence(sequence) => break sequence.type_param.id,
                _ => anyhow::bail!("commit queue is not a sequence"),
            }
        };
        let names = match &state
            .runtime
            .resolve(element)
            .map_err(rtao_core::CoreError::from)?
            .type_def
        {
            TypeDef::Composite(composite) if composite.fields.iter().all(|f| f.name.is_some()) => {
                Some(
                    composite
                        .fields
                        .iter()
                        .filter_map(|field| field.name.clone())
                        .collect::<Vec<_>>(),
                )
            }
            _ => None,
        };
        Ok(Value::List(
            entries
                .iter()
                .map(|(who, block, commit, round)| match &names {
                    None => (*who, *block, commit.clone(), *round).into_core_value(),
                    Some(names) => Value::record(
                        names
                            .iter()
                            .map(|name| {
                                let value = match name.as_str() {
                                    "who" | "account" | "hotkey" => who.into_core_value(),
                                    "commit_block" => block.into_core_value(),
                                    "commit" | "serialized_compressed_commit" => {
                                        Value::Bytes(commit.clone())
                                    }
                                    "reveal_round" | "round_number" => round.into_core_value(),
                                    _ => Value::Int(0),
                                };
                                (name.clone(), value)
                            })
                            .collect(),
                    ),
                })
                .collect(),
        ))
    }

    /// A commit queue entry that became a named struct with reordered,
    /// aliased and added fields still locates the commitment and matches its
    /// reveal (D-089).
    #[tokio::test]
    async fn a_reordered_commit_struct_still_records_and_matches_its_reveal() -> anyhow::Result<()>
    {
        let (chain, client) =
            crate::weights::tests::fixture_with(crate::weights::read_tests::reordered_commits)
                .await?;
        let runtime = client.at(BlockRef::Best).await?.block().runtime.clone();
        assert!(
            !runtime
                .surface_report()
                .shape_matches("storage:SubtensorModule.TimelockedWeightCommits")
        );
        let (mut watcher, metrics) = record_slipped(&chain, &client, |_, hash| async move {
            Ok(crate::weights::submit::WeightSubmission {
                block_hash: hash,
                block_number: 110,
            })
        })
        .await?;
        assert_eq!(watcher.pending(), 1);
        assert_eq!(watcher.pending[0].commit_epoch, 6);
        assert_eq!(watcher.pending[0].reveal_epoch, 7);
        chain.mutate_state(|state| {
            let hash = state.block_hash(120).expect("reveal hash");
            state
                .seed_events(hash, success_event())
                .expect("seed reveal");
            let parent = state.block_hash(119).expect("parent hash");
            state.seed_at(parent, b":code".to_vec(), Some(b"reveal code".to_vec()));
        });
        watcher
            .observe(&client, &header(&chain, 120)?, &metrics)
            .await?;
        assert_eq!(watcher.pending(), 0);
        let result = watcher.outcomes().front().expect("outcome");
        assert_eq!(result.reveal_epoch, 7);
        assert!(result.missed.is_none());
        client.close().await;
        Ok(())
    }

    /// A commit queue entry that lost a field refuses by name, and the
    /// accepted commitment stays unlocated for a later attempt instead of
    /// being abandoned.
    #[tokio::test]
    async fn a_commit_queue_missing_a_field_refuses_and_keeps_the_commitment() -> anyhow::Result<()>
    {
        use crate::weights::read_tests::{add_type, commit_element, commit_fields};
        let (chain, client) = crate::weights::tests::fixture_with(|meta| {
            let [who, block, commit, _] = commit_fields(meta);
            let tuple = add_type(
                meta,
                &[],
                scale_info::TypeDef::Tuple(scale_info::TypeDefTuple {
                    fields: vec![who.into(), block.into(), commit.into()],
                }),
            );
            commit_element(meta, tuple);
        })
        .await?;
        let best = client.at(BlockRef::Best).await?;
        let state = WeightPreflight::fetch(&best, NetUid(1), MechId(0), HOTKEY).await?;
        chain.mutate_state(|state| state.advance(30));
        let report = crate::weights::submit::WeightSubmission {
            block_hash: header(&chain, 110)?.hash,
            block_number: 110,
        };
        let metrics = Metrics::default();
        let mut watcher = RevealWatcher::default();
        let error = watcher
            .record(
                &client,
                NetUid(1),
                MechId(0),
                HOTKEY,
                (vec![Uid(1)], vec![65535]),
                0,
                state,
                PreparedWeightCommit {
                    ciphertext: vec![4, 5, 6],
                    reveal_round: 500,
                },
                &report,
                &metrics,
            )
            .await
            .expect_err("a three-field commit has no reveal round");
        let Some(rtao_client::ClientError::ReadResolveIncompatible(refusal)) =
            error.downcast_ref::<rtao_client::ClientError>()
        else {
            panic!("expected a name-resolved refusal, got {error:#}");
        };
        assert_eq!(refusal.item, "TimelockedWeightCommits");
        assert!(
            refusal.detail.contains("tuple of 3 fields, expected 4"),
            "{refusal}"
        );
        assert!(watcher.pending.is_empty());
        assert_eq!(watcher.unlocated.len(), 1);
        assert_eq!(watcher.pending(), 1, "the commitment is still tracked");
        client.close().await;
        Ok(())
    }

    /// With a separate submission endpoint, the commit's inclusion block may
    /// exist only on that endpoint. Locating and recording it still succeeds.
    #[tokio::test]
    async fn split_endpoint_commit_is_located_and_recorded_while_reads_lag() -> anyhow::Result<()> {
        let (read, node, client) = split_fixture().await?;
        let (watcher, metrics) = record_slipped(&node, &client, |client, hash| async move {
            // The read endpoint has not imported the inclusion block.
            assert!(client.at(BlockRef::Hash(hash)).await.is_err());
            crate::weights::submit::inclusion(&client, hash).await
        })
        .await?;
        assert!(header(&read, 110).is_err(), "the read endpoint lags");
        assert_eq!(watcher.pending(), 1);
        assert!(watcher.unlocated.is_empty());
        assert_eq!(watcher.pending[0].inclusion_block, 110);
        assert_eq!(watcher.pending[0].commit_epoch, 6);
        assert_eq!(watcher.pending[0].reveal_epoch, 7);
        assert!(
            metrics
                .render()
                .contains("rtao_weights_commit_epoch_slip_total 1")
        );
        client.close().await;
        Ok(())
    }
    /// A commit tagged `tag` after following it under runtime code `code`.
    async fn slipped_followed(
        tag: u16,
        code: rtao_primitives::H256,
    ) -> anyhow::Result<(rtao_testkit::FakeChain, Client, RevealWatcher, Metrics)> {
        let (chain, client) = fixture().await?;
        let (watcher, metrics) = record_slipped_as(
            &chain,
            &client,
            |state| {
                state.commit_reveal_version = tag;
                state.runtime_code = Some(code);
            },
            |_, hash| async move {
                Ok(crate::weights::submit::WeightSubmission {
                    block_hash: hash,
                    block_number: 110,
                })
            },
        )
        .await?;
        Ok((chain, client, watcher, metrics))
    }

    fn code_hash(code: &[u8]) -> rtao_primitives::H256 {
        rtao_primitives::H256(
            <blake2::Blake2b<blake2::digest::consts::U32> as blake2::Digest>::digest(code).into(),
        )
    }

    fn success_event() -> serde_json::Value {
        serde_json::json!([{"phase":"Initialization","event":{"SubtensorModule":{"TimelockedWeightsRevealed":[1,HOTKEY.to_string()]}},"topics":[]}])
    }

    #[tokio::test]
    async fn inclusion_slip_follows_actual_epoch_and_never_counts_a_miss() -> anyhow::Result<()> {
        let (chain, client, mut watcher, metrics) = slipped().await?;
        assert_eq!(watcher.pending(), 1);
        assert_eq!(watcher.pending[0].commit_epoch, 6);
        assert_eq!(watcher.pending[0].reveal_epoch, 7);
        assert!(
            metrics
                .render()
                .contains("rtao_weights_commit_epoch_slip_total 1")
        );
        chain.mutate_state(|state| {
            let hash = state.block_hash(110).expect("included hash");
            state
                .seed_events(hash, success_event())
                .expect("seed another epoch's reveal");
        });
        watcher
            .observe(&client, &header(&chain, 110)?, &metrics)
            .await?;
        assert_eq!(
            watcher.pending(),
            1,
            "earlier epoch's same-account event cannot complete this commit"
        );
        assert!(watcher.revealed_code.is_empty());
        chain.mutate_state(|state| {
            let hash = state.block_hash(120).expect("actual reveal hash");
            state
                .seed_events(hash, success_event())
                .expect("seed actual reveal");
            // The reveal block executes under its parent's `:code` and tag.
            let parent = state.block_hash(119).expect("parent hash");
            state.seed_at(parent, b":code".to_vec(), Some(b"reveal code".to_vec()));
            seed_at(
                state,
                119,
                "SubtensorModule",
                "CommitRevealWeightsVersion",
                (),
                5u16,
            )
            .expect("seed stored tag");
        });
        watcher
            .observe(&client, &header(&chain, 120)?, &metrics)
            .await?;
        assert_eq!(watcher.pending(), 0);
        let code = code_hash(b"reveal code");
        assert!(watcher.reveals_native_payload(code, 5));
        assert!(
            !watcher.reveals_native_payload(code, 6),
            "evidence under one stored tag proves no other"
        );
        assert_eq!(watcher.revealed_code.len(), 1);
        let result = watcher.outcomes().front().expect("outcome");
        assert_eq!(result.commit_epoch, 6);
        assert_eq!(result.reveal_epoch, 7);
        assert_eq!(result.observed_block, 120);
        assert!(result.missed.is_none());
        watcher
            .observe(&client, &header(&chain, 130)?, &metrics)
            .await?;
        assert!(
            !metrics
                .render()
                .contains("rtao_weights_reveal_missed_total")
        );
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn round_arriving_after_actual_epoch_is_still_attributed_to_drand() -> anyhow::Result<()>
    {
        let (chain, client, mut watcher, metrics) = slipped().await?;
        chain.mutate_state(|state| {
            seed_at(
                state,
                120,
                "SubtensorModule",
                "ValidatorPermit",
                (NetUid(1),),
                vec![false, false, false],
            )
        })?;
        watcher
            .observe(&client, &header(&chain, 120)?, &metrics)
            .await?;
        assert_eq!(watcher.pending(), 1);
        watcher
            .observe(&client, &header(&chain, 130)?, &metrics)
            .await?;
        assert_eq!(
            watcher.outcomes().front().expect("miss").missed,
            Some(RevealMissReason::DrandRound)
        );
        assert!(
            watcher.revealed_code.is_empty(),
            "a missed reveal proves nothing about the runtime's decoder"
        );
        assert!(
            metrics
                .render()
                .contains("rtao_weights_reveal_missed_total{mech=\"0\",reason=\"drand_round\"} 1")
        );
        client.close().await;

        let (chain, client, mut watcher, metrics) = slipped().await?;
        chain.mutate_state(|state| -> anyhow::Result<()> {
            seed_at(
                state,
                120,
                "SubtensorModule",
                "ValidatorPermit",
                (NetUid(1),),
                vec![false, false, false],
            )?;
            seed_at(
                state,
                120,
                "SubtensorModule",
                "SubnetEpochConsensus",
                (NetUid(1),),
                "Null",
            )
        })?;
        watcher
            .observe(&client, &header(&chain, 120)?, &metrics)
            .await?;
        watcher
            .observe(&client, &header(&chain, 130)?, &metrics)
            .await?;
        assert_eq!(
            watcher.outcomes().front().expect("Null miss").missed,
            Some(RevealMissReason::Permit)
        );
        client.close().await;

        let (chain, client, mut watcher, metrics) = slipped().await?;
        chain.mutate_state(|state| -> anyhow::Result<()> {
            seed_at(
                state,
                120,
                "SubtensorModule",
                "ValidatorPermit",
                (NetUid(1),),
                vec![false, false, false],
            )?;
            seed_at(
                state,
                120,
                "SubtensorModule",
                "SubnetOwnerHotkey",
                (NetUid(1),),
                AccountId([8; 32]),
            )?;
            seed_at(state, 120, "SubtensorModule", "StakeThreshold", (), 0u64)?;
            state.seed_runtime(
                "SubnetInfoRuntimeApi",
                "get_selective_mechagraph",
                rtao_core::codec::value::to_corpus_json(&Value::record(vec![
                    ("netuid".into(), Value::Int(1)),
                    (
                        "hotkeys".into(),
                        Value::List(vec![Value::Bytes(HOTKEY.0.to_vec())]),
                    ),
                    ("total_stake".into(), Value::List(vec![Value::Int(0)])),
                ])),
            )?;
            Ok(())
        })?;
        watcher
            .observe(&client, &header(&chain, 120)?, &metrics)
            .await?;
        watcher
            .observe(&client, &header(&chain, 130)?, &metrics)
            .await?;
        assert_eq!(
            watcher
                .outcomes()
                .front()
                .expect("Yuma nonowner miss")
                .missed,
            Some(RevealMissReason::Permit)
        );
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn first_reveal_snapshot_attributes_version_or_deregistration_before_round()
    -> anyhow::Result<()> {
        let (chain, client, mut watcher, metrics) = slipped().await?;
        chain.mutate_state(|state| {
            seed_at(
                state,
                120,
                "SubtensorModule",
                "WeightsVersionKey",
                (NetUid(1),),
                2u64,
            )
        })?;
        watcher
            .observe(&client, &header(&chain, 120)?, &metrics)
            .await?;
        watcher
            .observe(&client, &header(&chain, 130)?, &metrics)
            .await?;
        assert_eq!(
            watcher.outcomes().front().expect("version miss").missed,
            Some(RevealMissReason::VersionKey)
        );
        client.close().await;

        let (chain, client, mut watcher, metrics) = slipped().await?;
        chain.mutate_state(|state| -> anyhow::Result<()> {
            let info = state
                .runtime
                .storage_entry("SubtensorModule", "Uids")
                .ok_or_else(|| anyhow::anyhow!("fixture Uids missing"))?;
            let key = state
                .runtime
                .storage_key(
                    info,
                    &[NetUid(1).into_core_value(), HOTKEY.into_core_value()],
                )
                .map_err(rtao_core::CoreError::from)?;
            let hash = state
                .block_hash(120)
                .ok_or_else(|| anyhow::anyhow!("fixture block missing"))?;
            state.seed_at(hash, key, None);
            Ok(())
        })?;
        watcher
            .observe(&client, &header(&chain, 120)?, &metrics)
            .await?;
        watcher
            .observe(&client, &header(&chain, 130)?, &metrics)
            .await?;
        assert_eq!(
            watcher
                .outcomes()
                .front()
                .expect("registration miss")
                .missed,
            Some(RevealMissReason::NotRegistered)
        );
        client.close().await;
        Ok(())
    }

    /// A reveal that succeeded while the native tag was stored proves nothing
    /// about the same code once root stores another tag: unreviewed code may
    /// decode by the stored version.
    #[tokio::test]
    async fn reveal_evidence_under_the_native_tag_never_follows_a_later_tag() -> anyhow::Result<()>
    {
        let (chain, client, mut watcher, metrics) = slipped().await?;
        chain.mutate_state(|state| {
            let hash = state.block_hash(120).expect("actual reveal hash");
            state
                .seed_events(hash, success_event())
                .expect("seed actual reveal");
            let parent = state.block_hash(119).expect("parent hash");
            state.seed_at(parent, b":code".to_vec(), Some(b"code X".to_vec()));
        });
        watcher
            .observe(&client, &header(&chain, 120)?, &metrics)
            .await?;
        assert_eq!(watcher.pending(), 0);
        assert!(
            watcher
                .outcomes()
                .front()
                .expect("outcome")
                .missed
                .is_none()
        );
        assert!(
            watcher.revealed_code.is_empty(),
            "the native tag needs and retains no evidence"
        );
        let code = code_hash(b"code X");
        assert!(!watcher.reveals_native_payload(code, 5));

        // Root then stores 5 under the same unreviewed code: the scheduler pauses.
        let mut scheduler = crate::weights::tests::fresh_scheduler()?;
        scheduler.reveals = watcher;
        let best = client.at(BlockRef::Best).await?;
        let mut state = WeightPreflight::fetch(&best, NetUid(1), MechId(0), HOTKEY).await?;
        state.commit_reveal_version = 5;
        state.runtime_code = Some(code);
        assert!(scheduler.commit_reveal_tag(&state).is_err());
        assert_eq!(
            scheduler.next_action(MechId(0), HOTKEY, &state)?,
            crate::weights::WeightOpportunity::Paused
        );
        client.close().await;
        Ok(())
    }

    /// An unexplained miss of a commitment tagged with a followed version
    /// refutes following it under that code: the scheduler pauses again. A
    /// miss with another explanation, or of a native-tag commitment, does not.
    #[tokio::test]
    async fn unexplained_miss_of_a_followed_tag_pauses_timelocked_commits() -> anyhow::Result<()> {
        let code = code_hash(b"code X");
        let unexplained = |chain: &rtao_testkit::FakeChain| {
            // Drand reached the round inside the window; no other reason applies.
            chain.mutate_state(|state| seed_at(state, 129, "Drand", "LastStoredRound", (), 501u64))
        };
        let (chain, client, mut watcher, metrics) = slipped_followed(5, code).await?;
        unexplained(&chain)?;
        let mut scheduler = crate::weights::tests::fresh_scheduler()?;
        let best = client.at(BlockRef::Best).await?;
        let mut state = WeightPreflight::fetch(&best, NetUid(1), MechId(0), HOTKEY).await?;
        state.commit_reveal_version = 5;
        state.runtime_code = Some(code);
        // The scheduler had followed tag 5 under this code from evidence.
        watcher.remember_revealed_code(code, 5);
        scheduler.reveals = watcher;
        assert_eq!(scheduler.commit_reveal_tag(&state)?, 5);
        for block in [120, 130] {
            scheduler
                .reveals
                .observe(&client, &header(&chain, block)?, &metrics)
                .await?;
        }
        assert_eq!(
            scheduler.reveals.outcomes().front().expect("miss").missed,
            Some(RevealMissReason::Unknown)
        );
        assert!(scheduler.reveals.native_reveal_refuted(code, 5));
        assert!(!scheduler.reveals.reveals_native_payload(code, 5));
        assert!(
            metrics
                .render()
                .contains("rtao_weights_cr_version_refuted_total 1")
        );
        let refusal = scheduler
            .commit_reveal_tag(&state)
            .expect_err("a refuted tag pauses");
        assert_eq!(refusal.name, "IncorrectCommitRevealVersion");
        assert!(
            refusal.remediation.contains("missed its reveal"),
            "{refusal}"
        );
        assert_eq!(
            scheduler.next_action(MechId(0), HOTKEY, &state)?,
            crate::weights::WeightOpportunity::Paused
        );
        // Even a reviewed runtime stops being followed after such a miss.
        let reviewed = crate::weights::REVIEWED_CR_RUNTIME_CODE[1].1;
        let mut bytes = [0u8; 32];
        hex::decode_to_slice(&reviewed[2..], &mut bytes)?;
        let reviewed = rtao_primitives::H256(bytes);
        state.runtime_code = Some(reviewed);
        assert_eq!(scheduler.commit_reveal_tag(&state)?, 5);
        scheduler.reveals.remember_missed_reveal(reviewed, 5);
        assert!(scheduler.commit_reveal_tag(&state).is_err());
        client.close().await;

        // A drand-explained miss of a followed commitment refutes nothing.
        let (chain, client, mut watcher, metrics) = slipped_followed(5, code).await?;
        for block in [120, 130] {
            watcher
                .observe(&client, &header(&chain, block)?, &metrics)
                .await?;
        }
        assert_eq!(
            watcher.outcomes().front().expect("miss").missed,
            Some(RevealMissReason::DrandRound)
        );
        assert!(watcher.revealed_code.is_empty());
        client.close().await;

        // An unexplained miss of a native-tag commitment refutes nothing.
        let (chain, client, mut watcher, metrics) = slipped().await?;
        unexplained(&chain)?;
        for block in [120, 130] {
            watcher
                .observe(&client, &header(&chain, block)?, &metrics)
                .await?;
        }
        assert_eq!(
            watcher.outcomes().front().expect("miss").missed,
            Some(RevealMissReason::Unknown)
        );
        assert!(watcher.revealed_code.is_empty());
        assert!(
            !metrics
                .render()
                .contains("rtao_weights_cr_version_refuted_total")
        );
        client.close().await;
        Ok(())
    }

    #[test]
    fn revealed_code_evidence_is_bounded_and_checkpoint_compatible() -> anyhow::Result<()> {
        // Checkpoints written before D-086 carry no evidence field.
        let old =
            serde_json::json!({"pending":[],"unlocated":[],"outcomes":[],"last_observed_block":7});
        let mut watcher: RevealWatcher = serde_json::from_value(old.clone())?;
        assert!(watcher.revealed_code.is_empty());
        assert_eq!(
            serde_json::to_value(&watcher)?,
            old,
            "empty evidence is omitted"
        );
        for byte in 0..=u8::try_from(REVEALED_CODE_LIMIT)? {
            watcher.remember_revealed_code(rtao_primitives::H256([byte; 32]), 5);
        }
        // Fresh evidence for known code moves it behind older entries.
        watcher.remember_revealed_code(rtao_primitives::H256([1; 32]), 5);
        watcher.remember_revealed_code(rtao_primitives::H256([17; 32]), 5);
        assert_eq!(watcher.revealed_code.len(), REVEALED_CODE_LIMIT);
        for (byte, retained) in [(0, false), (1, true), (2, false), (3, true), (17, true)] {
            assert_eq!(
                watcher.reveals_native_payload(rtao_primitives::H256([byte; 32]), 5),
                retained,
                "code {byte}"
            );
        }
        let restored: RevealWatcher = serde_json::from_value(serde_json::to_value(&watcher)?)?;
        assert_eq!(restored.revealed_code, watcher.revealed_code);
        // Evidence for another payload format never satisfies the native one.
        let mut other = RevealWatcher::default();
        other.revealed_code.push_back(RevealedCode {
            payload_version: 9,
            code: rtao_primitives::H256([3; 32]),
            stored_tag: Some(5),
            missed: false,
        });
        assert!(!other.reveals_native_payload(rtao_primitives::H256([3; 32]), 5));
        // Evidence recorded without its stored tag proves no tag.
        let legacy: RevealWatcher = serde_json::from_value(serde_json::json!({
            "pending": [], "unlocated": [], "outcomes": [], "last_observed_block": 7,
            "revealed_code": [{"payload_version": 4, "code": rtao_primitives::H256([3; 32])}],
        }))?;
        assert!(!legacy.reveals_native_payload(rtao_primitives::H256([3; 32]), 5));
        assert!(!legacy.native_reveal_refuted(rtao_primitives::H256([3; 32]), 5));

        // The latest outcome for a code and tag wins; refutations survive
        // eviction while positive evidence remains to evict.
        let mut watcher = RevealWatcher::default();
        let refuted = rtao_primitives::H256([200; 32]);
        watcher.remember_revealed_code(refuted, 5);
        watcher.remember_missed_reveal(refuted, 5);
        assert!(watcher.native_reveal_refuted(refuted, 5));
        assert!(!watcher.reveals_native_payload(refuted, 5));
        for byte in 0..=u8::try_from(REVEALED_CODE_LIMIT)? {
            watcher.remember_revealed_code(rtao_primitives::H256([byte; 32]), 5);
        }
        assert_eq!(watcher.revealed_code.len(), REVEALED_CODE_LIMIT);
        assert!(watcher.native_reveal_refuted(refuted, 5));
        let restored: RevealWatcher = serde_json::from_value(serde_json::to_value(&watcher)?)?;
        assert!(restored.native_reveal_refuted(refuted, 5));
        watcher.remember_revealed_code(refuted, 5);
        assert!(watcher.reveals_native_payload(refuted, 5));
        assert!(!watcher.native_reveal_refuted(refuted, 5));
        Ok(())
    }

    #[test]
    fn named_and_positional_event_fields_match_only_the_expected_mechanism_and_hotkey() {
        let event = Value::Tuple(vec![Value::Int(4097), Value::Str(HOTKEY.to_string())]);
        assert!(reveal_matches(&event, NetUidStorageIndex(4097), HOTKEY));
        assert!(!reveal_matches(&event, NetUidStorageIndex(1), HOTKEY));
        assert!(!reveal_matches(
            &event,
            NetUidStorageIndex(4097),
            AccountId([8; 32])
        ));
        assert_eq!(
            event_netuid(&Value::record(vec![("netuid".into(), Value::Int(1))])),
            Some(1)
        );
    }

    #[tokio::test]
    async fn accepted_commit_survives_a_temporary_inclusion_queue_read_failure()
    -> anyhow::Result<()> {
        let (chain, client, _, _) = slipped().await?;
        let preparation = client.at(BlockRef::Number(100)).await?;
        let state = WeightPreflight::fetch(&preparation, NetUid(1), MechId(0), HOTKEY).await?;
        let included = header(&chain, 110)?;
        let report = crate::weights::submit::WeightSubmission {
            block_hash: included.hash,
            block_number: 110,
        };
        chain.mutate_state(|state| {
            state.queue_reply(
                "state_queryStorageAt",
                rtao_testkit::Reply::error(-32603, "temporary inclusion queue error", None),
            )
        });
        let mut watcher = RevealWatcher::default();
        let metrics = Metrics::default();
        assert!(
            watcher
                .record(
                    &client,
                    NetUid(1),
                    MechId(0),
                    HOTKEY,
                    (vec![Uid(1)], vec![65535]),
                    0,
                    state,
                    PreparedWeightCommit {
                        ciphertext: vec![4, 5, 6],
                        reveal_round: 500
                    },
                    &report,
                    &metrics
                )
                .await
                .is_err()
        );
        assert_eq!(watcher.pending(), 1);
        assert_eq!(watcher.unlocated.len(), 1);
        watcher.observe(&client, &included, &metrics).await?;
        assert_eq!(watcher.unlocated.len(), 0);
        assert_eq!(watcher.pending(), 1);
        assert_eq!(watcher.pending[0].reveal_epoch, 7);
        watcher.observe(&client, &included, &metrics).await?;
        assert_eq!(
            metrics
                .render()
                .matches("rtao_weights_commit_epoch_slip_total 1")
                .count(),
            1
        );
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn finalized_history_recovers_a_reveal_emitted_during_reconciliation()
    -> anyhow::Result<()> {
        let (chain, client, mut watcher, metrics) = slipped().await?;
        chain.mutate_state(|state| {
            let hash = state.block_hash(120).expect("actual reveal hash");
            state
                .seed_events(hash, success_event())
                .expect("historical reveal event");
        });
        watcher
            .observe(&client, &header(&chain, 130)?, &metrics)
            .await?;
        assert_eq!(watcher.pending(), 0);
        assert_eq!(
            watcher
                .outcomes()
                .front()
                .expect("recovered reveal")
                .observed_block,
            120
        );
        assert!(
            watcher
                .outcomes()
                .front()
                .expect("recovered reveal")
                .missed
                .is_none()
        );
        assert!(
            metrics
                .render()
                .contains("rtao_weights_reveal_history_replayed_total")
        );
        assert!(
            !metrics
                .render()
                .contains("rtao_weights_reveal_missed_total")
        );
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn durable_restart_replays_missed_heads_and_recomputes_changed_reveal_period()
    -> anyhow::Result<()> {
        use crate::{ChainClock, ClockGuard, WeightPolicy, WeightScheduler};
        use rtao_client::journal::OperationJournal;
        use std::{path::PathBuf, sync::Arc};
        let (chain, client, watcher, metrics) = slipped().await?;
        let root = PathBuf::from(
            std::env::var_os("RTAO_TEST_TMPDIR").ok_or_else(|| anyhow::anyhow!("task scratch"))?,
        )
        .join(format!("durable-reveal-{}", std::process::id()));
        std::fs::create_dir_all(&root)?;
        let journal = OperationJournal::open(root.join("journal.json"))?;
        let mut scheduler = WeightScheduler::new(
            WeightPolicy::default(),
            Arc::new(ChainClock::new(1_000_000)),
            ClockGuard::default(),
        )?
        .with_journal(journal.clone(), client.genesis(), NetUid(1), HOTKEY)?;
        scheduler.reveals = watcher;
        scheduler.persist(None)?;
        assert!(
            WeightScheduler::new(
                WeightPolicy::default(),
                Arc::new(ChainClock::new(1_000_000)),
                ClockGuard::default()
            )?
            .with_journal(journal.clone(), client.genesis(), NetUid(1), HOTKEY)
            .is_err()
        );
        drop(scheduler);
        drop(journal);
        chain.mutate_state(|state| -> anyhow::Result<()> {
            for number in 110..130 {
                seed_at(
                    state,
                    number,
                    "SubtensorModule",
                    "RevealPeriodEpochs",
                    (NetUid(1),),
                    2u64,
                )?;
            }
            let hash = state
                .block_hash(130)
                .ok_or_else(|| anyhow::anyhow!("head"))?;
            seed_at(
                state,
                129,
                "SubtensorModule",
                "TimelockedWeightCommits",
                (NetUidStorageIndex(1), 6u64),
                vec![(HOTKEY, 110u64, vec![4u8, 5, 6], 500u64)],
            )?;
            state.seed_events(hash, success_event())?;
            Ok(())
        })?;
        let journal = OperationJournal::open(root.join("journal.json"))?;
        let mut restored = WeightScheduler::new(
            WeightPolicy::default(),
            Arc::new(ChainClock::new(1_000_000)),
            ClockGuard::default(),
        )?
        .with_journal(journal.clone(), client.genesis(), NetUid(1), HOTKEY)?;
        assert_eq!(restored.reveals.pending(), 1);
        restored
            .reveals
            .observe(&client, &header(&chain, 130)?, &metrics)
            .await?;
        assert_eq!(restored.reveals.pending(), 0);
        let outcome = restored
            .reveals
            .outcomes()
            .back()
            .ok_or_else(|| anyhow::anyhow!("reveal outcome"))?;
        assert_eq!(
            (
                outcome.commit_epoch,
                outcome.reveal_epoch,
                outcome.observed_block
            ),
            (6, 8, 130)
        );
        assert!(outcome.missed.is_none());
        restored.persist(None)?;
        restored
            .reveals
            .observe(&client, &header(&chain, 130)?, &metrics)
            .await?;
        assert_eq!(restored.reveals.outcomes().len(), 1);
        drop(restored);
        drop(journal);
        client.close().await;
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    /// Seed the schedule the chain stores at the end of `block`.
    fn seed_schedule_at(
        state: &mut rtao_testkit::FakeState,
        block: u64,
        (index, last, pending): (u64, u64, u64),
    ) -> anyhow::Result<()> {
        for (item, value) in [
            ("SubnetEpochIndex", index),
            ("LastEpochBlock", last),
            ("BlocksSinceLastStep", block - last),
            ("PendingEpochAt", pending),
        ] {
            seed_at(state, block, "SubtensorModule", item, (NetUid(1),), value)?;
        }
        Ok(())
    }

    /// Seed epochs every `tempo` blocks from `first` (index `index` there)
    /// at each block in `first..=to`.
    fn seed_epochs(
        state: &mut rtao_testkit::FakeState,
        (first, index, tempo): (u64, u64, u64),
        to: u64,
    ) -> anyhow::Result<()> {
        for block in first..=to {
            let fired = (block - first) / tempo;
            seed_schedule_at(state, block, (index + fired, first + fired * tempo, 0))?;
        }
        Ok(())
    }

    /// Seed the schedule a chain with tempo 10 stores at the end of each block
    /// in `from..=to`: an epoch every ten blocks from 100, index 5 at 100.
    fn seed_regular_schedule(
        state: &mut rtao_testkit::FakeState,
        from: u64,
        to: u64,
    ) -> anyhow::Result<()> {
        seed_epochs(state, (100, 5, 10), to)?;
        let _ = from;
        Ok(())
    }

    /// A pre-run `SubtensorModule` event record with named fields.
    fn subtensor_event(name: &str, fields: serde_json::Value) -> serde_json::Value {
        serde_json::json!({"phase":"Initialization","event":{"SubtensorModule":{name:fields}},"topics":[]})
    }

    /// A commitment included at 112 in epoch 6 of a regular chain, with its
    /// queue entry present through `kept_until` and a success event at
    /// `revealed`, if any. Returns the recorded watcher.
    async fn regular_commit(
        kept_until: u64,
        revealed: Option<u64>,
    ) -> anyhow::Result<(rtao_testkit::FakeChain, Client, RevealWatcher, Metrics)> {
        commit_with(kept_until, revealed, |_| Ok(())).await
    }

    /// [`regular_commit`] with the chain adjusted by `adjust` before the
    /// commitment is recorded.
    async fn commit_with(
        kept_until: u64,
        revealed: Option<u64>,
        adjust: impl FnOnce(&mut rtao_testkit::FakeState) -> anyhow::Result<()>,
    ) -> anyhow::Result<(rtao_testkit::FakeChain, Client, RevealWatcher, Metrics)> {
        let (chain, client) = fixture().await?;
        let commit = PreparedWeightCommit {
            ciphertext: vec![4, 5, 6],
            reveal_round: 500,
        };
        chain.mutate_state(|state| -> anyhow::Result<()> {
            state.advance(45);
            seed_regular_schedule(state, 101, 145)?;
            for block in 112..=kept_until {
                let queue = commit_queue(state, &[(HOTKEY, 112, commit.ciphertext.clone(), 500)])?;
                seed_at(
                    state,
                    block,
                    "SubtensorModule",
                    "TimelockedWeightCommits",
                    (NetUidStorageIndex(1), 6u64),
                    queue,
                )?;
            }
            if let Some(block) = revealed {
                let hash = state
                    .block_hash(block)
                    .ok_or_else(|| anyhow::anyhow!("reveal block"))?;
                state.seed_events(hash, success_event())?;
            }
            for block in 100..=145 {
                seed_at(state, block, "Drand", "LastStoredRound", (), 501u64)?;
                seed_at(
                    state,
                    block,
                    "SubtensorModule",
                    "SubnetOwnerHotkey",
                    (NetUid(1),),
                    HOTKEY,
                )?;
            }
            adjust(state)
        })?;
        let prepared = client.at(BlockRef::Number(111)).await?;
        let state = WeightPreflight::fetch(&prepared, NetUid(1), MechId(0), HOTKEY).await?;
        let report = crate::weights::submit::WeightSubmission {
            block_hash: header(&chain, 112)?.hash,
            block_number: 112,
        };
        let metrics = Metrics::default();
        let mut watcher = RevealWatcher::default();
        watcher
            .record(
                &client,
                NetUid(1),
                MechId(0),
                HOTKEY,
                (vec![Uid(1)], vec![65535]),
                0,
                state,
                commit,
                &report,
                &metrics,
            )
            .await?;
        assert_eq!(
            (
                watcher.pending[0].commit_epoch,
                watcher.pending[0].reveal_epoch
            ),
            (6, 7)
        );
        Ok((chain, client, watcher, metrics))
    }

    /// Requests `observe` makes for each head in `heads`.
    async fn observe_requests(
        chain: &rtao_testkit::FakeChain,
        client: &Client,
        watcher: &mut RevealWatcher,
        metrics: &Metrics,
        heads: impl IntoIterator<Item = u64>,
    ) -> anyhow::Result<usize> {
        let before = chain.requests().len();
        for head in heads {
            watcher
                .observe(client, &header(chain, head)?, metrics)
                .await?;
        }
        Ok(chain.requests().len() - before)
    }

    /// Requests the watcher makes against a regular chain (tempo 10, a
    /// commitment included at 112, its reveal window opening at 120), measured
    /// on this fixture before and after D-096, which reads only the blocks
    /// that can change the outcome:
    ///
    /// | case | before | after |
    /// |---|---|---|
    /// | live, one head per call, 112..=120 | 55 | 30 |
    /// | live, head snapshot already resolved (the scheduler's path) | 47 | 13 |
    /// | restart, one call replaying to 125 | 82 | 12 |
    /// | dropped at 120, window ends at 130 (calls at 125, 131) | 182 | 29 |
    /// | drand lag, revealed at 124 (one call to 126) | 124 | 31 |
    #[tokio::test]
    async fn reveal_watcher_request_budget() -> anyhow::Result<()> {
        let (chain, client, mut watcher, metrics) = regular_commit(119, Some(120)).await?;
        let live = observe_requests(&chain, &client, &mut watcher, &metrics, 112..=120).await?;
        let outcome = watcher.outcomes().back().cloned().expect("revealed");
        assert_eq!((outcome.observed_block, outcome.missed), (120, None));
        assert_eq!(live, 30);
        client.close().await;

        let (chain, client, mut watcher, metrics) = regular_commit(119, Some(120)).await?;
        let mut supplied = 0;
        for head in 112..=120 {
            let header = header(&chain, head)?;
            let snapshot = client.at(BlockRef::Hash(header.hash)).await?;
            let before = chain.requests().len();
            watcher
                .observe_at(&client, &header, Some(snapshot), &metrics)
                .await?;
            supplied += chain.requests().len() - before;
        }
        let outcome = watcher.outcomes().back().cloned().expect("revealed");
        assert_eq!((outcome.observed_block, outcome.missed), (120, None));
        assert_eq!(supplied, 13);
        client.close().await;

        let (chain, client, mut watcher, metrics) = regular_commit(119, Some(120)).await?;
        let replay = observe_requests(&chain, &client, &mut watcher, &metrics, [125]).await?;
        let outcome = watcher.outcomes().back().cloned().expect("revealed");
        assert_eq!((outcome.observed_block, outcome.missed), (120, None));
        assert!(watcher.caught_up(125));
        assert_eq!(replay, 12);
        client.close().await;

        let (chain, client, mut watcher, metrics) = regular_commit(119, None).await?;
        let missed = observe_requests(&chain, &client, &mut watcher, &metrics, [125, 131]).await?;
        let outcome = watcher.outcomes().back().cloned().expect("missed");
        assert_eq!(
            (outcome.observed_block, outcome.missed),
            (130, Some(RevealMissReason::Unknown))
        );
        assert!(watcher.caught_up(131));
        assert_eq!(missed, 29);
        client.close().await;

        let (chain, client, mut watcher, metrics) = regular_commit(123, Some(124)).await?;
        let lagged = observe_requests(&chain, &client, &mut watcher, &metrics, [126]).await?;
        let outcome = watcher.outcomes().back().cloned().expect("revealed late");
        assert_eq!((outcome.observed_block, outcome.missed), (124, None));
        assert_eq!(lagged, 31);
        client.close().await;
        Ok(())
    }

    /// An owner trigger at 115 makes the epoch fire at 118, two blocks early.
    /// No epoch is due at 115, yet the stored `PendingEpochAt` differs from
    /// the prediction, so bisection reads that block and keeps its notice:
    /// the dropped commitment's miss is `EpochTriggered`, not an unexplained
    /// miss that would refute a followed commit-reveal tag.
    #[tokio::test]
    async fn owner_trigger_between_epochs_is_located_and_explains_the_miss() -> anyhow::Result<()> {
        let (chain, client, mut watcher, metrics) = commit_with(117, None, |state| {
            for block in 115..=117 {
                seed_schedule_at(state, block, (6, 110, 118))?;
            }
            seed_epochs(state, (118, 7, 10), 145)?;
            let hash = state
                .block_hash(115)
                .ok_or_else(|| anyhow::anyhow!("trigger block"))?;
            state.seed_events(
                hash,
                serde_json::json!([subtensor_event(
                    "EpochTriggered",
                    serde_json::json!({"netuid": 1, "by": HOTKEY.to_string(), "fires_at": 118}),
                )]),
            )?;
            Ok(())
        })
        .await?;
        for head in [113, 116, 119, 124, 129] {
            watcher
                .observe(&client, &header(&chain, head)?, &metrics)
                .await?;
        }
        let outcome = watcher.outcomes().back().cloned().expect("missed");
        assert_eq!(
            (outcome.reveal_epoch, outcome.observed_block, outcome.missed),
            (7, 128, Some(RevealMissReason::EpochTriggered))
        );
        assert_eq!(outcome.epoch_notices, [EpochNotice::Triggered(115)]);
        assert!(watcher.caught_up(129));
        client.close().await;
        Ok(())
    }

    /// An epoch deferred from 120 to 121 opens the window at 120 (reveal
    /// processing looks ahead) and closes it a block later, at 131. The
    /// deferral notice is kept with the outcome.
    #[tokio::test]
    async fn deferred_epoch_keeps_its_notice_and_moves_the_window_end() -> anyhow::Result<()> {
        let (chain, client, mut watcher, metrics) = commit_with(119, None, |state| {
            seed_schedule_at(state, 120, (6, 110, 121))?;
            seed_epochs(state, (121, 7, 10), 145)?;
            let hash = state
                .block_hash(120)
                .ok_or_else(|| anyhow::anyhow!("deferred block"))?;
            state.seed_events(
                hash,
                serde_json::json!([subtensor_event(
                    "EpochDeferred",
                    serde_json::json!({"netuid": 1, "from_block": 120, "to_block": 121}),
                )]),
            )?;
            Ok(())
        })
        .await?;
        for head in [125, 130, 132] {
            watcher
                .observe(&client, &header(&chain, head)?, &metrics)
                .await?;
        }
        let outcome = watcher.outcomes().back().cloned().expect("missed");
        assert_eq!(
            (outcome.observed_block, outcome.missed),
            (131, Some(RevealMissReason::Unknown))
        );
        assert_eq!(outcome.epoch_notices, [EpochNotice::Deferred(120)]);
        client.close().await;
        Ok(())
    }

    /// A stored counter the ported schedule cannot predict (it never
    /// advances) breaks every prediction. The watcher then reads each block,
    /// still observes the reveal at 120, and makes fewer requests than reading
    /// every block did before D-096 (82 on this fixture).
    #[tokio::test]
    async fn unpredictable_schedule_falls_back_to_reading_each_block() -> anyhow::Result<()> {
        let (chain, client, mut watcher, metrics) = commit_with(119, Some(120), |state| {
            for block in 101..=145 {
                seed_at(
                    state,
                    block,
                    "SubtensorModule",
                    "BlocksSinceLastStep",
                    (NetUid(1),),
                    0u64,
                )?;
            }
            Ok(())
        })
        .await?;
        let requests = observe_requests(&chain, &client, &mut watcher, &metrics, [125]).await?;
        let outcome = watcher.outcomes().back().cloned().expect("revealed");
        assert_eq!((outcome.observed_block, outcome.missed), (120, None));
        assert!(watcher.caught_up(125));
        assert!(requests < 82, "{requests} requests");
        client.close().await;
        Ok(())
    }

    /// A tempo shortened at 113 leaves no event, but the stored schedule no
    /// longer matches the prediction: bisection finds 113, and the earlier
    /// window's reveal is observed at 115.
    #[tokio::test]
    async fn changed_tempo_is_bisected_and_the_early_reveal_is_observed() -> anyhow::Result<()> {
        let (chain, client, mut watcher, metrics) = commit_with(114, Some(115), |state| {
            for block in 113..=145 {
                seed_at(state, block, "SubtensorModule", "Tempo", (NetUid(1),), 5u16)?;
            }
            seed_epochs(state, (115, 7, 5), 145)
        })
        .await?;
        let requests = observe_requests(&chain, &client, &mut watcher, &metrics, [118]).await?;
        let outcome = watcher.outcomes().back().cloned().expect("revealed");
        assert_eq!(
            (outcome.reveal_epoch, outcome.observed_block, outcome.missed),
            (7, 115, None)
        );
        assert!(watcher.caught_up(118));
        assert!(requests < 40, "{requests} requests");
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn account_only_event_cannot_disambiguate_two_commitments_and_queued_work_is_not_expired()
    -> anyhow::Result<()> {
        let (chain, client, mut watcher, metrics) = slipped().await?;
        chain.mutate_state(|state| -> anyhow::Result<()> {
            seed_at(
                state,
                119,
                "SubtensorModule",
                "TimelockedWeightCommits",
                (NetUidStorageIndex(1), 6u64),
                vec![
                    (HOTKEY, 110u64, vec![4u8, 5, 6], 500u64),
                    (HOTKEY, 111u64, vec![7u8], 501u64),
                ],
            )?;
            let hash = state
                .block_hash(120)
                .ok_or_else(|| anyhow::anyhow!("head"))?;
            state.seed_events(hash, success_event())?;
            seed_at(
                state,
                130,
                "SubtensorModule",
                "TimelockedWeightCommits",
                (NetUidStorageIndex(1), 6u64),
                vec![(HOTKEY, 110u64, vec![4u8, 5, 6], 500u64)],
            )?;
            Ok(())
        })?;
        watcher
            .observe(&client, &header(&chain, 120)?, &metrics)
            .await?;
        assert_eq!(watcher.pending(), 1);
        assert!(watcher.outcomes().is_empty());
        assert!(
            watcher.revealed_code.is_empty(),
            "an ambiguous reveal is not evidence for this validator's payload"
        );
        // A current queue proves retention despite the predicted window ending.
        watcher.last_observed_block = Some(129);
        watcher
            .observe(&client, &header(&chain, 130)?, &metrics)
            .await?;
        assert_eq!(watcher.pending(), 1);
        assert!(watcher.outcomes().is_empty());
        client.close().await;
        Ok(())
    }
}

/// Historical client projection sufficient to locate an already signed commitment.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RevealPreparation {
    prepared_block: u64,
    uid: Option<Uid>,
    reveal_period: u64,
    schedule_fields: (u16, u64, u64, u64, u64, u64),
    /// Present only for a commit tagged with a followed non-native version,
    /// so native journal material keeps its bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    followed_cr: Option<FollowedCr>,
}
impl From<&WeightPreflight> for RevealPreparation {
    fn from(state: &WeightPreflight) -> Self {
        let s = &state.schedule;
        Self {
            prepared_block: state.at.number,
            uid: state.uid,
            reveal_period: state.reveal_period,
            schedule_fields: (
                s.tempo,
                s.last_epoch_block,
                s.pending_epoch_at,
                s.subnet_epoch_index,
                s.blocks_since_last_step,
                s.current_block,
            ),
            // The scheduler dispatches a non-native tag only after following
            // it under this preflight's code (D-086).
            followed_cr: (state.commit_reveal
                && state.commit_reveal_version != super::NATIVE_CR_PAYLOAD_VERSION)
                .then_some(state.runtime_code)
                .flatten()
                .map(|code| FollowedCr {
                    tag: state.commit_reveal_version,
                    code,
                }),
        }
    }
}
impl RevealPreparation {
    fn schedule(&self) -> rtao_core::timelock::epoch_schedule::EpochScheduleState {
        let (
            tempo,
            last_epoch_block,
            pending_epoch_at,
            subnet_epoch_index,
            blocks_since_last_step,
            current_block,
        ) = self.schedule_fields;
        rtao_core::timelock::epoch_schedule::EpochScheduleState {
            tempo,
            last_epoch_block,
            pending_epoch_at,
            subnet_epoch_index,
            blocks_since_last_step,
            current_block,
        }
    }
}
