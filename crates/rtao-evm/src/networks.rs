// Derived from RaoFoundation/subtensor sdk/python/bittensor/evm/networks.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! EVM endpoint presets and Subtensor WebSocket endpoint resolution.

use std::collections::BTreeMap;

use crate::{Error, Result};

/// Connectivity facts for one EVM endpoint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EvmNetwork {
    /// Preset name, or the explicit endpoint supplied by the caller.
    pub name: String,
    /// Expected chain ID; local and explicit endpoints must discover it by RPC.
    pub chain_id: Option<u64>,
    /// HTTP(S) endpoint serving Ethereum JSON-RPC.
    pub rpc_url: String,
    /// Native currency display symbol.
    pub currency_symbol: String,
    /// Decimal places in EVM transaction values, rather than native rao.
    pub evm_decimals: u8,
}

impl EvmNetwork {
    fn endpoint(name: &str, chain_id: Option<u64>, rpc_url: &str) -> Self {
        Self {
            name: name.into(),
            chain_id,
            rpc_url: rpc_url.into(),
            currency_symbol: "TAO".into(),
            evm_decimals: 18,
        }
    }
}

/// Resolve the pinned Python presets, reading `BT_EVM_ENDPOINT` for localnet.
///
/// Empty environment values use `http://127.0.0.1:9944`. The environment is
/// resolved at the time of the call, so callers can configure it before use.
pub fn evm_network(network: &str) -> Result<EvmNetwork> {
    evm_network_with_env(network, |name| std::env::var(name).ok())
}

/// Resolve a preset or endpoint using an injected environment lookup.
///
/// Explicit HTTP(S) URLs retain an unknown chain ID, including URLs that
/// happen to match a preset. Known Subtensor WebSocket endpoints resolve to
/// their preset; other WebSocket URLs use HTTP(S) on the same host and port.
/// Endpoint syntax is validated when constructing the RPC client.
pub fn evm_network_with_env(
    network: &str,
    environment: impl FnOnce(&str) -> Option<String>,
) -> Result<EvmNetwork> {
    if network.starts_with("http://") || network.starts_with("https://") {
        return Ok(EvmNetwork::endpoint(network, None, network));
    }
    let label = match network {
        "wss://entrypoint-finney.opentensor.ai:443" | "wss://lite.chain.opentensor.ai:443" => {
            "finney"
        }
        "wss://test.finney.opentensor.ai:443" => "test",
        "ws://127.0.0.1:9944" => "local",
        other => other,
    };
    let result = match label {
        "finney" => EvmNetwork::endpoint("finney", Some(964), "https://lite.chain.opentensor.ai"),
        "test" => EvmNetwork::endpoint("test", Some(945), "https://test.chain.opentensor.ai"),
        "local" => {
            let local = environment("BT_EVM_ENDPOINT")
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "http://127.0.0.1:9944".into());
            EvmNetwork::endpoint("local", None, &local)
        }
        other if other.starts_with("wss://") => {
            EvmNetwork::endpoint(network, None, &format!("https://{}", &other[6..]))
        }
        other if other.starts_with("ws://") => {
            EvmNetwork::endpoint(network, None, &format!("http://{}", &other[5..]))
        }
        _ => {
            return Err(Error::Validation(format!(
                "unknown network {network:?} for EVM access; known: finney, local, test, or an HTTP(S) EVM RPC URL"
            )));
        }
    };
    Ok(result)
}

/// Return the three pinned presets, resolving the current localnet environment.
pub fn evm_networks() -> BTreeMap<&'static str, EvmNetwork> {
    let local = std::env::var("BT_EVM_ENDPOINT")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "http://127.0.0.1:9944".into());
    BTreeMap::from([
        (
            "finney",
            EvmNetwork::endpoint("finney", Some(964), "https://lite.chain.opentensor.ai"),
        ),
        (
            "test",
            EvmNetwork::endpoint("test", Some(945), "https://test.chain.opentensor.ai"),
        ),
        ("local", EvmNetwork::endpoint("local", None, &local)),
    ])
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn presets_and_known_substrate_endpoints_match_pinned_facts() {
        for (input, name, chain_id, url) in [
            (
                "finney",
                "finney",
                Some(964),
                "https://lite.chain.opentensor.ai",
            ),
            (
                "test",
                "test",
                Some(945),
                "https://test.chain.opentensor.ai",
            ),
            (
                "wss://entrypoint-finney.opentensor.ai:443",
                "finney",
                Some(964),
                "https://lite.chain.opentensor.ai",
            ),
            (
                "wss://lite.chain.opentensor.ai:443",
                "finney",
                Some(964),
                "https://lite.chain.opentensor.ai",
            ),
            (
                "wss://test.finney.opentensor.ai:443",
                "test",
                Some(945),
                "https://test.chain.opentensor.ai",
            ),
        ] {
            let resolved = evm_network_with_env(input, |_| None).unwrap();
            assert_eq!(resolved.name, name);
            assert_eq!(resolved.chain_id, chain_id);
            assert_eq!(resolved.rpc_url, url);
            assert_eq!(resolved.currency_symbol, "TAO");
            assert_eq!(resolved.evm_decimals, 18);
        }
    }

    #[test]
    fn local_environment_is_injectable_and_empty_means_unset() {
        for input in ["local", "ws://127.0.0.1:9944"] {
            for unset in [None, Some(String::new())] {
                let network = evm_network_with_env(input, |_| unset).unwrap();
                assert_eq!(network.rpc_url, "http://127.0.0.1:9944");
                assert_eq!(network.chain_id, None);
            }
            let network = evm_network_with_env(input, |key| {
                assert_eq!(key, "BT_EVM_ENDPOINT");
                Some("https://local.example:1444/custom".into())
            })
            .unwrap();
            assert_eq!(network.name, "local");
            assert_eq!(network.rpc_url, "https://local.example:1444/custom");
        }
    }

    #[test]
    fn direct_urls_and_unknown_websocket_urls_preserve_identity() {
        for (input, expected) in [
            (
                "https://lite.chain.opentensor.ai",
                "https://lite.chain.opentensor.ai",
            ),
            ("http://localhost:9944", "http://localhost:9944"),
            ("wss://example.test:443/rpc", "https://example.test:443/rpc"),
            ("ws://[::1]:9944", "http://[::1]:9944"),
        ] {
            let network = evm_network_with_env(input, |_| Some("unused".into())).unwrap();
            assert_eq!(network.name, input);
            assert_eq!(network.rpc_url, expected);
            assert_eq!(network.chain_id, None);
        }
        assert!(evm_network_with_env("Finney", |_| None).is_err());
        assert!(evm_network_with_env("unknown", |_| None).is_err());
    }
}
