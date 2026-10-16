//! Staged extensions for the upstream Bittensor core.

#![doc = include_str!("../README.md")]
#![deny(missing_docs)]
#![deny(
    clippy::arithmetic_side_effects,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::unwrap_used
)]
#![allow(
    clippy::manual_inspect,
    clippy::result_large_err,
    clippy::type_complexity
)]

pub mod epoch_schedule;
pub mod ir;
pub mod keyfile_env;
pub mod keyfile_json;
pub mod ops;
pub mod timelock_commit;
pub mod value_ext;
pub mod verify_strict;

pub use verify_strict::verify_strict;
