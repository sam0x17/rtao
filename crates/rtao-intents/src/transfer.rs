// Derived from RaoFoundation/subtensor sdk/python/bittensor/intents/transfer.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Coldkey TAO transfers with checked native balances.

pub use crate::generated::{Transfer, TransferAll};
use rtao_client::View;
use rtao_primitives::{Amount, IntoCoreValue, NetUid, ResolvedAmount, Spend};
use rtao_runtime::{Call, calls::balances};
use rtao_tx::{BuildCx, Built, Intent, IntentError, PreflightCx};

impl Transfer {
    fn chain_call(&self) -> Result<Call, IntentError> {
        let destination = rtao_core::DynValue::new((&self.dest_ss58).into_core_value());
        match self.amount_tao.resolve(NetUid::ROOT, true)? {
            ResolvedAmount::All => Ok(balances::transfer_all(destination, self.keep_alive)),
            ResolvedAmount::Exact(amount) => {
                let amount = amount.tao()?.to_wire();
                Ok(if self.keep_alive {
                    balances::transfer_keep_alive(destination, amount)
                } else {
                    balances::transfer_allow_death(destination, amount)
                })
            }
        }
    }
}

impl Intent for Transfer {
    fn summary(&self) -> String {
        let amount = match self.amount_tao.resolve(NetUid::ROOT, true) {
            Ok(ResolvedAmount::All) => "ALL TAO".into(),
            Ok(ResolvedAmount::Exact(amount)) => amount.to_string(),
            // Typed callers can construct an invalid unit; validate() rejects it before execution.
            Err(_) => match &self.amount_tao {
                Amount::All => "ALL TAO".into(),
                Amount::Exact(amount) => amount.to_decimal_string(),
            },
        };
        format!("transfer {amount} to {}", self.dest_ss58)
    }
    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        Ok(Built::new(self.chain_call()?.compose(cx.runtime)?))
    }
    async fn warnings<V: View + ?Sized>(
        &self,
        _cx: &PreflightCx<'_, V>,
    ) -> Result<Vec<String>, IntentError> {
        Ok(if matches!(self.amount_tao, Amount::All) {
            vec!["transfers the entire transferable balance".into()]
        } else {
            vec![]
        })
    }
    fn spend(&self) -> Spend {
        match self.amount_tao.resolve(NetUid::ROOT, true) {
            Ok(ResolvedAmount::All) => Spend::Unbounded,
            Ok(ResolvedAmount::Exact(amount)) => match amount.tao() {
                Ok(value) => Spend::Bounded(value),
                Err(_) => Spend::Unbounded,
            },
            Err(_) => Spend::Unbounded,
        }
    }
    fn validate(&self) -> Result<(), IntentError> {
        self.amount_tao.resolve(NetUid::ROOT, true)?;
        Ok(())
    }
    fn semantic_intent(&self) -> &dyn rtao_tx::DynIntent {
        self
    }
}

impl Intent for TransferAll {
    fn summary(&self) -> String {
        format!("transfer ALL TAO to {}", self.dest_ss58)
    }
    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        crate::generated::passthrough::transfer_all(self, cx).await
    }
    async fn warnings<V: View + ?Sized>(
        &self,
        _cx: &PreflightCx<'_, V>,
    ) -> Result<Vec<String>, IntentError> {
        Ok(vec!["transfers the entire transferable balance".into()])
    }
    fn spend(&self) -> Spend {
        Spend::Unbounded
    }
    fn semantic_intent(&self) -> &dyn rtao_tx::DynIntent {
        self
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use rtao_primitives::{AccountId, Alpha, Balance, Tao};
    use rtao_tx::IntentInterface;

    #[test]
    fn fixed_all_and_keep_alive_choose_python_calls() {
        let mut intent = Transfer {
            dest_ss58: AccountId([1; 32]).into(),
            amount_tao: Balance::<Tao>::from_rao(123).into(),
            keep_alive: true,
        };
        assert_eq!(
            intent.chain_call().unwrap().descriptor.name,
            "transfer_keep_alive"
        );
        assert_eq!(
            intent.spend(),
            Spend::Bounded(Balance::<Tao>::from_rao(123))
        );
        intent.keep_alive = false;
        assert_eq!(
            intent.chain_call().unwrap().descriptor.name,
            "transfer_allow_death"
        );
        intent.amount_tao = Amount::All;
        assert_eq!(intent.chain_call().unwrap().descriptor.name, "transfer_all");
        assert_eq!(intent.spend(), Spend::Unbounded);
    }

    #[test]
    fn exact_amounts_validate_units_and_roundtrip_without_float() {
        let destination = AccountId([1; 32]);
        let args = serde_json::json!({"dest_ss58":destination.to_string(),"amount_tao":"18446744073.709551615"});
        let intent = crate::AnyIntent::from_tool("transfer", args).unwrap();
        let json = intent.to_json();
        assert_eq!(json["amount_tao"], "18446744073.709551615");
        assert_eq!(json["keep_alive"], true);
        assert_eq!(
            crate::AnyIntent::from_tool("transfer", json).unwrap(),
            intent
        );
        let invalid = Transfer {
            dest_ss58: destination.into(),
            amount_tao: Balance::<Alpha>::new(NetUid(1), 1).unwrap().into(),
            keep_alive: true,
        };
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn summaries_and_spend_match_python_transfer_semantics() {
        let destination = AccountId([1; 32]);
        let intent = Transfer {
            dest_ss58: destination.into(),
            amount_tao: Balance::<Tao>::from_rao(1_234_500_000_000).into(),
            keep_alive: true,
        };
        assert_eq!(
            intent.summary(),
            format!("transfer τ1,234.500000000 to {destination}")
        );
        assert!(
            <Transfer as IntentInterface>::SPEC
                .all_amount_fields
                .contains(&"amount_tao")
        );
        assert_eq!(
            TransferAll {
                dest_ss58: destination.into(),
                keep_alive: false
            }
            .summary(),
            format!("transfer ALL TAO to {destination}")
        );
    }

    #[tokio::test]
    async fn ss58_input_prefixes_preserve_json_and_summary_but_encode_the_same_account() {
        use rtao_client::{BlockRef, Client, ClientConfig, Network};
        use rtao_testkit::{FakeChain, FakeState};
        use rtao_tx::{OriginView, SystemClock};
        use serde_json::json;

        let fake = FakeChain::from_state(FakeState::default());
        let mut config =
            ClientConfig::new(Network::resolve_with("local", |_| None).unwrap()).unwrap();
        config.track_heads = false;
        config.runtime_cache_dir = None;
        config.config_store = None;
        let client = Client::from_rpc(rtao_testkit::rpc::RpcClient::new(fake.clone()), config)
            .await
            .unwrap();
        let snapshot = client.at(BlockRef::Finalized).await.unwrap();
        let runtime = &snapshot.block().runtime;
        let destination = AccountId([1; 32]);
        for prefix in [0, 2, 110] {
            let spelling = destination.to_ss58(prefix);
            for keep_alive in [false, true] {
                for (op, amount, summary) in [
                    ("transfer", Some(json!("0.000000123")), "τ0.000000123"),
                    ("transfer", Some(json!("all")), "ALL TAO"),
                    ("transfer_all", None, "ALL TAO"),
                ] {
                    let mut args = json!({"dest_ss58":spelling,"keep_alive":keep_alive});
                    if let Some(amount) = amount {
                        args["amount_tao"] = amount;
                    }
                    let intent = crate::AnyIntent::from_tool(op, args).unwrap();
                    assert_eq!(intent.to_json()["dest_ss58"], spelling);
                    assert_eq!(
                        intent.as_dyn().summary_dyn(),
                        format!("transfer {summary} to {spelling}")
                    );
                    let context = BuildCx {
                        view: &snapshot as &dyn rtao_client::DynView,
                        origin: OriginView::Account(AccountId([43; 32])),
                        runtime,
                        clock: &SystemClock,
                        spec: intent.spec(),
                    };
                    let built = intent.as_dyn().build_dyn(&context).await.unwrap();
                    let expected = if summary == "ALL TAO" {
                        balances::transfer_all(crate::account_argument(destination), keep_alive)
                    } else if keep_alive {
                        balances::transfer_keep_alive(
                            crate::account_argument(destination),
                            Balance::<Tao>::from_rao(123).to_wire(),
                        )
                    } else {
                        balances::transfer_allow_death(
                            crate::account_argument(destination),
                            Balance::<Tao>::from_rao(123).to_wire(),
                        )
                    }
                    .compose(runtime)
                    .unwrap();
                    assert_eq!(built.call.as_bytes(), expected.as_bytes());
                    assert_eq!(
                        crate::AnyIntent::from_tool(op, intent.to_json()).unwrap(),
                        intent
                    );
                }
            }
        }
        assert!(fake.state().lock().unwrap().submissions.is_empty());
        client.close().await;
    }
}
