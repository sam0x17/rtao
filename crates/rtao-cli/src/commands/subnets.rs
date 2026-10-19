// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/commands/subnets.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/metagraph_view.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Registration and raw metagraph inspection through real SDK operations.

use super::generated::{self, QueryRequest, TxRequest};
use crate::{
    books::{AccountLookup, FsAccountLookup, public_json_truthy},
    context::{AppContext, ExternalSignerFactory, SubmissionOptions},
    error::CliError,
    globals::{Tier, with_globals},
    output::{Output, TableSpec},
    prompt::PromptHistory,
};
use clap::{Arg, ArgAction, ArgMatches, Command};
use rtao::{
    client::{BlockRef, Client, ViewExt},
    primitives::{AccountId, NetUid, hyperparams::RawInteger, ratio::integer_division},
    reads::ReadName,
    tx::ExtrinsicResult,
};
use rtao_intents::pow::{PowConfig, PowSolver};
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
                .help(
                    "auto selects available GPU support or CPU; gpu requires the pow-gpu feature.",
                ),
        )
        .arg(
            Arg::new("pow_device")
                .long("pow-device")
                .action(ArgAction::Append)
                .value_parser(clap::value_parser!(u32))
                .help("GPU device id; repeat to select several (requires pow-gpu)."),
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
    let mut command = Command::new("subnets")
        .about("Register and inspect subnets.")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(register)
        .subcommand(generated::intent_command("register_subnet")?.name("create"))
        .subcommand(graph)
        .subcommand(Command::new("list").about("List subnets, highest emission first."))
        .subcommand(inspect_command("show", "Inspect one subnet."))
        .subcommand(
            inspect_command("hyperparameters", "Show subnet hyperparameters.")
                .alias("hparams")
                .arg(Arg::new("name").long("name")),
        )
        .subcommand(inspect_command(
            "burn-cost",
            "Show the hotkey registration cost.",
        ))
        .subcommand(Command::new("create-cost").about("Show the subnet registration cost."))
        .subcommand(inspect_command(
            "price",
            "Show the spot TAO price of alpha.",
        ))
        .subcommand(
            inspect_command("conviction", "Show per-hotkey subnet ownership conviction.")
                .arg(Arg::new("hotkey").long("hotkey"))
                .arg(Arg::new("all").long("all").action(ArgAction::SetTrue)),
        );
    for name in [
        "list",
        "show",
        "hyperparameters",
        "burn-cost",
        "create-cost",
        "price",
        "conviction",
    ] {
        command = command.mut_subcommand(name, |leaf| with_globals(leaf, Tier::Read));
    }
    Ok(command)
}

/// One subnet inspection, taking the subnet as `<netuid>` or `--netuid`.
///
/// Python accepts one spelling per command (positional for `show`, an option
/// for `price`); every rtao inspection accepts both (D-080).
fn inspect_command(name: &'static str, about: &'static str) -> Command {
    let netuid = || clap::value_parser!(u16).range(0..4096);
    Command::new(name)
        .about(about)
        .arg(
            Arg::new("netuid")
                .index(1)
                .value_name("NETUID")
                .value_parser(netuid())
                .required_unless_present("netuid_option")
                .conflicts_with("netuid_option"),
        )
        .arg(
            Arg::new("netuid_option")
                .long("netuid")
                .value_name("NETUID")
                .value_parser(netuid())
                .help("Same as the positional subnet"),
        )
}

/// Public, signer-free subnet inspection inputs.
pub struct InspectRequest {
    /// Canonical command name, including hidden alias normalization.
    pub command: String,
    /// Subnet, absent for whole-network inspection.
    pub netuid: Option<NetUid>,
    /// Optional single hyperparameter explanation.
    pub name: Option<String>,
    /// Optional resolved hotkey for conviction.
    pub hotkey: Option<AccountId>,
    /// Include negligible conviction locks in human output.
    pub all: bool,
    /// Historical block, pinned for the complete command.
    pub block: Option<u64>,
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
        "list" | "create-cost" => Ok(vec![]),
        "show" | "burn-cost" | "price" => Ok(vec!["netuid", "netuid_option"]),
        "hyperparameters" => Ok(vec!["netuid", "netuid_option", "name"]),
        "conviction" => Ok(vec!["netuid", "netuid_option", "hotkey", "all"]),
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
    /// Requested automatic, CPU or GPU mining backend.
    pub backend: String,
    /// Optional explicit GPU device selection.
    pub devices: Option<Vec<u64>>,
    /// Requested mortal transaction period.
    pub period: Option<u32>,
}

/// Source-shaped registration or metagraph request.
pub enum Request {
    /// Curated subnet inspection over one pinned view.
    Inspect(InspectRequest),
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
            "list" | "show" | "hyperparameters" | "burn-cost" | "create-cost" | "price"
            | "conviction" => {
                let hotkey = matches
                    .try_get_one::<String>("hotkey")
                    .ok()
                    .flatten()
                    .map(|hotkey| ctx.resolve_address("hotkey_ss58", Some(hotkey)))
                    .transpose()?
                    .flatten();
                Ok(Self::Inspect(InspectRequest {
                    command: name.into(),
                    netuid: ["netuid", "netuid_option"]
                        .into_iter()
                        .find_map(|id| matches.try_get_one::<u16>(id).ok().flatten())
                        .copied()
                        .map(NetUid),
                    name: matches
                        .try_get_one::<String>("name")
                        .ok()
                        .flatten()
                        .cloned(),
                    hotkey,
                    all: matches
                        .try_get_one::<bool>("all")
                        .ok()
                        .flatten()
                        .copied()
                        .unwrap_or(false),
                    block: matches
                        .try_get_one::<u64>("__block")
                        .ok()
                        .flatten()
                        .copied(),
                }))
            }
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
                    let backend = matches
                        .get_one::<String>("pow_backend")
                        .cloned()
                        .unwrap_or_else(|| "auto".into());
                    let devices = matches
                        .get_many::<u32>("pow_device")
                        .map(|ids| ids.map(|id| u64::from(*id)).collect::<Vec<_>>());
                    super::pow::validate_backend(&backend, devices.as_deref())?;
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
                        backend,
                        devices,
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
                ctx.external_signer_public_account().await?
            } else {
                ctx.resolve_address("coldkey_ss58", None)?
            };
            owner.ok_or_else(|| CliError::input("PoW registration requires a public coldkey"))?
        }
    };
    ctx.output
        .message("mining a fresh registration proof")
        .map_err(failure)?;
    let solver =
        PowSolver::new(request.config).map_err(|error| CliError::input(error.to_string()))?;
    let proof = super::pow::solve_public(
        &solver,
        client,
        request.netuid,
        request.hotkey,
        owner,
        &request.backend,
        request.devices.as_deref(),
    )
    .await?;
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
            ("mining_backend".into(), json!(request.backend)),
            ("mining_device_ids".into(), json!(request.devices)),
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

async fn execute_inspect(
    ctx: &AppContext,
    client: &Client,
    request: &InspectRequest,
) -> Result<(), CliError> {
    use rtao::reads::reads::{m6_locks, m6_root, m6_staking, owner};
    use rtao_runtime::storage::subtensor_module as st;
    let at = client
        .at(request.block.map_or(BlockRef::Best, BlockRef::Number))
        .await
        .map_err(CliError::from)?;
    let output = &ctx.output;
    let netuid = || {
        request
            .netuid
            .ok_or_else(|| CliError::input("missing subnet"))
    };
    match request.command.as_str() {
        "hyperparameters" => {
            let netuid = netuid()?;
            let params = super::hparams::fetch_hyperparameters(&at, netuid).await?;
            super::hparams::render_hyperparameters(output, netuid, &params, request.name.as_deref())
        }
        "price" => {
            let price = m6_staking::alpha_price(&at, netuid()?)
                .await
                .map_err(CliError::from)?;
            let payload = serde_json::to_value(price).map_err(failure)?;
            let fields = payload
                .as_object()
                .ok_or_else(|| failure("price is not an object"))?
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect::<Vec<_>>();
            output
                .detail(None, &fields, Some(&payload))
                .map_err(failure)
        }
        "burn-cost" | "create-cost" => {
            let (key, cost) = if request.command == "create-cost" {
                (
                    "create_cost",
                    m6_root::subnet_registration_cost(&at)
                        .await
                        .map_err(CliError::from)?,
                )
            } else {
                let netuid = netuid()?;
                if !at
                    .storage(&st::NETWORKS_ADDED, (netuid,))
                    .await
                    .map_err(CliError::from)?
                {
                    return Err(failure(format!("subnet {netuid} does not exist")));
                }
                (
                    "burn_cost",
                    rtao::primitives::DynBalance::from_rao(
                        at.storage(&st::BURN, (netuid,))
                            .await
                            .map_err(CliError::from)?
                            .0,
                        NetUid::ROOT,
                    ),
                )
            };
            output
                .detail(
                    None,
                    &[
                        (key.into(), json!(cost.to_python_string())),
                        ("tao".into(), json!(cost.amount())),
                    ],
                    None,
                )
                .map_err(failure)
        }
        "show" => {
            let netuid = netuid()?;
            // Python reads these one at a time; one coalesced query at the
            // pinned block returns the same values.
            let (added, tempo, burn, neuron_count, identity, owner, hotkey, registered, max) = at
                .fetch((
                    st::NETWORKS_ADDED.at((netuid,)),
                    st::TEMPO.at((netuid,)),
                    st::BURN.at((netuid,)),
                    st::SUBNETWORK_N.at((netuid,)),
                    st::SUBNET_IDENTITIES_V3.at((netuid,)),
                    st::SUBNET_OWNER.at((netuid,)),
                    st::SUBNET_OWNER_HOTKEY.at((netuid,)),
                    st::NETWORK_REGISTERED_AT.at((netuid,)),
                    st::MAX_ALLOWED_UIDS.at((netuid,)),
                ))
                .await
                .map_err(CliError::from)?;
            if !added {
                return Err(failure(format!("subnet {netuid} does not exist")));
            }
            let info = m6_root::SubnetInfo {
                netuid,
                tempo,
                burn: rtao::primitives::DynBalance::from_rao(burn.0, NetUid::ROOT),
                neuron_count,
            };
            let identity = identity
                .map(owner::normalize_identity)
                .transpose()
                .map_err(CliError::from)?;
            let name = identity
                .as_ref()
                .and_then(|i| i.get("subnet_name"))
                .filter(|v| v.as_str().is_some_and(|s| !s.is_empty()));
            if let Some(name) = name.and_then(Value::as_str) {
                output
                    .update_subnet_names([(netuid.0, name.into())])
                    .map_err(failure)?;
            }
            let record = json!({"netuid":netuid,"name":name,"symbol":output.unit(netuid),
                "owner_coldkey":owner.to_string(),"owner_hotkey":hotkey.to_string(),
                "registered_at":registered,"tempo":info.tempo,"burn_tao":info.burn.amount(),
                "neurons":info.neuron_count,"max_neurons":(max != 0).then_some(max),"identity":identity});
            let lookup = FsAccountLookup::new(ctx.globals.wallet_path.clone()).map_err(failure)?;
            let names: BTreeMap<_, _> = lookup
                .candidates()
                .into_iter()
                .map(|c| (c.address, c.name))
                .collect();
            let fields = vec![
                ("name".into(), name.cloned().unwrap_or(Value::Null)),
                (
                    "owner".into(),
                    json!(output.account_text(
                        &owner.to_string(),
                        names.get(&owner.to_string()).map(String::as_str)
                    )),
                ),
                (
                    "owner hotkey".into(),
                    json!(output.account_text(
                        &hotkey.to_string(),
                        names.get(&hotkey.to_string()).map(String::as_str)
                    )),
                ),
                (
                    "registered".into(),
                    json!(format!(
                        "block {registered}{}",
                        registered_note(registered, at.block().number)
                            .map(|s| format!(" ({s})"))
                            .unwrap_or_default()
                    )),
                ),
                ("tempo".into(), json!(info.tempo)),
                ("burn".into(), json!(info.burn.to_python_string())),
                (
                    "neurons".into(),
                    json!(if max == 0 {
                        info.neuron_count.to_string()
                    } else {
                        format!("{}/{max}", info.neuron_count)
                    }),
                ),
            ];
            output
                .detail(Some(&format!("subnet {netuid}")), &fields, Some(&record))
                .map_err(failure)?;
            if !output.options.json_mode {
                let links = [
                    ("github_repo", "github"),
                    ("subnet_url", "url"),
                    ("discord", "discord"),
                    ("subnet_contact", "contact"),
                    ("description", "description"),
                    ("logo_url", "logo"),
                    ("additional", "additional"),
                ]
                .into_iter()
                .filter_map(|(key, label)| {
                    identity
                        .as_ref()?
                        .get(key)
                        .filter(|v| !v.is_null() && **v != json!(""))
                        .map(|v| (label.into(), v.clone()))
                })
                .collect::<Vec<_>>();
                output
                    .detail(Some("identity"), &links, None)
                    .map_err(failure)?;
            }
            Ok(())
        }
        "list" => {
            let listing = m6_root::subnet_listing(&at).await.map_err(CliError::from)?;
            let names = listing.names;
            output
                .update_subnet_names(names.iter().map(|(id, name)| (id.0, name.clone())))
                .map_err(failure)?;
            let prices = m6_staking::alpha_prices(&at)
                .await
                .map_err(CliError::from)?;
            let flows = listing.tao_flows;
            let max = listing.max_uids;
            let emissions = listing.tao_in_emission;
            let mut infos = listing.subnets;
            infos.sort_by_key(|i| {
                (
                    std::cmp::Reverse(emissions.get(&i.netuid).copied().unwrap_or(0)),
                    i.netuid,
                )
            });
            let mut rows = Vec::new();
            let mut records = Vec::new();
            for i in &infos {
                let emission = emissions
                    .get(&i.netuid)
                    .copied()
                    .unwrap_or(0)
                    .checked_mul(7200)
                    .ok_or_else(|| failure("daily emission exceeds native u64 balance"))?;
                let emission =
                    rtao::primitives::DynBalance::from_rao(emission, NetUid::ROOT).amount();
                let price = prices
                    .get(&i.netuid)
                    .copied()
                    .filter(|_| i.netuid != NetUid::ROOT);
                let flow = flows
                    .get(&i.netuid)
                    .map(|v| v * 7200.0 / 1e9)
                    .filter(|_| i.netuid != NetUid::ROOT);
                let emission = (i.netuid != NetUid::ROOT).then_some(emission);
                rows.push(vec![
                    json!(i.netuid),
                    json!(names.get(&i.netuid)),
                    json!(
                        price
                            .map(|p| format!("{p:.6}"))
                            .unwrap_or_else(|| "—".into())
                    ),
                    json!(
                        emission
                            .map(|e| format!("{e:.2}"))
                            .unwrap_or_else(|| "—".into())
                    ),
                    json!(
                        flow.map(|f| format!("{f:+.2}"))
                            .unwrap_or_else(|| "—".into())
                    ),
                    json!(i.burn.to_python_string()),
                ]);
                records.push(json!({"netuid":i.netuid,"name":names.get(&i.netuid),"symbol":output.unit(i.netuid),"price_tao_per_alpha":price,"emission_tao_per_day":emission,"tao_flow_tao_per_day":flow,"tempo":i.tempo,"burn_tao":i.burn.amount(),"neurons":i.neuron_count,"max_neurons":max.get(&i.netuid),"url":output.options.network.as_deref().filter(|n| matches!(*n, "finney" | "archive")).map(|_| format!("https://taomarketcap.com/subnets/{}",i.netuid))}));
            }
            let mut table = TableSpec::new(
                "subnets",
                [
                    "netuid",
                    "name",
                    "price (τ)",
                    "emission (τ/day)",
                    "flow (τ/day)",
                    "burn",
                ]
                .into_iter()
                .map(str::to_owned)
                .collect(),
                rows,
            );
            table.records = Some(json!(records));
            table.legend = [
                ("price (τ)", "spot price of one alpha token in TAO (root has no alpha pool)."),
                ("emission (τ/day)", "TAO injected into the subnet per day at the current rate."),
                ("flow (τ/day)", "smoothed net TAO flow from staking: positive means TAO is entering the subnet, negative means it is leaving."),
                ("burn", "TAO burned to register a neuron on the subnet right now."),
                ("symbol", "the subnet's alpha token symbol is included with its netuid."),
            ]
            .into_iter()
            .map(|(term, meaning)| (term.into(), meaning.into()))
            .collect();
            table.footer = Some(format!(
                "{} subnets · {} neurons · sorted by emission",
                infos.len(),
                infos.iter().map(|i| u64::from(i.neuron_count)).sum::<u64>()
            ));
            output.table(&table).map_err(failure)
        }
        "conviction" => {
            let netuid = netuid()?;
            if let Some(hotkey) = request.hotkey {
                let data = m6_locks::hotkey_conviction(&at, hotkey, netuid)
                    .await
                    .map_err(CliError::from)?;
                return output
                    .detail(
                        Some(&format!("conviction netuid {netuid}")),
                        &[(
                            "conviction_alpha".into(),
                            json!(data.conviction_alpha.to_python_string()),
                        )],
                        Some(&serde_json::to_value(data).map_err(failure)?),
                    )
                    .map_err(failure);
            }
            let data = m6_locks::subnet_convictions(&at, netuid)
                .await
                .map_err(CliError::from)?;
            let lookup = FsAccountLookup::new(ctx.globals.wallet_path.clone()).map_err(failure)?;
            let mut names: BTreeMap<_, _> = lookup
                .candidates()
                .into_iter()
                .filter(|c| c.source == "hotkey")
                .map(|c| {
                    let name = c.name.rsplit('/').next().unwrap_or(&c.name).to_owned();
                    (c.address, name)
                })
                .collect();
            for entry in &ctx.books.addresses {
                if let (Some(address), Some(name)) = (
                    entry
                        .get("address")
                        .and_then(Value::as_str)
                        .filter(|address| !address.is_empty()),
                    entry
                        .get("name")
                        .and_then(Value::as_str)
                        .filter(|name| !name.is_empty()),
                ) {
                    names.entry(address.into()).or_insert_with(|| name.into());
                }
            }
            let unnamed = data
                .hotkeys
                .iter()
                .map(|e| e.hotkey)
                .filter(|key| !names.contains_key(&key.to_string()))
                .collect();
            for (key, identity) in owner::hotkey_identities(&at, unnamed)
                .await
                .map_err(CliError::from)?
            {
                if let Some(name) = identity
                    .get("name")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                {
                    names.entry(key.to_string()).or_insert_with(|| name.into());
                }
            }
            render_convictions_named(output, &data, request.all, &names)
        }
        _ => Err(CliError::input("unknown subnet inspection")),
    }
}

/// Human registration age at the pinned head.
pub fn registered_note(registered: u64, block: u64) -> Option<String> {
    let seconds = block.checked_sub(registered)?.checked_mul(12)?;
    if seconds == 0 {
        return None;
    }
    Some(if seconds >= 365 * 86400 {
        format!("~{:.1}y ago", seconds as f64 / (365.0 * 86400.0))
    } else if seconds >= 86400 {
        format!("~{:.0}d ago", seconds as f64 / 86400.0)
    } else if seconds >= 3600 {
        format!("~{:.0}h ago", seconds as f64 / 3600.0)
    } else if seconds >= 60 {
        format!("~{}m ago", seconds / 60)
    } else {
        "just now".into()
    })
}

/// Individual ownership-gate estimate; total conviction is never used as a gate.
pub fn conviction_eta(blocks: Option<u128>) -> String {
    match blocks {
        None => "won't clear 18%".into(),
        Some(0) => "above 18%".into(),
        Some(blocks) => {
            let seconds = blocks as f64 * 12.0;
            if seconds >= 86400.0 {
                format!("18% in ~{:.0}d", seconds / 86400.0)
            } else if seconds >= 3600.0 {
                format!("18% in ~{:.0}h", seconds / 3600.0)
            } else {
                format!("18% in ~{}m", (blocks.saturating_mul(12) / 60).max(1))
            }
        }
    }
}

/// Render all locks in JSON while applying the Python dust cutoff only to human rows.
pub fn render_convictions(
    output: &Output,
    data: &rtao::reads::reads::m6_locks::SubnetConvictions,
    all: bool,
) -> Result<(), CliError> {
    render_convictions_named(output, data, all, &BTreeMap::new())
}

fn render_convictions_named(
    output: &Output,
    data: &rtao::reads::reads::m6_locks::SubnetConvictions,
    all: bool,
    names: &BTreeMap<String, String>,
) -> Result<(), CliError> {
    let records = data.hotkeys.iter().map(|e| json!({"hotkey":e.hotkey,"is_owner":e.is_owner,"locked":e.locked_alpha.to_python_string(),"locked_alpha":e.locked_alpha.amount(),"conviction":e.conviction_alpha.to_python_string(),"conviction_alpha":e.conviction_alpha.amount(),"pct_of_threshold":e.pct_of_threshold,"blocks_to_threshold":e.blocks_to_threshold})).collect::<Vec<_>>();
    let payload = json!({"netuid":data.netuid,"block":data.block,"alpha_out":data.alpha_out.amount(),"threshold_alpha":data.threshold_alpha.amount(),"total_locked_alpha":data.total_locked_alpha.amount(),"total_conviction_alpha":data.total_conviction_alpha.amount(),"leader_hotkey":data.leader_hotkey,"leader_blocks_to_threshold":data.leader_blocks_to_threshold,"unlock_rate":data.unlock_rate,"maturity_rate":data.maturity_rate,"owner_hotkey":data.owner_hotkey,"registered_at":data.registered_at,"ownership_changeable_at_block":data.ownership_changeable_at_block,"hotkeys":records});
    if output.options.json_mode {
        return output.value(&payload).map_err(failure);
    }
    let cutoff = (data.threshold_alpha.rao / 10_000).max(1_000_000);
    let shown = data
        .hotkeys
        .iter()
        .filter(|e| all || e.locked_alpha.rao >= cutoff || e.conviction_alpha.rao >= cutoff)
        .collect::<Vec<_>>();
    output
        .detail(
            Some(&format!("conviction netuid {}", data.netuid)),
            &[
                (
                    "alpha out".into(),
                    json!(format!(
                        "{} — alpha in circulation on this subnet (grows with emissions)",
                        data.alpha_out.to_python_string()
                    )),
                ),
                (
                    "takeover threshold".into(),
                    json!(format!(
                        "{} — one hotkey must exceed 18% of eligible alpha",
                        data.threshold_alpha.to_python_string()
                    )),
                ),
                (
                    "unlock rate".into(),
                    json!(format!(
                        "{} blocks — a decaying lock's mass halves every ~{:.0}d",
                        data.unlock_rate,
                        data.unlock_rate as f64 * std::f64::consts::LN_2 * 12.0 / 86400.0
                    )),
                ),
                (
                    "maturity rate".into(),
                    json!(format!(
                        "{} blocks — conviction closes half its gap to the locked mass every ~{:.0}d",
                        data.maturity_rate,
                        data.maturity_rate as f64 * std::f64::consts::LN_2 * 12.0 / 86400.0
                    )),
                ),
                (
                    "age gate".into(),
                    json!(
                        if u128::from(data.block) >= data.ownership_changeable_at_block {
                            format!(
                                "open — registered at block {} — the one-year age gate has passed",
                                data.registered_at
                            )
                        } else {
                            format!(
                                "block {} — ~{:.0}d away — a subnet must be one year old before ownership can change",
                                data.ownership_changeable_at_block,
                                (data.ownership_changeable_at_block - u128::from(data.block)) as f64 * 12.0 / 86400.0
                            )
                        }
                    ),
                ),
                ("owner hotkey".into(), json!(data.owner_hotkey)),
                (
                    "owner rule".into(),
                    json!("its lock always counts conviction equal to its full locked mass"),
                ),
                (
                    "leader".into(),
                    json!(
                        data.leader_hotkey
                            .map(|key| format!(
                                "{}: {}",
                                names
                                    .get(&key.to_string())
                                    .cloned()
                                    .unwrap_or_else(|| key.to_string()),
                                conviction_eta(data.leader_blocks_to_threshold)
                            ))
                            .unwrap_or_else(|| "no locks yet".into())
                    ),
                ),
                (
                    "total locked".into(),
                    json!(data.total_locked_alpha.to_python_string()),
                ),
                (
                    "total conviction".into(),
                    json!(format!(
                        "{} — context only; the gate is per-hotkey",
                        data.total_conviction_alpha.to_python_string()
                    )),
                ),
            ],
            None,
        )
        .map_err(failure)?;
    let rows = shown
        .iter()
        .map(|e| {
            vec![
                json!(output.account_text(
                    &e.hotkey.to_string(),
                    names.get(&e.hotkey.to_string()).map(String::as_str)
                )),
                json!(if e.is_owner { "owner" } else { "" }),
                json!(e.locked_alpha.to_python_string()),
                json!(e.conviction_alpha.to_python_string()),
                json!(
                    e.pct_of_threshold
                        .map(|v| format!("{:.2}% of threshold", v * 100.0))
                        .unwrap_or_else(|| "no threshold".into())
                ),
                json!(conviction_eta(e.blocks_to_threshold)),
            ]
        })
        .collect();
    let table = TableSpec::new(
        "locks",
        [
            "hotkey",
            "note",
            "locked",
            "conviction",
            "threshold",
            "estimate",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
        rows,
    );
    output.table(&table).map_err(failure)?;
    output
        .detail(
            Some("how conviction works"),
            &[
                (
                    "lock alpha".into(),
                    json!(format!(
                        "Lock alpha to a hotkey with `rtao conviction add --netuid {} --amount <alpha>` to earn conviction — the score that gates subnet ownership.",
                        data.netuid
                    )),
                ),
                (
                    "estimates".into(),
                    json!("The times shown assume the rates and alpha accounting stay constant."),
                ),
                (
                    "full mechanics".into(),
                    json!(format!("{}/guides/conviction", rtao_catalog::py::DOCS_URL)),
                ),
            ],
            None,
        )
        .map_err(failure)?;
    let hidden = data.hotkeys.len() - shown.len();
    if hidden != 0 {
        output
            .message(&format!(
                "{hidden} negligible lock{} hidden (pass `--all` to show)",
                if hidden > 1 { "s" } else { "" }
            ))
            .map_err(failure)?;
    }
    Ok(())
}

fn raw_text(value: &Value) -> String {
    fn repr(value: &Value) -> String {
        match value {
            Value::String(value) => rtao::primitives::pyfmt::string_repr(value),
            _ => raw_text(value),
        }
    }
    match value {
        Value::Null => "None".into(),
        Value::String(value) => value.clone(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Array(values) => format!(
            "[{}]",
            values.iter().map(repr).collect::<Vec<_>>().join(", ")
        ),
        Value::Object(values) => format!(
            "{{{}}}",
            values
                .iter()
                .map(|(key, value)| format!(
                    "{}: {}",
                    rtao::primitives::pyfmt::string_repr(key),
                    repr(value)
                ))
                .collect::<Vec<_>>()
                .join(", ")
        ),
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
    let overview = vec![
        raw("owner_hotkey")?,
        raw("owner_coldkey")?,
        (
            "registered_at".into(),
            json!(required_u64(graph, "network_registered_at")?),
        ),
        raw("block")?,
        raw("tempo")?,
        (
            "last_step".into(),
            json!(format!(
                "{}  {} blocks ago",
                required_u64(graph, "last_step")?,
                required_u64(graph, "blocks_since_last_step")?
            )),
        ),
    ];
    output
        .detail(Some(&format!("metagraph netuid {netuid}")), &overview, None)
        .map_err(failure)?;
    let identity = record
        .get("identity")
        .and_then(Value::as_object)
        .map(|fields| {
            fields
                .iter()
                .filter(|(_, value)| public_json_truthy(value))
                .map(|(key, value)| (key.clone(), json!(raw_text(value))))
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
    pool.push(money("tao_in", NetUid::ROOT)?);
    pool.push(money("alpha_in", netuid)?);
    pool.push(money("alpha_out", netuid)?);
    // The cumulative volume is a native u128 counter, unlike u64 balances.
    // Keep its full range without constructing or narrowing a balance.
    let volume = record
        .get("subnet_volume")
        .and_then(integer)
        .and_then(RawInteger::as_u128)
        .ok_or_else(|| failure("metagraph subnet_volume is not a native u128"))?;
    pool.push((
        "volume".into(),
        json!(format!(
            "τ{}.{:09}",
            volume / 1_000_000_000,
            volume % 1_000_000_000
        )),
    ));
    for (key, subnet, note) in [
        ("subnet_emission", NetUid::ROOT, "per block"),
        ("tao_in_emission", NetUid::ROOT, "per block"),
        ("alpha_in_emission", netuid, "per block"),
        ("alpha_out_emission", netuid, "per block"),
        ("pending_alpha_emission", netuid, "undistributed"),
        ("pending_root_emission", NetUid::ROOT, "undistributed"),
    ] {
        let (key, value) = money(key, subnet)?;
        pool.push((key, json!(format!("{}  {note}", raw_text(&value)))));
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
            .map(|key| {
                if matches!(key, "burn" | "min_burn" | "max_burn") {
                    let (_, balance) = money(key, NetUid::ROOT)?;
                    Ok((
                        key.into(),
                        json!(format!(
                            "{}  = {}",
                            required_u64(graph, key)?,
                            raw_text(&balance)
                        )),
                    ))
                } else {
                    raw(key)
                }
            })
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
                .filter(|name| public_json_truthy(name))
                .map(raw_text)
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
        "{num_uids} uids · {validators} validator permits · {active_count} active. --json carries every per-uid field (trust, consensus, rank, axons); `rtao subnets hyperparameters {netuid}` explains each parameter."
    ));
    output.table(&table).map_err(failure)?;
    // The shared table omits its footer when empty. The metagraph still
    // reports its uid summary and help for an unpopulated subnet.
    if table.rows.is_empty()
        && let Some(footer) = &table.footer
    {
        output.value(footer).map_err(failure)?;
    }
    Ok(())
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
        Request::Inspect(request) => {
            execute_inspect(ctx, client, request).await?;
            // The pinned price command teaches the SDK form after a successful
            // prompted read. This is the actual typed native API; no argument
            // payloads or signer material are copied into the teaching line.
            if request.command == "price"
                && let Some(netuid) = request.netuid
            {
                let view = request.block.map_or_else(
                    || "&client".to_owned(),
                    |block| format!("&client.at(rtao::client::BlockRef::Number({block})).await?"),
                );
                history.record_sdk_hint(format!(
                    "rtao::reads::generated::interfaces::alpha_price({view}, rtao::primitives::NetUid({})).await?",
                    netuid.get()
                ));
            }
            Ok(None)
        }
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
                .map_err(CliError::from)?;
            let graph = rtao::reads::dispatch(&at, request.name, &request.params)
                .await
                .map_err(CliError::from)?;
            let netuid =
                serde_json::from_value(request.params["netuid"].clone()).map_err(failure)?;
            let names = if ctx.output.options.json_mode {
                BTreeMap::new()
            } else {
                let lookup =
                    FsAccountLookup::new(ctx.globals.wallet_path.clone()).map_err(failure)?;
                let mut candidates = lookup
                    .candidates()
                    .into_iter()
                    .filter(|candidate| candidate.source == "hotkey")
                    .collect::<Vec<_>>();
                // Python walks naturally sorted wallets, then naturally sorted
                // hotkeys, retaining the first name for a duplicate address.
                // Keep naturally equal wallet spellings in their original
                // complete groups instead of sorting their hotkeys together.
                let mut wallet_rank = BTreeMap::new();
                for candidate in &candidates {
                    let (wallet, _) = candidate
                        .name
                        .split_once('/')
                        .unwrap_or((&candidate.name, ""));
                    let next = wallet_rank.len();
                    wallet_rank.entry(wallet.to_owned()).or_insert(next);
                }
                candidates.sort_by(|left, right| {
                    let (left_wallet, left_hotkey) =
                        left.name.split_once('/').unwrap_or((&left.name, ""));
                    let (right_wallet, right_hotkey) =
                        right.name.split_once('/').unwrap_or((&right.name, ""));
                    super::wallet::natural_cmp(left_wallet, right_wallet)
                        .then_with(|| {
                            wallet_rank
                                .get(left_wallet)
                                .cmp(&wallet_rank.get(right_wallet))
                        })
                        .then_with(|| super::wallet::natural_cmp(left_hotkey, right_hotkey))
                });
                let mut names = BTreeMap::new();
                for candidate in candidates {
                    let name = candidate.name.rsplit('/').next().unwrap_or(&candidate.name);
                    names
                        .entry(candidate.address)
                        .or_insert_with(|| name.to_owned());
                }
                for entry in &ctx.books.addresses {
                    if let (Some(address), Some(name)) = (
                        entry
                            .get("address")
                            .and_then(Value::as_str)
                            .filter(|address| !address.is_empty()),
                        entry.get("name").filter(|name| public_json_truthy(name)),
                    ) {
                        names
                            .entry(address.into())
                            .or_insert_with(|| raw_text(name));
                    }
                }
                names
            };
            render_metagraph(&ctx.output, netuid, &graph, &names)?;
            Ok(None)
        }
    }
}
