// Derived from RaoFoundation/subtensor sdk/python/bittensor/intents/registration.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Registration calls and their current burn and collateral previews.

use rtao_client::{View, ViewExt};
use rtao_primitives::{Balance, DynBalance, NetUid, Spend, Tao};
use rtao_runtime::storage::subtensor_module as st;
use rtao_tx::{BuildCx, Built, Intent, IntentError, PreflightCx};
use smallvec::SmallVec;

pub use crate::{BurnedRegister, RegisterSubnet, RootRegister, StartCall};

/// Split a subnet's current registration price into burned and locked TAO.
///
/// All reads use one pinned state. Root has no collateral path, and the
/// non-root split uses integer division by the full u16 proportion.
pub async fn neuron_registration_split<V: View + ?Sized>(
    at: &V,
    netuid: NetUid,
) -> Result<(Balance<Tao>, Balance<Tao>), IntentError> {
    let snapshot = at.pin().await?;
    if netuid == NetUid::ROOT {
        let cost = snapshot.storage(&st::BURN, (netuid,)).await?;
        return Ok((Balance::from_rao(cost.0), Balance::ZERO));
    }
    let (cost, share) = snapshot
        .fetch((
            st::BURN.at((netuid,)),
            st::COLLATERAL_LOCK_SHARE.at((netuid,)),
        ))
        .await?;
    let cost = Balance::<Tao>::from_rao(cost.0);
    let lock = Balance::<Tao>::try_from(
        u128::from(cost.rao()) * u128::from(share) / u128::from(u16::MAX),
    )?;
    Ok((cost.checked_sub(lock)?, lock))
}

impl Intent for BurnedRegister {
    fn summary(&self) -> String {
        let target = self
            .hotkey_ss58
            .as_ref()
            .map_or("wallet hotkey", rtao_primitives::Ss58Address::as_str);
        format!(
            "register {target} on netuid {} (burned/collateral)",
            self.netuid
        )
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        crate::generated::passthrough::burned_register(self, cx).await
    }

    async fn effects<V: View + ?Sized>(
        &self,
        cx: &PreflightCx<'_, V>,
    ) -> Result<Vec<String>, IntentError> {
        let (burn, lock) = neuron_registration_split(cx.view, self.netuid).await?;
        let lock_line = if lock.is_zero() {
            "lock none (full cost is burned)".into()
        } else {
            format!("lock {} as miner collateral", DynBalance::from(lock))
        };
        Ok(vec![
            self.summary(),
            format!("burn {} (destroyed)", DynBalance::from(burn)),
            lock_line,
        ])
    }

    fn spend(&self) -> Spend {
        Spend::Unbounded
    }
}

impl Intent for RegisterSubnet {
    fn summary(&self) -> String {
        "register a new subnet".into()
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        crate::generated::passthrough::register_subnet(self, cx).await
    }

    fn spend(&self) -> Spend {
        Spend::Unbounded
    }
}

impl Intent for StartCall {
    fn summary(&self) -> String {
        format!("activate subnet {}", self.netuid)
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        crate::generated::passthrough::start_call(self, cx).await
    }
}

impl Intent for RootRegister {
    fn summary(&self) -> String {
        let target = self
            .hotkey_ss58
            .as_ref()
            .map_or("wallet hotkey", rtao_primitives::Ss58Address::as_str);
        format!("register {target} on the root network")
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        crate::generated::passthrough::root_register(self, cx).await
    }

    async fn effects<V: View + ?Sized>(
        &self,
        cx: &PreflightCx<'_, V>,
    ) -> Result<Vec<String>, IntentError> {
        let (burn, _) = neuron_registration_split(cx.view, NetUid::ROOT).await?;
        Ok(vec![
            self.summary(),
            format!("burn {} (recycled into issuance)", DynBalance::from(burn)),
            "lock none".into(),
            "auto-childkey to every subnet owner unless opted out".into(),
        ])
    }

    fn touches_netuids(&self) -> SmallVec<[NetUid; 2]> {
        SmallVec::from_slice(&[NetUid::ROOT])
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use rtao_client::{BlockRef, Client, ClientConfig, Network, Snapshot};
    use rtao_primitives::{AccountId, TaoBalance};
    use rtao_runtime::calls::subtensor_module as calls;
    use rtao_signer::{KeyView, SignerError};
    use rtao_testkit::{FakeChain, FakeState, RecordingView, rpc::RpcClient};
    use rtao_tx::{IntentInterface, IntentPreflight, OriginView, SystemClock};
    use serde_json::json;

    const OWNER: AccountId = AccountId([7; 32]);
    const HOTKEY: AccountId = AccountId([8; 32]);
    const NETUID: NetUid = NetUid(3);

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

    async fn fixture(
        setup: impl FnOnce(&mut FakeState) -> anyhow::Result<()>,
    ) -> anyhow::Result<(FakeChain, Client, Snapshot)> {
        let mut state = FakeState::try_default()?;
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

    async fn preview<I: Intent>(
        intent: &I,
        snapshot: &Snapshot,
        client: &Client,
    ) -> anyhow::Result<IntentPreflight> {
        let build_cx = BuildCx {
            view: snapshot,
            origin: OriginView::Keys(&PublicKeys),
            runtime: &snapshot.block().runtime,
            clock: &SystemClock,
            spec: I::SPEC,
        };
        let built = intent.build(&build_cx).await?;
        Ok(intent
            .preflight(&PreflightCx {
                view: snapshot,
                runtime: &snapshot.block().runtime,
                clock: &SystemClock,
                spec: I::SPEC,
                client,
                dispatch_origin: OWNER,
                fee_payer: OWNER,
                call: &built.call,
            })
            .await?)
    }

    #[tokio::test]
    async fn registration_split_is_integer_exact_at_all_share_boundaries() -> anyhow::Result<()> {
        for (cost, share) in [
            (0, 0),
            (1, u16::MAX / 2),
            (65_535, 1),
            (2_000_000_001, 32_768),
            (u64::MAX, 0),
            (u64::MAX, u16::MAX),
        ] {
            let (_, client, snapshot) = fixture(|state| {
                state.seed(&st::BURN, (NETUID,), TaoBalance(cost))?;
                state.seed(&st::COLLATERAL_LOCK_SHARE, (NETUID,), share)?;
                Ok(())
            })
            .await?;
            let (burn, lock) = neuron_registration_split(&snapshot, NETUID).await?;
            let expected_lock = u128::from(cost) * u128::from(share) / u128::from(u16::MAX);
            assert_eq!(u128::from(lock.rao()), expected_lock);
            assert_eq!(burn.rao().checked_add(lock.rao()), Some(cost));
            client.close().await;
        }
        Ok(())
    }

    #[tokio::test]
    async fn root_split_ignores_collateral_and_observes_only_its_burn() -> anyhow::Result<()> {
        let (_, client, snapshot) = fixture(|state| {
            state.seed(&st::BURN, (NetUid::ROOT,), TaoBalance(123))?;
            state.seed(&st::COLLATERAL_LOCK_SHARE, (NetUid::ROOT,), u16::MAX)?;
            Ok(())
        })
        .await?;
        let recorded = RecordingView::new(snapshot);
        assert_eq!(
            neuron_registration_split(&recorded, NetUid::ROOT).await?,
            (Balance::from_rao(123), Balance::ZERO)
        );
        let touches = recorded.touches();
        assert_eq!(touches.len(), 1);
        assert_eq!(touches[0].item, "Burn");
        assert_eq!(recorded.block_hashes().len(), 1);
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn registration_effects_spend_and_touched_root_match_python() -> anyhow::Result<()> {
        let (_, client, snapshot) = fixture(|state| {
            state.seed(&st::BURN, (NETUID,), TaoBalance(65_535))?;
            state.seed(&st::COLLATERAL_LOCK_SHARE, (NETUID,), 1u16)?;
            state.seed(&st::BURN, (NetUid::ROOT,), TaoBalance(2_000_000_000))?;
            Ok(())
        })
        .await?;
        let burned = BurnedRegister {
            netuid: NETUID,
            hotkey_ss58: None,
        };
        assert_eq!(
            preview(&burned, &snapshot, &client).await?.effects,
            [
                "register wallet hotkey on netuid 3 (burned/collateral)",
                "burn τ0.000065534 (destroyed)",
                "lock τ0.000000001 as miner collateral",
            ]
        );
        assert_eq!(burned.spend(), Spend::Unbounded);
        assert_eq!(burned.touches_netuids().as_slice(), &[NETUID]);
        assert!(BurnedRegister::SPEC.mev_shield_required);
        assert!(BurnedRegister::SPEC.mev_shield_default);
        let root = RootRegister { hotkey_ss58: None };
        assert_eq!(
            preview(&root, &snapshot, &client).await?.effects,
            [
                "register wallet hotkey on the root network",
                "burn τ2.000000000 (recycled into issuance)",
                "lock none",
                "auto-childkey to every subnet owner unless opted out",
            ]
        );
        // RootRegister inherits Python's base spend default despite its burn preview.
        assert_eq!(root.spend(), Spend::None);
        assert_eq!(root.touches_netuids().as_slice(), &[NetUid::ROOT]);
        assert_eq!(
            RegisterSubnet { hotkey_ss58: None }.spend(),
            Spend::Unbounded
        );
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn zero_collateral_preview_and_default_effects_keep_exact_text() -> anyhow::Result<()> {
        let (_, client, snapshot) = fixture(|state| {
            state.seed(&st::BURN, (NETUID,), TaoBalance(1_000_000_000))?;
            state.seed(&st::COLLATERAL_LOCK_SHARE, (NETUID,), 0u16)?;
            Ok(())
        })
        .await?;
        let burned = BurnedRegister {
            netuid: NETUID,
            hotkey_ss58: None,
        };
        assert_eq!(
            preview(&burned, &snapshot, &client).await?.effects[2],
            "lock none (full cost is burned)"
        );
        assert_eq!(
            preview(&RegisterSubnet { hotkey_ss58: None }, &snapshot, &client)
                .await?
                .effects,
            ["register a new subnet"]
        );
        assert_eq!(
            preview(&StartCall { netuid: NETUID }, &snapshot, &client)
                .await?
                .effects,
            ["activate subnet 3"]
        );
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn generated_registration_builds_use_only_public_hotkey_identities() -> anyhow::Result<()>
    {
        let (chain, client, snapshot) = fixture(|_| Ok(())).await?;
        let runtime = &snapshot.block().runtime;
        for op in ["burned_register", "register_subnet", "root_register"] {
            for explicit in [false, true] {
                let spelling = HOTKEY.to_ss58(2);
                let mut args = json!({});
                if op == "burned_register" {
                    args["netuid"] = json!(NETUID.0);
                }
                if explicit {
                    args["hotkey_ss58"] = json!(spelling);
                }
                let intent = crate::AnyIntent::from_tool(op, args)?;
                assert_eq!(
                    intent.to_json()["hotkey_ss58"],
                    if explicit {
                        json!(spelling)
                    } else {
                        json!(null)
                    }
                );
                let context = BuildCx {
                    view: &snapshot as &dyn rtao_client::DynView,
                    origin: if explicit {
                        OriginView::Account(OWNER)
                    } else {
                        OriginView::Keys(&PublicKeys)
                    },
                    runtime,
                    clock: &SystemClock,
                    spec: intent.spec(),
                };
                let built = intent.as_dyn().build_dyn(&context).await?;
                let expected = match op {
                    "burned_register" => calls::burned_register(NETUID, HOTKEY),
                    "register_subnet" => calls::register_network(HOTKEY),
                    "root_register" => calls::root_register(HOTKEY),
                    _ => anyhow::bail!("unexpected registration operation: {op}"),
                }
                .compose(runtime)?;
                assert_eq!(
                    built.call.as_bytes(),
                    expected.as_bytes(),
                    "{op}/{explicit}"
                );
                assert!(built.extras.is_empty());
                assert_eq!(crate::AnyIntent::from_tool(op, intent.to_json())?, intent);
                if explicit && op != "register_subnet" {
                    assert!(intent.as_dyn().summary_dyn().contains(&spelling));
                }
                if !explicit {
                    let bare_context = BuildCx {
                        origin: OriginView::Account(OWNER),
                        ..context
                    };
                    let error = intent.as_dyn().build_dyn(&bare_context).await.unwrap_err();
                    assert!(matches!(
                        &error,
                        IntentError::Signer(SignerError::KeyView(_))
                    ));
                    assert!(
                        error
                            .to_string()
                            .contains("pass the hotkey address explicitly")
                    );
                }
            }
        }
        let start = StartCall { netuid: NETUID };
        let context = BuildCx {
            view: &snapshot,
            origin: OriginView::Account(OWNER),
            runtime,
            clock: &SystemClock,
            spec: StartCall::SPEC,
        };
        assert_eq!(
            start.build(&context).await?.call.as_bytes(),
            calls::start_call(NETUID).compose(runtime)?.as_bytes()
        );
        assert!(chain.state().lock().unwrap().submissions.is_empty());
        client.close().await;
        Ok(())
    }
}
