// Derived from RaoFoundation/subtensor sdk/python/bittensor/emission_weights.py @ 6a972693ae9580541225bd37696ac2fee26b8ae9 (Apache-2.0); modified by rtao.
//! Per-subnet emission weight vector: slot names, rules, parsing and renormalisation.
//!
//! A subnet's alpha emission is divided by a five-slot vector
//! `[burn, recycle, owner, validator, miner]` whose entries are `u64` parts of
//! `u64::MAX`, so the slots sum to exactly one. The owner or root sets it with
//! `AdminUtils.sudo_set_subnet_emission_weights(netuid, uids, values)`, a
//! sparse update: every `uids[i]` names a slot and `values[i]` its new share.
//! Omitted owner/validator/miner slots keep their current value; omitted burn
//! and recycle are treated as zero. The chain then checks the v1 rules:
//!
//! * burn (0) and recycle (1) must be zero, reserved for a later runtime;
//! * `0 <= owner <= 0.33`; lease-created subnets may not drop below the
//!   launch default (18% on mainnet);
//! * `validator <= 0.50`, and it must stay above root's baseline share. Every
//!   block the chain pays root `min(V, r)` of the subnet's emission off the
//!   top of the validator slot, where `r` is what root would earn there with
//!   the default validator share and the global `TaoWeight`. A set with
//!   `V <= r` is rejected because validators would get nothing;
//! * `miner >= 0.33`;
//! * the five slots sum to exactly one.
//!
//! Everything here is pure, so callers can preview and explain a change
//! before submitting it; [`validate`](crate::emission_weights::validate) mirrors the on-chain checks and names
//! the same error codes.

use std::{borrow::Cow, fmt};

use num_bigint::{BigInt, BigUint, Sign};
use num_traits::{ToPrimitive, Zero};
use serde::Serialize;

use crate::python;

/// Denominator of every emission weight (`u64::MAX`).
pub const ONE: u64 = u64::MAX;

/// Slot order on chain; the index of a name is its `uid`.
pub const SLOT_NAMES: [&str; 5] = ["burn", "recycle", "owner", "validator", "miner"];
/// Burn slot index, reserved and pinned to zero.
pub const BURN: usize = 0;
/// Recycle slot index, reserved and pinned to zero.
pub const RECYCLE: usize = 1;
/// Subnet owner slot index.
pub const OWNER: usize = 2;
/// Validator dividend slot index.
pub const VALIDATOR: usize = 3;
/// Miner incentive slot index.
pub const MINER: usize = 4;

/// Slots the v1 rules let an owner move (burn/recycle are pinned to zero).
pub const ADJUSTABLE_SLOTS: [usize; 3] = [OWNER, VALIDATOR, MINER];

/// A complete emission weight vector in u64 parts of [`ONE`].
pub type Weights = [u64; 5];

// py: sdk/python/bittensor/emission_weights.py:52-54
/// `numerator / denominator` in parts of [`ONE`], floored as on chain.
///
/// The quotient saturates at [`ONE`]; callers pass proper fractions.
pub const fn fraction(numerator: u64, denominator: u64) -> u64 {
    let parts = (ONE as u128 * numerator as u128) / denominator as u128;
    if parts > ONE as u128 {
        ONE
    } else {
        parts as u64
    }
}

/// Inclusive maximum owner share (33%).
pub const MAX_OWNER: u64 = fraction(33, 100);
/// Inclusive maximum validator share (50%).
pub const MAX_VALIDATOR: u64 = fraction(50, 100);
/// Inclusive minimum miner share (33%).
pub const MIN_MINER: u64 = fraction(33, 100);

/// Mainnet launch owner cut `11796 / 65535` scaled to u64 parts.
///
/// It is also the floor for lease-created subnets and the base from which the
/// default vector `[0, 0, 0.18, 0.41, 0.41]` is derived.
pub const DEFAULT_OWNER: u64 = fraction(11796, 65535);

/// Validator share of the default vector (`V0`, about 41% on mainnet).
///
/// Root's baseline share is anchored to it: whatever a subnet sets, root is
/// paid what it would earn at `V0`.
pub const DEFAULT_VALIDATOR: u64 = (ONE - DEFAULT_OWNER) / 2;

/// How often a subnet owner may change the vector, in blocks (about 7 days).
///
/// Root is exempt and a subnet's first change is always allowed. Mirrors the
/// chain's `SubnetEmissionWeightsRateLimit`.
pub const OWNER_RATE_LIMIT_BLOCKS: u64 = 50_400;
/// Nominal block duration used for human wait estimates.
pub const BLOCK_SECONDS: u64 = 12;

/// A rejected slot, share or vector, carrying Python's `ValueError` text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmissionWeightsError(String);

impl EmissionWeightsError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }

    /// The diagnostic text.
    pub fn message(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for EmissionWeightsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for EmissionWeightsError {}

fn slot_listing() -> String {
    SLOT_NAMES
        .iter()
        .enumerate()
        .map(|(index, name)| format!("{index} {name}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// A slot given by name, by uid, or by any other JSON-native value.
#[derive(Clone, Debug, PartialEq)]
pub enum SlotInput<'a> {
    /// A slot name or decimal uid, matched case-insensitively after trimming.
    Text(Cow<'a, str>),
    /// A numeric uid.
    Uid(BigInt),
    /// A float uid, truncated like Python's `int(slot)` once in range.
    Float(f64),
    /// A boolean, which Python treats as the integer uid 0 or 1.
    Boolean(bool),
}

impl<'a> From<&'a str> for SlotInput<'a> {
    fn from(value: &'a str) -> Self {
        Self::Text(Cow::Borrowed(value))
    }
}

impl From<String> for SlotInput<'static> {
    fn from(value: String) -> Self {
        Self::Text(Cow::Owned(value))
    }
}

impl From<usize> for SlotInput<'static> {
    fn from(value: usize) -> Self {
        Self::Uid(value.into())
    }
}

impl From<i64> for SlotInput<'static> {
    fn from(value: i64) -> Self {
        Self::Uid(value.into())
    }
}

// py: sdk/python/bittensor/emission_weights.py:80-97
/// Resolve a slot given by name (`"owner"`) or uid (`2`).
pub fn slot_index<'a>(slot: impl Into<SlotInput<'a>>) -> Result<usize, EmissionWeightsError> {
    match slot.into() {
        SlotInput::Text(text) => {
            let key = text.trim_matches(python::whitespace).to_lowercase();
            if !key.is_empty() && key.chars().all(|c| python::decimal_digit(c).is_some()) {
                let uid = python::integer(&key).ok_or_else(|| {
                    EmissionWeightsError::new(format!(
                        "invalid literal for int() with base 10: {}",
                        python::string_repr(&key)
                    ))
                })?;
                return slot_index(SlotInput::Uid(uid));
            }
            SLOT_NAMES
                .iter()
                .position(|name| *name == key)
                .ok_or_else(|| {
                    EmissionWeightsError::new(format!(
                        "unknown emission slot {}; slots are {}",
                        python::string_repr(&text),
                        slot_listing()
                    ))
                })
        }
        SlotInput::Uid(uid) => uid
            .to_usize()
            .filter(|uid| *uid < SLOT_NAMES.len())
            .ok_or_else(|| out_of_range(&uid.to_string())),
        SlotInput::Float(uid) => {
            if (0.0..SLOT_NAMES.len() as f64).contains(&uid) {
                Ok(uid.trunc() as usize)
            } else {
                Err(out_of_range(&crate::pyfmt::float_repr(uid)))
            }
        }
        SlotInput::Boolean(flag) => Ok(usize::from(flag)),
    }
}

fn out_of_range(shown: &str) -> EmissionWeightsError {
    EmissionWeightsError::new(format!(
        "emission slot {shown} is out of range; slots are {}",
        slot_listing()
    ))
}

/// Resolve a JSON slot the way Python resolves its decoded `int | str`.
pub fn slot_index_json(slot: &serde_json::Value) -> Result<usize, EmissionWeightsError> {
    use serde_json::Value;
    match slot {
        Value::String(text) => slot_index(text.as_str()),
        Value::Bool(flag) => slot_index(SlotInput::Boolean(*flag)),
        Value::Number(number) => match number_input(number) {
            Some(Ok(integer)) => slot_index(SlotInput::Uid(integer)),
            Some(Err(float)) => slot_index(SlotInput::Float(float)),
            None => Err(EmissionWeightsError::new(format!(
                "unknown emission slot {number}; slots are {}",
                slot_listing()
            ))),
        },
        other => Err(EmissionWeightsError::new(format!(
            "unknown emission slot {other}; slots are {}",
            slot_listing()
        ))),
    }
}

/// An exact integer token, or the float Python's JSON decoder would produce.
fn number_input(number: &serde_json::Number) -> Option<Result<BigInt, f64>> {
    let text = number.to_string();
    if text.contains(['.', 'e', 'E']) {
        number.as_f64().map(Err)
    } else {
        text.parse().ok().map(Ok)
    }
}

/// A human share: raw parts, a 0..1 float, text, or a rejected boolean.
#[derive(Clone, Debug, PartialEq)]
pub enum ShareInput<'a> {
    /// Raw u64 parts of [`ONE`], retaining oversized input for diagnostics.
    Raw(BigInt),
    /// A 0..1 fraction, read exactly from its shortest decimal spelling.
    Float(f64),
    /// A fraction (`0.41`), percentage (`41%`) or raw part count.
    Text(Cow<'a, str>),
    /// A boolean, rejected instead of being treated as a share.
    Boolean(bool),
}

impl<'a> From<&'a str> for ShareInput<'a> {
    fn from(value: &'a str) -> Self {
        Self::Text(Cow::Borrowed(value))
    }
}

impl From<String> for ShareInput<'static> {
    fn from(value: String) -> Self {
        Self::Text(Cow::Owned(value))
    }
}

impl From<f64> for ShareInput<'static> {
    fn from(value: f64) -> Self {
        Self::Float(value)
    }
}

impl From<u64> for ShareInput<'static> {
    fn from(value: u64) -> Self {
        Self::Raw(value.into())
    }
}

impl From<i64> for ShareInput<'static> {
    fn from(value: i64) -> Self {
        Self::Raw(value.into())
    }
}

impl From<u128> for ShareInput<'static> {
    fn from(value: u128) -> Self {
        Self::Raw(value.into())
    }
}

impl From<bool> for ShareInput<'static> {
    fn from(value: bool) -> Self {
        Self::Boolean(value)
    }
}

fn unreadable(shown: &str) -> EmissionWeightsError {
    EmissionWeightsError::new(format!(
        "cannot read emission share {shown}; give a 0..1 fraction (0.41), a percentage (41%), or raw u64 parts"
    ))
}

// py: sdk/python/bittensor/emission_weights.py:100-131
/// Turn a human share into u64 parts.
///
/// Accepts a 0..1 fraction (`0.41` / `"0.41"`), a percentage (`"41%"`), or a
/// raw integer part count. Integers are raw parts; anything with a decimal
/// point, exponent or percent sign is a fraction, floored to parts exactly as
/// the chain floors its bounds.
pub fn parse_share<'a>(value: impl Into<ShareInput<'a>>) -> Result<u64, EmissionWeightsError> {
    let value = value.into();
    let (parts, shown) = match value {
        ShareInput::Boolean(_) => {
            return Err(EmissionWeightsError::new(
                "emission share must be a number, not a boolean",
            ));
        }
        ShareInput::Raw(raw) => {
            let shown = raw.to_string();
            (raw, shown)
        }
        ShareInput::Float(float) => {
            let shown = crate::pyfmt::float_repr(float);
            let exact = exact_decimal(&shown).ok_or_else(|| unreadable(&shown))?;
            return fraction_to_parts(&exact, &shown);
        }
        ShareInput::Text(original) => {
            let shown = python::string_repr(&original);
            let text = original.trim_matches(python::whitespace).replace('_', "");
            if text.is_empty() {
                return Err(EmissionWeightsError::new("emission share is empty"));
            }
            if let Some(percentage) = text.strip_suffix('%') {
                let mut exact = exact_decimal(percentage).ok_or_else(|| unreadable(&shown))?;
                exact.exponent -= 2;
                return fraction_to_parts(&exact, &shown);
            }
            if text.contains('.') || text.to_lowercase().contains('e') {
                let exact = exact_decimal(&text).ok_or_else(|| unreadable(&shown))?;
                return fraction_to_parts(&exact, &shown);
            }
            let raw = python::integer(&text).ok_or_else(|| unreadable(&shown))?;
            (raw, shown)
        }
    };
    parts.to_u64().ok_or_else(|| {
        EmissionWeightsError::new(format!(
            "emission share {shown} is outside 0..1 (0..{ONE} parts)"
        ))
    })
}

/// Parse a JSON share the way Python parses its decoded `int | float | str`.
pub fn parse_share_json(value: &serde_json::Value) -> Result<u64, EmissionWeightsError> {
    use serde_json::Value;
    match value {
        Value::Bool(flag) => parse_share(*flag),
        Value::String(text) => parse_share(text.as_str()),
        Value::Number(number) => match number_input(number) {
            Some(Ok(integer)) => parse_share(ShareInput::Raw(integer)),
            Some(Err(float)) => parse_share(float),
            None => Err(unreadable(&number.to_string())),
        },
        other => Err(unreadable(&other.to_string())),
    }
}

/// An exact signed decimal `coefficient * 10^exponent`.
struct Decimal {
    negative: bool,
    coefficient: BigUint,
    exponent: i64,
}

// Python's `Fraction(Decimal(text))` for finite decimal strings. Infinities
// and NaNs cannot become fractions and are reported as unreadable.
fn exact_decimal(text: &str) -> Option<Decimal> {
    let text = text.trim_matches(python::whitespace);
    let mut chars = text.chars().peekable();
    let negative = match chars.peek() {
        Some('-') => {
            chars.next();
            true
        }
        Some('+') => {
            chars.next();
            false
        }
        _ => false,
    };
    let mut digits = String::new();
    let mut fractional = 0i64;
    let mut seen_point = false;
    let mut mantissa_digits = 0usize;
    while let Some(&character) = chars.peek() {
        if let Some(digit) = python::decimal_digit(character) {
            digits.push(char::from(b'0' + digit));
            mantissa_digits += 1;
            if seen_point {
                fractional += 1;
            }
        } else if character == '.' && !seen_point {
            seen_point = true;
        } else {
            break;
        }
        chars.next();
    }
    if mantissa_digits == 0 {
        return None;
    }
    let mut exponent = BigInt::zero();
    if let Some(&indicator) = chars.peek() {
        if !matches!(indicator, 'e' | 'E') {
            return None;
        }
        chars.next();
        let mut exponent_text = String::new();
        if let Some(&sign @ ('+' | '-')) = chars.peek() {
            exponent_text.push(sign);
            chars.next();
        }
        let mut exponent_digits = 0usize;
        for character in chars.by_ref() {
            exponent_text.push(char::from(b'0' + python::decimal_digit(character)?));
            exponent_digits += 1;
        }
        if exponent_digits == 0 {
            return None;
        }
        exponent = exponent_text.parse().ok()?;
    }
    if chars.next().is_some() {
        return None;
    }
    let coefficient: BigUint = digits.parse().ok()?;
    // Exponents beyond i64 only matter for nonzero coefficients, whose value is
    // then decided by the sign of the exponent alone.
    let exponent = exponent - BigInt::from(fractional);
    let exponent = exponent
        .to_i64()
        .unwrap_or(if exponent.sign() == Sign::Minus {
            i64::MIN
        } else {
            i64::MAX
        });
    Some(Decimal {
        negative,
        coefficient,
        exponent,
    })
}

// py: sdk/python/bittensor/emission_weights.py:134-149
fn fraction_to_parts(value: &Decimal, shown: &str) -> Result<u64, EmissionWeightsError> {
    let outside = || {
        EmissionWeightsError::new(format!(
            "emission share {shown} must be between 0 and 1 (0% and 100%)"
        ))
    };
    if value.coefficient.is_zero() {
        return Ok(0);
    }
    if value.negative {
        return Err(outside());
    }
    let digits = i64::try_from(value.coefficient.to_str_radix(10).len()).unwrap_or(i64::MAX);
    let magnitude = digits.saturating_add(value.exponent);
    if magnitude > 1 {
        // At least ten.
        return Err(outside());
    }
    if magnitude <= -20 {
        // Below 1e-20, under one part of u64::MAX.
        return Ok(0);
    }
    let one = BigUint::from(ONE);
    let parts = if value.exponent >= 0 {
        let scaled =
            &value.coefficient * BigUint::from(10u8).pow(value.exponent.unsigned_abs() as u32);
        if scaled > BigUint::from(1u8) {
            return Err(outside());
        }
        scaled * one
    } else {
        let denominator = BigUint::from(10u8).pow(value.exponent.unsigned_abs() as u32);
        if value.coefficient > denominator {
            return Err(outside());
        }
        one * &value.coefficient / denominator
    };
    Ok(parts.to_u64().unwrap_or(ONE))
}

// py: sdk/python/bittensor/emission_weights.py:152-154
/// u64 parts as a 0..1 float, rounding the exact quotient once.
pub fn share_to_float(parts: u64) -> f64 {
    python::int_div_big_to_f64(&BigInt::from(parts), u128::from(ONE)).unwrap_or(f64::NAN)
}

// py: sdk/python/bittensor/emission_weights.py:157-159
/// Parts (u64 or a wider sum) as a percentage with `places` decimals, such as `"41.00%"`.
pub fn format_share(parts: impl Into<u128>, places: usize) -> String {
    let share = python::int_div_big_to_f64(&BigInt::from(parts.into()), u128::from(ONE))
        .unwrap_or(f64::NAN);
    format!("{:.places$}%", share * 100.0)
}

/// Allowed range of one slot, in the machine-readable shape.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SlotLimit {
    /// Lowest accepted share as a 0..1 float, absent when unknown.
    pub min: Option<f64>,
    /// Highest accepted share as a 0..1 float.
    pub max: f64,
    /// Lowest accepted share in exact parts, absent when unknown.
    pub min_parts: Option<u64>,
    /// Highest accepted share in exact parts.
    pub max_parts: u64,
    /// Why the range applies.
    pub why: &'static str,
}

impl SlotLimit {
    fn new(low: Option<u64>, high: u64, why: &'static str) -> Self {
        Self {
            min: low.map(share_to_float),
            max: share_to_float(high),
            min_parts: low,
            max_parts: high,
            why,
        }
    }
}

/// Allowed range of every slot, in slot order.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Limits {
    /// Burn slot, pinned to zero.
    pub burn: SlotLimit,
    /// Recycle slot, pinned to zero.
    pub recycle: SlotLimit,
    /// Owner slot.
    pub owner: SlotLimit,
    /// Validator slot, floored just above root's baseline share when known.
    pub validator: SlotLimit,
    /// Miner slot.
    pub miner: SlotLimit,
}

impl Limits {
    /// `(slot name, limit)` pairs in slot order.
    pub fn entries(&self) -> [(&'static str, &SlotLimit); 5] {
        [
            (SLOT_NAMES[BURN], &self.burn),
            (SLOT_NAMES[RECYCLE], &self.recycle),
            (SLOT_NAMES[OWNER], &self.owner),
            (SLOT_NAMES[VALIDATOR], &self.validator),
            (SLOT_NAMES[MINER], &self.miner),
        ]
    }
}

// py: sdk/python/bittensor/emission_weights.py:162-231
/// The per-subnet inputs the chain rules depend on.
///
/// `min_validator` is the lowest validator share in parts the chain accepts
/// at this block, one part above root's baseline share, or `None` when it is
/// unknown. `min_owner` is the lease floor, applied only to lease-created
/// subnets.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rules {
    /// Lowest accepted validator share in parts, when chain state is known.
    pub min_validator: Option<u64>,
    /// Owner floor for lease-created subnets.
    pub min_owner: u64,
    /// Whether the subnet was created through a lease.
    pub created_via_lease: bool,
}

const RESERVED_WHY: &str = "reserved for a future runtime; must stay 0";

impl Rules {
    /// Allowed range per slot, with `validator.min` the live floor.
    pub fn limits(&self) -> Limits {
        let owner_min = if self.created_via_lease {
            self.min_owner
        } else {
            0
        };
        Limits {
            burn: SlotLimit::new(Some(0), 0, RESERVED_WHY),
            recycle: SlotLimit::new(Some(0), 0, RESERVED_WHY),
            owner: SlotLimit::new(
                Some(owner_min),
                MAX_OWNER,
                if self.created_via_lease {
                    "lease-created subnet: contributors are paid from the owner cut"
                } else {
                    "0 turns the owner cut off"
                },
            ),
            validator: SlotLimit::new(
                self.min_validator,
                MAX_VALIDATOR,
                if self.min_validator.is_some() {
                    "must exceed root's baseline share, which is paid off the top of this slot"
                } else {
                    "must exceed root's baseline share (unknown without chain state)"
                },
            ),
            miner: SlotLimit::new(Some(MIN_MINER), ONE, "miner incentive floor"),
        }
    }

    /// `(slot, allowed range, why)` rows for human output, ending with `sum`.
    pub fn table(&self) -> Vec<(&'static str, String, String)> {
        let limits = self.limits();
        let mut rows: Vec<_> = limits
            .entries()
            .into_iter()
            .map(|(name, limit)| {
                let places = if name == "validator" { 4 } else { 2 };
                let (low, high) = (limit.min_parts, limit.max_parts);
                let rule = match low {
                    Some(low) if low == high => format!("= {}", format_share(high, 0)),
                    None => format!("<= {}", format_share(high, places)),
                    Some(low) if high == ONE => format!(">= {}", format_share(low, places)),
                    Some(low) => format!(
                        "{} to {}",
                        format_share(low, places),
                        format_share(high, places)
                    ),
                };
                (name, rule, limit.why.to_owned())
            })
            .collect();
        rows.push((
            "sum",
            "= 100%".into(),
            format!("all five slots add up to exactly {ONE} parts"),
        ));
        rows
    }
}

// py: sdk/python/bittensor/emission_weights.py:234-242
/// One broken rule: the chain error name and an explanation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Violation {
    /// Chain error variant the runtime would return.
    pub error: &'static str,
    /// Human explanation.
    pub message: String,
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.error, self.message)
    }
}

// py: sdk/python/bittensor/emission_weights.py:245-249
/// `[0, 0, owner, rest/2, rest - rest/2]`, the chain's seed vector.
pub fn default_weights(owner: u64) -> Weights {
    let validator = (ONE - owner) / 2;
    [0, 0, owner, validator, ONE - owner - validator]
}

// py: sdk/python/bittensor/emission_weights.py:252-258
/// Coerce a chain-shaped vector of at most five slots to five entries.
pub fn normalize_weights(weights: &[u64]) -> Result<Weights, EmissionWeightsError> {
    if weights.len() > SLOT_NAMES.len() {
        return Err(EmissionWeightsError::new(format!(
            "emission weight vector has {} slots; at most 5 are known",
            weights.len()
        )));
    }
    let mut out = [0; 5];
    out[..weights.len()].copy_from_slice(weights);
    Ok(out)
}

// py: sdk/python/bittensor/emission_weights.py:261-270
/// Apply a sparse update exactly as the chain does.
///
/// Named slots are replaced, omitted owner/validator/miner keep their value,
/// and omitted burn/recycle become zero.
pub fn merge(current: &Weights, updates: &[(usize, u64)]) -> Weights {
    let mut merged = *current;
    merged[BURN] = 0;
    merged[RECYCLE] = 0;
    for &(slot, value) in updates {
        merged[slot] = value;
    }
    merged
}

// py: sdk/python/bittensor/emission_weights.py:273-317
/// Pin the `fixed` slots and rescale the other adjustable slots to sum to one.
///
/// Untouched owner/validator/miner slots keep their relative proportions,
/// split evenly when all are zero; the last of them in slot order absorbs
/// the rounding remainder. Fails if the pinned shares alone exceed one, if a
/// reserved slot is pinned to a nonzero share, or if every adjustable slot is
/// pinned but the total is not one.
pub fn renormalize(
    current: &Weights,
    fixed: &[(usize, u64)],
) -> Result<Weights, EmissionWeightsError> {
    let mut pinned: Vec<(usize, u64)> = Vec::new();
    for &(slot, value) in fixed {
        if let Some(entry) = pinned.iter_mut().find(|(known, _)| *known == slot) {
            entry.1 = value;
        } else {
            pinned.push((slot, value));
        }
    }
    for slot in [BURN, RECYCLE] {
        match pinned.iter().find(|(known, _)| *known == slot) {
            Some(&(_, value)) if value != 0 => {
                return Err(EmissionWeightsError::new(format!(
                    "{} (slot {slot}) must stay 0 in this runtime",
                    SLOT_NAMES[slot]
                )));
            }
            Some(_) => {}
            None => pinned.push((slot, 0)),
        }
    }
    let free: Vec<usize> = ADJUSTABLE_SLOTS
        .into_iter()
        .filter(|slot| !pinned.iter().any(|(known, _)| known == slot))
        .collect();
    let total: u128 = pinned.iter().map(|(_, value)| u128::from(*value)).sum();
    let Some(remaining) = u128::from(ONE).checked_sub(total) else {
        return Err(EmissionWeightsError::new(format!(
            "the pinned shares alone exceed 100%: {}",
            pinned
                .iter()
                .filter(|(_, value)| *value != 0)
                .map(|(slot, value)| format!("{}={}", SLOT_NAMES[*slot], format_share(*value, 2)))
                .collect::<Vec<_>>()
                .join(", ")
        )));
    };
    let mut out = *current;
    for &(slot, value) in &pinned {
        out[slot] = value;
    }
    if free.is_empty() {
        if remaining != 0 {
            return Err(EmissionWeightsError::new(format!(
                "all adjustable slots are pinned but they sum to {}, not 100%",
                format_share(total, 2)
            )));
        }
        return Ok(out);
    }
    let free_total: u128 = free.iter().map(|slot| u128::from(current[*slot])).sum();
    let mut shares: Vec<u128> = if free_total == 0 {
        vec![remaining / free.len() as u128; free.len()]
    } else {
        // remaining and each slot fit in u64, so the product fits in u128.
        free.iter()
            .map(|slot| remaining * u128::from(current[*slot]) / free_total)
            .collect()
    };
    let assigned: u128 = shares.iter().sum();
    if let Some(last) = shares.last_mut() {
        *last += remaining - assigned;
    }
    for (slot, value) in free.into_iter().zip(shares) {
        // The free shares sum to `remaining`, which is at most ONE.
        out[slot] = u64::try_from(value).unwrap_or(ONE);
    }
    Ok(out)
}

// py: sdk/python/bittensor/emission_weights.py:320-395
/// Every rule the chain would reject, in the order it checks them.
pub fn validate(weights: &Weights, rules: &Rules) -> Vec<Violation> {
    let w = weights;
    let mut found = Vec::new();
    let mut push = |error, message: String| found.push(Violation { error, message });
    if w[BURN] != 0 {
        push(
            "BurnEmissionMustBeZero",
            format!(
                "burn (slot 0) is {}; burning emission is reserved for a future runtime, so it must be 0 (or omitted)",
                format_share(w[BURN], 2)
            ),
        );
    }
    if w[RECYCLE] != 0 {
        push(
            "RecycleEmissionMustBeZero",
            format!(
                "recycle (slot 1) is {}; recycling emission is reserved for a future runtime, so it must be 0 (or omitted)",
                format_share(w[RECYCLE], 2)
            ),
        );
    }
    if w[OWNER] > MAX_OWNER {
        push(
            "OwnerEmissionAboveMaximum",
            format!(
                "owner (slot 2) is {}; the owner cut may be at most {}",
                format_share(w[OWNER], 2),
                format_share(MAX_OWNER, 2)
            ),
        );
    }
    if rules.created_via_lease && w[OWNER] < rules.min_owner {
        push(
            "LeasedSubnetOwnerCutBelowMinimum",
            format!(
                "owner (slot 2) is {}; this subnet was created through a lease, so its owner cut may not drop below {} (lease contributors are paid from it)",
                format_share(w[OWNER], 2),
                format_share(rules.min_owner, 2)
            ),
        );
    }
    if w[VALIDATOR] > MAX_VALIDATOR {
        push(
            "ValidatorEmissionAboveMaximum",
            format!(
                "validator (slot 3) is {}; validator dividends may draw at most {}",
                format_share(w[VALIDATOR], 2),
                format_share(MAX_VALIDATOR, 2)
            ),
        );
    }
    if let Some(min_validator) = rules.min_validator
        && w[VALIDATOR] < min_validator
    {
        push(
            "ValidatorEmissionTooLowToPreserveRoot",
            format!(
                "validator (slot 3) is {}; root is paid its baseline share of emission off the top of this slot every block, so right now the validator share must be above {} or validators would get nothing",
                format_share(w[VALIDATOR], 4),
                format_share(min_validator.saturating_sub(1), 4)
            ),
        );
    }
    if w[MINER] < MIN_MINER {
        push(
            "MinerEmissionBelowMinimum",
            format!(
                "miner (slot 4) is {}; miner incentive must receive at least {}",
                format_share(w[MINER], 2),
                format_share(MIN_MINER, 2)
            ),
        );
    }
    let total: u128 = w.iter().map(|value| u128::from(*value)).sum();
    if total != u128::from(ONE) {
        push(
            "EmissionWeightsDoNotSumToOne",
            format!(
                "the five slots sum to {} ({total} parts), not exactly 100% ({ONE} parts); renormalise another slot in the same call (`btcli subnets emission set-value` does this for you)",
                format_share(total, 6)
            ),
        );
    }
    found
}

// py: sdk/python/bittensor/emission_weights.py:398-409
/// Root's share of the subnet's emission, `V * x / (x + A)` with `x = root_tao * tao_weight`.
///
/// `validator` and `tao_weight` are 0..1 floats; `root_tao` and
/// `alpha_issuance` are raw balances in any common unit.
pub fn root_emission_share(
    validator: f64,
    tao_weight: f64,
    root_tao: f64,
    alpha_issuance: f64,
) -> f64 {
    let weighted = root_tao * tao_weight;
    if weighted + alpha_issuance <= 0.0 {
        return 0.0;
    }
    validator * weighted / (weighted + alpha_issuance)
}

// py: sdk/python/bittensor/emission_weights.py:412-421
/// Root's baseline share: what it earns with `V0` and the global `TaoWeight`.
pub fn baseline_root_share(global_tao_weight: f64, root_tao: f64, alpha_issuance: f64) -> f64 {
    root_emission_share(
        share_to_float(DEFAULT_VALIDATOR),
        global_tao_weight,
        root_tao,
        alpha_issuance,
    )
}

// py: sdk/python/bittensor/emission_weights.py:424-428
/// The validator share (0..1) a set must exceed right now: root's baseline share.
pub fn validator_floor(global_tao_weight: f64, root_tao: f64, alpha_issuance: f64) -> f64 {
    baseline_root_share(global_tao_weight, root_tao, alpha_issuance)
}

// py: sdk/python/bittensor/emission_weights.py:431-434
/// Lowest accepted validator share in parts: one part above root's baseline.
pub fn min_validator_parts(baseline_parts: u64) -> u64 {
    baseline_parts.saturating_add(1)
}

// py: sdk/python/bittensor/emission_weights.py:437-442
/// What root is paid (0..1): the baseline, capped at the validator slot.
pub fn root_share_paid(validator_parts: u64, baseline_parts: u64) -> f64 {
    share_to_float(validator_parts.min(baseline_parts))
}

/// A vector as 0..1 floats keyed by slot name.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Shares {
    /// Burn slot share.
    pub burn: f64,
    /// Recycle slot share.
    pub recycle: f64,
    /// Owner slot share.
    pub owner: f64,
    /// Validator slot share.
    pub validator: f64,
    /// Miner slot share.
    pub miner: f64,
}

impl Shares {
    /// `(slot name, share)` pairs in slot order.
    pub fn entries(&self) -> [(&'static str, f64); 5] {
        [
            (SLOT_NAMES[BURN], self.burn),
            (SLOT_NAMES[RECYCLE], self.recycle),
            (SLOT_NAMES[OWNER], self.owner),
            (SLOT_NAMES[VALIDATOR], self.validator),
            (SLOT_NAMES[MINER], self.miner),
        ]
    }
}

// py: sdk/python/bittensor/emission_weights.py:445-449
/// `{slot name: "41.00%"}` for display, in slot order.
pub fn describe(weights: &Weights) -> [(&'static str, String); 5] {
    std::array::from_fn(|slot| (SLOT_NAMES[slot], format_share(weights[slot], 2)))
}

// py: sdk/python/bittensor/emission_weights.py:452-456
/// The vector as 0..1 floats keyed by slot name.
pub fn shares(weights: &Weights) -> Shares {
    Shares {
        burn: share_to_float(weights[BURN]),
        recycle: share_to_float(weights[RECYCLE]),
        owner: share_to_float(weights[OWNER]),
        validator: share_to_float(weights[VALIDATOR]),
        miner: share_to_float(weights[MINER]),
    }
}

/// One side of a previewed change.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct VectorPreview {
    /// Full vector in parts.
    pub weights: Weights,
    /// The same vector as 0..1 shares.
    pub shares: Shares,
}

impl VectorPreview {
    fn new(weights: Weights) -> Self {
        Self {
            weights,
            shares: shares(&weights),
        }
    }
}

/// What root and alpha validators earn before and after a change.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RootPreview {
    /// Root's paid share before the change.
    pub share_before: f64,
    /// Root's paid share after, absent when the chain would reject the change.
    pub share_after: Option<f64>,
    /// Root's live baseline share.
    pub baseline_share: f64,
    /// What alpha validators keep before the change.
    pub validators_before: f64,
    /// What alpha validators keep after, absent when rejected.
    pub validators_after: Option<f64>,
    /// Whether root's cut took the whole validator slot before.
    pub capped_before: bool,
    /// Whether root's cut takes the whole validator slot after.
    pub capped_after: bool,
    /// The share the validator slot must exceed.
    pub validator_floor: f64,
    /// Global `TaoWeight` as a 0..1 float.
    pub global_tao_weight: f64,
    /// Root subnet TAO in rao.
    pub root_tao: u64,
    /// The subnet's alpha issuance in rao.
    pub alpha_issuance: u128,
}

/// When the subnet owner may next change the vector.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct RateLimitStatus {
    /// Owner rate limit in blocks.
    pub limit_blocks: u64,
    /// Block of the owner's last change, zero when never changed.
    pub last_change_block: u64,
    /// First block at which the owner may change it again.
    pub next_allowed_block: u64,
    /// Blocks remaining until then.
    pub blocks_until_allowed: u64,
    /// Whether the owner may change it at the current block.
    pub owner_can_change_now: bool,
}

/// The machine-readable preview of a vector change.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Preview {
    /// The current vector.
    pub before: VectorPreview,
    /// The proposed vector.
    pub after: VectorPreview,
    /// Slots that move, in slot order.
    pub changed: Vec<&'static str>,
    /// Root's and alpha validators' shares before and after.
    pub root: RootPreview,
    /// Owner rate-limit status, attached by the intent's preflight.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate_limit: Option<RateLimitStatus>,
    /// Allowed ranges, attached by the intent's preflight.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limits: Option<Limits>,
}

/// Live chain inputs for [`preview`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RootInputs {
    /// Root's baseline share in parts, as the runtime API reports it.
    pub baseline_parts: u64,
    /// Global `TaoWeight` as a 0..1 float.
    pub global_tao_weight: f64,
    /// Root subnet TAO in rao.
    pub root_tao: u64,
    /// The subnet's alpha issuance in rao.
    pub alpha_issuance: u128,
}

// py: sdk/python/bittensor/emission_weights.py:459-506
/// The machine-readable preview of a vector change.
///
/// `share_after` and `validators_after` are absent when `V_new <= r`, which
/// the chain rejects with `ValidatorEmissionTooLowToPreserveRoot`.
/// Comparisons are made in parts so the preview agrees with [`validate`].
pub fn preview(before: &Weights, after: &Weights, inputs: &RootInputs) -> Preview {
    let baseline = inputs.baseline_parts;
    let baseline_share = share_to_float(baseline);
    let rejected = after[VALIDATOR] <= baseline;
    Preview {
        before: VectorPreview::new(*before),
        after: VectorPreview::new(*after),
        changed: (0..SLOT_NAMES.len())
            .filter(|slot| before[*slot] != after[*slot])
            .map(|slot| SLOT_NAMES[slot])
            .collect(),
        root: RootPreview {
            share_before: root_share_paid(before[VALIDATOR], baseline),
            share_after: (!rejected).then(|| root_share_paid(after[VALIDATOR], baseline)),
            baseline_share,
            validators_before: share_to_float(before[VALIDATOR] - before[VALIDATOR].min(baseline)),
            validators_after: (!rejected).then(|| share_to_float(after[VALIDATOR] - baseline)),
            capped_before: baseline > before[VALIDATOR],
            capped_after: !rejected && baseline > after[VALIDATOR],
            validator_floor: baseline_share,
            global_tao_weight: inputs.global_tao_weight,
            root_tao: inputs.root_tao,
            alpha_issuance: inputs.alpha_issuance,
        },
        rate_limit: None,
        limits: None,
    }
}

// py: sdk/python/bittensor/emission_weights.py:509-526
/// When the subnet owner may next change the vector.
///
/// `last_change_block` is the chain's `LastRateLimitedBlock` entry for the
/// subnet; zero means never changed, which the chain treats as a free first
/// change. Root is never limited.
pub fn rate_limit_status(
    last_change_block: u64,
    current_block: u64,
    limit: u64,
) -> RateLimitStatus {
    let next_allowed_block = if last_change_block == 0 {
        current_block
    } else {
        last_change_block.saturating_add(limit)
    };
    let blocks_until_allowed = next_allowed_block.saturating_sub(current_block);
    RateLimitStatus {
        limit_blocks: limit,
        last_change_block,
        next_allowed_block,
        blocks_until_allowed,
        owner_can_change_now: blocks_until_allowed == 0,
    }
}
