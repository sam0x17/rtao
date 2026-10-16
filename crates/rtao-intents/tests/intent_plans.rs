// Derived from RaoFoundation/subtensor sdk/python/tests/harness/samples.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/tests/unit/test_intents_table.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Ordinary M5 planning coverage from the authenticated upstream sample table.
//! Every operation uses the real executor and encoded FakeChain state.
#![cfg(all(feature = "all", feature = "pow"))]

use anyhow::{Context, Result, bail, ensure};
use rtao_catalog::{INTENTS, IntentSpec};
use rtao_client::{Client, ClientConfig, Network};
use rtao_core::{
    codec::Value as CoreValue,
    ir::MetadataIr,
    runtime::{Runtime, TypeSpec},
};
use rtao_intents::AnyIntent;
use rtao_primitives::{AccountId, DefaultAccount, H256, NetUid, TaoBalance};
use rtao_runtime::{RuntimeHandle, models::CrowdloanInfo, storage::subtensor_module as st};
use rtao_signer::{BoxFuture, DynKeySource, KeyView, Signer, SignerError, SignerRole};
use rtao_testkit::{FakeChain, FakeState, fixtures::decode_fixed_hex, rpc::RpcClient};
use rtao_tx::{Clock, Executor, PlanOptions};
use serde_json::{Map, Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, SystemTime},
};

const SAMPLE_PATH: &str = "sdk/python/tests/harness/samples.py";
const SUBNET: NetUid = NetUid(7);
const DEST_SUBNET: NetUid = NetUid(8);
const COLD: AccountId = AccountId([1; 32]);
const HOT: AccountId = AccountId([2; 32]);
const OTHER_HOT: AccountId = AccountId([3; 32]);
const FREE_RAO: u64 = 1_000_000_000_000_000_000;

struct FixtureClock;
impl Clock for FixtureClock {
    fn now(&self) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000)
    }
}

// There are no secret keys in this fixture. An accidental acquisition attempt
// fails the test immediately instead of returning a development signer.
#[derive(Default)]
struct PublicKeys {
    acquisitions: AtomicUsize,
}

impl KeyView for PublicKeys {
    fn hotkey_address(&self) -> Result<AccountId, SignerError> {
        Ok(HOT)
    }
    fn hotkey_public_key(&self) -> Result<[u8; 32], SignerError> {
        Ok(HOT.0)
    }
    fn coldkey_address(&self) -> Result<AccountId, SignerError> {
        Ok(COLD)
    }
}

impl DynKeySource for PublicKeys {
    #[allow(clippy::panic)] // This sentinel must fail if planning touches private keys.
    fn signer_for<'a>(
        &'a self,
        role: SignerRole,
    ) -> BoxFuture<'a, Result<Arc<dyn Signer>, SignerError>> {
        self.acquisitions.fetch_add(1, Ordering::SeqCst);
        panic!("planning attempted to acquire a private {role} signer");
    }
}

// The extractor preserves Python dictionaries as ordered key/value pairs.
// Decode those literal carriers, including nested batch and multisig calls;
// never manufacture a second handwritten copy of INTENT_SAMPLES.
fn literal(value: &Value) -> Result<Value> {
    match value {
        Value::Array(values) => values
            .iter()
            .map(literal)
            .collect::<Result<_>>()
            .map(Value::Array),
        Value::Object(tagged) if tagged.len() == 1 && tagged.contains_key("dict") => {
            let mut result = Map::new();
            for pair in tagged["dict"].as_array().context("Python dict pairs")? {
                let pair = pair.as_array().context("Python dict pair")?;
                ensure!(pair.len() == 2, "Python dict pair must have two members");
                let key = pair[0].as_str().context("sample dictionary string key")?;
                ensure!(
                    result.insert(key.into(), literal(&pair[1])?).is_none(),
                    "duplicate sample key {key}"
                );
            }
            Ok(Value::Object(result))
        }
        Value::Object(tagged) if tagged.len() == 1 && tagged.contains_key("tuple") => {
            literal(&tagged["tuple"])
        }
        Value::Object(_) => bail!("unknown extracted sample literal carrier"),
        value => Ok(value.clone()),
    }
}

fn samples() -> Result<BTreeMap<String, Value>> {
    let table = rtao_catalog::py::table(SAMPLE_PATH, "INTENT_SAMPLES")
        .context("authenticated INTENT_SAMPLES")?;
    let value = literal(&rtao_primitives::json::from_str::<Value>(table.value_json)?)?;
    Ok(value
        .as_object()
        .context("INTENT_SAMPLES dictionary")?
        .iter()
        .map(|(name, args)| (name.clone(), args.clone()))
        .collect())
}

fn scenario_netuid(value: &Value) -> Result<Value> {
    match value.as_u64().context("sample netuid")? {
        0 => Ok(json!(0)),
        1 => Ok(json!(SUBNET.0)),
        2 => Ok(json!(DEST_SUBNET.0)),
        other => bail!("unreviewed sample subnet {other}"),
    }
}

fn rewrite(args: &mut Value) -> Result<()> {
    match args {
        Value::Object(fields) => {
            for (name, value) in fields {
                if name == "netuid" || name.ends_with("_netuid") {
                    *value = scenario_netuid(value)?;
                } else if name == "netuids" || name == "subnets" {
                    for netuid in value.as_array_mut().context("sample subnet list")? {
                        *netuid = scenario_netuid(netuid)?;
                    }
                } else if name == "hotkey_ss58" || name.ends_with("_hotkey_ss58") {
                    *value = json!(HOT.to_string());
                } else {
                    rewrite(value)?;
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                rewrite(value)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn scenario_args(op: &str, original: &Value) -> Result<Value> {
    let mut args = original.clone();
    rewrite(&mut args)?;
    // This operation expressly needs two different hotkeys. The common slot
    // rewrite above must preserve that constructor invariant.
    if op == "move_swap_stake" {
        args["dest_hotkey_ss58"] = json!(OTHER_HOT.to_string());
    }
    Ok(args)
}

fn preview(hotkey: AccountId) -> Value {
    json!({"hotkey":hotkey.to_string(), "owed_shares":1,
        "accrued_tao":10_000_000, "redeemable_tao":10_000_000,
        "forfeited_tao_est":0, "rows":1, "rows_to_sell":1,
        "dust_rows":0, "swept":0, "flushed_credits":0})
}

fn seed_availability(state: &mut FakeState) -> Result<()> {
    // Runtime BTreeMap<NetUid, _> keys are integers, not JSON object strings.
    let row = CoreValue::record(vec![
        ("total".into(), CoreValue::Int(10_000_000_000)),
        ("locked".into(), CoreValue::Int(0)),
        ("available".into(), CoreValue::Int(10_000_000_000)),
    ]);
    let value = CoreValue::Dict(vec![(
        CoreValue::Str(COLD.to_string()),
        CoreValue::Dict(
            [NetUid::ROOT, SUBNET, DEST_SUBNET]
                .into_iter()
                .map(|netuid| (CoreValue::Int(i128::from(netuid.0)), row.clone()))
                .collect(),
        ),
    )]);
    let api = "StakeInfoRuntimeApi";
    let method = "get_stake_availability_for_coldkeys";
    let signature = rtao_core::ops::runtime_api::runtime_api_method(&state.runtime, api, method)?;
    let output = state
        .runtime
        .encode_spec(&TypeSpec::Id(signature.output), &value)?;
    let inputs = [NetUid::ROOT, SUBNET, DEST_SUBNET]
        .into_iter()
        .map(|netuid| {
            rtao_core::ops::runtime_api::encode_params(
                &state.runtime,
                api,
                signature,
                &[
                    CoreValue::List(vec![CoreValue::Bytes(COLD.0.to_vec())]),
                    CoreValue::List(vec![CoreValue::Int(i128::from(netuid.0))]),
                ],
            )
        })
        .collect::<std::result::Result<Vec<_>, _>>()?;
    for input in inputs {
        state.seed_runtime_encoded(
            "StakeInfoRuntimeApi_get_stake_availability_for_coldkeys",
            input,
            output.clone(),
        );
    }
    Ok(())
}

fn seeded_state(runtime: Arc<Runtime>, op: &str) -> Result<FakeState> {
    let mut state = FakeState::new(runtime);
    state.load_python_defaults()?;
    state.seed_default("System", "Account", json!({"data":{"free":FREE_RAO}}))?;
    state.seed(&st::ADMIN_FREEZE_WINDOW, (), 0u16)?;
    state.seed(&st::OWNER_HYPERPARAM_RATE_LIMIT, (), 0u16)?;
    state.seed(&st::ROOT_STAKE_UNLOCK_INTERVAL, (), 0u64)?;
    for netuid in [NetUid::ROOT, SUBNET, DEST_SUBNET] {
        state.seed(&st::NETWORKS_ADDED, (netuid,), true)?;
        state.seed(&st::SUBNET_OWNER, (netuid,), DefaultAccount::from(COLD))?;
        state.seed(&st::TEMPO, (netuid,), 360u16)?;
        state.seed(&st::LAST_EPOCH_BLOCK, (netuid,), 100u64)?;
        state.seed(&st::PENDING_EPOCH_AT, (netuid,), 0u64)?;
        // The default consensus enum can be Null in authentic metadata.
        // Supplying its sole validator permit also admits the weights samples.
        state.seed_python(
            "SubtensorModule",
            "ValidatorPermit",
            &[json!(netuid.0)],
            json!([true]),
        )?;
    }
    state.seed(&st::STAKING_HOTKEYS, (COLD,), vec![HOT, OTHER_HOT])?;
    state.seed_runtime(
        "StakeInfoRuntimeApi",
        "get_stake_info_for_coldkey",
        json!([
            {"coldkey":COLD.to_string(), "hotkey":HOT.to_string(), "netuid":0, "stake":10_000_000_000u64},
            {"coldkey":COLD.to_string(), "hotkey":HOT.to_string(), "netuid":SUBNET.0, "stake":10_000_000_000u64}
        ]),
    )?;
    state.seed_runtime(
        "StakeInfoRuntimeApi",
        "get_stake_info_for_hotkey_coldkey_netuid",
        json!({"coldkey":COLD.to_string(), "hotkey":HOT.to_string(), "netuid":SUBNET.0, "stake":10_000_000_000u64}),
    )?;
    state.seed_runtime(
        "SwapRuntimeApi",
        "current_alpha_price",
        json!(1_000_000_000u64),
    )?;
    state.seed_runtime(
        "SwapRuntimeApi",
        "sim_swap_tao_for_alpha",
        json!({"alpha_amount":1_000_000_000u64}),
    )?;
    state.seed_runtime(
        "SwapRuntimeApi",
        "sim_swap_alpha_for_tao",
        json!({"tao_amount":1_000_000_000u64}),
    )?;
    state.seed_runtime(
        "BetaBasketRuntimeApi",
        "get_validator_basket",
        json!([[SUBNET.0, 10_000_000_000u64, 1_000_000_000u64]]),
    )?;
    state.seed_runtime(
        "BetaBasketRuntimeApi",
        "get_basket_claim_preview",
        preview(HOT),
    )?;
    state.seed_runtime(
        "BetaBasketRuntimeApi",
        "get_root_basket_claim_previews",
        json!([preview(HOT)]),
    )?;
    state.seed_runtime(
        "BetaBasketRuntimeApi",
        "get_basket_payout",
        json!(10_000_000),
    )?;
    seed_availability(&mut state)?;
    if op == "finalize_crowdloan" {
        // The sample finalizes ID 0. Its target-only contract is explicit;
        // absence or an opaque callback no longer inherits a no-spend label.
        state.seed_typed(
            "Crowdloan",
            "Crowdloans",
            (0u32,),
            CrowdloanInfo {
                creator: COLD,
                deposit: TaoBalance(1),
                min_contribution: TaoBalance(1),
                end: 100,
                cap: TaoBalance(10),
                funds_account: OTHER_HOT,
                raised: TaoBalance(10),
                target_address: Some(COLD),
                call: None,
                finalized: false,
                contributors_count: 1,
            },
        )?;
    }
    if op == "pow_register" {
        // Uids is optional *storage*, so an absent key is not an encoded Option.
        // Override the upstream fake's blanket Uids=0 for this unregistered slot.
        let entry = state
            .runtime
            .storage_entry("SubtensorModule", "Uids")
            .context("Uids storage")?;
        let key = state.runtime.storage_key(
            entry,
            &[
                CoreValue::Int(i128::from(SUBNET.0)),
                CoreValue::Bytes(HOT.0.to_vec()),
            ],
        )?;
        state.seed_encoded(key, None);
        state.seed(&st::NETWORK_POW_REGISTRATION_ALLOWED, (SUBNET,), true)?;
        state.seed(&st::MAX_ALLOWED_UIDS, (SUBNET,), 16u16)?;
        state.seed(&st::DIFFICULTY, (SUBNET,), 1u64)?;
        state.seed(&st::MIN_DIFFICULTY, (SUBNET,), 1u64)?;
        state.seed(&st::MAX_DIFFICULTY, (SUBNET,), u64::MAX)?;
        state.seed(&st::MAX_REGISTRATIONS_PER_BLOCK, (SUBNET,), 1u16)?;
        state.seed(&st::REGISTRATIONS_THIS_BLOCK, (SUBNET,), 0u16)?;
    }
    Ok(state)
}

fn extra_rows<'a>(sample: &'a Value, section: &str) -> Result<&'a [Value]> {
    match sample.get(section) {
        None => Ok(&[]),
        Some(Value::Array(rows)) => Ok(rows),
        Some(_) => bail!("extra sample {section} must be an array"),
    }
}

fn apply_extra(state: &mut FakeState, sample: &Value) -> Result<()> {
    for name in sample.as_object().context("extra sample object")?.keys() {
        ensure!(
            matches!(
                name.as_str(),
                "id" | "args" | "review" | "storage" | "constants" | "runtime"
            ),
            "extra sample section {name} needs an explicit seed adapter"
        );
    }
    for row in extra_rows(sample, "storage")? {
        let pallet = row["container"].as_str().context("extra storage pallet")?;
        let item = row["name"].as_str().context("extra storage name")?;
        let mut params = row["params"]
            .as_array()
            .context("extra storage params")?
            .clone();
        let entry = state
            .runtime
            .storage_entry(pallet, item)
            .context("extra storage metadata")?;
        ensure!(
            entry.key_types.len() == params.len(),
            "extra storage key arity"
        );
        for (param, type_id) in params.iter_mut().zip(&entry.key_types) {
            if state.runtime.type_ident(*type_id) == "NetUid" {
                *param = scenario_netuid(param)?;
            } else if state.runtime.type_ident(*type_id) == "AccountId32" {
                // Reviewed upstream identities follow the same public scenario
                // slots used by constructor arguments.
                for (name, account) in [("ALICE", COLD), ("ALICE_HOT", HOT), ("BOB_HOT", HOT)] {
                    let original =
                        rtao_catalog::py::table(SAMPLE_PATH, name).context("sample account")?;
                    if *param == rtao_primitives::json::from_str::<Value>(original.value_json)? {
                        *param = json!(account.to_string());
                        break;
                    }
                }
            }
        }
        state.seed_python(pallet, item, &params, row["value"].clone())?;
    }
    for row in extra_rows(sample, "constants")? {
        state.seed_constant(
            row["container"].as_str().context("extra constant pallet")?,
            row["name"].as_str().context("extra constant name")?,
            row["value"].clone(),
        )?;
    }
    for row in extra_rows(sample, "runtime")? {
        state.seed_runtime(
            row["container"].as_str().context("extra runtime API")?,
            row["name"].as_str().context("extra runtime method")?,
            row["value"].clone(),
        )?;
    }
    Ok(())
}

fn outer_call(runtime: &RuntimeHandle, bytes: &[u8]) -> Result<(String, String)> {
    let [pallet_index, call_index, ..] = bytes else {
        bail!("composed call lacks its two metadata indices");
    };
    let metadata = MetadataIr::from_runtime(runtime.core())?;
    let pallet = metadata
        .pallets
        .iter()
        .find(|pallet| pallet.index == *pallet_index)
        .context("composed pallet index exists in live metadata")?;
    let call = pallet
        .calls
        .iter()
        .find(|call| call.index == *call_index)
        .context("composed call index exists in live metadata")?;
    Ok((pallet.name.clone(), call.name.clone()))
}

async fn plan_case(
    runtime: Arc<Runtime>,
    spec: &IntentSpec,
    mut args: Value,
    extra: Option<&Value>,
    expected_role: Option<SignerRole>,
) -> Result<()> {
    let mut state = seeded_state(runtime, spec.name)?;
    if let Some(extra) = extra {
        apply_extra(&mut state, extra)?;
    }
    if spec.name == "pow_register" {
        // The upstream sample seal is only a placeholder. Bind one fixed nonce
        // to this public fake challenge at difficulty one; no solver is run.
        let hash = state.block_hash(state.block).context("fake work block")?;
        let hash = H256(decode_fixed_hex(&hash)?);
        let prefix = rtao_intents::pow::registration_prefix(SUBNET, hash, HOT, COLD)?;
        args["work_block"] = json!(state.block);
        args["nonce"] = json!(0);
        args["work_hex"] = json!(hex::encode(
            rtao_core::pow::seal(&prefix, 0).map_err(anyhow::Error::msg)?
        ));
    }
    let fake = FakeChain::from_state(state);
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    let client = Client::from_rpc(RpcClient::new(fake.clone()), config).await?;
    let keys = PublicKeys::default();
    let result = async {
        let intent = AnyIntent::from_tool(spec.name, args)?;
        let expected_role = expected_role.unwrap_or(spec.signer.parse()?);
        let plan = Executor::new(client.clone())
            .with_clock(Arc::new(FixtureClock))
            .plan_dyn(intent.as_dyn(), &keys, &PlanOptions::default())
            .await?;
        ensure!(plan.op == spec.name, "plan returned operation {}", plan.op);
        ensure!(!plan.summary.is_empty(), "plan has an empty summary");
        if extra.is_none() {
            // Match the ordinary Python table's exact FakeSubstrate fee.
            // Supplemental seeds have their own reviewed runtime overrides.
            ensure!(
                plan.fee.map(|fee| fee.rao()) == Some(124_414),
                "ordinary catalog plan fee differs from 124414 rao: {:?}",
                plan.fee
            );
        }
        ensure!(
            plan.ok(),
            "plan has preflight blocks: {:?}",
            plan.violations
        );
        ensure!(!plan.call().as_bytes().is_empty(), "empty composed call");
        ensure!(
            plan.signer == expected_role,
            "plan selected the wrong signing role"
        );
        ensure!(
            plan.signer_address
                == match expected_role {
                    SignerRole::Coldkey => COLD,
                    SignerRole::Hotkey => HOT,
                },
            "plan selected the wrong public signing account"
        );
        let runtime = client.refresh_runtime().await?;
        let (pallet, call) = outer_call(&runtime, plan.call().as_bytes())?;
        ensure!(
            spec.wraps.contains(&(pallet.as_str(), call.as_str())),
            "composed {pallet}.{call} is outside catalog wraps {:?}",
            spec.wraps
        );
        if spec.origin == "root" {
            ensure!(
                pallet == "Sudo" && call == "sudo",
                "root call lacks Sudo.sudo"
            );
            // Sudo.sudo's verified descriptor carries one inline RuntimeCall.
            // Check that semantic call too: a shared sudo wrapper alone cannot
            // establish that the correct root operation was planned.
            let (inner_pallet, inner_call) = outer_call(&runtime, &plan.call().as_bytes()[2..])?;
            ensure!(
                inner_pallet != "Sudo"
                    && spec
                        .wraps
                        .contains(&(inner_pallet.as_str(), inner_call.as_str())),
                "root semantic call {inner_pallet}.{inner_call} is outside its catalog wraps"
            );
        }
        if matches!(spec.name, "claim_root" | "claim_root_with_hotkey") {
            ensure!(
                plan.effects
                    .iter()
                    .any(|effect| effect.starts_with("reserved ")),
                "root claim fixture did not verify its fee reserve"
            );
            ensure!(
                plan.effects
                    .iter()
                    .any(|effect| effect.starts_with("accrued ")),
                "root claim fixture lost its seeded modern payout preview"
            );
        }
        ensure!(
            keys.acquisitions.load(Ordering::SeqCst) == 0,
            "planning acquired a private signer"
        );
        ensure!(
            fake.requests()
                .iter()
                .all(|request| !request.method.starts_with("author_")),
            "planning used an author RPC"
        );
        ensure!(
            fake.mutate_state(|state| state.submissions.is_empty()),
            "planning submitted an extrinsic"
        );
        Ok(())
    }
    .await;
    client.close().await;
    result
}

#[test]
// pyt: unit/test_intents_table.py::test_json_schema_is_wellformed
// pyt: unit/test_intents_table.py::test_serialization_roundtrip
// pyt: unit/test_intents_table.py::test_rejects_unknown_argument
fn authenticated_samples_preserve_schemas_roundtrip_and_unknown_field_refusal() -> Result<()> {
    let samples = samples()?;
    let expected: BTreeSet<_> = INTENTS.iter().map(|spec| spec.name).collect();
    ensure!(
        expected.len() == 83,
        "review changes to the pinned 83-op catalog"
    );
    ensure!(
        samples.keys().map(String::as_str).collect::<BTreeSet<_>>() == expected,
        "INTENT_SAMPLES and the complete catalog must have identical operations"
    );
    for spec in &INTENTS {
        let original = &samples[spec.name];
        let sample = original.as_object().context("sample arguments object")?;
        let catalog: Value = serde_json::from_str(spec.catalog_json)?;
        let schema = &catalog["input_schema"];
        ensure!(schema["type"] == "object", "{} schema type", spec.name);
        ensure!(
            schema["additionalProperties"] == false,
            "{} allows unknown schema arguments",
            spec.name
        );
        let properties = schema["properties"]
            .as_object()
            .context("schema properties")?;
        for field in schema["required"]
            .as_array()
            .context("schema required array")?
        {
            let field = field.as_str().context("required field name")?;
            ensure!(
                properties.contains_key(field) && sample.contains_key(field),
                "{}.{} must exist in both schema properties and the original sample",
                spec.name,
                field
            );
        }
        // These are the unchanged authenticated sample arguments: no subnet,
        // hotkey or work-proof rewrite used by the planning fixture is needed.
        let intent = AnyIntent::from_tool(spec.name, original.clone())
            .with_context(|| format!("{} original sample constructor", spec.name))?;
        let encoded = intent.to_json();
        ensure!(encoded["op"] == spec.name, "{} serialized op", spec.name);
        let mut arguments = encoded.clone();
        ensure!(
            arguments
                .as_object_mut()
                .context("encoded intent object")?
                .remove("op")
                .is_some(),
            "{} serialized operation missing",
            spec.name
        );
        let rebuilt = AnyIntent::from_tool(spec.name, arguments)?;
        ensure!(
            rebuilt.to_json() == encoded,
            "{} original sample roundtrip changed normalized arguments",
            spec.name
        );
        let mut unknown = original.clone();
        unknown
            .as_object_mut()
            .context("sample arguments object")?
            .insert("definitely_not_a_field".into(), json!(1));
        ensure!(
            AnyIntent::from_tool(spec.name, unknown).is_err(),
            "{} accepted an unknown argument in its original sample",
            spec.name
        );
    }
    Ok(())
}

#[tokio::test]
// pyt: unit/test_intents_table.py::test_every_intent_has_a_sample
// pyt: unit/test_intents_table.py::test_composes_and_plans_offline
async fn all_83_catalog_operations_plan_with_fake_chain() -> Result<()> {
    let samples = samples()?;
    let expected: BTreeSet<_> = INTENTS.iter().map(|spec| spec.name).collect();
    ensure!(
        expected.len() == 83,
        "review changes to the pinned 83-op catalog"
    );
    ensure!(
        samples.keys().map(String::as_str).collect::<BTreeSet<_>>() == expected,
        "INTENT_SAMPLES and the complete catalog must have identical operations"
    );
    let runtime = FakeState::try_default()?.runtime;
    let mut failures = Vec::new();
    for spec in &INTENTS {
        let result = async {
            let args = scenario_args(spec.name, &samples[spec.name])?;
            plan_case(runtime.clone(), spec, args, None, None).await
        }
        .await;
        if let Err(error) = result {
            failures.push(format!("{}: {error:#}", spec.name));
        }
    }
    ensure!(
        failures.is_empty(),
        "all-83 planning failures:\n{}",
        failures.join("\n")
    );
    Ok(())
}

#[tokio::test]
async fn reviewed_supplementary_inputs_plan_with_fake_chain() -> Result<()> {
    let document: Value = rtao_primitives::json::from_slice(include_bytes!(
        "../../../vendor/oracle/samples_extra.json"
    ))?;
    ensure!(
        document["schema"] == 1,
        "review supplementary sample schema"
    );
    let inputs = document["intents"]
        .as_object()
        .context("extra intent samples")?;
    ensure!(
        !inputs.is_empty(),
        "supplementary intent samples must not disappear"
    );
    let runtime = FakeState::try_default()?.runtime;
    let mut failures = Vec::new();
    for (op, samples) in inputs {
        let spec = INTENTS
            .iter()
            .find(|spec| spec.name == op.as_str())
            .context("extra sample catalog op")?;
        let samples = samples.as_array().context("extra samples")?;
        ensure!(
            !samples.is_empty(),
            "empty supplementary sample list for {op}"
        );
        for sample in samples {
            let result = async {
                let args = scenario_args(op, &sample["args"])?;
                plan_case(runtime.clone(), spec, args, Some(sample), None).await
            }
            .await;
            if let Err(error) = result {
                failures.push(format!("{op}/{}: {error:#}", sample["id"]));
            }
        }
    }
    ensure!(
        failures.is_empty(),
        "supplementary planning failures:\n{}",
        failures.join("\n")
    );
    Ok(())
}

#[tokio::test]
async fn root_denominations_and_hotkey_batch_plan_with_fake_chain() -> Result<()> {
    let samples = samples()?;
    let runtime = FakeState::try_default()?.runtime;
    for op in [
        "remove_stake",
        "remove_stake_limit",
        "move_stake",
        "swap_stake",
        "move_swap_stake",
        "transfer_stake",
    ] {
        let spec = INTENTS
            .iter()
            .find(|spec| spec.name == op)
            .context("root branch catalog op")?;
        let mut args = scenario_args(op, &samples[op])?;
        let field = if args.get("origin_netuid").is_some() {
            "origin_netuid"
        } else {
            "netuid"
        };
        args[field] = json!(0);
        plan_case(runtime.clone(), spec, args, None, None)
            .await
            .with_context(|| format!("root-denominated {op}"))?;
    }
    let spec = INTENTS
        .iter()
        .find(|spec| spec.name == "batch")
        .context("batch catalog op")?;
    plan_case(
        runtime,
        spec,
        json!({"intents":[
            {"op":"reset_axon", "netuid":SUBNET.0},
            {"op":"serve_prometheus", "netuid":SUBNET.0, "ip":"203.0.113.5", "port":9090}
        ]}),
        None,
        Some(SignerRole::Hotkey),
    )
    .await
    .context("hotkey-selected batch")?;
    Ok(())
}
