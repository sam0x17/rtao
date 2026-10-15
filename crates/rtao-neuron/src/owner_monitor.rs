//! Pinned owner state and resumable finalized owner events.
//!
//! Conviction comes from the chain's roll-forward runtime API. The ownership
//! boundary retains all U64F64 bits and uses saturating native alpha balances.
// Behavioral sources: RaoFoundation/subtensor @ 850f2755bb05a3851f948489f0ab27b0645cebce,
// sdk/python/bittensor/reads/locks.py; pallets/alpha-assets/src/lib.rs:316-319;
// pallets/proxy/src/lib.rs:88-95, 576-584, 844-855.

use std::{
    collections::{BTreeSet, VecDeque},
    pin::Pin,
};

use futures::{Stream, StreamExt};
use rtao_client::{
    BlockHeader, BlockRef, BlockStream, Client, ClientError, EventRecord, Finality, HeadError,
    HeadProvider, Snapshot, View, ViewExt,
};
use rtao_core::{DynValue, codec::Value, value_ext::ValueExt as _};
use rtao_primitives::{AccountId, AlphaBalance, H256, NetUid, TaoBalance, U64F64};
use rtao_runtime::{
    EventDescriptor, StorageDescriptor, StorageSurface, SurfaceDescriptor,
    events::subtensor_module as ev,
    runtime_apis::stake_info_runtime_api as api,
    storage::{proxy, subtensor_module as st},
};
use tokio_util::sync::CancellationToken;

use crate::{PruneError, PruneRisk};

// AlphaAssets is outside the generated pallet scope. Its pinned source contract
// uses the same NetUid/AlphaBalance shape as SubnetAlphaOut, with Twox64Concat.
const ALPHA_BURNED: StorageDescriptor<(NetUid,), AlphaBalance> =
    StorageDescriptor::new(StorageSurface {
        pallet: "AlphaAssets",
        name: "AlphaBurned",
        prefix: "AlphaAssets",
        key_type_idents: &["NetUid"],
        value_type_ident: "AlphaBalance",
        hashers: &["Twox64Concat"],
        modifier: "Default",
        default_bytes: &[0; 8],
        shape: st::SUBNET_ALPHA_OUT.surface.shape,
    });
const OWNERSHIP_AGE: u64 = 7_200 * 365 + 1_800;
const PROXY_DEFINITIONS: StorageDescriptor<(AccountId,), DynValue> =
    StorageDescriptor::new(proxy::PROXIES.surface());

/// A monitor read, event contract, or finalized follower failed.
#[derive(Debug, thiserror::Error)]
pub enum OwnerMonitorError {
    /// The identifier is invalid or refers to the root network.
    #[error("owner monitoring requires a valid non-root subnet")]
    InvalidSubnet,
    /// The subnet is absent at the observation block.
    #[error("subnet {0} is not registered at the observation block")]
    SubnetMissing(NetUid),
    /// Decoded state or an event does not match the supported source contract.
    #[error("invalid owner monitor state: {0}")]
    InvalidState(String),
    /// A resume cursor is ahead of finalized history or belongs to another block.
    #[error("invalid finalized owner event cursor: {0}")]
    InvalidCursor(String),
    /// The follower ended without caller cancellation.
    #[error("finalized owner event follower ended")]
    FollowerEnded,
    /// A chain read or runtime descriptor failed.
    #[error(transparent)]
    Client(#[from] ClientError),
    /// Finalized header tracking failed.
    #[error(transparent)]
    Head(#[from] HeadError),
    /// Pruning inputs could not be observed.
    #[error(transparent)]
    Prune(#[from] PruneError),
}

/// Exact conviction and age gates for the chain-selected leader.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnerConviction {
    /// Leader selected by the chain's conviction runtime API.
    pub leader_hotkey: Option<AccountId>,
    /// Rolled-forward leader conviction, including fractional alpha rao bits.
    pub leader_conviction: U64F64,
    /// Alpha outside the pool.
    pub alpha_out: AlphaBalance,
    /// Protocol-owned alpha excluded from the ownership denominator.
    pub protocol_alpha: AlphaBalance,
    /// Burned alpha excluded from the ownership denominator.
    pub alpha_burned: AlphaBalance,
    /// `alpha_out.saturating_sub(protocol_alpha).saturating_sub(alpha_burned)`.
    pub eligible_alpha: AlphaBalance,
    /// Exact 18% threshold, rounded down only at the U64F64 bit boundary.
    pub threshold: U64F64,
    /// Leader conviction is strictly greater than 18% of nonzero eligible alpha.
    pub clears_threshold: bool,
    /// Registration block read at the observation hash.
    pub registered_at: u64,
    /// First ownership-transfer age, using the pallet's saturating block addition.
    pub ownership_changeable_at: u64,
    /// The subnet has reached the pallet's one-year age.
    pub old_enough: bool,
    /// A different hotkey clears both gates at this block.
    ///
    /// Transfer is evaluated by the pallet after an epoch; this flag does not
    /// claim that a transfer has already occurred or will occur at the next head.
    pub challenger_clears_gates: bool,
}

impl OwnerConviction {
    /// Evaluate the strict ownership boundary without floating-point conversion.
    #[allow(clippy::too_many_arguments)]
    pub fn evaluate(
        block: u64,
        registered_at: u64,
        owner_hotkey: AccountId,
        leader_hotkey: Option<AccountId>,
        leader_conviction: U64F64,
        alpha_out: AlphaBalance,
        protocol_alpha: AlphaBalance,
        alpha_burned: AlphaBalance,
    ) -> Self {
        let eligible = alpha_out
            .0
            .saturating_sub(protocol_alpha.0)
            .saturating_sub(alpha_burned.0);
        // eligible << 64 fits u128. Divide before multiplying to retain the
        // rational threshold without overflowing for u64::MAX alpha.
        let scaled = u128::from(eligible) << 64;
        let threshold = scaled / 100 * 18 + scaled % 100 * 18 / 100;
        let clears = eligible != 0 && leader_hotkey.is_some() && leader_conviction.bits > threshold;
        let changeable = registered_at.saturating_add(OWNERSHIP_AGE);
        let old_enough = block >= changeable;
        Self {
            leader_hotkey,
            leader_conviction,
            alpha_out,
            protocol_alpha,
            alpha_burned,
            eligible_alpha: AlphaBalance(eligible),
            threshold: U64F64::from_bits(threshold),
            clears_threshold: clears,
            registered_at,
            ownership_changeable_at: changeable,
            old_enough,
            challenger_clears_gates: clears && old_enough && leader_hotkey != Some(owner_hotkey),
        }
    }
}

/// One pending owner announcement, including rows from removed proxy delegates.
#[derive(Clone, Debug, PartialEq)]
pub struct OwnerProxyAnnouncement {
    /// Real owner account named by the announcement.
    pub real: AccountId,
    /// Delegate whose Announcements storage contains this row.
    pub delegate: AccountId,
    /// Announced call's 32-byte hash.
    pub call_hash: H256,
    /// Announcement block.
    pub announced_at: u32,
    /// All current definitions for this delegate, preserving proxy type and delay.
    /// Empty when the owner has removed this delegate.
    pub definitions: Vec<rtao_runtime::models::ProxyDefinition>,
    /// Earliest current definition's execution block, with saturating u32 arithmetic.
    /// Absent when no current definition permits this delegate to act for the owner.
    pub earliest_execution: Option<u32>,
    /// At least one current definition has reached its announcement delay.
    ///
    /// The call hash does not reveal the call or which proxy type accepts it.
    /// Readiness therefore does not promise permission to execute that call.
    pub delay_elapsed: bool,
    /// Deposit reserved for this delegate's complete announcement list.
    pub delegate_deposit: TaoBalance,
}

/// Owner state whose inputs all belong to one immutable block hash.
#[derive(Clone, Debug, PartialEq)]
pub struct OwnerObservation {
    /// Observed subnet.
    pub netuid: NetUid,
    /// Pinned block height.
    pub block: u64,
    /// Pinned block identity.
    pub block_hash: H256,
    /// Current owner coldkey.
    pub owner_coldkey: AccountId,
    /// Current owner hotkey.
    pub owner_hotkey: AccountId,
    /// Owner start-call flag: alpha activity and epoch processing are enabled.
    pub subtoken_enabled: bool,
    /// Independent root-controlled pool-side emission flag.
    pub subnet_emission_enabled: bool,
    /// Exact chain conviction leader and ownership gates.
    pub conviction: OwnerConviction,
    /// Pending announcements targeting this owner, including removed delegates.
    pub proxy_announcements: Vec<OwnerProxyAnnouncement>,
    /// Exact next-registration replacement exposure for each watched hotkey.
    pub pruning: Vec<PruneRisk>,
}

/// Last delivered finalized event, suitable for durable resume storage.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct OwnerEventCursor {
    /// Finalized block number containing the event.
    pub block: u64,
    /// Immutable identity of that block.
    pub block_hash: H256,
    /// Position in the complete System.Events list, including unrelated events.
    pub event_index: u32,
}

/// A source-checked owner event from finalized history.
#[derive(Clone, Debug, PartialEq)]
pub struct OwnerEvent {
    /// Durable identity of this event.
    pub cursor: OwnerEventCursor,
    /// Subnet monitored by this stream.
    pub netuid: NetUid,
    /// Full decoded record, retaining phase, extrinsic index, attributes and topics.
    pub record: EventRecord,
}

/// Cancellable finalized owner events, ending after the first error.
pub type OwnerEventStream =
    Pin<Box<dyn Stream<Item = Result<OwnerEvent, OwnerMonitorError>> + Send>>;

/// Read owner health and follow the owner events of one non-root subnet.
#[derive(Clone, Debug)]
pub struct OwnerMonitor {
    client: Client,
    netuid: NetUid,
    owner_account: Option<AccountId>,
    watched_hotkeys: BTreeSet<AccountId>,
}
impl OwnerMonitor {
    /// Construct a monitor; pruning watches the current owner hotkey by default.
    pub fn new(client: Client, netuid: NetUid) -> Result<Self, OwnerMonitorError> {
        if NetUid::new(netuid.0).is_err() || netuid == NetUid::ROOT {
            return Err(OwnerMonitorError::InvalidSubnet);
        }
        Ok(Self {
            client,
            netuid,
            owner_account: None,
            watched_hotkeys: BTreeSet::new(),
        })
    }
    /// Also include queue-registration events for this owner account.
    ///
    /// Without this override, each event block's current subnet owner is used.
    /// State samples always report the authoritative on-chain owner.
    pub fn with_owner_account(mut self, owner: AccountId) -> Self {
        self.owner_account = Some(owner);
        self
    }
    /// Watch additional registered or unregistered hotkeys for pruning exposure.
    pub fn with_hotkeys(mut self, hotkeys: impl IntoIterator<Item = AccountId>) -> Self {
        self.watched_hotkeys.extend(hotkeys);
        self
    }
    /// Pin the configured client head once and observe all state at that hash.
    pub async fn sample(&self) -> Result<OwnerObservation, OwnerMonitorError> {
        self.sample_at(&self.client.pin().await?).await
    }
    /// Observe an explicit historical snapshot without advancing its head.
    pub async fn sample_at(
        &self,
        snapshot: &Snapshot,
    ) -> Result<OwnerObservation, OwnerMonitorError> {
        let burned_contract = rtao_runtime::verify_surface_with(
            &snapshot.block().runtime,
            &[SurfaceDescriptor::Storage(ALPHA_BURNED.surface())],
        );
        if !burned_contract.changes.is_empty() {
            return Err(invalid("AlphaAssets.AlphaBurned source contract changed"));
        }
        let (
            exists,
            owner_coldkey,
            owner_hotkey,
            subtoken_enabled,
            subnet_emission_enabled,
            alpha_out,
            protocol_alpha,
            alpha_burned,
            registered_at,
        ) = snapshot
            .fetch((
                st::NETWORKS_ADDED.at((self.netuid,)),
                st::SUBNET_OWNER.at((self.netuid,)),
                st::SUBNET_OWNER_HOTKEY.at((self.netuid,)),
                st::SUBTOKEN_ENABLED.at((self.netuid,)),
                st::SUBNET_EMISSION_ENABLED.at((self.netuid,)),
                st::SUBNET_ALPHA_OUT.at((self.netuid,)),
                st::SUBNET_PROTOCOL_ALPHA.at((self.netuid,)),
                ALPHA_BURNED.at((self.netuid,)),
                st::NETWORK_REGISTERED_AT.at((self.netuid,)),
            ))
            .await?;
        if !exists {
            return Err(OwnerMonitorError::SubnetMissing(self.netuid));
        }
        let owner_hotkey = *owner_hotkey.raw();
        let owner_coldkey = *owner_coldkey.raw();
        let leader = snapshot
            .call(&api::GET_MOST_CONVICTED_HOTKEY_ON_SUBNET, (self.netuid,))
            .await?;
        let conviction = match leader {
            Some(hotkey) => {
                snapshot
                    .call(&api::GET_HOTKEY_CONVICTION, (hotkey, self.netuid))
                    .await?
            }
            None => U64F64::default(),
        };
        let conviction = OwnerConviction::evaluate(
            snapshot.block().number,
            registered_at,
            owner_hotkey,
            leader,
            conviction,
            alpha_out,
            protocol_alpha,
            alpha_burned,
        );
        let announcements = announcements(snapshot, owner_coldkey).await?;
        let mut watched = self.watched_hotkeys.clone();
        watched.insert(owner_hotkey);
        let mut pruning = Vec::with_capacity(watched.len());
        for hotkey in watched {
            pruning.push(PruneRisk::observe(snapshot, self.netuid, hotkey).await?);
        }
        Ok(OwnerObservation {
            netuid: self.netuid,
            block: snapshot.block().number,
            block_hash: snapshot.block().hash,
            owner_coldkey,
            owner_hotkey,
            subtoken_enabled,
            subnet_emission_enabled,
            conviction,
            proxy_announcements: announcements,
            pruning,
        })
    }
    /// Decode and filter owner events at one explicit block.
    ///
    /// Every recognized event's generated field contract is checked before its
    /// subnet or owner is inspected. Unknown event names remain unrelated.
    pub async fn events_at(
        &self,
        snapshot: &Snapshot,
    ) -> Result<Vec<OwnerEvent>, OwnerMonitorError> {
        let owner = match self.owner_account {
            Some(owner) => owner,
            None => *snapshot
                .storage(&st::SUBNET_OWNER, (self.netuid,))
                .await?
                .raw(),
        };
        let events = snapshot.events().await?;
        filtered_events(snapshot, self.netuid, owner, &events)
    }
    /// Follow finalized events from the current finalized block or after a cursor.
    ///
    /// The shared head follower recovers reconnect and slow-consumer gaps.
    /// Resuming also reads every intervening finalized block, including later
    /// events in the cursor's block. Historical hash disagreement, read failures,
    /// follower loss, or changed recognized event shapes emit one error and end.
    /// Cancelling or dropping the stream stops work without a background task.
    pub fn events(
        &self,
        after: Option<OwnerEventCursor>,
        cancellation: CancellationToken,
    ) -> Result<OwnerEventStream, OwnerMonitorError> {
        let headers = self.client.blocks(Finality::Finalized)?;
        Ok(event_stream(self.clone(), headers, after, cancellation))
    }
}

async fn announcements(
    snapshot: &Snapshot,
    owner: AccountId,
) -> Result<Vec<OwnerProxyAnnouncement>, OwnerMonitorError> {
    let (definitions, _): (Vec<rtao_runtime::models::ProxyDefinition>, TaoBalance) =
        serde_json::from_value(
            snapshot
                .storage(&PROXY_DEFINITIONS, (owner,))
                .await?
                .to_json(),
        )
        .map_err(|error| invalid(format!("proxy definition projection: {error}")))?;
    let mut pending = Vec::new();
    // Removing a proxy does not clear its announcements. Scan the pinned map
    // so old delegates' owner-targeted rows remain visible until removed.
    let rows = snapshot.storage_iter(&proxy::ANNOUNCEMENTS, ());
    futures::pin_mut!(rows);
    while let Some(entry) = rows.next().await {
        let (keys, (announcements, deposit)) = entry?;
        let [key] = keys.as_slice() else {
            return Err(invalid("announcement map key is not one delegate"));
        };
        let delegate = account(key.as_core())?;
        let current = definitions
            .iter()
            .filter(|definition| definition.delegate == delegate)
            .cloned()
            .collect::<Vec<_>>();
        let delay = current.iter().map(|definition| definition.delay).min();
        for row in announcements {
            let row = row.as_core();
            let real = account(
                row.field("real")
                    .ok_or_else(|| invalid("announcement has no real account"))?,
            )?;
            let call_hash = hash(
                row.field("call_hash")
                    .ok_or_else(|| invalid("announcement has no call hash"))?,
            )?;
            let height = uint(
                row.field("height")
                    .ok_or_else(|| invalid("announcement has no height"))?,
                "announcement height",
            )?;
            let height =
                u32::try_from(height).map_err(|_| invalid("announcement height exceeds u32"))?;
            if real != owner {
                continue;
            }
            let earliest_execution = delay.map(|delay| height.saturating_add(delay));
            pending.push(OwnerProxyAnnouncement {
                real,
                delegate,
                call_hash,
                announced_at: height,
                definitions: current.clone(),
                earliest_execution,
                delay_elapsed: delay.is_some_and(|delay| {
                    snapshot.block().number.saturating_sub(u64::from(height)) >= u64::from(delay)
                }),
                delegate_deposit: deposit,
            });
        }
    }
    Ok(pending)
}

fn event_descriptor(record: &EventRecord) -> Option<&'static EventDescriptor> {
    [
        &ev::NETWORK_ADDED,
        &ev::NETWORK_REGISTRATION_QUEUED,
        &ev::FIRST_EMISSION_BLOCK_NUMBER_SET,
        &ev::SUBNET_OWNER_CHANGED,
        &ev::SUBNET_OWNER_HOTKEY_SET,
        &ev::EPOCH_TRIGGERED,
        &ev::EPOCH_DEFERRED,
        &ev::EPOCH_SKIPPED,
        &ev::TEMPO_SET,
        &ev::WEIGHTS_VERSION_KEY_SET,
        &ev::COMMIT_REVEAL_ENABLED,
        &ev::MAX_ALLOWED_UIDS_SET,
        &ev::SUBNET_IDENTITY_SET,
        &ev::SYMBOL_UPDATED,
        &rtao_runtime::events::admin_utils::COLLATERAL_LOCK_SHARE_SET,
        &ev::SUBNET_LEASE_CREATED,
        &rtao_runtime::events::proxy::ANNOUNCED,
        &rtao_runtime::events::proxy::PROXY_ADDED,
        &rtao_runtime::events::proxy::PROXY_REMOVED,
    ]
    .into_iter()
    .find(|descriptor| record.module_id == descriptor.pallet && record.event_id == descriptor.name)
}
fn filtered_events(
    snapshot: &Snapshot,
    netuid: NetUid,
    owner: AccountId,
    events: &rtao_client::Events,
) -> Result<Vec<OwnerEvent>, OwnerMonitorError> {
    let mut result = Vec::new();
    for (index, record) in events.records().iter().enumerate() {
        let Some(descriptor) = event_descriptor(record) else {
            continue;
        };
        let report = rtao_runtime::verify_surface_with(
            &snapshot.block().runtime,
            &[SurfaceDescriptor::Event(descriptor)],
        );
        if !report.shapes_match() {
            return Err(invalid(format!(
                "{}.{} event contract changed",
                descriptor.pallet, descriptor.name
            )));
        }
        let matched = if descriptor.pallet == "Proxy" {
            let name = if descriptor.name == "Announced" {
                "real"
            } else {
                "delegator"
            };
            account(event_field(&record.attributes, name, 0)?)? == owner
        } else if descriptor.name == "NetworkRegistrationQueued" {
            account(event_field(&record.attributes, "coldkey", 0)?)? == owner
        } else {
            let value = uint(
                event_field(&record.attributes, "netuid", 0)?,
                "event netuid",
            )?;
            let event_netuid =
                u16::try_from(value).map_err(|_| invalid("event netuid exceeds u16"))?;
            NetUid::new(event_netuid)
                .map_err(|_| invalid("event netuid is outside the native domain"))?
                == netuid
        };
        if matched {
            result.push(OwnerEvent {
                cursor: OwnerEventCursor {
                    block: snapshot.block().number,
                    block_hash: snapshot.block().hash,
                    event_index: u32::try_from(index)
                        .map_err(|_| invalid("event index exceeds u32"))?,
                },
                netuid,
                record: record.clone(),
            });
        }
    }
    Ok(result)
}
fn event_field<'a>(
    attributes: &'a Value,
    name: &str,
    index: usize,
) -> Result<&'a Value, OwnerMonitorError> {
    match attributes {
        Value::List(values) | Value::Tuple(values) => values.get(index),
        Value::Int(_) | Value::Uint(_) if index == 0 => Some(attributes),
        _ => attributes.field(name),
    }
    .ok_or_else(|| invalid(format!("event has no {name} field")))
}
fn uint(value: &Value, name: &str) -> Result<u64, OwnerMonitorError> {
    value
        .as_u128()
        .and_then(|value| u64::try_from(value).ok())
        .ok_or_else(|| invalid(format!("{name} is not a native u64")))
}
fn account(value: &Value) -> Result<AccountId, OwnerMonitorError> {
    if let Some(text) = value.as_str() {
        return text
            .parse()
            .map_err(|_| invalid("account is not a valid address"));
    }
    value
        .value_bytes()
        .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok())
        .map(AccountId)
        .ok_or_else(|| invalid("account is not 32 bytes"))
}
fn hash(value: &Value) -> Result<H256, OwnerMonitorError> {
    let bytes = if let Some(text) = value.as_str() {
        hex::decode(
            text.strip_prefix("0x")
                .ok_or_else(|| invalid("call hash lacks 0x prefix"))?,
        )
        .map_err(|_| invalid("call hash is not hex"))?
    } else {
        value
            .value_bytes()
            .ok_or_else(|| invalid("call hash is not bytes"))?
    };
    bytes
        .try_into()
        .map(H256)
        .map_err(|_| invalid("call hash is not 32 bytes"))
}
fn invalid(detail: impl Into<String>) -> OwnerMonitorError {
    OwnerMonitorError::InvalidState(detail.into())
}

struct EventFollower {
    monitor: OwnerMonitor,
    headers: BlockStream,
    after: Option<OwnerEventCursor>,
    replay: VecDeque<(u64, H256)>,
    last: Option<(u64, H256)>,
    pending: VecDeque<OwnerEvent>,
    cancellation: CancellationToken,
    ended: bool,
}
fn event_stream(
    monitor: OwnerMonitor,
    headers: BlockStream,
    after: Option<OwnerEventCursor>,
    cancellation: CancellationToken,
) -> OwnerEventStream {
    let state = EventFollower {
        monitor,
        headers,
        after,
        replay: VecDeque::new(),
        last: None,
        pending: VecDeque::new(),
        cancellation,
        ended: false,
    };
    Box::pin(futures::stream::unfold(state, |mut state| async move {
        loop {
            if state.ended || state.cancellation.is_cancelled() {
                return None;
            }
            if let Some(event) = state.pending.pop_front() {
                return Some((Ok(event), state));
            }
            let cancellation = state.cancellation.clone();
            let read = tokio::select! { biased;
                () = cancellation.cancelled() => return None,
                result = state.advance() => result,
            };
            if let Err(error) = read {
                state.ended = true;
                return Some((Err(error), state));
            }
        }
    }))
}
impl EventFollower {
    async fn advance(&mut self) -> Result<(), OwnerMonitorError> {
        if self.replay.is_empty() {
            let header = self
                .headers
                .next()
                .await
                .ok_or(OwnerMonitorError::FollowerEnded)??;
            if let Some((number, hash)) = self.last {
                if header.number <= number {
                    if header.number == number && header.hash == hash {
                        return Ok(());
                    }
                    return Err(invalid("finalized header did not advance consistently"));
                }
                if header.number == number.saturating_add(1) && header.parent_hash != Some(hash) {
                    return Err(invalid(
                        "finalized header parent disagrees with processed history",
                    ));
                }
            }
            if let Some(cursor) = self.after
                && cursor.block > header.number
            {
                // The tracker may still be catching up when a recently
                // persisted cursor resumes. Check the node's finalized
                // state before rejecting it, then await the advancing
                // follower instead of skipping its backfill.
                let finalized = self.monitor.client.at(BlockRef::Finalized).await?;
                if cursor.block > finalized.block().number {
                    return Err(OwnerMonitorError::InvalidCursor(
                        "cursor is ahead of finalized head".into(),
                    ));
                }
                return Ok(());
            }
            let earliest = match self.last {
                Some((number, _)) => number
                    .checked_add(1)
                    .ok_or_else(|| invalid("finalized height exhausted u64"))?,
                None => self.after.map_or(header.number, |cursor| cursor.block),
            };
            self.replay =
                finalized_path(self.monitor.client.rpc(), header, earliest, self.last).await?;
        }
        let (number, hash) = self
            .replay
            .pop_front()
            .ok_or_else(|| invalid("finalized backfill path is empty"))?;
        let snapshot = self.monitor.client.at(BlockRef::Hash(hash)).await?;
        if snapshot.block().number != number || snapshot.block().hash != hash {
            return Err(invalid("finalized backfill resolved a different block"));
        }
        if let Some(cursor) = self.after {
            if snapshot.block().hash != cursor.block_hash {
                return Err(OwnerMonitorError::InvalidCursor(
                    "cursor block hash disagrees with finalized history".into(),
                ));
            }
            // A cursor must identify an actual event in this monitor's scope.
            let events = self.monitor.events_at(&snapshot).await?;
            if !events.iter().any(|event| event.cursor == cursor) {
                return Err(OwnerMonitorError::InvalidCursor(
                    "cursor does not identify an owner event".into(),
                ));
            }
            self.pending.extend(
                events
                    .into_iter()
                    .filter(|event| event.cursor.event_index > cursor.event_index),
            );
            self.after = None;
        } else {
            self.pending
                .extend(self.monitor.events_at(&snapshot).await?);
        }
        self.last = Some((number, snapshot.block().hash));
        Ok(())
    }
}

// Validate the whole replay branch against the known finalized target before
// yielding any event from it. Height lookups can lag across failover endpoints;
// walking parent hashes and then reading by hash avoids publishing that fork.
async fn finalized_path(
    provider: &impl HeadProvider,
    mut header: BlockHeader,
    earliest: u64,
    previous: Option<(u64, H256)>,
) -> Result<VecDeque<(u64, H256)>, OwnerMonitorError> {
    let mut path = VecDeque::new();
    loop {
        if header.number < earliest {
            return Err(invalid(
                "finalized backfill path starts below its replay range",
            ));
        }
        path.push_front((header.number, header.hash));
        if header.number == earliest {
            if let Some((number, hash)) = previous
                && (number.checked_add(1) != Some(earliest) || header.parent_hash != Some(hash))
            {
                return Err(invalid(
                    "finalized backfill parent disagrees with processed history",
                ));
            }
            return Ok(path);
        }
        let parent_hash = header
            .parent_hash
            .ok_or_else(|| invalid("finalized backfill has no parent hash"))?;
        let parent = provider
            .header(parent_hash)
            .await?
            .ok_or_else(|| invalid("finalized backfill parent header is absent"))?;
        if parent.hash != parent_hash || parent.number.checked_add(1) != Some(header.number) {
            return Err(invalid("finalized backfill parent identity disagrees"));
        }
        header = parent;
    }
}

#[cfg(test)]
mod tests;
