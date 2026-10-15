// Derived from RaoFoundation/subtensor sdk/python/bittensor/client.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/_substrate.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/_transport/interface.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Pinned block contents, metadata-directed extrinsics and UTC timestamps.

use chrono::{DateTime, Datelike, Utc};
use rtao_core::{CoreError, DynValue};
use serde_json::{Value, json};

use crate::{BlockHeader, BlockRef, Client, ClientError, Snapshot, ViewExt, hash_hex, parse_hash};

/// One immutable block's header, timestamp and decoded extrinsics.
#[derive(Clone, Debug, PartialEq)]
pub struct BlockInfo {
    /// Height reported by the fetched header.
    pub number: u64,
    /// Resolved Substrate block hash, rendered as 0x hex.
    pub hash: Option<String>,
    /// UTC timestamp from its Timestamp inherent, or pinned storage fallback.
    pub timestamp: DateTime<Utc>,
    /// Block explorer URL on networks indexed by the pinned Python settings.
    pub explorer_url: Option<String>,
    /// Original header fields with Python's numeric number and resolved hash.
    pub header: Value,
    /// Core-decoded values in block order, with null for undecodable entries.
    pub extrinsics: Vec<Value>,
}

impl Client {
    /// Fetch a numbered block, or the configured head when omitted.
    ///
    /// Extrinsics use the runtime that executed this block. Missing blocks
    /// return `None`; malformed extrinsics retain a null entry at their index.
    // py: sdk/python/bittensor/client.py:349-372
    pub async fn block_info(&self, block: Option<u64>) -> Result<Option<BlockInfo>, ClientError> {
        let Some(snapshot) = select(self, block).await? else {
            return Ok(None);
        };
        fetch(&snapshot).await
    }
}

/// Resolve a requested block without converting an absent height into an error.
pub(crate) async fn select(
    client: &Client,
    block: Option<u64>,
) -> Result<Option<Snapshot>, ClientError> {
    let (method, params) = match block {
        Some(number) => ("chain_getBlockHash", json!([number])),
        None => match client.head_policy() {
            crate::HeadPolicy::Best => ("chain_getBlockHash", json!([])),
            crate::HeadPolicy::Finalized => ("chain_getFinalizedHead", json!([])),
        },
    };
    let value = client.request::<Value>(method, params).await?;
    if value.is_null() {
        return Ok(None);
    }
    let hash = parse_hash(&value)?;
    match client.at(BlockRef::Hash(hash)).await {
        Ok(snapshot) => Ok(Some(snapshot)),
        Err(ClientError::BlockNotFound(_)) => Ok(None),
        Err(error) => Err(error),
    }
}

/// Fetch exactly the already selected identity, including its existing observer.
pub(crate) async fn fetch(snapshot: &Snapshot) -> Result<Option<BlockInfo>, ClientError> {
    let pinned = snapshot.block();
    let hash = hash_hex(pinned.hash);
    let raw = snapshot
        .client()
        .request::<Value>("chain_getBlock", json!([hash]))
        .await?;
    if raw.is_null() {
        return Ok(None);
    }
    let block = raw
        .get("block")
        .and_then(Value::as_object)
        .ok_or_else(|| protocol("chain_getBlock omitted its block object"))?;
    let Some(header) = block.get("header").filter(|header| {
        !header.is_null() && !header.as_object().is_some_and(serde_json::Map::is_empty)
    }) else {
        return Ok(None);
    };
    let header = BlockHeader::from_json_with_hash(header.clone(), pinned.hash)
        .map_err(|error| protocol(error.to_string()))?;
    if header.number != pinned.number {
        return Err(protocol(
            "block body header disagrees with the selected height",
        ));
    }
    let mut normalized_header = header
        .raw
        .as_object()
        .cloned()
        .ok_or_else(|| protocol("block header is not an object"))?;
    normalized_header.insert("number".into(), json!(header.number));
    normalized_header.insert("hash".into(), json!(hash));
    let encoded = match block.get("extrinsics") {
        None | Some(Value::Null) => Vec::new(),
        Some(value) => value
            .as_array()
            .cloned()
            .ok_or_else(|| protocol("block extrinsics are not an array"))?,
    };
    let large = encoded.len() > 64
        || encoded
            .iter()
            .map(|value| value.as_str().map_or(0, str::len))
            .sum::<usize>()
            > 64 * 1024;
    let runtime = pinned.runtime.clone();
    let decode = move || {
        encoded
            .into_iter()
            .map(|value| {
                let decoded = (|| {
                    let text = value
                        .as_str()
                        .ok_or_else(|| protocol("block extrinsic is not hexadecimal text"))?;
                    let bytes =
                        hex::decode(text.strip_prefix("0x").unwrap_or(text)).map_err(|error| {
                            protocol(format!("invalid block extrinsic hexadecimal: {error}"))
                        })?;
                    runtime
                        .core()
                        .decode_extrinsic(&bytes, true)
                        .map(|value| DynValue::new(value).to_json())
                        .map_err(|error| ClientError::from(CoreError::from(error)))
                })();
                decoded.unwrap_or(Value::Null)
            })
            .collect::<Vec<_>>()
    };
    let extrinsics = if large {
        crate::offload(decode)
            .await
            .map_err(|error| protocol(format!("block decoder task failed: {error}")))?
    } else {
        decode()
    };
    let mut inherent = None;
    for value in &extrinsics {
        let Some(call) = value.get("call").filter(|call| call.is_object()) else {
            continue;
        };
        if call.get("call_module").and_then(Value::as_str) != Some("Timestamp") {
            continue;
        }
        if let Some(value) = call
            .get("call_args")
            .and_then(Value::as_array)
            .and_then(|args| args.first())
            .and_then(|argument| argument.get("value"))
            .filter(|value| !value.is_null())
        {
            inherent = Some(
                value
                    .as_u64()
                    .ok_or_else(|| protocol("timestamp inherent is not u64"))?,
            );
            break;
        }
    }
    let millis = match inherent {
        Some(millis) => millis,
        None => snapshot
            .storage_dyn("Timestamp", "Now", &[])
            .await?
            .as_u128()
            .and_then(|millis| u64::try_from(millis).ok())
            .ok_or_else(|| protocol("Timestamp.Now is not u64"))?,
    };
    Ok(Some(BlockInfo {
        number: header.number,
        hash: Some(hash),
        timestamp: timestamp(millis)?,
        explorer_url: rtao_catalog::py::EXPLORER_BLOCK_URLS
            .iter()
            .find(|(label, _)| *label == snapshot.client().network().label())
            .map(|(_, template)| template.replace("{number}", &header.number.to_string())),
        header: Value::Object(normalized_header),
        extrinsics,
    }))
}

fn protocol(detail: impl Into<String>) -> ClientError {
    ClientError::Protocol(detail.into())
}

// CPython first divides the integer milliseconds into binary64 seconds, then
// rounds the fractional second to microseconds with ties to even.
fn timestamp(millis: u64) -> Result<DateTime<Utc>, ClientError> {
    let seconds = rtao_primitives::ratio::integer_division(i128::from(millis), 1000)
        .ok_or_else(|| protocol("timestamp division failed"))?;
    let mut whole = seconds.floor() as i64;
    let mut micros = ((seconds - seconds.floor()) * 1_000_000.0).round_ties_even() as u32;
    if micros == 1_000_000 {
        whole += 1;
        micros = 0;
    }
    DateTime::<Utc>::from_timestamp(whole, micros * 1000)
        .filter(|timestamp| (1..=9999).contains(&timestamp.year()))
        .ok_or_else(|| protocol("timestamp exceeds Python datetime range"))
}
