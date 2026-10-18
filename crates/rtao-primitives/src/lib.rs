//! Identifiers, amounts and shared Bittensor vocabulary.

#![doc = include_str!("../README.md")]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![deny(missing_docs)]

mod account;
/// Chain IP addresses rendered as HTTP axon targets.
pub mod axon;
mod error;
mod fixed;
mod ids;
mod money;
mod python;
#[cfg(feature = "schemars")]
mod schema;
mod text;
mod value;
mod wire;

/// Metadata-directed visitors shared by primitive newtypes.
#[doc(hidden)]
pub mod decoding;
/// Local drand round, timestamp and duration helpers.
pub mod drand;
/// Portable EVM address normalization and native-account mappings.
pub mod evm;
/// Table-free conversion of human proportions into chain integers.
pub mod hyperparams;
/// Digit-exact, Value-first JSON text parsing and oracle comparisons.
pub mod json;
/// Python-compatible number and balance rendering.
pub mod pyfmt;
/// Metadata type identity based ratio normalization.
pub mod ratio;

pub use account::{AccountId, CryptoType, DefaultAccount, Ss58Address};
pub use error::UnitError;
pub use fixed::{
    I32F32, I64F64, I96F32, PerMillion, PerU16, Perbill, Percent, Permill, Ratio, U64F64,
    U64Normalized, U96F32,
};
pub use ids::{BlockNumber, H256, MechId, NetUid, NetUidStorageIndex, Uid};
pub use money::{
    Alpha, Amount, Balance, DynBalance, Money, ResolvedAmount, Spend, Symbol, Tao, Unit,
};
pub use text::CompactBytesText;
pub use value::IntoCoreValue;
pub use wire::{AlphaBalance, PriceRao, StakeWeight, TaoBalance};
