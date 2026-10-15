// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/commands/axon.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Publish or reset an axon endpoint through semantic SDK intents.

use super::generated::TxRequest;
use crate::{
    error::CliError,
    globals::{Tier, with_globals},
};
use clap::{Arg, ArgAction, ArgMatches, Command};
use serde_json::{Map, json};

fn subnet() -> Arg {
    Arg::new("netuid")
        .long("netuid")
        .required(true)
        .value_parser(clap::value_parser!(u16))
        .help("Subnet on which to publish the endpoint.")
}

fn period() -> Arg {
    Arg::new("__period")
        .long("period")
        .value_parser(clap::value_parser!(u32))
        .help("Mortal transaction period in blocks (rtao addition).")
}

/// Build endpoint publishing commands. These commands only write chain data.
pub fn command() -> Command {
    Command::new("axon")
        .about("Axon serving commands.")
        .long_about(format!(
            "Axon serving commands.\n\nGuide: {}/guides/mining",
            rtao_catalog::py::DOCS_URL
        ))
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(with_globals(
            Command::new("set")
                .about("Publish an axon endpoint for the wallet hotkey.")
                .long_about(
                    "Publish an axon endpoint for the wallet hotkey.\n\nThis records connection info as chain data only; it does not start a server. Signed by the hotkey, which must be registered on the subnet.",
                )
                .arg(subnet())
                .arg(
                    Arg::new("ip")
                        .long("ip")
                        .required(true)
                        .help("Public IPv4 or IPv6 address of the endpoint, in standard dotted or colon notation."),
                )
                .arg(
                    Arg::new("port")
                        .long("port")
                        .required(true)
                        .value_parser(clap::value_parser!(u16))
                        .help("TCP port the endpoint listens on."),
                )
                .arg(
                    Arg::new("tls")
                        .long("tls")
                        .action(ArgAction::SetTrue)
                        .help("Also publish a TLS certificate for inter-neuron TLS."),
                )
                .arg(
                    Arg::new("certificate")
                        .long("certificate")
                        .default_value("")
                        .help("Neuron certificate as 0x-prefixed hex: 1 algorithm byte followed by up to 64 bytes of public key. Not an X.509 certificate; other formats fail to decode on chain."),
                )
                .arg(period()),
            Tier::Tx,
        ))
        .subcommand(with_globals(
            Command::new("reset")
                .about("Reset (stop serving) the wallet hotkey's axon on a subnet.")
                .arg(subnet().help("Subnet whose published axon endpoint to clear."))
                .arg(period()),
            Tier::Tx,
        ))
}

/// Parse an axon group before connecting; execution uses `generated::execute_tx`.
///
/// TLS is selected only by `--tls`, as in Python; a certificate supplied without
/// it is ignored. Address and certificate checks remain in the SDK builder and
/// therefore run before signing, using the usual review and refusal path.
pub fn request(matches: &ArgMatches) -> Result<TxRequest, CliError> {
    let (name, matches) = matches
        .subcommand()
        .ok_or_else(|| CliError::input("missing axon command"))?;
    let mut args = Map::new();
    args.insert(
        "netuid".into(),
        json!(
            matches
                .get_one::<u16>("netuid")
                .ok_or_else(|| CliError::input("missing --netuid"))?
        ),
    );
    let op = match name {
        "set" => {
            args.insert(
                "ip".into(),
                json!(
                    matches
                        .get_one::<String>("ip")
                        .ok_or_else(|| CliError::input("missing --ip"))?
                ),
            );
            args.insert(
                "port".into(),
                json!(
                    matches
                        .get_one::<u16>("port")
                        .ok_or_else(|| CliError::input("missing --port"))?
                ),
            );
            if matches.get_flag("tls") {
                args.insert(
                    "certificate".into(),
                    json!(
                        matches
                            .get_one::<String>("certificate")
                            .map_or("", String::as_str)
                    ),
                );
                "serve_axon_tls"
            } else {
                "serve_axon"
            }
        }
        "reset" => "reset_axon",
        _ => return Err(CliError::input("unknown axon command")),
    };
    let request = TxRequest {
        op,
        args,
        period: matches.get_one::<u32>("__period").copied(),
    };
    request.intent()?;
    Ok(request)
}
