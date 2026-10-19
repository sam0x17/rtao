// Derived from RaoFoundation/subtensor sdk/python/bittensor/executor.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/multisig.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/_transport/interface.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
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

/// Continue a specific pending full-call multisig operation without reopening it.
///
/// Automatic later approvals must retain the opening timepoint reviewed by the
/// first round. One pinned lookup authenticates that exact pending operation.
/// Missing, replaced or already-approved state refuses signing. The encoded
/// call always carries `Some(expected)`, so removal after composition cannot
/// turn it into a new opening approval. Threshold-one and intentional new operations use
/// [`as_multi_threshold_1`] and [`as_multi`] respectively (D-041).
pub async fn as_multi_existing<V: View + ?Sized>(
    view: &V,
    signer: AccountId,
    signatories: &[AccountId],
    threshold: u16,
    inner: CallBytes,
    estimate: &rtao_core::ops::fee::FeeEstimate,
    expected: MultisigTimepoint,
) -> Result<CallBytes, ClientError> {
    let snapshot = view.pin().await?;
    let runtime = &snapshot.block().runtime;
    inner.ensure_runtime(runtime)?;
    let (account, others) = multisig_members(runtime, signer, signatories, threshold)?;
    if threshold < 2 {
        return Err(ClientError::Protocol(
            "multisig continuation requires threshold at least two".into(),
        ));
    }
    let pending = snapshot
        .storage(
            &rtao_runtime::storage::multisig::MULTISIGS,
            (account, inner.hash()),
        )
        .await?;
    match pending {
        Some(pending) if pending.when == expected => {
            if pending.approvals.contains(&signer) {
                return Err(ClientError::SigningDisabled {
                    reason: "this signatory already approved the pending multisig operation; refusing automatic continuation"
                        .into(),
                });
            }
        }
        Some(_) => {
            return Err(ClientError::SigningDisabled {
                reason:
                    "the pending multisig operation was replaced; refusing automatic continuation"
                        .into(),
            });
        }
        None => {
            return Err(ClientError::SigningDisabled {
                reason: "the pending multisig operation is absent; refusing to reopen it during automatic continuation"
                    .into(),
            });
        }
    }
    rtao_runtime::calls::multisig::as_multi(
        threshold,
        others,
        Some(expected),
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

    struct ContinuationFixture {
        client: crate::Client,
        fake: rtao_testkit::FakeChain,
        inner: CallBytes,
        members: [AccountId; 3],
        key: Vec<u8>,
        estimate: rtao_core::ops::fee::FeeEstimate,
    }

    impl ContinuationFixture {
        async fn new() -> Result<Self, Box<dyn std::error::Error>> {
            Self::from_state(rtao_testkit::FakeState::try_default()?).await
        }

        async fn from_state(
            state: rtao_testkit::FakeState,
        ) -> Result<Self, Box<dyn std::error::Error>> {
            use rtao_testkit::{FakeChain, rpc::RpcClient};
            let fake = FakeChain::from_state(state);
            let mut config =
                crate::ClientConfig::new(crate::Network::resolve_with("local", |_| None)?)?;
            config.track_heads = false;
            config.runtime_cache_dir = None;
            config.config_store = None;
            let client = crate::Client::from_rpc(RpcClient::new(fake.clone()), config).await?;
            let runtime = View::runtime(&client).await?;
            let inner = rtao_runtime::calls::system::remark(vec![4, 5, 6]).compose(&runtime)?;
            let members = [AccountId([3; 32]), AccountId([1; 32]), AccountId([2; 32])];
            let (account, _) = rtao_core::extrinsic::multisig_account_id(
                &members.iter().map(|member| member.0).collect::<Vec<_>>(),
                2,
            )?;
            let storage = runtime
                .core()
                .storage_entry("Multisig", "Multisigs")
                .ok_or("Multisig storage fixture missing")?;
            let key = runtime.core().storage_key(
                storage,
                &[
                    AccountId(account).into_core_value(),
                    inner.hash().into_core_value(),
                ],
            )?;
            Ok(Self {
                client,
                fake,
                inner,
                members,
                key,
                estimate: rtao_core::ops::fee::FeeEstimate {
                    weight: Value::record(vec![
                        ("ref_time".into(), Value::Int(123_456)),
                        ("proof_size".into(), Value::Int(789)),
                    ]),
                    partial_fee: 0,
                    class: Value::str("Normal"),
                },
            })
        }

        fn seed(&self, when: &MultisigTimepoint) -> Result<(), Box<dyn std::error::Error>> {
            let hash = self.inner.hash();
            let (account, _) = rtao_core::extrinsic::multisig_account_id(
                &self
                    .members
                    .iter()
                    .map(|member| member.0)
                    .collect::<Vec<_>>(),
                2,
            )?;
            self.fake.mutate_state(|state| {
                state.seed_python("Multisig", "Multisigs", &[
                    serde_json::json!(AccountId(account).to_string()),
                    serde_json::json!(format!("0x{}", hex::encode(hash))),
                ], serde_json::json!({"when":when,"deposit":280_800_000,"depositor":self.members[1].to_string(),"approvals":[self.members[1].to_string()]}))
            })?;
            Ok(())
        }

        async fn decode(
            &self,
            call: &CallBytes,
        ) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
            let runtime = View::runtime(&self.client).await?;
            let decoded = runtime.core().decode_spec(
                &rtao_core::runtime::TypeSpec::Call,
                call.as_bytes(),
                true,
            )?;
            Ok(DynValue::new(decoded).to_json())
        }
    }

    fn decoded_arg<'a>(
        call: &'a serde_json::Value,
        name: &str,
    ) -> Result<&'a serde_json::Value, Box<dyn std::error::Error>> {
        call.get("call_args")
            .and_then(serde_json::Value::as_array)
            .and_then(|args| {
                args.iter()
                    .find(|arg| arg.get("name").and_then(serde_json::Value::as_str) == Some(name))
            })
            .and_then(|arg| arg.get("value"))
            .ok_or_else(|| format!("decoded call argument {name} missing").into())
    }

    #[tokio::test]
    async fn existing_multisig_approval_encodes_exact_timepoint_and_retains_it_after_removal()
    -> Result<(), Box<dyn std::error::Error>> {
        use serde_json::json;
        let fixture = ContinuationFixture::new().await?;
        let expected = MultisigTimepoint {
            height: 12,
            index: 3,
        };
        fixture.seed(&expected)?;
        let call = as_multi_existing(
            &fixture.client,
            fixture.members[0],
            &fixture.members,
            2,
            fixture.inner.clone(),
            &fixture.estimate,
            expected.clone(),
        )
        .await?;
        let encoded = call.as_bytes().to_vec();
        let decoded = fixture.decode(&call).await?;
        assert_eq!(decoded["call_module"], "Multisig");
        assert_eq!(decoded["call_function"], "as_multi");
        assert_eq!(
            decoded_arg(&decoded, "maybe_timepoint")?,
            &json!({"height":12,"index":3})
        );
        assert_eq!(decoded_arg(&decoded, "threshold")?, &json!(2));
        assert_eq!(
            decoded_arg(&decoded, "other_signatories")?,
            &json!([
                fixture.members[1].to_string(),
                fixture.members[2].to_string()
            ])
        );
        assert_eq!(decoded_arg(&decoded, "call")?["call_function"], "remark");
        assert_eq!(
            decoded_arg(&decoded, "max_weight")?,
            &json!({"ref_time":123_456,"proof_size":789})
        );
        fixture
            .fake
            .mutate_state(|state| state.seed_encoded(fixture.key.clone(), None));
        assert_eq!(call.as_bytes(), encoded);
        assert_eq!(
            decoded_arg(&fixture.decode(&call).await?, "maybe_timepoint")?,
            &json!({"height":12,"index":3})
        );
        let refusal = as_multi_existing(
            &fixture.client,
            fixture.members[0],
            &fixture.members,
            2,
            fixture.inner.clone(),
            &fixture.estimate,
            expected,
        )
        .await;
        assert!(matches!(refusal, Err(ClientError::SigningDisabled { .. })));
        // The ordinary composer deliberately retains its opening semantics.
        let fresh = as_multi(
            &fixture.client,
            fixture.members[0],
            &fixture.members,
            2,
            fixture.inner.clone(),
            &fixture.estimate,
        )
        .await?;
        assert!(decoded_arg(&fixture.decode(&fresh).await?, "maybe_timepoint")?.is_null());
        assert!(
            fixture
                .fake
                .state()
                .lock()
                .map_err(|_| "fake lock")?
                .submissions
                .is_empty()
        );
        fixture.client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn existing_multisig_refuses_absent_replaced_and_noncontinuation_inputs()
    -> Result<(), Box<dyn std::error::Error>> {
        let fixture = ContinuationFixture::new().await?;
        let expected = MultisigTimepoint {
            height: 12,
            index: 3,
        };
        for replacement in [
            None,
            Some(MultisigTimepoint {
                height: 13,
                index: 3,
            }),
            Some(MultisigTimepoint {
                height: 12,
                index: 4,
            }),
        ] {
            if let Some(when) = replacement {
                fixture.seed(&when)?;
            }
            let result = as_multi_existing(
                &fixture.client,
                fixture.members[0],
                &fixture.members,
                2,
                fixture.inner.clone(),
                &fixture.estimate,
                expected.clone(),
            )
            .await;
            assert!(matches!(result, Err(ClientError::SigningDisabled { .. })));
        }
        fixture.seed(&expected)?;
        fixture.fake.mutate_state(|state| state.requests.clear());
        assert!(matches!(
            as_multi_existing(
                &fixture.client,
                fixture.members[1],
                &fixture.members,
                2,
                fixture.inner.clone(),
                &fixture.estimate,
                expected.clone(),
            )
            .await,
            Err(ClientError::SigningDisabled { .. })
        ));
        assert_eq!(
            fixture
                .fake
                .requests()
                .iter()
                .filter(|request| request.method == "state_queryStorageAt")
                .count(),
            1
        );
        for (signer, members, threshold) in [
            (fixture.members[0], fixture.members.to_vec(), 1),
            (AccountId([9; 32]), fixture.members.to_vec(), 2),
            (
                fixture.members[0],
                vec![fixture.members[0], fixture.members[0]],
                2,
            ),
            (fixture.members[0], fixture.members.to_vec(), 4),
        ] {
            fixture.fake.mutate_state(|state| state.requests.clear());
            assert!(
                as_multi_existing(
                    &fixture.client,
                    signer,
                    &members,
                    threshold,
                    fixture.inner.clone(),
                    &fixture.estimate,
                    expected.clone()
                )
                .await
                .is_err()
            );
            assert!(
                !fixture
                    .fake
                    .requests()
                    .iter()
                    .any(|request| request.method == "state_queryStorageAt")
            );
        }
        // A runtime mismatch refuses before pending-state access as well.
        let newer = RuntimeHandle::parse(
            include_bytes!("../../../vendor/metadata/finney/478.v15.scale"),
            480,
            1,
            42,
        )?;
        let inner = rtao_runtime::calls::system::remark(vec![4, 5, 6]).compose(&newer)?;
        fixture.fake.mutate_state(|state| state.requests.clear());
        assert!(matches!(
            as_multi_existing(
                &fixture.client,
                fixture.members[0],
                &fixture.members,
                2,
                inner,
                &fixture.estimate,
                expected
            )
            .await,
            Err(ClientError::Compose(ComposeError::VersionMismatch { .. }))
        ));
        assert!(
            !fixture
                .fake
                .requests()
                .iter()
                .any(|request| request.method == "state_queryStorageAt")
        );
        fixture.client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn existing_multisig_enforces_a_changed_numeric_member_cap_before_pending_access()
    -> Result<(), Box<dyn std::error::Error>> {
        use serde_json::json;
        for cap in [2, 3] {
            let mut state = rtao_testkit::FakeState::try_default()?;
            state.seed_constant("Multisig", "MaxSignatories", json!(cap))?;
            let fixture = ContinuationFixture::from_state(state).await?;
            let expected = MultisigTimepoint {
                height: 12,
                index: 3,
            };
            fixture.seed(&expected)?;
            fixture.fake.mutate_state(|state| state.requests.clear());
            let result = as_multi_existing(
                &fixture.client,
                fixture.members[0],
                &fixture.members,
                2,
                fixture.inner.clone(),
                &fixture.estimate,
                expected,
            )
            .await;
            if cap == 2 {
                let error = result.err().ok_or("changed runtime cap was ignored")?;
                assert!(matches!(error, ClientError::Compose(_)));
                assert!(error.to_string().contains("MaxSignatories (2)"));
                assert!(
                    !fixture
                        .fake
                        .requests()
                        .iter()
                        .any(|request| request.method == "state_queryStorageAt")
                );
            } else {
                let decoded = fixture.decode(&result?).await?;
                assert_eq!(
                    decoded_arg(&decoded, "maybe_timepoint")?,
                    &json!({"height":12,"index":3})
                );
                assert_eq!(
                    fixture
                        .fake
                        .requests()
                        .iter()
                        .filter(|request| request.method == "state_queryStorageAt")
                        .count(),
                    1
                );
            }
            fixture.client.close().await;
        }
        Ok(())
    }

    #[tokio::test]
    async fn existing_multisig_uses_one_pinned_pending_read_even_when_best_head_moves()
    -> Result<(), Box<dyn std::error::Error>> {
        use rtao_testkit::Reply;
        use serde_json::json;
        let fixture = ContinuationFixture::new().await?;
        let expected = MultisigTimepoint {
            height: 12,
            index: 3,
        };
        fixture.seed(&expected)?;
        let pinned_hash = fixture
            .fake
            .mutate_state(|state| state.block_hash(state.block))
            .ok_or("head hash missing")?;
        let old_hash = pinned_hash.clone();
        let key = fixture.key.clone();
        fixture.fake.mutate_state(|state| {
            state.requests.clear();
            state.set_handler("chain_getHeader", move |state, request| {
                if request.params.as_array().is_some_and(Vec::is_empty) {
                    // Snapshot the valid old pending entry, then remove it at
                    // the next best head before the pending read is issued.
                    let old_header = state.header(&old_hash);
                    state.advance(1);
                    if let Some(new_hash) = state.block_hash(state.block) {
                        state.seed_at(new_hash, key.clone(), None);
                    }
                    Reply::result(old_header.unwrap_or(serde_json::Value::Null))
                } else {
                    state.dispatch(request)
                }
            });
        });
        let call = as_multi_existing(
            &fixture.client,
            fixture.members[0],
            &fixture.members,
            2,
            fixture.inner.clone(),
            &fixture.estimate,
            expected,
        )
        .await?;
        let requests = fixture.fake.requests();
        assert_eq!(
            requests
                .iter()
                .filter(|request| request.method == "chain_getHeader"
                    && request.params.as_array().is_some_and(Vec::is_empty))
                .count(),
            1
        );
        let storage = requests
            .iter()
            .filter(|request| request.method == "state_queryStorageAt")
            .collect::<Vec<_>>();
        assert_eq!(storage.len(), 1);
        assert_eq!(storage[0].params[1], json!(pinned_hash));
        assert_eq!(
            storage[0].params[0],
            json!([format!("0x{}", hex::encode(&fixture.key))])
        );
        assert_eq!(
            decoded_arg(&fixture.decode(&call).await?, "maybe_timepoint")?,
            &json!({"height":12,"index":3})
        );
        fixture.client.close().await;
        Ok(())
    }

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
                as_multi(
                    &snapshot,
                    signer,
                    &[signer, other],
                    2,
                    inner.clone(),
                    &estimate
                )
                .await
                .is_err(),
                "{mutation}"
            );
            assert!(
                as_multi_existing(
                    &snapshot,
                    signer,
                    &[signer, other],
                    2,
                    inner,
                    &estimate,
                    MultisigTimepoint {
                        height: 12,
                        index: 3
                    }
                )
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
