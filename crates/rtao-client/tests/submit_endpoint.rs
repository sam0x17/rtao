//! A separate submission endpoint carries nonce, fee, submission and receipt
//! calls; every other read stays on the main endpoint.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use rtao_client::{BlockRef, Client, ClientConfig, Inclusion, Network, SubmitOptions, Wait};
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
const DEFAULT_DIGEST: &str = "e5302805ea229928b9e99eba9e8b6bb2f454d4867cd1e8dc708fee7a6ffc8da1";
