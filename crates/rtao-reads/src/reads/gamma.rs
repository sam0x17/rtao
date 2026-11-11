// Derived from RaoFoundation/subtensor sdk/python/bittensor/reads/gamma.py @ b6a81d144078eac18bd25c3af7cb31579bb670f3 (Apache-2.0); modified by rtao.
//! Gamma reads: deposits, mint quotes, and the TAO/USD price.
//!
//! Gamma amounts are exact decimal strings of gamma, as in Python; see
//! [`rtao_primitives::gamma`].

use std::collections::BTreeMap;

use rtao_client::{View, ViewExt};
use rtao_core::{DynValue, codec::Value};
use rtao_primitives::{
    AccountId, AlphaBalance, DynBalance, NetUid, Symbol, TaoBalance, gamma::gamma_from_units,
};
use rtao_runtime::{runtime_apis as api, storage as st};
use serde::Serialize;

use crate::ReadError;

/// Most the accepted TAO/USD price rises per block, in parts per billion of
/// itself (`TAO_USD_PRICE_MAX_STEP`).
pub const TAO_USD_PRICE_MAX_STEP: u128 = 66_000;

const DEPOSIT_API: &str = "GammaRuntimeApi.get_gamma_deposit";
const QUOTE_API: &str = "GammaRuntimeApi.sim_mint_gamma";
const PRICE_STORAGE: &str = "SubtensorModule.TaoUsdPrice";

fn surface(descriptor: &'static str, detail: impl Into<String>) -> ReadError {
    ReadError::SurfaceChanged {
        descriptor,
        detail: detail.into(),
    }
}

fn field<'a>(value: &'a Value, name: &str) -> Option<&'a Value> {
    match value {
        Value::Dict(entries) => entries.iter().find_map(|(key, value)| {
            matches!(key, Value::Str(key) if key == name).then_some(value)
        }),
        _ => None,
    }
}

fn uint(value: &Value) -> Option<u128> {
    match value {
        Value::Int(value) => u128::try_from(*value).ok(),
        Value::Uint(value) => Some(*value),
        // Newtype wrappers decode as their single field.
        Value::Tuple(items) | Value::List(items) if items.len() == 1 => uint(&items[0]),
        _ => None,
    }
}

fn number(value: &Value, name: &str, descriptor: &'static str) -> Result<u128, ReadError> {
    field(value, name)
        .and_then(uint)
        .ok_or_else(|| surface(descriptor, format!("{name} is not an unsigned integer")))
}

fn account(value: &Value, name: &str, descriptor: &'static str) -> Result<AccountId, ReadError> {
    let text = match field(value, name) {
        Some(Value::Str(text)) => text,
        _ => return Err(surface(descriptor, format!("{name} is not an account"))),
    };
    text.parse()
        .map_err(|_| surface(descriptor, format!("{name} is not an account")))
}

/// One gamma deposit, as the runtime reports it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct GammaDeposit {
    /// Subnet of the deposit.
    pub netuid: NetUid,
    /// Hotkey the deposit is held at.
    pub hotkey: AccountId,
    /// Coldkey owning the deposit.
    pub coldkey: AccountId,
    /// Account allowed to spend it.
    pub spender: AccountId,
    /// Balance, as an exact decimal string of gamma.
    pub balance_gamma: String,
    /// Daily spend cap; `None` is no cap and `"0"` pauses the deposit.
    pub daily_spend_cap_gamma: Option<String>,
    /// The balance within what the daily spend cap allows now.
    pub spendable_gamma: String,
}

// py: sdk/python/bittensor/reads/gamma.py:19-30
fn deposit_record(info: &Value) -> Result<GammaDeposit, ReadError> {
    let deposit =
        field(info, "deposit").ok_or_else(|| surface(DEPOSIT_API, "deposit is missing"))?;
    let netuid = u16::try_from(number(info, "netuid", DEPOSIT_API)?)
        .map_err(|_| surface(DEPOSIT_API, "netuid is not a native NetUid"))?;
    let cap = match field(deposit, "daily_spend_cap") {
        None | Some(Value::Null) => None,
        Some(cap) => Some(number(cap, "cap", DEPOSIT_API)?),
    };
    Ok(GammaDeposit {
        netuid: NetUid(netuid),
        hotkey: account(info, "hotkey", DEPOSIT_API)?,
        coldkey: account(deposit, "coldkey", DEPOSIT_API)?,
        spender: account(deposit, "spender", DEPOSIT_API)?,
        balance_gamma: gamma_from_units(number(deposit, "balance", DEPOSIT_API)?),
        daily_spend_cap_gamma: cap.map(gamma_from_units),
        spendable_gamma: gamma_from_units(number(info, "spendable", DEPOSIT_API)?),
    })
}

/// The gamma deposit at a hotkey on a subnet, or `None`.
// py: sdk/python/bittensor/reads/gamma.py:46-63
pub async fn gamma_deposit<V: View + ?Sized>(
    view: &V,
    netuid: NetUid,
    hotkey_ss58: AccountId,
) -> Result<Option<GammaDeposit>, ReadError> {
    let info = view
        .call(
            &api::gamma_runtime_api::GET_GAMMA_DEPOSIT,
            (netuid, hotkey_ss58),
        )
        .await?;
    info.as_ref()
        .map(|info| deposit_record(info.as_core()))
        .transpose()
}

/// Every gamma deposit of a coldkey, on every subnet, in runtime order.
// py: sdk/python/bittensor/reads/gamma.py:66-74
pub async fn gamma_deposits<V: View + ?Sized>(
    view: &V,
    coldkey_ss58: AccountId,
) -> Result<Vec<GammaDeposit>, ReadError> {
    let infos = view
        .call(
            &api::gamma_runtime_api::GET_GAMMA_DEPOSITS_FOR_COLDKEY,
            (coldkey_ss58,),
        )
        .await?;
    infos
        .iter()
        .map(|info| deposit_record(info.as_core()))
        .collect()
}

/// What a mint would burn, its TAO value and the gamma it would mint.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MintQuote {
    /// Subnet to mint on.
    pub netuid: NetUid,
    /// Alpha bought and burned (or burned from stake), in the subnet's alpha.
    pub alpha_burned: DynBalance,
    /// TAO value of the burned alpha.
    pub tao_value: DynBalance,
    /// Gamma minted, as an exact decimal string.
    pub gamma: String,
    /// Gamma minted, in chain units.
    #[serde(skip)]
    pub gamma_units: u128,
}

/// Simulate a mint at the view: `(alpha burned, TAO value, gamma)` in chain
/// units, or the chain error that stops it. `with_stake` selects
/// `sim_mint_gamma_with_stake` (an alpha amount) over `sim_mint_gamma` (TAO).
// py: sdk/python/bittensor/intents/gamma.py:60-65
pub async fn sim_mint<V: View + ?Sized>(
    view: &V,
    netuid: NetUid,
    with_stake: bool,
    amount: u64,
) -> Result<(u64, u64, u128), ReadError> {
    let snapshot = view.pin().await?;
    let quote = if with_stake {
        snapshot
            .call(
                &api::gamma_runtime_api::SIM_MINT_GAMMA_WITH_STAKE,
                (netuid, AlphaBalance(amount)),
            )
            .await?
    } else {
        snapshot
            .call(
                &api::gamma_runtime_api::SIM_MINT_GAMMA,
                (netuid, TaoBalance(amount)),
            )
            .await?
    };
    let quote = mint_quote(&snapshot, &quote, netuid).await?;
    Ok((
        quote.alpha_burned.rao,
        quote.tao_value.rao,
        quote.gamma_units,
    ))
}

// py: sdk/python/bittensor/reads/gamma.py:33-43
async fn mint_quote(
    view: &rtao_client::Snapshot,
    quote: &DynValue,
    netuid: NetUid,
) -> Result<MintQuote, ReadError> {
    let quote = quote.as_core();
    if let Some(error) = field(quote, "Err") {
        let runtime = &view.block().runtime;
        let decoded = rtao_core::ops::dispatch_error::decode(runtime.core(), error)
            .map_err(|error| surface(QUOTE_API, error.to_string()))?;
        let (name, message) = match decoded {
            Some(info) => {
                let message = info
                    .result_message()
                    .map_err(|error| surface(QUOTE_API, error.to_string()))?;
                (Some(info.name), message)
            }
            None => (None, "dispatch error".to_owned()),
        };
        return Err(ReadError::Chain { name, message });
    }
    let parts = match field(quote, "Ok") {
        Some(Value::Tuple(parts) | Value::List(parts)) if parts.len() == 3 => parts,
        _ => return Err(surface(QUOTE_API, "expected Ok((alpha, tao, gamma))")),
    };
    let units = |index: usize, name: &str| {
        uint(&parts[index]).ok_or_else(|| surface(QUOTE_API, format!("{name} is not unsigned")))
    };
    let native = |index: usize, name: &str| {
        u64::try_from(units(index, name)?)
            .map_err(|_| surface(QUOTE_API, format!("{name} is not a native u64")))
    };
    let symbols = view.client().token_symbols().await;
    let mut alpha = DynBalance::from_rao(native(0, "alpha")?, netuid);
    alpha.symbol = symbols.get(&netuid).map(|text| Symbol::new(text.clone()));
    let mut tao = DynBalance::from_rao(native(1, "tao")?, NetUid::ROOT);
    tao.symbol = symbols
        .get(&NetUid::ROOT)
        .map(|text| Symbol::new(text.clone()));
    Ok(MintQuote {
        netuid,
        alpha_burned: alpha,
        tao_value: tao,
        gamma: gamma_from_units(units(2, "gamma")?),
        gamma_units: units(2, "gamma")?,
    })
}

/// The alpha burned, its TAO value and the gamma `mint_gamma` would mint now.
///
/// Fails with the chain error that stops the mint, such as
/// `TaoUsdPriceUnavailable` when the TAO/USD price is missing or stale, or
/// `AmountTooLow` below 0.1 gamma.
// py: sdk/python/bittensor/reads/gamma.py:77-94
pub async fn quote_mint_gamma<V: View + ?Sized>(
    view: &V,
    netuid: NetUid,
    amount_tao: f64,
) -> Result<MintQuote, ReadError> {
    let rao = crate::reads::m6_staking::quote_rao(amount_tao, "amount_tao")?;
    let snapshot = view.pin().await?;
    let quote = snapshot
        .call(
            &api::gamma_runtime_api::SIM_MINT_GAMMA,
            (netuid, TaoBalance(rao)),
        )
        .await?;
    mint_quote(&snapshot, &quote, netuid).await
}

/// The alpha burned, its TAO value and the gamma `mint_gamma_with_stake`
/// would mint now; errors as [`quote_mint_gamma`].
// py: sdk/python/bittensor/reads/gamma.py:97-111
pub async fn quote_mint_gamma_with_stake<V: View + ?Sized>(
    view: &V,
    netuid: NetUid,
    amount_alpha: f64,
) -> Result<MintQuote, ReadError> {
    let rao = crate::reads::m6_staking::quote_rao(amount_alpha, "amount_alpha")?;
    let snapshot = view.pin().await?;
    let quote = snapshot
        .call(
            &api::gamma_runtime_api::SIM_MINT_GAMMA_WITH_STAKE,
            (netuid, AlphaBalance(rao)),
        )
        .await?;
    mint_quote(&snapshot, &quote, netuid).await
}

/// The TAO/USD oracle, its last feed and the accepted price at the block.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TaoUsdPrice {
    /// Account allowed to feed the price, if any.
    pub oracle: Option<AccountId>,
    /// The price mints use at this block, in USD.
    pub accepted_usd: Option<f64>,
    /// The last fed price, in USD.
    pub feed_usd: Option<f64>,
    /// Block of the last feed.
    pub fed_at: Option<u64>,
    /// Blocks since the last feed; mints need at most 300.
    pub blocks_since_last_feed: Option<i128>,
}

// py: sdk/python/bittensor/reads/gamma.py:134-138
/// The accepted price `blocks` after its feed: raised by
/// [`TAO_USD_PRICE_MAX_STEP`] of itself, rounded up, per block, up to the
/// feed, as the chain's `tao_usd_price_at`.
pub fn accepted_price(accepted: u128, feed: u128, blocks: i128) -> i128 {
    let step = (accepted * TAO_USD_PRICE_MAX_STEP).div_ceil(1_000_000_000);
    let step = i128::try_from(step).unwrap_or(i128::MAX);
    let accepted = i128::try_from(accepted).unwrap_or(i128::MAX);
    let feed = i128::try_from(feed).unwrap_or(i128::MAX);
    feed.min(accepted.saturating_add(step.saturating_mul(blocks)))
}

/// The TAO/USD price oracle, its last feed and the accepted price.
///
/// The accepted price falls to the feed at once and rises toward it by at most
/// 0.0066% (66 ppm) of itself per block; mints value TAO at it. Minting needs
/// a feed at most 300 blocks old, so no new mint succeeds once
/// `blocks_since_last_feed` exceeds 300.
// py: sdk/python/bittensor/reads/gamma.py:114-131
pub async fn tao_usd_price<V: View + ?Sized>(view: &V) -> Result<TaoUsdPrice, ReadError> {
    let snapshot = view.pin().await?;
    let (oracle, state) = snapshot
        .fetch((
            st::subtensor_module::TAO_USD_PRICE_ORACLE.at(()),
            st::subtensor_module::TAO_USD_PRICE.at(()),
        ))
        .await?;
    let Some(state) = state else {
        return Ok(TaoUsdPrice {
            oracle,
            accepted_usd: None,
            feed_usd: None,
            fed_at: None,
            blocks_since_last_feed: None,
        });
    };
    let state = state.as_core();
    let fed_at = number(state, "fed_at", PRICE_STORAGE)?;
    let feed = number(state, "feed", PRICE_STORAGE)?;
    let accepted = number(state, "accepted", PRICE_STORAGE)?;
    let fed_at = u64::try_from(fed_at).map_err(|_| surface(PRICE_STORAGE, "fed_at overflows"))?;
    let blocks = i128::from(snapshot.block().number) - i128::from(fed_at);
    let usd = |value: i128| value as f64 / 1e9;
    Ok(TaoUsdPrice {
        oracle,
        accepted_usd: Some(usd(accepted_price(accepted, feed, blocks))),
        feed_usd: Some(usd(i128::try_from(feed).unwrap_or(i128::MAX))),
        fed_at: Some(fed_at),
        blocks_since_last_feed: Some(blocks),
    })
}

/// One published service.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Service {
    /// The url, or 0x-hex when it is not printable text.
    pub url: String,
    /// Spender customers name on gamma deposits for the service.
    pub spender: AccountId,
    /// Whether this is the subnet owner's hotkey while the owner owns it.
    pub owner: bool,
}

const SERVICES_STORAGE: &str = "SubtensorModule.Services";

// py: sdk/python/bittensor/reads/subnets.py:591-594
/// A url as text, or as hex when it holds characters a terminal would act on.
pub fn url_text(raw: &str) -> String {
    if rtao_primitives::pyfmt::is_printable(raw) {
        raw.to_owned()
    } else {
        format!("0x{}", hex::encode(raw.as_bytes()))
    }
}

/// Every service published on a subnet, by hotkey.
///
/// Each `url` points to its hotkey's own document of its services and the
/// payments they accept; the chain does not check what it says. `owner`
/// marks the subnet owner's hotkey while the owner owns it.
// py: sdk/python/bittensor/reads/subnets.py:558-588
pub async fn services<V: View + ?Sized>(
    view: &V,
    netuid: NetUid,
) -> Result<BTreeMap<AccountId, Service>, ReadError> {
    use futures::TryStreamExt;
    let snapshot = view.pin().await?;
    let rows: Vec<_> = snapshot
        .storage_iter(&st::subtensor_module::SERVICES, (netuid,))
        .try_collect()
        .await?;
    let (owner_hotkey, owner_coldkey) = snapshot
        .fetch((
            st::subtensor_module::SUBNET_OWNER_HOTKEY.at((netuid,)),
            st::subtensor_module::SUBNET_OWNER.at((netuid,)),
        ))
        .await?;
    let owner_hotkey = *owner_hotkey.raw();
    let hotkey_owner = snapshot
        .storage(&st::subtensor_module::OWNER, (owner_hotkey,))
        .await?;
    // Python compares the stored accounts, defaults included.
    let owner = (hotkey_owner.raw() == owner_coldkey.raw()).then_some(owner_hotkey);
    let mut services = BTreeMap::new();
    for (keys, info) in rows {
        let [hotkey] = keys.as_slice() else {
            return Err(surface(SERVICES_STORAGE, "expected one hotkey key"));
        };
        let hotkey: AccountId = match hotkey.as_core() {
            Value::Str(text) => text
                .parse()
                .map_err(|_| surface(SERVICES_STORAGE, "hotkey is not an account"))?,
            _ => return Err(surface(SERVICES_STORAGE, "hotkey is not an account")),
        };
        let info = info.as_core();
        let url = match field(info, "url") {
            Some(Value::Str(url)) => url_text(url),
            Some(Value::Bytes(bytes)) => format!("0x{}", hex::encode(bytes)),
            _ => return Err(surface(SERVICES_STORAGE, "url is not text")),
        };
        services.insert(
            hotkey,
            Service {
                url,
                spender: account(info, "spender", SERVICES_STORAGE)?,
                owner: Some(hotkey) == owner,
            },
        );
    }
    Ok(services)
}
