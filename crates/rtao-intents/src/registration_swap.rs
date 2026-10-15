// Derived from RaoFoundation/subtensor sdk/python/bittensor/intents/registration.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Hotkey rotation through Python's pinned legacy call.

use rtao_client::View;
use rtao_runtime::calls::subtensor_module;
use rtao_tx::{BuildCx, Built, Intent, IntentError, PreflightCx};

pub use crate::SwapHotkey;

impl Intent for SwapHotkey {
    fn summary(&self) -> String {
        let scope = self
            .netuid
            .map_or_else(|| "all subnets".into(), |netuid| format!("netuid {netuid}"));
        format!("swap hotkey to {} ({scope})", self.new_hotkey_ss58)
    }

    // The pinned Python intent deliberately calls the legacy swap_hotkey,
    // whose SCALE bytes differ from swap_hotkey_v2 even with keep_stake=false.
    #[allow(deprecated)]
    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        let old = cx.hotkey_address(self.hotkey_ss58.as_ref().map(|address| address.account()))?;
        let call = subtensor_module::swap_hotkey(old, self.new_hotkey_ss58.account(), self.netuid);
        Ok(Built::new(call.compose(cx.runtime)?))
    }

    async fn warnings<V: View + ?Sized>(
        &self,
        _cx: &PreflightCx<'_, V>,
    ) -> Result<Vec<String>, IntentError> {
        Ok(vec![
            "re-keys the neuron identity; the old hotkey stops earning".into(),
        ])
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use rtao_client::{BlockRef, Client, ClientConfig, Network, Snapshot};
    use rtao_primitives::{AccountId, NetUid, Spend};
    use rtao_runtime::{CallBytes, RuntimeHandle};
    use rtao_signer::{KeyView, SignerError};
    use rtao_testkit::{FakeChain, FakeState, RecordingView, rpc::RpcClient};
    use rtao_tx::{IntentInterface, OriginView, SystemClock};
    use serde_json::json;

    const COLDKEY: AccountId = AccountId([7; 32]);
    const HOTKEY: AccountId = AccountId([8; 32]);
    const NEW_HOTKEY: AccountId = AccountId([9; 32]);

    struct PublicKeys;
    impl KeyView for PublicKeys {
        fn hotkey_address(&self) -> Result<AccountId, SignerError> {
            Ok(HOTKEY)
        }
        fn hotkey_public_key(&self) -> Result<[u8; 32], SignerError> {
            Ok(HOTKEY.0)
        }
        fn coldkey_address(&self) -> Result<AccountId, SignerError> {
            Ok(COLDKEY)
        }
    }

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

    fn decode(call: &CallBytes, runtime: &RuntimeHandle) -> anyhow::Result<serde_json::Value> {
        let decoded = runtime
            .core()
            .decode_spec(&rtao_core::runtime::TypeSpec::Call, call.as_bytes(), true)
            .map_err(rtao_core::CoreError::from)?;
        Ok(rtao_core::codec::value::to_corpus_json(&decoded))
    }

    #[tokio::test]
    async fn swap_hotkey_preserves_legacy_call_and_wallet_defaults_for_every_scope()
    -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture().await?;
        let runtime = &snapshot.block().runtime;
        let recorded = RecordingView::new(snapshot.clone());
        let requests = chain.requests().len();
        for (netuid, option_bytes, scope) in [
            (None, vec![0], "all subnets"),
            (Some(NetUid::ROOT), vec![1, 0, 0], "netuid 0"),
            (Some(NetUid(513)), vec![1, 1, 2], "netuid 513"),
            (Some(NetUid(u16::MAX)), vec![1, 255, 255], "netuid 65535"),
        ] {
            let intent = SwapHotkey {
                new_hotkey_ss58: NEW_HOTKEY.into(),
                hotkey_ss58: None,
                netuid,
            };
            let built = intent
                .build(&BuildCx {
                    view: &recorded,
                    origin: OriginView::Keys(&PublicKeys),
                    runtime,
                    clock: &SystemClock,
                    spec: SwapHotkey::SPEC,
                })
                .await?;
            let mut expected = vec![7, 70];
            expected.extend_from_slice(&[8; 32]);
            expected.extend_from_slice(&[9; 32]);
            expected.extend_from_slice(&option_bytes);
            assert_eq!(built.call.as_bytes(), expected);
            assert_eq!(CallBytes::decode(&expected, runtime)?, built.call);
            assert!(built.extras.is_empty());
            let decoded = decode(&built.call, runtime)?;
            assert_eq!(decoded["call_module"], "SubtensorModule");
            assert_eq!(decoded["call_function"], "swap_hotkey");
            assert_eq!(decoded["call_args"][0]["name"], "hotkey");
            assert_eq!(decoded["call_args"][0]["value"], HOTKEY.to_string());
            assert_eq!(decoded["call_args"][1]["name"], "new_hotkey");
            assert_eq!(decoded["call_args"][1]["value"], NEW_HOTKEY.to_string());
            assert_eq!(decoded["call_args"][2]["name"], "netuid");
            assert_eq!(decoded["call_args"][2]["value"], json!(netuid));
            assert_eq!(
                intent.summary(),
                format!("swap hotkey to {NEW_HOTKEY} ({scope})")
            );
            assert_eq!(intent.touches_netuids().as_slice(), netuid.as_slice());
            // Preserve the base Python policy hooks even for the all-subnets
            // variant: it does not override spend or affects_all_subnets.
            assert_eq!(intent.spend(), Spend::None);
            assert!(!intent.affects_all_subnets());
        }
        assert!(recorded.touches().is_empty());
        assert_eq!(chain.requests().len(), requests);
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn swap_hotkey_explicit_accounts_override_defaults_and_retain_ss58_spelling()
    -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture().await?;
        let runtime = &snapshot.block().runtime;
        let requests = chain.requests().len();
        for prefix in [0, 2, 42, 110] {
            let old = AccountId::ZERO.to_ss58(prefix);
            let new = NEW_HOTKEY.to_ss58(prefix);
            let intent = SwapHotkey {
                new_hotkey_ss58: new.parse()?,
                hotkey_ss58: Some(old.parse()?),
                netuid: Some(NetUid(513)),
            };
            for origin in [OriginView::Keys(&PublicKeys), OriginView::Account(COLDKEY)] {
                let built = intent
                    .build(&BuildCx {
                        view: &snapshot,
                        origin,
                        runtime,
                        clock: &SystemClock,
                        spec: SwapHotkey::SPEC,
                    })
                    .await?;
                let mut expected = vec![7, 70];
                expected.extend_from_slice(&[0; 32]);
                expected.extend_from_slice(&[9; 32]);
                expected.extend_from_slice(&[1, 1, 2]);
                assert_eq!(built.call.as_bytes(), expected);
                assert_eq!(CallBytes::decode(&expected, runtime)?, built.call);
                assert_eq!(intent.to_json()["hotkey_ss58"], old);
                assert_eq!(intent.to_json()["new_hotkey_ss58"], new);
                assert_eq!(
                    intent.summary(),
                    format!("swap hotkey to {new} (netuid 513)")
                );
                assert_eq!(
                    crate::AnyIntent::from_tool("swap_hotkey", intent.to_json())?.to_json(),
                    intent.to_json()
                );
            }
        }
        let intent = SwapHotkey {
            new_hotkey_ss58: NEW_HOTKEY.into(),
            hotkey_ss58: None,
            netuid: None,
        };
        assert_eq!(
            intent
                .build(&BuildCx {
                    view: &snapshot,
                    origin: OriginView::Account(COLDKEY),
                    runtime,
                    clock: &SystemClock,
                    spec: SwapHotkey::SPEC,
                })
                .await
                .expect_err("bare coldkey has no attached hotkey")
                .to_string(),
            "swap_hotkey: this signer carries no hotkey to default to; pass the hotkey address explicitly or sign with a Wallet"
        );
        assert_eq!(chain.requests().len(), requests);
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn swap_hotkey_preflight_has_exact_warning_and_no_added_chain_checks()
    -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture().await?;
        let runtime = &snapshot.block().runtime;
        let requests = chain.requests().len();
        for netuid in [None, Some(NetUid::ROOT), Some(NetUid(513))] {
            // Python leaves same-key refusal, registration/cooldown checks and
            // recycle fees to chain execution; construction remains a pure call.
            let intent = SwapHotkey {
                new_hotkey_ss58: HOTKEY.into(),
                hotkey_ss58: Some(HOTKEY.into()),
                netuid,
            };
            intent.validate()?;
            let built = intent
                .build(&BuildCx {
                    view: &snapshot,
                    origin: OriginView::Account(COLDKEY),
                    runtime,
                    clock: &SystemClock,
                    spec: SwapHotkey::SPEC,
                })
                .await?;
            let preview = intent
                .preflight(&PreflightCx {
                    view: &snapshot,
                    runtime,
                    clock: &SystemClock,
                    spec: SwapHotkey::SPEC,
                    client: &client,
                    dispatch_origin: COLDKEY,
                    fee_payer: NEW_HOTKEY,
                    call: &built.call,
                })
                .await?;
            assert_eq!(preview.effects, vec![intent.summary()]);
            assert_eq!(
                preview.warnings,
                vec!["re-keys the neuron identity; the old hotkey stops earning"]
            );
            assert!(preview.blocks.is_empty());
            assert!(preview.facts.is_empty());
            assert!(preview.required_free.is_none());
            assert!(preview.available_free.is_none());
            assert!(preview.estimated_fee.is_none());
            assert_eq!(intent.spend(), Spend::None);
            assert!(!intent.affects_all_subnets());
        }
        assert_eq!(SwapHotkey::SPEC.signer, "coldkey");
        assert_eq!(SwapHotkey::SPEC.origin, "signed");
        assert!(SwapHotkey::SPEC.verify.is_none());
        assert!(SwapHotkey::SPEC.all_amount_fields.is_empty());
        assert_eq!(chain.requests().len(), requests);
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        client.close().await;
        Ok(())
    }

    #[test]
    fn swap_hotkey_tool_defaults_and_native_account_and_netuid_validation() -> anyhow::Result<()> {
        let default = crate::AnyIntent::from_tool(
            "swap_hotkey",
            json!({"new_hotkey_ss58":NEW_HOTKEY.to_string()}),
        )?;
        assert_eq!(default.to_json()["hotkey_ss58"], serde_json::Value::Null);
        assert_eq!(default.to_json()["netuid"], serde_json::Value::Null);
        assert_eq!(
            default.as_dyn().summary_dyn(),
            format!("swap hotkey to {NEW_HOTKEY} (all subnets)")
        );
        for input in [
            json!({}),
            json!({"new_hotkey_ss58":null}),
            json!({"new_hotkey_ss58":"invalid"}),
            json!({"new_hotkey_ss58":NEW_HOTKEY.to_string(),"hotkey_ss58":"invalid"}),
            json!({"new_hotkey_ss58":NEW_HOTKEY.to_string(),"netuid":-1}),
            json!({"new_hotkey_ss58":NEW_HOTKEY.to_string(),"netuid":65536}),
            json!({"new_hotkey_ss58":NEW_HOTKEY.to_string(),"netuid":1.5}),
            json!({"new_hotkey_ss58":NEW_HOTKEY.to_string(),"unexpected":true}),
        ] {
            assert!(crate::AnyIntent::from_tool("swap_hotkey", input).is_err());
        }
        Ok(())
    }
}
