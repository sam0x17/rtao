//! A separate submission endpoint carries nonce, fee, submission and receipt
//! calls; every other read stays on the main endpoint.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use rtao_client::{
    BlockRef, Client, ClientConfig, Inclusion, Network, SubmitOptions, ViewExt, Wait,
};
use rtao_signer::{CryptoType, KeypairSigner, Signer};
use rtao_testkit::{FakeChain, FakeState};
use subxt_rpcs::RpcClient;

fn config() -> ClientConfig {
    let mut cfg = ClientConfig::new(Network::resolve_with("local", |_| None).unwrap()).unwrap();
    cfg.track_heads = false;
    cfg.runtime_cache_dir = None;
    cfg.config_store = None;
    cfg
}

fn hash_hex(hash: rtao_primitives::H256) -> String {
    format!("0x{}", hex::encode(hash.0))
}

fn signer() -> KeypairSigner {
    KeypairSigner::from_seed(&[7; 32], CryptoType::Ed25519).unwrap()
}

/// Recorded methods from `from` on, with the runtime API named for state calls.
fn methods(fake: &FakeChain, from: usize) -> Vec<String> {
    fake.requests()
        .iter()
        .skip(from)
        .map(|request| match request.method.as_str() {
            "state_call" => format!(
                "state_call:{}",
                request.params[0].as_str().unwrap_or_default()
            ),
            method => method.to_owned(),
        })
        .collect()
}

fn is_submission(method: &str) -> bool {
    method == "account_nextIndex"
        || method == "system_accountNextIndex"
        || method.starts_with("state_call:TransactionPaymentApi")
        || method.starts_with("author_")
}

async fn remark(client: &Client, data: Vec<u8>) -> rtao_runtime::CallBytes {
    rtao_runtime::calls::system::remark(data)
        .compose(&client.refresh_runtime().await.unwrap())
        .unwrap()
}

async fn split() -> (Client, FakeChain, FakeChain) {
    let read = FakeChain::from_state(FakeState::default());
    let node = FakeChain::from_state(FakeState::default());
    let client = Client::from_rpc_with_submit(
        RpcClient::new(read.clone()),
        RpcClient::new(node.clone()),
        config(),
    )
    .await
    .unwrap();
    (client, read, node)
}

#[tokio::test]
async fn reads_alone_never_open_the_submit_endpoint() {
    let (client, read, node) = split().await;
    assert!(client.has_submit_endpoint());
    let before = read.requests().len();
    client.at(BlockRef::Finalized).await.unwrap();
    assert!(read.requests().len() > before);
    assert!(node.requests().is_empty(), "{:?}", methods(&node, 0));
    client.close().await;
}

#[tokio::test]
async fn nonce_fee_submission_and_receipt_use_the_submit_endpoint() {
    let (client, read, node) = split().await;
    let signer = signer();
    let account = signer.account();
    // Only the submitting node knows the pool-aware nonce.
    node.mutate_state(|state| state.set_nonce(account.to_string(), 7));
    let call = remark(&client, vec![1]).await;
    let read_start = read.requests().len();

    // Fee: fresh nonce and the payment runtime API come from the node.
    client
        .estimate_weight(call.clone(), account, CryptoType::Ed25519)
        .await
        .unwrap();
    let node_fee = methods(&node, 0);
    assert!(node_fee.contains(&"account_nextIndex".to_owned()));
    assert!(node_fee.contains(&"state_call:TransactionPaymentApi_query_info".to_owned()));

    // Submission with a receipt at the node's (newer) inclusion block. The
    // read endpoint has not seen that block, so a receipt read there fails.
    let node_start = node.requests().len();
    let report = client
        .submit(call, &signer, &SubmitOptions::default())
        .await
        .unwrap();
    assert!(report.success);
    assert_eq!(report.inclusion, Inclusion::Finalized);
    let block = report.block_hash.unwrap();
    assert_eq!(
        node.state()
            .lock()
            .unwrap()
            .block_hash(report.block_number.unwrap()),
        Some(hash_hex(block))
    );
    assert!(
        read.state()
            .lock()
            .unwrap()
            .header(&hash_hex(block))
            .is_none()
    );
    // The node's nonce was leased and settled.
    assert_eq!(client.nonces().cached(&account.to_string()).await, Some(7));
    assert_eq!(node.state().lock().unwrap().submissions.len(), 1);
    assert!(read.state().lock().unwrap().submissions.is_empty());

    let node_submit = methods(&node, node_start);
    for expected in [
        "account_nextIndex",
        "author_submitAndWatchExtrinsic",
        "chain_getBlock",
    ] {
        assert!(
            node_submit.iter().any(|method| method == expected),
            "{expected} not sent to the submit endpoint: {node_submit:?}"
        );
    }
    // The receipt's block and its events are read at the inclusion hash.
    let block_hex = hash_hex(block);
    assert!(
        node.requests().iter().skip(node_start).any(|request| {
            request.method == "chain_getHeader" && request.params[0] == block_hex
        })
    );
    assert!(node.requests().iter().skip(node_start).any(|request| {
        request.method.starts_with("state_")
            && request.method != "state_call"
            && request
                .params
                .as_array()
                .is_some_and(|params| params.last().is_some_and(|last| *last == block_hex))
    }));

    // Everything else, including preparation reads, stays on the main endpoint.
    let reads = methods(&read, read_start);
    assert!(
        reads.iter().all(|method| !is_submission(method)),
        "{reads:?}"
    );
    assert!(
        reads.iter().all(|method| method != "chain_getBlock"),
        "{reads:?}"
    );
    assert!(reads.contains(&"chain_getFinalizedHead".to_owned()));
    assert!(reads.contains(&"state_getRuntimeVersion".to_owned()));
    assert!(
        !read
            .requests()
            .iter()
            .skip(read_start)
            .any(|request| request
                .params
                .as_array()
                .is_some_and(|params| params.iter().any(|param| *param == block_hex))),
        "no main-endpoint read names the inclusion block"
    );
    client.close().await;
}

/// Follow-up reads at a receipt's block work while the read endpoint lags.
#[tokio::test]
async fn receipt_reads_fall_back_to_the_submit_endpoint() {
    let (client, read, node) = split().await;
    let signer = signer();
    let report = client
        .submit(
            remark(&client, vec![5]).await,
            &signer,
            &SubmitOptions::default(),
        )
        .await
        .unwrap();
    let block = report.block_hash.unwrap();
    // The read endpoint has not imported the inclusion block.
    assert!(matches!(
        client.at(BlockRef::Hash(block)).await,
        Err(rtao_client::ClientError::BlockNotFound(_))
    ));
    let snapshot = client.at_receipt(block).await.unwrap();
    assert_eq!(snapshot.block().hash, block);
    assert_eq!(Some(snapshot.block().number), report.block_number);
    // The receipt connection is one coherent view of the reporting chain.
    let chain = client.receipt_client().await.unwrap();
    let finalized = chain.at(BlockRef::Finalized).await.unwrap();
    assert!(finalized.block().number >= snapshot.block().number);
    assert_eq!(
        chain
            .at(BlockRef::Number(snapshot.block().number))
            .await
            .unwrap()
            .block()
            .hash,
        block
    );
    // A block the read endpoint has is read there.
    let read_head = client.at(BlockRef::Finalized).await.unwrap().block().hash;
    let node_before = node.requests().len();
    let read_before = read.requests().len();
    assert_eq!(
        client.at_receipt(read_head).await.unwrap().block().hash,
        read_head
    );
    assert_eq!(node.requests().len(), node_before);
    assert!(read.requests().len() > read_before);
    client.close().await;
}

/// Without a submit endpoint the receipt helpers are the plain reads.
#[tokio::test]
async fn receipt_reads_without_a_submit_endpoint_stay_on_the_client() {
    let fake = FakeChain::from_state(FakeState::default());
    let client = Client::from_rpc(RpcClient::new(fake.clone()), config())
        .await
        .unwrap();
    let head = client.at(BlockRef::Finalized).await.unwrap().block().hash;
    let start = fake.requests().len();
    client.at(BlockRef::Hash(head)).await.unwrap();
    let plain = methods(&fake, start);
    let start = fake.requests().len();
    client.at_receipt(head).await.unwrap();
    assert_eq!(methods(&fake, start), plain);
    let missing = rtao_primitives::H256([9; 32]);
    assert!(matches!(
        client.at_receipt(missing).await,
        Err(rtao_client::ClientError::BlockNotFound(_))
    ));
    let start = fake.requests().len();
    client.receipt_client().await.unwrap();
    assert_eq!(fake.requests().len(), start);
    client.close().await;
}

#[tokio::test]
async fn pool_submission_and_raw_bytes_use_the_submit_endpoint() {
    let (client, read, node) = split().await;
    let signer = signer();
    let read_start = read.requests().len();
    let opts = SubmitOptions {
        wait: Wait::Pool,
        ..Default::default()
    };
    let report = client
        .submit(remark(&client, vec![2]).await, &signer, &opts)
        .await
        .unwrap();
    assert_eq!(report.inclusion, Inclusion::PoolAccepted);
    let prepared = client
        .prepare_call(
            remark(&client, vec![3]).await,
            &signer,
            &SubmitOptions::default(),
        )
        .await
        .unwrap();
    let signature = signer
        .sign(&prepared.signing_request(rtao_signer::SignPurpose::Extrinsic))
        .await
        .unwrap();
    let (bytes, hash) = client.attach_signature(&prepared, signature).await.unwrap();
    assert_eq!(
        client
            .submit_signed_raw(&bytes, &signer.account().to_string())
            .await
            .unwrap(),
        hash
    );
    let node_methods = methods(&node, 0);
    assert_eq!(
        node_methods
            .iter()
            .filter(|method| *method == "author_submitExtrinsic")
            .count(),
        2
    );
    let reads = methods(&read, read_start);
    assert!(
        reads.iter().all(|method| !is_submission(method)),
        "{reads:?}"
    );
    client.close().await;
}

#[tokio::test]
async fn a_submit_endpoint_on_another_chain_is_refused_before_any_submission() {
    let read = FakeChain::from_state(FakeState::default());
    let node = FakeChain::from_state(FakeState::default());
    node.mutate_state(|state| state.genesis_hash = format!("0x{}", "11".repeat(32)));
    let client = Client::from_rpc_with_submit(
        RpcClient::new(read.clone()),
        RpcClient::new(node.clone()),
        config(),
    )
    .await
    .unwrap();
    let signer = signer();
    let error = client
        .submit(
            remark(&client, vec![4]).await,
            &signer,
            &SubmitOptions::default(),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("different chain"), "{error}");
    assert!(
        !node
            .requests()
            .iter()
            .any(|request| request.method.starts_with("author_"))
    );
    assert!(node.state().lock().unwrap().submissions.is_empty());
    client.close().await;
}

/// Drive every submission entry point over one fake chain and return the
/// recorded methods with a digest of each full request (method and params).
async fn default_flow(client: &Client, fake: &FakeChain) -> (Vec<String>, String) {
    use blake2::{Blake2b, Digest, digest::consts::U32};

    let signer = signer();
    fake.mutate_state(|state| state.set_nonce(signer.account().to_string(), 3));
    let call = remark(client, vec![1]).await;
    let start = fake.requests().len();
    client
        .estimate_weight(call.clone(), signer.account(), CryptoType::Ed25519)
        .await
        .unwrap();
    let report = client
        .submit(call, &signer, &SubmitOptions::default())
        .await
        .unwrap();
    assert_eq!(report.inclusion, Inclusion::Finalized);
    client
        .resolve_extrinsic(report.hash, report.block_hash.unwrap(), true)
        .await
        .unwrap();
    let pool = SubmitOptions {
        wait: Wait::Pool,
        ..Default::default()
    };
    client
        .submit(remark(client, vec![2]).await, &signer, &pool)
        .await
        .unwrap();
    let prepared = client
        .prepare_call(
            remark(client, vec![3]).await,
            &signer,
            &SubmitOptions::default(),
        )
        .await
        .unwrap();
    let signature = signer
        .sign(&prepared.signing_request(rtao_signer::SignPurpose::Extrinsic))
        .await
        .unwrap();
    let (bytes, _) = client.attach_signature(&prepared, signature).await.unwrap();
    client
        .submit_signed_raw(&bytes, &signer.account().to_string())
        .await
        .unwrap();
    let mut digest = Blake2b::<U32>::new();
    for request in fake.requests().iter().skip(start) {
        digest.update(request.method.as_bytes());
        digest.update(request.params.to_string().as_bytes());
    }
    (methods(fake, start), hex::encode(digest.finalize()))
}

/// The default configuration has no submit endpoint and issues exactly the
/// requests it issued before submit endpoints existed.
#[tokio::test]
async fn without_a_submit_endpoint_requests_are_unchanged() {
    let fake = FakeChain::from_state(FakeState::default());
    let client = Client::from_rpc(RpcClient::new(fake.clone()), config())
        .await
        .unwrap();
    assert!(!client.has_submit_endpoint());
    let (observed, digest) = default_flow(&client, &fake).await;
    assert_eq!(observed, DEFAULT_METHODS, "observed: {observed:?}");
    assert_eq!(digest, DEFAULT_DIGEST);
    client.close().await;
}

/// Recorded from the client before submit endpoints were added.
const DEFAULT_METHODS: &[&str] = &[
    "state_getRuntimeVersion",
    "account_nextIndex",
    "state_call:TransactionPaymentApi_query_info",
    "account_nextIndex",
    "state_getRuntimeVersion",
    "chain_getFinalizedHead",
    "chain_getHeader",
    "chain_getHeader",
    "chain_getFinalizedHead",
    "chain_getHeader",
    "chain_getBlockHash",
    "chain_getHeader",
    "state_getRuntimeVersion",
    "state_getRuntimeVersion",
    "state_getRuntimeVersion",
    "chain_getFinalizedHead",
    "chain_getHeader",
    "author_submitAndWatchExtrinsic",
    "chain_getBlock",
    "chain_getHeader",
    "state_getRuntimeVersion",
    "state_getStorage",
    "author_unwatchExtrinsic",
    "chain_getBlock",
    "chain_getHeader",
    "state_getStorage",
    "state_getRuntimeVersion",
    "state_getRuntimeVersion",
    "chain_getFinalizedHead",
    "chain_getHeader",
    "chain_getHeader",
    "chain_getBlockHash",
    "chain_getHeader",
    "state_getRuntimeVersion",
    "state_getRuntimeVersion",
    "state_getRuntimeVersion",
    "chain_getFinalizedHead",
    "chain_getHeader",
    "author_submitExtrinsic",
    "state_getRuntimeVersion",
    "account_nextIndex",
    "state_getRuntimeVersion",
    "chain_getFinalizedHead",
    "chain_getHeader",
    "state_getRuntimeVersion",
    "chain_getHeader",
    "chain_getBlockHash",
    "chain_getHeader",
    "state_getRuntimeVersion",
    "state_getRuntimeVersion",
    "state_getRuntimeVersion",
    "chain_getFinalizedHead",
    "chain_getHeader",
    "author_submitExtrinsic",
];
const DEFAULT_DIGEST: &str = "e592e43c23836c3d9a37a27ff78cb227a7d96a360b3cfed7fd9cbdd07183d298";

/// An encoded 1184-byte ML-KEM-768 encapsulation key accepted by the core.
fn shield_key(fill: u8) -> Vec<u8> {
    vec![fill; 1184]
}

fn seed_shield(state: &mut FakeState, key: Vec<u8>) {
    state
        .seed_constant("Aura", "SlotDuration", serde_json::json!(1))
        .unwrap();
    state.seed_typed("MevShield", "NextKey", (), key).unwrap();
}

/// Drive a pooled shielded submission and return the recorded methods with a
/// digest of each request. The carrier's ciphertext is randomized, so author
/// requests contribute their method alone.
async fn shield_flow(client: &Client, fake: &FakeChain) -> (Vec<String>, String) {
    use blake2::{Blake2b, Digest, digest::consts::U32};

    let signer = signer();
    fake.mutate_state(|state| state.set_nonce(signer.account().to_string(), 5));
    let call = remark(client, vec![6]).await;
    let start = fake.requests().len();
    let opts = rtao_client::ShieldOptions {
        wait: Wait::Pool,
        ..Default::default()
    };
    let report = client.submit_shielded(call, &signer, &opts).await.unwrap();
    assert_eq!(report.inclusion, Inclusion::PoolAccepted);
    let mut digest = Blake2b::<U32>::new();
    for request in fake.requests().iter().skip(start) {
        digest.update(request.method.as_bytes());
        if !request.method.starts_with("author_") {
            digest.update(request.params.to_string().as_bytes());
        }
    }
    (methods(fake, start), hex::encode(digest.finalize()))
}

/// Without a submit endpoint a shielded submission issues exactly the
/// requests it issued before submit endpoints existed, apart from the
/// deliberate NextKey change explained at `SHIELD_DIGEST`.
#[tokio::test]
async fn without_a_submit_endpoint_shield_requests_are_unchanged() {
    let mut state = FakeState::default();
    seed_shield(&mut state, shield_key(0));
    let fake = FakeChain::from_state(state);
    let client = Client::from_rpc(RpcClient::new(fake.clone()), config())
        .await
        .unwrap();
    let (observed, digest) = shield_flow(&client, &fake).await;
    assert_eq!(digest, SHIELD_DIGEST, "observed: {observed:?}");
    client.close().await;
}

/// Recorded from the client before shield reads followed the submit endpoint,
/// then updated once when NextKey became an unpinned read: each of the two
/// `chain_getHeader` and `state_queryStorageAt(key, best)` pairs became one
/// `state_getStorage(key)`. Every other request is unchanged.
const SHIELD_DIGEST: &str = "477f8d0746e803fe1a3b7131a68295f5a933c43c8aea2e499f2756caa593b9ce";

/// A read endpoint behind the submitting node, with its own NextKey.
async fn shield_split(
    read_key: Vec<u8>,
    node_key: Vec<u8>,
    node_ahead: u64,
) -> (Client, FakeChain, FakeChain) {
    let mut read = FakeState::default();
    seed_shield(&mut read, read_key);
    let mut node = FakeState::default();
    seed_shield(&mut node, node_key);
    node.advance(node_ahead);
    let read = FakeChain::from_state(read);
    let node = FakeChain::from_state(node);
    let client = Client::from_rpc_with_submit(
        RpcClient::new(read.clone()),
        RpcClient::new(node.clone()),
        config(),
    )
    .await
    .unwrap();
    (client, read, node)
}

/// NextKey rotates every block: the carrier is sealed to the submitting node's
/// key, never to the stale key a lagging read endpoint still serves.
#[tokio::test]
async fn shield_seals_to_the_submit_endpoint_key_when_the_read_endpoint_lags() {
    let stale = shield_key(0);
    let current = shield_key(1);
    let (client, read, node) = shield_split(stale.clone(), current.clone(), 2).await;
    let signer = signer();
    let opts = rtao_client::ShieldOptions {
        wait: Wait::Pool,
        ..Default::default()
    };
    let read_start = read.requests().len();
    let report = client
        .submit_shielded(remark(&client, vec![8]).await, &signer, &opts)
        .await
        .unwrap();
    assert_eq!(report.inclusion, Inclusion::PoolAccepted);
    let submissions = node.state().lock().unwrap().submissions.clone();
    assert_eq!(submissions.len(), 1);
    assert!(read.state().lock().unwrap().submissions.is_empty());
    assert!(
        sealed_to(&submissions[0], &current),
        "carrier is not sealed to the submit endpoint's key"
    );
    assert!(
        !sealed_to(&submissions[0], &stale),
        "carrier is sealed to the read endpoint's stale key"
    );
    // The key and the era guard's best height come from the node; the read
    // endpoint is never asked for MevShield storage.
    assert!(!next_key_reads(&node, 0).is_empty());
    assert!(
        next_key_reads(&read, read_start).is_empty(),
        "{:?}",
        methods(&read, read_start)
    );
    client.close().await;
}

/// The hex storage key of the plain `MevShield.NextKey` value.
fn next_key_storage_key() -> String {
    let mut key = rtao_core::mlkem::twox_128(b"MevShield").to_vec();
    key.extend_from_slice(&rtao_core::mlkem::twox_128(b"NextKey"));
    format!("0x{}", hex::encode(key))
}

/// Recorded storage requests, from `from` on, that name `MevShield.NextKey`;
/// asserts each is an unpinned single-key read of the newest state.
fn next_key_reads(fake: &FakeChain, from: usize) -> Vec<rtao_testkit::RpcRequest> {
    let key = next_key_storage_key();
    let reads: Vec<_> = fake
        .requests()
        .into_iter()
        .skip(from)
        .filter(|request| {
            request.method.starts_with("state_") && request.params.to_string().contains(&key)
        })
        .collect();
    for request in &reads {
        assert_eq!(request.method, "state_getStorage", "{request:?}");
        assert_eq!(request.params, serde_json::json!([key]), "{request:?}");
    }
    reads
}

/// Whether a submitted carrier is sealed to `key`: its envelope carries the
/// key's hash.
fn sealed_to(carrier: &[u8], key: &[u8]) -> bool {
    let hash = rtao_core::mlkem::twox_128(key);
    carrier.windows(hash.len()).any(|window| window == hash)
}

/// An endpoint whose newest state is one block past the header it reports as
/// best: it serves headers from its finalized view but answers storage reads
/// without a block hash from its latest state. NextKey rotated in that newest
/// block, so the header's state holds a key that has already expired.
fn trailing_headers_with_rotated_key(expired: Vec<u8>, current: Vec<u8>) -> FakeState {
    let mut state = FakeState::default();
    seed_shield(&mut state, expired);
    state.advance(1);
    state.set_finalized(state.block - 1).unwrap();
    state
        .seed_typed("MevShield", "NextKey", (), current)
        .unwrap();
    state.set_handler("chain_getHeader", |state, request| {
        let best = request
            .params
            .as_array()
            .is_none_or(|params| params.first().is_none_or(serde_json::Value::is_null));
        if best {
            let finalized = state.block_hash(state.finalized).unwrap();
            rtao_testkit::Reply::result(state.header(&finalized).unwrap())
        } else {
            state.dispatch(request)
        }
    });
    state
}

/// The key comes from an unpinned read, so a best header that trails the
/// endpoint's state does not seal the carrier to an expired key.
#[tokio::test]
async fn shield_key_is_read_unpinned_when_the_best_header_trails_the_state() {
    let expired = shield_key(0);
    let current = shield_key(1);
    let fake = FakeChain::from_state(trailing_headers_with_rotated_key(
        expired.clone(),
        current.clone(),
    ));
    let client = Client::from_rpc(RpcClient::new(fake.clone()), config())
        .await
        .unwrap();
    // The reported best header trails the newest state by one block, and a
    // read at its hash still returns the expired key.
    let best = client.at(BlockRef::Best).await.unwrap();
    assert_eq!(best.block().number + 1, fake.state().lock().unwrap().block);
    assert_eq!(
        best.storage(&rtao_runtime::storage::mev_shield::NEXT_KEY, ())
            .await
            .unwrap(),
        Some(expired.clone())
    );
    let start = fake.requests().len();
    let report = client
        .submit_shielded(
            remark(&client, vec![10]).await,
            &signer(),
            &rtao_client::ShieldOptions {
                wait: Wait::Pool,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(report.inclusion, Inclusion::PoolAccepted);
    let submissions = fake.state().lock().unwrap().submissions.clone();
    assert_eq!(submissions.len(), 1);
    assert!(
        sealed_to(&submissions[0], &current),
        "not sealed to the newest key"
    );
    assert!(
        !sealed_to(&submissions[0], &expired),
        "sealed to the expired key"
    );
    // Availability check and seal: two unpinned reads, none at a block hash.
    assert_eq!(next_key_reads(&fake, start).len(), 2);
    client.close().await;
}

/// The same holds for a submit endpoint whose best header trails its state
/// while the read endpoint lags further behind.
#[tokio::test]
async fn shield_key_is_read_unpinned_from_a_submit_endpoint_with_trailing_headers() {
    let expired = shield_key(1);
    let current = shield_key(2);
    let mut read = FakeState::default();
    seed_shield(&mut read, shield_key(0));
    let read = FakeChain::from_state(read);
    let node = FakeChain::from_state(trailing_headers_with_rotated_key(
        expired.clone(),
        current.clone(),
    ));
    let client = Client::from_rpc_with_submit(
        RpcClient::new(read.clone()),
        RpcClient::new(node.clone()),
        config(),
    )
    .await
    .unwrap();
    let read_start = read.requests().len();
    let report = client
        .submit_shielded(
            remark(&client, vec![11]).await,
            &signer(),
            &rtao_client::ShieldOptions {
                wait: Wait::Pool,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(report.inclusion, Inclusion::PoolAccepted);
    let submissions = node.state().lock().unwrap().submissions.clone();
    assert_eq!(submissions.len(), 1);
    assert!(
        sealed_to(&submissions[0], &current),
        "not sealed to the newest key"
    );
    assert!(
        !sealed_to(&submissions[0], &expired),
        "sealed to the expired key"
    );
    assert_eq!(next_key_reads(&node, 0).len(), 2);
    assert!(next_key_reads(&read, read_start).is_empty());
    client.close().await;
}

/// The era guard measures the submitting node's best head against the read
/// endpoint's finalized anchor, so a lagging read endpoint cannot hide a lag
/// that leaves the era too short for inclusion.
#[tokio::test]
async fn shield_era_guard_uses_the_submit_endpoint_best_head() {
    let (client, read, node) = shield_split(shield_key(0), shield_key(1), 6).await;
    let signer = signer();
    let opts = rtao_client::ShieldOptions {
        wait: Wait::Pool,
        finality_wait: std::time::Duration::ZERO,
        ..Default::default()
    };
    let error = client
        .submit_shielded(remark(&client, vec![9]).await, &signer, &opts)
        .await
        .unwrap_err();
    assert!(
        matches!(
            error,
            rtao_client::SubmitFailure::ShieldEraExpired {
                finality_lag: 6,
                ..
            }
        ),
        "{error:?}"
    );
    assert!(node.state().lock().unwrap().submissions.is_empty());
    assert!(read.state().lock().unwrap().submissions.is_empty());
    client.close().await;
}

/// A submit endpoint that cannot be opened is named in the error, which keeps
/// its transport variant.
#[tokio::test]
async fn a_submit_endpoint_connect_failure_names_the_submit_endpoint() {
    // A port that was bound and released refuses connections.
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let read = FakeChain::from_state(FakeState::default());
    let mut cfg = config();
    cfg.set_submit_endpoint(&format!("ws://127.0.0.1:{port}"))
        .unwrap();
    cfg.startup_timeout = std::time::Duration::from_secs(5);
    let client = Client::from_rpc(RpcClient::new(read.clone()), cfg)
        .await
        .unwrap();
    assert!(client.has_submit_endpoint());
    let error = tokio::time::timeout(std::time::Duration::from_secs(30), client.receipt_client())
        .await
        .expect("connect failure is bounded")
        .unwrap_err();
    assert!(
        matches!(error, rtao_client::ClientError::Rpc(_)),
        "{error:?}"
    );
    assert!(error.to_string().contains("submit endpoint"), "{error}");
    // Reads are unaffected.
    client.at(BlockRef::Finalized).await.unwrap();
    client.close().await;
}

/// Closing the client closes the submit connection it opened.
#[tokio::test]
async fn close_closes_an_opened_submit_session() {
    let (client, _read, node) = split().await;
    let submit = client.receipt_client().await.unwrap();
    assert!(!submit.is_closed());
    submit.at(BlockRef::Finalized).await.unwrap();
    let before = node.requests().len();
    client.close().await;
    assert!(client.is_closed());
    assert!(submit.is_closed());
    assert!(submit.at(BlockRef::Finalized).await.is_err());
    assert!(client.receipt_client().await.is_err());
    assert_eq!(node.requests().len(), before);
}
