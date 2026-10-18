// Derived from RaoFoundation/subtensor sdk/python/bittensor/reads/subnets.py @ 6a972693ae9580541225bd37696ac2fee26b8ae9 (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/reads/base.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Subnet catalog reads over pinned client views.

use std::collections::BTreeMap;

use futures::TryStreamExt;
use num_traits::ToPrimitive;
use rtao_client::{View, ViewExt};
use rtao_core::{DynValue, codec::Value};
use rtao_primitives::{NetUid, emission_weights as ew};
use rtao_runtime::{runtime_apis as api, storage as st};
use serde::Serialize;
use serde_json::Value as JsonValue;

use crate::ReadError;

/// Python's raw `get_metagraph` record with name and symbol decoded to text.
///
/// Other fields retain the core corpus shapes. A missing subnet yields JSON
/// `null`; the curated typed graph lives in `rtao-metagraph`.
// py: sdk/python/bittensor/reads/subnets.py:170-190
pub async fn metagraph<V: View + ?Sized>(view: &V, netuid: NetUid) -> Result<JsonValue, ReadError> {
    let graph = view
        .call_dyn(
            "SubnetInfoRuntimeApi",
            "get_metagraph",
            &[Value::Uint(u128::from(netuid.0))],
        )
        .await?;
    normalize_metagraph(graph.to_json())
}

fn normalize_metagraph(mut graph: JsonValue) -> Result<JsonValue, ReadError> {
    if let Some(graph) = graph.as_object_mut() {
        for key in ["name", "symbol"] {
            if let Some(value) = graph.get_mut(key) {
                *value = utf8_text(value.take())?;
            }
        }
    }
    Ok(graph)
}

/// Every chain-registered token symbol, keyed by native subnet identity.
// py: sdk/python/bittensor/reads/subnets.py:217-226
pub async fn token_symbols<V: View + ?Sized>(
    view: &V,
) -> Result<BTreeMap<NetUid, String>, ReadError> {
    let snapshot = view.pin().await?;
    let rows: Vec<_> = snapshot
        .storage_iter_dyn("SubtensorModule", "TokenSymbol", Vec::new())
        .try_collect()
        .await?;
    let offload =
        rows.len() > 64 || rows.iter().map(|(_, raw, _, _)| raw.len()).sum::<usize>() > 64 * 1024;
    let decode = move || {
        rows.into_iter()
            .map(|(keys, raw, runtime, info)| {
                let netuid = keys
                    .last()
                    .and_then(rtao_core::value_ext::as_u128)
                    .and_then(|value| u16::try_from(value).ok())
                    .ok_or_else(|| {
                        rtao_client::ClientError::Protocol(
                            "TokenSymbol key is not a native NetUid".into(),
                        )
                    })?;
                let value = rtao_core::ops::storage::decode_storage_bytes(
                    runtime.core(),
                    &info,
                    Some(&raw),
                )
                .map_err(rtao_core::CoreError::from)
                .map_err(rtao_client::ClientError::from)?;
                let value = utf8_text(DynValue::new(value).to_json())?;
                let value = value.as_str().ok_or_else(|| ReadError::SurfaceChanged {
                    descriptor: "SubtensorModule.TokenSymbol",
                    detail: "token symbol is not a byte string".into(),
                })?;
                Ok((NetUid(netuid), value.to_owned()))
            })
            .collect()
    };
    if offload {
        rtao_client::offload(decode).await.map_err(|error| {
            rtao_client::ClientError::Protocol(format!("token symbol decoder task failed: {error}"))
        })?
    } else {
        decode()
    }
}

/// `LastRateLimitedBlock` key for the owner's emission-weights change window.
// py: sdk/python/bittensor/reads/subnets.py:179-181
pub fn emission_weights_rate_limit_key(netuid: NetUid) -> DynValue {
    DynValue::new(Value::record(vec![(
        "OwnerHyperparamUpdate".into(),
        Value::Tuple(vec![
            Value::Uint(u128::from(netuid.0)),
            Value::Str("EmissionWeights".into()),
        ]),
    )]))
}

/// The live values root's baseline share of a subnet's emission comes from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RootPreservationInputs {
    /// Root subnet TAO in rao.
    pub root_tao: u64,
    /// The subnet's alpha issuance in rao: pool alpha in and out plus the
    /// balancer reservoir.
    pub alpha_issuance: u128,
    /// The global `TaoWeight` as a 0..1 float.
    pub global_tao_weight: f64,
}

/// Read root TAO, the subnet's alpha issuance and the global `TaoWeight`.
// py: sdk/python/bittensor/reads/subnets.py:184-197
pub async fn root_preservation_inputs<V: View + ?Sized>(
    view: &V,
    netuid: NetUid,
) -> Result<RootPreservationInputs, ReadError> {
    let (root_tao, alpha_in, alpha_out, reservoir, tao_weight) = view
        .fetch((
            st::subtensor_module::SUBNET_TAO.at((NetUid::ROOT,)),
            st::subtensor_module::SUBNET_ALPHA_IN.at((netuid,)),
            st::subtensor_module::SUBNET_ALPHA_OUT.at((netuid,)),
            st::swap::BALANCER_ALPHA_RESERVOIR.at((netuid,)),
            st::subtensor_module::TAO_WEIGHT.at(()),
        ))
        .await?;
    Ok(RootPreservationInputs {
        root_tao: root_tao.0,
        alpha_issuance: u128::from(alpha_in.0) + u128::from(alpha_out.0) + u128::from(reservoir.0),
        global_tao_weight: ew::share_to_float(tao_weight),
    })
}

/// A subnet's emission weight vector and the rule inputs that bound it.
///
/// Shares are u64 parts of `u64::MAX`; `shares` carries the same values as
/// 0..1 floats keyed by slot name. `baseline_root_share` is root's baseline
/// share of this subnet's emission at `block`; the chain pays root that off
/// the top of the validator slot every block, capped at the slot, so
/// `root_share = min(validator, baseline_root_share)` and alpha validators get
/// `validator_share_after_root`. `root_capped` is true when the baseline has
/// grown past the validator slot. `min_validator` is the lowest validator
/// share in parts the chain accepts at this block, and `last_change_block` is
/// when the owner last changed the vector (zero means never).
// py: sdk/python/bittensor/reads/subnets.py:200-251
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SubnetEmissionWeights {
    /// Subnet identity reported by the runtime.
    pub netuid: NetUid,
    /// Full vector in parts.
    pub weights: ew::Weights,
    /// The vector as 0..1 shares keyed by slot name.
    pub shares: ew::Shares,
    /// Burn slot in parts.
    pub burn: u64,
    /// Recycle slot in parts.
    pub recycle: u64,
    /// Owner slot in parts.
    pub owner: u64,
    /// Validator slot in parts.
    pub validator: u64,
    /// Miner slot in parts.
    pub miner: u64,
    /// Global `TaoWeight` as a 0..1 float.
    pub global_tao_weight: f64,
    /// Whether the subnet was created through a lease.
    pub created_via_lease: bool,
    /// Root subnet TAO in rao.
    pub root_tao: u64,
    /// The subnet's alpha issuance in rao.
    pub alpha_issuance: u128,
    /// What root is paid of this subnet's emission now.
    pub root_share: f64,
    /// Root's baseline share of this subnet's emission.
    pub baseline_root_share: f64,
    /// Root's baseline share in exact parts, as the runtime API reports it.
    pub baseline_root_parts: u64,
    /// What alpha validators share once root is paid.
    pub validator_share_after_root: f64,
    /// Whether the baseline exceeds the validator slot.
    pub root_capped: bool,
    /// Lowest validator share in parts the chain accepts at this block.
    pub min_validator: u64,
    /// Pinned block of this snapshot.
    pub block: u64,
    /// Block of the owner's last change, zero when never changed.
    pub last_change_block: u64,
}

impl SubnetEmissionWeights {
    /// The rule inputs the chain applies to a change at this block.
    pub fn rules(&self) -> ew::Rules {
        ew::Rules {
            min_validator: Some(self.min_validator),
            min_owner: ew::DEFAULT_OWNER,
            created_via_lease: self.created_via_lease,
        }
    }

    /// When the subnet owner may next change the vector (root is exempt).
    pub fn rate_limit(&self) -> ew::RateLimitStatus {
        ew::rate_limit_status(
            self.last_change_block,
            self.block,
            ew::OWNER_RATE_LIMIT_BLOCKS,
        )
    }

    /// The preview inputs for a change from this vector.
    pub fn root_inputs(&self) -> ew::RootInputs {
        ew::RootInputs {
            baseline_parts: self.baseline_root_parts,
            global_tao_weight: self.global_tao_weight,
            root_tao: self.root_tao,
            alpha_issuance: self.alpha_issuance,
        }
    }
}

/// The decoded `get_subnet_emission_weights` runtime record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubnetEmissionWeightsInfo {
    /// Subnet identity.
    pub netuid: NetUid,
    /// The stored vector, padded to five slots.
    pub weights: ew::Weights,
    /// Burn slot.
    pub burn: u64,
    /// Recycle slot.
    pub recycle: u64,
    /// Owner slot.
    pub owner: u64,
    /// Validator slot.
    pub validator: u64,
    /// Miner slot.
    pub miner: u64,
    /// Root's baseline share in parts.
    pub root_baseline_share: u64,
    /// Root's paid share in parts.
    pub root_share: u64,
    /// Whether the subnet was created through a lease.
    pub created_via_lease: bool,
}

const EMISSION_API: &str = "SubnetInfoRuntimeApi.get_subnet_emission_weights";

impl SubnetEmissionWeightsInfo {
    /// Decode the runtime record from its dynamic value.
    pub fn decode(value: &DynValue) -> Result<Self, ReadError> {
        let record = value.to_json();
        let surface = |detail: String| ReadError::SurfaceChanged {
            descriptor: EMISSION_API,
            detail,
        };
        let parts = |name: &str| {
            record
                .get(name)
                .and_then(JsonValue::as_u64)
                .ok_or_else(|| surface(format!("{name} is not a native u64")))
        };
        let weights = record
            .get("weights")
            .and_then(JsonValue::as_array)
            .ok_or_else(|| surface("weights is not a list".into()))?
            .iter()
            .map(|weight| {
                weight
                    .as_u64()
                    .ok_or_else(|| surface("weight is not a native u64".into()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let netuid = u16::try_from(parts("netuid")?)
            .map_err(|_| surface("netuid is not a native NetUid".into()))?;
        Ok(Self {
            netuid: NetUid(netuid),
            weights: ew::normalize_weights(&weights).map_err(|error| surface(error.to_string()))?,
            burn: parts("burn")?,
            recycle: parts("recycle")?,
            owner: parts("owner")?,
            validator: parts("validator")?,
            miner: parts("miner")?,
            root_baseline_share: parts("root_baseline_share")?,
            root_share: parts("root_share")?,
            created_via_lease: record
                .get("created_via_lease")
                .and_then(JsonValue::as_bool)
                .ok_or_else(|| surface("created_via_lease is not a boolean".into()))?,
        })
    }
}

/// The current vector, decoded, or `None` when the subnet does not exist.
pub async fn subnet_emission_weights_info<V: View + ?Sized>(
    view: &V,
    netuid: NetUid,
) -> Result<Option<SubnetEmissionWeightsInfo>, ReadError> {
    view.call(
        &api::subnet_info_runtime_api::GET_SUBNET_EMISSION_WEIGHTS,
        (netuid,),
    )
    .await?
    .as_ref()
    .map(SubnetEmissionWeightsInfo::decode)
    .transpose()
}

/// The subnet's emission weight vector with the bounds that apply to it.
///
/// Absent when the subnet does not exist; the root subnet has no vector.
/// `min_validator` moves with root stake, alpha issuance and the global TAO
/// weight, so it is a snapshot of the pinned block.
// py: sdk/python/bittensor/reads/subnets.py:254-303
pub async fn subnet_emission_weights<V: View + ?Sized>(
    view: &V,
    netuid: NetUid,
) -> Result<Option<SubnetEmissionWeights>, ReadError> {
    let snapshot = view.pin().await?;
    let Some(info) = subnet_emission_weights_info(&snapshot, netuid).await? else {
        return Ok(None);
    };
    let inputs = root_preservation_inputs(&snapshot, netuid).await?;
    let last_change = snapshot
        .storage(
            &st::subtensor_module::LAST_RATE_LIMITED_BLOCK,
            (emission_weights_rate_limit_key(netuid),),
        )
        .await?;
    let weights = info.weights;
    let baseline_parts = info.root_baseline_share;
    let root_parts = info.root_share.min(weights[ew::VALIDATOR]);
    Ok(Some(SubnetEmissionWeights {
        netuid: info.netuid,
        weights,
        shares: ew::shares(&weights),
        burn: info.burn,
        recycle: info.recycle,
        owner: info.owner,
        validator: info.validator,
        miner: info.miner,
        global_tao_weight: inputs.global_tao_weight,
        created_via_lease: info.created_via_lease,
        root_tao: inputs.root_tao,
        alpha_issuance: inputs.alpha_issuance,
        root_share: ew::share_to_float(root_parts),
        baseline_root_share: ew::share_to_float(baseline_parts),
        baseline_root_parts: baseline_parts,
        validator_share_after_root: ew::share_to_float(weights[ew::VALIDATOR] - root_parts),
        root_capped: baseline_parts > weights[ew::VALIDATOR],
        min_validator: ew::min_validator_parts(baseline_parts),
        block: snapshot.block().number,
        last_change_block: last_change,
    }))
}

// py: sdk/python/bittensor/reads/base.py:169-184
pub(crate) fn utf8_text(value: JsonValue) -> Result<JsonValue, ReadError> {
    let bytes = match &value {
        JsonValue::String(text) if text.starts_with("0x") => {
            Some(crate::commitments::python_fromhex(&text[2..], "text")?)
        }
        JsonValue::Array(values) => values
            .iter()
            .map(|value| match value {
                JsonValue::Number(number) => {
                    let truncated = number.as_f64()?.trunc();
                    (0.0..256.0).contains(&truncated).then_some(truncated as u8)
                }
                JsonValue::Bool(value) => Some(u8::from(*value)),
                JsonValue::String(value) => {
                    // CPython's default decimal-int limit counts digits,
                    // including leading zeros, but excludes signs/separators.
                    // CPython v3.12.11 longobject.c:2630-2700 (git blob
                    // c366034fe4bf90d7f2680647f38b47703a2080f3); default 4300
                    // is defined in Include/internal/pycore_long.h:29.
                    // Invalid syntax is rejected by parse_integer below.
                    let digits = value
                        .chars()
                        .filter(|character| {
                            !character.is_whitespace() && !matches!(*character, '_' | '+' | '-')
                        })
                        .take(4301)
                        .count();
                    if digits > 4300 {
                        None
                    } else {
                        rtao_primitives::pyfmt::parse_integer(value)?.to_u8()
                    }
                }
                _ => None,
            })
            .collect::<Option<Vec<_>>>(),
        _ => None,
    };
    let Some(bytes) = bytes else { return Ok(value) };
    Ok(JsonValue::String(String::from_utf8(bytes).unwrap_or_else(
        |error| format!("0x{}", hex::encode(error.into_bytes())),
    )))
}

/// Whether the subnet requires commit-reveal weights.
pub async fn commit_reveal_enabled<V: View + ?Sized>(
    view: &V,
    netuid: NetUid,
) -> Result<bool, ReadError> {
    Ok(view
        .storage(
            &rtao_runtime::storage::subtensor_module::COMMIT_REVEAL_WEIGHTS_ENABLED,
            (netuid,),
        )
        .await?)
}
/// Current mechanism count; unlike a mechanism index, the count may be sixteen.
pub async fn mechanism_count<V: View + ?Sized>(view: &V, netuid: NetUid) -> Result<u8, ReadError> {
    Ok(view
        .storage(
            &rtao_runtime::storage::subtensor_module::MECHANISM_COUNT_CURRENT,
            (netuid,),
        )
        .await?
        .0)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)] // Test failures identify unexpected fixture values.
mod tests {
    use super::*;
    use crate::commitments::CommitmentError;
    use serde_json::json;

    #[test]
    fn metagraph_only_normalizes_present_name_and_symbol() {
        let graph =
            normalize_metagraph(json!({"name": [207, 132], "symbol": "0xff", "other": [1, 2]}))
                .unwrap();
        assert_eq!(
            graph,
            json!({"name": "τ", "symbol": "0xff", "other": [1, 2]})
        );
        assert_eq!(
            normalize_metagraph(JsonValue::Null).unwrap(),
            JsonValue::Null
        );
        assert_eq!(normalize_metagraph(json!({})).unwrap(), json!({}));
    }

    #[test]
    fn utf8_text_retains_invalid_nonbyte_sequences_and_reports_invalid_hex() {
        assert_eq!(utf8_text(json!(["256"])).unwrap(), json!(["256"]));
        assert_eq!(utf8_text(json!([true, "65"])).unwrap(), json!("\u{1}A"));
        assert_eq!(utf8_text(json!("raw")).unwrap(), json!("raw"));
        let ReadError::Commitment(CommitmentError { message, .. }) =
            utf8_text(json!("0xz")).unwrap_err()
        else {
            panic!("wrong error")
        };
        assert!(message.contains("position 0"));
    }

    #[test]
    fn utf8_text_hex_accepts_vertical_tabs_and_keeps_python_non_ascii_priority() {
        assert_eq!(
            utf8_text(json!("0x61\u{b}62\u{b}63")).unwrap(),
            json!("abc")
        );
        for (input, position) in [("0xggé", 2), ("0x6\u{b}1", 1)] {
            let ReadError::Commitment(CommitmentError { field, message }) =
                utf8_text(json!(input)).unwrap_err()
            else {
                panic!("wrong error")
            };
            assert_eq!(field, "text");
            assert_eq!(
                message,
                format!("non-hexadecimal number found in fromhex() arg at position {position}")
            );
        }
    }

    #[test]
    fn utf8_text_retains_arrays_above_pythons_default_decimal_digit_limit() {
        for numeral in [
            "0".repeat(4300),
            format!("\u{2003}+{}\u{a0}", "0_".repeat(4299) + "0"),
            "٠".repeat(4300),
        ] {
            assert_eq!(
                utf8_text(json!([65, numeral, 66])).unwrap(),
                json!("A\u{0}B")
            );
        }
        for numeral in [
            "0".repeat(4301),
            format!("\u{2003}-{}\u{a0}", "0_".repeat(4300) + "0"),
            "٠".repeat(4301),
        ] {
            let original = json!([65, numeral, 66]);
            assert_eq!(utf8_text(original.clone()).unwrap(), original);
        }
    }

    #[test]
    fn utf8_text_truncates_numeric_items_before_checking_byte_bounds() {
        assert_eq!(
            utf8_text(json!([65.9, 66.1, -0.9, true, false])).unwrap(),
            json!("AB\u{0}\u{1}\u{0}")
        );
        assert_eq!(utf8_text(json!([255.999])).unwrap(), json!("0xff"));
        for item in [json!(256.0), json!(-1.0), json!(f64::MAX), json!(u128::MAX)] {
            let original = json!([65, item, 66]);
            assert_eq!(utf8_text(original.clone()).unwrap(), original);
        }
    }

    #[test]
    fn utf8_text_accepts_python_signed_underscored_unicode_decimal_items() {
        assert_eq!(
            utf8_text(json!([
                " \t+6_5\n",
                "\u{2003}٠٦٦\u{a0}",
                "６_７",
                "𝟞𝟠",
                "-٠"
            ]))
            .unwrap(),
            json!("ABCD\u{0}")
        );
        assert_eq!(utf8_text(json!([128.9, "2_5_5"])).unwrap(), json!("0x80ff"));
    }

    #[test]
    fn utf8_text_keeps_the_entire_original_array_when_integer_coercion_fails() {
        for spelling in [
            "",
            "_65",
            "65_",
            "6__5",
            "+ 65",
            "65.0",
            "6.5e1",
            "0x41",
            "²",
            "Ⅵ",
            "\u{1c}65",
            "65\u{1f}",
            "-1",
            "256",
            "99999999999999999999999999999999999999",
            "-99999999999999999999999999999999999999",
        ] {
            let original = json!([65, spelling, 66]);
            assert_eq!(
                utf8_text(original.clone()).unwrap(),
                original,
                "{spelling:?}"
            );
        }
        for item in [json!(null), json!([]), json!({"byte": 65})] {
            let original = json!([65, item, 66]);
            assert_eq!(utf8_text(original.clone()).unwrap(), original);
        }
        assert_eq!(utf8_text(json!([])).unwrap(), json!(""));
    }
}
