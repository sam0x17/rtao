// Derived from RaoFoundation/subtensor pallets/subtensor/src/subnets/serving.rs @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Idempotent axon publication with chain rate limits and certificate rotation.

#[cfg(test)]
mod tests;

use rtao_client::{BlockRef, ViewExt};
use rtao_intents::{ResetAxon, ServeAxon, ServeAxonTls};
use rtao_primitives::{AccountId, H256, NetUid};
pub use rtao_runtime::models::NeuronCertificate;
use rtao_runtime::{models::AxonInfo, storage::subtensor_module as st};
use rtao_signer::{Hotkey, KeySource};
use rtao_tx::{ChainError, ExecuteOptions, Executor, ExtrinsicResult, TxError};
use std::{
    net::{IpAddr, Ipv4Addr},
    sync::Arc,
    time::Duration,
};
use url::Url;

/// Listener and public endpoint advertised for a miner.
#[derive(Clone, Debug)]
pub struct AxonConfig {
    /// Local HTTP listening port.
    pub port: u16,
    /// Operator-supplied public IP; absent enables IPv4 discovery.
    pub external_ip: Option<IpAddr>,
    /// Advertised port when forwarding differs from the listener.
    pub external_port: Option<u16>,
    /// Opaque protocol byte; btauth HTTP uses four.
    pub protocol: u8,
    /// Application version advertised to validators.
    pub version: u32,
    /// Algorithm byte and at most 64 public-key bytes published through TLS serving.
    pub certificate: Option<NeuronCertificate>,
    /// Plain-text IPv4 discovery URLs, each with a five-second deadline.
    pub ip_sources: Vec<Url>,
    /// Permit local/private addresses, intended for explicit localnet use.
    pub allow_non_global: bool,
    /// Period between endpoint discovery checks.
    pub recheck: Duration,
    /// Clear the chain endpoint during graceful shutdown.
    pub reset_on_shutdown: bool,
}
impl Default for AxonConfig {
    fn default() -> Self {
        Self {
            port: 8091,
            external_ip: None,
            external_port: None,
            protocol: 4,
            version: 1,
            certificate: None,
            ip_sources: ["https://api.ipify.org", "https://ipv4.icanhazip.com"]
                .into_iter()
                .filter_map(|value| Url::parse(value).ok())
                .collect(),
            allow_non_global: false,
            recheck: Duration::from_secs(300),
            reset_on_shutdown: false,
        }
    }
}
impl AxonConfig {
    /// Carry the resolved neuron flags into the publisher.
    pub fn for_neuron(config: &crate::NeuronConfig) -> Self {
        Self {
            port: config.axon_port,
            external_ip: config.axon_external_ip,
            allow_non_global: config.network == "local",
            ..Self::default()
        }
    }
    /// Validate before HTTP, chain reads or signing.
    pub fn validate(&self) -> Result<(), PublisherError> {
        if self.port == 0 || self.external_port == Some(0) {
            return Err(PublisherError::Configuration(
                "axon port must be nonzero".into(),
            ));
        }
        if self.recheck.is_zero() {
            return Err(PublisherError::Configuration(
                "axon recheck period must be nonzero".into(),
            ));
        }
        if let Some(certificate) = &self.certificate {
            checked_certificate(certificate)?;
        }
        if let Some(ip) = self.external_ip {
            self.check_ip(ip)?;
        }
        if self.external_ip.is_none() && self.ip_sources.is_empty() {
            return Err(PublisherError::Configuration(
                "supply an external IP or an IPv4 discovery source".into(),
            ));
        }
        for source in &self.ip_sources {
            if !matches!(source.scheme(), "http" | "https")
                || source.host_str().is_none()
                || !source.username().is_empty()
                || source.password().is_some()
                || source.fragment().is_some()
            {
                return Err(PublisherError::Configuration(
                    "IP discovery sources must be credential-free HTTP URLs without fragments"
                        .into(),
                ));
            }
        }
        Ok(())
    }
    fn check_ip(&self, ip: IpAddr) -> Result<(), PublisherError> {
        if !self.allow_non_global && !is_global(ip) {
            return Err(PublisherError::Configuration(format!(
                "axon address {ip} is not globally routable; set allow_non_global only for a local network"
            )));
        }
        Ok(())
    }
}

/// One endpoint reconciliation result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AxonPublishOutcome {
    /// Both endpoint and any configured certificate already match storage.
    Unchanged,
    /// Retry when the first legal best-chain height is reached.
    Scheduled {
        /// Inclusive earliest best height allowed by `ServingRateLimit`.
        retry_at: u64,
    },
    /// A changed endpoint or certificate finalized successfully.
    Published {
        /// Finalized inclusion block.
        block_hash: H256,
    },
    /// Submission has unresolved fate; no unconditional resubmission is permitted.
    Unresolved {
        /// Exact submitted extrinsic identity retained for reconciliation.
        extrinsic_hash: H256,
    },
}

/// Invalid publication configuration, discovery failure or typed transaction error.
#[derive(Debug, thiserror::Error)]
pub enum PublisherError {
    /// Operator configuration cannot describe a publishable endpoint.
    #[error("{0}")]
    Configuration(String),
    /// No configured source supplied a valid, permitted IPv4 address.
    #[error("IPv4 discovery failed: {0}")]
    Discovery(String),
    /// RPC, runtime, policy or chain rejection, retaining semantic pool mappings.
    #[error(transparent)]
    Transaction(#[from] TxError),
    /// HTTP client configuration failure.
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    /// Chain read or source decoding failed.
    #[error(transparent)]
    Client(#[from] rtao_client::ClientError),
    /// Public key lookup failed before signing.
    #[error(transparent)]
    Key(#[from] rtao_signer::SignerError),
}

/// Reconciles the advertised endpoint against coherent best-head chain storage.
pub struct AxonPublisher {
    config: AxonConfig,
    http: tokio::sync::OnceCell<reqwest::Client>,
    load_native_roots: fn() -> Vec<rustls::pki_types::CertificateDer<'static>>,
    unresolved: Option<H256>,
}
impl AxonPublisher {
    /// Validate settings; initialize ring-backed HTTP on the first discovery lookup.
    pub fn new(config: AxonConfig) -> Result<Self, PublisherError> {
        Self::with_root_loader(config, || rustls_native_certs::load_native_certs().certs)
    }
    fn with_root_loader(
        config: AxonConfig,
        load_native_roots: fn() -> Vec<rustls::pki_types::CertificateDer<'static>>,
    ) -> Result<Self, PublisherError> {
        config.validate()?;
        Ok(Self {
            config,
            http: tokio::sync::OnceCell::new(),
            load_native_roots,
            unresolved: None,
        })
    }
    /// Inspect the active publication configuration.
    pub fn config(&self) -> &AxonConfig {
        &self.config
    }
    /// Replace validated settings, including a rotated certificate.
    pub fn update_config(&mut self, config: AxonConfig) -> Result<(), PublisherError> {
        config.validate()?;
        self.config = config;
        Ok(())
    }
    /// Clear unresolved fate only after an operator has definitively reconciled it.
    pub fn resolve_uncertainty(&mut self) {
        self.unresolved = None;
    }

    /// Discover or use the public address; automatic discovery accepts IPv4 only.
    pub async fn resolve_ip(&self) -> Result<IpAddr, PublisherError> {
        if let Some(ip) = self.config.external_ip {
            return Ok(ip);
        }
        let http = self
            .http
            .get_or_try_init(|| async { discovery_client((self.load_native_roots)()) })
            .await?;
        let mut errors = Vec::new();
        for source in &self.config.ip_sources {
            let attempt = async {
                let mut response = http.get(source.clone()).send().await?.error_for_status()?;
                let mut bytes = Vec::new();
                while let Some(chunk) = response.chunk().await? {
                    if bytes.len() + chunk.len() > 128 {
                        return Err(PublisherError::Discovery(
                            "source response exceeds 128 bytes".into(),
                        ));
                    }
                    bytes.extend_from_slice(&chunk);
                }
                let text = std::str::from_utf8(&bytes)
                    .map_err(|error| PublisherError::Discovery(error.to_string()))?;
                let ipv4: Ipv4Addr =
                    text.trim()
                        .parse()
                        .map_err(|error: std::net::AddrParseError| {
                            PublisherError::Discovery(error.to_string())
                        })?;
                let ip = IpAddr::V4(ipv4);
                self.config.check_ip(ip)?;
                Ok(ip)
            };
            match tokio::time::timeout(Duration::from_secs(5), attempt).await {
                Ok(Ok(ip)) => return Ok(ip),
                Ok(Err(error)) => errors.push(error.to_string()),
                Err(_) => errors.push("source timed out after five seconds".into()),
            }
        }
        Err(PublisherError::Discovery(errors.join("; ")))
    }

    /// Publish only a changed endpoint or certificate, scheduling chain rate limits.
    pub async fn poll<K: KeySource<Hotkey>>(
        &mut self,
        executor: &Executor,
        keys: &K,
        netuid: NetUid,
    ) -> Result<AxonPublishOutcome, PublisherError> {
        let ip = self.resolve_ip().await?;
        let hotkey = keys.address()?;
        let snapshot = executor.client().at(BlockRef::Best).await?;
        let (previous, rate) = snapshot
            .fetch((
                st::AXONS.at((netuid, hotkey)),
                st::SERVING_RATE_LIMIT.at((netuid,)),
            ))
            .await?;
        let previous = previous.unwrap_or_default();
        let certificate = if self.config.certificate.is_some() {
            snapshot
                .storage(&st::NEURON_CERTIFICATES, (netuid, hotkey))
                .await?
        } else {
            None
        };
        let desired = desired_axon(&self.config, ip);
        if endpoint_matches(&previous, &desired)
            && self
                .config
                .certificate
                .as_ref()
                .is_none_or(|configured| certificate.as_ref() == Some(configured))
        {
            self.unresolved = None;
            return Ok(AxonPublishOutcome::Unchanged);
        }
        if let Some(extrinsic_hash) = self.unresolved {
            return Ok(AxonPublishOutcome::Unresolved { extrinsic_hash });
        }
        let now = snapshot.block().number;
        if let Some(retry_at) = rate_limit_retry(now, previous.block, rate) {
            return Ok(AxonPublishOutcome::Scheduled { retry_at });
        }
        let options = ExecuteOptions {
            period: Some(128),
            wait_for_inclusion: true,
            wait_for_finalization: true,
            retries: 0,
            reconcile: true,
            deadline: Some(Duration::from_secs(60)),
            submit_deadline: Some(Duration::from_secs(30)),
            ..Default::default()
        };
        let result = if let Some(certificate) = &self.config.certificate {
            executor
                .execute(
                    &ServeAxonTls {
                        netuid,
                        ip: ip.to_string(),
                        port: desired.port,
                        certificate: hex::encode(checked_certificate(certificate)?),
                        protocol: desired.protocol,
                        version: desired.version,
                    },
                    keys,
                    &options,
                )
                .await
        } else {
            executor
                .execute(
                    &ServeAxon {
                        netuid,
                        ip: ip.to_string(),
                        port: desired.port,
                        protocol: desired.protocol,
                        version: desired.version,
                    },
                    keys,
                    &options,
                )
                .await
        };
        let result = match result {
            Err(TxError::Chain(error))
                if error.name.as_deref() == Some("ServingRateLimitExceeded") =>
            {
                return self.retry_after_rejection(executor, netuid, hotkey).await;
            }
            Err(error) => return Err(error.into()),
            Ok(result) => result,
        };
        if !result.success
            && result
                .error
                .as_ref()
                .and_then(|error| error.name.as_deref())
                == Some("ServingRateLimitExceeded")
        {
            return self.retry_after_rejection(executor, netuid, hotkey).await;
        }
        self.outcome(result)
    }
    async fn retry_after_rejection(
        &self,
        executor: &Executor,
        netuid: NetUid,
        hotkey: AccountId,
    ) -> Result<AxonPublishOutcome, PublisherError> {
        let snapshot = executor.client().at(BlockRef::Best).await?;
        let (previous, rate) = snapshot
            .fetch((
                st::AXONS.at((netuid, hotkey)),
                st::SERVING_RATE_LIMIT.at((netuid,)),
            ))
            .await?;
        Ok(AxonPublishOutcome::Scheduled {
            retry_at: previous
                .unwrap_or_default()
                .block
                .saturating_add(rate)
                .max(snapshot.block().number.saturating_add(1)),
        })
    }
    fn outcome<O>(
        &mut self,
        result: ExtrinsicResult<O>,
    ) -> Result<AxonPublishOutcome, PublisherError> {
        if let Some(extrinsic_hash) = result.fate_unknown {
            self.unresolved = Some(extrinsic_hash);
            return Ok(AxonPublishOutcome::Unresolved { extrinsic_hash });
        }
        if !result.success {
            return Err(TxError::Chain(
                result
                    .error
                    .unwrap_or_else(|| ChainError::new(result.message, None)),
            )
            .into());
        }
        let block_hash = result.block_hash.ok_or_else(|| {
            PublisherError::Configuration(
                "publication succeeded without finalized inclusion identity".into(),
            )
        })?;
        Ok(AxonPublishOutcome::Published { block_hash })
    }
    /// Optionally clear serving information during graceful shutdown.
    pub async fn reset<K: KeySource<Hotkey>>(
        &mut self,
        executor: &Executor,
        keys: &K,
        netuid: NetUid,
    ) -> Result<Option<AxonPublishOutcome>, PublisherError> {
        if !self.config.reset_on_shutdown {
            return Ok(None);
        }
        if let Some(extrinsic_hash) = self.unresolved {
            return Ok(Some(AxonPublishOutcome::Unresolved { extrinsic_hash }));
        }
        let hotkey = keys.address()?;
        let snapshot = executor.client().at(BlockRef::Best).await?;
        let (previous, rate) = snapshot
            .fetch((
                st::AXONS.at((netuid, hotkey)),
                st::SERVING_RATE_LIMIT.at((netuid,)),
            ))
            .await?;
        let previous = previous.unwrap_or_default();
        if previous.ip == 0 {
            return Ok(Some(AxonPublishOutcome::Unchanged));
        }
        if let Some(retry_at) = rate_limit_retry(snapshot.block().number, previous.block, rate) {
            return Ok(Some(AxonPublishOutcome::Scheduled { retry_at }));
        }
        let result = executor
            .execute(
                &ResetAxon { netuid },
                keys,
                &ExecuteOptions {
                    period: Some(128),
                    wait_for_inclusion: true,
                    wait_for_finalization: true,
                    retries: 0,
                    deadline: Some(Duration::from_secs(30)),
                    submit_deadline: Some(Duration::from_secs(10)),
                    ..Default::default()
                },
            )
            .await?;
        self.outcome(result).map(Some)
    }
}
fn discovery_client(
    native: Vec<rustls::pki_types::CertificateDer<'static>>,
) -> Result<reqwest::Client, PublisherError> {
    let mut roots = rustls::RootCertStore::empty();
    roots.add_parsable_certificates(native);
    let mut tls = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|error| PublisherError::Configuration(error.to_string()))?
    .with_root_certificates(roots)
    .with_no_client_auth();
    tls.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    Ok(reqwest::Client::builder()
        .tls_backend_preconfigured(tls)
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(5))
        .build()?)
}
fn checked_certificate(certificate: &NeuronCertificate) -> Result<Vec<u8>, PublisherError> {
    if certificate.public_key.len() > 64 {
        return Err(PublisherError::Configuration("TLS certificate exceeds 65 bytes including its algorithm byte; the chain would silently discard it".into()));
    }
    let mut bytes = Vec::with_capacity(1 + certificate.public_key.len());
    bytes.push(certificate.algorithm);
    bytes.extend_from_slice(&certificate.public_key);
    Ok(bytes)
}
fn desired_axon(config: &AxonConfig, ip: IpAddr) -> AxonInfo {
    let (ip, ip_type) = match ip {
        IpAddr::V4(ip) => (u128::from(u32::from(ip)), 4),
        IpAddr::V6(ip) => (u128::from(ip), 6),
    };
    AxonInfo {
        ip,
        ip_type,
        port: config.external_port.unwrap_or(config.port),
        version: config.version,
        protocol: config.protocol,
        ..Default::default()
    }
}
fn endpoint_matches(current: &AxonInfo, desired: &AxonInfo) -> bool {
    (
        current.ip,
        current.port,
        current.ip_type,
        current.protocol,
        current.version,
    ) == (
        desired.ip,
        desired.port,
        desired.ip_type,
        desired.protocol,
        desired.version,
    )
}
fn rate_limit_retry(now: u64, last: u64, rate: u64) -> Option<u64> {
    (rate != 0 && last != 0 && now.saturating_sub(last) < rate).then(|| last.saturating_add(rate))
}
fn is_global(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, c, d] = ip.octets();
            !(a == 0
                || a == 10
                || a == 127
                || a >= 224
                || (a == 100 && (64..=127).contains(&b))
                || (a == 169 && b == 254)
                || (a == 172 && (16..=31).contains(&b))
                || (a == 192 && b == 168)
                || (a == 192 && b == 0 && c == 0 && !matches!(d, 9 | 10))
                || (a == 192 && b == 0 && c == 2)
                || (a == 192 && b == 88 && c == 99 && d != 2)
                || (a == 198 && (b == 18 || b == 19 || (b == 51 && c == 100)))
                || (a == 203 && b == 0 && c == 113))
        }
        IpAddr::V6(ip) => {
            let segments = ip.segments();
            let first = segments[0];
            if let Some(ip) = ip.to_ipv4_mapped() {
                return is_global(IpAddr::V4(ip));
            }
            if segments[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
                return true;
            }
            let protocol_assignment = first == 0x2001 && segments[1] <= 0x01ff;
            let public_assignment =
                (segments[1] == 1 && segments[2..7] == [0; 5] && matches!(segments[7], 1..=3))
                    || segments[1] == 3
                    || (segments[1] == 4 && segments[2] == 0x112)
                    || (0x20..=0x3f).contains(&segments[1]);
            (first & 0xe000) == 0x2000
                && !(first == 0x2001 && segments[1] == 0x0db8)
                && (!protocol_assignment || public_assignment)
                && first != 0x2002
                && !(first == 0x3fff && (segments[1] & 0xf000) == 0)
        }
    }
}
