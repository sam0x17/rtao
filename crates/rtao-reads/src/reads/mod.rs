//! Hand-written implementations of generated catalog contracts.

/// Runtime duration and pinned chain time.
pub mod chain;
/// Epoch schedules and signed LastUpdate differences.
pub mod epochs;
/// Generated scalar hyperparameter entry points.
pub mod hyperparameters;
/// Epoch, weight and chain reads ported in the submission milestone.
pub mod m2;
/// Account, chain, commitment and neuron behavior from the read parity milestone.
pub mod m6_accounts;
/// Delegation relationships, subnet leases and crowdloan state.
pub mod m6_delegation;
/// Conviction locks, ownership projections and miner collateral accounting.
pub mod m6_locks;
/// Root basket accounting, subnet listings and mechanism state.
pub mod m6_root;
/// Native stake positions, availability, prices and swap quotes.
pub mod m6_staking;
mod namespaces;
/// Neuron registration reads.
pub mod neurons;
/// Owner-loop reads, balances, identities, delegations and spot stake valuations.
pub mod owner;
/// Subnet runtime records and chain-registered symbols.
pub mod subnets;
/// Sparse mechanism weight matrices.
pub mod weights;
