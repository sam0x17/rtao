//! Asynchronous Bittensor JSON-RPC transport.

#![doc = include_str!("../README.md")]
#![deny(missing_docs)]

/// Data generated from the pinned upstream Python transport.
pub mod generated {
    /// Session defaults and RPC policy classification constants.
    pub mod py_rpc;
}

/// Gateway refusal classification and bounded diagnostics.
pub mod policy;
mod session;
mod subscription;
mod types;

pub use policy::{PolicyRefusal, RefusalKind};
#[cfg(feature = "fuzzing")]
#[doc(hidden)]
pub use session::fuzz_json_rpc_frame;
pub use session::{ArchivePool, AuthorOwnership, AuthorRetirement, Session, tls_config};
pub use subscription::Subscription;
pub use types::*;

/// Classify a complete JSON-RPC response using the live session's decoder.
///
/// This entry point lets recording tools replay transport diagnostics without
/// creating a socket or duplicating the session's policy and pruning rules.
#[doc(hidden)]
pub fn classify_response(response: &serde_json::Value) -> Result<serde_json::Value, RpcError> {
    session::unwrap_response(response)
}

#[cfg(test)]
mod response_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn pruned_state_keeps_block_and_archive_guidance_before_policy() {
        let block = "0x123";
        let expected = format!(
            "State discarded for {block}. This indicates the block is too old, and you should instead make this request using an archive node."
        );
        for error in [
            json!({"code":-32004,"message":"Too many requests", "data":"State already discarded for 0x123"}),
            json!({"code":-32000,"message":"State already discarded for 0x123"}),
        ] {
            let result = classify_response(&json!({"jsonrpc":"2.0","id":1,"error":error}));
            assert!(matches!(&result, Err(RpcError::StateDiscarded { block }) if block == "0x123"));
            assert_eq!(
                result.err().map(|error| error.to_string()),
                Some(expected.clone())
            );
        }
        assert_eq!(
            classify_response(&json!({"id":1,"result":null})),
            Ok(serde_json::Value::Null)
        );
    }
}
