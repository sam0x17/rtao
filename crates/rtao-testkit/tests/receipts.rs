//! A failed Sudo inner dispatch resolved from actual encoded FakeChain events.

use blake2::{Blake2b, Digest, digest::consts::U32};
use rtao_core::{
    codec::Value,
    ops::{extrinsic_lookup, fee, receipt, storage},
    runtime::Runtime,
    value_ext::ValueExt,
};
use rtao_testkit::{FakeChain, FakeState};
use serde_json::{Value as Json, json};
use std::{error::Error, sync::Arc};
use subxt_rpcs::{RpcClient, client::RpcParams};

async fn execute(
    rpc: &RpcClient,
    request: rtao_core::ops::RpcRequest,
) -> Result<Json, Box<dyn Error>> {
    let mut params = RpcParams::new();
    for value in request
        .params
        .as_array()
        .ok_or("request params are not array")?
    {
        params.push(value)?;
    }
    Ok(rpc.request(request.method, params).await?)
}

#[tokio::test]
async fn sudo_sudo_inner_bad_origin_overrides_encoded_extrinsic_success()
-> Result<(), Box<dyn Error>> {
    let runtime = Arc::new(Runtime::parse(
        include_bytes!("../../../vendor/metadata/finney/478.v15.scale"),
        478,
        1,
        42,
    )?);
    // Root cannot dispatch a signed-origin balance transfer. Sudo.sudo therefore
    // succeeds as an outer dispatch and emits Sudid(Err(BadOrigin)) inside it.
    let inner = runtime.compose_call(
        "Balances",
        "transfer_keep_alive",
        &Value::record(vec![
            ("dest".into(), Value::Bytes(vec![7; 32])),
            ("value".into(), Value::Int(1)),
        ]),
    )?;
    let call = runtime.compose_call(
        "Sudo",
        "sudo",
        &Value::record(vec![("call".into(), Value::Bytes(inner))]),
    )?;
    let xt = fee::probe(&runtime, &call, [7; 32], 0, 0, [8; 32])?;
    let wanted: [u8; 32] = Blake2b::<U32>::digest(&xt).into();
    let at = format!("0x{}", hex::encode([9; 32]));
    let phase = || Value::record(vec![("ApplyExtrinsic".into(), Value::Int(1))]);
    let raw_event = |pallet: &str, name: &str, attributes: Value| {
        Value::record(vec![
            ("phase".into(), phase()),
            (
                "event".into(),
                Value::record(vec![(
                    pallet.into(),
                    Value::record(vec![(name.into(), attributes)]),
                )]),
            ),
            ("topics".into(), Value::List(vec![])),
        ])
    };
    let events = Value::List(vec![
        raw_event(
            "Sudo",
            "Sudid",
            Value::record(vec![(
                "sudo_result".into(),
                Value::record(vec![("Err".into(), Value::str("BadOrigin"))]),
            )]),
        ),
        raw_event(
            "System",
            "ExtrinsicSuccess",
            Value::record(vec![(
                "dispatch_info".into(),
                Value::record(vec![
                    (
                        "weight".into(),
                        Value::record(vec![
                            ("ref_time".into(), Value::Int(42)),
                            ("proof_size".into(), Value::Int(9)),
                        ]),
                    ),
                    ("class".into(), Value::str("Normal")),
                    ("pays_fee".into(), Value::str("Yes")),
                ]),
            )]),
        ),
    ]);
    let mut state = FakeState::new(runtime.clone());
    state.seed_events(&at, rtao_core::codec::value::to_corpus_json(&events))?;
    state.set_result("chain_getBlock",json!({"block":{"header":{"number":"0x0c"},"extrinsics":["0x00",format!("0x{}",hex::encode(&xt))]}}));
    let chain = FakeChain::from_state(state);
    let rpc = RpcClient::new(chain.clone());
    let block = execute(&rpc, extrinsic_lookup::request(&at)).await?;
    let index = extrinsic_lookup::find(&block, wanted)?.ok_or("included extrinsic missing")?;
    assert_eq!(index, 1);
    assert_eq!(extrinsic_lookup::block_number(&block)?, 12);
    let info = storage::storage_info(&runtime, "System", "Events")?;
    let raw = execute(
        &rpc,
        storage::storage_request(&runtime, info, &[], Some(&at))?,
    )
    .await?;
    let records = storage::decode_storage(&runtime, info, raw.as_str())?;
    let triggered = receipt::events_for_extrinsic(&records, index)?;
    assert_eq!(triggered.len(), 2);
    let sudo = triggered
        .first()
        .and_then(|event| event.field("attributes"))
        .ok_or("Sudid attributes missing")?;
    assert!(sudo.field("result").is_none());
    assert_eq!(
        sudo.field("sudo_result")
            .and_then(|result| result.variant("Err"))
            .and_then(Value::as_str),
        Some("BadOrigin")
    );
    let outcome = receipt::resolve(&runtime, &records, index)?;
    assert!(!outcome.success);
    let error = outcome.error.ok_or("inner failure lost")?;
    assert_eq!(error.name, "BadOrigin");
    assert_eq!(
        error.to_value().field("type").and_then(Value::as_str),
        Some("System")
    );
    assert_eq!(extrinsic_lookup::find(&block, [0; 32])?, None);
    assert!(receipt::events_for_extrinsic(&records, 0)?.is_empty());
    assert_eq!(
        chain
            .requests()
            .iter()
            .map(|request| request.method.as_str())
            .collect::<Vec<_>>(),
        vec!["chain_getBlock", "state_getStorage"]
    );
    assert!(chain.requests().iter().all(|request| {
        request
            .params
            .as_array()
            .and_then(|params| params.last())
            .and_then(Json::as_str)
            == Some(at.as_str())
    }));
    Ok(())
}
