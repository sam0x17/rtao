//! Shared, immutable runtime metadata and spec-stamped call bytes.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, Mutex, OnceLock};

use blake2::{Blake2b, Digest, digest::consts::U32};
use bytes::Bytes;
use frame_metadata::{RuntimeMetadata, RuntimeMetadataPrefixed};
use parity_scale_codec::Decode;
use rtao_core::codec::Value;
use rtao_core::digest::ChainInfo;
use rtao_core::runtime::{Runtime, RuntimeApiMethodInfo, StorageInfo};
use rtao_core::{CoreError, extrinsic::TxParams};
use scale_info::{Field, form::PortableForm};

use crate::shape::{OuterEnums, Projection, ShapeHash, ShapeHasher};
use crate::surface::{SurfaceReport, verify_surface};

/// Metadata features available to this runtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MetadataLevel {
    /// V15 metadata describes storage, runtime APIs, and signed transactions.
    V15,
    /// Legacy V14 metadata supports reads, with runtime APIs restricted by the client.
    ///
    /// The client may create this level only after its V15 fallback condition
    /// has been established. Call composition and signing are refused.
    V14Degraded,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum ShapeKey {
    Exact(u32),
    Fields(Vec<(Option<String>, u32)>),
    Tuple(Vec<u32>),
    Projection(u32, Projection),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ChainInfoKey {
    spec_version: u32,
    spec_name: String,
    base58_prefix: u16,
    decimals: u8,
    token_symbol: String,
}

impl From<&ChainInfo> for ChainInfoKey {
    fn from(info: &ChainInfo) -> Self {
        Self {
            spec_version: info.spec_version,
            spec_name: info.spec_name.clone(),
            base58_prefix: info.base58_prefix,
            decimals: info.decimals,
            token_symbol: info.token_symbol.clone(),
        }
    }
}

/// A parsed runtime and the caches shared by all handles for that runtime.
///
/// This type performs no I/O. The client owns cache eviction and resolves the
/// runtime associated with a block before creating an entry.
pub struct RuntimeEntry {
    runtime: Runtime,
    level: MetadataLevel,
    outer_enums: Option<OuterEnums>,
    pallet_indices: BTreeMap<String, u8>,
    call_indices: BTreeMap<(String, String), u8>,
    storage_indices: BTreeMap<(String, String), (usize, usize)>,
    api_indices: BTreeMap<(String, String), (usize, usize)>,
    api_versions: BTreeMap<String, u32>,
    shapes: Mutex<BTreeMap<ShapeKey, Result<ShapeHash, CoreError>>>,
    surface: OnceLock<SurfaceReport>,
    digest: OnceLock<(ChainInfoKey, Result<[u8; 32], CoreError>)>,
    signing: Result<(), CoreError>,
}

impl fmt::Debug for RuntimeEntry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeEntry")
            .field("spec_version", &self.runtime.spec_version)
            .field("transaction_version", &self.runtime.transaction_version)
            .field("metadata_level", &self.level)
            .field("pallet_count", &self.runtime.pallets.len())
            .field("signing", &self.signing)
            .finish_non_exhaustive()
    }
}

impl RuntimeEntry {
    /// The immutable parsed runtime shared by all handles.
    pub fn core(&self) -> &Runtime {
        &self.runtime
    }

    /// Metadata capabilities carried by this entry.
    pub fn metadata_level(&self) -> MetadataLevel {
        self.level
    }
}

/// A cheap clone of one immutable parsed runtime and its verification caches.
#[derive(Clone, Debug)]
pub struct RuntimeHandle(Arc<RuntimeEntry>);

impl RuntimeHandle {
    /// Parse an unwrapped V14 or V15 `MetadataVersioned` blob.
    ///
    /// V16 is tooling-only and is rejected. A client must establish the
    /// missing-method or unavailable-version fallback before using V14.
    pub fn parse(
        metadata: &[u8],
        spec_version: u32,
        transaction_version: u32,
        ss58_format: u16,
    ) -> Result<Self, CoreError> {
        Self::new(
            Runtime::parse(metadata, spec_version, transaction_version, ss58_format)
                .map_err(CoreError::from)?,
        )
    }

    /// Adopt an upstream runtime, build name caches, and check extension encoding.
    ///
    /// An unknown extension with a nonempty payload disables signing, but
    /// leaves the entry available for reads.
    pub fn new(runtime: Runtime) -> Result<Self, CoreError> {
        Self::new_with_api_versions(runtime, BTreeMap::new())
    }

    /// Adopt a runtime and known runtime API versions obtained at the same block.
    ///
    /// V15 metadata contains signatures but no API versions. The client
    /// resolves these trait names from the runtime version's advertised API
    /// IDs. An unknown trait name is rejected, and omitted versions remain
    /// explicitly unverified by surface inspection.
    pub fn new_with_api_versions(
        runtime: Runtime,
        api_versions: BTreeMap<String, u32>,
    ) -> Result<Self, CoreError> {
        let mut input = runtime.metadata_bytes.as_slice();
        let metadata = RuntimeMetadataPrefixed::decode(&mut input).map_err(|error| {
            CoreError::Codec(format!("cannot decode runtime metadata: {error}"))
        })?;
        if metadata.0 != frame_metadata::META_RESERVED {
            return Err(CoreError::Codec(
                "invalid runtime metadata magic prefix".into(),
            ));
        }
        if !input.is_empty() {
            return Err(CoreError::Codec(format!(
                "{} trailing bytes in runtime metadata",
                input.len()
            )));
        }
        let (level, outer_enums) = match metadata.1 {
            RuntimeMetadata::V15(metadata) if runtime.is_v15 => (
                MetadataLevel::V15,
                Some(OuterEnums {
                    call: metadata.outer_enums.call_enum_ty.id,
                    event: metadata.outer_enums.event_enum_ty.id,
                    error: metadata.outer_enums.error_enum_ty.id,
                }),
            ),
            RuntimeMetadata::V14(_) if !runtime.is_v15 => (MetadataLevel::V14Degraded, None),
            metadata => {
                return Err(CoreError::Codec(format!(
                    "runtime metadata level disagrees with V{} bytes",
                    metadata.version()
                )));
            }
        };
        let mut pallet_indices = BTreeMap::new();
        let mut used_pallet_indices = std::collections::BTreeSet::new();
        let mut call_indices = BTreeMap::new();
        let mut storage_indices = BTreeMap::new();
        for (pallet_position, pallet) in runtime.pallets.iter().enumerate() {
            if !used_pallet_indices.insert(pallet.index) {
                return Err(CoreError::Codec(format!(
                    "duplicate pallet index {}",
                    pallet.index
                )));
            }
            if pallet_indices
                .insert(pallet.name.clone(), pallet.index)
                .is_some()
            {
                return Err(CoreError::Codec(format!(
                    "duplicate pallet {}",
                    pallet.name
                )));
            }
            if let Some(call_type) = pallet.calls_type {
                let scale_info::TypeDef::Variant(variants) = &runtime
                    .resolve(call_type)
                    .map_err(CoreError::from)?
                    .type_def
                else {
                    return Err(CoreError::Codec(format!(
                        "call type for {} is not an enum",
                        pallet.name
                    )));
                };
                let mut used_call_indices = std::collections::BTreeSet::new();
                for call in &variants.variants {
                    if !used_call_indices.insert(call.index) {
                        return Err(CoreError::Codec(format!(
                            "duplicate call index {} in {}",
                            call.index, pallet.name
                        )));
                    }
                    if call_indices
                        .insert((pallet.name.clone(), call.name.clone()), call.index)
                        .is_some()
                    {
                        return Err(CoreError::Codec(format!(
                            "duplicate call {}.{}",
                            pallet.name, call.name
                        )));
                    }
                }
            }
            for (storage_position, storage) in pallet.storage.iter().enumerate() {
                if storage_indices
                    .insert(
                        (pallet.name.clone(), storage.name.clone()),
                        (pallet_position, storage_position),
                    )
                    .is_some()
                {
                    return Err(CoreError::Codec(format!(
                        "duplicate storage {}.{}",
                        pallet.name, storage.name
                    )));
                }
            }
        }
        let mut api_indices = BTreeMap::new();
        let mut used_api_names = std::collections::BTreeSet::new();
        for api in api_versions.keys() {
            if !runtime.apis.iter().any(|known| known.name == *api) {
                return Err(CoreError::NotInRuntime(format!("runtime API {api}")));
            }
        }
        for (api_position, api) in runtime.apis.iter().enumerate() {
            if !used_api_names.insert(api.name.clone()) {
                return Err(CoreError::Codec(format!(
                    "duplicate runtime API {}",
                    api.name
                )));
            }
            for (method_position, method) in api.methods.iter().enumerate() {
                if api_indices
                    .insert(
                        (api.name.clone(), method.name.clone()),
                        (api_position, method_position),
                    )
                    .is_some()
                {
                    return Err(CoreError::Codec(format!(
                        "duplicate runtime API {}.{}",
                        api.name, method.name
                    )));
                }
            }
        }
        let signing = if level == MetadataLevel::V15 && runtime.extrinsic.version != 4 {
            Err(CoreError::Policy(format!(
                "core signing requires extrinsic version 4, found {}",
                runtime.extrinsic.version
            )))
        } else if level == MetadataLevel::V15 {
            runtime
                .signature_payload_parts(&probe_params())
                .map(|_| ())
                .map_err(CoreError::from)
        } else {
            Err(CoreError::Policy(
                "V14 metadata does not support signing".into(),
            ))
        };
        Ok(Self(Arc::new(RuntimeEntry {
            runtime,
            level,
            outer_enums,
            pallet_indices,
            call_indices,
            storage_indices,
            api_indices,
            api_versions,
            shapes: Mutex::new(BTreeMap::new()),
            surface: OnceLock::new(),
            digest: OnceLock::new(),
            signing,
        })))
    }

    /// The upstream parsed runtime used for all encoding and storage-key construction.
    pub fn core(&self) -> &Runtime {
        &self.0.runtime
    }

    /// The immutable cache entry shared by clones of this handle.
    pub fn entry(&self) -> &RuntimeEntry {
        &self.0
    }

    /// Metadata capabilities of this runtime.
    pub fn metadata_level(&self) -> MetadataLevel {
        self.0.level
    }

    /// Runtime specification version.
    pub fn spec_version(&self) -> u32 {
        self.core().spec_version
    }

    /// Signed-transaction version.
    pub fn transaction_version(&self) -> u32 {
        self.core().transaction_version
    }

    /// Runtime and signed-transaction versions, in that order.
    pub fn stamp(&self) -> RuntimeStamp {
        RuntimeStamp {
            spec_version: self.spec_version(),
            transaction_version: self.transaction_version(),
        }
    }

    /// Whether the extension probe permits signing under this runtime.
    pub fn signing_status(&self) -> Result<(), CoreError> {
        self.0.signing.clone()
    }

    /// Resolve a pallet index by name from the live metadata.
    pub fn pallet_index(&self, pallet: &str) -> Option<u8> {
        self.0.pallet_indices.get(pallet).copied()
    }

    /// Resolve a call index by name from the live metadata.
    pub fn call_index(&self, pallet: &str, call: &str) -> Option<u8> {
        self.0
            .call_indices
            .get(&(pallet.into(), call.into()))
            .copied()
    }

    /// Resolve a storage entry from the live metadata; its hashers remain authoritative.
    pub fn storage_entry(&self, pallet: &str, name: &str) -> Option<&StorageInfo> {
        let &(pallet, storage) = self.0.storage_indices.get(&(pallet.into(), name.into()))?;
        self.core().pallets.get(pallet)?.storage.get(storage)
    }

    /// Resolve a V15 runtime API method by trait and method name.
    pub fn runtime_api(&self, api: &str, method: &str) -> Option<&RuntimeApiMethodInfo> {
        let &(api, method) = self.0.api_indices.get(&(api.into(), method.into()))?;
        self.core().apis.get(api)?.methods.get(method)
    }

    /// The advertised version of a runtime API trait, if the client supplied it.
    pub fn runtime_api_version(&self, api: &str) -> Option<u32> {
        self.0.api_versions.get(api).copied()
    }

    /// Check the generated used surface once for this runtime.
    pub fn surface_report(&self) -> &SurfaceReport {
        self.0.surface.get_or_init(|| verify_surface(self))
    }

    /// Compute and cache the RFC-0078 digest through the core.
    ///
    /// The chain information must describe this runtime. A subsequent call
    /// with different chain information is rejected rather than returning a
    /// digest computed from different constants.
    pub fn metadata_digest(&self, info: &ChainInfo) -> Result<[u8; 32], CoreError> {
        if self.metadata_level() != MetadataLevel::V15 {
            return Err(CoreError::Policy(
                "V14 metadata does not support signing".into(),
            ));
        }
        if info.spec_version != self.spec_version() || info.base58_prefix != self.core().ss58_format
        {
            return Err(CoreError::Policy(
                "metadata digest chain information does not match the runtime".into(),
            ));
        }
        let expected = ChainInfoKey::from(info);
        let (cached_info, digest) = self.0.digest.get_or_init(|| {
            (
                expected.clone(),
                rtao_core::digest::metadata_digest(&self.core().metadata_bytes, info)
                    .map_err(CoreError::from),
            )
        });
        if *cached_info != expected {
            return Err(CoreError::Policy(
                "metadata digest was already cached with different chain information".into(),
            ));
        }
        digest.clone()
    }

    fn shape_hasher(&self) -> ShapeHasher<'_> {
        match self.0.outer_enums {
            Some(outer) => ShapeHasher::new(&self.core().types, outer),
            None => ShapeHasher::without_outer_enums(&self.core().types),
        }
    }

    fn cached_shape(
        &self,
        key: ShapeKey,
        compute: impl FnOnce(&mut ShapeHasher<'_>) -> Result<ShapeHash, crate::shape::ShapeError>,
    ) -> Result<ShapeHash, CoreError> {
        let mut cache = self
            .0
            .shapes
            .lock()
            .map_err(|_| CoreError::Codec("runtime shape cache lock was poisoned".into()))?;
        if let Some(shape) = cache.get(&key) {
            return shape.clone();
        }
        let result =
            compute(&mut self.shape_hasher()).map_err(|error| CoreError::Codec(error.to_string()));
        cache.insert(key, result.clone());
        result
    }

    /// Compute an exact structural shape, caching it for this runtime.
    pub fn exact_shape(&self, id: u32) -> Result<ShapeHash, CoreError> {
        self.cached_shape(ShapeKey::Exact(id), |hasher| hasher.exact(id))
    }

    /// Compute a by-name model projection, caching it for this runtime.
    pub fn projection_shape(
        &self,
        id: u32,
        projection: &Projection,
    ) -> Result<ShapeHash, CoreError> {
        self.cached_shape(ShapeKey::Projection(id, projection.clone()), |hasher| {
            hasher.projection(id, projection)
        })
    }

    /// Hash an ordered metadata field list through the shared exact-shape cache.
    pub fn fields_shape(&self, fields: &[Field<PortableForm>]) -> Result<ShapeHash, CoreError> {
        let key = fields
            .iter()
            .map(|field| (field.name.clone(), field.ty.id))
            .collect();
        self.cached_shape(ShapeKey::Fields(key), |hasher| hasher.exact_fields(fields))
    }

    /// Hash ordered key or parameter types through the shared structural cache.
    pub fn tuple_shape(&self, types: &[u32]) -> Result<ShapeHash, CoreError> {
        self.cached_shape(ShapeKey::Tuple(types.to_vec()), |hasher| {
            hasher.exact_tuple(types)
        })
    }
}

/// Dummy parameters used only to check that every signed extension is encodable.
pub fn probe_params() -> TxParams {
    TxParams {
        era: Value::Str("00".into()),
        nonce: 0,
        tip: 0,
        tip_asset_id: None,
        genesis_hash: [0; 32],
        era_block_hash: [0; 32],
        metadata_hash: None,
    }
}

/// Versions that determine a call's signed transaction encoding.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RuntimeStamp {
    /// Runtime specification version.
    pub spec_version: u32,
    /// Signed-transaction version.
    pub transaction_version: u32,
}

/// A composed call carrying the runtime versions under which it was encoded.
///
/// Bytes are created only by the core encoder after descriptor verification.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallBytes {
    bytes: Bytes,
    spec_version: u32,
    transaction_version: u32,
    hash: [u8; 32],
}

impl CallBytes {
    pub(crate) fn from_encoded(bytes: Vec<u8>, runtime: &RuntimeHandle) -> Self {
        let hash = Blake2b::<U32>::digest(&bytes).into();
        Self {
            bytes: bytes.into(),
            spec_version: runtime.spec_version(),
            transaction_version: runtime.transaction_version(),
            hash,
        }
    }

    /// The complete SCALE call bytes, including live pallet and call indices.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The runtime specification version used for composition.
    pub fn spec_version(&self) -> u32 {
        self.spec_version
    }

    /// The signed-transaction version used for composition.
    pub fn transaction_version(&self) -> u32 {
        self.transaction_version
    }

    /// Runtime and signed-transaction versions, in that order.
    pub fn stamp(&self) -> RuntimeStamp {
        RuntimeStamp {
            spec_version: self.spec_version,
            transaction_version: self.transaction_version,
        }
    }

    /// BLAKE2b-256 of the complete call bytes.
    pub fn hash(&self) -> [u8; 32] {
        self.hash
    }

    /// Refuse bytes composed under a different specification or transaction version.
    pub fn ensure_runtime(&self, runtime: &RuntimeHandle) -> Result<(), ComposeError> {
        if self.stamp() == runtime.stamp() {
            Ok(())
        } else {
            Err(ComposeError::VersionMismatch {
                composed: self.stamp(),
                runtime: runtime.stamp(),
            })
        }
    }

    /// Convert to the core's nested-call value after checking both version stamps.
    pub fn into_nested_value(self, runtime: &RuntimeHandle) -> Result<Value, ComposeError> {
        self.ensure_runtime(runtime)?;
        Ok(Value::Bytes(self.bytes.to_vec()))
    }
}

impl AsRef<[u8]> for CallBytes {
    fn as_ref(&self) -> &[u8] {
        self.as_bytes()
    }
}

impl rtao_primitives::IntoCoreValue for CallBytes {
    fn into_core_value(self) -> Value {
        Value::Bytes(self.bytes.to_vec())
    }
}

/// A call could not be safely composed for the live runtime.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ComposeError {
    /// Signing and typed call composition require V15 metadata.
    MetadataDegraded,
    /// The live call is absent or its arguments differ from the compiled descriptor.
    SurfaceChanged {
        /// Pallet containing the call.
        pallet: String,
        /// Call name.
        call: String,
        /// Exact reason verification failed.
        detail: String,
    },
    /// An inner call or submission belongs to a different runtime stamp.
    VersionMismatch {
        /// Specification and transaction versions used for composition.
        composed: RuntimeStamp,
        /// Specification and transaction versions expected by the runtime.
        runtime: RuntimeStamp,
    },
    /// A signed extension has no safe encoding under this runtime.
    SigningDisabled(CoreError),
    /// The core rejected argument values or encoding.
    Core(CoreError),
}

impl fmt::Display for ComposeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MetadataDegraded => formatter.write_str("call composition requires V15 metadata"),
            Self::SurfaceChanged {
                pallet,
                call,
                detail,
            } => {
                write!(
                    formatter,
                    "call surface changed for {pallet}.{call}: {detail}"
                )
            }
            Self::VersionMismatch { composed, runtime } => write!(
                formatter,
                "call runtime stamp {composed:?} does not match {runtime:?}"
            ),
            Self::SigningDisabled(error) => write!(formatter, "signing is disabled: {error}"),
            Self::Core(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for ComposeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::SigningDisabled(error) | Self::Core(error) => Some(error),
            _ => None,
        }
    }
}

impl From<CoreError> for ComposeError {
    fn from(error: CoreError) -> Self {
        Self::Core(error)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use parity_scale_codec::Encode;

    const METADATA: &[u8] = include_bytes!("../../../vendor/metadata/finney/478.v15.scale");

    fn runtime(spec: u32, tx: u32) -> RuntimeHandle {
        RuntimeHandle::parse(METADATA, spec, tx, 42).unwrap()
    }

    fn info(spec: u32) -> ChainInfo {
        ChainInfo {
            spec_version: spec,
            spec_name: "node-subtensor".into(),
            base58_prefix: 42,
            decimals: 9,
            token_symbol: "TAO".into(),
        }
    }

    #[test]
    fn live_names_and_clones_share_one_runtime() {
        let runtime = runtime(478, 1);
        let clone = runtime.clone();
        assert!(Arc::ptr_eq(&runtime.0, &clone.0));
        assert_eq!(runtime.metadata_level(), MetadataLevel::V15);
        assert_eq!(
            runtime.pallet_index("System"),
            runtime.core().pallet("System").map(|pallet| pallet.index)
        );
        assert!(runtime.call_index("System", "remark").is_some());
        assert!(runtime.storage_entry("System", "Account").is_some());
        assert!(
            runtime
                .runtime_api("AccountNonceApi", "account_nonce")
                .is_some()
        );
        assert!(runtime.signing_status().is_ok());
    }

    #[test]
    fn parser_rejects_v16_and_trailing_bytes() {
        let v16 = include_bytes!("../../../vendor/metadata/finney/478.v16.scale");
        assert!(RuntimeHandle::parse(v16, 478, 1, 42).is_err());
        let mut trailing = METADATA.to_vec();
        trailing.push(0);
        let error = RuntimeHandle::parse(&trailing, 478, 1, 42).unwrap_err();
        assert!(error.to_string().contains("trailing bytes"));
        let mut bad_magic = METADATA.to_vec();
        *bad_magic.first_mut().unwrap() = 0;
        assert!(
            RuntimeHandle::parse(&bad_magic, 478, 1, 42)
                .unwrap_err()
                .to_string()
                .contains("magic prefix")
        );
    }

    #[test]
    fn digest_matches_core_and_rejects_conflicting_chain_information() {
        let runtime = runtime(478, 1);
        let expected = rtao_core::digest::metadata_digest(METADATA, &info(478)).unwrap();
        assert_eq!(runtime.metadata_digest(&info(478)).unwrap(), expected);
        assert_eq!(
            runtime.clone().metadata_digest(&info(478)).unwrap(),
            expected
        );
        assert!(runtime.metadata_digest(&info(479)).is_err());
        let mut conflicting = info(478);
        conflicting.token_symbol = "OTHER".into();
        assert!(runtime.metadata_digest(&conflicting).is_err());
        let mut wrong_prefix = info(478);
        wrong_prefix.base58_prefix = 0;
        assert!(runtime.metadata_digest(&wrong_prefix).is_err());
    }

    #[test]
    fn both_call_stamp_versions_are_checked_before_nesting() {
        let original = runtime(478, 1);
        let bytes = CallBytes::from_encoded(vec![1, 2, 3], &original);
        assert!(bytes.ensure_runtime(&original).is_ok());
        assert!(bytes.ensure_runtime(&runtime(479, 1)).is_err());
        assert!(bytes.ensure_runtime(&runtime(478, 2)).is_err());
        assert!(bytes.clone().into_nested_value(&runtime(478, 2)).is_err());
        assert_eq!(
            bytes.into_nested_value(&original).unwrap(),
            Value::Bytes(vec![1, 2, 3])
        );
    }

    #[test]
    fn unknown_extension_payload_disables_signing_but_preserves_reads() {
        let mut core = Runtime::parse(METADATA, 478, 1, 42).unwrap();
        let nonce = core
            .extrinsic
            .signed_extensions
            .iter_mut()
            .find(|extension| extension.identifier == "CheckNonce")
            .unwrap();
        nonce.identifier = "UnknownNonce".into();
        let runtime = RuntimeHandle::new(core).unwrap();
        let error = runtime.signing_status().unwrap_err();
        assert!(error.to_string().contains("UnknownNonce"));
        assert!(runtime.storage_entry("System", "Account").is_some());
    }

    #[test]
    fn new_zero_sized_extension_does_not_disable_signing() {
        let mut core = Runtime::parse(METADATA, 478, 1, 42).unwrap();
        let unit = core
            .types
            .types
            .iter()
            .find(|ty| match &ty.ty.type_def {
                scale_info::TypeDef::Tuple(tuple) => tuple.fields.is_empty(),
                scale_info::TypeDef::Composite(composite) => composite.fields.is_empty(),
                _ => false,
            })
            .unwrap()
            .id;
        core.extrinsic
            .signed_extensions
            .push(rtao_core::runtime::SignedExtensionInfo {
                identifier: "NewEmptyExtension".into(),
                ty: unit,
                additional_signed: unit,
            });
        assert!(RuntimeHandle::new(core).unwrap().signing_status().is_ok());
    }

    #[test]
    fn unsupported_extrinsic_version_disables_signing_early() {
        let mut core = Runtime::parse(METADATA, 478, 1, 42).unwrap();
        core.extrinsic.version = 5;
        let runtime = RuntimeHandle::new(core).unwrap();
        assert!(
            runtime
                .signing_status()
                .unwrap_err()
                .to_string()
                .contains("version 4")
        );
        assert!(runtime.storage_entry("System", "Account").is_some());
    }

    #[test]
    fn supplied_api_versions_must_name_live_traits() {
        let mut versions = BTreeMap::new();
        versions.insert("AccountNonceApi".into(), 1);
        let runtime = RuntimeHandle::new_with_api_versions(
            Runtime::parse(METADATA, 478, 1, 42).unwrap(),
            versions,
        )
        .unwrap();
        assert_eq!(runtime.runtime_api_version("AccountNonceApi"), Some(1));
        assert_eq!(runtime.runtime_api_version("TransactionPaymentApi"), None);
        let mut unknown = BTreeMap::new();
        unknown.insert("UnknownApi".into(), 1);
        assert!(matches!(
            RuntimeHandle::new_with_api_versions(
                Runtime::parse(METADATA, 478, 1, 42).unwrap(),
                unknown,
            ),
            Err(CoreError::NotInRuntime(_))
        ));
    }

    #[test]
    fn duplicate_indices_and_names_fail_closed() {
        let mut core = Runtime::parse(METADATA, 478, 1, 42).unwrap();
        let index = core.pallets.first().unwrap().index;
        core.pallets.get_mut(1).unwrap().index = index;
        assert!(
            RuntimeHandle::new(core)
                .unwrap_err()
                .to_string()
                .contains("duplicate pallet index")
        );
        let mut core = Runtime::parse(METADATA, 478, 1, 42).unwrap();
        let name = core.pallets.first().unwrap().name.clone();
        core.pallets.get_mut(1).unwrap().name = name;
        assert!(
            RuntimeHandle::new(core)
                .unwrap_err()
                .to_string()
                .contains("duplicate pallet")
        );
        let mut core = Runtime::parse(METADATA, 478, 1, 42).unwrap();
        let mut duplicate_api = core.apis.first().unwrap().clone();
        duplicate_api.methods.clear();
        core.apis.push(duplicate_api);
        assert!(
            RuntimeHandle::new(core)
                .unwrap_err()
                .to_string()
                .contains("duplicate runtime API")
        );
    }

    #[test]
    fn v14_is_readable_but_signing_and_digest_are_refused() {
        let prefixed = RuntimeMetadataPrefixed::decode(&mut &METADATA[..]).unwrap();
        let RuntimeMetadata::V15(v15) = prefixed.1 else {
            unreachable!()
        };
        let extrinsic_ty = v15
            .types
            .types
            .iter()
            .find(|ty| {
                ty.ty
                    .path
                    .segments
                    .last()
                    .is_some_and(|name| name == "UncheckedExtrinsic")
            })
            .map(|ty| ty.id)
            .unwrap_or(v15.ty.id);
        let v14 = frame_metadata::v14::RuntimeMetadataV14 {
            types: v15.types,
            pallets: v15
                .pallets
                .into_iter()
                .map(|pallet| frame_metadata::v14::PalletMetadata {
                    name: pallet.name,
                    storage: pallet.storage,
                    calls: pallet.calls,
                    event: pallet.event,
                    constants: pallet.constants,
                    error: pallet.error,
                    index: pallet.index,
                })
                .collect(),
            extrinsic: frame_metadata::v14::ExtrinsicMetadata {
                ty: extrinsic_ty.into(),
                version: v15.extrinsic.version,
                signed_extensions: v15
                    .extrinsic
                    .signed_extensions
                    .into_iter()
                    .map(|extension| frame_metadata::v14::SignedExtensionMetadata {
                        identifier: extension.identifier,
                        ty: extension.ty,
                        additional_signed: extension.additional_signed,
                    })
                    .collect(),
            },
            ty: v15.ty,
        };
        let bytes =
            RuntimeMetadataPrefixed(frame_metadata::META_RESERVED, RuntimeMetadata::V14(v14))
                .encode();
        let runtime = RuntimeHandle::parse(&bytes, 400, 1, 42).unwrap();
        assert_eq!(runtime.metadata_level(), MetadataLevel::V14Degraded);
        assert!(runtime.storage_entry("System", "Account").is_some());
        assert!(runtime.signing_status().is_err());
        assert!(runtime.metadata_digest(&info(400)).is_err());
        assert!(
            runtime
                .runtime_api("AccountNonceApi", "account_nonce")
                .is_none()
        );
    }
}
