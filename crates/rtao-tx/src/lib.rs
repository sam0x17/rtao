//! Bittensor execution plans, policies and transaction results.

#![deny(missing_docs)]
#![doc = include_str!("../README.md")]

mod error;
mod executor;
pub mod hashed;
mod intent;
/// Multisig account approvals and reusable opening-deposit funding checks.
pub mod multisig;
/// Public preparation and policy-gated external signature submission.
pub mod offline;
mod output;
/// Approved dynamic inputs for a multisig round that rebuilds its call.
pub mod pinned;
mod plan;
mod policy;
pub mod receiving;
mod registration;
pub mod sponsorship;
/// Portable Quicknet timelock envelopes and asynchronous beacon decryption.
pub mod timelock;
pub mod wallet_registration;

pub use error::{ChainError, IntentError, PolicyError, TxError, classify};
pub use executor::{
    ExecuteOptions, Executor, IntentRegistry, PlanOptions, Progress, ShieldOptions, ToolSpec,
    compose_nested_intent_call, contains_account_registration,
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
pub use sponsorship::{RegistrationSponsorship, SponsorshipConsent, SponsorshipRequired};
