// Derived from RaoFoundation/subtensor runtime/src/transaction_payment_wrapper.rs @ 8cf229f8ef969389d054a4c161b53ffd164a041f (Apache-2.0); modified by rtao.
//! Authenticated source policy used by client fee defaults and documentation.
//!
//! This validates reviewed runtime AST shapes; it neither executes the extension
//! nor resolves payers, discounts, account debits, refunds or pool admission.

use std::path::Path;

use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use syn::{ImplItem, Item, ItemImpl};
use toml_edit::DocumentMut;

use super::{Resolver, document, tokens};

const SOURCE: &str = "runtime/src/transaction_payment_wrapper.rs";

// Reviewed executable declarations and methods only. syn ignores ordinary
// comments; method document attributes are excluded below. Exact signatures, bindings,
// implementation owners/bounds and non-document attributes remain significant.
// Other extension methods stay runtime-owned and are not projected here.
const EXPECTED: &str = r###"use crate::{NORMAL_DISPATCH_BASE_PRIORITY, OPERATIONAL_DISPATCH_PRIORITY, Weight};
use codec::{Decode, DecodeWithMemTracking, Encode};
use frame_election_provider_support::private::sp_arithmetic::traits::SaturatedConversion;
use frame_support::dispatch::{DispatchClass, DispatchInfo, PostDispatchInfo};
use frame_support::pallet_prelude::TypeInfo;
use frame_support::traits::{Get, IsSubType, IsType};
use pallet_subtensor_proxy as pallet_proxy;
use pallet_subtensor_utility as pallet_utility;
use pallet_transaction_payment::OnChargeTransaction;
use pallet_transaction_payment::{ChargeTransactionPayment, Config, Pre, Val};
use sp_runtime::DispatchResult;
use sp_runtime::traits::{
    AsSystemOriginSigner, DispatchInfoOf, DispatchOriginOf, Dispatchable, Implication,
    PostDispatchInfoOf, StaticLookup, TransactionExtension, TransactionExtensionMetadata,
    ValidateResult, Zero,
};
use sp_runtime::transaction_validity::{
    TransactionPriority, TransactionSource, TransactionValidity, TransactionValidityError,
};
use sp_std::vec::Vec;
use subtensor_macros::freeze_struct;

type BalanceOf<T> = <<T as Config>::OnChargeTransaction as OnChargeTransaction<T>>::Balance;

type RuntimeCallOf<T> = <T as frame_system::Config>::RuntimeCall;
type RuntimeOriginOf<T> = <T as frame_system::Config>::RuntimeOrigin;
type AccountIdOf<T> = <T as frame_system::Config>::AccountId;
type LookupOf<T> = <T as frame_system::Config>::Lookup;

const MAX_REAL_PAYS_FEE_PROXY_DEPTH: u8 = 3;

/// Runtime-supplied policy: which calls have their fee charged to the signing
/// hotkey's coldkey. Keeping the concrete list in the runtime (see `fee_filters`)
/// lets this generic extension stay free of a hardcoded allow-list.
pub trait ColdkeyFeeCallFilter<Call> {
    fn charges_coldkey(call: &Call) -> bool;
}

/// Weight subsidies apply only to fees, never to block admission or execution accounting.
pub trait FeeWeightDiscount<Call> {
    /// Weight to subtract from `info.call_weight` before pricing the fee.
    fn fee_weight_discount(call: &Call, info: &DispatchInfo) -> Weight;
    fn fee_discount_overhead(call: &Call) -> Weight;
}

pub fn fee_dispatch_info(info: &DispatchInfo, discount: Weight) -> DispatchInfo {
    DispatchInfo {
        call_weight: info.call_weight.saturating_sub(discount),
        ..*info
    }
}

#[freeze_struct("f003cde1f9da4a90")]
#[derive(Encode, Decode, DecodeWithMemTracking, Clone, Eq, PartialEq, TypeInfo)]
#[scale_info(skip_type_params(T))]
pub struct ChargeTransactionPaymentWrapper<T: Config> {
    inner: ChargeTransactionPayment<T>,
}

impl<T: Config + pallet_proxy::Config + pallet_utility::Config + pallet_subtensor::Config>
    ChargeTransactionPaymentWrapper<T>
where
    RuntimeCallOf<T>: IsSubType<pallet_proxy::Call<T>> + IsSubType<pallet_utility::Call<T>>,
    T: ColdkeyFeeCallFilter<RuntimeCallOf<T>>,
    RuntimeOriginOf<T>: AsSystemOriginSigner<AccountIdOf<T>> + Clone,
{
    fn resolve_proxy_call(call: &RuntimeCallOf<T>) -> Option<(AccountIdOf<T>, &RuntimeCallOf<T>)> {
        match call.is_sub_type()? {
            pallet_proxy::Call::proxy { real, call, .. } => {
                let real = LookupOf::<T>::lookup(real.clone()).ok()?;
                Some((real, call.as_ref().into_ref()))
            }
            _ => None,
        }
    }
    fn resolve_real_fee_payer(
        call: &RuntimeCallOf<T>,
        origin: &RuntimeOriginOf<T>,
    ) -> Option<AccountIdOf<T>> {
        let signer = origin.as_system_origin_signer()?;
        let (mut real, mut inner_call) = Self::resolve_proxy_call(call)?;

        if !pallet_proxy::Pallet::<T>::is_real_pays_fee(&real, signer) {
            return None;
        }

        for _ in 1..MAX_REAL_PAYS_FEE_PROXY_DEPTH {
            let Some((inner_real, next_call)) = Self::resolve_proxy_call(inner_call) else {
                return Self::resolve_batch_proxy_payer(inner_call, &real).or(Some(real));
            };
            if !pallet_proxy::Pallet::<T>::is_real_pays_fee(&inner_real, &real) {
                break;
            }

            real = inner_real;
            inner_call = next_call;
        }

        Some(real)
    }
    fn resolve_batch_proxy_payer(
        inner_call: &RuntimeCallOf<T>,
        outer_real: &AccountIdOf<T>,
    ) -> Option<AccountIdOf<T>> {
        let calls: &Vec<<T as pallet_utility::Config>::RuntimeCall> =
            match inner_call.is_sub_type()? {
                pallet_utility::Call::batch { calls }
                | pallet_utility::Call::batch_all { calls }
                | pallet_utility::Call::force_batch { calls } => calls,
                _ => return None,
            };

        if calls.is_empty() {
            return None;
        }

        let mut common_real: Option<AccountIdOf<T>> = None;

        for call in calls.iter() {
            let call_ref: &RuntimeCallOf<T> = call.into_ref();
            let (inner_real, _) = Self::resolve_proxy_call(call_ref)?;

            match &common_real {
                None => {
                    // Every item must have the same real and implicit delegate, so one
                    // RealPaysFee read covers the entire batch.
                    if !pallet_proxy::Pallet::<T>::is_real_pays_fee(&inner_real, outer_real) {
                        return None;
                    }
                    common_real = Some(inner_real);
                }
                // All items must share the same real account.
                Some(existing) if *existing != inner_real => return None,
                _ => {}
            }
        }

        common_real
    }
    fn extract_coldkey_fee_payer(
        call: &RuntimeCallOf<T>,
        origin: &RuntimeOriginOf<T>,
    ) -> Option<AccountIdOf<T>> {
        if !T::charges_coldkey(call) {
            return None;
        }

        let signer = origin.as_system_origin_signer()?;
        pallet_subtensor::Pallet::<T>::maybe_coldkey_for_hotkey(signer)
    }
}
impl<T: Config + pallet_proxy::Config + pallet_utility::Config + pallet_subtensor::Config>
    TransactionExtension<RuntimeCallOf<T>> for ChargeTransactionPaymentWrapper<T>
where
    RuntimeCallOf<T>: Dispatchable<Info = DispatchInfo, PostInfo = PostDispatchInfo>
        + IsSubType<pallet_proxy::Call<T>>
        + IsSubType<pallet_utility::Call<T>>,
    T: ColdkeyFeeCallFilter<RuntimeCallOf<T>> + FeeWeightDiscount<RuntimeCallOf<T>>,
    BalanceOf<T>: Zero + Send + Sync,
    RuntimeOriginOf<T>: AsSystemOriginSigner<AccountIdOf<T>>
        + Clone
        + From<frame_system::RawOrigin<AccountIdOf<T>>>,
{
    const IDENTIFIER: &'static str = "ChargeTransactionPaymentWrapper";
    type Implicit = ();
    type Val = Val<T>;
    type Pre = (Pre<T>, Weight);

    fn validate(
        &self,
        origin: DispatchOriginOf<RuntimeCallOf<T>>,
        call: &RuntimeCallOf<T>,
        info: &DispatchInfoOf<RuntimeCallOf<T>>,
        len: usize,
        self_implicit: Self::Implicit,
        inherited_implication: &impl Implication,
        source: TransactionSource,
    ) -> ValidateResult<Self::Val, RuntimeCallOf<T>> {
        let overridden_priority = {
            let base: TransactionPriority = match info.class {
                DispatchClass::Normal => NORMAL_DISPATCH_BASE_PRIORITY,
                DispatchClass::Mandatory => NORMAL_DISPATCH_BASE_PRIORITY,
                DispatchClass::Operational => OPERATIONAL_DISPATCH_PRIORITY,
            };
            base.saturated_into::<TransactionPriority>()
        };

        // Resolve the fee payer. A proxy `RealPaysFee` opt-in takes precedence; otherwise an
        // owned hotkey's coldkey pays for allow-listed calls. Whenever the payer is not the
        // signer, only the protocol fee is redirected — the signer-chosen tip is dropped. A tip
        // does not buy priority in this wrapper (priority is overridden above), and billing it
        // to another account would let the signer spend that account's funds without limit,
        // regardless of what the proxy type allows. Only a self-paying signer keeps its tip.
        let (fee_origin, tip) = if let Some(real) = Self::resolve_real_fee_payer(call, &origin) {
            (frame_system::RawOrigin::Signed(real).into(), Zero::zero())
        } else if let Some(coldkey) = Self::extract_coldkey_fee_payer(call, &origin) {
            (
                frame_system::RawOrigin::Signed(coldkey).into(),
                Zero::zero(),
            )
        } else {
            (origin.clone(), self.inner.tip())
        };

        let fee_info = fee_dispatch_info(info, T::fee_weight_discount(call, info));
        let (mut valid_transaction, val, _fee_origin) = ChargeTransactionPayment::<T>::from(tip)
            .validate(
                fee_origin,
                call,
                &fee_info,
                len,
                self_implicit,
                inherited_implication,
                source,
            )?;

        valid_transaction.priority = overridden_priority;

        // Always return the original origin so the actual signer remains
        // the origin for dispatch and all subsequent extensions.
        Ok((valid_transaction, val, origin))
    }
    fn metadata() -> Vec<TransactionExtensionMetadata> {
        ChargeTransactionPayment::<T>::metadata()
    }
}
impl<T: Config> core::fmt::Debug for ChargeTransactionPaymentWrapper<T> {}
impl<T: Config> ChargeTransactionPaymentWrapper<T>
where
    T::RuntimeCall: Dispatchable<Info = DispatchInfo, PostInfo = PostDispatchInfo>,
    BalanceOf<T>: Send + Sync,
{}
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {}
"###;

fn implementation_header(item: &ItemImpl) -> String {
    let mut header = item.clone();
    header.items.clear();
    tokens(&header)
}

fn without_docs(item: &ImplItem) -> ImplItem {
    let mut item = item.clone();
    match &mut item {
        ImplItem::Fn(method) => method.attrs.retain(|attr| !attr.path().is_ident("doc")),
        ImplItem::Const(value) => value.attrs.retain(|attr| !attr.path().is_ident("doc")),
        ImplItem::Type(value) => value.attrs.retain(|attr| !attr.path().is_ident("doc")),
        _ => {}
    }
    item
}

fn item_name(item: &ImplItem) -> Option<String> {
    match item {
        ImplItem::Fn(method) => Some(method.sig.ident.to_string()),
        ImplItem::Const(value) => Some(value.ident.to_string()),
        ImplItem::Type(value) => Some(value.ident.to_string()),
        _ => None,
    }
}

fn production_context(file: &syn::File) -> Result<Vec<String>> {
    ensure!(
        file.attrs.is_empty(),
        "conditional payment source unsupported"
    );
    file.items
        .iter()
        .map(|item| match item {
            Item::Use(_)
            | Item::Type(_)
            | Item::Const(_)
            | Item::Trait(_)
            | Item::Struct(_)
            | Item::Fn(_) => Ok(tokens(item)),
            Item::Impl(item) => Ok(implementation_header(item)),
            Item::Mod(item) if item.ident == "tests" && item.content.is_some() => {
                let mut header = item.clone();
                header.content = Some((syn::token::Brace::default(), Vec::new()));
                Ok(tokens(&header))
            }
            _ => bail!("unsupported payment source declaration"),
        })
        .collect()
}

fn values(source: &str) -> Result<Value> {
    let file = syn::parse_file(source)?;
    let expected = syn::parse_file(EXPECTED)?;
    for implementation in file.items.iter().filter_map(|item| match item {
        Item::Impl(item) => Some(item),
        _ => None,
    }) {
        ensure!(
            implementation.items.iter().all(|item| matches!(
                item,
                ImplItem::Fn(_) | ImplItem::Const(_) | ImplItem::Type(_)
            )),
            "unsupported payment implementation item"
        );
    }
    let mut context = production_context(&file)?;
    let mut expected_context = production_context(&expected)?;
    context.sort();
    expected_context.sort();
    ensure!(
        context == expected_context,
        "payment policy declarations/imports/owners changed"
    );
    for expected_impl in expected.items.iter().filter_map(|item| match item {
        Item::Impl(item) => Some(item),
        _ => None,
    }) {
        let header = implementation_header(expected_impl);
        let matching = file
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Impl(item) if implementation_header(item) == header => Some(item),
                _ => None,
            })
            .collect::<Vec<_>>();
        ensure!(
            matching.len() == 1,
            "payment policy implementation changed or duplicated"
        );
        for expected_item in &expected_impl.items {
            let name = item_name(expected_item).context("unsupported expected payment item")?;
            let matching = matching[0]
                .items
                .iter()
                .filter(|item| item_name(item).as_deref() == Some(name.as_str()))
                .collect::<Vec<_>>();
            ensure!(
                matching.len() == 1
                    && tokens(&without_docs(matching[0])) == tokens(&without_docs(expected_item)),
                "payment policy method or associated item changed: {name}"
            );
        }
    }
    Ok(json!({
        "priority": {"Normal":"NORMAL_DISPATCH_BASE_PRIORITY", "Mandatory":"NORMAL_DISPATCH_BASE_PRIORITY", "Operational":"OPERATIONAL_DISPATCH_PRIORITY", "tip_increases_priority":false},
        "payer_precedence":["proxy_real_pays_fee_opt_in", "allowlisted_hotkey_owner", "signer"],
        "proxy": {"call":"Proxy.proxy", "proxy_announced":false, "maximum_layers":3, "outer_opt_in_required":true, "every_hop_opt_in_required":true, "payer":"deepest consecutively opted-in real", "batch_calls":["Utility.batch", "Utility.batch_all", "Utility.force_batch"], "batch_nonempty":true, "batch_position":"final level only; items are never descended", "batch_members":"all direct proxies with the same real and shared opt-in"},
        "tip": {"payer_override":0, "no_payer_override":"signer_selected"},
        "dispatch_origin":"original_origin",
        "fee_pricing":"call_weight.saturating_sub(runtime_fee_weight_discount)",
        "metadata":"ChargeTransactionPayment::metadata"
    }))
}

pub(super) fn output(root: &Path, pins: &DocumentMut) -> Result<Value> {
    let resolver = Resolver::new(root)?;
    let mut result = document(&resolver, pins, SOURCE, values(&resolver.source(SOURCE)?)?);
    result["scope"] = json!(
        "Reviewed validate-time payer/priority/tip policy and fee-dispatch projection only. Concrete priority values, discount implementations, preparation, account debits/refunds and runtime admission are runtime-owned and unexecuted; the client's any-leaf coldkey warning is not a payer resolver."
    );
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> Result<String> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .context("workspace root missing")?;
        Resolver::new(root)?.source(SOURCE)
    }

    #[test]
    fn payment_policy_projection_preserves_symbolic_priority_tip_and_origin_contracts() -> Result<()>
    {
        let projected = values(&source()?)?;
        ensure!(projected["priority"]["tip_increases_priority"] == false);
        ensure!(projected["priority"]["Normal"] == projected["priority"]["Mandatory"]);
        ensure!(
            projected["payer_precedence"]
                == json!([
                    "proxy_real_pays_fee_opt_in",
                    "allowlisted_hotkey_owner",
                    "signer"
                ])
        );
        ensure!(projected["tip"]["payer_override"] == 0);
        ensure!(projected["dispatch_origin"] == "original_origin");
        ensure!(projected["proxy"]["maximum_layers"] == 3);
        ensure!(projected["proxy"]["every_hop_opt_in_required"] == true);
        ensure!(projected["proxy"]["batch_nonempty"] == true);
        Ok(())
    }

    #[test]
    fn payment_policy_projection_rejects_routing_tip_priority_and_context_drift() -> Result<()> {
        let source = source()?;
        for (before, after) in [
            (
                "pallet_proxy::Call::proxy { real, call, .. }",
                "pallet_proxy::Call::proxy_announced { real, call, .. }",
            ),
            (
                "if !pallet_proxy::Pallet::<T>::is_real_pays_fee(&real, signer)",
                "if pallet_proxy::Pallet::<T>::is_real_pays_fee(&real, signer)",
            ),
            (
                "if !pallet_proxy::Pallet::<T>::is_real_pays_fee(&inner_real, &real)",
                "if !pallet_proxy::Pallet::<T>::is_real_pays_fee(&inner_real, signer)",
            ),
            (
                "const MAX_REAL_PAYS_FEE_PROXY_DEPTH: u8 = 3;",
                "const MAX_REAL_PAYS_FEE_PROXY_DEPTH: u8 = 4;",
            ),
            (
                "for _ in 1..MAX_REAL_PAYS_FEE_PROXY_DEPTH",
                "for _ in 0..MAX_REAL_PAYS_FEE_PROXY_DEPTH",
            ),
            (
                "Self::resolve_real_fee_payer(call, &origin)",
                "Self::extract_coldkey_fee_payer(call, &origin)",
            ),
            (
                "for call in calls.iter()",
                "for call in calls.iter().take(1)",
            ),
            ("if calls.is_empty()", "if !calls.is_empty()"),
            (
                "Some(existing) if *existing != inner_real",
                "Some(existing) if *existing == inner_real",
            ),
            (
                "Self::resolve_proxy_call(call_ref)?",
                "Self::resolve_proxy_call(call_ref).unwrap()",
            ),
            (
                "Self::resolve_batch_proxy_payer(inner_call, &real).or(Some(real))",
                "Self::resolve_real_fee_payer(inner_call, origin)",
            ),
            (
                "if !T::charges_coldkey(call)",
                "if T::charges_coldkey(call)",
            ),
            (
                "(frame_system::RawOrigin::Signed(real).into(), Zero::zero())",
                "(frame_system::RawOrigin::Signed(real).into(), self.inner.tip())",
            ),
            (
                "(origin.clone(), self.inner.tip())",
                "(origin.clone(), Zero::zero())",
            ),
            (
                "valid_transaction.priority = overridden_priority",
                "valid_transaction.priority += self.inner.tip()",
            ),
            (
                "DispatchClass::Operational => OPERATIONAL_DISPATCH_PRIORITY",
                "DispatchClass::Operational => NORMAL_DISPATCH_BASE_PRIORITY",
            ),
            (
                "Ok((valid_transaction, val, origin))",
                "Ok((valid_transaction, val, _fee_origin))",
            ),
            (
                "call_weight: info.call_weight.saturating_sub(discount)",
                "call_weight: info.call_weight.saturating_add(discount)",
            ),
            ("ChargeTransactionPayment::<T>::metadata()", "Vec::new()"),
            (
                "call: &RuntimeCallOf<T>,\n        origin: &RuntimeOriginOf<T>,",
                "_call: &RuntimeCallOf<T>,\n        origin: &RuntimeOriginOf<T>,",
            ),
            (
                "    fn validate(",
                "    #[cfg(feature = \"alternate-payment\")]\n    fn validate(",
            ),
            (
                "    TransactionExtension<RuntimeCallOf<T>> for ChargeTransactionPaymentWrapper<T>",
                "    TransactionExtension<RuntimeCallOf<T>> for OtherPaymentWrapper<T>",
            ),
            ("type BalanceOf<T> =", "type OtherBalanceOf<T> ="),
            (
                "use pallet_subtensor_proxy as pallet_proxy;",
                "use other_proxy as pallet_proxy;",
            ),
        ] {
            ensure!(source.contains(before), "mutation anchor absent: {before}");
            ensure!(
                values(&source.replacen(before, after, 1)).is_err(),
                "accepted payment policy drift: {before}"
            );
        }
        for extra in [
            "mod pallet_subtensor {}",
            "extern crate other as pallet_subtensor;",
            "macro_rules! replacement { () => {} }",
            "enum Added { Case }",
            "union Added { field: u8 }",
            "const ADDED: u8 = 0;",
            "static ADDED: u8 = 0;",
            "impl Added {}",
            "impl<T: Config> ChargeTransactionPaymentWrapper<T> { invoke_policy!(); }",
        ] {
            ensure!(
                values(&format!("{source}\n{extra}")).is_err(),
                "accepted unsupported source context: {extra}"
            );
        }
        ensure!(
            values(&format!(
                "#![cfg(feature = \"alternate-payment\")]\n{source}"
            ))
            .is_err()
        );
        Ok(())
    }
}
