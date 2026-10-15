// Derived from RaoFoundation/subtensor sdk/python/bittensor/intents/root.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Root administration calls; the executor applies the catalog's sudo wrapper.

use rtao_client::View;
use rtao_primitives::{Amount, NetUid};
use rtao_runtime::calls::{admin_utils, subtensor_module, utility};
use rtao_tx::{BuildCx, Built, Intent, IntentError};
use smallvec::SmallVec;

pub use crate::{SetRootClaimThreshold, SetSubnetEmissionEnabled};

impl Intent for SetRootClaimThreshold {
    fn summary(&self) -> String {
        let threshold = match &self.threshold {
            Amount::Exact(money) => money
                .resolve(NetUid::ROOT)
                .map(|balance| balance.to_string())
                .unwrap_or_else(|_| money.to_decimal_string()),
            // Typed construction can bypass validation; summaries must remain total.
            Amount::All => "all".into(),
        };
        format!("set the root claim threshold to {threshold}")
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        let threshold = crate::arguments::tao_amount(&self.threshold)?;
        Ok(Built::new(
            subtensor_module::sudo_set_root_claim_threshold(NetUid::ROOT, threshold.rao())
                .compose(cx.runtime)?,
        ))
    }

    fn validate(&self) -> Result<(), IntentError> {
        // Python delegates the chain's 0.01 TAO policy cap to dispatch.
        crate::arguments::tao_amount(&self.threshold)?;
        Ok(())
    }
}

impl Intent for SetSubnetEmissionEnabled {
    fn summary(&self) -> String {
        let action = if self.enabled { "enable" } else { "disable" };
        let netuids: Vec<_> = self.netuids.iter().map(|netuid| netuid.get()).collect();
        format!("{action} subnet emission on netuids {netuids:?}")
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        self.validate()?;
        let call = if let [netuid] = self.netuids.as_slice() {
            admin_utils::sudo_set_subnet_emission_enabled(*netuid, self.enabled)
                .compose(cx.runtime)?
        } else {
            let calls = self
                .netuids
                .iter()
                .map(|netuid| {
                    admin_utils::sudo_set_subnet_emission_enabled(*netuid, self.enabled)
                        .compose(cx.runtime)
                })
                .collect::<Result<Vec<_>, _>>()?;
            utility::batch_all(calls).compose(cx.runtime)?
        };
        Ok(Built::new(call))
    }

    fn validate(&self) -> Result<(), IntentError> {
        if self.netuids.is_empty() {
            return Err(IntentError::Validation(
                "set_subnet_emission_enabled requires at least one netuid".into(),
            ));
        }
        Ok(())
    }

    fn touches_netuids(&self) -> SmallVec<[NetUid; 2]> {
        self.netuids.iter().copied().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rtao_client::{BlockRef, Client, ClientConfig, Network, Snapshot};
    use rtao_primitives::{AccountId, Alpha, Balance, Spend, Tao, UnitError};
    use rtao_runtime::{CallBytes, RuntimeHandle};
    use rtao_testkit::{FakeChain, FakeState, rpc::RpcClient};
    use rtao_tx::{IntentInterface, OriginView, PreflightCx, SystemClock};
    use serde_json::{Value, json};

    const ORIGIN: AccountId = AccountId([7; 32]);

    async fn fixture() -> anyhow::Result<(FakeChain, Client, Snapshot)> {
        let chain = FakeChain::from_state(FakeState::try_default()?);
        let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
        config.track_heads = false;
        config.runtime_cache_dir = None;
        config.config_store = None;
        let client = Client::from_rpc(RpcClient::new(chain.clone()), config).await?;
        let snapshot = client.at(BlockRef::Finalized).await?;
        Ok((chain, client, snapshot))
    }

    fn decoded(call: &CallBytes, runtime: &RuntimeHandle) -> anyhow::Result<Value> {
        let decoded = runtime
            .core()
            .decode_spec(&rtao_core::runtime::TypeSpec::Call, call.as_bytes(), true)
            .map_err(rtao_core::CoreError::from)?;
        Ok(rtao_core::codec::value::to_corpus_json(&decoded))
    }

    async fn assert_base_preflight<I: Intent>(
        intent: &I,
        call: &CallBytes,
        snapshot: &Snapshot,
        client: &Client,
    ) -> anyhow::Result<()> {
        let preview = intent
            .preflight(&PreflightCx {
                view: snapshot,
                runtime: &snapshot.block().runtime,
                clock: &SystemClock,
                spec: I::SPEC,
                client,
                dispatch_origin: ORIGIN,
                fee_payer: ORIGIN,
                call,
            })
            .await?;
        assert_eq!(preview.effects, [intent.summary()]);
        assert!(preview.warnings.is_empty());
        assert!(preview.blocks.is_empty());
        assert!(preview.required_free.is_none());
        assert!(preview.available_free.is_none());
        assert!(preview.estimated_fee.is_none());
        assert!(preview.facts.is_empty());
        assert_eq!(intent.spend(), Spend::None);
        assert!(!intent.affects_all_subnets());
        Ok(())
    }

    #[test]
    fn root_threshold_preserves_native_amounts_and_refuses_all_or_alpha() -> anyhow::Result<()> {
        for rao in [0, 1, 500_000, 10_000_000, 10_000_001, u64::MAX] {
            let intent = SetRootClaimThreshold {
                threshold: Balance::<Tao>::from_rao(rao).into(),
            };
            intent.validate()?;
            assert_eq!(intent.spend(), Spend::None);
            assert!(intent.touches_netuids().is_empty());
            assert_eq!(
                crate::AnyIntent::from_tool("set_root_claim_threshold", intent.to_json())?
                    .to_json(),
                intent.to_json()
            );
        }
        assert_eq!(
            SetRootClaimThreshold {
                threshold: Balance::<Tao>::from_rao(500_000).into(),
            }
            .summary(),
            "set the root claim threshold to τ0.000500000"
        );
        assert!(matches!(
            SetRootClaimThreshold {
                threshold: Amount::All,
            }
            .validate(),
            Err(IntentError::UnitMismatch(UnitError::AllNotAllowed))
        ));
        assert!(matches!(
            SetRootClaimThreshold {
                threshold: Balance::<Alpha>::new(NetUid(1), 500_000)?.into(),
            }
            .validate(),
            Err(IntentError::UnitMismatch(UnitError::UnitMismatch {
                expected: NetUid::ROOT,
                actual: NetUid(1),
            }))
        ));
        for threshold in [json!("all"), json!("18446744073.709551616"), json!(-1)] {
            assert!(
                crate::AnyIntent::from_tool(
                    "set_root_claim_threshold",
                    json!({"threshold":threshold})
                )
                .is_err()
            );
        }
        assert_eq!(SetRootClaimThreshold::SPEC.signer, "coldkey");
        assert_eq!(SetRootClaimThreshold::SPEC.origin, "root");
        assert_eq!(
            SetRootClaimThreshold::SPEC.verify,
            Some("root_claim_threshold")
        );
        Ok(())
    }

    #[tokio::test]
    async fn root_threshold_fake_chain_encodes_root_entry_without_sudo() -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture().await?;
        let runtime = &snapshot.block().runtime;
        let requests = chain.requests().len();
        for rao in [0u64, 1, 500_000, 10_000_000, 10_000_001, u64::MAX] {
            let intent = SetRootClaimThreshold {
                threshold: Balance::<Tao>::from_rao(rao).into(),
            };
            let built = intent
                .build(&BuildCx {
                    view: &snapshot,
                    origin: OriginView::Account(ORIGIN),
                    runtime,
                    clock: &SystemClock,
                    spec: SetRootClaimThreshold::SPEC,
                })
                .await?;
            // Independent pinned SCALE layout: pallet/call, plain root u16, plain u64.
            let mut expected = vec![7, 124, 0, 0];
            expected.extend_from_slice(&rao.to_le_bytes());
            assert_eq!(built.call.as_bytes(), expected);
            assert!(built.extras.is_empty());
            assert_eq!(CallBytes::decode(&expected, runtime)?, built.call);
            let decoded = decoded(&built.call, runtime)?;
            assert_eq!(decoded["call_module"], "SubtensorModule");
            assert_eq!(decoded["call_function"], "sudo_set_root_claim_threshold");
            assert_eq!(decoded["call_args"][0]["name"], "netuid");
            assert_eq!(decoded["call_args"][0]["value"], 0);
            assert_eq!(decoded["call_args"][1]["name"], "new_value");
            assert_eq!(decoded["call_args"][1]["value"], json!(rao));
            assert_base_preflight(&intent, &built.call, &snapshot, &client).await?;
        }
        assert_eq!(chain.requests().len(), requests);
        client.close().await;
        Ok(())
    }

    #[test]
    fn root_emission_keeps_order_duplicates_and_python_summaries() -> anyhow::Result<()> {
        let intent = SetSubnetEmissionEnabled {
            netuids: vec![NetUid(9), NetUid(1), NetUid(9)],
            enabled: false,
        };
        intent.validate()?;
        assert_eq!(
            intent.summary(),
            "disable subnet emission on netuids [9, 1, 9]"
        );
        assert_eq!(intent.touches_netuids().as_slice(), intent.netuids);
        assert_eq!(intent.spend(), Spend::None);
        assert_eq!(
            crate::AnyIntent::from_tool("set_subnet_emission_enabled", intent.to_json())?.to_json(),
            intent.to_json()
        );
        let empty = SetSubnetEmissionEnabled {
            netuids: vec![],
            enabled: true,
        };
        assert_eq!(
            empty.validate().err().map(|error| error.to_string()),
            Some("set_subnet_emission_enabled requires at least one netuid".into())
        );
        assert!(
            crate::AnyIntent::from_tool(
                "set_subnet_emission_enabled",
                json!({"netuids":[],"enabled":true})
            )
            .is_err()
        );
        assert_eq!(SetSubnetEmissionEnabled::SPEC.signer, "coldkey");
        assert_eq!(SetSubnetEmissionEnabled::SPEC.origin, "root");
        assert_eq!(
            SetSubnetEmissionEnabled::SPEC.verify,
            Some("subnet_emission_enabled")
        );
        assert_eq!(
            SetSubnetEmissionEnabled::SPEC.wraps,
            &[
                ("AdminUtils", "sudo_set_subnet_emission_enabled"),
                ("Sudo", "sudo")
            ]
        );
        Ok(())
    }

    #[tokio::test]
    async fn root_emission_fake_chain_uses_single_call_or_atomic_batch() -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture().await?;
        let runtime = &snapshot.block().runtime;
        let requests = chain.requests().len();
        for (netuids, enabled, summary) in [
            (
                vec![NetUid(7)],
                true,
                "enable subnet emission on netuids [7]",
            ),
            (
                vec![NetUid(9), NetUid(1), NetUid(9)],
                false,
                "disable subnet emission on netuids [9, 1, 9]",
            ),
            (
                vec![NetUid::ROOT, NetUid(u16::MAX)],
                true,
                "enable subnet emission on netuids [0, 65535]",
            ),
        ] {
            let intent = SetSubnetEmissionEnabled { netuids, enabled };
            assert_eq!(intent.summary(), summary);
            let built = intent
                .build(&BuildCx {
                    view: &snapshot,
                    origin: OriginView::Account(ORIGIN),
                    runtime,
                    clock: &SystemClock,
                    spec: SetSubnetEmissionEnabled::SPEC,
                })
                .await?;
            let mut expected = if intent.netuids.len() == 1 {
                vec![]
            } else {
                // All fixture lists fit SCALE compact mode 0.
                vec![11, 2, u8::try_from(intent.netuids.len())? << 2]
            };
            for netuid in &intent.netuids {
                expected.extend_from_slice(&[19, 94]);
                expected.extend_from_slice(&netuid.get().to_le_bytes());
                expected.push(u8::from(enabled));
            }
            assert_eq!(built.call.as_bytes(), expected);
            assert!(built.extras.is_empty());
            assert_eq!(CallBytes::decode(&expected, runtime)?, built.call);
            let decoded = decoded(&built.call, runtime)?;
            let children = if intent.netuids.len() == 1 {
                assert_eq!(decoded["call_module"], "AdminUtils");
                vec![decoded]
            } else {
                assert_eq!(decoded["call_module"], "Utility");
                assert_eq!(decoded["call_function"], "batch_all");
                serde_json::from_value::<Vec<Value>>(decoded["call_args"][0]["value"].clone())?
            };
            assert_eq!(children.len(), intent.netuids.len());
            for (child, netuid) in children.iter().zip(&intent.netuids) {
                assert_eq!(child["call_module"], "AdminUtils");
                assert_eq!(child["call_function"], "sudo_set_subnet_emission_enabled");
                assert_eq!(child["call_args"][0]["name"], "netuid");
                assert_eq!(child["call_args"][0]["value"], json!(netuid.get()));
                assert_eq!(child["call_args"][1]["name"], "enabled");
                assert_eq!(child["call_args"][1]["value"], enabled);
            }
            assert_base_preflight(&intent, &built.call, &snapshot, &client).await?;
        }
        let empty = SetSubnetEmissionEnabled {
            netuids: vec![],
            enabled: true,
        };
        assert!(
            empty
                .build(&BuildCx {
                    view: &snapshot,
                    origin: OriginView::Account(ORIGIN),
                    runtime,
                    clock: &SystemClock,
                    spec: SetSubnetEmissionEnabled::SPEC,
                })
                .await
                .is_err()
        );
        assert_eq!(chain.requests().len(), requests);
        client.close().await;
        Ok(())
    }
}
