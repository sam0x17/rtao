// Derived from RaoFoundation/subtensor sdk/python/bittensor/_transport/contract.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/_transport/interface.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/_transport/extrinsics.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Public receipts retain the exact event-derived weight and failed event evidence.
#![allow(clippy::unwrap_used)]

use rtao_client::{
    BlockRef, Client, ClientConfig, Inclusion, Network, SubmitOptions, ViewExt, Wait,
};
use rtao_core::{DynValue, codec::Value as CoreValue};
use rtao_signer::{CryptoType, KeypairSigner};
use rtao_testkit::{FakeChain, FakeState, SubmitScript, rpc::RpcClient};
use serde_json::{Value, json};

async fn fixture() -> (Client, FakeChain, KeypairSigner) {
    let chain = FakeChain::from_state(FakeState::default());
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None).unwrap()).unwrap();
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    let client = Client::from_rpc(RpcClient::new(chain.clone()), config)
        .await
        .unwrap();
    let signer = KeypairSigner::from_seed(&[7; 32], CryptoType::Ed25519).unwrap();
    (client, chain, signer)
}

async fn call(client: &Client) -> rtao_runtime::CallBytes {
    rtao_runtime::calls::system::remark(vec![1, 2, 3])
        .compose(&client.refresh_runtime().await.unwrap())
        .unwrap()
}

fn event(module: &str, name: &str, attributes: Value) -> Value {
    json!({"phase":"ApplyExtrinsic","extrinsic_idx":0,
        "module_id":module,"event_id":name,"attributes":attributes,"topics":[]})
}

#[tokio::test]
async fn pool_receipt_has_null_weight_before_inclusion_is_observed() {
    let (client, chain, signer) = fixture().await;
    let report = client
        .submit(
            call(&client).await,
            &signer,
            &SubmitOptions {
                wait: Wait::Pool,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(report.inclusion, Inclusion::PoolAccepted);
    let weight: &DynValue = &report.weight;
    assert_eq!(weight.as_core(), &CoreValue::Null);
    assert_eq!(weight.to_json(), Value::Null);
    assert!(report.block_hash.is_none());
    assert!(report.fee.is_none());
    assert!(report.events.is_empty());
    assert_eq!(chain.state().lock().unwrap().submissions.len(), 1);
    client.close().await;
}

#[tokio::test]
async fn included_success_and_direct_or_nested_failure_keep_exact_weights_and_events() {
    // Values above 2^53 require lossless integer transport; distinct components
    // and outcomes also detect accidental reuse of a fee estimate or last report.
    for (terminal, nested, expected_success, expected_weight) in [
        (
            "ExtrinsicSuccess",
            false,
            true,
            json!({"ref_time":9_007_199_254_740_993u64,"proof_size":73}),
        ),
        (
            "ExtrinsicFailed",
            false,
            false,
            json!({"ref_time":123_456_789,"proof_size":9_007_199_254_740_995u64}),
        ),
        (
            "ExtrinsicSuccess",
            true,
            false,
            json!({"ref_time":u64::MAX,"proof_size":8191}),
        ),
    ] {
        let (client, chain, signer) = fixture().await;
        let mut attributes = json!({"dispatch_info":{
            "weight":expected_weight,"class":"Normal","pays_fee":"Yes"}});
        if terminal == "ExtrinsicFailed" {
            attributes["dispatch_error"] = json!({"BadOrigin":null});
        }
        let mut events = Vec::new();
        if nested {
            events.push(event(
                "Sudo",
                "Sudid",
                json!({"sudo_result":{"Err":{"BadOrigin":null}}}),
            ));
        }
        events.push(event("System", terminal, attributes));
        chain.mutate_state(|state| {
            state.queue_submit(SubmitScript::InBlockThenFinalized {
                events: json!(events),
            });
        });
        let report = client
            .submit(
                call(&client).await,
                &signer,
                &SubmitOptions {
                    wait: Wait::Inclusion,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(report.inclusion, Inclusion::InBlock);
        assert_eq!(report.success, expected_success);
        assert_eq!(report.weight.to_json(), expected_weight);
        assert_eq!(
            report.error.as_ref().map(|error| error.name.as_str()),
            (!expected_success).then_some("BadOrigin")
        );
        // D-023: receipt weights are additional data; failures must retain the
        // complete event vector used by native multisig and recovery consumers.
        let snapshot = client
            .at(BlockRef::Hash(report.block_hash.unwrap()))
            .await
            .unwrap();
        let decoded = snapshot.events().await.unwrap();
        let expected_events = decoded
            .for_extrinsic(report.extrinsic_index.unwrap())
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(report.events, expected_events);
        assert_eq!(report.events.len(), if nested { 2 } else { 1 });
        assert!(report.events.last().unwrap().matches("System", terminal));
        if nested {
            assert!(report.events[0].matches("Sudo", "Sudid"));
        }
        assert_eq!(chain.state().lock().unwrap().submissions.len(), 1);
        client.close().await;
    }
}
