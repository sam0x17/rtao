// Derived from RaoFoundation/polkadot-sdk substrate/primitives/core/src/crypto.rs @ cacb4310f20c7cac83eb3ccd8ed5a5ad4212608a (Apache-2.0); modified by rtao.
//! Prefix-agnostic SS58 decoding, as bittensor-wallet's `Keypair(ss58_address=…)`.

use std::fmt;

use blake2::{Blake2b512, Digest};
use rtao_primitives::AccountId;

/// sp-core `PublicError` variants reachable from `from_ss58check_with_version`.
///
/// `Display` renders sp-core's `Debug` text (its std build prints the error
/// message), which is how Python reports it: `Invalid SS58 address: {text}`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ss58Error {
    /// The text is not base58.
    BadBase58,
    /// The decoded length does not fit a prefix, 32-byte key and checksum.
    BadLength,
    /// The first byte is not a valid one- or two-byte prefix.
    InvalidPrefix,
    /// The two checksum bytes do not match.
    InvalidChecksum,
}

impl fmt::Display for Ss58Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::BadBase58 => "Base 58 requirement is violated",
            Self::BadLength => "Length is bad",
            Self::InvalidPrefix => "Invalid SS58 prefix byte.",
            Self::InvalidChecksum => "Invalid checksum",
        })
    }
}
impl std::error::Error for Ss58Error {}

const MAX_SS58_TEXT_BYTES: usize = rtao_btauth::MAX_SENDER_SS58_BYTES;

/// Decode an SS58 AccountId32 address with any checksum-valid prefix.
///
/// Mirrors sp-core's `from_ss58check_with_version`, which bittensor-wallet
/// uses and which ignores the network prefix. Text longer than 49 bytes cannot
/// encode a 35-byte payload and is refused before decoding.
pub fn decode_ss58_any_prefix(text: &str) -> Result<(AccountId, u16), Ss58Error> {
    if text.len() > MAX_SS58_TEXT_BYTES {
        return Err(
            if text
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() && !b"0OIl".contains(&byte))
            {
                Ss58Error::BadLength
            } else {
                Ss58Error::BadBase58
            },
        );
    }
    let data = bs58::decode(text)
        .into_vec()
        .map_err(|_| Ss58Error::BadBase58)?;
    if data.len() < 2 {
        return Err(Ss58Error::BadLength);
    }
    let (prefix_len, ident) = match data[0] {
        0..=63 => (1, u16::from(data[0])),
        64..=127 => {
            let lower = (data[0] << 2) | (data[1] >> 6);
            let upper = data[1] & 0b0011_1111;
            (2, u16::from(lower) | (u16::from(upper) << 8))
        }
        _ => return Err(Ss58Error::InvalidPrefix),
    };
    if data.len() != prefix_len + 32 + 2 {
        return Err(Ss58Error::BadLength);
    }
    let mut hasher = Blake2b512::new();
    hasher.update(b"SS58PRE");
    hasher.update(&data[..prefix_len + 32]);
    let hash = hasher.finalize();
    if data[prefix_len + 32..] != hash[..2] {
        return Err(Ss58Error::InvalidChecksum);
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(&data[prefix_len..prefix_len + 32]);
    Ok((AccountId(key), ident))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn decodes_any_prefix() {
        let alice = "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY";
        let (account, prefix) = decode_ss58_any_prefix(alice).unwrap();
        assert_eq!(prefix, 42);
        assert_eq!(account.to_string(), alice);
        for format in [0u16, 2, 63, 64, 255, 4096, 16383] {
            let text = account.to_ss58(format);
            assert_eq!(decode_ss58_any_prefix(&text).unwrap(), (account, format));
        }
        assert_eq!(decode_ss58_any_prefix("0OIl"), Err(Ss58Error::BadBase58));
        assert_eq!(decode_ss58_any_prefix("5Grw"), Err(Ss58Error::BadLength));
        assert_eq!(decode_ss58_any_prefix(""), Err(Ss58Error::BadLength));
        let mut corrupt = alice.to_owned();
        corrupt.replace_range(47.., "R");
        assert_eq!(
            decode_ss58_any_prefix(&corrupt),
            Err(Ss58Error::InvalidChecksum)
        );
        assert_eq!(
            Ss58Error::BadBase58.to_string(),
            "Base 58 requirement is violated"
        );
    }
}
