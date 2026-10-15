// Derived from RaoFoundation/subtensor sdk/python/bittensor/_transport/contract.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/_transport/extrinsics.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Import separates untrusted JSON from an authenticated signing context.

use crate::{BlockRef, Client, ClientError, NoncePolicy, SubmitOptions, UnsignedExtrinsic, View};
use rtao_core::{CoreError, codec::Value, extrinsic::TxParams, value_ext::ValueExt};
use rtao_primitives::{AccountId, CryptoType, H256};
use rtao_runtime::{CallBytes, RuntimeHandle};
use rtao_signer::{
    BoxFuture, MultiSignature, Signer, SignerCaps, SignerError, SignerPayloadJson, SigningContext,
    SigningRequest,
};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::json;

/// Maximum unsigned transaction JSON size accepted before parsing or copying.
///
/// This permits repeated hex views of a full chain-sized call while bounding
/// untrusted input allocations. Metadata is supplied independently.
pub const MAX_UNSIGNED_JSON_BYTES: usize = 64 * 1024 * 1024;

/// Preparation that uses a public account without resolving a private signer.
#[derive(Clone, Debug)]
pub struct PublicPrepareOptions {
    /// Nonce, era, display constants and operation deadlines.
    pub submit: SubmitOptions,
    /// Algorithm expected from the external signer.
    pub crypto_type: CryptoType,
    /// Native TAO tip in rao.
    pub tip: u64,
    /// Optional fee asset identifier.
    pub tip_asset_id: Option<u64>,
    /// Optional digest, checked against authenticated metadata and display constants.
    pub metadata_hash: Option<[u8; 32]>,
}
impl Default for PublicPrepareOptions {
    fn default() -> Self {
        Self {
            submit: SubmitOptions::default(),
            crypto_type: CryptoType::Sr25519,
            tip: 0,
            tip_asset_id: None,
            metadata_hash: None,
        }
    }
}

// All 17 fields are required, including nullable fields. No policy or context
// from a file can authorize submission or replace authenticated metadata.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Wire {
    call_data: String,
    address: String,
    public_key: String,
    crypto_type: u8,
    era: serde_json::Value,
    nonce: u64,
    tip: u64,
    #[serde(deserialize_with = "required_option")]
    tip_asset_id: Option<u64>,
    genesis_hash: String,
    era_block_hash: String,
    spec_version: u32,
    transaction_version: u32,
    #[serde(deserialize_with = "required_option")]
    metadata_hash: Option<String>,
    payload: String,
    payload_json: SignerPayloadJson,
    included_in_extrinsic: String,
    included_in_signed_data: String,
}
fn required_option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::deserialize(deserializer)
}

/// A parsed file that cannot be signed or submitted until its contents are validated.
#[derive(Debug)]
pub struct UntrustedUnsignedExtrinsic(Wire);
impl UntrustedUnsignedExtrinsic {
    /// Parse Python's exact field schema with native unsigned integer bounds.
    pub fn from_json(data: &[u8]) -> Result<Self, ClientError> {
        check_input_size(data)?;
        // Domain decoding goes through Value to retain exact numeric tokens.
        // Check every object before Value can collapse duplicate keys, including
        // era and SignerPayloadJSON. Alternate parsers must see the same input.
        let mut parser = serde_json::Deserializer::from_slice(data);
        let mut budget = JsonBudget::default();
        parser
            .deserialize_map(UniqueFields(&mut budget))
            .map_err(|e| invalid(e.to_string()))?;
        parser.end().map_err(|e| invalid(e.to_string()))?;
        let wire = rtao_primitives::json::from_slice(data).map_err(|e| invalid(e.to_string()))?;
        Ok(Self(wire))
    }

    /// Validate every signing view against independently authenticated inputs.
    ///
    /// The caller supplies metadata, chain context, the era anchor's canonical
    /// hash, and a current finalized height obtained independently of the file.
    /// Use [`Client::import_unsigned_json`] to fetch those inputs from a client.
    pub fn validate(
        self,
        runtime: &RuntimeHandle,
        context: &SigningContext,
        era_block_hash: H256,
        finalized_height: u64,
    ) -> Result<UnsignedExtrinsic, ClientError> {
        let wire = self.0;
        if wire.spec_version != runtime.spec_version()
            || wire.transaction_version != runtime.transaction_version()
        {
            return Err(invalid(
                "prepared runtime versions changed; prepare and sign again",
            ));
        }
        if context.metadata_bytes != runtime.core().metadata_bytes
            || context.transaction_version != runtime.transaction_version()
            || context.chain_info.spec_version != runtime.spec_version()
        {
            return Err(invalid(
                "authenticated signing context does not match runtime",
            ));
        }
        let genesis = H256(context.genesis_hash);
        if hash(&wire.genesis_hash)? != genesis || hash(&wire.era_block_hash)? != era_block_hash {
            return Err(invalid(
                "prepared genesis or era anchor does not match the chain",
            ));
        }
        let address: AccountId = wire
            .address
            .parse()
            .map_err(|e| invalid(format!("invalid signing address: {e}")))?;
        if fixed::<32>(&wire.public_key)? != address.0 {
            return Err(invalid("public_key does not match address"));
        }
        let crypto_type =
            CryptoType::try_from(wire.crypto_type).map_err(|e| invalid(e.to_string()))?;
        let era = parse_era(&wire.era)?;
        let (birth, expiry) = era_window(&era, finalized_height)?;
        if birth.is_none() && era_block_hash != genesis {
            return Err(invalid("immortal transaction must anchor at genesis"));
        }
        let metadata_hash = wire.metadata_hash.as_deref().map(fixed::<32>).transpose()?;
        if let Some(digest) = metadata_hash
            && runtime.metadata_digest(&context.chain_info)? != digest
        {
            return Err(invalid(
                "metadata_hash does not match authenticated runtime",
            ));
        }
        let call = CallBytes::decode(&bytes(&wire.call_data)?, runtime)?;
        let params = TxParams {
            era: era.clone(),
            nonce: wire.nonce,
            tip: wire.tip.into(),
            tip_asset_id: wire.tip_asset_id.map(u128::from),
            genesis_hash: genesis.0,
            era_block_hash: era_block_hash.0,
            metadata_hash,
        };
        let (extra, additional) = runtime
            .core()
            .signature_payload_parts(&params)
            .map_err(CoreError::from)?;
        let payload = runtime
            .core()
            .signature_payload(call.as_bytes(), &params)
            .map_err(CoreError::from)?;
        let mut payload_json = crate::submit::payload_json(runtime, &call, address, &params)?;
        // SS58 prefix is a presentation choice. Verify the account above and
        // retain Python's original presentation in the JSON signer view.
        payload_json.insert("address".into(), json!(wire.address));
        if bytes(&wire.payload)? != payload
            || bytes(&wire.included_in_extrinsic)? != extra
            || bytes(&wire.included_in_signed_data)? != additional
            || wire.payload_json != payload_json
        {
            return Err(invalid(
                "prepared payload, JSON view or signed parts were altered",
            ));
        }
        Ok(UnsignedExtrinsic {
            call,
            address,
            crypto_type,
            era,
            nonce: wire.nonce,
            tip: wire.tip,
            tip_asset_id: wire.tip_asset_id,
            genesis_hash: genesis,
            era_block_hash,
            metadata_hash,
            payload: payload.into(),
            included_in_extrinsic: extra.into(),
            included_in_signed_data: additional.into(),
            payload_json,
            context: context.clone(),
            submission_head: finalized_height,
            expiry,
            runtime: runtime.clone(),
        })
    }
}

impl Serialize for UnsignedExtrinsic {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let era = match &self.era {
            Value::Str(s) => json!(s),
            Value::Dict(entries) => {
                let fields =
                    entries
                        .iter()
                        .map(|(k, v)| {
                            let key = k.as_str().ok_or_else(|| {
                                serde::ser::Error::custom("invalid prepared era key")
                            })?;
                            let number =
                                v.as_u128().and_then(|v| u64::try_from(v).ok()).ok_or_else(
                                    || serde::ser::Error::custom("invalid prepared era number"),
                                )?;
                            Ok((key.to_owned(), json!(number)))
                        })
                        .collect::<Result<serde_json::Map<_, _>, S::Error>>()?;
                serde_json::Value::Object(fields)
            }
            _ => return Err(serde::ser::Error::custom("invalid prepared era")),
        };
        Wire {
            call_data: hex_bytes(self.call.as_bytes()),
            address: self
                .payload_json
                .get("address")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| serde::ser::Error::custom("missing prepared address"))?
                .to_owned(),
            public_key: hex_bytes(&self.address.0),
            crypto_type: self.crypto_type.into(),
            era,
            nonce: self.nonce,
            tip: self.tip,
            tip_asset_id: self.tip_asset_id,
            genesis_hash: hex_bytes(&self.genesis_hash.0),
            era_block_hash: hex_bytes(&self.era_block_hash.0),
            spec_version: self.call.spec_version(),
            transaction_version: self.call.transaction_version(),
            metadata_hash: self.metadata_hash.as_ref().map(|v| hex_bytes(v)),
            payload: hex_bytes(&self.payload),
            payload_json: self.payload_json.clone(),
            included_in_extrinsic: hex_bytes(&self.included_in_extrinsic),
            included_in_signed_data: hex_bytes(&self.included_in_signed_data),
        }
        .serialize(serializer)
    }
}
impl UnsignedExtrinsic {
    /// Export exactly Python's 17-field `UnsignedExtrinsic.to_dict()` schema.
    pub fn to_json(&self) -> Result<serde_json::Value, serde_json::Error> {
        serde_json::to_value(self)
    }
    /// Verify the negotiated signature over the immutable, size-normalized payload.
    pub fn verify_signature(&self, signature: MultiSignature) -> Result<(), ClientError> {
        if signature.crypto != self.crypto_type
            || !rtao_core::keys::verify_strict(
                &self.payload,
                &signature.bytes,
                &self.address.to_ss58(42),
                signature.crypto.into(),
            )
            .map_err(CoreError::from)?
        {
            return Err(invalid(
                "signature does not verify for the prepared account and payload",
            ));
        }
        Ok(())
    }
}

impl Client {
    /// Prepare using a public account; no key source is resolved and no nonce is leased.
    pub async fn prepare_public(
        &self,
        call: CallBytes,
        address: AccountId,
        opts: &PublicPrepareOptions,
    ) -> Result<UnsignedExtrinsic, ClientError> {
        let limit = crate::submit::SubmissionLimits::new(&opts.submit)?.admission;
        limit
            .run(self, async {
                let nonce = match opts.submit.nonce {
                    NoncePolicy::Auto => self.nonces().fresh(&address).await?,
                    NoncePolicy::Explicit(nonce) => nonce,
                };
                let signer = PublicIdentity {
                    address,
                    crypto: opts.crypto_type,
                    metadata: opts.metadata_hash.is_some(),
                };
                let unsigned = self
                    .prepare_at_nonce_with_fees(
                        call,
                        &signer,
                        &opts.submit,
                        nonce,
                        None,
                        (opts.tip, opts.tip_asset_id),
                    )
                    .await?;
                if unsigned.metadata_hash != opts.metadata_hash {
                    return Err(invalid(
                        "requested metadata_hash does not match authenticated runtime",
                    ));
                }
                Ok(unsigned)
            })
            .await
    }

    /// Import a file after authenticating runtime, genesis, era anchor and all signing views.
    ///
    /// `opts` supplies metadata display constants and the import deadline. The
    /// file supplies neither policy authorization nor private signing material.
    pub async fn import_unsigned_json(
        &self,
        data: &[u8],
        opts: &SubmitOptions,
    ) -> Result<UnsignedExtrinsic, ClientError> {
        let limit = crate::submit::SubmissionLimits::new(opts)?.admission;
        limit
            .run(self, async {
                check_input_size(data)?;
                let data = data.to_vec();
                let untrusted =
                    rtao_core::offload(move || UntrustedUnsignedExtrinsic::from_json(&data))
                        .await
                        .map_err(|e| invalid(e.to_string()))??;
                self.ensure_signing().await?;
                let runtime = self.refresh_runtime().await?;
                let version: serde_json::Value = self
                    .request("state_getRuntimeVersion", json!([null]))
                    .await?;
                if version
                    .get("specVersion")
                    .and_then(serde_json::Value::as_u64)
                    != Some(runtime.spec_version().into())
                    || version
                        .get("transactionVersion")
                        .and_then(serde_json::Value::as_u64)
                        != Some(runtime.transaction_version().into())
                {
                    return Err(invalid("live runtime changed during import"));
                }
                let height = self.at(BlockRef::Finalized).await?.block().number;
                let era = parse_era(&untrusted.0.era)?;
                let (birth, _) = era_window(&era, height)?;
                let anchor = match birth {
                    Some(birth) => self.at(BlockRef::Number(birth)).await?.block().hash,
                    None => self.genesis(),
                };
                let context = SigningContext {
                    metadata_bytes: runtime.core().metadata_bytes.clone(),
                    chain_info: rtao_core::digest::ChainInfo {
                        spec_version: runtime.spec_version(),
                        spec_name: version
                            .get("specName")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                        base58_prefix: 42,
                        decimals: opts.metadata_decimals,
                        token_symbol: opts.metadata_symbol.clone(),
                    },
                    transaction_version: runtime.transaction_version(),
                    genesis_hash: self.genesis().0,
                };
                rtao_core::offload(move || untrusted.validate(&runtime, &context, anchor, height))
                    .await
                    .map_err(|e| invalid(e.to_string()))?
            })
            .await
    }
}

struct PublicIdentity {
    address: AccountId,
    crypto: CryptoType,
    metadata: bool,
}
impl Signer for PublicIdentity {
    fn account(&self) -> AccountId {
        self.address
    }
    fn crypto_type(&self) -> CryptoType {
        self.crypto
    }
    fn caps(&self) -> SignerCaps {
        if self.metadata {
            SignerCaps::METADATA_HASH
        } else {
            SignerCaps::empty()
        }
    }
    fn sign<'a>(
        &'a self,
        _: &'a SigningRequest<'a>,
    ) -> BoxFuture<'a, Result<MultiSignature, SignerError>> {
        Box::pin(async { Err(SignerError::RawSigningUnsupported) })
    }
}

fn invalid(message: impl Into<String>) -> ClientError {
    ClientError::Protocol(message.into())
}
fn check_input_size(data: &[u8]) -> Result<(), ClientError> {
    if data.len() > MAX_UNSIGNED_JSON_BYTES {
        return Err(invalid("unsigned transaction JSON exceeds 64 MiB"));
    }
    Ok(())
}
fn hex_bytes(bytes: &[u8]) -> String {
    format!("0x{}", hex::encode(bytes))
}
fn bytes(value: &str) -> Result<Vec<u8>, ClientError> {
    hex::decode(value.strip_prefix("0x").unwrap_or(value))
        .map_err(|e| invalid(format!("invalid prepared hex: {e}")))
}
fn fixed<const N: usize>(value: &str) -> Result<[u8; N], ClientError> {
    bytes(value)?
        .try_into()
        .map_err(|_| invalid(format!("expected {N} prepared bytes")))
}
fn hash(value: &str) -> Result<H256, ClientError> {
    fixed(value).map(H256)
}
fn parse_era(value: &serde_json::Value) -> Result<Value, ClientError> {
    if value == "00" {
        return Ok(Value::Str("00".into()));
    }
    let object = value
        .as_object()
        .ok_or_else(|| invalid("era must be 00 or a mortal era object"))?;
    if object
        .keys()
        .any(|k| !matches!(k.as_str(), "period" | "current" | "phase"))
    {
        return Err(invalid("unknown mortal era field"));
    }
    let fields = object
        .iter()
        .map(|(k, v)| {
            v.as_u64()
                .map(|v| (k.clone(), Value::Uint(v.into())))
                .ok_or_else(|| invalid("era fields must be native unsigned integers"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let era = Value::record(fields);
    let period = era
        .field("period")
        .and_then(ValueExt::as_u128)
        .ok_or_else(|| invalid("mortal era needs period"))?;
    if period > 1u128 << 63 {
        return Err(invalid(
            "mortal era period overflows the core's normalization",
        ));
    }
    if era.field("current").is_none() {
        return Err(invalid("normalized mortal era needs current"));
    }
    if let Some(phase) = era.field("phase").and_then(ValueExt::as_u128) {
        let current = era
            .field("current")
            .and_then(ValueExt::as_u128)
            .ok_or_else(|| invalid("mortal era needs current"))?;
        let normalized = u128::from(crate::Period::new(period as u64).get());
        let quantize = (normalized >> 12).max(1);
        if phase != (current % normalized) / quantize * quantize {
            return Err(invalid("mortal era phase disagrees with current"));
        }
    }
    Ok(era)
}
fn era_window(era: &Value, height: u64) -> Result<(Option<u64>, Option<u64>), ClientError> {
    if matches!(era, Value::Str(s) if s == "00") {
        return Ok((None, None));
    }
    let get = |name| {
        era.field(name)
            .and_then(ValueExt::as_u128)
            .and_then(|v| u64::try_from(v).ok())
            .ok_or_else(|| invalid("invalid normalized mortal era"))
    };
    let period = get("period")?;
    let current = get("current")?;
    let birth = rtao_core::extrinsic::era_birth(period, current);
    let expiry = birth
        .checked_add(crate::Period::new(period).get())
        .ok_or_else(|| invalid("mortal era expiry overflow"))?;
    if current > height || height >= expiry {
        return Err(invalid(
            "mortal era is future or expired; prepare and sign again",
        ));
    }
    Ok((Some(birth), Some(expiry)))
}

struct JsonBudget {
    nodes_remaining: u32,
    depth: u32,
}
impl Default for JsonBudget {
    fn default() -> Self {
        Self {
            nodes_remaining: 1 << 20,
            depth: 0,
        }
    }
}
struct UniqueFields<'a>(&'a mut JsonBudget);
impl<'de> serde::de::DeserializeSeed<'de> for UniqueFields<'_> {
    type Value = ();
    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        use serde::de::Error;
        self.0.nodes_remaining =
            self.0.nodes_remaining.checked_sub(1).ok_or_else(|| {
                D::Error::custom("unsigned transaction JSON exceeds its node budget")
            })?;
        if self.0.depth >= 128 {
            return Err(D::Error::custom(
                "unsigned transaction JSON exceeds its depth budget",
            ));
        }
        self.0.depth += 1;
        let result = deserializer.deserialize_any(UniqueFields(&mut *self.0));
        self.0.depth -= 1;
        result
    }
}
impl<'de> serde::de::Visitor<'de> for UniqueFields<'_> {
    type Value = ();
    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("an object with unique unsigned transaction fields")
    }
    fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<(), E> {
        Ok(())
    }
    fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<(), E> {
        Ok(())
    }
    fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<(), E> {
        Ok(())
    }
    fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<(), E> {
        Ok(())
    }
    fn visit_str<E: serde::de::Error>(self, _: &str) -> Result<(), E> {
        Ok(())
    }
    fn visit_unit<E: serde::de::Error>(self) -> Result<(), E> {
        Ok(())
    }
    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        while seq.next_element_seed(UniqueFields(&mut *self.0))?.is_some() {}
        Ok(())
    }
    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        let mut seen = std::collections::BTreeSet::new();
        while let Some(key) = map.next_key::<String>()? {
            // serde_json's Value deserializer recognizes these literal private
            // carrier keys when raw_value/arbitrary_precision are enabled. A
            // RawValue string could otherwise inject a second, unscanned JSON
            // document after this pass, including duplicates or excessive nodes.
            if matches!(
                key.as_str(),
                "$serde_json::private::RawValue" | "$serde_json::private::Number"
            ) {
                return Err(serde::de::Error::custom(
                    "private JSON carrier keys are not unsigned transaction fields",
                ));
            }
            if !seen.insert(key) {
                return Err(serde::de::Error::custom(
                    "duplicate unsigned transaction field",
                ));
            }
            map.next_value_seed(UniqueFields(&mut *self.0))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn structural_budget_is_shared_across_siblings_before_domain_parsing() {
        let mut parser = serde_json::Deserializer::from_slice(br#"{"a":[0,1],"b":[2,3]}"#);
        let mut budget = JsonBudget {
            nodes_remaining: 5,
            depth: 0,
        };
        let error = parser
            .deserialize_map(UniqueFields(&mut budget))
            .unwrap_err();
        assert!(error.to_string().contains("node budget"));
        assert_eq!(budget.depth, 0);
    }

    #[test]
    fn structural_scan_bounds_nested_containers_and_rejects_duplicate_array_objects() {
        let mut parser = serde_json::Deserializer::from_slice(br#"{"a":[[[0]]]}"#);
        let mut budget = JsonBudget {
            nodes_remaining: 20,
            depth: 127,
        };
        let error = parser
            .deserialize_map(UniqueFields(&mut budget))
            .unwrap_err();
        assert!(error.to_string().contains("depth budget"));
        assert_eq!(budget.depth, 127);

        let mut parser = serde_json::Deserializer::from_slice(br#"{"a":[{"x":0,"x":1}]}"#);
        let error = parser
            .deserialize_map(UniqueFields(&mut JsonBudget::default()))
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("duplicate unsigned transaction field")
        );
    }
}
