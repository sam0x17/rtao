// Derived from RaoFoundation/subtensor sdk/python/bittensor/intents/registry.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Generated tool dispatch attached to the lower-layer transaction executor.

use std::sync::Arc;

use crate::Registry;
use rtao_client::Client;

pub(crate) fn unknown_op(op: &str) -> rtao_tx::IntentError {
    let mut known = crate::interface_metadata()
        .into_iter()
        .filter(|entry| entry.compiled && entry.implemented)
        .map(|entry| entry.name)
        .collect::<Vec<_>>();
    known.sort_unstable();
    let known = known
        .into_iter()
        .map(rtao_primitives::pyfmt::string_repr)
        .collect::<Vec<_>>()
        .join(", ");
    rtao_tx::IntentError::Validation(format!(
        "Unknown op {}. Known ops: [{known}]",
        rtao_primitives::pyfmt::string_repr(op)
    ))
}

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

    #[cfg(feature = "all")]
    #[test]
    fn unknown_tools_keep_python_quoted_input_and_complete_sorted_registry()
    -> Result<(), Box<dyn std::error::Error>> {
        // Python registry.build uses repr(op) and sorted(REGISTRY). Its
        // catalog supplies the complete independently authenticated op set.
        let mut known = rtao_catalog::INTENTS
            .iter()
            .map(|entry| entry.name)
            .collect::<Vec<_>>();
        known.sort_unstable();
        let list = known
            .into_iter()
            .map(|name| format!("'{name}'"))
            .collect::<Vec<_>>()
            .join(", ");
        for (op, quoted) in [("absent", "'absent'"), ("missing'\n", "\"missing'\\n\"")] {
            let expected = format!("Unknown op {quoted}. Known ops: [{list}]");
            for result in [
                crate::AnyIntent::from_tool(op, serde_json::json!({})).map(|_| ()),
                Registry.from_tool(op, serde_json::json!({})).map(|_| ()),
            ] {
                let Err(rtao_tx::IntentError::Validation(message)) = result else {
                    return Err("unknown operation did not return its validation diagnostic".into());
                };
                assert_eq!(message, expected);
            }
        }
        Ok(())
    }
}
