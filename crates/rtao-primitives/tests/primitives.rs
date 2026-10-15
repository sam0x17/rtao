//! Identifier, wire amount, and fixed-point conversion contracts.

use std::error::Error;

use parity_scale_codec::{Compact, Decode, Encode};
use proptest::prelude::*;
use rtao_core::codec::Value;
use rtao_primitives::{
    AccountId, AlphaBalance, CryptoType, DefaultAccount, I32F32, I64F64, I96F32, IntoCoreValue,
    MechId, NetUid, NetUidStorageIndex, PriceRao, Ratio, StakeWeight, TaoBalance, U64F64,
};
use scale_decode::DecodeAsType;
use scale_info::{PortableRegistry, TypeInfo};

fn registry<T: TypeInfo + 'static>() -> (u32, PortableRegistry) {
    let mut registry = scale_info::Registry::new();
    let id = registry.register_type(&scale_info::MetaType::new::<T>()).id;
    (id, registry.into())
}

fn decodes<S, D>(source: S, expected: D) -> Result<(), Box<dyn Error>>
where
    S: TypeInfo + Encode + 'static,
    D: DecodeAsType + PartialEq + std::fmt::Debug,
{
    let (id, registry) = registry::<S>();
    let encoded = source.encode();
    let mut remaining = encoded.as_slice();
    assert_eq!(D::decode_as_type(&mut remaining, id, &registry)?, expected);
    assert!(remaining.is_empty());
    Ok(())
}

#[test]
fn identifiers_and_amounts_decode_bare_wrapped_nested_and_compact() -> Result<(), Box<dyn Error>> {
    #[derive(TypeInfo, Encode, parity_scale_codec::CompactAs)]
    struct RuntimeNetUid(u16);
    #[derive(TypeInfo, Encode)]
    struct Outer(RuntimeNetUid);
    #[derive(TypeInfo, Encode)]
    struct RuntimeTaoBalance(u64);
    decodes(4095u16, NetUid(4095))?;
    decodes(RuntimeNetUid(77), NetUid(77))?;
    decodes(Outer(RuntimeNetUid(91)), NetUid(91))?;
    decodes(Compact(123u16), NetUid(123))?;
    decodes(Compact(RuntimeNetUid(17)), NetUid(17))?;
    decodes(255u8, MechId(255))?;
    decodes(u16::MAX, NetUidStorageIndex(u16::MAX))?;
    decodes(RuntimeTaoBalance(u64::MAX), TaoBalance(u64::MAX))?;
    decodes(Compact(1_000_000_000u64), AlphaBalance(1_000_000_000))?;
    decodes(Compact(999u64), PriceRao(999))?;
    decodes(Compact(999u64), StakeWeight(999))?;
    let (id, registry) = registry::<u32>();
    assert!(NetUid::decode_as_type(&mut u32::MAX.encode().as_slice(), id, &registry).is_err());
    Ok(())
}

#[test]
fn account_default_is_typed_optional_but_keeps_zero_account_ss58() -> Result<(), Box<dyn Error>> {
    let raw = AccountId::ZERO;
    let account = DefaultAccount::new(raw);
    assert_eq!(account.get(), None);
    assert_eq!(account.raw(), &raw);
    assert_eq!(account.to_string(), raw.to_string());
    decodes([0u8; 32], account)?;
    decodes(([5u8; 32],), AccountId([5; 32]))?;
    assert_eq!(
        DefaultAccount::new(AccountId([5; 32])).get(),
        Some(&AccountId([5; 32]))
    );
    #[cfg(feature = "serde")]
    {
        assert_eq!(
            serde_json::to_string(&account)?,
            serde_json::to_string(&raw)?
        );
        assert_eq!(
            rtao_primitives::json::from_str::<DefaultAccount>(&serde_json::to_string(&account)?)?,
            account
        );
    }
    Ok(())
}

#[test]
fn identifiers_use_core_bytes_and_checked_storage_index_has_no_aliases()
-> Result<(), Box<dyn Error>> {
    assert_eq!(NetUid(2).into_core_value(), Value::Int(2));
    assert_eq!(
        AccountId([8; 32]).into_core_value(),
        Value::Bytes(vec![8; 32])
    );
    assert_eq!(
        TaoBalance(u64::MAX).into_core_value(),
        Value::Int(i128::from(u64::MAX))
    );
    assert_eq!(u128::MAX.into_core_value(), Value::Uint(u128::MAX));
    assert_eq!(
        (NetUid(1), MechId(2)).into_core_value(),
        Value::Tuple(vec![Value::Int(1), Value::Int(2)])
    );
    assert!(NetUidStorageIndex::new(NetUid(4096), MechId(0)).is_err());
    assert!(NetUidStorageIndex::new(NetUid(0), MechId(16)).is_err());
    assert!(MechId::new(255).is_err());
    assert!(NetUid::new(u16::MAX).is_err());
    assert_eq!(
        NetUidStorageIndex::new(NetUid(4095), MechId(15))?.0,
        u16::MAX
    );
    assert_eq!(CryptoType::Ed25519.encode(), vec![0]);
    assert_eq!(CryptoType::Sr25519.encode(), vec![1]);
    assert!(CryptoType::try_from(2).is_err());
    assert_eq!(
        rtao_primitives::DynBalance::from_rao(u64::MAX, NetUid::ROOT).try_into_core_value()?,
        Value::Int(i128::from(u64::MAX))
    );
    assert!(rtao_primitives::DynBalance::try_from_rao(-1, NetUid::ROOT).is_err());
    assert!(
        rtao_primitives::DynBalance::try_from_unsigned_rao(1_u128 << 80, NetUid::ROOT).is_err()
    );
    for value in [TaoBalance(0), TaoBalance(1000), TaoBalance(u64::MAX)] {
        assert_eq!(TaoBalance::decode(&mut value.encode().as_slice())?, value);
        let compact = Compact(value);
        assert_eq!(
            Compact::<TaoBalance>::decode(&mut compact.encode().as_slice())?,
            compact
        );
    }
    Ok(())
}

#[test]
fn fixed_point_values_decode_by_bits_and_render_exact_decimal() -> Result<(), Box<dyn Error>> {
    #[derive(TypeInfo, Encode)]
    struct RuntimeFixedU128 {
        bits: u128,
    }
    #[derive(TypeInfo, Encode)]
    struct RuntimeFixedI128 {
        bits: i128,
    }
    #[derive(TypeInfo, Encode)]
    struct RuntimeFixedI64 {
        bits: i64,
    }
    decodes(
        RuntimeFixedU128 { bits: u128::MAX },
        U64F64::from_bits(u128::MAX),
    )?;
    decodes(
        RuntimeFixedI128 { bits: i128::MIN },
        I96F32::from_bits(i128::MIN),
    )?;
    decodes(
        RuntimeFixedI128 { bits: i128::MIN },
        I64F64::from_bits(i128::MIN),
    )?;
    decodes(
        RuntimeFixedI64 { bits: i64::MIN },
        I32F32::from_bits(i64::MIN),
    )?;
    assert_eq!(U64F64::from_bits(1u128 << 63).to_string(), "0.5");
    assert_eq!(
        I96F32::from_bits(-1).to_string(),
        "-0.00000000023283064365386962890625"
    );
    assert_eq!(I32F32::from_bits(-1).int_part(), -1);
    assert_eq!(
        I96F32::from_bits(i128::MIN).to_string(),
        "-39614081257132168796771975168"
    );
    assert_eq!(
        I64F64::from_bits(i128::MIN).to_string(),
        "-9223372036854775808"
    );
    assert_eq!(
        I96F32::from_bits(7).into_core_value(),
        Value::record(vec![("bits".into(), Value::Int(7))])
    );
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(Ratio::new(value).is_err());
    }
    #[cfg(feature = "serde")]
    for input in ["0.05", "1e-05", "\"0.05\""] {
        let ratio: Ratio = rtao_primitives::json::from_str(input)?;
        assert!(ratio.get().is_finite());
    }
    Ok(())
}

#[cfg(feature = "schemars")]
#[test]
fn money_schemas_match_number_decimal_and_all_input_shapes() -> Result<(), Box<dyn Error>> {
    let money = serde_json::to_value(schemars::schema_for!(rtao_primitives::Money))?;
    let amount = serde_json::to_value(schemars::schema_for!(rtao_primitives::Amount))?;
    assert_eq!(
        money["anyOf"]
            .as_array()
            .ok_or("money schema missing anyOf")?
            .len(),
        2
    );
    assert_eq!(
        amount["anyOf"]
            .as_array()
            .ok_or("amount schema missing anyOf")?
            .len(),
        3
    );
    assert_eq!(amount["anyOf"][2]["enum"][0], "all");
    assert_eq!(money["anyOf"][0]["minimum"], 0);
    assert_eq!(money["anyOf"][1]["pattern"], "^[0-9]+(\\.[0-9]{1,9})?$");
    let account = serde_json::to_value(schemars::schema_for!(AccountId))?;
    assert_eq!(account["type"], "string");
    Ok(())
}

proptest! {
    // py: sdk/python/bittensor/settings.py:31 (GLOBAL_MAX_SUBNET_COUNT)
    // py: sdk/python/bittensor/intents/weights.py:167 (_already_revealed storage index)
    #[test]
    fn storage_index_roundtrips_all_wire_indices(index in any::<u16>()) {
        let combined = NetUidStorageIndex(index);
        let (netuid, mech) = combined.split();
        prop_assert_eq!(NetUidStorageIndex::new(netuid, mech), Ok(combined));
    }

    #[test]
    fn account_ss58_uses_core_and_roundtrips(bytes in any::<[u8;32]>(), prefix in 0u16..=16383) {
        let account = AccountId(bytes);
        prop_assert_eq!(account.to_string(), rtao_core::keys::ss58_from_public(bytes, 42));
        prop_assert_eq!(account.to_string().parse::<AccountId>().ok(), Some(account));
        // The core rejects reserved and unknown prefixes; adapters preserve that policy.
        let address = account.to_ss58(prefix);
        prop_assert_eq!(address.parse::<AccountId>().ok(), rtao_core::keys::public_key_from_ss58(&address).ok().map(AccountId));
    }

    #[test]
    fn fixed_point_float_conversion_is_one_integer_rounding(bits in any::<i128>()) {
        prop_assert_eq!(I96F32::from_bits(bits).to_f64().to_bits(), (bits as f64 / 4_294_967_296.0).to_bits());
        prop_assert_eq!(I64F64::from_bits(bits).to_f64().to_bits(), (bits as f64 / 18_446_744_073_709_551_616.0).to_bits());
    }
}
