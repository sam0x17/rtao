// Derived from RaoFoundation/subtensor sdk/bittensor-core/tests/e2e.rs @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Exact TLS-axon and Prometheus submissions over a finite client transcript.
//!
//! Success receipts are supplied observations. Subnet registration/ownership,
//! runtime serving admission, endpoint/certificate persistence, reachability
//! and actual TLS operation remain unverified. No server or node is started.
#![cfg(feature = "serving")]

use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result, ensure};
use rtao_client::{Client, ClientConfig, Inclusion, Network};
use rtao_core::{runtime::TypeSpec, value_ext::ValueExt};
use rtao_intents::{ServeAxonTls, ServePrometheus};
use rtao_primitives::{AccountId, CryptoType, NetUid};
use rtao_runtime::{CallBytes, calls::subtensor_module as calls};
use rtao_signer::{Hotkey, KeySource, KeyView, KeypairSigner, Signer, SignerError, SignerRole};
use rtao_testkit::{
    FakeChain, FakeState, Reply, RpcRequest, SubmitScript, fakes::fill_defaults, rpc::RpcClient,
};
use rtao_tx::{ExecuteOptions, Executor, ExtrinsicResult, Intent};
use serde_json::{Value, json};

// The source registers a subnet dynamically. This fixed ID represents its
// already-owned context; this fixture does not emulate registration/ownership.
const NETUID: NetUid = NetUid(1);
const IP: &str = "203.0.113.5";
const NONCE: u64 = 9;
const STEPS: usize = 2;
const MAX_AUTHOR_HEX_BYTES: usize = 256 * 1024;
const WATCH: &str = "author_submitAndWatchExtrinsic";

struct DevHotkey {
    signer: KeypairSigner,
    acquisitions: AtomicUsize,
    cold_lookups: AtomicUsize,
}
impl KeyView for DevHotkey {
    fn hotkey_address(&self) -> Result<AccountId, SignerError> {
        Ok(self.signer.account())
    }
    fn hotkey_public_key(&self) -> Result<[u8; 32], SignerError> {
        Ok(self.signer.account().0)
    }
    fn coldkey_address(&self) -> Result<AccountId, SignerError> {
        self.cold_lookups.fetch_add(1, Ordering::SeqCst);
        Err(SignerError::KeyUnavailable(SignerRole::Coldkey))
    }
}
impl KeySource<Hotkey> for DevHotkey {
    fn address(&self) -> Result<AccountId, SignerError> {
        self.hotkey_address()
    }
    async fn signer(&self) -> Result<Arc<dyn Signer>, SignerError> {
        self.acquisitions.fetch_add(1, Ordering::SeqCst);
        Ok(Arc::new(self.signer.clone()))
    }
}

#[derive(Clone)]
struct Step {
    expected: CallBytes,
    nonce: u64,
}
#[derive(Clone)]
struct Receipt {
    height: u64,
    hash: String,
    bytes: Vec<u8>,
}
struct Transcript {
    steps: VecDeque<Step>,
    hotkey: AccountId,
    events: Value,
    attempts: usize,
    receipts: Vec<Receipt>,
    violations: Vec<String>,
}
type Script = Arc<Mutex<Transcript>>;

fn lock_script(script: &Script) -> Result<std::sync::MutexGuard<'_, Transcript>> {
    script
        .lock()
        .map_err(|_| anyhow::anyhow!("poisoned serving transcript"))
}

fn guarded_author(state: &mut FakeState, request: &RpcRequest, script: &Script) -> Reply {
    let result = (|| -> Result<Reply> {
        let (step, hotkey, events) = {
            let mut transcript = lock_script(script)?;
            transcript.attempts = transcript.attempts.saturating_add(1);
            ensure!(request.method == WATCH, "unexpected author method");
            (
                transcript
                    .steps
                    .front()
                    .cloned()
                    .context("author transcript exhausted")?,
                transcript.hotkey,
                transcript.events.clone(),
            )
        };
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
        let actual = decoded.field("call").context("submitted call")?;
        // The strict decoder consumes the framed body and its call is last.
        // Its decoded call record is a display shape, not an enum-encoder
        // input. Check the actual trailing call bytes against the independent
        // expectation as well as comparing the fully decoded call.
        ensure!(
            actual == &expected && bytes.ends_with(step.expected.as_bytes()),
            "submitted call differs from independent expectation"
        );
        let account = decoded
            .field("address")
            .and_then(ValueExt::as_str)
            .context("signed author account")?
            .parse::<AccountId>()?;
        ensure!(account == hotkey, "unexpected author account");
        ensure!(
            decoded.field("nonce").and_then(ValueExt::as_u128) == Some(u128::from(step.nonce)),
            "unexpected author nonce"
        );
        let signature = decoded
            .field("signature")
            .and_then(|value| value.variant("Sr25519"))
            .and_then(ValueExt::value_bytes)
            .context("Sr25519 author signature")?;
        ensure!(
            signature.len() == 64 && signature.iter().any(|byte| *byte != 0),
            "missing nonzero development signature"
        );
        // Nothing is recorded, queued or advanced before the full request has
        // passed. Exhaustion and alternate author methods cannot use defaults.
        let previous_height = state.block;
        let previous_count = state.submissions.len();
        state.queue_submit(SubmitScript::InBlockThenFinalized { events });
        // FakeState removes this handler while invoking it. The nested dispatch
        // uses the built-in path; only the outer dispatch records the request.
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
            state.submissions.len() == previous_count.saturating_add(1)
                && state.submissions.last() == Some(&bytes),
            "unexpected submitted bytes or count"
        );
        let hash = state.block_hash(state.block).context("receipt hash")?;
        ensure!(
            reply
                .notifications
                .iter()
                .map(|notification| notification.result.clone())
                .collect::<Vec<_>>()
                == [
                    json!("ready"),
                    json!({"inBlock":hash}),
                    json!({"finalized":hash})
                ],
            "unexpected receipt transcript"
        );
        let mut transcript = lock_script(script)?;
        ensure!(transcript.steps.pop_front().is_some(), "lost serving step");
        transcript.receipts.push(Receipt {
            height: state.block,
            hash,
            bytes,
        });
        Ok(reply)
    })();
    match result {
        Ok(reply) => reply,
        Err(error) => {
            let mut transcript = script
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if transcript.violations.len() <= STEPS {
                transcript.violations.push(error.to_string());
            }
            Reply::error(-32000, "finite serving transcript refused request", None)
        }
    }
}

struct Fixture {
    executor: Executor,
    chain: FakeChain,
    keys: DevHotkey,
    script: Script,
    initial_height: u64,
    expected: [CallBytes; STEPS],
}
async fn fixture() -> Result<Fixture> {
    // The same public-development hot URI as the upstream Alice wallet. No
    // cold signer, wallet file or user recovery material is acquired here.
    let keys = DevHotkey {
        signer: KeypairSigner::from_uri("//Alice//hot", CryptoType::Sr25519)?,
        acquisitions: AtomicUsize::new(0),
        cold_lookups: AtomicUsize::new(0),
    };
    let mut state = FakeState::try_default()?;
    state.set_nonce(keys.signer.account().to_string(), NONCE);
    let initial_height = state.block;
    let chain = FakeChain::from_state(state);
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    let client = Client::from_rpc(RpcClient::new(chain.clone()), config).await?;
    let assembled: Result<_> = async {
        let runtime = client.refresh_runtime().await?;
        // Independently composed source literals, never the intent's own output.
        let expected = [
            calls::serve_axon_tls(NETUID, 1, 3_405_803_781, 8_091, 4, 4, 0, 0, vec![0xab; 32])
                .compose(&runtime)?,
            calls::serve_prometheus(NETUID, 1, 3_405_803_781, 9_090, 4).compose(&runtime)?,
        ];
        let events = json!([{
            "phase":"ApplyExtrinsic", "extrinsic_idx":0, "module_id":"System",
            "event_id":"ExtrinsicSuccess", "attributes":{
                "dispatch_info":{"weight":{"ref_time":1,"proof_size":0},"class":"Normal","pays_fee":"Yes"}
            }, "topics":[]
        }]);
        // Authenticate the receipt encoding before installing either author guard.
        chain.mutate_state(|state| -> Result<()> {
            let entry = state
                .runtime
                .storage_entry("System", "Events")
                .context("System.Events metadata")?;
            let value = fill_defaults(&state.runtime, entry.value_type, events.clone())?;
            state
                .runtime
                .encode_spec(&TypeSpec::Id(entry.value_type), &value)?;
            Ok(())
        })?;
        let script = Arc::new(Mutex::new(Transcript {
            steps: VecDeque::from([
                Step {
                    expected: expected[0].clone(),
                    nonce: NONCE,
                },
                Step {
                    expected: expected[1].clone(),
                    nonce: NONCE + 1,
                },
            ]),
            hotkey: keys.signer.account(),
            events,
            attempts: 0,
            receipts: Vec::new(),
            violations: Vec::new(),
        }));
        chain.mutate_state(|state| {
            for method in [WATCH, "author_submitExtrinsic"] {
                let transcript = script.clone();
                state.set_handler(method, move |state, request| {
                    guarded_author(state, request, &transcript)
                });
            }
        });
        Ok((expected, script))
    }
    .await;
    let (expected, script) = match assembled {
        Ok(value) => value,
        Err(error) => {
            client.close().await;
            return Err(error);
        }
    };
    Ok(Fixture {
        executor: rtao_intents::executor(client),
        chain,
        keys,
        script,
        initial_height,
        expected,
    })
}

fn tls() -> ServeAxonTls {
    ServeAxonTls {
        netuid: NETUID,
        ip: IP.into(),
        port: 8_091,
        version: 1,
        protocol: 4,
        certificate: "ab".repeat(32),
    }
}
fn prometheus() -> ServePrometheus {
    ServePrometheus {
        netuid: NETUID,
        ip: IP.into(),
        port: 9_090,
        version: 1,
    }
}
async fn execute<I: Intent<Role = Hotkey, Output = ()>>(
    fixture: &Fixture,
    intent: &I,
) -> Result<ExtrinsicResult> {
    let options = ExecuteOptions {
        retries: 0,
        replan_owner_rate_limit: false,
        deadline: Some(Duration::from_secs(5)),
        ..Default::default()
    };
    Ok(tokio::time::timeout(
        Duration::from_secs(10),
        fixture.executor.execute(intent, &fixture.keys, &options),
    )
    .await??)
}

fn assert_receipt(fixture: &Fixture, result: &ExtrinsicResult, position: usize) -> Result<()> {
    let transcript = lock_script(&fixture.script)?;
    let receipt = transcript
        .receipts
        .get(position)
        .cloned()
        .with_context(|| {
            format!(
                "accepted serving receipt {position}: attempts={}, violations={:?}, result error={:?}",
                transcript.attempts, transcript.violations, result.error
            )
        })?;
    drop(transcript);
    assert!(result.success);
    assert_eq!(result.inclusion, Inclusion::Finalized);
    assert!(result.error.is_none());
    assert!(result.fate_unknown.is_none());
    assert_eq!(
        result
            .block_hash
            .map(|hash| format!("0x{}", hex::encode(hash.0))),
        Some(receipt.hash)
    );
    assert_eq!(
        receipt.height,
        fixture.initial_height + u64::try_from(position)? + 1
    );
    assert_eq!(
        result.extrinsic_id,
        Some(format!("{}-0000", receipt.height))
    );
    let event = result.events.first().context("success receipt event")?;
    assert_eq!(result.events.len(), 1);
    assert!(event.matches("System", "ExtrinsicSuccess"));
    assert_eq!(event.phase, "ApplyExtrinsic");
    assert_eq!(event.extrinsic_idx, Some(0));
    Ok(())
}

fn assert_consumed(fixture: &Fixture) -> Result<()> {
    let transcript = lock_script(&fixture.script)?;
    assert_eq!(transcript.attempts, STEPS);
    assert_eq!(transcript.receipts.len(), STEPS);
    assert!(transcript.steps.is_empty());
    assert!(transcript.violations.is_empty());
    let receipts = transcript.receipts.clone();
    drop(transcript);
    assert_ne!(receipts[0].hash, receipts[1].hash);
    assert_eq!(fixture.keys.acquisitions.load(Ordering::SeqCst), STEPS);
    assert_eq!(fixture.keys.cold_lookups.load(Ordering::SeqCst), 0);
    fixture.chain.mutate_state(|state| -> Result<()> {
        assert_eq!(state.block, fixture.initial_height + u64::try_from(STEPS)?);
        assert_eq!(state.submissions.len(), STEPS);
        for ((bytes, receipt), expected) in state
            .submissions
            .iter()
            .zip(&receipts)
            .zip(&fixture.expected)
        {
            assert_eq!(bytes, &receipt.bytes);
            let decoded = state.runtime.decode_extrinsic(bytes, true)?;
            let expected = state
                .runtime
                .decode_spec(&TypeSpec::Call, expected.as_bytes(), true)?;
            assert_eq!(decoded.field("call"), Some(&expected));
        }
        Ok(())
    })
}

// upstream e2e.rs::test_serving_endpoints: exact IPv4, ports, fields and bytes.
#[tokio::test]
async fn client_tls_axon_and_prometheus_submit_exact_upstream_calls() -> Result<()> {
    let fixture = fixture().await?;
    let result = async {
        let axon = execute(&fixture, &tls()).await?;
        assert_receipt(&fixture, &axon, 0)?;
        let metrics = execute(&fixture, &prometheus()).await?;
        assert_receipt(&fixture, &metrics, 1)?;
        assert_consumed(&fixture)?;
        assert_eq!(
            fixture
                .executor
                .client()
                .nonces()
                .fresh(&fixture.keys.signer.account())
                .await?,
            NONCE + 2
        );
        Ok(())
    }
    .await;
    fixture.executor.client().close().await;
    result
}

#[tokio::test]
async fn finite_serving_transcript_refuses_changed_certificate_before_submission() -> Result<()> {
    let fixture = fixture().await?;
    let result = async {
        let mut changed = tls();
        changed.certificate = "ac".repeat(32);
        ensure!(
            !execute(&fixture, &changed)
                .await
                .as_ref()
                .is_ok_and(|result| result.success),
            "changed certificate unexpectedly succeeded"
        );
        {
            let transcript = lock_script(&fixture.script)?;
            assert_eq!(
                (
                    transcript.attempts,
                    transcript.receipts.len(),
                    transcript.steps.len()
                ),
                (1, 0, STEPS)
            );
            assert_eq!(
                transcript.violations,
                ["submitted call differs from independent expectation"]
            );
        }
        fixture.chain.mutate_state(|state| {
            assert!(state.submissions.is_empty());
            assert_eq!(state.block, fixture.initial_height);
        });
        assert_eq!(fixture.keys.acquisitions.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.keys.cold_lookups.load(Ordering::SeqCst), 0);
        assert_eq!(
            fixture
                .executor
                .client()
                .nonces()
                .fresh(&fixture.keys.signer.account())
                .await?,
            NONCE
        );
        Ok(())
    }
    .await;
    fixture.executor.client().close().await;
    result
}

#[tokio::test]
async fn finite_serving_transcript_refuses_exhaustion_instead_of_default_success() -> Result<()> {
    let fixture = fixture().await?;
    let result = async {
        let axon = execute(&fixture, &tls()).await?;
        assert_receipt(&fixture, &axon, 0)?;
        let metrics = execute(&fixture, &prometheus()).await?;
        assert_receipt(&fixture, &metrics, 1)?;
        assert_consumed(&fixture)?;
        ensure!(
            !execute(&fixture, &tls())
                .await
                .as_ref()
                .is_ok_and(|result| result.success),
            "exhausted serving transcript unexpectedly succeeded"
        );
        {
            let transcript = lock_script(&fixture.script)?;
            assert_eq!(
                (
                    transcript.attempts,
                    transcript.receipts.len(),
                    transcript.steps.len()
                ),
                (3, STEPS, 0)
            );
            assert_eq!(transcript.violations, ["author transcript exhausted"]);
        }
        fixture.chain.mutate_state(|state| {
            assert_eq!(state.submissions.len(), STEPS);
            assert_eq!(state.block, fixture.initial_height + 2);
        });
        assert_eq!(fixture.keys.acquisitions.load(Ordering::SeqCst), 3);
        assert_eq!(fixture.keys.cold_lookups.load(Ordering::SeqCst), 0);
        assert_eq!(
            fixture
                .executor
                .client()
                .nonces()
                .fresh(&fixture.keys.signer.account())
                .await?,
            NONCE + 2
        );
        Ok(())
    }
    .await;
    fixture.executor.client().close().await;
    result
}
