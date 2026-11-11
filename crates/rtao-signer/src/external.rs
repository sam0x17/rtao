// Derived from RaoFoundation/subtensor sdk/python/bittensor/_transport/extrinsics.py @ d967d13588834f4f5aaa8ef4c8fc1a865d581462 (Apache-2.0); modified by rtao.
//! Signatures that external signers return for prepared transactions,
//! including 65-byte raw ECDSA signatures (D-180).

use std::fmt;

use blake2::{Blake2b, Digest, digest::consts::U32};
use rtao_primitives::{AccountId, SignatureScheme};

use crate::{MultiSignature, SignerError};

/// A raw ECDSA (secp256k1) signature, `r || s || v`: the payload of
/// `MultiSignature::Ecdsa`.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct EcdsaSignature(pub [u8; 65]);

impl fmt::Debug for EcdsaSignature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("EcdsaSignature(0x")?;
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        f.write_str(")")
    }
}

impl EcdsaSignature {
    /// The account that signed `payload`, as the runtime's
    /// `MultiSignature::Ecdsa` check derives it: the secp256k1 public key
    /// recovered from this signature over `blake2_256(payload)` (a `v` above
    /// 26 counts less 27), and the account is `blake2_256` of the compressed
    /// key. `None` when no key recovers.
    ///
    /// A high-`s` signature recovers here as the runtime recovers it: it is
    /// normalized with its recovery parity flipped, which recovers the same
    /// key, while the signature itself is attached unchanged.
    pub fn recover_account(&self, payload: &[u8]) -> Option<AccountId> {
        use k256::ecdsa::{RecoveryId, Signature, VerifyingKey};
        let digest: [u8; 32] = Blake2b::<U32>::digest(payload).into();
        let v = self.0[64];
        let mut id = RecoveryId::from_byte(if v > 26 { v - 27 } else { v })?;
        let mut signature = Signature::from_slice(&self.0[..64]).ok()?;
        if let Some(normalized) = signature.normalize_s() {
            signature = normalized;
            id = RecoveryId::new(!id.is_y_odd(), id.is_x_reduced());
        }
        let key = VerifyingKey::recover_from_prehash(&digest, &signature, id).ok()?;
        rtao_core::keys::account_id_from_ecdsa_public(key.to_encoded_point(true).as_bytes())
            .ok()
            .map(AccountId)
    }
}

/// A signature from an external signer for a prepared transaction: a
/// classical [`MultiSignature`] or a raw ECDSA signature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ExternalSignature {
    /// An Ed25519 or Sr25519 signature.
    Classical(MultiSignature),
    /// A 65-byte raw ECDSA signature.
    Ecdsa(EcdsaSignature),
}

impl From<MultiSignature> for ExternalSignature {
    fn from(value: MultiSignature) -> Self {
        Self::Classical(value)
    }
}

impl From<EcdsaSignature> for ExternalSignature {
    fn from(value: EcdsaSignature) -> Self {
        Self::Ecdsa(value)
    }
}

impl ExternalSignature {
    /// The signature's `MultiSignature` scheme.
    pub fn scheme(&self) -> SignatureScheme {
        match self {
            Self::Classical(signature) => signature.crypto.into(),
            Self::Ecdsa(_) => SignatureScheme::Ecdsa,
        }
    }

    /// The raw signature bytes, without the variant byte.
    pub fn raw(&self) -> &[u8] {
        match self {
            Self::Classical(signature) => &signature.bytes,
            Self::Ecdsa(signature) => &signature.0,
        }
    }

    /// The `MultiSignature` encoding: the variant byte, then the raw bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut encoded = Vec::with_capacity(1 + self.raw().len());
        encoded.push(self.scheme().code());
        encoded.extend_from_slice(self.raw());
        encoded
    }

    /// Read an external signer's reply for a transaction prepared for
    /// `expected` (Python v490 `_normalize_signature`).
    ///
    /// A reply of the scheme's raw length is raw (so a 65-byte reply for an
    /// ECDSA account is raw ECDSA, whatever its first byte); a reply one byte
    /// longer carries its `MultiSignature` variant first (the convention of
    /// extensions and hardware devices). Unlike Python, which passes any
    /// other shape through for the chain to reject and lets a classical
    /// variant byte override the prepared scheme, both are refused here: the
    /// variant must be the prepared scheme, as [`MultiSignature::from_bytes_for`]
    /// requires.
    pub fn normalize(signature: &[u8], expected: SignatureScheme) -> Result<Self, SignerError> {
        let raw_len = |code: u8| {
            SignatureScheme::try_from(code)
                .ok()
                .map(SignatureScheme::signature_len)
        };
        let (raw, scheme) = if signature.len() == expected.signature_len() {
            (signature, expected)
        } else {
            match signature.split_first() {
                Some((&variant, raw)) if Some(raw.len()) == raw_len(variant) => (
                    raw,
                    SignatureScheme::try_from(variant).map_err(|_| unrecognized(expected))?,
                ),
                _ => return Err(unrecognized(expected)),
            }
        };
        if scheme != expected {
            return Err(SignerError::InvalidSignature(
                "MultiSignature variant does not match signer crypto type".into(),
            ));
        }
        match (scheme.crypto_type(), scheme) {
            (Some(crypto), _) => Ok(Self::Classical(MultiSignature {
                crypto,
                bytes: raw.try_into().map_err(|_| unrecognized(expected))?,
            })),
            (None, SignatureScheme::Ecdsa) => Ok(Self::Ecdsa(EcdsaSignature(
                raw.try_into().map_err(|_| unrecognized(expected))?,
            ))),
            (None, _) => Err(unrecognized(expected)),
        }
    }

    /// [`Self::normalize`] for a hex reply, with or without `0x`.
    pub fn normalize_hex(signature: &str, expected: SignatureScheme) -> Result<Self, SignerError> {
        let digits = signature.strip_prefix("0x").unwrap_or(signature);
        if !digits.len().is_multiple_of(2) || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(SignerError::InvalidSignature("signature is not hex".into()));
        }
        let bytes = (0..digits.len())
            .step_by(2)
            .map(|index| u8::from_str_radix(&digits[index..index + 2], 16))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| SignerError::InvalidSignature("signature is not hex".into()))?;
        Self::normalize(&bytes, expected)
    }

    /// Whether this signature over `payload` is `account`'s, by the runtime's
    /// `MultiSignature` rules: Ed25519 and Sr25519 through the core's strict
    /// verifier, ECDSA by public-key recovery ([`EcdsaSignature::recover_account`]).
    pub fn verify(&self, payload: &[u8], account: &AccountId) -> Result<bool, SignerError> {
        match self {
            Self::Classical(signature) => rtao_core::keys::verify_strict(
                payload,
                &signature.bytes,
                &account.to_ss58(42),
                signature.crypto.into(),
            )
            .map_err(|error| SignerError::Core(error.into())),
            Self::Ecdsa(signature) => Ok(signature.recover_account(payload) == Some(*account)),
        }
    }
}

fn unrecognized(expected: SignatureScheme) -> SignerError {
    let raw = expected.signature_len();
    SignerError::InvalidSignature(format!(
        "a {expected} signature is {raw} raw bytes, or {} with its MultiSignature variant first",
        raw + 1
    ))
}
