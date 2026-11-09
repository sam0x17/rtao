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
#[derive(Clone, Debug)]
pub struct PlanOptions {
    /// Per-operation policy replaces the executor default when present.
    pub policy: Option<Policy>,
    /// Inner proxy dispatch account.
    pub proxy_for: Option<AccountId>,
    /// Optional exact proxy permission category.
    pub force_proxy_type: Option<ProxyType>,
    /// Bound the complete preview operation.
    pub deadline: Option<Duration>,
    /// Select the MEV Shield route, as `ExecuteOptions::shielded` does for
    /// execution; a required-Shield intent selects it regardless. The fee is
    /// then the inner transaction plus its carrier (Python's
    /// `preflight(shielded=True)` and `plan(shielded=True)`).
    pub shielded: bool,
    /// Mortal era period, as `ExecuteOptions::period`. On the Shield route the
    /// default (or none) quotes the carrier at the 8-block shield era, and only
    /// 4 or 8 blocks are accepted, as execution requires.
    pub period: Option<u32>,
}
impl Default for PlanOptions {
    fn default() -> Self {
        Self {
            policy: None,
            proxy_for: None,
            force_proxy_type: None,
            deadline: None,
            shielded: false,
            period: Some(128),
        }
    }
}

/// Python execution defaults plus bounded waiting and fate reconciliation.
#[derive(Clone, Debug)]
pub struct ExecuteOptions {
    /// Optional logical operation journal, retained across physical attempts.
    pub journal: Option<rtao_client::journal::JournalBinding>,
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
    /// Explicit typed/raw shield opt-in, implemented by the shield execution path.
    pub shielded: bool,
    /// Authorize up to this permanent deposit for automatic recipient
    /// registration, separate from the payment and fees (D-148). A larger
    /// deposit is refused, even when the callback approves it.
    pub max_registration_deposit: Option<Balance<Tao>>,
    /// Consent callback for automatic recipient registration; it must return
    /// `true` for the quote. Without it or an allowance, a call that registers
    /// a recipient is refused before a key is unlocked.
    pub on_registration_sponsorship: Option<crate::SponsorshipConsent>,
}
impl Default for ExecuteOptions {
    fn default() -> Self {
        Self {
            journal: None,
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
            max_registration_deposit: None,
            on_registration_sponsorship: None,
        }
    }
}
impl ExecuteOptions {
    /// Authorize the composed call's automatic registration reserve, if any,
    /// before a signer is resolved (Python's `authorize_registration_sponsorship`).
    async fn authorize_sponsorship(
        &self,
        extras: &Map<String, Value>,
        payer: AccountId,
    ) -> Result<(), TxError> {
        crate::sponsorship::authorize_registration_sponsorship(
            extras,
            payer,
            self.max_registration_deposit,
            self.on_registration_sponsorship.as_ref(),
        )
        .await
    }
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
            shielded: self.shielded,
            period: self.period,
        }
    }
    pub(crate) fn submit_options(&self) -> SubmitOptions {
        SubmitOptions {
            journal: self.journal.clone(),
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
        shield_period(self.period)
    }
}

/// The shield era for a requested period: the default (128) or none selects
/// the 8-block shield era, as Python's `MEV_SHIELD_ERA_PERIOD` substitution.
fn shield_period(period: Option<u32>) -> Result<ShieldPeriod, TxError> {
    match period {
        None | Some(128) | Some(8) => Ok(ShieldPeriod::P8),
        Some(4) => Ok(ShieldPeriod::P4),
        Some(period) => Err(IntentError::InvalidArgument(format!(
            "MEV shield period must be 4 or 8 blocks, got {period}"
        ))
        .into()),
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
    /// Re-seals after a carrier whose decrypted inner was proven never included.
    pub dropped_inner_retries: u32,
    /// Re-seals after a carrier proven to have lost its nonce, never included.
    pub nonce_conflict_retries: u32,
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
            dropped_inner_retries: rtao_client::SHIELD_DROPPED_INNER_RETRIES,
            nonce_conflict_retries: rtao_client::SHIELD_NONCE_CONFLICT_RETRIES,
        }
    }
}

#[derive(Clone, Copy)]
enum RecoveryRequest<'a> {
    Raw(&'a CallBytes),
    Intent(&'a dyn DynIntent),
}

struct ReviewedIntent {
    built: crate::Built,
    safety: crate::IntentSafety,
    preview: IntentPreflight,
    warnings: Vec<String>,
    role: SignerRole,
    /// The selected route: requested, or required by the intent.
    shielded: bool,
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
    auto_shield: bool,
}
impl Executor {
    /// Start without policy limits or private-key ownership.
    pub fn new(client: Client) -> Self {
        Self {
            client,
            policy: None,
            clock: Arc::new(SystemClock),
            registry: None,
            auto_shield: false,
        }
    }
    /// Route account registration through MEV Shield when the network offers
    /// it, as Python's default `shielded=None` does (D-140). Off by default:
    /// `ExecuteOptions::shielded` and `PlanOptions::shielded` stay an explicit
    /// on/off, and with this setting off nothing changes. When on, a plan,
    /// execution or raw call whose call registers a protected account
    /// (`HashedAccounts.register`, `register_derived`,
    /// `register_derived_sponsored`, `Proxy.create_protected_pure` or
    /// `Utility.as_protected_derivative`, also inside proxy, multisig,
    /// derivative, dispatch, sudo, `if_else` and batch wrappers) takes the
    /// Shield route when `MevShield.NextKey` is set, and otherwise is planned
    /// unshielded with a warning. An explicit `shielded` or a required-Shield
    /// intent is unaffected.
    pub fn with_auto_shield(mut self, enabled: bool) -> Self {
        self.auto_shield = enabled;
        self
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
            shielded,
        } = self
            .review_with_keys(intent, keys, signer_address, opts)
            .await?;
        let mut plan = Plan::new(
            built.call,
            intent.spec().name,
            intent.summary_dyn(),
            role,
            signer_address,
        );
        plan.fee = preview.estimated_fee;
        plan.shielded = shielded;
        plan.effects = preview.effects.clone();
        if let Some(real) = opts.proxy_for {
            plan.effects.push(format!(
                "dispatched via proxy as {real} (signed by {signer_address})"
            ));
        }
        plan.warnings = warnings;
        let reserve = crate::policy::setup_reserve(&built.extras);
        if let Some(policy) = opts.policy.as_ref().or(self.policy.as_ref()) {
            plan.violations = policy.check_with_setup_reserve(&safety, plan.fee, reserve);
        }
        plan.violations.extend(preview.blocks.clone());
        plan.extras = std::mem::take(&mut built.extras);
        plan.spend = if reserve.rao() == 0 {
            safety.spend
        } else {
            crate::policy::with_setup_reserve(safety.spend, reserve)
        };
        plan.args = intent.to_json_dyn();
        if let Some(args) = plan.args.as_object_mut() {
            args.remove("op");
        }
        plan.preview = preview.preview.clone();
        Ok((plan, preview))
    }

    async fn review_with_keys(
        &self,
        intent: &dyn DynIntent,
        keys: &dyn KeyView,
        signer_address: AccountId,
        opts: &PlanOptions,
    ) -> Result<ReviewedIntent, TxError> {
        // Fee probes price the signing key's own proof length (D-149).
        let probe = crate::intent::FeeProbe::from_descriptor(
            intent
                .signer_role_dyn()
                .ok()
                .and_then(|role| keys.authority_descriptor(role)),
        );
        Box::pin(crate::intent::with_fee_probe(
            probe,
            self.review_with_keys_probed(intent, keys, signer_address, opts),
        ))
        .await
    }

    async fn review_with_keys_probed(
        &self,
        intent: &dyn DynIntent,
        keys: &dyn KeyView,
        signer_address: AccountId,
        opts: &PlanOptions,
    ) -> Result<ReviewedIntent, TxError> {
        let shielded = opts.shielded;
        intent.validate_dyn()?;
        let semantic = intent.semantic_intent_dyn();
        let declared_safety = semantic.safety_dyn()?;
        let snapshot = View::pin(&self.client).await?;
        // Queries remain pinned to one block; new calls use the head's signing
        // runtime rather than a historical block's execution runtime.
        let runtime = self.client.refresh_runtime().await?;
        // A wrapped call whose compiled shape differs from live metadata is
        // composed from live metadata by name (D-123); composition itself
        // refuses a call the live metadata cannot encode. The difference is
        // reported as a warning, never a refusal here.
        let mut surface_warnings = Vec::new();
        for descriptor in semantic.wraps() {
            let report = rtao_runtime::verify_surface_with(
                &runtime,
                &[rtao_runtime::SurfaceDescriptor::Call(descriptor)],
            );
            if let Some(change) = report
                .changes
                .into_iter()
                .find(|change| change.class == rtao_runtime::SurfaceClass::Breaking)
            {
                surface_warnings.push(format!(
                    "{}.{} differs from this build's descriptor ({}); it is composed from live metadata by name",
                    descriptor.pallet, descriptor.name, change.detail
                ));
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
        let pinned = intent.inner_call_data_dyn();
        // Receiving addresses resolve at the call boundary, under the
        // registration guard composed below (D-148).
        let recipients = crate::hashed::receiving_recipients(semantic)?;
        // Later multisig rounds approve round 1's exact bytes. A rebuild is not
        // always stable (timelock commits, `ALL` balances), so the pinned bytes
        // are reused unless the semantic intent needs its build for policy
        // metadata or carries recipient setup inputs (imported bytes cannot be
        // authenticated by display metadata); then it is rebuilt with the
        // approved dynamic inputs and must reproduce the pinned bytes exactly.
        let verify_pinned = pinned.is_some()
            && (semantic.requires_build_for_policy_dyn()
                || crate::hashed::has_receiving_setup_inputs(semantic, keys)?);
        let mut built = if let Some(call) = pinned.filter(|_| !verify_pinned) {
            call.ensure_runtime(&runtime)
                .map_err(rtao_client::ClientError::from)?;
            intent.wrap_call_dyn(&cx, call.clone()).await?
        } else {
            let mut built = match pinned {
                Some(call) => {
                    call.ensure_runtime(&runtime)
                        .map_err(rtao_client::ClientError::from)?;
                    let inputs = crate::pinned::PinnedInputs::decode(call, &runtime)?;
                    crate::hashed::guarded_build(
                        &recipients,
                        crate::pinned::scope(inputs, semantic.build_dyn(&cx)),
                    )
                    .await?
                }
                None => crate::hashed::guarded_build(&recipients, semantic.build_dyn(&cx)).await?,
            };
            let finalized = if matches!(
                semantic.spec().name,
                "swap_coldkey_announced"
                    | "pow_register"
                    | "execute_proxy_announced"
                    | "announce_coldkey_swap"
            ) {
                Some(self.client.at(rtao_client::BlockRef::Finalized).await?)
            } else {
                None
            };
            let (call, registration) = crate::hashed::with_recipient_registration(
                &crate::hashed::RecipientCx {
                    view: &snapshot,
                    runtime: &runtime,
                    keys,
                    origin,
                    finalized: finalized
                        .as_ref()
                        .map(|view| view as &dyn rtao_client::DynView),
                },
                semantic,
                built.call,
                &recipients,
            )
            .await?;
            built.call = call;
            let wrapped_intent = !std::ptr::addr_eq(
                semantic as *const dyn DynIntent,
                intent as *const dyn DynIntent,
            );
            if (registration.contains_key("hashed_registration")
                || built.extras.contains_key("hashed_registration"))
                && (opts.proxy_for.is_some() || wrapped_intent || semantic.spec().origin == "root")
            {
                return Err(IntentError::validation(
                    "register the hashed recipient first using a direct sponsor wallet; initial registration cannot be wrapped in proxy, multisig or sudo",
                )
                .into());
            }
            built.extras.extend(registration);
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
            if let Some(pinned) = pinned
                && built.call.as_bytes() != pinned.as_bytes()
            {
                return Err(IntentError::validation(
                    "imported multisig call bytes do not match the verified recipient payment and guards or account creation; use the same receiving address, amount and origin as the opening approval",
                )
                .into());
            }
            let wrapped = intent.wrap_call_dyn(&cx, built.call).await?;
            built.call = wrapped.call;
            built.extras =
                crate::multisig::merge_approval_metadata(&[&built.extras, &wrapped.extras]);
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
        let mut preview =
            crate::hashed::guarded_build(&recipients, intent.preflight_dyn(&pfcx)).await?;
        preview.registration_sponsorship =
            crate::sponsorship::registration_sponsorship_quote(&built.extras, dispatch_origin)?;
        preview.account_registration = contains_account_registration(&runtime, &built.call)?;
        // Effects name the recipient as the user wrote it.
        for (_, recipient) in &recipients {
            for spelling in [
                recipient.account.to_string(),
                recipient.account.to_ss58(runtime.core().ss58_format),
            ] {
                for effect in &mut preview.effects {
                    *effect = effect.replace(&spelling, &recipient.address);
                }
            }
        }
        self.setup_reserve_preview(&snapshot, &pfcx, &built.extras, semantic, &mut preview)
            .await;
        self.account_reserve_preview(
            &snapshot,
            &pfcx,
            &built.extras,
            semantic.spend_dyn(),
            &mut preview,
        )
        .await;
        let mut warnings = preview.warnings.clone();
        warnings.extend(surface_warnings);
        let shielded = shielded
            || (!declared_safety.mev_shield_required
                && self
                    .auto_shield_route(&snapshot, &runtime, &built.call, &mut warnings)
                    .await?);
        let role: SignerRole = intent.signer_role_dyn()?;
        if !shielded
            && role == SignerRole::Hotkey
            && opts.proxy_for.is_none()
            && coldkey_pays(&runtime, &built.call)?
        {
            warnings.push(rtao_catalog::py::COLDKEY_FEE_WARNING.into());
        }
        // Python's `_quote_submission`: price both signed transactions of the
        // selected route before approval, without opening keys. A required
        // Shield route is the route even when planning.
        let quote_shielded = shielded || declared_safety.mev_shield_required;
        let carrier_period = if quote_shielded {
            shield_period(opts.period)?.get()
        } else {
            0
        };
        let inner_fee = match preview.estimated_fee {
            Some(fee) => Some(fee),
            None => match pfcx.estimate_fee().await {
                Ok(fee) => Some(fee),
                Err(error) => {
                    warnings.push(format!("could not estimate fee: {error}"));
                    None
                }
            },
        };
        if quote_shielded {
            let probe = crate::intent::fee_probe();
            let carrier = self
                .client
                .estimate_shielded_carrier_fee_as(
                    signer_address,
                    probe.signer(),
                    built.call.clone(),
                    carrier_period,
                )
                .await;
            let carrier = match carrier {
                Err(error) => {
                    warnings.push(format!(
                        "could not estimate MEV-shield carrier fee: {error}"
                    ));
                    if preview.required_free.is_some() {
                        preview.blocks.push(format!(
                            "could not verify free TAO for the shielded call plus its carrier fee; refusing to risk a post-carrier reserve shortfall ({error})"
                        ));
                    }
                    None
                }
                Ok(carrier) => {
                    if let Some(required) = preview.required_free {
                        let available = match preview.available_free {
                            Some(free) => Some(free),
                            None => crate::multisig::free_balance(&snapshot, signer_address)
                                .await
                                .ok(),
                        };
                        let combined = required
                            .rao()
                            .checked_add(carrier.rao())
                            .ok_or(rtao_primitives::UnitError::Overflow)
                            .map_err(IntentError::from)?;
                        let combined = Balance::<Tao>::from_rao(combined);
                        match available {
                            Some(free) => {
                                preview.available_free = Some(free);
                                if required.rao() <= free.rao() && free.rao() < combined.rao() {
                                    preview.blocks.push(format!(
                                        "free TAO ({}) covers the inner call reserve ({}) but not that reserve plus the MEV-shield carrier fee (~{})",
                                        rtao_primitives::DynBalance::from(free),
                                        rtao_primitives::DynBalance::from(required),
                                        rtao_primitives::DynBalance::from(combined),
                                    ));
                                }
                            }
                            None => preview.blocks.push(
                                "could not verify free TAO for the shielded call plus its carrier fee; refusing to risk a post-carrier reserve shortfall".into(),
                            ),
                        }
                        preview.required_free = Some(combined);
                    }
                    Some(carrier)
                }
            };
            preview.estimated_fee = match (inner_fee, carrier) {
                (Some(inner), Some(carrier)) => Some(Balance::from_rao(
                    inner
                        .rao()
                        .checked_add(carrier.rao())
                        .ok_or(rtao_primitives::UnitError::Overflow)
                        .map_err(IntentError::from)?,
                )),
                _ => None,
            };
            let text = |fee: Option<Balance<Tao>>| {
                fee.map_or_else(
                    || "unavailable".to_owned(),
                    |fee| rtao_primitives::DynBalance::from(fee).to_string(),
                )
            };
            preview.facts.extend([
                (
                    "Inner transaction fee (estimated)".to_owned(),
                    text(inner_fee),
                ),
                ("Shield carrier fee (estimated)".to_owned(), text(carrier)),
                (
                    "Total transaction fee (estimated)".to_owned(),
                    text(preview.estimated_fee),
                ),
            ]);
        } else {
            preview.estimated_fee = inner_fee;
        }
        Ok(ReviewedIntent {
            built,
            safety,
            preview,
            warnings,
            role,
            shielded: quote_shielded,
        })
    }

    /// Python's preview of automatic recipient setup (D-148): the maximum
    /// reserve a guard may lock and, for a first payment, the deposit it
    /// reserves, repricing the whole setup batch; a first `transfer` or
    /// `transfer_all` must also leave the sender its retained balance.
    async fn setup_reserve_preview(
        &self,
        view: &rtao_client::Snapshot,
        pfcx: &PreflightCx<'_, dyn rtao_client::DynView + '_>,
        extras: &Map<String, Value>,
        semantic: &dyn DynIntent,
        preview: &mut IntentPreflight,
    ) {
        use rtao_client::ViewExt;
        let maximum = crate::hashed::maximum_setup_deposit(extras);
        if maximum.rao() != 0 {
            preview.facts.push((
                "Account setup reserve (maximum, permanently locked if needed)".into(),
                rtao_primitives::DynBalance::from(maximum).to_string(),
            ));
            if !extras.contains_key("hashed_registration") {
                preview.effects.insert(
                    0,
                    "recipient is already set up; no new reserve unless chain state rolls back"
                        .into(),
                );
            }
        }
        let Some(deposit) = extras
            .get("hashed_registration_deposit_rao")
            .and_then(Value::as_u64)
            .map(Balance::<Tao>::from_rao)
        else {
            return;
        };
        let shown = rtao_primitives::DynBalance::from(deposit).to_string();
        preview.effects.insert(
            0,
            format!("set up recipient and permanently reserve {shown} from the sender"),
        );
        preview.facts.push(("Account setup reserve".into(), shown));
        // An intent-specific quote may have priced only the semantic call;
        // the setup batch is repriced.
        preview.estimated_fee = None;
        if let Some(required) = preview.required_free {
            preview.required_free = Some(Balance::from_rao(
                required.rao().saturating_add(deposit.rao()),
            ));
        }
        if !matches!(semantic.spec().name, "transfer" | "transfer_all") {
            return;
        }
        // The runtime transfer_all runs after registration, so it sweeps only
        // the remainder after the reserve and fee; the new reserve keeps a
        // Balances consumer, so the existential deposit must remain funded.
        let checked = async {
            let account = view
                .storage(
                    &rtao_runtime::storage::system::ACCOUNT,
                    (pfcx.dispatch_origin,),
                )
                .await
                .map_err(IntentError::from)?
                .to_json();
            let data = account.get("data").cloned().unwrap_or_default();
            let field = |name: &str| data.get(name).and_then(Value::as_u64).unwrap_or_default();
            let (free, frozen) = (field("free"), field("frozen"));
            let minimum = view
                .constant(&rtao_runtime::constants::balances::EXISTENTIAL_DEPOSIT)
                .await
                .map_err(IntentError::from)?
                .0;
            Ok::<_, IntentError>((free, frozen.max(minimum)))
        };
        let (free, retained) = match checked.await {
            Ok(values) => values,
            Err(error) => {
                preview.blocks.push(format!(
                    "could not quote first-payment fee before account setup: {error}"
                ));
                return;
            }
        };
        match pfcx.estimate_fee().await {
            Err(error) => preview.blocks.push(format!(
                "could not quote first-payment fee before account setup: {error}"
            )),
            Ok(fee) => {
                let amount = match semantic.spend_dyn() {
                    rtao_primitives::Spend::Bounded(amount) => amount.rao(),
                    _ => 0,
                };
                let required = amount
                    .saturating_add(deposit.rao())
                    .saturating_add(fee.rao())
                    .saturating_add(retained);
                preview.required_free = Some(Balance::from_rao(required));
                preview.available_free = Some(Balance::from_rao(free));
                preview.estimated_fee = Some(fee);
                if free < required {
                    preview.blocks.push(
                        "insufficient spendable balance for payment, account setup reserve and transaction fee"
                            .into(),
                    );
                }
            }
        }
    }

    /// Python's account-reserve preview for calls that create a protected
    /// pure proxy or carry multisig approval obligations: the permanent and
    /// refundable reserves are shown, and every account that pays a reserve,
    /// the outer fee or the payment must hold it above its retained balance
    /// (existential deposit or freeze).
    async fn account_reserve_preview(
        &self,
        view: &rtao_client::Snapshot,
        pfcx: &PreflightCx<'_, dyn rtao_client::DynView + '_>,
        extras: &Map<String, Value>,
        spend: rtao_primitives::Spend,
        preview: &mut IntentPreflight,
    ) {
        use rtao_client::ViewExt;
        let rao = |key: &str| extras.get(key).and_then(Value::as_u64);
        let protected = rao("protected_pure_registration_deposit_rao");
        if let Some(deposit) = protected {
            let deposit = rtao_primitives::DynBalance::from(Balance::<Tao>::from_rao(deposit));
            let refundable = rtao_primitives::DynBalance::from(Balance::<Tao>::from_rao(
                rao("pure_proxy_deposit_rao").unwrap_or_default(),
            ));
            preview.effects.push(format!(
                "authorize a permanent reserve of {deposit} when protected pure-proxy creation executes; killing the proxy does not refund this registration reserve"
            ));
            preview.facts.extend([
                (
                    "Authorized pure-proxy registration reserve (permanent)".into(),
                    deposit.to_string(),
                ),
                (
                    "Authorized pure-proxy deposit (refundable on kill)".into(),
                    refundable.to_string(),
                ),
            ]);
        }
        let approvals = extras
            .get("multisig_approvals")
            .and_then(Value::as_array)
            .filter(|approvals| !approvals.is_empty());
        if protected.is_none() && approvals.is_none() {
            return;
        }
        let maximum = rao("hashed_registration_max_deposit_rao").unwrap_or_default();
        let checked = async {
            let fee = pfcx.estimate_fee().await?.rao();
            let minimum = view
                .constant(&rtao_runtime::constants::balances::EXISTENTIAL_DEPOSIT)
                .await
                .map_err(IntentError::from)?
                .0;
            let mut reserves: Vec<(AccountId, u128)> = Vec::new();
            let mut add = |payer: AccountId, reserve: u128| {
                match reserves.iter_mut().find(|(account, _)| *account == payer) {
                    Some((_, total)) => *total += reserve,
                    None => reserves.push((payer, reserve)),
                }
            };
            let invalid = |what: &str| IntentError::validation(format!("invalid {what}"));
            if let Some(Value::Object(payers)) = extras.get("pure_proxy_reserves") {
                for (payer, reserve) in payers {
                    let payer: AccountId = payer.parse().map_err(|_| invalid("reserve payer"))?;
                    add(payer, u128::from(reserve.as_u64().ok_or_else(|| invalid("reserve"))?));
                }
            }
            let mut approval_deposit = 0u128;
            for approval in approvals.into_iter().flatten() {
                let payer: AccountId = approval["payer"]
                    .as_str()
                    .and_then(|payer| payer.parse().ok())
                    .ok_or_else(|| invalid("approval payer"))?;
                let threshold = approval["threshold"]
                    .as_u64()
                    .and_then(|threshold| u16::try_from(threshold).ok())
                    .ok_or_else(|| invalid("approval threshold"))?;
                let reserve = if approval["opening"].as_bool() == Some(true) && threshold >= 2 {
                    u128::from(crate::multisig::multisig_deposit(view, threshold).await?.rao())
                } else {
                    0
                };
                approval_deposit += reserve;
                add(payer, reserve);
            }
            if approval_deposit != 0 {
                preview.facts.push((
                    "Multisig approval reserve (refundable)".into(),
                    u64::try_from(approval_deposit).map_or_else(
                        |_| format!("{approval_deposit} rao"),
                        |rao| {
                            rtao_primitives::DynBalance::from(Balance::<Tao>::from_rao(rao))
                                .to_string()
                        },
                    ),
                ));
            }
            add(pfcx.fee_payer, 0);
            let spent = match spend {
                rtao_primitives::Spend::Bounded(spend) => u128::from(spend.rao()),
                _ => 0,
            };
            for (payer, reserve) in reserves {
                let account = view
                    .storage(&rtao_runtime::storage::system::ACCOUNT, (payer,))
                    .await
                    .map_err(IntentError::from)?;
                let data = account.as_core().field("data");
                let read = |name: &str| {
                    data.and_then(|data| data.field(name))
                        .and_then(ValueExt::as_u128)
                        .unwrap_or_default()
                };
                let free = read("free");
                let mut required = reserve + read("frozen").max(u128::from(minimum));
                if payer == pfcx.fee_payer {
                    required += u128::from(fee);
                }
                if payer == pfcx.dispatch_origin {
                    required += spent + u128::from(maximum);
                }
                if payer == pfcx.fee_payer {
                    let required = Balance::<Tao>::from_rao(
                        u64::try_from(required).map_err(|_| rtao_primitives::UnitError::Overflow)?,
                    );
                    preview.required_free = Some(match preview.required_free {
                        Some(existing) if existing.rao() > required.rao() => existing,
                        _ => required,
                    });
                    preview.available_free = Some(Balance::from_rao(
                        u64::try_from(free).map_err(|_| rtao_primitives::UnitError::Overflow)?,
                    ));
                }
                if free < required {
                    preview.blocks.push(
                        "insufficient spendable balance for account reserves, payment and transaction fee"
                            .into(),
                    );
                }
            }
            preview.estimated_fee = Some(Balance::from_rao(fee));
            Ok::<_, IntentError>(())
        }
        .await;
        if let Err(error) = checked {
            preview.blocks.push(format!(
                "could not quote account reserves and transaction fee: {error}"
            ));
        }
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
        let result =
            Box::pin(self.execute_with_keys(intent, keys, address, || keys.signer(), opts, budget))
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
        let result = Box::pin(self.submit_shielded_with_keys(
            intent,
            keys,
            keys.address()?,
            || keys.signer(),
            opts,
            budget,
        ))
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
        Box::pin(self.submit_shielded_with_keys(
            intent,
            keys,
            public_address(intent, keys)?,
            || keys.signer_for(role),
            opts,
            budget,
        ))
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
        Box::pin(self.submit_call_with_keys(
            call,
            keys.address()?,
            || keys.signer(),
            opts,
            budget,
            admission,
        ))
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
        Box::pin(self.submit_call_with_keys(
            call,
            address,
            || keys.signer_for(role),
            opts,
            budget,
            admission,
        ))
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
        let auto;
        let opts = if !opts.shielded && self.auto_shield {
            let snapshot = View::pin(&self.client).await?;
            let runtime = self.client.refresh_runtime().await?;
            // A raw call has no plan to carry Python's warning; without a
            // shield key it is submitted unshielded, as Python does.
            let route = self
                .auto_shield_route(&snapshot, &runtime, &call, &mut Vec::new())
                .await?;
            auto = ExecuteOptions {
                shielded: route,
                ..opts.clone()
            };
            &auto
        } else {
            opts
        };
        let (mut scoped, _owner) = scoped_journal_execution(opts)?;
        let shield_period = opts.shielded.then(|| opts.shield_period()).transpose()?;
        let max_fee = opts
            .policy
            .as_ref()
            .or(self.policy.as_ref())
            .and_then(|p| p.max_fee);
        let mut journal_preparation = None;
        if let (Some(period), Some(binding)) = (shield_period, scoped.journal.clone()) {
            if binding
                .record()
                .map_err(rtao_client::ClientError::Journal)?
                .intent
                .account
                != address
            {
                return Err(rtao_client::ClientError::Journal(
                    rtao_client::journal::JournalError::Identity,
                )
                .into());
            }
            let mut submit = scoped.submit_options();
            submit.deadline = budget.remaining(&self.client)?;
            submit.submit_deadline = admission.remaining(&self.client)?;
            let prepare = ClientShieldOptions {
                period,
                wait: submit.wait,
                max_fee,
                finality_wait: rtao_client::SHIELD_FINALITY_WAIT,
                dropped_inner_retries: rtao_client::SHIELD_DROPPED_INNER_RETRIES,
                nonce_conflict_retries: rtao_client::SHIELD_NONCE_CONFLICT_RETRIES,
                submit,
            };
            match Box::pin(self.client.prepare_journal_shield(&prepare)).await {
                Ok(rtao_client::JournalShieldReadiness::Ready(owner)) => {
                    scoped.journal = Some(owner.binding());
                    journal_preparation = Some(owner);
                }
                Ok(rtao_client::JournalShieldReadiness::Finalized(report)) => {
                    return self.restore_journal_result(
                        &binding,
                        *report,
                        RecoveryRequest::Raw(&call),
                        &scoped,
                    );
                }
                Err(error) => return self.lift_submission(Err(error), Map::new()),
            }
        } else if let Some(binding) = &scoped.journal {
            binding
                .can_dispatch()
                .map_err(rtao_client::ClientError::Journal)?;
        }
        let opts = &scoped;
        let _journal_preparation = journal_preparation;
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
        submit.journal = submit
            .journal
            .map(|binding| executor_context(binding, &call, &Map::new(), None, opts));
        submit.deadline = budget.remaining(&self.client)?;
        submit.submit_deadline = admission.remaining(&self.client)?;
        let result = match shield_period {
            Some(period) => {
                Box::pin(self.client.submit_shielded(
                    call.clone(),
                    signer.as_ref(),
                    &ClientShieldOptions {
                        period,
                        wait: submit.wait,
                        max_fee,
                        finality_wait: rtao_client::SHIELD_FINALITY_WAIT,
                        dropped_inner_retries: rtao_client::SHIELD_DROPPED_INNER_RETRIES,
                        nonce_conflict_retries: rtao_client::SHIELD_NONCE_CONFLICT_RETRIES,
                        submit,
                    },
                ))
                .await
            }
            None => {
                self.client
                    .submit(call.clone(), signer.as_ref(), &submit)
                    .await
            }
        };
        match (&opts.journal, result) {
            (Some(binding), Ok(report)) if shield_period.is_some() => {
                self.restore_journal_result(binding, report, RecoveryRequest::Raw(&call), opts)
            }
            (_, result) => self.lift_submission(result, Map::new()),
        }
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
        let mut effective = opts.clone();
        let mut journal_preparation = None;
        let admission = TimeBudget::new(opts.execute.submit_deadline)?;
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
        if let Some(binding) = &opts.execute.journal {
            if opts.execute.submit_options().wait != rtao_client::Wait::Finalization {
                return Err(rtao_client::ClientError::Journal(
                    rtao_client::journal::JournalError::Blocked(
                        "durable shield requires finalized inner wait".into(),
                    ),
                )
                .into());
            }
            if binding
                .record()
                .map_err(rtao_client::ClientError::Journal)?
                .intent
                .account
                != address
            {
                return Err(rtao_client::ClientError::Journal(
                    rtao_client::journal::JournalError::Identity,
                )
                .into());
            }
            let mut submit = opts.execute.submit_options();
            submit.deadline = budget.remaining(&self.client)?;
            submit.submit_deadline = admission.remaining(&self.client)?;
            let prepare = ClientShieldOptions {
                period: opts.period,
                wait: submit.wait,
                max_fee: active_policy.as_ref().and_then(|p| p.max_fee),
                finality_wait: rtao_client::SHIELD_FINALITY_WAIT,
                dropped_inner_retries: opts.dropped_inner_retries,
                nonce_conflict_retries: opts.nonce_conflict_retries,
                submit,
            };
            match Box::pin(self.client.prepare_journal_shield(&prepare)).await {
                Ok(rtao_client::JournalShieldReadiness::Ready(owner)) => {
                    effective.execute.journal = Some(owner.binding());
                    journal_preparation = Some(owner);
                }
                Ok(rtao_client::JournalShieldReadiness::Finalized(report)) => {
                    let result = self.restore_journal_result(
                        binding,
                        *report,
                        RecoveryRequest::Intent(intent),
                        &opts.execute,
                    )?;
                    return self
                        .complete_intent(intent, keys, result, &opts.execute, true, budget)
                        .await;
                }
                Err(error) => return self.lift_submission(Err(error), Map::new()),
            }
        }
        let opts = &effective;
        let _journal_preparation = journal_preparation;
        let mut plan_opts = opts.execute.plan_options();
        plan_opts.policy = active_policy.clone();
        // The shield route, quoted at this submission's shield era.
        plan_opts.shielded = true;
        plan_opts.period = u32::try_from(opts.period.get()).ok();
        let mut resolved_signer = None;
        let mut replanned = false;
        let mut owner_replanned = false;
        loop {
            let reviewed = budget
                .run(
                    &self.client,
                    self.review_with_keys(intent, keys, address, &plan_opts),
                )
                .await?;
            let mut violations = active_policy.as_ref().map_or_else(Vec::new, |policy| {
                policy.check_with_setup_reserve(
                    &reviewed.safety,
                    reviewed.preview.estimated_fee,
                    crate::policy::setup_reserve(&reviewed.built.extras),
                )
            });
            violations.extend(reviewed.preview.blocks);
            if !violations.is_empty() {
                return Err(TxError::Policy(PolicyError { violations }));
            }
            opts.execute.check_expected_call(&reviewed.built.call)?;
            opts.execute
                .authorize_sponsorship(&reviewed.built.extras, address)
                .await?;
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
            submit.journal = submit.journal.map(|binding| {
                executor_context(
                    binding,
                    &reviewed.built.call,
                    &reviewed.built.extras,
                    Some(intent),
                    &opts.execute,
                )
            });
            submit.deadline = budget.remaining(&self.client)?;
            submit.submit_deadline = admission.remaining(&self.client)?;
            let shield_opts = ClientShieldOptions {
                period: opts.period,
                wait: submit.wait,
                // The semantic preflight above owns the exact inner fee model.
                // The client-level guard is for callers without that review.
                max_fee: active_policy.as_ref().and_then(|p| p.max_fee),
                finality_wait: rtao_client::SHIELD_FINALITY_WAIT,
                dropped_inner_retries: opts.dropped_inner_retries,
                nonce_conflict_retries: opts.nonce_conflict_retries,
                submit,
            };
            let result = Box::pin(self.client.submit_shielded(
                reviewed.built.call,
                signer.as_ref(),
                &shield_opts,
            ))
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
            let mut result = match (&opts.execute.journal, result) {
                (Some(binding), Ok(report)) => self.restore_journal_result(
                    binding,
                    report,
                    RecoveryRequest::Intent(intent),
                    &opts.execute,
                )?,
                (_, result) => self.lift_submission(result, reviewed.built.extras)?,
            };
            // Python's shield path adds the same operation data after the
            // carrier's metadata (v488).
            if result.success {
                semantic_result_data(intent.semantic_intent_dyn().spec().name, &mut result);
            }
            if opts.execute.journal.is_none()
                && opts.execute.replan_owner_rate_limit
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

    fn restore_journal_result(
        &self,
        binding: &rtao_client::journal::JournalBinding,
        report: rtao_client::ExtrinsicReport,
        request: RecoveryRequest<'_>,
        opts: &ExecuteOptions,
    ) -> Result<ExtrinsicResult, TxError> {
        let receipt =
            ExtrinsicResult::from_report(report.clone(), self.client.network().label(), Map::new());
        let restore = || {
            let record = binding
                .record()
                .map_err(rtao_client::ClientError::Journal)?;
            let attempt = record
                .finalized_effect()
                .map_err(rtao_client::ClientError::Journal)?
                .filter(|attempt| attempt.hash == report.hash)
                .ok_or_else(|| {
                    rtao_client::ClientError::Journal(rtao_client::journal::JournalError::Identity)
                })?;
            use rtao_client::journal::{JournalError, RecoveryContextError as Reason};
            let refuse = |reason| TxError::RecoveryContext { reason };
            match request {
                RecoveryRequest::Raw(call) => {
                    opts.check_expected_call(call)
                        .map_err(|_| refuse(Reason::ExpectedCallMismatch))?;
                    binding
                        .check_recovered_call(&report, call)
                        .map_err(rtao_client::ClientError::Journal)?;
                }
                RecoveryRequest::Intent(_) => {}
            }
            if let Some(expected) = &opts.expected_call {
                binding
                    .check_recovered_call(&report, expected)
                    .map_err(|error| match error {
                        JournalError::RecoveryContext(Reason::CallMismatch) => {
                            refuse(Reason::ExpectedCallMismatch)
                        }
                        error => rtao_client::ClientError::Journal(error).into(),
                    })?;
                if expected.stamp().spec_version != attempt.spec_version
                    || expected.stamp().transaction_version != attempt.transaction_version
                {
                    return Err(refuse(Reason::ExpectedCallMismatch));
                }
            }
            let original: [u8; 32] = attempt
                .extensions
                .get("call_hash")
                .cloned()
                .and_then(|v| serde_json::from_value(v).ok())
                .ok_or_else(|| refuse(Reason::MissingCallHash))?;
            let context = attempt
                .extensions
                .get("rtao_submission_context_v1")
                .ok_or_else(|| refuse(Reason::MissingExecutorContext))?;
            if context.get("version") != Some(&json!(2))
                || context.get("proxy_for").is_none()
                || context.get("force_proxy_type").is_none()
            {
                return Err(refuse(Reason::MissingExecutorContext));
            }
            if context.get("proxy_for") != Some(&json!(opts.proxy_for))
                || context.get("force_proxy_type") != Some(&json!(opts.force_proxy_type))
            {
                return Err(refuse(Reason::OriginMismatch));
            }
            let kind = match request {
                RecoveryRequest::Intent(_) => "intent",
                RecoveryRequest::Raw(_) => "raw",
            };
            if context.get("version") != Some(&json!(2))
                || context.get("kind") != Some(&json!(kind))
                || context.get("call_hash") != Some(&json!(original))
                || context.get("spec_version") != Some(&json!(attempt.spec_version))
                || context.get("transaction_version") != Some(&json!(attempt.transaction_version))
                || matches!(request,RecoveryRequest::Intent(intent) if context.get("intent")!=Some(&json!(intent.semantic_intent_dyn().spec().name)) || context.get("args")!=Some(&intent.to_json_dyn()))
            {
                return Err(refuse(Reason::ExecutorMismatch));
            }
            let extras = context
                .get("extras")
                .and_then(Value::as_object)
                .cloned()
                .ok_or_else(|| refuse(Reason::MissingExecutorContext))?;
            self.lift_submission(Ok(report), extras)
        };
        restore().map_err(|error| error.with_submitted_receipt(receipt))
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
                // A successful carrier whose inner never landed in its era.
                let inner_expired = shield.is_some_and(|shield| {
                    !report.success && report.hash == shield.carrier_hash && report.error.is_none()
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
                        // Python v488: both fees of a resolved Shield pair.
                        if let Some(fees) = shield.fees {
                            result.data.insert(
                                "shield_carrier_fee_rao".into(),
                                json!(fees.carrier.map(|fee| fee.0)),
                            );
                            result.data.insert(
                                "shield_inner_fee_rao".into(),
                                json!(fees.inner.map(|fee| fee.0)),
                            );
                        }
                    }
                }
                if !result.success && result.error.is_none() {
                    result.error = Some(ChainError::new(
                        result.message.clone(),
                        inner_expired.then(|| "ShieldInnerExpired".to_owned()),
                    ));
                }
                Ok(result)
            }
            Err(SubmitFailure::Pool(error)) => Ok(ExtrinsicResult::rejected(
                ChainError::from_pool(&error),
                None,
            )),
            Err(SubmitFailure::RecoveryContext { receipt, cause }) => {
                let result = ExtrinsicResult::from_report(
                    *receipt,
                    self.client.network().label(),
                    Map::new(),
                );
                Err(TxError::from(rtao_client::ClientError::Journal(cause))
                    .with_submitted_receipt(result))
            }
            Err(SubmitFailure::PendingShield {
                operation,
                pair,
                carrier_hash,
                inner_hash,
                reason,
            }) => Err(TxError::PendingOperation {
                operation,
                pair,
                carrier_hash,
                inner_hash,
                reason,
            }),
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
        Box::pin(self.execute_with_keys(
            intent,
            keys,
            public_address(intent, keys)?,
            || keys.signer_for(role),
            opts,
            budget,
        ))
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
        let required = intent
            .semantic_intent_dyn()
            .safety_dyn()?
            .mev_shield_required;
        if required || opts.shielded {
            return Box::pin(self.submit_shielded_with_keys(
                intent,
                keys,
                address,
                signer,
                &ShieldOptions {
                    period: opts.shield_period()?,
                    execute: opts.clone(),
                    ..Default::default()
                },
                budget,
            ))
            .await;
        }
        let (scoped, _owner) = scoped_journal_execution(opts)?;
        let opts = &scoped;
        if let Some(journal) = &opts.journal {
            journal
                .can_dispatch()
                .map_err(rtao_client::ClientError::Journal)?;
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
            if plan.shielded {
                // Only the opt-in registration default selects Shield here;
                // explicit and required routes returned above (D-140).
                let mut shielded = opts.clone();
                shielded.shielded = true;
                return Box::pin(self.submit_shielded_with_keys(
                    intent,
                    keys,
                    address,
                    signer,
                    &ShieldOptions {
                        period: shielded.shield_period()?,
                        execute: shielded,
                        ..Default::default()
                    },
                    budget,
                ))
                .await;
            }
            if !plan.ok() {
                return Err(TxError::Policy(PolicyError {
                    violations: plan.violations,
                }));
            }
            opts.check_expected_call(plan.call())?;
            // Registration sponsorship is authorized before any key is
            // unlocked, for the exact composed call (D-148).
            opts.authorize_sponsorship(&plan.extras, address).await?;
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
            submit_options.journal = submit_options.journal.map(|binding| {
                executor_context(binding, plan.call(), &plan.extras, Some(intent), opts)
            });
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
                    if opts.journal.is_none()
                        && opts.replan_owner_rate_limit
                        && owner_rate_replan(intent, &result)
                        && !owner_replanned
                    {
                        owner_replanned = true;
                        continue;
                    }
                    if result.success {
                        semantic_result_data(intent.semantic_intent_dyn().spec().name, &mut result);
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
        let completion = async {
            let origin = opts
                .proxy_for
                .map(OriginView::Account)
                .unwrap_or_else(|| intent.origin_view_dyn(keys));
            let view =
                IntentKeyView::new(&origin, semantic.signer_role_dyn()?, semantic.spec().name);
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
                result.clone(),
                view.coldkey_address()?,
                view.hotkey_address(explicit)?,
                opts,
                deferred_dispatch,
                budget.expires,
            )
            .await
        };
        completion
            .await
            .map_err(|cause| cause.with_submitted_receipt(result))
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

/// Python's `Balance.decimal` formatted with `"f"`: only significant fraction digits.
fn decimal_text(rao: u128) -> String {
    let integer = rao / 1_000_000_000;
    let fraction = rao % 1_000_000_000;
    let text = format!("{integer}.{fraction:09}");
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// `reason.name or reason.message` of Python's `chain_error_from_dispatch`.
fn dispatch_reason(error: &Value) -> String {
    if let Some(module) = error.get("Module") {
        let pallet = module.get("index").and_then(Value::as_u64);
        let index = match module.get("error") {
            Some(Value::String(raw)) => {
                let raw = raw.strip_prefix("0x").unwrap_or(raw);
                u64::from_str_radix(raw.get(..2).unwrap_or("0"), 16).ok()
            }
            Some(Value::Array(raw)) => raw.first().and_then(Value::as_u64),
            Some(raw) => raw.as_u64(),
            None => Some(0),
        };
        if let (Some(pallet), Some(index)) = (pallet, index)
            && let Some(info) = rtao_runtime::errors::MODULE_ERRORS.iter().find(|info| {
                u64::from(info.pallet_index) == pallet && u64::from(info.error_index) == index
            })
        {
            return info.name.to_owned();
        }
        return format!(
            "module error {}/{}",
            pallet.map_or_else(|| "-1".into(), |value| value.to_string()),
            index.map_or_else(|| "0".into(), |value| value.to_string())
        );
    }
    if let Value::Object(error) = error {
        for wrapper in [
            "BadOrigin",
            "CannotLookup",
            "Other",
            "Token",
            "Arithmetic",
            "Transactional",
            "Exhausted",
            "Corruption",
            "Unavailable",
            "RootNotAllowed",
        ] {
            let Some(variant) = error.get(wrapper) else {
                continue;
            };
            if matches!(wrapper, "Token" | "Arithmetic" | "Transactional")
                && let Some(name) = variant
                    .as_object()
                    .and_then(|variant| variant.keys().next())
            {
                return name.clone();
            }
            return wrapper.to_owned();
        }
    }
    match error {
        Value::String(name) => name.clone(),
        other => other.to_string(),
    }
}

fn reference_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// What each gamma spend entry did, in order (Python's `_gamma_spent_data`).
///
/// Nothing while the spend has not run: submitted without waiting, or a
/// multisig approval short of its threshold.
fn gamma_spent_data(result: &mut ExtrinsicResult) {
    if result.events.is_empty()
        || result.events.iter().any(|event| {
            event.matches("Multisig", "NewMultisig")
                || event.matches("Multisig", "MultisigApproval")
        })
    {
        return;
    }
    let mut spent = Vec::new();
    let mut skipped = Vec::new();
    for event in &result.events {
        let skip = event.matches("SubtensorModule", "GammaSpendSkipped");
        if !skip && !event.matches("SubtensorModule", "GammaSpent") {
            continue;
        }
        let attributes = rtao_core::codec::value::to_corpus_json(&event.attributes);
        let mut entry = Map::new();
        entry.insert(
            "hotkey_ss58".into(),
            json!(reference_text(&attributes["hotkey"])),
        );
        entry.insert(
            "amount_gamma".into(),
            json!(rtao_primitives::gamma::gamma_from_units(
                attributes["amount"]
                    .as_u64()
                    .map(u128::from)
                    .or_else(|| attributes["amount"].to_string().parse().ok())
                    .unwrap_or_default()
            )),
        );
        entry.insert(
            "reference".into(),
            json!(reference_text(&attributes["reference"])),
        );
        if skip {
            entry.insert(
                "reason".into(),
                json!(dispatch_reason(&attributes["reason"])),
            );
            skipped.push(Value::Object(entry));
        } else {
            spent.push(Value::Object(entry));
        }
    }
    result.data.insert("spent".into(), Value::Array(spent));
    result.data.insert("skipped".into(), Value::Array(skipped));
}

/// What a gamma mint credited, from its `GammaMinted` event (Python's `_gamma_minted_data`).
fn gamma_minted_data(result: &mut ExtrinsicResult) {
    let Some(event) = result
        .events
        .iter()
        .find(|event| event.matches("SubtensorModule", "GammaMinted"))
    else {
        return;
    };
    let attributes = rtao_core::codec::value::to_corpus_json(&event.attributes);
    let units = |value: &Value| {
        value
            .as_u64()
            .map(u128::from)
            .or_else(|| value.to_string().parse().ok())
            .unwrap_or_default()
    };
    let cap = &attributes["daily_spend_cap"];
    result.data.insert(
        "minted".into(),
        json!({
            "hotkey_ss58": reference_text(&attributes["hotkey"]),
            "spender_ss58": reference_text(&attributes["spender"]),
            "daily_spend_cap_gamma": if cap.is_null() {
                Value::Null
            } else {
                json!(rtao_primitives::gamma::gamma_from_units(units(cap)))
            },
            "alpha_burned": decimal_text(units(&attributes["alpha"])),
            "tao_value": decimal_text(units(&attributes["value"])),
            "gamma": rtao_primitives::gamma::gamma_from_units(units(&attributes["gamma"])),
        }),
    );
}

/// Receipt data an operation's own events add (pure proxy coordinates,
/// gamma spends and mints), as Python's executor attaches it.
fn semantic_result_data(op: &str, result: &mut ExtrinsicResult) {
    match op {
        "create_pure_proxy" => pure_created_data(result),
        "spend_gamma" => gamma_spent_data(result),
        "mint_gamma" | "mint_gamma_with_stake" => gamma_minted_data(result),
        _ => {}
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

fn executor_context(
    binding: rtao_client::journal::JournalBinding,
    call: &CallBytes,
    extras: &Map<String, Value>,
    intent: Option<&dyn DynIntent>,
    opts: &ExecuteOptions,
) -> rtao_client::journal::JournalBinding {
    binding.with_submission_context(json!({"version":2,"kind":if intent.is_some(){"intent"}else{"raw"},
        "intent":intent.map(|i|i.semantic_intent_dyn().spec().name),"args":intent.map(|i|i.to_json_dyn()),
        "proxy_for":opts.proxy_for,"force_proxy_type":opts.force_proxy_type,
        "call_hash":call.hash(),"spec_version":call.stamp().spec_version,"transaction_version":call.stamp().transaction_version,"extras":extras}))
}

fn scoped_journal_execution(
    opts: &ExecuteOptions,
) -> Result<
    (
        ExecuteOptions,
        Option<rtao_client::journal::JournalPreparation>,
    ),
    rtao_client::ClientError,
> {
    let owner = opts
        .journal
        .as_ref()
        .map(|binding| binding.reserve())
        .transpose()
        .map_err(rtao_client::ClientError::Journal)?;
    let mut scoped = opts.clone();
    if let Some(owner) = &owner {
        scoped.journal = Some(owner.binding());
    }
    Ok((scoped, owner))
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
impl Executor {
    // py: sdk/python/bittensor/shielding.py:77-89
    /// Python's `registration_shield_default`, behind the opt-in setting.
    pub(crate) async fn auto_shield_route<V: View + ?Sized>(
        &self,
        view: &V,
        runtime: &rtao_runtime::RuntimeHandle,
        call: &CallBytes,
        warnings: &mut Vec<String>,
    ) -> Result<bool, TxError> {
        if !self.auto_shield || !contains_account_registration(runtime, call)? {
            return Ok(false);
        }
        use rtao_client::ViewExt;
        let key: Option<Vec<u8>> = view
            .storage(&rtao_runtime::storage::mev_shield::NEXT_KEY, ())
            .await?;
        if key.is_some_and(|key| !key.is_empty()) {
            return Ok(true);
        }
        warnings.push(
            "MEV Shield is not active on this network; submitting account registration unshielded. Set ExecuteOptions::shielded to require encrypted submission."
                .into(),
        );
        Ok(false)
    }
}

// py: sdk/python/bittensor/shielding.py:10-74
/// Whether a call registers a protected account, directly or inside a
/// single-call wrapper, `Utility.if_else` or a batch (Python's
/// `contains_account_registration`).
pub fn contains_account_registration(
    runtime: &rtao_runtime::RuntimeHandle,
    call: &CallBytes,
) -> Result<bool, TxError> {
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
        let (Some(pallet), Some(function)) = (
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
        match (pallet, function) {
            ("HashedAccounts", "register" | "register_derived" | "register_derived_sponsored")
            | ("Proxy", "create_protected_pure")
            | ("Utility", "as_protected_derivative") => true,
            ("Proxy", "proxy" | "proxy_announced")
            | (
                "Multisig",
                "as_multi" | "as_multi_safe" | "as_multi_threshold_1" | "as_multi_threshold_1_safe",
            )
            | (
                "Utility",
                "as_derivative"
                | "as_safe_derivative"
                | "dispatch_as"
                | "dispatch_as_fallible"
                | "with_weight",
            )
            | ("Sudo", "sudo" | "sudo_unchecked_weight" | "sudo_as") => {
                arg("call").is_some_and(walk)
            }
            ("Utility", "if_else") => {
                arg("main").is_some_and(walk) || arg("fallback").is_some_and(walk)
            }
            ("Utility", "batch" | "batch_all" | "force_batch") => arg("calls")
                .and_then(list)
                .is_some_and(|calls| calls.iter().any(walk)),
            _ => false,
        }
    }
    Ok(walk(&decoded))
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
            ("Proxy", "proxy")
            | (
                "Multisig",
                "as_multi" | "as_multi_threshold_1" | "as_multi_safe" | "as_multi_threshold_1_safe",
            ) => arg("call").is_some_and(walk),
            ("Utility", "batch_all" | "batch" | "force_batch") => arg("calls")
                .and_then(list)
                .is_some_and(|calls| calls.iter().any(walk)),
            pair => rtao_catalog::rt::COLDKEY_PAYS_FEE_CALLS.contains(&pair),
        }
    }
    Ok(walk(&decoded))
}

/// Compose a nested intent the way a direct submission composes it.
///
/// The nested intent builds against `origin` (the account that will dispatch
/// it: the proxied real account, the multisig, the crowdloan creator), then
/// gains the sudo wrapper a root-origin operation needs and its own execution
/// adapter. Its extras are returned for the enclosing dispatch to propagate.
/// `wrapper` names the enclosing dispatch in refusals.
#[doc(alias = "bittensor.executor._compose_nested_intent_call")]
pub async fn compose_nested_intent_call(
    cx: &BuildCx<'_, dyn rtao_client::DynView + '_>,
    intent: &dyn DynIntent,
    origin: OriginView<'_>,
    wrapper: &str,
) -> Result<crate::Built, IntentError> {
    let semantic = intent.semantic_intent_dyn();
    let origin = intent.origin_view_dyn(&origin);
    let inner_cx = BuildCx {
        view: cx.view,
        origin,
        runtime: cx.runtime,
        clock: cx.clock,
        spec: semantic.spec(),
    };
    let recipients = crate::hashed::receiving_recipients(semantic)?;
    let mut built =
        crate::hashed::guarded_build(&recipients, semantic.build_dyn(&inner_cx)).await?;
    // The nested call carries its own recipient guard; the enclosing
    // dispatch propagates its metadata so an outer batch can refuse it.
    let (call, registration) = crate::hashed::with_recipient_registration(
        &crate::hashed::RecipientCx {
            view: cx.view,
            runtime: cx.runtime,
            keys: &origin,
            origin,
            finalized: None,
        },
        semantic,
        built.call,
        &recipients,
    )
    .await?;
    built.call = call;
    built.extras.extend(registration);
    if built.extras.contains_key("hashed_registration") {
        return Err(IntentError::validation(format!(
            "register the hashed recipient first using a direct sponsor wallet; initial registration cannot be wrapped in {wrapper}"
        )));
    }
    if semantic.spec().origin == "root"
        && !is_sudo(cx.runtime, &built.call).map_err(|error| match error {
            TxError::Intent(error) => error,
            error => IntentError::validation(error.to_string()),
        })?
    {
        built.call = rtao_runtime::calls::sudo::sudo(built.call).compose(cx.runtime)?;
    }
    let wrap_cx = BuildCx {
        view: cx.view,
        origin,
        runtime: cx.runtime,
        clock: cx.clock,
        spec: intent.spec(),
    };
    let wrapped = intent.wrap_call_dyn(&wrap_cx, built.call).await?;
    Ok(crate::Built {
        call: wrapped.call,
        extras: crate::multisig::merge_approval_metadata(&[&built.extras, &wrapped.extras]),
    })
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

    #[test]
    fn gamma_receipt_amounts_and_skip_reasons_follow_python() {
        // Python's Balance.decimal formatted with "f".
        assert_eq!(decimal_text(0), "0");
        assert_eq!(decimal_text(1), "0.000000001");
        assert_eq!(decimal_text(1_500_000_000), "1.5");
        assert_eq!(decimal_text(10_000_000_000), "10");
        // chain_error_from_dispatch: catalog name, unknown module, wrappers.
        let known = rtao_runtime::errors::MODULE_ERRORS
            .iter()
            .find(|error| error.pallet == "SubtensorModule" && error.name == "GammaDisabled")
            .expect("generated error");
        assert_eq!(
            dispatch_reason(&json!({"Module": {
                "index": known.pallet_index,
                "error": [known.error_index, 0, 0, 0],
            }})),
            "GammaDisabled"
        );
        assert_eq!(
            dispatch_reason(&json!({"Module": {
                "index": known.pallet_index,
                "error": format!("0x{:02x}000000", known.error_index),
            }})),
            "GammaDisabled"
        );
        assert_eq!(
            dispatch_reason(&json!({"Module": {"index": 250, "error": [7, 0, 0, 0]}})),
            "module error 250/7"
        );
        assert_eq!(
            dispatch_reason(&json!({"Token": {"FundsUnavailable": null}})),
            "FundsUnavailable"
        );
        assert_eq!(
            dispatch_reason(&json!({"Token": "FundsUnavailable"})),
            "Token"
        );
        assert_eq!(dispatch_reason(&json!({"BadOrigin": null})), "BadOrigin");
        assert_eq!(dispatch_reason(&json!("BadOrigin")), "BadOrigin");
    }

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
        const SPEC: &'static rtao_catalog::IntentSpec = &rtao_catalog::INTENTS[8];
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
            include_bytes!("../../../vendor/metadata/finney/482.v15.scale"),
            482,
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

    // pyt: unit/test_shielding.py (contains_account_registration)
    #[test]
    fn account_registration_is_found_through_wrappers_only()
    -> Result<(), Box<dyn std::error::Error>> {
        let runtime = rtao_runtime::RuntimeHandle::parse(
            include_bytes!("../../../vendor/metadata/finney/488.v15.scale"),
            488,
            1,
            42,
        )?;
        let register = rtao_runtime::calls::proxy::create_protected_pure(
            rtao_runtime::models::ProxyType::Any,
            0,
            0,
        )
        .compose(&runtime)?;
        let remark = rtao_runtime::calls::system::remark(vec![]).compose(&runtime)?;
        assert!(contains_account_registration(&runtime, &register)?);
        assert!(!contains_account_registration(&runtime, &remark)?);
        let batch = rtao_runtime::calls::utility::batch_all(vec![remark.clone(), register.clone()])
            .compose(&runtime)?;
        let proxy = rtao_runtime::calls::proxy::proxy(
            rtao_core::DynValue::new(AccountId([1; 32]).into_core_value()),
            None,
            batch,
        )
        .compose(&runtime)?;
        assert!(contains_account_registration(&runtime, &proxy)?);
        let sudo = rtao_runtime::calls::sudo::sudo(register).compose(&runtime)?;
        assert!(contains_account_registration(&runtime, &sudo)?);
        let plain = rtao_runtime::calls::utility::batch_all(vec![remark.clone(), remark])
            .compose(&runtime)?;
        assert!(!contains_account_registration(&runtime, &plain)?);
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
            include_bytes!("../../../vendor/metadata/finney/482.v15.scale"),
            482,
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
