// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/commands/config.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Persistent CLI defaults through the same injected store as the application.

use crate::{
    error::CliError,
    globals::{Tier, with_globals},
    output::Output,
};
use clap::{Arg, ArgMatches, Command};
use rtao::client::config::{ConfigFile, Store, settable};
use serde_json::{Value, json};

/// The five persistent config commands, with read-tier global overrides.
pub fn command() -> Command {
    Command::new("config")
        .about("Read and write persistent CLI config.")
        .arg_required_else_help(true)
        .subcommand_required(true)
        .subcommands([
            with_globals(
                Command::new("set")
                    .about("Set a persistent default, e.g. `rtao config set network test`.")
                    .arg(Arg::new("key").required(true).help(key_help()))
                    .arg(Arg::new("value").required(true).help("Value to store.")),
                Tier::Read,
            ),
            with_globals(
                Command::new("get")
                    .about("Show one stored value, or the entire config.")
                    .arg(Arg::new("key").help("Key to read; omit to show the whole config.")),
                Tier::Read,
            ),
            with_globals(
                Command::new("unset").about("Remove a stored value.").arg(
                    Arg::new("key")
                        .required(true)
                        .help("Key to remove (reverts to env/default)."),
                ),
                Tier::Read,
            ),
            with_globals(
                Command::new("clear")
                    .about("Clear all stored config values.")
                    .long_about(
                        "Clear all stored config values.\n\nRemoves every key from the config file. Subsequent commands fall back to environment variables and built-in defaults.",
                    ),
                Tier::Read,
            ),
            with_globals(
                Command::new("path").about("Print the config file path."),
                Tier::Read,
            ),
        ])
}

fn key_names() -> String {
    let mut keys: Vec<_> = settable().iter().map(|(key, _)| *key).collect();
    keys.sort_unstable();
    keys.join(", ")
}

fn key_help() -> String {
    format!("One of: {}.", key_names())
}

fn argument<'a>(matches: &'a ArgMatches, name: &str) -> Result<&'a str, CliError> {
    matches
        .get_one::<String>(name)
        .map(String::as_str)
        .ok_or_else(|| CliError::input(format!("missing {name}")))
}

/// Execute a parsed config family against explicit paths and output streams.
///
/// `matches` is the `config` group's matches. This function does not resolve
/// process environment or home paths, connect to a node, or emit diagnostics
/// twice: the application renders a returned [`CliError`] once.
pub async fn run(store: &Store, output: &Output, matches: &ArgMatches) -> Result<(), CliError> {
    let Some((name, matches)) = matches.subcommand() else {
        return Err(CliError::input("missing config command"));
    };
    let path = store.paths().path(ConfigFile::Config).to_string_lossy();
    match name {
        "set" => {
            let key = argument(matches, "key")?;
            let stored = store
                .set_value(key, argument(matches, "value")?)
                .await
                .map_err(|error| CliError::failure(error.to_string()))?;
            output.detail(
                Some("set"),
                &[
                    ("key".into(), json!(key)),
                    ("value".into(), stored),
                    ("path".into(), json!(path)),
                ],
                None,
            )
        }
        "get" => {
            // Python loads before checking the key, including corrupt-file recovery.
            let data = store
                .load()
                .await
                .map_err(|error| CliError::failure(error.to_string()))?;
            if let Some(key) = matches.get_one::<String>("key") {
                if !settable().iter().any(|(candidate, _)| candidate == key) {
                    return Err(CliError::failure(format!(
                        "unknown config key {}",
                        rtao_rpc::policy::python_string_repr(key)
                    ))
                    .with_help(format!("settable keys: {}", key_names())));
                }
                output.value(data.get(key).unwrap_or(&Value::Null))
            } else {
                let fields: Vec<_> = if data.is_empty() {
                    vec![("(empty)".into(), json!(""))]
                } else {
                    data.into_iter().collect()
                };
                output.detail(Some(&format!("config ({path})")), &fields, None)
            }
        }
        "unset" => {
            let key = argument(matches, "key")?;
            let existed = store
                .unset_value(key)
                .await
                .map_err(|error| CliError::failure(error.to_string()))?;
            output.detail(
                Some("unset"),
                &[
                    ("key".into(), json!(key)),
                    ("existed".into(), json!(existed)),
                ],
                None,
            )
        }
        "clear" => {
            // Retain Python's per-key mutation and empty-file behavior. An empty
            // or solely unknown-key config is not rewritten by `clear`.
            let data = store
                .load()
                .await
                .map_err(|error| CliError::failure(error.to_string()))?;
            for key in data.keys() {
                store
                    .unset_value(key)
                    .await
                    .map_err(|error| CliError::failure(error.to_string()))?;
            }
            output.detail(
                Some("cleared config"),
                &[("path".into(), json!(path))],
                None,
            )
        }
        "path" => output.value(&path),
        _ => return Err(CliError::input(format!("unknown config command {name:?}"))),
    }
    .map_err(|error| CliError::failure(error.to_string()))
}
