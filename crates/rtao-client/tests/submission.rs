//! Production transaction preparation, lease settlement and watch semantics.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use rtao_client::{
    BlockRef, Client, ClientConfig, Inclusion, Mortality, Network, NoncePolicy, Period,
    PoolRejection, SubmitFailure, SubmitOptions, Wait,
};
use rtao_signer::{
    AccountId, BoxFuture, CryptoType, KeypairSigner, MultiSignature, Signer, SignerCaps,
    SignerError, SigningRequest,
};
use rtao_testkit::{FakeChain, FakeState, Reply, SubmitScript};
use serde_json::json;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use subxt_rpcs::RpcClient;

fn config() -> ClientConfig {
    let mut cfg = ClientConfig::new(Network::resolve_with("local", |_| None).unwrap()).unwrap();
    cfg.track_heads = false;
    cfg.runtime_cache_dir = None;
    cfg.config_store = None;
    cfg
}
async fn fixture() -> (Client, FakeChain, KeypairSigner) {
    let fake = FakeChain::from_state(FakeState::default());
    let client = Client::from_rpc(RpcClient::new(fake.clone()), config())
        .await
        .unwrap();
    let signer = KeypairSigner::from_seed(&[7; 32], CryptoType::Ed25519).unwrap();
    (client, fake, signer)
}
async fn call(client: &Client, data: Vec<u8>) -> rtao_runtime::CallBytes {
    rtao_runtime::calls::system::remark(data)
        .compose(&client.refresh_runtime().await.unwrap())
        .unwrap()
}

async fn submit_entry(
    client: &Client,
    call: rtao_runtime::CallBytes,
    signer: &KeypairSigner,
    opts: &SubmitOptions,
    entry: u8,
) -> Result<rtao_client::ExtrinsicReport, SubmitFailure> {
    match entry {
        0 => client.submit(call, signer, opts).await,
        1 => {
            let lease = client.nonces().lease(&signer.account()).await.unwrap();
            client.submit_leased(call, signer, lease, opts).await
        }
        _ => {
            let prepared = client
                .prepare_call(call, signer, &SubmitOptions::default())
                .await
                .unwrap();
            let signature = signer
                .sign(&prepared.signing_request(rtao_signer::SignPurpose::Extrinsic))
                .await
                .unwrap();
            client.submit_signature(&prepared, signature, opts).await
        }
    }
}

fn assert_exact_unknown_fate(
    result: Result<rtao_client::ExtrinsicReport, SubmitFailure>,
    fake: &FakeChain,
) {
    use blake2::{Blake2b, Digest, digest::consts::U32};

    let state = fake.state();
    let state = state.lock().unwrap();
    assert_eq!(state.submissions.len(), 1);
    let expected = rtao_rpc::H256(Blake2b::<U32>::digest(&state.submissions[0]).into());
    assert!(matches!(
        result,
        Err(SubmitFailure::Submit(rtao_rpc::SubmitError::FateUnknown { hash })) if hash == expected
    ));
}

#[tokio::test]
async fn preparation_anchors_finalized_era_and_does_not_reserve_nonce() {
    let (client, fake, signer) = fixture().await;
    fake.mutate_state(|state| state.finalized = 90);
    client.nonces().pin(&signer.account(), 50);
    let prepared = client
        .prepare_call(
            call(&client, vec![1]).await,
            &signer,
            &SubmitOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(prepared.nonce(), 0);
    assert_eq!(
        client.nonces().cached(&signer.account().to_string()).await,
        Some(50)
    );
    assert_eq!(prepared.submission_head(), 90);
    assert_eq!(
        prepared.era_block_hash(),
        client.at(BlockRef::Number(90)).await.unwrap().block().hash
    );
    assert_eq!(prepared.payload_json()["nonce"], json!("0x00000000"));
    assert_eq!(prepared.payload_json()["specVersion"], json!("0x000001de"));
    assert_eq!(
        prepared.payload_json()["transactionVersion"],
        json!("0x00000001")
    );
    assert_eq!(
        prepared.payload_json()["tip"],
        json!("0x00000000000000000000000000000000")
    );
    assert_eq!(prepared.payload_json()["mode"], json!(0));
    assert_eq!(
        prepared.payload_json()["metadataHash"],
        serde_json::Value::Null
    );
    assert_eq!(prepared.context().chain_info.spec_name, "node-subtensor");
}

#[tokio::test]
async fn fee_probe_uses_public_identity_fresh_nonce_and_runtime_api_without_signing() {
    let (client, fake, signer) = fixture().await;
    client.nonces().pin(&signer.account(), 40);
    let estimate = client
        .estimate_weight(
            call(&client, vec![]).await,
            signer.account(),
            signer.crypto_type(),
        )
        .await
        .unwrap();
    assert_eq!(estimate.partial_fee, fake.state().lock().unwrap().fee);
    assert_eq!(
        client.nonces().cached(&signer.account().to_string()).await,
        Some(40)
    );
    let request = fake
        .requests()
        .into_iter()
        .find(|request| {
            request.method == "state_call"
                && request.params[0] == "TransactionPaymentApi_query_info"
        })
        .unwrap();
    let args = hex::decode(
        request.params[1]
            .as_str()
            .unwrap()
            .strip_prefix("0x")
            .unwrap(),
    )
    .unwrap();
    let encoded_len = u32::from_le_bytes(args[args.len() - 4..].try_into().unwrap()) as usize;
    assert_eq!(encoded_len, args.len() - 4);
    assert!(fake.state().lock().unwrap().submissions.is_empty());
    assert!(
        !fake
            .requests()
            .iter()
            .any(|request| request.method == "payment_queryInfo")
    );
}

#[tokio::test]
async fn large_payload_is_hashed_once_and_signature_variant_is_checked() {
    let (client, _, signer) = fixture().await;
    let opts = SubmitOptions {
        mortality: Mortality::Immortal,
        ..Default::default()
    };
    let prepared = client
        .prepare_call(call(&client, vec![8; 300]).await, &signer, &opts)
        .await
        .unwrap();
    assert_eq!(prepared.payload().len(), 32);
    assert_eq!(prepared.era_block_hash(), client.genesis());
    let request = prepared.signing_request(rtao_signer::SignPurpose::Extrinsic);
    let signature = signer.sign(&request).await.unwrap();
    assert!(
        rtao_core::keys::verify_strict(
            prepared.payload(),
            &signature.bytes,
            &signer.account().to_string(),
            u8::from(signature.crypto)
        )
        .unwrap()
    );
    let (bytes, hash) = client.attach_signature(&prepared, signature).await.unwrap();
    assert!(!bytes.is_empty());
    assert_ne!(hash.0, [0; 32]);
    assert!(
        client
            .attach_signature(
                &prepared,
                MultiSignature {
                    crypto: CryptoType::Sr25519,
                    bytes: signature.bytes
                }
            )
            .await
            .is_err()
    );
}

struct MetadataSigner {
    signer: KeypairSigner,
}
impl Signer for MetadataSigner {
    fn account(&self) -> AccountId {
        self.signer.account()
    }
    fn crypto_type(&self) -> CryptoType {
        self.signer.crypto_type()
    }
    fn caps(&self) -> SignerCaps {
        SignerCaps::METADATA_HASH | SignerCaps::UNSIGNED_TX
    }
    fn sign<'a>(
        &'a self,
        request: &'a SigningRequest<'a>,
    ) -> BoxFuture<'a, Result<MultiSignature, SignerError>> {
        Box::pin(async move {
            let digest = rtao_core::digest::metadata_digest(
                &request.context.metadata_bytes,
                &request.context.chain_info,
            )
            .map_err(rtao_core::CoreError::from)?;
            assert_eq!(request.payload_json["mode"], json!(1));
            assert_eq!(
                request.payload_json["metadataHash"],
                json!(format!("0x{}", hex::encode(digest)))
            );
            let proof = rtao_core::digest::generate_extrinsic_proof(
                request.call,
                request.included_in_extrinsic,
                request.included_in_signed_data,
                &request.context.metadata_bytes,
                &request.context.chain_info,
            )
            .map_err(rtao_core::CoreError::from)?;
            assert!(!proof.is_empty());
            self.signer.sign(request).await
        })
    }
}

#[tokio::test]
async fn metadata_verifying_signer_receives_a_proof_consistent_with_the_signed_payload() {
    let (client, fake, signer) = fixture().await;
    let signer = MetadataSigner { signer };
    fake.mutate_state(|state| {
        state.set_result(
            "state_getRuntimeVersion",
            json!({"specName":"runtime-renamed","specVersion":478,"transactionVersion":1,"apis":[]}),
        )
    });
    let call = call(&client, vec![1]).await;
    let prepared = client
        .prepare_call(call.clone(), &signer, &SubmitOptions::default())
        .await
        .unwrap();
    assert_eq!(prepared.context().chain_info.spec_name, "runtime-renamed");
    assert!(prepared.metadata_hash().is_some());
    let repeated = client
        .prepare_call(call.clone(), &signer, &SubmitOptions::default())
        .await
        .unwrap();
    assert_eq!(prepared.metadata_hash(), repeated.metadata_hash());
    assert_eq!(prepared.payload(), repeated.payload());
    let report = client
        .submit(call, &signer, &SubmitOptions::default())
        .await
        .unwrap();
    assert!(report.success);
    assert_eq!(fake.state().lock().unwrap().submissions.len(), 1);
}

#[tokio::test]
async fn pool_acceptance_and_dispatch_failure_both_settle_nonce_leases() {
    let (client, fake, signer) = fixture().await;
    let opts = SubmitOptions {
        wait: Wait::Pool,
        ..Default::default()
    };
    let report = client
        .submit(call(&client, vec![]).await, &signer, &opts)
        .await
        .unwrap();
    assert!(report.success);
    assert_eq!(report.inclusion, Inclusion::PoolAccepted);
    assert_eq!(report.message, "Submitted (not waiting).");
    assert!(report.block_hash.is_none());
    assert_eq!(
        client.nonces().cached(&signer.account().to_string()).await,
        Some(0)
    );
    fake.mutate_state(|state|state.queue_submit(SubmitScript::InBlockThenFinalized{events:json!([{"phase":"ApplyExtrinsic","extrinsic_idx":0,"module_id":"System","event_id":"ExtrinsicFailed","attributes":{"dispatch_error":{"BadOrigin":null},"dispatch_info":{"weight":{"ref_time":1,"proof_size":0},"class":"Normal","pays_fee":"Yes"}},"topics":[]}])}));
    let report = client
        .submit(
            call(&client, vec![1]).await,
            &signer,
            &SubmitOptions::default(),
        )
        .await
        .unwrap();
    assert!(!report.success);
    assert_eq!(report.inclusion, Inclusion::Finalized);
    assert_eq!(report.error.unwrap().name, "BadOrigin");
    assert_eq!(
        client.nonces().cached(&signer.account().to_string()).await,
        Some(1)
    );
}

#[tokio::test]
async fn policy_pool_and_signer_failures_clear_the_account_cache() {
    let (client, fake, signer) = fixture().await;
    client.nonces().pin(&signer.account(), 8);
    fake.mutate_state(|state| {
        state.queue_reply(
            "author_submitAndWatchExtrinsic",
            Reply::error(1014, "opaque", None),
        )
    });
    assert!(matches!(
        client
            .submit(
                call(&client, vec![]).await,
                &signer,
                &SubmitOptions::default()
            )
            .await,
        Err(SubmitFailure::Pool(PoolRejection::PriorityTooLow))
    ));
    assert_eq!(
        client.nonces().cached(&signer.account().to_string()).await,
        None
    );
    fake.mutate_state(|state| {
        state.queue_reply(
            "author_submitAndWatchExtrinsic",
            Reply::error(-32005, "rate limit", Some(json!({"retry_after":"9"}))),
        )
    });
    let error = client
        .submit(
            call(&client, vec![]).await,
            &signer,
            &SubmitOptions::default(),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(error,SubmitFailure::Client(ref error) if error.policy().and_then(|policy|policy.retry_after.as_deref())==Some("9"))
    );
    assert_eq!(
        client.nonces().cached(&signer.account().to_string()).await,
        None
    );
}

struct SlowSigner {
    signer: KeypairSigner,
}
impl Signer for SlowSigner {
    fn account(&self) -> AccountId {
        self.signer.account()
    }
    fn crypto_type(&self) -> CryptoType {
        self.signer.crypto_type()
    }
    fn sign<'a>(
        &'a self,
        _request: &'a SigningRequest<'a>,
    ) -> BoxFuture<'a, Result<MultiSignature, SignerError>> {
        Box::pin(std::future::pending())
    }
}
#[tokio::test]
async fn full_deadline_cancels_signing_and_drops_the_lease_before_broadcast() {
    let (client, fake, signer) = fixture().await;
    let signer = SlowSigner { signer };
    let opts = SubmitOptions {
        deadline: Some(Duration::from_millis(50)),
        ..Default::default()
    };
    assert!(matches!(
        client
            .submit(call(&client, vec![]).await, &signer, &opts)
            .await,
        Err(SubmitFailure::Client(_))
    ));
    assert_eq!(
        client.nonces().cached(&signer.account().to_string()).await,
        None
    );
    assert!(fake.state().lock().unwrap().submissions.is_empty());
}

struct BlockingSigner {
    signer: KeypairSigner,
    delay: Duration,
    entered: AtomicBool,
    fake: FakeChain,
    requests_at_sign: AtomicUsize,
}
impl Signer for BlockingSigner {
    fn account(&self) -> AccountId {
        self.signer.account()
    }
    fn crypto_type(&self) -> CryptoType {
        self.signer.crypto_type()
    }
    fn sign<'a>(
        &'a self,
        request: &'a SigningRequest<'a>,
    ) -> BoxFuture<'a, Result<MultiSignature, SignerError>> {
        Box::pin(async move {
            self.entered.store(true, Ordering::SeqCst);
            self.requests_at_sign
                .store(self.fake.requests().len(), Ordering::SeqCst);
            std::thread::sleep(self.delay);
            self.signer.sign(request).await
        })
    }
}

#[tokio::test]
async fn synchronous_signing_past_deadline_never_broadcasts() {
    let (client, fake, signer) = fixture().await;
    let signer = BlockingSigner {
        signer,
        delay: Duration::from_millis(50),
        entered: AtomicBool::new(false),
        fake: fake.clone(),
        requests_at_sign: AtomicUsize::new(0),
    };
    let opts = SubmitOptions {
        deadline: Some(Duration::from_millis(30)),
        ..Default::default()
    };
    assert!(matches!(
        client
            .submit(call(&client, vec![]).await, &signer, &opts)
            .await,
        Err(SubmitFailure::Client(rtao_client::ClientError::Rpc(
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
async fn standalone_preparation_honors_deadline_without_mutating_cached_nonce() {
    let (client, fake, signer) = fixture().await;
    let call = call(&client, vec![]).await;
    client.nonces().pin(&signer.account(), 8);
    fake.mutate_state(|state| state.queue_reply("account_nextIndex", Reply::silent()));
    assert!(matches!(
        client
            .prepare_call(
                call,
                &signer,
                &SubmitOptions {
                    deadline: Some(Duration::from_millis(30)),
                    ..Default::default()
                }
            )
            .await,
        Err(rtao_client::ClientError::Rpc(
            rtao_rpc::RpcError::Timeout { .. }
        ))
    ));
    assert_eq!(
        client.nonces().cached(&signer.account().to_string()).await,
        Some(8)
    );
    assert!(fake.state().lock().unwrap().submissions.is_empty());
}

#[tokio::test]
async fn admission_deadline_cancels_signing_and_invalidates_owned_or_supplied_lease() {
    for supplied_lease in [false, true] {
        let (client, fake, signer) = fixture().await;
        let call = call(&client, vec![]).await;
        let signer = SlowSigner { signer };
        let opts = SubmitOptions {
            submit_deadline: Some(Duration::from_millis(50)),
            ..Default::default()
        };
        let result = if supplied_lease {
            let lease = client.nonces().lease(&signer.account()).await.unwrap();
            client.submit_leased(call, &signer, lease, &opts).await
        } else {
            client.submit(call, &signer, &opts).await
        };
        assert!(matches!(
            result,
            Err(SubmitFailure::Client(rtao_client::ClientError::Rpc(
                rtao_rpc::RpcError::Timeout { .. }
            )))
        ));
        assert_eq!(
            client.nonces().cached(&signer.account().to_string()).await,
            None
        );
        assert!(fake.state().lock().unwrap().submissions.is_empty());
    }
}

#[tokio::test]
async fn synchronous_signing_obeys_absolute_admission_budget_and_never_broadcasts() {
    let (client, fake, signer) = fixture().await;
    let call = call(&client, vec![]).await;
    let signer = BlockingSigner {
        signer,
        delay: Duration::from_millis(120),
        entered: AtomicBool::new(false),
        fake: fake.clone(),
        requests_at_sign: AtomicUsize::new(0),
    };
    let opts = SubmitOptions {
        deadline: Some(Duration::from_secs(1)),
        submit_deadline: Some(Duration::from_millis(200)),
        ..Default::default()
    };
    fake.mutate_state(|state| {
        state.set_handler("account_nextIndex", |state, request| {
            // Each stage fits in 200ms; their cumulative time exceeds it.
            std::thread::sleep(Duration::from_millis(120));
            state.dispatch(request)
        });
    });
    assert!(matches!(
        client.submit(call, &signer, &opts).await,
        Err(SubmitFailure::Client(rtao_client::ClientError::Rpc(
            rtao_rpc::RpcError::Timeout { .. }
        )))
    ));
    assert!(signer.entered.load(Ordering::SeqCst));
    assert!(fake.state().lock().unwrap().submissions.is_empty());
    assert_eq!(
        client.nonces().cached(&signer.account().to_string()).await,
        None
    );
    // Signature attachment would issue further runtime-stamp reads.
    assert_eq!(
        fake.requests().len(),
        signer.requests_at_sign.load(Ordering::SeqCst)
    );
}

#[tokio::test]
async fn admission_deadline_bounds_nonce_fetch_and_preparation_uses_the_shorter_limit() {
    let (client, fake, signer) = fixture().await;
    let nonce_call = call(&client, vec![]).await;
    fake.mutate_state(|state| state.queue_reply("account_nextIndex", Reply::silent()));
    assert!(matches!(
        client
            .submit(
                nonce_call,
                &signer,
                &SubmitOptions {
                    submit_deadline: Some(Duration::from_millis(30)),
                    ..Default::default()
                },
            )
            .await,
        Err(SubmitFailure::Client(rtao_client::ClientError::Rpc(
            rtao_rpc::RpcError::Timeout { .. }
        )))
    ));
    assert_eq!(
        client.nonces().cached(&signer.account().to_string()).await,
        None
    );
    assert!(fake.state().lock().unwrap().submissions.is_empty());

    for (deadline, submit_deadline) in [
        (None, Some(Duration::from_millis(30))),
        (
            Some(Duration::from_millis(30)),
            Some(Duration::from_secs(1)),
        ),
    ] {
        let (client, fake, signer) = fixture().await;
        let call = call(&client, vec![]).await;
        client.nonces().pin(&signer.account(), 8);
        fake.mutate_state(|state| state.queue_reply("account_nextIndex", Reply::silent()));
        assert!(matches!(
            client
                .prepare_call(
                    call,
                    &signer,
                    &SubmitOptions {
                        deadline,
                        submit_deadline,
                        ..Default::default()
                    },
                )
                .await,
            Err(rtao_client::ClientError::Rpc(
                rtao_rpc::RpcError::Timeout { .. }
            ))
        ));
        assert_eq!(
            client.nonces().cached(&signer.account().to_string()).await,
            Some(8)
        );
        assert!(fake.state().lock().unwrap().submissions.is_empty());
    }
}

#[tokio::test]
async fn admission_deadline_bounds_pool_and_watch_acknowledgment_for_external_signatures_too() {
    for wait in [Wait::Pool, Wait::Finalization] {
        for external_signature in [false, true] {
            let (client, fake, signer) = fixture().await;
            let call = call(&client, vec![]).await;
            let prepared = client
                .prepare_call(call.clone(), &signer, &SubmitOptions::default())
                .await
                .unwrap();
            let signature = signer
                .sign(&prepared.signing_request(rtao_signer::SignPurpose::Extrinsic))
                .await
                .unwrap();
            let method = if wait == Wait::Pool {
                "author_submitExtrinsic"
            } else {
                "author_submitAndWatchExtrinsic"
            };
            fake.mutate_state(|state| {
                state.set_handler(method, |state, request| {
                    // Preserve node-side acceptance while withholding its acknowledgment.
                    let _accepted = state.dispatch(request);
                    Reply::silent()
                });
            });
            let opts = SubmitOptions {
                wait,
                submit_deadline: Some(Duration::from_millis(50)),
                deadline: Some(Duration::from_secs(1)),
                ..Default::default()
            };
            let result = if external_signature {
                client.submit_signature(&prepared, signature, &opts).await
            } else {
                client.submit(call, &signer, &opts).await
            };
            assert!(matches!(
                result,
                Err(SubmitFailure::Submit(
                    rtao_rpc::SubmitError::FateUnknown { .. }
                ))
            ));
            assert_eq!(fake.state().lock().unwrap().submissions.len(), 1);
            assert_eq!(
                client.nonces().cached(&signer.account().to_string()).await,
                None
            );
        }
    }
}

#[tokio::test]
async fn admission_ack_timeout_drops_lease_before_reconciling_actual_inclusion() {
    use futures::FutureExt;

    for wait in [Wait::Pool, Wait::Finalization] {
        for entry in 0..3 {
            let (client, fake, signer) = fixture().await;
            let call = call(&client, vec![]).await;
            let prepared = client
                .prepare_call(call.clone(), &signer, &SubmitOptions::default())
                .await
                .unwrap();
            let signature = signer
                .sign(&prepared.signing_request(rtao_signer::SignPurpose::Extrinsic))
                .await
                .unwrap();
            let accepted = Arc::new(AtomicBool::new(false));
            let cleared_before_scan = Arc::new(AtomicBool::new(false));
            let mut nonces = Some(client.nonces().clone());
            let account = signer.account().to_string();
            let method = if wait == Wait::Pool {
                "author_submitExtrinsic"
            } else {
                "author_submitAndWatchExtrinsic"
            };
            fake.mutate_state(|state| {
                let accepted_by_author = accepted.clone();
                state.set_handler(method, move |state, request| {
                    let reply = state.dispatch(request);
                    assert!(matches!(reply.outcome, Some(Ok(_))));
                    accepted_by_author.store(true, Ordering::SeqCst);
                    Reply::silent()
                });
                let scan_checked = cleared_before_scan.clone();
                state.set_handler("chain_getFinalizedHead", move |state, request| {
                    if accepted.load(Ordering::SeqCst)
                        && let Some(nonces) = nonces.take()
                    {
                        // Consume the observer's manager clone so it retains no RPC cycle.
                        assert_eq!(nonces.cached(&account).now_or_never(), Some(None));
                        scan_checked.store(true, Ordering::SeqCst);
                    }
                    state.dispatch(request)
                });
            });
            let opts = SubmitOptions {
                wait,
                reconcile: true,
                submit_deadline: Some(Duration::from_millis(50)),
                deadline: Some(Duration::from_secs(1)),
                ..Default::default()
            };
            let report = match entry {
                0 => client.submit(call, &signer, &opts).await,
                1 => {
                    let lease = client.nonces().lease(&signer.account()).await.unwrap();
                    client.submit_leased(call, &signer, lease, &opts).await
                }
                _ => client.submit_signature(&prepared, signature, &opts).await,
            }
            .unwrap();
            assert!(report.success);
            assert_eq!(report.inclusion, Inclusion::Finalized);
            assert!(cleared_before_scan.load(Ordering::SeqCst));
            assert_eq!(fake.state().lock().unwrap().submissions.len(), 1);
            assert_eq!(
                client.nonces().cached(&signer.account().to_string()).await,
                None
            );
            let next = client.nonces().lease(&signer.account()).await.unwrap();
            assert_eq!(next.value(), 1);
            next.settle();
        }
    }
}

#[tokio::test]
async fn acknowledged_watch_can_finalize_after_admission_deadline_for_all_submit_entries() {
    for entry in 0..3 {
        let (client, fake, signer) = fixture().await;
        let call = call(&client, vec![]).await;
        let prepared = client
            .prepare_call(call.clone(), &signer, &SubmitOptions::default())
            .await
            .unwrap();
        let signature = signer
            .sign(&prepared.signing_request(rtao_signer::SignPurpose::Extrinsic))
            .await
            .unwrap();
        let (notification_tx, notification_rx) = tokio::sync::oneshot::channel();
        let mut notification_tx = Some(notification_tx);
        fake.mutate_state(|state| {
            state.set_handler("author_submitAndWatchExtrinsic", move |state, request| {
                let mut reply = state.dispatch(request);
                let finalized = reply.notifications.pop().unwrap();
                assert!(finalized.result.get("finalized").is_some());
                notification_tx.take().unwrap().send(finalized).unwrap();
                reply
            });
        });
        let opts = SubmitOptions {
            submit_deadline: Some(Duration::from_millis(50)),
            deadline: Some(Duration::from_secs(1)),
            ..Default::default()
        };
        let submit = async {
            match entry {
                0 => client.submit(call, &signer, &opts).await,
                1 => {
                    let lease = client.nonces().lease(&signer.account()).await.unwrap();
                    client.submit_leased(call, &signer, lease, &opts).await
                }
                _ => client.submit_signature(&prepared, signature, &opts).await,
            }
        };
        let finalize = async {
            let notification = tokio::time::timeout(Duration::from_secs(1), notification_rx)
                .await
                .unwrap()
                .unwrap();
            tokio::time::sleep(Duration::from_millis(100)).await;
            fake.publish_to(&notification.subscription, notification.result)
                .unwrap();
        };
        let (result, ()) = tokio::join!(submit, finalize);
        let report = result.unwrap();
        assert!(report.success);
        assert_eq!(report.inclusion, Inclusion::Finalized);
        assert_eq!(fake.state().lock().unwrap().submissions.len(), 1);
        assert_eq!(
            client.nonces().cached(&signer.account().to_string()).await,
            Some(0)
        );
    }
}

#[tokio::test]
async fn acknowledged_watch_still_obeys_the_full_deadline() {
    for entry in 0..3 {
        for reconcile in [false, true] {
            let (client, fake, signer) = fixture().await;
            let call = call(&client, vec![]).await;
            let accepted = Arc::new(AtomicBool::new(false));
            let scans = Arc::new(AtomicUsize::new(0));
            let author_attempts = Arc::new(AtomicUsize::new(0));
            fake.mutate_state(|state| {
                let accepted_by_author = accepted.clone();
                let author_attempts = author_attempts.clone();
                state.set_handler("author_submitAndWatchExtrinsic", move |state, request| {
                    author_attempts.fetch_add(1, Ordering::SeqCst);
                    let mut reply = state.dispatch(request);
                    reply.notifications.pop();
                    accepted_by_author.store(true, Ordering::SeqCst);
                    reply
                });
                let scans = scans.clone();
                state.set_handler("chain_getFinalizedHead", move |state, request| {
                    if accepted.load(Ordering::SeqCst) {
                        scans.fetch_add(1, Ordering::SeqCst);
                    }
                    state.dispatch(request)
                });
            });
            let result = submit_entry(
                &client,
                call,
                &signer,
                &SubmitOptions {
                    reconcile,
                    submit_deadline: Some(Duration::from_millis(50)),
                    deadline: Some(Duration::from_millis(100)),
                    ..Default::default()
                },
                entry,
            )
            .await;
            assert_exact_unknown_fate(result, &fake);
            assert_eq!(scans.load(Ordering::SeqCst), 0);
            assert_eq!(
                client.nonces().cached(&signer.account().to_string()).await,
                None
            );
            assert_eq!(author_attempts.load(Ordering::SeqCst), 1);
        }
    }
}

#[tokio::test]
async fn full_ack_timeout_keeps_hash_without_starting_reconciliation_after_deadline() {
    for entry in 0..3 {
        for wait in [Wait::Pool, Wait::Finalization] {
            let (client, fake, signer) = fixture().await;
            let call = call(&client, vec![]).await;
            let accepted = Arc::new(AtomicBool::new(false));
            let scans = Arc::new(AtomicUsize::new(0));
            let author = if wait == Wait::Pool {
                "author_submitExtrinsic"
            } else {
                "author_submitAndWatchExtrinsic"
            };
            fake.mutate_state(|state| {
                let accepted_by_author = accepted.clone();
                state.set_handler(author, move |state, request| {
                    let _reply = state.dispatch(request);
                    accepted_by_author.store(true, Ordering::SeqCst);
                    Reply::silent()
                });
                let scans = scans.clone();
                state.set_handler("chain_getFinalizedHead", move |state, request| {
                    if accepted.load(Ordering::SeqCst) {
                        scans.fetch_add(1, Ordering::SeqCst);
                    }
                    state.dispatch(request)
                });
            });
            let result = submit_entry(
                &client,
                call,
                &signer,
                &SubmitOptions {
                    wait,
                    reconcile: true,
                    submit_deadline: Some(Duration::from_millis(50)),
                    deadline: Some(Duration::from_millis(50)),
                    ..Default::default()
                },
                entry,
            )
            .await;
            assert_exact_unknown_fate(result, &fake);
            assert_eq!(scans.load(Ordering::SeqCst), 0);
            assert_eq!(
                client.nonces().cached(&signer.account().to_string()).await,
                None
            );
        }
    }
}

#[tokio::test]
async fn full_timeout_during_reconciliation_keeps_hash_and_clears_all_entry_leases() {
    for entry in 0..3 {
        let (client, fake, signer) = fixture().await;
        let call = call(&client, vec![]).await;
        let accepted = Arc::new(AtomicBool::new(false));
        let scans = Arc::new(AtomicUsize::new(0));
        fake.mutate_state(|state| {
            let accepted_by_author = accepted.clone();
            state.set_handler("author_submitAndWatchExtrinsic", move |state, request| {
                state.queue_submit(SubmitScript::Dropped);
                let reply = state.dispatch(request);
                accepted_by_author.store(true, Ordering::SeqCst);
                reply
            });
            let scans = scans.clone();
            state.set_handler("chain_getFinalizedHead", move |state, request| {
                if accepted.load(Ordering::SeqCst) {
                    scans.fetch_add(1, Ordering::SeqCst);
                    Reply::silent()
                } else {
                    state.dispatch(request)
                }
            });
        });
        let result = submit_entry(
            &client,
            call,
            &signer,
            &SubmitOptions {
                reconcile: true,
                submit_deadline: Some(Duration::from_millis(50)),
                deadline: Some(Duration::from_millis(100)),
                ..Default::default()
            },
            entry,
        )
        .await;
        assert_exact_unknown_fate(result, &fake);
        assert_eq!(scans.load(Ordering::SeqCst), 1);
        assert_eq!(
            client.nonces().cached(&signer.account().to_string()).await,
            None
        );
    }
}

#[tokio::test]
async fn full_timeout_during_report_resolution_keeps_hash_without_rebroadcast() {
    for entry in 0..3 {
        let (client, fake, signer) = fixture().await;
        let call = call(&client, vec![]).await;
        let reports = Arc::new(AtomicUsize::new(0));
        fake.mutate_state(|state| {
            let reports = reports.clone();
            state.set_handler("chain_getBlock", move |_state, _request| {
                reports.fetch_add(1, Ordering::SeqCst);
                Reply::silent()
            });
        });
        let result = submit_entry(
            &client,
            call,
            &signer,
            &SubmitOptions {
                reconcile: true,
                submit_deadline: Some(Duration::from_millis(50)),
                deadline: Some(Duration::from_millis(100)),
                ..Default::default()
            },
            entry,
        )
        .await;
        assert_exact_unknown_fate(result, &fake);
        assert_eq!(reports.load(Ordering::SeqCst), 1);
        assert_eq!(
            client.nonces().cached(&signer.account().to_string()).await,
            None
        );
    }
}

#[tokio::test]
async fn expired_full_budget_before_send_stays_hash_free_for_all_submit_entries() {
    for entry in 0..3 {
        let (client, fake, signer) = fixture().await;
        let call = call(&client, vec![]).await;
        let result = submit_entry(
            &client,
            call,
            &signer,
            &SubmitOptions {
                reconcile: true,
                deadline: Some(Duration::ZERO),
                ..Default::default()
            },
            entry,
        )
        .await;
        assert!(matches!(
            result,
            Err(SubmitFailure::Client(rtao_client::ClientError::Rpc(
                rtao_rpc::RpcError::Timeout { .. }
            )))
        ));
        assert!(fake.state().lock().unwrap().submissions.is_empty());
        assert!(
            fake.requests()
                .iter()
                .all(|request| !request.method.starts_with("author_submit"))
        );
        assert_eq!(
            client.nonces().cached(&signer.account().to_string()).await,
            None
        );
    }
}

#[tokio::test]
async fn closed_injected_client_refuses_both_pool_and_watch_broadcasts() {
    for wait in [Wait::Pool, Wait::Finalization] {
        let (client, fake, signer) = fixture().await;
        let call = call(&client, vec![]).await;
        let prepared = client
            .prepare_call(call.clone(), &signer, &SubmitOptions::default())
            .await
            .unwrap();
        let signature = signer
            .sign(&prepared.signing_request(rtao_signer::SignPurpose::Extrinsic))
            .await
            .unwrap();
        client.close().await;
        let opts = SubmitOptions {
            wait,
            ..Default::default()
        };
        assert!(matches!(
            client.submit(call, &signer, &opts).await,
            Err(SubmitFailure::Client(rtao_client::ClientError::Rpc(
                rtao_rpc::RpcError::Closed
            )))
        ));
        assert!(
            client
                .submit_signature(&prepared, signature, &opts)
                .await
                .is_err()
        );
        assert!(fake.state().lock().unwrap().submissions.is_empty());
    }
}

struct GatedSigner {
    signer: KeypairSigner,
    entered: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
}
impl Signer for GatedSigner {
    fn account(&self) -> AccountId {
        self.signer.account()
    }
    fn crypto_type(&self) -> CryptoType {
        self.signer.crypto_type()
    }
    fn sign<'a>(
        &'a self,
        request: &'a SigningRequest<'a>,
    ) -> BoxFuture<'a, Result<MultiSignature, SignerError>> {
        Box::pin(async move {
            self.entered.notify_one();
            self.release.notified().await;
            self.signer.sign(request).await
        })
    }
}

#[tokio::test]
async fn close_during_signing_cancels_the_operation_and_clears_the_lease() {
    let (client, fake, signer) = fixture().await;
    let account = signer.account();
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let call = call(&client, vec![]).await;
    let signer = GatedSigner {
        signer,
        entered: entered.clone(),
        release: release.clone(),
    };
    let task_client = client.clone();
    let task = tokio::spawn(async move {
        task_client
            .submit(call, &signer, &SubmitOptions::default())
            .await
    });
    tokio::time::timeout(Duration::from_secs(1), entered.notified())
        .await
        .unwrap();
    client.close().await;
    release.notify_one();
    assert!(matches!(
        task.await.unwrap(),
        Err(SubmitFailure::Client(rtao_client::ClientError::Rpc(
            rtao_rpc::RpcError::Closed
        )))
    ));
    assert_eq!(client.nonces().cached(&account.to_string()).await, None);
    assert!(fake.state().lock().unwrap().submissions.is_empty());
}

#[tokio::test]
async fn aborted_live_watch_unsubscribes_and_clears_its_nonce_synchronously() {
    let (client, fake, signer) = fixture().await;
    fake.mutate_state(|state| state.queue_submit(SubmitScript::Statuses(vec![json!("ready")])));
    let account = signer.account();
    let call = call(&client, vec![]).await;
    let task_client = client.clone();
    let task = tokio::spawn(async move {
        task_client
            .submit(call, &signer, &SubmitOptions::default())
            .await
    });
    tokio::time::timeout(Duration::from_secs(1), async {
        while !fake
            .requests()
            .iter()
            .any(|request| request.method == "author_submitAndWatchExtrinsic")
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(client.nonces().cached(&account.to_string()).await, Some(0));
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(client.nonces().cached(&account.to_string()).await, None);
    assert!(
        fake.requests()
            .iter()
            .any(|request| request.method == "author_unwatchExtrinsic")
    );
}

#[tokio::test]
async fn explicit_submission_nonce_is_shared_with_client_clones() {
    let (client, _, signer) = fixture().await;
    let clone = client.clone();
    client
        .submit(
            call(&client, vec![]).await,
            &signer,
            &SubmitOptions {
                nonce: NoncePolicy::Explicit(7),
                wait: Wait::Pool,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let next = clone.nonces().lease(&signer.account()).await.unwrap();
    assert_eq!(next.value(), 8);
    next.settle();
}

#[tokio::test]
async fn prepared_signature_refuses_same_spec_transaction_version_change() {
    let (client, fake, signer) = fixture().await;
    let prepared = client
        .prepare_call(
            call(&client, vec![]).await,
            &signer,
            &SubmitOptions::default(),
        )
        .await
        .unwrap();
    let request = prepared.signing_request(rtao_signer::SignPurpose::Extrinsic);
    let signature = signer.sign(&request).await.unwrap();
    fake.mutate_state(|state| {
        state.set_result(
            "state_getRuntimeVersion",
            json!({"specName":"node-subtensor","specVersion":478,"transactionVersion":2,"apis":[]}),
        )
    });
    assert!(
        client
            .submit_signature(&prepared, signature, &SubmitOptions::default())
            .await
            .is_err()
    );
    assert!(fake.state().lock().unwrap().submissions.is_empty());
    assert_eq!(
        client.nonces().cached(&signer.account().to_string()).await,
        None
    );
    let updated = client.refresh_runtime().await.unwrap();
    assert_eq!(updated.transaction_version(), 2);
    let fresh_call = rtao_runtime::calls::system::remark(vec![])
        .compose(&updated)
        .unwrap();
    let fresh = client
        .prepare_call(fresh_call.clone(), &signer, &SubmitOptions::default())
        .await
        .unwrap();
    assert_eq!(
        fresh.payload_json()["transactionVersion"],
        json!("0x00000002")
    );
    assert!(
        client
            .submit(fresh_call, &signer, &SubmitOptions::default())
            .await
            .unwrap()
            .success
    );
    assert_eq!(fake.state().lock().unwrap().submissions.len(), 1);
}

#[test]
fn normalized_periods_and_pool_error_precedence_match_their_contracts() {
    assert_eq!(Period::new(0).get(), 4);
    assert_eq!(Period::new(129).get(), 256);
    assert_eq!(Period::new(u64::MAX).get(), 65536);
    assert_eq!(
        PoolRejection::from_rpc(1014, "transaction is outdated", None),
        PoolRejection::PriorityTooLow
    );
    assert_eq!(
        PoolRejection::from_rpc(1014, "Extrinsic dropped: {}", None),
        PoolRejection::PriorityTooLow
    );
    assert_eq!(
        PoolRejection::from_rpc(1010, "Invalid Transaction", Some(&json!({"Custom":3}))),
        PoolRejection::Custom(3)
    );
    assert_eq!(
        PoolRejection::from_rpc(1014, "opaque", Some(&json!({"Custom":" ١_٢ "}))),
        PoolRejection::Custom(12)
    );
    assert_eq!(
        PoolRejection::from_rpc(1014, "opaque", Some(&json!({"Custom":true}))),
        PoolRejection::Custom(1)
    );
    assert_eq!(
        PoolRejection::from_rpc(1010, "CuStOm ErRoR: ٣", None),
        PoolRejection::Custom(3)
    );
    let watch = PoolRejection::from_rpc(
        1010,
        "opaque",
        Some(&json!("Extrinsic DROPPED: {'dropped': None}")),
    );
    assert!(matches!(watch, PoolRejection::Watch(_)));
    assert_eq!(watch.name(), Some("Dropped"));
    assert_eq!(
        PoolRejection::from_rpc(
            1010,
            "Invalid Transaction",
            Some(&json!("Transaction is outdated"))
        ),
        PoolRejection::Stale
    );
    assert_eq!(
        PoolRejection::from_rpc(1010, "Custom error: 256", None),
        PoolRejection::Unknown("Custom error: 256".into())
    );
}

struct LostAck {
    chain: FakeChain,
    lost: std::sync::atomic::AtomicBool,
}
impl subxt_rpcs::RpcClientT for LostAck {
    fn request_raw<'a>(
        &'a self,
        method: &'a str,
        params: Option<Box<serde_json::value::RawValue>>,
    ) -> subxt_rpcs::client::RawRpcFuture<'a, Box<serde_json::value::RawValue>> {
        self.chain.request_raw(method, params)
    }
    fn subscribe_raw<'a>(
        &'a self,
        sub: &'a str,
        params: Option<Box<serde_json::value::RawValue>>,
        unsub: &'a str,
    ) -> subxt_rpcs::client::RawRpcFuture<'a, subxt_rpcs::client::RawRpcSubscription> {
        Box::pin(async move {
            use blake2::{Blake2b, Digest, digest::consts::U32};
            let hash = if sub == "author_submitAndWatchExtrinsic" {
                let params: serde_json::Value =
                    serde_json::from_str(params.as_ref().unwrap().get()).unwrap();
                let bytes =
                    hex::decode(params[0].as_str().unwrap().strip_prefix("0x").unwrap()).unwrap();
                Some(rtao_rpc::H256(Blake2b::<U32>::digest(bytes).into()))
            } else {
                None
            };
            let reply = self.chain.subscribe_raw(sub, params, unsub).await?;
            if let Some(hash) = hash
                && !self.lost.swap(true, std::sync::atomic::Ordering::SeqCst)
            {
                drop(reply);
                return Err(subxt_rpcs::Error::Client(Box::new(
                    rtao_rpc::SubmitError::FateUnknown { hash },
                )));
            }
            Ok(reply)
        })
    }
}

struct WatchTimeout {
    chain: FakeChain,
}
impl subxt_rpcs::RpcClientT for WatchTimeout {
    fn request_raw<'a>(
        &'a self,
        method: &'a str,
        params: Option<Box<serde_json::value::RawValue>>,
    ) -> subxt_rpcs::client::RawRpcFuture<'a, Box<serde_json::value::RawValue>> {
        self.chain.request_raw(method, params)
    }
    fn subscribe_raw<'a>(
        &'a self,
        sub: &'a str,
        params: Option<Box<serde_json::value::RawValue>>,
        unsub: &'a str,
    ) -> subxt_rpcs::client::RawRpcFuture<'a, subxt_rpcs::client::RawRpcSubscription> {
        Box::pin(async move {
            use futures::StreamExt;

            let mut reply = self.chain.subscribe_raw(sub, params, unsub).await?;
            if sub == "author_submitAndWatchExtrinsic" {
                // The encoded transaction landed; the acknowledged watch then
                // reports a transport timeout before its finalization item.
                reply.stream = futures::stream::once(async {
                    Err(subxt_rpcs::Error::Client(Box::new(
                        rtao_rpc::RpcError::Timeout {
                            detail: "transaction watch transport timed out".into(),
                        },
                    )))
                })
                .boxed();
            }
            Ok(reply)
        })
    }
}

#[tokio::test]
async fn watch_transport_timeout_reconciles_with_remaining_full_budget_for_all_entries() {
    for entry in 0..3 {
        let fake = FakeChain::from_state(FakeState::default());
        let client = Client::from_rpc(
            RpcClient::new(WatchTimeout {
                chain: fake.clone(),
            }),
            config(),
        )
        .await
        .unwrap();
        let signer = KeypairSigner::from_seed(&[8; 32], CryptoType::Ed25519).unwrap();
        let report = submit_entry(
            &client,
            call(&client, vec![]).await,
            &signer,
            &SubmitOptions {
                reconcile: true,
                submit_deadline: Some(Duration::from_millis(50)),
                deadline: Some(Duration::from_secs(1)),
                ..Default::default()
            },
            entry,
        )
        .await
        .unwrap();
        assert!(report.success);
        assert_eq!(report.inclusion, Inclusion::Finalized);
        assert_eq!(fake.state().lock().unwrap().submissions.len(), 1);
        assert_eq!(
            client.nonces().cached(&signer.account().to_string()).await,
            None
        );
    }
}

#[tokio::test]
async fn lost_acknowledgement_reconciles_actual_inclusion_without_retaining_nonce() {
    let chain = FakeChain::from_state(FakeState::default());
    let client = Client::from_rpc(
        RpcClient::new(LostAck {
            chain: chain.clone(),
            lost: std::sync::atomic::AtomicBool::new(false),
        }),
        config(),
    )
    .await
    .unwrap();
    let signer = KeypairSigner::from_seed(&[8; 32], CryptoType::Ed25519).unwrap();
    let opts = SubmitOptions {
        reconcile: true,
        deadline: Some(Duration::from_secs(1)),
        ..Default::default()
    };
    let report = client
        .submit(call(&client, vec![]).await, &signer, &opts)
        .await
        .unwrap();
    assert_eq!(report.inclusion, Inclusion::Finalized);
    assert!(report.success);
    assert_eq!(
        client.nonces().cached(&signer.account().to_string()).await,
        None
    );
    assert_eq!(chain.state().lock().unwrap().submissions.len(), 1);
    let next = client.nonces().lease(&signer.account()).await.unwrap();
    assert_eq!(next.value(), 1);
    next.settle();
}

#[tokio::test]
async fn retracted_status_keeps_watching_for_later_finalized_inclusion() {
    let (client, fake, signer) = fixture().await;
    let block = fake.state().lock().unwrap().block_hash(100).unwrap();
    fake.mutate_state(|state|{
        state.seed_events(block.clone(),json!([{"phase":"ApplyExtrinsic","extrinsic_idx":0,"module_id":"System","event_id":"ExtrinsicSuccess","attributes":{"dispatch_info":{"weight":{"ref_time":1,"proof_size":0},"class":"Normal","pays_fee":"Yes"}},"topics":[]}])).unwrap();
        state.queue_submit(SubmitScript::Statuses(vec![json!({"inBlock":block}),json!({"retracted":block}),json!({"inBlock":block}),json!({"finalized":block})]));
    });
    let report = client
        .submit(
            call(&client, vec![]).await,
            &signer,
            &SubmitOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(report.inclusion, Inclusion::Finalized);
    assert!(report.success);
    assert_eq!(
        client.nonces().cached(&signer.account().to_string()).await,
        Some(0)
    );
}

#[tokio::test]
async fn dropped_after_retraction_reconciles_even_without_the_dropped_opt_in() {
    let (client, fake, signer) = fixture().await;
    let block = fake.state().lock().unwrap().block_hash(100).unwrap();
    fake.mutate_state(|state| {
        state
            .seed_events(
                block.clone(),
                json!([{"phase":"ApplyExtrinsic","extrinsic_idx":0,"module_id":"System","event_id":"ExtrinsicSuccess","attributes":{"dispatch_info":{"weight":{"ref_time":1,"proof_size":0},"class":"Normal","pays_fee":"Yes"}},"topics":[]}]),
            )
            .unwrap();
        state.queue_submit(SubmitScript::Statuses(vec![
            json!({"inBlock":block}),
            json!({"retracted":block}),
            json!("dropped"),
        ]));
    });
    let report = client
        .submit(
            call(&client, vec![]).await,
            &signer,
            &SubmitOptions {
                deadline: Some(Duration::from_secs(1)),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(report.success);
    assert_eq!(report.inclusion, Inclusion::Finalized);
    assert_eq!(
        client.nonces().cached(&signer.account().to_string()).await,
        None
    );
}

#[tokio::test]
async fn finality_timeout_resolves_actual_inclusion_and_confirms_finality() {
    let (client, fake, signer) = fixture().await;
    let prepared = client
        .prepare_call(
            call(&client, vec![]).await,
            &signer,
            &SubmitOptions::default(),
        )
        .await
        .unwrap();
    let request = prepared.signing_request(rtao_signer::SignPurpose::Extrinsic);
    let signature = signer.sign(&request).await.unwrap();
    let pool = SubmitOptions {
        wait: Wait::Pool,
        ..Default::default()
    };
    client
        .submit_signature(&prepared, signature, &pool)
        .await
        .unwrap();
    let block = fake.state().lock().unwrap().block_hash(101).unwrap();
    fake.mutate_state(|state| state.queue_submit(SubmitScript::FinalityTimeout(block)));
    let report = client
        .submit_signature(
            &prepared,
            signature,
            &SubmitOptions {
                deadline: Some(Duration::from_secs(1)),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(report.success);
    assert_eq!(report.inclusion, Inclusion::Finalized);
    assert_eq!(report.block_number, Some(101));
}

#[tokio::test]
async fn finality_timeout_returns_inclusion_when_finality_was_not_requested() {
    let (client, fake, signer) = fixture().await;
    let prepared = client
        .prepare_call(
            call(&client, vec![]).await,
            &signer,
            &SubmitOptions::default(),
        )
        .await
        .unwrap();
    let signature = signer
        .sign(&prepared.signing_request(rtao_signer::SignPurpose::Extrinsic))
        .await
        .unwrap();
    client
        .submit_signature(
            &prepared,
            signature,
            &SubmitOptions {
                wait: Wait::Pool,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let block = fake.state().lock().unwrap().block_hash(101).unwrap();
    fake.mutate_state(|state| {
        state.finalized = 100;
        state.queue_submit(SubmitScript::FinalityTimeout(block));
    });
    let report = client
        .submit_signature(
            &prepared,
            signature,
            &SubmitOptions {
                wait: Wait::Inclusion,
                deadline: Some(Duration::from_secs(1)),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(report.inclusion, Inclusion::InBlock);
    assert_eq!(report.block_number, Some(101));
    assert!(report.success);
}

#[tokio::test]
async fn dropped_watch_preserves_python_failure_without_reconcile_and_expiry_is_definite() {
    let (client, fake, signer) = fixture().await;
    fake.mutate_state(|state| state.queue_submit(SubmitScript::Dropped));
    assert!(
        matches!(client.submit(call(&client,vec![]).await,&signer,&SubmitOptions::default()).await,Err(SubmitFailure::Pool(PoolRejection::Watch(ref message))) if message=="Extrinsic dropped: {'dropped': None}")
    );
    assert_eq!(
        client.nonces().cached(&signer.account().to_string()).await,
        None
    );
    fake.mutate_state(|state| {
        state.set_handler("author_submitAndWatchExtrinsic", |state, _request| {
            state.advance(10);
            Reply::subscription("expired", vec![json!("dropped")], false)
        })
    });
    let opts = SubmitOptions {
        mortality: Mortality::Mortal(Period::new(4)),
        reconcile: true,
        deadline: Some(Duration::from_secs(1)),
        ..Default::default()
    };
    assert!(matches!(
        client
            .submit(call(&client, vec![]).await, &signer, &opts)
            .await,
        Err(SubmitFailure::Expired { expiry: 104, .. })
    ));
    assert_eq!(
        client.nonces().cached(&signer.account().to_string()).await,
        None
    );
}
