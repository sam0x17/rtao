//! Fixtures and test infrastructure for rtao.

#![doc = include_str!("../README.md")]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![deny(missing_docs)]

pub mod fakes;
pub mod fixtures;
/// Synthetic-genesis native journal/paired-shield lifecycle harness.
#[cfg(feature = "journal-shield")]
pub mod journal_shield;
/// Descriptor observations for catalog read coverage and pinned-block tests.
pub mod recording;
/// Upstream synchronous client for dev-only constructor differential tests.
#[cfg(feature = "core-policy")]
#[cfg_attr(docsrs, doc(cfg(feature = "core-policy")))]
pub use bittensor_core::client as core_client;
/// Upstream transaction constructors and policy for dev-only differential tests.
#[cfg(feature = "core-policy")]
#[cfg_attr(docsrs, doc(cfg(feature = "core-policy")))]
pub use bittensor_core::transaction as core_transaction;
pub use recording::RecordingView;
/// RPC types used by dev-only fake backends and their client adapters.
pub use subxt_rpcs as rpc;

#[cfg(feature = "ledger-emu")]
#[cfg_attr(docsrs, doc(cfg(feature = "ledger-emu")))]
pub mod ledger_emu;
#[cfg(feature = "docker")]
#[cfg_attr(docsrs, doc(cfg(feature = "docker")))]
pub mod localnet;
#[cfg(feature = "docker")]
mod scenario;
#[cfg(feature = "docker")]
#[cfg_attr(docsrs, doc(cfg(feature = "docker")))]
pub use localnet::{Localnet, LocalnetError, LocalnetProfile, LocalnetSlot};
#[cfg(feature = "docker")]
#[cfg_attr(docsrs, doc(cfg(feature = "docker")))]
pub use scenario::{BuiltScenario, Scenario, ScenarioError, VALIDATOR_STAKE};

pub use fakes::{
    FakeChain, FakeNode, FakeState, Fault, FaultPlan, Notification, Reply, RpcFailure, RpcRequest,
    SubmitScript, SubscriptionScript,
};
