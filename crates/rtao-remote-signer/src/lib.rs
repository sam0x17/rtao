//! Policy-checked remote transaction signing over btauth/1.
//!
//! This crate implements the rtao-authored `rtao-remote-signer/1` protocol:
//! a [`client::RemoteSigner`] that implements [`rtao_signer::Signer`] by
//! sending the public views of a prepared transaction to a signing server,
//! and a reference [`server`] that decodes
//! the call with its own trusted runtime, re-derives every signed byte,
//! applies an operator policy and only then signs.
//!
//! Requests are authenticated with btauth/1 using a client key that is
//! distinct from the signing account. Responses are self-authenticating: the
//! client verifies the returned signature against its own payload.
//!
//! There is no upstream Python counterpart for this protocol; only the
//! btauth/1 layer and the extrinsic payload construction underneath it are
//! upstream-normative.

#![doc = include_str!("../README.md")]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![deny(missing_docs)]

pub mod wire;

/// The `RemoteSigner` client.
#[cfg(feature = "client")]
#[cfg_attr(docsrs, doc(cfg(feature = "client")))]
pub mod client;

/// The reference signing server.
#[cfg(feature = "server")]
#[cfg_attr(docsrs, doc(cfg(feature = "server")))]
pub mod server;

/// Configuration file loading for the `rtao-remote-signerd` daemon.
#[cfg(feature = "server-bin")]
#[cfg_attr(docsrs, doc(cfg(feature = "server-bin")))]
pub mod daemon;

#[cfg(feature = "client")]
#[cfg_attr(docsrs, doc(cfg(feature = "client")))]
pub use client::{RemoteSigner, RemoteSignerConfig, RemoteSignerError};
pub use wire::{ACCOUNTS_PATH, PROTOCOL, SIGN_PATH};
