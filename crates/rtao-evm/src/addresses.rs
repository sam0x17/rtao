// Derived from RaoFoundation/subtensor sdk/python/bittensor/evm/addresses.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/tests/unit/test_evm.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Native account mirrors and truncated EVM origins.

use crate::{EvmError, Result};
use rtao_primitives::AccountId;

fn address_error(error: rtao_primitives::evm::AddressError) -> EvmError {
    EvmError::invalid(error.to_string())
}

/// Whether the input has Python's prefixed, forty-character hex-address shape.
///
/// Python's historical helper accepts ASCII whitespace between complete byte
/// pairs. [`parse_h160`] additionally requires twenty decoded address bytes
/// before constructing a typed Ethereum address.
pub fn is_h160(value: &str) -> bool {
    rtao_primitives::evm::is_h160(value)
}

/// Normalize an address using the pinned Python helper's accepted text syntax.
pub fn normalize_h160(value: &str) -> Result<String> {
    rtao_primitives::evm::normalize_h160(value).map_err(address_error)
}

/// Parse twenty address bytes for an Ethereum wire operation.
pub fn parse_h160(value: &str) -> Result<alloy_primitives::Address> {
    let bytes = rtao_primitives::evm::decode_h160(value).map_err(address_error)?;
    let bytes: [u8; 20] = bytes
        .try_into()
        .map_err(|_| EvmError::invalid("EVM address must contain exactly twenty decoded bytes"))?;
    Ok(alloy_primitives::Address::from(bytes))
}

/// Derive the native account credited by Frontier's hashed address mapping.
pub fn mirror_account(evm_address: &str) -> Result<AccountId> {
    rtao_primitives::evm::mirror_account(evm_address).map_err(address_error)
}

/// The default-prefix SS58 mirror at which an EVM address's native TAO lives.
pub fn h160_to_ss58(evm_address: &str) -> Result<String> {
    rtao_primitives::evm::h160_to_ss58(evm_address).map_err(address_error)
}

/// Derive the same native mirror with an explicit SS58 display prefix.
pub fn h160_to_ss58_with_prefix(evm_address: &str, prefix: u16) -> Result<String> {
    rtao_primitives::evm::h160_to_ss58_with_prefix(evm_address, prefix).map_err(address_error)
}

/// Decode an SS58 account to its `0x`-prefixed, thirty-two-byte public key.
pub fn ss58_to_pubkey(ss58_address: &str) -> Result<String> {
    rtao_primitives::evm::ss58_to_pubkey(ss58_address).map_err(address_error)
}

/// Encode exactly thirty-two public-key bytes with the default SS58 prefix.
pub fn pubkey_to_ss58(pubkey: &[u8]) -> Result<String> {
    rtao_primitives::evm::pubkey_to_ss58(pubkey).map_err(address_error)
}

/// Encode exactly thirty-two public-key bytes with an explicit display prefix.
pub fn pubkey_to_ss58_with_prefix(pubkey: &[u8], prefix: u16) -> Result<String> {
    rtao_primitives::evm::pubkey_to_ss58_with_prefix(pubkey, prefix).map_err(address_error)
}

/// Return the first twenty bytes of a native account's public key as an h160.
pub fn ss58_to_h160_truncated(ss58_address: &str) -> Result<String> {
    rtao_primitives::evm::ss58_to_h160_truncated(ss58_address).map_err(address_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    // pyt: tests/unit/test_evm.py::TestAddressMath::test_h160_to_ss58
    // pyt: tests/unit/test_evm.py::TestAddressMath::test_ss58_to_pubkey_round_trip
    // pyt: tests/unit/test_evm.py::TestAddressMath::test_ss58_to_h160_truncated_is_twenty_bytes
    fn pinned_python_mirrors_and_native_truncation() -> Result<()> {
        for (address, expected) in [
            (
                "0x1111111111111111111111111111111111111111",
                "5DDYKUgyHe2Sx8a9oTZPAXrgFaTiGnYZXrecNdeC9bG7TbtX",
            ),
            (
                "0xabababababababababababababababababababab",
                "5F2S3KWCNrTLuuwPbA6b3kSBveGS27yVvQVipym8TwcvP2cs",
            ),
            (
                "0x1234567890123456789012345678901234567890",
                "5Ettm6fSye3WnsW22Z1UNYa7Mo4gm2KQdcVYHs6rYuJHnhtj",
            ),
        ] {
            assert_eq!(h160_to_ss58(address)?, expected);
        }
        let key = [0xab; 32];
        let ss58 = pubkey_to_ss58(&key)?;
        assert_eq!(ss58_to_pubkey(&ss58)?, format!("0x{}", "ab".repeat(32)));
        assert_eq!(
            ss58_to_h160_truncated(&ss58)?,
            format!("0x{}", "ab".repeat(20))
        );
        let alternate = pubkey_to_ss58_with_prefix(&key, 0)?;
        assert_eq!(ss58_to_pubkey(&alternate)?, ss58_to_pubkey(&ss58)?);
        Ok(())
    }

    #[test]
    fn typed_wire_addresses_refuse_pythons_short_decoded_shape() -> Result<()> {
        let odd_legacy = format!("11  {}", "11".repeat(18));
        assert!(is_h160(&format!("0x{odd_legacy}")));
        assert!(normalize_h160(&odd_legacy).is_ok());
        assert!(parse_h160(&odd_legacy).is_err());
        assert_eq!(
            normalize_h160(&format!("\u{1c}{}\n", "AB".repeat(20)))?,
            format!("0x{}", "ab".repeat(20))
        );
        for address in [
            "0X1111111111111111111111111111111111111111",
            "0x11",
            "zz",
            "",
        ] {
            assert!(normalize_h160(address).is_err());
        }
        assert!(pubkey_to_ss58(&[0; 31]).is_err());
        Ok(())
    }
}
