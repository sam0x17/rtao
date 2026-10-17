//! The online `ClientRuntimeSource` against an in-process fake chain, with
//! requests built by rtao-client's real `prepare_public` (review findings 4,
//! 5 and 9). No node or network is used.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::{collections::BTreeSet, net::SocketAddr, sync::Arc, time::Duration};

use rtao_btauth::InMemoryNonceStore;
use rtao_client::{
    Client, ClientConfig, Network, NoncePolicy, PublicPrepareOptions, SubmitOptions,
    UnsignedExtrinsic,
};
use rtao_core::codec::Value;
use rtao_primitives::{Balance, CryptoType, Tao, TaoBalance};
use rtao_remote_signer::{
    RemoteSigner, RemoteSignerConfig, RemoteSignerError,
    server::{
        AuthorizedClient, ClientRuntimeSource, EraInfo, RemoteSignerServer, ServedAccount,
        ServerConfig, rebuild,
    },
    wire::{HexBytes, SignRequest, blake2_256, to_hex},
};
use rtao_runtime::{RuntimeHandle, calls::balances};
use rtao_signer::{KeypairSigner, SignPurpose, Signer, SigningContext, SigningRequest};
use rtao_testkit::{FakeChain, FakeState, Reply, rpc::RpcClient};
use url::Url;

fn key(uri: &str) -> KeypairSigner {
    KeypairSigner::from_uri(uri, CryptoType::Sr25519).unwrap()
}

async fn chain() -> (Client, FakeChain) {
    let fake = FakeChain::from_state(FakeState::default());
    let mut cfg = ClientConfig::new(Network::resolve_with("local", |_| None).unwrap()).unwrap();
    cfg.track_heads = false;
    cfg.runtime_cache_dir = None;
    cfg.config_store = None;
    let client = Client::from_rpc(RpcClient::new(fake.clone()), cfg)
        .await
        .unwrap();
    (client, fake)
}

/// The reference server, online against the fake chain, on loopback.
async fn serve(client: &Client, edit: impl FnOnce(&mut ServerConfig)) -> SocketAddr {
    let mut cfg = ServerConfig::new(client.genesis().0);
    cfg.accounts = vec![ServedAccount::new(Arc::new(key("//Alice")))];
    cfg.clients = vec![AuthorizedClient {
        client: key("//Charlie").account(),
        accounts: BTreeSet::from([key("//Alice").account()]),
    }];
    edit(&mut cfg);
    let server = RemoteSignerServer::new(
        cfg,
        Arc::new(ClientRuntimeSource::new(client.clone())),
        Arc::new(InMemoryNonceStore::new()),
    )
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(server.serve(listener));
    address
}

async fn remote(address: SocketAddr) -> RemoteSigner {
    let mut cfg = RemoteSignerConfig::new(
        Url::parse(&format!("http://{address}/")).unwrap(),
        key("//Alice").account(),
        CryptoType::Sr25519,
        Arc::new(key("//Charlie")),
    );
    cfg.timeout = Duration::from_secs(10);
    RemoteSigner::connect(cfg).await.unwrap().0
}

/// rtao-client's own preparation, exactly as `--signer remote` sends it.
async fn prepared(client: &Client, rao: u64) -> UnsignedExtrinsic {
    let runtime = client.refresh_runtime().await.unwrap();
    let call = balances::transfer_keep_alive(
        Value::Bytes(key("//Bob").account().0.to_vec()).into(),
        TaoBalance(rao),
    )
    .compose(&runtime)
    .unwrap();
    client
        .prepare_public(
            call,
            key("//Alice").account(),
            &PublicPrepareOptions {
                submit: SubmitOptions {
                    nonce: NoncePolicy::Explicit(5),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .await
        .unwrap()
}

/// Owned signing views, consistent with each other.
struct Views {
    request: SignRequest,
    context: SigningContext,
}

impl Views {
    fn of(unsigned: &UnsignedExtrinsic) -> Self {
        let request = SignRequest::from_signing_request(
            &unsigned.signing_request(SignPurpose::Extrinsic),
            unsigned.address(),
            CryptoType::Sr25519,
        );
        Self {
            request,
            context: unsigned.context().clone(),
        }
    }

    /// Re-anchor the views at another block and era, re-deriving every byte.
    fn anchored(&self, runtime: &RuntimeHandle, anchor: [u8; 32], era: EraInfo) -> Self {
        let (mut params, _) = rebuild::parse_payload_json(runtime, &self.request).unwrap();
        params.era = era.to_value();
        params.era_block_hash = anchor;
        let (extra, additional) = runtime.core().signature_payload_parts(&params).unwrap();
        let mut data = [self.request.call.0.as_slice(), &extra, &additional].concat();
        if data.len() > 256 {
            data = blake2_256(&data).to_vec();
        }
        let mut request = self.request.clone();
        request.included_in_extrinsic = HexBytes(extra);
        request.included_in_signed_data = HexBytes(additional);
        request.payload = HexBytes(data);
        request.payload_json = rebuild::build_payload_json(
            runtime,
            &self.request.call.0,
            self.request.account.0,
            &params,
        )
        .unwrap();
        Self {
            request,
            context: self.context.clone(),
        }
    }

    async fn sign(&self, signer: &RemoteSigner) -> Result<Vec<u8>, RemoteSignerError> {
        let request = &self.request;
        let signing = SigningRequest {
            call: &request.call.0,
            included_in_extrinsic: &request.included_in_extrinsic.0,
            included_in_signed_data: &request.included_in_signed_data.0,
            payload: &request.payload.0,
            payload_json: &request.payload_json,
            context: &self.context,
            purpose: SignPurpose::Extrinsic,
        };
        signer
            .sign_request(&signing)
            .await
            .map(|signature| signature.bytes.to_vec())
    }
}

fn assert_era_rejected(result: Result<Vec<u8>, RemoteSignerError>, detail: &str) {
    match result {
        Err(RemoteSignerError::Http {
            status: 422,
            code: Some(code),
            detail: actual,
            ..
        }) if code == "era_rejected" => assert_eq!(actual, detail),
        other => panic!("expected era_rejected ({detail}), got {other:?}"),
    }
}

fn hash32(text: &str) -> [u8; 32] {
    rtao_remote_signer::wire::from_hex(text)
        .unwrap()
        .try_into()
        .unwrap()
}

fn mortal(views: &Views, runtime: &RuntimeHandle) -> (u64, u64) {
    match rebuild::parse_payload_json(runtime, &views.request)
        .unwrap()
        .1
    {
        EraInfo::Mortal { period, phase } => (period, phase),
        EraInfo::Immortal => panic!("expected a mortal era"),
    }
}

#[tokio::test]
async fn real_client_payloads_are_signed_and_era_anchors_are_checked_online() {
    let (client, fake) = chain().await;
    let runtime = client.refresh_runtime().await.unwrap();
    let signer = remote(serve(&client, |_| {}).await).await;

    // The real rtao-client builder's views are accepted and signed.
    let unsigned = prepared(&client, 1_000).await;
    let signature = signer
        .sign(&unsigned.signing_request(SignPurpose::Extrinsic))
        .await
        .unwrap();
    unsigned.verify_signature(signature).unwrap();
    let views = Views::of(&unsigned);
    let (period, _) = mortal(&views, &runtime);
    let (best, finalized) = fake.mutate_state(|state| (state.block, state.finalized));
    assert_eq!(best, finalized);
    let anchor = views.request.payload_json["blockHash"]
        .as_str()
        .map(hash32)
        .unwrap();
    assert_eq!(
        Some(to_hex(&anchor)),
        fake.mutate_state(|state| state.block_hash(finalized))
    );

    // Unknown anchor hash.
    let era = |number: u64| EraInfo::Mortal {
        period,
        phase: number % period,
    };
    let unknown = views.anchored(&runtime, [9; 32], era(best));
    assert_era_rejected(
        unknown.sign(&signer).await,
        "era block is unknown to the server's node",
    );

    // A fork block at the anchor's height is known but not canonical.
    let parent = fake
        .mutate_state(|state| state.block_hash(best - 1))
        .unwrap();
    let fork = fake.mutate_state(|state| state.fork(&parent)).unwrap();
    let forked = views.anchored(&runtime, hash32(&fork), era(best));
    assert_era_rejected(
        forked.sign(&signer).await,
        "era block is not on the canonical chain",
    );

    // A fork block above the best height is refused by the window check.
    let tip = fake.mutate_state(|state| state.block_hash(best)).unwrap();
    let above = fake.mutate_state(|state| state.fork(&tip)).unwrap();
    let ahead = views.anchored(&runtime, hash32(&above), era(best + 1));
    assert_era_rejected(
        ahead.sign(&signer).await,
        "era block does not match the era phase or the era has expired",
    );

    // The canonical anchor with a phase for another height.
    let shifted = views.anchored(&runtime, anchor, era(best + 1));
    assert_era_rejected(
        shifted.sign(&signer).await,
        "era block does not match the era phase or the era has expired",
    );

    // Still live one block before expiry, refused once the period has passed.
    fake.advance(period - 1).unwrap();
    assert!(views.sign(&signer).await.is_ok());
    fake.advance(1).unwrap();
    assert_era_rejected(
        views.sign(&signer).await,
        "era block does not match the era phase or the era has expired",
    );

    // Anchor lookups never ask for the anchor's runtime or metadata.
    let anchored_runtime_reads = fake
        .requests()
        .iter()
        .filter(|request| {
            matches!(
                request.method.as_str(),
                "state_getRuntimeVersion" | "state_getMetadata"
            ) && request.params.get(0).and_then(serde_json::Value::as_str) == Some(fork.as_str())
        })
        .count();
    assert_eq!(anchored_runtime_reads, 0);
}

#[tokio::test]
async fn online_source_failures_are_unavailable_not_refusals() {
    let (client, fake) = chain().await;
    let signer = remote(serve(&client, |_| {}).await).await;
    let unsigned = prepared(&client, 1_000).await;
    fake.mutate_state(|state| {
        state.queue_reply(
            "chain_getHeader",
            Reply::error(-32000, "node-internal failure", None),
        );
    });
    let error = signer
        .sign_request(&unsigned.signing_request(SignPurpose::Extrinsic))
        .await
        .unwrap_err();
    match error {
        RemoteSignerError::Http {
            status: 503,
            code: Some(code),
            detail,
            ..
        } => {
            assert_eq!(code, "chain_unavailable");
            assert!(!detail.contains("node-internal"), "{detail}");
        }
        other => panic!("{other:?}"),
    }
    // The next request succeeds once the node answers again.
    assert!(
        signer
            .sign_request(&unsigned.signing_request(SignPurpose::Extrinsic))
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn online_max_fee_uses_the_nodes_estimate() {
    let (client, fake) = chain().await;
    let fee = fake.mutate_state(|state| state.fee);
    let unsigned = prepared(&client, 1_000).await;
    let request = unsigned.signing_request(SignPurpose::Extrinsic);
    let at_cap = remote(
        serve(&client, |cfg| {
            cfg.policy.limits.max_fee = Some(Balance::<Tao>::from_rao(fee));
        })
        .await,
    )
    .await;
    assert!(at_cap.sign_request(&request).await.is_ok());
    let below = remote(
        serve(&client, |cfg| {
            cfg.policy.limits.max_fee = Some(Balance::<Tao>::from_rao(fee - 1));
        })
        .await,
    )
    .await;
    match below.sign_request(&request).await.unwrap_err() {
        RemoteSignerError::Http {
            status: 403,
            violations,
            ..
        } => assert!(
            violations
                .first()
                .is_some_and(|violation| violation.contains("exceeds max_fee_tao")),
            "{violations:?}"
        ),
        other => panic!("{other:?}"),
    }
}
