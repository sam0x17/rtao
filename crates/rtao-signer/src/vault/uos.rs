// Derived from RaoFoundation/subtensor sdk/python/bittensor/vault/uos.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Universal Offline Signatures framing for Polkadot Vault.
//!
//! This ports the pure byte protocol in the pinned Python `vault/uos.py`.
//! A signing frame is `0x53 ++ crypto ++ command ++ public key ++ payload ++
//! genesis hash`. Every frame, including an empty or single-frame input, has
//! the legacy `0x00 ++ count:u16-be ++ index:u16-be ++ chunk` envelope.
//!
//! Framing does not validate a metadata proof, recompute a signing hash, or
//! verify that supplied signing payloads describe the supplied call. Those
//! responsibilities belong to transaction preparation and the signer.

use std::fmt;

use crate::{CryptoType, SignPurpose, SigningRequest};

/// Substrate protocol discriminator in a UOS signing frame.
pub const SUBSTRATE_ID: u8 = 0x53;
/// Command carrying an already hashed transaction signing payload.
pub const CMD_SIGN_TX_HASH: u8 = 0x01;
/// Command carrying the call-length-prefixed transaction signing payload.
pub const CMD_SIGN_TX: u8 = 0x02;
/// Command carrying a raw message, displayed as text by Vault.
pub const CMD_SIGN_MSG: u8 = 0x03;
/// Command carrying an RFC-0078 proof followed by the full transaction payload.
pub const CMD_SIGN_TX_WITH_PROOF: u8 = 0x06;
/// Maximum number of data bytes in each multipart envelope.
pub const FRAME_SIZE: usize = 1024;

// The Python/Polkadot-JS condition is call_length * 2 + 2 > 5000.
// Comparing against 2499 avoids arithmetic overflow on the input length.
const MAX_DISPLAY_CALL_LENGTH: usize = 2499;

/// The borrowed transaction fields used by UOS, without transport JSON or secrets.
#[derive(Debug, Clone, Copy)]
pub struct Transaction<'a> {
    /// SCALE-encoded call bytes, without a compact length prefix.
    pub call_data: &'a [u8],
    /// Signed extension fields present in the extrinsic.
    pub included_in_extrinsic: &'a [u8],
    /// Additional signed fields absent from the extrinsic.
    pub included_in_signed_data: &'a [u8],
    /// Exact prepared signing bytes, already hashed when required by Substrate.
    pub payload: &'a [u8],
    /// Algorithm of the public signing key.
    pub crypto_type: CryptoType,
    /// Raw public signing key.
    pub public_key: &'a [u8; 32],
    /// Raw genesis hash of the selected chain.
    pub genesis_hash: &'a [u8; 32],
    /// Submission stage for the signer's UI; it is not a UOS wire field.
    pub purpose: SignPurpose,
}

impl<'a> Transaction<'a> {
    /// Borrow a prepared request and the signer's public identity without re-encoding.
    ///
    /// The request's context provides the raw genesis hash. Its purpose is
    /// preserved for UI labels; shield stages use the same UOS byte protocol.
    pub fn from_request(
        request: &'a SigningRequest<'_>,
        public_key: &'a [u8; 32],
        crypto_type: CryptoType,
    ) -> Self {
        Self {
            call_data: request.call,
            included_in_extrinsic: request.included_in_extrinsic,
            included_in_signed_data: request.included_in_signed_data,
            payload: request.payload,
            crypto_type,
            public_key,
            genesis_hash: &request.context.genesis_hash,
            purpose: request.purpose,
        }
    }
}

/// A value that cannot be represented or allocated as a UOS frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UosError {
    /// The raw public key has a length other than 32 bytes.
    InvalidPublicKeyLength {
        /// Supplied byte length.
        actual: usize,
    },
    /// The raw genesis hash has a length other than 32 bytes.
    InvalidGenesisHashLength {
        /// Supplied byte length.
        actual: usize,
    },
    /// Only ed25519 (zero) and sr25519 (one) are supported by this protocol.
    UnsupportedCryptoType(u8),
    /// A legacy oversized call was not supplied a 32-byte signing hash.
    OversizedCallWithoutHash {
        /// Length of the supplied prepared signing payload.
        payload_length: usize,
    },
    /// The compact call length is outside Python's supported three SCALE modes.
    CallTooLarge {
        /// Supplied call length, at least 2^30 bytes.
        length: usize,
    },
    /// Multipart counts use two bytes and cannot describe this many frames.
    TooManyFrames {
        /// Required number of multipart frames.
        count: usize,
    },
    /// Combining the supplied byte lengths overflowed the platform size type.
    LengthOverflow,
    /// The allocator could not reserve the requested output buffer.
    AllocationFailed {
        /// Required output length or multipart entry count.
        length: usize,
    },
}

impl fmt::Display for UosError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPublicKeyLength { actual } => {
                write!(formatter, "expected a 32-byte public key, got {actual}")
            }
            Self::InvalidGenesisHashLength { actual } => {
                write!(formatter, "expected a 32-byte genesis hash, got {actual}")
            }
            Self::UnsupportedCryptoType(crypto) => {
                write!(
                    formatter,
                    "unsupported crypto type for Vault signing: {crypto}"
                )
            }
            Self::OversizedCallWithoutHash { .. } => {
                formatter.write_str("oversized call without a hashed signing payload")
            }
            Self::CallTooLarge { length } => {
                write!(
                    formatter,
                    "call too large for compact-u32 framing: {length} bytes"
                )
            }
            Self::TooManyFrames { count } => {
                write!(formatter, "UOS multipart frame count exceeds u16: {count}")
            }
            Self::LengthOverflow => formatter.write_str("UOS output length overflow"),
            Self::AllocationFailed { length } => {
                write!(formatter, "cannot allocate UOS output of length {length}")
            }
        }
    }
}

impl std::error::Error for UosError {}

/// Return the transaction command and its legacy portal-metadata payload.
///
/// Calls through 2499 bytes retain a compact call length and both signed
/// extension parts, even when the prepared signing payload is already hashed
/// by Substrate's separate 256-byte rule. A call of 2500 bytes or more uses
/// the supplied 32-byte signing hash, without hashing it again.
pub fn sign_tx_payload(unsigned: &Transaction<'_>) -> Result<(u8, Vec<u8>), UosError> {
    if unsigned.call_data.len() > MAX_DISPLAY_CALL_LENGTH {
        if unsigned.payload.len() != 32 {
            return Err(UosError::OversizedCallWithoutHash {
                payload_length: unsigned.payload.len(),
            });
        }
        return Ok((CMD_SIGN_TX_HASH, concatenate(&[unsigned.payload])?));
    }
    Ok((CMD_SIGN_TX, full_payload(unsigned, &[])?))
}

/// Construct a complete signing frame, before the multipart envelope.
///
/// The raw algorithm code is checked so callers importing wire data receive
/// the same refusal as Python. The command is carried unchanged, including
/// commands outside the transaction helpers.
pub fn sign_frame(
    crypto_type: u8,
    cmd: u8,
    public_key: &[u8],
    payload: &[u8],
    genesis_hash: &[u8],
) -> Result<Vec<u8>, UosError> {
    if public_key.len() != 32 {
        return Err(UosError::InvalidPublicKeyLength {
            actual: public_key.len(),
        });
    }
    if genesis_hash.len() != 32 {
        return Err(UosError::InvalidGenesisHashLength {
            actual: genesis_hash.len(),
        });
    }
    if !matches!(crypto_type, 0 | 1) {
        return Err(UosError::UnsupportedCryptoType(crypto_type));
    }
    concatenate(&[
        &[SUBSTRATE_ID, crypto_type, cmd],
        public_key,
        payload,
        genesis_hash,
    ])
}

/// Wrap bytes in zero-based multipart frames with big-endian count and index.
///
/// Empty data produces the sole five-byte envelope `00 00 01 00 00`.
/// More than 65535 chunks is refused before allocating any multipart output.
pub fn multipart_frames(data: &[u8]) -> Result<Vec<Vec<u8>>, UosError> {
    let count = frame_count(data.len())?;
    let mut frames = Vec::new();
    frames
        .try_reserve_exact(usize::from(count))
        .map_err(|_| UosError::AllocationFailed {
            length: usize::from(count),
        })?;
    for index in 0..count {
        let start = usize::from(index) * FRAME_SIZE;
        let end = start.saturating_add(FRAME_SIZE).min(data.len());
        let header = [
            0,
            count.to_be_bytes()[0],
            count.to_be_bytes()[1],
            index.to_be_bytes()[0],
            index.to_be_bytes()[1],
        ];
        frames.push(concatenate(&[&header, &data[start..end]])?);
    }
    Ok(frames)
}

/// Return legacy transaction multipart frames and whether they use blind signing.
///
/// The boolean reports command `0x01`, not Substrate's separate signing-payload
/// hash rule. The transaction purpose is UI metadata and does not alter framing.
pub fn transaction_frames(unsigned: &Transaction<'_>) -> Result<(Vec<Vec<u8>>, bool), UosError> {
    let (cmd, payload) = sign_tx_payload(unsigned)?;
    let frame = sign_frame(
        u8::from(unsigned.crypto_type),
        cmd,
        unsigned.public_key,
        &payload,
        unsigned.genesis_hash,
    )?;
    Ok((multipart_frames(&frame)?, cmd == CMD_SIGN_TX_HASH))
}

/// Return multipart transaction frames carrying an RFC-0078 metadata proof.
///
/// The payload is `proof ++ compact(call length) ++ call ++ extrinsic fields ++
/// additional signed fields`, without a proof length prefix. Proof framing
/// always uses command `0x06` and retains the full call, including calls above
/// the legacy blind-signing threshold. Neither the proof nor the supplied
/// prepared signing bytes are changed or verified here.
pub fn transaction_frames_with_proof(
    unsigned: &Transaction<'_>,
    metadata_proof: &[u8],
) -> Result<Vec<Vec<u8>>, UosError> {
    let payload = full_payload(unsigned, metadata_proof)?;
    let frame = sign_frame(
        u8::from(unsigned.crypto_type),
        CMD_SIGN_TX_WITH_PROOF,
        unsigned.public_key,
        &payload,
        unsigned.genesis_hash,
    )?;
    multipart_frames(&frame)
}

fn full_payload(unsigned: &Transaction<'_>, proof: &[u8]) -> Result<Vec<u8>, UosError> {
    let (compact, length) = compact_length(unsigned.call_data.len())?;
    concatenate(&[
        proof,
        &compact[..length],
        unsigned.call_data,
        unsigned.included_in_extrinsic,
        unsigned.included_in_signed_data,
    ])
}

fn compact_length(length: usize) -> Result<([u8; 4], usize), UosError> {
    if length >= 1 << 30 {
        return Err(UosError::CallTooLarge { length });
    }
    // The preceding bound makes conversion exact on every supported platform.
    let value = length as u32;
    let (encoded, used) = if value < 1 << 6 {
        (value << 2, 1)
    } else if value < 1 << 14 {
        ((value << 2) | 1, 2)
    } else {
        ((value << 2) | 2, 4)
    };
    Ok((encoded.to_le_bytes(), used))
}

fn frame_count(length: usize) -> Result<u16, UosError> {
    let count = if length == 0 {
        1
    } else {
        (length - 1) / FRAME_SIZE + 1
    };
    u16::try_from(count).map_err(|_| UosError::TooManyFrames { count })
}

fn combined_length(lengths: impl IntoIterator<Item = usize>) -> Result<usize, UosError> {
    lengths
        .into_iter()
        .try_fold(0usize, |total, length| total.checked_add(length))
        .ok_or(UosError::LengthOverflow)
}

fn concatenate(parts: &[&[u8]]) -> Result<Vec<u8>, UosError> {
    let length = combined_length(parts.iter().map(|part| part.len()))?;
    let mut output = Vec::new();
    output
        .try_reserve_exact(length)
        .map_err(|_| UosError::AllocationFailed { length })?;
    for part in parts {
        output.extend_from_slice(part);
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::*;
    use crate::{SignerPayloadJson, SigningContext};

    // Hand-derived protocol fixtures, not a recorded Python-oracle result.
    const PUBLIC_KEY: [u8; 32] = *b"\x00\x01\x02\x03\x04\x05\x06\x07\x08\x09\x0a\x0b\x0c\x0d\x0e\x0f\x10\x11\x12\x13\x14\x15\x16\x17\x18\x19\x1a\x1b\x1c\x1d\x1e\x1f";
    const GENESIS: [u8; 32] = *b"\x20\x21\x22\x23\x24\x25\x26\x27\x28\x29\x2a\x2b\x2c\x2d\x2e\x2f\x30\x31\x32\x33\x34\x35\x36\x37\x38\x39\x3a\x3b\x3c\x3d\x3e\x3f";

    fn transaction<'a>(call_data: &'a [u8], payload: &'a [u8]) -> Transaction<'a> {
        Transaction {
            call_data,
            included_in_extrinsic: &[0x00, 0x10, 0x00],
            included_in_signed_data: &[0xab, 0xcd],
            payload,
            crypto_type: CryptoType::Sr25519,
            public_key: &PUBLIC_KEY,
            genesis_hash: &GENESIS,
            purpose: SignPurpose::Extrinsic,
        }
    }

    fn reassemble(frames: &[Vec<u8>]) -> Vec<u8> {
        frames
            .iter()
            .flat_map(|frame| &frame[5..])
            .copied()
            .collect()
    }

    #[test]
    fn compact_call_lengths_match_exact_scale_boundaries() {
        for (length, expected) in [
            (0, &[0x00][..]),
            (63, &[0xfc][..]),
            (64, &[0x01, 0x01][..]),
            (16383, &[0xfd, 0xff][..]),
            (16384, &[0x02, 0x00, 0x01, 0x00][..]),
            ((1 << 30) - 1, &[0xfe, 0xff, 0xff, 0xff][..]),
        ] {
            let (encoded, used) = compact_length(length).expect("representable length");
            assert_eq!(&encoded[..used], expected, "length {length}");
        }
        for length in [1 << 30, usize::MAX] {
            assert_eq!(
                compact_length(length),
                Err(UosError::CallTooLarge { length })
            );
        }
    }

    #[test]
    fn ordinary_payload_restores_call_length_and_both_extension_parts() {
        let unsigned = transaction(&[0x04, 0x00, 0x2a], &[0x99]);
        assert_eq!(
            sign_tx_payload(&unsigned).expect("full payload"),
            (
                CMD_SIGN_TX,
                vec![0x0c, 0x04, 0x00, 0x2a, 0x00, 0x10, 0x00, 0xab, 0xcd]
            )
        );
        let empty = Transaction {
            included_in_extrinsic: &[],
            included_in_signed_data: &[],
            ..transaction(&[], &[])
        };
        assert_eq!(
            sign_tx_payload(&empty).expect("empty call"),
            (CMD_SIGN_TX, vec![0])
        );
    }

    #[test]
    fn legacy_hash_boundary_is_2499_and_2500_call_bytes() {
        let call = vec![0x7b; 2500];
        let signing_hash = [0x9a; 32];
        let unsigned = transaction(&call[..2499], &signing_hash);
        let (cmd, payload) = sign_tx_payload(&unsigned).expect("displayable call");
        assert_eq!(cmd, CMD_SIGN_TX);
        assert_eq!(&payload[..2], &[0x0d, 0x27]);
        assert_eq!(&payload[2..2501], &call[..2499]);
        assert_eq!(&payload[2501..], &[0, 0x10, 0, 0xab, 0xcd]);

        let oversized = transaction(&call, &signing_hash);
        assert_eq!(
            sign_tx_payload(&oversized).expect("prepared signing hash"),
            (CMD_SIGN_TX_HASH, signing_hash.to_vec())
        );
        let (frames, hashed) = transaction_frames(&oversized).expect("hashed transaction");
        assert!(hashed);
        assert_eq!(frames.len(), 1);
        let frame = &frames[0];
        assert_eq!(&frame[..8], &[0, 0, 1, 0, 0, 0x53, 1, 1]);
        assert_eq!(&frame[8..40], &PUBLIC_KEY);
        assert_eq!(&frame[40..72], &signing_hash);
        assert_eq!(&frame[72..], &GENESIS);
    }

    #[test]
    fn substrate_256_byte_hash_rule_does_not_force_legacy_blind_signing() {
        let extensions = [0x88; 300];
        let signing_hash = [0x55; 32];
        let unsigned = Transaction {
            included_in_extrinsic: &extensions,
            ..transaction(&[0x04], &signing_hash)
        };
        let (frames, hashed) = transaction_frames(&unsigned).expect("visible full payload");
        assert!(!hashed);
        let frame = reassemble(&frames);
        assert_eq!(&frame[35..37], &[0x04, 0x04]);
        assert_eq!(&frame[37..337], &extensions);
        assert_eq!(&frame[337..339], &[0xab, 0xcd]);
        assert_eq!(&frame[339..], &GENESIS);
    }

    #[test]
    fn oversized_legacy_call_requires_exactly_32_prepared_hash_bytes() {
        let call = [0; 2500];
        for length in [0, 31, 33, 257] {
            let payload = vec![0; length];
            assert_eq!(
                sign_tx_payload(&transaction(&call, &payload)),
                Err(UosError::OversizedCallWithoutHash {
                    payload_length: length
                })
            );
        }
    }

    #[test]
    fn signing_frame_is_exact_and_preserves_command_and_algorithm() {
        let expected = b"\x53\x01\x02\x00\x01\x02\x03\x04\x05\x06\x07\x08\x09\x0a\x0b\x0c\x0d\x0e\x0f\x10\x11\x12\x13\x14\x15\x16\x17\x18\x19\x1a\x1b\x1c\x1d\x1e\x1f\xde\xad\xbe\xef\x20\x21\x22\x23\x24\x25\x26\x27\x28\x29\x2a\x2b\x2c\x2d\x2e\x2f\x30\x31\x32\x33\x34\x35\x36\x37\x38\x39\x3a\x3b\x3c\x3d\x3e\x3f";
        let frame = sign_frame(1, 2, &PUBLIC_KEY, &[0xde, 0xad, 0xbe, 0xef], &GENESIS)
            .expect("sr25519 frame");
        assert_eq!(frame, expected);
        let frame = sign_frame(0, 0xfe, &PUBLIC_KEY, &[], &GENESIS).expect("ed25519 frame");
        assert_eq!(&frame[..3], &[0x53, 0, 0xfe]);
        assert_eq!(&frame[3..35], &PUBLIC_KEY);
        assert_eq!(&frame[35..], &GENESIS);
    }

    #[test]
    fn signing_frame_validates_keys_hashes_and_crypto_in_python_order() {
        assert_eq!(
            sign_frame(255, 2, &[], &[], &[]),
            Err(UosError::InvalidPublicKeyLength { actual: 0 })
        );
        assert_eq!(
            sign_frame(255, 2, &PUBLIC_KEY, &[], &[0; 31]),
            Err(UosError::InvalidGenesisHashLength { actual: 31 })
        );
        for crypto in [2, 255] {
            assert_eq!(
                sign_frame(crypto, 2, &PUBLIC_KEY, &[], &GENESIS),
                Err(UosError::UnsupportedCryptoType(crypto))
            );
        }
        assert_eq!(
            sign_frame(0, 2, &[0; 33], &[], &GENESIS),
            Err(UosError::InvalidPublicKeyLength { actual: 33 })
        );
    }

    #[test]
    fn multipart_empty_1024_and_1025_bytes_have_exact_envelopes() {
        assert_eq!(
            multipart_frames(&[]).expect("empty envelope"),
            vec![vec![0, 0, 1, 0, 0]]
        );
        let mut data = vec![0xa5; 1024];
        let frames = multipart_frames(&data).expect("single full frame");
        assert_eq!(frames.len(), 1);
        assert_eq!(&frames[0][..5], &[0, 0, 1, 0, 0]);
        assert_eq!(&frames[0][5..], data);

        data.push(0x7e);
        let frames = multipart_frames(&data).expect("two frames");
        assert_eq!(frames.len(), 2);
        assert_eq!(&frames[0][..5], &[0, 0, 2, 0, 0]);
        assert_eq!(&frames[0][5..], &[0xa5; 1024]);
        assert_eq!(frames[1], [0, 0, 2, 0, 1, 0x7e]);
        assert_eq!(reassemble(&frames), data);
    }

    #[test]
    fn multipart_count_and_index_keep_both_big_endian_bytes() {
        // About 257 KiB exercises 0x0102 frames and indexes beyond 0x00ff.
        let mut data = vec![0xa5; 257 * 1024];
        data.push(0x7e);
        let frames = multipart_frames(&data).expect("258 multipart chunks");
        assert_eq!(frames.len(), 258);
        assert_eq!(&frames[0][..5], &[0, 1, 2, 0, 0]);
        assert_eq!(&frames[255][..5], &[0, 1, 2, 0, 255]);
        assert_eq!(&frames[256][..5], &[0, 1, 2, 1, 0]);
        assert_eq!(frames[257], [0, 1, 2, 1, 1, 0x7e]);
        assert_eq!(reassemble(&frames), data);
    }

    #[test]
    fn multipart_count_and_combined_lengths_refuse_overflow_without_allocation() {
        let largest = usize::from(u16::MAX) * FRAME_SIZE;
        assert_eq!(frame_count(0), Ok(1));
        assert_eq!(frame_count(largest), Ok(u16::MAX));
        assert_eq!(
            frame_count(largest + 1),
            Err(UosError::TooManyFrames { count: 65536 })
        );
        assert!(matches!(
            frame_count(usize::MAX),
            Err(UosError::TooManyFrames { .. })
        ));
        assert_eq!(
            combined_length([usize::MAX, 1]),
            Err(UosError::LengthOverflow)
        );
        assert_eq!(combined_length([usize::MAX - 1, 1]), Ok(usize::MAX));
    }

    #[test]
    fn transaction_frame_matches_complete_manual_fixture() {
        let unsigned = transaction(&[0x04, 0x00, 0x2a], &[0x99]);
        let (frames, hashed) = transaction_frames(&unsigned).expect("legacy transaction");
        assert!(!hashed);
        let expected = b"\x00\x00\x01\x00\x00\x53\x01\x02\x00\x01\x02\x03\x04\x05\x06\x07\x08\x09\x0a\x0b\x0c\x0d\x0e\x0f\x10\x11\x12\x13\x14\x15\x16\x17\x18\x19\x1a\x1b\x1c\x1d\x1e\x1f\x0c\x04\x00\x2a\x00\x10\x00\xab\xcd\x20\x21\x22\x23\x24\x25\x26\x27\x28\x29\x2a\x2b\x2c\x2d\x2e\x2f\x30\x31\x32\x33\x34\x35\x36\x37\x38\x39\x3a\x3b\x3c\x3d\x3e\x3f";
        assert_eq!(frames, vec![expected.to_vec()]);
    }

    #[test]
    fn proof_frame_concatenates_proof_without_prefix_and_ignores_prepared_hash() {
        let unsigned = transaction(&[0x04, 0x00, 0x2a], &[0x99; 32]);
        let frames = transaction_frames_with_proof(&unsigned, &[0xfa, 0xfb]).expect("proof frame");
        let expected = b"\x00\x00\x01\x00\x00\x53\x01\x06\x00\x01\x02\x03\x04\x05\x06\x07\x08\x09\x0a\x0b\x0c\x0d\x0e\x0f\x10\x11\x12\x13\x14\x15\x16\x17\x18\x19\x1a\x1b\x1c\x1d\x1e\x1f\xfa\xfb\x0c\x04\x00\x2a\x00\x10\x00\xab\xcd\x20\x21\x22\x23\x24\x25\x26\x27\x28\x29\x2a\x2b\x2c\x2d\x2e\x2f\x30\x31\x32\x33\x34\x35\x36\x37\x38\x39\x3a\x3b\x3c\x3d\x3e\x3f";
        assert_eq!(frames, vec![expected.to_vec()]);

        let frames = transaction_frames_with_proof(&unsigned, &[]).expect("empty proof is opaque");
        let frame = reassemble(&frames);
        assert_eq!(&frame[..3], &[0x53, 1, 6]);
        assert_eq!(&frame[35..44], &[0x0c, 4, 0, 0x2a, 0, 0x10, 0, 0xab, 0xcd]);
    }

    #[test]
    fn proof_frame_keeps_2500_byte_call_instead_of_switching_to_hash() {
        let call = [0x7b; 2500];
        let unsigned = transaction(&call, &[]);
        let frames =
            transaction_frames_with_proof(&unsigned, &[0xfa, 0xfb]).expect("full proof call");
        assert_eq!(frames.len(), 3);
        for (index, envelope) in frames.iter().enumerate() {
            assert_eq!(&envelope[..4], &[0, 0, 3, 0]);
            assert_eq!(envelope[4], index as u8);
        }
        let frame = reassemble(&frames);
        assert_eq!(&frame[..3], &[0x53, 1, 6]);
        assert_eq!(&frame[35..39], &[0xfa, 0xfb, 0x11, 0x27]);
        assert_eq!(&frame[39..2539], &call);
        assert_eq!(&frame[2539..2544], &[0, 0x10, 0, 0xab, 0xcd]);
        assert_eq!(&frame[2544..], &GENESIS);
    }

    #[test]
    fn request_conversion_preserves_borrows_and_purpose_without_wire_changes() {
        let context = SigningContext {
            metadata_bytes: vec![],
            chain_info: rtao_core::digest::ChainInfo {
                spec_version: 478,
                spec_name: "node-subtensor".into(),
                base58_prefix: 42,
                decimals: 9,
                token_symbol: "TAO".into(),
            },
            transaction_version: 1,
            genesis_hash: GENESIS,
        };
        let json = SignerPayloadJson::new();
        let mut expected = None;
        for purpose in [
            SignPurpose::Extrinsic,
            SignPurpose::ShieldInner,
            SignPurpose::ShieldCarrier,
        ] {
            let request = SigningRequest {
                call: &[4, 0, 0x2a],
                included_in_extrinsic: &[0, 0x10, 0],
                included_in_signed_data: &[0xab, 0xcd],
                payload: &[0x99],
                payload_json: &json,
                context: &context,
                purpose,
            };
            let unsigned = Transaction::from_request(&request, &PUBLIC_KEY, CryptoType::Ed25519);
            assert_eq!(unsigned.purpose, purpose);
            assert!(std::ptr::eq(unsigned.call_data, request.call));
            assert!(std::ptr::eq(unsigned.payload, request.payload));
            assert!(std::ptr::eq(unsigned.genesis_hash, &context.genesis_hash));
            let result = transaction_frames(&unsigned).expect("all purposes frame identically");
            assert_eq!(result.0[0][6], 0);
            if let Some(expected) = &expected {
                assert_eq!(&result, expected);
            } else {
                expected = Some(result);
            }
        }
    }
}
