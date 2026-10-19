// Derived from RaoFoundation/subtensor sdk/python/bittensor/reads/subnets.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/reads/base.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Subnet catalog reads over pinned client views.

use std::collections::BTreeMap;

use futures::TryStreamExt;
use num_traits::ToPrimitive;
use rtao_client::{View, ViewExt};
use rtao_core::{DynValue, codec::Value};
use rtao_primitives::NetUid;
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

// py: sdk/python/bittensor/reads/base.py:169-184
pub(crate) fn utf8_text(value: JsonValue) -> Result<JsonValue, ReadError> {
    let bytes = match &value {
        // The core decoder renders a byte string as UTF-8 text when it is
        // valid and as 0x-hex otherwise, so "0x" text that is not hex (a subnet
        // named "0xgrizz") is already decoded and stays as it is (D-079).
        JsonValue::String(text) if text.starts_with("0x") => {
            match rtao_primitives::pyfmt::bytes_fromhex(&text[2..]) {
                Ok(bytes) => Some(bytes),
                Err(_) => return Ok(value),
            }
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
    fn utf8_text_retains_invalid_nonbyte_sequences_and_decoded_0x_text() {
        assert_eq!(utf8_text(json!(["256"])).unwrap(), json!(["256"]));
        assert_eq!(utf8_text(json!([true, "65"])).unwrap(), json!("\u{1}A"));
        assert_eq!(utf8_text(json!("raw")).unwrap(), json!("raw"));
        // The decoder emits valid UTF-8 byte strings as text, so a chain value
        // that reads "0xgrizz" arrives here already decoded.
        assert_eq!(utf8_text(json!("0xz")).unwrap(), json!("0xz"));
        assert_eq!(utf8_text(json!("0xgrizz")).unwrap(), json!("0xgrizz"));
    }

    #[test]
    fn utf8_text_hex_accepts_vertical_tabs_and_keeps_non_hex_text() {
        assert_eq!(
            utf8_text(json!("0x61\u{b}62\u{b}63")).unwrap(),
            json!("abc")
        );
        for input in ["0xggé", "0x6\u{b}1"] {
            assert_eq!(utf8_text(json!(input)).unwrap(), json!(input));
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
