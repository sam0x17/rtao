// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/commands/crowd.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Public crowdloan queries through the typed native-chain read boundary.

use crate::{
    context::AppContext,
    error::CliError,
    globals::{Tier, with_globals},
    output::TableSpec,
};
use clap::{Arg, ArgMatches, Command};
use rtao::{client::Client, reads::ReadName};
use serde_json::{Map, Value, json};

fn failure(error: impl std::fmt::Display) -> CliError {
    CliError::failure(error.to_string())
}

fn id() -> Arg {
    Arg::new("crowdloan_id")
        .index(1)
        .required(true)
        .value_parser(clap::value_parser!(u32))
        .help("Crowdloan ID.")
}

/// Build the three public crowdloan queries, without signing options.
pub fn command() -> Command {
    Command::new("crowd")
        .about("Crowdloan queries (mutations use `rtao tx`).")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(with_globals(
            Command::new("list").about("List all crowdloans."),
            Tier::Read,
        ))
        .subcommand(with_globals(
            Command::new("info")
                .about("Show details for one crowdloan.")
                .arg(id()),
            Tier::Read,
        ))
        .subcommand(with_globals(
            Command::new("contributors")
                .about("List contributors to a crowdloan.")
                .arg(id()),
            Tier::Read,
        ))
}

#[derive(Clone, Copy)]
enum Request {
    List,
    Info(u32),
    Contributors(u32),
}

fn request(group: &ArgMatches) -> Result<Request, CliError> {
    let (name, leaf) = group
        .subcommand()
        .ok_or_else(|| CliError::input("missing crowd command"))?;
    match name {
        "list" => Ok(Request::List),
        "info" | "contributors" => {
            let id = leaf
                .try_get_one::<u32>("crowdloan_id")
                .map_err(|_| CliError::input("invalid crowdloan ID"))?
                .copied()
                .ok_or_else(|| CliError::input("missing crowdloan ID"))?;
            Ok(if name == "info" {
                Request::Info(id)
            } else {
                Request::Contributors(id)
            })
        }
        _ => Err(CliError::input("unknown crowd command")),
    }
}

/// Reserve the native ID from command-global wallet option resolution.
pub fn reserved(group: &ArgMatches) -> Result<Vec<&'static str>, CliError> {
    Ok(match request(group)? {
        Request::List => vec![],
        Request::Info(_) | Request::Contributors(_) => vec!["crowdloan_id"],
    })
}

/// Validate public inputs before creating a connection or opening a wallet.
pub fn validate_before_connect(group: &ArgMatches) -> Result<(), CliError> {
    request(group).map(|_| ())
}

fn detail(ctx: &AppContext, title: &str, keys: &[&str], payload: Value) -> Result<(), CliError> {
    let fields = keys
        .iter()
        .map(|key| ((*key).to_owned(), payload[*key].clone()))
        .collect::<Vec<_>>();
    ctx.output
        .detail(Some(title), &fields, Some(&payload))
        .map_err(failure)
}

fn table(
    ctx: &AppContext,
    title: &str,
    columns: &[&str],
    fields: &[&str],
    payload: Value,
) -> Result<(), CliError> {
    let records = payload
        .as_array()
        .ok_or_else(|| CliError::failure("crowdloan rows are not an array"))?;
    let rows = records
        .iter()
        .map(|row| fields.iter().map(|field| row[*field].clone()).collect())
        .collect();
    let mut table = TableSpec::new(
        title,
        columns.iter().map(|name| (*name).to_owned()).collect(),
        rows,
    );
    table.records = Some(payload);
    ctx.output.table(&table).map_err(failure)
}

/// Execute one public query, retaining Python's full machine records and empty shapes.
pub async fn execute(
    ctx: &AppContext,
    client: &Client,
    group: &ArgMatches,
) -> Result<(), CliError> {
    let request = request(group)?;
    let (name, params) = match request {
        Request::List => (ReadName::Crowdloans, Map::new()),
        Request::Info(id) => (
            ReadName::Crowdloan,
            Map::from_iter([("crowdloan_id".into(), json!(id))]),
        ),
        Request::Contributors(id) => (
            ReadName::CrowdloanContributors,
            Map::from_iter([("crowdloan_id".into(), json!(id))]),
        ),
    };
    let payload = rtao::reads::dispatch(client, name, &params)
        .await
        .map_err(CliError::from)?;
    match request {
        Request::Info(id) => {
            if payload.is_null() {
                return Err(CliError::failure(format!("crowdloan {id} not found")));
            }
            detail(
                ctx,
                &format!("crowdloan {id}"),
                &[
                    "creator",
                    "deposit",
                    "min_contribution",
                    "cap",
                    "raised",
                    "end",
                    "finalized",
                    "target_address",
                    "funds_account",
                ],
                payload,
            )
        }
        Request::List => {
            if payload.as_array().is_some_and(Vec::is_empty) {
                return detail(ctx, "crowdloans", &["count"], json!({"count": 0}));
            }
            table(
                ctx,
                "crowdloans",
                &["id", "creator", "raised", "cap", "finalized"],
                &["id", "creator", "raised_tao", "cap_tao", "finalized"],
                payload,
            )
        }
        Request::Contributors(id) => {
            if payload.as_array().is_some_and(Vec::is_empty) {
                return detail(
                    ctx,
                    "contributors",
                    &["crowdloan_id", "contributors"],
                    json!({"crowdloan_id": id, "contributors": []}),
                );
            }
            table(
                ctx,
                "contributors",
                &["contributor", "amount_tao"],
                &["contributor", "amount_tao"],
                payload,
            )
        }
    }
}
