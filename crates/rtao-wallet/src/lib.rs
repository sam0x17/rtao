//! Btcli-compatible keyfiles, public wallet views and explicit password sources.
#![deny(missing_docs)]

mod files;
mod messages;
mod password;
#[cfg(test)]
#[path = "../tests/support/mod.rs"]
mod testsupport;
mod wallet;

pub use files::read_data as read_keyfile_data;
pub use files::{ColdkeyFile, HotkeyFile, Overwrite, PublicKeyInfo, WriteOptions};
#[cfg(feature = "encrypted")]
pub use messages::{EncryptedMessage, encrypt_message};
pub use messages::{SignedMessage, verify_message};
pub use password::{
    NativePasswordFuture, NativePasswordProvider, PasswordPrompt, PasswordSource, PasswordSources,
    PromptPolicy, env_var_name, resolve_wallet_password,
};
pub use secrecy::{ExposeSecret, SecretString};
pub use wallet::{GeneratedKey, RegenerateFrom, Wallet, WalletRef};

use rtao_core::CoreError;
use std::{fmt, path::PathBuf};

/// Failure to inspect, create or unlock a wallet file.
#[derive(Debug)]
pub enum WalletError {
    /// A filesystem operation failed at this exact path.
    Io {
        /// The configured file or directory.
        path: PathBuf,
        /// The operating system's error.
        error: std::io::Error,
    },
    /// An upstream keyfile or key derivation error.
    Core(CoreError),
    /// Wallet/hotkey names violate the compound-name contract.
    InvalidName(String),
    /// A destination exists and replacement was not explicitly allowed.
    AlreadyExists(PathBuf),
    /// The requested operation needs an optional capability.
    FeatureRequired(&'static str),
    /// No password source supplied a usable password.
    PasswordRequired,
    /// Decryption rejected the first chosen password source.
    WrongPassword {
        /// The first password source selected, with no secret value.
        source: PasswordSource,
    },
    /// A CLI prompt was requested without an eligible terminal or callback.
    PromptUnavailable,
    /// An explicit native password operation failed with a fixed diagnostic.
    NativePassword(&'static str),
    /// The requested Keychain wallet account did not contain a password.
    KeychainMissing(String),
    /// Blocking work could not finish.
    Blocking(String),
    /// A keyfile lacks the public fields needed for a public-only view.
    PublicKeyUnavailable(PathBuf),
    /// A symlink would redirect a keyfile write.
    Symlink(PathBuf),
}
impl fmt::Display for WalletError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, error } => write!(f, "keyfile at {path:?}: {error}"),
            Self::Core(error) => error.fmt(f),
            Self::InvalidName(reason) | Self::Blocking(reason) => f.write_str(reason),
            Self::AlreadyExists(path) => {
                write!(f, "refusing to overwrite existing keyfile {path:?}")
            }
            Self::FeatureRequired(feature) => write!(f, "wallet operation requires {feature}"),
            Self::PasswordRequired => f.write_str("password required to decrypt keyfile"),
            Self::WrongPassword { source } => write!(f, "wrong password ({source})"),
            Self::PromptUnavailable => {
                f.write_str("wallet password prompt requires a terminal and a CLI prompt callback")
            }
            Self::NativePassword(message) => f.write_str(message),
            Self::KeychainMissing(wallet_name) => write!(
                f,
                "no Keychain password for wallet {}; run `rtao wallet keychain save -w {wallet_name}`",
                rtao_primitives::pyfmt::string_repr(wallet_name)
            ),
            Self::PublicKeyUnavailable(path) => write!(f, "public key unavailable at {path:?}"),
            Self::Symlink(path) => write!(f, "refusing to write keyfile through symlink {path:?}"),
        }
    }
}
impl std::error::Error for WalletError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { error, .. } => Some(error),
            Self::Core(error) => Some(error),
            _ => None,
        }
    }
}
impl From<CoreError> for WalletError {
    fn from(error: CoreError) -> Self {
        Self::Core(error)
    }
}
impl From<rtao_core::codec::Error> for WalletError {
    fn from(error: rtao_core::codec::Error) -> Self {
        Self::Core(error.into())
    }
}
pub(crate) fn io_error(path: &std::path::Path, error: std::io::Error) -> WalletError {
    WalletError::Io {
        path: path.to_owned(),
        error,
    }
}
pub(crate) fn signer_error(error: WalletError) -> rtao_signer::SignerError {
    match error {
        WalletError::Core(error) => rtao_signer::SignerError::Core(error),
        error => rtao_signer::SignerError::KeyView(error.to_string()),
    }
}
