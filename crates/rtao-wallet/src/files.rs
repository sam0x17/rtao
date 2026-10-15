// Derived from RaoFoundation/subtensor sdk/python/bittensor/keyfiles.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/signing.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Atomic keyfile writes, public-field inspection and once-per-source unlocks.

#[cfg(feature = "encrypted")]
use crate::password::resolve_password_for_keyfile;
use crate::{PasswordSources, WalletError, io_error, signer_error};
use rtao_core::{keyfile_json, keys::Keypair};
use rtao_signer::{
    AccountId, BoxFuture, Coldkey, CryptoType, DynKeySource, Hotkey, KeySource, KeyView,
    KeypairSigner, Signer, SignerError, SignerRole,
};
#[cfg(feature = "encrypted")]
use secrecy::ExposeSecret;
use secrecy::SecretString;
use std::{
    fmt,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
use tokio::sync::Mutex;
use zeroize::{Zeroize, Zeroizing};

const MAX_KEYFILE_BYTES: u64 = 16 * 1024 * 1024;
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

/// Public identity and scheme obtained without deriving or unlocking a key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PublicKeyInfo {
    /// Exact public account bytes.
    pub account: AccountId,
    /// Stored scheme, defaulting to sr25519 for legacy files as in the core.
    pub crypto_type: CryptoType,
}

/// Whether an existing keyfile may be replaced.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Overwrite {
    /// Refuse replacement; this is the default.
    #[default]
    No,
    /// Explicitly replace an existing keyfile.
    Yes,
}
/// Keyfile creation/regeneration options.
///
/// Role-specific wallet writes force encryption when a password is supplied.
/// The generic [`crate::Keyfile`] instead honors `encrypt`, matching Python's
/// single-file operation.
#[derive(Clone, Debug)]
pub struct WriteOptions {
    /// Encrypt using the core's btcli-compatible NaCl format.
    ///
    /// Generic keyfiles honor this field directly; a role-specific wallet also
    /// encrypts when `password` is present.
    pub encrypt: bool,
    /// Password for encryption, otherwise resolved from the file's sources.
    ///
    /// Role-specific wallet writes reuse configured [`PasswordSources`],
    /// including coldkey environment/file/native sources. Generic
    /// [`crate::Keyfile::set_keypair`] uses explicit input or an enabled
    /// confirming prompt and skips existing environment values.
    pub password: Option<SecretString>,
    /// Permission to replace existing secret and public files.
    pub overwrite: Overwrite,
}
impl WriteOptions {
    /// Python's coldkey defaults: encrypted and no overwrite.
    pub fn coldkey() -> Self {
        Self {
            encrypt: true,
            password: None,
            overwrite: Overwrite::No,
        }
    }
    /// Python's hotkey defaults: plaintext and no overwrite.
    pub fn hotkey() -> Self {
        Self {
            encrypt: false,
            password: None,
            overwrite: Overwrite::No,
        }
    }
}
impl Default for WriteOptions {
    fn default() -> Self {
        Self::hotkey()
    }
}

pub(crate) struct FileState {
    pub path: PathBuf,
    pub public: PathBuf,
    pub cold_public: Option<PathBuf>,
    pub sources: PasswordSources,
    pub role: SignerRole,
    signer: Arc<Mutex<Option<Arc<dyn Signer>>>>,
}
impl fmt::Debug for FileState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Keyfile")
            .field("path", &self.path)
            .field("role", &self.role)
            .field("secrets", &"[REDACTED]")
            .finish()
    }
}
impl FileState {
    pub(crate) fn new(
        path: PathBuf,
        public: PathBuf,
        cold_public: Option<PathBuf>,
        role: SignerRole,
    ) -> Arc<Self> {
        Arc::new(Self {
            path,
            public,
            cold_public,
            role,
            sources: PasswordSources::default(),
            signer: Arc::new(Mutex::new(None)),
        })
    }
    pub(crate) async fn unlock(&self) -> Result<Arc<dyn Signer>, WalletError> {
        let mut cached = self.signer.lock().await;
        if let Some(signer) = cached.as_ref() {
            return Ok(signer.clone());
        }
        let keypair = self.load_keypair().await?;
        let signer: Arc<dyn Signer> = Arc::new(
            KeypairSigner::new(keypair)
                .map_err(|error| WalletError::Blocking(error.to_string()))?,
        );
        *cached = Some(signer.clone());
        Ok(signer)
    }

    // One authenticated read/KDF path serves both signing and sealed-box
    // decryption. The private key never becomes a CLI value or public API.
    async fn load_keypair(&self) -> Result<Keypair, WalletError> {
        let path = self.path.clone();
        let bytes = rtao_core::offload(move || read_data(&path))
            .await
            .map_err(|error| WalletError::Blocking(error.to_string()))??;
        let password: Option<crate::password::ResolvedPassword> =
            if keyfile_json::keyfile_data_is_encrypted(&bytes) {
                #[cfg(not(feature = "encrypted"))]
                return Err(WalletError::FeatureRequired("encrypted"));
                #[cfg(feature = "encrypted")]
                {
                    Some(
                        resolve_password_for_keyfile(
                            &self.path,
                            &self.sources,
                            self.role == SignerRole::Coldkey,
                        )
                        .await?
                        .ok_or(WalletError::PasswordRequired)?,
                    )
                }
            } else {
                None
            };
        let keypair = rtao_core::offload(move || {
            #[cfg(feature = "encrypted")]
            let bytes = if let Some(password) = password {
                match rtao_core::keyfiles::decrypt_keyfile_data(
                    &bytes,
                    Some(password.secret.expose_secret()),
                ) {
                    Ok(plain) => Zeroizing::new(plain),
                    Err(error) => {
                        let error = rtao_core::CoreError::from(error);
                        return Err(match error {
                            rtao_core::CoreError::WrongPassword(_) => WalletError::WrongPassword {
                                source: password.source,
                            },
                            error => WalletError::Core(error),
                        });
                    }
                }
            } else {
                bytes
            };
            #[cfg(not(feature = "encrypted"))]
            let _ = password;
            keyfile_json::deserialize_keypair_from_keyfile_data(&bytes).map_err(WalletError::from)
        })
        .await
        .map_err(|error| WalletError::Blocking(error.to_string()))??;
        Ok(keypair)
    }
    #[cfg(feature = "encrypted")]
    pub(crate) async fn decrypt_message_bytes(
        &self,
        bytes: Vec<u8>,
    ) -> Result<SecretString, WalletError> {
        // Serialize with key replacement and initial unlock. Decryption never
        // mutates the retained signing cache or exports its private key.
        let _cached = self.signer.lock().await;
        let keypair = self.load_keypair().await?;
        rtao_core::offload(move || {
            let plain = Zeroizing::new(keypair.decrypt(&bytes).map_err(WalletError::from)?);
            let text = std::str::from_utf8(&plain).map_err(|_| {
                WalletError::Core(rtao_core::CoreError::Crypto(
                    "decrypted message is not valid UTF-8".into(),
                ))
            })?;
            Ok(SecretString::new(text.to_owned()))
        })
        .await
        .map_err(|_| WalletError::Blocking("message decryption task failed".into()))?
    }
    pub(crate) async fn store(
        &self,
        keypair: Keypair,
        options: WriteOptions,
    ) -> Result<AccountId, WalletError> {
        self.store_with(keypair, options, || Ok(())).await
    }
    // The callback lets unit fixtures pause the actual worker after it owns
    // the operation lock, before it can install either file. Its returned owner
    // stays alive until publication finishes, including errors and unwind.
    async fn store_with<T: Send + 'static>(
        &self,
        keypair: Keypair,
        options: WriteOptions,
        before_install: impl FnOnce() -> Result<T, WalletError> + Send + 'static,
    ) -> Result<AccountId, WalletError> {
        let mut cached = self.signer.clone().lock_owned().await;
        preflight(&self.path, options.overwrite)?;
        preflight(&self.public, options.overwrite)?;
        let account = AccountId(keypair.public_key_bytes());
        let encrypt = options.encrypt || options.password.is_some();
        let password: Option<SecretString> = if encrypt {
            #[cfg(not(feature = "encrypted"))]
            return Err(WalletError::FeatureRequired("encrypted"));
            #[cfg(feature = "encrypted")]
            {
                let mut sources = self.sources.clone();
                if options.password.is_some() {
                    sources.password = options.password;
                }
                Some(
                    resolve_password_for_keyfile(
                        &self.path,
                        &sources,
                        self.role == SignerRole::Coldkey,
                    )
                    .await?
                    .ok_or(WalletError::PasswordRequired)?
                    .secret,
                )
            }
        } else {
            None
        };
        let path = self.path.clone();
        let public_path = self.public.clone();
        // A failed or cancelled write may still install the secret file. The
        // blocking worker retains this owned guard until publication finishes,
        // even if its async caller is dropped. No unlock or later replacement
        // can cache/interleave an older key while that worker is still running.
        *cached = None;
        rtao_core::offload(move || {
            let result = (|| {
                let _installation = before_install()?;
                let public = Keypair::new(
                    None,
                    Some(&keypair.public_key_bytes()),
                    keypair.crypto_type(),
                    keypair.ss58_format(),
                )?;
                let bytes =
                    Zeroizing::new(keyfile_json::serialized_keypair_to_keyfile_data(&keypair)?);
                #[cfg(feature = "encrypted")]
                let bytes = if let Some(password) = password {
                    Zeroizing::new(rtao_core::keyfiles::encrypt_keyfile_data(
                        &bytes,
                        password.expose_secret(),
                    )?)
                } else {
                    bytes
                };
                #[cfg(not(feature = "encrypted"))]
                let _ = password;
                let public_bytes =
                    Zeroizing::new(keyfile_json::serialized_keypair_to_keyfile_data(&public)?);
                atomic_write(&path, &bytes, options.overwrite)?;
                atomic_write(&public_path, &public_bytes, options.overwrite)?;
                Ok::<_, WalletError>(())
            })();
            // Covers success and partial-write errors. If the worker panics,
            // the cache was already empty and the guard is released by unwind.
            *cached = None;
            result
        })
        .await
        .map_err(|error| WalletError::Blocking(error.to_string()))??;
        Ok(account)
    }
    pub(crate) fn hot_address(&self) -> Result<AccountId, SignerError> {
        if self.role != SignerRole::Hotkey {
            return Err(SignerError::KeyUnavailable(SignerRole::Hotkey));
        }
        match public_account(&self.public) {
            Ok(account) => Ok(account),
            Err(WalletError::Io { error, .. }) if error.kind() == std::io::ErrorKind::NotFound => {
                public_account(&self.path).map_err(signer_error)
            }
            Err(error) => Err(signer_error(error)),
        }
    }
    pub(crate) fn public_info(&self) -> Result<PublicKeyInfo, WalletError> {
        match public_info(&self.public) {
            Ok(info) => Ok(info),
            Err(WalletError::Io { error, .. })
                if self.role == SignerRole::Hotkey
                    && error.kind() == std::io::ErrorKind::NotFound =>
            {
                public_info(&self.path)
            }
            Err(error) => Err(error),
        }
    }
    pub(crate) fn cold_address(&self) -> Result<AccountId, SignerError> {
        let path = if self.role == SignerRole::Coldkey {
            Some(&self.public)
        } else {
            self.cold_public.as_ref()
        };
        match path {
            Some(path) => public_account(path).map_err(signer_error),
            None => Err(SignerError::KeyUnavailable(SignerRole::Coldkey)),
        }
    }
}

/// A hotkey-only signing source; the default build requires no core host feature.
///
/// A hotkey file cannot satisfy a coldkey-only signing operation:
///
/// ```compile_fail
/// use rtao_signer::{Coldkey, KeySource};
/// use rtao_wallet::HotkeyFile;
/// fn owner_operation(_: &impl KeySource<Coldkey>) {}
/// owner_operation(&HotkeyFile::new("disposable/hotkeys/worker"));
/// ```
#[derive(Clone, Debug)]
pub struct HotkeyFile(pub(crate) Arc<FileState>);
impl HotkeyFile {
    /// Use an explicit keyfile and its sibling `<name>pub.txt` companion.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let mut public = path.as_os_str().to_owned();
        public.push("pub.txt");
        let cold_public = path
            .parent()
            .filter(|parent| parent.file_name() == Some(std::ffi::OsStr::new("hotkeys")))
            .and_then(Path::parent)
            .map(|parent| parent.join("coldkeypub.txt"));
        Self(FileState::new(
            path,
            PathBuf::from(public),
            cold_public,
            SignerRole::Hotkey,
        ))
    }
    /// Borrow the configured keyfile path without reading it.
    pub fn path(&self) -> &Path {
        &self.0.path
    }
    /// Borrow the public-only companion path.
    pub fn public_path(&self) -> &Path {
        &self.0.public
    }
    /// Read attached identity and scheme using public JSON fields only.
    pub fn public_info(&self) -> Result<PublicKeyInfo, WalletError> {
        self.0.public_info()
    }
    /// Borrow the configured resolution policy; secret Debug output is redacted.
    pub fn password_sources(&self) -> &PasswordSources {
        &self.0.sources
    }
    /// Supply password sources for an optional encrypted hotkey.
    pub fn with_password_sources(mut self, sources: PasswordSources) -> Self {
        self.0 = Arc::new(FileState {
            path: self.0.path.clone(),
            public: self.0.public.clone(),
            cold_public: self.0.cold_public.clone(),
            role: self.0.role,
            sources,
            signer: Arc::new(Mutex::new(None)),
        });
        self
    }
    /// Load and retain one signer, offloading keyfile work from the reactor.
    pub async fn unlock(&self) -> Result<Arc<dyn Signer>, WalletError> {
        self.0.unlock().await
    }
}
/// A coldkey-only source with a separate public companion and lazy unlock.
#[derive(Clone, Debug)]
pub struct ColdkeyFile(pub(crate) Arc<FileState>);
impl ColdkeyFile {
    /// Use an explicit coldkey and its sibling `coldkeypub.txt` companion.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let public = path.with_file_name("coldkeypub.txt");
        Self(FileState::new(path, public, None, SignerRole::Coldkey))
    }
    /// Borrow the configured secret-file path without reading it.
    pub fn path(&self) -> &Path {
        &self.0.path
    }
    /// Borrow the public-only companion path.
    pub fn public_path(&self) -> &Path {
        &self.0.public
    }
    /// Read the coldkey companion's identity and scheme without unlocking.
    pub fn public_info(&self) -> Result<PublicKeyInfo, WalletError> {
        self.0.public_info()
    }
    /// Borrow the configured resolution policy; secret Debug output is redacted.
    pub fn password_sources(&self) -> &PasswordSources {
        &self.0.sources
    }
    /// Configure explicit/env/file password resolution for lazy unlocking.
    pub fn with_password_sources(mut self, sources: PasswordSources) -> Self {
        self.0 = Arc::new(FileState {
            path: self.0.path.clone(),
            public: self.0.public.clone(),
            cold_public: None,
            role: self.0.role,
            sources,
            signer: Arc::new(Mutex::new(None)),
        });
        self
    }
    /// Load and retain one signer, offloading expensive KDF work.
    pub async fn unlock(&self) -> Result<Arc<dyn Signer>, WalletError> {
        self.0.unlock().await
    }
}
macro_rules! file_roles {
    ($file:ident,$role:ty,$kind:expr,$address:ident) => {
        impl KeyView for $file {
            fn hotkey_address(&self) -> Result<AccountId, SignerError> {
                self.0.hot_address()
            }
            fn hotkey_public_key(&self) -> Result<[u8; 32], SignerError> {
                Ok(self.hotkey_address()?.0)
            }
            fn coldkey_address(&self) -> Result<AccountId, SignerError> {
                self.0.cold_address()
            }
        }
        impl KeySource<$role> for $file {
            fn address(&self) -> Result<AccountId, SignerError> {
                self.$address()
            }
            async fn signer(&self) -> Result<Arc<dyn Signer>, SignerError> {
                self.unlock().await.map_err(signer_error)
            }
        }
        impl DynKeySource for $file {
            fn signer_for<'a>(
                &'a self,
                role: SignerRole,
            ) -> BoxFuture<'a, Result<Arc<dyn Signer>, SignerError>> {
                Box::pin(async move {
                    if role != $kind {
                        return Err(SignerError::RoleUnavailable(role));
                    }
                    self.unlock().await.map_err(signer_error)
                })
            }
        }
    };
}
file_roles!(HotkeyFile, Hotkey, SignerRole::Hotkey, hotkey_address);
file_roles!(ColdkeyFile, Coldkey, SignerRole::Coldkey, coldkey_address);

/// Read at most 16 MiB from an opened regular keyfile, wiping the returned bytes
/// on drop. Regular symlink targets are supported; Unix special files cannot
/// block before the descriptor's file type is checked.
pub fn read_data(path: &Path) -> Result<Zeroizing<Vec<u8>>, WalletError> {
    read_secret_file(path, "keyfile")
}

pub(crate) fn read_secret_file(
    path: &Path,
    description: &str,
) -> Result<Zeroizing<Vec<u8>>, WalletError> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // Regular symlink targets remain supported. A FIFO must not block the
        // worker before we can validate the opened descriptor.
        options.custom_flags(libc::O_NONBLOCK);
    }
    #[cfg(not(unix))]
    if !fs::metadata(path)
        .map_err(|error| io_error(path, error))?
        .is_file()
    {
        return Err(io_error(
            path,
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "expected a regular file"),
        ));
    }
    let file = options.open(path).map_err(|error| io_error(path, error))?;
    let metadata = file.metadata().map_err(|error| io_error(path, error))?;
    if !metadata.is_file() {
        return Err(io_error(
            path,
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "expected a regular file"),
        ));
    }
    let oversized = || {
        WalletError::Core(rtao_core::CoreError::Keyfile(format!(
            "{description} exceeds 16 MiB limit"
        )))
    };
    if metadata.len() > MAX_KEYFILE_BYTES {
        return Err(oversized());
    }
    read_secret_bytes(
        file,
        metadata.len() as usize,
        MAX_KEYFILE_BYTES as usize,
        path,
        description,
        |_| {},
    )
}

// A file can grow after metadata was read. Copy to a new guarded allocation
// before every required growth, wipe the initialized old prefix, then drop its
// wiping owner. Neither extend_from_slice below can reallocate secret bytes.
// The retirement observer is a no-op outside unit fixtures.
fn read_secret_bytes(
    mut reader: impl Read,
    size_hint: usize,
    limit: usize,
    path: &Path,
    description: &str,
    mut retired: impl FnMut(&[u8]),
) -> Result<Zeroizing<Vec<u8>>, WalletError> {
    let mut bytes = Zeroizing::new(Vec::with_capacity(size_hint.min(limit)));
    let mut chunk = Zeroizing::new([0u8; 8192]);
    loop {
        // Read one byte beyond the remaining allowance to detect growth while
        // bounding acquisition. The extra byte stays in the wiping scratch.
        let allowed = (limit - bytes.len()).saturating_add(1).min(chunk.len());
        let count = match reader.read(&mut chunk[..allowed]) {
            Ok(0) => return Ok(bytes),
            Ok(count) => count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(io_error(path, error)),
        };
        let needed = bytes.len() + count;
        if needed > limit {
            return Err(WalletError::Core(rtao_core::CoreError::Keyfile(format!(
                "{description} exceeds 16 MiB limit"
            ))));
        }
        if needed > bytes.capacity() {
            let capacity = bytes.capacity().saturating_mul(2).max(needed).min(limit);
            let mut replacement = Zeroizing::new(Vec::with_capacity(capacity));
            replacement.extend_from_slice(&bytes);
            bytes.as_mut_slice().zeroize();
            retired(&bytes);
            // Dropping Zeroizing also wipes spare capacity before freeing it.
            bytes = replacement;
        }
        bytes.extend_from_slice(&chunk[..count]);
    }
}

/// Inspect only public JSON fields; this never calls the secret-key decoder.
pub(crate) fn public_account(path: &Path) -> Result<AccountId, WalletError> {
    public_info(path).map(|info| info.account)
}
pub(crate) fn public_info(path: &Path) -> Result<PublicKeyInfo, WalletError> {
    let bytes = read_data(path)?;
    if keyfile_json::keyfile_data_is_encrypted(&bytes) {
        return Err(WalletError::PublicKeyUnavailable(path.to_owned()));
    }
    keyfile_json::with_public_keyfile_fields(&bytes, |crypto_type, address, public| {
        let crypto_type = CryptoType::try_from(crypto_type).map_err(|_| {
            WalletError::Core(rtao_core::CoreError::Crypto(
                "unsupported public keyfile crypto type".into(),
            ))
        })?;
        if let Some(address) = address {
            return address
                .parse()
                .map(|account| PublicKeyInfo {
                    account,
                    crypto_type,
                })
                .map_err(WalletError::from);
        }
        if let Some(key) = public {
            let bytes = hex::decode(key.strip_prefix("0x").unwrap_or(key))
                .map_err(|_| WalletError::PublicKeyUnavailable(path.to_owned()))?;
            return bytes
                .try_into()
                .map(|bytes| PublicKeyInfo {
                    account: AccountId(bytes),
                    crypto_type,
                })
                .map_err(|_| WalletError::PublicKeyUnavailable(path.to_owned()));
        }
        Err(WalletError::PublicKeyUnavailable(path.to_owned()))
    })
    .map_err(|_| WalletError::PublicKeyUnavailable(path.to_owned()))?
}
fn preflight(path: &Path, overwrite: Overwrite) -> Result<(), WalletError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(WalletError::Symlink(path.to_owned()))
        }
        Ok(_) if overwrite == Overwrite::No => Err(WalletError::AlreadyExists(path.to_owned())),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_error(path, error)),
    }
}
pub(crate) fn private_dirs(path: &Path) -> Result<(), WalletError> {
    if path.as_os_str().is_empty() {
        return Ok(());
    }
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(WalletError::Symlink(path.to_owned()));
        }
        Ok(metadata) if metadata.is_dir() => return Ok(()),
        Ok(_) => {
            return Err(io_error(
                path,
                std::io::Error::new(
                    std::io::ErrorKind::NotADirectory,
                    "wallet parent is not a directory",
                ),
            ));
        }
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
            if metadata.file_type().is_symlink() {
                Err(WalletError::Symlink(path.to_owned()))
            } else if metadata.is_dir() {
                Ok(())
            } else {
                Err(io_error(path, error))
            }
        }
        Err(error) => Err(io_error(path, error)),
    }
}
struct TempFile(PathBuf);
impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
pub(crate) fn atomic_write(
    path: &Path,
    data: &[u8],
    overwrite: Overwrite,
) -> Result<(), WalletError> {
    let parent = path
        .parent()
        .ok_or_else(|| WalletError::InvalidName("keyfile has no parent directory".into()))?;
    let parent = if parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        parent
    };
    // Check every existing ancestor, including ancestors above the nearest directory.
    for ancestor in parent
        .ancestors()
        .filter(|path| !path.as_os_str().is_empty())
    {
        if fs::symlink_metadata(ancestor).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
            return Err(WalletError::Symlink(ancestor.to_owned()));
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
        let temporary = parent.join(format!(
            ".rtao-keyfile-{}-{}.tmp",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        match options.open(&temporary) {
            Ok(file) => {
                created = Some((TempFile(temporary), file));
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(io_error(&temporary, error)),
        }
    }
    let (temporary, mut file) = created.ok_or_else(|| {
        io_error(
            path,
            std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "could not allocate keyfile temporary",
            ),
        )
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|error| io_error(&temporary.0, error))?;
    }
    file.write_all(data)
        .and_then(|()| file.sync_all())
        .map_err(|error| io_error(&temporary.0, error))?;
    drop(file);
    if overwrite == Overwrite::Yes {
        fs::rename(&temporary.0, path).map_err(|error| io_error(path, error))?;
    } else {
        fs::hard_link(&temporary.0, path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                WalletError::AlreadyExists(path.to_owned())
            } else {
                io_error(path, error)
            }
        })?;
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
mod replacement_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::testsupport::TempRoot;
    use std::{future::Future, pin::Pin, sync::mpsc, task::Poll, time::Duration};
    use tokio::sync::oneshot;

    struct ReleaseWorker {
        resume: Option<mpsc::Sender<()>>,
        finished: Option<mpsc::Receiver<()>>,
    }
    impl ReleaseWorker {
        fn release(mut self) {
            self.finish().unwrap();
        }
        fn finish(&mut self) -> Result<(), mpsc::RecvTimeoutError> {
            if let Some(sender) = self.resume.take() {
                let _ = sender.send(());
            }
            if let Some(finished) = self.finished.take() {
                finished.recv_timeout(Duration::from_secs(30))?;
            }
            Ok(())
        }
    }
    impl Drop for ReleaseWorker {
        fn drop(&mut self) {
            // Join publication before TempRoot cleanup even if an assertion
            // panics. The worker does not depend on this Tokio task to finish.
            let _ = self.finish();
        }
    }
    struct WorkerFinished {
        finished: Option<mpsc::Sender<()>>,
        // Keep disposable files alive until publication ends even if a fixture
        // assertion fails and its bounded completion wait expires.
        _root: Arc<TempRoot>,
    }
    impl Drop for WorkerFinished {
        fn drop(&mut self) {
            if let Some(sender) = self.finished.take() {
                let _ = sender.send(());
            }
        }
    }
    fn paused_worker(
        root: Arc<TempRoot>,
    ) -> (
        oneshot::Receiver<()>,
        ReleaseWorker,
        impl FnOnce() -> Result<WorkerFinished, WalletError> + Send + 'static,
    ) {
        let (started, observed) = oneshot::channel();
        let (release, resume) = mpsc::channel();
        let (finished, completion) = mpsc::channel();
        let pause = move || {
            let finished = WorkerFinished {
                finished: Some(finished),
                _root: root,
            };
            let _ = started.send(());
            resume.recv_timeout(Duration::from_secs(30)).map_err(|_| {
                WalletError::Blocking("disposable replacement worker was not released".into())
            })?;
            Ok(finished)
        };
        (
            observed,
            ReleaseWorker {
                resume: Some(release),
                finished: Some(completion),
            },
            pause,
        )
    }
    async fn pending_once<F: Future>(mut future: Pin<&mut F>) -> bool {
        std::future::poll_fn(|cx| Poll::Ready(future.as_mut().poll(cx).is_pending())).await
    }
    fn replacement() -> WriteOptions {
        WriteOptions {
            overwrite: Overwrite::Yes,
            ..WriteOptions::hotkey()
        }
    }
    fn state(root: &TempRoot) -> Arc<FileState> {
        FileState::new(
            root.path().join("hotkey"),
            root.path().join("hotkeypub.txt"),
            None,
            SignerRole::Hotkey,
        )
    }
    fn key(uri: &str) -> Keypair {
        Keypair::from_uri(uri, 1).unwrap()
    }
    fn account(uri: &str) -> AccountId {
        AccountId(key(uri).public_key_bytes())
    }

    #[tokio::test]
    async fn cancelled_replacement_keeps_worker_lock_and_unlocks_only_the_installed_key() {
        let root = Arc::new(TempRoot::new());
        let state = state(&root);
        state.store(key("//Alice"), replacement()).await.unwrap();
        let old = state.unlock().await.unwrap();
        assert_eq!(old.account(), account("//Alice"));
        let (started, release, pause) = paused_worker(root.clone());
        let replacing = {
            let state = state.clone();
            tokio::spawn(async move { state.store_with(key("//Bob"), replacement(), pause).await })
        };
        started.await.unwrap();
        replacing.abort();
        assert!(replacing.await.unwrap_err().is_cancelled());
        assert!(state.signer.try_lock().is_err());

        let mut unlocking = Box::pin(state.unlock());
        // Poll the actual unlock through its lock acquisition. No elapsed-time
        // or scheduler-yield assumption is used to establish that it waits.
        assert!(pending_once(unlocking.as_mut()).await);
        assert_eq!(public_account(&state.path).unwrap(), old.account());
        release.release();

        let fresh = unlocking.await.unwrap();
        assert_eq!(fresh.account(), account("//Bob"));
        assert_eq!(state.hot_address().unwrap(), fresh.account());
        assert!(Arc::ptr_eq(&fresh, &state.unlock().await.unwrap()));
        let signature = fresh
            .sign_raw(b"cancelled wallet replacement")
            .await
            .unwrap();
        assert!(
            rtao_core::keys::verify_strict(
                b"cancelled wallet replacement",
                &signature,
                &fresh.account().to_string(),
                1,
            )
            .unwrap()
        );
        assert_ne!(old.account(), fresh.account());
    }

    #[tokio::test]
    async fn cancelled_earlier_replacement_cannot_overwrite_a_later_completed_replacement() {
        let root = Arc::new(TempRoot::new());
        let state = state(&root);
        state.store(key("//Alice"), replacement()).await.unwrap();
        state.unlock().await.unwrap();
        let (started, release, pause) = paused_worker(root.clone());
        let earlier = {
            let state = state.clone();
            tokio::spawn(async move { state.store_with(key("//Bob"), replacement(), pause).await })
        };
        started.await.unwrap();
        earlier.abort();
        assert!(earlier.await.unwrap_err().is_cancelled());

        let mut later = Box::pin(state.store(key("//Charlie"), replacement()));
        assert!(pending_once(later.as_mut()).await);
        assert!(state.signer.try_lock().is_err());
        assert_eq!(public_account(&state.path).unwrap(), account("//Alice"));
        release.release();

        assert_eq!(later.await.unwrap(), account("//Charlie"));
        // Acquiring the same lock proves the abandoned earlier worker finished
        // before these observations, so it cannot publish afterward.
        let signer = state.unlock().await.unwrap();
        assert_eq!(signer.account(), account("//Charlie"));
        assert_eq!(public_account(&state.path).unwrap(), signer.account());
        assert_eq!(public_account(&state.public).unwrap(), signer.account());
    }

    #[tokio::test]
    async fn partial_replacement_failure_discards_the_previous_cached_signer() {
        let root = TempRoot::new();
        let state = state(&root);
        state.store(key("//Alice"), replacement()).await.unwrap();
        let old = state.unlock().await.unwrap();
        fs::remove_file(&state.public).unwrap();
        fs::create_dir(&state.public).unwrap();
        assert!(state.store(key("//Bob"), replacement()).await.is_err());
        assert_eq!(public_account(&state.path).unwrap(), account("//Bob"));
        let fresh = state.unlock().await.unwrap();
        assert_eq!(fresh.account(), account("//Bob"));
        assert_ne!(fresh.account(), old.account());
    }
}

#[cfg(test)]
mod read_security_tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use crate::testsupport::TempRoot;

    struct Fragmented<'a> {
        input: &'a [u8],
        fragment: usize,
        read: usize,
        interrupt_once: bool,
    }
    impl Read for Fragmented<'_> {
        fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
            if self.interrupt_once {
                self.interrupt_once = false;
                return Err(std::io::Error::from(std::io::ErrorKind::Interrupted));
            }
            let count = output.len().min(self.fragment).min(self.input.len());
            output[..count].copy_from_slice(&self.input[..count]);
            self.input = &self.input[count..];
            self.read += count;
            Ok(count)
        }
    }

    #[test]
    fn growing_secret_reads_wipe_each_retired_prefix_before_releasing_its_allocation() {
        let input = b"disposable-secret-prefix-continued-after-the-metadata-length-was-read";
        let mut reader = Fragmented {
            input,
            fragment: 3,
            read: 0,
            interrupt_once: true,
        };
        let mut retired_lengths = Vec::new();
        let bytes = read_secret_bytes(
            &mut reader,
            1,
            128,
            Path::new("disposable-growing-keyfile"),
            "keyfile",
            |retired| {
                assert!(retired.iter().all(|byte| *byte == 0));
                retired_lengths.push(retired.len());
            },
        )
        .unwrap();
        assert_eq!(bytes.as_slice(), input);
        assert_eq!(reader.read, input.len());
        assert!(retired_lengths.iter().filter(|length| **length > 0).count() >= 3);
    }

    #[test]
    fn accurate_secret_size_hint_never_retires_a_secret_allocation() {
        let input = b"disposable-password-file";
        let reader = Fragmented {
            input,
            fragment: 2,
            read: 0,
            interrupt_once: false,
        };
        let mut retirements = 0;
        let bytes = read_secret_bytes(
            reader,
            input.len(),
            128,
            Path::new("disposable-stable-password-file"),
            "password file",
            |_| retirements += 1,
        )
        .unwrap();
        assert_eq!(bytes.as_slice(), input);
        assert_eq!(retirements, 0);
    }

    #[test]
    fn secret_growth_refuses_at_one_byte_beyond_the_budget_without_reading_the_tail() {
        let input = [0x5a; 128];
        let mut reader = Fragmented {
            input: &input,
            fragment: 3,
            read: 0,
            interrupt_once: false,
        };
        let error = read_secret_bytes(
            &mut reader,
            1,
            32,
            Path::new("disposable-over-budget-keyfile"),
            "keyfile",
            |retired| assert!(retired.iter().all(|byte| *byte == 0)),
        )
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "keyfile error: keyfile exceeds 16 MiB limit"
        );
        assert_eq!(reader.read, 33);
        assert_eq!(reader.input.len(), input.len() - 33);
    }

    #[test]
    fn plaintext_hotkey_public_fallback_validates_whole_json_and_preserves_alias_precedence() {
        let root = TempRoot::new();
        let path = root.path().join("public-fallback-hotkey");
        let file = HotkeyFile::new(&path);
        let account = AccountId([7; 32]);
        let address = account.to_string();
        // These invalid recovery fields must never reach a key constructor.
        // Escaped unknown keys and values share the secret-aware parse path.
        let input = format!(
            r#"{{"ss58Address":"invalid first address","ss58\u0041ddress":"{address}","cryptoType":"\u00200\u0020","secretPhrase":"invalid\u0020recovery","secretSeed":"invalid","private\u0020field":"disposable\u0020secret"}}"#,
        );
        fs::write(&path, &input).unwrap();
        assert_eq!(
            file.public_info().unwrap(),
            PublicKeyInfo {
                account,
                crypto_type: CryptoType::Ed25519
            }
        );
        for malformed in [
            format!("{input} trailing"),
            format!("{},\"partial\":", &input[..input.len() - 1]),
        ] {
            fs::write(&path, malformed).unwrap();
            assert!(matches!(
                file.public_info(),
                Err(WalletError::PublicKeyUnavailable(_))
            ));
        }
        fs::write(&path, format!(r#"{{"ss58Address":null," ss58Address":"{address}","publicKey":false,"accountId":"{}"}}"#, hex::encode(account.0))).unwrap();
        assert!(matches!(
            file.public_info(),
            Err(WalletError::PublicKeyUnavailable(_))
        ));
        fs::write(
            &path,
            format!(r#"{{" ss58Address":"{address}","cryptoType":0}}"#),
        )
        .unwrap();
        assert_eq!(file.public_info().unwrap().account, account);
        fs::write(
            &path,
            format!(r#"{{"accountId":"0x{}"}}"#, hex::encode(account.0)),
        )
        .unwrap();
        assert_eq!(file.public_info().unwrap().account, account);
        fs::write(
            &path,
            format!(r#"{{"ss58Address":"{address}","cryptoType":99}}"#),
        )
        .unwrap();
        assert_eq!(
            file.public_info().unwrap_err().to_string(),
            "crypto error: unsupported public keyfile crypto type"
        );
    }

    #[test]
    fn bounded_wallet_reads_reject_large_and_nonregular_files() {
        let root = TempRoot::new();
        let path = root.path().join("sparse-keyfile");
        let file = File::create(&path).unwrap();
        file.set_len(MAX_KEYFILE_BYTES + 1).unwrap();
        drop(file);
        let error = read_data(&path).unwrap_err();
        assert_eq!(
            error.to_string(),
            "keyfile error: keyfile exceeds 16 MiB limit"
        );
        assert!(read_data(root.path()).is_err());
        assert_eq!(fs::metadata(&path).unwrap().len(), MAX_KEYFILE_BYTES + 1);
    }

    #[cfg(unix)]
    #[test]
    fn wallet_reads_allow_regular_symlinks_and_reject_fifos_promptly() {
        use std::{
            os::unix::fs::OpenOptionsExt,
            process::Command,
            time::{Duration, Instant},
        };
        let root = TempRoot::new();
        let regular = root.path().join("regular");
        let link = root.path().join("link");
        fs::write(&regular, b"public-test-value").unwrap();
        std::os::unix::fs::symlink(&regular, &link).unwrap();
        assert_eq!(read_data(&link).unwrap().as_slice(), b"public-test-value");

        let fifo = root.path().join("fifo");
        assert!(
            Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .unwrap()
                .success()
        );
        // Release an accidentally blocking implementation, so a regression
        // fails an assertion without leaving a worker or fixture behind.
        let rescue_path = fifo.clone();
        let rescue = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(500));
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(rescue_path)
                .unwrap();
            drop(file);
        });
        let started = Instant::now();
        let result = read_data(&fifo);
        let elapsed = started.elapsed();
        rescue.join().unwrap();
        assert!(result.is_err());
        assert!(elapsed < Duration::from_millis(400));
    }
}
