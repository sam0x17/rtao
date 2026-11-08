//! Self-contained receiving addresses (Python `bittensor.receiving`).
//!
//! A `tao_` address carries a hashed or ML-DSA account's public descriptor and
//! a checksum; it has no network field. Its account is the runtime's existing
//! `AccountId`, derived from the descriptor by the core's codec. Resolving one
//! for a read never needs the chain; a write that cannot carry a registration
//! guard must instead check the finalized registration (Python's
//! `account_for_registered_write`).

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
}
