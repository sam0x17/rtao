// Derived from RaoFoundation/subtensor sdk/python/bittensor/evm/addresses.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Portable address helpers for EVM clients and native funding intents.
//!
//! The pinned Python helpers check forty text characters, including permitted
//! whitespace between hex byte pairs. This historical shape can decode to fewer
//! than twenty bytes. Ethereum wire encoders must additionally enforce their
//! twenty-byte address requirement before constructing a typed address.

use std::fmt;

use crate::AccountId;

/// An invalid address or a failure in the upstream native-address codec.
#[derive(Debug)]
pub enum AddressError {
    /// Public address input failed validation.
    Invalid(String),
    /// The native-account codec or hashing operation failed.
    Core(rtao_core::CoreError),
}

impl fmt::Display for AddressError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) => formatter.write_str(message),
            Self::Core(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for AddressError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Core(error) => Some(error),
            Self::Invalid(_) => None,
        }
    }
}

fn python_whitespace(character: char) -> bool {
    character.is_whitespace() || matches!(character, '\u{1c}'..='\u{1f}')
}

fn python_fromhex(input: &str) -> Option<Vec<u8>> {
    fn nibble(byte: u8) -> Option<u8> {
        match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            b'A'..=b'F' => Some(byte - b'A' + 10),
            _ => None,
        }
    }

    let mut output = Vec::with_capacity(input.len() / 2);
    // Python accepts vertical tab as well as the five whitespace bytes in
    // Rust's is_ascii_whitespace. Whitespace may not divide a byte's nibbles.
    for group in input
        .as_bytes()
        .split(|byte| matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c))
    {
        if !group.len().is_multiple_of(2) {
            return None;
        }
        for pair in group.chunks_exact(2) {
            output.push((nibble(pair[0])? << 4) | nibble(pair[1])?);
        }
    }
    Some(output)
}

fn invalid_h160(value: &str) -> AddressError {
    AddressError::Invalid(format!(
        "not a valid EVM (h160) address: {}",
        crate::pyfmt::string_repr(value)
    ))
}

fn normalized_parts(value: &str) -> Result<(&str, Vec<u8>), AddressError> {
    let text = value.trim_matches(python_whitespace);
    let body = text.strip_prefix("0x").unwrap_or(text);
    if body.chars().count() != 40 {
        return Err(invalid_h160(value));
    }
    let bytes = python_fromhex(body).ok_or_else(|| invalid_h160(value))?;
    Ok((body, bytes))
}

/// Test Python's exact prefixed, forty-character hex-address shape.
///
/// This predicate does not strip surrounding whitespace or add a prefix.
/// Interior ASCII whitespace between byte pairs is accepted, so success does
/// not guarantee twenty decoded bytes.
pub fn is_h160(value: &str) -> bool {
    value.starts_with("0x") && value.chars().count() == 42 && python_fromhex(&value[2..]).is_some()
}

/// Strip Python whitespace, add a missing lowercase prefix and lowercase hex.
///
/// An uppercase `0X` prefix is rejected. Permitted interior ASCII whitespace
/// remains in the returned text, matching the pinned Python helper.
pub fn normalize_h160(value: &str) -> Result<String, AddressError> {
    let (body, _) = normalized_parts(value)?;
    Ok(format!("0x{}", body.to_ascii_lowercase()))
}

/// Decode a normalized-compatible h160 input using Python's legacy shape rule.
///
/// Like [`normalize_h160`], this accepts missing prefixes and strips surrounding
/// Python whitespace. The decoded result can be shorter than twenty bytes;
/// callers writing an Ethereum address must check its exact length.
pub fn decode_h160(value: &str) -> Result<Vec<u8>, AddressError> {
    normalized_parts(value).map(|(_, bytes)| bytes)
}

/// Derive Frontier's native mirror as `blake2_256(b"evm:" || decoded_address)`.
pub fn mirror_account(evm_address: &str) -> Result<AccountId, AddressError> {
    let bytes = decode_h160(evm_address)?;
    let mut payload = Vec::with_capacity(4 + bytes.len());
    payload.extend_from_slice(b"evm:");
    payload.extend_from_slice(&bytes);
    let hash = rtao_core::codec::storage::hash_param("Blake2_256", &payload)
        .map_err(rtao_core::CoreError::from)
        .map_err(AddressError::Core)?;
    let hash = hash
        .try_into()
        .map_err(|_| AddressError::Invalid("Blake2_256 returned an invalid hash length".into()))?;
    Ok(AccountId(hash))
}

/// Render the EVM address's native mirror using the default SS58 prefix, 42.
pub fn h160_to_ss58(evm_address: &str) -> Result<String, AddressError> {
    h160_to_ss58_with_prefix(evm_address, 42)
}

/// Render the same native mirror using an explicit SS58 display prefix.
pub fn h160_to_ss58_with_prefix(evm_address: &str, prefix: u16) -> Result<String, AddressError> {
    Ok(mirror_account(evm_address)?.to_ss58(prefix))
}

fn decode_ss58(value: &str) -> Result<AccountId, AddressError> {
    value.parse().map_err(AddressError::Core)
}

fn prefixed_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(2 + bytes.len() * 2);
    text.push_str("0x");
    for byte in bytes {
        text.push(char::from(DIGITS[usize::from(byte >> 4)]));
        text.push(char::from(DIGITS[usize::from(byte & 0xf)]));
    }
    text
}

/// Decode an SS58 account to its prefixed, thirty-two-byte public-key hex.
pub fn ss58_to_pubkey(ss58_address: &str) -> Result<String, AddressError> {
    Ok(prefixed_hex(&decode_ss58(ss58_address)?.0))
}

/// Encode exactly thirty-two public-key bytes using default SS58 prefix 42.
pub fn pubkey_to_ss58(pubkey: &[u8]) -> Result<String, AddressError> {
    pubkey_to_ss58_with_prefix(pubkey, 42)
}

/// Encode exactly thirty-two public-key bytes with an explicit display prefix.
pub fn pubkey_to_ss58_with_prefix(pubkey: &[u8], prefix: u16) -> Result<String, AddressError> {
    let key: [u8; 32] = pubkey.try_into().map_err(|_| {
        AddressError::Invalid(format!(
            "expected a 32-byte public key, got {} bytes",
            pubkey.len()
        ))
    })?;
    Ok(AccountId(key).to_ss58(prefix))
}

/// Return a native account's first twenty public-key bytes as prefixed hex.
pub fn ss58_to_h160_truncated(ss58_address: &str) -> Result<String, AddressError> {
    Ok(prefixed_hex(&decode_ss58(ss58_address)?.0[..20]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinned_python_mirrors_are_stable_across_display_prefixes() -> Result<(), AddressError> {
        // Pinned Python tests/unit/test_evm.py::H160_TO_SS58_VECTORS.
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
            let custom = h160_to_ss58_with_prefix(address, 0)?;
            assert_ne!(custom, expected);
            assert_eq!(ss58_to_pubkey(&custom)?, ss58_to_pubkey(expected)?);
        }
        assert_eq!(
            ss58_to_pubkey("5DDYKUgyHe2Sx8a9oTZPAXrgFaTiGnYZXrecNdeC9bG7TbtX")?,
            "0x32f955c958e51189a4921aed41ef00818f7368dfaec8d9969f091006f8066228"
        );
        Ok(())
    }

    #[test]
    fn python_whitespace_is_accepted_between_complete_byte_pairs() -> Result<(), AddressError> {
        for whitespace in [' ', '\t', '\n', '\r', '\u{b}', '\u{c}'] {
            let address = format!("0x11{whitespace}{whitespace}{}", "AB".repeat(18));
            assert!(is_h160(&address));
            assert_eq!(
                normalize_h160(&address)?,
                format!("0x11{whitespace}{whitespace}{}", "ab".repeat(18))
            );
            assert_eq!(
                decode_h160(&address)?,
                [vec![0x11], vec![0xab; 18]].concat()
            );
            let equivalent = format!("0x11ab  {}", "ab".repeat(17));
            assert_eq!(mirror_account(&address)?, mirror_account(&equivalent)?);
            let split_nibble = format!("0x1{whitespace}{whitespace}1{}", "ab".repeat(18));
            assert!(!is_h160(&split_nibble));
            assert!(normalize_h160(&split_nibble).is_err());
        }
        let unicode_space = format!("0x11\u{a0} {}", "ab".repeat(18));
        assert!(!is_h160(&unicode_space));
        assert_eq!(
            normalize_h160(&format!("\u{1c}\u{a0}{}\u{1f}\n", "AB".repeat(20)))?,
            format!("0x{}", "ab".repeat(20))
        );
        let unprefixed = "AB".repeat(20);
        assert!(!is_h160(&unprefixed));
        assert!(normalize_h160(&unprefixed).is_ok());
        Ok(())
    }

    #[test]
    fn invalid_addresses_retain_original_python_quoted_input() {
        for address in [
            "",
            "0x",
            "0X1111111111111111111111111111111111111111",
            "0x111111111111111111111111111111111111111",
            "0xgg11111111111111111111111111111111111111",
            "0xé111111111111111111111111111111111111111",
        ] {
            let expected = format!(
                "not a valid EVM (h160) address: {}",
                crate::pyfmt::string_repr(address)
            );
            assert_eq!(
                normalize_h160(address).err().map(|e| e.to_string()),
                Some(expected)
            );
            assert!(decode_h160(address).is_err());
            assert!(!is_h160(address));
        }
    }

    #[test]
    fn ss58_conversion_preserves_full_keys_and_truncates_only_the_origin()
    -> Result<(), AddressError> {
        let key: [u8; 32] = std::array::from_fn(|index| index as u8);
        for prefix in [0, 42, 1000] {
            let address = pubkey_to_ss58_with_prefix(&key, prefix)?;
            assert_eq!(
                ss58_to_pubkey(&address)?,
                "0x000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f"
            );
            assert_eq!(
                ss58_to_h160_truncated(&address)?,
                "0x000102030405060708090a0b0c0d0e0f10111213"
            );
        }
        assert_eq!(pubkey_to_ss58(&key)?, pubkey_to_ss58_with_prefix(&key, 42)?);
        for length in [0, 20, 31, 33] {
            assert!(pubkey_to_ss58(&vec![0; length]).is_err());
        }
        assert!(ss58_to_pubkey("invalid").is_err());
        assert!(ss58_to_h160_truncated("invalid").is_err());
        Ok(())
    }
}
