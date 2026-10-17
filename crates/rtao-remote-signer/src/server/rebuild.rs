// Derived from RaoFoundation/subtensor sdk/bittensor-core/src/codec/decode.rs @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/_transport/extrinsics.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Re-derive every signed byte of a request from its structured views.
//!
//! The SignerPayloadJSON view is parsed into the core's transaction
//! parameters with exact key names and widths. The extension bytes and the
//! signing payload are then recomputed with the server's trusted runtime and
//! compared byte for byte with what the client sent.

use rtao_core::{codec::Value, extrinsic::TxParams};
use rtao_primitives::AccountId;
use rtao_runtime::RuntimeHandle;
use rtao_signer::SignerPayloadJson;
use serde_json::{Value as Json, json};

use crate::wire::{SignRequest, WIRE_SS58_PREFIX, blake2_256, from_hex, to_hex};

/// Mortality decoded from a SignerPayloadJSON `era`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EraInfo {
    /// Immortal transaction, anchored at genesis.
    Immortal,
    /// Mortal transaction.
    Mortal {
        /// Validity period, a power of two in `4..=65536`.
        period: u64,
        /// Quantized phase, below `period`.
        phase: u64,
    },
}

impl EraInfo {
    /// Quantization factor of a mortal era.
    pub fn quantize_factor(period: u64) -> u64 {
        (period >> 12).max(1)
    }

    /// Core encoder carrier for this era.
    pub fn to_value(self) -> Value {
        match self {
            Self::Immortal => Value::str("00"),
            Self::Mortal { period, phase } => Value::record(vec![
                ("period".into(), Value::Uint(u128::from(period))),
                ("phase".into(), Value::Uint(u128::from(phase))),
            ]),
        }
    }

    /// Whether a mortal era born at `birth` matches this phase.
    pub fn born_at(self, birth: u64) -> bool {
        match self {
            Self::Immortal => false,
            Self::Mortal { period, phase } => {
                let factor = Self::quantize_factor(period);
                (birth % period) / factor * factor == phase
            }
        }
    }
}

impl std::fmt::Display for EraInfo {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Immortal => formatter.write_str("immortal"),
            Self::Mortal { period, phase } => write!(formatter, "mortal({period}/{phase})"),
        }
    }
}

/// Decode an encoded `Era` exactly as the core decoder does, then validate it.
///
/// `0x00` is immortal. Two bytes are a little-endian `u16` `e`, with
/// `period = 2 << (e % 16)`, quantize factor `max(period >> 12, 1)` and
/// `phase = (e >> 4) * factor`. The period must lie in `4..=65536` and the
/// phase must be below the period.
pub fn decode_era(bytes: &[u8]) -> Result<EraInfo, String> {
    match bytes {
        [0] => Ok(EraInfo::Immortal),
        [first, second] => {
            let encoded = u64::from(u16::from_le_bytes([*first, *second]));
            let period = 2u64 << (encoded % 16);
            let phase = (encoded >> 4) * EraInfo::quantize_factor(period);
            if !(4..=65_536).contains(&period) {
                return Err(format!("era period {period} is outside 4..=65536"));
            }
            if phase >= period {
                return Err(format!("era phase {phase} is not below period {period}"));
            }
            Ok(EraInfo::Mortal { period, phase })
        }
        _ => Err("era must be one byte (immortal) or two bytes (mortal)".into()),
    }
}

/// Why a request's views do not reproduce the signed bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RebuildError {
    /// A field has the wrong JSON type or width (HTTP 400).
    Malformed(String),
    /// A field disagrees with another view or with the runtime (HTTP 422).
    Tampered(String),
}

impl std::fmt::Display for RebuildError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Malformed(message) | Self::Tampered(message) => formatter.write_str(message),
        }
    }
}

/// Every server-derived view of an accepted request.
pub struct Rebuilt {
    /// Transaction parameters parsed from the JSON view.
    pub params: TxParams,
    /// Decoded mortality.
    pub era: EraInfo,
    /// Server-encoded signed extension bytes.
    pub extra: Vec<u8>,
    /// Server-encoded implied extension bytes.
    pub additional: Vec<u8>,
    /// Server-computed signing payload (hashed above 256 bytes).
    pub payload: Vec<u8>,
    /// Server-built SignerPayloadJSON view.
    pub payload_json: SignerPayloadJson,
}

impl std::fmt::Debug for Rebuilt {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Rebuilt")
            .field("era", &self.era)
            .field("nonce", &self.params.nonce)
            .field("tip", &self.params.tip)
            .field("extra", &to_hex(&self.extra))
            .field("additional", &to_hex(&self.additional))
            .field("payload", &to_hex(&self.payload))
            .finish_non_exhaustive()
    }
}

const KNOWN_KEYS: &[&str] = &[
    "address",
    "blockHash",
    "genesisHash",
    "method",
    "nonce",
    "specVersion",
    "tip",
    "transactionVersion",
    "era",
    "version",
    "signedExtensions",
    "assetId",
    "mode",
    "metadataHash",
];

fn malformed(message: impl Into<String>) -> RebuildError {
    RebuildError::Malformed(message.into())
}
fn tampered(message: impl Into<String>) -> RebuildError {
    RebuildError::Tampered(message.into())
}

fn string<'a>(json: &'a SignerPayloadJson, key: &str) -> Result<&'a str, RebuildError> {
    match json.get(key) {
        Some(Json::String(value)) => Ok(value),
        Some(_) => Err(malformed(format!("payload_json.{key} must be a string"))),
        None => Err(tampered(format!("payload_json.{key} is missing"))),
    }
}

fn hex_field(json: &SignerPayloadJson, key: &str) -> Result<Vec<u8>, RebuildError> {
    from_hex(string(json, key)?).map_err(|error| malformed(format!("payload_json.{key}: {error}")))
}

fn fixed_hex<const N: usize>(json: &SignerPayloadJson, key: &str) -> Result<[u8; N], RebuildError> {
    hex_field(json, key)?
        .try_into()
        .map_err(|_| malformed(format!("payload_json.{key} must be exactly {N} bytes")))
}

/// Parse the closed SignerPayloadJSON key set into transaction parameters.
///
/// Key names and widths follow rtao-client's builder, which mirrors Python's
/// `signer_payload_json`. Each value is cross-checked against the request's
/// other views and the trusted runtime.
pub fn parse_payload_json(
    runtime: &RuntimeHandle,
    request: &SignRequest,
) -> Result<(TxParams, EraInfo), RebuildError> {
    let json = &request.payload_json;
    // The key itself is client-chosen and is not echoed.
    if json.keys().any(|key| !KNOWN_KEYS.contains(&key.as_str())) {
        return Err(tampered(
            "payload_json has a key outside the SignerPayloadJSON set",
        ));
    }
    let address: AccountId = string(json, "address")?
        .parse()
        .map_err(|_| malformed("payload_json.address is not an SS58 address"))?;
    if address != request.account.0 {
        return Err(tampered("payload_json.address is not the signing account"));
    }
    if hex_field(json, "method")? != request.call.0 {
        return Err(tampered("payload_json.method differs from call"));
    }
    let genesis = fixed_hex::<32>(json, "genesisHash")?;
    if genesis != request.chain.genesis_hash.0 {
        return Err(tampered(
            "payload_json.genesisHash differs from chain.genesis_hash",
        ));
    }
    let spec_version = u32::from_be_bytes(fixed_hex::<4>(json, "specVersion")?);
    if spec_version != request.chain.spec_version {
        return Err(tampered(
            "payload_json.specVersion differs from chain.spec_version",
        ));
    }
    let transaction_version = u32::from_be_bytes(fixed_hex::<4>(json, "transactionVersion")?);
    if transaction_version != request.chain.transaction_version {
        return Err(tampered(
            "payload_json.transactionVersion differs from chain.transaction_version",
        ));
    }
    match json.get("version") {
        Some(Json::Number(number)) => {
            if number.as_u64() != Some(u64::from(runtime.core().extrinsic.version)) {
                return Err(tampered(
                    "payload_json.version is not the runtime extrinsic version",
                ));
            }
        }
        Some(_) => return Err(malformed("payload_json.version must be an integer")),
        None => return Err(tampered("payload_json.version is missing")),
    }
    let identifiers: Vec<&str> = runtime
        .core()
        .extrinsic
        .signed_extensions
        .iter()
        .map(|extension| extension.identifier.as_str())
        .collect();
    match json.get("signedExtensions") {
        Some(Json::Array(values)) => {
            let names = values
                .iter()
                .map(|value| value.as_str())
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| malformed("payload_json.signedExtensions must contain strings"))?;
            if names != identifiers || identifiers.is_empty() {
                return Err(tampered(
                    "payload_json.signedExtensions differs from the runtime's signed extensions",
                ));
            }
        }
        Some(_) => return Err(malformed("payload_json.signedExtensions must be an array")),
        None if identifiers.is_empty() => {}
        None => return Err(tampered("payload_json.signedExtensions is missing")),
    }
    let nonce = u32::from_be_bytes(fixed_hex::<4>(json, "nonce")?);
    let tip = u128::from_be_bytes(fixed_hex::<16>(json, "tip")?);
    let era_bytes = hex_field(json, "era")?;
    let era =
        decode_era(&era_bytes).map_err(|error| malformed(format!("payload_json.era: {error}")))?;
    let mut reencoded = Vec::new();
    runtime
        .core()
        .encode_era_value(&era.to_value(), &mut reencoded)
        .map_err(|_| tampered("payload_json.era cannot be re-encoded"))?;
    if reencoded != era_bytes {
        return Err(tampered("payload_json.era is not canonically encoded"));
    }
    let block_hash = fixed_hex::<32>(json, "blockHash")?;
    if era == EraInfo::Immortal && block_hash != genesis {
        return Err(tampered("an immortal transaction must anchor at genesis"));
    }
    let has = |name: &str| identifiers.contains(&name);
    let tip_asset_id = match json.get("assetId") {
        None => None,
        Some(Json::String(text)) => {
            if !has("ChargeAssetTxPayment") {
                return Err(tampered(
                    "payload_json.assetId is present but the runtime has no ChargeAssetTxPayment",
                ));
            }
            let bytes = from_hex(text)
                .map_err(|error| malformed(format!("payload_json.assetId: {error}")))?;
            Some(
                decode_compact(&bytes)
                    .map_err(|error| malformed(format!("payload_json.assetId: {error}")))?,
            )
        }
        Some(_) => return Err(malformed("payload_json.assetId must be a string")),
    };
    let metadata_hash = match (json.get("mode"), json.get("metadataHash")) {
        (None, None) if !has("CheckMetadataHash") => None,
        (Some(Json::Number(mode)), Some(Json::Null)) if mode.as_u64() == Some(0) => {
            if !has("CheckMetadataHash") {
                return Err(tampered(
                    "payload_json.mode is present but the runtime has no CheckMetadataHash",
                ));
            }
            None
        }
        (Some(Json::Number(mode)), Some(Json::String(_))) if mode.as_u64() == Some(1) => {
            if !has("CheckMetadataHash") {
                return Err(tampered(
                    "payload_json.mode is present but the runtime has no CheckMetadataHash",
                ));
            }
            Some(fixed_hex::<32>(json, "metadataHash")?)
        }
        (Some(Json::Number(_)) | None, Some(Json::Null | Json::String(_)) | None) => {
            return Err(tampered(
                "payload_json.mode and metadataHash are inconsistent",
            ));
        }
        _ => {
            return Err(malformed(
                "payload_json.mode must be an integer and metadataHash a string or null",
            ));
        }
    };
    Ok((
        TxParams {
            era: era.to_value(),
            nonce: u64::from(nonce),
            tip,
            tip_asset_id,
            genesis_hash: genesis,
            era_block_hash: block_hash,
            metadata_hash,
        },
        era,
    ))
}

/// Decode one SCALE compact integer that must consume every byte canonically.
pub fn decode_compact(bytes: &[u8]) -> Result<u128, String> {
    let first = *bytes.first().ok_or("empty compact integer")?;
    let (value, length) = match first & 0b11 {
        0b00 => (u128::from(first >> 2), 1),
        0b01 => {
            let raw = u16::from_le_bytes(
                bytes
                    .get(..2)
                    .ok_or("truncated compact integer")?
                    .try_into()
                    .map_err(|_| "truncated compact integer")?,
            );
            let value = u128::from(raw >> 2);
            if value < 0x40 {
                return Err("non-canonical compact integer".into());
            }
            (value, 2)
        }
        0b10 => {
            let raw = u32::from_le_bytes(
                bytes
                    .get(..4)
                    .ok_or("truncated compact integer")?
                    .try_into()
                    .map_err(|_| "truncated compact integer")?,
            );
            let value = u128::from(raw >> 2);
            if value < 0x4000 {
                return Err("non-canonical compact integer".into());
            }
            (value, 4)
        }
        _ => {
            let length = usize::from(first >> 2) + 4;
            if length > 16 {
                return Err("compact integer exceeds 128 bits".into());
            }
            let data = bytes.get(1..=length).ok_or("truncated compact integer")?;
            if data.last() == Some(&0) {
                return Err("non-canonical compact integer".into());
            }
            let mut buffer = [0u8; 16];
            buffer[..length].copy_from_slice(data);
            let value = u128::from_le_bytes(buffer);
            if value < 0x4000_0000 {
                return Err("non-canonical compact integer".into());
            }
            (value, length + 1)
        }
    };
    if bytes.len() != length {
        return Err("compact integer has trailing bytes".into());
    }
    Ok(value)
}

/// Build the SignerPayloadJSON view of verified parameters.
///
/// This mirrors rtao-client's builder so the backend signer receives a view
/// derived entirely by the server.
pub fn build_payload_json(
    runtime: &RuntimeHandle,
    call: &[u8],
    account: AccountId,
    params: &TxParams,
) -> Result<SignerPayloadJson, RebuildError> {
    let nonce = u32::try_from(params.nonce)
        .map_err(|_| malformed("nonce exceeds the SignerPayloadJSON u32 width"))?;
    let mut era = Vec::new();
    runtime
        .core()
        .encode_era_value(&params.era, &mut era)
        .map_err(|_| tampered("the era cannot be encoded"))?;
    let mut payload = SignerPayloadJson::new();
    for (name, value) in [
        ("address", json!(account.to_ss58(WIRE_SS58_PREFIX))),
        ("blockHash", json!(to_hex(&params.era_block_hash))),
        ("genesisHash", json!(to_hex(&params.genesis_hash))),
        ("method", json!(to_hex(call))),
        ("nonce", json!(to_hex(&nonce.to_be_bytes()))),
        (
            "specVersion",
            json!(to_hex(&runtime.spec_version().to_be_bytes())),
        ),
        ("tip", json!(to_hex(&params.tip.to_be_bytes()))),
        (
            "transactionVersion",
            json!(to_hex(&runtime.transaction_version().to_be_bytes())),
        ),
        ("era", json!(to_hex(&era))),
        ("version", json!(runtime.core().extrinsic.version)),
    ] {
        payload.insert(name.into(), value);
    }
    let extensions: Vec<String> = runtime
        .core()
        .extrinsic
        .signed_extensions
        .iter()
        .map(|extension| extension.identifier.clone())
        .collect();
    if !extensions.is_empty() {
        payload.insert("signedExtensions".into(), json!(extensions));
    }
    if let Some(asset) = params.tip_asset_id {
        let mut encoded = Vec::new();
        rtao_core::codec::encode::compact(asset, &mut encoded)
            .map_err(|_| tampered("the assetId cannot be encoded"))?;
        payload.insert("assetId".into(), json!(to_hex(&encoded)));
    }
    if let Some(hash) = params.metadata_hash {
        payload.insert("mode".into(), json!(1));
        payload.insert("metadataHash".into(), json!(to_hex(&hash)));
    } else if extensions
        .iter()
        .any(|extension| extension == "CheckMetadataHash")
    {
        payload.insert("mode".into(), json!(0));
        payload.insert("metadataHash".into(), Json::Null);
    }
    Ok(payload)
}

/// Parse the JSON view, re-derive the signed bytes and compare every view.
///
/// The caller must already have checked the chain stamp against the trusted
/// runtime and decoded the call with it.
pub fn rebuild(runtime: &RuntimeHandle, request: &SignRequest) -> Result<Rebuilt, RebuildError> {
    let (params, era) = parse_payload_json(runtime, request)?;
    let (extra, additional) = runtime
        .core()
        .signature_payload_parts(&params)
        .map_err(|_| tampered("signed extensions cannot be encoded from payload_json"))?;
    if extra != request.included_in_extrinsic.0 {
        return Err(tampered(
            "included_in_extrinsic differs from the payload_json parameters",
        ));
    }
    if additional != request.included_in_signed_data.0 {
        return Err(tampered(
            "included_in_signed_data differs from the payload_json parameters",
        ));
    }
    let mut data = request.call.0.clone();
    data.extend_from_slice(&extra);
    data.extend_from_slice(&additional);
    let payload = if data.len() > 256 {
        blake2_256(&data).to_vec()
    } else {
        data
    };
    if payload != request.payload.0 {
        return Err(tampered(
            "payload differs from call, extra and additional data",
        ));
    }
    let payload_json = build_payload_json(runtime, &request.call.0, request.account.0, &params)?;
    Ok(Rebuilt {
        params,
        era,
        extra,
        additional,
        payload,
        payload_json,
    })
}
