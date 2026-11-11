// Derived from RaoFoundation/subtensor sdk/python/bittensor/multisig.py @ b6a81d144078eac18bd25c3af7cb31579bb670f3 (Apache-2.0); modified by rtao.
//! Multisig funding checks price a hashed or post-quantum member's proof
//! (D-177), not a 64-byte signature.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use rtao_client::{Client, ClientConfig, Network, View};
use rtao_core::keys::{CRYPTO_HASHED, CRYPTO_MLDSA65_FIXED, Keypair};
use rtao_primitives::CryptoType;
use rtao_signer::{AuthoritySigner, KeypairAuthoritySigner};
use rtao_testkit::rpc::RpcClient;
use rtao_testkit::{FakeChain, FakeState};
use rtao_tx::multisig::{FundsRequest, check_multisig_funds};

async fn probe_length(descriptor: Option<&[u8]>, signer: rtao_primitives::AccountId) -> usize {
    let chain = FakeChain::from_state(FakeState::try_default().unwrap());
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None).unwrap()).unwrap();
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    let client = Client::from_rpc(RpcClient::new(chain.clone()), config)
        .await
        .unwrap();
    let snapshot = View::pin(&client).await.unwrap();
    let runtime = client.refresh_runtime().await.unwrap();
    let call = rtao_runtime::calls::system::remark(vec![1, 2, 3])
        .compose(&runtime)
        .unwrap();
    let start = chain.requests().len();
    // The empty account cannot pay; only the probe matters here.
    let _ = check_multisig_funds(
        &client,
        &snapshot,
        FundsRequest {
            signer,
            threshold: 2,
            opening: false,
            outer_call: Some(&call),
            crypto_type: CryptoType::Sr25519,
            descriptor,
            signer_label: None,
            shielded: false,
        },
    )
    .await;
    let requests = chain.requests();
    let probe = requests[start..]
        .iter()
        .find(|request| {
            request.method == "state_call"
                && request.params[0] == "TransactionPaymentApi_query_info"
        })
        .expect("fee probe");
    client.close().await;
    hex::decode(probe.params[1].as_str().unwrap().trim_start_matches("0x"))
        .unwrap()
        .len()
}

#[tokio::test]
async fn funding_probes_carry_the_scheme_proof() {
    for crypto in [CRYPTO_HASHED, CRYPTO_MLDSA65_FIXED] {
        let key = KeypairAuthoritySigner::new(Keypair::from_seed(&[crypto; 32], crypto).unwrap())
            .unwrap();
        let account = AuthoritySigner::account(&key);
        let classical = probe_length(None, account).await;
        let authority = probe_length(Some(key.descriptor()), account).await;
        assert!(
            authority >= classical + key.authority_type().proof_length() - 64,
            "{crypto}: {authority} vs {classical}"
        );
    }
}
