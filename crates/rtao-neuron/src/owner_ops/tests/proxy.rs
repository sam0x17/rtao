//! Effective owner, permissions, fee source and typed proxy composition.

use super::*;
use rtao_core::{codec::Value as CoreValue, value_ext::ValueExt};
use rtao_runtime::models::ProxyType;

pub(crate) const REAL: AccountId = AccountId([9; 32]);

pub(crate) fn seed_proxy(
    state: &mut FakeState,
    delegate: AccountId,
    proxy_type: ProxyType,
    delay: u32,
    calls: Option<&[(&str, &str)]>,
) -> anyhow::Result<()> {
    state.seed_python(
        "Proxy",
        "Proxies",
        &[json!(REAL)],
        json!([[{
        "delegate":delegate,"proxy_type":proxy_type,"delay":delay
    }],0]),
    )?;
    let filter = filter_row(state, proxy_type, calls)?;
    state.seed_runtime(
        "ProxyFilterRuntimeApi",
        "get_proxy_filters",
        json!([filter]),
    )?;
    Ok(())
}

fn filter_row(
    state: &FakeState,
    proxy_type: ProxyType,
    calls: Option<&[(&str, &str)]>,
) -> anyhow::Result<Value> {
    let runtime = rtao_runtime::RuntimeHandle::parse(
        &state.runtime.metadata_bytes,
        state.runtime.spec_version,
        state.runtime.transaction_version,
        state.runtime.ss58_format,
    )?;
    let mode = if let Some(calls) = calls {
        json!({"Allow":calls.iter().map(|(pallet, call)| json!({
            "pallet_name":pallet.as_bytes(),"call_name":call.as_bytes(),
            "pallet_index":runtime.pallet_index(pallet),
            "call_index":runtime.call_index(pallet, call),"constraint":null
        })).collect::<Vec<_>>()})
    } else {
        json!("AllowAll")
    };
    // Resolve the bare category through a separate live call field, independent
    // of production's Option<ProxyType> field encoding.
    let ir = rtao_core_ext::ir::MetadataIr::from_runtime(&state.runtime)?;
    let type_id = ir
        .pallets
        .iter()
        .find(|p| p.name == "Proxy")
        .and_then(|p| p.calls.iter().find(|c| c.name == "add_proxy"))
        .and_then(|c| c.args.iter().find(|a| a.name == "proxy_type"))
        .map(|a| a.type_id)
        .ok_or_else(|| anyhow::anyhow!("proxy_type field"))?;
    let encoded_type = state.runtime.encode_spec(
        &rtao_core::runtime::TypeSpec::Id(type_id),
        &CoreValue::Str(format!("{proxy_type:?}")),
    )?;
    assert_eq!(encoded_type.len(), 1);
    if proxy_type == ProxyType::Owner {
        assert_eq!(encoded_type[0], 1);
    }
    Ok(json!({
    "proxy_type":encoded_type[0],"name":format!("{proxy_type:?}").as_bytes(),
        "deprecated":false,"filter_mode":mode
    }))
}

pub(super) async fn proxied(
    proxy_type: ProxyType,
) -> anyhow::Result<(OwnerOps<Keys>, FakeChain, Arc<Keys>)> {
    let (ops, chain, keys) = fixture().await?;
    chain.mutate_state(|s| -> anyhow::Result<()> {
        s.seed(&st::SUBNET_OWNER, (NET,), DefaultAccount::from(REAL))?;
        seed_proxy(s, keys.signer.account(), proxy_type, 0, None)?;
        Ok(())
    })?;
    Ok((
        ops.with_execute_options(ExecuteOptions {
            proxy_for: Some(REAL),
            force_proxy_type: Some(proxy_type),
            reconcile: false,
            ..Default::default()
        })?,
        chain,
        keys,
    ))
}

pub(crate) fn executed() -> Value {
    json!({"phase":"ApplyExtrinsic","extrinsic_idx":0,"module_id":"Proxy",
        "event_id":"ProxyExecuted","attributes":{"result":{"Ok":[]}},"topics":[]})
}

fn queue_proxy(chain: &FakeChain) {
    chain.mutate_state(|s| {
        s.queue_submit(SubmitScript::InBlockThenFinalized {
            events: json!([executed(), success()]),
        })
    });
}

#[tokio::test]
async fn proxy_hparam_signs_exact_typed_call_with_delegate_fee_source() -> anyhow::Result<()> {
    let (ops, chain, keys) = proxied(ProxyType::Owner).await?;
    chain.mutate_state(|s| {
        seed_proxy(
            s,
            keys.signer.account(),
            ProxyType::Owner,
            0,
            Some(&[("AdminUtils", "sudo_set_rho")]),
        )
    })?;
    queue_proxy(&chain);
    let result = ops
        .set_hparam(NET, "rho", 1u16.into(), WindowWait::NoWait)
        .await?;
    assert!(result.success, "{result:?}");
    let runtime = ops.client().refresh_runtime().await?;
    let inner = rtao_runtime::calls::admin_utils::sudo_set_rho(NET, 1).compose(&runtime)?;
    let expected = rtao_client::compose::proxy(&runtime, REAL, Some(ProxyType::Owner), inner)?;
    assert_eq!(
        keys.signer
            .calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)[0],
        expected.as_bytes()
    );
    let submissions = chain.mutate_state(|s| s.submissions.clone());
    let signed = runtime.core().decode_extrinsic(&submissions[0], true)?;
    assert_eq!(
        signed
            .field("address")
            .and_then(ValueExt::as_str)
            .and_then(|s| s.parse::<AccountId>().ok()),
        Some(keys.signer.account())
    );
    let estimates = chain
        .requests()
        .into_iter()
        .filter(|r| r.method == "state_call" && r.params[0] == "TransactionPaymentApi_query_info")
        .collect::<Vec<_>>();
    assert!(!estimates.is_empty());
    for request in estimates {
        let encoded = request.params[1]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("estimate hex"))?;
        let bytes = hex::decode(encoded.trim_start_matches("0x"))?;
        // Fee API arguments are the complete probe followed by a u32 length.
        let decoded = runtime
            .core()
            .decode_extrinsic(&bytes[..bytes.len() - 4], true)?;
        assert_eq!(
            decoded
                .field("address")
                .and_then(ValueExt::as_str)
                .and_then(|s| s.parse::<AccountId>().ok()),
            Some(keys.signer.account())
        );
    }
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 1);
    ops.client().close().await;
    Ok(())
}

#[tokio::test]
async fn proxy_planner_reads_effective_owner_transaction_cooldown() -> anyhow::Result<()> {
    let (ops, chain, keys) = proxied(ProxyType::Owner).await?;
    chain.mutate_state(|s| s.seed(&st::TRANSACTION_KEY_LAST_BLOCK, (REAL, NET, 11u16), 100u64))?;
    let error = ops
        .set_hparam(NET, "tempo", 400u16.into(), WindowWait::NoWait)
        .await
        .err()
        .ok_or_else(|| anyhow::anyhow!("expected preflight refusal"))?;
    assert!(error.to_string().contains("cooldown"), "{error}");
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    chain.mutate_state(|s| -> anyhow::Result<()> {
        s.seed(&st::TRANSACTION_KEY_LAST_BLOCK, (REAL, NET, 11u16), 0u64)?;
        s.seed(
            &st::TRANSACTION_KEY_LAST_BLOCK,
            (keys.signer.account(), NET, 11u16),
            100u64,
        )?;
        Ok(())
    })?;
    queue_proxy(&chain);
    assert!(
        ops.set_hparam(NET, "tempo", 400u16.into(), WindowWait::NoWait)
            .await?
            .success
    );
    ops.client().close().await;
    Ok(())
}

#[tokio::test]
async fn proxy_wrong_owner_missing_exact_permission_delay_and_filter_refuse_before_unlock()
-> anyhow::Result<()> {
    for case in 0..5 {
        let (ops, chain, keys) = proxied(ProxyType::Owner).await?;
        chain.mutate_state(|s| -> anyhow::Result<()> {
            match case {
                0 => s.seed(
                    &st::SUBNET_OWNER,
                    (NET,),
                    DefaultAccount::from(keys.signer.account()),
                )?,
                1 => seed_proxy(s, HOTKEY, ProxyType::Owner, 0, None)?,
                2 => seed_proxy(s, keys.signer.account(), ProxyType::Any, 0, None)?,
                3 => seed_proxy(s, keys.signer.account(), ProxyType::Owner, 1, None)?,
                _ => seed_proxy(s, keys.signer.account(), ProxyType::Owner, 0, Some(&[]))?,
            }
            Ok(())
        })?;
        assert!(ops.update_symbol(NET, "a").await.is_err(), "case {case}");
        assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0, "case {case}");
        assert!(chain.mutate_state(|s| s.submissions.is_empty()));
        ops.client().close().await;
    }
    Ok(())
}

#[tokio::test]
async fn proxy_retains_policy_and_freeze_checks_and_rejects_orphan_force() -> anyhow::Result<()> {
    let (ops, chain, keys) = proxied(ProxyType::Any).await?;
    assert!(
        ops.clone()
            .with_execute_options(ExecuteOptions {
                force_proxy_type: Some(ProxyType::Any),
                ..Default::default()
            })
            .is_err()
    );
    let mut restricted = ops.options.clone();
    restricted.policy = Some(rtao_tx::Policy {
        allowed_netuids: Some(Default::default()),
        ..Default::default()
    });
    assert!(
        ops.clone()
            .with_execute_options(restricted)?
            .update_symbol(NET, "a")
            .await
            .is_err()
    );
    chain.mutate_state(|s| s.seed(&st::PENDING_EPOCH_AT, (NET,), 110u64))?;
    assert!(
        ops.set_hparam(NET, "rho", 1u16.into(), WindowWait::NoWait)
            .await
            .is_err()
    );
    assert!(
        ops.set_hparam(NET, "tx_rate_limit", 0u16.into(), WindowWait::NoWait)
            .await
            .is_err()
    );
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    assert!(chain.mutate_state(|s| s.submissions.is_empty()));
    ops.client().close().await;
    Ok(())
}

#[tokio::test]
async fn proxy_owner_helpers_keep_their_typed_inner_calls() -> anyhow::Result<()> {
    let (ops, chain, keys) = proxied(ProxyType::Any).await?;
    chain.mutate_state(|s| -> anyhow::Result<()> {
        s.seed(&st::MAX_MECHANISM_COUNT, (), MechId(2))?;
        s.seed(&st::MIN_ALLOWED_UIDS, (NET,), 1u16)?;
        s.seed(&st::MAX_ALLOWED_UIDS, (NET,), 128u16)?;
        s.seed(&st::NETWORK_REGISTERED_AT, (NET,), 1u64)?;
        s.seed(&st::START_CALL_DELAY, (), 1u64)?;
        Ok(())
    })?;
    queue_proxy(&chain);
    assert!(
        ops.set_identity(
            NET,
            SubnetIdentity {
                subnet_name: "proxy".into(),
                ..Default::default()
            }
        )
        .await?
        .success
    );
    queue_proxy(&chain);
    assert!(ops.update_symbol(NET, "a").await?.success);
    queue_proxy(&chain);
    assert!(ops.set_mechanism_count(NET, 2).await?.success);
    queue_proxy(&chain);
    assert!(ops.trim(NET, 100).await?.success);
    queue_proxy(&chain);
    assert!(ops.start(NET).await?.success);
    let runtime = ops.client().refresh_runtime().await?;
    let calls = keys
        .signer
        .calls
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    for (encoded, (pallet, function)) in calls.iter().zip([
        ("SubtensorModule", "set_subnet_identity"),
        ("SubtensorModule", "update_symbol"),
        ("AdminUtils", "sudo_set_mechanism_count"),
        ("AdminUtils", "sudo_trim_to_max_allowed_uids"),
        ("SubtensorModule", "start_call"),
    ]) {
        let call =
            runtime
                .core()
                .decode_spec(&rtao_core::runtime::TypeSpec::Call, encoded, true)?;
        assert_eq!(
            call.field("call_module").and_then(ValueExt::as_str),
            Some("Proxy")
        );
        let CoreValue::List(args) = call
            .field("call_args")
            .ok_or_else(|| anyhow::anyhow!("args"))?
        else {
            anyhow::bail!("args list")
        };
        let inner = args
            .iter()
            .find(|a| a.field("name").and_then(ValueExt::as_str) == Some("call"))
            .and_then(|a| a.field("value"))
            .ok_or_else(|| anyhow::anyhow!("inner"))?;
        assert_eq!(
            inner.field("call_module").and_then(ValueExt::as_str),
            Some(pallet)
        );
        assert_eq!(
            inner.field("call_function").and_then(ValueExt::as_str),
            Some(function)
        );
    }
    assert_eq!(calls.len(), 5);
    ops.client().close().await;
    Ok(())
}

#[tokio::test]
async fn proxy_admission_is_rechecked_by_executor_after_public_review() -> anyhow::Result<()> {
    let (ops, chain, keys) = proxied(ProxyType::Owner).await?;
    let intent = UpdateSymbol {
        netuid: NET,
        symbol: "a".into(),
    };
    ops.review(&intent).await?;
    chain.mutate_state(|s| seed_proxy(s, HOTKEY, ProxyType::Owner, 0, None))?;
    assert!(
        ops.operation()?
            .execute(&intent, &ops.options)
            .await
            .is_err()
    );
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    assert!(chain.mutate_state(|s| s.submissions.is_empty()));
    ops.client().close().await;
    Ok(())
}

#[tokio::test]
async fn unforced_proxy_selects_first_grant_while_force_selects_exact_type() -> anyhow::Result<()> {
    let (ops, chain, keys) = proxied(ProxyType::Any).await?;
    chain.mutate_state(|s| {
        s.seed_python(
            "Proxy",
            "Proxies",
            &[json!(REAL)],
            json!([[{
        "delegate":keys.signer.account(),"proxy_type":"Owner","delay":1
    }, {"delegate":keys.signer.account(),"proxy_type":"Any","delay":0}],0]),
        )
    })?;
    let mut options = ops.options.clone();
    options.force_proxy_type = None;
    assert!(
        ops.clone()
            .with_execute_options(options)?
            .update_symbol(NET, "a")
            .await
            .is_err()
    );
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    queue_proxy(&chain);
    assert!(ops.update_symbol(NET, "a").await?.success);
    chain.mutate_state(|s| seed_proxy(s, keys.signer.account(), ProxyType::Owner, 0, None))?;
    let mut unforced = ops.options.clone();
    unforced.force_proxy_type = None;
    queue_proxy(&chain);
    assert!(
        ops.clone()
            .with_execute_options(unforced)?
            .update_symbol(NET, "b")
            .await?
            .success
    );
    ops.client().close().await;
    Ok(())
}

#[tokio::test]
async fn proxy_inner_failure_overrides_outer_success_without_retry() -> anyhow::Result<()> {
    let (ops, chain, _) = proxied(ProxyType::Any).await?;
    let mut event = executed();
    event["attributes"]["result"] = json!({"Err":"BadOrigin"});
    chain.mutate_state(|s| {
        s.queue_submit(SubmitScript::InBlockThenFinalized {
            events: json!([event, success()]),
        })
    });
    let result = ops
        .set_hparam(NET, "rho", 1u16.into(), WindowWait::NoWait)
        .await?;
    assert!(!result.success);
    assert_eq!(
        result.error.and_then(|e| e.name).as_deref(),
        Some("BadOrigin")
    );
    assert_eq!(chain.mutate_state(|s| s.submissions.len()), 1);
    ops.client().close().await;
    Ok(())
}

#[tokio::test]
async fn owner_filter_refuses_activation_and_creation_before_unlock() -> anyhow::Result<()> {
    let (ops, chain, keys) = proxied(ProxyType::Owner).await?;
    chain.mutate_state(|s| -> anyhow::Result<()> {
        seed_proxy(
            s,
            keys.signer.account(),
            ProxyType::Owner,
            0,
            Some(&[
                ("SubtensorModule", "update_symbol"),
                ("AdminUtils", "sudo_set_rho"),
            ]),
        )?;
        s.seed(&st::NETWORK_REGISTERED_AT, (NET,), 1u64)?;
        s.seed(&st::START_CALL_DELAY, (), 1u64)?;
        Ok(())
    })?;
    let start = ops
        .start(NET)
        .await
        .err()
        .ok_or_else(|| anyhow::anyhow!("expected preflight refusal"))?;
    assert!(start.to_string().contains("does not admit"), "{start}");
    let create = ops
        .create_subnet(HOTKEY, RegistrationWait::Receipt)
        .await
        .err()
        .ok_or_else(|| anyhow::anyhow!("expected preflight refusal"))?;
    assert!(create.to_string().contains("does not admit"), "{create}");
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    assert!(chain.mutate_state(|s| s.submissions.is_empty()));
    ops.client().close().await;
    Ok(())
}

#[tokio::test]
async fn proxy_postbroadcast_cancellation_drains_exact_attempted_hash() -> anyhow::Result<()> {
    let (ops, chain, keys) = proxied(ProxyType::Owner).await?;
    chain.mutate_state(|s| s.queue_submit(SubmitScript::Statuses(vec![])));
    let cancel = CancellationToken::new();
    let pending = ops
        .clone()
        .with_cancellation(cancel.clone())
        .with_config(OwnerConfig {
            operation_timeout: Some(Duration::from_millis(300)),
            ..Default::default()
        })?;
    let task = tokio::spawn(async move {
        pending
            .set_hparam(NET, "rho", 1u16.into(), WindowWait::NoWait)
            .await
    });
    tokio::time::timeout(Duration::from_secs(1), async {
        while chain.mutate_state(|s| s.submissions.is_empty()) {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    cancel.cancel();
    let result = tokio::time::timeout(Duration::from_secs(1), task).await???;
    assert!(!result.success);
    assert_eq!(result.fate_unknown, Some(attempted_hash(&chain)));
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 1);
    ops.client().close().await;
    Ok(())
}

#[tokio::test]
async fn live_filter_parameter_constraint_has_a_strict_encoded_call_boundary() -> anyhow::Result<()>
{
    let (ops, chain, keys) = proxied(ProxyType::Owner).await?;
    chain.mutate_state(|s| -> anyhow::Result<()> {
        let mut row = filter_row(s, ProxyType::Owner, Some(&[("AdminUtils", "sudo_set_rho")]))?;
        row["filter_mode"]["Allow"][0]["constraint"] = json!({"ParamLessThan":{
            "param_name":b"rho", "limit":2
        }});
        s.seed_runtime("ProxyFilterRuntimeApi", "get_proxy_filters", json!([row]))?;
        Ok(())
    })?;
    for value in [2u16, 3] {
        let error = ops
            .set_hparam(NET, "rho", value.into(), WindowWait::NoWait)
            .await
            .err()
            .ok_or_else(|| anyhow::anyhow!("expected preflight refusal"))?;
        assert!(error.to_string().contains("does not admit"), "{error}");
    }
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    queue_proxy(&chain);
    assert!(
        ops.set_hparam(NET, "rho", 1u16.into(), WindowWait::NoWait)
            .await?
            .success
    );
    let runtime = ops.client().refresh_runtime().await?;
    let inner = rtao_runtime::calls::admin_utils::sudo_set_rho(NET, 1).compose(&runtime)?;
    let wrapped = rtao_client::compose::proxy(&runtime, REAL, Some(ProxyType::Owner), inner)?;
    assert_eq!(
        keys.signer
            .calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)[0],
        wrapped.as_bytes()
    );
    ops.client().close().await;
    Ok(())
}

// Test-only typed call for the runtime's existing SudoUncheckedSetCode constraint.
// Production OwnerOps methods continue to accept only their own typed intents.
#[derive(Clone)]
struct NestedCall {
    set_code: bool,
}
impl rtao_tx::IntentInterface for NestedCall {
    const SPEC: &'static rtao_catalog::IntentSpec =
        <UpdateSymbol as rtao_tx::IntentInterface>::SPEC;
    type Role = Coldkey;
    type Origin = rtao_tx::SubnetOwner;
    type Output = ();
    const WRAPS: &'static [&'static rtao_runtime::CallDescriptor] =
        &[&rtao_runtime::calls::sudo::SUDO_UNCHECKED_WEIGHT];
    fn to_json(&self) -> Value {
        json!({"netuid":NET.0})
    }
}
impl Intent for NestedCall {
    fn summary(&self) -> String {
        "test nested filter admission".into()
    }
    async fn build<V: rtao_client::View + ?Sized>(
        &self,
        cx: &rtao_tx::BuildCx<'_, V>,
    ) -> Result<rtao_tx::Built, IntentError> {
        let child = if self.set_code {
            rtao_runtime::calls::system::set_code(vec![1])
        } else {
            rtao_runtime::calls::system::remark(vec![1])
        };
        let child = child.compose(cx.runtime)?;
        let weight = rtao_core::DynValue::new(CoreValue::record(vec![
            ("ref_time".into(), CoreValue::Uint(0)),
            ("proof_size".into(), CoreValue::Uint(0)),
        ]));
        Ok(rtao_tx::Built::new(
            rtao_runtime::calls::sudo::sudo_unchecked_weight(child, weight).compose(cx.runtime)?,
        ))
    }
}

#[tokio::test]
async fn live_filter_nested_constraint_checks_the_actual_encoded_child() -> anyhow::Result<()> {
    let (ops, chain, keys) = proxied(ProxyType::SudoUncheckedSetCode).await?;
    chain.mutate_state(|s| -> anyhow::Result<()> {
        let mut row = filter_row(
            s,
            ProxyType::SudoUncheckedSetCode,
            Some(&[("Sudo", "sudo_unchecked_weight")]),
        )?;
        row["filter_mode"]["Allow"][0]["constraint"] = json!({"NestedCallMustBe":{
            "param_name":b"call", "pallet_name":b"System", "call_name":b"set_code"
        }});
        s.seed_runtime("ProxyFilterRuntimeApi", "get_proxy_filters", json!([row]))?;
        Ok(())
    })?;
    let wrong = NestedCall { set_code: false };
    assert!(
        ops.operation()?
            .execute(&wrong, &ops.options)
            .await
            .is_err()
    );
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    assert!(chain.mutate_state(|s| s.submissions.is_empty()));
    queue_proxy(&chain);
    let right = NestedCall { set_code: true };
    assert!(
        ops.operation()?
            .execute(&right, &ops.options)
            .await?
            .success
    );
    let runtime = ops.client().refresh_runtime().await?;
    let child = rtao_runtime::calls::system::set_code(vec![1]).compose(&runtime)?;
    let weight = rtao_core::DynValue::new(CoreValue::record(vec![
        ("ref_time".into(), CoreValue::Uint(0)),
        ("proof_size".into(), CoreValue::Uint(0)),
    ]));
    let inner =
        rtao_runtime::calls::sudo::sudo_unchecked_weight(child, weight).compose(&runtime)?;
    let expected =
        rtao_client::compose::proxy(&runtime, REAL, Some(ProxyType::SudoUncheckedSetCode), inner)?;
    assert_eq!(
        keys.signer
            .calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)[0],
        expected.as_bytes()
    );
    ops.client().close().await;
    Ok(())
}

#[tokio::test]
async fn inconsistent_missing_or_unknown_filter_metadata_refuses_before_unlock()
-> anyhow::Result<()> {
    for case in 0..6 {
        let (ops, chain, keys) = proxied(ProxyType::Owner).await?;
        chain.mutate_state(|s| -> anyhow::Result<()> {
            let mut row = filter_row(
                s,
                ProxyType::Owner,
                Some(&[("SubtensorModule", "update_symbol")]),
            )?;
            match case {
                0 => row["proxy_type"] = json!(0),
                1 => row["filter_mode"]["Allow"][0]["call_index"] = json!(0),
                2 => row["filter_mode"]["Allow"][0]["pallet_index"] = json!(0),
                3 => row["name"] = json!(b"UnknownType"),
                _ => {}
            }
            let rows = match case {
                4 => json!([]),
                5 => json!([row.clone(), row]),
                _ => json!([row]),
            };
            s.seed_runtime("ProxyFilterRuntimeApi", "get_proxy_filters", rows)?;
            Ok(())
        })?;
        assert!(ops.update_symbol(NET, "a").await.is_err(), "case {case}");
        assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0, "case {case}");
        assert!(chain.mutate_state(|s| s.submissions.is_empty()));
        ops.client().close().await;
    }
    Ok(())
}
