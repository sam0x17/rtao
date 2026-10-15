// Derived from RaoFoundation/subtensor sdk/python/bittensor/reads/{root,subnets,weights}.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Basket accounting, subnet listings and pending mechanism weight commits.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, SecondsFormat, Utc};
use futures::TryStreamExt;
use rtao_client::{ReadPlan, View, ViewExt};
use rtao_core::{DynValue, codec::Value};
use rtao_primitives::{AccountId, DynBalance, MechId, NetUid, NetUidStorageIndex, Uid};
use rtao_runtime::{models, runtime_apis as api, storage::subtensor_module as st};
use serde::{Deserialize, Serialize, Serializer, de::DeserializeOwned};

use crate::{ReadError, reads::owner};

fn surface(descriptor: &'static str, detail: impl Into<String>) -> ReadError {
    ReadError::SurfaceChanged {
        descriptor,
        detail: detail.into(),
    }
}

fn model<T: DeserializeOwned>(value: DynValue, descriptor: &'static str) -> Result<T, ReadError> {
    serde_json::from_value(value.to_json()).map_err(|error| surface(descriptor, error.to_string()))
}

fn native_key(keys: &[DynValue], descriptor: &'static str) -> Result<u64, ReadError> {
    keys.last()
        .and_then(DynValue::as_u128)
        .and_then(|key| u64::try_from(key).ok())
        .ok_or_else(|| surface(descriptor, "map key is not a native unsigned integer"))
}

fn subnet_key(keys: &[DynValue], descriptor: &'static str) -> Result<NetUid, ReadError> {
    u16::try_from(native_key(keys, descriptor)?)
        .map(NetUid)
        .map_err(|error| surface(descriptor, error.to_string()))
}

/// A staker's raw beta entitlement and its chain-computed TAO values.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BasketPosition {
    /// Validator whose fund holds this position.
    pub hotkey: AccountId,
    /// Exact beta balance in native raw units.
    pub beta_raw: u64,
    /// Exact outstanding beta supply in native raw units.
    pub beta_total_raw: u64,
    /// Beta tokens at one billion raw units per token.
    pub beta: f64,
    /// Fraction of the outstanding supply, or zero when supply is zero.
    pub fund_fraction: f64,
    /// Realizable fund NAV per beta token, or zero when supply is zero.
    pub beta_price_tao: f64,
    /// Fund's realizable TAO NAV.
    pub nav_tao: DynBalance,
    /// TAO a claim would realize at current pool depth.
    pub value_tao: DynBalance,
    /// Display-only TAO spot valuation.
    pub spot_value_tao: DynBalance,
}

fn position(row: models::BasketPosition, symbols: &BTreeMap<NetUid, String>) -> BasketPosition {
    BasketPosition {
        hotkey: row.hotkey,
        beta_raw: row.beta,
        beta_total_raw: row.beta_total,
        beta: rtao_primitives::pyfmt::rao_to_f64(row.beta),
        fund_fraction: rtao_primitives::ratio::integer_division(
            i128::from(row.beta),
            row.beta_total,
        )
        .unwrap_or(0.0),
        beta_price_tao: rtao_primitives::ratio::integer_division(
            i128::from(row.nav_tao.0),
            row.beta_total,
        )
        .unwrap_or(0.0),
        nav_tao: owner::contextual_balance(row.nav_tao.0, NetUid::ROOT, symbols),
        value_tao: owner::contextual_balance(row.value_tao.0, NetUid::ROOT, symbols),
        spot_value_tao: owner::contextual_balance(row.spot_value_tao.0, NetUid::ROOT, symbols),
    }
}

/// A staker's beta position in one validator's fund; absent when no position exists.
// py: sdk/python/bittensor/reads/root.py:212-234,246-264
pub async fn basket_position<V: View + ?Sized>(
    view: &V,
    hotkey_ss58: AccountId,
    coldkey_ss58: AccountId,
) -> Result<Option<BasketPosition>, ReadError> {
    let snapshot = view.pin().await?;
    let row: Option<models::BasketPosition> = model(
        snapshot
            .call_dyn(
                "BetaBasketRuntimeApi",
                "get_basket_position",
                &[
                    Value::Str(hotkey_ss58.to_string()),
                    Value::Str(coldkey_ss58.to_string()),
                ],
            )
            .await?,
        "get_basket_position",
    )?;
    let symbols = snapshot.client().token_symbols().await;
    Ok(row.map(|row| position(row, &symbols)))
}

/// Every held beta position, sorted stably by realizable TAO value descending.
// py: sdk/python/bittensor/reads/root.py:273-286
pub async fn root_basket_portfolio<V: View + ?Sized>(
    view: &V,
    coldkey_ss58: AccountId,
) -> Result<Vec<BasketPosition>, ReadError> {
    let snapshot = view.pin().await?;
    let rows: Vec<models::BasketPosition> = model(
        snapshot
            .call_dyn(
                "BetaBasketRuntimeApi",
                "get_root_basket_portfolio",
                &[Value::Str(coldkey_ss58.to_string())],
            )
            .await?,
        "get_root_basket_portfolio",
    )?;
    let symbols = snapshot.client().token_symbols().await;
    let mut records: Vec<_> = rows
        .into_iter()
        .map(|row| position(row, &symbols))
        .collect();
    records.sort_by_key(|record| std::cmp::Reverse(record.value_tao.rao));
    Ok(records)
}

/// One validator's pending root dividend payout for the queried coldkey.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BasketOwed {
    /// Validator hotkey.
    pub hotkey: AccountId,
    /// Realizable pending TAO, including values below the claim threshold.
    pub owed_tao: DynBalance,
}

/// Root dividends by validator, sorted stably by payout descending.
///
/// The runtime determines which positions are omitted; this projection keeps
/// every returned row, as Python does, including a zero if the runtime returns one.
// py: sdk/python/bittensor/reads/root.py:193-209
pub async fn root_basket_owed_breakdown<V: View + ?Sized>(
    view: &V,
    coldkey_ss58: AccountId,
) -> Result<Vec<BasketOwed>, ReadError> {
    let snapshot = view.pin().await?;
    let rows: Vec<(AccountId, u64, u64)> = model(
        snapshot
            .call_dyn(
                "BetaBasketRuntimeApi",
                "get_root_basket_positions",
                &[Value::Str(coldkey_ss58.to_string())],
            )
            .await?,
        "get_root_basket_positions",
    )?;
    let symbols = snapshot.client().token_symbols().await;
    let mut records: Vec<_> = rows
        .into_iter()
        .map(|(hotkey, _, payout)| BasketOwed {
            hotkey,
            owed_tao: owner::contextual_balance(payout, NetUid::ROOT, &symbols),
        })
        .collect();
    records.sort_by_key(|record| std::cmp::Reverse(record.owed_tao.rao));
    Ok(records)
}

/// One validator basket holding with the subnet's own currency and TAO quote.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ValidatorBasketHolding {
    /// Subnet holding the alpha, or root for TAO cash.
    pub netuid: NetUid,
    /// Native alpha amount, or TAO when `netuid` is root.
    pub alpha: DynBalance,
    /// Realizable TAO value, rather than a spot mark.
    pub value_tao: DynBalance,
}

/// Validator basket holdings in runtime order, retaining root's cash slot.
// py: sdk/python/bittensor/reads/root.py:295-310
pub async fn validator_basket<V: View + ?Sized>(
    view: &V,
    hotkey_ss58: AccountId,
) -> Result<Vec<ValidatorBasketHolding>, ReadError> {
    let snapshot = view.pin().await?;
    let rows: Vec<(NetUid, u64, u64)> = model(
        snapshot
            .call_dyn(
                "BetaBasketRuntimeApi",
                "get_validator_basket",
                &[Value::Str(hotkey_ss58.to_string())],
            )
            .await?,
        "get_validator_basket",
    )?;
    let symbols = snapshot.client().token_symbols().await;
    Ok(rows
        .into_iter()
        .map(|(netuid, alpha, tao)| ValidatorBasketHolding {
            netuid,
            alpha: owner::contextual_balance(alpha, netuid, &symbols),
            value_tao: owner::contextual_balance(tao, NetUid::ROOT, &symbols),
        })
        .collect())
}

/// Realizable TAO net asset value of a validator's fund.
// py: sdk/python/bittensor/reads/root.py:383-386
pub async fn validator_basket_nav<V: View + ?Sized>(
    view: &V,
    hotkey_ss58: AccountId,
) -> Result<DynBalance, ReadError> {
    let snapshot = view.pin().await?;
    let amount: u64 = model(
        snapshot
            .call_dyn(
                "BetaBasketRuntimeApi",
                "get_validator_basket_nav",
                &[Value::Str(hotkey_ss58.to_string())],
            )
            .await?,
        "get_validator_basket_nav",
    )?;
    Ok(owner::contextual_balance(
        amount,
        NetUid::ROOT,
        &snapshot.client().token_symbols().await,
    ))
}

/// Network-wide realizable basket NAV, denominated in TAO.
// py: sdk/python/bittensor/reads/root.py:428-434
pub async fn root_basket_total_nav<V: View + ?Sized>(view: &V) -> Result<DynBalance, ReadError> {
    let snapshot = view.pin().await?;
    let amount: u64 = model(
        snapshot
            .call_dyn("BetaBasketRuntimeApi", "get_root_basket_total_nav", &[])
            .await?,
        "get_root_basket_total_nav",
    )?;
    Ok(owner::contextual_balance(
        amount,
        NetUid::ROOT,
        &snapshot.client().token_symbols().await,
    ))
}

#[derive(Deserialize)]
struct PricingPage {
    #[serde(default)]
    pricing: Vec<PricingRow>,
    next: Option<AccountId>,
}

// Only fields Python presents are projected. In particular, a historical
// pricing record need not contain the unused spot_nav_tao or shares fields.
#[derive(Deserialize)]
struct PricingRow {
    hotkey: AccountId,
    #[serde(default)]
    spot_price: rtao_primitives::U64F64,
    #[serde(default)]
    display_price: rtao_primitives::U64F64,
    #[serde(default)]
    stake_price: rtao_primitives::U64F64,
    #[serde(default)]
    staker_yield: rtao_primitives::U64F64,
    #[serde(default)]
    staker_twr: rtao_primitives::U64F64,
    #[serde(default)]
    bag_index: rtao_primitives::U64F64,
    #[serde(default)]
    stake_index: rtao_primitives::U64F64,
    #[serde(default)]
    first_block: u64,
    #[serde(default)]
    provisional: bool,
    #[serde(default)]
    display_shares: rtao_primitives::U64F64,
}

impl From<PricingRow> for owner::ChainPricing {
    fn from(row: PricingRow) -> Self {
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

async fn pricing<V: View + ?Sized>(
    view: &V,
) -> Result<BTreeMap<AccountId, owner::ChainPricing>, ReadError> {
    let mut cursor = None;
    let mut seen = BTreeSet::new();
    let mut pricing = BTreeMap::new();
    loop {
        let page: PricingPage = model(
            view.call_dyn(
                "BetaBasketRuntimeApi",
                "get_all_beta_pricing",
                &[
                    cursor.map_or(Value::Null, |key: AccountId| Value::Str(key.to_string())),
                    Value::Uint(0),
                ],
            )
            .await?,
            "get_all_beta_pricing",
        )?;
        for row in page.pricing {
            pricing.insert(row.hotkey, owner::ChainPricing::from(row));
        }
        cursor = page.next;
        let Some(next) = cursor else {
            return Ok(pricing);
        };
        if !seen.insert(next) {
            return Err(surface("get_all_beta_pricing", "pricing cursor repeated"));
        }
    }
}

/// Every active validator fund with batched rates and optional paginated pricing.
///
/// Any pricing failure discards the partial pricing enrichment, matching the
/// Python compatibility path for older nodes; core basket data remains available.
// py: sdk/python/bittensor/reads/root.py:97-128,352-374
pub async fn root_baskets<V: View + ?Sized>(
    view: &V,
) -> Result<Vec<owner::ValidatorBasketSummary>, ReadError> {
    let snapshot = view.pin().await?;
    let rows: Vec<models::BasketSummary> = model(
        snapshot
            .call_dyn("BetaBasketRuntimeApi", "get_all_validator_baskets", &[])
            .await?,
        "get_all_validator_baskets",
    )?;
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let symbols = snapshot.client().token_symbols().await;
    let mut records: Vec<_> = rows
        .into_iter()
        .map(|row| owner::basket_summary(row, &symbols))
        .collect();
    let mut plan = ReadPlan::new();
    for record in &records {
        plan.push(
            st::BASKET_RATE.surface(),
            vec![Value::Str(record.hotkey.to_string())],
        );
    }
    for (record, rate) in records.iter_mut().zip(snapshot.fetch(plan).await?) {
        let rate: rtao_primitives::I96F32 = model(rate, "SubtensorModule.BasketRate")?;
        record.basket_rate = rate.to_f64();
    }
    if let Ok(pricing) = pricing(&snapshot).await {
        for record in &mut records {
            record.chain_pricing = pricing.get(&record.hotkey).cloned();
        }
    }
    records.sort_by_key(|record| std::cmp::Reverse(record.nav_tao.rao));
    Ok(records)
}

/// Trading gates and the validator fund's guarded-NAV turnover bucket.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BasketTradingStatus {
    /// Network-wide trading gate.
    pub enabled: bool,
    /// Per-validator governance freeze.
    pub frozen: bool,
    /// Number of blocks required to refill an empty bucket.
    pub refill_blocks: u64,
    /// Whole rao replenished each block; zero when refill duration is zero.
    pub refill_per_block_tao: DynBalance,
    /// Capacity less availability, saturated at zero.
    pub used_tao: DynBalance,
    /// Bucket capacity in TAO.
    pub budget_tao: DynBalance,
    /// Availability capped at capacity.
    pub remaining_tao: DynBalance,
}

#[derive(Deserialize)]
struct TradingRow {
    enabled: bool,
    frozen: bool,
    refill_blocks: u64,
    tao_available: u64,
    budget_tao: u64,
}

/// Trading gate status, used budget and clamped availability in exact TAO rao.
// py: sdk/python/bittensor/reads/root.py:395-420
pub async fn basket_trading_status<V: View + ?Sized>(
    view: &V,
    hotkey_ss58: AccountId,
) -> Result<BasketTradingStatus, ReadError> {
    let snapshot = view.pin().await?;
    let row: TradingRow = model(
        snapshot
            .call_dyn(
                "BetaBasketRuntimeApi",
                "get_basket_trading_status",
                &[Value::Str(hotkey_ss58.to_string())],
            )
            .await?,
        "get_basket_trading_status",
    )?;
    let symbols = snapshot.client().token_symbols().await;
    let amount = |rao| owner::contextual_balance(rao, NetUid::ROOT, &symbols);
    Ok(BasketTradingStatus {
        enabled: row.enabled,
        frozen: row.frozen,
        refill_blocks: row.refill_blocks,
        refill_per_block_tao: amount(row.budget_tao.checked_div(row.refill_blocks).unwrap_or(0)),
        used_tao: amount(row.budget_tao.saturating_sub(row.tao_available)),
        budget_tao: amount(row.budget_tao),
        remaining_tao: amount(row.tao_available.min(row.budget_tao)),
    })
}

/// Minimal subnet listing record with a TAO-denominated registration burn.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SubnetInfo {
    /// Native subnet identity.
    pub netuid: NetUid,
    /// Blocks per epoch.
    pub tempo: u16,
    /// TAO burned to register a neuron.
    pub burn: DynBalance,
    /// Current number of neurons.
    pub neuron_count: u16,
}

/// Tempo, burn and neuron count fetched together at one immutable block.
// py: sdk/python/bittensor/reads/subnets.py:30-43
pub async fn subnet<V: View + ?Sized>(view: &V, netuid: NetUid) -> Result<SubnetInfo, ReadError> {
    let (tempo, burn, neuron_count) = view
        .fetch((
            st::TEMPO.at((netuid,)),
            st::BURN.at((netuid,)),
            st::SUBNETWORK_N.at((netuid,)),
        ))
        .await?;
    Ok(SubnetInfo {
        netuid,
        tempo,
        burn: DynBalance::from_rao(burn.0, NetUid::ROOT),
        neuron_count,
    })
}

/// All added subnets in native netuid order, using four complete map scans.
///
/// Missing entries in the tempo, burn and count maps produce zero, rather than
/// a storage metadata default; no per-subnet follow-up queries are performed.
// py: sdk/python/bittensor/reads/subnets.py:47-70
pub async fn subnets<V: View + ?Sized>(view: &V) -> Result<Vec<SubnetInfo>, ReadError> {
    let snapshot = view.pin().await?;
    let (added, tempos, burns, counts) = futures::try_join!(
        snapshot
            .storage_iter(&st::NETWORKS_ADDED, ())
            .try_collect::<Vec<_>>(),
        snapshot
            .storage_iter(&st::TEMPO, ())
            .try_collect::<Vec<_>>(),
        snapshot.storage_iter(&st::BURN, ()).try_collect::<Vec<_>>(),
        snapshot
            .storage_iter(&st::SUBNETWORK_N, ())
            .try_collect::<Vec<_>>(),
    )?;
    let mut netuids = BTreeSet::new();
    for (keys, is_added) in added {
        if is_added {
            netuids.insert(subnet_key(&keys, "SubtensorModule.NetworksAdded")?);
        }
    }
    let tempos = tempos
        .into_iter()
        .map(|(keys, value)| Ok((subnet_key(&keys, "SubtensorModule.Tempo")?, value)))
        .collect::<Result<BTreeMap<_, _>, ReadError>>()?;
    let burns = burns
        .into_iter()
        .map(|(keys, value)| Ok((subnet_key(&keys, "SubtensorModule.Burn")?, value.0)))
        .collect::<Result<BTreeMap<_, _>, ReadError>>()?;
    let counts = counts
        .into_iter()
        .map(|(keys, value)| Ok((subnet_key(&keys, "SubtensorModule.SubnetworkN")?, value)))
        .collect::<Result<BTreeMap<_, _>, ReadError>>()?;
    Ok(netuids
        .into_iter()
        .map(|netuid| SubnetInfo {
            netuid,
            tempo: tempos.get(&netuid).copied().unwrap_or(0),
            burn: DynBalance::from_rao(burns.get(&netuid).copied().unwrap_or(0), NetUid::ROOT),
            neuron_count: counts.get(&netuid).copied().unwrap_or(0),
        })
        .collect())
}

/// Recorded signed EMA TAO flows in rao per block, retaining fractional rao.
// py: sdk/python/bittensor/reads/subnets.py:93-111
pub async fn subnet_tao_flows<V: View + ?Sized>(
    view: &V,
) -> Result<BTreeMap<NetUid, f64>, ReadError> {
    let snapshot = view.pin().await?;
    let rows = snapshot
        .storage_iter(&st::SUBNET_EMA_TAO_FLOW, ())
        .try_collect::<Vec<_>>()
        .await?;
    let mut flows = BTreeMap::new();
    for (keys, (_, ema)) in rows {
        flows.insert(
            subnet_key(&keys, "SubtensorModule.SubnetEmaTaoFlow")?,
            ema.to_f64(),
        );
    }
    Ok(flows)
}

/// Current dynamic TAO cost to register a new subnet.
// py: sdk/python/bittensor/reads/subnets.py:282-290
pub async fn subnet_registration_cost<V: View + ?Sized>(view: &V) -> Result<DynBalance, ReadError> {
    let amount = view
        .call(
            &api::subnet_registration_runtime_api::GET_NETWORK_REGISTRATION_COST,
            (),
        )
        .await?;
    Ok(DynBalance::from_rao(amount.0, NetUid::ROOT))
}

/// Sparse validator bond matrix with PerU16 fractions, rather than row normalization.
pub type BondMatrix = BTreeMap<Uid, BTreeMap<Uid, f64>>;

fn mechanism_index(netuid: NetUid, mechid: MechId) -> Result<NetUidStorageIndex, ReadError> {
    NetUidStorageIndex::new(netuid, mechid).map_err(|error| ReadError::InvalidParam {
        name: "netuid/mechid".into(),
        reason: error.to_string(),
    })
}

/// Bond fractions in sorted validator order, keeping empty rows and last duplicate targets.
// py: sdk/python/bittensor/reads/weights.py:59-76
pub async fn bonds<V: View + ?Sized>(
    view: &V,
    netuid: NetUid,
    mechid: MechId,
) -> Result<BondMatrix, ReadError> {
    let index = mechanism_index(netuid, mechid)?;
    let snapshot = view.pin().await?;
    let rows = snapshot
        .storage_iter(&st::BONDS, (index,))
        .try_collect::<Vec<_>>()
        .await?;
    let mut matrix = BTreeMap::new();
    for (keys, row) in rows {
        let uid = u16::try_from(native_key(&keys, "SubtensorModule.Bonds")?)
            .map_err(|error| surface("SubtensorModule.Bonds", error.to_string()))?;
        matrix.insert(
            Uid(uid),
            row.into_iter()
                .map(|(target, value)| {
                    (
                        Uid(target),
                        rtao_primitives::ratio::ratio_fraction("PerU16", i128::from(value))
                            .unwrap_or(0.0),
                    )
                })
                .collect(),
        );
    }
    Ok(matrix)
}

/// One pending encrypted weight commit with its locally computed UTC reveal time.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TimelockedWeightCommit {
    /// Committing validator hotkey.
    pub hotkey: AccountId,
    /// Chain block that accepted the commit.
    pub commit_block: u64,
    /// Native drand reveal round.
    pub reveal_round: u64,
    /// UTC time of that round; round zero is one period before genesis.
    #[serde(serialize_with = "serialize_reveal_time")]
    pub reveals_at: DateTime<Utc>,
    /// Python-compatible `0x` ciphertext string.
    pub ciphertext: String,
}

fn serialize_reveal_time<S: Serializer>(
    value: &DateTime<Utc>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&value.to_rfc3339_opts(SecondsFormat::Secs, false))
}

/// Pending commit rows grouped by sorted epoch, preserving commit order within each row.
// py: sdk/python/bittensor/reads/weights.py:89-113
pub async fn timelocked_weight_commits<V: View + ?Sized>(
    view: &V,
    netuid: NetUid,
    mechid: MechId,
) -> Result<BTreeMap<u64, Vec<TimelockedWeightCommit>>, ReadError> {
    let index = mechanism_index(netuid, mechid)?;
    let snapshot = view.pin().await?;
    let rows = snapshot
        .storage_iter(&st::TIMELOCKED_WEIGHT_COMMITS, (index,))
        .try_collect::<Vec<_>>()
        .await?;
    let mut out = BTreeMap::new();
    for (keys, commits) in rows {
        let epoch = native_key(&keys, "SubtensorModule.TimelockedWeightCommits")?;
        let records = commits
            .into_iter()
            .map(|(hotkey, commit_block, ciphertext, reveal_round)| {
                Ok(TimelockedWeightCommit {
                    hotkey,
                    commit_block,
                    reveal_round,
                    reveals_at: rtao_primitives::drand::reveal_time(reveal_round).map_err(
                        |error| {
                            surface("SubtensorModule.TimelockedWeightCommits", error.to_string())
                        },
                    )?,
                    ciphertext: format!("0x{}", hex::encode(ciphertext)),
                })
            })
            .collect::<Result<Vec<_>, ReadError>>()?;
        out.insert(epoch, records);
    }
    Ok(out)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // Invalid historical model fixtures must fail these assertions.
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn pricing_projection_accepts_missing_historical_fields_and_full_native_bits() {
        let page: PricingPage = serde_json::from_value(json!({
            "pricing": [{"hotkey": AccountId([8; 32]), "spot_price": {"bits": u128::MAX}}],
            "next": null,
        }))
        .unwrap();
        let rows: Vec<owner::ChainPricing> = page.pricing.into_iter().map(Into::into).collect();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].spot_price,
            u128::MAX as f64 / 18446744073709551616.0
        );
        assert_eq!(rows[0].display_price, 0.0);
        assert_eq!(rows[0].display_shares, 0.0);
        assert_eq!(rows[0].first_block, 0);
        assert!(!rows[0].provisional);
        assert!(page.next.is_none());
    }
}
