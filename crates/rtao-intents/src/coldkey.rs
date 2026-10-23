// Derived from RaoFoundation/subtensor sdk/python/bittensor/intents/coldkey.py @ 8cf229f8ef969389d054a4c161b53ffd164a041f (Apache-2.0); modified by rtao.
//! Coldkey migration announcements, execution, cancellation, disputes and
//! destination reservations.
//!
//! Announcing publishes the cleartext destination through
//! `announce_coldkey_swap_v2`, which the chain validates and reserves before
//! charging the swap cost. The legacy hash-only `announce_coldkey_swap`
//! extrinsic stays raw-only for callers that must keep the destination secret.

use rtao_client::View;
use rtao_tx::{BuildCx, Built, Intent, IntentError, PreflightCx};

pub use crate::{
    AnnounceColdkeySwap, ClearColdkeySwapAnnouncement, DisputeColdkeySwap,
    ReleaseOwnColdkeySwapDestinationReservation, SwapColdkeyAnnounced,
};

impl Intent for AnnounceColdkeySwap {
    fn summary(&self) -> String {
        format!("announce coldkey swap to {}", self.new_coldkey_ss58)
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        crate::generated::passthrough::announce_coldkey_swap(self, cx).await
    }

    async fn warnings<V: View + ?Sized>(
        &self,
        _cx: &PreflightCx<'_, V>,
    ) -> Result<Vec<String>, IntentError> {
        Ok(vec![
            "after the announcement delay, swap_coldkey_announced will move EVERYTHING this coldkey owns (balance, stake, subnets) to the new coldkey".into(),
            "make sure you control the new coldkey and have its mnemonic backed up".into(),
            "if the destination already holds assets, the swap merges into it".into(),
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

impl Intent for ReleaseOwnColdkeySwapDestinationReservation {
    fn summary(&self) -> String {
        "release this account's coldkey-swap destination reservation".into()
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        crate::generated::passthrough::release_own_coldkey_swap_destination_reservation(self, cx)
            .await
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
    use rtao_primitives::{AccountId, Spend, Ss58Address};
    use rtao_runtime::{CallBytes, RuntimeHandle};
    use rtao_testkit::{FakeChain, FakeState, rpc::RpcClient};
    use rtao_tx::{DynIntent, OriginView, SystemClock};
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

    fn decode(call: &CallBytes, runtime: &RuntimeHandle) -> anyhow::Result<Value> {
        let decoded = runtime
            .core()
            .decode_spec(&rtao_core::runtime::TypeSpec::Call, call.as_bytes(), true)
            .map_err(rtao_core::CoreError::from)?;
        Ok(rtao_core::codec::value::to_corpus_json(&decoded))
    }

    #[tokio::test]
    async fn coldkey_swaps_keep_ss58_spelling_but_encode_one_account_across_prefixes()
    -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture().await?;
        let runtime = &snapshot.block().runtime;
        let requests = chain.requests().len();
        let account = AccountId::ZERO;
        // Announce publishes the cleartext key through announce_coldkey_swap_v2
        // (index 155); the chain derives its stored commitment itself.
        let mut announced = vec![7, 155];
        announced.extend_from_slice(account.as_bytes());
        let mut swapped = vec![7, 126];
        swapped.extend_from_slice(account.as_bytes());
        // The pinned core accepts registered network formats; arbitrary custom
        // prefixes can be rendered but are refused by its SS58 parser.
        for prefix in [0, 2, 42, 110] {
            let spelling = account.to_ss58(prefix);
            let announce = AnnounceColdkeySwap {
                new_coldkey_ss58: spelling.parse()?,
            };
            let swap = SwapColdkeyAnnounced {
                new_coldkey_ss58: spelling.parse()?,
            };
            for (intent, summary, bytes, function) in [
                (
                    &announce as &dyn DynIntent,
                    format!("announce coldkey swap to {spelling}"),
                    announced.as_slice(),
                    "announce_coldkey_swap_v2",
                ),
                (
                    &swap as &dyn DynIntent,
                    format!("SWAP coldkey to {spelling} (moves all ownership)"),
                    swapped.as_slice(),
                    "swap_coldkey_announced",
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
                assert_eq!(decoded["call_function"], function);
                assert_eq!(decoded["call_args"][0]["name"], "new_coldkey");
                assert_eq!(decoded["call_args"][0]["value"], json!(account.to_string()));
            }
        }
        let unknown_format = account.to_ss58(1000);
        assert!(unknown_format.parse::<Ss58Address>().is_err());
        for op in ["announce_coldkey_swap", "swap_coldkey_announced"] {
            assert!(
                crate::AnyIntent::from_tool(op, json!({"new_coldkey_ss58":unknown_format}))
                    .is_err()
            );
        }
        assert_eq!(chain.requests().len(), requests);
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn coldkey_clear_dispute_and_release_encode_exact_zero_argument_calls()
    -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture().await?;
        let runtime = &snapshot.block().runtime;
        let requests = chain.requests().len();
        let clear = ClearColdkeySwapAnnouncement {};
        let dispute = DisputeColdkeySwap {};
        let release = ReleaseOwnColdkeySwapDestinationReservation {};
        for (intent, bytes) in [
            (&clear as &dyn DynIntent, [7u8, 133]),
            (&dispute as &dyn DynIntent, [7u8, 127]),
            (&release as &dyn DynIntent, [7u8, 154]),
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
        let release = ReleaseOwnColdkeySwapDestinationReservation {};
        for (intent, summary, warnings, all_subnets) in [
            (
                &announce as &dyn DynIntent,
                format!("announce coldkey swap to {address}"),
                vec![
                    "after the announcement delay, swap_coldkey_announced will move EVERYTHING this coldkey owns (balance, stake, subnets) to the new coldkey",
                    "make sure you control the new coldkey and have its mnemonic backed up",
                    "if the destination already holds assets, the swap merges into it",
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
            (
                &release as &dyn DynIntent,
                "release this account's coldkey-swap destination reservation".into(),
                vec![],
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
        for op in [
            "clear_coldkey_swap_announcement",
            "dispute_coldkey_swap",
            "release_own_coldkey_swap_destination_reservation",
        ] {
            assert!(crate::AnyIntent::from_tool(op, json!({})).is_ok(), "{op}");
            for args in [json!({"unexpected":true}), json!([])] {
                assert!(crate::AnyIntent::from_tool(op, args).is_err(), "{op}");
            }
        }
    }
}
