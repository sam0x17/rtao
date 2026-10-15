// Derived from RaoFoundation/subtensor sdk/python/bittensor/timelock.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Bounded Quicknet beacon HTTP, with an explicit ring TLS provider.

use std::{sync::Arc, time::Duration};

use reqwest::{Url, redirect::Policy};
use rtao_core::timelock::constants::{DRAND_ENDPOINTS, QUICKNET_CHAIN_HASH};

use super::TimelockError;

const MAX_ENDPOINTS: usize = 32;
const MAX_RESPONSE_BYTES: usize = 64 * 1024;

/// Bounded HTTP settings for the Quicknet signature client.
#[derive(Clone, Debug)]
pub struct DrandConfig {
    /// Beacon base URLs, tried in order; defaults to the core's five relays.
    pub endpoints: Vec<String>,
    /// Total budget for one fetch, including all endpoint attempts and body.
    ///
    /// Each relay receives an equal share, so a stalled earlier relay cannot
    /// consume the entire budget and prevent failover to later endpoints.
    pub request_timeout: Duration,
    /// Maximum response bytes (1 through 65536); defaults to 16384.
    pub max_response_bytes: usize,
}

impl Default for DrandConfig {
    fn default() -> Self {
        Self {
            endpoints: DRAND_ENDPOINTS.iter().map(|url| (*url).into()).collect(),
            request_timeout: Duration::from_secs(5),
            max_response_bytes: 16 * 1024,
        }
    }
}

/// A validated Quicknet response for exactly the requested round.
#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize)]
pub struct DrandRound {
    /// Quicknet round returned by the relay.
    pub round: u64,
    /// Compressed 48-byte BLS signature as hex; crypto verifies it on opening.
    pub signature: String,
}

/// Cloneable asynchronous Quicknet relay client.
///
/// Client construction never installs or reads the process TLS provider.
/// Redirects are disabled; requests and response buffering have explicit
/// bounds. Plain HTTP is accepted only when explicitly supplied in config.
#[derive(Clone)]
pub struct DrandClient {
    http: reqwest::Client,
    endpoints: Vec<Url>,
    config: DrandConfig,
    pub(super) clock: Arc<dyn crate::Clock>,
}

impl DrandClient {
    /// Build using the core's endpoints, native trust roots and explicit ring.
    pub fn new() -> Result<Self, TimelockError> {
        Self::with_config(DrandConfig::default())
    }

    /// Build with endpoint and request bounds, retaining native trust roots.
    pub fn with_config(config: DrandConfig) -> Result<Self, TimelockError> {
        Self::with_extra_roots(config, Vec::new())
    }

    /// Add explicit trust roots, including loopback fixture certificates.
    pub fn with_extra_roots(
        config: DrandConfig,
        extra_roots: Vec<rustls_pki_types::CertificateDer<'static>>,
    ) -> Result<Self, TimelockError> {
        let mut roots = rustls::RootCertStore::empty();
        let native = rustls_native_certs::load_native_certs();
        let (added, _) = roots.add_parsable_certificates(native.certs);
        if added == 0 && extra_roots.is_empty() {
            return Err(TimelockError::Configuration(
                "no usable native TLS roots; configure an explicit trust root".into(),
            ));
        }
        for root in extra_roots {
            roots.add(root).map_err(|error| {
                TimelockError::Configuration(format!("invalid TLS trust root: {error}"))
            })?;
        }
        let tls = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .map_err(|error| TimelockError::Configuration(error.to_string()))?
        .with_root_certificates(roots)
        .with_no_client_auth();
        Self::with_tls_config(config, tls)
    }

    /// Build from an owned TLS config with an explicitly selected provider.
    ///
    /// ALPN is set to h2 and HTTP/1.1. The owned value, rather than an Arc, is
    /// passed to reqwest's preconfigured backend.
    pub fn with_tls_config(
        config: DrandConfig,
        mut tls: rustls::ClientConfig,
    ) -> Result<Self, TimelockError> {
        if config.endpoints.is_empty() || config.endpoints.len() > MAX_ENDPOINTS {
            return Err(TimelockError::Configuration(format!(
                "configure between 1 and {MAX_ENDPOINTS} drand endpoints"
            )));
        }
        if config.request_timeout.is_zero()
            || config.max_response_bytes == 0
            || config.max_response_bytes > MAX_RESPONSE_BYTES
        {
            return Err(TimelockError::Configuration(
                "drand requires a positive timeout and response bound of 1..=65536 bytes".into(),
            ));
        }
        let mut endpoints = Vec::with_capacity(config.endpoints.len());
        for endpoint in &config.endpoints {
            let url = Url::parse(endpoint)
                .map_err(|_| TimelockError::Configuration("invalid drand endpoint URL".into()))?;
            if !matches!(url.scheme(), "https" | "http")
                || url.host_str().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
            {
                return Err(TimelockError::Configuration(
                    "drand endpoints require HTTP(S), a host, and no credentials, query or fragment"
                        .into(),
                ));
            }
            endpoints.push(url);
        }
        tls.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
        let http = reqwest::Client::builder()
            .tls_backend_preconfigured(tls)
            .redirect(Policy::none())
            .connect_timeout(config.request_timeout)
            .timeout(config.request_timeout)
            .build()
            .map_err(|error| TimelockError::Configuration(error.to_string()))?;
        Ok(Self {
            http,
            endpoints,
            config,
            clock: Arc::new(crate::SystemClock),
        })
    }

    /// Inject the wall clock used by [`super::open`].
    ///
    /// Deadlines remain monotonic and independent of wall-clock changes.
    pub fn with_clock(mut self, clock: Arc<dyn crate::Clock>) -> Self {
        self.clock = clock;
        self
    }

    /// Configured request bounds and endpoint spelling.
    pub fn config(&self) -> &DrandConfig {
        &self.config
    }

    /// Fetch a signature for exactly one round, with bounded relay failover.
    ///
    /// A response for another round, a malformed/oversized body, or an invalid
    /// signature encoding is rejected before cryptography. Signature group
    /// validation and authenticated payload decryption stay in the core.
    pub async fn fetch_round(&self, round: u64) -> Result<DrandRound, TimelockError> {
        tokio::time::timeout(self.config.request_timeout, self.fetch_round_inner(round))
            .await
            .map_err(|_| TimelockError::Beacon("drand request deadline exceeded".into()))?
    }

    async fn fetch_round_inner(&self, round: u64) -> Result<DrandRound, TimelockError> {
        let mut last_error = String::from("no drand endpoints available");
        for endpoint in &self.endpoints {
            let url = format!(
                "{}/{QUICKNET_CHAIN_HASH}/public/{round}",
                endpoint.as_str().trim_end_matches('/')
            );
            match self.fetch_from(&url, round).await {
                Ok(response) => return Ok(response),
                Err(error) => last_error = error.to_string(),
            }
        }
        Err(TimelockError::Beacon(last_error))
    }

    async fn fetch_from(&self, url: &str, round: u64) -> Result<DrandRound, TimelockError> {
        let mut response = self
            .http
            .get(url)
            .timeout(self.config.request_timeout / self.endpoints.len() as u32)
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(|error| TimelockError::Beacon(error.to_string()))?;
        if !response.status().is_success() {
            return Err(TimelockError::Beacon(format!(
                "drand returned HTTP {}",
                response.status()
            )));
        }
        let limit = self.config.max_response_bytes;
        if response
            .content_length()
            .is_some_and(|len| len > limit as u64)
        {
            return Err(TimelockError::Beacon(
                "drand response exceeds the configured byte limit".into(),
            ));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| TimelockError::Beacon(error.to_string()))?
        {
            if chunk.len() > limit.saturating_sub(bytes.len()) {
                return Err(TimelockError::Beacon(
                    "drand response exceeds the configured byte limit".into(),
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        let response: DrandRound = rtao_primitives::json::from_slice(&bytes)
            .map_err(|error| TimelockError::Beacon(format!("invalid drand response: {error}")))?;
        if response.round != round {
            return Err(TimelockError::Beacon(format!(
                "drand returned round {}, requested {round}",
                response.round
            )));
        }
        if response.signature.len() != 96 || hex::decode(&response.signature).is_err() {
            return Err(TimelockError::Beacon(
                "drand signature must be 48 bytes of hex".into(),
            ));
        }
        Ok(response)
    }
}
