// Derived from RaoFoundation/subtensor sdk/python/bittensor/evm/precompiles.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! The pinned Bittensor precompile catalog and dynamic Solidity ABI helpers.
//!
//! ABI JSON is embedded directly from the reviewed upstream vendor directory.
//! Generic contract arguments retain Solidity's full integer widths; the
//! catalog's [`Precompile::encode_call`] additionally checks native rao amounts.

use std::{borrow::Cow, collections::BTreeMap};

use alloy_dyn_abi::{DynSolType, DynSolValue, FunctionExt, JsonAbiExt, Specifier};
use alloy_json_abi::{Function, JsonAbi, Param};
use alloy_primitives::{B256, U256};
use serde_json::Value;

use crate::{Error, Result, addresses};

/// Maximum calldata, result or artifact size accepted by these helpers.
pub const MAX_ABI_BYTES: usize = 16 * 1024 * 1024;
const MAX_ABI_ITEMS: usize = MAX_ABI_BYTES / 32;
const MAX_ABI_DEPTH: usize = 64;

/// One native runtime operation exposed at a fixed EVM address.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Precompile {
    /// CLI-facing kebab-case name.
    pub name: &'static str,
    /// Runtime `INDEX`, encoded as the low bytes of its 20-byte address.
    pub index: u16,
    /// Name of the pinned ABI JSON file.
    pub abi_file: &'static str,
    /// Description from the pinned Python catalog.
    pub description: &'static str,
    /// Whether the whole precompile is deprecated.
    pub deprecated: bool,
    abi_json: &'static str,
}

impl Precompile {
    /// The canonical lowercase, 0x-prefixed 20-byte address.
    pub fn address(&self) -> String {
        format!("0x{:040x}", self.index)
    }

    /// Exact pinned JSON ABI text, retaining its original array order.
    pub fn abi_json(&self) -> &'static str {
        self.abi_json
    }

    /// Parse the embedded JSON ABI without filesystem or network access.
    pub fn abi(&self) -> Result<JsonAbi> {
        serde_json::from_str(self.abi_json).map_err(Error::Json)
    }

    /// All function entries in the embedded ABI.
    pub fn functions(&self) -> Result<Vec<Function>> {
        Ok(self.abi()?.functions().cloned().collect())
    }

    /// Resolve a name, or an exact canonical signature for an overloaded name.
    ///
    /// A bare name selects the first overload, matching Python's catalog.
    pub fn function(&self, name: &str) -> Result<Function> {
        find_function(&self.abi()?, name, self.name).cloned()
    }

    /// Encode a native call, refusing rao-denominated inputs above `u64::MAX`.
    ///
    /// This check does not narrow unrelated ABI `uint256` values such as
    /// hashes, timestamps or general contract integers.
    pub fn encode_call(&self, name: &str, args: &[Value]) -> Result<String> {
        let function = self.function(name)?;
        let values = coerce_inputs(&function.inputs, args)?;
        check_native_amounts(&function.inputs, &values)?;
        encode_values(&function, &values)
    }
}

macro_rules! precompile {
    ($name:literal, $index:literal, $file:literal, $description:literal, $deprecated:literal) => {
        Precompile {
            name: $name,
            index: $index,
            abi_file: $file,
            description: $description,
            deprecated: $deprecated,
            abi_json: include_str!(concat!("../../../vendor/upstream/evm/abi/", $file)),
        }
    };
}

/// All 21 native precompiles, in the pinned Python catalog's order.
pub const PRECOMPILES: &[Precompile] = &[
    precompile!(
        "balance-transfer",
        2048,
        "balanceTransfer.json",
        "Transfer TAO from an EVM account to an ss58 account.",
        false
    ),
    precompile!(
        "staking",
        2049,
        "staking.json",
        "Staking V1 — deprecated; amounts ride msg.value. Use staking-v2.",
        true
    ),
    precompile!(
        "metagraph",
        2050,
        "metagraph.json",
        "Read-only neuron/subnet state: stake, ranks, trust, axons, keys by (netuid, uid).",
        false
    ),
    precompile!(
        "subnet",
        2051,
        "subnet.json",
        "Subnet registration and owner hyperparameter get/set pairs.",
        false
    ),
    precompile!(
        "neuron",
        2052,
        "neuron.json",
        "Neuron lifecycle: burned registration, weights (direct and commit-reveal), serving.",
        false
    ),
    precompile!(
        "staking-v2",
        2053,
        "stakingV2.json",
        "Staking V2: add/remove/move/transfer stake with explicit rao amounts.",
        false
    ),
    precompile!(
        "uid-lookup",
        2054,
        "uidLookup.json",
        "Look up the uid(s) associated with an EVM address on a subnet.",
        false
    ),
    precompile!(
        "alpha",
        2056,
        "alpha.json",
        "Subnet Alpha pools, prices, issuance, emissions, and owner configuration.",
        false
    ),
    precompile!(
        "crowdloan",
        2057,
        "crowdloan.json",
        "Crowdloan operations from EVM.",
        false
    ),
    precompile!(
        "leasing",
        2058,
        "leasing.json",
        "Subnet leasing operations from EVM.",
        false
    ),
    precompile!(
        "proxy",
        2059,
        "proxy.json",
        "Manage proxy delegations and delayed proxy announcements from EVM.",
        false
    ),
    precompile!(
        "address-mapping",
        2060,
        "addressMapping.json",
        "The chain's own h160 -> ss58-mirror mapping (addressMapping(address) -> bytes32).",
        false
    ),
    precompile!(
        "voting-power",
        2061,
        "votingPower.json",
        "Validator voting power (stake EMA) per subnet: per-hotkey, totals, tracking state.",
        false
    ),
    precompile!(
        "balance",
        2062,
        "balance.json",
        "Read free TAO balances and dispatch signed Balances operations.",
        false
    ),
    precompile!(
        "scheduler",
        2063,
        "scheduler.json",
        "Read stable metadata for scheduled runtime calls.",
        false
    ),
    precompile!(
        "drand",
        2064,
        "drand.json",
        "Read stored Drand beacon configuration and pulse data.",
        false
    ),
    precompile!(
        "timestamp",
        2065,
        "timestamp.json",
        "Read the runtime timestamp and its per-block update state.",
        false
    ),
    precompile!(
        "runtime-configuration",
        2066,
        "runtimeConfiguration.json",
        "Read stable global runtime configuration.",
        false
    ),
    precompile!(
        "precompile-registry",
        2067,
        "registry.json",
        "Inspect precompile lifecycle and operational availability.",
        false
    ),
    precompile!(
        "ed25519-verify",
        1026,
        "ed25519Verify.json",
        "Verify an ed25519 signature on-chain (prove ss58 key ownership from EVM).",
        false
    ),
    precompile!(
        "sr25519-verify",
        1027,
        "sr25519Verify.json",
        "Verify an sr25519 signature on-chain.",
        false
    ),
];

/// Standard Ethereum precompiles present on the pinned Bittensor runtime.
///
/// These indices follow the pinned runtime, including its BN128 ordering.
pub const STANDARD_PRECOMPILES: &[(&str, u16)] = &[
    ("ecrecover", 1),
    ("sha256", 2),
    ("ripemd160", 3),
    ("identity", 4),
    ("modexp", 5),
    ("dispatch", 6),
    ("bn128-mul", 7),
    ("bn128-pairing", 8),
    ("bn128-add", 9),
    ("sha3-fips256", 1024),
    ("ecrecover-publickey", 1025),
];

/// Named unsigned ABI inputs denominated in native rao rather than EVM wei.
pub const RAO_PARAM_NAMES: &[&str] = &[
    "amount",
    "limit_price",
    "limitPrice",
    "absoluteAmount",
    "increaseAmount",
    "decreaseAmount",
    "tao",
    "alpha",
    "amountRao",
];

/// Find a precompile by its exact catalog name.
pub fn get_precompile(name: &str) -> Result<&'static Precompile> {
    PRECOMPILES
        .iter()
        .find(|item| item.name == name)
        .ok_or_else(|| {
            let mut names = PRECOMPILES.iter().map(|item| item.name).collect::<Vec<_>>();
            names.sort_unstable();
            invalid(format!(
                "unknown precompile {name:?}. Available: {}",
                names.join(", ")
            ))
        })
}

/// The pinned warning for an individually deprecated function, if present.
pub fn function_deprecation(precompile_name: &str, function_name: &str) -> Option<&'static str> {
    match (precompile_name, function_name) {
        ("metagraph", "getRank") => {
            Some("always returns 0 on chain; derive standing from getIncentive")
        }
        ("metagraph", "getTrust") => Some("always returns 0 on chain; trust is no longer computed"),
        _ => None,
    }
}

/// The substrate role played by the caller's ss58 mirror, when documented.
pub fn caller_role(precompile_name: &str, function_name: &str) -> Option<&'static str> {
    match (precompile_name, function_name) {
        ("balance-transfer", "transfer") => Some("sending account (must hold the funds)"),
        ("staking-v2", "addStake" | "addStakeLimit") => Some("coldkey (owns the resulting stake)"),
        (
            "staking-v2",
            "removeStake"
            | "removeStakeLimit"
            | "removeStakeFull"
            | "removeStakeFullLimit"
            | "moveStake"
            | "moveStakeLimit"
            | "transferStake"
            | "burnAlpha",
        ) => Some("coldkey (must own the stake)"),
        ("staking-v2", "addProxy" | "removeProxy") => Some("delegating coldkey"),
        ("staking-v2", "approve" | "increaseAllowance" | "decreaseAllowance") => {
            Some("approving coldkey")
        }
        ("staking-v2", "transferStakeFrom") => Some("approved spender"),
        ("neuron", "burnedRegister") => Some("coldkey (pays the burn, owns the neuron)"),
        (
            "neuron",
            "setWeights" | "commitWeights" | "revealWeights" | "serveAxon" | "serveAxonTls"
            | "servePrometheus",
        ) => Some("hotkey (must be the registered hotkey)"),
        ("subnet", "registerNetwork") => Some("subnet owner coldkey (becomes the owner)"),
        ("proxy", "addProxy" | "removeProxy") => Some("delegating account"),
        _ => None,
    }
}

/// Parse a CLI-shaped JSON value into one typed ABI argument.
///
/// Arrays accept JSON arrays, JSON array strings, or comma-separated strings;
/// tuples accept JSON arrays. Byte parameters additionally accept ss58 keys.
/// Integer input is exact decimal or `0x` hexadecimal, without float rounding.
pub fn coerce_argument(abi_type: &str, raw: &Value) -> Result<DynSolValue> {
    check_type_text(abi_type)?;
    let ty = DynSolType::parse(abi_type).map_err(abi_error)?;
    check_type(&ty, 0)?;
    coerce_value(&ty, raw, &mut Budget::new(), 0)
}

/// ABI-encode a generic contract function, including its four-byte selector.
///
/// Solidity integer widths are preserved. Use [`Precompile::encode_call`] for
/// native precompile calls so native rao amounts also receive a `u64` check.
pub fn encode_call(function: &Function, args: &[Value]) -> Result<String> {
    encode_values(function, &coerce_inputs(&function.inputs, args)?)
}

fn encode_values(function: &Function, values: &[DynSolValue]) -> Result<String> {
    check_byte_length(add(4, encoded_arguments_size(values)?)?)?;
    let bytes = function.abi_encode_input(values).map_err(abi_error)?;
    check_byte_length(bytes.len())?;
    Ok(format!("0x{}", hex::encode(bytes)))
}

/// Decode an `eth_call` 0x-hex result into JSON-compatible values.
///
/// Full-width integers remain JSON integer tokens; bytes become 0x-hex and
/// tuples become arrays. An empty result follows Python's empty-list behavior.
pub fn decode_result(function: &Function, data: &str) -> Result<Vec<Value>> {
    if data.len() > MAX_ABI_BYTES * 2 + 2 {
        return Err(invalid("ABI result exceeds 16 MiB limit"));
    }
    let bytes = hex::decode(data.strip_prefix("0x").unwrap_or(data)).map_err(abi_error)?;
    decode_result_bytes(function, &bytes)
}

/// Decode an `eth_call` result supplied as raw ABI bytes.
pub fn decode_result_bytes(function: &Function, data: &[u8]) -> Result<Vec<Value>> {
    check_byte_length(data.len())?;
    if function.outputs.is_empty() || data.is_empty() {
        return Ok(Vec::new());
    }
    let types = resolve_params(&function.outputs)?;
    // Preflight the decoded size before asking Alloy to allocate nested values.
    // Repeated offsets otherwise let a small reply expand into enormous output.
    scan_sequence(&types, data, 0, &mut Budget::new(), 0)?;
    let decoded = function.abi_decode_output(data).map_err(abi_error)?;
    decoded.iter().map(jsonable).collect()
}

/// Echo coerced arguments with ss58 keys and exact native rao denominations.
pub fn describe_arguments(function: &Function, args: &[Value]) -> Result<BTreeMap<String, String>> {
    let values = coerce_inputs(&function.inputs, args)?;
    let mut described = BTreeMap::new();
    for ((param, value), raw) in function.inputs.iter().zip(&values).zip(args) {
        let text = match value {
            DynSolValue::FixedBytes(word, 32) => {
                let ss58 =
                    addresses::pubkey_to_ss58_with_prefix(word.as_slice(), crate::SS58_FORMAT)?;
                format!("0x{} (ss58 {ss58})", hex::encode(word))
            }
            DynSolValue::Uint(amount, _) if RAO_PARAM_NAMES.contains(&param.name.as_str()) => {
                let amount = native_amount(*amount, &param.name)?;
                format!(
                    "{} rao (= {}.{:09} TAO/alpha)",
                    grouped(amount),
                    grouped(amount / 1_000_000_000),
                    amount % 1_000_000_000
                )
            }
            _ => raw
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| raw.to_string()),
        };
        described.insert(param.name.clone(), text);
    }
    Ok(described)
}

pub(crate) fn find_function<'a>(
    abi: &'a JsonAbi,
    name: &str,
    source: &str,
) -> Result<&'a Function> {
    let found = if name.contains('(') {
        abi.functions()
            .find(|function| function.signature() == name)
    } else {
        abi.function(name).and_then(|functions| functions.first())
    };
    found.ok_or_else(|| {
        let available = abi
            .functions()
            .map(|function| function.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        invalid(format!(
            "{source} has no function {name:?}. Available: {}",
            if available.is_empty() {
                "(none)"
            } else {
                &available
            }
        ))
    })
}

pub(crate) fn coerce_inputs(params: &[Param], args: &[Value]) -> Result<Vec<DynSolValue>> {
    if params.len() != args.len() {
        return Err(invalid(format!(
            "expected {} ABI argument(s), got {}",
            params.len(),
            args.len()
        )));
    }
    let types = resolve_params(params)?;
    let mut budget = Budget::new();
    types
        .iter()
        .zip(args)
        .map(|(ty, value)| coerce_value(ty, value, &mut budget, 0))
        .collect()
}

fn resolve_params(params: &[Param]) -> Result<Vec<DynSolType>> {
    if params.len() > MAX_ABI_ITEMS {
        return Err(invalid("ABI parameter list exceeds item limit"));
    }
    params
        .iter()
        .map(|param| {
            check_param(param, 0)?;
            let ty = param.resolve().map_err(abi_error)?;
            check_type(&ty, 0)?;
            Ok(ty)
        })
        .collect()
}

fn check_param(param: &Param, depth: usize) -> Result<()> {
    if depth > MAX_ABI_DEPTH || param.components.len() > MAX_ABI_ITEMS {
        return Err(invalid("ABI parameter exceeds nesting or item limit"));
    }
    check_type_text(&param.ty)?;
    for component in &param.components {
        check_param(component, depth + 1)?;
    }
    Ok(())
}

fn check_type_text(text: &str) -> Result<()> {
    if text.len() > MAX_ABI_BYTES
        || text
            .bytes()
            .filter(|byte| matches!(byte, b'[' | b'('))
            .count()
            > MAX_ABI_DEPTH
    {
        return Err(invalid("ABI type exceeds nesting or size limit"));
    }
    Ok(())
}

fn check_type(ty: &DynSolType, depth: usize) -> Result<usize> {
    if depth > MAX_ABI_DEPTH {
        return Err(invalid("ABI type exceeds nesting limit"));
    }
    let words =
        match ty {
            DynSolType::Array(inner) => check_type(inner, depth + 1)?,
            DynSolType::FixedArray(inner, count) => check_type(inner, depth + 1)?
                .checked_mul(*count)
                .ok_or_else(|| invalid("ABI fixed array is too large"))?,
            DynSolType::Tuple(types) => {
                let mut total = 0usize;
                for inner in types {
                    total = total
                        .checked_add(check_type(inner, depth + 1)?)
                        .ok_or_else(|| invalid("ABI tuple is too large"))?;
                }
                total.max(1)
            }
            DynSolType::Int(bits) | DynSolType::Uint(bits)
                if !(8..=256).contains(bits) || !bits.is_multiple_of(8) =>
            {
                return Err(invalid("invalid Solidity integer width"));
            }
            DynSolType::FixedBytes(size) if !(1..=32).contains(size) => {
                return Err(invalid("invalid Solidity fixed-bytes width"));
            }
            _ => 1,
        };
    if words > MAX_ABI_ITEMS {
        return Err(invalid("ABI type exceeds item limit"));
    }
    Ok(words)
}

fn coerce_value(
    ty: &DynSolType,
    raw: &Value,
    budget: &mut Budget,
    depth: usize,
) -> Result<DynSolValue> {
    budget.item(depth)?;
    match ty {
        DynSolType::Array(inner) | DynSolType::FixedArray(inner, _) => {
            let parts = argument_parts(raw)?;
            if let DynSolType::FixedArray(_, count) = ty
                && parts.len() != *count
            {
                return Err(invalid(format!(
                    "expected {count} array entries, got {}",
                    parts.len()
                )));
            }
            if parts.len() > budget.items {
                return Err(invalid("ABI argument exceeds item limit"));
            }
            let values = parts
                .iter()
                .map(|part| coerce_value(inner, part, budget, depth + 1))
                .collect::<Result<Vec<_>>>()?;
            Ok(if matches!(ty, DynSolType::Array(_)) {
                DynSolValue::Array(values)
            } else {
                DynSolValue::FixedArray(values)
            })
        }
        DynSolType::Tuple(types) => {
            let parts = argument_parts(raw)?;
            if parts.len() != types.len() {
                return Err(invalid(format!(
                    "expected {} tuple entries, got {}",
                    types.len(),
                    parts.len()
                )));
            }
            Ok(DynSolValue::Tuple(
                types
                    .iter()
                    .zip(parts.iter())
                    .map(|(ty, part)| coerce_value(ty, part, budget, depth + 1))
                    .collect::<Result<Vec<_>>>()?,
            ))
        }
        DynSolType::String => {
            let text = argument_text(raw)?;
            budget.bytes(text.len())?;
            Ok(DynSolValue::String(text))
        }
        DynSolType::Bool => {
            budget.bytes(32)?;
            let text = argument_text(raw)?;
            Ok(DynSolValue::Bool(matches!(
                text.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "y"
            )))
        }
        DynSolType::Bytes | DynSolType::FixedBytes(_) => {
            let text = argument_text(raw)?;
            budget.bytes(text.len().max(32))?;
            let text = text.trim();
            let bare_h160 = text.len() == 40 && text.bytes().all(|byte| byte.is_ascii_hexdigit());
            let converted;
            let hex_text = if !text.starts_with("0x") && !bare_h160 {
                converted = addresses::ss58_to_pubkey(text)?;
                converted.as_str()
            } else {
                text
            };
            let bytes =
                hex::decode(hex_text.strip_prefix("0x").unwrap_or(hex_text)).map_err(abi_error)?;
            if let DynSolType::FixedBytes(size) = ty {
                if bytes.len() > *size {
                    return Err(invalid(format!(
                        "bytes{size} argument has {} bytes",
                        bytes.len()
                    )));
                }
                let mut word = [0u8; 32];
                word[..bytes.len()].copy_from_slice(&bytes);
                Ok(DynSolValue::FixedBytes(B256::from(word), *size))
            } else {
                Ok(DynSolValue::Bytes(bytes))
            }
        }
        _ => {
            let text = argument_text(raw)?;
            budget.bytes(text.len().max(32))?;
            let text = text.trim();
            if matches!(ty, DynSolType::Int(_) | DynSolType::Uint(_)) {
                // Do not silently accept Solidity units, decimals or exponents.
                let digits = text.strip_prefix("0x").unwrap_or_else(|| {
                    text.strip_prefix('-')
                        .or_else(|| text.strip_prefix('+'))
                        .unwrap_or(text)
                });
                let valid = !digits.is_empty()
                    && if text.starts_with("0x") {
                        digits.bytes().all(|byte| byte.is_ascii_hexdigit())
                    } else {
                        digits.bytes().all(|byte| byte.is_ascii_digit())
                    };
                if !valid {
                    return Err(invalid(
                        "ABI integers require exact decimal or 0x hexadecimal input",
                    ));
                }
            }
            let text = if matches!(ty, DynSolType::Uint(_)) {
                text.strip_prefix('+').unwrap_or(text)
            } else {
                text
            };
            let value = ty.coerce_str(text).map_err(abi_error)?;
            check_integer_width(&value)?;
            Ok(value)
        }
    }
}

fn argument_text(raw: &Value) -> Result<String> {
    match raw {
        Value::String(text) => {
            check_byte_length(text.len())?;
            Ok(text.clone())
        }
        Value::Number(number) => Ok(number.to_string()),
        Value::Bool(value) => Ok(value.to_string()),
        _ => Err(invalid("expected a scalar ABI argument")),
    }
}

fn argument_parts(raw: &Value) -> Result<Cow<'_, [Value]>> {
    match raw {
        Value::Array(parts) => {
            if parts.len() > MAX_ABI_ITEMS {
                return Err(invalid("ABI array exceeds item limit"));
            }
            Ok(Cow::Borrowed(parts))
        }
        Value::String(text) => {
            check_byte_length(text.len())?;
            let text = text.trim();
            if text.starts_with('[') {
                let parts = serde_json::from_str::<Vec<Value>>(text).map_err(Error::Json)?;
                if parts.len() > MAX_ABI_ITEMS {
                    return Err(invalid("ABI array exceeds item limit"));
                }
                Ok(Cow::Owned(parts))
            } else {
                let parts = text
                    .split(',')
                    .map(|part| Value::String(part.trim().to_owned()))
                    .collect::<Vec<_>>();
                if parts.len() > MAX_ABI_ITEMS {
                    return Err(invalid("ABI array exceeds item limit"));
                }
                Ok(Cow::Owned(parts))
            }
        }
        _ => Err(invalid("expected an ABI array or tuple")),
    }
}

fn check_native_amounts(params: &[Param], values: &[DynSolValue]) -> Result<()> {
    for (param, value) in params.iter().zip(values) {
        if RAO_PARAM_NAMES.contains(&param.name.as_str())
            && let DynSolValue::Uint(amount, _) = value
        {
            native_amount(*amount, &param.name)?;
        }
    }
    Ok(())
}

fn native_amount(amount: U256, name: &str) -> Result<u64> {
    u64::try_from(amount)
        .map_err(|_| invalid(format!("native rao argument {name:?} exceeds u64::MAX")))
}

fn grouped(amount: u64) -> String {
    let digits = amount.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

pub(crate) fn encoded_arguments_size(values: &[DynSolValue]) -> Result<usize> {
    let mut total = 0usize;
    for value in values {
        let size = match value {
            DynSolValue::Bytes(bytes) => add(64, add(bytes.len(), 31)? / 32 * 32)?,
            DynSolValue::String(text) => add(64, add(text.len(), 31)? / 32 * 32)?,
            DynSolValue::Array(values) => add(64, encoded_arguments_size(values)?)?,
            DynSolValue::FixedArray(values) | DynSolValue::Tuple(values) => {
                let inner = encoded_arguments_size(values)?;
                if values.iter().any(value_is_dynamic) {
                    add(32, inner)?
                } else {
                    inner
                }
            }
            _ => 32,
        };
        total = add(total, size)?;
        check_byte_length(total)?;
    }
    Ok(total)
}

fn value_is_dynamic(value: &DynSolValue) -> bool {
    match value {
        DynSolValue::Bytes(_) | DynSolValue::String(_) | DynSolValue::Array(_) => true,
        DynSolValue::FixedArray(values) | DynSolValue::Tuple(values) => {
            values.iter().any(value_is_dynamic)
        }
        _ => false,
    }
}

fn jsonable(value: &DynSolValue) -> Result<Value> {
    check_integer_width(value)?;
    match value {
        DynSolValue::Bool(value) => Ok(Value::Bool(*value)),
        DynSolValue::Int(value, _) => serde_json::from_str(&value.to_string()).map_err(Error::Json),
        DynSolValue::Uint(value, _) => {
            serde_json::from_str(&value.to_string()).map_err(Error::Json)
        }
        DynSolValue::FixedBytes(word, size) => {
            Ok(Value::String(format!("0x{}", hex::encode(&word[..*size]))))
        }
        DynSolValue::Bytes(bytes) => Ok(Value::String(format!("0x{}", hex::encode(bytes)))),
        DynSolValue::Address(address) => Ok(Value::String(format!(
            "0x{}",
            hex::encode(address.as_slice())
        ))),
        DynSolValue::Function(function) => Ok(Value::String(format!(
            "0x{}",
            hex::encode(function.as_slice())
        ))),
        DynSolValue::String(text) => Ok(Value::String(text.clone())),
        DynSolValue::Array(values)
        | DynSolValue::FixedArray(values)
        | DynSolValue::Tuple(values) => values
            .iter()
            .map(jsonable)
            .collect::<Result<Vec<_>>>()
            .map(Value::Array),
    }
}

fn check_integer_width(value: &DynSolValue) -> Result<()> {
    match value {
        DynSolValue::Int(value, bits) => {
            // Apply the signed lower bound explicitly, including decoded
            // sub-256-bit values whose high bits are not canonical.
            let limit = U256::from(1u8) << (bits - 1);
            let magnitude = value.unsigned_abs();
            if (value.is_negative() && magnitude > limit)
                || (!value.is_negative() && magnitude >= limit)
            {
                return Err(invalid(format!("integer is outside int{bits}")));
            }
        }
        DynSolValue::Uint(value, bits) if value.bit_len() > *bits => {
            return Err(invalid(format!("integer is outside uint{bits}")));
        }
        _ => {}
    }
    Ok(())
}

struct Budget {
    items: usize,
    remaining_bytes: usize,
}

impl Budget {
    fn new() -> Self {
        Self {
            items: MAX_ABI_ITEMS,
            remaining_bytes: MAX_ABI_BYTES,
        }
    }
    fn item(&mut self, depth: usize) -> Result<()> {
        if depth > MAX_ABI_DEPTH {
            return Err(invalid("ABI value exceeds nesting limit"));
        }
        self.items = self
            .items
            .checked_sub(1)
            .ok_or_else(|| invalid("ABI value exceeds item limit"))?;
        Ok(())
    }
    fn bytes(&mut self, count: usize) -> Result<()> {
        self.remaining_bytes = self
            .remaining_bytes
            .checked_sub(count)
            .ok_or_else(|| invalid("decoded ABI value exceeds 16 MiB limit"))?;
        Ok(())
    }
}

fn check_byte_length(length: usize) -> Result<()> {
    if length > MAX_ABI_BYTES {
        return Err(invalid("ABI input exceeds 16 MiB limit"));
    }
    Ok(())
}

fn abi_error(error: impl std::fmt::Display) -> Error {
    invalid(format!("invalid ABI: {error}"))
}
fn invalid(message: impl Into<String>) -> Error {
    Error::Validation(message.into())
}

fn is_dynamic(ty: &DynSolType) -> bool {
    match ty {
        DynSolType::Bytes | DynSolType::String | DynSolType::Array(_) => true,
        DynSolType::FixedArray(inner, _) => is_dynamic(inner),
        DynSolType::Tuple(types) => types.iter().any(is_dynamic),
        _ => false,
    }
}

fn head_size(ty: &DynSolType) -> Result<usize> {
    if is_dynamic(ty) {
        return Ok(32);
    }
    match ty {
        DynSolType::FixedArray(inner, count) => head_size(inner)?.checked_mul(*count),
        DynSolType::Tuple(types) => types
            .iter()
            .try_fold(0usize, |sum, ty| sum.checked_add(head_size(ty).ok()?)),
        _ => Some(32),
    }
    .ok_or_else(|| invalid("ABI head size overflow"))
}

fn at(data: &[u8], start: usize, length: usize) -> Result<&[u8]> {
    data.get(
        start
            ..start
                .checked_add(length)
                .ok_or_else(|| invalid("ABI offset overflow"))?,
    )
    .ok_or_else(|| invalid("truncated ABI data"))
}

fn usize_word(data: &[u8], start: usize) -> Result<usize> {
    let word = at(data, start, 32)?;
    if word[..24].iter().any(|byte| *byte != 0) {
        return Err(invalid("ABI length or offset exceeds host limit"));
    }
    let mut tail = [0u8; 8];
    tail.copy_from_slice(&word[24..]);
    usize::try_from(u64::from_be_bytes(tail)).map_err(abi_error)
}

fn add(left: usize, right: usize) -> Result<usize> {
    left.checked_add(right)
        .ok_or_else(|| invalid("ABI offset overflow"))
}

fn scan_sequence(
    types: &[DynSolType],
    data: &[u8],
    base: usize,
    budget: &mut Budget,
    depth: usize,
) -> Result<()> {
    let mut size = 0usize;
    for ty in types {
        size = add(size, head_size(ty)?)?;
    }
    at(data, base, size)?;
    let mut head = base;
    for ty in types {
        scan_head(ty, data, base, head, size, budget, depth)?;
        head = add(head, head_size(ty)?)?;
    }
    Ok(())
}

fn scan_repeated(
    ty: &DynSolType,
    count: usize,
    data: &[u8],
    base: usize,
    budget: &mut Budget,
    depth: usize,
) -> Result<()> {
    if count > budget.items {
        return Err(invalid("ABI array exceeds item limit"));
    }
    let step = head_size(ty)?;
    let size = step
        .checked_mul(count)
        .ok_or_else(|| invalid("ABI array size overflow"))?;
    at(data, base, size)?;
    let mut head = base;
    for _ in 0..count {
        scan_head(ty, data, base, head, size, budget, depth)?;
        head = add(head, step)?;
    }
    Ok(())
}

fn scan_head(
    ty: &DynSolType,
    data: &[u8],
    base: usize,
    head: usize,
    size: usize,
    budget: &mut Budget,
    depth: usize,
) -> Result<()> {
    let value = if is_dynamic(ty) {
        let offset = usize_word(data, head)?;
        if offset < size || !offset.is_multiple_of(32) {
            return Err(invalid("invalid ABI tail offset"));
        }
        add(base, offset)?
    } else {
        head
    };
    scan_value(ty, data, value, budget, depth)
}

fn scan_value(
    ty: &DynSolType,
    data: &[u8],
    start: usize,
    budget: &mut Budget,
    depth: usize,
) -> Result<()> {
    budget.item(depth)?;
    match ty {
        DynSolType::Bytes | DynSolType::String => {
            let length = usize_word(data, start)?;
            let padded_length = add(length, 31)? / 32 * 32;
            let payload = at(data, add(start, 32)?, padded_length)?;
            if payload[length..].iter().any(|byte| *byte != 0) {
                return Err(invalid("nonzero ABI bytes/string padding"));
            }
            if matches!(ty, DynSolType::String) {
                std::str::from_utf8(&payload[..length]).map_err(abi_error)?;
            }
            budget.bytes(add(32, length)?)
        }
        DynSolType::Array(inner) => {
            let count = usize_word(data, start)?;
            scan_repeated(inner, count, data, add(start, 32)?, budget, depth + 1)
        }
        DynSolType::FixedArray(inner, count) => {
            scan_repeated(inner, *count, data, start, budget, depth + 1)
        }
        DynSolType::Tuple(types) => scan_sequence(types, data, start, budget, depth + 1),
        _ => {
            let word = at(data, start, 32)?;
            let valid = match ty {
                DynSolType::Uint(bits) => word[..32 - bits / 8].iter().all(|byte| *byte == 0),
                DynSolType::Int(bits) => {
                    let first = 32 - bits / 8;
                    let padding = if word[first] & 0x80 == 0 { 0 } else { 0xff };
                    word[..first].iter().all(|byte| *byte == padding)
                }
                DynSolType::Bool => word[..31].iter().all(|byte| *byte == 0) && word[31] <= 1,
                DynSolType::FixedBytes(size) => word[*size..].iter().all(|byte| *byte == 0),
                DynSolType::Address => word[..12].iter().all(|byte| *byte == 0),
                DynSolType::Function => word[24..].iter().all(|byte| *byte == 0),
                _ => false,
            };
            if !valid {
                return Err(invalid("noncanonical ABI scalar word"));
            }
            budget.bytes(32)
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn catalog_embeds_every_registered_abi_and_runtime_index() {
        let expected = [
            ("balance-transfer", 2048),
            ("staking", 2049),
            ("metagraph", 2050),
            ("subnet", 2051),
            ("neuron", 2052),
            ("staking-v2", 2053),
            ("uid-lookup", 2054),
            ("alpha", 2056),
            ("crowdloan", 2057),
            ("leasing", 2058),
            ("proxy", 2059),
            ("address-mapping", 2060),
            ("voting-power", 2061),
            ("balance", 2062),
            ("scheduler", 2063),
            ("drand", 2064),
            ("timestamp", 2065),
            ("runtime-configuration", 2066),
            ("precompile-registry", 2067),
            ("ed25519-verify", 1026),
            ("sr25519-verify", 1027),
        ];
        assert_eq!(PRECOMPILES.len(), expected.len());
        for (precompile, (name, index)) in PRECOMPILES.iter().zip(expected) {
            assert_eq!((precompile.name, precompile.index), (name, index));
            assert_eq!(precompile.address().len(), 42);
            assert!(!precompile.functions().expect("embedded ABI").is_empty());
            assert_eq!(get_precompile(name).unwrap(), precompile);
        }
        assert_eq!(
            get_precompile("balance-transfer").unwrap().address(),
            "0x0000000000000000000000000000000000000800"
        );
        assert!(get_precompile("staking").unwrap().deprecated);
        assert_eq!(STANDARD_PRECOMPILES.len(), 11);
        assert!(STANDARD_PRECOMPILES.contains(&("bn128-add", 9)));
        assert!(get_precompile("unknown").is_err());
    }

    #[test]
    fn catalog_roles_and_deprecations_match_pinned_dispatch_semantics() {
        assert_eq!(
            function_deprecation("metagraph", "getTrust"),
            Some("always returns 0 on chain; trust is no longer computed")
        );
        assert_eq!(
            caller_role("neuron", "setWeights"),
            Some("hotkey (must be the registered hotkey)")
        );
        assert_eq!(
            caller_role("staking-v2", "transferStakeFrom"),
            Some("approved spender")
        );
        assert_eq!(caller_role("balance", "freeBalance"), None);
        for name in [
            "approve",
            "increaseAllowance",
            "decreaseAllowance",
            "transferStakeFrom",
        ] {
            // pyt: tests/unit/test_evm.py::TestPrecompileEncoding::test_staking_v2_state_changers_are_nonpayable
            let function = get_precompile("staking-v2")
                .unwrap()
                .function(name)
                .unwrap();
            let value = serde_json::to_value(function).unwrap();
            assert_eq!(value["stateMutability"], "nonpayable");
        }
    }

    #[test]
    fn ethereum_transfer_vector_has_known_selector_and_exact_words() {
        // Standard ERC20 fixture: transfer(address,uint256) has selector a9059cbb.
        let function = Function::parse("transfer(address,uint256)").unwrap();
        let encoded = encode_call(
            &function,
            &[
                json!("0x1111111111111111111111111111111111111111"),
                json!(1000),
            ],
        )
        .unwrap();
        assert_eq!(
            encoded,
            concat!(
                "0xa9059cbb",
                "0000000000000000000000001111111111111111111111111111111111111111",
                "00000000000000000000000000000000000000000000000000000000000003e8"
            )
        );
        assert!(encode_call(&function, &[]).is_err());
        assert!(encode_call(&function, &[json!("bad-address"), json!(1)]).is_err());
    }

    #[test]
    fn ss58_bytes32_and_bounded_array_calls_encode_exact_arguments() {
        // pyt: tests/unit/test_evm.py::TestPrecompileEncoding::test_coerce_argument_accepts_ss58_for_bytes32
        let alice = "5GrwvaEF5zXb26Fz9rcQpDWS64D1AU2pEdPhBY6VZC6pQYgB";
        let public = "0xd43593c715fdd31c61141abd04a99fd6822c8558854ccde39a5684e7a56da27d";
        let staking = get_precompile("staking-v2").unwrap();
        let from_ss58 = staking
            .encode_call(
                "addStake",
                &[json!(alice), json!(1_000_000_000u64), json!(1)],
            )
            .unwrap();
        let from_hex = staking
            .encode_call(
                "addStake",
                &[json!(public), json!(1_000_000_000u64), json!(1)],
            )
            .unwrap();
        assert_eq!(from_ss58, from_hex);
        assert!(from_hex[10..].starts_with(&public[2..]));
        // pyt: tests/unit/test_evm.py::TestPrecompileEncoding::test_balance_transfer_encode
        assert!(
            get_precompile("balance-transfer")
                .unwrap()
                .encode_call("transfer", &[json!(public)])
                .is_ok()
        );
        // pyt: tests/unit/test_evm.py::TestPrecompileEncoding::test_total_coldkey_stake_on_subnet_encode
        assert!(
            staking
                .encode_call("getTotalColdkeyStakeOnSubnet", &[json!(public), json!(1)])
                .is_ok()
        );
        // pyt: tests/unit/test_evm.py::TestPrecompileEncoding::test_new_bounded_array_calls_encode
        let claim = staking
            .encode_call("claimRoot", &[json!("[1, 2]")])
            .unwrap();
        let claim_csv = staking.encode_call("claimRoot", &[json!("1,2")]).unwrap();
        assert_eq!(claim, claim_csv);
        assert!(
            get_precompile("neuron")
                .unwrap()
                .encode_call(
                    "batchCommitWeights",
                    &[
                        json!("[1,2]"),
                        json!([
                            format!("0x{}", "11".repeat(32)),
                            format!("0x{}", "22".repeat(32))
                        ])
                    ]
                )
                .is_ok()
        );
    }

    #[test]
    fn native_amounts_are_u64_but_generic_contract_integers_keep_256_bits() {
        let precompile = get_precompile("staking-v2").unwrap();
        let public = format!("0x{}", "11".repeat(32));
        assert!(
            precompile
                .encode_call("addStake", &[json!(public), json!(u64::MAX), json!(1)])
                .is_ok()
        );
        assert!(
            precompile
                .encode_call(
                    "addStake",
                    &[json!(public), json!("18446744073709551616"), json!(1)]
                )
                .is_err()
        );
        let function = Function::parse("f(uint256)").unwrap();
        let max = "115792089237316195423570985008687907853269984665640564039457584007913129639935";
        assert!(
            encode_call(&function, &[json!(max)])
                .unwrap()
                .ends_with(&"ff".repeat(32))
        );
        assert!(encode_call(&function, &[json!(format!("{max}0"))]).is_err());
        let small = Function::parse("f(uint8,int8)").unwrap();
        assert!(encode_call(&small, &[json!(255), json!(-128)]).is_ok());
        assert!(encode_call(&small, &[json!(256), json!(0)]).is_err());
        assert!(encode_call(&small, &[json!(0), json!(-129)]).is_err());
        for value in ["1.5", "1e3", "1 ether"] {
            assert!(encode_call(&function, &[json!(value)]).is_err());
        }
        assert_eq!(
            encode_call(&function, &[json!("+1")]).unwrap(),
            encode_call(&function, &[json!(1)]).unwrap()
        );
    }

    #[test]
    fn dynamic_results_keep_bytes_tuples_and_full_width_integers() {
        let function: Function = serde_json::from_value(json!({"type":"function","name":"read","inputs":[],"outputs":[{"name":"n","type":"uint256"},{"name":"b","type":"bytes"}],"stateMutability":"view"})).unwrap();
        // Hand-derived ABI: max uint256; bytes tail at 64; two-byte payload 0xabcd.
        let max = "ff".repeat(32);
        let data = format!("0x{max}{:064x}{:064x}abcd{:0<60}", 64, 2, "");
        let result = decode_result(&function, &data).unwrap();
        assert_eq!(result[0].to_string(), U256::MAX.to_string());
        assert_eq!(result[1], json!("0xabcd"));
        assert_eq!(decode_result(&function, "0x").unwrap(), Vec::<Value>::new());
        assert!(decode_result(&function, "0x01").is_err());
        let signed: Function = serde_json::from_value(json!({"type":"function","name":"read","inputs":[],"outputs":[{"name":"n","type":"int256"}],"stateMutability":"view"})).unwrap();
        assert_eq!(
            decode_result(&signed, &format!("0x{}", "ff".repeat(32))).unwrap(),
            vec![json!(-1)]
        );
        let tuple: Function = serde_json::from_value(json!({"type":"function","name":"read","inputs":[],"outputs":[{"name":"v","type":"tuple[]","components":[{"name":"n","type":"uint256"},{"name":"ok","type":"bool"}]}],"stateMutability":"view"})).unwrap();
        let data = format!("0x{:064x}{:064x}{:064x}{:064x}", 32, 1, 7, 1);
        assert_eq!(
            decode_result(&tuple, &data).unwrap(),
            vec![json!([[7, true]])]
        );
    }

    #[test]
    fn decode_refuses_huge_arrays_and_offset_amplification_before_allocation() {
        let function: Function = serde_json::from_value(json!({"type":"function","name":"read","inputs":[],"outputs":[{"name":"v","type":"bytes[]"}],"stateMutability":"view"})).unwrap();
        let enormous = format!("0x{:064x}{}", 32, "ff".repeat(32));
        assert!(decode_result(&function, &enormous).is_err());
        let invalid_head = format!("0x{:064x}{:064x}{:064x}", 32, 1, 0);
        assert!(decode_result(&function, &invalid_head).is_err());
        let giant_type = Function::parse("f(uint256[999999999999999999])").unwrap();
        assert!(encode_call(&giant_type, &[json!([])]).is_err());
        let mut budget = Budget {
            items: 10,
            remaining_bytes: 60,
        };
        // Both array offsets point to the same four-byte body. Two decoded
        // copies require 72 budget bytes even though only one tail is stored.
        let data = hex::decode(format!(
            "{:064x}{:064x}{:064x}{:064x}{:064x}abcd0000{:0<56}",
            32, 2, 64, 64, 4, ""
        ))
        .unwrap();
        assert!(
            scan_sequence(
                &[DynSolType::Array(Box::new(DynSolType::Bytes))],
                &data,
                0,
                &mut budget,
                0
            )
            .is_err()
        );
    }

    #[test]
    fn decode_refuses_noncanonical_scalar_words_padding_and_invalid_utf8() {
        let function = |ty: &str| -> Function {
            serde_json::from_value(json!({"type":"function","name":"read","inputs":[],"outputs":[{"name":"v","type":ty}],"stateMutability":"view"})).unwrap()
        };
        for (ty, word) in [
            ("uint8", format!("{:064x}", 256)),
            ("int8", format!("{:064x}", 255)),
            ("bool", format!("{:064x}", 2)),
            ("address", format!("01{}", "00".repeat(31))),
            ("bytes1", format!("aa{:0<60}ff", "")),
            ("function", format!("{}01", "00".repeat(31))),
        ] {
            assert!(
                decode_result(&function(ty), &format!("0x{word}")).is_err(),
                "{ty}"
            );
        }
        let invalid_utf8 = format!("0x{:064x}{:064x}ff{:0<62}", 32, 1, "");
        assert!(decode_result(&function("string"), &invalid_utf8).is_err());
        let bad_padding = format!("0x{:064x}{:064x}aaff{:0<60}", 32, 1, "");
        assert!(decode_result(&function("bytes"), &bad_padding).is_err());
        let nested = format!("0x{:064x}{:064x}{:064x}", 32, 1, 256);
        assert!(decode_result(&function("uint8[]"), &nested).is_err());
    }

    #[test]
    fn valid_shared_offsets_gaps_and_trailing_data_match_python_decoding() {
        let function = |ty: &str| -> Function {
            serde_json::from_value(json!({"type":"function","name":"read","inputs":[],"outputs":[{"name":"v","type":ty}],"stateMutability":"view"})).unwrap()
        };
        let shared = format!(
            "0x{:064x}{:064x}{:064x}{:064x}{:064x}abcd0000{:0<56}",
            32, 2, 64, 64, 4, ""
        );
        assert_eq!(
            decode_result(&function("bytes[]"), &shared).unwrap(),
            vec![json!(["0xabcd0000", "0xabcd0000"])]
        );
        let gap = format!("0x{:064x}{:064x}{:064x}abcd{:0<60}", 64, 0, 2, "");
        assert_eq!(
            decode_result(&function("bytes"), &gap).unwrap(),
            vec![json!("0xabcd")]
        );
        assert_eq!(
            decode_result(&function("uint256"), &format!("0x{:064x}ff", 7)).unwrap(),
            vec![json!(7)]
        );
    }

    #[test]
    fn previews_use_exact_native_amounts_and_ss58() {
        let function = get_precompile("staking-v2")
            .unwrap()
            .function("addStake")
            .unwrap();
        let described = describe_arguments(
            &function,
            &[
                json!(format!("0x{}", "11".repeat(32))),
                json!(u64::MAX),
                json!(1),
            ],
        )
        .unwrap();
        assert!(described["hotkey"].contains(" (ss58 5"));
        assert_eq!(
            described["amount"],
            "18,446,744,073,709,551,615 rao (= 18,446,744,073.709551615 TAO/alpha)"
        );
    }
}
