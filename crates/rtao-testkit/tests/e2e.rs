//! Live localnet scenarios. CI supplies a separately bootstrapped endpoint.
//!
//! Framework scenarios use production scheduler, executor and transport services.
#![allow(clippy::panic, clippy::expect_used, clippy::unwrap_used)]

use std::{
    collections::BTreeSet,
    error::Error,
    time::{Duration, Instant},
};

use futures::{StreamExt, future::try_join_all};
use rtao_client::{BlockRef, Client, ExtrinsicReport, SubmitOptions, View, ViewExt, Wait};
use rtao_core::{codec::Value, value_ext::ValueExt};
use rtao_primitives::{AccountId, NetUidStorageIndex, TaoBalance};
use rtao_runtime::{
    calls::{admin_utils, balances, subtensor_module},
    storage::{subtensor_module as storage, system},
};
use rtao_signer::{KeypairSigner, Signer};
use rtao_testkit::{Localnet, LocalnetProfile, RecordingView, Scenario};
use serde_json::{Value as Json, json};

type TestResult = Result<(), Box<dyn Error>>;

#[path = "e2e/node_examples.rs"]
mod node_examples;

use node_examples::neurons_api;

#[path = "e2e/emission_weights.rs"]
mod emission_weights;
#[path = "e2e/framework.rs"]
mod framework;
#[path = "e2e/lost_watch.rs"]
mod lost_watch;
#[path = "e2e/pr_u4.rs"]
mod pr_u4;
#[path = "e2e/upgrade.rs"]
mod upgrade;
#[path = "e2e/watch_proxy.rs"]
mod watch_proxy;

async fn localnet() -> Result<Localnet, Box<dyn Error>> {
    // Creating/bootstrapping a container belongs to the workflow. Accidental
    // local invocations must fail before any Docker or chain mutation.
    let endpoint = std::env::var("E2E_ENDPOINT")
        .map_err(|_| "E2E_ENDPOINT is required; run xtask localnet bootstrap first")?;
    let profile = LocalnetProfile::parse(
        &std::env::var("E2E_BLOCKS").map_err(|_| "E2E_BLOCKS=fast|slow is required")?,
    )?;
    Ok(Localnet::connect(&endpoint, profile).await?)
}

fn options() -> SubmitOptions {
    SubmitOptions {
        wait: Wait::Inclusion,
        deadline: Some(Duration::from_secs(240)),
        ..Default::default()
    }
}

async fn wait_for_height(client: &Client, height: u64) -> TestResult {
    let deadline = Instant::now() + Duration::from_secs(240);
    loop {
        if client.at(BlockRef::Best).await?.block().number >= height {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(format!("node did not reach block {height} in 240 seconds").into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn assert_dispatch(report: &ExtrinsicReport, name: &str) {
    assert!(!report.success, "expected {name}, report: {report:?}");
    assert_eq!(
        report.error.as_ref().map(|error| error.name.as_str()),
        Some(name)
    );
}

// upstream e2e.rs::test_block_number
#[tokio::test]
async fn test_block_number() -> TestResult {
    let ln = localnet().await?;
    assert!(ln.client().at(BlockRef::Best).await?.block().number > 0);
    Ok(())
}

// upstream e2e.rs::test_block_subscription_streams_increasing_headers
#[tokio::test]
async fn test_block_subscription_streams_increasing_headers() -> TestResult {
    let ln = localnet().await?;
    let mut blocks = ln.client().blocks(rtao_client::Finality::Finalized)?;
    let first = tokio::time::timeout(Duration::from_secs(60), blocks.next())
        .await?
        .ok_or("finalized block stream ended")??;
    let second = tokio::time::timeout(Duration::from_secs(60), blocks.next())
        .await?
        .ok_or("finalized block stream ended")??;
    assert!(second.number > first.number);
    assert_ne!(second.hash, first.hash);
    Ok(())
}

// upstream e2e.rs::test_generic_accessors_over_generated_descriptors
#[tokio::test]
async fn test_generic_accessors_over_generated_descriptors() -> TestResult {
    let ln = localnet().await?;
    node_examples::typed_reads(ln.client(), rtao_primitives::NetUid(1)).await
}

// upstream e2e.rs::test_snapshot_pins_reads_to_one_block
#[tokio::test]
async fn test_snapshot_pins_reads_to_one_block() -> TestResult {
    let ln = localnet().await?;
    let snapshot = ln.client().at(BlockRef::Best).await?;
    let recorded = RecordingView::new(snapshot.clone());
    let slot = ln.slot()?;
    let before = recorded
        .storage(&system::ACCOUNT, (slot.owner.account(),))
        .await?;
    let neurons_before = recorded
        .call(&neurons_api(), (rtao_primitives::NetUid(1),))
        .await?;
    wait_for_height(ln.client(), snapshot.block().number + 2).await?;
    let after = recorded
        .storage(&system::ACCOUNT, (slot.owner.account(),))
        .await?;
    let neurons_after = recorded
        .call(&neurons_api(), (rtao_primitives::NetUid(1),))
        .await?;
    assert_eq!(before, after);
    assert_eq!(neurons_before, neurons_after);
    assert_eq!(
        recorded.block_hashes(),
        BTreeSet::from([snapshot.block().hash])
    );
    assert_eq!(recorded.touches().len(), 4);
    Ok(())
}

// upstream e2e.rs::test_compose_nested_sudo_call
#[tokio::test]
async fn test_compose_nested_sudo_call() -> TestResult {
    let ln = localnet().await?;
    let before = ln.client().storage(&storage::TX_RATE_LIMIT, ()).await?;
    let value = before.checked_add(1).ok_or("TxRateLimit overflow")?;
    let changed = ln
        .sudo(admin_utils::sudo_set_tx_rate_limit(value).compose(&ln.client().runtime().await?)?)
        .await;
    let observed = ln.client().storage(&storage::TX_RATE_LIMIT, ()).await;
    let restored = ln
        .sudo(admin_utils::sudo_set_tx_rate_limit(before).compose(&ln.client().runtime().await?)?)
        .await?;
    assert!(restored.success, "global TxRateLimit restoration failed");
    let changed = changed?;
    assert!(changed.success, "nested Sudo failed: {changed:?}");
    assert!(
        changed
            .events
            .iter()
            .any(|event| event.matches("Sudo", "Sudid"))
    );
    assert_eq!(observed?, value);
    Ok(())
}

// upstream e2e.rs::test_reads_catalog_nonempty
#[tokio::test]
async fn test_reads_catalog_nonempty() -> TestResult {
    let _ln = localnet().await?;
    assert!(rtao_catalog::READS.len() >= 12);
    assert_eq!(
        rtao_catalog::READS
            .iter()
            .map(|read| read.name)
            .collect::<BTreeSet<_>>()
            .len(),
        rtao_catalog::READS.len()
    );
    Ok(())
}

#[tokio::test]
async fn scenario_02_direct_weights_obey_rate_limit() -> TestResult {
    let ln = localnet().await?;
    let setup = Scenario::new(&ln)
        .owner()
        .subnet()
        .start()
        .validator()
        .stake(rtao_testkit::VALIDATOR_STAKE)
        .commit_reveal(false)
        .build()
        .await?;
    let netuid = setup.netuid.ok_or("subnet was not created")?;
    let validator = setup
        .validator
        .as_ref()
        .ok_or("validator was not registered")?;
    let uid = ln
        .client()
        .storage(&storage::UIDS, (netuid, validator.account()))
        .await?
        .ok_or("validator UID missing")?;
    let key = NetUidStorageIndex::new(netuid, rtao_primitives::MechId(0))?;
    let version = ln
        .client()
        .storage(&storage::WEIGHTS_VERSION_KEY, (netuid,))
        .await?;
    let call = subtensor_module::set_weights(netuid, vec![uid], vec![u16::MAX], version);
    let first = ln
        .client()
        .submit(
            call.compose(&ln.client().runtime().await?)?,
            validator,
            &options(),
        )
        .await?;
    assert!(first.success, "first direct set failed: {first:?}");
    assert!(
        first
            .events
            .iter()
            .any(|event| event.matches("SubtensorModule", "WeightsSet"))
    );
    let last_update = ln.client().storage(&storage::LAST_UPDATE, (key,)).await?;
    let updated_at = *last_update
        .get(usize::from(uid))
        .ok_or("validator LastUpdate missing")?;
    assert_eq!(Some(updated_at), first.block_number);
    let rate_limit = 8;
    let knob = admin_utils::sudo_set_weights_set_rate_limit(netuid, rate_limit)
        .compose(&ln.client().runtime().await?)?;
    let configured = ln.sudo(knob).await?;
    assert!(
        configured.success,
        "rate-limit setup failed: {configured:?}"
    );
    assert_eq!(
        ln.client()
            .storage(&storage::WEIGHTS_SET_RATE_LIMIT, (netuid,))
            .await?,
        rate_limit
    );
    // The runtime checks the weights rate limit while validating the
    // transaction (SubtensorTransactionExtension / CheckRateLimits), so an
    // early set is refused by the pool with RateLimitExceeded and never
    // reaches a block as a SettingWeightsTooFast dispatch failure.
    let refused = ln
        .client()
        .submit(
            call.compose(&ln.client().runtime().await?)?,
            validator,
            &options(),
        )
        .await;
    let refused_at = ln.client().at(BlockRef::Best).await?.block().number;
    match refused {
        Err(rtao_client::SubmitFailure::Pool(rejection)) => {
            assert_eq!(
                rejection.name(),
                Some("RateLimitExceeded"),
                "unexpected pool refusal: {rejection:?}"
            );
        }
        other => panic!("rate-limited set was not refused by the pool: {other:?}"),
    }
    assert!(
        refused_at < updated_at + rate_limit,
        "refusal observed after the rate window closed at {}",
        updated_at + rate_limit
    );
    assert_eq!(
        ln.client().storage(&storage::LAST_UPDATE, (key,)).await?,
        last_update
    );
    wait_for_height(ln.client(), updated_at + rate_limit).await?;
    let resumed = ln
        .client()
        .submit(
            call.compose(&ln.client().runtime().await?)?,
            validator,
            &options(),
        )
        .await?;
    assert!(resumed.success, "set after rate window failed: {resumed:?}");
    assert!(
        resumed
            .block_number
            .is_some_and(|height| height >= updated_at + rate_limit)
    );
    assert_eq!(
        ln.client().storage(&storage::WEIGHTS, (key, uid)).await?,
        vec![(uid, u16::MAX)]
    );
    Ok(())
}

#[tokio::test]
async fn scenario_15_unregistered_owner_is_native_zero() -> TestResult {
    let ln = localnet().await?;
    let slot = ln.slot()?;
    // Nextest may reuse a slot after an earlier test registered its funded hotkey.
    // This read-only case uses a separate URI that no setup path registers.
    let unregistered = KeypairSigner::from_uri(
        &format!("//rtao-e2e//slot/{}//unregistered-owner-probe", slot.index),
        rtao_primitives::CryptoType::Sr25519,
    )?;
    let snapshot = ln.client().at(BlockRef::Best).await?;
    let owner_key = snapshot.block().runtime.core().storage_key(
        snapshot
            .block()
            .runtime
            .storage_entry("SubtensorModule", "Owner")
            .ok_or("Owner storage missing")?,
        &[Value::Bytes(unregistered.account().0.to_vec())],
    )?;
    assert!(
        snapshot.storage_raw(&owner_key).await?.is_none(),
        "unregistered probe hotkey already has an owner"
    );
    let owner = snapshot
        .storage(&storage::OWNER, (unregistered.account(),))
        .await?;
    assert_eq!(owner.raw().as_bytes(), &[0; 32]);
    assert_eq!(owner.get(), None);
    Ok(())
}

#[tokio::test]
async fn protocol_cr_version_gate_returns_chain_dispatch_error() -> TestResult {
    let ln = localnet().await?;
    let original = ln
        .client()
        .storage(&storage::COMMIT_REVEAL_WEIGHTS_VERSION, ())
        .await?;
    let setup = Scenario::new(&ln)
        .owner()
        .subnet()
        .start()
        .validator()
        .stake(rtao_testkit::VALIDATOR_STAKE)
        .commit_reveal(true)
        .build()
        .await?;
    let netuid = setup.netuid.ok_or("subnet was not created")?;
    let validator = setup
        .validator
        .as_ref()
        .ok_or("validator was not registered")?;
    let configured = ln
        .sudo(
            admin_utils::sudo_set_commit_reveal_version(5)
                .compose(&ln.client().runtime().await?)?,
        )
        .await?;
    assert!(configured.success);
    // Restore the global knob even if submission/receipt resolution fails.
    let outcome = async {
        // Transaction validation refuses a reveal round older than the last
        // stored drand round (InvalidRevealRound), so target a future round to
        // reach the dispatch-time version check.
        let latest = ln
            .client()
            .storage(&rtao_runtime::storage::drand::LAST_STORED_ROUND, ())
            .await?;
        let reveal_round = latest.checked_add(1_000).ok_or("drand round overflow")?;
        let call = subtensor_module::commit_timelocked_weights(netuid, vec![], reveal_round, 4)
            .compose(&ln.client().runtime().await?)?;
        Ok::<_, Box<dyn Error>>(ln.client().submit(call, validator, &options()).await?)
    }
    .await;
    let restored = ln
        .sudo(
            admin_utils::sudo_set_commit_reveal_version(original)
                .compose(&ln.client().runtime().await?)?,
        )
        .await?;
    assert!(restored.success, "global CR version restoration failed");
    let report = outcome?;
    assert_dispatch(&report, "IncorrectCommitRevealVersion");
    assert!(
        !report
            .events
            .iter()
            .any(|event| event.matches("SubtensorModule", "TimelockedWeightsCommitted"))
    );
    Ok(())
}

#[tokio::test]
async fn protocol_client_clones_pipeline_fifty_transfer_nonces() -> TestResult {
    let ln = localnet().await?;
    let slot = ln.slot()?;
    let before = ln.client().nonces().fresh(&slot.owner.account()).await?;
    let receiver_before = ln
        .client()
        .storage(&system::ACCOUNT, (slot.hotkey.account(),))
        .await?;
    let clients = [ln.client().clone(), ln.client().clone()];
    let call = balances::transfer_keep_alive(
        Value::Bytes(slot.hotkey.account().0.to_vec()).into(),
        TaoBalance(1_000_000),
    );
    let call = call.compose(&ln.client().runtime().await?)?;
    let opts = options();
    let reports = try_join_all(
        (0..50).map(|index| clients[index % 2].submit(call.clone(), &slot.owner, &opts)),
    )
    .await?;
    assert_eq!(reports.len(), 50);
    assert!(reports.iter().all(|report| report.success));
    assert_eq!(
        reports
            .iter()
            .map(|report| report.hash)
            .collect::<BTreeSet<_>>()
            .len(),
        50
    );
    assert_eq!(
        ln.client().nonces().fresh(&slot.owner.account()).await?,
        before + 50
    );
    let receiver_after = ln
        .client()
        .storage(&system::ACCOUNT, (slot.hotkey.account(),))
        .await?;
    let free = |value: &rtao_core::DynValue| -> Result<u128, Box<dyn Error>> {
        value
            .as_core()
            .field("data")
            .and_then(|data| data.field("free"))
            .and_then(ValueExt::as_u128)
            .ok_or_else(|| "System.Account.data.free missing".into())
    };
    assert_eq!(free(&receiver_after)? - free(&receiver_before)?, 50_000_000);
    // Read the actual included extrinsic bytes and prove an uninterrupted nonce
    // sequence, rather than relying on the client's cache or report count.
    let mut nonces = BTreeSet::new();
    for report in &reports {
        let hash = report.block_hash.ok_or("transfer has no inclusion block")?;
        let block: Json = ln
            .client()
            .request(
                "chain_getBlock",
                json!([format!("0x{}", hex::encode(hash.0))]),
            )
            .await?;
        let index = report
            .extrinsic_index
            .ok_or("transfer has no inclusion index")?;
        let encoded = block["block"]["extrinsics"][index as usize]
            .as_str()
            .ok_or("included transfer bytes missing")?;
        let bytes = hex::decode(encoded.strip_prefix("0x").ok_or("extrinsic is not hex")?)?;
        let runtime = ln
            .client()
            .at(BlockRef::Hash(hash))
            .await?
            .block()
            .runtime
            .clone();
        let decoded = runtime.core().decode_extrinsic(&bytes, true)?;
        let address = decoded
            .field("address")
            .ok_or("signed transfer address missing")?;
        let address = address.variant("Id").unwrap_or(address);
        if let Some(address) = address.as_str() {
            assert_eq!(address.parse::<AccountId>()?, slot.owner.account());
        } else {
            assert_eq!(
                address
                    .value_bytes()
                    .ok_or("signed transfer address has unknown shape")?
                    .as_slice(),
                slot.owner.account().as_bytes()
            );
        }
        let nonce = decoded
            .field("nonce")
            .and_then(ValueExt::as_u128)
            .ok_or("signed transfer nonce missing")?;
        nonces.insert(u64::try_from(nonce)?);
    }
    assert_eq!(nonces, (before..before + 50).collect());
    Ok(())
}

#[tokio::test]
async fn scenario_01_validator_every_commit_reveals() -> TestResult {
    framework::scenario_01().await
}

#[tokio::test]
async fn scenario_03_two_mechanism_commits() -> TestResult {
    framework::scenario_03().await
}

#[tokio::test]
async fn scenario_05_validator_version_key_remediation() -> TestResult {
    framework::scenario_05().await
}

#[tokio::test]
async fn scenario_06_commit_reveal_version_gate() -> TestResult {
    framework::scenario_06().await
}

#[tokio::test]
async fn scenario_07_lost_watch_reconciliation() -> TestResult {
    let ln = localnet().await?;
    lost_watch::run(&ln).await
}

#[tokio::test]
async fn scenario_08_runtime_upgrade_mid_session() -> TestResult {
    let ln = localnet().await?;
    upgrade::run(&ln).await
}

#[tokio::test]
async fn scenario_16_pr_u4_localnet_reveal() -> TestResult {
    pr_u4::scenario_16_pr_u4_localnet_reveal().await
}

#[tokio::test]
async fn scenario_18_emission_weights_pay_root_its_baseline() -> TestResult {
    emission_weights::run(&localnet().await?).await
}

#[tokio::test]
async fn scenario_17_two_executors_fifty_transfers() -> TestResult {
    let ln = localnet().await?;
    let slot = ln.slot()?;
    let before = ln.client().nonces().fresh(&slot.owner.account()).await?;
    let receiver_before = ln
        .client()
        .storage(&system::ACCOUNT, (slot.hotkey.account(),))
        .await?;
    let executors = [
        rtao_tx::Executor::new(ln.client().clone()),
        rtao_tx::Executor::new(ln.client().clone()),
    ];
    let amount = rtao_primitives::Balance::<rtao_primitives::Tao>::from_rao(1_000_000);
    let results = try_join_all((0..50).map(|index| {
        node_examples::execute_transfer(
            &executors[index % 2],
            &slot.owner,
            slot.hotkey.account(),
            amount,
        )
    }))
    .await?;
    assert_eq!(results.len(), 50);
    assert!(
        results
            .iter()
            .all(|result| result.success && result.inclusion == rtao_client::Inclusion::Finalized)
    );
    let mut identities = BTreeSet::new();
    let mut nonces = BTreeSet::new();
    for result in &results {
        let hash = result
            .block_hash
            .ok_or("Executor transfer has no block hash")?;
        let identity = result
            .extrinsic_id
            .as_ref()
            .ok_or("Executor transfer has no extrinsic id")?;
        assert!(
            identities.insert((hash, identity.clone())),
            "two Executor results share one inclusion"
        );
        let index: usize = identity
            .rsplit_once('-')
            .ok_or("invalid extrinsic id")?
            .1
            .parse()?;
        let block: Json = ln
            .client()
            .request(
                "chain_getBlock",
                json!([format!("0x{}", hex::encode(hash.0))]),
            )
            .await?;
        let encoded = block["block"]["extrinsics"]
            .get(index)
            .and_then(Json::as_str)
            .ok_or("Executor result index absent from included block")?;
        let bytes = hex::decode(
            encoded
                .strip_prefix("0x")
                .ok_or("included extrinsic is not hex")?,
        )?;
        let snapshot = ln.client().at(BlockRef::Hash(hash)).await?;
        let decoded = snapshot
            .block()
            .runtime
            .core()
            .decode_extrinsic(&bytes, true)?;
        let address = decoded
            .field("address")
            .ok_or("included transfer has no signer")?;
        let address = address.variant("Id").unwrap_or(address);
        if let Some(address) = address.as_str() {
            assert_eq!(address.parse::<AccountId>()?, slot.owner.account());
        } else {
            assert_eq!(
                address
                    .value_bytes()
                    .ok_or("unknown included signer shape")?
                    .as_slice(),
                slot.owner.account().as_bytes()
            );
        }
        let nonce = decoded
            .field("nonce")
            .and_then(ValueExt::as_u128)
            .ok_or("included transfer has no nonce")?;
        assert!(
            nonces.insert(u64::try_from(nonce)?),
            "Executor pipeline reused a nonce"
        );
    }
    assert_eq!(nonces, (before..before + 50).collect());
    assert_eq!(
        ln.client().nonces().fresh(&slot.owner.account()).await?,
        before + 50
    );
    let receiver_after = ln
        .client()
        .storage(&system::ACCOUNT, (slot.hotkey.account(),))
        .await?;
    let free = |value: &rtao_core::DynValue| {
        value
            .as_core()
            .field("data")
            .and_then(|data| data.field("free"))
            .and_then(ValueExt::as_u128)
            .ok_or("System.Account.data.free missing")
    };
    assert_eq!(
        free(&receiver_after)?.checked_sub(free(&receiver_before)?),
        Some(50_000_000)
    );
    ln.client().close().await;
    Ok(())
}
