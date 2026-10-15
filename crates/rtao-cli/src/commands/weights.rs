// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/commands/weights.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Validator weight commands routed through the SDK's semantic intents.

use super::generated::TxRequest;
use crate::{
    error::CliError,
    globals::{Tier, with_globals},
    prompt::python_strip,
};
use clap::{Arg, ArgAction, ArgMatches, Command};
use rtao::client::config::{MAX_JSON_FILE_BYTES, read_bounded_json_file};
use rtao::primitives::pyfmt;
use serde_json::{Map, Value, json};
use std::path::Path;

fn csv(id: &'static str, help: &'static str) -> Arg {
    Arg::new(id).long(id).allow_hyphen_values(true).help(help)
}

fn subnet() -> Arg {
    Arg::new("netuid")
        .long("netuid")
        .required(true)
        .value_parser(clap::value_parser!(u16))
        .help("Subnet whose miners the weights score.")
}

fn version_key() -> Arg {
    Arg::new("version_key")
        .long("version-key")
        .default_value("0")
        .value_parser(clap::value_parser!(u64))
        .help("Weights version key; leave 0 unless the subnet owner requires a value.")
}

fn period() -> Arg {
    Arg::new("__period")
        .long("period")
        .value_parser(clap::value_parser!(u32))
        .help("Mortal transaction period in blocks (rtao addition).")
}

fn weighted(name: &'static str, description: &'static str) -> Command {
    with_globals(
        Command::new(name)
            .about(description)
            .arg(subnet())
            .arg(csv(
                "uids",
                "Comma-separated miner UIDs, parallel to --weights.",
            ))
            .arg(csv(
                "weights",
                "Comma-separated relative weights, parallel to --uids.",
            ))
            .arg(
                Arg::new("raw_u16")
                    .long("raw-u16")
                    .action(ArgAction::SetTrue)
                    .help("Preserve exact integer weights (Null consensus only)."),
            )
            .arg(
                Arg::new("weights_file")
                    .long("weights-file")
                    .value_parser(clap::value_parser!(std::path::PathBuf))
                    .help("Regular JSON file mapping UID to weight (at most 16 MiB); supports full 2,500-UID rows."),
            )
            .arg(
                Arg::new("mechid")
                    .long("mechid")
                    .default_value("0")
                    .value_parser(clap::value_parser!(u8))
                    .help("Mechanism index within the subnet; 0 is the default."),
            )
            .arg(version_key())
            .arg(period()),
        Tier::Tx,
    )
}

/// Build set, forced timelock commit, and legacy salt-based reveal commands.
pub fn command() -> Command {
    Command::new("weights")
        .about("Validator weight commands.")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(weighted(
            "set",
            "Set weights using the subnet's plaintext or timelocked commit path.",
        ))
        .subcommand(weighted(
            "commit",
            "Force a timelocked commit; the chain reveals it automatically.",
        ))
        .subcommand(with_globals(
            Command::new("reveal")
                .about("Reveal a legacy salt-based commit using its original inputs.")
                .arg(subnet())
                .arg(
                    csv("uids", "Comma-separated miner UIDs, exactly as committed.").required(true),
                )
                .arg(
                    csv("weights", "Comma-separated weights, exactly as committed.").required(true),
                )
                .arg(csv("salt", "Comma-separated salt values used at commit time.").required(true))
                .arg(version_key())
                .arg(period()),
            Tier::Tx,
        ))
}

fn text<'a>(matches: &'a ArgMatches, name: &str) -> Result<&'a str, CliError> {
    matches
        .get_one::<String>(name)
        .map(String::as_str)
        .ok_or_else(|| CliError::input(format!("missing --{}", name.replace('_', "-"))))
}

fn integers(raw: &str) -> Result<Value, CliError> {
    check_input_length(raw)?;
    raw.split(',')
        .map(python_strip)
        .filter(|part| !part.is_empty())
        .map(|part| {
            pyfmt::parse_integer(part)
                .and_then(|number| u16::try_from(number).ok())
                .map(Value::from)
                .ok_or_else(|| CliError::input("Invalid comma-separated u16 integers."))
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Value::Array)
}

fn floats(raw: &str) -> Result<Value, CliError> {
    check_input_length(raw)?;
    raw.split(',')
        .map(python_strip)
        .filter(|part| !part.is_empty())
        .map(|part| {
            let value = pyfmt::parse_float(part)
                .ok_or_else(|| CliError::input("Invalid comma-separated UIDs or weights."))?;
            if !value.is_finite() {
                return Err(CliError::input("Weights must be finite and non-negative."));
            }
            Ok(json!(value))
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Value::Array)
}

fn check_input_length(raw: &str) -> Result<(), CliError> {
    if raw.len() > MAX_JSON_FILE_BYTES {
        return Err(CliError::input("Weights input exceeds 16 MiB limit"));
    }
    Ok(())
}

fn inputs(
    matches: &ArgMatches,
    read: impl FnOnce(&Path) -> Result<String, std::io::Error>,
) -> Result<Map<String, Value>, CliError> {
    let uids = matches.get_one::<String>("uids");
    let weights = matches.get_one::<String>("weights");
    let raw = matches.get_flag("raw_u16");
    let mut args = Map::new();
    if let Some(path) = matches.get_one::<std::path::PathBuf>("weights_file") {
        if uids.is_some() || weights.is_some() {
            return Err(CliError::input(
                "Use --weights-file or --uids/--weights, not both.",
            ));
        }
        let contents = read(path)
            .map_err(|error| CliError::input(format!("Cannot read weights file: {error}")))?;
        check_input_length(&contents)?;
        let data: Value = serde_json::from_str(&contents)
            .map_err(|error| CliError::input(format!("Cannot read weights file: {error}")))?;
        if !data.is_object() {
            return Err(CliError::input(
                "Weights file must contain a UID-to-weight JSON object.",
            ));
        }
        args.insert("weights".into(), data);
    } else {
        let (Some(uids), Some(weights)) = (uids, weights) else {
            return Err(CliError::input(
                "Provide --weights-file or both --uids and --weights.",
            ));
        };
        args.insert("uids".into(), integers(uids)?);
        args.insert(
            "weights".into(),
            if raw {
                integers(weights)?
            } else {
                floats(weights)?
            },
        );
    }
    args.insert("raw_u16".into(), json!(raw));
    args.insert(
        "mechid".into(),
        json!(matches.get_one::<u8>("mechid").copied().unwrap_or(0)),
    );
    Ok(args)
}

/// Parse a weights group before connecting; execution uses `generated::execute_tx`.
pub fn request(matches: &ArgMatches) -> Result<TxRequest, CliError> {
    request_with_reader(matches, |path| {
        let bytes = read_bounded_json_file(path)?;
        String::from_utf8(bytes)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
    })
}

/// Parse with an injected file reader, preserving file and CSV exclusivity.
///
/// Input conversion delegates canonicalization, UID duplicate detection, native
/// widths, and defaults to the same SDK interface as generated transactions.
/// The returned file text and total inline inputs are limited to 16 MiB; an
/// injected reader must bound acquisition itself, as [`request`] does.
pub fn request_with_reader(
    matches: &ArgMatches,
    read: impl FnOnce(&Path) -> Result<String, std::io::Error>,
) -> Result<TxRequest, CliError> {
    let (name, matches) = matches
        .subcommand()
        .ok_or_else(|| CliError::input("missing weights command"))?;
    // Check the complete inline input before splitting, allocating converted
    // arrays, looking up public keys or making any transport request.
    let mut inline_bytes = 0usize;
    for field in ["uids", "weights", "salt"] {
        if let Some(value) = matches.try_get_one::<String>(field).ok().flatten() {
            inline_bytes = inline_bytes.saturating_add(value.len());
        }
    }
    if inline_bytes > MAX_JSON_FILE_BYTES {
        return Err(CliError::input("Weights input exceeds 16 MiB limit"));
    }
    let (op, mut args) = match name {
        "set" => ("set_weights", inputs(matches, read)?),
        "commit" => ("commit_weights", inputs(matches, read)?),
        "reveal" => (
            "reveal_weights",
            Map::from_iter([
                ("uids".into(), integers(text(matches, "uids")?)?),
                ("weights".into(), floats(text(matches, "weights")?)?),
                ("salt".into(), integers(text(matches, "salt")?)?),
            ]),
        ),
        _ => return Err(CliError::input("unknown weights command")),
    };
    args.insert(
        "netuid".into(),
        json!(
            matches
                .get_one::<u16>("netuid")
                .ok_or_else(|| CliError::input("missing --netuid"))?
        ),
    );
    args.insert(
        "version_key".into(),
        json!(matches.get_one::<u64>("version_key").copied().unwrap_or(0)),
    );
    let request = TxRequest {
        op,
        args,
        period: matches.get_one::<u32>("__period").copied(),
    };
    request.intent()?;
    Ok(request)
}
