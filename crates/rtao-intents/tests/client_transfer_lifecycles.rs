// Derived from RaoFoundation/subtensor sdk/bittensor-core/tests/e2e.rs @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Client transfer and batch lifecycles over a finite, explicit RPC transcript.
//!
//! Post-state balances below are supplied observations, not calculated effects.
//! These fixtures exercise signing, exact submitted calls, receipt decoding and
//! pinned reads. They do not prove runtime economics, atomic application,
//! rollback, fee calibration or consensus.
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
struct AccountObservation {
    key: Vec<u8>,
    encoded: Vec<u8>,
}

// This uses metadata directly, independently of owner::balance and the typed
// System.Account descriptor whose actual requests are checked below.
fn account_observation(
    runtime: &Runtime,
    account: AccountId,
    free: u64,
) -> Result<AccountObservation> {
    let entry = runtime
        .storage_entry("System", "Account")
        .context("System.Account metadata")?;
    let key = runtime.storage_key(entry, &[CoreValue::Bytes(account.0.to_vec())])?;
    let value = fill_defaults(runtime, entry.value_type, json!({"data":{"free":free}}))?;
    let encoded = runtime.encode_spec(&TypeSpec::Id(entry.value_type), &value)?;
    Ok(AccountObservation { key, encoded })
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
    post: Vec<AccountObservation>,
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
