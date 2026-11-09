// Derived from RaoFoundation/subtensor sdk/python/bittensor/sp_core.py @ b6a81d144078eac18bd25c3af7cb31579bb670f3 (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/_transport/extrinsics.py @ b6a81d144078eac18bd25c3af7cb31579bb670f3 (Apache-2.0); modified by rtao.
//! Hashed and post-quantum signing authorities (Python `sp_core` crypto types).
//!
//! A classical Ed25519 or Sr25519 account signs with a 64-byte
//! [`crate::CryptoType`] signature. The account types here sign with a
//! variable-length authorization proof instead: rotating ("hashed") accounts
//! prove the key of the generation recorded on chain and commit to the next
//! one, and standard post-quantum accounts prove a fixed key their account
//! identifier commits to.

use crate::AccountId;
use std::fmt;

/// A signing authority whose extrinsic carries an authorization proof.
///
/// The discriminants are the core's crypto codes and the runtime's signature
/// versions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
#[repr(u8)]
pub enum AuthorityType {
    /// A rotating Sr25519 account (`CRYPTO_HASHED`).
    HashedSr25519 = 4,
    /// A rotating Ed25519 account (`CRYPTO_HASHED_ED25519`).
    HashedEd25519 = 6,
    /// A rotating ML-DSA-65 account (`CRYPTO_MLDSA65`).
    MlDsa65 = 7,
    /// A standard (fixed-key) ML-DSA-65 account (`CRYPTO_MLDSA65_FIXED`).
    MlDsa65Fixed = 8,
    /// A standard Falcon-512 account (`CRYPTO_FALCON512`).
    Falcon512 = 9,
    /// A rotating Falcon-512 account (`CRYPTO_FALCON512_HASHED`).
    Falcon512Hashed = 10,
    /// A standard Falcon-1024 account (`CRYPTO_FALCON1024`).
    Falcon1024 = 11,
    /// A rotating Falcon-1024 account (`CRYPTO_FALCON1024_HASHED`).
    Falcon1024Hashed = 12,
}

/// Python `HASHED_PROOF_LENGTH`: generation, current key, next commitment and
/// a 64-byte signature.
pub const HASHED_PROOF_LENGTH: usize = 136;
/// Python `MLDSA_PROOF_LENGTH`: an ML-DSA-65 signer address.
pub const MLDSA_PROOF_LENGTH: usize = 5367;

/// A crypto code that is not a hashed or post-quantum authority.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AuthorityTypeError(pub u8);

impl fmt::Display for AuthorityTypeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "crypto type {} is not a hashed or post-quantum signing scheme",
            self.0
        )
    }
}

impl std::error::Error for AuthorityTypeError {}

impl TryFrom<u8> for AuthorityType {
    type Error = AuthorityTypeError;
    fn try_from(value: u8) -> Result<Self, Self::Error> {
        Ok(match value {
            4 => Self::HashedSr25519,
            6 => Self::HashedEd25519,
            7 => Self::MlDsa65,
            8 => Self::MlDsa65Fixed,
            9 => Self::Falcon512,
            10 => Self::Falcon512Hashed,
            11 => Self::Falcon1024,
            12 => Self::Falcon1024Hashed,
            _ => return Err(AuthorityTypeError(value)),
        })
    }
}

impl From<AuthorityType> for u8 {
    fn from(value: AuthorityType) -> Self {
        value as Self
    }
}

impl AuthorityType {
    /// The core crypto code, which is also the runtime signature version.
    pub const fn code(self) -> u8 {
        self as u8
    }

    /// Python `PQ_CRYPTO_TYPES`: ML-DSA and Falcon.
    pub const fn is_post_quantum(self) -> bool {
        !matches!(self, Self::HashedSr25519 | Self::HashedEd25519)
    }

    /// Python `is_stateless_pq` (`FIXED_PQ_CRYPTO_TYPES`): a standard
    /// post-quantum account, whose fixed key needs no authority registration.
    pub const fn is_stateless(self) -> bool {
        matches!(
            self,
            Self::MlDsa65Fixed | Self::Falcon512 | Self::Falcon1024
        )
    }

    /// Python `requires_registration` (`HASHED_CRYPTO_TYPES`): a rotating
    /// account, which signs only at the generation registered on chain.
    pub const fn requires_registration(self) -> bool {
        !self.is_stateless()
    }

    /// The `HashedAccounts` record map of a rotating account.
    pub const fn records_storage(self) -> &'static str {
        if self.is_post_quantum() {
            "MlDsaAccounts"
        } else {
            "Accounts"
        }
    }

    /// Length of a chain commitment to one key: 32 bytes for a classical
    /// hashed account, 64 for a post-quantum one.
    pub const fn commitment_length(self) -> usize {
        if self.is_post_quantum() { 64 } else { 32 }
    }

    /// Python `PQ_PROFILES`: the descriptor's algorithm byte.
    pub const fn profile(self) -> Option<u8> {
        match self {
            Self::MlDsa65 => Some(4),
            Self::MlDsa65Fixed => Some(5),
            Self::Falcon512Hashed => Some(6),
            Self::Falcon512 => Some(7),
            Self::Falcon1024Hashed => Some(8),
            Self::Falcon1024 => Some(9),
            Self::HashedSr25519 | Self::HashedEd25519 => None,
        }
    }

    /// Python `pq_lengths`: canonical public-key and signature lengths.
    pub const fn pq_lengths(self) -> Option<(usize, usize)> {
        match self {
            Self::MlDsa65 | Self::MlDsa65Fixed => Some((1952, 3309)),
            Self::Falcon512 | Self::Falcon512Hashed => Some((897, 666)),
            Self::Falcon1024 | Self::Falcon1024Hashed => Some((1793, 1280)),
            Self::HashedSr25519 | Self::HashedEd25519 => None,
        }
    }

    /// Python `hashed_proof_length`: the exact length of one authorization proof.
    pub const fn proof_length(self) -> usize {
        match self.pq_lengths() {
            Some((public_key, signature)) => 106 + public_key + signature,
            None => HASHED_PROOF_LENGTH,
        }
    }

    /// Python's zero-signature quote: a proof of the complete envelope's size
    /// that can never authorize anything, for fee estimation without a key.
    pub fn placeholder_proof(
        self,
        account: AccountId,
        descriptor: &[u8],
        generation: u64,
    ) -> Vec<u8> {
        match self.pq_lengths() {
            Some((public_key, signature)) => {
                let mut proof = Vec::with_capacity(self.proof_length());
                proof.extend_from_slice(&account.0);
                proof.extend_from_slice(descriptor.get(..2).unwrap_or(&[2, 0]));
                proof.extend_from_slice(&generation.to_le_bytes());
                proof.resize(proof.len() + public_key, 0);
                proof.resize(proof.len() + 64, 1);
                proof.resize(proof.len() + signature, 0);
                proof
            }
            None => {
                let mut proof = generation.to_le_bytes().to_vec();
                proof.resize(HASHED_PROOF_LENGTH, 0);
                proof
            }
        }
    }
}

impl fmt::Display for AuthorityType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::HashedSr25519 => "hashed sr25519",
            Self::HashedEd25519 => "hashed ed25519",
            Self::MlDsa65 => "hashed ML-DSA-65",
            Self::MlDsa65Fixed => "standard ML-DSA-65",
            Self::Falcon512 => "standard Falcon-512",
            Self::Falcon512Hashed => "hashed Falcon-512",
            Self::Falcon1024 => "standard Falcon-1024",
            Self::Falcon1024Hashed => "hashed Falcon-1024",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_classes_and_lengths_follow_python() {
        for code in 0..=255u8 {
            match AuthorityType::try_from(code) {
                Ok(kind) => assert_eq!(kind.code(), code),
                Err(error) => {
                    assert!(!matches!(code, 4 | 6..=12));
                    assert_eq!(error, AuthorityTypeError(code));
                }
            }
        }
        use AuthorityType::*;
        let stateless: Vec<_> = [
            HashedSr25519,
            HashedEd25519,
            MlDsa65,
            MlDsa65Fixed,
            Falcon512,
            Falcon512Hashed,
            Falcon1024,
            Falcon1024Hashed,
        ]
        .into_iter()
        .filter(|kind| kind.is_stateless())
        .collect();
        assert_eq!(stateless, [MlDsa65Fixed, Falcon512, Falcon1024]);
        assert!(HashedSr25519.requires_registration() && MlDsa65.requires_registration());
        assert_eq!(HashedEd25519.proof_length(), HASHED_PROOF_LENGTH);
        assert_eq!(MlDsa65.proof_length(), MLDSA_PROOF_LENGTH);
        assert_eq!(MlDsa65Fixed.proof_length(), MLDSA_PROOF_LENGTH);
        assert_eq!(Falcon512.proof_length(), 106 + 897 + 666);
        assert_eq!(HashedSr25519.records_storage(), "Accounts");
        assert_eq!(Falcon1024Hashed.records_storage(), "MlDsaAccounts");
        assert_eq!(MlDsa65Fixed.profile(), Some(5));
        assert_eq!(HashedSr25519.commitment_length(), 32);
        assert_eq!(MlDsa65.commitment_length(), 64);
    }

    #[test]
    fn placeholder_proofs_have_the_envelope_length() {
        let account = AccountId([9; 32]);
        let hashed = AuthorityType::HashedSr25519.placeholder_proof(account, &[1, 0], 7);
        assert_eq!(hashed.len(), HASHED_PROOF_LENGTH);
        assert_eq!(hashed[..8], 7u64.to_le_bytes());
        let pq = AuthorityType::MlDsa65Fixed.placeholder_proof(account, &[2, 5, 0], 0);
        assert_eq!(pq.len(), MLDSA_PROOF_LENGTH);
        assert_eq!(pq[..32], [9; 32]);
        assert_eq!(pq[32..34], [2, 5]);
    }
}
