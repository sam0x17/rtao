// Derived from RaoFoundation/subtensor sdk/python/bittensor/intents/proxy.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Proxy delegation, keyless accounts and announced-call dispatch.

use rtao_client::{DynView, View};
use rtao_primitives::IntoCoreValue;
use rtao_runtime::{Call, calls::proxy};
use rtao_tx::{BuildCx, Built, Intent, IntentError, PreflightCx};
use serde_json::Value;

pub use crate::{
    AddProxy, CreatePureProxy, ExecuteProxyAnnounced, KillPureProxy, RemoveProxies, RemoveProxy,
};

/// Runtime proxy type names, in the Python choice and on-chain variant order.
#[doc(alias = "bittensor.intents.proxy.PROXY_TYPES")]
pub const PROXY_TYPES: [&str; 19] = [
    "Any",
    "Owner",
    "NonCritical",
    "NonTransfer",
    "Senate",
    "NonFungible",
    "Triumvirate",
    "Governance",
    "Staking",
    "Registration",
    "Transfer",
    "SmallTransfer",
    "RootWeights",
    "ChildKeys",
    "SudoUncheckedSetCode",
    "SwapHotkey",
    "SubnetLeaseBeneficiary",
    "RootClaim",
    "BasketTrading",
];

/// Validate an exact Python proxy choice without normalizing its spelling.
#[doc(alias = "bittensor.intents.proxy.check_proxy_type")]
pub fn check_proxy_type(proxy_type: &str) -> Result<&str, IntentError> {
    if PROXY_TYPES.contains(&proxy_type) {
        Ok(proxy_type)
    } else {
        Err(IntentError::validation(format!(
            "unknown proxy type {}; expected one of: {}",
            rtao_primitives::pyfmt::string_repr(proxy_type),
            PROXY_TYPES.join(", ")
        )))
    }
}

/// Bound nested arguments before generated serde conversion clones them.
pub(crate) fn check_constructor_input(args: &Value) -> Result<(), IntentError> {
    if let Some(inner_args) = args.get("inner_args") {
        crate::arguments::check_nested_json(inner_args)?;
    }
    Ok(())
}

impl Intent for AddProxy {
    fn summary(&self) -> String {
        format!(
            "add {} proxy {} (delay {})",
            self.proxy_type, self.delegate_ss58, self.delay
        )
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        self.validate()?;
        crate::generated::passthrough::add_proxy(self, cx).await
    }

    async fn warnings<V: View + ?Sized>(
        &self,
        _cx: &PreflightCx<'_, V>,
    ) -> Result<Vec<String>, IntentError> {
        let mut warnings =
            vec!["reserves a proxy deposit from the signer (returned on removal)".into()];
        if self.proxy_type == "Any" {
            warnings.push(
                "an Any proxy can do everything this account can, including transfers; prefer a narrower proxy type".into(),
            );
        }
        Ok(warnings)
    }

    fn validate(&self) -> Result<(), IntentError> {
        check_proxy_type(&self.proxy_type)?;
        Ok(())
    }
}

impl Intent for RemoveProxy {
    fn summary(&self) -> String {
        format!(
            "remove {} proxy {} (delay {})",
            self.proxy_type, self.delegate_ss58, self.delay
        )
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        self.validate()?;
        crate::generated::passthrough::remove_proxy(self, cx).await
    }

    fn validate(&self) -> Result<(), IntentError> {
        check_proxy_type(&self.proxy_type)?;
        Ok(())
    }
}

impl Intent for RemoveProxies {
    fn summary(&self) -> String {
        "remove ALL proxies for the signer".into()
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        crate::generated::passthrough::remove_proxies(self, cx).await
    }

    async fn warnings<V: View + ?Sized>(
        &self,
        _cx: &PreflightCx<'_, V>,
    ) -> Result<Vec<String>, IntentError> {
        Ok(vec![
            "revokes every delegate at once; pure-proxy accounts would become inaccessible".into(),
        ])
    }
}

impl Intent for CreatePureProxy {
    fn summary(&self) -> String {
        format!(
            "create pure {} proxy (index {}, delay {})",
            self.proxy_type, self.index, self.delay
        )
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        self.validate()?;
        crate::generated::passthrough::create_pure_proxy(self, cx).await
    }

    fn validate(&self) -> Result<(), IntentError> {
        check_proxy_type(&self.proxy_type)?;
        Ok(())
    }
}

impl Intent for KillPureProxy {
    fn summary(&self) -> String {
        format!(
            "kill pure proxy index {} for {}",
            self.index, self.spawner_ss58
        )
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        self.validate()?;
        crate::generated::passthrough::kill_pure_proxy(self, cx).await
    }

    fn validate(&self) -> Result<(), IntentError> {
        check_proxy_type(&self.proxy_type)?;
        Ok(())
    }
}

impl Intent for ExecuteProxyAnnounced {
    fn summary(&self) -> String {
        format!(
            "execute announced {} as {} via {}",
            self.inner_op, self.real_ss58, self.delegate_ss58
        )
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        self.validate()?;
        let inner = crate::AnyIntent::from_tool(&self.inner_op, self.inner_args.clone())?;
        let borrowed = cx.view;
        let inner_cx = BuildCx {
            view: &borrowed as &dyn DynView,
            origin: cx.origin,
            runtime: cx.runtime,
            clock: cx.clock,
            spec: inner.spec(),
        };
        // Python builds with the caller's original wallet, then extracts only
        // the inner call. The real address does not substitute a nested origin;
        // executor-only sudo/proxy adapters and inner extras do not enter it.
        let inner_call = inner.as_dyn().build_dyn(&inner_cx).await?.call;
        let nested_stamps = rtao_runtime::nested_call_stamps(&inner_call);
        // Keep this field dynamic until the checked core encoder sees it:
        // unlike the delegation constructors, Python has no __post_init__
        // proxy-type check here, and encoding follows the inner build.
        let call = Call::new(
            &proxy::PROXY_ANNOUNCED,
            rtao_core::codec::Value::record(vec![
                ("delegate".into(), (&self.delegate_ss58).into_core_value()),
                ("real".into(), (&self.real_ss58).into_core_value()),
                (
                    "force_proxy_type".into(),
                    self.force_proxy_type.clone().into_core_value(),
                ),
                ("call".into(), inner_call.into_core_value()),
            ]),
            nested_stamps,
        );
        Ok(Built::new(call.compose(cx.runtime)?))
    }

    fn validate(&self) -> Result<(), IntentError> {
        crate::arguments::check_nested_json(&self.inner_args)?;
        // Python deliberately does not call check_proxy_type for this intent.
        // The nested registry checks its argument shape during build; runtime
        // composition validates force_proxy_type after that nested build.
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use rtao_client::{BlockRef, Client, ClientConfig, Network, Snapshot};
    use rtao_primitives::{AccountId, Spend};
    use rtao_runtime::{CallBytes, RuntimeHandle};
    use rtao_testkit::{FakeChain, FakeState, RecordingView, rpc::RpcClient};
    use rtao_tx::{DynIntent, IntentInterface, OriginView, SystemClock};
    use serde_json::json;

    const DELEGATE: AccountId = AccountId([3; 32]);
    const REAL: AccountId = AccountId([4; 32]);
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

    async fn build(
        intent: &dyn DynIntent,
        view: &dyn DynView,
        runtime: &RuntimeHandle,
    ) -> Result<Built, IntentError> {
        intent
            .build_dyn(&BuildCx {
                view,
                origin: OriginView::Account(ORIGIN),
                runtime,
                clock: &SystemClock,
                spec: intent.spec(),
            })
            .await
    }

    fn announced(inner_op: &str, inner_args: Value) -> ExecuteProxyAnnounced {
        ExecuteProxyAnnounced {
            delegate_ss58: DELEGATE.into(),
            real_ss58: REAL.into(),
            inner_op: inner_op.into(),
            inner_args,
            force_proxy_type: None,
        }
    }

    #[test]
    fn proxy_choices_validate_exact_spelling_with_python_diagnostic() -> anyhow::Result<()> {
        assert_eq!(PROXY_TYPES.len(), 19);
        for name in PROXY_TYPES {
            assert_eq!(check_proxy_type(name)?, name);
        }
        for name in ["", "any", "STAKING", "Staking ", "Any'\n", "🦀"] {
            assert_eq!(
                check_proxy_type(name)
                    .expect_err("invalid choice")
                    .to_string(),
                format!(
                    "unknown proxy type {}; expected one of: Any, Owner, NonCritical, NonTransfer, Senate, NonFungible, Triumvirate, Governance, Staking, Registration, Transfer, SmallTransfer, RootWeights, ChildKeys, SudoUncheckedSetCode, SwapHotkey, SubnetLeaseBeneficiary, RootClaim, BasketTrading",
                    rtao_primitives::pyfmt::string_repr(name)
                )
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn proxy_builds_match_native_variant_and_integer_boundaries() -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture().await?;
        let runtime = &snapshot.block().runtime;
        let view = RecordingView::new(snapshot.clone());
        let requests = chain.requests().len();
        // The independent byte layout follows pallet-proxy's call indices and
        // common/src/proxy.rs's implicit SCALE variant indices, not the builders.
        for (variant, name) in (0u8..).zip(PROXY_TYPES) {
            let add = AddProxy {
                delegate_ss58: DELEGATE.into(),
                proxy_type: name.into(),
                delay: u32::MAX,
            };
            let remove = RemoveProxy {
                delegate_ss58: DELEGATE.into(),
                proxy_type: name.into(),
                delay: 0,
            };
            let create = CreatePureProxy {
                proxy_type: name.into(),
                delay: u32::MAX,
                index: u16::MAX,
            };
            let kill = KillPureProxy {
                spawner_ss58: REAL.into(),
                proxy_type: name.into(),
                index: u16::MAX,
                height: u32::MAX,
                ext_index: u32::MAX,
            };
            let mut add_bytes = vec![16, 1, 0];
            add_bytes.extend_from_slice(DELEGATE.as_bytes());
            add_bytes.push(variant);
            add_bytes.extend_from_slice(&[255; 4]);
            let mut remove_bytes = vec![16, 2, 0];
            remove_bytes.extend_from_slice(DELEGATE.as_bytes());
            remove_bytes.extend_from_slice(&[variant, 0, 0, 0, 0]);
            let create_bytes = vec![16, 4, variant, 255, 255, 255, 255, 255, 255];
            let mut kill_bytes = vec![16, 5, 0];
            kill_bytes.extend_from_slice(REAL.as_bytes());
            kill_bytes.extend_from_slice(&[
                variant, 255, 255, 3, 255, 255, 255, 255, 3, 255, 255, 255, 255,
            ]);
            for (intent, expected, call, fields) in [
                (
                    &add as &dyn DynIntent,
                    add_bytes,
                    "add_proxy",
                    vec![
                        ("delegate", json!(DELEGATE.to_string())),
                        ("proxy_type", json!(name)),
                        ("delay", json!(u32::MAX)),
                    ],
                ),
                (
                    &remove as &dyn DynIntent,
                    remove_bytes,
                    "remove_proxy",
                    vec![
                        ("delegate", json!(DELEGATE.to_string())),
                        ("proxy_type", json!(name)),
                        ("delay", json!(0)),
                    ],
                ),
                (
                    &create as &dyn DynIntent,
                    create_bytes,
                    "create_pure",
                    vec![
                        ("proxy_type", json!(name)),
                        ("delay", json!(u32::MAX)),
                        ("index", json!(u16::MAX)),
                    ],
                ),
                (
                    &kill as &dyn DynIntent,
                    kill_bytes,
                    "kill_pure",
                    vec![
                        ("spawner", json!(REAL.to_string())),
                        ("proxy_type", json!(name)),
                        ("index", json!(u16::MAX)),
                        ("height", json!(u32::MAX)),
                        ("ext_index", json!(u32::MAX)),
                    ],
                ),
            ] {
                let built = build(intent, &view, runtime).await?;
                assert_eq!(
                    built.call.as_bytes(),
                    expected,
                    "{} {name}",
                    intent.spec().name
                );
                assert_eq!(CallBytes::decode(&expected, runtime)?, built.call);
                assert!(built.extras.is_empty());
                let decoded = decode(&built.call, runtime)?;
                assert_eq!(decoded["call_module"], "Proxy");
                assert_eq!(decoded["call_function"], call);
                assert_eq!(
                    decoded["call_args"].as_array().map(Vec::len),
                    Some(fields.len())
                );
                for (index, (field, value)) in fields.into_iter().enumerate() {
                    assert_eq!(decoded["call_args"][index]["name"], field);
                    assert_eq!(decoded["call_args"][index]["value"], value);
                }
            }
        }
        let built = build(&RemoveProxies {}, &view, runtime).await?;
        assert_eq!(built.call.as_bytes(), &[16, 3]);
        assert!(built.extras.is_empty());
        assert_eq!(decode(&built.call, runtime)?["call_args"], json!([]));
        for prefix in [0, 2, 42, 110] {
            let spelling = DELEGATE.to_ss58(prefix);
            let intent = AddProxy {
                delegate_ss58: spelling.parse()?,
                proxy_type: "Staking".into(),
                delay: 0,
            };
            assert_eq!(
                intent.summary(),
                format!("add Staking proxy {spelling} (delay 0)")
            );
            assert_eq!(intent.to_json()["delegate_ss58"], spelling);
            let built = build(&intent, &view, runtime).await?;
            let mut expected = vec![16, 1, 0];
            expected.extend_from_slice(DELEGATE.as_bytes());
            expected.extend_from_slice(&[8, 0, 0, 0, 0]);
            assert_eq!(built.call.as_bytes(), expected);
        }
        assert!(
            crate::AnyIntent::from_tool(
                "add_proxy",
                json!({"delegate_ss58":DELEGATE.to_ss58(1000)})
            )
            .is_err()
        );
        assert!(view.touches().is_empty());
        assert_eq!(chain.requests().len(), requests);
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn proxy_preflight_defaults_roundtrip_and_exact_warnings() -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture().await?;
        let runtime = &snapshot.block().runtime;
        let view = RecordingView::new(snapshot.clone());
        let requests = chain.requests().len();
        for (op, args, summary, warnings) in [
            (
                "add_proxy",
                json!({"delegate_ss58":DELEGATE.to_string()}),
                format!("add Staking proxy {DELEGATE} (delay 0)"),
                vec!["reserves a proxy deposit from the signer (returned on removal)"],
            ),
            (
                "add_proxy",
                json!({"delegate_ss58":DELEGATE.to_string(),"proxy_type":"Any"}),
                format!("add Any proxy {DELEGATE} (delay 0)"),
                vec![
                    "reserves a proxy deposit from the signer (returned on removal)",
                    "an Any proxy can do everything this account can, including transfers; prefer a narrower proxy type",
                ],
            ),
            (
                "remove_proxy",
                json!({"delegate_ss58":DELEGATE.to_string()}),
                format!("remove Staking proxy {DELEGATE} (delay 0)"),
                vec![],
            ),
            (
                "remove_proxies",
                json!({}),
                "remove ALL proxies for the signer".into(),
                vec![
                    "revokes every delegate at once; pure-proxy accounts would become inaccessible",
                ],
            ),
            (
                "create_pure_proxy",
                json!({}),
                "create pure Staking proxy (index 0, delay 0)".into(),
                vec![],
            ),
            (
                "kill_pure_proxy",
                json!({"spawner_ss58":REAL.to_string()}),
                format!("kill pure proxy index 0 for {REAL}"),
                vec![],
            ),
            (
                "execute_proxy_announced",
                json!({
                    "delegate_ss58":DELEGATE.to_string(), "real_ss58":REAL.to_string(),
                    "inner_op":"remove_proxies", "inner_args":{}
                }),
                format!("execute announced remove_proxies as {REAL} via {DELEGATE}"),
                vec![],
            ),
        ] {
            let intent = crate::AnyIntent::from_tool(op, args)?;
            let dynamic = intent.as_dyn();
            let built = build(dynamic, &view, runtime).await?;
            let preview = dynamic
                .preflight_dyn(&PreflightCx {
                    view: &view as &dyn DynView,
                    runtime,
                    clock: &SystemClock,
                    spec: dynamic.spec(),
                    client: &client,
                    dispatch_origin: REAL,
                    fee_payer: ORIGIN,
                    call: &built.call,
                })
                .await?;
            assert_eq!(dynamic.summary_dyn(), summary);
            assert_eq!(preview.effects, vec![summary]);
            assert_eq!(preview.warnings, warnings);
            assert!(preview.blocks.is_empty());
            assert!(preview.required_free.is_none());
            assert!(preview.available_free.is_none());
            assert!(preview.estimated_fee.is_none());
            assert!(preview.facts.is_empty());
            assert_eq!(dynamic.spend_dyn(), Spend::None);
            assert!(dynamic.touches_netuids_dyn().is_empty());
            assert!(!dynamic.affects_all_subnets_dyn());
            assert_eq!(dynamic.spec().signer, "coldkey");
            assert_eq!(dynamic.spec().origin, "signed");
            assert!(dynamic.spec().verify.is_none());
            assert!(dynamic.spec().all_amount_fields.is_empty());
            assert_eq!(crate::AnyIntent::from_tool(op, intent.to_json())?, intent);
            for field in ["delay", "index", "height", "ext_index"] {
                if let Some(value) = intent.to_json().get(field) {
                    assert_eq!(*value, json!(0));
                }
            }
            if op == "execute_proxy_announced" {
                assert_eq!(intent.to_json()["force_proxy_type"], Value::Null);
            }
        }
        assert!(view.touches().is_empty());
        assert_eq!(chain.requests().len(), requests);
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn announced_proxy_nests_exact_calls_and_optional_type_without_side_effects()
    -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture().await?;
        let runtime = &snapshot.block().runtime;
        let view = RecordingView::new(snapshot.clone());
        let requests = chain.requests().len();
        for name in [None, Some("Any"), Some("BasketTrading")] {
            let mut intent = announced("remove_proxies", json!({"op":"ignored"}));
            intent.force_proxy_type = name.map(str::to_owned);
            let built = build(&intent, &view, runtime).await?;
            let mut expected = vec![16, 9, 0];
            expected.extend_from_slice(DELEGATE.as_bytes());
            expected.push(0);
            expected.extend_from_slice(REAL.as_bytes());
            expected.extend_from_slice(match name {
                None => &[0][..],
                Some("Any") => &[1, 0],
                _ => &[1, 18],
            });
            expected.extend_from_slice(&[16, 3]);
            assert_eq!(built.call.as_bytes(), expected);
            assert_eq!(CallBytes::decode(&expected, runtime)?, built.call);
            assert!(built.extras.is_empty());
            let decoded = decode(&built.call, runtime)?;
            assert_eq!(decoded["call_module"], "Proxy");
            assert_eq!(decoded["call_function"], "proxy_announced");
            assert_eq!(decoded["call_args"][0]["value"], DELEGATE.to_string());
            assert_eq!(decoded["call_args"][1]["value"], REAL.to_string());
            assert_eq!(decoded["call_args"][2]["value"], json!(name));
            assert_eq!(decoded["call_args"][3]["value"]["call_module"], "Proxy");
            assert_eq!(
                decoded["call_args"][3]["value"]["call_function"],
                "remove_proxies"
            );
        }
        // Recursive dispatch stays in semantic build, without a second executor
        // proxy wrapper or substituting the outer real address as nested input.
        let nested = announced("remove_proxies", json!({}));
        let outer = announced("execute_proxy_announced", nested.to_json());
        let inner_built = build(&nested, &view, runtime).await?;
        let outer_built = build(&outer, &view, runtime).await?;
        assert_eq!(
            &outer_built.call.as_bytes()[69..],
            inner_built.call.as_bytes()
        );
        assert!(view.touches().is_empty());
        assert_eq!(chain.requests().len(), requests);
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        client.close().await;
        Ok(())
    }

    #[test]
    fn proxy_tool_inputs_reject_invalid_choices_addresses_and_integer_widths() -> anyhow::Result<()>
    {
        for (op, mut args) in [
            ("add_proxy", json!({"delegate_ss58":DELEGATE.to_string()})),
            (
                "remove_proxy",
                json!({"delegate_ss58":DELEGATE.to_string()}),
            ),
            ("create_pure_proxy", json!({})),
            ("kill_pure_proxy", json!({"spawner_ss58":REAL.to_string()})),
        ] {
            for spelling in ["", "any", "Staking ", "Unknown"] {
                args["proxy_type"] = json!(spelling);
                let error = crate::AnyIntent::from_tool(op, args.clone())
                    .expect_err("invalid proxy choice");
                assert_eq!(
                    error.to_string(),
                    check_proxy_type(spelling).expect_err("invalid").to_string()
                );
            }
            args["proxy_type"] = json!("Staking");
            assert!(crate::AnyIntent::from_tool(op, args.clone()).is_ok());
            for field in match op {
                "add_proxy" | "remove_proxy" => &["delay"][..],
                "create_pure_proxy" => &["delay", "index"],
                _ => &["index", "height", "ext_index"],
            } {
                let maximum = if *field == "index" {
                    u64::from(u16::MAX)
                } else {
                    u64::from(u32::MAX)
                };
                for invalid in [
                    json!(-1),
                    json!(maximum + 1),
                    json!(true),
                    json!(1.5),
                    Value::Null,
                ] {
                    let mut invalid_args = args.clone();
                    invalid_args[*field] = invalid;
                    assert!(
                        crate::AnyIntent::from_tool(op, invalid_args).is_err(),
                        "{op}.{field}"
                    );
                }
            }
            let mut extra = args;
            extra["unexpected"] = json!(true);
            assert!(crate::AnyIntent::from_tool(op, extra).is_err());
        }
        for op in ["add_proxy", "remove_proxy", "kill_pure_proxy"] {
            assert!(crate::AnyIntent::from_tool(op, json!({})).is_err());
            let field = if op == "kill_pure_proxy" {
                "spawner_ss58"
            } else {
                "delegate_ss58"
            };
            assert!(crate::AnyIntent::from_tool(op, json!({field:"bad ss58"})).is_err());
        }
        assert!(crate::AnyIntent::from_tool("remove_proxies", json!({"extra":true})).is_err());
        Ok(())
    }

    #[tokio::test]
    async fn announced_proxy_refusals_preserve_nested_error_order_and_never_read()
    -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture().await?;
        let runtime = &snapshot.block().runtime;
        let view = RecordingView::new(snapshot.clone());
        let requests = chain.requests().len();
        let mut intent = announced("missing-intent", json!({}));
        intent.force_proxy_type = Some("InvalidType".into());
        // Unlike AddProxy, Python accepts this constructor field and composes
        // the inner intent before checking the optional force type.
        intent.validate()?;
        assert!(crate::AnyIntent::from_tool("execute_proxy_announced", intent.to_json()).is_ok());
        assert_eq!(
            build(&intent, &view, runtime)
                .await
                .expect_err("unknown inner")
                .to_string(),
            "Unknown or disabled intent: missing-intent"
        );
        intent.inner_op = "remove_proxies".into();
        assert!(
            build(&intent, &view, runtime)
                .await
                .expect_err("invalid force type")
                .to_string()
                .contains("InvalidType")
        );
        for malformed in [Value::Null, json!([]), json!("{}"), json!(1), json!(false)] {
            intent.inner_args = malformed;
            intent.validate()?;
            assert_eq!(
                build(&intent, &view, runtime)
                    .await
                    .expect_err("object required")
                    .to_string(),
                "Intent arguments must be a JSON object."
            );
            intent.inner_op = "missing-intent".into();
            assert_eq!(
                build(&intent, &view, runtime)
                    .await
                    .expect_err("unknown inner first")
                    .to_string(),
                "Unknown or disabled intent: missing-intent"
            );
            intent.inner_op = "remove_proxies".into();
        }
        for key in [
            "$serde_json::private::RawValue",
            "$serde_json::private::Number",
        ] {
            intent.inner_args = json!({key:"{}"});
            assert!(intent.validate().is_err());
            assert!(check_constructor_input(&json!({"inner_args":intent.inner_args})).is_err());
            assert!(
                crate::AnyIntent::from_tool("execute_proxy_announced", intent.to_json()).is_err()
            );
        }
        let mut deep = Value::Null;
        for _ in 0..130 {
            deep = json!({"nested":deep});
        }
        intent.inner_args = deep;
        assert!(intent.validate().is_err());
        assert!(crate::AnyIntent::from_tool("execute_proxy_announced", intent.to_json()).is_err());
        assert!(view.touches().is_empty());
        assert_eq!(chain.requests().len(), requests);
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        client.close().await;
        Ok(())
    }

    #[cfg(feature = "registration")]
    #[tokio::test]
    async fn announced_proxy_inner_defaults_keep_callers_wallet_not_real_origin()
    -> anyhow::Result<()> {
        use rtao_signer::{KeyView, SignerError};

        struct AttachedKeys;
        impl KeyView for AttachedKeys {
            fn hotkey_address(&self) -> Result<AccountId, SignerError> {
                Ok(AccountId([9; 32]))
            }
            fn hotkey_public_key(&self) -> Result<[u8; 32], SignerError> {
                Ok([9; 32])
            }
            fn coldkey_address(&self) -> Result<AccountId, SignerError> {
                Ok(ORIGIN)
            }
        }

        let (chain, client, snapshot) = fixture().await?;
        let runtime = &snapshot.block().runtime;
        let view = RecordingView::new(snapshot.clone());
        let requests = chain.requests().len();
        let intent = announced("root_register", json!({}));
        let built = intent
            .build(&BuildCx {
                view: &view,
                origin: OriginView::Keys(&AttachedKeys),
                runtime,
                clock: &SystemClock,
                spec: ExecuteProxyAnnounced::SPEC,
            })
            .await?;
        let decoded = decode(&built.call, runtime)?;
        let nested = &decoded["call_args"][3]["value"];
        assert_eq!(nested["call_module"], "SubtensorModule");
        assert_eq!(nested["call_function"], "root_register");
        assert_eq!(
            nested["call_args"][0]["value"],
            AccountId([9; 32]).to_string()
        );
        assert!(built.extras.is_empty());
        assert!(view.touches().is_empty());
        assert_eq!(chain.requests().len(), requests);
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        client.close().await;
        Ok(())
    }

    #[cfg(feature = "root")]
    #[tokio::test]
    async fn announced_root_inner_remains_unwrapped_semantic_call() -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture().await?;
        let runtime = &snapshot.block().runtime;
        let view = RecordingView::new(snapshot.clone());
        let requests = chain.requests().len();
        let intent = announced(
            "set_subnet_emission_enabled",
            json!({"netuids":[3],"enabled":true}),
        );
        assert_eq!(crate::SetSubnetEmissionEnabled::SPEC.origin, "root");
        let built = build(&intent, &view, runtime).await?;
        let decoded = decode(&built.call, runtime)?;
        let nested = &decoded["call_args"][3]["value"];
        assert_eq!(nested["call_module"], "AdminUtils");
        assert_eq!(nested["call_function"], "sudo_set_subnet_emission_enabled");
        assert_eq!(nested["call_args"][0]["value"], json!(3));
        assert_eq!(nested["call_args"][1]["value"], json!(true));
        assert!(intent.touches_netuids().is_empty());
        assert!(built.extras.is_empty());
        assert!(view.touches().is_empty());
        assert_eq!(chain.requests().len(), requests);
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        client.close().await;
        Ok(())
    }
}
