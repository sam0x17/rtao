//! Byte-level fixtures for Python-shaped JSON output, independent of a node.

use std::sync::Arc;

use rtao_cli::output::{
    DiagnosticContext, MemorySink, Output, OutputOptions, PyJsonFormatter, Stream,
};
use serde::Serialize;
use serde_json::{Value, json};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn bytes<T: Serialize + ?Sized>(value: &T) -> Result<String, Box<dyn std::error::Error>> {
    let mut result = Vec::new();
    PyJsonFormatter::to_writer(&mut result, value)?;
    Ok(String::from_utf8(result)?)
}

#[test]
fn python_float_notation_covers_thresholds_zero_and_binary_extremes() -> TestResult {
    for (value, expected) in [
        (0.0, "0.0"),
        (-0.0, "-0.0"),
        (1.0, "1.0"),
        (1e-4, "0.0001"),
        (1e-5, "1e-05"),
        (1e-9, "1e-09"),
        (1e15, "1000000000000000.0"),
        (1e16, "1e+16"),
        (1.0000000000000002, "1.0000000000000002"),
        (f64::from_bits(1), "5e-324"),
        (f64::MAX, "1.7976931348623157e+308"),
    ] {
        assert_eq!(bytes(&value)?, expected);
        // Arbitrary-precision Number has a distinct serde_json serializer
        // path, so exercise the CLI's existing domain Value payloads too.
        assert_eq!(bytes(&json!(value))?, expected);
    }
    Ok(())
}

#[test]
fn integers_remain_exact_without_float_conversion() -> TestResult {
    for integer in [
        "18446744073709551615",
        "340282366920938463463374607431768211455",
        "-1208925819614629174706176",
        "1208925819614629174706176",
    ] {
        let value: Value = serde_json::from_str(integer)?;
        assert_eq!(bytes(&value)?, integer);
    }
    assert_eq!(bytes(&u128::MAX)?, u128::MAX.to_string());
    Ok(())
}

#[derive(Serialize)]
struct OrderedRecord<'a> {
    zeta: &'a str,
    alpha: [f64; 2],
    empty: Vec<u8>,
}

#[test]
fn two_space_indent_unicode_and_supplied_field_order_are_preserved() -> TestResult {
    let record = OrderedRecord {
        zeta: "τ ש 🦀\n\"\\\u{0001}\u{2028}",
        alpha: [1e-5, -0.0],
        empty: Vec::new(),
    };
    assert_eq!(
        bytes(&record)?,
        "{\n  \"zeta\": \"τ ש 🦀\\n\\\"\\\\\\u0001\u{2028}\",\n  \"alpha\": [\n    1e-05,\n    -0.0\n  ],\n  \"empty\": []\n}"
    );
    let mut ordered = serde_json::Map::new();
    ordered.insert("zeta".into(), json!(1));
    ordered.insert("alpha".into(), json!(2));
    assert_eq!(bytes(&ordered)?, "{\n  \"zeta\": 1,\n  \"alpha\": 2\n}");
    Ok(())
}

#[test]
fn primary_values_and_secret_public_fields_use_one_formatter() -> TestResult {
    let sink = Arc::new(MemorySink::default());
    let output = Output::new(
        OutputOptions {
            json_mode: true,
            quiet: true,
            network: None,
        },
        sink.clone(),
    );
    output.value(&OrderedRecord {
        zeta: "τ",
        alpha: [1e-5, -0.0],
        empty: Vec::new(),
    })?;
    assert_eq!(
        sink.text(Stream::Stdout)?,
        "{\n  \"zeta\": \"τ\",\n  \"alpha\": [\n    1e-05,\n    -0.0\n  ],\n  \"empty\": []\n}\n"
    );
    let sink = Arc::new(MemorySink::default());
    let output = Output::new(output.options.clone(), sink.clone());
    output.secret_detail(
        None,
        &[
            ("zeta".into(), json!(1e16)),
            ("alpha".into(), json!(1e-5)),
            ("zeta".into(), json!(1e15)),
        ],
        &[
            ("mnemonic", "τ synthetic fixture"),
            ("alpha", "replacement"),
        ],
    )?;
    assert_eq!(
        sink.text(Stream::Stdout)?,
        "{\n  \"zeta\": 1000000000000000.0,\n  \"alpha\": \"replacement\",\n  \"mnemonic\": \"τ synthetic fixture\"\n}\n"
    );
    assert!(sink.bytes(Stream::Stderr)?.is_empty());
    Ok(())
}

#[test]
fn diagnostics_keep_unicode_and_append_exactly_one_newline() -> TestResult {
    let sink = Arc::new(MemorySink::default());
    let output = Output::new(
        OutputOptions {
            json_mode: true,
            ..Default::default()
        },
        sink.clone(),
    );
    output.error("τ failed", DiagnosticContext::default())?;
    assert_eq!(
        sink.text(Stream::Stderr)?,
        "{\n  \"error\": \"τ failed\"\n}\n"
    );
    assert!(sink.bytes(Stream::Stdout)?.is_empty());
    Ok(())
}
