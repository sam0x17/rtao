//! Hand-written command families and catalog discovery.

/// Operation discovery and exact semantic/chain error explanations.
pub mod catalog;
/// Miner registration collateral inspection and transactions.
pub mod collateral;
/// Shared persistent config commands.
pub mod config;
/// Stake locking, conviction and locked-alpha acceptance.
pub mod conviction;
/// EVM keys, native funding, HTTP transactions and precompile commands.
#[cfg(feature = "evm")]
pub mod evm;
/// Browser-extension bridge lifecycle and public account discovery.
#[cfg(feature = "extension")]
pub mod extension;

/// On-chain axon endpoint commands.
pub mod axon;
/// Catalog-driven transaction and read commands.
pub mod generated;
/// Subnet owner settings and read commands.
pub mod hparams;
/// Saved multisig signer sets, public pending operations and proposal discovery.
pub mod multisig;
/// Public preparation and late proof refresh for registration.
pub mod pow;
/// Proxy delegation, pure-account lifecycle and pending proxy execution.
pub mod proxy;
/// Root validator funds, allocation and registration.
pub mod root;
/// Schema-derived argument coercion and defaults.
pub mod schema;
/// Stake aliases of ordinary generated transaction operations.
pub mod stake;
/// Subnet creation, registration and metagraph views.
pub mod subnets;
/// Standalone timelock encryption, decryption and envelope inspection.
pub mod timelock;
/// Local wallet management and public account views.
pub mod wallet;
/// Validator weight submission commands.
pub mod weights;
