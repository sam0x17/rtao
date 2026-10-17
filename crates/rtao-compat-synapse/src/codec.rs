// Derived from opentensor/bittensor bittensor/core/synapse.py @ v10.5.0 (MIT); modified by rtao.
//! v10 synapse model, headers and body hash.

use std::{collections::HashMap, fmt, net::SocketAddr};

use http::{HeaderMap, HeaderName, HeaderValue};
use num_bigint::BigInt;
use num_traits::ToPrimitive;
use rtao_primitives::pyfmt::{float_repr, parse_float, parse_integer};
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{DeserializeOwned, MapAccess, Visitor},
};
use serde_json::{Value, value::RawValue};

use crate::{
    pyjson::{self, Node},
    pyvalue::{PyValue, PyValueError, get_hash, py_str, to_py_value},
};

/// Codec failure. The axon maps every variant to Python's 500 response.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CodecError {
    /// A header that must appear at most once appeared several times.
    DuplicateHeader(String),
    /// A header value could not be parsed into its Python field type.
    InvalidHeader(String),
    /// A header value would not be visible ASCII, space or tab.
    UnsendableHeader(String),
    /// The JSON body did not validate as this synapse.
    Body(String),
    /// A value has no exact Python `str()` equivalent.
    Unsupported(String),
}

impl fmt::Display for CodecError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateHeader(name) => write!(formatter, "duplicate header {name}"),
            Self::InvalidHeader(message) => write!(formatter, "invalid header: {message}"),
            Self::UnsendableHeader(name) => {
                write!(formatter, "header {name} is not visible ASCII text")
            }
            Self::Body(message) => write!(formatter, "invalid synapse body: {message}"),
            Self::Unsupported(message) => formatter.write_str(message),
        }
    }
}
impl std::error::Error for CodecError {}
impl From<PyValueError> for CodecError {
    fn from(error: PyValueError) -> Self {
        Self::Unsupported(error.0)
    }
}

/// Python's `TerminalInfo`: one side's identity and status.
///
/// Field order is the pydantic order, which fixes JSON key order and header
/// emission order. Integer fields deserialize with Python's `int()` coercion
/// (`8091.7` becomes `8091`, `"12"` becomes `12`); the nonce is held as `i64`
/// on this path (Python accepts any integer; `time_ns()` fits until 2262).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TerminalInfo {
    /// HTTP-like status code.
    #[serde(default, deserialize_with = "de_py_int")]
    pub status_code: Option<i64>,
    /// Status text.
    #[serde(default)]
    pub status_message: Option<String>,
    /// Processing time in seconds.
    #[serde(default, deserialize_with = "de_py_float")]
    pub process_time: Option<f64>,
    /// IP address text.
    #[serde(default)]
    pub ip: Option<String>,
    /// TCP port.
    #[serde(default, deserialize_with = "de_py_int")]
    pub port: Option<i64>,
    /// `bittensor.__version_as_int__` of the peer.
    #[serde(default, deserialize_with = "de_py_int")]
    pub version: Option<i64>,
    /// Unix-nanosecond nonce.
    #[serde(default, deserialize_with = "de_py_int")]
    pub nonce: Option<i64>,
    /// Instance uuid text.
    #[serde(default)]
    pub uuid: Option<String>,
    /// SS58 hotkey text.
    #[serde(default)]
    pub hotkey: Option<String>,
    /// `0x`-prefixed hex signature.
    #[serde(default)]
    pub signature: Option<String>,
}

impl TerminalInfo {
    /// Python `model_dump()` items with their `str()` header values.
    pub fn header_items(&self) -> Vec<(&'static str, String)> {
        let mut items = Vec::new();
        let mut push = |name: &'static str, value: Option<String>| {
            if let Some(value) = value {
                items.push((name, value));
            }
        };
        push(
            "status_code",
            self.status_code.map(|value| value.to_string()),
        );
        push("status_message", self.status_message.clone());
        push("process_time", self.process_time.map(float_repr));
        push("ip", self.ip.clone());
        push("port", self.port.map(|value| value.to_string()));
        push("version", self.version.map(|value| value.to_string()));
        push("nonce", self.nonce.map(|value| value.to_string()));
        push("uuid", self.uuid.clone());
        push("hotkey", self.hotkey.clone());
        push("signature", self.signature.clone());
        items
    }

    /// Overwrite fields with every non-`None` field of `other`, as the Python
    /// dendrite merges response headers.
    pub fn merge_present(&mut self, other: &Self) {
        macro_rules! take {
            ($($field:ident),*) => {$(
                if other.$field.is_some() {
                    self.$field.clone_from(&other.$field);
                }
            )*};
        }
        take!(
            status_code,
            status_message,
            process_time,
            ip,
            port,
            version,
            nonce,
            uuid,
            hotkey,
            signature
        );
    }

    fn set_header_field(&mut self, field: &str, value: &str) -> Result<(), CodecError> {
        let text = || Some(value.to_owned());
        match field {
            "status_code" => self.status_code = Some(header_i64(field, value)?),
            "status_message" => self.status_message = text(),
            "process_time" => self.process_time = Some(header_f64(field, value)?),
            "ip" => self.ip = text(),
            "port" => self.port = Some(header_i64(field, value)?),
            "version" => self.version = Some(header_i64(field, value)?),
            "nonce" => self.nonce = Some(header_i64(field, value)?),
            "uuid" => self.uuid = text(),
            "hotkey" => self.hotkey = text(),
            "signature" => self.signature = text(),
            // pydantic `extra="ignore"` drops unknown keys.
            _ => {}
        }
        Ok(())
    }
}

/// Runtime Python type whose empty instance fills `bt_header_input_obj_*`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DummyKind {
    /// `str()` → `""`.
    Str,
    /// `int()` → `0`.
    Int,
    /// `float()` → `0.0`.
    Float,
    /// `bool()` → `false`.
    Bool,
    /// `list()`/`tuple()` → `[]`.
    List,
    /// `dict()` and nested models → `{}`.
    Dict,
}

impl DummyKind {
    /// The kind of a Python value; `None` emits no header.
    pub fn of(value: &PyValue) -> Option<Self> {
        Some(match value {
            PyValue::None => return None,
            PyValue::Bool(_) => Self::Bool,
            PyValue::Int(_) => Self::Int,
            PyValue::Float(_) => Self::Float,
            PyValue::Str(_) => Self::Str,
            PyValue::List(_) | PyValue::Tuple(_) => Self::List,
            PyValue::Dict(_) => Self::Dict,
        })
    }

    /// `base64(json.dumps(T()))`.
    pub fn header_value(self) -> &'static str {
        match self {
            Self::Str => "IiI=",
            Self::Int => "MA==",
            Self::Float => "MC4w",
            Self::Bool => "ZmFsc2U=",
            Self::List => "W10=",
            Self::Dict => "e30=",
        }
    }
}

/// A Python `Synapse` subclass's body fields and class metadata.
///
/// The implementing type holds only the subclass fields; [`Envelope`] adds
/// the base fields. Serialization order must equal pydantic's field order.
/// Use Rust types that mirror the Python annotations (`i64` for `int`, `f64`
/// for `float`, `Vec` for `list`, tuples for `tuple`, [`crate::PyDict`] for
/// `dict`), since the body hash renders the typed values with `str()`.
pub trait Synapse: Serialize + DeserializeOwned + Clone + Send + Sync + 'static {
    /// Python class name: the route `/NAME` and the `name` field. It must
    /// satisfy [`is_valid_synapse_name`]; attaching or calling a synapse with
    /// another name fails to compile.
    const NAME: &'static str;
    /// Hash fields in effective order: class-variable tuple order, or
    /// `model_dump()` order for the deprecated pydantic-field form. In that
    /// form the `required_hash_fields` field itself is serialized first among
    /// the subclass fields (pydantic keeps the base class's position), with
    /// the default value Python uses.
    const REQUIRED_HASH_FIELDS: &'static [&'static str] = &[];
    /// Fields with no Python default; each non-`None` one gets an input-obj
    /// dummy header of its runtime type.
    const REQUIRED_FIELDS: &'static [&'static str] = &[];
    /// Fields declared `frozen=True`; a 200 response never overwrites them.
    const FROZEN_FIELDS: &'static [&'static str] = &[];

    /// Python values of [`Self::REQUIRED_HASH_FIELDS`], in that order.
    ///
    /// Override when a Rust type does not mirror the Python type.
    fn hash_values(&self) -> Result<Vec<PyValue>, CodecError> {
        default_hash_values(self)
    }
}

/// Look up the hash fields in the serialized body.
pub fn default_hash_values<S: Synapse>(body: &S) -> Result<Vec<PyValue>, CodecError> {
    let dump = to_py_value(body)?;
    S::REQUIRED_HASH_FIELDS
        .iter()
        .map(|field| {
            dump.get(field).cloned().ok_or_else(|| {
                CodecError::Unsupported(format!("hash field {field} is not serialized"))
            })
        })
        .collect()
}

/// The base `bittensor.Synapse`, attached at `/Synapse` as a ping.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BaseSynapse {}

impl Synapse for BaseSynapse {
    const NAME: &'static str = "Synapse";
}

/// A complete v10 wire object: the base fields followed by the subclass body.
#[derive(Clone, Debug, PartialEq)]
pub struct Envelope<S> {
    /// Class name; forced to `S::NAME` when parsed, as pydantic does.
    pub name: Option<String>,
    /// Timeout in seconds (default 12.0).
    pub timeout: Option<f64>,
    /// Informational total size.
    pub total_size: Option<i64>,
    /// Informational header size.
    pub header_size: Option<i64>,
    /// The dendrite (client) terminal.
    pub dendrite: Option<TerminalInfo>,
    /// The axon (server) terminal.
    pub axon: Option<TerminalInfo>,
    /// Body hash as carried in the JSON body (`""` from Python dendrites).
    pub computed_body_hash: Option<String>,
    /// Subclass fields.
    pub body: S,
}

impl<S: Synapse> Envelope<S> {
    /// Python's defaults around a body.
    pub fn new(body: S) -> Self {
        Self {
            name: Some(S::NAME.to_owned()),
            timeout: Some(12.0),
            total_size: Some(0),
            header_size: Some(0),
            dendrite: Some(TerminalInfo::default()),
            axon: Some(TerminalInfo::default()),
            computed_body_hash: Some(String::new()),
            body,
        }
    }

    /// Python's `Synapse.body_hash`.
    pub fn body_hash(&self) -> Result<String, CodecError> {
        let values = self.body.hash_values()?;
        let joined: String = values
            .iter()
            .map(|value| get_hash(&py_str(value)))
            .collect();
        Ok(get_hash(&joined))
    }

    /// The Python-mode `model_dump()` of this envelope.
    pub fn py_dump(&self) -> Result<PyValue, CodecError> {
        Ok(to_py_value(self)?)
    }

    /// Python's `to_headers()` order and formatting, as name/value pairs.
    ///
    /// `header_size` is the byte length of the preceding header lines and
    /// `total_size` is this envelope's `total_size` (Python's values are
    /// CPython object sizes and informational only). Values that are not
    /// visible ASCII, space or tab are refused.
    pub fn header_pairs(&self) -> Result<Vec<(String, String)>, CodecError> {
        let pairs = self.unchecked_header_pairs()?;
        check_sendable(&pairs)?;
        Ok(pairs)
    }

    /// [`Self::header_pairs`] without the visible-ASCII check.
    pub(crate) fn unchecked_header_pairs(&self) -> Result<Vec<(String, String)>, CodecError> {
        let mut pairs: Vec<(String, String)> = vec![
            (
                "name".into(),
                self.name.clone().unwrap_or_else(|| S::NAME.to_owned()),
            ),
            (
                "timeout".into(),
                self.timeout.map_or_else(|| "None".to_owned(), float_repr),
            ),
        ];
        if let Some(axon) = &self.axon {
            for (field, value) in axon.header_items() {
                pairs.push((format!("bt_header_axon_{field}"), value));
            }
        }
        if let Some(dendrite) = &self.dendrite {
            for (field, value) in dendrite.header_items() {
                pairs.push((format!("bt_header_dendrite_{field}"), value));
            }
        }
        if !S::REQUIRED_FIELDS.is_empty()
            && let PyValue::Dict(entries) = to_py_value(&self.body)?
        {
            for (key, value) in entries {
                let PyValue::Str(field) = key else { continue };
                if !S::REQUIRED_FIELDS.contains(&field.as_str())
                    || field == "name"
                    || field == "timeout"
                {
                    continue;
                }
                if let Some(kind) = DummyKind::of(&value) {
                    pairs.push((
                        format!("bt_header_input_obj_{}", field.to_ascii_lowercase()),
                        kind.header_value().to_owned(),
                    ));
                }
            }
        }
        let header_size: usize = pairs
            .iter()
            .map(|(name, value)| name.len() + value.len() + 4)
            .sum();
        pairs.push(("header_size".into(), header_size.to_string()));
        pairs.push((
            "total_size".into(),
            self.total_size.unwrap_or(0).to_string(),
        ));
        pairs.push(("computed_body_hash".into(), self.body_hash()?));
        Ok(pairs)
    }

    /// [`Self::header_pairs`] as an ordered `HeaderMap`.
    pub fn to_headers(&self) -> Result<HeaderMap, CodecError> {
        pairs_to_headers(&self.header_pairs()?)
    }

    /// Parse a JSON body with pydantic's coercions and defaults.
    ///
    /// Subclass fields are deserialized from their original JSON text, so
    /// nested object key order (Python dict order) is preserved. A key that
    /// appears twice keeps its last value at its first position, as Python's
    /// `json.loads` does. Python's bare `NaN`, `Infinity` and `-Infinity`
    /// tokens, which Python dendrites send for non-finite floats, are
    /// accepted as `f64` values.
    pub fn from_json_slice(bytes: &[u8]) -> Result<Self, CodecError> {
        match serde_json::from_slice::<Entries>(bytes) {
            Ok(entries) => Self::from_entries(entries),
            Err(error) => {
                if pyjson::may_have_constants(bytes)
                    && let Ok(text) = std::str::from_utf8(bytes)
                    && let Ok(Node::Object(entries)) = pyjson::parse(text)
                {
                    return Self::from_nodes(entries);
                }
                Err(CodecError::Body(error.to_string()))
            }
        }
    }

    /// Validate a decoded JSON object as this synapse.
    ///
    /// `serde_json::Value` objects may not keep key order; prefer
    /// [`Self::from_json_slice`] when hashed fields contain dicts.
    pub fn from_json_value(value: Value) -> Result<Self, CodecError> {
        let bytes =
            serde_json::to_vec(&value).map_err(|error| CodecError::Body(error.to_string()))?;
        Self::from_json_slice(&bytes)
    }

    fn from_entries(entries: Entries) -> Result<Self, CodecError> {
        let Entries { keys, values } = entries;
        Self::from_nodes(
            keys.into_iter()
                .zip(values.iter().map(|raw| Node::Raw(raw.get())))
                .collect(),
        )
    }

    fn from_nodes(entries: Vec<(String, Node<'_>)>) -> Result<Self, CodecError> {
        let field = |name: &str, node: Node<'_>| -> Result<Value, CodecError> {
            if let Node::NonFinite(_) = node {
                return Err(CodecError::Body(format!(
                    "{name}: unexpected non-finite float"
                )));
            }
            Value::deserialize(node).map_err(|error| CodecError::Body(format!("{name}: {error}")))
        };
        let mut envelope: Envelope<Option<S>> = Envelope {
            name: Some(S::NAME.to_owned()),
            timeout: Some(12.0),
            total_size: Some(0),
            header_size: Some(0),
            dendrite: Some(TerminalInfo::default()),
            axon: Some(TerminalInfo::default()),
            computed_body_hash: Some(String::new()),
            body: None,
        };
        let mut rest: Vec<(String, Node<'_>)> = Vec::new();
        for (key, node) in entries {
            match key.as_str() {
                // pydantic's model validator forces the class name.
                "name" => {}
                "timeout" => {
                    envelope.timeout = match node {
                        Node::NonFinite(value) => Some(value),
                        node => py_float_value(&field(&key, node)?)
                            .map_err(|error| CodecError::Body(format!("timeout: {error}")))?,
                    };
                }
                "total_size" | "header_size" => {
                    let value = py_int_value(&field(&key, node)?)
                        .map_err(|error| CodecError::Body(format!("{key}: {error}")))?;
                    if key == "total_size" {
                        envelope.total_size = value;
                    } else {
                        envelope.header_size = value;
                    }
                }
                "dendrite" | "axon" => {
                    let value = Option::<TerminalInfo>::deserialize(node)
                        .map_err(|error| CodecError::Body(format!("{key}: {error}")))?;
                    if key == "dendrite" {
                        envelope.dendrite = value;
                    } else {
                        envelope.axon = value;
                    }
                }
                "computed_body_hash" => {
                    envelope.computed_body_hash = match field(&key, node)? {
                        Value::Null => None,
                        Value::String(text) => Some(text),
                        _ => {
                            return Err(CodecError::Body(
                                "computed_body_hash: expected a string".into(),
                            ));
                        }
                    };
                }
                _ => rest.push((key, node)),
            }
        }
        let body: S = if rest.iter().all(|(_, node)| node.is_raw()) {
            let mut text = String::from("{");
            for (key, node) in &rest {
                let Node::Raw(raw) = node else { continue };
                if text.len() > 1 {
                    text.push(',');
                }
                text.push_str(
                    &serde_json::to_string(key)
                        .map_err(|error| CodecError::Body(error.to_string()))?,
                );
                text.push(':');
                text.push_str(raw);
            }
            text.push('}');
            serde_json::from_str(&text).map_err(|error| CodecError::Body(error.to_string()))?
        } else {
            S::deserialize(Node::Object(rest))
                .map_err(|error| CodecError::Body(error.to_string()))?
        };
        Ok(Envelope {
            name: envelope.name,
            timeout: envelope.timeout,
            total_size: envelope.total_size,
            header_size: envelope.header_size,
            dendrite: envelope.dendrite,
            axon: envelope.axon,
            computed_body_hash: envelope.computed_body_hash,
            body,
        })
    }

    /// Replace every field Python's 200 merge would replace: all base fields
    /// except `computed_body_hash`, and every subclass field not listed in
    /// [`Synapse::FROZEN_FIELDS`].
    pub fn merge_success(&mut self, server: Self) -> Result<(), CodecError> {
        self.name = server.name;
        self.timeout = server.timeout;
        self.total_size = server.total_size;
        self.header_size = server.header_size;
        self.dendrite = server.dendrite;
        self.axon = server.axon;
        if S::FROZEN_FIELDS.is_empty() {
            self.body = server.body;
            return Ok(());
        }
        let to_entries = |body: &S| -> Result<Entries, CodecError> {
            let text =
                serde_json::to_string(body).map_err(|error| CodecError::Body(error.to_string()))?;
            serde_json::from_str(&text).map_err(|error| CodecError::Body(error.to_string()))
        };
        let local = to_entries(&self.body)?;
        let server = to_entries(&server.body)?;
        let server: HashMap<&str, &RawValue> = server
            .keys
            .iter()
            .map(String::as_str)
            .zip(server.values.iter().map(AsRef::as_ref))
            .collect();
        let mut merged = String::from("{");
        for (key, raw) in local.keys.iter().zip(&local.values) {
            let raw: &RawValue = if S::FROZEN_FIELDS.contains(&key.as_str()) {
                raw
            } else {
                server.get(key.as_str()).copied().unwrap_or(raw)
            };
            if merged.len() > 1 {
                merged.push(',');
            }
            merged.push_str(
                &serde_json::to_string(key).map_err(|error| CodecError::Body(error.to_string()))?,
            );
            merged.push(':');
            merged.push_str(raw.get());
        }
        merged.push('}');
        self.body =
            serde_json::from_str(&merged).map_err(|error| CodecError::Body(error.to_string()))?;
        Ok(())
    }
}

/// Top-level JSON object entries as raw text, last duplicate winning at the
/// first position.
struct Entries {
    keys: Vec<String>,
    values: Vec<Box<RawValue>>,
}

impl<'de> Deserialize<'de> for Entries {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct EntriesVisitor;
        impl<'de> Visitor<'de> for EntriesVisitor {
            type Value = Entries;
            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a JSON object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Entries, A::Error> {
                let mut entries = Entries {
                    keys: Vec::new(),
                    values: Vec::new(),
                };
                // Index by key so that many distinct keys stay linear.
                let mut index: HashMap<String, usize> = HashMap::new();
                while let Some((key, value)) = access.next_entry::<String, Box<RawValue>>()? {
                    if let Some(&slot) = index.get(&key) {
                        entries.values[slot] = value;
                    } else {
                        index.insert(key.clone(), entries.keys.len());
                        entries.keys.push(key);
                        entries.values.push(value);
                    }
                }
                Ok(entries)
            }
        }
        deserializer.deserialize_map(EntriesVisitor)
    }
}

impl<S: Serialize> Serialize for Envelope<S> {
    fn serialize<Z: Serializer>(&self, serializer: Z) -> Result<Z::Ok, Z::Error> {
        #[derive(Serialize)]
        struct Wire<'a, S> {
            name: &'a Option<String>,
            timeout: &'a Option<f64>,
            total_size: &'a Option<i64>,
            header_size: &'a Option<i64>,
            dendrite: &'a Option<TerminalInfo>,
            axon: &'a Option<TerminalInfo>,
            computed_body_hash: &'a Option<String>,
            #[serde(flatten)]
            body: &'a S,
        }
        Wire {
            name: &self.name,
            timeout: &self.timeout,
            total_size: &self.total_size,
            header_size: &self.header_size,
            dendrite: &self.dendrite,
            axon: &self.axon,
            computed_body_hash: &self.computed_body_hash,
            body: &self.body,
        }
        .serialize(serializer)
    }
}

/// JSON only: the subclass body is re-read from raw JSON text.
impl<'de, S: Synapse> Deserialize<'de> for Envelope<S> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let entries = Entries::deserialize(deserializer)?;
        Self::from_entries(entries).map_err(serde::de::Error::custom)
    }
}

/// Request headers as the axon's hooks see them; never the body.
///
/// Python hands hooks a dummy-filled synapse built from headers. rtao passes
/// this view instead and does not decode `bt_header_input_obj_*` headers.
#[derive(Clone, Debug, PartialEq)]
pub struct RequestHead {
    /// Route name (first path segment); the `name` header is ignored.
    pub name: String,
    /// Parsed `timeout` header; `None` when absent.
    pub timeout: Option<f64>,
    /// `bt_header_dendrite_*` values.
    pub dendrite: TerminalInfo,
    /// `bt_header_axon_*` values.
    pub axon: TerminalInfo,
    /// `computed_body_hash` header; `None` when absent.
    pub computed_body_hash: Option<String>,
    /// Exact parsed `bt_header_dendrite_nonce`.
    pub nonce: Option<BigInt>,
    /// Parsed `header_size` header.
    pub header_size: Option<i64>,
    /// Parsed `total_size` header.
    pub total_size: Option<i64>,
    /// TCP peer address.
    pub peer: SocketAddr,
}

impl RequestHead {
    /// Python's header-only synapse fields, for error responses.
    pub fn header_pairs(&self) -> Result<Vec<(String, String)>, CodecError> {
        let pairs = self.unchecked_header_pairs();
        check_sendable(&pairs)?;
        Ok(pairs)
    }

    /// [`Self::header_pairs`] without the visible-ASCII check.
    pub(crate) fn unchecked_header_pairs(&self) -> Vec<(String, String)> {
        let mut pairs: Vec<(String, String)> = vec![
            ("name".into(), self.name.clone()),
            (
                "timeout".into(),
                self.timeout.map_or_else(|| "None".to_owned(), float_repr),
            ),
        ];
        for (field, value) in self.axon.header_items() {
            pairs.push((format!("bt_header_axon_{field}"), value));
        }
        for (field, value) in self.dendrite.header_items() {
            pairs.push((format!("bt_header_dendrite_{field}"), value));
        }
        let header_size: usize = pairs
            .iter()
            .map(|(name, value)| name.len() + value.len() + 4)
            .sum();
        pairs.push(("header_size".into(), header_size.to_string()));
        pairs.push((
            "total_size".into(),
            self.total_size.unwrap_or(0).to_string(),
        ));
        pairs
    }
}

/// Parse request headers with Python's `int()`/`float()` syntax.
///
/// Unlike Python, prefixes match only at the start of a header name, any
/// duplicate `name`, `timeout`, `computed_body_hash`, size or `bt_header_*`
/// header is refused, and input-obj headers are ignored. Values are decoded as
/// Latin-1, as Starlette does. Integer fields must fit `i64`, and authentication
/// fields must fit btauth's D-054 budgets.
pub fn parse_head(
    headers: &HeaderMap,
    route: &str,
    peer: SocketAddr,
) -> Result<RequestHead, CodecError> {
    rtao_btauth::check_auth_header_budget(
        headers,
        &[
            "bt_header_dendrite_hotkey",
            "bt_header_dendrite_nonce",
            "bt_header_dendrite_uuid",
            "bt_header_dendrite_signature",
            "bt_header_axon_hotkey",
            "computed_body_hash",
        ],
        0,
    )
    .map_err(|error| CodecError::InvalidHeader(error.to_string()))?;
    let mut head = RequestHead {
        name: route.to_owned(),
        timeout: None,
        dendrite: TerminalInfo::default(),
        axon: TerminalInfo::default(),
        computed_body_hash: None,
        nonce: None,
        header_size: None,
        total_size: None,
        peer,
    };
    for name in headers.keys() {
        let name = name.as_str();
        let single = matches!(
            name,
            "name" | "timeout" | "computed_body_hash" | "header_size" | "total_size"
        ) || name.starts_with("bt_header_");
        if single && headers.get_all(name).iter().nth(1).is_some() {
            return Err(CodecError::DuplicateHeader(name.to_owned()));
        }
    }
    for (name, value) in headers {
        let text = latin1(value);
        let name = name.as_str();
        if let Some(field) = name.strip_prefix("bt_header_axon_") {
            head.axon.set_header_field(field, &text)?;
        } else if let Some(field) = name.strip_prefix("bt_header_dendrite_") {
            if field == "nonce" {
                head.nonce = Some(header_bigint(name, &text)?);
            }
            head.dendrite.set_header_field(field, &text)?;
        } else {
            match name {
                "timeout" => head.timeout = Some(header_f64(name, &text)?),
                "computed_body_hash" => head.computed_body_hash = Some(text),
                "header_size" => head.header_size = Some(header_i64(name, &text)?),
                "total_size" => head.total_size = Some(header_i64(name, &text)?),
                _ => {}
            }
        }
    }
    Ok(head)
}

fn latin1(value: &HeaderValue) -> String {
    value
        .as_bytes()
        .iter()
        .map(|byte| char::from(*byte))
        .collect()
}

/// Python's `int()` refuses more than 4,300 digits.
const MAX_INT_DIGITS: usize = 4_300;

fn header_bigint(name: &str, text: &str) -> Result<BigInt, CodecError> {
    if text.chars().filter(char::is_ascii_digit).count() > MAX_INT_DIGITS
        || text.len() > 4 * MAX_INT_DIGITS
    {
        return Err(CodecError::InvalidHeader(format!(
            "{name} exceeds 4300 digits"
        )));
    }
    parse_integer(text)
        .ok_or_else(|| CodecError::InvalidHeader(format!("{name} is not a Python int")))
}

fn header_i64(name: &str, text: &str) -> Result<i64, CodecError> {
    header_bigint(name, text)?
        .to_i64()
        .ok_or_else(|| CodecError::InvalidHeader(format!("{name} does not fit i64")))
}

fn header_f64(name: &str, text: &str) -> Result<f64, CodecError> {
    if text.len() > 4 * MAX_INT_DIGITS {
        return Err(CodecError::InvalidHeader(format!("{name} is too long")));
    }
    parse_float(text)
        .ok_or_else(|| CodecError::InvalidHeader(format!("{name} is not a Python float")))
}

/// Visible ASCII, space and tab: safe for every HTTP stack, unlike Latin-1.
fn sendable(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte == b'\t' || (b' '..=b'~').contains(&byte))
}

fn check_sendable(pairs: &[(String, String)]) -> Result<(), CodecError> {
    match pairs.iter().find(|(_, value)| !sendable(value)) {
        Some((name, _)) => Err(CodecError::UnsendableHeader(name.clone())),
        None => Ok(()),
    }
}

/// Whether `name` can be a [`Synapse::NAME`]: an ASCII Python identifier
/// (`[A-Za-z_][A-Za-z0-9_]*`), which is also a single safe route segment.
pub const fn is_valid_synapse_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    if bytes.is_empty() {
        return false;
    }
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if !(byte == b'_' || byte.is_ascii_alphabetic() || (index > 0 && byte.is_ascii_digit())) {
            return false;
        }
        index += 1;
    }
    true
}

fn pairs_to_headers(pairs: &[(String, String)]) -> Result<HeaderMap, CodecError> {
    let mut headers = HeaderMap::with_capacity(pairs.len());
    for (name, value) in pairs {
        let header = HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| CodecError::UnsendableHeader(name.clone()))?;
        let value =
            HeaderValue::from_str(value).map_err(|_| CodecError::UnsendableHeader(name.clone()))?;
        headers.append(header, value);
    }
    Ok(headers)
}

fn py_int_value(value: &Value) -> Result<Option<i64>, String> {
    match value {
        Value::Null => Ok(None),
        Value::Bool(flag) => Ok(Some(i64::from(*flag))),
        Value::Number(number) => {
            if let Some(integer) = number.as_i64() {
                return Ok(Some(integer));
            }
            let text = number.to_string();
            if !text.contains(['.', 'e', 'E']) {
                return Err(format!("integer {text} does not fit i64"));
            }
            let float = number
                .as_f64()
                .ok_or_else(|| format!("invalid number {text}"))?;
            if !float.is_finite() {
                return Err("cannot convert a non-finite float to int".into());
            }
            let truncated = float.trunc();
            if !(-9.223_372_036_854_776e18..9.223_372_036_854_776e18).contains(&truncated) {
                return Err(format!("integer {truncated} does not fit i64"));
            }
            #[allow(clippy::cast_possible_truncation)]
            Ok(Some(truncated as i64))
        }
        Value::String(text) => header_i64("value", text)
            .map(Some)
            .map_err(|error| error.to_string()),
        _ => Err("expected an integer".into()),
    }
}

fn py_float_value(value: &Value) -> Result<Option<f64>, String> {
    match value {
        Value::Null => Ok(None),
        Value::Bool(flag) => Ok(Some(f64::from(u8::from(*flag)))),
        Value::Number(number) => number
            .as_f64()
            .map(Some)
            .ok_or_else(|| format!("invalid number {number}")),
        Value::String(text) => header_f64("value", text)
            .map(Some)
            .map_err(|error| error.to_string()),
        _ => Err("expected a float".into()),
    }
}

fn de_py_int<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<i64>, D::Error> {
    let value = Value::deserialize(deserializer)?;
    py_int_value(&value).map_err(serde::de::Error::custom)
}

fn de_py_float<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<f64>, D::Error> {
    let value = Value::deserialize(deserializer)?;
    py_float_value(&value).map_err(serde::de::Error::custom)
}

/// Parse response headers into the dendrite/axon terminals as Python's
/// `Synapse.from_headers` does for a dendrite merge (last copy wins).
#[cfg(feature = "dendrite")]
pub(crate) fn response_terminals(
    headers: &HeaderMap,
) -> Result<(TerminalInfo, TerminalInfo), CodecError> {
    let mut dendrite = TerminalInfo::default();
    let mut axon = TerminalInfo::default();
    for (name, value) in headers {
        let text = latin1(value);
        if let Some(field) = name.as_str().strip_prefix("bt_header_axon_") {
            axon.set_header_field(field, &text)?;
        } else if let Some(field) = name.as_str().strip_prefix("bt_header_dendrite_") {
            dendrite.set_header_field(field, &text)?;
        }
    }
    Ok((dendrite, axon))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    struct Hashed {
        a: i64,
        b: i64,
        #[serde(default)]
        c: Option<i64>,
        #[serde(default)]
        d: Option<Vec<String>>,
    }
    impl Synapse for Hashed {
        const NAME: &'static str = "HashedSynapse";
        const REQUIRED_HASH_FIELDS: &'static [&'static str] = &["a", "b", "d"];
        const REQUIRED_FIELDS: &'static [&'static str] = &["a", "b"];
    }

    #[test]
    fn upstream_hash_vectors() {
        let h2 = Envelope::new(Hashed {
            a: 1,
            b: 2,
            c: None,
            d: Some(vec!["foobar".into()]),
        });
        assert_eq!(
            h2.body_hash().unwrap(),
            "ae06397d08f30f75c91395c59f05c62ac3b62b88250eb78b109213258e6ced0c"
        );
        let mut h4 = h2.clone();
        h4.body.c = Some(3);
        assert_eq!(h4.body_hash().unwrap(), h2.body_hash().unwrap());
        let mut h5 = h2;
        h5.body.d = None;
        assert_eq!(
            h5.body_hash().unwrap(),
            "74206c06daa8ed9ff696af797404a312e5bedd91964b7d281535790495fac371"
        );
        assert_eq!(
            Envelope::new(BaseSynapse {}).body_hash().unwrap(),
            crate::EMPTY_BODY_HASH
        );
    }

    #[test]
    fn header_order_and_dummies() {
        let mut envelope = Envelope::new(Hashed {
            a: 1,
            b: 2,
            c: None,
            d: None,
        });
        envelope.dendrite = Some(TerminalInfo {
            nonce: Some(5),
            hotkey: Some("x".into()),
            ..TerminalInfo::default()
        });
        let names: Vec<_> = envelope
            .header_pairs()
            .unwrap()
            .into_iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect();
        assert_eq!(names[0], "name=HashedSynapse");
        assert_eq!(names[1], "timeout=12.0");
        assert_eq!(names[2], "bt_header_dendrite_nonce=5");
        assert_eq!(names[3], "bt_header_dendrite_hotkey=x");
        assert_eq!(names[4], "bt_header_input_obj_a=MA==");
        assert_eq!(names[5], "bt_header_input_obj_b=MA==");
        assert!(names[6].starts_with("header_size="));
        assert_eq!(names[7], "total_size=0");
        assert!(names[8].starts_with("computed_body_hash="));
    }

    #[test]
    fn body_coercions_and_defaults() {
        let envelope: Envelope<Hashed> = serde_json::from_str(
            r#"{"name":"Other","a":1,"b":2,"axon":{"port":8091.7,"nonce":"12"},"timeout":3}"#,
        )
        .unwrap();
        assert_eq!(envelope.name.as_deref(), Some("HashedSynapse"));
        assert_eq!(envelope.timeout, Some(3.0));
        assert_eq!(envelope.total_size, Some(0));
        assert_eq!(envelope.computed_body_hash.as_deref(), Some(""));
        let axon = envelope.axon.unwrap();
        assert_eq!((axon.port, axon.nonce), (Some(8091), Some(12)));
        assert_eq!(envelope.dendrite, Some(TerminalInfo::default()));
        let text =
            String::from_utf8(crate::to_python_json(&Envelope::new(BaseSynapse {})).unwrap())
                .unwrap();
        assert!(text.starts_with(r#"{"name": "Synapse", "timeout": 12.0, "total_size": 0, "header_size": 0, "dendrite": {"status_code": null"#));
        assert!(text.ends_with(r#""computed_body_hash": ""}"#));
    }

    #[test]
    fn head_parsing_is_strict() {
        let peer: SocketAddr = "127.0.0.1:5".parse().unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            "bt_header_dendrite_nonce",
            HeaderValue::from_static(" +1_700 "),
        );
        headers.insert("x-bt_header_dendrite_uuid", HeaderValue::from_static("u"));
        headers.insert("timeout", HeaderValue::from_static("nan"));
        let head = parse_head(&headers, "Synapse", peer).unwrap();
        assert_eq!(head.nonce, Some(BigInt::from(1700)));
        assert_eq!(head.dendrite.uuid, None);
        assert!(head.timeout.unwrap().is_nan());
        headers.append("timeout", HeaderValue::from_static("1"));
        assert_eq!(
            parse_head(&headers, "Synapse", peer),
            Err(CodecError::DuplicateHeader("timeout".into()))
        );
        let mut bad = HeaderMap::new();
        bad.insert("bt_header_dendrite_nonce", HeaderValue::from_static("1.0"));
        assert!(parse_head(&bad, "Synapse", peer).is_err());
        let mut long = HeaderMap::new();
        long.insert(
            "bt_header_dendrite_version",
            HeaderValue::from_str(&"9".repeat(4301)).unwrap(),
        );
        assert!(parse_head(&long, "Synapse", peer).is_err());
    }

    #[test]
    fn many_keys_parse_in_linear_time() {
        // Quadratic duplicate detection took about 3 s for 40,000 keys in a
        // debug build; 200,000 keys would take over a minute.
        let mut body = String::from("{");
        for index in 0..200_000 {
            if index > 0 {
                body.push(',');
            }
            body.push_str(&format!("\"k{index}\":0"));
        }
        body.push_str(",\"k7\":1}");
        let started = std::time::Instant::now();
        let envelope = Envelope::<BaseSynapse>::from_json_slice(body.as_bytes()).unwrap();
        assert_eq!(envelope.name.as_deref(), Some("Synapse"));
        let dict: crate::PyDict<String, i64> = serde_json::from_str(&body).unwrap();
        assert_eq!(dict.0.len(), 200_000);
        assert_eq!(dict.0[7], ("k7".into(), 1));
        assert!(
            started.elapsed() < std::time::Duration::from_secs(20),
            "took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn python_non_finite_tokens_are_accepted() {
        let body = br#"{"name": "HashedSynapse", "timeout": NaN, "a": 1, "b": 2, "c": 3,
            "dendrite": {"process_time": Infinity, "hotkey": "x"}, "d": ["NaN"], "a": 5}"#;
        let envelope = Envelope::<Hashed>::from_json_slice(body).unwrap();
        assert!(envelope.timeout.unwrap().is_nan());
        assert_eq!(envelope.body.a, 5);
        assert_eq!(envelope.body.d, Some(vec!["NaN".to_owned()]));
        // serde_json's Value has no non-finite floats; the field reads as None.
        let dendrite = envelope.dendrite.unwrap();
        assert_eq!(
            (dendrite.process_time, dendrite.hotkey.as_deref()),
            (None, Some("x"))
        );

        #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
        struct Floats {
            v: f64,
            vs: Vec<f64>,
        }
        impl Synapse for Floats {
            const NAME: &'static str = "Floats";
            const REQUIRED_HASH_FIELDS: &'static [&'static str] = &["v", "vs"];
        }
        let body = br#"{"v": NaN, "vs": [Infinity, -Infinity, 1.0]}"#;
        let envelope = Envelope::<Floats>::from_json_slice(body).unwrap();
        assert!(envelope.body.v.is_nan());
        assert_eq!(
            envelope.body.vs,
            vec![f64::INFINITY, f64::NEG_INFINITY, 1.0]
        );
        // Python: get_hash(get_hash("nan") + get_hash("[inf, -inf, 1.0]")).
        let expected = get_hash(&(get_hash("nan") + &get_hash("[inf, -inf, 1.0]")));
        assert_eq!(envelope.body_hash().unwrap(), expected);
        assert!(Envelope::<Floats>::from_json_slice(br#"{"v": NaN, "vs": [}"#).is_err());
        assert!(Envelope::<Floats>::from_json_slice(br#"{"v": -NaN, "vs": []}"#).is_err());
        assert!(
            Envelope::<Hashed>::from_json_slice(br#"{"a": 1, "b": 2, "total_size": NaN}"#).is_err()
        );
    }

    #[test]
    fn synapse_names_are_identifiers() {
        for name in ["Synapse", "_x", "A1_b"] {
            assert!(is_valid_synapse_name(name), "{name}");
        }
        for name in ["", "1A", "a/b", "{x}", "*", "a-b", "\u{e9}t\u{e9}", "a b"] {
            assert!(!is_valid_synapse_name(name), "{name}");
        }
    }
}
