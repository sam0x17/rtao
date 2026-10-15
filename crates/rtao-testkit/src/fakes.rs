//! A socket node and a direct RPC client over the same encoded chain state.
//!
//! The node exercises transport behavior with real loopback sockets. The direct
//! client bypasses sockets while preserving metadata, key building and SCALE
//! decoding in callers.

mod chain;
mod node;
mod state;

pub use chain::FakeChain;
pub use node::{ConnectionRecord, FakeError, FakeNode, Fault, FaultPlan};
pub use state::{
    FakeState, Notification, Reply, RpcFailure, RpcRequest, SubmitScript, SubscriptionScript,
    fill_defaults, json_to_core,
};
