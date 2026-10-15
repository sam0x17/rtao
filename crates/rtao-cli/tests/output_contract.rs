// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/output.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/query.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Stream and value contracts from the pinned Python output machinery.

use std::sync::Arc;

use rtao::client::Inclusion;
use rtao::primitives::{Balance, DynBalance, NetUid, Symbol, Tao};
use rtao::tx::{ChainError, ExtrinsicResult};
use rtao_cli::output::{
    DiagnosticContext, HumanView, MemorySink, Output, OutputOptions, OutputSink, PythonBalance,
    PythonBytes, PythonString, StepState, Stream, TableSpec, diagnostic,
};
use serde::Serialize;
use serde_json::{Value, json};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn capture(json_mode: bool, quiet: bool) -> (Output, MemorySink) {
    let sink = MemorySink::default();
    (
        Output::new(
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

fn outcome(success: bool) -> ExtrinsicResult {
    ExtrinsicResult {
        success,
        message: if success {
            "Finalized".into()
        } else {
            "The hotkey is not registered.".into()
        },
        block_hash: None,
        extrinsic_id: Some("12-3".into()),
        explorer_url: None,
        fee: Some(Balance::<Tao>::from_rao(124_414)),
        events: Vec::new(),
        error: (!success).then(|| {
            ChainError::new(
                "The hotkey is not registered.",
                Some("HotKeyNotRegisteredInSubNet".into()),
            )
        }),
        data: serde_json::Map::from_iter([
            ("netuid".into(), json!(4)),
            ("receipt_bytes".into(), json!("0x0102")),
        ]),
        output: (),
        inclusion: Inclusion::Unobserved,
        fate_unknown: None,
    }
}

#[test]
fn json_stdout_and_diagnostics_stderr_remain_separate() -> TestResult {
    let (output, sink) = capture(true, false);
    output.message("connecting")?;
    output.step("submit", "0xabcdef", StepState::Running)?;
    output.value(&json!({"value": u64::MAX,"nested":[true,null,"τ"]}))?;
    output.error(
        "Connection failed.",
        DiagnosticContext {
            note: Some("Try later."),
            help: Some("Use --network."),
            see: Some("https://example.invalid"),
        },
    )?;
    assert_eq!(
        serde_json::from_str::<Value>(&sink.text(Stream::Stdout)?)?,
        json!({"value":18446744073709551615_u64,"nested":[true,null,"τ"]})
    );
    assert_eq!(
        serde_json::from_str::<Value>(&sink.text(Stream::Stderr)?)?,
        json!({"error":"Connection failed.","note":"Try later.","help":"Use --network.","see":"https://example.invalid"})
    );
    assert!(!sink.text(Stream::Stdout)?.contains("connecting"));
    Ok(())
}

#[test]
fn quiet_keeps_primary_data_and_errors_but_suppresses_optional_followups() -> TestResult {
    for json_mode in [false, true] {
        let (output, sink) = capture(json_mode, true);
        output.message("chatter")?;
        output.multisig_followup(&json!({"secret":"followup"}))?;
        output.pending_multisigs("pending", &[json!({"call":"followup"})])?;
        output.value(&json!({"answer":42}))?;
        output.error("Something failed.", DiagnosticContext::default())?;
        assert!(sink.text(Stream::Stdout)?.contains("42"));
        assert!(!sink.text(Stream::Stdout)?.contains("followup"));
        assert!(!sink.text(Stream::Stderr)?.contains("chatter"));
        assert!(sink.text(Stream::Stderr)?.contains("failed"));
    }
    Ok(())
}

#[test]
fn human_chatter_goes_only_to_stderr() -> TestResult {
    let (output, sink) = capture(false, false);
    output.message("connecting")?;
    output.step("connected", "Finney", StepState::Done)?;
    assert!(sink.text(Stream::Stdout)?.is_empty());
    assert!(sink.text(Stream::Stderr)?.contains("connecting"));
    assert!(sink.text(Stream::Stderr)?.contains("✓ connected"));
    Ok(())
}

#[derive(Serialize)]
struct NestedRead<'a> {
    balance: PythonBalance,
    alpha: PythonBalance,
    bytes: PythonBytes<'a>,
    enumeration: PythonString<'a>,
    stamp: PythonString<'a>,
    optional: Option<u64>,
    tuple: (bool, u64),
    nested: Vec<Value>,
}

#[test]
fn recursive_read_serialization_keeps_balance_bytes_enum_and_dataclass_values() -> TestResult {
    let (output, sink) = capture(true, false);
    output.update_token_symbols([(4, Symbol::new("ش"))])?;
    output.value(&NestedRead {
        balance: output.balance(1_500_000_000, NetUid::ROOT),
        alpha: output.balance(2_000_000_001, NetUid(4)),
        bytes: PythonBytes(&[0, b'\n', b'\'', b'"', 255]),
        enumeration: PythonString("ErrorCode.UNKNOWN"),
        stamp: PythonString("2026-10-14T00:01:02+00:00"),
        optional: None,
        tuple: (true, u64::MAX),
        nested: vec![
            json!({"balance":PythonBalance(DynBalance::from_rao(1,NetUid(42))),"list":[1,2]}),
        ],
    })?;
    let payload: Value = serde_json::from_str(&sink.text(Stream::Stdout)?)?;
    assert_eq!(
        payload,
        json!({"balance":"τ1.500000000","alpha":"2.000000001ش","bytes":"b'\\x00\\n\\'\"\\xff'","enumeration":"ErrorCode.UNKNOWN","stamp":"2026-10-14T00:01:02+00:00","optional":null,"tuple":[true,u64::MAX],"nested":[{"balance":"0.000000001α₄₂","list":[1,2]}]})
    );
    Ok(())
}

#[test]
fn byte_fallback_uses_python_repr_quote_selection_and_ascii_escapes() -> TestResult {
    assert_eq!(
        serde_json::to_value(PythonBytes(b"a'b"))?,
        json!("b\"a'b\"")
    );
    assert_eq!(
        serde_json::to_value(PythonBytes(b"a\"b"))?,
        json!("b'a\"b'")
    );
    assert_eq!(
        serde_json::to_value(PythonBytes(b"\\\t\r\x7f"))?,
        json!("b'\\\\\\t\\r\\x7f'")
    );
    assert_eq!(serde_json::to_value(PythonBytes(b""))?, json!("b''"));
    Ok(())
}

#[test]
fn native_u64_balance_is_not_serialized_as_float_or_raw_decimal_intent_value() -> TestResult {
    let (output, sink) = capture(true, false);
    output.value(&json!({"balance":output.balance(u64::MAX,NetUid::ROOT),"rao":u64::MAX}))?;
    let payload: Value = serde_json::from_str(&sink.text(Stream::Stdout)?)?;
    assert_eq!(payload["balance"], json!("τ18,446,744,073.709552765"));
    assert_eq!(payload["rao"], json!(u64::MAX));
    Ok(())
}

#[test]
fn explicit_table_machine_records_survive_trimmed_human_columns() -> TestResult {
    let records =
        json!([{"uid":3,"stake":"τ1.500000000","opaque":{"weights":[1,2]},"raw":"0x0123"}]);
    let mut spec = TableSpec::new(
        "metagraph",
        vec!["uid".into(), "stake".into()],
        vec![vec![json!(3), json!("τ1.500000000")]],
    );
    spec.records = Some(records.clone());
    spec.legend.push(("stake".into(), "total stake".into()));
    spec.footer = Some("1 neuron".into());
    let (json_output, json_sink) = capture(true, false);
    json_output.table(&spec)?;
    assert_eq!(
        serde_json::from_str::<Value>(&json_sink.text(Stream::Stdout)?)?,
        records
    );
    let (human, human_sink) = capture(false, false);
    human.table(&spec)?;
    let rendered = human_sink.text(Stream::Stdout)?;
    for content in [
        "metagraph",
        "uid",
        "stake",
        "3",
        "τ1.500000000",
        "total stake",
        "1 neuron",
    ] {
        assert!(rendered.contains(content), "missing {content}: {rendered}");
    }
    assert!(!rendered.contains("weights"));
    Ok(())
}

#[test]
fn table_json_fallback_matches_python_zip_and_duplicate_key_rules() -> TestResult {
    let spec = TableSpec::new(
        "pairs",
        vec!["key".into(), "key".into(), "missing".into()],
        vec![
            vec![json!(1), json!(2)],
            vec![json!(3), json!(4), json!(5), json!(6)],
        ],
    );
    assert_eq!(
        spec.json_payload(),
        json!([{"key":2},{"key":4,"missing":5}])
    );
    let (output, sink) = capture(false, false);
    assert!(output.table(&spec).is_err());
    assert!(sink.bytes(Stream::Stdout)?.is_empty());
    Ok(())
}

#[test]
fn renderer_registry_cannot_change_or_corrupt_machine_payload() -> TestResult {
    let (mut output, sink) = capture(true, false);
    output.registry_mut().register("balance", |_| {
        Err(rtao_cli::output::OutputError::Render(
            "human formatter failed".into(),
        ))
    });
    let payload = json!({"address":"5raw","amount":"τ1.000000000","nested":[{"key":true}]});
    output.name_address("5raw", "local nickname")?;
    output.render("balance", &payload)?;
    assert_eq!(
        serde_json::from_str::<Value>(&sink.text(Stream::Stdout)?)?,
        payload
    );
    let (mut human, human_sink) = capture(false, false);
    human.registry_mut().register("balance", |_| {
        Ok(HumanView::Text("human-only projection".into()))
    });
    human.render("balance", &payload)?;
    assert_eq!(human_sink.text(Stream::Stdout)?, "human-only projection\n");
    Ok(())
}

#[test]
fn cloned_outputs_share_display_updates_without_rewriting_machine_records() -> TestResult {
    let (output, sink) = capture(false, false);
    let clone = output.clone();
    let mut machine = clone.clone();
    machine.options.json_mode = true;
    let old_names = clone.subnet_names()?;
    output.name_address("5raw", "alice")?;
    output.name_address("5raw", "later label")?;
    output.update_subnet_names([(4, "four".into())])?;
    clone.update_token_symbols([(4, Symbol::new("ش"))])?;

    assert_eq!(clone.address_names()?.get("5raw"), Some(&"alice".into()));
    assert_eq!(clone.account_text("5raw", None), "5raw (alice)");
    assert_eq!(clone.subnet_text(NetUid(4), true), "4 (four) ش");
    assert_eq!(output.unit(NetUid(4)), "ش");
    assert_eq!(output.token_symbols()?.get(&4), Some(&Symbol::new("ش")));
    assert_eq!(clone.subnet_names()?.get(&0), Some(&"root".into()));
    assert!(!old_names.contains_key(&4));

    let payload = json!({"address":"5raw","netuid":4,"amount":"2.000000001α₄"});
    machine.render("display-cache-test", &payload)?;
    assert_eq!(
        serde_json::from_str::<Value>(&sink.text(Stream::Stdout)?)?,
        payload
    );
    Ok(())
}

#[test]
fn generated_query_omits_opaque_human_cells_but_preserves_json() -> TestResult {
    let payload = json!([{"uid":1,"name":"first","raw":{"blob":"opaque-content"}},{"uid":2,"name":"second","raw":{"blob":"more-opaque"}}]);
    let (human, sink) = capture(false, false);
    human.query("neurons", &payload)?;
    let text = sink.text(Stream::Stdout)?;
    for value in ["uid", "name", "first", "second"] {
        assert!(text.contains(value));
    }
    assert!(!text.contains("opaque-content"));
    let (output, sink) = capture(true, false);
    output.query("neurons", &payload)?;
    assert_eq!(
        serde_json::from_str::<Value>(&sink.text(Stream::Stdout)?)?,
        payload
    );
    Ok(())
}

#[test]
fn dry_run_payload_preserves_full_plan_and_exact_replay_command() -> TestResult {
    let mut plan = json!({"op":"transfer","summary":"Transfer τ1.500000000","signer":"coldkey","signer_address":"5Alice","args":{"amount_tao":"1.500000000","dest":"5Bob"},"fee_tao":0.000124414,"spend_tao":1.5,"spend_unbounded":false,"effects":["dest gains funds"],"warnings":["low balance"],"violations":[],"ok":true,"docs_url":"https://example.invalid/tx/transfer"});
    let command = "rtao --yes tx transfer --dest 5Bob --amount-tao 1.500000000";
    let (output, sink) = capture(true, true);
    output.plan_payload(&plan, Some(command))?;
    plan["command"] = json!(command);
    assert_eq!(
        serde_json::from_str::<Value>(&sink.text(Stream::Stdout)?)?,
        plan
    );
    assert!(sink.text(Stream::Stderr)?.is_empty());
    let (human, sink) = capture(false, false);
    plan["ok"] = json!(false);
    plan["violations"] = json!(["transfer limit"]);
    human.plan_payload(&plan, Some(command))?;
    let text = sink.text(Stream::Stdout)?;
    assert!(text.contains("blocked by policy"));
    assert!(text.contains("transfer limit"));
    assert!(!text.contains("run for real"));
    Ok(())
}

#[test]
fn receipts_emit_canonical_domain_shape_including_failures_on_stdout() -> TestResult {
    for success in [false, true] {
        let (output, sink) = capture(true, false);
        let result = outcome(success);
        assert_eq!(output.result(&result, "transfer complete")?, success);
        assert_eq!(
            serde_json::from_str::<Value>(&sink.text(Stream::Stdout)?)?,
            result.to_json()
        );
        assert!(sink.text(Stream::Stderr)?.is_empty());
        let payload: Value = serde_json::from_str(&sink.text(Stream::Stdout)?)?;
        assert_eq!(payload["fee_tao"], json!(0.000124414));
        assert_eq!(payload["data"]["netuid"], json!(4));
        assert!(payload.get("events").is_none());
        assert!(payload.get("output").is_none());
    }
    Ok(())
}

#[test]
fn standalone_chain_error_retains_classification_and_optional_explorer_url() -> TestResult {
    let result = outcome(false);
    let error = result.error.ok_or("missing fixture error")?;
    let (output, sink) = capture(true, true);
    output.chain_error(&error, Some("https://example.invalid/12-3"))?;
    let mut expected = error.to_json();
    expected["explorer_url"] = json!("https://example.invalid/12-3");
    assert_eq!(
        serde_json::from_str::<Value>(&sink.text(Stream::Stderr)?)?,
        expected
    );
    assert!(sink.text(Stream::Stdout)?.is_empty());
    let (human, sink) = capture(false, true);
    human.chain_error(&error, None)?;
    let text = sink.text(Stream::Stderr)?;
    assert!(text.contains("error["));
    assert!(text.contains("HotKeyNotRegisteredInSubNet"));
    assert!(text.contains("help:"));
    Ok(())
}

#[test]
fn mnemonic_is_primary_data_under_quiet_and_not_an_extra_json_document() -> TestResult {
    let (human, sink) = capture(false, true);
    human.mnemonic("coldkey", "synthetic recovery phrase")?;
    assert!(
        sink.text(Stream::Stdout)?
            .contains("synthetic recovery phrase")
    );
    let (output, sink) = capture(true, true);
    output.mnemonic("coldkey", "synthetic recovery phrase")?;
    output.value(&json!({"mnemonic":"synthetic recovery phrase"}))?;
    assert_eq!(
        serde_json::from_str::<Value>(&sink.text(Stream::Stdout)?)?,
        json!({"mnemonic":"synthetic recovery phrase"})
    );
    Ok(())
}

#[test]
fn copyable_hashes_and_brackets_are_never_markup_or_hard_wrapped() -> TestResult {
    let (output, sink) = capture(false, false);
    let long = format!("0x{}", "a".repeat(256));
    output.detail(
        Some("record"),
        &[
            ("call".into(), json!(long)),
            ("name".into(), json!("[red]literal[/red]")),
        ],
        None,
    )?;
    let text = sink.text(Stream::Stdout)?;
    assert!(text.contains(&long));
    assert!(text.contains("[red]literal[/red]"));
    Ok(())
}

#[test]
fn diagnostic_normalization_keeps_acronyms_identifiers_and_ellipses() {
    assert_eq!(diagnostic("  Missing balance.  "), "missing balance");
    assert_eq!(diagnostic("RPC failure."), "RPC failure");
    assert_eq!(diagnostic("FooBar."), "fooBar");
    assert_eq!(diagnostic("Retry..."), "retry...");
    assert_eq!(diagnostic("."), "");
    assert_eq!(diagnostic("Échec."), "échec");
}

struct BrokenSink;
impl OutputSink for BrokenSink {
    fn write(&self, _stream: Stream, _bytes: &[u8]) -> std::io::Result<()> {
        Err(std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "closed consumer",
        ))
    }
}

#[test]
fn broken_output_pipe_is_reported_without_panicking() {
    let output = Output::new(OutputOptions::default(), Arc::new(BrokenSink));
    assert!(
        matches!(output.value(&json!({"answer":42})),Err(rtao_cli::output::OutputError::Io(error)) if error.kind() == std::io::ErrorKind::BrokenPipe)
    );
}
