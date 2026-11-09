// Derived from RaoFoundation/subtensor sdk/python/bittensor/receiving.py @ b6a81d144078eac18bd25c3af7cb31579bb670f3 (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/hashed.py @ b6a81d144078eac18bd25c3af7cb31579bb670f3 (Apache-2.0); modified by rtao.
//! Self-contained receiving addresses (Python `bittensor.receiving`).
//!
//! A `tao_` address carries a hashed or ML-DSA account's public descriptor and
//! a checksum; it has no network field. Its account is the runtime's existing
//! `AccountId`, derived from the descriptor by the core's codec. Resolving one
//! for a read never needs the chain; a write that cannot carry a registration
//! guard must instead check the finalized registration (Python's
//! `account_for_registered_write`); the descriptor helpers here convert between
//! a descriptor and the record `HashedAccounts` stores for it.

use crate::AccountId;

/// Python's `is_receiving_address`: the reserved family, malformed or
/// unsupported versions included.
///
/// This is a routing predicate, not validation: an invalid member must reach
/// the strict decoder rather than another lookup.
pub fn is_receiving_address(value: &str) -> bool {
    let value = value.trim_start().to_ascii_lowercase();
    value.starts_with("bth") || value.starts_with("tao_")
}

/// A parsed recipient (Python's `Recipient`): the text as given, its account
/// and, for a receiving address, the descriptor it carries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recipient {
    /// The address as supplied.
    pub address: String,
    /// The account the address names.
    pub account: AccountId,
    /// The public descriptor of a receiving address; `None` for SS58.
    pub descriptor: Option<Vec<u8>>,
}

/// Python's `parse_recipient`: a receiving address through the core's strict
/// decoder, anything else as SS58.
pub fn parse_recipient(value: &str) -> Result<Recipient, rtao_core::CoreError> {
    if is_receiving_address(value) {
        let descriptor = rtao_core::keys::decode_hashed_receiving_address(value)?;
        let public = rtao_core::keys::Keypair::from_hashed_descriptor(
            &descriptor,
            rtao_core::keys::DEFAULT_SS58_FORMAT,
        )?;
        return Ok(Recipient {
            address: value.to_owned(),
            account: AccountId(public.public_key_bytes()),
            descriptor: Some(descriptor),
        });
    }
    Ok(Recipient {
        address: value.to_owned(),
        account: value.parse()?,
        descriptor: None,
    })
}

/// Python's `account_for_read`: the account a read uses for an address,
/// without changing its identity. SS58 is returned as given; only a receiving
/// address is resolved.
pub fn account_for_read(value: &str) -> Result<String, rtao_core::CoreError> {
    if !is_receiving_address(value) {
        return Ok(value.to_owned());
    }
    Ok(parse_recipient(value)?.account.to_string())
}

/// Python's `is_stateless_pq_descriptor`: a fixed (registration-free)
/// post-quantum descriptor, ML-DSA-65 or Falcon, which no registry holds.
pub fn is_stateless_pq_descriptor(descriptor: &[u8]) -> bool {
    descriptor.len() == 66 && descriptor[0] == 2 && matches!(descriptor[1], 5 | 7 | 9)
}

fn invalid(message: &str) -> rtao_core::CoreError {
    rtao_core::CoreError::Codec(message.to_owned())
}

/// Python's `descriptor_value`: a descriptor's stored fields, as the chain
/// renders them (`version`, `scheme` or `algorithm`, `initial_commitment`).
pub fn descriptor_value(descriptor: &[u8]) -> Result<serde_json::Value, rtao_core::CoreError> {
    let commitment = descriptor.get(2..).unwrap_or_default();
    if descriptor.len() == 66
        && descriptor[0] == 2
        && (4..=9).contains(&descriptor[1])
        && commitment.iter().any(|byte| *byte != 0)
    {
        return Ok(serde_json::json!({
            "version": 2,
            "algorithm": descriptor[1],
            "initial_commitment": crate::evm::prefixed_hex(commitment),
        }));
    }
    let scheme = match (descriptor.len(), descriptor.first(), descriptor.get(1)) {
        (34, Some(1), Some(1)) => "Sr25519",
        (34, Some(1), Some(3)) => "Ed25519",
        _ => return Err(invalid("unsupported hashed descriptor")),
    };
    if commitment.iter().all(|byte| *byte == 0) {
        return Err(invalid("unsupported hashed descriptor"));
    }
    Ok(serde_json::json!({
        "version": 1,
        "scheme": scheme,
        "initial_commitment": crate::evm::prefixed_hex(commitment),
    }))
}

/// Python's `descriptor_storage`: the permanent registry that holds this
/// descriptor's account, after validating it.
pub fn descriptor_storage(descriptor: &[u8]) -> Result<&'static str, rtao_core::CoreError> {
    descriptor_value(descriptor)?;
    Ok(if descriptor[0] == 2 {
        "MlDsaAccounts"
    } else {
        "Accounts"
    })
}

/// Python's `registration_descriptor_value`: the descriptor as the original
/// `register`/`check_registered` calls take it, under its scheme's variant.
pub fn registration_descriptor_value(
    descriptor: &[u8],
) -> Result<serde_json::Value, rtao_core::CoreError> {
    let value = descriptor_value(descriptor)?;
    let variant = match (descriptor[0], descriptor[1]) {
        (2, 6..=9) => "Falcon",
        (2, _) => "MlDsa65",
        _ => "Classical",
    };
    Ok(serde_json::json!({ variant: value }))
}

/// Python `hashed.raw_bytes`: bytes stored as 0x hex or a byte list.
pub fn raw_bytes(value: &serde_json::Value) -> Option<Vec<u8>> {
    match value {
        serde_json::Value::String(text) => {
            crate::pyfmt::bytes_fromhex(text.strip_prefix("0x").unwrap_or(text)).ok()
        }
        serde_json::Value::Array(items) => items
            .iter()
            .map(|item| item.as_u64().and_then(|byte| u8::try_from(byte).ok()))
            .collect(),
        _ => None,
    }
}

/// Python's `descriptor_bytes`: validate the versioned public descriptor a
/// `HashedAccounts` record stores and return its encoding.
pub fn descriptor_bytes(value: &serde_json::Value) -> Result<Vec<u8>, rtao_core::CoreError> {
    let commitment = value
        .get("initial_commitment")
        .and_then(raw_bytes)
        .ok_or_else(|| invalid("unsupported hashed descriptor"))?;
    if value.get("version").and_then(serde_json::Value::as_u64) == Some(2) {
        let algorithm = value
            .get("algorithm")
            .and_then(serde_json::Value::as_u64)
            .filter(|algorithm| (4..=9).contains(algorithm))
            .ok_or_else(|| invalid("unsupported post-quantum descriptor"))?;
        if commitment.len() != 64 || commitment.iter().all(|byte| *byte == 0) {
            return Err(invalid("unsupported post-quantum descriptor"));
        }
        let mut descriptor = vec![2, algorithm as u8];
        descriptor.extend_from_slice(&commitment);
        return Ok(descriptor);
    }
    let scheme = match value.get("scheme") {
        Some(serde_json::Value::String(scheme)) => Some(scheme.as_str()),
        Some(serde_json::Value::Object(variant)) if variant.len() == 1 => {
            variant.keys().next().map(String::as_str)
        }
        _ => None,
    };
    let scheme = match scheme {
        Some("Sr25519") => 1,
        Some("Ed25519") => 3,
        _ => return Err(invalid("unsupported hashed descriptor")),
    };
    if value.get("version").and_then(serde_json::Value::as_u64) != Some(1) {
        return Err(invalid("unsupported hashed descriptor"));
    }
    if commitment.len() != 32 || commitment.iter().all(|byte| *byte == 0) {
        return Err(invalid("hashed key commitments must contain 32 bytes"));
    }
    let mut descriptor = vec![1, scheme];
    descriptor.extend_from_slice(&commitment);
    Ok(descriptor)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn receiving_addresses_resolve_to_their_descriptor_account() {
        let key =
            rtao_core::keys::Keypair::from_seed(&[31; 32], rtao_core::keys::CRYPTO_HASHED_ED25519)
                .unwrap();
        let address = key.hashed_receiving_address().unwrap();
        assert!(address.starts_with("tao_"));
        let recipient = parse_recipient(&address).unwrap();
        assert_eq!(recipient.account, AccountId(key.public_key_bytes()));
        assert_eq!(
            recipient.descriptor.as_deref(),
            Some(&key.hashed_descriptor().unwrap()[..])
        );
        assert_eq!(account_for_read(&address).unwrap(), key.ss58_address());
        // SS58 stays as written; the family is routed to the strict decoder.
        let alice = "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY";
        assert_eq!(account_for_read(alice).unwrap(), alice);
        assert!(parse_recipient(alice).unwrap().descriptor.is_none());
        for routed in [" TAO_x", "bth2_x", "BTHx"] {
            assert!(is_receiving_address(routed));
            assert!(account_for_read(routed).is_err());
        }
        let mut tampered = address.clone();
        tampered.pop();
        tampered.push(if address.ends_with('A') { 'B' } else { 'A' });
        assert!(parse_recipient(&tampered).is_err());
        assert!(!is_receiving_address(alice));
    }

    #[test]
    fn descriptors_round_trip_through_their_stored_records() {
        for crypto in [
            rtao_core::keys::CRYPTO_HASHED,
            rtao_core::keys::CRYPTO_HASHED_ED25519,
            rtao_core::keys::CRYPTO_MLDSA65,
            rtao_core::keys::CRYPTO_MLDSA65_FIXED,
        ] {
            let key = rtao_core::keys::Keypair::from_seed(&[7; 32], crypto).unwrap();
            let descriptor = key.authorization_descriptor().unwrap();
            let record = descriptor_value(&descriptor).unwrap();
            assert_eq!(descriptor_bytes(&record).unwrap(), descriptor);
            let storage = descriptor_storage(&descriptor).unwrap();
            assert_eq!(
                storage,
                if descriptor[0] == 2 {
                    "MlDsaAccounts"
                } else {
                    "Accounts"
                }
            );
            assert_eq!(
                is_stateless_pq_descriptor(&descriptor),
                crypto == rtao_core::keys::CRYPTO_MLDSA65_FIXED
            );
            let variant = registration_descriptor_value(&descriptor).unwrap();
            let name = if descriptor[0] == 2 {
                "MlDsa65"
            } else {
                "Classical"
            };
            assert_eq!(variant[name], record);
        }
        // A scheme decoded as a single-key variant and commitments as byte lists.
        let mut record = descriptor_value(&[[1, 1].as_slice(), &[9; 32]].concat()).unwrap();
        record["scheme"] = serde_json::json!({"Sr25519": null});
        record["initial_commitment"] = serde_json::json!(vec![9; 32]);
        assert_eq!(descriptor_bytes(&record).unwrap()[..2], [1, 1]);
        // Zero commitments, unknown versions and lengths are refused.
        assert!(descriptor_value(&[[1, 1].as_slice(), &[0; 32]].concat()).is_err());
        assert!(descriptor_value(&[[1, 2].as_slice(), &[9; 32]].concat()).is_err());
        assert!(descriptor_value(&[2, 4, 9]).is_err());
        assert!(descriptor_storage(&[3; 34]).is_err());
        record["initial_commitment"] = serde_json::json!(vec![9; 31]);
        assert_eq!(
            descriptor_bytes(&record).unwrap_err().to_string(),
            "codec error: hashed key commitments must contain 32 bytes"
        );
    }
}
