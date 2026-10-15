// Derived from RaoFoundation/subtensor sdk/python/bittensor/executor.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Origin wrappers that preserve the encoded inner call and runtime stamp.

use crate::{ClientError, View, ViewExt};
use rtao_core::{CoreError, DynValue, codec::Value, value_ext::ValueExt};
use rtao_primitives::{AccountId, H256, IntoCoreValue};
use rtao_runtime::models::{MultisigTimepoint, ProxyType};
use rtao_runtime::{CallBytes, ComposeError, MetadataLevel, RuntimeHandle};

/// Wrap a call in `Proxy.proxy` using the live runtime's permission enum.
pub fn proxy(
    runtime: &RuntimeHandle,
    real: AccountId,
    force: Option<ProxyType>,
    inner: CallBytes,
) -> Result<CallBytes, ComposeError> {
    inner.ensure_runtime(runtime)?;
    rtao_runtime::calls::proxy::proxy(DynValue::new(real.into_core_value()), force, inner)
        .compose(runtime)
}

/// Compose an atomic batch, refusing a batch anywhere inside a child wrapper.
pub fn batch_all(
    runtime: &RuntimeHandle,
    calls: Vec<CallBytes>,
) -> Result<CallBytes, ComposeError> {
    for call in &calls {
        call.ensure_runtime(runtime)?;
        let decoded = runtime
            .core()
            .decode_spec(&rtao_core::runtime::TypeSpec::Call, call.as_bytes(), true)
            .map_err(CoreError::from)?;
        let mut values = vec![&decoded];
        while let Some(value) = values.pop() {
            if value.field("call_module").and_then(ValueExt::as_str) == Some("Utility")
                && matches!(
                    value.field("call_function").and_then(ValueExt::as_str),
                    Some("batch" | "batch_all" | "force_batch")
                )
            {
                return Err(CoreError::Codec("nested batches are not supported".into()).into());
            }
            match value {
                Value::Dict(fields) => values.extend(fields.iter().map(|(_, value)| value)),
                Value::List(items) | Value::Tuple(items) => values.extend(items),
                _ => {}
            }
        }
    }
    rtao_runtime::calls::utility::batch_all(calls).compose(runtime)
}

/// Compose a threshold-one multisig dispatch with the sorted other signatories.
pub fn as_multi_threshold_1(
    runtime: &RuntimeHandle,
    others: &[AccountId],
    inner: CallBytes,
) -> Result<CallBytes, ComposeError> {
    inner.ensure_runtime(runtime)?;
    if others.is_empty() {
        return Err(
            CoreError::Codec("multisig requires at least one other signatory".into()).into(),
        );
    }
    validate_multisig_signatory_count(runtime, others.len().saturating_add(1))?;
    let mut others = others.to_vec();
    others.sort_unstable_by_key(|account| account.0);
    if others.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(CoreError::Codec("duplicate multisig signatory".into()).into());
    }
    rtao_runtime::calls::multisig::as_multi_threshold_1(others, inner).compose(runtime)
}

/// Check a complete signer set against this runtime's `MaxSignatories`.
///
/// The pallet requires `other_signatories.len() < MaxSignatories`, including
/// threshold-one calls. The submitting signer is the remaining member.
/// Missing, changed-type or malformed constants refuse account construction
/// and approval composition; a changed value is enforced from this runtime.
pub fn validate_multisig_signatory_count(
    runtime: &RuntimeHandle,
    count: usize,
) -> Result<(), ComposeError> {
    let expected = rtao_runtime::constants::multisig::MAX_SIGNATORIES.surface();
    let constant = runtime
        .core()
        .constant(expected.pallet, expected.name)
        .ok_or_else(|| CoreError::NotInRuntime("constant Multisig.MaxSignatories".into()))?;
    let shape = runtime
        .exact_shape(constant.ty)
        .map_err(|error| CoreError::Codec(error.to_string()))?;
    if shape != expected.shape {
        return Err(
            CoreError::Codec("Multisig.MaxSignatories is not a u32 constant".into()).into(),
        );
    }
    let max = runtime
        .core()
        .decode_spec(
            &rtao_core::runtime::TypeSpec::Id(constant.ty),
            &constant.value,
            true,
        )
        .map_err(CoreError::from)?
        .as_u128()
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| CoreError::Codec("Multisig.MaxSignatories is not a u32 value".into()))?;
    if count as u128 > u128::from(max) {
        return Err(CoreError::Codec(format!(
            "multisig has {count} signatories, exceeding this runtime's MaxSignatories ({max})"
        ))
        .into());
    }
    Ok(())
}

fn multisig_members(
    runtime: &RuntimeHandle,
    signer: AccountId,
    signatories: &[AccountId],
    threshold: u16,
) -> Result<(AccountId, Vec<AccountId>), ClientError> {
    if signatories.len() < 2 {
        return Err(ClientError::Protocol(
            "multisig requires at least two signatories".into(),
        ));
    }
    if threshold == 0 || usize::from(threshold) > signatories.len() {
        return Err(ClientError::Protocol(
            "multisig threshold is outside the signer set".into(),
        ));
    }
    validate_multisig_signatory_count(runtime, signatories.len())?;
    let keys = signatories
        .iter()
        .map(|account| account.0)
        .collect::<Vec<_>>();
    let (account, sorted) =
        rtao_core::extrinsic::multisig_account_id(&keys, threshold).map_err(CoreError::from)?;
    if sorted.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(ClientError::Protocol("duplicate multisig signatory".into()));
    }
    if !sorted.contains(&signer.0) {
        return Err(ClientError::Protocol(
            "signer is not a multisig signatory".into(),
        ));
    }
    Ok((
        AccountId(account),
        sorted
            .into_iter()
            .filter(|key| *key != signer.0)
            .map(AccountId)
            .collect(),
    ))
}

/// Compose a full-call multisig approval using pending state from one pinned view.
///
/// Threshold one uses `as_multi_threshold_1`, which has no opening deposit or
/// pending timepoint. Higher thresholds preserve the existing opening timepoint.
pub async fn as_multi<V: View + ?Sized>(
    view: &V,
    signer: AccountId,
    signatories: &[AccountId],
    threshold: u16,
    inner: CallBytes,
    estimate: &rtao_core::ops::fee::FeeEstimate,
) -> Result<CallBytes, ClientError> {
    let snapshot = view.pin().await?;
    let runtime = &snapshot.block().runtime;
    inner.ensure_runtime(runtime)?;
    let (account, others) = multisig_members(runtime, signer, signatories, threshold)?;
    if threshold == 1 {
        return as_multi_threshold_1(runtime, &others, inner).map_err(ClientError::from);
    }
    let pending = snapshot
        .storage(
            &rtao_runtime::storage::multisig::MULTISIGS,
            (account, inner.hash()),
        )
        .await?;
    rtao_runtime::calls::multisig::as_multi(
        threshold,
        others,
        pending.map(|pending| pending.when),
        inner,
        DynValue::new(estimate.weight.clone()),
    )
    .compose(runtime)
    .map_err(ClientError::from)
}

/// Compose a hash-only multisig approval with an authenticated opening timepoint.
///
/// A hash-only approval cannot execute the inner call at the threshold. The
/// semantic `multisig_approve` intent supplies that warning to its plan.
pub async fn approve_as_multi<V: View + ?Sized>(
    view: &V,
    signer: AccountId,
    signatories: &[AccountId],
    threshold: u16,
    call_hash: H256,
    timepoint: MultisigTimepoint,
    estimate: &rtao_core::ops::fee::FeeEstimate,
) -> Result<CallBytes, ClientError> {
    let snapshot = view.pin().await?;
    let runtime = &snapshot.block().runtime;
    let (_, others) = multisig_members(runtime, signer, signatories, threshold)?;
    if threshold < 2 {
        return Err(ClientError::Protocol(
            "hash-only multisig approval requires threshold at least two".into(),
        ));
    }
    rtao_runtime::calls::multisig::approve_as_multi(
        threshold,
        others,
        Some(timepoint),
        call_hash.0,
        DynValue::new(estimate.weight.clone()),
    )
    .compose(runtime)
    .map_err(ClientError::from)
}

/// Wrap a root-origin call in `Sudo.sudo`, preserving an existing Sudo wrapper.
///
/// Live metadata identifies the wrapper. Both runtime version stamps must match,
/// including when the call is already wrapped.
pub fn sudo(runtime: &RuntimeHandle, inner: CallBytes) -> Result<CallBytes, ComposeError> {
    inner.ensure_runtime(runtime)?;
    if runtime.metadata_level() != MetadataLevel::V15 {
        return Err(ComposeError::MetadataDegraded);
    }
    runtime
        .signing_status()
        .map_err(ComposeError::SigningDisabled)?;
    if inner.as_bytes().first().copied() == runtime.pallet_index("Sudo")
        && inner.as_bytes().get(1).copied() == runtime.call_index("Sudo", "sudo")
    {
        return Ok(inner);
    }
    rtao_runtime::calls::sudo::sudo(inner).compose(runtime)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[allow(clippy::unwrap_used, clippy::panic)]
    async fn malformed_or_missing_cap_metadata_refuses_every_approval_composer() {
        use frame_metadata::{RuntimeMetadata, RuntimeMetadataPrefixed};
        use parity_scale_codec::{Decode, Encode};
        use rtao_testkit::{FakeChain, FakeState, rpc::RpcClient};

        for mutation in ["missing", "truncated", "trailing", "type"] {
            let mut metadata = RuntimeMetadataPrefixed::decode(
                &mut &include_bytes!("../../../vendor/metadata/finney/478.v15.scale")[..],
            )
            .unwrap();
            let RuntimeMetadata::V15(v15) = &mut metadata.1 else {
                panic!("V15 fixture required")
            };
            let other_type = v15
                .pallets
                .iter()
                .find(|pallet| pallet.name == "Balances")
                .unwrap()
                .constants
                .iter()
                .find(|constant| constant.name == "ExistentialDeposit")
                .unwrap()
                .ty;
            let constants = &mut v15
                .pallets
                .iter_mut()
                .find(|pallet| pallet.name == "Multisig")
                .unwrap()
                .constants;
            if mutation == "missing" {
                constants.retain(|constant| constant.name != "MaxSignatories");
            } else {
                let cap = constants
                    .iter_mut()
                    .find(|constant| constant.name == "MaxSignatories")
                    .unwrap();
                match mutation {
                    "truncated" => {
                        cap.value.pop();
                    }
                    "trailing" => cap.value.push(0),
                    "type" => cap.ty = other_type,
                    _ => unreachable!(),
                }
            }
            // Serve authentic SCALE metadata through the real client cache.
            let mut state = FakeState::default();
            state.runtime = std::sync::Arc::new(
                rtao_core::runtime::Runtime::parse(&metadata.encode(), 478, 1, 42).unwrap(),
            );
            let fake = FakeChain::from_state(state);
            let mut config =
                crate::ClientConfig::new(crate::Network::resolve_with("local", |_| None).unwrap())
                    .unwrap();
            config.track_heads = false;
            config.runtime_cache_dir = None;
            config.config_store = None;
            let client = crate::Client::from_rpc(RpcClient::new(fake.clone()), config)
                .await
                .unwrap();
            let snapshot = client.pin().await.unwrap();
            let runtime = &snapshot.block().runtime;
            let inner = rtao_runtime::calls::system::remark(vec![1])
                .compose(runtime)
                .unwrap();
            let signer = AccountId([1; 32]);
            let other = AccountId([2; 32]);
            let estimate = rtao_core::ops::fee::FeeEstimate {
                weight: Value::record(vec![
                    ("ref_time".into(), Value::Int(1)),
                    ("proof_size".into(), Value::Int(0)),
                ]),
                partial_fee: 0,
                class: Value::str("Normal"),
            };
            assert!(
                as_multi_threshold_1(runtime, &[other], inner.clone()).is_err(),
                "{mutation}"
            );
            assert!(
                as_multi(&snapshot, signer, &[signer, other], 2, inner, &estimate)
                    .await
                    .is_err(),
                "{mutation}"
            );
            assert!(
                approve_as_multi(
                    &snapshot,
                    signer,
                    &[signer, other],
                    2,
                    H256([7; 32]),
                    MultisigTimepoint {
                        height: 12,
                        index: 3
                    },
                    &estimate,
                )
                .await
                .is_err(),
                "{mutation}"
            );
            let state = fake.state();
            let state = state.lock().unwrap();
            assert!(state.submissions.is_empty());
            assert!(
                !state
                    .requests
                    .iter()
                    .any(|request| request.method == "state_queryStorageAt")
            );
        }
    }

    #[test]
    fn root_wrapper_preserves_inner_bytes_and_never_wraps_twice()
    -> Result<(), Box<dyn std::error::Error>> {
        let runtime = RuntimeHandle::parse(
            include_bytes!("../../../vendor/metadata/finney/478.v15.scale"),
            478,
            1,
            42,
        )?;
        let inner = rtao_runtime::calls::system::remark(vec![1, 2, 3]).compose(&runtime)?;
        let once = sudo(&runtime, inner.clone())?;
        assert!(once.as_bytes().ends_with(inner.as_bytes()));
        assert_eq!(once.as_bytes().len(), inner.as_bytes().len() + 2);
        assert_eq!(sudo(&runtime, once.clone())?, once);
        let upgraded = RuntimeHandle::parse(
            include_bytes!("../../../vendor/metadata/finney/478.v15.scale"),
            479,
            1,
            42,
        )?;
        assert!(matches!(
            sudo(&upgraded, once),
            Err(ComposeError::VersionMismatch { .. })
        ));
        Ok(())
    }
}
