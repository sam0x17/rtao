// Derived from RaoFoundation/subtensor sdk/python/bittensor/config.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Shared btcli file paths, persistent options and per-network display caches.
//!
//! All store methods run filesystem work on the blocking pool. Missing and
//! unreadable files behave as empty data, as in Python. Invalid JSON is retained
//! in a `.corrupt.<timestamp>` backup. Writes replace complete, fsync'd files;
//! concurrent processes retain Python's last-writer-wins behavior.

use std::{
    collections::BTreeMap,
    ffi::OsString,
    fmt,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use num_bigint::BigInt;
use num_traits::ToPrimitive;
use rtao_primitives::NetUid;
use serde_json::{Map, Number, Value};
use unicode_general_category::{GeneralCategory, get_general_category};

/// Stored CLI options, preserving the exact JSON values loaded by Python.
pub type ConfigData = Map<String, Value>;

/// The seven files shared with btcli.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfigFile {
    /// CLI options (`btcli.json`).
    Config,
    /// Named ss58 contacts.
    Addresses,
    /// Named proxy accounts.
    Proxies,
    /// Saved multisig accounts.
    Multisigs,
    /// Multisig calls indexed by call hash.
    MultisigCache,
    /// Per-network subnet names.
    SubnetNames,
    /// Per-network token symbols.
    TokenSymbols,
}

impl ConfigFile {
    /// All files in a stable order.
    pub const ALL: [Self; 7] = [
        Self::Config,
        Self::Addresses,
        Self::Proxies,
        Self::Multisigs,
        Self::MultisigCache,
        Self::SubnetNames,
        Self::TokenSymbols,
    ];

    fn index(self) -> usize {
        match self {
            Self::Config => 0,
            Self::Addresses => 1,
            Self::Proxies => 2,
            Self::Multisigs => 3,
            Self::MultisigCache => 4,
            Self::SubnetNames => 5,
            Self::TokenSymbols => 6,
        }
    }

    /// The environment variable overriding this file's default path.
    pub const fn environment(self) -> &'static str {
        use rtao_catalog::config as catalog;
        match self {
            Self::Config => catalog::CONFIG_ENV_VAR,
            Self::Addresses => catalog::ADDRESSES_ENV_VAR,
            Self::Proxies => catalog::PROXIES_ENV_VAR,
            Self::Multisigs => catalog::MULTISIGS_ENV_VAR,
            Self::MultisigCache => catalog::MULTISIG_CACHE_ENV_VAR,
            Self::SubnetNames => catalog::SUBNET_NAMES_ENV_VAR,
            Self::TokenSymbols => catalog::TOKEN_SYMBOLS_ENV_VAR,
        }
    }

    fn default_parts(self) -> &'static [&'static str] {
        use rtao_catalog::config as catalog;
        match self {
            Self::Config => catalog::DEFAULT_CONFIG_PATH,
            Self::Addresses => catalog::DEFAULT_ADDRESSES_PATH,
            Self::Proxies => catalog::DEFAULT_PROXIES_PATH,
            Self::Multisigs => catalog::DEFAULT_MULTISIGS_PATH,
            Self::MultisigCache => catalog::DEFAULT_MULTISIG_CACHE_PATH,
            Self::SubnetNames => catalog::DEFAULT_SUBNET_NAMES_PATH,
            Self::TokenSymbols => catalog::DEFAULT_TOKEN_SYMBOLS_PATH,
        }
    }
}

/// Resolved shared file paths. Constructing paths performs no filesystem writes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Paths {
    paths: [PathBuf; 7],
}

impl Paths {
    /// Resolve catalog defaults and nonempty overrides with an injected home and
    /// environment reader. Relative overrides stay relative, as in Python.
    pub fn new(
        home: impl AsRef<Path>,
        mut environment: impl FnMut(&str) -> Option<OsString>,
    ) -> Self {
        let paths = ConfigFile::ALL.map(|file| {
            environment(file.environment())
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    file.default_parts()
                        .iter()
                        .fold(home.as_ref().to_owned(), |path, part| path.join(part))
                })
        });
        Self { paths }
    }

    /// Resolve the current process's environment. A missing home is an error;
    /// callers with an injected environment can use [`Self::new`] instead.
    pub fn from_process() -> Result<Self, ConfigError> {
        #[allow(deprecated)] // Includes the Unix account database fallback used by Path.home().
        let home = std::env::home_dir()
            .filter(|value| !value.as_os_str().is_empty())
            .ok_or(ConfigError::MissingHome)?;
        Ok(Self::new(home, |key| std::env::var_os(key)))
    }

    /// The path for a shared file.
    pub fn path(&self, file: ConfigFile) -> &Path {
        &self.paths[file.index()]
    }
}

/// Python's raw display-cache map. Integer keys may lie outside chain limits.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CacheMap(pub BTreeMap<BigInt, String>);

impl From<BTreeMap<NetUid, String>> for CacheMap {
    fn from(values: BTreeMap<NetUid, String>) -> Self {
        Self(
            values
                .into_iter()
                .map(|(key, value)| (BigInt::from(key.get()), value))
                .collect(),
        )
    }
}

/// A checked cache view for use with chain identifiers.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ChainCache {
    /// Entries with valid chain subnet identifiers.
    pub values: BTreeMap<NetUid, String>,
    /// Integer keys rejected by the chain's subnet-identifier rules.
    pub rejected: Vec<BigInt>,
}

impl CacheMap {
    /// Convert raw Python cache keys into checked chain identifiers. Rejected
    /// keys are returned explicitly so callers can report stale or invalid data.
    pub fn chain_values(&self) -> ChainCache {
        let mut result = ChainCache::default();
        for (key, value) in &self.0 {
            if let Some(netuid) = key.to_u16().and_then(|raw| NetUid::new(raw).ok()) {
                result.values.insert(netuid, value.clone());
            } else {
                result.rejected.push(key.clone());
            }
        }
        result
    }
}

/// Errors setting options or persisting shared files.
#[derive(Debug)]
pub enum ConfigError {
    /// No home directory is available from the process or account database.
    MissingHome,
    /// A config key is absent from the extracted `SETTABLE` table.
    UnknownKey {
        /// The requested key.
        key: String,
    },
    /// A boolean config value was not one of Python's accepted spellings.
    InvalidBoolean {
        /// The requested key.
        key: String,
        /// The original, untrimmed input.
        value: String,
    },
    /// A settable value uses a type not exercised by this pinned SDK. A future
    /// pin must implement that parser before accepting the new setting.
    UnsupportedCoercion {
        /// The requested key.
        key: String,
    },
    /// An injected clock returned a non-finite timestamp.
    InvalidClock,
    /// A persistent write failed.
    Io {
        /// The destination file.
        path: PathBuf,
        /// The operating-system error.
        source: io::Error,
    },
    /// JSON serialization failed.
    Json(serde_json::Error),
    /// The blocking task panicked or was cancelled.
    Offload(String),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use rtao_rpc::policy::python_string_repr as repr;
        match self {
            Self::MissingHome => f.write_str("a home directory is not available for btcli paths"),
            Self::UnknownKey { key } => {
                let mut keys: Vec<_> = settable().iter().map(|(key, _)| *key).collect();
                keys.sort_unstable();
                let keys = keys.into_iter().map(repr).collect::<Vec<_>>().join(", ");
                write!(
                    f,
                    "unknown config key {}; settable keys: [{keys}]",
                    repr(key)
                )
            }
            Self::InvalidBoolean { key, value } => write!(
                f,
                "{key} expects a boolean (true/false), got {}",
                repr(value)
            ),
            Self::UnsupportedCoercion { key } => {
                write!(f, "unsupported config coercion for {}", repr(key))
            }
            Self::InvalidClock => f.write_str("config clock returned a non-finite timestamp"),
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::Json(source) => source.fmt(f),
            Self::Offload(source) => write!(f, "config blocking task failed: {source}"),
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Json(source) => Some(source),
            _ => None,
        }
    }
}

/// Extracted Python config keys and their builtin type tags.
pub fn settable() -> &'static [(&'static str, rtao_catalog::config::BuiltinType)] {
    rtao_catalog::config::SETTABLE
}

/// A shared-file store with injectable paths and clock.
#[derive(Clone)]
pub struct Store {
    paths: Paths,
    clock: Arc<dyn Fn() -> f64 + Send + Sync>,
}

impl fmt::Debug for Store {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Store")
            .field("paths", &self.paths)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy)]
enum CacheKind {
    Names,
    Symbols,
}

impl CacheKind {
    fn file(self) -> ConfigFile {
        match self {
            Self::Names => ConfigFile::SubnetNames,
            Self::Symbols => ConfigFile::TokenSymbols,
        }
    }
    fn field(self) -> &'static str {
        match self {
            Self::Names => "names",
            Self::Symbols => "symbols",
        }
    }
    fn ttl(self) -> f64 {
        match self {
            Self::Names => rtao_catalog::config::SUBNET_NAMES_TTL_SECONDS as f64,
            Self::Symbols => rtao_catalog::config::TOKEN_SYMBOLS_TTL_SECONDS as f64,
        }
    }
}

impl Store {
    /// Construct a store using the system's Unix clock.
    pub fn new(paths: Paths) -> Self {
        Self::with_clock(paths, || {
            match SystemTime::now().duration_since(UNIX_EPOCH) {
                Ok(duration) => duration.as_secs_f64(),
                Err(error) => -error.duration().as_secs_f64(),
            }
        })
    }

    /// Construct a store with an injected clock returning Unix seconds. The
    /// clock runs on the blocking pool along with each file operation.
    pub fn with_clock(paths: Paths, clock: impl Fn() -> f64 + Send + Sync + 'static) -> Self {
        Self {
            paths,
            clock: Arc::new(clock),
        }
    }

    /// The resolved shared-file paths.
    pub fn paths(&self) -> &Paths {
        &self.paths
    }

    async fn run<T: Send + 'static>(
        &self,
        work: impl FnOnce(&Paths, f64) -> Result<T, ConfigError> + Send + 'static,
    ) -> Result<T, ConfigError> {
        let paths = self.paths.clone();
        let clock = self.clock.clone();
        rtao_core::offload(move || {
            let now = clock();
            if !now.is_finite() {
                return Err(ConfigError::InvalidClock);
            }
            work(&paths, now)
        })
        .await
        .map_err(|error| ConfigError::Offload(error.to_string()))?
    }

    /// Read known config keys. Unknown keys are ignored, and stored values are
    /// not coerced until callers explicitly set a value.
    pub async fn load(&self) -> Result<ConfigData, ConfigError> {
        self.run(|paths, now| Ok(load_config(paths, now))).await
    }

    /// Atomically save a complete config object with sorted keys.
    pub async fn save(&self, data: ConfigData) -> Result<PathBuf, ConfigError> {
        self.run(move |paths, _| {
            let path = paths.path(ConfigFile::Config);
            atomic_write(path, &Value::Object(sorted_object(data)))?;
            Ok(path.to_owned())
        })
        .await
    }

    /// Coerce and persist one extracted settable key, returning its stored value.
    pub async fn set_value(&self, key: &str, value: &str) -> Result<Value, ConfigError> {
        let value = coerce(key, value)?;
        let key = key.to_owned();
        self.run(move |paths, now| {
            let mut data = load_config(paths, now);
            data.insert(key, value.clone());
            atomic_write(
                paths.path(ConfigFile::Config),
                &Value::Object(sorted_object(data)),
            )?;
            Ok(value)
        })
        .await
    }

    /// Remove a key and persist the config, returning Python's result: a removed
    /// null value counts as unset.
    pub async fn unset_value(&self, key: &str) -> Result<bool, ConfigError> {
        require_key(key)?;
        let key = key.to_owned();
        self.run(move |paths, now| {
            let mut data = load_config(paths, now);
            let existed = data.remove(&key).is_some_and(|value| !value.is_null());
            atomic_write(
                paths.path(ConfigFile::Config),
                &Value::Object(sorted_object(data)),
            )?;
            Ok(existed)
        })
        .await
    }

    /// Read one stored value, returning the supplied default only when absent.
    pub async fn get(&self, key: &str, default: Value) -> Result<Value, ConfigError> {
        Ok(self.load().await?.remove(key).unwrap_or(default))
    }

    /// Read Python's subnet-name cache map for a network.
    pub async fn load_subnet_names(&self, network: &str) -> Result<CacheMap, ConfigError> {
        self.load_cache(CacheKind::Names, network).await
    }

    /// Read Python's token-symbol cache map for a network.
    pub async fn load_token_symbols(&self, network: &str) -> Result<CacheMap, ConfigError> {
        self.load_cache(CacheKind::Symbols, network).await
    }

    /// Whether a network's names were fetched less than the extracted TTL ago.
    pub async fn subnet_names_fresh(&self, network: &str) -> Result<bool, ConfigError> {
        self.cache_fresh(CacheKind::Names, network).await
    }

    /// Whether a network's symbols were fetched less than the extracted TTL ago.
    pub async fn token_symbols_fresh(&self, network: &str) -> Result<bool, ConfigError> {
        self.cache_fresh(CacheKind::Symbols, network).await
    }

    /// Atomically update one network's cached names, preserving other networks.
    pub async fn save_subnet_names(
        &self,
        network: &str,
        names: CacheMap,
    ) -> Result<PathBuf, ConfigError> {
        self.save_cache(CacheKind::Names, network, names).await
    }

    /// Atomically update one network's cached symbols, preserving other networks.
    pub async fn save_token_symbols(
        &self,
        network: &str,
        symbols: CacheMap,
    ) -> Result<PathBuf, ConfigError> {
        self.save_cache(CacheKind::Symbols, network, symbols).await
    }

    async fn load_cache(&self, kind: CacheKind, network: &str) -> Result<CacheMap, ConfigError> {
        let network = network.to_owned();
        self.run(move |paths, now| {
            let data = load_object(paths.path(kind.file()), now);
            let mut result = BTreeMap::new();
            if let Some(values) = data
                .get(&network)
                .and_then(|entry| entry.get(kind.field()))
                .and_then(Value::as_object)
            {
                for (key, value) in values {
                    if let Some(key) = parse_cache_key(key) {
                        result.insert(key, python_string(value));
                    }
                }
            }
            Ok(CacheMap(result))
        })
        .await
    }

    async fn cache_fresh(&self, kind: CacheKind, network: &str) -> Result<bool, ConfigError> {
        let network = network.to_owned();
        self.run(move |paths, now| {
            let data = load_object(paths.path(kind.file()), now);
            let timestamp = data.get(&network).and_then(|entry| entry.get("fetched_at"));
            let timestamp = match timestamp {
                Some(Value::Number(number)) => number.as_f64(),
                Some(Value::Bool(value)) => Some(f64::from(u8::from(*value))),
                _ => None,
            };
            Ok(timestamp.is_some_and(|timestamp| now - timestamp < kind.ttl()))
        })
        .await
    }

    async fn save_cache(
        &self,
        kind: CacheKind,
        network: &str,
        values: CacheMap,
    ) -> Result<PathBuf, ConfigError> {
        let network = network.to_owned();
        self.run(move |paths, now| {
            let path = paths.path(kind.file());
            let mut data = load_object(path, now);
            let values = values
                .0
                .into_iter()
                .map(|(key, value)| (key.to_string(), Value::String(value)))
                .collect();
            let timestamp: Number =
                format!("{:.0}", if now.trunc() == 0.0 { 0.0 } else { now.trunc() })
                    .parse()
                    .map_err(ConfigError::Json)?;
            let mut entry = Map::new();
            entry.insert("fetched_at".to_owned(), Value::Number(timestamp));
            entry.insert(kind.field().to_owned(), Value::Object(values));
            data.insert(network, Value::Object(entry));
            atomic_write(path, &Value::Object(data))?;
            Ok(path.to_owned())
        })
        .await
    }
}

fn require_key(key: &str) -> Result<rtao_catalog::config::BuiltinType, ConfigError> {
    settable()
        .iter()
        .find(|(candidate, _)| *candidate == key)
        .map(|(_, kind)| *kind)
        .ok_or_else(|| ConfigError::UnknownKey {
            key: key.to_owned(),
        })
}

fn coerce(key: &str, value: &str) -> Result<Value, ConfigError> {
    use rtao_catalog::config::BuiltinType;
    match require_key(key)? {
        BuiltinType::Str => Ok(Value::String(value.to_owned())),
        BuiltinType::Bool => match value
            .trim_matches(|character: char| {
                character.is_whitespace() || matches!(character, '\u{1c}'..='\u{1f}')
            })
            .to_lowercase()
            .as_str()
        {
            "true" | "1" | "yes" | "on" => Ok(Value::Bool(true)),
            "false" | "0" | "no" | "off" => Ok(Value::Bool(false)),
            _ => Err(ConfigError::InvalidBoolean {
                key: key.to_owned(),
                value: value.to_owned(),
            }),
        },
        BuiltinType::Int | BuiltinType::Float => Err(ConfigError::UnsupportedCoercion {
            key: key.to_owned(),
        }),
    }
}

fn load_config(paths: &Paths, now: f64) -> ConfigData {
    load_object(paths.path(ConfigFile::Config), now)
        .into_iter()
        .filter(|(key, _)| settable().iter().any(|(known, _)| *known == key))
        .collect()
}

fn load_object(path: &Path, now: f64) -> Map<String, Value> {
    if !path.is_file() {
        return Map::new();
    }
    let Ok(bytes) = fs::read(path) else {
        return Map::new();
    };
    match rtao_primitives::json::from_slice::<Value>(&bytes) {
        Ok(Value::Object(data)) => data,
        Ok(_) => Map::new(),
        Err(_) => {
            quarantine(path, now);
            Map::new()
        }
    }
}

fn quarantine(path: &Path, now: f64) {
    let Some(name) = path.file_name() else {
        return;
    };
    let base = format!(".corrupt.{:.0}", now.trunc());
    for ordinal in 0_u64.. {
        let mut name = name.to_owned();
        name.push(&base);
        if ordinal > 0 {
            name.push(format!(".{ordinal}"));
        }
        let backup = path.with_file_name(name);
        if !backup.exists() {
            let _ = fs::rename(path, backup);
            return;
        }
    }
}

fn sorted_object(data: Map<String, Value>) -> Map<String, Value> {
    data.into_iter()
        .collect::<BTreeMap<_, _>>()
        .into_iter()
        .collect()
}

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

fn atomic_write(path: &Path, value: &Value) -> Result<(), ConfigError> {
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let io_error = |source| ConfigError::Io {
        path: path.to_owned(),
        source,
    };
    fs::create_dir_all(parent).map_err(io_error)?;
    let mut bytes = serde_json::to_vec_pretty(value).map_err(ConfigError::Json)?;
    bytes.push(b'\n');
    let name = path.file_name().ok_or_else(|| {
        io_error(io::Error::new(
            io::ErrorKind::InvalidInput,
            "file path has no name",
        ))
    })?;
    let (temporary, mut file) = loop {
        let counter = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let mut temporary = name.to_owned();
        temporary.push(format!(".tmp.{}.{counter}", std::process::id()));
        let temporary = parent.join(temporary);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => break (temporary, file),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(io_error(error)),
        }
    };
    let result = (|| {
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path)?;
        File::open(parent)?.sync_all()
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result.map_err(io_error)
}

pub(crate) fn parse_cache_key(value: &str) -> Option<BigInt> {
    let value = value.trim_matches(|character: char| {
        matches!(character, ' ' | '\t' | '\n' | '\r' | '\x0b' | '\x0c')
            || (!character.is_ascii() && character.is_whitespace())
    });
    let negative = value.starts_with('-');
    let value = value
        .strip_prefix('+')
        .or_else(|| value.strip_prefix('-'))
        .unwrap_or(value);
    if value.is_empty() {
        return None;
    }
    let mut decimal = String::new();
    if negative {
        decimal.push('-');
    }
    let mut previous_digit = false;
    for character in value.chars() {
        if character == '_' && previous_digit {
            previous_digit = false;
            continue;
        }
        if get_general_category(character) != GeneralCategory::DecimalNumber {
            return None;
        }
        let mut start = u32::from(character);
        while start > 0 {
            let previous = char::from_u32(start - 1)?;
            if get_general_category(previous) != GeneralCategory::DecimalNumber {
                break;
            }
            start -= 1;
        }
        let digit = u8::try_from((u32::from(character) - start) % 10).ok()?;
        decimal.push(char::from(b'0' + digit));
        previous_digit = true;
    }
    if !previous_digit {
        return None;
    }
    decimal.parse().ok()
}

fn python_string(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        _ => python_repr(value),
    }
}

pub(crate) fn python_repr(value: &Value) -> String {
    match value {
        Value::Null => "None".to_owned(),
        Value::Bool(true) => "True".to_owned(),
        Value::Bool(false) => "False".to_owned(),
        Value::String(value) => rtao_rpc::policy::python_string_repr(value),
        Value::Number(value) => {
            let spelling = value.as_str();
            if spelling.contains(['.', 'e', 'E']) {
                spelling
                    .parse::<f64>()
                    .ok()
                    .map(rtao_primitives::pyfmt::float_repr)
                    .unwrap_or_else(|| spelling.to_owned())
            } else {
                spelling
                    .parse::<BigInt>()
                    .map_or_else(|_| spelling.to_owned(), |value| value.to_string())
            }
        }
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
                .map(|(key, value)| format!(
                    "{}: {}",
                    rtao_rpc::policy::python_string_repr(key),
                    python_repr(value)
                ))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::*;
    use std::sync::atomic::AtomicBool;

    struct Fixture {
        root: PathBuf,
        store: Store,
    }

    impl Fixture {
        fn new(now: f64) -> Self {
            let temp_root = std::env::var_os("RTAO_TEMP_ROOT")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/config-tests")
                });
            let root = temp_root.join("tmp").join(format!(
                "config-{}-{}",
                std::process::id(),
                NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&root).expect("create isolated config fixture");
            let paths = Paths::new(&root, |_| None);
            Self {
                root,
                store: Store::with_clock(paths, move || now),
            }
        }

        fn write(&self, file: ConfigFile, bytes: &[u8]) {
            let path = self.store.paths().path(file);
            fs::create_dir_all(path.parent().expect("fixture path parent"))
                .expect("create fixture parent");
            fs::write(path, bytes).expect("write fixture");
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            // Fixtures contain only files created by these tests; all offloaded
            // tasks have completed before the fixture leaves scope.
            fs::remove_dir_all(&self.root).expect("remove isolated config fixture");
        }
    }

    #[test]
    fn resolves_all_seven_paths_and_nonempty_environment_overrides() {
        let paths = Paths::new("/isolated/home", |key| match key {
            "BTCLI_CONFIG" => Some(OsString::from("relative/config.json")),
            "BTCLI_TOKEN_SYMBOLS_CACHE" => Some(OsString::from("/isolated/symbols.json")),
            _ => Some(OsString::new()),
        });
        assert_eq!(
            paths.path(ConfigFile::Config),
            Path::new("relative/config.json")
        );
        assert_eq!(
            paths.path(ConfigFile::TokenSymbols),
            Path::new("/isolated/symbols.json")
        );
        for file in ConfigFile::ALL {
            if matches!(file, ConfigFile::Config | ConfigFile::TokenSymbols) {
                continue;
            }
            let expected = file
                .default_parts()
                .iter()
                .fold(PathBuf::from("/isolated/home"), |path, part| {
                    path.join(part)
                });
            assert_eq!(paths.path(file), expected);
        }
    }

    #[tokio::test]
    async fn load_ignores_unknown_keys_and_preserves_exact_known_values() {
        let fixture = Fixture::new(42.0);
        assert!(
            fixture
                .store
                .load()
                .await
                .expect("missing config")
                .is_empty()
        );
        fixture.write(
            ConfigFile::Config,
            br#"{"network":"test","quiet":null,"wallet":18446744073709551616,"unknown":"discard"}"#,
        );
        let loaded = fixture.store.load().await.expect("load config");
        assert_eq!(loaded.len(), 3);
        assert!(loaded["quiet"].is_null());
        assert_eq!(
            loaded["wallet"].as_number().expect("integer").as_str(),
            "18446744073709551616"
        );
        assert_eq!(
            fixture
                .store
                .get("quiet", Value::Bool(true))
                .await
                .expect("get null"),
            Value::Null
        );
        fixture.write(ConfigFile::Config, b"[]");
        assert!(
            fixture
                .store
                .load()
                .await
                .expect("nonobject config")
                .is_empty()
        );
        assert!(fixture.store.paths().path(ConfigFile::Config).exists());
    }

    #[tokio::test]
    async fn malformed_json_is_retained_and_same_timestamp_backups_are_distinct() {
        let fixture = Fixture::new(1234.75);
        let path = fixture.store.paths().path(ConfigFile::Config);
        fixture.write(ConfigFile::Config, b"{first invalid");
        assert!(
            fixture
                .store
                .load()
                .await
                .expect("corrupt config")
                .is_empty()
        );
        let backup = path.with_file_name("btcli.json.corrupt.1234");
        assert_eq!(
            fs::read(&backup).expect("retained corrupt bytes"),
            b"{first invalid"
        );
        fixture.write(ConfigFile::Config, b"{second invalid");
        assert!(
            fixture
                .store
                .load()
                .await
                .expect("second corrupt config")
                .is_empty()
        );
        assert_eq!(
            fs::read(&backup).expect("first backup preserved"),
            b"{first invalid"
        );
        assert_eq!(
            fs::read(path.with_file_name("btcli.json.corrupt.1234.1")).expect("second backup"),
            b"{second invalid"
        );
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn set_unset_and_diagnostics_follow_python() {
        let fixture = Fixture::new(42.0);
        for (key, _) in settable() {
            assert!(
                coerce(key, "true").is_ok(),
                "every pinned settable key has a parser: {key}"
            );
        }
        for value in [" true ", "1", "YES", "on", "\u{1c}true\u{1f}"] {
            assert_eq!(
                fixture
                    .store
                    .set_value("quiet", value)
                    .await
                    .expect("coerce true"),
                Value::Bool(true)
            );
        }
        for value in [" false ", "0", "NO", "OFF"] {
            assert_eq!(
                fixture
                    .store
                    .set_value("quiet", value)
                    .await
                    .expect("coerce false"),
                Value::Bool(false)
            );
        }
        assert_eq!(
            fixture
                .store
                .set_value("wallet", "  preserved  ")
                .await
                .expect("set string"),
            Value::String("  preserved  ".to_owned())
        );
        let error = fixture
            .store
            .set_value("quiet", "maybe")
            .await
            .expect_err("bad boolean");
        assert_eq!(
            error.to_string(),
            "quiet expects a boolean (true/false), got 'maybe'"
        );
        let error = fixture
            .store
            .set_value("missing", "x")
            .await
            .expect_err("bad key");
        assert!(
            error
                .to_string()
                .starts_with("unknown config key 'missing'; settable keys: ['archive_endpoints',")
        );
        assert!(
            fixture
                .store
                .unset_value("wallet")
                .await
                .expect("unset existing")
        );
        assert!(
            !fixture
                .store
                .unset_value("wallet")
                .await
                .expect("unset absent")
        );
        fixture.write(ConfigFile::Config, br#"{"quiet":null}"#);
        assert!(
            !fixture
                .store
                .unset_value("quiet")
                .await
                .expect("unset null follows Python")
        );
    }

    #[tokio::test]
    async fn save_replaces_complete_file_and_removes_its_temporary_file() {
        let fixture = Fixture::new(42.0);
        fixture.write(ConfigFile::Config, b"old payload");
        let data = Map::from_iter([
            ("wallet".to_owned(), Value::String("alice".to_owned())),
            ("network".to_owned(), Value::String("test".to_owned())),
        ]);
        let path = fixture.store.save(data).await.expect("atomic config save");
        assert_eq!(
            fs::read_to_string(&path).expect("read complete config"),
            "{\n  \"network\": \"test\",\n  \"wallet\": \"alice\"\n}\n"
        );
        let entries: Vec<_> = fs::read_dir(path.parent().expect("parent"))
            .expect("read directory")
            .collect();
        assert_eq!(entries.len(), 1);
    }

    #[tokio::test]
    async fn failed_replace_cleans_temporary_file_and_preserves_destination() {
        let fixture = Fixture::new(42.0);
        let path = fixture.store.paths().path(ConfigFile::Config);
        fs::create_dir_all(path).expect("directory destination fixture");
        fs::write(path.join("retain.txt"), b"keep").expect("directory child");
        assert!(matches!(
            fixture.store.save(Map::new()).await,
            Err(ConfigError::Io { .. })
        ));
        assert_eq!(
            fs::read(path.join("retain.txt")).expect("preserved child"),
            b"keep"
        );
        assert_eq!(
            fs::read_dir(path.parent().expect("parent"))
                .expect("parent entries")
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn caches_preserve_other_networks_and_integer_timestamps() {
        let fixture = Fixture::new(1000.875);
        for kind in [CacheKind::Names, CacheKind::Symbols] {
            fixture.write(kind.file(), br#"{"other":{"fetched_at":7,"names":{"1":"other-name"},"symbols":{"1":"other-symbol"}}}"#);
            let values = CacheMap(BTreeMap::from([
                (BigInt::from(2), "α".to_owned()),
                (BigInt::from(10), "ten".to_owned()),
            ]));
            let path = fixture
                .store
                .save_cache(kind, "test", values.clone())
                .await
                .expect("save cache");
            assert_eq!(
                fixture
                    .store
                    .load_cache(kind, "test")
                    .await
                    .expect("load cache"),
                values
            );
            let data: Value =
                rtao_primitives::json::from_slice(&fs::read(path).expect("cache bytes"))
                    .expect("cache JSON");
            assert_eq!(
                data["test"]["fetched_at"]
                    .as_number()
                    .expect("timestamp")
                    .as_str(),
                "1000"
            );
            assert_eq!(data["other"]["fetched_at"].as_u64(), Some(7));
            assert!(
                fixture
                    .store
                    .cache_fresh(kind, "test")
                    .await
                    .expect("fresh cache")
            );
        }
    }

    #[tokio::test]
    async fn cache_ttl_is_strict_and_accepts_python_numeric_timestamps() {
        let fixture = Fixture::new(86400.0);
        fixture.write(ConfigFile::SubnetNames, br#"{"old":{"fetched_at":0},"fractional":{"fetched_at":0.5},"future":{"fetched_at":999999},"boolean":{"fetched_at":true},"string":{"fetched_at":"1000"}}"#);
        assert!(
            !fixture
                .store
                .subnet_names_fresh("old")
                .await
                .expect("exact TTL")
        );
        for network in ["fractional", "future", "boolean"] {
            assert!(
                fixture
                    .store
                    .subnet_names_fresh(network)
                    .await
                    .expect("numeric timestamp")
            );
        }
        for network in ["string", "missing"] {
            assert!(
                !fixture
                    .store
                    .subnet_names_fresh(network)
                    .await
                    .expect("invalid/missing timestamp")
            );
        }
        fixture.write(ConfigFile::TokenSymbols, br#"{"test":{"fetched_at":0}}"#);
        assert!(
            fixture
                .store
                .token_symbols_fresh("test")
                .await
                .expect("7-day symbols cache")
        );
    }

    #[tokio::test]
    async fn cache_integer_and_string_coercion_preserves_python_behavior() {
        let fixture = Fixture::new(42.0);
        fixture.write(ConfigFile::TokenSymbols, br#"{"test":{"symbols":{" +1_2 ":true,"-1":null,"184467440737095516160":184467440737095516160,"bad":"skip","13":1e-5,"14":[true,null,"x"],"15":{"a":false},"16":"original","17":"first","017":"last"}}}"#);
        let values = fixture
            .store
            .load_token_symbols("test")
            .await
            .expect("load raw cache");
        assert_eq!(values.0[&BigInt::from(12)], "True");
        assert_eq!(values.0[&BigInt::from(-1)], "None");
        assert_eq!(values.0[&BigInt::from(13)], "1e-05");
        assert_eq!(values.0[&BigInt::from(14)], "[True, None, 'x']");
        assert_eq!(values.0[&BigInt::from(15)], "{'a': False}");
        assert_eq!(values.0[&BigInt::from(17)], "last");
        let chain = values.chain_values();
        assert_eq!(chain.rejected.len(), 2);
        assert_eq!(
            chain.values[&NetUid::new(16).expect("chain netuid")],
            "original"
        );
        assert_eq!(
            parse_cache_key("\u{2003}+١_٢\u{2003}"),
            Some(BigInt::from(12))
        );
        assert_eq!(parse_cache_key("\u{1e4f1}"), Some(BigInt::from(1)));
        for key in ["1__2", "1_", "_1", "\u{1c}1", "1.0", "", "\u{10d41}"] {
            assert_eq!(parse_cache_key(key), None, "{key:?}");
        }
    }

    #[tokio::test]
    async fn corrupt_cache_is_preserved_before_replacement() {
        let fixture = Fixture::new(42.0);
        fixture.write(ConfigFile::TokenSymbols, b"corrupt symbols");
        assert!(
            fixture
                .store
                .load_token_symbols("test")
                .await
                .expect("corrupt symbols")
                .0
                .is_empty()
        );
        fixture
            .store
            .save_token_symbols("test", CacheMap::default())
            .await
            .expect("new symbols cache");
        let path = fixture.store.paths().path(ConfigFile::TokenSymbols);
        assert_eq!(
            fs::read(path.with_file_name("btcli_token_symbols.json.corrupt.42"))
                .expect("cache backup"),
            b"corrupt symbols"
        );
        assert!(path.exists());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn filesystem_operations_and_clock_run_off_the_reactor() {
        let fixture = Fixture::new(42.0);
        let reactor = std::thread::current().id();
        let observed = Arc::new(AtomicBool::new(false));
        let flag = observed.clone();
        let store = Store::with_clock(fixture.store.paths().clone(), move || {
            flag.store(std::thread::current().id() != reactor, Ordering::Relaxed);
            42.0
        });
        store.save(Map::new()).await.expect("offloaded save");
        assert!(observed.load(Ordering::Relaxed));
        let invalid = Store::with_clock(fixture.store.paths().clone(), || f64::NAN);
        assert!(matches!(
            invalid.load().await,
            Err(ConfigError::InvalidClock)
        ));
    }
}
