// Derived from RaoFoundation/subtensor sdk/python/bittensor/keyfiles.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Generic keyfiles and explicit password/environment interaction.

use crate::{
    PasswordPrompt, PasswordSource, PromptPolicy, SecretString, WalletError, WriteOptions,
    files::{atomic_write, private_dirs, read_data},
};
use rtao_core::{keyfile_json, keys::Keypair};
use secrecy::ExposeSecret;
use std::{
    collections::BTreeMap,
    fmt,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};
use zeroize::Zeroizing;

/// An explicitly owned environment for encoded keyfile passwords.
///
/// Rust applications may already have background threads, so changing their
/// global process environment is unsafe. Implementations must instead own the
/// environment they mutate, such as a child-process environment or an overlay.
/// Encoded values use Python's reversible XOR/base64 format and remain secrets.
pub trait KeyfileEnvironment: Send + Sync {
    /// Read an encoded value, without rendering it in errors or logs.
    fn get(&self, name: &str) -> Result<Option<SecretString>, WalletError>;
    /// Store an encoded value in this explicitly owned environment.
    fn set(&self, name: &str, value: SecretString) -> Result<(), WalletError>;
    /// Remove an encoded value from this explicitly owned environment.
    fn remove(&self, name: &str) -> Result<(), WalletError>;
}

/// A thread-safe password environment that reads through to process variables.
///
/// Writes remain in the overlay. A removal masks the process value too. Clones
/// of a keyfile or password configuration share the same overlay through `Arc`.
/// Other processes and unrelated keyfiles do not acquire its saved passwords.
#[derive(Default)]
pub struct KeyfileEnvironmentOverlay {
    values: RwLock<BTreeMap<String, Option<SecretString>>>,
}
impl fmt::Debug for KeyfileEnvironmentOverlay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KeyfileEnvironmentOverlay")
            .field("values", &"[REDACTED]")
            .finish()
    }
}
impl KeyfileEnvironment for KeyfileEnvironmentOverlay {
    fn get(&self, name: &str) -> Result<Option<SecretString>, WalletError> {
        let values = self.values.read().map_err(|_| environment_lock_error())?;
        if let Some(value) = values.get(name) {
            return Ok(value.clone());
        }
        Ok(std::env::var(name).ok().map(SecretString::new))
    }
    fn set(&self, name: &str, value: SecretString) -> Result<(), WalletError> {
        validate_environment_name(name)?;
        self.values
            .write()
            .map_err(|_| environment_lock_error())?
            .insert(name.to_owned(), Some(value));
        Ok(())
    }
    fn remove(&self, name: &str) -> Result<(), WalletError> {
        self.values
            .write()
            .map_err(|_| environment_lock_error())?
            .insert(name.to_owned(), None);
        Ok(())
    }
}
fn environment_lock_error() -> WalletError {
    WalletError::Blocking("keyfile password environment lock failed".into())
}
fn validate_environment_name(name: &str) -> Result<(), WalletError> {
    if name.is_empty() || name.contains(['=', '\0']) {
        return Err(WalletError::Core(rtao_core::CoreError::Keyfile(
            "invalid keyfile password environment name".into(),
        )));
    }
    Ok(())
}

/// Inputs for a generic key password, independent of wallet-wide password policy.
///
/// Resolution is nonempty explicit input, then the chosen encoded environment
/// variable, then an explicitly enabled hidden-input callback. Wallet globals,
/// password files and native dialogs are deliberately separate SDK operations.
#[derive(Clone)]
pub struct KeyPasswordSources {
    /// Explicit input; an empty value falls through as in Python.
    pub password: Option<SecretString>,
    /// An encoded password variable; `None` or an empty name skips this source.
    pub env_var_name: Option<String>,
    /// Message supplied to the hidden-entry callback.
    pub prompt_message: String,
    /// Ask a second time only when the selected source is the prompt.
    pub confirm: bool,
    /// Whether the explicitly supplied callback may run; defaults to never.
    pub prompt: PromptPolicy,
    /// Hidden input and optional fixed validation diagnostics supplied by the caller.
    pub prompt_callback: Option<Arc<dyn PasswordPrompt>>,
    /// The environment this operation can read or mutate.
    pub environment: Arc<dyn KeyfileEnvironment>,
}
impl Default for KeyPasswordSources {
    fn default() -> Self {
        Self {
            password: None,
            env_var_name: None,
            prompt_message: "Enter password to unlock key: ".into(),
            confirm: false,
            prompt: PromptPolicy::Never,
            prompt_callback: None,
            environment: Arc::new(KeyfileEnvironmentOverlay::default()),
        }
    }
}
impl fmt::Debug for KeyPasswordSources {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KeyPasswordSources")
            .field("password", &self.password.as_ref().map(|_| "[REDACTED]"))
            .field("env_var_name", &self.env_var_name)
            .field("prompt_message", &self.prompt_message)
            .field("confirm", &self.confirm)
            .field("prompt", &self.prompt)
            .field(
                "prompt_callback",
                &self.prompt_callback.as_ref().map(|_| "configured"),
            )
            .field("environment", &"configured")
            .finish()
    }
}

/// Resolve a generic key password with Python's explicit/environment/prompt order.
///
/// An unavailable password returns `None` when prompting is disabled. Empty or
/// mismatched interactive entries retry only if the supplied callback reports a
/// terminal; nonterminal callers receive a fixed error without leaking input.
#[doc(alias = "bittensor.keyfiles.resolve_key_password")]
pub async fn resolve_key_password(
    sources: &KeyPasswordSources,
) -> Result<Option<SecretString>, WalletError> {
    Ok(resolve_key_password_with_source(sources)
        .await?
        .map(|resolved| resolved.secret))
}
async fn resolve_key_password_with_source(
    sources: &KeyPasswordSources,
) -> Result<Option<crate::password::ResolvedPassword>, WalletError> {
    use crate::password::ResolvedPassword;
    if let Some(secret) = &sources.password
        && !secret.expose_secret().is_empty()
    {
        return Ok(Some(ResolvedPassword {
            secret: secret.clone(),
            source: PasswordSource::Explicit,
        }));
    }
    if let Some(name) = sources
        .env_var_name
        .as_deref()
        .filter(|name| !name.is_empty())
        && let Some(encoded) = sources.environment.get(name)?
    {
        let secret = rtao_core::keyfile_env::decode_password_from_environment(
            name,
            encoded.expose_secret(),
        )?;
        if !secret.is_empty() {
            return Ok(Some(ResolvedPassword {
                secret: SecretString::new(secret),
                source: PasswordSource::KeyfileEnvironment,
            }));
        }
    }
    if sources.prompt == PromptPolicy::Never {
        return Ok(None);
    }
    let prompt = sources
        .prompt_callback
        .clone()
        .ok_or(WalletError::PromptUnavailable)?;
    if sources.prompt == PromptPolicy::IfTty && !prompt.is_terminal() {
        return Err(WalletError::PromptUnavailable);
    }
    let message = sources.prompt_message.clone();
    let confirm = sources.confirm;
    let secret = rtao_core::offload(move || {
        loop {
            let password = prompt.prompt(&message)?;
            if password.expose_secret().is_empty() {
                if !prompt.is_terminal() {
                    return Err(WalletError::PasswordEmpty);
                }
                prompt.invalid_password("password cannot be empty")?;
                continue;
            }
            if confirm {
                let again = prompt.prompt("Retype password: ")?;
                if again.expose_secret() != password.expose_secret() {
                    if !prompt.is_terminal() {
                        return Err(WalletError::PasswordMismatch);
                    }
                    prompt.invalid_password("passwords do not match")?;
                    continue;
                }
            }
            return Ok(password);
        }
    })
    .await
    .map_err(|_| WalletError::Blocking("keyfile password callback task failed".into()))??;
    Ok(Some(ResolvedPassword {
        secret,
        source: PasswordSource::Prompt,
    }))
}

/// A single generic keyfile, including secret files and public-only companions.
///
/// Unlike [`crate::ColdkeyFile`] and [`crate::HotkeyFile`], this type has no
/// signing role and never creates or reads a companion file automatically.
/// Reads are bounded to 16 MiB, writes are atomic and owner-only, overwrites
/// need explicit permission, and prompting is disabled by default.
#[derive(Clone)]
#[doc(alias = "bittensor.keyfiles.Keyfile")]
pub struct Keyfile {
    path: PathBuf,
    name: String,
    should_save_to_env: bool,
    sources: KeyPasswordSources,
}
impl fmt::Debug for Keyfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Keyfile")
            .field("path", &self.path)
            .field("name", &self.name)
            .finish()
    }
}
impl Keyfile {
    /// Configure an exact path without reading or expanding it.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        Self {
            path,
            name,
            should_save_to_env: false,
            sources: KeyPasswordSources::default(),
        }
    }
    /// Borrow the exact configured path.
    pub fn path(&self) -> &Path {
        &self.path
    }
    /// Borrow the display name, initially the path's final component.
    pub fn name(&self) -> &str {
        &self.name
    }
    /// Supply a display name; an empty name keeps the existing name.
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        let name = name.into();
        if !name.is_empty() {
            self.name = name;
        }
        self
    }
    /// Whether a successful encrypted write will save its password.
    pub fn should_save_to_env(&self) -> bool {
        self.should_save_to_env
    }
    /// Save encrypted-write passwords into the explicitly owned environment.
    pub fn with_save_to_env(mut self, enabled: bool) -> Self {
        self.should_save_to_env = enabled;
        self
    }
    /// Configure generic password callbacks and the owned environment.
    ///
    /// Reads select this file's encoded variable and a path-specific prompt;
    /// writes skip environment lookup and confirm newly prompted passwords.
    pub fn with_password_sources(mut self, sources: KeyPasswordSources) -> Self {
        self.sources = sources;
        self
    }
    /// Borrow the configured generic password policy and owned environment.
    pub fn password_sources(&self) -> &KeyPasswordSources {
        &self.sources
    }
    /// Return Python's environment variable name for this exact path.
    pub fn env_var_name(&self) -> String {
        crate::env_var_name(&self.path)
    }
    /// Whether the path currently exists, following links as Python does.
    pub fn exists_on_device(&self) -> bool {
        filesystem_path(&self.path).exists()
    }
    /// Whether the current user can read the path, following links.
    pub fn is_readable(&self) -> bool {
        path_access(filesystem_path(&self.path), AccessMode::Read)
    }
    /// Whether the path, or the parent of an absent path, is writable.
    pub fn is_writable(&self) -> bool {
        if self.exists_on_device() {
            path_access(filesystem_path(&self.path), AccessMode::Write)
        } else {
            path_access(parent_directory(&self.path), AccessMode::Write)
        }
    }
    /// Create missing private parent directories without opening the keyfile.
    pub fn make_dirs(&self) -> Result<(), WalletError> {
        let parent = parent_directory(&self.path);
        for ancestor in parent.ancestors() {
            if std::fs::symlink_metadata(ancestor)
                .is_ok_and(|metadata| metadata.file_type().is_symlink())
            {
                return Err(WalletError::Symlink(ancestor.to_owned()));
            }
        }
        private_dirs(parent)
    }
    /// Detect the pinned core's encryption formats without asking for a password.
    ///
    /// Missing or unreadable paths return false. Other read errors, including
    /// oversized/nonregular inputs, remain errors rather than hidden failures.
    pub fn is_encrypted(&self) -> Result<bool, WalletError> {
        if !self.exists_on_device() || !self.is_readable() {
            return Ok(false);
        }
        Ok(keyfile_json::keyfile_data_is_encrypted(&read_data(
            filesystem_path(&self.path),
        )?))
    }
    /// Read and decode one keypair without a retained signing cache.
    ///
    /// Public-only files return public-only keypairs. Secret byte buffers are
    /// wiped on drop, and an incorrect first password source is not retried.
    pub async fn get_keypair(
        &self,
        password: Option<SecretString>,
    ) -> Result<Keypair, WalletError> {
        let path = filesystem_path(&self.path).to_owned();
        let bytes = rtao_core::offload(move || read_data(&path))
            .await
            .map_err(|_| WalletError::Blocking("keyfile read task failed".into()))??;
        let resolved = if keyfile_json::keyfile_data_is_encrypted(&bytes) {
            #[cfg(not(feature = "encrypted"))]
            return Err(WalletError::FeatureRequired("encrypted"));
            #[cfg(feature = "encrypted")]
            {
                let mut sources = self.sources.clone();
                if password.is_some() {
                    sources.password = password;
                }
                sources.env_var_name = Some(self.env_var_name());
                sources.prompt_message =
                    format!("Enter password to unlock key {}: ", self.path.display());
                sources.confirm = false;
                Some(
                    resolve_key_password_with_source(&sources)
                        .await?
                        .ok_or(WalletError::PasswordRequired)?,
                )
            }
        } else {
            None
        };
        #[cfg(not(feature = "encrypted"))]
        let _ = password;
        rtao_core::offload(move || {
            #[cfg(feature = "encrypted")]
            let bytes = if let Some(resolved) = resolved {
                match rtao_core::keyfiles::decrypt_keyfile_data(
                    &bytes,
                    Some(resolved.secret.expose_secret()),
                ) {
                    Ok(plain) => Zeroizing::new(plain),
                    Err(error) => {
                        let error = rtao_core::CoreError::from(error);
                        return Err(match error {
                            rtao_core::CoreError::WrongPassword(_) => WalletError::WrongPassword {
                                source: resolved.source,
                            },
                            error => WalletError::Core(error),
                        });
                    }
                }
            } else {
                bytes
            };
            #[cfg(not(feature = "encrypted"))]
            let _: Option<crate::password::ResolvedPassword> = resolved;
            keyfile_json::deserialize_keypair_from_keyfile_data(&bytes).map_err(WalletError::from)
        })
        .await
        .map_err(|_| WalletError::Blocking("keyfile decode task failed".into()))?
    }
    /// Atomically store exactly one file, without creating a public companion.
    ///
    /// The generic Python keyfile honors `options.encrypt` even if a password is
    /// supplied. This differs from role-specific wallet creation, where an
    /// explicit password forces encryption. Encrypted writes resolve an explicit
    /// password or an enabled confirming prompt, never a preexisting env value.
    /// A password is saved only after the filesystem write succeeds.
    pub async fn set_keypair(
        &self,
        keypair: Keypair,
        options: WriteOptions,
    ) -> Result<(), WalletError> {
        let password: Option<SecretString> = if options.encrypt {
            #[cfg(not(feature = "encrypted"))]
            return Err(WalletError::FeatureRequired("encrypted"));
            #[cfg(feature = "encrypted")]
            {
                let mut sources = self.sources.clone();
                if options.password.is_some() {
                    sources.password = options.password;
                }
                sources.env_var_name = None;
                sources.prompt_message =
                    format!("Enter password to encrypt key {}: ", self.path.display());
                sources.confirm = sources.password.is_none();
                Some(
                    resolve_key_password(&sources)
                        .await?
                        .ok_or(WalletError::PasswordRequired)?,
                )
            }
        } else {
            None
        };
        let save_password = if self.should_save_to_env {
            password.clone()
        } else {
            None
        };
        let path = filesystem_path(&self.path).to_owned();
        rtao_core::offload(move || {
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
            atomic_write(&path, &bytes, options.overwrite)
        })
        .await
        .map_err(|_| WalletError::Blocking("keyfile write task failed".into()))??;
        if let Some(password) = save_password {
            self.save_password_to_env(Some(password)).await?;
        }
        Ok(())
    }
    /// Resolve and save a password in the explicitly owned environment.
    ///
    /// Returns the variable name, as in Python. Existing encoded values are not
    /// consulted; an empty explicit value falls through to an enabled prompt.
    pub async fn save_password_to_env(
        &self,
        password: Option<SecretString>,
    ) -> Result<String, WalletError> {
        let mut sources = self.sources.clone();
        if password.is_some() {
            sources.password = password;
        }
        sources.env_var_name = None;
        sources.prompt_message = format!(
            "Enter password to store in environment for {}: ",
            self.path.display()
        );
        sources.confirm = false;
        let password = resolve_key_password(&sources)
            .await?
            .ok_or(WalletError::PasswordRequired)?;
        let name = self.env_var_name();
        let encoded = SecretString::new(rtao_core::keyfile_env::encode_password_for_environment(
            &name,
            password.expose_secret(),
        )?);
        self.sources.environment.set(&name, encoded)?;
        Ok(name)
    }
    /// Remove this file's password from its explicitly owned environment.
    pub fn remove_password_from_env(&self) -> Result<(), WalletError> {
        self.sources.environment.remove(&self.env_var_name())
    }
}

fn parent_directory(path: &Path) -> &Path {
    path.parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn filesystem_path(path: &Path) -> &Path {
    if path.as_os_str().is_empty() {
        Path::new(".")
    } else {
        path
    }
}

enum AccessMode {
    Read,
    Write,
}
#[cfg(unix)]
fn path_access(path: &Path, mode: AccessMode) -> bool {
    let mode = match mode {
        AccessMode::Read => rustix::fs::Access::READ_OK,
        AccessMode::Write => rustix::fs::Access::WRITE_OK,
    };
    rustix::fs::access(path, mode).is_ok()
}
#[cfg(windows)]
fn path_access(path: &Path, mode: AccessMode) -> bool {
    // Python's Windows os.access follows GetFileAttributes: existence is
    // sufficient for read access, and the read-only bit restricts only files.
    std::fs::metadata(path).is_ok_and(|metadata| match mode {
        AccessMode::Read => true,
        AccessMode::Write => metadata.is_dir() || !metadata.permissions().readonly(),
    })
}
#[cfg(not(any(unix, windows)))]
fn path_access(path: &Path, mode: AccessMode) -> bool {
    match mode {
        AccessMode::Read => std::fs::File::open(path).is_ok(),
        AccessMode::Write => {
            std::fs::metadata(path).is_ok_and(|metadata| !metadata.permissions().readonly())
        }
    }
}
