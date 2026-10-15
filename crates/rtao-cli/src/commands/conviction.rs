// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/commands/lock.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Stake-lock commands routed through semantic intents and public reads.
use super::generated::{self, TxRequest};
use crate::books::{AccountLookup, FsAccountLookup};
use crate::{
    context::AppContext,
    error::CliError,
    globals::{Tier, with_globals},
    output::TableSpec,
    prompt::PromptHistory,
};
use clap::{Arg, ArgAction, ArgGroup, ArgMatches, Command};
use rtao::{
    client::Client,
    primitives::{AccountId, DynBalance, NetUid},
    reads::{
        ReadName,
        reads::{m6_locks, m6_staking},
    },
};
use serde_json::{Map, Value, json};

fn local_names(ctx: &AppContext) -> Result<std::collections::BTreeMap<String, String>, CliError> {
    let mut names = std::collections::BTreeMap::new();
    let lookup = FsAccountLookup::new(ctx.globals.wallet_path.clone()).map_err(failure)?;
    let candidates = lookup.candidates();
    for entry in candidates.iter().filter(|e| e.source == "hotkey") {
        names.entry(entry.address.clone()).or_insert_with(|| {
            entry
                .name
                .rsplit('/')
                .next()
                .unwrap_or(&entry.name)
                .to_owned()
        });
    }
    for entry in &ctx.books.addresses {
        if let (Some(address), Some(name)) = (entry["address"].as_str(), entry["name"].as_str())
            && !address.is_empty()
            && !name.is_empty()
        {
            names
                .entry(address.to_owned())
                .or_insert_with(|| name.to_owned());
        }
    }
    Ok(names)
}
fn subnet(required: bool) -> Arg {
    Arg::new("netuid")
        .long("netuid")
        .required(required)
        .value_parser(clap::value_parser!(u16))
}
fn address(id: &'static str, flag: &'static str) -> Arg {
    Arg::new(id)
        .long(flag)
        .help(crate::books::ss58_param_help(id))
}
fn switch(id: &'static str) -> Arg {
    Arg::new(id).long(id).action(ArgAction::SetTrue)
}
fn period() -> Arg {
    Arg::new("__period")
        .long("period")
        .value_parser(clap::value_parser!(u32))
}
/// Build all seven conviction commands.
pub fn command() -> Result<Command, CliError> {
    Ok(Command::new("conviction")
        .about("Stake-lock and conviction.")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(with_globals(
            Command::new("list")
                .arg(address("coldkey_ss58", "coldkey"))
                .arg(subnet(false))
                .arg(switch("dust")),
            Tier::Read,
        ))
        .subcommand(with_globals(
            Command::new("targeting")
                .arg(address("hotkey_ss58", "hotkey"))
                .arg(subnet(true)),
            Tier::Read,
        ))
        .subcommand(with_globals(
            Command::new("show")
                .arg(address("coldkey_ss58", "coldkey"))
                .arg(subnet(true)),
            Tier::Read,
        ))
        .subcommand(
            generated::intent_command("lock_stake")?
                .name("add")
                .arg(switch("perpetual")),
        )
        .subcommand(with_globals(
            Command::new("mode")
                .arg(subnet(true))
                .arg(switch("perpetual"))
                .arg(switch("decaying"))
                .group(
                    ArgGroup::new("mode_choice")
                        .args(["perpetual", "decaying"])
                        .required(true),
                )
                .arg(period()),
            Tier::Tx,
        ))
        .subcommand(with_globals(
            Command::new("accept")
                .arg(switch("allow"))
                .arg(switch("reject"))
                .group(
                    ArgGroup::new("accept_choice")
                        .args(["allow", "reject"])
                        .required(true),
                )
                .arg(period()),
            Tier::Tx,
        ))
        .subcommand(generated::intent_command("move_lock")?.name("move")))
}
/// Reserve operation flags so own addresses remain command inputs.
pub fn reserved(matches: &ArgMatches) -> Result<Vec<&'static str>, CliError> {
    Ok(match matches.subcommand_name() {
        Some("add") => {
            let mut v = generated::intent_reserved("lock_stake")?;
            v.push("perpetual");
            v
        }
        Some("move") => generated::intent_reserved("move_lock")?,
        Some("mode") => vec!["netuid", "perpetual", "decaying", "__period"],
        Some("accept") => vec!["allow", "reject", "__period"],
        Some("list") => vec!["coldkey_ss58", "netuid", "dust"],
        Some("show") => vec!["coldkey_ss58", "netuid"],
        Some("targeting") => vec!["hotkey_ss58", "netuid"],
        _ => return Err(CliError::input("missing conviction command")),
    })
}
/// Prepare the entire ordered transaction sequence before any submission.
pub fn tx_requests(
    ctx: &mut AppContext,
    matches: &ArgMatches,
    history: &mut PromptHistory,
) -> Result<Vec<TxRequest>, CliError> {
    let (name, leaf) = matches
        .subcommand()
        .ok_or_else(|| CliError::input("missing conviction command"))?;
    if matches!(name, "list" | "targeting" | "show") {
        return Ok(vec![]);
    }
    if matches!(name, "add" | "move") {
        let tx = TxRequest::from_matches(
            ctx,
            if name == "add" {
                "lock_stake"
            } else {
                "move_lock"
            },
            leaf,
            history,
        )?;
        if name == "add" && leaf.get_flag("perpetual") {
            let mode = TxRequest {
                op: "set_perpetual_lock",
                args: Map::from_iter([
                    (
                        "netuid".into(),
                        tx.args
                            .get("netuid")
                            .cloned()
                            .ok_or_else(|| CliError::input("missing netuid"))?,
                    ),
                    ("enabled".into(), json!(true)),
                ]),
                period: tx.period,
            };
            mode.intent()?;
            return Ok(vec![mode, tx]);
        }
        return Ok(vec![tx]);
    }
    let (op, args) = match name {
        "mode" => (
            "set_perpetual_lock",
            Map::from_iter([
                (
                    "netuid".into(),
                    json!(
                        leaf.get_one::<u16>("netuid")
                            .ok_or_else(|| CliError::input("missing --netuid"))?
                    ),
                ),
                ("enabled".into(), json!(leaf.get_flag("perpetual"))),
            ]),
        ),
        "accept" => (
            "set_reject_locked_alpha",
            Map::from_iter([("enabled".into(), json!(!leaf.get_flag("allow")))]),
        ),
        _ => return Err(CliError::input("unknown conviction command")),
    };
    let tx = TxRequest {
        op,
        args,
        period: leaf.get_one::<u32>("__period").copied(),
    };
    tx.intent()?;
    Ok(vec![tx])
}
fn failure(e: impl std::fmt::Display) -> CliError {
    CliError::failure(e.to_string())
}
/// Public arguments resolved once, before opening the network connection.
pub struct ReadRequest {
    name: &'static str,
    account: AccountId,
    netuid: Option<NetUid>,
    dust: bool,
}
/// Apply the normal public wallet selection flow before connecting.
pub fn prepare_read(
    ctx: &mut AppContext,
    matches: &ArgMatches,
    history: &mut PromptHistory,
) -> Result<ReadRequest, CliError> {
    let (name, leaf) = matches
        .subcommand()
        .ok_or_else(|| CliError::input("missing conviction command"))?;
    let name = match name {
        "list" => "list",
        "show" => "show",
        "targeting" => "targeting",
        _ => return Err(CliError::input("not a conviction read command")),
    };
    let id = if name == "targeting" {
        "hotkey_ss58"
    } else {
        "coldkey_ss58"
    };
    let account = ctx
        .resolve_target(id, leaf.get_one::<String>(id).map(String::as_str), history)?
        .ok_or_else(|| CliError::input("command requires a public account"))?;
    Ok(ReadRequest {
        name,
        account,
        netuid: leaf.get_one::<u16>("netuid").copied().map(NetUid),
        dust: name == "list" && leaf.get_flag("dust"),
    })
}
fn spot(
    balance: &DynBalance,
    prices: &std::collections::BTreeMap<NetUid, f64>,
) -> Result<DynBalance, CliError> {
    let raw = if balance.netuid.0 == 0 {
        return Ok(DynBalance::from_rao(balance.rao, NetUid(0)));
    } else {
        (balance.rao as f64 * prices.get(&balance.netuid).copied().unwrap_or(0.0)).trunc()
    };
    if !(0.0..18_446_744_073_709_551_616.0).contains(&raw) {
        return Err(CliError::failure(
            "lock spot value exceeds native balance range",
        ));
    }
    Ok(DynBalance::from_rao(raw as u64, NetUid(0)))
}
/// Execute public lock views. JSON always contains dust positions.
pub async fn execute_prepared_read(
    ctx: &AppContext,
    client: &Client,
    request: &ReadRequest,
) -> Result<(), CliError> {
    let name = request.name;
    let netuid = request.netuid;
    if name == "show" {
        let owner = request.account;
        let subnet = netuid.ok_or_else(|| CliError::input("missing --netuid"))?;
        let mut params = Map::from_iter([
            ("coldkey_ss58".into(), json!(owner)),
            ("netuid".into(), json!(subnet)),
        ]);
        let lock = rtao::reads::dispatch(client, ReadName::ColdkeyLock, &params)
            .await
            .map_err(failure)?;
        let conviction = if lock.is_null() {
            Value::Null
        } else {
            params.remove("coldkey_ss58");
            params.insert("hotkey_ss58".into(), lock["hotkey"].clone());
            rtao::reads::dispatch(client, ReadName::HotkeyConviction, &params)
                .await
                .map_err(failure)?
        };
        let data = json!({"lock":lock,"conviction":conviction});
        return ctx
            .output
            .detail(
                Some(&format!("lock on netuid {}", subnet.0)),
                &[
                    ("lock".into(), data["lock"].clone()),
                    ("conviction".into(), data["conviction"].clone()),
                ],
                Some(&data),
            )
            .map_err(failure);
    }
    let prices = m6_staking::alpha_prices(client).await.map_err(failure)?;
    let names = local_names(ctx)?;
    let mut records = Vec::new();
    let mut rows = Vec::new();
    let mut total = 0u64;
    let (title, columns, footer) = match name {
        "list" => {
            let owner = request.account;
            let locks = m6_locks::locks_for_coldkey(client, owner)
                .await
                .map_err(failure)?;
            let accepts = m6_locks::accepts_locked_alpha(client, owner)
                .await
                .map_err(failure)?;
            let identities = rtao::reads::reads::owner::hotkey_identities(
                client,
                locks
                    .iter()
                    .map(|l| l.hotkey)
                    .filter(|h| !names.contains_key(&h.to_string()))
                    .collect(),
            )
            .await
            .map_err(failure)?;
            let mut dust_count = 0usize;
            let mut dust_value = 0.0;
            for lock in locks
                .into_iter()
                .filter(|l| netuid.is_none_or(|n| l.netuid == n))
            {
                let value = spot(&lock.locked_alpha, &prices)?;
                total = total
                    .checked_add(value.rao)
                    .ok_or_else(|| failure("total lock value overflow"))?;
                records.push(json!({"netuid":lock.netuid.0,"hotkey":lock.hotkey,"locked":lock.locked_alpha.to_string(),"locked_amount":lock.locked_alpha.amount(),"locked_unit":if lock.netuid.0==0 {"TAO".into()}else{format!("alpha (netuid {})",lock.netuid.0)},"value":value.to_string(),"value_tao":value.amount(),"is_perpetual":lock.is_perpetual}));
                if request.dust || value.rao >= 1_000_000 {
                    let label = names
                        .get(&lock.hotkey.to_string())
                        .cloned()
                        .or_else(|| {
                            identities
                                .get(&lock.hotkey)
                                .and_then(|i| i.get("name"))
                                .and_then(Value::as_str)
                                .filter(|s| !s.is_empty())
                                .map(str::to_owned)
                        })
                        .unwrap_or_else(|| lock.hotkey.to_string());
                    rows.push(vec![
                        json!(lock.netuid.0),
                        json!(label),
                        json!(lock.locked_alpha.to_string()),
                        json!(value.to_string()),
                        json!(if lock.is_perpetual {
                            "perpetual"
                        } else {
                            "decaying"
                        }),
                    ]);
                } else {
                    dust_count += 1;
                    dust_value += value.amount();
                }
            }
            (
                "locks (per-subnet currency: TAO on netuid 0, alpha elsewhere)".to_owned(),
                vec!["netuid", "hotkey", "locked", "value", "mode"],
                Some(format!(
                    "total locked value {}{}\n{}",
                    DynBalance::from_rao(total, NetUid(0)),
                    if dust_count == 0 {
                        String::new()
                    } else {
                        format!(
                            "\n{dust_count} dust subnet{} hidden (each < τ0.001, ≈ τ{dust_value:.6} total) — pass --dust to show",
                            if dust_count == 1 { "" } else { "s" }
                        )
                    },
                    if accepts {
                        "incoming locked alpha: accepted (`conviction accept --reject` to opt out)"
                    } else {
                        "incoming locked alpha: rejected (chain default; `conviction accept --allow` to opt in)"
                    }
                )),
            )
        }
        "targeting" => {
            let hotkey = request.account;
            let n = netuid.ok_or_else(|| CliError::input("missing --netuid"))?;
            let mut locks = m6_locks::locks_for_hotkey(client, hotkey, n)
                .await
                .map_err(failure)?;
            locks.sort_by_key(|l| std::cmp::Reverse(l.lock.locked_alpha.rao));
            let conviction = m6_locks::hotkey_conviction(client, hotkey, n)
                .await
                .map_err(failure)?;
            for position in locks {
                let lock = &position.lock;
                let value = spot(&lock.locked_alpha, &prices)?;
                total = total
                    .checked_add(value.rao)
                    .ok_or_else(|| failure("total lock value overflow"))?;
                let local = names.get(&position.coldkey.to_string());
                let identity = if local.is_some() {
                    Value::Null
                } else {
                    rtao::reads::dispatch(
                        client,
                        ReadName::Identity,
                        &Map::from_iter([("coldkey_ss58".into(), json!(position.coldkey))]),
                    )
                    .await
                    .map_err(failure)?
                };
                let label = local.map(String::as_str).or_else(|| {
                    identity
                        .get("name")
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())
                });
                records.push(json!({"coldkey":position.coldkey,"name":label,"locked":lock.locked_alpha.to_string(),"locked_amount":lock.locked_alpha.amount(),"value":value.to_string(),"value_tao":value.amount(),"is_perpetual":lock.is_perpetual}));
                rows.push(vec![
                    json!(label.unwrap_or("—")),
                    json!(position.coldkey),
                    json!(lock.locked_alpha.to_string()),
                    json!(value.to_string()),
                    json!(if lock.is_perpetual {
                        "perpetual"
                    } else {
                        "decaying"
                    }),
                ]);
            }
            (
                format!("locks targeting {hotkey} on netuid {}", n.0),
                vec!["name", "coldkey", "locked", "value", "mode"],
                Some(format!(
                    "total locked value {} · hotkey conviction {}",
                    DynBalance::from_rao(total, NetUid(0)),
                    conviction.conviction_alpha
                )),
            )
        }
        _ => return Err(CliError::input("not a conviction read command")),
    };
    let message = footer;
    ctx.output
        .columns(&TableSpec {
            title,
            columns: columns.into_iter().map(str::to_owned).collect(),
            rows,
            records: Some(json!(records)),
            right_align: Default::default(),
            legend: vec![],
            footer: None,
        })
        .map_err(failure)?;
    if let Some(message) = message {
        ctx.output.message(&message).map_err(failure)?;
    }
    Ok(())
}
