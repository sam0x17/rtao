// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/commands/subnets.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Additional source at the same commit/license: sdk/python/bittensor/cli/metagraph_view.py.
//! Registration and raw metagraph inspection through real SDK operations.

use super::generated::{self, QueryRequest, TxRequest};
use crate::{
    books::{AccountLookup, FsAccountLookup},
    context::{AppContext, ExternalSignerFactory, SubmissionOptions},
    error::CliError,
    output::{Output, TableSpec},
    prompt::PromptHistory,
};
use clap::{Arg, ArgAction, ArgMatches, Command};
use rtao::{
    client::{BlockRef, Client},
    primitives::{AccountId, NetUid, hyperparams::RawInteger, ratio::integer_division},
    reads::ReadName,
    tx::ExtrinsicResult,
};
use rtao_intents::pow::{CancellationToken, PowConfig, PowSolver};
use serde_json::{Map, Value, json};
use std::{
    collections::BTreeMap,
    net::{Ipv4Addr, Ipv6Addr},
    time::Duration,
};

fn failure(error: impl std::fmt::Display) -> CliError {
    CliError::failure(error.to_string())
}

fn timeout(raw: &str) -> Result<String, String> {
    raw.parse::<f64>()
        .ok()
        .filter(|seconds| seconds.is_finite() && (1.0..=3600.0).contains(seconds))
        .map(|_| raw.to_owned())
        .ok_or_else(|| "PoW timeout must be finite and within 1..3600 seconds".into())
}

/// Subnet creation, burn/root/CPU registration and full metagraph inspection.
pub fn command() -> Result<Command, CliError> {
    let register = generated::intent_command("burned_register")?
        .name("register")
        .about("Register a hotkey by paying TAO, or mine an owner-enabled proof with --pow.")
        .arg(
            Arg::new("pow")
                .long("pow")
                .action(ArgAction::SetTrue)
                .help("Mine a public registration proof instead of paying TAO (non-root subnets)."),
        )
        .arg(
            Arg::new("pow_workers")
                .long("pow-workers")
                .default_value("4")
                .value_parser(clap::value_parser!(u8).range(1..=32))
                .help("CPU mining workers for --pow."),
        )
        .arg(
            Arg::new("pow_backend")
                .long("pow-backend")
                .default_value("auto")
                .value_parser(["auto", "cpu", "gpu"])
                .help("auto/cpu use the native CPU solver; gpu is unavailable in this build."),
        )
        .arg(
            Arg::new("pow_device")
                .long("pow-device")
                .action(ArgAction::Append)
                .value_parser(clap::value_parser!(u32))
                .help("GPU device id (GPU execution is unavailable in this build)."),
        )
        .arg(
            Arg::new("pow_timeout")
                .long("pow-timeout")
                .default_value("300")
                .value_parser(clap::builder::ValueParser::new(timeout))
                .help("Maximum public mining time in seconds (1..3600)."),
        );
    let graph = generated::read_command(ReadName::Metagraph)?
        .name("metagraph")
        .mut_arg("netuid", |arg| {
            arg.long(clap::builder::Resettable::<clap::builder::Str>::Reset)
                .index(1)
                .required(true)
        });
    Ok(Command::new("subnets")
        .about("Register and inspect subnets.")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(register)
        .subcommand(generated::intent_command("register_subnet")?.name("create"))
        .subcommand(graph))
}

/// Command-owned identifiers to reserve from leaf-global overrides.
pub fn reserved(matches: &ArgMatches) -> Result<Vec<&'static str>, CliError> {
    let (name, _) = matches
        .subcommand()
        .ok_or_else(|| CliError::input("missing subnets command"))?;
    match name {
        "create" => generated::intent_reserved("register_subnet"),
        "register" => {
            let mut fields = generated::intent_reserved("burned_register")?;
            fields.extend([
                "pow",
                "pow_workers",
                "pow_backend",
                "pow_device",
                "pow_timeout",
            ]);
            Ok(fields)
        }
        "metagraph" => Ok(generated::read_reserved(ReadName::Metagraph)),
        _ => Err(CliError::input(format!("unknown subnets command {name:?}"))),
    }
}

/// Validated public inputs for an initial proof search, without a signing key.
pub struct PowRegistration {
    /// Subnet receiving this registration.
    pub netuid: NetUid,
    /// Explicit or name-resolved receiving hotkey.
    pub hotkey: AccountId,
    /// Validated bounded CPU worker and timeout settings.
    pub config: PowConfig,
    /// Requested mortal transaction period.
    pub period: Option<u32>,
}

/// Source-shaped registration or metagraph request.
pub enum Request {
    /// A ready SDK transaction retaining its semantic submit controls.
    Transaction(TxRequest),
    /// Public mining happens only on an actual execution.
    Pow(PowRegistration),
    /// Raw catalog metagraph read, optionally at a historical height.
    Metagraph(QueryRequest),
}

impl Request {
    /// Parse inputs and resolve the target hotkey before chain connection.
    pub fn from_matches(
        ctx: &mut AppContext,
        matches: &ArgMatches,
        history: &mut PromptHistory,
    ) -> Result<Self, CliError> {
        let (name, matches) = matches
            .subcommand()
            .ok_or_else(|| CliError::input("missing subnets command"))?;
        match name {
            "create" => TxRequest::from_matches(ctx, "register_subnet", matches, history)
                .map(Self::Transaction),
            "metagraph" => QueryRequest::from_matches(ctx, ReadName::Metagraph, matches, history)
                .map(Self::Metagraph),
            "register" => {
                let request = TxRequest::from_matches(ctx, "burned_register", matches, history)?;
                let netuid: NetUid =
                    serde_json::from_value(request.args["netuid"].clone()).map_err(failure)?;
                if matches.get_flag("pow") {
                    if netuid == NetUid::ROOT {
                        return Err(CliError::input("PoW registration is unavailable on root"));
                    }
                    if matches
                        .get_one::<String>("pow_backend")
                        .is_some_and(|backend| backend == "gpu")
                        || matches.get_many::<u32>("pow_device").is_some()
                    {
                        return Err(CliError::input(
                            "GPU PoW registration is unavailable in this build; use --pow-backend cpu without --pow-device",
                        ));
                    }
                    if ctx.globals.dry_run {
                        return Err(CliError::input(
                            "--pow --dry-run does not start a proof search; preview an existing proof with rtao tx pow-register",
                        ));
                    }
                    let hotkey = request
                        .args
                        .get("hotkey_ss58")
                        .and_then(Value::as_str)
                        .ok_or_else(|| {
                            CliError::input("PoW registration requires a resolved hotkey")
                        })?
                        .parse()
                        .map_err(failure)?;
                    let seconds = matches
                        .get_one::<String>("pow_timeout")
                        .ok_or_else(|| failure("missing PoW timeout"))?
                        .parse::<f64>()
                        .map_err(failure)?;
                    let config = PowConfig {
                        workers: *matches
                            .get_one::<u8>("pow_workers")
                            .ok_or_else(|| failure("missing PoW workers"))?,
                        timeout: Duration::try_from_secs_f64(seconds).map_err(failure)?,
                    };
                    PowSolver::new(config).map_err(|error| CliError::input(error.to_string()))?;
                    return Ok(Self::Pow(PowRegistration {
                        netuid,
                        hotkey,
                        config,
                        period: request.period,
                    }));
                }
                if netuid == NetUid::ROOT {
                    let mut args = request.args;
                    args.remove("netuid");
                    let request = TxRequest {
                        op: "root_register",
                        args,
                        period: request.period,
                    };
                    request.intent()?;
                    Ok(Self::Transaction(request))
                } else {
                    Ok(Self::Transaction(request))
                }
            }
            _ => Err(CliError::input(format!("unknown subnets command {name:?}"))),
        }
    }
}

/// Mine against the public effective owner and the already selected hotkey.
/// The returned request preserves the mining limits for late proof refresh.
pub async fn prepare_pow(
    ctx: &AppContext,
    client: &Client,
    request: &PowRegistration,
) -> Result<TxRequest, CliError> {
    if ctx.globals.dry_run {
        return Err(CliError::input("proof searches are disabled for --dry-run"));
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
            owner.ok_or_else(|| CliError::input("PoW registration requires a public coldkey"))?
        }
    };
    ctx.output
        .message("mining a fresh registration proof")
        .map_err(failure)?;
    let proof = PowSolver::new(request.config)
        .map_err(|error| CliError::input(error.to_string()))?
        .solve(
            client,
            request.netuid,
            request.hotkey,
            owner,
            &CancellationToken::new(),
        )
        .await
        .map_err(failure)?;
    let request = TxRequest {
        op: "pow_register",
        args: Map::from_iter([
            ("netuid".into(), json!(request.netuid)),
            ("hotkey_ss58".into(), json!(request.hotkey)),
            ("work_block".into(), json!(proof.work_block)),
            ("nonce".into(), json!(proof.nonce)),
            (
                "work_hex".into(),
                json!(format!("0x{}", hex::encode(proof.work))),
            ),
            ("mining_workers".into(), json!(request.config.workers)),
            (
                "mining_timeout".into(),
                json!(request.config.timeout.as_secs_f64()),
            ),
            ("mining_backend".into(), json!("cpu")),
        ]),
        period: request.period,
    };
    request.intent()?;
    Ok(request)
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

fn raw_text(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        _ => value.to_string(),
    }
}

fn required_u64(graph: &Value, key: &str) -> Result<u64, CliError> {
    graph
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| failure(format!("metagraph {key} is not a native u64")))
}

fn optional_u64(value: &Value, key: &str) -> Result<u64, CliError> {
    if value.is_null() {
        return Ok(0);
    }
    value
        .as_u64()
        .ok_or_else(|| failure(format!("metagraph {key} is not a native u64")))
}

fn endpoint(axon: &Value) -> Result<Option<String>, CliError> {
    let Some(axon) = axon.as_object() else {
        return Ok(None);
    };
    let ip = axon
        .get("ip")
        .and_then(Value::as_number)
        .and_then(serde_json::Number::as_u128)
        .unwrap_or(0);
    if ip == 0 {
        return Ok(None);
    }
    let port = axon.get("port").and_then(Value::as_u64).unwrap_or(0);
    if axon.get("ip_type").and_then(Value::as_u64).unwrap_or(4) == 6 {
        Ok(Some(format!("[{}]:{port}", Ipv6Addr::from(ip))))
    } else {
        let ip =
            u32::try_from(ip).map_err(|_| failure("metagraph IPv4 endpoint exceeds 32 bits"))?;
        Ok(Some(format!("{}:{port}", Ipv4Addr::from(ip))))
    }
}

/// Human metagraph sections and stake-sorted neuron records; JSON stays raw.
pub fn render_metagraph(
    output: &Output,
    netuid: NetUid,
    graph: &Value,
    local_names: &BTreeMap<String, String>,
) -> Result<(), CliError> {
    let record = graph
        .as_object()
        .ok_or_else(|| failure(format!("subnet {netuid} does not exist")))?;
    if output.options.json_mode {
        return output.value(graph).map_err(failure);
    }
    let blocks = required_u64(graph, "block")?;
    let raw = |key: &str| -> Result<(String, Value), CliError> {
        let value = record
            .get(key)
            .ok_or_else(|| failure(format!("metagraph lacks {key}")))?;
        let annotation =
            integer(value).and_then(|raw| rtao_catalog::hyperparams::TABLE.annotate(key, raw));
        Ok((
            key.into(),
            json!(annotation.map_or_else(
                || raw_text(value),
                |note| format!("{}  {note}", raw_text(value))
            )),
        ))
    };
    let money = |key: &str, subnet: NetUid| -> Result<(String, Value), CliError> {
        Ok((
            key.into(),
            json!(
                output
                    .balance(required_u64(graph, key)?, subnet)
                    .0
                    .to_python_string()
            ),
        ))
    };
    let overview = [
        "owner_hotkey",
        "owner_coldkey",
        "network_registered_at",
        "block",
        "tempo",
        "last_step",
        "blocks_since_last_step",
    ]
    .into_iter()
    .map(raw)
    .collect::<Result<Vec<_>, _>>()?;
    output
        .detail(Some(&format!("metagraph netuid {netuid}")), &overview, None)
        .map_err(failure)?;
    let identity = record
        .get("identity")
        .and_then(Value::as_object)
        .map(|fields| {
            fields
                .iter()
                .filter(|(_, value)| !value.is_null() && **value != json!(""))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    output
        .detail(Some("identity"), &identity, None)
        .map_err(failure)?;
    let tao_in = required_u64(graph, "tao_in")?;
    let alpha_in = required_u64(graph, "alpha_in")?;
    let mut pool = Vec::new();
    if alpha_in != 0 {
        let price = integer_division(i128::from(tao_in), alpha_in)
            .ok_or_else(|| failure("metagraph spot-price denominator is zero"))?;
        pool.push((
            "price".into(),
            json!(format!("{price:.6} τ per alpha (spot)")),
        ));
    }
    if let Some(bits) = record
        .get("moving_price")
        .and_then(|value| value.get("bits"))
        .and_then(Value::as_number)
        .and_then(serde_json::Number::as_i128)
    {
        pool.push((
            "moving_price".into(),
            json!(format!("{:.6}", bits as f64 / 4_294_967_296.0)),
        ));
    }
    for key in [
        "tao_in",
        "subnet_emission",
        "tao_in_emission",
        "pending_root_emission",
    ] {
        pool.push(money(key, NetUid::ROOT)?);
    }
    // The cumulative volume is a native u128 counter, unlike u64 balances.
    // Keep its full range without constructing or narrowing a balance.
    let volume = record
        .get("subnet_volume")
        .and_then(integer)
        .and_then(RawInteger::as_u128)
        .ok_or_else(|| failure("metagraph subnet_volume is not a native u128"))?;
    pool.push((
        "subnet_volume".into(),
        json!(format!(
            "τ{}.{:09}",
            volume / 1_000_000_000,
            volume % 1_000_000_000
        )),
    ));
    for key in [
        "alpha_in",
        "alpha_out",
        "alpha_in_emission",
        "alpha_out_emission",
        "pending_alpha_emission",
    ] {
        pool.push(money(key, netuid)?);
    }
    output
        .detail(Some("pool & emission"), &pool, None)
        .map_err(failure)?;
    for (title, keys) in [
        (
            "registration",
            &[
                "registration_allowed",
                "pow_registration_allowed",
                "burn",
                "min_burn",
                "max_burn",
                "difficulty",
                "min_difficulty",
                "max_difficulty",
                "immunity_period",
                "adjustment_interval",
                "adjustment_alpha",
                "target_regs_per_interval",
                "max_regs_per_block",
                "serving_rate_limit",
            ][..],
        ),
        (
            "weights & bonds",
            &[
                "rho",
                "kappa",
                "min_allowed_weights",
                "max_weights_limit",
                "weights_version",
                "weights_rate_limit",
                "activity_cutoff",
                "max_validators",
                "commit_reveal_weights_enabled",
                "commit_reveal_period",
                "liquid_alpha_enabled",
                "alpha_high",
                "alpha_low",
                "bonds_moving_avg",
            ][..],
        ),
    ] {
        let fields = keys
            .iter()
            .copied()
            .map(raw)
            .collect::<Result<Vec<_>, _>>()?;
        output.detail(Some(title), &fields, None).map_err(failure)?;
    }
    let hotkeys = record
        .get("hotkeys")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let column = |key: &str, uid: usize| {
        record
            .get(key)
            .and_then(Value::as_array)
            .and_then(|values| values.get(uid))
            .cloned()
            .unwrap_or(Value::Null)
    };
    let mut neurons = Vec::new();
    let mut validators = 0usize;
    let mut active_count = 0usize;
    for (uid, hotkey) in hotkeys.into_iter().enumerate() {
        let hotkey = raw_text(&hotkey);
        let name = local_names.get(&hotkey).cloned().or_else(|| {
            column("identities", uid)
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
                .map(str::to_owned)
        });
        let stake = optional_u64(&column("total_stake", uid), "total_stake")?;
        let emission = optional_u64(&column("emission", uid), "emission")?;
        let validator = column("validator_permit", uid).as_bool().unwrap_or(false);
        let active = column("active", uid).as_bool().unwrap_or(false);
        validators += usize::from(validator);
        active_count += usize::from(active);
        let updated = column("last_update", uid)
            .as_u64()
            .filter(|updated| *updated != 0)
            .map(|updated| i128::from(blocks) - i128::from(updated));
        neurons.push((
            stake,
            vec![
                json!(uid),
                json!(output.account_text(&hotkey, name.as_deref())),
                json!(output.balance(stake, NetUid::ROOT).0.to_python_string()),
                json!(output.balance(emission, netuid).0.to_python_string()),
                json!(column("incentives", uid).as_u64().unwrap_or(0) as f64 / f64::from(u16::MAX)),
                json!(column("dividends", uid).as_u64().unwrap_or(0) as f64 / f64::from(u16::MAX)),
                json!(validator),
                json!(active),
                json!(updated),
                json!(endpoint(&column("axons", uid))?),
            ],
        ));
    }
    neurons.sort_by_key(|neuron| std::cmp::Reverse(neuron.0));
    let num_uids = record
        .get("num_uids")
        .and_then(Value::as_u64)
        .filter(|count| *count != 0)
        .unwrap_or(neurons.len() as u64);
    let max_uids = record.get("max_uids").and_then(Value::as_u64).unwrap_or(0);
    let mut table = TableSpec::new(
        if max_uids == 0 {
            format!("neurons {num_uids}")
        } else {
            format!("neurons {num_uids}/{max_uids}")
        },
        [
            "uid",
            "hotkey",
            "stake",
            "emission",
            "incentive",
            "dividends",
            "validator",
            "active",
            "updated",
            "axon",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
        neurons.into_iter().map(|(_, row)| row).collect(),
    );
    table.records = Some(graph.clone());
    table.footer = Some(format!(
        "{num_uids} uids · {validators} validator permits · {active_count} active. --json carries every per-uid field."
    ));
    output.table(&table).map_err(failure)
}

/// Execute transactions through the shared review path, or inspect a metagraph.
pub async fn execute(
    ctx: &mut AppContext,
    client: &Client,
    request: &Request,
    options: SubmissionOptions<'_>,
    external: Option<&dyn ExternalSignerFactory>,
    history: &mut PromptHistory,
) -> Result<Option<ExtrinsicResult>, CliError> {
    match request {
        Request::Transaction(request) => {
            generated::execute_tx(ctx, client, request, options, external, history).await
        }
        Request::Pow(request) => {
            let request = prepare_pow(ctx, client, request).await?;
            generated::execute_tx(ctx, client, &request, options, external, history).await
        }
        Request::Metagraph(request) => {
            let at = client
                .at(request.block.map_or(BlockRef::Best, BlockRef::Number))
                .await
                .map_err(failure)?;
            let graph = rtao::reads::dispatch(&at, request.name, &request.params)
                .await
                .map_err(failure)?;
            let netuid =
                serde_json::from_value(request.params["netuid"].clone()).map_err(failure)?;
            let names = if ctx.output.options.json_mode {
                BTreeMap::new()
            } else {
                let lookup =
                    FsAccountLookup::new(ctx.globals.wallet_path.clone()).map_err(failure)?;
                lookup
                    .candidates()
                    .into_iter()
                    .filter(|candidate| candidate.source == "hotkey")
                    .map(|candidate| (candidate.address, candidate.name))
                    .collect()
            };
            render_metagraph(&ctx.output, netuid, &graph, &names)?;
            Ok(None)
        }
    }
}
