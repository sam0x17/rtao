// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/commands/evm/contracts_cmd.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Deployment of bounded user-supplied compiler artifacts.

use clap::{Arg, ArgMatches, Command};
use serde_json::Map;

use crate::{
    context::AppContext,
    error::CliError,
    globals::{Tier, with_globals},
};

use super::{failure, optional, submit_evm_tx_with_backend, tao_to_wei};

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
    run_with_backend(ctx, leaf, &super::NativeEvmTx).await
}

async fn run_with_backend(
    ctx: &AppContext,
    leaf: &ArgMatches,
    backend: &dyn super::EvmTxBackend,
) -> Result<(), CliError> {
    let path = optional(leaf, "artifact_path")
        .ok_or_else(|| CliError::input("missing contract artifact"))?;
    let args = super::precompiles::arguments(leaf)?;
    let value = optional(leaf, "value_tao")
        .filter(|value| !value.is_empty())
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
    let result = submit_evm_tx_with_backend(
        ctx,
        optional(leaf, "evm_key"),
        None,
        value,
        data,
        format!("deploy {basename}"),
        optional(leaf, "rpc_url"),
        Map::new(),
        backend,
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

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::{
        application::command_tree,
        books::Books,
        commands::evm::{EvmFuture, EvmTxBackend},
        globals::Globals,
        output::{MemorySink, Output, OutputOptions, Stream},
        prompt::{PromptError, Terminal},
    };
    use rtao::evm::{Address, EvmKeyInfo, EvmTxPreview, U256};
    use secrecy::SecretString;
    use serde_json::{Value, json};
    use std::{
        path::Path,
        sync::{Arc, Mutex},
    };

    struct NoTerminal;
    impl Terminal for NoTerminal {
        fn stdin_is_tty(&self) -> bool {
            false
        }
        fn stderr_is_tty(&self) -> bool {
            false
        }
        fn read_line(&mut self, _: &str) -> Result<String, PromptError> {
            Err(PromptError::Unavailable)
        }
        fn read_secret(&mut self, _: &str) -> Result<SecretString, PromptError> {
            panic!("deployment fixture must not request private material")
        }
        fn message(&mut self, _: &str) -> Result<(), PromptError> {
            Ok(())
        }
    }

    type Prepared = (Option<Address>, U256, Vec<u8>);
    #[derive(Default)]
    struct FakeTx {
        calls: Mutex<Vec<&'static str>>,
        prepared: Mutex<Vec<Prepared>>,
    }
    impl EvmTxBackend for FakeTx {
        fn key_info<'a>(
            &'a self,
            _: &'a AppContext,
            key: Option<&str>,
        ) -> EvmFuture<'a, EvmKeyInfo> {
            let key = key.map(str::to_owned);
            Box::pin(async move {
                assert_eq!(key.as_deref(), Some("trade"));
                self.calls.lock().unwrap().push("public-key");
                Ok(EvmKeyInfo {
                    name: "trade".into(),
                    address: format!("{:#x}", Address::from([7; 20])),
                    ss58_mirror: "public-mirror".into(),
                    path: "/fixture/default/evmkeys/trade".into(),
                })
            })
        }
        fn prepare<'a>(
            &'a self,
            _: &'a AppContext,
            endpoint: Option<&str>,
            sender: Address,
            to: Option<Address>,
            value: U256,
            data: Vec<u8>,
        ) -> EvmFuture<'a, EvmTxPreview> {
            let endpoint = endpoint.map(str::to_owned);
            Box::pin(async move {
                assert_eq!(endpoint.as_deref(), Some("https://fixture.invalid"));
                self.calls.lock().unwrap().push("prepare");
                self.prepared
                    .lock()
                    .unwrap()
                    .push((to, value, data.clone()));
                Ok(EvmTxPreview {
                    sender,
                    to,
                    value_wei: value,
                    data,
                    gas: 53_000,
                    gas_price_wei: U256::from(1u64),
                    nonce: 9,
                    chain_id: 964,
                })
            })
        }
        fn submit<'a>(
            &'a self,
            _: &'a AppContext,
            _: Option<&str>,
            _: Option<&str>,
            _: EvmTxPreview,
        ) -> EvmFuture<'a, Value> {
            Box::pin(async { panic!("preview or denied confirmation must never submit") })
        }
    }

    fn fixture(amount: Option<&str>, dry: bool) -> (AppContext, ArgMatches, MemorySink) {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/evm-deploy-no-args.json");
        let mut args = vec![
            "rtao",
            "evm",
            "deploy",
            path.to_str().expect("fixture path"),
            "--evm-key",
            "trade",
            "--rpc-url",
            "https://fixture.invalid",
            "--json",
        ];
        if dry {
            args.push("--dry-run");
        }
        if let Some(amount) = amount {
            args.extend(["--value-tao", amount]);
        }
        let matches = command_tree([crate::commands::evm::command().unwrap()])
            .try_get_matches_from(args)
            .unwrap();
        let leaf = matches
            .subcommand()
            .unwrap()
            .1
            .subcommand()
            .unwrap()
            .1
            .clone();
        let mut globals =
            Globals::resolve(&matches, &Default::default(), Path::new("/fixture"), |_| {
                None
            })
            .unwrap();
        globals
            .apply_with_env(&leaf, &reserved(), |_| None)
            .unwrap();
        let sink = MemorySink::default();
        let mut ctx = AppContext::new(
            globals,
            Default::default(),
            Books::default(),
            Arc::new(Mutex::new(Box::new(NoTerminal))),
        );
        ctx.output = Output::new(
            OutputOptions {
                json_mode: true,
                ..Default::default()
            },
            Arc::new(sink.clone()),
        );
        (ctx, leaf, sink)
    }

    #[tokio::test]
    async fn omitted_empty_and_zero_values_reach_deployment_preview_as_zero() {
        for (amount, expected) in [
            (None, 0u64),
            (Some(""), 0),
            (Some("0"), 0),
            (Some("1"), 1_000_000_000_000_000_000),
        ] {
            let (ctx, leaf, sink) = fixture(amount, true);
            let backend = FakeTx::default();
            run_with_backend(&ctx, &leaf, &backend).await.unwrap();
            assert_eq!(*backend.calls.lock().unwrap(), ["public-key", "prepare"]);
            assert_eq!(
                *backend.prepared.lock().unwrap(),
                [(None, U256::from(expected), vec![0x60, 0x00])],
            );
            let output: Value = serde_json::from_str(&sink.text(Stream::Stdout).unwrap()).unwrap();
            assert_eq!(output["value_wei"], json!(expected));
            assert_eq!(output["data"], "0x6000");
        }
    }

    #[tokio::test]
    async fn empty_value_real_deployment_still_requires_confirmation() {
        let (ctx, leaf, _) = fixture(Some(""), false);
        let backend = FakeTx::default();
        let error = run_with_backend(&ctx, &leaf, &backend).await.unwrap_err();
        assert!(error.message.contains("without confirmation"));
        assert_eq!(*backend.calls.lock().unwrap(), ["public-key", "prepare"]);
        assert_eq!(
            *backend.prepared.lock().unwrap(),
            [(None, U256::from(0u64), vec![0x60, 0x00])],
        );
    }
    #[tokio::test]
    async fn nonempty_invalid_or_overflowing_deploy_values_still_refuse_before_preparation() {
        for amount in [" ", "not-an-amount", "18446744073.709551616"] {
            let (ctx, leaf, sink) = fixture(Some(amount), true);
            let backend = FakeTx::default();
            let error = run_with_backend(&ctx, &leaf, &backend).await.unwrap_err();
            assert_eq!(error.code, 2);
            assert!(backend.calls.lock().unwrap().is_empty());
            assert!(backend.prepared.lock().unwrap().is_empty());
            assert!(sink.text(Stream::Stdout).unwrap().is_empty());
        }
    }
}
