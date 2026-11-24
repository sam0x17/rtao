//! Weight clipping and quantization with the pinned Python operation order.

mod commit;
mod intents;
mod math;

pub use crate::{
    CommitWeightHash, CommitWeights, RevealWeights, SetMechWeightDelegate, SetWeights,
};

pub use commit::{
    CommitError, CommitSizeLimit, PreparedWeightCommit, TimelockCommitRequest, TimelockCommitter,
};

pub use math::{ConformState, WeightsError, clip_to_max_weight, conform, normalize};

/// First runtime that limits the number of entries in each Null mechanism's row.
pub const NULL_ROW_CAP_SPEC: u32 = 495;

/// Maximum Null weight row length on runtimes that enforce the shared budget.
pub fn null_row_cap(mechanisms: rtao_primitives::MechId) -> u16 {
    (2500 / u16::from(mechanisms.0.max(1))).max(1)
}
