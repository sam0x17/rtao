//! JSON text in the two layouts v10 peers produce.

use std::io::{self, Write};

use rtao_primitives::pyfmt::float_repr;
use serde::Serialize;
use serde_json::ser::{CharEscape, Formatter, Serializer};

/// Serialize like Python's `json.dumps(value)` as aiohttp sends request bodies.
///
/// Separators are `", "` and `": "`, non-ASCII text is escaped as `\uXXXX`
/// (`ensure_ascii=True`) and floats use Python's `repr`. Callers refuse
/// non-finite floats first; serde_json would write them as `null`, whereas
/// Python writes `NaN`/`Infinity`.
pub fn to_python_json<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, serde_json::Error> {
    let mut out = Vec::new();
    let mut serializer = Serializer::with_formatter(&mut out, PythonFormatter);
    value.serialize(&mut serializer)?;
    Ok(out)
}

/// Serialize like Starlette's `JSONResponse`: compact separators and raw
/// UTF-8 (`ensure_ascii=False`). Non-finite floats become `null`, matching
/// pydantic's JSON-mode dump.
pub fn to_starlette_json<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, serde_json::Error> {
    serde_json::to_vec(value)
}

struct PythonFormatter;

impl Formatter for PythonFormatter {
    fn begin_array_value<W: ?Sized + Write>(
        &mut self,
        writer: &mut W,
        first: bool,
    ) -> io::Result<()> {
        if first {
            Ok(())
        } else {
            writer.write_all(b", ")
        }
    }
    fn begin_object_key<W: ?Sized + Write>(
        &mut self,
        writer: &mut W,
        first: bool,
    ) -> io::Result<()> {
        if first {
            Ok(())
        } else {
            writer.write_all(b", ")
        }
    }
    fn begin_object_value<W: ?Sized + Write>(&mut self, writer: &mut W) -> io::Result<()> {
        writer.write_all(b": ")
    }
    fn write_f64<W: ?Sized + Write>(&mut self, writer: &mut W, value: f64) -> io::Result<()> {
        writer.write_all(float_repr(value).as_bytes())
    }
    fn write_f32<W: ?Sized + Write>(&mut self, writer: &mut W, value: f32) -> io::Result<()> {
        let value: f64 = value.to_string().parse().unwrap_or(f64::NAN);
        writer.write_all(float_repr(value).as_bytes())
    }
    fn write_string_fragment<W: ?Sized + Write>(
        &mut self,
        writer: &mut W,
        fragment: &str,
    ) -> io::Result<()> {
        let mut start = 0;
        for (index, character) in fragment.char_indices() {
            if character.is_ascii() {
                continue;
            }
            writer.write_all(&fragment.as_bytes()[start..index])?;
            let mut units = [0u16; 2];
            for unit in character.encode_utf16(&mut units) {
                write!(writer, "\\u{unit:04x}")?;
            }
            start = index + character.len_utf8();
        }
        writer.write_all(&fragment.as_bytes()[start..])
    }
    fn write_char_escape<W: ?Sized + Write>(
        &mut self,
        writer: &mut W,
        escape: CharEscape,
    ) -> io::Result<()> {
        let text: &[u8] = match escape {
            CharEscape::Quote => b"\\\"",
            CharEscape::ReverseSolidus => b"\\\\",
            CharEscape::Solidus => b"/",
            CharEscape::Backspace => b"\\b",
            CharEscape::FormFeed => b"\\f",
            CharEscape::LineFeed => b"\\n",
            CharEscape::CarriageReturn => b"\\r",
            CharEscape::Tab => b"\\t",
            CharEscape::AsciiControl(byte) => {
                return write!(writer, "\\u{byte:04x}");
            }
        };
        writer.write_all(text)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn python_layout() {
        let value = serde_json::json!({"a": [1, 2.5], "b": "é\u{1F600}\n\u{1}"});
        let text = String::from_utf8(to_python_json(&value).unwrap()).unwrap();
        assert_eq!(
            text,
            r#"{"a": [1, 2.5], "b": "\u00e9\ud83d\ude00\n\u0001"}"#
        );
        assert_eq!(
            String::from_utf8(to_python_json(&(1e16_f64, 12.0_f64, 1e-5_f64)).unwrap()).unwrap(),
            "[1e+16, 12.0, 1e-05]"
        );
        assert_eq!(
            String::from_utf8(to_starlette_json(&(f64::NAN, "é")).unwrap()).unwrap(),
            "[null,\"é\"]"
        );
    }
}
