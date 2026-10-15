// Derived from RaoFoundation/subtensor sdk/python/bittensor/intents/identity.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Public subnet identity metadata, encoded as UTF-8 bytes.

use rtao_client::View;
use rtao_tx::{BuildCx, Built, Intent, IntentError};

pub use crate::SetSubnetIdentity;

impl Intent for SetSubnetIdentity {
    fn summary(&self) -> String {
        format!(
            "set subnet {} identity to {}",
            self.netuid,
            rtao_primitives::pyfmt::string_repr(&self.subnet_name)
        )
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        crate::generated::passthrough::set_subnet_identity(self, cx).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rtao_client::{BlockRef, Client, ClientConfig, Network};
    use rtao_primitives::{AccountId, NetUid, Spend};
    use rtao_runtime::calls::subtensor_module as calls;
    use rtao_testkit::{FakeChain, FakeState, rpc::RpcClient};
    use rtao_tx::{IntentInterface, OriginView, PreflightCx, SystemClock};
    use serde_json::json;

    #[test]
    fn identity_defaults_json_and_repr_preserve_python_values() -> anyhow::Result<()> {
        let intent: SetSubnetIdentity = serde_json::from_value(json!({
            "netuid":3,
            "subnet_name":"it's\nα"
        }))?;
        assert_eq!(intent.summary(), "set subnet 3 identity to \"it's\\nα\"");
        for field in [
            "github_repo",
            "subnet_contact",
            "subnet_url",
            "discord",
            "description",
            "logo_url",
            "additional",
        ] {
            assert_eq!(intent.to_json()[field], "", "{field}");
        }
        assert_eq!(intent.spend(), Spend::None);
        assert_eq!(intent.touches_netuids().as_slice(), &[NetUid(3)]);
        assert_eq!(SetSubnetIdentity::SPEC.origin, "subnet_owner");
        assert_eq!(SetSubnetIdentity::SPEC.signer, "coldkey");
        assert_eq!(
            crate::AnyIntent::from_tool("set_subnet_identity", intent.to_json())?.to_json(),
            intent.to_json()
        );
        Ok(())
    }

    #[tokio::test]
    async fn generated_identity_build_encodes_each_field_as_utf8_in_order() -> anyhow::Result<()> {
        let chain = FakeChain::from_state(FakeState::try_default()?);
        let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
        config.track_heads = false;
        config.runtime_cache_dir = None;
        config.config_store = None;
        let client = Client::from_rpc(RpcClient::new(chain), config).await?;
        let snapshot = client.at(BlockRef::Finalized).await?;
        let runtime = &snapshot.block().runtime;
        let owner = AccountId([7; 32]);
        for fields in [
            [
                "α🦀",
                "répo",
                "連絡",
                "https://例.test",
                "discord\0",
                "a\nb",
                "🖼",
                "'\"",
            ],
            ["clear", "", "", "", "", "", "", ""],
        ] {
            let intent = SetSubnetIdentity {
                netuid: NetUid(3),
                subnet_name: fields[0].into(),
                github_repo: fields[1].into(),
                subnet_contact: fields[2].into(),
                subnet_url: fields[3].into(),
                discord: fields[4].into(),
                description: fields[5].into(),
                logo_url: fields[6].into(),
                additional: fields[7].into(),
            };
            let built = intent
                .build(&BuildCx {
                    view: &snapshot,
                    origin: OriginView::Account(owner),
                    runtime,
                    clock: &SystemClock,
                    spec: SetSubnetIdentity::SPEC,
                })
                .await?;
            let [
                name,
                github,
                contact,
                url,
                discord,
                description,
                logo,
                additional,
            ] = fields.map(|field| field.as_bytes().to_vec());
            let expected = calls::set_subnet_identity(
                NetUid(3),
                name,
                github,
                contact,
                url,
                discord,
                description,
                logo,
                additional,
            )
            .compose(runtime)?;
            assert_eq!(built.call.as_bytes(), expected.as_bytes());
            assert!(built.extras.is_empty());
            let preview = intent
                .preflight(&PreflightCx {
                    view: &snapshot,
                    runtime,
                    clock: &SystemClock,
                    spec: SetSubnetIdentity::SPEC,
                    client: &client,
                    dispatch_origin: owner,
                    fee_payer: owner,
                    call: &built.call,
                })
                .await?;
            assert_eq!(preview.effects, [intent.summary()]);
            assert!(preview.warnings.is_empty());
            assert!(preview.blocks.is_empty());
        }
        client.close().await;
        Ok(())
    }
}
