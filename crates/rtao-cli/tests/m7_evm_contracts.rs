//! EVM ABI and contract CLI fixtures; reads use a fake transport and no keyfiles.

#![cfg(feature = "evm")]

use std::{
    error::Error,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use rtao_cli::{
    application,
    books::Books,
    commands::evm::{
        contracts,
        precompiles::{self, ReadBackend, ReadFuture},
    },
    context::AppContext,
    error::CliError,
    globals::Globals,
    output::{MemorySink, Output, OutputOptions, Stream},
    prompt::{PromptError, Terminal},
};
use secrecy::SecretString;
use serde_json::{Value, json};

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

fn tree() -> clap::Command {
    application::command_tree([clap::Command::new("evm")
        .subcommands(precompiles::commands())
        .subcommand(contracts::command())])
}

struct Fixture {
    matches: clap::ArgMatches,
    ctx: AppContext,
    sink: MemorySink,
}
impl Fixture {
    fn new(args: &[&str]) -> Result<Self, Box<dyn Error>> {
        let matches = tree().try_get_matches_from(args)?;
        let home = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let mut globals = Globals::resolve(&matches, &Default::default(), &home, |_| None)?;
        let leaf = matches
            .subcommand()
            .and_then(|(_, group)| group.subcommand())
            .ok_or("missing leaf")?
            .1;
        globals.apply_with_env(leaf, &[], |_| None)?;
        let sink = MemorySink::default();
        let mut ctx = AppContext::new(
            globals,
            Default::default(),
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
        Ok(Self { matches, ctx, sink })
    }
    fn leaf(&self) -> Result<(&str, &clap::ArgMatches), CliError> {
        self.matches
            .subcommand()
            .and_then(|(_, group)| group.subcommand())
            .ok_or_else(|| CliError::input("missing leaf"))
    }
    async fn run(&self, backend: &FakeRead) -> Result<(), CliError> {
        let (name, leaf) = self.leaf()?;
        if name == "deploy" {
            contracts::run(&self.ctx, leaf).await
        } else {
            precompiles::run_with_backend(&self.ctx, name, leaf, backend).await
        }
    }
    fn json(&self) -> Result<Value, Box<dyn Error>> {
        Ok(serde_json::from_slice(&self.sink.bytes(Stream::Stdout)?)?)
    }
}

#[derive(Default)]
struct FakeRead {
    requests: Mutex<Vec<(Option<String>, Value)>>,
    result: Vec<u8>,
    fail: bool,
}
impl ReadBackend for FakeRead {
    fn eth_call<'a>(
        &'a self,
        _: &'a AppContext,
        rpc_url: Option<&'a str>,
        tx: Value,
    ) -> ReadFuture<'a> {
        Box::pin(async move {
            self.requests
                .lock()
                .map_err(|_| CliError::failure("fixture poisoned"))?
                .push((rpc_url.map(str::to_owned), tx));
            if self.fail {
                Err(CliError::failure("fixture reverted"))
            } else {
                Ok(self.result.clone())
            }
        })
    }
}

#[tokio::test]
async fn catalog_keeps_native_index_order_and_explicit_machine_deprecation() -> TestResult {
    let fixture = Fixture::new(&["rtao", "evm", "precompiles", "--json"])?;
    let backend = FakeRead::default();
    fixture.run(&backend).await?;
    let value = fixture.json()?;
    let records = value.as_array().ok_or("catalog not an array")?;
    assert_eq!(records.len(), 21);
    assert_eq!(records[0]["name"], "ed25519-verify");
    assert_eq!(
        records[0]["address"],
        "0x0000000000000000000000000000000000000402"
    );
    assert_eq!(records[1]["name"], "sr25519-verify");
    let staking = records
        .iter()
        .find(|row| row["name"] == "staking")
        .ok_or("missing staking")?;
    assert_eq!(staking["deprecated"], true);
    assert_eq!(
        staking["address"],
        "0x0000000000000000000000000000000000000801"
    );
    assert_eq!(staking.as_object().ok_or("bad record")?.len(), 4);
    assert!(backend.requests.lock().map_err(|_| "poisoned")?.is_empty());
    Ok(())
}

#[tokio::test]
async fn abi_command_preserves_every_original_array_entry_and_order() -> TestResult {
    let fixture = Fixture::new(&["rtao", "evm", "abi", "staking-v2", "--json"])?;
    let backend = FakeRead::default();
    fixture.run(&backend).await?;
    let expected: Value = serde_json::from_str(include_str!(
        "../../../vendor/upstream/evm/abi/stakingV2.json"
    ))?;
    assert_eq!(
        fixture.json()?,
        json!({"address":"0x0000000000000000000000000000000000000805","abi":expected})
    );
    assert!(backend.requests.lock().map_err(|_| "poisoned")?.is_empty());
    Ok(())
}

#[tokio::test]
async fn view_call_needs_no_key_and_uses_only_read_transport_with_exact_argument_word() -> TestResult
{
    let fixture = Fixture::new(&[
        "rtao",
        "evm",
        "call",
        "metagraph",
        "getUidCount",
        "1",
        "--rpc-url",
        "https://fixture.invalid",
        "--evm-key",
        "does-not-exist",
        "--json",
    ])?;
    let mut raw = vec![0; 32];
    raw[31] = 37;
    let backend = FakeRead {
        result: raw,
        ..Default::default()
    };
    fixture.run(&backend).await?;
    assert_eq!(fixture.json()?, json!(37));
    let requests = backend.requests.lock().map_err(|_| "poisoned")?;
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].0.as_deref(), Some("https://fixture.invalid"));
    assert_eq!(
        requests[0].1["to"],
        "0x0000000000000000000000000000000000000802"
    );
    assert_eq!(requests[0].1.as_object().ok_or("not an object")?.len(), 2);
    let data = requests[0].1["data"].as_str().ok_or("not calldata")?;
    assert_eq!(data.len(), 2 + 8 + 64);
    assert_eq!(&data[10..], format!("{:064x}", 1));
    assert!(fixture.sink.bytes(Stream::Stderr)?.is_empty());
    Ok(())
}

#[tokio::test]
async fn empty_view_result_is_an_array_and_rpc_failure_is_not_retried() -> TestResult {
    for fail in [false, true] {
        let fixture =
            Fixture::new(&["rtao", "evm", "call", "timestamp", "getTimestamp", "--json"])?;
        let backend = FakeRead {
            fail,
            ..Default::default()
        };
        let result = fixture.run(&backend).await;
        if fail {
            assert!(result.is_err());
            assert!(fixture.sink.bytes(Stream::Stdout)?.is_empty());
        } else {
            result?;
            assert_eq!(fixture.json()?, json!([]));
        }
        assert_eq!(backend.requests.lock().map_err(|_| "poisoned")?.len(), 1);
    }
    Ok(())
}

#[tokio::test]
async fn function_listing_keeps_python_columns_and_deprecated_note_without_rpc() -> TestResult {
    let fixture = Fixture::new(&["rtao", "evm", "call", "metagraph", "--json"])?;
    let backend = FakeRead::default();
    fixture.run(&backend).await?;
    let value = fixture.json()?;
    let rows = value.as_array().ok_or("not rows")?;
    let row = rows
        .iter()
        .find(|row| row["function"] == "getRank")
        .ok_or("no getRank")?;
    assert_eq!(
        row,
        &json!({"function":"getRank","inputs":"uint16 netuid, uint16 uid","outputs":"uint16","mutability":"view (deprecated: always returns 0 on chain; derive standing from getIncentive)"})
    );
    assert!(backend.requests.lock().map_err(|_| "poisoned")?.is_empty());
    Ok(())
}

#[tokio::test]
async fn invalid_native_amount_function_and_missing_interface_fail_before_rpc_or_key_resolution()
-> TestResult {
    let hotkey = format!("0x{}", "11".repeat(32));
    for args in [
        vec![
            "rtao",
            "evm",
            "call",
            "staking-v2",
            "addStake",
            hotkey.as_str(),
            "18446744073709551616",
            "1",
            "--evm-key",
            "does-not-exist",
        ],
        vec!["rtao", "evm", "call", "metagraph", "notAFunction"],
        vec![
            "rtao",
            "evm",
            "call",
            "0x1111111111111111111111111111111111111111",
            "balanceOf",
        ],
    ] {
        let fixture = Fixture::new(&args)?;
        let backend = FakeRead::default();
        let error = fixture
            .run(&backend)
            .await
            .err()
            .ok_or("unexpected success")?;
        assert_eq!(error.code, 2);
        assert!(!error.message.contains("does-not-exist"));
        assert!(backend.requests.lock().map_err(|_| "poisoned")?.is_empty());
    }
    Ok(())
}

#[tokio::test]
async fn deploy_rejects_abi_only_artifact_before_key_resolution_or_rpc() -> TestResult {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../vendor/upstream/evm/abi/stakingV2.json");
    let path = path.to_str().ok_or("non-UTF-8 fixture path")?;
    let fixture = Fixture::new(&["rtao", "evm", "deploy", path, "--evm-key", "does-not-exist"])?;
    let error = fixture
        .run(&FakeRead::default())
        .await
        .err()
        .ok_or("unexpected deployment")?;
    assert_eq!(error.code, 2);
    assert!(error.message.contains("no bytecode"));
    Ok(())
}
