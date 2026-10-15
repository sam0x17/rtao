// Derived from RaoFoundation/subtensor sdk/python/bittensor/intents/leasing.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Crowdloan-funded subnet leases and their beneficiary's termination call.

use rtao_client::View;
use rtao_primitives::Spend;
use rtao_tx::{BuildCx, Built, Intent, IntentError, PreflightCx};

pub use crate::{RegisterLeasedNetwork, TerminateLease};

impl Intent for RegisterLeasedNetwork {
    fn summary(&self) -> String {
        let horizon = self.end_block.map_or_else(
            || "perpetual".into(),
            |block| format!("until block {block}"),
        );
        format!(
            "register leased subnet ({}% to contributors, {horizon})",
            self.emissions_share.0
        )
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        self.validate()?;
        // Python leaves crowdloan-context admission and the floating cost to
        // dispatch. A standalone semantic build is still a valid preview.
        crate::generated::passthrough::register_leased_network(self, cx).await
    }

    fn validate(&self) -> Result<(), IntentError> {
        if self.emissions_share.0 > 100 {
            return Err(IntentError::Validation(format!(
                "emissions_share must be a percent 0-100, got {}",
                self.emissions_share.0
            )));
        }
        Ok(())
    }

    fn spend(&self) -> Spend {
        Spend::Unbounded
    }
}

impl Intent for TerminateLease {
    fn summary(&self) -> String {
        format!(
            "terminate lease {} and take subnet ownership",
            self.lease_id
        )
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        crate::generated::passthrough::terminate_lease(self, cx).await
    }

    async fn warnings<V: View + ?Sized>(
        &self,
        _cx: &PreflightCx<'_, V>,
    ) -> Result<Vec<String>, IntentError> {
        Ok(vec![
            "only succeeds after the lease's end block has passed".into(),
        ])
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use rtao_client::{BlockRef, Client, ClientConfig, DynView, Network, Snapshot};
    use rtao_primitives::{AccountId, Percent};
    use rtao_runtime::CallBytes;
    use rtao_signer::{KeyView, SignerError};
    use rtao_testkit::{FakeChain, FakeState, rpc::RpcClient};
    use rtao_tx::{DynIntent, IntentInterface, OriginView, SystemClock};
    use serde_json::{Value, json};

    const OWNER: AccountId = AccountId([7; 32]);
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
            Ok(OWNER)
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

    #[tokio::test]
    async fn leased_registration_preserves_percent_horizon_and_native_scale() -> anyhow::Result<()>
    {
        let (chain, client, snapshot) = fixture().await?;
        let runtime = &snapshot.block().runtime;
        let requests = chain.requests().len();
        for share in [0, 1, 50, 100] {
            for end in [None, Some(0), Some(0x0403_0201), Some(u32::MAX)] {
                let intent = RegisterLeasedNetwork {
                    emissions_share: Percent(share),
                    end_block: end,
                };
                let built = intent
                    .build(&BuildCx {
                        view: &snapshot,
                        origin: OriginView::Account(OWNER),
                        runtime,
                        clock: &SystemClock,
                        spec: RegisterLeasedNetwork::SPEC,
                    })
                    .await?;
                let mut expected = vec![7, 110, share];
                if let Some(end) = end {
                    expected.push(1);
                    expected.extend_from_slice(&end.to_le_bytes());
                } else {
                    expected.push(0);
                }
                assert_eq!(built.call.as_bytes(), expected);
                assert_eq!(CallBytes::decode(&expected, runtime)?, built.call);
                assert!(built.extras.is_empty());
                let horizon =
                    end.map_or_else(|| "perpetual".into(), |end| format!("until block {end}"));
                assert_eq!(
                    intent.summary(),
                    format!("register leased subnet ({share}% to contributors, {horizon})")
                );
                let json = intent.to_json();
                assert_eq!(json["emissions_share"], share);
                assert_eq!(json["end_block"], json!(end));
                assert_eq!(
                    crate::AnyIntent::from_tool("register_leased_network", json.clone())?.to_json(),
                    json
                );
            }
        }
        assert_eq!(chain.requests().len(), requests);
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn terminate_lease_uses_explicit_or_wallet_hotkey_without_state_guesses()
    -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture().await?;
        let runtime = &snapshot.block().runtime;
        let requests = chain.requests().len();
        for lease_id in [0, 0x0403_0201, u32::MAX] {
            for prefix in [0, 2, 42] {
                let spelling = HOTKEY.to_ss58(prefix);
                let intent = TerminateLease {
                    lease_id,
                    hotkey_ss58: Some(spelling.parse()?),
                };
                let built = intent
                    .build(&BuildCx {
                        view: &snapshot,
                        origin: OriginView::Account(OWNER),
                        runtime,
                        clock: &SystemClock,
                        spec: TerminateLease::SPEC,
                    })
                    .await?;
                let mut expected = vec![7, 111];
                expected.extend_from_slice(&lease_id.to_le_bytes());
                expected.extend_from_slice(&HOTKEY.0);
                assert_eq!(built.call.as_bytes(), expected);
                assert_eq!(CallBytes::decode(&expected, runtime)?, built.call);
                assert!(built.extras.is_empty());
                assert_eq!(intent.to_json()["hotkey_ss58"], spelling);
                let json = intent.to_json();
                assert_eq!(
                    crate::AnyIntent::from_tool("terminate_lease", json.clone())?.to_json(),
                    json
                );
                let default = TerminateLease {
                    lease_id,
                    hotkey_ss58: None,
                };
                assert_eq!(
                    default
                        .build(&BuildCx {
                            view: &snapshot,
                            origin: OriginView::Keys(&PublicKeys),
                            runtime,
                            clock: &SystemClock,
                            spec: TerminateLease::SPEC,
                        })
                        .await?
                        .call,
                    built.call
                );
                assert!(
                    default
                        .build(&BuildCx {
                            view: &snapshot,
                            origin: OriginView::Account(OWNER),
                            runtime,
                            clock: &SystemClock,
                            spec: TerminateLease::SPEC,
                        })
                        .await
                        .is_err()
                );
            }
        }
        assert_eq!(chain.requests().len(), requests);
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn leasing_preflight_keeps_python_warnings_and_spend_policy() -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture().await?;
        let runtime = &snapshot.block().runtime;
        let requests = chain.requests().len();
        let registration = RegisterLeasedNetwork {
            emissions_share: Percent(100),
            end_block: None,
        };
        let termination = TerminateLease {
            lease_id: 123,
            hotkey_ss58: None,
        };
        for (intent, summary, warnings, spend) in [
            (
                &registration as &dyn DynIntent,
                "register leased subnet (100% to contributors, perpetual)",
                vec![],
                Spend::Unbounded,
            ),
            (
                &termination as &dyn DynIntent,
                "terminate lease 123 and take subnet ownership",
                vec!["only succeeds after the lease's end block has passed"],
                Spend::None,
            ),
        ] {
            let built = intent
                .build_dyn(&BuildCx {
                    view: &snapshot as &dyn DynView,
                    origin: OriginView::Keys(&PublicKeys),
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
                    dispatch_origin: OWNER,
                    fee_payer: OWNER,
                    call: &built.call,
                })
                .await?;
            assert_eq!(intent.summary_dyn(), summary);
            assert_eq!(preview.effects, [summary]);
            assert_eq!(preview.warnings, warnings);
            assert!(preview.blocks.is_empty());
            assert!(preview.required_free.is_none());
            assert!(preview.available_free.is_none());
            assert!(preview.estimated_fee.is_none());
            assert!(preview.facts.is_empty());
            assert_eq!(intent.spend_dyn(), spend);
            assert!(intent.touches_netuids_dyn().is_empty());
            assert!(!intent.affects_all_subnets_dyn());
            assert_eq!(intent.spec().signer, "coldkey");
            assert_eq!(intent.spec().origin, "signed");
            assert!(!intent.spec().mev_shield_required);
            assert!(intent.spec().verify.is_none());
            assert!(intent.spec().all_amount_fields.is_empty());
        }
        assert_eq!(chain.requests().len(), requests);
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        client.close().await;
        Ok(())
    }

    #[test]
    fn lease_constructors_refuse_invalid_percent_native_bounds_and_unknown_fields()
    -> anyhow::Result<()> {
        for share in [101, 255] {
            let intent = RegisterLeasedNetwork {
                emissions_share: Percent(share),
                end_block: None,
            };
            assert_eq!(
                intent.validate().expect_err("invalid percent").to_string(),
                format!("emissions_share must be a percent 0-100, got {share}")
            );
        }
        assert_eq!(
            crate::AnyIntent::from_tool("register_leased_network", json!({"emissions_share":0}))?
                .to_json(),
            json!({"op":"register_leased_network","emissions_share":0,"end_block":null})
        );
        for args in [
            json!({"emissions_share":101}),
            json!({"emissions_share":-1}),
            json!({"emissions_share":256}),
            json!({"emissions_share":null}),
            json!({"emissions_share":1,"end_block":-1}),
            json!({"emissions_share":1,"end_block":4_294_967_296_u64}),
            json!({"emissions_share":1,"extra":true}),
            json!({}),
            json!([]),
        ] {
            assert!(crate::AnyIntent::from_tool("register_leased_network", args).is_err());
        }
        assert_eq!(
            crate::AnyIntent::from_tool("terminate_lease", json!({"lease_id":0}))?.to_json(),
            json!({"op":"terminate_lease","lease_id":0,"hotkey_ss58":null})
        );
        for args in [
            json!({"lease_id":-1}),
            json!({"lease_id":4_294_967_296_u64}),
            json!({"lease_id":null}),
            json!({"lease_id":0,"hotkey_ss58":"invalid"}),
            json!({"lease_id":0,"hotkey_ss58":""}),
            json!({"lease_id":0,"extra":true}),
            json!({}),
            Value::Null,
        ] {
            assert!(crate::AnyIntent::from_tool("terminate_lease", args).is_err());
        }
        Ok(())
    }
}
