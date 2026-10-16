// Derived from RaoFoundation/subtensor sdk/python/bittensor/wallets.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Message operations share the wallet's authenticated private-key reader.

use crate::{Wallet, WalletError};
use rtao_signer::SignerRole;
#[cfg(feature = "encrypted")]
use zeroize::Zeroizing;

/// Public signer identity and a bare lowercase signature over exact UTF-8 bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedMessage {
    /// Bittensor SS58 signer address.
    pub ss58: String,
    /// A 64-byte signature encoded without a `0x` prefix.
    pub signature: String,
}

/// Public sealed-box ciphertext and its requested recipient.
#[cfg(feature = "encrypted")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncryptedMessage {
    /// `0x`-prefixed sealed-box ciphertext.
    pub ciphertext: String,
    /// Recipient address, in the supplied spelling.
    pub recipient: String,
}

/// Verify Python's default sr25519 message-signature contract.
///
/// Malformed hex or SS58 addresses return false. The upstream verifier accepts
/// exact UTF-8 bytes and its historical `<Bytes>` wrapper. SS58 carries no
/// scheme discriminator; this follows Python's public-only Keypair default.
#[doc(alias = "bittensor.wallets.verify_message")]
pub fn verify_message(message: &str, signature: &str, ss58_address: &str) -> bool {
    python_hex(signature.strip_prefix("0x").unwrap_or(signature)).is_some_and(|signature| {
        rtao_core::keys::verify(message.as_bytes(), &signature, ss58_address, 1).unwrap_or(false)
    })
}

// bytes.fromhex skips ASCII whitespace between complete byte pairs, but not
// between the two nibbles of one byte. The input remains bounded before any
// allocation. Ciphertext and signature bytes are public.
fn python_hex(encoded: &str) -> Option<Vec<u8>> {
    const MAX_HEX_BYTES: usize = 32 * 1024 * 1024;
    if encoded.len() > MAX_HEX_BYTES {
        return None;
    }
    let nibble = |byte: u8| match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    };
    let mut bytes = encoded.bytes();
    let mut result = Vec::with_capacity(encoded.len() / 2);
    while let Some(first) = bytes.next() {
        if matches!(first, b' ' | b'\t'..=b'\r') {
            continue;
        }
        result.push(nibble(first)?.checked_mul(16)? + nibble(bytes.next()?)?);
    }
    Some(result)
}

/// Encrypt exact UTF-8 text for an ed25519 recipient using the core sealed box.
///
/// The optional `encrypted` capability enables host cryptography. Plaintext is
/// copied directly into a zeroizing allocation before offloading.
#[cfg(feature = "encrypted")]
#[doc(alias = "bittensor.wallets.encrypt_message")]
pub async fn encrypt_message(
    message: &str,
    recipient_ss58: &str,
) -> Result<EncryptedMessage, WalletError> {
    let message = Zeroizing::new(message.as_bytes().to_vec());
    let recipient = recipient_ss58.to_owned();
    let result_recipient = recipient.clone();
    let bytes = rtao_core::offload(move || {
        rtao_core::keys::Keypair::encrypt_for(&recipient, &message, 0).map_err(WalletError::from)
    })
    .await
    .map_err(|_| WalletError::Blocking("message encryption task failed".into()))??;
    Ok(EncryptedMessage {
        ciphertext: format!("0x{}", hex::encode(bytes)),
        recipient: result_recipient,
    })
}

impl Wallet {
    /// Sign exact UTF-8 message bytes with the selected local wallet key.
    #[doc(alias = "bittensor.wallets.sign_message")]
    pub async fn sign_message(
        &self,
        message: &str,
        role: SignerRole,
    ) -> Result<SignedMessage, WalletError> {
        let signer = self.state(role).unlock().await?;
        let signature = signer.sign_raw(message.as_bytes()).await.map_err(|_| {
            WalletError::Core(rtao_core::CoreError::Crypto(
                "message signing failed".into(),
            ))
        })?;
        Ok(SignedMessage {
            ss58: signer.account().to_string(),
            signature: hex::encode(signature),
        })
    }

    /// Decrypt a bare or `0x`-prefixed sealed box into a guarded UTF-8 string.
    ///
    /// Password selection and KDF/keyfile parsing use the same implementation
    /// as signing. Private keys and plaintext never leave as ordinary owned
    /// strings. Encrypted-hotkey sources remain the hotkey's own policy.
    #[cfg(feature = "encrypted")]
    #[doc(alias = "bittensor.wallets.decrypt_message")]
    #[cfg_attr(docsrs, doc(cfg(feature = "encrypted")))]
    pub async fn decrypt_message(
        &self,
        ciphertext_hex: &str,
        use_hotkey: bool,
    ) -> Result<secrecy::SecretString, WalletError> {
        const MAX_CIPHERTEXT_HEX: usize = 32 * 1024 * 1024;
        let encoded = ciphertext_hex.strip_prefix("0x").unwrap_or(ciphertext_hex);
        if encoded.len() > MAX_CIPHERTEXT_HEX {
            return Err(WalletError::Core(rtao_core::CoreError::Crypto(
                "message ciphertext exceeds 16 MiB".into(),
            )));
        }
        let bytes = python_hex(encoded).ok_or_else(|| {
            WalletError::Core(rtao_core::CoreError::Crypto(
                "ciphertext must be hexadecimal".into(),
            ))
        })?;
        let role = if use_hotkey {
            SignerRole::Hotkey
        } else {
            SignerRole::Coldkey
        };
        self.state(role).decrypt_message_bytes(bytes).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn python_hex_whitespace_is_allowed_only_between_byte_pairs() {
        assert_eq!(
            python_hex("\t01 ab\nCD\x0bfe\x0c00\r"),
            Some(vec![1, 171, 205, 254, 0])
        );
        assert_eq!(python_hex("a b"), None);
        assert_eq!(python_hex("01\u{85}ab"), None);
        assert_eq!(python_hex("f"), None);
    }

    #[test]
    fn oversized_message_hex_refuses_before_decoding() {
        let encoded = "0".repeat(32 * 1024 * 1024 + 1);
        assert!(python_hex(&encoded).is_none());
    }
}
