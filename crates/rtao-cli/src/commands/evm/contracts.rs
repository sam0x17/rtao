// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/commands/evm/contracts_cmd.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Deployment of bounded user-supplied compiler artifacts.

use clap::{Arg, ArgMatches, Command};
use serde_json::Map;

use crate::{
    context::AppContext,
    error::CliError,
    globals::{Tier, with_globals},
};

use super::{failure, optional, submit_evm_tx, tao_to_wei};

/// Build the pinned deployment command.
pub fn command() -> Command {
    with_globals(
        Command::new("deploy")
            .about("Deploy a Hardhat/Foundry artifact or raw .bin bytecode.")
            .arg(Arg::new("artifact_path").required(true))
            .arg(
                Arg::new("args")
                    .num_args(0..)
                    .allow_negative_numbers(true)
                    .help("Constructor arguments in declaration order."),
            )
            .arg(Arg::new("value_tao").long("value-tao"))
            .arg(Arg::new("evm_key").long("evm-key"))
            .arg(Arg::new("rpc_url").long("rpc-url")),
        Tier::Tx,
    )
}

/// Argument names reserved from wallet target aliases.
pub fn reserved() -> Vec<&'static str> {
    vec!["artifact_path", "args", "value_tao", "evm_key", "rpc_url"]
}

/// Load and encode before resolving a key or contacting an EVM endpoint.
pub async fn run(ctx: &AppContext, leaf: &ArgMatches) -> Result<(), CliError> {
    let path = optional(leaf, "artifact_path")
        .ok_or_else(|| CliError::input("missing contract artifact"))?;
    let args = super::precompiles::arguments(leaf)?;
    let value = optional(leaf, "value_tao")
        .map(tao_to_wei)
        .transpose()?
        .unwrap_or_default();
    let path_owned = path.to_owned();
    let data = rtao::core::offload(move || {
        let artifact = rtao::evm::load_artifact(path_owned)
            .map_err(|error| CliError::input(error.to_string()))?;
        let encoded = rtao::evm::encode_deploy(&artifact, &args)
            .map_err(|error| CliError::input(error.to_string()))?;
        hex::decode(encoded.strip_prefix("0x").unwrap_or(&encoded)).map_err(failure)
    })
    .await
    .map_err(failure)??;
    let basename = std::path::Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_else(|| path.into());
    let result = submit_evm_tx(
        ctx,
        optional(leaf, "evm_key"),
        None,
        value,
        data,
        format!("deploy {basename}"),
        optional(leaf, "rpc_url"),
        Map::new(),
    )
    .await?;
    if let Some(address) = result
        .as_ref()
        .and_then(|value| value.get("contract_address"))
        .and_then(serde_json::Value::as_str)
    {
        let mirror = rtao::evm::h160_to_ss58(address).map_err(failure)?;
        ctx.output.message(&format!(
            "contract mirror: {mirror} — this ss58 account is the contract's identity for native TAO and precompile calls"
        )).map_err(failure)?;
    }
    Ok(())
}
