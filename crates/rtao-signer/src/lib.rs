// Derived from RaoFoundation/subtensor sdk/python/bittensor/_transport/protocols.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/signing.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/_transport/contract.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Bittensor signing contracts and key sources.

#![doc = include_str!("../README.md")]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![deny(missing_docs)]

use std::{fmt, future::Future, pin::Pin, sync::Arc};

#[cfg(any(feature = "vault", feature = "extension"))]
mod browser;

/// Ledger signing through the core device API and injected backend seam.
#[cfg(feature = "ledger")]
#[cfg_attr(docsrs, doc(cfg(feature = "ledger")))]
pub mod ledger;
mod roles;
#[cfg(feature = "ledger")]
#[cfg_attr(docsrs, doc(cfg(feature = "ledger")))]
pub use ledger::{LedgerError, LedgerOptions, LedgerSigner};
#[cfg(feature = "extension")]
#[cfg_attr(docsrs, doc(cfg(feature = "extension")))]
pub mod extension;
#[cfg(feature = "extension")]
#[cfg_attr(docsrs, doc(cfg(feature = "extension")))]
pub use extension::ExtensionSigner;
#[cfg(feature = "vault")]
#[cfg_attr(docsrs, doc(cfg(feature = "vault")))]
pub mod vault;
pub use roles::{
    Coldkey, DynKeySource, Hotkey, IntentKeyView, KeySource, KeyView, Role, SignerRole,
};
#[cfg(feature = "vault")]
#[cfg_attr(docsrs, doc(cfg(feature = "vault")))]
pub use vault::VaultSigner;

use rtao_core::keyfile_json::{SecretInput, redacted_derivation_error};
use rtao_core::{CoreError, keys::Keypair};
pub use rtao_primitives::{AccountId, CryptoType};

/// A sendable future at the heterogeneous signer boundary.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

bitflags::bitflags! {
    /// The signing formats and interactions supported by a signer.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct SignerCaps: u8 {
        /// Signs the prepared, possibly hashed, raw extrinsic payload.
        const RAW_PAYLOAD = 1 << 0;
        /// Requires CheckMetadataHash and an RFC-0078 metadata proof.
        const METADATA_HASH = 1 << 1;
        /// Accepts a Polkadot-JS SignerPayloadJSON object.
        const PAYLOAD_JSON = 1 << 2;
        /// Accepts the entire prepared unsigned extrinsic.
        const UNSIGNED_TX = 1 << 3;
        /// Requires a human round trip, such as a Vault QR exchange.
        const OUT_OF_BAND = 1 << 4;
    }
}

/// Polkadot-JS payload fields, including big-endian hex numbers at SCALE width.
///
/// The transport constructs this object; a raw keypair signs only
/// [`SigningRequest::payload`] and never re-encodes the object.
pub type SignerPayloadJson = serde_json::Map<String, serde_json::Value>;

/// The runtime materials needed by a metadata-verifying signing device.
#[derive(Debug, Clone)]
pub struct SigningContext {
    /// Raw MetadataVersioned bytes, preferably V15.
    pub metadata_bytes: Vec<u8>,
    /// Chain identity and display constants used in the RFC-0078 digest.
    pub chain_info: rtao_core::digest::ChainInfo,
    /// Transaction format version for the target runtime.
    pub transaction_version: u32,
    /// Genesis hash of the target chain.
    pub genesis_hash: [u8; 32],
}

/// The stage of submission for which a signature is requested.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SignPurpose {
    /// An ordinary signed extrinsic.
    Extrinsic,
    /// The signed transaction carried inside a shield envelope.
    ShieldInner,
    /// The signed transaction submitting a shield envelope.
    ShieldCarrier,
}

/// All views of a prepared payload, borrowed for one signing operation.
#[derive(Debug)]
pub struct SigningRequest<'a> {
    /// SCALE-encoded call bytes.
    pub call: &'a [u8],
    /// Signed extension fields included in the extrinsic.
    pub included_in_extrinsic: &'a [u8],
    /// Additional signed extension fields included only in the signing data.
    pub included_in_signed_data: &'a [u8],
    /// Exact bytes to sign, already hashed when the payload exceeds 256 bytes.
    pub payload: &'a [u8],
    /// Extension-signing JSON view of the same extrinsic.
    pub payload_json: &'a SignerPayloadJson,
    /// Runtime context for hardware metadata proofs.
    pub context: &'a SigningContext,
    /// Submission stage shown to the signer.
    pub purpose: SignPurpose,
}

/// A Substrate signature with its algorithm discriminator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MultiSignature {
    /// SCALE MultiSignature variant: ed25519 = 0, sr25519 = 1.
    pub crypto: CryptoType,
    /// The raw 64-byte cryptographic signature.
    pub bytes: [u8; 64],
}

impl MultiSignature {
    /// Encode the algorithm discriminator followed by the signature bytes.
    pub fn to_bytes(self) -> [u8; 65] {
        let mut encoded = [0; 65];
        encoded[0] = u8::from(self.crypto);
        encoded[1..].copy_from_slice(&self.bytes);
        encoded
    }

    /// Decode and validate a device or extension's 65-byte reply.
    pub fn from_bytes(encoded: &[u8]) -> Result<Self, SignerError> {
        if encoded.len() != 65 {
            return Err(SignerError::InvalidSignature(
                "expected a 65-byte MultiSignature".into(),
            ));
        }
        let crypto = CryptoType::try_from(encoded[0]).map_err(|_| {
            SignerError::InvalidSignature("unsupported MultiSignature variant".into())
        })?;
        let mut bytes = [0; 64];
        bytes.copy_from_slice(&encoded[1..]);
        Ok(Self { crypto, bytes })
    }

    /// Decode a reply and require the algorithm negotiated with the signer.
    pub fn from_bytes_for(encoded: &[u8], expected: CryptoType) -> Result<Self, SignerError> {
        let signature = Self::from_bytes(encoded)?;
        if signature.crypto != expected {
            return Err(SignerError::InvalidSignature(
                "MultiSignature variant does not match signer crypto type".into(),
            ));
        }
        Ok(signature)
    }
}

/// Failure to derive or use a signing key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignerError {
    /// This signer cannot sign arbitrary messages without transaction context.
    RawSigningUnsupported,
    /// An upstream derivation or signing operation failed.
    Core(CoreError),
    /// A signing backend returned an invalid signature encoding or algorithm.
    InvalidSignature(String),
    /// The requested token is neither `coldkey` nor `hotkey`.
    InvalidRole(String),
    /// This public source has no attached key for the requested role.
    KeyUnavailable(SignerRole),
    /// The source supports a different signing role.
    RoleUnavailable(SignerRole),
    /// Intent-dependent key defaults cannot be resolved from this public view.
    KeyView(String),
    /// A backend failure with safe, separately rendered remediation.
    WithHelp {
        /// Public diagnostic; never include passwords or captured secret output.
        message: String,
        /// Actionable next step containing no secret values.
        help: String,
    },
    /// A browser-extension signing session failed.
    Extension(String),
    /// An offline Vault signing session failed.
    Vault(String),
}

impl SignerError {
    /// Safe remediation supplied by the failing backend, separately from its message.
    pub fn help(&self) -> Option<&str> {
        match self {
            Self::WithHelp { help, .. } => Some(help),
            _ => None,
        }
    }
}

impl fmt::Display for SignerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RawSigningUnsupported => {
                formatter.write_str("raw message signing is not supported by this signer")
            }
            Self::Core(error) => error.fmt(formatter),
            Self::InvalidSignature(message) => formatter.write_str(message),
            Self::InvalidRole(role) => write!(
                formatter,
                "unknown signer role {role:?}; expected coldkey or hotkey"
            ),
            Self::KeyUnavailable(role) => write!(
                formatter,
                "this key source has no attached {role} public key"
            ),
            Self::RoleUnavailable(role) => {
                write!(formatter, "this key source cannot sign as {role}")
            }
            Self::KeyView(message) => formatter.write_str(message),
            Self::WithHelp { message, .. } => formatter.write_str(message),
            Self::Extension(message) => formatter.write_str(message),
            Self::Vault(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for SignerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Core(error) => Some(error),
            _ => None,
        }
    }
}

impl From<CoreError> for SignerError {
    fn from(error: CoreError) -> Self {
        Self::Core(error)
    }
}

/// An object-safe asynchronous signing seam for software and hardware keys.
pub trait Signer: Send + Sync + 'static {
    /// Public account whose signatures this signer produces.
    fn account(&self) -> AccountId;
    /// Signature algorithm used by the key.
    fn crypto_type(&self) -> CryptoType;
    /// Signing formats supported by this implementation.
    fn caps(&self) -> SignerCaps {
        SignerCaps::RAW_PAYLOAD
    }
    /// Display constants required by this signer's metadata-verifying device.
    ///
    /// `None` uses the submission options. The runtime name, version and SS58
    /// prefix still come from the client; only token display constants vary.
    fn metadata_constants(&self) -> Option<(u8, String)> {
        None
    }
    /// Prepare an interactive signer before shield nonces and mortal eras are chosen.
    ///
    /// Software signers require no preparation. Implementations must not sign
    /// or submit a transaction here; the caller bounds this future by its
    /// existing submission deadline. Preparation must preserve the account
    /// and signature algorithm.
    fn prepare_shielded(&self) -> BoxFuture<'_, Result<(), SignerError>> {
        Box::pin(async { Ok(()) })
    }
    /// Sign a prepared transaction, using the relevant view of the request.
    fn sign<'a>(
        &'a self,
        request: &'a SigningRequest<'a>,
    ) -> BoxFuture<'a, Result<MultiSignature, SignerError>>;
    /// Sign exact arbitrary message bytes, without a `<Bytes>` wrapper.
    ///
    /// Hardware signers refuse this operation by default.
    fn sign_raw<'a>(&'a self, _message: &'a [u8]) -> BoxFuture<'a, Result<[u8; 64], SignerError>> {
        Box::pin(async { Err(SignerError::RawSigningUnsupported) })
    }
}

/// An unlocked pinned-core keypair shared until the final `Arc` owner drops.
///
/// The core zeroizes its separately retained mnemonic and seed buffers. Rtao
/// cannot wipe the opaque signing pair; the pinned ed25519 pair has no guaranteed
/// secret wipe on drop. Borrowed constructor inputs remain the caller's responsibility.
/// Cryptography runs inline. Debug output and constructor error chains never
/// include the seed, URI, mnemonic, password, or private key.
#[derive(Clone)]
pub struct KeypairSigner {
    keypair: Arc<Keypair>,
    crypto: CryptoType,
}

impl KeypairSigner {
    /// Wrap a previously derived or unlocked upstream keypair.
    pub fn new(keypair: Keypair) -> Result<Self, SignerError> {
        let crypto = CryptoType::try_from(keypair.crypto_type())
            .map_err(|_| SignerError::InvalidSignature("unsupported keypair crypto type".into()))?;
        Ok(Self {
            keypair: Arc::new(keypair),
            crypto,
        })
    }

    /// Derive a signer from an upstream secret URI, including dev URIs.
    pub fn from_uri(uri: &str, crypto: CryptoType) -> Result<Self, SignerError> {
        Self::new(
            Keypair::from_uri(uri, u8::from(crypto))
                .map_err(|error| redacted_derivation_error(error, SecretInput::Uri))
                .map_err(CoreError::from)?,
        )
    }

    /// Derive a signer from a BIP39 mnemonic and optional derivation password.
    pub fn from_mnemonic(
        mnemonic: &str,
        crypto: CryptoType,
        password: Option<&str>,
    ) -> Result<Self, SignerError> {
        Self::new(
            Keypair::from_mnemonic(mnemonic, u8::from(crypto), password)
                .map_err(|error| redacted_derivation_error(error, SecretInput::Mnemonic))
                .map_err(CoreError::from)?,
        )
    }

    /// Derive a signer from a raw secret seed using upstream's length rules.
    pub fn from_seed(seed: &[u8], crypto: CryptoType) -> Result<Self, SignerError> {
        Self::new(
            Keypair::from_seed(seed, u8::from(crypto))
                .map_err(|error| redacted_derivation_error(error, SecretInput::Seed))
                .map_err(CoreError::from)?,
        )
    }

    fn signature(&self, message: &[u8]) -> Result<[u8; 64], SignerError> {
        let signature = self.keypair.sign(message).map_err(CoreError::from)?;
        signature.try_into().map_err(|_| {
            SignerError::InvalidSignature("keypair returned a signature other than 64 bytes".into())
        })
    }
}

impl fmt::Debug for KeypairSigner {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("KeypairSigner")
            .field("account", &self.account())
            .field("crypto", &self.crypto)
            .field("keypair", &"[REDACTED]")
            .finish()
    }
}

impl Signer for KeypairSigner {
    fn account(&self) -> AccountId {
        AccountId(self.keypair.public_key_bytes())
    }
    fn crypto_type(&self) -> CryptoType {
        self.crypto
    }
    fn sign<'a>(
        &'a self,
        request: &'a SigningRequest<'a>,
    ) -> BoxFuture<'a, Result<MultiSignature, SignerError>> {
        Box::pin(async move {
            Ok(MultiSignature {
                crypto: self.crypto,
                bytes: self.signature(request.payload)?,
            })
        })
    }
    fn sign_raw<'a>(&'a self, message: &'a [u8]) -> BoxFuture<'a, Result<[u8; 64], SignerError>> {
        Box::pin(async move { self.signature(message) })
    }
}

impl<S: Signer + ?Sized> Signer for Arc<S> {
    fn account(&self) -> AccountId {
        (**self).account()
    }
    fn crypto_type(&self) -> CryptoType {
        (**self).crypto_type()
    }
    fn caps(&self) -> SignerCaps {
        (**self).caps()
    }
    fn metadata_constants(&self) -> Option<(u8, String)> {
        (**self).metadata_constants()
    }
    fn prepare_shielded(&self) -> BoxFuture<'_, Result<(), SignerError>> {
        (**self).prepare_shielded()
    }
    fn sign<'a>(
        &'a self,
        request: &'a SigningRequest<'a>,
    ) -> BoxFuture<'a, Result<MultiSignature, SignerError>> {
        (**self).sign(request)
    }
    fn sign_raw<'a>(&'a self, message: &'a [u8]) -> BoxFuture<'a, Result<[u8; 64], SignerError>> {
        (**self).sign_raw(message)
    }
}
