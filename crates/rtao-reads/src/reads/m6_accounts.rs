// Derived from RaoFoundation/subtensor sdk/python/bittensor/reads/{accounts,chain,identity,neurons}.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/reads/accounts.py @ 8cf229f8ef969389d054a4c161b53ffd164a041f (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/reads/identity.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/reads/identity.py @ a68c6e8c65411a842fbf69269dc385aa13041391 (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/reads/neurons.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/reads/chain.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Account, commitment and neuron catalog reads at immutable state identities.

use futures::TryStreamExt;
use rtao_client::{ClientError, View, ViewExt, decode_typed};
use rtao_core::codec::Value;
use rtao_core::codec::value::to_corpus_json;
use rtao_core::value_ext::{as_bool, as_u128, field};
use rtao_primitives::{
    AccountId, DefaultAccount, DynBalance, MechId, NetUid, NetUidStorageIndex, Symbol, U64F64, Uid,
};
use rtao_runtime::{
    constants,
    models::{CommitmentRegistration, Data, MultisigTimepoint, ToDynamic, bytes_dynamic},
    runtime_apis, storage as st,
};
use serde::Serialize;
use serde_json::Value as JsonValue;

use crate::{ReadError, ReadName, commitments};

fn surface(descriptor: &'static str, detail: impl Into<String>) -> ReadError {
    ReadError::SurfaceChanged {
        descriptor,
        detail: detail.into(),
    }
}

/// A coldkey's swap state: a pending migration, its dispute and the recovery lock.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ColdkeySwapAnnouncement {
    /// Earliest block at which the announced migration can execute; `None`
    /// when the coldkey is recovery-locked without an announcement.
    pub execute_block: Option<u32>,
    /// The stored announcement commitment, as 0x hex. Announcements through
    /// `announce_coldkey_swap_v2` commit to the destination and its policy,
    /// not to the bare key hash. `None` without an announcement.
    pub new_coldkey_hash: Option<String>,
    /// The V2 announcement's decision to import the source auto-stake policy.
    /// The outer option is absent for catalogs before this read was added;
    /// `Some(None)` is a legacy announcement without a policy snapshot.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub import_source_policies: Option<Option<bool>>,
    /// Whether the chain records a nonzero dispute block.
    pub disputed: bool,
    /// Dispute block; zero has the same absent meaning as Python's false value.
    pub dispute_block: Option<u32>,
    /// Whether root's `reset_coldkey_swap_keep_locked` left the coldkey
    /// recovery-locked (stored dispute block 0): it can only make swap calls
    /// until a successful swap or a full reset, even with no announcement.
    pub recovery_locked: bool,
}

/// Read an announcement, dispute and available V2 policy at one block (D-002, D-204).
// py: sdk/python/bittensor/reads/accounts.py:104-142 @ be3a11b1a5300bb58d6fbad3c629e4a926bb2f44
pub async fn coldkey_swap_announcement<V: View + ?Sized>(
    view: &V,
    coldkey_ss58: AccountId,
) -> Result<Option<ColdkeySwapAnnouncement>, ReadError> {
    let snapshot = view.pin().await?;
    let policy_in_catalog = ReadName::ColdkeySwapAnnouncement
        .spec()
        .hits
        .iter()
        .any(|hit| {
            hit.container == "SubtensorModule" && hit.name == "ColdkeySwapV2PolicySnapshots"
        });
    let policy_in_runtime = policy_in_catalog
        && snapshot
            .runtime()
            .await?
            .storage_entry("SubtensorModule", "ColdkeySwapV2PolicySnapshots")
            .is_some();
    let (announcement, dispute, import_source_policies) = if policy_in_runtime {
        let (announcement, dispute, policy) = snapshot
            .fetch((
                st::subtensor_module::COLDKEY_SWAP_ANNOUNCEMENTS.at((coldkey_ss58,)),
                st::subtensor_module::COLDKEY_SWAP_DISPUTES.at((coldkey_ss58,)),
                st::subtensor_module::COLDKEY_SWAP_V2_POLICY_SNAPSHOTS.at((coldkey_ss58,)),
            ))
            .await?;
        (announcement, dispute, Some(policy))
    } else {
        let (announcement, dispute) = snapshot
            .fetch((
                st::subtensor_module::COLDKEY_SWAP_ANNOUNCEMENTS.at((coldkey_ss58,)),
                st::subtensor_module::COLDKEY_SWAP_DISPUTES.at((coldkey_ss58,)),
            ))
            .await?;
        (announcement, dispute, policy_in_catalog.then_some(None))
    };
    let recovery_locked = dispute == Some(0);
    if announcement.is_none() && !recovery_locked {
        return Ok(None);
    }
    let dispute_block = dispute.filter(|block| *block != 0);
    Ok(Some(ColdkeySwapAnnouncement {
        execute_block: announcement
            .as_ref()
            .map(|(execute_block, _)| *execute_block),
        new_coldkey_hash: announcement
            .map(|(_, new_coldkey_hash)| format!("0x{}", hex::encode(new_coldkey_hash.0))),
        import_source_policies,
        disputed: dispute_block.is_some(),
        dispute_block,
        recovery_locked,
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
    /// Concatenated Raw and BigRaw fields as UTF-8 or 0x hex.
    pub data: String,
    /// Original Python-shaped decoded field variants.
    pub fields: Vec<JsonValue>,
}

/// A channel's active commitment and independently retained reveal history.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ChannelCommitment {
    /// Active data, zero or empty when only reveals remain.
    #[serde(flatten)]
    pub active: Commitment,
    /// Plaintext reveals in storage order.
    pub revealed: Vec<ChannelCommitmentReveal>,
}

/// One plaintext entry from the channel's reveal history.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ChannelCommitmentReveal {
    /// Decoded plaintext or 0x hex for non-UTF-8 bytes.
    pub data: String,
    /// Block that recorded the reveal.
    pub block: u64,
}

fn commitment_record(record: CommitmentRegistration) -> Commitment {
    let fields = record.info.fields;
    let mut raw = Vec::new();
    for field in &fields {
        if let Data::Raw(bytes) | Data::BigRaw(bytes) = field {
            raw.extend_from_slice(bytes);
        }
    }
    Commitment {
        block: u64::from(record.block),
        deposit: DynBalance::from_rao(record.deposit.0, NetUid::ROOT),
        data: String::from_utf8(raw)
            .unwrap_or_else(|error| format!("0x{}", hex::encode(error.into_bytes()))),
        fields: fields
            .iter()
            .map(|field| to_corpus_json(&field.to_dynamic()))
            .collect(),
    }
}

/// Read a hotkey's commitment and reveals for one mechanism channel.
// py: sdk/python/bittensor/reads/identity.py:215-293
pub async fn channel_commitment<V: View + ?Sized>(
    view: &V,
    netuid: NetUid,
    mechid: MechId,
    channel: i64,
    hotkey_ss58: Option<AccountId>,
) -> Result<Option<ChannelCommitment>, ReadError> {
    let (index, channel) = commitment_channel(netuid, mechid, channel)?;
    let hotkey = hotkey_ss58.ok_or_else(|| ReadError::InvalidParam {
        name: "hotkey_ss58".into(),
        reason: "hotkey_ss58 is required".into(),
    })?;
    let snapshot = view.pin().await?;
    let runtime = &snapshot.block().runtime;
    let active_storage = runtime.storage_entry("Commitments", "ChannelCommitmentOf");
    let key = (index, hotkey, channel);
    let active = match snapshot
        .call(
            &runtime_apis::subnet_registration_runtime_api::GET_CHANNEL_COMMITMENT,
            (netuid, mechid, channel, hotkey),
        )
        .await
    {
        Ok(Some(bytes)) if active_storage.is_some() => {
            Some(snapshot.decode_commitment_registration(&bytes).await?)
        }
        Ok(Some(bytes)) => {
            let registration = runtime
                .storage_entry("Commitments", "CommitmentOf")
                .ok_or_else(|| surface("get_channel_commitment", "registration type is absent"))?;
            Some(decode_typed::<CommitmentRegistration>(
                runtime,
                registration.value_type,
                &bytes,
            )?)
        }
        Ok(None) => None,
        Err(ClientError::RuntimeApiUnavailable { .. } | ClientError::ApiVersion { .. }) => {
            if active_storage.is_none() {
                return Ok(None);
            }
            snapshot
                .storage(&st::commitments::CHANNEL_COMMITMENT_OF, key)
                .await?
        }
        Err(error) => return Err(error.into()),
    };
    let history = if runtime
        .storage_entry("Commitments", "ChannelRevealedCommitments")
        .is_some()
    {
        snapshot
            .storage(&st::commitments::CHANNEL_REVEALED_COMMITMENTS, key)
            .await?
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    if active.is_none() && history.is_empty() {
        return Ok(None);
    }
    let mut revealed = Vec::with_capacity(history.len());
    for (bytes, block) in history {
        let JsonValue::String(data) = to_corpus_json(&bytes_dynamic(&bytes)) else {
            return Err(surface(
                "Commitments.ChannelRevealedCommitments",
                "reveal text is not a string",
            ));
        };
        revealed.push(ChannelCommitmentReveal { data, block });
    }
    let active = active.map_or_else(
        || Commitment {
            block: 0,
            deposit: DynBalance::from_rao(0, NetUid::ROOT),
            data: String::new(),
            fields: Vec::new(),
        },
        commitment_record,
    );
    Ok(Some(ChannelCommitment { active, revealed }))
}

/// Commitment burn parameters from the runtime quote or stored configuration.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CommitBurn {
    /// Whether the channel's burn is enabled in the returned parameters.
    pub enabled: bool,
    /// Burn amount when enabled, including the current quote when available.
    pub burn: Option<DynBalance>,
    /// Burn field from the selected parameters, matching the upstream read.
    pub stored_burn: DynBalance,
    /// Minimum burn for the channel.
    pub min_burn: DynBalance,
    /// Maximum burn for the channel.
    pub max_burn: DynBalance,
    /// Decay half-life in blocks.
    pub half_life: u64,
    /// Increase multiplier rendered as a fixed decimal string.
    pub increase_mult: String,
    /// Block number recorded in the selected parameters.
    pub last_update_block: u64,
    /// Whether the parameters came from the runtime's decayed quote.
    pub price_is_decayed: bool,
}

/// Read the live burn quote and stored configuration at the same block.
pub async fn commit_burn<V: View + ?Sized>(
    view: &V,
    netuid: NetUid,
    mechid: MechId,
    channel: i64,
) -> Result<Option<CommitBurn>, ReadError> {
    let (index, channel) = commitment_channel(netuid, mechid, channel)?;
    let snapshot = view.pin().await?;
    let (live, stored) = futures::try_join!(
        snapshot.call(
            &runtime_apis::subnet_registration_runtime_api::GET_COMMIT_BURN,
            (netuid, mechid, channel)
        ),
        snapshot.storage(&st::commitments::COMMIT_BURN_CONFIG, ((index, channel),)),
    )?;
    if let Some(value) = live {
        return Ok(Some(burn_parameters(
            value.as_core(),
            true,
            "get_commit_burn",
        )?));
    }
    stored
        .map(|value| burn_parameters(value.as_core(), false, "Commitments.CommitBurnConfig"))
        .transpose()
}

fn commitment_channel(
    netuid: NetUid,
    mechid: MechId,
    channel: i64,
) -> Result<(NetUidStorageIndex, u8), ReadError> {
    let channel = u8::try_from(channel).map_err(|_| ReadError::InvalidParam {
        name: "channel".into(),
        reason: "channel must be in 0..255".into(),
    })?;
    let index =
        NetUidStorageIndex::new(netuid, mechid).map_err(|error| ReadError::InvalidParam {
            name: "netuid/mechid".into(),
            reason: error.to_string(),
        })?;
    Ok((index, channel))
}

fn burn_parameters(
    value: &Value,
    live: bool,
    descriptor: &'static str,
) -> Result<CommitBurn, ReadError> {
    let number = |name: &str, aliases: &[&str]| {
        field(value, name)
            .and_then(|raw| {
                as_u128(
                    aliases
                        .iter()
                        .find_map(|alias| field(raw, alias))
                        .unwrap_or(raw),
                )
            })
            .ok_or_else(|| surface(descriptor, format!("{name} is not an unsigned integer")))
    };
    let native = |name, aliases: &[&str]| {
        u64::try_from(number(name, aliases)?)
            .map_err(|_| surface(descriptor, format!("{name} exceeds u64")))
    };
    let amount = |name| {
        native(name, &["amount", "value"]).map(|rao| DynBalance::from_rao(rao, NetUid::ROOT))
    };
    let enabled = field(value, "enabled")
        .and_then(as_bool)
        .ok_or_else(|| surface(descriptor, "enabled is not a boolean"))?;
    let stored_burn = amount("burn")?;
    Ok(CommitBurn {
        enabled,
        burn: (live || enabled).then(|| stored_burn.clone()),
        stored_burn,
        min_burn: amount("min_burn")?,
        max_burn: amount("max_burn")?,
        half_life: native("half_life", &[])?,
        increase_mult: rtao_primitives::pyfmt::fmt_u64f64_decimal(U64F64::from_bits(number(
            "increase_mult",
            &["bits"],
        )?)),
        last_update_block: native("last_update_block", &[])?,
        price_is_decayed: live,
    })
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

/// Read one live commitment using the registration type declared by the runtime.
// py: sdk/python/bittensor/reads/identity.py:67-78,90-101
pub async fn commitment<V: View + ?Sized>(
    view: &V,
    netuid: NetUid,
    hotkey_ss58: AccountId,
) -> Result<Option<Commitment>, ReadError> {
    Ok(view
        .storage(&st::commitments::COMMITMENT_OF, (netuid, hotkey_ss58))
        .await?
        .map(commitment_record))
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
