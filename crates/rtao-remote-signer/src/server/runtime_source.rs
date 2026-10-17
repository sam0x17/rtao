//! Trusted runtime sources. Client-supplied metadata is never decoded.

use std::fmt;

use rtao_btauth::BoxFuture;
use rtao_primitives::{AccountId, Balance, CryptoType, Tao};
use rtao_runtime::{CallBytes, RuntimeHandle};

#[cfg(feature = "server-online")]
use crate::wire::{from_hex, to_hex};

/// A trusted runtime source failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceError(pub String);

impl fmt::Display for SourceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
impl std::error::Error for SourceError {}

/// The runtime a server is willing to sign for.
pub trait RuntimeSource: Send + Sync {
    /// The trusted runtime the server would sign for now.
    fn current(&self) -> BoxFuture<'_, Result<RuntimeHandle, SourceError>>;
    /// Re-read the trusted runtime after a version mismatch; offline sources repeat `current`.
    fn refresh(&self) -> BoxFuture<'_, Result<RuntimeHandle, SourceError>> {
        self.current()
    }
    /// The runtime's `specName`, used in the RFC-0078 chain information.
    fn spec_name(&self) -> BoxFuture<'_, Result<String, SourceError>>;
    /// Whether the source observes a live chain (era anchors and fees).
    fn is_online(&self) -> bool {
        false
    }
    /// The chain genesis observed by an online source.
    fn genesis(&self) -> Option<[u8; 32]> {
        None
    }
    /// Online: the number of the block with this hash, read from its header.
    ///
    /// `Ok(None)` means the node does not know the hash. This must not
    /// resolve the block's runtime: the hash is client-chosen. Offline
    /// sources return `Ok(None)`; the server skips the anchor check for them.
    fn block_number(&self, _hash: [u8; 32]) -> BoxFuture<'_, Result<Option<u64>, SourceError>> {
        Box::pin(async { Ok(None) })
    }
    /// Online: the current best block number.
    fn best_number(&self) -> BoxFuture<'_, Result<u64, SourceError>> {
        Box::pin(async { Err(SourceError("this runtime source observes no chain".into())) })
    }
    /// Online: the canonical block hash at `number`; `Ok(None)` above the best block.
    fn canonical_hash(&self, _number: u64) -> BoxFuture<'_, Result<Option<[u8; 32]>, SourceError>> {
        Box::pin(async { Ok(None) })
    }
    /// Online: estimate the call's fee for `max_fee`; offline returns `None`.
    fn estimate_fee(
        &self,
        _call: CallBytes,
        _account: AccountId,
        _crypto: CryptoType,
    ) -> BoxFuture<'_, Result<Option<Balance<Tao>>, SourceError>> {
        Box::pin(async { Ok(None) })
    }
}

/// An operator-pinned runtime for offline or air-gapped signing.
#[derive(Clone, Debug)]
pub struct PinnedRuntimeSource {
    runtime: RuntimeHandle,
    spec_name: String,
}

impl PinnedRuntimeSource {
    /// Adopt an already parsed runtime and its `specName`.
    pub fn new(runtime: RuntimeHandle, spec_name: impl Into<String>) -> Self {
        Self {
            runtime,
            spec_name: spec_name.into(),
        }
    }

    /// Parse operator-supplied V15 metadata with operator-supplied versions.
    pub fn parse(
        metadata: &[u8],
        spec_version: u32,
        transaction_version: u32,
        ss58_format: u16,
        spec_name: impl Into<String>,
    ) -> Result<Self, SourceError> {
        let runtime =
            RuntimeHandle::parse(metadata, spec_version, transaction_version, ss58_format)
                .map_err(|error| SourceError(format!("cannot parse pinned metadata: {error}")))?;
        Ok(Self::new(runtime, spec_name))
    }

    /// The pinned runtime.
    pub fn runtime(&self) -> &RuntimeHandle {
        &self.runtime
    }
}

impl RuntimeSource for PinnedRuntimeSource {
    fn current(&self) -> BoxFuture<'_, Result<RuntimeHandle, SourceError>> {
        Box::pin(async move { Ok(self.runtime.clone()) })
    }
    fn spec_name(&self) -> BoxFuture<'_, Result<String, SourceError>> {
        Box::pin(async move { Ok(self.spec_name.clone()) })
    }
}

/// A live node chosen by the operator.
#[cfg(feature = "server-online")]
#[cfg_attr(docsrs, doc(cfg(feature = "server-online")))]
#[derive(Clone, Debug)]
pub struct ClientRuntimeSource {
    client: rtao_client::Client,
}

#[cfg(feature = "server-online")]
impl ClientRuntimeSource {
    /// Adopt a connected client.
    pub fn new(client: rtao_client::Client) -> Self {
        Self { client }
    }

    /// Connect to an operator-chosen endpoint without a persistent cache.
    pub async fn connect(endpoint: &str) -> Result<Self, SourceError> {
        let mut config = rtao_client::ClientConfig::for_network(endpoint)
            .map_err(|error| SourceError(error.to_string()))?;
        config.runtime_cache_dir = None;
        config.config_store = None;
        let client = rtao_client::Client::connect(config)
            .await
            .map_err(|error| SourceError(error.to_string()))?;
        Ok(Self::new(client))
    }

    /// The underlying client.
    pub fn client(&self) -> &rtao_client::Client {
        &self.client
    }
}

#[cfg(feature = "server-online")]
fn source_error(error: rtao_client::ClientError) -> SourceError {
    SourceError(error.to_string())
}

/// Read a Substrate JSON header's `number`, a `0x` hex quantity.
#[cfg(feature = "server-online")]
fn header_number(header: &serde_json::Value) -> Result<u64, SourceError> {
    header
        .get("number")
        .and_then(serde_json::Value::as_str)
        .and_then(|number| number.strip_prefix("0x"))
        .and_then(|digits| u64::from_str_radix(digits, 16).ok())
        .ok_or_else(|| SourceError("the node returned a header without a valid number".into()))
}

#[cfg(feature = "server-online")]
impl RuntimeSource for ClientRuntimeSource {
    fn current(&self) -> BoxFuture<'_, Result<RuntimeHandle, SourceError>> {
        Box::pin(async move {
            rtao_client::View::runtime(&self.client)
                .await
                .map_err(source_error)
        })
    }
    fn refresh(&self) -> BoxFuture<'_, Result<RuntimeHandle, SourceError>> {
        Box::pin(async move { self.client.refresh_runtime().await.map_err(source_error) })
    }
    fn spec_name(&self) -> BoxFuture<'_, Result<String, SourceError>> {
        Box::pin(async move {
            let version: serde_json::Value = self
                .client
                .request("state_getRuntimeVersion", serde_json::json!([null]))
                .await
                .map_err(source_error)?;
            version
                .get("specName")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| SourceError("state_getRuntimeVersion has no specName".into()))
        })
    }
    fn is_online(&self) -> bool {
        true
    }
    fn genesis(&self) -> Option<[u8; 32]> {
        Some(self.client.genesis().0)
    }
    fn block_number(&self, hash: [u8; 32]) -> BoxFuture<'_, Result<Option<u64>, SourceError>> {
        Box::pin(async move {
            let header: Option<serde_json::Value> = self
                .client
                .request("chain_getHeader", serde_json::json!([to_hex(&hash)]))
                .await
                .map_err(source_error)?;
            header.map(|header| header_number(&header)).transpose()
        })
    }
    fn best_number(&self) -> BoxFuture<'_, Result<u64, SourceError>> {
        Box::pin(async move {
            let header: Option<serde_json::Value> = self
                .client
                .request("chain_getHeader", serde_json::json!([]))
                .await
                .map_err(source_error)?;
            header
                .as_ref()
                .ok_or_else(|| SourceError("the node returned no best header".into()))
                .and_then(header_number)
        })
    }
    fn canonical_hash(&self, number: u64) -> BoxFuture<'_, Result<Option<[u8; 32]>, SourceError>> {
        Box::pin(async move {
            let hash: Option<String> = self
                .client
                .request("chain_getBlockHash", serde_json::json!([number]))
                .await
                .map_err(source_error)?;
            hash.map(|hash| {
                from_hex(&hash)
                    .ok()
                    .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok())
                    .ok_or_else(|| {
                        SourceError("chain_getBlockHash returned an invalid hash".into())
                    })
            })
            .transpose()
        })
    }
    fn estimate_fee(
        &self,
        call: CallBytes,
        account: AccountId,
        crypto: CryptoType,
    ) -> BoxFuture<'_, Result<Option<Balance<Tao>>, SourceError>> {
        Box::pin(async move {
            let estimate = self
                .client
                .estimate_weight(call, account, crypto)
                .await
                .map_err(source_error)?;
            Ok(Some(Balance::<Tao>::from_rao(estimate.partial_fee)))
        })
    }
}
