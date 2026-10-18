// Derived from RaoFoundation/subtensor sdk/python/bittensor/intents/crowdloan.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Crowdloan funding, settlement and creator updates.

use crate::arguments::check_nested_json as check_call_budget;
#[cfg(test)]
use crate::nested_json::{MAX_CALL_DEPTH, check_call_limits, inspect_call_limits};
use rtao_client::{DynView, View, ViewExt};
use rtao_core::{codec::Value as CoreValue, value_ext::ValueExt};
use rtao_primitives::{Amount, Balance, DynBalance, NetUid, Spend, TaoBalance};
use rtao_runtime::{
    CallBytes, calls::crowdloan, models::CrowdloanCall, storage::crowdloan::CROWDLOANS,
};
use rtao_tx::{BuildCx, Built, Intent, IntentError, IntentInterface, IntentSafety, PreflightCx};
use serde_json::Value;
use smallvec::SmallVec;
use std::collections::BTreeSet;

pub use crate::generated::{
    ContributeCrowdloan, CreateCrowdloan, DissolveCrowdloan, FinalizeCrowdloan, RefundCrowdloan,
    SetCrowdloanMaxContribution, UpdateCrowdloanCap, UpdateCrowdloanEnd,
    UpdateCrowdloanMinContribution, WithdrawCrowdloan,
};

fn amount_summary(amount: &Amount) -> String {
    match crate::arguments::tao_amount(amount) {
        Ok(amount) => DynBalance::from(amount).to_string(),
        // Constructor validation rejects invalid typed inputs before execution.
        Err(_) => match amount {
            Amount::All => "all".into(),
            Amount::Exact(amount) => amount.to_decimal_string(),
        },
    }
}

fn bounded_spend(amount: &Amount) -> Spend {
    crate::arguments::tao_amount(amount).map_or(Spend::Unbounded, Spend::Bounded)
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

/// Bound tool input before generated serde conversion recurses into `call`.
pub(crate) fn check_constructor_input(args: &Value) -> Result<(), IntentError> {
    if let Some(call) = args.get("call") {
        check_call_budget(call)?;
    }
    Ok(())
}

fn base_safety(op: &'static str, spend: Spend) -> IntentSafety {
    IntentSafety {
        op,
        spend,
        netuids: SmallVec::new(),
        affects_all_subnets: false,
        contains_raw_calls: false,
        mev_shield_required: false,
    }
}

fn child_intent(call: &Value) -> Result<crate::AnyIntent, IntentError> {
    check_call_budget(call)?;
    let fields = call
        .as_object()
        .ok_or_else(|| IntentError::validation("crowdloan 'call' must be an intent object"))?;
    let op = fields
        .get("op")
        .and_then(Value::as_str)
        .filter(|op| !op.is_empty())
        .ok_or_else(|| IntentError::validation("crowdloan 'call' needs an 'op' key"))?;
    crate::AnyIntent::from_tool(op, call.clone())
}

fn deferred_shield_refusal(safety: &IntentSafety) -> Result<(), IntentError> {
    if safety.mev_shield_required {
        return Err(IntentError::validation(
            "crowdloan callbacks requiring MEV shielding are unsupported: shielding creation or finalization cannot protect the deferred child dispatch",
        ));
    }
    Ok(())
}

fn stored_deferred_source_refusal(call: &CoreValue) -> Result<(), IntentError> {
    let mut pending = vec![call];
    let mut visits = 0usize;
    while let Some(call) = pending.pop() {
        visits += 1;
        if visits > MAX_DEFERRED_CALLS {
            return Err(callback_error(
                "callback exceeds its inspection node budget",
            ));
        }
        match (
            call.field("call_module").and_then(ValueExt::as_str),
            call.field("call_function").and_then(ValueExt::as_str),
        ) {
            (Some("Crowdloan"), Some("finalize")) => {
                return Err(callback_error(
                    "a future callback cannot authorize another stored finalization whose source may change before dispatch",
                ));
            }
            (Some("Utility"), Some("batch" | "batch_all" | "force_batch")) => {
                let children = core_list(call_arg(call, "calls")?)
                    .ok_or_else(|| callback_error("invalid callback batch"))?;
                if children
                    .len()
                    .saturating_add(pending.len())
                    .saturating_add(visits)
                    > MAX_DEFERRED_CALLS
                {
                    return Err(callback_error(
                        "callback exceeds its inspection node budget",
                    ));
                }
                pending.extend(children);
            }
            (Some("Proxy"), Some("proxy" | "proxy_announced"))
            | (Some("Multisig"), Some("as_multi" | "as_multi_threshold_1"))
            | (Some("Sudo"), Some("sudo" | "sudo_unchecked_weight")) => {
                pending.push(call_arg(call, "call")?)
            }
            (Some("Crowdloan"), Some("create")) => {
                let child = call_arg(call, "call")?;
                if !matches!(child, CoreValue::Null) {
                    pending.push(child);
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn callback_error(detail: impl std::fmt::Display) -> IntentError {
    IntentError::validation(format!(
        "cannot verify deferred crowdloan callback: {detail}"
    ))
}

fn call_args(call: &CoreValue) -> Result<&[CoreValue], IntentError> {
    call.field("call_args")
        .and_then(core_list)
        .ok_or_else(|| callback_error("missing decoded call arguments"))
}

fn core_list(value: &CoreValue) -> Option<&[CoreValue]> {
    match value {
        CoreValue::List(values) | CoreValue::Tuple(values) => Some(values),
        _ => None,
    }
}

fn call_arg<'a>(call: &'a CoreValue, name: &str) -> Result<&'a CoreValue, IntentError> {
    call_args(call)?
        .iter()
        .find(|field| field.field("name").and_then(ValueExt::as_str) == Some(name))
        .and_then(|field| field.field("value"))
        .ok_or_else(|| callback_error(format!("missing decoded argument {name}")))
}

fn unsigned(value: &CoreValue, name: &str) -> Result<u64, IntentError> {
    value
        .as_u128()
        .and_then(|value| u64::try_from(value).ok())
        .ok_or_else(|| callback_error(format!("{name} is not a native u64")))
}

fn netuid(value: &CoreValue) -> Result<NetUid, IntentError> {
    u16::try_from(unsigned(value, "netuid")?)
        .map(NetUid)
        .map_err(|_| callback_error("netuid exceeds u16"))
}

const MAX_DEFERRED_DEPTH: usize = 32;
const MAX_DEFERRED_CALLS: usize = 4096;
const MAX_DEFERRED_BYTES: usize = 4 * 1024 * 1024;

async fn bounded_preimage<V: View + ?Sized>(
    cx: &BuildCx<'_, V>,
    bounded: CrowdloanCall,
    bytes_left: usize,
) -> Result<Vec<u8>, IntentError> {
    let (hash, len) = match bounded {
        CrowdloanCall::Inline(bytes) => {
            if bytes.len() > bytes_left || bytes.len() > 128 {
                return Err(callback_error(
                    "inline call exceeds its inspection byte budget",
                ));
            }
            return Ok(bytes);
        }
        CrowdloanCall::Lookup { hash, len } => (hash, len),
        CrowdloanCall::Legacy { hash } => {
            let status_name = ["RequestStatusFor", "StatusFor"]
                .into_iter()
                .find(|name| cx.runtime.storage_entry("Preimage", name).is_some())
                .ok_or_else(|| callback_error("legacy preimage length is unavailable"))?;
            let status = cx
                .view
                .storage_dyn("Preimage", status_name, &[CoreValue::Bytes(hash.to_vec())])
                .await?;
            let len = status
                .as_core()
                .variant("Requested")
                .or_else(|| status.as_core().variant("Unrequested"))
                .and_then(|status| status.field("maybe_len").or_else(|| status.field("len")))
                .and_then(ValueExt::as_u128)
                .and_then(|len| u32::try_from(len).ok())
                .ok_or_else(|| callback_error("legacy preimage has no authenticated length"))?;
            (hash, len)
        }
    };
    if usize::try_from(len).ok().is_none_or(|len| len > bytes_left) {
        return Err(callback_error(
            "preimage exceeds its inspection byte budget",
        ));
    }
    let entry = cx
        .runtime
        .storage_entry("Preimage", "PreimageFor")
        .ok_or_else(|| callback_error("preimage storage is unavailable"))?;
    let hash_value = CoreValue::Bytes(hash.to_vec());
    let len_value = CoreValue::Uint(u128::from(len));
    let keys = match entry.key_types.len() {
        1 => vec![CoreValue::Tuple(vec![hash_value, len_value])],
        2 => vec![hash_value, len_value],
        _ => return Err(callback_error("unsupported preimage storage key shape")),
    };
    let value = cx
        .view
        .storage_dyn("Preimage", "PreimageFor", &keys)
        .await?;
    let bytes = value
        .as_core()
        .value_bytes()
        .filter(|bytes| bytes.len() == len as usize)
        .ok_or_else(|| callback_error("preimage is missing or its length does not match"))?;
    let call = CallBytes::decode(&bytes, cx.runtime)?;
    if call.hash() != hash {
        return Err(callback_error(
            "preimage hash does not match its stored commitment",
        ));
    }
    Ok(bytes)
}

/// Inspect only positively reviewed wire semantics. Unknown calls, opaque
/// approvals and new catalog operations never inherit a default no-spend label.
fn leaf_safety(
    call: &CoreValue,
    descriptor: &'static rtao_runtime::CallDescriptor,
) -> Result<IntentSafety, IntentError> {
    let (pallet, name) = (descriptor.pallet, descriptor.name);
    let mut safety = base_safety(name, Spend::None);
    if rtao_catalog::INTENTS
        .iter()
        .any(|spec| spec.mev_shield_required && spec.wraps.contains(&(pallet, name)))
    {
        safety.mev_shield_required = true;
        deferred_shield_refusal(&safety)?;
    }
    let spend_field = match (pallet, name) {
        ("Balances", "transfer_keep_alive" | "transfer_allow_death") => Some("value"),
        ("SubtensorModule", "add_stake" | "add_stake_limit" | "stake_into_basket") => {
            Some("amount_staked")
        }
        ("SubtensorModule", "add_stake_burn") | ("Crowdloan", "contribute") => Some("amount"),
        ("Balances", "transfer_all")
        | (
            "SubtensorModule",
            "register_network"
            | "register_leased_network"
            | "transfer_stake"
            | "transfer_stake_and_hotkey",
        ) => {
            safety.spend = Spend::Unbounded;
            None
        }
        (
            "Proxy",
            "add_proxy" | "create_pure" | "kill_pure" | "remove_proxies" | "remove_proxy",
        )
        | ("Multisig", "cancel_as_multi")
        | ("EVM", "withdraw")
        | (
            "Crowdloan",
            "dissolve"
            | "refund"
            | "withdraw"
            | "set_max_contribution"
            | "update_cap"
            | "update_end"
            | "update_min_contribution",
        )
        | (
            "SubtensorModule",
            "announce_coldkey_swap"
            | "associate_evm_key"
            | "try_associate_hotkey"
            | "claim_root"
            | "claim_root_with_hotkey"
            | "clear_coldkey_swap_announcement"
            | "commit_timelocked_mechanism_weights"
            | "decrease_take"
            | "dispute_coldkey_swap"
            | "increase_take"
            | "lock_stake"
            | "move_lock"
            | "move_stake"
            | "move_stake_limit"
            | "pow_register"
            | "remove_stake"
            | "remove_stake_limit"
            | "reveal_weights"
            | "root_register"
            | "serve_axon"
            | "serve_axon_tls"
            | "serve_prometheus"
            | "set_coldkey_auto_stake_hotkey"
            | "set_childkey_take"
            | "set_children"
            | "set_identity"
            | "set_min_collateral"
            | "set_perpetual_lock"
            | "set_reject_locked_alpha"
            | "set_reject_staking_hotkey_growth"
            | "sudo_set_root_claim_threshold"
            | "set_subnet_identity"
            | "set_mechanism_weights"
            | "start_call"
            | "swap_basket"
            | "swap_basket_many"
            | "swap_coldkey_announced"
            | "swap_hotkey"
            | "swap_stake"
            | "swap_stake_limit"
            | "terminate_lease"
            | "unstake_all"
            | "unstake_all_alpha"
            | "update_symbol",
        )
        | (
            "AdminUtils",
            "sudo_set_epoch_consensus"
            | "sudo_set_tempo"
            | "sudo_set_immunity_period"
            | "sudo_set_min_allowed_weights"
            | "sudo_set_weights_version_key"
            | "sudo_set_activity_cutoff_factor"
            | "sudo_set_min_burn"
            | "sudo_set_max_burn"
            | "sudo_set_bonds_moving_average"
            | "sudo_set_bonds_penalty"
            | "sudo_set_serving_rate_limit"
            | "sudo_set_commit_reveal_weights_interval"
            | "sudo_set_max_allowed_uids"
            | "sudo_set_burn_increase_mult"
            | "sudo_set_burn_half_life"
            | "sudo_set_collateral_lock_share"
            | "sudo_set_collateral_drain_ratio"
            | "sudo_set_adjustment_alpha"
            | "sudo_set_rho"
            | "sudo_set_max_difficulty"
            | "sudo_set_alpha_sigmoid_steepness"
            | "sudo_set_min_childkey_take_per_subnet"
            | "sudo_set_owner_immune_neuron_limit"
            | "sudo_set_alpha_values"
            | "sudo_set_commit_reveal_weights_enabled"
            | "sudo_set_liquid_alpha_enabled"
            | "sudo_set_network_registration_allowed"
            | "sudo_set_network_pow_registration_allowed"
            | "sudo_set_yuma3_enabled"
            | "sudo_set_bonds_reset_enabled"
            | "sudo_set_toggle_transfer"
            | "sudo_set_owner_cut_enabled"
            | "sudo_set_owner_cut_auto_lock_enabled"
            | "sudo_set_mechanism_count"
            | "sudo_set_subnet_emission_enabled"
            | "sudo_set_subnet_emission_weights"
            | "sudo_trim_null_uids_batch"
            | "sudo_trim_to_max_allowed_uids",
        ) => None,
        _ => {
            return Err(callback_error(format!("unreviewed call {pallet}.{name}")));
        }
    };
    if let Some(field) = spend_field {
        safety.spend = Spend::Bounded(Balance::from_rao(unsigned(call_arg(call, field)?, field)?));
    }
    safety.affects_all_subnets = matches!(
        name,
        "swap_coldkey_announced" | "unstake_all" | "unstake_all_alpha"
    );
    if (pallet, name) == ("SubtensorModule", "root_register") {
        safety.netuids.push(NetUid::ROOT);
    }
    for field in call_args(call)? {
        let name = field
            .field("name")
            .and_then(ValueExt::as_str)
            .ok_or_else(|| callback_error("decoded argument has no name"))?;
        let value = field
            .field("value")
            .ok_or_else(|| callback_error("decoded argument has no value"))?;
        // sudo_set_root_claim_threshold always encodes root, but its semantic
        // API has no netuid field and deliberately retains the empty scope.
        if (pallet, descriptor.name) == ("SubtensorModule", "sudo_set_root_claim_threshold") {
            continue;
        }
        if (name == "netuid" || name.ends_with("_netuid")) && !matches!(value, CoreValue::Null) {
            safety.netuids.push(netuid(value)?);
        }
        if name == "netuids" || name.ends_with("_netuids") {
            for uid in core_list(value).ok_or_else(|| callback_error("invalid subnet list"))? {
                safety.netuids.push(netuid(uid)?);
            }
        }
    }
    if (pallet, name) == ("SubtensorModule", "swap_basket_many") {
        for leg in core_list(call_arg(call, "legs")?)
            .ok_or_else(|| callback_error("invalid basket legs"))?
        {
            let leg = core_list(leg)
                .filter(|leg| leg.len() == 4)
                .ok_or_else(|| callback_error("invalid basket leg"))?;
            safety.netuids.extend([netuid(&leg[0])?, netuid(&leg[1])?]);
        }
    }
    safety.netuids.sort_unstable();
    safety.netuids.dedup();
    Ok(safety)
}

async fn finalize_safety_at<V: View + ?Sized>(
    cx: &BuildCx<'_, V>,
    id: u32,
) -> Result<IntentSafety, IntentError> {
    enum Pending {
        Finalize(u32, usize, bool),
        Encoded(CrowdloanCall, usize, bool),
        Call(CoreValue, usize, bool),
    }
    let mut safety = base_safety(FinalizeCrowdloan::SPEC.name, Spend::None);
    let mut pending = vec![Pending::Finalize(id, 0, false)];
    let mut seen = BTreeSet::new();
    let mut visits = 0usize;
    let mut bytes_left = MAX_DEFERRED_BYTES;
    while let Some(item) = pending.pop() {
        visits += 1;
        if visits > MAX_DEFERRED_CALLS {
            return Err(callback_error(
                "callback exceeds its inspection node budget",
            ));
        }
        match item {
            Pending::Finalize(id, depth, future) => {
                if future {
                    return Err(callback_error(
                        "a future callback cannot authorize another stored finalization whose source may change before dispatch",
                    ));
                }
                if depth > MAX_DEFERRED_DEPTH || !seen.insert(id) {
                    return Err(callback_error("cyclic or repeated deferred finalization"));
                }
                let loan = cx
                    .view
                    .storage(&CROWDLOANS, (id,))
                    .await?
                    .ok_or_else(|| callback_error(format!("crowdloan {id} is unavailable")))?;
                match (loan.target_address, loan.call) {
                    (Some(_), None) => safety.spend = Spend::Unbounded,
                    (None, Some(call)) => pending.push(Pending::Encoded(call, depth + 1, future)),
                    _ => {
                        return Err(callback_error(
                            "stored crowdloan needs exactly one target or callback",
                        ));
                    }
                }
            }
            Pending::Encoded(bounded, depth, future) => {
                if depth > MAX_DEFERRED_DEPTH {
                    return Err(callback_error(
                        "callback exceeds its inspection depth budget",
                    ));
                }
                let bytes = bounded_preimage(cx, bounded, bytes_left).await?;
                bytes_left = bytes_left.checked_sub(bytes.len()).ok_or_else(|| {
                    callback_error("callbacks exceed their total inspection byte budget")
                })?;
                CallBytes::decode(&bytes, cx.runtime)?;
                let decoded = cx
                    .runtime
                    .core()
                    .decode_spec(&rtao_core::runtime::TypeSpec::Call, &bytes, true)
                    .map_err(rtao_core::CoreError::from)?;
                pending.push(Pending::Call(decoded, depth, future));
            }
            Pending::Call(call, depth, future) => {
                if depth > MAX_DEFERRED_DEPTH {
                    return Err(callback_error(
                        "callback exceeds its inspection depth budget",
                    ));
                }
                let pallet = call
                    .field("call_module")
                    .and_then(ValueExt::as_str)
                    .ok_or_else(|| callback_error("call has no pallet"))?;
                let name = call
                    .field("call_function")
                    .and_then(ValueExt::as_str)
                    .ok_or_else(|| callback_error("call has no function"))?;
                let descriptor = rtao_runtime::calls::ALL
                    .iter()
                    .copied()
                    .find(|descriptor| descriptor.pallet == pallet && descriptor.name == name)
                    .ok_or_else(|| callback_error(format!("unsupported call {pallet}.{name}")))?;
                let report = rtao_runtime::verify_surface_with(
                    cx.runtime,
                    &[rtao_runtime::SurfaceDescriptor::Call(descriptor)],
                );
                if report
                    .changes
                    .iter()
                    .any(|change| change.class == rtao_runtime::SurfaceClass::Breaking)
                {
                    return Err(callback_error(format!(
                        "call surface changed for {pallet}.{name}"
                    )));
                }
                match (pallet, name) {
                    ("Utility", "batch" | "batch_all" | "force_batch") => {
                        let children = core_list(call_arg(&call, "calls")?)
                            .ok_or_else(|| callback_error("invalid batched callback"))?;
                        if children
                            .len()
                            .saturating_add(pending.len())
                            .saturating_add(visits)
                            > MAX_DEFERRED_CALLS
                        {
                            return Err(callback_error(
                                "callback exceeds its inspection node budget",
                            ));
                        }
                        pending.extend(
                            children
                                .iter()
                                .cloned()
                                .map(|call| Pending::Call(call, depth + 1, future)),
                        );
                    }
                    ("Proxy", "proxy" | "proxy_announced")
                    | ("Multisig", "as_multi" | "as_multi_threshold_1")
                    | ("Sudo", "sudo" | "sudo_unchecked_weight") => {
                        pending.push(Pending::Call(
                            call_arg(&call, "call")?.clone(),
                            depth + 1,
                            future,
                        ));
                    }
                    ("Crowdloan", "finalize") => {
                        let id = u32::try_from(unsigned(
                            call_arg(&call, "crowdloan_id")?,
                            "crowdloan_id",
                        )?)
                        .map_err(|_| callback_error("crowdloan ID exceeds u32"))?;
                        pending.push(Pending::Finalize(id, depth + 1, future));
                    }
                    ("Crowdloan", "create") => {
                        let deposit = unsigned(call_arg(&call, "deposit")?, "deposit")?;
                        safety.include(base_safety(
                            "create_crowdloan",
                            Spend::Bounded(Balance::from_rao(deposit)),
                        ));
                        let child = call_arg(&call, "call")?;
                        let target = call_arg(&call, "target_address")?;
                        if matches!(child, CoreValue::Null) == matches!(target, CoreValue::Null) {
                            return Err(callback_error(
                                "nested creation needs exactly one target or callback",
                            ));
                        }
                        if !matches!(child, CoreValue::Null) {
                            pending.push(Pending::Call(child.clone(), depth + 1, true));
                        }
                    }
                    ("System", "remark" | "remark_with_event") => {}
                    _ => safety.include(leaf_safety(&call, descriptor)?),
                }
            }
        }
    }
    deferred_shield_refusal(&safety)?;
    Ok(safety)
}

impl Intent for CreateCrowdloan {
    fn safety(&self) -> Result<IntentSafety, IntentError> {
        self.validate()?;
        let mut safety = base_safety(Self::SPEC.name, bounded_spend(&self.deposit_tao));
        if let Some(call) = self.call.as_ref().filter(|call| truthy(call)) {
            let child = child_intent(call)?.as_dyn().safety_dyn()?;
            deferred_shield_refusal(&child)?;
            safety.include(child);
        }
        Ok(safety)
    }

    async fn safety_at<V: View + ?Sized>(
        &self,
        cx: &BuildCx<'_, V>,
    ) -> Result<IntentSafety, IntentError> {
        let safety = self.safety()?;
        if let Some(call) = self.call.as_ref().filter(|call| truthy(call)) {
            let child = child_intent(call)?;
            let borrowed = cx.view;
            let resolved = child
                .as_dyn()
                .safety_at_dyn(&BuildCx {
                    view: &borrowed as &dyn DynView,
                    origin: cx.origin,
                    runtime: cx.runtime,
                    clock: cx.clock,
                    spec: child.spec(),
                })
                .await?;
            deferred_shield_refusal(&resolved)?;
        }
        // Authorization happens now; later state-dependent amounts remain
        // unbounded even when today's pinned state happens to be cheap.
        Ok(safety)
    }

    fn summary(&self) -> String {
        format!(
            "create crowdloan (cap {}, deposit {}, end {})",
            amount_summary(&self.cap_tao),
            amount_summary(&self.deposit_tao),
            self.end
        )
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        self.safety()?;
        let inner = if let Some(call) = self.call.as_ref().filter(|value| truthy(value)) {
            let intent = child_intent(call)?;
            let borrowed = cx.view;
            let inner_cx = BuildCx {
                view: &borrowed as &dyn rtao_client::DynView,
                origin: cx.origin,
                runtime: cx.runtime,
                clock: cx.clock,
                spec: intent.spec(),
            };
            // Python invokes the nested semantic build directly. Executor
            // sudo/proxy adapters and nested build extras do not enter create.
            let inner = intent.as_dyn().build_dyn(&inner_cx).await?.call;
            let decoded = cx
                .runtime
                .core()
                .decode_spec(&rtao_core::runtime::TypeSpec::Call, inner.as_bytes(), true)
                .map_err(rtao_core::CoreError::from)?;
            stored_deferred_source_refusal(&decoded)?;
            Some(inner)
        } else {
            None
        };
        Ok(Built::new(
            crowdloan::create(
                crate::arguments::tao_amount(&self.deposit_tao)?.to_wire(),
                crate::arguments::tao_amount(&self.min_contribution_tao)?.to_wire(),
                crate::arguments::tao_amount(&self.cap_tao)?.to_wire(),
                self.end,
                inner,
                self.target_ss58.as_ref().map(|target| target.account()),
            )
            .compose(cx.runtime)?,
        ))
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

    fn validate(&self) -> Result<(), IntentError> {
        let has_call = self.call.as_ref().is_some_and(truthy);
        if self.target_ss58.is_some() == has_call {
            return Err(IntentError::validation(
                "provide exactly one of target_ss58 or call",
            ));
        }
        crate::arguments::tao_amount(&self.deposit_tao)?;
        crate::arguments::tao_amount(&self.min_contribution_tao)?;
        crate::arguments::tao_amount(&self.cap_tao)?;
        if let Some(call) = &self.call {
            check_call_budget(call)?;
        }
        Ok(())
    }
}

/// Resolve a contribution through the shared exact/all-free TAO helper.
pub(crate) async fn stakeable_amount<V: View + ?Sized>(
    intent: &ContributeCrowdloan,
    cx: &BuildCx<'_, V>,
) -> Result<TaoBalance, IntentError> {
    crate::arguments::stakeable_amount(&intent.amount_tao, cx).await
}

impl Intent for ContributeCrowdloan {
    fn summary(&self) -> String {
        let amount = if matches!(self.amount_tao, Amount::All) {
            "ALL free TAO".into()
        } else {
            amount_summary(&self.amount_tao)
        };
        format!("contribute {amount} to crowdloan {}", self.crowdloan_id)
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        crate::generated::passthrough::contribute_crowdloan(self, cx).await
    }

    async fn warnings<V: View + ?Sized>(
        &self,
        _cx: &PreflightCx<'_, V>,
    ) -> Result<Vec<String>, IntentError> {
        Ok(if matches!(self.amount_tao, Amount::All) {
            vec![
                "contributes the entire free balance (minus the existential deposit and fee headroom)"
                    .into(),
            ]
        } else {
            vec![]
        })
    }

    fn spend(&self) -> Spend {
        bounded_spend(&self.amount_tao)
    }

    fn validate(&self) -> Result<(), IntentError> {
        self.amount_tao.resolve(NetUid::ROOT, true)?;
        Ok(())
    }
}

impl Intent for FinalizeCrowdloan {
    fn spend(&self) -> Spend {
        // The callback is stored on chain; target-only loans transfer a live
        // raised amount. Neither can be bounded from the ID alone.
        Spend::Unbounded
    }

    fn affects_all_subnets(&self) -> bool {
        true
    }

    async fn safety_at<V: View + ?Sized>(
        &self,
        cx: &BuildCx<'_, V>,
    ) -> Result<IntentSafety, IntentError> {
        finalize_safety_at(cx, self.crowdloan_id).await
    }

    fn summary(&self) -> String {
        format!("finalize crowdloan {}", self.crowdloan_id)
    }
    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        // Building bytes also serves cancellation/hash discovery, which does
        // not authorize the callback. Execution owns pinned safety_at review.
        crate::generated::passthrough::finalize_crowdloan(self, cx).await
    }
}

impl Intent for RefundCrowdloan {
    fn summary(&self) -> String {
        format!("refund contributors of crowdloan {}", self.crowdloan_id)
    }
    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        crate::generated::passthrough::refund_crowdloan(self, cx).await
    }
}

impl Intent for DissolveCrowdloan {
    fn summary(&self) -> String {
        format!("dissolve crowdloan {}", self.crowdloan_id)
    }
    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        crate::generated::passthrough::dissolve_crowdloan(self, cx).await
    }
}

impl Intent for WithdrawCrowdloan {
    fn summary(&self) -> String {
        format!("withdraw from crowdloan {}", self.crowdloan_id)
    }
    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        crate::generated::passthrough::withdraw_crowdloan(self, cx).await
    }
}

impl Intent for UpdateCrowdloanCap {
    fn summary(&self) -> String {
        format!(
            "update crowdloan {} cap to {}",
            self.crowdloan_id,
            amount_summary(&self.new_cap_tao)
        )
    }
    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        crate::generated::passthrough::update_crowdloan_cap(self, cx).await
    }
    fn validate(&self) -> Result<(), IntentError> {
        crate::arguments::tao_amount(&self.new_cap_tao)?;
        Ok(())
    }
}

impl Intent for UpdateCrowdloanEnd {
    fn summary(&self) -> String {
        format!(
            "update crowdloan {} end to block {}",
            self.crowdloan_id, self.new_end
        )
    }
    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        crate::generated::passthrough::update_crowdloan_end(self, cx).await
    }
}

impl Intent for UpdateCrowdloanMinContribution {
    fn summary(&self) -> String {
        format!(
            "update crowdloan {} min contribution to {}",
            self.crowdloan_id,
            amount_summary(&self.new_min_contribution_tao)
        )
    }
    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        crate::generated::passthrough::update_crowdloan_min_contribution(self, cx).await
    }
    fn validate(&self) -> Result<(), IntentError> {
        crate::arguments::tao_amount(&self.new_min_contribution_tao)?;
        Ok(())
    }
}

impl Intent for SetCrowdloanMaxContribution {
    fn summary(&self) -> String {
        match &self.new_max_contribution_tao {
            Some(amount) => format!(
                "set crowdloan {} max contribution to {}",
                self.crowdloan_id,
                amount_summary(amount)
            ),
            None => format!("clear crowdloan {} max contribution", self.crowdloan_id),
        }
    }
    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        crate::generated::passthrough::set_crowdloan_max_contribution(self, cx).await
    }
    fn validate(&self) -> Result<(), IntentError> {
        if let Some(amount) = &self.new_max_contribution_tao {
            crate::arguments::tao_amount(amount)?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use rtao_client::{BlockRef, Client, ClientConfig, Network, Snapshot};
    use rtao_primitives::{AccountId, Alpha, Balance, Tao};
    use rtao_testkit::{FakeChain, FakeState, rpc::RpcClient};
    use rtao_tx::{IntentInterface, OriginView, SystemClock};
    use serde_json::json;

    const ORIGIN: AccountId = AccountId([7; 32]);
    const OTHER: AccountId = AccountId([8; 32]);

    async fn fixture(free: u64, deposit: u64) -> anyhow::Result<(FakeChain, Client, Snapshot)> {
        let mut state = FakeState::try_default()?;
        state.seed_python(
            "System",
            "Account",
            &[json!(ORIGIN.to_string())],
            json!({"data":{"free":free}}),
        )?;
        state.seed_python(
            "System",
            "Account",
            &[json!(OTHER.to_string())],
            json!({"data":{"free":u64::MAX}}),
        )?;
        state.seed_constant("Balances", "ExistentialDeposit", json!(deposit))?;
        state.seed_python(
            "Crowdloan",
            "Crowdloans",
            &[json!(42)],
            json!({
                "creator":ORIGIN,"target_address":OTHER,"call":null,"raised":100u64
            }),
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

    #[test]
    fn summaries_spend_and_default_policy_match_all_ten_python_intents() {
        let create = CreateCrowdloan {
            deposit_tao: Balance::<Tao>::from_rao(1_000_000_000).into(),
            min_contribution_tao: Balance::<Tao>::ZERO.into(),
            cap_tao: Balance::<Tao>::from_rao(1_234_500_000_000).into(),
            end: 900,
            target_ss58: Some(OTHER.into()),
            call: None,
        };
        assert_eq!(
            create.summary(),
            "create crowdloan (cap τ1,234.500000000, deposit τ1.000000000, end 900)"
        );
        assert_eq!(
            create.spend(),
            Spend::Bounded(Balance::<Tao>::from_rao(1_000_000_000))
        );
        let contribution = ContributeCrowdloan {
            crowdloan_id: 42,
            amount_tao: Balance::<Tao>::from_rao(1).into(),
        };
        assert_eq!(
            contribution.summary(),
            "contribute τ0.000000001 to crowdloan 42"
        );
        assert_eq!(
            contribution.spend(),
            Spend::Bounded(Balance::<Tao>::from_rao(1))
        );
        let all = ContributeCrowdloan {
            amount_tao: Amount::All,
            ..contribution
        };
        assert_eq!(all.summary(), "contribute ALL free TAO to crowdloan 42");
        assert_eq!(all.spend(), Spend::Unbounded);
        assert_eq!(
            FinalizeCrowdloan { crowdloan_id: 42 }.summary(),
            "finalize crowdloan 42"
        );
        assert_eq!(
            RefundCrowdloan { crowdloan_id: 42 }.summary(),
            "refund contributors of crowdloan 42"
        );
        assert_eq!(
            DissolveCrowdloan { crowdloan_id: 42 }.summary(),
            "dissolve crowdloan 42"
        );
        assert_eq!(
            WithdrawCrowdloan { crowdloan_id: 42 }.summary(),
            "withdraw from crowdloan 42"
        );
        assert_eq!(
            UpdateCrowdloanCap {
                crowdloan_id: 42,
                new_cap_tao: Balance::<Tao>::from_rao(1).into()
            }
            .summary(),
            "update crowdloan 42 cap to τ0.000000001"
        );
        assert_eq!(
            UpdateCrowdloanEnd {
                crowdloan_id: 42,
                new_end: 12
            }
            .summary(),
            "update crowdloan 42 end to block 12"
        );
        assert_eq!(
            UpdateCrowdloanMinContribution {
                crowdloan_id: 42,
                new_min_contribution_tao: Balance::<Tao>::ZERO.into()
            }
            .summary(),
            "update crowdloan 42 min contribution to τ0.000000000"
        );
        let clear = SetCrowdloanMaxContribution {
            crowdloan_id: 42,
            new_max_contribution_tao: None,
        };
        assert_eq!(clear.summary(), "clear crowdloan 42 max contribution");
        assert_eq!(clear.spend(), Spend::None);
        assert_eq!(
            SetCrowdloanMaxContribution {
                new_max_contribution_tao: Some(Balance::<Tao>::ZERO.into()),
                ..clear
            }
            .summary(),
            "set crowdloan 42 max contribution to τ0.000000000"
        );
        assert!(create.touches_netuids().is_empty());
        assert!(!create.affects_all_subnets());
        assert_eq!(
            <ContributeCrowdloan as IntentInterface>::SPEC.all_amount_fields,
            &["amount_tao"]
        );
    }

    #[tokio::test]
    async fn all_ten_builds_encode_python_arguments_and_preserve_json() -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture(2_000_000, 100).await?;
        let runtime = &snapshot.block().runtime;
        let wire = TaoBalance(u64::MAX);
        for (op, args, expected) in [
            (
                "create_crowdloan",
                json!({"deposit_tao":"18446744073.709551615","min_contribution_tao":"0","cap_tao":"0.000000001","end":0,"target_ss58":OTHER.to_string()}),
                crowdloan::create(wire, TaoBalance(0), TaoBalance(1), 0, None, Some(OTHER)),
            ),
            (
                "contribute_crowdloan",
                json!({"crowdloan_id":u32::MAX,"amount_tao":"18446744073.709551615"}),
                crowdloan::contribute(u32::MAX, wire),
            ),
            (
                "finalize_crowdloan",
                json!({"crowdloan_id":42}),
                crowdloan::finalize(42),
            ),
            (
                "refund_crowdloan",
                json!({"crowdloan_id":42}),
                crowdloan::refund(42),
            ),
            (
                "dissolve_crowdloan",
                json!({"crowdloan_id":42}),
                crowdloan::dissolve(42),
            ),
            (
                "withdraw_crowdloan",
                json!({"crowdloan_id":42}),
                crowdloan::withdraw(42),
            ),
            (
                "update_crowdloan_cap",
                json!({"crowdloan_id":42,"new_cap_tao":"0"}),
                crowdloan::update_cap(42, TaoBalance(0)),
            ),
            (
                "update_crowdloan_end",
                json!({"crowdloan_id":42,"new_end":u32::MAX}),
                crowdloan::update_end(42, u32::MAX),
            ),
            (
                "update_crowdloan_min_contribution",
                json!({"crowdloan_id":42,"new_min_contribution_tao":"18446744073.709551615"}),
                crowdloan::update_min_contribution(42, wire),
            ),
            (
                "set_crowdloan_max_contribution",
                json!({"crowdloan_id":42}),
                crowdloan::set_max_contribution(42, None),
            ),
            (
                "set_crowdloan_max_contribution",
                json!({"crowdloan_id":42,"new_max_contribution_tao":"0"}),
                crowdloan::set_max_contribution(42, Some(TaoBalance(0))),
            ),
        ] {
            let intent = crate::AnyIntent::from_tool(op, args)?;
            let cx = BuildCx {
                view: &snapshot as &dyn rtao_client::DynView,
                origin: OriginView::Account(ORIGIN),
                runtime,
                clock: &SystemClock,
                spec: intent.spec(),
            };
            let built = intent.as_dyn().build_dyn(&cx).await?;
            assert_eq!(
                built.call.as_bytes(),
                expected.compose(runtime)?.as_bytes(),
                "{op}"
            );
            assert!(built.extras.is_empty(), "{op}");
            assert_eq!(
                crate::AnyIntent::from_tool(op, intent.to_json())?,
                intent,
                "{op}"
            );
            assert_eq!(intent.spec().signer, "coldkey", "{op}");
            assert!(intent.as_dyn().touches_netuids_dyn().is_empty(), "{op}");
            assert_eq!(
                intent.as_dyn().affects_all_subnets_dyn(),
                op == "finalize_crowdloan",
                "{op}"
            );
        }
        assert!(chain.state().lock().unwrap().submissions.is_empty());
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn all_uses_effective_origin_free_balance_and_warns_without_submission()
    -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture(2_000_000, 100).await?;
        let intent = ContributeCrowdloan {
            crowdloan_id: 42,
            amount_tao: Amount::All,
        };
        let cx = BuildCx {
            view: &snapshot,
            origin: OriginView::Account(ORIGIN),
            runtime: &snapshot.block().runtime,
            clock: &SystemClock,
            spec: ContributeCrowdloan::SPEC,
        };
        let built = intent.build(&cx).await?;
        assert_eq!(
            built.call.as_bytes(),
            crowdloan::contribute(42, TaoBalance(1_499_900))
                .compose(cx.runtime)?
                .as_bytes()
        );
        let preflight = PreflightCx {
            view: &snapshot,
            runtime: cx.runtime,
            clock: &SystemClock,
            spec: ContributeCrowdloan::SPEC,
            client: &client,
            dispatch_origin: ORIGIN,
            fee_payer: OTHER,
            call: &built.call,
        };
        let preview = intent.preflight(&preflight).await?;
        assert_eq!(preview.effects, vec![intent.summary()]);
        assert_eq!(
            preview.warnings,
            vec![
                "contributes the entire free balance (minus the existential deposit and fee headroom)"
            ]
        );
        assert!(preview.blocks.is_empty());
        let exact = ContributeCrowdloan {
            amount_tao: Balance::<Tao>::ZERO.into(),
            ..intent
        };
        assert!(exact.warnings(&preflight).await?.is_empty());
        assert!(chain.state().lock().unwrap().submissions.is_empty());
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn all_refuses_zero_shortfall_and_deposit_overflow_with_python_diagnostic()
    -> anyhow::Result<()> {
        for (free, deposit) in [(0, 0), (500_099, 100), (500_100, 100), (u64::MAX, u64::MAX)] {
            let (_, client, snapshot) = fixture(free, deposit).await?;
            let intent = ContributeCrowdloan {
                crowdloan_id: 42,
                amount_tao: Amount::All,
            };
            let cx = BuildCx {
                view: &snapshot,
                origin: OriginView::Account(ORIGIN),
                runtime: &snapshot.block().runtime,
                clock: &SystemClock,
                spec: ContributeCrowdloan::SPEC,
            };
            assert_eq!(
                intent.build(&cx).await.unwrap_err().to_string(),
                format!(
                    "nothing to stake: free balance {} does not cover the existential deposit plus the transaction fee",
                    DynBalance::from(Balance::<Tao>::from_rao(free))
                )
            );
            client.close().await;
        }
        let (_, client, snapshot) = fixture(500_101, 100).await?;
        let intent = ContributeCrowdloan {
            crowdloan_id: 42,
            amount_tao: Amount::All,
        };
        let cx = BuildCx {
            view: &snapshot,
            origin: OriginView::Account(ORIGIN),
            runtime: &snapshot.block().runtime,
            clock: &SystemClock,
            spec: ContributeCrowdloan::SPEC,
        };
        assert_eq!(stakeable_amount(&intent, &cx).await?, TaoBalance(1));
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn nested_build_preserves_origin_and_inserts_only_semantic_call() -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture(2_000_000, 100).await?;
        let create = CreateCrowdloan {
            deposit_tao: Balance::<Tao>::ZERO.into(),
            min_contribution_tao: Balance::<Tao>::ZERO.into(),
            cap_tao: Balance::<Tao>::ZERO.into(),
            end: 1,
            target_ss58: None,
            call: Some(json!({"op":"contribute_crowdloan","crowdloan_id":42,"amount_tao":"all"})),
        };
        let cx = BuildCx {
            view: &snapshot,
            origin: OriginView::Account(ORIGIN),
            runtime: &snapshot.block().runtime,
            clock: &SystemClock,
            spec: CreateCrowdloan::SPEC,
        };
        let built = create.build(&cx).await?;
        let inner = crowdloan::contribute(42, TaoBalance(1_499_900)).compose(cx.runtime)?;
        let expected = crowdloan::create(
            TaoBalance(0),
            TaoBalance(0),
            TaoBalance(0),
            1,
            Some(inner),
            None,
        )
        .compose(cx.runtime)?;
        assert_eq!(built.call.as_bytes(), expected.as_bytes());
        assert!(built.extras.is_empty());
        assert_eq!(create.spend(), Spend::Unbounded);
        let preflight = PreflightCx {
            view: &snapshot,
            runtime: cx.runtime,
            clock: &SystemClock,
            spec: CreateCrowdloan::SPEC,
            client: &client,
            dispatch_origin: ORIGIN,
            fee_payer: OTHER,
            call: &built.call,
        };
        assert!(create.warnings(&preflight).await?.is_empty());
        assert!(chain.state().lock().unwrap().submissions.is_empty());
        client.close().await;
        Ok(())
    }

    #[test]
    fn constructor_preserves_python_truthiness_and_rejects_invalid_money() {
        let create = CreateCrowdloan {
            deposit_tao: Balance::<Tao>::ZERO.into(),
            min_contribution_tao: Balance::<Tao>::ZERO.into(),
            cap_tao: Balance::<Tao>::ZERO.into(),
            end: 0,
            target_ss58: None,
            call: None,
        };
        assert_eq!(
            create.validate().unwrap_err().to_string(),
            "provide exactly one of target_ss58 or call"
        );
        let empty = CreateCrowdloan {
            call: Some(json!({})),
            ..create.clone()
        };
        assert!(empty.validate().is_err());
        let target = CreateCrowdloan {
            target_ss58: Some(OTHER.into()),
            ..empty
        };
        assert!(target.validate().is_ok());
        let both = CreateCrowdloan {
            call: Some(json!({"op":"finalize_crowdloan","crowdloan_id":1})),
            ..target.clone()
        };
        assert_eq!(
            both.validate().unwrap_err().to_string(),
            "provide exactly one of target_ss58 or call"
        );
        for invalid in [
            Amount::All,
            Balance::<Alpha>::new(NetUid(1), 1).unwrap().into(),
        ] {
            assert!(
                CreateCrowdloan {
                    deposit_tao: invalid.clone(),
                    ..target.clone()
                }
                .validate()
                .is_err()
            );
            assert!(
                CreateCrowdloan {
                    min_contribution_tao: invalid.clone(),
                    ..target.clone()
                }
                .validate()
                .is_err()
            );
            assert!(
                CreateCrowdloan {
                    cap_tao: invalid.clone(),
                    ..target.clone()
                }
                .validate()
                .is_err()
            );
            assert!(
                UpdateCrowdloanCap {
                    crowdloan_id: 0,
                    new_cap_tao: invalid.clone()
                }
                .validate()
                .is_err()
            );
            assert!(
                UpdateCrowdloanMinContribution {
                    crowdloan_id: 0,
                    new_min_contribution_tao: invalid.clone()
                }
                .validate()
                .is_err()
            );
            assert!(
                SetCrowdloanMaxContribution {
                    crowdloan_id: 0,
                    new_max_contribution_tao: Some(invalid)
                }
                .validate()
                .is_err()
            );
        }
        assert!(
            ContributeCrowdloan {
                crowdloan_id: 0,
                amount_tao: Balance::<Alpha>::new(NetUid(1), 1).unwrap().into()
            }
            .validate()
            .is_err()
        );
        assert!(
            crate::AnyIntent::from_tool(
                "contribute_crowdloan",
                json!({"crowdloan_id":0,"amount_tao":"18446744073.709551616"})
            )
            .is_err()
        );
    }

    #[tokio::test]
    async fn nested_missing_or_false_op_refuses_before_reads() -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture(2_000_000, 100).await?;
        for call in [
            json!({"crowdloan_id":42}),
            json!({"op":""}),
            json!({"op":false}),
            json!({"op":null}),
        ] {
            let create = CreateCrowdloan {
                deposit_tao: Balance::<Tao>::ZERO.into(),
                min_contribution_tao: Balance::<Tao>::ZERO.into(),
                cap_tao: Balance::<Tao>::ZERO.into(),
                end: 1,
                target_ss58: None,
                call: Some(call),
            };
            let cx = BuildCx {
                view: &snapshot,
                origin: OriginView::Account(ORIGIN),
                runtime: &snapshot.block().runtime,
                clock: &SystemClock,
                spec: CreateCrowdloan::SPEC,
            };
            let requests_before = chain.state().lock().unwrap().requests.len();
            assert_eq!(
                create.build(&cx).await.unwrap_err().to_string(),
                "crowdloan 'call' needs an 'op' key"
            );
            assert_eq!(
                chain.state().lock().unwrap().requests.len(),
                requests_before
            );
        }
        client.close().await;
        Ok(())
    }

    #[test]
    fn programmatic_nested_json_has_depth_node_and_encoded_byte_budgets() {
        let mut call = Value::Null;
        for _ in 1..MAX_CALL_DEPTH {
            call = Value::Array(vec![call]);
        }
        assert!(check_call_budget(&call).is_ok());
        call = Value::Array(vec![call]);
        assert!(
            check_call_budget(&call)
                .unwrap_err()
                .to_string()
                .contains("depth budget")
        );
        let wide = json!([null, null]);
        assert!(inspect_call_limits(&wide, 128, 3, 100).is_ok());
        assert!(
            inspect_call_limits(&wide, 128, 2, 100)
                .unwrap_err()
                .to_string()
                .contains("node budget")
        );
        let escaped = json!("\u{0000}");
        assert!(inspect_call_limits(&escaped, 128, 1, 8).is_ok());
        assert!(
            inspect_call_limits(&escaped, 128, 1, 7)
                .unwrap_err()
                .to_string()
                .contains("byte budget")
        );
        for key in [
            "$serde_json::private::RawValue",
            "$serde_json::private::Number",
        ] {
            let mut fields = serde_json::Map::new();
            fields.insert(key.into(), json!("{\"op\":\"finalize_crowdloan\"}"));
            assert!(
                check_call_budget(&Value::Object(fields))
                    .unwrap_err()
                    .to_string()
                    .contains("private JSON carrier")
            );
        }
    }

    #[test]
    fn recursive_work_budget_refuses_inputs_that_fit_literal_limits() {
        let wide = json!([null, null]);
        let cost = inspect_call_limits(&wide, 128, 3, 100).unwrap();
        assert_eq!((cost.nodes, cost.levels, cost.bytes), (3, 2, 11));
        assert!(
            check_call_limits(&wide, 128, 3, 100)
                .unwrap_err()
                .to_string()
                .contains("recursive JSON node-work budget")
        );
        let nested = json!([null]);
        let cost = inspect_call_limits(&nested, 128, 4, 6).unwrap();
        assert_eq!((cost.nodes, cost.levels, cost.bytes), (2, 2, 6));
        assert!(
            check_call_limits(&nested, 128, 4, 6)
                .unwrap_err()
                .to_string()
                .contains("recursive JSON byte-work budget")
        );
        assert!(check_call_limits(&nested, 128, 4, 12).is_ok());
    }

    #[tokio::test]
    async fn ordinary_nested_create_fits_aggregate_budget_and_preserves_call() -> anyhow::Result<()>
    {
        let (_, client, snapshot) = fixture(2_000_000, 100).await?;
        let create = CreateCrowdloan {
            deposit_tao: Balance::<Tao>::ZERO.into(),
            min_contribution_tao: Balance::<Tao>::ZERO.into(),
            cap_tao: Balance::<Tao>::ZERO.into(),
            end: 1,
            target_ss58: None,
            call: Some(json!({
                "op":"create_crowdloan", "deposit_tao":"0", "min_contribution_tao":"0",
                "cap_tao":"0", "end":2, "target_ss58":OTHER.to_string()
            })),
        };
        let cx = BuildCx {
            view: &snapshot,
            origin: OriginView::Account(ORIGIN),
            runtime: &snapshot.block().runtime,
            clock: &SystemClock,
            spec: CreateCrowdloan::SPEC,
        };
        assert!(check_call_budget(create.call.as_ref().unwrap()).is_ok());
        let inner = crowdloan::create(
            TaoBalance(0),
            TaoBalance(0),
            TaoBalance(0),
            2,
            None,
            Some(OTHER),
        )
        .compose(cx.runtime)?;
        let expected = crowdloan::create(
            TaoBalance(0),
            TaoBalance(0),
            TaoBalance(0),
            1,
            Some(inner),
            None,
        )
        .compose(cx.runtime)?;
        let built = create.build(&cx).await?;
        assert_eq!(built.call.as_bytes(), expected.as_bytes());
        assert!(built.extras.is_empty());
        client.close().await;
        Ok(())
    }

    #[test]
    fn tool_constructor_bounds_nested_call_before_serde_conversion() {
        let mut call = Value::Null;
        for _ in 0..MAX_CALL_DEPTH {
            call = Value::Array(vec![call]);
        }
        let mut args = json!({
            "deposit_tao":"0", "min_contribution_tao":"0", "cap_tao":"0", "end":0
        });
        args["call"] = call;
        assert!(
            crate::AnyIntent::from_tool("create_crowdloan", args)
                .unwrap_err()
                .to_string()
                .contains("depth budget")
        );
        for key in [
            "$serde_json::private::RawValue",
            "$serde_json::private::Number",
        ] {
            let mut fields = serde_json::Map::new();
            fields.insert(key.into(), json!("{\"op\":\"finalize_crowdloan\"}"));
            let mut args = json!({
                "deposit_tao":"0", "min_contribution_tao":"0", "cap_tao":"0", "end":0
            });
            args["call"] = Value::Object(fields);
            assert!(
                crate::AnyIntent::from_tool("create_crowdloan", args)
                    .unwrap_err()
                    .to_string()
                    .contains("private JSON carrier")
            );
        }
    }
}
