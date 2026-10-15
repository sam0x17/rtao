// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/output.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! JSON whitespace and float notation shared by every machine output path.

use std::io::{self, Write};

use rtao::primitives::pyfmt::float_repr;
use serde::Serialize;
use serde_json::ser::{Formatter, PrettyFormatter};

/// Two-space JSON with Python's finite float notation and unescaped Unicode.
///
/// Objects keep the order supplied by their serializer. Integer tokens retain
/// all their digits, including integers outside the chain's balance domain.
/// Non-finite Rust floats retain serde_json's existing `null` behavior.
#[derive(Clone, Debug, Default)]
pub struct PyJsonFormatter(PrettyFormatter<'static>);

impl PyJsonFormatter {
    /// Construct a formatter with two spaces per nesting level.
    pub fn new() -> Self {
        Self::default()
    }

    /// Serialize into a supplied writer without allocating a whole document.
    /// A terminating newline belongs to the caller's output protocol.
    pub fn to_writer<W: Write, T: Serialize + ?Sized>(
        writer: W,
        value: &T,
    ) -> Result<(), serde_json::Error> {
        value.serialize(&mut serde_json::Serializer::with_formatter(
            writer,
            Self::new(),
        ))
    }
}

impl Formatter for PyJsonFormatter {
    fn write_f32<W: Write + ?Sized>(&mut self, writer: &mut W, value: f32) -> io::Result<()> {
        self.write_f64(writer, f64::from(value))
    }

    fn write_f64<W: Write + ?Sized>(&mut self, writer: &mut W, value: f64) -> io::Result<()> {
        writer.write_all(float_repr(value).as_bytes())
    }

    fn write_number_str<W: Write + ?Sized>(
        &mut self,
        writer: &mut W,
        value: &str,
    ) -> io::Result<()> {
        // serde_json's arbitrary_precision Number serializes through this
        // method, rather than write_f64. Preserve exact integer tokens and
        // out-of-range custom JSON decimals; format representable floats with
        // the same routine as directly serialized f64 values.
        if value.bytes().any(|byte| matches!(byte, b'.' | b'e' | b'E'))
            && let Ok(number) = value.parse::<f64>()
            && number.is_finite()
        {
            return self.write_f64(writer, number);
        }
        writer.write_all(value.as_bytes())
    }

    fn begin_array<W: Write + ?Sized>(&mut self, writer: &mut W) -> io::Result<()> {
        self.0.begin_array(writer)
    }

    fn end_array<W: Write + ?Sized>(&mut self, writer: &mut W) -> io::Result<()> {
        self.0.end_array(writer)
    }

    fn begin_array_value<W: Write + ?Sized>(
        &mut self,
        writer: &mut W,
        first: bool,
    ) -> io::Result<()> {
        self.0.begin_array_value(writer, first)
    }

    fn end_array_value<W: Write + ?Sized>(&mut self, writer: &mut W) -> io::Result<()> {
        self.0.end_array_value(writer)
    }

    fn begin_object<W: Write + ?Sized>(&mut self, writer: &mut W) -> io::Result<()> {
        self.0.begin_object(writer)
    }

    fn end_object<W: Write + ?Sized>(&mut self, writer: &mut W) -> io::Result<()> {
        self.0.end_object(writer)
    }

    fn begin_object_key<W: Write + ?Sized>(
        &mut self,
        writer: &mut W,
        first: bool,
    ) -> io::Result<()> {
        self.0.begin_object_key(writer, first)
    }

    fn end_object_key<W: Write + ?Sized>(&mut self, writer: &mut W) -> io::Result<()> {
        self.0.end_object_key(writer)
    }

    fn begin_object_value<W: Write + ?Sized>(&mut self, writer: &mut W) -> io::Result<()> {
        self.0.begin_object_value(writer)
    }

    fn end_object_value<W: Write + ?Sized>(&mut self, writer: &mut W) -> io::Result<()> {
        self.0.end_object_value(writer)
    }
}
