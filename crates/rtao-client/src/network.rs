// Derived from RaoFoundation/subtensor sdk/python/bittensor/settings.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Network labels and endpoint pools derived from Python settings.

use std::{error::Error, fmt};

use rtao_catalog::py;
use rtao_primitives::H256;
use rtao_rpc::{EndpointPool, RateLimit};
use url::Url;

use crate::generated::network_pins;

/// Primary endpoints that deliberately differ from Python's presets (D-078).
///
/// Python's preset URL keeps resolving to the same label and stays in the
/// failover pool behind the rtao primary.
const PRIMARY_OVERRIDES: &[(&str, &str)] = &[("test", "wss://test.chain.opentensor.ai:443")];

/// Everyday names accepted beside Python's preset labels (D-082).
const ALIASES: &[(&str, &str)] = &[("mainnet", "finney"), ("testnet", "test")];

/// The preset label an alias stands for (`mainnet` is `finney`), or the input.
pub fn canonical_name(input: &str) -> &str {
    ALIASES
        .iter()
        .find(|(alias, _)| *alias == input)
        .map_or(input, |(_, name)| name)
}

/// A named network or explicit WebSocket endpoint.
///
/// Known endpoint strings retain Python's corresponding network label. An
/// explicit URL that differs from the preset is a custom unpinned endpoint.
#[derive(Clone, PartialEq, Eq)]
pub struct Network {
    label: String,
    endpoint: String,
}

impl Network {
    /// Resolve against the current process environment.
    pub fn resolve(input: &str) -> Result<Self, NetworkError> {
        Self::resolve_with(input, |name| std::env::var(name).ok())
    }

    /// Resolve against an injected environment reader.
    pub fn resolve_with(
        input: &str,
        mut read: impl FnMut(&str) -> Option<String>,
    ) -> Result<Self, NetworkError> {
        let input = canonical_name(input);
        let presets = py::NETWORKS
            .iter()
            .map(|(name, endpoint)| (*name, endpoint.resolve(&mut read)))
            .collect::<Vec<_>>();
        let (label, endpoint) = if input.starts_with("ws://") || input.starts_with("wss://") {
            (
                presets
                    .iter()
                    .map(|(name, endpoint)| (*name, endpoint.as_str()))
                    .chain(PRIMARY_OVERRIDES.iter().copied())
                    .find(|(_, endpoint)| *endpoint == input)
                    .map(|(name, _)| name.to_owned())
                    .unwrap_or_else(|| input.to_owned()),
                input.to_owned(),
            )
        } else {
            let endpoint = presets
                .iter()
                .find(|(name, _)| *name == input)
                .map(|(name, endpoint)| {
                    primary_override(name).map_or_else(|| endpoint.clone(), str::to_owned)
                })
                .ok_or_else(|| NetworkError::Unknown(input.to_owned()))?;
            (input.to_owned(), endpoint)
        };
        endpoint_url(&endpoint)?;
        Ok(Self { label, endpoint })
    }

    /// Resolve the extracted default against an injected environment.
    pub fn default_with(read: impl FnMut(&str) -> Option<String>) -> Result<Self, NetworkError> {
        Self::resolve_with(py::DEFAULT_NETWORK, read)
    }

    /// Python's label, or the original explicit endpoint for a custom network.
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Primary endpoint exactly as configured.
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Primary and extracted default failover/archive endpoints.
    ///
    /// A custom URL is pinned to that single endpoint. Explicit pool overrides
    /// belong in ClientConfig and replace these defaults.
    pub fn endpoint_pool(&self) -> Result<EndpointPool, NetworkError> {
        let mut urls = vec![endpoint_url(&self.endpoint)?];
        let mut fallbacks = Vec::new();
        if let Some(primary) = primary_override(&self.label) {
            fallbacks.push(primary);
            fallbacks.extend(
                py::NETWORKS
                    .iter()
                    .filter(|(name, _)| *name == self.label)
                    .filter_map(|(_, endpoint)| endpoint.env.is_none().then_some(endpoint.default)),
            );
        }
        if let Some((_, pool)) = py::FALLBACK_ENDPOINTS
            .iter()
            .find(|(name, _)| *name == self.label)
        {
            fallbacks.extend(pool.iter().copied());
        }
        for fallback in fallbacks {
            let url = endpoint_url(fallback)?;
            if !urls.contains(&url) {
                urls.push(url);
            }
        }
        let archive = py::ARCHIVE_ENDPOINTS
            .iter()
            .find(|(name, _)| *name == self.label)
            .map(|(_, endpoints)| {
                endpoints
                    .iter()
                    .map(|endpoint| endpoint_url(endpoint))
                    .collect()
            })
            .transpose()?
            .unwrap_or_default();
        Ok(EndpointPool { urls, archive })
    }

    /// Immutable identity for a public named network; custom/local are unpinned.
    pub fn genesis(&self) -> Result<Option<H256>, NetworkError> {
        let value = match self.label.as_str() {
            "finney" | "archive" => network_pins::FINNEY_GENESIS,
            "test" => network_pins::TEST_GENESIS,
            "devnet" => network_pins::DEVNET_GENESIS,
            _ => return Ok(None),
        };
        let bytes = hex::decode(
            value
                .strip_prefix("0x")
                .ok_or(NetworkError::InvalidPinnedGenesis)?,
        )
        .map_err(|_| NetworkError::InvalidPinnedGenesis)?;
        let bytes: [u8; 32] = bytes
            .try_into()
            .map_err(|_| NetworkError::InvalidPinnedGenesis)?;
        Ok(Some(H256(bytes)))
    }

    /// Default pacing for public presets, with local/custom endpoints unpaced.
    pub fn rate_limit(&self) -> Option<RateLimit> {
        matches!(
            self.label.as_str(),
            "finney" | "test" | "devnet" | "archive"
        )
        .then_some(RateLimit::PUBLIC)
    }
}

impl fmt::Debug for Network {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Network")
            .field("endpoint", &rtao_rpc::policy::safe_endpoint(&self.endpoint))
            .finish()
    }
}

/// Invalid network configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum NetworkError {
    /// An input is neither an extracted label nor a WebSocket URL.
    Unknown(String),
    /// The endpoint is not a valid ws/wss URL with a hostname.
    InvalidEndpoint,
    /// A generated immutable identity is not a valid 32-byte hash.
    InvalidPinnedGenesis,
}

impl fmt::Display for NetworkError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown(input) => {
                let mut names = py::NETWORKS
                    .iter()
                    .map(|(name, _)| *name)
                    .collect::<Vec<_>>();
                names.sort_unstable();
                let known = names
                    .iter()
                    .map(|name| format!("'{name}'"))
                    .collect::<Vec<_>>()
                    .join(", ");
                write!(
                    formatter,
                    "Unknown network {}. Known networks: [{known}], or pass a ws:// / wss:// endpoint directly.",
                    rtao_rpc::policy::python_string_repr(input)
                )
            }
            Self::InvalidEndpoint => {
                formatter.write_str("configured RPC endpoint is not a valid ws:// or wss:// URL")
            }
            Self::InvalidPinnedGenesis => {
                formatter.write_str("generated network genesis is invalid")
            }
        }
    }
}

impl Error for NetworkError {}

fn primary_override(label: &str) -> Option<&'static str> {
    PRIMARY_OVERRIDES
        .iter()
        .find(|(name, _)| *name == label)
        .map(|(_, endpoint)| *endpoint)
}

fn endpoint_url(input: &str) -> Result<Url, NetworkError> {
    let url = Url::parse(input).map_err(|_| NetworkError::InvalidEndpoint)?;
    if !matches!(url.scheme(), "ws" | "wss") || url.host_str().is_none() {
        return Err(NetworkError::InvalidEndpoint);
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    // pyt: settings.py::resolve_endpoint, default_fallback_endpoints,
    // default_archive_endpoints (no standalone upstream unit tests).
    #[test]
    fn every_extracted_label_and_exact_url_resolves_to_the_same_network() -> Result<(), NetworkError>
    {
        for (name, endpoint) in py::NETWORKS {
            let expected = endpoint.resolve(|_| None);
            let named = Network::resolve_with(name, |_| None)?;
            let direct = Network::resolve_with(&expected, |_| None)?;
            assert_eq!(named.label(), direct.label());
            assert_eq!(direct.endpoint(), expected);
            assert_eq!(
                named.endpoint(),
                primary_override(name).unwrap_or(&expected)
            );
            assert_eq!(named.label(), *name);
            assert_eq!(named.genesis()?.is_some(), *name != "local");
        }
        Ok(())
    }

    #[test]
    fn everyday_aliases_resolve_to_their_presets() -> Result<(), NetworkError> {
        assert_eq!(
            Network::resolve_with("mainnet", |_| None)?,
            Network::resolve_with("finney", |_| None)?
        );
        let testnet = Network::resolve_with("testnet", |_| None)?;
        assert_eq!(testnet.label(), "test");
        assert_eq!(testnet, Network::resolve_with("test", |_| None)?);
        assert_eq!(canonical_name("devnet"), "devnet");
        assert!(Network::resolve_with("Mainnet", |_| None).is_err());
        Ok(())
    }

    #[test]
    fn testnet_prefers_the_chain_endpoint_and_keeps_python_preset_as_fallback()
    -> Result<(), NetworkError> {
        let test = Network::resolve_with("test", |_| None)?;
        assert_eq!(test.endpoint(), "wss://test.chain.opentensor.ai:443");
        let urls = test.endpoint_pool()?.urls;
        assert_eq!(
            urls,
            [
                endpoint_url("wss://test.chain.opentensor.ai:443")?,
                endpoint_url("wss://test.finney.opentensor.ai:443")?,
            ]
        );
        let legacy = Network::resolve_with("wss://test.finney.opentensor.ai:443", |_| None)?;
        assert_eq!(legacy.label(), "test");
        assert_eq!(
            legacy.endpoint_pool()?.urls,
            [urls[1].clone(), urls[0].clone()]
        );
        assert_eq!(legacy.genesis()?, test.genesis()?);
        let direct = Network::resolve_with("wss://test.chain.opentensor.ai:443", |_| None)?;
        assert_eq!(direct, test);
        Ok(())
    }

    #[test]
    fn raw_endpoints_do_not_inherit_a_named_identity_or_failover_pool() -> Result<(), NetworkError>
    {
        let custom = Network::resolve_with("wss://private.invalid:9944/rpc", |_| None)?;
        let pool = custom.endpoint_pool()?;
        assert_eq!(pool.urls.len(), 1);
        assert!(pool.archive.is_empty());
        assert!(custom.genesis()?.is_none());
        assert!(custom.rate_limit().is_none());
        Ok(())
    }

    #[test]
    fn local_environment_and_empty_override_match_extracted_contract() -> Result<(), NetworkError> {
        let overridden = Network::resolve_with("local", |_| Some("ws://127.0.0.1:12345".into()))?;
        assert_eq!(overridden.endpoint(), "ws://127.0.0.1:12345");
        let default = Network::resolve_with("local", |_| Some(String::new()))?;
        assert_eq!(default, Network::resolve_with("local", |_| None)?);
        assert!(overridden.genesis()?.is_none());
        Ok(())
    }

    #[test]
    fn public_pools_preserve_extracted_fallback_and_archive_order() -> Result<(), NetworkError> {
        let finney = Network::resolve_with("finney", |_| None)?;
        let pool = finney.endpoint_pool()?;
        let expected = py::FALLBACK_ENDPOINTS
            .iter()
            .find(|(name, _)| *name == "finney");
        assert_eq!(
            pool.urls.len(),
            1 + expected.map_or(0, |(_, values)| values.len())
        );
        if let Some((_, endpoints)) = expected {
            for (actual, expected) in pool.urls.iter().skip(1).zip(*endpoints) {
                assert_eq!(*actual, endpoint_url(expected)?);
            }
        }
        assert_eq!(
            Network::resolve_with("archive", |_| None)?.genesis()?,
            finney.genesis()?
        );
        assert!(finney.rate_limit().is_some());
        Ok(())
    }

    #[test]
    fn unknown_labels_case_and_secret_endpoint_debug_are_checked() -> Result<(), NetworkError> {
        let error = Network::resolve_with("FINNEY", |_| None)
            .err()
            .ok_or(NetworkError::InvalidEndpoint)?;
        assert!(error.to_string().contains("Unknown network"));
        let secret = Network::resolve_with(
            "wss://alice:password@private.invalid/rpc?token=secret",
            |_| None,
        )?;
        let debug = format!("{secret:?}");
        assert!(
            !debug.contains("password") && !debug.contains("secret") && !debug.contains("alice")
        );
        assert!(Network::resolve_with("ws://", |_| None).is_err());
        Ok(())
    }
}
