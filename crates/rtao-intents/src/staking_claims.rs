// Derived from RaoFoundation/subtensor sdk/python/bittensor/intents/staking.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/tests/unit/test_slippage_protection.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/tests/unit/test_intents_table.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Position exits and transfers, including atomic root claims and their preview.

use futures::TryStreamExt;
use rtao_client::{View, ViewExt};
use rtao_core::{codec::Value, value_ext::ValueExt};
use rtao_primitives::{
    AccountId, AlphaBalance, Amount, DynBalance, NetUid, PriceRao, ResolvedAmount, Spend,
    Ss58Address, UnitError,
};
use rtao_runtime::{
    CallBytes,
    calls::{subtensor_module as calls, utility},
    runtime_apis::{beta_basket_runtime_api as basket, stake_info_runtime_api as stake},
    storage::subtensor_module as st,
};
use rtao_tx::{BuildCx, Built, FactsTitle, Intent, IntentError, IntentPreflight, PreflightCx};
use smallvec::{SmallVec, smallvec};

use crate::staking_trades::{
    QuoteInput, alpha_price_rao, check_rate_tolerance, limit_price_rao, staked_rao,
    swap_limit_price_rao, swap_quote_facts,
};
pub use crate::{MoveStake, RemoveStake, RemoveStakeLimit, TransferStake, UnstakeAll};

const CLAIM_NOT_PROPORTIONAL: &str = "claims the entire basket entitlement for this validator (not a proportional slice of the unstake), then unstakes principal";
const MOVE_CLAIM_NOT_PROPORTIONAL: &str = "claims the entire basket entitlement for this validator (not a proportional slice of the move), then moves principal";

fn amount_summary(amount: &Amount, netuid: NetUid) -> String {
    match amount.resolve(netuid, true) {
        Ok(ResolvedAmount::Exact(amount)) => amount.to_string(),
        Ok(ResolvedAmount::All) => "ALL alpha".into(),
        Err(_) => match amount {
            Amount::All => "ALL alpha".into(),
            Amount::Exact(amount) => amount.to_decimal_string(),
        },
    }
}

async fn optional_claim<V: View + ?Sized>(
    cx: &BuildCx<'_, V>,
    follow_up: CallBytes,
    claim: bool,
    hotkey: &Ss58Address,
    netuid: Option<NetUid>,
) -> Result<CallBytes, IntentError> {
    if !claim {
        return Ok(follow_up);
    }
    if netuid.is_some_and(|netuid| netuid != NetUid::ROOT) {
        return Err(IntentError::validation(
            "claim=True is only valid when the origin is root (netuid 0). claim_root_with_hotkey pays this validator's whole basket entitlement, not a proportional slice of the follow-up.",
        ));
    }
    let interval = cx.view.storage(&st::ROOT_STAKE_UNLOCK_INTERVAL, ()).await?;
    if interval > 0 {
        return Err(IntentError::validation(format!(
            "claim then move/unstake cannot execute atomically while RootStakeUnlockInterval is {interval} blocks: a successful claim refreshes the root-stake hold before the follow-up runs. Claim first, wait for the hold interval, then move or unstake."
        )));
    }
    let claim = calls::claim_root_with_hotkey(hotkey.account()).compose(cx.runtime)?;
    Ok(utility::batch_all(vec![claim, follow_up]).compose(cx.runtime)?)
}

async fn unstake_amount<V: View + ?Sized>(
    cx: &BuildCx<'_, V>,
    amount: &Amount,
    hotkey: &Ss58Address,
    netuid: NetUid,
    claim: bool,
) -> Result<(u64, bool), IntentError> {
    let claim_all = claim && netuid == NetUid::ROOT && matches!(amount, Amount::All);
    match amount.resolve(netuid, true)? {
        ResolvedAmount::Exact(amount) => Ok((amount.rao, claim_all)),
        ResolvedAmount::All => {
            let current = staked_rao(
                cx.view,
                cx.coldkey_address()?,
                hotkey,
                netuid,
                "unstake",
                false,
            )
            .await?;
            Ok((if claim_all { u64::MAX } else { current }, claim_all))
        }
    }
}

async fn root_claimable_warning<V: View + ?Sized>(
    view: &V,
    coldkey: AccountId,
    hotkey: &Ss58Address,
) -> Option<String> {
    let amount = view
        .call(&basket::GET_BASKET_PAYOUT, (hotkey.account(), coldkey))
        .await
        .ok()?
        .0;
    (amount > 0).then(||format!("{} remains claimable via `btcli root claim` or `--claim` (whole entitlement, not a slice of this unstake)",DynBalance::from_rao(amount, NetUid::ROOT)))
}

async fn remove_warnings<V: View + ?Sized>(
    amount: &Amount,
    netuid: NetUid,
    claim: bool,
    hotkey: &Ss58Address,
    cx: &PreflightCx<'_, V>,
) -> Vec<String> {
    let mut warnings = Vec::new();
    if matches!(amount, Amount::All) {
        warnings.push("removes the entire stake from this hotkey on this subnet".into());
    }
    if netuid == NetUid::ROOT {
        if claim {
            warnings.push(CLAIM_NOT_PROPORTIONAL.into());
        } else if let Some(warning) =
            root_claimable_warning(cx.view, cx.dispatch_origin, hotkey).await
        {
            warnings.push(warning);
        }
    }
    warnings
}

async fn claim_preflight<I: Intent, V: View + ?Sized>(
    intent: &I,
    cx: &PreflightCx<'_, V>,
    claim: bool,
    netuid: Option<NetUid>,
    hotkey: &Ss58Address,
) -> Result<IntentPreflight, IntentError> {
    let mut base = IntentPreflight {
        effects: intent.effects(cx).await?,
        warnings: intent.warnings(cx).await?,
        blocks: intent.blocks(cx).await?,
        ..Default::default()
    };
    if !claim || netuid.is_some_and(|netuid| netuid != NetUid::ROOT) {
        return Ok(base);
    }
    // Reuse the original context: admission reads the dispatch origin, while
    // affordability and the fully wrapped fee probe use the outer fee payer.
    let claim = crate::root_claim::claim_with_hotkey_preflight(hotkey, cx).await?;
    base.effects.extend(claim.effects);
    base.warnings.extend(claim.warnings);
    base.blocks.extend(claim.blocks);
    base.required_free = claim.required_free;
    base.available_free = claim.available_free;
    base.estimated_fee = claim.estimated_fee;
    base.facts.extend(claim.facts);
    Ok(base)
}

impl Intent for RemoveStake {
    fn summary(&self) -> String {
        let prefix = if self.claim {
            "claim basket yield then "
        } else {
            ""
        };
        let note = if self.slippage_protection {
            format!(
                " (fails if price moves >{:.2}%)",
                self.rate_tolerance.get() * 100.0
            )
        } else {
            " (no slippage protection)".into()
        };
        format!(
            "{prefix}unstake {} from {} on netuid {}{note}",
            amount_summary(&self.amount_alpha, self.netuid),
            self.hotkey_ss58,
            self.netuid
        )
    }
    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        self.validate()?;
        let (amount, claim_all) = unstake_amount(
            cx,
            &self.amount_alpha,
            &self.hotkey_ss58,
            self.netuid,
            self.claim,
        )
        .await?;
        let unstake = if self.slippage_protection && !claim_all {
            calls::remove_stake_limit(
                self.hotkey_ss58.account(),
                self.netuid,
                AlphaBalance(amount),
                limit_price_rao(
                    alpha_price_rao(cx.view, self.netuid).await?,
                    self.rate_tolerance,
                    false,
                )?,
                false,
            )
        } else {
            calls::remove_stake(
                self.hotkey_ss58.account(),
                self.netuid,
                AlphaBalance(amount),
            )
        }
        .compose(cx.runtime)?;
        Ok(Built::new(
            optional_claim(
                cx,
                unstake,
                self.claim,
                &self.hotkey_ss58,
                Some(self.netuid),
            )
            .await?,
        ))
    }
    async fn warnings<V: View + ?Sized>(
        &self,
        cx: &PreflightCx<'_, V>,
    ) -> Result<Vec<String>, IntentError> {
        Ok(remove_warnings(
            &self.amount_alpha,
            self.netuid,
            self.claim,
            &self.hotkey_ss58,
            cx,
        )
        .await)
    }
    async fn preflight<V: View + ?Sized>(
        &self,
        cx: &PreflightCx<'_, V>,
    ) -> Result<IntentPreflight, IntentError> {
        let mut preview =
            claim_preflight(self, cx, self.claim, Some(self.netuid), &self.hotkey_ss58).await?;
        if self.netuid != NetUid::ROOT
            && let ResolvedAmount::Exact(amount) = self.amount_alpha.resolve(self.netuid, true)?
        {
            let mut facts =
                swap_quote_facts(cx.view, self.netuid, QuoteInput::Alpha(amount.rao)).await?;
            if !facts.is_empty() {
                facts.extend(preview.facts);
                preview.facts = facts;
                preview.facts_title = FactsTitle::Quote;
            }
        }
        Ok(preview)
    }
    fn validate(&self) -> Result<(), IntentError> {
        self.amount_alpha.resolve(self.netuid, true)?;
        check_rate_tolerance(self.rate_tolerance)
    }
}

impl Intent for RemoveStakeLimit {
    fn summary(&self) -> String {
        let prefix = if self.claim {
            "claim basket yield then "
        } else {
            ""
        };
        format!(
            "{prefix}unstake {} from {} on netuid {} (limit {} rao/alpha)",
            amount_summary(&self.amount_alpha, self.netuid),
            self.hotkey_ss58,
            self.netuid,
            self.limit_price_rao
        )
    }
    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        self.validate()?;
        let (amount, claim_all) = unstake_amount(
            cx,
            &self.amount_alpha,
            &self.hotkey_ss58,
            self.netuid,
            self.claim,
        )
        .await?;
        let unstake = if claim_all {
            if self.limit_price_rao > 1_000_000_000 {
                return Err(IntentError::validation(format!(
                    "root remove_stake_limit requires limit_price_rao in [0, 1000000000]; got {}",
                    self.limit_price_rao
                )));
            }
            calls::remove_stake(
                self.hotkey_ss58.account(),
                self.netuid,
                AlphaBalance(amount),
            )
        } else {
            calls::remove_stake_limit(
                self.hotkey_ss58.account(),
                self.netuid,
                AlphaBalance(amount),
                PriceRao(self.limit_price_rao),
                self.allow_partial,
            )
        }
        .compose(cx.runtime)?;
        Ok(Built::new(
            optional_claim(
                cx,
                unstake,
                self.claim,
                &self.hotkey_ss58,
                Some(self.netuid),
            )
            .await?,
        ))
    }
    async fn warnings<V: View + ?Sized>(
        &self,
        cx: &PreflightCx<'_, V>,
    ) -> Result<Vec<String>, IntentError> {
        Ok(remove_warnings(
            &self.amount_alpha,
            self.netuid,
            self.claim,
            &self.hotkey_ss58,
            cx,
        )
        .await)
    }
    async fn preflight<V: View + ?Sized>(
        &self,
        cx: &PreflightCx<'_, V>,
    ) -> Result<IntentPreflight, IntentError> {
        claim_preflight(self, cx, self.claim, Some(self.netuid), &self.hotkey_ss58).await
    }
    fn validate(&self) -> Result<(), IntentError> {
        self.amount_alpha.resolve(self.netuid, true)?;
        Ok(())
    }
}

impl Intent for MoveStake {
    fn summary(&self) -> String {
        let prefix = if self.claim {
            "claim basket yield then "
        } else {
            ""
        };
        let note = if self.origin_netuid == self.dest_netuid {
            String::new()
        } else if self.slippage_protection {
            format!(
                " (fails if price ratio moves >{:.2}%)",
                self.rate_tolerance.get() * 100.0
            )
        } else {
            " (no slippage protection)".into()
        };
        format!(
            "{prefix}move {} from {} on netuid {} to {} on netuid {}{note}",
            amount_summary(&self.amount_alpha, self.origin_netuid),
            self.origin_hotkey_ss58,
            self.origin_netuid,
            self.dest_hotkey_ss58,
            self.dest_netuid
        )
    }
    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        self.validate()?;
        if self.claim && self.origin_netuid != NetUid::ROOT {
            return Err(IntentError::validation(
                "claim=True is only valid when moving from root (netuid 0). claim_root_with_hotkey pays this validator's whole basket entitlement, not a proportional slice of the move.",
            ));
        }
        let amount = match self.amount_alpha.resolve(self.origin_netuid, true)? {
            ResolvedAmount::Exact(amount) => amount.rao,
            ResolvedAmount::All if self.claim => u64::MAX,
            ResolvedAmount::All => {
                staked_rao(
                    cx.view,
                    cx.coldkey_address()?,
                    &self.origin_hotkey_ss58,
                    self.origin_netuid,
                    "move",
                    false,
                )
                .await?
            }
        };
        let moved = if self.slippage_protection && self.origin_netuid != self.dest_netuid {
            calls::move_stake_limit(
                self.origin_hotkey_ss58.account(),
                self.dest_hotkey_ss58.account(),
                self.origin_netuid,
                self.dest_netuid,
                AlphaBalance(amount),
                swap_limit_price_rao(
                    cx.view,
                    self.origin_netuid,
                    self.dest_netuid,
                    self.rate_tolerance,
                )
                .await?,
                false,
            )
        } else {
            calls::move_stake(
                self.origin_hotkey_ss58.account(),
                self.dest_hotkey_ss58.account(),
                self.origin_netuid,
                self.dest_netuid,
                AlphaBalance(amount),
            )
        }
        .compose(cx.runtime)?;
        Ok(Built::new(
            optional_claim(
                cx,
                moved,
                self.claim,
                &self.origin_hotkey_ss58,
                Some(self.origin_netuid),
            )
            .await?,
        ))
    }
    async fn warnings<V: View + ?Sized>(
        &self,
        cx: &PreflightCx<'_, V>,
    ) -> Result<Vec<String>, IntentError> {
        let mut warnings = Vec::new();
        if matches!(self.amount_alpha, Amount::All) {
            warnings.push("moves the entire origin position".into());
        }
        if self.origin_netuid == NetUid::ROOT {
            if self.claim {
                warnings.push(MOVE_CLAIM_NOT_PROPORTIONAL.into());
            } else if let Some(warning) =
                root_claimable_warning(cx.view, cx.dispatch_origin, &self.origin_hotkey_ss58).await
            {
                warnings.push(warning);
            }
        }
        Ok(warnings)
    }
    async fn preflight<V: View + ?Sized>(
        &self,
        cx: &PreflightCx<'_, V>,
    ) -> Result<IntentPreflight, IntentError> {
        claim_preflight(
            self,
            cx,
            self.claim,
            Some(self.origin_netuid),
            &self.origin_hotkey_ss58,
        )
        .await
    }
    fn touches_netuids(&self) -> SmallVec<[NetUid; 2]> {
        smallvec![self.origin_netuid, self.dest_netuid]
    }
    fn validate(&self) -> Result<(), IntentError> {
        self.amount_alpha.resolve(self.origin_netuid, true)?;
        check_rate_tolerance(self.rate_tolerance)
    }
}

impl Intent for UnstakeAll {
    fn summary(&self) -> String {
        format!(
            "{}unstake ALL from {}",
            if self.claim {
                "claim basket yield then "
            } else {
                ""
            },
            self.hotkey_ss58
        )
    }
    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        let unstake = calls::unstake_all(self.hotkey_ss58.account()).compose(cx.runtime)?;
        Ok(Built::new(
            optional_claim(cx, unstake, self.claim, &self.hotkey_ss58, None).await?,
        ))
    }
    async fn warnings<V: View + ?Sized>(
        &self,
        cx: &PreflightCx<'_, V>,
    ) -> Result<Vec<String>, IntentError> {
        let mut warnings = vec!["removes the entire stake from this hotkey".into()];
        if self.claim {
            warnings.push(CLAIM_NOT_PROPORTIONAL.into());
        } else if let Some(warning) =
            root_claimable_warning(cx.view, cx.dispatch_origin, &self.hotkey_ss58).await
        {
            warnings.push(warning);
        }
        Ok(warnings)
    }
    async fn preflight<V: View + ?Sized>(
        &self,
        cx: &PreflightCx<'_, V>,
    ) -> Result<IntentPreflight, IntentError> {
        claim_preflight(self, cx, self.claim, None, &self.hotkey_ss58).await
    }
    fn affects_all_subnets(&self) -> bool {
        true
    }
}

fn row_truthy(value: &Value) -> bool {
    !matches!(value, Value::Null) && !matches!(value,Value::Dict(values) if values.is_empty())
}

fn availability_row(
    raw: &Value,
    coldkey: &str,
    netuid: NetUid,
) -> Result<(u64, u64, u64), IntentError> {
    let row = raw.field(coldkey).and_then(|rows| {
        let Value::Dict(entries) = rows else {
            return None;
        };
        entries
            .iter()
            .find(|(key, _)| key.as_u128() == Some(u128::from(netuid.0)))
            .map(|(_, row)| row)
            .filter(|row| row_truthy(row))
            .or_else(|| rows.field(&netuid.to_string()))
    });
    let get = |field: &str| -> Result<u64, IntentError> {
        match row.and_then(|row| row.field(field)) {
            None | Some(Value::Null) => Ok(0),
            Some(amount) => amount
                .as_u128()
                .and_then(|amount| u64::try_from(amount).ok())
                .ok_or_else(|| UnitError::Overflow.into()),
        }
    };
    Ok((get("total")?, get("locked")?, get("available")?))
}

async fn availability_rao<V: View + ?Sized>(
    view: &V,
    coldkey: AccountId,
    netuid: NetUid,
    prefix: u16,
) -> Result<(u64, u64, u64), IntentError> {
    let raw = view
        .call(
            &stake::GET_STAKE_AVAILABILITY_FOR_COLDKEYS,
            (vec![coldkey], Some(vec![netuid])),
        )
        .await?;
    availability_row(raw.as_core(), &coldkey.to_ss58(prefix), netuid)
}

async fn lock_hotkey<V: View + ?Sized>(
    view: &V,
    coldkey: AccountId,
    netuid: NetUid,
) -> Result<Option<String>, IntentError> {
    let rows = view.storage_iter(&st::LOCK, (coldkey,));
    futures::pin_mut!(rows);
    while let Some((keys, _)) = rows.try_next().await? {
        if keys.len() >= 2 && keys[0].as_core().as_u128() == Some(u128::from(netuid.0)) {
            return keys[1]
                .as_core()
                .as_str()
                .map(|key| Some(key.to_owned()))
                .ok_or_else(|| {
                    IntentError::validation("SubtensorModule.Lock hotkey is not an account string")
                });
        }
    }
    Ok(None)
}

impl TransferStake {
    fn changes_hotkey(&self) -> bool {
        self.dest_hotkey_ss58
            .as_ref()
            .is_some_and(|hotkey| hotkey != &self.hotkey_ss58)
    }
}

impl Intent for TransferStake {
    fn summary(&self) -> String {
        let note = if self.changes_hotkey() {
            format!(
                " (landing on hotkey {})",
                self.dest_hotkey_ss58
                    .as_ref()
                    .map_or("", Ss58Address::as_str)
            )
        } else {
            String::new()
        };
        format!(
            "transfer {} on netuid {} to coldkey {}{note}",
            amount_summary(&self.amount_alpha, self.origin_netuid),
            self.origin_netuid,
            self.dest_coldkey_ss58
        )
    }
    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        self.validate()?;
        let amount = match self.amount_alpha.resolve(self.origin_netuid, true)? {
            ResolvedAmount::Exact(amount) => amount.rao,
            ResolvedAmount::All => {
                staked_rao(
                    cx.view,
                    cx.coldkey_address()?,
                    &self.hotkey_ss58,
                    self.origin_netuid,
                    "transfer",
                    false,
                )
                .await?
            }
        };
        let call = if self.changes_hotkey() {
            calls::transfer_stake_and_hotkey(
                self.dest_coldkey_ss58.account(),
                self.hotkey_ss58.account(),
                self.dest_hotkey_ss58
                    .as_ref()
                    .map_or(self.hotkey_ss58.account(), Ss58Address::account),
                self.origin_netuid,
                self.dest_netuid,
                AlphaBalance(amount),
            )
        } else {
            calls::transfer_stake(
                self.dest_coldkey_ss58.account(),
                self.hotkey_ss58.account(),
                self.origin_netuid,
                self.dest_netuid,
                AlphaBalance(amount),
            )
        };
        Ok(Built::new(call.compose(cx.runtime)?))
    }
    async fn warnings<V: View + ?Sized>(
        &self,
        cx: &PreflightCx<'_, V>,
    ) -> Result<Vec<String>, IntentError> {
        let mut warnings = vec!["transfers stake OWNERSHIP to another coldkey".into()];
        if matches!(self.amount_alpha, Amount::All) {
            warnings.push("hands over the entire origin position".into());
        }
        if self.origin_netuid != self.dest_netuid {
            return Ok(warnings);
        }
        let landing = self
            .dest_hotkey_ss58
            .as_ref()
            .unwrap_or(&self.hotkey_ss58)
            .as_str();
        let ((_total, locked, available), sender_lock, receiver_lock, position) = futures::try_join!(
            availability_rao(
                cx.view,
                cx.dispatch_origin,
                self.origin_netuid,
                cx.runtime.core().ss58_format
            ),
            lock_hotkey(cx.view, cx.dispatch_origin, self.origin_netuid),
            lock_hotkey(cx.view, self.dest_coldkey_ss58.account(), self.dest_netuid),
            staked_rao(
                cx.view,
                cx.dispatch_origin,
                &self.hotkey_ss58,
                self.origin_netuid,
                "transfer",
                true
            )
        )?;
        let amount = match self.amount_alpha.resolve(self.origin_netuid, true)? {
            ResolvedAmount::Exact(amount) => amount.rao,
            ResolvedAmount::All => position,
        };
        if position < amount {
            warnings.push(format!("origin hotkey {} holds only {} on netuid {} — transfer will fail with NotEnoughStakeToWithdraw; move stake onto this hotkey first, or pass a different origin hotkey",self.hotkey_ss58,DynBalance::from_rao(position,self.origin_netuid),self.origin_netuid));
        }
        if let Some(sender) = &sender_lock
            && locked > 0
            && sender != self.hotkey_ss58.as_str()
        {
            warnings.push(format!("your conviction lock on netuid {} targets {sender}, but stake is leaving {} (lock hotkey and stake hotkey can differ)",self.origin_netuid,self.hotkey_ss58));
        }
        if amount > available && locked > 0 {
            let moving = (amount - available).min(locked);
            warnings.push(format!("{} locked alpha will move with this transfer ({} is free; the rest pulls from the conviction lock)",DynBalance::from_rao(moving,self.origin_netuid),DynBalance::from_rao(available,self.origin_netuid)));
            if let Some(receiver) = &receiver_lock {
                if receiver != landing {
                    warnings.push(format!("receiver already locks to {receiver}; locked alpha can only land on that hotkey — pass --destination-hotkey {receiver} (current landing hotkey is {landing}) or transfer only the free amount"));
                }
            } else if landing != sender_lock.as_deref().unwrap_or(landing) {
                warnings.push(format!("receiver has no lock yet; locked alpha will create one on landing hotkey {landing}"));
            }
        }
        Ok(warnings)
    }
    fn spend(&self) -> Spend {
        Spend::Unbounded
    }
    fn touches_netuids(&self) -> SmallVec<[NetUid; 2]> {
        smallvec![self.origin_netuid, self.dest_netuid]
    }
    fn validate(&self) -> Result<(), IntentError> {
        self.amount_alpha.resolve(self.origin_netuid, true)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rtao_client::{BlockRef, Client, ClientConfig, DynView, Network, Snapshot};
    use rtao_core::{DynValue, runtime::TypeSpec};
    use rtao_primitives::{Alpha, Balance, Ratio, Tao};
    use rtao_runtime::{RuntimeHandle, calls::proxy};
    use rtao_testkit::{FakeChain, FakeState, RecordingView, Reply, rpc::RpcClient};
    use rtao_tx::{DynIntent, OriginView, Policy, SystemClock};
    use serde_json::{Value as Json, json};
    use std::collections::BTreeSet;

    const ORIGIN: AccountId = AccountId([7; 32]);
    const HOTKEY: AccountId = AccountId([8; 32]);
    const PAYER: AccountId = AccountId([9; 32]);
    const DESTINATION: AccountId = AccountId([10; 32]);
    const SENDER_LOCK: AccountId = AccountId([11; 32]);
    const RECEIVER_LOCK: AccountId = AccountId([12; 32]);
    const RECEIVER: AccountId = AccountId([6; 32]);

    fn seed_availability(
        state: &mut FakeState,
        total: u64,
        locked: u64,
        available: u64,
    ) -> anyhow::Result<()> {
        let row = Value::record(vec![
            ("total".into(), Value::Int(i128::from(total))),
            ("locked".into(), Value::Int(i128::from(locked))),
            ("available".into(), Value::Int(i128::from(available))),
        ]);
        let value = Value::Dict(vec![(
            Value::Str(ORIGIN.to_string()),
            Value::Dict(vec![(Value::Int(0), row.clone()), (Value::Int(1), row)]),
        )]);
        let method = rtao_core::ops::runtime_api::runtime_api_method(
            &state.runtime,
            "StakeInfoRuntimeApi",
            "get_stake_availability_for_coldkeys",
        )?;
        let encoded = state
            .runtime
            .encode_spec(&TypeSpec::Id(method.output), &value)?;
        let mut inputs = Vec::new();
        for netuid in [0, 1] {
            let input = rtao_core::ops::runtime_api::encode_params(
                &state.runtime,
                "StakeInfoRuntimeApi",
                method,
                &[
                    Value::List(vec![Value::Bytes(ORIGIN.0.to_vec())]),
                    Value::List(vec![Value::Int(netuid)]),
                ],
            )?;
            inputs.push(input);
        }
        for input in inputs {
            state.seed_runtime_encoded(
                "StakeInfoRuntimeApi_get_stake_availability_for_coldkeys",
                input,
                encoded.clone(),
            );
        }
        Ok(())
    }

    async fn fixture(
        setup: impl FnOnce(&mut FakeState) -> anyhow::Result<()>,
    ) -> anyhow::Result<(FakeChain, Client, Snapshot)> {
        let mut state = FakeState::try_default()?;
        for (account, free) in [(ORIGIN, u64::MAX), (PAYER, 1_000_000_000)] {
            state.seed_python(
                "System",
                "Account",
                &[json!(account.to_string())],
                json!({"data":{"free":free}}),
            )?;
        }
        state.seed(&st::ROOT_STAKE_UNLOCK_INTERVAL, (), 0)?;
        state.seed(&st::NETWORKS_ADDED, (NetUid::ROOT,), true)?;
        state.seed_runtime(
            "StakeInfoRuntimeApi",
            "get_stake_info_for_hotkey_coldkey_netuid",
            json!({"stake":100}),
        )?;
        state.seed_runtime(
            "SwapRuntimeApi",
            "current_alpha_price",
            json!(2_000_000_000u64),
        )?;
        for netuid in [0u16, 2] {
            state.seed_runtime_encoded(
                "SwapRuntimeApi_current_alpha_price",
                netuid.to_le_bytes().to_vec(),
                1_000_000_000u64.to_le_bytes().to_vec(),
            );
        }
        state.seed_runtime(
            "SwapRuntimeApi",
            "sim_swap_alpha_for_tao",
            json!({"tao_amount":2_000_000_000u64,"alpha_fee":3}),
        )?;
        state.seed_runtime(
            "BetaBasketRuntimeApi",
            "get_validator_basket",
            json!([[1, 1, 1], [2, 1, 1], [3, 1, 1]]),
        )?;
        state.seed_runtime("BetaBasketRuntimeApi", "get_basket_payout", json!(900_000))?;
        state.seed_runtime(
            "BetaBasketRuntimeApi",
            "get_basket_claim_preview",
            Json::Null,
        )?;
        state.seed_runtime(
            "BetaBasketRuntimeApi",
            "get_root_basket_positions",
            json!([[HOTKEY.to_string(), 1, 900_000]]),
        )?;
        seed_availability(&mut state, 200, 100, 30)?;
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

    async fn build(intent: &dyn DynIntent, snapshot: &Snapshot) -> Result<Built, IntentError> {
        intent
            .build_dyn(&BuildCx {
                view: snapshot as &dyn DynView,
                origin: OriginView::Account(ORIGIN),
                runtime: &snapshot.block().runtime,
                clock: &SystemClock,
                spec: intent.spec(),
            })
            .await
    }
    async fn inspect(
        intent: &dyn DynIntent,
        client: &Client,
        snapshot: &Snapshot,
        call: &CallBytes,
    ) -> Result<IntentPreflight, IntentError> {
        intent
            .preflight_dyn(&PreflightCx {
                view: snapshot as &dyn DynView,
                runtime: &snapshot.block().runtime,
                clock: &SystemClock,
                spec: intent.spec(),
                client,
                dispatch_origin: ORIGIN,
                fee_payer: PAYER,
                call,
            })
            .await
    }
    fn decode(call: &CallBytes, runtime: &RuntimeHandle) -> anyhow::Result<Json> {
        Ok(rtao_core::codec::value::to_corpus_json(
            &runtime
                .core()
                .decode_spec(&TypeSpec::Call, call.as_bytes(), true)?,
        ))
    }
    fn remove(
        amount: Amount,
        netuid: NetUid,
        claim: bool,
        protected: bool,
    ) -> anyhow::Result<RemoveStake> {
        Ok(RemoveStake {
            hotkey_ss58: HOTKEY.into(),
            netuid,
            amount_alpha: amount,
            claim,
            slippage_protection: protected,
            rate_tolerance: Ratio::new(0.05)?,
        })
    }
    fn remove_limit(amount: Amount, netuid: NetUid, claim: bool, limit: u64) -> RemoveStakeLimit {
        RemoveStakeLimit {
            hotkey_ss58: HOTKEY.into(),
            netuid,
            amount_alpha: amount,
            claim,
            limit_price_rao: limit,
            allow_partial: true,
        }
    }
    fn moved(
        amount: Amount,
        origin: NetUid,
        destination: NetUid,
        claim: bool,
        protected: bool,
    ) -> anyhow::Result<MoveStake> {
        Ok(MoveStake {
            origin_hotkey_ss58: HOTKEY.into(),
            dest_hotkey_ss58: DESTINATION.into(),
            origin_netuid: origin,
            dest_netuid: destination,
            amount_alpha: amount,
            claim,
            slippage_protection: protected,
            rate_tolerance: Ratio::new(0.05)?,
        })
    }
    fn transfer(
        amount: Amount,
        origin: NetUid,
        destination: NetUid,
        hotkey: Option<AccountId>,
    ) -> TransferStake {
        TransferStake {
            dest_coldkey_ss58: RECEIVER.into(),
            hotkey_ss58: HOTKEY.into(),
            origin_netuid: origin,
            dest_netuid: destination,
            amount_alpha: amount,
            dest_hotkey_ss58: hotkey.map(Into::into),
        }
    }
    fn plain_remove(netuid: NetUid, amount: u64, limit: Option<(u64, bool)>) -> Vec<u8> {
        let mut bytes = vec![7, if limit.is_some() { 89 } else { 3 }];
        bytes.extend_from_slice(HOTKEY.as_bytes());
        bytes.extend_from_slice(&netuid.0.to_le_bytes());
        bytes.extend_from_slice(&amount.to_le_bytes());
        if let Some((price, partial)) = limit {
            bytes.extend_from_slice(&price.to_le_bytes());
            bytes.push(u8::from(partial));
        }
        bytes
    }
    fn plain_move(origin: NetUid, destination: NetUid, amount: u64, limit: Option<u64>) -> Vec<u8> {
        let mut bytes = vec![7, if limit.is_some() { 149 } else { 85 }];
        bytes.extend_from_slice(HOTKEY.as_bytes());
        bytes.extend_from_slice(DESTINATION.as_bytes());
        bytes.extend_from_slice(&origin.0.to_le_bytes());
        bytes.extend_from_slice(&destination.0.to_le_bytes());
        bytes.extend_from_slice(&amount.to_le_bytes());
        if let Some(price) = limit {
            bytes.extend_from_slice(&price.to_le_bytes());
            bytes.push(0);
        }
        bytes
    }
    fn claim_batch(follow_up: &[u8]) -> Vec<u8> {
        let mut bytes = vec![11, 2, 8, 7, 148];
        bytes.extend_from_slice(HOTKEY.as_bytes());
        bytes.extend_from_slice(follow_up);
        bytes
    }

    // pyt: unit/test_slippage_protection.py::TestDefaultProtection::test_remove_stake_composes_limit_call
    // pyt: unit/test_slippage_protection.py::TestDefaultProtection::test_custom_tolerance_moves_the_limit
    // pyt: unit/test_slippage_protection.py::TestDefaultProtection::test_remove_all_resolves_stake_and_keeps_protection
    #[tokio::test]
    async fn source_removal_limits_keep_default_custom_and_all_principal() -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture(|state| {
            state.seed_runtime(
                "StakeInfoRuntimeApi",
                "get_stake_info_for_hotkey_coldkey_netuid",
                json!({"stake":5_000_000_000u64}),
            )?;
            Ok(())
        })
        .await?;
        for (amount, tolerance, expected_amount, expected_limit) in [
            (json!(1.0), None, 1_000_000_000, 1_900_000_000),
            (json!(1.0), Some(0.1), 1_000_000_000, 1_800_000_000),
            (json!("all"), None, 5_000_000_000u64, 1_900_000_000),
        ] {
            let mut args =
                json!({"hotkey_ss58":HOTKEY.to_string(),"netuid":1,"amount_alpha":amount});
            if let Some(tolerance) = tolerance {
                args["rate_tolerance"] = json!(tolerance);
            }
            let constructed = crate::AnyIntent::from_tool("remove_stake", args)?;
            assert_eq!(constructed.to_json()["slippage_protection"], json!(true));
            assert_eq!(constructed.to_json()["claim"], json!(false));
            let intent = constructed.into_dyn();
            let built = build(intent.as_ref(), &snapshot).await?;
            let expected = plain_remove(NetUid(1), expected_amount, Some((expected_limit, false)));
            assert_eq!(built.call.as_bytes(), expected);
            assert_eq!(
                CallBytes::decode(&expected, &snapshot.block().runtime)?,
                built.call
            );
        }
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        client.close().await;
        Ok(())
    }

    // pyt: unit/test_intents_table.py::TestRootClaimOnUnstake::test_claim_batches_claim_then_unstake
    // pyt: unit/test_intents_table.py::TestRootClaimOnUnstake::test_claim_all_uses_runtime_capped_max_after_claim
    // pyt: unit/test_intents_table.py::TestRootClaimOnUnstake::test_limit_claim_all_uses_runtime_capped_plain_unstake
    #[tokio::test]
    async fn root_claim_batches_encode_ordered_followups_with_live_max_sentinels()
    -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture(|state| {
            state.seed_runtime("BetaBasketRuntimeApi", "get_basket_payout", json!(0))?;
            Ok(())
        })
        .await?;
        let mut sweep = vec![7, 83];
        sweep.extend_from_slice(HOTKEY.as_bytes());
        for (intent, follow) in [
            (
                Box::new(remove(
                    Balance::<Tao>::from_rao(1_000_000_000).into(),
                    NetUid::ROOT,
                    true,
                    false,
                )?) as Box<dyn DynIntent>,
                plain_remove(NetUid::ROOT, 1_000_000_000, None),
            ),
            (
                Box::new(remove(Amount::All, NetUid::ROOT, true, true)?) as Box<dyn DynIntent>,
                plain_remove(NetUid::ROOT, u64::MAX, None),
            ),
            (
                Box::new(remove_limit(Amount::All, NetUid::ROOT, true, 1_000_000_000)),
                plain_remove(NetUid::ROOT, u64::MAX, None),
            ),
            (
                Box::new(remove_limit(Amount::All, NetUid::ROOT, true, 0)),
                plain_remove(NetUid::ROOT, u64::MAX, None),
            ),
            (
                Box::new(moved(
                    Balance::<Tao>::from_rao(1_000_000_000).into(),
                    NetUid::ROOT,
                    NetUid::ROOT,
                    true,
                    false,
                )?),
                plain_move(NetUid::ROOT, NetUid::ROOT, 1_000_000_000, None),
            ),
            (
                Box::new(moved(Amount::All, NetUid::ROOT, NetUid::ROOT, true, true)?),
                plain_move(NetUid::ROOT, NetUid::ROOT, u64::MAX, None),
            ),
            (
                Box::new(UnstakeAll {
                    hotkey_ss58: HOTKEY.into(),
                    claim: true,
                }),
                sweep,
            ),
        ] {
            let built = build(intent.as_ref(), &snapshot).await?;
            let expected = claim_batch(&follow);
            assert_eq!(built.call.as_bytes(), expected);
            assert_eq!(
                CallBytes::decode(&expected, &snapshot.block().runtime)?,
                built.call
            );
            assert!(built.extras.is_empty());
            let tree = decode(&built.call, &snapshot.block().runtime)?;
            assert_eq!(tree["call_module"], "Utility");
            assert_eq!(tree["call_function"], "batch_all");
        }
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn typed_staking_money_preserves_call_bytes_and_all_currency_mismatch_payloads()
    -> anyhow::Result<()> {
        use crate::AnyIntent;

        fn amount(intent: &mut AnyIntent) -> anyhow::Result<&mut Amount> {
            Ok(match intent {
                AnyIntent::AddStake(intent) => &mut intent.amount_tao,
                AnyIntent::AddStakeLimit(intent) => &mut intent.amount_tao,
                AnyIntent::RemoveStake(intent) => &mut intent.amount_alpha,
                AnyIntent::RemoveStakeLimit(intent) => &mut intent.amount_alpha,
                AnyIntent::MoveStake(intent) => &mut intent.amount_alpha,
                AnyIntent::MoveSwapStake(intent) => &mut intent.amount_alpha,
                AnyIntent::SwapStake(intent) => &mut intent.amount_alpha,
                AnyIntent::TransferStake(intent) => &mut intent.amount_alpha,
                _ => anyhow::bail!("fixture requires a staking amount"),
            })
        }

        let (chain, client, snapshot) = fixture(|_| Ok(())).await?;
        for (op, field, expected_unit) in [
            ("add_stake", "amount_tao", NetUid::ROOT),
            ("remove_stake", "amount_alpha", NetUid(1)),
            ("add_stake_limit", "amount_tao", NetUid::ROOT),
            ("remove_stake_limit", "amount_alpha", NetUid(1)),
        ] {
            let mut args = json!({"hotkey_ss58":HOTKEY,"netuid":1});
            args[field] = json!("1.5");
            if op.ends_with("_limit") {
                args["limit_price_rao"] = json!(1_000_000_000);
            }
            let mut plain = AnyIntent::from_tool(op, args)?;
            let mut tagged = plain.clone();
            *amount(&mut tagged)? = DynBalance::from_rao(1_500_000_000, expected_unit).into();
            let resolved =
                ResolvedAmount::Exact(DynBalance::from_rao(1_500_000_000, expected_unit));
            assert_eq!(
                amount(&mut plain)?.resolve(expected_unit, true)?,
                resolved,
                "{op}"
            );
            assert_eq!(
                amount(&mut tagged)?.resolve(expected_unit, true)?,
                resolved,
                "{op}"
            );
            plain.as_dyn().validate_dyn()?;
            tagged.as_dyn().validate_dyn()?;
            let plain_call = build(plain.as_dyn(), &snapshot).await?.call;
            let tagged_call = build(tagged.as_dyn(), &snapshot).await?.call;
            assert_eq!(
                tagged_call.as_bytes(),
                plain_call.as_bytes(),
                "{op} tagged and untagged money must encode identically"
            );
        }

        for (op, expected, actual, args) in [
            (
                "add_stake",
                NetUid::ROOT,
                NetUid(1),
                json!({"hotkey_ss58":HOTKEY,"netuid":1,"amount_tao":1}),
            ),
            (
                "remove_stake",
                NetUid(1),
                NetUid::ROOT,
                json!({"hotkey_ss58":HOTKEY,"netuid":1,"amount_alpha":1}),
            ),
            (
                "add_stake_limit",
                NetUid::ROOT,
                NetUid(2),
                json!({"hotkey_ss58":HOTKEY,"netuid":2,"amount_tao":1,"limit_price_rao":1_000_000_000}),
            ),
            (
                "remove_stake_limit",
                NetUid(2),
                NetUid::ROOT,
                json!({"hotkey_ss58":HOTKEY,"netuid":2,"amount_alpha":1,"limit_price_rao":1_000_000_000}),
            ),
            (
                "move_stake",
                NetUid(1),
                NetUid::ROOT,
                json!({"origin_hotkey_ss58":HOTKEY,"origin_netuid":1,"dest_hotkey_ss58":DESTINATION,"dest_netuid":2,"amount_alpha":1}),
            ),
            (
                "move_swap_stake",
                NetUid(1),
                NetUid::ROOT,
                json!({"origin_hotkey_ss58":HOTKEY,"origin_netuid":1,"dest_hotkey_ss58":DESTINATION,"dest_netuid":2,"amount_alpha":1}),
            ),
            (
                "swap_stake",
                NetUid(1),
                NetUid(2),
                json!({"hotkey_ss58":HOTKEY,"origin_netuid":1,"dest_netuid":2,"amount_alpha":1}),
            ),
            (
                "transfer_stake",
                NetUid(1),
                NetUid::ROOT,
                json!({"hotkey_ss58":HOTKEY,"dest_coldkey_ss58":RECEIVER,"origin_netuid":1,"dest_netuid":2,"amount_alpha":1}),
            ),
        ] {
            let mut intent = AnyIntent::from_tool(op, args)?;
            *amount(&mut intent)? = DynBalance::from_rao(1_000_000_000, actual).into();
            assert!(
                matches!(
                    intent.as_dyn().validate_dyn(),
                    Err(IntentError::UnitMismatch(UnitError::UnitMismatch {
                        expected: found_expected,
                        actual: found_actual,
                    })) if found_expected == expected && found_actual == actual
                ),
                "{op} must report the consuming and supplied currencies"
            );
        }

        let mut root = AnyIntent::from_tool(
            "remove_stake",
            json!({"hotkey_ss58":HOTKEY,"netuid":0,"amount_alpha":1}),
        )?;
        *amount(&mut root)? = Balance::<Tao>::from_rao(1_000_000_000).into();
        root.as_dyn().validate_dyn()?;
        assert_eq!(
            amount(&mut root)?.resolve(NetUid::ROOT, true)?,
            ResolvedAmount::Exact(DynBalance::from_rao(1_000_000_000, NetUid::ROOT))
        );
        // Native chain fields reject a negative unsigned price at input time.
        let mut root_limit_args = json!({"hotkey_ss58":HOTKEY,"netuid":0,"amount_alpha":"all","claim":true,"limit_price_rao":0});
        AnyIntent::from_tool("remove_stake_limit", root_limit_args.clone())?;
        root_limit_args["limit_price_rao"] = json!(-1);
        assert!(AnyIntent::from_tool("remove_stake_limit", root_limit_args).is_err());
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn remove_requires_existing_principal_while_root_claim_move_can_create_it()
    -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture(|state| {
            state.seed_runtime(
                "StakeInfoRuntimeApi",
                "get_stake_info_for_hotkey_coldkey_netuid",
                Json::Null,
            )?;
            Ok(())
        })
        .await?;
        for intent in [
            Box::new(remove(Amount::All, NetUid::ROOT, true, true)?) as Box<dyn DynIntent>,
            Box::new(remove_limit(Amount::All, NetUid::ROOT, true, 1_000_000_000)),
        ] {
            assert_eq!(
                build(intent.as_ref(), &snapshot)
                    .await
                    .err()
                    .map(|error| error.to_string()),
                Some(format!(
                    "nothing to unstake: no stake on {HOTKEY} at netuid 0"
                ))
            );
        }
        let intent = moved(Amount::All, NetUid::ROOT, NetUid::ROOT, true, true)?;
        let before = chain.requests().len();
        assert_eq!(
            build(&intent, &snapshot).await?.call.as_bytes(),
            claim_batch(&plain_move(NetUid::ROOT, NetUid::ROOT, u64::MAX, None))
        );
        assert!(
            !chain.requests()[before..]
                .iter()
                .any(|request| request.method == "state_call")
        );
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn explicit_limits_and_cross_subnet_moves_keep_protection_and_native_root_units()
    -> anyhow::Result<()> {
        let (_chain, client, snapshot) = fixture(|_| Ok(())).await?;
        for (intent, expected) in [
            (
                Box::new(remove(
                    Balance::<Alpha>::new(NetUid(1), u64::MAX)?.into(),
                    NetUid(1),
                    false,
                    true,
                )?) as Box<dyn DynIntent>,
                plain_remove(NetUid(1), u64::MAX, Some((1_900_000_000, false))),
            ),
            (
                Box::new(remove_limit(Amount::All, NetUid(1), false, u64::MAX)),
                plain_remove(NetUid(1), 100, Some((u64::MAX, true))),
            ),
            (
                Box::new(remove_limit(
                    Balance::<Tao>::from_rao(1).into(),
                    NetUid::ROOT,
                    true,
                    u64::MAX,
                )),
                claim_batch(&plain_remove(NetUid::ROOT, 1, Some((u64::MAX, true)))),
            ),
            (
                Box::new(moved(Amount::All, NetUid(1), NetUid(2), false, true)?),
                plain_move(NetUid(1), NetUid(2), 100, Some(1_900_000_000)),
            ),
            (
                Box::new(moved(
                    Balance::<Tao>::from_rao(1).into(),
                    NetUid::ROOT,
                    NetUid(2),
                    true,
                    true,
                )?),
                claim_batch(&plain_move(NetUid::ROOT, NetUid(2), 1, Some(950_000_000))),
            ),
        ] {
            let built = build(intent.as_ref(), &snapshot).await?;
            assert_eq!(built.call.as_bytes(), expected);
            assert_eq!(intent.spend(), Spend::None);
        }
        client.close().await;
        Ok(())
    }

    // pyt: unit/test_intents_table.py::TestRootClaimOnUnstake::test_claim_then_unstake_refuses_active_root_hold
    #[tokio::test]
    async fn holds_nonroot_claims_and_root_claim_all_limit_errors_keep_python_order()
    -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture(|state| {
            state.seed(&st::ROOT_STAKE_UNLOCK_INTERVAL, (), 100)?;
            Ok(())
        })
        .await?;
        for intent in [
            Box::new(remove(
                Balance::<Tao>::from_rao(1).into(),
                NetUid::ROOT,
                true,
                false,
            )?) as Box<dyn DynIntent>,
            Box::new(remove_limit(
                Balance::<Tao>::from_rao(1).into(),
                NetUid::ROOT,
                true,
                1_000_000_000,
            )),
            Box::new(moved(Amount::All, NetUid::ROOT, NetUid::ROOT, true, true)?),
            Box::new(UnstakeAll {
                hotkey_ss58: HOTKEY.into(),
                claim: true,
            }),
        ] {
            assert!(
                build(intent.as_ref(), &snapshot)
                    .await
                    .err()
                    .is_some_and(|error| error
                        .to_string()
                        .contains("RootStakeUnlockInterval is 100 blocks"))
            );
        }
        let early = moved(Amount::All, NetUid(1), NetUid(2), true, true)?;
        let before = chain.requests().len();
        assert!(build(&early, &snapshot).await.err().is_some_and(|error| {
            error
                .to_string()
                .starts_with("claim=True is only valid when moving from root")
        }));
        assert_eq!(chain.requests().len(), before);
        let late = remove(Amount::All, NetUid(1), true, true)?;
        let before = chain.requests().len();
        assert!(build(&late, &snapshot).await.err().is_some_and(|error| {
            error
                .to_string()
                .starts_with("claim=True is only valid when the origin is root")
        }));
        let methods: Vec<_> = chain.requests()[before..]
            .iter()
            .filter(|request| request.method == "state_call")
            .map(|request| request.params[0].clone())
            .collect();
        assert_eq!(
            methods,
            [
                json!("StakeInfoRuntimeApi_get_stake_info_for_hotkey_coldkey_netuid"),
                json!("SwapRuntimeApi_current_alpha_price")
            ]
        );
        let invalid = remove_limit(Amount::All, NetUid::ROOT, true, 1_000_000_001);
        assert_eq!(build(&invalid,&snapshot).await.err().map(|error|error.to_string()),Some("root remove_stake_limit requires limit_price_rao in [0, 1000000000]; got 1000000001".into()));
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn merged_claim_previews_preserve_admission_dust_legacy_and_financial_fields()
    -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture(|_| Ok(())).await?;
        for intent in [
            Box::new(remove(
                Balance::<Tao>::from_rao(1).into(),
                NetUid::ROOT,
                true,
                false,
            )?) as Box<dyn DynIntent>,
            Box::new(remove_limit(
                Balance::<Tao>::from_rao(1).into(),
                NetUid::ROOT,
                true,
                1_000_000_000,
            )),
            Box::new(moved(
                Balance::<Tao>::from_rao(1).into(),
                NetUid::ROOT,
                NetUid::ROOT,
                true,
                true,
            )?),
            Box::new(UnstakeAll {
                hotkey_ss58: HOTKEY.into(),
                claim: true,
            }),
        ] {
            let built = build(intent.as_ref(), &snapshot).await?;
            let preview = inspect(intent.as_ref(), &client, &snapshot, &built.call).await?;
            assert_eq!(preview.effects.first(), Some(&intent.summary_dyn()));
            assert_eq!(
                preview.effects.get(1),
                Some(&format!(
                    "claim root dividends on {HOTKEY} (redeem basket shares to root stake)"
                ))
            );
            assert!(
                preview
                    .warnings
                    .first()
                    .is_some_and(|warning| warning == CLAIM_NOT_PROPORTIONAL
                        || warning == MOVE_CLAIM_NOT_PROPORTIONAL
                        || warning == "removes the entire stake from this hotkey")
            );
            assert_eq!(
                preview.required_free,
                Some(Balance::<Tao>::from_rao(8_209_263))
            );
            assert_eq!(
                preview.available_free,
                Some(Balance::<Tao>::from_rao(1_000_000_000))
            );
            assert_eq!(preview.estimated_fee, preview.required_free);
            assert_eq!(preview.facts_title, FactsTitle::Fees);
            assert!(preview.facts.iter().any(|(key, _)| key == "accrued"));
        }
        chain.mutate_state(|state|state.seed_runtime("BetaBasketRuntimeApi","get_basket_claim_preview",json!({"hotkey":HOTKEY.to_string(),"owed_shares":1,"accrued_tao":2_250_000,"redeemable_tao":2_200_000,"forfeited_tao_est":50_000,"rows":3,"rows_to_sell":2,"dust_rows":1,"swept":0,"flushed_credits":0})))?;
        let intent = remove(
            Balance::<Tao>::from_rao(1).into(),
            NetUid::ROOT,
            true,
            false,
        )?;
        let built = build(&intent, &snapshot).await?;
        let preview = inspect(&intent, &client, &snapshot, &built.call).await?;
        assert!(preview.facts.iter().any(|(key, value)| key == "dust"
            && value == "1 rows skipped; ~τ0.000050000 stays in the fund"));
        assert!(
            preview
                .facts
                .iter()
                .any(|(key, value)| key == "redeemable" && value == "τ0.002200000")
        );
        chain.mutate_state(|state| {
            state.seed_runtime(
                "BetaBasketRuntimeApi",
                "get_validator_basket",
                json!((0..129).map(|_| json!([1, 1, 1])).collect::<Vec<_>>()),
            )
        })?;
        let denied = inspect(&intent, &client, &snapshot, &built.call).await?;
        assert!(
            denied
                .blocks
                .iter()
                .any(|block| block.contains("129 basket holdings")
                    && block.contains("129-unit admission limit"))
        );
        assert!(denied.required_free.is_none());
        assert!(denied.facts.is_empty());
        client.close().await;
        Ok(())
    }

    // pyt: unit/test_intents_table.py::TestRootClaimOnUnstake::test_embedded_claim_proxy_uses_delegate_for_reserve
    #[tokio::test]
    async fn wrapped_claim_prices_original_call_for_outer_payer_and_keeps_reserve_failure()
    -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture(|state| {
            state.seed_python(
                "System",
                "Account",
                &[json!(PAYER.to_string())],
                json!({"data":{"free":0}}),
            )?;
            Ok(())
        })
        .await?;
        let intent = remove(
            Balance::<Tao>::from_rao(1).into(),
            NetUid::ROOT,
            true,
            false,
        )?;
        let built = build(&intent, &snapshot).await?;
        let wrapped = proxy::proxy(
            DynValue::new(Value::Bytes(ORIGIN.0.to_vec())),
            None,
            built.call,
        )
        .compose(&snapshot.block().runtime)?;
        let view = RecordingView::new(snapshot.clone());
        let preview = intent
            .preflight(&PreflightCx {
                view: &view,
                runtime: &snapshot.block().runtime,
                clock: &SystemClock,
                spec: <RemoveStake as rtao_tx::IntentInterface>::SPEC,
                client: &client,
                dispatch_origin: ORIGIN,
                fee_payer: PAYER,
                call: &wrapped,
            })
            .await?;
        assert_eq!(preview.available_free, Some(Balance::<Tao>::ZERO));
        assert_eq!(
            preview.required_free,
            Some(Balance::<Tao>::from_rao(8_209_263))
        );
        assert!(
            preview
                .blocks
                .iter()
                .any(|block| block.contains("below the reserved claim fee"))
        );
        assert_eq!(view.block_hashes(), BTreeSet::from([snapshot.block().hash]));
        let request = chain
            .requests()
            .into_iter()
            .find(|request| {
                request.method == "state_call"
                    && request.params[0] == "TransactionPaymentApi_query_info"
            })
            .ok_or_else(|| anyhow::anyhow!("missing fee probe"))?;
        let bytes = hex::decode(
            request.params[1]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("fee probe bytes"))?
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
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn no_claim_warning_is_best_effort_and_only_plain_remove_adds_swap_quotes()
    -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture(|_| Ok(())).await?;
        let root = remove(
            Balance::<Tao>::from_rao(1).into(),
            NetUid::ROOT,
            false,
            false,
        )?;
        let built = build(&root, &snapshot).await?;
        let preview = inspect(&root, &client, &snapshot, &built.call).await?;
        assert_eq!(
            preview.warnings,
            [
                "τ0.000900000 remains claimable via `btcli root claim` or `--claim` (whole entitlement, not a slice of this unstake)"
            ]
        );
        chain.mutate_state(|state| state.queue_reply("state_call", Reply::result(json!("0x"))));
        assert!(
            inspect(&root, &client, &snapshot, &built.call)
                .await?
                .warnings
                .is_empty()
        );
        let alpha = remove(
            Balance::<Alpha>::new(NetUid(1), 1_000_000_000)?.into(),
            NetUid(1),
            false,
            false,
        )?;
        let built = build(&alpha, &snapshot).await?;
        let preview = inspect(&alpha, &client, &snapshot, &built.call).await?;
        assert_eq!(preview.facts_title, FactsTitle::Quote);
        assert_eq!(preview.facts[0], ("receive".into(), "~τ2.000000000".into()));
        let explicit = remove_limit(
            Balance::<Alpha>::new(NetUid(1), 1_000_000_000)?.into(),
            NetUid(1),
            false,
            1,
        );
        let built = build(&explicit, &snapshot).await?;
        let before = chain.requests().len();
        assert!(
            inspect(&explicit, &client, &snapshot, &built.call)
                .await?
                .facts
                .is_empty()
        );
        assert_eq!(chain.requests().len(), before);
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn transfer_variants_encode_native_amounts_and_all_resolves_position_instead_of_max()
    -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture(|_| Ok(())).await?;
        for (origin, amount, rao, hotkey) in [
            (NetUid(1), Amount::All, 100, None),
            (
                NetUid::ROOT,
                Balance::<Tao>::from_rao(u64::MAX).into(),
                u64::MAX,
                Some(DESTINATION),
            ),
            (
                NetUid(1),
                Balance::<Alpha>::new(NetUid(1), 1)?.into(),
                1,
                Some(HOTKEY),
            ),
        ] {
            let intent = transfer(amount, origin, NetUid(2), hotkey);
            let built = build(&intent, &snapshot).await?;
            let changed = hotkey.is_some_and(|key| key != HOTKEY);
            let mut expected = vec![7, if changed { 143 } else { 86 }];
            expected.extend_from_slice(RECEIVER.as_bytes());
            expected.extend_from_slice(HOTKEY.as_bytes());
            if changed {
                expected.extend_from_slice(DESTINATION.as_bytes());
            }
            expected.extend_from_slice(&origin.0.to_le_bytes());
            expected.extend_from_slice(&2u16.to_le_bytes());
            expected.extend_from_slice(&rao.to_le_bytes());
            assert_eq!(built.call.as_bytes(), expected);
            assert_eq!(intent.spend(), Spend::Unbounded);
            assert_eq!(intent.touches_netuids().as_slice(), [origin, NetUid(2)]);
            let before = chain.requests().len();
            let preview = inspect(&intent, &client, &snapshot, &built.call).await?;
            assert_eq!(chain.requests().len(), before);
            assert_eq!(
                preview.warnings[0],
                "transfers stake OWNERSHIP to another coldkey"
            );
        }
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn same_subnet_transfer_reports_position_lock_and_receiver_conflicts_in_order()
    -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture(|state| {
            for (coldkey, hotkey) in [(ORIGIN, SENDER_LOCK), (RECEIVER, RECEIVER_LOCK)] {
                state.seed_python(
                    "SubtensorModule",
                    "Lock",
                    &[
                        json!(coldkey.to_string()),
                        json!(1),
                        json!(hotkey.to_string()),
                    ],
                    json!({}),
                )?;
            }
            Ok(())
        })
        .await?;
        let intent = transfer(
            Balance::<Alpha>::new(NetUid(1), 150)?.into(),
            NetUid(1),
            NetUid(1),
            None,
        );
        let built = build(&intent, &snapshot).await?;
        let preview = inspect(&intent, &client, &snapshot, &built.call).await?;
        assert_eq!(preview.warnings,vec![
            "transfers stake OWNERSHIP to another coldkey".to_owned(),
            format!("origin hotkey {HOTKEY} holds only 0.000000100α₁ on netuid 1 — transfer will fail with NotEnoughStakeToWithdraw; move stake onto this hotkey first, or pass a different origin hotkey"),
            format!("your conviction lock on netuid 1 targets {SENDER_LOCK}, but stake is leaving {HOTKEY} (lock hotkey and stake hotkey can differ)"),
            "0.000000100α₁ locked alpha will move with this transfer (0.000000030α₁ is free; the rest pulls from the conviction lock)".into(),
            format!("receiver already locks to {RECEIVER_LOCK}; locked alpha can only land on that hotkey — pass --destination-hotkey {RECEIVER_LOCK} (current landing hotkey is {HOTKEY}) or transfer only the free amount"),
        ]);
        let matching = TransferStake {
            dest_hotkey_ss58: Some(RECEIVER_LOCK.into()),
            amount_alpha: Balance::<Alpha>::new(NetUid(1), 50)?.into(),
            ..intent
        };
        let built = build(&matching, &snapshot).await?;
        let preview = inspect(&matching, &client, &snapshot, &built.call).await?;
        assert_eq!(preview.warnings.len(), 3);
        assert!(preview.warnings[2].starts_with("0.000000020α₁ locked alpha"));
        let zero = TransferStake {
            amount_alpha: Balance::<Alpha>::new(NetUid(1), 0)?.into(),
            ..matching
        };
        let built = build(&zero, &snapshot).await?;
        chain.mutate_state(|state| state.queue_reply("state_call", Reply::result(json!("0x"))));
        assert!(
            inspect(&zero, &client, &snapshot, &built.call)
                .await
                .is_err()
        );
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn transfer_all_moves_only_locked_excess_and_warns_about_new_receiver_lock()
    -> anyhow::Result<()> {
        let (_chain, client, snapshot) = fixture(|state| {
            state.seed_python(
                "SubtensorModule",
                "Lock",
                &[
                    json!(ORIGIN.to_string()),
                    json!(1),
                    json!(SENDER_LOCK.to_string()),
                ],
                json!({}),
            )?;
            Ok(())
        })
        .await?;
        let intent = transfer(Amount::All, NetUid(1), NetUid(1), Some(DESTINATION));
        let built = build(&intent, &snapshot).await?;
        let preview = inspect(&intent, &client, &snapshot, &built.call).await?;
        assert_eq!(preview.warnings[1], "hands over the entire origin position");
        assert_eq!(
            preview.warnings[3],
            "0.000000070α₁ locked alpha will move with this transfer (0.000000030α₁ is free; the rest pulls from the conviction lock)"
        );
        assert_eq!(
            preview.warnings.last(),
            Some(&format!(
                "receiver has no lock yet; locked alpha will create one on landing hotkey {DESTINATION}"
            ))
        );
        assert_eq!(intent.touches_netuids().as_slice(), [NetUid(1), NetUid(1)]);
        let policy = Policy {
            max_spend: Some(Balance::<Tao>::from_rao(u64::MAX)),
            ..Policy::default()
        };
        assert_eq!(policy.check(&intent, None).len(), 1);
        client.close().await;
        Ok(())
    }

    #[test]
    fn availability_selector_preserves_integer_precedence_fallback_and_native_range()
    -> anyhow::Result<()> {
        let coldkey = ORIGIN.to_string();
        let native = |available: u64| {
            Value::record(vec![(
                "available".into(),
                Value::Int(i128::from(available)),
            )])
        };
        let raw = Value::Dict(vec![(
            Value::Str(coldkey.clone()),
            Value::Dict(vec![
                (Value::Int(1), native(7)),
                (Value::Str("1".into()), native(9)),
            ]),
        )]);
        assert_eq!(availability_row(&raw, &coldkey, NetUid(1))?, (0, 0, 7));
        let raw = Value::Dict(vec![(
            Value::Str(coldkey.clone()),
            Value::Dict(vec![
                (Value::Int(1), Value::Null),
                (Value::Str("1".into()), native(9)),
            ]),
        )]);
        assert_eq!(availability_row(&raw, &coldkey, NetUid(1))?, (0, 0, 9));
        assert_eq!(
            availability_row(&raw, &RECEIVER.to_string(), NetUid(1))?,
            (0, 0, 0)
        );
        let overflow = Value::Dict(vec![(
            Value::Str(coldkey.clone()),
            Value::Dict(vec![(
                Value::Int(1),
                Value::record(vec![(
                    "locked".into(),
                    Value::Int(i128::from(u64::MAX) + 1),
                )]),
            )]),
        )]);
        assert!(availability_row(&overflow, &coldkey, NetUid(1)).is_err());
        Ok(())
    }

    #[test]
    fn summaries_currency_validation_and_subnet_policy_keep_root_semantics() -> anyhow::Result<()> {
        let exit: RemoveStake = remove(Amount::All, NetUid::ROOT, true, true)?;
        assert_eq!(
            exit.summary(),
            format!(
                "claim basket yield then unstake ALL alpha from {HOTKEY} on netuid 0 (fails if price moves >5.00%)"
            )
        );
        assert_eq!(exit.touches_netuids().as_slice(), [NetUid::ROOT]);
        assert_eq!(exit.spend(), Spend::None);
        let limited: RemoveStakeLimit = remove_limit(
            Balance::<Tao>::from_rao(1_000_000_000).into(),
            NetUid::ROOT,
            false,
            7,
        );
        assert_eq!(
            limited.summary(),
            format!("unstake τ1.000000000 from {HOTKEY} on netuid 0 (limit 7 rao/alpha)")
        );
        let same: MoveStake = moved(Amount::All, NetUid::ROOT, NetUid::ROOT, true, true)?;
        assert_eq!(
            same.summary(),
            format!(
                "claim basket yield then move ALL alpha from {HOTKEY} on netuid 0 to {DESTINATION} on netuid 0"
            )
        );
        assert_eq!(
            same.touches_netuids().as_slice(),
            [NetUid::ROOT, NetUid::ROOT]
        );
        let sweep = UnstakeAll {
            hotkey_ss58: HOTKEY.into(),
            claim: false,
        };
        assert_eq!(sweep.summary(), format!("unstake ALL from {HOTKEY}"));
        assert!(sweep.affects_all_subnets());
        assert!(sweep.touches_netuids().is_empty());
        assert_eq!(sweep.spend(), Spend::None);
        let policy = Policy {
            allowed_netuids: Some(BTreeSet::from([NetUid::ROOT])),
            ..Policy::default()
        };
        assert_eq!(
            policy.check(&sweep, None),
            ["unstake_all acts on all subnets, not just allowed_netuids [0]"]
        );
        for intent in [
            Box::new(remove(
                Balance::<Tao>::ZERO.into(),
                NetUid(1),
                false,
                false,
            )?) as Box<dyn DynIntent>,
            Box::new(remove_limit(
                Balance::<Tao>::ZERO.into(),
                NetUid(1),
                false,
                0,
            )),
            Box::new(moved(
                Balance::<Tao>::ZERO.into(),
                NetUid(1),
                NetUid(2),
                false,
                false,
            )?),
            Box::new(transfer(
                Balance::<Tao>::ZERO.into(),
                NetUid(1),
                NetUid(2),
                None,
            )),
        ] {
            assert!(matches!(
                intent.validate_dyn(),
                Err(IntentError::UnitMismatch(UnitError::UnitMismatch {
                    expected: NetUid(1),
                    ..
                }))
            ));
        }
        let mut disabled = remove(
            Balance::<Alpha>::new(NetUid(1), 0)?.into(),
            NetUid(1),
            false,
            false,
        )?;
        disabled.rate_tolerance = Ratio::new(1.0)?;
        assert!(disabled.validate().is_err());
        let ownership: TransferStake = transfer(
            Balance::<Tao>::from_rao(1_000_000_000).into(),
            NetUid::ROOT,
            NetUid(1),
            Some(DESTINATION),
        );
        assert_eq!(
            ownership.summary(),
            format!(
                "transfer τ1.000000000 on netuid 0 to coldkey {RECEIVER} (landing on hotkey {DESTINATION})"
            )
        );
        Ok(())
    }
}
