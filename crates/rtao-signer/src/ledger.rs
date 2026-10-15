// Derived from RaoFoundation/subtensor sdk/python/bittensor/ledger.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Ledger clear signing through the pinned core's Polkadot generic app API.
//!
//! Device calls run on Tokio's blocking pool. A process-wide operation lock
//! covers every complete device call, including calls made by separately opened
//! signers, so address and version APDUs cannot interrupt a signing exchange.
//! Dropping a signing future does not cancel an already running device request.

use std::{fmt, sync::Arc};

use rtao_core::{CoreError, digest::ChainInfo, ledger::LedgerDevice};

use crate::{
    AccountId, BoxFuture, CryptoType, MultiSignature, Signer, SignerCaps, SignerError,
    SigningContext, SigningRequest,
};

pub use rtao_core::ledger::LedgerAddress;

/// Failure to open, derive, prove or sign with a Ledger device.
#[doc(alias = "bittensor.ledger.LedgerError")]
pub type LedgerError = SignerError;

mod backend {
    use super::{CoreError, LedgerAddress, LedgerDevice};

    /// Blocking Ledger operations, implemented by the pinned core in production.
    ///
    /// Testkit backends may record or replay whole operations. Implementations
    /// must not call back into a `LedgerSigner` while an operation is running.
    pub trait LedgerBackend: Send + Sync + 'static {
        /// Read the Polkadot generic app's `(major, minor, patch)` version.
        fn app_version(&self) -> Result<(u16, u16, u16), CoreError>;

        /// Derive the ed25519 address, optionally showing it on the device.
        fn address(
            &self,
            account: u32,
            index: u32,
            ss58_prefix: u16,
            confirm: bool,
        ) -> Result<LedgerAddress, CoreError>;

        /// Clear-sign the unhashed payload with its RFC-0078 metadata proof.
        ///
        /// The result is the complete 65-byte SCALE `MultiSignature` reply.
        fn sign(
            &self,
            account: u32,
            index: u32,
            payload: &[u8],
            proof: &[u8],
        ) -> Result<Vec<u8>, CoreError>;
    }

    impl LedgerBackend for LedgerDevice {
        fn app_version(&self) -> Result<(u16, u16, u16), CoreError> {
            LedgerDevice::app_version(self).map_err(Into::into)
        }

        fn address(
            &self,
            account: u32,
            index: u32,
            ss58_prefix: u16,
            confirm: bool,
        ) -> Result<LedgerAddress, CoreError> {
            LedgerDevice::address(self, account, index, ss58_prefix, confirm).map_err(Into::into)
        }

        fn sign(
            &self,
            account: u32,
            index: u32,
            payload: &[u8],
            proof: &[u8],
        ) -> Result<Vec<u8>, CoreError> {
            LedgerDevice::sign(self, account, index, payload, proof).map_err(Into::into)
        }
    }
}

#[cfg(not(feature = "test-util"))]
use backend::LedgerBackend;
#[cfg(feature = "test-util")]
pub use backend::LedgerBackend;

// The core opens the first HID device rather than exposing a stable device ID.
// Serializing all handles is conservative and also protects callers that open
// two different accounts on the same physical Ledger. The guard lives inside
// the blocking closure, so cancellation cannot release it between APDUs.
static DEVICE_OPERATION: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Derivation and metadata display options for a Ledger signer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerOptions {
    /// Hardened account component in `m/44'/354'/account'/0'/index'`.
    pub account: u32,
    /// Hardened address index in `m/44'/354'/account'/0'/index'`.
    pub index: u32,
    /// Prefix used when deriving and confirming the device address.
    pub ss58_format: u16,
    /// Token decimal places used in the RFC-0078 digest and proof.
    pub decimals: u8,
    /// Token symbol used in the RFC-0078 digest and proof.
    pub token_symbol: String,
}

impl Default for LedgerOptions {
    fn default() -> Self {
        Self {
            account: 0,
            index: 0,
            ss58_format: 42,
            decimals: 9,
            token_symbol: "TAO".into(),
        }
    }
}

/// An ed25519 clear signer backed by the Ledger Polkadot generic app.
///
/// Construction opens the device and derives its address silently. The app
/// needs RFC-0078 metadata; arbitrary messages are refused. No device, emulator
/// or HID compatibility is implied by the offline backend tests.
#[derive(Clone)]
#[doc(alias = "bittensor.ledger.LedgerSigner")]
pub struct LedgerSigner {
    backend: Arc<dyn LedgerBackend>,
    options: LedgerOptions,
    address: LedgerAddress,
}

impl fmt::Debug for LedgerSigner {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LedgerSigner")
            .field("address", &self.address.ss58_address)
            .field(
                "path",
                &format_args!(
                    "m/44'/354'/{}'/0'/{}'",
                    self.options.account, self.options.index
                ),
            )
            .finish()
    }
}

impl LedgerSigner {
    /// Open the first reachable Ledger and silently derive its ed25519 address.
    ///
    /// Requires an entered Tokio runtime. Device discovery and address I/O both
    /// run off the runtime worker and preserve the core's device errors.
    pub async fn open(options: LedgerOptions) -> Result<Self, SignerError> {
        offload(move || {
            with_device_operation(|| {
                let backend: Arc<dyn LedgerBackend> =
                    Arc::new(LedgerDevice::open().map_err(CoreError::from)?);
                Self::derive(backend, options)
            })
        })
        .await
    }

    /// Use an injected whole-operation backend instead of opening a HID device.
    ///
    /// This constructor is available only with `test-util`; it shares the same
    /// operation lock, offload and signature checks as the production path.
    #[cfg(feature = "test-util")]
    pub async fn with_backend(
        backend: Arc<dyn LedgerBackend>,
        options: LedgerOptions,
    ) -> Result<Self, SignerError> {
        Self::from_backend(backend, options).await
    }

    #[cfg(any(test, feature = "test-util"))]
    async fn from_backend(
        backend: Arc<dyn LedgerBackend>,
        options: LedgerOptions,
    ) -> Result<Self, SignerError> {
        offload(move || with_device_operation(|| Self::derive(backend, options))).await
    }

    fn derive(
        backend: Arc<dyn LedgerBackend>,
        options: LedgerOptions,
    ) -> Result<Self, SignerError> {
        let address =
            backend.address(options.account, options.index, options.ss58_format, false)?;
        Ok(Self {
            backend,
            options,
            address,
        })
    }

    /// The address returned by the device at construction.
    pub fn ss58_address(&self) -> &str {
        &self.address.ss58_address
    }

    /// The ed25519 public key returned by the device at construction.
    pub fn public_key(&self) -> &[u8; 32] {
        &self.address.public_key
    }

    /// Prefix used for address derivation and confirmation.
    pub fn ss58_format(&self) -> u16 {
        self.options.ss58_format
    }

    /// The derivation and token display options of this signer.
    pub fn options(&self) -> &LedgerOptions {
        &self.options
    }

    /// Read the generic app version off the async runtime's worker thread.
    pub async fn app_version(&self) -> Result<(u16, u16, u16), SignerError> {
        let backend = self.backend.clone();
        offload(move || with_device_operation(|| backend.app_version().map_err(Into::into))).await
    }

    /// Show the derived address on the Ledger and await the user's approval.
    ///
    /// Rejects a different public key, for example if the attached device was
    /// replaced since construction.
    pub async fn confirm_address(&self) -> Result<String, SignerError> {
        let backend = self.backend.clone();
        let options = self.options.clone();
        let expected = self.address.public_key;
        offload(move || {
            with_device_operation(|| {
                let address =
                    backend.address(options.account, options.index, options.ss58_format, true)?;
                if address.public_key != expected {
                    return Err(CoreError::Device(
                        "confirmed Ledger public key differs from the signer's account".into(),
                    )
                    .into());
                }
                Ok(address.ss58_address)
            })
        })
        .await
    }

    /// Compute the core's RFC-0078 digest with this signer's token options.
    ///
    /// The runtime's spec name, version and client SS58 prefix come from the
    /// context. The caller must prepare `CheckMetadataHash` with the same token
    /// constants before signing. Client submission caches this digest on its
    /// runtime handle; this standalone helper does not maintain another cache.
    pub async fn metadata_digest(&self, context: &SigningContext) -> Result<[u8; 32], SignerError> {
        let metadata = context.metadata_bytes.clone();
        let info = self.chain_info(context);
        offload(move || {
            rtao_core::digest::metadata_digest(&metadata, &info)
                .map_err(CoreError::from)
                .map_err(Into::into)
        })
        .await
    }

    fn chain_info(&self, context: &SigningContext) -> ChainInfo {
        let mut info = context.chain_info.clone();
        info.decimals = self.options.decimals;
        info.token_symbol.clone_from(&self.options.token_symbol);
        info
    }
}

impl Signer for LedgerSigner {
    fn account(&self) -> AccountId {
        AccountId(self.address.public_key)
    }

    fn crypto_type(&self) -> CryptoType {
        CryptoType::Ed25519
    }

    fn caps(&self) -> SignerCaps {
        SignerCaps::METADATA_HASH | SignerCaps::UNSIGNED_TX
    }

    fn metadata_constants(&self) -> Option<(u8, String)> {
        Some((self.options.decimals, self.options.token_symbol.clone()))
    }

    fn sign<'a>(
        &'a self,
        request: &'a SigningRequest<'a>,
    ) -> BoxFuture<'a, Result<MultiSignature, SignerError>> {
        Box::pin(async move {
            let backend = self.backend.clone();
            let account = self.options.account;
            let index = self.options.index;
            let call = request.call.to_vec();
            let extra = request.included_in_extrinsic.to_vec();
            let additional = request.included_in_signed_data.to_vec();
            let metadata = request.context.metadata_bytes.clone();
            let info = self.chain_info(request.context);
            offload(move || {
                let proof = rtao_core::digest::generate_extrinsic_proof(
                    &call,
                    &extra,
                    &additional,
                    &metadata,
                    &info,
                )
                .map_err(CoreError::from)?;
                // The generic app hashes long payloads itself. In particular,
                // request.payload is already hashed above 256 bytes and must
                // never be sent in place of these three original wire parts.
                let mut payload = call;
                payload.extend_from_slice(&extra);
                payload.extend_from_slice(&additional);
                with_device_operation(|| {
                    let reply = backend.sign(account, index, &payload, &proof)?;
                    MultiSignature::from_bytes_for(&reply, CryptoType::Ed25519)
                })
            })
            .await
        })
    }
}

async fn offload<T, F>(work: F) -> Result<T, SignerError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, SignerError> + Send + 'static,
{
    rtao_core::offload(work).await.map_err(|error| {
        SignerError::Core(CoreError::Device(format!(
            "Ledger blocking operation did not complete: {error}"
        )))
    })?
}

fn with_device_operation<T>(
    operation: impl FnOnce() -> Result<T, SignerError>,
) -> Result<T, SignerError> {
    let _guard = DEVICE_OPERATION.lock().map_err(|_| {
        CoreError::Device("a previous Ledger operation panicked; restart the process".into())
    })?;
    operation()
}

#[cfg(test)]
#[path = "ledger/tests.rs"]
mod tests;
