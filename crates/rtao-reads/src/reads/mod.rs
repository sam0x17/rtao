//! Hand-written implementations of generated catalog contracts.

/// Runtime duration and pinned chain time.
pub mod chain;
/// Epoch schedules and signed LastUpdate differences.
pub mod epochs;
/// Generated scalar hyperparameter entry points.
pub mod hyperparameters;
/// Epoch, weight and chain reads ported in the submission milestone.
pub mod m2;
/// Neuron registration reads.
pub mod neurons;
/// Owner-loop reads, balances, identities, delegations and spot stake valuations.
pub mod owner;
/// Subnet runtime records and chain-registered symbols.
pub mod subnets;
/// Sparse mechanism weight matrices.
pub mod weights;
