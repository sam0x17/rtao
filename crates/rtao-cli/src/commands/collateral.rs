// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/commands/collateral.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Miner collateral commands; no miner process is started.

use super::generated::{self, TxRequest};
use crate::{
    context::AppContext,
    error::CliError,
    globals::{Tier, with_globals},
    prompt::PromptHistory,
};
use clap::{Arg, ArgMatches, Command};
use rtao::{client::Client, primitives::AccountId, reads::ReadName};
use serde_json::{Map, Value, json};

fn netuid() -> Arg {
    Arg::new("netuid")
        .long("netuid")
        .required(true)
        .value_parser(clap::value_parser!(u16))
}
fn address(id: &'static str, flag: &'static str) -> Arg {
    Arg::new(id)
        .long(flag)
        .help(crate::books::ss58_param_help(id))
}
/// Build the pinned collateral command family.
pub fn command() -> Result<Command, CliError> {
    Ok(Command::new("collateral")
        .about("Miner registration collateral.")
        .long_about(format!(
            "Miner registration collateral.\n\nGuide: {}/guides/mining/collateral",
            rtao_catalog::py::DOCS_URL
        ))
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(with_globals(
            Command::new("show")
                .about("Show a position and the subnet collateral policy.")
                .arg(netuid())
                .arg(address("hotkey_ss58", "hotkey"))
                .arg(address("coldkey_ss58", "coldkey").help(
                    "ss58 address, address-book or proxy-book name, saved multisig name, or a local wallet name (uses its coldkey). Defaults to the hotkey owner (collateral is keyed by hotkey + coldkey).",
                )),
            Tier::Read,
        ))
        .subcommand(with_globals(
            Command::new("list")
                .about("List subnet collateral positions.")
                .arg(netuid()),
            Tier::Read,
        ))
        .subcommand(
            generated::intent_command("add_collateral")?
                .name("add")
                .mut_arg("netuid", |arg| arg.required(true)),
        )
        .subcommand(
            generated::intent_command("set_min_collateral")?
                .name("set-min")
                .mut_arg("netuid", |arg| arg.required(true))
                .mut_arg("min_alpha", |arg| arg.required(true).visible_alias("amount")),
        ))
}
/// Reserve command fields from global wallet aliases.
pub fn reserved(matches: &ArgMatches) -> Result<Vec<&'static str>, CliError> {
    match matches.subcommand_name() {
        Some("add") => generated::intent_reserved("add_collateral"),
        Some("set-min") => generated::intent_reserved("set_min_collateral"),
        Some("show") => Ok(vec!["netuid", "hotkey_ss58", "coldkey_ss58"]),
        Some("list") => Ok(vec!["netuid"]),
        _ => Err(CliError::input("missing collateral command")),
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
        .ok_or_else(|| CliError::input("missing collateral command"))?;
    let op = match name {
        "add" => "add_collateral",
        "set-min" => "set_min_collateral",
        "show" | "list" => return Ok(vec![]),
        _ => return Err(CliError::input("unknown collateral command")),
    };
    Ok(vec![TxRequest::from_matches(ctx, op, leaf, history)?])
}
fn failure(error: impl std::fmt::Display) -> CliError {
    CliError::failure(error.to_string())
}
/// Public read inputs resolved before connecting; absent coldkey means owner.
pub struct ReadRequest {
    netuid: u16,
    position: Option<(AccountId, Option<AccountId>)>,
}
/// Select public wallet targets once, preserving an omitted owner lookup.
pub fn prepare_read(
    ctx: &mut AppContext,
    matches: &ArgMatches,
    history: &mut PromptHistory,
) -> Result<ReadRequest, CliError> {
    let (name, leaf) = matches
        .subcommand()
        .ok_or_else(|| CliError::input("missing collateral command"))?;
    let netuid = *leaf
        .get_one::<u16>("netuid")
        .ok_or_else(|| CliError::input("missing --netuid"))?;
    let position = match name {
        "list" => None,
        "show" => {
            let hotkey = ctx
                .resolve_target(
                    "hotkey_ss58",
                    leaf.get_one::<String>("hotkey_ss58").map(String::as_str),
                    history,
                )?
                .ok_or_else(|| CliError::input("collateral show requires a public hotkey"))?;
            let coldkey = leaf
                .get_one::<String>("coldkey_ss58")
                .map(|raw| ctx.resolve_target("coldkey_ss58", Some(raw), history))
                .transpose()?
                .flatten();
            Some((hotkey, coldkey))
        }
        _ => return Err(CliError::input("not a collateral read command")),
    };
    Ok(ReadRequest { netuid, position })
}
/// Execute only resolved public reads, preserving owner lookup.
pub async fn execute_prepared_read(
    ctx: &AppContext,
    client: &Client,
    request: &ReadRequest,
) -> Result<(), CliError> {
    let netuid = request.netuid;
    let mut params = Map::from_iter([("netuid".into(), json!(netuid))]);
    let data = match request.position {
        None => rtao::reads::dispatch(client, ReadName::SubnetCollateral, &params)
            .await
            .map_err(failure)?,
        Some((hotkey, coldkey)) => {
            params.insert("hotkey_ss58".into(), json!(hotkey));
            params.insert("coldkey_ss58".into(), json!(coldkey));
            let collateral = rtao::reads::dispatch(client, ReadName::MinerCollateral, &params)
                .await
                .map_err(failure)?;
            let policy = rtao::reads::dispatch(
                client,
                ReadName::CollateralPolicy,
                &Map::from_iter([("netuid".into(), json!(netuid))]),
            )
            .await
            .map_err(failure)?;
            json!({"collateral":collateral,"policy":policy})
        }
    };
    // The subnet read returns an array. Keep it untouched in JSON, and wrap
    // it for the detail renderer instead of Python's nonempty-list .items()
    // failure in Output.detail.
    let fields: Vec<(String, Value)> = data
        .as_object()
        .map(|o| o.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_else(|| vec![("positions".into(), data.clone())]);
    ctx.output
        .detail(
            Some(&format!("collateral on netuid {netuid}")),
            &fields,
            Some(&data),
        )
        .map_err(failure)
}
