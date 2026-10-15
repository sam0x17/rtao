// Derived from RaoFoundation/subtensor sdk/python/bittensor/intents/multisig.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/multisig_helpers.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Internal semantic-intent dispatch through a saved multisig wallet.

use rtao::{
    client::{Client, ClientError, DynView, ViewExt},
    primitives::{AccountId, CryptoType, NetUid, Spend},
    signer::{BoxFuture, KeyView, SignerRole},
    tx::{
        BuildCx, Built, DynIntent, IntentError, IntentPreflight, Multisig, OriginView,
        PolicySubject, PreflightCx, TxError,
        multisig::{FundsRequest, check_multisig_funds, multisig_opening_shortfall},
    },
};
use rtao_catalog::IntentSpec;
use rtao_runtime::{CallBytes, CallDescriptor, RuntimeHandle, models::MultisigTimepoint};
use serde_json::{Value, json};
use smallvec::SmallVec;
use std::sync::{Mutex, MutexGuard};

#[derive(Default)]
struct DispatchState {
    timepoint: Option<MultisigTimepoint>,
    inner: Option<CallBytes>,
}

/// An unregistered CLI adapter that preserves a semantic intent's safety contract.
///
/// Approvals sign with the member's coldkey. The semantic intent builds against
/// the multisig account, including when it ordinarily requires a hotkey. The
/// executor adds semantic sudo and proxy layers before this adapter wraps them.
pub struct SemanticMultisig<'a> {
    semantic: &'a dyn DynIntent,
    client: &'a Client,
    group: Multisig,
    member: AccountId,
    crypto: CryptoType,
    others: Vec<AccountId>,
    dispatch_spec: &'static IntentSpec,
    pinned_call: Option<CallBytes>,
    expected_opening: Option<MultisigTimepoint>,
    state: Mutex<DispatchState>,
}

impl<'a> SemanticMultisig<'a> {
    /// Validate a public member and group against the actual signing runtime.
    ///
    /// No fee, funds or private-key operation precedes runtime validation.
    /// Later rounds may supply the first round's exact runtime-stamped call;
    /// the executor then skips semantic rebuilding and its sudo/proxy layers.
    pub async fn new(
        semantic: &'a dyn DynIntent,
        client: &'a Client,
        group: Multisig,
        member: AccountId,
        crypto: CryptoType,
        pinned_call: Option<CallBytes>,
    ) -> Result<Self, IntentError> {
        let runtime = client.refresh_runtime().await?;
        let canonical = validated_group(&runtime, &group, member)?;
        if let Some(call) = &pinned_call {
            call.ensure_runtime(&runtime)?;
        }
        semantic.validate_dyn()?;
        let op = if canonical.threshold == 1 {
            "multisig_threshold_1"
        } else {
            "multisig_execute"
        };
        let dispatch_spec = rtao_catalog::INTENTS
            .iter()
            .find(|spec| spec.name == op)
            .ok_or_else(|| IntentError::InvalidArgument(format!("missing catalog intent {op}")))?;
        let others = canonical
            .signatories
            .iter()
            .copied()
            .filter(|account| *account != member)
            .collect();
        Ok(Self {
            semantic,
            client,
            group: canonical,
            member,
            crypto,
            others,
            dispatch_spec,
            pinned_call,
            expected_opening: None,
            state: Mutex::new(DispatchState::default()),
        })
    }

    /// Borrow the runtime-validated, canonically ordered signer set.
    pub fn group(&self) -> &Multisig {
        &self.group
    }

    /// Copy the exact semantic, sudo and proxy bytes used by the latest review.
    ///
    /// The caller carries this stamped call into subsequent approval rounds.
    pub fn reviewed_inner_call(&self) -> Result<Option<CallBytes>, IntentError> {
        Ok(self.state()?.inner.clone())
    }

    /// Require a later round to continue this exact pending operation.
    ///
    /// D-041: absent or replaced openings refuse rather than creating another
    /// operation with the same call bytes. Initial approvals do not set this.
    pub fn with_opening_timepoint(mut self, expected: MultisigTimepoint) -> Self {
        self.expected_opening = Some(expected);
        self
    }

    /// Copy the pending opening used by the latest successful review.
    /// A newly opening approval has no timepoint until its receipt is included.
    pub fn reviewed_timepoint(&self) -> Result<Option<MultisigTimepoint>, IntentError> {
        Ok(self.state()?.timepoint.clone())
    }

    fn state(&self) -> Result<MutexGuard<'_, DispatchState>, IntentError> {
        self.state
            .lock()
            .map_err(|_| IntentError::Validation("multisig review state lock was poisoned".into()))
    }

    fn dispatch_summary(&self) -> String {
        if self.group.threshold == 1 {
            format!("multisig (1-of-N) dispatch {}", self.semantic.spec().name)
        } else {
            format!(
                "multisig {}-of-{} execute {}",
                self.group.threshold,
                self.group.signatories.len(),
                self.semantic.spec().name
            )
        }
    }
}

fn validated_group(
    runtime: &RuntimeHandle,
    group: &Multisig,
    member: AccountId,
) -> Result<Multisig, IntentError> {
    let canonical =
        Multisig::new(runtime, &group.signatories, group.threshold).map_err(intent_error)?;
    if canonical.address != group.address {
        return Err(IntentError::InvalidArgument(
            "multisig address does not match its signer set".into(),
        ));
    }
    if !canonical.signatories.contains(&member) {
        return Err(IntentError::InvalidArgument(
            "signer is not a multisig signatory".into(),
        ));
    }
    Ok(canonical)
}

fn intent_error(error: TxError) -> IntentError {
    match error {
        TxError::Intent(error) => error,
        TxError::Chain(error) => IntentError::Chain(error),
        TxError::Signer(error) => IntentError::Signer(error),
        TxError::UnitMismatch(error) => IntentError::UnitMismatch(error),
        error => IntentError::Client(ClientError::Protocol(error.to_string())),
    }
}

impl PolicySubject for SemanticMultisig<'_> {
    fn op(&self) -> &'static str {
        self.semantic.op()
    }
    fn spend(&self) -> Spend {
        self.semantic.spend()
    }
    fn touches_netuids(&self) -> SmallVec<[NetUid; 2]> {
        self.semantic.touches_netuids()
    }
    fn affects_all_subnets(&self) -> bool {
        self.semantic.affects_all_subnets()
    }
    fn is_raw(&self) -> bool {
        self.semantic.is_raw()
    }
}

impl DynIntent for SemanticMultisig<'_> {
    fn spec(&self) -> &'static IntentSpec {
        self.dispatch_spec
    }
    fn signer_role_dyn(&self) -> Result<SignerRole, IntentError> {
        // The semantic origin can be a hotkey, but approval extrinsics always
        // acquire the selected member's coldkey.
        Ok(SignerRole::Coldkey)
    }
    fn wraps(&self) -> &'static [&'static CallDescriptor] {
        if self.group.threshold == 1 {
            &[&rtao_runtime::calls::multisig::AS_MULTI_THRESHOLD_1]
        } else {
            &[&rtao_runtime::calls::multisig::AS_MULTI]
        }
    }
    fn to_json_dyn(&self) -> Value {
        // This infallible trait hook must retain the last complete public
        // record. A poisoned lock is recovered here; fallible hooks refuse it.
        let timepoint = self
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .timepoint
            .clone();
        let mut result = json!({
            "op": self.dispatch_spec.name,
            "other_signatories": self.others,
            "call": self.semantic.to_json_dyn(),
        });
        if self.group.threshold >= 2 {
            result["threshold"] = json!(self.group.threshold);
            result["timepoint"] = json!(timepoint);
        }
        result
    }
    fn summary_dyn(&self) -> String {
        let outer = self.dispatch_summary();
        let suffix = format!(" {}", self.semantic.spec().name);
        let approval = outer.strip_suffix(&suffix).unwrap_or(&outer);
        format!("{approval}: {}", self.semantic.summary_dyn())
    }
    fn validate_dyn(&self) -> Result<(), IntentError> {
        self.semantic.validate_dyn()
    }
    fn spend_dyn(&self) -> Spend {
        self.semantic.spend_dyn()
    }
    fn touches_netuids_dyn(&self) -> SmallVec<[NetUid; 2]> {
        self.semantic.touches_netuids_dyn()
    }
    fn affects_all_subnets_dyn(&self) -> bool {
        self.semantic.affects_all_subnets_dyn()
    }
    fn semantic_intent_dyn(&self) -> &dyn DynIntent {
        self.semantic
    }
    fn inner_call_data_dyn(&self) -> Option<&CallBytes> {
        self.pinned_call.as_ref()
    }
    fn origin_view_dyn<'b>(&'b self, _keys: &'b dyn KeyView) -> OriginView<'b> {
        OriginView::Account(self.group.address)
    }
    fn build_dyn<'b>(
        &'b self,
        cx: &'b BuildCx<'b, dyn DynView + 'b>,
    ) -> BoxFuture<'b, Result<Built, IntentError>> {
        Box::pin(async move {
            if let Some(call) = &self.pinned_call {
                return self.wrap_call_dyn(cx, call.clone()).await;
            }
            let semantic_cx = BuildCx {
                view: cx.view,
                origin: OriginView::Account(self.group.address),
                runtime: cx.runtime,
                clock: cx.clock,
                spec: self.semantic.spec(),
            };
            let mut built = self.semantic.build_dyn(&semantic_cx).await?;
            let wrapped = self.wrap_call_dyn(&semantic_cx, built.call).await?;
            built.call = wrapped.call;
            built.extras.extend(wrapped.extras);
            Ok(built)
        })
    }
    fn preflight_dyn<'b>(
        &'b self,
        cx: &'b PreflightCx<'b, dyn DynView + 'b>,
    ) -> BoxFuture<'b, Result<IntentPreflight, IntentError>> {
        Box::pin(async move {
            if cx.fee_payer != self.member {
                return Err(IntentError::InvalidArgument(
                    "approval fee payer differs from the selected multisig member".into(),
                ));
            }
            let semantic_cx = PreflightCx {
                view: cx.view,
                runtime: cx.runtime,
                clock: cx.clock,
                spec: self.semantic.spec(),
                client: cx.client,
                dispatch_origin: cx.dispatch_origin,
                fee_payer: cx.fee_payer,
                call: cx.call,
            };
            let mut preview = self.semantic.preflight_dyn(&semantic_cx).await?;
            preview.effects.push(self.dispatch_summary());
            let opening = self.state()?.timepoint.is_none();
            if self.group.threshold >= 2
                && opening
                && let Some(warning) =
                    multisig_opening_shortfall(cx.view, self.member, self.group.threshold).await
            {
                preview.warnings.insert(0, warning);
            }
            Ok(preview)
        })
    }
    fn effects_dyn<'b>(
        &'b self,
        cx: &'b PreflightCx<'b, dyn DynView + 'b>,
    ) -> BoxFuture<'b, Result<Vec<String>, IntentError>> {
        Box::pin(async move {
            let semantic_cx = PreflightCx {
                view: cx.view,
                runtime: cx.runtime,
                clock: cx.clock,
                spec: self.semantic.spec(),
                client: cx.client,
                dispatch_origin: cx.dispatch_origin,
                fee_payer: cx.fee_payer,
                call: cx.call,
            };
            let mut effects = self.semantic.effects_dyn(&semantic_cx).await?;
            effects.push(self.dispatch_summary());
            Ok(effects)
        })
    }
    fn warnings_dyn<'b>(
        &'b self,
        cx: &'b PreflightCx<'b, dyn DynView + 'b>,
    ) -> BoxFuture<'b, Result<Vec<String>, IntentError>> {
        Box::pin(async move {
            let semantic_cx = PreflightCx {
                view: cx.view,
                runtime: cx.runtime,
                clock: cx.clock,
                spec: self.semantic.spec(),
                client: cx.client,
                dispatch_origin: cx.dispatch_origin,
                fee_payer: cx.fee_payer,
                call: cx.call,
            };
            let mut warnings = self.semantic.warnings_dyn(&semantic_cx).await?;
            let opening = self.state()?.timepoint.is_none();
            if self.group.threshold >= 2
                && opening
                && let Some(warning) =
                    multisig_opening_shortfall(cx.view, self.member, self.group.threshold).await
            {
                warnings.insert(0, warning);
            }
            Ok(warnings)
        })
    }
    fn blocks_dyn<'b>(
        &'b self,
        cx: &'b PreflightCx<'b, dyn DynView + 'b>,
    ) -> BoxFuture<'b, Result<Vec<String>, IntentError>> {
        Box::pin(async move {
            let semantic_cx = PreflightCx {
                view: cx.view,
                runtime: cx.runtime,
                clock: cx.clock,
                spec: self.semantic.spec(),
                client: cx.client,
                dispatch_origin: cx.dispatch_origin,
                fee_payer: cx.fee_payer,
                call: cx.call,
            };
            self.semantic.blocks_dyn(&semantic_cx).await
        })
    }
    fn wrap_call_dyn<'b>(
        &'b self,
        cx: &'b BuildCx<'b, dyn DynView + 'b>,
        call: CallBytes,
    ) -> BoxFuture<'b, Result<Built, IntentError>> {
        Box::pin(async move {
            validated_group(cx.runtime, &self.group, self.member)?;
            let inner = self.pinned_call.as_ref().unwrap_or(&call).clone();
            inner.ensure_runtime(cx.runtime)?;
            let (outer, timepoint) = if self.group.threshold == 1 {
                (
                    rtao::client::compose::as_multi_threshold_1(
                        cx.runtime,
                        &self.others,
                        inner.clone(),
                    )?,
                    None,
                )
            } else {
                let (outer, timepoint) = if let Some(expected) = &self.expected_opening {
                    let estimate = self
                        .client
                        .estimate_weight(inner.clone(), self.member, self.crypto)
                        .await?;
                    let outer = rtao::client::compose::as_multi_existing(
                        cx.view,
                        self.member,
                        &self.group.signatories,
                        self.group.threshold,
                        inner.clone(),
                        &estimate,
                        expected.clone(),
                    )
                    .await?;
                    (outer, Some(expected.clone()))
                } else {
                    let pending = cx
                        .view
                        .storage(
                            &rtao_runtime::storage::multisig::MULTISIGS,
                            (self.group.address, inner.hash()),
                        )
                        .await?;
                    if pending
                        .as_ref()
                        .is_some_and(|pending| pending.approvals.contains(&self.member))
                    {
                        return Err(IntentError::InvalidArgument(format!(
                            "this signatory already approved pending call 0x{}; wait for another member, or cancel the operation",
                            hex::encode(inner.hash())
                        )));
                    }
                    let timepoint = pending.map(|pending| pending.when);
                    let estimate = self
                        .client
                        .estimate_weight(inner.clone(), self.member, self.crypto)
                        .await?;
                    let outer = rtao::client::compose::as_multi(
                        cx.view,
                        self.member,
                        &self.group.signatories,
                        self.group.threshold,
                        inner.clone(),
                        &estimate,
                    )
                    .await?;
                    (outer, timepoint)
                };
                check_multisig_funds(
                    self.client,
                    cx.view,
                    FundsRequest {
                        signer: self.member,
                        threshold: self.group.threshold,
                        opening: timepoint.is_none(),
                        outer_call: Some(&outer),
                        crypto_type: self.crypto,
                        signer_label: None,
                    },
                )
                .await
                .map_err(intent_error)?;
                (outer, timepoint)
            };
            let mut built = Built::new(outer);
            if self.group.threshold >= 2 {
                built.extras.insert(
                    "multisig_call_hash".into(),
                    json!(format!("0x{}", hex::encode(inner.hash()))),
                );
                built.extras.insert(
                    "multisig_call_data".into(),
                    json!(format!("0x{}", hex::encode(inner.as_bytes()))),
                );
            }
            let mut state = self.state()?;
            state.timepoint = timepoint;
            state.inner = Some(inner);
            Ok(built)
        })
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod isolated_hook_tests {
    use super::*;
    use rtao::{
        client::{ClientConfig, Network, View},
        signer::Hotkey,
        tx::{Intent, IntentInterface, Signed, SystemClock},
    };
    use rtao_testkit::{FakeChain, FakeState, rpc::RpcClient};

    type HookInput = (&'static str, &'static str, AccountId, AccountId, Vec<u8>);

    #[derive(Default)]
    struct HookProbe(Mutex<Vec<HookInput>>);

    impl HookProbe {
        fn record<V: View + ?Sized>(
            &self,
            hook: &'static str,
            cx: &PreflightCx<'_, V>,
        ) -> Result<(), IntentError> {
            self.0
                .lock()
                .map_err(|_| IntentError::validation("hook recorder poisoned"))?
                .push((
                    hook,
                    cx.spec.name,
                    cx.dispatch_origin,
                    cx.fee_payer,
                    cx.call.as_bytes().to_vec(),
                ));
            Ok(())
        }
    }

    impl IntentInterface for HookProbe {
        const SPEC: &'static IntentSpec = <rtao::intents::ServeAxon as IntentInterface>::SPEC;
        type Role = Hotkey;
        type Origin = Signed;
        type Output = ();
        const WRAPS: &'static [&'static CallDescriptor] = &[&rtao_runtime::calls::system::REMARK];
        fn to_json(&self) -> Value {
            json!({"op": Self::SPEC.name})
        }
    }

    impl Intent for HookProbe {
        fn summary(&self) -> String {
            "isolated semantic hooks".into()
        }
        async fn build<V: View + ?Sized>(
            &self,
            _cx: &BuildCx<'_, V>,
        ) -> Result<Built, IntentError> {
            Err(IntentError::validation("isolated hook must not build"))
        }
        async fn effects<V: View + ?Sized>(
            &self,
            cx: &PreflightCx<'_, V>,
        ) -> Result<Vec<String>, IntentError> {
            self.record("effects", cx)?;
            Ok(vec!["semantic effect".into()])
        }
        async fn warnings<V: View + ?Sized>(
            &self,
            cx: &PreflightCx<'_, V>,
        ) -> Result<Vec<String>, IntentError> {
            self.record("warnings", cx)?;
            Ok(vec!["semantic warning".into()])
        }
        async fn blocks<V: View + ?Sized>(
            &self,
            cx: &PreflightCx<'_, V>,
        ) -> Result<Vec<String>, IntentError> {
            self.record("blocks", cx)?;
            Ok(vec!["semantic block".into()])
        }
        async fn preflight<V: View + ?Sized>(
            &self,
            _cx: &PreflightCx<'_, V>,
        ) -> Result<IntentPreflight, IntentError> {
            Err(IntentError::validation(
                "isolated hook must not invoke full preflight",
            ))
        }
    }

    #[tokio::test]
    async fn isolated_hooks_preserve_semantic_context_without_calling_full_preflight()
    -> Result<(), Box<dyn std::error::Error>> {
        let member = AccountId([7; 32]);
        let mut state = FakeState::try_default()?;
        state.seed_python(
            "System",
            "Account",
            &[json!(member.to_string())],
            json!({"data": {"free": 0}}),
        )?;
        let chain = FakeChain::from_state(state);
        let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
        config.track_heads = false;
        config.runtime_cache_dir = None;
        config.config_store = None;
        let client = Client::from_rpc(RpcClient::new(chain.clone()), config).await?;
        let executor = rtao::Executor::new(client.clone());
        let semantic = HookProbe::default();
        let group = executor.multisig(&[member, AccountId([9; 32])], 2).await?;
        let adapter = SemanticMultisig::new(
            &semantic,
            &client,
            group.clone(),
            member,
            CryptoType::Sr25519,
            None,
        )
        .await?;
        assert_eq!(semantic.signer_role()?, SignerRole::Hotkey);
        assert_eq!(adapter.signer_role_dyn()?, SignerRole::Coldkey);
        let snapshot = View::pin(&client).await?;
        let runtime = client.refresh_runtime().await?;
        let call = rtao_runtime::calls::system::remark(vec![1, 2, 3]).compose(&runtime)?;
        let cx = PreflightCx {
            view: &snapshot as &dyn DynView,
            runtime: &runtime,
            clock: &SystemClock,
            spec: adapter.spec(),
            client: &client,
            dispatch_origin: group.address,
            fee_payer: member,
            call: &call,
        };
        assert_eq!(
            adapter.effects_dyn(&cx).await?,
            ["semantic effect", "multisig 2-of-2 execute serve_axon"]
        );
        let warnings = adapter.warnings_dyn(&cx).await?;
        assert_eq!(warnings.len(), 2);
        assert!(warnings[0].contains("opening approval reserves"));
        assert_eq!(warnings[1], "semantic warning");
        assert_eq!(adapter.blocks_dyn(&cx).await?, ["semantic block"]);
        let recorded = semantic
            .0
            .lock()
            .map_err(|_| std::io::Error::other("hook recorder poisoned"))?
            .clone();
        assert_eq!(
            recorded,
            ["effects", "warnings", "blocks"].map(|hook| (
                hook,
                HookProbe::SPEC.name,
                group.address,
                member,
                call.as_bytes().to_vec()
            ))
        );
        assert!(
            !chain
                .requests()
                .iter()
                .any(|request| request.method.starts_with("author_")
                    || request.params[0] == "TransactionPaymentApi_query_info")
        );
        // A later approval keeps the same semantic hook and omits only the
        // adapter's opening-deposit warning.
        adapter.state()?.timepoint = Some(MultisigTimepoint {
            height: 12,
            index: 3,
        });
        assert_eq!(adapter.warnings_dyn(&cx).await?, ["semantic warning"]);
        client.close().await;
        Ok(())
    }
}
