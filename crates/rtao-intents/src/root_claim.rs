// Derived from RaoFoundation/subtensor sdk/python/bittensor/intents/registration.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Additional source at the same commit/license: sdk/python/bittensor/intents/_root_claim_fee.py.
//! Fail-closed root-claim admission and reserve, with optional yield previews.

use std::collections::{BTreeMap, BTreeSet};

use futures::{StreamExt, TryStreamExt, future::try_join_all, stream};
use rtao_client::{View, ViewExt};
use rtao_core::{DynValue, codec::Value, value_ext::ValueExt};
use rtao_primitives::{AccountId, Balance, DynBalance, NetUid, Ss58Address, Tao};
use rtao_runtime::{
    runtime_apis::{beta_basket_runtime_api as basket, stake_info_runtime_api as stake},
    storage,
};
use rtao_tx::{IntentError, IntentPreflight, PreflightCx};

#[cfg(feature = "registration")]
pub use crate::{ClaimRoot, ClaimRootWithHotkey};
#[cfg(all(test, feature = "registration"))]
use rtao_primitives::IntoCoreValue;
#[cfg(feature = "registration")]
use rtao_tx::{BuildCx, Built, Intent};

const MAX_WORK: u64 = 256;
const MAX_HOTKEY_WORK: u64 = 129;
const FEE_ALLOWANCE: u64 = 4;
const DEFAULT_THRESHOLD_RAO: u64 = 500_000;

fn claim_root_ref_time(units: u64) -> u128 {
    let units = u128::from(units);
    567_441_000
        + 437_318_379 * units
        + (23 + 22 * units) * 25_000_000
        + (18 + 16 * units) * 100_000_000
}

fn claim_root_scan_ref_time(units: u64) -> u128 {
    let units = u128::from(units);
    309_208_000 + 263_604_241 * units + (6 + 21 * units) * 25_000_000
}

fn claim_ref_time(units: u64, quotes: u64, rows: u64) -> u128 {
    let sweep = u128::from(quotes) * (10_000_000 + 4 * 25_000_000);
    let redeem = if rows == 0 {
        0
    } else {
        claim_root_ref_time(rows)
    };
    claim_root_ref_time(units) + claim_root_scan_ref_time(units) + sweep + redeem
}

fn weight_fee_rao(ref_time: u128) -> u128 {
    // LinearWeightToFee rounds to nearest rao; exact ties round down.
    let scaled = ref_time * 250_000;
    scaled / 1_000_000_000 + u128::from((scaled % 1_000_000_000) * 2 > 1_000_000_000)
}

fn charged_ref_time(limit: u64) -> u128 {
    let units = limit.min(FEE_ALLOWANCE);
    claim_ref_time(units, 6 * units, units).min(249_916_000_000 - 60_000_000)
}

fn approximate_reserve(limit: u64) -> Result<Balance<Tao>, IntentError> {
    Ok(Balance::<Tao>::try_from(weight_fee_rao(charged_ref_time(
        limit.max(1),
    )))?)
}

fn money(rao: u64) -> String {
    DynBalance::from(Balance::<Tao>::from_rao(rao)).to_string()
}

fn native_sum(values: impl IntoIterator<Item = u64>) -> Result<u64, IntentError> {
    values.into_iter().try_fold(0u64, |sum, value| {
        sum.checked_add(value)
            .ok_or_else(|| IntentError::validation("root-claim total exceeds native u64"))
    })
}

#[derive(Debug)]
struct Admission {
    hotkeys: Vec<AccountId>,
    holding_counts: Vec<u64>,
    limit: u64,
    selection_scans: u64,
}

impl Admission {
    fn holdings(&self) -> Result<u64, IntentError> {
        native_sum(self.holding_counts.iter().copied())
    }

    fn blocks(&self) -> Result<Vec<String>, IntentError> {
        admission_blocks(
            self.hotkeys.len() as u64,
            self.holdings()?,
            self.limit,
            self.selection_scans,
        )
    }
}

fn admission_blocks(
    hotkeys: u64,
    holdings: u64,
    limit: u64,
    selection_scans: u64,
) -> Result<Vec<String>, IntentError> {
    let work = native_sum([hotkeys, holdings])?;
    if work <= limit && selection_scans <= limit {
        return Ok(vec![]);
    }
    let remediation = if hotkeys > 1 {
        "claim one validator at a time with claim_root_with_hotkey"
    } else {
        "the claim cannot be admitted until this basket's work is reduced"
    };
    let mut reasons = vec![];
    if work > limit {
        let hotkey_label = if hotkeys == 1 { "hotkey" } else { "hotkeys" };
        let holding_label = if holdings == 1 { "holding" } else { "holdings" };
        reasons.push(format!(
            "{hotkeys} root {hotkey_label} + {holdings} basket {holding_label} = {work}"
        ));
    }
    if selection_scans > limit {
        reasons.push(format!(
            "{selection_scans} staking-hotkey relationships to classify"
        ));
    }
    Ok(vec![format!(
        "root claim exceeds the {limit}-unit admission limit ({}); {remediation}",
        reasons.join("; ")
    )])
}

async fn admission<V: View + ?Sized>(
    view: &V,
    claimant: AccountId,
    hotkey: Option<AccountId>,
) -> Result<Admission, IntentError> {
    let (selected, selection_scans) = if let Some(hotkey) = hotkey {
        (vec![hotkey], 0)
    } else {
        let raw_keys = view
            .storage(&storage::subtensor_module::STAKING_HOTKEYS, (claimant,))
            .await?;
        let positions = view
            .call(&stake::GET_STAKE_INFO_FOR_COLDKEY, (claimant,))
            .await?;
        let root_hotkeys: BTreeSet<_> = positions
            .into_iter()
            .filter(|row| row.netuid == NetUid::ROOT && row.stake.0 > 0)
            .map(|row| row.hotkey)
            .collect();
        let watermarks = try_join_all(raw_keys.iter().map(|hotkey| {
            view.storage(
                &storage::subtensor_module::BASKET_CLAIMED,
                (*hotkey, claimant),
            )
        }))
        .await?;
        let scans = raw_keys.len() as u64;
        let selected = raw_keys
            .into_iter()
            .zip(watermarks)
            .filter_map(|(hotkey, watermark)| {
                (root_hotkeys.contains(&hotkey) || watermark < 0).then_some(hotkey)
            })
            .collect();
        (selected, scans)
    };
    let holding_counts = stream::iter(selected.iter().copied().map(|hotkey| async move {
        Ok::<_, IntentError>(
            view.call(&basket::GET_VALIDATOR_BASKET, (hotkey,))
                .await?
                .len() as u64,
        )
    }))
    .buffered(16)
    .try_collect()
    .await?;
    // Python still verifies NetworksAdded, although network count is no longer
    // used by admission or any displayed fact. A failed scan must still refuse.
    let networks = view.storage_iter(&storage::subtensor_module::NETWORKS_ADDED, ());
    futures::pin_mut!(networks);
    while networks.try_next().await?.is_some() {}
    Ok(Admission {
        hotkeys: selected,
        holding_counts,
        limit: if hotkey.is_some() {
            MAX_HOTKEY_WORK
        } else {
            MAX_WORK
        },
        selection_scans,
    })
}

struct Reserve {
    reserved: Balance<Tao>,
    free: Balance<Tao>,
    exact: bool,
}

impl Reserve {
    fn blocks(&self) -> Vec<String> {
        if self.free >= self.reserved {
            vec![]
        } else {
            vec![format!(
                "free TAO ({}) is below the reserved claim fee ({})",
                money(self.free.rao()),
                money(self.reserved.rao())
            )]
        }
    }
}

async fn reserve<V: View + ?Sized>(
    cx: &PreflightCx<'_, V>,
    limit: u64,
) -> Result<Reserve, IntentError> {
    let account = cx
        .view
        .storage(&storage::system::ACCOUNT, (cx.fee_payer,))
        .await?;
    let free = account
        .as_core()
        .field("data")
        .and_then(|data| data.field("free"))
        .and_then(Value::as_u128)
        .and_then(|value| u64::try_from(value).ok())
        .ok_or_else(|| IntentError::validation("System.Account.data.free is not native u64"))?;
    let (reserved, exact) = match cx.estimate_fee().await {
        Ok(fee) => (fee, true),
        Err(_) => (approximate_reserve(limit)?, false),
    };
    Ok(Reserve {
        reserved,
        free: Balance::from_rao(free),
        exact,
    })
}

#[derive(Clone, Debug)]
struct Preview {
    accrued: u64,
    redeemable: u64,
    forfeited: u64,
    rows: u64,
    rows_to_sell: u64,
    dust_rows: u64,
    swept: u64,
}

fn preview_number(raw: &Value, field: &str) -> Result<u64, IntentError> {
    raw.field(field)
        .and_then(Value::as_u128)
        .and_then(|number| u64::try_from(number).ok())
        .ok_or_else(|| {
            IntentError::validation(format!("BasketClaimPreview.{field} is not native u64"))
        })
}

fn decode_preview(raw: &DynValue) -> Result<Preview, IntentError> {
    let raw = raw.as_core();
    // The pinned Python decoder validates this field even though the spent
    // floor deliberately excludes unknowable flush work.
    preview_number(raw, "flushed_credits")?;
    Ok(Preview {
        accrued: preview_number(raw, "accrued_tao")?,
        redeemable: preview_number(raw, "redeemable_tao")?,
        forfeited: preview_number(raw, "forfeited_tao_est")?,
        rows: preview_number(raw, "rows")?,
        rows_to_sell: preview_number(raw, "rows_to_sell")?,
        dust_rows: preview_number(raw, "dust_rows")?,
        swept: preview_number(raw, "swept")?,
    })
}

async fn claim_previews<V: View + ?Sized>(
    view: &V,
    claimant: AccountId,
    admission: &Admission,
    coldkey_wide: bool,
) -> Result<Option<Vec<Option<Preview>>>, IntentError> {
    if coldkey_wide {
        let rows = view
            .call(&basket::GET_ROOT_BASKET_CLAIM_PREVIEWS, (claimant,))
            .await?;
        let mut by_hotkey = BTreeMap::new();
        for row in rows {
            let hotkey: AccountId = row
                .as_core()
                .field("hotkey")
                .and_then(Value::as_str)
                .ok_or_else(|| IntentError::validation("BasketClaimPreview.hotkey is unavailable"))?
                .parse()?;
            by_hotkey.insert(hotkey, decode_preview(&row)?);
        }
        Ok(Some(
            admission
                .hotkeys
                .iter()
                .map(|hotkey| by_hotkey.get(hotkey).cloned())
                .collect(),
        ))
    } else {
        let hotkey = admission.hotkeys[0];
        let raw = match view
            .call(&basket::GET_BASKET_CLAIM_PREVIEW, (hotkey, claimant))
            .await
        {
            Ok(raw) => raw,
            Err(_) => return Ok(None),
        };
        raw.map(|raw| decode_preview(&raw).map(|preview| vec![Some(preview)]))
            .transpose()
    }
}

fn spent_fee(
    reserved: Balance<Tao>,
    hotkeys: u64,
    redeem_holdings: u64,
    scan_holdings: u64,
    selection_scans: u64,
    limit: u64,
) -> Balance<Tao> {
    let reserved_rao = u128::from(reserved.rao());
    let charged = weight_fee_rao(charged_ref_time(limit));
    let base = reserved_rao - reserved_rao.min(charged);
    let active = redeem_holdings.max(hotkeys).max(selection_scans).max(1);
    let actual =
        weight_fee_rao(claim_root_ref_time(active) + claim_root_scan_ref_time(scan_holdings));
    // min with a native u64 reserve proves this conversion cannot overflow.
    Balance::from_rao((base + actual).min(reserved_rao) as u64)
}

struct Quote {
    holdings: u64,
    spent: Balance<Tao>,
    accrued: u64,
    threshold: u64,
    hotkeys: u64,
    eligible_hotkeys: u64,
    below_threshold_hotkeys: u64,
    redeemable: u64,
    dust_rows: u64,
    forfeited: u64,
}

async fn quote<V: View + ?Sized>(
    cx: &PreflightCx<'_, V>,
    admission: &Admission,
    reserve: &Reserve,
    coldkey_wide: bool,
) -> Result<Quote, IntentError> {
    let claimant = cx.dispatch_origin;
    // Missing or failed dust preview alone falls back to the older payout API.
    let previews = if coldkey_wide {
        claim_previews(cx.view, claimant, admission, true)
            .await
            .ok()
            .flatten()
    } else {
        claim_previews(cx.view, claimant, admission, false).await?
    };
    let mut payouts = vec![];
    let mut accrued = vec![];
    let mut sell_counts = vec![];
    let mut swept_counts = vec![];
    let mut scan_counts = vec![];
    let mut dust_counts = vec![];
    let mut forfeited = vec![];
    if let Some(previews) = previews {
        for (preview, count) in previews.into_iter().zip(&admission.holding_counts) {
            payouts.push(preview.as_ref().map(|preview| preview.redeemable));
            accrued.push(preview.as_ref().map_or(0, |preview| preview.accrued));
            sell_counts.push(
                preview
                    .as_ref()
                    .map_or(*count, |preview| preview.rows_to_sell),
            );
            swept_counts.push(preview.as_ref().map_or(0, |preview| preview.swept));
            scan_counts.push(preview.as_ref().map_or(*count, |preview| preview.rows));
            dust_counts.push(preview.as_ref().map_or(0, |preview| preview.dust_rows));
            forfeited.push(preview.as_ref().map_or(0, |preview| preview.forfeited));
        }
    } else {
        if coldkey_wide {
            let rows = cx
                .view
                .call(&basket::GET_ROOT_BASKET_POSITIONS, (claimant,))
                .await?;
            let by_hotkey: BTreeMap<_, _> = rows
                .into_iter()
                .map(|(hotkey, _shares, payout)| (hotkey, payout.0))
                .collect();
            payouts = admission
                .hotkeys
                .iter()
                .map(|hotkey| by_hotkey.get(hotkey).copied())
                .collect();
        } else {
            payouts.push(Some(
                cx.view
                    .call(&basket::GET_BASKET_PAYOUT, (admission.hotkeys[0], claimant))
                    .await?
                    .0,
            ));
        }
        accrued.extend(payouts.iter().flatten().copied());
        sell_counts.clone_from(&admission.holding_counts);
        scan_counts.clone_from(&admission.holding_counts);
        swept_counts.resize(payouts.len(), 0);
    }
    let raw_threshold = cx
        .view
        .storage(
            &storage::subtensor_module::ROOT_CLAIMABLE_THRESHOLD,
            (NetUid::ROOT,),
        )
        .await?
        .int_part();
    let threshold = if raw_threshold > 0 {
        u64::try_from(raw_threshold)
            .map_err(|_| IntentError::validation("root claim threshold exceeds native u64"))?
    } else {
        DEFAULT_THRESHOLD_RAO
    };
    let mut eligible_hotkeys = 0;
    let mut below_threshold_hotkeys = 0;
    let mut redeemed = swept_counts;
    let mut scanned = vec![];
    let mut redeemable = vec![];
    for ((payout, sold), count) in payouts.into_iter().zip(sell_counts).zip(scan_counts) {
        if let Some(payout) = payout {
            if payout > 0 && payout >= threshold {
                eligible_hotkeys += 1;
                redeemed.push(sold);
                scanned.push(count.checked_sub(sold).ok_or_else(|| {
                    IntentError::validation("BasketClaimPreview.rows_to_sell exceeds rows")
                })?);
                redeemable.push(payout);
            } else {
                below_threshold_hotkeys += 1;
                scanned.push(count);
            }
        }
    }
    let hotkeys = admission.hotkeys.len() as u64;
    Ok(Quote {
        holdings: admission.holdings()?,
        spent: spent_fee(
            reserve.reserved,
            hotkeys.max(1),
            native_sum(redeemed)?,
            native_sum(scanned)?,
            admission.selection_scans,
            admission.limit,
        ),
        accrued: native_sum(accrued)?,
        threshold,
        hotkeys,
        eligible_hotkeys,
        below_threshold_hotkeys,
        redeemable: native_sum(redeemable)?,
        dust_rows: native_sum(dust_counts)?,
        forfeited: native_sum(forfeited)?,
    })
}

impl Quote {
    fn apply(&self, result: &mut IntentPreflight, reserve: &Reserve) {
        let kinds = if self.holdings == 1 {
            "holding"
        } else {
            "holdings"
        };
        let holdings = format!(
            "{} basket {kinds} (fee scales with ALPHA types)",
            self.holdings
        );
        let reserved = money(reserve.reserved.rao());
        let spent = money(self.spent.rao());
        let accrued = money(self.accrued);
        let redeemable = money(self.redeemable);
        let threshold = money(self.threshold);
        let refund = reserve.reserved.rao() - self.spent.rao();
        let mut fee_line = format!("reserved {reserved} at inclusion; spent ~{spent}");
        result.facts.extend([
            ("holdings".into(), holdings.clone()),
            ("reserved".into(), format!("{reserved} at inclusion")),
            ("spent".into(), format!("~{spent}")),
        ]);
        if refund > 0 {
            let refund = money(refund);
            fee_line.push_str(&format!("; ~{refund} refunded"));
            result.facts.push(("refunded".into(), format!("~{refund}")));
        }
        result.facts.push(("accrued".into(), accrued.clone()));
        result
            .effects
            .extend([holdings, fee_line, format!("accrued {accrued}")]);
        if self.dust_rows > 0 {
            let forfeited = money(self.forfeited);
            result.facts.extend([
                ("redeemable".into(), redeemable.clone()),
                (
                    "dust".into(),
                    format!(
                        "{} rows skipped; ~{forfeited} stays in the fund",
                        self.dust_rows
                    ),
                ),
            ]);
            let row = if self.dust_rows == 1 { "row" } else { "rows" };
            result.effects.push(format!(
                "{} dust {row} skipped: redeems {redeemable}, ~{forfeited} stays in the fund",
                self.dust_rows
            ));
        }
        let below = self.eligible_hotkeys == 0 && self.below_threshold_hotkeys > 0;
        if below {
            result.effects.push(format!("accrued is below the claim threshold ({threshold}); the claim is a no-op and you still pay the scan fee"));
            result.warnings.push(format!("accrued {accrued} is below the claim threshold ({threshold}); the claim pays a scan fee and realizes nothing"));
        } else if self.below_threshold_hotkeys > 0 {
            result.effects.push(format!("{} of {} validators are below the per-validator claim threshold ({threshold}) and remain unclaimed", self.below_threshold_hotkeys, self.hotkeys));
            result.warnings.push(format!("{} of {} validators are individually below the claim threshold ({threshold}); their yield remains accrued", self.below_threshold_hotkeys, self.hotkeys));
        }
        if self.spent.rao() > self.redeemable {
            result.effects.push(format!(
                "this claim loses money: spent fee ~{spent} exceeds redeemable accrued {redeemable}"
            ));
            if !below {
                result.warnings.push(format!("this claim loses money: spent fee ~{spent} exceeds redeemable accrued {redeemable}; wait until more yield accrues"));
            }
        }
    }
}

async fn claim_preflight<V: View + ?Sized>(
    summary: String,
    hotkey: Option<AccountId>,
    cx: &PreflightCx<'_, V>,
) -> Result<IntentPreflight, IntentError> {
    let mut result = IntentPreflight {
        effects: vec![summary],
        ..Default::default()
    };
    let admission = match admission(cx.view, cx.dispatch_origin, hotkey).await {
        Ok(admission) => admission,
        Err(error) => {
            result.blocks.push(format!("could not verify the root claim's fixed admission budget; refusing to risk the unreduced declared fee ({error})"));
            return Ok(result);
        }
    };
    match admission.blocks() {
        Ok(blocks) if blocks.is_empty() => {}
        Ok(blocks) => {
            result.blocks = blocks;
            return Ok(result);
        }
        Err(error) => {
            result.blocks.push(format!("could not verify the root claim's fixed admission budget; refusing to risk the unreduced declared fee ({error})"));
            return Ok(result);
        }
    }
    let reserve = match reserve(cx, admission.limit).await {
        Ok(reserve) => reserve,
        Err(error) => {
            result.blocks.push(format!("could not verify the root claim's reserved fee and free TAO; refusing to risk the unreduced declared fee ({error})"));
            return Ok(result);
        }
    };
    result.required_free = Some(reserve.reserved);
    result.available_free = Some(reserve.free);
    result.estimated_fee = reserve.exact.then_some(reserve.reserved);
    result.blocks = reserve.blocks();
    // Payout and threshold failures remove optional facts, never the mandatory
    // admission or affordability result established above.
    if let Ok(quote) = quote(cx, &admission, &reserve, hotkey.is_none()).await {
        quote.apply(&mut result, &reserve);
    }
    Ok(result)
}

/// The same root-claim preview used by claim-aware staking batches.
pub(crate) async fn claim_with_hotkey_preflight<V: View + ?Sized>(
    hotkey: &Ss58Address,
    cx: &PreflightCx<'_, V>,
) -> Result<IntentPreflight, IntentError> {
    claim_preflight(
        format!("claim root dividends on {hotkey} (redeem basket shares to root stake)"),
        Some(hotkey.account()),
        cx,
    )
    .await
}

#[cfg(feature = "registration")]
impl Intent for ClaimRoot {
    fn summary(&self) -> String {
        "claim root dividends on all validators (redeem basket shares to root stake)".into()
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        crate::generated::passthrough::claim_root(self, cx).await
    }

    async fn preflight<V: View + ?Sized>(
        &self,
        cx: &PreflightCx<'_, V>,
    ) -> Result<IntentPreflight, IntentError> {
        claim_preflight(self.summary(), None, cx).await
    }

    async fn effects<V: View + ?Sized>(
        &self,
        cx: &PreflightCx<'_, V>,
    ) -> Result<Vec<String>, IntentError> {
        Ok(self.preflight(cx).await?.effects)
    }
    async fn warnings<V: View + ?Sized>(
        &self,
        cx: &PreflightCx<'_, V>,
    ) -> Result<Vec<String>, IntentError> {
        Ok(self.preflight(cx).await?.warnings)
    }
    async fn blocks<V: View + ?Sized>(
        &self,
        cx: &PreflightCx<'_, V>,
    ) -> Result<Vec<String>, IntentError> {
        Ok(self.preflight(cx).await?.blocks)
    }
}

#[cfg(feature = "registration")]
impl Intent for ClaimRootWithHotkey {
    fn summary(&self) -> String {
        format!(
            "claim root dividends on {} (redeem basket shares to root stake)",
            self.hotkey_ss58
        )
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        crate::generated::passthrough::claim_root_with_hotkey(self, cx).await
    }

    async fn preflight<V: View + ?Sized>(
        &self,
        cx: &PreflightCx<'_, V>,
    ) -> Result<IntentPreflight, IntentError> {
        claim_with_hotkey_preflight(&self.hotkey_ss58, cx).await
    }

    async fn effects<V: View + ?Sized>(
        &self,
        cx: &PreflightCx<'_, V>,
    ) -> Result<Vec<String>, IntentError> {
        Ok(self.preflight(cx).await?.effects)
    }
    async fn warnings<V: View + ?Sized>(
        &self,
        cx: &PreflightCx<'_, V>,
    ) -> Result<Vec<String>, IntentError> {
        Ok(self.preflight(cx).await?.warnings)
    }
    async fn blocks<V: View + ?Sized>(
        &self,
        cx: &PreflightCx<'_, V>,
    ) -> Result<Vec<String>, IntentError> {
        Ok(self.preflight(cx).await?.blocks)
    }
}

#[cfg(all(test, feature = "registration"))]
mod tests {
    use super::*;
    use rtao_client::{BlockRef, Client, ClientConfig, Network, Snapshot};
    use rtao_core::runtime::TypeSpec;
    use rtao_runtime::{
        CallBytes,
        calls::{proxy, subtensor_module as calls},
    };
    use rtao_testkit::{FakeChain, FakeState, RecordingView, rpc::RpcClient};
    use rtao_tx::{IntentInterface, OriginView, SystemClock};
    use serde_json::{Value as Json, json};

    const CLAIMANT: AccountId = AccountId([7; 32]);
    const PAYER: AccountId = AccountId([9; 32]);
    const HOTKEY: AccountId = AccountId([8; 32]);
    const OTHER_HOTKEY: AccountId = AccountId([10; 32]);

    fn seed_admission(
        state: &mut FakeState,
        keys: &[AccountId],
        root_keys: &[AccountId],
        holdings: usize,
    ) -> anyhow::Result<()> {
        state.seed(
            &storage::subtensor_module::STAKING_HOTKEYS,
            (CLAIMANT,),
            keys.to_vec(),
        )?;
        state.seed_runtime("StakeInfoRuntimeApi", "get_stake_info_for_coldkey", Json::Array(root_keys.iter().map(|hotkey| {
            json!({"hotkey":hotkey.to_string(), "coldkey":CLAIMANT.to_string(), "netuid":0, "stake":1})
        }).collect()))?;
        state.seed_runtime(
            "BetaBasketRuntimeApi",
            "get_validator_basket",
            json!((0..holdings).map(|_| json!([1, 1, 1])).collect::<Vec<_>>()),
        )?;
        state.seed(
            &storage::subtensor_module::NETWORKS_ADDED,
            (NetUid::ROOT,),
            true,
        )?;
        Ok(())
    }

    fn seed_single_preview(
        state: &mut FakeState,
        accrued: u64,
        redeemable: u64,
        rows: u32,
        sold: u32,
        swept: u32,
    ) -> anyhow::Result<()> {
        state.seed_runtime(
            "BetaBasketRuntimeApi",
            "get_basket_claim_preview",
            preview_json(HOTKEY, accrued, redeemable, rows, sold, swept),
        )?;
        Ok(())
    }

    fn preview_json(
        hotkey: AccountId,
        accrued: u64,
        redeemable: u64,
        rows: u32,
        sold: u32,
        swept: u32,
    ) -> Json {
        json!({"hotkey":hotkey.to_string(), "owed_shares":1, "accrued_tao":accrued,
            "redeemable_tao":redeemable, "forfeited_tao_est":accrued-redeemable,
            "rows":rows, "rows_to_sell":sold, "dust_rows":rows-sold,
            "swept":swept, "flushed_credits":0})
    }

    async fn fixture(
        setup: impl FnOnce(&mut FakeState) -> anyhow::Result<()>,
    ) -> anyhow::Result<(FakeChain, Client, Snapshot)> {
        let mut state = FakeState::try_default()?;
        seed_admission(&mut state, &[HOTKEY], &[HOTKEY], 1)?;
        state.seed_python(
            "System",
            "Account",
            &[json!(CLAIMANT.to_string())],
            json!({"data":{"free":1_000_000_000}}),
        )?;
        state.seed_python(
            "System",
            "Account",
            &[json!(PAYER.to_string())],
            json!({"data":{"free":1_000_000_000}}),
        )?;
        state.seed_runtime(
            "BetaBasketRuntimeApi",
            "get_basket_claim_preview",
            Json::Null,
        )?;
        // An unseeded old runtime view is represented by a failing modern
        // preview call. Explicit legacy positions below still produce a quote.
        state.seed_runtime_encoded(
            "BetaBasketRuntimeApi_get_root_basket_claim_previews",
            CLAIMANT.0.to_vec(),
            vec![255],
        );
        state.seed_runtime("BetaBasketRuntimeApi", "get_basket_payout", json!(900_000))?;
        state.seed_runtime(
            "BetaBasketRuntimeApi",
            "get_root_basket_positions",
            json!([[HOTKEY.to_string(), 1, 900_000]]),
        )?;
        state.fee = 8_209_263;
        setup(&mut state)?;
        let chain = FakeChain::from_state(state);
        let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
        config.track_heads = false;
        config.runtime_cache_dir = None;
        config.config_store = None;
        let client = Client::from_rpc(RpcClient::new(chain.clone()), config).await?;
        let snapshot = client.at(BlockRef::Finalized).await?;
        Ok((chain, client, snapshot))
    }

    fn single() -> ClaimRootWithHotkey {
        ClaimRootWithHotkey {
            hotkey_ss58: HOTKEY.into(),
        }
    }

    async fn build<I: Intent>(intent: &I, snapshot: &Snapshot) -> anyhow::Result<Built> {
        Ok(intent
            .build(&BuildCx {
                view: snapshot,
                origin: OriginView::Account(CLAIMANT),
                runtime: &snapshot.block().runtime,
                clock: &SystemClock,
                spec: I::SPEC,
            })
            .await?)
    }

    async fn inspect<I: Intent, V: View + ?Sized>(
        intent: &I,
        view: &V,
        snapshot: &Snapshot,
        client: &Client,
        call: &CallBytes,
        payer: AccountId,
    ) -> anyhow::Result<IntentPreflight> {
        Ok(intent
            .preflight(&PreflightCx {
                view,
                runtime: &snapshot.block().runtime,
                clock: &SystemClock,
                spec: I::SPEC,
                client,
                dispatch_origin: CLAIMANT,
                fee_payer: payer,
                call,
            })
            .await?)
    }

    fn fact<'a>(preview: &'a IntentPreflight, label: &str) -> Option<&'a str> {
        preview
            .facts
            .iter()
            .find(|(key, _)| key == label)
            .map(|(_, value)| value.as_str())
    }

    #[test]
    fn weight_model_matches_recorded_spec_467_node_quotes_and_rounding() -> anyhow::Result<()> {
        // Independent node quotes recorded in the pinned upstream regression:
        // declared ref_time includes the 60M dispatch-extension fold.
        for (limit, declared) in [(129, 2_048_205_118_028u128), (256, 2_476_947_290_768)] {
            assert_eq!(claim_ref_time(limit, 2_560, 512) + 60_000_000, declared);
            let discount = claim_ref_time(limit, 2_560, 512) - charged_ref_time(limit);
            assert_eq!(weight_fee_rao(declared - discount), 8_224_263);
            assert_eq!(approximate_reserve(limit)?.rao(), 8_209_263);
        }
        let discounts =
            [129, 256].map(|limit| claim_ref_time(limit, 2_560, 512) - charged_ref_time(limit));
        assert_eq!(
            weight_fee_rao(4_525_285_010_166 - discounts.into_iter().sum::<u128>()),
            16_481_677
        );
        assert_eq!(weight_fee_rao(2_000), 0);
        assert_eq!(weight_fee_rao(6_000), 1);
        assert_eq!(weight_fee_rao(6_001), 2);
        assert_eq!(
            spent_fee(Balance::from_rao(8_209_263 + 12_345), 1, 1, 0, 0, 129).rao(),
            1_509_587
        );
        assert_eq!(
            spent_fee(Balance::from_rao(8_209_263), 1, 32, 0, 0, 129).rao(),
            8_209_263
        );
        assert_eq!(spent_fee(Balance::ZERO, 1, 32, 0, 0, 129), Balance::ZERO);
        Ok(())
    }

    #[tokio::test]
    async fn claim_calls_preserve_native_scale_prefixes_subnets_and_base_policy()
    -> anyhow::Result<()> {
        let (_chain, client, snapshot) = fixture(|_| Ok(())).await?;
        for subnets in [
            vec![],
            vec![NetUid::ROOT],
            vec![NetUid::ROOT, NetUid(3), NetUid(u16::MAX)],
        ] {
            let intent = ClaimRoot {
                subnets: subnets.clone(),
            };
            let built = build(&intent, &snapshot).await?;
            let mut expected = vec![7, 121, (subnets.len() as u8) << 2];
            for netuid in subnets {
                expected.extend_from_slice(&netuid.get().to_le_bytes());
            }
            assert_eq!(built.call.as_bytes(), expected);
            assert_eq!(
                CallBytes::decode(&expected, &snapshot.block().runtime)?,
                built.call
            );
            assert!(built.extras.is_empty());
            assert!(intent.touches_netuids().is_empty());
            assert!(!intent.affects_all_subnets());
            assert_eq!(intent.spend(), rtao_primitives::Spend::None);
            assert_eq!(
                crate::AnyIntent::from_tool("claim_root", intent.to_json())?.to_json(),
                intent.to_json()
            );
        }
        for prefix in [0, 2, 42, 110] {
            let spelling = HOTKEY.to_ss58(prefix);
            let intent = ClaimRootWithHotkey {
                hotkey_ss58: spelling.parse()?,
            };
            let built = build(&intent, &snapshot).await?;
            let mut expected = vec![7, 148];
            expected.extend_from_slice(&HOTKEY.0);
            assert_eq!(built.call.as_bytes(), expected);
            assert_eq!(
                intent.summary(),
                format!("claim root dividends on {spelling} (redeem basket shares to root stake)")
            );
            assert_eq!(intent.to_json()["hotkey_ss58"], spelling);
            assert_eq!(intent.spend(), rtao_primitives::Spend::None);
            assert!(intent.touches_netuids().is_empty());
            assert!(!intent.affects_all_subnets());
        }
        assert_eq!(ClaimRoot::SPEC.signer, "coldkey");
        assert_eq!(ClaimRootWithHotkey::SPEC.signer, "coldkey");
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn coldkey_admission_keeps_root_and_negative_watermark_positions_in_vector_order()
    -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture(|state| {
            seed_admission(
                state,
                &[OTHER_HOTKEY, HOTKEY, AccountId([11; 32]), OTHER_HOTKEY],
                &[HOTKEY],
                1,
            )?;
            state.seed(
                &storage::subtensor_module::BASKET_CLAIMED,
                (OTHER_HOTKEY, CLAIMANT),
                -1,
            )?;
            Ok(())
        })
        .await?;
        let recording = RecordingView::new(snapshot.clone());
        let admitted = admission(&recording, CLAIMANT, None).await?;
        assert_eq!(admitted.hotkeys, [OTHER_HOTKEY, HOTKEY, OTHER_HOTKEY]);
        assert_eq!(admitted.holding_counts, [1, 1, 1]);
        assert_eq!(admitted.selection_scans, 4);
        assert!(admitted.blocks()?.is_empty());
        let touched = recording
            .touches()
            .into_iter()
            .map(|touch| touch.item)
            .collect::<Vec<_>>();
        assert_eq!(
            touched,
            [
                "StakingHotkeys",
                "get_stake_info_for_coldkey",
                "BasketClaimed",
                "BasketClaimed",
                "BasketClaimed",
                "BasketClaimed",
                "get_validator_basket",
                "get_validator_basket",
                "get_validator_basket",
                "NetworksAdded"
            ]
        );
        assert_eq!(recording.block_hashes().len(), 1);
        assert!(
            !chain
                .requests()
                .iter()
                .any(|request| request.params[0] == "TransactionPaymentApi_query_info")
        );
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn admission_boundaries_stop_before_fee_and_optional_yield_reads() -> anyhow::Result<()> {
        for (holdings, allowed) in [(128, true), (129, false)] {
            let (chain, client, snapshot) =
                fixture(|state| seed_admission(state, &[HOTKEY], &[HOTKEY], holdings)).await?;
            let intent = single();
            let built = build(&intent, &snapshot).await?;
            let preview =
                inspect(&intent, &snapshot, &snapshot, &client, &built.call, PAYER).await?;
            assert_eq!(
                preview
                    .blocks
                    .iter()
                    .all(|block| !block.contains("admission limit")),
                allowed
            );
            if !allowed {
                assert_eq!(preview.effects, [intent.summary()]);
                assert_eq!(
                    preview.blocks,
                    [
                        "root claim exceeds the 129-unit admission limit (1 root hotkey + 129 basket holdings = 130); the claim cannot be admitted until this basket's work is reduced"
                    ]
                );
                assert!(preview.required_free.is_none());
                assert!(
                    !chain
                        .requests()
                        .iter()
                        .any(|request| request.params[0] == "TransactionPaymentApi_query_info")
                );
            }
            client.close().await;
        }
        assert!(admission_blocks(1, 255, 256, 256)?.is_empty());
        assert!(
            admission_blocks(1, 256, 256, 256)?[0]
                .contains("1 root hotkey + 256 basket holdings = 257")
        );
        let (chain, client, snapshot) =
            fixture(|state| seed_admission(state, &[OTHER_HOTKEY; 257], &[], 0)).await?;
        let intent = ClaimRoot {
            subnets: vec![NetUid::ROOT],
        };
        let built = build(&intent, &snapshot).await?;
        let preview = inspect(&intent, &snapshot, &snapshot, &client, &built.call, PAYER).await?;
        assert_eq!(
            preview.blocks,
            [
                "root claim exceeds the 256-unit admission limit (257 staking-hotkey relationships to classify); the claim cannot be admitted until this basket's work is reduced"
            ]
        );
        assert!(
            !chain
                .requests()
                .iter()
                .any(|request| request.params[0] == "TransactionPaymentApi_query_info")
        );
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn admission_read_failure_refuses_without_pricing_or_optional_facts() -> anyhow::Result<()>
    {
        for failure in ["basket", "stake", "watermark", "networks"] {
            let (chain, client, snapshot) = fixture(|state| {
                match failure {
                    "basket" => state.seed_runtime_encoded(
                        "BetaBasketRuntimeApi_get_validator_basket",
                        HOTKEY.0.to_vec(),
                        vec![255],
                    ),
                    "stake" => state.seed_runtime_encoded(
                        "StakeInfoRuntimeApi_get_stake_info_for_coldkey",
                        CLAIMANT.0.to_vec(),
                        vec![255],
                    ),
                    "watermark" => {
                        let entry = state
                            .runtime
                            .storage_entry("SubtensorModule", "BasketClaimed")
                            .ok_or_else(|| anyhow::anyhow!("BasketClaimed"))?;
                        let key = state.runtime.storage_key(
                            entry,
                            &[HOTKEY.into_core_value(), CLAIMANT.into_core_value()],
                        )?;
                        state.seed_encoded(key, Some(vec![255]));
                    }
                    _ => state.set_result("state_getKeysPaged", json!("invalid")),
                }
                Ok(())
            })
            .await?;
            let intent = ClaimRoot {
                subnets: vec![NetUid::ROOT],
            };
            let built = build(&intent, &snapshot).await?;
            let preview =
                inspect(&intent, &snapshot, &snapshot, &client, &built.call, PAYER).await?;
            assert_eq!(preview.effects, [intent.summary()]);
            assert!(preview.blocks[0].starts_with("could not verify the root claim's fixed admission budget; refusing to risk the unreduced declared fee ("), "{failure}");
            assert!(preview.facts.is_empty());
            assert!(preview.warnings.is_empty());
            assert!(preview.required_free.is_none());
            assert!(
                !chain
                    .requests()
                    .iter()
                    .any(|request| request.params[0] == "TransactionPaymentApi_query_info")
            );
            client.close().await;
        }
        Ok(())
    }

    #[tokio::test]
    async fn reserve_failure_refuses_and_optional_quote_failure_keeps_affordability()
    -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture(|state| {
            let entry = state
                .runtime
                .storage_entry("System", "Account")
                .ok_or_else(|| anyhow::anyhow!("System.Account"))?;
            let key = state
                .runtime
                .storage_key(entry, &[PAYER.into_core_value()])?;
            state.seed_encoded(key, Some(vec![255]));
            Ok(())
        })
        .await?;
        let intent = single();
        let built = build(&intent, &snapshot).await?;
        let preview = inspect(&intent, &snapshot, &snapshot, &client, &built.call, PAYER).await?;
        assert!(preview.blocks[0].starts_with("could not verify the root claim's reserved fee and free TAO; refusing to risk the unreduced declared fee ("));
        assert!(preview.required_free.is_none());
        assert!(
            !chain
                .requests()
                .iter()
                .any(|request| request.params[0] == "TransactionPaymentApi_query_info")
        );
        client.close().await;

        let (_chain, client, snapshot) = fixture(|state| {
            state.seed_python(
                "System",
                "Account",
                &[json!(PAYER.to_string())],
                json!({"data":{"free":0}}),
            )?;
            let mut input = HOTKEY.0.to_vec();
            input.extend_from_slice(&CLAIMANT.0);
            state.seed_runtime_encoded("BetaBasketRuntimeApi_get_basket_payout", input, vec![255]);
            Ok(())
        })
        .await?;
        let built = build(&intent, &snapshot).await?;
        let preview = inspect(&intent, &snapshot, &snapshot, &client, &built.call, PAYER).await?;
        assert_eq!(preview.required_free, Some(Balance::from_rao(8_209_263)));
        assert_eq!(preview.available_free, Some(Balance::ZERO));
        assert_eq!(preview.estimated_fee, Some(Balance::from_rao(8_209_263)));
        assert_eq!(
            preview.blocks,
            ["free TAO (τ0.000000000) is below the reserved claim fee (τ0.008209263)"]
        );
        assert_eq!(preview.effects, [intent.summary()]);
        assert!(preview.facts.is_empty());
        assert!(preview.warnings.is_empty());
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn fee_fallback_retains_native_reserve_and_does_not_claim_exact_estimate()
    -> anyhow::Result<()> {
        let (_chain, client, snapshot) = fixture(|state| {
            state.set_result("account_nextIndex", json!("invalid"));
            Ok(())
        })
        .await?;
        let intent = single();
        let built = build(&intent, &snapshot).await?;
        let preview = inspect(&intent, &snapshot, &snapshot, &client, &built.call, PAYER).await?;
        assert_eq!(preview.required_free, Some(Balance::from_rao(8_209_263)));
        assert_eq!(
            preview.available_free,
            Some(Balance::from_rao(1_000_000_000))
        );
        assert!(preview.estimated_fee.is_none());
        assert!(preview.blocks.is_empty());
        assert_eq!(
            fact(&preview, "reserved"),
            Some("τ0.008209263 at inclusion")
        );
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn wrapped_claim_reads_dispatch_origin_but_checks_and_prices_fee_payer()
    -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture(|state| {
            state.seed_python(
                "System",
                "Account",
                &[json!(CLAIMANT.to_string())],
                json!({"data":{"free":u64::MAX}}),
            )?;
            state.seed_python(
                "System",
                "Account",
                &[json!(PAYER.to_string())],
                json!({"data":{"free":0}}),
            )?;
            Ok(())
        })
        .await?;
        let intent = ClaimRoot {
            subnets: vec![NetUid::ROOT],
        };
        let built = build(&intent, &snapshot).await?;
        let wrapped = proxy::proxy(
            DynValue::new(Value::Bytes(CLAIMANT.0.to_vec())),
            None,
            built.call,
        )
        .compose(&snapshot.block().runtime)?;
        let preview = inspect(&intent, &snapshot, &snapshot, &client, &wrapped, PAYER).await?;
        assert_eq!(preview.available_free, Some(Balance::ZERO));
        assert_eq!(preview.required_free, Some(Balance::from_rao(8_209_263)));
        assert!(preview.blocks[0].contains("below the reserved claim fee"));
        let request = chain
            .requests()
            .into_iter()
            .find(|request| {
                request.method == "state_call"
                    && request.params[0] == "TransactionPaymentApi_query_info"
            })
            .ok_or_else(|| anyhow::anyhow!("fee probe"))?;
        let bytes = hex::decode(
            request.params[1]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("probe hex"))?
                .trim_start_matches("0x"),
        )?;
        let decoded = snapshot
            .block()
            .runtime
            .core()
            .decode_extrinsic(&bytes[..bytes.len() - 4], true)?;
        assert_eq!(
            decoded.field("address").and_then(Value::as_str),
            Some(PAYER.to_string().as_str())
        );
        let expected = snapshot.block().runtime.core().decode_spec(
            &TypeSpec::Call,
            wrapped.as_bytes(),
            true,
        )?;
        assert_eq!(decoded.field("call"), Some(&expected));
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn dust_quote_preserves_exact_effects_facts_refunds_and_hook_outputs()
    -> anyhow::Result<()> {
        let (_chain, client, snapshot) = fixture(|state| {
            seed_admission(state, &[HOTKEY], &[HOTKEY], 3)?;
            seed_single_preview(state, 2_250_000, 2_200_000, 3, 2, 0)?;
            state.fee += 12_345;
            Ok(())
        })
        .await?;
        let intent = single();
        let built = build(&intent, &snapshot).await?;
        let preview = inspect(&intent, &snapshot, &snapshot, &client, &built.call, PAYER).await?;
        assert_eq!(preview.effects,[intent.summary(),"3 basket holdings (fee scales with ALPHA types)".into(),
            "reserved τ0.008221608 at inclusion; spent ~τ0.002353567; ~τ0.005868041 refunded".into(),
            "accrued τ0.002250000".into(),"1 dust row skipped: redeems τ0.002200000, ~τ0.000050000 stays in the fund".into(),
            "this claim loses money: spent fee ~τ0.002353567 exceeds redeemable accrued τ0.002200000".into()]);
        assert_eq!(
            preview.facts,
            [
                (
                    "holdings".into(),
                    "3 basket holdings (fee scales with ALPHA types)".into()
                ),
                ("reserved".into(), "τ0.008221608 at inclusion".into()),
                ("spent".into(), "~τ0.002353567".into()),
                ("refunded".into(), "~τ0.005868041".into()),
                ("accrued".into(), "τ0.002250000".into()),
                ("redeemable".into(), "τ0.002200000".into()),
                (
                    "dust".into(),
                    "1 rows skipped; ~τ0.000050000 stays in the fund".into()
                )
            ]
        );
        assert_eq!(
            preview.warnings,
            [
                "this claim loses money: spent fee ~τ0.002353567 exceeds redeemable accrued τ0.002200000; wait until more yield accrues"
            ]
        );
        let cx = PreflightCx {
            view: &snapshot,
            runtime: &snapshot.block().runtime,
            clock: &SystemClock,
            spec: ClaimRootWithHotkey::SPEC,
            client: &client,
            dispatch_origin: CLAIMANT,
            fee_payer: PAYER,
            call: &built.call,
        };
        assert_eq!(intent.effects(&cx).await?, preview.effects);
        assert_eq!(intent.warnings(&cx).await?, preview.warnings);
        assert_eq!(intent.blocks(&cx).await?, preview.blocks);
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn per_validator_threshold_and_absent_positions_are_distinct() -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture(|state| {
            seed_admission(state, &[HOTKEY, OTHER_HOTKEY], &[HOTKEY, OTHER_HOTKEY], 2)?;
            state.seed_runtime(
                "BetaBasketRuntimeApi",
                "get_root_basket_claim_previews",
                json!([
                    preview_json(HOTKEY, 700_000, 400_000, 2, 1, 0),
                    preview_json(OTHER_HOTKEY, 900_000, 880_000, 2, 1, 0)
                ]),
            )?;
            // Override the explicit unsupported response used by fixture().
            let info = rtao_core::ops::runtime_api::runtime_api_method(
                &state.runtime,
                "BetaBasketRuntimeApi",
                "get_root_basket_claim_previews",
            )?;
            let raw = rtao_testkit::fill_defaults(
                &state.runtime,
                info.output,
                json!([
                    preview_json(HOTKEY, 700_000, 400_000, 2, 1, 0),
                    preview_json(OTHER_HOTKEY, 900_000, 880_000, 2, 1, 0)
                ]),
            )?;
            let encoded = state
                .runtime
                .encode_spec(&TypeSpec::Id(info.output), &raw)?;
            state.seed_runtime_encoded(
                "BetaBasketRuntimeApi_get_root_basket_claim_previews",
                CLAIMANT.0.to_vec(),
                encoded,
            );
            Ok(())
        })
        .await?;
        let intent = ClaimRoot {
            subnets: vec![NetUid::ROOT],
        };
        let built = build(&intent, &snapshot).await?;
        let preview = inspect(&intent, &snapshot, &snapshot, &client, &built.call, PAYER).await?;
        assert_eq!(fact(&preview, "accrued"), Some("τ0.001600000"));
        assert_eq!(fact(&preview, "redeemable"), Some("τ0.000880000"));
        assert_eq!(
            fact(&preview, "dust"),
            Some("2 rows skipped; ~τ0.000320000 stays in the fund")
        );
        assert!(preview.warnings[0].starts_with("1 of 2 validators are individually below"));
        assert_eq!(fact(&preview, "spent"), Some("~τ0.002735525"));
        // Legacy fallback omits the other validator entirely; its two rows
        // incur no scan and do not count as a below-threshold entitlement.
        chain.mutate_state(|state| {
            state.seed_runtime_encoded(
                "BetaBasketRuntimeApi_get_root_basket_claim_previews",
                CLAIMANT.0.to_vec(),
                vec![255],
            )
        });
        let fallback = inspect(&intent, &snapshot, &snapshot, &client, &built.call, PAYER).await?;
        assert_eq!(fact(&fallback, "accrued"), Some("τ0.000900000"));
        assert_eq!(fact(&fallback, "spent"), Some("~τ0.002144071"));
        assert!(
            !fallback
                .warnings
                .iter()
                .any(|warning| warning.contains("threshold"))
        );
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn all_dust_and_consolidation_price_scan_and_redeem_work_separately() -> anyhow::Result<()>
    {
        for (accrued, redeemable, rows, sold, swept, holdings, spent) in [
            (8_000, 0, 2, 0, 0, 2, 1_891_544),
            (300_000, 300_000, 3, 3, 6, 8, 5_322_843),
            (2_700_000_000, 2_700_000_000, 3, 3, 6, 8, 6_671_879),
        ] {
            let (_chain, client, snapshot) = fixture(|state| {
                seed_admission(state, &[HOTKEY], &[HOTKEY], holdings)?;
                seed_single_preview(state, accrued, redeemable, rows, sold, swept)
            })
            .await?;
            let intent = single();
            let built = build(&intent, &snapshot).await?;
            let preview =
                inspect(&intent, &snapshot, &snapshot, &client, &built.call, PAYER).await?;
            assert_eq!(
                fact(&preview, "spent"),
                Some(format!("~{}", money(spent)).as_str())
            );
            assert_eq!(
                fact(&preview, "holdings"),
                Some(format!("{holdings} basket holdings (fee scales with ALPHA types)").as_str())
            );
            if redeemable < 500_000 {
                assert_eq!(preview.warnings.len(), 1);
                assert!(
                    preview.warnings[0].ends_with("the claim pays a scan fee and realizes nothing")
                );
            } else {
                assert!(preview.warnings.is_empty());
            }
            client.close().await;
        }
        Ok(())
    }

    #[tokio::test]
    async fn native_total_overflow_drops_optional_quote_without_dropping_reserve()
    -> anyhow::Result<()> {
        let (_chain, client, snapshot) = fixture(|state| {
            seed_admission(state, &[HOTKEY, OTHER_HOTKEY], &[HOTKEY, OTHER_HOTKEY], 1)?;
            state.seed_runtime(
                "BetaBasketRuntimeApi",
                "get_root_basket_positions",
                json!([
                    [HOTKEY.to_string(), 1, u64::MAX],
                    [OTHER_HOTKEY.to_string(), 1, 1]
                ]),
            )?;
            Ok(())
        })
        .await?;
        let intent = ClaimRoot {
            subnets: vec![NetUid::ROOT],
        };
        let built = build(&intent, &snapshot).await?;
        let preview = inspect(&intent, &snapshot, &snapshot, &client, &built.call, PAYER).await?;
        assert_eq!(preview.effects, [intent.summary()]);
        assert!(preview.facts.is_empty());
        assert_eq!(preview.required_free, Some(Balance::from_rao(8_209_263)));
        assert_eq!(
            preview.available_free,
            Some(Balance::from_rao(1_000_000_000))
        );
        client.close().await;
        Ok(())
    }
}
