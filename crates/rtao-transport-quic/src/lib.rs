//! btlightning-compatible QUIC transport for Bittensor neurons.
//!
//! This crate reimplements the wire protocol of
//! [btlightning](https://github.com/inference-labs-inc/lightning) (pinned at
//! `53c090b`, v0.2.15) on quinn and rustls with the ring provider. It does not
//! depend on btlightning, so it shares rtao's `sp-core`-free signing stack.
//!
//! - [`wire`]: frame layout, envelope structs and MessagePack rules.
//! - [`data`]: typed conversion of request and response maps.
//! - [`auth`]: handshake signing and verification.
//! - [`tls`]: ring TLS 1.3 and QUIC configuration.
//! - [`client`] (feature `client`): authenticated connections, unary and
//!   streaming queries.
//! - [`server`] (feature `server`): a miner endpoint with routing, rate
//!   limits, replay protection and per-connection authorization.
//!
//! Deliberate differences from btlightning, all invisible on the wire:
//!
//! - the client verifies the TLS 1.3 `CertificateVerify` signature;
//! - the server authorizes by QUIC connection rather than remote address and
//!   allows one validator identity per connection;
//! - replay protection keeps nonces for the signature age plus the future
//!   skew (plus a clock-step margin by default) and fails closed at capacity
//!   instead of evicting live nonces;
//! - unauthenticated connections are capped, evicted oldest-first from the
//!   busiest source when full, limited to small frames, and closed after the
//!   handshake timeout; per-source limits group IPv6 by /64;
//! - server reply writes have a deadline;
//! - a streaming reply is never cut inside a frame;
//! - [`data::from_data`] accepts every enum shape btlightning encoders emit.

#![cfg_attr(docsrs, feature(doc_cfg))]
#![deny(missing_docs)]

pub mod auth;
pub mod data;
pub mod tls;
pub mod wire;

#[cfg(feature = "client")]
#[cfg_attr(docsrs, doc(cfg(feature = "client")))]
pub mod client;
#[cfg(feature = "server")]
#[cfg_attr(docsrs, doc(cfg(feature = "server")))]
pub mod server;

pub use auth::AuthError;
pub use data::{DataError, from_data, to_data};
pub use tls::TlsError;
pub use wire::{DataMap, Value, WireError};

#[cfg(feature = "client")]
pub use client::{QuicClient, QuicClientConfig, QuicTarget, Reply};
#[cfg(feature = "server")]
pub use server::{QuicRouter, QuicServer, QuicServerConfig, ServerStats};

/// A transport, handshake or protocol failure.
#[derive(Debug, thiserror::Error)]
pub enum QuicError {
    /// Invalid configuration.
    #[error("config error: {0}")]
    Config(String),
    /// TLS or QUIC configuration failure.
    #[error(transparent)]
    Tls(#[from] TlsError),
    /// The QUIC connection could not be established or was lost.
    #[error("connection error: {0}")]
    Connection(String),
    /// The handshake failed or was rejected.
    #[error("handshake error: {0}")]
    Handshake(String),
    /// Handshake signing or verification failed.
    #[error("handshake error: {0}")]
    Auth(#[from] AuthError),
    /// A frame or envelope was malformed.
    #[error("transport error: {0}")]
    Wire(#[from] WireError),
    /// A typed value could not be converted.
    #[error("serialization error: {0}")]
    Data(#[from] DataError),
    /// The peer sent an unexpected frame or the stream failed.
    #[error("transport error: {0}")]
    Transport(String),
    /// A streamed reply failed.
    #[error("stream error: {0}")]
    Stream(String),
    /// The server reported `success = false`.
    #[error("handler error: {0}")]
    Handler(String),
    /// An operation exceeded its deadline.
    #[error("timed out: {0}")]
    Timeout(&'static str),
    /// Reconnection to this address is backing off.
    #[error("connection error: reconnect backing off for {0:?}")]
    Backoff(std::time::Duration),
}
