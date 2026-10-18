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
        "wss://test.chain.opentensor.ai:443" | "wss://test.finney.opentensor.ai:443" => "test",
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
        other if other.starts_with("wss://") || other.starts_with("ws://") => {
            let http = other
                .replacen("wss://", "https://", 1)
                .replacen("ws://", "http://", 1);
            EvmNetwork::endpoint(network, None, &http)
        }
        _ => {
            return Err(Error::Validation(format!(
                "Unknown network {} for EVM access. Known: ['finney', 'local', 'test'], or pass an http(s):// EVM RPC URL.",
                rtao_primitives::pyfmt::string_repr(network)
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

    fn extracted_preset_table() -> serde_json::Value {
        let extracted: serde_json::Value = serde_json::from_str(include_str!(
            "../../../vendor/upstream/extracted/named_tables.json"
        ))
        .unwrap();
        extracted["tables"]["sdk/python/bittensor/evm/networks.py"]["values"]["EVM_NETWORKS"]
            ["value"]
            .clone()
    }

    fn extracted_presets(
        table: &serde_json::Value,
        environment: impl Fn(&str) -> Option<String>,
    ) -> BTreeMap<String, EvmNetwork> {
        let mut networks = BTreeMap::new();
        for pair in table["dict"].as_array().unwrap() {
            let pair = pair.as_array().unwrap();
            assert_eq!(pair.len(), 2);
            let name = pair[0].as_str().unwrap();
            assert_eq!(pair[1]["constructor"], "EvmNetwork");
            let fields = pair[1]["fields"].as_object().unwrap();
            assert_eq!(fields.len(), 5);
            let chain_id = match fields.get("chain_id").unwrap() {
                serde_json::Value::Null => None,
                value => Some(value.as_u64().unwrap()),
            };
            let endpoint = fields.get("rpc_url").unwrap();
            let rpc_url = if let Some(url) = endpoint.as_str() {
                url.to_owned()
            } else {
                let descriptor = endpoint.as_object().unwrap();
                assert_eq!(descriptor.len(), 3);
                let empty_is_unset = descriptor["empty_is_unset"].as_bool().unwrap();
                environment(descriptor["env"].as_str().unwrap())
                    .filter(|value| !empty_is_unset || !value.is_empty())
                    .unwrap_or_else(|| descriptor["default"].as_str().unwrap().to_owned())
            };
            let network = EvmNetwork {
                name: fields["name"].as_str().unwrap().to_owned(),
                chain_id,
                rpc_url,
                currency_symbol: fields["currency_symbol"].as_str().unwrap().to_owned(),
                evm_decimals: fields["evm_decimals"].as_u64().unwrap().try_into().unwrap(),
            };
            assert!(networks.insert(name.to_owned(), network).is_none());
        }
        networks
    }

    fn preset_consumers_match(table: &serde_json::Value) -> bool {
        let exported: BTreeMap<_, _> = evm_networks()
            .into_iter()
            .map(|(name, network)| (name.to_owned(), network))
            .collect();
        if exported != extracted_presets(table, |key| std::env::var(key).ok()) {
            return false;
        }
        [None, Some(""), Some("https://local.example:1444/custom")]
            .into_iter()
            .all(|value| {
                let environment = |key: &str| {
                    if key == "BT_EVM_ENDPOINT" {
                        value.map(str::to_owned)
                    } else {
                        None
                    }
                };
                let resolved: BTreeMap<_, _> = exported
                    .keys()
                    .map(|name| {
                        (
                            name.clone(),
                            evm_network_with_env(name, environment).unwrap(),
                        )
                    })
                    .collect();
                extracted_presets(table, environment) == resolved
            })
    }

    #[test]
    fn preset_consumers_match_the_complete_extracted_python_table() {
        assert!(preset_consumers_match(&extracted_preset_table()));
    }

    #[test]
    fn websocket_aliases_resolve_to_their_extracted_python_presets() {
        let extracted: serde_json::Value = serde_json::from_str(include_str!(
            "../../../vendor/upstream/extracted/named_tables.json"
        ))
        .unwrap();
        let aliases = extracted["tables"]["sdk/python/bittensor/evm/networks.py"]["values"]
            ["_WS_TO_LABEL"]["value"]["dict"]
            .as_array()
            .unwrap();
        assert_eq!(aliases.len(), 4);
        for value in [None, Some(""), Some("https://local.example:1444/custom")] {
            let environment = |key: &str| {
                assert_eq!(key, "BT_EVM_ENDPOINT");
                value.map(str::to_owned)
            };
            let expected = extracted_presets(&extracted_preset_table(), environment);
            for pair in aliases {
                let pair = pair.as_array().unwrap();
                assert_eq!(pair.len(), 2);
                let endpoint = pair[0].as_str().unwrap();
                let label = pair[1].as_str().unwrap();
                assert_eq!(
                    evm_network_with_env(endpoint, environment).unwrap(),
                    expected[label],
                    "{endpoint} -> {label} with {value:?}"
                );
            }
        }
    }

    #[test]
    fn preset_consumer_guard_detects_table_and_environment_drift() {
        use serde_json::json;

        let table = extracted_preset_table();
        assert!(preset_consumers_match(&table));
        for (pointer, replacement) in [
            ("/dict/0/1/fields/name", json!("other")),
            ("/dict/0/1/fields/chain_id", json!(965)),
            ("/dict/0/1/fields/rpc_url", json!("https://changed.example")),
            ("/dict/0/1/fields/currency_symbol", json!("OTHER")),
            ("/dict/0/1/fields/evm_decimals", json!(9)),
            ("/dict/2/1/fields/chain_id", json!(42)),
            ("/dict/2/1/fields/rpc_url/env", json!("OTHER_ENDPOINT")),
            (
                "/dict/2/1/fields/rpc_url/default",
                json!("http://localhost:9945"),
            ),
            ("/dict/2/1/fields/rpc_url/empty_is_unset", json!(false)),
        ] {
            let mut mutant = table.clone();
            *mutant.pointer_mut(pointer).unwrap() = replacement;
            assert!(
                !preset_consumers_match(&mutant),
                "missed drift at {pointer}"
            );
        }
        let mut removed = table.clone();
        removed["dict"].as_array_mut().unwrap().remove(1);
        assert!(!preset_consumers_match(&removed));
        let mut added = table;
        let mut new_preset = added["dict"][0].clone();
        new_preset[0] = json!("new-preset");
        added["dict"].as_array_mut().unwrap().push(new_preset);
        assert!(!preset_consumers_match(&added));
    }

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
            (
                "wss://test.chain.opentensor.ai:443",
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
    fn exported_preset_map_has_exact_chain_facts_and_independent_values() {
        let current_local_url = std::env::var("BT_EVM_ENDPOINT")
            .ok()
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "http://127.0.0.1:9944".into());
        let mut networks = evm_networks();
        assert_eq!(
            networks.keys().copied().collect::<Vec<_>>(),
            ["finney", "local", "test"]
        );
        for (name, chain_id, url) in [
            ("finney", Some(964), "https://lite.chain.opentensor.ai"),
            ("test", Some(945), "https://test.chain.opentensor.ai"),
            ("local", None, current_local_url.as_str()),
        ] {
            let network = &networks[name];
            assert_eq!(network.name, name);
            assert_eq!(network.chain_id, chain_id);
            assert_eq!(network.rpc_url, url);
            assert_eq!(network.currency_symbol, "TAO");
            assert_eq!(network.evm_decimals, 18);
        }
        // A caller can change its returned map without modifying subsequent
        // calls, unlike Python's mutable module-level dictionary.
        networks.get_mut("finney").unwrap().rpc_url = "http://edited.invalid".into();
        networks.remove("test");
        let fresh = evm_networks();
        assert_eq!(fresh.len(), 3);
        assert_eq!(fresh["finney"].rpc_url, "https://lite.chain.opentensor.ai");
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

    #[test]
    fn websocket_conversion_applies_each_python_replacement_once() {
        for (input, expected) in [
            (
                "wss://example.test/ws://path/ws://tail",
                "https://example.test/http://path/ws://tail",
            ),
            (
                "ws://example.test/wss://path/wss://tail",
                "http://example.test/https://path/wss://tail",
            ),
            (
                "https://example.test/ws://path/wss://tail",
                "https://example.test/ws://path/wss://tail",
            ),
        ] {
            let network = evm_network_with_env(input, |_| None).unwrap();
            assert_eq!(network.name, input);
            assert_eq!(network.chain_id, None);
            assert_eq!(network.rpc_url, expected);
        }
    }

    #[test]
    fn unknown_network_diagnostic_retains_python_repr_and_sorted_labels() {
        for (input, expected_repr) in [
            ("", "''"),
            ("Finney", "'Finney'"),
            ("it's", "\"it's\""),
            ("line\nbreak", "'line\\nbreak'"),
            ("\u{a0}", "'\\xa0'"),
        ] {
            let error = evm_network_with_env(input, |_| None).unwrap_err();
            assert_eq!(
                error.to_string(),
                format!(
                    "Unknown network {expected_repr} for EVM access. Known: ['finney', 'local', 'test'], or pass an http(s):// EVM RPC URL."
                )
            );
        }
    }
}
