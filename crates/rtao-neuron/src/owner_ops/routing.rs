//! Proxy admission without changing the semantic intent or signing key.

use rtao_client::{View, ViewExt};
use rtao_core::{codec::Value, value_ext::ValueExt};
use rtao_primitives::AccountId;
use rtao_runtime::{models::ProxyType, runtime_apis::proxy_filter_runtime_api, storage::proxy};
use rtao_signer::KeyView;
use rtao_tx::{
    BuildCx, Built, DynIntent, Intent, IntentError, IntentInterface, IntentPreflight, OriginView,
    PreflightCx,
};

#[derive(Clone)]
pub(super) struct Routed<I> {
    pub intent: I,
    pub proxy_for: Option<AccountId>,
    pub force_proxy_type: Option<ProxyType>,
}

impl<I: Intent> IntentInterface for Routed<I> {
    const SPEC: &'static rtao_catalog::IntentSpec = I::SPEC;
    type Role = I::Role;
    type Origin = I::Origin;
    type Output = I::Output;
    const WRAPS: &'static [&'static rtao_runtime::CallDescriptor] = I::WRAPS;
    fn to_json(&self) -> serde_json::Value {
        self.intent.to_json()
    }
}

impl<I: Intent> Intent for Routed<I> {
    fn summary(&self) -> String {
        self.intent.summary()
    }
    fn semantic_intent(&self) -> &dyn DynIntent {
        self.intent.semantic_intent()
    }
    fn validate(&self) -> Result<(), IntentError> {
        self.intent.validate()
    }
    fn inner_call_data(&self) -> Option<&rtao_runtime::CallBytes> {
        self.intent.inner_call_data()
    }
    fn origin_view<'a>(&'a self, keys: &'a dyn KeyView) -> OriginView<'a> {
        self.intent.origin_view(keys)
    }
    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        self.intent.build(cx).await
    }
    async fn wrap_call<V: View + ?Sized>(
        &self,
        cx: &BuildCx<'_, V>,
        call: rtao_runtime::CallBytes,
    ) -> Result<Built, IntentError> {
        self.intent.wrap_call(cx, call).await
    }
    async fn preflight<V: View + ?Sized>(
        &self,
        cx: &PreflightCx<'_, V>,
    ) -> Result<IntentPreflight, IntentError> {
        let preview = self.intent.preflight(cx).await?;
        if let Some(real) = self.proxy_for {
            admit_proxy(cx, real, self.force_proxy_type).await?;
        }
        // Retain the original intent's funds, warnings and hard stops.
        Ok(preview)
    }
}

// Pinned pallet-proxy::find_proxy selects the first delegate/exact-type match,
// then proxy() requires zero delay. Do not search for a later, broader grant.
async fn admit_proxy<V: View + ?Sized>(
    cx: &PreflightCx<'_, V>,
    real: AccountId,
    force: Option<ProxyType>,
) -> Result<(), IntentError> {
    let at = cx.view.pin().await?;
    // Reuse the authenticated descriptor with a dynamic element projection:
    // ProxyDefinition is not a client ReadValue in this source boundary.
    let descriptor = rtao_runtime::StorageDescriptor::<
        (AccountId,),
        (Vec<rtao_core::DynValue>, rtao_primitives::TaoBalance),
    >::new(proxy::PROXIES.surface());
    let (definitions, _) = at.storage(&descriptor, (real,)).await?;
    let forced = force.map(|t| format!("{t:?}"));
    let definition = definitions
        .iter()
        .map(rtao_core::DynValue::as_core)
        .find(|d| {
            d.field("delegate")
                .and_then(ValueExt::as_str)
                .and_then(|s| s.parse::<AccountId>().ok())
                == Some(cx.fee_payer)
                && forced.as_ref().is_none_or(|t| {
                    d.field("proxy_type")
                        .and_then(ValueExt::variant_name)
                        .as_ref()
                        == Some(t)
                })
        })
        .ok_or_else(|| invalid("signing delegate has no matching proxy permission"))?;
    if definition.field("delay").and_then(ValueExt::as_u128) != Some(0) {
        return Err(invalid("proxy permission requires an announced call"));
    }
    let filters = at
        .call(&proxy_filter_runtime_api::GET_PROXY_FILTERS, (None,))
        .await?;
    let name = definition
        .field("proxy_type")
        .and_then(ValueExt::variant_name)
        .ok_or_else(|| invalid("invalid proxy type"))?;
    let mut matches = filters.iter().filter(|filter| {
        filter
            .as_core()
            .field("name")
            .and_then(ValueExt::value_bytes)
            .as_deref()
            == Some(name.as_bytes())
    });
    let filter = matches
        .next()
        .ok_or_else(|| invalid("proxy filter metadata missing"))?;
    if matches.next().is_some() {
        return Err(invalid("ambiguous proxy filter metadata"));
    }
    let expected_type = proxy_type_index(cx.runtime, &name)?;
    if filter
        .as_core()
        .field("proxy_type")
        .and_then(ValueExt::as_u128)
        != Some(u128::from(expected_type))
    {
        return Err(invalid("inconsistent proxy filter type identity"));
    }
    let decoded = cx
        .runtime
        .core()
        .decode_spec(
            &rtao_core::runtime::TypeSpec::Call,
            cx.call.as_bytes(),
            true,
        )
        .map_err(rtao_core::CoreError::from)?;
    // Executor composes Proxy outside semantic/Sudo calls. Check that exact
    // inner call against the live runtime API, including call constraints.
    let inner = argument(&decoded, "call")
        .filter(|_| call_name(&decoded) == Some(("Proxy", "proxy")))
        .ok_or_else(|| invalid("owner proxy composition is missing"))?;
    let mode = filter
        .as_core()
        .field("filter_mode")
        .ok_or_else(|| invalid("proxy filter mode missing"))?;
    let allowed = match mode.variant_name().as_deref() {
        Some("AllowAll") => true,
        Some("Allow") => {
            let calls = mode
                .variant("Allow")
                .and_then(sequence)
                .ok_or_else(|| invalid("invalid proxy call allowlist"))?;
            if calls
                .iter()
                .any(|entry| !authenticated_call_identity(cx.runtime, entry))
            {
                return Err(invalid("inconsistent proxy filter call identity"));
            }
            calls.iter().any(|entry| permits(entry, inner))
        }
        _ => false,
    };
    if !allowed {
        return Err(invalid("proxy permission does not admit this owner call"));
    }
    Ok(())
}

// Enum names are deliberately absent from the core short-name map. Resolve the
// actual composed proxy field through the existing metadata IR instead.
fn proxy_type_index(runtime: &rtao_runtime::RuntimeHandle, name: &str) -> Result<u8, IntentError> {
    let ir = rtao_core_ext::ir::MetadataIr::from_runtime(runtime.core())
        .map_err(rtao_core::CoreError::from)?;
    let field = ir
        .pallets
        .iter()
        .find(|p| p.name == "Proxy")
        .and_then(|p| p.calls.iter().find(|c| c.name == "proxy"))
        .and_then(|c| c.args.iter().find(|a| a.name == "force_proxy_type"))
        .ok_or_else(|| invalid("proxy permission field metadata missing"))?;
    let encoded = runtime
        .core()
        .encode_spec(
            &rtao_core::runtime::TypeSpec::Id(field.type_id),
            &Value::Str(name.into()),
        )
        .map_err(rtao_core::CoreError::from)?;
    // Authenticated force_proxy_type is Option<unit enum>: Some tag, then index.
    match encoded.as_slice() {
        [1, index] => Ok(*index),
        _ => Err(invalid("invalid proxy permission field encoding")),
    }
}

fn authenticated_call_identity(runtime: &rtao_runtime::RuntimeHandle, entry: &Value) -> bool {
    let Some(pallet) = entry.field("pallet_name").and_then(ValueExt::value_bytes) else {
        return false;
    };
    let Some(function) = entry.field("call_name").and_then(ValueExt::value_bytes) else {
        return false;
    };
    let (Ok(pallet), Ok(function)) = (std::str::from_utf8(&pallet), std::str::from_utf8(&function))
    else {
        return false;
    };
    matches!((runtime.pallet_index(pallet), runtime.call_index(pallet, function)), (Some(p), Some(c))
        if entry.field("pallet_index").and_then(ValueExt::as_u128) == Some(u128::from(p))
            && entry.field("call_index").and_then(ValueExt::as_u128) == Some(u128::from(c)))
}

fn sequence(value: &Value) -> Option<&[Value]> {
    match value {
        Value::Tuple(values) | Value::List(values) => Some(values),
        _ => None,
    }
}
fn call_name(call: &Value) -> Option<(&str, &str)> {
    Some((
        call.field("call_module")?.as_str()?,
        call.field("call_function")?.as_str()?,
    ))
}
fn argument<'a>(call: &'a Value, name: &str) -> Option<&'a Value> {
    sequence(call.field("call_args")?)?
        .iter()
        .find(|arg| arg.field("name").and_then(ValueExt::as_str) == Some(name))?
        .field("value")
}
fn permits(entry: &Value, call: &Value) -> bool {
    let Some((pallet, function)) = call_name(call) else {
        return false;
    };
    if entry
        .field("pallet_name")
        .and_then(ValueExt::value_bytes)
        .as_deref()
        != Some(pallet.as_bytes())
        || entry
            .field("call_name")
            .and_then(ValueExt::value_bytes)
            .as_deref()
            != Some(function.as_bytes())
    {
        return false;
    }
    match entry.field("constraint") {
        Some(Value::Null) => true,
        Some(constraint) => match constraint.variant_name().as_deref() {
            Some("ParamLessThan") => {
                let Some(fields) = constraint.variant("ParamLessThan") else {
                    return false;
                };
                let Some(name) = fields.field("param_name").and_then(ValueExt::value_bytes) else {
                    return false;
                };
                let Ok(name) = std::str::from_utf8(&name) else {
                    return false;
                };
                matches!((argument(call, name).and_then(ValueExt::as_u128), fields.field("limit").and_then(ValueExt::as_u128)), (Some(value), Some(limit)) if value < limit)
            }
            Some("NestedCallMustBe") => {
                let Some(fields) = constraint.variant("NestedCallMustBe") else {
                    return false;
                };
                let Some(name) = fields.field("param_name").and_then(ValueExt::value_bytes) else {
                    return false;
                };
                let Ok(name) = std::str::from_utf8(&name) else {
                    return false;
                };
                let Some((pallet, function)) = argument(call, name).and_then(call_name) else {
                    return false;
                };
                fields
                    .field("pallet_name")
                    .and_then(ValueExt::value_bytes)
                    .as_deref()
                    == Some(pallet.as_bytes())
                    && fields
                        .field("call_name")
                        .and_then(ValueExt::value_bytes)
                        .as_deref()
                        == Some(function.as_bytes())
            }
            _ => false,
        },
        _ => false,
    }
}
fn invalid(message: &str) -> IntentError {
    IntentError::InvalidArgument(message.into())
}
