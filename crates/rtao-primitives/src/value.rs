use rtao_core::codec::Value;

use crate::{
    AccountId, BlockNumber, CryptoType, DefaultAccount, H256, MechId, NetUid, NetUidStorageIndex,
    Ss58Address, Uid,
};

impl<U: crate::Unit> IntoCoreValue for crate::Balance<U> {
    fn into_core_value(self) -> Value {
        self.rao().into_core_value()
    }
}

impl crate::DynBalance {
    /// Validate the currency tag before sending a native balance to the core.
    ///
    /// Call builders still validate the currency and use the corresponding typed
    /// balance or wire newtype. The stored amount already has the chain's u64 range.
    pub fn try_into_core_value(&self) -> Result<Value, crate::UnitError> {
        crate::NetUid::new(self.netuid.get())?;
        Ok(self.rao.into_core_value())
    }
}

/// Convert a typed argument into the core's single dynamic SCALE encoder carrier.
pub trait IntoCoreValue {
    /// Consume the value without independently reimplementing chain encoding.
    fn into_core_value(self) -> Value;
}

impl IntoCoreValue for Value {
    fn into_core_value(self) -> Value {
        self
    }
}

impl IntoCoreValue for rtao_core::DynValue {
    fn into_core_value(self) -> Value {
        self.into_core()
    }
}

macro_rules! integers {
    ($($ty:ty),+ $(,)?) => {$ (
        impl IntoCoreValue for $ty {
            fn into_core_value(self) -> Value { Value::Int(i128::from(self)) }
        }
    )+};
}
integers!(u8, u16, u32, u64, i8, i16, i32, i64, i128);

impl IntoCoreValue for u128 {
    fn into_core_value(self) -> Value {
        i128::try_from(self)
            .map(Value::Int)
            .unwrap_or(Value::Uint(self))
    }
}

macro_rules! identifiers {
    ($($ty:ty),+ $(,)?) => {$ (
        impl IntoCoreValue for $ty {
            fn into_core_value(self) -> Value { self.0.into_core_value() }
        }
    )+};
}
identifiers!(NetUid, MechId, Uid, BlockNumber, NetUidStorageIndex);

impl IntoCoreValue for AccountId {
    fn into_core_value(self) -> Value {
        Value::Bytes(self.0.to_vec())
    }
}

impl IntoCoreValue for Ss58Address {
    fn into_core_value(self) -> Value {
        self.account().into_core_value()
    }
}

impl IntoCoreValue for DefaultAccount {
    fn into_core_value(self) -> Value {
        (*self.raw()).into_core_value()
    }
}

impl IntoCoreValue for H256 {
    fn into_core_value(self) -> Value {
        Value::Bytes(self.0.to_vec())
    }
}

impl IntoCoreValue for CryptoType {
    fn into_core_value(self) -> Value {
        u8::from(self).into_core_value()
    }
}

impl IntoCoreValue for bool {
    fn into_core_value(self) -> Value {
        Value::Bool(self)
    }
}

impl IntoCoreValue for () {
    fn into_core_value(self) -> Value {
        Value::Null
    }
}

impl IntoCoreValue for String {
    fn into_core_value(self) -> Value {
        Value::Str(self)
    }
}

impl IntoCoreValue for &str {
    fn into_core_value(self) -> Value {
        Value::Str(self.into())
    }
}

impl<T: IntoCoreValue> IntoCoreValue for Vec<T> {
    fn into_core_value(self) -> Value {
        Value::List(
            self.into_iter()
                .map(IntoCoreValue::into_core_value)
                .collect(),
        )
    }
}

impl<T: IntoCoreValue, const N: usize> IntoCoreValue for [T; N] {
    fn into_core_value(self) -> Value {
        Value::List(
            self.into_iter()
                .map(IntoCoreValue::into_core_value)
                .collect(),
        )
    }
}

impl<T: IntoCoreValue> IntoCoreValue for Option<T> {
    fn into_core_value(self) -> Value {
        self.map_or(Value::Null, IntoCoreValue::into_core_value)
    }
}

impl<T: Clone + IntoCoreValue> IntoCoreValue for &T {
    fn into_core_value(self) -> Value {
        self.clone().into_core_value()
    }
}

macro_rules! tuples {
    ($(($($ty:ident : $idx:tt),+)),+ $(,)?) => {$ (
        impl<$($ty: IntoCoreValue),+> IntoCoreValue for ($($ty,)+) {
            fn into_core_value(self) -> Value { Value::Tuple(vec![$(self.$idx.into_core_value()),+]) }
        }
    )+};
}
tuples!((A:0), (A:0, B:1), (A:0, B:1, C:2), (A:0, B:1, C:2, D:3), (A:0, B:1, C:2, D:3, E:4), (A:0, B:1, C:2, D:3, E:4, F:5));
