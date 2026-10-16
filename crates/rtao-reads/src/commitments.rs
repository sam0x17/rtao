// Derived from RaoFoundation/subtensor sdk/python/tests/unit/test_v446_accounting_and_commitments.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/metagraph.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/reads/identity.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Commitment status, decoded content and same-block storage scans.

use std::{collections::HashMap, fmt, time::Duration};

use chrono::{DateTime, SecondsFormat, Utc};
use futures::TryStreamExt;
use rtao_client::{View, view::ViewExt};
use rtao_core::{DynValue, codec::Value};
use rtao_primitives::{Balance, NetUid, Tao, Uid, drand};
use serde::{Serialize, ser::SerializeMap};
use serde_json::{Value as JsonValue, json};

use crate::ReadError;

/// A malformed decoded commitment field or unsupported numeric range.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommitmentError {
    /// Decoded field containing the error.
    pub field: String,
    /// Decoder diagnostic, retaining Python's hex errors.
    pub message: String,
}

impl fmt::Display for CommitmentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.field, self.message)
    }
}
impl std::error::Error for CommitmentError {}

fn invalid(field: &str, message: impl Into<String>) -> CommitmentError {
    CommitmentError {
        field: field.into(),
        message: message.into(),
    }
}

/// A commitment's current on-chain lifecycle.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CommitmentStatus {
    /// Content was committed in the clear.
    Plain,
    /// A pending encrypted payload is waiting for chain decryption.
    Sealed,
    /// A reveal at or after this commitment made its payload visible.
    Revealed,
    /// The chain retained a terminal reveal failure for audit.
    Failed,
}

impl CommitmentStatus {
    /// Python's lowercase status string.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Plain => "plain",
            Self::Sealed => "sealed",
            Self::Revealed => "revealed",
            Self::Failed => "failed",
        }
    }
}

/// A hotkey's live commitment and decrypted history on one subnet.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct NeuronCommitment {
    /// Core-rendered SS58 hotkey, including deregistered accounts.
    pub hotkey: String,
    /// Subnet containing this record.
    pub netuid: NetUid,
    /// Current registered UID; absent once deregistered.
    pub uid: Option<Uid>,
    /// Publication block, or latest reveal block when the live entry was removed.
    pub block: u64,
    /// Immutable block against which this read was made.
    pub queried_block: u64,
    /// Native chain TAO deposit in rao.
    #[serde(serialize_with = "serialize_deposit")]
    pub deposit: Balance<Tao>,
    /// Visible raw fields followed by rendered hash fields.
    pub data: String,
    /// Whether a pending encrypted field exists, or revealed history replaced its live entry.
    pub encrypted: bool,
    /// Latest pending drand round; terminal failures carry no pending round.
    pub reveal_round: Option<u64>,
    /// Reveals ordered by block and then plaintext, as Python tuple sorting does.
    pub revealed: Vec<(u64, String)>,
    fields: Vec<JsonValue>,
    #[serde(skip)]
    failed_round: Option<u64>,
}

fn serialize_deposit<S: serde::Serializer>(
    deposit: &Balance<Tao>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    rtao_primitives::DynBalance::from_rao(deposit.rao(), NetUid(0)).serialize(serializer)
}

impl NeuronCommitment {
    /// Untouched core corpus representation of decoded field variants.
    pub fn fields(&self) -> &[JsonValue] {
        &self.fields
    }
    /// True when a reveal exists at or after this commitment's block.
    pub fn decrypted(&self) -> bool {
        self.revealed.iter().any(|(block, _)| *block >= self.block)
    }

    /// Reveals take precedence over terminal failures and pending fields.
    pub fn status(&self) -> CommitmentStatus {
        if self.decrypted() {
            CommitmentStatus::Revealed
        } else if self.failed_round.is_some() {
            CommitmentStatus::Failed
        } else if self.encrypted {
            CommitmentStatus::Sealed
        } else {
            CommitmentStatus::Plain
        }
    }

    /// Whether the payload is plain or has been revealed on chain.
    pub fn is_revealed(&self) -> bool {
        matches!(
            self.status(),
            CommitmentStatus::Plain | CommitmentStatus::Revealed
        )
    }

    /// Commitment age, clamped to zero when the queried block precedes publication.
    pub fn age_blocks(&self) -> u64 {
        self.queried_block.saturating_sub(self.block)
    }

    /// Wall-clock age using the extracted Python block time.
    pub fn duration(&self) -> Result<Duration, CommitmentError> {
        Duration::try_from_secs_f64(self.age_blocks() as f64 * rtao_catalog::py::BLOCKTIME)
            .map_err(|error| invalid("duration", error.to_string()))
    }

    /// Latest reveal for this commitment, falling back to its readable plain part.
    pub fn value(&self) -> Option<&str> {
        self.revealed
            .iter()
            .rev()
            .find(|(block, _)| *block >= self.block)
            .map(|(_, text)| text.as_str())
            .or_else(|| (!self.data.is_empty()).then_some(self.data.as_str()))
    }

    /// UTC drand publication time, absent after decryption or when no round is pending.
    pub fn reveals_at(&self) -> Result<Option<DateTime<Utc>>, drand::TimelockError> {
        if self.decrypted() {
            Ok(None)
        } else {
            self.reveal_round.map(drand::reveal_time).transpose()
        }
    }

    /// Python's `reads.identity.commitments` row, including age and reveal state.
    pub fn to_read_json(&self) -> Result<JsonValue, CommitmentError> {
        let moment = self
            .reveals_at()
            .map_err(|error| invalid("reveals_at", error.to_string()))?;
        Ok(json!({
            "hotkey": self.hotkey, "uid": self.uid, "commitment": self.value(), "block": self.block,
            "duration": age_text(self.duration()?.as_secs()), "is_revealed": self.is_revealed(),
            "status": self.status(), "reveals_at": moment.map(|moment| moment.to_rfc3339_opts(SecondsFormat::Secs, false)),
        }))
    }
}

/// Ordered hotkey map; lookup of an absent hotkey returns `None`.
///
/// Decoder order follows storage rows followed by revealed-only entries.
/// [`fetch_commitments`] sorts this map newest first, preserving ties.
#[derive(Clone, Debug, Default)]
pub struct CommitmentMap {
    entries: Vec<(String, NeuronCommitment)>,
    indices: HashMap<String, usize>,
}

impl CommitmentMap {
    /// Number of hotkeys with live records or revealed history.
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    /// Whether no commitments exist.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    /// Look up a hotkey; missing entries are an ordinary absence.
    pub fn get(&self, hotkey: &str) -> Option<&NeuronCommitment> {
        self.indices
            .get(hotkey)
            .map(|index| &self.entries[*index].1)
    }
    /// Mutably look up a hotkey, for metagraph UID projection.
    pub fn get_mut(&mut self, hotkey: &str) -> Option<&mut NeuronCommitment> {
        self.indices
            .get(hotkey)
            .map(|index| &mut self.entries[*index].1)
    }
    /// Ordered hotkey/commitment pairs.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &NeuronCommitment)> {
        self.entries
            .iter()
            .map(|(key, value)| (key.as_str(), value))
    }
    /// Ordered commitments.
    pub fn values(&self) -> impl Iterator<Item = &NeuronCommitment> {
        self.entries.iter().map(|(_, value)| value)
    }
    /// Ordered mutable commitments.
    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut NeuronCommitment> {
        self.entries.iter_mut().map(|(_, value)| value)
    }
    /// Consume this map without discarding its stable order.
    pub fn into_entries(self) -> Vec<(String, NeuronCommitment)> {
        self.entries
    }
    fn insert(&mut self, hotkey: String, commitment: NeuronCommitment) {
        if let Some(existing) = self.get_mut(&hotkey) {
            *existing = commitment;
        } else {
            self.indices.insert(hotkey.clone(), self.entries.len());
            self.entries.push((hotkey, commitment));
        }
    }
    fn newest_first(&mut self) {
        self.entries
            .sort_by_key(|(_, commitment)| std::cmp::Reverse(commitment.block));
        for (index, (hotkey, _)) in self.entries.iter().enumerate() {
            self.indices.insert(hotkey.clone(), index);
        }
    }
}

impl Serialize for CommitmentMap {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.len()))?;
        for (hotkey, commitment) in &self.entries {
            map.serialize_entry(hotkey, commitment)?;
        }
        map.end()
    }
}

/// Decode corpus-shaped storage rows without performing any I/O.
// py: sdk/python/bittensor/metagraph.py:408-463 (_commitments)
pub fn decode_commitments(
    netuid: NetUid,
    committed: &[(String, JsonValue)],
    revealed: &[(String, JsonValue)],
    queried_block: u64,
) -> Result<CommitmentMap, CommitmentError> {
    let mut history: Vec<(String, Vec<(u64, String)>)> = Vec::new();
    for (hotkey, entries) in revealed {
        let entries = sequence_or_empty(entries, "revealed")?;
        let decoded = entries
            .iter()
            .map(decode_revealed_entry)
            .collect::<Result<Vec<_>, _>>()?;
        if let Some((_, old)) = history.iter_mut().find(|(key, _)| key == hotkey) {
            *old = decoded;
        } else {
            history.push((hotkey.clone(), decoded));
        }
    }
    let mut out = CommitmentMap::default();
    for (hotkey, record) in committed {
        let Some(record) = record.as_object() else {
            continue;
        };
        let fields = sequence_or_empty(
            record
                .get("info")
                .and_then(|info| info.get("fields"))
                .unwrap_or(&JsonValue::Null),
            "info.fields",
        )?
        .to_vec();
        let reveal_round = field_round(&fields, "TimelockEncrypted")?;
        let mut revealed = history
            .iter()
            .find(|(key, _)| key == hotkey)
            .map(|(_, entries)| entries.clone())
            .unwrap_or_default();
        revealed.sort();
        out.insert(
            hotkey.clone(),
            NeuronCommitment {
                hotkey: hotkey.clone(),
                netuid,
                uid: None,
                block: integer_or_zero(record.get("block"), "block")?,
                queried_block,
                deposit: Balance::from_rao(integer_or_zero(record.get("deposit"), "deposit")?),
                data: decode_fields(&fields)?,
                encrypted: reveal_round.is_some(),
                reveal_round,
                revealed,
                failed_round: field_round(&fields, "TimelockRevealFailed")?,
                fields,
            },
        );
    }
    for (hotkey, mut entries) in history {
        if out.get(&hotkey).is_some() || entries.is_empty() {
            continue;
        }
        entries.sort();
        let block = entries
            .iter()
            .map(|(block, _)| *block)
            .max()
            .unwrap_or_default();
        out.insert(
            hotkey.clone(),
            NeuronCommitment {
                hotkey,
                netuid,
                uid: None,
                block,
                queried_block,
                deposit: Balance::from_rao(0),
                data: String::new(),
                encrypted: true,
                reveal_round: None,
                revealed: entries,
                fields: Vec::new(),
                failed_round: None,
            },
        );
    }
    Ok(out)
}

fn sequence_or_empty<'a>(
    value: &'a JsonValue,
    field: &str,
) -> Result<&'a [JsonValue], CommitmentError> {
    if value.is_null() || value == &json!(false) || value == &json!("") || value == &json!(0) {
        return Ok(&[]);
    }
    value
        .as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| invalid(field, "expected a sequence"))
}

fn integer_or_zero(value: Option<&JsonValue>, field: &str) -> Result<u64, CommitmentError> {
    match value {
        None | Some(JsonValue::Null) => Ok(0),
        Some(JsonValue::String(value)) if value.is_empty() => Ok(0),
        Some(value) => native_integer(value, field),
    }
}

fn native_integer(value: &JsonValue, field: &str) -> Result<u64, CommitmentError> {
    match value {
        JsonValue::Bool(value) => Ok(u64::from(*value)),
        JsonValue::String(value) => value
            .trim()
            .parse()
            .map_err(|_| invalid(field, "expected a native unsigned chain integer")),
        value => value
            .as_u64()
            .ok_or_else(|| invalid(field, "expected a native unsigned chain integer")),
    }
}

fn field_items(entry: &JsonValue) -> impl Iterator<Item = (&str, Option<&JsonValue>)> {
    let mut items = Vec::new();
    if let Some(variant) = entry.as_str().filter(|variant| !variant.is_empty()) {
        items.push((variant, None));
    }
    if let Some(object) = entry.as_object() {
        items.extend(
            object
                .iter()
                .map(|(variant, value)| (variant.as_str(), Some(value))),
        );
    }
    items.into_iter()
}

fn field_round(fields: &[JsonValue], variant_name: &str) -> Result<Option<u64>, CommitmentError> {
    let mut round = None;
    for (variant, value) in fields.iter().flat_map(field_items) {
        if variant == variant_name
            && let Some(value) = value.filter(|value| value.is_object())
        {
            let value = value
                .get("reveal_round")
                .ok_or_else(|| invalid(variant, "missing reveal_round"))?;
            let current = native_integer(value, "reveal_round")?;
            round = Some(round.map_or(current, |old: u64| old.max(current)));
        }
    }
    Ok(round)
}

/// Concatenate Raw fields and render non-timelock string payloads as hashes.
// py: sdk/python/bittensor/metagraph.py:494-521 (_decode_fields)
pub fn decode_fields(fields: &[JsonValue]) -> Result<String, CommitmentError> {
    let mut raw = Vec::new();
    let mut hashes = Vec::new();
    for (variant, value) in fields.iter().flat_map(field_items) {
        let Some(value) = value.and_then(JsonValue::as_str) else {
            continue;
        };
        if variant.starts_with("Raw") {
            raw.extend(python_fromhex(
                value.strip_prefix("0x").unwrap_or(value),
                variant,
            )?);
        } else if variant != "TimelockEncrypted" {
            hashes.push(format!("{}:{value}", variant.to_lowercase()));
        }
    }
    let mut parts = Vec::new();
    if !raw.is_empty() {
        parts.push(utf8_or_hex(raw));
    }
    parts.extend(hashes);
    Ok(parts.join("\n"))
}

/// Decode a `(SCALE-framed plaintext, reveal_block)` pair.
///
/// As in Python, the compact prefix chooses only the number of bytes to skip;
/// its encoded length is not used to truncate or validate the plaintext.
// py: sdk/python/bittensor/metagraph.py:546-571 (_revealed_entry)
pub fn decode_revealed_entry(entry: &JsonValue) -> Result<(u64, String), CommitmentError> {
    let entry = entry
        .as_array()
        .filter(|entry| entry.len() == 2)
        .ok_or_else(|| invalid("revealed", "expected (data, reveal_block) pair"))?;
    let data = entry[0]
        .as_str()
        .ok_or_else(|| invalid("revealed.data", "expected core corpus bytes or text"))?;
    let raw = if let Some(hex) = data.strip_prefix("0x") {
        python_fromhex(hex, "revealed.data")?
    } else {
        data.as_bytes().to_vec()
    };
    let offset = raw.first().map_or(0, |byte| match byte & 3 {
        0 => 1,
        1 => 2,
        2 => 4,
        _ => 1 + usize::from(byte >> 2) + 4,
    });
    let plaintext = raw.get(offset..).unwrap_or_default().to_vec();
    Ok((
        native_integer(&entry[1], "reveal_block")?,
        utf8_or_hex(plaintext),
    ))
}

fn utf8_or_hex(raw: Vec<u8>) -> String {
    String::from_utf8(raw).unwrap_or_else(|error| format!("0x{}", hex::encode(error.into_bytes())))
}

pub(crate) fn python_fromhex(text: &str, field: &str) -> Result<Vec<u8>, CommitmentError> {
    let chars: Vec<_> = text.chars().collect();
    let mut index = 0;
    let mut bytes = Vec::new();
    let error = |position| {
        invalid(
            field,
            format!("non-hexadecimal number found in fromhex() arg at position {position}"),
        )
    };
    // CPython rejects the first non-ASCII character before inspecting any
    // hexadecimal pairs, even when an earlier ASCII character is malformed.
    if let Some(position) = chars.iter().position(|character| !character.is_ascii()) {
        return Err(error(position));
    }
    while index < chars.len() {
        if matches!(chars[index], '\t'..='\r' | ' ') {
            index += 1;
            continue;
        }
        let upper = chars[index]
            .to_digit(16)
            .filter(|_| chars[index].is_ascii())
            .ok_or_else(|| error(index))?;
        index += 1;
        let lower = chars
            .get(index)
            .and_then(|digit| digit.to_digit(16).filter(|_| digit.is_ascii()))
            .ok_or_else(|| error(index))?;
        bytes.push((upper * 16 + lower) as u8);
        index += 1;
    }
    Ok(bytes)
}

fn age_text(mut seconds: u64) -> String {
    if seconds < 60 {
        return "<1m".into();
    }
    let mut parts = Vec::new();
    for (suffix, size) in [("d", 86400), ("h", 3600), ("m", 60)] {
        if seconds >= size {
            parts.push(format!("{}{suffix}", seconds / size));
            seconds %= size;
        }
        if parts.len() == 2 {
            break;
        }
    }
    parts.join(" ")
}

async fn scan<V: View + ?Sized>(
    view: &V,
    pallet: &str,
    item: &str,
    netuid: NetUid,
) -> Result<Vec<(String, JsonValue)>, ReadError> {
    let prefix = vec![Value::Uint(u128::from(netuid.0))];
    let rows: Vec<_> = view
        .storage_iter_dyn(pallet, item, prefix)
        .try_collect()
        .await?;
    let large =
        rows.len() > 64 || rows.iter().map(|(_, raw, _, _)| raw.len()).sum::<usize>() > 64 * 1024;
    let decode = move || {
        rows.into_iter()
            .map(|(keys, raw, runtime, info)| {
                let hotkey = keys
                    .last()
                    .map(rtao_core::codec::value::to_corpus_json)
                    .and_then(|value| value.as_str().map(str::to_owned))
                    .ok_or_else(|| {
                        invalid(
                            "hotkey",
                            "core storage-key decoder did not render an account",
                        )
                    })?;
                let record = rtao_core::ops::storage::decode_storage_bytes(
                    runtime.core(),
                    &info,
                    Some(&raw),
                )
                .map_err(rtao_core::CoreError::from)
                .map_err(rtao_client::ClientError::from)?;
                Ok((hotkey, DynValue::new(record).to_json()))
            })
            .collect()
    };
    if large {
        rtao_client::offload(decode).await.map_err(|error| {
            rtao_client::ClientError::Protocol(format!("commitment decoder task failed: {error}"))
        })?
    } else {
        decode()
    }
}

/// Scan both commitment maps at one immutable block, optionally fetching current UIDs.
///
/// The view is pinned before any map request; snapshots retain their own hash.
// py: sdk/python/bittensor/metagraph.py:273-291 (_fetch_commitment_map)
pub async fn fetch_commitment_map<V: View + ?Sized>(
    view: &V,
    netuid: NetUid,
    with_uids: bool,
) -> Result<CommitmentMap, ReadError> {
    let snapshot = view.pin().await?;
    let uid_scan = async {
        if with_uids {
            scan(&snapshot, "SubtensorModule", "Uids", netuid).await
        } else {
            Ok(Vec::new())
        }
    };
    let (committed, revealed, uid_rows) = tokio::try_join!(
        scan(&snapshot, "Commitments", "CommitmentOf", netuid),
        scan(&snapshot, "Commitments", "RevealedCommitments", netuid),
        uid_scan,
    )?;
    let large = committed.len() + revealed.len() + uid_rows.len() > 64
        || committed
            .iter()
            .chain(&revealed)
            .map(|(_, value)| json_size_hint(value))
            .sum::<usize>()
            > 64 * 1024;
    let queried_block = snapshot.block().number;
    let project = move || {
        let mut commitments = decode_commitments(netuid, &committed, &revealed, queried_block)?;
        for (hotkey, uid) in uid_rows {
            let uid = integer_or_zero(Some(&uid), "uid")?;
            let uid = u16::try_from(uid).map_err(|_| invalid("uid", "UID exceeds native u16"))?;
            if let Some(commitment) = commitments.get_mut(&hotkey) {
                commitment.uid = Some(Uid(uid));
            }
        }
        Ok(commitments)
    };
    if large {
        rtao_client::offload(project).await.map_err(|error| {
            rtao_client::ClientError::Protocol(format!(
                "commitment projection task failed: {error}"
            ))
        })?
    } else {
        project()
    }
}

fn json_size_hint(value: &JsonValue) -> usize {
    let mut size = 0usize;
    let mut pending = vec![value];
    while let Some(value) = pending.pop() {
        size = size.saturating_add(match value {
            JsonValue::String(text) => text.len(),
            JsonValue::Number(number) => number.as_str().len(),
            JsonValue::Array(values) => {
                pending.extend(values);
                values.len()
            }
            JsonValue::Object(values) => {
                pending.extend(values.values());
                values.len()
            }
            _ => 1,
        });
        if size > 64 * 1024 {
            break;
        }
    }
    size
}

/// Every live or previously revealed commitment, sorted newest first.
// py: sdk/python/bittensor/metagraph.py:258-270 (fetch_commitments)
pub async fn fetch_commitments<V: View + ?Sized>(
    view: &V,
    netuid: NetUid,
) -> Result<CommitmentMap, ReadError> {
    let mut commitments = fetch_commitment_map(view, netuid, true).await?;
    commitments.newest_first();
    Ok(commitments)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // Assertions must fail when a fixture cannot decode.
mod tests {
    use super::*;

    fn record(block: u64, fields: JsonValue) -> JsonValue {
        json!({"block": block, "deposit": 7, "info": {"fields": fields}})
    }

    // pyt: sdk/python/tests/unit/test_v446_accounting_and_commitments.py::test_failed_timelock_commitment_is_terminal_and_not_revealed
    #[test]
    fn test_failed_timelock_commitment_is_terminal_and_not_revealed() -> Result<(), CommitmentError>
    {
        let commitments = decode_commitments(
            NetUid(42),
            &[(
                "5Fhotkey".into(),
                json!({
                    "block": 100, "deposit": 0, "info": {"fields": [{"TimelockRevealFailed": {"encrypted": "0x0102", "reveal_round": 123}}]},
                }),
            )],
            &[],
            110,
        )?;
        let commitment = commitments.get("5Fhotkey").unwrap();
        assert_eq!(commitment.status().as_str(), "failed");
        assert!(!commitment.is_revealed());
        assert_eq!(commitment.value(), None);
        assert_eq!(commitment.reveals_at().unwrap(), None);
        Ok(())
    }

    #[test]
    fn status_precedence_preserves_plaintext_and_ignores_old_reveals() -> Result<(), CommitmentError>
    {
        let fields = json!([
            {"Raw4": "0x706c6169"}, {"Raw1": "0x6e"},
            {"TimelockEncrypted": {"reveal_round": 12}},
            {"TimelockEncrypted": {"reveal_round": 15}},
            {"TimelockRevealFailed": {"reveal_round": 14}},
        ]);
        let old = json!([["0x0c6f6c64", 99]]);
        let map = decode_commitments(
            NetUid(1),
            &[("alice".into(), record(100, fields.clone()))],
            &[("alice".into(), old)],
            110,
        )?;
        let commitment = map.get("alice").unwrap();
        assert_eq!(commitment.status(), CommitmentStatus::Failed);
        assert_eq!(commitment.value(), Some("plain"));
        assert_eq!(commitment.reveal_round, Some(15));
        assert_eq!(commitment.age_blocks(), 10);
        assert_eq!(commitment.duration()?.as_secs(), 120);
        assert_eq!(
            commitment.reveals_at().unwrap(),
            Some(drand::reveal_time(15).unwrap())
        );
        let history = json!([
            ["0x186e6577657374", 105],
            ["0x0c6f6c64", 99],
            ["0x106c617374", 105]
        ]);
        let map = decode_commitments(
            NetUid(1),
            &[("alice".into(), record(100, fields))],
            &[("alice".into(), history)],
            110,
        )?;
        let commitment = map.get("alice").unwrap();
        assert_eq!(commitment.status(), CommitmentStatus::Revealed);
        assert!(commitment.is_revealed());
        assert_eq!(commitment.value(), Some("newest"));
        assert_eq!(commitment.reveals_at().unwrap(), None);
        Ok(())
    }

    #[test]
    fn revealed_only_records_use_latest_block_and_zero_deposit() -> Result<(), CommitmentError> {
        let map = decode_commitments(
            NetUid(42),
            &[("ignored".into(), JsonValue::Null)],
            &[
                (
                    "alice".into(),
                    json!([["0x1064617461", 50], ["0x0c6f6c64", 40]]),
                ),
                ("empty".into(), json!([])),
            ],
            49,
        )?;
        assert_eq!(map.len(), 1);
        assert!(map.get("missing").is_none());
        let commitment = map.get("alice").unwrap();
        assert_eq!(commitment.block, 50);
        assert_eq!(commitment.age_blocks(), 0);
        assert_eq!(commitment.deposit.rao(), 0);
        assert!(commitment.encrypted);
        assert_eq!(commitment.status(), CommitmentStatus::Revealed);
        assert_eq!(commitment.value(), Some("data"));
        assert!(commitment.fields.is_empty());
        Ok(())
    }

    #[test]
    fn raw_fields_join_before_utf8_decode_and_hashes_follow() -> Result<(), CommitmentError> {
        assert_eq!(
            decode_fields(&[
                json!({"Raw1": "0xcf"}),
                json!({"Raw1": "0x84"}),
                json!({"Sha256": "0xab"}),
                json!("ResetBondsFlag"),
                json!("None")
            ])?,
            "τ\nsha256:0xab"
        );
        assert_eq!(
            decode_fields(&[json!({"Raw2": "0xff00"}), json!({"BlakeTwo256": "0x12"})])?,
            "0xff00\nblaketwo256:0x12"
        );
        assert_eq!(decode_fields(&[json!({"Raw3": "61 62\t63"})])?, "abc");
        assert_eq!(
            decode_fields(&[json!({"Raw1": "a"})]).unwrap_err().message,
            "non-hexadecimal number found in fromhex() arg at position 1"
        );
        assert_eq!(
            decode_fields(&[json!({"Raw1": "a b"})])
                .unwrap_err()
                .message,
            "non-hexadecimal number found in fromhex() arg at position 1"
        );
        assert_eq!(
            decode_fields(&[json!({"Raw1": "zz"})]).unwrap_err().field,
            "Raw1"
        );
        Ok(())
    }

    #[test]
    fn hex_fields_accept_all_python_ascii_separators_only_between_complete_bytes()
    -> Result<(), CommitmentError> {
        for separator in [' ', '\t', '\n', '\u{b}', '\u{c}', '\r'] {
            let field = format!("{separator}61{separator}62{separator}63{separator}");
            assert_eq!(decode_fields(&[json!({"Raw3": field})])?, "abc");
            let malformed = format!("6{separator}1");
            let error = python_fromhex(&malformed, "Raw1").unwrap_err();
            assert_eq!(error.field, "Raw1");
            assert_eq!(
                error.message,
                "non-hexadecimal number found in fromhex() arg at position 1"
            );
        }
        for separator in ['\u{1c}', '\u{1d}', '\u{1e}', '\u{1f}'] {
            let malformed = format!("61{separator}62");
            assert_eq!(
                python_fromhex(&malformed, "Raw2").unwrap_err().message,
                "non-hexadecimal number found in fromhex() arg at position 2"
            );
        }
        Ok(())
    }

    #[test]
    fn hex_fields_report_first_non_ascii_before_earlier_ascii_syntax_errors() {
        for (input, position) in [
            ("ggé", 2),
            ("0 é", 2),
            ("g\t00😀", 4),
            ("00\u{a0}11", 2),
            ("égg", 0),
        ] {
            let error = python_fromhex(input, "Raw3").unwrap_err();
            assert_eq!(error.field, "Raw3");
            assert_eq!(
                error.message,
                format!("non-hexadecimal number found in fromhex() arg at position {position}"),
                "{input:?}"
            );
        }
    }

    #[test]
    fn revealed_compact_modes_follow_python_skip_semantics() -> Result<(), CommitmentError> {
        for raw in [
            "0x0461",
            "0x010061",
            "0x0200000061",
            "0x030000000061",
            "0x07000000000061",
        ] {
            assert_eq!(
                decode_revealed_entry(&json!([raw, 9]))?,
                (9, "a".into()),
                "{raw}"
            );
        }
        assert_eq!(
            decode_revealed_entry(&json!(["0x", 0]))?,
            (0, String::new())
        );
        assert_eq!(
            decode_revealed_entry(&json!(["0xff", 1]))?,
            (1, String::new())
        );
        assert_eq!(
            decode_revealed_entry(&json!(["0x04ff", 1]))?,
            (1, "0xff".into())
        );
        assert_eq!(
            decode_revealed_entry(&json!(["hello", 2]))?,
            (2, "ello".into())
        );
        // Python does not enforce the compact length: 1-byte length, 3-byte content.
        assert_eq!(
            decode_revealed_entry(&json!(["0x04616263", 3]))?,
            (3, "abc".into())
        );
        assert!(decode_revealed_entry(&json!(["0x00"])).is_err());
        assert!(decode_revealed_entry(&json!(["0x0g", 1])).is_err());
        assert!(decode_revealed_entry(&json!(["0x", ""])).is_err());
        Ok(())
    }

    #[test]
    fn json_rows_match_python_fields_and_reveal_timestamp() -> Result<(), CommitmentError> {
        let map = decode_commitments(
            NetUid(1),
            &[(
                "alice".into(),
                record(10, json!([{"TimelockEncrypted": {"reveal_round": 1}}])),
            )],
            &[],
            15,
        )?;
        let commitment = map.get("alice").unwrap();
        let mut expected = json!({"hotkey": "alice", "uid": null, "commitment": null, "block": 10, "duration": "1m", "is_revealed": false, "status": "sealed", "reveals_at": null});
        expected["reveals_at"] = json!(
            drand::reveal_time(1)
                .unwrap()
                .to_rfc3339_opts(SecondsFormat::Secs, false)
        );
        assert_eq!(commitment.to_read_json()?, expected);
        let json = serde_json::to_value(commitment).unwrap();
        assert_eq!(
            json["deposit"],
            json!(rtao_primitives::DynBalance::from_rao(7, NetUid(0)).to_python_string())
        );
        assert!(json.get("failed_round").is_none());
        assert_eq!(age_text(59), "<1m");
        assert_eq!(age_text(3 * 86400 + 4 * 3600 + 5 * 60), "3d 4h");
        assert_eq!(age_text(12 * 60), "12m");
        Ok(())
    }

    #[test]
    fn invalid_native_balances_and_missing_rounds_fail_explicitly() {
        for amount in [json!(-1), json!("18446744073709551616")] {
            let value = json!({"block": 1, "deposit": amount, "info": {"fields": []}});
            assert_eq!(
                decode_commitments(NetUid(1), &[("alice".into(), value)], &[], 1)
                    .unwrap_err()
                    .field,
                "deposit"
            );
        }
        let value = record(1, json!([{"TimelockEncrypted": {}}]));
        assert_eq!(
            decode_commitments(NetUid(1), &[("alice".into(), value)], &[], 1)
                .unwrap_err()
                .message,
            "missing reveal_round"
        );
        for round in [json!(null), json!("")] {
            let value = record(1, json!([{"TimelockEncrypted": {"reveal_round": round}}]));
            assert_eq!(
                decode_commitments(NetUid(1), &[("alice".into(), value)], &[], 1)
                    .unwrap_err()
                    .field,
                "reveal_round"
            );
        }
    }

    #[test]
    fn newest_first_sort_is_stable_and_duplicate_rows_replace_in_place()
    -> Result<(), CommitmentError> {
        let mut map = decode_commitments(
            NetUid(1),
            &[
                ("a".into(), record(5, json!([]))),
                ("b".into(), record(10, json!([]))),
                ("c".into(), record(10, json!([]))),
                ("a".into(), record(9, json!([]))),
            ],
            &[],
            20,
        )?;
        map.newest_first();
        assert_eq!(
            map.iter().map(|(hotkey, _)| hotkey).collect::<Vec<_>>(),
            ["b", "c", "a"]
        );
        Ok(())
    }
}
