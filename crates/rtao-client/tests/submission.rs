//! Production transaction preparation, lease settlement and watch semantics.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

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
) -> SubmitFailure {
    use blake2::{Blake2b, Digest, digest::consts::U32};

    let state = fake.state();
    let state = state.lock().unwrap();
    assert_eq!(state.submissions.len(), 1);
    let expected = rtao_primitives::H256(Blake2b::<U32>::digest(&state.submissions[0]).into());
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
    assert_eq!(prepared.payload_json()["specVersion"], json!("0x000001e1"));
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
    use rtao_core::{codec::Value, runtime::TypeSpec, value_ext::ValueExt};
    use rtao_primitives::{IntoCoreValue, NetUid};

    let (client, fake, signer) = fixture().await;
    let runtime = client.refresh_runtime().await.unwrap();
    let weights =
        rtao_runtime::calls::subtensor_module::set_weights(NetUid(1), vec![2], vec![65535], 0)
            .compose(&runtime)
            .unwrap();
    let proxy = |real: AccountId, call| {
        rtao_runtime::calls::proxy::proxy(
            rtao_core::DynValue::new(real.into_core_value()),
            None,
            call,
        )
        .compose(&runtime)
        .unwrap()
    };
    let real = AccountId([8; 32]);
    let outer_real = AccountId([9; 32]);
    let ordinary = call(&client, vec![1, 2]).await;
    let nested_proxy = proxy(outer_real, proxy(real, weights.clone()));
    let proxy_batch = proxy(
        outer_real,
        rtao_runtime::calls::utility::batch_all(vec![
            proxy(real, ordinary.clone()),
            proxy(real, weights.clone()),
        ])
        .compose(&runtime)
        .unwrap(),
    );
    let announced = rtao_runtime::calls::proxy::proxy_announced(
        rtao_core::DynValue::new(outer_real.into_core_value()),
        rtao_core::DynValue::new(real.into_core_value()),
        None,
        weights,
    )
    .compose(&runtime)
    .unwrap();
    client.nonces().pin(&signer.account(), 40);
    // The fake supplies independent fees/weights/classes. These are client
    // transport witnesses, not a simulation of runtime opt-ins or account debits.
    for (index, (call, class)) in [
        (ordinary, "Normal"),
        (nested_proxy, "Operational"),
        (proxy_batch, "Mandatory"),
        (announced, "Normal"),
    ]
    .into_iter()
    .enumerate()
    {
        let nonce = 7 + u64::try_from(index).unwrap();
        let fee = 1_000 + u64::try_from(index).unwrap();
        let weight = json!({"ref_time": 42 + index, "proof_size": 9 + index});
        fake.mutate_state(|state| {
            state.set_nonce(signer.account().to_string(), nonce);
            state
                .seed_runtime(
                    "TransactionPaymentApi",
                    "query_info",
                    json!({
                        "weight":weight, "class":class, "partial_fee":fee,
                    }),
                )
                .unwrap();
        });
        let first_request = fake.requests().len();
        let estimate = client
            .estimate_weight(call.clone(), signer.account(), signer.crypto_type())
            .await
            .unwrap();
        assert_eq!(estimate.partial_fee, fee);
        assert_eq!(
            rtao_core::codec::value::to_corpus_json(&estimate.weight),
            weight
        );
        assert_eq!(estimate.class.as_str(), Some(class));
        assert_eq!(
            client.nonces().cached(&signer.account().to_string()).await,
            Some(40)
        );
        let requests = fake.requests();
        let requests = &requests[first_request..];
        let fee_requests = requests
            .iter()
            .filter(|request| {
                request.method == "state_call"
                    && request.params[0] == "TransactionPaymentApi_query_info"
            })
            .collect::<Vec<_>>();
        assert_eq!(fee_requests.len(), 1);
        let request = fee_requests[0];
        assert!(request.params[2].is_null());
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
        let decoded = runtime
            .core()
            .decode_extrinsic(&args[..encoded_len], true)
            .unwrap();
        let expected_call = runtime
            .core()
            .decode_spec(&TypeSpec::Call, call.as_bytes(), true)
            .unwrap();
        assert_eq!(decoded.field("call"), Some(&expected_call));
        assert_eq!(
            decoded.field("address").and_then(ValueExt::as_str),
            Some(signer.account().to_string().as_str())
        );
        assert_eq!(
            decoded.field("nonce").and_then(ValueExt::as_u128),
            Some(u128::from(nonce))
        );
        assert_eq!(decoded.field("era").and_then(ValueExt::as_str), Some("00"));
        assert_eq!(decoded.field("tip").and_then(ValueExt::as_u128), Some(0));
        assert!(decoded.field("asset_id").is_none());
        assert_eq!(
            decoded
                .field("mode")
                .and_then(|mode| mode.field("mode"))
                .and_then(ValueExt::as_str),
            Some("Disabled")
        );
        assert_eq!(
            decoded
                .field("signature")
                .and_then(|signature| signature.variant("Ed25519"))
                .and_then(Value::value_bytes),
            Some(vec![0; 64])
        );
        assert_eq!(
            requests
                .iter()
                .filter(|request| request.method == "account_nextIndex")
                .count(),
            1
        );
        assert!(
            !requests
                .iter()
                .any(|request| request.method.starts_with("author_")
                    || request.method == "payment_queryInfo"
                    || request.method.starts_with("state_getStorage"))
        );
    }
    assert!(fake.state().lock().unwrap().submissions.is_empty());
    client.close().await;
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
    constants: Option<(u8, String)>,
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
    fn metadata_constants(&self) -> Option<(u8, String)> {
        self.constants.clone()
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
    let signer = MetadataSigner {
        signer,
        constants: None,
    };
    fake.mutate_state(|state| {
        state.set_result(
            "state_getRuntimeVersion",
            json!({"specName":"runtime-renamed","specVersion":481,"transactionVersion":1,"apis":[]}),
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
async fn signer_display_constants_define_the_signed_digest_through_arc_delegation() {
    let (client, _, signer) = fixture().await;
    let raw = MetadataSigner {
        signer: signer.clone(),
        constants: None,
    };
    let device = Arc::new(MetadataSigner {
        signer,
        constants: Some((12, "DEVICE".into())),
    });
    let call = call(&client, vec![1]).await;
    let fallback = client
        .prepare_call(call.clone(), &raw, &SubmitOptions::default())
        .await
        .unwrap();
    let custom = client
        .prepare_call(call.clone(), &device, &SubmitOptions::default())
        .await
        .unwrap();
    let info = &custom.context().chain_info;
    assert_eq!(info.decimals, 12);
    assert_eq!(info.token_symbol, "DEVICE");
    assert_eq!(info.base58_prefix, 42);
    assert_eq!(info.spec_name, fallback.context().chain_info.spec_name);
    assert_eq!(
        info.spec_version,
        fallback.context().chain_info.spec_version
    );
    assert_eq!(
        custom.metadata_hash(),
        Some(rtao_core::digest::metadata_digest(&custom.context().metadata_bytes, info).unwrap()),
    );
    assert_ne!(custom.metadata_hash(), fallback.metadata_hash());
    assert_ne!(custom.payload(), fallback.payload());
    let again = client
        .prepare_call(call, &raw, &SubmitOptions::default())
        .await
        .unwrap();
    assert_eq!(again.metadata_hash(), fallback.metadata_hash());
    assert_eq!(again.payload(), fallback.payload());
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
            let accepted = Arc::new(AtomicBool::new(false));
            let author_accepted = accepted.clone();
            fake.mutate_state(|state| {
                state.set_handler(method, move |state, request| {
                    // Preserve node-side acceptance while withholding its acknowledgment.
                    let reply = state.dispatch(request);
                    assert!(matches!(reply.outcome, Some(Ok(_))));
                    author_accepted.store(true, Ordering::SeqCst);
                    Reply::silent()
                });
            });
            tokio::time::pause();
            let opts = SubmitOptions {
                wait,
                submit_deadline: Some(Duration::from_millis(50)),
                deadline: Some(Duration::from_secs(1)),
                ..Default::default()
            };
            let started = tokio::time::Instant::now();
            let operation = async {
                if external_signature {
                    client.submit_signature(&prepared, signature, &opts).await
                } else {
                    client.submit(call, &signer, &opts).await
                }
            };
            tokio::pin!(operation);
            // Keep the runtime runnable while preparation finishes. Paused
            // time must not auto-advance during an offloaded preparation step.
            let wall_started = std::time::Instant::now();
            while !accepted.load(Ordering::SeqCst) {
                if let std::task::Poll::Ready(result) = futures::poll!(&mut operation) {
                    panic!("submission finished before author acceptance: {result:?}");
                }
                assert!(
                    wall_started.elapsed() < Duration::from_secs(10),
                    "fixture never reached author acceptance"
                );
                tokio::task::yield_now().await;
            }
            tokio::time::advance(Duration::from_millis(49)).await;
            assert!(futures::poll!(&mut operation).is_pending());
            tokio::time::advance(Duration::from_millis(2)).await;
            assert_exact_unknown_fate(operation.await, &fake);
            assert_eq!(started.elapsed(), Duration::from_millis(51));
            assert_eq!(fake.state().lock().unwrap().submissions.len(), 1);
            assert_eq!(
                client.nonces().cached(&signer.account().to_string()).await,
                None
            );
            client.close().await;
            tokio::time::resume();
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
                // Only the 50 ms admission deadline is under test. A loaded
                // hosted runner can need more than a second to reconcile.
                deadline: Some(Duration::from_secs(30)),
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
async fn receipt_rpc_protocol_and_event_decode_failures_keep_hash_and_cause_for_all_entries() {
    for entry in 0..3 {
        for fault in 0..3 {
            let (client, fake, signer) = fixture().await;
            let call = call(&client, vec![]).await;
            let runtime = client.refresh_runtime().await.unwrap();
            let events_key = json!(format!(
                "0x{}",
                hex::encode(
                    runtime
                        .core()
                        .storage_key(runtime.storage_entry("System", "Events").unwrap(), &[])
                        .unwrap()
                )
            ));
            fake.mutate_state(|state| match fault {
                0 => state.set_handler("chain_getBlock", |_, _| {
                    Reply::error(-32603, "receipt RPC sentinel", None)
                }),
                1 => state.set_handler("chain_getBlock", |_, _| {
                    Reply::result(json!({"block":{"header":{},"extrinsics":[]}}))
                }),
                _ => state.set_handler("state_getStorage", move |state, request| {
                    if state.submissions.is_empty() || request.params[0] != events_key {
                        state.dispatch(request)
                    } else {
                        // SCALE count=1 followed by a missing EventRecord.
                        Reply::result(json!("0x04"))
                    }
                }),
            });
            let error = assert_exact_unknown_fate(
                submit_entry(&client, call, &signer, &SubmitOptions::default(), entry).await,
                &fake,
            );
            let SubmitFailure::Unresolved {
                cause,
                reconciliation,
                ..
            } = error
            else {
                panic!("receipt failure lost its cause");
            };
            assert!(matches!(cause.as_ref(), SubmitFailure::Client(_)));
            assert!(reconciliation.is_none());
            if fault == 0 {
                assert!(cause.to_string().contains("receipt RPC sentinel"));
            } else if fault == 2 {
                assert!(matches!(cause.as_ref(), SubmitFailure::Client(
                    rtao_client::ClientError::Decode { what, .. }
                ) if what.contains("System.Events")));
            }
            assert_eq!(
                client.nonces().cached(&signer.account().to_string()).await,
                None
            );
        }
    }
}

#[tokio::test]
async fn malformed_watch_and_wrong_pool_acknowledgement_keep_attempted_identity() {
    for entry in 0..3 {
        for status in [
            json!({"ready":null,"finalized":null}),
            json!({"finalized":"not-a-block-hash"}),
            json!({"unrecognized":null}),
        ] {
            let (client, fake, signer) = fixture().await;
            fake.mutate_state(|state| state.queue_submit(SubmitScript::Statuses(vec![status])));
            let error = assert_exact_unknown_fate(
                submit_entry(
                    &client,
                    call(&client, vec![]).await,
                    &signer,
                    &SubmitOptions::default(),
                    entry,
                )
                .await,
                &fake,
            );
            assert!(matches!(error, SubmitFailure::Unresolved { ref cause, .. }
                if matches!(cause.as_ref(), SubmitFailure::Client(rtao_client::ClientError::Protocol(_)))));
            assert_eq!(
                client.nonces().cached(&signer.account().to_string()).await,
                None
            );
        }
        let (client, fake, signer) = fixture().await;
        fake.mutate_state(|state| {
            state.set_handler("author_submitExtrinsic", |state, request| {
                let _accepted = state.dispatch(request);
                Reply::result(json!(format!("0x{}", "77".repeat(32))))
            });
        });
        let error = assert_exact_unknown_fate(
            submit_entry(
                &client,
                call(&client, vec![]).await,
                &signer,
                &SubmitOptions {
                    wait: Wait::Pool,
                    ..Default::default()
                },
                entry,
            )
            .await,
            &fake,
        );
        assert!(error.to_string().contains("different extrinsic hash"));
        assert_eq!(
            client.nonces().cached(&signer.account().to_string()).await,
            None
        );
    }
}

#[tokio::test]
async fn transient_receipt_rpc_failure_reconciles_without_rebroadcast_or_a_reserved_nonce() {
    use futures::FutureExt;

    for entry in 0..3 {
        let (client, fake, signer) = fixture().await;
        let attempts = Arc::new(AtomicUsize::new(0));
        let account = signer.account().to_string();
        let mut nonces = Some(client.nonces().clone());
        fake.mutate_state(|state| {
            let attempts = attempts.clone();
            state.set_handler("chain_getBlock", move |state, request| {
                if attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                    return Reply::error(-32603, "transient receipt RPC sentinel", None);
                }
                if let Some(nonces) = nonces.take() {
                    assert_eq!(nonces.cached(&account).now_or_never(), Some(None));
                }
                state.dispatch(request)
            });
        });
        let report = submit_entry(
            &client,
            call(&client, vec![]).await,
            &signer,
            &SubmitOptions {
                reconcile: true,
                deadline: Some(Duration::from_secs(1)),
                ..Default::default()
            },
            entry,
        )
        .await
        .unwrap();
        assert!(report.success);
        assert_eq!(report.inclusion, Inclusion::Finalized);
        assert!(attempts.load(Ordering::SeqCst) >= 3);
        assert_eq!(fake.state().lock().unwrap().submissions.len(), 1);
        assert_eq!(
            fake.requests()
                .iter()
                .filter(|request| request.method.starts_with("author_submit"))
                .count(),
            1
        );
        assert_eq!(
            client.nonces().cached(&signer.account().to_string()).await,
            None
        );
    }
}

#[tokio::test]
async fn reconciliation_rpc_failure_or_deadline_retains_the_original_receipt_failure() {
    for entry in 0..3 {
        for silent in [false, true] {
            let (client, fake, signer) = fixture().await;
            fake.mutate_state(|state| {
                state.set_handler("chain_getBlock", |_, _| {
                    Reply::error(-32603, "original receipt sentinel", None)
                });
                state.set_handler("chain_getFinalizedHead", move |state, request| {
                    if state.submissions.is_empty() {
                        state.dispatch(request)
                    } else if silent {
                        Reply::silent()
                    } else {
                        Reply::error(-32603, "reconciliation RPC sentinel", None)
                    }
                });
            });
            let error = assert_exact_unknown_fate(
                submit_entry(
                    &client,
                    call(&client, vec![]).await,
                    &signer,
                    &SubmitOptions {
                        reconcile: true,
                        deadline: Some(Duration::from_millis(200)),
                        ..Default::default()
                    },
                    entry,
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
                panic!("reconciliation discarded one of its causes");
            };
            assert!(cause.to_string().contains("original receipt sentinel"));
            if silent {
                assert!(matches!(
                    reconciliation.as_ref(),
                    SubmitFailure::Client(rtao_client::ClientError::Rpc(
                        rtao_rpc::RpcError::Timeout { .. }
                    ))
                ));
            } else {
                assert!(
                    reconciliation
                        .to_string()
                        .contains("reconciliation RPC sentinel")
                );
            }
            assert_eq!(
                client.nonces().cached(&signer.account().to_string()).await,
                None
            );
        }
    }
}

#[tokio::test]
async fn author_internal_error_is_unresolved_but_exact_pool_refusal_remains_definite() {
    for entry in 0..3 {
        let (client, fake, signer) = fixture().await;
        fake.mutate_state(|state| {
            state.set_handler("author_submitAndWatchExtrinsic", |state, request| {
                let _accepted = state.dispatch(request);
                Reply::error(-32603, "author response failed after acceptance", None)
            });
        });
        let error = assert_exact_unknown_fate(
            submit_entry(
                &client,
                call(&client, vec![]).await,
                &signer,
                &SubmitOptions::default(),
                entry,
            )
            .await,
            &fake,
        );
        assert!(
            error
                .to_string()
                .contains("author response failed after acceptance")
        );
        assert_eq!(
            client.nonces().cached(&signer.account().to_string()).await,
            None
        );

        let (client, fake, signer) = fixture().await;
        fake.mutate_state(|state| {
            state.queue_reply(
                "author_submitAndWatchExtrinsic",
                Reply::error(1010, "opaque invalid-transaction refusal", None),
            );
        });
        let error = submit_entry(
            &client,
            call(&client, vec![]).await,
            &signer,
            &SubmitOptions {
                reconcile: true,
                ..Default::default()
            },
            entry,
        )
        .await
        .unwrap_err();
        assert!(matches!(
            error,
            SubmitFailure::Pool(PoolRejection::Unknown(_))
        ));
        assert!(error.unknown_fate_hash().is_none());
        assert!(fake.state().lock().unwrap().submissions.is_empty());
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
            json!({"specName":"node-subtensor","specVersion":481,"transactionVersion":2,"apis":[]}),
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
    let (first, replacement) = fake.mutate_state(|state| {
        state.finalized = 99;
        let first = state.block_hash(100).unwrap();
        let parent = state.block_hash(99).unwrap();
        let replacement = state.fork(&parent).unwrap();
        assert_ne!(first, replacement);
        state.set_best(&replacement).unwrap();
        state.set_finalized(100).unwrap();
        // Only B contains the successful receipt. Resolving the retracted A
        // would therefore fail the outcome and block identity assertions.
        state.seed_events(first.clone(), json!([])).unwrap();
        state
            .seed_events(
                replacement.clone(),
                json!([{"phase":"ApplyExtrinsic","extrinsic_idx":0,"module_id":"System","event_id":"ExtrinsicSuccess","attributes":{"dispatch_info":{"weight":{"ref_time":1,"proof_size":0},"class":"Normal","pays_fee":"Yes"}},"topics":[]}]),
            )
            .unwrap();
        state.queue_submit(SubmitScript::Statuses(vec![
            json!("ready"),
            json!({"inBlock":first}),
            json!({"retracted":first}),
            json!({"inBlock":replacement}),
            json!({"finalized":replacement}),
        ]));
        (first, replacement)
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
    let replacement_hash =
        rtao_primitives::H256(hex::decode(&replacement[2..]).unwrap().try_into().unwrap());
    assert_eq!(report.block_hash, Some(replacement_hash));
    assert_eq!(report.block_number, Some(100));
    assert_eq!(report.extrinsic_index, Some(0));
    assert_ne!(first, replacement);
    assert_eq!(fake.state().lock().unwrap().submissions.len(), 1);
    assert_eq!(
        client.nonces().cached(&signer.account().to_string()).await,
        Some(0)
    );
    let next = client.nonces().lease(&signer.account()).await.unwrap();
    assert_eq!(next.value(), 1);
    next.settle();
    client.close().await;
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
