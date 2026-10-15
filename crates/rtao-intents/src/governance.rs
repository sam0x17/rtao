// Derived from RaoFoundation/subtensor sdk/python/bittensor/intents/governance.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Subnet symbol, mechanism and bounded pruning operations.

use rtao_client::View;
use rtao_primitives::NetUid;
use rtao_tx::{BuildCx, Built, Intent, IntentError, PreflightCx};

pub use crate::{SetMechanismCount, TrimNullSubnetBatch, TrimSubnet, UpdateSymbol};

async fn owner_blocks<V: View + ?Sized>(
    cx: &PreflightCx<'_, V>,
    netuid: NetUid,
    setter: &str,
    null_target: Option<u16>,
) -> Result<Vec<String>, IntentError> {
    let (_, reasons) = crate::hyperparameters::HyperparamPlanner::new(cx.dispatch_origin)
        .operation_window(cx.view, netuid, setter, null_target)
        .await
        .map_err(crate::hyperparameters::planner_error)?;
    Ok(reasons)
}

impl Intent for TrimSubnet {
    fn summary(&self) -> String {
        format!("trim netuid {} to at most {} UIDs", self.netuid, self.max_n)
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        crate::generated::passthrough::trim_subnet(self, cx).await
    }

    async fn blocks<V: View + ?Sized>(
        &self,
        cx: &PreflightCx<'_, V>,
    ) -> Result<Vec<String>, IntentError> {
        owner_blocks(cx, self.netuid, "sudo_trim_to_max_allowed_uids", None).await
    }
}

impl Intent for TrimNullSubnetBatch {
    fn summary(&self) -> String {
        format!(
            "prune one Null batch on netuid {} toward {} UIDs",
            self.netuid, self.target
        )
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        crate::generated::passthrough::trim_null_subnet_batch(self, cx).await
    }

    async fn blocks<V: View + ?Sized>(
        &self,
        cx: &PreflightCx<'_, V>,
    ) -> Result<Vec<String>, IntentError> {
        owner_blocks(
            cx,
            self.netuid,
            "sudo_trim_null_uids_batch",
            Some(self.target),
        )
        .await
    }
}

impl Intent for SetMechanismCount {
    fn summary(&self) -> String {
        format!(
            "set mechanism count to {} on netuid {}",
            self.mechanism_count, self.netuid
        )
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        crate::generated::passthrough::set_mechanism_count(self, cx).await
    }

    async fn blocks<V: View + ?Sized>(
        &self,
        cx: &PreflightCx<'_, V>,
    ) -> Result<Vec<String>, IntentError> {
        owner_blocks(cx, self.netuid, "sudo_set_mechanism_count", None).await
    }
}

impl Intent for UpdateSymbol {
    fn summary(&self) -> String {
        format!(
            "set symbol for netuid {} to {}",
            self.netuid,
            rtao_primitives::pyfmt::string_repr(&self.symbol)
        )
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        crate::generated::passthrough::update_symbol(self, cx).await
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use rtao_client::{BlockRef, Client, ClientConfig, Network, Snapshot};
    use rtao_primitives::{AccountId, DefaultAccount, MechId, Spend};
    use rtao_runtime::{
        calls::{admin_utils, subtensor_module},
        storage::subtensor_module as st,
    };
    use rtao_testkit::{FakeChain, FakeState, rpc::RpcClient};
    use rtao_tx::{IntentPreflight, OriginView, SystemClock};
    use serde_json::json;

    const NETUID: NetUid = NetUid(3);
    const OWNER: AccountId = AccountId([7; 32]);
    const PAYER: AccountId = AccountId([9; 32]);

    async fn fixture(
        setup: impl FnOnce(&mut FakeState) -> anyhow::Result<()>,
    ) -> anyhow::Result<(FakeChain, Client, Snapshot)> {
        let mut state = FakeState::try_default()?;
        state.seed(&st::SUBNET_OWNER, (NETUID,), DefaultAccount::from(OWNER))?;
        state.seed(&st::TEMPO, (NETUID,), 360u16)?;
        state.seed(&st::LAST_EPOCH_BLOCK, (NETUID,), 100u64)?;
        state.seed(&st::PENDING_EPOCH_AT, (NETUID,), 0u64)?;
        state.seed(&st::ADMIN_FREEZE_WINDOW, (), 10u16)?;
        state.seed_constant("Aura", "SlotDuration", json!(12_000))?;
        setup(&mut state)?;
        let chain = FakeChain::from_state(state);
        let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
        config.runtime_cache_dir = None;
        config.config_store = None;
        config.track_heads = false;
        let client = Client::from_rpc(RpcClient::new(chain.clone()), config).await?;
        let snapshot = client.at(BlockRef::Finalized).await?;
        Ok((chain, client, snapshot))
    }

    async fn preview<I: Intent>(
        intent: &I,
        snapshot: &Snapshot,
        client: &Client,
        dispatch_origin: AccountId,
        fee_payer: AccountId,
    ) -> Result<IntentPreflight, IntentError> {
        let built = intent
            .build(&BuildCx {
                view: snapshot,
                origin: OriginView::Account(dispatch_origin),
                runtime: &snapshot.block().runtime,
                clock: &SystemClock,
                spec: I::SPEC,
            })
            .await?;
        intent
            .preflight(&PreflightCx {
                view: snapshot,
                runtime: &snapshot.block().runtime,
                clock: &SystemClock,
                spec: I::SPEC,
                client,
                dispatch_origin,
                fee_payer,
                call: &built.call,
            })
            .await
    }

    #[tokio::test]
    async fn generated_governance_builds_preserve_values_and_utf8_bytes() -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture(|_| Ok(())).await?;
        let runtime = &snapshot.block().runtime;
        for (op, args, expected) in [
            (
                "trim_subnet",
                json!({"netuid":3,"max_n":64}),
                admin_utils::sudo_trim_to_max_allowed_uids(NETUID, 64),
            ),
            (
                "trim_null_subnet_batch",
                json!({"netuid":3,"target":0}),
                admin_utils::sudo_trim_null_uids_batch(NETUID, 0),
            ),
            (
                "set_mechanism_count",
                json!({"netuid":3,"mechanism_count":0}),
                admin_utils::sudo_set_mechanism_count(NETUID, MechId(0)),
            ),
            (
                "set_mechanism_count",
                json!({"netuid":3,"mechanism_count":255}),
                admin_utils::sudo_set_mechanism_count(NETUID, MechId(255)),
            ),
            (
                "update_symbol",
                json!({"netuid":3,"symbol":"α🦀\n'"}),
                subtensor_module::update_symbol(NETUID, "α🦀\n'".as_bytes().to_vec()),
            ),
        ] {
            let intent = crate::AnyIntent::from_tool(op, args)?;
            let context = BuildCx {
                view: &snapshot as &dyn rtao_client::DynView,
                origin: OriginView::Account(PAYER),
                runtime,
                clock: &SystemClock,
                spec: intent.spec(),
            };
            let built = intent.as_dyn().build_dyn(&context).await?;
            assert_eq!(
                built.call.as_bytes(),
                expected.compose(runtime)?.as_bytes(),
                "{op}"
            );
            assert!(built.extras.is_empty());
            assert_eq!(crate::AnyIntent::from_tool(op, intent.to_json())?, intent);
        }
        assert!(chain.state().lock().unwrap().submissions.is_empty());
        client.close().await;
        Ok(())
    }

    #[test]
    fn governance_summaries_and_base_policy_facts_match_python() {
        let trim = TrimSubnet {
            netuid: NETUID,
            max_n: 64,
        };
        let batch = TrimNullSubnetBatch {
            netuid: NETUID,
            target: 10,
        };
        let count = SetMechanismCount {
            netuid: NETUID,
            mechanism_count: MechId(2),
        };
        let symbol = UpdateSymbol {
            netuid: NETUID,
            symbol: "it's\nα".into(),
        };
        assert_eq!(trim.summary(), "trim netuid 3 to at most 64 UIDs");
        assert_eq!(
            batch.summary(),
            "prune one Null batch on netuid 3 toward 10 UIDs"
        );
        assert_eq!(count.summary(), "set mechanism count to 2 on netuid 3");
        assert_eq!(symbol.summary(), "set symbol for netuid 3 to \"it's\\nα\"");
        for intent in [&trim as &dyn rtao_tx::DynIntent, &batch, &count, &symbol] {
            assert_eq!(intent.spend_dyn(), Spend::None);
            assert_eq!(intent.touches_netuids_dyn().as_slice(), &[NETUID]);
            assert_eq!(intent.spec().origin, "subnet_owner");
        }
    }

    #[tokio::test]
    async fn owner_windows_use_dispatch_owner_and_keep_freeze_and_cooldown() -> anyhow::Result<()> {
        let (_, client, snapshot) = fixture(|state| {
            state.seed(&st::PENDING_EPOCH_AT, (NETUID,), 110u64)?;
            state.seed(
                &st::TRANSACTION_KEY_LAST_BLOCK,
                (OWNER, NETUID, 7u16),
                99u64,
            )?;
            state.seed(
                &st::TRANSACTION_KEY_LAST_BLOCK,
                (OWNER, NETUID, 9u16),
                99u64,
            )?;
            Ok(())
        })
        .await?;
        let count = SetMechanismCount {
            netuid: NETUID,
            mechanism_count: MechId(2),
        };
        let trim = TrimSubnet {
            netuid: NETUID,
            max_n: 64,
        };
        for (preview, expected_cooldown) in [
            (
                preview(&count, &snapshot, &client, OWNER, PAYER).await?,
                7299,
            ),
            (
                preview(&trim, &snapshot, &client, OWNER, PAYER).await?,
                216099,
            ),
        ] {
            assert_eq!(
                preview.blocks,
                [
                    "admin freeze until pending epoch at block 110; recheck epoch state".to_owned(),
                    format!("owner setter cooldown until block {expected_cooldown}"),
                ]
            );
            assert!(preview.warnings.is_empty());
        }
        let error = preview(&count, &snapshot, &client, PAYER, OWNER)
            .await
            .unwrap_err();
        assert!(
            matches!(error, IntentError::Chain(ref error) if error.name.as_deref() == Some("BadOrigin"))
        );
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn matching_null_continuation_skips_only_cooldown() -> anyhow::Result<()> {
        for frozen in [false, true] {
            let (_, client, snapshot) = fixture(|state| {
                state.seed_python(
                    "SubtensorModule",
                    "NullPruningTarget",
                    &[json!(NETUID.0)],
                    json!(10),
                )?;
                state.seed(
                    &st::TRANSACTION_KEY_LAST_BLOCK,
                    (OWNER, NETUID, 9u16),
                    100u64,
                )?;
                if frozen {
                    state.seed(&st::PENDING_EPOCH_AT, (NETUID,), 110u64)?;
                }
                Ok(())
            })
            .await?;
            let continuation = TrimNullSubnetBatch {
                netuid: NETUID,
                target: 10,
            };
            let changed = TrimNullSubnetBatch {
                netuid: NETUID,
                target: 11,
            };
            let continuing = preview(&continuation, &snapshot, &client, OWNER, PAYER).await?;
            let restarting = preview(&changed, &snapshot, &client, OWNER, PAYER).await?;
            assert_eq!(continuing.blocks.len(), usize::from(frozen));
            assert_eq!(restarting.blocks.len(), usize::from(frozen) + 1);
            assert_eq!(
                restarting.blocks.last().map(String::as_str),
                Some("owner setter cooldown until block 216100")
            );
            if frozen {
                assert_eq!(
                    continuing.blocks[0],
                    "admin freeze until pending epoch at block 110; recheck epoch state"
                );
            }
            client.close().await;
        }
        Ok(())
    }

    #[tokio::test]
    async fn open_owner_windows_keep_python_summary_as_the_only_effect() -> anyhow::Result<()> {
        let (_, client, snapshot) = fixture(|_| Ok(())).await?;
        let trim = TrimSubnet {
            netuid: NETUID,
            max_n: 64,
        };
        let plan = preview(&trim, &snapshot, &client, OWNER, PAYER).await?;
        assert_eq!(plan.effects, [trim.summary()]);
        assert!(plan.blocks.is_empty());
        assert!(plan.warnings.is_empty());
        assert!(plan.required_free.is_none());
        client.close().await;
        Ok(())
    }
}
