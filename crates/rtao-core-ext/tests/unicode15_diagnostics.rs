//! Public dispatch-error diagnostics against pinned CPython Unicode 15 cases.

use bittensor_core::codec::value::Value;
use rtao_core_ext::ops::dispatch_error;

#[test]
fn result_message_uses_cpython312_unicode15() -> Result<(), Box<dyn std::error::Error>> {
    for (value, expected) in [
        ("\u{1c89}", "('\\u1c89',)"),
        ("\u{10d40}", "('\\U00010d40',)"),
        ("\u{10d41}", "('\\U00010d41',)"),
        ("\u{11bf1}", "('\\U00011bf1',)"),
        ("\u{116d1}", "('\\U000116d1',)"),
        ("\u{1e4f1}", "('\u{1e4f1}',)"),
        ("plain", "('plain',)"),
        (" ", "(' ',)"),
        ("\n", "('\\n',)"),
        ("\t", "('\\t',)"),
        ("\r", "('\\r',)"),
        ("\\", "('\\\\',)"),
        ("'", "(\"'\",)"),
        ("\"", "('\"',)"),
        ("'\"", "('\\'\"',)"),
        ("\u{85}", "('\\x85',)"),
        ("\u{a0}", "('\\xa0',)"),
        ("\u{200b}", "('\\u200b',)"),
        ("\u{2028}", "('\\u2028',)"),
        ("\u{e000}", "('\\ue000',)"),
        ("\u{10ffff}", "('\\U0010ffff',)"),
    ] {
        let error = dispatch_error::DispatchErrorInfo {
            kind: dispatch_error::DispatchErrorKind::System,
            pallet: None,
            name: "Probe".into(),
            docs: Value::Tuple(vec![Value::str(value)]),
        };
        assert_eq!(error.result_message()?, expected, "{value:?}");
    }
    Ok(())
}
