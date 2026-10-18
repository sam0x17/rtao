// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/tx.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/query.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/stake_picker.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/context.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/tests/unit/test_cli_commands.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Generated parser and SDK dispatch exercised with authenticated FakeChain state.

use rtao::{
    client::{Client, ClientConfig, HeadProvider, Network, View},
    primitives::{AccountId, CryptoType, H256, NetUid},
    reads::ReadName,
    signer::{BoxFuture, DynKeySource, KeyView, KeypairSigner, Signer, SignerError, SignerRole},
    tx::{ExtrinsicResult, IntentInterface},
};
use rtao_cli::{
    application,
    books::{AccountKind, Books, address_cli_name},
    commands::{
        self, catalog,
        generated::{self, QueryRequest, TxRequest},
        schema,
    },
    context::{AppContext, SubmissionHooks, SubmissionOptions},
    globals::{Globals, Tier, with_globals},
    output::{MemorySink, Output, OutputOptions, Stream},
    prompt::{PromptError, PromptHistory, Terminal},
};
use rtao_testkit::{
    FakeChain, FakeState,
    fixtures::{
        catalog_samples::{COLD, HOT, SUBNET, intent_args, intent_state, literal, read_state},
        decode_fixed_hex,
    },
    rpc::RpcClient,
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
// Public-only picker fixtures use the recorded task root and owned cleanup.
#[path = "../../rtao-wallet/tests/support/mod.rs"]
mod support;
use support::TempRoot;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

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
    fn read_secret(&mut self, _: &str) -> Result<rtao::wallet::SecretString, PromptError> {
        Err(PromptError::Unavailable)
    }
    fn message(&mut self, _: &str) -> Result<(), PromptError> {
        Ok(())
    }
}
// These table fixtures hold public identities only. A mistaken private
// acquisition returns an error and increments the sentinel even if a caller
// were to swallow that error.
#[derive(Default)]
struct SamplePublicKeys {
    acquisitions: AtomicUsize,
}
impl KeyView for SamplePublicKeys {
    fn coldkey_address(&self) -> Result<AccountId, SignerError> {
        Ok(COLD)
    }
    fn hotkey_address(&self) -> Result<AccountId, SignerError> {
        Ok(HOT)
    }
    fn hotkey_public_key(&self) -> Result<[u8; 32], SignerError> {
        Ok(HOT.0)
    }
}
impl DynKeySource for SamplePublicKeys {
    fn signer_for<'a>(
        &'a self,
        _: SignerRole,
    ) -> BoxFuture<'a, Result<Arc<dyn Signer>, SignerError>> {
        self.acquisitions.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {
            Err(SignerError::KeyView(
                "CLI sample dry-run attempted private signer acquisition".into(),
            ))
        })
    }
}

fn cli_samples(name: &str) -> TestResult<BTreeMap<String, Value>> {
    let table = rtao_catalog::py::table("sdk/python/tests/harness/samples.py", name)
        .ok_or("authenticated CLI sample table")?;
    assert_eq!(table.blob, "027e7b0b746a9e45d45340393f4691acc8575ecf");
    let extracted = rtao::primitives::json::from_str::<Value>(table.value_json)?;
    let decoded = literal(&extracted)?;
    Ok(decoded
        .as_object()
        .ok_or("CLI sample dictionary")?
        .iter()
        .map(|(name, args)| (name.clone(), args.clone()))
        .collect())
}

// Follow the pinned Python argv adapter, then let real Clap parsing, defaults,
// name resolution and native coercion validate the command. Do not construct
// TxRequest/QueryRequest parameters directly from these sample dictionaries.
fn argv_from_sample(args: &Value, tx: bool) -> TestResult<Vec<String>> {
    let mut argv = Vec::new();
    for (key, value) in args.as_object().ok_or("CLI sample arguments")? {
        if value.is_null() {
            continue;
        }
        let flag = if key.contains("_ss58") {
            address_cli_name(key)
        } else {
            format!("--{}", key.replace('_', "-"))
        };
        if let Some(enabled) = value.as_bool() {
            argv.push(if enabled {
                flag
            } else {
                format!("--no-{}", flag.trim_start_matches("--"))
            });
            continue;
        }
        let text = match value {
            Value::Array(parts) if !tx => parts
                .iter()
                .map(|part| match part {
                    Value::String(value) => Ok(value.clone()),
                    Value::Number(value) => Ok(value.to_string()),
                    Value::Bool(true) => Ok("True".into()),
                    Value::Bool(false) => Ok("False".into()),
                    Value::Null => Ok("None".into()),
                    _ => Err("review nested query sample list element"),
                })
                .collect::<Result<Vec<_>, _>>()?
                .join(","),
            Value::String(value) => value.clone(),
            _ => serde_json::to_string(value)?,
        };
        argv.extend([flag, text]);
    }
    Ok(argv)
}

fn bind_cli_sample_work(state: &FakeState, op: &str, args: &mut Value) -> TestResult {
    if op == "pow_register" {
        // Bind one fixed public seal, never run a PoW worker. This helper stays
        // above testkit because registration_prefix belongs to intents.
        let hash = state.block_hash(state.block).ok_or("fake work block")?;
        let prefix = rtao_intents::pow::registration_prefix(
            SUBNET,
            H256(decode_fixed_hex(&hash)?),
            HOT,
            COLD,
        )?;
        args["work_block"] = json!(state.block);
        args["nonce"] = json!(0);
        args["work_hex"] = json!(hex::encode(rtao::core::pow::seal(&prefix, 0)?));
    }
    Ok(())
}

fn tree() -> Result<clap::Command, rtao_cli::error::CliError> {
    Ok(application::command_tree([
        generated::tx_command()?,
        generated::query_command()?,
        catalog::tools_command(),
    ]))
}
fn context(
    parsed: &clap::ArgMatches,
    reserved: &[&str],
) -> Result<(AppContext, MemorySink), Box<dyn std::error::Error>> {
    let group = parsed.subcommand().ok_or("family")?.1;
    let leaf = group.subcommand().map(|(_, leaf)| leaf).unwrap_or(group);
    let config = serde_json::Map::new();
    let mut globals = Globals::resolve(
        parsed,
        &config,
        Path::new("/isolated/generated-no-user-home"),
        |_| None,
    )?;
    globals.apply_with_env(leaf, reserved, |_| None)?;
    let mut ctx = AppContext::new(
        globals,
        config,
        Books::default(),
        Arc::new(Mutex::new(Box::new(NoTerminal))),
    );
    let sink = MemorySink::default();
    ctx.output = Output::new(
        OutputOptions {
            json_mode: ctx.globals.json,
            quiet: ctx.globals.quiet,
            network: None,
        },
        Arc::new(sink.clone()),
    );
    Ok((ctx, sink))
}
#[derive(Clone)]
struct Keys {
    key: KeypairSigner,
    unlocks: Arc<AtomicUsize>,
}
impl Keys {
    fn new() -> Result<Self, SignerError> {
        Ok(Self {
            key: KeypairSigner::from_seed(&[19; 32], CryptoType::Ed25519)?,
            unlocks: Arc::new(AtomicUsize::new(0)),
        })
    }
}
impl KeyView for Keys {
    fn coldkey_address(&self) -> Result<AccountId, SignerError> {
        Ok(self.key.account())
    }
    fn hotkey_address(&self) -> Result<AccountId, SignerError> {
        Ok(self.key.account())
    }
    fn hotkey_public_key(&self) -> Result<[u8; 32], SignerError> {
        Ok(self.key.account().0)
    }
}
impl DynKeySource for Keys {
    fn signer_for<'a>(
        &'a self,
        _: SignerRole,
    ) -> BoxFuture<'a, Result<Arc<dyn Signer>, SignerError>> {
        Box::pin(async {
            self.unlocks.fetch_add(1, Ordering::SeqCst);
            Ok(Arc::new(self.key.clone()) as Arc<dyn Signer>)
        })
    }
}
#[derive(Default)]
struct Hooks {
    before: AtomicUsize,
    results: AtomicUsize,
    public_preparation_unlocks: Option<Arc<AtomicUsize>>,
}
impl SubmissionHooks for Hooks {
    fn before_submit(&self) -> BoxFuture<'_, Result<(), rtao_cli::error::CliError>> {
        Box::pin(async {
            if let Some(unlocks) = &self.public_preparation_unlocks {
                assert_eq!(unlocks.load(Ordering::SeqCst), 0);
            }
            self.before.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    }
    fn on_result<'a>(
        &'a self,
        _: &'a ExtrinsicResult,
    ) -> BoxFuture<'a, Result<(), rtao_cli::error::CliError>> {
        Box::pin(async {
            self.results.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    }
}
async fn client(state: FakeState) -> Result<(Client, FakeChain), Box<dyn std::error::Error>> {
    let chain = FakeChain::from_state(state);
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    Ok((
        Client::from_rpc(RpcClient::new(chain.clone()), config).await?,
        chain,
    ))
}
fn funded(keys: &Keys) -> Result<FakeState, Box<dyn std::error::Error>> {
    let mut state = FakeState::try_default()?;
    state.seed_python(
        "System",
        "Account",
        &[json!(keys.key.account())],
        json!({"data":{"free":1_000_000_000_000_u64}}),
    )?;
    Ok(state)
}
fn leaf(parsed: &clap::ArgMatches) -> Result<&clap::ArgMatches, Box<dyn std::error::Error>> {
    Ok(parsed
        .subcommand()
        .ok_or("family")?
        .1
        .subcommand()
        .ok_or("operation")?
        .1)
}

// pyt: unit/test_cli_commands.py::test_every_intent_has_a_tx_command
// pyt: unit/test_cli_commands.py::test_every_read_has_a_query_command
#[test]
fn advertisement_distinguishes_real_behavior_and_compiled_feature_availability() -> TestResult {
    let tx = generated::tx_command()?;
    assert!(tx.find_subcommand("transfer").is_some());
    assert!(tx.find_subcommand("set-identity").is_some());
    assert!(tx.find_subcommand("add-stake").is_some());
    let rows = rtao_intents::interface_metadata();
    assert_eq!(rows.len(), rtao_catalog::INTENTS.len());
    assert!(rows.iter().all(|row| row.implemented));
    let expected_leaves = rows
        .iter()
        .filter(|row| row.compiled && row.implemented)
        .map(|row| row.name.replace('_', "-"))
        .collect::<BTreeSet<_>>();
    let actual_leaves = tx
        .get_subcommands()
        .filter(|command| !command.is_hide_set())
        .map(|command| {
            assert!(
                !command.has_subcommands(),
                "tx operation {} is not a leaf",
                command.get_name()
            );
            command.get_name().to_owned()
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(
        actual_leaves, expected_leaves,
        "visible tx leaves must match the complete available intent catalog"
    );
    for row in &rows {
        let command = row.name.replace('_', "-");
        assert_eq!(
            tx.find_subcommand(&command).is_some(),
            row.compiled && row.implemented,
            "{}",
            row.name
        );
        assert_eq!(row.type_name.is_some(), row.compiled, "{}", row.name);
        assert_eq!(row.role.is_some(), row.compiled, "{}", row.name);
        assert_eq!(row.output.is_some(), row.compiled, "{}", row.name);
        if !row.compiled || !row.implemented {
            assert!(generated::intent_command(row.name).is_err(), "{}", row.name);
        } else {
            assert_eq!(
                generated::intent_command(row.name)?.get_name(),
                command,
                "{}",
                row.name
            );
        }
    }
    let transfer = rows
        .iter()
        .find(|row| row.name == "transfer")
        .ok_or("transfer")?;
    assert_eq!(
        transfer.type_name,
        Some(std::any::type_name::<rtao_intents::Transfer>())
    );
    assert_eq!(
        transfer.role,
        Some(std::any::type_name::<
            <rtao_intents::Transfer as IntentInterface>::Role,
        >())
    );
    assert_eq!(
        transfer.output,
        Some(std::any::type_name::<
            <rtao_intents::Transfer as IntentInterface>::Output,
        >())
    );
    assert!(generated::intent_command("not_a_catalog_operation").is_err());
    let query = generated::query_command()?;
    for read in rtao::reads::ReadName::ALL {
        let name = read.as_str().replace('_', "-");
        assert_eq!(
            query.find_subcommand(&name).is_some(),
            read.implemented(),
            "{read}"
        );
        assert_eq!(
            generated::read_command(read).is_ok(),
            read.implemented(),
            "{read}"
        );
    }
    let balance: rtao::reads::ReadName = "balance".parse()?;
    assert!(balance.implemented());
    assert_eq!(
        balance.interface_metadata().output_type_name,
        std::any::type_name::<rtao::primitives::DynBalance>()
    );
    Ok(())
}

#[test]
fn catalog_interface_metadata_covers_every_operation_and_pending_body() -> TestResult {
    let pending: toml_edit::DocumentMut = include_str!("../../../parity/pending.toml").parse()?;
    let pending_intents = pending["intents"]
        .as_array()
        .ok_or("pending intents")?
        .iter()
        .map(|name| name.as_str().ok_or("pending intent name"))
        .collect::<Result<BTreeSet<_>, _>>()?;
    let pending_reads = pending["reads"]
        .as_array()
        .ok_or("pending reads")?
        .iter()
        .map(|name| name.as_str().ok_or("pending read name"))
        .collect::<Result<BTreeSet<_>, _>>()?;
    let rows = rtao_intents::interface_metadata();
    assert_eq!(rows.len(), rtao_catalog::INTENTS.len());
    assert_eq!(
        rows.iter().map(|row| row.name).collect::<BTreeSet<_>>(),
        rtao_catalog::INTENTS
            .iter()
            .map(|spec| spec.name)
            .collect::<BTreeSet<_>>()
    );
    for row in rows {
        assert_eq!(row.implemented, !pending_intents.contains(row.name));
        assert_eq!(row.type_name.is_some(), row.compiled);
        assert_eq!(row.role.is_some(), row.compiled);
        assert_eq!(row.output.is_some(), row.compiled);
    }
    let reads = rtao::reads::ReadName::ALL;
    assert_eq!(reads.len(), rtao_catalog::READS.len());
    assert_eq!(
        reads
            .iter()
            .map(|name| name.as_str())
            .collect::<BTreeSet<_>>(),
        rtao_catalog::READS
            .iter()
            .map(|spec| spec.name)
            .collect::<BTreeSet<_>>()
    );
    for read in reads {
        let metadata = read.interface_metadata();
        assert_eq!(metadata.implemented, !pending_reads.contains(read.as_str()));
        assert_eq!(read.implemented(), metadata.implemented);
        assert!(!metadata.params_type_name.is_empty());
        assert!(!metadata.output_type_name.is_empty());
    }
    Ok(())
}

#[test]
fn complete_query_tree_exposes_every_read_and_authenticated_parameter() -> TestResult {
    let query = generated::query_command()?;
    let actual = query
        .get_subcommands()
        .map(|command| command.get_name().to_owned())
        .collect::<BTreeSet<_>>();
    let expected = rtao::reads::ReadName::ALL
        .into_iter()
        .map(|name| name.as_str().replace('_', "-"))
        .collect::<BTreeSet<_>>();
    assert_eq!(actual.len(), 102);
    assert_eq!(actual, expected);
    for name in rtao::reads::ReadName::ALL {
        let spelling = name.as_str().replace('_', "-");
        let command = query.find_subcommand(&spelling).ok_or("read command")?;
        let arguments = command
            .get_arguments()
            .map(|arg| arg.get_id().as_str())
            .collect::<BTreeSet<_>>();
        for field in schema::read_fields(name)? {
            assert!(arguments.contains(field.name), "{name}.{}", field.name);
        }
        let error = tree()?
            .try_get_matches_from(["rtao", "query", name.as_str(), "--help"])
            .err()
            .ok_or("query help")?;
        assert_eq!(error.kind(), clap::error::ErrorKind::DisplayHelp);
        assert!(error.to_string().contains(&format!("/query/{spelling}")));
    }
    Ok(())
}

#[test]
fn tools_retain_python_tool_fields_and_distinguish_unavailable_rust_metadata() -> TestResult {
    let rust_tools = generated::rust_tools()?;
    let tools = catalog::tools_payload(&rust_tools)?;
    let tools = tools.as_array().ok_or("tools")?;
    let metadata = rtao_intents::interface_metadata();
    assert_eq!(tools.len(), rtao_catalog::INTENTS.len());
    // registry.list_tools() projects these nine fields. The extraction catalog
    // additionally carries documentation-generation fields that btcli tools
    // does not emit (cli, markdown_url, python_class and sources).
    let python_fields = [
        "name",
        "summary",
        "description",
        "signer",
        "origin",
        "verify",
        "wraps",
        "docs_url",
        "input_schema",
    ];
    for spec in &rtao_catalog::INTENTS {
        let mut actual = tools
            .iter()
            .find(|row| row["name"] == json!(spec.name))
            .ok_or("catalog row")?
            .clone();
        let rust = actual
            .as_object_mut()
            .ok_or("tool object")?
            .remove("rust")
            .ok_or("Rust metadata")?;
        let interface = metadata
            .iter()
            .find(|row| row.name == spec.name)
            .ok_or("interface metadata")?;
        assert_eq!(
            rust,
            json!({
                "type": interface.type_name,
                "role": interface.role,
                "output": interface.output,
                "cli": (interface.compiled && interface.implemented)
                    .then(|| format!("rtao tx {}", spec.name.replace('_', "-"))),
            }),
            "{}",
            spec.name
        );
        let source: Value = serde_json::from_str(spec.catalog_json)?;
        let mut expected = serde_json::Map::new();
        for field in python_fields {
            expected.insert(
                field.into(),
                source.get(field).ok_or("Python tool field")?.clone(),
            );
        }
        expected.insert(
            "docs_url".into(),
            json!(format!(
                "{}/tx/{}",
                rtao_catalog::py::DOCS_URL,
                spec.name.replace('_', "-")
            )),
        );
        assert_eq!(actual, Value::Object(expected), "{}", spec.name);
    }
    // All current intent bodies are implemented. Exercise the catalog's two
    // unavailable representations explicitly instead of mislabeling one as
    // pending: a missing feature has no Rust record; a compiled pending body
    // retains its type/role/output and has no CLI invocation.
    let transfer = rust_tools
        .iter()
        .find(|tool| tool.name == "transfer")
        .ok_or("transfer metadata")?;
    let mut pending = transfer.clone();
    pending.cli = None;
    for supplied in [Vec::new(), vec![pending]] {
        let payload = catalog::tools_payload(&supplied)?;
        let row = payload
            .as_array()
            .ok_or("tools array")?
            .iter()
            .find(|row| row["name"] == "transfer")
            .ok_or("transfer tool")?;
        assert_eq!(
            row["rust"],
            if supplied.is_empty() {
                json!({"type":null,"role":null,"output":null,"cli":null})
            } else {
                json!({"type":transfer.type_name,"role":transfer.role,"output":transfer.output,"cli":null})
            }
        );
        let actual = row.as_object().ok_or("tool object")?;
        let original = tools
            .iter()
            .find(|row| row["name"] == "transfer")
            .and_then(Value::as_object)
            .ok_or("original tool")?;
        for field in python_fields {
            assert_eq!(actual.get(field), original.get(field), "{field}");
        }
    }
    Ok(())
}

#[test]
fn exact_money_keeps_native_u64_precision_exponents_and_all_policy() -> TestResult {
    use rtao::primitives::{Amount, Money, ResolvedAmount};

    let spec = rtao_catalog::INTENTS
        .iter()
        .find(|spec| spec.name == "transfer")
        .ok_or("transfer")?;
    let fields = schema::intent_fields(spec)?;
    let money = fields
        .iter()
        .find(|field| field.name == "amount_tao")
        .ok_or("amount")?;
    let command = tree()?;
    let dest = AccountId([7; 32]).to_string();
    let reserved = generated::intent_reserved("transfer")?;
    let mut cases = [
        ("18446744073.709551615", u64::MAX),
        ("18446744073709551615e-9", u64::MAX),
        ("1_0", 10_000_000_000),
        ("١_٢.٥", 12_500_000_000),
        ("１.０", 1_000_000_000),
        ("١e+٠", 1_000_000_000),
        ("1_e_+_0", 1_000_000_000),
        ("+1.", 1_000_000_000),
        ("\u{1c}\u{85}\u{a0}١\u{202f}\u{1f}", 1_000_000_000),
        ("1.00000000000000000000000000000", 1_000_000_000),
        (".٥", 500_000_000),
        ("1e-9", 1),
        ("10e-10", 1),
        ("-0", 0),
        ("-0.000", 0),
        ("-0e4096", 0),
        ("-0e-4096", 0),
    ]
    .into_iter()
    .map(|(raw, rao)| (raw.to_owned(), rao))
    .collect::<Vec<_>>();
    cases.push(("0".repeat(4096), 0));
    for (raw, expected_rao) in cases {
        let attached = format!("--amount={raw}");
        let parsed = command.clone().try_get_matches_from([
            "rtao", "tx", "transfer", "--dest", &dest, &attached, "--yes", "--json",
        ])?;
        let (mut ctx, sink) = context(&parsed, &reserved)?;
        let request = TxRequest::from_matches(
            &mut ctx,
            "transfer",
            leaf(&parsed)?,
            &mut PromptHistory::default(),
        )?;
        let canonical = Money::from_rao(expected_rao).to_decimal_string();
        assert_eq!(request.args["amount_tao"], json!(canonical), "{raw:?}");
        let intent = request.intent()?;
        assert_eq!(intent.to_json()["amount_tao"], json!(canonical), "{raw:?}");
        let rtao_intents::AnyIntent::Transfer(transfer) = intent else {
            return Err("unexpected money intent".into());
        };
        let ResolvedAmount::Exact(balance) = transfer.amount_tao.resolve(NetUid::ROOT, false)?
        else {
            return Err("exact CLI amount became all".into());
        };
        assert_eq!(balance.rao, expected_rao, "{raw:?}");
        assert!(sink.text(Stream::Stdout)?.is_empty());
    }
    let mut refused = [
        "18446744073.709551616",
        "-1",
        "-1e0",
        "--0",
        "-+0",
        "NaN",
        "Infinity",
        "1e100",
        "1e-10",
        "0.0000000001",
        "1.0000000001",
        "0.00000000101",
        "",
        ".",
        "+",
        "_",
        "1..0",
        "1e",
        "1ee0",
        "1e+-1",
        "1e--1",
        "1e1.0",
        "1 0",
        "0x1",
        "+-1",
        "1\u{200b}",
        "all_",
        "0e4097",
        "-0e-4097",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<Vec<_>>();
    refused.extend([
        "0".repeat(4097),
        " ".repeat(4097),
        format!("1e{}", "9".repeat(32)),
    ]);
    for raw in refused {
        let attached = format!("--amount={raw}");
        let budget = raw.len() > 4096 || raw.contains("4097") || raw.starts_with("1e999");
        match command.clone().try_get_matches_from([
            "rtao", "tx", "transfer", "--dest", &dest, &attached, "--yes", "--json",
        ]) {
            Err(error) => {
                assert_eq!(error.exit_code(), 2, "{raw:?}");
                if budget {
                    assert!(error.to_string().contains("budget"), "{raw:?}: {error}");
                }
            }
            Ok(parsed) => {
                let (mut ctx, sink) = context(&parsed, &reserved)?;
                let error = TxRequest::from_matches(
                    &mut ctx,
                    "transfer",
                    leaf(&parsed)?,
                    &mut PromptHistory::default(),
                )
                .err()
                .ok_or_else(|| format!("invalid amount accepted: {raw:?}"))?;
                assert_eq!(error.code, 2, "{raw:?}");
                if budget {
                    assert!(error.message.contains("budget"), "{raw:?}: {error}");
                }
                assert!(sink.text(Stream::Stdout)?.is_empty());
            }
        }
    }
    for raw in [" ALL ", "", "  "] {
        let attached = format!("--amount={raw}");
        let parsed = command.clone().try_get_matches_from([
            "rtao", "tx", "transfer", "--dest", &dest, &attached, "--all", "--yes",
        ])?;
        let (mut ctx, _) = context(&parsed, &reserved)?;
        let request = TxRequest::from_matches(
            &mut ctx,
            "transfer",
            leaf(&parsed)?,
            &mut PromptHistory::default(),
        )?;
        assert_eq!(request.args["amount_tao"], json!("all"));
        assert_eq!(request.intent()?.to_json()["amount_tao"], json!("all"));
    }
    let parsed = command.try_get_matches_from([
        "rtao", "tx", "transfer", "--dest", &dest, "--amount", "1", "--all", "--yes",
    ])?;
    let (mut ctx, _) = context(&parsed, &reserved)?;
    let conflict = TxRequest::from_matches(
        &mut ctx,
        "transfer",
        leaf(&parsed)?,
        &mut PromptHistory::default(),
    )
    .err()
    .ok_or("amount/all conflict")?;
    assert_eq!(conflict.code, 2);
    assert!(conflict.message.contains("not both"));
    let mut allowed = money.clone();
    allowed.allow_all = true;
    assert_eq!(schema::coerce(&allowed, " ALL ")?, json!("all"));
    allowed.allow_all = false;
    assert!(schema::coerce(&allowed, "all").is_err());
    let alpha_spec = rtao_catalog::INTENTS
        .iter()
        .find(|spec| spec.name == "transfer_stake")
        .ok_or("alpha intent")?;
    let alpha = schema::intent_fields(alpha_spec)?
        .into_iter()
        .find(|field| field.name == "amount_alpha")
        .ok_or("alpha")?;
    assert!(alpha.help.contains("alpha"));
    assert!(money.help.contains("TAO"));
    assert_eq!(schema::coerce(&alpha, "١e-٩")?, json!("0.000000001"));
    // CLI normalization must not broaden the public exact constructors.
    for raw in ["1_0", "١", "1e0", "+1", ".5", "1.", " 1 ", "-0"] {
        assert!(Money::parse(raw).is_err(), "{raw:?}");
        assert!(Amount::parse(raw).is_err(), "{raw:?}");
    }
    Ok(())
}

#[test]
fn tx_lists_match_python_csv_and_json_list_rules_and_keep_integer_digits() -> TestResult {
    let spec = rtao_catalog::INTENTS
        .iter()
        .find(|spec| spec.name == "set_weights")
        .ok_or("set_weights")?;
    let fields = schema::intent_fields(spec)?;
    let uids = fields
        .iter()
        .find(|field| field.name == "uids")
        .ok_or("uids")?;
    assert_eq!(schema::coerce(uids, " 1, 2,,٣ ")?, json!([1, 2, 3]));
    assert_eq!(schema::coerce(uids, "[1,2,3]")?, json!([1, 2, 3]));
    let weights = fields
        .iter()
        .find(|field| field.name == "weights")
        .ok_or("weights")?;
    assert_eq!(schema::coerce(weights, "[0.5,1.0]")?, json!([0.5, 1.0]));
    assert_eq!(schema::coerce(weights, " 0.5, 1.0,, ")?, json!([0.5, 1.0]));
    // The SDK's mapping input is broader than tx.py's list[float] CLI
    // coercion: only a leading '[' selects JSON before the CSV float path.
    assert!(schema::coerce(weights, r#"{"1":0.5,"9":1.0}"#).is_err());
    assert!(schema::coerce(weights, "{bad}").is_err());
    for raw in ["0.5,1.0", "[0.5,1.0]"] {
        let parsed = tree()?.try_get_matches_from([
            "rtao",
            "tx",
            "set-weights",
            "--netuid",
            "1",
            "--uids",
            "1,9",
            "--weights",
            raw,
        ])?;
        let values = schema::values(leaf(&parsed)?, &fields)?;
        assert_eq!(values["uids"], json!([1, 9]));
        assert_eq!(values["weights"], json!([0.5, 1.0]));
    }
    assert!(
        tree()?
            .try_get_matches_from([
                "rtao",
                "tx",
                "set-weights",
                "--netuid",
                "1",
                "--weights",
                r#"{"1":0.5,"9":1.0}"#,
            ])
            .is_err()
    );
    let mut integer = uids.clone();
    integer.kind = "integer".into();
    integer.annotation = "int";
    assert_eq!(
        schema::coerce(&integer, "+18_446_744_073_709_551_615")?,
        json!(u64::MAX)
    );
    assert!(schema::coerce(&integer, "1.1").is_err());
    Ok(())
}

#[test]
fn parser_aliases_boolean_twins_defaults_and_leaf_globals_use_the_real_schema() -> TestResult {
    let parsed = tree()?.try_get_matches_from([
        "rtao",
        "-n",
        "finney",
        "tx",
        "transfer",
        "--dest",
        "contact",
        "--amount",
        "1.000000001",
        "--no-keep-alive",
        "-n",
        "local",
        "--yes",
        "--json",
    ])?;
    let reserved = generated::intent_reserved("transfer")?;
    let (mut ctx, _) = context(&parsed, &reserved)?;
    let dest = AccountId([7; 32]);
    ctx.books = Books::from_values(
        json!([{"name":"contact","address":dest}]),
        json!([]),
        json!([]),
    );
    let request = TxRequest::from_matches(
        &mut ctx,
        "transfer",
        leaf(&parsed)?,
        &mut PromptHistory::default(),
    )?;
    assert_eq!(ctx.globals.network, "local");
    assert_eq!(request.args["dest_ss58"], json!(dest));
    assert_eq!(request.args["amount_tao"], json!("1.000000001"));
    assert_eq!(request.args["keep_alive"], json!(false));
    assert_eq!(request.intent()?.to_json()["keep_alive"], json!(false));
    assert!(
        tree()?
            .try_get_matches_from(["rtao", "tx", "transfer", "--amount", "not-money"])
            .is_err()
    );
    let snake = tree()?.try_get_matches_from([
        "rtao",
        "tx",
        "transfer_all",
        "--dest",
        "contact",
        "--yes",
    ])?;
    assert_eq!(
        snake
            .subcommand()
            .ok_or("tx")?
            .1
            .subcommand()
            .ok_or("op")?
            .0,
        "transfer-all"
    );
    Ok(())
}

#[test]
fn generated_nested_names_render_public_resolution_and_preserve_raw_address_silence() -> TestResult
{
    let coldkey = AccountId([7; 32]);
    let hotkey = AccountId([8; 32]);
    let raw = AccountId([9; 32]);
    let children = json!([
        {"op":"transfer","dest_ss58":"contact","amount_tao":"1"},
        {"op":"add_stake","hotkey_ss58":"operator","netuid":1,"amount_tao":"1"},
        {"op":"transfer","dest_ss58":raw,"amount_tao":"1"}
    ])
    .to_string();
    for mode in [None, Some("--json"), Some("--quiet")] {
        let mut argv = vec!["rtao", "tx", "batch", "--intents", &children, "--yes"];
        argv.extend(mode);
        let parsed = tree()?.try_get_matches_from(argv)?;
        let (mut ctx, sink) = context(&parsed, &generated::intent_reserved("batch")?)?;
        ctx.books = Books::from_values(
            json!([{"name":"contact","address":coldkey},{"name":"operator","address":hotkey}]),
            json!([]),
            json!([]),
        );
        let request = TxRequest::from_matches(
            &mut ctx,
            "batch",
            leaf(&parsed)?,
            &mut PromptHistory::default(),
        )?;
        assert_eq!(request.args["intents"][0]["dest_ss58"], json!(coldkey));
        assert_eq!(request.args["intents"][1]["hotkey_ss58"], json!(hotkey));
        assert_eq!(request.args["intents"][2]["dest_ss58"], json!(raw));
        assert_eq!(
            ctx.output
                .address_names()?
                .get(&coldkey.to_string())
                .map(String::as_str),
            Some("contact")
        );
        assert_eq!(
            ctx.output
                .address_names()?
                .get(&hotkey.to_string())
                .map(String::as_str),
            Some("operator")
        );
        assert_eq!(
            ctx.output.address_kinds()?.get(&coldkey.to_string()),
            Some(&AccountKind::Coldkey)
        );
        assert_eq!(
            ctx.output.address_kinds()?.get(&hotkey.to_string()),
            Some(&AccountKind::Hotkey)
        );
        assert!(!ctx.output.address_kinds()?.contains_key(&raw.to_string()));
        assert!(sink.bytes(Stream::Stdout)?.is_empty());
        assert_eq!(
            sink.text(Stream::Stderr)?,
            if mode.is_none() {
                format!(
                    "dest_ss58: resolved address-book entry 'contact' to {coldkey}\nhotkey_ss58: resolved address-book entry 'operator' to {hotkey}\n"
                )
            } else {
                String::new()
            }
        );
    }
    Ok(())
}

#[test]
fn field_collision_reserves_leaf_force_proxy_type_without_rewriting_outer_route() -> TestResult {
    let spec = rtao_catalog::INTENTS
        .iter()
        .find(|spec| spec.name == "execute_proxy_announced")
        .ok_or("proxy schema")?;
    let fields = schema::intent_fields(spec)?;
    let probe = with_globals(
        schema::arguments(clap::Command::new("probe"), &fields, true),
        Tier::Tx,
    );
    let root = application::command_tree([probe]);
    let parsed =
        root.try_get_matches_from(["rtao", "probe", "--force-proxy-type", "NonTransfer"])?;
    let config = serde_json::Map::new();
    let mut globals = Globals::resolve(
        &parsed,
        &config,
        Path::new("/isolated/generated-home"),
        |_| None,
    )?;
    // A caller's already-selected outer proxy route must remain separate
    // from the generated intent's identically named semantic argument.
    globals.force_proxy_type = Some("Any".into());
    let leaf = parsed.subcommand().ok_or("probe")?.1;
    globals.apply_with_env(
        leaf,
        &fields.iter().map(|field| field.name).collect::<Vec<_>>(),
        |_| None,
    )?;
    assert_eq!(globals.force_proxy_type.as_deref(), Some("Any"));
    assert_eq!(
        schema::values(leaf, &fields)?["force_proxy_type"],
        json!("NonTransfer")
    );
    Ok(())
}

#[test]
fn missing_required_values_refuse_with_input_status_before_any_connection() -> TestResult {
    let parsed = tree()?.try_get_matches_from(["rtao", "tx", "transfer", "--json"])?;
    let (mut ctx, sink) = context(&parsed, &generated::intent_reserved("transfer")?)?;
    let error = TxRequest::from_matches(
        &mut ctx,
        "transfer",
        leaf(&parsed)?,
        &mut PromptHistory::default(),
    )
    .err()
    .ok_or("missing fields")?;
    assert_eq!(error.code, 2);
    assert!(error.message.contains("--dest") && error.message.contains("--amount-tao"));
    assert!(sink.text(Stream::Stdout)?.is_empty());
    Ok(())
}

// In-process installed parsing and handlers use fake transport and public-only
// keys. Installed subprocess exits and wallet-file access are separate fixtures.
// pyt: unit/test_cli_commands.py::test_tx_dry_run_from_sample
#[tokio::test]
async fn all_84_samples_run_through_actual_cli_json_dry_run_without_private_access() -> TestResult {
    let samples = cli_samples("INTENT_SAMPLES")?;
    let expected = rtao_catalog::INTENTS
        .iter()
        .map(|spec| spec.name)
        .collect::<BTreeSet<_>>();
    assert_eq!(expected.len(), 84);
    assert_eq!(
        samples.keys().map(String::as_str).collect::<BTreeSet<_>>(),
        expected,
        "CLI dry-runs must cover the exact authenticated intent table"
    );
    // Required public hotkey arguments resolve through the real wallet lookup
    // before the injected key source is used. Supply public records only.
    let root = TempRoot::new();
    let wallet = root.path().join("sample");
    std::fs::create_dir(&wallet)?;
    std::fs::create_dir(wallet.join("hotkeys"))?;
    for (path, account) in [
        (wallet.join("coldkeypub.txt"), COLD),
        (wallet.join("hotkeys/sample"), HOT),
    ] {
        std::fs::write(
            path,
            serde_json::to_vec(&json!({"ss58Address":account,"cryptoType":1}))?,
        )?;
    }
    let runtime = FakeState::try_default()?.runtime;
    let mut visited = BTreeSet::new();
    let mut failures = Vec::new();
    for spec in &rtao_catalog::INTENTS {
        visited.insert(spec.name);
        let outcome: TestResult = async {
            let state = intent_state(runtime.clone(), spec.name)?;
            let mut args = intent_args(spec.name, &samples[spec.name])?;
            bind_cli_sample_work(&state, spec.name, &mut args)?;
            let mut argv = vec![
                "rtao".into(),
                "--wallet-path".into(),
                root.path().to_str().ok_or("fixture wallet path")?.into(),
                "--wallet".into(),
                "sample".into(),
                "--wallet-hotkey".into(),
                "sample".into(),
                "--json".into(),
                "--dry-run".into(),
                "tx".into(),
                spec.name.replace('_', "-"),
            ];
            argv.extend(argv_from_sample(&args, true)?);
            let parsed = commands::command_tree()?.try_get_matches_from(argv.clone())?;
            let (mut ctx, sink) = context(&parsed, &generated::intent_reserved(spec.name)?)?;
            // JSON/non-TTY policy retains the real confirmation stage without
            // --yes, private key material, or a secret-bearing test signer.
            let request = TxRequest::from_matches(
                &mut ctx,
                spec.name,
                leaf(&parsed)?,
                &mut PromptHistory::default(),
            )?;
            let keys = SamplePublicKeys::default();
            let hooks = Hooks::default();
            let (client, chain) = client(state).await?;
            let result = generated::execute_tx_with_keys(
                &ctx,
                &client,
                &request,
                &keys,
                SubmissionOptions {
                    hooks: Some(&hooks),
                    ..Default::default()
                },
            )
            .await;
            client.close().await;
            assert_eq!(keys.acquisitions.load(Ordering::SeqCst), 0);
            assert_eq!(hooks.before.load(Ordering::SeqCst), 0);
            assert_eq!(hooks.results.load(Ordering::SeqCst), 0);
            assert!(chain.mutate_state(|state| state.submissions.is_empty()));
            assert!(
                chain
                    .requests()
                    .iter()
                    .all(|request| !request.method.starts_with("author_"))
            );
            if result?.is_some() {
                return Err("CLI dry-run returned a submitted receipt".into());
            }
            let plan: Value = serde_json::from_str(&sink.text(Stream::Stdout)?)?;
            if plan["op"] != spec.name || plan["ok"] != true {
                return Err(format!(
                    "CLI dry-run did not render a successful {} plan: {plan}",
                    spec.name
                )
                .into());
            }
            assert!(sink.text(Stream::Stderr)?.is_empty());
            // Root and owner origin disclosures are live human narration as
            // well as help text. Keep the same real parser and public source.
            if spec.origin == "root" || spec.name == "set_mechanism_count" {
                let mut human_argv = argv;
                human_argv.retain(|argument| argument != "--json");
                let parsed = commands::command_tree()?.try_get_matches_from(human_argv)?;
                let (mut ctx, sink) = context(&parsed, &generated::intent_reserved(spec.name)?)?;
                let request = TxRequest::from_matches(
                    &mut ctx,
                    spec.name,
                    leaf(&parsed)?,
                    &mut PromptHistory::default(),
                )?;
                let keys = SamplePublicKeys::default();
                let hooks = Hooks::default();
                let (human_client, chain) =
                    self::client(intent_state(runtime.clone(), spec.name)?).await?;
                let result = generated::execute_tx_with_keys(
                    &ctx,
                    &human_client,
                    &request,
                    &keys,
                    SubmissionOptions {
                        hooks: Some(&hooks),
                        ..Default::default()
                    },
                )
                .await;
                human_client.close().await;
                assert!(result?.is_none());
                let stderr = sink.text(Stream::Stderr)?;
                let expected = if spec.origin == "root" {
                    "requires: chain sudo key (call wrapped in Sudo.sudo)"
                } else {
                    "requires: subnet owner coldkey"
                };
                assert_eq!(stderr.matches(expected).count(), 1, "{}", spec.name);
                assert!(!stderr.contains("verify:"), "a preview has not submitted");
                assert_eq!(keys.acquisitions.load(Ordering::SeqCst), 0);
                assert_eq!(hooks.before.load(Ordering::SeqCst), 0);
                assert_eq!(hooks.results.load(Ordering::SeqCst), 0);
                assert!(chain.mutate_state(|state| state.submissions.is_empty()));
                assert!(
                    chain
                        .requests()
                        .iter()
                        .all(|request| !request.method.starts_with("author_"))
                );
            }
            Ok(())
        }
        .await;
        if let Err(error) = outcome {
            failures.push(format!("{}: {error}", spec.name));
        }
    }
    assert_eq!(visited, expected);
    assert!(
        failures.is_empty(),
        "CLI sample dry-run failures:\n{}",
        failures.join("\n")
    );
    Ok(())
}

// pyt: unit/test_cli_commands.py::test_query_from_sample
#[tokio::test]
async fn all_102_samples_parse_dispatch_and_render_actual_cli_json_queries() -> TestResult {
    let samples = cli_samples("READ_SAMPLES")?;
    let expected = ReadName::ALL
        .into_iter()
        .map(|name| name.as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(expected.len(), 102);
    assert_eq!(
        samples.keys().map(String::as_str).collect::<BTreeSet<_>>(),
        expected,
        "CLI queries must cover the exact authenticated read table"
    );
    let coldkey = samples["stake_availability"]["coldkey_ss58"]
        .as_str()
        .ok_or("CLI availability coldkey sample")?;
    let (client, chain) = client(read_state(coldkey)?).await?;
    let mut visited = BTreeSet::new();
    let mut failures = Vec::new();
    for name in ReadName::ALL {
        visited.insert(name.as_str());
        let outcome: TestResult = async {
            let mut argv = vec![
                "rtao".into(),
                "--json".into(),
                "query".into(),
                name.as_str().replace('_', "-"),
            ];
            argv.extend(argv_from_sample(&samples[name.as_str()], false)?);
            let parsed = commands::command_tree()?.try_get_matches_from(argv)?;
            let (ctx, sink) = context(&parsed, &generated::read_reserved(name))?;
            let request = QueryRequest::from_matches(
                &ctx,
                name,
                leaf(&parsed)?,
                &mut PromptHistory::default(),
            )?;
            let value = generated::execute_query(&client, &request, &ctx.output).await?;
            let stdout = sink.text(Stream::Stdout)?;
            let rendered: Value = serde_json::from_str(&stdout)?;
            if rendered != value || stdout.contains("Traceback") {
                return Err("CLI query JSON differs from its actual native payload".into());
            }
            Ok(())
        }
        .await;
        if let Err(error) = outcome {
            failures.push(format!("{name}: {error}"));
        }
    }
    client.close().await;
    assert_eq!(visited, expected);
    assert!(chain.mutate_state(|state| state.submissions.is_empty()));
    assert!(
        chain
            .requests()
            .iter()
            .all(|request| !request.method.starts_with("author_"))
    );
    assert!(
        failures.is_empty(),
        "CLI query sample failures:\n{}",
        failures.join("\n")
    );
    Ok(())
}

#[tokio::test]
async fn generated_transfer_dry_run_uses_sdk_plan_and_never_unlocks_or_runs_hooks() -> TestResult {
    let keys = Keys::new()?;
    let (client, chain) = client(funded(&keys)?).await?;
    let dest = AccountId([7; 32]).to_string();
    let parsed = tree()?.try_get_matches_from([
        "rtao",
        "tx",
        "transfer",
        "--dest",
        &dest,
        "--amount",
        "1.000000001",
        "--period",
        "64",
        "--dry-run",
        "--json",
        "--yes",
    ])?;
    let (mut ctx, sink) = context(&parsed, &generated::intent_reserved("transfer")?)?;
    let request = TxRequest::from_matches(
        &mut ctx,
        "transfer",
        leaf(&parsed)?,
        &mut PromptHistory::default(),
    )?;
    let hooks = Hooks::default();
    let receipt = generated::execute_tx_with_keys(
        &ctx,
        &client,
        &request,
        &keys,
        SubmissionOptions {
            replay: Some("rtao tx transfer --amount 1.000000001 --period 64"),
            hooks: Some(&hooks),
            ..Default::default()
        },
    )
    .await?;
    assert!(receipt.is_none());
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    assert_eq!(hooks.before.load(Ordering::SeqCst), 0);
    assert_eq!(hooks.results.load(Ordering::SeqCst), 0);
    assert!(
        chain
            .state()
            .lock()
            .map_err(|_| "state")?
            .submissions
            .is_empty()
    );
    let plan: Value = serde_json::from_str(&sink.text(Stream::Stdout)?)?;
    assert_eq!(plan["args"]["amount_tao"], json!("1.000000001"));
    assert!(
        plan["effects"]
            .as_array()
            .ok_or("effects")?
            .iter()
            .any(|value| value == "mortal transaction period: 64 blocks")
    );
    assert!(
        plan["command"]
            .as_str()
            .is_some_and(|command| command.contains("--period 64"))
    );
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn generated_transfer_submits_real_semantic_bytes_hooks_and_requested_period() -> TestResult {
    let keys = Keys::new()?;
    let (client, chain) = client(funded(&keys)?).await?;
    let dest = AccountId([7; 32]).to_string();
    let parsed = tree()?.try_get_matches_from([
        "rtao",
        "tx",
        "transfer",
        "--dest",
        &dest,
        "--amount",
        "1.000000001",
        "--period",
        "64",
        "--json",
        "--yes",
    ])?;
    let (mut ctx, sink) = context(&parsed, &generated::intent_reserved("transfer")?)?;
    let request = TxRequest::from_matches(
        &mut ctx,
        "transfer",
        leaf(&parsed)?,
        &mut PromptHistory::default(),
    )?;
    let hooks = Hooks::default();
    let receipt = generated::execute_tx_with_keys(
        &ctx,
        &client,
        &request,
        &keys,
        SubmissionOptions {
            hooks: Some(&hooks),
            ..Default::default()
        },
    )
    .await?
    .ok_or("receipt")?;
    assert!(receipt.success);
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 1);
    assert_eq!(hooks.before.load(Ordering::SeqCst), 1);
    assert_eq!(hooks.results.load(Ordering::SeqCst), 1);
    let submissions = chain
        .state()
        .lock()
        .map_err(|_| "state")?
        .submissions
        .clone();
    assert_eq!(submissions.len(), 1);
    let runtime = View::runtime(&client).await?;
    let decoded =
        rtao::core::DynValue::new(runtime.core().decode_extrinsic(&submissions[0], true)?)
            .to_json();
    assert_eq!(decoded["era"][0], json!(64));
    assert_eq!(decoded["call"]["call_module"], json!("Balances"));
    assert_eq!(
        serde_json::from_str::<Value>(&sink.text(Stream::Stdout)?)?,
        receipt.to_json()
    );
    client.close().await;
    for mode in [None, Some("--quiet"), Some("--json")] {
        let keys = Keys::new()?;
        let mut state = funded(&keys)?;
        state.seed_python("Sudo", "Key", &[], json!(keys.key.account()))?;
        let (client, chain) = self::client(state).await?;
        let mut args = vec![
            "rtao",
            "tx",
            "set-root-claim-threshold",
            "--threshold",
            "0.001",
            "--yes",
            "--no-mev-shield",
        ];
        if let Some(mode) = mode {
            args.push(mode);
        }
        let parsed = tree()?.try_get_matches_from(args)?;
        let (mut ctx, sink) = context(
            &parsed,
            &generated::intent_reserved("set_root_claim_threshold")?,
        )?;
        let request = TxRequest::from_matches(
            &mut ctx,
            "set_root_claim_threshold",
            leaf(&parsed)?,
            &mut PromptHistory::default(),
        )?;
        let hooks = Hooks::default();
        let receipt = generated::execute_tx_with_keys(
            &ctx,
            &client,
            &request,
            &keys,
            SubmissionOptions {
                hooks: Some(&hooks),
                ..Default::default()
            },
        )
        .await?
        .ok_or("root receipt")?;
        assert!(receipt.success);
        let stderr = sink.text(Stream::Stderr)?;
        if mode.is_none() {
            assert_eq!(
                stderr
                    .matches("requires: chain sudo key (call wrapped in Sudo.sudo)")
                    .count(),
                1
            );
            assert_eq!(
                stderr
                    .matches("verify: `rtao query root-claim-threshold ...`")
                    .count(),
                1
            );
        } else {
            assert!(stderr.is_empty());
        }
        assert_eq!(keys.unlocks.load(Ordering::SeqCst), 1);
        assert_eq!(hooks.before.load(Ordering::SeqCst), 1);
        assert_eq!(hooks.results.load(Ordering::SeqCst), 1);
        let submitted = chain.mutate_state(|state| state.submissions.clone());
        assert_eq!(submitted.len(), 1);
        let runtime = View::runtime(&client).await?;
        let decoded =
            rtao::core::DynValue::new(runtime.core().decode_extrinsic(&submitted[0], true)?)
                .to_json();
        assert_eq!(decoded["call"]["call_module"], "Sudo");
        assert_eq!(decoded["call"]["call_function"], "sudo");
        client.close().await;
    }
    Ok(())
}

#[tokio::test]
async fn invalid_period_refuses_before_unlock_and_any_submission() -> TestResult {
    let keys = Keys::new()?;
    let (client, chain) = client(funded(&keys)?).await?;
    let request = TxRequest {
        op: "transfer",
        args: serde_json::Map::from_iter([
            ("dest_ss58".into(), json!(AccountId([7; 32]))),
            ("amount_tao".into(), json!("1")),
        ]),
        period: Some(3),
    };
    let parsed = tree()?.try_get_matches_from(["rtao", "tx", "transfer", "--json", "--yes"])?;
    let (ctx, _) = context(&parsed, &generated::intent_reserved("transfer")?)?;
    let error = generated::execute_tx_with_keys(
        &ctx,
        &client,
        &request,
        &keys,
        SubmissionOptions::default(),
    )
    .await
    .err()
    .ok_or("invalid period")?;
    assert_eq!(error.code, 2);
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    assert!(
        chain
            .state()
            .lock()
            .map_err(|_| "state")?
            .submissions
            .is_empty()
    );
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn block_query_pins_real_storage_and_does_not_read_the_current_head_value() -> TestResult {
    let keys = Keys::new()?;
    let mut state = funded(&keys)?;
    state.seed_python(
        "System",
        "Account",
        &[json!(keys.key.account())],
        json!({"data":{"free":10}}),
    )?;
    let old = state.block;
    state.advance(1);
    state.seed_python(
        "System",
        "Account",
        &[json!(keys.key.account())],
        json!({"data":{"free":20}}),
    )?;
    let (client, chain) = client(state).await?;
    let address = keys.key.account().to_string();
    let block = old.to_string();
    let parsed = tree()?.try_get_matches_from([
        "rtao",
        "query",
        "balance",
        "--coldkey",
        &address,
        "--block",
        &block,
        "--json",
    ])?;
    let name = "balance".parse()?;
    let (ctx, sink) = context(&parsed, &generated::read_reserved(name))?;
    let request =
        QueryRequest::from_matches(&ctx, name, leaf(&parsed)?, &mut PromptHistory::default())?;
    let payload = generated::execute_query(&client, &request, &ctx.output).await?;
    assert_eq!(payload, json!("τ0.000000010"));
    assert_eq!(
        serde_json::from_str::<Value>(&sink.text(Stream::Stdout)?)?,
        payload
    );
    let head = QueryRequest {
        block: None,
        ..request
    };
    let (other, _) = {
        let sink = MemorySink::default();
        (
            Output::new(
                OutputOptions {
                    json_mode: true,
                    ..Default::default()
                },
                Arc::new(sink.clone()),
            ),
            sink,
        )
    };
    assert_eq!(
        generated::execute_query(&client, &head, &other).await?,
        json!("τ0.000000020")
    );
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    assert!(
        chain
            .state()
            .lock()
            .map_err(|_| "state")?
            .submissions
            .is_empty()
    );
    client.close().await;
    Ok(())
}

#[test]
fn optional_own_key_null_defaults_survive_and_query_arrays_use_csv_rules() -> TestResult {
    let optional = rtao::reads::ReadName::ALL
        .into_iter()
        .find(|name| {
            schema::read_fields(*name).is_ok_and(|fields| {
                fields
                    .iter()
                    .any(|field| field.own_key && field.preserve_null())
            })
        })
        .ok_or("optional own key read")?;
    let fields = schema::read_fields(optional)?;
    // Signature defaults remain independently inspectable without touching a wallet.
    assert!(
        fields
            .iter()
            .any(|field| field.own_key && field.preserve_null())
    );
    let name: rtao::reads::ReadName = "balances".parse()?;
    let array = schema::read_fields(name)?
        .into_iter()
        .find(|field| field.name == "coldkey_ss58s")
        .ok_or("array")?;
    assert_eq!(
        schema::coerce(&array, " alice, bob ")?,
        json!(["alice", "bob"])
    );
    assert_eq!(
        schema::coerce(&array, "[alice,bob]")?,
        json!(["[alice", "bob]"])
    );
    Ok(())
}

#[tokio::test]
async fn m6_query_dispatch_preserves_native_lease_ids_balances_and_historical_state() -> TestResult
{
    let cold = AccountId([21; 32]);
    let hot = AccountId([22; 32]);
    let mut state = FakeState::try_default()?;
    let value = |cost| json!({"beneficiary":cold,"coldkey":cold,"hotkey":hot,"emissions_share":100,"end_block":null,"netuid":4095,"cost":cost});
    state.seed_python(
        "SubtensorModule",
        "SubnetLeases",
        &[json!(u32::MAX)],
        value(u64::MAX),
    )?;
    let old = state.block;
    state.advance(1);
    state.seed_python(
        "SubtensorModule",
        "SubnetLeases",
        &[json!(u32::MAX)],
        value(1),
    )?;
    let (client, chain) = client(state).await?;
    for (block, expected_cost) in [
        (Some(old.to_string()), "τ18,446,744,073.709552765"),
        (None, "τ0.000000001"),
    ] {
        let mut argv = vec![
            "rtao",
            "query",
            "lease",
            "--lease-id",
            "4294967295",
            "--json",
        ];
        if let Some(block) = &block {
            argv.extend(["--block", block.as_str()]);
        }
        let parsed = tree()?.try_get_matches_from(argv)?;
        let read = "lease".parse()?;
        let (ctx, sink) = context(&parsed, &generated::read_reserved(read))?;
        let request =
            QueryRequest::from_matches(&ctx, read, leaf(&parsed)?, &mut PromptHistory::default())?;
        let result = generated::execute_query(&client, &request, &ctx.output).await?;
        assert_eq!(result["lease_id"], json!(u32::MAX));
        assert_eq!(result["netuid"], json!(4095));
        assert!(result["end_block"].is_null());
        assert_eq!(result["cost"], json!(expected_cost));
        assert_eq!(
            serde_json::from_str::<Value>(&sink.text(Stream::Stdout)?)?,
            result
        );
    }
    let parsed = tree()?.try_get_matches_from([
        "rtao",
        "query",
        "lease",
        "--lease-id",
        "4294967296",
        "--json",
    ])?;
    let read = "lease".parse()?;
    let (ctx, _) = context(&parsed, &generated::read_reserved(read))?;
    let request =
        QueryRequest::from_matches(&ctx, read, leaf(&parsed)?, &mut PromptHistory::default())?;
    let observed = chain.requests().len();
    let error = generated::execute_query(&client, &request, &ctx.output)
        .await
        .err()
        .ok_or("invalid native lease id")?;
    assert_eq!(error.code, 2);
    assert_eq!(chain.requests().len(), observed);
    assert!(
        chain
            .state()
            .lock()
            .map_err(|_| "state")?
            .submissions
            .is_empty()
    );
    client.close().await;
    Ok(())
}

#[test]
fn account_list_queries_resolve_every_reference_and_refuse_an_unknown_entry() -> TestResult {
    let alice = AccountId([7; 32]);
    let bob = AccountId([8; 32]);
    for (command, parameter, flag) in [
        ("balances", "coldkey_ss58s", "--coldkeys"),
        ("hotkey-identities", "hotkey_ss58s", "--hotkeys"),
        ("stake-value-for-coldkeys", "coldkey_ss58s", "--coldkeys"),
    ] {
        let read = command.replace('-', "_").parse()?;
        let parsed = tree()?.try_get_matches_from([
            "rtao",
            "query",
            command,
            flag,
            "bob, alice, bob",
            "--json",
        ])?;
        let (mut ctx, _) = context(&parsed, &generated::read_reserved(read))?;
        ctx.books = Books::from_values(
            json!([{"name":"alice","address":alice},{"name":"bob","address":bob}]),
            json!([]),
            json!([]),
        );
        let request =
            QueryRequest::from_matches(&ctx, read, leaf(&parsed)?, &mut PromptHistory::default())?;
        // Each entry is resolved independently; input order and duplicates
        // survive so the read receives exactly the requested account list.
        assert_eq!(request.params[parameter], json!([bob, alice, bob]));
        let field = schema::read_fields(read)?
            .into_iter()
            .find(|field| field.name == parameter)
            .ok_or("account-list field")?;
        assert!(field.help.contains("address-book"));
        assert!(field.help.contains("Comma-separated"));
        assert!(!field.help.contains("Defaults to your wallet"));

        let parsed = tree()?.try_get_matches_from([
            "rtao",
            "query",
            command,
            flag,
            "alice,not-in-book",
            "--json",
        ])?;
        let error =
            QueryRequest::from_matches(&ctx, read, leaf(&parsed)?, &mut PromptHistory::default())
                .err()
                .ok_or("unknown list entry must be refused before connecting")?;
        assert_eq!(error.code, 2);
    }
    Ok(())
}

#[tokio::test]
async fn query_resolves_csv_contact_names_before_actual_batched_read_dispatch() -> TestResult {
    let keys = Keys::new()?;
    let alice = keys.key.account();
    let bob = AccountId([8; 32]);
    let mut state = funded(&keys)?;
    state.seed_python(
        "System",
        "Account",
        &[json!(bob)],
        json!({"data":{"free":42}}),
    )?;
    let (client, _) = client(state).await?;
    let parsed = tree()?.try_get_matches_from([
        "rtao",
        "query",
        "balances",
        "--coldkeys",
        "alice,bob",
        "--json",
    ])?;
    let name = "balances".parse()?;
    let (mut ctx, sink) = context(&parsed, &generated::read_reserved(name))?;
    ctx.books = Books::from_values(
        json!([{"name":"alice","address":alice},{"name":"bob","address":bob}]),
        json!([]),
        json!([]),
    );
    let request =
        QueryRequest::from_matches(&ctx, name, leaf(&parsed)?, &mut PromptHistory::default())?;
    assert_eq!(request.params["coldkey_ss58s"], json!([alice, bob]));
    let result = generated::execute_query(&client, &request, &ctx.output).await?;
    assert_eq!(result[&bob.to_string()], json!("τ0.000000042"));
    assert_eq!(
        serde_json::from_str::<Value>(&sink.text(Stream::Stdout)?)?,
        result
    );
    client.close().await;
    Ok(())
}

#[test]
fn human_grouped_or_matrix_projection_never_changes_machine_json() -> TestResult {
    let name = rtao::reads::ReadName::ALL
        .into_iter()
        .find(|name| name.implemented() && name.spec().render.is_some())
        .ok_or("render hint")?;
    let payload = match name.spec().render {
        Some(rtao::reads::RenderHint::Grouped { .. }) => {
            json!({"owner":[{"amount":"btcli user data","raw":{"opaque":true}}]})
        }
        Some(rtao::reads::RenderHint::Matrix { .. }) => json!({"row":{"column":0.25}}),
        _ => return Err("missing hint".into()),
    };
    let sink = MemorySink::default();
    let output = Output::new(
        OutputOptions {
            json_mode: true,
            ..Default::default()
        },
        Arc::new(sink.clone()),
    );
    generated::render_query(&output, name, &payload)?;
    assert_eq!(
        serde_json::from_str::<Value>(&sink.text(Stream::Stdout)?)?,
        payload
    );
    Ok(())
}

#[test]
fn hyperparameter_union_preserves_text_bool_and_exact_integer_until_native_conversion() -> TestResult
{
    for (name, input, expected) in [
        ("epoch_consensus", "Yuma", json!("Yuma")),
        ("registration_allowed", "true", json!(1)),
        (
            "weights_version",
            "9007199254740993",
            json!(9_007_199_254_740_993u64),
        ),
    ] {
        let parsed = tree()?.try_get_matches_from([
            "rtao",
            "tx",
            "set-hyperparameter",
            "--netuid",
            "1",
            "--name",
            name,
            "--value",
            input,
            "--yes",
        ])?;
        let (mut ctx, _) = context(&parsed, &generated::intent_reserved("set_hyperparameter")?)?;
        let request = TxRequest::from_matches(
            &mut ctx,
            "set_hyperparameter",
            leaf(&parsed)?,
            &mut PromptHistory::default(),
        )?;
        assert_eq!(request.args["value"], json!(input));
        assert_eq!(request.intent()?.to_json()["value"], expected);
    }
    // This union's native field is Number. It is discovered from generator
    // field_type rather than treating every union as a floating point number.
    let parsed = tree()?.try_get_matches_from([
        "rtao",
        "tx",
        "pow-register",
        "--netuid",
        "1",
        "--work-block",
        "100",
        "--nonce",
        "0",
        "--work-hex",
        &"00".repeat(32),
        "--hotkey",
        &AccountId([7; 32]).to_string(),
        "--mining-timeout",
        "9007199254740993",
        "--yes",
    ])?;
    let (mut ctx, _) = context(&parsed, &generated::intent_reserved("pow_register")?)?;
    let request = TxRequest::from_matches(
        &mut ctx,
        "pow_register",
        leaf(&parsed)?,
        &mut PromptHistory::default(),
    )?;
    assert_eq!(
        request.intent()?.to_json()["mining_timeout"],
        json!(9_007_199_254_740_993u64)
    );
    Ok(())
}

#[test]
fn all_overrides_trimmed_empty_amount_before_native_money_validation() -> TestResult {
    let parsed = tree()?.try_get_matches_from([
        "rtao",
        "tx",
        "transfer",
        "--dest",
        &AccountId([7; 32]).to_string(),
        "--all",
        "--amount",
        "  ",
        "--yes",
    ])?;
    let (mut ctx, _) = context(&parsed, &generated::intent_reserved("transfer")?)?;
    let request = TxRequest::from_matches(
        &mut ctx,
        "transfer",
        leaf(&parsed)?,
        &mut PromptHistory::default(),
    )?;
    assert_eq!(request.args["amount_tao"], json!("all"));
    assert_eq!(request.intent()?.to_json()["amount_tao"], json!("all"));
    Ok(())
}

struct PromptTerminal {
    answers: VecDeque<String>,
    messages: Arc<Mutex<Vec<String>>>,
}
impl Terminal for PromptTerminal {
    fn stdin_is_tty(&self) -> bool {
        true
    }
    fn stderr_is_tty(&self) -> bool {
        true
    }
    fn read_line(&mut self, label: &str) -> Result<String, PromptError> {
        self.messages
            .lock()
            .map_err(|_| PromptError::Io)?
            .push(label.into());
        self.answers.pop_front().ok_or(PromptError::Unavailable)
    }
    fn read_secret(&mut self, _: &str) -> Result<rtao::wallet::SecretString, PromptError> {
        Err(PromptError::Unavailable)
    }
    fn message(&mut self, message: &str) -> Result<(), PromptError> {
        self.messages
            .lock()
            .map_err(|_| PromptError::Io)?
            .push(message.into());
        Ok(())
    }
}

#[tokio::test]
async fn transfer_amount_prompt_reads_proxy_free_balance_and_records_public_replay() -> TestResult {
    let keys = Keys::new()?;
    let proxy = AccountId([9; 32]);
    let mut state = funded(&keys)?;
    state.seed_python(
        "System",
        "Account",
        &[json!(proxy)],
        json!({"data":{"free":2_500_000_000_000_u64}}),
    )?;
    let (client, chain) = client(state).await?;
    let address = keys.key.account().to_string();
    let proxy_address = proxy.to_string();
    for (proxy_ref, configured) in [
        (proxy_address.as_str(), false),
        ("ProxyAlias", false),
        ("ProxyAlias", true),
    ] {
        let mut argv = vec!["rtao", "tx", "transfer", "-w", "chosen", "--dest", &address];
        if !configured {
            argv.extend(["--proxy-for", proxy_ref]);
        }
        let parsed = tree()?.try_get_matches_from(argv)?;
        let (mut ctx, _) = context(&parsed, &generated::intent_reserved("transfer")?)?;
        ctx.books.addresses = vec![json!({"name":"ProxyAlias","address":proxy})];
        if configured {
            ctx.config.insert("proxy_for".into(), json!(proxy_ref));
        }
        let messages = Arc::new(Mutex::new(Vec::new()));
        ctx.terminal = Arc::new(Mutex::new(Box::new(PromptTerminal {
            answers: VecDeque::from(["1.000000001".into()]),
            messages: messages.clone(),
        })));
        let mut history = PromptHistory::default();
        let request = TxRequest::from_matches_with_client(
            &mut ctx,
            &client,
            "transfer",
            leaf(&parsed)?,
            &mut history,
        )
        .await?;
        assert_eq!(request.args["amount_tao"], json!("1.000000001"));
        {
            let messages = messages.lock().map_err(|_| "messages")?;
            let hint = messages
                .iter()
                .position(|s| {
                    s.starts_with(&format!(
                        "--amount-tao: free balance of {proxy_ref}: τ2,500.000000000\\n"
                    ))
                })
                .ok_or("live free balance hint")?;
            assert!(!messages[hint].chars().any(char::is_control));
            let prompt = messages
                .iter()
                .position(|s| s.starts_with("  amount-tao"))
                .ok_or("amount prompt")?;
            assert!(hint < prompt);
        }
        assert!(
            history
                .replay_command(&["rtao".into(), "tx".into(), "transfer".into()])
                .contains("--amount-tao 1.000000001")
        );
        assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
        assert!(
            chain
                .state()
                .lock()
                .map_err(|_| "state")?
                .submissions
                .is_empty()
        );
    }
    client.close().await;
    Ok(())
}

fn pow_state(keys: &Keys) -> Result<FakeState, Box<dyn std::error::Error>> {
    let mut state = FakeState::new(FakeState::try_default()?.runtime);
    state.set_finalized(98)?;
    state.seed_python(
        "System",
        "Account",
        &[json!(keys.key.account())],
        json!({"data":{"free":1_000_000_000_000u64}}),
    )?;
    let netuid = NetUid(1);
    for (item, enabled) in [
        ("NetworksAdded", true),
        ("NetworkRegistrationAllowed", false),
        ("NetworkPowRegistrationAllowed", true),
    ] {
        state.seed_typed("SubtensorModule", item, (netuid,), enabled)?;
    }
    for (item, value) in [
        ("Difficulty", 1),
        ("MinDifficulty", 1),
        ("MaxDifficulty", u64::MAX),
    ] {
        state.seed_typed("SubtensorModule", item, (netuid,), value)?;
    }
    for (item, value) in [
        ("MaxAllowedUids", 16u16),
        ("MaxRegistrationsPerBlock", 1u16),
        ("RegistrationsThisBlock", 0u16),
    ] {
        state.seed_typed("SubtensorModule", item, (netuid,), value)?;
    }
    Ok(state)
}
async fn pow_request(
    client: &Client,
    keys: &Keys,
) -> Result<TxRequest, Box<dyn std::error::Error>> {
    let hash = client.rpc().block_hash(100).await?.ok_or("work hash")?;
    let prefix = rtao_intents::pow::registration_prefix(
        NetUid(1),
        hash,
        keys.key.account(),
        keys.key.account(),
    )?;
    let work = rtao::core::pow::seal(&prefix, 0)?;
    Ok(TxRequest {
        op: "pow_register",
        args: serde_json::Map::from_iter([
            ("netuid".into(), json!(1)),
            ("hotkey_ss58".into(), json!(keys.key.account())),
            ("work_block".into(), json!(100)),
            ("nonce".into(), json!(0)),
            ("work_hex".into(), json!(hex::encode(work))),
            ("mining_workers".into(), json!(1)),
            ("mining_timeout".into(), json!(10)),
            ("mining_backend".into(), json!("cpu")),
        ]),
        period: None,
    })
}

#[tokio::test]
async fn initially_expired_future_and_difficulty_invalid_pow_prepare_before_signing() -> TestResult
{
    for (work_block, difficulty) in [(95u64, 1u64), (105, 1), (100, 2)] {
        let keys = Keys::new()?;
        let owner = AccountId([48; 32]);
        let hotkey = AccountId([47; 32]);
        assert_ne!(keys.key.account(), owner);
        assert_ne!(keys.key.account(), hotkey);
        let mut state = pow_state(&keys)?;
        state.seed_typed("SubtensorModule", "Difficulty", (NetUid(1),), difficulty)?;
        let (client, chain) = client(state).await?;
        let mut request = pow_request(&client, &keys).await?;
        let hash = client
            .rpc()
            .block_hash(work_block.min(100))
            .await?
            .ok_or("original work hash")?;
        let prefix = rtao_intents::pow::registration_prefix(NetUid(1), hash, hotkey, owner)?;
        let nonce = if difficulty == 1 {
            0
        } else {
            let mut invalid = None;
            for candidate in 0..1000 {
                if rtao::core::pow::mine_cpu(&prefix, difficulty, candidate, 1)?.is_none() {
                    invalid = Some(candidate);
                    break;
                }
            }
            invalid.ok_or("difficulty-invalid fixture nonce")?
        };
        request.args.insert("work_block".into(), json!(work_block));
        request.args.insert("nonce".into(), json!(nonce));
        request.args.insert("hotkey_ss58".into(), json!(hotkey));
        request.args.insert(
            "work_hex".into(),
            json!(hex::encode(rtao::core::pow::seal(&prefix, nonce)?)),
        );
        let original = request.intent()?;
        assert!(
            rtao::Executor::new(client.clone())
                .plan_dyn(
                    original.as_dyn(),
                    &keys,
                    &rtao::tx::PlanOptions {
                        proxy_for: Some(owner),
                        ..Default::default()
                    },
                )
                .await
                .is_err()
        );
        let owner_flag = owner.to_string();
        let parsed = tree()?.try_get_matches_from([
            "rtao",
            "tx",
            "pow-register",
            "--json",
            "--yes",
            "--proxy-for",
            &owner_flag,
        ])?;
        let (ctx, _) = context(&parsed, &generated::intent_reserved("pow_register")?)?;
        let hooks = Hooks {
            public_preparation_unlocks: Some(keys.unlocks.clone()),
            ..Default::default()
        };
        let result = generated::execute_tx_with_keys(
            &ctx,
            &client,
            &request,
            &keys,
            SubmissionOptions {
                hooks: Some(&hooks),
                ..Default::default()
            },
        )
        .await?
        .ok_or("receipt")?;
        assert!(result.success);
        assert_eq!(result.data["proxy_for"], json!(owner));
        assert_eq!(keys.unlocks.load(Ordering::SeqCst), 1);
        assert_eq!(hooks.before.load(Ordering::SeqCst), 1);
        assert_eq!(hooks.results.load(Ordering::SeqCst), 1);
        assert_eq!(request.intent()?, original);
        let submissions = chain
            .state()
            .lock()
            .map_err(|_| "state")?
            .submissions
            .clone();
        assert_eq!(submissions.len(), 1);
        let runtime = client.refresh_runtime().await?;
        let decoded =
            rtao::core::DynValue::new(runtime.core().decode_extrinsic(&submissions[0], true)?)
                .to_json();
        let proxy = &decoded["call"];
        assert_eq!(proxy["call_function"], json!("proxy"));
        let proxy_args = proxy["call_args"].as_array().ok_or("proxy args")?;
        let real = proxy_args
            .iter()
            .find(|arg| arg["name"] == "real")
            .map(|arg| &arg["value"])
            .ok_or("proxy owner")?;
        assert_eq!(real, &json!(owner));
        let call = proxy_args
            .iter()
            .find(|arg| arg["name"] == "call")
            .map(|arg| &arg["value"])
            .ok_or("inner call")?;
        assert_eq!(call["call_function"], json!("pow_register"));
        let args = call["call_args"].as_array().ok_or("call args")?;
        let arg = |name: &str| {
            args.iter()
                .find(|arg| arg["name"] == name)
                .map(|arg| &arg["value"])
        };
        assert_eq!(arg("work_block"), Some(&json!(100)));
        assert_eq!(arg("hotkey"), Some(&json!(hotkey)));
        let nonce = arg("nonce").and_then(Value::as_u64).ok_or("nonce")?;
        let hash = client.rpc().block_hash(100).await?.ok_or("fresh hash")?;
        let prefix = rtao_intents::pow::registration_prefix(NetUid(1), hash, hotkey, owner)?;
        let expected = rtao::core::pow::mine_cpu(&prefix, difficulty, nonce, 1)?
            .ok_or("submitted proof must satisfy the current difficulty")?
            .1;
        assert_eq!(
            arg("work"),
            Some(
                &rtao::core::DynValue::new(rtao::core::codec::Value::Bytes(expected.to_vec()))
                    .to_json()
            )
        );
        client.close().await;
    }
    Ok(())
}

#[tokio::test]
async fn expired_pow_dry_run_never_enters_public_mining_or_unlocks() -> TestResult {
    let keys = Keys::new()?;
    let (client, chain) = client(pow_state(&keys)?).await?;
    let mut request = pow_request(&client, &keys).await?;
    request.args.insert("work_block".into(), json!(95));
    // Preparation would refuse this backend before solving. The dry run must
    // instead retain strict SDK freshness validation without entering mining.
    request
        .args
        .insert("mining_backend".into(), json!("disabled-fixture"));
    let parsed = tree()?.try_get_matches_from([
        "rtao",
        "tx",
        "pow-register",
        "--json",
        "--yes",
        "--dry-run",
    ])?;
    let (ctx, _) = context(&parsed, &generated::intent_reserved("pow_register")?)?;
    let error = generated::execute_tx_with_keys(
        &ctx,
        &client,
        &request,
        &keys,
        SubmissionOptions::default(),
    )
    .await
    .err()
    .ok_or("strict dry-run freshness refusal")?;
    assert!(error.message.contains("work block"));
    assert!(!error.message.contains("supports CPU PoW refresh"));
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    assert!(
        chain
            .state()
            .lock()
            .map_err(|_| "state")?
            .submissions
            .is_empty()
    );
    client.close().await;
    Ok(())
}

struct ExpiringKeys {
    keys: Keys,
    chain: FakeChain,
}
impl KeyView for ExpiringKeys {
    fn coldkey_address(&self) -> Result<AccountId, SignerError> {
        self.keys.coldkey_address()
    }
    fn hotkey_address(&self) -> Result<AccountId, SignerError> {
        self.keys.hotkey_address()
    }
    fn hotkey_public_key(&self) -> Result<[u8; 32], SignerError> {
        self.keys.hotkey_public_key()
    }
}
impl DynKeySource for ExpiringKeys {
    fn signer_for<'a>(
        &'a self,
        role: SignerRole,
    ) -> BoxFuture<'a, Result<Arc<dyn Signer>, SignerError>> {
        Box::pin(async move {
            self.chain
                .advance(5)
                .map_err(|e| SignerError::KeyView(e.to_string()))?;
            self.keys.signer_for(role).await
        })
    }
}

#[tokio::test]
async fn pow_uses_real_cpu_refresh_after_unlock_delay_and_only_one_cached_signer() -> TestResult {
    let keys = Keys::new()?;
    let (client, chain) = client(pow_state(&keys)?).await?;
    let request = pow_request(&client, &keys).await?;
    let parsed = tree()?.try_get_matches_from(["rtao", "tx", "pow-register", "--json", "--yes"])?;
    let (ctx, _) = context(&parsed, &generated::intent_reserved("pow_register")?)?;
    let expiring = ExpiringKeys {
        keys: keys.clone(),
        chain: chain.clone(),
    };
    let hooks = Hooks::default();
    let result = generated::execute_tx_with_keys(
        &ctx,
        &client,
        &request,
        &expiring,
        SubmissionOptions {
            hooks: Some(&hooks),
            ..Default::default()
        },
    )
    .await?
    .ok_or("receipt")?;
    assert!(result.success);
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 1);
    assert_eq!(hooks.before.load(Ordering::SeqCst), 1);
    assert_eq!(hooks.results.load(Ordering::SeqCst), 1);
    let submissions = chain
        .state()
        .lock()
        .map_err(|_| "state")?
        .submissions
        .clone();
    assert_eq!(submissions.len(), 1);
    let runtime = client.refresh_runtime().await?;
    let decoded =
        rtao::core::DynValue::new(runtime.core().decode_extrinsic(&submissions[0], true)?)
            .to_json();
    assert_eq!(decoded["call"]["call_function"], json!("pow_register"));
    let args = decoded["call"]["call_args"].as_array().ok_or("call args")?;
    let work_block = args
        .iter()
        .find(|arg| arg["name"] == "work_block")
        .ok_or("work block")?;
    assert_eq!(work_block["value"], json!(105));
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn pow_preview_and_fresh_invalid_seal_cannot_start_signing_or_replace_the_proof() -> TestResult
{
    let keys = Keys::new()?;
    let (client, chain) = client(pow_state(&keys)?).await?;
    let mut request = pow_request(&client, &keys).await?;
    let parsed = tree()?.try_get_matches_from([
        "rtao",
        "tx",
        "pow-register",
        "--json",
        "--yes",
        "--dry-run",
    ])?;
    let (ctx, _) = context(&parsed, &generated::intent_reserved("pow_register")?)?;
    assert!(
        generated::execute_tx_with_keys(
            &ctx,
            &client,
            &request,
            &keys,
            SubmissionOptions::default(),
        )
        .await?
        .is_none()
    );
    request
        .args
        .insert("work_hex".into(), json!("00".repeat(32)));
    let original = request.intent()?;
    let refreshed =
        rtao_cli::commands::pow::refresh_if_needed(&client, original.clone(), &keys).await?;
    assert_eq!(refreshed, original);
    assert!(
        rtao::Executor::new(client.clone())
            .plan_dyn(refreshed.as_dyn(), &keys, &Default::default())
            .await
            .is_err()
    );
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    assert!(
        chain
            .state()
            .lock()
            .map_err(|_| "state")?
            .submissions
            .is_empty()
    );
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn pow_retries_only_three_confirmed_expired_pool_rejections() -> TestResult {
    let keys = Keys::new()?;
    let (client, chain) = client(pow_state(&keys)?).await?;
    let request = pow_request(&client, &keys).await?;
    let attempts = Arc::new(AtomicUsize::new(0));
    let counted = attempts.clone();
    chain.mutate_state(|state| {
        state.set_handler("author_submitAndWatchExtrinsic", move |state, _| {
            counted.fetch_add(1, Ordering::SeqCst);
            state.advance(5);
            rtao_testkit::Reply::error(1010, "Invalid Transaction", Some(json!({"Custom":255})))
        })
    });
    let parsed = tree()?.try_get_matches_from(["rtao", "tx", "pow-register", "--json", "--yes"])?;
    let (ctx, sink) = context(&parsed, &generated::intent_reserved("pow_register")?)?;
    let hooks = Hooks::default();
    let failure = generated::execute_tx_with_keys(
        &ctx,
        &client,
        &request,
        &keys,
        SubmissionOptions {
            hooks: Some(&hooks),
            ..Default::default()
        },
    )
    .await
    .err()
    .ok_or("final rejected receipt")?;
    assert_eq!(failure.code, 1);
    assert_eq!(attempts.load(Ordering::SeqCst), 3);
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 1);
    assert_eq!(hooks.before.load(Ordering::SeqCst), 1);
    assert_eq!(hooks.results.load(Ordering::SeqCst), 1);
    let receipt: Value = serde_json::from_str(&sink.text(Stream::Stdout)?)?;
    assert_eq!(receipt["success"], json!(false));
    assert_eq!(receipt["error"]["name"], json!("BadRequest"));
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn pow_fresh_pool_rejection_does_not_retry_even_when_name_matches() -> TestResult {
    let keys = Keys::new()?;
    let (client, chain) = client(pow_state(&keys)?).await?;
    let request = pow_request(&client, &keys).await?;
    let attempts = Arc::new(AtomicUsize::new(0));
    let counted = attempts.clone();
    chain.mutate_state(|state| {
        state.set_handler("author_submitAndWatchExtrinsic", move |_, _| {
            counted.fetch_add(1, Ordering::SeqCst);
            rtao_testkit::Reply::error(1010, "Invalid Transaction", Some(json!({"Custom":255})))
        })
    });
    let parsed = tree()?.try_get_matches_from(["rtao", "tx", "pow-register", "--json", "--yes"])?;
    let (ctx, _) = context(&parsed, &generated::intent_reserved("pow_register")?)?;
    assert!(
        generated::execute_tx_with_keys(
            &ctx,
            &client,
            &request,
            &keys,
            SubmissionOptions::default(),
        )
        .await
        .is_err()
    );
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    client.close().await;
    Ok(())
}

#[test]
fn synchronous_query_and_lock_pickers_keep_natural_choices_duplicate_keys_and_python_numbers()
-> TestResult {
    let root = TempRoot::new();
    let first_wallet = AccountId([61; 32]);
    let second_wallet = AccountId([62; 32]);
    let contact = AccountId([63; 32]);
    for (name, account) in [
        ("wallet10", second_wallet),
        ("wallet2", first_wallet),
        ("configured", AccountId([64; 32])),
    ] {
        let directory = root.path().join(name);
        std::fs::create_dir(&directory)?;
        std::fs::write(
            directory.join("coldkeypub.txt"),
            serde_json::to_vec(&json!({"ss58Address":account,"cryptoType":1}))?,
        )?;
    }
    let hotkeys = root.path().join("configured/hotkeys");
    std::fs::create_dir(&hotkeys)?;
    for (name, account) in [
        ("worker10", first_wallet),
        ("worker2", first_wallet),
        ("worker1", second_wallet),
    ] {
        std::fs::write(
            hotkeys.join(name),
            serde_json::to_vec(&json!({"ss58Address":account,"cryptoType":1}))?,
        )?;
    }
    let call_hash = format!("0x{}", "ab".repeat(32));
    for answer in ["2", "٢"] {
        let argv = [
            "rtao",
            "query",
            "multisig",
            "--call-hash",
            &call_hash,
            "--wallet",
            "configured",
        ];
        let parsed = tree()?.try_get_matches_from(argv)?;
        let (mut ctx, _) = context(
            &parsed,
            &generated::read_reserved(rtao::reads::ReadName::Multisig),
        )?;
        ctx.globals.wallet_path = root.path().to_owned();
        ctx.passwords.native_provider = None;
        ctx.books.addresses = vec![
            json!({"name":"empty must not consume a number","address":""}),
            json!({"name":"contact","address":contact}),
            json!({"name":"duplicate contact","address":contact}),
        ];
        let messages = Arc::new(Mutex::new(Vec::new()));
        ctx.terminal = Arc::new(Mutex::new(Box::new(PromptTerminal {
            answers: VecDeque::from([answer.into()]),
            messages: messages.clone(),
        })));
        let mut history = PromptHistory::default();
        let request = QueryRequest::from_matches(
            &ctx,
            rtao::reads::ReadName::Multisig,
            leaf(&parsed)?,
            &mut history,
        )?;
        assert_eq!(request.params["account_ss58"], json!(first_wallet));
        assert_eq!(request.params["call_hash"], call_hash);
        assert_eq!(
            history.replay_command(&argv.map(str::to_owned)),
            format!("{} --account {first_wallet}", argv.join(" "))
        );
        assert_eq!(
            ctx.output
                .address_names()?
                .get(&first_wallet.to_string())
                .map(String::as_str),
            Some("wallet2")
        );
        let messages = messages.lock().map_err(|_| "query picker messages")?;
        assert!(
            messages
                .iter()
                .any(|line| line == "--account: Destination account — a coldkey, not a hotkey.")
        );
        assert!(messages.iter().any(|line| line == "a number above, or any account: ss58 address, address-book name, or local wallet name"));
        for expected in [
            format!("1  contact  {contact}"),
            format!("2  wallet2  {first_wallet}  wallet"),
            format!("3  wallet10  {second_wallet}  wallet"),
        ] {
            assert!(
                messages.iter().any(|line| line == &expected),
                "choice {expected}"
            );
        }
        assert!(
            !messages
                .iter()
                .any(|line| line.contains("empty must") || line.contains("duplicate contact"))
        );
    }
    for (answer, expected_name) in [("3", "configured/worker10"), ("２", "configured/worker2")] {
        let argv = [
            "rtao",
            "conviction",
            "move",
            "--netuid",
            "3",
            "--wallet",
            "configured",
        ];
        let parsed = application::command_tree([rtao_cli::commands::conviction::command()?])
            .try_get_matches_from(argv)?;
        let (mut ctx, _) = context(&parsed, &generated::intent_reserved("move_lock")?)?;
        ctx.globals.wallet_path = root.path().to_owned();
        ctx.passwords.native_provider = None;
        ctx.books.addresses =
            vec![json!({"name":"contact-cannot-shadow-local-key","address":first_wallet})];
        let messages = Arc::new(Mutex::new(Vec::new()));
        ctx.terminal = Arc::new(Mutex::new(Box::new(PromptTerminal {
            answers: VecDeque::from([answer.into()]),
            messages: messages.clone(),
        })));
        let mut history = PromptHistory::default();
        let requests = rtao_cli::commands::conviction::tx_requests(
            &mut ctx,
            parsed.subcommand().ok_or("conviction group")?.1,
            &mut history,
        )?;
        assert_eq!(requests.len(), 1);
        let request = &requests[0];
        assert_eq!(request.op, "move_lock");
        assert_eq!(request.args["destination_hotkey_ss58"], json!(first_wallet));
        assert_eq!(
            request.intent()?.as_dyn().to_json_dyn()["destination_hotkey_ss58"],
            json!(first_wallet)
        );
        assert_eq!(
            history.replay_command(&argv.map(str::to_owned)),
            format!("{} --destination-hotkey {first_wallet}", argv.join(" "))
        );
        assert_eq!(
            ctx.output
                .address_names()?
                .get(&first_wallet.to_string())
                .map(String::as_str),
            Some(expected_name)
        );
        let messages = messages.lock().map_err(|_| "target picker messages")?;
        assert!(messages.iter().any(|line| line == "--destination-hotkey: Validator hotkey the stake goes to — it does not have to be a local key."));
        assert!(messages.iter().any(|line| line
            == "a number above, or any hotkey: ss58 address, address-book name, or WALLET/HOTKEY"));
        for expected in [
            format!("1  configured/worker1  {second_wallet}"),
            format!("2  configured/worker2  {first_wallet}"),
            format!("3  configured/worker10  {first_wallet}"),
        ] {
            assert!(
                messages.iter().any(|line| line == &expected),
                "choice {expected}"
            );
        }
    }
    for hotkey in [false, true] {
        let argv = if hotkey {
            vec![
                "rtao",
                "conviction",
                "move",
                "--netuid",
                "3",
                "--wallet",
                "configured",
            ]
        } else {
            vec![
                "rtao",
                "query",
                "multisig",
                "--call-hash",
                &call_hash,
                "--wallet",
                "configured",
            ]
        };
        let command = if hotkey {
            application::command_tree([rtao_cli::commands::conviction::command()?])
        } else {
            tree()?
        };
        let parsed = command.try_get_matches_from(argv)?;
        let reserved = if hotkey {
            generated::intent_reserved("move_lock")?
        } else {
            generated::read_reserved(rtao::reads::ReadName::Multisig)
        };
        let (mut ctx, _) = context(&parsed, &reserved)?;
        ctx.globals.wallet_path = root.path().join("absent-public-wallet-directory");
        ctx.passwords.native_provider = None;
        let messages = Arc::new(Mutex::new(Vec::new()));
        ctx.terminal = Arc::new(Mutex::new(Box::new(PromptTerminal {
            answers: VecDeque::from([first_wallet.to_string()]),
            messages: messages.clone(),
        })));
        let mut history = PromptHistory::default();
        if hotkey {
            let requests = rtao_cli::commands::conviction::tx_requests(
                &mut ctx,
                parsed.subcommand().ok_or("conviction group")?.1,
                &mut history,
            )?;
            assert_eq!(
                requests[0].args["destination_hotkey_ss58"],
                json!(first_wallet)
            );
        } else {
            let request = QueryRequest::from_matches(
                &ctx,
                rtao::reads::ReadName::Multisig,
                leaf(&parsed)?,
                &mut history,
            )?;
            assert_eq!(request.params["account_ss58"], json!(first_wallet));
        }
        let expected = if hotkey {
            "wallet 'configured' has no local hotkeys — paste the validator's ss58 address or an address-book name"
        } else {
            "no address-book contacts or other local wallets — paste the destination ss58"
        };
        let messages = messages.lock().map_err(|_| "empty picker messages")?;
        assert!(messages.iter().any(|line| line == expected));
        assert!(!messages.iter().any(|line| line.starts_with("1  ")));
    }
    // Only public parser/request paths ran: no client, signer or private file
    // exists in this fixture. Scripted secret input refuses if reached.
    Ok(())
}
