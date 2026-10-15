// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/commands/stake.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Stake transactions, position views, delegation and valued position selection.

use super::generated::{self, TxRequest};
use crate::{
    books::{AccountLookup, FsAccountLookup},
    context::{AppContext, ExternalSignerFactory, SubmissionOptions},
    error::CliError,
    globals::{Tier, with_globals},
    output::TableSpec,
    prompt::PromptHistory,
};
use clap::{Arg, ArgAction, ArgMatches, Command};
use rtao::{
    client::Client,
    primitives::{AccountId, DynBalance, NetUid},
    reads::reads::{m6_delegation, m6_locks, m6_staking, owner},
    tx::ExtrinsicResult,
};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// Atomic single/all-hotkey stake exits.
#[path = "stake_exit.rs"]
pub mod exit;
/// Source and destination pickers, including hidden dust selections.
#[path = "stake_picker.rs"]
pub mod picker;

const ALIASES: [(&str, &str); 7] = [
    ("add", "add_stake"),
    ("remove", "remove_stake"),
    ("move", "move_stake"),
    ("move-swap", "move_swap_stake"),
    ("transfer", "transfer_stake"),
    ("swap", "swap_stake"),
    ("burn", "stake_burn"),
];
fn failure(error: impl std::fmt::Display) -> CliError {
    CliError::failure(error.to_string())
}
fn address(id: &'static str, long: &'static str) -> Arg {
    Arg::new(id)
        .long(long)
        .help(crate::books::ss58_param_help(id))
}
fn netuid() -> Arg {
    Arg::new("netuid")
        .long("netuid")
        .required(true)
        .value_parser(clap::value_parser!(u16))
}
fn optional<'a>(matches: &'a ArgMatches, id: &str) -> Option<&'a str> {
    matches
        .try_get_one::<String>(id)
        .ok()
        .flatten()
        .map(String::as_str)
}
fn hotkey(
    ctx: &mut AppContext,
    leaf: &ArgMatches,
    history: &mut PromptHistory,
) -> Result<AccountId, CliError> {
    ctx.resolve_target("hotkey_ss58", optional(leaf, "hotkey_ss58"), history)?
        .ok_or_else(|| CliError::input("missing hotkey"))
}
fn subnet(leaf: &ArgMatches) -> Result<NetUid, CliError> {
    leaf.get_one::<u16>("netuid")
        .copied()
        .map(NetUid)
        .ok_or_else(|| CliError::input("missing --netuid"))
}
fn tx(
    op: &'static str,
    args: Map<String, Value>,
    leaf: &ArgMatches,
) -> Result<TxRequest, CliError> {
    let request = TxRequest {
        op,
        args,
        period: leaf.try_get_one::<u32>("__period").ok().flatten().copied(),
    };
    request.intent()?;
    Ok(request)
}

/// Build every stake verb, preserving generated argument and compatibility aliases.
pub fn command() -> Result<Command, CliError> {
    let available = rtao_intents::interface_metadata();
    let mut command = Command::new("stake")
        .about("Query and manage stake.")
        .subcommand_required(true)
        .arg_required_else_help(true);
    for (alias, op) in ALIASES {
        if available
            .iter()
            .any(|row| row.name == op && row.compiled && row.implemented)
        {
            command = command.subcommand(generated::intent_command(op)?.name(alias));
        }
    }
    command = command
        .subcommand(exit::command("unstake-all", true))
        .subcommand(exit::command("unstake-all-alpha", false))
        .subcommand(with_globals(
            Command::new("show")
                .about("Show stake in the subnet's own currency.")
                .arg(address("hotkey_ss58", "hotkey"))
                .arg(address("coldkey_ss58", "coldkey"))
                .arg(netuid()),
            Tier::Read,
        ))
        .subcommand(with_globals(
            Command::new("list")
                .about("List every position, its spot value, locks and accrued root yield.")
                .arg(address("coldkey_ss58", "coldkey"))
                .arg(
                    Arg::new("all_wallets")
                        .long("all")
                        .short('a')
                        .action(ArgAction::SetTrue),
                )
                .arg(Arg::new("dust").long("dust").action(ArgAction::SetTrue)),
            Tier::Read,
        ))
        .subcommand(with_globals(
            Command::new("auto")
                .about("List auto-stake destinations.")
                .arg(address("coldkey_ss58", "coldkey")),
            Tier::Read,
        ))
        .subcommand(with_globals(
            Command::new("set-auto")
                .about("Set the subnet's auto-stake destination.")
                .arg(netuid())
                .arg(address("hotkey_ss58", "hotkey")),
            Tier::Tx,
        ))
        .subcommand(with_globals(
            Command::new("wizard").about("Show the stake movement guide."),
            Tier::Read,
        ));
    let child = Command::new("child")
        .alias("children")
        .about("Child hotkey delegation.")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(with_globals(
            Command::new("get")
                .arg(netuid())
                .arg(address("hotkey_ss58", "hotkey")),
            Tier::Read,
        ))
        .subcommand(with_globals(
            Command::new("set")
                .arg(netuid())
                .arg(address("hotkey_ss58", "hotkey"))
                .arg(
                    Arg::new("children")
                        .long("children")
                        .required(true)
                        .help("JSON list of proportion/hotkey pairs."),
                ),
            Tier::Tx,
        ))
        .subcommand(with_globals(
            Command::new("revoke")
                .arg(netuid())
                .arg(address("hotkey_ss58", "hotkey")),
            Tier::Tx,
        ))
        .subcommand(with_globals(
            Command::new("take")
                .arg(netuid())
                .arg(address("hotkey_ss58", "hotkey"))
                .arg(
                    Arg::new("take")
                        .long("take")
                        .required(true)
                        .help("Fraction with a decimal point, or raw u16 proportion."),
                ),
            Tier::Tx,
        ));
    Ok(command.subcommand(child))
}

/// Resolve a familiar generated stake verb to its transaction operation.
pub fn operation(matches: &ArgMatches) -> Result<&'static str, CliError> {
    let (name, _) = matches
        .subcommand()
        .ok_or_else(|| CliError::input("missing stake command"))?;
    ALIASES
        .iter()
        .find_map(|(alias, op)| (*alias == name).then_some(*op))
        .ok_or_else(|| CliError::input(format!("{name:?} has its own stake handler")))
}

/// Leaf-owned names excluded from global wallet and execution overrides.
pub fn reserved(matches: &ArgMatches) -> Result<Vec<&'static str>, CliError> {
    let (name, _) = matches
        .subcommand()
        .ok_or_else(|| CliError::input("missing stake command"))?;
    if let Ok(op) = operation(matches) {
        return generated::intent_reserved(op);
    }
    Ok(match name {
        "unstake-all" | "unstake-all-alpha" => vec!["hotkey_ss58", "all_hotkeys"],
        "show" => vec!["hotkey_ss58", "coldkey_ss58", "netuid"],
        "list" => vec!["coldkey_ss58", "all_wallets", "dust"],
        "auto" => vec!["coldkey_ss58"],
        "set-auto" => vec!["hotkey_ss58", "netuid"],
        "child" => vec!["hotkey_ss58", "netuid", "children", "take"],
        "wizard" => vec![],
        _ => return Err(CliError::input("unknown stake command")),
    })
}

/// Public request prepared before a chain connection or signer acquisition.
pub enum Request {
    /// Ordinary generated intent, already validated.
    Tx(TxRequest),
    /// Interactive generated request whose source picker needs the connected client.
    InteractiveTx {
        /// Intent name.
        op: &'static str,
        /// Supplied leaf arguments.
        matches: ArgMatches,
    },
    /// One coldkey/hotkey position, with an optional live source selection.
    Show {
        /// Position owner.
        owner: AccountId,
        /// Explicit hotkey, or picker.
        hotkey: Option<AccountId>,
        /// Position subnet.
        netuid: NetUid,
    },
    /// Complete position listing for one or all local public wallets.
    List {
        /// Wallet label is included only in all-wallet mode.
        owners: Vec<(Option<String>, AccountId)>,
        /// Show dust in human output.
        dust: bool,
        /// Use batched valuation even when the wallet set is empty.
        all_wallets: bool,
    },
    /// Auto-stake destinations for one owner.
    Auto(AccountId),
    /// Stored child shares, retaining native u64 values.
    Children {
        /// Parent hotkey.
        hotkey: AccountId,
        /// Subnet.
        netuid: NetUid,
    },
    /// Single or atomic multi-hotkey exit.
    Exit(exit::Request),
    /// Static movement guidance, requiring no chain.
    Wizard,
}
impl Request {
    /// Resolve public targets and native fields; interactive generated aliases
    /// retain their supplied matches until the connected picker can run.
    pub fn from_matches(
        ctx: &mut AppContext,
        matches: &ArgMatches,
        history: &mut PromptHistory,
    ) -> Result<Self, CliError> {
        let (name, leaf) = matches
            .subcommand()
            .ok_or_else(|| CliError::input("missing stake command"))?;
        if let Ok(op) = operation(matches) {
            return if ctx.prompt_context().interactive(&ctx.terminal) {
                Ok(Self::InteractiveTx {
                    op,
                    matches: leaf.clone(),
                })
            } else {
                TxRequest::from_matches(ctx, op, leaf, history).map(Self::Tx)
            };
        }
        match name {
            "unstake-all" | "unstake-all-alpha" => {
                exit::Request::from_matches(ctx, name == "unstake-all", leaf, history)
                    .map(Self::Exit)
            }
            "show" => {
                let owner = ctx
                    .resolve_target("coldkey_ss58", optional(leaf, "coldkey_ss58"), history)?
                    .ok_or_else(|| CliError::input("missing public coldkey"))?;
                let hotkey = if optional(leaf, "hotkey_ss58").is_some()
                    || !ctx.prompt_context().interactive(&ctx.terminal)
                {
                    Some(hotkey(ctx, leaf, history)?)
                } else {
                    None
                };
                Ok(Self::Show {
                    owner,
                    hotkey,
                    netuid: subnet(leaf)?,
                })
            }
            "list" => {
                let all_wallets = leaf.get_flag("all_wallets");
                let owners = if all_wallets {
                    let lookup =
                        FsAccountLookup::new(ctx.globals.wallet_path.clone()).map_err(failure)?;
                    let mut wallets = lookup
                        .candidates()
                        .into_iter()
                        .filter(|entry| entry.source == "wallet")
                        .filter_map(|entry| {
                            entry
                                .address
                                .parse()
                                .ok()
                                .map(|account| (Some(entry.name), account))
                        })
                        .collect::<Vec<_>>();
                    wallets.sort_by(|left, right| {
                        super::wallet::natural_cmp(
                            left.0.as_deref().unwrap_or_default(),
                            right.0.as_deref().unwrap_or_default(),
                        )
                    });
                    wallets
                } else {
                    vec![(
                        None,
                        ctx.resolve_target(
                            "coldkey_ss58",
                            optional(leaf, "coldkey_ss58"),
                            history,
                        )?
                        .ok_or_else(|| CliError::input("missing public coldkey"))?,
                    )]
                };
                Ok(Self::List {
                    owners,
                    dust: leaf.get_flag("dust"),
                    all_wallets,
                })
            }
            "auto" => ctx
                .resolve_target("coldkey_ss58", optional(leaf, "coldkey_ss58"), history)?
                .map(Self::Auto)
                .ok_or_else(|| CliError::input("missing public coldkey")),
            "set-auto" => Ok(Self::Tx(tx(
                "set_auto_stake",
                Map::from_iter([
                    ("netuid".into(), json!(subnet(leaf)?)),
                    ("hotkey_ss58".into(), json!(hotkey(ctx, leaf, history)?)),
                ]),
                leaf,
            )?)),
            "child" => {
                let (verb, leaf) = leaf
                    .subcommand()
                    .ok_or_else(|| CliError::input("missing child command"))?;
                let hotkey = hotkey(ctx, leaf, history)?;
                let netuid = subnet(leaf)?;
                if verb == "get" {
                    return Ok(Self::Children { hotkey, netuid });
                }
                let mut args = Map::from_iter([
                    ("hotkey_ss58".into(), json!(hotkey)),
                    ("netuid".into(), json!(netuid)),
                ]);
                let op = match verb {
                    "set" => {
                        let raw = optional(leaf, "children")
                            .ok_or_else(|| CliError::input("missing --children"))?;
                        if raw.len() > 16 * 1024 * 1024 {
                            return Err(CliError::input("--children exceeds 16 MiB"));
                        }
                        let children: Value = serde_json::from_str(raw).map_err(|error| {
                            CliError::failure(format!("invalid --children JSON: {error}"))
                        })?;
                        args.insert("children".into(), children);
                        "set_children"
                    }
                    "revoke" => {
                        args.insert("children".into(), json!([]));
                        "set_children"
                    }
                    "take" => {
                        args.insert(
                            "take".into(),
                            json!(
                                optional(leaf, "take")
                                    .ok_or_else(|| CliError::input("missing --take"))?
                            ),
                        );
                        "set_childkey_take"
                    }
                    _ => return Err(CliError::input("unknown child command")),
                };
                Ok(Self::Tx(tx(op, args, leaf)?))
            }
            "wizard" => Ok(Self::Wizard),
            _ => Err(CliError::input("unknown stake command")),
        }
    }
}

/// Emit the pinned Python movement guide without opening a connection.
pub fn execute_local(ctx: &AppContext, request: &Request) -> Result<bool, CliError> {
    if !matches!(request, Request::Wizard) {
        return Ok(false);
    }
    let payload = json!({
        "move":"btcli stake move --origin-netuid N --destination-netuid M --all",
        "move-swap":"btcli stake move-swap --origin-netuid N --dest-netuid M --all",
        "transfer":"btcli stake transfer --origin-hotkey H1 --destination-hotkey H2 --all",
        "swap":"btcli stake swap --origin-netuid N --destination-netuid M --all",
        "units":"amounts are in the ORIGIN subnet's alpha (TAO when origin netuid is 0); pass `--all` or `--amount all` to move the whole position"
    });
    detail(ctx, Some("stake wizard"), payload)?;
    Ok(true)
}
fn detail(ctx: &AppContext, title: Option<&str>, payload: Value) -> Result<(), CliError> {
    let fields = payload
        .as_object()
        .ok_or_else(|| failure("invalid stake detail record"))?
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<Vec<_>>();
    ctx.output
        .detail(title, &fields, Some(&payload))
        .map_err(failure)
}

/// Execute public views or submit through the shared semantic/external signer path.
pub async fn execute(
    ctx: &mut AppContext,
    client: &Client,
    request: &Request,
    options: SubmissionOptions<'_>,
    external: Option<&dyn ExternalSignerFactory>,
    history: &mut PromptHistory,
) -> Result<Option<ExtrinsicResult>, CliError> {
    match request {
        Request::Tx(request) => {
            return generated::execute_tx(ctx, client, request, options, external, history).await;
        }
        Request::InteractiveTx { op, matches } => {
            let previous = history.replay_command(&[]);
            let request =
                TxRequest::from_matches_with_client(ctx, client, op, matches, history).await?;
            let replay = updated_replay(options.replay, &previous, history);
            return generated::execute_tx(
                ctx,
                client,
                &request,
                SubmissionOptions {
                    replay: replay.as_deref(),
                    ..options
                },
                external,
                history,
            )
            .await;
        }
        Request::Exit(request) => {
            return exit::execute(ctx, client, request, options, external, history).await;
        }
        Request::Show {
            owner,
            hotkey,
            netuid,
        } => {
            let hotkey = match hotkey {
                Some(account) => *account,
                None => {
                    let mut answers = Map::from_iter([("netuid".into(), json!(netuid))]);
                    picker::source_for_owner(
                        ctx,
                        client,
                        *owner,
                        "hotkey_ss58",
                        Some("netuid"),
                        &mut answers,
                        history,
                    )
                    .await?;
                    answers["hotkey_ss58"]
                        .as_str()
                        .ok_or_else(|| failure("picker omitted hotkey"))?
                        .parse()
                        .map_err(failure)?
                }
            };
            let stake = m6_staking::stake(client, *owner, hotkey, *netuid)
                .await
                .map_err(failure)?;
            detail(
                ctx,
                None,
                json!({"coldkey":owner,"hotkey":hotkey,"netuid":netuid,"stake":stake.to_python_string(),"stake_amount":stake.amount(),"stake_unit":stake_unit(*netuid)}),
            )?;
        }
        Request::List {
            owners,
            dust,
            all_wallets,
        } => list(ctx, client, owners, *dust, *all_wallets).await?,
        Request::Auto(owner) => {
            let rows = m6_staking::auto_stake_all(client, *owner)
                .await
                .map_err(failure)?;
            if rows.is_empty() {
                detail(ctx, Some("auto-stake"), json!({"destinations":[]}))?;
            } else {
                let mut names = local_names(ctx)?;
                fill_names(client, rows.iter().map(|row| row.hotkey), &mut names).await?;
                let mut spec = TableSpec::new(
                    "auto-stake destinations",
                    vec!["netuid".into(), "hotkey".into()],
                    rows.iter()
                        .map(|row| {
                            vec![
                                json!(row.netuid),
                                json!(
                                    names
                                        .get(&row.hotkey)
                                        .cloned()
                                        .unwrap_or_else(|| row.hotkey.to_string())
                                ),
                            ]
                        })
                        .collect(),
                );
                spec.records = Some(serde_json::to_value(rows).map_err(failure)?);
                ctx.output.table(&spec).map_err(failure)?;
            }
        }
        Request::Children { hotkey, netuid } => {
            let children = m6_delegation::children(client, *hotkey, *netuid)
                .await
                .map_err(failure)?;
            detail(
                ctx,
                Some("children"),
                json!({"hotkey":hotkey,"netuid":netuid,"children":children}),
            )?;
        }
        Request::Wizard => {
            execute_local(ctx, request)?;
        }
    }
    Ok(None)
}

fn updated_replay(base: Option<&str>, previous: &str, history: &PromptHistory) -> Option<String> {
    let base = base?;
    let current = history.replay_command(&[]);
    // History only appends shell-quoted public tokens. The caller's base
    // already contains the previous answers; append just the connected picks.
    let appended = current
        .strip_prefix(previous)
        .unwrap_or_default()
        .trim_start();
    Some(if appended.is_empty() {
        base.to_owned()
    } else {
        format!("{base} {appended}")
    })
}
fn stake_unit(netuid: NetUid) -> String {
    if netuid == NetUid::ROOT {
        "TAO".into()
    } else {
        format!("alpha (netuid {netuid})")
    }
}

/// Build flat Python records with subnet-wide locks and coldkey-wide root yield.
/// Every monetary aggregation fails when it exceeds the native u64 domain.
pub fn position_records(
    valuation: &owner::StakeValuation,
    locks: &[m6_locks::ColdkeyLock],
    availability: &[m6_staking::StakeAvailability],
    owed: &DynBalance,
    wallet: Option<&str>,
) -> Result<Vec<Value>, CliError> {
    let principal = valuation
        .positions
        .iter()
        .filter(|position| position.netuid == NetUid::ROOT)
        .try_fold(0u64, |sum, position| {
            sum.checked_add(position.stake.rao)
                .ok_or_else(|| CliError::input("root principal exceeds native u64 rao"))
        })?;
    let staked = DynBalance::from_rao(principal, NetUid::ROOT);
    let mut records = Vec::new();
    for position in &valuation.positions {
        let value = picker::spot_value(valuation, &position.stake)?;
        let mut row = Map::from_iter([
            ("netuid".into(), json!(position.netuid)),
            ("hotkey".into(), json!(position.hotkey)),
            ("stake".into(), json!(position.stake.to_python_string())),
            ("stake_amount".into(), json!(position.stake.amount())),
            ("stake_unit".into(), json!(stake_unit(position.netuid))),
            ("value".into(), json!(value.to_python_string())),
            ("value_tao".into(), json!(value.amount())),
            ("registered".into(), json!(position.is_registered)),
            (
                "accrued_basket_yield".into(),
                json!(owed.to_python_string()),
            ),
            ("accrued_basket_yield_tao".into(), json!(owed.amount())),
            ("root_staked".into(), json!(staked.to_python_string())),
            ("root_staked_tao".into(), json!(staked.amount())),
            (
                "personal_yield".into(),
                json!(rtao::primitives::ratio::integer_division(
                    i128::from(owed.rao),
                    principal
                )),
            ),
            ("auto_claim".into(), json!(false)),
        ]);
        if let Some(wallet) = wallet {
            row.insert("wallet".into(), json!(wallet));
            row.insert("coldkey".into(), json!(valuation.coldkey));
        }
        if let Some(available) = availability
            .iter()
            .find(|entry| entry.netuid == position.netuid)
        {
            row.insert("locked".into(), json!(available.locked.to_python_string()));
            row.insert("locked_amount".into(), json!(available.locked.amount()));
            row.insert(
                "available".into(),
                json!(available.available.to_python_string()),
            );
            row.insert(
                "available_amount".into(),
                json!(available.available.amount()),
            );
            if available.locked.rao > 0
                && let Some(lock) = locks.iter().find(|entry| entry.netuid == position.netuid)
            {
                row.insert("lock_hotkey".into(), json!(lock.hotkey));
                row.insert("is_perpetual".into(), json!(lock.is_perpetual));
            }
        }
        records.push(Value::Object(row));
    }
    Ok(records)
}
async fn list(
    ctx: &AppContext,
    client: &Client,
    owners: &[(Option<String>, AccountId)],
    dust: bool,
    all_wallets: bool,
) -> Result<(), CliError> {
    let valuations = if all_wallets {
        owner::stake_value_for_coldkeys(
            client,
            owners.iter().map(|(_, account)| *account).collect(),
        )
        .await
        .map_err(failure)?
    } else {
        let mut values = BTreeMap::new();
        for (_, account) in owners {
            values.insert(
                *account,
                owner::stake_value_for_coldkey(client, *account)
                    .await
                    .map_err(failure)?,
            );
        }
        values
    };
    let mut records = Vec::new();
    let mut rows = Vec::new();
    let mut names = local_names(ctx)?;
    fill_names(
        client,
        valuations
            .values()
            .flat_map(|value| value.positions.iter().map(|position| position.hotkey)),
        &mut names,
    )
    .await?;
    let mut total = 0u64;
    let mut accrued_total = 0u64;
    let mut principal_total = 0u64;
    let mut dust_groups = 0usize;
    let mut dust_positions = 0usize;
    let mut dust_value_tao = 0f64;
    for (name, account) in owners {
        let valuation = valuations
            .get(account)
            .ok_or_else(|| failure("missing coldkey valuation"))?;
        let netuids = valuation
            .positions
            .iter()
            .map(|position| position.netuid)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let (locks, availability, owed) = tokio::try_join!(
            async {
                m6_locks::locks_for_coldkey(client, *account)
                    .await
                    .map_err(failure)
            },
            async {
                m6_staking::stake_availability_for_coldkey(client, *account, netuids)
                    .await
                    .map_err(failure)
            },
            async {
                owner::root_basket_owed(client, *account)
                    .await
                    .map_err(failure)
            },
        )?;
        let wallet_records =
            position_records(valuation, &locks, &availability, &owed, name.as_deref())?;
        total = total
            .checked_add(valuation.stake_value.rao)
            .ok_or_else(|| CliError::input("stake total exceeds native u64 rao"))?;
        accrued_total = accrued_total
            .checked_add(owed.rao)
            .ok_or_else(|| CliError::input("accrued yield exceeds native u64 rao"))?;
        for position in valuation
            .positions
            .iter()
            .filter(|position| position.netuid == NetUid::ROOT)
        {
            principal_total = principal_total
                .checked_add(position.stake.rao)
                .ok_or_else(|| CliError::input("root principal exceeds native u64 rao"))?;
        }
        fill_names(client, locks.iter().map(|lock| lock.hotkey), &mut names).await?;
        let mut groups = BTreeMap::<NetUid, Vec<(u64, &Value)>>::new();
        for (position, record) in valuation.positions.iter().zip(&wallet_records) {
            groups
                .entry(position.netuid)
                .or_default()
                .push((position.stake.rao, record));
        }
        let ordered = groups
            .into_iter()
            .map(|(netuid, mut records)| {
                records.sort_by(|left, right| right.0.cmp(&left.0));
                let rao = valuation
                    .positions
                    .iter()
                    .filter(|position| position.netuid == netuid)
                    .try_fold(0u64, |sum, position| {
                        sum.checked_add(position.stake.rao).ok_or_else(|| {
                            CliError::input("subnet stake total exceeds native u64 rao")
                        })
                    })?;
                let value = picker::spot_value(valuation, &DynBalance::from_rao(rao, netuid))?.rao;
                Ok::<_, CliError>((netuid, value, records))
            })
            .collect::<Result<Vec<_>, _>>()?;
        for (netuid, value, group) in ordered {
            if !dust && value < 1_000_000 {
                dust_groups += 1;
                dust_value_tao += rtao::primitives::pyfmt::rao_to_f64(value);
                continue;
            }
            let group = group.into_iter().filter(|(_, record)| {
                let value = record["value_tao"].as_f64().unwrap_or_default();
                if !dust && value < 0.001 {
                    dust_positions += 1;
                    dust_value_tao += value;
                    false
                } else {
                    true
                }
            });
            for (index, (_, record)) in group.enumerate() {
                let hotkey: AccountId = record["hotkey"]
                    .as_str()
                    .ok_or_else(|| failure("invalid hotkey record"))?
                    .parse()
                    .map_err(failure)?;
                let mut row = Vec::new();
                if all_wallets {
                    row.push(json!(name));
                }
                let locked = record
                    .get("locked_amount")
                    .and_then(Value::as_f64)
                    .unwrap_or_default()
                    > 0.0;
                let lock = locks
                    .iter()
                    .find(|lock| lock.netuid == netuid)
                    .filter(|_| locked);
                let label = names
                    .get(&hotkey)
                    .cloned()
                    .unwrap_or_else(|| hotkey.to_string());
                let label = if let Some(lock) = lock.filter(|lock| lock.hotkey != hotkey) {
                    format!(
                        "{label}; lock on {}",
                        names
                            .get(&lock.hotkey)
                            .cloned()
                            .unwrap_or_else(|| lock.hotkey.to_string())
                    )
                } else {
                    label
                };
                row.extend([
                    record["netuid"].clone(),
                    record["stake"].clone(),
                    if index == 0 {
                        record.get("locked").cloned().unwrap_or(Value::Null)
                    } else {
                        Value::Null
                    },
                    record["value"].clone(),
                    json!(label),
                    json!(hotkey),
                ]);
                rows.push((record["value_tao"].as_f64().unwrap_or_default(), row));
            }
        }
        records.extend(wallet_records);
    }
    rows.sort_by(|left, right| right.0.total_cmp(&left.0));
    let mut columns = vec![
        "netuid",
        "stake",
        "locked",
        "spot value",
        "validator",
        "hotkey",
    ];
    if all_wallets {
        columns.insert(0, "wallet");
    }
    let mut spec = TableSpec::new(
        "stake (per-subnet currency: TAO on netuid 0, alpha elsewhere)",
        columns.into_iter().map(str::to_owned).collect(),
        rows.into_iter().map(|(_, row)| row).collect(),
    );
    spec.records = Some(Value::Array(records));
    let total = DynBalance::from_rao(total, NetUid::ROOT);
    let accrued = DynBalance::from_rao(accrued_total, NetUid::ROOT);
    let principal = DynBalance::from_rao(principal_total, NetUid::ROOT);
    let accrued_display =
        match rtao::primitives::ratio::integer_division(i128::from(accrued_total), principal_total)
        {
            Some(ratio) => format!("{accrued} ({:.2}% of root stake)", ratio * 100.0),
            None => accrued.to_string(),
        };
    spec.footer = Some(format!(
        "Total: {total}; accrued basket yield: {accrued_display}; root principal: {principal}; auto-claim is off → rtao root claim"
    ));
    ctx.output.table(&spec).map_err(failure)?;
    if spec.rows.is_empty()
        && !ctx.globals.json
        && let Some(footer) = &spec.footer
    {
        ctx.output
            .detail(None, &[("yield".into(), json!(footer))], None)
            .map_err(failure)?;
    }
    if dust_groups > 0 || dust_positions > 0 {
        let mut parts = Vec::new();
        if dust_groups > 0 {
            parts.push(format!(
                "{dust_groups} dust subnet{}",
                if dust_groups == 1 { "" } else { "s" }
            ));
        }
        if dust_positions > 0 {
            parts.push(format!(
                "{dust_positions} dust position{}",
                if dust_positions == 1 { "" } else { "s" }
            ));
        }
        ctx.output
            .message(&format!(
                "{} hidden (each < τ0.001, ≈ τ{dust_value_tao:.6} total) — pass --dust to show",
                parts.join(" and ")
            ))
            .map_err(failure)?;
    }
    Ok(())
}

fn local_names(ctx: &AppContext) -> Result<BTreeMap<AccountId, String>, CliError> {
    let lookup = FsAccountLookup::new(ctx.globals.wallet_path.clone()).map_err(failure)?;
    let mut names = BTreeMap::new();
    for entry in lookup
        .candidates()
        .into_iter()
        .filter(|entry| entry.source == "hotkey")
    {
        if let Ok(account) = entry.address.parse() {
            names.entry(account).or_insert_with(|| {
                entry
                    .name
                    .rsplit('/')
                    .next()
                    .unwrap_or(&entry.name)
                    .to_owned()
            });
        }
    }
    for entry in &ctx.books.addresses {
        if let (Some(address), Some(name)) = (entry["address"].as_str(), entry["name"].as_str())
            && let Ok(account) = address.parse()
        {
            names.entry(account).or_insert_with(|| name.to_owned());
        }
    }
    Ok(names)
}
async fn fill_names(
    client: &Client,
    hotkeys: impl IntoIterator<Item = AccountId>,
    names: &mut BTreeMap<AccountId, String>,
) -> Result<(), CliError> {
    let missing = hotkeys
        .into_iter()
        .filter(|account| !names.contains_key(account))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        for (account, identity) in owner::hotkey_identities(client, missing)
            .await
            .map_err(failure)?
        {
            if let Some(name) = identity["name"].as_str().filter(|name| !name.is_empty()) {
                names.entry(account).or_insert_with(|| name.to_owned());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connected_picker_answers_extend_the_preview_replay_without_repeating_prior_answers() {
        let mut history = PromptHistory::default();
        let argv = vec!["rtao".into(), "stake".into(), "remove".into()];
        history.record_answers(vec!["--wallet".into(), "wallet with space".into()]);
        let previous = history.replay_command(&[]);
        let base = history.replay_command(&argv);
        history.record_answers(vec![
            "--netuid".into(),
            "19".into(),
            "--hotkey".into(),
            "Straße".into(),
        ]);
        assert_eq!(
            updated_replay(Some(&base), &previous, &history),
            Some(history.replay_command(&argv))
        );
        assert_eq!(updated_replay(None, &previous, &history), None);
    }
}
