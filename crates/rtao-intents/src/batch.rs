// Derived from RaoFoundation/subtensor sdk/python/bittensor/intents/batch.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Atomic semantic batches with child normalization and aggregated policy.

use std::collections::BTreeSet;

use rtao_client::{DynView, View};
use rtao_primitives::{Balance, NetUid, Spend, Tao, pyfmt};
use rtao_runtime::calls::utility;
use rtao_signer::SignerRole;
use rtao_tx::{BuildCx, Built, DynIntent, Intent, IntentError, IntentPreflight, PreflightCx};
use serde_json::Value;
use smallvec::SmallVec;

pub use crate::Batch;

/// Bound the complete child array before generated serde conversion clones it.
pub(crate) fn check_constructor_input(args: &Value) -> Result<(), IntentError> {
    if let Some(intents) = args.get("intents") {
        crate::arguments::check_nested_json(intents)?;
    }
    Ok(())
}

/// Apply Python's constructor normalization to the stored child dictionaries.
pub(crate) fn canonicalize(batch: &mut Batch) -> Result<(), IntentError> {
    let normalized = children(batch)?
        .into_iter()
        .map(|child| child.to_json())
        .collect::<Vec<_>>();
    crate::arguments::check_nested_items(&normalized)?;
    batch.intents = normalized;
    Ok(())
}

/// Normalize typed child inputs when exposing the generated JSON interface.
pub(crate) fn to_json(batch: &Batch) -> Value {
    // Typed callers can initialize public fields without calling new(). Their
    // invalid input has no Python constructor result; keep this infallible
    // method bounded, and let validate() retain the actual refusal diagnostic.
    let items = children(batch)
        .map(|children| children.into_iter().map(|child| child.to_json()).collect())
        .unwrap_or_default();
    let mut args = serde_json::Map::new();
    args.insert("op".into(), Value::String("batch".into()));
    args.insert("intents".into(), Value::Array(items));
    Value::Object(args)
}

fn children(batch: &Batch) -> Result<Vec<crate::AnyIntent>, IntentError> {
    crate::arguments::check_nested_items(&batch.intents)?;
    if batch.intents.is_empty() {
        return Err(IntentError::validation(
            "batch requires at least one intent",
        ));
    }
    let mut children = Vec::with_capacity(batch.intents.len());
    let mut signers = BTreeSet::new();
    for item in &batch.intents {
        let Some(fields) = item.as_object() else {
            let class = match item {
                Value::Null => "NoneType",
                Value::Bool(_) => "bool",
                Value::String(_) => "str",
                Value::Array(_) => "list",
                Value::Number(number) => {
                    if number.to_string().contains(['.', 'e', 'E']) {
                        "float"
                    } else {
                        "int"
                    }
                }
                Value::Object(_) => "dict",
            };
            return Err(IntentError::validation(format!(
                "batched intent must be a dict or Intent, got <class '{class}'>"
            )));
        };
        if fields.get("op").and_then(Value::as_str) == Some("batch") {
            return Err(IntentError::validation("nested batches are not supported"));
        }
        let op = fields
            .get("op")
            .filter(|value| truthy(value))
            .ok_or_else(|| {
                IntentError::validation(format!(
                    "batched intent needs an 'op' key: {}",
                    repr(item, &mut 128, 0)
                ))
            })?;
        let op = op.as_str().ok_or_else(|| {
            IntentError::validation("batched intent 'op' must name a registered intent")
        })?;
        // The aggregate guard runs before this recursive Value clone, and the
        // registry supplies each child's own constructor validation/defaults.
        let child = crate::AnyIntent::from_tool(op, item.clone())?;
        signers.insert(child.as_dyn().signer_role_dyn()?.as_str());
        children.push(child);
    }
    if signers.len() > 1 {
        return Err(IntentError::validation(
            "all batched intents must share one signer, got ['coldkey', 'hotkey']; split into separate batches",
        ));
    }
    Ok(children)
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::String(value) => !value.is_empty(),
        Value::Number(value) => value.as_f64() != Some(0.0),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
    }
}

// Refusal output is bounded independently of the accepted input tree. Normal
// short dictionaries retain Python punctuation, quoting and scalar spelling.
fn repr(value: &Value, remaining: &mut usize, depth: usize) -> String {
    if *remaining == 0 || depth == 8 {
        return "...".into();
    }
    *remaining -= 1;
    match value {
        Value::Null => "None".into(),
        Value::Bool(value) => if *value { "True" } else { "False" }.into(),
        Value::String(value) if value.len() <= 2048 => pyfmt::string_repr(value),
        Value::String(_) => "'...'".into(),
        Value::Number(value) => {
            let text = value.to_string();
            if text.contains(['.', 'e', 'E']) {
                value.as_f64().map_or(text, pyfmt::float_repr)
            } else {
                text
            }
        }
        Value::Array(values) => {
            let mut out = Vec::new();
            for value in values {
                out.push(repr(value, remaining, depth + 1));
                if *remaining == 0 {
                    break;
                }
            }
            format!("[{}]", out.join(", "))
        }
        Value::Object(fields) => {
            let mut out = Vec::new();
            for (key, value) in fields {
                let key = if key.len() <= 2048 {
                    pyfmt::string_repr(key)
                } else {
                    "'...'".into()
                };
                out.push(format!("{key}: {}", repr(value, remaining, depth + 1)));
                if *remaining == 0 {
                    break;
                }
            }
            format!("{{{}}}", out.join(", "))
        }
    }
}

impl Batch {
    /// Construct a nonempty, single-signer batch and normalize every child.
    pub fn new(intents: Vec<Value>) -> Result<Self, IntentError> {
        let mut batch = Self { intents };
        canonicalize(&mut batch)?;
        Ok(batch)
    }

    /// Resolve the signing role shared by every child, including hotkey batches.
    pub fn signer_role(&self) -> Result<SignerRole, IntentError> {
        children(self)?
            .first()
            .ok_or_else(|| IntentError::validation("batch requires at least one intent"))?
            .as_dyn()
            .signer_role_dyn()
    }
}

fn refs(children: &[crate::AnyIntent]) -> Vec<&dyn DynIntent> {
    children.iter().map(crate::AnyIntent::as_dyn).collect()
}

async fn build_children<V: View + ?Sized>(
    children: &[&dyn DynIntent],
    cx: &BuildCx<'_, V>,
) -> Result<Built, IntentError> {
    let mut calls = Vec::with_capacity(children.len());
    let mut extras = serde_json::Map::new();
    let borrowed = cx.view;
    for (index, child) in children.iter().enumerate() {
        let child_cx = BuildCx {
            view: &borrowed as &dyn DynView,
            origin: cx.origin,
            runtime: cx.runtime,
            clock: cx.clock,
            spec: child.spec(),
        };
        // Semantic build only: Python does not run each child's executor sudo,
        // proxy or transport adapter while composing this one extrinsic.
        let built = child.build_dyn(&child_cx).await?;
        calls.push(built.call);
        for (key, value) in built.extras {
            extras.insert(format!("{index}:{}.{key}", child.spec().name), value);
        }
    }
    Ok(Built {
        call: utility::batch_all(calls).compose(cx.runtime)?,
        extras,
    })
}

#[derive(Clone, Copy)]
enum Hook {
    Effects,
    Warnings,
    Blocks,
}

fn prefix(out: &mut Vec<String>, index: usize, lines: Vec<String>) {
    out.extend(lines.into_iter().map(|line| format!("[{index}] {line}")));
}

async fn child_hook<V: View + ?Sized>(
    children: &[&dyn DynIntent],
    cx: &PreflightCx<'_, V>,
    hook: Hook,
) -> Result<Vec<String>, IntentError> {
    let mut out = if matches!(hook, Hook::Effects) {
        vec![format!(
            "all-or-nothing: {} calls in one extrinsic",
            children.len()
        )]
    } else {
        vec![]
    };
    let borrowed = cx.view;
    for (index, child) in children.iter().enumerate() {
        let child_cx = PreflightCx {
            view: &borrowed as &dyn DynView,
            runtime: cx.runtime,
            clock: cx.clock,
            spec: child.spec(),
            client: cx.client,
            dispatch_origin: cx.dispatch_origin,
            fee_payer: cx.fee_payer,
            call: cx.call,
        };
        prefix(
            &mut out,
            index,
            match hook {
                Hook::Effects => child.effects_dyn(&child_cx).await?,
                Hook::Warnings => child.warnings_dyn(&child_cx).await?,
                Hook::Blocks => child.blocks_dyn(&child_cx).await?,
            },
        );
    }
    Ok(out)
}

async fn preflight_children<V: View + ?Sized>(
    children: &[&dyn DynIntent],
    cx: &PreflightCx<'_, V>,
) -> Result<IntentPreflight, IntentError> {
    let mut out = IntentPreflight {
        effects: vec![format!(
            "all-or-nothing: {} calls in one extrinsic",
            children.len()
        )],
        ..Default::default()
    };
    let borrowed = cx.view;
    for (index, child) in children.iter().enumerate() {
        let child_cx = PreflightCx {
            view: &borrowed as &dyn DynView,
            runtime: cx.runtime,
            clock: cx.clock,
            spec: child.spec(),
            client: cx.client,
            dispatch_origin: cx.dispatch_origin,
            fee_payer: cx.fee_payer,
            call: cx.call,
        };
        let preview = child.preflight_dyn(&child_cx).await?;
        prefix(&mut out.effects, index, preview.effects);
        prefix(&mut out.warnings, index, preview.warnings);
        prefix(&mut out.blocks, index, preview.blocks);
        if preview.required_free.is_some_and(|required| {
            out.required_free
                .is_none_or(|previous| required.rao() > previous.rao())
        }) {
            // Each child describes the same complete extrinsic. Keep the full
            // triple from the strict maximum; summing reserves duplicates fees.
            out.required_free = preview.required_free;
            out.available_free = preview.available_free;
            out.estimated_fee = preview.estimated_fee;
        }
    }
    Ok(out)
}

impl Intent for Batch {
    fn signer_role(&self) -> Result<SignerRole, IntentError> {
        Batch::signer_role(self)
    }

    fn summary(&self) -> String {
        match children(self) {
            Ok(children) => format!(
                "atomic batch of {}: {}",
                children.len(),
                children
                    .iter()
                    .map(|child| child.as_dyn().summary_dyn())
                    .collect::<Vec<_>>()
                    .join("; ")
            ),
            Err(error) => format!("invalid atomic batch: {error}"),
        }
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        build_children(&refs(&children(self)?), cx).await
    }

    async fn effects<V: View + ?Sized>(
        &self,
        cx: &PreflightCx<'_, V>,
    ) -> Result<Vec<String>, IntentError> {
        child_hook(&refs(&children(self)?), cx, Hook::Effects).await
    }

    async fn warnings<V: View + ?Sized>(
        &self,
        cx: &PreflightCx<'_, V>,
    ) -> Result<Vec<String>, IntentError> {
        child_hook(&refs(&children(self)?), cx, Hook::Warnings).await
    }

    async fn blocks<V: View + ?Sized>(
        &self,
        cx: &PreflightCx<'_, V>,
    ) -> Result<Vec<String>, IntentError> {
        child_hook(&refs(&children(self)?), cx, Hook::Blocks).await
    }

    async fn preflight<V: View + ?Sized>(
        &self,
        cx: &PreflightCx<'_, V>,
    ) -> Result<IntentPreflight, IntentError> {
        preflight_children(&refs(&children(self)?), cx).await
    }

    fn spend(&self) -> Spend {
        let Ok(children) = children(self) else {
            return Spend::Unbounded;
        };
        let mut total = Balance::<Tao>::ZERO;
        let mut bounded = false;
        for child in children {
            match child.as_dyn().spend_dyn() {
                Spend::Unbounded => return Spend::Unbounded,
                Spend::None => {}
                Spend::Bounded(amount) => {
                    let Ok(sum) = total.checked_add(amount) else {
                        // Q25 keeps the native u64 range. Such a sum exceeds
                        // every representable configured spend cap.
                        return Spend::Unbounded;
                    };
                    total = sum;
                    bounded = true;
                }
            }
        }
        if bounded {
            Spend::Bounded(total)
        } else {
            Spend::None
        }
    }

    fn touches_netuids(&self) -> SmallVec<[NetUid; 2]> {
        children(self)
            .map(|children| {
                children
                    .iter()
                    .flat_map(|child| child.as_dyn().touches_netuids_dyn())
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect()
            })
            .unwrap_or_default()
    }

    fn affects_all_subnets(&self) -> bool {
        children(self).map_or(true, |children| {
            children
                .iter()
                .any(|child| child.as_dyn().affects_all_subnets_dyn())
        })
    }

    fn validate(&self) -> Result<(), IntentError> {
        children(self)?;
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use rtao_client::{BlockRef, Client, ClientConfig, Network, Snapshot};
    use rtao_primitives::AccountId;
    use rtao_runtime::{CallBytes, RuntimeHandle, calls::system};
    use rtao_testkit::{FakeChain, FakeState, RecordingView, rpc::RpcClient};
    use rtao_tx::{Clock, FactsTitle, IntentInterface, OriginView};
    use serde_json::json;
    use std::{
        sync::{Arc, Mutex},
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    const ORIGIN: AccountId = AccountId([7; 32]);
    const PAYER: AccountId = AccountId([8; 32]);
    #[cfg(any(feature = "transfer", all(feature = "coldkey", feature = "root")))]
    const DESTINATION: AccountId = AccountId([9; 32]);

    struct FixedClock;
    impl Clock for FixedClock {
        fn now(&self) -> SystemTime {
            UNIX_EPOCH + Duration::from_secs(123)
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

    fn decode(call: &CallBytes, runtime: &RuntimeHandle) -> anyhow::Result<Value> {
        let decoded = runtime
            .core()
            .decode_spec(&rtao_core::runtime::TypeSpec::Call, call.as_bytes(), true)
            .map_err(rtao_core::CoreError::from)?;
        Ok(rtao_core::codec::value::to_corpus_json(&decoded))
    }

    fn pfcx<'a, V: View + ?Sized>(
        view: &'a V,
        runtime: &'a RuntimeHandle,
        client: &'a Client,
        call: &'a CallBytes,
    ) -> PreflightCx<'a, V> {
        PreflightCx {
            view,
            runtime,
            client,
            call,
            clock: &FixedClock,
            spec: Batch::SPEC,
            dispatch_origin: ORIGIN,
            fee_payer: PAYER,
        }
    }

    #[test]
    fn constructor_rejects_empty_nested_missing_op_and_non_dictionary_children() {
        assert_eq!(
            Batch::new(vec![]).expect_err("empty batch").to_string(),
            "batch requires at least one intent"
        );
        assert_eq!(
            Batch::new(vec![json!({"op":"batch","intents":[]})])
                .expect_err("nested batch")
                .to_string(),
            "nested batches are not supported"
        );
        for (item, expected) in [
            (json!({}), "batched intent needs an 'op' key: {}"),
            (
                json!({"op":""}),
                "batched intent needs an 'op' key: {'op': ''}",
            ),
            (
                json!({"op":null}),
                "batched intent needs an 'op' key: {'op': None}",
            ),
            (
                json!({"op":false}),
                "batched intent needs an 'op' key: {'op': False}",
            ),
            (
                json!(null),
                "batched intent must be a dict or Intent, got <class 'NoneType'>",
            ),
            (
                json!(false),
                "batched intent must be a dict or Intent, got <class 'bool'>",
            ),
            (
                json!(1),
                "batched intent must be a dict or Intent, got <class 'int'>",
            ),
            (
                json!(1.5),
                "batched intent must be a dict or Intent, got <class 'float'>",
            ),
            (
                json!("{}"),
                "batched intent must be a dict or Intent, got <class 'str'>",
            ),
            (
                json!([]),
                "batched intent must be a dict or Intent, got <class 'list'>",
            ),
        ] {
            assert_eq!(
                Batch::new(vec![item])
                    .expect_err("invalid child")
                    .to_string(),
                expected
            );
        }
        let invalid = Batch {
            intents: vec![json!({})],
        };
        assert!(invalid.validate().is_err());
        assert_eq!(invalid.spend(), Spend::Unbounded);
        assert!(invalid.affects_all_subnets());
        assert!(invalid.summary().starts_with("invalid atomic batch:"));
        assert!(invalid.signer_role().is_err());
    }

    #[test]
    fn batch_constructor_and_programmatic_guards_reject_private_carriers_and_depth() {
        for key in [
            "$serde_json::private::RawValue",
            "$serde_json::private::Number",
        ] {
            let mut fields = serde_json::Map::new();
            fields.insert(key.into(), json!("{\"op\":\"batch\",\"intents\":[]}"));
            let item = Value::Object(fields);
            let mut args = json!({"intents":[]});
            args["intents"] = Value::Array(vec![item.clone()]);
            assert!(
                check_constructor_input(&args)
                    .expect_err("private constructor carrier")
                    .to_string()
                    .contains("private JSON carrier")
            );
            assert!(
                crate::AnyIntent::from_tool("batch", args)
                    .expect_err("pre-serde guard")
                    .to_string()
                    .contains("private JSON carrier")
            );
            let batch = Batch {
                intents: vec![item],
            };
            assert!(
                batch
                    .validate()
                    .expect_err("programmatic carrier")
                    .to_string()
                    .contains("private JSON carrier")
            );
        }
        let mut item = Value::Null;
        for _ in 0..127 {
            item = Value::Array(vec![item]);
        }
        assert!(crate::arguments::check_nested_json(&item).is_ok());
        let batch = Batch {
            intents: vec![item],
        };
        assert!(
            batch
                .validate()
                .expect_err("virtual array root counted")
                .to_string()
                .contains("depth budget")
        );
        let mut args = json!({"intents":[]});
        args["intents"] = Value::Array(batch.intents);
        assert!(
            crate::AnyIntent::from_tool("batch", args)
                .expect_err("constructor depth")
                .to_string()
                .contains("depth budget")
        );
    }

    #[cfg(all(feature = "transfer", feature = "proxy"))]
    #[tokio::test]
    async fn real_batch_normalizes_children_encodes_order_and_aggregates_exact_policy()
    -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture().await?;
        let view = RecordingView::new(snapshot.clone());
        let runtime = &snapshot.block().runtime;
        let requests = chain.requests().len();
        let batch = Batch::new(vec![
            json!({"op":"add_proxy","delegate_ss58":DESTINATION.to_string()}),
            json!({"op":"transfer","dest_ss58":DESTINATION.to_string(),"amount_tao":"0.000000123"}),
        ])?;
        assert_eq!(
            batch.intents[0],
            json!({"op":"add_proxy","delegate_ss58":DESTINATION.to_string(),"proxy_type":"Staking","delay":0})
        );
        assert_eq!(batch.intents[1]["keep_alive"], true);
        assert_eq!(batch.intents[1]["amount_tao"], "0.000000123");
        let parsed = crate::AnyIntent::from_tool("batch", batch.to_json())?;
        assert_eq!(parsed.to_json(), batch.to_json());
        assert_eq!(
            batch.summary(),
            format!(
                "atomic batch of 2: add Staking proxy {DESTINATION} (delay 0); transfer τ0.000000123 to {DESTINATION}"
            )
        );
        assert_eq!(batch.signer_role()?, SignerRole::Coldkey);
        assert_eq!(batch.spend(), Spend::Bounded(Balance::<Tao>::from_rao(123)));
        assert!(batch.touches_netuids().is_empty());
        assert!(!batch.affects_all_subnets());
        let built = batch
            .build(&BuildCx {
                view: &view,
                origin: OriginView::Account(ORIGIN),
                runtime,
                clock: &FixedClock,
                spec: Batch::SPEC,
            })
            .await?;
        let mut expected = vec![11, 2, 8, 16, 1, 0];
        expected.extend_from_slice(DESTINATION.as_bytes());
        expected.extend_from_slice(&[8, 0, 0, 0, 0]);
        // Balances.transfer_keep_alive: MultiAddress::Id + Compact(123).
        expected.extend_from_slice(&[5, 3, 0]);
        expected.extend_from_slice(DESTINATION.as_bytes());
        expected.extend_from_slice(&[237, 1]);
        assert_eq!(built.call.as_bytes(), expected);
        assert_eq!(CallBytes::decode(&expected, runtime)?, built.call);
        assert!(built.extras.is_empty());
        let decoded = decode(&built.call, runtime)?;
        assert_eq!(decoded["call_module"], "Utility");
        assert_eq!(decoded["call_function"], "batch_all");
        assert_eq!(
            decoded["call_args"][0]["value"][0]["call_function"],
            "add_proxy"
        );
        assert_eq!(
            decoded["call_args"][0]["value"][1]["call_function"],
            "transfer_keep_alive"
        );
        let preview = batch
            .preflight(&pfcx(&view, runtime, &client, &built.call))
            .await?;
        assert_eq!(
            preview.effects,
            vec![
                "all-or-nothing: 2 calls in one extrinsic".to_owned(),
                format!("[0] add Staking proxy {DESTINATION} (delay 0)"),
                format!("[1] transfer τ0.000000123 to {DESTINATION}")
            ]
        );
        assert_eq!(
            preview.warnings,
            ["[0] reserves a proxy deposit from the signer (returned on removal)"]
        );
        assert!(preview.blocks.is_empty());
        assert!(preview.required_free.is_none());
        assert!(preview.estimated_fee.is_none());
        assert!(view.touches().is_empty());
        assert_eq!(chain.requests().len(), requests);
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        client.close().await;
        Ok(())
    }

    #[cfg(feature = "transfer")]
    #[test]
    fn spend_distinguishes_none_bounded_zero_unbounded_and_native_sum_overflow()
    -> anyhow::Result<()> {
        let transfer = |amount: &str| json!({"op":"transfer","dest_ss58":DESTINATION.to_string(),"amount_tao":amount});
        assert_eq!(
            Batch::new(vec![transfer("0")])?.spend(),
            Spend::Bounded(Balance::<Tao>::ZERO)
        );
        assert_eq!(
            Batch::new(vec![transfer("0.000000001"), transfer("0.000000002")])?.spend(),
            Spend::Bounded(Balance::<Tao>::from_rao(3))
        );
        assert_eq!(
            Batch::new(vec![
                transfer("18446744073.709551615"),
                transfer("0.000000001")
            ])?
            .spend(),
            Spend::Unbounded
        );
        assert_eq!(
            Batch::new(vec![transfer("1"), transfer("all")])?.spend(),
            Spend::Unbounded
        );
        assert_eq!(
            Batch::new(vec![transfer("all"), transfer("1")])?.spend(),
            Spend::Unbounded
        );
        Ok(())
    }

    #[cfg(feature = "serving")]
    #[test]
    fn hotkey_batches_keep_role_and_sorted_deduplicated_subnet_union() -> anyhow::Result<()> {
        let batch = Batch::new(vec![
            json!({"op":"reset_axon","netuid":9}),
            json!({"op":"serve_axon","netuid":3,"ip":"127.0.0.1","port":80}),
            json!({"op":"reset_axon","netuid":9}),
        ])?;
        assert_eq!(batch.signer_role()?, SignerRole::Hotkey);
        assert_eq!(batch.signer_role_dyn()?, SignerRole::Hotkey);
        let parsed = crate::AnyIntent::from_tool("batch", batch.to_json())?;
        assert_eq!(parsed.as_dyn().signer_role_dyn()?, SignerRole::Hotkey);
        assert_eq!(batch.touches_netuids().as_slice(), &[NetUid(3), NetUid(9)]);
        assert_eq!(batch.spend(), Spend::None);
        assert!(!batch.affects_all_subnets());
        assert_eq!(batch.intents[1]["version"], json!(1));
        assert_eq!(batch.intents[1]["protocol"], json!(4));
        Ok(())
    }

    #[cfg(all(feature = "serving", feature = "proxy"))]
    #[test]
    fn mixed_signers_are_rejected_in_both_orders() {
        let cold = json!({"op":"remove_proxies"});
        let hot = json!({"op":"reset_axon","netuid":1});
        for children in [vec![cold.clone(), hot.clone()], vec![hot, cold]] {
            assert_eq!(
                Batch::new(children).expect_err("mixed roles").to_string(),
                "all batched intents must share one signer, got ['coldkey', 'hotkey']; split into separate batches"
            );
        }
    }

    #[cfg(all(feature = "coldkey", feature = "root"))]
    #[tokio::test]
    async fn child_root_calls_remain_semantic_and_global_policy_propagates() -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture().await?;
        let runtime = &snapshot.block().runtime;
        let view = RecordingView::new(snapshot.clone());
        let requests = chain.requests().len();
        let batch = Batch::new(vec![
            json!({"op":"set_subnet_emission_enabled","netuids":[9,3],"enabled":true}),
            json!({"op":"swap_coldkey_announced","new_coldkey_ss58":DESTINATION.to_string()}),
        ])?;
        assert!(batch.affects_all_subnets());
        assert_eq!(batch.touches_netuids().as_slice(), &[NetUid(3), NetUid(9)]);
        let built = batch
            .build(&BuildCx {
                view: &view,
                origin: OriginView::Account(ORIGIN),
                runtime,
                clock: &FixedClock,
                spec: Batch::SPEC,
            })
            .await?;
        let decoded = decode(&built.call, runtime)?;
        let inner = &decoded["call_args"][0]["value"][0];
        // The child's own semantic Utility.batch_all is allowed; an explicit
        // child Batch intent is what the Python constructor prohibits.
        assert_eq!(inner["call_module"], "Utility");
        assert_eq!(inner["call_function"], "batch_all");
        assert_eq!(
            inner["call_args"][0]["value"][0]["call_module"],
            "AdminUtils"
        );
        assert_eq!(
            inner["call_args"][0]["value"][0]["call_function"],
            "sudo_set_subnet_emission_enabled"
        );
        assert!(view.touches().is_empty());
        assert_eq!(chain.requests().len(), requests);
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        client.close().await;
        Ok(())
    }

    struct Probe {
        tag: u8,
        trace: Arc<Mutex<Vec<String>>>,
        preview: IntentPreflight,
        parent_call: CallBytes,
        fail_build: bool,
    }
    impl IntentInterface for Probe {
        const SPEC: &'static rtao_catalog::IntentSpec = &rtao_catalog::INTENTS[0];
        type Role = rtao_signer::Coldkey;
        type Origin = rtao_tx::Signed;
        type Output = ();
        const WRAPS: &'static [&'static rtao_runtime::CallDescriptor] = &[&system::REMARK];
        fn to_json(&self) -> Value {
            json!({"op":Self::SPEC.name,"tag":self.tag})
        }
    }
    impl Probe {
        fn note(&self, hook: &str) {
            self.trace
                .lock()
                .expect("trace")
                .push(format!("{hook}:{}", self.tag));
        }
        fn check<V: View + ?Sized>(&self, cx: &PreflightCx<'_, V>) {
            assert_eq!(cx.call, &self.parent_call);
            assert_eq!(cx.dispatch_origin, ORIGIN);
            assert_eq!(cx.fee_payer, PAYER);
            assert_eq!(cx.spec, Self::SPEC);
            assert_eq!(cx.clock.now(), FixedClock.now());
            assert_eq!(cx.call.stamp(), cx.runtime.stamp());
        }
    }
    impl Intent for Probe {
        fn summary(&self) -> String {
            format!("probe {}", self.tag)
        }
        async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
            self.note("build");
            assert_eq!(cx.coldkey_address()?, ORIGIN);
            assert_eq!(cx.spec, Self::SPEC);
            assert_eq!(cx.clock.now(), FixedClock.now());
            if self.fail_build {
                return Err(IntentError::validation("probe build refused"));
            }
            let mut built = Built::new(system::remark(vec![self.tag]).compose(cx.runtime)?);
            built.extras.insert("receipt".into(), json!(self.tag));
            Ok(built)
        }
        async fn effects<V: View + ?Sized>(
            &self,
            cx: &PreflightCx<'_, V>,
        ) -> Result<Vec<String>, IntentError> {
            self.check(cx);
            self.note("effects");
            Ok(vec![format!("effect {}", self.tag)])
        }
        async fn warnings<V: View + ?Sized>(
            &self,
            cx: &PreflightCx<'_, V>,
        ) -> Result<Vec<String>, IntentError> {
            self.check(cx);
            self.note("warnings");
            Ok(vec![format!("warning {}", self.tag)])
        }
        async fn blocks<V: View + ?Sized>(
            &self,
            cx: &PreflightCx<'_, V>,
        ) -> Result<Vec<String>, IntentError> {
            self.check(cx);
            self.note("blocks");
            Ok(vec![format!("block {}", self.tag)])
        }
        async fn preflight<V: View + ?Sized>(
            &self,
            cx: &PreflightCx<'_, V>,
        ) -> Result<IntentPreflight, IntentError> {
            self.check(cx);
            self.note("preflight");
            Ok(self.preview.clone())
        }
    }

    fn probe(
        tag: u8,
        call: &CallBytes,
        trace: &Arc<Mutex<Vec<String>>>,
        required: Option<u64>,
        available: Option<u64>,
        fee: Option<u64>,
    ) -> Probe {
        Probe {
            tag,
            trace: trace.clone(),
            parent_call: call.clone(),
            fail_build: false,
            preview: IntentPreflight {
                effects: vec![format!("effect {tag}"), format!("second {tag}")],
                warnings: vec![format!("warning {tag}")],
                blocks: vec![format!("block {tag}")],
                required_free: required.map(Balance::<Tao>::from_rao),
                available_free: available.map(Balance::<Tao>::from_rao),
                estimated_fee: fee.map(Balance::<Tao>::from_rao),
                facts: vec![("child fact".into(), tag.to_string())],
                facts_title: FactsTitle::Quote,
            },
        }
    }

    #[tokio::test]
    async fn batch_preflight_uses_strict_maximum_with_correlated_reserve_triple_and_full_parent_call()
    -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture().await?;
        let runtime = &snapshot.block().runtime;
        let view = RecordingView::new(snapshot.clone());
        let requests = chain.requests().len();
        let parent = system::remark(vec![99]).compose(runtime)?;
        let trace = Arc::new(Mutex::new(Vec::new()));
        let a = probe(0, &parent, &trace, Some(7), Some(100), Some(3));
        let b = probe(1, &parent, &trace, Some(9), Some(200), Some(5));
        let c = probe(2, &parent, &trace, Some(9), Some(999), Some(999));
        let d = probe(3, &parent, &trace, None, Some(888), Some(888));
        let preview =
            preflight_children(&[&a, &b, &c, &d], &pfcx(&view, runtime, &client, &parent)).await?;
        assert_eq!(preview.required_free, Some(Balance::<Tao>::from_rao(9)));
        assert_eq!(preview.available_free, Some(Balance::<Tao>::from_rao(200)));
        assert_eq!(preview.estimated_fee, Some(Balance::<Tao>::from_rao(5)));
        assert_eq!(
            preview.effects,
            [
                "all-or-nothing: 4 calls in one extrinsic",
                "[0] effect 0",
                "[0] second 0",
                "[1] effect 1",
                "[1] second 1",
                "[2] effect 2",
                "[2] second 2",
                "[3] effect 3",
                "[3] second 3"
            ]
        );
        assert_eq!(
            preview.warnings,
            [
                "[0] warning 0",
                "[1] warning 1",
                "[2] warning 2",
                "[3] warning 3"
            ]
        );
        assert_eq!(
            preview.blocks,
            ["[0] block 0", "[1] block 1", "[2] block 2", "[3] block 3"]
        );
        assert!(preview.facts.is_empty());
        assert_eq!(preview.facts_title, FactsTitle::Fees);
        assert_eq!(
            *trace.lock().expect("trace"),
            ["preflight:0", "preflight:1", "preflight:2", "preflight:3"]
        );
        let e = probe(4, &parent, &trace, Some(10), None, None);
        let preview =
            preflight_children(&[&b, &e], &pfcx(&view, runtime, &client, &parent)).await?;
        assert_eq!(preview.required_free, Some(Balance::<Tao>::from_rao(10)));
        assert!(preview.available_free.is_none());
        assert!(preview.estimated_fee.is_none());
        let zero = probe(5, &parent, &trace, Some(0), Some(0), Some(0));
        let preview =
            preflight_children(&[&d, &zero], &pfcx(&view, runtime, &client, &parent)).await?;
        assert_eq!(preview.required_free, Some(Balance::<Tao>::ZERO));
        assert_eq!(preview.available_free, Some(Balance::<Tao>::ZERO));
        assert_eq!(preview.estimated_fee, Some(Balance::<Tao>::ZERO));
        assert!(view.touches().is_empty());
        assert_eq!(chain.requests().len(), requests);
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn direct_hooks_do_not_invoke_other_child_hooks_or_full_preflight() -> anyhow::Result<()>
    {
        let (chain, client, snapshot) = fixture().await?;
        let runtime = &snapshot.block().runtime;
        let view = RecordingView::new(snapshot.clone());
        let requests = chain.requests().len();
        let call = system::remark(vec![99]).compose(runtime)?;
        let trace = Arc::new(Mutex::new(Vec::new()));
        let a = probe(0, &call, &trace, None, None, None);
        let b = probe(1, &call, &trace, None, None, None);
        let cx = pfcx(&view, runtime, &client, &call);
        for (hook, expected, notes) in [
            (
                Hook::Effects,
                vec![
                    "all-or-nothing: 2 calls in one extrinsic",
                    "[0] effect 0",
                    "[1] effect 1",
                ],
                vec!["effects:0", "effects:1"],
            ),
            (
                Hook::Warnings,
                vec!["[0] warning 0", "[1] warning 1"],
                vec!["warnings:0", "warnings:1"],
            ),
            (
                Hook::Blocks,
                vec!["[0] block 0", "[1] block 1"],
                vec!["blocks:0", "blocks:1"],
            ),
        ] {
            trace.lock().expect("trace").clear();
            assert_eq!(child_hook(&[&a, &b], &cx, hook).await?, expected);
            assert_eq!(*trace.lock().expect("trace"), notes);
        }
        assert!(view.touches().is_empty());
        assert_eq!(chain.requests().len(), requests);
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn batch_build_prefixes_child_extras_and_stops_at_first_refusal() -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture().await?;
        let runtime = &snapshot.block().runtime;
        let view = RecordingView::new(snapshot.clone());
        let requests = chain.requests().len();
        let call = system::remark(vec![99]).compose(runtime)?;
        let trace = Arc::new(Mutex::new(Vec::new()));
        let a = probe(0, &call, &trace, None, None, None);
        let mut b = probe(1, &call, &trace, None, None, None);
        let c = probe(2, &call, &trace, None, None, None);
        let cx = BuildCx {
            view: &view,
            origin: OriginView::Account(ORIGIN),
            runtime,
            clock: &FixedClock,
            spec: Batch::SPEC,
        };
        let built = build_children(&[&a, &b], &cx).await?;
        assert_eq!(
            built.extras,
            json!({"0:add_collateral.receipt":0,"1:add_collateral.receipt":1})
                .as_object()
                .expect("object")
                .clone()
        );
        let decoded = decode(&built.call, runtime)?;
        assert_eq!(decoded["call_module"], "Utility");
        assert_eq!(decoded["call_function"], "batch_all");
        assert_eq!(
            decoded["call_args"][0]["value"][0]["call_function"],
            "remark"
        );
        assert_eq!(
            decoded["call_args"][0]["value"][1]["call_function"],
            "remark"
        );
        assert_eq!(*trace.lock().expect("trace"), ["build:0", "build:1"]);
        trace.lock().expect("trace").clear();
        b.fail_build = true;
        assert_eq!(
            build_children(&[&a, &b, &c], &cx)
                .await
                .expect_err("middle child refused")
                .to_string(),
            "probe build refused"
        );
        assert_eq!(*trace.lock().expect("trace"), ["build:0", "build:1"]);
        assert!(view.touches().is_empty());
        assert_eq!(chain.requests().len(), requests);
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        client.close().await;
        Ok(())
    }
}
