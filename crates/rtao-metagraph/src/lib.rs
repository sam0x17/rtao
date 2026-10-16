//! Bittensor metagraph projections and synchronization.

#![doc = include_str!("../README.md")]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![deny(missing_docs)]

mod fetch;
mod graph;
#[cfg(feature = "persist")]
#[cfg_attr(docsrs, doc(cfg(feature = "persist")))]
pub mod persist;
mod sync;
mod view;

pub use fetch::SubnetsMetagraphExt;
pub use graph::{
    DecodePath, DecodePreference, Metagraph, MetagraphHeader, MetagraphNeuron, MetagraphOptions,
    NeuronRef, SnapshotInfo,
};
pub use sync::{
    MetagraphDiff, MetagraphHandle, MetagraphSync, Preset, SyncCadence, SyncError, SyncHealth,
    SyncPolicy,
};
pub use view::MetagraphView;

use std::fmt;

/// A metagraph response could not be decoded or projected safely.
#[derive(Debug)]
pub enum MetagraphError {
    /// The selected positional or typed contract differs from the runtime.
    Shape(String),
    /// A runtime value is invalid for the chain's native model.
    Malformed(String),
    /// The response contains bytes after the complete runtime value.
    TrailingBytes,
    /// The core decoder rejected the response.
    Core(rtao_core::CoreError),
    /// The by-name decoder rejected the response.
    Typed(scale_decode::Error),
}

impl fmt::Display for MetagraphError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Shape(message) | Self::Malformed(message) => f.write_str(message),
            Self::TrailingBytes => f.write_str("metagraph response contains trailing bytes"),
            Self::Core(error) => error.fmt(f),
            Self::Typed(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for MetagraphError {}
impl From<rtao_core::CoreError> for MetagraphError {
    fn from(error: rtao_core::CoreError) -> Self {
        Self::Core(error)
    }
}
impl From<rtao_core::codec::Error> for MetagraphError {
    fn from(error: rtao_core::codec::Error) -> Self {
        Self::Core(error.into())
    }
}
impl From<scale_decode::Error> for MetagraphError {
    fn from(error: scale_decode::Error) -> Self {
        Self::Typed(error)
    }
}

/// Result from a metagraph decoder or projector.
pub type Result<T> = std::result::Result<T, MetagraphError>;
