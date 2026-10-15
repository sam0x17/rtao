// Derived from RaoFoundation/subtensor sdk/python/bittensor/intents/staking.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Basket deposits and rebalances, reward routing and alpha consolidation.

use rtao_client::{View, ViewExt};
use rtao_primitives::{AlphaBalance, Amount, DynBalance, NetUid, ResolvedAmount, Spend, pyfmt};
use rtao_runtime::{
    calls::subtensor_module as calls, runtime_apis::beta_basket_runtime_api as api,
};
use rtao_tx::{BuildCx, Built, Intent, IntentError, PreflightCx};
use serde_json::{Value, json};
use smallvec::{SmallVec, smallvec};

pub use crate::{SetAutoStake, StakeIntoBasket, SwapBasket, SwapBasketMany, UnstakeAllAlpha};

fn amount_summary(amount: &Amount, netuid: NetUid, all: &str) -> String {
    match amount.resolve(netuid, true) {
        Ok(ResolvedAmount::Exact(amount)) => amount.to_string(),
        Ok(ResolvedAmount::All) => all.into(),
        // Invalid programmatic values are rejected by validate before execution.
        Err(_) => match amount {
            Amount::All => all.into(),
            Amount::Exact(amount) => amount.to_decimal_string(),
        },
    }
}

fn exact_rao(amount: &Amount, netuid: NetUid) -> Result<u64, IntentError> {
    match amount.resolve(netuid, false)? {
        ResolvedAmount::Exact(amount) => Ok(amount.rao),
        ResolvedAmount::All => Err(rtao_primitives::UnitError::AllNotAllowed.into()),
    }
}

impl Intent for SetAutoStake {
    fn summary(&self) -> String {
        let target = self
            .hotkey_ss58
            .as_ref()
            .map_or_else(|| "the wallet hotkey".into(), ToString::to_string);
        format!("auto-stake rewards on netuid {} to {target}", self.netuid)
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        crate::generated::passthrough::set_auto_stake(self, cx).await
    }
}

impl Intent for UnstakeAllAlpha {
    fn summary(&self) -> String {
        format!("unstake ALL alpha from {}", self.hotkey_ss58)
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        crate::generated::passthrough::unstake_all_alpha(self, cx).await
    }

    async fn warnings<V: View + ?Sized>(
        &self,
        _cx: &PreflightCx<'_, V>,
    ) -> Result<Vec<String>, IntentError> {
        Ok(vec![
            "removes all alpha stake from this hotkey across every subnet".into(),
        ])
    }

    fn affects_all_subnets(&self) -> bool {
        true
    }
}

impl Intent for StakeIntoBasket {
    fn summary(&self) -> String {
        format!(
            "buy into {}'s basket with {}",
            self.hotkey_ss58,
            amount_summary(&self.amount_tao, NetUid::ROOT, "ALL free TAO")
        )
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        let amount = crate::arguments::stakeable_amount(&self.amount_tao, cx).await?;
        Ok(Built::new(
            calls::stake_into_basket(self.hotkey_ss58.account(), amount).compose(cx.runtime)?,
        ))
    }

    async fn warnings<V: View + ?Sized>(
        &self,
        _cx: &PreflightCx<'_, V>,
    ) -> Result<Vec<String>, IntentError> {
        Ok(if matches!(self.amount_tao, Amount::All) {
            vec![
                "deploys the entire free balance (minus the existential deposit and fee headroom)"
                    .into(),
            ]
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

impl Intent for SwapBasket {
    fn summary(&self) -> String {
        let amount = amount_summary(&self.amount, self.origin_netuid, "the whole holding");
        let note = match self.min_amount_out.resolve(self.dest_netuid, false) {
            Ok(ResolvedAmount::Exact(floor)) if floor.rao > 0 => {
                format!(" (at least {floor} out)")
            }
            _ => String::new(),
        };
        format!(
            "rebalance {}'s basket: sell {amount} on netuid {} to buy netuid {}{note}",
            self.hotkey_ss58, self.origin_netuid, self.dest_netuid
        )
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        // Keep constructor refusals before the all-holding runtime lookup.
        self.validate()?;
        let amount = match self.amount.resolve(self.origin_netuid, true)? {
            ResolvedAmount::Exact(amount) => amount.rao,
            ResolvedAmount::All => {
                let rows = cx
                    .view
                    .call(&api::GET_VALIDATOR_BASKET, (self.hotkey_ss58.account(),))
                    .await?;
                let amount = rows
                    .into_iter()
                    .find(|(netuid, _, _)| *netuid == self.origin_netuid)
                    .map_or(0, |(_, amount, _)| amount.0);
                if amount == 0 {
                    return Err(IntentError::validation(format!(
                        "nothing to sell: {}'s basket holds nothing on netuid {}",
                        self.hotkey_ss58, self.origin_netuid
                    )));
                }
                amount
            }
        };
        Ok(Built::new(
            calls::swap_basket(
                self.hotkey_ss58.account(),
                self.origin_netuid,
                self.dest_netuid,
                AlphaBalance(amount),
                exact_rao(&self.min_amount_out, self.dest_netuid)?,
            )
            .compose(cx.runtime)?,
        ))
    }

    fn touches_netuids(&self) -> SmallVec<[NetUid; 2]> {
        smallvec![self.origin_netuid, self.dest_netuid]
    }

    fn validate(&self) -> Result<(), IntentError> {
        if self.origin_netuid == self.dest_netuid {
            return Err(IntentError::validation(
                "swap_basket: origin and destination netuid must differ",
            ));
        }
        self.amount.resolve(self.origin_netuid, true)?;
        self.min_amount_out.resolve(self.dest_netuid, false)?;
        Ok(())
    }
}

/// One native, ordered runtime basket trade.
pub(crate) type BasketLeg = (NetUid, NetUid, AlphaBalance, u64);

struct NormalizedLeg {
    origin: NetUid,
    destination: NetUid,
    amount: DynBalance,
    floor: DynBalance,
}

fn string_list(values: impl IntoIterator<Item = String>) -> String {
    let mut values: Vec<_> = values.into_iter().collect();
    values.sort();
    format!(
        "[{}]",
        values
            .iter()
            .map(|value| pyfmt::string_repr(value))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

// Python calls int() on JSON leg netuids before normalizing their amounts.
// Keep that coercion (including bools, Unicode digits and truncating floats),
// followed by the native identifier check used by other money fields.
fn leg_netuid(value: &Value, index: usize, field: &str) -> Result<NetUid, IntentError> {
    let raw = match value {
        Value::Bool(value) => Some(u16::from(u8::from(*value))),
        Value::String(value) => {
            pyfmt::parse_integer(value).and_then(|integer| integer.to_string().parse::<u16>().ok())
        }
        Value::Number(value) if !value.as_str().contains(['.', 'e', 'E']) => {
            pyfmt::parse_integer(value.as_str())
                .and_then(|integer| integer.to_string().parse::<u16>().ok())
        }
        Value::Number(value) => pyfmt::parse_float(value.as_str())
            .filter(|value| value.is_finite())
            .map(f64::trunc)
            .filter(|value| *value >= 0.0 && *value <= f64::from(u16::MAX))
            .map(|value| value as u16),
        _ => None,
    }
    .ok_or_else(|| {
        IntentError::validation(format!(
            "swap_basket_many: leg {index} {field} must be a native subnet identifier"
        ))
    })?;
    Ok(NetUid::new(raw)?)
}

fn leg_amount(value: &Value, netuid: NetUid) -> Result<DynBalance, IntentError> {
    // Clone only scalar values; malformed nested JSON never enters recursive
    // deserialization or cloning. Python's Decimal(str(bool)) rejects bools.
    let scalar = match value {
        Value::String(value) => Value::String(value.clone()),
        Value::Number(value) => Value::Number(value.clone()),
        _ => {
            return Err(IntentError::validation(format!(
                "alpha amount for netuid {netuid} must be a finite number"
            )));
        }
    };
    let amount: Amount = serde_json::from_value(scalar)?;
    if matches!(amount, Amount::All) {
        return Err(IntentError::validation(format!(
            "alpha amount for netuid {netuid} does not accept 'all'."
        )));
    }
    match amount.resolve(netuid, false)? {
        ResolvedAmount::Exact(amount) => Ok(amount),
        ResolvedAmount::All => Err(rtao_primitives::UnitError::AllNotAllowed.into()),
    }
}

fn decimal_amount(amount: &DynBalance) -> String {
    // Python's format(Decimal(rao) / 1e9, "f") does not retain insignificant
    // fractional zeroes. Nine digits remain available before this trimming.
    amount
        .to_decimal_string()
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_owned()
}

fn normalized_legs(values: &[Value]) -> Result<Vec<NormalizedLeg>, IntentError> {
    if values.is_empty() {
        return Err(IntentError::validation(
            "swap_basket_many: at least one leg is required",
        ));
    }
    if values.len() > 128 {
        return Err(IntentError::validation(
            "swap_basket_many: at most 128 legs are allowed",
        ));
    }
    values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let fields = value.as_object().ok_or_else(|| {
                IntentError::validation(format!("swap_basket_many: leg {index} must be an object"))
            })?;
            let required = ["origin_netuid", "dest_netuid", "amount"];
            let missing: Vec<_> = required
                .into_iter()
                .filter(|name| !fields.contains_key(*name))
                .map(str::to_owned)
                .collect();
            let extra: Vec<_> = fields
                .keys()
                .filter(|name| {
                    !matches!(
                        name.as_str(),
                        "origin_netuid" | "dest_netuid" | "amount" | "min_amount_out"
                    )
                })
                .cloned()
                .collect();
            if !missing.is_empty() || !extra.is_empty() {
                let mut detail = Vec::new();
                if !missing.is_empty() {
                    detail.push(format!("missing {}", string_list(missing)));
                }
                if !extra.is_empty() {
                    detail.push(format!("unknown {}", string_list(extra)));
                }
                return Err(IntentError::validation(format!(
                    "swap_basket_many: leg {index} has {}",
                    detail.join(", ")
                )));
            }
            let origin = leg_netuid(&fields["origin_netuid"], index, "origin_netuid")?;
            let destination = leg_netuid(&fields["dest_netuid"], index, "dest_netuid")?;
            if origin == destination {
                return Err(IntentError::validation(format!(
                    "swap_basket_many: leg {index} origin and destination must differ"
                )));
            }
            Ok(NormalizedLeg {
                origin,
                destination,
                amount: leg_amount(&fields["amount"], origin)?,
                floor: leg_amount(
                    fields.get("min_amount_out").unwrap_or(&json!(0)),
                    destination,
                )?,
            })
        })
        .collect()
}

/// Normalize Python's constructor-owned leg dictionaries in their input order.
pub(crate) fn canonical_legs(values: &mut [Value]) -> Result<(), IntentError> {
    let normalized = normalized_legs(values)?;
    for (value, leg) in values.iter_mut().zip(normalized) {
        *value = json!({
            "origin_netuid": leg.origin.0,
            "dest_netuid": leg.destination.0,
            "amount": decimal_amount(&leg.amount),
            "min_amount_out": decimal_amount(&leg.floor),
        });
    }
    Ok(())
}

/// Checked ordered tuples consumed by the generated `swap_basket_many` binding.
pub(crate) fn legs(values: &[Value]) -> Result<Vec<BasketLeg>, IntentError> {
    normalized_legs(values).map(|legs| {
        legs.into_iter()
            .map(|leg| {
                (
                    leg.origin,
                    leg.destination,
                    AlphaBalance(leg.amount.rao),
                    leg.floor.rao,
                )
            })
            .collect()
    })
}

/// Match constructor normalization for programmatically assembled leg inputs.
pub(crate) fn many_json(intent: &SwapBasketMany) -> Value {
    let mut value = crate::args_with_op("swap_basket_many", intent);
    if let Ok(normalized) = normalized_legs(&intent.legs) {
        value["legs"] = Value::Array(
            normalized
                .into_iter()
                .map(|leg| {
                    json!({
                        "origin_netuid": leg.origin.0,
                        "dest_netuid": leg.destination.0,
                        "amount": decimal_amount(&leg.amount),
                        "min_amount_out": decimal_amount(&leg.floor),
                    })
                })
                .collect(),
        );
    }
    value
}

impl Intent for SwapBasketMany {
    fn summary(&self) -> String {
        format!(
            "rebalance {}'s basket atomically across {} legs",
            self.hotkey_ss58,
            self.legs.len()
        )
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        crate::generated::passthrough::swap_basket_many(self, cx).await
    }

    fn touches_netuids(&self) -> SmallVec<[NetUid; 2]> {
        let mut touched = SmallVec::new();
        if let Ok(legs) = normalized_legs(&self.legs) {
            for netuid in legs
                .into_iter()
                .flat_map(|leg| [leg.origin, leg.destination])
            {
                if !touched.contains(&netuid) {
                    touched.push(netuid);
                }
            }
        }
        touched
    }

    fn validate(&self) -> Result<(), IntentError> {
        normalized_legs(&self.legs)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rtao_client::{BlockRef, Client, ClientConfig, DynView, Network, Snapshot};
    use rtao_primitives::{AccountId, Alpha, Balance, Tao};
    use rtao_runtime::{CallBytes, RuntimeHandle};
    use rtao_signer::{KeyView, SignerError};
    use rtao_testkit::{FakeChain, FakeState, rpc::RpcClient};
    use rtao_tx::{DynIntent, IntentInterface, OriginView, Policy, SystemClock};
    use std::collections::BTreeSet;

    const ORIGIN: AccountId = AccountId([7; 32]);
    const HOTKEY: AccountId = AccountId([8; 32]);

    struct PublicKeys;
    impl KeyView for PublicKeys {
        fn hotkey_address(&self) -> Result<AccountId, SignerError> {
            Ok(HOTKEY)
        }
        fn hotkey_public_key(&self) -> Result<[u8; 32], SignerError> {
            Ok(HOTKEY.0)
        }
        fn coldkey_address(&self) -> Result<AccountId, SignerError> {
            Ok(AccountId([9; 32]))
        }
    }

    async fn fixture(
        free: u64,
        deposit: u64,
        holdings: Value,
    ) -> anyhow::Result<(FakeChain, Client, Snapshot)> {
        let mut state = FakeState::try_default()?;
        state.seed_python(
            "System",
            "Account",
            &[json!(ORIGIN.to_string())],
            json!({"data":{"free":free}}),
        )?;
        state.seed_python(
            "System",
            "Account",
            &[json!(AccountId([9; 32]).to_string())],
            json!({"data":{"free":u64::MAX}}),
        )?;
        state.seed_constant("Balances", "ExistentialDeposit", json!(deposit))?;
        state.seed_runtime("BetaBasketRuntimeApi", "get_validator_basket", holdings)?;
        let chain = FakeChain::from_state(state);
        let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
        config.track_heads = false;
        config.runtime_cache_dir = None;
        config.config_store = None;
        let client = Client::from_rpc(RpcClient::new(chain.clone()), config).await?;
        let snapshot = client.at(BlockRef::Finalized).await?;
        Ok((chain, client, snapshot))
    }

    fn decode(call: &CallBytes, runtime: &RuntimeHandle) -> anyhow::Result<Value> {
        let value = runtime
            .core()
            .decode_spec(&rtao_core::runtime::TypeSpec::Call, call.as_bytes(), true)
            .map_err(rtao_core::CoreError::from)?;
        Ok(rtao_core::codec::value::to_corpus_json(&value))
    }

    async fn build(
        intent: &dyn DynIntent,
        snapshot: &Snapshot,
        origin: OriginView<'_>,
    ) -> Result<Built, IntentError> {
        intent
            .build_dyn(&BuildCx {
                view: snapshot as &dyn DynView,
                origin,
                runtime: &snapshot.block().runtime,
                clock: &SystemClock,
                spec: intent.spec(),
            })
            .await
    }

    fn call_prefix(index: u8) -> Vec<u8> {
        let mut bytes = vec![7, index];
        bytes.extend_from_slice(HOTKEY.as_bytes());
        bytes
    }

    #[tokio::test]
    async fn routing_and_consolidation_encode_public_keys_without_state_reads() -> anyhow::Result<()>
    {
        let (chain, client, snapshot) = fixture(0, 0, json!([])).await?;
        let requests = chain.requests().len();
        for explicit in [false, true] {
            let spelling = HOTKEY.to_ss58(2);
            let intent = SetAutoStake {
                netuid: NetUid(3),
                hotkey_ss58: explicit.then(|| spelling.parse()).transpose()?,
            };
            let built = build(
                &intent,
                &snapshot,
                if explicit {
                    OriginView::Account(ORIGIN)
                } else {
                    OriginView::Keys(&PublicKeys)
                },
            )
            .await?;
            // Pinned dispatch index and SCALE field widths, independent of the builder.
            let mut expected = vec![7, 114, 3, 0];
            expected.extend_from_slice(HOTKEY.as_bytes());
            assert_eq!(built.call.as_bytes(), expected);
            assert!(built.extras.is_empty());
            assert_eq!(
                intent.summary(),
                format!(
                    "auto-stake rewards on netuid 3 to {}",
                    if explicit {
                        &spelling
                    } else {
                        "the wallet hotkey"
                    }
                )
            );
            assert_eq!(intent.touches_netuids().as_slice(), [NetUid(3)]);
            assert_eq!(intent.spend(), Spend::None);
            assert_eq!(
                intent.to_json()["hotkey_ss58"],
                if explicit {
                    json!(spelling)
                } else {
                    Value::Null
                }
            );
        }
        let sweep = UnstakeAllAlpha {
            hotkey_ss58: HOTKEY.into(),
        };
        let built = build(&sweep, &snapshot, OriginView::Account(ORIGIN)).await?;
        assert_eq!(built.call.as_bytes(), call_prefix(84));
        assert!(sweep.affects_all_subnets());
        assert!(sweep.touches_netuids().is_empty());
        assert_eq!(sweep.spend(), Spend::None);
        let preview = sweep
            .preflight(&PreflightCx {
                view: &snapshot,
                runtime: &snapshot.block().runtime,
                clock: &SystemClock,
                spec: UnstakeAllAlpha::SPEC,
                client: &client,
                dispatch_origin: ORIGIN,
                fee_payer: ORIGIN,
                call: &built.call,
            })
            .await?;
        assert_eq!(
            preview.effects,
            [format!("unstake ALL alpha from {HOTKEY}")]
        );
        assert_eq!(
            preview.warnings,
            ["removes all alpha stake from this hotkey across every subnet"]
        );
        assert_eq!(chain.requests().len(), requests);
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn deposit_keeps_full_u64_amount_and_all_resolves_effective_origin() -> anyhow::Result<()>
    {
        let (chain, client, snapshot) = fixture(2_000_500_101, 100, json!([])).await?;
        for (amount, rao) in [
            (Balance::<Tao>::from_rao(u64::MAX).into(), u64::MAX),
            (Amount::All, 2_000_000_001),
        ] {
            let intent = StakeIntoBasket {
                hotkey_ss58: HOTKEY.into(),
                amount_tao: amount,
            };
            intent.validate()?;
            let before = chain.requests().len();
            let built = build(&intent, &snapshot, OriginView::Account(ORIGIN)).await?;
            let mut expected = call_prefix(147);
            expected.extend_from_slice(&rao.to_le_bytes());
            assert_eq!(built.call.as_bytes(), expected);
            assert_eq!(
                CallBytes::decode(&expected, &snapshot.block().runtime)?,
                built.call
            );
            let preview = intent
                .preflight(&PreflightCx {
                    view: &snapshot,
                    runtime: &snapshot.block().runtime,
                    clock: &SystemClock,
                    spec: StakeIntoBasket::SPEC,
                    client: &client,
                    dispatch_origin: ORIGIN,
                    fee_payer: AccountId([9; 32]),
                    call: &built.call,
                })
                .await?;
            assert_eq!(preview.effects, [intent.summary()]);
            if matches!(intent.amount_tao, Amount::All) {
                assert_eq!(intent.spend(), Spend::Unbounded);
                assert_eq!(
                    preview.warnings,
                    [
                        "deploys the entire free balance (minus the existential deposit and fee headroom)"
                    ]
                );
                assert!(
                    chain.requests()[before..]
                        .iter()
                        .any(|request| request.method == "state_getStorage")
                );
                assert_eq!(
                    intent.summary(),
                    format!("buy into {HOTKEY}'s basket with ALL free TAO")
                );
            } else {
                assert_eq!(
                    intent.spend(),
                    Spend::Bounded(Balance::<Tao>::from_rao(u64::MAX))
                );
                assert!(preview.warnings.is_empty());
                assert_eq!(chain.requests().len(), before);
            }
            assert!(intent.touches_netuids().is_empty());
        }
        let bad = StakeIntoBasket {
            hotkey_ss58: HOTKEY.into(),
            amount_tao: Balance::<Alpha>::new(NetUid(3), 1)?.into(),
        };
        assert!(bad.validate().is_err());
        assert_eq!(bad.spend(), Spend::Unbounded);
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn all_deposit_refuses_exhausted_ed_and_fee_headroom() -> anyhow::Result<()> {
        for free in [0, 99, 100, 500_099, 500_100] {
            let (_chain, client, snapshot) = fixture(free, 100, json!([])).await?;
            let intent = StakeIntoBasket {
                hotkey_ss58: HOTKEY.into(),
                amount_tao: Amount::All,
            };
            let error = build(&intent, &snapshot, OriginView::Account(ORIGIN))
                .await
                .err()
                .ok_or_else(|| anyhow::anyhow!("empty all-deposit unexpectedly built"))?;
            assert_eq!(
                error.to_string(),
                format!(
                    "nothing to stake: free balance {} does not cover the existential deposit plus the transaction fee",
                    DynBalance::from_rao(free, NetUid::ROOT)
                )
            );
            client.close().await;
        }
        Ok(())
    }

    #[tokio::test]
    async fn single_rebalance_preserves_root_alpha_units_and_resolves_only_all()
    -> anyhow::Result<()> {
        let (chain, client, snapshot) =
            fixture(0, 0, json!([[0, u64::MAX, 0], [3, 17, 0]])).await?;
        for (origin, destination, amount, expected_amount, floor) in [
            (NetUid::ROOT, NetUid(3), Amount::All, u64::MAX, 11),
            (
                NetUid(3),
                NetUid::ROOT,
                Balance::<Alpha>::new(NetUid(3), 9)?.into(),
                9,
                u64::MAX,
            ),
        ] {
            let floor = DynBalance::from_rao(floor, destination);
            let intent = SwapBasket {
                hotkey_ss58: HOTKEY.into(),
                origin_netuid: origin,
                dest_netuid: destination,
                amount,
                min_amount_out: floor.clone().into(),
            };
            let before = chain.requests().len();
            let built = build(&intent, &snapshot, OriginView::Account(ORIGIN)).await?;
            let mut expected = call_prefix(150);
            expected.extend_from_slice(&origin.0.to_le_bytes());
            expected.extend_from_slice(&destination.0.to_le_bytes());
            expected.extend_from_slice(&expected_amount.to_le_bytes());
            expected.extend_from_slice(&floor.rao.to_le_bytes());
            assert_eq!(built.call.as_bytes(), expected);
            let decoded = decode(&built.call, &snapshot.block().runtime)?;
            assert_eq!(decoded["call_function"], "swap_basket");
            assert_eq!(decoded["call_args"][3]["value"], json!(expected_amount));
            assert_eq!(decoded["call_args"][4]["value"], json!(floor.rao));
            assert_eq!(intent.touches_netuids().as_slice(), [origin, destination]);
            assert_eq!(intent.spend(), Spend::None);
            assert!(
                intent
                    .summary()
                    .ends_with(&format!(" (at least {floor} out)"))
            );
            if matches!(intent.amount, Amount::All) {
                assert!(intent.summary().contains("sell the whole holding"));
                assert!(
                    chain.requests()[before..]
                        .iter()
                        .any(|request| request.method == "state_call")
                );
            } else {
                assert_eq!(chain.requests().len(), before);
            }
        }
        let missing = SwapBasket {
            hotkey_ss58: HOTKEY.into(),
            origin_netuid: NetUid(4),
            dest_netuid: NetUid::ROOT,
            amount: Amount::All,
            min_amount_out: Balance::<Tao>::ZERO.into(),
        };
        assert_eq!(
            build(&missing, &snapshot, OriginView::Account(ORIGIN))
                .await
                .err()
                .map(|error| error.to_string()),
            Some(format!(
                "nothing to sell: {HOTKEY}'s basket holds nothing on netuid 4"
            ))
        );
        let invalid = SwapBasket {
            dest_netuid: missing.origin_netuid,
            ..missing
        };
        let before = chain.requests().len();
        assert_eq!(
            build(&invalid, &snapshot, OriginView::Account(ORIGIN))
                .await
                .err()
                .map(|error| error.to_string()),
            Some("swap_basket: origin and destination netuid must differ".into())
        );
        assert_eq!(chain.requests().len(), before);
        client.close().await;
        Ok(())
    }

    #[test]
    fn basket_leg_constructor_normalizes_exact_money_and_keeps_input_order() -> anyhow::Result<()> {
        let mut values = vec![
            json!({"origin_netuid":" +٣ ","dest_netuid":0.9,"amount":"9007199.254740993","min_amount_out":"18446744073.709551615"}),
            json!({"origin_netuid":false,"dest_netuid":true,"amount":1}),
            json!({"origin_netuid":1,"dest_netuid":3,"amount":"0.000000001","min_amount_out":0}),
        ];
        let expected = vec![
            (
                NetUid(3),
                NetUid::ROOT,
                AlphaBalance(9_007_199_254_740_993),
                u64::MAX,
            ),
            (NetUid::ROOT, NetUid(1), AlphaBalance(1_000_000_000), 0),
            (NetUid(1), NetUid(3), AlphaBalance(1), 0),
        ];
        assert_eq!(legs(&values)?, expected);
        canonical_legs(&mut values)?;
        assert_eq!(
            values,
            vec![
                json!({"origin_netuid":3,"dest_netuid":0,"amount":"9007199.254740993","min_amount_out":"18446744073.709551615"}),
                json!({"origin_netuid":0,"dest_netuid":1,"amount":"1","min_amount_out":"0"}),
                json!({"origin_netuid":1,"dest_netuid":3,"amount":"0.000000001","min_amount_out":"0"}),
            ]
        );
        assert_eq!(legs(&values)?, expected);
        let intent = SwapBasketMany {
            hotkey_ss58: HOTKEY.into(),
            legs: values.clone(),
        };
        assert_eq!(
            intent.touches_netuids().as_slice(),
            [NetUid(3), NetUid::ROOT, NetUid(1)]
        );
        assert_eq!(many_json(&intent)["legs"], json!(values));
        let tool = crate::AnyIntent::from_tool(
            "swap_basket_many",
            json!({"hotkey_ss58": HOTKEY.to_string(), "legs": values}),
        )?;
        assert_eq!(tool.to_json()["legs"], many_json(&intent)["legs"]);
        assert_eq!(
            crate::AnyIntent::from_tool("swap_basket_many", tool.to_json())?.to_json(),
            tool.to_json()
        );
        assert_eq!(intent.spend(), Spend::None);
        assert_eq!(
            intent.summary(),
            format!("rebalance {HOTKEY}'s basket atomically across 3 legs")
        );
        let policy = Policy {
            allowed_netuids: Some(BTreeSet::from([NetUid(3)])),
            ..Policy::default()
        };
        assert_eq!(
            policy.check(&intent, None),
            [
                "netuid 0 not in allowed_netuids [3]",
                "netuid 1 not in allowed_netuids [3]"
            ]
        );
        Ok(())
    }

    #[test]
    fn basket_leg_constructor_refuses_invalid_shapes_and_amounts_in_order() -> anyhow::Result<()> {
        let valid = json!({"origin_netuid":1,"dest_netuid":0,"amount":0});
        assert_eq!(
            legs(&[]).err().map(|error| error.to_string()),
            Some("swap_basket_many: at least one leg is required".into())
        );
        assert_eq!(
            legs(&vec![valid.clone(); 129])
                .err()
                .map(|error| error.to_string()),
            Some("swap_basket_many: at most 128 legs are allowed".into())
        );
        assert_eq!(legs(&vec![valid.clone(); 128])?.len(), 128);
        for (bad, expected) in [
            (json!(null), "swap_basket_many: leg 0 must be an object"),
            (
                json!({"z":1,"a":2}),
                "swap_basket_many: leg 0 has missing ['amount', 'dest_netuid', 'origin_netuid'], unknown ['a', 'z']",
            ),
            (
                json!({"origin_netuid":1,"dest_netuid":1,"amount":"bad"}),
                "swap_basket_many: leg 0 origin and destination must differ",
            ),
            (
                json!({"origin_netuid":1,"dest_netuid":0,"amount":"all"}),
                "alpha amount for netuid 1 does not accept 'all'.",
            ),
        ] {
            assert_eq!(
                legs(&[bad]).err().map(|error| error.to_string()),
                Some(expected.into())
            );
        }
        for bad in [
            json!(-1),
            json!("18446744073.709551616"),
            json!("NaN"),
            json!("Infinity"),
            json!([]),
            json!({}),
            json!(null),
            json!(true),
            json!(false),
        ] {
            let mut bad_leg = valid.clone();
            bad_leg["amount"] = bad;
            assert!(legs(&[bad_leg]).is_err());
        }
        let floor_all =
            json!({"origin_netuid":0,"dest_netuid":1,"amount":0,"min_amount_out":"all"});
        assert!(legs(&[floor_all]).is_err());
        let wrong_unit = SwapBasket {
            hotkey_ss58: HOTKEY.into(),
            origin_netuid: NetUid(3),
            dest_netuid: NetUid::ROOT,
            amount: Balance::<Tao>::ZERO.into(),
            min_amount_out: Balance::<Tao>::ZERO.into(),
        };
        assert!(wrong_unit.validate().is_err());
        let wrong_floor = SwapBasket {
            amount: Balance::<Alpha>::new(NetUid(3), 0)?.into(),
            min_amount_out: Balance::<Alpha>::new(NetUid(3), 0)?.into(),
            ..wrong_unit
        };
        assert!(wrong_floor.validate().is_err());
        Ok(())
    }

    #[tokio::test]
    async fn multi_rebalance_encodes_ordered_native_tuples_without_state_reads()
    -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture(0, 0, json!([])).await?;
        let intent = SwapBasketMany {
            hotkey_ss58: HOTKEY.into(),
            legs: vec![
                json!({"origin_netuid":3,"dest_netuid":0,"amount":"18446744073.709551615","min_amount_out":"0.000000001"}),
                json!({"origin_netuid":0,"dest_netuid":1,"amount":"0.000000002"}),
            ],
        };
        let requests = chain.requests().len();
        let built = build(&intent, &snapshot, OriginView::Account(ORIGIN)).await?;
        let mut expected = call_prefix(151);
        expected.push(8); // SCALE compact length: two trade tuples.
        for (origin, destination, amount, floor) in [(3u16, 0u16, u64::MAX, 1u64), (0, 1, 2, 0)] {
            expected.extend_from_slice(&origin.to_le_bytes());
            expected.extend_from_slice(&destination.to_le_bytes());
            expected.extend_from_slice(&amount.to_le_bytes());
            expected.extend_from_slice(&floor.to_le_bytes());
        }
        assert_eq!(built.call.as_bytes(), expected);
        assert_eq!(
            CallBytes::decode(&expected, &snapshot.block().runtime)?,
            built.call
        );
        assert!(built.extras.is_empty());
        assert_eq!(chain.requests().len(), requests);
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        client.close().await;
        Ok(())
    }
}
