// Derived from RaoFoundation/subtensor sdk/python/bittensor/intents/registration.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Additional source at the same commit/license: sdk/python/bittensor/intents/_root_claim_fee.py.
// Test source at the same commit/license: sdk/python/tests/unit/test_root_claim_fee.py.
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
    use rtao_runtime::{CallBytes, calls::proxy};
    use rtao_signer::{Coldkey, KeySource, KeyView, Signer, SignerError, SignerRole};
    use rtao_testkit::{FakeChain, FakeState, RecordingView, Reply, rpc::RpcClient};
    use rtao_tx::{
        Executor, IntentInterface, OriginView, PlanOptions, ShieldOptions, SystemClock, TxError,
    };
    use serde_json::{Value as Json, json};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    const CLAIMANT: AccountId = AccountId([7; 32]);
    const PAYER: AccountId = AccountId([9; 32]);
    const HOTKEY: AccountId = AccountId([8; 32]);
    const OTHER_HOTKEY: AccountId = AccountId([10; 32]);

    #[derive(serde::Deserialize)]
    struct Extracted<T> {
        value: T,
    }

    #[derive(serde::Deserialize)]
    struct FixtureData {
        format: u64,
        constants: BTreeMap<String, Extracted<u64>>,
        extension_ref_time: Extracted<u64>,
        node_quotes: Extracted<Vec<NodeQuote>>,
        admission_hard_stop: Extracted<Vec<AdmissionCase>>,
        admission_inclusive: Extracted<Vec<AdmissionCase>>,
        payout_failures: BTreeMap<String, Extracted<Vec<PayoutFailure>>>,
        network_population: Extracted<NetworkPopulation>,
        shielded_heavy: Extracted<ShieldedHeavy>,
        seed_cases: BTreeMap<String, Extracted<SeedCase>>,
        preview_cases: BTreeMap<String, Extracted<Vec<PreviewCase>>>,
    }

    #[derive(serde::Deserialize)]
    struct NodeQuote {
        name: String,
        weight: u128,
        weight_fee: u128,
        limits: Vec<u64>,
    }

    #[derive(serde::Deserialize)]
    struct AdmissionCase {
        hotkeys: u16,
        holdings: usize,
        networks: u16,
        reason: Option<String>,
        allowed: Option<bool>,
    }

    #[derive(Clone, serde::Deserialize)]
    #[serde(tag = "kind", rename_all = "lowercase")]
    enum PayoutFailure {
        None,
        Error {
            #[serde(rename = "type")]
            exception_type: String,
            message: String,
        },
    }

    #[derive(serde::Deserialize)]
    struct NetworkPopulation {
        hotkeys: u16,
        added: u16,
        absent: u16,
    }

    #[derive(serde::Deserialize)]
    struct ShieldedHeavy {
        hotkeys: u16,
        holdings: usize,
        payout: u64,
        networks: u16,
    }

    #[derive(serde::Deserialize)]
    struct SeedCase {
        hotkeys: Vec<String>,
        payouts: BTreeMap<String, u64>,
        holdings: BTreeMap<String, usize>,
        networks: u16,
    }

    #[derive(serde::Deserialize)]
    struct PreviewCase {
        hotkey: String,
        accrued: u64,
        redeemable: u64,
        rows: u32,
        sold: u32,
        swept: u32,
        flushed: u32,
    }

    fn fixture_data() -> anyhow::Result<FixtureData> {
        // Hosted extraction must adopt this artifact before execution. Reading
        // the committed test input at runtime keeps a generation-only checkout
        // compilable while the new file is awaiting that first hosted adoption.
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../vendor/upstream/extracted/root_claim_fixtures.json");
        let source = std::fs::read_to_string(&path).map_err(|error| {
            anyhow::anyhow!(
                "read {}: {error}; run hosted fixture extraction",
                path.display()
            )
        })?;
        let data: FixtureData = serde_json::from_str(&source)?;
        anyhow::ensure!(data.format == 1, "unsupported root-claim fixture format");
        Ok(data)
    }

    fn extracted<'a, T>(
        records: &'a BTreeMap<String, Extracted<T>>,
        name: &str,
    ) -> anyhow::Result<&'a T> {
        records
            .get(name)
            .map(|record| &record.value)
            .ok_or_else(|| anyhow::anyhow!("missing extracted root-claim record {name}"))
    }

    fn symbolic_hotkey(name: &str) -> anyhow::Result<AccountId> {
        // The extracted dev-key names retain source identity; the native fake
        // maps those names to its own deterministic public accounts.
        match name {
            "ALICE_HOT" => Ok(HOTKEY),
            "BOB_HOT" => Ok(OTHER_HOTKEY),
            _ => anyhow::bail!("unsupported extracted hotkey {name}"),
        }
    }

    fn indexed_hotkeys(count: u16) -> Vec<AccountId> {
        (0..count)
            .map(|index| {
                let mut bytes = [0; 32];
                bytes[..2].copy_from_slice(&index.to_le_bytes());
                bytes[2] = 17;
                AccountId(bytes)
            })
            .collect()
    }

    fn seed_extracted_case(state: &mut FakeState, case: &SeedCase) -> anyhow::Result<()> {
        let keys = case
            .hotkeys
            .iter()
            .map(|name| symbolic_hotkey(name))
            .collect::<anyhow::Result<Vec<_>>>()?;
        seed_admission(state, &keys, &keys, 0)?;
        let mut positions = Vec::new();
        for name in &case.hotkeys {
            let hotkey = symbolic_hotkey(name)?;
            let holdings = case
                .holdings
                .get(name)
                .ok_or_else(|| anyhow::anyhow!("missing extracted holdings for {name}"))?;
            seed_basket_for(state, hotkey, *holdings)?;
            if let Some(&payout) = case.payouts.get(name) {
                if payout != 0 {
                    positions.push(json!([hotkey.to_string(), 1, payout]));
                }
                let info = rtao_core::ops::runtime_api::runtime_api_method(
                    &state.runtime,
                    "BetaBasketRuntimeApi",
                    "get_basket_payout",
                )?;
                let raw =
                    rtao_testkit::fakes::fill_defaults(&state.runtime, info.output, json!(payout))?;
                let encoded = state
                    .runtime
                    .encode_spec(&TypeSpec::Id(info.output), &raw)?;
                state.seed_runtime_encoded(
                    "BetaBasketRuntimeApi_get_basket_payout",
                    [hotkey.0.as_slice(), CLAIMANT.0.as_slice()].concat(),
                    encoded,
                );
            }
        }
        state.seed_runtime(
            "BetaBasketRuntimeApi",
            "get_root_basket_positions",
            json!(positions),
        )?;
        seed_networks(state, case.networks, 0)?;
        Ok(())
    }

    fn extracted_preview(case: &PreviewCase) -> anyhow::Result<Json> {
        let mut preview = preview_json(
            symbolic_hotkey(&case.hotkey)?,
            case.accrued,
            case.redeemable,
            case.rows,
            case.sold,
            case.swept,
        );
        preview["flushed_credits"] = json!(case.flushed);
        Ok(preview)
    }

    fn seed_extracted_previews(
        state: &mut FakeState,
        cases: &[PreviewCase],
        coldkey_wide: bool,
    ) -> anyhow::Result<()> {
        let previews = cases
            .iter()
            .map(extracted_preview)
            .collect::<anyhow::Result<Vec<_>>>()?;
        if coldkey_wide {
            seed_coldkey_previews(state, json!(previews))
        } else {
            anyhow::ensure!(previews.len() == 1, "expected one extracted basket preview");
            state.seed_runtime(
                "BetaBasketRuntimeApi",
                "get_basket_claim_preview",
                previews
                    .into_iter()
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("missing preview"))?,
            )?;
            Ok(())
        }
    }

    #[derive(Default)]
    struct PublicOnlyKeys {
        acquisitions: AtomicUsize,
    }

    impl KeyView for PublicOnlyKeys {
        fn coldkey_address(&self) -> Result<AccountId, SignerError> {
            Ok(CLAIMANT)
        }
        fn hotkey_address(&self) -> Result<AccountId, SignerError> {
            Err(SignerError::KeyUnavailable(SignerRole::Hotkey))
        }
        fn hotkey_public_key(&self) -> Result<[u8; 32], SignerError> {
            Err(SignerError::KeyUnavailable(SignerRole::Hotkey))
        }
    }

    impl KeySource<Coldkey> for PublicOnlyKeys {
        fn address(&self) -> Result<AccountId, SignerError> {
            Ok(CLAIMANT)
        }
        async fn signer(&self) -> Result<Arc<dyn Signer>, SignerError> {
            self.acquisitions.fetch_add(1, Ordering::SeqCst);
            Err(SignerError::RawSigningUnsupported)
        }
    }

    fn seed_basket_for(
        state: &mut FakeState,
        hotkey: AccountId,
        holdings: usize,
    ) -> anyhow::Result<()> {
        let info = rtao_core::ops::runtime_api::runtime_api_method(
            &state.runtime,
            "BetaBasketRuntimeApi",
            "get_validator_basket",
        )?;
        let raw = rtao_testkit::fakes::fill_defaults(
            &state.runtime,
            info.output,
            json!((0..holdings).map(|_| json!([1, 1, 1])).collect::<Vec<_>>()),
        )?;
        let encoded = state
            .runtime
            .encode_spec(&TypeSpec::Id(info.output), &raw)?;
        state.seed_runtime_encoded(
            "BetaBasketRuntimeApi_get_validator_basket",
            hotkey.0.to_vec(),
            encoded,
        );
        Ok(())
    }

    fn seed_coldkey_previews(state: &mut FakeState, previews: Json) -> anyhow::Result<()> {
        let info = rtao_core::ops::runtime_api::runtime_api_method(
            &state.runtime,
            "BetaBasketRuntimeApi",
            "get_root_basket_claim_previews",
        )?;
        let raw = rtao_testkit::fakes::fill_defaults(&state.runtime, info.output, previews)?;
        let encoded = state
            .runtime
            .encode_spec(&TypeSpec::Id(info.output), &raw)?;
        state.seed_runtime_encoded(
            "BetaBasketRuntimeApi_get_root_basket_claim_previews",
            CLAIMANT.0.to_vec(),
            encoded,
        );
        Ok(())
    }

    fn seed_networks(state: &mut FakeState, added: u16, absent: u16) -> anyhow::Result<()> {
        for netuid in 0..added + absent {
            state.seed(
                &storage::subtensor_module::NETWORKS_ADDED,
                (NetUid(netuid),),
                netuid < added,
            )?;
        }
        Ok(())
    }

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

    async fn coldkey_quote(
        snapshot: &Snapshot,
        client: &Client,
        call: &CallBytes,
    ) -> anyhow::Result<Quote> {
        let cx = PreflightCx {
            view: snapshot,
            runtime: &snapshot.block().runtime,
            clock: &SystemClock,
            spec: ClaimRoot::SPEC,
            client,
            dispatch_origin: CLAIMANT,
            fee_payer: PAYER,
            call,
        };
        let admitted = admission(snapshot, CLAIMANT, None).await?;
        let reserved = reserve(&cx, admitted.limit).await?;
        Ok(quote(&cx, &admitted, &reserved, true).await?)
    }

    #[test]
    fn weight_model_matches_recorded_spec_467_node_quotes_and_rounding() -> anyhow::Result<()> {
        // pyt: unit/test_root_claim_fee.py::test_reserved_fallback_charges_the_fee_allowance_not_the_envelope
        // pyt: unit/test_root_claim_fee.py::test_discount_model_matches_the_467_release_node
        // pyt: unit/test_root_claim_fee.py::test_charged_allowance_is_the_same_for_both_claim_paths
        // pyt: unit/test_root_claim_fee.py::test_spent_caps_at_the_fee_allowance
        // pyt: unit/test_root_claim_fee.py::test_single_hotkey_spent_caps_at_the_same_allowance
        // pyt: unit/test_root_claim_fee.py::test_spent_keeps_non_weight_base_fee
        // pyt: unit/test_root_claim_fee.py::test_scan_only_uses_scan_ref_time
        let data = fixture_data()?;
        let constant = |name| extracted(&data.constants, name).copied();
        assert_eq!(MAX_WORK, constant("_MAX_ROOT_CLAIM_WORK")?);
        assert_eq!(MAX_HOTKEY_WORK, constant("_MAX_ROOT_CLAIM_HOTKEY_WORK")?);
        assert_eq!(FEE_ALLOWANCE, constant("_ROOT_CLAIM_FEE_ALLOWANCE")?);
        assert_eq!(DEFAULT_THRESHOLD_RAO, constant("_DEFAULT_THRESHOLD_RAO")?);
        let read = u128::from(constant("_ROCKSDB_READ_PS")?);
        let write = u128::from(constant("_ROCKSDB_WRITE_PS")?);
        for units in [0u64, 1, 32, MAX_WORK] {
            let count = u128::from(units);
            assert_eq!(
                claim_root_ref_time(units),
                567_441_000
                    + 437_318_379 * count
                    + (23 + 22 * count) * read
                    + (18 + 16 * count) * write
            );
            assert_eq!(
                claim_root_scan_ref_time(units),
                309_208_000 + 263_604_241 * count + (6 + 21 * count) * read
            );
        }
        let parts = u128::from(constant("_WEIGHT_FEE_PARTS")?);
        let perbill = u128::from(constant("_PERBILL")?);
        for ref_time in [2_000, 6_000, 6_001, 1_000_000_001] {
            let scaled = ref_time * parts;
            assert_eq!(
                weight_fee_rao(ref_time),
                scaled / perbill + u128::from((scaled % perbill) * 2 > perbill)
            );
        }
        let quotes = constant("_FLUSH_BOUND_QUOTES")?;
        let rows = constant("_FLUSH_BOUND_ROWS")?;
        assert_eq!(quotes, 10 * constant("_MAX_BASKET_ROWS")?);
        assert_eq!(rows, 2 * constant("_MAX_BASKET_ROWS")?);
        let extension = u128::from(data.extension_ref_time.value);
        assert_eq!(extension, u128::from(constant("_EXTENSION_FOLD_REF_TIME")?));
        let cap = u128::from(constant("_ROOT_CLAIM_CAP_459_REF_TIME")?) - extension;
        // Consume every recorded node row, including its name, limits and fee.
        // The batch's outer weight includes work beyond the inner declarations.
        for node in &data.node_quotes.value {
            anyhow::ensure!(!node.limits.is_empty(), "node quote has no claim limits");
            let mut discount = 0;
            for &limit in &node.limits {
                let declared = claim_ref_time(limit, quotes, rows);
                let units = limit.min(FEE_ALLOWANCE);
                assert_eq!(
                    charged_ref_time(limit),
                    claim_ref_time(units, 6 * units, units).min(cap)
                );
                discount += declared - charged_ref_time(limit);
                assert_eq!(approximate_reserve(limit)?.rao(), 8_209_263);
            }
            assert_eq!(
                weight_fee_rao(node.weight - discount),
                node.weight_fee,
                "{}",
                node.name
            );
            if let [limit] = node.limits.as_slice() {
                assert_eq!(
                    claim_ref_time(*limit, quotes, rows) + extension,
                    node.weight,
                    "{}",
                    node.name
                );
                assert_eq!(
                    charged_ref_time(*limit) + extension,
                    node.weight - discount,
                    "{}",
                    node.name
                );
            }
        }
        assert_eq!(weight_fee_rao(2_000), 0);
        assert_eq!(weight_fee_rao(6_000), 1);
        assert_eq!(weight_fee_rao(6_001), 2);
        assert_eq!(charged_ref_time(2), claim_ref_time(2, 12, 2));
        assert_eq!(charged_ref_time(129), claim_ref_time(4, 24, 4));
        assert_eq!(charged_ref_time(129), charged_ref_time(256));
        assert_eq!(
            spent_fee(Balance::from_rao(8_209_263 + 12_345), 1, 1, 0, 0, 129).rao(),
            1_509_587
        );
        for limit in [129, 256] {
            assert_eq!(
                spent_fee(Balance::from_rao(8_209_263), 1, 32, 0, 0, limit).rao(),
                8_209_263
            );
        }
        let scan_only = spent_fee(Balance::from_rao(8_209_263), 1, 0, 32, 0, 256);
        assert_eq!(scan_only.rao(), 7_806_076);
        assert!(scan_only.rao() < 8_209_263);
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
        // pyt: unit/test_root_claim_fee.py::test_coldkey_wide_admission_keeps_unstaked_basket_claimant
        let data = fixture_data()?;
        let case = extracted(
            &data.seed_cases,
            "test_coldkey_wide_admission_keeps_unstaked_basket_claimant",
        )?;
        let (_chain, client, snapshot) = fixture(|state| {
            seed_extracted_case(state, case)?;
            state.seed_runtime(
                "StakeInfoRuntimeApi",
                "get_stake_info_for_coldkey",
                json!([]),
            )?;
            state.seed(
                &storage::subtensor_module::BASKET_CLAIMED,
                (HOTKEY, CLAIMANT),
                -1,
            )?;
            Ok(())
        })
        .await?;
        assert_eq!(
            admission(&snapshot, CLAIMANT, None).await?.hotkeys,
            [HOTKEY]
        );
        client.close().await;
        // Additional native ordering/duplicate coverage surrounds the exact
        // one-validator source case with other staking relationships.
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
                // Map scans observe both the key page and its value batch.
                "NetworksAdded",
                "NetworksAdded"
            ]
        );
        assert_eq!(
            recording.block_hashes(),
            BTreeSet::from([snapshot.block().hash])
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
    async fn coldkey_admission_covers_selected_work_and_network_population_boundaries()
    -> anyhow::Result<()> {
        // pyt: unit/test_root_claim_fee.py::test_root_claim_too_heavy_is_a_hard_stop
        // pyt: unit/test_root_claim_fee.py::test_admission_limit_is_inclusive_at_256
        // pyt: unit/test_root_claim_fee.py::test_network_count_does_not_affect_admission
        let data = fixture_data()?;
        let mut cases = Vec::new();
        // Keep both source tables intact: the rejecting 1/256 row occurs in
        // each table and is exercised twice, with its original expectations.
        for case in &data.admission_hard_stop.value {
            let reason = case
                .reason
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("missing hard-stop reason"))?;
            cases.push((
                case.hotkeys,
                case.holdings,
                case.networks,
                0,
                false,
                Some(reason),
            ));
        }
        for case in &data.admission_inclusive.value {
            let allowed = case
                .allowed
                .ok_or_else(|| anyhow::anyhow!("missing admission expectation"))?;
            cases.push((case.hotkeys, case.holdings, case.networks, 0, allowed, None));
        }
        let population = &data.network_population.value;
        cases.push((
            population.hotkeys,
            0,
            population.added,
            population.absent,
            true,
            None,
        ));
        for (hotkeys, holdings, added, absent, allowed, reason) in cases {
            let keys = indexed_hotkeys(hotkeys);
            let (chain, client, snapshot) = fixture(|state| {
                seed_admission(state, &keys, &keys, holdings)?;
                seed_networks(state, added, absent)
            })
            .await?;
            let admitted = admission(&snapshot, CLAIMANT, None).await?;
            assert_eq!(admitted.hotkeys, keys);
            assert_eq!(admitted.holdings()?, u64::from(hotkeys) * holdings as u64);
            assert_eq!(admitted.selection_scans, u64::from(hotkeys));
            let blocks = admitted.blocks()?;
            assert_eq!(
                blocks.is_empty(),
                allowed,
                "{hotkeys}/{holdings}/{added}/{absent}"
            );
            if let Some(reason) = reason {
                assert!(
                    blocks.iter().any(|block| block.contains(reason)),
                    "{reason}: {blocks:?}"
                );
            }
            if hotkeys == 257 {
                assert_eq!(
                    blocks,
                    [
                        "root claim exceeds the 256-unit admission limit (257 root hotkeys + 0 basket holdings = 257; 257 staking-hotkey relationships to classify); claim one validator at a time with claim_root_with_hotkey"
                    ]
                );
            }
            if !allowed {
                let intent = ClaimRoot {
                    subnets: vec![NetUid::ROOT],
                };
                let built = build(&intent, &snapshot).await?;
                let preview =
                    inspect(&intent, &snapshot, &snapshot, &client, &built.call, PAYER).await?;
                assert_eq!(preview.blocks, blocks);
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
        Ok(())
    }

    #[tokio::test]
    async fn coldkey_admission_excludes_positive_nonroot_stake_and_its_heavy_basket()
    -> anyhow::Result<()> {
        // pyt: unit/test_root_claim_fee.py::test_coldkey_wide_admission_ignores_non_root_hotkeys
        let data = fixture_data()?;
        let case = extracted(
            &data.seed_cases,
            "test_coldkey_wide_admission_ignores_non_root_hotkeys",
        )?;
        let (_chain, client, snapshot) = fixture(|state| {
            seed_extracted_case(state, case)?;
            state.seed_runtime("StakeInfoRuntimeApi", "get_stake_info_for_coldkey", json!([
                {"hotkey":HOTKEY.to_string(), "coldkey":CLAIMANT.to_string(), "netuid":0, "stake":1},
                {"hotkey":OTHER_HOTKEY.to_string(), "coldkey":CLAIMANT.to_string(), "netuid":7, "stake":1}
            ]))?;
            Ok(())
        }).await?;
        let recording = RecordingView::new(snapshot.clone());
        let admitted = admission(&recording, CLAIMANT, None).await?;
        assert_eq!(admitted.hotkeys, [HOTKEY]);
        assert_eq!(admitted.holding_counts, [1]);
        assert_eq!(admitted.selection_scans, 2);
        assert!(admitted.blocks()?.is_empty());
        assert_eq!(
            recording
                .touches()
                .iter()
                .filter(|touch| touch.item == "get_validator_basket")
                .count(),
            1
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
    async fn real_shielded_root_claim_refuses_mandatory_limits_before_private_acquisition()
    -> anyhow::Result<()> {
        // pyt: unit/test_root_claim_fee.py::test_shielded_root_claim_refuses_too_heavy_work_before_signing
        // pyt: unit/test_root_claim_fee.py::test_shielded_claim_reserves_free_tao_for_inner_and_carrier
        // pyt: unit/test_root_claim_fee.py::test_admission_hard_stop_survives_unavailable_payout_preview
        // pyt: unit/test_root_claim_fee.py::test_reserve_hard_stop_survives_unavailable_payout_preview
        // pyt: unit/test_root_claim_fee.py::test_carrier_hard_stop_survives_unavailable_payout_preview
        let data = fixture_data()?;
        let heavy_reason = data
            .admission_hard_stop
            .value
            .iter()
            .find(|case| case.hotkeys > 1)
            .ok_or_else(|| anyhow::anyhow!("missing multi-hotkey admission case"))?;
        // Both source carrier cases use the same seed; preserve that separate
        // case identity instead of silently leaving one extracted row unused.
        let ordinary_carrier = extracted(
            &data.seed_cases,
            "test_shielded_claim_reserves_free_tao_for_inner_and_carrier",
        )?;
        let missing_carrier = extracted(
            &data.seed_cases,
            "test_carrier_hard_stop_survives_unavailable_payout_preview",
        )?;
        assert_eq!(
            (
                &ordinary_carrier.hotkeys,
                &ordinary_carrier.payouts,
                &ordinary_carrier.holdings,
                ordinary_carrier.networks
            ),
            (
                &missing_carrier.hotkeys,
                &missing_carrier.payouts,
                &missing_carrier.holdings,
                missing_carrier.networks
            )
        );
        for condition in ["heavy_basket", "heavy_hotkeys", "reserve", "carrier"] {
            let policy_seed = match condition {
                "heavy_basket" => Some(extracted(
                    &data.seed_cases,
                    "test_shielded_root_claim_refuses_too_heavy_work_before_signing",
                )?),
                "reserve" => Some(extracted(
                    &data.seed_cases,
                    "test_reserve_hard_stop_survives_unavailable_payout_preview",
                )?),
                "carrier" => Some(extracted(
                    &data.seed_cases,
                    "test_shielded_claim_reserves_free_tao_for_inner_and_carrier",
                )?),
                _ => None,
            };
            let mut payouts = if condition == "heavy_hotkeys" {
                Vec::new()
            } else {
                vec![None]
            };
            if condition != "heavy_basket" {
                let function = if condition == "heavy_hotkeys" {
                    "test_admission_hard_stop_survives_unavailable_payout_preview"
                } else {
                    "test_reserve_hard_stop_survives_unavailable_payout_preview"
                };
                payouts.extend(
                    extracted(&data.payout_failures, function)?
                        .iter()
                        .cloned()
                        .map(Some),
                );
            }
            for payout in payouts {
                let payout_name = match &payout {
                    None => "available",
                    Some(PayoutFailure::None) => "none",
                    Some(PayoutFailure::Error { .. }) => "error",
                };
                let (chain, client, _snapshot) = fixture(|state| {
                    if let Some(seed) = policy_seed {
                        seed_extracted_case(state, seed)?;
                    }
                    if condition == "heavy_hotkeys" {
                        // Exact source failure setup: 257 selected hotkeys,
                        // no holdings, 16 networks, before optional yield.
                        let heavy = &data.shielded_heavy.value;
                        let keys = indexed_hotkeys(heavy.hotkeys);
                        seed_admission(state, &keys, &keys, heavy.holdings)?;
                        state.seed_runtime(
                            "BetaBasketRuntimeApi",
                            "get_root_basket_positions",
                            json!(
                                keys.iter()
                                    .map(|hotkey| json!([hotkey.to_string(), 1, heavy.payout]))
                                    .collect::<Vec<_>>()
                            ),
                        )?;
                        seed_networks(state, heavy.networks, 0)?;
                    }
                    let free = match condition {
                        "reserve" => 0,
                        "carrier" => 8_209_263,
                        _ => 1_000_000_000,
                    };
                    state.seed_python(
                        "System",
                        "Account",
                        &[json!(CLAIMANT.to_string())],
                        json!({"data":{"free":free}}),
                    )?;
                    if let Some(failure) = payout.clone() {
                        if let PayoutFailure::Error { exception_type, .. } = &failure {
                            anyhow::ensure!(
                                exception_type == "RuntimeError",
                                "unsupported extracted exception {exception_type}"
                            );
                        }
                        state.set_handler("state_call", move |state, request| {
                            if request.params[0] == "BetaBasketRuntimeApi_get_root_basket_positions"
                            {
                                match &failure {
                                    PayoutFailure::None => Reply::result(Json::Null),
                                    PayoutFailure::Error { message, .. } => {
                                        Reply::error(-32603, message.clone(), None)
                                    }
                                }
                            } else {
                                state.dispatch(request)
                            }
                        });
                    }
                    Ok(())
                })
                .await?;
                let intent = ClaimRoot {
                    subnets: vec![NetUid::ROOT],
                };
                let keys = PublicOnlyKeys::default();
                let executor = Executor::new(client.clone());
                let expected = match condition {
                    "heavy_basket" | "heavy_hotkeys" => "exceeds the 256-unit admission limit",
                    "reserve" => "below the reserved claim fee",
                    _ => "reserve plus the MEV-shield carrier fee",
                };
                // Public planning also preserves admission and reserve blocks.
                // Carrier affordability belongs to shielded execution itself.
                if condition != "carrier" {
                    let plan = executor
                        .plan(&intent, &keys, &PlanOptions::default())
                        .await?;
                    assert!(
                        plan.violations
                            .iter()
                            .any(|violation| violation.contains(expected)),
                        "{condition}/{payout_name}"
                    );
                    if condition == "heavy_hotkeys" {
                        let reason = heavy_reason
                            .reason
                            .as_deref()
                            .ok_or_else(|| anyhow::anyhow!("missing admission reason"))?;
                        assert!(
                            plan.violations
                                .iter()
                                .any(|violation| violation.contains(reason))
                        );
                    }
                }
                let refusal = executor
                    .submit_shielded(&intent, &keys, &ShieldOptions::default())
                    .await;
                let Err(TxError::Policy(error)) = refusal else {
                    anyhow::bail!(
                        "expected policy refusal for {condition}/{payout_name}: {refusal:?}"
                    );
                };
                assert!(
                    error
                        .violations
                        .iter()
                        .any(|violation| violation.contains(expected)),
                    "{condition}/{payout_name}: {error}"
                );
                assert_eq!(keys.acquisitions.load(Ordering::SeqCst), 0);
                assert!(chain.mutate_state(|state| state.submissions.is_empty()));
                assert!(
                    !chain
                        .requests()
                        .iter()
                        .any(|request| request.method.starts_with("author_"))
                );
                if condition.starts_with("heavy") {
                    assert!(!chain.requests().iter().any(|request| {
                        request.params[0] == "BetaBasketRuntimeApi_get_root_basket_positions"
                            || request.params[0]
                                == "BetaBasketRuntimeApi_get_root_basket_claim_previews"
                    }));
                }
                client.close().await;
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn fee_fallback_retains_native_reserve_and_does_not_claim_exact_estimate()
    -> anyhow::Result<()> {
        // pyt: unit/test_root_claim_fee.py::test_reserved_fallback_is_max_root_claim_work
        // pyt: unit/test_root_claim_fee.py::test_reserved_fallback_uses_fixed_envelope
        // pyt: unit/test_root_claim_fee.py::test_single_hotkey_reserved_fallback_uses_declared_work
        // Native reserve receives an already-built call instead of Python's
        // compose callback. Fail the actual fee RPC for both native envelopes.
        for coldkey_wide in [false, true] {
            let (_chain, client, snapshot) = fixture(|state| {
                state.set_handler("state_call", |state, request| {
                    if request.params[0] == "TransactionPaymentApi_query_info" {
                        Reply::error(-32603, "fee probe unavailable", None)
                    } else {
                        state.dispatch(request)
                    }
                });
                Ok(())
            })
            .await?;
            let preview = if coldkey_wide {
                let intent = ClaimRoot {
                    subnets: vec![NetUid::ROOT],
                };
                let built = build(&intent, &snapshot).await?;
                inspect(&intent, &snapshot, &snapshot, &client, &built.call, PAYER).await?
            } else {
                let intent = single();
                let built = build(&intent, &snapshot).await?;
                inspect(&intent, &snapshot, &snapshot, &client, &built.call, PAYER).await?
            };
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
        }
        Ok(())
    }

    #[tokio::test]
    async fn wrapped_claim_reads_dispatch_origin_but_checks_and_prices_fee_payer()
    -> anyhow::Result<()> {
        // pyt: unit/test_root_claim_fee.py::test_proxy_claim_reads_dispatch_state_checks_delegate_and_prices_wrapper
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
        assert_eq!(fact(&preview, "accrued"), Some("τ0.000900000"));
        assert_eq!(
            fact(&preview, "holdings"),
            Some("1 basket holding (fee scales with ALPHA types)")
        );
        let claimant_reads = chain
            .requests()
            .into_iter()
            .filter(|request| {
                request.method == "state_call"
                    && request.params[0] == "StakeInfoRuntimeApi_get_stake_info_for_coldkey"
            })
            .collect::<Vec<_>>();
        assert!(!claimant_reads.is_empty());
        assert!(
            claimant_reads
                .iter()
                .all(|request| request.params[1] == format!("0x{}", hex::encode(CLAIMANT.0)))
        );
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
        // pyt: unit/test_root_claim_fee.py::test_single_hotkey_quote_uses_the_dust_aware_preview_on_mixed_dust
        let data = fixture_data()?;
        let function = "test_single_hotkey_quote_uses_the_dust_aware_preview_on_mixed_dust";
        let seed = extracted(&data.seed_cases, function)?;
        let previews = extracted(&data.preview_cases, function)?;
        let (_chain, client, snapshot) = fixture(|state| {
            seed_extracted_case(state, seed)?;
            seed_extracted_previews(state, previews, false)?;
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
        // pyt: unit/test_root_claim_fee.py::test_coldkey_quote_applies_the_threshold_to_the_dust_aware_payout
        let data = fixture_data()?;
        let function = "test_coldkey_quote_applies_the_threshold_to_the_dust_aware_payout";
        let seed = extracted(&data.seed_cases, function)?;
        let previews = extracted(&data.preview_cases, function)?;
        let (chain, client, snapshot) = fixture(|state| {
            seed_extracted_case(state, seed)?;
            seed_extracted_previews(state, previews, true)
        })
        .await?;
        let intent = ClaimRoot {
            subnets: vec![NetUid::ROOT],
        };
        let built = build(&intent, &snapshot).await?;
        let preview = inspect(&intent, &snapshot, &snapshot, &client, &built.call, PAYER).await?;
        let raw = coldkey_quote(&snapshot, &client, &built.call).await?;
        assert_eq!(
            (
                raw.accrued,
                raw.redeemable,
                raw.eligible_hotkeys,
                raw.below_threshold_hotkeys,
                raw.dust_rows,
                raw.forfeited
            ),
            (1_600_000, 880_000, 1, 1, 4, 320_000)
        );
        assert_eq!(fact(&preview, "accrued"), Some("τ0.001600000"));
        assert_eq!(fact(&preview, "redeemable"), Some("τ0.000880000"));
        assert_eq!(
            fact(&preview, "dust"),
            Some("4 rows skipped; ~τ0.000320000 stays in the fund")
        );
        assert!(preview.warnings[0].starts_with("1 of 2 validators are individually below"));
        assert_eq!(fact(&preview, "spent"), Some("~τ0.003129827"));
        // Legacy fallback omits the other validator entirely; its three rows
        // incur no scan and do not count as a below-threshold entitlement.
        chain.mutate_state(|state| {
            state.seed_runtime_encoded(
                "BetaBasketRuntimeApi_get_root_basket_claim_previews",
                CLAIMANT.0.to_vec(),
                vec![255],
            );
            state.seed_runtime(
                "BetaBasketRuntimeApi",
                "get_root_basket_positions",
                json!([[HOTKEY.to_string(), 1, 900_000]]),
            )
        })?;
        let fallback = inspect(&intent, &snapshot, &snapshot, &client, &built.call, PAYER).await?;
        assert_eq!(fact(&fallback, "accrued"), Some("τ0.000900000"));
        assert_eq!(fact(&fallback, "spent"), Some("~τ0.003437731"));
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
    async fn coldkey_threshold_is_per_present_validator_and_absence_does_not_scan()
    -> anyhow::Result<()> {
        // pyt: unit/test_root_claim_fee.py::test_coldkey_threshold_is_applied_per_hotkey_not_to_aggregate
        // pyt: unit/test_root_claim_fee.py::test_coldkey_quote_separates_eligible_and_below_threshold_hotkeys
        // pyt: unit/test_root_claim_fee.py::test_coldkey_quote_does_not_scan_validator_without_owed_shares
        let data = fixture_data()?;
        for (function, accrued, redeemable, eligible, below, spent) in [
            (
                "test_coldkey_threshold_is_applied_per_hotkey_not_to_aggregate",
                600_000u64,
                0,
                0,
                2,
                3_129_827,
            ),
            (
                "test_coldkey_quote_separates_eligible_and_below_threshold_hotkeys",
                1_200_000,
                900_000,
                1,
                1,
                2_735_525,
            ),
            (
                "test_coldkey_quote_does_not_scan_validator_without_owed_shares",
                300_000,
                0,
                0,
                1,
                2_538_374,
            ),
        ] {
            let seed = extracted(&data.seed_cases, function)?;
            let (_chain, client, snapshot) =
                fixture(|state| seed_extracted_case(state, seed)).await?;
            let intent = ClaimRoot {
                subnets: vec![NetUid::ROOT],
            };
            let built = build(&intent, &snapshot).await?;
            let raw = coldkey_quote(&snapshot, &client, &built.call).await?;
            assert_eq!((raw.hotkeys, raw.holdings), (2, 5));
            assert_eq!(
                (
                    raw.accrued,
                    raw.redeemable,
                    raw.eligible_hotkeys,
                    raw.below_threshold_hotkeys
                ),
                (accrued, redeemable, eligible, below)
            );
            assert_eq!(raw.spent.rao(), spent);
            let preview =
                inspect(&intent, &snapshot, &snapshot, &client, &built.call, PAYER).await?;
            if eligible == 0 {
                assert_eq!(preview.warnings.len(), 1);
                assert!(
                    preview.warnings[0].ends_with("the claim pays a scan fee and realizes nothing")
                );
            } else {
                assert!(
                    preview.warnings[0].starts_with("1 of 2 validators are individually below")
                );
            }
            client.close().await;
        }
        Ok(())
    }

    #[tokio::test]
    async fn coldkey_empty_baskets_floor_fee_at_selected_hotkey_work() -> anyhow::Result<()> {
        // pyt: unit/test_root_claim_fee.py::test_coldkey_wide_empty_baskets_floor_to_hotkey_count
        let keys = [HOTKEY, OTHER_HOTKEY, AccountId([11; 32])];
        let (_chain, client, snapshot) = fixture(|state| {
            seed_admission(state, &keys, &keys, 0)?;
            state.seed_runtime(
                "BetaBasketRuntimeApi",
                "get_root_basket_positions",
                json!([]),
            )?;
            Ok(())
        })
        .await?;
        let intent = ClaimRoot {
            subnets: vec![NetUid::ROOT],
        };
        let built = build(&intent, &snapshot).await?;
        let admitted = admission(&snapshot, CLAIMANT, None).await?;
        assert_eq!(admitted.selection_scans, 3);
        let raw = coldkey_quote(&snapshot, &client, &built.call).await?;
        assert_eq!(
            (raw.hotkeys, raw.holdings, raw.accrued, raw.redeemable),
            (3, 0, 0, 0)
        );
        assert_eq!((raw.eligible_hotkeys, raw.below_threshold_hotkeys), (0, 0));
        assert_eq!(raw.spent.rao(), 2_790_901);
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn single_preview_none_and_missing_method_fall_back_to_full_entitlement()
    -> anyhow::Result<()> {
        // pyt: unit/test_root_claim_fee.py::test_quote_falls_back_to_full_entitlement_without_the_preview_api
        let data = fixture_data()?;
        let seed = extracted(
            &data.seed_cases,
            "test_quote_falls_back_to_full_entitlement_without_the_preview_api",
        )?;
        for missing_method in [false, true] {
            let (_chain, client, snapshot) = fixture(|state| {
                seed_extracted_case(state, seed)?;
                if missing_method {
                    state.set_handler("state_call", |state, request| {
                        if request.params[0] == "BetaBasketRuntimeApi_get_basket_claim_preview" {
                            Reply::error(-32601, "Method not found", None)
                        } else {
                            state.dispatch(request)
                        }
                    });
                }
                Ok(())
            })
            .await?;
            let intent = single();
            let built = build(&intent, &snapshot).await?;
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
            let admitted = admission(&snapshot, CLAIMANT, Some(HOTKEY)).await?;
            let reserved = reserve(&cx, admitted.limit).await?;
            let raw = quote(&cx, &admitted, &reserved, false).await?;
            assert_eq!(
                (raw.accrued, raw.redeemable, raw.dust_rows, raw.forfeited),
                (900_000, 900_000, 0, 0)
            );
            let preview =
                inspect(&intent, &snapshot, &snapshot, &client, &built.call, PAYER).await?;
            assert_eq!(fact(&preview, "accrued"), Some("τ0.000900000"));
            assert_eq!(fact(&preview, "redeemable"), None);
            assert_eq!(fact(&preview, "dust"), None);
            assert_eq!(fact(&preview, "spent"), Some("~τ0.002790901"));
            client.close().await;
        }
        Ok(())
    }

    #[tokio::test]
    async fn all_dust_and_consolidation_price_scan_and_redeem_work_separately() -> anyhow::Result<()>
    {
        // pyt: unit/test_root_claim_fee.py::test_single_hotkey_quote_reports_an_all_dust_claim_as_a_noop
        // pyt: unit/test_root_claim_fee.py::test_quote_prices_consolidated_rows_as_full_claim_units
        // pyt: unit/test_root_claim_fee.py::test_quote_counts_consolidation_even_when_the_claim_is_below_threshold
        let data = fixture_data()?;
        for (function, accrued, redeemable, holdings, spent) in [
            (
                "test_single_hotkey_quote_reports_an_all_dust_claim_as_a_noop",
                8_000u64,
                0u64,
                2,
                1_891_544,
            ),
            (
                "test_quote_counts_consolidation_even_when_the_claim_is_below_threshold",
                300_000,
                300_000,
                8,
                5_322_843,
            ),
            (
                "test_quote_prices_consolidated_rows_as_full_claim_units",
                2_700_000_000,
                2_700_000_000,
                8,
                6_671_879,
            ),
        ] {
            let seed = extracted(&data.seed_cases, function)?;
            let previews = extracted(&data.preview_cases, function)?;
            let (_chain, client, snapshot) = fixture(|state| {
                seed_extracted_case(state, seed)?;
                seed_extracted_previews(state, previews, false)
            })
            .await?;
            let intent = single();
            let built = build(&intent, &snapshot).await?;
            let preview =
                inspect(&intent, &snapshot, &snapshot, &client, &built.call, PAYER).await?;
            assert_eq!(fact(&preview, "accrued"), Some(money(accrued).as_str()));
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
            if accrued == 8_000 {
                assert_eq!(fact(&preview, "redeemable"), Some("τ0.000000000"));
                assert_eq!(
                    fact(&preview, "dust"),
                    Some("2 rows skipped; ~τ0.000008000 stays in the fund")
                );
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
