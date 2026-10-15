// Derived from RaoFoundation/subtensor sdk/python/bittensor/reads/{accounts,chain,identity,neurons}.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Account, commitment and neuron catalog reads at immutable state identities.

use futures::TryStreamExt;
use rtao_client::{View, ViewExt};
use rtao_core::codec::Value;
use rtao_primitives::{AccountId, DefaultAccount, DynBalance, NetUid, Symbol, Uid};
use rtao_runtime::{constants, models::MultisigTimepoint, storage as st};
use serde::Serialize;
use serde_json::Value as JsonValue;

use crate::{ReadError, commitments};

fn surface(descriptor: &'static str, detail: impl Into<String>) -> ReadError {
    ReadError::SurfaceChanged {
        descriptor,
        detail: detail.into(),
    }
}

/// A pending coldkey migration and its independent dispute state.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ColdkeySwapAnnouncement {
    /// Earliest block at which the announced migration can execute.
    pub execute_block: u32,
    /// BlakeTwo256 commitment to the destination coldkey, as 0x hex.
    pub new_coldkey_hash: String,
    /// Whether the chain records a nonzero dispute block.
    pub disputed: bool,
    /// Dispute block; zero has the same absent meaning as Python's false value.
    pub dispute_block: Option<u32>,
}

/// Read an announcement and dispute together at one block (D-002).
// py: sdk/python/bittensor/reads/accounts.py:81-99
pub async fn coldkey_swap_announcement<V: View + ?Sized>(
    view: &V,
    coldkey_ss58: AccountId,
) -> Result<Option<ColdkeySwapAnnouncement>, ReadError> {
    let (announcement, dispute) = view
        .fetch((
            st::subtensor_module::COLDKEY_SWAP_ANNOUNCEMENTS.at((coldkey_ss58,)),
            st::subtensor_module::COLDKEY_SWAP_DISPUTES.at((coldkey_ss58,)),
        ))
        .await?;
    Ok(announcement.map(|(execute_block, new_coldkey_hash)| {
        let dispute_block = dispute.filter(|block| *block != 0);
        ColdkeySwapAnnouncement {
            execute_block,
            new_coldkey_hash: format!("0x{}", hex::encode(new_coldkey_hash.0)),
            disputed: dispute_block.is_some(),
            dispute_block,
        }
    }))
}

/// Minimum TAO balance required to keep an account alive.
// py: sdk/python/bittensor/reads/accounts.py:43-46
pub async fn existential_deposit<V: View + ?Sized>(view: &V) -> Result<DynBalance, ReadError> {
    let snapshot = view.pin().await?;
    let amount = snapshot
        .constant(&constants::balances::EXISTENTIAL_DEPOSIT)
        .await?;
    Ok(DynBalance::from_rao(amount.0, NetUid::ROOT))
}

/// A pending multisig operation and its reserved native TAO deposit.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MultisigOperation {
    /// Block and extrinsic index of the first approval.
    pub timepoint: MultisigTimepoint,
    /// Reserved TAO deposit, retaining exact native rao.
    pub deposit: DynBalance,
    /// Signer that reserved the deposit.
    pub depositor: AccountId,
    /// Approved signers, in the chain's order.
    pub approvals: Vec<AccountId>,
}

/// Look up a pending operation using a bare or 0x-prefixed call hash.
// py: sdk/python/bittensor/reads/accounts.py:112-126
pub async fn multisig<V: View + ?Sized>(
    view: &V,
    account_ss58: AccountId,
    call_hash: String,
) -> Result<Option<MultisigOperation>, ReadError> {
    let hash = hex::decode(call_hash.strip_prefix("0x").unwrap_or(&call_hash))
        .ok()
        .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok())
        .ok_or_else(|| ReadError::InvalidParam {
            name: "call_hash".into(),
            reason: "expected 32 bytes of hexadecimal, optionally prefixed by 0x".into(),
        })?;
    Ok(view
        .storage(&st::multisig::MULTISIGS, (account_ss58, hash))
        .await?
        .map(|operation| MultisigOperation {
            timepoint: operation.when,
            deposit: DynBalance::from_rao(operation.deposit.0, NetUid::ROOT),
            depositor: operation.depositor,
            approvals: operation.approvals,
        }))
}

/// Whether the selected runtime uses Python's fast 250 ms slot duration.
// py: sdk/python/bittensor/reads/chain.py:25-28; client.py:71,345-347
pub async fn is_fast_blocks<V: View + ?Sized>(view: &V) -> Result<bool, ReadError> {
    let snapshot = view.pin().await?;
    Ok(snapshot.block_time().await? == std::time::Duration::from_millis(250))
}

/// Catalog projection of a decoded block.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BlockSummary {
    /// Canonical block number reported by the header.
    pub number: u64,
    /// The resolved block hash when the node supplied one.
    pub hash: Option<String>,
    /// Python-compatible UTC ISO timestamp.
    pub timestamp: String,
    /// Module.function for decoded calls, or null for undecodable entries.
    pub extrinsics: Vec<Option<String>>,
    /// Explorer link on indexed networks.
    pub explorer_url: Option<String>,
}

/// Describe an explicit block using decoded extrinsics and timestamp fallback.
// py: sdk/python/bittensor/reads/chain.py:37-58
pub async fn block_info<V: View + ?Sized>(
    view: &V,
    block: i64,
) -> Result<Option<BlockSummary>, ReadError> {
    let block = u64::try_from(block).map_err(|_| ReadError::InvalidParam {
        name: "block".into(),
        reason: "block number must be nonnegative".into(),
    })?;
    let snapshot = view.pin().await?;
    let Some(info) = snapshot.block_info(Some(block)).await? else {
        return Ok(None);
    };
    Ok(Some(block_summary(info)))
}

fn block_summary(info: rtao_client::BlockInfo) -> BlockSummary {
    let precision = if info.timestamp.timestamp_subsec_micros() == 0 {
        chrono::SecondsFormat::Secs
    } else {
        chrono::SecondsFormat::Micros
    };
    let extrinsics = info
        .extrinsics
        .iter()
        .map(|value| {
            let call = value.get("call")?.as_object()?;
            let text = |name| call.get(name).and_then(JsonValue::as_str).unwrap_or("None");
            Some(format!("{}.{}", text("call_module"), text("call_function")))
        })
        .collect();
    BlockSummary {
        number: info.number,
        hash: info.hash,
        timestamp: info.timestamp.to_rfc3339_opts(precision, false),
        extrinsics,
        explorer_url: info.explorer_url,
    }
}

/// Global transaction rate limit, in blocks.
// py: sdk/python/bittensor/reads/chain.py:70-76
pub async fn tx_rate_limit<V: View + ?Sized>(view: &V) -> Result<u64, ReadError> {
    Ok(view
        .storage(&st::subtensor_module::TX_RATE_LIMIT, ())
        .await?)
}

/// A hotkey's commitment with its original decoded fields.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Commitment {
    /// Publication block.
    pub block: u64,
    /// Reserved native TAO deposit.
    pub deposit: DynBalance,
    /// Concatenated Raw fields, as UTF-8 or 0x hex; hash fields are not included.
    pub data: String,
    /// Original Python-shaped decoded field variants.
    pub fields: Vec<JsonValue>,
}

fn raw_commitment_data(fields: &[JsonValue]) -> Result<String, ReadError> {
    let mut bytes = Vec::new();
    for field in fields.iter().filter_map(JsonValue::as_object) {
        for (variant, value) in field {
            if variant.starts_with("Raw")
                && let Some(text) = value.as_str()
            {
                bytes.extend(commitments::python_fromhex(
                    text.strip_prefix("0x").unwrap_or(text),
                    variant,
                )?);
            }
        }
    }
    Ok(String::from_utf8(bytes)
        .unwrap_or_else(|error| format!("0x{}", hex::encode(error.into_bytes()))))
}

fn integer_or_zero(
    value: Option<&JsonValue>,
    descriptor: &'static str,
    field: &str,
) -> Result<u64, ReadError> {
    match value {
        None | Some(JsonValue::Null) => Ok(0),
        Some(value) => value
            .as_u64()
            .ok_or_else(|| surface(descriptor, format!("{field} is not a native u64"))),
    }
}

/// Return one live commitment; Raw fields retain Python's UTF-8/hex behavior.
// py: sdk/python/bittensor/reads/identity.py:67-78,90-101
pub async fn commitment<V: View + ?Sized>(
    view: &V,
    netuid: NetUid,
    hotkey_ss58: AccountId,
) -> Result<Option<Commitment>, ReadError> {
    let value = view
        .storage_dyn(
            "Commitments",
            "CommitmentOf",
            &[
                Value::Int(i128::from(netuid.0)),
                Value::Str(hotkey_ss58.to_string()),
            ],
        )
        .await?
        .to_json();
    if value.is_null() {
        return Ok(None);
    }
    let fields = value
        .get("info")
        .and_then(|info| info.get("fields"))
        .and_then(JsonValue::as_array)
        .cloned()
        .unwrap_or_default();
    Ok(Some(Commitment {
        block: integer_or_zero(value.get("block"), "Commitments.CommitmentOf", "block")?,
        deposit: DynBalance::from_rao(
            integer_or_zero(value.get("deposit"), "Commitments.CommitmentOf", "deposit")?,
            NetUid::ROOT,
        ),
        data: raw_commitment_data(&fields)?,
        fields,
    }))
}

/// Every live or revealed-only commitment, newest first, with status and age.
// py: sdk/python/bittensor/reads/identity.py:139-162
pub async fn commitments<V: View + ?Sized>(
    view: &V,
    netuid: NetUid,
) -> Result<Vec<JsonValue>, ReadError> {
    commitments::fetch_commitments(view, netuid)
        .await?
        .values()
        .map(|record| record.to_read_json().map_err(ReadError::from))
        .collect()
}

/// Raw revealed history, preserving the core's UTF-8/hex byte-vector rendering.
// py: sdk/python/bittensor/reads/identity.py:114-116
pub async fn revealed_commitment<V: View + ?Sized>(
    view: &V,
    netuid: NetUid,
    hotkey_ss58: AccountId,
) -> Result<Option<Vec<(String, u64)>>, ReadError> {
    Ok(view
        .storage(
            &st::commitments::REVEALED_COMMITMENTS,
            (netuid, hotkey_ss58),
        )
        .await?
        .map(|history| {
            history
                .into_iter()
                .map(|(bytes, block)| {
                    let text = match rtao_runtime::models::bytes_dynamic(&bytes) {
                        Value::Str(text) => text,
                        _ => format!("0x{}", hex::encode(bytes)),
                    };
                    (text, block)
                })
                .collect()
        }))
}

/// A neuron's associated EVM address and the block at which it was set.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AssociatedEvmKey {
    /// Native H160 address rendered by the core as 0x hex.
    pub evm_address: String,
    /// Last association block.
    pub block: u64,
}

/// EVM association indexed by subnet and neuron UID.
// py: sdk/python/bittensor/reads/neurons.py:133-139
pub async fn associated_evm_key<V: View + ?Sized>(
    view: &V,
    netuid: NetUid,
    uid: Uid,
) -> Result<Option<AssociatedEvmKey>, ReadError> {
    view.storage(
        &st::subtensor_module::ASSOCIATED_EVM_ADDRESS,
        (netuid, uid.0),
    )
    .await?
    .map(|(address, block)| {
        let evm_address = address
            .to_json()
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| {
                surface(
                    "SubtensorModule.AssociatedEvmAddress",
                    "H160 is not hex text",
                )
            })?;
        Ok(AssociatedEvmKey { evm_address, block })
    })
    .transpose()
}

/// Owning coldkey, retaining the zero-account default in JSON (D-001).
///
/// Use [`DefaultAccount::get`] to distinguish an unowned hotkey in typed code.
// py: sdk/python/bittensor/reads/neurons.py:74-77
pub async fn hotkey_owner<V: View + ?Sized>(
    view: &V,
    hotkey_ss58: AccountId,
) -> Result<DefaultAccount, ReadError> {
    Ok(view
        .storage(&st::subtensor_module::OWNER, (hotkey_ss58,))
        .await?)
}

/// Every owned hotkey, preserving chain order and duplicates.
// py: sdk/python/bittensor/reads/neurons.py:85-88
pub async fn owned_hotkeys<V: View + ?Sized>(
    view: &V,
    coldkey_ss58: AccountId,
) -> Result<Vec<AccountId>, ReadError> {
    Ok(view
        .storage(&st::subtensor_module::OWNED_HOTKEYS, (coldkey_ss58,))
        .await?)
}

/// Sorted subnet registrations, omitting explicitly false membership rows.
// py: sdk/python/bittensor/reads/neurons.py:99-102
pub async fn netuids_for_hotkey<V: View + ?Sized>(
    view: &V,
    hotkey_ss58: AccountId,
) -> Result<Vec<NetUid>, ReadError> {
    let rows: Vec<_> = view
        .storage_iter(&st::subtensor_module::IS_NETWORK_MEMBER, (hotkey_ss58,))
        .try_collect()
        .await?;
    let mut netuids = Vec::new();
    for (keys, member) in rows {
        if member {
            let netuid = keys
                .last()
                .and_then(rtao_core::DynValue::as_u128)
                .and_then(|value| u16::try_from(value).ok())
                .ok_or_else(|| surface("SubtensorModule.IsNetworkMember", "key is not a netuid"))?;
            netuids.push(NetUid(netuid));
        }
    }
    netuids.sort();
    Ok(netuids)
}

/// One registered neuron, including its unmodified dynamic runtime record.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Neuron {
    /// Neuron index within the subnet.
    pub uid: Uid,
    /// Registered hotkey.
    pub hotkey: AccountId,
    /// Owning coldkey.
    pub coldkey: AccountId,
    /// Runtime activity flag.
    pub active: bool,
    /// Runtime validator permission.
    pub validator_permit: bool,
    /// Last weight update block.
    pub last_update: u64,
    /// Sum of the runtime stake entries, checked against native u64 limits.
    pub total_stake: DynBalance,
    /// Complete raw record, including full weights and bonds when requested.
    pub raw: JsonValue,
}

/// Every neuron, ordered by UID, using one full or lite runtime API call.
// py: sdk/python/bittensor/reads/neurons.py:41-66
pub async fn neurons<V: View + ?Sized>(
    view: &V,
    netuid: NetUid,
    lite: bool,
) -> Result<Vec<Neuron>, ReadError> {
    let snapshot = view.pin().await?;
    let records = snapshot
        .call_dyn(
            "NeuronInfoRuntimeApi",
            if lite {
                "get_neurons_lite"
            } else {
                "get_neurons"
            },
            &[Value::Int(i128::from(netuid.0))],
        )
        .await?
        .to_json();
    let symbols = snapshot.client().token_symbols().await;
    let symbol = symbols
        .get(&netuid)
        .map(|symbol| Symbol::new(symbol.clone()));
    let rows = records
        .as_array()
        .ok_or_else(|| surface("NeuronInfoRuntimeApi", "result is not a neuron list"))?;
    let mut out = Vec::with_capacity(rows.len());
    for raw in rows {
        let field = |name: &str| {
            raw.get(name)
                .ok_or_else(|| surface("NeuronInfoRuntimeApi", format!("missing {name}")))
        };
        let account = |name: &str| {
            serde_json::from_value::<AccountId>(field(name)?.clone())
                .map_err(|error| surface("NeuronInfoRuntimeApi", format!("{name}: {error}")))
        };
        let flag = |name: &str| {
            field(name)?
                .as_bool()
                .ok_or_else(|| surface("NeuronInfoRuntimeApi", format!("{name} is not a boolean")))
        };
        let uid = field("uid")?
            .as_u64()
            .and_then(|value| u16::try_from(value).ok())
            .ok_or_else(|| surface("NeuronInfoRuntimeApi", "uid is not a native u16"))?;
        let mut stake = 0u64;
        for row in field("stake")?
            .as_array()
            .ok_or_else(|| surface("NeuronInfoRuntimeApi", "stake is not a list"))?
        {
            let amount = row
                .as_array()
                .filter(|row| row.len() == 2)
                .and_then(|row| row[1].as_u64())
                .ok_or_else(|| surface("NeuronInfoRuntimeApi", "stake amount is not native u64"))?;
            stake = stake
                .checked_add(amount)
                .ok_or_else(|| surface("NeuronInfoRuntimeApi", "total stake exceeds native u64"))?;
        }
        let mut total_stake = DynBalance::from_rao(stake, netuid);
        total_stake.symbol = symbol.clone();
        out.push(Neuron {
            uid: Uid(uid),
            hotkey: account("hotkey")?,
            coldkey: account("coldkey")?,
            active: flag("active")?,
            validator_permit: flag("validator_permit")?,
            last_update: integer_or_zero(
                Some(field("last_update")?),
                "NeuronInfoRuntimeApi",
                "last_update",
            )?,
            total_stake,
            raw: raw.clone(),
        });
    }
    out.sort_by_key(|neuron| neuron.uid);
    Ok(out)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // Malformed pinned fixtures must fail these assertions.
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn block_summary_keeps_python_call_and_missing_value_projection() {
        let timestamp = chrono::DateTime::from_timestamp(1700000000, 123000000).unwrap();
        let summary = block_summary(rtao_client::BlockInfo {
            number: 100,
            hash: Some("0xabc".into()),
            timestamp,
            explorer_url: Some("https://www.tao.app/block/100".into()),
            header: json!({"number": 100}),
            extrinsics: vec![
                json!({"call": {"call_module": "Timestamp", "call_function": "set"}}),
                json!({"call": {"call_module": "System", "call_function": "remark", "call_args": []}}),
                json!({"call": {"call_module": "System"}}),
                JsonValue::Null,
                json!({"call": null}),
            ],
        });
        assert_eq!(summary.number, 100);
        assert_eq!(summary.hash.as_deref(), Some("0xabc"));
        assert_eq!(summary.timestamp, "2023-11-14T22:13:20.123000+00:00");
        assert_eq!(
            summary.extrinsics,
            vec![
                Some("Timestamp.set".into()),
                Some("System.remark".into()),
                Some("System.None".into()),
                None,
                None,
            ]
        );
        assert_eq!(
            summary.explorer_url.as_deref(),
            Some("https://www.tao.app/block/100")
        );
    }
}
