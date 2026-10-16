// Derived from RaoFoundation/subtensor sdk/python/bittensor/namespaces.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Typed Bittensor reads and namespaces.

#![deny(missing_docs)]
#![doc = include_str!("../README.md")]

/// Timelock-aware commitment decoding and pinned subnet scans.
pub mod commitments;
/// Interfaces regenerated from the pinned read catalog.
pub mod generated;

pub use generated::namespaces::*;
pub use generated::{
    Category, Hit, HitKind, JsonType, ParamSpec, Pinning, READ_HIT_REPLACEMENTS, READ_SPECS,
    ReadHitReplacement, ReadInterfaceMetadata, ReadName, ReadSpec, RenderHint, dispatch,
    fill_defaults,
};
/// Typed entry points for catalog reads.
pub mod reads;

use std::fmt;

/// Failure of a typed read or its decoded projection.
#[derive(Debug)]
#[non_exhaustive]
pub enum ReadError {
    /// Client transport, metadata or SCALE decoding failed.
    Client(rtao_client::ClientError),
    /// A commitment's decoded data could not be projected.
    Commitment(commitments::CommitmentError),
    /// An input parameter did not satisfy the read contract.
    InvalidParam {
        /// Parameter name.
        name: String,
        /// Why this value was rejected.
        reason: String,
    },
    /// The catalog interface exists but its behavior has not been ported yet.
    NotYetPorted(ReadName),
    /// A hand model no longer agrees with the live metadata.
    SurfaceChanged {
        /// Stable descriptor identity.
        descriptor: &'static str,
        /// Shape difference.
        detail: String,
    },
}

impl ReadError {
    /// Semantic code from the shared Python error taxonomy.
    pub fn code(&self) -> rtao_catalog::ErrorCode {
        match self {
            Self::Client(error) => error.code(),
            Self::Commitment(_) | Self::SurfaceChanged { .. } | Self::NotYetPorted(_) => {
                rtao_catalog::ErrorCode::Internal
            }
            Self::InvalidParam { .. } => rtao_catalog::ErrorCode::InvalidArgument,
        }
    }
}

impl fmt::Display for ReadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Client(error) => error.fmt(formatter),
            Self::Commitment(error) => error.fmt(formatter),
            Self::InvalidParam { name, reason } => write!(formatter, "invalid {name}: {reason}"),
            Self::NotYetPorted(name) => write!(formatter, "read not yet ported: {name}"),
            Self::SurfaceChanged { descriptor, detail } => {
                write!(formatter, "surface changed for {descriptor}: {detail}")
            }
        }
    }
}

impl std::error::Error for ReadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Client(error) => Some(error),
            Self::Commitment(error) => Some(error),
            _ => None,
        }
    }
}

impl From<rtao_client::ClientError> for ReadError {
    fn from(value: rtao_client::ClientError) -> Self {
        Self::Client(value)
    }
}

impl From<commitments::CommitmentError> for ReadError {
    fn from(value: commitments::CommitmentError) -> Self {
        Self::Commitment(value)
    }
}

impl<V: rtao_client::View + ?Sized> Subnets<'_, V> {
    /// Every current or revealed commitment, newest first, including deregistered hotkeys.
    pub async fn commitments(
        &self,
        netuid: rtao_primitives::NetUid,
    ) -> Result<commitments::CommitmentMap, ReadError> {
        commitments::fetch_commitments(self.view, netuid).await
    }
}
