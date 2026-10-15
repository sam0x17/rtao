// Derived from RaoFoundation/subtensor sdk/python/bittensor/client.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Actual async waits over a finite loopback FakeNode, without a chain service.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime},
};

use rtao_client::{Client, ClientConfig, EpochEvent, WaitError};
use rtao_core::codec::Value as CoreValue;
use rtao_primitives::NetUid;
use rtao_testkit::{FakeNode, FakeState, Reply};
use serde_json::{Value, json};
use tokio::task::JoinHandle;

type TestResult = Result<(), Box<dyn std::error::Error>>;
const NETUID: NetUid = NetUid(1);
const BEST_SUBSCRIPTION: &str = "client-wait-best";

fn timestamp_key(state: &FakeState) -> Vec<u8> {
    state
        .runtime
        .storage_key(
            state.runtime.storage_entry("Timestamp", "Now").unwrap(),
            &[],
        )
        .unwrap()
}
fn epoch_key(state: &FakeState) -> Vec<u8> {
    state
        .runtime
        .storage_key(
            state
                .runtime
                .storage_entry("SubtensorModule", "LastEpochBlock")
                .unwrap(),
            &[CoreValue::Int(1)],
        )
        .unwrap()
}

async fn setup(tempo: u16, slot_millis: u64) -> (Client, FakeNode) {
    let mut state = FakeState::default();
    state
        .seed_python("Timestamp", "Now", &[], json!(1000))
        .unwrap();
    state
        .seed_typed("SubtensorModule", "Tempo", (NETUID,), tempo)
        .unwrap();
    state
        .seed_typed("SubtensorModule", "LastEpochBlock", (NETUID,), 90u64)
        .unwrap();
    state
        .seed_constant("Aura", "SlotDuration", json!(slot_millis))
        .unwrap();
    let head = state.header(&state.block_hash(100).unwrap()).unwrap();
    state.queue_reply(
        "chain_subscribeNewHeads",
        Reply::subscription(BEST_SUBSCRIPTION, vec![head], false),
    );
    let node = FakeNode::with_state(Arc::new(Mutex::new(state)))
        .await
        .unwrap();
    let mut config = ClientConfig::for_network(node.url()).unwrap();
    config.runtime_cache_dir = None;
    config.config_store = None;
    config.session.rate_limit = None;
    let client = Client::connect(config).await.unwrap();
    (client, node)
}

fn reads_of(node: &FakeNode, key: &[u8]) -> usize {
    let key = json!(format!("0x{}", hex::encode(key)));
    node.requests()
        .iter()
        .filter(|request| match request.method.as_str() {
            "state_getStorage" => request.params[0] == key,
            "state_queryStorageAt" => request.params[0]
                .as_array()
                .is_some_and(|keys| keys.contains(&key)),
            _ => false,
        })
        .count()
}

async fn observed_reads(node: &FakeNode, key: &[u8], minimum: usize) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while reads_of(node, key) < minimum {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
}

async fn observed_method(node: &FakeNode, method: &str, minimum: usize) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while node
            .requests()
            .iter()
            .filter(|request| request.method == method)
            .count()
            < minimum
        {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
}

async fn pending<T>(task: &mut JoinHandle<T>) {
    assert!(
        tokio::time::timeout(Duration::from_millis(20), task)
            .await
            .is_err()
    );
}

fn advance(node: &FakeNode, millis: u64, epoch: u64) -> Value {
    let header = node.mutate_state(|state| {
        let header = state.advance(1).pop().unwrap();
        let hash = state.block_hash(state.block).unwrap();
        state.seed_at(
            hash.clone(),
            timestamp_key(state),
            Some(millis.to_le_bytes().to_vec()),
        );
        state.seed_at(hash, epoch_key(state), Some(epoch.to_le_bytes().to_vec()));
        header
    });
    node.push_subscription_update(BEST_SUBSCRIPTION, header.clone())
        .unwrap();
    header
}

async fn finish<T>(task: JoinHandle<T>) -> T {
    tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn waits_for_next_and_explicit_blocks_return_observed_headers() -> TestResult {
    let (client, mut node) = setup(10, 100).await;
    let headers = node
        .requests()
        .iter()
        .filter(|request| request.method == "chain_getHeader")
        .count();
    let handle = client.clone();
    let mut next = tokio::spawn(async move { handle.wait_for_block(None, None).await });
    observed_method(&node, "chain_getHeader", headers + 1).await;
    pending(&mut next).await;
    let first = advance(&node, 1100, 90);
    let result = finish(next).await?;
    assert_eq!(result.number, 101);
    assert_eq!(result.raw, first);

    let handle = client.clone();
    let mut target = tokio::spawn(async move { handle.wait_for_block(Some(103), None).await });
    advance(&node, 1200, 90);
    pending(&mut target).await;
    let third = advance(&node, 1300, 90);
    let result = finish(target).await?;
    assert_eq!(result.number, 103);
    assert_eq!(result.raw, third);
    client.close().await;
    node.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn epoch_wait_ignores_height_predictions_and_returns_changed_storage() -> TestResult {
    let (client, mut node) = setup(10, 100).await;
    let key = node.mutate_state(|state| epoch_key(state));
    let before = reads_of(&node, &key);
    let handle = client.clone();
    let mut epoch = tokio::spawn(async move { handle.wait_for_epoch(NETUID, None).await });
    observed_reads(&node, &key, before + 2).await;
    advance(&node, 1100, 90);
    observed_reads(&node, &key, before + 3).await;
    pending(&mut epoch).await;
    advance(&node, 1200, 97);
    assert_eq!(
        finish(epoch).await?,
        EpochEvent {
            netuid: NETUID,
            block: 97,
            previous: 90
        }
    );
    assert!(
        node.requests()
            .iter()
            .filter(|request| request.method == "state_getStorage"
                && request.params[0] == json!(format!("0x{}", hex::encode(&key))))
            .all(|request| request.params.as_array().unwrap().len() == 2)
    );
    client.close().await;
    node.shutdown().await;

    let (client, mut node) = setup(0, 100).await;
    assert!(matches!(
        client.wait_for_epoch(NETUID, None).await,
        Err(WaitError::EpochDisabled(NETUID))
    ));
    client.close().await;
    node.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn timestamp_wait_sleeps_for_long_chain_gap_and_uses_chain_clock() -> TestResult {
    let (client, mut node) = setup(10, 10).await;
    let key = node.mutate_state(|state| timestamp_key(state));
    let before = reads_of(&node, &key);
    let started = Instant::now();
    let handle = client.clone();
    // This target is long past on the host clock, but future on the fixture chain.
    let mut wait = tokio::spawn(async move {
        handle
            .wait_for_timestamp(SystemTime::UNIX_EPOCH + Duration::from_millis(1200), None)
            .await
    });
    observed_reads(&node, &key, before + 1).await;
    advance(&node, 1100, 90);
    observed_reads(&node, &key, before + 2).await;
    assert!(started.elapsed() >= Duration::from_millis(150));
    pending(&mut wait).await;
    let observed = advance(&node, 1200, 90);
    let result = finish(wait).await?;
    assert_eq!(result.number, 102);
    assert_eq!(result.raw, observed);
    client.close().await;
    node.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn every_wait_keeps_zero_and_finite_budgets_distinct_from_none() -> TestResult {
    let (client, mut node) = setup(10, 100).await;
    let target = SystemTime::UNIX_EPOCH + Duration::from_millis(1150);
    for budget in [Duration::ZERO, Duration::from_millis(10)] {
        assert!(matches!(
            client.wait_for_block(Some(101), Some(budget)).await,
            Err(WaitError::Timeout)
        ));
        assert!(matches!(
            client.wait_for_timestamp(target, Some(budget)).await,
            Err(WaitError::Timeout)
        ));
        assert!(matches!(
            client.wait_for_epoch(NETUID, Some(budget)).await,
            Err(WaitError::Timeout)
        ));
    }
    let key = node.mutate_state(|state| epoch_key(state));
    let before = reads_of(&node, &key);
    let handle = client.clone();
    let mut block = tokio::spawn(async move { handle.wait_for_block(Some(101), None).await });
    let handle = client.clone();
    let mut timestamp = tokio::spawn(async move { handle.wait_for_timestamp(target, None).await });
    let handle = client.clone();
    let mut epoch = tokio::spawn(async move { handle.wait_for_epoch(NETUID, None).await });
    observed_reads(&node, &key, before + 2).await;
    pending(&mut block).await;
    pending(&mut timestamp).await;
    pending(&mut epoch).await;
    advance(&node, 1200, 95);
    assert_eq!(finish(block).await?.number, 101);
    assert_eq!(finish(timestamp).await?.number, 101);
    assert_eq!(
        finish(epoch).await?,
        EpochEvent {
            netuid: NETUID,
            block: 95,
            previous: 90
        }
    );
    client.close().await;
    node.shutdown().await;
    Ok(())
}
