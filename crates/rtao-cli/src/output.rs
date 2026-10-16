// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/output.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/query.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/context.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! CLI data and diagnostics, with injectable streams and human renderers.
//!
//! Machine values bypass human rendering. Domain objects supply their existing
//! Python-shaped JSON, and balances in read results use
//! [`PythonBalance`](crate::output::PythonBalance). Human
//! formatting preserves fields and columns, rather than Rich terminal bytes.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fmt::Write as _;
use std::io::{self, IsTerminal, Write};
use std::sync::{Arc, Mutex};

use comfy_table::{Cell, CellAlignment, Table, presets::UTF8_FULL};
use parking_lot::RwLock;
use rtao::primitives::{DynBalance, NetUid, Symbol};
use rtao::tx::{ChainError, ExtrinsicResult, Plan};
use serde::{Serialize, Serializer, ser::SerializeMap};
use serde_json::{Map, Value, json};
use zeroize::{Zeroize, Zeroizing};

use crate::books::AccountKind;

mod pyjson;
pub use pyjson::PyJsonFormatter;

fn terminal_control(character: char) -> bool {
    character.is_control()
        || matches!(character, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{2028}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

struct TerminalText<'a>(&'a str);

impl fmt::Display for TerminalText<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for character in self.0.chars() {
            match character {
                '\n' => formatter.write_str("\\n")?,
                '\r' => formatter.write_str("\\r")?,
                '\t' => formatter.write_str("\\t")?,
                character if terminal_control(character) && u32::from(character) <= 0xff => {
                    write!(formatter, "\\x{:02x}", u32::from(character))?;
                }
                character if terminal_control(character) => {
                    write!(formatter, "\\u{{{:x}}}", u32::from(character))?;
                }
                character => formatter.write_char(character)?,
            }
        }
        Ok(())
    }
}

/// Escape terminal controls and directional formatting in human text only.
/// JSON and transaction inputs must retain their original characters.
pub fn terminal_text(text: &str) -> Cow<'_, str> {
    if text.chars().any(terminal_control) {
        Cow::Owned(TerminalText(text).to_string())
    } else {
        Cow::Borrowed(text)
    }
}

#[cfg(test)]
type SecretWipeObserver = Arc<dyn Fn(&[u8]) + Send + Sync>;

struct SecretBuffer {
    bytes: Zeroizing<Box<[u8]>>,
    #[cfg(test)]
    after_wipe: Option<SecretWipeObserver>,
}

impl Drop for SecretBuffer {
    fn drop(&mut self) {
        self.bytes.zeroize();
        #[cfg(test)]
        if let Some(observer) = &self.after_wipe {
            // Observe the still-owned allocation, before either guard frees it.
            observer(&self.bytes);
        }
    }
}

struct OutputSize(usize);

impl Write for OutputSize {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0 = self
            .0
            .checked_add(bytes.len())
            .ok_or_else(|| io::Error::other("secret output is too large"))?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl fmt::Write for OutputSize {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        self.write_all(text.as_bytes()).map_err(|_| fmt::Error)
    }
}

struct OutputFormat<'a>(io::Cursor<&'a mut [u8]>);

impl fmt::Write for OutputFormat<'_> {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        self.0.write_all(text.as_bytes()).map_err(|_| fmt::Error)
    }
}

enum SecretField<'a> {
    Public(&'a Value),
    Secret(&'a str),
}

impl Serialize for SecretField<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Public(value) => value.serialize(serializer),
            Self::Secret(value) => serializer.serialize_str(value),
        }
    }
}

struct SecretFields<'a>(Vec<(&'a str, SecretField<'a>)>);

impl<'a> SecretFields<'a> {
    fn insert(&mut self, name: &'a str, value: SecretField<'a>) {
        if let Some((_, previous)) = self.0.iter_mut().find(|(key, _)| *key == name) {
            *previous = value;
        } else {
            self.0.push((name, value));
        }
    }
}

impl Serialize for SecretFields<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (name, value) in &self.0 {
            map.serialize_entry(name, value)?;
        }
        map.end()
    }
}

/// Destination of CLI output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stream {
    /// Machine values and human data.
    Stdout,
    /// Errors, prompts and diagnostic chatter.
    Stderr,
}

/// An injectable pair of output streams.
pub trait OutputSink: Send + Sync {
    /// Write complete output bytes, propagating failures such as broken pipes.
    fn write(&self, stream: Stream, bytes: &[u8]) -> io::Result<()>;

    /// Whether a stream accepts interactive terminal rendering.
    fn is_terminal(&self, _stream: Stream) -> bool {
        false
    }
}

/// The process's standard output and standard error.
#[derive(Clone, Copy, Debug, Default)]
pub struct ProcessSink;

impl OutputSink for ProcessSink {
    fn write(&self, stream: Stream, bytes: &[u8]) -> io::Result<()> {
        match stream {
            Stream::Stdout => {
                let mut out = io::stdout().lock();
                out.write_all(bytes)?;
                out.flush()
            }
            Stream::Stderr => {
                let mut err = io::stderr().lock();
                err.write_all(bytes)?;
                err.flush()
            }
        }
    }

    fn is_terminal(&self, stream: Stream) -> bool {
        match stream {
            Stream::Stdout => io::stdout().is_terminal(),
            Stream::Stderr => io::stderr().is_terminal(),
        }
    }
}

/// Shared bounded-by-the-caller output capture for tests and embedding.
#[derive(Clone, Debug, Default)]
pub struct MemorySink(Arc<Mutex<(Vec<u8>, Vec<u8>)>>);

impl MemorySink {
    /// Copy all bytes written to one stream.
    pub fn bytes(&self, stream: Stream) -> io::Result<Vec<u8>> {
        let captured = self
            .0
            .lock()
            .map_err(|_| io::Error::other("output capture poisoned"))?;
        Ok(match stream {
            Stream::Stdout => captured.0.clone(),
            Stream::Stderr => captured.1.clone(),
        })
    }

    /// Copy UTF-8 output written to one stream.
    pub fn text(&self, stream: Stream) -> io::Result<String> {
        String::from_utf8(self.bytes(stream)?)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }
}

impl OutputSink for MemorySink {
    fn write(&self, stream: Stream, bytes: &[u8]) -> io::Result<()> {
        let mut captured = self
            .0
            .lock()
            .map_err(|_| io::Error::other("output capture poisoned"))?;
        match stream {
            Stream::Stdout => captured.0.extend_from_slice(bytes),
            Stream::Stderr => captured.1.extend_from_slice(bytes),
        }
        Ok(())
    }
}

/// Process-independent rendering switches.
#[derive(Clone, Debug, Default)]
pub struct OutputOptions {
    /// Emit values as JSON and errors as JSON diagnostics.
    pub json_mode: bool,
    /// Suppress chatter and optional followups, but never primary data/errors.
    pub quiet: bool,
    /// Connected network, used by command renderers for explorer links.
    pub network: Option<String>,
}

/// Failure to serialize, render or write CLI output.
#[derive(Debug)]
pub enum OutputError {
    /// A stream rejected the write.
    Io(io::Error),
    /// A value could not be serialized.
    Json(serde_json::Error),
    /// The human view's schema did not match its data.
    Render(String),
}

impl fmt::Display for OutputError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => error.fmt(formatter),
            Self::Json(error) => error.fmt(formatter),
            Self::Render(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for OutputError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Json(error) => Some(error),
            Self::Render(_) => None,
        }
    }
}

impl From<io::Error> for OutputError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for OutputError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

/// Read-display balance; its JSON string is Python's `str(Balance)`.
///
/// Intent argument balances deliberately use exact decimal strings instead.
#[derive(Clone, Debug)]
pub struct PythonBalance(pub DynBalance);

impl Serialize for PythonBalance {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0.to_python_string())
    }
}

impl From<DynBalance> for PythonBalance {
    fn from(balance: DynBalance) -> Self {
        Self(balance)
    }
}

/// Python's `json.dumps(default=str)` byte fallback, rather than a byte array.
#[derive(Clone, Copy, Debug)]
pub struct PythonBytes<'a>(pub &'a [u8]);

impl Serialize for PythonBytes<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&python_bytes_repr(self.0))
    }
}

/// A Python object's string fallback (including enums and path objects).
///
/// Pass the corresponding Python string explicitly; Rust enum `Debug` names
/// do not in general match Python's `str(enum)` or a string-valued enum.
#[derive(Clone, Copy, Debug)]
pub struct PythonString<'a>(pub &'a str);

impl Serialize for PythonString<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.0)
    }
}

/// Human table data with an independent, lossless machine payload.
#[derive(Clone, Debug)]
pub struct TableSpec {
    /// Human title.
    pub title: String,
    /// Ordered column names.
    pub columns: Vec<String>,
    /// Ordered rows; cells remain typed until rendering.
    pub rows: Vec<Vec<Value>>,
    /// Complete machine records when the human view omits/reformats fields.
    pub records: Option<Value>,
    /// Amount/count columns to align right in borderless views.
    pub right_align: BTreeSet<usize>,
    /// Human-only descriptions of terms or columns.
    pub legend: Vec<(String, String)>,
    /// Human-only summary.
    pub footer: Option<String>,
}

/// One stage of a human-only pre-sign review card.
#[derive(Clone, Debug)]
pub struct ReviewSection {
    /// Stage label such as Call, Fees, Signer or Transaction.
    pub title: String,
    /// Public facts in the requested order.
    pub rows: Vec<(String, Value)>,
}

impl TableSpec {
    /// Start a table with an explicit ordered schema.
    pub fn new(title: impl Into<String>, columns: Vec<String>, rows: Vec<Vec<Value>>) -> Self {
        Self {
            title: title.into(),
            columns,
            rows,
            records: None,
            right_align: BTreeSet::new(),
            legend: Vec::new(),
            footer: None,
        }
    }

    /// Python's `records if records is not None else dict(zip(columns, row))`.
    ///
    /// Deliberately preserves Python zip truncation and last duplicate-key wins
    /// for machine output; malformed human rows are reported by the renderer.
    pub fn json_payload(&self) -> Value {
        self.records.clone().unwrap_or_else(|| {
            Value::Array(
                self.rows
                    .iter()
                    .map(|row| {
                        Value::Object(
                            self.columns
                                .iter()
                                .cloned()
                                .zip(row.iter().cloned())
                                .collect(),
                        )
                    })
                    .collect(),
            )
        })
    }

    fn validate(&self) -> Result<(), OutputError> {
        if self.rows.iter().any(|row| row.len() != self.columns.len()) {
            return Err(OutputError::Render(
                "table row does not match column count".into(),
            ));
        }
        if self
            .right_align
            .iter()
            .any(|column| *column >= self.columns.len())
        {
            return Err(OutputError::Render(
                "right-aligned table column is out of range".into(),
            ));
        }
        Ok(())
    }
}

/// Result of a registered human renderer; machine output ignores this result.
#[derive(Clone, Debug)]
pub enum HumanView {
    /// One human text fragment; terminal controls are displayed as escapes.
    Text(String),
    /// Renderer-owned line boundaries; each fragment is escaped independently.
    /// Keep untrusted newlines inside a fragment rather than splitting them.
    Lines(Vec<String>),
    /// Human key/value pairs in the requested order.
    Detail {
        /// Optional human title.
        title: Option<String>,
        /// Ordered key/value pairs.
        fields: Vec<(String, Value)>,
    },
    /// A table preserving content and column names.
    Table(TableSpec),
    /// Borderless table with optional right alignment and summary.
    Columns(TableSpec),
    /// Two-level branch/leaf grouping.
    Tree {
        /// Human root label.
        title: String,
        /// Ordered branches and their leaf labels.
        nodes: Vec<(String, Vec<String>)>,
    },
}

type Renderer = dyn Fn(&Value) -> Result<HumanView, OutputError> + Send + Sync;

/// Extensible human views for hand-written commands and generated reads.
#[derive(Clone, Default)]
pub struct RenderRegistry(BTreeMap<String, Arc<Renderer>>);

impl RenderRegistry {
    /// Register a human view without changing the command's machine payload.
    pub fn register<F>(&mut self, name: impl Into<String>, renderer: F)
    where
        F: Fn(&Value) -> Result<HumanView, OutputError> + Send + Sync + 'static,
    {
        self.0.insert(name.into(), Arc::new(renderer));
    }

    /// Whether a named human renderer exists.
    pub fn contains(&self, name: &str) -> bool {
        self.0.contains_key(name)
    }
}

/// One renderer for both humans and machine consumers.
#[derive(Clone)]
pub struct Output {
    /// Active output switches, also inspected by prompts and command context.
    pub options: OutputOptions,
    sink: Arc<dyn OutputSink>,
    registry: RenderRegistry,
    address_names: Arc<RwLock<BTreeMap<String, String>>>,
    address_name_order: Arc<RwLock<Vec<String>>>,
    address_kinds: Arc<RwLock<BTreeMap<String, AccountKind>>>,
    subnet_names: Arc<RwLock<BTreeMap<u16, String>>>,
    token_symbols: Arc<RwLock<BTreeMap<u16, Symbol>>>,
    #[cfg(test)]
    secret_wipe_observer: Option<SecretWipeObserver>,
}

impl Output {
    /// Use the process standard streams.
    pub fn process(options: OutputOptions) -> Self {
        Self::new(options, Arc::new(ProcessSink))
    }

    /// Use controlled streams, without changing global stdio or environment.
    pub fn new(options: OutputOptions, sink: Arc<dyn OutputSink>) -> Self {
        Self {
            options,
            sink,
            registry: RenderRegistry::default(),
            address_names: Arc::new(RwLock::new(BTreeMap::new())),
            address_name_order: Arc::new(RwLock::new(Vec::new())),
            address_kinds: Arc::new(RwLock::new(BTreeMap::new())),
            subnet_names: Arc::new(RwLock::new(BTreeMap::from([(0, "root".into())]))),
            token_symbols: Arc::new(RwLock::new(BTreeMap::new())),
            #[cfg(test)]
            secret_wipe_observer: None,
        }
    }

    /// Alias for [`Self::new`] for embedding callers supplying controlled streams.
    pub fn with_sink(options: OutputOptions, sink: Arc<dyn OutputSink>) -> Self {
        Self::new(options, sink)
    }

    /// Current output switches.
    pub fn options(&self) -> &OutputOptions {
        &self.options
    }

    /// Configure future human command views.
    pub fn registry_mut(&mut self) -> &mut RenderRegistry {
        &mut self.registry
    }

    /// The configured sink, also usable by prompts.
    pub fn sink(&self) -> &Arc<dyn OutputSink> {
        &self.sink
    }

    /// Remember the first local name for an address; never alter machine data.
    pub fn name_address(&self, address: &str, name: &str) -> Result<(), OutputError> {
        if !address.is_empty() && !name.is_empty() && address != name {
            let mut names = self.address_names.write();
            if !names.contains_key(address) {
                names.insert(address.into(), name.into());
                self.address_name_order.write().push(address.into());
            }
        }
        Ok(())
    }

    // py: sdk/python/bittensor/cli/output.py:425-446
    /// Substitute local account and subnet names in human prose only.
    pub(crate) fn prose(&self, text: &str) -> String {
        // Python's dict follows registration order, including when one name
        // contains another known address. Snapshots still expose sorted maps.
        let registered = {
            let names = self.address_names.read();
            self.address_name_order
                .read()
                .iter()
                .filter_map(|address| {
                    names
                        .get(address)
                        .map(|name| (address.clone(), name.clone()))
                })
                .collect::<Vec<_>>()
        };
        let mut text = text.to_owned();
        for (address, name) in registered {
            if text.contains(&address) && !text.contains(&format!("({address})")) {
                text = text.replace(&address, &format!("{name} ({address})"));
            }
        }
        self.with_subnets(&text)
    }

    /// Add cached subnet names to bare `netuid N` and `subnet N` references.
    pub(crate) fn with_subnets(&self, text: &str) -> String {
        use unicode_general_category::{GeneralCategory, get_general_category};
        let word = |character: char| {
            character == '_'
                || matches!(
                    get_general_category(character),
                    GeneralCategory::UppercaseLetter
                        | GeneralCategory::LowercaseLetter
                        | GeneralCategory::TitlecaseLetter
                        | GeneralCategory::ModifierLetter
                        | GeneralCategory::OtherLetter
                        | GeneralCategory::DecimalNumber
                        | GeneralCategory::LetterNumber
                        | GeneralCategory::OtherNumber
                )
        };
        let whitespace =
            |character: char| character.is_whitespace() || matches!(character, '\u{1c}'..='\u{1f}');
        let names = self.subnet_names.read();
        let mut output = String::with_capacity(text.len());
        let mut copied = 0;
        for (start, _) in text.char_indices() {
            if text[..start].chars().next_back().is_some_and(word) {
                continue;
            }
            let keyword = if text[start..].starts_with("netuid") {
                "netuid"
            } else if text[start..].starts_with("subnet") {
                "subnet"
            } else {
                continue;
            };
            let after_keyword = start + keyword.len();
            let rest = &text[after_keyword..];
            let digits_start =
                after_keyword + rest.len() - rest.trim_start_matches(whitespace).len();
            if digits_start == after_keyword {
                continue;
            }
            let rest = &text[digits_start..];
            let digits_len = rest
                .char_indices()
                .take_while(|(_, character)| {
                    get_general_category(*character) == GeneralCategory::DecimalNumber
                })
                .map(|(index, character)| index + character.len_utf8())
                .last()
                .unwrap_or(0);
            let end = digits_start + digits_len;
            if digits_len == 0
                || text[end..].chars().next().is_some_and(word)
                || text[end..]
                    .trim_start_matches(whitespace)
                    .starts_with(['(', ')'])
            {
                continue;
            }
            let digits = &text[digits_start..end];
            let Some(netuid) = rtao::primitives::pyfmt::parse_integer(digits)
                .and_then(|value| value.to_string().parse::<u16>().ok())
            else {
                continue;
            };
            let Some(name) = names.get(&netuid).filter(|name| !name.is_empty()) else {
                continue;
            };
            output.push_str(&text[copied..start]);
            output.push_str(&format!("{keyword} {digits} ({name})"));
            copied = end;
        }
        output.push_str(&text[copied..]);
        output
    }

    /// Remember the first account kind for human account rendering.
    /// Classification is cosmetic and never changes machine values.
    pub fn classify_address(&self, address: &str, kind: AccountKind) -> Result<(), OutputError> {
        if !address.is_empty() {
            self.address_kinds
                .write()
                .entry(address.into())
                .or_insert(kind);
        }
        Ok(())
    }

    /// Snapshot account classifications shared with cloned renderers.
    pub fn address_kinds(&self) -> Result<BTreeMap<String, AccountKind>, OutputError> {
        Ok(self.address_kinds.read().clone())
    }

    /// Install cosmetic subnet names; root always has a fallback name.
    pub fn update_subnet_names(
        &self,
        names: impl IntoIterator<Item = (u16, String)>,
    ) -> Result<(), OutputError> {
        let mut cached = self.subnet_names.write();
        cached.extend(names);
        cached.entry(0).or_insert_with(|| "root".into());
        Ok(())
    }

    /// Install known token symbols for balance display.
    pub fn update_token_symbols(
        &self,
        symbols: impl IntoIterator<Item = (u16, Symbol)>,
    ) -> Result<(), OutputError> {
        self.token_symbols.write().extend(symbols);
        Ok(())
    }

    /// Snapshot the address labels shared with cloned renderers.
    pub fn address_names(&self) -> Result<BTreeMap<String, String>, OutputError> {
        Ok(self.address_names.read().clone())
    }

    /// Snapshot the subnet names shared with cloned renderers.
    pub fn subnet_names(&self) -> Result<BTreeMap<u16, String>, OutputError> {
        Ok(self.subnet_names.read().clone())
    }

    /// Snapshot the token symbols shared with cloned renderers.
    pub fn token_symbols(&self) -> Result<BTreeMap<u16, Symbol>, OutputError> {
        Ok(self.token_symbols.read().clone())
    }

    /// Construct a display balance in the chain's checked native u64 domain.
    pub fn balance(&self, rao: u64, netuid: NetUid) -> PythonBalance {
        let mut balance = DynBalance::from_rao(rao, netuid);
        if netuid != NetUid::ROOT {
            balance.symbol = self.token_symbols.read().get(&netuid.get()).cloned();
        }
        PythonBalance(balance)
    }

    /// Known token unit, with the same root and untagged-alpha fallbacks as Python.
    pub fn unit(&self, netuid: NetUid) -> String {
        self.balance(0, netuid).0.unit().into_owned()
    }

    /// Canonical human subnet identity with its cached name and optional symbol.
    pub fn subnet_text(&self, netuid: NetUid, symbol: bool) -> String {
        let name = self
            .subnet_names
            .read()
            .get(&netuid.get())
            .map(|name| format!(" ({name})"))
            .unwrap_or_default();
        let symbol = if symbol {
            format!(" {}", self.unit(netuid))
        } else {
            String::new()
        };
        format!("{netuid}{name}{symbol}")
    }

    /// Compact account identity with a supplied or remembered local name.
    pub fn account_text(&self, address: &str, name: Option<&str>) -> String {
        let label = if address.chars().count() <= 12 {
            address.into()
        } else {
            format!("{}…", address.chars().take(8).collect::<String>())
        };
        match name
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
            .or_else(|| self.address_names.read().get(address).cloned())
        {
            Some(name) => format!("{label} ({name})"),
            None => label,
        }
    }

    /// Emit already JSON-friendly data; quiet mode does not discard values.
    pub fn value<T: Serialize + ?Sized>(&self, payload: &T) -> Result<(), OutputError> {
        if self.options.json_mode {
            self.json(Stream::Stdout, payload)
        } else {
            let payload = serde_json::to_value(payload)?;
            self.line(Stream::Stdout, &python_value(&payload))
        }
    }

    /// Render a complete machine payload using an optional human-only view.
    pub fn render(&self, name: &str, payload: &Value) -> Result<(), OutputError> {
        if self.options.json_mode {
            return self.json(Stream::Stdout, payload);
        }
        if let Some(renderer) = self.registry.0.get(name) {
            self.human(&renderer(payload)?)
        } else {
            self.query(name, payload)
        }
    }

    /// Generated query rendering: scalar, detail, or records with opaque cells hidden.
    pub fn query(&self, name: &str, payload: &Value) -> Result<(), OutputError> {
        if self.options.json_mode {
            return self.json(Stream::Stdout, payload);
        }
        match payload {
            Value::Null => self.detail(Some(name), &[], None),
            Value::Object(fields) => self.detail(
                Some(name),
                &fields
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect::<Vec<_>>(),
                None,
            ),
            Value::Array(records) if records.is_empty() => self.detail(Some(name), &[], None),
            Value::Array(records) if records.iter().all(Value::is_object) => {
                let first = records[0]
                    .as_object()
                    .ok_or_else(|| OutputError::Render("expected query record".into()))?;
                let mut columns: Vec<_> = first
                    .iter()
                    .filter(|(_, v)| !v.is_array() && !v.is_object())
                    .map(|(k, _)| k.clone())
                    .collect();
                if columns.is_empty() {
                    columns = first.keys().cloned().collect();
                }
                let rows = records
                    .iter()
                    .map(|record| {
                        columns
                            .iter()
                            .map(|key| record.get(key).cloned().unwrap_or(Value::Null))
                            .collect()
                    })
                    .collect();
                let mut spec = TableSpec::new(name, columns, rows);
                spec.records = Some(payload.clone());
                self.table(&spec)
            }
            _ => self.value(payload),
        }
    }

    /// Human detail pairs and an optional richer JSON object.
    pub fn detail(
        &self,
        title: Option<&str>,
        fields: &[(String, Value)],
        json_fields: Option<&Value>,
    ) -> Result<(), OutputError> {
        if self.options.json_mode {
            let payload = json_fields
                .cloned()
                .unwrap_or_else(|| Value::Object(fields.iter().cloned().collect()));
            return self.json(Stream::Stdout, &payload);
        }
        if let Some(title) = title {
            self.line(Stream::Stdout, title)?;
        }
        if fields.is_empty() {
            return self.line(Stream::Stdout, "  none");
        }
        self.fields(fields, 2)
    }

    /// Render public wallet fields with borrowed secret fields in JSON mode.
    /// Secrets are never copied into an owned JSON value or a human detail view.
    pub fn secret_detail(
        &self,
        title: Option<&str>,
        fields: &[(String, Value)],
        secrets: &[(&str, &str)],
    ) -> Result<(), OutputError> {
        if !self.options.json_mode {
            return self.detail(title, fields, None);
        }
        let mut payload = SecretFields(Vec::with_capacity(fields.len() + secrets.len()));
        for (name, value) in fields {
            payload.insert(name, SecretField::Public(value));
        }
        for (name, value) in secrets {
            payload.insert(name, SecretField::Secret(value));
        }
        self.secret_json(&payload)
    }

    fn secret_buffer(&self, length: usize) -> SecretBuffer {
        SecretBuffer {
            // Allocation and any capacity adjustment happen before plaintext
            // is written. A boxed slice cannot grow or release old copies.
            bytes: Zeroizing::new(vec![0; length].into_boxed_slice()),
            #[cfg(test)]
            after_wipe: self.secret_wipe_observer.clone(),
        }
    }

    fn secret_json(&self, payload: &SecretFields<'_>) -> Result<(), OutputError> {
        let mut size = OutputSize(0);
        PyJsonFormatter::to_writer(&mut size, payload)?;
        size.write_all(b"\n")?;
        let mut guarded = self.secret_buffer(size.0);
        let mut writer = io::Cursor::new(guarded.bytes.as_mut());
        PyJsonFormatter::to_writer(&mut writer, payload)?;
        writer.write_all(b"\n")?;
        if writer.position() != size.0 as u64 {
            return Err(OutputError::Render("secret output size changed".into()));
        }
        self.sink.write(Stream::Stdout, &guarded.bytes)?;
        Ok(())
    }

    /// Write one intentional secret line from borrowed fragments. The only
    /// formatted allocation is fixed-size and wiped on success or sink failure.
    pub fn secret_line(&self, fragments: &[&str]) -> Result<(), OutputError> {
        let render = |writer: &mut dyn fmt::Write| -> fmt::Result {
            for fragment in fragments {
                write!(writer, "{}", TerminalText(fragment))?;
            }
            writer.write_char('\n')
        };
        let mut size = OutputSize(0);
        render(&mut size).map_err(|_| OutputError::Render("secret output is too large".into()))?;
        let mut guarded = self.secret_buffer(size.0);
        let mut writer = OutputFormat(io::Cursor::new(guarded.bytes.as_mut()));
        render(&mut writer)
            .map_err(|_| OutputError::Render("secret output size changed".into()))?;
        if writer.0.position() != size.0 as u64 {
            return Err(OutputError::Render("secret output size changed".into()));
        }
        self.sink.write(Stream::Stdout, &guarded.bytes)?;
        Ok(())
    }

    /// Human table or complete machine records.
    pub fn table(&self, spec: &TableSpec) -> Result<(), OutputError> {
        if self.options.json_mode {
            return self.json(Stream::Stdout, &spec.json_payload());
        }
        spec.validate()?;
        self.line(Stream::Stdout, &spec.title)?;
        if spec.rows.is_empty() {
            return self.line(Stream::Stdout, "  none");
        }
        let mut table = Table::new();
        table.load_preset(UTF8_FULL).set_header(
            spec.columns
                .iter()
                .map(|column| terminal_text(column).into_owned()),
        );
        for row in &spec.rows {
            table.add_row(row.iter().enumerate().map(|(index, value)| {
                let cell = Cell::new(terminal_text(&self.cell(&spec.columns[index], value)));
                if spec.right_align.contains(&index) {
                    cell.set_alignment(CellAlignment::Right)
                } else {
                    cell
                }
            }));
        }
        // Only the table renderer supplies structural newlines. Untrusted
        // header and cell text has already been escaped before table parsing.
        for row in table.to_string().lines() {
            self.line(Stream::Stdout, row)?;
        }
        for (term, meaning) in &spec.legend {
            self.line(Stream::Stdout, &format!("  {term}  {meaning}"))?;
        }
        if let Some(footer) = &spec.footer {
            self.line(Stream::Stdout, footer)?;
        }
        Ok(())
    }

    /// Borderless human columns with unchanged machine records.
    pub fn columns(&self, spec: &TableSpec) -> Result<(), OutputError> {
        if self.options.json_mode {
            return self.json(Stream::Stdout, &spec.json_payload());
        }
        spec.validate()?;
        self.line(Stream::Stdout, &spec.title)?;
        if spec.rows.is_empty() {
            return self.line(Stream::Stdout, "  none");
        }
        let columns = spec
            .columns
            .iter()
            .map(|column| terminal_text(column).into_owned())
            .collect::<Vec<_>>();
        let cells: Vec<Vec<_>> = spec
            .rows
            .iter()
            .map(|row| {
                row.iter()
                    .map(|value| terminal_text(&python_value(value)).into_owned())
                    .collect()
            })
            .collect();
        let widths: Vec<_> = columns
            .iter()
            .enumerate()
            .map(|(i, name)| {
                cells
                    .iter()
                    .map(|row| row[i].chars().count())
                    .chain([name.chars().count()])
                    .max()
                    .unwrap_or(0)
            })
            .collect();
        for row in std::iter::once(&columns).chain(cells.iter()) {
            let rendered = row
                .iter()
                .enumerate()
                .map(|(i, cell)| {
                    let padding = " ".repeat(widths[i].saturating_sub(cell.chars().count()));
                    if spec.right_align.contains(&i) {
                        format!("{padding}{cell}")
                    } else {
                        format!("{cell}{padding}")
                    }
                })
                .collect::<Vec<_>>()
                .join("  ");
            self.line(Stream::Stdout, &format!("  {rendered}"))?;
        }
        if let Some(footer) = &spec.footer {
            self.line(Stream::Stdout, footer)?;
        }
        Ok(())
    }

    /// Recent transfers retain raw machine records and a human balance summary.
    ///
    /// Human amounts and successful root-TAO totals use checked native u64 rao.
    /// Malformed records or total overflow refuse before any human output.
    pub fn transfer_history(&self, title: &str, records: &[Value]) -> Result<(), OutputError> {
        if self.options.json_mode {
            return self.json(Stream::Stdout, records);
        }
        let number = |value: &Value, field: &str| {
            value
                .as_u64()
                .or_else(|| value.as_str().and_then(|text| text.parse::<u64>().ok()))
                .ok_or_else(|| {
                    OutputError::Render(format!("transfer history {field} must be a u64"))
                })
        };
        let names = self.address_names.read();
        let mut incoming = 0u64;
        let mut outgoing = 0u64;
        let mut failed = 0usize;
        let mut rows = Vec::with_capacity(records.len());
        for record in records {
            let amount = number(&record["amount_rao"], "amount_rao")?;
            let netuid = if record["netuid"].is_null() {
                NetUid::ROOT
            } else {
                NetUid(
                    u16::try_from(number(&record["netuid"], "netuid")?).map_err(|_| {
                        OutputError::Render("transfer history netuid must be a u16".into())
                    })?,
                )
            };
            let direction = record["direction"]
                .as_str()
                .filter(|direction| matches!(*direction, "in" | "out" | "self"))
                .ok_or_else(|| {
                    OutputError::Render("transfer history direction is invalid".into())
                })?;
            let success = record["success"].as_bool().ok_or_else(|| {
                OutputError::Render("transfer history success must be a boolean".into())
            })?;
            if !success {
                failed += 1;
            } else if netuid == NetUid::ROOT {
                let total = match direction {
                    "in" => Some(&mut incoming),
                    "out" => Some(&mut outgoing),
                    _ => None,
                };
                if let Some(total) = total {
                    *total = total.checked_add(amount).ok_or_else(|| {
                        OutputError::Render(format!(
                            "transfer history {direction} TAO total exceeds u64 rao"
                        ))
                    })?;
                }
            }
            let timestamp = record["timestamp"].as_str().ok_or_else(|| {
                OutputError::Render("transfer history timestamp must be text".into())
            })?;
            // Python formats the written wall time, without converting its offset.
            let when = jiff::fmt::temporal::DateTimeParser::new()
                .parse_datetime(timestamp.replace('Z', "+00:00"))
                .map(|datetime| datetime.strftime("%Y-%m-%d %H:%M").to_string())
                .unwrap_or_else(|_| timestamp.to_owned());
            let counterparty = record[if direction == "out" { "to" } else { "from" }]
                .as_str()
                .ok_or_else(|| {
                    OutputError::Render("transfer history counterparty must be text".into())
                })?;
            let account = match names.get(counterparty) {
                Some(name) => format!("{name} ({counterparty})"),
                None => counterparty.to_owned(),
            };
            let word = match direction {
                "out" => "to",
                "self" => "self",
                _ => "from",
            };
            rows.push(vec![
                json!(when),
                record["block_number"].clone(),
                json!(format!(
                    "{}{}",
                    if direction == "out" { "−" } else { "+" },
                    self.balance(amount, netuid).0.to_python_string()
                )),
                json!(format!(
                    "{word} {account}{}",
                    if success { "" } else { "  ✗ failed" }
                )),
            ]);
        }
        drop(names);
        let mut summary = vec![format!(
            "{} {}",
            records.len(),
            if records.len() == 1 {
                "transfer"
            } else {
                "transfers"
            }
        )];
        for (word, total) in [("in", incoming), ("out", outgoing)] {
            if total != 0 {
                summary.push(format!(
                    "{word} {}",
                    self.balance(total, NetUid::ROOT).0.to_python_string()
                ));
            }
        }
        if failed != 0 {
            summary.push(format!("{failed} failed excluded"));
        }
        let mut table = TableSpec::new(
            title,
            ["when", "block", "amount", "counterparty"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            rows,
        );
        table.right_align.extend([1, 2]);
        table.footer = Some(summary.join("  ·  "));
        self.columns(&table)
    }

    /// Two-level grouping; machine shape comes exclusively from records.
    pub fn tree(
        &self,
        title: &str,
        nodes: &[(String, Vec<String>)],
        records: Option<&Value>,
    ) -> Result<(), OutputError> {
        if self.options.json_mode {
            return self.json(Stream::Stdout, records.unwrap_or(&json!([])));
        }
        self.line(Stream::Stdout, title)?;
        for (branch, leaves) in nodes {
            self.line(Stream::Stdout, &format!("  {branch}"))?;
            for leaf in leaves {
                self.line(Stream::Stdout, &format!("    {leaf}"))?;
            }
        }
        Ok(())
    }

    /// Informational chatter is always stderr, suppressed by JSON and quiet.
    pub fn message(&self, text: &str) -> Result<(), OutputError> {
        if self.options.quiet || self.options.json_mode {
            Ok(())
        } else {
            self.line(Stream::Stderr, text)
        }
    }

    /// Human pre-sign review, suppressed in JSON and quiet sessions.
    pub fn transaction_card(&self, sections: &[ReviewSection]) -> Result<(), OutputError> {
        if self.options.json_mode || self.options.quiet {
            return Ok(());
        }
        let width = sections
            .iter()
            .filter(|section| !section.rows.is_empty())
            .flat_map(|section| &section.rows)
            .map(|(key, _)| terminal_text(key).chars().count())
            .max()
            .unwrap_or(0);
        for section in sections.iter().filter(|section| !section.rows.is_empty()) {
            self.line(Stream::Stdout, &section.title)?;
            for (key, value) in &section.rows {
                let key = terminal_text(key);
                let padding = " ".repeat(width.saturating_sub(key.chars().count()));
                self.line(
                    Stream::Stdout,
                    &format!("  {key}{padding}  {}", self.review_value(&key, value)),
                )?;
            }
        }
        Ok(())
    }

    // py: sdk/python/bittensor/cli/context.py:716-734
    // Values on a review card are human text; machine payloads remain untouched.
    pub(crate) fn review_value(&self, key: &str, value: &Value) -> String {
        let text = match value {
            Value::String(text) if crate::prompt::python_strip(text).is_empty() => "—".into(),
            Value::Bool(value) => if *value { "yes" } else { "no" }.into(),
            Value::Number(number) if key.contains("netuid") => number
                .as_u64()
                .and_then(|number| u16::try_from(number).ok())
                .map_or_else(
                    || number.to_string(),
                    |number| self.subnet_text(NetUid(number), false),
                ),
            Value::Number(number) if key.ends_with("tolerance") && number.is_f64() => {
                number.as_f64().map_or_else(
                    || number.to_string(),
                    |number| format!("{:.2}%", number * 100.0),
                )
            }
            Value::Array(_) | Value::Object(_) => {
                let text = review_json(value);
                if text.chars().count() > 160 {
                    format!("{}…", text.chars().take(159).collect::<String>())
                } else {
                    text
                }
            }
            _ => python_value(value),
        };
        self.prose(&text)
    }

    /// Render a progress step without contaminating JSON data.
    pub fn step(&self, label: &str, value: &str, state: StepState) -> Result<(), OutputError> {
        let glyph = match state {
            StepState::Done => "✓",
            StepState::Running => "→",
            StepState::Fact => "·",
            StepState::Failed => "✗",
        };
        self.message(&format!(
            "{glyph} {label}{}",
            if value.is_empty() {
                String::new()
            } else {
                format!("  {}", self.prose(value))
            }
        ))
    }

    /// Rust-style diagnostic; JSON retains exact input strings and optional fields.
    pub fn error(&self, text: &str, context: DiagnosticContext<'_>) -> Result<(), OutputError> {
        if self.options.json_mode {
            let mut payload = Map::from_iter([("error".into(), json!(text))]);
            for (key, value) in [
                ("note", context.note),
                ("help", context.help),
                ("see", context.see),
            ] {
                if let Some(value) = value.filter(|text| !text.is_empty()) {
                    payload.insert(key.into(), json!(value));
                }
            }
            return self.json(Stream::Stderr, &Value::Object(payload));
        }
        self.line(
            Stream::Stderr,
            &format!("error: {}", diagnostic(&self.with_subnets(text))),
        )?;
        for (label, value) in [
            ("note", context.note),
            ("help", context.help),
            ("see", context.see),
        ] {
            if let Some(value) = value.filter(|text| !text.is_empty()) {
                self.sub_diag(Stream::Stderr, label, value)?;
            }
        }
        Ok(())
    }

    /// Dry-run JSON supplied by the SDK; replay command remains in machine output.
    pub fn plan(&self, plan: &Plan, command: Option<&str>) -> Result<(), OutputError> {
        self.render_plan(
            &plan.to_json(),
            command,
            plan.fee.map(|fee| DynBalance::from(fee).to_python_string()),
        )
    }

    /// Render an existing Python-shaped plan, preserving every machine field.
    pub fn plan_payload(&self, plan: &Value, command: Option<&str>) -> Result<(), OutputError> {
        self.render_plan(plan, command, None)
    }

    fn render_plan(
        &self,
        plan: &Value,
        command: Option<&str>,
        fee_display: Option<String>,
    ) -> Result<(), OutputError> {
        let mut record = plan
            .as_object()
            .cloned()
            .ok_or_else(|| OutputError::Render("plan must be an object".into()))?;
        if let Some(command) = command.filter(|command| !command.is_empty()) {
            record.insert("command".into(), json!(command));
        }
        if self.options.json_mode {
            return self.json(Stream::Stdout, &Value::Object(record));
        }
        self.line(
            Stream::Stdout,
            &format!("dry run: {}", self.prose(text_field(plan, "summary"))),
        )?;
        let mut rows = Vec::new();
        let signer = text_field(plan, "signer");
        let address = text_field(plan, "signer_address");
        let signer_name = self
            .address_names
            .read()
            .get(address)
            .cloned()
            .unwrap_or_else(|| signer.into());
        rows.push((
            "signer".into(),
            json!(if address.is_empty() {
                signer_name
            } else {
                format!("{signer_name} ({address})")
            }),
        ));
        if let Some(fee) = plan.get("fee_tao").filter(|v| !v.is_null()) {
            rows.push((
                "est. fee".into(),
                fee_display
                    .map(Value::String)
                    .unwrap_or_else(|| fee.clone()),
            ));
        }
        for (field, label) in [
            ("effects", "effect"),
            ("warnings", "warning"),
            ("violations", "policy"),
        ] {
            if let Some(items) = plan.get(field).and_then(Value::as_array) {
                rows.extend(items.iter().map(|item| {
                    (
                        label.into(),
                        item.as_str()
                            .map(|text| Value::String(self.prose(text)))
                            .unwrap_or_else(|| item.clone()),
                    )
                }));
            }
        }
        self.fields(&rows, 2)?;
        if plan.get("ok").and_then(Value::as_bool) == Some(false) {
            self.line(Stream::Stdout, "  blocked by policy")?;
        }
        if plan.get("ok").and_then(Value::as_bool) == Some(true)
            && let Some(command) = command.filter(|command| !command.is_empty())
        {
            self.line(Stream::Stdout, &format!("  run for real  {command}"))?;
        }
        if let Some(docs) = plan
            .get("docs_url")
            .and_then(Value::as_str)
            .filter(|url| !url.is_empty())
        {
            self.sub_diag(Stream::Stdout, "see", docs)?;
        }
        Ok(())
    }

    /// Canonical receipt JSON on stdout, even on failure; human errors on stderr.
    pub fn result<O>(
        &self,
        result: &ExtrinsicResult<O>,
        success_message: &str,
    ) -> Result<bool, OutputError> {
        if self.options.json_mode {
            self.json(Stream::Stdout, &result.to_json())?;
            return Ok(result.success);
        }
        if !result.success {
            if let Some(error) = &result.error {
                self.chain_error(error, result.explorer_url.as_deref())?;
            } else {
                self.error(
                    &result.message,
                    DiagnosticContext {
                        see: result.explorer_url.as_deref(),
                        ..DiagnosticContext::default()
                    },
                )?;
            }
            return Ok(false);
        }
        self.line(Stream::Stdout, &format!("✓ {success_message}"))?;
        let mut fields = Vec::new();
        if let Some(fee) = result.fee {
            fields.push(("fee".into(), json!(PythonBalance(fee.into()))));
        }
        if let Some(block) = result.block_hash {
            fields.push(("block".into(), json!(block)));
        }
        if let Some(id) = result.extrinsic_id.as_ref().filter(|id| !id.is_empty()) {
            fields.push(("extrinsic".into(), json!(id)));
        }
        if let Some(url) = result.explorer_url.as_ref().filter(|url| !url.is_empty()) {
            fields.push(("explorer".into(), json!(url)));
        }
        for (key, value) in result
            .data
            .iter()
            .filter(|(key, _)| key.as_str() != "multisig_followup")
        {
            let label = key.replace('_', " ");
            if let Some((_, existing)) =
                fields.iter_mut().find(|(candidate, _)| *candidate == label)
            {
                *existing = value.clone();
            } else {
                fields.push((label, value.clone()));
            }
        }
        if !fields.is_empty() {
            self.detail(Some("Result"), &fields, None)?;
        }
        if let Some(followup) = result.data.get("multisig_followup") {
            self.multisig_followup(followup)?;
        }
        Ok(true)
    }

    /// Successful subnet registration uses human units and its two-phase state.
    /// Machine output and unsuccessful/incomplete receipts keep the canonical shape.
    pub fn registration_result<O>(&self, result: &ExtrinsicResult<O>) -> Result<bool, OutputError> {
        if self.options.json_mode || !result.success || !result.data.contains_key("netuid") {
            return self.result(result, "register a new subnet");
        }
        let netuid = result.data["netuid"]
            .as_u64()
            .and_then(|value| u16::try_from(value).ok())
            .ok_or_else(|| {
                OutputError::Render("registration result has a non-native netuid".into())
            })?;
        let registration = if result.data.get("registration_mode").and_then(Value::as_str)
            == Some("after_deregistration")
        {
            let prior = result
                .data
                .get("deregistered_netuid")
                .filter(|value| crate::books::public_json_truthy(value))
                .or_else(|| result.data.get("cleanup_netuid"))
                .filter(|value| !value.is_null());
            let mut text = "queued · registered after deregistration".to_owned();
            if let Some(prior) = prior {
                text.push_str(&format!(" of subnet {}", python_value(prior)));
            }
            text
        } else {
            "immediate · no deregistration needed".into()
        };
        let mut fields = vec![
            ("netuid".into(), json!(netuid)),
            ("registration".into(), json!(registration)),
        ];
        if let Some(price) = result
            .data
            .get("registration_price_rao")
            .filter(|value| !value.is_null())
        {
            let price = price.as_u64().ok_or_else(|| {
                OutputError::Render("registration price exceeds native u64 rao".into())
            })?;
            fields.push(("price".into(), json!(self.balance(price, NetUid::ROOT))));
        }
        for (key, label) in [
            ("queued_at_block", "queued block"),
            ("registered_at_block", "registered block"),
        ] {
            if let Some(value) = result.data.get(key).filter(|value| !value.is_null()) {
                fields.push((label.into(), value.clone()));
            }
        }
        if let Some(fee) = result.fee {
            fields.push(("fee".into(), json!(PythonBalance(fee.into()))));
        }
        if let Some(id) = result.extrinsic_id.as_ref().filter(|id| !id.is_empty()) {
            fields.push(("registration tx".into(), json!(id)));
        }
        if let Some(url) = result.explorer_url.as_ref().filter(|url| !url.is_empty()) {
            fields.push(("explorer".into(), json!(url)));
        }
        self.line(Stream::Stdout, &format!("✓ subnet {netuid} registered"))?;
        self.detail(Some("Result"), &fields, None)?;
        Ok(true)
    }

    /// A classified chain diagnostic uses the domain's existing JSON contract.
    pub fn chain_error(
        &self,
        error: &ChainError,
        explorer_url: Option<&str>,
    ) -> Result<(), OutputError> {
        if self.options.json_mode {
            let mut payload = error.to_json();
            if let Some(url) = explorer_url.filter(|url| !url.is_empty()) {
                payload["explorer_url"] = json!(url);
            }
            return self.json(Stream::Stderr, &payload);
        }
        self.line(
            Stream::Stderr,
            &format!(
                "error[{}]: {}",
                error.code.as_str(),
                diagnostic(&error.message)
            ),
        )?;
        if let Some(name) = error.name.as_deref().filter(|name| !name.is_empty()) {
            self.sub_diag(
                Stream::Stderr,
                "note",
                &format!("the chain rejected the call with `{name}`"),
            )?;
        }
        if let Some(description) = error
            .description()
            .filter(|description| *description != error.message)
        {
            self.sub_diag(Stream::Stderr, "note", description)?;
        }
        self.sub_diag(Stream::Stderr, "help", error.remediation())?;
        if let Some(url) = error.docs_url() {
            self.sub_diag(Stream::Stderr, "see", &url)?;
        }
        if let Some(url) = explorer_url.filter(|url| !url.is_empty()) {
            self.sub_diag(Stream::Stderr, "see", url)?;
        }
        let explain_target = error
            .name
            .as_deref()
            .filter(|name| rtao_catalog::errors::chain_error_code(name).is_some())
            .or_else(|| {
                (error.code != rtao_catalog::ErrorCode::Unknown).then_some(error.code.as_str())
            });
        if let Some(target) = explain_target {
            self.line(
                Stream::Stderr,
                &format!("for more information about this error, run `rtao explain {target}`"),
            )?;
        }
        Ok(())
    }

    /// Multisig followup is optional chatter and therefore suppressed by quiet.
    pub fn multisig_followup(&self, followup: &Value) -> Result<(), OutputError> {
        if self.options.quiet {
            return Ok(());
        }
        if self.options.json_mode {
            self.json(Stream::Stdout, &json!({"multisig_followup":followup}))
        } else {
            self.render("multisig_followup", followup)?;
            self.co_signer_guidance(followup)
        }
    }

    fn co_signer_guidance(&self, followup: &Value) -> Result<(), OutputError> {
        if matches!(
            followup.get("status").and_then(Value::as_str),
            Some("executed" | "submitted")
        ) || followup
            .get("co_signer_commands")
            .and_then(Value::as_array)
            .is_none_or(Vec::is_empty)
        {
            return Ok(());
        }
        self.sub_diag(
            Stream::Stdout,
            "help",
            "add --macos-password or --keychain-password before call if the co-signer needs them; for a Vault/Ledger/extension member, put --signer vault (or ledger/extension) before call; --signatory already names the account",
        )
    }

    /// Pending multisig list shape and quiet behavior from the shared renderer.
    pub fn pending_multisigs(&self, title: &str, records: &[Value]) -> Result<(), OutputError> {
        if self.options.quiet {
            return Ok(());
        }
        if self.options.json_mode {
            return self.json(
                Stream::Stdout,
                &json!({"pending_multisigs":records,"count":records.len()}),
            );
        }
        if let Some(renderer) = self.registry.0.get(title) {
            return self.human(&renderer(&json!(records))?);
        }
        if records.is_empty() {
            return self.line(Stream::Stdout, "No pending multisig operations.");
        }
        // Pending records carry structured members and full replay commands.
        // Generic query tables intentionally hide such cells, so prepare a
        // complete detail view for each operation instead.
        let mut fields = Vec::with_capacity(records.len());
        let mut decode = Vec::new();
        let mut seen = BTreeSet::new();
        for record in records {
            let record = record.as_object().ok_or_else(|| {
                OutputError::Render("pending multisig record must be an object".into())
            })?;
            fields.push(
                record
                    .iter()
                    .filter(|(key, _)| !matches!(key.as_str(), "decode_note" | "decode_hint"))
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect::<Vec<_>>(),
            );
            if record
                .get("co_signer_commands")
                .and_then(Value::as_array)
                .is_none_or(Vec::is_empty)
            {
                let text = |key: &str| {
                    record
                        .get(key)
                        .and_then(Value::as_str)
                        .filter(|text| !text.is_empty())
                        .map(str::to_owned)
                };
                let pair = (text("decode_note"), text("decode_hint"));
                if pair != (None, None) && seen.insert(pair.clone()) {
                    decode.push(pair);
                }
            }
        }
        self.line(
            Stream::Stdout,
            &format!(
                "{title}  {} {}",
                records.len(),
                if records.len() == 1 { "op" } else { "ops" }
            ),
        )?;
        for (index, fields) in fields.iter().enumerate() {
            if index != 0 {
                self.line(Stream::Stdout, &"─".repeat(60))?;
            }
            self.detail(None, fields, None)?;
            self.co_signer_guidance(&records[index])?;
        }
        for (note, hint) in decode {
            if let Some(note) = note {
                self.sub_diag(Stream::Stdout, "note", &note)?;
            }
            if let Some(hint) = hint {
                self.sub_diag(Stream::Stdout, "help", &hint)?;
            }
        }
        Ok(())
    }

    /// Mnemonic data survives quiet mode and is carried by the command in JSON.
    pub fn mnemonic(&self, role: &str, phrase: &str) -> Result<(), OutputError> {
        if self.options.json_mode {
            return Ok(());
        }
        self.line(Stream::Stdout, &format!("generating new {role}"))?;
        self.secret_line(&["  mnemonic  ", phrase])?;
        self.sub_diag(Stream::Stdout,"note",&format!("write this mnemonic down and store it somewhere safe — it is the only way to recover the {role}"))
    }

    /// Clear-scrollback guidance accompanies mnemonic data even under quiet.
    pub fn mnemonic_clear_hint(&self) -> Result<(), OutputError> {
        if self.options.json_mode {
            return Ok(());
        }
        self.sub_diag(
            Stream::Stdout,
            "help",
            "once stored, wipe the mnemonic from this terminal: run `clear && printf '\\e[3J'` (or clear terminal scrollback)",
        )
    }

    fn human(&self, view: &HumanView) -> Result<(), OutputError> {
        match view {
            HumanView::Text(text) => self.line(Stream::Stdout, text),
            HumanView::Lines(lines) => {
                for line in lines {
                    self.line(Stream::Stdout, line)?;
                }
                Ok(())
            }
            HumanView::Detail { title, fields } => self.detail(title.as_deref(), fields, None),
            HumanView::Table(spec) => self.table(spec),
            HumanView::Columns(spec) => self.columns(spec),
            HumanView::Tree { title, nodes } => self.tree(title, nodes, None),
        }
    }

    fn fields(&self, fields: &[(String, Value)], indent: usize) -> Result<(), OutputError> {
        let width = fields
            .iter()
            .map(|(key, _)| terminal_text(key).chars().count())
            .max()
            .unwrap_or(0);
        for (key, value) in fields {
            let display_key = terminal_text(key);
            let prefix = format!(
                "{}{}{display_key}",
                " ".repeat(indent),
                " ".repeat(width.saturating_sub(display_key.chars().count()))
            );
            match value {
                Value::Object(values) if !values.is_empty() => {
                    self.line(Stream::Stdout, &prefix)?;
                    self.fields(
                        &values
                            .iter()
                            .map(|(k, v)| (k.clone(), v.clone()))
                            .collect::<Vec<_>>(),
                        indent.saturating_add(2),
                    )?;
                }
                Value::Array(values)
                    if !values.is_empty() && values.iter().all(Value::is_object) =>
                {
                    self.line(Stream::Stdout, &prefix)?;
                    for record in values {
                        let fields = record
                            .as_object()
                            .ok_or_else(|| OutputError::Render("expected nested record".into()))?;
                        let rendered = fields
                            .iter()
                            .map(|(key, value)| format!("{key} {}", python_value(value)))
                            .collect::<Vec<_>>()
                            .join("  ");
                        self.line(
                            Stream::Stdout,
                            &format!("{}{rendered}", " ".repeat(indent.saturating_add(2))),
                        )?;
                    }
                }
                Value::Object(values) if values.is_empty() => {
                    self.line(Stream::Stdout, &format!("{prefix}  none"))?
                }
                Value::Array(values) => self.line(
                    Stream::Stdout,
                    &format!(
                        "{prefix}  {}",
                        if values.is_empty() {
                            "none".into()
                        } else {
                            values
                                .iter()
                                .map(python_value)
                                .collect::<Vec<_>>()
                                .join(", ")
                        }
                    ),
                )?,
                _ => self.line(
                    Stream::Stdout,
                    &format!("{prefix}  {}", self.cell(key, value)),
                )?,
            }
        }
        Ok(())
    }

    fn cell(&self, key: &str, value: &Value) -> String {
        let text = python_value(value);
        if key.ends_with("netuid")
            && !text.is_empty()
            && text.chars().all(|character| character.is_ascii_digit())
            && let Ok(netuid) = text.parse::<u16>()
        {
            return self.subnet_text(NetUid(netuid), true);
        }
        text
    }

    fn sub_diag(&self, stream: Stream, label: &str, text: &str) -> Result<(), OutputError> {
        self.line(
            stream,
            &format!("{:>7} {}", format!("{label}:"), diagnostic(text)),
        )
    }

    fn json<T: Serialize + ?Sized>(&self, stream: Stream, payload: &T) -> Result<(), OutputError> {
        let mut bytes = Vec::new();
        PyJsonFormatter::to_writer(&mut bytes, payload)?;
        bytes.push(b'\n');
        self.sink.write(stream, &bytes)?;
        Ok(())
    }

    fn line(&self, stream: Stream, text: &str) -> Result<(), OutputError> {
        self.sink
            .write(stream, format!("{}\n", TerminalText(text)).as_bytes())?;
        Ok(())
    }
}

/// Optional context attached to a user-facing error.
#[derive(Clone, Copy, Debug, Default)]
pub struct DiagnosticContext<'a> {
    /// Additional facts.
    pub note: Option<&'a str>,
    /// How to resolve the problem.
    pub help: Option<&'a str>,
    /// Documentation or explorer URL.
    pub see: Option<&'a str>,
}

/// State of a human progress step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepState {
    /// Completed successfully.
    Done,
    /// Currently running.
    Running,
    /// Incidental fact.
    Fact,
    /// Failed.
    Failed,
}

fn text_field<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or("")
}

/// Match Python's diagnostic capitalization without modifying machine strings.
pub fn diagnostic(text: &str) -> String {
    let text = text.trim();
    let text = if text.ends_with('.') && !text.ends_with("..") {
        &text[..text.len().saturating_sub(1)]
    } else {
        text
    };
    let mut characters = text.chars();
    match (characters.next(), characters.next()) {
        (Some(first), Some(second)) if first.is_uppercase() && second.is_lowercase() => first
            .to_lowercase()
            .chain(std::iter::once(second))
            .chain(characters)
            .collect(),
        _ => text.into(),
    }
}

// Python json.dumps defaults used by the context card: spaces and ASCII escapes.
fn review_json(value: &Value) -> String {
    let mut text = String::new();
    let mut quoted = false;
    let mut escaped = false;
    for character in value.to_string().chars() {
        if character == '"' && !escaped {
            quoted = !quoted;
        }
        if character.is_ascii() && character != '\u{7f}' {
            text.push(character);
        } else {
            let mut units = [0; 2];
            for unit in character.encode_utf16(&mut units) {
                text.push_str(&format!("\\u{:04x}", *unit));
            }
        }
        if !quoted && matches!(character, ',' | ':') {
            text.push(' ');
        }
        escaped = quoted && character == '\\' && !escaped;
    }
    text
}

fn python_value(value: &Value) -> String {
    match value {
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        Value::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(python_repr)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Object(values) => format!(
            "{{{}}}",
            values
                .iter()
                .map(|(key, value)| format!("{}: {}", python_repr(&json!(key)), python_repr(value)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn python_repr(value: &Value) -> String {
    if let Value::String(text) = value {
        // Human nested repr only; machine strings are never transformed.
        format!(
            "'{}'",
            text.replace('\\', "\\\\")
                .replace('\'', "\\'")
                .replace('\n', "\\n")
                .replace('\r', "\\r")
                .replace('\t', "\\t")
        )
    } else {
        python_value(value)
    }
}

fn python_bytes_repr(bytes: &[u8]) -> String {
    let quote = if bytes.contains(&b'\'') && !bytes.contains(&b'"') {
        '"'
    } else {
        '\''
    };
    let mut out = format!("b{quote}");
    for byte in bytes {
        match *byte {
            b'\\' => out.push_str("\\\\"),
            b'\t' => out.push_str("\\t"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            byte if char::from(byte) == quote => {
                out.push('\\');
                out.push(quote);
            }
            0x20..=0x7e => out.push(char::from(*byte)),
            byte => out.push_str(&format!("\\x{byte:02x}")),
        }
    }
    out.push(quote);
    out
}

#[cfg(test)]
mod secret_output_tests {
    use super::*;
    use secrecy::{ExposeSecret, SecretString};
    use std::sync::atomic::{AtomicUsize, Ordering};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    struct InspectSink {
        fail: bool,
        writes: AtomicUsize,
        documents: Mutex<Vec<Vec<u8>>>,
    }

    impl OutputSink for InspectSink {
        fn write(&self, stream: Stream, bytes: &[u8]) -> io::Result<()> {
            assert_eq!(stream, Stream::Stdout);
            self.writes.fetch_add(1, Ordering::SeqCst);
            self.documents
                .lock()
                .map_err(|_| io::Error::other("fixture sink poisoned"))?
                .push(bytes.to_vec());
            if self.fail {
                Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "fixture sink closed",
                ))
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn exact_secret_buffers_are_wiped_before_free_on_success_and_sink_error() -> TestResult {
        // Escaping expands this input; the counted allocation must still hold
        // the complete output without growing or creating ordinary JSON text.
        let secret = SecretString::new("disposable \"\\\n\u{1b}\u{202e} π sentinel".into());
        for json_mode in [false, true] {
            for fail in [false, true] {
                let sink = Arc::new(InspectSink {
                    fail,
                    writes: AtomicUsize::new(0),
                    documents: Mutex::new(Vec::new()),
                });
                let wiped = Arc::new(Mutex::new(Vec::new()));
                let mut output = Output::new(
                    OutputOptions {
                        json_mode,
                        quiet: true,
                        network: None,
                    },
                    sink.clone(),
                );
                let observed = wiped.clone();
                output.secret_wipe_observer = Some(Arc::new(move |bytes| {
                    // Drop calls this before freeing the boxed slice. No test
                    // reads allocator storage after its ownership ends.
                    assert!(!bytes.is_empty());
                    assert!(bytes.iter().all(|byte| *byte == 0));
                    if let Ok(mut lengths) = observed.lock() {
                        lengths.push(bytes.len());
                    }
                }));
                let result = if json_mode {
                    output.secret_detail(
                        Some("unused JSON title"),
                        &[("wallet".into(), json!("fixture"))],
                        &[("mnemonic", secret.expose_secret())],
                    )
                } else {
                    output.secret_line(&["mnemonic: ", secret.expose_secret()])
                };
                assert_eq!(result.is_err(), fail);
                if let Err(error) = result {
                    assert!(!error.to_string().contains("sentinel"));
                }
                assert_eq!(sink.writes.load(Ordering::SeqCst), 1);
                let documents = sink.documents.lock().map_err(|_| "fixture sink")?;
                assert_eq!(documents.len(), 1);
                let document = &documents[0];
                assert_eq!(
                    *wiped.lock().map_err(|_| "wipe observer")?,
                    vec![document.len()]
                );
                if json_mode {
                    let expected = json!({"wallet":"fixture","mnemonic":secret.expose_secret()});
                    assert_eq!(serde_json::from_slice::<Value>(document)?, expected);
                    let mut expected_bytes = serde_json::to_vec_pretty(&expected)?;
                    expected_bytes.push(b'\n');
                    assert_eq!(*document, expected_bytes);
                } else {
                    assert_eq!(
                        std::str::from_utf8(document)?,
                        format!("mnemonic: {}\n", terminal_text(secret.expose_secret()))
                    );
                }
            }
        }
        Ok(())
    }

    #[test]
    fn human_mnemonic_uses_one_guarded_buffer_and_json_helper_allocates_none() -> TestResult {
        for json_mode in [false, true] {
            let sink = Arc::new(InspectSink {
                fail: false,
                writes: AtomicUsize::new(0),
                documents: Mutex::new(Vec::new()),
            });
            let wipes = Arc::new(AtomicUsize::new(0));
            let observed = wipes.clone();
            let mut output = Output::new(
                OutputOptions {
                    json_mode,
                    quiet: true,
                    network: None,
                },
                sink.clone(),
            );
            output.secret_wipe_observer = Some(Arc::new(move |bytes| {
                assert!(bytes.iter().all(|byte| *byte == 0));
                observed.fetch_add(1, Ordering::SeqCst);
            }));
            output.mnemonic("coldkey", "disposable recovery sentinel")?;
            assert_eq!(wipes.load(Ordering::SeqCst), usize::from(!json_mode));
            if json_mode {
                assert_eq!(sink.writes.load(Ordering::SeqCst), 0);
            } else {
                let documents = sink.documents.lock().map_err(|_| "fixture sink")?;
                assert_eq!(
                    documents
                        .iter()
                        .filter(|bytes| bytes
                            .windows(b"disposable".len())
                            .any(|part| part == b"disposable"))
                        .count(),
                    1
                );
            }
        }
        Ok(())
    }

    #[test]
    fn human_secret_detail_only_formats_public_fields() -> TestResult {
        let sink = MemorySink::default();
        let mut output = Output::new(OutputOptions::default(), Arc::new(sink.clone()));
        let wipes = Arc::new(AtomicUsize::new(0));
        let observed = wipes.clone();
        output.secret_wipe_observer = Some(Arc::new(move |_| {
            observed.fetch_add(1, Ordering::SeqCst);
        }));
        output.secret_detail(
            Some("created coldkey"),
            &[("wallet".into(), json!("fixture"))],
            &[("mnemonic", "disposable private sentinel")],
        )?;
        let rendered = sink.text(Stream::Stdout)?;
        assert!(rendered.contains("created coldkey"));
        assert!(rendered.contains("fixture"));
        assert!(!rendered.contains("mnemonic"));
        assert!(!rendered.contains("sentinel"));
        assert_eq!(wipes.load(Ordering::SeqCst), 0);
        Ok(())
    }
}
