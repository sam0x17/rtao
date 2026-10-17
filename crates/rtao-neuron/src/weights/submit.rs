// Derived from RaoFoundation/subtensor pallets/subtensor/src/subnets/weights.rs @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Execute already conformed rows through the public policy-aware executor.

use super::SUPPORTED_CR_VERSION;
use crate::NeuronCx;
use rtao_client::{BlockRef, View};
use rtao_intents::weights::PreparedWeightCommit;
use rtao_primitives::{H256, MechId, NetUid, Uid};
use rtao_runtime::{Call, CallDescriptor, calls::subtensor_module as calls};
use rtao_tx::{
    BuildCx, Built, ExecuteOptions, Intent, IntentError, IntentInterface, WeightsSubmitted,
};
use serde_json::json;
use std::time::Duration;

#[derive(Clone)]
pub(super) struct WeightSubmission {
    pub block_hash: H256,
    pub block_number: u64,
}

struct PreparedWeights {
    netuid: NetUid,
    mechanism: MechId,
    row: (Vec<Uid>, Vec<u16>),
    version_key: u64,
    commit: Option<PreparedWeightCommit>,
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
                SUPPORTED_CR_VERSION,
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
    #[error("weight submission fate is unresolved: {0}")]
    Uncertain(String),
    #[error("weight submission rejected: {0}")]
    Rejected(String),
}

pub(super) async fn execute(
    cx: &NeuronCx,
    mechanism: MechId,
    row: (Vec<Uid>, Vec<u16>),
    version_key: u64,
    commit: Option<&PreparedWeightCommit>,
    block_time: f64,
) -> anyhow::Result<WeightSubmission> {
    let intent = PreparedWeights {
        netuid: cx.netuid,
        mechanism,
        row,
        version_key,
        commit: commit.cloned(),
    };
    let options = ExecuteOptions {
        period: Some(128),
        wait_for_inclusion: true,
        wait_for_finalization: true,
        retries: 0,
        reconcile: true,
        submit_deadline: Some(Duration::from_secs(30)),
        deadline: Some(Duration::from_secs_f64(128.0 * block_time + 30.0)),
        ..Default::default()
    };
    let result = cx.executor.execute(&intent, &cx.hotkey, &options).await?;
    if let Some(hash) = result.fate_unknown {
        return Err(SubmissionError::Uncertain(format!("0x{}", hex::encode(hash.0))).into());
    }
    if !result.success {
        if result
            .error
            .as_ref()
            .and_then(|error| error.name.as_deref())
            == Some("InvalidRevealRound")
        {
            return Err(SubmissionError::InvalidRevealRound(result.message).into());
        }
        return Err(SubmissionError::Rejected(result.message).into());
    }
    let block_hash = result
        .block_hash
        .ok_or_else(|| SubmissionError::Uncertain("no finalized inclusion hash".into()))?;
    let snapshot = cx
        .client
        .at(BlockRef::Hash(block_hash))
        .await
        .map_err(|error| {
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
pub(super) fn uncertain(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<SubmissionError>()
        .is_some_and(|error| matches!(error, SubmissionError::Uncertain(_)))
        || error
            .downcast_ref::<rtao_tx::TxError>()
            .is_some_and(|error| matches!(error, rtao_tx::TxError::Rpc(_)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_current_mechanism_calls_are_constructed() {
        let mut intent = PreparedWeights {
            netuid: NetUid(1),
            mechanism: MechId(2),
            row: (vec![Uid(0)], vec![123]),
            version_key: 42,
            commit: None,
        };
        assert_eq!(intent.call().descriptor.call_index, 119);
        intent.commit = Some(PreparedWeightCommit {
            ciphertext: vec![1],
            reveal_round: 123,
        });
        assert_eq!(intent.call().descriptor.call_index, 118);
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
    }
}
