// Derived from opentensor/bittensor bittensor/core/synapse.py @ v10.5.0 (MIT); modified by rtao.
// Derived from opentensor/bittensor bittensor/core/axon.py @ v10.5.0 (MIT); modified by rtao.
// Derived from opentensor/bittensor bittensor/core/dendrite.py @ v10.5.0 (MIT); modified by rtao.
//! Wire compatibility with `bittensor==10.5.0` Synapse, Axon and Dendrite peers.
//!
//! The v10 protocol carries a JSON-encoded pydantic model over plain HTTP. A
//! dendrite signs `"{nonce}.{hotkey}.{axon_hotkey}.{uuid}.{body_hash}"` with
//! sr25519 and sends the signature and metadata in `bt_header_*` headers.
//! Only the subnet's `required_hash_fields` are bound by that signature; the
//! remaining body is unauthenticated.
//!
//! This crate provides:
//!
//! - the codec ([`Envelope`], [`TerminalInfo`], [`Synapse`], [`parse_head`])
//!   with Python's `str()`/`repr()` body hash ([`PyValue`], [`get_hash`]);
//! - Python's `default_verify` ([`default_verify`]) with stricter defaults
//!   ([`V10VerifyPolicy`]) and a bounded, expiry-only nonce table
//!   ([`BoundedNonceTable`]);
//! - an axum axon (feature `axon`) and a reqwest dendrite (feature
//!   `dendrite`), including streaming responses;
//! - the msgpack-numpy `Tensor` wire form (feature `tensor`).
//!
//! # Divergences from Python
//!
//! Every divergence keeps wire compatibility with honest v10.5.0 peers:
//!
//! - The verifier always applies the nonce window (the unsigned
//!   `bt_header_dendrite_version` cannot disable it), rejects non-finite or
//!   negative `timeout` values, caps timeout widening at 300 seconds and
//!   refuses nonces more than 60 seconds ahead of its clock.
//!   [`V10VerifyPolicy::python`] restores Python's behaviour for differential
//!   tests, and then needs `allow_unbounded_nonce_table`.
//! - The nonce table is bounded: entries expire once no replay could pass the
//!   window, a full table answers 503, and one hotkey may hold at most
//!   [`BoundedNonceTable::DEFAULT_KEYS_PER_HOTKEY`] live `hotkey:uuid` keys.
//! - Header prefixes match only at position zero, duplicate signed headers are
//!   refused and `bt_header_input_obj_*` headers are ignored rather than
//!   decoded.
//! - Forward handlers receive the verified header identity, and the body's
//!   `dendrite`/`axon` objects are replaced before forwarding, so a handler
//!   cannot read a spoofed hotkey and a request cannot choose its response
//!   status.
//! - The axon does not sign attacker-chosen text before verification
//!   (`AxonSignature::Omit` by default).
//! - The default verification always runs; the verify hook only adds checks.
//! - Request bodies are limited (16 MiB by default, 413 above). Python's
//!   bare `NaN`/`Infinity` tokens are accepted in request bodies, as Python
//!   axons accept them, but the Rust dendrite refuses to send non-finite
//!   floats and the axon writes them as `null` in responses (Python's axon
//!   answers 500).
//! - Unknown synapse names answer 404 before any check, as in Python. Extra
//!   path segments and other methods answer FastAPI's 404/405 after
//!   blacklist and verification, as in Python; a trailing slash answers 404
//!   rather than Starlette's redirect. `Axon::routes` (without the
//!   fallback) does not see percent-encoded names.
//! - The dendrite ignores proxy environment variables, as aiohttp does, and
//!   limits response bodies (64 MiB by default).
//! - Tensor decoding refuses a buffer whose type differs from `dtype` (Python
//!   casts) or whose length differs from its shape (numpy accepts a longer
//!   buffer).
//! - IPv6 targets are bracketed; the dendrite maps transport errors by
//!   category rather than by exact exception type.
//!
//! Python dicts preserve insertion order and the body hash renders them in
//! that order. Use [`PyDict`] (or another order-preserving map) for hashed
//! `dict` fields; `HashMap`, `BTreeMap` and `serde_json::Value` objects do not
//! round-trip Python's order.

#![doc = include_str!("../README.md")]
#![cfg_attr(docsrs, feature(doc_cfg))]

#[cfg(feature = "axon")]
#[cfg_attr(docsrs, doc(cfg(feature = "axon")))]
pub mod axon;
mod codec;
#[cfg(feature = "dendrite")]
#[cfg_attr(docsrs, doc(cfg(feature = "dendrite")))]
pub mod dendrite;
mod json;
mod pyjson;
mod pyvalue;
mod ss58;
#[cfg(feature = "tensor")]
#[cfg_attr(docsrs, doc(cfg(feature = "tensor")))]
pub mod tensor;
mod uuid;
mod verify;

#[cfg(feature = "axon")]
pub use axon::{
    Axon, AxonConfigError, AxonSignature, DefaultHooks, Hooks, PriorityError, Reply, SynapseError,
    V10AxonConfig,
};
pub use codec::{
    BaseSynapse, CodecError, DummyKind, Envelope, RequestHead, Synapse, TerminalInfo,
    default_hash_values, is_valid_synapse_name, parse_head,
};
#[cfg(feature = "dendrite")]
pub use dendrite::{CallOptions, Dendrite, DendriteError, PreparedRequest, UuidMode};
pub use json::{to_python_json, to_starlette_json};
pub use pyvalue::{PyDict, PyValue, PyValueError, get_hash, py_repr, py_str, to_py_value};
pub use ss58::{Ss58Error, decode_ss58_any_prefix};
pub use uuid::{uuid1_string, uuid4_string};
pub use verify::{
    BoundedNonceTable, UnboundedNonceTable, V10NonceError, V10NonceTable, V10VerifyPolicy,
    VerifiedHead, VerifyRejection, default_verify, v10_message,
};

/// `bittensor.__version_as_int__` for 10.5.0: `10 * 1000^2 + 5 * 1000 + 0`.
pub const V10_VERSION_AS_INT: i64 = 10_005_000;
/// Version from which Python applies the nonce freshness window (7.2.0).
pub const V_7_2_0: i64 = 7_002_000;
/// Python's `ALLOWED_DELTA`: four seconds, in nanoseconds.
pub const ALLOWED_DELTA_NS: u64 = 4_000_000_000;
/// Body hash of a synapse with no hash fields: SHA3-256 of the empty string.
pub const EMPTY_BODY_HASH: &str =
    "a7ffc6f8bf1ed76651c14756a061d662f580ff4de43b49fa82d80a4b80f8434a";
