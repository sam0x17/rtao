// Derived from RaoFoundation/subtensor sdk/python/bittensor/signing.py @ b6a81d144078eac18bd25c3af7cb31579bb670f3 (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/_transport/extrinsics.py @ b6a81d144078eac18bd25c3af7cb31579bb670f3 (Apache-2.0); modified by rtao.
//! Hashed and post-quantum authorization signing (D-147).
//!
//! [`crate::Signer`] produces 64-byte Ed25519/Sr25519 signatures and is
//! unchanged. An [`AuthoritySigner`] signs for a hashed (rotating) or
//! post-quantum account: it returns a variable-length authorization proof
//! over the complete unshortened payload, at the generation the client read
//! from chain state. The signer never chooses its generation itself.

use std::{fmt, sync::Arc};

use rtao_core::{CoreError, keys::Keypair};
use rtao_primitives::{AccountId, AuthorityType};

use crate::{BoxFuture, SignPurpose, SignerError};

/// One authorization request: the payload and the chain-read generation.
#[derive(Debug, Clone, Copy)]
pub struct AuthorizationRequest<'a> {
    /// The complete signing payload: call, extra and implicit data. Never the
    /// blake2-256 shortcut ordinary signatures use above 256 bytes.
    pub payload: &'a [u8],
    /// SCALE call bytes, a prefix of `payload`, for display.
    pub call: &'a [u8],
    /// The generation to sign at, read from finalized or best chain state
    /// (zero for a standard post-quantum account).
    pub generation: u64,
    /// The submission stage.
    pub purpose: SignPurpose,
}

/// A complete authorization proof for one extrinsic.
///
/// The proof is public data: it carries the generation's public key, the
/// commitment to the next one and the signature.
#[derive(Clone, PartialEq, Eq)]
pub struct AuthorizationProof {
    /// The scheme the proof was produced with.
    pub scheme: AuthorityType,
    /// The encoded proof, [`AuthorityType::proof_length`] bytes long.
    pub bytes: Vec<u8>,
}

impl fmt::Debug for AuthorizationProof {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AuthorizationProof")
            .field("scheme", &self.scheme)
            .field("length", &self.bytes.len())
            .finish()
    }
}

impl AuthorizationProof {
    /// Require the scheme and exact length of `scheme`'s proofs.
    pub fn check(&self, scheme: AuthorityType) -> Result<(), SignerError> {
        if self.scheme != scheme {
            return Err(SignerError::InvalidSignature(
                "authorization proof scheme does not match the signer".into(),
            ));
        }
        if self.bytes.len() != scheme.proof_length() {
            return Err(SignerError::InvalidSignature(format!(
                "hashed authorization requires a complete {}-byte rotation proof",
                scheme.proof_length()
            )));
        }
        Ok(())
    }
}

/// An object-safe signing seam for hashed and post-quantum accounts.
pub trait AuthoritySigner: Send + Sync + 'static {
    /// The account the proofs authorize.
    fn account(&self) -> AccountId;
    /// The account's signing scheme.
    fn authority_type(&self) -> AuthorityType;
    /// The account's public descriptor; its account identifier commits to it.
    fn descriptor(&self) -> &[u8];
    /// The commitment to this signer's key at `generation`, compared with the
    /// chain's record before signing. May require the unlocked key.
    fn commitment(&self, generation: u64) -> Result<Vec<u8>, SignerError>;
    /// Sign the complete payload at the requested generation.
    fn sign_authorization<'a>(
        &'a self,
        request: &'a AuthorizationRequest<'a>,
    ) -> BoxFuture<'a, Result<AuthorizationProof, SignerError>>;
}

impl<S: AuthoritySigner + ?Sized> AuthoritySigner for Arc<S> {
    fn account(&self) -> AccountId {
        (**self).account()
    }
    fn authority_type(&self) -> AuthorityType {
        (**self).authority_type()
    }
    fn descriptor(&self) -> &[u8] {
        (**self).descriptor()
    }
    fn commitment(&self, generation: u64) -> Result<Vec<u8>, SignerError> {
        (**self).commitment(generation)
    }
    fn sign_authorization<'a>(
        &'a self,
        request: &'a AuthorizationRequest<'a>,
    ) -> BoxFuture<'a, Result<AuthorizationProof, SignerError>> {
        (**self).sign_authorization(request)
    }
}

/// An unlocked hashed or post-quantum core keypair.
///
/// The core keeps the master seed in a `Zeroizing` owner and derives each
/// generation's key from it only while signing; derived keys are wiped when
/// dropped. Debug output never includes key material.
#[derive(Clone)]
pub struct KeypairAuthoritySigner {
    keypair: Arc<Keypair>,
    scheme: AuthorityType,
    account: AccountId,
    descriptor: Vec<u8>,
}

impl KeypairAuthoritySigner {
    /// Wrap an unlocked keypair of a hashed or post-quantum scheme.
    ///
    /// Refuses a classical keypair (use [`crate::KeypairSigner`]) and a key
    /// whose account does not derive from its own public descriptor (Python's
    /// `_check_signer_identity`).
    pub fn new(keypair: Keypair) -> Result<Self, SignerError> {
        let scheme = AuthorityType::try_from(keypair.crypto_type()).map_err(|_| {
            SignerError::InvalidSignature(
                "keypair is not a hashed or post-quantum account; use KeypairSigner".into(),
            )
        })?;
        let descriptor = keypair
            .authorization_descriptor()
            .map_err(CoreError::from)?;
        let account = AccountId(keypair.public_key_bytes());
        let public = Keypair::from_hashed_descriptor(&descriptor, 42).map_err(CoreError::from)?;
        if AccountId(public.public_key_bytes()) != account || public.crypto_type() != scheme.code()
        {
            return Err(SignerError::InvalidSignature(
                "unlocked signing identity does not match the public wallet".into(),
            ));
        }
        Ok(Self {
            keypair: Arc::new(keypair),
            scheme,
            account,
            descriptor,
        })
    }

    /// Require this signer to be the public identity `account` with
    /// `descriptor`, as a wallet's public file names it.
    pub fn expect_identity(
        &self,
        account: AccountId,
        descriptor: &[u8],
    ) -> Result<(), SignerError> {
        if self.account != account || self.descriptor != descriptor {
            return Err(SignerError::InvalidSignature(
                "unlocked signing identity does not match the public wallet".into(),
            ));
        }
        Ok(())
    }

    fn at(&self, generation: u64) -> Result<Keypair, SignerError> {
        if self.scheme.is_stateless() && generation != 0 {
            return Err(SignerError::InvalidSignature(
                "a standard post-quantum account signs only at generation zero".into(),
            ));
        }
        let key = self
            .keypair
            .at_generation(generation)
            .map_err(CoreError::from)?;
        if AccountId(key.public_key_bytes()) != self.account
            || key.authorization_descriptor().map_err(CoreError::from)? != self.descriptor
        {
            return Err(SignerError::InvalidSignature(
                "unlocked signing identity does not match the public wallet".into(),
            ));
        }
        Ok(key)
    }
}

impl fmt::Debug for KeypairAuthoritySigner {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("KeypairAuthoritySigner")
            .field("account", &self.account)
            .field("scheme", &self.scheme)
            .field("keypair", &"[REDACTED]")
            .finish()
    }
}

impl AuthoritySigner for KeypairAuthoritySigner {
    fn account(&self) -> AccountId {
        self.account
    }
    fn authority_type(&self) -> AuthorityType {
        self.scheme
    }
    fn descriptor(&self) -> &[u8] {
        &self.descriptor
    }
    fn commitment(&self, generation: u64) -> Result<Vec<u8>, SignerError> {
        if self.scheme.is_stateless() {
            // The account commits to its one key through the descriptor.
            return Ok(self.descriptor.get(2..).unwrap_or_default().to_vec());
        }
        Ok(self
            .keypair
            .authorization_commitment(generation)
            .map_err(CoreError::from)?)
    }
    fn sign_authorization<'a>(
        &'a self,
        request: &'a AuthorizationRequest<'a>,
    ) -> BoxFuture<'a, Result<AuthorizationProof, SignerError>> {
        Box::pin(async move {
            let key = self.at(request.generation)?;
            let bytes = key
                .sign_authorization(request.payload)
                .map_err(CoreError::from)?;
            let proof = AuthorizationProof {
                scheme: self.scheme,
                bytes,
            };
            proof.check(self.scheme)?;
            Ok(proof)
        })
    }
}

/// A hashed or post-quantum keypair is also a [`crate::Signer`], so wallets,
/// key sources and the executor pass it like any key (D-149).
///
/// It produces no 64-byte signature: [`crate::Signer::sign`] and
/// [`crate::Signer::sign_raw`] refuse, and [`crate::Signer::authority`]
/// routes the client to its authorization path. `crypto_type` is the
/// nominal Sr25519 only for interfaces that require a classical type; no
/// 64-byte path signs with it.
impl crate::Signer for KeypairAuthoritySigner {
    fn account(&self) -> AccountId {
        self.account
    }
    fn crypto_type(&self) -> crate::CryptoType {
        crate::CryptoType::Sr25519
    }
    fn sign<'a>(
        &'a self,
        _request: &'a crate::SigningRequest<'a>,
    ) -> BoxFuture<'a, Result<crate::MultiSignature, SignerError>> {
        Box::pin(async {
            Err(SignerError::InvalidSignature(
                "hashed and post-quantum accounts sign authorization proofs, not 64-byte signatures"
                    .into(),
            ))
        })
    }
    fn authority(&self) -> Option<&dyn AuthoritySigner> {
        Some(self)
    }
}
