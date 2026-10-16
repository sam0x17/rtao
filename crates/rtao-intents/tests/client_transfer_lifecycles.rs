// Derived from RaoFoundation/subtensor sdk/bittensor-core/tests/e2e.rs @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/tests/unit/test_intents_table.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Client transfer, batch, association and auto-stake contracts over finite RPC.
//!
//! Balances, EVM keys and stake destinations are supplied observations, not effects.
//! These fixtures exercise signing, exact submitted calls, receipt decoding and
//! pinned reads. They do not prove runtime economics, atomic application,
//! rollback, registration/ownership, association, reward routing, fees or consensus.
#![cfg(all(feature = "transfer", feature = "batch"))]

use std::{
    collections::{BTreeSet, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result, ensure};
use rtao_client::{
    BlockRef, Client, ClientConfig, Inclusion, Network, ReadKind, Snapshot, ViewExt,
};
use rtao_core::{
    DynValue,
    codec::Value as CoreValue,
    runtime::{Runtime, TypeSpec},
    value_ext::ValueExt,
};
use rtao_intents::{Batch, Transfer};
use rtao_primitives::{AccountId, Balance, CryptoType, Tao, TaoBalance};
use rtao_reads::reads::owner;
use rtao_runtime::{
    CallBytes, RuntimeHandle,
    calls::{balances, utility},
    errors::MODULE_ERRORS,
};
use rtao_signer::{
    BoxFuture, Coldkey, DynKeySource, KeySource, KeyView, KeypairSigner, Signer, SignerError,
    SignerRole,
};
use rtao_testkit::{
    FakeChain, FakeState, RecordingView, Reply, RpcRequest, SubmitScript, fakes::fill_defaults,
    rpc::RpcClient,
};
use rtao_tx::{DynIntent, ExecuteOptions, Executor, ExtrinsicResult};
use serde_json::{Value, json};

const BOB: AccountId = AccountId([3; 32]);
const DAVE: AccountId = AccountId([5; 32]);
const EVE: AccountId = AccountId([6; 32]);
const TAO: u64 = 1_000_000_000;
const NONCE: u64 = 9;
const MAX_STEPS: usize = 8;
// Bound the received hexadecimal string before allocating decoded bytes.
const MAX_AUTHOR_HEX_BYTES: usize = 256 * 1024;
const WATCH: &str = "author_submitAndWatchExtrinsic";

struct DevKeys {
    signer: KeypairSigner,
    acquisitions: AtomicUsize,
}
impl KeyView for DevKeys {
    fn hotkey_address(&self) -> Result<AccountId, SignerError> {
        Ok(AccountId([2; 32]))
    }
    fn hotkey_public_key(&self) -> Result<[u8; 32], SignerError> {
        Ok([2; 32])
    }
    fn coldkey_address(&self) -> Result<AccountId, SignerError> {
        Ok(self.signer.account())
    }
}
impl DevKeys {
    fn acquire(&self, role: SignerRole) -> Result<Arc<dyn Signer>, SignerError> {
        self.acquisitions.fetch_add(1, Ordering::SeqCst);
        if role != SignerRole::Coldkey {
            return Err(SignerError::KeyUnavailable(role));
        }
        Ok(Arc::new(self.signer.clone()))
    }
}
impl KeySource<Coldkey> for DevKeys {
    fn address(&self) -> Result<AccountId, SignerError> {
        self.coldkey_address()
    }
    async fn signer(&self) -> Result<Arc<dyn Signer>, SignerError> {
        self.acquire(SignerRole::Coldkey)
    }
}
impl DynKeySource for DevKeys {
    fn signer_for<'a>(
        &'a self,
        role: SignerRole,
    ) -> BoxFuture<'a, Result<Arc<dyn Signer>, SignerError>> {
        Box::pin(async move { self.acquire(role) })
    }
}

#[derive(Clone)]
struct StorageObservation {
    key: Vec<u8>,
    encoded: Vec<u8>,
}

// This uses metadata directly, independently of owner::balance and the typed
// System.Account descriptor whose actual requests are checked below.
fn account_observation(
    runtime: &Runtime,
    account: AccountId,
    free: u64,
) -> Result<StorageObservation> {
    let entry = runtime
        .storage_entry("System", "Account")
        .context("System.Account metadata")?;
    let key = runtime.storage_key(entry, &[CoreValue::Bytes(account.0.to_vec())])?;
    let value = fill_defaults(runtime, entry.value_type, json!({"data":{"free":free}}))?;
    let encoded = runtime.encode_spec(&TypeSpec::Id(entry.value_type), &value)?;
    Ok(StorageObservation { key, encoded })
}

fn events(success: bool) -> Result<Value> {
    let dispatch_info =
        json!({"weight":{"ref_time":1,"proof_size":0},"class":"Normal","pays_fee":"Yes"});
    let (name, attributes) = if success {
        ("ExtrinsicSuccess", json!({"dispatch_info":dispatch_info}))
    } else {
        let failure = MODULE_ERRORS
            .iter()
            .find(|error| error.pallet == "Balances" && error.name == "InsufficientBalance")
            .context("pinned Balances.InsufficientBalance")?;
        ensure!(
            (failure.pallet_index, failure.error_index) == (5, 2),
            "unexpected pinned error identity"
        );
        (
            "ExtrinsicFailed",
            json!({"dispatch_error":{"Module":{"index":failure.pallet_index,"error":[failure.error_index,0,0,0]}},"dispatch_info":dispatch_info}),
        )
    };
    Ok(
        json!([{"phase":"ApplyExtrinsic","extrinsic_idx":0,"module_id":"System","event_id":name,"attributes":attributes,"topics":[]}]),
    )
}

#[derive(Clone)]
struct Step {
    expected: CallBytes,
    account: AccountId,
    nonce: u64,
    events: Value,
    post: Vec<StorageObservation>,
}

#[derive(Default)]
struct Transcript {
    steps: VecDeque<Step>,
    configured: usize,
    attempts: usize,
    accepted: usize,
    violations: Vec<String>,
}
type Script = Arc<Mutex<Transcript>>;

fn lock_script(script: &Script) -> Result<std::sync::MutexGuard<'_, Transcript>> {
    script
        .lock()
        .map_err(|_| anyhow::anyhow!("poisoned transcript"))
}

fn install_step(chain: &FakeChain, script: &Script, step: Step) -> Result<()> {
    ensure!(
        step.expected.as_bytes().len() <= MAX_AUTHOR_HEX_BYTES / 2,
        "expected call too large"
    );
    ensure!(
        step.post.len() <= MAX_STEPS,
        "too many post-state observations"
    );
    ensure!(
        serde_json::to_vec(&step.events)?.len() <= 16 * 1024,
        "events too large"
    );
    // Fail before queueing a submission if its event encoding is invalid.
    chain.mutate_state(|state| -> Result<()> {
        let entry = state
            .runtime
            .storage_entry("System", "Events")
            .context("System.Events metadata")?;
        let value = fill_defaults(&state.runtime, entry.value_type, step.events.clone())?;
        state
            .runtime
            .encode_spec(&TypeSpec::Id(entry.value_type), &value)?;
        Ok(())
    })?;
    let mut transcript = lock_script(script)?;
    ensure!(
        transcript.configured < MAX_STEPS,
        "transcript step budget exceeded"
    );
    transcript.configured = transcript.configured.saturating_add(1);
    transcript.steps.push_back(step);
    Ok(())
}

fn guarded_author(state: &mut FakeState, request: &RpcRequest, script: &Script) -> Reply {
    let result = (|| -> Result<Reply> {
        let step = {
            let mut transcript = lock_script(script)?;
            transcript.attempts = transcript.attempts.saturating_add(1);
            transcript
                .steps
                .front()
                .cloned()
                .context("author transcript exhausted")?
        };
        ensure!(request.method == WATCH, "unexpected author method");
        let params = request.params.as_array().context("author parameters")?;
        ensure!(params.len() == 1, "author parameter count");
        let raw = params
            .first()
            .and_then(Value::as_str)
            .context("author hexadecimal input")?;
        ensure!(
            raw.len() <= MAX_AUTHOR_HEX_BYTES,
            "author input exceeds byte budget"
        );
        let bytes = hex::decode(raw.strip_prefix("0x").context("author input hex prefix")?)?;
        let decoded = state.runtime.decode_extrinsic(&bytes, true)?;
        let expected =
            state
                .runtime
                .decode_spec(&TypeSpec::Call, step.expected.as_bytes(), true)?;
        ensure!(
            decoded.field("call") == Some(&expected),
            "submitted call differs from independent expectation"
        );
        let account = decoded
            .field("address")
            .and_then(ValueExt::as_str)
            .context("signed author account")?
            .parse::<AccountId>()?;
        ensure!(account == step.account, "unexpected author account");
        ensure!(
            decoded.field("nonce").and_then(ValueExt::as_u128) == Some(u128::from(step.nonce)),
            "unexpected author nonce"
        );
        let signature = decoded
            .field("signature")
            .and_then(|value| value.variant("Ed25519"))
            .and_then(ValueExt::value_bytes)
            .context("Ed25519 author signature")?;
        ensure!(
            signature.len() == 64 && signature.iter().any(|byte| *byte != 0),
            "missing nonzero development signature"
        );
        let previous_height = state.block;
        let previous_count = state.submissions.len();
        state.queue_submit(SubmitScript::InBlockThenFinalized {
            events: step.events,
        });
        // FakeState temporarily removes this handler while invoking it. Its
        // built-in path records the request again, so the script attempt counter
        // is authoritative for script consumption.
        let reply = state.dispatch(request);
        ensure!(
            matches!(&reply.outcome, Some(Ok(Value::String(_)))),
            "author did not acknowledge subscription"
        );
        ensure!(
            state.block == previous_height.checked_add(1).context("height overflow")?,
            "unexpected receipt height"
        );
        ensure!(
            state.submissions.len() == previous_count.saturating_add(1),
            "unexpected submission count"
        );
        ensure!(
            state.submissions.last() == Some(&bytes),
            "submitted bytes changed"
        );
        let hash = state
            .block_hash(state.block)
            .context("receipt block hash")?;
        let statuses = reply
            .notifications
            .iter()
            .map(|item| item.result.clone())
            .collect::<Vec<_>>();
        ensure!(
            statuses
                == vec![
                    json!("ready"),
                    json!({"inBlock":hash}),
                    json!({"finalized":hash})
                ],
            "unexpected receipt transcript"
        );
        for observation in step.post {
            // Install the supplied observation at the receipt hash as well as
            // head state. Seeding current storage alone would leave stale
            // historical values in the just-created receipt snapshot.
            state.seed_at(hash.clone(), observation.key, Some(observation.encoded));
        }
        let mut transcript = lock_script(script)?;
        ensure!(
            transcript.steps.pop_front().is_some(),
            "lost transcript step"
        );
        transcript.accepted = transcript.accepted.saturating_add(1);
        Ok(reply)
    })();
    match result {
        Ok(reply) => reply,
        Err(error) => {
            let mut transcript = script
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if transcript.violations.len() < MAX_STEPS {
                transcript.violations.push(error.to_string());
            }
            Reply::error(-32000, "finite author transcript refused request", None)
        }
    }
}

struct Fixture {
    executor: Executor,
    chain: FakeChain,
    keys: DevKeys,
    script: Script,
}
async fn fixture() -> Result<Fixture> {
    // This is a fixed, public development seed, never a user wallet.
    let keys = DevKeys {
        signer: KeypairSigner::from_seed(&[7; 32], CryptoType::Ed25519)?,
        acquisitions: AtomicUsize::new(0),
    };
    let script = Script::default();
    let mut state = FakeState::try_default()?;
    state.set_nonce(keys.signer.account().to_string(), NONCE);
    for (account, free) in [(BOB, 11 * TAO), (DAVE, 21 * TAO), (EVE, 31 * TAO)] {
        let observation = account_observation(&state.runtime, account, free)?;
        state.seed_encoded(observation.key, Some(observation.encoded));
    }
    for method in [WATCH, "author_submitExtrinsic"] {
        let transcript = script.clone();
        state.set_handler(method, move |state, request| {
            guarded_author(state, request, &transcript)
        });
    }
    let chain = FakeChain::from_state(state);
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    let client = Client::from_rpc(RpcClient::new(chain.clone()), config).await?;
    Ok(Fixture {
        executor: rtao_intents::executor(client),
        chain,
        keys,
        script,
    })
}

fn transfer(destination: AccountId, rao: u64) -> Transfer {
    Transfer {
        dest_ss58: destination.into(),
        amount_tao: Balance::<Tao>::from_rao(rao).into(),
        keep_alive: true,
    }
}
fn expected_transfer(
    runtime: &RuntimeHandle,
    destination: AccountId,
    rao: u64,
) -> Result<CallBytes> {
    Ok(balances::transfer_keep_alive(
        DynValue::new(CoreValue::Bytes(destination.0.to_vec())),
        TaoBalance(rao),
    )
    .compose(runtime)?)
}
fn batch(second: u64) -> Result<Batch> {
    Ok(Batch::new(vec![
        json!({"op":"transfer","dest_ss58":DAVE,"amount_tao":"1"}),
        json!({"op":"transfer","dest_ss58":EVE,"amount_tao":Balance::<Tao>::from_rao(second).to_decimal_string()}),
    ])?)
}
fn options() -> ExecuteOptions {
    ExecuteOptions {
        retries: 0,
        replan_owner_rate_limit: false,
        deadline: Some(Duration::from_secs(5)),
        ..Default::default()
    }
}
async fn execute(fixture: &Fixture, intent: &dyn DynIntent) -> Result<ExtrinsicResult> {
    Ok(tokio::time::timeout(
        Duration::from_secs(10),
        fixture
            .executor
            .execute_dyn(intent, &fixture.keys, &options()),
    )
    .await??)
}

async fn observed_balance(
    fixture: &Fixture,
    snapshot: &Snapshot,
    account: AccountId,
    expected: u64,
) -> Result<u64> {
    let view = RecordingView::new(snapshot.clone());
    let observation = account_observation(snapshot.block().runtime.core(), account, expected)?;
    let start = fixture.chain.requests().len();
    let actual = owner::balance(&view, account).await?;
    assert_eq!(actual.rao, expected);
    let touches = view.touches();
    assert_eq!(touches.len(), 1);
    let touch = touches.first().context("balance descriptor observation")?;
    assert_eq!(touch.kind, ReadKind::Storage);
    assert_eq!((&*touch.pallet_or_api, &*touch.item), ("System", "Account"));
    assert_eq!(touch.at, Some(snapshot.block().hash));
    assert_eq!(view.block_hashes(), BTreeSet::from([snapshot.block().hash]));
    let requests = fixture.chain.requests();
    let reads = requests
        .get(start..)
        .context("balance request window")?
        .iter()
        .filter(|request| {
            request.method.starts_with("state_getStorage")
                || request.method == "state_queryStorageAt"
        })
        .collect::<Vec<_>>();
    assert_eq!(reads.len(), 1);
    let read = reads.first().context("balance storage request")?;
    assert_eq!(read.method, "state_queryStorageAt");
    assert_eq!(
        read.params,
        json!([
            [format!("0x{}", hex::encode(observation.key))],
            format!("0x{}", hex::encode(snapshot.block().hash.0))
        ])
    );
    Ok(actual.rao)
}

async fn install(
    fixture: &Fixture,
    expected: CallBytes,
    success: bool,
    post: &[(AccountId, u64)],
) -> Result<()> {
    let runtime = fixture.executor.client().refresh_runtime().await?;
    let observations = post
        .iter()
        .map(|(account, free)| account_observation(runtime.core(), *account, *free))
        .collect::<Result<Vec<_>>>()?;
    install_step(
        &fixture.chain,
        &fixture.script,
        Step {
            expected,
            account: fixture.keys.signer.account(),
            nonce: NONCE,
            events: events(success)?,
            post: observations,
        },
    )
}

fn assert_consumed(fixture: &Fixture, expected: &CallBytes) -> Result<()> {
    let transcript = lock_script(&fixture.script)?;
    assert_eq!(
        (
            transcript.configured,
            transcript.attempts,
            transcript.accepted,
            transcript.steps.len()
        ),
        (1, 1, 1, 0)
    );
    assert!(transcript.violations.is_empty());
    drop(transcript);
    assert_eq!(fixture.keys.acquisitions.load(Ordering::SeqCst), 1);
    fixture.chain.mutate_state(|state| -> Result<()> {
        assert_eq!(state.submissions.len(), 1);
        let submitted = state.submissions.first().context("submitted extrinsic")?;
        let decoded = state.runtime.decode_extrinsic(submitted, true)?;
        let call = state
            .runtime
            .decode_spec(&TypeSpec::Call, expected.as_bytes(), true)?;
        assert_eq!(decoded.field("call"), Some(&call));
        Ok(())
    })
}

async fn receipt_snapshot(
    fixture: &Fixture,
    result: &ExtrinsicResult,
    before: &Snapshot,
    success: bool,
) -> Result<Snapshot> {
    assert_eq!(result.success, success);
    assert_eq!(result.inclusion, Inclusion::Finalized);
    assert!(result.fate_unknown.is_none());
    if success {
        assert!(result.error.is_none());
    } else {
        assert_eq!(
            result
                .error
                .as_ref()
                .and_then(|error| error.name.as_deref()),
            Some("InsufficientBalance")
        );
    }
    let snapshot = fixture
        .executor
        .client()
        .at(BlockRef::Hash(
            result.block_hash.context("resolved receipt hash")?,
        ))
        .await?;
    // D-023 retains failed receipt events for native multisig/recovery checks.
    // Compare both outcomes with actual decoded events at the pinned block.
    let events = snapshot.events().await?;
    assert!(events.records().iter().any(|event| event.matches(
        "System",
        if success {
            "ExtrinsicSuccess"
        } else {
            "ExtrinsicFailed"
        }
    )));
    assert_eq!(result.events.as_slice(), events.records());
    assert_ne!(snapshot.block().hash, before.block().hash);
    assert_eq!(
        snapshot.block().number,
        before
            .block()
            .number
            .checked_add(1)
            .context("receipt height")?
    );
    assert_eq!(
        result.extrinsic_id,
        Some(format!("{}-0000", snapshot.block().number))
    );
    fixture.chain.mutate_state(|state| {
        assert_eq!(state.block, snapshot.block().number);
        assert_eq!(
            state.block_hash(state.block),
            Some(format!("0x{}", hex::encode(snapshot.block().hash.0)))
        );
    });
    Ok(snapshot)
}

// upstream e2e.rs::test_transfer_moves_exactly_the_amount (3 TAO).
#[tokio::test]
async fn client_transfer_reads_exact_three_tao_delta_from_explicit_receipt_state() -> Result<()> {
    let fixture = fixture().await?;
    let runtime = fixture.executor.client().refresh_runtime().await?;
    let expected = expected_transfer(&runtime, BOB, 3 * TAO)?;
    install(&fixture, expected.clone(), true, &[(BOB, 14 * TAO)]).await?;
    let before = fixture.executor.client().at(BlockRef::Best).await?;
    let old = observed_balance(&fixture, &before, BOB, 11 * TAO).await?;
    let result = execute(&fixture, &transfer(BOB, 3 * TAO)).await?;
    let receipt = receipt_snapshot(&fixture, &result, &before, true).await?;
    let new = observed_balance(&fixture, &receipt, BOB, 14 * TAO).await?;
    assert_eq!(new.checked_sub(old), Some(3 * TAO));
    observed_balance(&fixture, &before, BOB, 11 * TAO).await?;
    assert_consumed(&fixture, &expected)?;
    fixture.executor.client().close().await;
    Ok(())
}

// upstream e2e.rs::test_batch_applies_all_children_atomically (1 + 2 TAO).
#[tokio::test]
async fn client_batch_reads_both_explicit_recipient_deltas_after_success() -> Result<()> {
    let fixture = fixture().await?;
    let runtime = fixture.executor.client().refresh_runtime().await?;
    let expected = utility::batch_all(vec![
        expected_transfer(&runtime, DAVE, TAO)?,
        expected_transfer(&runtime, EVE, 2 * TAO)?,
    ])
    .compose(&runtime)?;
    install(
        &fixture,
        expected.clone(),
        true,
        &[(DAVE, 22 * TAO), (EVE, 33 * TAO)],
    )
    .await?;
    let before = fixture.executor.client().at(BlockRef::Best).await?;
    let dave = observed_balance(&fixture, &before, DAVE, 21 * TAO).await?;
    let eve = observed_balance(&fixture, &before, EVE, 31 * TAO).await?;
    let result = execute(&fixture, &batch(2 * TAO)?).await?;
    let receipt = receipt_snapshot(&fixture, &result, &before, true).await?;
    assert_eq!(
        observed_balance(&fixture, &receipt, DAVE, 22 * TAO)
            .await?
            .checked_sub(dave),
        Some(TAO)
    );
    assert_eq!(
        observed_balance(&fixture, &receipt, EVE, 33 * TAO)
            .await?
            .checked_sub(eve),
        Some(2 * TAO)
    );
    observed_balance(&fixture, &before, DAVE, 21 * TAO).await?;
    observed_balance(&fixture, &before, EVE, 31 * TAO).await?;
    assert_consumed(&fixture, &expected)?;
    fixture.executor.client().close().await;
    Ok(())
}

// upstream e2e.rs::test_failed_batch_reverts_everything. The second amount is
// exactly 10,000,000,000 TAO = 10^19 rao, within the native u64 range.
#[tokio::test]
async fn client_failed_batch_reads_unchanged_first_recipient_from_explicit_receipt_state()
-> Result<()> {
    const SECOND: u64 = 10_000_000_000_000_000_000;
    let fixture = fixture().await?;
    let runtime = fixture.executor.client().refresh_runtime().await?;
    let expected = utility::batch_all(vec![
        expected_transfer(&runtime, DAVE, TAO)?,
        expected_transfer(&runtime, EVE, SECOND)?,
    ])
    .compose(&runtime)?;
    install(
        &fixture,
        expected.clone(),
        false,
        &[(DAVE, 21 * TAO), (EVE, 31 * TAO)],
    )
    .await?;
    let before = fixture.executor.client().at(BlockRef::Best).await?;
    let old = observed_balance(&fixture, &before, DAVE, 21 * TAO).await?;
    let result = execute(&fixture, &batch(SECOND)?).await?;
    let receipt = receipt_snapshot(&fixture, &result, &before, false).await?;
    assert_eq!(
        observed_balance(&fixture, &receipt, DAVE, 21 * TAO).await?,
        old
    );
    observed_balance(&fixture, &before, DAVE, 21 * TAO).await?;
    assert_consumed(&fixture, &expected)?;
    fixture.executor.client().close().await;
    Ok(())
}

#[tokio::test]
async fn finite_author_script_refuses_wrong_call_before_submission_or_state_advance() -> Result<()>
{
    let fixture = fixture().await?;
    let runtime = fixture.executor.client().refresh_runtime().await?;
    install(
        &fixture,
        expected_transfer(&runtime, BOB, 3 * TAO)?,
        true,
        &[(BOB, 14 * TAO)],
    )
    .await?;
    let before = fixture.executor.client().at(BlockRef::Best).await?;
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        fixture
            .executor
            .execute_dyn(&transfer(BOB, 2 * TAO), &fixture.keys, &options()),
    )
    .await?;
    ensure!(
        !result.as_ref().is_ok_and(|outcome| outcome.success),
        "wrong call unexpectedly succeeded"
    );
    {
        let transcript = lock_script(&fixture.script)?;
        assert_eq!(
            (
                transcript.attempts,
                transcript.accepted,
                transcript.steps.len()
            ),
            (1, 0, 1)
        );
        assert_eq!(
            transcript.violations,
            ["submitted call differs from independent expectation"]
        );
    }
    assert_eq!(fixture.keys.acquisitions.load(Ordering::SeqCst), 1);
    fixture.chain.mutate_state(|state| {
        assert!(state.submissions.is_empty());
        assert_eq!(state.block, before.block().number);
    });
    let head = fixture.executor.client().at(BlockRef::Best).await?;
    assert_eq!(head.block().hash, before.block().hash);
    observed_balance(&fixture, &head, BOB, 11 * TAO).await?;
    assert_eq!(
        fixture
            .executor
            .client()
            .nonces()
            .fresh(&fixture.keys.signer.account())
            .await?,
        NONCE
    );
    fixture.executor.client().close().await;
    Ok(())
}

#[tokio::test]
async fn finite_author_script_refuses_exhaustion_instead_of_fake_default_success() -> Result<()> {
    let fixture = fixture().await?;
    let runtime = fixture.executor.client().refresh_runtime().await?;
    let expected = expected_transfer(&runtime, BOB, 3 * TAO)?;
    install(&fixture, expected.clone(), true, &[(BOB, 14 * TAO)]).await?;
    let first = execute(&fixture, &transfer(BOB, 3 * TAO)).await?;
    assert!(first.success);
    assert_consumed(&fixture, &expected)?;
    let before = fixture.executor.client().at(BlockRef::Best).await?;
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        fixture
            .executor
            .execute_dyn(&transfer(BOB, 3 * TAO), &fixture.keys, &options()),
    )
    .await?;
    ensure!(
        !result.as_ref().is_ok_and(|outcome| outcome.success),
        "exhausted transcript unexpectedly succeeded"
    );
    {
        let transcript = lock_script(&fixture.script)?;
        assert_eq!(
            (
                transcript.attempts,
                transcript.accepted,
                transcript.steps.len()
            ),
            (2, 1, 0)
        );
        assert_eq!(transcript.violations, ["author transcript exhausted"]);
    }
    assert_eq!(fixture.keys.acquisitions.load(Ordering::SeqCst), 2);
    fixture.chain.mutate_state(|state| {
        assert_eq!(state.submissions.len(), 1);
        assert_eq!(state.block, before.block().number);
    });
    let head = fixture.executor.client().at(BlockRef::Best).await?;
    assert_eq!(head.block().hash, before.block().hash);
    observed_balance(&fixture, &head, BOB, 14 * TAO).await?;
    assert_eq!(
        fixture
            .executor
            .client()
            .nonces()
            .fresh(&fixture.keys.signer.account())
            .await?,
        10
    );
    fixture.executor.client().close().await;
    Ok(())
}

// upstream e2e.rs::test_key_association. This fixed subnet represents the
// source's already-owned registration context; no registration is simulated.
#[cfg(feature = "association")]
#[tokio::test]
async fn client_hotkey_association_submits_exact_call_then_reads_missing_uid_zero_evm_key()
-> Result<()> {
    use rtao_intents::AssociateHotkey;
    use rtao_primitives::{NetUid, Uid};
    use rtao_reads::reads::m6_accounts::associated_evm_key;
    use rtao_runtime::calls::subtensor_module;

    const NETUID: NetUid = NetUid(1);
    const UID: Uid = Uid(0);
    let fixture = fixture().await?;
    let runtime = fixture.executor.client().refresh_runtime().await?;
    let hotkey = fixture.keys.hotkey_address()?;
    assert_eq!(hotkey, AccountId([2; 32]));
    assert_ne!(hotkey, fixture.keys.signer.account());
    let intent = AssociateHotkey {
        hotkey_ss58: Some(hotkey.into()),
    };
    assert_eq!(intent.spec().name, "associate_hotkey");
    assert_eq!(intent.signer_role_dyn()?, SignerRole::Coldkey);
    let expected = subtensor_module::try_associate_hotkey(hotkey).compose(&runtime)?;
    // Pin the independent expectation to the exact metadata call indices and
    // 32-byte hotkey, rather than building it through the intent under test.
    assert_eq!(expected.as_bytes(), [&[7, 91][..], &hotkey.0].concat());
    install(&fixture, expected.clone(), true, &[]).await?;
    let before = fixture.executor.client().at(BlockRef::Best).await?;
    let entry = runtime
        .core()
        .storage_entry("SubtensorModule", "AssociatedEvmAddress")
        .context("AssociatedEvmAddress metadata")?;
    let key = runtime.core().storage_key(
        entry,
        &[
            CoreValue::Uint(u128::from(NETUID.0)),
            CoreValue::Uint(u128::from(UID.0)),
        ],
    )?;
    // The miss is a supplied observation before submission, inherited by the
    // receipt snapshot. It is not a computed hotkey-association state effect.
    fixture.chain.mutate_state(|state| {
        state.seed_at(
            format!("0x{}", hex::encode(before.block().hash.0)),
            key.clone(),
            None,
        );
    });
    let result = execute(&fixture, &intent).await?;
    let receipt = receipt_snapshot(&fixture, &result, &before, true).await?;
    let view = RecordingView::new(receipt.clone());
    let start = fixture.chain.requests().len();
    let associated = associated_evm_key(&view, NETUID, UID).await?;
    assert_eq!(associated, None);
    let touches = view.touches();
    assert_eq!(touches.len(), 1);
    let touch = touches.first().context("EVM association descriptor read")?;
    assert_eq!(touch.kind, ReadKind::Storage);
    assert_eq!(
        (&*touch.pallet_or_api, &*touch.item),
        ("SubtensorModule", "AssociatedEvmAddress")
    );
    assert_eq!(touch.at, Some(receipt.block().hash));
    assert_eq!(view.block_hashes(), BTreeSet::from([receipt.block().hash]));
    let requests = fixture.chain.requests();
    let reads = requests
        .get(start..)
        .context("EVM association request window")?
        .iter()
        .filter(|request| {
            request.method.starts_with("state_getStorage")
                || request.method == "state_queryStorageAt"
        })
        .collect::<Vec<_>>();
    assert_eq!(reads.len(), 1);
    let read = reads.first().context("EVM association storage request")?;
    assert_eq!(read.method, "state_queryStorageAt");
    assert_eq!(
        read.params,
        json!([
            [format!("0x{}", hex::encode(key))],
            format!("0x{}", hex::encode(receipt.block().hash.0))
        ])
    );
    assert_consumed(&fixture, &expected)?;
    fixture.executor.client().close().await;
    Ok(())
}

// upstream e2e.rs::test_auto_stake_destination discards the submission result.
// The fixed subnet represents already-owned context, without registration.
#[cfg(feature = "staking")]
#[tokio::test]
async fn client_auto_stake_submits_exact_call_then_reads_supplied_coldkey_destination() -> Result<()>
{
    use rtao_intents::SetAutoStake;
    use rtao_primitives::NetUid;
    use rtao_reads::reads::m6_staking::auto_stake;
    use rtao_runtime::calls::subtensor_module;

    const NETUID: NetUid = NetUid(1);
    let fixture = fixture().await?;
    let runtime = fixture.executor.client().refresh_runtime().await?;
    let coldkey = fixture.keys.coldkey_address()?;
    let hotkey = fixture.keys.hotkey_address()?;
    assert_eq!(coldkey, fixture.keys.signer.account());
    assert_eq!(hotkey, AccountId([2; 32]));
    assert_ne!(coldkey, hotkey);
    let intent = SetAutoStake {
        netuid: NETUID,
        hotkey_ss58: Some(hotkey.into()),
    };
    assert_eq!(intent.spec().name, "set_auto_stake");
    assert_eq!(intent.signer_role_dyn()?, SignerRole::Coldkey);
    assert_eq!(intent.touches_netuids_dyn().as_slice(), [NETUID]);
    let expected =
        subtensor_module::set_coldkey_auto_stake_hotkey(NETUID, hotkey).compose(&runtime)?;
    assert_eq!(
        expected.as_bytes(),
        [&[7, 114][..], &NETUID.0.to_le_bytes(), &hotkey.0].concat()
    );
    let entry = runtime
        .core()
        .storage_entry("SubtensorModule", "AutoStakeDestination")
        .context("AutoStakeDestination metadata")?;
    let key = runtime.core().storage_key(
        entry,
        &[
            CoreValue::Bytes(coldkey.0.to_vec()),
            CoreValue::Uint(u128::from(NETUID.0)),
        ],
    )?;
    let encoded = runtime.core().encode_spec(
        &TypeSpec::Id(entry.value_type),
        &CoreValue::Bytes(hotkey.0.to_vec()),
    )?;
    assert_eq!(encoded, hotkey.0);
    let before = fixture.executor.client().at(BlockRef::Best).await?;
    fixture.chain.mutate_state(|state| {
        state.seed_at(
            format!("0x{}", hex::encode(before.block().hash.0)),
            key.clone(),
            None,
        );
    });
    // This value is supplied at the controlled receipt block by the existing
    // transcript. No runtime registration or auto-stake effect is computed.
    install_step(
        &fixture.chain,
        &fixture.script,
        Step {
            expected: expected.clone(),
            account: coldkey,
            nonce: NONCE,
            events: events(true)?,
            post: vec![StorageObservation {
                key: key.clone(),
                encoded,
            }],
        },
    )?;
    // Preserve the source's ignored Result: no execution success is asserted.
    let _ = execute(&fixture, &intent).await;
    let after = fixture.executor.client().at(BlockRef::Best).await?;
    assert_ne!(after.block().hash, before.block().hash);
    assert_eq!(
        after.block().number,
        before
            .block()
            .number
            .checked_add(1)
            .context("receipt height")?
    );
    let view = RecordingView::new(after.clone());
    let start = fixture.chain.requests().len();
    let destination = auto_stake(&view, coldkey, NETUID).await?;
    assert_eq!(destination, Some(hotkey));
    let touches = view.touches();
    assert_eq!(touches.len(), 1);
    let touch = touches.first().context("auto-stake descriptor read")?;
    assert_eq!(touch.kind, ReadKind::Storage);
    assert_eq!(
        (&*touch.pallet_or_api, &*touch.item),
        ("SubtensorModule", "AutoStakeDestination")
    );
    assert_eq!(touch.at, Some(after.block().hash));
    assert_eq!(view.block_hashes(), BTreeSet::from([after.block().hash]));
    let requests = fixture.chain.requests();
    let reads = requests
        .get(start..)
        .context("auto-stake request window")?
        .iter()
        .filter(|request| {
            request.method.starts_with("state_getStorage")
                || request.method == "state_queryStorageAt"
        })
        .collect::<Vec<_>>();
    assert_eq!(reads.len(), 1);
    let read = reads.first().context("auto-stake storage request")?;
    assert_eq!(read.method, "state_queryStorageAt");
    assert_eq!(
        read.params,
        json!([
            [format!("0x{}", hex::encode(key))],
            format!("0x{}", hex::encode(after.block().hash.0))
        ])
    );
    assert_consumed(&fixture, &expected)?;
    fixture.executor.client().close().await;
    Ok(())
}

// pyt: unit/test_intents_table.py::TestExecuteFlow::test_proxy_wraps_call_and_detects_inner_failure
#[tokio::test]
async fn client_proxied_transfer_returns_success_or_the_inner_proxy_dispatch_error() -> Result<()> {
    for inner_success in [true, false] {
        let fixture = fixture().await?;
        let result = async {
            let runtime = fixture.executor.client().refresh_runtime().await?;
            let inner = expected_transfer(&runtime, BOB, TAO)?;
            let expected = rtao_runtime::calls::proxy::proxy(
                DynValue::new(CoreValue::Bytes(BOB.0.to_vec())),
                None,
                inner.clone(),
            )
            .compose(&runtime)?;
            // Pinned Proxy.proxy, MultiAddress::Id(real), no forced proxy type,
            // then the independently composed native transfer bytes.
            let mut wire = vec![16, 0, 0];
            wire.extend_from_slice(&BOB.0);
            wire.push(0);
            wire.extend_from_slice(inner.as_bytes());
            assert_eq!(expected.as_bytes(), wire);
            let failure = MODULE_ERRORS
                .iter()
                .find(|error| error.pallet == "Balances" && error.name == "InsufficientBalance")
                .context("pinned nested Balances error")?;
            assert_eq!((failure.pallet_index, failure.error_index), (5, 2));
            let nested = if inner_success {
                json!({"Ok":null})
            } else {
                json!({"Err":{"Module":{"index":failure.pallet_index,"error":[failure.error_index,0,0,0]}}})
            };
            let mut rows = events(true)?
                .as_array()
                .context("outer success event list")?
                .clone();
            rows.insert(0, json!({"phase":"ApplyExtrinsic","extrinsic_idx":0,"module_id":"Proxy","event_id":"ProxyExecuted","attributes":{"result":nested},"topics":[]}));
            install_step(
                &fixture.chain,
                &fixture.script,
                Step {
                    expected: expected.clone(),
                    account: fixture.keys.signer.account(),
                    nonce: NONCE,
                    events: Value::Array(rows),
                    post: Vec::new(),
                },
            )?;
            let intent = transfer(BOB, TAO);
            let mut opts = options();
            opts.proxy_for = Some(BOB);
            let receipt = tokio::time::timeout(
                Duration::from_secs(10),
                fixture.executor.execute_dyn(&intent, &fixture.keys, &opts),
            )
            .await??;
            assert_eq!(receipt.success, inner_success);
            assert_eq!(receipt.inclusion, Inclusion::Finalized);
            assert!(receipt.fate_unknown.is_none());
            assert_eq!(receipt.events.len(), 2);
            assert!(
                receipt
                    .events
                    .iter()
                    .any(|event| event.matches("System", "ExtrinsicSuccess"))
            );
            assert!(
                receipt
                    .events
                    .iter()
                    .any(|event| event.matches("Proxy", "ProxyExecuted"))
            );
            assert!(
                !receipt
                    .events
                    .iter()
                    .any(|event| event.matches("System", "ExtrinsicFailed"))
            );
            if inner_success {
                assert_eq!(receipt.message, "Success");
                assert!(receipt.error.is_none());
                assert_eq!(receipt.data["proxy_for"], json!(BOB));
            } else {
                // Native four-byte Module errors resolve against the executing
                // block's metadata, yielding named docs rather than mock text.
                let error = receipt.error.as_ref().context("nested proxy failure")?;
                assert_eq!(error.name.as_deref(), Some("InsufficientBalance"));
                assert_eq!(error.code, rtao_catalog::ErrorCode::InsufficientBalance);
                assert_eq!(error.message, failure.docs);
                assert_eq!(receipt.message, failure.docs);
                assert!(receipt.data.is_empty());
            }
            let snapshot = fixture
                .executor
                .client()
                .at(BlockRef::Hash(
                    receipt.block_hash.context("proxy receipt hash")?,
                ))
                .await?;
            assert_eq!(
                receipt.extrinsic_id,
                Some(format!("{}-0000", snapshot.block().number))
            );
            assert_eq!(
                receipt.events.as_slice(),
                snapshot.events().await?.records()
            );
            assert_consumed(&fixture, &expected)?;
            Ok::<(), anyhow::Error>(())
        }.await;
        fixture.executor.client().close().await;
        result?;
    }
    Ok(())
}
