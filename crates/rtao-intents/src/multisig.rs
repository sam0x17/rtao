// Derived from RaoFoundation/subtensor sdk/python/bittensor/intents/multisig.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Explicit multisig dispatch, approval and cancellation calls.

use rtao_client::View;
use rtao_core::DynValue;
use rtao_primitives::{AccountId, CryptoType};
use rtao_runtime::{CallBytes, calls::multisig, models::MultisigTimepoint};
use rtao_tx::{
    BuildCx, Built, Intent, IntentError, OriginView, PreflightCx, TxError,
    multisig::{FundsRequest, check_multisig_funds, multisig_opening_shortfall},
};
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;

pub use crate::{MultisigApprove, MultisigCancel, MultisigExecute, MultisigThreshold1};

/// Bound every untrusted JSON carrier before generated deserialization copies it.
pub(crate) fn check_constructor_input(args: &Value) -> Result<(), IntentError> {
    crate::arguments::check_nested_json(args)
}

/// Keep typed interface serialization bounded before cloning nested carriers.
pub(crate) fn intent_json(
    op: &str,
    intent: &impl serde::Serialize,
    others: &[Value],
    call: &Value,
    timepoint: Option<&Value>,
) -> Value {
    if check_inputs(others, call, timepoint).is_err() {
        // There is no Python constructor value for a rejected local-budget
        // input. Preserve bounded diagnostics; validate() carries the refusal.
        return json!({"op":op,"invalid_nested_input":true});
    }
    crate::args_with_op(op, intent)
}

fn check_inputs(
    others: &[Value],
    call: &Value,
    timepoint: Option<&Value>,
) -> Result<(), IntentError> {
    crate::arguments::check_nested_json(call)?;
    crate::arguments::check_nested_items(others)?;
    if let Some(timepoint) = timepoint {
        crate::arguments::check_nested_json(timepoint)?;
    }
    Ok(())
}

fn addresses(others: &[Value]) -> Result<Vec<AccountId>, IntentError> {
    others
        .iter()
        .map(|address| {
            address
                .as_str()
                .ok_or_else(|| {
                    IntentError::validation("multisig signatories must be SS58 strings")
                })?
                .parse::<AccountId>()
                .map_err(IntentError::from)
        })
        .collect()
}

fn validate_multisig(
    threshold: u16,
    others: &[Value],
    signer: AccountId,
) -> Result<(), IntentError> {
    if threshold < 2 {
        return Err(IntentError::validation(format!(
            "multisig threshold must be at least 2, got {threshold} (use multisig_threshold_1 for 1-of-N)"
        )));
    }
    if usize::from(threshold) > others.len().saturating_add(1) {
        return Err(IntentError::validation(format!(
            "multisig threshold {threshold} exceeds the signatory count {} (signer plus other_signatories)",
            others.len().saturating_add(1)
        )));
    }
    for other in others {
        let address = other
            .as_str()
            .ok_or_else(|| IntentError::validation("multisig signatories must be SS58 strings"))?
            .parse::<AccountId>()?;
        if address == signer {
            return Err(IntentError::validation(format!(
                "other_signatories must not include the signer's own account ({signer}); list only the other members"
            )));
        }
    }
    Ok(())
}

fn group_account(
    runtime: &rtao_runtime::RuntimeHandle,
    signer: AccountId,
    threshold: u16,
    others: &[Value],
) -> Result<AccountId, IntentError> {
    let mut members = addresses(others)?;
    members.push(signer);
    // D-039: use the existing runtime-cap and duplicate-set refusal before
    // returning any funding address. Core still owns sorting and derivation.
    Ok(rtao_tx::Multisig::new(runtime, &members, threshold)
        .map_err(intent_error)?
        .address)
}

fn sorted_others(others: &[Value]) -> Result<Vec<AccountId>, IntentError> {
    // Python deduplicates spellings first, then sorts by decoded account bytes.
    let unique: BTreeSet<&str> = others
        .iter()
        .map(|value| {
            value
                .as_str()
                .ok_or_else(|| IntentError::validation("multisig signatories must be SS58 strings"))
        })
        .collect::<Result<_, _>>()?;
    let mut result = unique
        .into_iter()
        .map(str::parse::<AccountId>)
        .collect::<Result<Vec<_>, _>>()?;
    result.sort_unstable();
    Ok(result)
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64() != Some(0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
    }
}

fn op_label(call: &Value) -> String {
    if crate::arguments::check_nested_json(call).is_err() {
        return "<invalid inner call>".into();
    }
    match call.get("op") {
        None | Some(Value::Null) => "None".into(),
        Some(Value::String(value)) => value.clone(),
        Some(Value::Bool(true)) => "True".into(),
        Some(Value::Bool(false)) => "False".into(),
        Some(value) => value.to_string(),
    }
}

async fn compose_inner<V: View + ?Sized>(
    cx: &BuildCx<'_, V>,
    account: AccountId,
    call: &Value,
) -> Result<CallBytes, IntentError> {
    crate::arguments::check_nested_json(call)?;
    let fields = call
        .as_object()
        .ok_or_else(|| IntentError::validation("multisig inner call must be an intent object"))?;
    let op = fields
        .get("op")
        .filter(|value| truthy(value))
        .ok_or_else(|| IntentError::validation("multisig inner call needs an 'op' key"))?;
    let op = op.as_str().ok_or_else(|| {
        IntentError::validation("multisig inner call 'op' must name a registered intent")
    })?;
    let intent = crate::AnyIntent::from_tool(op, call.clone())?;
    let borrowed = cx.view;
    let inner = intent
        .as_dyn()
        .build_dyn(&BuildCx {
            view: &borrowed as &dyn rtao_client::DynView,
            origin: OriginView::Account(account),
            runtime: cx.runtime,
            clock: cx.clock,
            spec: intent.spec(),
        })
        .await?;
    // Raw registered multisig intents call only the semantic child's build.
    // Python discards its extras and adds no executor sudo/proxy/adapters.
    Ok(inner.call)
}

fn timepoint(value: Option<&Value>) -> Result<Option<MultisigTimepoint>, IntentError> {
    let Some(value) = value.filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let fields = value
        .as_object()
        .ok_or_else(|| IntentError::validation("multisig timepoint must be an object"))?;
    let integer = |field: &str| {
        let value = fields.get(field).ok_or_else(|| {
            IntentError::validation(format!("multisig timepoint needs '{field}'"))
        })?;
        match value {
            Value::Bool(value) => Ok(u32::from(*value)),
            Value::Number(value) => {
                if let Some(value) = value.as_u64() {
                    return u32::try_from(value).map_err(|_| {
                        IntentError::validation(format!("multisig timepoint {field} exceeds u32"))
                    });
                }
                if let Some(value) = value.as_f64()
                    && value.is_finite()
                    && value.trunc() >= 0.0
                    && value.trunc() <= f64::from(u32::MAX)
                {
                    return Ok(value.trunc() as u32);
                }
                Err(IntentError::validation(format!(
                    "multisig timepoint {field} must fit u32"
                )))
            }
            Value::String(value) => rtao_primitives::pyfmt::parse_integer(value)
                .and_then(|value| value.to_string().parse::<u32>().ok())
                .ok_or_else(|| {
                    IntentError::validation(format!("multisig timepoint {field} must fit u32"))
                }),
            _ => Err(IntentError::validation(format!(
                "multisig timepoint {field} must be an integer"
            ))),
        }
    };
    Ok(Some(MultisigTimepoint {
        height: integer("height")?,
        index: integer("index")?,
    }))
}

fn inner_extras(inner: &CallBytes) -> Map<String, Value> {
    Map::from_iter([
        (
            "multisig_call_hash".into(),
            json!(format!("0x{}", hex::encode(inner.hash()))),
        ),
        (
            "multisig_call_data".into(),
            json!(format!("0x{}", hex::encode(inner.as_bytes()))),
        ),
    ])
}

fn intent_error(error: TxError) -> IntentError {
    match error {
        TxError::Intent(error) => error,
        TxError::Chain(error) => IntentError::Chain(error),
        TxError::Signer(error) => IntentError::Signer(error),
        TxError::UnitMismatch(error) => IntentError::UnitMismatch(error),
        error => IntentError::validation(error.to_string()),
    }
}

#[derive(Clone, Copy)]
enum Dispatch {
    Execute,
    Approve,
}

async fn build_approval<V: View + ?Sized>(
    cx: &BuildCx<'_, V>,
    dispatch: Dispatch,
    threshold: u16,
    others: &[Value],
    call: &Value,
    when: Option<&Value>,
) -> Result<Built, IntentError> {
    check_inputs(others, call, when)?;
    let signer = cx.coldkey_address()?;
    if matches!(dispatch, Dispatch::Approve) {
        // Approve checks threshold/self membership before building the child;
        // Execute deliberately performs this check after composing the child.
        validate_multisig(threshold, others, signer)?;
    }
    let account = group_account(cx.runtime, signer, threshold, others)?;
    let inner = compose_inner(cx, account, call).await?;
    if matches!(dispatch, Dispatch::Execute) {
        validate_multisig(threshold, others, signer)?;
    }
    let snapshot = cx.view.pin().await?;
    let estimate = snapshot
        .client()
        .estimate_weight(inner.clone(), signer, CryptoType::Sr25519)
        .await?;
    let when = timepoint(when)?;
    let outer = if matches!(dispatch, Dispatch::Approve) && when.is_some() {
        multisig::approve_as_multi(
            threshold,
            sorted_others(others)?,
            when.clone(),
            inner.hash(),
            DynValue::new(estimate.weight),
        )
    } else {
        multisig::as_multi(
            threshold,
            sorted_others(others)?,
            when.clone(),
            inner.clone(),
            DynValue::new(estimate.weight),
        )
    }
    .compose(cx.runtime)?;
    check_multisig_funds(
        snapshot.client(),
        cx.view,
        FundsRequest {
            signer,
            threshold,
            opening: when.is_none(),
            outer_call: Some(&outer),
            crypto_type: CryptoType::Sr25519,
            signer_label: None,
        },
    )
    .await
    .map_err(intent_error)?;
    Ok(Built {
        call: outer,
        extras: inner_extras(&inner),
    })
}

impl Intent for MultisigExecute {
    fn summary(&self) -> String {
        format!(
            "multisig {}-of-{} execute {}",
            self.threshold,
            self.other_signatories.len().saturating_add(1),
            op_label(&self.call)
        )
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        build_approval(
            cx,
            Dispatch::Execute,
            self.threshold,
            &self.other_signatories,
            &self.call,
            self.timepoint.as_ref(),
        )
        .await
    }

    async fn warnings<V: View + ?Sized>(
        &self,
        cx: &PreflightCx<'_, V>,
    ) -> Result<Vec<String>, IntentError> {
        Ok(if self.timepoint.is_none() {
            multisig_opening_shortfall(cx.view, cx.dispatch_origin, self.threshold)
                .await
                .into_iter()
                .collect()
        } else {
            vec![]
        })
    }

    fn validate(&self) -> Result<(), IntentError> {
        check_inputs(&self.other_signatories, &self.call, self.timepoint.as_ref())
    }
}

impl Intent for MultisigApprove {
    fn summary(&self) -> String {
        format!(
            "multisig {}-of-{} approve {}",
            self.threshold,
            self.other_signatories.len().saturating_add(1),
            op_label(&self.call)
        )
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        build_approval(
            cx,
            Dispatch::Approve,
            self.threshold,
            &self.other_signatories,
            &self.call,
            self.timepoint.as_ref(),
        )
        .await
    }

    async fn warnings<V: View + ?Sized>(
        &self,
        cx: &PreflightCx<'_, V>,
    ) -> Result<Vec<String>, IntentError> {
        Ok(if self.timepoint.is_none() {
            multisig_opening_shortfall(cx.view, cx.dispatch_origin, self.threshold)
                .await
                .into_iter()
                .collect()
        } else {
            vec![]
        })
    }

    fn validate(&self) -> Result<(), IntentError> {
        check_inputs(&self.other_signatories, &self.call, self.timepoint.as_ref())
    }
}

impl Intent for MultisigThreshold1 {
    fn summary(&self) -> String {
        format!("multisig (1-of-N) dispatch {}", op_label(&self.call))
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        check_inputs(&self.other_signatories, &self.call, None)?;
        let signer = cx.coldkey_address()?;
        let account = group_account(cx.runtime, signer, 1, &self.other_signatories)?;
        let inner = compose_inner(cx, account, &self.call).await?;
        Ok(Built::new(
            multisig::as_multi_threshold_1(sorted_others(&self.other_signatories)?, inner)
                .compose(cx.runtime)?,
        ))
    }

    fn validate(&self) -> Result<(), IntentError> {
        check_inputs(&self.other_signatories, &self.call, None)
    }
}

impl Intent for MultisigCancel {
    fn summary(&self) -> String {
        format!("cancel multisig operation for {}", op_label(&self.call))
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        check_inputs(&self.other_signatories, &self.call, Some(&self.timepoint))?;
        let signer = cx.coldkey_address()?;
        let account = group_account(cx.runtime, signer, self.threshold, &self.other_signatories)?;
        let inner = compose_inner(cx, account, &self.call).await?;
        let when = timepoint(Some(&self.timepoint))?
            .ok_or_else(|| IntentError::validation("multisig cancellation requires a timepoint"))?;
        Ok(Built::new(
            multisig::cancel_as_multi(
                self.threshold,
                sorted_others(&self.other_signatories)?,
                when,
                inner.hash(),
            )
            .compose(cx.runtime)?,
        ))
    }

    fn validate(&self) -> Result<(), IntentError> {
        check_inputs(&self.other_signatories, &self.call, Some(&self.timepoint))
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    #[cfg(feature = "transfer")]
    use rtao_client::DynView;
    use rtao_client::{BlockRef, Client, ClientConfig, Network, Snapshot};
    #[cfg(feature = "transfer")]
    use rtao_primitives::{H256, Spend};
    #[cfg(any(feature = "transfer", feature = "evm"))]
    use rtao_runtime::RuntimeHandle;
    #[cfg(feature = "transfer")]
    use rtao_testkit::RecordingView;
    use rtao_testkit::{FakeChain, FakeState, rpc::RpcClient};
    #[cfg(feature = "transfer")]
    use rtao_tx::{DynIntent, IntentOutput};
    use rtao_tx::{IntentInterface, OriginView, SystemClock};

    const MEMBER: AccountId = AccountId([7; 32]);
    const OTHER: AccountId = AccountId([8; 32]);

    async fn fixture(free: u64) -> anyhow::Result<(FakeChain, Client, Snapshot)> {
        let mut state = FakeState::try_default()?;
        state.fee = 7;
        state.weight = json!({"ref_time":1,"proof_size":2});
        state.seed_constant("Multisig", "DepositBase", json!(100))?;
        state.seed_constant("Multisig", "DepositFactor", json!(10))?;
        state.seed_python(
            "System",
            "Account",
            &[json!(MEMBER)],
            json!({"data":{"free":free}}),
        )?;
        let chain = FakeChain::from_state(state);
        let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
        config.track_heads = false;
        config.runtime_cache_dir = None;
        config.config_store = None;
        let client = Client::from_rpc(RpcClient::new(chain.clone()), config).await?;
        let snapshot = client.at(BlockRef::Finalized).await?;
        Ok((chain, client, snapshot))
    }

    #[cfg(any(feature = "transfer", feature = "evm"))]
    fn decode(call: &CallBytes, runtime: &RuntimeHandle) -> anyhow::Result<Value> {
        let decoded = runtime
            .core()
            .decode_spec(&rtao_core::runtime::TypeSpec::Call, call.as_bytes(), true)
            .map_err(rtao_core::CoreError::from)?;
        Ok(rtao_core::codec::value::to_corpus_json(&decoded))
    }

    #[cfg(any(feature = "transfer", feature = "evm"))]
    fn arg<'a>(call: &'a Value, name: &str) -> anyhow::Result<&'a Value> {
        call["call_args"]
            .as_array()
            .and_then(|args| args.iter().find(|arg| arg["name"] == name))
            .map(|arg| &arg["value"])
            .ok_or_else(|| anyhow::anyhow!("missing {name}"))
    }

    #[cfg(feature = "transfer")]
    fn transfer() -> Value {
        json!({"op":"transfer","dest_ss58":AccountId([1;32]).to_string(),"amount_tao":"0.000000001","keep_alive":false})
    }

    #[cfg(feature = "transfer")]
    fn transfer_bytes() -> Vec<u8> {
        let mut bytes = vec![5, 0, 0];
        bytes.extend_from_slice(&[1; 32]);
        bytes.push(4);
        bytes
    }

    #[cfg(feature = "transfer")]
    #[tokio::test]
    async fn raw_multisig_operations_encode_full_openings_hash_approvals_and_fixed_width_timepoints()
    -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture(1_000).await?;
        let runtime = &snapshot.block().runtime;
        let inner_bytes = transfer_bytes();
        let inner = CallBytes::decode(&inner_bytes, runtime)?;
        let execute = MultisigExecute {
            threshold: 2,
            other_signatories: vec![json!(OTHER)],
            call: transfer(),
            timepoint: None,
        };
        let approve = MultisigApprove {
            threshold: 2,
            other_signatories: vec![json!(OTHER)],
            call: transfer(),
            timepoint: None,
        };
        let approve_later = MultisigApprove {
            timepoint: Some(json!({"height":"12","index":3.9,"ignored":true})),
            ..approve.clone()
        };
        let execute_later = MultisigExecute {
            timepoint: Some(json!({"height":12,"index":3})),
            ..execute.clone()
        };
        let cancel = MultisigCancel {
            threshold: 2,
            other_signatories: vec![json!(OTHER)],
            call: transfer(),
            timepoint: json!({"height":12,"index":3}),
        };
        let immediate = MultisigThreshold1 {
            other_signatories: vec![json!(OTHER)],
            call: transfer(),
        };
        for (intent, call_index, opening, hash_only) in [
            (&execute as &dyn DynIntent, 1, true, false),
            (&approve as &dyn DynIntent, 1, true, false),
            (&execute_later as &dyn DynIntent, 1, false, false),
            (&approve_later as &dyn DynIntent, 2, false, true),
            (&cancel as &dyn DynIntent, 3, false, true),
            (&immediate as &dyn DynIntent, 0, true, false),
        ] {
            let built = intent
                .build_dyn(&BuildCx {
                    view: &snapshot as &dyn DynView,
                    origin: OriginView::Account(MEMBER),
                    runtime,
                    clock: &SystemClock,
                    spec: intent.spec(),
                })
                .await?;
            let mut expected = vec![13, call_index];
            if call_index != 0 {
                expected.extend_from_slice(&[2, 0]);
            }
            expected.push(4);
            expected.extend_from_slice(&[8; 32]);
            // Pinned pallet_multisig::Timepoint<u32> has two ordinary u32
            // fields, unlike the compact WeightV2 fields below.
            if call_index == 3 {
                expected.extend_from_slice(&12u32.to_le_bytes());
                expected.extend_from_slice(&3u32.to_le_bytes());
            } else if call_index != 0 {
                expected.push(u8::from(!opening));
                if !opening {
                    expected.extend_from_slice(&12u32.to_le_bytes());
                    expected.extend_from_slice(&3u32.to_le_bytes());
                }
            }
            if hash_only {
                expected.extend_from_slice(&inner.hash());
            } else {
                expected.extend_from_slice(&inner_bytes);
            }
            if matches!(call_index, 1 | 2) {
                expected.extend_from_slice(&[4, 8]);
            }
            assert_eq!(built.call.as_bytes(), expected, "{}", intent.spec().name);
            assert_eq!(CallBytes::decode(&expected, runtime)?, built.call);
            let decoded = decode(&built.call, runtime)?;
            assert_eq!(decoded["call_module"], "Multisig");
            if matches!(call_index, 1 | 2) {
                assert_eq!(built.extras, inner_extras(&inner));
                let typed = rtao_tx::MultisigCall::from_data(&built.extras)?;
                assert_eq!(typed.call_hash, Some(H256(inner.hash())));
                assert_eq!(typed.call_data, Some(inner_bytes.clone()));
                assert_eq!(
                    arg(&decoded, "max_weight")?,
                    &json!({"ref_time":1,"proof_size":2})
                );
            } else {
                assert!(built.extras.is_empty());
                assert_eq!(
                    rtao_tx::MultisigCall::from_data(&built.extras)?,
                    rtao_tx::MultisigCall::default()
                );
            }
            assert_eq!(intent.spend_dyn(), Spend::None);
            assert!(intent.touches_netuids_dyn().is_empty());
            assert!(!intent.affects_all_subnets_dyn());
            assert_eq!(intent.semantic_intent_dyn().spec().name, intent.spec().name);
        }
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        client.close().await;
        Ok(())
    }

    #[cfg(feature = "transfer")]
    #[tokio::test]
    async fn raw_multisig_never_looks_up_pending_or_autofills_the_explicit_opening()
    -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture(1_000).await?;
        let runtime = &snapshot.block().runtime;
        let account = group_account(runtime, MEMBER, 2, &[json!(OTHER)])?;
        let inner = CallBytes::decode(&transfer_bytes(), runtime)?;
        chain.mutate_state(|state|state.seed_python("Multisig","Multisigs",&[json!(account),json!(format!("0x{}",hex::encode(inner.hash())))],json!({"when":{"height":12,"index":3},"deposit":120,"depositor":MEMBER,"approvals":[MEMBER]})))?;
        let recorded = RecordingView::new(snapshot.clone());
        let intent = MultisigExecute {
            threshold: 2,
            other_signatories: vec![json!(OTHER)],
            call: transfer(),
            timepoint: None,
        };
        let built = intent
            .build(&BuildCx {
                view: &recorded,
                origin: OriginView::Account(MEMBER),
                runtime,
                clock: &SystemClock,
                spec: MultisigExecute::SPEC,
            })
            .await?;
        assert_eq!(
            arg(&decode(&built.call, runtime)?, "maybe_timepoint")?,
            &Value::Null
        );
        assert!(
            recorded
                .touches()
                .iter()
                .all(|touch| touch.pallet_or_api != "Multisig" || touch.item != "Multisigs")
        );
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        client.close().await;
        Ok(())
    }

    #[cfg(feature = "evm")]
    #[tokio::test]
    async fn raw_multisig_origin_default_resolves_to_composite_account() -> anyhow::Result<()> {
        let (_chain, client, snapshot) = fixture(1_000).await?;
        let runtime = &snapshot.block().runtime;
        let intent = MultisigThreshold1 {
            other_signatories: vec![json!(OTHER)],
            call: json!({"op":"evm_withdraw","amount_tao":"0.000000001"}),
        };
        let account = group_account(runtime, MEMBER, 1, &intent.other_signatories)?;
        let built = intent
            .build(&BuildCx {
                view: &snapshot,
                origin: OriginView::Account(MEMBER),
                runtime,
                clock: &SystemClock,
                spec: MultisigThreshold1::SPEC,
            })
            .await?;
        let decoded = decode(&built.call, runtime)?;
        assert_eq!(
            arg(arg(&decoded, "call")?, "address")?,
            &json!(format!("0x{}", hex::encode(&account.0[..20])))
        );
        assert_eq!(intent.spend(), rtao_primitives::Spend::None);
        assert!(intent.touches_netuids().is_empty());
        client.close().await;
        Ok(())
    }

    #[cfg(feature = "transfer")]
    #[tokio::test]
    async fn raw_multisig_build_checks_approval_member_deposit_plus_outer_fee_and_later_fee_only()
    -> anyhow::Result<()> {
        for (free, when, accepted) in [
            (126, None, false),
            (127, None, true),
            (6, Some(json!({"height":12,"index":3})), false),
            (7, Some(json!({"height":12,"index":3})), true),
        ] {
            let (chain, client, snapshot) = fixture(free).await?;
            let intent = MultisigApprove {
                threshold: 2,
                other_signatories: vec![json!(OTHER)],
                call: transfer(),
                timepoint: when.clone(),
            };
            let result = intent
                .build(&BuildCx {
                    view: &snapshot,
                    origin: OriginView::Account(MEMBER),
                    runtime: &snapshot.block().runtime,
                    clock: &SystemClock,
                    spec: MultisigApprove::SPEC,
                })
                .await;
            if accepted {
                assert!(result.is_ok());
            } else {
                let IntentError::Chain(error) = result.expect_err("shortfall must refuse") else {
                    return Err(anyhow::anyhow!("expected funding error"));
                };
                assert_eq!(error.code, rtao_catalog::ErrorCode::InsufficientBalance);
                assert!(error.message.contains(&MEMBER.to_string()));
                assert!(error.message.contains(if when.is_none() {
                    "deposit plus the transaction fee"
                } else {
                    "cannot cover the transaction fee"
                }));
                assert!(
                    error
                        .remediation()
                        .starts_with(&format!("fund {MEMBER} with ≥"))
                );
            }
            let nonce_requests: Vec<_> = chain
                .requests()
                .into_iter()
                .filter(|request| {
                    matches!(
                        request.method.as_str(),
                        "system_accountNextIndex" | "account_nextIndex"
                    )
                })
                .collect();
            assert!(nonce_requests.len() >= 2);
            assert!(
                nonce_requests
                    .iter()
                    .all(|request| request.params[0] == MEMBER.to_string())
            );
            assert!(chain.mutate_state(|state| state.submissions.is_empty()));
            client.close().await;
        }
        Ok(())
    }

    #[cfg(feature = "transfer")]
    #[tokio::test]
    async fn raw_multisig_opening_warning_and_preflight_keep_base_policy() -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture(0).await?;
        let runtime = &snapshot.block().runtime;
        let call = rtao_runtime::calls::system::remark(vec![]).compose(runtime)?;
        let open = MultisigExecute {
            threshold: 2,
            other_signatories: vec![json!(OTHER)],
            call: transfer(),
            timepoint: None,
        };
        let later = MultisigExecute {
            timepoint: Some(json!({"height":12,"index":3})),
            ..open.clone()
        };
        for (intent, warnings) in [
            (
                &open as &dyn DynIntent,
                vec![format!(
                    "the opening approval reserves a τ0.00000012 multisig deposit the signer cannot cover: fund {MEMBER} with ≥ τ0.006 — the deposit is returned when the op executes"
                )],
            ),
            (&later as &dyn DynIntent, vec![]),
        ] {
            let preview = intent
                .preflight_dyn(&PreflightCx {
                    view: &snapshot as &dyn DynView,
                    runtime,
                    clock: &SystemClock,
                    spec: intent.spec(),
                    client: &client,
                    dispatch_origin: MEMBER,
                    fee_payer: OTHER,
                    call: &call,
                })
                .await?;
            assert_eq!(preview.effects, vec![intent.summary_dyn()]);
            assert_eq!(preview.warnings, warnings);
            assert!(preview.blocks.is_empty());
            assert!(preview.facts.is_empty());
            assert!(preview.required_free.is_none());
            assert!(preview.available_free.is_none());
            assert!(preview.estimated_fee.is_none());
        }
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn raw_multisig_threshold_self_duplicate_and_error_order_refuse_before_signing()
    -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture(1_000).await?;
        let runtime = &snapshot.block().runtime;
        for (threshold,others,message) in [
            (1,vec![json!(OTHER)],"multisig threshold must be at least 2, got 1 (use multisig_threshold_1 for 1-of-N)".into()),
            (3,vec![json!(OTHER)],"multisig threshold 3 exceeds the signatory count 2 (signer plus other_signatories)".into()),
            (2,vec![json!(MEMBER)],format!("other_signatories must not include the signer's own account ({MEMBER}); list only the other members")),
        ] {
            let intent=MultisigApprove{threshold,other_signatories:others,call:json!({}),timepoint:None};
            intent.validate()?;
            let error=intent.build(&BuildCx{view:&snapshot,origin:OriginView::Account(MEMBER),runtime,clock:&SystemClock,spec:MultisigApprove::SPEC}).await.expect_err("invalid group");
            assert_eq!(error.to_string(),message);
        }
        for others in [
            vec![],
            vec![json!(OTHER), json!(OTHER)],
            vec![json!(OTHER), json!(OTHER.to_ss58(0))],
        ] {
            assert!(group_account(runtime, MEMBER, 1, &others).is_err());
        }
        let intent = MultisigExecute {
            threshold: 1,
            other_signatories: vec![json!(OTHER)],
            call: json!({}),
            timepoint: None,
        };
        assert_eq!(
            intent
                .build(&BuildCx {
                    view: &snapshot,
                    origin: OriginView::Account(MEMBER),
                    runtime,
                    clock: &SystemClock,
                    spec: MultisigExecute::SPEC
                })
                .await
                .expect_err("inner error precedes execute threshold validation")
                .to_string(),
            "multisig inner call needs an 'op' key"
        );
        assert_eq!(
            sorted_others(&[json!(AccountId([9; 32])), json!(OTHER), json!(OTHER)])?,
            vec![OTHER, AccountId([9; 32])]
        );
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn raw_multisig_rechecks_live_member_cap_before_nested_call_work() -> anyhow::Result<()> {
        let mut state = FakeState::try_default()?;
        state.seed_constant("Multisig", "MaxSignatories", json!(2))?;
        let chain = FakeChain::from_state(state);
        let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
        config.track_heads = false;
        config.runtime_cache_dir = None;
        config.config_store = None;
        let client = Client::from_rpc(RpcClient::new(chain.clone()), config).await?;
        let snapshot = client.at(BlockRef::Finalized).await?;
        let intent = MultisigThreshold1 {
            other_signatories: vec![json!(OTHER), json!(AccountId([9; 32]))],
            call: json!({}),
        };
        let requests = chain.requests().len();
        let error = intent
            .build(&BuildCx {
                view: &snapshot,
                origin: OriginView::Account(MEMBER),
                runtime: &snapshot.block().runtime,
                clock: &SystemClock,
                spec: MultisigThreshold1::SPEC,
            })
            .await
            .expect_err("cap rejects unusable account before nested op");
        assert!(error.to_string().contains("MaxSignatories (2)"));
        assert_eq!(chain.requests().len(), requests);
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        client.close().await;
        Ok(())
    }

    #[test]
    fn raw_multisig_timepoint_coercion_defaults_and_bounded_nested_inputs() -> anyhow::Result<()> {
        assert_eq!(
            timepoint(Some(
                &json!({"height":"١_٢","index":3.9,"unused":"retained in JSON only"})
            ))?,
            Some(MultisigTimepoint {
                height: 12,
                index: 3
            })
        );
        assert_eq!(
            timepoint(Some(&json!({"height":true,"index":false})))?,
            Some(MultisigTimepoint {
                height: 1,
                index: 0
            })
        );
        for value in [
            json!({}),
            json!({"height":1}),
            json!({"height":-1,"index":0}),
            json!({"height":4294967296u64,"index":0}),
            json!({"height":"12.0","index":0}),
            json!([]),
        ] {
            assert!(timepoint(Some(&value)).is_err());
        }
        let args = json!({"threshold":1,"other_signatories":[OTHER],"call":{}});
        let intent = crate::AnyIntent::from_tool("multisig_execute", args)?;
        assert_eq!(intent.to_json()["timepoint"], Value::Null);
        assert_eq!(
            intent.as_dyn().summary_dyn(),
            "multisig 1-of-2 execute None"
        );
        let malicious = json!({"$serde_json::private::RawValue":"{}"});
        for field in ["call", "other_signatories", "timepoint"] {
            let mut args =
                json!({"threshold":2,"other_signatories":[OTHER],"call":{},"timepoint":null});
            args[field] = if field == "other_signatories" {
                json!([malicious])
            } else {
                malicious.clone()
            };
            assert!(check_constructor_input(&args).is_err());
            assert!(crate::AnyIntent::from_tool("multisig_execute", args).is_err());
        }
        let mut deep = Value::Null;
        for _ in 0..128 {
            deep = json!([deep]);
        }
        assert!(check_inputs(&[json!(OTHER)], &deep, None).is_err());
        let invalid = MultisigThreshold1 {
            other_signatories: vec![json!(OTHER)],
            call: deep,
        };
        assert!(invalid.validate().is_err());
        assert_eq!(
            invalid.to_json(),
            json!({"op":"multisig_threshold_1","invalid_nested_input":true})
        );
        assert_eq!(
            invalid.summary(),
            "multisig (1-of-N) dispatch <invalid inner call>"
        );
        let cancel = json!({"threshold":2,"other_signatories":[OTHER],"call":{}});
        assert!(crate::AnyIntent::from_tool("multisig_cancel", cancel).is_err());
        for op in [
            "multisig_execute",
            "multisig_approve",
            "multisig_threshold_1",
        ] {
            let mut args = json!({"other_signatories":[OTHER],"call":{}});
            if op != "multisig_threshold_1" {
                args["threshold"] = json!(2);
            }
            assert!(crate::AnyIntent::from_tool(op, args.clone()).is_ok());
            args["unexpected"] = json!(true);
            assert!(crate::AnyIntent::from_tool(op, args).is_err());
        }
        Ok(())
    }

    #[cfg(feature = "root")]
    #[tokio::test]
    async fn raw_multisig_child_build_does_not_add_sudo_or_inherit_semantic_policy()
    -> anyhow::Result<()> {
        let (_chain, client, snapshot) = fixture(1_000).await?;
        let runtime = &snapshot.block().runtime;
        let intent = MultisigThreshold1 {
            other_signatories: vec![json!(OTHER)],
            call: json!({"op":"set_subnet_emission_enabled","netuids":[3],"enabled":true}),
        };
        let built = intent
            .build(&BuildCx {
                view: &snapshot,
                origin: OriginView::Account(MEMBER),
                runtime,
                clock: &SystemClock,
                spec: MultisigThreshold1::SPEC,
            })
            .await?;
        let decoded = runtime
            .core()
            .decode_spec(
                &rtao_core::runtime::TypeSpec::Call,
                built.call.as_bytes(),
                true,
            )
            .map_err(rtao_core::CoreError::from)?;
        let decoded = rtao_core::codec::value::to_corpus_json(&decoded);
        let inner = decoded["call_args"]
            .as_array()
            .and_then(|args| args.iter().find(|arg| arg["name"] == "call"))
            .map(|arg| &arg["value"])
            .ok_or_else(|| anyhow::anyhow!("missing inner"))?;
        assert_eq!(inner["call_module"], "AdminUtils");
        assert_eq!(inner["call_function"], "sudo_set_subnet_emission_enabled");
        assert_eq!(intent.spend(), rtao_primitives::Spend::None);
        assert!(intent.touches_netuids().is_empty());
        assert!(!intent.affects_all_subnets());
        assert!(built.extras.is_empty());
        client.close().await;
        Ok(())
    }

    #[cfg(feature = "transfer")]
    #[tokio::test]
    async fn raw_multisig_funding_probe_failure_is_best_effort_after_required_weight_probe()
    -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture(0).await?;
        let runtime = &snapshot.block().runtime;
        let info = rtao_core::codec::Value::record(vec![
            (
                "weight".into(),
                rtao_core::codec::Value::record(vec![
                    ("ref_time".into(), rtao_core::codec::Value::Uint(1)),
                    ("proof_size".into(), rtao_core::codec::Value::Uint(2)),
                ]),
            ),
            (
                "class".into(),
                rtao_core::codec::Value::Str("Normal".into()),
            ),
            ("partial_fee".into(), rtao_core::codec::Value::Uint(7)),
        ]);
        let method = rtao_core::ops::runtime_api::runtime_api_method(
            runtime.core(),
            "TransactionPaymentApi",
            "query_info",
        )?;
        let encoded = runtime
            .core()
            .encode_spec(&rtao_core::runtime::TypeSpec::Id(method.output), &info)
            .map_err(rtao_core::CoreError::from)?;
        chain.mutate_state(|state| {
            state.queue_reply(
                "state_call",
                rtao_testkit::Reply::result(json!(format!("0x{}", hex::encode(encoded)))),
            );
            state.queue_reply(
                "state_call",
                rtao_testkit::Reply::error(-32603, "fee estimate unavailable", None),
            );
        });
        let intent = MultisigApprove {
            threshold: 2,
            other_signatories: vec![json!(OTHER)],
            call: transfer(),
            timepoint: None,
        };
        // Connection setup also calls Metadata_metadata_at_version. Count
        // only this build's business probes, keeping their order observable.
        let before = chain.requests().len();
        let recorded = RecordingView::new(snapshot.clone());
        let built = intent
            .build(&BuildCx {
                view: &recorded,
                origin: OriginView::Account(MEMBER),
                runtime,
                clock: &SystemClock,
                spec: MultisigApprove::SPEC,
            })
            .await?;
        assert_eq!(
            arg(&decode(&built.call, runtime)?, "max_weight")?,
            &json!({"ref_time":1,"proof_size":2})
        );
        let requests = chain.requests();
        let probes: Vec<_> = requests[before..]
            .iter()
            .filter(|request| {
                request.method == "state_call"
                    && request.params[0] == "TransactionPaymentApi_query_info"
            })
            .collect();
        assert_eq!(probes.len(), 2);
        // Python first requires the inner call's dispatch weight, then makes
        // a best-effort fee probe for the composed outer approval. Check both
        // probe payloads, retaining independent SCALE length/call assertions.
        let inner_bytes = transfer_bytes();
        for (probe, call) in probes
            .into_iter()
            .zip([inner_bytes.as_slice(), built.call.as_bytes()])
        {
            let bytes = hex::decode(
                probe.params[1]
                    .as_str()
                    .and_then(|value| value.strip_prefix("0x"))
                    .ok_or_else(|| anyhow::anyhow!("fee probe omitted hexadecimal input"))?,
            )?;
            let length_at = bytes
                .len()
                .checked_sub(4)
                .ok_or_else(|| anyhow::anyhow!("fee probe omitted extrinsic length"))?;
            let (extrinsic, length) = bytes.split_at(length_at);
            assert_eq!(length, u32::try_from(extrinsic.len())?.to_le_bytes());
            assert!(extrinsic.ends_with(call));
            assert_eq!(probe.params[2], Value::Null);
        }
        // The failed outer fee estimate skips the funding check even with
        // an empty signatory balance; it must not query System.Account.
        assert!(
            recorded
                .touches()
                .iter()
                .all(|touch| { touch.pallet_or_api != "System" || touch.item != "Account" })
        );
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        client.close().await;
        Ok(())
    }
}
