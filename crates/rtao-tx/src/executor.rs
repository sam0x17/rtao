// Derived from RaoFoundation/subtensor sdk/python/tests/unit/test_fee_filters.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/executor.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/intents/registry.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/_substrate.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Plan, preview and execution using one client's shared nonce manager.

use crate::{
    BuildCx, ChainError, Clock, DynIntent, ExtrinsicResult, Intent, IntentError, IntentPreflight,
    OriginView, Plan, Policy, PolicyError, PreflightCx, SystemClock, TxError,
};
use futures::StreamExt;
use rtao_client::{
    CallBytes, Client, Mortality, Period, PoolRejection, ShieldOptions as ClientShieldOptions,
    ShieldPeriod, SubmitFailure, SubmitOptions, View, Wait,
};
use rtao_core::value_ext::ValueExt;
use rtao_primitives::{AccountId, Balance, IntoCoreValue, Tao};
use rtao_runtime::models::ProxyType;
use rtao_signer::{DynKeySource, IntentKeyView, KeySource, KeyView, Role, SignerRole};
use serde_json::{Map, Value, json};
use std::{sync::Arc, time::Duration};

/// Progress notification carrying the unchanged Python progress data.
#[derive(Clone, Debug)]
pub struct Progress {
    /// Current registration/execution stage.
    pub stage: String,
    /// Stage-specific JSON fields.
    pub data: Map<String, Value>,
}

/// Python's agent-facing operation manifest.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct ToolSpec {
    /// Catalog operation name.
    pub name: String,
    /// First description line.
    pub summary: String,
    /// Complete operation description.
    pub description: String,
    /// Public signing role token.
    pub signer: String,
    /// Required dispatch origin token.
    pub origin: String,
    /// Read used to verify privileged state changes.
    pub verify: Option<String>,
    /// Composed pallet/call pairs.
    pub wraps: Vec<(String, String)>,
    /// Absolute Python documentation URL.
    pub docs_url: String,
    /// Exact authenticated Python JSON schema.
    pub input_schema: Value,
}

/// Shared planning and preflight options.
#[derive(Clone, Debug, Default)]
pub struct PlanOptions {
    /// Per-operation policy replaces the executor default when present.
    pub policy: Option<Policy>,
    /// Inner proxy dispatch account.
    pub proxy_for: Option<AccountId>,
    /// Optional exact proxy permission category.
    pub force_proxy_type: Option<ProxyType>,
    /// Bound the complete preview operation.
    pub deadline: Option<Duration>,
}

/// Python execution defaults plus bounded waiting and fate reconciliation.
#[derive(Clone, Debug)]
pub struct ExecuteOptions {
    /// Require the rebuilt outer call to match these reviewed bytes and both
    /// runtime stamps before resolving a private signer. No wrapper is added.
    pub expected_call: Option<CallBytes>,
    /// Mortal era period; None means immortal.
    pub period: Option<u32>,
    /// Resolve an inclusion block when finalization is not requested.
    pub wait_for_inclusion: bool,
    /// Await and resolve finalized inclusion.
    pub wait_for_finalization: bool,
    /// Additional transient pool attempts; separate from the controlled owner replan.
    pub retries: u32,
    /// Replan a direct owner operation once for TxRateLimitExceeded.
    ///
    /// Multisig approvals do not inherit eligibility from their inner intent.
    /// Owner orchestration disables this when it owns the same bounded retry.
    pub replan_owner_rate_limit: bool,
    /// Inner proxy dispatch account.
    pub proxy_for: Option<AccountId>,
    /// Optional exact proxy category.
    pub force_proxy_type: Option<ProxyType>,
    /// Per-operation policy replaces the executor default.
    pub policy: Option<Policy>,
    /// Follow queued registration through completion.
    pub wait_for_registration: bool,
    /// Optional registration-completion deadline.
    pub registration_timeout: Option<Duration>,
    /// Bounded channel for progress notifications.
    pub progress: Option<tokio::sync::mpsc::Sender<Progress>>,
    /// Reconcile transmitted unknown fate through finalized history.
    pub reconcile: bool,
    /// Bound planning, signing, submission, retry and waiting together.
    pub deadline: Option<Duration>,
    /// Bound preparation/signing and author RPC admission separately from inclusion.
    pub submit_deadline: Option<Duration>,
    /// Raw-call shield opt-in, implemented by the shield execution path.
    pub shielded: bool,
}
impl Default for ExecuteOptions {
    fn default() -> Self {
        Self {
            expected_call: None,
            period: Some(128),
            wait_for_inclusion: true,
            wait_for_finalization: true,
            retries: 0,
            replan_owner_rate_limit: true,
            proxy_for: None,
            force_proxy_type: None,
            policy: None,
            wait_for_registration: true,
            registration_timeout: None,
            progress: None,
            reconcile: false,
            deadline: None,
            submit_deadline: None,
            shielded: false,
        }
    }
}
impl ExecuteOptions {
    pub(crate) fn check_expected_call(&self, call: &CallBytes) -> Result<(), TxError> {
        if let Some(expected) = &self.expected_call
            && (expected.stamp() != call.stamp()
                || expected.hash() != call.hash()
                || expected.as_bytes() != call.as_bytes())
        {
            return Err(IntentError::InvalidArgument(
                "rebuilt call differs from the reviewed call; review the new call before signing"
                    .into(),
            )
            .into());
        }
        Ok(())
    }
    fn plan_options(&self) -> PlanOptions {
        PlanOptions {
            policy: self.policy.clone(),
            proxy_for: self.proxy_for,
            force_proxy_type: self.force_proxy_type,
            deadline: self.deadline,
        }
    }
    pub(crate) fn submit_options(&self) -> SubmitOptions {
        SubmitOptions {
            mortality: self.period.map_or(Mortality::Immortal, |period| {
                Mortality::Mortal(Period::new(u64::from(period)))
            }),
            wait: if self.wait_for_finalization {
                Wait::Finalization
            } else if self.wait_for_inclusion {
                Wait::Inclusion
            } else {
                Wait::Pool
            },
            deadline: self.deadline,
            submit_deadline: self.submit_deadline,
            reconcile: self.reconcile,
            ..Default::default()
        }
    }
    fn shield_period(&self) -> Result<ShieldPeriod, TxError> {
        match self.period {
            None | Some(128) | Some(8) => Ok(ShieldPeriod::P8),
            Some(4) => Ok(ShieldPeriod::P4),
            Some(period) => Err(IntentError::InvalidArgument(format!(
                "MEV shield period must be 4 or 8 blocks, got {period}"
            ))
            .into()),
        }
    }
}

/// Intent shield options, retaining the executor's policy and origin controls.
#[derive(Clone, Debug)]
pub struct ShieldOptions {
    /// The short era used by both the inner transaction and its carrier.
    pub period: ShieldPeriod,
    /// Planning, signer, origin and waiting options; period and retries are unused.
    pub execute: ExecuteOptions,
    /// Additional inner-call fee cap; the carrier pays its own TAO fee.
    pub max_fee: Option<Balance<Tao>>,
}
impl Default for ShieldOptions {
    fn default() -> Self {
        Self {
            period: ShieldPeriod::P8,
            execute: ExecuteOptions {
                wait_for_finalization: false,
                ..Default::default()
            },
            max_fee: None,
        }
    }
}

struct ReviewedIntent {
    built: crate::Built,
    safety: crate::IntentSafety,
    preview: IntentPreflight,
    warnings: Vec<String>,
    role: SignerRole,
}

/// Object-safe parser injection avoiding a dependency cycle to generated intents.
pub trait IntentRegistry: Send + Sync {
    /// Parse one feature-enabled catalog operation into its behavior object.
    #[allow(clippy::wrong_self_convention)] // This names Python's registry operation.
    fn from_tool(&self, op: &str, args: Value) -> Result<Box<dyn DynIntent>, IntentError>;
}

/// Execution services sharing exactly one Client and nonce manager.
#[derive(Clone)]
pub struct Executor {
    client: Client,
    policy: Option<Policy>,
    clock: Arc<dyn Clock>,
    registry: Option<Arc<dyn IntentRegistry>>,
}
impl Executor {
    /// Start without policy limits or private-key ownership.
    pub fn new(client: Client) -> Self {
        Self {
            client,
            policy: None,
            clock: Arc::new(SystemClock),
            registry: None,
        }
    }
    /// Set the executor's default policy.
    pub fn with_policy(mut self, policy: Policy) -> Self {
        self.policy = Some(policy);
        self
    }
    /// Inject a deterministic clock for time-dependent calls.
    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }
    /// Attach the generated, feature-scoped tool parser.
    pub fn with_registry(mut self, registry: Arc<dyn IntentRegistry>) -> Self {
        self.registry = Some(registry);
        self
    }
    /// Borrow the shared chain connection.
    pub fn client(&self) -> &Client {
        &self.client
    }
    /// Borrow the active default policy.
    pub fn policy(&self) -> Option<&Policy> {
        self.policy.as_ref()
    }
    /// Borrow the clock used by semantic builders.
    pub fn clock(&self) -> &dyn Clock {
        self.clock.as_ref()
    }
    /// Complete authenticated tool catalog, retaining every Python schema field.
    pub fn tools(&self) -> Result<&'static [ToolSpec], IntentError> {
        static TOOLS: std::sync::OnceLock<Result<Vec<ToolSpec>, String>> =
            std::sync::OnceLock::new();
        match TOOLS.get_or_init(|| {
            rtao_catalog::INTENTS
                .iter()
                .map(|spec| {
                    let mut tool: ToolSpec = serde_json::from_str(spec.catalog_json)
                        .map_err(|error| error.to_string())?;
                    tool.docs_url = format!(
                        "{}/tx/{}",
                        rtao_catalog::py::DOCS_URL,
                        spec.name.replace('_', "-")
                    );
                    Ok(tool)
                })
                .collect()
        }) {
            Ok(tools) => Ok(tools),
            Err(error) => Err(IntentError::InvalidArgument(format!(
                "authenticated tool catalog is invalid: {error}"
            ))),
        }
    }

    /// Typed dry run; role access never unlocks the key source.
    pub async fn plan<I: Intent, K: KeySource<I::Role>>(
        &self,
        intent: &I,
        keys: &K,
        opts: &PlanOptions,
    ) -> Result<Plan, TxError> {
        check_typed_role(intent)?;
        let budget = TimeBudget::new(opts.deadline)?;
        budget
            .run(&self.client, async {
                self.plan_with_keys(intent, keys, keys.address()?, opts)
                    .await
                    .map(|(plan, _)| plan)
            })
            .await
    }
    /// Dynamic dry run with Python's public role-default behavior.
    pub async fn plan_dyn(
        &self,
        intent: &dyn DynIntent,
        keys: &dyn DynKeySource,
        opts: &PlanOptions,
    ) -> Result<Plan, TxError> {
        self.plan_and_preflight_dyn(intent, keys, opts)
            .await
            .map(|(plan, _)| plan)
    }
    /// Dynamic plan and structural facts from the same state snapshot and call.
    /// Neither planning nor the preflight hooks acquire a private signing key.
    pub async fn plan_and_preflight_dyn(
        &self,
        intent: &dyn DynIntent,
        keys: &dyn DynKeySource,
        opts: &PlanOptions,
    ) -> Result<(Plan, IntentPreflight), TxError> {
        let budget = TimeBudget::new(opts.deadline)?;
        budget
            .run(&self.client, async {
                self.plan_with_keys(intent, keys, public_address(intent, keys)?, opts)
                    .await
            })
            .await
    }
    /// Typed full preview, including best-effort fee estimation.
    pub async fn preflight<I: Intent, K: KeySource<I::Role>>(
        &self,
        intent: &I,
        keys: &K,
        opts: &PlanOptions,
    ) -> Result<IntentPreflight, TxError> {
        check_typed_role(intent)?;
        let budget = TimeBudget::new(opts.deadline)?;
        budget
            .run(&self.client, async {
                self.plan_with_keys(intent, keys, keys.address()?, opts)
                    .await
                    .map(|(_, preview)| preview)
            })
            .await
    }
    /// Dynamic full preview without a private-key unlock.
    pub async fn preflight_dyn(
        &self,
        intent: &dyn DynIntent,
        keys: &dyn DynKeySource,
        opts: &PlanOptions,
    ) -> Result<IntentPreflight, TxError> {
        self.plan_and_preflight_dyn(intent, keys, opts)
            .await
            .map(|(_, preview)| preview)
    }

    async fn plan_with_keys(
        &self,
        intent: &dyn DynIntent,
        keys: &dyn KeyView,
        signer_address: AccountId,
        opts: &PlanOptions,
    ) -> Result<(Plan, IntentPreflight), TxError> {
        let ReviewedIntent {
            mut built,
            safety,
            preview,
            warnings,
            role,
        } = self
            .review_with_keys(intent, keys, signer_address, opts, false)
            .await?;
        let mut plan = Plan::new(
            built.call,
            intent.spec().name,
            intent.summary_dyn(),
            role,
            signer_address,
        );
        plan.fee = preview.estimated_fee;
        plan.effects = preview.effects.clone();
        if let Some(real) = opts.proxy_for {
            plan.effects.push(format!(
                "dispatched via proxy as {real} (signed by {signer_address})"
            ));
        }
        plan.warnings = warnings;
        if let Some(policy) = opts.policy.as_ref().or(self.policy.as_ref()) {
            plan.violations = policy.check(&safety, plan.fee);
        }
        plan.violations.extend(preview.blocks.clone());
        plan.extras = std::mem::take(&mut built.extras);
        plan.spend = safety.spend;
        plan.args = intent.to_json_dyn();
        if let Some(args) = plan.args.as_object_mut() {
            args.remove("op");
        }
        Ok((plan, preview))
    }

    async fn review_with_keys(
        &self,
        intent: &dyn DynIntent,
        keys: &dyn KeyView,
        signer_address: AccountId,
        opts: &PlanOptions,
        shielded: bool,
    ) -> Result<ReviewedIntent, TxError> {
        intent.validate_dyn()?;
        let semantic = intent.semantic_intent_dyn();
        let declared_safety = semantic.safety_dyn()?;
        let snapshot = View::pin(&self.client).await?;
        // Queries remain pinned to one block; new calls use the head's signing
        // runtime rather than a historical block's execution runtime.
        let runtime = self.client.refresh_runtime().await?;
        for descriptor in semantic.wraps() {
            let report = rtao_runtime::verify_surface_with(
                &runtime,
                &[rtao_runtime::SurfaceDescriptor::Call(descriptor)],
            );
            if let Some(error) = report
                .changes
                .into_iter()
                .find(|change| change.class == rtao_runtime::SurfaceClass::Breaking)
            {
                return Err(TxError::SurfaceChanged {
                    pallet: descriptor.pallet.into(),
                    call: descriptor.name.into(),
                    detail: error.detail,
                });
            }
        }
        let origin = opts
            .proxy_for
            .map(OriginView::Account)
            .unwrap_or_else(|| intent.origin_view_dyn(keys));
        let cx = BuildCx {
            view: &snapshot as &dyn rtao_client::DynView,
            origin,
            runtime: &runtime,
            clock: self.clock.as_ref(),
            spec: semantic.spec(),
        };
        let safety = semantic.safety_at_dyn(&cx).await?;
        if !shielded && safety.mev_shield_required && !declared_safety.mev_shield_required {
            return Err(TxError::Policy(PolicyError {
                violations: vec![format!(
                    "{} requires MEV-shielded submission after pinned state review; use submit_shielded",
                    safety.op
                )],
            }));
        }
        let mut built = if let Some(call) = intent.inner_call_data_dyn() {
            call.ensure_runtime(&runtime)
                .map_err(rtao_client::ClientError::from)?;
            intent.wrap_call_dyn(&cx, call.clone()).await?
        } else {
            let mut built = semantic.build_dyn(&cx).await?;
            if semantic.spec().origin == "root" && !is_sudo(&runtime, &built.call)? {
                built.call = rtao_runtime::calls::sudo::sudo(built.call)
                    .compose(&runtime)
                    .map_err(IntentError::from)?;
            }
            if let Some(real) = opts.proxy_for {
                built.call = rtao_runtime::calls::proxy::proxy(
                    rtao_core::DynValue::new(real.into_core_value()),
                    opts.force_proxy_type,
                    built.call,
                )
                .compose(&runtime)
                .map_err(IntentError::from)?;
            }
            let wrapped = intent.wrap_call_dyn(&cx, built.call).await?;
            built.call = wrapped.call;
            built.extras.extend(wrapped.extras);
            built
        };
        if let Some(real) = opts.proxy_for {
            built.extras.entry("proxy_for").or_insert(json!(real));
        }
        built
            .call
            .ensure_runtime(&runtime)
            .map_err(rtao_client::ClientError::from)?;
        let semantic_role: SignerRole = semantic.signer_role_dyn()?;
        let dispatch_origin = opts.proxy_for.map_or_else(
            || role_address(&origin, semantic_role, semantic.spec().name),
            Ok,
        )?;
        let pfcx = PreflightCx {
            view: &snapshot as &dyn rtao_client::DynView,
            runtime: &runtime,
            clock: self.clock.as_ref(),
            spec: intent.spec(),
            client: &self.client,
            dispatch_origin,
            fee_payer: signer_address,
            call: &built.call,
        };
        let mut preview = intent.preflight_dyn(&pfcx).await?;
        let mut warnings = preview.warnings.clone();
        let role: SignerRole = intent.signer_role_dyn()?;
        if !shielded
            && role == SignerRole::Hotkey
            && opts.proxy_for.is_none()
            && coldkey_pays(&runtime, &built.call)?
        {
            warnings.push(rtao_catalog::py::COLDKEY_FEE_WARNING.into());
        }
        let active_policy = opts.policy.as_ref().or(self.policy.as_ref());
        if preview.estimated_fee.is_none()
            && (!shielded || active_policy.is_some_and(|policy| policy.max_fee.is_some()))
        {
            match pfcx.estimate_fee().await {
                Ok(fee) => preview.estimated_fee = Some(fee),
                Err(error) if shielded => {
                    return Err(TxError::Policy(PolicyError {
                        violations: vec![format!(
                            "could not estimate fee to enforce max_fee_tao: {error}"
                        )],
                    }));
                }
                Err(error) => warnings.push(format!("could not estimate fee: {error}")),
            }
        }
        if shielded && let Some(required) = preview.required_free {
            let funds = async {
                let free = match preview.available_free {
                    Some(free) => free,
                    None => crate::multisig::free_balance(&snapshot, signer_address).await?,
                };
                let carrier = self
                    .client
                    .estimate_shielded_carrier_fee(signer_address)
                    .await?;
                Ok::<_, rtao_client::ClientError>((free, carrier))
            }
            .await;
            match funds {
                Ok((free, carrier)) => {
                    let combined = u128::from(required.rao()) + u128::from(carrier.rao());
                    if required.rao() <= free.rao() && u128::from(free.rao()) < combined {
                        let combined = u64::try_from(combined)
                            .map(|rao| rtao_primitives::DynBalance::from(Balance::<Tao>::from_rao(rao)).to_string())
                            .unwrap_or_else(|_| format!("{} rao", combined));
                        preview.blocks.push(format!(
                            "free TAO ({}) covers the inner claim reserve ({}) but not that reserve plus the MEV-shield carrier fee (~{combined})",
                            rtao_primitives::DynBalance::from(free),
                            rtao_primitives::DynBalance::from(required),
                        ));
                    }
                }
                Err(error) => preview.blocks.push(format!(
                    "could not verify free TAO for the shielded call plus its carrier fee; refusing to risk a post-carrier reserve shortfall ({error})"
                )),
            }
        }
        Ok(ReviewedIntent {
            built,
            safety,
            preview,
            warnings,
            role,
        })
    }

    /// Typed execution, restricted to the interface's compile-time signing role.
    ///
    /// ```compile_fail
    /// use rtao_client::Client;
    /// use rtao_intents::Transfer;
    /// use rtao_signer::{AccountId, Coldkey, Hotkey, KeySource, KeyView, Signer, SignerError};
    /// use rtao_tx::{ExecuteOptions, Executor};
    /// use std::sync::Arc;
    /// struct HotkeyOnly;
    /// impl KeyView for HotkeyOnly {
    ///     fn hotkey_address(&self)->Result<AccountId,SignerError>{Ok(AccountId([1;32]))}
    ///     fn hotkey_public_key(&self)->Result<[u8;32],SignerError>{Ok([1;32])}
    ///     fn coldkey_address(&self)->Result<AccountId,SignerError>{Err(SignerError::KeyUnavailable(rtao_signer::SignerRole::Coldkey))}
    /// }
    /// impl KeySource<Hotkey> for HotkeyOnly {
    ///     fn address(&self)->Result<AccountId,SignerError>{self.hotkey_address()}
    ///     async fn signer(&self)->Result<Arc<dyn Signer>,SignerError>{unimplemented!()}
    /// }
    /// async fn wrong_role(executor:&Executor, transfer:&Transfer) {
    ///     executor.execute(transfer, &HotkeyOnly, &ExecuteOptions::default()).await;
    /// }
    /// ```
    pub async fn execute<I: Intent, K: KeySource<I::Role>>(
        &self,
        intent: &I,
        keys: &K,
        opts: &ExecuteOptions,
    ) -> Result<ExtrinsicResult<I::Output>, TxError> {
        check_typed_role(intent)?;
        let budget = TimeBudget::new(opts.deadline)?;
        budget.check(&self.client)?;
        let address = keys.address()?;
        let result = self
            .execute_with_keys(intent, keys, address, || keys.signer(), opts, budget)
            .await?;
        result.with_output::<I::Output>().map_err(TxError::from)
    }

    /// Dynamic execution with Python's runtime role checks.
    pub async fn execute_dyn(
        &self,
        intent: &dyn DynIntent,
        keys: &dyn DynKeySource,
        opts: &ExecuteOptions,
    ) -> Result<ExtrinsicResult, TxError> {
        self.execute_dyn_with_budget(intent, keys, opts, TimeBudget::new(opts.deadline)?)
            .await
    }

    /// Shield one intent with compile-time signing-role selection.
    pub async fn submit_shielded<I: Intent, K: KeySource<I::Role>>(
        &self,
        intent: &I,
        keys: &K,
        opts: &ShieldOptions,
    ) -> Result<ExtrinsicResult<I::Output>, TxError> {
        check_typed_role(intent)?;
        let budget = TimeBudget::new(opts.execute.deadline)?;
        budget.check(&self.client)?;
        let result = self
            .submit_shielded_with_keys(
                intent,
                keys,
                keys.address()?,
                || keys.signer(),
                opts,
                budget,
            )
            .await?;
        result.with_output::<I::Output>().map_err(TxError::from)
    }

    /// Shield one intent with the dynamic source's runtime role checks.
    pub async fn submit_shielded_dyn(
        &self,
        intent: &dyn DynIntent,
        keys: &dyn DynKeySource,
        opts: &ShieldOptions,
    ) -> Result<ExtrinsicResult, TxError> {
        let budget = TimeBudget::new(opts.execute.deadline)?;
        budget.check(&self.client)?;
        let role: SignerRole = intent.signer_role_dyn()?;
        self.submit_shielded_with_keys(
            intent,
            keys,
            public_address(intent, keys)?,
            || keys.signer_for(role),
            opts,
            budget,
        )
        .await
    }

    /// Submit runtime-stamped raw bytes using a compile-time signing role.
    ///
    /// An active policy must explicitly allow raw calls because their spend and
    /// affected subnets have no inspectable intent contract.
    pub async fn submit_call<R: Role, K: KeySource<R>>(
        &self,
        call: CallBytes,
        keys: &K,
        opts: &ExecuteOptions,
    ) -> Result<ExtrinsicResult, TxError> {
        let budget = TimeBudget::new(opts.deadline)?;
        let admission = TimeBudget::new(opts.submit_deadline)?;
        budget.check(&self.client)?;
        self.enforce_raw_call(opts)?;
        self.submit_call_with_keys(
            call,
            keys.address()?,
            || keys.signer(),
            opts,
            budget,
            admission,
        )
        .await
    }

    /// Submit raw bytes after resolving the requested runtime signing role.
    pub async fn submit_call_dyn(
        &self,
        call: CallBytes,
        keys: &dyn DynKeySource,
        role: SignerRole,
        opts: &ExecuteOptions,
    ) -> Result<ExtrinsicResult, TxError> {
        let budget = TimeBudget::new(opts.deadline)?;
        let admission = TimeBudget::new(opts.submit_deadline)?;
        budget.check(&self.client)?;
        self.enforce_raw_call(opts)?;
        let address = role_address(keys, role, "submit_call")?;
        self.submit_call_with_keys(
            call,
            address,
            || keys.signer_for(role),
            opts,
            budget,
            admission,
        )
        .await
    }

    pub(crate) fn enforce_raw_call(&self, opts: &ExecuteOptions) -> Result<(), TxError> {
        if let Some(policy) = opts.policy.as_ref().or(self.policy.as_ref()) {
            let violations = policy.check_raw_call();
            if !violations.is_empty() {
                return Err(TxError::Policy(PolicyError { violations }));
            }
        }
        Ok(())
    }

    async fn submit_call_with_keys<F, Fut>(
        &self,
        call: CallBytes,
        address: AccountId,
        signer: F,
        opts: &ExecuteOptions,
        budget: TimeBudget,
        admission: TimeBudget,
    ) -> Result<ExtrinsicResult, TxError>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<
                Output = Result<Arc<dyn rtao_signer::Signer>, rtao_signer::SignerError>,
            > + Send,
    {
        let shield_period = opts.shielded.then(|| opts.shield_period()).transpose()?;
        budget
            .run(
                &self.client,
                admission.run(&self.client, async {
                    call.ensure_runtime(&self.client.refresh_runtime().await?)
                        .map_err(rtao_client::ClientError::from)?;
                    opts.check_expected_call(&call)?;
                    Ok(())
                }),
            )
            .await?;
        let signer = budget
            .run(
                &self.client,
                admission.run(&self.client, async { Ok(signer().await?) }),
            )
            .await?;
        if signer.account() != address {
            return Err(TxError::Signer(rtao_signer::SignerError::InvalidSignature(
                "signer address changed while resolving the private key".into(),
            )));
        }
        let mut submit = opts.submit_options();
        submit.deadline = budget.remaining(&self.client)?;
        submit.submit_deadline = admission.remaining(&self.client)?;
        let result = match shield_period {
            Some(period) => {
                self.client
                    .submit_shielded(
                        call,
                        signer.as_ref(),
                        &ClientShieldOptions {
                            period,
                            wait: submit.wait,
                            max_fee: None,
                            submit,
                        },
                    )
                    .await
            }
            None => self.client.submit(call, signer.as_ref(), &submit).await,
        };
        self.lift_submission(result, Map::new())
    }

    async fn submit_shielded_with_keys<F, Fut>(
        &self,
        intent: &dyn DynIntent,
        keys: &dyn KeyView,
        address: AccountId,
        mut signer: F,
        opts: &ShieldOptions,
        budget: TimeBudget,
    ) -> Result<ExtrinsicResult, TxError>
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<
                Output = Result<Arc<dyn rtao_signer::Signer>, rtao_signer::SignerError>,
            > + Send,
    {
        let mut plan_opts = opts.execute.plan_options();
        let mut active_policy = opts
            .execute
            .policy
            .as_ref()
            .or(self.policy.as_ref())
            .cloned();
        if let Some(max_fee) = opts.max_fee {
            let policy = active_policy.get_or_insert_with(Policy::default);
            policy.max_fee = Some(policy.max_fee.map_or(max_fee, |cap| {
                Balance::from_rao(cap.rao().min(max_fee.rao()))
            }));
        }
        plan_opts.policy = active_policy.clone();
        let mut resolved_signer = None;
        let mut replanned = false;
        let mut owner_replanned = false;
        loop {
            let reviewed = budget
                .run(
                    &self.client,
                    self.review_with_keys(intent, keys, address, &plan_opts, true),
                )
                .await?;
            let mut violations = active_policy.as_ref().map_or_else(Vec::new, |policy| {
                policy.check(&reviewed.safety, reviewed.preview.estimated_fee)
            });
            violations.extend(reviewed.preview.blocks);
            if !violations.is_empty() {
                return Err(TxError::Policy(PolicyError { violations }));
            }
            opts.execute.check_expected_call(&reviewed.built.call)?;
            let admission = TimeBudget::new(opts.execute.submit_deadline)?;
            let signer = match &resolved_signer {
                Some(signer) => Arc::clone(signer),
                None => {
                    let signer = budget
                        .run(
                            &self.client,
                            admission.run(&self.client, async { Ok(signer().await?) }),
                        )
                        .await?;
                    resolved_signer = Some(Arc::clone(&signer));
                    signer
                }
            };
            if signer.account() != address {
                return Err(TxError::Signer(rtao_signer::SignerError::InvalidSignature(
                    "signer address changed while resolving the private key".into(),
                )));
            }
            let mut submit = opts.execute.submit_options();
            submit.deadline = budget.remaining(&self.client)?;
            submit.submit_deadline = admission.remaining(&self.client)?;
            let shield_opts = ClientShieldOptions {
                period: opts.period,
                wait: submit.wait,
                // The semantic preflight above owns the exact inner fee model.
                // The client-level guard is for callers without that review.
                max_fee: None,
                submit,
            };
            let result = self
                .client
                .submit_shielded(reviewed.built.call, signer.as_ref(), &shield_opts)
                .await;
            if matches!(
                &result,
                Err(SubmitFailure::Client(rtao_client::ClientError::Compose(
                    rtao_runtime::ComposeError::VersionMismatch { .. }
                )))
            ) && !replanned
            {
                replanned = true;
                budget
                    .run(&self.client, async {
                        self.client.refresh_runtime().await?;
                        Ok(())
                    })
                    .await?;
                continue;
            }
            // Python's explicit shield path keeps creation coordinates in the
            // receipt; only ordinary execute enriches pure-proxy result data.
            let result = self.lift_submission(result, reviewed.built.extras)?;
            if opts.execute.replan_owner_rate_limit
                && owner_rate_replan(intent, &result)
                && !owner_replanned
            {
                owner_replanned = true;
                continue;
            }
            return self
                .complete_intent(intent, keys, result, &opts.execute, true, budget)
                .await;
        }
    }

    pub(crate) fn lift_submission(
        &self,
        result: Result<rtao_client::ExtrinsicReport, SubmitFailure>,
        extras: Map<String, Value>,
    ) -> Result<ExtrinsicResult, TxError> {
        match result {
            Ok(report) => {
                let shield = report.shield.filter(|shield| {
                    report.success || report.hash == shield.inner_hash || report.error.is_none()
                });
                let mut result =
                    ExtrinsicResult::from_report(report, self.client.network().label(), Map::new());
                if result.success || shield.is_some() {
                    result.data.extend(extras);
                    if let Some(shield) = shield {
                        result.data.insert("shielded".into(), json!(true));
                        result.data.insert(
                            "inner_extrinsic_hash".into(),
                            json!(format!("0x{}", hex::encode(shield.inner_hash.0))),
                        );
                    }
                }
                if !result.success && result.error.is_none() {
                    result.error = Some(ChainError::new(result.message.clone(), None));
                }
                Ok(result)
            }
            Err(SubmitFailure::Pool(error)) => Ok(ExtrinsicResult::rejected(
                ChainError::from_pool(&error),
                None,
            )),
            Err(error) if error.unknown_fate_hash().is_some() => Ok(ExtrinsicResult::rejected(
                ChainError::new(error.to_string(), None),
                error.unknown_fate_hash(),
            )),
            Err(SubmitFailure::Expired { expiry, .. }) => Ok(ExtrinsicResult::rejected(
                ChainError::with_code(
                    format!("mortal transaction expired at block {expiry} without inclusion"),
                    Some("Expired".into()),
                    rtao_catalog::ErrorCode::Expired,
                ),
                None,
            )),
            Err(SubmitFailure::ShieldEraExpired {
                period,
                finality_lag,
            }) => Err(TxError::ShieldEraExpired {
                period: u32::try_from(period).unwrap_or(u32::MAX),
                finality_lag,
            }),
            Err(SubmitFailure::Client(error)) => Err(error.into()),
            Err(SubmitFailure::Submit(rtao_rpc::SubmitError::Rpc(error))) => {
                Err(rtao_client::ClientError::from(error).into())
            }
            Err(error) => Err(TxError::Chain(ChainError::new(error.to_string(), None))),
        }
    }

    async fn execute_dyn_with_budget(
        &self,
        intent: &dyn DynIntent,
        keys: &dyn DynKeySource,
        opts: &ExecuteOptions,
        budget: TimeBudget,
    ) -> Result<ExtrinsicResult, TxError> {
        budget.check(&self.client)?;
        let role: SignerRole = intent.signer_role_dyn()?;
        self.execute_with_keys(
            intent,
            keys,
            public_address(intent, keys)?,
            || keys.signer_for(role),
            opts,
            budget,
        )
        .await
    }

    /// Execute a JSON tool invocation through the attached generated registry.
    pub async fn execute_tool(
        &self,
        op: &str,
        args: Value,
        keys: &dyn DynKeySource,
        opts: &ExecuteOptions,
    ) -> Result<ExtrinsicResult, TxError> {
        let budget = TimeBudget::new(opts.deadline)?;
        let registry = self.registry.as_ref().ok_or_else(|| {
            IntentError::InvalidArgument(
                "no generated intent registry is attached to this executor".into(),
            )
        })?;
        let intent = registry.from_tool(op, args)?;
        budget.check(&self.client)?;
        self.execute_dyn_with_budget(intent.as_ref(), keys, opts, budget)
            .await
    }

    async fn execute_with_keys<F, Fut>(
        &self,
        intent: &dyn DynIntent,
        keys: &dyn KeyView,
        address: AccountId,
        mut signer: F,
        opts: &ExecuteOptions,
        budget: TimeBudget,
    ) -> Result<ExtrinsicResult, TxError>
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<
                Output = Result<Arc<dyn rtao_signer::Signer>, rtao_signer::SignerError>,
            > + Send,
    {
        if intent
            .semantic_intent_dyn()
            .safety_dyn()?
            .mev_shield_required
        {
            return self
                .submit_shielded_with_keys(
                    intent,
                    keys,
                    address,
                    signer,
                    &ShieldOptions {
                        execute: opts.clone(),
                        ..Default::default()
                    },
                    budget,
                )
                .await;
        }
        let mut retries = 0;
        let mut replanned = false;
        let mut owner_replanned = false;
        let mut cached_plan = None;
        let mut resolved_signer = None;
        loop {
            let plan = match cached_plan.take() {
                Some(plan) => plan,
                None => {
                    budget
                        .run(&self.client, async {
                            self.plan_with_keys(intent, keys, address, &opts.plan_options())
                                .await
                                .map(|(plan, _)| plan)
                        })
                        .await?
                }
            };
            budget.check(&self.client)?;
            if !plan.ok() {
                return Err(TxError::Policy(PolicyError {
                    violations: plan.violations,
                }));
            }
            opts.check_expected_call(plan.call())?;
            let admission = TimeBudget::new(opts.submit_deadline)?;
            let signer = match resolved_signer.as_ref() {
                Some(signer) => Arc::clone(signer),
                None => {
                    let signer = budget
                        .run(
                            &self.client,
                            admission.run(&self.client, async { Ok(signer().await?) }),
                        )
                        .await?;
                    budget.check(&self.client)?;
                    resolved_signer = Some(Arc::clone(&signer));
                    signer
                }
            };
            if signer.account() != plan.signer_address {
                return Err(TxError::Signer(rtao_signer::SignerError::InvalidSignature(
                    "signer address changed while resolving the private key".into(),
                )));
            }
            let mut submit_options = opts.submit_options();
            submit_options.deadline = budget.remaining(&self.client)?;
            submit_options.submit_deadline = admission.remaining(&self.client)?;
            // The Client now owns the remaining deadline and attempted hash.
            // An enclosing timeout could erase its postbroadcast FateUnknown.
            let result = self
                .client
                .submit(plan.call().clone(), signer.as_ref(), &submit_options)
                .await;
            match result {
                Ok(report) => {
                    let mut result = ExtrinsicResult::from_report(
                        report,
                        self.client.network().label(),
                        Map::new(),
                    );
                    if opts.replan_owner_rate_limit
                        && owner_rate_replan(intent, &result)
                        && !owner_replanned
                    {
                        owner_replanned = true;
                        continue;
                    }
                    if result.success {
                        if intent.semantic_intent_dyn().spec().name == "create_pure_proxy" {
                            pure_created_data(&mut result);
                        }
                        result.data.extend(plan.extras);
                    }
                    return self
                        .complete_intent(intent, keys, result, opts, false, budget)
                        .await;
                }
                Err(SubmitFailure::Client(rtao_client::ClientError::Compose(
                    rtao_runtime::ComposeError::VersionMismatch { .. },
                ))) if !replanned => {
                    replanned = true;
                    budget
                        .run(&self.client, async {
                            self.client.refresh_runtime().await?;
                            Ok(())
                        })
                        .await?;
                }
                Err(SubmitFailure::Pool(error)) if retries < opts.retries && transient(&error) => {
                    retries += 1;
                    budget.run(&self.client, self.wait_one_block()).await?;
                    cached_plan = Some(plan);
                }
                Err(SubmitFailure::Pool(error)) => {
                    return Ok(ExtrinsicResult::rejected(
                        ChainError::from_pool(&error),
                        None,
                    ));
                }
                Err(error) if error.unknown_fate_hash().is_some() => {
                    return Ok(ExtrinsicResult::rejected(
                        ChainError::new(error.to_string(), None),
                        error.unknown_fate_hash(),
                    ));
                }
                Err(SubmitFailure::Expired { hash: _, expiry }) => {
                    return Ok(ExtrinsicResult::rejected(
                        ChainError::with_code(
                            format!(
                                "mortal transaction expired at block {expiry} without inclusion"
                            ),
                            Some("Expired".into()),
                            rtao_catalog::ErrorCode::Expired,
                        ),
                        None,
                    ));
                }
                Err(SubmitFailure::Client(error)) => return Err(error.into()),
                Err(SubmitFailure::Submit(rtao_rpc::SubmitError::Rpc(error))) => {
                    return Err(rtao_client::ClientError::from(error).into());
                }
                Err(error) => return Err(TxError::Chain(ChainError::new(error.to_string(), None))),
            }
        }
    }

    async fn complete_intent(
        &self,
        intent: &dyn DynIntent,
        keys: &dyn KeyView,
        result: ExtrinsicResult,
        opts: &ExecuteOptions,
        deferred_dispatch: bool,
        budget: TimeBudget,
    ) -> Result<ExtrinsicResult, TxError> {
        let semantic = intent.semantic_intent_dyn();
        if semantic.spec().name != "register_subnet"
            || !result.success
            || !opts.wait_for_registration
            || !(opts.wait_for_inclusion || opts.wait_for_finalization)
            || result.fate_unknown.is_some()
        {
            return Ok(result);
        }
        let origin = opts
            .proxy_for
            .map(OriginView::Account)
            .unwrap_or_else(|| intent.origin_view_dyn(keys));
        let view = IntentKeyView::new(&origin, semantic.signer_role_dyn()?, semantic.spec().name);
        let args = semantic.to_json_dyn();
        let explicit = args
            .get("hotkey_ss58")
            .filter(|value| !value.is_null())
            .map(|value| {
                serde_json::from_value::<AccountId>(value.clone()).map_err(IntentError::from)
            })
            .transpose()?;
        crate::registration::complete(
            &self.client,
            result,
            view.coldkey_address()?,
            view.hotkey_address(explicit)?,
            opts,
            deferred_dispatch,
            budget.expires,
        )
        .await
    }

    async fn wait_one_block(&self) -> Result<(), TxError> {
        let mut blocks = self
            .client
            .blocks_strict(rtao_client::Finality::Best)
            .await?;
        let previous = self
            .client
            .at(rtao_client::BlockRef::Best)
            .await?
            .block()
            .number;
        while let Some(block) = blocks.next().await {
            if block
                .map_err(|error| match error {
                    rtao_client::HeadError::Rpc(error) => {
                        TxError::from(rtao_client::ClientError::from(error))
                    }
                    error => TxError::Chain(ChainError::new(error.to_string(), None)),
                })?
                .number
                > previous
            {
                return Ok(());
            }
        }
        Err(TxError::Rpc(rtao_rpc::RpcError::SubscriptionLost))
    }
}

fn pure_created_data(result: &mut ExtrinsicResult) {
    if let Some(event) = result
        .events
        .iter()
        .find(|event| event.matches("Proxy", "PureCreated"))
    {
        for (field, key) in [("pure", "pure_proxy"), ("who", "spawner")] {
            if let Some(value) = event
                .attributes
                .field(field)
                .and_then(rtao_core::value_ext::ValueExt::as_str)
            {
                result.data.insert(key.into(), json!(value));
            }
        }
    }
    if let Some((height, index)) = result
        .extrinsic_id
        .as_deref()
        .and_then(|id| id.split_once('-'))
        && let (Ok(height), Ok(index)) = (height.parse::<u64>(), index.parse::<u32>())
    {
        result.data.insert("height".into(), json!(height));
        result.data.insert("ext_index".into(), json!(index));
    }
}

fn transient(error: &PoolRejection) -> bool {
    if matches!(error, PoolRejection::Stale | PoolRejection::PriorityTooLow) {
        return true;
    }
    let message = error.to_string().to_lowercase();
    rtao_catalog::py::TRANSIENT_SUBSTRINGS
        .iter()
        .any(|needle| message.contains(needle))
}

#[derive(Clone, Copy)]
struct TimeBudget {
    expires: Option<tokio::time::Instant>,
}
impl TimeBudget {
    fn new(deadline: Option<Duration>) -> Result<Self, TxError> {
        Ok(Self {
            expires: deadline
                .map(|duration| {
                    tokio::time::Instant::now()
                        .checked_add(duration)
                        .ok_or_else(|| {
                            IntentError::InvalidArgument("execution deadline overflows time".into())
                        })
                })
                .transpose()?,
        })
    }
    fn timeout() -> TxError {
        TxError::Rpc(rtao_rpc::RpcError::Timeout {
            detail: "execution timed out without reaching the requested state".into(),
        })
    }
    fn check(self, client: &Client) -> Result<(), TxError> {
        if client.is_closed() {
            return Err(TxError::ConnectionNotReady);
        }
        if self
            .expires
            .is_some_and(|expires| tokio::time::Instant::now() >= expires)
        {
            return Err(Self::timeout());
        }
        Ok(())
    }
    fn remaining(self, client: &Client) -> Result<Option<Duration>, TxError> {
        self.check(client)?;
        Ok(self
            .expires
            .map(|expires| expires.saturating_duration_since(tokio::time::Instant::now())))
    }
    async fn run<T>(
        self,
        client: &Client,
        future: impl std::future::Future<Output = Result<T, TxError>>,
    ) -> Result<T, TxError> {
        self.check(client)?;
        let operation = async {
            tokio::select! {biased; ()=client.closed()=>Err(TxError::ConnectionNotReady),result=future=>result}
        };
        let result = match self.expires {
            Some(expires) => tokio::time::timeout_at(expires, operation)
                .await
                .map_err(|_| Self::timeout())?,
            None => operation.await,
        };
        self.check(client)?;
        result
    }
}

fn public_address(intent: &dyn DynIntent, keys: &dyn KeyView) -> Result<AccountId, TxError> {
    role_address(keys, intent.signer_role_dyn()?, intent.spec().name)
}

fn check_typed_role<I: Intent>(intent: &I) -> Result<(), TxError> {
    let actual = intent.signer_role()?;
    if actual != I::Role::KIND {
        return Err(IntentError::validation(format!(
            "{} selects the {} role at runtime; use the dynamic executor API with this intent",
            I::SPEC.name,
            actual.as_str()
        ))
        .into());
    }
    Ok(())
}

fn owner_rate_replan(intent: &dyn DynIntent, result: &ExtrinsicResult) -> bool {
    // A failed inner owner call does not make its execution adapter safe to
    // replay: a completed multisig approval has already consumed its pending
    // operation even when MultisigExecuted reports an inner dispatch error.
    intent.spec().origin == "subnet_owner"
        && intent.semantic_intent_dyn().spec().origin == "subnet_owner"
        && !result.success
        && result.fate_unknown.is_none()
        && result
            .error
            .as_ref()
            .and_then(|error| error.name.as_deref())
            == Some("TxRateLimitExceeded")
}
fn role_address(keys: &dyn KeyView, role: SignerRole, op: &str) -> Result<AccountId, TxError> {
    let view = IntentKeyView::new(keys, role, op);
    Ok(match role {
        SignerRole::Coldkey => view.coldkey_address()?,
        SignerRole::Hotkey => view.hotkey_address(None)?,
    })
}

fn is_sudo(runtime: &rtao_runtime::RuntimeHandle, call: &CallBytes) -> Result<bool, TxError> {
    let decoded = runtime
        .core()
        .decode_spec(&rtao_core::runtime::TypeSpec::Call, call.as_bytes(), true)
        .map_err(rtao_core::CoreError::from)
        .map_err(IntentError::from)?;
    Ok(decoded
        .field("call_module")
        .and_then(rtao_core::value_ext::ValueExt::as_str)
        == Some("Sudo")
        && decoded
            .field("call_function")
            .and_then(rtao_core::value_ext::ValueExt::as_str)
            == Some("sudo"))
}
fn coldkey_pays(runtime: &rtao_runtime::RuntimeHandle, call: &CallBytes) -> Result<bool, TxError> {
    let decoded = runtime
        .core()
        .decode_spec(&rtao_core::runtime::TypeSpec::Call, call.as_bytes(), true)
        .map_err(rtao_core::CoreError::from)
        .map_err(IntentError::from)?;
    fn walk(value: &rtao_core::codec::Value) -> bool {
        use rtao_core::value_ext::ValueExt;
        fn list(value: &rtao_core::codec::Value) -> Option<&[rtao_core::codec::Value]> {
            match value {
                rtao_core::codec::Value::List(values) | rtao_core::codec::Value::Tuple(values) => {
                    Some(values)
                }
                _ => None,
            }
        }
        let (Some(pallet), Some(call)) = (
            value.field("call_module").and_then(ValueExt::as_str),
            value.field("call_function").and_then(ValueExt::as_str),
        ) else {
            return false;
        };
        let arg = |name: &str| {
            value
                .field("call_args")
                .and_then(list)
                .and_then(|args| {
                    args.iter()
                        .find(|arg| arg.field("name").and_then(ValueExt::as_str) == Some(name))
                })
                .and_then(|arg| arg.field("value"))
        };
        match (pallet, call) {
            ("Proxy", "proxy") | ("Multisig", "as_multi" | "as_multi_threshold_1") => {
                arg("call").is_some_and(walk)
            }
            ("Utility", "batch_all" | "batch" | "force_batch") => arg("calls")
                .and_then(list)
                .is_some_and(|calls| calls.iter().any(walk)),
            pair => rtao_catalog::rt::COLDKEY_PAYS_FEE_CALLS.contains(&pair),
        }
    }
    Ok(walk(&decoded))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::*;
    use crate::{Built, IntentInterface, Signed};
    use rtao_client::{ClientConfig, Inclusion, Network};
    use rtao_primitives::{CryptoType, Spend};
    use rtao_signer::{BoxFuture, Coldkey, KeypairSigner, Signer, SignerError};
    use rtao_testkit::{FakeChain, FakeState, rpc::RpcClient};
    use std::sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

    #[derive(Default)]
    struct RoleAccess {
        typed_addresses: AtomicUsize,
        coldkey_public: AtomicUsize,
        hotkey_public: AtomicUsize,
        hotkey_public_key: AtomicUsize,
        typed_acquisitions: AtomicUsize,
        dynamic_acquisitions: Mutex<Vec<SignerRole>>,
        events: Mutex<Vec<&'static str>>,
    }

    struct RoleKeys {
        coldkey: KeypairSigner,
        hotkey: KeypairSigner,
        access: Arc<RoleAccess>,
    }

    impl KeyView for RoleKeys {
        fn hotkey_address(&self) -> Result<AccountId, SignerError> {
            self.access.hotkey_public.fetch_add(1, Ordering::SeqCst);
            self.access
                .events
                .lock()
                .expect("events")
                .push("public hotkey");
            Ok(self.hotkey.account())
        }
        fn hotkey_public_key(&self) -> Result<[u8; 32], SignerError> {
            self.access.hotkey_public_key.fetch_add(1, Ordering::SeqCst);
            Ok(self.hotkey.account().0)
        }
        fn coldkey_address(&self) -> Result<AccountId, SignerError> {
            self.access.coldkey_public.fetch_add(1, Ordering::SeqCst);
            self.access
                .events
                .lock()
                .expect("events")
                .push("public coldkey");
            Ok(self.coldkey.account())
        }
    }

    impl KeySource<Coldkey> for RoleKeys {
        fn address(&self) -> Result<AccountId, SignerError> {
            self.access.typed_addresses.fetch_add(1, Ordering::SeqCst);
            Ok(self.coldkey.account())
        }
        async fn signer(&self) -> Result<Arc<dyn Signer>, SignerError> {
            self.access
                .typed_acquisitions
                .fetch_add(1, Ordering::SeqCst);
            self.access
                .events
                .lock()
                .expect("events")
                .push("acquire coldkey");
            Ok(Arc::new(self.coldkey.clone()))
        }
    }

    impl DynKeySource for RoleKeys {
        fn signer_for<'a>(
            &'a self,
            role: SignerRole,
        ) -> BoxFuture<'a, Result<Arc<dyn Signer>, SignerError>> {
            Box::pin(async move {
                self.access
                    .dynamic_acquisitions
                    .lock()
                    .expect("roles")
                    .push(role);
                self.access.events.lock().expect("events").push(match role {
                    SignerRole::Coldkey => "acquire coldkey",
                    SignerRole::Hotkey => "acquire hotkey",
                });
                let signer: Arc<dyn Signer> = match role {
                    SignerRole::Coldkey => Arc::new(self.coldkey.clone()),
                    SignerRole::Hotkey => Arc::new(self.hotkey.clone()),
                };
                Ok(signer)
            })
        }
    }

    struct InstanceHotkey {
        builds: AtomicUsize,
        previews: AtomicUsize,
        origins: Mutex<Vec<(AccountId, AccountId)>>,
        access: Arc<RoleAccess>,
        required_at: bool,
    }

    impl IntentInterface for InstanceHotkey {
        // Batch is cataloged as coldkey but adopts its children's instance role.
        const SPEC: &'static rtao_catalog::IntentSpec = &rtao_catalog::INTENTS[7];
        type Role = Coldkey;
        type Origin = Signed;
        type Output = ();
        const WRAPS: &'static [&'static rtao_runtime::CallDescriptor] =
            &[&rtao_runtime::calls::system::REMARK];
        fn to_json(&self) -> Value {
            json!({"op":"batch","instance_role":"hotkey"})
        }
    }

    impl Intent for InstanceHotkey {
        async fn safety_at<V: View + ?Sized>(
            &self,
            _cx: &BuildCx<'_, V>,
        ) -> Result<crate::IntentSafety, IntentError> {
            let mut safety = self.safety()?;
            safety.mev_shield_required |= self.required_at;
            Ok(safety)
        }
        fn signer_role(&self) -> Result<SignerRole, IntentError> {
            Ok(SignerRole::Hotkey)
        }
        fn summary(&self) -> String {
            "instance-selected hotkey fixture".into()
        }
        fn spend(&self) -> Spend {
            Spend::Bounded(Balance::<Tao>::from_rao(1))
        }
        async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
            self.builds.fetch_add(1, Ordering::SeqCst);
            self.access.events.lock().expect("events").push("build");
            let mut built =
                Built::new(rtao_runtime::calls::system::remark(vec![73]).compose(cx.runtime)?);
            built.extras.insert("instance_hotkey".into(), json!(true));
            Ok(built)
        }
        async fn preflight<V: View + ?Sized>(
            &self,
            cx: &PreflightCx<'_, V>,
        ) -> Result<IntentPreflight, IntentError> {
            self.previews.fetch_add(1, Ordering::SeqCst);
            self.origins
                .lock()
                .expect("origins")
                .push((cx.dispatch_origin, cx.fee_payer));
            self.access.events.lock().expect("events").push("preflight");
            Ok(IntentPreflight {
                effects: vec![format!(
                    "dispatch={} payer={}",
                    cx.dispatch_origin, cx.fee_payer
                )],
                ..Default::default()
            })
        }
    }

    async fn role_fixture() -> TestResult<(Executor, FakeChain, RoleKeys, InstanceHotkey)> {
        let mut state = FakeState::try_default()?;
        state.seed_typed("MevShield", "NextKey", (), vec![0u8; 1184])?;
        state.seed_constant("Aura", "SlotDuration", json!(1))?;
        let chain = FakeChain::from_state(state);
        let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
        config.track_heads = false;
        config.runtime_cache_dir = None;
        config.config_store = None;
        let client = Client::from_rpc(RpcClient::new(chain.clone()), config).await?;
        let access = Arc::new(RoleAccess::default());
        let keys = RoleKeys {
            coldkey: KeypairSigner::from_seed(&[31; 32], CryptoType::Ed25519)?,
            hotkey: KeypairSigner::from_seed(&[37; 32], CryptoType::Ed25519)?,
            access: access.clone(),
        };
        assert_ne!(keys.coldkey.account(), keys.hotkey.account());
        let intent = InstanceHotkey {
            builds: AtomicUsize::new(0),
            previews: AtomicUsize::new(0),
            origins: Mutex::new(vec![]),
            access,
            required_at: false,
        };
        Ok((Executor::new(client), chain, keys, intent))
    }

    #[tokio::test]
    async fn pinned_state_only_shield_requirement_refuses_ordinary_execution_before_build_or_unlock()
    -> TestResult {
        let (executor, chain, keys, mut intent) = role_fixture().await?;
        intent.required_at = true;
        assert!(!intent.safety()?.mev_shield_required);
        let error = executor
            .execute_dyn(
                &intent,
                &keys,
                &ExecuteOptions {
                    wait_for_inclusion: false,
                    wait_for_finalization: false,
                    ..Default::default()
                },
            )
            .await
            .expect_err("state-only required shield must not submit unshielded");
        assert!(
            error
                .to_string()
                .contains("requires MEV-shielded submission after pinned state review")
        );
        assert_eq!(intent.builds.load(Ordering::SeqCst), 0);
        assert_no_private_role_access(&keys);
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        executor.client().close().await;
        Ok(())
    }

    fn assert_no_private_role_access(keys: &RoleKeys) {
        assert_eq!(keys.access.typed_acquisitions.load(Ordering::SeqCst), 0);
        assert!(
            keys.access
                .dynamic_acquisitions
                .lock()
                .expect("roles")
                .is_empty()
        );
    }

    fn assert_no_coldkey_access(keys: &RoleKeys) {
        assert_eq!(keys.access.typed_addresses.load(Ordering::SeqCst), 0);
        assert_eq!(keys.access.coldkey_public.load(Ordering::SeqCst), 0);
        assert_eq!(keys.access.typed_acquisitions.load(Ordering::SeqCst), 0);
    }

    fn assert_acquisition_after_review(keys: &RoleKeys) {
        let events = keys.access.events.lock().expect("events");
        let acquisition = events
            .iter()
            .position(|event| *event == "acquire hotkey")
            .expect("private acquisition");
        let preview = events
            .iter()
            .position(|event| *event == "preflight")
            .expect("preflight");
        assert!(preview < acquisition);
        assert!(!events.contains(&"acquire coldkey"));
    }

    #[tokio::test]
    async fn typed_entrypoints_refuse_instance_role_mismatch_before_any_key_or_rpc_access()
    -> TestResult {
        let (executor, chain, keys, intent) = role_fixture().await?;
        assert_eq!(InstanceHotkey::SPEC.signer, "coldkey");
        executor.client().nonces().pin(&keys.coldkey.account(), 40);
        executor.client().nonces().pin(&keys.hotkey.account(), 20);
        let requests = chain.requests().len();
        let errors = [
            executor
                .plan(&intent, &keys, &PlanOptions::default())
                .await
                .expect_err("typed plan role mismatch"),
            executor
                .preflight(&intent, &keys, &PlanOptions::default())
                .await
                .expect_err("typed preview role mismatch"),
            executor
                .execute(&intent, &keys, &ExecuteOptions::default())
                .await
                .expect_err("typed execution role mismatch"),
            executor
                .submit_shielded(&intent, &keys, &ShieldOptions::default())
                .await
                .expect_err("typed shield role mismatch"),
        ];
        for error in errors {
            assert!(matches!(
                &error,
                TxError::Intent(IntentError::Validation(_))
            ));
            assert_eq!(
                error.to_string(),
                "batch selects the hotkey role at runtime; use the dynamic executor API with this intent"
            );
        }
        assert_no_private_role_access(&keys);
        assert_no_coldkey_access(&keys);
        assert_eq!(keys.access.hotkey_public.load(Ordering::SeqCst), 0);
        assert_eq!(keys.access.hotkey_public_key.load(Ordering::SeqCst), 0);
        assert!(keys.access.events.lock().expect("events").is_empty());
        assert_eq!(intent.builds.load(Ordering::SeqCst), 0);
        assert_eq!(intent.previews.load(Ordering::SeqCst), 0);
        assert_eq!(chain.requests().len(), requests);
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        assert_eq!(
            executor
                .client()
                .nonces()
                .cached(&keys.coldkey.account().to_string())
                .await,
            Some(40)
        );
        assert_eq!(
            executor
                .client()
                .nonces()
                .cached(&keys.hotkey.account().to_string())
                .await,
            Some(20)
        );
        executor.client().close().await;
        Ok(())
    }

    #[tokio::test]
    async fn dynamic_preview_uses_instance_hotkey_without_unlocking_or_leasing_nonce() -> TestResult
    {
        let (executor, chain, keys, intent) = role_fixture().await?;
        let hotkey = keys.hotkey.account();
        executor.client().nonces().pin(&hotkey, 20);
        executor.client().nonces().pin(&keys.coldkey.account(), 40);
        let plan = executor
            .plan_dyn(&intent, &keys, &PlanOptions::default())
            .await?;
        assert!(plan.ok());
        assert_eq!(plan.signer, SignerRole::Hotkey);
        assert_eq!(plan.signer_address, hotkey);
        assert_eq!(plan.to_json()["signer"], json!("hotkey"));
        assert_eq!(plan.effects, [format!("dispatch={hotkey} payer={hotkey}")]);
        let preview = executor
            .preflight_dyn(&intent, &keys, &PlanOptions::default())
            .await?;
        assert_eq!(
            preview.effects,
            [format!("dispatch={hotkey} payer={hotkey}")]
        );
        assert_eq!(
            *intent.origins.lock().expect("origins"),
            [(hotkey, hotkey), (hotkey, hotkey)]
        );
        assert_eq!(intent.builds.load(Ordering::SeqCst), 2);
        assert_eq!(intent.previews.load(Ordering::SeqCst), 2);
        assert!(keys.access.hotkey_public.load(Ordering::SeqCst) > 0);
        assert_no_coldkey_access(&keys);
        assert_no_private_role_access(&keys);
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        assert_eq!(
            executor.client().nonces().cached(&hotkey.to_string()).await,
            Some(20)
        );
        assert_eq!(
            executor
                .client()
                .nonces()
                .cached(&keys.coldkey.account().to_string())
                .await,
            Some(40)
        );
        executor.client().close().await;
        Ok(())
    }

    #[tokio::test]
    async fn dynamic_normal_and_shield_policy_refusal_precede_private_hotkey_acquisition()
    -> TestResult {
        let (executor, chain, keys, intent) = role_fixture().await?;
        let options = ExecuteOptions {
            policy: Some(Policy {
                max_spend: Some(Balance::<Tao>::ZERO),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(matches!(
            executor.execute_dyn(&intent, &keys, &options).await,
            Err(TxError::Policy(_))
        ));
        assert!(matches!(
            executor
                .submit_shielded_dyn(
                    &intent,
                    &keys,
                    &ShieldOptions {
                        execute: options,
                        ..Default::default()
                    }
                )
                .await,
            Err(TxError::Policy(_))
        ));
        let hotkey = keys.hotkey.account();
        assert_eq!(
            *intent.origins.lock().expect("origins"),
            [(hotkey, hotkey), (hotkey, hotkey)]
        );
        assert_eq!(intent.builds.load(Ordering::SeqCst), 2);
        assert_eq!(intent.previews.load(Ordering::SeqCst), 2);
        assert_no_coldkey_access(&keys);
        assert_no_private_role_access(&keys);
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        assert!(
            executor
                .client()
                .nonces()
                .cached(&hotkey.to_string())
                .await
                .is_none()
        );
        executor.client().close().await;
        Ok(())
    }

    #[tokio::test]
    async fn dynamic_execution_acquires_only_the_instance_hotkey_after_review_and_signs_its_account()
    -> TestResult {
        let (executor, chain, keys, intent) = role_fixture().await?;
        let hotkey = keys.hotkey.account();
        // The cache retains the last allocated nonce, while the node returns
        // its next available value. Distinct nonzero nonces make selecting
        // the static catalog's coldkey observable in the signed bytes too.
        chain.mutate_state(|state| {
            state.set_nonce(hotkey.to_string(), 17);
            state.set_nonce(keys.coldkey.account().to_string(), 41);
        });
        let result = executor
            .execute_dyn(
                &intent,
                &keys,
                &ExecuteOptions {
                    period: None,
                    wait_for_inclusion: false,
                    wait_for_finalization: false,
                    ..Default::default()
                },
            )
            .await?;
        assert!(result.success);
        assert_eq!(result.inclusion, Inclusion::PoolAccepted);
        assert_eq!(result.data["instance_hotkey"], json!(true));
        assert_eq!(
            *keys.access.dynamic_acquisitions.lock().expect("roles"),
            [SignerRole::Hotkey]
        );
        assert_no_coldkey_access(&keys);
        assert_acquisition_after_review(&keys);
        assert_eq!(chain.mutate_state(|state| state.submissions.len()), 1);
        let bytes = chain.mutate_state(|state| state.submissions[0].clone());
        let runtime = executor.client().refresh_runtime().await?;
        let decoded = runtime.core().decode_extrinsic(&bytes, true)?;
        assert_eq!(
            decoded.field("address").and_then(ValueExt::as_str),
            Some(keys.hotkey.account().to_string().as_str())
        );
        assert_eq!(decoded.field("nonce").and_then(ValueExt::as_u128), Some(17));
        assert_eq!(
            *intent.origins.lock().expect("origins"),
            [(keys.hotkey.account(), keys.hotkey.account())]
        );
        assert_eq!(
            executor
                .client()
                .nonces()
                .cached(&keys.hotkey.account().to_string())
                .await,
            Some(17)
        );
        assert_eq!(executor.client().nonces().fresh(&hotkey).await?, 18);
        assert_eq!(
            executor.client().nonces().cached(&hotkey.to_string()).await,
            Some(17),
            "a public next-index probe must not allocate another nonce"
        );
        assert!(
            executor
                .client()
                .nonces()
                .cached(&keys.coldkey.account().to_string())
                .await
                .is_none()
        );
        executor.client().close().await;
        Ok(())
    }

    #[tokio::test]
    async fn dynamic_shield_acquires_only_the_instance_hotkey_after_review() -> TestResult {
        dynamic_shield_with_state_requirement(false).await
    }

    #[tokio::test]
    async fn pinned_state_only_shield_requirement_allows_explicit_shield_submission() -> TestResult
    {
        dynamic_shield_with_state_requirement(true).await
    }

    async fn dynamic_shield_with_state_requirement(required_at: bool) -> TestResult {
        let (executor, chain, keys, mut intent) = role_fixture().await?;
        intent.required_at = required_at;
        let result = executor
            .submit_shielded_dyn(
                &intent,
                &keys,
                &ShieldOptions {
                    execute: ExecuteOptions {
                        wait_for_inclusion: false,
                        wait_for_finalization: false,
                        ..Default::default()
                    },
                    ..Default::default()
                },
            )
            .await?;
        assert!(result.success);
        assert_eq!(result.inclusion, Inclusion::PoolAccepted);
        assert_eq!(
            *keys.access.dynamic_acquisitions.lock().expect("roles"),
            [SignerRole::Hotkey]
        );
        assert_no_coldkey_access(&keys);
        assert_acquisition_after_review(&keys);
        assert_eq!(chain.mutate_state(|state| state.submissions.len()), 1);
        let bytes = chain.mutate_state(|state| state.submissions[0].clone());
        let runtime = executor.client().refresh_runtime().await?;
        let decoded = runtime.core().decode_extrinsic(&bytes, true)?;
        assert_eq!(
            decoded.field("address").and_then(ValueExt::as_str),
            Some(keys.hotkey.account().to_string().as_str())
        );
        assert_eq!(
            *intent.origins.lock().expect("origins"),
            [(keys.hotkey.account(), keys.hotkey.account())]
        );
        executor.client().close().await;
        Ok(())
    }

    #[test]
    fn coldkey_warning_follows_python_wrapper_leaves() -> Result<(), Box<dyn std::error::Error>> {
        let runtime = rtao_runtime::RuntimeHandle::parse(
            include_bytes!("../../../vendor/metadata/finney/478.v15.scale"),
            478,
            1,
            42,
        )?;
        let weights = rtao_runtime::calls::subtensor_module::set_weights(
            rtao_primitives::NetUid(1),
            vec![1],
            vec![65535],
            0,
        )
        .compose(&runtime)?;
        let remark = rtao_runtime::calls::system::remark(vec![]).compose(&runtime)?;
        assert!(coldkey_pays(&runtime, &weights)?);
        assert!(!coldkey_pays(&runtime, &remark)?);
        let batch = rtao_runtime::calls::utility::batch_all(vec![remark, weights.clone()])
            .compose(&runtime)?;
        let multisig =
            rtao_runtime::calls::multisig::as_multi_threshold_1(vec![AccountId([2; 32])], batch)
                .compose(&runtime)?;
        let proxy = rtao_runtime::calls::proxy::proxy(
            rtao_core::DynValue::new(AccountId([1; 32]).into_core_value()),
            None,
            multisig,
        )
        .compose(&runtime)?;
        assert!(coldkey_pays(&runtime, &proxy)?);
        let sudo = rtao_runtime::calls::sudo::sudo(weights).compose(&runtime)?;
        assert!(!coldkey_pays(&runtime, &sudo)?); // Python treats sudo itself as a leaf.
        Ok(())
    }

    // pyt: unit/test_fee_filters.py::test_coldkey_pays_fee_calls_matches_runtime_subset
    #[test]
    fn coldkey_fee_table_preserves_the_python_runtime_subset() {
        let calls = rtao_catalog::rt::COLDKEY_PAYS_FEE_CALLS;
        assert!(calls.contains(&("SubtensorModule", "serve_axon")));
        assert!(calls.contains(&("Commitments", "set_commitment")));
        assert!(!calls.contains(&("Balances", "transfer_keep_alive")));
        assert_eq!(
            rtao_catalog::py::COLDKEY_FEE_WARNING,
            "transaction fee is charged to the hotkey owner's coldkey, not the signing hotkey"
        );
    }

    // pyt: unit/test_fee_filters.py::test_iter_call_leaves_unwraps_proxy
    #[test]
    fn coldkey_fee_proxy_leaf_is_the_python_serve_axon_call() -> TestResult {
        let runtime = rtao_runtime::RuntimeHandle::parse(
            include_bytes!("../../../vendor/metadata/finney/478.v15.scale"),
            478,
            1,
            42,
        )?;
        // The Python ComposedCall stub only supplies netuid. An encoded native
        // call also supplies the remaining fields required by pinned metadata.
        let inner = rtao_runtime::calls::subtensor_module::serve_axon(
            rtao_primitives::NetUid(1),
            1,
            u128::from(u32::from_be_bytes([203, 0, 113, 5])),
            8091,
            4,
            4,
            0,
            0,
        )
        .compose(&runtime)?;
        let real: rtao_primitives::Ss58Address =
            "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY".parse()?;
        let outer = rtao_runtime::calls::proxy::proxy(
            rtao_core::DynValue::new(real.account().into_core_value()),
            None,
            inner.clone(),
        )
        .compose(&runtime)?;
        let decoded = runtime.core().decode_spec(
            &rtao_core::runtime::TypeSpec::Call,
            outer.as_bytes(),
            true,
        )?;
        let json = rtao_core::codec::value::to_corpus_json(&decoded);
        assert_eq!(json["call_module"], "Proxy");
        assert_eq!(json["call_function"], "proxy");
        let leaf = &json["call_args"][2]["value"];
        assert_eq!(leaf["call_module"], "SubtensorModule");
        assert_eq!(leaf["call_function"], "serve_axon");
        assert_eq!(leaf["call_args"][0]["value"], 1);
        assert!(!rtao_catalog::rt::COLDKEY_PAYS_FEE_CALLS.contains(&("Proxy", "proxy")));
        assert!(coldkey_pays(&runtime, &inner)?);
        assert!(coldkey_pays(&runtime, &outer)?);
        Ok(())
    }

    // pyt: unit/test_fee_filters.py::test_charges_coldkey_fee_on_leaf
    #[test]
    fn coldkey_fee_associate_evm_leaf_keeps_the_python_classification() -> TestResult {
        let runtime = rtao_runtime::RuntimeHandle::parse(
            include_bytes!("../../../vendor/metadata/finney/478.v15.scale"),
            478,
            1,
            42,
        )?;
        let call = rtao_runtime::calls::subtensor_module::associate_evm_key(
            rtao_primitives::NetUid(1),
            rtao_core::DynValue::new(rtao_core::codec::Value::Bytes(vec![0; 20])),
            0,
            [0; 65],
        )
        .compose(&runtime)?;
        let mut expected = vec![7, 93, 1, 0];
        expected.extend_from_slice(&[0; 20 + 8 + 65]);
        assert_eq!(call.as_bytes(), expected);
        assert!(coldkey_pays(&runtime, &call)?);
        Ok(())
    }
}
