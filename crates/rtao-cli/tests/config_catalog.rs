// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/commands/config.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Command-boundary fixtures for persistent defaults and catalog discovery.

use clap::ArgMatches;
use rtao::client::config::{ConfigFile, Paths, Store};
use rtao_cli::{
    application,
    commands::{catalog, config},
    error::CliError,
    output::{DiagnosticContext, MemorySink, Output, OutputOptions, Stream},
};
use serde_json::{Value, json};
use std::{
    error::Error,
    fs,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

type TestResult = Result<(), Box<dyn Error>>;

struct TestStore {
    root: PathBuf,
    store: Store,
}
impl TestStore {
    fn new() -> Result<Self, Box<dyn Error>> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let base = std::env::var_os("RTAO_TEMP_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/config-command-tests")
            })
            .join("tmp");
        fs::create_dir_all(&base)?;
        let root = base.join(format!(
            "{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root)?;
        let store = Store::new(Paths::new(&root, |_| None));
        Ok(Self { root, store })
    }
    fn path(&self) -> &std::path::Path {
        self.store.paths().path(ConfigFile::Config)
    }
    fn write(&self, data: &Value) -> TestResult {
        fs::create_dir_all(self.path().parent().ok_or("config parent")?)?;
        fs::write(self.path(), serde_json::to_vec(data)?)?;
        Ok(())
    }
}
impl Drop for TestStore {
    fn drop(&mut self) {
        // No process or service is spawned by these fixtures. Awaited store
        // operations have completed and the injected paths own this directory.
        if self.root.file_name().is_some_and(|name| {
            name.to_string_lossy()
                .starts_with(&format!("{}-", std::process::id()))
        }) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}
fn capture(json_mode: bool, quiet: bool) -> (Output, MemorySink) {
    let sink = MemorySink::default();
    (
        Output::with_sink(
            OutputOptions {
                json_mode,
                quiet,
                network: None,
            },
            Arc::new(sink.clone()),
        ),
        sink,
    )
}
fn parsed(args: &[&str]) -> Result<ArgMatches, Box<dyn Error>> {
    Ok(application::command_tree([
        config::command(),
        catalog::tools_command(),
        catalog::explain_command(),
    ])
    .try_get_matches_from(args)?)
}
async fn config_run(fixture: &TestStore, args: &[&str], output: &Output) -> Result<(), CliError> {
    let matches = parsed(args).map_err(|error| CliError::input(error.to_string()))?;
    let (_, group) = matches
        .subcommand()
        .ok_or_else(|| CliError::input("missing group"))?;
    config::run(&fixture.store, output, group).await
}
fn payload(sink: &MemorySink) -> Result<Value, Box<dyn Error>> {
    Ok(serde_json::from_str(&sink.text(Stream::Stdout)?)?)
}
fn explain(args: &[&str], output: &Output) -> Result<(), CliError> {
    let matches = parsed(args).map_err(|error| CliError::input(error.to_string()))?;
    catalog::run_explain(
        output,
        matches
            .subcommand()
            .ok_or_else(|| CliError::input("missing explain"))?
            .1,
    )
}

#[tokio::test]
async fn config_set_get_unset_roundtrip_keeps_exact_command_payloads() -> TestResult {
    let fixture = TestStore::new()?;
    let (out, sink) = capture(true, false);
    config_run(
        &fixture,
        &["rtao", "config", "set", "network", "test", "--json"],
        &out,
    )
    .await?;
    assert_eq!(
        payload(&sink)?,
        json!({"key":"network","value":"test","path":fixture.path().to_string_lossy()})
    );
    assert_eq!(
        fixture.store.load().await?.get("network"),
        Some(&json!("test"))
    );
    let (out, sink) = capture(true, false);
    config_run(&fixture, &["rtao", "config", "get", "network"], &out).await?;
    assert_eq!(payload(&sink)?, json!("test"));
    let (out, sink) = capture(true, false);
    config_run(&fixture, &["rtao", "config", "unset", "network"], &out).await?;
    assert_eq!(payload(&sink)?, json!({"key":"network","existed":true}));
    let (out, sink) = capture(true, false);
    config_run(&fixture, &["rtao", "config", "unset", "network"], &out).await?;
    assert_eq!(payload(&sink)?, json!({"key":"network","existed":false}));
    Ok(())
}

#[tokio::test]
async fn config_get_empty_all_and_missing_key_preserve_python_values() -> TestResult {
    let fixture = TestStore::new()?;
    for (args, expected) in [
        (vec!["rtao", "config", "get"], json!({"(empty)":""})),
        (vec!["rtao", "config", "get", "wallet"], Value::Null),
        (
            vec!["rtao", "config", "path"],
            json!(fixture.path().to_string_lossy()),
        ),
    ] {
        let (out, sink) = capture(true, true);
        config_run(&fixture, &args, &out).await?;
        assert_eq!(payload(&sink)?, expected);
        assert!(sink.bytes(Stream::Stderr)?.is_empty());
    }
    assert!(!fixture.path().exists());
    fixture.write(&json!({"wallet":"demo","json":false,"network":"test","unknown":42}))?;
    let (out, sink) = capture(true, false);
    config_run(&fixture, &["rtao", "config", "get"], &out).await?;
    assert_eq!(
        payload(&sink)?,
        json!({"wallet":"demo","json":false,"network":"test"})
    );
    Ok(())
}

#[tokio::test]
async fn config_invalid_values_and_keys_return_status_one_without_mutating() -> TestResult {
    let fixture = TestStore::new()?;
    fixture.write(&json!({"wallet":"unchanged"}))?;
    let before = fs::read(fixture.path())?;
    for args in [
        vec!["rtao", "config", "set", "json", "sometimes"],
        vec!["rtao", "config", "set", "bad-key", "value"],
        vec!["rtao", "config", "unset", "bad-key"],
        vec!["rtao", "config", "get", "bad-key"],
    ] {
        let (out, sink) = capture(true, false);
        let error = config_run(&fixture, &args, &out)
            .await
            .err()
            .ok_or("invalid config accepted")?;
        assert_eq!(error.code, 1);
        assert!(sink.bytes(Stream::Stdout)?.is_empty());
        assert!(
            sink.bytes(Stream::Stderr)?.is_empty(),
            "application must render the diagnostic once"
        );
        out.error(
            &error.message,
            DiagnosticContext {
                help: error.help.as_deref(),
                ..Default::default()
            },
        )?;
        let diagnostic: Value = serde_json::from_str(&sink.text(Stream::Stderr)?)?;
        assert_eq!(diagnostic["error"], error.message);
        if args[2] == "get" {
            assert_eq!(diagnostic["help"], error.help.ok_or("missing get help")?);
        }
        assert_eq!(fs::read(fixture.path())?, before);
    }
    Ok(())
}

#[tokio::test]
async fn config_clear_matches_empty_unknown_and_null_unset_behavior() -> TestResult {
    let fixture = TestStore::new()?;
    let (out, sink) = capture(true, false);
    config_run(&fixture, &["rtao", "config", "clear"], &out).await?;
    assert_eq!(
        payload(&sink)?,
        json!({"path":fixture.path().to_string_lossy()})
    );
    assert!(
        !fixture.path().exists(),
        "clear of a missing file creates no file"
    );
    fixture.write(&json!({"foreign":42}))?;
    let before = fs::read(fixture.path())?;
    let (out, _) = capture(true, false);
    config_run(&fixture, &["rtao", "config", "clear"], &out).await?;
    assert_eq!(
        fs::read(fixture.path())?,
        before,
        "unknown-only config is not rewritten"
    );
    fixture.write(&json!({"wallet":null,"json":true,"foreign":42}))?;
    let (out, sink) = capture(true, false);
    config_run(&fixture, &["rtao", "config", "unset", "wallet"], &out).await?;
    assert_eq!(payload(&sink)?, json!({"key":"wallet","existed":false}));
    let (out, _) = capture(true, false);
    config_run(&fixture, &["rtao", "config", "clear"], &out).await?;
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(fixture.path())?)?,
        json!({})
    );
    Ok(())
}

#[tokio::test]
async fn config_set_uses_store_coercions_and_reports_write_failure() -> TestResult {
    let fixture = TestStore::new()?;
    for (key, input, expected) in [
        ("json", "YES", json!(true)),
        ("quiet", "off", json!(false)),
        (
            "fallback_endpoints",
            "wss://one, wss://two,,",
            json!("wss://one, wss://two,,"),
        ),
    ] {
        let (out, sink) = capture(true, false);
        config_run(&fixture, &["rtao", "config", "set", key, input], &out).await?;
        assert_eq!(payload(&sink)?["value"], expected);
    }
    fs::remove_file(fixture.path())?;
    fs::create_dir(fixture.path())?;
    let (out, sink) = capture(true, false);
    let error = config_run(
        &fixture,
        &["rtao", "config", "set", "network", "test"],
        &out,
    )
    .await
    .err()
    .ok_or("directory cannot be atomically replaced by config")?;
    assert_eq!(error.code, 1);
    assert!(sink.bytes(Stream::Stdout)?.is_empty());
    Ok(())
}

#[test]
fn command_parsers_require_values_show_help_and_accept_global_overrides() -> TestResult {
    for args in [
        vec!["rtao", "config", "set", "network"],
        vec!["rtao", "config", "unset"],
        vec!["rtao", "config", "path", "extra"],
    ] {
        let error = parsed(&args).err().ok_or("invalid argv accepted")?;
        assert!(error.to_string().contains("error:"));
    }
    let help = config::command()
        .try_get_matches_from(["config"])
        .err()
        .ok_or("bare config should show help")?;
    assert_eq!(
        help.kind(),
        clap::error::ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
    );
    for args in [
        vec!["rtao", "--json", "conf", "get"],
        vec!["rtao", "c", "set", "network", "test", "--json", "--quiet"],
        vec!["rtao", "tools", "--json", "--network", "test"],
        vec!["rtao", "explain", "--chain", "balances", "--quiet"],
    ] {
        parsed(&args)?;
    }
    assert!(parsed(&["rtao", "config", "clear", "--yes"]).is_err());
    assert!(parsed(&["rtao", "explain", "--chain", "--bad"]).is_err());
    Ok(())
}

#[test]
fn tools_json_equals_python_on_upstream_fields_for_all_83_ops() -> TestResult {
    let payload = catalog::tools_payload(&[])?;
    let rows = payload.as_array().ok_or("catalog array")?;
    assert_eq!(rows.len(), 83);
    let keys = [
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
    for (row, spec) in rows.iter().zip(&rtao_catalog::INTENTS) {
        let source: Value = serde_json::from_str(spec.catalog_json)?;
        assert_eq!(row.as_object().ok_or("tool object")?.len(), 10);
        for key in keys {
            let expected = if key == "docs_url" {
                json!(format!(
                    "{}/tx/{}",
                    rtao_catalog::py::DOCS_URL,
                    spec.name.replace('_', "-")
                ))
            } else {
                source[key].clone()
            };
            assert_eq!(row[key], expected, "{}.{}", spec.name, key);
        }
        assert_eq!(
            row["rust"],
            json!({"type":null,"role":null,"output":null,"cli":null})
        );
    }
    Ok(())
}

#[test]
fn tools_addition_keeps_pending_commands_unadvertised_and_rejects_bad_metadata() -> TestResult {
    let pending = catalog::RustTool {
        name: "add_stake".into(),
        type_name: "rtao::intents::AddStake".into(),
        role: "Coldkey".into(),
        output: "()".into(),
        cli: None,
    };
    let implemented = catalog::RustTool {
        name: "set_weights".into(),
        type_name: "rtao::intents::SetWeights".into(),
        role: "Hotkey".into(),
        output: "rtao::tx::WeightsSubmitted".into(),
        cli: Some("rtao tx set-weights".into()),
    };
    let payload = catalog::tools_payload(&[pending.clone(), implemented])?;
    let rows = payload.as_array().ok_or("catalog rows")?;
    let pending_row = rows
        .iter()
        .find(|row| row["name"] == "add_stake")
        .ok_or("pending tool")?;
    assert_eq!(pending_row["rust"]["cli"], Value::Null);
    let row = rows
        .iter()
        .find(|row| row["name"] == "set_weights")
        .ok_or("implemented tool")?;
    assert_eq!(row["rust"]["cli"], "rtao tx set-weights");
    assert!(catalog::tools_payload(&[pending.clone(), pending.clone()]).is_err());
    let mut unknown = pending;
    unknown.name = "invented".into();
    assert!(catalog::tools_payload(&[unknown]).is_err());
    Ok(())
}

#[test]
fn tools_always_emits_json_to_stdout_even_without_json_flag_and_with_quiet() -> TestResult {
    for quiet in [false, true] {
        let (out, sink) = capture(false, quiet);
        catalog::run_tools(&out, &[])?;
        assert_eq!(payload(&sink)?.as_array().ok_or("tools JSON")?.len(), 83);
        assert!(sink.bytes(Stream::Stderr)?.is_empty());
    }
    Ok(())
}

#[test]
fn explain_all_semantic_codes_preserves_exact_python_payloads() -> TestResult {
    for spec in &rtao_catalog::SEMANTIC_ERRORS {
        let (out, sink) = capture(true, false);
        let mixed = format!(" \u{1c}{}\u{1f} ", spec.code.as_str().to_uppercase());
        explain(&["rtao", "explain", &mixed], &out)?;
        assert_eq!(
            payload(&sink)?,
            json!({"code":spec.code.as_str(),"explanation":spec.explanation,"help":spec.remediation,"see":format!("{}/errors/{}",rtao_catalog::py::DOCS_URL,spec.code.as_str().replace('_',"-"))})
        );
        assert!(sink.bytes(Stream::Stderr)?.is_empty());
    }
    Ok(())
}

#[test]
fn explain_chain_single_collision_qualification_and_dispatch_shapes() -> TestResult {
    let (out, sink) = capture(true, false);
    explain(
        &["rtao", "explain", "sUbTeNsOrMoDuLe.sEtTiNgWeIgHtStOoFaSt"],
        &out,
    )?;
    let row = payload(&sink)?;
    assert_eq!(row["pallet"], "SubtensorModule");
    assert_eq!(row["name"], "SettingWeightsTooFast");
    assert_eq!(row["code"], "rate_limited");
    assert_eq!(row.as_object().ok_or("single error")?.len(), 7);
    assert_eq!(
        row["docs_url"],
        format!(
            "{}/errors/chain/SettingWeightsTooFast",
            rtao_catalog::py::DOCS_URL
        )
    );
    let (out, sink) = capture(true, false);
    explain(&["rtao", "explain", "TooMany"], &out)?;
    let collisions = payload(&sink)?;
    let rows = collisions.as_array().ok_or("collision array")?;
    assert!(rows.len() >= 2);
    assert!(rows.iter().any(|row| row["pallet"] == "Proxy"));
    let (out, sink) = capture(true, false);
    explain(&["rtao", "explain", "runtime.badOrigin"], &out)?;
    let row = payload(&sink)?;
    assert_eq!(row["pallet"], "Runtime");
    assert_eq!(row["name"], "BadOrigin");
    assert_eq!(row["code"], "not_authorized");
    assert_eq!(row["docs"], row["description"]);
    assert!(catalog::chain_matches("Balances.BadOrigin")?.is_empty());
    assert!(catalog::chain_matches("Runtime.SettingWeightsTooFast")?.is_empty());
    Ok(())
}

#[test]
fn explain_every_dispatch_variant_retains_docs_remediation_and_semantic_url() -> TestResult {
    for (name, code) in rtao_catalog::errors::DISPATCH_ERRORS {
        let matches = catalog::chain_matches(&format!("Runtime.{name}"))?;
        assert_eq!(matches.len(), 1, "{name}");
        let row = &matches[0];
        let docs = rtao_catalog::py::DISPATCH_ERROR_DOCS
            .iter()
            .find(|(candidate, _)| candidate == name)
            .ok_or("dispatch docs")?
            .1;
        assert_eq!(row["docs"], docs);
        assert_eq!(
            row["help"],
            rtao::tx::ChainError::new("", Some((*name).into())).remediation()
        );
        assert_eq!(row["code"], code.as_str());
        assert_eq!(
            row["docs_url"],
            format!(
                "{}/errors/{}",
                rtao_catalog::py::DOCS_URL,
                code.as_str().replace('_', "-")
            )
        );
    }
    Ok(())
}

#[test]
fn chain_listing_equals_every_authenticated_runtime_error_in_declaration_order() -> TestResult {
    let source = rtao_catalog::py::PYTHON_TABLES
        .iter()
        .find(|table| {
            table.path == "sdk/python/bittensor/_generated/errors.py" && table.name == "ERRORS"
        })
        .ok_or("authenticated ERRORS")?;
    let raw: Value = serde_json::from_str(source.value_json)?;
    let entries = raw["dict"].as_array().ok_or("raw entries")?;
    let payload = catalog::chain_catalog(None)?;
    let rows = payload.as_array().ok_or("error rows")?;
    assert_eq!(rows.len(), entries.len());
    for (row, entry) in rows.iter().zip(entries) {
        let raw = &entry[1]["fields"];
        assert_eq!(row["pallet"], raw["pallet"]);
        assert_eq!(row["name"], raw["name"]);
        assert_eq!(row["description"], raw["docs"]);
        assert_eq!(row.as_object().ok_or("chain row")?.len(), 5);
        assert_eq!(
            row["code"],
            rtao_catalog::errors::classify_error(
                raw["docs"].as_str().ok_or("raw docs")?,
                raw["name"].as_str()
            )
            .as_str()
        );
    }
    assert!(!rows.iter().any(|row| row["pallet"] == "Runtime"));
    let filtered = catalog::chain_catalog(Some("bAlAnCeS"))?;
    let rows = filtered.as_array().ok_or("filtered errors")?;
    assert!(!rows.is_empty());
    assert!(rows.iter().all(|row| row["pallet"] == "Balances"));
    Ok(())
}

#[test]
fn explain_bare_and_chain_modes_emit_one_flat_json_array_without_chatter() -> TestResult {
    for args in [vec!["rtao", "explain"], vec!["rtao", "explain", "--chain"]] {
        let (out, sink) = capture(true, false);
        explain(&args, &out)?;
        assert_eq!(payload(&sink)?, catalog::chain_catalog(None)?);
        assert!(sink.bytes(Stream::Stderr)?.is_empty());
    }
    Ok(())
}

#[test]
fn explain_unknown_code_and_pallet_preserve_status_and_actionable_help() -> TestResult {
    let (out, sink) = capture(true, false);
    let error = explain(&["rtao", "explain", "  not_a_real_code  "], &out)
        .err()
        .ok_or("unknown code accepted")?;
    assert_eq!(error.code, 1);
    assert_eq!(error.message, "unknown error code 'not_a_real_code'");
    assert_eq!(
        error.help.as_deref(),
        Some(
            "run `btcli explain` for the semantic codes, or `btcli explain --chain` for the chain error catalog"
        )
    );
    let error = explain(&["rtao", "explain", "--chain", " Runtime "], &out)
        .err()
        .ok_or("dispatch pallet not part of chain listing")?;
    assert_eq!(error.code, 1);
    assert_eq!(error.message, "unknown pallet ' Runtime '");
    assert!(
        error
            .help
            .as_deref()
            .ok_or("pallet help")?
            .starts_with("valid pallets: ")
    );
    assert!(sink.bytes(Stream::Stdout)?.is_empty());
    assert!(sink.bytes(Stream::Stderr)?.is_empty());
    Ok(())
}

#[test]
fn explain_human_output_preserves_primary_data_in_quiet_mode() -> TestResult {
    for args in [
        vec!["rtao", "explain", "insufficient_balance"],
        vec!["rtao", "explain", "SettingWeightsTooFast"],
        vec!["rtao", "explain", "--chain", "Balances"],
    ] {
        let (out, sink) = capture(false, true);
        explain(&args, &out)?;
        assert!(!sink.bytes(Stream::Stdout)?.is_empty());
        assert!(sink.bytes(Stream::Stderr)?.is_empty());
    }
    Ok(())
}
