// Derived from RaoFoundation/subtensor sdk/python/bittensor/intents/base.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/signing.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Signing roles and public key views that never unlock a private key.

use std::{fmt, future::Future, str::FromStr, sync::Arc};

use crate::{AccountId, BoxFuture, Signer, SignerError};

/// A signing role chosen at runtime.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum SignerRole {
    /// Owner transactions, normally signed by the wallet coldkey.
    Coldkey,
    /// Neuron transactions, normally signed by the wallet hotkey.
    Hotkey,
}

impl SignerRole {
    /// The exact Python role token.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Coldkey => "coldkey",
            Self::Hotkey => "hotkey",
        }
    }
}

impl fmt::Display for SignerRole {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for SignerRole {
    type Err = SignerError;
    fn from_str(role: &str) -> Result<Self, Self::Err> {
        match role {
            "coldkey" => Ok(Self::Coldkey),
            "hotkey" => Ok(Self::Hotkey),
            _ => Err(SignerError::InvalidRole(role.into())),
        }
    }
}

impl TryFrom<&str> for SignerRole {
    type Error = SignerError;
    fn try_from(role: &str) -> Result<Self, Self::Error> {
        role.parse()
    }
}

mod sealed {
    pub trait Sealed {}
}

/// Sealed marker selecting a key source at compile time.
pub trait Role: sealed::Sealed + Send + Sync + 'static {
    /// Runtime equivalent of the role.
    const KIND: SignerRole;
}

/// Compile-time coldkey signing role.
pub enum Coldkey {}
/// Compile-time hotkey signing role.
pub enum Hotkey {}

impl sealed::Sealed for Coldkey {}
impl sealed::Sealed for Hotkey {}
impl Role for Coldkey {
    const KIND: SignerRole = SignerRole::Coldkey;
}
impl Role for Hotkey {
    const KIND: SignerRole = SignerRole::Hotkey;
}

/// Public attached-key access, without private-key unlocks.
///
/// Wallet sources expose their attached keys. A bare signer carries one own
/// identity but no attached keys; use [`IntentKeyView`] when role-dependent
/// fallback is needed. Public access must never call `KeySource::signer`.
pub trait KeyView: Send + Sync {
    /// Attached hotkey account; missing keys return `KeyUnavailable(Hotkey)`.
    fn hotkey_address(&self) -> Result<AccountId, SignerError>;
    /// Attached hotkey's raw public key.
    fn hotkey_public_key(&self) -> Result<[u8; 32], SignerError>;
    /// Attached coldkey public account, without unlocking its secret.
    fn coldkey_address(&self) -> Result<AccountId, SignerError>;
    /// A bare signer's own identity, available for matching-role intent fallback.
    fn bare_signer_account(&self) -> Option<AccountId> {
        None
    }
}

/// Compile-time key-source selection for one signing role.
///
/// Role-specific sources expose only the supported marker. For example, a
/// hotkey file cannot satisfy a coldkey-only operation:
///
/// ```compile_fail
/// use rtao_signer::{AccountId, Coldkey, Hotkey, KeySource, KeyView, Signer, SignerError};
/// use std::sync::Arc;
/// struct HotkeyOnly;
/// impl KeyView for HotkeyOnly {
///     fn hotkey_address(&self) -> Result<AccountId, SignerError> { Ok(AccountId([1; 32])) }
///     fn hotkey_public_key(&self) -> Result<[u8; 32], SignerError> { Ok([1; 32]) }
///     fn coldkey_address(&self) -> Result<AccountId, SignerError> { Ok(AccountId([2; 32])) }
/// }
/// impl KeySource<Hotkey> for HotkeyOnly {
///     fn address(&self) -> Result<AccountId, SignerError> { self.hotkey_address() }
///     async fn signer(&self) -> Result<Arc<dyn Signer>, SignerError> { unimplemented!() }
/// }
/// fn owner_operation(_: &impl KeySource<Coldkey>) {}
/// owner_operation(&HotkeyOnly);
/// ```
pub trait KeySource<R: Role>: KeyView {
    /// The role's public account, obtained without unlocking.
    fn address(&self) -> Result<AccountId, SignerError>;
    /// Obtain the signer, unlocking lazily if necessary.
    fn signer(&self) -> impl Future<Output = Result<Arc<dyn Signer>, SignerError>> + Send;
}

/// Object-safe role selection for command-line and dynamic dispatch.
pub trait DynKeySource: KeyView {
    /// Obtain the signer for a runtime role, or return a typed unsupported-role error.
    fn signer_for<'a>(
        &'a self,
        role: SignerRole,
    ) -> BoxFuture<'a, Result<Arc<dyn Signer>, SignerError>>;
}

impl<S: Signer + ?Sized> KeyView for S {
    fn hotkey_address(&self) -> Result<AccountId, SignerError> {
        Err(SignerError::KeyUnavailable(SignerRole::Hotkey))
    }
    fn hotkey_public_key(&self) -> Result<[u8; 32], SignerError> {
        Err(SignerError::KeyUnavailable(SignerRole::Hotkey))
    }
    fn coldkey_address(&self) -> Result<AccountId, SignerError> {
        Err(SignerError::KeyUnavailable(SignerRole::Coldkey))
    }
    fn bare_signer_account(&self) -> Option<AccountId> {
        Some(self.account())
    }
}

impl<S: Signer + Clone, R: Role> KeySource<R> for S {
    fn address(&self) -> Result<AccountId, SignerError> {
        Ok(self.account())
    }
    async fn signer(&self) -> Result<Arc<dyn Signer>, SignerError> {
        Ok(Arc::new(self.clone()))
    }
}

impl<S: Signer + Clone> DynKeySource for S {
    fn signer_for<'a>(
        &'a self,
        _role: SignerRole,
    ) -> BoxFuture<'a, Result<Arc<dyn Signer>, SignerError>> {
        Box::pin(async move { Ok(Arc::new(self.clone()) as Arc<dyn Signer>) })
    }
}

/// Public intent key defaults with Python's matching-role fallback.
///
/// A bare signer can sign either role, but its identity supplies a default only
/// when the requested key has the intent's signing role. Attached wallet keys
/// take precedence over fallback, and an explicit hotkey override wins first.
pub struct IntentKeyView<'a> {
    keys: &'a dyn KeyView,
    signer_role: SignerRole,
    op: &'a str,
}

impl<'a> IntentKeyView<'a> {
    /// Bind a public key view to the intent's signing role and operation name.
    pub fn new(keys: &'a dyn KeyView, signer_role: SignerRole, op: &'a str) -> Self {
        Self {
            keys,
            signer_role,
            op,
        }
    }

    /// Resolve an explicit hotkey or the attached/matching-role default.
    pub fn hotkey_address(
        &self,
        override_address: Option<AccountId>,
    ) -> Result<AccountId, SignerError> {
        if let Some(address) = override_address {
            return Ok(address);
        }
        match self.keys.hotkey_address() {
            Ok(address) => Ok(address),
            Err(SignerError::KeyUnavailable(SignerRole::Hotkey)) => {
                if self.signer_role == SignerRole::Hotkey
                    && let Some(address) = self.keys.bare_signer_account()
                {
                    return Ok(address);
                }
                Err(SignerError::KeyView(format!(
                    "{}: this signer carries no hotkey to default to; pass the hotkey address explicitly or sign with a Wallet",
                    self.op
                )))
            }
            Err(error) => Err(error),
        }
    }

    /// Resolve an attached hotkey public key or matching-role bare signer key.
    pub fn hotkey_public_key(&self) -> Result<[u8; 32], SignerError> {
        match self.keys.hotkey_public_key() {
            Ok(key) => Ok(key),
            Err(SignerError::KeyUnavailable(SignerRole::Hotkey)) => {
                if self.signer_role == SignerRole::Hotkey
                    && let Some(account) = self.keys.bare_signer_account()
                {
                    return Ok(account.0);
                }
                Err(SignerError::KeyView(format!(
                    "{}: this signer carries no hotkey public key; sign with a Wallet or a hotkey Signer",
                    self.op
                )))
            }
            Err(error) => Err(error),
        }
    }

    /// Resolve the attached coldkey public view or matching-role bare identity.
    pub fn coldkey_address(&self) -> Result<AccountId, SignerError> {
        match self.keys.coldkey_address() {
            Ok(address) => Ok(address),
            Err(SignerError::KeyUnavailable(SignerRole::Coldkey)) => {
                if self.signer_role == SignerRole::Coldkey
                    && let Some(address) = self.keys.bare_signer_account()
                {
                    return Ok(address);
                }
                Err(SignerError::KeyView(format!(
                    "{}: this signer carries no coldkey to reference; sign with a Wallet or a coldkey Signer",
                    self.op
                )))
            }
            Err(error) => Err(error),
        }
    }
}
