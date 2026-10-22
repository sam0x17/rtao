// Derived from RaoFoundation/subtensor sdk/bittensor-core/src/codec/decode.rs @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/_transport/contract.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/_transport/codec.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
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
    /// The chain information must describe this runtime. One complete display
    /// context is cached; different signer display constants are computed
    /// independently without reusing or replacing that cached digest.
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
            return rtao_core::digest::metadata_digest(&self.core().metadata_bytes, info)
                .map_err(CoreError::from);
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
/// Bytes are created by the core encoder after descriptor verification, or by
/// strict complete import with metadata-directed core decoding and re-encoding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallBytes {
    bytes: Bytes,
    spec_version: u32,
    transaction_version: u32,
    hash: [u8; 32],
}

impl CallBytes {
    /// Import a complete SCALE call under this runtime's signing contract.
    ///
    /// Decoding consumes all bytes. Core re-encoding must reproduce them exactly,
    /// including every nested call, before either runtime stamp is attached.
    pub fn decode(bytes: &[u8], runtime: &RuntimeHandle) -> Result<Self, ComposeError> {
        if runtime.metadata_level() != MetadataLevel::V15 {
            return Err(ComposeError::MetadataDegraded);
        }
        runtime
            .signing_status()
            .map_err(ComposeError::SigningDisabled)?;
        let spec = rtao_core::runtime::TypeSpec::Call;
        runtime
            .core()
            .decode_spec(&spec, bytes, true)
            .map_err(CoreError::from)?;
        let call_type = runtime
            .core()
            .extrinsic
            .call_type
            .ok_or_else(|| CoreError::Codec("runtime has no outer call type".into()))?;
        let mut cursor = rtao_core::codec::decode::Cursor::new(bytes);
        cursor.strict = true;
        let mut budget = CallBridgeBudget::new(bytes.len());
        let value = canonical_scale_value(runtime.core(), call_type, &mut cursor, &mut budget)?;
        if cursor.remaining() != 0 {
            return Err(call_bridge_error().into());
        }
        let encoded = call_import_encoder(runtime.core())?
            .encode_spec(&spec, &value)
            .map_err(CoreError::from)?;
        if encoded != bytes {
            return Err(CoreError::Codec("imported call is not canonically encoded".into()).into());
        }
        Ok(Self::from_encoded(encoded, runtime))
    }

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

// The first strict core decode validates the complete call. This independently
// bounded metadata-directed second pass preserves byte spans whose
// inspection strings are ambiguous (literal UTF-8 "0x41" versus hex bytes).
// Numeric, compact and special scalar decoding remains in the core. Only typed
// byte leaves become raw Bytes; whole calls always become encoder enum inputs.
struct CallBridgeBudget {
    depth: usize,
    elements_remaining: u64,
}
impl CallBridgeBudget {
    fn new(input_bytes: usize) -> Self {
        Self {
            depth: 0,
            // Match the core's collection budget, without resetting it for
            // special scalar carriers decoded through fresh local cursors.
            elements_remaining: u64::try_from(input_bytes)
                .unwrap_or(u64::MAX)
                .saturating_mul(256)
                .max(1 << 20),
        }
    }
    fn enter(&mut self) -> Result<(), CoreError> {
        if self.depth >= 256 {
            return Err(CoreError::Codec(
                "call import recursion exceeds 256 levels".into(),
            ));
        }
        self.depth += 1;
        Ok(())
    }
    fn charge(&mut self, elements: usize) -> Result<(), CoreError> {
        self.elements_remaining = self
            .elements_remaining
            .checked_sub(u64::try_from(elements).map_err(|_| call_bridge_error())?)
            .ok_or_else(|| CoreError::Codec("call import exceeds its element budget".into()))?;
        Ok(())
    }
}

// Core's ordinary Option input carrier collapses Some(None) into None. A
// private encoding view changes only the presentation path that selects that
// coercion. Variant names, indices, fields and type IDs remain unchanged, so
// the core's ordinary enum encoder can represent both cases exactly. This
// view never participates in runtime identity, metadata digests or signing.
fn call_import_encoder(core: &Runtime) -> Result<Runtime, CoreError> {
    let mut encoder = Runtime::parse(
        &core.metadata_bytes,
        core.spec_version,
        core.transaction_version,
        core.ss58_format,
    )
    .map_err(CoreError::from)?;
    // Preserve RuntimeHandle::new's public core inputs as well as parsed runtimes.
    encoder.types = core.types.clone();
    encoder.extrinsic = core.extrinsic.clone();
    for ty in &mut encoder.types.types {
        if ty.ty.path.segments.as_slice() == ["Option"] {
            ty.ty.path.segments.insert(0, "rtao_call_import".into());
        }
    }
    Ok(encoder)
}

fn canonical_scale_value(
    core: &Runtime,
    id: u32,
    cursor: &mut rtao_core::codec::decode::Cursor<'_>,
    budget: &mut CallBridgeBudget,
) -> Result<Value, CoreError> {
    budget.enter()?;
    let result = canonical_scale_value_inner(core, id, cursor, budget);
    budget.depth -= 1;
    result
}
fn canonical_scale_value_inner(
    core: &Runtime,
    id: u32,
    cursor: &mut rtao_core::codec::decode::Cursor<'_>,
    budget: &mut CallBridgeBudget,
) -> Result<Value, CoreError> {
    if core.extrinsic.call_type == Some(id) {
        return canonical_scale_call(core, cursor, budget);
    }
    let ty = core.resolve(id).map_err(CoreError::from)?;
    let last = ty.path.segments.last().map(String::as_str);
    if ty
        .path
        .segments
        .iter()
        .map(String::as_str)
        .eq(["sp_core", "crypto", "AccountId32"])
    {
        return core.decode_id(id, cursor).map_err(CoreError::from);
    }
    if last == Some("Era") && ty.path.segments.first().map(String::as_str) == Some("sp_runtime") {
        return match core.decode_id(id, cursor).map_err(CoreError::from)? {
            Value::Str(era) if era == "00" => Ok(Value::Str(era)),
            Value::Tuple(fields) if fields.len() == 2 => Ok(Value::record(vec![
                ("period".into(), fields[0].clone()),
                ("phase".into(), fields[1].clone()),
            ])),
            _ => Err(call_bridge_error()),
        };
    }
    if ty
        .path
        .segments
        .iter()
        .map(String::as_str)
        .eq(["primitive_types", "U256"])
        && let scale_info::TypeDef::Composite(composite) = &ty.type_def
    {
        let Value::U256(le) = core.decode_id(id, cursor).map_err(CoreError::from)? else {
            return Err(call_bridge_error());
        };
        let [field] = composite.fields.as_slice() else {
            return Err(call_bridge_error());
        };
        let spec = rtao_core::runtime::TypeSpec::Id(field.ty.id);
        // Validate this special decoder carrier against its declared limb
        // layout through the core, including its existing recursion guard.
        core.decode_spec(&spec, &le, true)
            .map_err(CoreError::from)?;
        let mut limbs = rtao_core::codec::decode::Cursor::new(&le);
        limbs.strict = true;
        let value = canonical_scale_fields(core, &composite.fields, &mut limbs, budget)?;
        if limbs.remaining() != 0 {
            return Err(call_bridge_error());
        }
        return Ok(value);
    }
    if last == Some("BTreeMap") {
        let scale_info::TypeDef::Composite(composite) = &ty.type_def else {
            return Err(call_bridge_error());
        };
        let field = composite.fields.first().ok_or_else(call_bridge_error)?;
        let scale_info::TypeDef::Sequence(sequence) =
            &core.resolve(field.ty.id).map_err(CoreError::from)?.type_def
        else {
            return Err(call_bridge_error());
        };
        let scale_info::TypeDef::Tuple(tuple) = &core
            .resolve(sequence.type_param.id)
            .map_err(CoreError::from)?
            .type_def
        else {
            return Err(call_bridge_error());
        };
        let [key, value] = tuple.fields.as_slice() else {
            return Err(call_bridge_error());
        };
        let count = rtao_core::codec::decode::compact_len(cursor).map_err(CoreError::from)?;
        budget.charge(count)?;
        let mut entries = Vec::with_capacity(count.min(4096));
        for _ in 0..count {
            entries.push((
                canonical_scale_value(core, key.id, cursor, budget)?,
                canonical_scale_value(core, value.id, cursor, budget)?,
            ));
        }
        return Ok(Value::Dict(entries));
    }
    match &ty.type_def {
        scale_info::TypeDef::Composite(composite) => {
            canonical_scale_fields(core, &composite.fields, cursor, budget)
        }
        scale_info::TypeDef::Variant(variants) => {
            let index = cursor.byte().map_err(CoreError::from)?;
            let chosen = variants
                .variants
                .iter()
                .find(|v| v.index == index)
                .ok_or_else(call_bridge_error)?;
            let payload = canonical_scale_fields(core, &chosen.fields, cursor, budget)?;
            if chosen.fields.is_empty() {
                Ok(Value::Str(chosen.name.clone()))
            } else {
                Ok(Value::record(vec![(chosen.name.clone(), payload)]))
            }
        }
        scale_info::TypeDef::Sequence(sequence) => {
            let count = rtao_core::codec::decode::compact_len(cursor).map_err(CoreError::from)?;
            canonical_scale_sequence(core, sequence.type_param.id, count, cursor, budget)
        }
        scale_info::TypeDef::Array(array) => canonical_scale_sequence(
            core,
            array.type_param.id,
            array.len as usize,
            cursor,
            budget,
        ),
        scale_info::TypeDef::Tuple(tuple) => tuple
            .fields
            .iter()
            .map(|field| canonical_scale_value(core, field.id, cursor, budget))
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Tuple),
        _ => core.decode_id(id, cursor).map_err(CoreError::from),
    }
}
fn canonical_scale_sequence(
    core: &Runtime,
    item_type: u32,
    count: usize,
    cursor: &mut rtao_core::codec::decode::Cursor<'_>,
    budget: &mut CallBridgeBudget,
) -> Result<Value, CoreError> {
    if matches!(
        &core.resolve(item_type).map_err(CoreError::from)?.type_def,
        scale_info::TypeDef::Primitive(scale_info::TypeDefPrimitive::U8)
    ) {
        return cursor
            .take(count)
            .map(|bytes| Value::Bytes(bytes.to_vec()))
            .map_err(CoreError::from);
    }
    budget.charge(count)?;
    let mut values = Vec::with_capacity(count.min(4096));
    for _ in 0..count {
        values.push(canonical_scale_value(core, item_type, cursor, budget)?);
    }
    Ok(Value::List(values))
}
fn canonical_scale_fields(
    core: &Runtime,
    fields: &[Field<PortableForm>],
    cursor: &mut rtao_core::codec::decode::Cursor<'_>,
    budget: &mut CallBridgeBudget,
) -> Result<Value, CoreError> {
    if fields.is_empty() {
        return Ok(Value::Dict(Vec::new()));
    }
    if fields.iter().all(|f| f.name.is_some()) {
        return fields
            .iter()
            .map(|field| {
                Ok((
                    field.name.clone().ok_or_else(call_bridge_error)?,
                    canonical_scale_value(core, field.ty.id, cursor, budget)?,
                ))
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Value::record);
    }
    if let [field] = fields {
        let value = canonical_scale_value(core, field.ty.id, cursor, budget)?;
        // Core's legacy single-field sequence encoder removes one list level.
        // Supply that level explicitly so a one-element nested sequence keeps
        // its cardinality. Byte leaves use Bytes and bypass this list coercion.
        if matches!(value, Value::List(_))
            && matches!(
                &core.resolve(field.ty.id).map_err(CoreError::from)?.type_def,
                scale_info::TypeDef::Sequence(_)
            )
        {
            return Ok(Value::List(vec![value]));
        }
        return Ok(value);
    }
    fields
        .iter()
        .map(|field| canonical_scale_value(core, field.ty.id, cursor, budget))
        .collect::<Result<Vec<_>, _>>()
        .map(Value::Tuple)
}
fn canonical_scale_call(
    core: &Runtime,
    cursor: &mut rtao_core::codec::decode::Cursor<'_>,
    budget: &mut CallBridgeBudget,
) -> Result<Value, CoreError> {
    let pallet_index = cursor.byte().map_err(CoreError::from)?;
    let call_index = cursor.byte().map_err(CoreError::from)?;
    let pallet = core.pallet_at(pallet_index).ok_or_else(call_bridge_error)?;
    let call_type = pallet.calls_type.ok_or_else(call_bridge_error)?;
    let scale_info::TypeDef::Variant(calls) =
        &core.resolve(call_type).map_err(CoreError::from)?.type_def
    else {
        return Err(call_bridge_error());
    };
    let call = calls
        .variants
        .iter()
        .find(|v| v.index == call_index)
        .ok_or_else(call_bridge_error)?;
    let fields = canonical_scale_fields(core, &call.fields, cursor, budget)?;
    let outer = core.extrinsic.call_type.ok_or_else(call_bridge_error)?;
    let scale_info::TypeDef::Variant(variants) =
        &core.resolve(outer).map_err(CoreError::from)?.type_def
    else {
        return Err(call_bridge_error());
    };
    let outer_name = &variants
        .variants
        .iter()
        .find(|v| v.index == pallet.index)
        .ok_or_else(call_bridge_error)?
        .name;
    Ok(Value::record(vec![(
        outer_name.clone(),
        Value::record(vec![(call.name.clone(), fields)]),
    )]))
}
fn call_bridge_error() -> CoreError {
    CoreError::Codec("decoded call does not match its authenticated metadata type".into())
}

#[cfg(test)]
#[path = "call_import_tests.rs"]
mod call_import_tests;

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
        let mut alternate = info(478);
        alternate.token_symbol = "OTHER".into();
        alternate.decimals = 12;
        let alternate_expected = rtao_core::digest::metadata_digest(METADATA, &alternate).unwrap();
        assert_ne!(alternate_expected, expected);
        assert_eq!(
            runtime.metadata_digest(&alternate).unwrap(),
            alternate_expected
        );
        assert_eq!(
            runtime.clone().metadata_digest(&info(478)).unwrap(),
            expected
        );
        assert_eq!(
            runtime.metadata_digest(&alternate).unwrap(),
            alternate_expected
        );
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
        assert!(matches!(
            CallBytes::decode(&[0, 0, 0], &runtime),
            Err(ComposeError::SigningDisabled(_))
        ));
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
        assert!(matches!(
            CallBytes::decode(&[0, 0, 0], &runtime),
            Err(ComposeError::MetadataDegraded)
        ));
        assert!(runtime.storage_entry("System", "Account").is_some());
        assert!(runtime.signing_status().is_err());
        assert!(runtime.metadata_digest(&info(400)).is_err());
        assert!(
            runtime
                .runtime_api("AccountNonceApi", "account_nonce")
                .is_none()
        );
    }

    #[test]
    fn imported_calls_require_complete_canonical_core_roundtrips() {
        // Proxy calls carry the pinned ProxyType, which spec 482 extended.
        let runtime = RuntimeHandle::parse(
            include_bytes!("../../../vendor/metadata/finney/482.v15.scale"),
            482,
            1,
            42,
        )
        .unwrap();
        let inner = crate::calls::system::remark(vec![1, 2, 3])
            .compose(&runtime)
            .unwrap();
        let nested = crate::calls::sudo::sudo(inner.clone())
            .compose(&runtime)
            .unwrap();
        let batch = crate::calls::utility::batch_all(vec![inner.clone(), nested.clone()])
            .compose(&runtime)
            .unwrap();
        let proxy = crate::calls::proxy::proxy(
            rtao_core::DynValue::new(Value::Bytes(vec![7; 32])),
            Some(crate::models::ProxyType::Any),
            nested.clone(),
        )
        .compose(&runtime)
        .unwrap();
        for original in [inner.clone(), nested, batch, proxy] {
            assert_eq!(
                CallBytes::decode(original.as_bytes(), &runtime).unwrap(),
                original
            );
            let mut trailing = original.as_bytes().to_vec();
            trailing.push(0);
            assert!(CallBytes::decode(&trailing, &runtime).is_err());
            assert!(
                CallBytes::decode(
                    &original.as_bytes()[..original.as_bytes().len() - 1],
                    &runtime
                )
                .is_err()
            );
        }
        for malformed in [&[][..], &[255, 255][..], &[0][..]] {
            assert!(CallBytes::decode(malformed, &runtime).is_err());
        }
        // Replace the remark Vec length with an overlong compact encoding of 3.
        let mut noncanonical = inner.as_bytes()[..2].to_vec();
        noncanonical.extend_from_slice(&[13, 0, 1, 2, 3]);
        assert!(CallBytes::decode(&noncanonical, &runtime).is_err());
    }

    #[test]
    fn imported_nested_calls_reach_the_existing_core_depth_guard() {
        let runtime = RuntimeHandle::parse(METADATA, 478, 1, 42).unwrap();
        let mut nested = crate::calls::system::remark(vec![])
            .compose(&runtime)
            .unwrap();
        let mut boundary = None;
        for depth in 1..=257 {
            nested = crate::calls::sudo::sudo(nested).compose(&runtime).unwrap();
            match CallBytes::decode(nested.as_bytes(), &runtime) {
                Ok(imported) => assert_eq!(imported, nested),
                Err(error) => {
                    assert!(error.to_string().contains("recursion exceeds"));
                    boundary = Some(depth);
                    break;
                }
            }
        }
        assert!(boundary.is_some_and(|depth| depth > 1));
    }

    #[test]
    fn decoded_era_and_multiaddress_special_values_roundtrip_through_core() {
        let runtime = RuntimeHandle::parse(METADATA, 478, 1, 42).unwrap();
        let core = runtime.core();
        let era_type = core
            .types
            .types
            .iter()
            .find(|ty| {
                ty.ty
                    .path
                    .segments
                    .first()
                    .is_some_and(|s| s == "sp_runtime")
                    && ty.ty.path.segments.last().is_some_and(|s| s == "Era")
            })
            .unwrap()
            .id;
        for era in [
            Value::Str("00".into()),
            Value::record(vec![
                ("period".into(), Value::Uint(128)),
                ("phase".into(), Value::Uint(100)),
            ]),
        ] {
            let spec = rtao_core::runtime::TypeSpec::Id(era_type);
            let encoded = core.encode_spec(&spec, &era).unwrap();
            core.decode_spec(&spec, &encoded, true).unwrap();
            let mut cursor = rtao_core::codec::decode::Cursor::new(&encoded);
            cursor.strict = true;
            let canonical = canonical_scale_value(
                core,
                era_type,
                &mut cursor,
                &mut CallBridgeBudget::new(encoded.len()),
            )
            .unwrap();
            assert_eq!(cursor.remaining(), 0);
            assert_eq!(core.encode_spec(&spec, &canonical).unwrap(), encoded);
        }
        let call = core
            .compose_call(
                "Balances",
                "transfer_keep_alive",
                &Value::record(vec![
                    (
                        "dest".into(),
                        Value::record(vec![("Index".into(), Value::Uint(3))]),
                    ),
                    ("value".into(), Value::Uint(1)),
                ]),
            )
            .unwrap();
        assert_eq!(CallBytes::decode(&call, &runtime).unwrap().as_bytes(), call);
    }

    #[test]
    fn decoded_u256_carrier_roundtrips_the_pinned_metadata_limb_shape() {
        let runtime = RuntimeHandle::parse(METADATA, 478, 1, 42).unwrap();
        let core = runtime.core();
        let id = core
            .types
            .types
            .iter()
            .find(|ty| {
                ty.ty
                    .path
                    .segments
                    .iter()
                    .map(String::as_str)
                    .eq(["primitive_types", "U256"])
            })
            .unwrap()
            .id;
        for le in [[0; 32], [7; 32], [255; 32]] {
            let encoded = core
                .encode_spec(
                    &rtao_core::runtime::TypeSpec::Primitive(rtao_core::runtime::Primitive::U256),
                    &Value::U256(le),
                )
                .unwrap();
            core.decode_spec(&rtao_core::runtime::TypeSpec::Id(id), &encoded, true)
                .unwrap();
            let mut cursor = rtao_core::codec::decode::Cursor::new(&encoded);
            cursor.strict = true;
            let canonical = canonical_scale_value(
                core,
                id,
                &mut cursor,
                &mut CallBridgeBudget::new(encoded.len()),
            )
            .unwrap();
            assert_eq!(cursor.remaining(), 0);
            assert_eq!(
                core.encode_spec(&rtao_core::runtime::TypeSpec::Id(id), &canonical)
                    .unwrap(),
                encoded
            );
        }
    }

    #[test]
    fn ordinary_call_shaped_argument_structs_keep_their_declared_fields() {
        #[derive(scale_info::TypeInfo)]
        #[allow(dead_code)]
        struct CallShapedData {
            call_index: String,
            call_function: String,
            call_module: String,
            call_args: Vec<u8>,
            call_hash: [u8; 32],
        }
        let mut registry = scale_info::Registry::new();
        let id = registry
            .register_type(&scale_info::meta_type::<CallShapedData>())
            .id;
        let mut core = Runtime::parse(METADATA, 478, 1, 42).unwrap();
        core.types = registry.into();
        core.extrinsic.call_type = None;
        core.outer_event_type = None;
        let value = Value::record(vec![
            ("call_index".into(), Value::Str("ordinary index".into())),
            (
                "call_function".into(),
                Value::Str("ordinary function".into()),
            ),
            ("call_module".into(), Value::Str("ordinary module".into())),
            ("call_args".into(), Value::Bytes(b"0x41".to_vec())),
            ("call_hash".into(), Value::Bytes(vec![7; 32])),
        ]);
        let spec = rtao_core::runtime::TypeSpec::Id(id);
        let encoded = core.encode_spec(&spec, &value).unwrap();
        core.decode_spec(&spec, &encoded, true).unwrap();
        let mut cursor = rtao_core::codec::decode::Cursor::new(&encoded);
        cursor.strict = true;
        let canonical = canonical_scale_value(
            &core,
            id,
            &mut cursor,
            &mut CallBridgeBudget::new(encoded.len()),
        )
        .unwrap();
        assert_eq!(cursor.remaining(), 0);
        assert_eq!(canonical, value);
        assert_eq!(core.encode_spec(&spec, &canonical).unwrap(), encoded);
    }

    #[test]
    fn literal_hex_text_and_non_utf8_byte_leaves_keep_their_original_spans() {
        let runtime = RuntimeHandle::parse(METADATA, 478, 1, 42).unwrap();
        for payload in [
            b"0x41".to_vec(),
            b"0x".to_vec(),
            b"0x1".to_vec(),
            b"0x0xff".to_vec(),
            b"ordinary UTF-8".to_vec(),
            vec![0xff, 0xfe, 0x80],
        ] {
            let call = crate::calls::system::remark(payload)
                .compose(&runtime)
                .unwrap();
            let nested = crate::calls::sudo::sudo(call.clone())
                .compose(&runtime)
                .unwrap();
            for original in [call, nested] {
                assert_eq!(
                    CallBytes::decode(original.as_bytes(), &runtime).unwrap(),
                    original
                );
            }
        }
        #[derive(scale_info::TypeInfo)]
        #[allow(dead_code)]
        struct NestedBytes(Vec<Vec<u8>>);
        let mut registry = scale_info::Registry::new();
        let id = registry
            .register_type(&scale_info::meta_type::<NestedBytes>())
            .id;
        let mut core = Runtime::parse(METADATA, 478, 1, 42).unwrap();
        core.types = registry.into();
        core.extrinsic.call_type = None;
        core.outer_event_type = None;
        let value = Value::List(vec![
            Value::Bytes(b"0x41".to_vec()),
            Value::Bytes(vec![255]),
            Value::Bytes(b"0x".to_vec()),
        ]);
        let spec = rtao_core::runtime::TypeSpec::Id(id);
        let encoded = core.encode_spec(&spec, &value).unwrap();
        core.decode_spec(&spec, &encoded, true).unwrap();
        let mut cursor = rtao_core::codec::decode::Cursor::new(&encoded);
        cursor.strict = true;
        let canonical = canonical_scale_value(
            &core,
            id,
            &mut cursor,
            &mut CallBridgeBudget::new(encoded.len()),
        )
        .unwrap();
        assert_eq!(cursor.remaining(), 0);
        assert_eq!(core.encode_spec(&spec, &canonical).unwrap(), encoded);
    }

    #[test]
    fn evm_some_full_width_u256_fields_and_literal_input_roundtrip() {
        let runtime = RuntimeHandle::parse(METADATA, 478, 1, 42).unwrap();
        let limbs = || {
            rtao_core::DynValue::new(Value::List(vec![
                Value::Uint(u64::MAX.into()),
                Value::Uint(7),
                Value::Uint(9),
                Value::Uint(11),
            ]))
        };
        let call = crate::calls::evm::call(
            rtao_core::DynValue::new(Value::Bytes(vec![7; 20])),
            rtao_core::DynValue::new(Value::Bytes(vec![8; 20])),
            b"0x41".to_vec(),
            limbs(),
            21000,
            limbs(),
            Some(limbs()),
            Some(limbs()),
            Vec::new(),
            Vec::new(),
        )
        .compose(&runtime)
        .unwrap();
        assert_eq!(CallBytes::decode(call.as_bytes(), &runtime).unwrap(), call);
    }
}
