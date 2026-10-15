//! Generated tool dispatch attached to the lower-layer transaction executor.

use std::sync::Arc;

use crate::Registry;
use rtao_client::Client;

/// Construct an executor with the feature-enabled catalog registry attached.
///
/// Typed execution and JSON tools share this client's nonce manager. The
/// parser lives in the intents layer, keeping the transaction layer independent
/// of generated intent bodies.
pub fn executor(client: Client) -> rtao_tx::Executor {
    rtao_tx::Executor::new(client).with_registry(Arc::new(Registry))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use rtao_tx::IntentRegistry;

    #[cfg(feature = "weights")]
    #[test]
    fn owned_tools_keep_constructor_normalization_and_role() {
        let intent = Registry
            .from_tool(
                "set_weights",
                serde_json::json!({
                    "op":"ignored", "netuid":1, "weights":{"2":1.0,"1":2.0}
                }),
            )
            .unwrap();
        assert_eq!(intent.spec().name, "set_weights");
        assert_eq!(intent.spec().signer, "hotkey");
        let args = intent.to_json_dyn();
        assert_eq!(args["uids"], serde_json::json!([1, 2]));
        assert_eq!(args["weights"], serde_json::json!([2.0, 1.0]));
        assert_eq!(intent.wraps().len(), 2);
        assert!(
            Registry
                .from_tool(
                    "set_weights",
                    serde_json::json!({
                        "netuid":1, "uids":[1], "weights":{"1":1.0}
                    })
                )
                .is_err()
        );
    }

    #[test]
    fn unknown_tools_fail_before_any_client_or_key_is_available() {
        assert!(Registry.from_tool("absent", serde_json::json!({})).is_err());
    }
}
