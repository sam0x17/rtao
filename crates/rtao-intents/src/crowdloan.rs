// Derived from RaoFoundation/subtensor sdk/python/bittensor/intents/crowdloan.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Crowdloan funding, settlement and creator updates.

use crate::arguments::check_nested_json as check_call_budget;
#[cfg(test)]
use crate::nested_json::{MAX_CALL_DEPTH, check_call_limits, inspect_call_limits};
use rtao_client::View;
use rtao_primitives::{Amount, DynBalance, NetUid, Spend, TaoBalance};
use rtao_runtime::calls::crowdloan;
use rtao_tx::{BuildCx, Built, Intent, IntentError, PreflightCx};
use serde_json::Value;

pub use crate::generated::{
    ContributeCrowdloan, CreateCrowdloan, DissolveCrowdloan, FinalizeCrowdloan, RefundCrowdloan,
    SetCrowdloanMaxContribution, UpdateCrowdloanCap, UpdateCrowdloanEnd,
    UpdateCrowdloanMinContribution, WithdrawCrowdloan,
};

fn amount_summary(amount: &Amount) -> String {
    match crate::arguments::tao_amount(amount) {
        Ok(amount) => DynBalance::from(amount).to_string(),
        // Constructor validation rejects invalid typed inputs before execution.
        Err(_) => match amount {
            Amount::All => "all".into(),
            Amount::Exact(amount) => amount.to_decimal_string(),
        },
    }
}

fn bounded_spend(amount: &Amount) -> Spend {
    crate::arguments::tao_amount(amount).map_or(Spend::Unbounded, Spend::Bounded)
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64() != Some(0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
    }
}

/// Bound tool input before generated serde conversion recurses into `call`.
pub(crate) fn check_constructor_input(args: &Value) -> Result<(), IntentError> {
    if let Some(call) = args.get("call") {
        check_call_budget(call)?;
    }
    Ok(())
}

impl Intent for CreateCrowdloan {
    fn summary(&self) -> String {
        format!(
            "create crowdloan (cap {}, deposit {}, end {})",
            amount_summary(&self.cap_tao),
            amount_summary(&self.deposit_tao),
            self.end
        )
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        self.validate()?;
        let inner = if let Some(call) = self.call.as_ref().filter(|value| truthy(value)) {
            let fields = call.as_object().ok_or_else(|| {
                IntentError::validation("crowdloan 'call' must be an intent object")
            })?;
            let op = fields
                .get("op")
                .and_then(Value::as_str)
                .filter(|op| !op.is_empty())
                .ok_or_else(|| IntentError::validation("crowdloan 'call' needs an 'op' key"))?;
            // validate() bounds the tree before this recursive Value clone.
            let mut args = fields.clone();
            args.remove("op");
            let intent = crate::AnyIntent::from_tool(op, Value::Object(args))?;
            let borrowed = cx.view;
            let inner_cx = BuildCx {
                view: &borrowed as &dyn rtao_client::DynView,
                origin: cx.origin,
                runtime: cx.runtime,
                clock: cx.clock,
                spec: intent.spec(),
            };
            // Python invokes the nested semantic build directly. Executor
            // sudo/proxy adapters and nested build extras do not enter create.
            Some(intent.as_dyn().build_dyn(&inner_cx).await?.call)
        } else {
            None
        };
        Ok(Built::new(
            crowdloan::create(
                crate::arguments::tao_amount(&self.deposit_tao)?.to_wire(),
                crate::arguments::tao_amount(&self.min_contribution_tao)?.to_wire(),
                crate::arguments::tao_amount(&self.cap_tao)?.to_wire(),
                self.end,
                inner,
                self.target_ss58.as_ref().map(|target| target.account()),
            )
            .compose(cx.runtime)?,
        ))
    }

    fn spend(&self) -> Spend {
        bounded_spend(&self.deposit_tao)
    }

    fn validate(&self) -> Result<(), IntentError> {
        let has_call = self.call.as_ref().is_some_and(truthy);
        if self.target_ss58.is_some() == has_call {
            return Err(IntentError::validation(
                "provide exactly one of target_ss58 or call",
            ));
        }
        crate::arguments::tao_amount(&self.deposit_tao)?;
        crate::arguments::tao_amount(&self.min_contribution_tao)?;
        crate::arguments::tao_amount(&self.cap_tao)?;
        if let Some(call) = &self.call {
            check_call_budget(call)?;
        }
        Ok(())
    }
}

/// Resolve a contribution through the shared exact/all-free TAO helper.
pub(crate) async fn stakeable_amount<V: View + ?Sized>(
    intent: &ContributeCrowdloan,
    cx: &BuildCx<'_, V>,
) -> Result<TaoBalance, IntentError> {
    crate::arguments::stakeable_amount(&intent.amount_tao, cx).await
}

impl Intent for ContributeCrowdloan {
    fn summary(&self) -> String {
        let amount = if matches!(self.amount_tao, Amount::All) {
            "ALL free TAO".into()
        } else {
            amount_summary(&self.amount_tao)
        };
        format!("contribute {amount} to crowdloan {}", self.crowdloan_id)
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        crate::generated::passthrough::contribute_crowdloan(self, cx).await
    }

    async fn warnings<V: View + ?Sized>(
        &self,
        _cx: &PreflightCx<'_, V>,
    ) -> Result<Vec<String>, IntentError> {
        Ok(if matches!(self.amount_tao, Amount::All) {
            vec![
                "contributes the entire free balance (minus the existential deposit and fee headroom)"
                    .into(),
            ]
        } else {
            vec![]
        })
    }

    fn spend(&self) -> Spend {
        bounded_spend(&self.amount_tao)
    }

    fn validate(&self) -> Result<(), IntentError> {
        self.amount_tao.resolve(NetUid::ROOT, true)?;
        Ok(())
    }
}

impl Intent for FinalizeCrowdloan {
    fn summary(&self) -> String {
        format!("finalize crowdloan {}", self.crowdloan_id)
    }
    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        crate::generated::passthrough::finalize_crowdloan(self, cx).await
    }
}

impl Intent for RefundCrowdloan {
    fn summary(&self) -> String {
        format!("refund contributors of crowdloan {}", self.crowdloan_id)
    }
    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        crate::generated::passthrough::refund_crowdloan(self, cx).await
    }
}

impl Intent for DissolveCrowdloan {
    fn summary(&self) -> String {
        format!("dissolve crowdloan {}", self.crowdloan_id)
    }
    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        crate::generated::passthrough::dissolve_crowdloan(self, cx).await
    }
}

impl Intent for WithdrawCrowdloan {
    fn summary(&self) -> String {
        format!("withdraw from crowdloan {}", self.crowdloan_id)
    }
    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        crate::generated::passthrough::withdraw_crowdloan(self, cx).await
    }
}

impl Intent for UpdateCrowdloanCap {
    fn summary(&self) -> String {
        format!(
            "update crowdloan {} cap to {}",
            self.crowdloan_id,
            amount_summary(&self.new_cap_tao)
        )
    }
    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        crate::generated::passthrough::update_crowdloan_cap(self, cx).await
    }
    fn validate(&self) -> Result<(), IntentError> {
        crate::arguments::tao_amount(&self.new_cap_tao)?;
        Ok(())
    }
}

impl Intent for UpdateCrowdloanEnd {
    fn summary(&self) -> String {
        format!(
            "update crowdloan {} end to block {}",
            self.crowdloan_id, self.new_end
        )
    }
    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        crate::generated::passthrough::update_crowdloan_end(self, cx).await
    }
}

impl Intent for UpdateCrowdloanMinContribution {
    fn summary(&self) -> String {
        format!(
            "update crowdloan {} min contribution to {}",
            self.crowdloan_id,
            amount_summary(&self.new_min_contribution_tao)
        )
    }
    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        crate::generated::passthrough::update_crowdloan_min_contribution(self, cx).await
    }
    fn validate(&self) -> Result<(), IntentError> {
        crate::arguments::tao_amount(&self.new_min_contribution_tao)?;
        Ok(())
    }
}

impl Intent for SetCrowdloanMaxContribution {
    fn summary(&self) -> String {
        match &self.new_max_contribution_tao {
            Some(amount) => format!(
                "set crowdloan {} max contribution to {}",
                self.crowdloan_id,
                amount_summary(amount)
            ),
            None => format!("clear crowdloan {} max contribution", self.crowdloan_id),
        }
    }
    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        crate::generated::passthrough::set_crowdloan_max_contribution(self, cx).await
    }
    fn validate(&self) -> Result<(), IntentError> {
        if let Some(amount) = &self.new_max_contribution_tao {
            crate::arguments::tao_amount(amount)?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use rtao_client::{BlockRef, Client, ClientConfig, Network, Snapshot};
    use rtao_primitives::{AccountId, Alpha, Balance, Tao};
    use rtao_testkit::{FakeChain, FakeState, rpc::RpcClient};
    use rtao_tx::{IntentInterface, OriginView, SystemClock};
    use serde_json::json;

    const ORIGIN: AccountId = AccountId([7; 32]);
    const OTHER: AccountId = AccountId([8; 32]);

    async fn fixture(free: u64, deposit: u64) -> anyhow::Result<(FakeChain, Client, Snapshot)> {
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
            &[json!(OTHER.to_string())],
            json!({"data":{"free":u64::MAX}}),
        )?;
        state.seed_constant("Balances", "ExistentialDeposit", json!(deposit))?;
        let chain = FakeChain::from_state(state);
        let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
        config.track_heads = false;
        config.runtime_cache_dir = None;
        config.config_store = None;
        let client = Client::from_rpc(RpcClient::new(chain.clone()), config).await?;
        let snapshot = client.at(BlockRef::Finalized).await?;
        Ok((chain, client, snapshot))
    }

    #[test]
    fn summaries_spend_and_default_policy_match_all_ten_python_intents() {
        let create = CreateCrowdloan {
            deposit_tao: Balance::<Tao>::from_rao(1_000_000_000).into(),
            min_contribution_tao: Balance::<Tao>::ZERO.into(),
            cap_tao: Balance::<Tao>::from_rao(1_234_500_000_000).into(),
            end: 900,
            target_ss58: Some(OTHER.into()),
            call: None,
        };
        assert_eq!(
            create.summary(),
            "create crowdloan (cap τ1,234.500000000, deposit τ1.000000000, end 900)"
        );
        assert_eq!(
            create.spend(),
            Spend::Bounded(Balance::<Tao>::from_rao(1_000_000_000))
        );
        let contribution = ContributeCrowdloan {
            crowdloan_id: 42,
            amount_tao: Balance::<Tao>::from_rao(1).into(),
        };
        assert_eq!(
            contribution.summary(),
            "contribute τ0.000000001 to crowdloan 42"
        );
        assert_eq!(
            contribution.spend(),
            Spend::Bounded(Balance::<Tao>::from_rao(1))
        );
        let all = ContributeCrowdloan {
            amount_tao: Amount::All,
            ..contribution
        };
        assert_eq!(all.summary(), "contribute ALL free TAO to crowdloan 42");
        assert_eq!(all.spend(), Spend::Unbounded);
        assert_eq!(
            FinalizeCrowdloan { crowdloan_id: 42 }.summary(),
            "finalize crowdloan 42"
        );
        assert_eq!(
            RefundCrowdloan { crowdloan_id: 42 }.summary(),
            "refund contributors of crowdloan 42"
        );
        assert_eq!(
            DissolveCrowdloan { crowdloan_id: 42 }.summary(),
            "dissolve crowdloan 42"
        );
        assert_eq!(
            WithdrawCrowdloan { crowdloan_id: 42 }.summary(),
            "withdraw from crowdloan 42"
        );
        assert_eq!(
            UpdateCrowdloanCap {
                crowdloan_id: 42,
                new_cap_tao: Balance::<Tao>::from_rao(1).into()
            }
            .summary(),
            "update crowdloan 42 cap to τ0.000000001"
        );
        assert_eq!(
            UpdateCrowdloanEnd {
                crowdloan_id: 42,
                new_end: 12
            }
            .summary(),
            "update crowdloan 42 end to block 12"
        );
        assert_eq!(
            UpdateCrowdloanMinContribution {
                crowdloan_id: 42,
                new_min_contribution_tao: Balance::<Tao>::ZERO.into()
            }
            .summary(),
            "update crowdloan 42 min contribution to τ0.000000000"
        );
        let clear = SetCrowdloanMaxContribution {
            crowdloan_id: 42,
            new_max_contribution_tao: None,
        };
        assert_eq!(clear.summary(), "clear crowdloan 42 max contribution");
        assert_eq!(clear.spend(), Spend::None);
        assert_eq!(
            SetCrowdloanMaxContribution {
                new_max_contribution_tao: Some(Balance::<Tao>::ZERO.into()),
                ..clear
            }
            .summary(),
            "set crowdloan 42 max contribution to τ0.000000000"
        );
        assert!(create.touches_netuids().is_empty());
        assert!(!create.affects_all_subnets());
        assert_eq!(
            <ContributeCrowdloan as IntentInterface>::SPEC.all_amount_fields,
            &["amount_tao"]
        );
    }

    #[tokio::test]
    async fn all_ten_builds_encode_python_arguments_and_preserve_json() -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture(2_000_000, 100).await?;
        let runtime = &snapshot.block().runtime;
        let wire = TaoBalance(u64::MAX);
        for (op, args, expected) in [
            (
                "create_crowdloan",
                json!({"deposit_tao":"18446744073.709551615","min_contribution_tao":"0","cap_tao":"0.000000001","end":0,"target_ss58":OTHER.to_string()}),
                crowdloan::create(wire, TaoBalance(0), TaoBalance(1), 0, None, Some(OTHER)),
            ),
            (
                "contribute_crowdloan",
                json!({"crowdloan_id":u32::MAX,"amount_tao":"18446744073.709551615"}),
                crowdloan::contribute(u32::MAX, wire),
            ),
            (
                "finalize_crowdloan",
                json!({"crowdloan_id":42}),
                crowdloan::finalize(42),
            ),
            (
                "refund_crowdloan",
                json!({"crowdloan_id":42}),
                crowdloan::refund(42),
            ),
            (
                "dissolve_crowdloan",
                json!({"crowdloan_id":42}),
                crowdloan::dissolve(42),
            ),
            (
                "withdraw_crowdloan",
                json!({"crowdloan_id":42}),
                crowdloan::withdraw(42),
            ),
            (
                "update_crowdloan_cap",
                json!({"crowdloan_id":42,"new_cap_tao":"0"}),
                crowdloan::update_cap(42, TaoBalance(0)),
            ),
            (
                "update_crowdloan_end",
                json!({"crowdloan_id":42,"new_end":u32::MAX}),
                crowdloan::update_end(42, u32::MAX),
            ),
            (
                "update_crowdloan_min_contribution",
                json!({"crowdloan_id":42,"new_min_contribution_tao":"18446744073.709551615"}),
                crowdloan::update_min_contribution(42, wire),
            ),
            (
                "set_crowdloan_max_contribution",
                json!({"crowdloan_id":42}),
                crowdloan::set_max_contribution(42, None),
            ),
            (
                "set_crowdloan_max_contribution",
                json!({"crowdloan_id":42,"new_max_contribution_tao":"0"}),
                crowdloan::set_max_contribution(42, Some(TaoBalance(0))),
            ),
        ] {
            let intent = crate::AnyIntent::from_tool(op, args)?;
            let cx = BuildCx {
                view: &snapshot as &dyn rtao_client::DynView,
                origin: OriginView::Account(ORIGIN),
                runtime,
                clock: &SystemClock,
                spec: intent.spec(),
            };
            let built = intent.as_dyn().build_dyn(&cx).await?;
            assert_eq!(
                built.call.as_bytes(),
                expected.compose(runtime)?.as_bytes(),
                "{op}"
            );
            assert!(built.extras.is_empty(), "{op}");
            assert_eq!(
                crate::AnyIntent::from_tool(op, intent.to_json())?,
                intent,
                "{op}"
            );
            assert_eq!(intent.spec().signer, "coldkey", "{op}");
            assert!(intent.as_dyn().touches_netuids_dyn().is_empty(), "{op}");
            assert!(!intent.as_dyn().affects_all_subnets_dyn(), "{op}");
        }
        assert!(chain.state().lock().unwrap().submissions.is_empty());
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn all_uses_effective_origin_free_balance_and_warns_without_submission()
    -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture(2_000_000, 100).await?;
        let intent = ContributeCrowdloan {
            crowdloan_id: 42,
            amount_tao: Amount::All,
        };
        let cx = BuildCx {
            view: &snapshot,
            origin: OriginView::Account(ORIGIN),
            runtime: &snapshot.block().runtime,
            clock: &SystemClock,
            spec: ContributeCrowdloan::SPEC,
        };
        let built = intent.build(&cx).await?;
        assert_eq!(
            built.call.as_bytes(),
            crowdloan::contribute(42, TaoBalance(1_499_900))
                .compose(cx.runtime)?
                .as_bytes()
        );
        let preflight = PreflightCx {
            view: &snapshot,
            runtime: cx.runtime,
            clock: &SystemClock,
            spec: ContributeCrowdloan::SPEC,
            client: &client,
            dispatch_origin: ORIGIN,
            fee_payer: OTHER,
            call: &built.call,
        };
        let preview = intent.preflight(&preflight).await?;
        assert_eq!(preview.effects, vec![intent.summary()]);
        assert_eq!(
            preview.warnings,
            vec![
                "contributes the entire free balance (minus the existential deposit and fee headroom)"
            ]
        );
        assert!(preview.blocks.is_empty());
        let exact = ContributeCrowdloan {
            amount_tao: Balance::<Tao>::ZERO.into(),
            ..intent
        };
        assert!(exact.warnings(&preflight).await?.is_empty());
        assert!(chain.state().lock().unwrap().submissions.is_empty());
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn all_refuses_zero_shortfall_and_deposit_overflow_with_python_diagnostic()
    -> anyhow::Result<()> {
        for (free, deposit) in [(0, 0), (500_099, 100), (500_100, 100), (u64::MAX, u64::MAX)] {
            let (_, client, snapshot) = fixture(free, deposit).await?;
            let intent = ContributeCrowdloan {
                crowdloan_id: 42,
                amount_tao: Amount::All,
            };
            let cx = BuildCx {
                view: &snapshot,
                origin: OriginView::Account(ORIGIN),
                runtime: &snapshot.block().runtime,
                clock: &SystemClock,
                spec: ContributeCrowdloan::SPEC,
            };
            assert_eq!(
                intent.build(&cx).await.unwrap_err().to_string(),
                format!(
                    "nothing to stake: free balance {} does not cover the existential deposit plus the transaction fee",
                    DynBalance::from(Balance::<Tao>::from_rao(free))
                )
            );
            client.close().await;
        }
        let (_, client, snapshot) = fixture(500_101, 100).await?;
        let intent = ContributeCrowdloan {
            crowdloan_id: 42,
            amount_tao: Amount::All,
        };
        let cx = BuildCx {
            view: &snapshot,
            origin: OriginView::Account(ORIGIN),
            runtime: &snapshot.block().runtime,
            clock: &SystemClock,
            spec: ContributeCrowdloan::SPEC,
        };
        assert_eq!(stakeable_amount(&intent, &cx).await?, TaoBalance(1));
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn nested_build_preserves_origin_and_inserts_only_semantic_call() -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture(2_000_000, 100).await?;
        let create = CreateCrowdloan {
            deposit_tao: Balance::<Tao>::ZERO.into(),
            min_contribution_tao: Balance::<Tao>::ZERO.into(),
            cap_tao: Balance::<Tao>::ZERO.into(),
            end: 1,
            target_ss58: None,
            call: Some(json!({"op":"contribute_crowdloan","crowdloan_id":42,"amount_tao":"all"})),
        };
        let cx = BuildCx {
            view: &snapshot,
            origin: OriginView::Account(ORIGIN),
            runtime: &snapshot.block().runtime,
            clock: &SystemClock,
            spec: CreateCrowdloan::SPEC,
        };
        let built = create.build(&cx).await?;
        let inner = crowdloan::contribute(42, TaoBalance(1_499_900)).compose(cx.runtime)?;
        let expected = crowdloan::create(
            TaoBalance(0),
            TaoBalance(0),
            TaoBalance(0),
            1,
            Some(inner),
            None,
        )
        .compose(cx.runtime)?;
        assert_eq!(built.call.as_bytes(), expected.as_bytes());
        assert!(built.extras.is_empty());
        assert_eq!(create.spend(), Spend::Bounded(Balance::<Tao>::ZERO));
        let preflight = PreflightCx {
            view: &snapshot,
            runtime: cx.runtime,
            clock: &SystemClock,
            spec: CreateCrowdloan::SPEC,
            client: &client,
            dispatch_origin: ORIGIN,
            fee_payer: OTHER,
            call: &built.call,
        };
        assert!(create.warnings(&preflight).await?.is_empty());
        assert!(chain.state().lock().unwrap().submissions.is_empty());
        client.close().await;
        Ok(())
    }

    #[test]
    fn constructor_preserves_python_truthiness_and_rejects_invalid_money() {
        let create = CreateCrowdloan {
            deposit_tao: Balance::<Tao>::ZERO.into(),
            min_contribution_tao: Balance::<Tao>::ZERO.into(),
            cap_tao: Balance::<Tao>::ZERO.into(),
            end: 0,
            target_ss58: None,
            call: None,
        };
        assert_eq!(
            create.validate().unwrap_err().to_string(),
            "provide exactly one of target_ss58 or call"
        );
        let empty = CreateCrowdloan {
            call: Some(json!({})),
            ..create.clone()
        };
        assert!(empty.validate().is_err());
        let target = CreateCrowdloan {
            target_ss58: Some(OTHER.into()),
            ..empty
        };
        assert!(target.validate().is_ok());
        let both = CreateCrowdloan {
            call: Some(json!({"op":"finalize_crowdloan","crowdloan_id":1})),
            ..target.clone()
        };
        assert_eq!(
            both.validate().unwrap_err().to_string(),
            "provide exactly one of target_ss58 or call"
        );
        for invalid in [
            Amount::All,
            Balance::<Alpha>::new(NetUid(1), 1).unwrap().into(),
        ] {
            assert!(
                CreateCrowdloan {
                    deposit_tao: invalid.clone(),
                    ..target.clone()
                }
                .validate()
                .is_err()
            );
            assert!(
                CreateCrowdloan {
                    min_contribution_tao: invalid.clone(),
                    ..target.clone()
                }
                .validate()
                .is_err()
            );
            assert!(
                CreateCrowdloan {
                    cap_tao: invalid.clone(),
                    ..target.clone()
                }
                .validate()
                .is_err()
            );
            assert!(
                UpdateCrowdloanCap {
                    crowdloan_id: 0,
                    new_cap_tao: invalid.clone()
                }
                .validate()
                .is_err()
            );
            assert!(
                UpdateCrowdloanMinContribution {
                    crowdloan_id: 0,
                    new_min_contribution_tao: invalid.clone()
                }
                .validate()
                .is_err()
            );
            assert!(
                SetCrowdloanMaxContribution {
                    crowdloan_id: 0,
                    new_max_contribution_tao: Some(invalid)
                }
                .validate()
                .is_err()
            );
        }
        assert!(
            ContributeCrowdloan {
                crowdloan_id: 0,
                amount_tao: Balance::<Alpha>::new(NetUid(1), 1).unwrap().into()
            }
            .validate()
            .is_err()
        );
        assert!(
            crate::AnyIntent::from_tool(
                "contribute_crowdloan",
                json!({"crowdloan_id":0,"amount_tao":"18446744073.709551616"})
            )
            .is_err()
        );
    }

    #[tokio::test]
    async fn nested_missing_or_false_op_refuses_before_reads() -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture(2_000_000, 100).await?;
        for call in [
            json!({"crowdloan_id":42}),
            json!({"op":""}),
            json!({"op":false}),
            json!({"op":null}),
        ] {
            let create = CreateCrowdloan {
                deposit_tao: Balance::<Tao>::ZERO.into(),
                min_contribution_tao: Balance::<Tao>::ZERO.into(),
                cap_tao: Balance::<Tao>::ZERO.into(),
                end: 1,
                target_ss58: None,
                call: Some(call),
            };
            let cx = BuildCx {
                view: &snapshot,
                origin: OriginView::Account(ORIGIN),
                runtime: &snapshot.block().runtime,
                clock: &SystemClock,
                spec: CreateCrowdloan::SPEC,
            };
            let requests_before = chain.state().lock().unwrap().requests.len();
            assert_eq!(
                create.build(&cx).await.unwrap_err().to_string(),
                "crowdloan 'call' needs an 'op' key"
            );
            assert_eq!(
                chain.state().lock().unwrap().requests.len(),
                requests_before
            );
        }
        client.close().await;
        Ok(())
    }

    #[test]
    fn programmatic_nested_json_has_depth_node_and_encoded_byte_budgets() {
        let mut call = Value::Null;
        for _ in 1..MAX_CALL_DEPTH {
            call = Value::Array(vec![call]);
        }
        assert!(check_call_budget(&call).is_ok());
        call = Value::Array(vec![call]);
        assert!(
            check_call_budget(&call)
                .unwrap_err()
                .to_string()
                .contains("depth budget")
        );
        let wide = json!([null, null]);
        assert!(inspect_call_limits(&wide, 128, 3, 100).is_ok());
        assert!(
            inspect_call_limits(&wide, 128, 2, 100)
                .unwrap_err()
                .to_string()
                .contains("node budget")
        );
        let escaped = json!("\u{0000}");
        assert!(inspect_call_limits(&escaped, 128, 1, 8).is_ok());
        assert!(
            inspect_call_limits(&escaped, 128, 1, 7)
                .unwrap_err()
                .to_string()
                .contains("byte budget")
        );
        for key in [
            "$serde_json::private::RawValue",
            "$serde_json::private::Number",
        ] {
            let mut fields = serde_json::Map::new();
            fields.insert(key.into(), json!("{\"op\":\"finalize_crowdloan\"}"));
            assert!(
                check_call_budget(&Value::Object(fields))
                    .unwrap_err()
                    .to_string()
                    .contains("private JSON carrier")
            );
        }
    }

    #[test]
    fn recursive_work_budget_refuses_inputs_that_fit_literal_limits() {
        let wide = json!([null, null]);
        let cost = inspect_call_limits(&wide, 128, 3, 100).unwrap();
        assert_eq!((cost.nodes, cost.levels, cost.bytes), (3, 2, 11));
        assert!(
            check_call_limits(&wide, 128, 3, 100)
                .unwrap_err()
                .to_string()
                .contains("recursive JSON node-work budget")
        );
        let nested = json!([null]);
        let cost = inspect_call_limits(&nested, 128, 4, 6).unwrap();
        assert_eq!((cost.nodes, cost.levels, cost.bytes), (2, 2, 6));
        assert!(
            check_call_limits(&nested, 128, 4, 6)
                .unwrap_err()
                .to_string()
                .contains("recursive JSON byte-work budget")
        );
        assert!(check_call_limits(&nested, 128, 4, 12).is_ok());
    }

    #[tokio::test]
    async fn ordinary_nested_create_fits_aggregate_budget_and_preserves_call() -> anyhow::Result<()>
    {
        let (_, client, snapshot) = fixture(2_000_000, 100).await?;
        let create = CreateCrowdloan {
            deposit_tao: Balance::<Tao>::ZERO.into(),
            min_contribution_tao: Balance::<Tao>::ZERO.into(),
            cap_tao: Balance::<Tao>::ZERO.into(),
            end: 1,
            target_ss58: None,
            call: Some(json!({
                "op":"create_crowdloan", "deposit_tao":"0", "min_contribution_tao":"0",
                "cap_tao":"0", "end":2, "target_ss58":OTHER.to_string()
            })),
        };
        let cx = BuildCx {
            view: &snapshot,
            origin: OriginView::Account(ORIGIN),
            runtime: &snapshot.block().runtime,
            clock: &SystemClock,
            spec: CreateCrowdloan::SPEC,
        };
        assert!(check_call_budget(create.call.as_ref().unwrap()).is_ok());
        let inner = crowdloan::create(
            TaoBalance(0),
            TaoBalance(0),
            TaoBalance(0),
            2,
            None,
            Some(OTHER),
        )
        .compose(cx.runtime)?;
        let expected = crowdloan::create(
            TaoBalance(0),
            TaoBalance(0),
            TaoBalance(0),
            1,
            Some(inner),
            None,
        )
        .compose(cx.runtime)?;
        let built = create.build(&cx).await?;
        assert_eq!(built.call.as_bytes(), expected.as_bytes());
        assert!(built.extras.is_empty());
        client.close().await;
        Ok(())
    }

    #[test]
    fn tool_constructor_bounds_nested_call_before_serde_conversion() {
        let mut call = Value::Null;
        for _ in 0..MAX_CALL_DEPTH {
            call = Value::Array(vec![call]);
        }
        let mut args = json!({
            "deposit_tao":"0", "min_contribution_tao":"0", "cap_tao":"0", "end":0
        });
        args["call"] = call;
        assert!(
            crate::AnyIntent::from_tool("create_crowdloan", args)
                .unwrap_err()
                .to_string()
                .contains("depth budget")
        );
        for key in [
            "$serde_json::private::RawValue",
            "$serde_json::private::Number",
        ] {
            let mut fields = serde_json::Map::new();
            fields.insert(key.into(), json!("{\"op\":\"finalize_crowdloan\"}"));
            let mut args = json!({
                "deposit_tao":"0", "min_contribution_tao":"0", "cap_tao":"0", "end":0
            });
            args["call"] = Value::Object(fields);
            assert!(
                crate::AnyIntent::from_tool("create_crowdloan", args)
                    .unwrap_err()
                    .to_string()
                    .contains("private JSON carrier")
            );
        }
    }
}
