// Derived from RaoFoundation/subtensor pallets/subtensor/src/subnets/weights.rs @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Execute already conformed rows through the public policy-aware executor.

use crate::NeuronCx;
use rtao_client::View;
use rtao_intents::weights::PreparedWeightCommit;
use rtao_primitives::{H256, MechId, NetUid, Uid};
use rtao_runtime::{Call, CallDescriptor, calls::subtensor_module as calls};
use rtao_tx::{
    BuildCx, Built, ExecuteOptions, Intent, IntentError, IntentInterface, WeightsSubmitted,
};
use serde_json::json;
use std::time::Duration;

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(super) struct WeightSubmission {
    pub block_hash: H256,
    pub block_number: u64,
}

pub(super) struct PreparedWeights {
    pub(super) netuid: NetUid,
    pub(super) mechanism: MechId,
    pub(super) row: (Vec<Uid>, Vec<u16>),
    pub(super) version_key: u64,
    pub(super) commit: Option<PreparedWeightCommit>,
    /// Tag the runtime requires on a timelocked commit; the payload format
    /// stays [`super::NATIVE_CR_PAYLOAD_VERSION`] (D-086).
    pub(super) commit_reveal_version: u16,
}
impl IntentInterface for PreparedWeights {
    const SPEC: &'static rtao_catalog::IntentSpec =
        <rtao_intents::SetWeights as IntentInterface>::SPEC;
    type Role = rtao_signer::Hotkey;
    type Origin = rtao_tx::Signed;
    type Output = WeightsSubmitted;
    const WRAPS: &'static [&'static CallDescriptor] =
        <rtao_intents::SetWeights as IntentInterface>::WRAPS;
    fn to_json(&self) -> serde_json::Value {
        json!({"op":"set_weights", "netuid":self.netuid.0,"mechid":self.mechanism.0,"version_key":self.version_key})
    }
}
impl PreparedWeights {
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
}
impl Intent for PreparedWeights {
    fn summary(&self) -> String {
        format!(
            "submit prepared weights for subnet {} mechanism {}",
            self.netuid.0, self.mechanism.0
        )
    }
    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        let mut built = Built::new(self.call().compose(cx.runtime)?);
        if let Some(commit) = &self.commit {
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

#[derive(Default)]
pub(super) struct SubmissionOptions {
    pub journal: Option<rtao_client::journal::JournalBinding>,
    pub shield_period: Option<rtao_client::ShieldPeriod>,
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
            submit_deadline: Some(Duration::from_secs(30)),
            deadline: Some(Duration::from_secs_f64(128.0 * block_time + 30.0)),
            ..Default::default()
        }
    }
}

/// Submit `row`, or `commit` tagged with its commit-reveal version.
pub(super) async fn execute(
    cx: &NeuronCx,
    mechanism: MechId,
    row: (Vec<Uid>, Vec<u16>),
    version_key: u64,
    commit: Option<(&PreparedWeightCommit, u16)>,
    block_time: f64,
    options: SubmissionOptions,
) -> anyhow::Result<WeightSubmission> {
    let intent = PreparedWeights {
        netuid: cx.netuid,
        mechanism,
        row,
        version_key,
        commit: commit.map(|(commit, _)| commit.clone()),
        commit_reveal_version: commit.map_or(super::NATIVE_CR_PAYLOAD_VERSION, |(_, tag)| tag),
    };
    let options = options.execution(block_time);
    let result = cx.executor.execute(&intent, &cx.hotkey, &options).await?;
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
/// fee-free and certain, so the scheduler may re-select the tag once.
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
    fn only_current_mechanism_calls_are_constructed() {
        let mut intent = PreparedWeights {
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
            let intent = PreparedWeights {
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
