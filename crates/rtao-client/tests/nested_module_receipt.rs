//! Nested module failures resolve against the runtime that executed the block.
#![allow(clippy::unwrap_used, clippy::panic)]

use std::sync::Arc;

use blake2::{Blake2b, Digest, digest::consts::U32};
use parity_scale_codec::{Compact, Decode, Encode};
use rtao_client::{Client, ClientConfig, Inclusion, Network};
use rtao_core::{
    codec::Value as CoreValue, ops::dispatch_error::DispatchErrorKind, runtime::Runtime,
};
use rtao_primitives::H256;
use rtao_testkit::{FakeChain, FakeState, Reply, rpc::RpcClient};
use serde_json::{Value, json};

const WRAPPERS: [(&str, &str, &str, bool); 6] = [
    ("Sudo", "Sudid", "sudo_result", false),
    ("Sudo", "SudoAsDone", "sudo_result", false),
    ("Proxy", "ProxyExecuted", "result", false),
    ("Multisig", "MultisigExecuted", "result", false),
    ("Utility", "BatchInterrupted", "error", true),
    ("Utility", "ItemFailed", "error", true),
];

fn renamed_runtime(base: &Runtime, spec: u32, name: &str, docs: &str) -> Arc<Runtime> {
    let mut metadata =
        frame_metadata::RuntimeMetadataPrefixed::decode(&mut base.metadata_bytes.as_slice())
            .unwrap();
    let frame_metadata::RuntimeMetadata::V15(ref mut metadata_v15) = metadata.1 else {
        panic!("expected V15 metadata")
    };
    let error_type = metadata_v15
        .pallets
        .iter()
        .find(|pallet| pallet.name == "Balances")
        .unwrap()
        .error
        .as_ref()
        .unwrap()
        .ty
        .id;
    let error_type = metadata_v15
        .types
        .types
        .iter_mut()
        .find(|ty| ty.id == error_type)
        .unwrap();
    let scale_info::TypeDef::Variant(errors) = &mut error_type.ty.type_def else {
        panic!("Balances errors must be an enum")
    };
    let variant = errors.variants.first_mut().unwrap();
    variant.name = name.into();
    variant.docs = vec![docs.into()];
    Arc::new(
        Runtime::parse(
            &metadata.encode(),
            spec,
            base.transaction_version,
            base.ss58_format,
        )
        .unwrap(),
    )
}

fn hash(value: &str) -> H256 {
    H256(
        hex::decode(value.strip_prefix("0x").unwrap())
            .unwrap()
            .try_into()
            .unwrap(),
    )
}

fn event(index: usize, pallet: &str, name: &str, attributes: Value) -> Value {
    json!({"phase":"ApplyExtrinsic", "extrinsic_idx":index,
        "module_id":pallet,"event_id":name,"attributes":attributes,"topics":[]})
}

#[tokio::test]
async fn every_nested_module_wrapper_uses_supplied_blocks_parent_runtime_after_upgrade() {
    let mut state = FakeState::default();
    let old = renamed_runtime(&state.runtime, 478, "BeforeUpgrade", "Old runtime docs.");
    let new = renamed_runtime(&state.runtime, 479, "AfterUpgrade", "New runtime docs.");
    assert_ne!(old.metadata_bytes, new.metadata_bytes);
    let pallet = old.pallet("Balances").unwrap();
    let error_type = old.resolve(pallet.errors_type.unwrap()).unwrap();
    let scale_info::TypeDef::Variant(errors) = &error_type.type_def else {
        panic!("Balances errors must be an enum")
    };
    let error_index = errors.variants.first().unwrap().index;
    assert_eq!(
        old.module_error(pallet.index, error_index).unwrap().0,
        "BeforeUpgrade"
    );
    assert_eq!(
        new.module_error(pallet.index, error_index).unwrap().0,
        "AfterUpgrade"
    );

    // Block 99 performs the upgrade, so its events use state 98's old runtime.
    // Block 100 uses state 99's new runtime. The client's current head is new.
    let old_parent = state.block_hash(98).unwrap();
    let upgrade_block = state.block_hash(99).unwrap();
    let next_block = state.block_hash(100).unwrap();
    state.set_runtime_at(old_parent.clone(), old.clone());
    state.set_runtime_at(upgrade_block.clone(), new.clone());
    state.set_runtime_at(next_block.clone(), new.clone());
    state.runtime = new;

    let module_error = json!({"Module":{"index":pallet.index,"error":[error_index,0,0,0]}});
    let mut events = Vec::new();
    let mut extrinsics = Vec::new();
    let mut extrinsic_hashes = Vec::new();
    for (index, (wrapper, name, field, bare)) in WRAPPERS.iter().enumerate() {
        let mut attributes = json!({});
        attributes[*field] = if *bare {
            module_error.clone()
        } else {
            json!({"Err":module_error})
        };
        events.push(event(index, wrapper, name, attributes));
        events.push(event(
            index,
            "System",
            "ExtrinsicSuccess",
            json!({"dispatch_info":{
            "weight":{"ref_time":1,"proof_size":0},"class":"Normal","pays_fee":"Yes"}}),
        ));
        let call = old
            .compose_call(
                "System",
                "remark",
                &CoreValue::record(vec![(
                    "remark".into(),
                    CoreValue::Bytes(vec![u8::try_from(index).unwrap()]),
                )]),
            )
            .unwrap();
        let mut body = vec![4];
        body.extend(call);
        let mut encoded = Compact(u32::try_from(body.len()).unwrap()).encode();
        encoded.extend(body);
        extrinsic_hashes.push(H256(Blake2b::<U32>::digest(&encoded).into()));
        extrinsics.push(format!("0x{}", hex::encode(encoded)));
    }
    state
        .seed_events(upgrade_block.clone(), json!(events))
        .unwrap();
    state
        .seed_events(next_block.clone(), json!(events))
        .unwrap();
    state.set_handler("chain_getBlock", move |state, request| {
        let block = request.params[0].as_str().unwrap();
        Reply::result(json!({"block":{"header":state.header(block),"extrinsics":extrinsics}}))
    });
    let chain = FakeChain::from_state(state);
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None).unwrap()).unwrap();
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    let client = Client::from_rpc(RpcClient::new(chain.clone()), config)
        .await
        .unwrap();
    let start = chain.requests().len();
    // Revisit the older block after loading both revisions to catch cache mixups.
    for (block, number, expected_name, expected_docs) in [
        (&upgrade_block, 99, "BeforeUpgrade", "Old runtime docs."),
        (&next_block, 100, "AfterUpgrade", "New runtime docs."),
        (&upgrade_block, 99, "BeforeUpgrade", "Old runtime docs."),
    ] {
        for (index, (wrapper, name, _, _)) in WRAPPERS.iter().enumerate() {
            let report = client
                .resolve_extrinsic(extrinsic_hashes[index], hash(block), false)
                .await
                .unwrap();
            assert!(!report.success, "missed {wrapper}.{name}");
            assert_eq!(report.inclusion, Inclusion::InBlock);
            assert_eq!(report.block_hash, Some(hash(block)));
            assert_eq!(report.block_number, Some(number));
            assert_eq!(report.extrinsic_index, Some(u32::try_from(index).unwrap()));
            assert_eq!(report.events.len(), 2);
            assert!(report.events[0].matches(wrapper, name));
            assert!(report.events[1].matches("System", "ExtrinsicSuccess"));
            assert_eq!(report.message, expected_docs);
            let error = report.error.unwrap();
            assert_eq!(error.kind, DispatchErrorKind::Module);
            assert_eq!(error.pallet.as_deref(), Some("Balances"));
            assert_eq!(
                error.name, expected_name,
                "wrong runtime for {wrapper}.{name}"
            );
            assert_eq!(
                error.docs,
                CoreValue::List(vec![CoreValue::str(expected_docs)])
            );
        }
    }
    let requests = chain.requests();
    for parent in [&old_parent, &upgrade_block] {
        assert!(
            requests[start..]
                .iter()
                .any(|request| request.method == "state_getRuntimeVersion"
                    && request.params == json!([parent]))
        );
    }
    client.close().await;
}
