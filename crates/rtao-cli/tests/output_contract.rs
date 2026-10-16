// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/output.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/query.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Stream and value contracts from the pinned Python output machinery.

use std::sync::Arc;

use rtao::client::Inclusion;
use rtao::primitives::{Balance, DynBalance, NetUid, Symbol, Tao};
use rtao::tx::{ChainError, ExtrinsicResult};
use rtao_cli::output::{
    DiagnosticContext, HumanView, MemorySink, Output, OutputOptions, OutputSink, PythonBalance,
    PythonBytes, PythonString, ReviewSection, StepState, Stream, TableSpec, diagnostic,
    terminal_text,
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

const HOSTILE_TEXT: &str =
    "\x1b[2J\x1b]0;owned\x07\rrewrite\nforged\t\u{7f}\u{85}\u{2028}\u{2029}\u{202e}\u{2066}";
const VISIBLE_HOSTILE_TEXT: &str =
    r"\x1b[2J\x1b]0;owned\x07\rrewrite\nforged\t\x7f\x85\u{2028}\u{2029}\u{202e}\u{2066}";

fn hostile(label: &str) -> String {
    format!("{label}{HOSTILE_TEXT}")
}

fn assert_safe_human_text(text: &str, labels: &[&str]) {
    for character in text.chars() {
        assert!(
            !character.is_control() || character == '\n',
            "unexpected terminal control U+{:04X}",
            u32::from(character)
        );
        assert!(
            !matches!(character, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{2028}'..='\u{202e}' | '\u{2066}'..='\u{2069}'),
            "unexpected directional or line formatting U+{:04X}",
            u32::from(character)
        );
    }
    for label in labels {
        let expected = format!("{label}{VISIBLE_HOSTILE_TEXT}");
        assert!(text.contains(&expected), "missing visible field {label}");
    }
}

#[test]
fn human_terminal_text_escapes_controls_and_directional_formatting() {
    assert_eq!(terminal_text(HOSTILE_TEXT), VISIBLE_HOSTILE_TEXT);
    assert_eq!(
        terminal_text(
            "\u{061c}\u{200e}\u{200f}\u{202a}\u{202b}\u{202c}\u{202d}\u{202e}\u{2066}\u{2067}\u{2068}\u{2069}"
        ),
        r"\u{61c}\u{200e}\u{200f}\u{202a}\u{202b}\u{202c}\u{202d}\u{202e}\u{2066}\u{2067}\u{2068}\u{2069}"
    );
    let controls: String = ('\0'..='\u{9f}')
        .filter(|character| character.is_control())
        .collect();
    let escaped = terminal_text(&controls);
    assert!(escaped.chars().all(|character| !character.is_control()));
    assert!(escaped.contains(r"\x00\x01\x02"));
    assert!(escaped.contains(r"\x1b"));
    assert!(escaped.contains(r"\x9f"));
    let ordinary = "τ ش 日本語 [red]literal[/red] 0x010203";
    assert_eq!(terminal_text(ordinary), ordinary);
}

#[test]
fn human_detail_escapes_titles_keys_and_nested_values_without_changing_json() -> TestResult {
    let fields = vec![
        (hostile("field-key"), json!(hostile("scalar-value"))),
        (
            "object".into(),
            json!({(hostile("object-key")): hostile("object-value")}),
        ),
        (
            "array".into(),
            json!([
                hostile("array-value"),
                [hostile("nested-array-value")],
                {(hostile("repr-key")): hostile("repr-value")}
            ]),
        ),
        (
            "records".into(),
            json!([{(hostile("record-key")): hostile("record-value")}]),
        ),
    ];
    let title = hostile("detail-title");
    let (human, sink) = capture(false, false);
    human.detail(Some(&title), &fields, None)?;
    let rendered = sink.text(Stream::Stdout)?;
    assert_safe_human_text(
        &rendered,
        &[
            "detail-title",
            "field-key",
            "scalar-value",
            "object-key",
            "object-value",
            "array-value",
            "nested-array-value",
            "repr-key",
            "repr-value",
            "record-key",
            "record-value",
        ],
    );
    assert_eq!(
        rendered.lines().count(),
        7,
        "input newlines must remain within their fields"
    );
    assert!(sink.text(Stream::Stderr)?.is_empty());

    let expected = Value::Object(fields.iter().cloned().collect());
    let (machine, sink) = capture(true, false);
    machine.detail(Some(&title), &fields, None)?;
    assert_eq!(
        serde_json::from_str::<Value>(&sink.text(Stream::Stdout)?)?,
        expected
    );
    let rich_payload = json!({"original": expected, "opaque": HOSTILE_TEXT});
    let (machine, sink) = capture(true, false);
    machine.detail(Some(&title), &fields, Some(&rich_payload))?;
    assert_eq!(
        serde_json::from_str::<Value>(&sink.text(Stream::Stdout)?)?,
        rich_payload
    );
    Ok(())
}

#[test]
fn human_tables_escape_input_before_layout_and_keep_machine_records_exact() -> TestResult {
    let mut spec = TableSpec::new(
        hostile("table-title"),
        vec![hostile("header"), "count".into()],
        vec![vec![json!(hostile("cell")), json!(7)]],
    );
    spec.right_align.insert(1);
    spec.legend
        .push((hostile("legend-term"), hostile("legend-meaning")));
    spec.footer = Some(hostile("footer"));
    for bordered in [true, false] {
        let (human, sink) = capture(false, false);
        if bordered {
            human.table(&spec)?;
        } else {
            human.columns(&spec)?;
        }
        let rendered = sink.text(Stream::Stdout)?;
        assert_safe_human_text(&rendered, &["table-title", "header", "cell", "footer"]);
        if bordered {
            assert_safe_human_text(&rendered, &["legend-term", "legend-meaning"]);
            assert_eq!(
                rendered.lines().count(),
                8,
                "trusted table borders must stay on separate lines"
            );
            assert!(
                rendered.contains("\n│"),
                "table rows must retain structural newlines"
            );
        } else {
            assert_eq!(
                rendered.lines().count(),
                4,
                "columns must have exactly one header and data row"
            );
        }
        let expected = json!([{(hostile("header")): hostile("cell"), "count": 7}]);
        let (machine, sink) = capture(true, false);
        if bordered {
            machine.table(&spec)?;
        } else {
            machine.columns(&spec)?;
        }
        assert_eq!(
            serde_json::from_str::<Value>(&sink.text(Stream::Stdout)?)?,
            expected
        );
    }
    Ok(())
}

#[test]
fn human_cached_subnet_names_and_symbols_are_escaped_after_cell_substitution() -> TestResult {
    let (human, sink) = capture(false, false);
    human.update_subnet_names([(4, hostile("cached-subnet"))])?;
    human.update_token_symbols([(4, Symbol::new(hostile("cached-symbol")))])?;
    let fields = vec![("netuid".into(), json!(4))];
    human.detail(Some("subnet"), &fields, None)?;
    let spec = TableSpec::new("subnets", vec!["netuid".into()], vec![vec![json!(4)]]);
    human.table(&spec)?;
    let rendered = sink.text(Stream::Stdout)?;
    assert_safe_human_text(&rendered, &["cached-subnet", "cached-symbol"]);
    assert_eq!(rendered.matches("cached-subnet").count(), 2);
    assert_eq!(rendered.matches("cached-symbol").count(), 2);
    assert_eq!(human.subnet_names()?[&4], hostile("cached-subnet"));
    assert_eq!(
        human.token_symbols()?[&4].as_str(),
        hostile("cached-symbol")
    );

    let (machine, sink) = capture(true, false);
    machine.update_subnet_names([(4, hostile("cached-subnet"))])?;
    machine.update_token_symbols([(4, Symbol::new(hostile("cached-symbol")))])?;
    machine.table(&spec)?;
    assert_eq!(
        serde_json::from_str::<Value>(&sink.text(Stream::Stdout)?)?,
        json!([{"netuid": 4}])
    );
    Ok(())
}

#[test]
fn human_trees_and_registered_text_escape_input_while_json_bypasses_views() -> TestResult {
    let title = hostile("tree-title");
    let nodes = vec![(hostile("branch"), vec![hostile("leaf")])];
    let records = json!({"name": HOSTILE_TEXT, "nested": [{(hostile("key")): HOSTILE_TEXT}]});
    let (mut human, sink) = capture(false, false);
    human.tree(&title, &nodes, Some(&records))?;
    human.registry_mut().register("hostile-text", |_| {
        Ok(HumanView::Text(hostile("registered-text")))
    });
    human.render("hostile-text", &records)?;
    let rendered = sink.text(Stream::Stdout)?;
    assert_safe_human_text(
        &rendered,
        &["tree-title", "branch", "leaf", "registered-text"],
    );
    assert_eq!(rendered.lines().count(), 4);
    let (mut lines, sink) = capture(false, false);
    lines.registry_mut().register("trusted-layout", |_| {
        Ok(HumanView::Lines(vec![
            "Trusted heading".into(),
            hostile("line-value"),
            "Trusted next section".into(),
        ]))
    });
    lines.render("trusted-layout", &records)?;
    let rendered = sink.text(Stream::Stdout)?;
    assert_safe_human_text(&rendered, &["line-value"]);
    assert_eq!(rendered.lines().count(), 3);
    assert!(rendered.starts_with("Trusted heading\n"));
    assert!(rendered.ends_with("\nTrusted next section\n"));
    let (machine, sink) = capture(true, false);
    machine.tree(&title, &nodes, Some(&records))?;
    assert_eq!(
        serde_json::from_str::<Value>(&sink.text(Stream::Stdout)?)?,
        records
    );
    let (mut machine, sink) = capture(true, false);
    machine.registry_mut().register("hostile-text", |_| {
        Ok(HumanView::Text(hostile("registered-text")))
    });
    machine.render("hostile-text", &records)?;
    assert_eq!(
        serde_json::from_str::<Value>(&sink.text(Stream::Stdout)?)?,
        records
    );
    Ok(())
}

#[test]
fn human_messages_and_diagnostics_escape_controls_but_json_retains_original_strings() -> TestResult
{
    let error = hostile("diagnostic");
    let note = hostile("note-value");
    let help = hostile("help-value");
    let see = hostile("see-value");
    for json_mode in [false, true] {
        let (output, sink) = capture(json_mode, false);
        output.message(&hostile("message"))?;
        output.step(
            &hostile("step-label"),
            &hostile("step-value"),
            StepState::Running,
        )?;
        output.error(
            &error,
            DiagnosticContext {
                note: Some(&note),
                help: Some(&help),
                see: Some(&see),
            },
        )?;
        assert!(sink.text(Stream::Stdout)?.is_empty());
        let rendered = sink.text(Stream::Stderr)?;
        if json_mode {
            assert_eq!(
                serde_json::from_str::<Value>(&rendered)?,
                json!({"error": error, "note": note, "help": help, "see": see})
            );
        } else {
            assert_safe_human_text(
                &rendered,
                &[
                    "message",
                    "step-label",
                    "step-value",
                    "diagnostic",
                    "note-value",
                    "help-value",
                    "see-value",
                ],
            );
            assert_eq!(rendered.lines().count(), 6);
        }
    }
    Ok(())
}

#[test]
fn semantic_and_raw_review_cards_keep_signer_and_operation_visible_with_hostile_arguments()
-> TestResult {
    let call_hex = "0x050300000100020003000400";
    let sections = vec![
        ReviewSection {
            title: hostile("SetSubnetIdentity"),
            rows: vec![(hostile("description"), json!(hostile("identity-value")))],
        },
        ReviewSection {
            title: hostile("Balances.transfer_keep_alive"),
            rows: vec![
                (
                    "arguments".into(),
                    json!({(hostile("argument-key")): hostile("argument-value")}),
                ),
                ("encoded call".into(), json!(call_hex)),
            ],
        },
        ReviewSection {
            title: "Transaction".into(),
            rows: vec![
                ("signer".into(), json!("5Signer")),
                ("role".into(), json!("coldkey")),
            ],
        },
    ];
    let (human, sink) = capture(false, false);
    human.transaction_card(&sections)?;
    let rendered = sink.text(Stream::Stdout)?;
    assert_safe_human_text(
        &rendered,
        &[
            "SetSubnetIdentity",
            "description",
            "identity-value",
            "Balances.transfer_keep_alive",
            "argument-key",
            "argument-value",
        ],
    );
    for expected in [
        "Transaction",
        "signer",
        "5Signer",
        "role",
        "coldkey",
        call_hex,
    ] {
        assert!(rendered.contains(expected), "review lost {expected}");
    }
    assert!(sink.text(Stream::Stderr)?.is_empty());
    let (machine, sink) = capture(true, false);
    machine.transaction_card(&sections)?;
    assert!(sink.text(Stream::Stdout)?.is_empty());
    assert_eq!(sections[1].rows[1].1, json!(call_hex));
    Ok(())
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
        let (registration, sink) = capture(true, true);
        assert_eq!(registration.registration_result(&result)?, success);
        assert_eq!(
            serde_json::from_str::<Value>(&sink.text(Stream::Stdout)?)?,
            result.to_json()
        );
        assert!(sink.text(Stream::Stderr)?.is_empty());
    }
    for (mode, prior, explanation) in [
        ("immediate", None, "immediate · no deregistration needed"),
        (
            "after_deregistration",
            Some(7),
            "queued · registered after deregistration of subnet 7",
        ),
        (
            "after_deregistration",
            None,
            "queued · registered after deregistration of subnet 9",
        ),
    ] {
        let mut result = outcome(true);
        result.data.insert("registration_mode".into(), json!(mode));
        result
            .data
            .insert("registration_price_rao".into(), json!(1_234_567_890_123u64));
        result.data.insert("queued_at_block".into(), json!(99));
        result.data.insert("registered_at_block".into(), json!(100));
        result
            .data
            .insert("deregistered_netuid".into(), json!(prior));
        result.data.insert("cleanup_netuid".into(), json!(9));
        for quiet in [false, true] {
            let (human, sink) = capture(false, quiet);
            assert!(human.registration_result(&result)?);
            let text = sink.text(Stream::Stdout)?;
            for fact in [
                "subnet 4 registered",
                explanation,
                "τ1,234.567890123",
                "queued block",
                "registered block",
                "registration tx",
                "12-3",
            ] {
                assert!(
                    text.contains(fact),
                    "registration output lost {fact}: {text}"
                );
            }
            assert!(!text.contains("registration price rao"));
            assert!(sink.text(Stream::Stderr)?.is_empty());
            let (machine, sink) = capture(true, quiet);
            assert!(machine.registration_result(&result)?);
            assert_eq!(
                serde_json::from_str::<Value>(&sink.text(Stream::Stdout)?)?,
                result.to_json()
            );
        }
    }
    let mut invalid = outcome(true);
    invalid.data.insert(
        "registration_price_rao".into(),
        json!("18446744073709551616"),
    );
    let (human, sink) = capture(false, false);
    assert!(human.registration_result(&invalid).is_err());
    assert!(sink.bytes(Stream::Stdout)?.is_empty());
    let (machine, sink) = capture(true, false);
    assert!(machine.registration_result(&invalid)?);
    assert_eq!(
        serde_json::from_str::<Value>(&sink.text(Stream::Stdout)?)?,
        invalid.to_json()
    );
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
    for quiet in [false, true] {
        let (human, sink) = capture(false, quiet);
        human.mnemonic("coldkey", "synthetic recovery phrase")?;
        human.mnemonic_clear_hint()?;
        let text = sink.text(Stream::Stdout)?;
        assert!(text.contains("synthetic recovery phrase"));
        assert_eq!(text.matches("once stored, wipe the mnemonic").count(), 1);
        assert!(text.contains(r"clear && printf '\e[3J'"));
        assert!(text.find("synthetic recovery phrase") < text.find("once stored"));
        assert!(sink.bytes(Stream::Stderr)?.is_empty());
        let (output, sink) = capture(true, quiet);
        output.mnemonic("coldkey", "synthetic recovery phrase")?;
        output.secret_detail(None, &[], &[("mnemonic", "synthetic recovery phrase")])?;
        output.mnemonic_clear_hint()?;
        assert_eq!(
            serde_json::from_str::<Value>(&sink.text(Stream::Stdout)?)?,
            json!({"mnemonic":"synthetic recovery phrase"})
        );
        assert!(sink.bytes(Stream::Stderr)?.is_empty());
    }
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

#[test]
fn transfer_history_preserves_columns_names_dates_signs_and_successful_tao_totals() -> TestResult {
    let records = json!([
        {"block_number":15,"timestamp":"2026-10-16T12:34:56Z","amount_rao":2_000_000_000u64,"netuid":null,"from":"peer","to":"owner","direction":"in","success":true,"opaque":HOSTILE_TEXT},
        {"block_number":14,"timestamp":"2026-10-16T13:34:56+02:00","amount_rao":"1000000000","netuid":0,"from":"owner","to":"peer","direction":"out","success":true},
        {"block_number":13,"timestamp":"2026-10-16T14:34:00","amount_rao":9_000_000_000u64,"netuid":0,"from":"owner","to":"owner","direction":"self","success":true},
        {"block_number":12,"timestamp":"invalid date","amount_rao":8_000_000_000u64,"netuid":4,"from":"peer","to":"owner","direction":"in","success":true},
        {"block_number":11,"timestamp":hostile("date"),"amount_rao":7_000_000_000u64,"netuid":0,"from":"owner","to":"peer","direction":"out","success":false}
    ]);
    let rows = records.as_array().ok_or("records")?;
    for quiet in [false, true] {
        let (output, sink) = capture(false, quiet);
        output.name_address("peer", &hostile("contact"))?;
        output.name_address("owner", "local owner")?;
        output.transfer_history(&hostile("history-title"), rows)?;
        let rendered = sink.text(Stream::Stdout)?;
        assert_safe_human_text(&rendered, &["history-title", "contact", "date"]);
        for text in [
            "when",
            "block",
            "amount",
            "counterparty",
            "2026-10-16 12:34",
            "2026-10-16 13:34",
            "2026-10-16 14:34",
            "invalid date",
            "+τ2.000000000",
            "−τ1.000000000",
            "self local owner (owner)",
            "✗ failed",
            "5 transfers  ·  in τ2.000000000  ·  out τ1.000000000  ·  1 failed excluded",
        ] {
            assert!(rendered.contains(text), "missing {text}: {rendered}");
        }
        assert!(!rendered.contains("amount (rao)"));
        assert!(sink.text(Stream::Stderr)?.is_empty());
        let (output, sink) = capture(true, quiet);
        output.transfer_history("ignored", rows)?;
        assert_eq!(
            serde_json::from_str::<Value>(&sink.text(Stream::Stdout)?)?,
            records
        );
        assert!(sink.text(Stream::Stderr)?.is_empty());
    }
    let (output, sink) = capture(false, true);
    output.transfer_history("empty", &[])?;
    assert_eq!(sink.text(Stream::Stdout)?, "empty\n  none\n");
    let row = &rows[..1];
    let (output, sink) = capture(false, false);
    output.transfer_history("one", row)?;
    assert!(
        sink.text(Stream::Stdout)?
            .contains("1 transfer  ·  in τ2.000000000")
    );
    Ok(())
}

#[test]
fn transfer_history_refuses_native_overflow_without_partial_human_output_but_keeps_json()
-> TestResult {
    for direction in ["in", "out"] {
        let records = [
            json!({"block_number":1,"timestamp":"2026-10-16T12:00:00Z","amount_rao":u64::MAX,"netuid":0,"from":"a","to":"b","direction":direction,"success":true}),
            json!({"block_number":2,"timestamp":"2026-10-16T12:00:00Z","amount_rao":1,"netuid":0,"from":"a","to":"b","direction":direction,"success":true}),
        ];
        let (output, sink) = capture(false, false);
        let error = output
            .transfer_history("history", &records)
            .err()
            .ok_or("overflow must refuse")?;
        assert_eq!(
            error.to_string(),
            format!("transfer history {direction} TAO total exceeds u64 rao")
        );
        assert!(sink.bytes(Stream::Stdout)?.is_empty());
        assert!(sink.bytes(Stream::Stderr)?.is_empty());
        let (output, sink) = capture(true, true);
        output.transfer_history("history", &records)?;
        assert_eq!(
            serde_json::from_str::<Value>(&sink.text(Stream::Stdout)?)?,
            json!(records)
        );
    }
    let record = json!({"block_number":1,"timestamp":"x","amount_rao":"18446744073709551616","netuid":0,"from":"a","to":"b","direction":"in","success":true});
    let (output, sink) = capture(false, false);
    assert!(
        output
            .transfer_history("history", std::slice::from_ref(&record))
            .is_err()
    );
    assert!(sink.bytes(Stream::Stdout)?.is_empty());
    let (output, sink) = capture(true, true);
    output.transfer_history("history", std::slice::from_ref(&record))?;
    assert_eq!(
        serde_json::from_str::<Value>(&sink.text(Stream::Stdout)?)?,
        json!([record])
    );
    Ok(())
}

#[test]
fn chain_error_explain_tip_prefers_catalog_membership_then_non_unknown_code() -> TestResult {
    use rtao_catalog::ErrorCode;
    for (name, code, target) in [
        (
            Some("HotKeyNotRegisteredInSubNet"),
            ErrorCode::NotRegistered,
            Some("HotKeyNotRegisteredInSubNet"),
        ),
        (Some("Dropped"), ErrorCode::Unknown, Some("Dropped")),
        (
            Some("FreshRateLimit"),
            ErrorCode::RateLimited,
            Some("rate_limited"),
        ),
        (None, ErrorCode::InvalidArgument, Some("invalid_argument")),
        (Some("Unrecognized"), ErrorCode::Unknown, None),
        (None, ErrorCode::Unknown, None),
    ] {
        let error = ChainError::with_code("fixture failure", name.map(str::to_owned), code);
        for quiet in [false, true] {
            let (output, sink) = capture(false, quiet);
            output.chain_error(&error, Some("https://example.invalid/extrinsic"))?;
            let text = sink.text(Stream::Stderr)?;
            assert!(text.contains("error["));
            if let Some(target) = target {
                assert!(text.contains(&format!("rtao explain {target}")));
            } else {
                assert!(!text.contains("rtao explain"));
            }
            if name == Some("FreshRateLimit") {
                assert!(!text.contains("rtao explain FreshRateLimit"));
            }
            assert!(sink.bytes(Stream::Stdout)?.is_empty());
            let (output, sink) = capture(true, quiet);
            output.chain_error(&error, None)?;
            assert_eq!(
                serde_json::from_str::<Value>(&sink.text(Stream::Stderr)?)?,
                error.to_json()
            );
            assert!(!sink.text(Stream::Stderr)?.contains("rtao explain"));
            assert!(sink.bytes(Stream::Stdout)?.is_empty());
        }
    }
    Ok(())
}

#[test]
fn pending_multisigs_keep_structured_commands_members_and_unique_decode_guidance() -> TestResult {
    let command = "rtao call System.remark --args '{\"remark\":[1,2]}' --signatory bob";
    let records = vec![
        json!({"status":"pending","call_hash":"0xaaaa","approval_labels":["alice (5Alice)"],"remaining_labels":["bob (5Bob)"],"co_signer_commands":[{"label":"bob (5Bob)","command":command}],"decode_note":"suppressed when commands exist","decode_hint":"also suppressed"}),
        json!({"status":"pending","call_hash":"0xbbbb","approval_labels":["carol (5Carol)"],"remaining_labels":["dave (5Dave)"],"co_signer_commands":[],"decode_note":"Call details unavailable.","decode_hint":"Pass --call-data."}),
        json!({"status":"pending","call_hash":"0xcccc","approval_labels":[],"remaining_labels":["eve (5Eve)"],"co_signer_commands":[],"decode_note":"Call details unavailable.","decode_hint":"Pass --call-data."}),
    ];
    let (output, sink) = capture(false, false);
    output.pending_multisigs("pending team", &records)?;
    let text = sink.text(Stream::Stdout)?;
    for expected in [
        command,
        "alice (5Alice)",
        "bob (5Bob)",
        "carol (5Carol)",
        "dave (5Dave)",
        "eve (5Eve)",
        "0xaaaa",
        "0xbbbb",
        "0xcccc",
        "pending team  3 ops",
    ] {
        assert!(
            text.contains(expected),
            "missing pending-operation fact {expected}"
        );
    }
    assert_eq!(text.matches("call details unavailable").count(), 1);
    assert_eq!(text.matches("pass --call-data").count(), 1);
    assert_eq!(
        text.matches("--macos-password or --keychain-password")
            .count(),
        1
    );
    assert!(text.contains("--signer vault (or ledger/extension) before call"));
    assert!(text.contains("--signatory already names the account"));
    assert!(!text.contains("suppressed when commands exist"));
    assert!(sink.bytes(Stream::Stderr)?.is_empty());
    let (machine, sink) = capture(true, false);
    machine.pending_multisigs("ignored human title", &records)?;
    assert_eq!(
        serde_json::from_str::<Value>(&sink.text(Stream::Stdout)?)?,
        json!({"pending_multisigs":records,"count":3})
    );
    for json_mode in [false, true] {
        let (quiet, sink) = capture(json_mode, true);
        quiet.pending_multisigs("pending", &records)?;
        assert!(sink.bytes(Stream::Stdout)?.is_empty() && sink.bytes(Stream::Stderr)?.is_empty());
    }
    let (empty, sink) = capture(false, false);
    empty.pending_multisigs("pending", &[])?;
    assert_eq!(
        sink.text(Stream::Stdout)?,
        "No pending multisig operations.\n"
    );
    for record in &records {
        let (human, sink) = capture(false, false);
        human.multisig_followup(record)?;
        assert_eq!(
            sink.text(Stream::Stdout)?
                .contains("--macos-password or --keychain-password"),
            !record["co_signer_commands"]
                .as_array()
                .ok_or("commands")?
                .is_empty()
        );
        let (machine, sink) = capture(true, false);
        machine.multisig_followup(record)?;
        assert_eq!(
            serde_json::from_str::<Value>(&sink.text(Stream::Stdout)?)?,
            json!({"multisig_followup":record})
        );
    }
    for status in ["executed", "submitted"] {
        let (human, sink) = capture(false, false);
        human.multisig_followup(
            &json!({"status":status,"co_signer_commands":[{"command":command}]}),
        )?;
        assert!(
            !sink
                .text(Stream::Stdout)?
                .contains("--macos-password or --keychain-password")
        );
    }
    let (mut custom, sink) = capture(false, false);
    let expected = json!(records);
    custom
        .registry_mut()
        .register("custom pending", move |records| {
            assert_eq!(records, &expected);
            Ok(HumanView::Text("custom registered pending view".into()))
        });
    custom.pending_multisigs("custom pending", &records)?;
    assert_eq!(
        sink.text(Stream::Stdout)?,
        "custom registered pending view\n"
    );
    Ok(())
}
