//! Bittensor EVM wallets and helpers.
//!
//! EVM signing keys belong to a separate signing domain from native coldkeys
//! and hotkeys. Network I/O is explicit; importing this crate starts no service.

#![deny(missing_docs)]

/// The two mappings between native accounts and EVM addresses.
pub mod addresses;
/// User contract artifacts and deployment argument encoding.
pub mod contracts;
mod error;
/// Encrypted Ethereum keystore V3 files in a native wallet's `evmkeys` directory.
pub mod keys;
mod money;
/// EVM network labels, chain IDs and endpoint selection.
pub mod networks;
/// The pinned Bittensor precompile catalog and ABI helpers.
pub mod precompiles;
/// Bounded Ethereum HTTP JSON-RPC requests and native amount conversions.
pub mod rpc;
/// Legacy transaction preparation, signing and ownership proofs.
pub mod transactions;

pub use addresses::{
    h160_to_ss58, is_h160, normalize_h160, pubkey_to_ss58, ss58_to_h160_truncated, ss58_to_pubkey,
};
pub use alloy_primitives::{Address, B256, U256};
pub use alloy_signer_local::PrivateKeySigner as EvmAccount;
pub use contracts::{ContractArtifact, encode_deploy, load_artifact};
pub use error::EvmError;
/// Failure returned by an EVM library helper.
pub type Error = EvmError;
pub use keys::{
    ETH_DERIVATION_PATH, EvmKeyInfo, EvmKeyOptions, ImportSource, PasswordResolver, create_evm_key,
    export_evm_key, get_evm_key_info, import_evm_key, list_evm_keys, unlock_evm_key,
};
pub use networks::{EvmNetwork, evm_network, evm_networks};
pub use precompiles::{
    PRECOMPILES, Precompile, STANDARD_PRECOMPILES, caller_role, decode_result, describe_arguments,
    encode_call, function_deprecation, get_precompile,
};
pub use rpc::{EvmRpc, RpcOptions, WEI_PER_TAO, balance_to_wei, wei_to_balance};
pub use transactions::{EvmTxPreview, association_proof, prepare_transaction, send_transaction};

/// Result of an EVM helper operation.
pub type Result<T> = std::result::Result<T, EvmError>;

/// Bittensor's default SS58 account display prefix.
pub const SS58_FORMAT: u16 = 42;
