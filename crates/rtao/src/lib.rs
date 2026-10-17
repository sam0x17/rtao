// Derived from RaoFoundation/subtensor sdk/python/bittensor/__init__.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Rust SDK and operator framework for Bittensor subnets.

#![deny(missing_docs)]
#![doc = include_str!("../README.md")]
#![cfg_attr(docsrs, feature(doc_cfg))]

pub use rtao_core as core;
pub use rtao_primitives as primitives;

/// Blocking clients backed by a lazy private runtime thread.
#[cfg(feature = "blocking")]
#[cfg_attr(docsrs, doc(cfg(feature = "blocking")))]
pub mod blocking;

#[cfg(feature = "tx")]
mod executor;
#[cfg(feature = "tx")]
#[cfg_attr(docsrs, doc(cfg(feature = "tx")))]
pub use executor::Executor;

#[cfg(feature = "btauth")]
#[cfg_attr(docsrs, doc(cfg(feature = "btauth")))]
pub use rtao_btauth as btauth;
#[cfg(feature = "client")]
#[cfg_attr(docsrs, doc(cfg(feature = "client")))]
pub use rtao_client as client;
#[cfg(feature = "evm")]
#[cfg_attr(docsrs, doc(cfg(feature = "evm")))]
pub use rtao_evm as evm;
#[cfg(feature = "intents")]
#[cfg_attr(docsrs, doc(cfg(feature = "intents")))]
pub use rtao_intents as intents;
#[cfg(feature = "metagraph")]
#[cfg_attr(docsrs, doc(cfg(feature = "metagraph")))]
pub use rtao_metagraph as metagraph;
#[cfg(feature = "neuron")]
#[cfg_attr(docsrs, doc(cfg(feature = "neuron")))]
pub use rtao_neuron as neuron;
#[cfg(feature = "owner")]
#[cfg_attr(docsrs, doc(cfg(feature = "owner")))]
pub use rtao_neuron::{
    HparamArg, OwnerConfig, OwnerMonitor, OwnerOps, RegistrationWait, SubnetIdentity, TrimReport,
    WindowWait,
};
#[cfg(feature = "reads")]
#[cfg_attr(docsrs, doc(cfg(feature = "reads")))]
pub use rtao_reads as reads;
#[cfg(feature = "tx")]
#[cfg_attr(docsrs, doc(cfg(feature = "tx")))]
pub use rtao_signer as signer;
#[cfg(feature = "tx")]
#[cfg_attr(docsrs, doc(cfg(feature = "tx")))]
pub use rtao_tx as tx;
#[cfg(feature = "wallet")]
#[cfg_attr(docsrs, doc(cfg(feature = "wallet")))]
pub use rtao_wallet as wallet;

/// Compatibility crates for subnets migrating from pre-v11 protocols.
#[cfg(any(feature = "compat-synapse", feature = "compat-epistula"))]
#[cfg_attr(
    docsrs,
    doc(cfg(any(feature = "compat-synapse", feature = "compat-epistula")))
)]
pub mod compat {
    /// Epistula v2 signed headers with dialect presets.
    #[cfg(feature = "compat-epistula")]
    #[cfg_attr(docsrs, doc(cfg(feature = "compat-epistula")))]
    pub use rtao_compat_epistula as epistula;
    /// v10 Synapse, Axon and Dendrite wire compatibility.
    #[cfg(feature = "compat-synapse")]
    #[cfg_attr(docsrs, doc(cfg(feature = "compat-synapse")))]
    pub use rtao_compat_synapse as synapse;
}
#[cfg(feature = "remote-signer")]
#[cfg_attr(docsrs, doc(cfg(feature = "remote-signer")))]
pub use rtao_remote_signer as remote_signer;
#[cfg(feature = "transport-quic")]
#[cfg_attr(docsrs, doc(cfg(feature = "transport-quic")))]
pub use rtao_transport_quic as quic;

/// Traits and common types for fluent client and snapshot reads.
pub mod prelude {
    #[cfg(feature = "tx")]
    #[cfg_attr(docsrs, doc(cfg(feature = "tx")))]
    pub use crate::Executor;
    #[cfg(feature = "owner")]
    #[cfg_attr(docsrs, doc(cfg(feature = "owner")))]
    pub use crate::{
        HparamArg, OwnerConfig, OwnerMonitor, OwnerOps, RegistrationWait, SubnetIdentity,
        WindowWait,
    };
    #[cfg(feature = "client")]
    #[cfg_attr(docsrs, doc(cfg(feature = "client")))]
    pub use rtao_client::{BlockRef, Client, ClientConfig, Snapshot, View, ViewExt};
    #[cfg(feature = "metagraph")]
    #[cfg_attr(docsrs, doc(cfg(feature = "metagraph")))]
    pub use rtao_metagraph::SubnetsMetagraphExt;
    pub use rtao_primitives::{AccountId, Alpha, Balance, MechId, NetUid, Tao, Uid};
    #[cfg(feature = "reads")]
    #[cfg_attr(docsrs, doc(cfg(feature = "reads")))]
    pub use rtao_reads::Namespaces;
    #[cfg(feature = "tx")]
    #[cfg_attr(docsrs, doc(cfg(feature = "tx")))]
    pub use rtao_tx::{ExecuteOptions, Intent, IntentInterface, PlanOptions, Policy};
}
