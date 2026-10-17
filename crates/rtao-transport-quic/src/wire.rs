//! The btlightning frame layout and MessagePack envelope structs.
//!
//! This module performs no network I/O beyond the generic frame reader and
//! writer, which work with any tokio byte stream. Envelopes encode as
//! positional MessagePack arrays (`rmp_serde::to_vec`) and decode from either
//! arrays or maps, exactly like a btlightning peer.

use std::collections::HashMap;

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// The single ALPN protocol both peers offer.
pub const ALPN: &[u8] = b"btlightning";
/// Length of the `[type, u32 length]` frame header.
pub const FRAME_HEADER_LEN: usize = 5;
/// Default maximum frame payload (64 MiB).
pub const DEFAULT_MAX_FRAME_PAYLOAD: u32 = 64 * 1024 * 1024;
/// Smallest configurable maximum frame payload (1 MiB).
pub const MIN_MAX_FRAME_PAYLOAD: u32 = 1024 * 1024;
/// Accepted handshake clock lead of the validator, in seconds.
pub const MAX_FUTURE_SKEW_SECS: u64 = 60;
/// Default maximum handshake signature age, in seconds.
pub const DEFAULT_MAX_SIGNATURE_AGE_SECS: u64 = 300;
/// Largest configurable handshake signature age, in seconds.
pub const MAX_MAX_SIGNATURE_AGE_SECS: u64 = 3600;
/// Payloads up to this size are read with one allocation.
const INCREMENTAL_READ_THRESHOLD: usize = 1024 * 1024;
/// Larger payloads are read in chunks of this size.
const READ_CHUNK_SIZE: usize = 64 * 1024;

/// A dynamic MessagePack value carried in request and response maps.
pub type Value = rmpv::Value;
/// The `data` map of a request or response.
pub type DataMap = HashMap<String, Value>;

/// Error strings a server sends to its peer. They must match btlightning
/// byte for byte.
pub mod errors {
    /// The connection has not completed a handshake.
    pub const AUTH_FAILED: &str = "authentication failed";
    /// The `SynapsePacket` could not be decoded.
    pub const INVALID_REQUEST: &str = "invalid request format";
    /// The handler returned an error or panicked.
    pub const PROCESSING_FAILED: &str = "request processing failed";
    /// No handler is registered for the synapse type.
    pub const UNRECOGNIZED: &str = "unrecognized synapse type";
    /// The handler exceeded its time budget.
    pub const TIMED_OUT: &str = "handler timed out";
    /// The response could not be encoded.
    pub const SERIALIZATION: &str = "internal serialization error";
    /// A streaming handler failed, was cancelled or panicked.
    pub const STREAM_FAILED: &str = "stream processing failed";
    /// Writing a stream chunk failed.
    pub const STREAM_WRITE_FAILED: &str = "stream write failed";
}

/// A frame or envelope that cannot be read, written, encoded or decoded.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum WireError {
    /// The type byte names no message.
    #[error("unknown message type: 0x{0:02x}")]
    UnknownType(u8),
    /// The declared payload exceeds the configured maximum.
    #[error("frame payload {len} bytes exceeds maximum {max}")]
    Oversize {
        /// Declared payload length.
        len: u64,
        /// Configured maximum.
        max: u32,
    },
    /// The stream ended inside a header or payload.
    #[error("stream closed after {got} of {want} bytes")]
    Truncated {
        /// Bytes received.
        got: usize,
        /// Bytes required.
        want: usize,
    },
    /// The stream failed while reading or writing.
    #[error("stream i/o error: {0}")]
    Io(String),
    /// A value could not be encoded.
    #[error("encode error: {0}")]
    Encode(String),
    /// A payload could not be decoded.
    #[error("decode error: {0}")]
    Decode(String),
}

/// The one-byte frame discriminant.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MessageType {
    /// Validator handshake, client to server.
    HandshakeRequest = 0x01,
    /// Miner handshake reply, server to client.
    HandshakeResponse = 0x02,
    /// Synapse request, client to server.
    SynapsePacket = 0x03,
    /// Unary synapse reply, server to client.
    SynapseResponse = 0x04,
    /// One streamed chunk, server to client.
    StreamChunk = 0x05,
    /// End of a streamed reply, server to client.
    StreamEnd = 0x06,
}

impl TryFrom<u8> for MessageType {
    type Error = WireError;

    fn try_from(value: u8) -> Result<Self, WireError> {
        Ok(match value {
            0x01 => Self::HandshakeRequest,
            0x02 => Self::HandshakeResponse,
            0x03 => Self::SynapsePacket,
            0x04 => Self::SynapseResponse,
            0x05 => Self::StreamChunk,
            0x06 => Self::StreamEnd,
            other => return Err(WireError::UnknownType(other)),
        })
    }
}

/// A validated frame header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameHeader {
    /// Message type.
    pub kind: MessageType,
    /// Payload length in bytes.
    pub len: u32,
}

/// Parse a header, checking the type byte before the length.
pub fn parse_header(bytes: &[u8; FRAME_HEADER_LEN], max: u32) -> Result<FrameHeader, WireError> {
    let kind = MessageType::try_from(bytes[0])?;
    let len = u32::from_be_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]);
    if len > max {
        return Err(WireError::Oversize {
            len: u64::from(len),
            max,
        });
    }
    Ok(FrameHeader { kind, len })
}

/// Parse one complete frame from a byte slice, returning its payload slice.
///
/// Bytes after the frame are ignored.
pub fn parse_frame(data: &[u8], max: u32) -> Result<(MessageType, &[u8]), WireError> {
    let header: &[u8; FRAME_HEADER_LEN] = data
        .get(..FRAME_HEADER_LEN)
        .and_then(|h| h.try_into().ok())
        .ok_or(WireError::Truncated {
            got: data.len(),
            want: FRAME_HEADER_LEN,
        })?;
    let header = parse_header(header, max)?;
    let end = FRAME_HEADER_LEN + header.len as usize;
    let payload = data
        .get(FRAME_HEADER_LEN..end)
        .ok_or(WireError::Truncated {
            got: data.len() - FRAME_HEADER_LEN,
            want: header.len as usize,
        })?;
    Ok((header.kind, payload))
}

/// Encode a complete frame (header and payload) into one buffer.
pub fn encode_frame(kind: MessageType, payload: &[u8]) -> Result<Vec<u8>, WireError> {
    let len = u32::try_from(payload.len()).map_err(|_| WireError::Oversize {
        len: payload.len() as u64,
        max: u32::MAX,
    })?;
    let mut frame = Vec::with_capacity(FRAME_HEADER_LEN + payload.len());
    frame.push(kind as u8);
    frame.extend_from_slice(&len.to_be_bytes());
    frame.extend_from_slice(payload);
    Ok(frame)
}

async fn read_exact<R: AsyncRead + Unpin>(reader: &mut R, buf: &mut [u8]) -> Result<(), WireError> {
    let mut offset = 0;
    while offset < buf.len() {
        let n = reader
            .read(&mut buf[offset..])
            .await
            .map_err(|e| WireError::Io(e.to_string()))?;
        if n == 0 {
            return Err(WireError::Truncated {
                got: offset,
                want: buf.len(),
            });
        }
        offset += n;
    }
    Ok(())
}

/// Read and validate a frame header.
pub async fn read_header<R: AsyncRead + Unpin>(
    reader: &mut R,
    max: u32,
) -> Result<FrameHeader, WireError> {
    let mut header = [0u8; FRAME_HEADER_LEN];
    read_exact(reader, &mut header).await?;
    parse_header(&header, max)
}

/// Read a payload of `len` bytes.
///
/// Memory grows with the bytes actually received, so a peer that declares a
/// large length without sending it cannot force a large allocation. Every
/// stream error ends the read.
pub async fn read_payload<R: AsyncRead + Unpin>(
    reader: &mut R,
    len: u32,
) -> Result<Vec<u8>, WireError> {
    let len = len as usize;
    if len <= INCREMENTAL_READ_THRESHOLD {
        let mut payload = vec![0u8; len];
        read_exact(reader, &mut payload).await?;
        return Ok(payload);
    }
    let mut payload = Vec::with_capacity(INCREMENTAL_READ_THRESHOLD);
    while payload.len() < len {
        let start = payload.len();
        let chunk = (len - start).min(READ_CHUNK_SIZE);
        payload.resize(start + chunk, 0);
        read_exact(reader, &mut payload[start..])
            .await
            .map_err(|e| match e {
                WireError::Truncated { got, .. } => WireError::Truncated {
                    got: start + got,
                    want: len,
                },
                other => other,
            })?;
    }
    Ok(payload)
}

/// Read one frame from a stream (see [`read_header`] and [`read_payload`]).
pub async fn read_frame<R: AsyncRead + Unpin>(
    reader: &mut R,
    max: u32,
) -> Result<(MessageType, Vec<u8>), WireError> {
    let header = read_header(reader, max).await?;
    Ok((header.kind, read_payload(reader, header.len).await?))
}

/// Write one frame. The header and payload are sent as one buffer.
pub async fn write_frame<W: AsyncWrite + Unpin>(
    writer: &mut W,
    kind: MessageType,
    payload: &[u8],
) -> Result<(), WireError> {
    let frame = encode_frame(kind, payload)?;
    writer
        .write_all(&frame)
        .await
        .map_err(|e| WireError::Io(e.to_string()))
}

/// Encode an envelope as a positional MessagePack array.
pub fn encode<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, WireError> {
    rmp_serde::to_vec(value).map_err(|e| WireError::Encode(e.to_string()))
}

/// Decode an envelope from a MessagePack array or map.
///
/// Like btlightning, bytes after the first value are ignored.
pub fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, WireError> {
    rmp_serde::from_slice(bytes).map_err(|e| WireError::Decode(e.to_string()))
}

/// Validator-to-miner handshake (`0x01`).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct HandshakeRequest {
    /// Validator SS58 address, signed as sent.
    pub validator_hotkey: String,
    /// Unix seconds at signing.
    pub timestamp: u64,
    /// 32 hexadecimal characters.
    pub nonce: String,
    /// Standard padded base64 of the 64-byte sr25519 signature.
    pub signature: String,
}

/// Miner-to-validator handshake reply (`0x02`).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct HandshakeResponse {
    /// Miner SS58 address, or empty on a pre-authentication rejection.
    pub miner_hotkey: String,
    /// Unix seconds at signing.
    pub timestamp: u64,
    /// Standard padded base64 signature, or empty on rejection.
    pub signature: String,
    /// Whether the miner accepted the handshake.
    pub accepted: bool,
    /// Opaque connection label, or empty on rejection.
    pub connection_id: String,
    /// Base64 BLAKE2b-256 fingerprint of the miner certificate.
    #[serde(default)]
    pub cert_fingerprint: Option<String>,
}

/// Synapse request (`0x03`).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SynapsePacket {
    /// Handler name.
    pub synapse_type: String,
    /// Request payload.
    pub data: DataMap,
    /// Client unix seconds; never validated.
    pub timestamp: u64,
}

/// Unary synapse reply (`0x04`).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SynapseResponse {
    /// Whether the handler succeeded.
    pub success: bool,
    /// Handler output, empty on error.
    pub data: DataMap,
    /// Server unix seconds; never validated.
    pub timestamp: u64,
    /// Error string on failure.
    pub error: Option<String>,
}

/// One streamed chunk (`0x05`).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct StreamChunk {
    /// Raw chunk bytes.
    #[serde(with = "serde_bytes")]
    pub data: Vec<u8>,
}

/// End of a streamed reply (`0x06`).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct StreamEnd {
    /// Whether the stream completed.
    pub success: bool,
    /// Error string on failure.
    pub error: Option<String>,
}

impl HandshakeResponse {
    /// The pre-authentication rejection shape `["", now, "", false, "", nil]`.
    pub fn rejected_preauth(now: u64) -> Self {
        Self {
            miner_hotkey: String::new(),
            timestamp: now,
            signature: String::new(),
            accepted: false,
            connection_id: String::new(),
            cert_fingerprint: None,
        }
    }

    /// The post-parse rejection shape `[miner, now, "", false, "", nil]`.
    pub fn rejected(miner_hotkey: &str, now: u64) -> Self {
        Self {
            miner_hotkey: miner_hotkey.to_owned(),
            ..Self::rejected_preauth(now)
        }
    }
}

impl SynapseResponse {
    /// An error reply with an empty data map.
    pub fn error(message: &str, now: u64) -> Self {
        Self {
            success: false,
            data: DataMap::new(),
            timestamp: now,
            error: Some(message.to_owned()),
        }
    }
}

impl StreamEnd {
    /// A failed stream end.
    pub fn error(message: &str) -> Self {
        Self {
            success: false,
            error: Some(message.to_owned()),
        }
    }
}

/// Current unix time in seconds (zero before the epoch).
pub fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The write budget for one frame (a client request or a server reply): 5 s
/// plus 1 s per started MiB, capped at 60 s.
pub fn write_budget(len: usize) -> std::time::Duration {
    let mib = len.div_ceil(1024 * 1024) as u64;
    std::time::Duration::from_secs(5u64.saturating_add(mib).min(60))
}
