// Derived from RaoFoundation/subtensor sdk/python/bittensor/intents/coldkey.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Coldkey migration announcements, execution, cancellation and disputes.

use rtao_client::View;
use rtao_primitives::{H256, Ss58Address};
use rtao_runtime::calls::subtensor_module;
use rtao_tx::{BuildCx, Built, Intent, IntentError, PreflightCx};

pub use crate::{
    AnnounceColdkeySwap, ClearColdkeySwapAnnouncement, DisputeColdkeySwap, SwapColdkeyAnnounced,
};

/// Hash the decoded public key with BLAKE2b-256, independently of its SS58 prefix.
#[doc(alias = "bittensor.intents.coldkey.coldkey_hash")]
pub fn coldkey_hash(address: &Ss58Address) -> Result<H256, IntentError> {
    let hash = rtao_core::codec::storage::hash_param("Blake2_256", address.as_bytes())?;
    let hash = hash.try_into().map_err(|_| {
        IntentError::validation("Blake2_256 returned a coldkey hash with an invalid length")
    })?;
    Ok(H256(hash))
}

impl Intent for AnnounceColdkeySwap {
    fn summary(&self) -> String {
        format!("announce coldkey swap to {}", self.new_coldkey_ss58)
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        Ok(Built::new(
            subtensor_module::announce_coldkey_swap(coldkey_hash(&self.new_coldkey_ss58)?)
                .compose(cx.runtime)?,
        ))
    }

    async fn warnings<V: View + ?Sized>(
        &self,
        _cx: &PreflightCx<'_, V>,
    ) -> Result<Vec<String>, IntentError> {
        Ok(vec![
            "after the announcement delay, swap_coldkey_announced will move EVERYTHING this coldkey owns (balance, stake, subnets) to the new coldkey".into(),
            "make sure you control the new coldkey and have its mnemonic backed up".into(),
        ])
    }
}

impl Intent for SwapColdkeyAnnounced {
    fn summary(&self) -> String {
        format!(
            "SWAP coldkey to {} (moves all ownership)",
            self.new_coldkey_ss58
        )
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        crate::generated::passthrough::swap_coldkey_announced(self, cx).await
    }

    async fn warnings<V: View + ?Sized>(
        &self,
        _cx: &PreflightCx<'_, V>,
    ) -> Result<Vec<String>, IntentError> {
        Ok(vec![
            "irreversible: balance, stake, and subnet ownership move to the new coldkey".into(),
            "the new coldkey must match the announced hash exactly".into(),
        ])
    }

    fn affects_all_subnets(&self) -> bool {
        true
    }
}

impl Intent for ClearColdkeySwapAnnouncement {
    fn summary(&self) -> String {
        "clear the pending coldkey swap announcement".into()
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        crate::generated::passthrough::clear_coldkey_swap_announcement(self, cx).await
    }
}

impl Intent for DisputeColdkeySwap {
    fn summary(&self) -> String {
        "dispute the pending coldkey swap (freezes it for governance)".into()
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        crate::generated::passthrough::dispute_coldkey_swap(self, cx).await
    }

    async fn warnings<V: View + ?Sized>(
        &self,
        _cx: &PreflightCx<'_, V>,
    ) -> Result<Vec<String>, IntentError> {
        Ok(vec![
            "blocks the swap until the triumvirate resolves the dispute".into(),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rtao_client::{BlockRef, Client, ClientConfig, DynView, Network, Snapshot};
    use rtao_primitives::{AccountId, Spend};
    use rtao_runtime::{CallBytes, RuntimeHandle};
    use rtao_testkit::{FakeChain, FakeState, rpc::RpcClient};
    use rtao_tx::{DynIntent, OriginView, SystemClock};
    use serde_json::{Value, json};

    const ORIGIN: AccountId = AccountId([7; 32]);
    // Independent Bouncy Castle BLAKE2b-256 vector for 32 zero bytes:
    // attocash/commons @ 5ae16299214ed7fd44bc33c3ee6ee437cde393d9,
    // commons-core/src/commonTest/kotlin/cash/atto/commons/AttoHashTest.kt:9-18.
    const ZERO_KEY_HASH: &str = "89eb0d6a8a691dae2cd15ed0369931ce0a949ecafa5c3f93f8121833646e15c3";

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

    fn decode(call: &CallBytes, runtime: &RuntimeHandle) -> anyhow::Result<Value> {
        let decoded = runtime
            .core()
            .decode_spec(&rtao_core::runtime::TypeSpec::Call, call.as_bytes(), true)
            .map_err(rtao_core::CoreError::from)?;
        Ok(rtao_core::codec::value::to_corpus_json(&decoded))
    }

    #[test]
    fn coldkey_hash_matches_independent_blake2b256_vector() -> anyhow::Result<()> {
        let address: Ss58Address = AccountId::ZERO.into();
        assert_eq!(hex::encode(coldkey_hash(&address)?.0), ZERO_KEY_HASH);
        Ok(())
    }

    #[tokio::test]
    async fn coldkey_swaps_keep_ss58_spelling_but_encode_one_account_across_prefixes()
    -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture().await?;
        let runtime = &snapshot.block().runtime;
        let requests = chain.requests().len();
        let account = AccountId::ZERO;
        let mut announced = vec![7, 125];
        announced.extend_from_slice(&hex::decode(ZERO_KEY_HASH)?);
        let mut swapped = vec![7, 126];
        swapped.extend_from_slice(account.as_bytes());
        for prefix in [0, 2, 42, 110, 1000] {
            let spelling = account.to_ss58(prefix);
            let announce = AnnounceColdkeySwap {
                new_coldkey_ss58: spelling.parse()?,
            };
            let swap = SwapColdkeyAnnounced {
                new_coldkey_ss58: spelling.parse()?,
            };
            for (intent, summary, bytes, argument, value) in [
                (
                    &announce as &dyn DynIntent,
                    format!("announce coldkey swap to {spelling}"),
                    announced.as_slice(),
                    "new_coldkey_hash",
                    json!(format!("0x{ZERO_KEY_HASH}")),
                ),
                (
                    &swap as &dyn DynIntent,
                    format!("SWAP coldkey to {spelling} (moves all ownership)"),
                    swapped.as_slice(),
                    "new_coldkey",
                    json!(account.to_string()),
                ),
            ] {
                assert_eq!(intent.summary_dyn(), summary);
                assert_eq!(intent.to_json_dyn()["new_coldkey_ss58"], spelling);
                assert_eq!(
                    crate::AnyIntent::from_tool(intent.spec().name, intent.to_json_dyn())?
                        .to_json(),
                    intent.to_json_dyn()
                );
                let built = intent
                    .build_dyn(&BuildCx {
                        view: &snapshot as &dyn DynView,
                        origin: OriginView::Account(ORIGIN),
                        runtime,
                        clock: &SystemClock,
                        spec: intent.spec(),
                    })
                    .await?;
                assert_eq!(built.call.as_bytes(), bytes);
                assert_eq!(CallBytes::decode(bytes, runtime)?, built.call);
                assert!(built.extras.is_empty());
                let decoded = decode(&built.call, runtime)?;
                assert_eq!(decoded["call_module"], "SubtensorModule");
                assert_eq!(decoded["call_function"], intent.spec().name);
                assert_eq!(decoded["call_args"][0]["name"], argument);
                assert_eq!(decoded["call_args"][0]["value"], value);
            }
        }
        assert_eq!(chain.requests().len(), requests);
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn coldkey_clear_and_dispute_encode_exact_zero_argument_calls() -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture().await?;
        let runtime = &snapshot.block().runtime;
        let requests = chain.requests().len();
        let clear = ClearColdkeySwapAnnouncement {};
        let dispute = DisputeColdkeySwap {};
        for (intent, bytes) in [
            (&clear as &dyn DynIntent, [7u8, 133]),
            (&dispute as &dyn DynIntent, [7u8, 127]),
        ] {
            let built = intent
                .build_dyn(&BuildCx {
                    view: &snapshot as &dyn DynView,
                    origin: OriginView::Account(ORIGIN),
                    runtime,
                    clock: &SystemClock,
                    spec: intent.spec(),
                })
                .await?;
            assert_eq!(built.call.as_bytes(), bytes.as_slice());
            assert_eq!(CallBytes::decode(&bytes, runtime)?, built.call);
            assert!(built.extras.is_empty());
            let decoded = decode(&built.call, runtime)?;
            assert_eq!(decoded["call_module"], "SubtensorModule");
            assert_eq!(decoded["call_function"], intent.spec().name);
            assert_eq!(decoded["call_args"], json!([]));
        }
        assert_eq!(chain.requests().len(), requests);
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn coldkey_preflights_keep_exact_warnings_and_inherited_policy_facts()
    -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture().await?;
        let runtime = &snapshot.block().runtime;
        let requests = chain.requests().len();
        let address: Ss58Address = AccountId::ZERO.into();
        let announce = AnnounceColdkeySwap {
            new_coldkey_ss58: address.clone(),
        };
        let swap = SwapColdkeyAnnounced {
            new_coldkey_ss58: address.clone(),
        };
        let clear = ClearColdkeySwapAnnouncement {};
        let dispute = DisputeColdkeySwap {};
        for (intent, summary, warnings, all_subnets) in [
            (
                &announce as &dyn DynIntent,
                format!("announce coldkey swap to {address}"),
                vec![
                    "after the announcement delay, swap_coldkey_announced will move EVERYTHING this coldkey owns (balance, stake, subnets) to the new coldkey",
                    "make sure you control the new coldkey and have its mnemonic backed up",
                ],
                false,
            ),
            (
                &swap as &dyn DynIntent,
                format!("SWAP coldkey to {address} (moves all ownership)"),
                vec![
                    "irreversible: balance, stake, and subnet ownership move to the new coldkey",
                    "the new coldkey must match the announced hash exactly",
                ],
                true,
            ),
            (
                &clear as &dyn DynIntent,
                "clear the pending coldkey swap announcement".into(),
                vec![],
                false,
            ),
            (
                &dispute as &dyn DynIntent,
                "dispute the pending coldkey swap (freezes it for governance)".into(),
                vec!["blocks the swap until the triumvirate resolves the dispute"],
                false,
            ),
        ] {
            intent.validate_dyn()?;
            let built = intent
                .build_dyn(&BuildCx {
                    view: &snapshot as &dyn DynView,
                    origin: OriginView::Account(ORIGIN),
                    runtime,
                    clock: &SystemClock,
                    spec: intent.spec(),
                })
                .await?;
            let preview = intent
                .preflight_dyn(&PreflightCx {
                    view: &snapshot as &dyn DynView,
                    runtime,
                    clock: &SystemClock,
                    spec: intent.spec(),
                    client: &client,
                    dispatch_origin: ORIGIN,
                    fee_payer: ORIGIN,
                    call: &built.call,
                })
                .await?;
            assert_eq!(preview.effects, vec![summary]);
            assert_eq!(preview.warnings, warnings);
            assert!(preview.blocks.is_empty());
            assert!(preview.required_free.is_none());
            assert!(preview.available_free.is_none());
            assert!(preview.estimated_fee.is_none());
            assert!(preview.facts.is_empty());
            assert_eq!(intent.spend_dyn(), Spend::None);
            assert_eq!(intent.affects_all_subnets_dyn(), all_subnets);
            assert!(intent.touches_netuids_dyn().is_empty());
            assert_eq!(intent.spec().signer, "coldkey");
            assert_eq!(intent.spec().origin, "signed");
            assert!(intent.spec().verify.is_none());
            assert!(intent.spec().all_amount_fields.is_empty());
            assert_eq!(
                crate::AnyIntent::from_tool(intent.spec().name, intent.to_json_dyn())?.to_json(),
                intent.to_json_dyn()
            );
        }
        assert_eq!(chain.requests().len(), requests);
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        client.close().await;
        Ok(())
    }

    #[test]
    fn coldkey_tool_inputs_reject_unknown_missing_and_malformed_fields() {
        for op in ["announce_coldkey_swap", "swap_coldkey_announced"] {
            for args in [
                json!({}),
                json!({"new_coldkey_ss58":"invalid address"}),
                json!({"new_coldkey_ss58":null}),
                json!({"new_coldkey_ss58":AccountId::ZERO.to_string(),"unexpected":true}),
            ] {
                assert!(crate::AnyIntent::from_tool(op, args).is_err(), "{op}");
            }
        }
        for op in ["clear_coldkey_swap_announcement", "dispute_coldkey_swap"] {
            assert!(crate::AnyIntent::from_tool(op, json!({})).is_ok(), "{op}");
            for args in [json!({"unexpected":true}), json!([])] {
                assert!(crate::AnyIntent::from_tool(op, args).is_err(), "{op}");
            }
        }
    }
}
