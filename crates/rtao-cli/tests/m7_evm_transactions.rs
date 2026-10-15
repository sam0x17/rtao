//! Socket-free EVM CLI fixtures exercise the real public-preview authorization boundary.

#![cfg(feature = "evm")]

use rtao::evm::{Address, EvmKeyInfo, EvmTxPreview, U256};
use rtao_cli::{
    application,
    books::Books,
    commands::evm::{self, EvmFuture, EvmTxBackend, submit_evm_tx_with_backend},
    context::AppContext,
    error::CliError,
    globals::Globals,
    output::{MemorySink, Output, OutputOptions, Stream},
    prompt::{PromptError, Terminal},
};
use secrecy::SecretString;
use serde_json::{Map, Value, json};
use std::{
    error::Error,
    path::PathBuf,
    sync::{Arc, Mutex},
};

type TestResult = Result<(), Box<dyn Error>>;
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
        Err(PromptError::Unavailable)
    }
    fn message(&mut self, _: &str) -> Result<(), PromptError> {
        Ok(())
    }
}
fn fixture(dry: bool, yes: bool) -> Result<(AppContext, MemorySink), Box<dyn Error>> {
    let tree = application::command_tree([evm::command()?]);
    let matches = tree.try_get_matches_from([
        "rtao",
        "evm",
        "send",
        "--to",
        "0x3535353535353535353535353535353535353535",
        "--amount-tao",
        "1",
    ])?;
    let mut globals =
        Globals::resolve(&matches, &Map::new(), &PathBuf::from("/fixture"), |_| None)?;
    globals.json = true;
    globals.dry_run = dry;
    globals.assume_yes = yes;
    let sink = MemorySink::default();
    let mut ctx = AppContext::new(
        globals,
        Map::new(),
        Books::default(),
        Arc::new(Mutex::new(Box::new(NoTerminal))),
    );
    ctx.output = Output::with_sink(
        OutputOptions {
            json_mode: true,
            quiet: false,
            network: None,
        },
        Arc::new(sink.clone()),
    );
    Ok((ctx, sink))
}
#[derive(Default)]
struct Fake {
    calls: Mutex<Vec<&'static str>>,
    alter_data: bool,
    unresolved: bool,
}
impl EvmTxBackend for Fake {
    fn key_info<'a>(&'a self, _: &'a AppContext, _: Option<&'a str>) -> EvmFuture<'a, EvmKeyInfo> {
        Box::pin(async move {
            self.calls
                .lock()
                .map_err(|_| CliError::failure("poisoned"))?
                .push("public-key");
            Ok(EvmKeyInfo {
                name: "default".into(),
                address: format!("{:#x}", Address::from([7; 20])),
                ss58_mirror: "public-mirror".into(),
                path: "/fixture/default/evmkeys/default".into(),
            })
        })
    }
    fn prepare<'a>(
        &'a self,
        _: &'a AppContext,
        _: Option<&'a str>,
        sender: Address,
        to: Option<Address>,
        value: U256,
        mut data: Vec<u8>,
    ) -> EvmFuture<'a, EvmTxPreview> {
        Box::pin(async move {
            self.calls
                .lock()
                .map_err(|_| CliError::failure("poisoned"))?
                .push("prepare");
            if self.alter_data {
                data.push(42);
            }
            Ok(EvmTxPreview {
                sender,
                to,
                value_wei: value,
                data,
                gas: 21_000,
                gas_price_wei: U256::from(20_000_000_000u64),
                nonce: 9,
                chain_id: 964,
            })
        })
    }
    fn submit<'a>(
        &'a self,
        _: &'a AppContext,
        _: Option<&'a str>,
        _: Option<&'a str>,
        _: EvmTxPreview,
    ) -> EvmFuture<'a, Value> {
        Box::pin(async move {
            self.calls
                .lock()
                .map_err(|_| CliError::failure("poisoned"))?
                .push("unlock-sign-submit-once");
            if self.unresolved {
                return Err(
                    CliError::failure("transaction 0xabcdef has unresolved fate")
                        .with_help("inspect receipt for 0xabcdef; do not resend automatically"),
                );
            }
            Ok(json!({"tx_hash":"0xabcdef","success":true,"block_number":10,"gas_used":21000}))
        })
    }
}
async fn invoke(ctx: &AppContext, fake: &Fake) -> Result<Option<Value>, CliError> {
    submit_evm_tx_with_backend(
        ctx,
        None,
        Some("0x3535353535353535353535353535353535353535"),
        U256::from(1_000_000_000_000_000_000u64),
        vec![1, 2],
        "send one TAO".into(),
        None,
        Map::new(),
        fake,
    )
    .await
}
#[tokio::test]
async fn dry_run_emits_exact_public_preview_without_private_boundary() -> TestResult {
    let (ctx, sink) = fixture(true, false)?;
    let fake = Fake::default();
    assert!(invoke(&ctx, &fake).await?.is_none());
    assert_eq!(
        *fake.calls.lock().map_err(|_| "poisoned")?,
        ["public-key", "prepare"]
    );
    let payload: Value = serde_json::from_str(&sink.text(Stream::Stdout)?)?;
    assert_eq!(payload["value_wei"], json!(1_000_000_000_000_000_000u64));
    assert_eq!(payload["value_tao"], "1");
    assert_eq!(payload["max_fee_tao"], "0.00042");
    assert_eq!(payload["data"], "0x0102");
    Ok(())
}
#[tokio::test]
async fn noninteractive_real_run_refuses_before_private_boundary() -> TestResult {
    let (ctx, _) = fixture(false, false)?;
    let fake = Fake::default();
    let error = invoke(&ctx, &fake).await.err().ok_or("expected refusal")?;
    assert!(error.message.contains("without confirmation"));
    assert_eq!(
        *fake.calls.lock().map_err(|_| "poisoned")?,
        ["public-key", "prepare"]
    );
    Ok(())
}
#[tokio::test]
async fn confirmed_run_submits_once_and_renders_python_result_shape() -> TestResult {
    let (ctx, sink) = fixture(false, true)?;
    let fake = Fake::default();
    let result = invoke(&ctx, &fake).await?.ok_or("missing result")?;
    assert_eq!(
        *fake.calls.lock().map_err(|_| "poisoned")?,
        ["public-key", "prepare", "unlock-sign-submit-once"]
    );
    assert_eq!(result["from"], format!("{:#x}", Address::from([7; 20])));
    assert_eq!(result["to"], "0x3535353535353535353535353535353535353535");
    assert_eq!(
        serde_json::from_str::<Value>(&sink.text(Stream::Stdout)?)?,
        result
    );
    Ok(())
}
#[tokio::test]
async fn changed_calldata_refuses_even_with_yes() -> TestResult {
    let (ctx, _) = fixture(false, true)?;
    let fake = Fake {
        alter_data: true,
        ..Default::default()
    };
    assert!(
        invoke(&ctx, &fake)
            .await
            .err()
            .ok_or("expected changed-data refusal")?
            .message
            .contains("calldata")
    );
    assert_eq!(
        *fake.calls.lock().map_err(|_| "poisoned")?,
        ["public-key", "prepare"]
    );
    Ok(())
}
#[tokio::test]
async fn unresolved_result_keeps_recovery_help_and_never_retries() -> TestResult {
    let (ctx, _) = fixture(false, true)?;
    let fake = Fake {
        unresolved: true,
        ..Default::default()
    };
    let error = invoke(&ctx, &fake)
        .await
        .err()
        .ok_or("expected unresolved result")?;
    assert!(error.message.contains("0xabcdef"));
    assert!(error.help.unwrap_or_default().contains("do not resend"));
    assert_eq!(
        *fake.calls.lock().map_err(|_| "poisoned")?,
        ["public-key", "prepare", "unlock-sign-submit-once"]
    );
    Ok(())
}
#[tokio::test]
async fn native_value_overflow_refuses_before_any_public_or_secret_io() -> TestResult {
    let (ctx, _) = fixture(false, true)?;
    let fake = Fake::default();
    let value = (U256::from(u64::MAX) + U256::from(1)) * U256::from(1_000_000_000u64);
    let result = submit_evm_tx_with_backend(
        &ctx,
        None,
        Some("0x3535353535353535353535353535353535353535"),
        value,
        Vec::new(),
        "overflow".into(),
        None,
        Map::new(),
        &fake,
    )
    .await;
    assert!(result.is_err());
    assert!(fake.calls.lock().map_err(|_| "poisoned")?.is_empty());
    Ok(())
}
#[test]
fn command_tree_has_all_twenty_five_pinned_routes() -> TestResult {
    fn leaves(command: &clap::Command) -> usize {
        let children = command.get_subcommands().collect::<Vec<_>>();
        if children.is_empty() {
            1
        } else {
            children.into_iter().map(leaves).sum()
        }
    }
    let tree = evm::command()?;
    assert_eq!(leaves(&tree), 25);
    for args in [
        vec![
            "evm",
            "stake",
            "remove",
            "--netuid",
            "1",
            "--amount-alpha",
            "1",
        ],
        vec!["evm", "key", "export", "--private-key"],
        vec!["evm", "setup-localnet", "--chain-id", "945"],
        vec!["evm", "associate", "--netuid", "1"],
    ] {
        tree.clone().try_get_matches_from(args)?;
    }
    Ok(())
}
