// Derived from RaoFoundation/subtensor sdk/python/bittensor/keyfiles.py and signing.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
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
/// Creation/regeneration options; an explicit password forces encryption.
#[derive(Clone, Debug)]
pub struct WriteOptions {
    /// Encrypt using the core's btcli-compatible NaCl format.
    pub encrypt: bool,
    /// Password for encryption, otherwise resolved from the file's sources.
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
    signer: Mutex<Option<Arc<dyn Signer>>>,
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
            signer: Mutex::new(None),
        })
    }
    pub(crate) async fn unlock(&self) -> Result<Arc<dyn Signer>, WalletError> {
        let mut cached = self.signer.lock().await;
        if let Some(signer) = cached.as_ref() {
            return Ok(signer.clone());
        }
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
        let signer: Arc<dyn Signer> = Arc::new(
            KeypairSigner::new(keypair)
                .map_err(|error| WalletError::Blocking(error.to_string()))?,
        );
        *cached = Some(signer.clone());
        Ok(signer)
    }
    pub(crate) async fn store(
        &self,
        keypair: Keypair,
        options: WriteOptions,
    ) -> Result<AccountId, WalletError> {
        let mut cached = self.signer.lock().await;
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
        // A failed filesystem operation may already have installed the secret
        // file. Never retain a signer for the previous on-disk key after a write.
        *cached = None;
        rtao_core::offload(move || {
            let public = Keypair::new(
                None,
                Some(&keypair.public_key_bytes()),
                keypair.crypto_type(),
                keypair.ss58_format(),
            )?;
            let bytes = Zeroizing::new(keyfile_json::serialized_keypair_to_keyfile_data(&keypair)?);
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
        })
        .await
        .map_err(|error| WalletError::Blocking(error.to_string()))??;
        *cached = None;
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
            signer: Mutex::new(None),
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
            signer: Mutex::new(None),
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

pub(crate) fn read_data(path: &Path) -> Result<Zeroizing<Vec<u8>>, WalletError> {
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
    let mut bytes = Zeroizing::new(Vec::new());
    file.take(MAX_KEYFILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| io_error(path, error))?;
    if bytes.len() as u64 > MAX_KEYFILE_BYTES {
        return Err(oversized());
    }
    Ok(bytes)
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
    let value = PublicJson(
        serde_json::from_slice(&bytes)
            .map_err(|_| WalletError::PublicKeyUnavailable(path.to_owned()))?,
    );
    let crypto_type = value
        .0
        .get("cryptoType")
        .and_then(|value| match value {
            serde_json::Value::Number(number) => number.to_string().parse::<u8>().ok(),
            serde_json::Value::String(text) => text.trim().parse::<u8>().ok(),
            _ => None,
        })
        .unwrap_or(1);
    let crypto_type = CryptoType::try_from(crypto_type).map_err(|_| {
        WalletError::Core(rtao_core::CoreError::Crypto(
            "unsupported public keyfile crypto type".into(),
        ))
    })?;
    if let Some(address) = value
        .0
        .get("ss58Address")
        .or_else(|| value.0.get(" ss58Address"))
        .and_then(serde_json::Value::as_str)
    {
        return address
            .parse()
            .map(|account| PublicKeyInfo {
                account,
                crypto_type,
            })
            .map_err(WalletError::from);
    }
    if let Some(key) = value
        .0
        .get("publicKey")
        .or_else(|| value.0.get("accountId"))
        .and_then(serde_json::Value::as_str)
    {
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
}
struct PublicJson(serde_json::Value);
impl Drop for PublicJson {
    fn drop(&mut self) {
        fn clear(value: &mut serde_json::Value) {
            match value {
                serde_json::Value::String(text) => text.zeroize(),
                serde_json::Value::Array(values) => values.iter_mut().for_each(clear),
                serde_json::Value::Object(values) => values.values_mut().for_each(clear),
                _ => {}
            }
        }
        clear(&mut self.0);
    }
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
fn private_dirs(path: &Path) -> Result<(), WalletError> {
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
mod read_security_tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use crate::testsupport::TempRoot;

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
