//! Hand-written command families and catalog discovery.

/// Operation discovery and exact semantic/chain error explanations.
pub mod catalog;
/// Shared persistent config commands.
pub mod config;

/// On-chain axon endpoint commands.
pub mod axon;
/// Catalog-driven transaction and read commands.
pub mod generated;
/// Subnet owner settings and read commands.
pub mod hparams;
/// Public preparation and late proof refresh for registration.
pub mod pow;
/// Schema-derived argument coercion and defaults.
pub mod schema;
/// Subnet creation, registration and metagraph views.
pub mod subnets;
/// Local wallet management and public account views.
pub mod wallet;
/// Validator weight submission commands.
pub mod weights;
