// Derived from RaoFoundation/subtensor sdk/python/tests/harness/samples.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/tests/unit/test_intents_table.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Ordinary M5 planning coverage from the authenticated upstream sample table.
//! Every operation uses the real executor and encoded FakeChain state.
#![cfg(all(feature = "all", feature = "pow"))]

use anyhow::{Context, Result, bail, ensure};
use rtao_catalog::{INTENTS, IntentSpec};
use rtao_client::{Client, ClientConfig, Network};
use rtao_core::{
    ir::MetadataIr,
    runtime::{Runtime, TypeSpec},
};
use rtao_intents::AnyIntent;
use rtao_primitives::{AccountId, Balance, DynBalance, H256, NetUid, Spend, Tao};
use rtao_runtime::RuntimeHandle;
use rtao_signer::{BoxFuture, DynKeySource, KeyView, Signer, SignerError, SignerRole};
use rtao_testkit::{
    FakeChain, FakeState,
    fixtures::{
        catalog_samples::{
            COLD, HOT, OTHER_HOT, ROOT_NATIVE_AMOUNT_ID, SUBNET, intent_args as scenario_args,
            intent_state as seeded_state, literal, root_native_amount_call,
            root_native_amount_inputs, scenario_netuid,
        },
        decode_fixed_hex,
    },
    rpc::RpcClient,
};
use rtao_tx::{Clock, Executor, Plan, PlanOptions, Policy};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, SystemTime},
};

const SAMPLE_PATH: &str = "sdk/python/tests/harness/samples.py";
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

impl rtao_signer::KeySource<rtao_signer::Coldkey> for PublicKeys {
    fn address(&self) -> Result<AccountId, SignerError> {
        self.coldkey_address()
    }

    async fn signer(&self) -> Result<Arc<dyn Signer>, SignerError> {
        self.signer_for(SignerRole::Coldkey).await
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

fn bind_sample_work(state: &FakeState, op: &str, args: &mut Value) -> Result<()> {
    if op == "pow_register" {
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
    Ok(())
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
    bind_sample_work(&state, spec.name, &mut args)?;
    let root_expected = extra
        .filter(|sample| sample["id"] == ROOT_NATIVE_AMOUNT_ID)
        .map(|_| root_native_amount_call(spec.name, &args))
        .transpose()?;
    let fake = FakeChain::from_state(state);
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    let client = Client::from_rpc(RpcClient::new(fake.clone()), config).await?;
    let keys = PublicKeys::default();
    let result = async {
        let intent = AnyIntent::from_tool(spec.name, args)?;
        if root_expected.is_some() {
            let amount = match &intent {
                AnyIntent::RemoveStake(intent) => &intent.amount_alpha,
                AnyIntent::RemoveStakeLimit(intent) => &intent.amount_alpha,
                AnyIntent::MoveStake(intent) => &intent.amount_alpha,
                AnyIntent::SwapStake(intent) => &intent.amount_alpha,
                AnyIntent::MoveSwapStake(intent) => &intent.amount_alpha,
                AnyIntent::TransferStake(intent) => &intent.amount_alpha,
                _ => bail!("root sample selected a different intent family"),
            };
            ensure!(
                matches!(amount.resolve(NetUid::ROOT, false)?, rtao_primitives::ResolvedAmount::Exact(value) if value.netuid == NetUid::ROOT && value.rao == 1_000_000_001),
                "root sample lost exact native TAO denomination"
            );
        }
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
        if let Some(expected) = &root_expected {
            ensure!(plan.call().as_bytes() == expected, "root input {0} changed independent call arguments", spec.name);
        }
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

// Use the same encoded state, public slots and fixed PoW proof as ordinary
// catalog planning. A policy refusal must not be an unrelated preflight block.
async fn policy_plan_case(
    runtime: Arc<Runtime>,
    spec: &IntentSpec,
    mut args: Value,
    policy: Policy,
) -> Result<Plan> {
    let state = seeded_state(runtime, spec.name)?;
    bind_sample_work(&state, spec.name, &mut args)?;
    let intent = AnyIntent::from_tool(spec.name, args)?;
    let fake = FakeChain::from_state(state);
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    let client = Client::from_rpc(RpcClient::new(fake.clone()), config).await?;
    for (account, nonce) in [(COLD, 17), (HOT, 23)] {
        client.nonces().pin(&account, nonce);
    }
    let keys = PublicKeys::default();
    let result = async {
        let plan = Executor::new(client.clone())
            .with_clock(Arc::new(FixtureClock))
            .plan_dyn(
                intent.as_dyn(),
                &keys,
                &PlanOptions {
                    policy: Some(policy),
                    ..Default::default()
                },
            )
            .await?;
        ensure!(plan.op == spec.name, "policy plan changed operation");
        ensure!(!plan.call().as_bytes().is_empty(), "empty policy plan call");
        ensure!(
            keys.acquisitions.load(Ordering::SeqCst) == 0,
            "policy planning acquired a private signer"
        );
        ensure!(
            fake.requests()
                .iter()
                .all(|request| !request.method.starts_with("author_")),
            "policy planning used an author RPC"
        );
        ensure!(
            fake.mutate_state(|state| state.submissions.is_empty()),
            "policy planning submitted an extrinsic"
        );
        for (account, nonce) in [(COLD, 17), (HOT, 23)] {
            ensure!(
                client.nonces().cached(&account.to_string()).await == Some(nonce),
                "policy planning changed a cached nonce"
            );
        }
        Ok(plan)
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
        expected.len() == 84,
        "review changes to the pinned 84-op catalog"
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
async fn all_84_catalog_operations_plan_with_fake_chain() -> Result<()> {
    let samples = samples()?;
    let expected: BTreeSet<_> = INTENTS.iter().map(|spec| spec.name).collect();
    ensure!(
        expected.len() == 84,
        "review changes to the pinned 84-op catalog"
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
        "all-84 planning failures:\n{}",
        failures.join("\n")
    );
    Ok(())
}

#[tokio::test]
// pyt: unit/test_intents_table.py::TestPolicyEnforcement::test_spend_cap_blocks_every_spending_intent
async fn all_84_sample_plans_enforce_declared_spend_caps_without_private_access() -> Result<()> {
    let samples = samples()?;
    let names: BTreeSet<_> = INTENTS.iter().map(|spec| spec.name).collect();
    ensure!(names.len() == 84, "review the pinned 84-op catalog");
    ensure!(
        samples.keys().map(String::as_str).collect::<BTreeSet<_>>() == names,
        "spend policy must cover every authenticated sample"
    );
    let runtime = FakeState::try_default()?.runtime;
    let policy = Policy {
        max_spend: Some(Balance::<Tao>::from_rao(1)),
        ..Default::default()
    };
    let mut bounded = BTreeSet::new();
    let mut unbounded = BTreeSet::new();
    let mut permitted = BTreeSet::new();
    for spec in &INTENTS {
        let args = scenario_args(spec.name, &samples[spec.name])?;
        let intent = AnyIntent::from_tool(spec.name, args.clone())?;
        let spend = intent.as_dyn().spend_dyn();
        let expected = match spend {
            Spend::Unbounded => {
                unbounded.insert(spec.name);
                vec![format!(
                    "{} spends an amount that cannot be bounded before execution; max_spend_tao τ0.000000001 blocks it",
                    spec.name
                )]
            }
            Spend::Bounded(amount) if amount.rao() > 1 => {
                bounded.insert(spec.name);
                vec![format!(
                    "spend {} exceeds max_spend_tao τ0.000000001",
                    DynBalance::from(amount)
                )]
            }
            // Native zero/one-rao bounds remain admissible. Do not turn a
            // bounded-zero declaration into an unbounded-spend assertion.
            Spend::None | Spend::Bounded(_) => {
                permitted.insert(spec.name);
                Vec::new()
            }
        };
        let plan = policy_plan_case(runtime.clone(), spec, args, policy.clone())
            .await
            .with_context(|| format!("{} spend-cap plan", spec.name))?;
        ensure!(plan.spend == spend, "{} changed declared spend", spec.name);
        ensure!(
            plan.violations == expected,
            "{} spend-cap violations: {:?}, expected {:?}",
            spec.name,
            plan.violations,
            expected
        );
        ensure!(
            plan.ok() == expected.is_empty(),
            "{} spend-cap permission",
            spec.name
        );
    }
    ensure!(
        bounded.contains("transfer") && bounded.contains("batch"),
        "positive sample spending declarations disappeared"
    );
    ensure!(
        unbounded.contains("transfer_all") && unbounded.contains("register_subnet"),
        "unbounded sample spending declarations disappeared"
    );
    ensure!(
        permitted.contains("serve_axon") && !permitted.is_empty(),
        "nonspending controls disappeared"
    );
    ensure!(
        bounded.len() + unbounded.len() + permitted.len() == 84,
        "spend-policy sample inventory is incomplete"
    );
    Ok(())
}

#[tokio::test]
// pyt: unit/test_intents_table.py::TestPolicyEnforcement::test_netuid_allowlist
async fn all_84_sample_plans_enforce_subnet_allowlists_without_private_access() -> Result<()> {
    let samples = samples()?;
    let names: BTreeSet<_> = INTENTS.iter().map(|spec| spec.name).collect();
    ensure!(names.len() == 84, "review the pinned 84-op catalog");
    ensure!(
        samples.keys().map(String::as_str).collect::<BTreeSet<_>>() == names,
        "subnet policy must cover every authenticated sample"
    );
    let runtime = FakeState::try_default()?.runtime;
    // Source subnet1 maps to the existing public fixture subnet7. Source
    // selectors exempt0/1; preserve that selection before rewriting inputs.
    let policy = Policy {
        allowed_netuids: Some(BTreeSet::from([SUBNET])),
        ..Default::default()
    };
    let mut outside = BTreeSet::new();
    let mut global = BTreeSet::new();
    let mut permitted = BTreeSet::new();
    let mut root_scoped = BTreeSet::new();
    let mut visited = BTreeSet::new();
    for spec in &INTENTS {
        let original = AnyIntent::from_tool(spec.name, samples[spec.name].clone())?;
        let original_scope = original.as_dyn().touches_netuids_dyn();
        let declared_global = original.as_dyn().affects_all_subnets_dyn();
        let args = scenario_args(spec.name, &samples[spec.name])?;
        let intent = AnyIntent::from_tool(spec.name, args.clone())?;
        let scope = intent.as_dyn().touches_netuids_dyn();
        let mapped = original_scope
            .iter()
            .map(|netuid| scenario_netuid(&json!(netuid.0)))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        ensure!(
            scope
                .iter()
                .map(|netuid| json!(netuid.0))
                .collect::<Vec<_>>()
                == mapped,
            "{} rewrote the semantic subnet scope",
            spec.name
        );
        ensure!(
            intent.as_dyn().affects_all_subnets_dyn() == declared_global,
            "{} rewrote the declared global scope",
            spec.name
        );
        // The native ID-only declaration is conservative. This fixture pins
        // a target-only crowdloan: its raised TAO is unbounded spend, but its
        // actual stored dispatch has no subnet effect after state inspection.
        let affects_all = if spec.name == "finalize_crowdloan" {
            ensure!(
                declared_global && scope.is_empty(),
                "review the target-only crowdloan scope adaptation"
            );
            false
        } else {
            declared_global
        };
        let source_outside = original_scope
            .iter()
            .any(|netuid| !matches!(netuid.0, 0 | 1));
        if source_outside {
            outside.insert(spec.name);
        }
        if affects_all {
            global.insert(spec.name);
        }
        let mut expected = Vec::new();
        if affects_all {
            expected.push(format!(
                "{} acts on all subnets, not just allowed_netuids [7]",
                spec.name
            ));
        }
        for netuid in &scope {
            if *netuid != SUBNET {
                expected.push(format!("netuid {} not in allowed_netuids [7]", netuid.0));
            }
            // Unlike the source test's selection predicate, an actual native
            // allowlist containing only7 also excludes root0. Check it too.
            if *netuid == NetUid::ROOT {
                root_scoped.insert(spec.name);
            }
        }
        let plan = policy_plan_case(runtime.clone(), spec, args, policy.clone())
            .await
            .with_context(|| format!("{} subnet-allowlist plan", spec.name))?;
        ensure!(
            plan.violations == expected,
            "{} subnet-allowlist violations: {:?}, expected {:?}",
            spec.name,
            plan.violations,
            expected
        );
        ensure!(
            plan.ok() == expected.is_empty(),
            "{} subnet-allowlist permission",
            spec.name
        );
        if source_outside || affects_all {
            ensure!(
                !plan.ok(),
                "{} escaped the source scope selector",
                spec.name
            );
        }
        if plan.ok() {
            permitted.insert(spec.name);
        }
        ensure!(visited.insert(spec.name), "duplicate policy sample");
    }
    ensure!(
        visited == names,
        "subnet-policy sample inventory is incomplete"
    );
    ensure!(
        outside.contains("move_swap_stake") && outside.contains("swap_basket_many"),
        "cross-subnet controls disappeared"
    );
    ensure!(
        global.contains("unstake_all") && global.contains("unstake_all_alpha"),
        "global-subnet controls disappeared"
    );
    ensure!(
        root_scoped.contains("swap_basket") && permitted.contains("transfer"),
        "root refusal or no-subnet control disappeared"
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
    // A shrinking extras table must not erase a required root branch.
    root_native_amount_inputs(&document)?;
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
    let document: Value = rtao_primitives::json::from_slice(include_bytes!(
        "../../../vendor/oracle/samples_extra.json"
    ))?;
    for (op, sample) in root_native_amount_inputs(&document)? {
        let spec = INTENTS
            .iter()
            .find(|spec| spec.name == op)
            .context("root extra catalog op")?;
        plan_case(
            runtime.clone(),
            spec,
            scenario_args(op, &sample["args"])?,
            Some(sample),
            None,
        )
        .await
        .with_context(|| format!("root supplementary {op}"))?;
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

#[tokio::test]
// pyt: unit/test_intents_table.py::TestExecuteFlow::test_proxy_all_reads_proxied_account_balance
async fn proxied_all_stake_plan_reads_treasury_balance_without_private_access() -> Result<()> {
    const TREASURY: AccountId = AccountId([4; 32]);
    let runtime = FakeState::try_default()?.runtime;
    let mut state = seeded_state(runtime, "add_stake")?;
    state.seed_constant("Balances", "ExistentialDeposit", json!(500))?;
    for (account, free) in [(TREASURY, 10_000_000_000u64), (COLD, 0)] {
        state.seed_python(
            "System",
            "Account",
            &[json!(account.to_string())],
            json!({"data":{"free":free}}),
        )?;
    }
    let fake = FakeChain::from_state(state);
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    let client = Client::from_rpc(RpcClient::new(fake.clone()), config).await?;
    let keys = PublicKeys::default();
    let result = async {
        // Preserve AddStake's default price protection and the existing
        // sample fixture's source subnet1 -> native subnet7 adaptation.
        let intent: rtao_intents::AddStake = serde_json::from_value(json!({
            "hotkey_ss58": OTHER_HOT.to_string(),
            "netuid": SUBNET.0,
            "amount_tao": "all"
        }))?;
        let plan = Executor::new(client.clone())
            .with_clock(Arc::new(FixtureClock))
            .plan(
                &intent,
                &keys,
                &PlanOptions {
                    proxy_for: Some(TREASURY),
                    ..Default::default()
                },
            )
            .await?;
        ensure!(
            plan.ok(),
            "proxied all-stake violations: {:?}",
            plan.violations
        );
        ensure!(plan.signer == SignerRole::Coldkey, "wrong delegate role");
        ensure!(plan.signer_address == COLD, "wrong delegate account");
        ensure!(
            plan.extras["proxy_for"] == json!(TREASURY),
            "proxy metadata"
        );
        let runtime = client.refresh_runtime().await?;
        let outer = rtao_core::DynValue::new(
            runtime
                .core()
                .decode_spec(&TypeSpec::Call, plan.call().as_bytes(), true)
                .map_err(rtao_core::CoreError::from)?,
        )
        .to_json();
        ensure!(
            outer["call_module"] == "Proxy" && outer["call_function"] == "proxy",
            "missing real proxy wrapper"
        );
        let args = outer["call_args"].as_array().context("proxy arguments")?;
        let real = args
            .iter()
            .find(|arg| arg["name"] == "real")
            .context("proxy real")?;
        ensure!(
            real["value"] == json!(TREASURY),
            "proxy targets the wrong account"
        );
        let inner = &args
            .iter()
            .find(|arg| arg["name"] == "call")
            .context("proxy inner call")?["value"];
        ensure!(
            inner["call_module"] == "SubtensorModule"
                && inner["call_function"] == "add_stake_limit",
            "default protected AddStake call changed"
        );
        let amount = inner["call_args"]
            .as_array()
            .context("stake arguments")?
            .iter()
            .find(|arg| arg["name"] == "amount_staked")
            .context("stake amount")?;
        // Independent upstream vector: treasury free minus ED500 and
        // 500000 rao fee headroom, never the zero-free delegate balance.
        ensure!(
            amount["value"] == json!(9_999_499_500u64),
            "wrong treasury drain amount"
        );
        ensure!(
            keys.acquisitions.load(Ordering::SeqCst) == 0,
            "private signer acquisition"
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
