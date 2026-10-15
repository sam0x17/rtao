// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/commands/sudo.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Additional source at the same commit/license: sdk/python/bittensor/cli/hyperparams_view.py.
//! Subnet-owner commands using the ordinary semantic submission boundary.

use super::generated::{self, QueryRequest, TxRequest};
use crate::{
    context::{AppContext, ExternalSignerFactory, SubmissionHooks, SubmissionOptions},
    error::CliError,
    globals::{Tier, with_globals},
    output::{Output, StepState, TableSpec},
    prompt::{PromptHistory, WalletConfirmation},
};
use clap::{Arg, ArgAction, ArgMatches, Command};
use rtao::{
    client::{BlockRef, Client, View, ViewExt},
    core::codec::Value as CoreValue,
    neuron::{TrimNullAction, TrimNullConfig, TrimNullSession, TrimNullStep},
    primitives::{NetUid, hyperparams::RawInteger, pyfmt},
    reads::ReadName,
    signer::BoxFuture,
    tx::{ExtrinsicResult, Plan},
};
use rtao_catalog::{hyperparams::TABLE, owner_map::OWNER_HYPERPARAMETERS};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

fn failure(error: impl std::fmt::Display) -> CliError {
    CliError::failure(error.to_string())
}

fn mutation(name: &str) -> Option<&'static str> {
    match name {
        "set" => Some("set_hyperparameter"),
        "start" => Some("start_call"),
        "set-symbol" => Some("update_symbol"),
        "set-identity" => Some("set_subnet_identity"),
        "trim" => Some("trim_subnet"),
        "trim-null-batch" => Some("trim_null_subnet_batch"),
        _ => None,
    }
}

fn wait_seconds(raw: &str) -> Result<String, String> {
    seconds(raw, 86_400.0)
}

fn poll_seconds(raw: &str) -> Result<String, String> {
    seconds(raw, 60.0)
}

fn seconds(raw: &str, maximum: f64) -> Result<String, String> {
    raw.parse::<f64>()
        .ok()
        .filter(|seconds| seconds.is_finite() && (1.0..=maximum).contains(seconds))
        .map(|_| raw.to_owned())
        .ok_or_else(|| format!("seconds must be finite and within 1..{maximum}"))
}

/// Bounded inputs for sequential, finalized Null pruning and optional activation.
pub struct NullPruningRequest {
    /// Non-root native subnet identity.
    pub netuid: NetUid,
    /// Explicit population ceiling, or the current target/Yuma ceiling.
    pub target: Option<u16>,
    /// Switch consensus only after finalized pruning and queue drainage.
    pub switch_to_yuma: bool,
    /// Maximum wait for each administration or switch prerequisite window.
    pub wait_timeout: Duration,
    /// Interval between live state checks while waiting.
    pub poll_interval: Duration,
    /// Maximum successful bounded batches before a resumable stop.
    pub max_batches: u32,
    /// Explicit recovery record, otherwise derived beside shared CLI config.
    pub state_file: Option<PathBuf>,
    /// Optional mortality override for every actual submitted transaction.
    pub period: Option<u32>,
}

/// Sequential Null pruning options, preserving Python's bounded CLI inputs.
pub fn trim_null_command() -> Command {
    with_globals(
        Command::new("trim-null")
            .about("Prune finalized Null batches, with an optional return to Yuma.")
            .long_about("Prune sequential finalized Null batches. Deregisters participants and can change survivor UIDs. Restart with the same target to resume from finalized storage. Each transaction uses the normal confirmation and signing flow; --yes skips prompts. --dry-run previews one transaction. --switch-to-yuma is opt-in.")
            .arg(Arg::new("netuid").long("netuid").required(true).value_parser(clap::value_parser!(u16).range(1..=4095)))
            .arg(Arg::new("target").long("target").value_parser(clap::value_parser!(u16).range(1..=65535)).help("Final population; defaults to the pending target or floor(256 / mechanism_count)."))
            .arg(Arg::new("switch_to_yuma").long("switch-to-yuma").action(ArgAction::SetTrue).help("Switch to Yuma after finalized pruning and prerequisite checks."))
            .arg(Arg::new("wait_timeout").long("wait-timeout").default_value("3600").value_parser(clap::builder::ValueParser::new(wait_seconds)).help("Maximum seconds for each administration or switch-prerequisite wait (1..86400)."))
            .arg(Arg::new("poll_seconds").long("poll-seconds").default_value("12").value_parser(clap::builder::ValueParser::new(poll_seconds)).help("Seconds between live state polls (1..60)."))
            .arg(Arg::new("max_batches").long("max-batches").default_value("256").value_parser(clap::value_parser!(u32).range(1..=4096)).help("Stop after this many batches; rerun to resume."))
            .arg(Arg::new("state_file").long("state-file").value_parser(clap::value_parser!(PathBuf)).help("Durable unresolved-submission record; defaults beside CLI config, keyed by chain and subnet."))
            .arg(Arg::new("__period").long("period").value_parser(clap::value_parser!(u32)).help("Mortal transaction period in blocks (rtao addition).")),
        Tier::Tx,
    )
}

impl NullPruningRequest {
    /// Confirm the owner wallet, then retain exact validated workflow controls.
    pub fn from_matches(
        ctx: &mut AppContext,
        matches: &ArgMatches,
        history: &mut PromptHistory,
    ) -> Result<Self, CliError> {
        ctx.confirm_wallet(
            &WalletConfirmation {
                help: "Subnet owner coldkey signing every batch.".into(),
                require_coldkey: true,
                ..Default::default()
            },
            history,
        )?;
        let duration = |key: &str| -> Result<Duration, CliError> {
            let raw = matches
                .get_one::<String>(key)
                .ok_or_else(|| CliError::input(format!("missing {key}")))?;
            Duration::try_from_secs_f64(raw.parse::<f64>().map_err(failure)?).map_err(failure)
        };
        Ok(Self {
            netuid: NetUid(
                *matches
                    .get_one::<u16>("netuid")
                    .ok_or_else(|| CliError::input("missing --netuid"))?,
            ),
            target: matches.get_one::<u16>("target").copied(),
            switch_to_yuma: matches.get_flag("switch_to_yuma"),
            wait_timeout: duration("wait_timeout")?,
            poll_interval: duration("poll_seconds")?,
            max_batches: *matches
                .get_one::<u32>("max_batches")
                .ok_or_else(|| CliError::input("missing --max-batches"))?,
            state_file: matches.get_one::<PathBuf>("state_file").cloned(),
            period: matches.get_one::<u32>("__period").copied(),
        })
    }
}

fn pruning_recovery_path(
    ctx: &AppContext,
    client: &Client,
    request: &NullPruningRequest,
) -> Result<PathBuf, CliError> {
    if let Some(path) = &request.state_file {
        return Ok(path.clone());
    }
    let process_paths;
    let paths = if let Some(store) = &ctx.config_store {
        store.paths()
    } else {
        process_paths = rtao::client::config::Paths::from_process().map_err(failure)?;
        &process_paths
    };
    let config = paths.path(rtao::client::config::ConfigFile::Config);
    let parent = config
        .parent()
        .ok_or_else(|| failure("CLI config has no parent"))?;
    let genesis = format!("0x{}", hex::encode(client.genesis().0));
    let identity = Sha256::digest(format!("{genesis}:{}", request.netuid).as_bytes());
    Ok(parent
        .join("null-pruning")
        .join(format!("{}.json", &hex::encode(identity)[..20])))
}

struct PruningHooks<'a> {
    session: &'a TrimNullSession,
    action: &'a TrimNullAction,
    forwarded: Option<&'a dyn SubmissionHooks>,
    receipt: parking_lot::Mutex<Option<ExtrinsicResult>>,
    failed: AtomicBool,
}

impl SubmissionHooks for PruningHooks<'_> {
    fn before_submit(&self) -> BoxFuture<'_, Result<(), CliError>> {
        Box::pin(async {
            Err(failure(
                "Null pruning requires the reviewed outer plan before signing",
            ))
        })
    }

    fn before_submit_with_plan<'a>(
        &'a self,
        plan: &'a Plan,
    ) -> BoxFuture<'a, Result<(), CliError>> {
        Box::pin(async move {
            self.session
                .before_attempt(self.action, plan)
                .map_err(failure)?;
            if let Some(hooks) = self.forwarded {
                hooks.before_submit_with_plan(plan).await?;
            }
            Ok(())
        })
    }

    fn on_result<'a>(
        &'a self,
        receipt: &'a ExtrinsicResult,
    ) -> BoxFuture<'a, Result<(), CliError>> {
        Box::pin(async move {
            *self.receipt.lock() = Some(receipt.clone());
            let result = async {
                self.session.record_result(receipt).map_err(failure)?;
                if let Some(hooks) = self.forwarded {
                    hooks.on_result(receipt).await?;
                }
                Ok(())
            }
            .await;
            if result.is_err() {
                self.failed.store(true, Ordering::SeqCst);
            }
            result
        })
    }

    fn after_followup<'a>(
        &'a self,
        receipt: &'a ExtrinsicResult,
    ) -> BoxFuture<'a, Result<(), CliError>> {
        Box::pin(async move {
            *self.receipt.lock() = Some(receipt.clone());
            let result = async {
                self.session
                    .record_reviewed_result(self.action, receipt)
                    .await
                    .map_err(failure)?;
                if let Some(hooks) = self.forwarded {
                    hooks.after_followup(receipt).await?;
                }
                Ok(())
            }
            .await;
            if result.is_err() {
                self.failed.store(true, Ordering::SeqCst);
            }
            result
        })
    }
}

fn retain_recovery(mut error: CliError, recovery: &Path) -> CliError {
    if recovery.exists() {
        let message = format!(
            "retain {}; reconcile its recorded inclusion, expiry or pending multisig approval before starting a different operation",
            recovery.display()
        );
        error.help = Some(match error.help.take() {
            Some(help) => format!("{help}; {message}"),
            None => message,
        });
    }
    error
}

async fn execute_null_pruning(
    ctx: &mut AppContext,
    client: &Client,
    request: &NullPruningRequest,
    options: SubmissionOptions<'_>,
    external: Option<&dyn ExternalSignerFactory>,
    history: &mut PromptHistory,
) -> Result<Option<ExtrinsicResult>, CliError> {
    // Carrier finality alone does not prove the inner pruning operation ran.
    // Refuse that route before any durable attempt or signing acquisition.
    if ctx.resolve_mev_shield(false, false, "trim-null")?.enabled {
        return Err(CliError::input(
            "sequential Null pruning requires direct finalized receipts; pass --no-mev-shield",
        ));
    }
    let owner = match ctx.dispatch_proxy(None)? {
        Some(owner) => owner,
        None => {
            let owner = if ctx.globals.uses_external_signer()
                && ctx.books.multisig(&ctx.globals.wallet_name).is_none()
            {
                ctx.external_signer_address()?
            } else {
                ctx.resolve_address("coldkey_ss58", None)?
            };
            owner.ok_or_else(|| {
                CliError::input("Null pruning requires the public subnet owner coldkey")
            })?
        }
    };
    let recovery = pruning_recovery_path(ctx, client, request)?;
    let mut session = TrimNullSession::begin(
        client.clone(),
        owner,
        request.netuid,
        request.target,
        &recovery,
        TrimNullConfig {
            preview: ctx.globals.dry_run,
            switch_to_yuma: request.switch_to_yuma,
            max_batches: request.max_batches,
            poll_interval: request.poll_interval,
            wait_timeout: request.wait_timeout,
        },
    )
    .await
    .map_err(failure)?;
    ctx.output.message(&format!("Pruning deregisters participants and may change survivor UIDs. Target {}. Each transaction uses the normal confirmation and signing flow.", session.report().target)).map_err(failure)?;
    let saved_multisig = ctx.books.multisig(&ctx.globals.wallet_name).is_some();
    let mut last = None;
    loop {
        match session.next_action().await.map_err(failure)? {
            TrimNullStep::Complete(report) => {
                let payload = if report.already_yuma {
                    json!({"status":"already Yuma", "population": report.population})
                } else {
                    json!({"status":"complete", "population":report.population, "target":report.target,
                        "consensus":if report.switched_to_yuma {"Yuma"} else {"Null"}, "batches":report.batches})
                };
                let fields = payload
                    .as_object()
                    .ok_or_else(|| failure("invalid pruning summary"))?
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect::<Vec<_>>();
                ctx.output
                    .detail(Some("Null pruning"), &fields, Some(&payload))
                    .map_err(failure)?;
                return Ok(last);
            }
            TrimNullStep::Wait { reason, delay } => {
                ctx.output
                    .step("waiting", &reason, StepState::Fact)
                    .map_err(failure)?;
                session.wait(&reason, delay).await.map_err(failure)?;
            }
            TrimNullStep::Submit(action) => {
                let hooks = PruningHooks {
                    session: &session,
                    action: &action,
                    forwarded: options.hooks,
                    receipt: parking_lot::Mutex::new(None),
                    failed: AtomicBool::new(false),
                };
                let result = ctx
                    .submit_with_options(
                        client,
                        action.intent(),
                        SubmissionOptions {
                            bind_reviewed_call: true,
                            period: request.period.or(options.period),
                            wait_for_finalization: true,
                            replan_owner_rate_limit: Some(false),
                            // The executor owns this deadline and drains attempted delivery.
                            // Do not wrap submission in a timeout that drops its future.
                            deadline: options.deadline.or(Some(request.wait_timeout)),
                            hooks: Some(&hooks),
                            ..options
                        },
                        external,
                        history,
                    )
                    .await;
                let captured = hooks.receipt.lock().take();
                let hook_failed = hooks.failed.load(Ordering::SeqCst);
                drop(hooks);
                let (receipt, submission_error) = match result {
                    Ok(None) => return Ok(None), // A preview never begins an attempt.
                    Ok(Some(receipt)) => (receipt, None),
                    Err(error) if !hook_failed && captured.is_some() => (
                        captured.ok_or_else(|| failure("missing captured pruning receipt"))?,
                        Some(error),
                    ),
                    Err(error) => return Err(retain_recovery(error, &recovery)),
                };
                let retry_window = !saved_multisig
                    && !receipt.success
                    && receipt
                        .error
                        .as_ref()
                        .and_then(|error| error.name.as_deref())
                        == Some("AdminActionProhibitedDuringWeightsWindow");
                let before_batches = session.report().batches;
                let before_removed = session.report().removed_uids;
                let before_cleared = session.report().cleared_commits;
                let finished = session.finish_action(action, &receipt).await;
                if let Err(error) = finished {
                    let mut error = failure(error);
                    if let Some(submission_error) =
                        submission_error.filter(|error| !error.message.is_empty())
                    {
                        error.help = Some(format!("submission also reported: {submission_error}"));
                    }
                    return Err(retain_recovery(error, &recovery));
                }
                if let Some(error) = submission_error
                    && !retry_window
                {
                    return Err(retain_recovery(error, &recovery));
                }
                if session.report().batches != before_batches {
                    let report = session.report();
                    let removed = report
                        .removed_uids
                        .checked_sub(before_removed)
                        .ok_or_else(|| failure("pruning removal counter moved backwards"))?;
                    let cleared = report
                        .cleared_commits
                        .checked_sub(before_cleared)
                        .ok_or_else(|| failure("pruning cleanup counter moved backwards"))?;
                    let payload = json!({"status":"finalized", "batch":report.batches, "population":report.population,
                        "target":report.target, "removed_uids":removed, "cleared_commits":cleared, "extrinsic":receipt.extrinsic_id});
                    let fields = payload
                        .as_object()
                        .ok_or_else(|| failure("invalid pruning progress"))?
                        .iter()
                        .map(|(key, value)| (key.clone(), value.clone()))
                        .collect::<Vec<_>>();
                    ctx.output
                        .detail(Some("Pruning progress"), &fields, Some(&payload))
                        .map_err(failure)?;
                }
                last = Some(receipt);
            }
        }
    }
}

/// Owner commands and the shared durable sequential Null-pruning workflow.
pub fn command() -> Result<Command, CliError> {
    let mut group = Command::new("hparams")
        .about("Subnet-owner config and governance.")
        .subcommand_required(true)
        .arg_required_else_help(true);
    for name in [
        "set",
        "start",
        "set-symbol",
        "set-identity",
        "trim",
        "trim-null-batch",
    ] {
        let op = mutation(name).ok_or_else(|| failure("missing owner operation"))?;
        let mut child = generated::intent_command(op)?.name(name);
        if name == "set-identity" {
            child = child
                .mut_arg("subnet_name", |arg| arg.long("name").alias("subnet-name"))
                .mut_arg("subnet_url", |arg| arg.long("url").alias("subnet-url"));
        }
        group = group.subcommand(child);
    }
    let get =
        generated::read_command(ReadName::SubnetHyperparameters)?
            .name("get")
            .arg(Arg::new("name").long("name").help(
                "Explain one hyperparameter, its raw value, and the command that changes it.",
            ));
    let identity = generated::read_command(ReadName::SubnetIdentity)?
        .name("get-identity")
        .mut_arg("netuid", |arg| {
            arg.long(clap::builder::Resettable::<clap::builder::Str>::Reset)
                .index(1)
                .required(true)
        });
    let count = generated::intent_command("set_mechanism_count")?
        .name("count")
        .about("Get mechanism count, or set it with --value (subnet owner only).")
        .mut_arg("mechanism_count", |arg| {
            arg.long("value")
                .help("New mechanism count; omit for a read-only query of the current count.")
        });
    let emissions = generated::read_command(ReadName::MechanismEmissionSplit)?.name("emissions");
    Ok(group
        .subcommand(trim_null_command())
        .subcommand(get)
        .subcommand(generated::read_command(ReadName::DelegateTake)?.name("get-take"))
        .subcommand(generated::read_command(ReadName::SubnetStartSchedule)?.name("check-start"))
        .subcommand(identity)
        .subcommand(
            Command::new("mechanisms")
                .alias("mech")
                .about("Subnet mechanism configuration.")
                .subcommand_required(true)
                .arg_required_else_help(true)
                .subcommand(count)
                .subcommand(emissions),
        ))
}

fn leaf(matches: &ArgMatches) -> Result<(&str, &ArgMatches), CliError> {
    let (name, matches) = matches
        .subcommand()
        .ok_or_else(|| CliError::input("missing hparams command"))?;
    if name == "mechanisms" {
        matches
            .subcommand()
            .ok_or_else(|| CliError::input("missing mechanisms command"))
    } else {
        Ok((name, matches))
    }
}

/// Command-owned identifiers to reserve from leaf-global overrides.
pub fn reserved(matches: &ArgMatches) -> Result<Vec<&'static str>, CliError> {
    let (name, _) = leaf(matches)?;
    if let Some(op) = mutation(name) {
        return generated::intent_reserved(op);
    }
    let mut fields = match name {
        "trim-null" => vec![
            "netuid",
            "target",
            "switch_to_yuma",
            "wait_timeout",
            "poll_seconds",
            "max_batches",
            "state_file",
            "__period",
        ],
        "count" => generated::intent_reserved("set_mechanism_count")?,
        "get" => generated::read_reserved(ReadName::SubnetHyperparameters),
        "get-take" => generated::read_reserved(ReadName::DelegateTake),
        "check-start" => generated::read_reserved(ReadName::SubnetStartSchedule),
        "get-identity" => generated::read_reserved(ReadName::SubnetIdentity),
        "emissions" => generated::read_reserved(ReadName::MechanismEmissionSplit),
        _ => return Err(CliError::input(format!("unknown hparams command {name:?}"))),
    };
    if name == "get" {
        fields.push("name");
    }
    Ok(fields)
}

/// Source-shaped request, prepared before connecting in noninteractive mode.
pub enum Request {
    /// A real SDK intent, retaining all normal review and signing controls.
    Transaction(TxRequest),
    /// Raw hyperparameters plus the owner-settable supplemental storage values.
    Hyperparameters {
        /// Catalog read and optional historical block.
        query: QueryRequest,
        /// Optional parameter explanation.
        name: Option<String>,
    },
    /// A curated read with its original command-specific machine payload.
    Read(QueryRequest),
    /// Sequential finalized pruning through the SDK's shared durable session.
    NullPruning(NullPruningRequest),
}

impl Request {
    /// Parse the selected owner command without acquiring signing keys.
    pub fn from_matches(
        ctx: &mut AppContext,
        matches: &ArgMatches,
        history: &mut PromptHistory,
    ) -> Result<Self, CliError> {
        let (name, matches) = leaf(matches)?;
        if name == "trim-null" {
            return NullPruningRequest::from_matches(ctx, matches, history).map(Self::NullPruning);
        }
        if let Some(op) = mutation(name) {
            return TxRequest::from_matches(ctx, op, matches, history).map(Self::Transaction);
        }
        if name == "count" && matches.get_one::<String>("mechanism_count").is_some() {
            return TxRequest::from_matches(ctx, "set_mechanism_count", matches, history)
                .map(Self::Transaction);
        }
        let read = match name {
            "get" => ReadName::SubnetHyperparameters,
            "get-take" => ReadName::DelegateTake,
            "check-start" => ReadName::SubnetStartSchedule,
            "get-identity" => ReadName::SubnetIdentity,
            "count" => ReadName::MechanismCount,
            "emissions" => ReadName::MechanismEmissionSplit,
            _ => return Err(CliError::input(format!("unknown hparams command {name:?}"))),
        };
        let query = QueryRequest::from_matches(ctx, read, matches, history)?;
        Ok(if name == "get" {
            Self::Hyperparameters {
                query,
                name: matches.get_one::<String>("name").cloned(),
            }
        } else {
            Self::Read(query)
        })
    }

    /// Show current values before prompting for an omitted parameter name.
    pub async fn from_matches_with_client(
        ctx: &mut AppContext,
        client: &Client,
        matches: &ArgMatches,
        history: &mut PromptHistory,
    ) -> Result<Self, CliError> {
        let (name, args) = leaf(matches)?;
        if name == "set"
            && args.get_one::<String>("name").is_none()
            && ctx.prompt_context().interactive(&ctx.terminal)
        {
            ctx.confirm_wallet(
                &WalletConfirmation {
                    help: "Wallet whose coldkey signs this transaction.".into(),
                    require_coldkey: true,
                    ..Default::default()
                },
                history,
            )?;
            if let Some(netuid) = args.get_one::<String>("netuid") {
                let netuid = netuid.parse::<u16>().map(NetUid).map_err(failure)?;
                let params = fetch_hyperparameters(client, netuid).await?;
                render_hyperparameters(&ctx.output, netuid, &params, None)?;
            }
        }
        Self::from_matches(ctx, matches, history)
    }
}

fn netuid(query: &QueryRequest) -> Result<NetUid, CliError> {
    serde_json::from_value(query.params.get("netuid").cloned().unwrap_or(Value::Null))
        .map_err(|error| CliError::input(error.to_string()))
}

/// Extend the runtime hyperparameter record at one pinned block, as Python does.
pub async fn fetch_hyperparameters<V: View + ?Sized>(
    view: &V,
    netuid: NetUid,
) -> Result<Option<Map<String, Value>>, CliError> {
    let snapshot = view.pin().await.map_err(failure)?;
    let Some(params) = rtao::reads::reads::owner::subnet_hyperparameters(&snapshot, netuid)
        .await
        .map_err(failure)?
    else {
        return Ok(None);
    };
    let mut params: Map<String, Value> = params.into_iter().collect();
    // The extracted storage table is the same source allowlist used by Python's
    // view. Do not substitute the setter's paired/newtype storage representation.
    for row in OWNER_HYPERPARAMETERS {
        if params.contains_key(row.name) {
            continue;
        }
        let Some(item) = TABLE.storage.iter().find(|item| item.parameter == row.name) else {
            continue;
        };
        let mut value = snapshot
            .storage_dyn(
                item.container,
                item.name,
                &[CoreValue::Uint(u128::from(netuid.0))],
            )
            .await
            .map_err(failure)?
            .to_json();
        if value.as_object().is_some_and(|fields| fields.len() == 1)
            && let Some(bits) = value.get("bits")
        {
            value = bits.clone();
        }
        if !value.is_null() {
            params.insert(row.name.into(), value);
        }
    }
    Ok(Some(params))
}

fn integer(value: &Value) -> Option<RawInteger> {
    let Value::Number(value) = value else {
        return None;
    };
    value
        .as_i128()
        .map(RawInteger::Signed)
        .or_else(|| value.as_u128().map(RawInteger::Unsigned))
}

fn text(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        _ => value.to_string(),
    }
}

/// Render raw machine values independently from human annotations and hints.
pub fn render_hyperparameters(
    output: &Output,
    netuid: NetUid,
    params: &Option<Map<String, Value>>,
    name: Option<&str>,
) -> Result<(), CliError> {
    let params = params
        .as_ref()
        .ok_or_else(|| failure(format!("subnet {netuid} does not exist")))?;
    if let Some(name) = name {
        let raw = params.get(name).ok_or_else(|| {
            CliError::input(format!(
                "no hyperparameter named {name:?} on netuid {netuid}"
            ))
            .with_help("run without --name to list them all")
        })?;
        let normalized = integer(raw).and_then(|raw| TABLE.normalized(name, raw));
        let annotation = integer(raw).and_then(|raw| TABLE.annotate(name, raw));
        let settable = OWNER_HYPERPARAMETERS.iter().any(|row| row.name == name);
        let payload = json!({
            "netuid": netuid,
            "name": name,
            "value": raw,
            "normalized": normalized,
            "kind": TABLE.kind_of(name),
            "doc": TABLE.doc_of(name),
            "owner_settable": settable,
        });
        let mut fields = vec![("value".into(), raw.clone())];
        if let Some(fraction) = normalized {
            fields.push(("normalized".into(), json!(pyfmt::fmt_g(fraction))));
        } else if let Some(reading) = annotation {
            fields.push((
                "reading".into(),
                json!(reading.trim_start_matches(['=', '≈', ' '])),
            ));
        }
        if let Some(doc) = TABLE.doc_of(name) {
            fields.push(("description".into(), json!(doc)));
        }
        fields.push((
            "note".into(),
            json!(if settable {
                format!("--value takes {}", TABLE.value_forms(name))
            } else {
                "not settable by the subnet owner (root/governance only)".into()
            }),
        ));
        if settable {
            // A raw integer is always an exact, accepted copy/paste example.
            let example = if name == "epoch_consensus" {
                if integer(raw).and_then(RawInteger::as_u128) == Some(1) {
                    "Null".into()
                } else {
                    "Yuma".into()
                }
            } else {
                text(raw)
            };
            fields.push((
                "set it".into(),
                json!(format!(
                    "rtao hparams set --netuid {netuid} --name {name} --value {example}"
                )),
            ));
        }
        fields.push((
            "see".into(),
            json!(format!(
                "{}/hyperparameters/{}",
                rtao_catalog::py::DOCS_URL,
                name.replace('_', "-")
            )),
        ));
        return output
            .detail(
                Some(&format!("{name}  netuid {netuid}")),
                &fields,
                Some(&payload),
            )
            .map_err(failure);
    }
    let rows = params
        .iter()
        .map(|(name, raw)| {
            vec![
                json!(name),
                json!(text(raw)),
                json!(
                    integer(raw)
                        .and_then(|raw| TABLE.annotate(name, raw))
                        .unwrap_or_default()
                ),
                json!(TABLE.short_of(name).unwrap_or("")),
            ]
        })
        .collect();
    let mut table = TableSpec::new(
        format!("hyperparameters netuid {netuid}"),
        vec![
            "parameter".into(),
            "value".into(),
            "reading".into(),
            "description".into(),
        ],
        rows,
    );
    table.records = Some(Value::Object(params.clone()));
    table.footer = Some(format!(
        "--name <parameter> explains one parameter and how to set it. {}/hyperparameters",
        rtao_catalog::py::DOCS_URL
    ));
    output.table(&table).map_err(failure)
}

fn render_read(output: &Output, query: &QueryRequest, payload: Value) -> Result<(), CliError> {
    match query.name {
        ReadName::MechanismCount => output.detail(
            None,
            &[
                ("netuid".into(), json!(netuid(query)?)),
                ("mechanism_count".into(), payload),
            ],
            None,
        ),
        ReadName::MechanismEmissionSplit => output.detail(
            None,
            &[
                ("netuid".into(), json!(netuid(query)?)),
                ("split".into(), payload),
            ],
            None,
        ),
        ReadName::DelegateTake => {
            let number = |key: &str| {
                payload
                    .get(key)
                    .and_then(Value::as_f64)
                    .ok_or_else(|| failure(format!("delegate_take lacks {key}")))
            };
            let fields = [
                ("hotkey".into(), payload["hotkey"].clone()),
                (
                    "take".into(),
                    json!(format!(
                        "{:.2}%  ({} as u16)",
                        number("take")? * 100.0,
                        payload["take_u16"]
                    )),
                ),
                (
                    "allowed".into(),
                    json!(format!(
                        "{:.2}% – {:.2}%",
                        number("min")? * 100.0,
                        number("max")? * 100.0
                    )),
                ),
            ];
            output.detail(None, &fields, Some(&payload))
        }
        ReadName::SubnetStartSchedule | ReadName::SubnetIdentity => {
            let title = if query.name == ReadName::SubnetIdentity {
                format!("subnet {} identity", netuid(query)?)
            } else {
                format!("start schedule netuid {}", netuid(query)?)
            };
            let fields = payload
                .as_object()
                .map(|fields| {
                    fields
                        .iter()
                        .map(|(key, value)| (key.clone(), value.clone()))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            output.detail(Some(&title), &fields, Some(&payload))
        }
        _ => return Err(failure("unrecognized curated owner read")),
    }
    .map_err(failure)
}

/// Execute a request through typed reads or the uniform semantic submit route.
pub async fn execute(
    ctx: &mut AppContext,
    client: &Client,
    request: &Request,
    options: SubmissionOptions<'_>,
    external: Option<&dyn ExternalSignerFactory>,
    history: &mut PromptHistory,
) -> Result<Option<ExtrinsicResult>, CliError> {
    match request {
        Request::NullPruning(request) => {
            execute_null_pruning(ctx, client, request, options, external, history).await
        }
        Request::Transaction(request) => {
            generated::execute_tx(ctx, client, request, options, external, history).await
        }
        Request::Hyperparameters { query, name } => {
            let snapshot = client
                .at(query.block.map_or(BlockRef::Best, BlockRef::Number))
                .await
                .map_err(failure)?;
            let netuid = netuid(query)?;
            let params = fetch_hyperparameters(&snapshot, netuid).await?;
            render_hyperparameters(&ctx.output, netuid, &params, name.as_deref())?;
            Ok(None)
        }
        Request::Read(query) => {
            let snapshot = client
                .at(query.block.map_or(BlockRef::Best, BlockRef::Number))
                .await
                .map_err(failure)?;
            let payload = rtao::reads::dispatch(&snapshot, query.name, &query.params)
                .await
                .map_err(failure)?;
            render_read(&ctx.output, query, payload)?;
            Ok(None)
        }
    }
}
