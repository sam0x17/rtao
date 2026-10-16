// Derived from RaoFoundation/subtensor sdk/python/tests/unit/test_reads_table.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/tests/harness/samples.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Every authenticated Python sample through the native head and snapshot dispatcher.

use std::collections::{BTreeMap, BTreeSet};

use rtao_client::{BlockRef, Client, ClientConfig, Network};
use rtao_core::codec::Value as CoreValue;
use rtao_primitives::{AccountId, NetUid};
use rtao_reads::{ReadName, dispatch, reads::owner};
use rtao_testkit::{
    FakeChain, FakeState, RecordingView,
    fixtures::catalog_samples::{literal, read_state},
    rpc::RpcClient,
};
use serde_json::{Map, Value, json};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

const SAMPLE_PATH: &str = "sdk/python/tests/harness/samples.py";

fn samples() -> TestResult<BTreeMap<String, Map<String, Value>>> {
    let table = rtao_catalog::py::table(SAMPLE_PATH, "READ_SAMPLES")
        .ok_or("authenticated READ_SAMPLES table missing")?;
    assert_eq!(table.blob, "f04e4a3681a9798f20bebcb44ba508561509e391");
    let encoded: Value = rtao_primitives::json::from_str(table.value_json)?;
    let decoded = literal(&encoded)?;
    decoded
        .as_object()
        .ok_or("READ_SAMPLES must be a dictionary")?
        .iter()
        .map(|(name, args)| {
            Ok((
                name.clone(),
                args.as_object().ok_or("read sample arguments")?.clone(),
            ))
        })
        .collect()
}

// pyt: unit/test_reads_table.py::test_every_read_has_a_sample
// pyt: unit/test_reads_table.py::test_catalog_is_wellformed
// pyt: unit/test_reads_table.py::test_sample_params_match_declaration
#[test]
fn source_samples_equal_the_complete_registry_and_only_use_declared_parameters() -> TestResult {
    let samples = samples()?;
    let expected = ReadName::ALL
        .into_iter()
        .map(|name| name.as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(expected.len(), 102);
    assert_eq!(
        samples.keys().map(String::as_str).collect::<BTreeSet<_>>(),
        expected,
        "missing or stale authenticated source read samples"
    );
    for name in ReadName::ALL {
        let spec = name.catalog_spec();
        assert!(!spec.summary.is_empty(), "{name}: empty summary");
        assert!(!spec.category.is_empty(), "{name}: empty category");
        let declared = spec
            .params
            .iter()
            .map(|param| param.name)
            .collect::<BTreeSet<_>>();
        let supplied = samples[name.as_str()]
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        assert!(
            supplied.is_subset(&declared),
            "{name}: undeclared sample arguments"
        );
    }
    Ok(())
}

fn seeded_state() -> TestResult<FakeState> {
    let samples = samples()?;
    let coldkey = samples["stake_availability"]["coldkey_ss58"]
        .as_str()
        .ok_or("availability coldkey sample")?;
    Ok(read_state(coldkey)?)
}

async fn client(state: FakeState) -> TestResult<(Client, FakeChain)> {
    let chain = FakeChain::from_state(state);
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    let client = Client::from_rpc(RpcClient::new(chain.clone()), config).await?;
    Ok((client, chain))
}

// pyt: unit/test_reads_table.py::test_dispatches_at_head
// pyt: unit/test_reads_table.py::test_dispatches_pinned_to_block
#[tokio::test]
async fn all_102_source_samples_dispatch_at_the_client_head_and_snapshot_90() -> TestResult {
    let samples = samples()?;
    let (client, chain) = client(seeded_state()?).await?;
    let outcome = async {
        let snapshot = client.at(BlockRef::Number(90)).await?;
        let mut failures = Vec::new();
        let mut observed = BTreeSet::new();
        for name in ReadName::ALL {
            let args = samples.get(name.as_str()).ok_or("source sample missing")?;
            observed.insert(name.as_str());
            let head = RecordingView::new(client.clone());
            if let Err(error) = dispatch(&head, name, args).await {
                failures.push(format!("{name} at head: {error}"));
            }
            let pinned = RecordingView::new(snapshot.clone());
            if let Err(error) = dispatch(&pinned, name, args).await {
                failures.push(format!("{name} at snapshot90: {error}"));
            }
            // block_info has its own explicit block=100 argument in the exact
            // source sample, including when invoked through snapshot90.
            if name != ReadName::BlockInfo {
                for hash in pinned.block_hashes() {
                    assert_eq!(hash, snapshot.block().hash, "{name}: escaped snapshot90");
                }
            }
        }
        assert_eq!(observed.len(), 102);
        assert!(
            failures.is_empty(),
            "catalog dispatch failures:\n{}",
            failures.join("\n")
        );
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        assert!(
            chain
                .requests()
                .iter()
                .all(|request| !request.method.starts_with("author_"))
        );
        Ok(())
    }
    .await;
    client.close().await;
    outcome
}

// pyt: unit/test_reads_table.py::TestReadSemantics::test_balance_is_tao_denominated
// pyt: unit/test_reads_table.py::TestReadSemantics::test_scalar_read_casts_to_int
// pyt: unit/test_reads_table.py::TestReadSemantics::test_uid_read_none_when_unregistered
// pyt: unit/test_reads_table.py::TestReadSemantics::test_block_time_from_slot_duration
#[tokio::test]
async fn source_semantic_spots_keep_exact_amount_scalar_absence_and_slot_duration() -> TestResult {
    let samples = samples()?;
    let coldkey = samples["balance"]["coldkey_ss58"]
        .as_str()
        .ok_or("balance coldkey")?;
    let hotkey = samples["uid"]["hotkey_ss58"].as_str().ok_or("uid hotkey")?;
    let mut state = seeded_state()?;
    state.seed_python(
        "System",
        "Account",
        &[json!(coldkey)],
        json!({"data":{"free":1_500_000_000}}),
    )?;
    state.seed_python("SubtensorModule", "TxRateLimit", &[], json!(1000))?;
    state.seed_constant("Aura", "SlotDuration", json!(250))?;
    let entry = state
        .runtime
        .storage_entry("SubtensorModule", "Uids")
        .ok_or("Uids metadata")?;
    let key = state
        .runtime
        .storage_key(entry, &[CoreValue::Int(1), CoreValue::Str(hotkey.into())])?;
    state.seed_encoded(key, None);
    let (client, chain) = client(state).await?;
    let outcome = async {
        let amount = owner::balance(&client, coldkey.parse::<AccountId>()?).await?;
        assert_eq!(amount.rao, 1_500_000_000);
        assert_eq!(amount.netuid, NetUid::ROOT);
        assert_eq!(
            dispatch(&client, ReadName::Balance, &samples["balance"]).await?,
            json!("τ1.500000000")
        );
        assert_eq!(
            dispatch(&client, ReadName::TxRateLimit, &samples["tx_rate_limit"]).await?,
            json!(1000)
        );
        assert_eq!(
            dispatch(&client, ReadName::Uid, &samples["uid"]).await?,
            Value::Null
        );
        assert_eq!(
            dispatch(&client, ReadName::BlockTime, &samples["block_time"]).await?,
            json!(0.25)
        );
        assert_eq!(
            dispatch(&client, ReadName::IsFastBlocks, &samples["is_fast_blocks"]).await?,
            json!(true)
        );
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        Ok(())
    }
    .await;
    client.close().await;
    outcome
}
