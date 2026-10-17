// Derived from macrocosm-os/apex shared/common/src/common/utils/epistula.py @ bd71ceb070cd72b4bd80562e96884b66e86d908a (MIT); modified by rtao.
//! Apex `create_message_body`: Python `json.dumps(data, default=str, sort_keys=True)`.
//!
//! Apex clients sign this text but send the body serialized without
//! `sort_keys`. Send the canonical bytes as the wire body too, so receivers
//! that hash the raw body agree with the signature.

use std::fmt::Write as _;

use num_bigint::BigInt;
use rtao_primitives::pyfmt;
use serde_json::Value;

/// Serialize like Python `json.dumps(value, default=str, sort_keys=True)`.
///
/// Separators are `", "` and `": "`; non-ASCII text uses lowercase `\uXXXX`
/// escapes with surrogate pairs; keys sort by code point. Integer literals
/// are exact Python integers. Other numbers are binary64 floats in Python
/// `repr` form, with `Infinity` for out-of-range literals.
pub fn canonical_json(value: &Value) -> Vec<u8> {
    let mut out = String::new();
    write_value(&mut out, value);
    out.into_bytes()
}

fn write_value(out: &mut String, value: &Value) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Number(number) => write_number(out, &number.to_string()),
        Value::String(text) => write_string(out, text),
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                write_value(out, item);
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut entries: Vec<_> = map.iter().collect();
            // UTF-8 byte order equals Python's code point order for `str`.
            entries.sort_by(|left, right| left.0.cmp(right.0));
            out.push('{');
            for (index, (key, item)) in entries.into_iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                write_string(out, key);
                out.push_str(": ");
                write_value(out, item);
            }
            out.push('}');
        }
    }
}

fn write_number(out: &mut String, text: &str) {
    if !text.contains(['.', 'e', 'E'])
        && let Ok(integer) = text.parse::<BigInt>()
    {
        let _ = write!(out, "{integer}");
        return;
    }
    match text.parse::<f64>() {
        Ok(value) if value.is_infinite() => {
            out.push_str(if value > 0.0 { "Infinity" } else { "-Infinity" });
        }
        Ok(value) => out.push_str(&pyfmt::float_repr(value)),
        Err(_) => out.push_str(text),
    }
}

fn write_string(out: &mut String, text: &str) {
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            ' '..='~' => out.push(character),
            _ => {
                let mut units = [0u16; 2];
                for unit in character.encode_utf16(&mut units) {
                    let _ = write!(out, "\\u{unit:04x}");
                }
            }
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    fn dumps(text: &str) -> String {
        String::from_utf8(canonical_json(&serde_json::from_str(text).unwrap())).unwrap()
    }

    #[test]
    fn matches_python_sort_keys_dumps() {
        assert_eq!(dumps(r#"{"b":1,"a":"x"}"#), r#"{"a": "x", "b": 1}"#);
        assert_eq!(dumps("{}"), "{}");
        assert_eq!(dumps("[1,[2,{}],null,true]"), "[1, [2, {}], null, true]");
        assert_eq!(
            dumps("[1.0,1e16,-0.0,1e400,0.1,-0]"),
            "[1.0, 1e+16, -0.0, Infinity, 0.1, 0]"
        );
        assert_eq!(
            dumps("123456789012345678901234567890"),
            "123456789012345678901234567890"
        );
        assert_eq!(
            dumps(r#""é😀\u0001\u007f\"\\/""#),
            r#""\u00e9\ud83d\ude00\u0001\u007f\"\\/""#
        );
        assert_eq!(
            dumps(r#"{"\u00e9":1,"z":2,"\ud83d\ude00":3,"\uffff":4}"#),
            r#"{"z": 2, "\u00e9": 1, "\uffff": 4, "\ud83d\ude00": 3}"#
        );
    }
}
