//! Load the pinned upstream golden vectors and SCALE shape corpus.
//!
//! Loaders also accept outputs from the upstream recorders, so a new runtime
//! can be replayed without changing fixture formats or consulting a node.

use std::{collections::BTreeMap, error::Error, fmt, fs, path::Path};

use rtao_core::{
    CoreError,
    codec::decode::{Cursor, compact_len},
    runtime::Runtime,
};
use serde::{Deserialize, Deserializer};
use serde_json::Value;

/// Public native sample scenarios shared by SDK and CLI table fixtures.
pub mod catalog_samples;

/// A failed fixture read, parse, or metadata decode.
#[derive(Debug)]
pub enum FixtureError {
    /// The fixture file could not be read.
    Io(std::io::Error),
    /// The JSON does not match the upstream fixture format.
    Json(serde_json::Error),
    /// Metadata or SCALE decoding failed in the core.
    Core(CoreError),
    /// A fixture field violates its documented framing or shape.
    Invalid(String),
}

impl fmt::Display for FixtureError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => error.fmt(formatter),
            Self::Json(error) => error.fmt(formatter),
            Self::Core(error) => error.fmt(formatter),
            Self::Invalid(message) => formatter.write_str(message),
        }
    }
}

impl Error for FixtureError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Json(error) => Some(error),
            Self::Core(error) => Some(error),
            Self::Invalid(_) => None,
        }
    }
}

impl From<std::io::Error> for FixtureError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}
impl From<serde_json::Error> for FixtureError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}
impl From<rtao_core::codec::Error> for FixtureError {
    fn from(error: rtao_core::codec::Error) -> Self {
        Self::Core(error.into())
    }
}

/// Decode a recorder's hex field, requiring the `0x` prefix.
pub fn decode_hex(encoded: &str) -> Result<Vec<u8>, FixtureError> {
    let digits = encoded.strip_prefix("0x").ok_or_else(|| {
        FixtureError::Invalid("fixture hex field is missing its 0x prefix".into())
    })?;
    hex::decode(digits)
        .map_err(|error| FixtureError::Invalid(format!("invalid fixture hex: {error}")))
}

/// Decode a fixed-width recorder field without silently padding or truncating.
pub fn decode_fixed_hex<const N: usize>(encoded: &str) -> Result<[u8; N], FixtureError> {
    let bytes = decode_hex(encoded)?;
    let length = bytes.len();
    bytes.try_into().map_err(|_| {
        FixtureError::Invalid(format!(
            "fixture hex field has {length} bytes, expected {N}"
        ))
    })
}

/// Remove the SCALE `Option<OpaqueMetadata>` frame returned by `state_call`.
///
/// Raw `MetadataVersioned` from `state_getMetadata` is accepted too. An absent
/// option, noncanonical length, truncation, or trailing bytes is rejected.
pub fn metadata_bytes(encoded: &str) -> Result<Vec<u8>, FixtureError> {
    let bytes = decode_hex(encoded)?;
    if bytes.starts_with(b"meta") {
        return Ok(bytes);
    }
    let mut cursor = Cursor::new(&bytes);
    cursor.strict = true;
    if cursor.byte()? != 1 {
        return Err(FixtureError::Invalid(
            "metadata Option must contain Some".into(),
        ));
    }
    let length = compact_len(&mut cursor)?;
    let metadata = cursor.take(length)?.to_vec();
    if cursor.remaining() != 0 || !metadata.starts_with(b"meta") {
        return Err(FixtureError::Invalid(
            "invalid Option<OpaqueMetadata> framing".into(),
        ));
    }
    Ok(metadata)
}

/// Network identity captured with the golden metadata.
#[derive(Debug, Clone, Deserialize)]
pub struct Network {
    /// Chain genesis block hash.
    pub genesis_hash: String,
    /// Runtime spec version at recording.
    pub spec_version: u32,
    /// Transaction version at recording.
    pub transaction_version: u32,
    /// SS58 address format used by the recorder.
    pub ss58_format: u16,
}

/// Both metadata generations recorded for the same runtime.
#[derive(Debug, Clone, Deserialize)]
pub struct Metadata {
    /// Hash at which the metadata was read.
    pub block_hash: String,
    /// V15 inside its `Option<OpaqueMetadata>` response frame.
    pub v15_hex: String,
    /// Raw V14 `MetadataVersioned` response.
    pub v14_hex: String,
}

/// A composed call and the arguments used by the upstream recorder.
#[derive(Debug, Clone, Deserialize)]
pub struct Call {
    /// Pallet name.
    pub module: String,
    /// Call name.
    pub function: String,
    /// Python-shaped arguments, including the recorder's nested-call labels.
    pub params: Value,
    /// Expected complete SCALE call bytes.
    pub data_hex: String,
}

/// A complete storage key and its source parameters.
#[derive(Debug, Clone, Deserialize)]
pub struct StorageKey {
    /// Pallet name.
    pub pallet: String,
    /// Storage item name.
    pub storage_function: String,
    /// Map-key components in metadata order.
    pub params: Vec<Value>,
    /// Expected complete storage key bytes.
    pub key_hex: String,
    /// Upstream type identifier for the stored value.
    pub value_scale_type: String,
}

/// An unhashed signing payload vector.
#[derive(Debug, Clone, Deserialize)]
pub struct SignaturePayload {
    /// Complete SCALE call bytes.
    pub call_data_hex: String,
    /// Immortal `"00"` or a mortal era object.
    pub era: Value,
    /// Mortal era's birth hash; absent for immortal vectors.
    pub era_birth_block_hash: Option<String>,
    /// Signer nonce.
    pub nonce: u64,
    /// Tip in raw chain units.
    pub tip: u128,
    /// Expected bytes submitted to the signer.
    pub payload_hex: String,
}

/// A signed extrinsic with a deterministic fake signature.
#[derive(Debug, Clone, Deserialize)]
pub struct Extrinsic {
    /// Complete SCALE call bytes.
    pub call_data_hex: String,
    /// Signer's SS58 address.
    pub address: String,
    /// Signer's raw public key.
    pub public_key_hex: String,
    /// Upstream key scheme identifier.
    pub crypto_type: u8,
    /// Immortal `"00"` or a mortal era object.
    pub era: Value,
    /// Signer nonce.
    pub nonce: u64,
    /// Tip in raw chain units.
    pub tip: u128,
    /// Fake raw signature bytes, without the scheme tag.
    pub signature_hex: String,
    /// Runtime signature variant index.
    pub signature_version: u8,
    /// Expected complete extrinsic bytes.
    pub extrinsic_hex: String,
    /// Expected extrinsic hash.
    pub extrinsic_hash: String,
}

/// A deterministic multisig account derivation vector.
#[derive(Debug, Clone, Deserialize)]
pub struct Multisig {
    /// Signer SS58 addresses, preserving the recorder's input order.
    pub signatories: Vec<String>,
    /// Required signature count.
    pub threshold: u16,
    /// Expected derived account address.
    pub ss58_address: String,
}

/// A public key's SS58 representation.
#[derive(Debug, Clone, Deserialize)]
pub struct Ss58 {
    /// Raw public key bytes.
    pub public_key_hex: String,
    /// Expected SS58 address.
    pub address: String,
}

/// The upstream `golden.json` format.
#[derive(Debug, Clone)]
pub struct Golden {
    /// Network identity associated with these vectors.
    pub network: Network,
    /// Embedded V15 and V14 metadata.
    pub metadata: Metadata,
    /// Composed-call byte vectors.
    pub calls: Vec<Call>,
    /// Storage-key byte vectors.
    pub storage_keys: Vec<StorageKey>,
    /// Signing-payload byte vectors.
    pub signature_payloads: Vec<SignaturePayload>,
    /// Signed-extrinsic byte and hash vectors.
    pub extrinsics: Vec<Extrinsic>,
    /// Multisig account vectors.
    pub multisig: Vec<Multisig>,
    /// SS58 vectors.
    pub ss58: Vec<Ss58>,
    /// Remaining sections retained for constants, reads, events and receipts.
    pub sections: BTreeMap<String, Value>,
}

impl<'de> Deserialize<'de> for Golden {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Serde's flattened Content carrier rejects arbitrary-precision
        // integers. Keep the unmodeled read sections as Values throughout.
        let mut fields = match Value::deserialize(deserializer)? {
            Value::Object(fields) => fields,
            _ => return Err(serde::de::Error::custom("golden fixture must be an object")),
        };
        fn take<T: serde::de::DeserializeOwned, E: serde::de::Error>(
            fields: &mut serde_json::Map<String, Value>,
            name: &str,
        ) -> Result<T, E> {
            let value = fields
                .remove(name)
                .ok_or_else(|| E::custom(format!("missing golden section {name}")))?;
            serde_json::from_value(value).map_err(E::custom)
        }
        Ok(Self {
            network: take(&mut fields, "network")?,
            metadata: take(&mut fields, "metadata")?,
            calls: take(&mut fields, "calls")?,
            storage_keys: take(&mut fields, "storage_keys")?,
            signature_payloads: take(&mut fields, "signature_payloads")?,
            extrinsics: take(&mut fields, "extrinsics")?,
            multisig: take(&mut fields, "multisig")?,
            ss58: take(&mut fields, "ss58")?,
            sections: fields.into_iter().collect(),
        })
    }
}

impl Golden {
    /// Parse a recorder output through the exact JSON value carrier first.
    pub fn from_slice(bytes: &[u8]) -> Result<Self, FixtureError> {
        Ok(rtao_primitives::json::from_slice(bytes)?)
    }

    /// Read either the vendored golden file or a spec-current recorder output.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, FixtureError> {
        Self::from_slice(&fs::read(path)?)
    }

    /// Parse the embedded V15 using the recorded network identity.
    pub fn runtime(&self) -> Result<Runtime, FixtureError> {
        self.parse_runtime(&self.metadata.v15_hex)
    }

    /// Parse the embedded V14 using the same recorded network identity.
    pub fn runtime_v14(&self) -> Result<Runtime, FixtureError> {
        self.parse_runtime(&self.metadata.v14_hex)
    }

    fn parse_runtime(&self, encoded: &str) -> Result<Runtime, FixtureError> {
        Ok(Runtime::parse(
            &metadata_bytes(encoded)?,
            self.network.spec_version,
            self.network.transaction_version,
            self.network.ss58_format,
        )?)
    }
}

/// One SCALE byte sample and its expected decoded JSON shape.
#[derive(Debug, Clone, Deserialize)]
pub struct ShapeSample {
    /// SCALE bytes for this sample.
    pub scale_hex: String,
    /// Exact corpus rendering of the decoded value.
    pub decoded: Value,
}

/// Every recorded sample of one portable-registry type.
#[derive(Debug, Clone, Deserialize)]
pub struct ShapeType {
    /// Portable-registry type identifier.
    pub id: u32,
    /// Qualified type path or type-definition kind.
    pub name: String,
    /// Recorded byte and shape pairs.
    pub samples: Vec<ShapeSample>,
}

/// A registry type for which the recorder cannot produce an inhabited sample.
#[derive(Debug, Clone, Deserialize)]
pub struct SkippedType {
    /// Portable-registry type identifier.
    pub id: u32,
    /// Qualified type path or type-definition kind.
    pub name: String,
    /// Recorder explanation for the omission.
    pub reason: String,
}

/// The upstream `shape_corpus/corpus.json` format.
#[derive(Debug, Clone, Deserialize)]
pub struct ShapeCorpus {
    /// Codec used by the recorder.
    pub codec: String,
    /// Runtime spec version whose registry the samples describe.
    pub spec_version: u32,
    /// Number of types in the recorded portable registry.
    pub types_total: usize,
    /// Number of types for which the recorder generated samples.
    pub types_covered: usize,
    /// Sampled portable-registry types.
    pub types: Vec<ShapeType>,
    /// Registry types omitted by the recorder, with explicit reasons.
    pub skipped: Vec<SkippedType>,
}

impl ShapeCorpus {
    /// Parse a corpus output through the exact JSON value carrier first.
    pub fn from_slice(bytes: &[u8]) -> Result<Self, FixtureError> {
        Ok(rtao_primitives::json::from_slice(bytes)?)
    }

    /// Read either the vendored corpus or a spec-current recorder output.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, FixtureError> {
        Self::from_slice(&fs::read(path)?)
    }
}
