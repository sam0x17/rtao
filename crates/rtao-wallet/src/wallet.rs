// Derived from RaoFoundation/subtensor sdk/python/bittensor/wallet.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! The btcli directory layout and explicit creation/regeneration operations.

use crate::{
    ColdkeyFile, HotkeyFile, Overwrite, PasswordSources, PublicKeyInfo, WalletError, WriteOptions,
    files::{atomic_write, public_account},
    signer_error,
};
use rtao_core::keyfile_json::{SecretInput, redacted_derivation_error};
use rtao_core::keys::Keypair;
use rtao_signer::{
    AccountId, BoxFuture, Coldkey, CryptoType, DynKeySource, Hotkey, KeySource, KeyView, Signer,
    SignerError, SignerRole,
};
use secrecy::{ExposeSecret, SecretString};
use std::{
    fmt,
    path::{Path, PathBuf},
    sync::Arc,
};
use zeroize::Zeroizing;

pub(crate) fn expand_user(path: &Path) -> Result<PathBuf, WalletError> {
    if path == Path::new("~") || path.starts_with("~/") {
        let home = std::env::var_os("HOME").ok_or_else(|| {
            WalletError::InvalidName("cannot expand wallet path without HOME".into())
        })?;
        let suffix = path
            .strip_prefix("~")
            .map_err(|_| WalletError::InvalidName("invalid home-relative wallet path".into()))?;
        return Ok(PathBuf::from(home).join(suffix));
    }
    Ok(path.to_owned())
}
/// Immutable public wallet layout; it cannot unlock keys.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WalletRef {
    name: String,
    hotkey: String,
    path: PathBuf,
}
impl WalletRef {
    /// Resolve Python's `wallet/hotkey` shorthand and an explicit wallet root.
    ///
    /// The library does not consult `BT_WALLET_PATH`.
    pub fn new(
        name: impl Into<String>,
        hotkey: impl Into<String>,
        path: impl Into<PathBuf>,
    ) -> Result<Self, WalletError> {
        let mut name = name.into();
        let mut hotkey = hotkey.into();
        if name.contains('/') {
            if hotkey != "default" {
                return Err(WalletError::InvalidName("give the hotkey either in the name ('cold/hot') or as the hotkey= argument, not both".into()));
            }
            let compound = name.clone();
            let (cold, hot) = compound
                .split_once('/')
                .ok_or_else(|| WalletError::InvalidName("invalid compound wallet name".into()))?;
            if cold.is_empty() || hot.is_empty() || hot.contains('/') {
                return Err(WalletError::InvalidName(format!(
                    "invalid wallet name {}: expected 'wallet/hotkey' with exactly one slash and both parts non-empty",
                    rtao_primitives::pyfmt::string_repr(&compound)
                )));
            }
            name = cold.into();
            hotkey = hot.into();
        }
        for (label, segment) in [("wallet", &name), ("hotkey", &hotkey)] {
            if segment.is_empty()
                || segment == "."
                || segment == ".."
                || Path::new(segment).components().count() != 1
                || segment.contains(std::path::MAIN_SEPARATOR)
            {
                return Err(WalletError::InvalidName(format!(
                    "invalid {label} name: expected one non-empty path component"
                )));
            }
        }
        Ok(Self {
            name,
            hotkey,
            path: expand_user(&path.into())?,
        })
    }
    /// Construct the Python defaults under `~/.bittensor/wallets`.
    pub fn default_wallet() -> Result<Self, WalletError> {
        Self::new("default", "default", "~/.bittensor/wallets")
    }
    /// Wallet directory name.
    pub fn name(&self) -> &str {
        &self.name
    }
    /// Selected hotkey file name.
    pub fn hotkey_name(&self) -> &str {
        &self.hotkey
    }
    /// Expanded wallet root directory.
    pub fn path(&self) -> &Path {
        &self.path
    }
    /// This wallet's directory.
    pub fn wallet_dir(&self) -> PathBuf {
        self.path.join(&self.name)
    }
    /// The coldkey's secret-file path.
    pub fn coldkey_path(&self) -> PathBuf {
        self.wallet_dir().join("coldkey")
    }
    /// The coldkey's public-only path.
    pub fn coldkeypub_path(&self) -> PathBuf {
        self.wallet_dir().join("coldkeypub.txt")
    }
    /// The selected hotkey's secret-file path.
    pub fn hotkey_path(&self) -> PathBuf {
        self.wallet_dir().join("hotkeys").join(&self.hotkey)
    }
    /// The selected hotkey's public-only path.
    pub fn hotkeypub_path(&self) -> PathBuf {
        self.wallet_dir()
            .join("hotkeys")
            .join(format!("{}pub.txt", self.hotkey))
    }
    /// Read the selected role's attached public identity and stored scheme.
    pub fn public_info(&self, role: SignerRole) -> Result<PublicKeyInfo, WalletError> {
        match role {
            SignerRole::Coldkey => ColdkeyFile::new(self.coldkey_path()).public_info(),
            SignerRole::Hotkey => HotkeyFile::new(self.hotkey_path()).public_info(),
        }
    }
}
impl KeyView for WalletRef {
    fn hotkey_address(&self) -> Result<AccountId, SignerError> {
        HotkeyFile::new(self.hotkey_path()).hotkey_address()
    }
    fn hotkey_public_key(&self) -> Result<[u8; 32], SignerError> {
        Ok(self.hotkey_address()?.0)
    }
    fn coldkey_address(&self) -> Result<AccountId, SignerError> {
        public_account(&self.coldkeypub_path()).map_err(signer_error)
    }
}
/// A wallet with independent, shared hotkey/coldkey unlock caches.
#[derive(Clone, Debug)]
pub struct Wallet {
    reference: WalletRef,
    hotkey: HotkeyFile,
    coldkey: ColdkeyFile,
}
impl Wallet {
    /// Construct a wallet without reading, deriving or unlocking any keys.
    pub fn new(
        name: impl Into<String>,
        hotkey: impl Into<String>,
        path: impl Into<PathBuf>,
    ) -> Result<Self, WalletError> {
        Ok(Self::from_reference(WalletRef::new(name, hotkey, path)?))
    }
    /// Bind secret-key sources to a previously resolved public layout.
    pub fn from_reference(reference: WalletRef) -> Self {
        Self {
            hotkey: HotkeyFile::new(reference.hotkey_path()),
            coldkey: ColdkeyFile::new(reference.coldkey_path()),
            reference,
        }
    }
    /// Use Python's default names and wallet root.
    pub fn default_wallet() -> Result<Self, WalletError> {
        Ok(Self::from_reference(WalletRef::default_wallet()?))
    }
    /// Borrow the public-only wallet reference.
    pub fn reference(&self) -> &WalletRef {
        &self.reference
    }
    /// Wallet directory name.
    pub fn name(&self) -> &str {
        self.reference.name()
    }
    /// Borrow the configured coldkey file source.
    pub fn coldkey_file(&self) -> &ColdkeyFile {
        &self.coldkey
    }
    /// Borrow the configured hotkey file source.
    pub fn hotkey_file(&self) -> &HotkeyFile {
        &self.hotkey
    }
    /// Lazily unlock and retain the coldkey with the configured password policy.
    pub async fn unlock_coldkey(&self) -> Result<Arc<dyn Signer>, WalletError> {
        self.coldkey.unlock().await
    }
    /// Lazily load and retain the selected hotkey.
    pub async fn unlock_hotkey(&self) -> Result<Arc<dyn Signer>, WalletError> {
        self.hotkey.unlock().await
    }
    /// Read account and crypto scheme without acquiring a signer.
    pub fn public_info(&self, role: SignerRole) -> Result<PublicKeyInfo, WalletError> {
        self.state(role).public_info()
    }
    /// Return a source that can sign only as a hotkey.
    pub fn hotkey_only(&self) -> HotkeyFile {
        self.hotkey.clone()
    }
    /// Return a source that can sign only as a coldkey.
    pub fn coldkey_only(&self) -> ColdkeyFile {
        self.coldkey.clone()
    }
    /// Configure lazy password resolution for the coldkey.
    pub fn with_password_sources(mut self, sources: PasswordSources) -> Self {
        self.coldkey = self.coldkey.with_password_sources(sources);
        self
    }
    /// Configure the selected hotkey's independent password-resolution policy.
    /// Coldkey password files and native password services are not inherited.
    pub fn with_hotkey_password_sources(mut self, sources: PasswordSources) -> Self {
        self.hotkey = self.hotkey.with_password_sources(sources);
        self
    }
    /// Create a new coldkey and return its recovery phrase without printing it.
    pub async fn create_new_coldkey(
        &self,
        n_words: usize,
        crypto: CryptoType,
        options: WriteOptions,
    ) -> Result<GeneratedKey, WalletError> {
        self.create_key(SignerRole::Coldkey, n_words, crypto, options)
            .await
    }
    /// Create a new hotkey and its public companion without printing secrets.
    pub async fn create_new_hotkey(
        &self,
        n_words: usize,
        crypto: CryptoType,
        options: WriteOptions,
    ) -> Result<GeneratedKey, WalletError> {
        self.create_key(SignerRole::Hotkey, n_words, crypto, options)
            .await
    }
    async fn create_key(
        &self,
        role: SignerRole,
        n_words: usize,
        crypto: CryptoType,
        options: WriteOptions,
    ) -> Result<GeneratedKey, WalletError> {
        let (keypair, mnemonic) = rtao_core::offload(move || {
            let mnemonic = SecretString::new(Keypair::generate_mnemonic(n_words)?);
            let keypair = Keypair::from_mnemonic(mnemonic.expose_secret(), u8::from(crypto), None)
                .map_err(|error| redacted_derivation_error(error, SecretInput::Mnemonic))?;
            Ok::<_, WalletError>((keypair, mnemonic))
        })
        .await
        .map_err(|_| WalletError::Blocking("key creation task failed".into()))??;
        let account = self.state(role).store(keypair, options).await?;
        Ok(GeneratedKey {
            mnemonic,
            account,
            crypto,
        })
    }
    pub(crate) fn state(&self, role: SignerRole) -> &crate::files::FileState {
        match role {
            SignerRole::Coldkey => &self.coldkey.0,
            SignerRole::Hotkey => &self.hotkey.0,
        }
    }
    /// Regenerate a coldkey using the core's exact key derivation rules.
    pub async fn regenerate_coldkey(
        &self,
        source: RegenerateFrom,
        crypto: CryptoType,
        options: WriteOptions,
    ) -> Result<AccountId, WalletError> {
        self.regenerate(SignerRole::Coldkey, source, crypto, options)
            .await
    }
    /// Regenerate a hotkey using the core's exact key derivation rules.
    pub async fn regenerate_hotkey(
        &self,
        source: RegenerateFrom,
        crypto: CryptoType,
        options: WriteOptions,
    ) -> Result<AccountId, WalletError> {
        self.regenerate(SignerRole::Hotkey, source, crypto, options)
            .await
    }
    async fn regenerate(
        &self,
        role: SignerRole,
        source: RegenerateFrom,
        crypto: CryptoType,
        options: WriteOptions,
    ) -> Result<AccountId, WalletError> {
        let keypair = rtao_core::offload(move || source.derive(crypto))
            .await
            .map_err(|_| WalletError::Blocking("key derivation task failed".into()))??;
        self.state(role).store(keypair, options).await
    }
    /// Create or replace only the coldkey public companion, without secrets.
    pub async fn regenerate_coldkeypub(
        &self,
        account: AccountId,
        crypto: CryptoType,
        overwrite: Overwrite,
    ) -> Result<(), WalletError> {
        self.regenerate_public(self.reference.coldkeypub_path(), account, crypto, overwrite)
            .await
    }
    /// Create or replace only the hotkey public companion, without secrets.
    pub async fn regenerate_hotkeypub(
        &self,
        account: AccountId,
        crypto: CryptoType,
        overwrite: Overwrite,
    ) -> Result<(), WalletError> {
        self.regenerate_public(self.reference.hotkeypub_path(), account, crypto, overwrite)
            .await
    }
    async fn regenerate_public(
        &self,
        path: PathBuf,
        account: AccountId,
        crypto: CryptoType,
        overwrite: Overwrite,
    ) -> Result<(), WalletError> {
        rtao_core::offload(move || {
            let keypair = Keypair::new(None, Some(&account.0), u8::from(crypto), 42)?;
            let bytes = rtao_core::keyfile_json::serialized_keypair_to_keyfile_data(&keypair)?;
            atomic_write(&path, &bytes, overwrite)
        })
        .await
        .map_err(|error| WalletError::Blocking(error.to_string()))?
    }
}
impl KeyView for Wallet {
    fn hotkey_address(&self) -> Result<AccountId, SignerError> {
        self.hotkey.hotkey_address()
    }
    fn hotkey_public_key(&self) -> Result<[u8; 32], SignerError> {
        self.hotkey.hotkey_public_key()
    }
    fn coldkey_address(&self) -> Result<AccountId, SignerError> {
        self.coldkey.coldkey_address()
    }
}
impl KeySource<Coldkey> for Wallet {
    fn address(&self) -> Result<AccountId, SignerError> {
        self.coldkey_address()
    }
    async fn signer(&self) -> Result<Arc<dyn Signer>, SignerError> {
        self.coldkey.unlock().await.map_err(signer_error)
    }
}
impl KeySource<Hotkey> for Wallet {
    fn address(&self) -> Result<AccountId, SignerError> {
        self.hotkey_address()
    }
    async fn signer(&self) -> Result<Arc<dyn Signer>, SignerError> {
        self.hotkey.unlock().await.map_err(signer_error)
    }
}
impl DynKeySource for Wallet {
    fn signer_for<'a>(
        &'a self,
        role: SignerRole,
    ) -> BoxFuture<'a, Result<Arc<dyn Signer>, SignerError>> {
        Box::pin(async move { self.state(role).unlock().await.map_err(signer_error) })
    }
}
/// A new key's recovery material; Debug output always redacts the mnemonic.
#[derive(Debug)]
pub struct GeneratedKey {
    /// BIP39 recovery phrase, zeroized on drop.
    pub mnemonic: SecretString,
    /// Created account's public identity.
    pub account: AccountId,
    /// Signature algorithm.
    pub crypto: CryptoType,
}
/// An explicit recovery source; no source is printed or logged.
pub enum RegenerateFrom {
    /// A BIP39 phrase.
    Mnemonic(SecretString),
    /// Raw secret seed bytes.
    Seed(Zeroizing<Vec<u8>>),
    /// A Substrate secret URI, including dev URIs.
    Uri(SecretString),
    /// The core's hexadecimal private-key representation.
    PrivateKey(SecretString),
    /// A password-protected Polkadot-JS JSON export, requiring `encrypted`.
    EncryptedJson {
        /// The encrypted JSON export; zeroized on drop.
        json: SecretString,
        /// Its import passphrase; zeroized on drop.
        passphrase: SecretString,
    },
}
impl fmt::Debug for RegenerateFrom {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RegenerateFrom([REDACTED])")
    }
}
impl RegenerateFrom {
    fn derive(self, crypto: CryptoType) -> Result<Keypair, WalletError> {
        Ok(match self {
            Self::Mnemonic(secret) => {
                Keypair::from_mnemonic(secret.expose_secret(), u8::from(crypto), None)
                    .map_err(|error| redacted_derivation_error(error, SecretInput::Mnemonic))?
            }
            Self::Seed(secret) => Keypair::from_seed(&secret, u8::from(crypto))
                .map_err(|error| redacted_derivation_error(error, SecretInput::Seed))?,
            Self::Uri(secret) => Keypair::from_uri(secret.expose_secret(), u8::from(crypto))
                .map_err(|error| redacted_derivation_error(error, SecretInput::Uri))?,
            Self::PrivateKey(secret) => {
                Keypair::from_private_key(secret.expose_secret(), u8::from(crypto))
                    .map_err(|error| redacted_derivation_error(error, SecretInput::PrivateKey))?
            }
            Self::EncryptedJson { json, passphrase } => {
                if json.expose_secret().is_empty() {
                    return Err(WalletError::InvalidName(
                        "encrypted JSON coldkey data is empty".into(),
                    ));
                }
                if passphrase.expose_secret().is_empty() {
                    return Err(WalletError::PasswordRequired);
                }
                #[cfg(feature = "encrypted")]
                {
                    Keypair::from_encrypted_json(json.expose_secret(), passphrase.expose_secret())
                        .map_err(|error| redacted_derivation_error(error, SecretInput::EncryptedJson))?
                }
                #[cfg(not(feature = "encrypted"))]
                {
                    return Err(WalletError::FeatureRequired("encrypted"));
                }
            }
        })
    }
}
