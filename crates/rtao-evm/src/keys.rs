// Derived from RaoFoundation/subtensor sdk/python/bittensor/evm/keys.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Encrypted Ethereum V3 keystores beside the wallet's native hotkeys.
//!
//! These synchronous operations perform filesystem work and bounded key
//! derivation. Async applications should call them on a blocking worker.
//! Password interaction belongs to the caller; this module never reads stdin.
//!
//! V3 imports accept scrypt and PBKDF2-HMAC-SHA256 with bounded parameters:
//! the scrypt table is at most 256 MiB, `N*r*p` at most 2^24, and PBKDF2 at
//! most two million rounds. Excessive parameters fail before key derivation.

use crate::{
    Error, Result,
    addresses::{h160_to_ss58, normalize_h160},
};
use aes::cipher::{KeyIvInit, StreamCipher};
use alloy_primitives::keccak256;
use alloy_signer_local::{
    MnemonicBuilder, PrivateKeySigner,
    coins_bip39::{
        ChineseSimplified, ChineseTraditional, Czech, English, French, Italian, Japanese, Korean,
        Portuguese, Spanish, Wordlist,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::Sha256;
#[cfg(unix)]
use std::fs::File;
use std::{
    fmt,
    fs::{self, OpenOptions},
    io::Write,
    path::{Component, Path, PathBuf},
};
use subtle::ConstantTimeEq;
use unicode_normalization::UnicodeNormalization;
use zeroize::Zeroizing;

/// A local secp256k1 signing account. Its upstream `Debug` omits the private key.
pub type EvmAccount = PrivateKeySigner;

/// BIP-44 path used by Python's Ethereum mnemonic imports.
pub const ETH_DERIVATION_PATH: &str = "m/44'/60'/0'/0/0";
/// Maximum encoded keystore size, shared with the native-wallet file reader.
pub const MAX_KEYSTORE_BYTES: usize = 16 * 1024 * 1024;
const MAX_PASSWORD_BYTES: usize = 1024 * 1024;
const MAX_SCRYPT_MEMORY: u64 = 256 * 1024 * 1024;
const MAX_SCRYPT_WORK: u64 = 1 << 24;
const MAX_PBKDF2_ROUNDS: u64 = 2_000_000;
const STORE_SCRYPT_N: u64 = 1 << 18;

/// Public metadata readable without decrypting an EVM key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvmKeyInfo {
    /// The key's filename within `evmkeys`.
    pub name: String,
    /// Lowercase, `0x`-prefixed Ethereum address.
    pub address: String,
    /// Native SS58 mirror credited by the runtime's hashed address mapping.
    pub ss58_mirror: String,
    /// Exact expanded keyfile path.
    pub path: PathBuf,
}

/// Destination and password inputs for key creation, import and unlock.
///
/// An empty explicit password falls through to `BT_WALLET_PASSWORD`, matching
/// Python. An optional caller resolver is consulted last.
#[derive(Clone)]
pub struct EvmKeyOptions {
    /// Filename; it must be a single ordinary path component.
    pub name: String,
    /// Wallet directory name; it must be a single ordinary path component.
    pub wallet_name: String,
    /// Wallet root; a leading `~/` is expanded using `HOME`.
    pub wallet_path: PathBuf,
    /// Explicit encryption/decryption password, erased when dropped.
    pub password: Option<Zeroizing<String>>,
    /// Allow atomic replacement of an existing regular keyfile.
    pub overwrite: bool,
}
impl Default for EvmKeyOptions {
    fn default() -> Self {
        Self {
            name: "default".into(),
            wallet_name: "default".into(),
            wallet_path: "~/.bittensor/wallets".into(),
            password: None,
            overwrite: false,
        }
    }
}
impl fmt::Debug for EvmKeyOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EvmKeyOptions")
            .field("name", &self.name)
            .field("wallet_name", &self.wallet_name)
            .field("wallet_path", &self.wallet_path)
            .field("password", &self.password.as_ref().map(|_| "[REDACTED]"))
            .field("overwrite", &self.overwrite)
            .finish()
    }
}

/// Password fallback supplied explicitly by an application.
pub trait PasswordResolver: Send + Sync {
    /// Return a nonempty password. `confirm` requests the application's
    /// confirmation flow when encrypting a newly stored key.
    fn resolve(&self, confirm: bool) -> Result<Zeroizing<String>>;
}
impl<F> PasswordResolver for F
where
    F: Fn(bool) -> Result<Zeroizing<String>> + Send + Sync,
{
    fn resolve(&self, confirm: bool) -> Result<Zeroizing<String>> {
        self(confirm)
    }
}

/// Exactly one borrowed source for importing an EVM account.
pub enum ImportSource<'a> {
    /// A 32-byte private key encoded as 64 hex digits, optionally `0x`-prefixed.
    PrivateKey(&'a str),
    /// Standard Ethereum V3 JSON and an optional distinct unlocking password.
    Keystore {
        /// Encrypted JSON; limited to [`MAX_KEYSTORE_BYTES`].
        json: &'a str,
        /// Password for this source, falling back to the destination password.
        password: Option<&'a str>,
    },
    /// BIP-39 words in any standard language and an optional path override.
    Mnemonic {
        /// Recovery phrase; never included in library diagnostics.
        phrase: &'a str,
        /// Defaults to [`ETH_DERIVATION_PATH`].
        derivation_path: Option<&'a str>,
    },
}
impl fmt::Debug for ImportSource<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::PrivateKey(_) => "PrivateKey([REDACTED])",
            Self::Keystore { .. } => "Keystore([REDACTED])",
            Self::Mnemonic { .. } => "Mnemonic([REDACTED])",
        })
    }
}

/// Resolve a safe EVM keyfile path beneath the selected wallet.
pub fn keyfile_path(name: &str, wallet_name: &str, wallet_path: &Path) -> Result<PathBuf> {
    validate_name(name)?;
    Ok(evm_dir(wallet_name, wallet_path)?.join(name))
}

/// List valid public key metadata in filename order, without passwords.
/// Malformed or nonregular entries are omitted, matching Python's listing.
pub fn list_evm_keys(wallet_name: &str, wallet_path: &Path) -> Result<Vec<EvmKeyInfo>> {
    let directory = evm_dir(wallet_name, wallet_path)?;
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(io_error(&directory, error)),
    };
    let mut paths = entries
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    paths.sort();
    Ok(paths
        .into_iter()
        .filter_map(|path| info(&path).ok())
        .collect())
}

/// Read public metadata for a named key or report available key names.
pub fn get_evm_key_info(name: &str, wallet_name: &str, wallet_path: &Path) -> Result<EvmKeyInfo> {
    let path = keyfile_path(name, wallet_name, wallet_path)?;
    if let Ok(info) = info(&path) {
        return Ok(info);
    }
    let available = list_evm_keys(wallet_name, wallet_path)?
        .into_iter()
        .map(|info| info.name)
        .collect::<Vec<_>>();
    let listing = if available.is_empty() {
        String::new()
    } else {
        format!(" Available: {}.", available.join(", "))
    };
    Err(Error::Validation(format!(
        "no EVM key {name:?} in wallet {wallet_name:?}.{listing}"
    )))
}

/// Generate a fresh random account and store an encrypted V3 keystore.
/// The account is independent of the wallet's native coldkey and hotkeys.
pub fn create_evm_key(
    options: &EvmKeyOptions,
    resolver: Option<&dyn PasswordResolver>,
) -> Result<EvmKeyInfo> {
    store(&EvmAccount::random(), options, resolver)
}

/// Import a raw key, mnemonic or encrypted keystore, reencrypting for storage.
pub fn import_evm_key(
    source: ImportSource<'_>,
    options: &EvmKeyOptions,
    resolver: Option<&dyn PasswordResolver>,
) -> Result<EvmKeyInfo> {
    let account = match source {
        ImportSource::PrivateKey(value) => account_from_private_key(value)?,
        ImportSource::Mnemonic {
            phrase,
            derivation_path,
        } => account_from_mnemonic(phrase, derivation_path.unwrap_or(ETH_DERIVATION_PATH))?,
        ImportSource::Keystore { json, password } => {
            let document = parse_keystore(json.as_bytes())?;
            let password = match password.filter(|password| !password.is_empty()) {
                Some(password) => checked_password(password.to_owned())?,
                None => resolve_password(options, resolver, false)?,
            };
            decrypt_keystore(&document, &password)?
        }
    };
    store(&account, options, resolver)
}

/// Decrypt the selected keyfile into a local signing account.
pub fn unlock_evm_key(
    options: &EvmKeyOptions,
    resolver: Option<&dyn PasswordResolver>,
) -> Result<EvmAccount> {
    let path = keyfile_path(&options.name, &options.wallet_name, &options.wallet_path)?;
    let document = read_document(&path)?;
    let password = resolve_password(options, resolver, false)?;
    decrypt_keystore(&document, &password)
}

/// Export encrypted JSON without decrypting or exposing the private scalar.
/// Unknown keystore metadata is preserved in the returned JSON value.
pub fn export_evm_key(name: &str, wallet_name: &str, wallet_path: &Path) -> Result<Value> {
    read_document(&keyfile_path(name, wallet_name, wallet_path)?)
}

/// Atomically publish V3 JSON with owner-only file permissions on Unix.
/// Existing files require `overwrite`; symlink destinations and symlink
/// directory ancestors are refused. New directories use mode `0700`.
pub fn write_keystore_file(path: &Path, keystore: &Value, overwrite: bool) -> Result<()> {
    validate_keystore(keystore)?;
    let mut content = BoundedKeystore(Vec::new());
    serde_json::to_writer_pretty(&mut content, keystore)?;
    content.0.push(b'\n');
    atomic_write(path, &content.0, overwrite)
}

struct BoundedKeystore(Vec<u8>);
impl Write for BoundedKeystore {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.0.len().saturating_add(bytes.len()) >= MAX_KEYSTORE_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "keystore exceeds 16 MiB limit",
            ));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn evm_dir(wallet_name: &str, wallet_path: &Path) -> Result<PathBuf> {
    validate_name(wallet_name)?;
    let mut components = wallet_path.components();
    let path = if components.next() == Some(Component::Normal("~".as_ref())) {
        let home = std::env::var_os("HOME")
            .ok_or_else(|| validation("HOME is required to expand wallet path"))?;
        PathBuf::from(home).join(components.as_path())
    } else {
        wallet_path.to_owned()
    };
    Ok(path.join(wallet_name).join("evmkeys"))
}
fn validate_name(name: &str) -> Result<()> {
    let mut components = Path::new(name).components();
    if name.is_empty()
        || name.contains(['/', '\\', '\0'])
        || !matches!(components.next(), Some(Component::Normal(_)))
        || components.next().is_some()
        || name == "."
        || name == ".."
    {
        return Err(validation(
            "wallet and EVM key names must be single ordinary path components",
        ));
    }
    Ok(())
}
fn info(path: &Path) -> Result<EvmKeyInfo> {
    let document = read_document(path)?;
    let address = normalize_h160(string(&document, "address")?)?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| validation("EVM key filename is not valid UTF-8"))?
        .to_owned();
    Ok(EvmKeyInfo {
        name,
        ss58_mirror: h160_to_ss58(&address)?,
        address,
        path: path.to_owned(),
    })
}
fn read_document(path: &Path) -> Result<Value> {
    let bytes = rtao_wallet::read_keyfile_data(path).map_err(|error| match error {
        rtao_wallet::WalletError::Io { error, .. } => io_error(path, error),
        _ => validation("could not read bounded regular EVM keystore"),
    })?;
    parse_keystore(&bytes)
}
fn parse_keystore(bytes: &[u8]) -> Result<Value> {
    if bytes.len() > MAX_KEYSTORE_BYTES {
        return Err(validation("keystore exceeds 16 MiB limit"));
    }
    let value = serde_json::from_slice(bytes)?;
    validate_keystore(&value)?;
    Ok(value)
}
fn validate_keystore(value: &Value) -> Result<()> {
    if value.get("version").and_then(Value::as_u64) != Some(3) {
        return Err(validation("expected Ethereum keystore version 3"));
    }
    let crypto = crypto(value)?;
    if string(crypto, "cipher")? != "aes-128-ctr" {
        return Err(validation("unsupported keystore cipher"));
    }
    decode_fixed::<16>(string(field(crypto, "cipherparams")?, "iv")?)?;
    decode_fixed::<32>(string(crypto, "ciphertext")?)?;
    decode_fixed::<32>(string(crypto, "mac")?)?;
    let _ = kdf_spec(crypto)?;
    Ok(())
}
fn crypto(value: &Value) -> Result<&Value> {
    value
        .get("crypto")
        .or_else(|| value.get("Crypto"))
        .ok_or_else(|| validation("missing keystore crypto object"))
}
fn field<'a>(value: &'a Value, name: &str) -> Result<&'a Value> {
    value
        .get(name)
        .ok_or_else(|| validation("missing required keystore field"))
}
fn string<'a>(value: &'a Value, name: &str) -> Result<&'a str> {
    field(value, name)?
        .as_str()
        .ok_or_else(|| validation("keystore field must be a string"))
}
fn integer(value: &Value, name: &str) -> Result<u64> {
    field(value, name)?
        .as_u64()
        .ok_or_else(|| validation("keystore parameter must be an unsigned integer"))
}
fn validation(message: &str) -> Error {
    Error::Validation(message.into())
}
fn io_error(path: &Path, source: std::io::Error) -> Error {
    Error::Io {
        path: path.to_owned(),
        source,
    }
}
fn decode_fixed<const N: usize>(value: &str) -> Result<[u8; N]> {
    let mut bytes = [0u8; N];
    hex::decode_to_slice(value, &mut bytes)
        .map_err(|_| validation("invalid keystore hex field length or digits"))?;
    Ok(bytes)
}

enum KdfSpec {
    Scrypt {
        params: scrypt::Params,
        salt: Vec<u8>,
    },
    Pbkdf2 {
        rounds: u32,
        salt: Vec<u8>,
    },
}
fn kdf_spec(crypto: &Value) -> Result<KdfSpec> {
    let parameters = field(crypto, "kdfparams")?;
    if !(32..=1024).contains(&integer(parameters, "dklen")?) {
        return Err(validation("keystore dklen must be between 32 and 1024"));
    }
    let salt_hex = string(parameters, "salt")?;
    if salt_hex.is_empty() || salt_hex.len() > 2048 || !salt_hex.len().is_multiple_of(2) {
        return Err(validation("keystore salt must contain 1 to 1024 bytes"));
    }
    let salt = hex::decode(salt_hex).map_err(|_| validation("invalid keystore salt"))?;
    match string(crypto, "kdf")? {
        "scrypt" => {
            let n = integer(parameters, "n")?;
            let r = integer(parameters, "r")?;
            let p = integer(parameters, "p")?;
            let memory = n.checked_mul(r).and_then(|value| value.checked_mul(128));
            let work = n.checked_mul(r).and_then(|value| value.checked_mul(p));
            if n < 2
                || !n.is_power_of_two()
                || r == 0
                || r > 32
                || p == 0
                || p > 16
                || memory.is_none_or(|bytes| bytes > MAX_SCRYPT_MEMORY)
                || work.is_none_or(|work| work > MAX_SCRYPT_WORK)
            {
                return Err(validation(
                    "keystore scrypt parameters exceed resource limits",
                ));
            }
            let params = scrypt::Params::new(n.ilog2() as u8, r as u32, p as u32)
                .map_err(|_| validation("invalid keystore scrypt parameters"))?;
            Ok(KdfSpec::Scrypt { params, salt })
        }
        "pbkdf2" => {
            let rounds = integer(parameters, "c")?;
            if rounds == 0
                || rounds > MAX_PBKDF2_ROUNDS
                || string(parameters, "prf")? != "hmac-sha256"
            {
                return Err(validation(
                    "unsupported or excessive keystore PBKDF2 parameters",
                ));
            }
            Ok(KdfSpec::Pbkdf2 {
                rounds: rounds as u32,
                salt,
            })
        }
        _ => Err(validation("unsupported keystore KDF")),
    }
}
fn derive_key(crypto: &Value, password: &str) -> Result<Zeroizing<[u8; 32]>> {
    if password.len() > MAX_PASSWORD_BYTES {
        return Err(validation("keystore password exceeds limit"));
    }
    // V3 only consumes the first 32 bytes, even when dklen is larger. Both
    // supported KDFs are prefix stable, so unused key bytes need not be derived.
    let mut key = Zeroizing::new([0u8; 32]);
    match kdf_spec(crypto)? {
        KdfSpec::Scrypt { params, salt } => {
            scrypt::scrypt(password.as_bytes(), &salt, &params, key.as_mut())
                .map_err(|_| Error::Crypto("keystore key derivation failed".into()))?
        }
        KdfSpec::Pbkdf2 { rounds, salt } => {
            pbkdf2::pbkdf2_hmac::<Sha256>(password.as_bytes(), &salt, rounds, key.as_mut())
        }
    }
    Ok(key)
}
fn decrypt_keystore(document: &Value, password: &str) -> Result<EvmAccount> {
    validate_keystore(document)?;
    let crypto = crypto(document)?;
    let derived = derive_key(crypto, password)?;
    let mut plain = Zeroizing::new(decode_fixed::<32>(string(crypto, "ciphertext")?)?);
    let expected_mac = decode_fixed::<32>(string(crypto, "mac")?)?;
    let actual_mac = keystore_mac(&derived, &plain);
    if !bool::from(actual_mac.ct_eq(&expected_mac)) {
        return Err(Error::Crypto(
            "could not unlock EVM keystore: password or authentication failed".into(),
        ));
    }
    let iv = decode_fixed::<16>(string(field(crypto, "cipherparams")?, "iv")?)?;
    apply_aes_ctr(&derived[..16], iv, &mut plain)?;
    let account = EvmAccount::from_slice(plain.as_ref())
        .map_err(|_| Error::Crypto("keystore contains an invalid secp256k1 private key".into()))?;
    // Address metadata is not covered by V3's MAC. Refuse a misleading label
    // rather than silently signing as a different account after unlock.
    if let Some(address) = document.get("address") {
        let address = address
            .as_str()
            .ok_or_else(|| validation("keystore address must be a string"))?;
        if normalize_h160(address)? != account_address(&account) {
            return Err(validation(
                "keystore address does not match decrypted account",
            ));
        }
    }
    Ok(account)
}
fn keystore_mac(derived: &[u8; 32], cipher: &[u8; 32]) -> [u8; 32] {
    let mut input = Zeroizing::new([0u8; 48]);
    input[..16].copy_from_slice(&derived[16..]);
    input[16..].copy_from_slice(cipher);
    keccak256(&input[..]).0
}
fn apply_aes_ctr(key: &[u8], iv: [u8; 16], bytes: &mut [u8; 32]) -> Result<()> {
    // Ctr128BE adds an independent block counter to the initial nonce modulo
    // 2^128, including Ethereum's published `evilnonce` fixture.
    let mut cipher = ctr::Ctr128BE::<aes::Aes128>::new_from_slices(key, &iv)
        .map_err(|_| Error::Crypto("invalid AES key or nonce length".into()))?;
    cipher
        .try_apply_keystream(bytes)
        .map_err(|_| Error::Crypto("AES counter exhausted".into()))
}

fn account_from_private_key(value: &str) -> Result<EvmAccount> {
    let value = value.trim();
    let value = value.strip_prefix("0x").unwrap_or(value);
    let mut bytes = Zeroizing::new([0u8; 32]);
    hex::decode_to_slice(value, bytes.as_mut())
        .map_err(|_| validation("private key must contain exactly 32 hex-encoded bytes"))?;
    EvmAccount::from_slice(bytes.as_ref()).map_err(|_| validation("invalid secp256k1 private key"))
}
fn account_from_mnemonic(phrase: &str, path: &str) -> Result<EvmAccount> {
    if phrase.len() > 4096 || path.len() > 1024 || path.split('/').count() > 64 {
        return Err(validation("mnemonic or derivation path exceeds limit"));
    }
    // Coins accepts already-hardened integer values and adds 2^31 for a
    // hardened suffix. Validate indices before that arithmetic can overflow
    // or an unmarked child can alias a hardened child.
    let path = normalized_derivation_path(path)?;
    // BIP-39 uses NFKD, including Japanese ideographic word separators.
    // Count first so String growth cannot discard unprotected secret prefixes.
    let normalized_len: usize = phrase.nfkd().map(char::len_utf8).sum();
    let mut expanded = Zeroizing::new(String::with_capacity(normalized_len));
    expanded.extend(phrase.nfkd());
    let mut normalized = Zeroizing::new(String::with_capacity(expanded.len()));
    for word in expanded.split_whitespace() {
        if !normalized.is_empty() {
            normalized.push(' ');
        }
        normalized.push_str(word);
    }
    if ![12, 15, 18, 21, 24].contains(&normalized.split(' ').count()) {
        return Err(validation("invalid Ethereum recovery phrase word count"));
    }
    // Check the canonical public path against the upstream parser as well.
    let _ = MnemonicBuilder::<English>::default()
        .derivation_path(&path)
        .map_err(|_| validation("invalid Ethereum derivation path"))?;
    macro_rules! try_languages {
        ($($language:ty),+ $(,)?) => {$(
            if let Some(Ok(account)) = try_mnemonic::<$language>(&normalized, &path) {
                return Ok(account);
            }
        )+};
    }
    try_languages!(
        English,
        ChineseSimplified,
        ChineseTraditional,
        Czech,
        French,
        Italian,
        Japanese,
        Korean,
        Portuguese,
        Spanish
    );
    Err(validation("invalid Ethereum recovery phrase or checksum"))
}

fn normalized_derivation_path(path: &str) -> Result<String> {
    let invalid = || validation("invalid Ethereum derivation path");
    let mut components = path.split('/');
    if !matches!(components.next(), Some("m" | "M")) {
        return Err(invalid());
    }
    let mut normalized = String::with_capacity(path.len());
    normalized.push('m');
    for component in components {
        let (digits, hardened) = component
            .strip_suffix(['\'', 'h', 'H'])
            .map(|digits| (digits, true))
            .unwrap_or((component, false));
        if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(invalid());
        }
        let index: u32 = digits.parse().map_err(|_| invalid())?;
        if index >= (1 << 31) {
            return Err(invalid());
        }
        normalized.push('/');
        normalized.push_str(digits);
        if hardened {
            normalized.push('\'');
        }
    }
    Ok(normalized)
}
fn try_mnemonic<W: Wordlist>(phrase: &str, path: &str) -> Option<Result<EvmAccount>> {
    if !phrase.split(' ').all(|word| W::get_all().contains(&word)) {
        return None;
    }
    // Alloy's enabled mnemonic feature erases the builder's owned phrase and
    // password on drop. Coins' entropy/seed internals remain an upstream
    // boundary. Never format upstream errors: some contain recovery words.
    Some(
        MnemonicBuilder::<W>::default()
            .phrase(phrase)
            .derivation_path(path)
            .and_then(|builder| builder.build())
            .map_err(|_| validation("invalid Ethereum recovery phrase or checksum")),
    )
}
fn account_address(account: &EvmAccount) -> String {
    format!("0x{}", hex::encode(account.address().as_slice()))
}
fn resolve_password(
    options: &EvmKeyOptions,
    resolver: Option<&dyn PasswordResolver>,
    confirm: bool,
) -> Result<Zeroizing<String>> {
    resolve_password_with(options, resolver, confirm, || {
        std::env::var("BT_WALLET_PASSWORD").ok()
    })
}
fn resolve_password_with(
    options: &EvmKeyOptions,
    resolver: Option<&dyn PasswordResolver>,
    confirm: bool,
    environment: impl FnOnce() -> Option<String>,
) -> Result<Zeroizing<String>> {
    if let Some(password) = options
        .password
        .as_ref()
        .filter(|password| !password.is_empty())
    {
        return checked_password(password.to_string());
    }
    if let Some(password) = environment().filter(|password| !password.is_empty()) {
        return checked_password(password);
    }
    let password = resolver
        .ok_or_else(|| {
            validation("no EVM password available: pass one explicitly or set BT_WALLET_PASSWORD")
        })?
        .resolve(confirm)?;
    if password.is_empty() || password.len() > MAX_PASSWORD_BYTES {
        return Err(validation("empty or excessive EVM password"));
    }
    Ok(password)
}
fn checked_password(password: String) -> Result<Zeroizing<String>> {
    let password = Zeroizing::new(password);
    if password.is_empty() || password.len() > MAX_PASSWORD_BYTES {
        return Err(validation("empty or excessive EVM password"));
    }
    Ok(password)
}
fn store(
    account: &EvmAccount,
    options: &EvmKeyOptions,
    resolver: Option<&dyn PasswordResolver>,
) -> Result<EvmKeyInfo> {
    let path = keyfile_path(&options.name, &options.wallet_name, &options.wallet_path)?;
    preflight(&path, options.overwrite)?;
    let password = resolve_password(options, resolver, true)?;
    let keystore = encrypt_keystore(account, &password, STORE_SCRYPT_N)?;
    write_keystore_file(&path, &keystore, options.overwrite)?;
    info(&path)
}
fn encrypt_keystore(account: &EvmAccount, password: &str, n: u64) -> Result<Value> {
    let mut salt = [0u8; 32];
    let mut iv = [0u8; 16];
    let mut id = [0u8; 16];
    for bytes in [&mut salt[..], &mut iv[..], &mut id[..]] {
        getrandom::fill(bytes)
            .map_err(|_| Error::Crypto("operating-system randomness unavailable".into()))?;
    }
    id[6] = (id[6] & 0x0f) | 0x40;
    id[8] = (id[8] & 0x3f) | 0x80;
    let id = hex::encode(id);
    let mut crypto = json!({
        "cipher": "aes-128-ctr", "cipherparams": { "iv": hex::encode(iv) },
        "kdf": "scrypt", "kdfparams": { "dklen": 32, "n": n, "r": 8, "p": 1, "salt": hex::encode(salt) }
    });
    let derived = derive_key(&crypto, password)?;
    let scalar = Zeroizing::new(account.to_field_bytes());
    let mut cipher = Zeroizing::new([0u8; 32]);
    cipher.copy_from_slice(&scalar[..]);
    apply_aes_ctr(&derived[..16], iv, &mut cipher)?;
    crypto["ciphertext"] = Value::String(hex::encode(&cipher[..]));
    crypto["mac"] = Value::String(hex::encode(keystore_mac(&derived, &cipher)));
    Ok(json!({ "version": 3,
        "id": format!("{}-{}-{}-{}-{}", &id[..8], &id[8..12], &id[12..16], &id[16..20], &id[20..]),
        "address": &account_address(account)[2..], "crypto": crypto
    }))
}

fn preflight(path: &Path, overwrite: bool) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => Err(
            validation("refusing nonregular or symlink EVM keystore destination"),
        ),
        Ok(_) if !overwrite => Err(validation(
            "EVM key already exists; enable overwrite to replace it",
        )),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_error(path, error)),
    }
}
fn private_dirs(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty() {
        return Ok(());
    }
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(validation(
                "refusing non-directory or symlink EVM keystore ancestor",
            ));
        }
        Ok(_) => return Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(io_error(path, error)),
    }
    if let Some(parent) = path.parent() {
        private_dirs(parent)?;
    }
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    match builder.create(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let metadata = fs::symlink_metadata(path).map_err(|error| io_error(path, error))?;
            if metadata.is_dir() && !metadata.file_type().is_symlink() {
                Ok(())
            } else {
                Err(validation(
                    "refusing non-directory or symlink EVM keystore ancestor",
                ))
            }
        }
        Err(error) => Err(io_error(path, error)),
    }
}
struct TemporaryFile(PathBuf);
impl Drop for TemporaryFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
fn atomic_write(path: &Path, content: &[u8], overwrite: bool) -> Result<()> {
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or_else(|| validation("EVM keystore destination needs a parent directory"))?;
    for ancestor in parent
        .ancestors()
        .filter(|path| !path.as_os_str().is_empty())
    {
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(validation("refusing symlink EVM keystore ancestor"));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(io_error(ancestor, error)),
        }
    }
    private_dirs(parent)?;
    preflight(path, overwrite)?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut created = None;
    for _ in 0..32 {
        let mut nonce = [0u8; 16];
        getrandom::fill(&mut nonce)
            .map_err(|_| Error::Crypto("operating-system randomness unavailable".into()))?;
        let temporary = parent.join(format!(".rtao-evm-{}.tmp", hex::encode(nonce)));
        match options.open(&temporary) {
            Ok(file) => {
                created = Some((TemporaryFile(temporary), file));
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(io_error(&temporary, error)),
        }
    }
    let (temporary, mut file) =
        created.ok_or_else(|| validation("could not create EVM keystore temporary"))?;
    file.write_all(content)
        .map_err(|error| io_error(&temporary.0, error))?;
    file.sync_all()
        .map_err(|error| io_error(&temporary.0, error))?;
    drop(file);
    if overwrite {
        fs::rename(&temporary.0, path).map_err(|error| io_error(path, error))?;
    } else {
        fs::hard_link(&temporary.0, path).map_err(|error| io_error(path, error))?;
    }
    #[cfg(unix)]
    {
        File::open(parent)
            .and_then(|file| file.sync_all())
            .map_err(|error| io_error(parent, error))?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "keys/tests.rs"]
mod tests;
