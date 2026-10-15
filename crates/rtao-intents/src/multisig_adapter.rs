// Derived from RaoFoundation/subtensor sdk/python/bittensor/intents/multisig.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Typed adapters that dispatch a semantic intent through a multisig account.
//!
//! The executor builds the semantic intent, then adds sudo and proxy layers,
//! then invokes this adapter. Later approvals can pin the complete inner call
//! from a previous review so randomized calls are never silently rebuilt.

use rtao_client::{Client, ClientError, View, ViewExt};
use rtao_primitives::{AccountId, CryptoType, NetUid, Spend};
use rtao_runtime::{CallBytes, CallDescriptor, RuntimeHandle, models::MultisigTimepoint};
use rtao_signer::{Coldkey, IntentKeyView, KeySource, KeyView, SignerRole};
use rtao_tx::{
    BuildCx, Built, DynIntent, Executor, Intent, IntentError, IntentInterface, IntentPreflight,
    IntentSafety, Multisig, MultisigCall, OriginView, Plan, PlanOptions, PreflightCx, Signed,
    TxError,
    multisig::{FundsRequest, check_multisig_funds, multisig_opening_shortfall},
};
use serde_json::{Value, json};
use smallvec::SmallVec;
use std::sync::{Mutex, MutexGuard};

#[derive(Default)]
struct ReviewState {
    timepoint: Option<MultisigTimepoint>,
    inner: Option<CallBytes>,
    outer: Option<CallBytes>,
}

/// Opaque evidence linking exact reviewed call bytes to their semantic intent.
///
/// Only an adapter's [`MultisigAdapter::review`] or
/// [`MultisigThreshold1Adapter::review`] can create this value. It binds the
/// complete semantic arguments, canonical group and effective origin to bytes
/// built through the executor. It cannot be constructed from arbitrary calls.
#[derive(Clone, Debug)]
pub struct MultisigContinuation {
    call: CallBytes,
    semantic: Value,
    group: Multisig,
    origin: AccountId,
}

impl MultisigContinuation {
    /// Borrow the exact runtime-stamped semantic, sudo and proxy call bytes.
    pub fn call(&self) -> &CallBytes {
        &self.call
    }

    fn validate<I: Intent>(&self, inner: &I, group: &Multisig) -> Result<(), IntentError> {
        if self.group != *group || self.semantic != inner.to_json() {
            return Err(IntentError::InvalidArgument(
                "retained multisig call differs from its reviewed semantic intent or signer set"
                    .into(),
            ));
        }
        Ok(())
    }
}

struct Adapter<I> {
    inner: I,
    client: Client,
    group: Multisig,
    member: AccountId,
    crypto: CryptoType,
    others: Vec<AccountId>,
    continuation: Option<MultisigContinuation>,
    expected_opening: Option<MultisigTimepoint>,
    state: Mutex<ReviewState>,
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

impl<I: Intent> Adapter<I> {
    async fn new(
        inner: I,
        client: &Client,
        group: Multisig,
        member: AccountId,
        crypto: CryptoType,
        continuation: Option<MultisigContinuation>,
        threshold_one: bool,
    ) -> Result<Self, IntentError> {
        let runtime = client.refresh_runtime().await?;
        let group = validated_group(&runtime, &group, member)?;
        if (group.threshold == 1) != threshold_one {
            return Err(IntentError::InvalidArgument(if threshold_one {
                "MultisigThreshold1Adapter requires threshold 1".into()
            } else {
                "MultisigAdapter requires threshold at least 2".into()
            }));
        }
        if let Some(proof) = &continuation {
            proof.validate(&inner, &group)?;
            proof.call.ensure_runtime(&runtime)?;
        }
        inner.validate()?;
        let others = group
            .signatories
            .iter()
            .copied()
            .filter(|account| *account != member)
            .collect();
        Ok(Self {
            inner,
            client: client.clone(),
            group,
            member,
            crypto,
            others,
            continuation,
            expected_opening: None,
            state: Mutex::new(ReviewState::default()),
        })
    }

    fn state(&self) -> Result<MutexGuard<'_, ReviewState>, IntentError> {
        self.state
            .lock()
            .map_err(|_| IntentError::validation("multisig review state lock was poisoned"))
    }

    fn dispatch_summary(&self) -> String {
        if self.group.threshold == 1 {
            format!("multisig (1-of-N) dispatch {}", I::SPEC.name)
        } else {
            format!(
                "multisig {}-of-{} execute {}",
                self.group.threshold,
                self.group.signatories.len(),
                I::SPEC.name
            )
        }
    }

    fn to_json(&self, op: &str) -> Value {
        // Preserve the last complete public record in this infallible hook;
        // fallible hooks refuse to use poisoned review state.
        let timepoint = self
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .timepoint
            .clone();
        let mut result = json!({
            "op": op,
            "other_signatories": self.others,
            "call": self.inner.to_json(),
        });
        if self.group.threshold >= 2 {
            result["threshold"] = json!(self.group.threshold);
            result["timepoint"] = json!(timepoint);
        }
        result
    }

    fn summary(&self) -> String {
        let outer = self.dispatch_summary();
        let suffix = format!(" {}", I::SPEC.name);
        let approval = outer.strip_suffix(&suffix).unwrap_or(&outer);
        format!("{approval}: {}", self.inner.summary())
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        if let Some(proof) = &self.continuation {
            return self.wrap_call(cx, proof.call.clone()).await;
        }
        let semantic_cx = BuildCx {
            view: cx.view,
            origin: OriginView::Account(self.group.address),
            runtime: cx.runtime,
            clock: cx.clock,
            spec: I::SPEC,
        };
        let mut built = self.inner.build(&semantic_cx).await?;
        let wrapped = self.wrap_call(&semantic_cx, built.call).await?;
        built.call = wrapped.call;
        built.extras.extend(wrapped.extras);
        Ok(built)
    }

    fn semantic_cx<'a, V: View + ?Sized>(cx: &PreflightCx<'a, V>) -> PreflightCx<'a, V> {
        PreflightCx {
            view: cx.view,
            runtime: cx.runtime,
            clock: cx.clock,
            spec: I::SPEC,
            client: cx.client,
            dispatch_origin: cx.dispatch_origin,
            fee_payer: cx.fee_payer,
            call: cx.call,
        }
    }

    async fn opening_warning<V: View + ?Sized>(
        &self,
        cx: &PreflightCx<'_, V>,
    ) -> Result<Option<String>, IntentError> {
        let opening = self.state()?.timepoint.is_none();
        if self.group.threshold >= 2 && opening {
            Ok(multisig_opening_shortfall(cx.view, self.member, self.group.threshold).await)
        } else {
            Ok(None)
        }
    }

    async fn preflight<V: View + ?Sized>(
        &self,
        cx: &PreflightCx<'_, V>,
    ) -> Result<IntentPreflight, IntentError> {
        if cx.fee_payer != self.member {
            return Err(IntentError::InvalidArgument(
                "approval fee payer differs from the selected multisig member".into(),
            ));
        }
        let mut preview = self.inner.preflight(&Self::semantic_cx(cx)).await?;
        preview.effects.push(self.dispatch_summary());
        if let Some(warning) = self.opening_warning(cx).await? {
            preview.warnings.insert(0, warning);
        }
        Ok(preview)
    }

    async fn wrap_call<V: View + ?Sized>(
        &self,
        cx: &BuildCx<'_, V>,
        call: CallBytes,
    ) -> Result<Built, IntentError> {
        validated_group(cx.runtime, &self.group, self.member)?;
        let inner = if let Some(proof) = &self.continuation {
            proof.validate(&self.inner, &self.group)?;
            let role = self.inner.signer_role()?;
            let keys = IntentKeyView::new(&cx.origin, role, I::SPEC.name);
            let origin = match role {
                SignerRole::Coldkey => keys.coldkey_address()?,
                SignerRole::Hotkey => keys.hotkey_address(None)?,
            };
            if origin != proof.origin {
                return Err(IntentError::InvalidArgument(
                    "retained multisig call differs from its reviewed dispatch origin".into(),
                ));
            }
            proof.call.clone()
        } else {
            call
        };
        inner.ensure_runtime(cx.runtime)?;
        let (outer, timepoint) = if self.group.threshold == 1 {
            (
                rtao_client::compose::as_multi_threshold_1(
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
                let outer = rtao_client::compose::as_multi_existing(
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
                let outer = rtao_client::compose::as_multi(
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
                &self.client,
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
        state.outer = Some(built.call.clone());
        Ok(built)
    }
}

/// Dispatch an owned semantic intent through an M-of-N group with M ≥ 2.
///
/// Semantic policy, shielding requirements and previews come from `I`; the
/// approval extrinsic uses the selected member's coldkey. The executor places
/// semantic sudo/proxy layers inside the multisig wrapper. Construction and
/// review use public identities only. [`Self::review`] supplies an opaque
/// continuation proof to pass to a later approval's constructor.
#[doc(alias = "bittensor.intents.multisig.MultisigIntentAdapter")]
pub struct MultisigAdapter<I>(Adapter<I>);

/// Immediately dispatch an owned semantic intent through a 1-of-N group.
///
/// This has the same semantic safety and coldkey approval contract as
/// [`MultisigAdapter`], with no opening deposit or pending operation.
#[doc(alias = "bittensor.intents.multisig.MultisigThreshold1IntentAdapter")]
pub struct MultisigThreshold1Adapter<I>(Adapter<I>);

macro_rules! impl_adapter {
    ($adapter:ident, $dispatch:ty, $threshold_one:expr) => {
        impl<I: Intent> $adapter<I> {
            /// Validate the group, member and optional previously reviewed call
            /// against the client's current signing runtime, without signing.
            ///
            /// `continuation` retains semantic sudo/proxy layers from an earlier
            /// [`Self::review`]. Later reviews reuse those bytes exactly and
            /// refuse a different semantic JSON value, group, origin or runtime.
            pub async fn new(
                inner: I,
                client: &Client,
                group: Multisig,
                member: AccountId,
                crypto: CryptoType,
                continuation: Option<MultisigContinuation>,
            ) -> Result<Self, IntentError> {
                Adapter::new(
                    inner,
                    client,
                    group,
                    member,
                    crypto,
                    continuation,
                    $threshold_one,
                )
                .await
                .map(Self)
            }

            /// Borrow the owned semantic intent.
            pub fn inner(&self) -> &I {
                &self.0.inner
            }

            /// Borrow the runtime-validated, canonically ordered signer set.
            pub fn group(&self) -> &Multisig {
                &self.0.group
            }

            /// Copy the exact semantic, sudo and proxy bytes from the latest review.
            pub fn reviewed_inner_call(&self) -> Result<Option<CallBytes>, IntentError> {
                Ok(self.0.state()?.inner.clone())
            }

            /// Copy the opening used by the latest review, if one already existed.
            pub fn reviewed_timepoint(&self) -> Result<Option<MultisigTimepoint>, IntentError> {
                Ok(self.0.state()?.timepoint.clone())
            }

            /// Build a full executor plan and retain its exact inner-call provenance.
            ///
            /// This operation uses public keys only. The returned plan retains
            /// any policy violations; the continuation proves which semantic
            /// intent built the bytes and does not waive future policy checks.
            /// Calling the public `wrap_call` hook directly cannot mint a proof.
            pub async fn review<K: KeySource<Coldkey>>(
                &self,
                executor: &Executor,
                keys: &K,
                options: &PlanOptions,
            ) -> Result<(Plan, MultisigContinuation), TxError> {
                let semantic = self.0.inner.to_json();
                let plan = executor.plan(self, keys, options).await?;
                if self.0.inner.to_json() != semantic || plan.args.get("call") != Some(&semantic) {
                    return Err(IntentError::validation(
                        "multisig semantic intent changed during review",
                    )
                    .into());
                }
                let state = self.0.state()?;
                let outer = state.outer.as_ref().ok_or_else(|| {
                    IntentError::validation("multisig review did not retain its outer call")
                })?;
                if outer.as_bytes() != plan.call().as_bytes()
                    || outer.stamp() != plan.call().stamp()
                {
                    return Err(IntentError::validation(
                        "another multisig review replaced the retained call",
                    )
                    .into());
                }
                let call = state.inner.clone().ok_or_else(|| {
                    IntentError::validation("multisig review did not retain its inner call")
                })?;
                let proof = MultisigContinuation {
                    call,
                    semantic,
                    group: self.0.group.clone(),
                    origin: options.proxy_for.unwrap_or(self.0.group.address),
                };
                Ok((plan, proof))
            }
        }

        impl<I: Intent> IntentInterface for $adapter<I> {
            const SPEC: &'static rtao_catalog::IntentSpec = <$dispatch>::SPEC;
            type Role = Coldkey;
            type Origin = Signed;
            type Output = MultisigCall;
            const WRAPS: &'static [&'static CallDescriptor] = <$dispatch>::WRAPS;

            fn to_json(&self) -> Value {
                self.0.to_json(Self::SPEC.name)
            }
        }

        impl<I: Intent> Intent for $adapter<I> {
            fn signer_role(&self) -> Result<SignerRole, IntentError> {
                Ok(SignerRole::Coldkey)
            }
            fn summary(&self) -> String {
                self.0.summary()
            }
            async fn build<V: View + ?Sized>(
                &self,
                cx: &BuildCx<'_, V>,
            ) -> Result<Built, IntentError> {
                self.0.build(cx).await
            }
            fn validate(&self) -> Result<(), IntentError> {
                self.0.inner.validate()
            }
            fn spend(&self) -> Spend {
                Intent::spend(&self.0.inner)
            }
            fn touches_netuids(&self) -> SmallVec<[NetUid; 2]> {
                Intent::touches_netuids(&self.0.inner)
            }
            fn affects_all_subnets(&self) -> bool {
                Intent::affects_all_subnets(&self.0.inner)
            }
            fn safety(&self) -> Result<IntentSafety, IntentError> {
                self.0.inner.safety()
            }
            async fn safety_at<V: View + ?Sized>(
                &self,
                cx: &BuildCx<'_, V>,
            ) -> Result<IntentSafety, IntentError> {
                self.0
                    .inner
                    .safety_at(&BuildCx {
                        view: cx.view,
                        origin: cx.origin,
                        runtime: cx.runtime,
                        clock: cx.clock,
                        spec: I::SPEC,
                    })
                    .await
            }
            fn semantic_intent(&self) -> &dyn DynIntent {
                &self.0.inner
            }
            fn inner_call_data(&self) -> Option<&CallBytes> {
                self.0.continuation.as_ref().map(|proof| &proof.call)
            }
            fn origin_view<'a>(&'a self, _keys: &'a dyn KeyView) -> OriginView<'a> {
                OriginView::Account(self.0.group.address)
            }
            async fn wrap_call<V: View + ?Sized>(
                &self,
                cx: &BuildCx<'_, V>,
                call: CallBytes,
            ) -> Result<Built, IntentError> {
                self.0.wrap_call(cx, call).await
            }
            async fn preflight<V: View + ?Sized>(
                &self,
                cx: &PreflightCx<'_, V>,
            ) -> Result<IntentPreflight, IntentError> {
                self.0.preflight(cx).await
            }
            async fn effects<V: View + ?Sized>(
                &self,
                cx: &PreflightCx<'_, V>,
            ) -> Result<Vec<String>, IntentError> {
                let mut effects = self.0.inner.effects(&Adapter::<I>::semantic_cx(cx)).await?;
                effects.push(self.0.dispatch_summary());
                Ok(effects)
            }
            async fn warnings<V: View + ?Sized>(
                &self,
                cx: &PreflightCx<'_, V>,
            ) -> Result<Vec<String>, IntentError> {
                let mut warnings = self
                    .0
                    .inner
                    .warnings(&Adapter::<I>::semantic_cx(cx))
                    .await?;
                if let Some(warning) = self.0.opening_warning(cx).await? {
                    warnings.insert(0, warning);
                }
                Ok(warnings)
            }
            async fn blocks<V: View + ?Sized>(
                &self,
                cx: &PreflightCx<'_, V>,
            ) -> Result<Vec<String>, IntentError> {
                self.0.inner.blocks(&Adapter::<I>::semantic_cx(cx)).await
            }
        }
    };
}

impl_adapter!(MultisigAdapter, crate::MultisigExecute, false);
impl_adapter!(MultisigThreshold1Adapter, crate::MultisigThreshold1, true);

impl<I: Intent> MultisigAdapter<I> {
    /// Continue this exact pending opening, refusing a missing/replaced opening.
    ///
    /// This is normally paired with a pinned call copied from a prior review.
    /// D-041 prevents silently opening another operation after cancellation.
    pub fn with_opening_timepoint(mut self, expected: MultisigTimepoint) -> Self {
        self.0.expected_opening = Some(expected);
        self
    }
}
