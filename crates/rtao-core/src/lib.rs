//! The curated boundary to the upstream Bittensor core.
//!
//! Chain-defined encoding and cryptography stay upstream. This crate selects
//! the portable surface, maps errors, and contains the optional async offload.

#![deny(missing_docs)]

mod error;
mod value;

/// Shared provenance validation for maintenance tools and final workspaces.
#[doc(hidden)]
pub mod dependency_policy;

pub use error::CoreError;
pub use value::DynValue;

/// Serializable runtime metadata inspection staged for upstream adoption.
pub use rtao_core_ext::ir;
/// Pure btcli-compatible environment password encoding.
pub use rtao_core_ext::keyfile_env;
/// Portable plaintext keyfile JSON and format detection.
pub use rtao_core_ext::keyfile_json;
/// Portable protocol construction and decoding staged for upstream adoption.
pub use rtao_core_ext::ops;
/// Dynamic-value traversal helpers staged for upstream adoption.
pub use rtao_core_ext::value_ext;

include!(concat!(env!("OUT_DIR"), "/core_pin.rs"));

/// Substrate key derivation, SS58, and signature verification.
///
/// These are upstream primitives. Their secret constructors can return
/// diagnostics containing supplied recovery material. The signer and wallet
/// APIs redact those errors at their owned boundaries; direct use of the
/// upstream `Keypair` retains its diagnostic behavior pending subtensor #3252.
pub mod keys {
    pub use bittensor_core::keys::{
        CRYPTO_ED25519, CRYPTO_SR25519, DEFAULT_SS58_FORMAT, Keypair, public_key_from_ss58,
        ss58_from_public, verify,
    };
    pub use rtao_core_ext::verify_strict;
}

/// Portable SCALE codecs and the core's decoded-value carrier.
pub mod codec {
    /// Error returned directly by upstream encoding and cryptography functions.
    /// Convert with [`crate::CoreError::from`] at an owned non-secret error boundary.
    /// Secret-derived diagnostics require the higher-level constructor redaction.
    pub use bittensor_core::CoreError as Error;
    pub use bittensor_core::codec::value::Value;
    pub use bittensor_core::codec::{batch, decode, encode, storage, value};
}

/// Portable runtime metadata models and type expressions.
pub mod runtime {
    pub use bittensor_core::runtime::type_string::{Primitive, TypeSpec};
    pub use bittensor_core::runtime::{
        ConstantInfo, ExtrinsicInfo, PalletInfo, Runtime, RuntimeApiInfo, RuntimeApiMethodInfo,
        RuntimeApiParamInfo, SignedExtensionInfo, StorageInfo,
    };
}

/// Signed-extrinsic parameters, era math, and multisig account derivation.
pub mod extrinsic {
    pub use bittensor_core::codec::extrinsic::{
        TxParams, era_birth, multisig_account_id, multisig_ss58,
    };
}

/// RFC-0078 metadata digests and Ledger proofs.
pub mod digest {
    pub use bittensor_core::digest::{ChainInfo, generate_extrinsic_proof, metadata_digest};
}

/// Runtime-compatible ML-KEM shield envelope and storage hashing.
pub mod mlkem {
    pub use bittensor_core::mlkem::{KDF_ID, MLKEM_NONCE_LEN, seal, twox_128};
}

/// Pure timelock crypto; drand network I/O belongs to the async client.
pub mod timelock {
    pub use bittensor_core::timelock::{
        UserData, WeightsTlockPayload, constants, decrypt_and_decompress, decrypt_with_signature,
        encrypt_and_compress, encrypt_at_round, inner_ciphertext, reveal_round,
    };
    /// Pallet-faithful epoch prediction staged for PR-U5.
    pub use rtao_core_ext::epoch_schedule;
    /// Injectable-clock commitment preparation staged for PR-U5.
    pub use rtao_core_ext::timelock_commit;
}

/// Core-compatible encrypted keyfiles, available only to host applications.
#[cfg(feature = "host")]
pub mod keyfiles {
    pub use bittensor_core::keyfiles::{
        decrypt_keyfile_data, deserialize_keypair_from_keyfile_data, encrypt_keyfile_data,
        get_password_from_environment, keyfile_data_encryption_method, keyfile_data_is_encrypted,
        keyfile_data_is_encrypted_ansible, keyfile_data_is_encrypted_legacy,
        keyfile_data_is_encrypted_nacl, serialized_keypair_to_keyfile_data,
    };
}

/// Chain-compatible proof-of-work primitives for host applications.
#[cfg(feature = "host")]
pub mod pow {
    pub use bittensor_core::pow::{MAX_BATCH, PREFIX_LEN, mine_cpu, seal};

    /// Optional OpenCL mining; drivers are loaded only when requested.
    pub mod opencl {
        pub use bittensor_core::pow::{GpuDevice, GpuMiner, gpu_devices};
    }
}

/// Ledger signing through the upstream hardware implementation.
#[cfg(feature = "ledger")]
pub mod ledger {
    pub use bittensor_core::signers::ledger::{LedgerAddress, LedgerDevice};
}

/// Run blocking crypto, KDF or device work off the async runtime's worker thread.
///
/// A panic or cancellation of the blocking task is returned as a join error.
/// Like `spawn_blocking`, this requires an entered Tokio runtime and does not
/// cancel blocking work that has already started when its caller is dropped.
#[cfg(feature = "offload")]
pub async fn offload<T, F>(work: F) -> Result<T, tokio::task::JoinError>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    tokio::task::spawn_blocking(work).await
}
