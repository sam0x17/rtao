// Derived from RaoFoundation/subtensor sdk/python/bittensor/reads/{staking,prices}.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Exact stake positions, availability, destinations and simulated swaps.

use std::collections::{BTreeMap, BTreeSet};

use futures::TryStreamExt;
use num_bigint::BigUint;
use num_traits::{ToPrimitive, Zero};
use rtao_client::{View, ViewExt};
use rtao_core::{codec::Value, value_ext::ValueExt};
use rtao_primitives::{AccountId, AlphaBalance, DynBalance, NetUid, Symbol, TaoBalance};
use rtao_runtime::{models, runtime_apis as api, storage as st};
use serde::Serialize;

use super::owner::StakePosition;
use crate::ReadError;

type Symbols = BTreeMap<NetUid, String>;

fn surface(descriptor: &'static str, detail: impl Into<String>) -> ReadError {
    ReadError::SurfaceChanged {
        descriptor,
        detail: detail.into(),
    }
}

fn balance(rao: u64, netuid: NetUid, symbols: &Symbols) -> DynBalance {
    let mut value = DynBalance::from_rao(rao, netuid);
    value.symbol = symbols.get(&netuid).map(|text| Symbol::new(text.clone()));
    value
}

fn position(record: models::StakeInfo, symbols: &Symbols) -> StakePosition {
    StakePosition {
        hotkey: record.hotkey,
        coldkey: record.coldkey,
        netuid: record.netuid,
        stake: balance(record.stake.0, record.netuid, symbols),
        is_registered: record.is_registered,
    }
}

/// Exact stake in the queried subnet's currency; a missing position is zero.
// py: sdk/python/bittensor/reads/staking.py:75-81
pub async fn stake<V: View + ?Sized>(
    view: &V,
    coldkey_ss58: AccountId,
    hotkey_ss58: AccountId,
    netuid: NetUid,
) -> Result<DynBalance, ReadError> {
    let snapshot = view.pin().await?;
    let record = snapshot
        .call(
            &api::stake_info_runtime_api::GET_STAKE_INFO_FOR_HOTKEY_COLDKEY_NETUID,
            (hotkey_ss58, coldkey_ss58, netuid),
        )
        .await?;
    Ok(balance(
        record.map_or(0, |record| record.stake.0),
        netuid,
        &snapshot.client().token_symbols().await,
    ))
}

/// Every runtime-reported position, in runtime order and its own currency.
// py: sdk/python/bittensor/reads/staking.py:90-98; _stake_position:55-62
pub async fn stake_for_coldkey<V: View + ?Sized>(
    view: &V,
    coldkey_ss58: AccountId,
) -> Result<Vec<StakePosition>, ReadError> {
    let snapshot = view.pin().await?;
    let records = snapshot
        .call(
            &api::stake_info_runtime_api::GET_STAKE_INFO_FOR_COLDKEY,
            (coldkey_ss58,),
        )
        .await?;
    let symbols = snapshot.client().token_symbols().await;
    Ok(records
        .into_iter()
        .map(|record| position(record, &symbols))
        .collect())
}

/// Runtime-reported coldkeys and their positions, with duplicate keys overwritten.
///
/// Coldkeys omitted by the runtime remain omitted; this is not a valuation read.
// py: sdk/python/bittensor/reads/staking.py:107-119; _stake_position:55-62
pub async fn stake_for_coldkeys<V: View + ?Sized>(
    view: &V,
    coldkey_ss58s: Vec<AccountId>,
) -> Result<BTreeMap<AccountId, Vec<StakePosition>>, ReadError> {
    let snapshot = view.pin().await?;
    let records = snapshot
        .call(
            &api::stake_info_runtime_api::GET_STAKE_INFO_FOR_COLDKEYS,
            (coldkey_ss58s,),
        )
        .await?;
    let symbols = snapshot.client().token_symbols().await;
    Ok(records
        .into_iter()
        .map(|(coldkey, records)| {
            (
                coldkey,
                records
                    .into_iter()
                    .map(|record| position(record, &symbols))
                    .collect(),
            )
        })
        .collect())
}

/// Coldkey-wide free and locked stake, denominated in the indicated subnet.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct StakeAvailability {
    /// Currency of all three balances (TAO on root).
    pub netuid: NetUid,
    /// Total stake held across the coldkey's hotkeys.
    pub total: DynBalance,
    /// Conviction-locked mass and reserved miner collateral.
    pub locked: DynBalance,
    /// Stake available without moving lock mass.
    pub available: DynBalance,
}

fn present(value: &Value) -> bool {
    !matches!(value, Value::Null) && !matches!(value, Value::Dict(entries) if entries.is_empty())
}

fn availability(
    raw: &Value,
    coldkey: &str,
    netuid: NetUid,
    symbols: &Symbols,
) -> Result<StakeAvailability, ReadError> {
    let row = raw.field(coldkey).and_then(|rows| {
        let Value::Dict(entries) = rows else {
            return None;
        };
        entries
            .iter()
            .find(|(key, _)| key.as_u128() == Some(u128::from(netuid.0)))
            .map(|(_, row)| row)
            .filter(|row| present(row))
            .or_else(|| rows.field(&netuid.to_string()))
    });
    let amount = |field: &str| -> Result<DynBalance, ReadError> {
        let rao = match row.and_then(|row| row.field(field)) {
            None | Some(Value::Null) => 0,
            Some(value) => value
                .as_u128()
                .and_then(|value| u64::try_from(value).ok())
                .ok_or_else(|| {
                    surface(
                        "StakeInfoRuntimeApi.get_stake_availability_for_coldkeys",
                        format!("{field} is not a native u64 balance"),
                    )
                })?,
        };
        Ok(balance(rao, netuid, symbols))
    };
    Ok(StakeAvailability {
        netuid,
        total: amount("total")?,
        locked: amount("locked")?,
        available: amount("available")?,
    })
}

/// Free and locked stake on one subnet, with zero balances for absent rows.
// py: sdk/python/bittensor/reads/staking.py:194-215
pub async fn stake_availability<V: View + ?Sized>(
    view: &V,
    coldkey_ss58: AccountId,
    netuid: NetUid,
) -> Result<StakeAvailability, ReadError> {
    let snapshot = view.pin().await?;
    let raw = snapshot
        .call(
            &api::stake_info_runtime_api::GET_STAKE_AVAILABILITY_FOR_COLDKEYS,
            (vec![coldkey_ss58], Some(vec![netuid])),
        )
        .await?;
    availability(
        raw.as_core(),
        &coldkey_ss58.to_ss58(snapshot.block().runtime.core().ss58_format),
        netuid,
        &snapshot.client().token_symbols().await,
    )
}

/// Sorted, deduplicated availability rows, using one runtime call.
///
/// An empty subnet list performs no chain reads.
// py: sdk/python/bittensor/reads/staking.py:227-248
pub async fn stake_availability_for_coldkey<V: View + ?Sized>(
    view: &V,
    coldkey_ss58: AccountId,
    netuids: Vec<NetUid>,
) -> Result<Vec<StakeAvailability>, ReadError> {
    if netuids.is_empty() {
        return Ok(Vec::new());
    }
    let ordered: Vec<_> = netuids
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let snapshot = view.pin().await?;
    let raw = snapshot
        .call(
            &api::stake_info_runtime_api::GET_STAKE_AVAILABILITY_FOR_COLDKEYS,
            (vec![coldkey_ss58], Some(ordered.clone())),
        )
        .await?;
    let coldkey = coldkey_ss58.to_ss58(snapshot.block().runtime.core().ss58_format);
    let symbols = snapshot.client().token_symbols().await;
    ordered
        .into_iter()
        .map(|netuid| availability(raw.as_core(), &coldkey, netuid, &symbols))
        .collect()
}

/// Chain-maintained total stake, in alpha or TAO on root.
///
/// During the v448 bounded backfill, keys ahead of the migration cursor can be
/// incomplete; the read preserves that chain value without inventing a sum.
// py: sdk/python/bittensor/reads/staking.py:257-268
pub async fn total_alpha_staked<V: View + ?Sized>(
    view: &V,
    netuid: NetUid,
) -> Result<DynBalance, ReadError> {
    let snapshot = view.pin().await?;
    let value = snapshot
        .storage(&st::subtensor_module::TOTAL_ALPHA_STAKED, (netuid,))
        .await?;
    Ok(balance(
        value.0,
        netuid,
        &snapshot.client().token_symbols().await,
    ))
}

/// Associated staking and root-basket hotkeys, preserving chain order.
// py: sdk/python/bittensor/reads/staking.py:277-288
pub async fn staking_hotkeys<V: View + ?Sized>(
    view: &V,
    coldkey_ss58: AccountId,
) -> Result<Vec<AccountId>, ReadError> {
    Ok(view
        .storage(&st::subtensor_module::STAKING_HOTKEYS, (coldkey_ss58,))
        .await?)
}

/// Reward auto-stake destination on one subnet, or `None` when unset.
// py: sdk/python/bittensor/reads/staking.py:300-303
pub async fn auto_stake<V: View + ?Sized>(
    view: &V,
    coldkey_ss58: AccountId,
    netuid: NetUid,
) -> Result<Option<AccountId>, ReadError> {
    Ok(view
        .storage(
            &st::subtensor_module::AUTO_STAKE_DESTINATION,
            (coldkey_ss58, netuid),
        )
        .await?)
}

/// A configured destination returned by the coldkey-scoped map scan.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AutoStakeDestination {
    /// Subnet where rewards are auto-staked.
    pub netuid: NetUid,
    /// Destination hotkey.
    pub hotkey: AccountId,
}

/// Every configured destination of this coldkey, sorted by subnet.
// py: sdk/python/bittensor/reads/staking.py:312-323
pub async fn auto_stake_all<V: View + ?Sized>(
    view: &V,
    coldkey_ss58: AccountId,
) -> Result<Vec<AutoStakeDestination>, ReadError> {
    let snapshot = view.pin().await?;
    let rows = snapshot.storage_iter(
        &st::subtensor_module::AUTO_STAKE_DESTINATION,
        (coldkey_ss58,),
    );
    futures::pin_mut!(rows);
    let mut result = Vec::new();
    while let Some((keys, hotkey)) = rows.try_next().await? {
        let netuid = keys
            .last()
            .and_then(rtao_core::DynValue::as_u128)
            .and_then(|value| u16::try_from(value).ok())
            .ok_or_else(|| surface("SubtensorModule.AutoStakeDestination", "invalid subnet key"))?;
        result.push(AutoStakeDestination {
            netuid: NetUid(netuid),
            hotkey,
        });
    }
    result.sort_by_key(|row| row.netuid);
    Ok(result)
}

/// Spot alpha price in TAO, together with its exact native rao price.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AlphaPrice {
    /// Queried subnet.
    pub netuid: NetUid,
    /// Correctly rounded TAO per alpha, as a binary64 estimate.
    pub tao_per_alpha: f64,
    /// Exact runtime price before conversion to TAO.
    pub price_rao: u64,
}

/// Current spot price for one subnet; root's value is supplied by the runtime.
// py: sdk/python/bittensor/reads/prices.py:88-95
pub async fn alpha_price<V: View + ?Sized>(
    view: &V,
    netuid: NetUid,
) -> Result<AlphaPrice, ReadError> {
    let price_rao = view
        .call(&api::swap_runtime_api::CURRENT_ALPHA_PRICE, (netuid,))
        .await?;
    Ok(AlphaPrice {
        netuid,
        tao_per_alpha: rtao_primitives::pyfmt::rao_to_f64(price_rao),
        price_rao,
    })
}

/// Spot prices in TAO per alpha, keeping the last row for duplicate subnets.
// py: sdk/python/bittensor/reads/prices.py:76-79
pub async fn alpha_prices<V: View + ?Sized>(view: &V) -> Result<BTreeMap<NetUid, f64>, ReadError> {
    Ok(view
        .call(&api::swap_runtime_api::CURRENT_ALPHA_PRICE_ALL, ())
        .await?
        .into_iter()
        .map(|record| {
            (
                record.netuid,
                rtao_primitives::pyfmt::rao_to_f64(record.price.0),
            )
        })
        .collect())
}

/// Simulated swap amounts, fees and slippage, tagged with their currencies.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SwapQuote {
    /// TAO-side amount, without a chain display symbol (Python `from_rao`).
    pub tao: DynBalance,
    /// Alpha-side amount, or TAO for a root quote.
    pub alpha: DynBalance,
    /// TAO-side fee.
    pub tao_fee: DynBalance,
    /// Alpha-side fee.
    pub alpha_fee: DynBalance,
    /// TAO-side slippage.
    pub tao_slippage: DynBalance,
    /// Alpha-side slippage.
    pub alpha_slippage: DynBalance,
}

fn quote(record: models::SimSwapResult, netuid: NetUid, symbols: &Symbols) -> SwapQuote {
    SwapQuote {
        tao: DynBalance::from_rao(record.tao_amount.0, NetUid::ROOT),
        alpha: balance(record.alpha_amount.0, netuid, symbols),
        tao_fee: DynBalance::from_rao(record.tao_fee.0, NetUid::ROOT),
        alpha_fee: balance(record.alpha_fee.0, netuid, symbols),
        tao_slippage: DynBalance::from_rao(record.tao_slippage.0, NetUid::ROOT),
        alpha_slippage: balance(record.alpha_slippage.0, netuid, symbols),
    }
}

// Python's quote convenience input is Decimal(str(amount)), rounded to integral
// rao with ties to even. Multiplying the binary64 directly would double-round.
// The decimal is bounded by a single finite f64, rather than external text.
fn quote_rao(amount: f64, name: &str) -> Result<u64, ReadError> {
    let invalid = |reason: &str| ReadError::InvalidParam {
        name: name.into(),
        reason: reason.into(),
    };
    if !amount.is_finite() || amount < 0.0 {
        return Err(invalid("amount must be finite and non-negative"));
    }
    let text = rtao_primitives::pyfmt::float_repr(amount.abs());
    let (mantissa, exponent) = match text.split_once(['e', 'E']) {
        Some((mantissa, exponent)) => (
            mantissa,
            exponent
                .parse::<i32>()
                .map_err(|_| invalid("invalid exponent"))?,
        ),
        None => (text.as_str(), 0),
    };
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let digits = format!("{whole}{fraction}");
    let value = BigUint::parse_bytes(digits.as_bytes(), 10)
        .ok_or_else(|| invalid("invalid decimal amount"))?;
    let fraction_len = i32::try_from(fraction.len()).map_err(|_| invalid("amount is too long"))?;
    let power = 9 + exponent - fraction_len;
    let rounded = if power >= 0 {
        value * BigUint::from(10u8).pow(power.unsigned_abs())
    } else {
        let divisor = BigUint::from(10u8).pow(power.unsigned_abs());
        let mut quotient = &value / &divisor;
        let twice_remainder = (&value % &divisor) * 2u8;
        if twice_remainder > divisor || (twice_remainder == divisor && !(&quotient % 2u8).is_zero())
        {
            quotient += 1u8;
        }
        quotient
    };
    rounded
        .to_u64()
        .ok_or_else(|| invalid("amount exceeds native u64 rao"))
}

/// Simulate staking a TAO amount, retaining the runtime's fee and slippage.
// py: sdk/python/bittensor/reads/prices.py:45-54; balance.py:89-95
pub async fn quote_stake<V: View + ?Sized>(
    view: &V,
    netuid: NetUid,
    amount_tao: f64,
) -> Result<SwapQuote, ReadError> {
    let amount = TaoBalance(quote_rao(amount_tao, "amount_tao")?);
    let snapshot = view.pin().await?;
    let record = snapshot
        .call(
            &api::swap_runtime_api::SIM_SWAP_TAO_FOR_ALPHA,
            (netuid, amount),
        )
        .await?;
    Ok(quote(
        record,
        netuid,
        &snapshot.client().token_symbols().await,
    ))
}

/// Simulate unstaking alpha, retaining the runtime's fee and slippage.
///
/// On root, `amount_alpha` follows Python's variable-unit amount constructor
/// and denotes TAO; no subnet alpha is invented for netuid zero.
// py: sdk/python/bittensor/reads/prices.py:66-72; balance.py:89-95
pub async fn quote_unstake<V: View + ?Sized>(
    view: &V,
    netuid: NetUid,
    amount_alpha: f64,
) -> Result<SwapQuote, ReadError> {
    let amount = AlphaBalance(quote_rao(amount_alpha, "amount_alpha")?);
    let snapshot = view.pin().await?;
    let record = snapshot
        .call(
            &api::swap_runtime_api::SIM_SWAP_ALPHA_FOR_TAO,
            (netuid, amount),
        )
        .await?;
    Ok(quote(
        record,
        netuid,
        &snapshot.client().token_symbols().await,
    ))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn quote_decimal_rounding_matches_python_ties_even_without_binary_multiplication() {
        for (amount, rao) in [
            (0.0, 0),
            (-0.0, 0),
            (0.0000000005, 0),
            (0.0000000015, 2),
            (0.0000000025, 2),
            (1.0000000005, 1_000_000_000),
            (1.0000000015, 1_000_000_002),
            (0.000000000000001, 0),
            (18_446_744_073.709_55, 18_446_744_073_709_550_000),
        ] {
            assert_eq!(quote_rao(amount, "amount").unwrap(), rao, "{amount}");
        }
        for amount in [
            -1.0,
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::MAX,
            18_446_744_073.709_553,
        ] {
            assert!(quote_rao(amount, "amount").is_err(), "{amount}");
        }
    }

    #[test]
    fn availability_accepts_integer_and_decoder_string_keys_but_rejects_native_overflow() {
        let record = |value| Value::record(vec![("available".into(), value)]);
        let raw = |entries| Value::record(vec![("cold".into(), Value::Dict(entries))]);
        let symbols = Symbols::new();
        let value = raw(vec![
            (Value::Int(1), Value::Dict(Vec::new())),
            (Value::Str("1".into()), record(Value::Int(7))),
        ]);
        let result = availability(&value, "cold", NetUid(1), &symbols).unwrap();
        assert_eq!(
            (result.total.rao, result.locked.rao, result.available.rao),
            (0, 0, 7)
        );
        let value = raw(vec![
            (Value::Int(1), record(Value::Uint(u128::from(u64::MAX)))),
            (Value::Str("1".into()), record(Value::Int(7))),
        ]);
        assert_eq!(
            availability(&value, "cold", NetUid(1), &symbols)
                .unwrap()
                .available
                .rao,
            u64::MAX
        );
        for amount in [Value::Int(-1), Value::Uint(u128::from(u64::MAX) + 1)] {
            let value = raw(vec![(Value::Int(1), record(amount))]);
            assert!(availability(&value, "cold", NetUid(1), &symbols).is_err());
        }
    }
}
