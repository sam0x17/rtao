// Derived from opentensor/bittensor bittensor/core/tensor.py @ v10.5.0 (MIT); modified by rtao.
//! bittensor's `Tensor`: a base64 msgpack-numpy buffer with dtype and shape.
//!
//! The buffer is msgpack-numpy's map `{b"nd": True, b"type": "<f4",
//! b"kind": b"", b"shape": [..], b"data": <C-order bytes>}` with binary keys.
//! A scalar is sent with `shape == [0]` and an empty array shape. Note that
//! bittensor 10.5.0 cannot validate any `Tensor` unless torch is installed:
//! its dtype table imports torch on first lookup.

use base64::Engine as _;
use rtao_primitives::pyfmt::parse_integer;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

/// A serialized tensor, with pydantic's field order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tensor {
    /// Base64 msgpack-numpy buffer.
    #[serde(default)]
    pub buffer: Option<String>,
    /// numpy (`float32`) or torch (`torch.float32`) dtype name.
    pub dtype: String,
    /// Shape; `[0]` for a scalar. Python's `cast_shape` also accepts text
    /// such as `"[2, 3]"`, and refuses an empty shape.
    #[serde(deserialize_with = "de_shape")]
    pub shape: Vec<i64>,
}

/// Python's `cast_shape` followed by pydantic's lax `list[int]`.
fn de_shape<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<i64>, D::Error> {
    use serde::de::Error;
    let int = |text: &str| {
        parse_integer(text)
            .and_then(|value| i64::try_from(value).ok())
            .ok_or_else(|| D::Error::custom(format!("shape element {text:?} is not an int")))
    };
    match Value::deserialize(deserializer)? {
        Value::String(text) if !text.is_empty() => {
            // raw.split("[")[1].split("]")[0].split(",")
            let inner = text
                .split('[')
                .nth(1)
                .ok_or_else(|| D::Error::custom("shape text has no '['"))?;
            let inner = inner.split(']').next().unwrap_or("");
            inner.split(',').map(int).collect()
        }
        Value::Array(items) if !items.is_empty() => {
            if !items[0].is_i64() && !items[0].is_u64() {
                return Err(D::Error::custom("shape list elements are not of type int"));
            }
            items
                .iter()
                .map(|item| match item {
                    Value::Number(number) => number
                        .as_i64()
                        .or_else(|| {
                            number
                                .as_f64()
                                .filter(|value| value.fract() == 0.0 && value.abs() < 9.0e15)
                                .map(|value| value as i64)
                        })
                        .ok_or_else(|| D::Error::custom("shape element is not an int")),
                    Value::String(text) => int(text),
                    _ => Err(D::Error::custom("shape element is not an int")),
                })
                .collect()
        }
        _ => Err(D::Error::custom("shape must be a non-empty list of int")),
    }
}

/// A decoded msgpack-numpy array.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawArray {
    /// numpy `dtype.str`, such as `<f4`.
    pub type_str: String,
    /// Array shape (empty for a scalar).
    pub shape: Vec<u64>,
    /// Little-endian C-order element bytes.
    pub data: Vec<u8>,
}

/// A tensor could not be encoded or decoded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TensorError(pub String);

impl std::fmt::Display for TensorError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}
impl std::error::Error for TensorError {}

fn error(message: impl Into<String>) -> TensorError {
    TensorError(message.into())
}

const NUMPY_TYPES: [(&str, &str, usize); 9] = [
    ("float16", "<f2", 2),
    ("float32", "<f4", 4),
    ("float64", "<f8", 8),
    ("uint8", "|u1", 1),
    ("int16", "<i2", 2),
    ("int8", "|i1", 1),
    ("int32", "<i4", 4),
    ("int64", "<i8", 8),
    ("bool", "|b1", 1),
];

/// numpy `dtype.str` and item size for a bittensor dtype name.
pub fn numpy_type(dtype: &str) -> Option<(&'static str, usize)> {
    let name = dtype.strip_prefix("torch.").unwrap_or(dtype);
    NUMPY_TYPES
        .iter()
        .find(|(known, _, _)| *known == name)
        .map(|(_, type_str, size)| (*type_str, *size))
}

/// Item size of an allowed numpy `dtype.str`.
fn item_size(type_str: &str) -> Option<usize> {
    NUMPY_TYPES
        .iter()
        .find(|(_, known, _)| *known == type_str)
        .map(|(_, _, size)| *size)
}

fn element_count(shape: &[u64]) -> Option<u64> {
    shape
        .iter()
        .try_fold(1u64, |count, dim| count.checked_mul(*dim))
}

impl Tensor {
    /// Serialize C-order little-endian element bytes of an array.
    pub fn from_raw(dtype: &str, shape: &[u64], data: &[u8]) -> Result<Self, TensorError> {
        let (type_str, size) =
            numpy_type(dtype).ok_or_else(|| error(format!("{dtype} is not an allowed dtype")))?;
        let count = element_count(shape).ok_or_else(|| error("shape overflows"))?;
        if u64::try_from(data.len()).ok() != count.checked_mul(size as u64) {
            return Err(error("data length does not match shape and dtype"));
        }
        if u32::try_from(data.len()).is_err() || u32::try_from(shape.len()).is_err() {
            return Err(error("tensor exceeds msgpack's 32-bit lengths"));
        }
        let mut packed = Vec::with_capacity(data.len() + 64);
        packed.push(0x85);
        bin(&mut packed, b"nd");
        packed.push(0xc3);
        bin(&mut packed, b"type");
        str8(&mut packed, type_str);
        bin(&mut packed, b"kind");
        bin(&mut packed, b"");
        bin(&mut packed, b"shape");
        array_header(&mut packed, shape.len());
        for dim in shape {
            uint(&mut packed, *dim);
        }
        bin(&mut packed, b"data");
        bin(&mut packed, data);
        let shape = if shape.is_empty() {
            vec![0]
        } else {
            shape
                .iter()
                .map(|dim| i64::try_from(*dim).map_err(|_| error("dimension exceeds i64")))
                .collect::<Result<_, _>>()?
        };
        Ok(Self {
            buffer: Some(base64::engine::general_purpose::STANDARD.encode(packed)),
            dtype: dtype.to_owned(),
            shape,
        })
    }

    /// Decode the buffer.
    ///
    /// The buffer's numpy type must be this tensor's `dtype` (Python would
    /// cast instead), its data must hold exactly the elements of its shape
    /// (numpy accepts a longer buffer), and unless `shape` is `[0]` the
    /// element counts must agree as Python's `reshape(shape)` requires.
    pub fn to_raw(&self) -> Result<RawArray, TensorError> {
        let buffer = self
            .buffer
            .as_deref()
            .ok_or_else(|| error("tensor has no buffer"))?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(buffer)
            .map_err(|decode| error(decode.to_string()))?;
        let mut reader = Reader {
            bytes: &bytes,
            at: 0,
        };
        let entries = reader.map_len()?;
        let mut raw = RawArray {
            type_str: String::new(),
            shape: Vec::new(),
            data: Vec::new(),
        };
        let mut is_array = false;
        for _ in 0..entries {
            let key = reader.bytes_or_str()?;
            match key.as_slice() {
                b"nd" => is_array = reader.boolean()?,
                b"type" => {
                    raw.type_str = String::from_utf8(reader.bytes_or_str()?)
                        .map_err(|_| error("type is not UTF-8"))?;
                }
                b"kind" => {
                    reader.bytes_or_str()?;
                }
                b"shape" => {
                    let length = reader.array_len()?;
                    raw.shape = (0..length)
                        .map(|_| reader.uint())
                        .collect::<Result<_, _>>()?;
                }
                b"data" => raw.data = reader.bytes_or_str()?,
                _ => return Err(error("unexpected msgpack-numpy key")),
            }
        }
        if !is_array {
            return Err(error("buffer is not a numpy array"));
        }
        let (expected, _) = numpy_type(&self.dtype)
            .ok_or_else(|| error(format!("{} is not an allowed dtype", self.dtype)))?;
        if raw.type_str != expected {
            return Err(error(format!(
                "buffer type {} does not match dtype {}",
                raw.type_str, self.dtype
            )));
        }
        let size = item_size(&raw.type_str).ok_or_else(|| error("unknown buffer type"))?;
        let count = element_count(&raw.shape).ok_or_else(|| error("shape overflows"))?;
        if u64::try_from(raw.data.len()).ok() != count.checked_mul(size as u64) {
            return Err(error("buffer length does not match its shape and type"));
        }
        if self.shape != [0] && !reshapes(count, &self.shape) {
            return Err(error(format!(
                "cannot reshape {count} elements into {:?}",
                self.shape
            )));
        }
        Ok(raw)
    }
}

/// numpy's `reshape` rule: equal element counts, with at most one `-1`
/// dimension inferred.
fn reshapes(count: u64, shape: &[i64]) -> bool {
    let mut known: u64 = 1;
    let mut inferred = 0;
    for dim in shape {
        match u64::try_from(*dim) {
            Ok(dim) => match known.checked_mul(dim) {
                Some(product) => known = product,
                None => return false,
            },
            Err(_) if *dim == -1 => inferred += 1,
            Err(_) => return false,
        }
    }
    match inferred {
        0 => known == count,
        1 => known != 0 && count.is_multiple_of(known),
        _ => false,
    }
}

// Lengths are checked against u32 by `Tensor::from_raw` before packing.
fn bin(out: &mut Vec<u8>, bytes: &[u8]) {
    let length = bytes.len();
    if let Ok(short) = u8::try_from(length) {
        out.extend_from_slice(&[0xc4, short]);
    } else if let Ok(medium) = u16::try_from(length) {
        out.push(0xc5);
        out.extend_from_slice(&medium.to_be_bytes());
    } else {
        out.push(0xc6);
        out.extend_from_slice(&u32::try_from(length).unwrap_or(u32::MAX).to_be_bytes());
    }
    out.extend_from_slice(bytes);
}

fn str8(out: &mut Vec<u8>, text: &str) {
    // Type strings are at most four characters: a fixstr.
    out.push(0xa0 | (text.len() as u8 & 0x1f));
    out.extend_from_slice(text.as_bytes());
}

fn array_header(out: &mut Vec<u8>, length: usize) {
    if length < 16 {
        out.push(0x90 | length as u8);
    } else if let Ok(short) = u16::try_from(length) {
        out.push(0xdc);
        out.extend_from_slice(&short.to_be_bytes());
    } else {
        out.push(0xdd);
        out.extend_from_slice(&u32::try_from(length).unwrap_or(u32::MAX).to_be_bytes());
    }
}

fn uint(out: &mut Vec<u8>, value: u64) {
    if value < 0x80 {
        out.push(value as u8);
    } else if let Ok(byte) = u8::try_from(value) {
        out.extend_from_slice(&[0xcc, byte]);
    } else if let Ok(short) = u16::try_from(value) {
        out.push(0xcd);
        out.extend_from_slice(&short.to_be_bytes());
    } else if let Ok(word) = u32::try_from(value) {
        out.push(0xce);
        out.extend_from_slice(&word.to_be_bytes());
    } else {
        out.push(0xcf);
        out.extend_from_slice(&value.to_be_bytes());
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn take(&mut self, count: usize) -> Result<&[u8], TensorError> {
        let end = self
            .at
            .checked_add(count)
            .filter(|end| *end <= self.bytes.len());
        let end = end.ok_or_else(|| error("truncated msgpack"))?;
        let slice = &self.bytes[self.at..end];
        self.at = end;
        Ok(slice)
    }
    fn byte(&mut self) -> Result<u8, TensorError> {
        Ok(self.take(1)?[0])
    }
    fn be(&mut self, width: usize) -> Result<u64, TensorError> {
        Ok(self
            .take(width)?
            .iter()
            .fold(0u64, |value, byte| (value << 8) | u64::from(*byte)))
    }
    fn length(&mut self, width: usize) -> Result<usize, TensorError> {
        usize::try_from(self.be(width)?).map_err(|_| error("msgpack length overflows"))
    }
    fn map_len(&mut self) -> Result<usize, TensorError> {
        match self.byte()? {
            tag @ 0x80..=0x8f => Ok(usize::from(tag & 0x0f)),
            0xde => self.length(2),
            0xdf => self.length(4),
            _ => Err(error("expected a msgpack map")),
        }
    }
    fn array_len(&mut self) -> Result<usize, TensorError> {
        match self.byte()? {
            tag @ 0x90..=0x9f => Ok(usize::from(tag & 0x0f)),
            0xdc => self.length(2),
            0xdd => self.length(4),
            _ => Err(error("expected a msgpack array")),
        }
    }
    fn bytes_or_str(&mut self) -> Result<Vec<u8>, TensorError> {
        let length = match self.byte()? {
            tag @ 0xa0..=0xbf => usize::from(tag & 0x1f),
            0xc4 | 0xd9 => self.length(1)?,
            0xc5 | 0xda => self.length(2)?,
            0xc6 | 0xdb => self.length(4)?,
            _ => return Err(error("expected msgpack bytes or str")),
        };
        Ok(self.take(length)?.to_vec())
    }
    fn boolean(&mut self) -> Result<bool, TensorError> {
        match self.byte()? {
            0xc2 => Ok(false),
            0xc3 => Ok(true),
            _ => Err(error("expected a msgpack bool")),
        }
    }
    fn uint(&mut self) -> Result<u64, TensorError> {
        match self.byte()? {
            tag @ 0x00..=0x7f => Ok(u64::from(tag)),
            0xcc => self.be(1),
            0xcd => self.be(2),
            0xce => self.be(4),
            0xcf => self.be(8),
            0xd0 => self.be(1).and_then(non_negative::<1>),
            0xd1 => self.be(2).and_then(non_negative::<2>),
            0xd2 => self.be(4).and_then(non_negative::<4>),
            0xd3 => self.be(8).and_then(non_negative::<8>),
            _ => Err(error("expected a non-negative msgpack integer")),
        }
    }
}

fn non_negative<const WIDTH: usize>(value: u64) -> Result<u64, TensorError> {
    if value >> (WIDTH * 8 - 1) & 1 == 1 {
        Err(error("negative dimension"))
    } else {
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn matches_python_buffers() {
        let data: Vec<u8> = [1.5f32, 2.0, 3.0, 4.0]
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect();
        let tensor = Tensor::from_raw("float32", &[2, 2], &data).unwrap();
        assert_eq!(
            tensor.buffer.as_deref(),
            Some(
                "hcQCbmTDxAR0eXBlozxmNMQEa2luZMQAxAVzaGFwZZICAsQEZGF0YcQQAADAPwAAAEAAAEBAAACAQA=="
            )
        );
        assert_eq!(tensor.shape, vec![2, 2]);
        let raw = tensor.to_raw().unwrap();
        assert_eq!(
            (raw.type_str.as_str(), raw.shape, raw.data),
            ("<f4", vec![2, 2], data)
        );

        let scalar = Tensor::from_raw("int64", &[], &3i64.to_le_bytes()).unwrap();
        assert_eq!(
            scalar.buffer.as_deref(),
            Some("hcQCbmTDxAR0eXBlozxpOMQEa2luZMQAxAVzaGFwZZDEBGRhdGHECAMAAAAAAAAA")
        );
        assert_eq!(scalar.shape, vec![0]);
        let flags = Tensor::from_raw("bool", &[2], &[1, 0]).unwrap();
        assert_eq!(
            flags.buffer.as_deref(),
            Some("hcQCbmTDxAR0eXBlo3xiMcQEa2luZMQAxAVzaGFwZZECxARkYXRhxAIBAA==")
        );
        assert!(Tensor::from_raw("complex64", &[1], &[0; 8]).is_err());
        assert!(Tensor::from_raw("int8", &[2], &[0]).is_err());
    }

    #[test]
    fn decoding_checks_buffer_against_shape_and_dtype() {
        let data: Vec<u8> = [1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0]
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect();
        let mut tensor = Tensor::from_raw("float32", &[2, 3], &data).unwrap();
        tensor.shape = vec![3, -1];
        assert_eq!(tensor.to_raw().unwrap().shape, vec![2, 3]);
        tensor.shape = vec![4, 2];
        assert!(tensor.to_raw().is_err(), "reshape count mismatch");
        tensor.shape = vec![-1, -1];
        assert!(tensor.to_raw().is_err());
        tensor.shape = vec![2, 3];
        tensor.dtype = "int32".into();
        assert!(tensor.to_raw().is_err(), "dtype mismatch");

        // A buffer whose data is shorter than its own shape.
        let short = Tensor::from_raw("float32", &[1], &data[..4]).unwrap();
        let mut packed = base64::engine::general_purpose::STANDARD
            .decode(short.buffer.unwrap())
            .unwrap();
        let shape_at = packed.windows(2).position(|w| w == [0x91, 0x01]).unwrap();
        packed[shape_at + 1] = 0x02;
        let forged = Tensor {
            buffer: Some(base64::engine::general_purpose::STANDARD.encode(packed)),
            dtype: "float32".into(),
            shape: vec![2],
        };
        assert!(forged.to_raw().is_err(), "buffer shorter than shape");

        // Many dimensions use a 32-bit msgpack array header.
        let dims = vec![1u64; 70_000];
        let wide = Tensor::from_raw("uint8", &dims, &[7]).unwrap();
        let raw = wide.to_raw().unwrap();
        assert_eq!((raw.shape.len(), raw.data), (70_000, vec![7]));
    }

    #[test]
    fn shape_follows_python_cast_shape() {
        let parse = |shape: &str| {
            serde_json::from_str::<Tensor>(&format!(r#"{{"dtype":"float32","shape":{shape}}}"#))
                .map(|tensor| tensor.shape)
        };
        assert_eq!(parse(r#""[2, 3]""#).unwrap(), vec![2, 3]);
        assert_eq!(parse(r#""(4)[ 5 ,6 ]""#).unwrap(), vec![5, 6]);
        assert_eq!(parse("[2, 3.0, \"4\"]").unwrap(), vec![2, 3, 4]);
        for bad in [
            "[]", "\"\"", "\"2, 3\"", "[2.0, 3]", "null", "[\"x\"]", "\"[]\"",
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }
}
