//! Typed, by-name projections of runtime return values.

mod hand;
mod metagraph;
mod serving;
#[cfg(test)]
mod tests;

pub use hand::*;
pub use metagraph::{MetagraphWire, SelectiveMetagraphWire};
pub use serving::{AxonInfo, ChainIdentityV2, SubnetIdentityV3};

use rtao_core::codec::Value;
use rtao_primitives::{
    AccountId, AlphaBalance, CompactBytesText, I96F32, NetUid, NetUidStorageIndex, PerU16,
    StakeWeight, TaoBalance,
};

/// Project a typed wire model into the upstream decoder's value conventions.
pub trait ToDynamic {
    /// Produce the runtime-shaped value, retaining full integer precision.
    fn to_dynamic(&self) -> Value;
}

macro_rules! number {
    ($($ty:ty),*) => {$(impl ToDynamic for $ty {
        fn to_dynamic(&self) -> Value { Value::Int(i128::from(*self)) }
    })*};
}
number!(u8, u16, u32, u64, i128);
impl ToDynamic for u128 {
    fn to_dynamic(&self) -> Value {
        i128::try_from(*self).map_or(Value::Uint(*self), Value::Int)
    }
}
macro_rules! newtype {
    ($($ty:ty),*) => {$(impl ToDynamic for $ty {
        fn to_dynamic(&self) -> Value { self.0.to_dynamic() }
    })*};
}
newtype!(
    AlphaBalance,
    TaoBalance,
    StakeWeight,
    NetUid,
    NetUidStorageIndex,
    PerU16
);
impl ToDynamic for bool {
    fn to_dynamic(&self) -> Value {
        Value::Bool(*self)
    }
}
impl ToDynamic for AccountId {
    fn to_dynamic(&self) -> Value {
        Value::Str(self.to_string())
    }
}
impl ToDynamic for I96F32 {
    fn to_dynamic(&self) -> Value {
        Value::record(vec![("bits".into(), self.bits.to_dynamic())])
    }
}
impl ToDynamic for CompactBytesText {
    fn to_dynamic(&self) -> Value {
        self.0.to_dynamic()
    }
}
impl<T: ToDynamic> ToDynamic for Vec<T> {
    fn to_dynamic(&self) -> Value {
        Value::List(self.iter().map(ToDynamic::to_dynamic).collect())
    }
}
impl<T: ToDynamic> ToDynamic for Option<T> {
    fn to_dynamic(&self) -> Value {
        self.as_ref().map_or(Value::Null, ToDynamic::to_dynamic)
    }
}
impl<A: ToDynamic, B: ToDynamic> ToDynamic for (A, B) {
    fn to_dynamic(&self) -> Value {
        Value::Tuple(vec![self.0.to_dynamic(), self.1.to_dynamic()])
    }
}

/// Core convention for a byte vector: UTF-8 text, or `0x` hex when invalid.
pub fn bytes_dynamic(bytes: &[u8]) -> Value {
    std::str::from_utf8(bytes).map_or_else(|_| Value::hex(bytes), |text| Value::Str(text.into()))
}
