//! Bittensor execution plans, policies and transaction results.

#![deny(missing_docs)]

mod error;
mod executor;
mod intent;
/// Multisig account approvals and reusable opening-deposit funding checks.
pub mod multisig;
/// Public preparation and policy-gated external signature submission.
pub mod offline;
mod output;
mod plan;
mod policy;
mod registration;
/// Portable Quicknet timelock envelopes and asynchronous beacon decryption.
pub mod timelock;

pub use error::{ChainError, IntentError, PolicyError, TxError, classify};
pub use executor::{
    ExecuteOptions, Executor, IntentRegistry, PlanOptions, Progress, ShieldOptions, ToolSpec,
};
pub use intent::{
    BuildCx, Built, Clock, DynIntent, FactsTitle, Intent, IntentInterface, IntentPreflight,
    OriginMarker, OriginView, PreflightCx, Root, Signed, SubnetOwner, SystemClock,
};
pub use multisig::{ApproveOptions, Multisig};
pub use offline::PrepareOptions;
pub use output::{
    ExtrinsicResult, IntentOutput, MultisigCall, PureCreated, SubnetRegistered, WeightsSubmitted,
};
pub use plan::Plan;
pub use policy::{IntentSafety, Policy, PolicySubject};
pub use rtao_client::{ShieldPeriod, UnsignedExtrinsic};
pub use rtao_primitives::Spend;
pub use rtao_runtime::models::ProxyType;
