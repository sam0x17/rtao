// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/context.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Additional sources at the same commit/license: sdk/python/bittensor/cli/multisig_helpers.py, sdk/python/bittensor/cli/call_names.py and sdk/python/bittensor/config.py.
//! Read-only books and offline account reference resolution.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
};

use rtao::{
    client::config::{ConfigFile, Paths},
    primitives::{AccountId, pyfmt},
    signer::SignerRole,
    wallet::{WalletError, WalletRef},
};
use serde_json::Value;
use zeroize::Zeroizing;

/// Bound each book or cosmetic keyfile read before parsing untrusted JSON.
const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;

/// Maximum offline multisig-reference depth and in-memory JSON nesting.
///
/// Text JSON also retains serde_json's default nesting guard. Programmatic
/// values and saved preset chains receive an explicit refusal beyond this
/// limit; they are never silently truncated or treated as successful lookups.
pub const MAX_NAME_RESOLUTION_DEPTH: usize = 128;

/// Shared btcli books, retaining row order and additional fields.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Books {
    /// Named contacts, including optional signing-backend tags.
    pub addresses: Vec<Value>,
    /// Proxy account presets.
    pub proxies: Vec<Value>,
    /// Saved multisig signer sets.
    pub multisigs: Vec<Value>,
}

impl Books {
    /// Read exactly the paths supplied by the shared config path resolver.
    ///
    /// Missing, unreadable, invalid-JSON and non-list files are
    /// empty, matching the book readers' best-effort behavior. No file is
    /// created, backed up or changed. Oversized files return a bounded-input
    /// error. Filesystem work runs on a blocking worker.
    pub async fn load(paths: &Paths) -> Result<Self, ResolveError> {
        let paths = paths.clone();
        rtao::core::offload(move || Self::load_blocking(&paths))
            .await
            .map_err(|error| ResolveError::new(error.to_string()))?
    }

    fn load_blocking(paths: &Paths) -> Result<Self, ResolveError> {
        Ok(Self::from_values(
            read_json(paths.path(ConfigFile::Addresses))?,
            read_json(paths.path(ConfigFile::Proxies))?,
            read_json(paths.path(ConfigFile::Multisigs))?,
        ))
    }

    /// Inject parsed book values without filesystem access.
    pub fn from_values(addresses: Value, proxies: Value, multisigs: Value) -> Self {
        fn rows(value: Value) -> Vec<Value> {
            match value {
                Value::Array(rows) => rows,
                _ => Vec::new(),
            }
        }
        Self {
            addresses: rows(addresses),
            proxies: rows(proxies),
            multisigs: rows(multisigs),
        }
    }

    /// Return the first contact row matching its name or stored address.
    pub fn address_entry(&self, name_or_address: &str) -> Option<&Value> {
        self.addresses.iter().find(|entry| {
            entry.get("name").and_then(Value::as_str) == Some(name_or_address)
                || entry.get("address").and_then(Value::as_str) == Some(name_or_address)
        })
    }

    /// Return the first proxy row matching its name.
    pub fn proxy(&self, name: &str) -> Option<&Value> {
        named(&self.proxies, name)
    }

    /// Return the first saved multisig row matching its name.
    pub fn multisig(&self, name: &str) -> Option<&Value> {
        named(&self.multisigs, name)
    }

    fn address(&self, name: &str) -> Option<&str> {
        named(&self.addresses, name)?
            .get("address")?
            .as_str()
            .filter(|address| !address.is_empty())
    }
}

fn named<'a>(rows: &'a [Value], name: &str) -> Option<&'a Value> {
    rows.iter()
        .find(|entry| entry.get("name").and_then(Value::as_str) == Some(name))
}

fn read_json(path: &Path) -> Result<Value, ResolveError> {
    let read = || -> Result<Option<Value>, ResolveError> {
        if !path.is_file() {
            return Ok(None);
        }
        let Ok(file) = File::open(path) else {
            return Ok(None);
        };
        let oversized =
            || ResolveError::new("book or public keyfile exceeds the 16 MiB input limit");
        if file
            .metadata()
            .is_ok_and(|metadata| metadata.len() > MAX_FILE_BYTES)
        {
            return Err(oversized());
        }
        let mut bytes = Vec::new();
        if file
            .take(MAX_FILE_BYTES + 1)
            .read_to_end(&mut bytes)
            .is_err()
        {
            return Ok(None);
        }
        if bytes.len() as u64 > MAX_FILE_BYTES {
            return Err(oversized());
        }
        Ok(rtao::primitives::json::from_slice(&bytes).ok())
    };
    read().map(|value| value.unwrap_or(Value::Null))
}

/// An account-reference lookup failure without any secret key material.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolveError {
    message: String,
}

impl ResolveError {
    /// Preserve a public lookup diagnostic supplied by an injected provider.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for ResolveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ResolveError {}

/// A known local name used for did-you-mean suggestions.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NameCandidate {
    /// Contact name, wallet name or `wallet/hotkey` reference.
    pub name: String,
    /// Stored public ss58 spelling.
    pub address: String,
    /// Description such as `wallet` or `hotkey`.
    pub source: String,
}

/// Public-only local wallet access, injectable for offline tests and commands.
pub trait AccountLookup {
    /// Read a wallet's coldkey public account without unlocking it.
    fn coldkey(&self, wallet: &str) -> Result<String, ResolveError>;
    /// Read a named hotkey's public account without unlocking it.
    fn hotkey(&self, wallet: &str, hotkey: &str) -> Result<String, ResolveError>;
    /// List cosmetic name candidates. An unreadable directory may return none.
    fn candidates(&self) -> Vec<NameCandidate> {
        Vec::new()
    }
}

/// Filesystem adapter using the wallet library's public key view.
#[derive(Clone, Debug)]
pub struct FsAccountLookup {
    root: PathBuf,
}

impl FsAccountLookup {
    /// Resolve a wallet root, including the wallet library's `~/` expansion.
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, ResolveError> {
        let reference = WalletRef::new("default", "default", root)
            .map_err(|error| ResolveError::new(error.to_string()))?;
        Ok(Self {
            root: reference.path().to_owned(),
        })
    }

    /// Borrow the expanded wallet root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn reference(&self, wallet: &str, hotkey: &str) -> Result<WalletRef, ResolveError> {
        WalletRef::new(wallet, hotkey, self.root.clone())
            .map_err(|error| ResolveError::new(error.to_string()))
    }
}

impl AccountLookup for FsAccountLookup {
    fn coldkey(&self, wallet: &str) -> Result<String, ResolveError> {
        self.reference(wallet, "default")?
            .public_info(SignerRole::Coldkey)
            .map(|info| info.account.to_string())
            .map_err(public_lookup_error)
    }

    fn hotkey(&self, wallet: &str, hotkey: &str) -> Result<String, ResolveError> {
        self.reference(wallet, hotkey)?
            .public_info(SignerRole::Hotkey)
            .map(|info| info.account.to_string())
            .map_err(public_lookup_error)
    }

    fn candidates(&self) -> Vec<NameCandidate> {
        let mut candidates = Vec::new();
        let Ok(wallets) = fs::read_dir(&self.root) else {
            return candidates;
        };
        for entry in wallets.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let Some(wallet) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if let Some(address) = stored_address(&path.join("coldkeypub.txt")) {
                candidates.push(NameCandidate {
                    name: wallet.clone(),
                    address,
                    source: "wallet".into(),
                });
            }
            let Ok(hotkeys) = fs::read_dir(path.join("hotkeys")) else {
                continue;
            };
            for entry in hotkeys.flatten() {
                let Some(hotkey) = entry.file_name().to_str().map(str::to_owned) else {
                    continue;
                };
                if hotkey.ends_with(".lock")
                    || hotkey.ends_with("pub.txt")
                    || !entry.path().is_file()
                {
                    continue;
                }
                if let Some(address) = stored_address(&entry.path()) {
                    candidates.push(NameCandidate {
                        name: format!("{wallet}/{hotkey}"),
                        address,
                        source: "hotkey".into(),
                    });
                }
            }
        }
        candidates
    }
}

fn public_lookup_error(error: WalletError) -> ResolveError {
    // Preserve a useful public cause without exposing absolute home paths or
    // passing keyfile/parser diagnostics into an account-name error.
    let message = match error {
        WalletError::Io { error, .. } => format!("cannot read wallet public key: {}", error.kind()),
        WalletError::PublicKeyUnavailable(_) => "wallet public key is unavailable".into(),
        WalletError::InvalidName(message) => message,
        _ => "wallet public key lookup failed".into(),
    };
    ResolveError::new(message)
}

fn stored_address(path: &Path) -> Option<String> {
    use serde::{Deserialize, de::IgnoredAny};

    // Suggestions inspect plaintext files rather than public companions, as
    // Python does. Read the bytes in wiping storage and deserialize only the
    // public address. Unknown fields (including recovery material) are consumed
    // by IgnoredAny and never copied into owned JSON strings.
    struct PublicAddress {
        address: Option<PublicString>,
    }
    impl<'de> Deserialize<'de> for PublicAddress {
        fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            #[derive(Deserialize)]
            #[serde(field_identifier)]
            enum Field {
                #[serde(rename = "ss58Address")]
                Address,
                #[serde(other)]
                Ignore,
            }
            struct AddressVisitor;
            impl<'de> serde::de::Visitor<'de> for AddressVisitor {
                type Value = PublicAddress;
                fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                    formatter.write_str("a public keyfile object")
                }
                fn visit_map<A: serde::de::MapAccess<'de>>(
                    self,
                    mut access: A,
                ) -> Result<Self::Value, A::Error> {
                    let mut address = None;
                    while let Some(field) = access.next_key::<Field>()? {
                        match field {
                            Field::Address => address = Some(access.next_value::<PublicString>()?),
                            Field::Ignore => {
                                let _: IgnoredAny = access.next_value()?;
                            }
                        }
                    }
                    // Python json.loads keeps the final duplicate-key value.
                    Ok(PublicAddress { address })
                }
            }
            deserializer.deserialize_map(AddressVisitor)
        }
    }
    struct PublicString(Option<String>);
    impl<'de> Deserialize<'de> for PublicString {
        fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            struct PublicVisitor;
            impl<'de> serde::de::Visitor<'de> for PublicVisitor {
                type Value = PublicString;
                fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                    formatter.write_str("a public address or ignored non-string value")
                }
                fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
                    Ok(PublicString(Some(value.to_owned())))
                }
                fn visit_string<E: serde::de::Error>(
                    self,
                    value: String,
                ) -> Result<Self::Value, E> {
                    Ok(PublicString(Some(value)))
                }
                fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<Self::Value, E> {
                    Ok(PublicString(None))
                }
                fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<Self::Value, E> {
                    Ok(PublicString(None))
                }
                fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<Self::Value, E> {
                    Ok(PublicString(None))
                }
                fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<Self::Value, E> {
                    Ok(PublicString(None))
                }
                fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                    Ok(PublicString(None))
                }
                fn visit_seq<A: serde::de::SeqAccess<'de>>(
                    self,
                    mut access: A,
                ) -> Result<Self::Value, A::Error> {
                    while access.next_element::<IgnoredAny>()?.is_some() {}
                    Ok(PublicString(None))
                }
                fn visit_map<A: serde::de::MapAccess<'de>>(
                    self,
                    mut access: A,
                ) -> Result<Self::Value, A::Error> {
                    while access.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
                    Ok(PublicString(None))
                }
            }
            deserializer.deserialize_any(PublicVisitor)
        }
    }
    if !path.is_file() {
        return None;
    }
    let file = File::open(path).ok()?;
    if file
        .metadata()
        .is_ok_and(|metadata| metadata.len() > MAX_FILE_BYTES)
    {
        return None;
    }
    let mut bytes = Zeroizing::new(Vec::new());
    file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return None;
    }
    // The typed reader retains only a string. It does not deserialize numeric
    // fields and therefore does not use Serde's untagged/flatten buffering.
    let address = serde_json::from_slice::<PublicAddress>(&bytes)
        .ok()?
        .address?
        .0?;
    (!address.is_empty()).then_some(address)
}

/// Display classification derived from the account parameter's name.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccountKind {
    /// A parameter containing `hotkey`.
    Hotkey,
    /// Every other account parameter.
    Coldkey,
}

impl AccountKind {
    /// Python's name-based display classification.
    pub fn for_param(param: &str) -> Self {
        if param.contains("hotkey") {
            Self::Hotkey
        } else {
            Self::Coldkey
        }
    }
}

/// An account and the public lookup information needed by the output renderer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedAddress {
    /// Resolved ss58 address, retaining explicit raw or book spelling.
    pub address: String,
    /// Lookup description, including the reference when it was a name.
    pub source: String,
    /// Name registered for this address, when known.
    pub name: Option<String>,
    /// Hotkey/coldkey display classification.
    pub kind: AccountKind,
}

/// Offline resolver shared by flag values and account fields in JSON.
pub struct NameResolver<'a> {
    books: &'a Books,
    accounts: &'a dyn AccountLookup,
    wallet: &'a str,
    hotkey: &'a str,
}

#[derive(Default)]
struct ResolutionState {
    active: BTreeSet<String>,
    // Memoize only within one top-level lookup. Shared signer graphs cannot
    // force exponential re-derivation, and changed files are read next time.
    derived: BTreeMap<String, (String, usize)>,
    heights: Vec<usize>,
}

impl<'a> NameResolver<'a> {
    /// Inject books, public wallet access and the invocation's default keys.
    pub fn new(
        books: &'a Books,
        accounts: &'a dyn AccountLookup,
        wallet: &'a str,
        hotkey: &'a str,
    ) -> Self {
        Self {
            books,
            accounts,
            wallet,
            hotkey,
        }
    }

    /// Resolve an explicit account reference, without prompting or chain I/O.
    ///
    /// Precedence is raw Bittensor ss58, contact, proxy, saved multisig
    /// (coldkey parameters), then local wallet/hotkey.
    pub fn resolve_address_ref(
        &self,
        param: &str,
        value: &str,
    ) -> Result<ResolvedAddress, ResolveError> {
        self.resolve_ref(param, value, &mut ResolutionState::default())
    }

    /// Resolve a flag after any interactive target selection.
    ///
    /// Only omitted `coldkey_ss58` and `hotkey_ss58` use the configured keys.
    /// Destination and all other omitted account fields return `None`.
    pub fn resolve_address(
        &self,
        param: &str,
        value: Option<&str>,
    ) -> Result<Option<ResolvedAddress>, ResolveError> {
        if let Some(value) = value {
            return self.resolve_address_ref(param, value).map(Some);
        }
        let (address, name, source) = match param {
            "coldkey_ss58" => {
                if let Some(address) = self.saved_multisig_address(self.wallet)? {
                    (address, self.wallet.to_owned(), "saved multisig")
                } else {
                    (
                        self.accounts.coldkey(self.wallet)?,
                        self.wallet.to_owned(),
                        "wallet",
                    )
                }
            }
            "hotkey_ss58" => (
                self.accounts.hotkey(self.wallet, self.hotkey)?,
                format!("{}/{}", self.wallet, self.hotkey),
                "hotkey",
            ),
            _ => return Ok(None),
        };
        Ok(Some(ResolvedAddress {
            address,
            source: format!("{source} {}", pyfmt::string_repr(&name)),
            name: Some(name),
            kind: AccountKind::for_param(param),
        }))
    }

    /// Derive a saved multisig account locally, refusing indirect self-reference.
    pub fn saved_multisig_address(&self, name: &str) -> Result<Option<String>, ResolveError> {
        self.saved_multisig(name, &mut ResolutionState::default())
    }

    /// Resolve comma/whitespace-separated coldkey references and deduplicate
    /// addresses while retaining their first occurrence.
    pub fn resolve_signatory_list(&self, raw: &str) -> Result<Vec<String>, ResolveError> {
        let refs = split_signatory_refs(raw);
        if refs.is_empty() {
            return Err(ResolveError::new("need at least one signatory"));
        }
        let mut resolved = Vec::new();
        for value in refs {
            let address = self.resolve_address_ref("coldkey_ss58", value)?.address;
            if !resolved.contains(&address) {
                resolved.push(address);
            }
        }
        Ok(resolved)
    }

    /// Python's closest-name suggestion: contact and proxy names precede local
    /// wallet candidates, with SequenceMatcher's 0.6 cutoff and tie ordering.
    /// Saved multisigs are resolved normally but are absent from this cosmetic
    /// candidate list, as in the pinned Python implementation.
    pub fn closest_name(&self, value: &str) -> Option<NameCandidate> {
        let mut candidates = BTreeMap::new();
        for (rows, source) in [
            (&self.books.addresses, "address-book entry"),
            (&self.books.proxies, "proxy-book entry"),
        ] {
            for entry in rows {
                let (Some(name), Some(address)) = (
                    entry.get("name").and_then(Value::as_str),
                    entry.get("address").and_then(Value::as_str),
                ) else {
                    continue;
                };
                if !name.is_empty() {
                    candidates.entry(name.to_owned()).or_insert(NameCandidate {
                        name: name.to_owned(),
                        address: address.to_owned(),
                        source: source.to_owned(),
                    });
                }
            }
        }
        for candidate in self.accounts.candidates() {
            if !candidate.address.is_empty() {
                candidates
                    .entry(candidate.name.clone())
                    .or_insert(candidate);
            }
        }
        candidates
            .into_values()
            .filter_map(|candidate| {
                // get_close_matches compares each candidate (a) to input (b).
                let ratio = sequence_ratio(&candidate.name, value);
                (ratio >= 0.6).then_some((ratio, candidate))
            })
            .max_by(|(left_ratio, left), (right_ratio, right)| {
                left_ratio
                    .total_cmp(right_ratio)
                    .then_with(|| left.name.cmp(&right.name))
            })
            .map(|(_, candidate)| candidate)
    }

    fn resolve_ref(
        &self,
        param: &str,
        value: &str,
        resolving: &mut ResolutionState,
    ) -> Result<ResolvedAddress, ResolveError> {
        let kind = AccountKind::for_param(param);
        let (address, source, name) = if is_bittensor_address(value) {
            let name = self
                .books
                .addresses
                .iter()
                .find(|entry| entry.get("address").and_then(Value::as_str) == Some(value))
                .and_then(|entry| entry.get("name"))
                .and_then(Value::as_str)
                .map(str::to_owned);
            (value.to_owned(), "ss58 address".to_owned(), name)
        } else if let Some(address) = self.books.address(value) {
            (
                address.to_owned(),
                format!("address-book entry {}", pyfmt::string_repr(value)),
                Some(value.to_owned()),
            )
        } else if let Some(address) = self
            .books
            .proxy(value)
            .and_then(|entry| entry.get("address"))
            .and_then(Value::as_str)
            .filter(|address| !address.is_empty())
        {
            (
                address.to_owned(),
                format!("proxy-book entry {}", pyfmt::string_repr(value)),
                Some(value.to_owned()),
            )
        } else if kind == AccountKind::Coldkey {
            match self.saved_multisig(value, resolving)? {
                Some(address) => (
                    address,
                    format!("saved multisig {}", pyfmt::string_repr(value)),
                    Some(value.to_owned()),
                ),
                None => (
                    self.accounts.coldkey(value)?,
                    format!("wallet {}", pyfmt::string_repr(value)),
                    Some(value.to_owned()),
                ),
            }
        } else {
            let (wallet, hotkey) = value.rsplit_once('/').unwrap_or(("", value));
            let wallet = if wallet.is_empty() {
                self.wallet
            } else {
                wallet
            };
            (
                self.accounts.hotkey(wallet, hotkey)?,
                format!("hotkey {}", pyfmt::string_repr(value)),
                Some(value.to_owned()),
            )
        };
        Ok(ResolvedAddress {
            address,
            source,
            name,
            kind,
        })
    }

    fn saved_multisig(
        &self,
        name: &str,
        resolving: &mut ResolutionState,
    ) -> Result<Option<String>, ResolveError> {
        let Some(entry) = self.books.multisig(name).filter(|_| !name.is_empty()) else {
            return Ok(None);
        };
        if resolving.active.contains(name) {
            return Err(ResolveError::new(format!(
                "multisig {} refers to itself through its signatories",
                pyfmt::string_repr(name)
            )));
        }
        if let Some((address, height)) = resolving.derived.get(name) {
            if resolving.heights.len() + height > MAX_NAME_RESOLUTION_DEPTH {
                return Err(multisig_depth_error());
            }
            let address = address.clone();
            if let Some(parent_height) = resolving.heights.last_mut() {
                *parent_height = (*parent_height).max(height + 1);
            }
            return Ok(Some(address));
        }
        if resolving.heights.len() >= MAX_NAME_RESOLUTION_DEPTH {
            return Err(multisig_depth_error());
        }
        resolving.active.insert(name.to_owned());
        resolving.heights.push(1);
        let mut derive = || -> Result<String, ResolveError> {
            let threshold = entry
                .get("threshold")
                .and_then(threshold_u16)
                .ok_or_else(|| {
                    ResolveError::new("saved multisig threshold is not a u16 integer")
                })?;
            let refs = entry
                .get("signatories")
                .and_then(Value::as_array)
                .ok_or_else(|| ResolveError::new("saved multisig signatories must be a list"))?;
            let mut addresses = Vec::new();
            for value in refs {
                let reference = value.as_str().ok_or_else(|| {
                    ResolveError::new(
                        "saved multisig signatory must be an account reference string",
                    )
                })?;
                let address = self
                    .resolve_ref("coldkey_ss58", reference, resolving)?
                    .address;
                if !addresses.contains(&address) {
                    addresses.push(address);
                }
            }
            let keys = addresses
                .iter()
                .map(|address| {
                    address
                        .parse::<AccountId>()
                        .map(|account| account.0)
                        .map_err(|error| ResolveError::new(error.to_string()))
                })
                .collect::<Result<Vec<_>, _>>()?;
            let (account, _) = rtao::core::extrinsic::multisig_account_id(&keys, threshold)
                .map_err(|error| ResolveError::new(error.to_string()))?;
            Ok(AccountId(account).to_string())
        };
        let result = derive();
        let height = resolving.heights.pop().unwrap_or(1);
        resolving.active.remove(name);
        if let Ok(address) = &result {
            resolving
                .derived
                .insert(name.to_owned(), (address.clone(), height));
            if let Some(parent_height) = resolving.heights.last_mut() {
                *parent_height = (*parent_height).max(height + 1);
            }
        }
        result.map(Some)
    }
}

fn multisig_depth_error() -> ResolveError {
    ResolveError::new(format!(
        "saved multisig references exceed the resolution depth limit ({MAX_NAME_RESOLUTION_DEPTH})"
    ))
}

fn threshold_u16(value: &Value) -> Option<u16> {
    match value {
        Value::String(value) => pyfmt::parse_integer(value)?.to_string().parse().ok(),
        Value::Bool(value) => Some(u16::from(*value)),
        Value::Number(number) => {
            if let Some(integer) = pyfmt::parse_integer(number.as_str()) {
                return integer.to_string().parse().ok();
            }
            let number = number.as_f64()?.trunc();
            (number.is_finite() && (0.0..=f64::from(u16::MAX)).contains(&number))
                .then_some(number as u16)
        }
        _ => None,
    }
}

/// True only for a valid Bittensor (format 42) ss58 account.
pub fn is_bittensor_address(value: &str) -> bool {
    rtao::core::keys::public_key_from_ss58(value)
        .is_ok_and(|public| rtao::core::keys::ss58_from_public(public, 42) == value)
}

/// CLI flag spelling for address parameters, including embedded `_ss58`.
pub fn address_cli_name(param: &str) -> String {
    let base = param
        .strip_suffix("_ss58")
        .map_or_else(|| param.replace("_ss58", ""), str::to_owned);
    format!("--{}", base.replace('_', "-"))
}

/// Python's help text for an address-typed CLI option.
pub fn ss58_param_help(param: &str) -> String {
    let book = "address-book or proxy-book name, ";
    if param.contains("hotkey") {
        let mut text =
            format!("ss58 address, {book}or a local hotkey name (HOTKEY or WALLET/HOTKEY).");
        if param == "hotkey_ss58" {
            text.push_str(" Defaults to your wallet's hotkey.");
        }
        text
    } else if matches!(param, "dest_ss58" | "dest_coldkey_ss58") {
        format!(
            "Destination account (a coldkey, not a hotkey): ss58 address, {book}saved multisig name, or a local wallet name. Omit this flag on a terminal to pick from the address book."
        )
    } else {
        let mut text = format!(
            "ss58 address, {book}saved multisig name, or a local wallet name (uses its coldkey)."
        );
        if param == "coldkey_ss58" {
            text.push_str(" Defaults to your wallet's coldkey.");
        }
        text
    }
}

/// Split comma/whitespace-separated signer references, preserving order.
pub fn split_signatory_refs(raw: &str) -> Vec<&str> {
    raw.split(|character: char| character == ',' || python_whitespace(character))
        .filter(|part| !part.is_empty())
        .collect()
}

fn python_whitespace(character: char) -> bool {
    // CPython str/regex whitespace additionally includes these C0 separators.
    character.is_whitespace() || matches!(character, '\u{1c}'..='\u{1f}')
}

// Derived from CPython 3.12.12 Lib/difflib.py @ 4a5632fbf9bf59477c540e3f53fa7cdbeea3e3f5 (PSF-2.0); modified by rtao.
// Port of SequenceMatcher with isjunk=None and autojunk=True.
// Popular characters cannot seed a match, but may extend a seeded match.
pub(crate) fn sequence_ratio(left: &str, right: &str) -> f64 {
    let left: Vec<_> = left.chars().collect();
    let right: Vec<_> = right.chars().collect();
    if left.is_empty() && right.is_empty() {
        return 1.0;
    }
    let mut positions: BTreeMap<char, Vec<usize>> = BTreeMap::new();
    for (index, character) in right.iter().enumerate() {
        positions.entry(*character).or_default().push(index);
    }
    if right.len() >= 200 {
        let popular = right.len() / 100 + 1;
        positions.retain(|_, indexes| indexes.len() <= popular);
    }
    let mut queue = vec![(0, left.len(), 0, right.len())];
    let mut matched = 0;
    while let Some((left_lo, left_hi, right_lo, right_hi)) = queue.pop() {
        let (mut best_left, mut best_right, mut size) = (left_lo, right_lo, 0);
        let mut previous = BTreeMap::new();
        for (left_index, character) in left.iter().enumerate().take(left_hi).skip(left_lo) {
            let mut next = BTreeMap::new();
            if let Some(indexes) = positions.get(character) {
                for &right_index in indexes {
                    if right_index < right_lo {
                        continue;
                    }
                    if right_index >= right_hi {
                        break;
                    }
                    let length = right_index
                        .checked_sub(1)
                        .and_then(|index| previous.get(&index).copied())
                        .unwrap_or(0)
                        + 1;
                    next.insert(right_index, length);
                    if length > size {
                        (best_left, best_right, size) =
                            (left_index + 1 - length, right_index + 1 - length, length);
                    }
                }
            }
            previous = next;
        }
        while best_left > left_lo
            && best_right > right_lo
            && left[best_left - 1] == right[best_right - 1]
        {
            best_left -= 1;
            best_right -= 1;
            size += 1;
        }
        while best_left + size < left_hi
            && best_right + size < right_hi
            && left[best_left + size] == right[best_right + size]
        {
            size += 1;
        }
        if size == 0 {
            continue;
        }
        matched += size;
        if left_lo < best_left && right_lo < best_right {
            queue.push((left_lo, best_left, right_lo, best_right));
        }
        if best_left + size < left_hi && best_right + size < right_hi {
            queue.push((best_left + size, left_hi, best_right + size, right_hi));
        }
    }
    2.0 * matched as f64 / (left.len() + right.len()) as f64
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::{
        error::Error,
        sync::{
            Mutex,
            atomic::{AtomicU64, Ordering},
        },
    };

    const ALICE: &str = "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY";
    const BOB: &str = "5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty";
    const CHARLIE: &str = "5FLSigC9HGRKVhB9FiEo4Y3koPsNmBmLJbpXg2mp1hXcS59Y";

    #[derive(Default)]
    struct Lookup {
        calls: Mutex<Vec<String>>,
    }

    impl AccountLookup for Lookup {
        fn coldkey(&self, wallet: &str) -> Result<String, ResolveError> {
            if let Ok(mut calls) = self.calls.lock() {
                calls.push(format!("cold:{wallet}"));
            }
            match wallet {
                "default" | "alice" => Ok(ALICE.into()),
                "bob" => Ok(BOB.into()),
                "treasury" => Ok(CHARLIE.into()),
                _ => Err(ResolveError::new(format!("no public wallet {wallet}"))),
            }
        }

        fn hotkey(&self, wallet: &str, hotkey: &str) -> Result<String, ResolveError> {
            if let Ok(mut calls) = self.calls.lock() {
                calls.push(format!("hot:{wallet}/{hotkey}"));
            }
            Ok(CHARLIE.into())
        }

        fn candidates(&self) -> Vec<NameCandidate> {
            vec![
                NameCandidate {
                    name: "alice".into(),
                    address: ALICE.into(),
                    source: "wallet".into(),
                },
                NameCandidate {
                    name: "default/worker".into(),
                    address: CHARLIE.into(),
                    source: "hotkey".into(),
                },
            ]
        }
    }

    #[test]
    fn explicit_lookup_obeys_all_book_precedence_and_raw_name_registration()
    -> Result<(), Box<dyn Error>> {
        let books = Books::from_values(
            json!([
                {"name":"alice","address":BOB,"signer":"ledger"},
                {"name":"alice","address":CHARLIE},
                {"name":"raw-label","address":ALICE},
            ]),
            json!([{"name":"alice","address":CHARLIE},{"name":"treasury","address":BOB}]),
            json!([{"name":"treasury","threshold":2,"signatories":[ALICE,BOB]}]),
        );
        let lookup = Lookup::default();
        let names = NameResolver::new(&books, &lookup, "default", "worker");
        let contact = names.resolve_address_ref("dest_ss58", "alice")?;
        assert_eq!(contact.address, BOB);
        assert_eq!(contact.source, "address-book entry 'alice'");
        assert_eq!(
            books
                .address_entry("alice")
                .and_then(|row| row.get("signer")),
            Some(&json!("ledger"))
        );
        let proxy = names.resolve_address_ref("new_coldkey", "treasury")?;
        assert_eq!(proxy.address, BOB);
        assert_eq!(proxy.source, "proxy-book entry 'treasury'");
        let raw = names.resolve_address_ref("dest_ss58", ALICE)?;
        assert_eq!(raw.address, ALICE);
        assert_eq!(raw.source, "ss58 address");
        assert_eq!(raw.name.as_deref(), Some("raw-label"));
        assert!(
            lookup
                .calls
                .lock()
                .map(|calls| calls.is_empty())
                .unwrap_or(false)
        );
        Ok(())
    }

    #[test]
    fn saved_multisig_aliases_deduplicate_before_core_derivation_matching_recorded_golden()
    -> Result<(), Box<dyn Error>> {
        // vendor/upstream/golden.json multisig[0], recorded by the upstream
        // native codec. Aliases repeat Bob and deliberately reverse key order.
        let books = Books::from_values(
            json!([{"name":"contact-bob","address":BOB}]),
            json!([{"name":"proxy-alice","address":ALICE}]),
            json!([{"name":"treasury","threshold":2,"signatories":["contact-bob","proxy-alice","bob"]}]),
        );
        let lookup = Lookup::default();
        let names = NameResolver::new(&books, &lookup, "treasury", "worker");
        let expected = "5F3QVbS78a4aTYLiRAD8N3czjqVoNyV42L19CXyhqUMCh4Ch";
        let explicit = names.resolve_address_ref("new_coldkey", "treasury")?;
        assert_eq!(explicit.address, expected);
        assert_eq!(explicit.source, "saved multisig 'treasury'");
        let default = names.resolve_address("coldkey_ss58", None)?;
        assert_eq!(
            default.as_ref().map(|value| value.address.as_str()),
            Some(expected)
        );
        assert_eq!(
            names.resolve_address_ref("new_hotkey", "treasury")?.address,
            CHARLIE
        );
        assert_eq!(names.saved_multisig_address("absent")?, None);
        Ok(())
    }

    #[test]
    fn defaults_are_canonical_keys_only_and_destinations_never_default()
    -> Result<(), Box<dyn Error>> {
        let books = Books::default();
        let lookup = Lookup::default();
        let names = NameResolver::new(&books, &lookup, "default", "worker");
        for param in [
            "dest_ss58",
            "dest_coldkey_ss58",
            "destination_hotkey_ss58",
            "new_hotkey_ss58",
            "proxy_for",
        ] {
            assert_eq!(names.resolve_address(param, None)?, None, "{param}");
        }
        assert!(
            lookup
                .calls
                .lock()
                .map(|calls| calls.is_empty())
                .unwrap_or(false)
        );
        let coldkey = names.resolve_address("coldkey_ss58", None)?;
        let hotkey = names.resolve_address("hotkey_ss58", None)?;
        assert_eq!(
            coldkey.as_ref().map(|value| value.address.as_str()),
            Some(ALICE)
        );
        assert_eq!(
            hotkey.as_ref().and_then(|value| value.name.as_deref()),
            Some("default/worker")
        );
        Ok(())
    }

    #[test]
    fn hotkey_references_use_rpartition_and_configured_wallet() -> Result<(), Box<dyn Error>> {
        let books = Books::default();
        let lookup = Lookup::default();
        let names = NameResolver::new(&books, &lookup, "alice", "worker");
        for value in ["gpu", "bob/gpu", "/gpu", "bob/nested/gpu"] {
            let resolved = names.resolve_address_ref("origin_hotkey_ss58", value)?;
            assert_eq!(resolved.kind, AccountKind::Hotkey);
            assert_eq!(resolved.name.as_deref(), Some(value));
        }
        let calls = lookup
            .calls
            .lock()
            .map_err(|_| ResolveError::new("test lookup lock"))?;
        assert_eq!(
            &*calls,
            &[
                "hot:alice/gpu",
                "hot:bob/gpu",
                "hot:alice/gpu",
                "hot:bob/nested/gpu"
            ]
        );
        Ok(())
    }

    #[test]
    fn indirect_multisig_cycles_refuse_and_do_not_poison_later_resolutions()
    -> Result<(), Box<dyn Error>> {
        let books = Books::from_values(
            json!([]),
            json!([]),
            json!([
                {"name":"a","threshold":1,"signatories":["b"]},
                {"name":"b","threshold":1,"signatories":["a"]},
                {"name":"valid","threshold":2,"signatories":[ALICE,BOB]},
            ]),
        );
        let lookup = Lookup::default();
        let names = NameResolver::new(&books, &lookup, "default", "worker");
        let error = names
            .saved_multisig_address("a")
            .err()
            .ok_or("cycle unexpectedly accepted")?;
        assert_eq!(
            error.to_string(),
            "multisig 'a' refers to itself through its signatories"
        );
        assert_eq!(
            names.saved_multisig_address("valid")?.as_deref(),
            Some("5F3QVbS78a4aTYLiRAD8N3czjqVoNyV42L19CXyhqUMCh4Ch")
        );
        assert_eq!(names.saved_multisig_address("a").err(), Some(error));
        Ok(())
    }

    #[test]
    fn acyclic_multisig_chains_have_a_bounded_refusal_and_repeated_branches_are_valid()
    -> Result<(), Box<dyn Error>> {
        let lookup = Lookup::default();
        for depth in [MAX_NAME_RESOLUTION_DEPTH, MAX_NAME_RESOLUTION_DEPTH + 1] {
            let entries = (0..depth)
                .map(|index| {
                    let next = if index + 1 == depth {
                        ALICE.to_owned()
                    } else {
                        format!("group{}", index + 1)
                    };
                    json!({"name":format!("group{index}"),"threshold":1,"signatories":[next]})
                })
                .collect::<Vec<_>>();
            let books = Books::from_values(json!([]), json!([]), Value::Array(entries));
            let names = NameResolver::new(&books, &lookup, "alice", "worker");
            let resolved = names.saved_multisig_address("group0");
            if depth == MAX_NAME_RESOLUTION_DEPTH {
                assert!(resolved?.is_some());
            } else {
                assert_eq!(
                    resolved.err().map(|error| error.to_string()),
                    Some(format!(
                        "saved multisig references exceed the resolution depth limit ({MAX_NAME_RESOLUTION_DEPTH})"
                    ))
                );
            }
        }
        let books = Books::from_values(
            json!([]),
            json!([]),
            json!([
                {"name":"member","threshold":1,"signatories":[ALICE]},
                {"name":"parent","threshold":1,"signatories":["member","member"]},
            ]),
        );
        let names = NameResolver::new(&books, &lookup, "alice", "worker");
        assert!(names.saved_multisig_address("parent")?.is_some());
        // A duplicate-child DAG formerly repeated derivation exponentially.
        // The provider count proves completed descendants are reused, without
        // timing assertions or a test that merely inspects internal caching.
        let entries = (0..25).map(|index| {
            let next = if index == 24 { "alice".to_owned() } else { format!("dag{}", index + 1) };
            json!({"name":format!("dag{index}"),"threshold":1,"signatories":[next.clone(),next]})
        }).collect::<Vec<_>>();
        let books = Books::from_values(json!([]), json!([]), Value::Array(entries));
        let counted = Lookup::default();
        let names = NameResolver::new(&books, &counted, "alice", "worker");
        assert!(names.saved_multisig_address("dag0")?.is_some());
        assert_eq!(counted.calls.lock().map_err(|_| "lookup lock")?.len(), 2);
        // A shallow cache hit cannot conceal a later over-deep path to the
        // same subtree; cache entries carry the full descendant height.
        let mut entries = vec![
            json!({"name":"root","threshold":1,"signatories":["leaf","chain0"]}),
            json!({"name":"leaf","threshold":1,"signatories":[ALICE]}),
        ];
        for index in 0..MAX_NAME_RESOLUTION_DEPTH - 1 {
            let next = if index == MAX_NAME_RESOLUTION_DEPTH - 2 {
                "leaf".to_owned()
            } else {
                format!("chain{}", index + 1)
            };
            entries
                .push(json!({"name":format!("chain{index}"),"threshold":1,"signatories":[next]}));
        }
        let books = Books::from_values(json!([]), json!([]), Value::Array(entries));
        assert_eq!(
            NameResolver::new(&books, &lookup, "alice", "worker")
                .saved_multisig_address("root")
                .err()
                .map(|error| error.to_string()),
            Some(multisig_depth_error().to_string())
        );
        Ok(())
    }

    #[test]
    fn malformed_multisig_schema_refuses_offline() {
        let lookup = Lookup::default();
        for entry in [
            json!({"name":"bad","threshold":65536,"signatories":[ALICE]}),
            json!({"name":"bad","threshold":-1,"signatories":[ALICE]}),
            json!({"name":"bad","threshold":2,"signatories":null}),
            json!({"name":"bad","threshold":2,"signatories":[42]}),
            json!({"name":"bad","threshold":2,"signatories":[]}),
        ] {
            let books = Books::from_values(json!([]), json!([]), json!([entry]));
            let names = NameResolver::new(&books, &lookup, "default", "worker");
            assert!(names.saved_multisig_address("bad").is_err());
        }
        assert_eq!(threshold_u16(&json!(" +０_２ ")), Some(2));
        assert_eq!(threshold_u16(&json!(2.9)), Some(2));
    }

    #[test]
    fn split_and_resolve_signatories_preserve_first_address_and_python_whitespace()
    -> Result<(), Box<dyn Error>> {
        let books = Books::from_values(
            json!([{"name":"ali","address":ALICE}]),
            json!([]),
            json!([]),
        );
        let lookup = Lookup::default();
        let names = NameResolver::new(&books, &lookup, "default", "worker");
        assert_eq!(
            names.resolve_signatory_list("  ali,bob, alice\t\u{1c}bob  ")?,
            [ALICE, BOB]
        );
        assert_eq!(split_signatory_refs("a\u{85}b\u{a0},c"), ["a", "b", "c"]);
        assert!(names.resolve_signatory_list(" ,\t").is_err());
        Ok(())
    }

    #[test]
    fn raw_address_check_requires_network_42_but_preserves_book_values()
    -> Result<(), Box<dyn Error>> {
        let alice = ALICE.parse::<AccountId>()?;
        let foreign = alice.to_ss58(0);
        assert!(is_bittensor_address(ALICE));
        assert!(!is_bittensor_address(&foreign));
        assert!(!is_bittensor_address("0x1234"));
        assert!(!is_bittensor_address(
            "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQZ"
        ));
        let books = Books::from_values(
            json!([{"name":"foreign","address":foreign}]),
            json!([]),
            json!([]),
        );
        let lookup = Lookup::default();
        let names = NameResolver::new(&books, &lookup, "default", "worker");
        assert_eq!(
            names.resolve_address_ref("dest_ss58", "foreign")?.address,
            foreign
        );
        Ok(())
    }

    #[test]
    fn first_invalid_or_empty_contact_falls_through_to_proxy_not_later_contact()
    -> Result<(), Box<dyn Error>> {
        let books = Books::from_values(
            json!([{"name":"alice","address":17},{"name":"alice","address":ALICE}]),
            json!([{"name":"alice","address":BOB}]),
            json!([]),
        );
        let lookup = Lookup::default();
        assert_eq!(
            NameResolver::new(&books, &lookup, "default", "worker")
                .resolve_address_ref("coldkey_ss58", "alice")?
                .address,
            BOB
        );
        Ok(())
    }

    #[test]
    fn address_flag_names_drop_terminal_suffix_or_all_embedded_occurrences() {
        for (param, expected) in [
            ("dest_coldkey_ss58", "--dest-coldkey"),
            ("some_ss58_ref", "--some-ref"),
            ("some_ss58_ref_ss58", "--some-ss58-ref"),
            ("proxy_for", "--proxy-for"),
        ] {
            assert_eq!(address_cli_name(param), expected);
        }
        assert!(ss58_param_help("hotkey_ss58").ends_with("Defaults to your wallet's hotkey."));
        assert!(ss58_param_help("coldkey_ss58").ends_with("Defaults to your wallet's coldkey."));
        assert_eq!(
            ss58_param_help("dest_ss58"),
            ss58_param_help("dest_coldkey_ss58")
        );
        assert!(!ss58_param_help("destination_hotkey_ss58").contains("Defaults"));
    }

    #[test]
    fn suggestions_match_sequence_direction_cutoff_ties_and_book_precedence()
    -> Result<(), Box<dyn Error>> {
        // Directional ratios and popular-character behavior come from the
        // documented CPython SequenceMatcher algorithm, not edit distance.
        assert_eq!(sequence_ratio("tide", "diet"), 0.25);
        assert_eq!(sequence_ratio("diet", "tide"), 0.5);
        assert_eq!(sequence_ratio("abcd", "abdc"), 0.75);
        assert_eq!(
            sequence_ratio(&"a".repeat(200), &format!("b{}", "a".repeat(200))),
            0.0
        );
        let books = Books::from_values(
            json!([{"name":"alice","address":BOB},{"name":"abx","address":ALICE},{"name":"aby","address":BOB}]),
            json!([{"name":"alice","address":CHARLIE}]),
            json!([{"name":"multisig-only","threshold":2,"signatories":[ALICE,BOB]}]),
        );
        let lookup = Lookup::default();
        let names = NameResolver::new(&books, &lookup, "default", "worker");
        let suggestion = names.closest_name("alcie").ok_or("missing suggestion")?;
        assert_eq!(suggestion.address, BOB);
        assert_eq!(suggestion.source, "address-book entry");
        assert_eq!(
            names
                .closest_name("abz")
                .as_ref()
                .map(|value| value.name.as_str()),
            Some("aby")
        );
        assert_eq!(names.closest_name("zzzzzz"), None);
        assert_eq!(names.closest_name("multisig-only"), None);
        Ok(())
    }

    struct TempDir(PathBuf);
    impl TempDir {
        fn new() -> Result<Self, Box<dyn Error>> {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "rtao-cli-books-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path)?;
            Ok(Self(path))
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            // No processes, services, configuration or venvs depend on this
            // exact directory: these fixtures contain only public JSON files.
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[tokio::test]
    async fn book_reader_honors_path_overrides_and_never_writes_corruption_backups()
    -> Result<(), Box<dyn Error>> {
        let dir = TempDir::new()?;
        let addresses = dir.0.join("custom-addresses.json");
        let proxies = dir.0.join("custom-proxies.json");
        let multisigs = dir.0.join("custom-multisigs.json");
        fs::write(
            &addresses,
            serde_json::to_vec(&json!([{"name":"alice","address":ALICE,"custom":true}]))?,
        )?;
        fs::write(&proxies, b"{corrupt")?;
        fs::write(&multisigs, b"{\"not\":\"a list\"}")?;
        let paths = Paths::new(&dir.0, |key| match key {
            "BTCLI_ADDRESSES_PATH" => Some(addresses.clone().into_os_string()),
            "BTCLI_PROXIES_PATH" => Some(proxies.clone().into_os_string()),
            "BTCLI_MULTISIGS_PATH" => Some(multisigs.clone().into_os_string()),
            _ => None,
        });
        let books = Books::load(&paths).await?;
        assert_eq!(
            books.addresses,
            [json!({"name":"alice","address":ALICE,"custom":true})]
        );
        assert!(books.proxies.is_empty());
        assert!(books.multisigs.is_empty());
        assert_eq!(fs::read(&proxies)?, b"{corrupt");
        assert_eq!(fs::read_dir(&dir.0)?.count(), 3);
        fs::remove_file(&addresses)?;
        assert!(Books::load(&paths).await?.addresses.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn oversized_books_refuse_instead_of_redirecting_names_to_wallet_fallback()
    -> Result<(), Box<dyn Error>> {
        let dir = TempDir::new()?;
        let address_path = dir.0.join("oversized-addresses.json");
        // This is valid JSON with a same-name wallet/contact collision, so an
        // empty-book fallback would otherwise choose Alice instead of Bob.
        let bytes = serde_json::to_vec(
            &json!([{"name":"alice","address":BOB,"padding":"x".repeat(MAX_FILE_BYTES as usize)}]),
        )?;
        let expected_size = bytes.len() as u64;
        fs::write(&address_path, bytes)?;
        let paths = Paths::new(&dir.0, |key| {
            (key == "BTCLI_ADDRESSES_PATH").then(|| address_path.clone().into_os_string())
        });
        let error = Books::load(&paths)
            .await
            .err()
            .ok_or("oversized book unexpectedly accepted")?;
        assert_eq!(
            error.to_string(),
            "book or public keyfile exceeds the 16 MiB input limit"
        );
        assert!(
            !error
                .to_string()
                .contains(&dir.0.to_string_lossy().to_string())
        );
        assert!(expected_size > MAX_FILE_BYTES);
        assert_eq!(fs::metadata(&address_path)?.len(), expected_size);
        assert_eq!(fs::read_dir(&dir.0)?.count(), 1);
        Ok(())
    }

    #[test]
    fn public_wallet_adapter_reads_companions_and_skips_non_hotkey_artifacts()
    -> Result<(), Box<dyn Error>> {
        let dir = TempDir::new()?;
        let wallet = dir.0.join("alice");
        fs::create_dir_all(wallet.join("hotkeys"))?;
        fs::write(
            wallet.join("coldkeypub.txt"),
            serde_json::to_vec(&json!({"ss58Address":ALICE,"cryptoType":1}))?,
        )?;
        fs::write(
            wallet.join("hotkeys/worker"),
            serde_json::to_vec(
                &json!({"ss58Address":CHARLIE,"cryptoType":1,"secretPhrase":"public-reader poison fixture","secretSeed":"ignored seed fixture","privateKey":{"nested":["ignored key fixture"]}}),
            )?,
        )?;
        fs::write(wallet.join("hotkeys/locked"), b"$NACL-not-a-valid-envelope")?;
        fs::write(
            wallet.join("hotkeys/lockedpub.txt"),
            serde_json::to_vec(&json!({"ss58Address":BOB,"cryptoType":1}))?,
        )?;
        fs::write(wallet.join("hotkeys/worker.lock"), b"artifact")?;
        let accounts = FsAccountLookup::new(&dir.0)?;
        assert_eq!(accounts.coldkey("alice")?, ALICE);
        assert_eq!(accounts.hotkey("alice", "worker")?, CHARLIE);
        assert_eq!(accounts.hotkey("alice", "locked")?, BOB);
        assert_eq!(
            fs::read(wallet.join("hotkeys/locked"))?,
            b"$NACL-not-a-valid-envelope"
        );
        let candidates = accounts.candidates();
        assert_eq!(candidates.len(), 2);
        assert!(
            candidates
                .iter()
                .any(|candidate| candidate.name == "alice" && candidate.address == ALICE)
        );
        assert!(
            candidates
                .iter()
                .any(|candidate| candidate.name == "alice/worker" && candidate.address == CHARLIE)
        );
        assert!(accounts.coldkey("../outside").is_err());
        let error = accounts
            .coldkey("missing")
            .err()
            .ok_or("missing wallet resolved")?;
        assert!(
            !error
                .to_string()
                .contains(&dir.0.to_string_lossy().to_string())
        );
        let duplicate = wallet.join("hotkeys/duplicate");
        fs::write(
            &duplicate,
            format!(
                "{{\"ss58Address\":\"{ALICE}\",\"ss58Address\":\"{BOB}\",\"secretPhrase\":\"ignored fixture\"}}"
            ),
        )?;
        assert_eq!(stored_address(&duplicate).as_deref(), Some(BOB));
        fs::write(
            &duplicate,
            format!("{{\"ss58Address\":\"{ALICE}\",\"ss58Address\":17}}"),
        )?;
        assert_eq!(stored_address(&duplicate), None);
        Ok(())
    }
}
