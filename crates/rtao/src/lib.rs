//! Rust SDK and operator framework for Bittensor subnets.

#![deny(missing_docs)]

pub use rtao_core as core;
pub use rtao_primitives as primitives;

#[cfg(feature = "tx")]
mod executor;
#[cfg(feature = "tx")]
pub use executor::Executor;

#[cfg(feature = "btauth")]
pub use rtao_btauth as btauth;
#[cfg(feature = "client")]
pub use rtao_client as client;
#[cfg(feature = "evm")]
pub use rtao_evm as evm;
#[cfg(feature = "intents")]
pub use rtao_intents as intents;
#[cfg(feature = "metagraph")]
pub use rtao_metagraph as metagraph;
#[cfg(feature = "neuron")]
pub use rtao_neuron as neuron;
#[cfg(feature = "owner")]
pub use rtao_neuron::{
    HparamArg, OwnerConfig, OwnerMonitor, OwnerOps, RegistrationWait, SubnetIdentity, TrimReport,
    WindowWait,
};
#[cfg(feature = "reads")]
pub use rtao_reads as reads;
#[cfg(feature = "tx")]
pub use rtao_signer as signer;
#[cfg(feature = "tx")]
pub use rtao_tx as tx;
#[cfg(feature = "wallet")]
pub use rtao_wallet as wallet;

/// Traits and common types for fluent client and snapshot reads.
pub mod prelude {
    #[cfg(feature = "tx")]
    pub use crate::Executor;
    #[cfg(feature = "owner")]
    pub use crate::{
        HparamArg, OwnerConfig, OwnerMonitor, OwnerOps, RegistrationWait, SubnetIdentity,
        WindowWait,
    };
    #[cfg(feature = "client")]
    pub use rtao_client::{BlockRef, Client, ClientConfig, Snapshot, View, ViewExt};
    #[cfg(feature = "metagraph")]
    pub use rtao_metagraph::SubnetsMetagraphExt;
    pub use rtao_primitives::{AccountId, Alpha, Balance, MechId, NetUid, Tao, Uid};
    #[cfg(feature = "reads")]
    pub use rtao_reads::Namespaces;
    #[cfg(feature = "tx")]
    pub use rtao_tx::{ExecuteOptions, Intent, IntentInterface, PlanOptions, Policy};
}
