// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/commands/lock.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Stake-lock commands routed through semantic intents and public reads.
use super::generated::{self, TxRequest};
use crate::books::{AccountLookup, FsAccountLookup, NameCandidate, public_json_truthy};
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
use std::collections::BTreeMap;

fn local_names(ctx: &AppContext) -> Result<BTreeMap<String, Value>, CliError> {
    let lookup = FsAccountLookup::new(ctx.globals.wallet_path.clone()).map_err(failure)?;
    Ok(names_from_candidates(
        lookup.candidates(),
        &ctx.books.addresses,
    ))
}

fn names_from_candidates(
    candidates: Vec<NameCandidate>,
    contacts: &[Value],
) -> BTreeMap<String, Value> {
    // Python walks naturally sorted wallets, then naturally sorted hotkeys
    // within each wallet, before keeping the first alias for each address.
    let mut wallets = Vec::<(String, Vec<NameCandidate>)>::new();
    let mut wallet_indices = BTreeMap::new();
    for entry in candidates
        .into_iter()
        .filter(|entry| entry.source == "hotkey")
    {
        let wallet = entry
            .name
            .split_once('/')
            .map_or(entry.name.as_str(), |(wallet, _)| wallet)
            .to_owned();
        let index = *wallet_indices.entry(wallet.clone()).or_insert_with(|| {
            let index = wallets.len();
            wallets.push((wallet, Vec::new()));
            index
        });
        wallets[index].1.push(entry);
    }
    // Stable wallet sorting preserves Python's directory encounter order for
    // equal natural keys such as wallet2/wallet02; their hotkeys must not mix.
    wallets.sort_by(|left, right| super::wallet::natural_cmp(&left.0, &right.0));
    let mut names = BTreeMap::new();
    for (_, mut hotkeys) in wallets {
        hotkeys.sort_by(|left, right| {
            let left = left.name.rsplit('/').next().unwrap_or(&left.name);
            let right = right.name.rsplit('/').next().unwrap_or(&right.name);
            super::wallet::natural_cmp(left, right)
        });
        for entry in hotkeys {
            names.entry(entry.address).or_insert_with(|| {
                Value::String(
                    entry
                        .name
                        .rsplit('/')
                        .next()
                        .unwrap_or(&entry.name)
                        .to_owned(),
                )
            });
        }
    }
    for entry in contacts {
        if let Some(address) = entry["address"].as_str()
            && !address.is_empty()
            && public_json_truthy(&entry["name"])
        {
            // The shared contact file has an open JSON schema. Python keeps
            // truthy labels as their original values, including in JSON.
            names
                .entry(address.to_owned())
                .or_insert_with(|| entry["name"].clone());
        }
    }
    names
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
                                .map(|name| json!(name))
                        })
                        .unwrap_or_else(|| json!(lock.hotkey));
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
                let label = local
                    .cloned()
                    .or_else(|| {
                        identity
                            .get("name")
                            .and_then(Value::as_str)
                            .filter(|s| !s.is_empty())
                            .map(|name| json!(name))
                    })
                    .filter(|name| name.as_str() != Some("—"));
                records.push(json!({"coldkey":position.coldkey,"name":label,"locked":lock.locked_alpha.to_string(),"locked_amount":lock.locked_alpha.amount(),"value":value.to_string(),"value_tao":value.amount(),"is_perpetual":lock.is_perpetual}));
                rows.push(vec![
                    label.unwrap_or_else(|| json!("—")),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wallet_alias_collision_uses_python_natural_wallet_then_hotkey_order() {
        let address = AccountId([7; 32]).to_string();
        let aliases = ["wallet10/worker1", "wallet2/worker10", "wallet2/worker2"];
        for order in [
            [0, 1, 2],
            [0, 2, 1],
            [1, 0, 2],
            [1, 2, 0],
            [2, 0, 1],
            [2, 1, 0],
        ] {
            let mut candidates = order
                .into_iter()
                .map(|index| NameCandidate {
                    name: aliases[index].into(),
                    address: address.clone(),
                    source: "hotkey".into(),
                })
                .collect::<Vec<_>>();
            candidates.insert(
                0,
                NameCandidate {
                    name: "wallet1".into(),
                    address: address.clone(),
                    source: "wallet".into(),
                },
            );
            let contacts = [json!({"address":address, "name":"contact fallback"})];
            let names = names_from_candidates(candidates, &contacts);
            assert_eq!(names.get(&address), Some(&json!("worker2")), "{order:?}");
        }
        for (aliases, expected) in [
            (["wallet2/worker10", "wallet02/worker2"], "worker10"),
            (["wallet02/worker2", "wallet2/worker10"], "worker2"),
        ] {
            let candidates = aliases
                .into_iter()
                .map(|name| NameCandidate {
                    name: name.into(),
                    address: address.clone(),
                    source: "hotkey".into(),
                })
                .collect();
            let names = names_from_candidates(candidates, &[]);
            assert_eq!(names.get(&address), Some(&json!(expected)), "{aliases:?}");
        }
    }
}
