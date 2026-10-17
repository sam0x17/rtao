//! Native replay of immutable vectors generated only in the oracle CI lane.

use anyhow::{Context, Result, ensure};
use rtao_client::{BlockRef, Client, ClientConfig, Network, View};
use rtao_intents::AnyIntent;
use rtao_primitives::{AccountId, CryptoType, Spend};
use rtao_signer::{
    BoxFuture, DynKeySource, KeyView, KeypairSigner, Signer, SignerError, SignerRole,
};
use rtao_testkit::{FakeChain, FakeState, RecordingView, rpc::RpcClient};
use rtao_tx::{BuildCx, Executor, OriginView, PlanOptions, PreflightCx, SystemClock};
use serde_json::{Value, json};
use std::{collections::BTreeSet, sync::Arc};

struct DevWallet {
    cold: KeypairSigner,
    hot: KeypairSigner,
}
impl DevWallet {
    fn new() -> Result<Self> {
        Ok(Self {
            cold: KeypairSigner::from_uri("//Alice", CryptoType::Sr25519)?,
            hot: KeypairSigner::from_uri("//Alice//hot", CryptoType::Sr25519)?,
        })
    }
}
impl KeyView for DevWallet {
    fn hotkey_address(&self) -> Result<AccountId, SignerError> {
        Ok(self.hot.account())
    }
    fn hotkey_public_key(&self) -> Result<[u8; 32], SignerError> {
        Ok(self.hot.account().0)
    }
    fn coldkey_address(&self) -> Result<AccountId, SignerError> {
        Ok(self.cold.account())
    }
}
impl DynKeySource for DevWallet {
    fn signer_for<'a>(
        &'a self,
        role: SignerRole,
    ) -> BoxFuture<'a, Result<Arc<dyn Signer>, SignerError>> {
        Box::pin(async move {
            Ok(match role {
                SignerRole::Coldkey => Arc::new(self.cold.clone()),
                SignerRole::Hotkey => Arc::new(self.hot.clone()),
            } as Arc<dyn Signer>)
        })
    }
}

fn text<'a>(value: &'a Value, name: &str) -> Result<&'a str> {
    value[name]
        .as_str()
        .with_context(|| format!("missing string {name}"))
}

// CI records Python's container/name spelling; FakeState deliberately uses
// pallet/item and api/method at its public native boundary.
fn native_seed(runtime: &rtao_core::runtime::Runtime, source: &Value) -> Result<Value> {
    let mut seed = json!({"block":source["block"],"finalized":source["finalized"],"fee":source["fee_rao"],"weight":source["weight"],"nonces":source["nonces"]});
    for section in ["storage", "storage_defaults", "constants", "runtime"] {
        let rows = source[section]
            .as_array()
            .with_context(|| format!("missing seed {section}"))?;
        let mut native = Vec::new();
        for row in rows {
            let container = text(row, "container")?;
            let name = text(row, "name")?;
            let exists = match section {
                "storage" | "storage_defaults" => runtime.storage_entry(container, name).is_some(),
                "constants" => runtime.constant(container, name).is_some(),
                _ => runtime.apis.iter().any(|api| {
                    api.name == container && api.methods.iter().any(|method| method.name == name)
                }),
            };
            if !exists {
                // The upstream table seeds historical storage defaults and
                // runtime APIs absent from live metadata. Preserve those in
                // FakeState's source tables; invoking them still fails native
                // metadata lookup rather than returning invented bytes.
                ensure!(
                    matches!(section, "storage_defaults" | "runtime"),
                    "unknown explicit {section} seed {container}.{name}"
                );
                continue;
            }
            let mut row = row.as_object().context("seed entry object")?.clone();
            row.remove("container");
            row.remove("name");
            if section == "runtime" {
                row.insert("api".into(), json!(container));
                row.insert("method".into(), json!(name));
            } else {
                row.insert("pallet".into(), json!(container));
                row.insert("item".into(), json!(name));
            }
            // The upstream fake uses scalar zero as an empty LastUpdate
            // sentinel. The native fake uses SCALE's empty sequence.
            if matches!(section, "storage" | "storage_defaults")
                && name == "LastUpdate"
                && row["value"].as_u64() == Some(0)
            {
                row.insert("value".into(), json!([]));
            }
            native.push(Value::Object(row));
        }
        seed[section] = json!(native);
    }
    Ok(seed)
}

pub(super) fn seeded_state(source: &Value, vector: &Value) -> Result<FakeState> {
    let object = source.as_object().context("full Python seed state")?;
    let fields = [
        "storage",
        "storage_defaults",
        "storage_maps",
        "constants",
        "runtime",
        "fee_rao",
        "weight",
        "block",
        "finalized",
        "spec_version",
        "token_symbols",
        "nonces",
        "events",
    ];
    ensure!(
        object.len() == fields.len() && fields.iter().all(|field| object.contains_key(*field)),
        "Python seed fields differ from the complete recorded schema"
    );
    let runtime = FakeState::try_default()?.runtime;
    let spec = u32::try_from(
        source["spec_version"]
            .as_u64()
            .context("seed spec version")?,
    )?;
    let runtime = Arc::new(rtao_core::runtime::Runtime::parse(
        &runtime.metadata_bytes,
        spec,
        runtime.transaction_version,
        runtime.ss58_format,
    )?);
    let mut seed = native_seed(&runtime, source)?;
    // A null point seed is a storage miss, including for Optional map entries.
    let rows = seed["storage"]
        .as_array_mut()
        .context("storage seed array")?;
    let missing: Vec<_> = rows
        .iter()
        .filter(|row| row["value"].is_null())
        .cloned()
        .collect();
    rows.retain(|row| !row["value"].is_null());
    let mut state = FakeState::from_python_seed(runtime, &seed)?;
    for section in ["storage_defaults", "constants", "runtime"] {
        let table = match section {
            "storage_defaults" => &mut state.python_storage_defaults,
            "constants" => &mut state.python_constant_defaults,
            _ => &mut state.python_runtime_defaults,
        };
        table.clear();
        for row in source[section]
            .as_array()
            .context("recorded source table")?
        {
            table.insert(
                (
                    text(row, "container")?.to_owned(),
                    text(row, "name")?.to_owned(),
                ),
                row["value"].clone(),
            );
        }
    }
    for row in missing {
        let info = state
            .runtime
            .storage_entry(text(&row, "pallet")?, text(&row, "item")?)
            .context("missing storage entry")?;
        let params = row["params"]
            .as_array()
            .context("storage params")?
            .iter()
            .map(rtao_testkit::fakes::json_to_core)
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let key = state.runtime.storage_key(info, &params)?;
        state.seed_encoded(key, None);
    }
    for row in source["storage_maps"]
        .as_array()
        .context("storage map seeds")?
    {
        let container = text(row, "container")?;
        let name = text(row, "name")?;
        for pair in row["value"].as_array().context("storage map pairs")? {
            let pair = pair
                .as_array()
                .filter(|pair| pair.len() == 2)
                .context("map pair must contain key and value")?;
            let mut params = Vec::new();
            if name == "Weights" && container == "SubtensorModule" {
                let netuid = vector["params"]["netuid"]
                    .as_u64()
                    .context("weights netuid")?;
                let mechid = vector["params"]
                    .get("mechid")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                params.push(json!(
                    mechid
                        .checked_mul(4096)
                        .and_then(|value| value.checked_add(netuid))
                        .context("mechanism storage index overflow")?
                ));
            }
            match &pair[0] {
                Value::Array(keys) => params.extend(keys.clone()),
                key => params.push(key.clone()),
            }
            state.seed_python(container, name, &params, pair[1].clone())?;
        }
    }
    ensure!(
        source["token_symbols"]
            .as_object()
            .is_some_and(|symbols| symbols.is_empty()),
        "nonempty display-symbol seeds need a native adapter"
    );
    ensure!(
        source["events"]
            .as_object()
            .is_some_and(|events| events.is_empty()),
        "eventful seed requires an explicit native adapter"
    );
    Ok(state)
}

pub(super) type Hits = BTreeSet<(String, String, String)>;

fn observed(view: &RecordingView<rtao_client::Snapshot>) -> Hits {
    view.touches()
        .into_iter()
        .map(|touch| {
            (
                match touch.kind {
                    rtao_client::ReadKind::Storage => "storage",
                    rtao_client::ReadKind::RuntimeApi => "runtime",
                    rtao_client::ReadKind::Constant => "constant",
                }
                .to_owned(),
                touch.pallet_or_api,
                touch.item,
            )
        })
        .collect()
}

fn expected_hits(vector: &Value) -> Result<Hits> {
    vector["touched"]
        .as_array()
        .context("oracle touched record")?
        .iter()
        .map(|touch| {
            Ok((
                text(touch, "kind")?.to_owned(),
                text(touch, "container")?.to_owned(),
                text(touch, "name")?.to_owned(),
            ))
        })
        .collect()
}

async fn client(state: FakeState) -> Result<(Client, FakeChain)> {
    let fake = FakeChain::from_state(state);
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    let client = Client::from_rpc(RpcClient::new(fake.clone()), config).await?;
    Ok((client, fake))
}

pub(super) async fn read(vector: &Value, seed: &Value) -> Result<Hits> {
    let (client, fake) = client(seeded_state(seed, vector)?).await?;
    let result = async {
        let block = match &vector["block"] {
            Value::Null => BlockRef::Best,
            value => BlockRef::Number(value.as_u64().context("read block")?),
        };
        let view = RecordingView::new(client.at(block).await?);
        let selected = view.inner().block().hash;
        let before = fake.requests().len();
        let actual = rtao_reads::dispatch(
            &view,
            text(vector, "name")?.parse()?,
            vector["params"].as_object().context("read parameters")?,
        )
        .await?;
        rtao_primitives::json::compare(&vector["result"], &actual).with_context(|| {
            format!(
                "read result: expected {}, actual {actual}",
                vector["result"]
            )
        })?;
        let hits = observed(&view);
        ensure!(
            hits == expected_hits(vector)?,
            "read descriptor hit set differs: Python {:?}, Rust {hits:?}",
            expected_hits(vector)?
        );
        ensure!(
            view.touches()
                .iter()
                .all(|touch| touch.at == Some(selected)),
            "native read touches an unselected block"
        );
        for request in &fake.requests()[before..] {
            if [
                "state_getStorage",
                "state_queryStorageAt",
                "state_call",
                "state_getKeysPaged",
            ]
            .contains(&request.method.as_str())
            {
                ensure!(
                    request.params.as_array().and_then(|params| params.last())
                        == Some(&json!(format!("0x{}", hex::encode(selected.0)))),
                    "read {} used an unselected hash: {request:?}",
                    text(vector, "name")?
                );
            }
        }
        Ok(hits)
    }
    .await;
    client.close().await;
    result
}

fn encoded_oracle_call(
    runtime: &rtao_core::runtime::Runtime,
    expected: &Value,
    actual: &rtao_runtime::CallBytes,
) -> Result<Vec<u8>> {
    let module = text(expected, "module")?;
    let function = text(expected, "function")?;
    let mut params = expected["params"]
        .as_object()
        .context("oracle call params")?
        .clone();
    if function.contains("timelocked") {
        let decoded = runtime.decode_call_value(&mut rtao_core::codec::decode::Cursor::new(
            actual.as_bytes(),
        ))?;
        let decoded = rtao_core::codec::value::to_corpus_json(&decoded);
        ensure!(
            decoded["call_module"] == module && decoded["call_function"] == function,
            "timelock call identity differs"
        );
        for argument in decoded["call_args"]
            .as_array()
            .context("decoded call args")?
        {
            let name = text(argument, "name")?;
            if matches!(name, "commit" | "reveal_round") {
                ensure!(
                    params.get(name) == Some(&json!({"excluded":"randomized-or-clock-derived"})),
                    "unreviewed timelock mask for {name}"
                );
                params.insert(name.into(), argument["value"].clone());
            }
        }
    }
    Ok(runtime.compose_call(
        module,
        function,
        &rtao_testkit::fakes::json_to_core(&Value::Object(params))?,
    )?)
}

fn mask_extras(extras: &serde_json::Map<String, Value>) -> Value {
    let mut extras = extras.clone();
    if extras.contains_key("reveal_round") {
        extras.insert(
            "reveal_round".into(),
            json!({"excluded":"randomized-or-clock-derived"}),
        );
    }
    Value::Object(extras)
}

/// Replay pure beacon vectors through the actual public timelock helpers.
pub(super) fn timelock(document: &Value) -> Result<()> {
    use rtao_primitives::drand::{reveal_time, round_at};
    let rounds = document["round_at"]
        .as_array()
        .context("round_at vectors")?;
    ensure!(rounds.len() >= 4, "timelock round coverage missing");
    for row in rounds {
        let stamp = row["unix"]
            .as_u64()
            .context("timelock integer Unix timestamp")?;
        let round = round_at(stamp).context("round_at native helper")?;
        rtao_primitives::json::compare(&row["result"], &json!({"ok":round}))
            .context("round_at native helper result")?;
    }
    let times = document["reveal_time"]
        .as_array()
        .context("reveal_time vectors")?;
    ensure!(times.len() >= 4, "timelock reveal time coverage missing");
    for row in times {
        let round = row["round"].as_u64().context("beacon round")?;
        let actual = json!({"ok":reveal_time(round)
            .context("reveal_time native helper")?.to_rfc3339()});
        rtao_primitives::json::compare(&row["result"], &actual)
            .context("reveal_time native helper result")?;
    }
    Ok(())
}

pub(super) async fn intent(vector: &Value, seed: &Value) -> Result<()> {
    let name = text(vector, "name")?;
    let (client, fake) = client(seeded_state(seed, vector)?).await?;
    let result = async {
        let keys = DevWallet::new()?;
        let intent = AnyIntent::from_tool(name, vector["args"].clone())?;
        let catalog: Value = serde_json::from_str(intent.spec().catalog_json)?;
        compare("input schema", &vector["schema"], &catalog["input_schema"])?;
        compare("constructor/default JSON", &vector["intent"], &intent.to_json())?;
        let view = RecordingView::new(client.at(BlockRef::Best).await?);
        let runtime = view.runtime().await?;
        let build: BuildCx<dyn rtao_client::DynView> = BuildCx {
            view: &view,
            origin: OriginView::Keys(&keys),
            runtime: &runtime,
            clock: &SystemClock,
            spec: intent.spec(),
        };
        let built = intent.as_dyn().build_dyn(&build).await?;
        ensure!(
            encoded_oracle_call(runtime.core(), &vector["call"], &built.call)? == built.call.as_bytes(),
            "{name} composed call bytes differ; expected call {} and actual bytes 0x{}",
            vector["call"], hex::encode(built.call.as_bytes())
        );
        compare("build extras", &vector["extras"], &mask_extras(&built.extras))?;
        let address = if intent.spec().signer == "hotkey" { keys.hot.account() } else { keys.cold.account() };
        let preflight: PreflightCx<dyn rtao_client::DynView> = PreflightCx {
            view: &view,
            runtime: &runtime,
            clock: &SystemClock,
            spec: intent.spec(),
            client: &client,
            dispatch_origin: address,
            fee_payer: address,
            call: &built.call,
        };
        let preflight = intent.as_dyn().preflight_dyn(&preflight).await?;
        let display = |balance: Option<rtao_primitives::Balance<rtao_primitives::Tao>>|
            balance.map(|value| rtao_primitives::DynBalance::from(value).to_string());
        let preflight = json!({
            "effects":preflight.effects, "warnings":preflight.warnings, "blocks":preflight.blocks,
            "required_free":display(preflight.required_free), "available_free":display(preflight.available_free),
            "estimated_fee":display(preflight.estimated_fee), "facts":preflight.facts,
            "facts_title":preflight.facts_title.as_str()
        });
        compare("preflight hooks", &vector["preflight"], &preflight)?;
        let expected = expected_hits(vector)?;
        let actual = observed(&view);
        ensure!(expected == actual, "{name} descriptor hit set differs: Python {expected:?}, Rust {actual:?}");
        ensure!(view.touches().iter().all(|touch| touch.at == Some(view.inner().block().hash)), "{name} used an unselected state block");
        let spend = match intent.as_dyn().spend_dyn() {
            Spend::None => Value::Null,
            Spend::Unbounded => json!({"unbounded":true}),
            Spend::Bounded(value) => json!(rtao_primitives::DynBalance::from(value).to_string()),
        };
        compare("spend semantics", &vector["spend"], &spend)?;
        let plan = Executor::new(client.clone()).plan_dyn(intent.as_dyn(), &keys, &PlanOptions::default()).await?;
        compare("executor plan", &vector["plan"], &plan.to_json())?;
        ensure!(vector["fee_rao"].as_u64() == plan.fee.map(|fee| fee.rao()), "native fee rao differs");
        ensure!(fake.requests().iter().all(|request| !request.method.starts_with("author_")), "oracle replay must not submit");
        Ok::<_, anyhow::Error>(())
    }.await;
    client.close().await;
    result
}

fn compare(label: &str, expected: &Value, actual: &Value) -> Result<()> {
    rtao_primitives::json::compare(expected, actual)
        .with_context(|| format!("{label}: expected {expected}, actual {actual}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rtao_testkit::fixtures::catalog_samples::{
        ROOT_NATIVE_AMOUNT_ID, root_native_amount_call, root_native_amount_inputs,
    };

    #[test]
    fn timelock_replay_uses_public_helpers_and_rejects_changed_results() -> Result<()> {
        let vectors = json!({
            "round_at":[
                {"unix":1_692_803_367u64,"result":{"ok":1}},
                {"unix":1_692_803_368u64,"result":{"ok":2}},
                {"unix":1_692_803_370u64,"result":{"ok":2}},
                {"unix":1_692_803_371u64,"result":{"ok":3}}
            ],
            "reveal_time":[
                {"round":0,"result":{"ok":"2023-08-23T15:09:24+00:00"}},
                {"round":1,"result":{"ok":"2023-08-23T15:09:27+00:00"}},
                {"round":2,"result":{"ok":"2023-08-23T15:09:30+00:00"}},
                {"round":3,"result":{"ok":"2023-08-23T15:09:33+00:00"}}
            ]
        });
        timelock(&vectors)?;
        let mut changed_round = vectors.clone();
        changed_round["round_at"][1]["result"]["ok"] = json!(3);
        ensure!(timelock(&changed_round).is_err());
        let mut changed_time = vectors;
        changed_time["reveal_time"][1]["result"]["ok"] = json!("2023-08-23T15:09:28+00:00");
        ensure!(timelock(&changed_time).is_err());
        Ok(())
    }

    fn workspace() -> Result<&'static std::path::Path> {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .find(|path| path.join("upstream.toml").is_file())
            .context("workspace root")
    }

    fn baseline() -> Result<Value> {
        let state = FakeState::try_default()?;
        let rows = |table: std::collections::BTreeMap<(String, String), Value>| {
            table.into_iter().map(|((container,name),value)|json!({"container":container,"name":name,"value":value})).collect::<Vec<_>>()
        };
        Ok(
            json!({"block":100,"finalized":100,"spec_version":300,"storage":[],"storage_maps":[],"storage_defaults":rows(state.python_storage_defaults),"constants":rows(state.python_constant_defaults),"runtime":rows(state.python_runtime_defaults),"token_symbols":{},"events":{},"nonces":{},"fee_rao":124414,"weight":{"ref_time":1000000,"proof_size":3593}}),
        )
    }

    fn apply(source: &mut Value, sample: &Value) -> Result<()> {
        for section in [
            "storage",
            "storage_maps",
            "storage_defaults",
            "constants",
            "runtime",
        ] {
            for row in sample[section].as_array().into_iter().flatten() {
                let entries = source[section].as_array_mut().context("seed section")?;
                entries.retain(|old| {
                    old["container"] != row["container"]
                        || old["name"] != row["name"]
                        || (section == "storage" && old["params"] != row["params"])
                });
                entries.push(row.clone());
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn reviewed_read_inputs_encode_and_dispatch_at_both_blocks() -> Result<()> {
        let extras: Value = serde_json::from_slice(&std::fs::read(
            workspace()?.join("vendor/oracle/samples_extra.json"),
        )?)?;
        for (name, samples) in extras["reads"]
            .as_object()
            .context("reviewed read inputs")?
        {
            for sample in samples.as_array().context("reviewed samples")? {
                let mut source = baseline()?;
                apply(&mut source, sample)?;
                let vector = json!({"name":name,"params":sample["params"]});
                for block in [100, 90] {
                    let (client, _) = client(
                        seeded_state(&source, &vector)
                            .with_context(|| format!("encode {name}/{}", sample["id"]))?,
                    )
                    .await?;
                    let result = async {
                        let view = RecordingView::new(client.at(BlockRef::Number(block)).await?);
                        let name: rtao_reads::ReadName = name.parse()?;
                        rtao_reads::dispatch(
                            &view,
                            name,
                            vector["params"].as_object().context("read params")?,
                        )
                        .await?;
                        ensure!(
                            view.touches().iter().all(|touch| name
                                .spec()
                                .hits
                                .iter()
                                .any(|hit| hit.matches(touch))),
                            "reviewed branch exceeds native H"
                        );
                        ensure!(
                            view.block_hashes().len() == 1,
                            "reviewed read mixes block identities"
                        );
                        Ok::<_, anyhow::Error>(())
                    }
                    .await;
                    client.close().await;
                    result
                        .with_context(|| format!("dispatch {name}/{} at {block}", sample["id"]))?;
                }
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn reviewed_intent_inputs_build_and_plan_without_oracle_outputs() -> Result<()> {
        let extras: Value = serde_json::from_slice(&std::fs::read(
            workspace()?.join("vendor/oracle/samples_extra.json"),
        )?)?;
        root_native_amount_inputs(&extras)?;
        for (name, samples) in extras["intents"]
            .as_object()
            .context("reviewed intent inputs")?
        {
            for sample in samples.as_array().context("reviewed samples")? {
                let mut source = baseline()?;
                apply(&mut source, sample)?;
                let root_expected = (sample["id"] == ROOT_NATIVE_AMOUNT_ID)
                    .then(|| root_native_amount_call(name, &sample["args"]))
                    .transpose()?;
                let (client, fake) = client(seeded_state(&source, &json!({"name":name}))?).await?;
                let result = async {
                    let intent = AnyIntent::from_tool(name, sample["args"].clone())?;
                    let keys = DevWallet::new()?;
                    let view = RecordingView::new(client.at(BlockRef::Best).await?);
                    let runtime = view.runtime().await?;
                    let cx: BuildCx<dyn rtao_client::DynView> = BuildCx {
                        view: &view,
                        origin: OriginView::Keys(&keys),
                        runtime: &runtime,
                        clock: &SystemClock,
                        spec: intent.spec(),
                    };
                    let built = intent.as_dyn().build_dyn(&cx).await?;
                    let plan = Executor::new(client.clone())
                        .plan_dyn(intent.as_dyn(), &keys, &PlanOptions::default())
                        .await?;
                    if let Some(expected) = &root_expected {
                        ensure!(
                            plan.ok(),
                            "root input has unrelated plan blocks: {:?}",
                            plan.violations
                        );
                        ensure!(plan.op == *name, "root input operation changed");
                        ensure!(
                            built.call.as_bytes() == expected && plan.call().as_bytes() == expected,
                            "original root input {name} changed independent native call arguments"
                        );
                    }
                    ensure!(
                        fake.requests()
                            .iter()
                            .all(|request| !request.method.starts_with("author_")),
                        "input validation submitted a transaction"
                    );
                    Ok::<_, anyhow::Error>(())
                }
                .await;
                client.close().await;
                result.with_context(|| format!("build/plan {name}/{}", sample["id"]))?;
            }
        }
        Ok(())
    }
}
