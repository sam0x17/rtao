// Derived from RaoFoundation/subtensor pallets/subtensor/src/utils/misc.rs @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor pallets/subtensor/src/subnets/weights.rs @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Coherent best-head weight preparation and actual-epoch reveal tracking.

mod durable;
mod reveal;
mod submit;
#[cfg(test)]
mod tests;

use crate::{Clock, ClockGuard, NeuronCx, StateCell, Validator};
use blake2::{Blake2b, Digest, digest::consts::U32};
use futures::StreamExt;
pub use reveal::{EpochNotice, PendingReveal, RevealMissReason, RevealOutcome, RevealWatcher};
use rtao_client::{BlockHeader, BlockRef, Client, Finality, ReadPlan, Snapshot, ViewExt};
use rtao_core::{DynValue, codec::Value, timelock::epoch_schedule::EpochScheduleState};
use rtao_intents::weights::{
    CommitSizeLimit, ConformState, TimelockCommitRequest, TimelockCommitter, conform,
};
use rtao_primitives::{AccountId, IntoCoreValue, MechId, NetUid, NetUidStorageIndex, Uid};
use rtao_runtime::storage::subtensor_module as st;
use rtao_signer::KeyView;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

/// Commit-reveal payload version supported by this build.
pub const SUPPORTED_CR_VERSION: u16 = 4;

/// Pure scoring result, evaluated only after one coherent preflight.
#[derive(Clone, Debug, PartialEq)]
pub enum WeightsProposal {
    /// Prepare and submit a new row.
    Set(WeightsInput),
    /// Stable application work. Repeated callback delivery uses the same ID.
    Operation {
        /// Application operation identity; never reuse it for changed input.
        id: String,
        /// Opaque application correlation identity.
        correlation_id: Option<String>,
        /// Intended row, retained across missed epochs and restart.
        input: WeightsInput,
    },
    /// Retain the previous chain row.
    HoldPrevious,
    /// Skip this opportunity with an operator-visible reason.
    Skip(&'static str),
}

/// Destinations and scores accepted by a validator's pure weights callback.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum WeightsInput {
    /// Deterministically ordered UID-to-score mapping.
    ByUid(BTreeMap<Uid, f64>),
    /// Parallel destination and relative-score vectors.
    Parallel {
        /// Destinations in submission order.
        uids: Vec<Uid>,
        /// Finite, nonnegative relative scores.
        weights: Vec<f64>,
    },
    /// Resolve account identities using the current last-good graph.
    ByHotkey(Vec<(AccountId, f64)>),
    /// Preserve every integer UID/value pair under a sole-permit Null consensus proof.
    /// Chain count and configured share limits apply without scaling or dropping zeros.
    RawU16 {
        /// Destinations in submission order.
        uids: Vec<Uid>,
        /// Chain-native integer ratios.
        values: Vec<u16>,
    },
}

/// Explicit weight version; never follows storage and defeats upgrade policy.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VersionKey {
    /// Python v11's default.
    #[default]
    Zero,
    /// Operator-selected version supported by the scoring implementation.
    Fixed(u64),
}
impl VersionKey {
    /// Value carried by both direct rows and encrypted payloads.
    pub const fn value(self) -> u64 {
        match self {
            Self::Zero => 0,
            Self::Fixed(value) => value,
        }
    }
}

/// Weight opportunity cadence, independent of the scoring callback cadence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WeightCadence {
    /// One attempt in each actual subnet epoch after this offset.
    OncePerEpoch {
        /// Minimum blocks after the observed epoch boundary.
        offset_blocks: u64,
    },
    /// Wait for the chain rate limit plus an operator margin.
    RateLimited {
        /// Extra blocks beyond the chain rate limit.
        margin: u64,
    },
}

/// Validator-specific submission policy.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WeightPolicy {
    /// Explicit supported weight version.
    pub version_key: VersionKey,
    /// Override the default CR epoch jitter or direct-row rate limit.
    pub cadence: Option<WeightCadence>,
}

/// Certain preflight rejection with a shared semantic code and remediation.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{name}: {remediation}")]
pub struct WeightRefusal {
    /// Semantic chain-error category.
    pub code: rtao_catalog::ErrorCode,
    /// Specific failed check.
    pub name: &'static str,
    /// Action needed before another attempt can succeed.
    pub remediation: String,
}

/// Observable result of the latest mechanism scheduling opportunity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WeightAttemptOutcome {
    /// Cadence, rate limit or unresolved submission prevented work.
    Scheduled,
    /// The chain rate limit schedules this row without marking readiness failed.
    RateLimited {
        /// Earliest best height allowed by the current chain limit.
        retry_after_block: u64,
        /// Explanation for the next scheduling opportunity.
        remediation: String,
    },
    /// The application or self/owner guard retained the existing row.
    HoldPrevious,
    /// The application supplied a reason to skip this opportunity.
    Skipped(String),
    /// A certain chain rejection was detected before signing.
    Refused(WeightRefusal),
    /// Runtime call shape changed; this mechanism resumes after verification.
    PausedSurface {
        /// Full name of the incompatible descriptor.
        descriptor: String,
        /// Operator remediation and structural difference.
        detail: String,
    },
    /// The row reached finalized inclusion.
    Submitted {
        /// Immutable finalized inclusion identity.
        block_hash: rtao_primitives::H256,
        /// Inclusion height.
        block_number: u64,
    },
    /// Fate remains unknown and requires reconciliation before another row.
    Unresolved(String),
    /// RPC, preparation or a definite submission failed.
    Failed(String),
}
impl WeightAttemptOutcome {
    /// Shared semantic category for an actionable refusal or scheduled rate limit.
    pub fn code(&self) -> Option<rtao_catalog::ErrorCode> {
        match self {
            Self::Refused(refusal) => Some(refusal.code),
            Self::RateLimited { .. } => Some(rtao_catalog::ErrorCode::RateLimited),
            Self::PausedSurface { .. } => Some(rtao_catalog::ErrorCode::PolicyViolation),
            Self::Unresolved(_) | Self::Failed(_) => Some(rtao_catalog::ErrorCode::Internal),
            Self::Scheduled | Self::HoldPrevious | Self::Skipped(_) | Self::Submitted { .. } => {
                None
            }
        }
    }
}

/// Retained scheduling result available without inspecting tracing logs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WeightAttempt {
    /// Mechanism evaluated by the scheduler.
    pub mechanism: MechId,
    /// Best height used for the coherent preflight.
    pub best_block: u64,
    /// Latest scheduling or finalized-submission outcome.
    pub outcome: WeightAttemptOutcome,
}

/// Next scheduling action under one observed chain view, not signing authority.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WeightOpportunity {
    /// Cadence permits preparation; runtime, policy and semantic checks still apply.
    Ready,
    /// New submissions are paused; continue finalized observation.
    Paused,
    /// Reconcile a retained attempt or continue bounded inclusion/reveal observation.
    Reconcile,
    /// Recheck chain limits at or after this best height.
    RateLimited {
        /// Earliest height under the currently observed chain limit.
        retry_after_block: u64,
    },
    /// Wait for an observed epoch newer than this one, then apply its offset.
    /// No wall-clock guarantee is implied by a predicted epoch boundary.
    AwaitEpoch {
        /// Last epoch already used by this scheduler.
        after_epoch: u64,
        /// Required elapsed blocks inside a newly observed epoch.
        offset_blocks: u64,
    },
    /// Recheck the offset at or after this best height.
    AwaitBlock {
        /// Earliest height under the currently observed epoch state.
        not_before_block: u64,
    },
}
fn refuse(
    code: rtao_catalog::ErrorCode,
    name: &'static str,
    remediation: impl Into<String>,
) -> anyhow::Error {
    WeightRefusal {
        code,
        name,
        remediation: remediation.into(),
    }
    .into()
}

/// The seventeen storage fields used by one mechanism's preflight.
#[derive(Clone, Debug)]
pub struct WeightPreflight {
    /// Block hash and best height used for every field and the predictor.
    pub at: rtao_client::PinnedBlock,
    /// Current UID, absent when the hotkey is unregistered.
    pub uid: Option<Uid>,
    /// Per-UID last updates for this mechanism.
    pub last_updates: Vec<u64>,
    /// Minimum interval between weight submissions.
    pub rate_limit: u64,
    /// Select the timelocked path rather than direct mechanism weights.
    pub commit_reveal: bool,
    /// Runtime's required client payload version.
    pub commit_reveal_version: u16,
    /// Number of epochs between commitment and reveal.
    pub reveal_period: u64,
    /// Runtime's forced-upgrade key.
    pub required_version_key: u64,
    /// Required count before population capping.
    pub min_allowed_weights: u16,
    /// Maximum relative share, represented over 65535.
    pub max_weights_limit: u16,
    /// Current registered population.
    pub population: u16,
    /// Whether consensus is Null rather than Yuma.
    pub null_consensus: bool,
    /// Current validator-permit vector.
    pub permits: Vec<bool>,
    /// Coherent schedule at the pinned best head.
    pub schedule: EpochScheduleState,
}

impl WeightPreflight {
    /// Fetch all seventeen fields using one `state_queryStorageAt` at `snapshot`.
    pub async fn fetch(
        snapshot: &Snapshot,
        netuid: NetUid,
        mech: MechId,
        hotkey: AccountId,
    ) -> anyhow::Result<Self> {
        let index = NetUidStorageIndex::new(netuid, mech)?;
        let net = vec![netuid.into_core_value()];
        let mut plan = ReadPlan::new();
        plan.push(
            st::UIDS.surface(),
            vec![netuid.into_core_value(), hotkey.into_core_value()],
        );
        plan.push(st::LAST_UPDATE.surface(), vec![index.into_core_value()]);
        plan.push(st::WEIGHTS_SET_RATE_LIMIT.surface(), net.clone());
        plan.push(st::COMMIT_REVEAL_WEIGHTS_ENABLED.surface(), net.clone());
        plan.push(st::COMMIT_REVEAL_WEIGHTS_VERSION.surface(), Vec::new());
        for surface in [
            st::REVEAL_PERIOD_EPOCHS.surface(),
            st::WEIGHTS_VERSION_KEY.surface(),
            st::MIN_ALLOWED_WEIGHTS.surface(),
            st::MAX_WEIGHTS_LIMIT.surface(),
            st::SUBNETWORK_N.surface(),
            st::SUBNET_EPOCH_CONSENSUS.surface(),
            st::VALIDATOR_PERMIT.surface(),
            st::TEMPO.surface(),
            st::LAST_EPOCH_BLOCK.surface(),
            st::PENDING_EPOCH_AT.surface(),
            st::SUBNET_EPOCH_INDEX.surface(),
            st::BLOCKS_SINCE_LAST_STEP.surface(),
        ] {
            plan.push(surface, net.clone());
        }
        let values = snapshot.fetch(plan).await?;
        anyhow::ensure!(
            values.len() == 17,
            "weight preflight omitted storage fields"
        );
        let integer = |index, name| number(&values[index], name);
        let uid = if matches!(values[0].as_core(), Value::Null) {
            None
        } else {
            Some(Uid(u16::try_from(integer(0, "Uids")?)?))
        };
        Ok(Self {
            at: snapshot.block().clone(),
            uid,
            last_updates: integers(&values[1], "LastUpdate")?,
            rate_limit: integer(2, "WeightsSetRateLimit")?,
            commit_reveal: boolean(&values[3], "CommitRevealWeightsEnabled")?,
            commit_reveal_version: u16::try_from(integer(4, "CommitRevealWeightsVersion")?)?,
            reveal_period: integer(5, "RevealPeriodEpochs")?,
            required_version_key: integer(6, "WeightsVersionKey")?,
            min_allowed_weights: u16::try_from(integer(7, "MinAllowedWeights")?)?,
            max_weights_limit: u16::try_from(integer(8, "MaxWeightsLimit")?)?,
            population: u16::try_from(integer(9, "SubnetworkN")?)?,
            null_consensus: consensus_null(&values[10])?,
            permits: booleans(&values[11], "ValidatorPermit")?,
            schedule: EpochScheduleState {
                tempo: u16::try_from(integer(12, "Tempo")?)?,
                last_epoch_block: integer(13, "LastEpochBlock")?,
                pending_epoch_at: integer(14, "PendingEpochAt")?,
                subnet_epoch_index: integer(15, "SubnetEpochIndex")?,
                blocks_since_last_step: integer(16, "BlocksSinceLastStep")?,
                current_block: snapshot.block().number,
            },
        })
    }

    /// Check certain registration, Null permit and client upgrade refusals.
    pub fn check(&self, version_key: u64) -> anyhow::Result<Uid> {
        let uid = self.uid.ok_or_else(|| {
            refuse(
                rtao_catalog::ErrorCode::NotRegistered,
                "HotKeyNotRegisteredInSubNet",
                "register this hotkey in the configured subnet",
            )
        })?;
        if self.null_consensus
            && !self
                .permits
                .get(usize::from(uid.0))
                .copied()
                .unwrap_or(false)
        {
            return Err(refuse(
                rtao_catalog::ErrorCode::NotAuthorized,
                "NeuronNoValidatorPermit",
                "Null consensus requires this hotkey to hold the validator permit",
            ));
        }
        if self.commit_reveal && self.commit_reveal_version != SUPPORTED_CR_VERSION {
            return Err(refuse(
                rtao_catalog::ErrorCode::InvalidArgument,
                "IncorrectCommitRevealVersion",
                format!(
                    "the chain requires commit-reveal client version {}; this rtao build supports {SUPPORTED_CR_VERSION}; upgrade rtao",
                    self.commit_reveal_version
                ),
            ));
        }
        if version_key < self.required_version_key {
            return Err(refuse(
                rtao_catalog::ErrorCode::InvalidArgument,
                "IncorrectWeightVersionKey",
                format!(
                    "the chain requires weight version {}; configure a scoring implementation that supports this version",
                    self.required_version_key
                ),
            ));
        }
        Ok(uid)
    }
    fn last_update(&self, uid: Uid) -> u64 {
        self.last_updates
            .get(usize::from(uid.0))
            .copied()
            .unwrap_or(0)
    }
    fn activity_alarm_cutoff(&self, uid: Uid, activity_factor: u32) -> Option<u64> {
        let cutoff =
            u64::from(activity_factor).saturating_mul(u64::from(self.schedule.tempo)) / 1000;
        let cutoff = cutoff.max(1);
        (self.at.number.saturating_sub(self.last_update(uid)) >= cutoff.saturating_mul(4) / 5)
            .then_some(cutoff)
    }
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Slot {
    last_attempt_epoch: Option<u64>,
    last_attempt_block: Option<u64>,
    uncertain: bool,
}

/// Advisory conditions currently reported for one mechanism. Each warning is
/// logged when its condition starts and again only after it clears, so a
/// persistent condition does not repeat on every block; metrics still count
/// every occurrence.
#[derive(Default)]
struct Advisories {
    permit_absent: bool,
    stake_below_threshold: bool,
    activity_alarm: bool,
}

/// Record whether an advisory holds and report whether it just started.
fn advisory_started(reported: &mut bool, holds: bool) -> bool {
    let started = holds && !*reported;
    *reported = holds;
    started
}

/// One scheduler owns cadence slots and outstanding commits for a validator.
pub struct WeightScheduler {
    policy: WeightPolicy,
    clock: Arc<dyn Clock>,
    clock_guard: ClockGuard,
    slots: BTreeMap<MechId, Slot>,
    attempts: BTreeMap<MechId, WeightAttempt>,
    advisories: BTreeMap<MechId, Advisories>,
    reveals: RevealWatcher,
    durable: Option<durable::Durable>,
    semantic_policy: Option<String>,
    semantic_guard: Option<Arc<dyn rtao_client::journal::SemanticGuard>>,
}
impl WeightScheduler {
    /// Construct a scheduler with explicit wall time and a best-head skew guard.
    pub fn new(
        policy: WeightPolicy,
        clock: Arc<dyn Clock>,
        clock_guard: ClockGuard,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            policy,
            clock,
            clock_guard,
            slots: BTreeMap::new(),
            attempts: BTreeMap::new(),
            advisories: BTreeMap::new(),
            reveals: RevealWatcher::default(),
            durable: None,
            semantic_policy: None,
            semantic_guard: None,
        })
    }
    /// Borrow retained reveal outcomes and outstanding commitment state.
    pub fn reveals(&self) -> &RevealWatcher {
        &self.reveals
    }
    /// Replace operator policy while preserving uncertainty and reveal tracking.
    pub fn update_policy(&mut self, policy: WeightPolicy) {
        self.policy = policy;
    }
    /// Inspect the latest mechanism outcome and any actionable refusal.
    pub fn last_attempt(&self, mechanism: MechId) -> Option<&WeightAttempt> {
        self.attempts.get(&mechanism)
    }
    /// Inspect scheduling using a fresh coherent preflight, without submitting.
    /// Recompute after every relevant parameter change; this is not a future
    /// chain guarantee or a replacement for runtime/policy/semantic checks.
    pub fn next_action(
        &self,
        mechanism: MechId,
        hotkey: AccountId,
        state: &WeightPreflight,
    ) -> anyhow::Result<WeightOpportunity> {
        if let Some(d) = &self.durable {
            anyhow::ensure!(hotkey == d.account, "scheduler account mismatch");
            if d.journal.snapshot()?.control.submissions_paused {
                return Ok(WeightOpportunity::Paused);
            }
        }
        let pending = self.pending_work(mechanism)?;
        if self.reveals.pending_for(mechanism) >= 10
            || self.slots.get(&mechanism).is_some_and(|s| s.uncertain)
            || pending
                .as_ref()
                .is_some_and(|(b, _)| b.can_dispatch().is_err())
        {
            return Ok(WeightOpportunity::Reconcile);
        }
        let version = pending
            .as_ref()
            .map_or(self.policy.version_key.value(), |(_, w)| w.version_key);
        let uid = state.check(version)?;
        let cadence = self
            .policy
            .cadence
            .unwrap_or_else(|| default_cadence(hotkey, state.schedule.tempo, state.commit_reveal));
        let empty = Slot::default();
        Ok(cadence_opportunity(
            cadence,
            self.slots.get(&mechanism).unwrap_or(&empty),
            state,
            uid,
        ))
    }
    /// Clear a legacy in-memory uncertainty block after external reconciliation.
    ///
    /// The scheduler normally reconciles through the executor. An unresolved
    /// result remains blocked so the same pool provides-tag cannot be duplicated.
    /// Durable schedulers ignore this toggle and require retained chain evidence.
    pub fn resolve_uncertainty(&mut self, mechanism: MechId) {
        // A manual memory toggle cannot authorize replacement of durable bytes.
        if self.durable.is_some() {
            return;
        }
        if let Some(slot) = self.slots.get_mut(&mechanism) {
            slot.uncertain = false;
        }
    }

    /// Observe a finalized head, then prepare due proposals at one fresh best head.
    pub async fn on_finalized(
        &mut self,
        cx: &NeuronCx,
        finalized: &BlockHeader,
        mut proposals: impl FnMut(MechId) -> WeightsProposal,
    ) -> anyhow::Result<()> {
        if let Some(d) = &self.durable {
            anyhow::ensure!(
                cx.netuid == d.netuid && cx.hotkey.hotkey_address()? == d.account,
                "scheduler context differs from durable identity"
            );
        }
        self.recover_finalized(&cx.client, finalized, &cx.metrics)
            .await?;
        if !self.reveals.caught_up(finalized.number) {
            return Ok(());
        }
        if let Some(durable) = &self.durable
            && durable.journal.snapshot()?.control.submissions_paused
        {
            return Ok(());
        }
        if cx.shutdown.is_cancelled() {
            return Ok(());
        }
        let snapshot = cx.client.at(BlockRef::Best).await?;
        let hotkey = cx.hotkey.hotkey_address()?;
        let auxiliary = snapshot
            .fetch((
                st::MECHANISM_COUNT_CURRENT.at((cx.netuid,)),
                st::ACTIVITY_CUTOFF_FACTOR_MILLI.at((cx.netuid,)),
                st::STAKE_THRESHOLD.at(()),
                st::SUBNET_OWNER_HOTKEY.at((cx.netuid,)),
            ))
            .await?;
        let mechanisms = auxiliary.0.0;
        anyhow::ensure!(
            (1..=16).contains(&mechanisms),
            "runtime mechanism count is outside 1..=16"
        );
        let owner = auxiliary.3.raw();
        for mechanism in 0..mechanisms {
            let mech = MechId(mechanism);
            let preflight = WeightPreflight::fetch(&snapshot, cx.netuid, mech, hotkey).await?;
            let best_block = preflight.at.number;
            if let Err(error) = self
                .attempt(
                    cx,
                    hotkey,
                    *owner,
                    auxiliary.1,
                    auxiliary.2,
                    mechanisms,
                    mech,
                    preflight,
                    &mut proposals,
                )
                .await
            {
                let outcome = if let Some(refusal) = error.downcast_ref::<WeightRefusal>() {
                    WeightAttemptOutcome::Refused(refusal.clone())
                } else if submit::uncertain(&error) {
                    WeightAttemptOutcome::Unresolved(error.to_string())
                } else {
                    WeightAttemptOutcome::Failed(error.to_string())
                };
                self.attempts.insert(
                    mech,
                    WeightAttempt {
                        mechanism: mech,
                        best_block,
                        outcome,
                    },
                );
                cx.metrics.increment("rtao_weights_errors_total");
                cx.metrics
                    .update_readiness(|ready| ready.last_weight_succeeded = false);
                tracing::warn!(mechanism = mech.0, error = %error, "weight attempt failed");
            }
            if cx.shutdown.is_cancelled() {
                break;
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn attempt(
        &mut self,
        cx: &NeuronCx,
        hotkey: AccountId,
        owner: AccountId,
        activity_factor: u32,
        stake_threshold: u64,
        mechanisms: u8,
        mech: MechId,
        preflight: WeightPreflight,
        proposals: &mut impl FnMut(MechId) -> WeightsProposal,
    ) -> anyhow::Result<()> {
        if self.slots.get(&mech).is_some_and(|slot| slot.uncertain) {
            return Ok(());
        }
        if self.reveals.pending_for(mech) >= 10 {
            cx.metrics.increment("rtao_weights_pending_limit_total");
            return Ok(());
        }
        self.attempts.insert(
            mech,
            WeightAttempt {
                mechanism: mech,
                best_block: preflight.at.number,
                outcome: WeightAttemptOutcome::Scheduled,
            },
        );
        let call = if preflight.commit_reveal {
            &rtao_runtime::calls::subtensor_module::COMMIT_TIMELOCKED_MECHANISM_WEIGHTS
        } else {
            &rtao_runtime::calls::subtensor_module::SET_MECHANISM_WEIGHTS
        };
        let descriptor = format!("call:{}.{}", call.pallet, call.name);
        let report = preflight.at.runtime.surface_report();
        if !report.shape_matches(&descriptor) {
            let difference = report
                .changes
                .iter()
                .filter(|change| change.descriptor == descriptor)
                .map(|change| change.detail.as_str())
                .collect::<Vec<_>>()
                .join("; ");
            self.attempts.insert(mech, WeightAttempt { mechanism: mech, best_block: preflight.at.number,
                outcome: WeightAttemptOutcome::PausedSurface { descriptor, detail: format!("weight call is incompatible with this rtao build; update rtao and re-verify the runtime: {difference}") } });
            cx.metrics.increment("rtao_weights_surface_paused_total");
            cx.metrics.update_readiness(|ready| {
                ready.runtime_verified = false;
                ready.last_weight_succeeded = false;
            });
            return Ok(());
        }
        let pending_work = self.pending_work(mech)?;
        let version_key = pending_work
            .as_ref()
            .map_or(self.policy.version_key.value(), |(_, work)| {
                work.version_key
            });
        let uid = match preflight.check(version_key) {
            Ok(uid) => uid,
            Err(error) => {
                if preflight.commit_reveal
                    && preflight.commit_reveal_version != SUPPORTED_CR_VERSION
                {
                    cx.metrics
                        .increment("rtao_weights_cr_version_mismatch_total");
                }
                return Err(error);
            }
        };
        let advisories = self.advisories.entry(mech).or_default();
        let permit_absent = !preflight.null_consensus
            && !preflight
                .permits
                .get(usize::from(uid.0))
                .copied()
                .unwrap_or(false);
        if advisory_started(&mut advisories.permit_absent, permit_absent) {
            tracing::warn!(
                mechanism = mech.0,
                "Yuma validator permit is absent; chain checks remain authoritative"
            );
        }
        let graph = cx.metagraph.load();
        let stake_below_threshold = hotkey != owner
            && graph
                .by_hotkey(&hotkey)
                .and_then(|neuron| neuron.total_stake().ok())
                .is_some_and(|stake| stake.0 < stake_threshold);
        if advisory_started(&mut advisories.stake_below_threshold, stake_below_threshold) {
            tracing::warn!(
                stake_threshold,
                "last-good stake is below the chain threshold; chain checks remain authoritative"
            );
        }
        let activity_cutoff = preflight.activity_alarm_cutoff(uid, activity_factor);
        if activity_cutoff.is_some() {
            cx.metrics.increment("rtao_weights_activity_alarm_total");
        }
        if advisory_started(&mut advisories.activity_alarm, activity_cutoff.is_some())
            && let Some(cutoff) = activity_cutoff
        {
            tracing::warn!(
                mechanism = mech.0,
                cutoff_blocks = cutoff,
                "validator is approaching the activity cutoff"
            );
        }
        let slot = self.slots.entry(mech).or_default();
        if slot.uncertain {
            return Ok(());
        }
        let cadence = self.policy.cadence.unwrap_or_else(|| {
            default_cadence(hotkey, preflight.schedule.tempo, preflight.commit_reveal)
        });
        let last_submission = preflight
            .last_update(uid)
            .max(slot.last_attempt_block.unwrap_or(0));
        if last_submission != 0
            && preflight.at.number.saturating_sub(last_submission) < preflight.rate_limit
        {
            if let Some(attempt) = self.attempts.get_mut(&mech) {
                attempt.outcome = WeightAttemptOutcome::RateLimited {
                retry_after_block: last_submission.saturating_add(preflight.rate_limit),
                remediation: "wait for the mechanism's chain rate-limit slot; this is scheduled work, not a failed transaction".into(),
            };
            }
            return Ok(());
        }
        if !due(cadence, slot, &preflight, uid) {
            return Ok(());
        }
        let proposal = match &pending_work {
            Some((_, work)) => WeightsProposal::Set(work.input.clone()),
            None => std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| proposals(mech)))
                .map_err(|_| anyhow::anyhow!("validator weights callback panicked"))?,
        };
        let (input, operation) = match proposal {
            WeightsProposal::Set(input) => (input, None),
            WeightsProposal::Operation {
                id,
                correlation_id,
                input,
            } => {
                anyhow::ensure!(
                    self.durable.is_some(),
                    "application operation IDs require a durable scheduler"
                );
                (input, Some((id, correlation_id)))
            }
            WeightsProposal::HoldPrevious => {
                if let Some(attempt) = self.attempts.get_mut(&mech) {
                    attempt.outcome = WeightAttemptOutcome::HoldPrevious;
                }
                cx.metrics.increment("rtao_weights_hold_total");
                return Ok(());
            }
            WeightsProposal::Skip(reason) => {
                if let Some(attempt) = self.attempts.get_mut(&mech) {
                    attempt.outcome = WeightAttemptOutcome::Skipped(reason.into());
                }
                tracing::debug!(mechanism = mech.0, reason, "weight proposal skipped");
                return Ok(());
            }
        };
        if self.durable.is_some() && pending_work.is_none() {
            let d = self
                .durable
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("missing journal"))?;
            let revision = d.journal.snapshot()?.revision;
            let (id, correlation) =
                operation.unwrap_or_else(|| (format!("{}:op:{revision}", d.scope), None));
            self.enqueue(id, correlation, mech, input.clone())?;
            if self.pending_work(mech)?.is_none() {
                // The application redelivered a completed logical operation.
                return Ok(());
            }
        }
        let row = prepare_row(input.clone(), &preflight, uid, cx.netuid, &graph)?;
        if hold_previous(&row, uid, owner, &graph) {
            if let Some(attempt) = self.attempts.get_mut(&mech) {
                attempt.outcome = WeightAttemptOutcome::HoldPrevious;
            }
            cx.metrics.increment("rtao_weights_hold_total");
            return Ok(());
        }
        let block_time = snapshot_slot_duration(&cx.client, &preflight.at).await?;
        let mut commit = None;
        if preflight.commit_reveal {
            self.clock_guard.check(self.clock.as_ref())?;
            let committer = TimelockCommitter::new(if preflight.null_consensus {
                CommitSizeLimit::Null {
                    mechanisms: u16::from(mechanisms),
                }
            } else {
                CommitSizeLimit::Yuma
            })?;
            commit = Some(
                committer
                    .commit_at(TimelockCommitRequest {
                        now: self.clock.now_unix(),
                        hotkey,
                        row: row.clone(),
                        version_key,
                        state: preflight.schedule.clone(),
                        reveal_period_epochs: preflight.reveal_period,
                        block_time,
                    })
                    .await?,
            );
        }
        // Reserve this cadence slot before await; pool-unknown outcomes must
        // never cause an unconditional resubmission on the next finalized head.
        self.slots.entry(mech).or_default().last_attempt_epoch =
            Some(preflight.schedule.subnet_epoch_index);
        self.slots.entry(mech).or_default().last_attempt_block = Some(preflight.at.number);
        let binding = self.prepare_binding(mech, row.clone(), commit.clone(), &preflight)?;
        self.persist(None)?;
        let result = submit::execute(
            cx,
            mech,
            row.clone(),
            version_key,
            commit.as_ref(),
            block_time,
            binding.clone(),
        )
        .await;
        let report = match result {
            Ok(report) => report,
            Err(error) if submit::invalid_reveal_round(&error) && commit.is_some() => {
                if binding.is_some() {
                    // Even a rejection cannot prove the same signed bytes were never
                    // accepted elsewhere. Durable recovery must exclude them first.
                    self.slots.entry(mech).or_default().uncertain = true;
                    return Err(error);
                }
                let fresh = cx.client.at(BlockRef::Best).await?;
                let state = WeightPreflight::fetch(&fresh, cx.netuid, mech, hotkey).await?;
                let uid = state.check(version_key)?;
                let (count, current_owner) = fresh
                    .fetch((
                        st::MECHANISM_COUNT_CURRENT.at((cx.netuid,)),
                        st::SUBNET_OWNER_HOTKEY.at((cx.netuid,)),
                    ))
                    .await?;
                anyhow::ensure!(
                    mech.0 < count.0 && count.0 <= 16,
                    "mechanism no longer exists after the rejected commitment"
                );
                let graph = cx.metagraph.load();
                let row = prepare_row(input, &state, uid, cx.netuid, &graph)?;
                if hold_previous(&row, uid, *current_owner.raw(), &graph) {
                    if let Some(attempt) = self.attempts.get_mut(&mech) {
                        attempt.outcome = WeightAttemptOutcome::HoldPrevious;
                    }
                    return Ok(());
                }
                let block_time = snapshot_slot_duration(&cx.client, &state.at).await?;
                commit = if state.commit_reveal {
                    self.clock_guard.check(self.clock.as_ref())?;
                    Some(
                        TimelockCommitter::new(if state.null_consensus {
                            CommitSizeLimit::Null {
                                mechanisms: u16::from(count.0),
                            }
                        } else {
                            CommitSizeLimit::Yuma
                        })?
                        .commit_at(TimelockCommitRequest {
                            now: self.clock.now_unix(),
                            hotkey,
                            row: row.clone(),
                            version_key,
                            state: state.schedule.clone(),
                            reveal_period_epochs: state.reveal_period,
                            block_time,
                        })
                        .await?,
                    )
                } else {
                    None
                };
                self.slots.entry(mech).or_default().last_attempt_epoch =
                    Some(state.schedule.subnet_epoch_index);
                self.slots.entry(mech).or_default().last_attempt_block = Some(state.at.number);
                cx.metrics
                    .increment("rtao_weights_reveal_round_recomputed_total");
                let retry = submit::execute(
                    cx,
                    mech,
                    row.clone(),
                    version_key,
                    commit.as_ref(),
                    block_time,
                    None,
                )
                .await;
                let report = match retry {
                    Ok(report) => report,
                    Err(error) => {
                        self.slots.entry(mech).or_default().uncertain = submit::uncertain(&error);
                        return Err(error);
                    }
                };
                if let Some(attempt) = self.attempts.get_mut(&mech) {
                    attempt.outcome = WeightAttemptOutcome::Submitted {
                        block_hash: report.block_hash,
                        block_number: report.block_number,
                    };
                }
                log_submitted(mech, report.block_number, commit.is_some());
                if let Some(commit) = commit {
                    self.reveals
                        .record(
                            &cx.client,
                            cx.netuid,
                            mech,
                            hotkey,
                            row,
                            version_key,
                            state,
                            commit,
                            &report,
                            &cx.metrics,
                        )
                        .await?;
                }
                cx.metrics
                    .update_readiness(|ready| ready.last_weight_succeeded = true);
                return Ok(());
            }
            Err(error) => {
                self.slots.entry(mech).or_default().uncertain =
                    binding.is_some() || submit::uncertain(&error);
                self.persist(None)?;
                return Err(error);
            }
        };
        self.record_inclusion(&cx.client, mech, &report).await?;
        if let Some(attempt) = self.attempts.get_mut(&mech) {
            attempt.outcome = WeightAttemptOutcome::Submitted {
                block_hash: report.block_hash,
                block_number: report.block_number,
            };
        }
        log_submitted(mech, report.block_number, commit.is_some());
        if let Some(commit) = commit {
            self.reveals
                .record(
                    &cx.client,
                    cx.netuid,
                    mech,
                    hotkey,
                    row,
                    version_key,
                    preflight,
                    commit,
                    &report,
                    &cx.metrics,
                )
                .await?;
        }
        self.persist(binding.as_ref().map(|b| b.id()))?;
        cx.metrics.increment("rtao_weights_submitted_total");
        cx.metrics
            .update_readiness(|ready| ready.last_weight_succeeded = true);
        Ok(())
    }
}

/// Operators watch this line to confirm the validator is scoring on chain.
fn log_submitted(mech: MechId, block: u64, committed: bool) {
    if committed {
        tracing::info!(
            mechanism = mech.0,
            block,
            "weights committed; the chain reveals them after the reveal period"
        );
    } else {
        tracing::info!(mechanism = mech.0, block, "weights set");
    }
}

/// Deterministic default jitter, with direct rows governed by the chain limit.
pub fn default_cadence(hotkey: AccountId, tempo: u16, commit_reveal: bool) -> WeightCadence {
    if !commit_reveal {
        return WeightCadence::RateLimited { margin: 0 };
    }
    let digest = Blake2b::<U32>::digest(hotkey.0);
    let modulus = u64::from((tempo / 10).max(1));
    // Reduce all 256 digest bits rather than silently selecting one limb.
    let remainder = digest.iter().fold(0u64, |value, byte| {
        (value * 256 + u64::from(*byte)) % modulus
    });
    WeightCadence::OncePerEpoch {
        offset_blocks: 2 + remainder,
    }
}
fn due(cadence: WeightCadence, slot: &Slot, state: &WeightPreflight, uid: Uid) -> bool {
    cadence_opportunity(cadence, slot, state, uid) == WeightOpportunity::Ready
}
fn cadence_opportunity(
    cadence: WeightCadence,
    slot: &Slot,
    state: &WeightPreflight,
    uid: Uid,
) -> WeightOpportunity {
    let last = state
        .last_update(uid)
        .max(slot.last_attempt_block.unwrap_or(0));
    if last != 0 && state.at.number.saturating_sub(last) < state.rate_limit {
        return WeightOpportunity::RateLimited {
            retry_after_block: last.saturating_add(state.rate_limit),
        };
    }
    match cadence {
        WeightCadence::OncePerEpoch { offset_blocks } => {
            if slot.last_attempt_epoch == Some(state.schedule.subnet_epoch_index) {
                WeightOpportunity::AwaitEpoch {
                    after_epoch: state.schedule.subnet_epoch_index,
                    offset_blocks,
                }
            } else if state
                .at
                .number
                .saturating_sub(state.schedule.last_epoch_block)
                < offset_blocks
            {
                WeightOpportunity::AwaitBlock {
                    not_before_block: state
                        .schedule
                        .last_epoch_block
                        .saturating_add(offset_blocks),
                }
            } else {
                WeightOpportunity::Ready
            }
        }
        WeightCadence::RateLimited { margin } => {
            let delay = state.rate_limit.saturating_add(margin).max(1);
            if last == 0 || state.at.number.saturating_sub(last) >= delay {
                WeightOpportunity::Ready
            } else {
                WeightOpportunity::RateLimited {
                    retry_after_block: last.saturating_add(delay),
                }
            }
        }
    }
}
fn prepare_row(
    input: WeightsInput,
    state: &WeightPreflight,
    uid: Uid,
    netuid: NetUid,
    graph: &rtao_metagraph::Metagraph,
) -> anyhow::Result<(Vec<Uid>, Vec<u16>)> {
    let (uids, weights, raw) = match input {
        WeightsInput::ByUid(row) => {
            let (uids, values) = row.into_iter().unzip();
            (uids, values, false)
        }
        WeightsInput::Parallel { uids, weights } => (uids, weights, false),
        WeightsInput::ByHotkey(row) => {
            let mut uids = Vec::with_capacity(row.len());
            let mut values = Vec::with_capacity(row.len());
            for (hotkey, weight) in row {
                uids.push(
                    graph
                        .by_hotkey(&hotkey)
                        .ok_or_else(|| {
                            anyhow::anyhow!(
                                "weight destination {hotkey} is absent from the last-good graph"
                            )
                        })?
                        .uid,
                );
                values.push(weight);
            }
            (uids, values, false)
        }
        WeightsInput::RawU16 { uids, values } => {
            anyhow::ensure!(
                state.null_consensus
                    && state.permits.iter().filter(|permit| **permit).count() == 1
                    && state.permits.get(usize::from(uid.0)) == Some(&true),
                "RawU16 requires Null consensus and this hotkey as the sole permit holder"
            );
            anyhow::ensure!(
                uids.len() == values.len(),
                "raw destinations and values differ in length"
            );
            (uids, values.into_iter().map(f64::from).collect(), true)
        }
    };
    anyhow::ensure!(
        uids.iter().collect::<BTreeSet<_>>().len() == uids.len(),
        "weight destinations contain duplicate UIDs"
    );
    anyhow::ensure!(
        weights
            .iter()
            .all(|weight| weight.is_finite() && *weight >= 0.0),
        "weights must be finite and non-negative"
    );
    anyhow::ensure!(
        uids.iter().all(|dest| dest.0 < state.population),
        "weight destination UID is outside the current subnet population"
    );
    if raw {
        // Pinned Null direct dispatch preserves zeros and checks length before
        // max/sum. The raw API promises those exact bytes, unlike Python's
        // positive-only conform path; never normalize or filter this vector.
        // The configured limit is a client guard: v478's chain getter currently
        // returns u16::MAX even when MaxWeightsLimit storage is smaller.
        let values = weights
            .into_iter()
            .map(|weight| weight as u16)
            .collect::<Vec<_>>();
        anyhow::ensure!(
            values.iter().any(|value| *value > 0),
            "all raw weights are zero"
        );
        let self_weight = uids == [uid];
        let minimum = state.min_allowed_weights.min(state.population);
        anyhow::ensure!(
            (netuid.0 != 0 && self_weight) || values.len() >= usize::from(minimum),
            "raw row is below the chain minimum destination count"
        );
        let total: u128 = values.iter().map(|value| u128::from(*value)).sum();
        let maximum = u128::from(values.iter().copied().max().unwrap_or(0));
        anyhow::ensure!(
            self_weight
                || maximum * u128::from(u16::MAX) <= total * u128::from(state.max_weights_limit),
            "raw weights exceed the configured maximum share; change integer ratios or the subnet limit"
        );
        return Ok((uids, values));
    }
    Ok(conform(
        &uids,
        &weights,
        ConformState {
            uid,
            min_allowed_weights: state.min_allowed_weights.min(state.population),
            max_weight_limit: state.max_weights_limit,
        },
        netuid,
        false,
    )?)
}
fn hold_previous(
    row: &(Vec<Uid>, Vec<u16>),
    uid: Uid,
    owner: AccountId,
    graph: &rtao_metagraph::Metagraph,
) -> bool {
    row.0
        .iter()
        .zip(&row.1)
        .filter(|(_, value)| **value > 0)
        .all(|(dest, _)| *dest == uid || graph.hotkey(*dest) == Some(&owner))
}
async fn snapshot_slot_duration(
    client: &Client,
    block: &rtao_client::PinnedBlock,
) -> anyhow::Result<f64> {
    let snapshot = Snapshot::new(client.clone(), block.clone());
    let millis = snapshot
        .constant(&rtao_runtime::constants::aura::SLOT_DURATION)
        .await?;
    anyhow::ensure!(millis > 0, "Aura slot duration is zero");
    Ok(millis as f64 / 1000.0)
}
pub(super) fn number(value: &DynValue, name: &str) -> anyhow::Result<u64> {
    value
        .as_u128()
        .and_then(|value| u64::try_from(value).ok())
        .ok_or_else(|| anyhow::anyhow!("{name} is not a u64"))
}
fn boolean(value: &DynValue, name: &str) -> anyhow::Result<bool> {
    value
        .as_bool()
        .ok_or_else(|| anyhow::anyhow!("{name} is not a bool"))
}
fn integers(value: &DynValue, name: &str) -> anyhow::Result<Vec<u64>> {
    let Value::List(values) = value.as_core() else {
        anyhow::bail!("{name} is not a list")
    };
    values
        .iter()
        .map(|value| number(&DynValue::new(value.clone()), name))
        .collect()
}
fn booleans(value: &DynValue, name: &str) -> anyhow::Result<Vec<bool>> {
    let Value::List(values) = value.as_core() else {
        anyhow::bail!("{name} is not a list")
    };
    values
        .iter()
        .map(|value| boolean(&DynValue::new(value.clone()), name))
        .collect()
}
fn consensus_null(value: &DynValue) -> anyhow::Result<bool> {
    match value.as_core() {
        Value::Str(name) if name == "Null" => Ok(true),
        Value::Str(name) if name == "Yuma" => Ok(false),
        Value::Dict(fields) if fields.len() == 1 && fields[0].0 == Value::Str("Null".into()) => {
            Ok(true)
        }
        Value::Dict(fields) if fields.len() == 1 && fields[0].0 == Value::Str("Yuma".into()) => {
            Ok(false)
        }
        _ => anyhow::bail!("unknown SubnetEpochConsensus"),
    }
}

pub(crate) async fn run_scheduler<V: Validator>(
    validator: Arc<V>,
    state: StateCell<V::State>,
    cx: NeuronCx,
    guard: ClockGuard,
    clock: Arc<dyn Clock>,
    policy: WeightPolicy,
    journal: Option<Arc<rtao_client::journal::OperationJournal>>,
) -> anyhow::Result<()> {
    let journal = match journal {
        Some(journal) => journal,
        None => {
            let path = cx.state_dir.join("weights-journal.json");
            rtao_client::offload(move || rtao_client::journal::OperationJournal::open(path))
                .await??
        }
    };
    let mut scheduler = WeightScheduler::new(policy, clock, guard)?.with_journal(
        journal,
        cx.client.genesis(),
        cx.netuid,
        cx.hotkey.hotkey_address()?,
    )?;
    if let Some((contract, guard)) = validator.weight_semantics() {
        scheduler = scheduler.with_semantic_guard(contract, guard);
    }
    let mut heads = cx.client.blocks(Finality::Finalized)?;
    loop {
        let head = tokio::select! { biased; () = cx.shutdown.cancelled() => return Ok(()), next = heads.next() => next };
        let Some(head) = head else {
            anyhow::bail!("weight finalized-head stream ended")
        };
        let head = match head {
            Ok(head) => head,
            Err(error) => {
                tracing::warn!(%error, "weight head observer reconnecting");
                continue;
            }
        };
        let proposal_cx = cx.clone();
        if let Err(error) = scheduler
            .on_finalized(&cx, &head, |mech| {
                validator.weights(&proposal_cx, &state.read(), mech)
            })
            .await
        {
            cx.metrics.increment("rtao_weights_errors_total");
            cx.metrics
                .update_readiness(|ready| ready.last_weight_succeeded = false);
            tracing::warn!(%error, "weight scheduler will retry on the next finalized head");
        }
    }
}
