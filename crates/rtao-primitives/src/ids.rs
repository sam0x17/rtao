// Derived from RaoFoundation/subtensor sdk/python/bittensor/settings.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/intents/weights.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.

use std::fmt;

use parity_scale_codec::{CompactAs, Decode, Encode};
use scale_decode::{IntoVisitor, TypeResolver};
use scale_info::TypeInfo;

use crate::{
    UnitError,
    decoding::{FromWire, WireVisitor},
};

macro_rules! id {
    ($name:ident, $wire:ty, $doc:literal) => {
        #[doc = $doc]
        #[derive(
            Copy,
            Clone,
            Debug,
            PartialEq,
            Eq,
            PartialOrd,
            Ord,
            Hash,
            Default,
            Encode,
            Decode,
            CompactAs,
            TypeInfo,
        )]
        #[cfg_attr(
            feature = "serde",
            derive(serde::Serialize, serde::Deserialize),
            serde(transparent)
        )]
        #[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
        pub struct $name(#[doc = "The raw wire value."] pub $wire);
        impl $name {
            /// Recover the exact wire value.
            pub const fn get(self) -> $wire {
                self.0
            }
        }
        impl FromWire for $name {
            type Wire = $wire;
            fn from_wire(value: $wire) -> Result<Self, scale_decode::Error> {
                Ok(Self(value))
            }
        }
        impl IntoVisitor for $name {
            type AnyVisitor<R: TypeResolver> = WireVisitor<Self, R>;
            fn into_visitor<R: TypeResolver>() -> Self::AnyVisitor<R> {
                WireVisitor::new()
            }
        }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }
        impl From<$wire> for $name {
            fn from(value: $wire) -> Self {
                Self(value)
            }
        }
        impl From<$name> for $wire {
            fn from(value: $name) -> Self {
                value.0
            }
        }
    };
}

id!(
    NetUid,
    u16,
    "A subnet identifier; zero denotes the root TAO subnet."
);
id!(MechId, u8, "A mechanism identifier within a subnet.");
id!(
    Uid,
    u16,
    "A neuron identifier within a mechanism's metagraph."
);
id!(BlockNumber, u64, "A chain block number.");
id!(
    NetUidStorageIndex,
    u16,
    "The combined subnet/mechanism storage index."
);

impl NetUid {
    /// The root subnet, whose currency is TAO.
    pub const ROOT: Self = Self(0);
    /// Exclusive bound on subnet identifiers accepted by combined storage keys.
    pub const LIMIT: u16 = NetUidStorageIndex::STRIDE;
    /// Check a subnet identifier against the global subnet-count bound.
    pub fn new(value: u16) -> Result<Self, UnitError> {
        if value >= Self::LIMIT {
            Err(UnitError::InvalidIdentifier {
                kind: "netuid",
                value: u64::from(value),
                limit: u64::from(Self::LIMIT),
            })
        } else {
            Ok(Self(value))
        }
    }
}

impl MechId {
    /// Exclusive mechanism-count bound imposed by the u16 storage index.
    pub const LIMIT: u8 = 16;
    /// Check a mechanism identifier against the maximum mechanism count.
    pub fn new(value: u8) -> Result<Self, UnitError> {
        if value >= Self::LIMIT {
            Err(UnitError::InvalidIdentifier {
                kind: "mechid",
                value: u64::from(value),
                limit: u64::from(Self::LIMIT),
            })
        } else {
            Ok(Self(value))
        }
    }
}

impl NetUidStorageIndex {
    /// The global maximum subnet count from pinned Python settings.
    pub const STRIDE: u16 = 4096;
    /// Construct a storage key without aliasing out-of-range subnet identifiers.
    pub fn new(netuid: NetUid, mech: MechId) -> Result<Self, UnitError> {
        NetUid::new(netuid.0)?;
        MechId::new(mech.0)?;
        let offset = u16::from(mech.0)
            .checked_mul(Self::STRIDE)
            .ok_or(UnitError::Overflow)?;
        offset
            .checked_add(netuid.0)
            .map(Self)
            .ok_or(UnitError::Overflow)
    }
    /// Recover the subnet and mechanism represented by any u16 wire index.
    pub const fn split(self) -> (NetUid, MechId) {
        (
            NetUid(self.0 % Self::STRIDE),
            MechId((self.0 / Self::STRIDE) as u8),
        )
    }
}

/// A 256-bit block, genesis, or transaction hash.
#[derive(
    Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Encode, Decode, TypeInfo,
)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(transparent)
)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct H256(
    /// The exact 32-byte hash.
    pub [u8; 32],
);

impl FromWire for H256 {
    type Wire = [u8; 32];
    fn from_wire(value: Self::Wire) -> Result<Self, scale_decode::Error> {
        Ok(Self(value))
    }
}

impl IntoVisitor for H256 {
    type AnyVisitor<R: TypeResolver> = WireVisitor<Self, R>;
    fn into_visitor<R: TypeResolver>() -> Self::AnyVisitor<R> {
        WireVisitor::new()
    }
}
