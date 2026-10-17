//! The `rtao-remote-signer/1` wire schema.
//!
//! Every type here is an rtao-authored contract. Byte strings are `"0x"`
//! followed by lowercase, even-length hex; accounts are SS58 strings that are
//! emitted at prefix 42 and accepted at any valid prefix, then compared as
//! bytes. Request and response objects reject unknown fields.

use std::fmt;

use blake2::{Blake2b, Digest, digest::consts::U32};
use rtao_primitives::{AccountId, CryptoType};
use rtao_signer::{MultiSignature, SignPurpose, SignerError, SignerPayloadJson, SigningRequest};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Protocol identifier carried in every body.
pub const PROTOCOL: &str = "rtao-remote-signer/1";
/// Relative path of the signing endpoint, joined to the base URL.
pub const SIGN_PATH: &str = "sign";
/// Relative path of the account discovery endpoint, joined to the base URL.
pub const ACCOUNTS_PATH: &str = "accounts";
/// Largest response body a client reads, enforced while streaming.
pub const MAX_RESPONSE_BYTES: usize = 64 * 1024;
/// Default largest request body a server accepts.
pub const DEFAULT_BODY_LIMIT: usize = 1024 * 1024;
/// SS58 prefix used when this protocol emits an account.
pub const WIRE_SS58_PREFIX: u16 = 42;

/// BLAKE2b-256 of `data`.
pub fn blake2_256(data: &[u8]) -> [u8; 32] {
    Blake2b::<U32>::digest(data).into()
}

/// Encode bytes in the protocol's strict `0x`-prefixed lowercase hex form.
pub fn to_hex(bytes: &[u8]) -> String {
    format!("0x{}", hex::encode(bytes))
}

/// Decode the protocol's strict hex form: `0x` prefix, lowercase, even length.
pub fn from_hex(text: &str) -> Result<Vec<u8>, String> {
    let digits = text
        .strip_prefix("0x")
        .ok_or_else(|| "byte string must start with 0x".to_owned())?;
    if digits.len() % 2 != 0 {
        return Err("byte string has an odd number of hex digits".into());
    }
    if !digits
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("byte string must use lowercase hex digits".into());
    }
    hex::decode(digits).map_err(|_| "byte string is not hex".to_owned())
}

/// Arbitrary bytes in strict `0x` lowercase hex form.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct HexBytes(pub Vec<u8>);

impl fmt::Debug for HexBytes {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&to_hex(&self.0))
    }
}
impl From<&[u8]> for HexBytes {
    fn from(bytes: &[u8]) -> Self {
        Self(bytes.to_vec())
    }
}
impl From<Vec<u8>> for HexBytes {
    fn from(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }
}
impl AsRef<[u8]> for HexBytes {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}
impl Serialize for HexBytes {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&to_hex(&self.0))
    }
}
impl<'de> Deserialize<'de> for HexBytes {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        from_hex(&text).map(Self).map_err(serde::de::Error::custom)
    }
}

/// Exactly 32 bytes in strict `0x` lowercase hex form.
#[derive(Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct Hex32(pub [u8; 32]);

impl fmt::Debug for Hex32 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&to_hex(&self.0))
    }
}
impl Serialize for Hex32 {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&to_hex(&self.0))
    }
}
impl<'de> Deserialize<'de> for Hex32 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        let bytes = from_hex(&text).map_err(serde::de::Error::custom)?;
        bytes
            .try_into()
            .map(Self)
            .map_err(|_| serde::de::Error::custom("expected exactly 32 bytes"))
    }
}

/// An AccountId32 emitted at SS58 prefix 42 and accepted at any valid prefix.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Ss58(pub AccountId);

impl fmt::Debug for Ss58 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0.to_ss58(WIRE_SS58_PREFIX))
    }
}
impl From<AccountId> for Ss58 {
    fn from(account: AccountId) -> Self {
        Self(account)
    }
}
impl Serialize for Ss58 {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0.to_ss58(WIRE_SS58_PREFIX))
    }
}
impl<'de> Deserialize<'de> for Ss58 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        if text.len() > 64 {
            return Err(serde::de::Error::custom("SS58 address is too long"));
        }
        text.parse::<AccountId>()
            .map(Self)
            .map_err(|_| serde::de::Error::custom("invalid SS58 address"))
    }
}

/// Signature algorithm spelled exactly `sr25519` or `ed25519`.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum WireCrypto {
    /// Schnorrkel sr25519, MultiSignature variant 1.
    Sr25519,
    /// Ed25519, MultiSignature variant 0.
    Ed25519,
}
impl From<CryptoType> for WireCrypto {
    fn from(crypto: CryptoType) -> Self {
        match crypto {
            CryptoType::Sr25519 => Self::Sr25519,
            CryptoType::Ed25519 => Self::Ed25519,
        }
    }
}
impl From<WireCrypto> for CryptoType {
    fn from(crypto: WireCrypto) -> Self {
        match crypto {
            WireCrypto::Sr25519 => Self::Sr25519,
            WireCrypto::Ed25519 => Self::Ed25519,
        }
    }
}

/// Submission stage, spelled `extrinsic`, `shield_inner` or `shield_carrier`.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum WirePurpose {
    /// An ordinary signed extrinsic.
    Extrinsic,
    /// The signed transaction sealed inside a shield envelope.
    ShieldInner,
    /// The signed transaction carrying a shield envelope.
    ShieldCarrier,
}
impl From<SignPurpose> for WirePurpose {
    fn from(purpose: SignPurpose) -> Self {
        match purpose {
            SignPurpose::Extrinsic => Self::Extrinsic,
            SignPurpose::ShieldInner => Self::ShieldInner,
            SignPurpose::ShieldCarrier => Self::ShieldCarrier,
        }
    }
}
impl From<WirePurpose> for SignPurpose {
    fn from(purpose: WirePurpose) -> Self {
        match purpose {
            WirePurpose::Extrinsic => Self::Extrinsic,
            WirePurpose::ShieldInner => Self::ShieldInner,
            WirePurpose::ShieldCarrier => Self::ShieldCarrier,
        }
    }
}
impl WirePurpose {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Extrinsic => "extrinsic",
            Self::ShieldInner => "shield_inner",
            Self::ShieldCarrier => "shield_carrier",
        }
    }
}

/// The chain and runtime identity the client prepared against.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ChainStamp {
    /// Genesis hash of the target chain.
    pub genesis_hash: Hex32,
    /// Runtime specification version.
    pub spec_version: u32,
    /// Signed-transaction format version.
    pub transaction_version: u32,
    /// BLAKE2b-256 of the client's metadata bytes; diagnostic only.
    pub metadata_blake2_256: Hex32,
}

/// `POST /sign` request body: every public view of one prepared transaction.
///
/// The client's metadata bytes and display constants are not sent; the
/// server decodes and re-derives everything with its own trusted runtime.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SignRequest {
    /// Always [`PROTOCOL`].
    pub version: String,
    /// Account whose signature is requested.
    pub account: Ss58,
    /// Expected signature algorithm of `account`.
    pub crypto: WireCrypto,
    /// Submission stage.
    pub purpose: WirePurpose,
    /// Runtime identity used for preparation.
    pub chain: ChainStamp,
    /// SCALE call bytes.
    pub call: HexBytes,
    /// Signed extension bytes carried in the extrinsic.
    pub included_in_extrinsic: HexBytes,
    /// Signed extension bytes implied in the signing data.
    pub included_in_signed_data: HexBytes,
    /// Bytes to sign, already hashed when longer than 256 bytes.
    pub payload: HexBytes,
    /// Polkadot-JS SignerPayloadJSON view, verbatim.
    pub payload_json: SignerPayloadJson,
}

impl SignRequest {
    /// Copy every view of a signing request without re-encoding it.
    ///
    /// Only the BLAKE2b-256 of the client's metadata is sent, as a diagnostic.
    pub fn from_signing_request(
        request: &SigningRequest<'_>,
        account: AccountId,
        crypto: CryptoType,
    ) -> Self {
        Self {
            version: PROTOCOL.into(),
            account: Ss58(account),
            crypto: crypto.into(),
            purpose: request.purpose.into(),
            chain: ChainStamp {
                genesis_hash: Hex32(request.context.genesis_hash),
                spec_version: request.context.chain_info.spec_version,
                transaction_version: request.context.transaction_version,
                metadata_blake2_256: Hex32(blake2_256(&request.context.metadata_bytes)),
            },
            call: request.call.into(),
            included_in_extrinsic: request.included_in_extrinsic.into(),
            included_in_signed_data: request.included_in_signed_data.into(),
            payload: request.payload.into(),
            payload_json: request.payload_json.clone(),
        }
    }
}

/// `POST /sign` success body.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SignResponse {
    /// Always [`PROTOCOL`].
    pub version: String,
    /// The signing account.
    pub account: Ss58,
    /// The 65-byte MultiSignature: variant byte, then the 64-byte signature.
    pub signature: HexBytes,
}

impl SignResponse {
    /// Decode the MultiSignature and require the expected algorithm.
    pub fn signature(&self, expected: CryptoType) -> Result<MultiSignature, SignerError> {
        MultiSignature::from_bytes_for(&self.signature.0, expected)
    }
}

/// One account a client may request signatures for.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AccountEntry {
    /// The served account.
    pub account: Ss58,
    /// Its signature algorithm.
    pub crypto: WireCrypto,
}

/// `GET /accounts` success body, filtered to the caller's grants.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AccountsResponse {
    /// Always [`PROTOCOL`].
    pub version: String,
    /// The server's btauth receiver identity.
    pub server_key: Ss58,
    /// Accounts the caller is authorized for.
    pub accounts: Vec<AccountEntry>,
    /// The chain the server signs for.
    pub genesis_hash: Hex32,
    /// Purposes the server accepts.
    pub purposes: Vec<WirePurpose>,
}

/// Error body. Authentication-layer errors carry only `detail`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct ErrorBody {
    /// Human-readable diagnostic; never echoes request bytes.
    pub detail: String,
    /// Stable handler error code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// Policy violations in check order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub violations: Option<Vec<String>>,
}

/// Stable handler error codes and their HTTP statuses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorCode {
    /// JSON, shape, hex, version or field type error (400).
    MalformedRequest,
    /// The call does not decode canonically under the trusted runtime (400).
    UndecodableCall,
    /// The account is not served or not granted to the caller (403).
    AccountNotAuthorized,
    /// The purpose is disabled on this server (403).
    PurposeNotAllowed,
    /// Genesis, versions or metadata differ from the trusted runtime (409).
    RuntimeMismatch,
    /// A signed view does not match its server re-derivation (422).
    TamperedRequest,
    /// The era is immortal, too long, or not anchored canonically (422).
    EraRejected,
    /// Operator policy refused the call (403).
    PolicyDenied,
    /// The trusted runtime source failed (503).
    ChainUnavailable,
    /// The backend signer failed or its signature did not verify (500).
    SigningFailed,
}

impl ErrorCode {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MalformedRequest => "malformed_request",
            Self::UndecodableCall => "undecodable_call",
            Self::AccountNotAuthorized => "account_not_authorized",
            Self::PurposeNotAllowed => "purpose_not_allowed",
            Self::RuntimeMismatch => "runtime_mismatch",
            Self::TamperedRequest => "tampered_request",
            Self::EraRejected => "era_rejected",
            Self::PolicyDenied => "policy_denied",
            Self::ChainUnavailable => "chain_unavailable",
            Self::SigningFailed => "signing_failed",
        }
    }

    /// The HTTP status used with this code.
    pub fn status(self) -> u16 {
        match self {
            Self::MalformedRequest | Self::UndecodableCall => 400,
            Self::AccountNotAuthorized | Self::PurposeNotAllowed | Self::PolicyDenied => 403,
            Self::RuntimeMismatch => 409,
            Self::TamperedRequest | Self::EraRejected => 422,
            Self::ChainUnavailable => 503,
            Self::SigningFailed => 500,
        }
    }

    /// Parse a wire spelling.
    pub fn parse(code: &str) -> Option<Self> {
        Some(match code {
            "malformed_request" => Self::MalformedRequest,
            "undecodable_call" => Self::UndecodableCall,
            "account_not_authorized" => Self::AccountNotAuthorized,
            "purpose_not_allowed" => Self::PurposeNotAllowed,
            "runtime_mismatch" => Self::RuntimeMismatch,
            "tampered_request" => Self::TamperedRequest,
            "era_rejected" => Self::EraRejected,
            "policy_denied" => Self::PolicyDenied,
            "chain_unavailable" => Self::ChainUnavailable,
            "signing_failed" => Self::SigningFailed,
            _ => return None,
        })
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}
