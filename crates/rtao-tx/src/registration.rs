// Derived from RaoFoundation/subtensor sdk/python/bittensor/executor.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Follow a successful register_network call through immediate or queued creation.

use crate::{ChainError, ExecuteOptions, ExtrinsicResult, IntentError, Progress, TxError};
use futures::StreamExt;
use rtao_client::{BlockRef, Client, Finality, HeadError, HeadProvider, Snapshot, ViewExt};
use rtao_core::{codec::Value, value_ext::ValueExt};
use rtao_primitives::{AccountId, H256, NetUid};
use rtao_runtime::storage::subtensor_module as storage;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use tokio::time::Instant;

/// Resolve a successful registration while retaining its submission receipt.
///
/// `execution_deadline` is the executor's original absolute deadline. No author
/// RPC or transaction retry occurs here, including after a timeout or reorg.
pub(crate) async fn complete(
    client: &Client,
    result: ExtrinsicResult,
    owner: AccountId,
    hotkey: AccountId,
    opts: &ExecuteOptions,
    deferred_dispatch: bool,
    execution_deadline: Option<Instant>,
) -> Result<ExtrinsicResult, TxError> {
    if !result.success
        || result.fate_unknown.is_some()
        || !opts.wait_for_registration
        || !(opts.wait_for_inclusion || opts.wait_for_finalization)
    {
        return Ok(result);
    }
    // Python treats a zero registration timeout as an unbounded wait.
    let registration_deadline = opts
        .registration_timeout
        .filter(|duration| !duration.is_zero())
        .map(|duration| {
            Instant::now().checked_add(duration).ok_or_else(|| {
                IntentError::InvalidArgument("registration timeout overflows time".into())
            })
        })
        .transpose()?;
    let deadline = match (execution_deadline, registration_deadline) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (left, right) => left.or(right),
    };
    let wait = async {
        tokio::select! {
            biased;
            () = client.closed() => Err(TxError::ConnectionNotReady),
            outcome = follow(client, result, owner, hotkey, opts, deferred_dispatch) => outcome,
        }
    };
    match deadline {
        Some(deadline) => tokio::time::timeout_at(deadline, wait).await.map_err(|_| {
            TxError::Rpc(rtao_rpc::RpcError::Timeout {
                detail: "registration timed out without reaching NetworkAdded".into(),
            })
        })?,
        None => wait.await,
    }
}

#[derive(Clone, Default)]
struct Registration {
    queued: bool,
    queued_at: Option<u64>,
    registration_block: Option<u64>,
    price: Option<u64>,
    deregistered: Option<NetUid>,
    cleanup: Option<NetUid>,
}

impl Registration {
    fn observe_queue(&mut self, attributes: &Value, block: Option<u64>) {
        self.queued = true;
        self.queued_at = self.queued_at.or(block);
        self.registration_block = self
            .registration_block
            .or_else(|| number_field(attributes, "registration_block"));
        self.price = self
            .price
            .or_else(|| number_field(attributes, "lock_amount"));
    }

    async fn queued_progress(&self, opts: &ExecuteOptions) {
        progress(
            opts,
            "queued",
            json!({
                "block": self.queued_at,
                "cleanup_netuid": self.cleanup,
                "deregistered_netuid": self.deregistered,
            }),
        )
        .await;
    }

    async fn finish(
        &self,
        base: &ExtrinsicResult,
        netuid: NetUid,
        block: Option<u64>,
        locked: Option<u64>,
        opts: &ExecuteOptions,
    ) -> ExtrinsicResult {
        let mut result = base.clone();
        let mode = if self.queued {
            "after_deregistration"
        } else {
            "immediate"
        };
        result.message = format!("Subnet {} registered.", netuid.0);
        result.data.insert("netuid".into(), json!(netuid));
        result.data.insert("registration_mode".into(), json!(mode));
        result
            .data
            .insert("registered_at_block".into(), json!(block));
        if let Some(queued_at) = self.queued_at {
            result
                .data
                .insert("queued_at_block".into(), json!(queued_at));
        }
        if self.queued {
            if let Some(cleanup) = self.cleanup {
                result.data.insert("cleanup_netuid".into(), json!(cleanup));
            }
            if let Some(deregistered) = self.deregistered {
                result
                    .data
                    .insert("deregistered_netuid".into(), json!(deregistered));
            }
        }
        if let Some(price) = locked.or(self.price) {
            result
                .data
                .insert("registration_price_rao".into(), json!(price));
        }
        let mut fields = json!({"mode": mode, "netuid": netuid, "block": block});
        if self.queued {
            fields["cleanup_netuid"] = json!(self.cleanup);
            fields["deregistered_netuid"] = json!(self.deregistered);
        }
        progress(opts, "registered", fields).await;
        result
    }
}

async fn follow(
    client: &Client,
    result: ExtrinsicResult,
    owner: AccountId,
    hotkey: AccountId,
    opts: &ExecuteOptions,
    deferred_dispatch: bool,
) -> Result<ExtrinsicResult, TxError> {
    let included_at = result
        .extrinsic_id
        .as_deref()
        .and_then(|id| id.split('-').next())
        .and_then(|height| height.parse::<u64>().ok());
    let mut initial = Registration::default();
    for event in &result.events {
        if event.matches("SubtensorModule", "NetworkAdded") {
            if let Some(netuid) = event_netuid(&event.attributes) {
                let snapshot = match (result.block_hash, included_at) {
                    (Some(hash), _) => client.at(BlockRef::Hash(hash)).await?,
                    (None, Some(height)) => client.at(BlockRef::Number(height)).await?,
                    (None, None) => client.at(BlockRef::Best).await?,
                };
                if canonical_hash(client, snapshot.block().number).await? != snapshot.block().hash {
                    return Err(inclusion_reorg());
                }
                let (actual_owner, actual_hotkey, locked) = snapshot
                    .fetch((
                        storage::SUBNET_OWNER.at((netuid,)),
                        storage::SUBNET_OWNER_HOTKEY.at((netuid,)),
                        storage::SUBNET_LOCKED.at((netuid,)),
                    ))
                    .await?;
                if canonical_hash(client, snapshot.block().number).await? != snapshot.block().hash {
                    return Err(inclusion_reorg());
                }
                if actual_owner.raw() == &owner && actual_hotkey.raw() == &hotkey {
                    return Ok(Registration::default()
                        .finish(&result, netuid, included_at, Some(locked.0), opts)
                        .await);
                }
            }
        } else if event.matches("SubtensorModule", "NetworkRegistrationQueued")
            && identities_match(&event.attributes, owner, hotkey)
        {
            initial.observe_queue(&event.attributes, included_at);
        } else if event.matches("SubtensorModule", "NetworkRemoved") {
            initial.deregistered = event_netuid(&event.attributes);
        }
    }
    // A multisig approval which did not execute the inner call has no deferred
    // registration. Shield carriers explicitly opt into the deferred path.
    if !initial.queued && !deferred_dispatch {
        return Ok(result);
    }

    let first = match included_at {
        Some(height) => height,
        None => client.at(BlockRef::Best).await?.block().number,
    };
    let anchor = canonical_hash(client, first).await?;
    if result.block_hash.is_some_and(|hash| hash != anchor) {
        return Err(inclusion_reorg());
    }
    let inclusion = client.at(BlockRef::Hash(anchor)).await?;
    if initial.queued {
        initial.cleanup = match initial.deregistered {
            Some(netuid) => Some(netuid),
            None => cleanup_netuid(&inclusion).await?,
        };
        initial.queued_progress(opts).await;
    }

    // Open the live stream before reading the head. Scanning every intervening
    // height also closes races during the receipt and subscription setup.
    let mut heads = match client.blocks(Finality::Best) {
        Ok(heads) => heads,
        Err(_) => client.blocks_strict(Finality::Best).await?,
    };
    let mut state = initial.clone();
    let mut next = first;
    let mut last_hash = None;
    let mut head = client.at(BlockRef::Best).await?.block().number;
    let mut live_update = false;
    loop {
        if canonical_hash(client, first).await? != anchor {
            return Err(inclusion_reorg());
        }
        if let Some((height, hash)) = last_hash
            && (height > head || canonical_hash(client, height).await? != hash)
        {
            // Rebuild queue/cleanup state from canonical events after a best
            // reorg. State from abandoned blocks cannot satisfy this request.
            state = initial.clone();
            next = first;
        }
        while next <= head {
            let hash = canonical_hash(client, next).await?;
            let snapshot = client.at(BlockRef::Hash(hash)).await?;
            let before = state.clone();
            let completed = scan(&snapshot, &mut state, owner, hotkey).await?;
            if canonical_hash(client, next).await? != hash {
                state = before;
                continue;
            }
            last_hash = Some((next, hash));
            next = next
                .checked_add(1)
                .ok_or_else(|| ChainError::new("registration block height overflowed", None))?;
            if !before.queued && state.queued {
                state.queued_progress(opts).await;
            }
            if let Some(completed) = completed {
                return Ok(state
                    .finish(
                        &result,
                        completed.netuid,
                        Some(snapshot.block().number),
                        Some(completed.locked),
                        opts,
                    )
                    .await);
            }
        }
        if live_update {
            progress(
                opts,
                "waiting",
                json!({
                    "block": head,
                    "blocks_since_call": head.saturating_sub(state.queued_at.unwrap_or(first)),
                    "cleanup_netuid": state.cleanup,
                    "deregistered_netuid": state.deregistered,
                }),
            )
            .await;
        }
        let Some(update) = heads.next().await else {
            return Err(ChainError::new(
                "block subscription ended before the queued subnet was registered",
                None,
            )
            .into());
        };
        head = match update {
            Ok(header) => header.number,
            Err(HeadError::Reorg(reorg)) => {
                state = initial.clone();
                next = first;
                last_hash = None;
                reorg.replacement.number
            }
            Err(HeadError::Rpc(error)) => return Err(TxError::Rpc(error)),
            Err(error) => return Err(ChainError::new(error.to_string(), None).into()),
        };
        live_update = true;
    }
}

struct Completion {
    netuid: NetUid,
    locked: u64,
}

async fn scan(
    snapshot: &Snapshot,
    state: &mut Registration,
    owner: AccountId,
    hotkey: AccountId,
) -> Result<Option<Completion>, TxError> {
    let events = snapshot.events().await?;
    let mut target_indices = BTreeSet::new();
    let mut removed = BTreeMap::new();
    for event in events.records() {
        if event.matches("SubtensorModule", "NetworkRegistrationQueued")
            && identities_match(&event.attributes, owner, hotkey)
            && state.registration_block.is_none_or(|height| {
                number_field(&event.attributes, "registration_block") == Some(height)
            })
        {
            target_indices.insert(event.extrinsic_idx);
            state.observe_queue(&event.attributes, Some(snapshot.block().number));
        } else if event.matches("SubtensorModule", "NetworkRemoved")
            && let Some(netuid) = event_netuid(&event.attributes)
        {
            removed.insert(event.extrinsic_idx, netuid);
        }
    }
    if state.deregistered.is_none() {
        state.deregistered = target_indices
            .iter()
            .find_map(|index| removed.get(index).copied());
    }
    if state.queued && state.cleanup.is_none() {
        state.cleanup = match state.deregistered {
            Some(netuid) => Some(netuid),
            None => cleanup_netuid(snapshot).await?,
        };
    }
    for event in events.records() {
        if !event.matches("SubtensorModule", "NetworkAdded") {
            continue;
        }
        let Some(netuid) = event_netuid(&event.attributes) else {
            continue;
        };
        let (actual_owner, actual_hotkey, pending, locked) = snapshot
            .fetch((
                storage::SUBNET_OWNER.at((netuid,)),
                storage::SUBNET_OWNER_HOTKEY.at((netuid,)),
                storage::NETWORK_REGISTRATION_QUEUE.at(()),
                storage::SUBNET_LOCKED.at((netuid,)),
            ))
            .await?;
        if actual_owner.raw() != &owner || actual_hotkey.raw() != &hotkey {
            continue;
        }
        if state.queued
            && let Some(height) = state.registration_block
        {
            let mut still_pending = false;
            for entry in &pending {
                let entry = entry.as_core();
                if identities_match(entry, owner, hotkey) {
                    let registered =
                        number_field(entry, "registration_block").ok_or_else(|| {
                            ChainError::new(
                                "queued registration has no valid registration_block",
                                None,
                            )
                        })?;
                    still_pending |= registered == height;
                }
            }
            if still_pending {
                continue;
            }
        }
        return Ok(Some(Completion {
            netuid,
            locked: locked.0,
        }));
    }
    Ok(None)
}

async fn cleanup_netuid(snapshot: &Snapshot) -> Result<Option<NetUid>, TxError> {
    let (status, queue) = snapshot
        .fetch((
            storage::CURRENT_DISSOLVE_CLEANUP_STATUS.at(()),
            storage::DISSOLVE_CLEANUP_QUEUE.at(()),
        ))
        .await?;
    Ok(status
        .as_ref()
        .and_then(|status| status.as_core().field("netuid").and_then(netuid))
        .or_else(|| queue.first().copied()))
}

async fn canonical_hash(client: &Client, height: u64) -> Result<H256, TxError> {
    client
        .rpc()
        .block_hash(height)
        .await
        .map_err(|error| match error {
            HeadError::Rpc(error) => TxError::Rpc(error),
            error => ChainError::new(error.to_string(), None).into(),
        })?
        .ok_or_else(|| ChainError::new(format!("missing registration block {height}"), None).into())
}

fn inclusion_reorg() -> TxError {
    ChainError::new(
        "registration inclusion was reorganized before NetworkAdded",
        None,
    )
    .into()
}

fn account(value: &Value) -> Option<AccountId> {
    value
        .as_str()
        .and_then(|value| value.parse().ok())
        .or_else(|| {
            value
                .value_bytes()
                .and_then(|bytes| bytes.try_into().ok())
                .map(AccountId)
        })
}

fn identities_match(value: &Value, owner: AccountId, hotkey: AccountId) -> bool {
    value.field("coldkey").and_then(account) == Some(owner)
        && value.field("hotkey").and_then(account) == Some(hotkey)
}

fn number_field(value: &Value, field: &str) -> Option<u64> {
    value
        .field(field)
        .and_then(ValueExt::as_u128)
        .and_then(|number| number.try_into().ok())
}

fn netuid(value: &Value) -> Option<NetUid> {
    value
        .as_u128()
        .and_then(|number| number.try_into().ok())
        .map(NetUid)
}

fn event_netuid(attributes: &Value) -> Option<NetUid> {
    let value = attributes.field("netuid").unwrap_or(attributes);
    netuid(value.tuple_item(0).unwrap_or(value))
}

async fn progress(opts: &ExecuteOptions, stage: &str, fields: serde_json::Value) {
    if let Some(sender) = &opts.progress {
        let data = fields.as_object().cloned().unwrap_or_default();
        let _ = sender
            .send(Progress {
                stage: stage.into(),
                data,
            })
            .await;
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use rtao_client::{ClientConfig, Inclusion, Network};
    use rtao_core::{CoreError, runtime::TypeSpec};
    use rtao_primitives::IntoCoreValue;
    use rtao_testkit::{FakeChain, FakeState, fakes::fill_defaults, rpc::RpcClient};
    use serde_json::{Map, Value as Json};
    use std::time::Duration;

    const OWNER: AccountId = AccountId([1; 32]);
    const HOTKEY: AccountId = AccountId([2; 32]);

    async fn fixture(state: FakeState) -> (Client, FakeChain) {
        let fake = FakeChain::from_state(state);
        let mut config =
            ClientConfig::new(Network::resolve_with("local", |_| None).unwrap()).unwrap();
        config.track_heads = false;
        config.runtime_cache_dir = None;
        config.config_store = None;
        let client = Client::from_rpc(RpcClient::new(fake.clone()), config)
            .await
            .unwrap();
        (client, fake)
    }

    fn event(name: &str, attributes: Json, index: Option<u32>) -> Json {
        json!({
            "phase": if index.is_some() { "ApplyExtrinsic" } else { "Finalization" },
            "extrinsic_idx": index,
            "module_id": "SubtensorModule", "event_id": name,
            "attributes": attributes, "topics": [],
        })
    }

    fn added(netuid: u16, index: Option<u32>) -> Json {
        event("NetworkAdded", json!([netuid, 0]), index)
    }

    fn queued(height: u64, index: u32) -> Json {
        event(
            "NetworkRegistrationQueued",
            json!({
                "coldkey": OWNER, "hotkey": HOTKEY,
                "registration_block": height, "lock_amount": 444,
            }),
            Some(index),
        )
    }

    fn at(
        state: &mut FakeState,
        hash: &str,
        item: &str,
        keys: &[Value],
        value: Json,
    ) -> Result<(), CoreError> {
        let info = state
            .runtime
            .storage_entry("SubtensorModule", item)
            .unwrap();
        let value = fill_defaults(&state.runtime, info.value_type, value)?;
        let key = state
            .runtime
            .storage_key(info, keys)
            .map_err(CoreError::from)?;
        let bytes = state
            .runtime
            .encode_spec(&TypeSpec::Id(info.value_type), &value)
            .map_err(CoreError::from)?;
        state.seed_at(hash, key, Some(bytes));
        Ok(())
    }

    fn identities(
        state: &mut FakeState,
        hash: &str,
        netuid: u16,
        owner: AccountId,
        hotkey: AccountId,
    ) {
        let keys = [NetUid(netuid).into_core_value()];
        at(state, hash, "SubnetOwner", &keys, json!(owner)).unwrap();
        at(state, hash, "SubnetOwnerHotkey", &keys, json!(hotkey)).unwrap();
        at(state, hash, "SubnetLocked", &keys, json!(777)).unwrap();
    }

    async fn receipt(client: &Client, height: u64, index: u32) -> ExtrinsicResult {
        let snapshot = client.at(BlockRef::Number(height)).await.unwrap();
        ExtrinsicResult {
            success: true,
            message: "Extrinsic submitted.".into(),
            block_hash: Some(snapshot.block().hash),
            extrinsic_id: Some(format!("{height}-{index}")),
            explorer_url: None,
            fee: None,
            events: snapshot
                .events()
                .await
                .unwrap()
                .records()
                .iter()
                .filter(|event| event.extrinsic_idx == Some(index))
                .cloned()
                .collect(),
            error: None,
            data: Map::from_iter([("kept".into(), json!(true))]),
            output: (),
            inclusion: Inclusion::Finalized,
            fate_unknown: None,
        }
    }

    fn bounded() -> ExecuteOptions {
        ExecuteOptions {
            registration_timeout: Some(Duration::from_secs(3)),
            ..Default::default()
        }
    }

    async fn wait_subscribed(fake: &FakeChain) {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if fake
                    .requests()
                    .iter()
                    .any(|request| request.method == "chain_subscribeNewHeads")
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    #[test]
    fn netuid_shapes_and_ranges() {
        assert_eq!(event_netuid(&Value::Uint(0)), Some(NetUid(0)));
        assert_eq!(
            event_netuid(&Value::Tuple(vec![Value::Uint(42), Value::Uint(1)])),
            Some(NetUid(42))
        );
        assert_eq!(
            event_netuid(&Value::Dict(vec![(
                Value::Str("netuid".into()),
                Value::Uint(7)
            )])),
            Some(NetUid(7))
        );
        for value in [
            Value::Null,
            Value::Int(-1),
            Value::Uint(65536),
            Value::List(vec![]),
        ] {
            assert_eq!(event_netuid(&value), None);
        }
    }

    #[tokio::test]
    async fn immediate_receipt_checks_historical_identities_and_preserves_receipt() {
        let mut state = FakeState::default();
        let hash = state.block_hash(100).unwrap();
        identities(&mut state, &hash, 42, OWNER, HOTKEY);
        state
            .seed_events(&hash, json!([added(42, Some(0))]))
            .unwrap();
        let (client, fake) = fixture(state).await;
        let original = receipt(&client, 100, 0).await;
        let result = complete(
            &client,
            original.clone(),
            OWNER,
            HOTKEY,
            &bounded(),
            false,
            None,
        )
        .await
        .unwrap();
        assert_eq!(result.message, "Subnet 42 registered.");
        assert_eq!(
            result.data,
            Map::from_iter([
                ("kept".into(), json!(true)),
                ("netuid".into(), json!(42)),
                ("registration_mode".into(), json!("immediate")),
                ("registered_at_block".into(), json!(100)),
                ("registration_price_rao".into(), json!(777)),
            ])
        );
        assert_eq!(result.events, original.events);
        assert_eq!(result.block_hash, original.block_hash);
        assert_eq!(result.extrinsic_id, original.extrinsic_id);
        assert!(
            !fake
                .requests()
                .iter()
                .any(|request| request.method.starts_with("author_"))
        );
        assert!(
            !fake
                .requests()
                .iter()
                .any(|request| request.method == "chain_subscribeNewHeads")
        );
    }

    #[tokio::test]
    async fn receipt_event_with_wrong_owner_or_hotkey_does_not_claim_registration() {
        for (owner, hotkey) in [(AccountId([9; 32]), HOTKEY), (OWNER, AccountId([9; 32]))] {
            let mut state = FakeState::default();
            let hash = state.block_hash(100).unwrap();
            identities(&mut state, &hash, 42, owner, hotkey);
            state
                .seed_events(&hash, json!([added(42, Some(0))]))
                .unwrap();
            let (client, _) = fixture(state).await;
            let original = receipt(&client, 100, 0).await;
            let result = complete(&client, original, OWNER, HOTKEY, &bounded(), false, None)
                .await
                .unwrap();
            assert!(!result.data.contains_key("netuid"));
        }
    }

    #[tokio::test]
    async fn approval_opt_out_pool_failure_and_unknown_fate_do_not_start_a_follower() {
        let (client, fake) = fixture(FakeState::default()).await;
        let original = receipt(&client, 100, 0).await;
        for case in 0..5 {
            let mut result = original.clone();
            let mut opts = bounded();
            match case {
                0 => {}
                1 => opts.wait_for_registration = false,
                2 => {
                    opts.wait_for_inclusion = false;
                    opts.wait_for_finalization = false;
                }
                3 => result.success = false,
                _ => result.fate_unknown = Some(H256([7; 32])),
            }
            let returned = complete(
                &client,
                result.clone(),
                OWNER,
                HOTKEY,
                &opts,
                case != 0,
                None,
            )
            .await
            .unwrap();
            assert_eq!(returned.fate_unknown, result.fate_unknown);
            assert_eq!(returned.success, result.success);
            assert_eq!(returned.data, result.data);
        }
        assert!(
            !fake
                .requests()
                .iter()
                .any(|request| request.method == "chain_subscribeNewHeads")
        );
    }

    #[tokio::test]
    async fn queued_receipt_rescans_on_idle_and_pairs_removed_by_extrinsic() {
        let mut state = FakeState::default();
        let hash = state.block_hash(100).unwrap();
        identities(&mut state, &hash, 42, OWNER, HOTKEY);
        at(
            &mut state,
            &hash,
            "NetworkRegistrationQueue",
            &[],
            json!([]),
        )
        .unwrap();
        state
            .seed_events(
                &hash,
                json!([
                    event("NetworkRemoved", json!(9), Some(3)),
                    queued(100, 2),
                    event("NetworkRemoved", json!(7), Some(2)),
                    added(42, None),
                ]),
            )
            .unwrap();
        let (client, _) = fixture(state).await;
        let mut original = receipt(&client, 100, 2).await;
        // The receipt may omit on-idle and backend-filtered removal events.
        original
            .events
            .retain(|event| event.event_id == "NetworkRegistrationQueued");
        let (sender, mut receiver) = tokio::sync::mpsc::channel(8);
        let opts = ExecuteOptions {
            progress: Some(sender),
            ..bounded()
        };
        let result = complete(&client, original, OWNER, HOTKEY, &opts, false, None)
            .await
            .unwrap();
        assert_eq!(result.data["registration_mode"], "after_deregistration");
        assert_eq!(result.data["queued_at_block"], 100);
        assert_eq!(result.data["registered_at_block"], 100);
        assert_eq!(result.data["deregistered_netuid"], 7);
        assert_eq!(result.data["cleanup_netuid"], 7);
        assert_eq!(result.data["registration_price_rao"], 777);
        assert_eq!(receiver.recv().await.unwrap().stage, "queued");
        let progress = receiver.recv().await.unwrap();
        assert_eq!(progress.stage, "registered");
        assert_eq!(progress.data["mode"], "after_deregistration");
    }

    #[tokio::test]
    async fn backfill_rejects_unrelated_and_later_same_identity_registration() {
        let mut state = FakeState::default();
        let inclusion = state.block_hash(100).unwrap();
        state
            .seed_events(&inclusion, json!([queued(100, 0)]))
            .unwrap();
        at(
            &mut state,
            &inclusion,
            "CurrentDissolveCleanupStatus",
            &[],
            json!({"netuid":7}),
        )
        .unwrap();
        state.advance(3);
        for (height, netuid, owner, hotkey) in [
            (101, 40, AccountId([9; 32]), HOTKEY),
            (102, 41, OWNER, HOTKEY),
            (103, 42, OWNER, HOTKEY),
        ] {
            let hash = state.block_hash(height).unwrap();
            identities(&mut state, &hash, netuid, owner, hotkey);
            at(
                &mut state,
                &hash,
                "NetworkRegistrationQueue",
                &[],
                if height == 102 {
                    json!([{"coldkey":OWNER,"hotkey":HOTKEY,"registration_block":100}])
                } else {
                    json!([])
                },
            )
            .unwrap();
            let events = if height == 102 {
                json!([queued(102, 1), added(netuid, None)])
            } else {
                json!([added(netuid, None)])
            };
            state.seed_events(&hash, events).unwrap();
        }
        let (client, fake) = fixture(state).await;
        let original = receipt(&client, 100, 0).await;
        let result = complete(&client, original, OWNER, HOTKEY, &bounded(), false, None)
            .await
            .unwrap();
        assert_eq!(result.data["netuid"], 42);
        assert_eq!(result.data["registered_at_block"], 103);
        assert_eq!(result.data["queued_at_block"], 100);
        assert_eq!(result.data["cleanup_netuid"], 7);
        // State reads must retain the exact event block, including identity and queue reads.
        for height in 100..=103 {
            let hash = fake.mutate_state(|state| state.block_hash(height).unwrap());
            assert!(
                fake.requests().iter().any(
                    |request| request.method == "state_getStorage" && request.params[1] == hash
                )
            );
        }
    }

    #[tokio::test]
    async fn deferred_carrier_finds_later_queue_and_cleanup_queue_fallback() {
        let mut state = FakeState::default();
        state.advance(2);
        let queued_hash = state.block_hash(101).unwrap();
        state
            .seed_events(
                &queued_hash,
                json!([event("NetworkRemoved", json!(99), Some(9)), queued(101, 2),]),
            )
            .unwrap();
        at(
            &mut state,
            &queued_hash,
            "DissolveCleanupQueue",
            &[],
            json!([7, 8]),
        )
        .unwrap();
        let completed_hash = state.block_hash(102).unwrap();
        identities(&mut state, &completed_hash, 42, OWNER, HOTKEY);
        at(
            &mut state,
            &completed_hash,
            "NetworkRegistrationQueue",
            &[],
            json!([]),
        )
        .unwrap();
        state
            .seed_events(&completed_hash, json!([added(42, None)]))
            .unwrap();
        let (client, _) = fixture(state).await;
        let original = receipt(&client, 100, 0).await;
        let result = complete(&client, original, OWNER, HOTKEY, &bounded(), true, None)
            .await
            .unwrap();
        assert_eq!(result.data["queued_at_block"], 101);
        assert_eq!(result.data["cleanup_netuid"], 7);
        assert!(!result.data.contains_key("deregistered_netuid"));
        assert_eq!(result.data["registered_at_block"], 102);
    }

    #[tokio::test]
    async fn registration_and_total_deadlines_bound_live_wait_and_progress_backpressure() {
        for total in [false, true] {
            for backpressure in [false, true] {
                let mut state = FakeState::default();
                if backpressure {
                    let hash = state.block_hash(100).unwrap();
                    identities(&mut state, &hash, 42, OWNER, HOTKEY);
                    state
                        .seed_events(&hash, json!([queued(100, 0), added(42, None)]))
                        .unwrap();
                    at(
                        &mut state,
                        &hash,
                        "NetworkRegistrationQueue",
                        &[],
                        json!([]),
                    )
                    .unwrap();
                }
                let (client, fake) = fixture(state).await;
                let original = receipt(&client, 100, 0).await;
                let (sender, _receiver) = tokio::sync::mpsc::channel(1);
                let opts = ExecuteOptions {
                    registration_timeout: Some(if total {
                        Duration::from_secs(20)
                    } else {
                        Duration::from_millis(40)
                    }),
                    progress: Some(sender),
                    ..Default::default()
                };
                let deadline = total.then(|| Instant::now() + Duration::from_millis(40));
                let error = complete(&client, original, OWNER, HOTKEY, &opts, true, deadline)
                    .await
                    .unwrap_err();
                assert!(matches!(
                    error,
                    TxError::Rpc(rtao_rpc::RpcError::Timeout { .. })
                ));
                assert!(
                    !fake
                        .requests()
                        .iter()
                        .any(|request| request.method.starts_with("author_"))
                );
            }
        }
    }

    #[tokio::test]
    async fn closing_client_cancels_an_unbounded_registration_wait() {
        let (client, fake) = fixture(FakeState::default()).await;
        let original = receipt(&client, 100, 0).await;
        let worker_client = client.clone();
        let task = tokio::spawn(async move {
            complete(
                &worker_client,
                original,
                OWNER,
                HOTKEY,
                &ExecuteOptions::default(),
                true,
                None,
            )
            .await
        });
        wait_subscribed(&fake).await;
        client.close().await;
        assert!(matches!(
            task.await.unwrap(),
            Err(TxError::ConnectionNotReady)
        ));
    }

    #[tokio::test]
    async fn an_orphaned_inclusion_cannot_be_followed_as_a_new_request() {
        let mut state = FakeState::default();
        state.set_finalized(99).unwrap();
        let original_hash = state.block_hash(100).unwrap();
        state
            .seed_events(&original_hash, json!([queued(100, 0)]))
            .unwrap();
        let (client, fake) = fixture(state).await;
        let original = receipt(&client, 100, 0).await;
        fake.mutate_state(|state| {
            let parent = state.block_hash(99).unwrap();
            let replacement = state.fork(&parent).unwrap();
            state.set_best(&replacement).unwrap();
        });
        let error = complete(&client, original, OWNER, HOTKEY, &bounded(), false, None)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("inclusion was reorganized"));
    }

    #[tokio::test]
    async fn live_best_reorg_discards_queue_state_from_abandoned_blocks() {
        let mut state = FakeState::default();
        state.set_finalized(99).unwrap();
        let (client, fake) = fixture(state).await;
        let mut original = receipt(&client, 100, 0).await;
        original.inclusion = Inclusion::InBlock;
        let worker_client = client.clone();
        let (sender, mut receiver) = tokio::sync::mpsc::channel(16);
        let task = tokio::spawn(async move {
            let opts = ExecuteOptions {
                progress: Some(sender),
                wait_for_finalization: false,
                ..bounded()
            };
            complete(&worker_client, original, OWNER, HOTKEY, &opts, true, None).await
        });
        wait_subscribed(&fake).await;
        let old_header = fake.mutate_state(|state| {
            let header = state.advance(1).pop().unwrap();
            let hash = state.block_hash(101).unwrap();
            state.seed_events(&hash, json!([queued(101, 0)])).unwrap();
            at(state, &hash, "DissolveCleanupQueue", &[], json!([7])).unwrap();
            header
        });
        fake.publish_method("chain_subscribeNewHeads", old_header)
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while receiver.recv().await.unwrap().stage != "queued" {}
        })
        .await
        .unwrap();
        let replacement = fake.mutate_state(|state| {
            let parent = state.block_hash(100).unwrap();
            let hash = state.fork(&parent).unwrap();
            state.set_best(&hash).unwrap();
            identities(state, &hash, 42, OWNER, HOTKEY);
            state.seed_events(&hash, json!([added(42, None)])).unwrap();
            at(state, &hash, "NetworkRegistrationQueue", &[], json!([])).unwrap();
            state.header(&hash).unwrap()
        });
        fake.publish_method("chain_subscribeNewHeads", replacement)
            .unwrap();
        let result = task.await.unwrap().unwrap();
        assert_eq!(result.data["registration_mode"], "immediate");
        assert_eq!(result.data["registered_at_block"], 101);
        assert!(!result.data.contains_key("queued_at_block"));
        assert!(!result.data.contains_key("cleanup_netuid"));
    }
}
