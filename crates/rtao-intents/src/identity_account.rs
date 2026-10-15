// Derived from RaoFoundation/subtensor sdk/python/bittensor/intents/identity.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Public coldkey identity fields, preserved as UTF-8 bytes.

use rtao_client::View;
use rtao_tx::{BuildCx, Built, Intent, IntentError};

pub use crate::SetIdentity;

impl Intent for SetIdentity {
    fn summary(&self) -> String {
        format!(
            "set on-chain identity to {}",
            rtao_primitives::pyfmt::string_repr(&self.name)
        )
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        crate::generated::passthrough::set_identity(self, cx).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rtao_client::{BlockRef, Client, ClientConfig, Network, Snapshot};
    use rtao_primitives::{AccountId, Spend};
    use rtao_runtime::{CallBytes, RuntimeHandle};
    use rtao_testkit::{FakeChain, FakeState, RecordingView, rpc::RpcClient};
    use rtao_tx::{IntentInterface, OriginView, PreflightCx, SystemClock};
    use serde_json::json;

    const COLDKEY: AccountId = AccountId([7; 32]);

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

    #[test]
    fn set_identity_defaults_clear_all_optional_fields_and_keep_python_repr() -> anyhow::Result<()>
    {
        let intent: SetIdentity = serde_json::from_value(json!({"name":"it's\nα"}))?;
        assert_eq!(intent.summary(), "set on-chain identity to \"it's\\nα\"");
        for field in [
            "url",
            "github_repo",
            "image",
            "discord",
            "description",
            "additional",
        ] {
            assert_eq!(intent.to_json()[field], "", "{field}");
        }
        for (name, summary) in [
            ("", "set on-chain identity to ''"),
            ("a\0b", "set on-chain identity to 'a\\x00b'"),
            ("'\"", "set on-chain identity to '\\'\"'"),
            ("α🦀", "set on-chain identity to 'α🦀'"),
        ] {
            let intent = crate::AnyIntent::from_tool("set_identity", json!({"name":name}))?;
            assert_eq!(intent.as_dyn().summary_dyn(), summary);
            assert_eq!(intent.to_json()["name"], name);
            assert_eq!(
                crate::AnyIntent::from_tool("set_identity", intent.to_json())?.to_json(),
                intent.to_json()
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn set_identity_encodes_independent_utf8_and_empty_field_scale_vectors()
    -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture().await?;
        let runtime = &snapshot.block().runtime;
        let recorded = RecordingView::new(snapshot.clone());
        let requests = chain.requests().len();
        let complete = SetIdentity {
            name: "α🦀".into(),
            url: "é".into(),
            github_repo: "g".into(),
            image: "".into(),
            discord: "d\0".into(),
            description: "a\nb".into(),
            additional: "'\"".into(),
        };
        let empty: SetIdentity = serde_json::from_value(json!({"name":""}))?;
        for (intent, expected) in [
            (
                complete,
                vec![
                    7, 68, 24, 0xce, 0xb1, 0xf0, 0x9f, 0xa6, 0x80, 8, 0xc3, 0xa9, 4, b'g', 0, 8,
                    b'd', 0, 12, b'a', 10, b'b', 8, b'\'', b'"',
                ],
            ),
            (empty, vec![7, 68, 0, 0, 0, 0, 0, 0, 0]),
        ] {
            let built = intent
                .build(&BuildCx {
                    view: &recorded,
                    origin: OriginView::Account(COLDKEY),
                    runtime,
                    clock: &SystemClock,
                    spec: SetIdentity::SPEC,
                })
                .await?;
            assert_eq!(built.call.as_bytes(), expected);
            assert_eq!(CallBytes::decode(&expected, runtime)?, built.call);
            assert!(built.extras.is_empty());
            let decoded = decode(&built.call, runtime)?;
            assert_eq!(decoded["call_module"], "SubtensorModule");
            assert_eq!(decoded["call_function"], "set_identity");
            for (index, (name, value)) in [
                ("name", &intent.name),
                ("url", &intent.url),
                ("github_repo", &intent.github_repo),
                ("image", &intent.image),
                ("discord", &intent.discord),
                ("description", &intent.description),
                ("additional", &intent.additional),
            ]
            .into_iter()
            .enumerate()
            {
                assert_eq!(decoded["call_args"][index]["name"], name);
                assert_eq!(decoded["call_args"][index]["value"], *value);
            }
        }
        assert!(recorded.touches().is_empty());
        assert_eq!(chain.requests().len(), requests);
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn set_identity_leaves_length_limits_to_chain_and_inherits_pure_preflight()
    -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture().await?;
        let runtime = &snapshot.block().runtime;
        let requests = chain.requests().len();
        let intent: SetIdentity = serde_json::from_value(json!({"name":"x".repeat(65)}))?;
        intent.validate()?;
        let built = intent
            .build(&BuildCx {
                view: &snapshot,
                origin: OriginView::Account(COLDKEY),
                runtime,
                clock: &SystemClock,
                spec: SetIdentity::SPEC,
            })
            .await?;
        let mut expected = vec![7, 68, 5, 1]; // Compact(65) = 0x0105.
        expected.extend_from_slice(&[b'x'; 65]);
        expected.extend_from_slice(&[0; 6]);
        assert_eq!(built.call.as_bytes(), expected);
        assert_eq!(CallBytes::decode(&expected, runtime)?, built.call);
        let preview = intent
            .preflight(&PreflightCx {
                view: &snapshot,
                runtime,
                clock: &SystemClock,
                spec: SetIdentity::SPEC,
                client: &client,
                dispatch_origin: COLDKEY,
                fee_payer: AccountId::ZERO,
                call: &built.call,
            })
            .await?;
        assert_eq!(preview.effects, vec![intent.summary()]);
        assert!(preview.warnings.is_empty());
        assert!(preview.blocks.is_empty());
        assert!(preview.facts.is_empty());
        assert!(preview.required_free.is_none());
        assert!(preview.available_free.is_none());
        assert!(preview.estimated_fee.is_none());
        assert_eq!(intent.spend(), Spend::None);
        assert!(intent.touches_netuids().is_empty());
        assert!(!intent.affects_all_subnets());
        assert_eq!(SetIdentity::SPEC.signer, "coldkey");
        assert_eq!(SetIdentity::SPEC.origin, "signed");
        assert!(SetIdentity::SPEC.verify.is_none());
        assert!(SetIdentity::SPEC.all_amount_fields.is_empty());
        assert_eq!(chain.requests().len(), requests);
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        client.close().await;
        Ok(())
    }

    #[test]
    fn set_identity_tool_inputs_require_string_fields_and_reject_unknown_arguments() {
        for input in [
            json!({}),
            json!({"name":null}),
            json!({"name":7}),
            json!({"name":"n","unexpected":true}),
            json!([]),
        ] {
            assert!(crate::AnyIntent::from_tool("set_identity", input).is_err());
        }
        for field in [
            "url",
            "github_repo",
            "image",
            "discord",
            "description",
            "additional",
        ] {
            for value in [json!(null), json!([1]), json!(false)] {
                let mut input = json!({"name":"n"});
                input[field] = value;
                assert!(
                    crate::AnyIntent::from_tool("set_identity", input).is_err(),
                    "{field}"
                );
            }
        }
    }
}
