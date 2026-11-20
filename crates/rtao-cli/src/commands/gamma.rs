// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/commands/gamma.py @ b6a81d144078eac18bd25c3af7cb31579bb670f3 (Apache-2.0); modified by rtao.
//! `rtao gamma`: gamma minting, deposits and spending.

use super::generated::{self, TxRequest};
use crate::{
    context::AppContext,
    error::CliError,
    globals::{Tier, with_globals},
    output::{DiagnosticContext, TableSpec},
    prompt::PromptHistory,
};
use clap::{Arg, ArgMatches, Command};
use rtao_client::Client;
use rtao_primitives::AccountId;
use rtao_reads::ReadName;
use serde_json::{Map, Value, json};

/// The Gamma calls are the generated intent commands (`tx
/// mint-gamma` and so on), mounted as `stake` mounts its verbs, so both
/// spellings stay in sync.
const INTENT_COMMANDS: [(&str, &str); 6] = [
    ("mint", "mint_gamma"),
    ("mint-with-stake", "mint_gamma_with_stake"),
    ("move", "move_gamma"),
    ("close", "close_gamma_deposit"),
    ("spend", "spend_gamma"),
    ("set-cap", "set_gamma_deposit_daily_spend_cap"),
];

fn netuid(help: &'static str) -> Arg {
    Arg::new("netuid")
        .long("netuid")
        .required(true)
        .value_parser(clap::value_parser!(u16))
        .help(help)
}

/// A required hotkey option, which never falls back to the wallet's hotkey.
fn hotkey(help: &str) -> Arg {
    Arg::new("hotkey_ss58")
        .long("hotkey")
        .required(true)
        .help(format!(
            "{help} {}",
            crate::books::ss58_param_help_for("hotkey_ss58", false)
        ))
}

/// Build the gamma command group.
pub fn command() -> Result<Command, CliError> {
    let mut group = Command::new("gamma")
        .about("Gamma: dollar-valued service credits.")
        .long_about(format!(
            "Gamma: dollar-valued service credits.\n\nGuide: {}/guides/gamma",
            rtao_catalog::py::DOCS_URL
        ))
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(with_globals(
            Command::new("show")
                .about("Show the gamma deposit at a hotkey on a subnet.")
                .long_about(format!(
                    "Show the gamma deposit at a hotkey on a subnet.\n\nDocs: {}/query/gamma-deposit",
                    rtao_catalog::py::DOCS_URL
                ))
                .arg(netuid("Subnet of the deposit."))
                .arg(hotkey("Hotkey of the deposit.")),
            Tier::Read,
        ))
        .subcommand(with_globals(
            Command::new("list")
                .about("List every gamma deposit of a coldkey, on every subnet.")
                .long_about(format!(
                    "List every gamma deposit of a coldkey, on every subnet.\n\nDocs: {}/query/gamma-deposits",
                    rtao_catalog::py::DOCS_URL
                ))
                .arg(Arg::new("coldkey_ss58").long("coldkey").help(format!(
                    "Coldkey owning the deposits. {}",
                    crate::books::ss58_param_help("coldkey_ss58")
                ))),
            Tier::Read,
        ));
    for (alias, op) in INTENT_COMMANDS {
        group = group.subcommand(generated::intent_command(op)?.name(alias));
    }
    Ok(group)
}

fn intent_op(name: &str) -> Option<&'static str> {
    INTENT_COMMANDS
        .iter()
        .find(|(alias, _)| *alias == name)
        .map(|(_, op)| *op)
}

/// Reserve command fields from global wallet aliases.
pub fn reserved(matches: &ArgMatches) -> Result<Vec<&'static str>, CliError> {
    match matches.subcommand_name() {
        Some("show") => Ok(vec!["netuid", "hotkey_ss58"]),
        Some("list") => Ok(vec!["coldkey_ss58"]),
        Some(name) => match intent_op(name) {
            Some(op) => generated::intent_reserved(op),
            None => Err(CliError::input("unknown gamma command")),
        },
        None => Err(CliError::input("missing gamma command")),
    }
}

/// Prepare transactions before signing; reads return an empty sequence.
pub fn tx_requests(
    ctx: &mut AppContext,
    matches: &ArgMatches,
    history: &mut PromptHistory,
) -> Result<Vec<TxRequest>, CliError> {
    let (name, leaf) = matches
        .subcommand()
        .ok_or_else(|| CliError::input("missing gamma command"))?;
    match name {
        "show" | "list" => Ok(vec![]),
        name => match intent_op(name) {
            Some(op) => Ok(vec![TxRequest::from_matches(ctx, op, leaf, history)?]),
            None => Err(CliError::input("unknown gamma command")),
        },
    }
}

/// Public read inputs resolved before connecting.
pub enum ReadRequest {
    /// One deposit.
    Show {
        /// Subnet of the deposit.
        netuid: u16,
        /// Hotkey of the deposit.
        hotkey: AccountId,
    },
    /// Every deposit of a coldkey.
    List {
        /// The owning coldkey.
        coldkey: AccountId,
    },
}

/// Resolve the public read target once.
pub fn prepare_read(
    ctx: &mut AppContext,
    matches: &ArgMatches,
    history: &mut PromptHistory,
) -> Result<ReadRequest, CliError> {
    let (name, leaf) = matches
        .subcommand()
        .ok_or_else(|| CliError::input("missing gamma command"))?;
    match name {
        "show" => {
            let netuid = *leaf
                .get_one::<u16>("netuid")
                .ok_or_else(|| CliError::input("missing --netuid"))?;
            let hotkey = ctx
                .resolve_target(
                    "hotkey_ss58",
                    leaf.get_one::<String>("hotkey_ss58").map(String::as_str),
                    history,
                )?
                .ok_or_else(|| CliError::input("gamma show requires --hotkey"))?;
            Ok(ReadRequest::Show { netuid, hotkey })
        }
        "list" => {
            let coldkey = ctx
                .resolve_target(
                    "coldkey_ss58",
                    leaf.get_one::<String>("coldkey_ss58").map(String::as_str),
                    history,
                )?
                .ok_or_else(|| CliError::input("gamma list requires a public coldkey"))?;
            Ok(ReadRequest::List { coldkey })
        }
        _ => Err(CliError::input("not a gamma read command")),
    }
}

fn failure(error: impl std::fmt::Display) -> CliError {
    CliError::failure(error.to_string())
}

/// Python's `f"{Decimal(value):,f} gamma"`, or `no cap` for an absent cap.
fn gamma_text(value: &Value) -> String {
    let Some(text) = value.as_str() else {
        return "no cap".into();
    };
    let (whole, fraction) = text
        .split_once('.')
        .map_or((text, None), |(w, f)| (w, Some(f)));
    let digits: Vec<char> = whole.chars().collect();
    let mut grouped = String::new();
    for (index, digit) in digits.iter().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(*digit);
    }
    match fraction {
        Some(fraction) => format!("{grouped}.{fraction} gamma"),
        None => format!("{grouped} gamma"),
    }
}

fn text(value: &Value) -> Value {
    match value {
        Value::String(_) => value.clone(),
        Value::Null => json!(""),
        other => json!(other.to_string()),
    }
}

/// Execute a resolved public read.
pub async fn execute_prepared_read(
    ctx: &AppContext,
    client: &Client,
    request: &ReadRequest,
) -> Result<(), CliError> {
    match request {
        ReadRequest::Show { netuid, hotkey } => {
            let params = Map::from_iter([
                ("netuid".into(), json!(netuid)),
                ("hotkey_ss58".into(), json!(hotkey)),
            ]);
            let deposit = rtao_reads::dispatch(client, ReadName::GammaDeposit, &params)
                .await
                .map_err(CliError::from)?;
            if deposit.is_null() {
                if let Err(error) = ctx.output.error(
                    &format!("no gamma deposit at {hotkey} on netuid {netuid}"),
                    DiagnosticContext {
                        note: None,
                        help: Some("list a coldkey's deposits with `rtao gamma list`"),
                        see: None,
                    },
                ) {
                    return Err(failure(error));
                }
                return Err(CliError {
                    code: 1,
                    ..CliError::failure(String::new())
                });
            }
            let fields = vec![
                ("hotkey".to_owned(), text(&deposit["hotkey"])),
                ("coldkey".to_owned(), text(&deposit["coldkey"])),
                (
                    "balance".to_owned(),
                    json!(gamma_text(&deposit["balance_gamma"])),
                ),
                (
                    "daily spend cap".to_owned(),
                    json!(gamma_text(&deposit["daily_spend_cap_gamma"])),
                ),
                (
                    "spendable".to_owned(),
                    json!(gamma_text(&deposit["spendable_gamma"])),
                ),
            ];
            ctx.output
                .detail(
                    Some(&format!("gamma deposit on netuid {netuid}")),
                    &fields,
                    Some(&deposit),
                )
                .map_err(failure)
        }
        ReadRequest::List { coldkey } => {
            let params = Map::from_iter([("coldkey_ss58".into(), json!(coldkey))]);
            let deposits = rtao_reads::dispatch(client, ReadName::GammaDeposits, &params)
                .await
                .map_err(CliError::from)?;
            let rows = deposits
                .as_array()
                .map(|deposits| {
                    deposits
                        .iter()
                        .map(|d| {
                            vec![
                                d["netuid"].clone(),
                                d["hotkey"].clone(),
                                json!(gamma_text(&d["balance_gamma"])),
                                json!(gamma_text(&d["daily_spend_cap_gamma"])),
                                json!(gamma_text(&d["spendable_gamma"])),
                            ]
                        })
                        .collect()
                })
                .unwrap_or_default();
            ctx.output
                .columns(&TableSpec {
                    title: "gamma deposits".into(),
                    columns: [
                        "netuid",
                        "hotkey",
                        "balance",
                        "daily spend cap",
                        "spendable",
                    ]
                    .into_iter()
                    .map(str::to_owned)
                    .collect(),
                    rows,
                    records: Some(deposits),
                    right_align: [0, 2, 3, 4].into_iter().collect(),
                    legend: vec![],
                    footer: None,
                })
                .map_err(failure)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gamma_amounts_render_as_python_decimal_with_thousands() {
        assert_eq!(gamma_text(&json!("0")), "0 gamma");
        assert_eq!(
            gamma_text(&json!("1234567.000000001")),
            "1,234,567.000000001 gamma"
        );
        assert_eq!(gamma_text(&json!("999")), "999 gamma");
        assert_eq!(gamma_text(&json!("1000")), "1,000 gamma");
        assert_eq!(gamma_text(&Value::Null), "no cap");
    }

    #[test]
    fn the_group_mounts_every_owner_call_and_spend() -> Result<(), CliError> {
        let group = command()?;
        for name in [
            "show",
            "list",
            "mint",
            "mint-with-stake",
            "move",
            "close",
            "set-cap",
            "spend",
        ] {
            assert!(
                group.find_subcommand(name).is_some(),
                "missing gamma {name}"
            );
        }
        let show = group
            .find_subcommand("show")
            .ok_or_else(|| CliError::input("show"))?;
        let hotkey = show
            .get_arguments()
            .find(|arg| arg.get_id() == "hotkey_ss58")
            .ok_or_else(|| CliError::input("hotkey"))?;
        assert!(hotkey.is_required_set());
        assert!(
            !hotkey
                .get_help()
                .map(|h| h.to_string())
                .unwrap_or_default()
                .contains("Defaults to your wallet")
        );
        Ok(())
    }
}
