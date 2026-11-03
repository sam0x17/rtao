// Derived from RaoFoundation/subtensor pallets/subtensor/src/subnets/weights.rs @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Execute already conformed rows through the public policy-aware executor.

use super::compose::{self, Composer, WeightCall};
use crate::NeuronCx;
use rtao_client::View;
use rtao_intents::weights::PreparedWeightCommit;
use rtao_primitives::{H256, MechId, NetUid, Uid};
use rtao_runtime::{
    Call, CallBytes, CallDescriptor, RuntimeHandle, calls::subtensor_module as calls,
};
use rtao_tx::{
    BuildCx, Built, ExecuteOptions, Intent, IntentError, IntentInterface, WeightsSubmitted,
};
use serde_json::json;
use std::{marker::PhantomData, time::Duration};

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(super) struct WeightSubmission {
    pub block_hash: H256,
    pub block_number: u64,
}

/// Calls an attempt's intent declares to the executor's surface review.
///
/// Each compiled intent wraps only the call its mechanism uses, so a changed
/// shape of the other weight call never refuses it. A verified intent carries
/// call bytes already composed and validated for the signing runtime
/// (name-resolved or application-encoded, D-088) and wraps no descriptor.
pub(super) trait Wraps: Send + Sync + 'static {
    const WRAPS: &'static [&'static CallDescriptor];
}
/// Compiled `set_mechanism_weights`.
pub(super) struct SetCall;
/// Compiled `commit_timelocked_mechanism_weights`.
pub(super) struct CommitCall;
/// Bytes composed and validated before execution.
pub(super) struct Verified;
impl Wraps for SetCall {
    const WRAPS: &'static [&'static CallDescriptor] = &[&calls::SET_MECHANISM_WEIGHTS];
}
impl Wraps for CommitCall {
    const WRAPS: &'static [&'static CallDescriptor] =
        &[&calls::COMMIT_TIMELOCKED_MECHANISM_WEIGHTS];
}
impl Wraps for Verified {
    const WRAPS: &'static [&'static CallDescriptor] = &[];
}

/// One attempt's final call values, before composition.
#[derive(Clone)]
pub(super) struct WeightCallPlan {
    pub(super) netuid: NetUid,
    pub(super) mechanism: MechId,
    pub(super) row: (Vec<Uid>, Vec<u16>),
    pub(super) version_key: u64,
    pub(super) commit: Option<PreparedWeightCommit>,
    /// Tag the runtime requires on a timelocked commit; the payload format
    /// stays [`super::NATIVE_CR_PAYLOAD_VERSION`] (D-086).
    pub(super) commit_reveal_version: u16,
}
impl WeightCallPlan {
    pub(super) fn new(
        netuid: NetUid,
        mechanism: MechId,
        row: (Vec<Uid>, Vec<u16>),
        version_key: u64,
        commit: Option<(&PreparedWeightCommit, u16)>,
    ) -> Self {
        Self {
            netuid,
            mechanism,
            row,
            version_key,
            commit: commit.map(|(commit, _)| commit.clone()),
            commit_reveal_version: commit.map_or(super::NATIVE_CR_PAYLOAD_VERSION, |(_, tag)| tag),
        }
    }
    fn call(&self) -> Call {
        if let Some(commit) = &self.commit {
            calls::commit_timelocked_mechanism_weights(
                self.netuid,
                self.mechanism,
                commit.ciphertext.clone(),
                commit.reveal_round,
                self.commit_reveal_version,
            )
        } else {
            calls::set_mechanism_weights(
                self.netuid,
                self.mechanism,
                self.row.0.iter().map(|uid| uid.0).collect(),
                self.row.1.clone(),
                self.version_key,
            )
        }
    }
    /// Run `f` on this plan as an application-facing [`WeightCall`].
    pub(super) fn with_call<T>(&self, f: impl FnOnce(&WeightCall<'_>) -> T) -> T {
        if let Some(commit) = &self.commit {
            f(&WeightCall::Commit {
                netuid: self.netuid,
                mechanism: self.mechanism,
                commit: &commit.ciphertext,
                reveal_round: commit.reveal_round,
                commit_reveal_version: self.commit_reveal_version,
            })
        } else {
            let dests = self.row.0.iter().map(|uid| uid.0).collect::<Vec<_>>();
            f(&WeightCall::Set {
                netuid: self.netuid,
                mechanism: self.mechanism,
                dests: &dests,
                weights: &self.row.1,
                version_key: self.version_key,
            })
        }
    }
}

/// How an attempt's call reaches the executor.
pub(super) enum Composed {
    /// Compose from the compiled descriptor at build time.
    Compiled,
    /// Bytes already composed and validated, with the composer that produced
    /// them.
    Verified(VerifiedCall),
}

/// Call bytes composed and validated without the compiled descriptor, and
/// how to compose them again for another runtime (D-088).
#[derive(Clone)]
pub(super) struct VerifiedCall {
    call: CallBytes,
    composer: Composer,
    guarded: bool,
}
impl VerifiedCall {
    /// `call` was composed by `composer`; `guarded` is the semantic-guard
    /// coverage it was validated under.
    pub(super) fn new(call: CallBytes, composer: Composer, guarded: bool) -> Self {
        Self {
            call,
            composer,
            guarded,
        }
    }
    /// The call for the runtime the executor signs with.
    ///
    /// The scheduler composes under its best-head preflight runtime, while
    /// the executor signs under the refreshed head runtime. When the two
    /// differ, for example across an upgrade enacted between them, the same
    /// composer composes the plan again under the signing runtime and its
    /// validation runs again; a refusal there signs nothing.
    pub(super) fn for_runtime(
        &self,
        plan: &WeightCallPlan,
        runtime: &RuntimeHandle,
    ) -> Result<CallBytes, IntentError> {
        let Err(stale) = self.call.ensure_runtime(runtime) else {
            return Ok(self.call.clone());
        };
        let Some(recomposed) = plan.with_call(|call| {
            compose::compose_verified(&self.composer, runtime, call, self.guarded)
        }) else {
            return Err(rtao_client::ClientError::from(stale).into());
        };
        let call = recomposed.map_err(|detail| {
            IntentError::validation(format!(
                "weight call refused under the signing runtime (spec {}): {detail}",
                runtime.spec_version()
            ))
        })?;
        tracing::info!(
            mechanism = plan.mechanism.0,
            composer = self.composer.label(),
            composed_spec = self.call.stamp().spec_version,
            signing_spec = runtime.spec_version(),
            "the signing runtime differs from the preflight runtime; composed the weight call again"
        );
        Ok(call)
    }
}

pub(super) struct PreparedWeights<K> {
    pub(super) plan: WeightCallPlan,
    pub(super) call: Option<VerifiedCall>,
    kind: PhantomData<K>,
}
impl<K: Wraps> PreparedWeights<K> {
    /// An intent that composes `plan` from its compiled descriptor at build time.
    #[cfg(test)]
    pub(super) fn compiled(plan: WeightCallPlan) -> Self {
        Self {
            plan,
            call: None,
            kind: PhantomData,
        }
    }
}
impl<K: Wraps> IntentInterface for PreparedWeights<K> {
    const SPEC: &'static rtao_catalog::IntentSpec =
        <rtao_intents::SetWeights as IntentInterface>::SPEC;
    type Role = rtao_signer::Hotkey;
    type Origin = rtao_tx::Signed;
    type Output = WeightsSubmitted;
    const WRAPS: &'static [&'static CallDescriptor] = K::WRAPS;
    fn to_json(&self) -> serde_json::Value {
        json!({"op":"set_weights", "netuid":self.plan.netuid.0,"mechid":self.plan.mechanism.0,"version_key":self.plan.version_key})
    }
}
impl<K: Wraps> Intent for PreparedWeights<K> {
    fn summary(&self) -> String {
        format!(
            "submit prepared weights for subnet {} mechanism {}",
            self.plan.netuid.0, self.plan.mechanism.0
        )
    }
    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        let call = match &self.call {
            Some(verified) => verified.for_runtime(&self.plan, cx.runtime)?,
            None => self.plan.call().compose(cx.runtime)?,
        };
        let mut built = Built::new(call);
        if let Some(commit) = &self.plan.commit {
            built
                .extras
                .insert("reveal_round".into(), json!(commit.reveal_round));
        }
        Ok(built)
    }
}

#[derive(Debug, thiserror::Error)]
enum SubmissionError {
    #[error("{0}")]
    InvalidRevealRound(String),
    #[error("{0}")]
    IncorrectCommitRevealVersion(String),
    #[error("weight submission fate is unresolved: {0}")]
    Uncertain(String),
    #[error("weight submission rejected: {0}")]
    Rejected(String),
}

/// The default admission window of a weight transaction (D-097).
///
/// See [`super::WeightScheduler::with_submit_deadline`].
pub const DEFAULT_WEIGHT_SUBMIT_DEADLINE: Duration = Duration::from_secs(30);

pub(super) struct SubmissionOptions {
    pub journal: Option<rtao_client::journal::JournalBinding>,
    pub shield_period: Option<rtao_client::ShieldPeriod>,
    /// The admission window: signer resolution, nonce lease, preparation,
    /// signing and the pool's acknowledgment.
    pub submit_deadline: Duration,
}
impl Default for SubmissionOptions {
    fn default() -> Self {
        Self {
            journal: None,
            shield_period: None,
            submit_deadline: DEFAULT_WEIGHT_SUBMIT_DEADLINE,
        }
    }
}
impl SubmissionOptions {
    pub(super) fn execution(self, block_time: f64) -> ExecuteOptions {
        ExecuteOptions {
            journal: self.journal,
            shielded: self.shield_period.is_some(),
            period: Some(self.shield_period.map_or(128, |p| p.get() as u32)),
            wait_for_inclusion: true,
            wait_for_finalization: true,
            retries: 0,
            reconcile: true,
            submit_deadline: Some(self.submit_deadline),
            // The admission window plus a whole 128-block era, so a longer
            // window never shortens the wait for inclusion or the finalized
            // reconciliation of a transmitted attempt.
            deadline: Some(
                Duration::from_secs_f64(128.0 * block_time).saturating_add(self.submit_deadline),
            ),
            ..Default::default()
        }
    }
}

/// Submit a composed weight call through the policy-aware executor.
pub(super) async fn execute(
    cx: &NeuronCx,
    plan: WeightCallPlan,
    composed: Composed,
    block_time: f64,
    options: SubmissionOptions,
) -> anyhow::Result<WeightSubmission> {
    let options = options.execution(block_time);
    let commit = plan.commit.is_some();
    let result = match composed {
        Composed::Verified(call) => {
            let intent = PreparedWeights::<Verified> {
                plan,
                call: Some(call),
                kind: PhantomData,
            };
            cx.executor.execute(&intent, &cx.hotkey, &options).await?
        }
        Composed::Compiled if commit => {
            let intent = PreparedWeights::<CommitCall> {
                plan,
                call: None,
                kind: PhantomData,
            };
            cx.executor.execute(&intent, &cx.hotkey, &options).await?
        }
        Composed::Compiled => {
            let intent = PreparedWeights::<SetCall> {
                plan,
                call: None,
                kind: PhantomData,
            };
            cx.executor.execute(&intent, &cx.hotkey, &options).await?
        }
    };
    if let Some(hash) = result.fate_unknown {
        return Err(SubmissionError::Uncertain(format!("0x{}", hex::encode(hash.0))).into());
    }
    if !result.success {
        match result
            .error
            .as_ref()
            .and_then(|error| error.name.as_deref())
        {
            Some("InvalidRevealRound") => {
                return Err(SubmissionError::InvalidRevealRound(result.message).into());
            }
            Some("IncorrectCommitRevealVersion") => {
                return Err(SubmissionError::IncorrectCommitRevealVersion(result.message).into());
            }
            _ => return Err(SubmissionError::Rejected(result.message).into()),
        }
    }
    let block_hash = result
        .block_hash
        .ok_or_else(|| SubmissionError::Uncertain("no finalized inclusion hash".into()))?;
    inclusion(&cx.client, block_hash).await
}

/// Locate an accepted submission's finalized inclusion block.
///
/// The receipt may come from a separate submission endpoint whose block the
/// read endpoint has not imported yet, so the header is read where it exists.
pub(super) async fn inclusion(
    client: &rtao_client::Client,
    block_hash: H256,
) -> anyhow::Result<WeightSubmission> {
    let snapshot = client.at_receipt(block_hash).await.map_err(|error| {
        SubmissionError::Uncertain(format!(
            "accepted submission's finalized inclusion header is unavailable: {error}"
        ))
    })?;
    Ok(WeightSubmission {
        block_hash,
        block_number: snapshot.block().number,
    })
}
pub(super) fn invalid_reveal_round(error: &anyhow::Error) -> bool {
    error.downcast_ref::<SubmissionError>().is_some_and(|error| matches!(error, SubmissionError::InvalidRevealRound(_)))
        || error.downcast_ref::<rtao_tx::TxError>().is_some_and(|error| matches!(error, rtao_tx::TxError::Chain(chain) if chain.name.as_deref() == Some("InvalidRevealRound")))
}
/// Root changed `CommitRevealWeightsVersion` after preflight. The dispatch is
/// fee-free and certain, so the scheduler may re-select the tag from fresh
/// state, at most [`super::CR_VERSION_RETRIES`] times.
pub(super) fn incorrect_cr_version(error: &anyhow::Error) -> bool {
    error.downcast_ref::<SubmissionError>().is_some_and(|error| matches!(error, SubmissionError::IncorrectCommitRevealVersion(_)))
        || error.downcast_ref::<rtao_tx::TxError>().is_some_and(|error| matches!(error, rtao_tx::TxError::Chain(chain) if chain.name.as_deref() == Some("IncorrectCommitRevealVersion")))
}
pub(super) fn uncertain(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<SubmissionError>()
        .is_some_and(|error| matches!(error, SubmissionError::Uncertain(_)))
        || error
            .downcast_ref::<rtao_tx::TxError>()
            .is_some_and(|error| {
                matches!(
                    error.primary_cause(),
                    rtao_tx::TxError::Rpc(_) | rtao_tx::TxError::PendingOperation { .. }
                )
            })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::panic)]
    use super::*;
    #[test]
    fn execution_bounds_admission_by_the_window_and_inclusion_by_an_era_after_it() {
        let default = SubmissionOptions::default().execution(12.0);
        assert_eq!(default.submit_deadline, Some(Duration::from_secs(30)));
        assert_eq!(default.deadline, Some(Duration::from_secs(128 * 12 + 30)));
        let paced = SubmissionOptions {
            submit_deadline: Duration::from_secs(150),
            ..Default::default()
        }
        .execution(12.0);
        assert_eq!(paced.submit_deadline, Some(Duration::from_secs(150)));
        assert_eq!(paced.deadline, Some(Duration::from_secs(128 * 12 + 150)));
        assert_eq!(paced.period, Some(128));
        assert!(paced.reconcile && paced.wait_for_finalization);
    }
    #[test]
    fn only_current_mechanism_calls_are_constructed() {
        let mut intent = WeightCallPlan {
            netuid: NetUid(1),
            mechanism: MechId(2),
            row: (vec![Uid(0)], vec![123]),
            version_key: 42,
            commit: None,
            commit_reveal_version: 4,
        };
        assert_eq!(intent.call().descriptor.call_index, 119);
        intent.commit = Some(PreparedWeightCommit {
            ciphertext: vec![1],
            reveal_round: 123,
        });
        assert_eq!(intent.call().descriptor.call_index, 118);
    }
    #[test]
    fn timelocked_commit_carries_the_selected_tag() {
        let commit = PreparedWeightCommit {
            ciphertext: vec![1, 2],
            reveal_round: 123,
        };
        for tag in [4u16, 5, 3] {
            let intent = WeightCallPlan {
                netuid: NetUid(1),
                mechanism: MechId(0),
                row: (vec![Uid(0)], vec![1]),
                version_key: 0,
                commit: Some(commit.clone()),
                commit_reveal_version: tag,
            };
            let call = intent.call();
            let rtao_core::codec::Value::Dict(fields) = &call.args else {
                panic!("call arguments are not a record")
            };
            let carried = fields
                .iter()
                .find(|(name, _)| {
                    *name == rtao_core::codec::Value::Str("commit_reveal_version".into())
                })
                .map(|(_, value)| value.clone());
            assert_eq!(
                carried,
                Some(rtao_primitives::IntoCoreValue::into_core_value(tag))
            );
        }
    }
    /// Each compiled intent declares only its own call to the executor's
    /// surface review, so a changed shape of the other weight call cannot
    /// refuse it; verified bytes declare none (D-088).
    #[test]
    fn each_intent_wraps_only_the_call_it_composes() {
        let names = |wraps: &[&CallDescriptor]| wraps.iter().map(|d| d.name).collect::<Vec<_>>();
        assert_eq!(
            names(<PreparedWeights<SetCall> as IntentInterface>::WRAPS),
            ["set_mechanism_weights"]
        );
        assert_eq!(
            names(<PreparedWeights<CommitCall> as IntentInterface>::WRAPS),
            ["commit_timelocked_mechanism_weights"]
        );
        assert!(<PreparedWeights<Verified> as IntentInterface>::WRAPS.is_empty());
    }
    #[test]
    fn only_known_round_rejection_recomputes() {
        assert!(invalid_reveal_round(
            &SubmissionError::InvalidRevealRound("round".into()).into()
        ));
        assert!(!invalid_reveal_round(
            &SubmissionError::Rejected("InvalidRevealRound text from arbitrary failure".into())
                .into()
        ));
        assert!(uncertain(&SubmissionError::Uncertain("hash".into()).into()));
        assert!(incorrect_cr_version(
            &SubmissionError::IncorrectCommitRevealVersion("tag".into()).into()
        ));
        assert!(!incorrect_cr_version(
            &SubmissionError::InvalidRevealRound("round".into()).into()
        ));
        assert!(!invalid_reveal_round(
            &SubmissionError::IncorrectCommitRevealVersion("tag".into()).into()
        ));
    }
}
