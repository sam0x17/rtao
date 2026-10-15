// Derived from RaoFoundation/subtensor sdk/python/bittensor/reads/{accounts,chain,delegation,epochs,hyperparameters,identity,locks,root,staking,subnets,prices}.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/reads/chain.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/reads/epochs.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/reads/hyperparameters.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Owner-loop catalog reads, with one immutable state identity per logical read.

use std::collections::{BTreeMap, BTreeSet};

use futures::TryStreamExt;
use rtao_client::{ReadPlan, View, ViewExt};
use rtao_core::{DynValue, codec::Value, value_ext::ValueExt};
use rtao_primitives::{AccountId, DynBalance, NetUid, Symbol};
use rtao_runtime::{models, runtime_apis as api, storage as st};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value as JsonValue;

use crate::ReadError;

type Symbols = BTreeMap<NetUid, String>;

fn surface(descriptor: &'static str, detail: impl Into<String>) -> ReadError {
    ReadError::SurfaceChanged {
        descriptor,
        detail: detail.into(),
    }
}

pub(crate) fn contextual_balance(rao: u64, netuid: NetUid, symbols: &Symbols) -> DynBalance {
    let mut amount = DynBalance::from_rao(rao, netuid);
    amount.symbol = symbols
        .get(&netuid)
        .map(|symbol| Symbol::new(symbol.clone()));
    amount
}

fn free_balance(account: DynValue) -> Result<DynBalance, ReadError> {
    let free = account
        .as_core()
        .field("data")
        .and_then(|data| data.field("free"))
        .and_then(rtao_core::value_ext::as_u128)
        .and_then(|value| u64::try_from(value).ok())
        .ok_or_else(|| surface("System.Account", "data.free is not a native u64 balance"))?;
    Ok(DynBalance::from_rao(free, NetUid::ROOT))
}

/// Free TAO balance of one coldkey.
// py: sdk/python/bittensor/reads/accounts.py:20-23
pub async fn balance<V: View + ?Sized>(
    view: &V,
    coldkey_ss58: AccountId,
) -> Result<DynBalance, ReadError> {
    free_balance(view.storage(&st::system::ACCOUNT, (coldkey_ss58,)).await?)
}

/// Free TAO balances, fetched together, with duplicate coldkeys collapsed.
// py: sdk/python/bittensor/reads/accounts.py:32-39
pub async fn balances<V: View + ?Sized>(
    view: &V,
    coldkey_ss58s: Vec<AccountId>,
) -> Result<BTreeMap<AccountId, DynBalance>, ReadError> {
    let mut plan = ReadPlan::new();
    for address in &coldkey_ss58s {
        plan.push(
            st::system::ACCOUNT.surface(),
            vec![Value::Str(address.to_string())],
        );
    }
    coldkey_ss58s
        .into_iter()
        .zip(view.fetch(plan).await?)
        .map(|(address, account)| {
            let amount = if matches!(account.as_core(), Value::Null) {
                DynBalance::from_rao(0, NetUid::ROOT)
            } else {
                free_balance(account)?
            };
            Ok((address, amount))
        })
        .collect()
}

/// Proxy delegations in chain order and the reserved TAO deposit.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ProxyDelegations {
    /// Accounts allowed to act for the queried account, in runtime order.
    pub proxies: Vec<models::ProxyDefinition>,
    /// Reserved proxy deposit in native rao, denominated in TAO.
    pub deposit: DynBalance,
}

/// Proxy delegations of one account, using the metadata's empty default.
// py: sdk/python/bittensor/reads/accounts.py:56-70
pub async fn proxies<V: View + ?Sized>(
    view: &V,
    coldkey_ss58: AccountId,
) -> Result<ProxyDelegations, ReadError> {
    let (proxies, deposit) = view.storage(&st::proxy::PROXIES, (coldkey_ss58,)).await?;
    Ok(ProxyDelegations {
        proxies,
        deposit: DynBalance::from_rao(deposit.0, NetUid::ROOT),
    })
}

/// Next MEV-shield public key as 0x hex; absent or empty keys yield `None`.
// py: sdk/python/bittensor/reads/chain.py:63-67
pub async fn mev_shield_next_key<V: View + ?Sized>(view: &V) -> Result<Option<String>, ReadError> {
    Ok(view
        .storage(&st::mev_shield::NEXT_KEY, ())
        .await?
        .filter(|key| !key.is_empty())
        .map(|key| format!("0x{}", hex::encode(key))))
}

/// A delegate's take and its permitted bounds, all as fractions.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DelegateTake {
    /// Queried hotkey.
    pub hotkey: AccountId,
    /// Fraction of emissions kept by the delegate.
    pub take: f64,
    /// Exact native ratio bits.
    pub take_u16: u16,
    /// Minimum permitted fraction.
    pub min: f64,
    /// Maximum permitted fraction.
    pub max: f64,
}

fn take_fraction(raw: u16, ident: &str) -> f64 {
    rtao_primitives::ratio::ratio_fraction(ident, i128::from(raw))
        .unwrap_or_else(|| f64::from(raw) / f64::from(u16::MAX))
}

/// Delegate take and chain bounds at one block (D-002).
// py: sdk/python/bittensor/reads/delegation.py:104-121
pub async fn delegate_take<V: View + ?Sized>(
    view: &V,
    hotkey_ss58: AccountId,
) -> Result<DelegateTake, ReadError> {
    let (raw, lo, hi) = view
        .fetch((
            st::subtensor_module::DELEGATES.at((hotkey_ss58,)),
            st::subtensor_module::MIN_DELEGATE_TAKE.at(()),
            st::subtensor_module::MAX_DELEGATE_TAKE.at(()),
        ))
        .await?;
    Ok(DelegateTake {
        hotkey: hotkey_ss58,
        take: take_fraction(
            raw.0,
            st::subtensor_module::DELEGATES.surface().value_type_ident,
        ),
        take_u16: raw.0,
        min: take_fraction(
            lo.0,
            st::subtensor_module::MIN_DELEGATE_TAKE
                .surface()
                .value_type_ident,
        ),
        max: take_fraction(
            hi.0,
            st::subtensor_module::MAX_DELEGATE_TAKE
                .surface()
                .value_type_ident,
        ),
    })
}

/// Python's delegate presentation, retaining registrations in chain order.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DelegateInfo {
    /// Delegate hotkey.
    pub hotkey: AccountId,
    /// Owning coldkey.
    pub owner: AccountId,
    /// Fraction of emissions retained.
    pub take: f64,
    /// Number of nominating coldkeys.
    pub nominators: usize,
    /// Registered subnets in runtime order.
    pub registrations: Vec<NetUid>,
    /// Validator permits in runtime order.
    pub validator_permits: Vec<NetUid>,
    /// Return per thousand, in TAO.
    pub return_per_1000: DynBalance,
    /// Total daily TAO return.
    pub total_daily_return: DynBalance,
}

pub(crate) fn delegate_info(info: models::DelegateInfo) -> DelegateInfo {
    DelegateInfo {
        hotkey: info.delegate_ss58,
        owner: info.owner_ss58,
        take: take_fraction(info.take.0, "PerU16"),
        nominators: info.nominators.len(),
        registrations: info.registrations,
        validator_permits: info.validator_permits,
        return_per_1000: DynBalance::from_rao(info.return_per_1000, NetUid::ROOT),
        total_daily_return: DynBalance::from_rao(info.total_daily_return, NetUid::ROOT),
    }
}

/// One delegated stake position, denominated in its subnet's currency.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DelegatedStake {
    /// Delegate information.
    pub delegate: DelegateInfo,
    /// Subnet of this position.
    pub netuid: NetUid,
    /// Alpha, or TAO on root.
    pub stake: DynBalance,
}

/// Every nomination held by a coldkey, in runtime order.
// py: sdk/python/bittensor/reads/delegation.py:130-146
pub async fn delegated<V: View + ?Sized>(
    view: &V,
    coldkey_ss58: AccountId,
) -> Result<Vec<DelegatedStake>, ReadError> {
    let snapshot = view.pin().await?;
    let records = snapshot
        .call(
            &api::delegate_info_runtime_api::GET_DELEGATED,
            (coldkey_ss58,),
        )
        .await?;
    let symbols = snapshot.client().token_symbols().await;
    Ok(records
        .into_iter()
        .map(|(info, (netuid, stake))| DelegatedStake {
            delegate: delegate_info(info),
            netuid,
            stake: contextual_balance(stake.0, netuid, &symbols),
        })
        .collect())
}

/// Chain-computed next epoch start, absent when tempo is zero.
// py: sdk/python/bittensor/reads/epochs.py:19-28
pub async fn next_epoch_start_block<V: View + ?Sized>(
    view: &V,
    netuid: NetUid,
) -> Result<Option<u64>, ReadError> {
    Ok(view
        .call(
            &api::subnet_info_runtime_api::GET_NEXT_EPOCH_START_BLOCK,
            (netuid,),
        )
        .await?)
}

/// Remaining blocks until the next epoch at one pinned head, clamped at zero.
// py: sdk/python/bittensor/reads/epochs.py:37-49
pub async fn blocks_until_next_epoch<V: View + ?Sized>(
    view: &V,
    netuid: NetUid,
) -> Result<Option<u64>, ReadError> {
    let snapshot = view.pin().await?;
    Ok(next_epoch_start_block(&snapshot, netuid)
        .await?
        .map(|next| next.saturating_sub(snapshot.block().number)))
}

/// Native PoW registration difficulty.
pub async fn difficulty<V: View + ?Sized>(view: &V, netuid: NetUid) -> Result<u64, ReadError> {
    Ok(view
        .storage(&st::subtensor_module::DIFFICULTY, (netuid,))
        .await?)
}

/// Pool-side emission flag as Python's scalar integer `0` or `1`.
pub async fn subnet_emission_enabled<V: View + ?Sized>(
    view: &V,
    netuid: NetUid,
) -> Result<u8, ReadError> {
    Ok(u8::from(
        view.storage(&st::subtensor_module::SUBNET_EMISSION_ENABLED, (netuid,))
            .await?,
    ))
}

fn normalize_identity<T: models::ToDynamic>(record: T) -> Result<JsonValue, ReadError> {
    let mut record = DynValue::new(record.to_dynamic()).to_json();
    let fields = record
        .as_object_mut()
        .ok_or_else(|| surface("identity", "identity is not a record"))?;
    for value in fields.values_mut() {
        *value = super::subnets::utf8_text(value.take())?;
    }
    Ok(record)
}

/// Coldkey identity with UTF-8 fields, or 0x hex for invalid bytes.
// py: sdk/python/bittensor/reads/identity.py:31-34
pub async fn identity<V: View + ?Sized>(
    view: &V,
    coldkey_ss58: AccountId,
) -> Result<Option<JsonValue>, ReadError> {
    view.storage(&st::subtensor_module::IDENTITIES_V2, (coldkey_ss58,))
        .await?
        .map(normalize_identity)
        .transpose()
}

/// Deduplicated hotkey identities resolved through owner coldkeys at one block (D-002).
// py: sdk/python/bittensor/reads/identity.py:43-69
pub async fn hotkey_identities<V: View + ?Sized>(
    view: &V,
    hotkey_ss58s: Vec<AccountId>,
) -> Result<BTreeMap<AccountId, JsonValue>, ReadError> {
    if hotkey_ss58s.is_empty() {
        return Ok(BTreeMap::new());
    }
    let snapshot = view.pin().await?;
    let mut hotkeys = Vec::new();
    let mut seen_hotkeys = BTreeSet::new();
    for hotkey in hotkey_ss58s {
        if seen_hotkeys.insert(hotkey) {
            hotkeys.push(hotkey);
        }
    }
    let mut plan = ReadPlan::new();
    for hotkey in &hotkeys {
        plan.push(
            st::subtensor_module::OWNER.surface(),
            vec![Value::Str(hotkey.to_string())],
        );
    }
    let owners = snapshot.fetch(plan).await?;
    let mut owner_of = Vec::new();
    let mut coldkeys = Vec::new();
    let mut seen_coldkeys = BTreeSet::new();
    for (hotkey, owner) in hotkeys.into_iter().zip(owners) {
        let owner = owner
            .as_str()
            .ok_or_else(|| surface("SubtensorModule.Owner", "owner is not an address"))?;
        let owner = owner
            .parse::<AccountId>()
            .map_err(|error| surface("SubtensorModule.Owner", error.to_string()))?;
        owner_of.push((hotkey, owner));
        if seen_coldkeys.insert(owner) {
            coldkeys.push(owner);
        }
    }
    let mut plan = ReadPlan::new();
    for coldkey in &coldkeys {
        plan.push(
            st::subtensor_module::IDENTITIES_V2.surface(),
            vec![Value::Str(coldkey.to_string())],
        );
    }
    let mut identity_of = BTreeMap::new();
    for (coldkey, value) in coldkeys.into_iter().zip(snapshot.fetch(plan).await?) {
        let mut value = value.to_json();
        if value.is_null() {
            continue;
        }
        let fields = value
            .as_object_mut()
            .ok_or_else(|| surface("SubtensorModule.IdentitiesV2", "identity is not a record"))?;
        for value in fields.values_mut() {
            *value = super::subnets::utf8_text(value.take())?;
        }
        identity_of.insert(coldkey, value);
    }
    Ok(owner_of
        .into_iter()
        .filter_map(|(hotkey, coldkey)| {
            identity_of
                .get(&coldkey)
                .cloned()
                .map(|identity| (hotkey, identity))
        })
        .collect())
}

/// Most convicted hotkey on a subnet, if present.
// py: sdk/python/bittensor/reads/locks.py:163-170
pub async fn most_convicted_hotkey<V: View + ?Sized>(
    view: &V,
    netuid: NetUid,
) -> Result<Option<AccountId>, ReadError> {
    Ok(view
        .call(
            &api::stake_info_runtime_api::GET_MOST_CONVICTED_HOTKEY_ON_SUBNET,
            (netuid,),
        )
        .await?)
}

/// Pending root dividends valued in TAO by the chain.
// py: sdk/python/bittensor/reads/root.py:172-185
pub async fn root_basket_owed<V: View + ?Sized>(
    view: &V,
    coldkey_ss58: AccountId,
) -> Result<DynBalance, ReadError> {
    let snapshot = view.pin().await?;
    // Python names this method dynamically: it existed in API v1, while the
    // generated descriptor carries the whole pinned API's v5 version.
    let value = snapshot
        .call_dyn(
            "BetaBasketRuntimeApi",
            "get_root_basket_owed",
            &[Value::Str(coldkey_ss58.to_string())],
        )
        .await?;
    let rao = value
        .as_u128()
        .and_then(|value| u64::try_from(value).ok())
        .ok_or_else(|| {
            surface(
                "get_root_basket_owed",
                "owed TAO is not a native u64 balance",
            )
        })?;
    Ok(contextual_balance(
        rao,
        NetUid::ROOT,
        &snapshot.client().token_symbols().await,
    ))
}

/// Minimum TAO payout for a root dividend claim, discarding fractional rao.
// py: sdk/python/bittensor/reads/root.py:442-451, _i96f32_rao:54-59
pub async fn root_claim_threshold<V: View + ?Sized>(view: &V) -> Result<DynBalance, ReadError> {
    let snapshot = view.pin().await?;
    let threshold = snapshot
        .storage(
            &st::subtensor_module::ROOT_CLAIMABLE_THRESHOLD,
            (NetUid::ROOT,),
        )
        .await?;
    let rao = u64::try_from(threshold.int_part()).map_err(|_| {
        surface(
            "SubtensorModule.RootClaimableThreshold",
            "claim threshold is not a native u64 balance",
        )
    })?;
    Ok(contextual_balance(
        rao,
        NetUid::ROOT,
        &snapshot.client().token_symbols().await,
    ))
}

/// One chain stake position, with explicit currency tagging.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct StakePosition {
    /// Staking hotkey.
    pub hotkey: AccountId,
    /// Owning coldkey.
    pub coldkey: AccountId,
    /// Subnet whose currency the position holds.
    pub netuid: NetUid,
    /// Exact alpha amount, or TAO on root.
    pub stake: DynBalance,
    /// Whether the hotkey is currently registered.
    pub is_registered: bool,
}

/// A coldkey's stake valued at spot prices at one immutable block.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct StakeValuation {
    /// Queried coldkey.
    pub coldkey: AccountId,
    /// Pinned block number.
    pub block: u64,
    /// Positions in runtime order.
    pub positions: Vec<StakePosition>,
    /// TAO spot estimate, excluding actual unstake fees and slippage.
    pub stake_value: DynBalance,
    /// Spot TAO per alpha for each reported subnet.
    pub tao_per_alpha: BTreeMap<NetUid, f64>,
}

impl StakeValuation {
    /// Spot TAO value of one stake balance, with root amounts kept exact.
    ///
    /// Missing subnet prices yield zero. Alpha values use Python's binary64
    /// product followed by truncation, and reject results outside native u64 rao.
    // py: sdk/python/bittensor/reads/staking.py:48-52
    pub fn spot_value(&self, stake: &DynBalance) -> Result<DynBalance, ReadError> {
        let rao = if stake.netuid == NetUid::ROOT {
            stake.rao
        } else {
            spot_rao(
                stake.rao,
                self.tao_per_alpha
                    .get(&stake.netuid)
                    .copied()
                    .unwrap_or(0.0),
            )?
        };
        Ok(DynBalance::from_rao(rao, NetUid::ROOT))
    }
}

fn spot_rao(rao: u64, price: f64) -> Result<u64, ReadError> {
    let value = (rao as f64 * price).trunc();
    // A float at 2^64 is already out of range; casting it would saturate.
    if !(0.0..18_446_744_073_709_551_616.0).contains(&value) {
        return Err(ReadError::InvalidParam {
            name: "stake_value".into(),
            reason: "spot valuation exceeds native u64 rao".into(),
        });
    }
    Ok(value as u64)
}

fn valuation(
    coldkey: AccountId,
    block: u64,
    records: Vec<models::StakeInfo>,
    prices: &BTreeMap<NetUid, f64>,
    symbols: &Symbols,
) -> Result<StakeValuation, ReadError> {
    let mut value_rao = 0u64;
    let mut positions = Vec::with_capacity(records.len());
    for record in records {
        let rao = if record.netuid == NetUid::ROOT {
            record.stake.0
        } else {
            spot_rao(
                record.stake.0,
                prices.get(&record.netuid).copied().unwrap_or(0.0),
            )?
        };
        value_rao = value_rao
            .checked_add(rao)
            .ok_or_else(|| ReadError::InvalidParam {
                name: "stake_value".into(),
                reason: "sum of spot valuations exceeds native u64 rao".into(),
            })?;
        positions.push(StakePosition {
            hotkey: record.hotkey,
            coldkey: record.coldkey,
            netuid: record.netuid,
            stake: contextual_balance(record.stake.0, record.netuid, symbols),
            is_registered: record.is_registered,
        });
    }
    Ok(StakeValuation {
        coldkey,
        block,
        positions,
        stake_value: DynBalance::from_rao(value_rao, NetUid::ROOT),
        tao_per_alpha: prices.clone(),
    })
}

fn prices(records: Vec<models::SubnetPrice>) -> BTreeMap<NetUid, f64> {
    records
        .into_iter()
        .map(|record| {
            (
                record.netuid,
                rtao_primitives::pyfmt::rao_to_f64(record.price.0),
            )
        })
        .collect()
}

/// One coldkey's spot valuation, fetching positions and prices concurrently at one block.
// py: sdk/python/bittensor/reads/staking.py:139-153
pub async fn stake_value_for_coldkey<V: View + ?Sized>(
    view: &V,
    coldkey_ss58: AccountId,
) -> Result<StakeValuation, ReadError> {
    let snapshot = view.pin().await?;
    let (records, quotes) = tokio::try_join!(
        snapshot.call(
            &api::stake_info_runtime_api::GET_STAKE_INFO_FOR_COLDKEY,
            (coldkey_ss58,)
        ),
        snapshot.call(&api::swap_runtime_api::CURRENT_ALPHA_PRICE_ALL, ())
    )?;
    valuation(
        coldkey_ss58,
        snapshot.block().number,
        records,
        &prices(quotes),
        &snapshot.client().token_symbols().await,
    )
}

/// Several coldkeys' spot valuations, including empty rows omitted by the runtime.
// py: sdk/python/bittensor/reads/staking.py:162-186
pub async fn stake_value_for_coldkeys<V: View + ?Sized>(
    view: &V,
    coldkey_ss58s: Vec<AccountId>,
) -> Result<BTreeMap<AccountId, StakeValuation>, ReadError> {
    let snapshot = view.pin().await?;
    let (records, quotes) = tokio::try_join!(
        snapshot.call(
            &api::stake_info_runtime_api::GET_STAKE_INFO_FOR_COLDKEYS,
            (coldkey_ss58s.clone(),)
        ),
        snapshot.call(&api::swap_runtime_api::CURRENT_ALPHA_PRICE_ALL, ())
    )?;
    let by_coldkey: BTreeMap<_, _> = records.into_iter().collect();
    let prices = prices(quotes);
    let symbols = snapshot.client().token_symbols().await;
    let mut out = BTreeMap::new();
    // Duplicate input keys overwrite the same value, as a Python dict comprehension does.
    for coldkey in coldkey_ss58s {
        let positions = by_coldkey.get(&coldkey).cloned().unwrap_or_default();
        out.insert(
            coldkey,
            valuation(
                coldkey,
                snapshot.block().number,
                positions,
                &prices,
                &symbols,
            )?,
        );
    }
    Ok(out)
}

/// One basket holding with its own alpha currency and two TAO valuations.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BasketHolding {
    /// Subnet of this holding.
    pub netuid: NetUid,
    /// Exact alpha held.
    pub alpha: DynBalance,
    /// Spot TAO estimate.
    pub spot_tao: DynBalance,
    /// Slippage-aware TAO realization estimate.
    pub realizable_tao: DynBalance,
}

/// Standardized chain beta pricing, flattened to Python floats.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ChainPricing {
    /// Validator hotkey.
    pub hotkey: AccountId,
    /// Spot beta price.
    pub spot_price: f64,
    /// Display beta price.
    pub display_price: f64,
    /// Stake price.
    pub stake_price: f64,
    /// Staker yield.
    pub staker_yield: f64,
    /// Staker time-weighted return.
    pub staker_twr: f64,
    /// Basket accrual index.
    pub bag_index: f64,
    /// Stake index.
    pub stake_index: f64,
    /// First block in the pricing history.
    pub first_block: u64,
    /// Whether the estimate is provisional.
    pub provisional: bool,
    /// Display beta shares.
    pub display_shares: f64,
}

impl From<models::BetaPricing> for ChainPricing {
    fn from(row: models::BetaPricing) -> Self {
        Self {
            hotkey: row.hotkey,
            spot_price: row.spot_price.to_f64(),
            display_price: row.display_price.to_f64(),
            stake_price: row.stake_price.to_f64(),
            staker_yield: row.staker_yield.to_f64(),
            staker_twr: row.staker_twr.to_f64(),
            bag_index: row.bag_index.to_f64(),
            stake_index: row.stake_index.to_f64(),
            first_block: row.first_block,
            provisional: row.provisional,
            display_shares: row.display_shares.to_f64(),
        }
    }
}

/// Validator basket balances and lifetime performance.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ValidatorBasketSummary {
    /// Validator hotkey.
    pub hotkey: AccountId,
    /// Raw beta shares, independent of the balance amount domain.
    pub beta_total_raw: u64,
    /// Beta tokens at one billion raw units per token.
    pub beta_total: f64,
    /// TAO NAV per beta token.
    pub beta_price_tao: f64,
    /// Realizable TAO NAV.
    pub nav_tao: DynBalance,
    /// Spot TAO NAV.
    pub spot_nav_tao: DynBalance,
    /// Lifetime deposited TAO.
    pub deposited_tao: DynBalance,
    /// Lifetime redeemed TAO.
    pub redeemed_tao: DynBalance,
    /// `(NAV + redeemed) / deposited`, absent when nothing was deposited.
    pub lifetime_return: Option<f64>,
    /// Holdings in chain order.
    pub holdings: Vec<BasketHolding>,
    /// Cumulative raw beta minted per rao of root stake.
    pub basket_rate: f64,
    /// Chain pricing is omitted on older nodes or any failed pricing request, matching Python.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chain_pricing: Option<ChainPricing>,
}

pub(crate) fn basket_summary(
    summary: models::BasketSummary,
    symbols: &Symbols,
) -> ValidatorBasketSummary {
    let nav = summary.nav_tao.0;
    let deposited = summary.deposited_tao.0;
    let redeemed = summary.redeemed_tao.0;
    ValidatorBasketSummary {
        hotkey: summary.hotkey,
        beta_total_raw: summary.shares,
        beta_total: rtao_primitives::pyfmt::rao_to_f64(summary.shares),
        beta_price_tao: rtao_primitives::ratio::integer_division(i128::from(nav), summary.shares)
            .unwrap_or(0.0),
        nav_tao: contextual_balance(nav, NetUid::ROOT, symbols),
        spot_nav_tao: contextual_balance(summary.spot_nav_tao.0, NetUid::ROOT, symbols),
        deposited_tao: contextual_balance(deposited, NetUid::ROOT, symbols),
        redeemed_tao: contextual_balance(redeemed, NetUid::ROOT, symbols),
        lifetime_return: rtao_primitives::ratio::integer_division(
            i128::from(nav) + i128::from(redeemed),
            deposited,
        ),
        holdings: summary
            .holdings
            .into_iter()
            .map(|holding| BasketHolding {
                netuid: holding.netuid,
                alpha: contextual_balance(holding.alpha.0, holding.netuid, symbols),
                spot_tao: contextual_balance(holding.spot_tao.0, NetUid::ROOT, symbols),
                realizable_tao: contextual_balance(holding.realizable_tao.0, NetUid::ROOT, symbols),
            })
            .collect(),
        basket_rate: 0.0,
        chain_pricing: None,
    }
}

fn basket_model<T: DeserializeOwned>(
    value: DynValue,
    descriptor: &'static str,
) -> Result<T, ReadError> {
    // SCALE and archive metadata stay in the core; the existing by-name model
    // performs only the native-domain projection of that decoded record.
    serde_json::from_value(value.to_json()).map_err(|error| surface(descriptor, error.to_string()))
}

/// Basket summary, rate and optional pricing at one immutable block.
// py: sdk/python/bittensor/reads/root.py:319-345
pub async fn validator_basket_summary<V: View + ?Sized>(
    view: &V,
    hotkey_ss58: AccountId,
) -> Result<ValidatorBasketSummary, ReadError> {
    let snapshot = view.pin().await?;
    let summary = basket_model(
        snapshot
            .call_dyn(
                "BetaBasketRuntimeApi",
                "get_validator_basket_summary",
                &[Value::Str(hotkey_ss58.to_string())],
            )
            .await?,
        "get_validator_basket_summary",
    )?;
    let mut record = basket_summary(summary, &snapshot.client().token_symbols().await);
    record.basket_rate = snapshot
        .storage(&st::subtensor_module::BASKET_RATE, (hotkey_ss58,))
        .await?
        .to_f64();
    let pricing = snapshot
        .call_dyn(
            "BetaBasketRuntimeApi",
            "get_beta_pricing",
            &[Value::Str(hotkey_ss58.to_string())],
        )
        .await;
    record.chain_pricing = pricing
        .ok()
        .map(|value| basket_model::<Option<models::BetaPricing>>(value, "get_beta_pricing"))
        .transpose()?
        .flatten()
        .map(ChainPricing::from);
    Ok(record)
}

/// TAO burn/recycle cost of registering on a subnet.
// py: sdk/python/bittensor/reads/subnets.py:79-82
pub async fn burn<V: View + ?Sized>(view: &V, netuid: NetUid) -> Result<DynBalance, ReadError> {
    Ok(DynBalance::from_rao(
        view.storage(&st::subtensor_module::BURN, (netuid,))
            .await?
            .0,
        NetUid::ROOT,
    ))
}

/// Mechanism shares in mechanism order; unset values produce an empty list.
// py: sdk/python/bittensor/reads/subnets.py:272-280
pub async fn mechanism_emission_split<V: View + ?Sized>(
    view: &V,
    netuid: NetUid,
) -> Result<Vec<u16>, ReadError> {
    Ok(view
        .storage(&st::subtensor_module::MECHANISM_EMISSION_SPLIT, (netuid,))
        .await?
        .unwrap_or_default())
}

fn hyperparam_value(tagged: &JsonValue) -> Result<JsonValue, ReadError> {
    let fields = tagged
        .as_object()
        .filter(|fields| fields.len() == 1)
        .ok_or_else(|| {
            surface(
                "get_subnet_hyperparams_v3",
                "hyperparameter must have exactly one value tag",
            )
        })?;
    let (tag, payload) = fields
        .iter()
        .next()
        .ok_or_else(|| surface("get_subnet_hyperparams_v3", "empty hyperparameter value"))?;
    if let Some(bits) = payload
        .as_object()
        .filter(|fields| fields.len() == 1)
        .and_then(|fields| fields.get("bits"))
    {
        if tag == "I32F32" {
            let bits = bits
                .as_i64()
                .ok_or_else(|| surface("get_subnet_hyperparams_v3", "I32F32 bits are not i64"))?;
            return Ok(JsonValue::from(bits >> 32));
        }
        return Ok(bits.clone());
    }
    Ok(payload.clone())
}

/// Forward-compatible name-to-raw-value map, absent for a nonexistent subnet.
// py: sdk/python/bittensor/reads/subnets.py:156-167
pub async fn subnet_hyperparameters<V: View + ?Sized>(
    view: &V,
    netuid: NetUid,
) -> Result<Option<BTreeMap<String, JsonValue>>, ReadError> {
    let entries = view
        .call_dyn(
            "SubnetInfoRuntimeApi",
            "get_subnet_hyperparams_v3",
            &[Value::Uint(u128::from(netuid.0))],
        )
        .await?
        .to_json();
    if entries.is_null() {
        return Ok(None);
    }
    let entries = entries.as_array().ok_or_else(|| {
        surface(
            "get_subnet_hyperparams_v3",
            "hyperparameters are not a list",
        )
    })?;
    entries
        .iter()
        .map(|entry| {
            let name = entry
                .get("name")
                .and_then(JsonValue::as_str)
                .ok_or_else(|| {
                    surface(
                        "get_subnet_hyperparams_v3",
                        "hyperparameter name is not text",
                    )
                })?;
            let value = entry.get("value").ok_or_else(|| {
                surface("get_subnet_hyperparams_v3", "hyperparameter lacks value")
            })?;
            Ok((name.to_owned(), hyperparam_value(value)?))
        })
        .collect::<Result<_, _>>()
        .map(Some)
}

/// Subnet identity text, preserving invalid bytes as 0x hex.
// py: sdk/python/bittensor/reads/subnets.py:199-203
pub async fn subnet_identity<V: View + ?Sized>(
    view: &V,
    netuid: NetUid,
) -> Result<Option<JsonValue>, ReadError> {
    view.storage(&st::subtensor_module::SUBNET_IDENTITIES_V3, (netuid,))
        .await?
        .map(normalize_identity)
        .transpose()
}

/// Registered nonempty subnet names keyed by subnet ID.
// py: sdk/python/bittensor/reads/subnets.py:206-214
pub async fn subnet_names<V: View + ?Sized>(
    view: &V,
) -> Result<BTreeMap<NetUid, String>, ReadError> {
    let snapshot = view.pin().await?;
    let rows: Vec<_> = snapshot
        .storage_iter(&st::subtensor_module::SUBNET_IDENTITIES_V3, ())
        .try_collect()
        .await?;
    let mut names = BTreeMap::new();
    for (keys, identity) in rows {
        let netuid = keys
            .last()
            .and_then(DynValue::as_u128)
            .and_then(|value| u16::try_from(value).ok())
            .ok_or_else(|| {
                surface(
                    "SubtensorModule.SubnetIdentitiesV3",
                    "subnet key is not NetUid",
                )
            })?;
        let record = normalize_identity(identity)?;
        if let Some(name) = record
            .get("subnet_name")
            .and_then(JsonValue::as_str)
            .filter(|name| !name.is_empty())
        {
            names.insert(NetUid(netuid), name.to_owned());
        }
    }
    Ok(names)
}

/// Registered subnet start schedule, using the pinned mutable runtime delay.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SubnetStartSchedule {
    /// Queried subnet.
    pub netuid: NetUid,
    /// Registration block, or zero when absent.
    pub registered_at: u64,
    /// Chain's mutable start-call delay at the pinned block.
    pub start_call_delay: u64,
    /// Runtime's saturating u64 threshold, exposed through the existing u128 field.
    pub earliest_start_block: u128,
    /// Pinned current block.
    pub current_block: u64,
}

/// Earliest owner start-call block and current pinned head.
// py: sdk/python/bittensor/reads/subnets.py:235-253
pub async fn subnet_start_schedule<V: View + ?Sized>(
    view: &V,
    netuid: NetUid,
) -> Result<SubnetStartSchedule, ReadError> {
    let snapshot = view.pin().await?;
    let registered_at = snapshot
        .storage(&st::subtensor_module::NETWORK_REGISTERED_AT, (netuid,))
        .await?;
    // D-040: the runtime uses mutable StartCallDelay, whose default is the initial constant.
    let delay = snapshot
        .storage(&st::subtensor_module::START_CALL_DELAY, ())
        .await?;
    Ok(SubnetStartSchedule {
        netuid,
        registered_at,
        start_call_delay: delay,
        earliest_start_block: u128::from(registered_at.saturating_add(delay)),
        current_block: snapshot.block().number,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;

    fn stake_valuation(tao_per_alpha: BTreeMap<NetUid, f64>) -> StakeValuation {
        StakeValuation {
            coldkey: AccountId::ZERO,
            block: 10,
            positions: Vec::new(),
            stake_value: DynBalance::from_rao(0, NetUid::ROOT),
            tao_per_alpha,
        }
    }

    #[test]
    fn public_spot_value_keeps_root_u64_max_exact_and_defaults_missing_prices_to_zero() {
        let valuation = stake_valuation(BTreeMap::from([(NetUid::ROOT, f64::NAN)]));
        assert_eq!(
            valuation
                .spot_value(&DynBalance::from_rao(u64::MAX, NetUid::ROOT))
                .unwrap(),
            DynBalance::from_rao(u64::MAX, NetUid::ROOT)
        );
        assert_eq!(
            valuation
                .spot_value(&DynBalance::from_rao(u64::MAX, NetUid(999)))
                .unwrap(),
            DynBalance::from_rao(0, NetUid::ROOT)
        );
    }

    #[test]
    fn public_spot_value_uses_binary64_product_truncation_and_returns_tao() {
        let valuation = stake_valuation(BTreeMap::from([(NetUid(1), 0.5), (NetUid(2), 1.0)]));
        assert_eq!(
            valuation
                .spot_value(&DynBalance::from_rao(3, NetUid(1)))
                .unwrap(),
            DynBalance::from_rao(1, NetUid::ROOT)
        );
        assert_eq!(
            valuation
                .spot_value(&DynBalance::from_rao(9_007_199_254_740_993, NetUid(2)))
                .unwrap(),
            DynBalance::from_rao(9_007_199_254_740_992, NetUid::ROOT)
        );
    }

    #[test]
    fn public_spot_value_rejects_nonfinite_negative_and_overflowing_native_results() {
        for (rao, price) in [
            (1, f64::NAN),
            (1, f64::INFINITY),
            (1, f64::NEG_INFINITY),
            (3, -1.0),
            (u64::MAX, 1.0),
            (u64::MAX, 2.0),
        ] {
            let valuation = stake_valuation(BTreeMap::from([(NetUid(1), price)]));
            assert!(matches!(
                valuation.spot_value(&DynBalance::from_rao(rao, NetUid(1))),
                Err(ReadError::InvalidParam { name, reason })
                    if name == "stake_value" && reason == "spot valuation exceeds native u64 rao"
            ));
        }
    }

    #[test]
    fn hyperparameters_preserve_full_bits_and_floor_negative_i32f32() {
        assert_eq!(
            hyperparam_value(&json!({"I32F32": {"bits": -1}})).unwrap(),
            json!(-1)
        );
        assert_eq!(
            hyperparam_value(&json!({"U64F64": {"bits": u128::MAX}})).unwrap(),
            json!(u128::MAX)
        );
        assert_eq!(
            hyperparam_value(&json!({"Future": {"bits": 7, "other": 9}})).unwrap(),
            json!({"bits": 7, "other": 9})
        );
        assert!(hyperparam_value(&json!({"U16": 1, "U64": 2})).is_err());
    }

    #[test]
    fn spot_projection_uses_float_multiply_then_truncation_and_checks_native_limits() {
        assert_eq!(spot_rao(3, 0.5).unwrap(), 1);
        assert_eq!(
            spot_rao(9_007_199_254_740_993, 1.0).unwrap(),
            9_007_199_254_740_992
        );
        assert_eq!(spot_rao(10, 0.0).unwrap(), 0);
        assert!(spot_rao(u64::MAX, 1.0).is_err());
        assert!(spot_rao(1, f64::INFINITY).is_err());
    }
}
