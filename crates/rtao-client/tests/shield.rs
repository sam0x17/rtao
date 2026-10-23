// Derived from RaoFoundation/subtensor sdk/python/tests/unit/test_intents_table.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/executor.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Signed shield carriers, paired nonce accounting and canonical inner receipts.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use bytes::Bytes;
use rtao_client::{
    Client, ClientConfig, ClientError, Inclusion, Mortality, Network, NoncePolicy, Period,
    ShieldCiphertext, ShieldOptions, ShieldPeriod, SubmitFailure, SubmitOptions, Wait,
};
use rtao_core::{codec::Value, value_ext::ValueExt};
use rtao_primitives::{Balance, H256, Tao};
use rtao_signer::{
    AccountId, BoxFuture, CryptoType, KeypairSigner, MultiSignature, SignPurpose, Signer,
    SignerCaps, SignerError, SigningRequest,
};
use rtao_testkit::{FakeChain, FakeState, Reply, SubmitScript};
use serde_json::json;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use subxt_rpcs::RpcClient;

#[derive(Clone, Debug)]
struct SignedView {
    purpose: SignPurpose,
    call: Vec<u8>,
    payload: Vec<u8>,
    extra: Vec<u8>,
    additional: Vec<u8>,
    json: rtao_signer::SignerPayloadJson,
}
struct RecordingSigner {
    key: KeypairSigner,
    signed: Arc<Mutex<Vec<SignedView>>>,
    delay: Duration,
    after_inner: Option<FakeChain>,
}
impl Signer for RecordingSigner {
    fn account(&self) -> AccountId {
        self.key.account()
    }
    fn crypto_type(&self) -> CryptoType {
        self.key.crypto_type()
    }
    fn caps(&self) -> SignerCaps {
        self.key.caps()
    }
    fn sign<'a>(
        &'a self,
        request: &'a SigningRequest<'a>,
    ) -> BoxFuture<'a, Result<MultiSignature, SignerError>> {
        Box::pin(async move {
            self.signed.lock().unwrap().push(SignedView {
                purpose: request.purpose,
                call: request.call.to_vec(),
                payload: request.payload.to_vec(),
                extra: request.included_in_extrinsic.to_vec(),
                additional: request.included_in_signed_data.to_vec(),
                json: request.payload_json.clone(),
            });
            if !self.delay.is_zero() {
                tokio::time::sleep(self.delay).await;
            }
            if request.purpose == SignPurpose::ShieldInner
                && let Some(fake) = &self.after_inner
            {
                fake.mutate_state(|state| {
                    state.advance(1);
                });
            }
            self.key.sign(request).await
        })
    }
}

async fn fixture() -> (Client, FakeChain, RecordingSigner) {
    let mut state = FakeState::default();
    state
        .seed_constant("Aura", "SlotDuration", json!(1))
        .unwrap();
    // An encoded 1184-byte ML-KEM-768 encapsulation key accepted by the core.
    state
        .seed_typed("MevShield", "NextKey", (), vec![0u8; 1184])
        .unwrap();
    let fake = FakeChain::from_state(state);
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None).unwrap()).unwrap();
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    let client = Client::from_rpc(RpcClient::new(fake.clone()), config)
        .await
        .unwrap();
    let signer = RecordingSigner {
        key: KeypairSigner::from_seed(&[7; 32], CryptoType::Ed25519).unwrap(),
        signed: Arc::new(Mutex::new(Vec::new())),
        delay: Duration::ZERO,
        after_inner: None,
    };
    (client, fake, signer)
}
async fn call(client: &Client, length: usize) -> rtao_client::CallBytes {
    rtao_runtime::calls::system::remark(vec![42; length])
        .compose(&client.refresh_runtime().await.unwrap())
        .unwrap()
}
fn pool() -> ShieldOptions {
    ShieldOptions {
        wait: Wait::Pool,
        ..Default::default()
    }
}
fn success(index: u32) -> serde_json::Value {
    json!({"phase":"ApplyExtrinsic","extrinsic_idx":index,"module_id":"System","event_id":"ExtrinsicSuccess","attributes":{"dispatch_info":{"weight":{"ref_time":1,"proof_size":0},"class":"Normal","pays_fee":"Yes"}},"topics":[]})
}
async fn signed_inner(
    client: &Client,
    signer: &RecordingSigner,
    call: rtao_client::CallBytes,
    nonce: u64,
) -> (Bytes, H256) {
    let opts = SubmitOptions {
        nonce: NoncePolicy::Explicit(nonce),
        mortality: Mortality::Mortal(Period::new(8)),
        ..Default::default()
    };
    let prepared = client.prepare_call(call, &signer.key, &opts).await.unwrap();
    let signature = signer
        .key
        .sign(&prepared.signing_request(SignPurpose::ShieldInner))
        .await
        .unwrap();
    client.attach_signature(&prepared, signature).await.unwrap()
}
// The fake author exposes the exact signed inner in its decrypted block, while
// author requests themselves record only the encrypted carrier bytes.
fn install_inner(fake: &FakeChain, bytes: Bytes, included: u64, failed: bool) {
    fake.mutate_state(|state| {
        state.set_handler("chain_getBlock", move |state, request| {
            let mut reply = state.dispatch(request);
            if let Some(Ok(raw)) = &mut reply.outcome {
                let number = raw["block"]["header"]["number"]
                    .as_str()
                    .and_then(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).ok());
                if number == Some(included) {
                    let extrinsics = raw["block"]["extrinsics"].as_array_mut().unwrap();
                    let index = u32::try_from(extrinsics.len()).unwrap();
                    extrinsics.push(json!(format!("0x{}", hex::encode(&bytes))));
                    let inner_event = if failed {
                        json!({"phase":"ApplyExtrinsic","extrinsic_idx":index,"module_id":"System","event_id":"ExtrinsicFailed","attributes":{"dispatch_error":{"Token":"FundsUnavailable"},"dispatch_info":{"weight":{"ref_time":1,"proof_size":0},"class":"Normal","pays_fee":"Yes"}},"topics":[]})
                    } else {
                        success(index)
                    };
                    let mut events: Vec<_> = (0..index).map(success).collect();
                    events.push(inner_event);
                    state
                        .seed_events(request.params[0].as_str().unwrap(), json!(events))
                        .unwrap();
                }
            }
            reply
        });
    });
}
fn assert_unknown(
    result: Result<rtao_client::ExtrinsicReport, SubmitFailure>,
    fake: &FakeChain,
) -> SubmitFailure {
    use blake2::{Blake2b, Digest, digest::consts::U32};
    let state = fake.state();
    let state = state.lock().unwrap();
    assert_eq!(state.submissions.len(), 1);
    let expected = H256(Blake2b::<U32>::digest(&state.submissions[0]).into());
    let error = result.unwrap_err();
    assert_eq!(error.unknown_fate_hash(), Some(expected));
    drop(state);
    assert_eq!(
        fake.requests()
            .iter()
            .filter(|request| request.method.starts_with("author_submit"))
            .count(),
        1
    );
    error
}

#[test]
fn ciphertext_and_period_bounds_are_closed() {
    assert_eq!(ShieldPeriod::default(), ShieldPeriod::P8);
    assert_eq!(
        ShieldPeriod::P8.get(),
        rtao_catalog::rt::MAX_SHIELD_ERA_PERIOD
    );
    assert!(ShieldPeriod::try_from(rtao_catalog::rt::MAX_SHIELD_ERA_PERIOD + 1).is_err());
    for period in [0, 1, 2, 3, 5, 7, 9, 128, u64::MAX] {
        assert!(ShieldPeriod::try_from(period).is_err());
    }
    assert_eq!(ShieldPeriod::try_from(4).unwrap(), ShieldPeriod::P4);
    assert_eq!(ShieldPeriod::try_from(8).unwrap(), ShieldPeriod::P8);
    let bound = usize::try_from(rtao_catalog::py::MAX_SHIELDED_CIPHERTEXT_BYTES).unwrap();
    assert!(ShieldCiphertext::new(vec![0; bound]).is_ok());
    assert!(ShieldCiphertext::new(vec![0; bound + 1]).is_err());
}

#[tokio::test]
async fn pool_transmits_only_signed_carrier_and_preserves_pair_nonce_and_anchor() {
    let (client, fake, mut signer) = fixture().await;
    fake.mutate_state(|state| {
        state.set_nonce(signer.account().to_string(), 7);
        state.finalized = 98;
    });
    signer.after_inner = Some(fake.clone());
    let inner_call = call(&client, 3).await;
    let report = client
        .submit_shielded(inner_call.clone(), &signer, &pool())
        .await
        .unwrap();
    assert_eq!(report.inclusion, Inclusion::PoolAccepted);
    assert_eq!(report.shield.unwrap().carrier_hash, report.hash);
    let signed = signer.signed.lock().unwrap().clone();
    assert_eq!(signed.len(), 2);
    assert_eq!(signed[0].purpose, SignPurpose::ShieldInner);
    assert_eq!(signed[1].purpose, SignPurpose::ShieldCarrier);
    assert_eq!(signed[0].call, inner_call.as_bytes());
    assert_eq!(signed[0].json["nonce"], json!("0x00000008"));
    assert_eq!(signed[1].json["nonce"], json!("0x00000007"));
    assert_eq!(signed[0].json["era"], signed[1].json["era"]);
    assert_eq!(signed[0].json["blockHash"], signed[1].json["blockHash"]);
    assert_eq!(signed[0].extra[0..2], signed[1].extra[0..2]);
    assert_eq!(signed[0].additional, signed[1].additional);
    assert!(!signed[0].payload.is_empty());
    {
        let state = fake.state();
        let state = state.lock().unwrap();
        assert_eq!(state.submissions.len(), 1);
        let decoded = state
            .runtime
            .decode_extrinsic(&state.submissions[0], true)
            .unwrap();
        let encoded = rtao_runtime::calls::mev_shield::SUBMIT_ENCRYPTED;
        assert_eq!(
            &signed[1].call[0..2],
            &[encoded.pallet_index, encoded.call_index]
        );
        assert_eq!(decoded.field("nonce").and_then(ValueExt::as_u128), Some(7));
        let ciphertext = signed[1].call[2..].to_vec();
        assert!(
            !ciphertext
                .windows(inner_call.as_bytes().len())
                .any(|w| w == inner_call.as_bytes())
        );
    }
    let next = client.nonces().lease(&signer.account()).await.unwrap();
    assert_eq!(next.value(), 9);
    next.settle();
}

#[tokio::test]
async fn conservative_carrier_probe_is_public_bounded_and_does_not_mutate_nonce() {
    let (client, fake, signer) = fixture().await;
    client.nonces().pin(&signer.account(), 44);
    let fee = client
        .estimate_shielded_carrier_fee(signer.account())
        .await
        .unwrap();
    assert_eq!(fee.rao(), fake.state().lock().unwrap().fee);
    assert_eq!(
        client.nonces().cached(&signer.account().to_string()).await,
        Some(44)
    );
    assert!(signer.signed.lock().unwrap().is_empty());
    assert!(fake.state().lock().unwrap().submissions.is_empty());
    let request = fake
        .requests()
        .into_iter()
        .find(|r| r.method == "state_call" && r.params[0] == "TransactionPaymentApi_query_info")
        .unwrap();
    let args = hex::decode(request.params[1].as_str().unwrap().trim_start_matches("0x")).unwrap();
    let probe = &args[..args.len() - 4];
    let state = fake.state();
    let state = state.lock().unwrap();
    let decoded = state.runtime.decode_extrinsic(probe, true).unwrap();
    let call = decoded.field("call").unwrap();
    let Value::List(args) = call.field("call_args").unwrap() else {
        panic!("call args are not a list");
    };
    let ciphertext = args[0].field("value").unwrap();
    let length = match ciphertext {
        Value::Bytes(bytes) => bytes.len(),
        Value::List(bytes) => bytes.len(),
        Value::Str(bytes) => bytes.len(),
        other => panic!("unexpected ciphertext {other:?}"),
    };
    assert_eq!(length, 8192);
}

#[tokio::test]
async fn finality_lag_waits_for_finality_to_come_within_the_era() {
    let (client, fake, signer) = fixture().await;
    fake.mutate_state(|s| s.finalized = s.block - 6);
    let catching_up = fake.clone();
    let finality = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(800)).await;
        catching_up.mutate_state(|s| s.finalized = s.block - 1);
    });
    let result = client
        .submit_shielded(call(&client, 1).await, &signer, &pool())
        .await;
    finality.await.unwrap();
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(fake.state().lock().unwrap().submissions.len(), 1);
}

#[tokio::test]
async fn finality_lag_refuses_exact_expiry_boundary_before_signing() {
    for (period, allowed, refused) in [(ShieldPeriod::P4, 1, 2), (ShieldPeriod::P8, 5, 6)] {
        for lag in [allowed, refused, refused + 1] {
            let (client, fake, signer) = fixture().await;
            fake.mutate_state(|s| s.finalized = s.block - lag);
            let opts = ShieldOptions {
                period,
                finality_wait: std::time::Duration::ZERO,
                ..pool()
            };
            let result = client
                .submit_shielded(call(&client, 1).await, &signer, &opts)
                .await;
            if lag == allowed {
                assert!(result.is_ok());
            } else {
                assert!(
                    matches!(result,Err(SubmitFailure::ShieldEraExpired {period:p,finality_lag:l}) if p==period.get() && l==lag)
                );
                assert!(signer.signed.lock().unwrap().is_empty());
                assert!(fake.state().lock().unwrap().submissions.is_empty());
                assert!(
                    !fake
                        .requests()
                        .iter()
                        .any(|r| r.method == "account_nextIndex")
                );
            }
        }
    }
}

#[tokio::test]
async fn oversized_inner_ciphertext_is_rejected_before_carrier_signature() {
    let (client, fake, signer) = fixture().await;
    let result = client
        .submit_shielded(call(&client, 8192).await, &signer, &pool())
        .await;
    assert!(
        matches!(result,Err(SubmitFailure::Client(ClientError::Protocol(ref message))) if message.contains("shield ciphertext"))
    );
    assert_eq!(signer.signed.lock().unwrap().len(), 1);
    assert!(fake.state().lock().unwrap().submissions.is_empty());
    assert_eq!(
        client.nonces().cached(&signer.account().to_string()).await,
        None
    );
}

#[tokio::test]
async fn fee_limit_and_probe_failure_fail_before_signing() {
    for failed_probe in [false, true] {
        let (client, fake, signer) = fixture().await;
        if failed_probe {
            fake.mutate_state(|s| {
                s.queue_reply("state_call", Reply::error(-32603, "fee unavailable", None))
            });
        }
        let opts = ShieldOptions {
            max_fee: Some(Balance::<Tao>::ZERO),
            ..pool()
        };
        assert!(
            client
                .submit_shielded(call(&client, 1).await, &signer, &opts)
                .await
                .is_err()
        );
        assert!(signer.signed.lock().unwrap().is_empty());
        assert!(fake.state().lock().unwrap().submissions.is_empty());
    }
}

#[tokio::test]
async fn absent_next_key_is_retried_twice_one_block_apart_then_refused() {
    let (client, fake, signer) = fixture().await;
    let key_prefix = rtao_core::mlkem::twox_128(b"MevShield");
    fake.mutate_state(|state| {
        state.set_handler("state_getStorage", move |state, request| {
            let key = request.params[0].as_str().unwrap();
            if hex::decode(key.trim_start_matches("0x"))
                .unwrap()
                .starts_with(&key_prefix)
            {
                Reply::result(serde_json::Value::Null)
            } else {
                state.dispatch(request)
            }
        });
    });
    let started = tokio::time::Instant::now();
    assert!(
        matches!(client.submit_shielded(call(&client,1).await,&signer,&pool()).await,Err(SubmitFailure::Client(ClientError::Protocol(ref msg))) if msg.contains("NextKey not available"))
    );
    // Each attempt is one unpinned read of the endpoint's newest state.
    let mut next_key = rtao_core::mlkem::twox_128(b"MevShield").to_vec();
    next_key.extend_from_slice(&rtao_core::mlkem::twox_128(b"NextKey"));
    let next_key = format!("0x{}", hex::encode(next_key));
    let reads = fake
        .requests()
        .iter()
        .filter(|r| r.method.starts_with("state_") && r.params.to_string().contains(&next_key))
        .inspect(|r| {
            assert_eq!(r.method, "state_getStorage", "{r:?}");
            assert_eq!(r.params, json!([next_key]), "{r:?}");
        })
        .count();
    assert_eq!(reads, 3);
    assert!(started.elapsed() >= Duration::from_millis(2));
    assert!(signer.signed.lock().unwrap().is_empty());
}

#[tokio::test]
async fn carrier_rejection_never_retries_and_drops_both_leases() {
    let (client, fake, signer) = fixture().await;
    fake.mutate_state(|state| state.queue_submit(SubmitScript::Invalid("Stale".into())));
    let result = client
        .submit_shielded(call(&client, 1).await, &signer, &ShieldOptions::default())
        .await;
    assert!(matches!(result, Err(SubmitFailure::Pool(_))));
    assert_eq!(fake.state().lock().unwrap().submissions.len(), 1);
    assert_eq!(
        client.nonces().cached(&signer.account().to_string()).await,
        None
    );
}

#[tokio::test]
async fn inner_success_and_dispatch_failure_resolve_actual_signed_inner() {
    for failed in [false, true] {
        let (client, fake, signer) = fixture().await;
        let inner_call = call(&client, 3).await;
        let (inner_bytes, inner_hash) = signed_inner(&client, &signer, inner_call.clone(), 1).await;
        install_inner(&fake, inner_bytes, 101, failed);
        let report = client
            .submit_shielded(inner_call, &signer, &ShieldOptions::default())
            .await
            .unwrap();
        assert_eq!(report.hash, inner_hash);
        assert_eq!(report.shield.unwrap().inner_hash, inner_hash);
        assert_eq!(report.block_number, Some(101));
        assert_eq!(report.extrinsic_index, Some(1));
        assert_eq!(report.success, !failed);
        assert_eq!(report.error.is_some(), failed);
        assert_eq!(fake.state().lock().unwrap().submissions.len(), 1);
        let next = client.nonces().lease(&signer.account()).await.unwrap();
        assert_eq!(next.value(), 2);
        next.settle();
    }
}

#[tokio::test]
async fn stalled_chain_keeps_carrier_unresolved_and_inner_can_still_execute_later() {
    let (client, fake, signer) = fixture().await;
    let inner_call = call(&client, 1).await;
    let (inner_bytes, inner_hash) = signed_inner(&client, &signer, inner_call.clone(), 1).await;
    let result = client
        .submit_shielded(inner_call, &signer, &ShieldOptions::default())
        .await;
    let error = assert_unknown(result, &fake);
    assert!(
        matches!(error, SubmitFailure::Unresolved { ref cause, reconciliation: None, .. }
        if matches!(cause.as_ref(), SubmitFailure::Client(ClientError::Protocol(message))
            if message.contains("block 102 is unavailable after 36 polls")
                && message.contains("expiry and absence have not been established")))
    );
    assert_eq!(fake.state().lock().unwrap().block, 101);
    assert_eq!(
        client.nonces().cached(&signer.account().to_string()).await,
        None
    );
    let attempts = fake
        .requests()
        .iter()
        .filter(|r| r.method == "chain_getBlockHash" && r.params[0] == 102)
        .count();
    assert_eq!(
        attempts,
        usize::try_from(rtao_catalog::py::SHIELDED_POLL_ATTEMPTS_PER_BLOCK * 9).unwrap()
    );

    // Null replies exhausted a read budget, not the signed era. Once the
    // stalled chain resumes, the same signed inner can still execute.
    fake.mutate_state(|state| {
        state.advance(1);
    });
    install_inner(&fake, inner_bytes, 102, false);
    let block = client
        .at(rtao_client::BlockRef::Number(102))
        .await
        .unwrap()
        .block()
        .hash;
    let recovered = client
        .resolve_extrinsic(inner_hash, block, true)
        .await
        .unwrap();
    assert!(recovered.success);
    assert_eq!(recovered.hash, inner_hash);
    assert_eq!(recovered.block_number, Some(102));
    assert_eq!(recovered.extrinsic_index, Some(0));
    assert_eq!(fake.state().lock().unwrap().submissions.len(), 1);
}

#[tokio::test]
async fn inner_block_lookup_failures_keep_the_carrier_hash_and_original_cause() {
    for malformed in [false, true] {
        let (client, fake, signer) = fixture().await;
        let inner_call = call(&client, 1).await;
        let (inner_bytes, inner_hash) = signed_inner(&client, &signer, inner_call.clone(), 1).await;
        install_inner(&fake, inner_bytes, 101, false);
        let failures = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed = failures.clone();
        fake.mutate_state(|state| {
            state.set_handler("chain_getBlockHash", move |state, request| {
                if !state.submissions.is_empty()
                    && request.params[0] == 101
                    && observed.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0
                {
                    if malformed {
                        Reply::result(json!("0x01"))
                    } else {
                        Reply::error(-32603, "inner block-hash RPC sentinel", None)
                    }
                } else {
                    state.dispatch(request)
                }
            });
        });
        let error = assert_unknown(
            client
                .submit_shielded(inner_call, &signer, &ShieldOptions::default())
                .await,
            &fake,
        );
        let SubmitFailure::Unresolved {
            cause,
            reconciliation: None,
            ..
        } = error
        else {
            panic!("inner lookup failure lost its original cause");
        };
        if malformed {
            assert!(
                matches!(cause.as_ref(), SubmitFailure::Client(ClientError::Protocol(message))
                if message == "RPC hash must contain 32 bytes")
            );
        } else {
            assert!(matches!(
                cause.as_ref(),
                SubmitFailure::Client(ClientError::Rpc(_))
            ));
            assert!(cause.to_string().contains("inner block-hash RPC sentinel"));
        }
        assert_eq!(failures.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(
            client.nonces().cached(&signer.account().to_string()).await,
            None
        );

        // The carrier and inner were actually included; read recovery must
        // recover the receipt without submitting either transaction again.
        let block = client
            .at(rtao_client::BlockRef::Number(101))
            .await
            .unwrap()
            .block()
            .hash;
        let recovered = client
            .resolve_extrinsic(inner_hash, block, true)
            .await
            .unwrap();
        assert!(recovered.success);
        assert_eq!(recovered.hash, inner_hash);
        assert_eq!(recovered.extrinsic_index, Some(1));
        assert_eq!(fake.state().lock().unwrap().submissions.len(), 1);
    }
}

#[tokio::test]
async fn ordinary_inner_noninclusion_requires_finalized_expiry_and_complete_canonical_scan() {
    let once = ShieldOptions {
        dropped_inner_retries: 0,
        ..Default::default()
    };
    for finalized in [false, true] {
        let (client, fake, signer) = fixture().await;
        fake.mutate_state(|state| {
            state.set_handler("chain_getBlockHash", move |state, request| {
                if !state.submissions.is_empty()
                    && let Some(height) = request.params[0].as_u64()
                    && height > state.block
                    && height <= 108
                {
                    state.advance(height - state.block);
                    if !finalized {
                        state.set_finalized(100).unwrap();
                    }
                }
                state.dispatch(request)
            });
        });
        let result = client
            .submit_shielded(call(&client, 1).await, &signer, &once)
            .await;
        if finalized {
            let report = result.unwrap();
            assert!(!report.success);
            assert!(
                report
                    .message
                    .contains("not included before its era expired")
            );
            assert_eq!(report.inclusion, Inclusion::InBlock);
            let requests = fake.requests();
            let finality_check = requests
                .iter()
                .rposition(|request| request.method == "chain_getFinalizedHead")
                .unwrap();
            let mut scanned: Vec<_> = requests[finality_check + 1..]
                .iter()
                .filter(|request| request.method == "chain_getBlockHash")
                .filter_map(|request| request.params[0].as_u64())
                .collect();
            scanned.sort_unstable();
            scanned.dedup();
            assert_eq!(scanned, (100..=108).collect::<Vec<_>>());
        } else {
            let error = assert_unknown(result, &fake);
            assert!(matches!(error, SubmitFailure::Unresolved { ref cause, .. }
                if cause.to_string().contains("block 108 did not finalize after 36 polls")));
        }
        assert_eq!(fake.state().lock().unwrap().block, 108);
        assert_eq!(fake.state().lock().unwrap().submissions.len(), 1);
        assert_eq!(
            client.nonces().cached(&signer.account().to_string()).await,
            None
        );
    }
}

#[tokio::test]
async fn incomplete_finalized_inner_body_never_authorizes_resealing() {
    for body in [json!({"block":{}}), json!({"block":{"extrinsics":null}})] {
        let (client, fake, signer) = fixture().await;
        fake.mutate_state(|state| {
            state.set_handler("chain_getBlockHash", |state, request| {
                if !state.submissions.is_empty()
                    && let Some(height) = request.params[0].as_u64()
                    && height > state.block
                    && height <= 108
                {
                    state.advance(height - state.block);
                }
                state.dispatch(request)
            });
            state.set_handler("chain_getBlock", move |state, request| {
                if state.block >= 108
                    && request.params[0].as_str() == state.block_hash(100).as_deref()
                {
                    Reply::result(body.clone())
                } else {
                    state.dispatch(request)
                }
            });
        });
        let options = ShieldOptions {
            dropped_inner_retries: 1,
            submit: SubmitOptions {
                deadline: Some(Duration::from_secs(2)),
                ..Default::default()
            },
            ..Default::default()
        };
        let error = assert_unknown(
            client
                .submit_shielded(call(&client, 1).await, &signer, &options)
                .await,
            &fake,
        );
        assert!(error.to_string().contains("complete extrinsics array"));
        assert_eq!(fake.state().lock().unwrap().submissions.len(), 1);
        client.close().await;
    }
}

#[tokio::test]
async fn proven_dropped_inner_is_resealed_with_a_fresh_carrier() {
    let (client, fake, signer) = fixture().await;
    fake.mutate_state(|state| {
        state.set_handler("chain_getBlockHash", move |state, request| {
            if !state.submissions.is_empty()
                && let Some(height) = request.params[0].as_u64()
                && height > state.block
            {
                state.advance(height - state.block);
            }
            state.dispatch(request)
        });
    });
    let opts = ShieldOptions {
        dropped_inner_retries: 1,
        ..Default::default()
    };
    let report = client
        .submit_shielded(call(&client, 1).await, &signer, &opts)
        .await
        .unwrap();
    // Both carriers were included and both inners proven absent; the call
    // never executed, so exactly one retry re-sealed it.
    assert!(!report.success);
    assert!(
        report
            .message
            .contains("not included before its era expired")
    );
    let submissions = fake.state().lock().unwrap().submissions.clone();
    assert_eq!(submissions.len(), 2);
    assert_ne!(submissions[0], submissions[1]);
}

#[tokio::test]
async fn finalized_absence_rescan_finds_an_inner_missed_on_the_orphan_branch() {
    let (client, fake, signer) = fixture().await;
    let inner_call = call(&client, 1).await;
    let (inner_bytes, inner_hash) = signed_inner(&client, &signer, inner_call.clone(), 1).await;
    let canonical_inner = Arc::new(Mutex::new(None::<String>));
    let before_finality = canonical_inner.clone();
    fake.mutate_state(|state| {
        state.set_handler("chain_getBlockHash", move |state, request| {
            if !state.submissions.is_empty() && before_finality.lock().unwrap().is_none() {
                // The first scan sees an unfinalized branch without the inner.
                state.set_finalized(100).unwrap();
                if let Some(height) = request.params[0].as_u64()
                    && height > state.block
                    && height <= 108
                {
                    state.advance(height - state.block);
                    state.set_finalized(100).unwrap();
                }
            }
            state.dispatch(request)
        });
    });
    let finality_branch = canonical_inner.clone();
    fake.mutate_state(|state| {
        state.set_handler("chain_getFinalizedHead", move |state, request| {
            if !state.submissions.is_empty()
                && state.block == 108
                && finality_branch.lock().unwrap().is_none()
            {
                let parent = state.block_hash(100).unwrap();
                let first = state.fork(&parent).unwrap();
                let mut tip = first.clone();
                for _ in 102..=108 {
                    tip = state.fork(&tip).unwrap();
                }
                state.set_best(&tip).unwrap();
                state.set_finalized(108).unwrap();
                *finality_branch.lock().unwrap() = Some(first);
            }
            state.dispatch(request)
        });
    });
    let receipt_branch = canonical_inner.clone();
    fake.mutate_state(|state| {
        state.set_handler("chain_getBlock", move |state, request| {
            let mut reply = state.dispatch(request);
            if receipt_branch.lock().unwrap().as_deref() == request.params[0].as_str()
                && let Some(Ok(raw)) = &mut reply.outcome
            {
                let extrinsics = raw["block"]["extrinsics"].as_array_mut().unwrap();
                extrinsics.push(json!(format!("0x{}", hex::encode(&state.submissions[0]))));
                extrinsics.push(json!(format!("0x{}", hex::encode(&inner_bytes))));
                state
                    .seed_events(
                        request.params[0].as_str().unwrap(),
                        json!([success(0), success(1)]),
                    )
                    .unwrap();
            }
            reply
        });
    });
    let report = client
        .submit_shielded(inner_call, &signer, &ShieldOptions::default())
        .await
        .unwrap();
    assert!(report.success);
    assert_eq!(report.hash, inner_hash);
    assert_eq!(report.block_number, Some(101));
    assert_eq!(report.inclusion, Inclusion::Finalized);
    assert_eq!(report.shield.unwrap().inner_hash, inner_hash);
    let canonical = canonical_inner.lock().unwrap().clone().unwrap();
    assert_eq!(
        format!("0x{}", hex::encode(report.block_hash.unwrap().0)),
        canonical
    );
    assert_eq!(fake.state().lock().unwrap().submissions.len(), 1);
}

#[tokio::test]
async fn missing_height_uses_shared_budget_until_delayed_inner_appears() {
    let (client, fake, signer) = fixture().await;
    let inner_call = call(&client, 3).await;
    let (inner_bytes, inner_hash) = signed_inner(&client, &signer, inner_call.clone(), 1).await;
    install_inner(&fake, inner_bytes, 102, false);
    let misses = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observed = misses.clone();
    fake.mutate_state(|state| {
        state.set_handler("chain_getBlockHash", move |state, request| {
            if request.params[0] == 102 {
                if observed.fetch_add(1, std::sync::atomic::Ordering::SeqCst) < 5 {
                    return Reply::result(serde_json::Value::Null);
                }
                state.advance(1);
            }
            state.dispatch(request)
        })
    });
    let report = client
        .submit_shielded(inner_call, &signer, &ShieldOptions::default())
        .await
        .unwrap();
    assert!(report.success);
    assert_eq!(report.hash, inner_hash);
    assert_eq!(report.block_number, Some(102));
    assert_eq!(misses.load(std::sync::atomic::Ordering::SeqCst), 6);
    assert_eq!(fake.state().lock().unwrap().submissions.len(), 1);
    let next = client.nonces().lease(&signer.account()).await.unwrap();
    assert_eq!(next.value(), 2);
    next.settle();
}

#[tokio::test]
async fn admission_timeout_after_synchronous_stage_does_not_broadcast() {
    let (client, fake, mut signer) = fixture().await;
    signer.delay = Duration::from_millis(50);
    let opts = ShieldOptions {
        submit: SubmitOptions {
            submit_deadline: Some(Duration::from_millis(10)),
            ..Default::default()
        },
        ..pool()
    };
    assert!(matches!(
        client
            .submit_shielded(call(&client, 1).await, &signer, &opts)
            .await,
        Err(SubmitFailure::Client(ClientError::Rpc(
            rtao_rpc::RpcError::Timeout { .. }
        )))
    ));
    assert!(fake.state().lock().unwrap().submissions.is_empty());
    assert_eq!(
        client.nonces().cached(&signer.account().to_string()).await,
        None
    );
}

#[tokio::test]
async fn watch_timeout_keeps_exact_attempted_carrier_hash_without_retransmission() {
    let (client, fake, signer) = fixture().await;
    fake.mutate_state(|state| state.queue_submit(SubmitScript::Statuses(vec![json!("ready")])));
    let opts = ShieldOptions {
        submit: SubmitOptions {
            deadline: Some(Duration::from_secs(2)),
            ..Default::default()
        },
        ..Default::default()
    };
    let result = client
        .submit_shielded(call(&client, 1).await, &signer, &opts)
        .await;
    assert_unknown(result, &fake);
    assert_eq!(
        client.nonces().cached(&signer.account().to_string()).await,
        None
    );
    assert!(
        fake.requests()
            .iter()
            .any(|r| r.method == "author_unwatchExtrinsic")
    );
}

#[tokio::test]
async fn cancellation_of_waiting_shield_drops_pair_and_unwatches_carrier() {
    let (client, fake, signer) = fixture().await;
    fake.mutate_state(|state| state.queue_submit(SubmitScript::Statuses(vec![json!("ready")])));
    let inner_call = call(&client, 1).await;
    let account = signer.account();
    let task_client = client.clone();
    let task = tokio::spawn(async move {
        task_client
            .submit_shielded(inner_call, &signer, &ShieldOptions::default())
            .await
    });
    for _ in 0..100 {
        if !fake.state().lock().unwrap().submissions.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    assert_eq!(fake.state().lock().unwrap().submissions.len(), 1);
    task.abort();
    let _ = task.await;
    assert_eq!(client.nonces().cached(&account.to_string()).await, None);
    assert!(
        fake.requests()
            .iter()
            .any(|r| r.method == "author_unwatchExtrinsic")
    );
}

#[tokio::test]
async fn finality_rpc_failure_retries_four_then_refuses_without_resubmitting() {
    let (client, fake, signer) = fixture().await;
    let inner_call = call(&client, 1).await;
    let (bytes, _) = signed_inner(&client, &signer, inner_call.clone(), 1).await;
    install_inner(&fake, bytes, 101, false);
    fake.mutate_state(|state| {
        state.set_handler("chain_getFinalizedHead", |state, request| {
            if !state.submissions.is_empty() {
                Reply::error(-32603, "temporarily unavailable", None)
            } else {
                state.dispatch(request)
            }
        })
    });
    let opts = ShieldOptions {
        wait: Wait::Finalization,
        ..Default::default()
    };
    let result = client.submit_shielded(inner_call, &signer, &opts).await;
    let error = assert_unknown(result, &fake);
    assert!(matches!(error, SubmitFailure::Unresolved { ref cause, .. }
        if matches!(cause.as_ref(), SubmitFailure::Client(ClientError::Protocol(msg))
            if msg.contains("4 consecutive finalized-head RPC attempts"))));
    assert_eq!(fake.state().lock().unwrap().submissions.len(), 1);
    assert_eq!(
        client.nonces().cached(&signer.account().to_string()).await,
        None
    );
}

#[tokio::test]
async fn finalized_inner_reorg_is_resolved_on_the_canonical_branch() {
    let (client, fake, signer) = fixture().await;
    fake.mutate_state(|state| state.finalized = 100);
    let inner_call = call(&client, 1).await;
    let (bytes, inner_hash) = signed_inner(&client, &signer, inner_call.clone(), 1).await;
    install_inner(&fake, bytes.clone(), 101, false);
    let canonical_seen = Arc::new(Mutex::new(None::<String>));
    let canonical_slot = canonical_seen.clone();
    fake.mutate_state(|state| {
        state.set_handler("chain_getFinalizedHead", move |state, request| {
            if !state.submissions.is_empty() && canonical_slot.lock().unwrap().is_none() {
                let old = state.block_hash(101).unwrap();
                let parent = state.block_hash(100).unwrap();
                // The observed inner's block is still unfinalized when the branch
                // changes. A different block at the same height includes it.
                state.finalized = 100;
                let canonical = state.fork(&parent).unwrap();
                state.set_best(&canonical).unwrap();
                state.set_finalized(101).unwrap();
                state
                    .seed_events(&canonical, json!([success(0), success(1)]))
                    .unwrap();
                *canonical_slot.lock().unwrap() = Some(canonical.clone());
                assert_ne!(canonical, old);
            }
            state.dispatch(request)
        })
    });
    let opts = ShieldOptions {
        wait: Wait::Finalization,
        ..Default::default()
    };
    let report = client
        .submit_shielded(inner_call, &signer, &opts)
        .await
        .unwrap();
    assert_eq!(report.hash, inner_hash);
    assert_eq!(report.inclusion, Inclusion::Finalized);
    let expected = canonical_seen.lock().unwrap().clone().unwrap();
    assert_eq!(
        format!("0x{}", hex::encode(report.block_hash.unwrap().0)),
        expected
    );
    assert_eq!(fake.state().lock().unwrap().submissions.len(), 1);
}

#[tokio::test]
async fn finalization_failure_never_returns_an_unfinalized_carrier_failure() {
    let (client, fake, signer) = fixture().await;
    let failed = json!([{"phase":"ApplyExtrinsic","extrinsic_idx":0,"module_id":"System","event_id":"ExtrinsicFailed","attributes":{"dispatch_error":{"Token":"FundsUnavailable"},"dispatch_info":{"weight":{"ref_time":1,"proof_size":0},"class":"Normal","pays_fee":"Yes"}},"topics":[]}]);
    fake.mutate_state(|state| {
        state.queue_submit(SubmitScript::InBlockThenFinalized { events: failed })
    });
    let opts = ShieldOptions {
        wait: Wait::Finalization,
        ..Default::default()
    };
    let report = client
        .submit_shielded(call(&client, 1).await, &signer, &opts)
        .await
        .unwrap();
    assert!(!report.success);
    assert_eq!(report.inclusion, Inclusion::Finalized);
    assert!(report.error.is_some());
    assert_eq!(
        client.nonces().cached(&signer.account().to_string()).await,
        None
    );
}

#[tokio::test]
async fn earlier_canonical_inner_is_rescanned_after_orphan_disappears() {
    let (client, fake, signer) = fixture().await;
    let inner_call = call(&client, 1).await;
    let (bytes, inner_hash) = signed_inner(&client, &signer, inner_call.clone(), 1).await;
    let reorganized = Arc::new(Mutex::new(false));
    let reorg_block = reorganized.clone();
    fake.mutate_state(|state| {
        state.set_handler("chain_getBlock", move |state, request| {
            let mut reply = state.dispatch(request);
            if let Some(Ok(raw)) = &mut reply.outcome {
                let number = raw["block"]["header"]["number"]
                    .as_str()
                    .and_then(|n| u64::from_str_radix(n.trim_start_matches("0x"), 16).ok());
                let changed = *reorg_block.lock().unwrap();
                if (number == Some(102) && !changed) || (number == Some(101) && changed) {
                    raw["block"]["extrinsics"]
                        .as_array_mut()
                        .unwrap()
                        .push(json!(format!("0x{}", hex::encode(&bytes))));
                    state
                        .seed_events(
                            request.params[0].as_str().unwrap(),
                            json!([success(0), success(1)]),
                        )
                        .unwrap();
                }
                if number == Some(101) && !changed {
                    state.advance(1);
                }
            }
            reply
        });
    });
    let reorg_finality = reorganized.clone();
    fake.mutate_state(|state| {
        state.set_handler("chain_getFinalizedHead", move |state, request| {
            if !state.submissions.is_empty() && !*reorg_finality.lock().unwrap() {
                state.finalized = 100;
                let parent = state.block_hash(101).unwrap();
                let canonical = state.fork(&parent).unwrap();
                state.set_best(&canonical).unwrap();
                state.set_finalized(102).unwrap();
                *reorg_finality.lock().unwrap() = true;
            }
            state.dispatch(request)
        })
    });
    let opts = ShieldOptions {
        wait: Wait::Finalization,
        ..Default::default()
    };
    let report = client
        .submit_shielded(inner_call, &signer, &opts)
        .await
        .unwrap();
    assert_eq!(report.hash, inner_hash);
    assert_eq!(report.block_number, Some(101));
    assert_eq!(report.inclusion, Inclusion::Finalized);
    assert!(*reorganized.lock().unwrap());
    assert_eq!(fake.state().lock().unwrap().submissions.len(), 1);
}

#[tokio::test]
async fn author_ack_timeout_keeps_carrier_hash_and_clears_pair() {
    let (client, fake, signer) = fixture().await;
    fake.mutate_state(|state| {
        state.set_handler("author_submitExtrinsic", |state, request| {
            let _ = state.dispatch(request);
            Reply::silent()
        })
    });
    let opts = ShieldOptions {
        submit: SubmitOptions {
            submit_deadline: Some(Duration::from_secs(2)),
            ..Default::default()
        },
        ..pool()
    };
    let result = client
        .submit_shielded(call(&client, 1).await, &signer, &opts)
        .await;
    assert_unknown(result, &fake);
    assert_eq!(
        client.nonces().cached(&signer.account().to_string()).await,
        None
    );
}

#[tokio::test]
async fn unknown_carrier_fate_reconciles_history_without_retransmission() {
    let (client, fake, signer) = fixture().await;
    let inner_call = call(&client, 1).await;
    let (bytes, inner_hash) = signed_inner(&client, &signer, inner_call.clone(), 1).await;
    install_inner(&fake, bytes, 101, false);
    // A transport fate loss after successful admission follows the same
    // reconciliation branch as an acknowledgment deadline, with the complete
    // operation budget left available for finalized history.
    fake.mutate_state(|state| {
        state.set_handler("author_submitExtrinsic", |state, request| {
            let _ = state.dispatch(request);
            Reply::silent()
        })
    });
    let opts = ShieldOptions {
        submit: SubmitOptions {
            submit_deadline: Some(Duration::from_secs(2)),
            deadline: Some(Duration::from_secs(5)),
            reconcile: true,
            ..Default::default()
        },
        ..pool()
    };
    let report = client
        .submit_shielded(inner_call, &signer, &opts)
        .await
        .unwrap();
    assert_eq!(report.hash, inner_hash);
    assert_eq!(fake.state().lock().unwrap().submissions.len(), 1);
    assert_eq!(
        client.nonces().cached(&signer.account().to_string()).await,
        None
    );
}

#[tokio::test]
async fn shield_receipt_watch_and_inner_receipt_errors_retain_the_attempted_carrier_and_cause() {
    for fault in 0..4 {
        let (client, fake, signer) = fixture().await;
        let mut block_reads = 0usize;
        fake.mutate_state(|state| match fault {
            0 => state.set_handler("chain_getBlock", |_, _| {
                Reply::error(-32603, "carrier receipt RPC sentinel", None)
            }),
            1 => state.set_handler("chain_getBlock", |_, _| {
                Reply::result(json!({"block":{"header":{},"extrinsics":[]}}))
            }),
            2 => state.queue_submit(SubmitScript::Statuses(vec![json!({
                "finalized":"malformed-block-hash"
            })])),
            _ => state.set_handler("chain_getBlock", move |state, request| {
                block_reads += 1;
                if block_reads == 1 {
                    state.dispatch(request)
                } else {
                    // The carrier receipt is resolved; this fails the search
                    // for the decrypted inner while its nonce is unsettled.
                    Reply::error(-32603, "inner receipt RPC sentinel", None)
                }
            }),
        });
        let error = assert_unknown(
            client
                .submit_shielded(
                    call(&client, 1).await,
                    &signer,
                    &ShieldOptions {
                        wait: Wait::Inclusion,
                        submit: SubmitOptions {
                            deadline: Some(Duration::from_secs(2)),
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                )
                .await,
            &fake,
        );
        let SubmitFailure::Unresolved { cause, .. } = error else {
            panic!("shield receipt failure lost its original cause");
        };
        assert!(matches!(cause.as_ref(), SubmitFailure::Client(_)));
        if fault == 0 {
            assert!(cause.to_string().contains("carrier receipt RPC sentinel"));
        } else if fault == 3 {
            assert!(cause.to_string().contains("inner receipt RPC sentinel"));
        }
        assert_eq!(
            client.nonces().cached(&signer.account().to_string()).await,
            None
        );
        let next = client.nonces().lease(&signer.account()).await.unwrap();
        // A fresh pool-aware query supplies this value; neither n nor n+1
        // remains reserved by the failed shield operation.
        let fresh = client.nonces().fresh(&signer.account()).await.unwrap();
        assert_eq!(next.value(), fresh);
        next.settle();
    }
}

#[tokio::test]
async fn shield_reconciliation_failure_keeps_both_receipt_and_scan_causes() {
    let (client, fake, signer) = fixture().await;
    fake.mutate_state(|state| {
        state.set_handler("chain_getBlock", |_, _| {
            Reply::error(-32603, "original shield receipt sentinel", None)
        });
        state.set_handler("chain_getFinalizedHead", |state, request| {
            if state.submissions.is_empty() {
                state.dispatch(request)
            } else {
                Reply::error(-32603, "shield reconciliation RPC sentinel", None)
            }
        });
    });
    let error = assert_unknown(
        client
            .submit_shielded(
                call(&client, 1).await,
                &signer,
                &ShieldOptions {
                    wait: Wait::Inclusion,
                    submit: SubmitOptions {
                        reconcile: true,
                        deadline: Some(Duration::from_secs(2)),
                        ..Default::default()
                    },
                    ..Default::default()
                },
            )
            .await,
        &fake,
    );
    let SubmitFailure::Unresolved {
        cause,
        reconciliation: Some(reconciliation),
        ..
    } = error
    else {
        panic!("shield reconciliation discarded one of its causes");
    };
    assert!(
        cause
            .to_string()
            .contains("original shield receipt sentinel")
    );
    assert!(
        reconciliation
            .to_string()
            .contains("shield reconciliation RPC sentinel")
    );
    assert_eq!(
        client.nonces().cached(&signer.account().to_string()).await,
        None
    );
}

#[tokio::test]
async fn runtime_upgrade_after_inner_signature_refuses_carrier_broadcast() {
    let (client, fake, signer) = fixture().await;
    let signed = signer.signed.clone();
    let inner_call = call(&client, 1).await;
    fake.mutate_state(|state| {
        state.set_handler("state_getRuntimeVersion", move |state, request| {
            if signed.lock().unwrap().len() == 1 {
                state.runtime = Arc::new(
                    rtao_core::runtime::Runtime::parse(
                        &state.runtime.metadata_bytes,
                        state.runtime.spec_version,
                        2,
                        state.runtime.ss58_format,
                    )
                    .unwrap(),
                );
            }
            state.dispatch(request)
        })
    });
    let result = client.submit_shielded(inner_call, &signer, &pool()).await;
    assert!(result.is_err());
    assert_eq!(signer.signed.lock().unwrap().len(), 1);
    assert!(fake.state().lock().unwrap().submissions.is_empty());
    assert_eq!(
        client.nonces().cached(&signer.account().to_string()).await,
        None
    );
}

#[derive(Clone, Copy)]
enum PrepareBehavior {
    Advance(u64),
    Refuse,
    Pending,
    ChangeAccount,
    ChangeAlgorithm,
}
struct PreparingSigner {
    inner: RecordingSigner,
    fake: FakeChain,
    behavior: PrepareBehavior,
    entered: Arc<tokio::sync::Notify>,
    calls: std::sync::atomic::AtomicUsize,
}
impl PreparingSigner {
    fn new(inner: RecordingSigner, fake: FakeChain, behavior: PrepareBehavior) -> Self {
        Self {
            inner,
            fake,
            behavior,
            entered: Arc::new(tokio::sync::Notify::new()),
            calls: std::sync::atomic::AtomicUsize::new(0),
        }
    }
}
impl Signer for PreparingSigner {
    fn account(&self) -> AccountId {
        if matches!(self.behavior, PrepareBehavior::ChangeAccount)
            && self.calls.load(std::sync::atomic::Ordering::SeqCst) > 0
        {
            AccountId([19; 32])
        } else {
            self.inner.account()
        }
    }
    fn crypto_type(&self) -> CryptoType {
        if matches!(self.behavior, PrepareBehavior::ChangeAlgorithm)
            && self.calls.load(std::sync::atomic::Ordering::SeqCst) > 0
        {
            CryptoType::Sr25519
        } else {
            self.inner.crypto_type()
        }
    }
    fn caps(&self) -> SignerCaps {
        self.inner.caps()
    }
    fn prepare_shielded(&self) -> BoxFuture<'_, Result<(), SignerError>> {
        Box::pin(async move {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.entered.notify_one();
            match self.behavior {
                PrepareBehavior::Advance(lag) => {
                    self.fake.mutate_state(|state| {
                        state.advance(40);
                        state.set_nonce(self.account().to_string(), 47);
                        state.set_finalized(state.block - lag).unwrap();
                    });
                    Ok(())
                }
                PrepareBehavior::Refuse => {
                    Err(SignerError::Vault("fixture readiness refused".into()))
                }
                PrepareBehavior::Pending => std::future::pending().await,
                PrepareBehavior::ChangeAccount | PrepareBehavior::ChangeAlgorithm => Ok(()),
            }
        })
    }
    fn sign<'a>(
        &'a self,
        request: &'a SigningRequest<'a>,
    ) -> BoxFuture<'a, Result<MultiSignature, SignerError>> {
        self.inner.sign(request)
    }
}

// The three refused-lag cases each exercise the real default finality wait.
// Advance that clock virtually instead of waiting three wall-clock minutes.
#[tokio::test(start_paused = true)]
async fn shield_preparation_precedes_fresh_nonce_anchor_and_finality_guard() {
    for wrapper in ["direct", "arc", "dyn_arc"] {
        for lag in [2, 6] {
            let (client, fake, inner) = fixture().await;
            let signer = Arc::new(PreparingSigner::new(
                inner,
                fake.clone(),
                PrepareBehavior::Advance(lag),
            ));
            let dynamic: Arc<dyn Signer> = signer.clone();
            let selected: &dyn Signer = match wrapper {
                "arc" => &signer,
                "dyn_arc" => &dynamic,
                _ => signer.as_ref(),
            };
            fake.mutate_state(|state| {
                state.set_nonce(signer.account().to_string(), 7);
                state.set_finalized(state.block - 2).unwrap();
            });
            let initial_finalized = fake.state().lock().unwrap().finalized;
            let inner_call = call(&client, 3).await;
            let started = tokio::time::Instant::now();
            let result = client
                .submit_shielded(inner_call.clone(), selected, &pool())
                .await;
            assert_eq!(signer.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
            if lag == 6 {
                assert!(started.elapsed() >= rtao_client::SHIELD_FINALITY_WAIT);
                assert!(matches!(
                    result,
                    Err(SubmitFailure::ShieldEraExpired {
                        period: 8,
                        finality_lag: 6,
                    })
                ));
                assert!(signer.inner.signed.lock().unwrap().is_empty());
                assert!(fake.state().lock().unwrap().submissions.is_empty());
                assert_eq!(
                    client.nonces().cached(&signer.account().to_string()).await,
                    None
                );
            } else {
                let result = result.unwrap();
                assert_eq!(result.inclusion, Inclusion::PoolAccepted);
                let signed = signer.inner.signed.lock().unwrap().clone();
                let expected_anchor = fake
                    .state()
                    .lock()
                    .unwrap()
                    .block_hash(initial_finalized + 40)
                    .unwrap();
                assert_eq!(signed.len(), 2);
                assert_eq!(signed[0].purpose, SignPurpose::ShieldInner);
                assert_eq!(signed[1].purpose, SignPurpose::ShieldCarrier);
                assert_eq!(signed[0].call, inner_call.as_bytes());
                assert_eq!(signed[0].json["nonce"], "0x00000030");
                assert_eq!(signed[1].json["nonce"], "0x0000002f");
                assert_eq!(signed[0].json["blockHash"], expected_anchor);
                assert_eq!(signed[1].json["blockHash"], expected_anchor);
                assert_eq!(signed[0].json["era"], signed[1].json["era"]);
                assert_eq!(fake.state().lock().unwrap().submissions.len(), 1);
                let next = client.nonces().lease(&signer.account()).await.unwrap();
                assert_eq!(next.value(), 49);
                next.settle();
            }
            client.close().await;
        }
    }
}

#[tokio::test]
async fn shield_preparation_failure_cancellation_and_deadline_leave_no_signing_material() {
    for case in ["refuse", "cancel", "deadline", "account", "algorithm"] {
        let (client, fake, inner) = fixture().await;
        let behavior = match case {
            "refuse" => PrepareBehavior::Refuse,
            "account" => PrepareBehavior::ChangeAccount,
            "algorithm" => PrepareBehavior::ChangeAlgorithm,
            _ => PrepareBehavior::Pending,
        };
        let signer = Arc::new(PreparingSigner::new(inner, fake.clone(), behavior));
        let reviewed_account = signer.account();
        let inner_call = call(&client, 1).await;
        let before = fake.requests().len();
        if case == "cancel" {
            let task_client = client.clone();
            let task_signer = signer.clone();
            let task = tokio::spawn(async move {
                task_client
                    .submit_shielded(inner_call, task_signer.as_ref(), &pool())
                    .await
            });
            tokio::time::timeout(Duration::from_secs(2), signer.entered.notified())
                .await
                .unwrap();
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
        } else {
            let options = ShieldOptions {
                submit: SubmitOptions {
                    submit_deadline: (case == "deadline").then_some(Duration::from_secs(2)),
                    ..Default::default()
                },
                ..pool()
            };
            let result = client
                .submit_shielded(inner_call, signer.as_ref(), &options)
                .await;
            if case == "refuse" {
                assert!(
                    matches!(result, Err(SubmitFailure::Client(ClientError::Signing(
                    SignerError::Vault(message)
                ))) if message == "fixture readiness refused")
                );
            } else if matches!(case, "account" | "algorithm") {
                assert!(
                    matches!(result, Err(SubmitFailure::Client(ClientError::Signing(
                        SignerError::InvalidSignature(message)
                    ))) if message == "signer identity changed during shield preparation")
                );
            } else {
                assert!(matches!(
                    result,
                    Err(SubmitFailure::Client(ClientError::Rpc(
                        rtao_rpc::RpcError::Timeout { .. }
                    )))
                ));
            }
        }
        assert_eq!(signer.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(signer.inner.signed.lock().unwrap().is_empty());
        assert!(fake.state().lock().unwrap().submissions.is_empty());
        assert_eq!(
            client.nonces().cached(&reviewed_account.to_string()).await,
            None
        );
        assert_eq!(
            client.nonces().cached(&signer.account().to_string()).await,
            None
        );
        assert!(!fake.requests()[before..].iter().any(|request| {
            request.method.starts_with("author_")
                || request.method == "account_nextIndex"
                || request.method == "chain_getFinalizedHead"
        }));
        client.close().await;
    }
}

// pyt: unit/test_intents_table.py::TestExecuteFlow::test_shielded_inner_finalization_recovers_from_transient_rpc_failure
// pyt: unit/test_intents_table.py::TestExecuteFlow::test_shielded_inner_finalization_fails_after_bounded_rpc_retries
// pyt: unit/test_intents_table.py::TestExecuteFlow::test_shielded_inner_finalization_fails_when_finality_stalls
// Native P8 uses 36 finality polls; successful behind-head reads reset the
// consecutive-RPC-failure counter without finalizing the included inner.
#[tokio::test(start_paused = true)]
async fn found_inner_finality_recovers_resets_and_stops_at_exact_native_bounds() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    for scenario in ["transient", "reset", "permanent", "stall"] {
        let (client, fake, signer) = fixture().await;
        let inner_call = call(&client, 1).await;
        let (inner_bytes, inner_hash) = signed_inner(&client, &signer, inner_call.clone(), 1).await;
        install_inner(&fake, inner_bytes, 101, false);
        let reads = Arc::new(AtomicUsize::new(0));
        let counted = reads.clone();
        fake.mutate_state(|state| {
            state.set_handler("chain_getFinalizedHead", move |state, request| {
                if state.submissions.is_empty() {
                    return state.dispatch(request);
                }
                let attempt = counted.fetch_add(1, Ordering::SeqCst);
                let fail = match scenario {
                    "transient" => attempt == 0,
                    "reset" => matches!(attempt, 0 | 2 | 4 | 6),
                    "permanent" => true,
                    _ => false,
                };
                if fail {
                    return Reply::error(-32603, "temporary finality fixture failure", None);
                }
                let finalized = if scenario == "stall" || (scenario == "reset" && attempt < 7) {
                    100
                } else {
                    101
                };
                state.set_finalized(finalized).unwrap();
                state.dispatch(request)
            });
        });
        let options = ShieldOptions {
            wait: Wait::Finalization,
            submit: SubmitOptions {
                deadline: Some(Duration::from_secs(5)),
                reconcile: false,
                ..Default::default()
            },
            ..Default::default()
        };
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            client.submit_shielded(inner_call, &signer, &options),
        )
        .await
        .unwrap();
        let expected_reads = match scenario {
            "transient" => 2,
            "reset" => 8,
            "permanent" => 4,
            _ => 36,
        };
        assert_eq!(reads.load(Ordering::SeqCst), expected_reads, "{scenario}");
        if matches!(scenario, "transient" | "reset") {
            let report = result.unwrap();
            assert!(report.success, "{scenario}");
            assert_eq!(report.inclusion, Inclusion::Finalized);
            assert_eq!(report.hash, inner_hash);
            assert_eq!(report.block_number, Some(101));
            assert_eq!(report.extrinsic_index, Some(1));
            assert_eq!(report.shield.unwrap().inner_hash, inner_hash);
        } else {
            let expected = if scenario == "permanent" {
                "4 consecutive finalized-head RPC attempts"
            } else {
                "did not finalize after 36 polls"
            };
            let error = assert_unknown(result, &fake);
            assert!(
                matches!(error, SubmitFailure::Unresolved { ref cause, .. }
                    if matches!(cause.as_ref(), SubmitFailure::Client(ClientError::Protocol(message))
                        if message.contains(expected))),
                "{scenario}: {error}"
            );
        }
        assert_eq!(fake.state().lock().unwrap().submissions.len(), 1);
        assert_eq!(
            fake.requests()
                .iter()
                .filter(|request| request.method.starts_with("author_submit"))
                .count(),
            1,
            "{scenario}"
        );
        {
            let signed = signer.signed.lock().unwrap();
            assert_eq!(signed.len(), 2);
            assert_eq!(signed[0].purpose, SignPurpose::ShieldInner);
            assert_eq!(signed[1].purpose, SignPurpose::ShieldCarrier);
        }
        client.close().await;
    }
}

// pyt: unit/test_intents_table.py::TestExecuteFlow::test_shielded_inner_finalization_recovers_from_canonical_hash_rpc_failure
// pyt: unit/test_intents_table.py::TestExecuteFlow::test_shielded_inner_finalization_bounds_hung_canonical_hash_rpc
// pyt: unit/test_intents_table.py::TestExecuteFlow::test_shielded_inner_finalization_recovers_from_canonical_receipt_rpc_failure
// pyt: unit/test_intents_table.py::TestExecuteFlow::test_shielded_inner_finalization_bounds_hung_canonical_receipt_rpc
// Native P8 uses signed carrier/inner bytes and a FakeChain fork. A failed
// wait retains the carrier identity; public read-only recovery resolves the
// same inner from its independently finalized canonical block without resending.
#[tokio::test(start_paused = true)]
async fn canonical_inner_hash_and_receipt_reads_recover_or_stop_without_retransmission() {
    use blake2::{Blake2b, Digest, digest::consts::U32};

    for (method, hung) in [
        ("chain_getBlockHash", false),
        ("chain_getBlockHash", true),
        ("chain_getBlock", false),
        ("chain_getBlock", true),
    ] {
        let (client, fake, signer) = fixture().await;
        fake.mutate_state(|state| state.set_finalized(99).unwrap());
        let inner_call = call(&client, 1).await;
        let (inner_bytes, inner_hash) = signed_inner(&client, &signer, inner_call.clone(), 1).await;
        install_inner(&fake, inner_bytes, 101, false);
        let branch = Arc::new(Mutex::new(None::<(String, String, usize)>));
        let forked = branch.clone();
        fake.mutate_state(|state| {
            state.set_handler("chain_getFinalizedHead", move |state, request| {
                if !state.submissions.is_empty() && forked.lock().unwrap().is_none() {
                    let orphan = state.block_hash(101).unwrap();
                    let parent = state.block_hash(100).unwrap();
                    assert_eq!(state.finalized, 100);
                    let canonical = state.fork(&parent).unwrap();
                    state.set_best(&canonical).unwrap();
                    state.set_finalized(101).unwrap();
                    // Arm faults only after the orphan's signed inner was found.
                    // The queue precedes install_inner's unchanged block handler.
                    for _ in 0..if hung { 4 } else { 1 } {
                        state.queue_reply(
                            method,
                            if hung {
                                Reply::silent()
                            } else {
                                Reply::error(-32603, "canonical read temporarily unavailable", None)
                            },
                        );
                    }
                    *forked.lock().unwrap() = Some((orphan, canonical, state.requests.len()));
                }
                state.dispatch(request)
            });
        });
        let options = ShieldOptions {
            wait: Wait::Finalization,
            submit: SubmitOptions {
                deadline: Some(Duration::from_secs(5)),
                reconcile: false,
                ..Default::default()
            },
            ..Default::default()
        };
        let started = tokio::time::Instant::now();
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            client.submit_shielded(inner_call.clone(), &signer, &options),
        )
        .await
        .unwrap();
        let (orphan, canonical, first_canonical_read) = branch.lock().unwrap().clone().unwrap();
        assert_ne!(canonical, orphan);
        let canonical_hash = H256(
            hex::decode(canonical.trim_start_matches("0x"))
                .unwrap()
                .try_into()
                .unwrap(),
        );
        assert_eq!(
            fake.state().lock().unwrap().block_hash(101),
            Some(canonical.clone())
        );
        assert_eq!(fake.state().lock().unwrap().finalized, 101);
        let requests = fake.requests();
        assert!(requests[..first_canonical_read].iter().any(|request| {
            request.method == "chain_getBlock" && request.params == json!([orphan])
        }));
        let canonical_params = if method == "chain_getBlockHash" {
            json!([101])
        } else {
            json!([canonical])
        };
        let reads = requests[first_canonical_read..]
            .iter()
            .filter(|request| request.method == method && request.params == canonical_params)
            .count();
        // Receipt recovery makes one additional block read to decode the result.
        assert_eq!(
            reads,
            if hung {
                4
            } else if method == "chain_getBlockHash" {
                2
            } else {
                3
            }
        );
        let submitted = fake.state().lock().unwrap().submissions.clone();
        assert_eq!(submitted.len(), 1);
        let carrier_hash = H256(Blake2b::<U32>::digest(&submitted[0]).into());
        let returned = if hung {
            assert!(started.elapsed() >= Duration::from_millis(400));
            assert!(started.elapsed() < Duration::from_secs(5));
            let expected = if method == "chain_getBlockHash" {
                "after 4 canonical block-hash RPC attempts"
            } else {
                "after 4 canonical receipt RPC attempts"
            };
            let error = assert_unknown(result, &fake);
            assert!(
                matches!(error, SubmitFailure::Unresolved { hash, ref cause, reconciliation: None }
                    if hash == carrier_hash
                        && matches!(cause.as_ref(), SubmitFailure::Client(ClientError::Protocol(message))
                            if message.contains(expected))),
                "{method}: {error}"
            );
            assert_eq!(
                client.nonces().cached(&signer.account().to_string()).await,
                None
            );
            None
        } else {
            let report = result.unwrap();
            let paired = report.shield.unwrap();
            assert_eq!(paired.carrier_hash, carrier_hash);
            assert_eq!(paired.inner_hash, inner_hash);
            Some(report)
        };
        // This public API consumes the caller's canonical hash/finality flag;
        // it performs receipt lookup, not another submission or finality wait.
        let recovered = client
            .resolve_extrinsic(inner_hash, canonical_hash, true)
            .await
            .unwrap();
        for report in returned.iter().chain(std::iter::once(&recovered)) {
            assert!(report.success);
            assert_eq!(report.inclusion, Inclusion::Finalized);
            assert_eq!(report.hash, inner_hash);
            assert_eq!(report.block_hash, Some(canonical_hash));
            assert_eq!(report.block_number, Some(101));
            assert_eq!(report.extrinsic_index, Some(0));
            assert_eq!(report.extrinsic_id.as_deref(), Some("101-0000"));
            assert_eq!(report.events.len(), 1);
        }
        assert_eq!(fake.state().lock().unwrap().submissions, submitted);
        assert_eq!(
            fake.requests()
                .iter()
                .filter(|request| request.method.starts_with("author_submit"))
                .count(),
            1
        );
        {
            let signed = signer.signed.lock().unwrap();
            assert_eq!(signed.len(), 2);
            assert_eq!(signed[0].purpose, SignPurpose::ShieldInner);
            assert_eq!(signed[0].call, inner_call.as_bytes());
            assert_eq!(signed[1].purpose, SignPurpose::ShieldCarrier);
        }
        client.close().await;
    }
}
