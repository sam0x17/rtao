// Derived from RaoFoundation/subtensor sdk/python/bittensor/intents/proxy.py @ b6a81d144078eac18bd25c3af7cb31579bb670f3 (Apache-2.0); modified by rtao.
//! Proxy delegation, keyless accounts and announced-call dispatch.

use rtao_client::{DynView, View, ViewExt};
use rtao_primitives::{H256, IntoCoreValue, NetUid, Spend, Ss58Address};
use rtao_runtime::{Call, CallBytes, RuntimeHandle, calls::proxy};
use rtao_signer::SignerRole;
use rtao_tx::{BuildCx, Built, Intent, IntentError, IntentInterface, IntentSafety, PreflightCx};
use serde_json::Value;
use smallvec::SmallVec;

pub use crate::{
    AddProxy, AnnounceProxyCall, CreatePureProxy, ExecuteProxyAnnounced, KillPureProxy,
    RejectProxyAnnouncement, RemoveProxies, RemoveProxy, RemoveProxyAnnouncement,
};

/// Runtime proxy type names, in the Python choice and on-chain variant order.
#[doc(alias = "bittensor.intents.proxy.PROXY_TYPES")]
pub const PROXY_TYPES: [&str; 23] = [
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
    "Validate",
    "Weights",
    "Lock",
    "GammaSpend",
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

/// Proxy types whose calls are hotkey-signed (weights, serving, commitments,
/// EVM association), so the real account is the validator hotkey.
///
/// An explicit add-proxy signer override may select a coldkey or proxied
/// account, for example when preserving a pure account's existing grants.
#[doc(alias = "bittensor.intents.proxy.HOTKEY_PROXY_TYPES")]
pub const HOTKEY_PROXY_TYPES: [&str; 2] = ["Validate", "Weights"];

// Proxy dispatch reports a failed inner call as an event. An enclosing
// batch_all can therefore commit its later calls even when an announced
// atomic batch fails. Inspect the already composed call, including wrappers
// that change the dispatch origin, before allowing it into another batch.
fn announced_call_requires_direct(call: &CallBytes, runtime: &RuntimeHandle) -> bool {
    let decoded = runtime
        .core()
        .decode_spec(&rtao_core::runtime::TypeSpec::Call, call.as_bytes(), true)
        .map(|value| rtao_core::codec::value::to_corpus_json(&value));
    decoded.map_or(true, |value| {
        contains_atomic_batch(&value, 0).unwrap_or(true)
    })
}

fn contains_atomic_batch(call: &Value, depth: usize) -> Option<bool> {
    if depth >= 32 {
        return None;
    }
    let pallet = call.get("call_module")?.as_str()?;
    let function = call.get("call_function")?.as_str()?;
    if pallet.is_empty() || function.is_empty() {
        return None;
    }
    if matches!(
        (pallet, function),
        ("Utility", "batch_all") | ("Crowdloan", "finalize")
    ) {
        return Some(true);
    }
    let single = matches!(
        (pallet, function),
        ("Proxy", "proxy" | "proxy_announced")
            | (
                "Multisig",
                "as_multi" | "as_multi_safe" | "as_multi_threshold_1" | "as_multi_threshold_1_safe"
            )
            | (
                "Utility",
                "as_derivative"
                    | "as_safe_derivative"
                    | "as_protected_derivative"
                    | "dispatch_as"
                    | "dispatch_as_fallible"
                    | "with_weight"
            )
            | ("Sudo", "sudo" | "sudo_unchecked_weight" | "sudo_as")
    );
    let fields = call.get("call_args")?.as_array()?;
    let arg = |name: &str| -> Option<&Value> {
        fields
            .iter()
            .find(|field| field.get("name").and_then(Value::as_str) == Some(name))?
            .get("value")
    };
    if single {
        return contains_atomic_batch(arg("call")?, depth + 1);
    }
    if (pallet, function) == ("Utility", "if_else") {
        return Some(
            contains_atomic_batch(arg("main")?, depth + 1)?
                || contains_atomic_batch(arg("fallback")?, depth + 1)?,
        );
    }
    if pallet == "Utility" && matches!(function, "batch" | "force_batch") {
        for child in arg("calls")?.as_array()? {
            if contains_atomic_batch(child, depth + 1)? {
                return Some(true);
            }
        }
    }
    Some(false)
}

/// Key that signs add/remove for `proxy_type`: the hotkey for hotkey-scoped types.
#[doc(alias = "bittensor.intents.proxy.proxy_grant_signer")]
pub fn proxy_grant_signer(proxy_type: &str) -> SignerRole {
    if HOTKEY_PROXY_TYPES.contains(&proxy_type) {
        SignerRole::Hotkey
    } else {
        SignerRole::Coldkey
    }
}

/// Bound nested arguments before generated serde conversion clones them.
pub(crate) fn check_constructor_input(args: &Value) -> Result<(), IntentError> {
    check_saved_announcement_input(args)?;
    if let Some(inner_args) = args.get("inner_args") {
        crate::arguments::check_nested_json(inner_args)?;
    }
    Ok(())
}

// Saved bytes are a commitment, not a hint to rebuild. Refuse any partial or
// malformed pin before construction as well as after typed mutation.
fn check_saved_announcement_input(args: &Value) -> Result<(), IntentError> {
    let present = |field: &str| args.get(field).filter(|value| !value.is_null());
    let Some(data) = present("announced_call_data") else {
        for field in [
            "expected_call_hash",
            "announced_spec_version",
            "announced_call_semantics",
        ] {
            if present(field).is_some() {
                return Err(IntentError::validation(format!(
                    "{field} requires announced_call_data; the supplied commitment cannot be ignored"
                )));
            }
        }
        return Ok(());
    };
    let Some(data) = data.as_str() else {
        return Err(IntentError::validation(
            "announced_call_data must be nonempty 0x-prefixed byte hex",
        ));
    };
    let Some(digits) = data.strip_prefix("0x") else {
        return Err(IntentError::validation(
            "announced_call_data must be nonempty 0x-prefixed byte hex",
        ));
    };
    if digits.is_empty()
        || digits.len() % 2 != 0
        || !digits.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(IntentError::validation(
            "announced_call_data must be nonempty 0x-prefixed byte hex",
        ));
    }
    if let Some(hash) = present("expected_call_hash") {
        let valid = hash
            .as_str()
            .and_then(|value| value.strip_prefix("0x"))
            .is_some_and(|digits| {
                digits.len() == 64 && digits.bytes().all(|byte| byte.is_ascii_hexdigit())
            });
        if !valid {
            return Err(IntentError::validation(
                "expected_call_hash must be 0x-prefixed Blake2-256 hex",
            ));
        }
    }
    if present("announced_spec_version").is_some_and(|value| {
        value
            .as_u64()
            .is_none_or(|version| version > u64::from(u32::MAX))
    }) {
        return Err(IntentError::validation(
            "announced_spec_version must be a u32",
        ));
    }
    if present("announced_call_semantics").is_some_and(|value| !value.is_object()) {
        return Err(IntentError::validation(
            "announced_call_semantics must be a decoded call object",
        ));
    }
    Ok(())
}

impl ExecuteProxyAnnounced {
    fn saved_call(&self, runtime: &RuntimeHandle) -> Result<Option<CallBytes>, IntentError> {
        let Some(data) = &self.announced_call_data else {
            return Ok(None);
        };
        let digits = data.strip_prefix("0x").ok_or_else(|| {
            IntentError::validation("announced_call_data must be nonempty 0x-prefixed byte hex")
        })?;
        let bytes = hex::decode(digits).map_err(|_| {
            IntentError::validation("announced_call_data must be nonempty 0x-prefixed byte hex")
        })?;
        let call = CallBytes::decode(&bytes, runtime)?;
        if let Some(expected) = &self.expected_call_hash {
            let actual = format!("0x{}", hex::encode(call.hash()));
            if !actual.eq_ignore_ascii_case(expected) {
                return Err(IntentError::validation(format!(
                    "saved call bytes do not match the selected announcement hash ({actual} != {expected})"
                )));
            }
        }
        if self
            .announced_spec_version
            .is_some_and(|version| version != u64::from(runtime.spec_version()))
        {
            let Some(original) = &self.announced_call_semantics else {
                return Err(IntentError::validation(
                    "runtime changed since this call was announced and the saved row has no original decoded semantics; cancel and re-announce it",
                ));
            };
            let decoded = runtime.core().decode_spec(
                &rtao_core::runtime::TypeSpec::Call,
                call.as_bytes(),
                true,
            )?;
            let current = rtao_core::codec::value::to_corpus_json(&decoded);
            if &current != original {
                return Err(IntentError::validation(
                    "runtime metadata now decodes the announced bytes differently; cancel and re-announce the operation",
                ));
            }
        }
        Ok(Some(call))
    }
}

fn include_saved_wire_safety(declared: &mut IntentSafety, wire: IntentSafety) {
    declared.spend = match (declared.spend, wire.spend) {
        (Spend::Unbounded, _) | (_, Spend::Unbounded) => Spend::Unbounded,
        (Spend::None, spend) | (spend, Spend::None) => spend,
        (Spend::Bounded(left), Spend::Bounded(right)) => {
            Spend::Bounded(if left.rao() >= right.rao() {
                left
            } else {
                right
            })
        }
    };
    declared.netuids.extend(wire.netuids);
    declared.netuids.sort_unstable();
    declared.netuids.dedup();
    declared.affects_all_subnets |= wire.affects_all_subnets;
    declared.contains_raw_calls |= wire.contains_raw_calls;
    declared.mev_shield_required |= wire.mev_shield_required;
}

/// Compose an intent exactly as a real account's future announced proxy call,
/// returning its call bytes, Blake2-256 hash and build extras.
#[doc(alias = "bittensor.intents.proxy.compose_announceable_call")]
pub async fn compose_announceable_call<V: View + ?Sized>(
    cx: &BuildCx<'_, V>,
    intent: &dyn rtao_tx::DynIntent,
    real: &Ss58Address,
) -> Result<(Built, String), IntentError> {
    let borrowed = cx.view;
    let nested_cx = BuildCx {
        view: &borrowed as &dyn DynView,
        origin: cx.origin,
        runtime: cx.runtime,
        clock: cx.clock,
        spec: intent.spec(),
    };
    let built = rtao_tx::compose_nested_intent_call(
        &nested_cx,
        intent,
        rtao_tx::OriginView::Account(real.account()),
        "announced proxy",
    )
    .await?;
    let hash = format!("0x{}", hex::encode(built.call.hash()));
    Ok((built, hash))
}

fn add_proxy_signer(
    proxy_type: &str,
    from_hotkey: Option<&Value>,
) -> Result<SignerRole, IntentError> {
    match from_hotkey {
        None | Some(Value::Null) => Ok(proxy_grant_signer(proxy_type)),
        Some(Value::Bool(true)) => Ok(SignerRole::Hotkey),
        Some(Value::Bool(false)) => Ok(SignerRole::Coldkey),
        _ => Err(IntentError::validation(
            "from_hotkey must be a boolean or null",
        )),
    }
}

fn proxy_call_hash(value: &str) -> Result<H256, IntentError> {
    let digits = value
        .strip_prefix("0x")
        .ok_or_else(|| IntentError::validation("call_hash must be 0x-prefixed Blake2-256 hex"))?;
    let bytes = hex::decode(digits)
        .map_err(|_| IntentError::validation("call_hash must be 0x-prefixed Blake2-256 hex"))?;
    let hash = <[u8; 32]>::try_from(bytes.as_slice())
        .map_err(|_| IntentError::validation("call_hash must be 0x-prefixed Blake2-256 hex"))?;
    Ok(H256(hash))
}

fn short_hash(value: &str) -> String {
    let chars = value.chars().collect::<Vec<_>>();
    if chars.len() <= 18 {
        value.to_owned()
    } else {
        format!(
            "{}…{}",
            chars[..10].iter().collect::<String>(),
            chars[chars.len() - 6..].iter().collect::<String>()
        )
    }
}

impl Intent for AnnounceProxyCall {
    fn summary(&self) -> String {
        format!(
            "announce proxy call {} for {}",
            short_hash(&self.call_hash),
            self.real_ss58
        )
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        self.validate()?;
        let real = rtao_core::DynValue::new((&self.real_ss58).into_core_value());
        Ok(Built::new(
            proxy::announce(real, proxy_call_hash(&self.call_hash)?).compose(cx.runtime)?,
        ))
    }

    async fn warnings<V: View + ?Sized>(
        &self,
        _cx: &PreflightCx<'_, V>,
    ) -> Result<Vec<String>, IntentError> {
        Ok(vec![
            "reserves an announcement deposit from the signer (returned on execute, remove, or reject)".into(),
        ])
    }

    fn validate(&self) -> Result<(), IntentError> {
        proxy_call_hash(&self.call_hash)?;
        Ok(())
    }
}

impl Intent for RemoveProxyAnnouncement {
    fn summary(&self) -> String {
        format!(
            "remove announcement {} for {}",
            short_hash(&self.call_hash),
            self.real_ss58
        )
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        self.validate()?;
        let real = rtao_core::DynValue::new((&self.real_ss58).into_core_value());
        Ok(Built::new(
            proxy::remove_announcement(real, proxy_call_hash(&self.call_hash)?)
                .compose(cx.runtime)?,
        ))
    }

    fn validate(&self) -> Result<(), IntentError> {
        proxy_call_hash(&self.call_hash)?;
        Ok(())
    }
}

impl Intent for RejectProxyAnnouncement {
    fn summary(&self) -> String {
        format!(
            "reject announcement {} from {}",
            short_hash(&self.call_hash),
            self.delegate_ss58
        )
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        self.validate()?;
        let delegate = rtao_core::DynValue::new((&self.delegate_ss58).into_core_value());
        Ok(Built::new(
            proxy::reject_announcement(delegate, proxy_call_hash(&self.call_hash)?)
                .compose(cx.runtime)?,
        ))
    }

    fn validate(&self) -> Result<(), IntentError> {
        proxy_call_hash(&self.call_hash)?;
        Ok(())
    }
}

impl Intent for AddProxy {
    fn signer_role(&self) -> Result<SignerRole, IntentError> {
        add_proxy_signer(
            &self.proxy_type,
            serde_json::to_value(self)?.get("from_hotkey"),
        )
    }

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
        if self.signer_role()? == SignerRole::Hotkey {
            warnings.push(format!(
                "{} acts for the hotkey: the hotkey signs this grant and needs free TAO for the deposit and fee",
                self.proxy_type
            ));
        }
        Ok(warnings)
    }

    fn validate(&self) -> Result<(), IntentError> {
        check_proxy_type(&self.proxy_type)?;
        self.signer_role()?;
        Ok(())
    }
}

impl Intent for RemoveProxy {
    fn signer_role(&self) -> Result<SignerRole, IntentError> {
        Ok(proxy_grant_signer(&self.proxy_type))
    }

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
    fn signer_role(&self) -> Result<SignerRole, IntentError> {
        Ok(if self.from_hotkey {
            SignerRole::Hotkey
        } else {
            SignerRole::Coldkey
        })
    }

    fn summary(&self) -> String {
        let signer = if self.from_hotkey {
            "hotkey"
        } else {
            "coldkey"
        };
        format!("remove ALL proxies for the {signer}")
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

    fn requires_build_for_policy(&self) -> bool {
        // A later multisig round rebuilds this call to recover the reserve
        // metadata a protected pure proxy needs for policy and preview.
        true
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        self.validate()?;
        let payer = cx.coldkey_address()?;
        let proxy_type: rtao_runtime::models::ProxyType =
            serde_json::from_value(Value::String(self.proxy_type.clone()))?;
        // Python v488 infers `safe` for a stateless post-quantum coldkey (its
        // legacy derivation does not guarantee an off-curve account); classical
        // and rotating keys have no such inference.
        let mut mode = self
            .account_mode
            .as_deref()
            .map(crate::account_modes::validate_account_mode)
            .transpose()?;
        if mode.is_none()
            && rtao_signer::KeyView::authority_descriptor(&cx.origin, SignerRole::Coldkey)
                .is_some_and(|descriptor| {
                    rtao_primitives::receiving::is_stateless_pq_descriptor(&descriptor)
                })
        {
            mode = Some(crate::account_modes::AccountMode::Safe);
        }
        if mode == Some(crate::account_modes::AccountMode::Safe) {
            let call =
                proxy::create_safe_pure(proxy_type, self.delay, self.index).compose(cx.runtime)?;
            return Ok(Built {
                call,
                extras: serde_json::Map::from_iter([(
                    "pure_proxy_account_mode".to_owned(),
                    Value::from("safe"),
                )]),
            });
        }
        let protected = crate::protected::is_pq_required(cx.view, cx.runtime, payer).await?;
        if !protected {
            return Ok(Built::new(
                proxy::create_pure(proxy_type, self.delay, self.index).compose(cx.runtime)?,
            ));
        }
        let mut call =
            proxy::create_protected_pure(proxy_type, self.delay, self.index).compose(cx.runtime)?;
        let quote = crate::protected::quote_registration_deposit(
            cx.view,
            cx.runtime,
            crate::protected::DERIVED_REGISTRATION_DEPOSIT,
        )
        .await?;
        let deposit = quote.amount_rao;
        if quote.configurable {
            let guard = rtao_runtime::calls::hashed_accounts::check_registration_deposit(
                rtao_primitives::TaoBalance(deposit),
            )
            .compose(cx.runtime)?;
            call =
                rtao_runtime::calls::utility::batch_all(vec![guard, call]).compose(cx.runtime)?;
        }
        let mut proxy_deposit = 0u64;
        for (pallet, name) in [
            ("Proxy", "ProxyDepositBase"),
            ("Proxy", "ProxyDepositFactor"),
        ] {
            if cx.runtime.core().constant(pallet, name).is_none() {
                return Err(IntentError::validation(
                    "could not quote refundable pure-proxy deposit",
                ));
            }
        }
        for constant in [
            &rtao_runtime::constants::proxy::PROXY_DEPOSIT_BASE,
            &rtao_runtime::constants::proxy::PROXY_DEPOSIT_FACTOR,
        ] {
            proxy_deposit = proxy_deposit
                .checked_add(cx.view.constant(constant).await?.0)
                .ok_or(rtao_primitives::UnitError::Overflow)?;
        }
        let reserve = deposit
            .checked_add(proxy_deposit)
            .ok_or(rtao_primitives::UnitError::Overflow)?;
        Ok(Built {
            call,
            extras: serde_json::Map::from_iter([
                (
                    "protected_pure_registration_deposit_rao".to_owned(),
                    Value::from(deposit),
                ),
                (
                    "pure_proxy_deposit_rao".to_owned(),
                    Value::from(proxy_deposit),
                ),
                (
                    "pure_proxy_reserves".to_owned(),
                    serde_json::json!({ payer.to_ss58(cx.runtime.core().ss58_format): reserve }),
                ),
            ]),
        })
    }

    fn validate(&self) -> Result<(), IntentError> {
        // Python validates the derivation mode when building, not at construction.
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
        let spawner = crate::account_argument(self.spawner_ss58.account());
        let proxy_type: rtao_runtime::models::ProxyType =
            serde_json::from_value(Value::String(self.proxy_type.clone()))?;
        let call = match crate::account_modes::validate_account_mode(&self.account_mode)? {
            crate::account_modes::AccountMode::Safe => {
                proxy::kill_safe_pure(spawner, proxy_type, self.index, self.height, self.ext_index)
            }
            crate::account_modes::AccountMode::Legacy => {
                proxy::kill_pure(spawner, proxy_type, self.index, self.height, self.ext_index)
            }
        };
        Ok(Built::new(call.compose(cx.runtime)?))
    }

    fn validate(&self) -> Result<(), IntentError> {
        check_proxy_type(&self.proxy_type)?;
        Ok(())
    }
}

impl Intent for ExecuteProxyAnnounced {
    async fn safety_at<V: View + ?Sized>(
        &self,
        cx: &BuildCx<'_, V>,
    ) -> Result<IntentSafety, IntentError> {
        self.validate()?;
        let inner = crate::AnyIntent::from_tool(&self.inner_op, self.inner_args.clone())?;
        let saved = self.saved_call(cx.runtime)?;
        let borrowed = cx.view;
        let mut safety = if saved.is_some() {
            inner.as_dyn().safety_dyn()?
        } else {
            inner
                .as_dyn()
                .safety_at_dyn(&BuildCx {
                    view: &borrowed as &dyn DynView,
                    origin: cx.origin,
                    runtime: cx.runtime,
                    clock: cx.clock,
                    spec: inner.spec(),
                })
                .await?
        };
        if let Some(call) = saved {
            let wire = crate::crowdloan::announced_wire_safety_at(cx, &call).await?;
            include_saved_wire_safety(&mut safety, wire);
        }
        safety.op = Self::SPEC.name;
        safety.mev_shield_required |= Self::SPEC.mev_shield_required;
        Ok(safety)
    }

    fn safety(&self) -> Result<IntentSafety, IntentError> {
        self.validate()?;
        let inner = crate::AnyIntent::from_tool(&self.inner_op, self.inner_args.clone())?;
        let mut safety = inner.as_dyn().safety_dyn()?;
        if self.announced_call_data.is_some() {
            // A context-free check cannot authenticate saved wire semantics.
            safety.spend = Spend::Unbounded;
            safety.affects_all_subnets = true;
        }
        safety.op = Self::SPEC.name;
        safety.mev_shield_required |= Self::SPEC.mev_shield_required;
        Ok(safety)
    }

    fn spend(&self) -> Spend {
        self.safety()
            .map_or(Spend::Unbounded, |safety| safety.spend)
    }

    fn touches_netuids(&self) -> SmallVec<[NetUid; 2]> {
        self.safety()
            .map(|safety| safety.netuids)
            .unwrap_or_default()
    }

    fn affects_all_subnets(&self) -> bool {
        self.safety()
            .map_or(true, |safety| safety.affects_all_subnets)
    }

    fn summary(&self) -> String {
        format!(
            "execute announced {} as {} via {}",
            self.inner_op, self.real_ss58, self.delegate_ss58
        )
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        self.validate()?;
        let inner = crate::AnyIntent::from_tool(&self.inner_op, self.inner_args.clone())?;
        // The announced call dispatches as the real account: compose it as a
        // direct submission from that account would be (its sudo wrapper and
        // adapters included) and keep its extras for the enclosing dispatch.
        let built = if let Some(call) = self.saved_call(cx.runtime)? {
            Built::new(call)
        } else {
            compose_announceable_call(cx, inner.as_dyn(), &self.real_ss58)
                .await?
                .0
        };
        let inner_call = built.call;
        let mut extras = built.extras;
        if announced_call_requires_direct(&inner_call, cx.runtime) {
            extras.insert("requires_direct_submission".into(), Value::Bool(true));
        }
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
        Ok(Built {
            call: call.compose(cx.runtime)?,
            extras,
        })
    }

    fn requires_build_for_policy(&self) -> bool {
        true
    }

    fn validate(&self) -> Result<(), IntentError> {
        crate::arguments::check_nested_json(&self.inner_args)?;
        check_saved_announcement_input(&serde_json::to_value(self)?)?;
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
    use rtao_primitives::{AccountId, Balance, Spend, Tao};
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

    #[test]
    fn announced_batch_inspection_follows_dispatch_wrappers_and_refuses_unknown_calls() {
        let leaf = json!({"call_module":"System","call_function":"remark","call_args":[]});
        let atomic = json!({"call_module":"Utility","call_function":"batch_all","call_args":[]});
        let wrap = |module: &str, function: &str, name: &str, child: Value| {
            json!({
                "call_module": module, "call_function": function,
                "call_args": [{"name": name, "value": child}],
            })
        };
        assert_eq!(contains_atomic_batch(&leaf, 0), Some(false));
        assert_eq!(contains_atomic_batch(&atomic, 0), Some(true));
        assert_eq!(
            contains_atomic_batch(&wrap("Sudo", "sudo", "call", atomic.clone()), 0),
            Some(true)
        );
        assert_eq!(
            contains_atomic_batch(&wrap("Multisig", "as_multi", "call", atomic.clone()), 0),
            Some(true)
        );
        assert_eq!(
            contains_atomic_batch(
                &wrap(
                    "Utility",
                    "batch",
                    "calls",
                    json!([leaf.clone(), atomic.clone()])
                ),
                0
            ),
            Some(true)
        );
        assert_eq!(
            contains_atomic_batch(
                &wrap("Utility", "force_batch", "calls", json!([leaf.clone()])),
                0
            ),
            Some(false)
        );
        assert_eq!(
            contains_atomic_batch(
                &json!({
                    "call_module":"Utility", "call_function":"if_else",
                    "call_args":[{"name":"main","value":leaf},{"name":"fallback","value":atomic}],
                }),
                0
            ),
            Some(true)
        );
        assert_eq!(
            contains_atomic_batch(
                &json!({"call_module":"Crowdloan","call_function":"finalize","call_args":[]}),
                0
            ),
            Some(true)
        );
        assert_eq!(
            contains_atomic_batch(&wrap("Proxy", "proxy", "call", Value::Null), 0),
            None
        );
        assert_eq!(
            contains_atomic_batch(
                &json!({"call_module":"Utility","call_function":"batch","call_args":[]}),
                0
            ),
            None
        );
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
        serde_json::from_value(json!({
            "delegate_ss58": DELEGATE,
            "real_ss58": REAL,
            "inner_op": inner_op,
            "inner_args": inner_args,
        }))
        .expect("announced proxy defaults")
    }

    fn pure(account_mode: Option<&str>) -> CreatePureProxy {
        CreatePureProxy {
            proxy_type: "Staking".into(),
            delay: 0,
            index: 0,
            account_mode: account_mode.map(str::to_owned),
        }
    }

    #[test]
    fn saved_announcement_pins_are_validated_before_build() -> anyhow::Result<()> {
        let mut args = announced("remove_proxies", json!({})).to_json();
        for field in [
            "expected_call_hash",
            "announced_spec_version",
            "announced_call_semantics",
        ] {
            args[field] = match field {
                "expected_call_hash" => json!(format!("0x{}", "00".repeat(32))),
                "announced_spec_version" => json!(491),
                _ => json!({}),
            };
            assert!(check_constructor_input(&args).is_err(), "orphaned {field}");
            args[field] = Value::Null;
        }
        for bad in [
            json!(false),
            json!(0),
            json!(""),
            json!("0x"),
            json!("0x0"),
            json!("0xgg"),
            json!({}),
        ] {
            args["announced_call_data"] = bad;
            assert!(check_constructor_input(&args).is_err());
        }
        args["announced_call_data"] = json!("0x1003");
        assert!(check_constructor_input(&args).is_ok());
        for (field, bad) in [
            ("expected_call_hash", json!("0x00")),
            ("announced_spec_version", json!(-1)),
            ("announced_call_semantics", json!([])),
        ] {
            args[field] = bad;
            assert!(check_constructor_input(&args).is_err(), "invalid {field}");
            args[field] = Value::Null;
        }
        assert!(crate::AnyIntent::from_tool("execute_proxy_announced", args.clone()).is_ok());
        args["announced_call_data"] = Value::Null;
        assert!(check_constructor_input(&args).is_ok());
        Ok(())
    }

    #[test]
    fn announcement_hash_inputs_require_exact_blake2_256_bytes() {
        assert_eq!(AnnounceProxyCall::SPEC.name, "announce_proxy_call");
        assert_eq!(
            RemoveProxyAnnouncement::SPEC.name,
            "remove_proxy_announcement"
        );
        assert_eq!(
            RejectProxyAnnouncement::SPEC.name,
            "reject_proxy_announcement"
        );
        for (op, address) in [
            ("announce_proxy_call", "real_ss58"),
            ("remove_proxy_announcement", "real_ss58"),
            ("reject_proxy_announcement", "delegate_ss58"),
        ] {
            for bad in ["", "abcd", "0x", "0x00", "0xgg", "éééééééééééééééééééé"]
            {
                let args = json!({address: REAL, "call_hash": bad});
                assert!(
                    crate::AnyIntent::from_tool(op, args).is_err(),
                    "{op}: {bad}"
                );
            }
        }
        assert_eq!(short_hash("éééééééééééééééééééé"), "éééééééééé…éééééé");
    }

    #[tokio::test]
    async fn saved_announcement_replays_exact_bytes_and_checks_hash_and_runtime_semantics()
    -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture().await?;
        let runtime = &snapshot.block().runtime;
        let view = RecordingView::new(snapshot.clone());
        let requests = chain.requests().len();
        let original = build(&announced("remove_proxies", json!({})), &view, runtime).await?;
        let inner_call = proxy::remove_proxies().compose(runtime)?;
        let saved = decode(&inner_call, runtime)?;
        let mut intent = announced("remove_proxies", json!({}));
        intent.announced_call_data = Some(format!("0x{}", hex::encode(inner_call.as_bytes())));
        intent.expected_call_hash = Some(format!("0x{}", hex::encode(inner_call.hash())));
        intent.announced_spec_version = Some(u64::from(runtime.spec_version()));
        intent.announced_call_semantics = Some(saved.clone());
        let replay = build(&intent, &view, runtime).await?;
        assert_eq!(replay.call, original.call);
        assert!(replay.extras.is_empty());
        assert!(intent.requires_build_for_policy());
        intent.expected_call_hash = Some(format!("0x{}", "00".repeat(32)));
        assert!(
            build(&intent, &view, runtime)
                .await
                .expect_err("wrong hash")
                .to_string()
                .contains("selected announcement hash")
        );
        intent.expected_call_hash = None;
        intent.announced_spec_version = Some(u64::from(runtime.spec_version()) - 1);
        intent.announced_call_semantics = None;
        assert!(
            build(&intent, &view, runtime)
                .await
                .expect_err("missing semantics")
                .to_string()
                .contains("no original decoded semantics")
        );
        intent.announced_call_semantics = Some(json!({"call_module":"System"}));
        assert!(
            build(&intent, &view, runtime)
                .await
                .expect_err("changed semantics")
                .to_string()
                .contains("decodes the announced bytes differently")
        );
        intent.announced_call_semantics = Some(saved.clone());
        assert_eq!(build(&intent, &view, runtime).await?.call, original.call);
        assert!(view.touches().is_empty());
        assert_eq!(chain.requests().len(), requests);
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn saved_announcement_policy_uses_wire_amount_when_display_args_drift()
    -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture().await?;
        let runtime = &snapshot.block().runtime;
        let view = RecordingView::new(snapshot.clone());
        let transfer = crate::AnyIntent::from_tool(
            "transfer",
            json!({"dest_ss58": REAL, "amount_tao": "0.000000010"}),
        )?;
        let call = build(transfer.as_dyn(), &view, runtime).await?.call;
        let mut intent = announced(
            "transfer",
            json!({
                "dest_ss58": REAL, "amount_tao": "0.000000001",
            }),
        );
        intent.announced_call_data = Some(format!("0x{}", hex::encode(call.as_bytes())));
        let cx = BuildCx {
            view: &view,
            origin: OriginView::Account(ORIGIN),
            runtime,
            clock: &SystemClock,
            spec: ExecuteProxyAnnounced::SPEC,
        };
        let safety = intent.safety_at(&cx).await?;
        assert_eq!(safety.spend, Spend::Bounded(Balance::<Tao>::from_rao(10)));
        let policy = rtao_tx::Policy {
            max_spend: Some(Balance::<Tao>::from_rao(1)),
            ..Default::default()
        };
        assert!(
            policy
                .check(&safety, None)
                .iter()
                .any(|message| message.contains("max_spend_tao"))
        );
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        client.close().await;
        Ok(())
    }

    #[test]
    fn add_proxy_signer_override_preserves_explicit_false() -> anyhow::Result<()> {
        for name in PROXY_TYPES {
            assert_eq!(add_proxy_signer(name, None)?, proxy_grant_signer(name));
            assert_eq!(
                add_proxy_signer(name, Some(&Value::Null))?,
                proxy_grant_signer(name)
            );
            for (value, role) in [(false, SignerRole::Coldkey), (true, SignerRole::Hotkey)] {
                assert_eq!(add_proxy_signer(name, Some(&json!(value)))?, role);
                if AddProxy::SPEC
                    .fields
                    .iter()
                    .any(|f| f.name == "from_hotkey")
                {
                    let intent = crate::AnyIntent::from_tool(
                        "add_proxy",
                        json!({
                            "delegate_ss58": DELEGATE, "proxy_type": name, "from_hotkey": value,
                        }),
                    )?;
                    assert_eq!(intent.as_dyn().signer_role_dyn()?, role);
                }
            }
        }
        for value in [json!(0), json!("false"), json!([]), json!({})] {
            assert!(add_proxy_signer("Staking", Some(&value)).is_err());
        }
        Ok(())
    }

    #[tokio::test]
    async fn pure_proxy_safe_mode_protected_payers_and_kill_modes_follow_python()
    -> anyhow::Result<()> {
        let mut state = FakeState::try_default()?;
        state.seed_python(
            "HashedAccounts",
            "MlDsaAccounts",
            &[json!(REAL.to_string())],
            json!({}),
        )?;
        state.seed_python(
            "HashedAccounts",
            "HashedAccountRegistrationDeposit",
            &[],
            json!(250_000_000u64),
        )?;
        let chain = FakeChain::from_state(state);
        let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
        config.track_heads = false;
        config.runtime_cache_dir = None;
        config.config_store = None;
        let client = Client::from_rpc(RpcClient::new(chain.clone()), config).await?;
        let snapshot = client.at(BlockRef::Finalized).await?;
        let runtime = &snapshot.block().runtime;
        let build_as = |intent: CreatePureProxy, origin: AccountId| {
            let snapshot = snapshot.clone();
            async move {
                let cx = BuildCx {
                    view: &snapshot,
                    origin: OriginView::Account(origin),
                    runtime: &snapshot.block().runtime,
                    clock: &SystemClock,
                    spec: CreatePureProxy::SPEC,
                };
                intent.build(&cx).await
            }
        };
        assert!(pure(None).requires_build_for_policy());

        // An ordinary payer keeps the plain call and no reserve metadata.
        for mode in [None, Some("legacy")] {
            let built = build_as(pure(mode), ORIGIN).await?;
            let decoded = decode(&built.call, runtime)?;
            assert_eq!(decoded["call_function"], "create_pure");
            assert!(built.extras.is_empty());
        }

        // Safe derivation is a distinct call; the chain derives the account.
        let built = build_as(pure(Some("safe")), ORIGIN).await?;
        let decoded = decode(&built.call, runtime)?;
        assert_eq!(decoded["call_module"], "Proxy");
        assert_eq!(decoded["call_function"], "create_safe_pure");
        assert_eq!(built.extras["pure_proxy_account_mode"], "safe");
        assert_eq!(built.extras.len(), 1);

        // The mode is checked when building, not at construction.
        assert!(pure(Some("Safe")).validate().is_ok());
        let error = build_as(pure(Some("Safe")), ORIGIN)
            .await
            .expect_err("unknown mode");
        assert_eq!(error.to_string(), "account_mode must be 'legacy' or 'safe'");

        // A quantum-resistant payer creates a protected pure proxy, guarded by
        // the live configurable reserve, and reports every reserve it holds.
        let built = build_as(pure(None), REAL).await?;
        let decoded = decode(&built.call, runtime)?;
        assert_eq!(decoded["call_module"], "Utility");
        assert_eq!(decoded["call_function"], "batch_all");
        let calls = &decoded["call_args"][0]["value"];
        assert_eq!(calls[0]["call_module"], "HashedAccounts");
        assert_eq!(calls[0]["call_function"], "check_registration_deposit");
        assert_eq!(calls[0]["call_args"][0]["value"], json!(250_000_000u64));
        assert_eq!(calls[1]["call_module"], "Proxy");
        assert_eq!(calls[1]["call_function"], "create_protected_pure");
        let base = snapshot
            .constant(&rtao_runtime::constants::proxy::PROXY_DEPOSIT_BASE)
            .await?
            .0;
        let factor = snapshot
            .constant(&rtao_runtime::constants::proxy::PROXY_DEPOSIT_FACTOR)
            .await?
            .0;
        assert_eq!(
            built.extras["protected_pure_registration_deposit_rao"],
            json!(250_000_000u64)
        );
        assert_eq!(built.extras["pure_proxy_deposit_rao"], json!(base + factor));
        assert_eq!(
            built.extras["pure_proxy_reserves"],
            json!({ REAL.to_string(): 250_000_000u64 + base + factor })
        );
        // An explicit safe mode never takes the protected path.
        let built = build_as(pure(Some("safe")), REAL).await?;
        assert_eq!(
            decode(&built.call, runtime)?["call_function"],
            "create_safe_pure"
        );

        for (mode, function) in [("legacy", "kill_pure"), ("safe", "kill_safe_pure")] {
            let intent = KillPureProxy {
                spawner_ss58: DELEGATE.into(),
                proxy_type: "Staking".into(),
                index: 0,
                height: 5,
                ext_index: 1,
                account_mode: mode.into(),
            };
            let built = build(&intent, &snapshot, runtime).await?;
            assert_eq!(decode(&built.call, runtime)?["call_function"], function);
        }
        let intent = KillPureProxy {
            spawner_ss58: DELEGATE.into(),
            proxy_type: "Staking".into(),
            index: 0,
            height: 5,
            ext_index: 1,
            account_mode: "other".into(),
        };
        assert!(intent.validate().is_ok());
        assert_eq!(
            build(&intent, &snapshot, runtime)
                .await
                .expect_err("unknown mode")
                .to_string(),
            "account_mode must be 'legacy' or 'safe'"
        );
        client.close().await;
        Ok(())
    }

    #[test]
    fn announced_proxies_always_build_for_policy() {
        assert!(announced("create_pure_proxy", json!({})).requires_build_for_policy());
        assert!(announced("remove_proxies", json!({})).requires_build_for_policy());
        assert!(announced("missing-intent", json!({})).requires_build_for_policy());
    }

    #[test]
    fn proxy_choices_validate_exact_spelling_with_python_diagnostic() -> anyhow::Result<()> {
        assert_eq!(PROXY_TYPES.len(), 23);
        for name in PROXY_TYPES {
            assert_eq!(check_proxy_type(name)?, name);
        }
        for name in ["", "any", "STAKING", "Staking ", "Any'\n", "🦀"] {
            assert_eq!(
                check_proxy_type(name)
                    .expect_err("invalid choice")
                    .to_string(),
                format!(
                    "unknown proxy type {}; expected one of: Any, Owner, NonCritical, NonTransfer, Senate, NonFungible, Triumvirate, Governance, Staking, Registration, Transfer, SmallTransfer, RootWeights, ChildKeys, SudoUncheckedSetCode, SwapHotkey, SubnetLeaseBeneficiary, RootClaim, BasketTrading, Validate, Weights, Lock, GammaSpend",
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
            let add: AddProxy = serde_json::from_value(json!({
                "delegate_ss58": DELEGATE,
                "proxy_type": name,
                "delay": u32::MAX,
            }))?;
            let remove = RemoveProxy {
                delegate_ss58: DELEGATE.into(),
                proxy_type: name.into(),
                delay: 0,
            };
            let create = CreatePureProxy {
                account_mode: None,
                proxy_type: name.into(),
                delay: u32::MAX,
                index: u16::MAX,
            };
            let kill = KillPureProxy {
                account_mode: "legacy".to_owned(),
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
        // The hotkey variant changes only who signs, never the call.
        for from_hotkey in [false, true] {
            let built = build(&RemoveProxies { from_hotkey }, &view, runtime).await?;
            assert_eq!(built.call.as_bytes(), &[16, 3]);
            assert!(built.extras.is_empty());
            assert_eq!(decode(&built.call, runtime)?["call_args"], json!([]));
        }
        for prefix in [0, 2, 42, 110] {
            let spelling = DELEGATE.to_ss58(prefix);
            let intent: AddProxy = serde_json::from_value(json!({
                "delegate_ss58": spelling,
                "proxy_type": "Staking",
                "delay": 0,
            }))?;
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
        // Only create_pure_proxy reads state: whether its payer is protected.
        assert!(view.touches().iter().all(|touch| {
            touch.pallet_or_api == "HashedAccounts"
                && ["Accounts", "MlDsaAccounts", "DerivedAuthorities"]
                    .contains(&touch.item.as_str())
        }));
        assert!(
            chain.requests()[requests..]
                .iter()
                .all(|request| request.method.starts_with("state_"))
        );
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
                "remove ALL proxies for the coldkey".into(),
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
            (
                "announce_proxy_call",
                json!({"real_ss58":REAL,"call_hash":format!("0x{}", "ab".repeat(32))}),
                format!("announce proxy call 0xabababab…ababab for {REAL}"),
                vec![
                    "reserves an announcement deposit from the signer (returned on execute, remove, or reject)",
                ],
            ),
            (
                "remove_proxy_announcement",
                json!({"real_ss58":REAL,"call_hash":format!("0x{}", "ab".repeat(32))}),
                format!("remove announcement 0xabababab…ababab for {REAL}"),
                vec![],
            ),
            (
                "reject_proxy_announcement",
                json!({"delegate_ss58":DELEGATE,"call_hash":format!("0x{}", "ab".repeat(32))}),
                format!("reject announcement 0xabababab…ababab from {DELEGATE}"),
                vec![],
            ),
        ] {
            let intent = crate::AnyIntent::from_tool(op, args)?;
            let dynamic = intent.as_dyn();
            let built = build(dynamic, &view, runtime).await?;
            if let Some(function) = match op {
                "announce_proxy_call" => Some("announce"),
                "remove_proxy_announcement" => Some("remove_announcement"),
                "reject_proxy_announcement" => Some("reject_announcement"),
                _ => None,
            } {
                let call = decode(&built.call, runtime)?;
                assert_eq!(call["call_module"], "Proxy");
                assert_eq!(call["call_function"], function);
                assert_eq!(
                    call["call_args"][1]["value"],
                    format!("0x{}", "ab".repeat(32))
                );
            }
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
        // Only create_pure_proxy reads state: whether its payer is protected.
        assert!(view.touches().iter().all(|touch| {
            touch.pallet_or_api == "HashedAccounts"
                && ["Accounts", "MlDsaAccounts", "DerivedAuthorities"]
                    .contains(&touch.item.as_str())
        }));
        assert!(
            chain.requests()[requests..]
                .iter()
                .all(|request| request.method.starts_with("state_"))
        );
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
        let inner = crate::AnyIntent::from_tool("remove_proxies", json!({}))?;
        let (composed, hash) = compose_announceable_call(
            &BuildCx {
                view: &view,
                origin: OriginView::Account(ORIGIN),
                runtime,
                clock: &SystemClock,
                spec: ExecuteProxyAnnounced::SPEC,
            },
            inner.as_dyn(),
            &nested.real_ss58,
        )
        .await?;
        assert_eq!(composed.call, proxy::remove_proxies().compose(runtime)?);
        assert_eq!(hash, format!("0x{}", hex::encode(composed.call.hash())));
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

    #[cfg(feature = "batch")]
    #[tokio::test]
    async fn announced_atomic_batch_is_directly_buildable_but_refused_as_batch_child()
    -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture().await?;
        let runtime = &snapshot.block().runtime;
        let view = RecordingView::new(snapshot.clone());
        let intent = announced(
            "batch",
            json!({
                "intents": [{"op":"remove_proxies"}],
            }),
        );
        let built = build(&intent, &view, runtime).await?;
        assert_eq!(
            built.extras.get("requires_direct_submission"),
            Some(&json!(true))
        );
        let batch = crate::Batch::new(vec![intent.to_json()])?;
        let error = build(&batch, &view, runtime)
            .await
            .expect_err("announced batch nested in batch");
        assert_eq!(
            error.to_string(),
            "this announced payload contains or may contain an atomic batch; submit it directly, not inside another batch"
        );
        let inner = rtao_runtime::calls::utility::batch_all(vec![
            proxy::remove_proxies().compose(runtime)?,
        ])
        .compose(runtime)?;
        let mut saved = intent;
        saved.announced_call_data = Some(format!("0x{}", hex::encode(inner.as_bytes())));
        assert_eq!(
            build(&saved, &view, runtime)
                .await?
                .extras
                .get("requires_direct_submission"),
            Some(&json!(true))
        );
        let batch = crate::Batch::new(vec![saved.to_json()])?;
        assert!(build(&batch, &view, runtime).await.is_err());
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn hotkey_scoped_grants_sign_with_the_hotkey_and_warn_about_its_deposit()
    -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture().await?;
        let runtime = &snapshot.block().runtime;
        let view = RecordingView::new(snapshot.clone());
        let requests = chain.requests().len();
        assert_eq!(HOTKEY_PROXY_TYPES, ["Validate", "Weights"]);
        for name in PROXY_TYPES {
            let expected = if HOTKEY_PROXY_TYPES.contains(&name) {
                SignerRole::Hotkey
            } else {
                SignerRole::Coldkey
            };
            assert_eq!(proxy_grant_signer(name), expected, "{name}");
            for op in ["add_proxy", "remove_proxy"] {
                let intent = crate::AnyIntent::from_tool(
                    op,
                    json!({"delegate_ss58":DELEGATE.to_string(),"proxy_type":name}),
                )?;
                assert_eq!(intent.as_dyn().signer_role_dyn()?, expected, "{op} {name}");
                // The catalog keeps Python's class-level default.
                assert_eq!(intent.spec().signer, "coldkey");
            }
            // Pure proxies and kills stay with the spawner's coldkey.
            let create =
                crate::AnyIntent::from_tool("create_pure_proxy", json!({"proxy_type":name}))?;
            assert_eq!(create.as_dyn().signer_role_dyn()?, SignerRole::Coldkey);
        }
        for name in HOTKEY_PROXY_TYPES {
            let add: AddProxy = serde_json::from_value(json!({
                "delegate_ss58": DELEGATE,
                "proxy_type": name,
                "delay": 0,
            }))?;
            let built = build(&add, &view, runtime).await?;
            let preview = add
                .preflight_dyn(&PreflightCx {
                    view: &view as &dyn DynView,
                    runtime,
                    clock: &SystemClock,
                    spec: add.spec(),
                    client: &client,
                    dispatch_origin: ORIGIN,
                    fee_payer: ORIGIN,
                    call: &built.call,
                })
                .await?;
            assert_eq!(
                preview.warnings,
                vec![
                    "reserves a proxy deposit from the signer (returned on removal)".to_owned(),
                    format!(
                        "{name} acts for the hotkey: the hotkey signs this grant and needs free TAO for the deposit and fee"
                    ),
                ]
            );
        }
        for (args, role, summary) in [
            (
                json!({}),
                SignerRole::Coldkey,
                "remove ALL proxies for the coldkey",
            ),
            (
                json!({"from_hotkey":false}),
                SignerRole::Coldkey,
                "remove ALL proxies for the coldkey",
            ),
            (
                json!({"from_hotkey":true}),
                SignerRole::Hotkey,
                "remove ALL proxies for the hotkey",
            ),
        ] {
            let intent = crate::AnyIntent::from_tool("remove_proxies", args)?;
            assert_eq!(intent.as_dyn().signer_role_dyn()?, role);
            assert_eq!(intent.as_dyn().summary_dyn(), summary);
            assert_eq!(
                crate::AnyIntent::from_tool("remove_proxies", intent.to_json())?,
                intent
            );
        }
        for invalid in [json!(1), json!("true"), Value::Null] {
            assert!(
                crate::AnyIntent::from_tool("remove_proxies", json!({"from_hotkey":invalid}))
                    .is_err()
            );
        }
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
            crate::registry::unknown_op("missing-intent").to_string()
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
                crate::registry::unknown_op("missing-intent").to_string()
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

    // Python v488 `CreatePureProxy.build`: a stateless post-quantum coldkey
    // (fixed ML-DSA or Falcon) creates a safe pure proxy unless a mode is
    // given; rotating and classical keys keep the legacy derivation.
    #[tokio::test]
    async fn stateless_post_quantum_creators_default_to_safe_pure_proxies() -> anyhow::Result<()> {
        use rtao_signer::{KeyView, SignerError};

        struct Creator(Vec<u8>);
        impl KeyView for Creator {
            fn hotkey_address(&self) -> Result<AccountId, SignerError> {
                Err(SignerError::KeyUnavailable(SignerRole::Hotkey))
            }
            fn hotkey_public_key(&self) -> Result<[u8; 32], SignerError> {
                Err(SignerError::KeyUnavailable(SignerRole::Hotkey))
            }
            fn coldkey_address(&self) -> Result<AccountId, SignerError> {
                Ok(ORIGIN)
            }
            fn authority_descriptor(&self, role: SignerRole) -> Option<Vec<u8>> {
                (role == SignerRole::Coldkey).then(|| self.0.clone())
            }
        }

        let (_chain, client, snapshot) = fixture().await?;
        let runtime = &snapshot.block().runtime;
        for (code, inferred) in [(8u8, true), (9, true), (11, true), (7, false), (4, false)] {
            let key = rtao_core::keys::Keypair::from_seed(&[3; 32], code)?;
            let keys = Creator(key.authorization_descriptor()?);
            let cx = BuildCx {
                view: &snapshot,
                origin: OriginView::Keys(&keys),
                runtime,
                clock: &SystemClock,
                spec: CreatePureProxy::SPEC,
            };
            for (mode, safe) in [
                (None, inferred),
                (Some("legacy"), false),
                (Some("safe"), true),
            ] {
                let built = pure(mode).build(&cx).await?;
                let decoded = decode(&built.call, runtime)?;
                assert_eq!(
                    decoded["call_function"] == "create_safe_pure",
                    safe,
                    "{code} {mode:?}: {decoded}"
                );
            }
        }
        client.close().await;
        Ok(())
    }

    #[cfg(feature = "registration")]
    #[tokio::test]
    async fn announced_proxy_inner_builds_against_the_real_account_not_the_callers_wallet()
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
        let cx = BuildCx {
            view: &view,
            origin: OriginView::Keys(&AttachedKeys),
            runtime,
            clock: &SystemClock,
            spec: ExecuteProxyAnnounced::SPEC,
        };
        // The announced call dispatches as the real account, which carries no
        // hotkey: a defaulted hotkey no longer comes from the caller's wallet.
        let error = announced("root_register", json!({}))
            .build(&cx)
            .await
            .expect_err("the real account has no hotkey to default to");
        assert_eq!(
            error.to_string(),
            "root_register: this signer carries no hotkey to default to; pass the hotkey address explicitly or sign with a Wallet"
        );
        let explicit = AccountId([7; 32]).to_string();
        let built = announced("root_register", json!({"hotkey_ss58": explicit}))
            .build(&cx)
            .await?;
        let decoded = decode(&built.call, runtime)?;
        let nested = &decoded["call_args"][3]["value"];
        assert_eq!(nested["call_module"], "SubtensorModule");
        assert_eq!(nested["call_function"], "root_register");
        assert_eq!(nested["call_args"][0]["value"], explicit);
        assert!(built.extras.is_empty());
        assert!(view.touches().is_empty());
        assert_eq!(chain.requests().len(), requests);
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        client.close().await;
        Ok(())
    }

    #[cfg(feature = "root")]
    #[tokio::test]
    async fn announced_root_inner_gains_its_sudo_wrapper() -> anyhow::Result<()> {
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
        // Composed as a direct submission would be: a root operation is
        // dispatched through Sudo.sudo.
        let sudo = &decoded["call_args"][3]["value"];
        assert_eq!(sudo["call_module"], "Sudo");
        assert_eq!(sudo["call_function"], "sudo");
        let nested = &sudo["call_args"][0]["value"];
        assert_eq!(nested["call_module"], "AdminUtils");
        assert_eq!(nested["call_function"], "sudo_set_subnet_emission_enabled");
        assert_eq!(nested["call_args"][0]["value"], json!(3));
        assert_eq!(nested["call_args"][1]["value"], json!(true));
        assert_eq!(intent.touches_netuids().as_slice(), &[NetUid(3)]);
        assert!(built.extras.is_empty());
        assert!(view.touches().is_empty());
        assert_eq!(chain.requests().len(), requests);
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        client.close().await;
        Ok(())
    }
}
