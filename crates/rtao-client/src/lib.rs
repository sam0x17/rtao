//! Asynchronous Bittensor chain access.

#![deny(missing_docs)]

mod block_info;
/// Private runtime thread used by the facade's synchronous adapters.
#[cfg(feature = "blocking")]
pub mod blocking;
/// Runtime metadata resolution, coalescing, and Python-compatible disk cache.
pub mod cache;
mod client;
/// Compiled release compatibility evidence and its read-only runtime guard.
pub mod compat;
/// Runtime-checked origin wrappers for composed calls.
pub mod compose;
/// Shared btcli configuration paths and atomic caches.
pub mod config;
mod decode;
mod error;
/// Parent-runtime event records and lazy typed event decoding.
pub mod events;
/// Command-generated network inputs.
pub mod generated;
/// Best and finalized head followers with gap recovery.
pub mod head;
mod legacy_api;
/// Network presets and immutable public-network identities.
pub mod network;
mod nonce;
/// Public-identity preparation and authenticated Python unsigned-JSON import.
pub mod offline;
/// Metadata-directed decoding for typed primitive and dynamic read outputs.
pub mod read_decode;
mod rpc;
/// Bounded MEV shield envelopes and the paired-nonce submission pipeline.
pub mod shield;
/// Transaction preparation, nonce leases, submission, and inclusion receipts.
pub mod submit;
/// Typed and dynamic read views pinned to immutable chain state.
pub mod view;

pub use block_info::BlockInfo;
pub use cache::RuntimeCache;
pub use client::{Client, ClientConfig};
pub use error::ClientError;
pub use events::{Event, EventRecord, Events, Phase, RawEvent};
pub use head::{
    BlockHeader, BlockStream, Finality, HeadError, HeadEvent, HeadProvider, HeadTracker, Reorg,
};
pub use network::{Network, NetworkError};
pub use nonce::{NonceLease, NonceManager};
pub use offline::{PublicPrepareOptions, UntrustedUnsignedExtrinsic};
pub use read_decode::{ReadValue, decode_typed};
pub(crate) use rpc::{hash_hex, parse_hash, rpc_request};
/// Run blocking chain-code work on Tokio's blocking pool.
pub use rtao_core::offload;
pub use rtao_runtime::{CallBytes, MetadataLevel, RuntimeHandle, StorageQuery};
pub use shield::{ShieldCiphertext, ShieldOptions, ShieldPeriod, ShieldReport};
pub use submit::{
    ExtrinsicReport, Inclusion, Mortality, NoncePolicy, Period, PoolRejection, SubmitFailure,
    SubmitOptions, UnsignedExtrinsic, Wait,
};
pub use view::{
    BlockRef, DynView, Fetch, FromStorage, HeadPolicy, IntoCoreValues, KeyArgs, PinnedBlock,
    ReadKind, ReadObserver, ReadPlan, ReadSpec, ReadTouch, Snapshot, View, ViewExt,
};
