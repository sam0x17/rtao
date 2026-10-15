use parity_scale_codec::{CompactAs, Decode, Encode};
use scale_decode::{IntoVisitor, TypeResolver};
use scale_info::TypeInfo;

use crate::{
    IntoCoreValue,
    decoding::{FromWire, WireVisitor},
};

macro_rules! amount {
    ($name:ident, $doc:literal) => {
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
        pub struct $name(#[doc = "The exact u64 wire amount."] pub u64);
        impl FromWire for $name {
            type Wire = u64;
            fn from_wire(value: u64) -> Result<Self, scale_decode::Error> {
                Ok(Self(value))
            }
        }
        impl IntoVisitor for $name {
            type AnyVisitor<R: TypeResolver> = WireVisitor<Self, R>;
            fn into_visitor<R: TypeResolver>() -> Self::AnyVisitor<R> {
                WireVisitor::new()
            }
        }
        impl IntoCoreValue for $name {
            fn into_core_value(self) -> rtao_core::codec::Value {
                self.0.into_core_value()
            }
        }
    };
}

amount!(TaoBalance, "A TAO wire amount measured in rao.");
amount!(
    AlphaBalance,
    "An alpha wire amount whose subnet is supplied by call context."
);
amount!(PriceRao, "A wire price in rao per alpha token.");
amount!(
    StakeWeight,
    "A wire stake weight combining alpha and TAO times the TAO weight."
);
