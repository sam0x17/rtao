// Derived from RaoFoundation/subtensor sdk/python/bittensor/intents/staking.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/tests/harness/samples.py @ 6a972693ae9580541225bd37696ac2fee26b8ae9 (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/tests/unit/test_intents_table.py @ 6a972693ae9580541225bd37696ac2fee26b8ae9 (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/tests/unit/test_reads_table.py @ 6a972693ae9580541225bd37696ac2fee26b8ae9 (Apache-2.0); modified by rtao.
//! Pure literal decoding and native FakeChain seeds shared by table consumers.
//!
//! Callers authenticate the catalog tables and run their own real SDK or CLI
//! paths. This module neither imports those consumers nor creates signers.

use std::sync::Arc;

use crate::FakeState;
use rtao_core::{
    CoreError,
    codec::Value as CoreValue,
    runtime::{Runtime, TypeSpec},
};
use rtao_primitives::{AccountId, DefaultAccount, NetUid, TaoBalance};
use rtao_runtime::{models::CrowdloanInfo, storage::subtensor_module as st};
use serde_json::{Map, Value, json};

/// Public coldkey slot used by the native intent scenarios.
pub const COLD: AccountId = AccountId([1; 32]);
/// Public hotkey slot used by the native intent scenarios.
pub const HOT: AccountId = AccountId([2; 32]);
/// Distinct destination hotkey required by move-swap-stake.
pub const OTHER_HOT: AccountId = AccountId([3; 32]);
/// Native source subnet corresponding to sample subnet one.
pub const SUBNET: NetUid = NetUid(7);
/// Native destination subnet corresponding to sample subnet two.
pub const DEST_SUBNET: NetUid = NetUid(8);
const FREE_RAO: u64 = 1_000_000_000_000_000_000;

fn invalid(message: impl Into<String>) -> CoreError {
    CoreError::Codec(message.into())
}

/// Decode lossless Python dict/tuple carriers without dropping duplicate keys.
pub fn literal(value: &Value) -> Result<Value, CoreError> {
    match value {
        Value::Array(values) => values
            .iter()
            .map(literal)
            .collect::<Result<_, _>>()
            .map(Value::Array),
        Value::Object(tagged) if tagged.len() == 1 && tagged.contains_key("dict") => {
            let pairs = tagged["dict"]
                .as_array()
                .ok_or_else(|| invalid("Python dict pairs"))?;
            let mut fields = Map::new();
            for pair in pairs {
                let pair = pair
                    .as_array()
                    .filter(|pair| pair.len() == 2)
                    .ok_or_else(|| invalid("Python dict pair must have two entries"))?;
                let key = pair[0]
                    .as_str()
                    .ok_or_else(|| invalid("Python sample string key"))?;
                if fields.insert(key.into(), literal(&pair[1])?).is_some() {
                    return Err(invalid(format!("duplicate Python sample key {key}")));
                }
            }
            Ok(Value::Object(fields))
        }
        Value::Object(tagged) if tagged.len() == 1 && tagged.contains_key("tuple") => {
            literal(&tagged["tuple"])
        }
        Value::Object(_) => Err(invalid("unknown extracted sample literal carrier")),
        value => Ok(value.clone()),
    }
}

/// Map the pinned sample's three subnet slots into the native fixture state.
pub fn scenario_netuid(value: &Value) -> Result<Value, CoreError> {
    match value.as_u64().ok_or_else(|| invalid("sample netuid"))? {
        0 => Ok(json!(0)),
        1 => Ok(json!(SUBNET.0)),
        2 => Ok(json!(DEST_SUBNET.0)),
        other => Err(invalid(format!("unreviewed sample subnet {other}"))),
    }
}

fn rewrite(args: &mut Value) -> Result<(), CoreError> {
    match args {
        Value::Object(fields) => {
            for (name, value) in fields {
                if name == "netuid" || name.ends_with("_netuid") {
                    *value = scenario_netuid(value)?;
                } else if name == "netuids" || name == "subnets" {
                    for netuid in value
                        .as_array_mut()
                        .ok_or_else(|| invalid("sample subnet list"))?
                    {
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

/// Adapt authenticated intent arguments to the same public native scenario.
pub fn intent_args(op: &str, original: &Value) -> Result<Value, CoreError> {
    let mut args = original.clone();
    rewrite(&mut args)?;
    // This operation requires two distinct hotkeys, including after adaptation.
    if op == "move_swap_stake" {
        args["dest_hotkey_ss58"] = json!(OTHER_HOT.to_string());
    }
    Ok(args)
}

/// Required root-origin money cases, separate from canonical upstream samples.
pub const ROOT_NATIVE_AMOUNT_OPS: [&str; 6] = [
    "remove_stake",
    "remove_stake_limit",
    "move_stake",
    "swap_stake",
    "move_swap_stake",
    "transfer_stake",
];
/// Identity of each reviewed root-origin supplementary input.
pub const ROOT_NATIVE_AMOUNT_ID: &str = "root-native-decimal";

/// Require all six exact original-identity root inputs, once each.
///
/// These are authored inputs, not recorded Python outputs. Original sample
/// addresses come from the pinned harness; native scenario rewriting happens
/// only after this check. One decimal rao above a whole token catches rounding.
pub fn root_native_amount_inputs(
    document: &Value,
) -> Result<Vec<(&'static str, &Value)>, CoreError> {
    let entries = document["intents"]
        .as_object()
        .ok_or_else(|| invalid("extra intent inputs"))?;
    let hot = "5CS3j9wEaMEt5tpEkF17ZVY8DCkLjrmkj3LxGnKzodS4ccoM";
    let destination = "5G6Fyo6U2fDhJXDJGp4Dr6XViVCdPcryiyLBDsusguSRcYvJ";
    let receiver = "5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty";
    let expected = [
        json!({"hotkey_ss58":hot,"netuid":0,"amount_alpha":"1.000000001","slippage_protection":false,"claim":false}),
        json!({"hotkey_ss58":hot,"netuid":0,"amount_alpha":"1.000000001","limit_price_rao":1_000_000_000u64,"allow_partial":false,"claim":false}),
        json!({"origin_hotkey_ss58":hot,"origin_netuid":0,"dest_hotkey_ss58":destination,"dest_netuid":2,"amount_alpha":"1.000000001","slippage_protection":false,"claim":false}),
        json!({"hotkey_ss58":hot,"origin_netuid":0,"dest_netuid":2,"amount_alpha":"1.000000001","slippage_protection":false}),
        json!({"origin_hotkey_ss58":hot,"origin_netuid":0,"dest_hotkey_ss58":destination,"dest_netuid":2,"amount_alpha":"1.000000001","slippage_protection":false}),
        json!({"dest_coldkey_ss58":receiver,"hotkey_ss58":hot,"origin_netuid":0,"dest_netuid":2,"amount_alpha":"1.000000001"}),
    ];
    let mut selected = Vec::with_capacity(6);
    for (op, expected) in ROOT_NATIVE_AMOUNT_OPS.into_iter().zip(expected) {
        let rows = entries
            .get(op)
            .and_then(Value::as_array)
            .ok_or_else(|| invalid(format!("missing root input {op}")))?;
        let rows = rows
            .iter()
            .filter(|row| row["id"] == ROOT_NATIVE_AMOUNT_ID)
            .collect::<Vec<_>>();
        if rows.len() != 1 || rows[0]["args"] != expected {
            return Err(invalid(format!(
                "root input {op} must retain its exact amount and original origin/destination identities"
            )));
        }
        selected.push((op, rows[0]));
    }
    let count = entries
        .values()
        .filter_map(Value::as_array)
        .flatten()
        .filter(|row| row["id"] == ROOT_NATIVE_AMOUNT_ID)
        .count();
    if count != selected.len() {
        return Err(invalid(
            "root sample identity appeared outside its six reviewed operations",
        ));
    }
    Ok(selected)
}

/// Independent V478 SCALE bytes for a root input after optional scenario rewriting.
///
/// No call builder, amount parser or runtime encoder supplies these bytes.
/// Addresses/subnets are supplied by the inspected input; the fixed amount and
/// layouts independently pin exact money, call argument order and batch dust.
pub fn root_native_amount_call(op: &str, args: &Value) -> Result<Vec<u8>, CoreError> {
    const RAO: u64 = 1_000_000_001;
    let field = if matches!(op, "remove_stake" | "remove_stake_limit") {
        "netuid"
    } else {
        "origin_netuid"
    };
    if args[field] != 0 || args["amount_alpha"] != "1.000000001" {
        return Err(invalid(
            "root byte witness requires root origin and exact decimal amount",
        ));
    }
    let account = |field: &str| -> Result<AccountId, CoreError> {
        args[field]
            .as_str()
            .ok_or_else(|| invalid(format!("root input {field}")))?
            .parse()
            .map_err(|error| invalid(format!("root account: {error}")))
    };
    let hot = account(if matches!(op, "move_stake" | "move_swap_stake") {
        "origin_hotkey_ss58"
    } else {
        "hotkey_ss58"
    })?;
    let destination = || -> Result<u16, CoreError> {
        args["dest_netuid"]
            .as_u64()
            .and_then(|value| u16::try_from(value).ok())
            .filter(|value| *value != 0)
            .ok_or_else(|| invalid("root destination subnet must remain positive"))
    };
    let append_move = |bytes: &mut Vec<u8>, destination_hot: AccountId, netuid: u16| {
        bytes.extend_from_slice(&[7, 85]);
        bytes.extend_from_slice(hot.as_bytes());
        bytes.extend_from_slice(destination_hot.as_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes());
        bytes.extend_from_slice(&netuid.to_le_bytes());
        bytes.extend_from_slice(&RAO.to_le_bytes());
    };
    let append_swap = |bytes: &mut Vec<u8>, hot: AccountId, rao: u64, netuid: u16| {
        bytes.extend_from_slice(&[7, 87]);
        bytes.extend_from_slice(hot.as_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes());
        bytes.extend_from_slice(&netuid.to_le_bytes());
        bytes.extend_from_slice(&rao.to_le_bytes());
    };
    let mut bytes = Vec::new();
    match op {
        "remove_stake" | "remove_stake_limit" => {
            bytes.extend_from_slice(&[7, if op == "remove_stake" { 3 } else { 89 }]);
            bytes.extend_from_slice(hot.as_bytes());
            bytes.extend_from_slice(&0u16.to_le_bytes());
            bytes.extend_from_slice(&RAO.to_le_bytes());
            if op == "remove_stake_limit" {
                bytes.extend_from_slice(&1_000_000_000u64.to_le_bytes());
                bytes.push(0);
            }
        }
        "move_stake" => append_move(&mut bytes, account("dest_hotkey_ss58")?, destination()?),
        "swap_stake" => append_swap(&mut bytes, hot, RAO, destination()?),
        "move_swap_stake" => {
            let destination_hot = account("dest_hotkey_ss58")?;
            if hot == destination_hot {
                return Err(invalid("move-swap needs distinct public hotkeys"));
            }
            bytes.extend_from_slice(&[11, 2, 8]); // batch_all + compact two-call length.
            append_move(&mut bytes, destination_hot, 0);
            append_swap(&mut bytes, destination_hot, RAO - 1, destination()?);
        }
        "transfer_stake" => {
            bytes.extend_from_slice(&[7, 86]);
            bytes.extend_from_slice(account("dest_coldkey_ss58")?.as_bytes());
            bytes.extend_from_slice(hot.as_bytes());
            bytes.extend_from_slice(&0u16.to_le_bytes());
            bytes.extend_from_slice(&destination()?.to_le_bytes());
            bytes.extend_from_slice(&RAO.to_le_bytes());
        }
        _ => return Err(invalid("unreviewed root amount operation")),
    }
    Ok(bytes)
}

fn preview(hotkey: AccountId) -> Value {
    json!({"hotkey":hotkey.to_string(), "owed_shares":1,
        "accrued_tao":10_000_000, "redeemable_tao":10_000_000,
        "forfeited_tao_est":0, "rows":1, "rows_to_sell":1,
        "dust_rows":0, "swept":0, "flushed_credits":0})
}

fn seed_availability(state: &mut FakeState) -> Result<(), CoreError> {
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

/// Mainnet `SubtensorInitialTaoWeight` (about 5.27%) used by the emission fixtures.
pub const EMISSION_TAO_WEIGHT: u64 = 971_718_665_099_567_868;
/// Root TAO in rao that the emission fixtures are seeded with.
pub const EMISSION_ROOT_TAO: u64 = 5_443_000 * 1_000_000_000;
/// The sample subnet's alpha issuance in rao for the emission fixtures.
pub const EMISSION_ALPHA_ISSUANCE: u64 = 6_457_000 * 1_000_000_000;

// py: sdk/python/tests/harness/samples.py:23-51
/// Root's baseline share in parts, `V0 * x / (x + A)` with `x = T * w / ONE`,
/// derived exactly as the chain's `get_subnet_emission_weights` reports it.
pub fn emission_root_baseline_share() -> u64 {
    use rtao_primitives::emission_weights::{DEFAULT_VALIDATOR, ONE};
    let root_x = u128::from(EMISSION_ROOT_TAO) * u128::from(EMISSION_TAO_WEIGHT) / u128::from(ONE);
    let share =
        u128::from(DEFAULT_VALIDATOR) * root_x / (root_x + u128::from(EMISSION_ALPHA_ISSUANCE));
    u64::try_from(share).unwrap_or(ONE)
}

/// The chain's default emission weight vector record for `netuid`
/// (18% owner, 41% validator, 41% miner), with root's baseline share.
pub fn emission_weights_info(netuid: NetUid) -> Value {
    use rtao_primitives::emission_weights::{DEFAULT_OWNER, DEFAULT_VALIDATOR, ONE};
    let miner = ONE - DEFAULT_OWNER - DEFAULT_VALIDATOR;
    let baseline = emission_root_baseline_share();
    json!({
        "netuid": netuid.0,
        "weights": [0, 0, DEFAULT_OWNER, DEFAULT_VALIDATOR, miner],
        "burn": 0,
        "recycle": 0,
        "owner": DEFAULT_OWNER,
        "validator": DEFAULT_VALIDATOR,
        "miner": miner,
        "root_baseline_share": baseline,
        "root_share": baseline.min(DEFAULT_VALIDATOR),
        "created_via_lease": false,
    })
}

/// Seed valid native SCALE state for the complete authenticated intent table.
///
/// This holds only public fixture identities; proof binding stays in callers.
pub fn intent_state(runtime: Arc<Runtime>, op: &str) -> Result<FakeState, CoreError> {
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
    // py: sdk/python/tests/unit/test_intents_table.py:35-40
    state.seed_runtime(
        "SubnetInfoRuntimeApi",
        "get_subnet_emission_weights",
        emission_weights_info(SUBNET),
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
            .ok_or_else(|| invalid("Uids storage"))?;
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

/// Seed valid empty native answers for the authenticated read sample table.
///
/// The caller authenticates the table and supplies its availability coldkey.
pub fn read_state(availability_coldkey: &str) -> Result<FakeState, CoreError> {
    let mut state = FakeState::try_default()?;
    let empty_swap = json!({
        "tao_amount":0,"alpha_amount":0,"tao_fee":0,"alpha_fee":0,
        "tao_slippage":0,"alpha_slippage":0
    });
    // Exact active seeded_substrate answers from the Python module.
    for (api, method, value) in [
        ("NeuronInfoRuntimeApi", "get_neurons_lite", json!([])),
        ("NeuronInfoRuntimeApi", "get_neurons", json!([])),
        ("SubnetInfoRuntimeApi", "get_all_dynamic_info", json!([])),
        ("SubnetInfoRuntimeApi", "get_dynamic_info", Value::Null),
        (
            "SubnetInfoRuntimeApi",
            "get_subnet_hyperparams_v3",
            Value::Null,
        ),
        (
            "SubnetInfoRuntimeApi",
            "get_subnet_emission_weights",
            Value::Null,
        ),
        ("SubnetInfoRuntimeApi", "get_subnet_state", Value::Null),
        (
            "StakeInfoRuntimeApi",
            "get_stake_info_for_coldkey",
            json!([]),
        ),
        (
            "StakeInfoRuntimeApi",
            "get_stake_info_for_coldkeys",
            json!([]),
        ),
        (
            "StakeInfoRuntimeApi",
            "get_stake_info_for_hotkey_coldkey_netuid",
            Value::Null,
        ),
        ("DelegateInfoRuntimeApi", "get_delegates", json!([])),
        ("DelegateInfoRuntimeApi", "get_delegate", Value::Null),
        ("DelegateInfoRuntimeApi", "get_delegated", json!([])),
        (
            "SwapRuntimeApi",
            "sim_swap_tao_for_alpha",
            empty_swap.clone(),
        ),
        ("SwapRuntimeApi", "sim_swap_alpha_for_tao", empty_swap),
        (
            "SwapRuntimeApi",
            "current_alpha_price",
            json!(1_000_000_000),
        ),
        ("SwapRuntimeApi", "current_alpha_price_all", json!([])),
        (
            "SubnetRegistrationRuntimeApi",
            "get_network_registration_cost",
            json!(1_000_000_000),
        ),
    ] {
        state.seed_runtime(api, method, value)?;
    }
    // These seven Python fake answers name APIs absent from pinned metadata.
    // Their native consumers use storage or stake-info projections instead.
    // Retain their identities explicitly; never suppress arbitrary seed errors.
    for (api, method) in [
        ("StakeInfoRuntimeApi", "get_stake_value_for_coldkey"),
        ("StakeInfoRuntimeApi", "get_stake_value_for_coldkeys"),
        ("ColdkeySwapRuntimeApi", "get_scheduled_coldkey_swap"),
        ("CrowdloanRuntimeApi", "get_crowdloans"),
        ("CrowdloanRuntimeApi", "get_crowdloan"),
        ("LeasingRuntimeApi", "get_leases"),
        ("LeasingRuntimeApi", "get_lease"),
    ] {
        if rtao_core::ops::runtime_api::runtime_api_method(&state.runtime, api, method).is_ok() {
            return Err(invalid(format!(
                "review newly available sample API {api}.{method}"
            )));
        }
    }
    // Python's permissive fake returns empty values for these unseeded APIs.
    // Supply valid empty/zero native SCALE rather than bypassing the transport.
    for (api, method, value) in [
        ("SubnetInfoRuntimeApi", "get_metagraph", Value::Null),
        (
            "SubnetInfoRuntimeApi",
            "get_next_epoch_start_block",
            Value::Null,
        ),
        (
            "StakeInfoRuntimeApi",
            "get_most_convicted_hotkey_on_subnet",
            Value::Null,
        ),
        (
            "StakeInfoRuntimeApi",
            "get_hotkey_conviction",
            json!({"bits":0}),
        ),
        ("BetaBasketRuntimeApi", "get_basket_position", Value::Null),
        (
            "BetaBasketRuntimeApi",
            "get_root_basket_portfolio",
            json!([]),
        ),
        (
            "BetaBasketRuntimeApi",
            "get_root_basket_positions",
            json!([]),
        ),
        ("BetaBasketRuntimeApi", "get_root_basket_owed", json!(0)),
        ("BetaBasketRuntimeApi", "get_validator_basket", json!([])),
        ("BetaBasketRuntimeApi", "get_validator_basket_nav", json!(0)),
        (
            "BetaBasketRuntimeApi",
            "get_root_basket_total_nav",
            json!(0),
        ),
        (
            "BetaBasketRuntimeApi",
            "get_all_validator_baskets",
            json!([]),
        ),
        (
            "BetaBasketRuntimeApi",
            "get_all_beta_pricing",
            json!({"pricing":[],"next":null}),
        ),
        (
            "BetaBasketRuntimeApi",
            "get_validator_basket_summary",
            json!({}),
        ),
        ("BetaBasketRuntimeApi", "get_beta_pricing", Value::Null),
        (
            "BetaBasketRuntimeApi",
            "get_basket_trading_status",
            json!({}),
        ),
    ] {
        state.seed_runtime(api, method, value)?;
    }
    let api = "StakeInfoRuntimeApi";
    let method = "get_stake_availability_for_coldkeys";
    let signature = rtao_core::ops::runtime_api::runtime_api_method(&state.runtime, api, method)?;
    let output = state
        .runtime
        .encode_spec(&TypeSpec::Id(signature.output), &CoreValue::Dict(vec![]))?;
    let input = rtao_core::ops::runtime_api::encode_params(
        &state.runtime,
        api,
        signature,
        &[
            CoreValue::List(vec![CoreValue::Str(availability_coldkey.into())]),
            CoreValue::List(vec![CoreValue::Int(1)]),
        ],
    )?;
    state.seed_runtime_encoded(format!("{api}_{method}"), input, output);
    Ok(state)
}
