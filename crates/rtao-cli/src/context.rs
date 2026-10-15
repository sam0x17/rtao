// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/context.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Shared public account, network, confirmation and submission context.

use crate::{
    books::{Books, FsAccountLookup, NameResolver},
    error::CliError,
    globals::Globals,
    output::{Output, OutputOptions, ReviewSection},
    prompt::Terminal,
};
use rtao::{
    client::{Client, ClientConfig},
    primitives::AccountId,
    signer::{DynKeySource, SignerRole},
    tx::{DynIntent, ExecuteOptions, PlanOptions, ShieldOptions},
    wallet::{PasswordSources, PromptPolicy, Wallet},
};
use serde_json::{Map, Value};
use std::{
    future::Future,
    sync::{Arc, Mutex},
    time::Duration,
};

/// The resolved shield choice and whether it can be downgraded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShieldChoice {
    /// Encrypt the submitted extrinsic.
    pub enabled: bool,
    /// A required, flag or config opt-in prevents fallback to clear submission.
    pub forced: bool,
}

/// Application-owned context shared by all command handlers.
#[derive(Clone)]
pub struct AppContext {
    /// Flags after root and command resolution.
    pub globals: Globals,
    /// Authenticated shared config values, retained for runtime fallbacks.
    pub config: Map<String, Value>,
    /// Optional injected shared cache store used during chain startup.
    pub config_store: Option<rtao::client::config::Store>,
    /// Shared address/proxy/multisig book readers.
    pub books: Books,
    /// Data and diagnostic streams.
    pub output: Output,
    /// Password sources with CLI-only callbacks; no secret value is logged.
    pub passwords: PasswordSources,
    /// Injected terminal authority for prompts and non-interactive refusal.
    pub terminal: Arc<Mutex<Box<dyn Terminal>>>,
}
impl AppContext {
    /// Construct an application boundary without connecting or unlocking.
    pub fn new(
        globals: Globals,
        config: Map<String, Value>,
        books: Books,
        terminal: Arc<Mutex<Box<dyn Terminal>>>,
    ) -> Self {
        let output = Output::process(OutputOptions {
            json_mode: globals.json,
            quiet: globals.quiet,
            network: Some(globals.network.clone()),
        });
        let passwords = PasswordSources {
            prompt_callback: Some(Arc::new(crate::prompt::TerminalPasswordPrompt::new(
                terminal.clone(),
                crate::prompt::PromptContext::default(),
                false,
            ))),
            native_provider: Some(Arc::new(crate::macos_password::NativePasswordAdapter::new(
                crate::macos_password::MacosPasswords::new(
                    crate::process::SystemProcessRunner::default(),
                ),
            ))),
            password_file: globals.wallet_password_file.clone(),
            keychain: globals.keychain_password,
            macos_prompt: globals.macos_password,
            prompt: PromptPolicy::IfTty,
            ..PasswordSources::default()
        };
        Self {
            globals,
            config,
            config_store: None,
            books,
            output,
            passwords,
            terminal,
        }
    }
    /// Open public wallet layout and bind explicit CLI password sources.
    pub fn wallet(&self) -> Result<Wallet, CliError> {
        Wallet::new(
            &self.globals.wallet_name,
            &self.globals.hotkey_name,
            &self.globals.wallet_path,
        )
        .map(|wallet| wallet.with_password_sources(self.passwords.clone()))
        .map_err(|error| CliError::failure(error.to_string()))
    }
    /// Resolve a target without unlocking; only canonical omitted key fields default.
    pub fn resolve_address(
        &self,
        param: &str,
        value: Option<&str>,
    ) -> Result<Option<AccountId>, CliError> {
        let lookup = FsAccountLookup::new(self.globals.wallet_path.clone())
            .map_err(|e| CliError::failure(e.to_string()))?;
        let resolver = NameResolver::new(
            &self.books,
            &lookup,
            &self.globals.wallet_name,
            &self.globals.hotkey_name,
        );
        let resolved = resolver
            .resolve_address(param, value)
            .map_err(|e| CliError::failure(e.to_string()))?;
        let Some(resolved) = resolved else {
            return Ok(None);
        };
        if let Some(name) = &resolved.name {
            self.output
                .name_address(&resolved.address, name)
                .map_err(|e| CliError::failure(e.to_string()))?;
        }
        resolved
            .address
            .parse()
            .map(Some)
            .map_err(|error| CliError::input(format!("cannot resolve {param}: {error}")))
    }
    /// Resolve proxy flag > config; `self` explicitly bypasses the default.
    pub fn dispatch_proxy(&self, explicit: Option<&str>) -> Result<Option<AccountId>, CliError> {
        match explicit.or(self.globals.proxy_for.as_deref()) {
            Some("self") => Ok(None),
            Some("") => Err(CliError::input(
                "cannot resolve --proxy-for: empty account reference",
            )),
            Some(value) => self.resolve_address("proxy_for", Some(value)),
            None => {
                let Some(configured) = self
                    .config
                    .get("proxy_for")
                    .filter(|value| json_truthy(value))
                else {
                    return Ok(None);
                };
                let configured = crate::globals::config_string(configured);
                self.output.message(&format!("dispatching via configured proxy_for {} — pass `--proxy-for self` to sign directly",rtao::primitives::pyfmt::string_repr(&configured))).map_err(|e|CliError::failure(e.to_string()))?;
                self.resolve_address("proxy_for", Some(&configured))
            }
        }
    }
    /// Required shielding > flag > persistent config > intent default.
    pub fn resolve_mev_shield(
        &self,
        default: bool,
        required: bool,
        op: &str,
    ) -> Result<ShieldChoice, CliError> {
        let configured = self
            .config
            .get("mev_shield")
            .filter(|value| !value.is_null())
            .map(json_truthy);
        if required {
            if self.globals.mev_shield == Some(false)
                || self.config.get("mev_shield") == Some(&Value::Bool(false))
            {
                return Err(CliError::input(format!("{op} must be submitted MEV-shielded")).with_help("collateral / burned-registration AMM fills cannot run unshielded; omit --no-mev-shield"));
            }
            return Ok(ShieldChoice {
                enabled: true,
                forced: true,
            });
        }
        if let Some(enabled) = self.globals.mev_shield.or(configured) {
            Ok(ShieldChoice {
                enabled,
                forced: enabled,
            })
        } else {
            Ok(ShieldChoice {
                enabled: default,
                forced: false,
            })
        }
    }
    /// Confirm a mutation, refusing without `--yes` in JSON or piped sessions.
    pub fn confirm(&self, question: &str) -> Result<(), CliError> {
        if self.globals.assume_yes {
            return Ok(());
        }
        if self.globals.json
            || !self
                .terminal
                .lock()
                .map_err(|_| CliError::failure("terminal lock poisoned"))?
                .stdin_is_tty()
        {
            return Err(CliError::failure("refusing to submit without confirmation")
                .with_help("pass `--yes` to skip the prompt in non-interactive sessions"));
        }
        let mut terminal = self
            .terminal
            .lock()
            .map_err(|_| CliError::failure("terminal lock poisoned"))?;
        loop {
            let reply = terminal
                .read_line(&format!("{question} [y/N]: "))
                .map_err(|error| match error {
                    crate::prompt::PromptError::Aborted => CliError::aborted(),
                    _ => CliError::failure("could not read confirmation"),
                })?;
            match crate::prompt::python_strip(&reply).to_lowercase().as_str() {
                "y" | "yes" => return Ok(()),
                "" | "n" | "no" => return Err(CliError::failure("aborted.")),
                _ => terminal
                    .message("Please enter Y or N")
                    .map_err(|_| CliError::failure("could not write confirmation prompt"))?,
            }
        }
    }
    /// Apply endpoint pool overrides without touching host settings.
    pub fn client_config(&self) -> Result<ClientConfig, CliError> {
        let mut config = ClientConfig::for_network(&self.globals.network)
            .map_err(|e| CliError::failure(e.to_string()))?;
        if let Some(value) = &self.globals.fallback_endpoints {
            let primary = config
                .session
                .pool
                .urls
                .first()
                .cloned()
                .ok_or_else(|| CliError::input("network has no primary endpoint"))?;
            config.session.pool.urls = std::iter::once(primary)
                .chain(endpoint_pool(value)?)
                .collect();
        }
        if let Some(value) = &self.globals.archive_endpoints {
            config.session.pool.archive = endpoint_pool(value)?;
        }
        config.session.retry_forever = self.globals.retry_forever;
        config.config_store = self.config_store.clone();
        Ok(config)
    }
    /// Connect, run real typed client work, then close on success or error.
    pub async fn run<T, F, Fut>(&self, work: F) -> Result<T, CliError>
    where
        F: FnOnce(Client) -> Fut,
        Fut: Future<Output = Result<T, CliError>>,
    {
        self.run_with(&NetworkConnector, work).await
    }
    /// Downgrade a default shield on an unavailable key; forced or
    /// required shielding refuses before key acquisition or submission.
    pub async fn honor_shield(
        &self,
        client: &Client,
        _payer: AccountId,
        mut choice: ShieldChoice,
        required: bool,
        op: &str,
    ) -> Result<ShieldChoice, CliError> {
        if !choice.enabled {
            return Ok(choice);
        }
        let key = rtao::reads::generated::interfaces::mev_shield_next_key(client)
            .await
            .map_err(|e| CliError::failure(e.to_string()))?;
        if key.is_none() {
            if choice.forced || required {
                return Err(CliError::input(
                    "MEV shield is not active on this network (MevShield.NextKey is unset)",
                )
                .with_help(if required {
                    format!("{op} cannot submit unshielded")
                } else {
                    "pass --no-mev-shield to submit unshielded".into()
                }));
            }
            choice.enabled = false;
            self.output
                .message("MEV shield is not active on this network — submitting unshielded")
                .map_err(|e| CliError::failure(e.to_string()))?;
            return Ok(choice);
        }
        Ok(choice)
    }
    /// Best-effort carrier warning, including Python's second balance probe
    /// after a balance or fee failure. A known zero balance is sufficient.
    pub async fn shield_fee_shortfall(&self, client: &Client, payer: AccountId) -> Option<String> {
        let pair = async {
            let free = rtao::reads::generated::interfaces::balance(client, payer)
                .await
                .ok()?;
            let fee = client.estimate_shielded_carrier_fee(payer).await.ok()?;
            Some((free, fee))
        }
        .await;
        if let Some((free, fee)) = pair {
            if free.rao >= fee.rao() {
                return None;
            }
            let fee =
                rtao::primitives::DynBalance::from_rao(fee.rao(), rtao::primitives::NetUid::ROOT);
            return Some(format!(
                "free TAO ({free}) cannot cover the MEV-shield outer carrier fee (~{fee}); alpha fees only apply to the unshielded call — use --no-mev-shield"
            ));
        }
        let free = rtao::reads::generated::interfaces::balance(client, payer)
            .await
            .ok()?;
        (free.rao==0).then(||"free TAO cannot cover the MEV-shield outer carrier fee; alpha fees only apply to the unshielded call — use --no-mev-shield".into())
    }
    /// Resolve a local carrier shortfall before preview or confirmation.
    pub async fn preflight_shield_fee(
        &self,
        client: &Client,
        payer: AccountId,
        mut choice: ShieldChoice,
        required: bool,
        op: &str,
    ) -> Result<ShieldChoice, CliError> {
        if choice.enabled && self.shield_fee_shortfall(client, payer).await.is_some() {
            if choice.forced || required {
                return Err(CliError::input("MEV-shielded submission needs free TAO for the outer carrier fee").with_help(if required {format!("{op} cannot submit unshielded")} else {"pass --no-mev-shield to submit unshielded (alpha fees work on the bare call), or fund the signing account with free TAO".into()}));
            }
            choice.enabled = false;
            self.output.message("free TAO cannot cover the outer carrier fee (alpha fees only work on the unshielded call)").map_err(|e|CliError::failure(e.to_string()))?;
        }
        Ok(choice)
    }
    /// Run the uniform plan/confirm/execute flow over a real dynamic intent.
    /// The executor is intentionally built without a library policy, as btcli is.
    pub async fn submit_with(
        &self,
        client: &Client,
        intent: &dyn DynIntent,
        keys: &dyn DynKeySource,
        wait_for_finalization: bool,
        replay: Option<&str>,
    ) -> Result<Option<rtao::tx::ExtrinsicResult>, CliError> {
        self.submit_with_hooks(client, intent, keys, wait_for_finalization, replay, None)
            .await
    }
    /// Run the same mutation flow with optional recovery callbacks. Callbacks
    /// never run for dry runs; the receipt hook precedes rendering a failure.
    pub async fn submit_with_hooks(
        &self,
        client: &Client,
        intent: &dyn DynIntent,
        keys: &dyn DynKeySource,
        wait_for_finalization: bool,
        replay: Option<&str>,
        hooks: Option<&dyn SubmissionHooks>,
    ) -> Result<Option<rtao::tx::ExtrinsicResult>, CliError> {
        self.submit_with_keys_options(
            client,
            intent,
            keys,
            SubmissionOptions {
                wait_for_finalization,
                replay,
                hooks,
                period: None,
                execution: None,
                ..Default::default()
            },
        )
        .await
    }
    /// Inject keys while preserving every semantic execution option and hook.
    pub async fn submit_with_keys_options(
        &self,
        client: &Client,
        intent: &dyn DynIntent,
        keys: &dyn DynKeySource,
        options: SubmissionOptions<'_>,
    ) -> Result<Option<rtao::tx::ExtrinsicResult>, CliError> {
        let outcome = self
            .perform_submission(
                client,
                intent,
                keys,
                SubmissionControl {
                    mev_shield_default: options.mev_shield_default,
                    wait_for_finalization: options.wait_for_finalization,
                    replan_owner_rate_limit: options.replan_owner_rate_limit.unwrap_or(true),
                    bind_reviewed_call: options.bind_reviewed_call,
                    deadline: options.deadline,
                    replay: options.replay,
                    hooks: options.hooks,
                    period: options.period,
                    execution: options.execution,
                    factory: None,
                },
            )
            .await?;
        self.finish_submission(intent, outcome).await
    }
    async fn finish_submission(
        &self,
        intent: &dyn DynIntent,
        outcome: Option<SubmissionOutcome>,
    ) -> Result<Option<rtao::tx::ExtrinsicResult>, CliError> {
        let Some(outcome) = outcome else {
            return Ok(None);
        };
        let result = outcome.into_receipt()?;
        if !self
            .output
            .result(&result, &format!("submitted {}", intent.spec().name))
            .map_err(|error| known_receipt_error(CliError::failure(error.to_string()), &result))?
        {
            return Err(CliError::failure(""));
        }
        Ok(Some(result))
    }
    async fn perform_submission(
        &self,
        client: &Client,
        intent: &dyn DynIntent,
        keys: &dyn DynKeySource,
        control: SubmissionControl<'_>,
    ) -> Result<Option<SubmissionOutcome>, CliError> {
        let proxy_for = self.dispatch_proxy(None)?;
        let force_proxy_type = self
            .globals
            .force_proxy_type
            .as_deref()
            .map(|v| {
                serde_json::from_value(Value::String(v.into()))
                    .map_err(|e| CliError::input(e.to_string()))
            })
            .transpose()?;
        if force_proxy_type.is_some() && proxy_for.is_none() {
            return Err(CliError::input("--force-proxy-type requires --proxy-for"));
        }
        let executor = rtao::Executor::new(client.clone());
        let spec = intent.semantic_intent_dyn().spec();
        let mut choice = self.resolve_mev_shield(
            control
                .mev_shield_default
                .unwrap_or(spec.mev_shield_default),
            spec.mev_shield_required,
            spec.name,
        )?;
        if let Some(period) = control.period
            && (!(4..=65_536).contains(&period) || !period.is_power_of_two())
        {
            return Err(CliError::input(
                "--period must be a power of two between 4 and 65536 blocks",
            ));
        }
        let role = intent
            .signer_role_dyn()
            .map_err(|error| CliError::input(error.to_string()))?;
        let payer: AccountId = match role {
            SignerRole::Hotkey => keys.hotkey_address(),
            SignerRole::Coldkey => keys.coldkey_address(),
        }
        .or_else(|error| keys.bare_signer_account().ok_or(error))
        .map_err(|e| CliError::failure(e.to_string()))?;
        if !self.globals.uses_external_signer() {
            choice = self
                .preflight_shield_fee(client, payer, choice, spec.mev_shield_required, spec.name)
                .await?;
        }
        if control.period.is_some() || (!self.globals.dry_run && control.execution.is_some()) {
            choice = self
                .honor_shield(client, payer, choice, spec.mev_shield_required, spec.name)
                .await?;
            if choice.enabled
                && control
                    .period
                    .is_some_and(|period| !matches!(period, 4 | 8))
            {
                return Err(CliError::input("MEV shield period must be 4 or 8 blocks"));
            }
        }
        let plan_options = PlanOptions {
            proxy_for,
            force_proxy_type,
            deadline: control.deadline,
            ..Default::default()
        };
        // Strict SDK planning validates PoW freshness. Actual direct requests
        // can refresh public work first; previews never start CPU workers and
        // saved multisig rounds retain their already reviewed call bytes.
        let prepared = if !self.globals.dry_run && !choice.enabled {
            match control.execution {
                Some(execution) => {
                    execution
                        .prepare(client, intent, keys, &plan_options)
                        .await?
                }
                None => None,
            }
        } else {
            None
        };
        let intent = prepared.as_deref().unwrap_or(intent);
        let (mut plan, preflight) = executor
            .plan_and_preflight_dyn(intent, keys, &plan_options)
            .await
            .map_err(|e| CliError::failure(e.to_string()))?;
        if let Some(period) = control.period {
            plan.effects
                .push(format!("mortal transaction period: {period} blocks"));
        }
        if self.globals.dry_run {
            if choice.enabled {
                plan.effects.push("submitted MEV-shielded: the call stays encrypted in the mempool until the block author decrypts and executes it".into());
                if let Some(warning) = self.shield_fee_shortfall(client, plan.signer_address).await
                {
                    plan.warnings.push(warning);
                }
            }
            self.output
                .plan(&plan, control.replay)
                .map_err(|e| CliError::failure(e.to_string()))?;
            if !plan.ok() {
                return Err(CliError::failure("transaction preview contains violations"));
            }
            return Ok(None);
        }
        for warning in &plan.warnings {
            self.output
                .message(warning)
                .map_err(|e| CliError::failure(e.to_string()))?;
        }
        if !plan.ok() {
            return Err(CliError::failure(format!(
                "transaction refused: {}",
                plan.violations.join("; ")
            )));
        }
        let payer = match plan.signer {
            SignerRole::Coldkey => keys.coldkey_address(),
            SignerRole::Hotkey => keys.hotkey_address(),
        }
        .or_else(|error| keys.bare_signer_account().ok_or(error))
        .map_err(|e| CliError::failure(e.to_string()))?;
        let choice = self
            .honor_shield(client, payer, choice, spec.mev_shield_required, spec.name)
            .await?;
        let mut transaction = vec![
            ("signer".into(), serde_json::json!(plan.signer_address)),
            ("role".into(), serde_json::json!(plan.signer.as_str())),
            ("MEV shield".into(), serde_json::json!(choice.enabled)),
        ];
        if let Some(proxy) = proxy_for {
            transaction.push(("proxy for".into(), serde_json::json!(proxy)));
        }
        if let Some(period) = control.period {
            transaction.push(("period (blocks)".into(), serde_json::json!(period)));
        }
        if let Some(fee) = plan.fee {
            transaction.push((
                "estimated fee (TAO)".into(),
                serde_json::json!(fee.to_decimal_string()),
            ));
        }
        let args = plan
            .args
            .as_object()
            .map(|args| {
                args.iter()
                    .map(|(name, value)| (name.clone(), value.clone()))
                    .collect()
            })
            .unwrap_or_default();
        self.output
            .transaction_card(&[
                ReviewSection {
                    title: plan.summary.clone(),
                    rows: args,
                },
                ReviewSection {
                    title: "Transaction".into(),
                    rows: transaction,
                },
                ReviewSection {
                    title: preflight.facts_title.as_str().into(),
                    rows: preflight
                        .facts
                        .into_iter()
                        .map(|(name, value)| (name, Value::String(value)))
                        .collect(),
                },
                ReviewSection {
                    title: "Effects".into(),
                    rows: plan
                        .effects
                        .iter()
                        .enumerate()
                        .map(|(i, effect)| ((i + 1).to_string(), serde_json::json!(effect)))
                        .collect(),
                },
            ])
            .map_err(|error| CliError::failure(error.to_string()))?;
        self.confirm(&format!("{}?", intent.summary_dyn()))?;
        if let Some(hooks) = control.hooks {
            hooks.before_submit_with_plan(&plan).await?;
        }
        let acquired = match &control.factory {
            Some(binding) => Some(binding.factory.source(self, binding.selection).await?),
            None => None,
        };
        let signing = acquired.as_deref().unwrap_or(keys);
        let opts = ExecuteOptions {
            expected_call: control.bind_reviewed_call.then(|| plan.call().clone()),
            period: control.period.or(ExecuteOptions::default().period),
            proxy_for,
            force_proxy_type,
            wait_for_finalization: control.wait_for_finalization,
            replan_owner_rate_limit: control.replan_owner_rate_limit,
            deadline: control.deadline,
            // Durable callers own any replay decision. No additional pool
            // attempts are introduced by the CLI execution wrapper.
            retries: 0,
            shielded: choice.enabled,
            ..Default::default()
        };
        let result = async {
            if acquired.is_some() {
                let actual = match role {
                    SignerRole::Hotkey => signing.hotkey_address(),
                    SignerRole::Coldkey => signing.coldkey_address(),
                }
                .or_else(|error| signing.bare_signer_account().ok_or(error))
                .map_err(|error| CliError::failure(error.to_string()))?;
                if actual != plan.signer_address {
                    return Err(CliError::input(
                        "selected signing source does not match the reviewed approval member",
                    ));
                }
            }
            if choice.enabled {
                executor
                    .submit_shielded_dyn(
                        intent,
                        signing,
                        &ShieldOptions {
                            period: if control.period == Some(4) {
                                rtao::client::ShieldPeriod::P4
                            } else {
                                rtao::client::ShieldPeriod::P8
                            },
                            execute: opts.clone(),
                            ..Default::default()
                        },
                    )
                    .await
            } else if let Some(execution) = control.execution {
                return execution.execute(client, intent, signing, &opts).await;
            } else {
                executor.execute_dyn(intent, signing, &opts).await
            }
            .map_err(|e| CliError::failure(e.to_string()))
        }
        .await;
        // The raw receipt must reach durable storage before releasing an
        // external signer or performing fallible follow-up work. A failed
        // journal write still releases the source and retains the receipt.
        let hook_error = match (&result, control.hooks) {
            (Ok(receipt), Some(hooks)) => hooks.on_result(receipt).await.err(),
            _ => None,
        };
        let release_error = if let (Some(binding), Some(acquired)) = (&control.factory, &acquired) {
            binding
                .factory
                .release(
                    binding.selection,
                    acquired.as_ref(),
                    result.as_ref().is_ok_and(|receipt| receipt.success),
                )
                .await
                .err()
        } else {
            None
        };
        // A submission error remains primary when closing the source also
        // fails. Successful submissions retain their receipt for recovery.
        let result = result?;
        Ok(Some(SubmissionOutcome {
            receipt: result,
            release_error,
            hook_error,
        }))
    }
}

struct ApprovalFactory<'a> {
    factory: &'a dyn crate::call::RawSignerFactory,
    selection: &'a crate::call::SigningSelection,
}
struct SubmissionControl<'a> {
    mev_shield_default: Option<bool>,
    bind_reviewed_call: bool,
    replan_owner_rate_limit: bool,
    deadline: Option<Duration>,
    period: Option<u32>,
    execution: Option<&'a dyn SubmissionExecution>,
    wait_for_finalization: bool,
    replay: Option<&'a str>,
    hooks: Option<&'a dyn SubmissionHooks>,
    factory: Option<ApprovalFactory<'a>>,
}
struct SubmissionOutcome {
    receipt: rtao::tx::ExtrinsicResult,
    release_error: Option<CliError>,
    hook_error: Option<CliError>,
}
impl SubmissionOutcome {
    fn into_receipt(self) -> Result<rtao::tx::ExtrinsicResult, CliError> {
        if let Some(error) = self.hook_error {
            return Err(known_receipt_error(error, &self.receipt));
        }
        if self.receipt.success
            && let Some(error) = self.release_error
        {
            return Err(known_receipt_error(error, &self.receipt));
        }
        Ok(self.receipt)
    }
}

fn known_receipt_error(mut error: CliError, receipt: &rtao::tx::ExtrinsicResult) -> CliError {
    let recovery = format!(
        "the transaction was already submitted{}; inspect that result before retrying",
        receipt
            .extrinsic_id
            .as_ref()
            .map(|id| format!(" as {id}"))
            .unwrap_or_default(),
    );
    error.help = Some(match error.help.take() {
        Some(help) => format!("{help}; {recovery}"),
        None => recovery,
    });
    error
}

/// Recovery callbacks owned by commands with sequential or durable workflows.
pub trait SubmissionHooks: Send + Sync {
    /// Persist intent/recovery state after confirmation, before signing.
    fn before_submit(&self) -> rtao::signer::BoxFuture<'_, Result<(), CliError>>;
    /// Persist the reviewed outer call after confirmation, before acquiring an
    /// approval source or private signer. The default retains legacy hooks.
    fn before_submit_with_plan<'a>(
        &'a self,
        _plan: &'a rtao::tx::Plan,
    ) -> rtao::signer::BoxFuture<'a, Result<(), CliError>> {
        self.before_submit()
    }
    /// Persist each known raw outcome exactly once, before source release,
    /// follow-up lookup or rendering can fail. It has no follow-up annotation.
    fn on_result<'a>(
        &'a self,
        result: &'a rtao::tx::ExtrinsicResult,
    ) -> rtao::signer::BoxFuture<'a, Result<(), CliError>>;
    /// Update durable recovery details after saved-multisig enrichment. This
    /// never replaces the earlier raw-receipt callback and must be idempotent.
    fn after_followup<'a>(
        &'a self,
        _result: &'a rtao::tx::ExtrinsicResult,
    ) -> rtao::signer::BoxFuture<'a, Result<(), CliError>> {
        Box::pin(async { Ok(()) })
    }
}
/// Shared command submission settings, including durable recovery callbacks.
#[derive(Clone, Copy, Default)]
pub struct SubmissionOptions<'a> {
    /// Override the intent's shielding default without forcing the user's choice.
    /// Composed command workflows retain ordinary fallback and explicit flags.
    pub mev_shield_default: Option<bool>,
    /// Refuse rebuilt outer bytes or runtime stamps that differ from the
    /// reviewed plan. Durable workflows opt in; proof-refresh routes do not.
    pub bind_reviewed_call: bool,
    /// Override the direct owner retry; saved approvals always disable it.
    pub replan_owner_rate_limit: Option<bool>,
    /// Bound each preview and execution, including inclusion waiting.
    pub deadline: Option<Duration>,
    /// Optional explicit D030 mortal era; absent retains the existing defaults.
    pub period: Option<u32>,
    /// Optional public preparation and semantic execution strategy.
    /// Saved multisig rounds retain their exact reviewed inner call instead.
    pub execution: Option<&'a dyn SubmissionExecution>,
    /// Wait for finalized inclusion instead of in-block inclusion.
    pub wait_for_finalization: bool,
    /// Optional command displayed with a preview.
    pub replay: Option<&'a str>,
    /// Hooks applied to every submitted approval, but never to a preview.
    pub hooks: Option<&'a dyn SubmissionHooks>,
}
/// Public proof preparation and semantic execution after confirmation.
/// Shielded and saved multisig dispatch keep their ordinary executor.
pub trait SubmissionExecution: Send + Sync {
    /// Refresh public proof materials before strict planning on an actual run.
    /// Called without an unlock capability; omitted for previews and retained
    /// multisig calls. The registration target and dispatch origin must remain
    /// unchanged. The returned intent is both reviewed and executed.
    fn prepare<'a>(
        &'a self,
        _client: &'a Client,
        _intent: &'a dyn DynIntent,
        _public: &'a dyn rtao::signer::KeyView,
        _options: &'a PlanOptions,
    ) -> rtao::signer::BoxFuture<'a, Result<Option<Box<dyn DynIntent>>, CliError>> {
        Box::pin(async { Ok(None) })
    }
    /// Execute the reviewed semantic target with the selected signing source.
    fn execute<'a>(
        &'a self,
        client: &'a Client,
        intent: &'a dyn DynIntent,
        keys: &'a dyn DynKeySource,
        options: &'a ExecuteOptions,
    ) -> rtao::signer::BoxFuture<'a, Result<rtao::tx::ExtrinsicResult, CliError>>;
}
fn json_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(v) => *v,
        Value::String(v) => !v.is_empty(),
        Value::Number(v) => v.as_f64() != Some(0.0),
        Value::Array(v) => !v.is_empty(),
        Value::Object(v) => !v.is_empty(),
    }
}
/// Parse Python's endpoint override: unset is handled by the caller; `none`
/// becomes an empty pool, and comma separated URLs retain their order.
pub fn endpoint_pool(value: &str) -> Result<Vec<url::Url>, CliError> {
    if ["none", "off"]
        .iter()
        .any(|sentinel| crate::prompt::python_strip(value).eq_ignore_ascii_case(sentinel))
    {
        return Ok(Vec::new());
    }
    value
        .split(',')
        .map(crate::prompt::python_strip)
        .filter(|v| !v.is_empty())
        .map(|v| {
            let url = url::Url::parse(v)
                .map_err(|e| CliError::input(format!("invalid WebSocket endpoint {v:?}: {e}")))?;
            if !["ws", "wss"].contains(&url.scheme()) {
                return Err(CliError::input(format!(
                    "endpoint must use ws:// or wss://: {v:?}"
                )));
            }
            Ok(url)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        application::command_tree,
        globals::{Tier, with_globals},
        output::{MemorySink, Stream},
        prompt::{LiveTerminal, PromptError},
    };
    use std::{collections::VecDeque, path::Path};
    struct TerminalFixture {
        tty: bool,
        replies: VecDeque<Result<String, PromptError>>,
        reads: usize,
    }
    impl Terminal for TerminalFixture {
        fn stdin_is_tty(&self) -> bool {
            self.tty
        }
        fn stderr_is_tty(&self) -> bool {
            self.tty
        }
        fn read_line(&mut self, _: &str) -> Result<String, PromptError> {
            self.reads += 1;
            self.replies
                .pop_front()
                .unwrap_or(Err(PromptError::Aborted))
        }
        fn read_secret(&mut self, _: &str) -> Result<rtao::wallet::SecretString, PromptError> {
            Err(PromptError::Unavailable)
        }
        fn message(&mut self, _: &str) -> Result<(), PromptError> {
            Ok(())
        }
    }
    fn context(
        flags: &[&str],
        config: Value,
        terminal: TerminalFixture,
    ) -> Result<AppContext, Box<dyn std::error::Error>> {
        let argv = std::iter::once("rtao")
            .chain(std::iter::once("submit"))
            .chain(flags.iter().copied());
        let parsed = command_tree([with_globals(clap::Command::new("submit"), Tier::Tx)])
            .try_get_matches_from(argv)?;
        let config = config.as_object().cloned().unwrap_or_default();
        let mut globals =
            Globals::resolve(&parsed, &config, Path::new("/isolated/home"), |_| None)?;
        globals.apply(parsed.subcommand().ok_or("submit")?.1, &[])?;
        Ok(AppContext::new(
            globals,
            config,
            Books::default(),
            Arc::new(Mutex::new(Box::new(terminal))),
        ))
    }
    fn terminal(tty: bool, reply: &str) -> TerminalFixture {
        TerminalFixture {
            tty,
            replies: VecDeque::from([Ok(reply.into())]),
            reads: 0,
        }
    }
    #[test]
    fn noninteractive_mutations_refuse_before_any_prompt_and_yes_bypasses()
    -> Result<(), Box<dyn std::error::Error>> {
        let ctx = context(&[], Value::Null, terminal(false, "yes"))?;
        assert_eq!(ctx.confirm("submit?").err().ok_or("must refuse")?.code, 1);
        let ctx = context(&["--json"], Value::Null, terminal(true, "yes"))?;
        assert_eq!(ctx.confirm("submit?").err().ok_or("must refuse")?.code, 1);
        let ctx = context(&["--yes", "--json"], Value::Null, terminal(false, "yes"))?;
        ctx.confirm("submit?")?;
        let ctx = context(&[], Value::Null, terminal(true, "YES"))?;
        ctx.confirm("submit?")?;
        let ctx = context(&[], Value::Null, terminal(true, ""))?;
        assert_eq!(ctx.confirm("submit?").err().ok_or("decline")?.code, 1);
        Ok(())
    }
    #[test]
    fn shield_required_flag_config_default_and_forced_classification()
    -> Result<(), Box<dyn std::error::Error>> {
        let ctx = context(
            &[],
            serde_json::json!({"mev_shield":true}),
            terminal(false, ""),
        )?;
        assert_eq!(
            ctx.resolve_mev_shield(false, false, "call")?,
            ShieldChoice {
                enabled: true,
                forced: true
            }
        );
        let ctx = context(
            &["--no-mev-shield"],
            serde_json::json!({"mev_shield":true}),
            terminal(false, ""),
        )?;
        assert_eq!(
            ctx.resolve_mev_shield(true, false, "call")?,
            ShieldChoice {
                enabled: false,
                forced: false
            }
        );
        assert_eq!(
            ctx.resolve_mev_shield(true, true, "burned_register")
                .err()
                .ok_or("required")?
                .code,
            2
        );
        let ctx = context(
            &["--mev-shield"],
            serde_json::json!({"mev_shield":false}),
            terminal(false, ""),
        )?;
        // Python rejects an explicit persisted false even when the invocation
        // requests true for required operations.
        assert!(
            ctx.resolve_mev_shield(true, true, "burned_register")
                .is_err()
        );
        let ctx = context(&[], Value::Null, terminal(false, ""))?;
        assert_eq!(
            ctx.resolve_mev_shield(true, false, "call")?,
            ShieldChoice {
                enabled: true,
                forced: false
            }
        );
        Ok(())
    }
    #[test]
    fn endpoint_none_and_order_override_network_defaults() -> Result<(), Box<dyn std::error::Error>>
    {
        let ctx = context(
            &[
                "--network",
                "local",
                "--fallback-endpoints",
                "none",
                "--archive-endpoints",
                "ws://127.0.0.1:9945, wss://example.test",
                "--retry-forever",
            ],
            Value::Null,
            terminal(false, ""),
        )?;
        let config = ctx.client_config()?;
        assert_eq!(config.session.pool.urls.len(), 1);
        assert_eq!(config.session.pool.archive.len(), 2);
        assert!(config.session.retry_forever);
        assert!(endpoint_pool("https://example.test").is_err());
        assert_eq!(endpoint_pool(" NONE ")?, Vec::<url::Url>::new());
        let sink = MemorySink::default();
        let out = Output::new(
            OutputOptions {
                json_mode: true,
                ..Default::default()
            },
            Arc::new(sink.clone()),
        );
        out.message("diagnostic")?;
        assert_eq!(sink.text(Stream::Stdout)?, "");
        let _ = LiveTerminal;
        Ok(())
    }

    #[tokio::test]
    async fn real_hotkey_batch_preview_uses_hotkey_payer_without_coldkey_access_or_unlock()
    -> Result<(), Box<dyn std::error::Error>> {
        use rtao::{
            client::Network,
            signer::{BoxFuture, KeyView, Signer, SignerError},
        };
        use rtao_testkit::{FakeChain, FakeState, rpc::RpcClient};
        use std::sync::atomic::{AtomicUsize, Ordering};

        const HOTKEY: AccountId = AccountId([8; 32]);

        #[derive(Default)]
        struct PublicHotkeyOnly {
            hotkey_reads: AtomicUsize,
            coldkey_reads: AtomicUsize,
            acquisitions: AtomicUsize,
        }
        impl KeyView for PublicHotkeyOnly {
            fn hotkey_address(&self) -> Result<AccountId, SignerError> {
                self.hotkey_reads.fetch_add(1, Ordering::SeqCst);
                Ok(HOTKEY)
            }
            fn hotkey_public_key(&self) -> Result<[u8; 32], SignerError> {
                Ok(HOTKEY.0)
            }
            fn coldkey_address(&self) -> Result<AccountId, SignerError> {
                self.coldkey_reads.fetch_add(1, Ordering::SeqCst);
                Err(SignerError::KeyUnavailable(SignerRole::Coldkey))
            }
        }
        impl DynKeySource for PublicHotkeyOnly {
            fn signer_for<'a>(
                &'a self,
                _role: SignerRole,
            ) -> BoxFuture<'a, Result<Arc<dyn Signer>, SignerError>> {
                Box::pin(async move {
                    self.acquisitions.fetch_add(1, Ordering::SeqCst);
                    Err(SignerError::KeyView(
                        "preview must never acquire a private signer".into(),
                    ))
                })
            }
        }

        struct PublicFactory {
            keys: Arc<PublicHotkeyOnly>,
            requested_roles: Mutex<Vec<SignerRole>>,
            releases: AtomicUsize,
        }
        impl ExternalSignerFactory for PublicFactory {
            fn keys<'a>(
                &'a self,
                _globals: &'a Globals,
                role: SignerRole,
            ) -> BoxFuture<'a, Result<Arc<dyn DynKeySource>, CliError>> {
                Box::pin(async move {
                    self.requested_roles
                        .lock()
                        .map_err(|_| CliError::failure("fixture role lock poisoned"))?
                        .push(role);
                    Ok(self.keys.clone() as Arc<dyn DynKeySource>)
                })
            }
            fn release<'a>(
                &'a self,
                _globals: &'a Globals,
                _keys: &'a dyn DynKeySource,
                success: bool,
            ) -> BoxFuture<'a, Result<(), CliError>> {
                Box::pin(async move {
                    assert!(!success, "a preview has no submitted receipt");
                    self.releases.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
            }
        }

        let batch = rtao_intents::Batch::new(vec![
            serde_json::json!({"op":"reset_axon","netuid":3}),
            serde_json::json!({"op":"serve_prometheus","netuid":3,"ip":"127.0.0.1","port":9090}),
        ])?;
        assert_eq!(batch.signer_role()?, SignerRole::Hotkey);

        // Exercise both the injected-key path and the real submit entry point
        // with a public external source. No backend feature or device is needed.
        for external in [false, true] {
            let keys = Arc::new(PublicHotkeyOnly::default());
            let factory = PublicFactory {
                keys: keys.clone(),
                requested_roles: Mutex::new(Vec::new()),
                releases: AtomicUsize::new(0),
            };
            let mut state = FakeState::try_default()?;
            state.seed_python(
                "System",
                "Account",
                &[serde_json::json!(HOTKEY)],
                serde_json::json!({"data":{"free":1_000_000_000u64}}),
            )?;
            let chain = FakeChain::from_state(state);
            let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
            config.track_heads = false;
            config.runtime_cache_dir = None;
            config.config_store = None;
            let client = Client::from_rpc(RpcClient::new(chain.clone()), config).await?;
            let mut ctx = context(
                &["--dry-run", "--json", "--no-mev-shield"],
                Value::Null,
                terminal(false, ""),
            )?;
            let sink = MemorySink::default();
            ctx.output = Output::new(
                OutputOptions {
                    json_mode: true,
                    ..Default::default()
                },
                Arc::new(sink.clone()),
            );
            let receipt = if external {
                // The external factory seam is feature-independent; it returns
                // public identity without touching a Vault service or key file.
                ctx.globals.signer_backend = Some("vault".into());
                ctx.submit(
                    &client,
                    &batch,
                    false,
                    None,
                    Some(&factory),
                    &mut crate::prompt::PromptHistory::default(),
                )
                .await?
            } else {
                ctx.submit_with(&client, &batch, keys.as_ref(), false, None)
                    .await?
            };
            assert!(receipt.is_none());
            let preview: Value = serde_json::from_str(&sink.text(Stream::Stdout)?)?;
            assert_eq!(preview["op"], "batch");
            assert_eq!(preview["signer"], "hotkey");
            assert_eq!(preview["signer_address"], HOTKEY.to_string());
            assert_eq!(preview["ok"], true);
            assert_eq!(preview["violations"], serde_json::json!([]));
            assert!(!preview["fee_tao"].is_null());
            assert!(keys.hotkey_reads.load(Ordering::SeqCst) > 0);
            assert_eq!(keys.coldkey_reads.load(Ordering::SeqCst), 0);
            assert_eq!(keys.acquisitions.load(Ordering::SeqCst), 0);

            // A real fee probe queries this public payer's nonce. This checks
            // the RPC identity as well as the rendered signer label.
            let requests = chain.requests();
            let payer_queries: Vec<_> = requests
                .iter()
                .filter(|request| {
                    matches!(
                        request.method.as_str(),
                        "system_accountNextIndex" | "account_nextIndex"
                    )
                })
                .collect();
            assert!(!payer_queries.is_empty());
            for request in payer_queries {
                assert_eq!(request.params[0], HOTKEY.to_string());
            }
            assert!(chain.mutate_state(|state| state.submissions.is_empty()));
            assert_eq!(
                *factory
                    .requested_roles
                    .lock()
                    .map_err(|_| "fixture roles")?,
                if external {
                    vec![SignerRole::Hotkey]
                } else {
                    vec![]
                }
            );
            assert_eq!(
                factory.releases.load(Ordering::SeqCst),
                usize::from(external)
            );
            client.close().await;
        }
        Ok(())
    }
}

/// Application hook for external signer backends, implemented by their command
/// families when they ship. A missing backend always refuses instead of using
/// a local wallet key under a different signing identity.
pub trait ExternalSignerFactory: Send + Sync {
    /// Prepare public identity and a lazy source for the selected backend.
    /// This may discover an external account for an ordinary invocation but
    /// must not unlock a private key, sign, or submit: `signer_for` runs only
    /// after the durable plan hook. Previews require a public account without
    /// discovery that opens a device or browser.
    fn keys<'a>(
        &'a self,
        globals: &'a Globals,
        role: SignerRole,
    ) -> rtao::signer::BoxFuture<'a, Result<Arc<dyn DynKeySource>, CliError>>;
    /// Report a completed or failed external session, then release its UI.
    fn release<'a>(
        &'a self,
        _globals: &'a Globals,
        _keys: &'a dyn DynKeySource,
        _success: bool,
    ) -> rtao::signer::BoxFuture<'a, Result<(), CliError>> {
        Box::pin(async { Ok(()) })
    }
}

/// Public wallet view plus CLI-only lazy unlock and wrong-password retries.
#[derive(Clone)]
pub struct CliWalletKeys {
    wallet: Wallet,
    hotkey: rtao::wallet::HotkeyFile,
    output: Output,
}
impl CliWalletKeys {
    /// Attach the CLI renderer without unlocking a private key.
    pub fn new(wallet: Wallet, output: Output) -> Self {
        let cold_sources = wallet.coldkey_file().password_sources();
        let sources = PasswordSources {
            prompt: cold_sources.prompt,
            prompt_callback: cold_sources.prompt_callback.clone(),
            ..Default::default()
        };
        let hotkey = wallet.hotkey_only().with_password_sources(sources);
        Self {
            wallet,
            hotkey,
            output,
        }
    }
    /// Borrow the public layout and configured secret-source policy.
    pub fn wallet(&self) -> &Wallet {
        &self.wallet
    }
    async fn unlock(
        &self,
        role: SignerRole,
    ) -> Result<Arc<dyn rtao::signer::Signer>, rtao::wallet::WalletError> {
        for attempts_left in [2, 1, 0] {
            let result = match role {
                SignerRole::Coldkey => self.wallet.unlock_coldkey().await,
                SignerRole::Hotkey => self.hotkey.unlock().await,
            };
            match result {
                Err(rtao::wallet::WalletError::WrongPassword {
                    source: rtao::wallet::PasswordSource::Prompt,
                }) if role == SignerRole::Coldkey && attempts_left > 0 => {
                    self.output
                        .error(
                            "wrong password",
                            crate::output::DiagnosticContext::default(),
                        )
                        .map_err(|_| {
                            rtao::wallet::WalletError::NativePassword("password diagnostic failed")
                        })?;
                }
                result => return result,
            }
        }
        Err(rtao::wallet::WalletError::PasswordRequired)
    }
}
impl rtao::signer::KeyView for CliWalletKeys {
    fn hotkey_address(&self) -> Result<AccountId, rtao::signer::SignerError> {
        self.wallet.hotkey_address()
    }
    fn hotkey_public_key(&self) -> Result<[u8; 32], rtao::signer::SignerError> {
        self.wallet.hotkey_public_key()
    }
    fn coldkey_address(&self) -> Result<AccountId, rtao::signer::SignerError> {
        self.wallet.coldkey_address()
    }
}
impl DynKeySource for CliWalletKeys {
    fn signer_for<'a>(
        &'a self,
        role: SignerRole,
    ) -> rtao::signer::BoxFuture<'a, Result<Arc<dyn rtao::signer::Signer>, rtao::signer::SignerError>>
    {
        Box::pin(async move {
            self.unlock(role)
                .await
                .map_err(|e| rtao::signer::SignerError::KeyView(e.to_string()))
        })
    }
}
impl AppContext {
    /// Resolve signing keys lazily through injected or native external backends.
    pub async fn signing_keys(
        &self,
        role: SignerRole,
        external: Option<&dyn ExternalSignerFactory>,
    ) -> Result<Arc<dyn DynKeySource>, CliError> {
        let backend = self
            .globals
            .signer_backend
            .as_deref()
            .unwrap_or("wallet")
            .trim()
            .to_lowercase();
        if backend == "wallet" {
            return Ok(Arc::new(CliWalletKeys::new(
                self.wallet()?,
                self.output.clone(),
            )));
        }
        match external {
            Some(factory) => factory.keys(&self.globals, role).await,
            None => {
                crate::signer_backends::NativeSignerFactory::new(self)
                    .keys(&self.globals, role)
                    .await
            }
        }
    }
    /// Resolve a public external account for prompts and public PoW preparation.
    ///
    /// Live account discovery does not acquire a signing session. A preview
    /// uses only an already pinned address and never opens a device or browser.
    pub async fn external_signer_public_account(&self) -> Result<Option<AccountId>, CliError> {
        if !self.globals.uses_external_signer() {
            return Ok(None);
        }
        if self.globals.dry_run {
            return self.external_signer_address();
        }
        let keys = self.signing_keys(SignerRole::Coldkey, None).await?;
        keys.bare_signer_account()
            .or_else(|| keys.coldkey_address().ok())
            .map(Some)
            .ok_or_else(|| CliError::input("external signer has no public account"))
    }
    /// Missing-input policy shared by generated and hand-written commands.
    pub fn prompt_context(&self) -> crate::prompt::PromptContext {
        crate::prompt::PromptContext {
            policy: PromptPolicy::IfTty,
            json: self.globals.json,
            quiet: self.globals.quiet,
            assume_yes: self.globals.assume_yes,
            external_signer: self.globals.uses_external_signer(),
            extension_signer: self
                .globals
                .signer_backend
                .as_deref()
                .is_some_and(|v| v.trim().eq_ignore_ascii_case("extension")),
        }
    }
}

/// Inject the chain connection boundary while retaining the real client and
/// executor. Tests can serve authenticated metadata through a fake RPC chain.
pub trait ConnectionFactory: Send + Sync {
    /// Open the selected client configuration.
    fn connect<'a>(
        &'a self,
        config: ClientConfig,
    ) -> rtao::signer::BoxFuture<'a, Result<Client, CliError>>;
}
/// The normal WebSocket client connector.
#[derive(Clone, Copy, Debug, Default)]
pub struct NetworkConnector;
impl ConnectionFactory for NetworkConnector {
    fn connect<'a>(
        &'a self,
        config: ClientConfig,
    ) -> rtao::signer::BoxFuture<'a, Result<Client, CliError>> {
        Box::pin(async move {
            Client::connect(config)
                .await
                .map_err(|e| CliError::failure(e.to_string()))
        })
    }
}
impl AppContext {
    /// Load shared options/books through an injected store and environment.
    /// The caller controls the home path, command-owned global identifiers and
    /// terminal; tests never mutate the process environment or native settings.
    pub async fn from_store(
        root: &clap::ArgMatches,
        command: &clap::ArgMatches,
        reserved: &[&str],
        store: &rtao::client::config::Store,
        home: &std::path::Path,
        env: impl Fn(&str) -> Option<String>,
        terminal: crate::prompt::SharedTerminal,
    ) -> Result<Self, CliError> {
        let config = store
            .load()
            .await
            .map_err(|e| CliError::failure(e.to_string()))?;
        let mut globals = Globals::resolve(root, &config, home, &env).map_err(CliError::input)?;
        globals
            .apply_with_env(command, reserved, &env)
            .map_err(CliError::input)?;
        let books = Books::load(store.paths())
            .await
            .map_err(|e| CliError::failure(e.to_string()))?;
        let mut context = Self::new(globals, config, books, terminal);
        context.config_store = Some(store.clone());
        if let Ok(names) = store.load_subnet_names(&context.globals.network).await {
            context
                .output
                .update_subnet_names(
                    names
                        .chain_values()
                        .values
                        .into_iter()
                        .map(|(netuid, name)| (netuid.get(), name)),
                )
                .map_err(|e| CliError::failure(e.to_string()))?;
        }
        Ok(context)
    }
    /// Connect through a supplied boundary, run typed SDK work and always close
    /// the connected client after the work returns.
    pub async fn run_with<T, F, Fut>(
        &self,
        connector: &dyn ConnectionFactory,
        work: F,
    ) -> Result<T, CliError>
    where
        F: FnOnce(Client) -> Fut,
        Fut: Future<Output = Result<T, CliError>>,
    {
        let client = connector.connect(self.client_config()?).await?;
        let result = async {
            self.output
                .update_token_symbols(
                    client
                        .token_symbols()
                        .await
                        .into_iter()
                        .map(|(netuid, symbol)| (netuid.get(), symbol.into())),
                )
                .map_err(|e| CliError::failure(e.to_string()))?;
            let fresh = match &self.config_store {
                Some(store) => store
                    .subnet_names_fresh(&self.globals.network)
                    .await
                    .unwrap_or(false),
                None => false,
            };
            let mut names_task = if fresh {
                None
            } else {
                let source = client.clone();
                Some(SubnetNamesTask(tokio::spawn(async move {
                    rtao::reads::generated::interfaces::subnet_names(&source).await
                })))
            };
            let result = work(client.clone()).await;
            if let Some(task) = names_task.as_mut() {
                if result.is_err() {
                    task.0.abort();
                    let _ = (&mut task.0).await;
                } else if let Ok(Ok(names)) = (&mut task.0).await {
                    self.output
                        .update_subnet_names(
                            names
                                .iter()
                                .map(|(netuid, name)| (netuid.get(), name.clone())),
                        )
                        .map_err(|e| CliError::failure(e.to_string()))?;
                    if let Some(store) = &self.config_store {
                        let _ = store
                            .save_subnet_names(&self.globals.network, names.into())
                            .await;
                    }
                }
            }
            result
        }
        .await;
        client.close().await;
        result
    }
}

impl AppContext {
    /// Confirm defaulted local or public target wallet selections before a
    /// command's own required-input round; return a pasted target when allowed.
    pub fn confirm_wallet(
        &mut self,
        options: &crate::prompt::WalletConfirmation,
        history: &mut crate::prompt::PromptHistory,
    ) -> Result<Option<AccountId>, CliError> {
        let lookup = crate::prompt::FsWalletPromptLookup::new(self.globals.wallet_path.clone())
            .map_err(|e| CliError::failure(e.to_string()))?;
        let mut selection = crate::prompt::WalletSelection {
            wallet_name: self.globals.wallet_name.clone(),
            hotkey_name: self.globals.hotkey_name.clone(),
            wallet_given: self.globals.wallet_given,
            hotkey_given: self.globals.hotkey_given,
            selected_accounts: Vec::new(),
        };
        let mut terminal = self.terminal.clone();
        let target = crate::prompt::confirm_wallet(
            &self.prompt_context(),
            &mut terminal,
            &mut selection,
            &self.books,
            &lookup,
            options,
            history,
        )
        .map_err(|e| CliError {
            code: e.exit_code(),
            message: e.to_string(),
            help: None,
        })?;
        self.globals.wallet_name = selection.wallet_name;
        self.globals.hotkey_name = selection.hotkey_name;
        self.globals.wallet_given = selection.wallet_given;
        self.globals.hotkey_given = selection.hotkey_given;
        for (address, name) in selection.selected_accounts {
            self.output
                .name_address(&address, &name)
                .map_err(|e| CliError::failure(e.to_string()))?;
        }
        target
            .map(|address| {
                address
                    .parse()
                    .map_err(|error| CliError::input(format!("invalid target account: {error}")))
            })
            .transpose()
    }
    /// Resolve an ordinary target flag and register its public display name.
    /// Only omitted canonical key fields trigger the default-wallet round.
    pub fn resolve_target(
        &mut self,
        param: &str,
        value: Option<&str>,
        history: &mut crate::prompt::PromptHistory,
    ) -> Result<Option<AccountId>, CliError> {
        let mut supplied = value.map(str::to_owned);
        if supplied.is_none() && ["coldkey_ss58", "hotkey_ss58"].contains(&param) {
            let options = crate::prompt::WalletConfirmation {
                help: format!(
                    "Wallet whose {} this command targets.",
                    if param == "hotkey_ss58" {
                        "hotkey"
                    } else {
                        "coldkey"
                    }
                ),
                require_coldkey: param == "coldkey_ss58",
                hotkey_help: (param == "hotkey_ss58")
                    .then(|| "Hotkey this command targets.".into()),
                accept_address: true,
                ..Default::default()
            };
            if let Some(target) = self.confirm_wallet(&options, history)? {
                supplied = Some(target.to_string());
            }
        }
        let lookup = FsAccountLookup::new(self.globals.wallet_path.clone())
            .map_err(|e| CliError::failure(e.to_string()))?;
        let resolver = NameResolver::new(
            &self.books,
            &lookup,
            &self.globals.wallet_name,
            &self.globals.hotkey_name,
        );
        let resolved = resolver
            .resolve_address(param, supplied.as_deref())
            .map_err(|e| CliError::failure(e.to_string()))?;
        let Some(resolved) = resolved else {
            return Ok(None);
        };
        if let Some(name) = resolved.name {
            self.output
                .name_address(&resolved.address, &name)
                .map_err(|e| CliError::failure(e.to_string()))?;
        }
        resolved.address.parse().map(Some).map_err(|e| {
            CliError::input(format!(
                "cannot resolve {}: {e}",
                crate::books::address_cli_name(param)
            ))
        })
    }
    /// Confirm the signing wallet then run a real intent through the uniform
    /// mutation path. Saved multisig routing is attached by its CLI adapter.
    pub async fn submit(
        &mut self,
        client: &Client,
        intent: &dyn DynIntent,
        wait_for_finalization: bool,
        replay: Option<&str>,
        external: Option<&dyn ExternalSignerFactory>,
        history: &mut crate::prompt::PromptHistory,
    ) -> Result<Option<rtao::tx::ExtrinsicResult>, CliError> {
        self.submit_with_options(
            client,
            intent,
            SubmissionOptions {
                period: None,
                execution: None,
                wait_for_finalization,
                replay,
                hooks: None,
                ..Default::default()
            },
            external,
            history,
        )
        .await
    }
    /// Resolve the command route and retain recovery hooks for all approvals.
    pub async fn submit_with_options(
        &mut self,
        client: &Client,
        intent: &dyn DynIntent,
        options: SubmissionOptions<'_>,
        external: Option<&dyn ExternalSignerFactory>,
        history: &mut crate::prompt::PromptHistory,
    ) -> Result<Option<rtao::tx::ExtrinsicResult>, CliError> {
        let native = crate::signer_backends::NativeSignerFactory::new(self);
        let external = Some(external.unwrap_or(&native));
        let role = intent
            .signer_role_dyn()
            .map_err(|e| CliError::input(e.to_string()))?;
        self.confirm_wallet(
            &crate::prompt::WalletConfirmation {
                help: if role == SignerRole::Hotkey {
                    "Wallet containing the signing hotkey."
                } else {
                    "Wallet whose coldkey signs this transaction."
                }
                .into(),
                require_coldkey: role == SignerRole::Coldkey,
                ..Default::default()
            },
            history,
        )?;
        if !self.globals.signatories.is_empty()
            && self.books.multisig(&self.globals.wallet_name).is_none()
        {
            return Err(CliError::input(format!(
                "--signatory given, but -w {:?} is not in this environment's multisig book",
                self.globals.wallet_name
            )));
        }
        if self.books.multisig(&self.globals.wallet_name).is_some()
            && ![
                "multisig_approve",
                "multisig_cancel",
                "multisig_execute",
                "multisig_threshold_1",
            ]
            .contains(&intent.spec().name)
        {
            let cache = match &self.config_store {
                Some(store) => crate::call::FsRawCallCache::new(store.paths()),
                None => crate::call::FsRawCallCache::from_process()?,
            };
            return self
                .submit_saved_with_options(
                    client,
                    intent,
                    options,
                    &ExternalRoundFactory::new(external),
                    &cache,
                )
                .await;
        }
        let keys = self.signing_keys(role, external).await?;
        let result = self
            .perform_submission(
                client,
                intent,
                keys.as_ref(),
                SubmissionControl {
                    mev_shield_default: options.mev_shield_default,
                    period: options.period,
                    execution: options.execution,
                    wait_for_finalization: options.wait_for_finalization,
                    replan_owner_rate_limit: options.replan_owner_rate_limit.unwrap_or(true),
                    bind_reviewed_call: options.bind_reviewed_call,
                    deadline: options.deadline,
                    replay: options.replay,
                    hooks: options.hooks,
                    factory: None,
                },
            )
            .await;
        let release = if self.globals.uses_external_signer()
            && let Some(factory) = external
        {
            factory
                .release(
                    &self.globals,
                    keys.as_ref(),
                    result.as_ref().is_ok_and(|receipt| {
                        receipt
                            .as_ref()
                            .is_some_and(|outcome| outcome.receipt.success)
                    }),
                )
                .await
        } else {
            Ok(())
        };
        // Release on refusal and preview too, preserving any submission error.
        let mut outcome = result?;
        if let Err(error) = release {
            match &mut outcome {
                Some(outcome) => outcome.release_error = Some(error),
                None => return Err(error),
            }
        }
        self.finish_submission(intent, outcome).await
    }
}

impl AppContext {
    /// Resolve an external signing account without device/browser interaction.
    /// Explicit address > selected member > persisted address > Vault companion.
    pub fn external_signer_address(&self) -> Result<Option<AccountId>, CliError> {
        if !self.globals.uses_external_signer() {
            return Ok(None);
        }
        let lookup = FsAccountLookup::new(self.globals.wallet_path.clone())
            .map_err(|e| CliError::failure(e.to_string()))?;
        let resolve = |value: &str| -> Option<AccountId> {
            if crate::books::is_bittensor_address(value) {
                return value.parse().ok();
            }
            if let Some(entry) = self.books.address_entry(value)
                && let Some(address) = entry.get("address").and_then(Value::as_str)
                && let Ok(account) = address.parse()
            {
                return Some(account);
            }
            crate::books::AccountLookup::coldkey(&lookup, value)
                .ok()
                .and_then(|address| address.parse().ok())
        };
        if let Some(explicit) = self
            .globals
            .signer_address
            .as_deref()
            .filter(|value| !value.is_empty())
        {
            return Ok(resolve(explicit));
        }
        if let Some(member) = self.globals.signatories.first()
            && let Some(address) = resolve(member)
        {
            return Ok(Some(address));
        }
        if let Some(configured) = self
            .config
            .get("signer_address")
            .filter(|value| json_truthy(value))
        {
            let spelling = crate::globals::config_string(configured);
            if let Some(address) = resolve(&spelling) {
                return Ok(Some(address));
            }
        }
        if self
            .globals
            .signer_backend
            .as_deref()
            .is_some_and(|backend| backend.trim().eq_ignore_ascii_case("vault"))
        {
            return Ok(
                crate::books::AccountLookup::coldkey(&lookup, &self.globals.wallet_name)
                    .ok()
                    .and_then(|address| address.parse().ok()),
            );
        }
        Ok(None)
    }
}

type SubnetNameMap = std::collections::BTreeMap<rtao::primitives::NetUid, String>;
struct SubnetNamesTask(tokio::task::JoinHandle<Result<SubnetNameMap, rtao::reads::ReadError>>);
impl Drop for SubnetNamesTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

struct PublicApprovalKeys(AccountId);
impl rtao::signer::KeyView for PublicApprovalKeys {
    fn coldkey_address(&self) -> Result<AccountId, rtao::signer::SignerError> {
        Ok(self.0)
    }
    fn hotkey_address(&self) -> Result<AccountId, rtao::signer::SignerError> {
        Ok(self.0)
    }
    fn hotkey_public_key(&self) -> Result<[u8; 32], rtao::signer::SignerError> {
        Ok(self.0.0)
    }
}
impl DynKeySource for PublicApprovalKeys {
    fn signer_for<'a>(
        &'a self,
        _: SignerRole,
    ) -> rtao::signer::BoxFuture<'a, Result<Arc<dyn rtao::signer::Signer>, rtao::signer::SignerError>>
    {
        Box::pin(async {
            Err(rtao::signer::SignerError::KeyView(
                "public approval view cannot sign".into(),
            ))
        })
    }
}
struct ExternalRoundFactory<'a> {
    external: Option<&'a dyn ExternalSignerFactory>,
    active: parking_lot::Mutex<std::collections::BTreeMap<(String, Option<AccountId>), Globals>>,
}
impl<'a> ExternalRoundFactory<'a> {
    fn new(external: Option<&'a dyn ExternalSignerFactory>) -> Self {
        Self {
            external,
            active: parking_lot::Mutex::new(std::collections::BTreeMap::new()),
        }
    }
}
fn member_globals(ctx: &AppContext, selection: &crate::call::SigningSelection) -> Globals {
    let mut globals = ctx.globals.clone();
    globals.wallet_name = selection.wallet.clone();
    globals.wallet_given = true;
    globals.signer_backend = Some(selection.backend.clone());
    globals.signer_address = selection.address.map(|account| account.to_string());
    globals
}
impl crate::call::RawSignerFactory for ExternalRoundFactory<'_> {
    fn source<'a>(
        &'a self,
        ctx: &'a AppContext,
        selection: &'a crate::call::SigningSelection,
    ) -> rtao::signer::BoxFuture<'a, Result<Arc<dyn DynKeySource>, CliError>> {
        Box::pin(async move {
            if selection.backend == "wallet" {
                return crate::call::RawSignerFactory::source(
                    &crate::call::WalletSignerFactory,
                    ctx,
                    selection,
                )
                .await;
            }
            let factory = self.external.ok_or_else(|| {
                CliError::failure(format!(
                    "{} signing requires its enabled CLI backend",
                    selection.backend
                ))
            })?;
            let globals = member_globals(ctx, selection);
            let keys = factory.keys(&globals, SignerRole::Coldkey).await?;
            self.active
                .lock()
                .insert((selection.backend.clone(), selection.address), globals);
            Ok(keys)
        })
    }
    fn release<'a>(
        &'a self,
        selection: &'a crate::call::SigningSelection,
        source: &'a dyn DynKeySource,
        success: bool,
    ) -> rtao::signer::BoxFuture<'a, Result<(), CliError>> {
        Box::pin(async move {
            let globals = self
                .active
                .lock()
                .remove(&(selection.backend.clone(), selection.address));
            if let (Some(factory), Some(globals)) = (self.external, globals) {
                factory.release(&globals, source, success).await?;
            }
            Ok(())
        })
    }
}
impl AppContext {
    /// Dispatch a semantic intent through the configured saved multisig.
    /// Public previews never acquire a signing source. After confirmation,
    /// rounds reuse one inner call and stop when it executes or fails.
    pub async fn submit_saved_with_factory(
        &self,
        client: &Client,
        intent: &dyn DynIntent,
        wait_for_finalization: bool,
        replay: Option<&str>,
        factory: &dyn crate::call::RawSignerFactory,
        cache: &dyn crate::call::RawCallCache,
    ) -> Result<Option<rtao::tx::ExtrinsicResult>, CliError> {
        self.submit_saved_with_options(
            client,
            intent,
            SubmissionOptions {
                period: None,
                execution: None,
                wait_for_finalization,
                replay,
                hooks: None,
                ..Default::default()
            },
            factory,
            cache,
        )
        .await
    }
    /// Dispatch saved approvals with recovery hooks on each actual round.
    pub async fn submit_saved_with_options(
        &self,
        client: &Client,
        intent: &dyn DynIntent,
        options: SubmissionOptions<'_>,
        factory: &dyn crate::call::RawSignerFactory,
        cache: &dyn crate::call::RawCallCache,
    ) -> Result<Option<rtao::tx::ExtrinsicResult>, CliError> {
        use crate::call::MultisigFollowup;
        let raw = crate::call::resolve_preset(self, &self.globals.wallet_name)?;
        let rounds = crate::call::plan_rounds(self, Some(&raw), SignerRole::Coldkey, None)?;
        let group = rtao::Executor::new(client.clone())
            .multisig(&raw.signatories, raw.threshold)
            .await
            .map_err(|error| CliError::failure(error.to_string()))?;
        if intent
            .signer_role_dyn()
            .map_err(|error| CliError::input(error.to_string()))?
            == SignerRole::Hotkey
        {
            self.output.message(&format!("note: {} runs with the multisig account as its hotkey — {} ({}) must be the registered hotkey on-chain",intent.spec().name,self.globals.wallet_name,group.address)).map_err(|error|CliError::failure(error.to_string()))?;
        }
        let mut inner: Option<rtao_runtime::CallBytes> = None;
        let mut opening: Option<rtao_runtime::models::MultisigTimepoint> = None;
        let mut last: Option<rtao::tx::ExtrinsicResult> = None;
        let attempt = async {
            for (index, selection) in rounds.iter().enumerate() {
                if let Some(receipt) = &last {
                    if crate::call::multisig_executed(receipt) || !receipt.success {
                        break;
                    }
                    crate::call::await_pending_visible(
                        client,
                        &group,
                        inner.as_ref().ok_or_else(|| {
                            CliError::failure("approval has no retained inner call")
                        })?,
                    )
                    .await?;
                }
                let member = selection
                    .address
                    .ok_or_else(|| CliError::input("multisig member has no public account"))?;
                let mut round = self.clone();
                round.globals = member_globals(self, selection);
                round.globals.assume_yes = self.globals.assume_yes || index > 0;
                // Scheme is public companion data. Non-file test/external sources
                // use the same sr25519 fee-probe default as the transaction layer.
                let crypto = if selection.backend == "wallet" {
                    Wallet::new(
                        &selection.wallet,
                        &self.globals.hotkey_name,
                        &self.globals.wallet_path,
                    )
                    .ok()
                    .and_then(|wallet| wallet.public_info(SignerRole::Coldkey).ok())
                    .map(|info| info.crypto_type)
                    .unwrap_or(rtao::primitives::CryptoType::Sr25519)
                } else {
                    rtao::primitives::CryptoType::Sr25519
                };
                let adapter = crate::multisig::SemanticMultisig::new(
                    intent,
                    client,
                    group.clone(),
                    member,
                    crypto,
                    inner.clone(),
                )
                .await
                .map_err(|error| CliError::failure(error.to_string()))?;
                let adapter = if index > 0 && raw.threshold >= 2 {
                    adapter.with_opening_timepoint(opening.clone().ok_or_else(|| {
                        CliError::failure(
                            "cannot continue approvals without a verified opening timepoint",
                        )
                    })?)
                } else {
                    adapter
                };
                let public = PublicApprovalKeys(member);
                let receipt = round
                    .perform_submission(
                        client,
                        &adapter,
                        &public,
                        SubmissionControl {
                            mev_shield_default: options.mev_shield_default,
                            period: options.period,
                            execution: None,
                            wait_for_finalization: options.wait_for_finalization,
                            // A failed inner call can consume the pending operation.
                            // Retrying this approval could open a new one (D-041).
                            replan_owner_rate_limit: false,
                            bind_reviewed_call: options.bind_reviewed_call,
                            deadline: options.deadline,
                            replay: options.replay,
                            hooks: options.hooks,
                            factory: Some(ApprovalFactory { factory, selection }),
                        },
                    )
                    .await?;
                let Some(outcome) = receipt else {
                    return Ok(None);
                };
                let mut receipt = outcome.receipt;
                // Retain every known on-chain result before any fallible local
                // post-processing, including an external signer's close failure.
                last = Some(receipt.clone());
                if let Some(error) = outcome.hook_error {
                    return Err(known_receipt_error(error, &receipt));
                }
                let actual = adapter
                    .reviewed_inner_call()
                    .map_err(|error| CliError::failure(error.to_string()))?
                    .ok_or_else(|| {
                        CliError::failure(
                            "approval completed without retaining its inner call bytes",
                        )
                    })?;
                if inner.is_none() {
                    inner = Some(actual.clone());
                }
                if receipt.success && raw.threshold >= 2 {
                    let spec = crate::call::decode_multisig_call_spec(
                        client,
                        &actual,
                        SignerRole::Coldkey,
                    )
                    .await;
                    if let Ok(followup) = crate::call::multisig_followup_for_spec(
                        &round,
                        client,
                        MultisigFollowup {
                            spec: &spec,
                            group: &raw,
                            multisig: &group,
                            call: &actual,
                            result: &receipt,
                        },
                        cache,
                    )
                    .await
                    {
                        receipt.data.insert("multisig_followup".into(), followup);
                    }
                }
                let executed = crate::call::multisig_executed(&receipt);
                let succeeded = receipt.success;
                // The enriched form is an optional update after the raw
                // result has already been persisted, even if closing failed.
                last = Some(receipt.clone());
                if let Some(hooks) = options.hooks {
                    hooks.after_followup(&receipt).await?;
                }
                if succeeded && let Some(error) = outcome.release_error {
                    last = Some(receipt);
                    return Err(error);
                }
                last = Some(receipt);
                if !succeeded {
                    break;
                }
                if index == 0 && index + 1 < rounds.len() && raw.threshold >= 2 && !executed {
                    opening = Some(crate::call::approval_timepoint(
                        last.as_ref()
                            .ok_or_else(|| CliError::failure("missing retained approval result"))?,
                        &group,
                        &actual,
                    )?);
                }
                if index + 1 < rounds.len() && !executed {
                    self.output
                        .message(&format!(
                            "approval {} of {} submitted",
                            index + 1,
                            raw.threshold
                        ))
                        .map_err(|error| CliError::failure(error.to_string()))?;
                }
                if executed {
                    break;
                }
            }
            Ok(last.take())
        }
        .await;
        let result = match attempt {
            Ok(Some(result)) => result,
            Ok(None) => return Ok(None),
            Err(mut error) => {
                if let Some(receipt) = &last {
                    let hash = inner
                        .as_ref()
                        .map(|call| format!("0x{}", hex::encode(call.hash())))
                        .or_else(|| {
                            receipt
                                .data
                                .get("multisig_call_hash")
                                .and_then(Value::as_str)
                                .map(str::to_owned)
                        });
                    let state = if crate::call::multisig_executed(receipt) {
                        "the call already executed; do not submit it again"
                    } else if receipt.success {
                        "the earlier approval(s) may remain pending; `rtao multisig pending` shows co-signer commands to finish or cancel"
                    } else {
                        "the last approval failed; inspect its on-chain result before retrying"
                    };
                    let recovery = match hash {
                        Some(hash) => format!("{state} (call hash {hash})"),
                        None => state.to_owned(),
                    };
                    error.help = Some(match error.help.take() {
                        Some(help) => format!("{help}; {recovery}"),
                        None => recovery,
                    });
                }
                return Err(error);
            }
        };
        if !self
            .output
            .result(&result, &format!("submitted {}", intent.spec().name))
            .map_err(|error| known_receipt_error(CliError::failure(error.to_string()), &result))?
        {
            return Err(CliError::failure(""));
        }
        Ok(Some(result))
    }
}
