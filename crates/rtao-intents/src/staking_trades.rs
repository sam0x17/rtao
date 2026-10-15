// Derived from RaoFoundation/subtensor sdk/python/bittensor/intents/staking.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Native staking and protected subnet swaps, composed at one pinned state.

use rtao_client::{View, ViewExt};
use rtao_primitives::{
    AccountId, AlphaBalance, Amount, DynBalance, NetUid, PriceRao, Ratio, ResolvedAmount, Spend,
    Ss58Address, TaoBalance, UnitError, pyfmt,
};
use rtao_runtime::{
    CallBytes,
    calls::{subtensor_module as calls, utility},
    runtime_apis::{stake_info_runtime_api as stake_api, swap_runtime_api as swap_api},
};
use rtao_tx::{BuildCx, Built, FactsTitle, Intent, IntentError, IntentPreflight, PreflightCx};
use smallvec::{SmallVec, smallvec};

pub use crate::{AddStake, AddStakeLimit, MoveSwapStake, SwapStake};

const SHARE_POOL_DUST_WARNING: &str = "same-subnet move can floor the dest position by 1 rao; the following swap uses amount-1 so the batch does not fail, and 1 rao stays on the dest hotkey at the origin subnet";
const ALL_STAKE_WARNING: &str =
    "stakes the entire free balance (minus the existential deposit and fee headroom)";

/// Constructor validation also applies when price protection is disabled.
pub(crate) fn check_rate_tolerance(tolerance: Ratio) -> Result<(), IntentError> {
    if !(0.0..1.0).contains(&tolerance.get()) {
        return Err(IntentError::validation(format!(
            "rate_tolerance must be a fraction in [0, 1), got {} (0.05 = 5%). To trade with no price bound, disable slippage protection (slippage_protection=False / --no-slippage-protection) instead.",
            pyfmt::float_repr(tolerance.get())
        )));
    }
    Ok(())
}

fn amount_summary(amount: &Amount, netuid: NetUid, all: &str) -> String {
    match amount.resolve(netuid, true) {
        Ok(ResolvedAmount::Exact(amount)) => amount.to_string(),
        Ok(ResolvedAmount::All) => all.into(),
        // Invalid typed inputs are refused by validate before execution.
        Err(_) => match amount {
            Amount::All => all.into(),
            Amount::Exact(amount) => amount.to_decimal_string(),
        },
    }
}

fn tolerance_note(tolerance: Ratio, protected: bool, ratio: bool) -> String {
    if !protected {
        return " (no slippage protection)".into();
    }
    let fraction = if ratio { " ratio" } else { "" };
    format!(
        " (fails if price{fraction} moves >{:.2}%)",
        tolerance.get() * 100.0
    )
}

/// Current spot price; transport and typed-decoding failures remain hard stops.
pub(crate) async fn alpha_price_rao<V: View + ?Sized>(
    view: &V,
    netuid: NetUid,
) -> Result<u64, IntentError> {
    Ok(view.call(&swap_api::CURRENT_ALPHA_PRICE, (netuid,)).await?)
}

// Python converts the entire integer to binary64 before multiplication and
// truncates the resulting float. In particular u64::MAX can round to 2^64;
// a saturating `as u64` would silently alter its encoded slippage limit.
fn scaled_price(price: u128, factor: f64) -> Result<PriceRao, IntentError> {
    let price = ((price as f64) * factor).trunc();
    if !price.is_finite() {
        return Err(UnitError::NonFinite.into());
    }
    if price < 0.0 {
        return Err(UnitError::Negative.into());
    }
    if price >= 18_446_744_073_709_551_616.0 {
        return Err(UnitError::Overflow.into());
    }
    Ok(PriceRao(price as u64))
}

/// Python's checked `int(price * (1 +/- tolerance))` for a single subnet.
pub(crate) fn limit_price_rao(
    price: u64,
    tolerance: Ratio,
    increase: bool,
) -> Result<PriceRao, IntentError> {
    check_rate_tolerance(tolerance)?;
    scaled_price(
        u128::from(price),
        if increase {
            1.0 + tolerance.get()
        } else {
            1.0 - tolerance.get()
        },
    )
}

/// Integer price ratio first, binary64 tolerance multiplication second.
pub(crate) async fn swap_limit_price_rao<V: View + ?Sized>(
    view: &V,
    origin: NetUid,
    destination: NetUid,
    tolerance: Ratio,
) -> Result<PriceRao, IntentError> {
    let origin_price = alpha_price_rao(view, origin).await?;
    let destination_price = alpha_price_rao(view, destination).await?;
    if destination_price == 0 {
        return Err(IntentError::validation(format!(
            "netuid {destination} has no alpha price; cannot derive a slippage limit — disable slippage protection to submit anyway"
        )));
    }
    // Both source prices are native u64, so the product fits u128. The
    // quotient may still exceed u64 before the tolerance is applied.
    let ratio = u128::from(origin_price) * 1_000_000_000 / u128::from(destination_price);
    scaled_price(ratio, 1.0 - tolerance.get())
}

/// Resolve an origin position for `all`, using the semantic dispatch coldkey.
pub(crate) async fn staked_rao<V: View + ?Sized>(
    view: &V,
    coldkey: AccountId,
    hotkey: &Ss58Address,
    netuid: NetUid,
    action: &str,
    allow_empty: bool,
) -> Result<u64, IntentError> {
    let position = view
        .call(
            &stake_api::GET_STAKE_INFO_FOR_HOTKEY_COLDKEY_NETUID,
            (hotkey.account(), coldkey, netuid),
        )
        .await?;
    let amount = position.map_or(0, |position| position.stake.0);
    if amount == 0 && !allow_empty {
        return Err(IntentError::validation(format!(
            "nothing to {action}: no stake on {hotkey} at netuid {netuid}"
        )));
    }
    Ok(amount)
}

/// Direction and native input for a best-effort pool simulation.
pub(crate) enum QuoteInput {
    /// Stake free TAO to receive subnet alpha.
    Tao(u64),
    /// Unstake subnet alpha to receive TAO.
    #[allow(dead_code)] // Shared with the separately ported remove-stake intents.
    Alpha(u64),
}

fn comma_four(value: f64) -> String {
    // Rust and Python fixed formatting round the exact binary64 value once,
    // with ties to even. Group the finished integer without further rounding.
    let fixed = format!("{value:.4}");
    let (integer, fraction) = fixed.split_once('.').unwrap_or((&fixed, "0000"));
    let mut result = String::with_capacity(fixed.len() + integer.len() / 3);
    for (index, digit) in integer.bytes().enumerate() {
        if index != 0 && (integer.len() - index) % 3 == 0 {
            result.push(',');
        }
        result.push(char::from(digit));
    }
    result.push('.');
    result.push_str(fraction);
    result
}

/// Best-effort receive/rate/fee/impact facts in Python's presentation order.
pub(crate) async fn swap_quote_facts<V: View + ?Sized>(
    view: &V,
    netuid: NetUid,
    input: QuoteInput,
) -> Result<Vec<(String, String)>, IntentError> {
    let (result, paid, staking) = match input {
        QuoteInput::Tao(amount) => (
            view.call(
                &swap_api::SIM_SWAP_TAO_FOR_ALPHA,
                (netuid, TaoBalance(amount)),
            )
            .await,
            amount,
            true,
        ),
        QuoteInput::Alpha(amount) => (
            view.call(
                &swap_api::SIM_SWAP_ALPHA_FOR_TAO,
                (netuid, AlphaBalance(amount)),
            )
            .await,
            amount,
            false,
        ),
    };
    let Ok(result) = result else {
        return Ok(vec![]);
    };
    let (received, received_netuid) = if staking {
        (result.alpha_amount.0, netuid)
    } else {
        (result.tao_amount.0, NetUid::ROOT)
    };
    if received == 0 {
        return Ok(vec![]);
    }
    let (numerator, denominator) = if staking {
        (paid, received)
    } else {
        (received, paid)
    };
    let rate = rtao_primitives::ratio::integer_division(i128::from(numerator), denominator)
        .ok_or_else(|| IntentError::validation("division by zero"))?;
    let mut facts = vec![
        (
            "receive".into(),
            format!("~{}", DynBalance::from_rao(received, received_netuid)),
        ),
        ("rate".into(), format!("{} τ/α", comma_four(rate))),
    ];
    if result.tao_fee.0 != 0 {
        facts.push((
            "swap fee".into(),
            format!("~{}", DynBalance::from_rao(result.tao_fee.0, NetUid::ROOT)),
        ));
    } else if result.alpha_fee.0 != 0 {
        facts.push((
            "swap fee".into(),
            format!("~{}", DynBalance::from_rao(result.alpha_fee.0, netuid)),
        ));
    }
    if result.tao_slippage.0 != 0 {
        facts.push((
            "slippage".into(),
            format!(
                "~{} price impact",
                DynBalance::from_rao(result.tao_slippage.0, NetUid::ROOT)
            ),
        ));
    } else if result.alpha_slippage.0 != 0 {
        facts.push((
            "slippage".into(),
            format!(
                "~{} price impact",
                DynBalance::from_rao(result.alpha_slippage.0, netuid)
            ),
        ));
    }
    Ok(facts)
}

/// Protected or plain cross-subnet swap on one hotkey.
pub(crate) async fn compose_swap_stake<V: View + ?Sized>(
    cx: &BuildCx<'_, V>,
    hotkey: AccountId,
    origin: NetUid,
    destination: NetUid,
    amount: u64,
    protected: bool,
    tolerance: Ratio,
) -> Result<CallBytes, IntentError> {
    let call = if protected {
        calls::swap_stake_limit(
            hotkey,
            origin,
            destination,
            AlphaBalance(amount),
            swap_limit_price_rao(cx.view, origin, destination, tolerance).await?,
            false,
        )
    } else {
        calls::swap_stake(hotkey, origin, destination, AlphaBalance(amount))
    };
    Ok(call.compose(cx.runtime)?)
}

impl Intent for AddStake {
    fn summary(&self) -> String {
        format!(
            "stake {} to {} on netuid {}{}",
            amount_summary(&self.amount_tao, NetUid::ROOT, "ALL free TAO"),
            self.hotkey_ss58,
            self.netuid,
            tolerance_note(self.rate_tolerance, self.slippage_protection, false)
        )
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        self.validate()?;
        let amount = crate::arguments::stakeable_amount(&self.amount_tao, cx).await?;
        let call = if self.slippage_protection {
            calls::add_stake_limit(
                self.hotkey_ss58.account(),
                self.netuid,
                amount,
                limit_price_rao(
                    alpha_price_rao(cx.view, self.netuid).await?,
                    self.rate_tolerance,
                    true,
                )?,
                false,
            )
        } else {
            calls::add_stake(self.hotkey_ss58.account(), self.netuid, amount)
        };
        Ok(Built::new(call.compose(cx.runtime)?))
    }

    async fn warnings<V: View + ?Sized>(
        &self,
        _cx: &PreflightCx<'_, V>,
    ) -> Result<Vec<String>, IntentError> {
        Ok(if matches!(self.amount_tao, Amount::All) {
            vec![ALL_STAKE_WARNING.into()]
        } else {
            vec![]
        })
    }

    async fn preflight<V: View + ?Sized>(
        &self,
        cx: &PreflightCx<'_, V>,
    ) -> Result<IntentPreflight, IntentError> {
        let mut preview = IntentPreflight {
            effects: self.effects(cx).await?,
            warnings: self.warnings(cx).await?,
            blocks: self.blocks(cx).await?,
            ..Default::default()
        };
        if self.netuid != NetUid::ROOT
            && let ResolvedAmount::Exact(amount) = self.amount_tao.resolve(NetUid::ROOT, true)?
        {
            let facts = swap_quote_facts(cx.view, self.netuid, QuoteInput::Tao(amount.rao)).await?;
            if !facts.is_empty() {
                preview.facts = facts;
                preview.facts_title = FactsTitle::Quote;
            }
        }
        Ok(preview)
    }

    fn spend(&self) -> Spend {
        crate::arguments::tao_amount(&self.amount_tao).map_or(Spend::Unbounded, Spend::Bounded)
    }

    fn validate(&self) -> Result<(), IntentError> {
        self.amount_tao.resolve(NetUid::ROOT, true)?;
        check_rate_tolerance(self.rate_tolerance)
    }
}

impl Intent for AddStakeLimit {
    fn summary(&self) -> String {
        format!(
            "stake {} to {} on netuid {} (limit {} rao/alpha)",
            amount_summary(&self.amount_tao, NetUid::ROOT, "ALL free TAO"),
            self.hotkey_ss58,
            self.netuid,
            self.limit_price_rao
        )
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        let amount = crate::arguments::stakeable_amount(&self.amount_tao, cx).await?;
        Ok(Built::new(
            calls::add_stake_limit(
                self.hotkey_ss58.account(),
                self.netuid,
                amount,
                PriceRao(self.limit_price_rao),
                self.allow_partial,
            )
            .compose(cx.runtime)?,
        ))
    }

    async fn warnings<V: View + ?Sized>(
        &self,
        _cx: &PreflightCx<'_, V>,
    ) -> Result<Vec<String>, IntentError> {
        Ok(if matches!(self.amount_tao, Amount::All) {
            vec![ALL_STAKE_WARNING.into()]
        } else {
            vec![]
        })
    }

    fn spend(&self) -> Spend {
        crate::arguments::tao_amount(&self.amount_tao).map_or(Spend::Unbounded, Spend::Bounded)
    }

    fn validate(&self) -> Result<(), IntentError> {
        self.amount_tao.resolve(NetUid::ROOT, true)?;
        Ok(())
    }
}

impl Intent for SwapStake {
    fn summary(&self) -> String {
        format!(
            "swap {} on {} from netuid {} to netuid {}{}",
            amount_summary(&self.amount_alpha, self.origin_netuid, "ALL alpha"),
            self.hotkey_ss58,
            self.origin_netuid,
            self.dest_netuid,
            tolerance_note(self.rate_tolerance, self.slippage_protection, true)
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
                    "swap",
                    false,
                )
                .await?
            }
        };
        Ok(Built::new(
            compose_swap_stake(
                cx,
                self.hotkey_ss58.account(),
                self.origin_netuid,
                self.dest_netuid,
                amount,
                self.slippage_protection,
                self.rate_tolerance,
            )
            .await?,
        ))
    }

    async fn warnings<V: View + ?Sized>(
        &self,
        _cx: &PreflightCx<'_, V>,
    ) -> Result<Vec<String>, IntentError> {
        Ok(if matches!(self.amount_alpha, Amount::All) {
            vec!["swaps the entire origin position".into()]
        } else {
            vec![]
        })
    }

    fn touches_netuids(&self) -> SmallVec<[NetUid; 2]> {
        smallvec![self.origin_netuid, self.dest_netuid]
    }

    fn validate(&self) -> Result<(), IntentError> {
        self.amount_alpha.resolve(self.origin_netuid, true)?;
        check_rate_tolerance(self.rate_tolerance)
    }
}

impl Intent for MoveSwapStake {
    fn summary(&self) -> String {
        format!(
            "move {} from {} to {} on netuid {}, then swap to netuid {}{}",
            amount_summary(&self.amount_alpha, self.origin_netuid, "ALL alpha"),
            self.origin_hotkey_ss58,
            self.dest_hotkey_ss58,
            self.origin_netuid,
            self.dest_netuid,
            tolerance_note(self.rate_tolerance, self.slippage_protection, true)
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
                    &self.origin_hotkey_ss58,
                    self.origin_netuid,
                    "move",
                    false,
                )
                .await?
            }
        };
        let moved = calls::move_stake(
            self.origin_hotkey_ss58.account(),
            self.dest_hotkey_ss58.account(),
            self.origin_netuid,
            self.origin_netuid,
            AlphaBalance(amount),
        )
        .compose(cx.runtime)?;
        let swap_amount = amount.checked_sub(1).filter(|amount| *amount > 0).ok_or_else(|| {
            IntentError::validation(format!(
                "cannot move-then-swap {amount} rao: same-subnet move_stake can lose 1 rao to share-pool rounding, leaving nothing to swap. Move a larger amount, or change only the hotkey or only the subnet"
            ))
        })?;
        let swapped = compose_swap_stake(
            cx,
            self.dest_hotkey_ss58.account(),
            self.origin_netuid,
            self.dest_netuid,
            swap_amount,
            self.slippage_protection,
            self.rate_tolerance,
        )
        .await?;
        Ok(Built::new(
            utility::batch_all(vec![moved, swapped]).compose(cx.runtime)?,
        ))
    }

    async fn warnings<V: View + ?Sized>(
        &self,
        _cx: &PreflightCx<'_, V>,
    ) -> Result<Vec<String>, IntentError> {
        let mut warnings = vec![SHARE_POOL_DUST_WARNING.into()];
        if matches!(self.amount_alpha, Amount::All) {
            warnings.push("moves the entire origin position".into());
        }
        Ok(warnings)
    }

    fn touches_netuids(&self) -> SmallVec<[NetUid; 2]> {
        smallvec![self.origin_netuid, self.dest_netuid]
    }

    fn validate(&self) -> Result<(), IntentError> {
        // Python compares the supplied address spellings at construction.
        if self.origin_hotkey_ss58 == self.dest_hotkey_ss58 {
            return Err(IntentError::validation(
                "move_swap_stake needs a different dest hotkey; use swap_stake when only the subnet changes",
            ));
        }
        if self.origin_netuid == self.dest_netuid {
            return Err(IntentError::validation(
                "move_swap_stake needs a different dest subnet; use move_stake when only the hotkey changes",
            ));
        }
        self.amount_alpha.resolve(self.origin_netuid, true)?;
        check_rate_tolerance(self.rate_tolerance)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rtao_client::{BlockRef, Client, ClientConfig, DynView, Network, Snapshot};
    use rtao_primitives::{Alpha, Balance, Tao};
    use rtao_runtime::RuntimeHandle;
    use rtao_testkit::{FakeChain, FakeState, Reply, rpc::RpcClient};
    use rtao_tx::{DynIntent, OriginView, Policy, SystemClock};
    use serde_json::{Value, json};
    use std::collections::BTreeSet;

    const ORIGIN: AccountId = AccountId([7; 32]);
    const HOTKEY: AccountId = AccountId([8; 32]);
    const DESTINATION: AccountId = AccountId([9; 32]);

    async fn fixture() -> anyhow::Result<(FakeChain, Client, Snapshot)> {
        let mut state = FakeState::try_default()?;
        state.seed_python(
            "System",
            "Account",
            &[json!(ORIGIN.to_string())],
            json!({"data":{"free":2_000_500_100u64}}),
        )?;
        state.seed_constant("Balances", "ExistentialDeposit", json!(100))?;
        state.seed_runtime(
            "SwapRuntimeApi",
            "current_alpha_price",
            json!(2_000_000_000u64),
        )?;
        for (netuid, price) in [(0u16, 1_000_000_000u64), (2, 1_000_000_000)] {
            state.seed_runtime_encoded(
                "SwapRuntimeApi_current_alpha_price",
                netuid.to_le_bytes().to_vec(),
                price.to_le_bytes().to_vec(),
            );
        }
        state.seed_runtime(
            "StakeInfoRuntimeApi",
            "get_stake_info_for_hotkey_coldkey_netuid",
            json!({"stake":5_000_000_003u64}),
        )?;
        state.seed_runtime(
            "SwapRuntimeApi",
            "sim_swap_tao_for_alpha",
            json!({"alpha_amount":500_000_000u64,"tao_fee":2,"alpha_fee":3,"tao_slippage":4,"alpha_slippage":5}),
        )?;
        state.seed_runtime(
            "SwapRuntimeApi",
            "sim_swap_alpha_for_tao",
            json!({"tao_amount":2_000_000_000u64,"alpha_fee":3,"alpha_slippage":5}),
        )?;
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

    async fn preflight(
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
                fee_payer: DESTINATION,
                call,
            })
            .await
    }

    fn decode(call: &CallBytes, runtime: &RuntimeHandle) -> anyhow::Result<Value> {
        let value = runtime
            .core()
            .decode_spec(&rtao_core::runtime::TypeSpec::Call, call.as_bytes(), true)
            .map_err(rtao_core::CoreError::from)?;
        Ok(rtao_core::codec::value::to_corpus_json(&value))
    }

    fn add_bytes(index: u8, netuid: NetUid, amount: u64, limit: Option<(u64, bool)>) -> Vec<u8> {
        let mut bytes = vec![7, index];
        bytes.extend_from_slice(HOTKEY.as_bytes());
        bytes.extend_from_slice(&netuid.0.to_le_bytes());
        bytes.extend_from_slice(&amount.to_le_bytes());
        if let Some((price, partial)) = limit {
            bytes.extend_from_slice(&price.to_le_bytes());
            bytes.push(u8::from(partial));
        }
        bytes
    }

    fn swap_bytes(
        hotkey: AccountId,
        origin: NetUid,
        destination: NetUid,
        amount: u64,
        limit: Option<u64>,
    ) -> Vec<u8> {
        let mut bytes = vec![7, if limit.is_some() { 90 } else { 87 }];
        bytes.extend_from_slice(hotkey.as_bytes());
        bytes.extend_from_slice(&origin.0.to_le_bytes());
        bytes.extend_from_slice(&destination.0.to_le_bytes());
        bytes.extend_from_slice(&amount.to_le_bytes());
        if let Some(price) = limit {
            bytes.extend_from_slice(&price.to_le_bytes());
            bytes.push(0);
        }
        bytes
    }

    fn add(amount: Amount, netuid: NetUid, protected: bool) -> anyhow::Result<AddStake> {
        Ok(AddStake {
            hotkey_ss58: HOTKEY.into(),
            netuid,
            amount_tao: amount,
            slippage_protection: protected,
            rate_tolerance: Ratio::new(0.05)?,
        })
    }

    fn swap(amount: Amount, origin: NetUid, protected: bool) -> anyhow::Result<SwapStake> {
        Ok(SwapStake {
            hotkey_ss58: HOTKEY.into(),
            origin_netuid: origin,
            dest_netuid: NetUid(2),
            amount_alpha: amount,
            slippage_protection: protected,
            rate_tolerance: Ratio::new(0.05)?,
        })
    }

    fn move_swap(amount: Amount, origin: NetUid, protected: bool) -> anyhow::Result<MoveSwapStake> {
        Ok(MoveSwapStake {
            origin_hotkey_ss58: HOTKEY.into(),
            origin_netuid: origin,
            dest_hotkey_ss58: DESTINATION.into(),
            dest_netuid: NetUid(2),
            amount_alpha: amount,
            slippage_protection: protected,
            rate_tolerance: Ratio::new(0.05)?,
        })
    }

    // pyt: unit/test_slippage_protection.py::TestDefaultProtection::test_add_stake_composes_limit_call
    // pyt: unit/test_slippage_protection.py::TestDefaultProtection::test_custom_tolerance_moves_the_limit
    // pyt: unit/test_slippage_protection.py::TestDisabledProtection::test_disabled_composes_plain_call
    #[tokio::test]
    async fn add_stake_encodes_default_custom_and_explicit_limits_and_unprotected_u64()
    -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture().await?;
        let amount: Amount = Balance::<Tao>::from_rao(1_000_000_001).into();
        for (protected, tolerance, expected) in [
            (
                true,
                0.05,
                add_bytes(88, NetUid(1), 1_000_000_001, Some((2_100_000_000, false))),
            ),
            (
                true,
                0.10,
                add_bytes(88, NetUid(1), 1_000_000_001, Some((2_200_000_000, false))),
            ),
            (false, 0.05, add_bytes(2, NetUid(1), 1_000_000_001, None)),
        ] {
            let mut intent = add(amount.clone(), NetUid(1), protected)?;
            intent.rate_tolerance = Ratio::new(tolerance)?;
            let before = chain.requests().len();
            let built = build(&intent, &snapshot).await?;
            assert_eq!(built.call.as_bytes(), expected);
            assert_eq!(
                CallBytes::decode(&expected, &snapshot.block().runtime)?,
                built.call
            );
            assert!(built.extras.is_empty());
            assert_eq!(
                intent.spend(),
                Spend::Bounded(Balance::<Tao>::from_rao(1_000_000_001))
            );
            assert_eq!(intent.touches_netuids().as_slice(), [NetUid(1)]);
            if !protected {
                assert_eq!(chain.requests().len(), before);
            }
        }
        let full = add(
            Balance::<Tao>::from_rao(u64::MAX).into(),
            NetUid::ROOT,
            false,
        )?;
        assert_eq!(
            build(&full, &snapshot).await?.call.as_bytes(),
            add_bytes(2, NetUid::ROOT, u64::MAX, None)
        );
        let explicit = AddStakeLimit {
            hotkey_ss58: HOTKEY.into(),
            netuid: NetUid(1),
            amount_tao: amount,
            limit_price_rao: u64::MAX,
            allow_partial: true,
        };
        let before = chain.requests().len();
        let built = build(&explicit, &snapshot).await?;
        assert_eq!(
            built.call.as_bytes(),
            add_bytes(88, NetUid(1), 1_000_000_001, Some((u64::MAX, true)))
        );
        assert_eq!(chain.requests().len(), before);
        assert_eq!(
            explicit.summary(),
            format!(
                "stake τ1.000000001 to {HOTKEY} on netuid 1 (limit {} rao/alpha)",
                u64::MAX
            )
        );
        let preview = preflight(&explicit, &client, &snapshot, &built.call).await?;
        assert!(preview.facts.is_empty());
        assert!(preview.warnings.is_empty());
        assert_eq!(chain.requests().len(), before);
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn all_stake_uses_effective_origin_headroom_and_skips_quote() -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture().await?;
        for intent in [
            Box::new(add(Amount::All, NetUid(1), true)?) as Box<dyn DynIntent>,
            Box::new(AddStakeLimit {
                hotkey_ss58: HOTKEY.into(),
                netuid: NetUid::ROOT,
                amount_tao: Amount::All,
                limit_price_rao: 1_000_000_000,
                allow_partial: false,
            }),
        ] {
            let built = build(intent.as_ref(), &snapshot).await?;
            let decoded = decode(&built.call, &snapshot.block().runtime)?;
            assert_eq!(decoded["call_args"][2]["value"], json!(2_000_000_000u64));
            let before = chain.requests().len();
            let preview = preflight(intent.as_ref(), &client, &snapshot, &built.call).await?;
            assert_eq!(preview.effects, [intent.summary_dyn()]);
            assert_eq!(preview.warnings, [ALL_STAKE_WARNING]);
            assert!(preview.facts.is_empty());
            assert_eq!(chain.requests().len(), before);
            assert_eq!(intent.spend(), Spend::Unbounded);
            let policy = Policy {
                max_spend: Some(Balance::<Tao>::from_rao(u64::MAX)),
                ..Policy::default()
            };
            assert_eq!(policy.check(intent.as_ref(), None).len(), 1);
        }
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        client.close().await;
        Ok(())
    }

    // pyt: unit/test_slippage_protection.py::TestDefaultProtection::test_swap_stake_composes_limit_call
    #[tokio::test]
    async fn swaps_resolve_all_against_effective_coldkey_and_support_root_currency()
    -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture().await?;
        for (origin, amount, rao, protected, limit) in [
            (
                NetUid(1),
                Balance::<Alpha>::new(NetUid(1), 1_000_000_001)?.into(),
                1_000_000_001,
                true,
                Some(1_900_000_000),
            ),
            (
                NetUid(1),
                Amount::All,
                5_000_000_003,
                true,
                Some(1_900_000_000),
            ),
            (
                NetUid::ROOT,
                Balance::<Tao>::from_rao(u64::MAX).into(),
                u64::MAX,
                false,
                None,
            ),
        ] {
            let intent = swap(amount, origin, protected)?;
            let before = chain.requests().len();
            let built = build(&intent, &snapshot).await?;
            assert_eq!(
                built.call.as_bytes(),
                swap_bytes(HOTKEY, origin, NetUid(2), rao, limit)
            );
            let preview = preflight(&intent, &client, &snapshot, &built.call).await?;
            assert!(preview.facts.is_empty());
            assert_eq!(intent.spend(), Spend::None);
            assert_eq!(intent.touches_netuids().as_slice(), [origin, NetUid(2)]);
            if matches!(intent.amount_alpha, Amount::All) {
                assert_eq!(preview.warnings, ["swaps the entire origin position"]);
                let mut expected = Vec::new();
                expected.extend_from_slice(HOTKEY.as_bytes());
                expected.extend_from_slice(ORIGIN.as_bytes());
                expected.extend_from_slice(&origin.0.to_le_bytes());
                let requests = chain.requests();
                let request = requests[before..]
                    .iter()
                    .find(|request| {
                        request.method == "state_call"
                            && request.params[0]
                                == "StakeInfoRuntimeApi_get_stake_info_for_hotkey_coldkey_netuid"
                    })
                    .ok_or_else(|| anyhow::anyhow!("position read missing"))?;
                assert_eq!(
                    request.params[1],
                    json!(format!("0x{}", hex::encode(expected)))
                );
                assert_eq!(
                    request.params[2],
                    json!(format!("0x{}", hex::encode(snapshot.block().hash.0)))
                );
            } else {
                assert!(preview.warnings.is_empty());
                if !protected {
                    assert_eq!(chain.requests().len(), before);
                }
            }
        }
        // Python leaves SameNetuid to chain dispatch for SwapStake.
        let same = swap(
            Balance::<Alpha>::new(NetUid(2), 1)?.into(),
            NetUid(2),
            false,
        )?;
        same.validate()?;
        assert_eq!(
            build(&same, &snapshot).await?.call.as_bytes(),
            swap_bytes(HOTKEY, NetUid(2), NetUid(2), 1, None)
        );
        client.close().await;
        Ok(())
    }

    // pyt: unit/test_slippage_protection.py::TestDefaultProtection::test_move_swap_stake_is_the_named_batch
    // pyt: unit/test_slippage_protection.py::TestDisabledProtection::test_unprotected_move_swap_still_batches_plain_swap
    #[tokio::test]
    async fn move_then_swap_is_atomic_ordered_and_leaves_exactly_one_rao_of_dust()
    -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture().await?;
        for (origin, amount, rao, protected, limit) in [
            (
                NetUid(1),
                Balance::<Alpha>::new(NetUid(1), 2)?.into(),
                2,
                true,
                Some(1_900_000_000),
            ),
            (NetUid(1), Amount::All, 5_000_000_003, false, None),
            (
                NetUid::ROOT,
                Balance::<Tao>::from_rao(u64::MAX).into(),
                u64::MAX,
                true,
                Some(950_000_000),
            ),
        ] {
            let intent = move_swap(amount, origin, protected)?;
            let built = build(&intent, &snapshot).await?;
            let mut expected = vec![11, 2, 8]; // Utility.batch_all, two calls.
            expected.extend_from_slice(&[7, 85]);
            expected.extend_from_slice(HOTKEY.as_bytes());
            expected.extend_from_slice(DESTINATION.as_bytes());
            expected.extend_from_slice(&origin.0.to_le_bytes());
            expected.extend_from_slice(&origin.0.to_le_bytes());
            expected.extend_from_slice(&rao.to_le_bytes());
            expected.extend_from_slice(&swap_bytes(DESTINATION, origin, NetUid(2), rao - 1, limit));
            assert_eq!(built.call.as_bytes(), expected);
            assert_eq!(
                CallBytes::decode(&expected, &snapshot.block().runtime)?,
                built.call
            );
            assert!(built.extras.is_empty());
            let preview = preflight(&intent, &client, &snapshot, &built.call).await?;
            assert_eq!(
                preview.warnings.first().map(String::as_str),
                Some(SHARE_POOL_DUST_WARNING)
            );
            assert_eq!(
                preview.warnings.len(),
                if matches!(intent.amount_alpha, Amount::All) {
                    2
                } else {
                    1
                }
            );
            assert_eq!(intent.spend(), Spend::None);
            assert_eq!(intent.touches_netuids().as_slice(), [origin, NetUid(2)]);
        }
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn empty_positions_and_move_dust_refuse_before_price_reads() -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture().await?;
        for rao in [0, 1] {
            let intent = move_swap(
                Balance::<Alpha>::new(NetUid(1), rao)?.into(),
                NetUid(1),
                true,
            )?;
            let before = chain.requests().len();
            assert_eq!(
                build(&intent, &snapshot)
                    .await
                    .err()
                    .map(|error| error.to_string()),
                Some(format!(
                    "cannot move-then-swap {rao} rao: same-subnet move_stake can lose 1 rao to share-pool rounding, leaving nothing to swap. Move a larger amount, or change only the hotkey or only the subnet"
                ))
            );
            assert_eq!(chain.requests().len(), before);
        }
        chain.mutate_state(|state| {
            state.seed_runtime(
                "StakeInfoRuntimeApi",
                "get_stake_info_for_hotkey_coldkey_netuid",
                Value::Null,
            )
        })?;
        for (intent, action) in [
            (
                Box::new(swap(Amount::All, NetUid(1), true)?) as Box<dyn DynIntent>,
                "swap",
            ),
            (Box::new(move_swap(Amount::All, NetUid(1), true)?), "move"),
        ] {
            let before = chain.requests().len();
            assert_eq!(
                build(intent.as_ref(), &snapshot)
                    .await
                    .err()
                    .map(|error| error.to_string()),
                Some(format!(
                    "nothing to {action}: no stake on {HOTKEY} at netuid 1"
                ))
            );
            let requests = chain.requests();
            assert_eq!(
                requests[before..]
                    .iter()
                    .filter(|request| request.method == "state_call")
                    .count(),
                1
            );
            assert!(
                !requests[before..]
                    .iter()
                    .any(|request| request.params[0] == "SwapRuntimeApi_current_alpha_price")
            );
        }
        client.close().await;
        Ok(())
    }

    #[test]
    fn constructor_order_summaries_and_subnet_policy_match_python() -> anyhow::Result<()> {
        let mut bad_add = add(
            Balance::<Alpha>::new(NetUid(1), 1)?.into(),
            NetUid(1),
            false,
        )?;
        bad_add.rate_tolerance = Ratio::new(-1.0)?;
        assert!(matches!(
            bad_add.validate(),
            Err(IntentError::UnitMismatch(UnitError::UnitMismatch {
                expected: NetUid::ROOT,
                ..
            }))
        ));
        let mut bad_swap = swap(Balance::<Tao>::ZERO.into(), NetUid(1), false)?;
        bad_swap.rate_tolerance = Ratio::new(-1.0)?;
        assert!(matches!(
            bad_swap.validate(),
            Err(IntentError::UnitMismatch(UnitError::UnitMismatch {
                expected: NetUid(1),
                ..
            }))
        ));
        for bad in [-1.0, 1.0, 2.0] {
            let mut intent = add(Balance::<Tao>::ZERO.into(), NetUid(1), false)?;
            intent.rate_tolerance = Ratio::new(bad)?;
            assert_eq!(
                intent.validate().err().map(|error| error.to_string()),
                Some(format!(
                    "rate_tolerance must be a fraction in [0, 1), got {} (0.05 = 5%). To trade with no price bound, disable slippage protection (slippage_protection=False / --no-slippage-protection) instead.",
                    pyfmt::float_repr(bad)
                ))
            );
        }
        let mut moved = move_swap(Balance::<Tao>::ZERO.into(), NetUid(1), false)?;
        moved.dest_hotkey_ss58 = moved.origin_hotkey_ss58.clone();
        moved.dest_netuid = moved.origin_netuid;
        moved.rate_tolerance = Ratio::new(-1.0)?;
        assert_eq!(moved.validate().err().map(|error|error.to_string()),Some("move_swap_stake needs a different dest hotkey; use swap_stake when only the subnet changes".into()));
        moved.dest_hotkey_ss58 = DESTINATION.into();
        assert_eq!(moved.validate().err().map(|error|error.to_string()),Some("move_swap_stake needs a different dest subnet; use move_stake when only the hotkey changes".into()));
        moved.dest_netuid = NetUid(2);
        assert!(matches!(
            moved.validate(),
            Err(IntentError::UnitMismatch(_))
        ));
        moved.amount_alpha = Balance::<Alpha>::new(NetUid(1), 1)?.into();
        moved.rate_tolerance = Ratio::new(0.05)?;
        moved.dest_hotkey_ss58 = HOTKEY.to_ss58(2).parse()?;
        moved.validate()?; // Python compares the supplied SS58 spellings.
        let mut negative_zero = add(
            Balance::<Tao>::from_rao(1_000_000_000).into(),
            NetUid(1),
            true,
        )?;
        negative_zero.rate_tolerance = Ratio::new(-0.0)?;
        assert_eq!(
            negative_zero.summary(),
            format!("stake τ1.000000000 to {HOTKEY} on netuid 1 (fails if price moves >-0.00%)")
        );
        let swapped = swap(Amount::All, NetUid::ROOT, true)?;
        assert_eq!(
            swapped.summary(),
            format!(
                "swap ALL alpha on {HOTKEY} from netuid 0 to netuid 2 (fails if price ratio moves >5.00%)"
            )
        );
        let moved = move_swap(Amount::All, NetUid(1), false)?;
        assert_eq!(
            moved.summary(),
            format!(
                "move ALL alpha from {HOTKEY} to {DESTINATION} on netuid 1, then swap to netuid 2 (no slippage protection)"
            )
        );
        let policy = Policy {
            allowed_netuids: Some(BTreeSet::from([NetUid(1)])),
            ..Policy::default()
        };
        assert_eq!(
            policy.check(&moved, None),
            ["netuid 2 not in allowed_netuids [1]"]
        );
        Ok(())
    }

    #[test]
    fn limit_math_rounds_binary64_once_and_checks_native_boundary() -> anyhow::Result<()> {
        assert_eq!(
            limit_price_rao(2_000_000_000, Ratio::new(0.05)?, true)?,
            PriceRao(2_100_000_000)
        );
        assert_eq!(
            limit_price_rao(2_000_000_000, Ratio::new(0.05)?, false)?,
            PriceRao(1_900_000_000)
        );
        assert!(matches!(
            limit_price_rao(u64::MAX, Ratio::new(0.0)?, true),
            Err(IntentError::UnitMismatch(UnitError::Overflow))
        ));
        // Ratio may exceed u64 before the floating tolerance brings it back
        // into range. Python's int -> binary64 -> multiply order is retained.
        assert_eq!(
            scaled_price(u128::from(u64::MAX) * 1_000_000_000, 2.0f64.powi(-30))?,
            PriceRao(17_179_869_184_000_000_000)
        );
        assert_eq!(comma_four(1234.5), "1,234.5000");
        assert_eq!(comma_four(0.000_05), "0.0001");
        assert_eq!(comma_four(2.0), "2.0000");
        Ok(())
    }

    #[tokio::test]
    async fn ratio_limits_floor_before_tolerance_and_fail_closed_on_prices() -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture().await?;
        chain.mutate_state(|state| {
            state.seed_runtime_encoded(
                "SwapRuntimeApi_current_alpha_price",
                2u16.to_le_bytes().to_vec(),
                3_000_000_000u64.to_le_bytes().to_vec(),
            )
        });
        assert_eq!(
            swap_limit_price_rao(&snapshot, NetUid(1), NetUid(2), Ratio::new(0.05)?).await?,
            PriceRao(633_333_332)
        );
        chain.mutate_state(|state| {
            state.seed_runtime_encoded(
                "SwapRuntimeApi_current_alpha_price",
                2u16.to_le_bytes().to_vec(),
                0u64.to_le_bytes().to_vec(),
            )
        });
        let intent = swap(Balance::<Alpha>::new(NetUid(1), 1)?.into(), NetUid(1), true)?;
        assert_eq!(build(&intent,&snapshot).await.err().map(|error|error.to_string()),Some("netuid 2 has no alpha price; cannot derive a slippage limit — disable slippage protection to submit anyway".into()));
        let plain = SwapStake {
            slippage_protection: false,
            ..intent
        };
        let before = chain.requests().len();
        assert!(build(&plain, &snapshot).await.is_ok());
        assert_eq!(chain.requests().len(), before);
        chain.mutate_state(|state| {
            state.seed_runtime_encoded(
                "SwapRuntimeApi_current_alpha_price",
                1u16.to_le_bytes().to_vec(),
                0u64.to_le_bytes().to_vec(),
            );
            state.seed_runtime_encoded(
                "SwapRuntimeApi_current_alpha_price",
                2u16.to_le_bytes().to_vec(),
                1_000_000_000u64.to_le_bytes().to_vec(),
            );
        });
        assert_eq!(
            swap_limit_price_rao(&snapshot, NetUid(1), NetUid(2), Ratio::new(0.05)?).await?,
            PriceRao(0)
        );
        chain.mutate_state(|state| state.queue_reply("state_call", Reply::result(json!("0x"))));
        let protected = add(Balance::<Tao>::from_rao(1).into(), NetUid(1), true)?;
        assert!(build(&protected, &snapshot).await.is_err());
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn quote_facts_keep_direction_fee_priority_and_exact_integer_rate() -> anyhow::Result<()>
    {
        let (chain, client, snapshot) = fixture().await?;
        let intent = add(
            Balance::<Tao>::from_rao(1_000_000_000).into(),
            NetUid(1),
            true,
        )?;
        let built = build(&intent, &snapshot).await?;
        let preview = preflight(&intent, &client, &snapshot, &built.call).await?;
        assert_eq!(preview.effects, [intent.summary()]);
        assert_eq!(preview.facts_title, FactsTitle::Quote);
        assert_eq!(
            preview.facts,
            vec![
                ("receive".into(), "~0.500000000α₁".into()),
                ("rate".into(), "2.0000 τ/α".into()),
                ("swap fee".into(), "~τ0.000000002".into()),
                ("slippage".into(), "~τ0.000000004 price impact".into()),
            ]
        );
        assert!(preview.warnings.is_empty());
        let alpha =
            swap_quote_facts(&snapshot, NetUid(1), QuoteInput::Alpha(1_000_000_000)).await?;
        assert_eq!(
            alpha,
            vec![
                ("receive".into(), "~τ2.000000000".into()),
                ("rate".into(), "2.0000 τ/α".into()),
                ("swap fee".into(), "~0.000000003α₁".into()),
                ("slippage".into(), "~0.000000005α₁ price impact".into()),
            ]
        );
        chain.mutate_state(|state| {
            state.seed_runtime(
                "SwapRuntimeApi",
                "sim_swap_tao_for_alpha",
                json!({"alpha_amount":3u64}),
            )
        })?;
        let large =
            swap_quote_facts(&snapshot, NetUid(1), QuoteInput::Tao(9_007_199_254_740_995)).await?;
        assert_eq!(
            large[1],
            ("rate".into(), "3,002,399,751,580,331.5000 τ/α".into())
        );
        assert_eq!(
            comma_four(9_007_199_254_740_995u64 as f64 / 3.0),
            "3,002,399,751,580,332.0000"
        );
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn quote_failures_and_zero_output_are_best_effort_but_root_never_quotes()
    -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture().await?;
        let intent = add(Balance::<Tao>::from_rao(1).into(), NetUid(1), false)?;
        let built = build(&intent, &snapshot).await?;
        chain.mutate_state(|state| state.queue_reply("state_call", Reply::result(json!("0x"))));
        let preview = preflight(&intent, &client, &snapshot, &built.call).await?;
        assert!(preview.facts.is_empty());
        assert_eq!(preview.facts_title, FactsTitle::Fees);
        assert_eq!(preview.effects, [intent.summary()]);
        chain.mutate_state(|state| {
            state.seed_runtime(
                "SwapRuntimeApi",
                "sim_swap_tao_for_alpha",
                json!({"alpha_amount":0}),
            )
        })?;
        assert!(
            preflight(&intent, &client, &snapshot, &built.call)
                .await?
                .facts
                .is_empty()
        );
        let root = add(Balance::<Tao>::from_rao(1).into(), NetUid::ROOT, false)?;
        let built = build(&root, &snapshot).await?;
        let before = chain.requests().len();
        assert!(
            preflight(&root, &client, &snapshot, &built.call)
                .await?
                .facts
                .is_empty()
        );
        assert_eq!(chain.requests().len(), before);
        // Preserve Python's explicit failure for an internally inconsistent
        // alpha simulation: it divides by its zero input outside the catch.
        assert!(
            swap_quote_facts(&snapshot, NetUid(1), QuoteInput::Alpha(0))
                .await
                .is_err()
        );
        client.close().await;
        Ok(())
    }
}
