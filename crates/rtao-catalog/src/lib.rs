//! Generated Bittensor intent, read and error catalogs.

#![doc = include_str!("../README.md")]
#![deny(missing_docs)]

pub mod py;

pub mod config;
pub mod errors;
pub mod hyperparams;
/// Authenticated subnet-owner setter and cooldown policies.
pub mod owner_map;
/// Constants and code tables extracted from authoritative runtime Rust sources.
pub mod rt;
pub mod runtime_py;
pub mod specs;

/// Semantic error classification shared by the client and transaction layers.
pub use errors::ErrorCode;

/// Complete authenticated semantic and chain error documentation.
pub use errors::{CHAIN_ERRORS, ChainErrorSpec, SEMANTIC_ERRORS, SemanticErrorSpec};

/// Complete authenticated intent and read catalogs with Python defaults.
pub use specs::{INTENTS, IntentSpec, ParamSpec, READS, ReadSpec};
