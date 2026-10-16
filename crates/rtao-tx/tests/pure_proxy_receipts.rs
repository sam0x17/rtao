// Derived from RaoFoundation/subtensor sdk/python/bittensor/executor.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Pure-proxy result maps through clear and explicit shield Executor routes.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use rtao_client::{
    Client, ClientConfig, Inclusion, Mortality, Network, NoncePolicy, Period, SubmitOptions,
};
use rtao_core::value_ext::ValueExt;
use rtao_intents::CreatePureProxy;
use rtao_primitives::{AccountId, CryptoType, H256};
use rtao_signer::{KeypairSigner, SignPurpose, Signer};
use rtao_testkit::rpc::RpcClient;
use rtao_testkit::{FakeChain, FakeState, SubmitScript};
use rtao_tx::{ExecuteOptions, Executor, ExtrinsicResult, PlanOptions, PureCreated, ShieldOptions};
use serde_json::{Value, json};

const PURE: AccountId = AccountId([19; 32]);

async fn fixture() -> (Executor, FakeChain, KeypairSigner) {
    let mut state = FakeState::default();
    state
        .seed_typed("MevShield", "NextKey", (), vec![0u8; 1184])
        .unwrap();
    state
        .seed_constant("Aura", "SlotDuration", json!(1))
        .unwrap();
    let fake = FakeChain::from_state(state);
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None).unwrap()).unwrap();
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    let client = Client::from_rpc(RpcClient::new(fake.clone()), config)
        .await
        .unwrap();
    let signer = KeypairSigner::from_seed(&[7; 32], CryptoType::Ed25519).unwrap();
    (Executor::new(client), fake, signer)
}

fn success(index: u32) -> Value {
    json!({"phase":"ApplyExtrinsic","extrinsic_idx":index,"module_id":"System","event_id":"ExtrinsicSuccess","attributes":{"dispatch_info":{"weight":{"ref_time":1,"proof_size":0},"class":"Normal","pays_fee":"Yes"}},"topics":[]})
}

fn pure_created(index: u32, spawner: AccountId) -> Value {
    json!({"phase":"ApplyExtrinsic","extrinsic_idx":index,"module_id":"Proxy","event_id":"PureCreated","attributes":{"pure":PURE,"who":spawner,"proxy_type":"Any","disambiguation_index":9},"topics":[]})
}

async fn signed_inner(
    executor: &Executor,
    signer: &KeypairSigner,
    intent: &CreatePureProxy,
) -> (Vec<u8>, H256) {
    let plan = executor
        .plan(intent, signer, &PlanOptions::default())
        .await
        .unwrap();
    assert!(plan.ok());
    assert!(plan.extras.is_empty());
    let prepared = executor
        .client()
        .prepare_call(
            plan.call().clone(),
            signer,
            &SubmitOptions {
                nonce: NoncePolicy::Explicit(1),
                mortality: Mortality::Mortal(Period::new(8)),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let signature = signer
        .sign(&prepared.signing_request(SignPurpose::ShieldInner))
        .await
        .unwrap();
    let (bytes, hash) = executor
        .client()
        .attach_signature(&prepared, signature)
        .await
        .unwrap();
    (bytes.to_vec(), hash)
}

// The fixture author exposes the exact signed inner after the carrier at block
// 101/index 1. This exercises production inner-hash discovery and event decoding;
// it does not emulate author decryption or prove live-chain/hardware behavior.
fn install_inner(fake: &FakeChain, bytes: Vec<u8>, spawner: AccountId) {
    fake.mutate_state(|state| {
        state.set_handler("chain_getBlock", move |state, request| {
            let mut reply = state.dispatch(request);
            if let Some(Ok(raw)) = &mut reply.outcome {
                let number = raw["block"]["header"]["number"]
                    .as_str()
                    .and_then(|value| u64::from_str_radix(value.trim_start_matches("0x"), 16).ok());
                if number == Some(101) {
                    let extrinsics = raw["block"]["extrinsics"].as_array_mut().unwrap();
                    assert_eq!(extrinsics.len(), 1);
                    extrinsics.push(json!(format!("0x{}", hex::encode(&bytes))));
                    state
                        .seed_events(
                            request.params[0].as_str().unwrap(),
                            json!([success(0), success(1), pure_created(1, spawner)]),
                        )
                        .unwrap();
                }
            }
            reply
        });
    });
}

fn assert_receipt<O>(
    result: &ExtrinsicResult<O>,
    fake: &FakeChain,
    signer: &KeypairSigner,
    index: u32,
) {
    assert!(result.success);
    assert!(result.error.is_none());
    assert!(result.fate_unknown.is_none());
    assert_eq!(result.inclusion, Inclusion::Finalized);
    let expected_id = format!("101-{index:04}");
    assert_eq!(result.extrinsic_id.as_deref(), Some(expected_id.as_str()));
    let expected_block = fake.state().lock().unwrap().block_hash(101).unwrap();
    assert_eq!(
        result
            .block_hash
            .map(|hash| format!("0x{}", hex::encode(hash.0)))
            .as_deref(),
        Some(expected_block.as_str())
    );
    assert_eq!(result.events.len(), 2);
    assert!(
        result
            .events
            .iter()
            .any(|event| event.matches("System", "ExtrinsicSuccess"))
    );
    let created = result
        .events
        .iter()
        .find(|event| event.matches("Proxy", "PureCreated"))
        .unwrap();
    let pure = PURE.to_string();
    let spawner = signer.account().to_string();
    assert_eq!(
        created.attributes.field("pure").and_then(ValueExt::as_str),
        Some(pure.as_str())
    );
    assert_eq!(
        created.attributes.field("who").and_then(ValueExt::as_str),
        Some(spawner.as_str())
    );
    assert_eq!(fake.state().lock().unwrap().submissions.len(), 1);
}

#[tokio::test]
async fn pure_proxy_clear_and_explicit_shield_receipts_preserve_python_data_shapes() {
    let intent = CreatePureProxy {
        proxy_type: "Any".into(),
        delay: 7,
        index: 9,
    };
    let (executor, fake, signer) = fixture().await;
    fake.mutate_state(|state| {
        state.queue_submit(SubmitScript::InBlockThenFinalized {
            events: json!([success(0), pure_created(0, signer.account())]),
        });
    });
    let clear = executor
        .execute(&intent, &signer, &ExecuteOptions::default())
        .await
        .unwrap();
    assert_receipt(&clear, &fake, &signer, 0);
    let expected_clear = json!({
        "pure_proxy": PURE,
        "spawner": signer.account(),
        "height": 101,
        "ext_index": 0,
    });
    assert_eq!(Value::Object(clear.data.clone()), expected_clear);
    assert_eq!(clear.to_json()["data"], expected_clear);
    assert_eq!(
        clear.output,
        PureCreated {
            pure: Some(PURE),
            spawner: Some(signer.account()),
            height: Some(101),
            ext_index: Some(0),
        }
    );

    for dynamic in [false, true] {
        let (executor, fake, signer) = fixture().await;
        let (inner_bytes, inner_hash) = signed_inner(&executor, &signer, &intent).await;
        install_inner(&fake, inner_bytes, signer.account());
        let options = ShieldOptions {
            execute: ExecuteOptions {
                wait_for_finalization: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let shielded = if dynamic {
            executor
                .submit_shielded_dyn(&intent, &signer, &options)
                .await
                .unwrap()
                .with_output::<PureCreated>()
                .unwrap()
        } else {
            executor
                .submit_shielded(&intent, &signer, &options)
                .await
                .unwrap()
        };
        assert_receipt(&shielded, &fake, &signer, 1);
        let expected_shield = json!({
            "shielded": true,
            "inner_extrinsic_hash": format!("0x{}", hex::encode(inner_hash.0)),
        });
        assert_eq!(Value::Object(shielded.data.clone()), expected_shield);
        assert_eq!(shielded.to_json()["data"], expected_shield);
        assert_eq!(shielded.output, PureCreated::default());
    }
}
